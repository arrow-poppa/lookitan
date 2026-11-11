// The mouse-button shortcut's logic for the Linux backend (linux_mouse/): which evdev code is
// which button, when a press is the shortcut (click or long press), and what of a mouse's events
// goes on to its clone while the mouse is grabbed. No I/O here, so it is tested with made-up
// events on every system. Windows keeps its own hook (win.rs).

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::time::{Duration, Instant};

use crate::settings::{MouseButton, MouseHotkey};

/// How long the button is held before a long press looks the word up, as on Windows.
pub const LONG_PRESS: Duration = Duration::from_millis(450);
/// Two presses closer than this are one look-up: the same press seen on two event nodes.
pub const REPEAT_GUARD: Duration = Duration::from_millis(200);

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const SYN_REPORT: u16 = 0;
pub const SYN_DROPPED: u16 = 3;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
pub const BTN_SIDE: u16 = 0x113;
pub const BTN_EXTRA: u16 = 0x114;
pub const BTN_FORWARD: u16 = 0x115;
pub const BTN_BACK: u16 = 0x116;

/// Which button an evdev code is on a mouse with the codes `has` answers for. Most mice send
/// BTN_SIDE and BTN_EXTRA for the two thumb buttons (X's buttons 8 and 9, "back" and "forward");
/// some send BTN_BACK and BTN_FORWARD instead, which count only on a mouse without the first pair
/// (on one with both pairs they are a sixth and seventh button).
pub fn button_of(code: u16, has: impl Fn(u16) -> bool) -> Option<MouseButton> {
    match code {
        BTN_LEFT => Some(MouseButton::Left),
        BTN_RIGHT => Some(MouseButton::Right),
        BTN_MIDDLE => Some(MouseButton::Middle),
        BTN_SIDE => Some(MouseButton::X1),
        BTN_EXTRA => Some(MouseButton::X2),
        BTN_BACK if !has(BTN_SIDE) => Some(MouseButton::X1),
        BTN_FORWARD if !has(BTN_EXTRA) => Some(MouseButton::X2),
        _ => None,
    }
}

/// The code a mouse sends for `button`, or None when it has no such button.
pub fn code_of(button: MouseButton, has: impl Fn(u16) -> bool) -> Option<u16> {
    let candidates: &[u16] = match button {
        MouseButton::None => &[],
        MouseButton::Left => &[BTN_LEFT],
        MouseButton::Right => &[BTN_RIGHT],
        MouseButton::Middle => &[BTN_MIDDLE],
        MouseButton::X1 => &[BTN_SIDE, BTN_BACK],
        MouseButton::X2 => &[BTN_EXTRA, BTN_FORWARD],
    };
    candidates.iter().copied().find(|&c| has(c))
}

/// Alt, Ctrl and Shift as held when the button went down (either side of the keyboard).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
}

/// Whether the keys held are exactly the shortcut's, as the Windows hook asks. `None` is a
/// keyboard that cannot be read: a shortcut without keys then counts whatever is held, and one
/// with keys never does (it is not guessed).
pub fn modifiers_match(cfg: &MouseHotkey, mods: Option<Mods>) -> bool {
    match mods {
        Some(m) => m.alt == cfg.alt && m.ctrl == cfg.ctrl && m.shift == cfg.shift,
        None => !needs_keyboard(cfg),
    }
}

pub fn needs_keyboard(cfg: &MouseHotkey) -> bool {
    cfg.alt || cfg.ctrl || cfg.shift
}

/// What to do with one button event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The other programs get the event. False only on a grabbed mouse, for the shortcut's click.
    pub pass: bool,
    /// Look the word up now.
    pub fire: bool,
}

const PASS: Outcome = Outcome { pass: true, fire: false };

/// One mouse's shortcut state. Click: the press looks up, and on a grabbed mouse the press and
/// its release are kept from the other programs. Long press: the press starts a timer the release
/// cancels, and the click always goes through.
#[derive(Debug)]
pub struct Machine {
    cfg: MouseHotkey,
    /// The mouse is grabbed, so a click can be kept from the other programs.
    grabbed: bool,
    /// The shortcut's button is down (its press was seen and counted).
    down: bool,
    /// The button whose press was kept back; its release is kept back too, whatever the shortcut
    /// has become meanwhile.
    swallowed: Option<MouseButton>,
    /// When the held button becomes a long press.
    pending: Option<Instant>,
}

impl Machine {
    pub fn new(cfg: MouseHotkey, grabbed: bool) -> Self {
        Self { cfg, grabbed, down: false, swallowed: None, pending: None }
    }

    /// A new shortcut, or the mouse grabbed or let go. A long press under way is given up; a
    /// press already kept back still has its release kept back while the mouse stays grabbed.
    pub fn set(&mut self, cfg: MouseHotkey, grabbed: bool) {
        self.cfg = cfg;
        self.grabbed = grabbed;
        self.down = false;
        self.pending = None;
        if !grabbed {
            self.swallowed = None;
        }
    }

    pub fn cfg(&self) -> &MouseHotkey {
        &self.cfg
    }

    /// Whether a press of `button` needs the keyboard asked: only the shortcut's own button.
    pub fn is_hotkey(&self, button: MouseButton) -> bool {
        self.cfg.button != MouseButton::None && button == self.cfg.button
    }

    fn blocks(&self) -> bool {
        self.grabbed && self.cfg.block_click && !self.cfg.long_press
    }

    pub fn press(&mut self, button: MouseButton, now: Instant, mods: Option<Mods>) -> Outcome {
        if self.swallowed == Some(button) {
            // The same press reported again before its release.
            return Outcome { pass: false, fire: false };
        }
        if !self.is_hotkey(button) {
            return PASS;
        }
        if self.down {
            return PASS;
        }
        if !modifiers_match(&self.cfg, mods) {
            return PASS;
        }
        self.down = true;
        if self.cfg.long_press {
            self.pending = Some(now + LONG_PRESS);
            return PASS;
        }
        if self.blocks() {
            self.swallowed = Some(button);
            return Outcome { pass: false, fire: true };
        }
        Outcome { pass: true, fire: true }
    }

    /// A release never looks anything up.
    pub fn release(&mut self, button: MouseButton) -> Outcome {
        if self.is_hotkey(button) {
            self.down = false;
            self.pending = None;
        }
        if self.swallowed == Some(button) {
            self.swallowed = None;
            return Outcome { pass: false, fire: false };
        }
        PASS
    }

    /// When the long press under way is due.
    pub fn deadline(&self) -> Option<Instant> {
        self.pending
    }

    /// True once, when the button has been held long enough.
    pub fn tick(&mut self, now: Instant) -> bool {
        match self.pending {
            Some(due) if now >= due => {
                self.pending = None;
                true
            }
            _ => false,
        }
    }

    /// The mouse's state is unknown again (events were lost, or it is being let go).
    pub fn reset(&mut self) {
        self.down = false;
        self.swallowed = None;
        self.pending = None;
    }
}

/// An input event without its time (the kernel stamps what is written to the clone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ev {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

impl Ev {
    pub const REPORT: Ev = Ev { kind: EV_SYN, code: SYN_REPORT, value: 0 };
}

/// What `Filter::feed` found in an event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fed {
    pub fire: bool,
    /// The kernel dropped events and the frame after them has ended: ask the mouse which buttons
    /// are down and call `Filter::resync`.
    pub resync: bool,
}

/// A mouse's events on their way to its clone: every event but the shortcut's click, a whole frame
/// at a time (up to SYN_REPORT), and which buttons the clone was told are down, so they can be let
/// go when the grab ends. A mouse only watched runs the same code and throws the output away.
#[derive(Debug)]
pub struct Filter {
    pub machine: Machine,
    /// The mouse has BTN_SIDE / BTN_EXTRA (see `button_of`).
    side: bool,
    extra: bool,
    frame: Vec<Ev>,
    down: Vec<u16>,
    /// After SYN_DROPPED: events are thrown away up to the next SYN_REPORT.
    dropping: bool,
}

impl Filter {
    pub fn new(machine: Machine, side: bool, extra: bool) -> Self {
        Self { machine, side, extra, frame: Vec::new(), down: Vec::new(), dropping: false }
    }

    pub fn button(&self, code: u16) -> Option<MouseButton> {
        button_of(code, |c| (c == BTN_SIDE && self.side) || (c == BTN_EXTRA && self.extra))
    }

    /// Takes one event. Finished frames are appended to `out`. `mods` is asked only for a press
    /// of the shortcut's button.
    pub fn feed(&mut self, ev: Ev, now: Instant, mods: &mut dyn FnMut() -> Option<Mods>, out: &mut Vec<Ev>) -> Fed {
        let mut fed = Fed::default();
        if ev.kind == EV_SYN && ev.code == SYN_DROPPED {
            self.frame.clear();
            self.dropping = true;
            return fed;
        }
        if ev.kind == EV_SYN && ev.code == SYN_REPORT {
            if self.dropping {
                self.dropping = false;
                self.frame.clear();
                fed.resync = true;
            } else if !self.frame.is_empty() {
                out.append(&mut self.frame);
                out.push(Ev::REPORT);
            }
            return fed;
        }
        if self.dropping {
            return fed;
        }
        if ev.kind == EV_KEY && (ev.value == 0 || ev.value == 1) {
            if let Some(button) = self.button(ev.code) {
                let outcome = if ev.value == 1 {
                    let held = if self.machine.is_hotkey(button) { mods() } else { None };
                    self.machine.press(button, now, held)
                } else {
                    self.machine.release(button)
                };
                fed.fire = outcome.fire;
                if !outcome.pass {
                    return fed;
                }
            }
            self.down.retain(|&c| c != ev.code);
            if ev.value == 1 {
                self.down.push(ev.code);
            }
        }
        self.frame.push(ev);
        fed
    }

    /// Lets go, on the clone, of every button it was told is down: before the grab ends.
    pub fn release_all(&mut self, out: &mut Vec<Ev>) {
        self.frame.clear();
        self.dropping = false;
        self.machine.reset();
        if self.down.is_empty() {
            return;
        }
        out.extend(self.down.drain(..).map(|code| Ev { kind: EV_KEY, code, value: 0 }));
        out.push(Ev::REPORT);
    }

    /// After lost events: lets go of the buttons the clone holds that the mouse no longer does.
    /// A button pressed meanwhile is not made up; its release will simply pass.
    pub fn resync(&mut self, is_down: impl Fn(u16) -> bool, out: &mut Vec<Ev>) {
        self.machine.reset();
        let (held, gone): (Vec<u16>, Vec<u16>) = self.down.iter().copied().partition(|&c| is_down(c));
        self.down = held;
        if gone.is_empty() {
            return;
        }
        out.extend(gone.into_iter().map(|code| Ev { kind: EV_KEY, code, value: 0 }));
        out.push(Ev::REPORT);
    }
}

/// Lets a look-up through only when the last one is not too recent.
#[derive(Debug, Default)]
pub struct RepeatGuard {
    last: Option<Instant>,
}

impl RepeatGuard {
    pub fn allow(&mut self, now: Instant) -> bool {
        if self.last.is_some_and(|t| now.duration_since(t) < REPEAT_GUARD) {
            return false;
        }
        self.last = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REL: u16 = 0x02;

    fn cfg(button: MouseButton) -> MouseHotkey {
        MouseHotkey { button, ..MouseHotkey::default() }
    }

    fn key(code: u16, value: i32) -> Ev {
        Ev { kind: EV_KEY, code, value }
    }

    fn none() -> Option<Mods> {
        Some(Mods::default())
    }

    /// Feeds events to a filter with no key held, returning what went to the clone and how many
    /// look-ups fired.
    fn run(filter: &mut Filter, events: &[Ev], now: Instant) -> (Vec<Ev>, usize) {
        let mut out = Vec::new();
        let mut fired = 0;
        for &ev in events {
            if filter.feed(ev, now, &mut none, &mut out).fire {
                fired += 1;
            }
        }
        (out, fired)
    }

    #[test]
    fn evdev_codes_are_the_five_buttons() {
        let plain = |c: u16| matches!(c, BTN_LEFT | BTN_RIGHT | BTN_MIDDLE | BTN_SIDE | BTN_EXTRA);
        assert_eq!(button_of(BTN_LEFT, plain), Some(MouseButton::Left));
        assert_eq!(button_of(BTN_RIGHT, plain), Some(MouseButton::Right));
        assert_eq!(button_of(BTN_MIDDLE, plain), Some(MouseButton::Middle));
        assert_eq!(button_of(BTN_SIDE, plain), Some(MouseButton::X1));
        assert_eq!(button_of(BTN_EXTRA, plain), Some(MouseButton::X2));
        assert_eq!(button_of(0x117, plain), None, "BTN_TASK is no shortcut button");
        assert_eq!(code_of(MouseButton::X1, plain), Some(BTN_SIDE));
        assert_eq!(code_of(MouseButton::None, plain), None);
    }

    #[test]
    fn back_and_forward_are_the_side_buttons_only_on_a_mouse_without_side_and_extra() {
        let other = |c: u16| matches!(c, BTN_LEFT | BTN_RIGHT | BTN_MIDDLE | BTN_BACK | BTN_FORWARD);
        assert_eq!(button_of(BTN_BACK, other), Some(MouseButton::X1));
        assert_eq!(button_of(BTN_FORWARD, other), Some(MouseButton::X2));
        assert_eq!(code_of(MouseButton::X1, other), Some(BTN_BACK));
        assert_eq!(code_of(MouseButton::X2, other), Some(BTN_FORWARD));
        let both = |_c: u16| true;
        assert_eq!(button_of(BTN_BACK, both), None, "a sixth button beside BTN_SIDE");
        assert_eq!(button_of(BTN_FORWARD, both), None);
        assert_eq!(code_of(MouseButton::X2, both), Some(BTN_EXTRA));
        let three = |c: u16| matches!(c, BTN_LEFT | BTN_RIGHT | BTN_MIDDLE);
        assert_eq!(code_of(MouseButton::X1, three), None, "a three-button mouse has no X1");
    }

    #[test]
    fn the_keys_held_must_be_exactly_the_shortcut_s() {
        let ctrl = MouseHotkey { button: MouseButton::Left, ctrl: true, ..MouseHotkey::default() };
        assert!(modifiers_match(&ctrl, Some(Mods { ctrl: true, ..Mods::default() })));
        assert!(!modifiers_match(&ctrl, Some(Mods::default())));
        assert!(!modifiers_match(&ctrl, Some(Mods { ctrl: true, shift: true, ..Mods::default() })), "one key too many");
        let all = MouseHotkey { button: MouseButton::Right, alt: true, ctrl: true, shift: true, ..MouseHotkey::default() };
        assert!(modifiers_match(&all, Some(Mods { alt: true, ctrl: true, shift: true })));
        let bare = cfg(MouseButton::Middle);
        assert!(modifiers_match(&bare, Some(Mods::default())));
        assert!(!modifiers_match(&bare, Some(Mods { alt: true, ..Mods::default() })));
    }

    #[test]
    fn an_unreadable_keyboard_never_guesses_a_shortcut_with_keys() {
        let ctrl = MouseHotkey { button: MouseButton::Left, ctrl: true, ..MouseHotkey::default() };
        assert!(!modifiers_match(&ctrl, None));
        assert!(modifiers_match(&cfg(MouseButton::Middle), None), "without keys it counts whatever is held");
        let mut m = Machine::new(ctrl, true);
        assert_eq!(m.press(MouseButton::Left, Instant::now(), None), PASS, "the click goes on as any other");
    }

    #[test]
    fn a_click_looks_up_at_once_and_its_release_never_does() {
        let now = Instant::now();
        let mut m = Machine::new(cfg(MouseButton::Middle), false);
        assert_eq!(m.press(MouseButton::Middle, now, none()), Outcome { pass: true, fire: true });
        assert_eq!(m.release(MouseButton::Middle), PASS);
        assert_eq!(m.deadline(), None);
    }

    #[test]
    fn on_a_grabbed_mouse_the_click_and_its_release_are_kept_back_as_a_pair() {
        let now = Instant::now();
        let mut m = Machine::new(cfg(MouseButton::X1), true);
        assert_eq!(m.press(MouseButton::X1, now, none()), Outcome { pass: false, fire: true });
        assert_eq!(m.release(MouseButton::X1), Outcome { pass: false, fire: false });
        // The next release of that button, with no press kept back, goes through.
        assert_eq!(m.release(MouseButton::X1), PASS);
        // With the wrong keys held neither the press nor its release is kept back.
        assert_eq!(m.press(MouseButton::X1, now, Some(Mods { shift: true, ..Mods::default() })), PASS);
        assert_eq!(m.release(MouseButton::X1), PASS);
    }

    #[test]
    fn block_click_off_looks_up_and_lets_the_click_through_even_when_grabbed() {
        let mut m = Machine::new(MouseHotkey { block_click: false, ..cfg(MouseButton::X2) }, true);
        assert_eq!(m.press(MouseButton::X2, Instant::now(), none()), Outcome { pass: true, fire: true });
        assert_eq!(m.release(MouseButton::X2), PASS);
    }

    #[test]
    fn a_long_press_fires_once_after_the_threshold_and_keeps_the_click() {
        let start = Instant::now();
        let mut m = Machine::new(MouseHotkey { long_press: true, ..cfg(MouseButton::Middle) }, true);
        assert_eq!(m.press(MouseButton::Middle, start, none()), PASS, "the click is never kept back");
        assert_eq!(m.deadline(), Some(start + LONG_PRESS));
        assert!(!m.tick(start + LONG_PRESS - Duration::from_millis(1)));
        assert!(m.tick(start + LONG_PRESS));
        assert!(!m.tick(start + LONG_PRESS * 3), "once for each press");
        assert_eq!(m.deadline(), None);
        assert_eq!(m.release(MouseButton::Middle), PASS);
        assert!(!m.tick(start + LONG_PRESS * 4), "a release looks nothing up");
    }

    #[test]
    fn a_release_before_the_threshold_gives_the_long_press_up() {
        let start = Instant::now();
        let mut m = Machine::new(MouseHotkey { long_press: true, ..cfg(MouseButton::X1) }, false);
        m.press(MouseButton::X1, start, none());
        assert_eq!(m.release(MouseButton::X1), PASS);
        assert_eq!(m.deadline(), None);
        assert!(!m.tick(start + LONG_PRESS * 2));
    }

    #[test]
    fn a_press_reported_twice_is_one_press() {
        let now = Instant::now();
        let mut click = Machine::new(cfg(MouseButton::Middle), false);
        assert!(click.press(MouseButton::Middle, now, none()).fire);
        assert!(!click.press(MouseButton::Middle, now, none()).fire);
        let mut grabbed = Machine::new(cfg(MouseButton::Middle), true);
        assert!(grabbed.press(MouseButton::Middle, now, none()).fire);
        assert_eq!(grabbed.press(MouseButton::Middle, now, none()), Outcome { pass: false, fire: false });
        let mut held = Machine::new(MouseHotkey { long_press: true, ..cfg(MouseButton::Middle) }, false);
        held.press(MouseButton::Middle, now, none());
        held.press(MouseButton::Middle, now + Duration::from_millis(300), none());
        assert_eq!(held.deadline(), Some(now + LONG_PRESS), "the second report does not restart the timer");
    }

    #[test]
    fn other_buttons_and_a_shortcut_turned_off_pass_untouched() {
        let now = Instant::now();
        let mut m = Machine::new(cfg(MouseButton::X2), true);
        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::X1] {
            assert_eq!(m.press(button, now, None), PASS);
            assert_eq!(m.release(button), PASS);
        }
        let mut off = Machine::new(cfg(MouseButton::None), true);
        for button in [MouseButton::Left, MouseButton::Middle, MouseButton::X1, MouseButton::X2] {
            assert_eq!(off.press(button, now, none()), PASS);
            assert_eq!(off.release(button), PASS);
        }
    }

    #[test]
    fn a_new_shortcut_while_the_button_is_down_still_keeps_its_release_back() {
        let now = Instant::now();
        let mut m = Machine::new(cfg(MouseButton::X1), true);
        assert!(m.press(MouseButton::X1, now, none()).fire);
        m.set(cfg(MouseButton::Middle), true);
        assert_eq!(m.release(MouseButton::X1), Outcome { pass: false, fire: false }, "no release without its press");
        assert_eq!(m.press(MouseButton::X1, now, none()), PASS, "X1 is an ordinary button now");
        assert!(m.press(MouseButton::Middle, now, none()).fire);
    }

    #[test]
    fn a_new_shortcut_gives_up_the_long_press_under_way() {
        let now = Instant::now();
        let mut m = Machine::new(MouseHotkey { long_press: true, ..cfg(MouseButton::Middle) }, false);
        m.press(MouseButton::Middle, now, none());
        m.set(MouseHotkey { long_press: true, ..cfg(MouseButton::X1) }, false);
        assert_eq!(m.deadline(), None);
        assert!(!m.tick(now + LONG_PRESS * 2));
        // Turned off altogether: the same.
        m.press(MouseButton::X1, now, none());
        m.set(cfg(MouseButton::None), false);
        assert!(!m.tick(now + LONG_PRESS * 2));
    }

    #[test]
    fn a_mouse_let_go_forgets_the_press_it_kept_back() {
        let now = Instant::now();
        let mut m = Machine::new(cfg(MouseButton::X1), true);
        m.press(MouseButton::X1, now, none());
        m.set(cfg(MouseButton::X1), false);
        assert_eq!(m.release(MouseButton::X1), PASS, "nothing is kept back on a mouse only watched");
    }

    #[test]
    fn the_clone_gets_every_frame_but_the_shortcut_s_click() {
        let now = Instant::now();
        let mut f = Filter::new(Machine::new(cfg(MouseButton::X1), true), true, true);
        let moved = [Ev { kind: REL, code: 0, value: 5 }, Ev { kind: REL, code: 1, value: -3 }, Ev::REPORT];
        // Movement, a wheel step with its high-resolution twin, and an ordinary click.
        let wheel = [Ev { kind: REL, code: 8, value: 1 }, Ev { kind: REL, code: 11, value: 120 }, Ev::REPORT];
        let click = [Ev { kind: 4, code: 4, value: 0x90001 }, key(BTN_LEFT, 1), Ev::REPORT, key(BTN_LEFT, 0), Ev::REPORT];
        for events in [&moved[..], &wheel[..], &click[..]] {
            assert_eq!(run(&mut f, events, now), (events.to_vec(), 0), "unrelated input is written as it came");
        }
        // The shortcut's press, in a frame with movement: only the movement goes on.
        let (out, fired) = run(&mut f, &[key(BTN_SIDE, 1), moved[0], Ev::REPORT], now);
        assert_eq!((out, fired), (vec![moved[0], Ev::REPORT], 1));
        // Its release alone in a frame: nothing at all, not even an empty report.
        assert_eq!(run(&mut f, &[key(BTN_SIDE, 0), Ev::REPORT], now), (vec![], 0));
    }

    #[test]
    fn a_frame_is_written_only_when_it_is_whole() {
        let now = Instant::now();
        let mut f = Filter::new(Machine::new(cfg(MouseButton::Middle), true), false, false);
        let (out, _) = run(&mut f, &[Ev { kind: REL, code: 0, value: 1 }], now);
        assert!(out.is_empty(), "nothing before SYN_REPORT");
        let (out, _) = run(&mut f, &[Ev::REPORT], now);
        assert_eq!(out, vec![Ev { kind: REL, code: 0, value: 1 }, Ev::REPORT]);
    }

    #[test]
    fn a_mouse_only_watched_looks_up_and_passes_the_click() {
        let now = Instant::now();
        let mut f = Filter::new(Machine::new(cfg(MouseButton::Middle), false), false, false);
        let events = [key(BTN_MIDDLE, 1), Ev::REPORT, key(BTN_MIDDLE, 0), Ev::REPORT];
        assert_eq!(run(&mut f, &events, now), (events.to_vec(), 1));
    }

    #[test]
    fn the_keyboard_is_asked_only_for_the_shortcut_s_own_press() {
        let now = Instant::now();
        let mut f = Filter::new(Machine::new(cfg(MouseButton::Middle), true), false, false);
        let mut asked = 0;
        let mut mods = || {
            asked += 1;
            Some(Mods::default())
        };
        let mut out = Vec::new();
        for ev in [key(BTN_LEFT, 1), key(BTN_LEFT, 0), key(BTN_MIDDLE, 0), Ev { kind: REL, code: 0, value: 1 }, Ev::REPORT] {
            f.feed(ev, now, &mut mods, &mut out);
        }
        f.feed(key(BTN_MIDDLE, 1), now, &mut mods, &mut out);
        assert_eq!(asked, 1);
    }

    #[test]
    fn ending_the_grab_lets_go_of_the_buttons_the_clone_holds() {
        let now = Instant::now();
        let mut f = Filter::new(Machine::new(cfg(MouseButton::X1), true), true, true);
        run(&mut f, &[key(BTN_LEFT, 1), Ev::REPORT, key(BTN_SIDE, 1), Ev::REPORT], now);
        let mut out = Vec::new();
        f.release_all(&mut out);
        assert_eq!(out, vec![key(BTN_LEFT, 0), Ev::REPORT], "the left button was told down, the shortcut's never was");
        let mut again = Vec::new();
        f.release_all(&mut again);
        assert!(again.is_empty(), "and only once");
        // The press kept back is forgotten with the grab.
        assert_eq!(run(&mut f, &[key(BTN_SIDE, 0), Ev::REPORT], now).0, vec![key(BTN_SIDE, 0), Ev::REPORT]);
    }

    #[test]
    fn lost_events_drop_the_frame_and_let_go_of_buttons_no_longer_down() {
        let now = Instant::now();
        let mut f = Filter::new(Machine::new(cfg(MouseButton::X1), true), true, true);
        run(&mut f, &[key(BTN_LEFT, 1), key(BTN_RIGHT, 1), Ev::REPORT], now);
        let mut out = Vec::new();
        let dropped = [Ev { kind: REL, code: 0, value: 9 }, Ev { kind: EV_SYN, code: SYN_DROPPED, value: 0 }, key(BTN_LEFT, 0)];
        for ev in dropped {
            assert_eq!(f.feed(ev, now, &mut none, &mut out), Fed::default());
        }
        assert_eq!(f.feed(Ev::REPORT, now, &mut none, &mut out), Fed { fire: false, resync: true });
        assert!(out.is_empty(), "nothing of the broken frames is written");
        // The mouse says only the right button is still down.
        f.resync(|code| code == BTN_RIGHT, &mut out);
        assert_eq!(out, vec![key(BTN_LEFT, 0), Ev::REPORT]);
        // Afterwards events flow again.
        assert_eq!(run(&mut f, &[key(BTN_RIGHT, 0), Ev::REPORT], now).0, vec![key(BTN_RIGHT, 0), Ev::REPORT]);
    }

    #[test]
    fn the_same_press_on_two_event_nodes_is_one_look_up() {
        let now = Instant::now();
        let mut guard = RepeatGuard::default();
        assert!(guard.allow(now));
        assert!(!guard.allow(now + Duration::from_millis(3)));
        assert!(!guard.allow(now + REPEAT_GUARD - Duration::from_millis(1)));
        assert!(guard.allow(now + REPEAT_GUARD));
    }
}
