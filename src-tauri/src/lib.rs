// Lookitan: OCR pop-up dictionary with bring-your-own-key AI.
// Copyright (C) 2026 arrow-poppa
//
// This program is free software: you can redistribute it and/or modify it under the terms of the
// GNU General Public License version 3 as published by the Free Software Foundation.

mod audio;
mod edge_tts;
mod block;
mod gamepad;
mod cards;
mod dict;
mod dictionaries;
mod layout;
mod mdict;
mod ocr;
mod platform;
mod protocol;
mod segment;
mod sentence;
mod settings;
mod words;

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use parking_lot::{Condvar, Mutex, RwLock};
use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;

use crate::cards::CardStore;
use crate::dict::{DictList, Dictionary, EntryRef};
use crate::ocr::bing::BingOcr;
use crate::ocr::lens::LensOcr;
use crate::ocr::cache::{frame_key, FrameKey, OcrCache};
use crate::ocr::ctd::CtdDet;
use crate::ocr::indra::{Group as IndraGroup, IndraDet, IndraRec};
use crate::ocr::meiki::MeikiRec;
use crate::ocr::models;
use crate::ocr::oneocr::OneOcr;
use crate::ocr::screenai::ScreenAi;
use crate::ocr::paddle::{recognize_boxes_with, split_focus, PaddleOcr, PaddleUse, TextBox, MANGA_DET, MANGA_REC};
use crate::ocr::OcrLine;
use crate::platform::{Capture, ForegroundApp, SessionKind};
use crate::platform::geometry::DisplayGeometry;
use crate::settings::{AppProfile, Detector, DictionaryConfig, DictionaryKind, FreezeMode, OcrEngineKind, OcrScope, Recognizer, Settings};

/// The last OCR pass: what the overlay shows and what clicks on it look up.
struct Recognition {
    id: u64,
    capture: Capture,
    lines: Vec<OcrLine>,
    /// Lines around the first word, used for the "sentence" screenshot.
    focus_lines: Vec<OcrLine>,
    /// Capture-space and window-space rectangles of the same output.
    monitor: DisplayGeometry,
    /// The `scan_seq` of the scan that made this recognition.
    scan: u64,
    complete: bool,
    app: Option<ForegroundApp>,
    frozen_pid: Option<u32>,
    freeze_frame: bool,
    /// Whole-screen lookup without a pop-up. With the app profile set to freeze, the overlay
    /// shows the captured frame slightly blurred and dimmed (Lookupper uses OpenCV's GaussianBlur)
    /// with the words as sharp cut-outs; without it the screen stays live, as in Lookupper.
    spotlight: bool,
    /// The whole monitor, captured with a region lookup when Anki's {screenshot} asks for the
    /// whole screen. `None` when `capture` already is the whole monitor.
    screen: Option<Capture>,
    /// The block of text the shortcut was pressed on but off any of its words (automatic
    /// capture), in image coordinates: the overlay highlights it, as Lookupper does.
    block: Option<TextBox>,
    engine: String,
    ocr_ms: u64,
    /// Why the chosen engine was not used (Google Lens failed), shown by the overlay.
    notice: Option<String>,
}

pub struct AppState {
    config_dir: PathBuf,
    /// Installed dictionaries (`content/<id>/`) and their caches, in the local (not roaming)
    /// data folder: like Lookupper's LocalState, it can hold several gigabytes.
    dictionaries_dir: PathBuf,
    resource_dir: PathBuf,
    pub settings: RwLock<Settings>,
    pub dicts: RwLock<DictList>,
    dict_errors: RwLock<HashMap<String, String>>,
    dictionary_gate: Mutex<()>,
    /// PP-OCR by the models loaded and their plan, the last used first (`ensure_paddle`).
    paddle: Mutex<Vec<(PaddleUse, Arc<PaddleOcr>)>>,
    engine_error: RwLock<Option<String>>,
    ort_ready: Mutex<Option<std::result::Result<(), String>>>,
    lens: Mutex<Option<Arc<LensOcr>>>,
    bing: Mutex<Option<Arc<BingOcr>>>,
    /// MeikiOCR's recognizer, and whether it was asked for the GPU.
    meiki: Mutex<Option<(bool, Arc<MeikiRec>)>>,
    /// IndraOCR's recognizer for one group, and whether it was asked for the GPU.
    indra: Mutex<Option<(IndraGroup, bool, Arc<IndraRec>)>>,
    /// IndraOCR's line detector, and whether it was asked for the GPU.
    linenet: Mutex<Option<(bool, Arc<IndraDet>)>>,
    /// comic-text-detector, and whether it was asked for the GPU.
    ctd: Mutex<Option<(bool, Arc<CtdDet>)>>,
    /// Chrome ScreenAI's and OneOCR's helper processes, started with their first lookup.
    screenai: ScreenAi,
    oneocr: OneOcr,
    /// Each downloaded engine's download while it runs, or why it failed (`Downloaded::id`).
    downloads: Mutex<HashMap<&'static str, String>>,
    recognition: Mutex<Option<Recognition>>,
    ocr_cache: Mutex<OcrCache>,
    recognition_seq: AtomicU64,
    /// Bumped when a scan starts, so background OCR of an older scan stops and never shows its
    /// overlay over the new one.
    scan_seq: AtomicU64,
    pub cards: CardStore,
    busy: AtomicBool,
    pinned: AtomicBool,
    blank: Blanking,
    /// The local audio databases used most recently, newest first, at most AUDIO_DBS_OPEN (audio.rs).
    audio_dbs: Mutex<Vec<(String, Arc<audio::AudioDb>)>>,
}

/// The overlay and the pop-up are hidden only once they show nothing (see `hide_blank`).
#[derive(Default)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Blanking {
    /// The last blank request each window confirmed painting, by `window_slot`.
    painted: Mutex<[u64; 2]>,
    changed: Condvar,
    token: AtomicU64,
    /// Held while a window is shown or hidden, so a late hide cannot take down a newer lookup.
    order: Mutex<[u64; 2]>,
    /// The monitor the overlay last went fullscreen on (KDE), so a lookup on the same monitor does
    /// not take it out of fullscreen and back.
    overlay_output: Mutex<Option<platform::geometry::Rect>>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LookupPayload {
    pub word: String,
    /// The sentence holding the word: context translation and Anki.
    pub sentence: String,
    /// The paragraph holding the word: what the AI reads and the pop-up shows as context.
    pub context: String,
    /// Where the word starts in `context`, in characters, when known.
    pub context_at: Option<usize>,
    /// How many of the word's characters come before a line or column break it runs across
    /// (an analyzer joined it there), for the AI's hints.
    pub line_break: Option<usize>,
    /// Forms to look up: the word as written first, then its dictionary forms and other fallbacks.
    pub candidates: Vec<String>,
    /// The analyzer's dictionary forms of the word (`Pick::lemmas`), for the AI's hints; not sent
    /// when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lemmas: Vec<String>,
    /// What found the word (`Pick::analyzer`; for typed text, the analyzer whose dictionary forms
    /// lead `candidates`), for the AI's hints; not sent when unknown.
    #[serde(skip_serializing_if = "str::is_empty")]
    pub analyzer: &'static str,
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
    /// For a word, the area that takes the pointer, `[x, y, w, h]`: wider than its drawn box.
    #[serde(skip_serializing_if = "Option::is_none")]
    hit: Option<[f64; 4]>,
    /// For a word on a turned line, how far its boxes are turned about the hit area's centre, in
    /// radians (see `WordBox::angle`).
    #[serde(skip_serializing_if = "Option::is_none")]
    angle: Option<f64>,
    /// For a piece of a word broken across lines or columns, the number its other pieces share
    /// (see `WordBox::group`): the overlay highlights them together.
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<u32>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct OverlayPayload {
    id: u64,
    words: Vec<OverlayWord>,
    /// Rectangle in the window toolkit's logical units, before WebView DPI/zoom conversion.
    selected: Option<OverlayWord>,
    frame_url: Option<String>,
    frame_rect: Option<OverlayWord>,
    spotlight: bool,
    block: Option<OverlayWord>,
    viewport_width: f64,
    viewport_height: f64,
    complete: bool,
    /// Google Lens failed and the local OCR read the screen: the overlay says so when no pop-up
    /// opened (a whole-screen lookup), as the pop-up does otherwise.
    notice: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub session: SessionKind,
    pub runtime_loaded: bool,
    pub runtime_error: Option<String>,
    /// What each selector reads with, as loaded now: [selector, models] (`engines_status`).
    pub engines: Vec<[String; 2]>,
    pub error: Option<String>,
    /// The word analyzer for the recognition language (segment.rs), for Japanese, Chinese and Korean.
    pub segmenter: Option<String>,
    /// Whether that analyzer's files are downloaded.
    pub segmenter_installed: Option<bool>,
    /// The engines whose files are downloaded when chosen, by `Downloaded::id`.
    pub models: HashMap<&'static str, ModelState>,
    /// Where MeikiOCR's recognizer runs, once loaded.
    pub meiki_on_gpu: Option<bool>,
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
    pub format: String,
    pub description: String,
    pub warnings: Vec<String>,
    /// The dictionary is a copy Lookitan unpacked into its own folder (removing it deletes it).
    pub installed: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct LanguageInfo {
    pub tag: &'static str,
    pub name: &'static str,
    /// Its recognition model by default; with `rec_tiny`, it can switch to `rec_small`.
    pub rec: &'static str,
    /// An experimental recognizer it can switch to (`ocr::experimental_rec`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<&'static str>,
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

/// PP-OCR with the models `kind` reads with of it, and where they run (`Settings::paddle_use`):
/// its detector, its recognizer or both. Two are kept, so the pointer's engine and the whole
/// screen's can differ without one reloading the other's.
fn ensure_paddle(state: &AppState, settings: &Settings, kind: OcrEngineKind) -> Result<Arc<PaddleOcr>> {
    const KEPT: usize = 2;
    let parts = settings.paddle_use(kind).ok_or_else(|| anyhow!("{kind:?} reads with no PP-OCR model"))?;
    if (parts.det && parts.plan.det == MANGA_DET) || parts.rec == Some(MANGA_REC) {
        require(state, Downloaded::Manga, "do PP-OCRv6 Manga")?;
    }
    ensure_ort(state)?;
    let mut cache = state.paddle.lock();
    if let Some(i) = cache.iter().position(|(p, _)| *p == parts) {
        let entry = cache.remove(i);
        let engine = entry.1.clone();
        cache.insert(0, entry);
        return Ok(engine);
    }
    let models = state.resource_dir.join("models");
    // Drop the oldest cached models before allocating new ones.
    cache.truncate(KEPT - 1);
    // A detector or recognizer the kept one has is shared, not loaded again.
    let loaded: Vec<Arc<PaddleOcr>> = cache.iter().map(|(_, e)| e.clone()).collect();
    let manga = models::MANGA.dir().ok();
    match PaddleOcr::load_sharing(&models, manga.as_deref(), parts, &loaded) {
        Ok(engine) => {
            let engine = Arc::new(engine);
            engine.warm_up();
            cache.insert(0, (parts, engine.clone()));
            *state.engine_error.write() = None;
            Ok(engine)
        }
        Err(e) => {
            *state.engine_error.write() = Some(format!("{e:#}"));
            Err(e)
        }
    }
}

fn bing_engine(state: &AppState) -> Result<Arc<BingOcr>> {
    let mut slot = state.bing.lock();
    if let Some(b) = slot.as_ref() {
        return Ok(b.clone());
    }
    let b = Arc::new(BingOcr::new()?);
    *slot = Some(b.clone());
    Ok(b)
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

fn content_dir(state: &AppState) -> PathBuf {
    state.dictionaries_dir.join("content")
}

/// Finishes what an earlier session left: an install that was interrupted, or a removed
/// dictionary whose files were still open. Only folders marked as such are deleted, so a damaged
/// settings file can never take installed dictionaries with it.
fn remove_unused_copies(state: &AppState) {
    let content = content_dir(state);
    let Ok(entries) = std::fs::read_dir(&content) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let unfinished = entry.file_name().to_string_lossy().starts_with(".unpacking-");
        if (unfinished || path.join(dictionaries::source::REMOVED_MARKER).is_file()) && dictionaries::source::is_installed_copy(&path, &content) {
            log::info!("removing unused dictionary copy {}", path.display());
            if let Err(e) = std::fs::remove_dir_all(&path) { log::warn!("remove {}: {e}", path.display()); }
        }
    }
}

fn load_dictionaries(state: &AppState) {
    let _loading = state.dictionary_gate.lock();
    let configs: Vec<DictionaryConfig> = state.settings.read().dictionaries.clone();
    let mut loaded: DictList = Vec::new();
    let mut errors = HashMap::new();
    for c in configs.iter().filter(|c| c.enabled && c.kind == DictionaryKind::Mdict) {
        if let Some(d) = state.dicts.read().iter().find(|d| d.id == c.id) {
            loaded.push(d.clone());
            continue;
        }
        match Dictionary::open_cached(&c.id, std::path::Path::new(&c.path), &state.dictionaries_dir, &|_| {}) {
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
    let content = content_dir(state);
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
                entries: d.map(|d| d.len()).unwrap_or(0),
                resource_files: d.map(|d| d.mdds.len()).unwrap_or(0),
                format: d.map(|d| d.format.clone()).unwrap_or_default(),
                description: d.map(|d| d.description.clone()).unwrap_or_default(),
                warnings: d.map(|d| d.warnings.clone()).unwrap_or_default(),
                installed: dictionaries::source::is_installed_copy(std::path::Path::new(&c.path), &content),
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
    let language = state.settings.read().ocr_language.clone();
    let mut forms = Vec::new();
    for surface in candidates {
        // The spelling on screen first, then Hunspell's dictionary forms, then the rule-based
        // guesses: a stem Hunspell knows is a real word, a guessed reduction may not be.
        let clean = surface.trim().trim_matches(|c: char| !c.is_alphanumeric());
        let stems = dictionaries::hunspell::stems(clean, &language);
        let spellings = dictionaries::morphology::candidates(surface, "und");
        let guesses = dictionaries::morphology::candidates(surface, &language);
        for form in spellings.into_iter().chain(stems).chain(guesses) {
            if !forms.contains(&form) { forms.push(form); }
            if forms.len() >= 256 { break; }
        }
        if forms.len() >= 256 { break; }
    }
    dict::lookup(&dicts, &forms)
}

/// Forms to look up for typed text: the text as typed first, then the dictionary forms of its
/// first word when the language has an analyzer (食べた → 食べる), then, for an unspaced script, its
/// prefixes, longest first; with those dictionary forms and the analyzer's name when it gave them.
fn manual_candidates(text: &str, analyzer: Option<&dyn segment::Segment>) -> (Vec<String>, Vec<String>, &'static str) {
    let text = text.trim();
    let mut out: Vec<String> = Vec::new();
    if !text.is_empty() { out.push(text.to_string()); }
    let mut lemmas: Vec<String> = Vec::new();
    let mut by = "";
    if let Some(seg) = analyzer.filter(|s| text.chars().next().is_some_and(|c| s.handles(c))) {
        if let Some(w) = seg.words(text).into_iter().find(|w| w.kind != segment::Kind::Punct).filter(|w| w.start == 0) {
            if !w.lemmas.is_empty() { by = seg.name(); }
            lemmas = w.lemmas.into_iter().filter(|l| l != text).collect();
            for form in &lemmas {
                if !out.contains(form) { out.push(form.clone()); }
            }
        }
    }
    let rest = if text.chars().next().map(ocr::is_unspaced_script).unwrap_or(false) {
        let chars: Vec<char> = text.chars().take(16).collect();
        (1..=chars.len()).rev().map(|n| chars[..n].iter().collect()).collect()
    } else {
        dict::candidate_forms(text)
    };
    for form in rest {
        if !out.contains(&form) { out.push(form); }
    }
    (out, lemmas, by)
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
            // A whole-screen lookup raises the (transparent) overlay before its OCR; take it down.
            if let Some(w) = app.get_webview_window("overlay") { let _ = w.hide(); }
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
    // Ahead of a game for the CPU until the lookup is read, as in Lookupper.
    let _priority = platform::priority::lookup();
    let state = app.state::<AppState>();
    #[cfg(target_os = "linux")]
    if platform::session_kind() == SessionKind::KdeWayland {
        platform::kwin::track_for(std::time::Duration::from_secs(3));
    }
    let scan = state.scan_seq.fetch_add(1, Ordering::SeqCst) + 1;
    // A new lookup replaces the previous overlay (and resumes a frozen app).
    let overlay_hide = close_overlay(app);
    let popup_visible = app.get_webview_window("popup").is_some_and(|p| p.is_visible().unwrap_or(false));
    let popup_hide = hide_blank(app, "popup");
    // Both must be off the screen before the capture.
    for hidden in [overlay_hide, popup_hide].into_iter().flatten() { let _ = hidden.join(); }
    // Give the compositor one frame to remove our popup from the next captured bitmap.
    if popup_visible { std::thread::sleep(std::time::Duration::from_millis(20)); }
    if !wait_for_segmenter(app) {
        return Ok(());
    }

    let settings = state.settings.read().clone();
    let fg = platform::foreground_app();
    let profile = fg.as_ref().map(|f| profile_for(app, f)).unwrap_or_default();
    let (cx, cy) = platform::cursor_position(app)?;
    let whole_screen = settings.ocr_scope == OcrScope::Screen;
    // The automatic capture reads the whole monitor but recognizes only the block under the pointer.
    let auto = !whole_screen && settings.capture_auto;
    let mut capture = if whole_screen || auto {
        platform::capture_screen(cx, cy)?
    } else {
        platform::capture_around(cx, cy, settings.capture_width, settings.capture_height)?
    };
    // Anki's {screenshot} can ask for the whole screen; grab it now, while the screen still shows
    // what the user looked up.
    let anki_whole_screen = settings.anki.get("enable").and_then(|v| v.as_bool()).unwrap_or(false)
        && settings.anki.get("screenshotArea").and_then(|v| v.as_str()) == Some("whole");
    let mut screen = if anki_whole_screen && !whole_screen && !auto {
        platform::capture_screen(cx, cy).map_err(|e| log::warn!("whole-screen capture for Anki failed: {e:#}")).ok()
    } else {
        None
    };
    // Resolve the output before OCR: the pointer may move to another monitor while it runs.
    let mut monitor = platform::display_geometry(app, cx, cy, &capture)?;
    // Each app chooses in its profile; "no freeze" (the default) leaves the screen live and only
    // highlights the words.
    let freeze = match profile.freeze {
        FreezeMode::Default => FreezeMode::None,
        mode => mode,
    };
    let frozen_pid = match (freeze, fg.as_ref()) {
        (FreezeMode::Process, Some(f)) if f.pid != 0 => {
            platform::suspend_process(f.pid);
            Some(f.pid)
        }
        _ => None,
    };
    let (mut px, mut py) = capture.to_image(cx, cy);

    let started = Instant::now();
    let reading = if whole_screen { Reading::Screen } else if auto { Reading::Block } else { Reading::Region };
    let mut pass = ocr_pass(&state, &settings, settings.engine_for(whole_screen), &capture.image, px, py, reading)?;
    let analyzer = segment::current(&settings.ocr_language);
    let mut pick = words::pick(&pass.lines, px, py, analyzer.as_deref().map(|a| a as &dyn segment::Segment)).filter(|p| p.is_under(px, py));
    // Like Lookupper: a shortcut pressed away from text reads the whole screen and shows the
    // clickable words without a pop-up; clicking one of them looks it up. Pressed on a block of
    // text but off its words, the automatic capture shows that block's words instead.
    if pick.is_none() && !whole_screen && pass.block.is_none() {
        if !auto {
            // Reuse the whole-screen capture taken for Anki, if any.
            capture = match screen.take() {
                Some(s) => s,
                None => platform::capture_screen(cx, cy)?,
            };
            monitor = platform::display_geometry(app, cx, cy, &capture)?;
            (px, py) = capture.to_image(cx, cy);
        }
        // From the moment of the capture, hold the clicks and the scroll wheel with the still
        // transparent overlay, so the screen cannot move away from the frame the OCR reads.
        show_overlay_window(app, monitor, true);
        pass = ocr_pass(&state, &settings, settings.engine_for(true), &capture.image, px, py, Reading::Screen)?;
        pick = words::pick(&pass.lines, px, py, analyzer.as_deref().map(|a| a as &dyn segment::Segment)).filter(|p| p.is_under(px, py));
        // A click or Esc on the transparent overlay cancelled this lookup while it was read.
        if state.scan_seq.load(Ordering::SeqCst) != scan {
            if let Some(pid) = frozen_pid { platform::resume_process(pid); }
            return Ok(());
        }
    }
    let OcrPass { lines, engine, kind: _, rest, paddle: paddle_engine, notice, key: ocr_key, block: text_block } = pass;
    let ocr_ms = started.elapsed().as_millis() as u64;

    let id = state.recognition_seq.fetch_add(1, Ordering::SeqCst) + 1;
    *state.recognition.lock() = Some(Recognition {
        id,
        capture: capture.clone(),
        lines: lines.clone(),
        focus_lines: match &pick {
            Some(p) => p.sentence_lines.iter().map(|&i| lines[i].clone()).collect(),
            None => lines.clone(),
        },
        monitor,
        scan,
        complete: rest.is_empty(),
        app: fg.clone(),
        frozen_pid,
        freeze_frame: freeze == FreezeMode::Frame,
        spotlight: pick.is_none() && !(lines.is_empty() && rest.is_empty()),
        screen,
        block: text_block.filter(|_| pick.is_none()),
        engine: engine.clone(),
        ocr_ms,
        notice: notice.clone(),
    });

    // Like Lookupper, the whole-screen overlay appears only once the whole screen is read: the
    // background pass below sends its words when it finishes. Until then the window is up but
    // transparent (raised right after the capture), holding the clicks and the scroll wheel so the
    // screen cannot move away from the captured frame; a click or Esc meanwhile cancels the lookup.
    let deferred = pick.is_none() && !rest.is_empty() && paddle_engine.is_some();
    match pick {
        None if lines.is_empty() && rest.is_empty() => {
            if let Some(w) = app.get_webview_window("overlay") { let _ = w.hide(); }
            let payload = LookupPayload {
                engine,
                ocr_ms,
                notice,
                error: Some("Nenhum texto encontrado na tela.".into()),
                app: fg,
                recognition: Some(id),
                ..Default::default()
            };
            show_popup(app, &payload, Some((cx, cy, cx, cy)));
        }
        // No pop-up for this lookup: the previous word's pending AI requests are dropped.
        None if deferred => popup_hidden(app),
        // Focus the overlay so Esc and clicks reach it.
        None => {
            popup_hidden(app);
            show_overlay(app, id, None, true, true);
        }
        Some(pick) => {
            let (bx0, by0, bx1, by1) = pick.bounds();
            let (x0, y0) = capture.to_desktop(bx0, by0);
            let (x1, y1) = capture.to_desktop(bx1, by1);
            let anchor = (x0, y0, x1, y1);
            show_overlay(app, id, Some(anchor), rest.is_empty(), profile.focus_overlay);
            let payload = LookupPayload {
                entries: lookup_words(&state, &pick.candidates),
                word: pick.word,
                sentence: pick.sentence,
                context: pick.context,
                context_at: Some(pick.context_at),
                line_break: pick.line_break,
                analyzer: pick.analyzer,
                lemmas: pick.lemmas,
                candidates: pick.candidates,
                engine,
                ocr_ms,
                notice,
                error: None,
                manual: false,
                app: fg,
                recognition: Some(id),
            };
            show_popup(app, &payload, Some(anchor));
        }
    }

    // Recognize the rest of the screen in the background so every word becomes clickable.
    if let (Some(p), false) = (paddle_engine, rest.is_empty()) {
        let app = app.clone();
        let image = capture.image.clone();
        std::thread::spawn(move || {
            let current = |app: &AppHandle| {
                let state = app.state::<AppState>();
                state.scan_seq.load(Ordering::SeqCst) == scan
                    && matches!(state.recognition.lock().as_ref(), Some(r) if r.id == id)
            };
            // Lines of similar width share one recognizer run.
            for chunk in p.plan_batches(&rest) {
                // A new lookup supersedes this overlay; do not run another stale OCR batch.
                if !current(&app) {
                    return;
                }
                let more = cached_recognize(&app.state::<AppState>(), &p, &image, ocr_key, &chunk, true);
                let state = app.state::<AppState>();
                let mut all = {
                    let mut guard = state.recognition.lock();
                    match guard.as_mut() {
                        Some(r) if r.id == id => {
                            r.lines.extend(more);
                            r.lines.clone()
                        }
                        _ => return,
                    }
                };
                // The paragraphs read every line's look, measured again over all of them.
                layout::look(&image, &mut all);
                if let Some(r) = state.recognition.lock().as_mut().filter(|r| r.id == id && r.lines.len() == all.len()) {
                    for (l, a) in r.lines.iter_mut().zip(&all) {
                        l.look = a.look;
                    }
                }
                if !deferred { emit_overlay(&app, id, None, false); }
            }
            // Closing the overlay or a new scan bumps `scan_seq`, so a cancelled pass sends nothing.
            if current(&app) { emit_overlay(&app, id, None, true); }
            let _ = app.emit("engine-status", ());
        });
    }
    Ok(())
}

struct OcrPass {
    lines: Vec<OcrLine>,
    engine: String,
    /// The engine that was asked for; `engine` names its models.
    kind: OcrEngineKind,
    /// Boxes still to recognize in the background for the overlay (whole-screen passes only).
    rest: Vec<TextBox>,
    paddle: Option<BoxEngine>,
    notice: Option<String>,
    /// The cache key of the engine that read `lines`.
    key: FrameKey,
    /// The block of text under the pointer (`Reading::Block`), when the pointer is on one.
    block: Option<TextBox>,
}

/// What an OCR pass recognizes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reading {
    /// A region capture: the lines around the pointer.
    Region,
    /// The automatic capture: only the block of text under the pointer, out of the whole monitor.
    Block,
    /// A whole-screen capture: the lines around the pointer, then the rest in the background.
    Screen,
}

/// Recognizes the lines around the pointer first. In a whole-screen pass the other boxes are
/// returned in `rest` so the overlay can fill in while the user reads.
fn ocr_pass(state: &AppState, settings: &Settings, kind: OcrEngineKind, image: &image::RgbaImage, px: f32, py: f32, reading: Reading) -> Result<OcrPass> {
    // Each engine's results, and each choice of its models', have their own key.
    let context = format!("{:?}|{}|{}|{:?}|{:?}|{}", kind, settings.ocr_language, settings.rec_model(), settings.models(kind), settings.plan_for(kind),
        if kind == OcrEngineKind::Lens { settings.lens_api_key.as_str() } else { "" });
    let mut key = frame_key(image, &context);
    // Engines that read the whole image they get at once. When one fails, the lookup says why:
    // nothing reads in its place (no fallback's models held in memory).
    let page = match kind {
        OcrEngineKind::Lens => Some("Google Lens"),
        OcrEngineKind::ScreenAi => Some("Chrome ScreenAI"),
        OcrEngineKind::OneOcr => Some("OneOCR"),
        OcrEngineKind::Bing => Some("Bing"),
        OcrEngineKind::Paddle | OcrEngineKind::Meiki | OcrEngineKind::Indra | OcrEngineKind::Custom | OcrEngineKind::CustomScreen | OcrEngineKind::Manga => None,
    };
    if let Some(name) = page {
        let read_page = |img: &image::RgbaImage| -> Result<Vec<OcrLine>> {
            match kind {
                OcrEngineKind::Lens => lens_engine(state)?.recognize(img, &settings.lens_api_key, &settings.ocr_language),
                OcrEngineKind::ScreenAi => state.screenai.recognize(img),
                OcrEngineKind::OneOcr => state.oneocr.recognize(img),
                _ => bing_engine(state)?.recognize(img),
            }
        };
        // The automatic capture reads only the block of text under the pointer, as PP-OCR's pass
        // does: the detector chosen for the engine (PP-OCRv6's or IndraOCR's) finds the block, and
        // the engine gets that part of the monitor (not
        // all of it, which OneOCR would also shrink to 1152×768). Off any block, nothing is read
        // and the lookup goes on to the whole screen, as with PP-OCR.
        // Or its own: the whole capture is read, and every line of it kept (below), so the
        // overlay has the whole screen's words; the block under the pointer is only outlined.
        let own = settings.models(kind).det == Detector::Page;
        let region = if reading == Reading::Block && !own {
            pointer_block(state, settings, kind, image, frame_key(image, &format!("block|{context}")), px, py)?
        } else {
            None
        };
        if reading == Reading::Block && !own && region.is_none() {
            return Ok(OcrPass { lines: Vec::new(), engine: name.into(), kind, rest: Vec::new(), paddle: None, notice: None, key, block: None });
        }
        let read = match region {
            Some(b) => {
                let (x, y, crop) = block_crop(image, &b);
                // Where the part is, too: its lines are kept in the capture's pixels.
                key = frame_key(&crop, &format!("{context}|at {x},{y}"));
                cached_page(state, key, || {
                    let mut lines = read_page(&crop)?;
                    for l in &mut lines {
                        l.shift(x as f32, y as f32);
                    }
                    Ok(lines)
                })
            }
            None => cached_page(state, key, || read_page(image)),
        };
        let mut lines = read.with_context(|| format!("O {name} não leu"))?;
        // The lines the block is judged by: the part read, or those under the pointer.
        let mut in_block: Vec<usize> = (0..lines.len()).collect();
        let mut region = region;
        if reading == Reading::Block && own {
            let boxes: Vec<TextBox> = lines.iter().map(|l| TextBox { x0: l.x0, y0: l.y0, x1: l.x1, y1: l.y1, quad: None }).collect();
            in_block = block::under(&boxes, px, py);
            region = block::bounds(&in_block.iter().map(|&i| boxes[i]).collect::<Vec<_>>());
        }
        layout::look(image, &mut lines);
        // As PP-OCR's block: one read as at most one letter or digit is not trusted.
        let block = region.filter(|_| block::reads_as_text(in_block.iter().map(|&i| lines[i].text.as_str())));
        return Ok(OcrPass { lines, engine: name.into(), kind, rest: Vec::new(), paddle: None, notice: None, key, block });
    }
    let p = box_engine(state, settings, kind)?;
    let cached_boxes = state.ocr_cache.lock().boxes(key);
    let boxes = match cached_boxes {
        Some(boxes) => boxes,
        None => {
            let mut boxes = p.finder.detect_boxes(image)?;
            // PP-OCRv6 Manga's recognizer reads each column whole and without its furigana, as
            // Fushi gives it them (ocr/manga_layout.rs).
            if settings.models(kind).rec == Recognizer::Manga {
                boxes = ocr::manga_layout::tidy(boxes, ocr::language(&settings.ocr_language).tag == "ja-JP");
            }
            state.ocr_cache.lock().save_boxes(key, &boxes);
            boxes
        }
    };
    let (focus, rest, mut block) = if reading == Reading::Block {
        let focus: Vec<TextBox> = block::under(&boxes, px, py).into_iter().map(|i| boxes[i]).collect();
        let bounds = block::bounds(&focus);
        (focus, Vec::new(), bounds)
    } else {
        let (focus, others) = split_focus(&boxes, px, py);
        (focus, if reading == Reading::Screen { others } else { Vec::new() }, None)
    };
    // The lines around the pointer stay one run per line: the pop-up waits only for these. A
    // larger block shares recognizer runs between lines of similar width.
    let mut lines = cached_recognize(state, &p, image, key, &focus, reading == Reading::Block && focus.len() > 6);
    // Like Lookupper, a block read as at most one letter or digit is not trusted: every box on the
    // screen is read, and the lookup goes on as if the pointer were on no block.
    if reading == Reading::Block && !focus.is_empty() && !block::reads_as_text(lines.iter().map(|l| l.text.as_str())) {
        lines = cached_recognize(state, &p, image, key, &boxes, true);
        block = None;
    }
    layout::look(image, &mut lines);
    let engine = p.name();
    Ok(OcrPass { lines, engine, kind, rest, paddle: Some(p), notice: None, key, block })
}

/// The block of text under the pointer, found by `kind`'s detector (its boxes cached under `key`).
fn pointer_block(state: &AppState, settings: &Settings, kind: OcrEngineKind, image: &image::RgbaImage, key: FrameKey, px: f32, py: f32) -> Result<Option<TextBox>> {
    let cached = state.ocr_cache.lock().boxes(key);
    let boxes = match cached {
        Some(boxes) => boxes,
        None => {
            let boxes = finder(state, settings, kind)?.detect_boxes(image)?;
            state.ocr_cache.lock().save_boxes(key, &boxes);
            boxes
        }
    };
    let under: Vec<TextBox> = block::under(&boxes, px, py).into_iter().map(|i| boxes[i]).collect();
    Ok(block::bounds(&under))
}

/// The part of the capture a block takes, with a margin, and where that part starts.
fn block_crop(image: &image::RgbaImage, b: &TextBox) -> (u32, u32, image::RgbaImage) {
    const MARGIN: f32 = 16.0;
    let (w, h) = (image.width(), image.height());
    let x0 = ((b.x0 - MARGIN).max(0.0) as u32).min(w.saturating_sub(1));
    let y0 = ((b.y0 - MARGIN).max(0.0) as u32).min(h.saturating_sub(1));
    let x1 = ((b.x1 + MARGIN).ceil().max(0.0) as u32).clamp(x0 + 1, w.max(x0 + 1));
    let y1 = ((b.y1 + MARGIN).ceil().max(0.0) as u32).clamp(y0 + 1, h.max(y0 + 1));
    (x0, y0, image::imageops::crop_imm(image, x0, y0, x1 - x0, y1 - y0).to_image())
}

fn device_name(gpu: bool) -> &'static str {
    if gpu { "GPU" } else { "CPU" }
}

/// What finds a pass's lines: PP-OCR's detector (PP-OCRv6's or PP-OCRv6 Manga's), IndraOCR's
/// LineNet or comic-text-detector.
#[derive(Clone)]
enum Finder {
    Paddle(Arc<PaddleOcr>),
    LineNet(Arc<IndraDet>),
    Ctd(Arc<CtdDet>),
}

impl Finder {
    fn detect_boxes(&self, image: &image::RgbaImage) -> Result<Vec<TextBox>> {
        match self {
            Finder::Paddle(p) => p.detect_boxes(image),
            Finder::LineNet(d) => d.detect_boxes(image),
            Finder::Ctd(d) => d.detect_boxes(image),
        }
    }

    fn name(&self) -> String {
        match self {
            Finder::Paddle(p) => format!("PP-OCRv6 {} · {}", p.det_model(), device_name(p.det_on_gpu())),
            Finder::LineNet(d) => format!("LineNet · {}", device_name(d.on_gpu())),
            Finder::Ctd(d) => format!("comic-text-detector · {}", device_name(d.on_gpu())),
        }
    }
}

/// What reads a pass's boxes: PP-OCR's recognizer or another one.
#[derive(Clone)]
enum Reader {
    Paddle(Arc<PaddleOcr>),
    Meiki(Arc<MeikiRec>),
    Indra(Arc<IndraRec>),
}

impl Reader {
    fn name(&self) -> String {
        match self {
            Reader::Paddle(p) => format!("PP-OCRv6 {}", p.rec_model),
            Reader::Meiki(_) => "MeikiOCR".into(),
            Reader::Indra(_) => "IndraOCR".into(),
        }
    }

    fn on_gpu(&self) -> bool {
        match self {
            Reader::Paddle(p) => p.rec_on_gpu(),
            Reader::Meiki(m) => m.on_gpu(),
            Reader::Indra(i) => i.on_gpu(),
        }
    }
}

/// What reads a local engine's pass: a detector and a recognizer.
#[derive(Clone)]
struct BoxEngine {
    finder: Finder,
    reader: Reader,
}

impl BoxEngine {
    fn recognize(&self, image: &image::RgbaImage, boxes: &[TextBox], batched: bool) -> Vec<OcrLine> {
        match &self.reader {
            Reader::Paddle(p) if batched => p.recognize_boxes_batched(image, boxes),
            Reader::Paddle(p) => p.recognize_boxes(image, boxes),
            Reader::Meiki(m) => recognize_boxes_with(image, boxes, m.as_ref()),
            Reader::Indra(i) => recognize_boxes_with(image, boxes, i.as_ref()),
        }
    }

    /// The background pass's runs: PP-OCR's batches, or another recognizer's runs of 16 boxes.
    fn plan_batches(&self, boxes: &[TextBox]) -> Vec<Vec<TextBox>> {
        match &self.reader {
            Reader::Paddle(p) => p.plan_batches(boxes),
            _ => boxes.chunks(16).map(<[TextBox]>::to_vec).collect(),
        }
    }

    /// The engine as the pop-up and the overlay name it.
    fn name(&self) -> String {
        match (&self.reader, &self.finder) {
            (Reader::Paddle(r), Finder::Paddle(d)) if Arc::ptr_eq(r, d) => self.finder.name(),
            (r, f) => format!("{} · {} · detecção {}", r.name(), device_name(r.on_gpu()), f.name()),
        }
    }
}

/// The detector and the recognizer `kind` reads with (`Settings::models`), loaded. One that
/// cannot load fails the pass, saying which: nothing reads in its place.
fn box_engine(state: &AppState, settings: &Settings, kind: OcrEngineKind) -> Result<BoxEngine> {
    let gpu = settings.rec_gpu_for(kind);
    let reader = match settings.models(kind).rec {
        Recognizer::Meiki => Reader::Meiki(meiki_engine(state, settings, gpu).context("O MeikiOCR não leu")?),
        Recognizer::Indra => Reader::Indra(indra_engine(state, settings, gpu).context("O IndraOCR não leu")?),
        Recognizer::Paddle | Recognizer::Manga | Recognizer::Page => Reader::Paddle(ensure_paddle(state, settings, kind)?),
    };
    Ok(BoxEngine { finder: finder(state, settings, kind)?, reader })
}

/// Fails, saying why, while `which`'s files are not all downloaded.
fn require(state: &AppState, which: Downloaded, whose: &str) -> Result<()> {
    if which.installed() {
        return Ok(());
    }
    match state.downloads.lock().get(which.id()).cloned() {
        Some(download) => bail!("os modelos {whose} {download}"),
        None => bail!("os modelos {whose} não estão baixados (Configurações › OCR)"),
    }
}

/// comic-text-detector, loaded once its model is downloaded, on the GPU when `gpu`.
fn ctd_engine(state: &AppState, settings: &Settings, gpu: bool) -> Result<Arc<CtdDet>> {
    require(state, Downloaded::Ctd, "do comic-text-detector")?;
    ensure_ort(state)?;
    let mut slot = state.ctd.lock();
    if let Some((on, d)) = slot.as_ref() {
        if *on == gpu {
            return Ok(d.clone());
        }
    }
    // Drop the previous session before loading its replacement.
    *slot = None;
    let d = Arc::new(CtdDet::new(&models::CTD.dir()?, gpu, settings.gpu_choice())?);
    d.warm_up();
    *slot = Some((gpu, d.clone()));
    Ok(d)
}

/// What finds `kind`'s lines, or, for a page engine, the block under the pointer.
fn finder(state: &AppState, settings: &Settings, kind: OcrEngineKind) -> Result<Finder> {
    let m = settings.models(kind);
    match m.det {
        Detector::LineNet => Ok(Finder::LineNet(linenet_engine(state, settings, m.det_gpu).context("A detecção do IndraOCR (LineNet) não carregou")?)),
        Detector::Ctd => Ok(Finder::Ctd(ctd_engine(state, settings, m.det_gpu).context("O comic-text-detector não carregou")?)),
        Detector::PaddleTiny | Detector::PaddleSmall | Detector::MangaDet => Ok(Finder::Paddle(ensure_paddle(state, settings, kind)?)),
        Detector::Page => bail!("{kind:?} finds its lines by itself"),
    }
}

/// IndraOCR's line detector, loaded once its models are downloaded, on the GPU when `gpu`.
fn linenet_engine(state: &AppState, settings: &Settings, gpu: bool) -> Result<Arc<IndraDet>> {
    if !Downloaded::Indra.installed() {
        match state.downloads.lock().get(Downloaded::Indra.id()).cloned() {
            Some(state) => bail!("os modelos do IndraOCR {state}"),
            None => bail!("os modelos do IndraOCR não estão baixados (Configurações › OCR)"),
        }
    }
    ensure_ort(state)?;
    let mut slot = state.linenet.lock();
    if let Some((on, d)) = slot.as_ref() {
        if *on == gpu {
            return Ok(d.clone());
        }
    }
    // Drop the previous session before loading its replacement.
    *slot = None;
    let d = Arc::new(IndraDet::new(&models::INDRA.dir()?, gpu, settings.gpu_choice())?);
    d.warm_up();
    *slot = Some((gpu, d.clone()));
    Ok(d)
}

/// IndraOCR's recognizer for the OCR language's group, loaded once its models are downloaded (after
/// the user accepted their licence), on the GPU when `gpu` (`Settings::rec_gpu_for`).
fn indra_engine(state: &AppState, settings: &Settings, gpu: bool) -> Result<Arc<IndraRec>> {
    if !Downloaded::Indra.installed() {
        match state.downloads.lock().get(Downloaded::Indra.id()).cloned() {
            Some(state) => bail!("os modelos dele {state}"),
            None => bail!("os modelos dele não estão baixados (Configurações › OCR)"),
        }
    }
    let group = IndraGroup::of(&settings.ocr_language);
    ensure_ort(state)?;
    let mut slot = state.indra.lock();
    if let Some((g, on, r)) = slot.as_ref() {
        if *g == group && *on == gpu {
            return Ok(r.clone());
        }
    }
    // One group at a time: drop the previous sessions before loading their replacements.
    *slot = None;
    let r = Arc::new(IndraRec::new(&models::INDRA.dir()?, group, gpu, settings.gpu_choice())?);
    *slot = Some((group, gpu, r.clone()));
    Ok(r)
}

/// MeikiOCR's recognizer, loaded once its models are downloaded, on the GPU when `gpu`
/// (`Settings::rec_gpu_for`). An error (not Japanese, models missing or downloading) fails the
/// pass, saying why.
fn meiki_engine(state: &AppState, settings: &Settings, gpu: bool) -> Result<Arc<MeikiRec>> {
    if ocr::language(&settings.ocr_language).tag != "ja-JP" {
        bail!("ele lê só japonês");
    }
    if !Downloaded::Meiki.installed() {
        match state.downloads.lock().get(Downloaded::Meiki.id()).cloned() {
            Some(state) => bail!("os modelos dele {state}"),
            None => bail!("os modelos dele não estão baixados (Configurações › OCR)"),
        }
    }
    ensure_ort(state)?;
    let mut slot = state.meiki.lock();
    if let Some((on, m)) = slot.as_ref() {
        if *on == gpu {
            return Ok(m.clone());
        }
    }
    // Drop the previous sessions before loading their replacements.
    *slot = None;
    let m = Arc::new(MeikiRec::new(&models::MEIKI.dir()?, gpu, settings.gpu_choice())?);
    *slot = Some((gpu, m.clone()));
    Ok(m)
}

/// The engines whose files are not in the installers but downloaded when the user picks them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Downloaded {
    Meiki,
    ScreenAi,
    /// Copied from the Snipping Tool installed here, not downloaded.
    OneOcr,
    /// Downloaded only once the user accepts its licence (`install_ocr_models`).
    Indra,
    /// PP-OCRv6 Manga's models (Apache-2.0).
    Manga,
    /// comic-text-detector's model (GPL-3.0), chosen as a detector.
    Ctd,
}

/// A downloaded engine's files, for Settings › OCR.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelState {
    pub installed: bool,
    /// The download while it runs ("estão sendo baixados (40%)"), or why it failed.
    pub download: Option<String>,
}

const DOWNLOADING: &str = "estão sendo baixados";

impl Downloaded {
    const ALL: [Downloaded; 6] = [Downloaded::Meiki, Downloaded::ScreenAi, Downloaded::OneOcr, Downloaded::Indra, Downloaded::Manga, Downloaded::Ctd];

    /// Its name in Settings and in the `models` map: the engine's own (`OcrEngineKind`).
    fn id(self) -> &'static str {
        match self {
            Downloaded::Meiki => "meiki",
            Downloaded::ScreenAi => "screenai",
            Downloaded::OneOcr => "oneocr",
            Downloaded::Indra => "indra",
            Downloaded::Manga => "manga",
            Downloaded::Ctd => "ctd",
        }
    }

    fn of(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.id() == id)
    }

    fn supported(self) -> bool {
        match self {
            Downloaded::Meiki | Downloaded::Indra | Downloaded::Manga | Downloaded::Ctd => true,
            Downloaded::ScreenAi => ocr::screenai::supported(),
            Downloaded::OneOcr => ocr::oneocr::supported(),
        }
    }

    fn installed(self) -> bool {
        match self {
            Downloaded::Meiki => models::MEIKI.installed(),
            Downloaded::ScreenAi => ocr::screenai::installed(),
            Downloaded::OneOcr => ocr::oneocr::installed(),
            Downloaded::Indra => models::INDRA.installed(),
            Downloaded::Manga => models::MANGA.installed(),
            Downloaded::Ctd => models::CTD.installed(),
        }
    }

    fn bytes(self) -> u64 {
        match self {
            Downloaded::Meiki => models::MEIKI.bytes(),
            Downloaded::ScreenAi => ocr::screenai::bytes(),
            // About the Snipping Tool 11.2608's three files, as written.
            Downloaded::OneOcr => 114_000_000,
            Downloaded::Indra => models::INDRA.bytes(),
            Downloaded::Manga => models::MANGA.bytes(),
            Downloaded::Ctd => models::CTD.bytes(),
        }
    }

    fn ensure(self, progress: &dyn Fn(u64)) -> Result<()> {
        match self {
            Downloaded::Meiki => models::MEIKI.ensure(progress).map(|_| ()),
            Downloaded::ScreenAi => ocr::screenai::ensure(progress),
            Downloaded::OneOcr => ocr::oneocr::install(progress),
            Downloaded::Indra => models::INDRA.ensure(progress).map(|_| ()),
            Downloaded::Manga => models::MANGA.ensure(progress).map(|_| ()),
            Downloaded::Ctd => models::CTD.ensure(progress).map(|_| ()),
        }
    }

    /// Ends what holds its files open, then deletes them.
    fn remove(self, state: &AppState) -> Result<()> {
        match self {
            Downloaded::Meiki => {
                *state.meiki.lock() = None;
                models::MEIKI.remove()
            }
            Downloaded::ScreenAi => {
                state.screenai.stop();
                ocr::screenai::remove()
            }
            Downloaded::OneOcr => {
                state.oneocr.stop();
                ocr::oneocr::remove()
            }
            Downloaded::Indra => {
                *state.indra.lock() = None;
                *state.linenet.lock() = None;
                models::INDRA.remove()
            }
            Downloaded::Manga => {
                state.paddle.lock().retain(|(u, _)| !((u.det && u.plan.det == MANGA_DET) || u.rec == Some(MANGA_REC)));
                models::MANGA.remove()
            }
            Downloaded::Ctd => {
                *state.ctd.lock() = None;
                models::CTD.remove()
            }
        }
    }
}

/// Downloads an engine's files in the background, one download each at a time; Settings › OCR
/// follows it.
fn download_models(app: &AppHandle, which: Downloaded) {
    let state = app.state::<AppState>();
    {
        let mut downloads = state.downloads.lock();
        if !which.supported() || which.installed() || downloads.get(which.id()).is_some_and(|d| d.starts_with(DOWNLOADING)) {
            return;
        }
        downloads.insert(which.id(), format!("{DOWNLOADING} (0%)"));
    }
    let _ = app.emit("engine-status", ());
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        // Asked each time: ScreenAI's is known once CIPD has said which package is the latest.
        let progress = |done: u64| {
            let percent = (done * 100 / which.bytes().max(1)).min(100);
            state.downloads.lock().insert(which.id(), format!("{DOWNLOADING} ({percent}%)"));
            let _ = app.emit("engine-status", ());
        };
        let result = which.ensure(&progress);
        let mut downloads = state.downloads.lock();
        match result {
            Ok(()) => {
                downloads.remove(which.id());
            }
            Err(e) => {
                log::warn!("{} download failed: {e:#}", which.id());
                downloads.insert(which.id(), format!("não puderam ser baixados: {e:#}"));
            }
        }
        drop(downloads);
        let _ = app.emit("engine-status", ());
    });
}

/// Settings › OCR: downloads an engine's files (`Downloaded::id`). IndraOCR's need `accept_license`:
/// the user accepted its weights' licence in the dialog just before.
#[tauri::command]
fn install_ocr_models(app: AppHandle, engine: String, accept_license: Option<bool>) -> std::result::Result<(), String> {
    let which = Downloaded::of(&engine).ok_or_else(|| format!("unknown engine {engine}"))?;
    if which == Downloaded::Indra && accept_license != Some(true) {
        return Err("Os modelos do IndraOCR só são baixados depois que você aceita a licença deles.".into());
    }
    download_models(&app, which);
    Ok(())
}

/// Settings › OCR: deletes an engine's files; a pass asked for that engine then fails, saying
/// why, until they are back.
#[tauri::command]
fn remove_ocr_models(app: AppHandle, state: tauri::State<'_, AppState>, engine: String) -> std::result::Result<(), String> {
    let which = Downloaded::of(&engine).ok_or_else(|| format!("unknown engine {engine}"))?;
    let removed = which.remove(&state).map_err(|e| format!("{e:#}"));
    state.downloads.lock().remove(which.id());
    let _ = app.emit("engine-status", ());
    removed
}

fn cached_recognize(state: &AppState, engine: &BoxEngine, image: &image::RgbaImage, key: FrameKey, boxes: &[TextBox], batched: bool) -> Vec<OcrLine> {
    let cached = state.ocr_cache.lock().lines(key, boxes);
    if let Some(lines) = cached { return lines; }
    let lines = engine.recognize(image, boxes, batched);
    state.ocr_cache.lock().save_lines(key, boxes, &lines);
    lines
}

/// The lines of an engine that reads the whole image at once (`read`), cached under `key` with a
/// box per line.
fn cached_page(state: &AppState, key: FrameKey, read: impl FnOnce() -> Result<Vec<OcrLine>>) -> Result<Vec<OcrLine>> {
    let cached = {
        let mut cache = state.ocr_cache.lock();
        cache.boxes(key).and_then(|boxes| cache.lines(key, &boxes))
    };
    if let Some(lines) = cached { return Ok(lines); }
    let lines = read()?;
    let boxes: Vec<TextBox> = lines.iter().map(|line| TextBox { x0: line.x0, y0: line.y0, x1: line.x1, y1: line.y1, quad: None }).collect();
    let mut cache = state.ocr_cache.lock();
    cache.save_boxes(key, &boxes);
    cache.save_lines(key, &boxes, &lines);
    Ok(lines)
}

fn overlay_viewport(app: &AppHandle, monitor: DisplayGeometry) -> ((f64, f64), f64, (f64, f64)) {
    let win = app.get_webview_window("overlay");
    let origin = win.as_ref().and_then(|w| w.inner_position().ok())
        .map(|p| (p.x as f64, p.y as f64)).unwrap_or((monitor.native.x, monitor.native.y));
    let scale = win.as_ref().and_then(|w| w.scale_factor().ok()).unwrap_or(monitor.scale).max(0.5);
    let size = win.and_then(|w| w.inner_size().ok())
        .map(|s| (s.width as f64 / scale, s.height as f64 / scale))
        .unwrap_or((monitor.native.width / scale, monitor.native.height / scale));
    (origin, scale, size)
}

fn emit_overlay(app: &AppHandle, id: u64, selected: Option<(f64, f64, f64, f64)>, complete: bool) {
    let state = app.state::<AppState>();
    let Some(monitor) = state.recognition.lock().as_ref().filter(|r| r.id == id).map(|r| r.monitor) else { return };
    // Query the window outside the recognition lock: its dispatcher may need the event thread.
    let (origin, scale, (viewport_width, viewport_height)) = overlay_viewport(app, monitor);
    let analyzer = segment::current(&state.settings.read().ocr_language);
    let payload = {
        let mut guard = state.recognition.lock();
        let Some(r) = guard.as_mut().filter(|r| r.id == id) else { return };
        r.complete = complete;
        let css_rect = |x0, y0, x1, y1| r.monitor.css_rect((x0, y0, x1, y1), origin, scale);
        let words = words::word_boxes(&r.lines, analyzer.as_deref().map(|a| a as &dyn segment::Segment))
            .into_iter()
            .map(|w| {
                let (x0, y0) = r.capture.to_desktop(w.x0, w.y0);
                let (x1, y1) = r.capture.to_desktop(w.x1, w.y1);
                let (x, y, w2, h) = css_rect(x0, y0, x1, y1);
                let (hx0, hy0) = r.capture.to_desktop(w.hit.0, w.hit.1);
                let (hx1, hy1) = r.capture.to_desktop(w.hit.2, w.hit.3);
                let (hx, hy, hw, hh) = css_rect(hx0, hy0, hx1, hy1);
                OverlayWord { text: w.text, x, y, w: w2, h, hit: Some([hx, hy, hw, hh]), angle: (w.angle != 0.0).then_some(w.angle as f64), group: w.group }
            })
            .collect();
        let selected = selected.map(|(x0, y0, x1, y1)| {
            let (x, y, w, h) = css_rect(x0, y0, x1, y1);
            OverlayWord { text: String::new(), x, y, w, h, hit: None, angle: None, group: None }
        });
        // Only a profile set to freeze shows the captured frame; otherwise the screen stays live
        // under the word highlights.
        let with_frame = r.freeze_frame;
        let frame_rect = with_frame.then(|| {
            let c = &r.capture;
            let (x, y, w, h) = css_rect(c.origin_x, c.origin_y,
                c.origin_x + c.image.width() as f64 / c.scale, c.origin_y + c.image.height() as f64 / c.scale);
            OverlayWord { text: String::new(), x, y, w, h, hit: None, angle: None, group: None }
        });
        OverlayPayload {
            id,
            words,
            selected,
            frame_url: with_frame.then(|| format!("/c/{id}/frame.png")),
            frame_rect,
            spotlight: r.spotlight,
            block: r.block.map(|b| {
                let (x0, y0) = r.capture.to_desktop(b.x0, b.y0);
                let (x1, y1) = r.capture.to_desktop(b.x1, b.y1);
                let (x, y, w, h) = css_rect(x0, y0, x1, y1);
                OverlayWord { text: String::new(), x, y, w, h, hit: None, angle: None, group: None }
            }),
            viewport_width,
            viewport_height,
            complete,
            notice: r.notice.clone(),
        }
    };
    let _ = app.emit_to("overlay", "overlay", &payload);
}

fn show_overlay(app: &AppHandle, id: u64, anchor: Option<(f64, f64, f64, f64)>, complete: bool, focus: bool) {
    let Some(monitor) = app.state::<AppState>().recognition.lock().as_ref().map(|r| r.monitor) else { return };
    show_overlay_window(app, monitor, false);
    emit_overlay(app, id, anchor, complete);
    if focus {
        if let Some(win) = app.get_webview_window("overlay") { let _ = win.set_focus(); }
    }
}

/// Shows the overlay window over a monitor. Its content stays transparent until an `overlay` event
/// for a new recognition arrives (see `close_overlay`).
fn show_overlay_window(app: &AppHandle, monitor: DisplayGeometry, focus: bool) {
    let Some(win) = app.get_webview_window("overlay") else { return };
    let state = app.state::<AppState>();
    let mut order = state.blank.order.lock();
    order[window_slot("overlay")] += 1;
    let kde = platform::session_kind() == SessionKind::KdeWayland;
    let m = monitor.native;
    // On KDE, leaving and re-entering fullscreen at every lookup makes KWin animate the overlay;
    // on the same monitor it is already fullscreen there.
    let same_output = kde && state.blank.overlay_output.lock().replace(m) == Some(m);
    if !same_output {
        if kde { let _ = win.set_fullscreen(false); }
        let _ = win.set_position(PhysicalPosition::new(m.x.round() as i32, m.y.round() as i32));
        let _ = win.set_size(PhysicalSize::new(m.width.round() as u32, m.height.round() as u32));
    }
    let _ = win.show();
    // A normal KDE window is constrained by the panel work area (e.g. y=28). Fullscreen
    // covers the actual output; resize/move events update the borders after configuration.
    if kde { let _ = win.set_fullscreen(true); }
    drop(order);
    if focus {
        let _ = win.set_focus();
    }
}

/// Hides the overlay and clears its content, so the next lookup does not flash the old one.
/// Returns the pending hide on Linux (see `hide_blank`).
fn close_overlay(app: &AppHandle) -> Option<std::thread::JoinHandle<()>> {
    let state = app.state::<AppState>();
    let frozen = state.recognition.lock().as_mut().and_then(|r| r.frozen_pid.take());
    if let Some(pid) = frozen {
        platform::resume_process(pid);
    }
    hide_blank(app, "overlay")
}

fn window_slot(label: &str) -> usize {
    if label == "overlay" { 0 } else { 1 }
}

/// How long a window may take to paint its empty state before it is hidden anyway.
#[cfg(target_os = "linux")]
const BLANK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(150);

/// Hides the overlay or the pop-up.
///
/// On Linux, WebKitGTK stops painting a hidden webview and, when the window is shown again, the
/// compositor first shows the last frame it painted: the previous lookup's dimmed or frozen
/// screen, or the previous pop-up at full size. That frame is the flash the user sees at every
/// lookup. So the window is first told to show nothing (`overlay-closed` / `popup-blank` with a
/// token), and hidden once it confirms it painted that (`webview_blank`), or after
/// `BLANK_TIMEOUT`. The hide runs on its own thread, since the main thread must keep running for
/// the window to paint; the caller can join it. Windows hides at once, as before.
fn hide_blank(app: &AppHandle, label: &'static str) -> Option<std::thread::JoinHandle<()>> {
    let win = app.get_webview_window(label)?;
    let event = if label == "overlay" { "overlay-closed" } else { "popup-blank" };
    #[cfg(target_os = "linux")]
    if win.is_visible().unwrap_or(false) {
        let state = app.state::<AppState>();
        let slot = window_slot(label);
        let shown = state.blank.order.lock()[slot];
        let token = state.blank.token.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = app.emit_to(label, event, serde_json::json!({ "token": token }));
        let app = app.clone();
        return Some(std::thread::spawn(move || {
            let state = app.state::<AppState>();
            let painted = {
                let deadline = Instant::now() + BLANK_TIMEOUT;
                let mut painted = state.blank.painted.lock();
                while painted[slot] < token {
                    if state.blank.changed.wait_until(&mut painted, deadline).timed_out() { break; }
                }
                painted[slot] >= token
            };
            // The page has drawn its empty frame; let GTK and the compositor put it on screen.
            if painted { std::thread::sleep(std::time::Duration::from_millis(20)); }
            let order = state.blank.order.lock();
            // Shown again for a newer lookup meanwhile: keep it.
            if order[slot] == shown {
                if let Some(w) = app.get_webview_window(label) { let _ = w.hide(); }
            }
            drop(order);
        }));
    }
    let _ = win.hide();
    if label == "overlay" { let _ = app.emit_to(label, event, ()); }
    None
}

/// Tells the pop-up it was closed: like Yomitan, a closed pop-up cancels its pending AI requests
/// (when "Cancelar pedidos pendentes" is on). A new word does it through its own render instead.
fn popup_hidden(app: &AppHandle) {
    let _ = app.emit_to("popup", "popup-hidden", ());
}

/// Hides the pop-up and cancels its pending AI requests.
fn dismiss_popup(app: &AppHandle) {
    hide_blank(app, "popup");
    popup_hidden(app);
}

/// Where the pop-up goes for a word, and the point of it facing the word (where the open
/// animation grows from, relative to the pop-up). All in physical pixels.
#[derive(Debug, PartialEq)]
struct PopupPlace {
    x: f64,
    y: f64,
    origin: (f64, f64),
}

/// Lookupper's lookup popup is a WinUI TeachingTip aimed at the word, with no placement of its
/// own: this is WinUI's targeted placement (`DetermineEffectivePlacementTargeted` and
/// `PositionTargetedPopup` in microsoft-ui-xaml, controls/dev/TeachingTip/TeachingTip.cpp), read
/// on 2026-10-21. The tip is the card inside a grid with an 8 px strip on every side, where the
/// 20×10 tail is drawn on the side facing the word; Lookitan has no tail, so the card keeps the
/// same 8 px gap. WinUI tries the modes in order and takes the first whose fit test passes:
/// centred above or below only when the whole card fits centred on the word, then centred on
/// either side, then the shifted variants, where the word's centre is 28 px from the grid's edge.
/// Its bounds are the overlay window, the whole monitor.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Placement { Top, Bottom, Left, Right, TopLeft, TopRight, BottomLeft, BottomRight, LeftTop, LeftBottom, RightTop, RightBottom }

/// WinUI's order for `PreferredPlacement = Auto` starts with Top; with a Bottom preference it
/// swaps Top and Bottom (and their variants). Lookitan keeps the word visible below it first,
/// as before, until it is known which one Lookupper sets.
const PLACEMENT_ORDER: [Placement; 12] = {
    use Placement::*;
    [Bottom, Top, Left, Right, BottomLeft, BottomRight, TopLeft, TopRight, LeftTop, LeftBottom, RightTop, RightBottom]
};

fn popup_position(word: (f64, f64, f64, f64), size: (f64, f64), monitor: Option<platform::geometry::Rect>, scale: f64) -> PopupPlace {
    use Placement::*;
    let (x0, y0, x1, y1) = word;
    let (w, h) = size;
    // WinUI's sizes (TeachingTip_themeresources.xaml), in logical pixels.
    let strip = 8.0 * scale; // TeachingTipTailShortSideLength: the tail's depth, the gap to the word
    let to_tail = 28.0 * scale; // MinimumTipEdgeToTailCenter: 8 + TeachingTipTailMargin 10 + half the tail
    let (tw, th) = (x1 - x0, y1 - y0);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    // The grid around the card, as WinUI measures it, and the tip with its tail.
    let (gw, gh) = (w + 2.0 * strip, h + 2.0 * strip);
    let (tip_w, tip_h) = (gw + strip, gh + strip);
    let grid_at = |mode: Placement| -> (f64, f64) {
        match mode {
            Top => (cx - gw / 2.0, y0 - gh),
            Bottom => (cx - gw / 2.0, y1),
            Left => (x0 - gw, cy - gh / 2.0),
            Right => (x1, cy - gh / 2.0),
            TopLeft => (cx - gw + to_tail, y0 - gh),
            TopRight => (cx - to_tail, y0 - gh),
            BottomLeft => (cx - gw + to_tail, y1),
            BottomRight => (cx - to_tail, y1),
            LeftTop => (x0 - gw, cy - gh + to_tail),
            LeftBottom => (x0 - gw, cy - to_tail),
            RightTop => (x1, cy - gh + to_tail),
            RightBottom => (x1, cy - to_tail),
        }
    };
    let place = |mode: Placement| -> PopupPlace {
        let (gx, gy) = grid_at(mode);
        let (x, y) = (gx + strip, gy + strip);
        let origin = match mode {
            Top | TopLeft | TopRight => ((cx - x).clamp(0.0, w), h),
            Bottom | BottomLeft | BottomRight => ((cx - x).clamp(0.0, w), 0.0),
            Left | LeftTop | LeftBottom => (w, (cy - y).clamp(0.0, h)),
            Right | RightTop | RightBottom => (0.0, (cy - y).clamp(0.0, h)),
        };
        PopupPlace { x, y, origin }
    };
    let Some(m) = monitor else { return place(Bottom) };
    // Room between the word and each edge of the monitor.
    let (above, below) = (y0 - m.y, m.y + m.height - y1);
    let (left, right) = (x0 - m.x, m.x + m.width - x1);
    let fits = |mode: Placement| -> bool {
        let vertical = match mode {
            Top | TopLeft | TopRight => tip_h <= above,
            Bottom | BottomLeft | BottomRight => tip_h <= below,
            Left | Right => gh / 2.0 <= above + th / 2.0 && gh / 2.0 <= below + th / 2.0,
            LeftTop | RightTop => gh - to_tail <= above + th / 2.0,
            LeftBottom | RightBottom => gh - to_tail <= below + th / 2.0,
        };
        let horizontal = match mode {
            Left | LeftTop | LeftBottom => tip_w <= left,
            Right | RightTop | RightBottom => tip_w <= right,
            Top | Bottom => gw / 2.0 <= left + tw / 2.0 && gw / 2.0 <= right + tw / 2.0,
            TopLeft | BottomLeft => gw - to_tail <= left + tw / 2.0,
            TopRight | BottomRight => gw - to_tail <= right + tw / 2.0,
        };
        vertical && horizontal
    };
    if let Some(&mode) = PLACEMENT_ORDER.iter().find(|&&mode| fits(mode)) {
        return place(mode);
    }
    // WinUI would not open the tip at all. Lookitan still shows it: centred below the word, or
    // above it with more room there, kept on the monitor.
    let mut p = place(if below >= above { Bottom } else { Top });
    p.x = p.x.clamp(m.x, (m.x + m.width - w).max(m.x));
    p.y = p.y.clamp(m.y, (m.y + m.height - h).max(m.y));
    p.origin = ((cx - p.x).clamp(0.0, w), if p.y >= y1 { 0.0 } else { h });
    p
}

/// Sends the result to the popup and shows it next to the word (desktop coordinates).
fn show_popup(app: &AppHandle, payload: &LookupPayload, anchor: Option<(f64, f64, f64, f64)>) {
    let Some(win) = app.get_webview_window("popup") else { return };
    let _ = app.emit_to("popup", "lookup", payload);
    let display = app.state::<AppState>().recognition.lock().as_ref()
        .filter(|r| payload.recognition == Some(r.id)).map(|r| r.monitor);
    open_popup(app, &win, anchor, display);
}

/// Shows the pop-up next to `anchor` (desktop coordinates), on `display`.
fn open_popup(app: &AppHandle, win: &tauri::WebviewWindow, anchor: Option<(f64, f64, f64, f64)>, display: Option<DisplayGeometry>) {
    // Where the open animation grows from, in CSS pixels of the popup: the edge facing the word,
    // like the tail of Lookupper's TeachingTip. Centre of the top edge without a word.
    let mut origin = (0.5_f64, 0.0_f64, true);
    if let Some((x0, y0, x1, y1)) = anchor {
        let ((x0, y0), (x1, y1)) = display.map(|d| (d.to_native(x0, y0), d.to_native(x1, y1)))
            .unwrap_or(((x0, y0), (x1, y1)));
        let size = win.outer_size().unwrap_or(PhysicalSize::new(560, 520));
        let (w, h) = (size.width as f64, size.height as f64);
        let bounds = display.map(|d| d.native).or_else(|| app.monitor_from_point(x0, y0).ok().flatten()
            .map(|m| platform::geometry::Rect { x: m.position().x as f64, y: m.position().y as f64,
                width: m.size().width as f64, height: m.size().height as f64 }));
        let scale = win.scale_factor().unwrap_or(1.0).max(0.5);
        let place = popup_position((x0, y0, x1, y1), (w, h), bounds, scale);
        let _ = win.set_position(PhysicalPosition::new(place.x.round() as i32, place.y.round() as i32));
        origin = (place.origin.0 / scale, place.origin.1 / scale, false);
    }
    let (ox, oy, relative) = origin;
    let _ = app.emit_to("popup", "popup-open", serde_json::json!({ "x": ox, "y": oy, "relative": relative }));
    let state = app.state::<AppState>();
    let mut order = state.blank.order.lock();
    order[window_slot("popup")] += 1;
    let _ = win.show();
    drop(order);
    let _ = win.set_focus();
}

/// PNG of the last capture: "sentence" (the lines around the word with some margin), "frame" (the
/// image OCR read) or "whole" (the whole monitor when it was captured for Anki, else the frame).
/// The lines around the picked word with a margin, as `[x, y, width, height]` of the image OCR
/// read: the "sentence" screenshot, and a saved word's crop until the user changes it.
fn sentence_rect(r: &Recognition) -> Option<[u32; 4]> {
    if r.focus_lines.is_empty() {
        return None;
    }
    let img = &r.capture.image;
    let pad = 24.0;
    let x0 = r.focus_lines.iter().map(|l| l.x0).fold(f32::MAX, f32::min) - pad;
    let y0 = r.focus_lines.iter().map(|l| l.y0).fold(f32::MAX, f32::min) - pad;
    let x1 = r.focus_lines.iter().map(|l| l.x1).fold(0.0, f32::max) + pad;
    let y1 = r.focus_lines.iter().map(|l| l.y1).fold(0.0, f32::max) + pad;
    let x = (x0.max(0.0) as u32).min(img.width().saturating_sub(1));
    let y = (y0.max(0.0) as u32).min(img.height().saturating_sub(1));
    let w = (x1.min(img.width() as f32) as u32).saturating_sub(x).max(1);
    let h = (y1.min(img.height() as f32) as u32).saturating_sub(y).max(1);
    Some([x, y, w, h])
}

/// A saved word's picture: the whole image OCR read, and the part shown (the sentence's lines).
/// Lookupper keeps the frame and a crop rectangle the same way, and lets the crop be changed.
#[derive(Serialize)]
struct CardImage {
    data: String,
    crop: Option<[u32; 4]>,
}

#[tauri::command]
fn capture_card_image(state: tauri::State<'_, AppState>, recognition: Option<u64>) -> Option<CardImage> {
    // The frame is copied out so the overlay is not kept waiting while it is encoded.
    let (img, crop) = {
        let guard = state.recognition.lock();
        let r = guard.as_ref().filter(|r| recognition.map(|i| i == r.id).unwrap_or(true))?;
        (r.capture.image.clone(), sentence_rect(r))
    };
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(CardImage { data: base64::engine::general_purpose::STANDARD.encode(png), crop })
}

pub fn capture_png(state: &AppState, id: Option<u64>, area: &str) -> Option<Vec<u8>> {
    let guard = state.recognition.lock();
    let r = guard.as_ref().filter(|r| id.map(|i| i == r.id).unwrap_or(true))?;
    // "whole" means the whole monitor, even when OCR only read the region around the pointer;
    // "frame" is always the image OCR read (the overlay's frozen frame).
    let img = match (area, &r.screen) {
        ("whole", Some(screen)) => &screen.image,
        _ => &r.capture.image,
    };
    let cropped = match sentence_rect(r) {
        Some([x, y, w, h]) if area == "sentence" => image::imageops::crop_imm(img, x, y, w, h).to_image(),
        _ => img.clone(),
    };
    let mut png = Vec::new();
    if area == "frame" {
        // The overlay waits for this frame; favour encoding speed over file size.
        use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
        use image::ImageEncoder;
        PngEncoder::new_with_quality(&mut png, CompressionType::Fast, PngFilter::NoFilter)
            .write_image(cropped.as_raw(), cropped.width(), cropped.height(), image::ExtendedColorType::Rgba8)
            .ok()?;
    } else {
        image::DynamicImage::ImageRgba8(cropped).write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    }
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

/// The window the controller navigates: the pop-up while it is up, else the overlay's words.
fn gamepad_target(app: &AppHandle) -> Option<&'static str> {
    let visible = |label: &str| app.get_webview_window(label).is_some_and(|w| w.is_visible().unwrap_or(false));
    if visible("popup") { Some("popup") } else if visible("overlay") { Some("overlay") } else { None }
}

fn on_gamepad(app: &AppHandle, signal: gamepad::Signal) {
    use gamepad::{Action, Signal};
    let close = matches!(signal, Signal::Act(Action::Close));
    let message = match signal {
        Signal::Hotkey => return scan(app),
        Signal::Capture { buttons, done } => {
            let _ = app.emit("gamepad-capture", serde_json::json!({"buttons": buttons, "done": done}));
            return;
        }
        Signal::Move(dir) => serde_json::json!({"action": "move", "dir": dir}),
        Signal::Scroll(dy) => serde_json::json!({"action": "scroll", "dy": dy}),
        Signal::Act(action) => serde_json::json!({"action": action}),
    };
    match gamepad_target(app) {
        // Close takes the pop-up away first and leaves the overlay's words to move between,
        // as Lookupper's DismissTopmost does; on the overlay it closes the lookup.
        Some("overlay") if close => overlay_close(app.clone()),
        Some(label) => { let _ = app.emit_to(label, "gamepad", message); }
        None => {}
    }
}

#[tauri::command]
fn gamepad_capture(on: bool) {
    gamepad::capture(on);
}

/// The connected controller's name, for the settings.
#[tauri::command]
fn gamepad_status() -> Option<String> {
    gamepad::connected()
}

/// The pop-up closed from the controller: the overlay stays, so another word can be chosen.
#[tauri::command]
fn popup_dismiss(app: AppHandle) {
    dismiss_popup(&app);
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

/// Drops the models the settings no longer read with (a PP-OCR plan or recognizer that changed,
/// MeikiOCR or IndraOCR no longer chosen), so they do not hold memory until something replaces
/// them. A pass still using one keeps it until it ends.
fn free_unused_engines(state: &AppState, settings: &Settings) {
    let wanted = settings.paddle_configs();
    state.paddle.lock().retain(|(parts, _)| wanted.contains(parts));
    if wanted.is_empty() {
        // A PP-OCR load that failed no longer matters.
        *state.engine_error.write() = None;
    }
    if !settings.detects_with(Detector::LineNet) {
        *state.linenet.lock() = None;
    }
    if !settings.detects_with(Detector::Ctd) {
        *state.ctd.lock() = None;
    }
    let japanese = ocr::language(&settings.ocr_language).tag == "ja-JP";
    if !(settings.reads_with(Recognizer::Meiki) && japanese) {
        *state.meiki.lock() = None;
    }
    if !settings.reads_with(Recognizer::Indra) {
        *state.indra.lock() = None;
    }
    // A page engine no selector reads with ends its helper now, not after `IDLE`.
    let selected = [settings.engine_for(false), settings.engine_for(true)];
    if !selected.contains(&OcrEngineKind::ScreenAi) {
        state.screenai.stop();
    }
    if !selected.contains(&OcrEngineKind::OneOcr) {
        state.oneocr.stop();
    }
    if !selected.contains(&OcrEngineKind::Lens) {
        *state.lens.lock() = None;
    }
    if !selected.contains(&OcrEngineKind::Bing) {
        *state.bing.lock() = None;
    }
}

fn warm_engine_in_background(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let settings = state.settings.read().clone();
        free_unused_engines(&state, &settings);
        let indra = Downloaded::Indra.installed();
        // IndraOCR downloaded by a version without its detector gets it now: its licence was
        // accepted then.
        if (settings.reads_with(Recognizer::Indra) || settings.detects_with(Detector::LineNet)) && !indra && models::INDRA.partly_installed() {
            download_models(&app, Downloaded::Indra);
        }
        // The models of each engine in use, the pointer's last, so its PP-OCR is the one kept first.
        for kind in settings.engines_in_use().into_iter().rev() {
            let m = settings.models(kind);
            // PP-OCRv6 Manga's and comic-text-detector's files are fetched when chosen; they load
            // with the first lookup after.
            if (m.det == Detector::MangaDet || m.rec == Recognizer::Manga) && !Downloaded::Manga.installed() {
                download_models(&app, Downloaded::Manga);
            }
            if m.det == Detector::Ctd {
                if !Downloaded::Ctd.installed() {
                    download_models(&app, Downloaded::Ctd);
                } else if let Err(e) = ctd_engine(&state, &settings, m.det_gpu) {
                    log::warn!("comic-text-detector unavailable: {e:#}");
                }
            }
            if settings.paddle_use(kind).is_some() {
                if let Err(e) = ensure_paddle(&state, &settings, kind) {
                    log::warn!("OCR engine unavailable: {e:#}");
                }
            }
            if m.det == Detector::LineNet && indra {
                if let Err(e) = linenet_engine(&state, &settings, m.det_gpu) {
                    log::warn!("IndraOCR's detector unavailable: {e:#}");
                }
            }
            if m.rec == Recognizer::Indra && indra {
                if let Err(e) = indra_engine(&state, &settings, settings.rec_gpu_for(kind)) {
                    log::warn!("IndraOCR unavailable: {e:#}");
                }
            }
        }
        if settings.wants_meiki() && ocr::language(&settings.ocr_language).tag == "ja-JP" {
            if Downloaded::Meiki.installed() {
                let reader = [settings.engine_for(false), settings.engine_for(true)].into_iter().find(|&k| settings.recognizer_for(k) == Recognizer::Meiki);
                let gpu = reader.is_some_and(|k| settings.rec_gpu_for(k));
                if let Err(e) = meiki_engine(&state, &settings, gpu) {
                    log::warn!("MeikiOCR unavailable: {e:#}");
                }
            } else {
                download_models(&app, Downloaded::Meiki);
            }
        }
        // ScreenAI's and OneOCR's helpers start with their first lookup; only their files are
        // fetched (copied, for OneOCR) here.
        for (kind, which) in [(OcrEngineKind::ScreenAi, Downloaded::ScreenAi), (OcrEngineKind::OneOcr, Downloaded::OneOcr)] {
            if settings.wants(kind) && which.supported() && !which.installed() {
                download_models(&app, which);
            }
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
    // Profiles change through their own commands; a save can only rename them.
    let names: Vec<(String, String)> = next.settings_profiles.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
    next.settings_profiles = previous.settings_profiles.clone();
    for p in &mut next.settings_profiles {
        if let Some((_, name)) = names.iter().find(|(id, name)| *id == p.id && !name.trim().is_empty()) {
            p.name = name.trim().to_string();
        }
    }
    next.active_settings_profile = previous.active_settings_profile.clone();
    next.global_settings = previous.global_settings.clone();
    apply_settings(&app, &state, previous, next)
}

/// Uses the settings profile `id` (empty: the global settings), Configurações › "Perfil".
#[tauri::command]
fn choose_settings_profile(app: AppHandle, state: tauri::State<'_, AppState>, id: String) -> std::result::Result<(), String> {
    let previous = state.settings.read().clone();
    let next = settings::choose_profile(previous.clone(), &id);
    switch_profile(&app, &state, previous, next)
}

/// Adds a settings profile, which starts from the global settings, and uses it. Returns its id.
#[tauri::command]
fn create_settings_profile(app: AppHandle, state: tauri::State<'_, AppState>, name: String) -> std::result::Result<String, String> {
    let previous = state.settings.read().clone();
    let (next, id) = settings::create_profile(previous.clone(), &name);
    switch_profile(&app, &state, previous, next)?;
    Ok(id)
}

/// Deletes a settings profile; the global settings are used if it was in use.
#[tauri::command]
fn delete_settings_profile(app: AppHandle, state: tauri::State<'_, AppState>, id: String) -> std::result::Result<(), String> {
    let previous = state.settings.read().clone();
    let next = settings::delete_profile(previous.clone(), &id);
    switch_profile(&app, &state, previous, next)
}

/// Applies the settings of another profile, its dictionaries' switches and order included.
fn switch_profile(app: &AppHandle, state: &AppState, previous: Settings, next: Settings) -> std::result::Result<(), String> {
    let dictionaries_changed = previous.dictionaries.iter().map(|d| (&d.id, d.enabled)).ne(next.dictionaries.iter().map(|d| (&d.id, d.enabled)));
    apply_settings(app, state, previous, next)?;
    if dictionaries_changed {
        // Opening dictionaries can take a while; the switch does not wait for it.
        let app = app.clone();
        std::thread::spawn(move || {
            load_dictionaries(&app.state::<AppState>());
            let _ = app.emit("dictionaries-changed", ());
        });
    }
    Ok(())
}

/// Replaces every setting with those of a settings file (see src/lib/settings-file.js, which
/// has already put this computer's API keys back). Dictionaries whose file is not on this
/// computer are left out, and the ones installed here but not in the file are kept. Returns the
/// names of the dictionaries left out.
#[tauri::command]
fn import_settings(app: AppHandle, state: tauri::State<'_, AppState>, settings: Settings) -> std::result::Result<Vec<String>, String> {
    let previous = state.settings.read().clone();
    let mut next = settings::migrate(settings);
    let mut missing = Vec::new();
    next.dictionaries.retain(|d| {
        let path = std::path::Path::new(&d.path);
        let here = d.kind != DictionaryKind::Mdict || path.is_file() || path.is_dir();
        if !here { missing.push(if d.name.is_empty() { d.path.clone() } else { d.name.clone() }); }
        here
    });
    for d in &previous.dictionaries {
        if !next.dictionaries.iter().any(|n| n.id == d.id) { next.dictionaries.push(d.clone()); }
    }
    apply_settings(&app, &state, previous, next)?;
    load_dictionaries(&state);
    let _ = app.emit("dictionaries-changed", ());
    Ok(missing)
}

fn apply_settings(app: &AppHandle, state: &AppState, previous: Settings, next: Settings) -> std::result::Result<(), String> {
    settings::save(&state.config_dir, &next).map_err(|e| format!("{e:#}"))?;
    let engine_changed = previous.ocr_language != next.ocr_language || previous.paddle_configs() != next.paddle_configs()
        || previous.ocr_models != next.ocr_models || previous.ocr_engine != next.ocr_engine
        || previous.screen_ocr_engine != next.screen_ocr_engine || previous.capture_auto != next.capture_auto
        || previous.use_gpu != next.use_gpu || previous.gpu_choice() != next.gpu_choice();
    *state.settings.write() = next.clone();
    if engine_changed {
        warm_engine_in_background(app);
    }
    if previous.ocr_language != next.ocr_language {
        let language = next.ocr_language.clone();
        std::thread::spawn(move || dictionaries::hunspell::warm(&language));
        warm_segmenter(app, next.ocr_language.clone());
    }
    if previous.autostart != next.autostart {
        apply_autostart(app, next.autostart);
    }
    apply_mouse_hotkey(app, &next.mouse_hotkey);
    gamepad::configure(&next.gamepad);
    if previous.hotkey != next.hotkey {
        apply_hotkey(app, &next.hotkey)?;
    }
    let _ = app.emit("settings-changed", ());
    Ok(())
}

/// The first lookup in Japanese, Chinese or Korean may come while the word analyzer is still
/// downloading or loading. The pop-up then shows its progress by the pointer and the lookup waits,
/// so it splits the words as usual instead of the longest match. The capture comes after, of the
/// screen as it is then, with the pop-up hidden again. Closing the pop-up (Esc, X, a click
/// elsewhere) gives the lookup up and returns false; the download goes on.
fn wait_for_segmenter(app: &AppHandle) -> bool {
    let language = app.state::<AppState>().settings.read().ocr_language.clone();
    // Removed in Settings, or failed before: download it now.
    if segment::want(&language) {
        warm_segmenter(app, language.clone());
    }
    let Some(mut wait) = segment::pending(&language) else { return true };
    let Some(win) = app.get_webview_window("popup") else { return true };
    let _ = app.emit_to("popup", "segmenter-wait", &wait);
    match platform::cursor_position(app) {
        Ok((cx, cy)) => open_popup(app, &win, Some((cx, cy, cx, cy)), platform::pointer_display(app, cx, cy)),
        Err(_) => open_popup(app, &win, None, None),
    }
    loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if !win.is_visible().unwrap_or(false) {
            return false;
        }
        match segment::pending(&language) {
            Some(next) if next != wait => {
                let _ = app.emit_to("popup", "segmenter-wait", &next);
                wait = next;
            }
            Some(_) => {}
            None => break,
        }
    }
    if let Some(hidden) = hide_blank(app, "popup") { let _ = hidden.join(); }
    std::thread::sleep(std::time::Duration::from_millis(20));
    // The pointer tracking `run_scan` asked KWin for has run out meanwhile.
    #[cfg(target_os = "linux")]
    if platform::session_kind() == SessionKind::KdeWayland {
        platform::kwin::track_for(std::time::Duration::from_secs(3));
    }
    true
}

/// Downloads (once) and loads the word analyzer for a recognition language, in the background;
/// Settings › OCR follows its progress.
fn warm_segmenter(app: &AppHandle, language: String) {
    let app = app.clone();
    std::thread::spawn(move || segment::warm(&language, &|| { let _ = app.emit("engine-status", ()); }));
}

/// Settings › Separação de palavras: downloads (or loads) the recognition language's analyzer.
#[tauri::command]
fn install_segmenter(app: AppHandle, state: tauri::State<'_, AppState>) {
    let language = state.settings.read().ocr_language.clone();
    segment::want(&language);
    warm_segmenter(&app, language);
}

/// Settings › Separação de palavras: deletes the recognition language's analyzer.
#[tauri::command]
async fn remove_segmenter(app: AppHandle) -> std::result::Result<(), String> {
    let language = app.state::<AppState>().settings.read().ocr_language.clone();
    tauri::async_runtime::spawn_blocking(move || {
        segment::remove(&language, &|| { let _ = app.emit("engine-status", ()); })
    }).await.map_err(|e| e.to_string())?.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn engine_status(state: tauri::State<'_, AppState>) -> EngineStatus {
    // This synchronous command runs on the UI thread. Model initialization holds these
    // locks for seconds; report a loading snapshot instead of waiting and freezing the WebView.
    let ort = state.ort_ready.try_lock().and_then(|ready| ready.clone());
    let settings = state.settings.read().clone();
    EngineStatus {
        session: platform::session_kind(),
        runtime_loaded: matches!(ort, Some(Ok(()))),
        runtime_error: ort.and_then(|r| r.err()),
        engines: engines_status(&state, &settings),
        error: state.engine_error.read().clone(),
        segmenter: segment::status(&state.settings.read().ocr_language),
        segmenter_installed: segment::installed(&state.settings.read().ocr_language),
        models: {
            let downloads = state.downloads.try_lock().map(|d| d.clone()).unwrap_or_default();
            Downloaded::ALL
                .iter()
                .filter(|d| d.supported())
                .map(|d| (d.id(), ModelState { installed: d.installed(), download: downloads.get(d.id()).cloned() }))
                .collect()
        },
        meiki_on_gpu: state.meiki.try_lock().and_then(|m| m.as_ref().map(|(_, m)| m.on_gpu())),
    }
}

/// An engine's name as Configurações › OCR lists it.
fn engine_name(kind: OcrEngineKind) -> &'static str {
    match kind {
        OcrEngineKind::Paddle => "PP-OCRv6",
        OcrEngineKind::Lens => "Google Lens",
        OcrEngineKind::Meiki => "MeikiOCR",
        OcrEngineKind::ScreenAi => "Chrome ScreenAI",
        OcrEngineKind::OneOcr => "OneOCR",
        OcrEngineKind::Bing => "Bing",
        OcrEngineKind::Indra => "IndraOCR",
        OcrEngineKind::Custom | OcrEngineKind::CustomScreen => "Personalizado",
        OcrEngineKind::Manga => "PP-OCRv6 Manga",
    }
}

/// What each selector reads with, as loaded now, for "Estado do motor": [selector, models]. A
/// model not loaded yet loads with the first lookup. Never waits for a lock a load holds.
fn engines_status(state: &AppState, settings: &Settings) -> Vec<[String; 2]> {
    let device = |gpu: bool| if !gpu { "CPU" } else if cfg!(windows) { "GPU (DirectML)" } else { "GPU (WebGPU)" };
    let waiting = "carrega na primeira consulta";
    let paddle: Vec<(PaddleUse, Arc<PaddleOcr>)> = state.paddle.try_lock().map(|cache| cache.clone()).unwrap_or_default();
    let loaded = |kind: OcrEngineKind| settings.paddle_use(kind).and_then(|u| paddle.iter().find(|(p, _)| *p == u).map(|(_, e)| e.clone()));
    let det = |kind: OcrEngineKind| match settings.models(kind).det {
        Detector::LineNet => match state.linenet.try_lock().and_then(|d| d.as_ref().map(|(_, d)| d.on_gpu())) {
            Some(gpu) => format!("LineNet na {}", device(gpu)),
            None => format!("LineNet ({waiting})"),
        },
        Detector::Ctd => match state.ctd.try_lock().and_then(|d| d.as_ref().map(|(_, d)| d.on_gpu())) {
            Some(gpu) => format!("comic-text-detector na {}", device(gpu)),
            None => format!("comic-text-detector ({waiting})"),
        },
        Detector::Page => "do próprio motor".to_string(),
        det => match loaded(kind) {
            Some(p) => format!("PP-OCRv6 {} na {}", p.det_model(), device(p.det_on_gpu())),
            None => format!("PP-OCRv6 {} ({waiting})", det.paddle_model().unwrap_or_default()),
        },
    };
    let rec = |kind: OcrEngineKind| {
        let other = |name: &str, gpu: Option<bool>| match gpu {
            Some(gpu) => format!("{name} na {}", device(gpu)),
            None => format!("{name} ({waiting})"),
        };
        match settings.models(kind).rec {
            Recognizer::Meiki => other("MeikiOCR", state.meiki.try_lock().and_then(|m| m.as_ref().map(|(_, m)| m.on_gpu()))),
            Recognizer::Indra => other("IndraOCR", state.indra.try_lock().and_then(|i| i.as_ref().map(|(_, _, i)| i.on_gpu()))),
            rec @ (Recognizer::Paddle | Recognizer::Manga | Recognizer::Page) => match loaded(kind) {
                Some(p) => format!("PP-OCRv6 {} na {}", p.rec_model, device(p.rec_on_gpu())),
                None => format!("PP-OCRv6 {} ({waiting})", if rec == Recognizer::Manga { MANGA_REC } else { settings.rec_model() }),
            },
        }
    };
    let describe = |kind: OcrEngineKind, whole_screen: bool| {
        let name = engine_name(kind);
        if kind.is_local() {
            format!("{name}: detecção {} · reconhecimento {}", det(kind), rec(kind))
        } else if !whole_screen && settings.capture_auto && settings.models(kind).det == Detector::Page {
            format!("{name}, que lê a tela inteira e deixa todas as palavras dela clicáveis")
        } else if !whole_screen && settings.capture_auto {
            format!("{name}; o bloco sob o cursor é achado pela detecção {}", det(kind))
        } else {
            name.to_string()
        }
    };
    let (pointer, screen) = (settings.engine_for(false), settings.engine_for(true));
    // The custom engine in both selectors with the same mix reads as one too.
    if (pointer == screen || (pointer == OcrEngineKind::Custom && screen == OcrEngineKind::CustomScreen && settings.models(pointer) == settings.models(screen)))
        && pointer.is_local() {
        return vec![["Cursor e tela inteira".into(), describe(pointer, false)]];
    }
    vec![["Cursor".into(), describe(pointer, false)], ["Tela inteira".into(), describe(screen, true)]]
}

#[tauri::command]
fn ocr_languages() -> Vec<LanguageInfo> {
    ocr::LANGUAGES.iter().map(|l| LanguageInfo { tag: l.tag, name: l.name, rec: l.rec, experimental: ocr::experimental_rec(l.tag) }).collect()
}

#[tauri::command]
fn list_dictionaries(state: tauri::State<'_, AppState>) -> Vec<DictInfo> {
    dictionary_infos(&state)
}

#[tauri::command]
async fn add_dictionary(app: AppHandle, path: String) -> std::result::Result<Vec<DictInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _loading = state.dictionary_gate.lock();
        let source = std::path::Path::new(&path).canonicalize().map_err(|e| format!("Abrir dicionário: {e}"))?;
        if state.settings.read().dictionaries.iter().any(|d| d.kind == DictionaryKind::Mdict
            && std::path::Path::new(&d.path).canonicalize().ok().as_ref() == Some(&source)) {
            return Err("Este dicionário já está instalado.".into());
        }
        let path = source.to_string_lossy().into_owned();
        let id = new_dict_id(&path);
        if state.settings.read().dictionaries.iter().any(|d| d.id == id) {
            return Err("This dictionary is already installed.".to_string());
        }
        // Like Lookupper: an archive is unpacked into the app's own folder and read from there.
        let content = content_dir(&state);
        let _ = app.emit("dictionary-progress", serde_json::json!({"stage": "unpacking", "bytes": 0}));
        let installed = dictionaries::source::install(&source, &content, &id, &|bytes| {
            let _ = app.emit("dictionary-progress", serde_json::json!({"stage": "unpacking", "bytes": bytes}));
        }).map_err(|e| format!("{e:#}"))?;
        let _ = app.emit("dictionary-progress", serde_json::json!({"stage": "reading", "entries": 0}));
        let opened = Dictionary::open_cached(&id, &installed, &state.dictionaries_dir, &|entries| {
            let _ = app.emit("dictionary-progress", serde_json::json!({"stage": "indexing", "entries": entries}));
        });
        let dict = match opened {
            Ok(d) => d,
            Err(e) => {
                if dictionaries::source::is_installed_copy(&installed, &content) {
                    let _ = std::fs::remove_dir_all(&installed);
                }
                return Err(format!("{e:#}"));
            }
        };
        let path = installed.to_string_lossy().into_owned();
        let name = dict.name.clone();
        {
            let mut s = state.settings.write();
            s.dictionaries.push(DictionaryConfig { id: id.clone(), kind: DictionaryKind::Mdict, path: path.clone(), name, enabled: true });
            settings::save(&state.config_dir, &s).map_err(|e| format!("{e:#}"))?;
        }
        state.dicts.write().push(Arc::new(dict));
        drop(_loading);
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
        let removed: Vec<String> = s.dictionaries.iter().filter(|d| d.id == id).map(|d| d.path.clone()).collect();
        s.dictionaries.retain(|d| d.id != id);
        settings::save(&state.config_dir, &s).map_err(|e| format!("{e:#}"))?;
        drop(s);
        state.dicts.write().retain(|d| d.id != id);
        // The unpacked copy is Lookitan's own and goes with the dictionary, as in Lookupper. A file
        // used in place is the user's and stays. A copy still in use is removed at the next start.
        let content = content_dir(&state);
        for path in removed {
            let path = std::path::Path::new(&path);
            if dictionaries::source::is_installed_copy(path, &content) {
                if let Err(e) = std::fs::remove_dir_all(path) {
                    log::warn!("remove {}: {e}; retrying at the next start", path.display());
                    let _ = std::fs::write(path.join(dictionaries::source::REMOVED_MARKER), b"");
                }
            }
        }
    }
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
fn lookup_text(state: tauri::State<'_, AppState>, text: String, sentence: Option<String>, context: Option<String>) -> LookupPayload {
    let analyzer = segment::current(&state.settings.read().ocr_language);
    let (candidates, lemmas, analyzer) = manual_candidates(&text, analyzer.as_deref().map(|a| a as &dyn segment::Segment));
    let entries = lookup_words(&state, &candidates);
    LookupPayload {
        word: text.trim().to_string(),
        sentence: sentence.unwrap_or_default(),
        context: context.unwrap_or_default(),
        candidates,
        lemmas,
        analyzer,
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
    let (origin, scale, _) = overlay_viewport(&app, monitor);
    let (dx, dy) = monitor.css_to_desktop(x, y, origin, scale);
    let (px, py) = capture.to_image(dx, dy);
    let analyzer = segment::current(&state.settings.read().ocr_language);
    let Some(pick) = words::pick(&lines, px, py, analyzer.as_deref().map(|a| a as &dyn segment::Segment)) else { return };
    if let Some(r) = state.recognition.lock().as_mut() {
        // The "sentence" screenshot frames the lines of the picked sentence.
        r.focus_lines = pick.sentence_lines.iter().map(|&i| lines[i].clone()).collect();
    }
    let (bx0, by0, bx1, by1) = pick.bounds();
    let (x0, y0) = capture.to_desktop(bx0, by0);
    let (x1, y1) = capture.to_desktop(bx1, by1);
    let payload = LookupPayload {
        entries: lookup_words(&state, &pick.candidates),
        word: pick.word,
        sentence: pick.sentence,
        context: pick.context,
        context_at: Some(pick.context_at),
        line_break: pick.line_break,
        analyzer: pick.analyzer,
        lemmas: pick.lemmas,
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

/// A click on empty space in the overlay. In a whole-screen lookup with the pop-up pinned, it
/// closes only the pop-up and keeps the words clickable, like Lookupper; the next empty click
/// closes the overlay. Otherwise it closes both at once.
#[tauri::command]
fn overlay_dismiss(app: AppHandle) {
    let state = app.state::<AppState>();
    let pinned = state.pinned.load(Ordering::SeqCst);
    let spotlight = state.recognition.lock().as_ref().is_some_and(|r| r.spotlight);
    let popup = app.get_webview_window("popup").filter(|w| w.is_visible().unwrap_or(false));
    match popup {
        Some(_) if pinned && spotlight => {
            dismiss_popup(&app);
            if let Some(w) = app.get_webview_window("overlay") {
                let _ = w.set_focus();
            }
        }
        _ => overlay_close(app),
    }
}

#[tauri::command]
fn overlay_close(app: AppHandle) {
    // Stop the background OCR of this lookup; a whole-screen lookup closed while it was still
    // reading must not reappear when the reading ends.
    app.state::<AppState>().scan_seq.fetch_add(1, Ordering::SeqCst);
    close_overlay(&app);
    dismiss_popup(&app);
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
    dismiss_popup(&app);
    // Closing the pop-up itself (Esc or its close button) closes the overlay too, even in a
    // whole-screen lookup; only an empty click on the overlay can keep it (`overlay_dismiss`).
    close_overlay(&app);
}

/// The overlay or the pop-up painted the empty state asked for by `hide_blank`.
#[tauri::command]
fn webview_blank(window: tauri::WebviewWindow, state: tauri::State<'_, AppState>, token: u64) {
    let slot = window_slot(window.label());
    let mut painted = state.blank.painted.lock();
    painted[slot] = painted[slot].max(token);
    state.blank.changed.notify_all();
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

/// Reads a file chosen by the user in the open dialog (a settings file to import).
#[tauri::command]
fn read_text_file(path: String) -> std::result::Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))
}

/// How many local audio databases stay open at once. Each costs a few MB (SQLite's page cache);
/// switching to a fourth closes the one used longest ago.
const AUDIO_DBS_OPEN: usize = 3;

/// The local audio database at `path`, opened read-only and kept among the last AUDIO_DBS_OPEN used.
fn audio_db(state: &AppState, path: &str) -> std::result::Result<Arc<audio::AudioDb>, String> {
    let mut dbs = state.audio_dbs.lock();
    if let Some(i) = dbs.iter().position(|(p, _)| p == path) {
        let entry = dbs.remove(i);
        let db = entry.1.clone();
        dbs.insert(0, entry);
        return Ok(db);
    }
    let db = Arc::new(audio::AudioDb::open(std::path::Path::new(path)).map_err(|e| format!("{e:#}"))?);
    dbs.insert(0, (path.to_string(), db.clone()));
    // Dropping the last handle closes the file (a lookup still using it keeps it until it ends).
    dbs.truncate(AUDIO_DBS_OPEN);
    Ok(db)
}

/// The recordings of a word in a local audio database (Configurações › Áudio), best first.
#[tauri::command]
async fn audio_lookup(app: AppHandle, path: String, term: String, reading: Option<String>, order: Option<Vec<String>>) -> std::result::Result<Vec<audio::AudioEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        audio_db(&state, &path)?.lookup(&term, reading.as_deref(), &order.unwrap_or_default()).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The sources inside a local audio database (nhk16, forvo, …), for Configurações › Áudio.
#[tauri::command]
async fn audio_db_sources(app: AppHandle, path: String) -> std::result::Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        audio_db(&state, &path)?.sources().map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One recording of a local audio database, as base64 with its file extension.
#[tauri::command]
async fn audio_data(app: AppHandle, path: String, source: String, file: String) -> std::result::Result<Option<(String, String)>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let bytes = audio_db(&state, &path)?.data(&source, &file).map_err(|e| format!("{e:#}"))?;
        Ok(bytes.map(|b| (base64::engine::general_purpose::STANDARD.encode(b), audio::extension(&file).to_string())))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Edge's neural voices, for Configurações › Áudio.
#[tauri::command]
async fn edge_tts_voices() -> std::result::Result<Vec<edge_tts::Voice>, String> {
    tauri::async_runtime::spawn_blocking(|| edge_tts::voices().map_err(|e| format!("{e:#}")))
        .await
        .map_err(|e| e.to_string())?
}

/// `text` read by an Edge neural voice (empty: the language's default voice), as base64 MP3.
#[tauri::command]
async fn edge_tts_speak(text: String, voice: String, language: String) -> std::result::Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        edge_tts::speak(&text, &voice, &language).map(|mp3| base64::engine::general_purpose::STANDARD.encode(mp3)).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn word_image(state: tauri::State<'_, AppState>, name: String, crop: Option<[u32; 4]>) -> Option<String> {
    let bytes = state.cards.media(&name)?;
    let bytes = match crop {
        Some(rect) => cards::crop_png(&bytes, rect).unwrap_or(bytes),
        None => bytes,
    };
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
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

    // An OCR library's helper process (ocr/helper.rs): this executable, without the app.
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if let Some(i) = args.iter().position(|a| a == ocr::helper::ARG) {
        std::process::exit(ocr::helper::main(&args[i + 1..]));
    }

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
            let dictionaries_dir = app.path().app_local_data_dir()?.join("dictionaries");
            let resource_dir = app.path().resource_dir()?;
            dictionaries::hunspell::set_root(resource_dir.join("hunspell"));
            segment::set_root(app.path().app_local_data_dir()?.join("segmenters"));
            models::set_root(app.path().app_local_data_dir()?.join("ocr-models"));
            let first_run = !settings::settings_path(&config_dir).exists();
            let settings = settings::load(&config_dir);
            let hotkey = settings.hotkey.clone();
            let mouse = settings.mouse_hotkey.clone();
            gamepad::configure(&settings.gamepad);
            app.manage(AppState {
                config_dir,
                dictionaries_dir,
                resource_dir,
                settings: RwLock::new(settings),
                dicts: RwLock::new(Vec::new()),
                dict_errors: RwLock::new(HashMap::new()),
                dictionary_gate: Mutex::new(()),
                paddle: Mutex::new(Vec::new()),
                engine_error: RwLock::new(None),
                ort_ready: Mutex::new(None),
                lens: Mutex::new(None),
                bing: Mutex::new(None),
                meiki: Mutex::new(None),
                indra: Mutex::new(None),
                linenet: Mutex::new(None),
                ctd: Mutex::new(None),
                screenai: ScreenAi::default(),
                oneocr: OneOcr::default(),
                downloads: Mutex::new(HashMap::new()),
                recognition: Mutex::new(None),
                ocr_cache: Mutex::new(OcrCache::default()),
                recognition_seq: AtomicU64::new(0),
                scan_seq: AtomicU64::new(0),
                cards: CardStore::open(&data_dir),
                busy: AtomicBool::new(false),
                pinned: AtomicBool::new(false),
                blank: Blanking::default(),
                audio_dbs: Mutex::new(Vec::new()),
            });

            // Configured WebViews can invoke get_settings before setup reaches manage().
            // Create them explicitly once AppState exists so every window's first IPC is safe.
            for config in app.config().app.windows.clone() {
                let window = WebviewWindowBuilder::from_config(app, &config)?.build()?;
                // The overlay covers the whole monitor: as a tool window it does not make the
                // browser under it stop drawing its video (see win::keep_tool_window).
                #[cfg(windows)]
                if matches!(config.label.as_str(), "overlay" | "popup") {
                    if let Ok(hwnd) = window.hwnd() { platform::win::keep_tool_window(hwnd.0 as _); }
                }
                #[cfg(not(windows))]
                let _ = window;
            }

            if let Err(e) = apply_hotkey(&handle, &hotkey) {
                log::warn!("{e}");
            }
            apply_mouse_hotkey(&handle, &mouse);
            {
                let (visible, signal) = (handle.clone(), handle.clone());
                gamepad::start(move || gamepad_target(&visible).is_some(), move |s| on_gamepad(&signal, s));
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
                    remove_unused_copies(&state);
                    load_dictionaries(&state);
                    let _ = handle.emit("dictionaries-changed", ());
                    let language = state.settings.read().ocr_language.clone();
                    dictionaries::hunspell::warm(&language);
                    warm_segmenter(&handle, language);
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
                let h = handle.clone();
                popup.on_window_event(move |event| {
                    if let WindowEvent::Focused(false) = event {
                        if !h.state::<AppState>().pinned.load(Ordering::SeqCst) {
                            dismiss_popup(&h);
                        }
                    }
                });
            }
            if let Some(overlay) = app.get_webview_window("overlay") {
                let h = handle.clone();
                overlay.on_window_event(move |event| {
                    if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }) {
                        let state = h.state::<AppState>();
                        // A newer scan may have raised the (transparent) overlay while the
                        // recognition is still the previous lookup's; do not redraw that one.
                        let scan = state.scan_seq.load(Ordering::SeqCst);
                        let current = state.recognition.lock().as_ref().filter(|r| r.scan == scan)
                            .map(|r| (r.id, r.complete, r.spotlight));
                        // A whole-screen lookup still being read keeps its words until it is done.
                        if let Some((id, complete, spotlight)) = current {
                            if complete || !spotlight { emit_overlay(&h, id, None, complete); }
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
            import_settings,
            choose_settings_profile,
            create_settings_profile,
            delete_settings_profile,
            engine_status,
            install_segmenter,
            remove_segmenter,
            install_ocr_models,
            remove_ocr_models,
            ocr_languages,
            list_dictionaries,
            add_dictionary,
            remove_dictionary,
            update_dictionary,
            lookup_text,
            overlay_pick,
            overlay_close,
            overlay_dismiss,
            capture_image,
            capture_card_image,
            scan_now,
            hide_popup,
            webview_blank,
            set_popup_pinned,
            open_external,
            open_settings,
            entry_html,
            list_words,
            save_word,
            delete_word,
            word_image,
            gamepad_capture,
            gamepad_status,
            popup_dismiss,
            audio_lookup,
            audio_db_sources,
            edge_tts_voices,
            edge_tts_speak,
            audio_data,
            write_text_file,
            read_text_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running Lookitan");
}

#[cfg(test)]
mod popup_position_tests {
    use super::{popup_position, PopupPlace};
    use crate::platform::geometry::Rect;

    const SCREEN: Rect = Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 };
    const POPUP: (f64, f64) = (560.0, 520.0);

    #[test]
    fn the_popup_is_centred_on_the_word_8_px_below_it() {
        let p = popup_position((900.0, 300.0, 1000.0, 320.0), POPUP, Some(SCREEN), 1.0);
        assert_eq!(p, PopupPlace { x: 670.0, y: 328.0, origin: (280.0, 0.0) }, "the word's centre is the popup's centre");
    }

    #[test]
    fn without_room_below_it_goes_above_the_word() {
        let p = popup_position((900.0, 800.0, 1000.0, 820.0), POPUP, Some(SCREEN), 1.0);
        assert_eq!((p.x, p.y, p.origin.1), (670.0, 800.0 - 8.0 - 520.0, 520.0));
    }

    #[test]
    fn a_word_too_near_a_side_to_centre_the_popup_gets_it_beside_the_word_first() {
        // As in Lookupper (the user's screenshot of "passou"): centred above or below does not fit
        // sideways, so WinUI takes Left before any shifted variant.
        let p = popup_position((1700.0, 300.0, 1760.0, 320.0), POPUP, Some(SCREEN), 1.0);
        assert_eq!(p, PopupPlace { x: 1700.0 - 8.0 - 560.0, y: 310.0 - 260.0, origin: (560.0, 260.0) });
        // Near the left edge: the right side.
        let p = popup_position((40.0, 300.0, 100.0, 320.0), POPUP, Some(SCREEN), 1.0);
        assert_eq!((p.x, p.origin.0), (108.0, 0.0));
    }

    #[test]
    fn with_no_side_free_either_it_slides_with_the_word_28_px_from_the_tips_edge() {
        // A narrow screen: no room on the sides, so below and shifted right of the word.
        let narrow = Rect { x: 0.0, y: 0.0, width: 680.0, height: 1080.0 };
        let p = popup_position((60.0, 300.0, 100.0, 320.0), POPUP, Some(narrow), 1.0);
        assert_eq!((p.x, p.y), (80.0 - 28.0 + 8.0, 328.0), "BottomRight: the word's centre 20 px into the card");
        assert_eq!(p.origin, (20.0, 0.0));
    }

    #[test]
    fn on_a_small_screen_it_goes_beside_the_word_instead_of_over_it() {
        let small = Rect { x: 0.0, y: 0.0, width: 1366.0, height: 600.0 };
        let p = popup_position((900.0, 280.0, 960.0, 300.0), POPUP, Some(small), 1.0);
        assert_eq!((p.x, p.origin.0), (900.0 - 8.0 - 560.0, 560.0));
        assert!(p.x + POPUP.0 <= 900.0, "the word stays visible");
    }

    #[test]
    fn with_room_nowhere_it_stays_on_the_screen() {
        let tiny = Rect { x: 0.0, y: 0.0, width: 800.0, height: 500.0 };
        let p = popup_position((380.0, 240.0, 420.0, 260.0), POPUP, Some(tiny), 1.0);
        assert!(p.y >= 0.0 && p.x >= 0.0 && p.x + POPUP.0 <= 800.0, "{p:?} stays on the screen");
    }

    #[test]
    fn sizes_follow_the_display_scale() {
        let big = Rect { x: 0.0, y: 0.0, width: 2880.0, height: 1620.0 };
        let p = popup_position((1400.0, 300.0, 1480.0, 330.0), (840.0, 780.0), Some(big), 1.5);
        assert_eq!((p.x, p.y), (1440.0 - 420.0, 342.0));
    }
}
