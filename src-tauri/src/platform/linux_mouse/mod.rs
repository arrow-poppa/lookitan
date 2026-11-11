// Mouse-button shortcuts on Linux, on X11 and on every Wayland desktop alike: the mice's own
// event devices (evdev) are read, so it needs no help from the desktop. One thread waits on
// every mouse that has the shortcut's button, on /dev/input (mice plugged in or out, permissions
// given or taken back) and on a wake-up for new settings, with no polling.
//
// - Watching (read access to the mice): the shortcut looks the word up and the click still
//   reaches the program under the pointer.
// - Blocking (also /dev/uinput): to keep the shortcut's click from that program, as Windows'
//   hook does, a mouse is taken for Lookitan alone (EVIOCGRAB) and everything else it sends is
//   passed on through a copy of it (uinput) that the desktop reads instead. Only once the desktop
//   is seen to have opened the copy, and only with no button held; a copy that cannot be written
//   to lets the mouse go at once, and the kernel lets it go by itself if Lookitan ends.
// - Alt, Ctrl and Shift are asked of the keyboards (their state, EVIOCGKEY; what is typed is
//   never read), only when the shortcut's button goes down.
//
// The logic (which press is the shortcut, what goes on to the copy) is in ../mouse_hotkey.rs.
// The permission, a udev rule of the user's choosing, is in setup.rs.

pub mod evdev;
pub mod setup;

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

use crate::platform::mouse_hotkey::{self as logic, Ev, Filter, Machine, Mods, RepeatGuard, BTN_EXTRA, BTN_SIDE, EV_KEY};
use crate::settings::{MouseButton, MouseHotkey};
use evdev::{Device, Info, Virtual};
use setup::Rule;

/// How long the desktop has to open a new copy before the mouse is left unblocked.
const OPEN_WAIT: Duration = Duration::from_secs(3);
/// How often a copy is looked for among the desktop's open files while it waits.
const OPEN_CHECK: Duration = Duration::from_millis(250);
/// Changes in /dev/input come in bursts (the node, then its permissions): one look after them.
const SETTLE: Duration = Duration::from_millis(150);

struct Shared {
    cfg: Mutex<MouseHotkey>,
    on_trigger: OnceLock<Box<dyn Fn() + Send + Sync>>,
    /// What the thread has: each mouse it holds, and why one is not blocking.
    held: Mutex<Vec<Held>>,
    wake: OwnedFd,
    stop: AtomicBool,
    rescan: AtomicBool,
    running: AtomicBool,
}

#[derive(Clone, Debug)]
struct Held {
    node: String,
    inode: u64,
    grabbed: bool,
    note: Option<String>,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

fn shared() -> Option<&'static Arc<Shared>> {
    if let Some(s) = SHARED.get() {
        return Some(s);
    }
    let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if fd < 0 {
        log::warn!("mouse shortcut: eventfd failed: {}", io::Error::last_os_error());
        return None;
    }
    let made = Arc::new(Shared {
        cfg: Mutex::new(MouseHotkey::default()),
        on_trigger: OnceLock::new(),
        held: Mutex::new(Vec::new()),
        wake: unsafe { OwnedFd::from_raw_fd(fd) },
        stop: AtomicBool::new(false),
        rescan: AtomicBool::new(false),
        running: AtomicBool::new(false),
    });
    let _ = SHARED.set(made);
    SHARED.get()
}

fn wake(s: &Shared) {
    let one: u64 = 1;
    unsafe { libc::write(s.wake.as_raw_fd(), (&one as *const u64).cast(), 8) };
}

/// Takes the new shortcut at once, while the app runs; the thread starts on first use.
pub fn set_mouse_hotkey(cfg: MouseHotkey, on_trigger: impl Fn() + Send + Sync + 'static) {
    let Some(s) = shared() else { return };
    let _ = s.on_trigger.set(Box::new(on_trigger));
    let enabled = cfg.button != MouseButton::None;
    *s.cfg.lock() = cfg;
    if !enabled && !s.running.load(Ordering::SeqCst) {
        return;
    }
    if !evdev::SUPPORTED {
        return;
    }
    if !s.running.swap(true, Ordering::SeqCst) {
        s.stop.store(false, Ordering::SeqCst);
        let s = Arc::clone(s);
        let started = std::thread::Builder::new().name("mouse-hotkey".into()).spawn(move || {
            run(&s);
            s.held.lock().clear();
            s.running.store(false, Ordering::SeqCst);
        });
        if let Err(e) = started {
            log::warn!("mouse shortcut: could not start its thread: {e}");
            if let Some(s) = SHARED.get() { s.running.store(false, Ordering::SeqCst); }
        }
        return;
    }
    wake(s);
}

/// Looks at the devices again (after the permission changed).
pub fn rescan() {
    if let Some(s) = SHARED.get() {
        s.rescan.store(true, Ordering::SeqCst);
        wake(s);
    }
}

/// Lets every mouse go and ends the thread (when Lookitan quits).
pub fn stop() {
    if let Some(s) = SHARED.get() {
        s.stop.store(true, Ordering::SeqCst);
        wake(s);
        // The thread lets go within one wake-up; give it a moment before the process ends.
        let until = Instant::now() + Duration::from_millis(300);
        while s.running.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

// ------------------------------------------------------------------ the thread

/// Where a mouse is on the way to being blocked.
enum Phase {
    /// Read only: its clicks also reach the programs.
    Watching,
    /// Its copy exists; waiting for the desktop to open it, then for no button to be held.
    Waiting { copy: Virtual, node: Option<String>, since: Instant, seen: bool, checked: Instant },
    /// Taken: the desktop gets its events through `copy`.
    Grabbed { copy: Virtual },
}

struct Mouse {
    info: Info,
    dev: Device,
    filter: Filter,
    phase: Phase,
    /// Why it is not blocking when it should; kept until the settings or the device change.
    note: Option<String>,
}

impl Mouse {
    fn grabbed(&self) -> bool {
        matches!(self.phase, Phase::Grabbed { .. })
    }

    /// Back to watching, the safe way: the copy lets go of what it holds, then the mouse is let
    /// go, then the copy goes.
    fn ungrab(&mut self, note: Option<String>) {
        let mut out = Vec::new();
        self.filter.release_all(&mut out);
        match std::mem::replace(&mut self.phase, Phase::Watching) {
            Phase::Grabbed { copy } => {
                let _ = copy.write(&out);
                if let Err(e) = self.dev.grab(false) {
                    log::warn!("mouse shortcut: letting {} go: {e}", self.info.name);
                }
                drop(copy);
            }
            Phase::Waiting { copy, .. } => drop(copy),
            Phase::Watching => {}
        }
        if note.is_some() {
            self.note = note;
        }
        let cfg = self.filter.machine.cfg().clone();
        self.filter.machine.set(cfg, false);
    }

    fn buttons_down(&self) -> bool {
        match self.dev.keys_down() {
            Ok(bits) => (logic::BTN_LEFT..=logic::BTN_BACK + 1).any(|c| evdev::bit(&bits, c)),
            Err(_) => true,
        }
    }
}

impl Drop for Mouse {
    fn drop(&mut self) {
        self.ungrab(None);
    }
}

/// The keyboards, opened the first time Alt, Ctrl or Shift are asked about.
#[derive(Default)]
struct Keyboards {
    known: Vec<Info>,
    open: Vec<(String, u64, Device)>,
    tried: Vec<(String, u64)>,
}

impl Keyboards {
    fn refresh(&mut self, infos: &[Info]) {
        self.known = infos.iter().filter(|i| i.keyboard()).cloned().collect();
        let known = &self.known;
        self.open.retain(|(node, inode, _)| known.iter().any(|i| &i.node == node && i.inode == *inode));
        // A permission may have come: try again the ones that failed.
        self.tried.clear();
        self.tried.extend(self.open.iter().map(|(n, i, _)| (n.clone(), *i)));
    }

    fn mods(&mut self) -> Option<Mods> {
        for info in &self.known {
            let key = (info.node.clone(), info.inode);
            if self.tried.contains(&key) {
                continue;
            }
            self.tried.push(key);
            if let Ok(dev) = Device::open(&info.path()) {
                self.open.push((info.node.clone(), info.inode, dev));
            }
        }
        let mut mods = Mods::default();
        let mut read = false;
        for (_, _, dev) in &self.open {
            if let Ok(bits) = dev.keys_down() {
                read = true;
                mods.ctrl |= evdev::bit(&bits, evdev::KEY_LEFTCTRL) || evdev::bit(&bits, evdev::KEY_RIGHTCTRL);
                mods.shift |= evdev::bit(&bits, evdev::KEY_LEFTSHIFT) || evdev::bit(&bits, evdev::KEY_RIGHTSHIFT);
                mods.alt |= evdev::bit(&bits, evdev::KEY_LEFTALT) || evdev::bit(&bits, evdev::KEY_RIGHTALT);
            }
        }
        read.then_some(mods)
    }
}

struct Inotify(OwnedFd);

impl Inotify {
    fn watch(dir: &str) -> Option<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let path = std::ffi::CString::new(dir).ok()?;
        let mask = libc::IN_CREATE | libc::IN_DELETE | libc::IN_ATTRIB | libc::IN_MOVED_TO | libc::IN_MOVED_FROM;
        if unsafe { libc::inotify_add_watch(fd.as_raw_fd(), path.as_ptr(), mask) } < 0 {
            return None;
        }
        Some(Self(fd))
    }

    fn drain(&self) {
        let mut buffer = [0u8; 4096];
        while unsafe { libc::read(self.0.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) } > 0 {}
    }
}

fn drain_wake(fd: RawFd) {
    let mut value = 0u64;
    unsafe { libc::read(fd, (&mut value as *mut u64).cast(), 8) };
}

struct Worker<'a> {
    s: &'a Shared,
    cfg: MouseHotkey,
    mice: HashMap<String, Mouse>,
    keyboards: Keyboards,
    guard: RepeatGuard,
    uinput: bool,
}

fn run(s: &Shared) {
    let inotify = Inotify::watch("/dev/input");
    if inotify.is_none() {
        log::warn!("mouse shortcut: cannot watch /dev/input; mice plugged in later need a restart");
    }
    let mut w = Worker { s, cfg: s.cfg.lock().clone(), mice: HashMap::new(), keyboards: Keyboards::default(), guard: RepeatGuard::default(), uinput: false };
    let mut rescan_at = Some(Instant::now());
    let mut events = Vec::new();
    loop {
        if s.stop.load(Ordering::SeqCst) {
            break;
        }
        let now = Instant::now();
        // Wake for the soonest of: a look at the devices, a long press, a copy to check.
        let mut due: Option<Instant> = rescan_at;
        let mut soonest = |t: Instant| due = Some(due.map_or(t, |d| d.min(t)));
        for mouse in w.mice.values() {
            if let Some(t) = mouse.filter.machine.deadline() { soonest(t); }
            if let Phase::Waiting { checked, .. } = mouse.phase { soonest(checked + OPEN_CHECK); }
        }
        // Rounded up: a deadline under a millisecond away is not a zero timeout spun on.
        let timeout = due.map_or(-1, |t| t.saturating_duration_since(now).as_micros().div_ceil(1000).min(60_000) as i32);

        let mut fds = vec![libc::pollfd { fd: s.wake.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        if let Some(i) = &inotify {
            fds.push(libc::pollfd { fd: i.0.as_raw_fd(), events: libc::POLLIN, revents: 0 });
        }
        let first_mouse = fds.len();
        let order: Vec<String> = w.mice.keys().cloned().collect();
        fds.extend(order.iter().map(|n| libc::pollfd { fd: w.mice[n].dev.fd(), events: libc::POLLIN, revents: 0 }));
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            log::warn!("mouse shortcut: poll failed: {e}");
            break;
        }
        let now = Instant::now();

        if fds[0].revents != 0 {
            drain_wake(s.wake.as_raw_fd());
            if s.stop.load(Ordering::SeqCst) {
                break;
            }
            let cfg = s.cfg.lock().clone();
            if cfg != w.cfg {
                w.configure(cfg);
                rescan_at = Some(now);
            }
            if s.rescan.swap(false, Ordering::SeqCst) {
                rescan_at = Some(now);
            }
        }
        if let Some(i) = &inotify {
            if fds[1].revents != 0 {
                i.drain();
                rescan_at = Some(rescan_at.map_or(now + SETTLE, |t| t.min(now + SETTLE)));
            }
        }
        for (k, node) in order.iter().enumerate() {
            let revents = fds[first_mouse + k].revents;
            if revents == 0 {
                continue;
            }
            if revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                w.forget(node);
                continue;
            }
            events.clear();
            let gone = w.mice.get(node).map(|m| m.dev.read(&mut events).is_err()).unwrap_or(true);
            w.feed(node, &events, now);
            if gone {
                w.forget(node);
            }
        }
        w.tick(now);
        if rescan_at.is_some_and(|t| now >= t) {
            rescan_at = None;
            w.rescan();
        }
    }
    // Everything let go: each Mouse does it as it is dropped.
    w.mice.clear();
}

impl Worker<'_> {
    fn fire(&mut self, now: Instant) {
        if !self.guard.allow(now) {
            return;
        }
        let s = SHARED.get().cloned();
        // Never wait here: a grabbed mouse's events wait with the thread.
        std::thread::spawn(move || {
            if let Some(cb) = s.as_ref().and_then(|s| s.on_trigger.get()) {
                cb();
            }
        });
    }

    fn wants_grab(&self, info: &Info) -> bool {
        self.cfg.block_click && !self.cfg.long_press && self.uinput && info.plain()
    }

    fn configure(&mut self, cfg: MouseHotkey) {
        self.cfg = cfg.clone();
        let nodes: Vec<String> = self.mice.keys().cloned().collect();
        for node in nodes {
            let info = self.mice[&node].info.clone();
            let keep = logic::code_of(cfg.button, |c| info.has_key(c)).is_some();
            if !keep {
                self.mice.remove(&node);
                continue;
            }
            let wants = self.wants_grab(&info);
            let mouse = self.mice.get_mut(&node).unwrap();
            mouse.note = None;
            if !wants && !matches!(mouse.phase, Phase::Watching) {
                mouse.ungrab(None);
            }
            let grabbed = mouse.grabbed();
            mouse.filter.machine.set(cfg.clone(), grabbed);
            if wants && matches!(mouse.phase, Phase::Watching) {
                start_copy(mouse, Instant::now());
            }
        }
        self.publish();
    }

    fn forget(&mut self, node: &str) {
        // Dropping it lets it go (Mouse::drop).
        self.mice.remove(node);
        self.publish();
    }

    fn feed(&mut self, node: &str, events: &[Ev], now: Instant) {
        let Some(mouse) = self.mice.get_mut(node) else { return };
        let keyboards = &mut self.keyboards;
        let mut mods = || keyboards.mods();
        let mut out = Vec::new();
        let mut fire = false;
        let mut resync = false;
        let mut released = false;
        for &ev in events {
            let fed = mouse.filter.feed(ev, now, &mut mods, &mut out);
            fire |= fed.fire;
            released |= ev.kind == EV_KEY && ev.value == 0;
            if fed.resync {
                resync = true;
                match mouse.dev.keys_down() {
                    Ok(bits) => mouse.filter.resync(|c| evdev::bit(&bits, c), &mut out),
                    Err(_) => mouse.filter.resync(|_| false, &mut out),
                }
            }
        }
        let mut failed = None;
        if let Phase::Grabbed { copy } = &mouse.phase {
            if let Err(e) = copy.write(&out) {
                failed = Some(e);
            }
        }
        if let Some(e) = failed {
            log::warn!("mouse shortcut: the copy of {} took no events ({e}); letting the mouse go", mouse.info.name);
            mouse.ungrab(Some("A cópia do mouse parou de receber eventos; o clique não é mais bloqueado.".into()));
            self.publish();
        } else if released || resync {
            // A mouse waiting for its buttons to be let go can be taken now.
            if matches!(mouse.phase, Phase::Waiting { seen: true, .. }) {
                try_grab(mouse);
                self.publish();
            }
        }
        if fire {
            self.fire(now);
        }
    }

    fn tick(&mut self, now: Instant) {
        let mut fire = false;
        let mut changed = false;
        for mouse in self.mice.values_mut() {
            fire |= mouse.filter.machine.tick(now);
            if let Phase::Waiting { node, since, seen, checked, .. } = &mut mouse.phase {
                if now < *checked + OPEN_CHECK {
                    continue;
                }
                *checked = now;
                if !*seen {
                    *seen = node.as_deref().is_some_and(opened_elsewhere);
                    if !*seen && now.duration_since(*since) >= OPEN_WAIT {
                        log::info!("mouse shortcut: nothing opened the copy of {}; not blocking", mouse.info.name);
                        mouse.ungrab(Some("A área de trabalho não abriu a cópia do mouse, então o clique não é bloqueado.".into()));
                        changed = true;
                        continue;
                    }
                }
                if matches!(mouse.phase, Phase::Waiting { seen: true, .. }) {
                    try_grab(mouse);
                    changed = true;
                }
            }
        }
        if fire {
            self.fire(now);
        }
        if changed {
            self.publish();
        }
    }

    fn rescan(&mut self) {
        let infos = evdev::list();
        self.keyboards.refresh(&infos);
        let uinput = evdev::uinput_writable();
        let uinput_changed = uinput != self.uinput;
        self.uinput = uinput;
        let cfg = self.cfg.clone();
        let enabled = cfg.button != MouseButton::None;
        // Gone, or another device under the same name.
        self.mice.retain(|node, mouse| enabled && infos.iter().any(|i| &i.node == node && i.inode == mouse.info.inode));
        if enabled {
            for info in infos.iter().filter(|i| i.mouse()) {
                if logic::code_of(cfg.button, |c| info.has_key(c)).is_none() || self.mices_has(info) {
                    continue;
                }
                let Ok(dev) = Device::open(&info.path()) else { continue };
                if dev.inode() != info.inode {
                    continue;
                }
                let side = info.has_key(BTN_SIDE);
                let extra = info.has_key(BTN_EXTRA);
                let mut mouse = Mouse { info: info.clone(), dev, filter: Filter::new(Machine::new(cfg.clone(), false), side, extra), phase: Phase::Watching, note: None };
                if self.wants_grab(info) {
                    start_copy(&mut mouse, Instant::now());
                }
                log::info!("mouse shortcut: reading {} ({})", info.name, info.node);
                self.mices_insert(mouse);
            }
            if uinput_changed {
                // uinput came or went: the mice already read follow.
                let nodes: Vec<String> = self.mice.keys().cloned().collect();
                for node in nodes {
                    let info = self.mice[&node].info.clone();
                    let wants = self.wants_grab(&info);
                    let mouse = self.mice.get_mut(&node).unwrap();
                    if !wants && !matches!(mouse.phase, Phase::Watching) {
                        mouse.ungrab(None);
                    } else if wants && matches!(mouse.phase, Phase::Watching) && mouse.note.is_none() {
                        start_copy(mouse, Instant::now());
                    }
                }
            }
        }
        self.publish();
    }

    fn mices_has(&self, info: &Info) -> bool {
        self.mice.get(&info.node).is_some_and(|m| m.info.inode == info.inode)
    }

    fn mices_insert(&mut self, mouse: Mouse) {
        self.mice.insert(mouse.info.node.clone(), mouse);
    }

    fn publish(&self) {
        let held = self.mice.values().map(|m| Held {
            node: m.info.node.clone(),
            inode: m.info.inode,
            grabbed: m.grabbed(),
            note: m.note.clone(),
        }).collect();
        *self.s.held.lock() = held;
    }
}

/// Makes the mouse's copy; the mouse is taken once the desktop has opened it (`tick`).
fn start_copy(mouse: &mut Mouse, now: Instant) {
    let made = mouse.dev.spec().and_then(|spec| Virtual::create(&spec, evdev::OWN_PHYS));
    match made {
        Ok(copy) => {
            let node = copy.node();
            mouse.phase = Phase::Waiting { copy, node, since: now, seen: false, checked: now };
        }
        Err(e) => {
            log::warn!("mouse shortcut: no copy of {}: {e}", mouse.info.name);
            mouse.note = Some(format!("Não foi possível criar a cópia do mouse ({e}); o clique não é bloqueado."));
        }
    }
}

/// Takes the mouse, only with no button held: a button the desktop saw go down must also reach
/// it going up, or it would stay pressed there.
fn try_grab(mouse: &mut Mouse) {
    if !matches!(mouse.phase, Phase::Waiting { .. }) || mouse.buttons_down() {
        return;
    }
    let Phase::Waiting { copy, .. } = std::mem::replace(&mut mouse.phase, Phase::Watching) else { return };
    if let Err(e) = mouse.dev.grab(true) {
        log::warn!("mouse shortcut: cannot take {}: {e}", mouse.info.name);
        mouse.note = Some(if e.raw_os_error() == Some(libc::EBUSY) {
            "Outro programa já tomou este mouse para si; o clique não é bloqueado.".to_string()
        } else {
            format!("Não foi possível tomar o mouse ({e}); o clique não é bloqueado.")
        });
        return;
    }
    // A button pressed in between: let go and wait again (both ways the desktop stays right).
    if mouse.buttons_down() {
        let _ = mouse.dev.grab(false);
        mouse.phase = Phase::Waiting { copy, node: None, since: Instant::now(), seen: true, checked: Instant::now() };
        return;
    }
    let mut discard = Vec::new();
    mouse.filter.release_all(&mut discard);
    let cfg = mouse.filter.machine.cfg().clone();
    mouse.filter.machine.set(cfg, true);
    mouse.phase = Phase::Grabbed { copy };
    mouse.note = None;
    log::info!("mouse shortcut: {} taken; its other events go through the copy", mouse.info.name);
}

/// Whether a program other than Lookitan has the device node open (the desktop reading the copy).
/// Programs of other users cannot be seen, so an X server running as root does not count.
fn opened_elsewhere(node: &str) -> bool {
    let target = Path::new("/dev/input").join(node);
    let me = std::process::id().to_string();
    let Ok(procs) = std::fs::read_dir("/proc") else { return false };
    for process in procs.flatten() {
        let name = process.file_name();
        let Some(pid) = name.to_str() else { continue };
        if pid == me || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else { continue };
        if fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|p| p == target)) {
            return true;
        }
    }
    false
}

// ------------------------------------------------------------------ status

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MouseStatus {
    pub name: String,
    pub node: String,
    /// It has the shortcut's button.
    pub has_button: bool,
    pub readable: bool,
    /// Its shortcut click is kept from the programs.
    pub grabbed: bool,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// This build can read mice here.
    pub supported: bool,
    pub enabled: bool,
    /// "unsupported", "off", "no-mouse", "needs-permission", "needs-keyboard", "active".
    pub state: &'static str,
    /// One line for the settings, in Portuguese.
    pub message: String,
    pub mice: Vec<MouseStatus>,
    /// A keyboard can be asked for Alt, Ctrl and Shift.
    pub keyboard: bool,
    pub needs_keyboard: bool,
    /// /dev/uinput can be written (needed to block the click).
    pub uinput: bool,
    /// The settings ask for the click to be kept from the program.
    pub wants_blocking: bool,
    /// It is, on at least one mouse.
    pub blocking: bool,
    /// Lookitan's udev rule as installed, if it is.
    pub rule: Option<Rule>,
    pub rule_path: &'static str,
    pub can_elevate: bool,
}

/// What the shortcut can do now, looked at afresh (cheap: sysfs, and opening a node or two).
pub fn status() -> Status {
    let cfg = SHARED.get().map(|s| s.cfg.lock().clone()).unwrap_or_default();
    let held = SHARED.get().filter(|s| s.running.load(Ordering::SeqCst)).map(|s| s.held.lock().clone()).unwrap_or_default();
    let infos = if evdev::SUPPORTED { evdev::list() } else { Vec::new() };
    let enabled = cfg.button != MouseButton::None;
    let mice: Vec<MouseStatus> = infos.iter().filter(|i| i.mouse()).map(|info| {
        let mine = held.iter().find(|h| h.node == info.node && h.inode == info.inode);
        MouseStatus {
            name: info.name.clone(),
            node: info.node.clone(),
            has_button: !enabled || logic::code_of(cfg.button, |c| info.has_key(c)).is_some(),
            readable: mine.is_some() || Device::open(&info.path()).is_ok(),
            grabbed: mine.is_some_and(|h| h.grabbed),
            note: mine.and_then(|h| h.note.clone()),
        }
    }).collect();
    let keyboard = infos.iter().filter(|i| i.keyboard()).any(|i| Device::open(&i.path()).is_ok());
    let uinput = evdev::uinput_writable();
    let wants_blocking = cfg.block_click && !cfg.long_press;
    let blocking = mice.iter().any(|m| m.grabbed);
    let usable: Vec<&MouseStatus> = mice.iter().filter(|m| m.has_button).collect();
    let needs_keyboard = logic::needs_keyboard(&cfg);
    let (state, message) = if !evdev::SUPPORTED {
        ("unsupported", "Os atalhos do mouse não funcionam nesta arquitetura.".to_string())
    } else if !enabled {
        ("off", "Escolha um botão para ativar.".to_string())
    } else if usable.is_empty() {
        ("no-mouse", format!("Nenhum mouse conectado tem o botão {}.", button_name(cfg.button)))
    } else if !usable.iter().any(|m| m.readable) {
        ("needs-permission", "O Lookitan ainda não tem acesso aos botões do mouse. Dê a permissão abaixo.".to_string())
    } else if needs_keyboard && !keyboard {
        ("needs-keyboard", "Com Ctrl, Alt ou Shift, o Lookitan precisa ver se essas teclas estão pressionadas: permita o teclado abaixo.".to_string())
    } else {
        let message = if cfg.long_press {
            "Ativo: segure o botão; o clique continua chegando ao programa.".to_string()
        } else if !cfg.block_click {
            "Ativo; o clique também chega ao programa.".to_string()
        } else if blocking {
            "Ativo: o clique do atalho não chega ao programa.".to_string()
        } else if !uinput {
            "Ativo, mas o clique também chega ao programa: bloquear pede a permissão de dispositivo virtual.".to_string()
        } else if let Some(note) = usable.iter().find_map(|m| m.note.clone()) {
            format!("Ativo, mas o clique também chega ao programa. {note}")
        } else {
            "Ativo; preparando o bloqueio do clique…".to_string()
        };
        ("active", message)
    };
    Status {
        supported: evdev::SUPPORTED,
        enabled,
        state,
        message,
        mice,
        keyboard,
        needs_keyboard,
        uinput,
        wants_blocking,
        blocking,
        rule: setup::installed(),
        rule_path: setup::RULE_PATH,
        can_elevate: setup::can_elevate(),
    }
}

fn button_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::None => "",
        MouseButton::Left => "esquerdo",
        MouseButton::Right => "direito",
        MouseButton::Middle => "do meio",
        MouseButton::X1 => "lateral 1 (voltar)",
        MouseButton::X2 => "lateral 2 (avançar)",
    }
}

/// Writes or removes the permission rule (polkit asks for the password), then looks again.
pub fn setup(rule: Option<Rule>) -> Result<(), String> {
    let result = setup::apply(rule);
    rescan();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::mouse_hotkey::{Ev, BTN_MIDDLE, EV_SYN, SYN_REPORT};

    #[test]
    fn button_names_cover_every_button() {
        for b in [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::X1, MouseButton::X2] {
            assert!(!button_name(b).is_empty());
        }
    }

    #[test]
    fn a_node_nobody_has_open_is_not_seen_open() {
        assert!(!opened_elsewhere("event-lookitan-test-none"));
    }

    /// A fake mouse made with uinput, read back as the backend reads a mouse: needs write access
    /// to /dev/uinput and read access to the node it makes, so it is not run by default
    /// (`cargo test -- --ignored mouse_backend` on a machine where both are given).
    #[test]
    #[ignore]
    fn mouse_backend_reads_a_fake_mouse_and_its_copy_gets_all_but_the_shortcut_click() {
        use std::time::Duration;
        let spec = evdev::Spec {
            name: "Lookitan test mouse".into(), bustype: 0x06, vendor: 0x1234, product: 0x5678, version: 1,
            keys: vec![logic::BTN_LEFT, logic::BTN_RIGHT, BTN_MIDDLE], rel: vec![0, 1, 8], msc: vec![], props: vec![], abs: vec![],
        };
        let fake = Virtual::create(&spec, "lookitan-test/fake").expect("uinput");
        std::thread::sleep(Duration::from_millis(300));
        let node = fake.node().expect("node");
        let dev = Device::open(&Path::new("/dev/input").join(&node)).expect("read the fake mouse");
        let copy = Virtual::create(&dev.spec().unwrap(), evdev::OWN_PHYS).expect("copy");
        std::thread::sleep(Duration::from_millis(300));
        let copy_dev = Device::open(&Path::new("/dev/input").join(copy.node().unwrap())).expect("read the copy");
        dev.grab(true).expect("grab");
        let cfg = MouseHotkey { button: MouseButton::Middle, ..Default::default() };
        let mut filter = Filter::new(Machine::new(cfg, true), false, false);
        let key = |code, value| Ev { kind: EV_KEY, code, value };
        let report = Ev { kind: EV_SYN, code: SYN_REPORT, value: 0 };
        fake.write(&[Ev { kind: 2, code: 0, value: 5 }, report, key(BTN_MIDDLE, 1), report, key(BTN_MIDDLE, 0), report, key(logic::BTN_LEFT, 1), report, key(logic::BTN_LEFT, 0), report]).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let mut read = Vec::new();
        dev.read(&mut read).unwrap();
        let mut out = Vec::new();
        let mut fired = 0;
        for ev in read {
            fired += filter.feed(ev, Instant::now(), &mut || None, &mut out).fire as u32;
        }
        assert_eq!(fired, 1);
        copy.write(&out).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let mut got = Vec::new();
        copy_dev.read(&mut got).unwrap();
        let keys: Vec<(u16, i32)> = got.iter().filter(|e| e.kind == EV_KEY).map(|e| (e.code, e.value)).collect();
        assert_eq!(keys, vec![(logic::BTN_LEFT, 1), (logic::BTN_LEFT, 0)]);
        assert!(got.iter().any(|e| e.kind == 2 && e.code == 0 && e.value == 5));
        dev.grab(false).unwrap();
    }
}
