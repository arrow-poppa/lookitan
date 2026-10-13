// KDE Plasma (Wayland) support, after the approach of MaruPop (LGPL-3.0, (c) 2026 marunine):
//  * a KWin script relays the global pointer position to Lookitan over D-Bus, because no
//    Wayland protocol reports the pointer to a client that does not have focus;
//  * org.kde.KWin.ScreenShot2.CaptureArea returns the region around the pointer without a
//    permission dialog when the installed .desktop file declares the restricted interface.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsFd;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use image::RgbaImage;
use parking_lot::Mutex;
use zbus::zvariant::{Fd, OwnedValue, Value};

use super::Capture;

pub const BUS_NAME: &str = "io.github.arrowpoppa.Lookitan";
const SCRIPT_NAME: &str = "lookitancursor";
const SCRIPT: &str = include_str!("../../resources/kwin/lookitan-cursor.js");

struct CursorState {
    x: f64,
    y: f64,
    scale: f64,
    at: Instant,
}

static CURSOR: Mutex<Option<CursorState>> = Mutex::new(None);
static TRACK_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);
static CONNECTION: OnceLock<zbus::blocking::Connection> = OnceLock::new();

struct CursorSink;

#[zbus::interface(name = "io.github.arrowpoppa.Lookitan.CursorSink")]
impl CursorSink {
    /// Called by the KWin script. Values arrive as strings so the D-Bus signature is fixed no
    /// matter how JavaScript chooses to marshal each number. Returns whether to report fast.
    fn update(&self, x: String, y: String, _screen: String, scale: String, _sent_at: String) -> bool {
        if let (Ok(x), Ok(y)) = (x.parse::<f64>(), y.parse::<f64>()) {
            let scale = scale.parse::<f64>().unwrap_or(1.0);
            *CURSOR.lock() = Some(CursorState { x, y, scale, at: Instant::now() });
        }
        let until = *TRACK_UNTIL.lock();
        until.map(|t| Instant::now() < t).unwrap_or(false)
    }
}

pub fn latest_cursor() -> Option<(f64, f64)> {
    let c = CURSOR.lock();
    let c = c.as_ref()?;
    if c.at.elapsed() > Duration::from_secs(600) {
        return None;
    }
    Some((c.x, c.y))
}

pub fn latest_scale() -> f64 {
    CURSOR.lock().as_ref().map(|c| c.scale).unwrap_or(1.0)
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

/// Captures a region centred on (cx, cy), in KWin's logical coordinates, at native resolution.
pub fn capture_area(cx: f64, cy: f64, width: u32, height: u32) -> Result<Capture> {
    let conn = match CONNECTION.get() {
        Some(c) => c.clone(),
        None => zbus::blocking::Connection::session()?,
    };
    let x = (cx - width as f64 / 2.0).max(0.0).round() as i32;
    let y = (cy - height as f64 / 2.0).max(0.0).round() as i32;

    let (mut reader, writer) = std::io::pipe()?;
    let read_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).map(|_| buf)
    });
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("native-resolution", Value::from(true));
    options.insert("include-cursor", Value::from(false));
    let reply = conn.call_method(
        Some("org.kde.KWin"),
        "/org/kde/KWin/ScreenShot2",
        Some("org.kde.KWin.ScreenShot2"),
        "CaptureArea",
        &(x, y, width, height, options, Fd::from(writer.as_fd())),
    );
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
    let scale = if width > 0 { w as f64 / width as f64 } else { latest_scale() };
    Ok(Capture { image, origin_x: x as f64, origin_y: y as f64, scale })
}
