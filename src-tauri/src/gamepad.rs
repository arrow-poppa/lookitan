//! Game controllers, as Lookupper has them (REA on 2026-10-23): a combination of buttons looks up
//! the word at the cursor, and with "Navegação pelo gamepad" the D-pad or the left stick moves
//! between the pop-up's controls, or the overlay's words when the pop-up is closed, the right
//! stick scrolls, and four buttons confirm, close, save the word and pronounce it. Lookupper reads
//! XInput (plus raw HID); Lookitan reads gilrs's XInput backend on Windows, which works while
//! another window has the focus, and evdev on Linux. Unlike Lookupper, the four action buttons can
//! be changed.
//!
//! `Machine` turns the buttons held at each tick into `Signal`s and knows nothing of gilrs, so it
//! is tested without a controller.

use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

use crate::settings::GamepadSettings;

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
pub enum Action { Confirm, Close, Save, Pronounce }

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

        let combo: Vec<&str> = config.hotkey.iter().map(String::as_str).filter(|b| BUTTONS.iter().any(|k| k == b)).collect();
        let holds_combo = !combo.is_empty() && combo.iter().all(|b| pressed.contains(*b));
        if holds_combo && !self.hotkey_held {
            out.push(Signal::Hotkey);
            self.wait_release = true;
        }
        self.hotkey_held = holds_combo;
        if self.wait_release {
            if pressed.is_empty() { self.wait_release = false; }
            self.held = None;
            self.previous = pressed.clone();
            return out;
        }

        if config.navigation && navigate {
            let newly = |b: &str| !b.is_empty() && pressed.contains(b) && !self.previous.contains(b);
            for (button, action) in [(&config.confirm, Action::Confirm), (&config.close, Action::Close), (&config.save, Action::Save), (&config.pronounce, Action::Pronounce)] {
                if newly(button.as_str()) { out.push(Signal::Act(action)); }
            }
            match direction(input) {
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
        }
        self.previous = pressed.clone();
        out
    }
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
        loop {
            let _ = gilrs.next_event_blocking(Some(TICK));
            while gilrs.next_event().is_some() {}
            let (config, capture) = {
                let mut s = shared().lock();
                s.connected = gilrs.gamepads().find(|(_, g)| g.is_connected()).map(|(_, g)| g.name().to_string());
                (s.config.clone(), s.capture.take())
            };
            match capture {
                Some(true) => machine.start_capture(Instant::now()),
                Some(false) => machine.stop_capture(),
                None => {}
            }
            let input = read(&gilrs);
            let busy = !input.buttons.is_empty() || input.left.0.abs().max(input.left.1.abs()) >= STICK_PRESS || input.right_y.abs() > SCROLL_DEAD;
            let navigate = config.navigation && busy && navigate();
            for s in machine.step(&input, &config, navigate, Instant::now()) {
                signal(s);
            }
        }
    });
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
        for (button, name) in NAMES {
            if pad.is_pressed(button) { input.buttons.insert(name); }
        }
        let left = (pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
        if left.0.abs().max(left.1.abs()) > input.left.0.abs().max(input.left.1.abs()) { input.left = left; }
        let right = pad.value(Axis::RightStickY);
        if right.abs() > input.right_y.abs() { input.right_y = right; }
    }
    input
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
        assert_eq!(m.step(&input(&["A"]), &c, true, t), vec![Signal::Act(Action::Confirm)]);
    }

    #[test]
    fn actions_follow_the_chosen_buttons_and_need_something_on_screen() {
        let mut c = config(&[]);
        c.save = "RB".into();
        let mut m = Machine::new();
        let t = Instant::now();
        assert!(m.step(&input(&["B"]), &c, false, t).is_empty(), "nothing on screen");
        m.step(&input(&[]), &c, true, t);
        assert_eq!(m.step(&input(&["B"]), &c, true, t), vec![Signal::Act(Action::Close)]);
        m.step(&input(&[]), &c, true, t);
        assert_eq!(m.step(&input(&["RB"]), &c, true, t), vec![Signal::Act(Action::Save)]);
        assert!(m.step(&input(&["X"]), &c, true, t).is_empty(), "X saves no more");
        c.navigation = false;
        assert!(m.step(&input(&["Y"]), &c, true, t).is_empty(), "navigation turned off");
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
