//! Game controllers, as Lookupper has them (REA on 2026-10-23): a combination of buttons looks up
//! the word at the cursor, and with "Navegação pelo gamepad" the D-pad or the left stick moves
//! between the pop-up's controls, or the overlay's words when the pop-up is closed, the right
//! stick scrolls, and four buttons confirm, close, save the word and pronounce it. Lookupper reads
//! XInput (plus raw HID); Lookitan reads gilrs's XInput backend on Windows, which works while
//! another window has the focus, PlayStation pads' HID reports there (playstation.rs), and evdev
//! on Linux, by the standard codes when gilrs has no mapping for the pad. Unlike Lookupper, each
//! action takes any button or combination, and two more act: Explicar and sending to Anki.
//!
//! `Machine` turns the buttons held at each tick into `Signal`s and knows nothing of gilrs, so it
//! is tested without a controller.

use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

use crate::settings::GamepadSettings;

mod playstation;

/// Buttons by their Xbox names, which settings store.
pub const BUTTONS: [&str; 16] = [
    "A", "B", "X", "Y", "LB", "RB", "LT", "RT", "View", "Menu", "LS", "RS", "Up", "Down", "Left", "Right",
];

const TICK: Duration = Duration::from_millis(16);
/// A held direction moves once, then again after this, then every `REPEAT`.
const FIRST_REPEAT: Duration = Duration::from_millis(400);
const REPEAT: Duration = Duration::from_millis(120);
/// A stick counts as a direction past this.
const STICK_PRESS: f32 = 0.6;
/// The right stick scrolls past this; it scrolls up to `SCROLL_SPEED` CSS pixels a tick.
const SCROLL_DEAD: f32 = 0.2;
const SCROLL_SPEED: f32 = 22.0;
/// A capture that sees no button for this long ends empty.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction { Up, Down, Left, Right }

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action { Confirm, Close, Save, Pronounce, Explain, Anki, Meme }

#[derive(Debug, PartialEq)]
pub enum Signal {
    /// The look-up combination was pressed.
    Hotkey,
    /// The buttons a capture has seen so far, and whether it ended (all released, or timed out).
    Capture { buttons: Vec<String>, done: bool },
    Move(Direction),
    Act(Action),
    /// CSS pixels to scroll, down positive.
    Scroll(f32),
}

/// What the controllers hold at one tick: buttons by name, the left stick and the right stick's
/// vertical axis (up positive, as gilrs reports it).
#[derive(Default, Clone)]
pub struct Input {
    pub buttons: BTreeSet<&'static str>,
    pub left: (f32, f32),
    pub right_y: f32,
}

#[derive(Default)]
pub struct Machine {
    previous: BTreeSet<&'static str>,
    hotkey_held: bool,
    /// After the combination fires, nothing navigates until every button is released, so the
    /// buttons of the combination do not also confirm or close (Lookupper waits the same way).
    wait_release: bool,
    held: Option<(Direction, Instant, bool)>,
    capture: Option<(BTreeSet<&'static str>, Instant)>,
    /// An action whose buttons are also part of a longer binding (LB alone and LB + A) waits
    /// for its buttons to be let go, and fires then only if no other button joined them.
    pending: Option<(Action, Vec<&'static str>, bool)>,
}

impl Machine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_capture(&mut self, now: Instant) {
        self.capture = Some((BTreeSet::new(), now));
    }

    pub fn stop_capture(&mut self) {
        self.capture = None;
    }

    /// `navigate` is false when nothing is on screen to navigate, so buttons pass through.
    pub fn step(&mut self, input: &Input, config: &GamepadSettings, navigate: bool, now: Instant) -> Vec<Signal> {
        let mut out = Vec::new();
        let pressed = &input.buttons;
        if let Some((seen, started)) = &mut self.capture {
            let before = seen.len();
            seen.extend(pressed.iter().copied());
            let grew = seen.len() > before;
            let names: Vec<String> = seen.iter().map(|b| b.to_string()).collect();
            let timed_out = names.is_empty() && now.duration_since(*started) > CAPTURE_TIMEOUT;
            if !names.is_empty() && pressed.is_empty() {
                out.push(Signal::Capture { buttons: names, done: true });
                self.capture = None;
                // The buttons just captured must not look up at once.
                self.hotkey_held = true;
            } else if timed_out {
                out.push(Signal::Capture { buttons: Vec::new(), done: true });
                self.capture = None;
            } else if grew {
                out.push(Signal::Capture { buttons: names, done: false });
            }
            self.previous = pressed.clone();
            return out;
        }

        let combo = names(&config.hotkey);
        let holds_combo = !combo.is_empty() && combo.iter().all(|b| pressed.contains(*b));
        if holds_combo && !self.hotkey_held {
            out.push(Signal::Hotkey);
            self.wait_release = true;
            // An action waiting for its buttons' release was the start of this combination.
            self.pending = None;
        }
        self.hotkey_held = holds_combo;
        if self.wait_release {
            if pressed.is_empty() { self.wait_release = false; }
            self.held = None;
            self.previous = pressed.clone();
            return out;
        }

        if config.navigation && navigate {
            let actions = bindings(config);
            let holds = |set: &BTreeSet<&'static str>, combo: &[&'static str]| !combo.is_empty() && combo.iter().all(|b| set.contains(*b));
            // A shorter binding inside a longer one (the look-up combination counts) waits for release.
            let longer: Vec<Vec<&'static str>> = actions.iter().map(|(c, _)| c.clone()).chain(std::iter::once(names(&config.hotkey))).collect();
            let inside = |combo: &[&'static str]| longer.iter().any(|l| l.len() > combo.len() && combo.iter().all(|b| l.contains(b)));
            if let Some((action, combo, spoiled)) = &mut self.pending {
                if pressed.iter().any(|b| !combo.contains(b)) { *spoiled = true; }
                if !holds(pressed, combo) {
                    if !*spoiled { out.push(Signal::Act(*action)); }
                    self.pending = None;
                }
            }
            let completed: Vec<_> = actions.iter().filter(|(c, _)| holds(pressed, c) && !holds(&self.previous, c)).collect();
            let longest = completed.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
            for (combo, action) in completed.into_iter().filter(|(c, _)| c.len() == longest) {
                if inside(combo) {
                    self.pending = Some((*action, combo.clone(), false));
                } else {
                    out.push(Signal::Act(*action));
                }
            }
            // A direction that is part of an action held now does not also move.
            let direction_bound = actions.iter().any(|(c, _)| holds(pressed, c) && c.iter().any(|b| matches!(*b, "Up" | "Down" | "Left" | "Right")));
            match direction(input) {
                _ if direction_bound => self.held = None,
                Some(dir) => match self.held {
                    Some((held, since, repeated)) if held == dir => {
                        let wait = if repeated { REPEAT } else { FIRST_REPEAT };
                        if now.duration_since(since) >= wait {
                            out.push(Signal::Move(dir));
                            self.held = Some((dir, now, true));
                        }
                    }
                    _ => {
                        out.push(Signal::Move(dir));
                        self.held = Some((dir, now, false));
                    }
                },
                None => self.held = None,
            }
            if input.right_y.abs() > SCROLL_DEAD {
                let v = (input.right_y.abs() - SCROLL_DEAD) / (1.0 - SCROLL_DEAD);
                out.push(Signal::Scroll(-input.right_y.signum() * v * v * SCROLL_SPEED));
            }
        } else {
            self.held = None;
            self.pending = None;
        }
        self.previous = pressed.clone();
        out
    }
}

/// Known button names of a binding, as the static names `Input` holds.
fn names(buttons: &[String]) -> Vec<&'static str> {
    buttons.iter().filter_map(|b| BUTTONS.iter().copied().find(|k| k == b)).collect()
}

/// Each action with its buttons; actions left without buttons are off.
fn bindings(config: &GamepadSettings) -> Vec<(Vec<&'static str>, Action)> {
    [(&config.confirm, Action::Confirm), (&config.close, Action::Close), (&config.save, Action::Save),
     (&config.pronounce, Action::Pronounce), (&config.explain, Action::Explain), (&config.anki, Action::Anki),
     (&config.meme, Action::Meme)]
        .into_iter().map(|(b, a)| (names(b), a)).filter(|(c, _)| !c.is_empty()).collect()
}

/// The D-pad, or the left stick past `STICK_PRESS` along its stronger axis.
fn direction(input: &Input) -> Option<Direction> {
    let b = &input.buttons;
    for (name, dir) in [("Up", Direction::Up), ("Down", Direction::Down), ("Left", Direction::Left), ("Right", Direction::Right)] {
        if b.contains(name) { return Some(dir); }
    }
    let (x, y) = input.left;
    if x.abs().max(y.abs()) < STICK_PRESS { return None; }
    Some(if x.abs() > y.abs() {
        if x > 0.0 { Direction::Right } else { Direction::Left }
    } else if y > 0.0 { Direction::Up } else { Direction::Down })
}

struct Shared {
    config: GamepadSettings,
    capture: Option<bool>,
    connected: Option<String>,
}

fn shared() -> &'static Mutex<Shared> {
    static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();
    SHARED.get_or_init(|| Mutex::new(Shared { config: GamepadSettings::default(), capture: None, connected: None }))
}

pub fn configure(config: &GamepadSettings) {
    shared().lock().config = config.clone();
}

/// Starts or stops recording a combination for the settings.
pub fn capture(on: bool) {
    shared().lock().capture = Some(on);
}

/// The name of a connected controller, if any.
pub fn connected() -> Option<String> {
    shared().lock().connected.clone()
}

/// Reads the controllers on a thread of its own. `navigate()` says whether a pop-up or an overlay
/// is on screen; `signal` receives what the buttons mean.
pub fn start(navigate: impl Fn() -> bool + Send + 'static, signal: impl Fn(Signal) + Send + 'static) {
    #[cfg(windows)]
    std::thread::spawn(playstation::run);
    std::thread::spawn(move || {
        let mut gilrs = match gilrs::Gilrs::new() {
            Ok(g) => g,
            Err(gilrs::Error::NotImplemented(g)) => g,
            Err(e) => {
                log::warn!("gamepad: {e}");
                return;
            }
        };
        let mut machine = Machine::new();
        let mut raw: HashMap<gilrs::GamepadId, Input> = HashMap::new();
        loop {
            if let Some(event) = gilrs.next_event_blocking(Some(TICK)) {
                raw_event(&mut raw, event);
                while let Some(event) = gilrs.next_event() {
                    raw_event(&mut raw, event);
                }
            }
            let hid = playstation::state();
            let (config, capture) = {
                let mut s = shared().lock();
                s.connected = gilrs.gamepads().find(|(_, g)| g.is_connected()).map(|(_, g)| g.name().to_string())
                    .or_else(|| hid.as_ref().map(|(name, _)| name.clone()));
                (s.config.clone(), s.capture.take())
            };
            match capture {
                Some(true) => machine.start_capture(Instant::now()),
                Some(false) => machine.stop_capture(),
                None => {}
            }
            let mut input = read(&gilrs);
            for extra in raw.values().chain(hid.as_ref().map(|(_, input)| input)) {
                merge(&mut input, extra);
            }
            let busy = !input.buttons.is_empty() || input.left.0.abs().max(input.left.1.abs()) >= STICK_PRESS || input.right_y.abs() > SCROLL_DEAD;
            let navigate = config.navigation && busy && navigate();
            for s in machine.step(&input, &config, navigate, Instant::now()) {
                signal(s);
            }
        }
    });
}

/// Adds what another source holds: its buttons, and each stick where it is pushed further.
fn merge(input: &mut Input, other: &Input) {
    input.buttons.extend(other.buttons.iter().copied());
    if other.left.0.abs().max(other.left.1.abs()) > input.left.0.abs().max(input.left.1.abs()) { input.left = other.left; }
    if other.right_y.abs() > input.right_y.abs() { input.right_y = other.right_y; }
}

fn read(gilrs: &gilrs::Gilrs) -> Input {
    use gilrs::{Axis, Button};
    const NAMES: [(Button, &str); 16] = [
        (Button::South, "A"), (Button::East, "B"), (Button::West, "X"), (Button::North, "Y"),
        (Button::LeftTrigger, "LB"), (Button::RightTrigger, "RB"), (Button::LeftTrigger2, "LT"), (Button::RightTrigger2, "RT"),
        (Button::Select, "View"), (Button::Start, "Menu"), (Button::LeftThumb, "LS"), (Button::RightThumb, "RS"),
        (Button::DPadUp, "Up"), (Button::DPadDown, "Down"), (Button::DPadLeft, "Left"), (Button::DPadRight, "Right"),
    ];
    let mut input = Input::default();
    for (_, pad) in gilrs.gamepads() {
        if !pad.is_connected() { continue; }
        let mut own = Input::default();
        for (button, name) in NAMES {
            if pad.is_pressed(button) { own.buttons.insert(name); }
        }
        // A D-pad some pads report as an axis (a hat) rather than as four buttons.
        hat(&mut own, pad.value(Axis::DPadX), pad.value(Axis::DPadY));
        own.left = (pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
        own.right_y = pad.value(Axis::RightStickY);
        merge(&mut input, &own);
    }
    input
}

const HAT_PRESS: f32 = 0.5;

/// A hat's two axes, up positive, as D-pad buttons.
fn hat(input: &mut Input, x: f32, y: f32) {
    if x > HAT_PRESS { input.buttons.insert("Right"); }
    if x < -HAT_PRESS { input.buttons.insert("Left"); }
    if y > HAT_PRESS { input.buttons.insert("Up"); }
    if y < -HAT_PRESS { input.buttons.insert("Down"); }
}

/// Buttons and axes gilrs has no mapping for (a generic pad on Linux), known by their standard
/// evdev codes, as Harbor (arrow-poppa/streaming, MIT) reads them.
fn raw_event(raw: &mut HashMap<gilrs::GamepadId, Input>, event: gilrs::Event) {
    use gilrs::{Axis, Button, EventType};
    if matches!(event.event, EventType::Disconnected) {
        raw.remove(&event.id);
        return;
    }
    let pad = raw.entry(event.id).or_default();
    match event.event {
        EventType::ButtonPressed(Button::Unknown, code) => {
            if let Some(name) = evdev_button(code.into_u32()) { pad.buttons.insert(name); }
        }
        EventType::ButtonReleased(Button::Unknown, code) => {
            if let Some(name) = evdev_button(code.into_u32()) { pad.buttons.remove(name); }
        }
        EventType::AxisChanged(Axis::Unknown, value, code) => {
            // gilrs does not turn an unknown axis, so its Y keeps evdev's down positive.
            match evdev_axis(code.into_u32()) {
                Some(RawAxis::LeftX) => pad.left.0 = value,
                Some(RawAxis::LeftY) => pad.left.1 = -value,
                Some(RawAxis::RightY) => pad.right_y = -value,
                Some(RawAxis::HatX) => {
                    pad.buttons.remove("Left");
                    pad.buttons.remove("Right");
                    if value > HAT_PRESS { pad.buttons.insert("Right"); } else if value < -HAT_PRESS { pad.buttons.insert("Left"); }
                }
                Some(RawAxis::HatY) => {
                    pad.buttons.remove("Up");
                    pad.buttons.remove("Down");
                    if value < -HAT_PRESS { pad.buttons.insert("Up"); } else if value > HAT_PRESS { pad.buttons.insert("Down"); }
                }
                None => {}
            }
        }
        _ => {}
    }
}

const EV_KEY: u32 = 1;
const EV_ABS: u32 = 3;

/// A Linux evdev key code (`EV_KEY << 16 | code`, as gilrs packs it) by Xbox name. Other
/// platforms' codes never match.
fn evdev_button(raw: u32) -> Option<&'static str> {
    if !cfg!(target_os = "linux") || raw >> 16 != EV_KEY { return None; }
    Some(match raw & 0xFFFF {
        0x130 => "A", 0x131 => "B", 0x133 => "Y", 0x134 => "X",
        0x136 => "LB", 0x137 => "RB", 0x138 => "LT", 0x139 => "RT",
        0x13a => "View", 0x13b => "Menu", 0x13d => "LS", 0x13e => "RS",
        0x220 => "Up", 0x221 => "Down", 0x222 => "Left", 0x223 => "Right",
        _ => return None,
    })
}

#[derive(Debug, PartialEq)]
enum RawAxis { LeftX, LeftY, RightY, HatX, HatY }

fn evdev_axis(raw: u32) -> Option<RawAxis> {
    if !cfg!(target_os = "linux") || raw >> 16 != EV_ABS { return None; }
    Some(match raw & 0xFFFF {
        0x00 => RawAxis::LeftX,
        0x01 => RawAxis::LeftY,
        0x04 => RawAxis::RightY,
        0x10 => RawAxis::HatX,
        0x11 => RawAxis::HatY,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(buttons: &[&'static str]) -> Input {
        Input { buttons: buttons.iter().copied().collect(), ..Input::default() }
    }

    fn config(hotkey: &[&str]) -> GamepadSettings {
        GamepadSettings { hotkey: hotkey.iter().map(|s| s.to_string()).collect(), ..GamepadSettings::default() }
    }

    #[test]
    fn the_combination_fires_once_and_its_buttons_do_not_navigate_until_released() {
        let c = config(&["LB", "A"]);
        let mut m = Machine::new();
        let t = Instant::now();
        assert!(m.step(&input(&["LB"]), &c, true, t).is_empty());
        assert_eq!(m.step(&input(&["LB", "A"]), &c, true, t), vec![Signal::Hotkey]);
        assert!(m.step(&input(&["LB", "A"]), &c, true, t).is_empty(), "held, it does not repeat");
        assert!(m.step(&input(&["A"]), &c, true, t).is_empty(), "A does not confirm while the combination is let go");
        assert!(m.step(&input(&[]), &c, true, t).is_empty());
        // A is also in LB + A, so Confirm waits for A to be let go alone.
        assert!(m.step(&input(&["A"]), &c, true, t).is_empty());
        assert_eq!(m.step(&input(&[]), &c, true, t), vec![Signal::Act(Action::Confirm)]);
    }

    #[test]
    fn actions_follow_the_chosen_buttons_and_need_something_on_screen() {
        let mut c = config(&[]);
        c.save = vec!["LT".into()];
        let mut m = Machine::new();
        let t = Instant::now();
        assert!(m.step(&input(&["B"]), &c, false, t).is_empty(), "nothing on screen");
        m.step(&input(&[]), &c, true, t);
        assert_eq!(m.step(&input(&["B"]), &c, true, t), vec![Signal::Act(Action::Close)]);
        m.step(&input(&[]), &c, true, t);
        assert_eq!(m.step(&input(&["LT"]), &c, true, t), vec![Signal::Act(Action::Save)]);
        assert!(m.step(&input(&["X"]), &c, true, t).is_empty(), "X saves no more");
        c.navigation = false;
        assert!(m.step(&input(&["Y"]), &c, true, t).is_empty(), "navigation turned off");
    }

    fn combo(buttons: &[&str]) -> Vec<String> {
        buttons.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_combination_fires_its_action_and_not_the_single_buttons_inside_it() {
        let mut c = config(&[]);
        c.save = combo(&["LB", "A"]);
        c.anki = combo(&["LB"]);
        let mut m = Machine::new();
        let t = Instant::now();
        assert!(m.step(&input(&["LB"]), &c, true, t).is_empty(), "LB alone waits: it starts LB + A");
        assert_eq!(m.step(&input(&["LB", "A"]), &c, true, t), vec![Signal::Act(Action::Save)], "the longer binding wins over A alone");
        assert!(m.step(&input(&["LB"]), &c, true, t).is_empty());
        assert!(m.step(&input(&[]), &c, true, t).is_empty(), "LB was used in a combination");
        m.step(&input(&["LB"]), &c, true, t);
        assert_eq!(m.step(&input(&[]), &c, true, t), vec![Signal::Act(Action::Anki)], "LB pressed and let go alone");
        assert_eq!(m.step(&input(&["RB"]), &c, true, t), vec![Signal::Act(Action::Explain)]);
        c.meme = combo(&["View"]);
        m.step(&input(&[]), &c, true, t);
        assert_eq!(m.step(&input(&["View"]), &c, true, t), vec![Signal::Act(Action::Meme)]);
    }

    #[test]
    fn the_look_up_combination_cancels_an_action_waiting_inside_it() {
        let mut c = config(&["LB", "Y"]);
        c.anki = combo(&["LB"]);
        let mut m = Machine::new();
        let t = Instant::now();
        assert!(m.step(&input(&["LB"]), &c, true, t).is_empty());
        assert_eq!(m.step(&input(&["LB", "Y"]), &c, true, t), vec![Signal::Hotkey]);
        m.step(&input(&[]), &c, true, t);
        assert!(m.step(&input(&[]), &c, true, t).is_empty(), "no Anki after the look-up");
    }

    #[test]
    fn a_direction_bound_to_an_action_does_not_also_move() {
        let mut c = config(&[]);
        c.pronounce = combo(&["LB", "Up"]);
        let mut m = Machine::new();
        let t = Instant::now();
        m.step(&input(&["LB"]), &c, true, t);
        assert_eq!(m.step(&input(&["LB", "Up"]), &c, true, t), vec![Signal::Act(Action::Pronounce)]);
        assert_eq!(m.step(&input(&["Up"]), &c, true, t), vec![Signal::Move(Direction::Up)], "Up alone moves again");
    }

    #[test]
    fn a_held_direction_repeats_and_the_stick_counts_as_the_d_pad() {
        let c = config(&[]);
        let mut m = Machine::new();
        let t = Instant::now();
        assert_eq!(m.step(&input(&["Down"]), &c, true, t), vec![Signal::Move(Direction::Down)]);
        assert!(m.step(&input(&["Down"]), &c, true, t + Duration::from_millis(300)).is_empty());
        assert_eq!(m.step(&input(&["Down"]), &c, true, t + Duration::from_millis(410)), vec![Signal::Move(Direction::Down)]);
        assert_eq!(m.step(&input(&["Down"]), &c, true, t + Duration::from_millis(540)), vec![Signal::Move(Direction::Down)]);
        m.step(&input(&[]), &c, true, t + Duration::from_millis(600));
        let stick = Input { left: (0.9, 0.2), ..Input::default() };
        assert_eq!(m.step(&stick, &c, true, t + Duration::from_millis(620)), vec![Signal::Move(Direction::Right)]);
        let scroll = Input { right_y: -1.0, ..Input::default() };
        assert_eq!(m.step(&scroll, &c, true, t + Duration::from_millis(700)), vec![Signal::Scroll(SCROLL_SPEED)]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unmapped_evdev_codes_are_read_by_their_standard_names() {
        let key = |code: u32| (EV_KEY << 16) | code;
        let abs = |code: u32| (EV_ABS << 16) | code;
        assert_eq!(evdev_button(key(0x130)), Some("A"));
        assert_eq!(evdev_button(key(0x134)), Some("X"));
        assert_eq!(evdev_button(key(0x13b)), Some("Menu"));
        assert_eq!(evdev_button(key(0x223)), Some("Right"));
        assert_eq!(evdev_axis(abs(0x01)), Some(RawAxis::LeftY));
        assert_eq!(evdev_axis(abs(0x11)), Some(RawAxis::HatY));
        assert_eq!(evdev_axis(abs(0x02)), None, "a trigger is not a stick");
        assert_eq!(evdev_button(abs(0x130)), None);
        for raw in 0u32..=255 { assert_eq!(evdev_button(raw), None); }
    }

    #[test]
    fn a_hat_counts_as_the_d_pad() {
        let mut input = Input::default();
        hat(&mut input, 1.0, -1.0);
        assert_eq!(input.buttons.iter().copied().collect::<Vec<_>>(), vec!["Down", "Right"]);
    }

    #[test]
    fn a_capture_records_every_button_held_until_all_are_released() {
        let c = config(&["Y"]);
        let mut m = Machine::new();
        let t = Instant::now();
        m.start_capture(t);
        assert_eq!(m.step(&input(&["LT"]), &c, true, t), vec![Signal::Capture { buttons: vec!["LT".into()], done: false }]);
        assert_eq!(m.step(&input(&["LT", "Y"]), &c, true, t), vec![Signal::Capture { buttons: vec!["LT".into(), "Y".into()], done: false }]);
        assert!(m.step(&input(&["Y"]), &c, true, t).is_empty(), "the old combination does not fire during a capture");
        assert_eq!(m.step(&input(&[]), &c, true, t), vec![Signal::Capture { buttons: vec!["LT".into(), "Y".into()], done: true }]);
        m.start_capture(t);
        assert_eq!(m.step(&input(&[]), &c, true, t + CAPTURE_TIMEOUT + TICK), vec![Signal::Capture { buttons: vec![], done: true }]);
    }
}
