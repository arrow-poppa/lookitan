// The kernel's evdev and uinput interfaces, as far as the mouse shortcut needs them: which input
// devices there are (from sysfs, which needs no permission), a device opened to read its events
// and to grab it, and a uinput copy of a device that the events are written on to. Plain ioctls
// through libc: the structures and numbers are the kernel's (linux/input.h, linux/uinput.h).

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;

use crate::platform::mouse_hotkey::{Ev, BTN_LEFT};

const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;
const EV_MSC: u16 = 0x04;
/// The event types a copy can stand in for: a mouse's. A device with any other (LEDs, force
/// feedback, switches, sounds, key repeat) is watched but never grabbed.
const PLAIN_TYPES: u32 = 1 << 0 | 1 << EV_KEY | 1 << EV_REL | 1 << EV_ABS | 1 << EV_MSC;
/// One past the highest code of each type.
const KEY_CNT: usize = 0x300;
const REL_CNT: usize = 0x10;
const ABS_CNT: usize = 0x40;
const MSC_CNT: usize = 0x08;
const PROP_CNT: usize = 0x20;

const BTN_TOOL_PEN: u16 = 0x140;
const BTN_TOOL_FINGER: u16 = 0x145;
const BTN_TOUCH: u16 = 0x14a;
const ABS_MT_POSITION_X: u16 = 0x35;
/// Ctrl, Shift and Alt, left and right.
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_RIGHTALT: u16 = 100;
const MODIFIERS: [u16; 6] = [KEY_LEFTCTRL, KEY_LEFTSHIFT, KEY_RIGHTSHIFT, KEY_LEFTALT, KEY_RIGHTCTRL, KEY_RIGHTALT];

/// The ioctl numbers below are built the way most architectures build them (asm-generic/ioctl.h);
/// PowerPC, MIPS and SPARC differ, and there the shortcut says it is not available.
pub const SUPPORTED: bool = cfg!(any(
    target_arch = "x86_64",
    target_arch = "x86",
    target_arch = "aarch64",
    target_arch = "arm",
    target_arch = "riscv64",
    target_arch = "loongarch64",
    target_arch = "s390x"
));

const NONE: u64 = 0;
const WRITE: u64 = 1;
const READ: u64 = 2;

const fn ioc(dir: u64, kind: u8, nr: u64, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((kind as u64) << 8) | nr
}

const INT: usize = std::mem::size_of::<libc::c_int>();
const EVIOCGID: u64 = ioc(READ, b'E', 0x02, std::mem::size_of::<libc::input_id>());
const fn eviocgname(len: usize) -> u64 { ioc(READ, b'E', 0x06, len) }
const fn eviocgprop(len: usize) -> u64 { ioc(READ, b'E', 0x09, len) }
const fn eviocgkey(len: usize) -> u64 { ioc(READ, b'E', 0x18, len) }
const fn eviocgbit(kind: u16, len: usize) -> u64 { ioc(READ, b'E', 0x20 + kind as u64, len) }
const fn eviocgabs(axis: u16) -> u64 { ioc(READ, b'E', 0x40 + axis as u64, std::mem::size_of::<libc::input_absinfo>()) }
const EVIOCGRAB: u64 = ioc(WRITE, b'E', 0x90, INT);
const UI_DEV_CREATE: u64 = ioc(NONE, b'U', 1, 0);
const UI_DEV_DESTROY: u64 = ioc(NONE, b'U', 2, 0);
const UI_DEV_SETUP: u64 = ioc(WRITE, b'U', 3, std::mem::size_of::<libc::uinput_setup>());
const UI_ABS_SETUP: u64 = ioc(WRITE, b'U', 4, std::mem::size_of::<libc::uinput_abs_setup>());
const UI_SET_EVBIT: u64 = ioc(WRITE, b'U', 100, INT);
const UI_SET_KEYBIT: u64 = ioc(WRITE, b'U', 101, INT);
const UI_SET_RELBIT: u64 = ioc(WRITE, b'U', 102, INT);
const UI_SET_ABSBIT: u64 = ioc(WRITE, b'U', 103, INT);
const UI_SET_MSCBIT: u64 = ioc(WRITE, b'U', 104, INT);
const UI_SET_PHYS: u64 = ioc(WRITE, b'U', 108, std::mem::size_of::<*const libc::c_char>());
const UI_SET_PROPBIT: u64 = ioc(WRITE, b'U', 110, INT);
const fn ui_get_sysname(len: usize) -> u64 { ioc(READ, b'U', 44, len) }

fn checked(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

/// An ioctl whose argument is a number.
fn ioctl_int(fd: RawFd, request: u64, value: libc::c_int) -> io::Result<()> {
    checked(unsafe { libc::ioctl(fd, request as _, value as libc::c_long) }).map(|_| ())
}

/// An ioctl that reads into, or writes from, `arg`.
fn ioctl_ptr<T>(fd: RawFd, request: u64, arg: *mut T) -> io::Result<libc::c_int> {
    checked(unsafe { libc::ioctl(fd, request as _, arg) })
}

pub fn bit(bits: &[u8], code: u16) -> bool {
    bits.get(code as usize / 8).is_some_and(|b| b >> (code % 8) & 1 == 1)
}

fn codes(bits: &[u8], count: usize) -> Vec<u16> {
    (0..count as u16).filter(|&c| bit(bits, c)).collect()
}

/// A bit of one of sysfs's capability files: the kernel's `unsigned long` words in hexadecimal,
/// the highest first ("70000 0 0 0 0" has bits 272 to 274 on a 64-bit kernel).
pub fn sysfs_bit(text: &str, code: u16, word_bits: usize) -> bool {
    let code = code as usize;
    text.split_whitespace().rev().nth(code / word_bits)
        .and_then(|word| u64::from_str_radix(word, 16).ok())
        .is_some_and(|word| word >> (code % word_bits) & 1 == 1)
}

/// The physical path every copy carries, which is how a copy is told from a mouse.
pub const OWN_PHYS: &str = "lookitan/mouse-hotkey";

/// An input device as sysfs describes it; nothing is opened to learn this.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    /// "event4": the name under /dev/input and /sys/class/input.
    pub node: String,
    /// The device node's inode: a node made again under the same name is another device.
    pub inode: u64,
    pub name: String,
    pub phys: String,
    types: String,
    keys: String,
    abs: String,
}

impl Info {
    pub fn path(&self) -> PathBuf {
        PathBuf::from("/dev/input").join(&self.node)
    }

    pub fn has_key(&self, code: u16) -> bool {
        sysfs_bit(&self.keys, code, usize::BITS as usize)
    }

    /// One of Lookitan's own copies: never read, or its events would go round in a circle.
    pub fn own(&self) -> bool {
        self.phys == OWN_PHYS
    }

    /// Touchpads, touch screens and pen tablets: their clicks are made by the desktop from the
    /// touches (a tap, two fingers for the right button), which the kernel does not show.
    pub fn touch(&self) -> bool {
        self.has_key(BTN_TOUCH) || self.has_key(BTN_TOOL_FINGER) || self.has_key(BTN_TOOL_PEN)
            || sysfs_bit(&self.abs, ABS_MT_POSITION_X, usize::BITS as usize)
    }

    /// A mouse (or trackball, or pointing stick) whose buttons can be the shortcut.
    pub fn mouse(&self) -> bool {
        self.has_key(BTN_LEFT) && !self.touch() && !self.own()
    }

    /// A keyboard that can say whether Ctrl, Alt or Shift are down.
    pub fn keyboard(&self) -> bool {
        !self.own() && MODIFIERS.iter().any(|&k| self.has_key(k))
    }

    /// Only a mouse's kinds of events: a copy can stand in for it.
    pub fn plain(&self) -> bool {
        u32::from_str_radix(self.types.trim(), 16).is_ok_and(|types| types & !PLAIN_TYPES == 0)
    }

    #[cfg(test)]
    pub fn of(node: &str, name: &str, phys: &str, types: &str, keys: &str, abs: &str) -> Self {
        Self { node: node.into(), inode: 0, name: name.into(), phys: phys.into(), types: types.into(), keys: keys.into(), abs: abs.into() }
    }
}

/// Every event device there is now, by the number in its name.
pub fn list() -> Vec<Info> {
    let Ok(entries) = std::fs::read_dir("/dev/input") else { return Vec::new() };
    let mut found: Vec<(u32, Info)> = entries.flatten().filter_map(|entry| {
        let node = entry.file_name().into_string().ok()?;
        let number = node.strip_prefix("event")?.parse::<u32>().ok()?;
        let inode = entry.metadata().ok()?.ino();
        let sys = PathBuf::from("/sys/class/input").join(&node).join("device");
        let read = |file: &str| std::fs::read_to_string(sys.join(file)).map(|s| s.trim().to_string()).unwrap_or_default();
        Some((number, Info {
            inode,
            name: read("name"),
            phys: read("phys"),
            types: read("capabilities/ev"),
            keys: read("capabilities/key"),
            abs: read("capabilities/abs"),
            node,
        }))
    }).collect();
    found.sort_by_key(|(number, _)| *number);
    found.into_iter().map(|(_, info)| info).collect()
}

/// What a copy of a device is made from: its name and ids, and every event it can send.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub name: String,
    pub bustype: u16,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
    pub keys: Vec<u16>,
    pub rel: Vec<u16>,
    pub msc: Vec<u16>,
    pub props: Vec<u16>,
    /// Each absolute axis with its range: minimum, maximum, fuzz, flat, resolution.
    pub abs: Vec<(u16, [i32; 5])>,
}

/// An event device open for reading. Opening it changes nothing for the other programs; `grab`
/// does.
pub struct Device {
    file: File,
}

impl Device {
    pub fn open(path: &std::path::Path) -> io::Result<Self> {
        if !SUPPORTED {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "arquitetura sem suporte"));
        }
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path)?;
        Ok(Self { file })
    }

    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    pub fn inode(&self) -> u64 {
        self.file.metadata().map(|m| m.ino()).unwrap_or(0)
    }

    fn bits(&self, kind: u16, count: usize) -> io::Result<Vec<u8>> {
        let mut bits = vec![0u8; count.div_ceil(8)];
        ioctl_ptr(self.fd(), eviocgbit(kind, bits.len()), bits.as_mut_ptr())?;
        Ok(bits)
    }

    /// The keys and buttons down now, a bit each. This is the device's state, not its events:
    /// it is how a keyboard is asked for Ctrl, Alt and Shift without reading what is typed.
    pub fn keys_down(&self) -> io::Result<Vec<u8>> {
        let mut bits = vec![0u8; KEY_CNT / 8];
        ioctl_ptr(self.fd(), eviocgkey(bits.len()), bits.as_mut_ptr())?;
        Ok(bits)
    }

    /// Takes the device for Lookitan alone (EVIOCGRAB): no other program, the desktop included,
    /// gets its events until it is let go, this is closed, or Lookitan ends in any way.
    pub fn grab(&self, on: bool) -> io::Result<()> {
        ioctl_int(self.fd(), EVIOCGRAB, on as libc::c_int)
    }

    /// Appends the events waiting. An error means the device is gone.
    pub fn read(&self, events: &mut Vec<Ev>) -> io::Result<()> {
        const BATCH: usize = 64;
        let size = std::mem::size_of::<libc::input_event>();
        // At most a few batches a call, so one busy device does not keep the others waiting.
        for _ in 0..8 {
            let mut buffer: [libc::input_event; BATCH] = unsafe { std::mem::zeroed() };
            let n = unsafe { libc::read(self.fd(), buffer.as_mut_ptr().cast(), size * BATCH) };
            if n < 0 {
                let error = io::Error::last_os_error();
                match error.kind() {
                    io::ErrorKind::WouldBlock => return Ok(()),
                    io::ErrorKind::Interrupted => continue,
                    _ => return Err(error),
                }
            }
            let count = n as usize / size;
            if count == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            events.extend(buffer[..count].iter().map(|e| Ev { kind: e.type_, code: e.code, value: e.value }));
            if count < BATCH {
                return Ok(());
            }
        }
        Ok(())
    }

    pub fn spec(&self) -> io::Result<Spec> {
        let fd = self.fd();
        let mut id: libc::input_id = unsafe { std::mem::zeroed() };
        ioctl_ptr(fd, EVIOCGID, &mut id as *mut libc::input_id)?;
        let mut name = [0u8; 256];
        let len = ioctl_ptr(fd, eviocgname(name.len()), name.as_mut_ptr()).unwrap_or(0).max(0) as usize;
        let name = String::from_utf8_lossy(&name[..len.min(name.len())]).trim_end_matches('\0').to_string();
        let mut props = vec![0u8; PROP_CNT / 8];
        // Kernels before 3.8 have no properties: none then.
        let _ = ioctl_ptr(fd, eviocgprop(props.len()), props.as_mut_ptr());
        let types = self.bits(0, 0x20)?;
        let of = |kind: u16, count: usize| -> io::Result<Vec<u16>> {
            if bit(&types, kind) { Ok(codes(&self.bits(kind, count)?, count)) } else { Ok(Vec::new()) }
        };
        let mut abs = Vec::new();
        for axis in of(EV_ABS, ABS_CNT)? {
            let mut info: libc::input_absinfo = unsafe { std::mem::zeroed() };
            ioctl_ptr(fd, eviocgabs(axis), &mut info as *mut libc::input_absinfo)?;
            abs.push((axis, [info.minimum, info.maximum, info.fuzz, info.flat, info.resolution]));
        }
        Ok(Spec {
            name,
            bustype: id.bustype,
            vendor: id.vendor,
            product: id.product,
            version: id.version,
            keys: of(EV_KEY, KEY_CNT)?,
            rel: of(EV_REL, REL_CNT)?,
            msc: of(EV_MSC, MSC_CNT)?,
            props: codes(&props, PROP_CNT),
            abs,
        })
    }
}

/// Whether /dev/uinput can be opened to make a copy. Opening it makes no device.
pub fn uinput_writable() -> bool {
    SUPPORTED && OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open("/dev/uinput").is_ok()
}

/// A uinput device: the copy of a grabbed mouse that the desktop reads in its place. It is gone
/// when this is dropped, and when Lookitan ends in any way (the kernel removes it with the file).
pub struct Virtual {
    file: File,
}

impl Virtual {
    /// Makes a device that sends everything `spec` lists, under its name and ids (so the desktop
    /// gives it the mouse's own speed and button settings), with `phys` as its physical path.
    pub fn create(spec: &Spec, phys: &str) -> io::Result<Self> {
        if !SUPPORTED {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "arquitetura sem suporte"));
        }
        let file = OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open("/dev/uinput")?;
        let fd = file.as_raw_fd();
        for (kind, set, list) in [(EV_KEY, UI_SET_KEYBIT, &spec.keys), (EV_REL, UI_SET_RELBIT, &spec.rel), (EV_MSC, UI_SET_MSCBIT, &spec.msc)] {
            if list.is_empty() {
                continue;
            }
            ioctl_int(fd, UI_SET_EVBIT, kind as libc::c_int)?;
            for &code in list {
                ioctl_int(fd, set, code as libc::c_int)?;
            }
        }
        if !spec.abs.is_empty() {
            ioctl_int(fd, UI_SET_EVBIT, EV_ABS as libc::c_int)?;
        }
        for &(axis, [minimum, maximum, fuzz, flat, resolution]) in &spec.abs {
            ioctl_int(fd, UI_SET_ABSBIT, axis as libc::c_int)?;
            let mut setup: libc::uinput_abs_setup = unsafe { std::mem::zeroed() };
            setup.code = axis;
            setup.absinfo = libc::input_absinfo { value: 0, minimum, maximum, fuzz, flat, resolution };
            ioctl_ptr(fd, UI_ABS_SETUP, &mut setup as *mut libc::uinput_abs_setup)?;
        }
        for &prop in &spec.props {
            ioctl_int(fd, UI_SET_PROPBIT, prop as libc::c_int)?;
        }
        let phys = std::ffi::CString::new(phys).map_err(|_| io::ErrorKind::InvalidInput)?;
        ioctl_ptr(fd, UI_SET_PHYS, phys.as_ptr() as *mut libc::c_char)?;
        let mut setup: libc::uinput_setup = unsafe { std::mem::zeroed() };
        setup.id = libc::input_id { bustype: spec.bustype, vendor: spec.vendor, product: spec.product, version: spec.version };
        let name = if spec.name.is_empty() { "Lookitan mouse" } else { spec.name.as_str() };
        // The last byte stays 0: the kernel wants the name ended.
        for (slot, byte) in setup.name.iter_mut().zip(name.bytes().take(libc::UINPUT_MAX_NAME_SIZE - 1)) {
            *slot = byte as libc::c_char;
        }
        ioctl_ptr(fd, UI_DEV_SETUP, &mut setup as *mut libc::uinput_setup)?;
        checked(unsafe { libc::ioctl(fd, UI_DEV_CREATE as _) })?;
        let device = Self { file };
        // Events are written before any mouse is grabbed for this copy: an empty report costs
        // nothing and shows the copy takes them.
        device.write(&[Ev::REPORT])?;
        Ok(device)
    }

    /// Writes whole frames. Anything short of all of it is an error: the caller then lets the
    /// mouse go at once, so the pointer is never left without its events.
    pub fn write(&self, events: &[Ev]) -> io::Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let raw: Vec<libc::input_event> = events.iter().map(|ev| {
            let mut event: libc::input_event = unsafe { std::mem::zeroed() };
            event.type_ = ev.kind;
            event.code = ev.code;
            event.value = ev.value;
            event
        }).collect();
        let bytes = std::mem::size_of_val(raw.as_slice());
        loop {
            let n = unsafe { libc::write(self.file.as_raw_fd(), raw.as_ptr().cast(), bytes) };
            if n < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            return if n as usize == bytes { Ok(()) } else { Err(io::ErrorKind::WriteZero.into()) };
        }
    }

    /// The copy's event node ("event17"): where the desktop is seen to open it.
    pub fn node(&self) -> Option<String> {
        let mut name = [0u8; 64];
        ioctl_ptr(self.file.as_raw_fd(), ui_get_sysname(name.len()), name.as_mut_ptr()).ok()?;
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        let sys = PathBuf::from("/sys/devices/virtual/input").join(std::str::from_utf8(&name[..end]).ok()?);
        std::fs::read_dir(sys).ok()?.flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .find(|n| n.starts_with("event"))
    }
}

impl Drop for Virtual {
    fn drop(&mut self) {
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY as _) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::mouse_hotkey::{BTN_EXTRA, BTN_MIDDLE, BTN_RIGHT, BTN_SIDE};

    #[test]
    fn the_ioctl_numbers_are_the_kernel_s() {
        // From linux/input.h and linux/uinput.h on x86-64 (the same on every generic architecture).
        assert_eq!(EVIOCGRAB, 0x4004_4590);
        assert_eq!(EVIOCGID, 0x8008_4502);
        assert_eq!(eviocgkey(96), 0x8060_4518);
        assert_eq!(eviocgbit(EV_KEY, 96), 0x8060_4521);
        assert_eq!(eviocgabs(0), 0x8018_4540);
        assert_eq!(UI_DEV_CREATE, 0x5501);
        assert_eq!(UI_DEV_DESTROY, 0x5502);
        assert_eq!(UI_DEV_SETUP, 0x405c_5503);
        assert_eq!(UI_ABS_SETUP, 0x401c_5504);
        assert_eq!(UI_SET_EVBIT, 0x4004_5564);
        assert_eq!(UI_SET_PROPBIT, 0x4004_556e);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(UI_SET_PHYS, 0x4008_556c);
    }

    #[test]
    fn sysfs_capabilities_are_read_a_word_at_a_time_from_the_right() {
        // The key capabilities of a three-button USB mouse on a 64-bit kernel.
        let keys = "70000 0 0 0 0";
        for code in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE] {
            assert!(sysfs_bit(keys, code, 64));
        }
        assert!(!sysfs_bit(keys, BTN_SIDE, 64));
        assert!(!sysfs_bit(keys, KEY_LEFTCTRL, 64));
        // The same mouse on a 32-bit kernel: more, shorter words.
        assert!(sysfs_bit("70000 0 0 0 0 0 0 0 0", BTN_MIDDLE, 32));
        // A five-button mouse, and a file that is empty or missing.
        assert!(sysfs_bit("1f0000 0 0 0 0", BTN_EXTRA, 64));
        assert!(!sysfs_bit("", BTN_LEFT, 64));
        assert!(!sysfs_bit("zz", 0, 64));
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn devices_are_told_apart_by_what_they_can_send() {
        let mouse = Info::of("event4", "Logitech USB Optical Mouse", "usb-0000:03:00.3-2/input0", "17", "70000 0 0 0 0", "0");
        assert!(mouse.mouse() && mouse.plain() && !mouse.keyboard() && !mouse.touch());
        // A laptop touchpad: BTN_LEFT too, but its clicks are the desktop's.
        let touchpad = Info::of("event7", "MSFT0001:00 04F3:31BE Touchpad", "i2c-MSFT0001:00", "1b", "e520 10000 0 0 0 0", "2e0800000000003");
        assert!(touchpad.touch() && !touchpad.mouse());
        let keyboard = Info::of("event3", "AT Translated Set 2 keyboard", "isa0060/serio0/input0", "120013",
            "402000007 ff803078f800d001 feffffdfffcfffff fffffffffffffffe", "0");
        assert!(keyboard.keyboard() && !keyboard.mouse() && !keyboard.plain(), "it has LEDs and key repeat");
        let power = Info::of("event1", "Power Button", "PNP0C0C/button/input0", "3", "8000 10000000000000 0", "0");
        assert!(!power.keyboard() && !power.mouse());
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn lookitan_s_own_copy_is_never_a_mouse_to_read() {
        // The copy has the mouse's name and buttons; only its physical path tells it apart.
        let copy = Info::of("event17", "Logitech USB Optical Mouse", OWN_PHYS, "17", "70000 0 0 0 0", "0");
        assert!(copy.own() && !copy.mouse() && !copy.keyboard());
        // Another program's virtual mouse is a mouse like any other.
        let other = Info::of("event18", "input-remapper mouse forwarded", "py-evdev-uinput", "17", "1f0000 0 0 0 0", "0");
        assert!(other.mouse());
    }
}
