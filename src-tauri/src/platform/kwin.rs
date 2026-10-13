// KDE Plasma (Wayland) support, after the approach of MaruPop (LGPL-3.0, (c) 2026 marunine):
//  * a KWin script relays the global pointer position, the screen under it and the active
//    window to Lookitan over D-Bus, because no Wayland protocol reports the pointer to a client
//    that does not have focus;
//  * org.kde.KWin.ScreenShot2.CaptureArea returns pixels without a permission dialog when the
//    installed .desktop file declares the restricted interface.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsFd;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use image::RgbaImage;
use parking_lot::{Condvar, Mutex};
use serde::Deserialize;
use zbus::zvariant::{Fd, OwnedValue, Value};

use super::Capture;

pub const BUS_NAME: &str = "io.github.arrowpoppa.Lookitan";
const SCRIPT_NAME: &str = "lookitancursor";
const SCRIPT: &str = include_str!("../../resources/kwin/lookitan-cursor.js");

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Report {
    pub x: f64,
    pub y: f64,
    pub screen: String,
    pub scale: f64,
    pub sx: f64,
    pub sy: f64,
    pub sw: f64,
    pub sh: f64,
    pub app_class: String,
    pub app_pid: u32,
    pub app_title: String,
}

static LATEST: Mutex<Option<(Report, Instant)>> = Mutex::new(None);
static LATEST_CHANGED: Condvar = Condvar::new();
static TRACK_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);
static CONNECTION: OnceLock<zbus::blocking::Connection> = OnceLock::new();

struct CursorSink;

#[zbus::interface(name = "io.github.arrowpoppa.Lookitan.CursorSink")]
impl CursorSink {
    /// Called by the KWin script with one JSON report. Returns whether to report fast.
    fn update(&self, report: String) -> bool {
        match serde_json::from_str::<Report>(&report) {
            Ok(r) => {
                *LATEST.lock() = Some((r, Instant::now()));
                LATEST_CHANGED.notify_all();
            }
            Err(e) => log::debug!("bad cursor report: {e}"),
        }
        let until = *TRACK_UNTIL.lock();
        until.map(|t| Instant::now() < t).unwrap_or(false)
    }
}

pub fn latest() -> Option<Report> {
    let guard = LATEST.lock();
    let (r, at) = guard.as_ref()?;
    if at.elapsed() > Duration::from_secs(600) {
        return None;
    }
    Some(r.clone())
}

/// --scan can start the app before the newly loaded script's first timer tick.
pub fn wait_for_cursor(timeout: Duration) -> Option<(f64, f64)> {
    let deadline = Instant::now() + timeout;
    let mut guard = LATEST.lock();
    loop {
        if let Some((report, at)) = guard.as_ref() {
            if at.elapsed() < Duration::from_secs(2) { return Some((report.x, report.y)); }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return None; }
        LATEST_CHANGED.wait_for(&mut guard, remaining);
    }
}

/// Asks the relay to report at its fast rate for a few seconds (around a lookup).
pub fn track_for(d: Duration) {
    *TRACK_UNTIL.lock() = Some(Instant::now() + d);
}

/// Exposes the cursor sink on the session bus and loads the relay script into KWin.
pub fn start(data_dir: &std::path::Path) -> Result<()> {
    let conn = zbus::blocking::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at("/Cursor", CursorSink)?
        .build()
        .context("register D-Bus service")?;
    let script_path = data_dir.join("lookitan-cursor.js");
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(&script_path, SCRIPT)?;

    // A previous run may have left the script loaded.
    let _ = conn.call_method(
        Some("org.kde.KWin"),
        "/Scripting",
        Some("org.kde.kwin.Scripting"),
        "unloadScript",
        &(SCRIPT_NAME,),
    );
    let reply = conn.call_method(
        Some("org.kde.KWin"),
        "/Scripting",
        Some("org.kde.kwin.Scripting"),
        "loadScript",
        &(script_path.to_string_lossy().to_string(), SCRIPT_NAME),
    )?;
    let id: i32 = reply.body().deserialize()?;
    if id < 0 {
        bail!("KWin refused to load the cursor relay script");
    }
    // Plasma 6 exposes each script at /Scripting/Script<id>; older builds used /<id>.
    let mut started = false;
    for path in [format!("/Scripting/Script{id}"), format!("/{id}")] {
        if conn
            .call_method(Some("org.kde.KWin"), path.as_str(), Some("org.kde.kwin.Script"), "run", &())
            .is_ok()
        {
            started = true;
            break;
        }
    }
    if !started {
        conn.call_method(Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), "start", &())?;
    }
    log::info!("KWin cursor relay loaded (script id {id})");
    let _ = CONNECTION.set(conn);
    Ok(())
}

pub fn stop() {
    if let Some(conn) = CONNECTION.get() {
        let _ = conn.call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &(SCRIPT_NAME,),
        );
    }
}

fn get_u32(map: &HashMap<String, OwnedValue>, key: &str) -> Option<u32> {
    map.get(key).and_then(|v| u32::try_from(v).ok())
}

type CaptureAreaBody<'a> = (i32, i32, i32, i32, HashMap<&'a str, Value<'a>>, Fd<'a>);

fn capture_area_body<'a>(x: i32, y: i32, width: u32, height: u32,
    options: HashMap<&'a str, Value<'a>>, fd: Fd<'a>) -> Result<CaptureAreaBody<'a>> {
    if width == 0 || height == 0 { bail!("Empty screenshot area"); }
    // KWin's CaptureArea signature is iiiia{sv}h, including SIGNED width/height.
    let width = i32::try_from(width).context("Screenshot width exceeds KWin's limit")?;
    let height = i32::try_from(height).context("Screenshot height exceeds KWin's limit")?;
    Ok((x, y, width, height, options, fd))
}

/// The screen under the pointer, in KWin's logical coordinates.
pub fn screen_rect() -> Option<(f64, f64, f64, f64)> {
    let r = latest()?;
    if r.sw > 0.0 && r.sh > 0.0 {
        Some((r.sx, r.sy, r.sw, r.sh))
    } else {
        None
    }
}

/// Captures an area in KWin's logical coordinates at native resolution.
pub fn capture_rect(x: i32, y: i32, width: u32, height: u32) -> Result<Capture> {
    let conn = match CONNECTION.get() {
        Some(c) => c.clone(),
        None => zbus::blocking::Connection::session()?,
    };
    let (mut reader, writer) = std::io::pipe()?;
    let read_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).map(|_| buf)
    });
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("native-resolution", Value::from(true));
    options.insert("include-cursor", Value::from(false));
    let body = capture_area_body(x, y, width, height, options, Fd::from(writer.as_fd()))?;
    let reply = conn.call_method(
        Some("org.kde.KWin"),
        "/org/kde/KWin/ScreenShot2",
        Some("org.kde.KWin.ScreenShot2"),
        "CaptureArea",
        &body,
    );
    drop(body);
    drop(writer);
    let reply = reply.context("org.kde.KWin.ScreenShot2.CaptureArea (is Lookitan installed from its package?)")?;
    let results: HashMap<String, OwnedValue> = reply.body().deserialize()?;
    let data = read_thread.join().map_err(|_| anyhow!("pipe reader panicked"))??;

    let w = get_u32(&results, "width").ok_or_else(|| anyhow!("no width in ScreenShot2 reply"))?;
    let h = get_u32(&results, "height").ok_or_else(|| anyhow!("no height in ScreenShot2 reply"))?;
    let stride = get_u32(&results, "stride").unwrap_or(w * 4) as usize;
    let format = get_u32(&results, "format").unwrap_or(6);
    if data.len() < stride * h as usize {
        bail!("ScreenShot2 returned {} bytes for {w}x{h}", data.len());
    }
    // QImage formats: 4/5/6 = (A)RGB32 stored as BGRA on little-endian; 16/17/18 = RGBA8888.
    let bgra = matches!(format, 4 | 5 | 6);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h as usize {
        let line = &data[row * stride..row * stride + w as usize * 4];
        for px in line.chunks_exact(4) {
            if bgra {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            } else {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
    }
    let image = RgbaImage::from_raw(w, h, rgba).ok_or_else(|| anyhow!("bad image buffer"))?;
    let scale = if width > 0 { w as f64 / width as f64 } else { 1.0 };
    Ok(Capture { image, origin_x: x as f64, origin_y: y as f64, scale })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Type;

    #[test]
    fn capture_area_matches_kwin_dbus_contract() {
        let (_reader, writer) = std::io::pipe().unwrap();
        let body = capture_area_body(-900, 0, 900, 260, HashMap::new(), Fd::from(writer.as_fd())).unwrap();
        assert_eq!(body.0, -900);
        assert_eq!(body.2, 900);
        assert_eq!(CaptureAreaBody::SIGNATURE.to_string(), "(iiiia{sv}h)");
    }

    #[test]
    fn invalid_capture_dimensions_are_rejected_before_dbus() {
        let (_reader, writer) = std::io::pipe().unwrap();
        assert!(capture_area_body(0, 0, 0, 260, HashMap::new(), Fd::from(writer.as_fd())).is_err());
        assert!(capture_area_body(0, 0, u32::MAX, 260, HashMap::new(), Fd::from(writer.as_fd())).is_err());
    }

    #[test]
    fn first_lookup_waits_for_a_fresh_cursor_report_and_missing_relay_times_out() {
        *LATEST.lock() = None;
        let sender = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(10));
            CursorSink.update(r#"{"x":-100,"y":200}"#.to_string());
        });
        assert_eq!(wait_for_cursor(Duration::from_secs(1)), Some((-100.0, 200.0)));
        sender.join().unwrap();
        *LATEST.lock() = None;
        assert_eq!(wait_for_cursor(Duration::from_millis(5)), None);
    }
}
