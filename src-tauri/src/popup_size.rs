// The pop-up's size as the user last left it, kept across restarts in `popup-size.json` beside
// settings.json. Not a setting: the window itself is the control (drag its edge), and keeping it
// out of settings.json keeps it out of profiles, exports and the main window's redraws.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

const FILE: &str = "popup-size.json";
/// Smaller than this the pop-up has no room for its header; larger is no screen.
const MIN: (f64, f64) = (280.0, 180.0);
const MAX: f64 = 8000.0;
/// A drag sends many sizes; the one it ends on is written once it has stopped this long.
const QUIET: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Size {
    /// Logical pixels (CSS pixels), so it is the same size on a monitor of another scale.
    pub width: f64,
    pub height: f64,
}

fn path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

/// A size kept within what the pop-up can be; None for one that is not a size.
pub fn clamp(width: f64, height: f64) -> Option<Size> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some(Size { width: width.clamp(MIN.0, MAX).round(), height: height.clamp(MIN.1, MAX).round() })
}

pub fn parse(text: &str) -> Option<Size> {
    let size: Size = serde_json::from_str(text).ok()?;
    clamp(size.width, size.height)
}

/// The size saved, if any.
pub fn load(config_dir: &Path) -> Option<Size> {
    parse(&std::fs::read_to_string(path(config_dir)).ok()?)
}

fn save(config_dir: &Path, size: Size) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let file = path(config_dir);
    let temp = file.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_string(&size).unwrap_or_default())?;
    std::fs::rename(temp, file)
}

/// Sizes a pixel or two apart are the same (rounding between physical and logical pixels).
pub fn differs(a: Size, b: Size) -> bool {
    (a.width - b.width).abs() > 2.0 || (a.height - b.height).abs() > 2.0
}

static PENDING: Mutex<Option<(PathBuf, Size, Instant)>> = Mutex::new(None);
static SAVING: AtomicBool = AtomicBool::new(false);

/// Called on every size the visible pop-up takes; writes the last one once the drag is over.
pub fn remember(config_dir: &Path, width: f64, height: f64) {
    let Some(size) = clamp(width, height) else { return };
    *PENDING.lock() = Some((config_dir.to_path_buf(), size, Instant::now()));
    if SAVING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_millis(200));
        let mut pending = PENDING.lock();
        if pending.as_ref().is_some_and(|(_, _, at)| at.elapsed() < QUIET) {
            continue;
        }
        let job = pending.take();
        // Under the lock: a size that comes after this starts a writer of its own.
        SAVING.store(false, Ordering::SeqCst);
        drop(pending);
        if let Some((dir, size, _)) = job {
            if load(&dir).is_none_or(|old| differs(old, size)) {
                if let Err(e) = save(&dir, size) {
                    log::warn!("could not keep the pop-up's size: {e}");
                }
            }
        }
        break;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_size_is_read_back_within_what_the_pop_up_can_be() {
        assert_eq!(parse(r#"{"width": 720.4, "height": 610}"#), Some(Size { width: 720.0, height: 610.0 }));
        assert_eq!(parse(r#"{"width": 40, "height": 20000}"#), Some(Size { width: MIN.0, height: MAX }));
        assert_eq!(parse(r#"{"width": 0, "height": 300}"#), None);
        assert_eq!(parse("not json"), None);
        assert_eq!(clamp(f64::NAN, 300.0), None);
    }

    #[test]
    fn rounding_between_pixels_is_not_a_new_size() {
        let a = Size { width: 580.0, height: 520.0 };
        assert!(!differs(a, Size { width: 581.0, height: 519.0 }));
        assert!(differs(a, Size { width: 640.0, height: 520.0 }));
    }

    #[test]
    fn the_last_size_of_a_drag_is_written_once_it_stops() {
        let dir = std::env::temp_dir().join(format!("lookitan-popup-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        remember(&dir, 600.0, 500.0);
        remember(&dir, 700.0, 540.0);
        assert_eq!(load(&dir), None, "nothing while the drag goes on");
        std::thread::sleep(QUIET + Duration::from_millis(600));
        assert_eq!(load(&dir), Some(Size { width: 700.0, height: 540.0 }));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
