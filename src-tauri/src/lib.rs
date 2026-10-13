// Lookitan: OCR pop-up dictionary with bring-your-own-key AI.
// Copyright (C) 2026 arrow-poppa
//
// This program is free software: you can redistribute it and/or modify it under the terms of the
// GNU General Public License version 3 as published by the Free Software Foundation.

mod dict;
mod mdict;
mod ocr;
mod platform;
mod protocol;
mod settings;
mod words;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;

use crate::dict::{DictList, Dictionary, EntryRef};
use crate::ocr::lens::LensOcr;
use crate::ocr::paddle::PaddleOcr;
use crate::platform::SessionKind;
use crate::settings::{DictionaryConfig, OcrEngineKind, Settings};

pub struct AppState {
    config_dir: PathBuf,
    data_dir: PathBuf,
    resource_dir: PathBuf,
    pub settings: RwLock<Settings>,
    pub dicts: RwLock<DictList>,
    dict_errors: RwLock<HashMap<String, String>>,
    paddle: Mutex<Option<(String, bool, Arc<PaddleOcr>)>>,
    engine_error: RwLock<Option<String>>,
    ort_ready: Mutex<Option<std::result::Result<(), String>>>,
    lens: Mutex<Option<Arc<LensOcr>>>,
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
    for c in configs.iter().filter(|c| c.enabled) {
        // Reuse dictionaries that are already open.
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
                name: d.map(|d| d.name.clone()).unwrap_or_else(|| c.name.clone()),
                path: c.path.clone(),
                enabled: c.enabled,
                loaded: d.is_some(),
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
        if let Err(e) = result {
            log::warn!("lookup failed: {e:#}");
            let payload = LookupPayload { error: Some(format!("{e:#}")), ..Default::default() };
            show_popup(&app, &payload, None);
        }
    });
}

fn run_scan(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    #[cfg(target_os = "linux")]
    if platform::session_kind() == SessionKind::KdeWayland {
        platform::kwin::track_for(std::time::Duration::from_secs(3));
    }
    let settings = state.settings.read().clone();
    let (cx, cy) = platform::cursor_position(app)?;
    let capture = platform::capture_around(cx, cy, settings.capture_width, settings.capture_height)?;
    let (px, py) = capture.to_image(cx, cy);

    let started = Instant::now();
    let mut notice = None;
    let (lines, engine) = match settings.ocr_engine {
        OcrEngineKind::Lens => {
            match lens_engine(&state).and_then(|l| l.recognize(&capture.image, &settings.lens_api_key, &settings.ocr_language)) {
                Ok(lines) => (lines, "Google Lens".to_string()),
                Err(e) => {
                    notice = Some(format!("Google Lens failed, used PP-OCRv6 instead: {e:#}"));
                    let p = ensure_paddle(&state)?;
                    (p.recognize(&capture.image, Some((px, py)))?, format!("PP-OCRv6 ({})", p.det_model))
                }
            }
        }
        OcrEngineKind::Paddle => {
            let p = ensure_paddle(&state)?;
            let gpu = if p.det_on_gpu { "GPU" } else { "CPU" };
            (p.recognize(&capture.image, Some((px, py)))?, format!("PP-OCRv6 {} · {gpu}", p.det_model))
        }
    };
    let ocr_ms = started.elapsed().as_millis() as u64;

    let Some(pick) = words::pick(&lines, px, py) else {
        let payload = LookupPayload {
            engine,
            ocr_ms,
            notice,
            error: Some("No text found under the pointer.".into()),
            ..Default::default()
        };
        show_popup(app, &payload, Some((cx, cy, cx, cy)));
        return Ok(());
    };
    let entries = lookup_words(&state, &pick.candidates);
    let (ax0, ay0) = capture.to_desktop(pick.x0, pick.y0);
    let (ax1, ay1) = capture.to_desktop(pick.x1, pick.y1);
    let payload = LookupPayload {
        word: pick.word,
        sentence: pick.sentence,
        candidates: pick.candidates,
        entries,
        engine,
        ocr_ms,
        notice,
        error: None,
        manual: false,
    };
    show_popup(app, &payload, Some((ax0, ay0, ax1, ay1)));
    Ok(())
}

/// Sends the result to the popup and shows it next to the word (desktop coordinates).
fn show_popup(app: &AppHandle, payload: &LookupPayload, anchor: Option<(f64, f64, f64, f64)>) {
    let Some(win) = app.get_webview_window("popup") else { return };
    let _ = app.emit_to("popup", "lookup", payload);
    if let Some((x0, y0, _x1, y1)) = anchor {
        let size = win.outer_size().unwrap_or(tauri::PhysicalSize::new(460, 540));
        let (w, h) = (size.width as f64, size.height as f64);
        let mut x = x0 - 12.0;
        let mut y = y1 + 8.0;
        if let Ok(Some(m)) = app.monitor_from_point(x0, y0) {
            let (mx, my) = (m.position().x as f64, m.position().y as f64);
            let (mw, mh) = (m.size().width as f64, m.size().height as f64);
            if y + h > my + mh {
                y = (y0 - h - 8.0).max(my);
            }
            x = x.clamp(mx, (mx + mw - w).max(mx));
        }
        let _ = win.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    }
    let _ = win.show();
    let _ = win.set_focus();
}

// ---------------------------------------------------------------------------------------------
// Settings, shortcut, windows

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
    // Dictionaries are managed by their own commands; keep the stored list.
    let mut next = settings;
    next.dictionaries = previous.dictionaries.clone();
    settings::save(&state.config_dir, &next).map_err(|e| format!("{e:#}"))?;
    let hotkey_changed = previous.hotkey != next.hotkey;
    let engine_changed = previous.ocr_language != next.ocr_language || previous.use_gpu != next.use_gpu;
    *state.settings.write() = next.clone();
    if engine_changed {
        warm_engine_in_background(&app);
    }
    if hotkey_changed {
        apply_hotkey(&app, &next.hotkey)?;
    }
    Ok(())
}

#[tauri::command]
fn engine_status(state: tauri::State<'_, AppState>) -> EngineStatus {
    let ort = state.ort_ready.lock().clone();
    let paddle = state.paddle.lock().as_ref().map(|(_, _, e)| e.clone());
    EngineStatus {
        session: platform::session_kind(),
        runtime_loaded: matches!(ort, Some(Ok(()))),
        runtime_error: ort.and_then(|r| r.err()),
        det_model: paddle.as_ref().map(|p| p.det_model.to_string()),
        det_on_gpu: paddle.as_ref().map(|p| p.det_on_gpu).unwrap_or(false),
        rec_model: paddle.as_ref().map(|p| p.rec_model.clone()),
        rec_on_gpu: paddle.as_ref().map(|p| p.rec_on_gpu).unwrap_or(false),
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
            s.dictionaries.push(DictionaryConfig { id: id.clone(), path: path.clone(), name, enabled: true });
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
        s.dictionaries.retain(|d| d.id != id);
        settings::save(&state.config_dir, &s).map_err(|e| format!("{e:#}"))?;
    }
    state.dicts.write().retain(|d| d.id != id);
    Ok(dictionary_infos(&state))
}

#[tauri::command]
async fn update_dictionary(app: AppHandle, id: String, enabled: Option<bool>, move_by: Option<i32>) -> std::result::Result<Vec<DictInfo>, String> {
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
            if let Some(delta) = move_by {
                let target = (pos as i32 + delta).clamp(0, s.dictionaries.len() as i32 - 1) as usize;
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
        engine: String::new(),
        manual: true,
        ..Default::default()
    }
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

// ---------------------------------------------------------------------------------------------

pub fn run() {
    // On KDE Plasma Wayland the pop-up runs through XWayland so it can be placed next to the word
    // and kept above other windows; pointer and capture come from KWin (see platform::kwin).
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
        .register_uri_scheme_protocol("lkt", |ctx, request| protocol::handle(ctx.app_handle(), &request))
        .setup(move |app| {
            let handle = app.handle().clone();
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            let resource_dir = app.path().resource_dir()?;
            let settings = settings::load(&config_dir);
            let first_run = !settings::settings_path(&config_dir).exists();
            let hotkey = settings.hotkey.clone();
            app.manage(AppState {
                config_dir,
                data_dir: data_dir.clone(),
                resource_dir,
                settings: RwLock::new(settings),
                dicts: RwLock::new(Vec::new()),
                dict_errors: RwLock::new(HashMap::new()),
                paddle: Mutex::new(None),
                engine_error: RwLock::new(None),
                ort_ready: Mutex::new(None),
                lens: Mutex::new(None),
                busy: AtomicBool::new(false),
                pinned: AtomicBool::new(false),
            });

            if let Err(e) = apply_hotkey(&handle, &hotkey) {
                log::warn!("{e}");
            }

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

            let settings_item = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Lookitan", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings_item, &quit_item])?;
            let mut tray = TrayIconBuilder::with_id("main").menu(&menu).tooltip("Lookitan");
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.on_menu_event(|app, event| match event.id.as_ref() {
                "settings" => open_settings_window(app),
                "quit" => {
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
                    // Closing the settings window keeps Lookitan running in the tray.
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
            scan_now,
            hide_popup,
            set_popup_pinned,
            open_external,
            open_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running Lookitan");
}
