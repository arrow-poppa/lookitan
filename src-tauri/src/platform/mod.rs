// Pointer position, screen capture, the foreground app and process freezing for each desktop.

#[cfg(target_os = "linux")]
pub mod kwin;
#[cfg(windows)]
pub mod win;

use anyhow::{anyhow, Result};
use image::RgbaImage;
use serde::Serialize;
use tauri::AppHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionKind {
    Windows,
    X11,
    KdeWayland,
    OtherWayland,
}

pub fn session_kind() -> SessionKind {
    if cfg!(windows) {
        return SessionKind::Windows;
    }
    let wayland = std::env::var("XDG_SESSION_TYPE").map(|v| v.eq_ignore_ascii_case("wayland")).unwrap_or(false)
        || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if !wayland {
        return SessionKind::X11;
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_uppercase();
    if desktop.split(':').any(|d| d == "KDE") {
        SessionKind::KdeWayland
    } else {
        SessionKind::OtherWayland
    }
}

/// A captured image and where it sits on the desktop.
#[derive(Clone)]
pub struct Capture {
    pub image: RgbaImage,
    /// Desktop coordinates of the image's top-left pixel.
    pub origin_x: f64,
    pub origin_y: f64,
    /// Image pixels per desktop coordinate unit.
    pub scale: f64,
}

impl Capture {
    pub fn to_image(&self, x: f64, y: f64) -> (f32, f32) {
        (((x - self.origin_x) * self.scale) as f32, ((y - self.origin_y) * self.scale) as f32)
    }

    pub fn to_desktop(&self, x: f32, y: f32) -> (f64, f64) {
        (self.origin_x + x as f64 / self.scale, self.origin_y + y as f64 / self.scale)
    }
}

/// Pointer position in desktop coordinates.
pub fn cursor_position(app: &AppHandle) -> Result<(f64, f64)> {
    #[cfg(target_os = "linux")]
    if session_kind() == SessionKind::KdeWayland {
        if let Some(p) = kwin::latest_cursor() {
            return Ok(p);
        }
        log::warn!("KWin cursor relay has not reported yet; falling back to the toolkit position");
    }
    let p = app.cursor_position().map_err(|e| anyhow!("cursor position: {e}"))?;
    Ok((p.x, p.y))
}

/// Captures the whole monitor under the pointer.
pub fn capture_screen(cx: f64, cy: f64) -> Result<Capture> {
    #[cfg(target_os = "linux")]
    if session_kind() == SessionKind::KdeWayland {
        if let Some((x, y, w, h)) = kwin::screen_rect() {
            match kwin::capture_rect(x as i32, y as i32, w as u32, h as u32) {
                Ok(c) => return Ok(c),
                Err(e) => log::warn!("KWin ScreenShot2 failed ({e:#}); trying the generic capture"),
            }
        }
    }
    let monitor = xcap::Monitor::from_point(cx as i32, cy as i32).map_err(|e| anyhow!("monitor at pointer: {e}"))?;
    let mx = monitor.x().map_err(|e| anyhow!("{e}"))? as f64;
    let my = monitor.y().map_err(|e| anyhow!("{e}"))? as f64;
    let image = monitor.capture_image().map_err(|e| anyhow!("capture: {e}"))?;
    Ok(Capture { image, origin_x: mx, origin_y: my, scale: 1.0 })
}

/// Captures a `width` x `height` region (logical size) centred on the pointer, clamped to its monitor.
pub fn capture_around(cx: f64, cy: f64, width: u32, height: u32) -> Result<Capture> {
    #[cfg(target_os = "linux")]
    if session_kind() == SessionKind::KdeWayland {
        let x = (cx - width as f64 / 2.0).max(0.0).round() as i32;
        let y = (cy - height as f64 / 2.0).max(0.0).round() as i32;
        match kwin::capture_rect(x, y, width, height) {
            Ok(c) => return Ok(c),
            Err(e) => log::warn!("KWin ScreenShot2 failed ({e:#}); trying the generic capture"),
        }
    }
    let monitor = xcap::Monitor::from_point(cx as i32, cy as i32).map_err(|e| anyhow!("monitor at pointer: {e}"))?;
    let mx = monitor.x().map_err(|e| anyhow!("{e}"))? as f64;
    let my = monitor.y().map_err(|e| anyhow!("{e}"))? as f64;
    let mw = monitor.width().map_err(|e| anyhow!("{e}"))?;
    let mh = monitor.height().map_err(|e| anyhow!("{e}"))?;
    let scale = monitor.scale_factor().map(|s| s as f64).unwrap_or(1.0).max(0.5);
    // Desktop coordinates are physical pixels on Windows and X11.
    let w = ((width as f64 * scale) as u32).clamp(64, mw.max(64));
    let h = ((height as f64 * scale) as u32).clamp(32, mh.max(32));
    let rx = (cx - mx - w as f64 / 2.0).clamp(0.0, (mw.saturating_sub(w)) as f64) as u32;
    let ry = (cy - my - h as f64 / 2.0).clamp(0.0, (mh.saturating_sub(h)) as f64) as u32;
    let image = monitor.capture_region(rx, ry, w.min(mw), h.min(mh)).map_err(|e| anyhow!("capture: {e}"))?;
    Ok(Capture { image, origin_x: mx + rx as f64, origin_y: my + ry as f64, scale: 1.0 })
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForegroundApp {
    pub pid: u32,
    /// Executable file name on Windows, window class on KDE.
    pub executable: String,
    pub title: String,
}

/// The app in front of the user when the lookup started, used to pick its profile.
#[cfg(windows)]
pub fn foreground_app() -> Option<ForegroundApp> {
    win::foreground_app().map(|a| ForegroundApp { pid: a.pid, executable: a.executable, title: a.title })
}

/// The app in front of the user when the lookup started (reported by the KWin relay).
#[cfg(target_os = "linux")]
pub fn foreground_app() -> Option<ForegroundApp> {
    let r = kwin::latest()?;
    if r.app_pid == 0 || r.app_pid == std::process::id() {
        return None;
    }
    Some(ForegroundApp { pid: r.app_pid, executable: r.app_class, title: r.app_title })
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn foreground_app() -> Option<ForegroundApp> {
    None
}

pub fn suspend_process(pid: u32) {
    #[cfg(windows)]
    win::suspend_process(pid);
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("kill").args(["-STOP", &pid.to_string()]).status();
    }
}

pub fn resume_process(pid: u32) {
    #[cfg(windows)]
    win::resume_process(pid);
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("kill").args(["-CONT", &pid.to_string()]).status();
    }
}
