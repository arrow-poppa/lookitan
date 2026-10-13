// Lookitan: OCR pop-up dictionary with bring-your-own-key AI.
// Copyright (C) 2026 arrow-poppa
//
// This program is free software: you can redistribute it and/or modify it under the terms of the
// GNU General Public License version 3 as published by the Free Software Foundation.

mod cards;
mod dict;
mod mdict;
mod ocr;
mod platform;
mod protocol;
mod settings;
mod words;

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use base64::Engine;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;

use crate::cards::CardStore;
use crate::dict::{DictList, Dictionary, EntryRef};
use crate::ocr::lens::LensOcr;
use crate::ocr::cache::{frame_key, FrameKey, OcrCache};
use crate::ocr::paddle::{split_focus, PaddleOcr, TextBox};
use crate::ocr::OcrLine;
use crate::platform::{Capture, ForegroundApp, SessionKind};
use crate::settings::{AppProfile, DictionaryConfig, DictionaryKind, FreezeMode, OcrEngineKind, OcrScope, Settings};

/// The last OCR pass: what the overlay shows and what clicks on it look up.
struct Recognition {
    id: u64,
    capture: Capture,
    lines: Vec<OcrLine>,
    /// Lines around the first word, used for the "sentence" screenshot.
    focus_lines: Vec<OcrLine>,
    /// Monitor of the overlay: desktop x, y (physical), and its scale factor.
    monitor: (f64, f64, f64),
    app: Option<ForegroundApp>,
    frozen_pid: Option<u32>,
    freeze_frame: bool,
    engine: String,
    ocr_ms: u64,
}

pub struct AppState {
    config_dir: PathBuf,
    resource_dir: PathBuf,
    pub settings: RwLock<Settings>,
    pub dicts: RwLock<DictList>,
    dict_errors: RwLock<HashMap<String, String>>,
    paddle: Mutex<Option<(String, bool, Arc<PaddleOcr>)>>,
    engine_error: RwLock<Option<String>>,
    ort_ready: Mutex<Option<std::result::Result<(), String>>>,
    lens: Mutex<Option<Arc<LensOcr>>>,
    recognition: Mutex<Option<Recognition>>,
    ocr_cache: Mutex<OcrCache>,
    recognition_seq: AtomicU64,
    pub cards: CardStore,
    busy: AtomicBool,
    pinned: AtomicBool,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LookupPayload {
    pub word: String,
    pub sentence: String,
    pub candidates: Vec<String>,
    pub entries: Vec<EntryRef>,
    pub engine: String,
    pub ocr_ms: u64,
    pub notice: Option<String>,
    pub error: Option<String>,
    pub manual: bool,
    pub app: Option<ForegroundApp>,
    pub recognition: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct OverlayWord {
    text: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct OverlayPayload {
    id: u64,
    words: Vec<OverlayWord>,
    /// Desktop rectangle (CSS px of the overlay) of the selected word.
    selected: Option<OverlayWord>,
    frame_url: Option<String>,
    complete: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub session: SessionKind,
    pub runtime_loaded: bool,
    pub runtime_error: Option<String>,
    pub det_model: Option<String>,
    pub det_on_gpu: bool,
    pub rec_model: Option<String>,
    pub rec_on_gpu: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DictInfo {
    pub id: String,
    pub kind: DictionaryKind,
    pub name: String,
    pub path: String,
    pub enabled: bool,
    pub loaded: bool,
    pub entries: usize,
    pub resource_files: usize,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct LanguageInfo {
    pub tag: &'static str,
    pub name: &'static str,
}

// ---------------------------------------------------------------------------------------------
// OCR engine

fn ort_library_path(resource_dir: &std::path::Path) -> PathBuf {
    if cfg!(windows) {
        resource_dir.join("onnxruntime.dll")
    } else {
        resource_dir.join("ort").join("libonnxruntime.so")
    }
}

fn ensure_ort(state: &AppState) -> Result<()> {
    let mut ready = state.ort_ready.lock();
    if let Some(r) = ready.as_ref() {
        return r.clone().map_err(|e| anyhow!(e));
    }
    let path = ort_library_path(&state.resource_dir);
    let result = (|| -> Result<()> {
        if !path.exists() {
            return Err(anyhow!("ONNX Runtime not found at {}", path.display()));
        }
        let builder = ort::init_from(&path).map_err(|e| anyhow!("load {}: {e}", path.display()))?;
        builder.with_name("lookitan").commit();
        Ok(())
    })();
    *ready = Some(result.as_ref().map(|_| ()).map_err(|e| format!("{e:#}")));
    result
}

fn ensure_paddle(state: &AppState) -> Result<Arc<PaddleOcr>> {
    ensure_ort(state)?;
    let (rec, gpu) = {
        let s = state.settings.read();
        (ocr::language(&s.ocr_language).rec.to_string(), s.use_gpu && cfg!(windows))
    };
    let mut slot = state.paddle.lock();
    if let Some((r, g, engine)) = slot.as_ref() {
        if *r == rec && *g == gpu {
            return Ok(engine.clone());
        }
    }
    let models = state.resource_dir.join("models");
    // Drop the previous cached models before allocating their replacements.
    *slot = None;
    match PaddleOcr::new(&models, &rec, gpu) {
        Ok(engine) => {
            let engine = Arc::new(engine);
            engine.warm_up();
            *slot = Some((rec, gpu, engine.clone()));
            *state.engine_error.write() = None;
            Ok(engine)
        }
        Err(e) => {
            *state.engine_error.write() = Some(format!("{e:#}"));
            Err(e)
        }
    }
}

fn lens_engine(state: &AppState) -> Result<Arc<LensOcr>> {
    let mut slot = state.lens.lock();
    if let Some(l) = slot.as_ref() {
        return Ok(l.clone());
    }
    let l = Arc::new(LensOcr::new()?);
    *slot = Some(l.clone());
    Ok(l)
}

// ---------------------------------------------------------------------------------------------
// Dictionaries

fn load_dictionaries(state: &AppState) {
    let configs: Vec<DictionaryConfig> = state.settings.read().dictionaries.clone();
    let mut loaded: DictList = Vec::new();
    let mut errors = HashMap::new();
    for c in configs.iter().filter(|c| c.enabled && c.kind == DictionaryKind::Mdict) {
        if let Some(d) = state.dicts.read().iter().find(|d| d.id == c.id) {
            loaded.push(d.clone());
            continue;
        }
        match Dictionary::open(&c.id, std::path::Path::new(&c.path)) {
            Ok(d) => loaded.push(Arc::new(d)),
            Err(e) => {
                log::warn!("dictionary {}: {e:#}", c.path);
                errors.insert(c.id.clone(), format!("{e:#}"));
            }
        }
    }
    *state.dicts.write() = loaded;
    *state.dict_errors.write() = errors;
}

fn dictionary_infos(state: &AppState) -> Vec<DictInfo> {
    let settings = state.settings.read();
    let dicts = state.dicts.read();
    let errors = state.dict_errors.read();
    settings
        .dictionaries
        .iter()
        .map(|c| {
            let d = dicts.iter().find(|d| d.id == c.id);
            DictInfo {
                id: c.id.clone(),
                kind: c.kind,
                name: d.map(|d| d.name.clone()).unwrap_or_else(|| c.name.clone()),
                path: c.path.clone(),
                enabled: c.enabled,
                loaded: d.is_some() || c.kind != DictionaryKind::Mdict,
                entries: d.map(|d| d.mdx.len()).unwrap_or(0),
                resource_files: d.map(|d| d.mdds.len()).unwrap_or(0),
                error: errors.get(&c.id).cloned(),
            }
        })
        .collect()
}

fn new_dict_id(path: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    format!("d{:x}", h.finish())
}

// ---------------------------------------------------------------------------------------------
// Lookup

fn lookup_words(state: &AppState, candidates: &[String]) -> Vec<EntryRef> {
    let dicts = state.dicts.read().clone();
    dict::lookup(&dicts, candidates)
}

fn manual_candidates(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.chars().next().map(ocr::is_unspaced_script).unwrap_or(false) {
        let chars: Vec<char> = text.chars().take(16).collect();
        (1..=chars.len()).rev().map(|n| chars[..n].iter().collect()).collect()
    } else {
        dict::candidate_forms(text)
    }
}

pub fn scan(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let result = run_scan(&app);
        state.busy.store(false, Ordering::SeqCst);
        let _ = app.emit("engine-status", ());
        if let Err(e) = result {
            log::warn!("lookup failed: {e:#}");
            let payload = LookupPayload { error: Some(format!("{e:#}")), ..Default::default() };
            show_popup(&app, &payload, None);
        }
    });
}

/// Finds (or records) the profile of the app in front, so it shows up in Settings › Games.
fn profile_for(app: &AppHandle, fg: &ForegroundApp) -> AppProfile {
    let state = app.state::<AppState>();
    if fg.executable.is_empty() {
        return AppProfile::default();
    }
    if let Some(p) = state.settings.read().profiles.iter().find(|p| p.executable.eq_ignore_ascii_case(&fg.executable)) {
        return p.clone();
    }
    let profile = AppProfile { executable: fg.executable.clone(), display_name: fg.title.clone(), ..Default::default() };
    {
        let mut s = state.settings.write();
        s.profiles.push(profile.clone());
        let _ = settings::save(&state.config_dir, &s);
    }
    let _ = app.emit("settings-changed", ());
    profile
}

fn run_scan(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    #[cfg(target_os = "linux")]
    if platform::session_kind() == SessionKind::KdeWayland {
        platform::kwin::track_for(std::time::Duration::from_secs(3));
    }
    // A new lookup replaces the previous overlay (and resumes a frozen app).
    close_overlay(app);
    if let Some(popup) = app.get_webview_window("popup") {
        let was_visible = popup.is_visible().unwrap_or(false);
        let _ = popup.hide();
        // Give the compositor one frame to remove our popup from the next captured bitmap.
        if was_visible { std::thread::sleep(std::time::Duration::from_millis(20)); }
    }

    let settings = state.settings.read().clone();
    let fg = platform::foreground_app();
    let profile = fg.as_ref().map(|f| profile_for(app, f)).unwrap_or_default();
    let (cx, cy) = platform::cursor_position(app)?;
    let capture = match settings.ocr_scope {
        OcrScope::Screen => platform::capture_screen(cx, cy)?,
        OcrScope::Region => platform::capture_around(cx, cy, settings.capture_width, settings.capture_height)?,
    };
    let frozen_pid = match (profile.freeze, fg.as_ref()) {
        (FreezeMode::Process, Some(f)) if f.pid != 0 => {
            platform::suspend_process(f.pid);
            Some(f.pid)
        }
        _ => None,
    };
    let (px, py) = capture.to_image(cx, cy);

    let started = Instant::now();
    let ocr_key = frame_key(&capture.image, &format!("{:?}|{}|{}|{}", settings.ocr_engine, settings.ocr_language, settings.use_gpu,
        if settings.ocr_engine == OcrEngineKind::Lens { settings.lens_api_key.as_str() } else { "" }));
    let mut notice = None;
    let mut rest: Vec<TextBox> = Vec::new();
    let mut paddle_engine: Option<Arc<PaddleOcr>> = None;
    let (lines, engine) = match settings.ocr_engine {
        OcrEngineKind::Lens => match cached_lens(&state, &capture.image, ocr_key, &settings) {
            Ok(lines) => (lines, "Google Lens".to_string()),
            Err(e) => {
                notice = Some(format!("Google Lens failed, used PP-OCRv6 instead: {e:#}"));
                let p = ensure_paddle(&state)?;
                let boxes = p.detect_boxes(&capture.image)?;
                let (focus, others) = split_focus(&boxes, px, py);
                if settings.ocr_scope == OcrScope::Screen { rest = others; }
                let lines = p.recognize_boxes(&capture.image, &focus);
                paddle_engine = Some(p.clone());
                (lines, format!("PP-OCRv6 {}", p.det_model()))
            }
        },
        OcrEngineKind::Paddle => {
            let p = ensure_paddle(&state)?;
            let cached_boxes = state.ocr_cache.lock().boxes(ocr_key);
            let boxes = match cached_boxes {
                Some(boxes) => boxes,
                None => {
                    let boxes = p.detect_boxes(&capture.image)?;
                    state.ocr_cache.lock().save_boxes(ocr_key, &boxes);
                    boxes
                }
            };
            let (focus, others) = split_focus(&boxes, px, py);
            if settings.ocr_scope == OcrScope::Screen { rest = others; }
            let lines = cached_recognize(&state, &p, &capture.image, ocr_key, &focus);
            let gpu = if p.det_on_gpu() { "GPU" } else { "CPU" };
            paddle_engine = Some(p.clone());
            (lines, format!("PP-OCRv6 {} · {gpu}", p.det_model()))
        }
    };
    let ocr_ms = started.elapsed().as_millis() as u64;

    let monitor = app
        .monitor_from_point(cx, cy)
        .ok()
        .flatten()
        .map(|m| (m.position().x as f64, m.position().y as f64, m.scale_factor()))
        .unwrap_or((capture.origin_x, capture.origin_y, 1.0));
    let id = state.recognition_seq.fetch_add(1, Ordering::SeqCst) + 1;
    *state.recognition.lock() = Some(Recognition {
        id,
        capture: capture.clone(),
        lines: lines.clone(),
        focus_lines: lines.clone(),
        monitor,
        app: fg.clone(),
        frozen_pid,
        freeze_frame: profile.freeze == FreezeMode::Frame,
        engine: engine.clone(),
        ocr_ms,
    });

    let pick = words::pick(&lines, px, py);
    let anchor = pick.as_ref().map(|p| {
        let (x0, y0) = capture.to_desktop(p.x0, p.y0);
        let (x1, y1) = capture.to_desktop(p.x1, p.y1);
        (x0, y0, x1, y1)
    });
    show_overlay(app, id, anchor, rest.is_empty(), profile.focus_overlay);

    let payload = match pick {
        None => LookupPayload {
            engine,
            ocr_ms,
            notice,
            error: Some("No text found under the pointer.".into()),
            app: fg,
            recognition: Some(id),
            ..Default::default()
        },
        Some(pick) => LookupPayload {
            entries: lookup_words(&state, &pick.candidates),
            word: pick.word,
            sentence: pick.sentence,
            candidates: pick.candidates,
            engine,
            ocr_ms,
            notice,
            error: None,
            manual: false,
            app: fg,
            recognition: Some(id),
        },
    };
    show_popup(app, &payload, anchor.or(Some((cx, cy, cx, cy))));

    // Recognize the rest of the screen in the background so every word becomes clickable.
    if let (Some(p), false) = (paddle_engine, rest.is_empty()) {
        let app = app.clone();
        let image = capture.image.clone();
        std::thread::spawn(move || {
            for chunk in rest.chunks(8) {
                // A new lookup supersedes this overlay; do not run another stale OCR batch.
                if !matches!(app.state::<AppState>().recognition.lock().as_ref(), Some(r) if r.id == id) {
                    return;
                }
                let more = if settings.ocr_engine == OcrEngineKind::Paddle {
                    cached_recognize(&app.state::<AppState>(), &p, &image, ocr_key, chunk)
                } else {
                    p.recognize_boxes(&image, chunk)
                };
                let state = app.state::<AppState>();
                {
                    let mut guard = state.recognition.lock();
                    match guard.as_mut() {
                        Some(r) if r.id == id => r.lines.extend(more),
                        _ => return,
                    }
                }
                emit_overlay(&app, id, None, false);
            }
            emit_overlay(&app, id, None, true);
            let _ = app.emit("engine-status", ());
        });
    }
    Ok(())
}

fn cached_recognize(state: &AppState, engine: &PaddleOcr, image: &image::RgbaImage, key: FrameKey, boxes: &[TextBox]) -> Vec<OcrLine> {
    let cached = state.ocr_cache.lock().lines(key, boxes);
    if let Some(lines) = cached { return lines; }
    let lines = engine.recognize_boxes(image, boxes);
    state.ocr_cache.lock().save_lines(key, boxes, &lines);
    lines
}

fn cached_lens(state: &AppState, image: &image::RgbaImage, key: FrameKey, settings: &Settings) -> Result<Vec<OcrLine>> {
    let cached = {
        let mut cache = state.ocr_cache.lock();
        cache.boxes(key).and_then(|boxes| cache.lines(key, &boxes))
    };
    if let Some(lines) = cached { return Ok(lines); }
    let lines = lens_engine(state)?.recognize(image, &settings.lens_api_key, &settings.ocr_language)?;
    let boxes: Vec<TextBox> = lines.iter().map(|line| TextBox { x0: line.x0, y0: line.y0, x1: line.x1, y1: line.y1 }).collect();
    let mut cache = state.ocr_cache.lock();
    cache.save_boxes(key, &boxes);
    cache.save_lines(key, &boxes, &lines);
    Ok(lines)
}

fn css_rect(r: &Recognition, x0: f64, y0: f64, x1: f64, y1: f64) -> (f64, f64, f64, f64) {
    let (mx, my, s) = r.monitor;
    ((x0 - mx) / s, (y0 - my) / s, (x1 - x0) / s, (y1 - y0) / s)
}

fn emit_overlay(app: &AppHandle, id: u64, selected: Option<(f64, f64, f64, f64)>, complete: bool) {
    let state = app.state::<AppState>();
    let payload = {
        let guard = state.recognition.lock();
        let Some(r) = guard.as_ref().filter(|r| r.id == id) else { return };
        let words = words::word_boxes(&r.lines)
            .into_iter()
            .map(|w| {
                let (x0, y0) = r.capture.to_desktop(w.x0, w.y0);
                let (x1, y1) = r.capture.to_desktop(w.x1, w.y1);
                let (x, y, w2, h) = css_rect(r, x0, y0, x1, y1);
                OverlayWord { text: w.text, x, y, w: w2, h }
            })
            .collect();
        let selected = selected.map(|(x0, y0, x1, y1)| {
            let (x, y, w, h) = css_rect(r, x0, y0, x1, y1);
            OverlayWord { text: String::new(), x, y, w, h }
        });
        OverlayPayload {
            id,
            words,
            selected,
            frame_url: r.freeze_frame.then(|| format!("/c/{id}/frame.png")),
            complete,
        }
    };
    let _ = app.emit_to("overlay", "overlay", &payload);
}

fn show_overlay(app: &AppHandle, id: u64, anchor: Option<(f64, f64, f64, f64)>, complete: bool, focus: bool) {
    let Some(win) = app.get_webview_window("overlay") else { return };
    let state = app.state::<AppState>();
    let (mx, my) = state.recognition.lock().as_ref().map(|r| (r.monitor.0, r.monitor.1)).unwrap_or((0.0, 0.0));
    if let Ok(Some(m)) = app.monitor_from_point(mx + 1.0, my + 1.0) {
        let _ = win.set_position(*m.position());
        let _ = win.set_size(*m.size());
    }
    emit_overlay(app, id, anchor, complete);
    let _ = win.show();
    if focus {
        let _ = win.set_focus();
    }
}

fn close_overlay(app: &AppHandle) {
    let state = app.state::<AppState>();
    let frozen = state.recognition.lock().as_mut().and_then(|r| r.frozen_pid.take());
    if let Some(pid) = frozen {
        platform::resume_process(pid);
    }
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
    }
}

/// Sends the result to the popup and shows it next to the word (desktop coordinates).
fn show_popup(app: &AppHandle, payload: &LookupPayload, anchor: Option<(f64, f64, f64, f64)>) {
    let Some(win) = app.get_webview_window("popup") else { return };
    let _ = app.emit_to("popup", "lookup", payload);
    if let Some((x0, y0, _x1, y1)) = anchor {
        let size = win.outer_size().unwrap_or(PhysicalSize::new(560, 520));
        let (w, h) = (size.width as f64, size.height as f64);
        let mut x = x0 - 24.0;
        let mut y = y1 + 10.0;
        if let Ok(Some(m)) = app.monitor_from_point(x0, y0) {
            let (mx, my) = (m.position().x as f64, m.position().y as f64);
            let (mw, mh) = (m.size().width as f64, m.size().height as f64);
            if y + h > my + mh {
                y = (y0 - h - 10.0).max(my);
            }
            x = x.clamp(mx, (mx + mw - w).max(mx));
        }
        let _ = win.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    }
    let _ = win.show();
    let _ = win.set_focus();
}

/// PNG of the last capture: the whole frame, or the lines around the word with some margin.
pub fn capture_png(state: &AppState, id: Option<u64>, area: &str) -> Option<Vec<u8>> {
    let guard = state.recognition.lock();
    let r = guard.as_ref().filter(|r| id.map(|i| i == r.id).unwrap_or(true))?;
    let img = &r.capture.image;
    let cropped = if area == "sentence" && !r.focus_lines.is_empty() {
        let pad = 24.0;
        let x0 = r.focus_lines.iter().map(|l| l.x0).fold(f32::MAX, f32::min) - pad;
        let y0 = r.focus_lines.iter().map(|l| l.y0).fold(f32::MAX, f32::min) - pad;
        let x1 = r.focus_lines.iter().map(|l| l.x1).fold(0.0, f32::max) + pad;
        let y1 = r.focus_lines.iter().map(|l| l.y1).fold(0.0, f32::max) + pad;
        let x = x0.max(0.0) as u32;
        let y = y0.max(0.0) as u32;
        let w = (x1.min(img.width() as f32) as u32).saturating_sub(x).max(1);
        let h = (y1.min(img.height() as f32) as u32).saturating_sub(y).max(1);
        image::imageops::crop_imm(img, x, y, w, h).to_image()
    } else {
        img.clone()
    };
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(cropped).write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
}

// ---------------------------------------------------------------------------------------------
// Settings, shortcuts, windows

fn apply_hotkey(app: &AppHandle, hotkey: &str) -> std::result::Result<(), String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let hotkey = hotkey.trim();
    if hotkey.is_empty() {
        return Ok(());
    }
    gs.on_shortcut(hotkey, |app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            scan(app);
        }
    })
    .map_err(|e| format!("could not register shortcut {hotkey}: {e}"))
}

fn apply_mouse_hotkey(app: &AppHandle, cfg: &settings::MouseHotkey) {
    #[cfg(windows)]
    {
        let handle = app.clone();
        platform::win::set_mouse_hotkey(cfg.clone(), move || scan(&handle));
    }
    #[cfg(not(windows))]
    {
        let _ = (app, cfg);
    }
}

fn apply_autostart(app: &AppHandle, enabled: bool) {
    let launcher = app.autolaunch();
    let r = if enabled { launcher.enable() } else { launcher.disable() };
    if let Err(e) = r {
        log::warn!("autostart: {e}");
    }
}

fn open_settings_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn warm_engine_in_background(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        // PP-OCRv6 is loaded even when Google Lens is selected: it is the fallback.
        if let Err(e) = ensure_paddle(&state) {
            log::warn!("OCR engine unavailable: {e:#}");
        }
        let _ = app.emit("engine-status", ());
    });
}

// ---------------------------------------------------------------------------------------------
// Commands

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> Settings {
    state.settings.read().clone()
}

#[tauri::command]
fn save_settings(app: AppHandle, state: tauri::State<'_, AppState>, settings: Settings) -> std::result::Result<(), String> {
    let previous = state.settings.read().clone();
    // Dictionary files are managed by their own commands; keep the stored list.
    let mut next = settings;
    next.dictionaries = previous.dictionaries.clone();
    settings::save(&state.config_dir, &next).map_err(|e| format!("{e:#}"))?;
    let engine_changed = previous.ocr_language != next.ocr_language || previous.use_gpu != next.use_gpu;
    *state.settings.write() = next.clone();
    if engine_changed {
        warm_engine_in_background(&app);
    }
    if previous.autostart != next.autostart {
        apply_autostart(&app, next.autostart);
    }
    apply_mouse_hotkey(&app, &next.mouse_hotkey);
    if previous.hotkey != next.hotkey {
        apply_hotkey(&app, &next.hotkey)?;
    }
    let _ = app.emit("settings-changed", ());
    Ok(())
}

#[tauri::command]
fn engine_status(state: tauri::State<'_, AppState>) -> EngineStatus {
    // This synchronous command runs on the UI thread. Model initialization holds these
    // locks for seconds; report a loading snapshot instead of waiting and freezing the WebView.
    let ort = state.ort_ready.try_lock().and_then(|ready| ready.clone());
    let paddle = state.paddle.try_lock().and_then(|slot| slot.as_ref().map(|(_, _, e)| e.clone()));
    EngineStatus {
        session: platform::session_kind(),
        runtime_loaded: matches!(ort, Some(Ok(()))),
        runtime_error: ort.and_then(|r| r.err()),
        det_model: paddle.as_ref().map(|p| p.det_model().to_string()),
        det_on_gpu: paddle.as_ref().map(|p| p.det_on_gpu()).unwrap_or(false),
        rec_model: paddle.as_ref().map(|p| p.rec_model.clone()),
        rec_on_gpu: paddle.as_ref().map(|p| p.rec_on_gpu()).unwrap_or(false),
        error: state.engine_error.read().clone(),
    }
}

#[tauri::command]
fn ocr_languages() -> Vec<LanguageInfo> {
    ocr::LANGUAGES.iter().map(|l| LanguageInfo { tag: l.tag, name: l.name }).collect()
}

#[tauri::command]
fn list_dictionaries(state: tauri::State<'_, AppState>) -> Vec<DictInfo> {
    dictionary_infos(&state)
}

#[tauri::command]
async fn add_dictionary(app: AppHandle, path: String) -> std::result::Result<Vec<DictInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let id = new_dict_id(&path);
        if state.settings.read().dictionaries.iter().any(|d| d.id == id) {
            return Err("This dictionary is already installed.".to_string());
        }
        let dict = Dictionary::open(&id, std::path::Path::new(&path)).map_err(|e| format!("{e:#}"))?;
        let name = dict.name.clone();
        {
            let mut s = state.settings.write();
            s.dictionaries.push(DictionaryConfig { id: id.clone(), kind: DictionaryKind::Mdict, path: path.clone(), name, enabled: true });
            settings::save(&state.config_dir, &s).map_err(|e| format!("{e:#}"))?;
        }
        state.dicts.write().push(Arc::new(dict));
        load_dictionaries(&state);
        Ok(dictionary_infos(&state))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn remove_dictionary(state: tauri::State<'_, AppState>, id: String) -> std::result::Result<Vec<DictInfo>, String> {
    {
        let mut s = state.settings.write();
        if s.dictionaries.iter().any(|d| d.id == id && d.kind != DictionaryKind::Mdict) {
            return Err("Built-in dictionaries can be turned off but not removed.".into());
        }
        s.dictionaries.retain(|d| d.id != id);
        settings::save(&state.config_dir, &s).map_err(|e| format!("{e:#}"))?;
    }
    state.dicts.write().retain(|d| d.id != id);
    Ok(dictionary_infos(&state))
}

#[tauri::command]
async fn update_dictionary(app: AppHandle, id: String, enabled: Option<bool>, move_to: Option<usize>) -> std::result::Result<Vec<DictInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        {
            let mut s = state.settings.write();
            let Some(pos) = s.dictionaries.iter().position(|d| d.id == id) else {
                return Err("unknown dictionary".to_string());
            };
            if let Some(e) = enabled {
                s.dictionaries[pos].enabled = e;
            }
            if let Some(target) = move_to {
                let target = target.min(s.dictionaries.len() - 1);
                let item = s.dictionaries.remove(pos);
                s.dictionaries.insert(target, item);
            }
            settings::save(&state.config_dir, &s).map_err(|e| format!("{e:#}"))?;
        }
        load_dictionaries(&state);
        Ok(dictionary_infos(&state))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn lookup_text(state: tauri::State<'_, AppState>, text: String, sentence: Option<String>) -> LookupPayload {
    let candidates = manual_candidates(&text);
    let entries = lookup_words(&state, &candidates);
    LookupPayload {
        word: text.trim().to_string(),
        sentence: sentence.unwrap_or_default(),
        candidates,
        entries,
        manual: true,
        ..Default::default()
    }
}

/// A click on the overlay (CSS pixels of the overlay window): look up the word there.
#[tauri::command]
fn overlay_pick(app: AppHandle, state: tauri::State<'_, AppState>, x: f64, y: f64) {
    let (lines, capture, monitor, app_info, id, engine, ocr_ms) = {
        let guard = state.recognition.lock();
        let Some(r) = guard.as_ref() else { return };
        (r.lines.clone(), r.capture.clone(), r.monitor, r.app.clone(), r.id, r.engine.clone(), r.ocr_ms)
    };
    let (mx, my, s) = monitor;
    let (dx, dy) = (mx + x * s, my + y * s);
    let (px, py) = capture.to_image(dx, dy);
    let Some(pick) = words::pick(&lines, px, py) else { return };
    if let Some(r) = state.recognition.lock().as_mut() {
        let h = (pick.y1 - pick.y0).max(4.0);
        r.focus_lines = lines
            .iter()
            .filter(|l| l.y1 >= pick.y0 - 1.6 * h && l.y0 <= pick.y1 + 1.6 * h)
            .cloned()
            .collect();
    }
    let (x0, y0) = capture.to_desktop(pick.x0, pick.y0);
    let (x1, y1) = capture.to_desktop(pick.x1, pick.y1);
    let payload = LookupPayload {
        entries: lookup_words(&state, &pick.candidates),
        word: pick.word,
        sentence: pick.sentence,
        candidates: pick.candidates,
        engine,
        ocr_ms,
        app: app_info,
        recognition: Some(id),
        ..Default::default()
    };
    emit_overlay(&app, id, Some((x0, y0, x1, y1)), true);
    show_popup(&app, &payload, Some((x0, y0, x1, y1)));
}

#[tauri::command]
fn overlay_close(app: AppHandle) {
    close_overlay(&app);
    if let Some(w) = app.get_webview_window("popup") {
        let _ = w.hide();
    }
}

#[tauri::command]
fn capture_image(state: tauri::State<'_, AppState>, recognition: Option<u64>, area: String) -> Option<String> {
    let png = capture_png(&state, recognition, &area)?;
    Some(base64::engine::general_purpose::STANDARD.encode(png))
}

#[tauri::command]
fn scan_now(app: AppHandle) {
    scan(&app);
}

#[tauri::command]
fn hide_popup(app: AppHandle) {
    if let Some(w) = app.get_webview_window("popup") {
        let _ = w.hide();
    }
    close_overlay(&app);
}

#[tauri::command]
fn set_popup_pinned(state: tauri::State<'_, AppState>, pinned: bool) {
    state.pinned.store(pinned, Ordering::SeqCst);
}

#[tauri::command]
fn open_external(app: AppHandle, url: String) -> std::result::Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) links can be opened".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    open_settings_window(&app);
}

#[tauri::command]
fn entry_html(state: tauri::State<'_, AppState>, dict_id: String, entry: u32) -> std::result::Result<String, String> {
    let dicts = state.dicts.read().clone();
    let d = dicts.iter().find(|d| d.id == dict_id).ok_or("unknown dictionary")?;
    d.entry_html(entry).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn list_words(state: tauri::State<'_, AppState>) -> Vec<serde_json::Value> {
    state.cards.list()
}

#[tauri::command]
fn save_word(app: AppHandle, state: tauri::State<'_, AppState>, card: serde_json::Value) -> std::result::Result<serde_json::Value, String> {
    let saved = state.cards.save(card).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("words-changed", ());
    Ok(saved)
}

#[tauri::command]
fn delete_word(app: AppHandle, state: tauri::State<'_, AppState>, id: String) -> std::result::Result<(), String> {
    state.cards.delete(&id).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("words-changed", ());
    Ok(())
}

/// Writes an export chosen by the user in the save dialog.
#[tauri::command]
fn write_text_file(path: String, content: String) -> std::result::Result<(), String> {
    std::fs::write(&path, content).map_err(|e| format!("{path}: {e}"))
}

#[tauri::command]
fn word_image(state: tauri::State<'_, AppState>, name: String) -> Option<String> {
    state.cards.media(&name).map(|b| base64::engine::general_purpose::STANDARD.encode(b))
}

// ---------------------------------------------------------------------------------------------

pub fn run() {
    // On KDE Plasma Wayland the windows run through XWayland so the pop-up can be placed next to the
    // word and kept above other windows; pointer and capture come from KWin (see platform::kwin).
    #[cfg(target_os = "linux")]
    if platform::session_kind() == SessionKind::KdeWayland && std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "x11");
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let wants_scan = std::env::args().any(|a| a == "--scan");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if argv.iter().any(|a| a == "--scan") {
                scan(app);
            } else {
                open_settings_window(app);
            }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .register_uri_scheme_protocol("lkt", |ctx, request| protocol::handle(ctx.app_handle(), &request))
        .setup(move |app| {
            let handle = app.handle().clone();
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            let resource_dir = app.path().resource_dir()?;
            let first_run = !settings::settings_path(&config_dir).exists();
            let settings = settings::load(&config_dir);
            let hotkey = settings.hotkey.clone();
            let mouse = settings.mouse_hotkey.clone();
            app.manage(AppState {
                config_dir,
                resource_dir,
                settings: RwLock::new(settings),
                dicts: RwLock::new(Vec::new()),
                dict_errors: RwLock::new(HashMap::new()),
                paddle: Mutex::new(None),
                engine_error: RwLock::new(None),
                ort_ready: Mutex::new(None),
                lens: Mutex::new(None),
                recognition: Mutex::new(None),
                ocr_cache: Mutex::new(OcrCache::default()),
                recognition_seq: AtomicU64::new(0),
                cards: CardStore::open(&data_dir),
                busy: AtomicBool::new(false),
                pinned: AtomicBool::new(false),
            });

            // Configured WebViews can invoke get_settings before setup reaches manage().
            // Create them explicitly once AppState exists so every window's first IPC is safe.
            for config in app.config().app.windows.clone() {
                WebviewWindowBuilder::from_config(app, &config)?.build()?;
            }

            if let Err(e) = apply_hotkey(&handle, &hotkey) {
                log::warn!("{e}");
            }
            apply_mouse_hotkey(&handle, &mouse);

            #[cfg(target_os = "linux")]
            if platform::session_kind() == SessionKind::KdeWayland {
                if let Err(e) = platform::kwin::start(&data_dir) {
                    log::warn!("KWin integration unavailable: {e:#}");
                }
            }

            // Load dictionaries and warm the OCR engine without blocking start-up.
            {
                let handle = handle.clone();
                std::thread::spawn(move || {
                    let state = handle.state::<AppState>();
                    load_dictionaries(&state);
                    let _ = handle.emit("dictionaries-changed", ());
                });
            }
            warm_engine_in_background(&handle);

            let settings_item = MenuItem::with_id(app, "settings", "Lookitan", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings_item, &quit_item])?;
            let mut tray = TrayIconBuilder::with_id("main").menu(&menu).tooltip("Lookitan");
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.on_menu_event(|app, event| match event.id.as_ref() {
                "settings" => open_settings_window(app),
                "quit" => {
                    close_overlay(app);
                    #[cfg(target_os = "linux")]
                    platform::kwin::stop();
                    app.exit(0);
                }
                _ => {}
            })
            .build(app)?;

            if let Some(popup) = app.get_webview_window("popup") {
                let p = popup.clone();
                let h = handle.clone();
                popup.on_window_event(move |event| {
                    if let WindowEvent::Focused(false) = event {
                        if !h.state::<AppState>().pinned.load(Ordering::SeqCst) {
                            let _ = p.hide();
                        }
                    }
                });
            }
            if let Some(main) = app.get_webview_window("main") {
                let m = main.clone();
                main.on_window_event(move |event| {
                    // Closing the main window keeps Lookitan running in the tray.
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = m.hide();
                    }
                });
                if first_run {
                    let _ = main.show();
                }
            }
            if wants_scan {
                scan(&handle);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            engine_status,
            ocr_languages,
            list_dictionaries,
            add_dictionary,
            remove_dictionary,
            update_dictionary,
            lookup_text,
            overlay_pick,
            overlay_close,
            capture_image,
            scan_now,
            hide_popup,
            set_popup_pinned,
            open_external,
            open_settings,
            entry_html,
            list_words,
            save_word,
            delete_word,
            word_image,
            write_text_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running Lookitan");
}
