//! PlayStation pads on Windows (DualSense, DualSense Edge, DualShock 4), which XInput does not
//! see without Steam or DS4Windows: read from their HID reports, as Harbor (arrow-poppa/streaming,
//! MIT) reads them, without its touchpad part. Lookupper reads raw HID pads too.
//!
//! Harbor asks a Bluetooth pad for its long report (by reading the calibration feature report)
//! because only that one has the touchpad. Lookitan never does: the switch changes what the pad
//! sends to every other program, and the short report already has the buttons and sticks.
//!
//! On Linux the kernel driver (hid-playstation) makes them evdev pads that gilrs reads.

use super::Input;

/// The pad last read, by name, and what it holds; `None` with no PlayStation pad reporting.
pub fn state() -> Option<(String, Input)> {
    #[cfg(windows)]
    return hid_windows::STATE.lock().clone();
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
pub use hid_windows::run;

/// The two report layouts. The button bytes are the same in both.
#[cfg(any(windows, test))]
#[derive(Clone, Copy, PartialEq, Debug)]
enum Model {
    DualSense,
    DualShock4,
}

#[cfg(any(windows, test))]
impl Model {
    fn of(product: u16) -> Option<Model> {
        match product {
            // DualSense, DualSense Edge.
            0x0ce6 | 0x0df2 => Some(Model::DualSense),
            // DualShock 4 v1 and v2, and Sony's USB wireless adapter for it.
            0x05c4 | 0x09cc | 0x0ba0 => Some(Model::DualShock4),
            _ => None,
        }
    }
}

/// `PadState::buttons`' bits by Xbox name (gamepad::BUTTONS); the PS button is left out.
#[cfg(any(windows, test))]
const BITS: [(u32, &str); 16] = [
    (0, "A"), (1, "B"), (2, "X"), (3, "Y"), (4, "LB"), (5, "RB"), (6, "LT"), (7, "RT"),
    (8, "View"), (9, "Menu"), (11, "LS"), (12, "RS"), (13, "Up"), (14, "Down"), (15, "Left"), (16, "Right"),
];

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct PadState {
    buttons: u32,
    /// LX, LY, RX, RY, 0 left or up to 255.
    sticks: [u8; 4],
}

#[cfg(any(windows, test))]
fn bit(on: bool, index: usize) -> u32 {
    if on { 1 << index } else { 0 }
}

/// `sticks` is LX, LY, RX, RY; `b` the three button bytes; `l2`/`r2` the trigger positions. The
/// layout is the one hid-playstation reads. Bits: cross, circle, square, triangle, L1, R1, L2,
/// R2, create/share, options, PS, L3, R3, then the D-pad's four directions.
#[cfg(any(windows, test))]
fn decode(sticks: &[u8], b: &[u8], l2: u8, r2: u8) -> PadState {
    let hat = b[0] & 0x0f;
    let up = matches!(hat, 0 | 1 | 7);
    let right = matches!(hat, 1..=3);
    let down = matches!(hat, 3..=5);
    let left = matches!(hat, 5..=7);
    let buttons = bit(b[0] & 0x20 != 0, 0)
        | bit(b[0] & 0x40 != 0, 1)
        | bit(b[0] & 0x10 != 0, 2)
        | bit(b[0] & 0x80 != 0, 3)
        | bit(b[1] & 0x01 != 0, 4)
        | bit(b[1] & 0x02 != 0, 5)
        // The triggers count as pressed halfway down.
        | bit(l2 >= 128, 6)
        | bit(r2 >= 128, 7)
        | bit(b[1] & 0x10 != 0, 8)
        | bit(b[1] & 0x20 != 0, 9)
        | bit(b[2] & 0x01 != 0, 10)
        | bit(b[1] & 0x40 != 0, 11)
        | bit(b[1] & 0x80 != 0, 12)
        | bit(up, 13)
        | bit(down, 14)
        | bit(left, 15)
        | bit(right, 16);
    PadState { buttons, sticks: [sticks[0], sticks[1], sticks[2], sticks[3]] }
}

/// A report's state, and whether the pad sends this report continuously (so silence means it is
/// gone). Report 0x01 is the full report over USB but the short one over Bluetooth, and its length
/// cannot tell them apart: Windows pads every input report of a Bluetooth pad to the longest one
/// the pad declares.
#[cfg(any(windows, test))]
fn parse_report(model: Model, bluetooth: bool, buf: &[u8]) -> Option<(PadState, bool)> {
    match (model, buf.first()?, buf.len()) {
        // USB: the full report right after its id.
        (Model::DualSense, 0x01, n) if !bluetooth && n >= 64 => Some((dualsense_report(&buf[1..]), true)),
        (Model::DualShock4, 0x01, n) if !bluetooth && n >= 64 => Some((dualshock4_report(&buf[1..]), true)),
        // Bluetooth, short: sticks, buttons, then the triggers; sent on a change.
        (_, 0x01, n) if bluetooth && n >= 10 => Some((decode(&buf[1..5], &buf[5..8], buf[8], buf[9]), false)),
        // Bluetooth, long (another program switched the pad): the USB layout a few bytes later.
        (Model::DualSense, 0x31, n) if n >= 78 => Some((dualsense_report(&buf[2..]), true)),
        (Model::DualShock4, 0x11, n) if n >= 78 => Some((dualshock4_report(&buf[3..]), true)),
        _ => None,
    }
}

#[cfg(any(windows, test))]
fn dualsense_report(d: &[u8]) -> PadState {
    decode(&d[0..4], &d[7..10], d[4], d[5])
}

#[cfg(any(windows, test))]
fn dualshock4_report(d: &[u8]) -> PadState {
    decode(&d[0..4], &d[4..7], d[7], d[8])
}

#[cfg(any(windows, test))]
fn stick_value(raw: u8) -> f32 {
    ((raw as f32 - 127.5) / 127.5).clamp(-1.0, 1.0)
}

/// The pad's state as gamepad.rs reads controllers: Xbox names, sticks up positive.
#[cfg(any(windows, test))]
fn input_of(state: &PadState) -> Input {
    let mut input = Input::default();
    for (index, name) in BITS {
        if state.buttons & (1 << index) != 0 { input.buttons.insert(name); }
    }
    let [lx, ly, _, ry] = state.sticks;
    input.left = (stick_value(lx), -stick_value(ly));
    input.right_y = -stick_value(ry);
    input
}

#[cfg(windows)]
mod hid_windows {
    use super::{input_of, parse_report, Model};
    use crate::gamepad::Input;
    use parking_lot::Mutex;
    use std::time::{Duration, Instant};

    pub static STATE: Mutex<Option<(String, Input)>> = Mutex::new(None);
    const SONY: u16 = 0x054c;
    /// A pad that sends its report continuously and stops for this long is gone (switched off;
    /// Windows keeps a paired Bluetooth pad's device).
    const SILENCE: Duration = Duration::from_secs(2);

    pub fn run() {
        let mut api = match hidapi::HidApi::new() {
            Ok(api) => api,
            Err(e) => {
                log::warn!("gamepad: HID unavailable: {e}");
                return;
            }
        };
        loop {
            let _ = api.refresh_devices();
            let found = api.device_list().filter(|d| d.vendor_id() == SONY).find_map(|d| {
                let model = Model::of(d.product_id())?;
                // A DualShock 4 calls itself "Wireless Controller"; the model name lets the
                // settings show PlayStation button names.
                let fallback = match model {
                    Model::DualSense => "DualSense Wireless Controller",
                    Model::DualShock4 => "DualShock 4 Wireless Controller",
                };
                let name = d.product_string().filter(|s| s.contains("Dual")).unwrap_or(fallback);
                Some((d.path().to_owned(), name.to_string(), model, d.bus_type()))
            });
            if let Some((path, name, model, bus)) = found {
                if let Ok(device) = api.open_path(&path) {
                    read_device(&device, &name, model, bus);
                }
            }
            *STATE.lock() = None;
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    fn read_device(device: &hidapi::HidDevice, name: &str, model: Model, bus: hidapi::BusType) {
        let mut buf = [0u8; 128];
        let mut last: Option<(Instant, bool)> = None;
        loop {
            let n = match device.read_timeout(&mut buf, 100) {
                Ok(n) => n,
                Err(_) => break,
            };
            if n == 0 {
                if last.is_some_and(|(at, continuous)| continuous && at.elapsed() > SILENCE) { break; }
                continue;
            }
            let bluetooth = match bus {
                hidapi::BusType::Usb => false,
                hidapi::BusType::Bluetooth => true,
                // A USB report is exactly 64 bytes long.
                _ => n != 64,
            };
            let Some((state, continuous)) = parse_report(model, bluetooth, &buf[..n]) else { continue };
            last = Some((Instant::now(), continuous));
            *STATE.lock() = Some((name.to_string(), input_of(&state)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{input_of, parse_report, Model, PadState};

    fn pressed(state: &PadState) -> Vec<&'static str> {
        input_of(state).buttons.into_iter().collect()
    }

    #[test]
    fn reads_usb_buttons_and_sticks() {
        let mut r = [0u8; 64];
        r[0] = 0x01;
        r[1..5].copy_from_slice(&[0, 255, 128, 128]);
        r[5] = 200; // L2 past halfway
        r[8] = 0x20 | 0x02; // cross, D-pad right (hat 2)
        r[9] = 0x01 | 0x20; // L1, options
        let Some((state, true)) = parse_report(Model::DualSense, false, &r) else { panic!("not a full report") };
        assert_eq!(pressed(&state), ["A", "LB", "LT", "Menu", "Right"]);
        assert_eq!(state.sticks, [0, 255, 128, 128]);
        let input = input_of(&state);
        assert_eq!(input.left, (-1.0, -1.0), "left and down");
    }

    #[test]
    fn reads_the_long_bluetooth_report_one_byte_later() {
        let mut r = [0u8; 78];
        r[0] = 0x31;
        r[2..6].copy_from_slice(&[10, 20, 30, 40]);
        r[9] = 0x08; // hat 8: D-pad released
        let Some((state, true)) = parse_report(Model::DualSense, true, &r) else { panic!("not a full report") };
        assert!(pressed(&state).is_empty());
        assert_eq!(state.sticks, [10, 20, 30, 40]);
    }

    #[test]
    fn the_short_bluetooth_report_has_the_buttons() {
        let mut r = [0u8; 10];
        r[0] = 0x01;
        r[5] = 0x80 | 0x07; // triangle, D-pad up-left
        let Some((state, false)) = parse_report(Model::DualSense, true, &r) else { panic!("not a short report") };
        assert_eq!(pressed(&state), ["Left", "Up", "Y"]);
    }

    #[test]
    fn a_short_bluetooth_report_padded_by_windows_is_still_short() {
        // Windows hands it over at the pad's longest report length, zeros after the triggers;
        // read as a USB report the idle L2 byte would be the D-pad held up.
        let mut r = [0u8; 78];
        r[0] = 0x01;
        r[1..5].copy_from_slice(&[128, 128, 128, 128]);
        r[5] = 0x08; // D-pad released
        for model in [Model::DualSense, Model::DualShock4] {
            assert!(matches!(parse_report(model, true, &r), Some((s, false)) if pressed(&s).is_empty()));
        }
    }

    #[test]
    fn reads_dualshock4_usb_reports() {
        let mut r = [0u8; 64];
        r[0] = 0x01;
        r[1..5].copy_from_slice(&[1, 2, 3, 4]);
        r[5] = 0x40 | 0x04; // circle, D-pad down (hat 4)
        r[6] = 0x02 | 0x10; // R1, share
        r[7] = 0x01; // PS, left out
        r[9] = 255; // R2 fully down
        let Some((state, true)) = parse_report(Model::DualShock4, false, &r) else { panic!("not a full report") };
        assert_eq!(pressed(&state), ["B", "Down", "RB", "RT", "View"]);
        assert_eq!(state.sticks, [1, 2, 3, 4]);
    }

    #[test]
    fn reads_the_dualshock4_bluetooth_report_two_bytes_later() {
        let mut r = [0u8; 78];
        r[0] = 0x11;
        r[3..7].copy_from_slice(&[9, 8, 7, 6]);
        r[7] = 0x08 | 0x10; // D-pad released, square
        let Some((state, true)) = parse_report(Model::DualShock4, true, &r) else { panic!("not a full report") };
        assert_eq!(pressed(&state), ["X"]);
        assert_eq!(state.sticks, [9, 8, 7, 6]);
        // The DualSense's long Bluetooth report id means nothing to a DS4.
        r[0] = 0x31;
        assert_eq!(parse_report(Model::DualShock4, true, &r), None);
    }

    #[test]
    fn knows_each_sony_pad_by_product_id() {
        assert_eq!(Model::of(0x0ce6), Some(Model::DualSense));
        assert_eq!(Model::of(0x0df2), Some(Model::DualSense));
        assert_eq!(Model::of(0x05c4), Some(Model::DualShock4));
        assert_eq!(Model::of(0x09cc), Some(Model::DualShock4));
        assert_eq!(Model::of(0x0ba0), Some(Model::DualShock4));
        assert_eq!(Model::of(0x0268), None);
    }
}
