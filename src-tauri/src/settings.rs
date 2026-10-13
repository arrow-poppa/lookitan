// Persistent user settings, stored as JSON in the app config directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OcrEngineKind {
    /// PP-OCRv6 on the local GPU (DirectML) or CPU.
    Paddle,
    /// Google Lens over the network.
    Lens,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OcrScope {
    /// The whole monitor under the pointer; every word on it becomes clickable.
    Screen,
    /// Only a region around the pointer (faster, no clickable overlay beyond it).
    Region,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DictionaryKind {
    Mdict,
    /// Built-in: AI explanation of the word in context.
    AiWord,
    /// Built-in: AI translation of the sentence around the word.
    AiContext,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct DictionaryConfig {
    pub id: String,
    pub kind: DictionaryKind,
    pub path: String,
    pub name: String,
    pub enabled: bool,
}

impl Default for DictionaryConfig {
    fn default() -> Self {
        Self { id: String::new(), kind: DictionaryKind::Mdict, path: String::new(), name: String::new(), enabled: true }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FreezeMode {
    None,
    /// Show the captured frame behind the overlay, so a video or game seems paused.
    Frame,
    /// Suspend the foreground process while the overlay is open.
    Process,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct AppProfile {
    pub executable: String,
    pub display_name: String,
    /// Give the overlay keyboard focus (for games that keep the keyboard otherwise).
    pub focus_overlay: bool,
    pub freeze: FreezeMode,
}

impl Default for AppProfile {
    fn default() -> Self {
        Self { executable: String::new(), display_name: String::new(), focus_overlay: false, freeze: FreezeMode::None }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    None,
    Left,
    Right,
    Middle,
    X1,
    X2,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct MouseHotkey {
    pub button: MouseButton,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    /// Trigger when the button is held instead of clicked.
    pub long_press: bool,
}

impl Default for MouseHotkey {
    fn default() -> Self {
        Self { button: MouseButton::None, alt: false, ctrl: false, shift: false, long_press: false }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// "popup" (dictionary) or "translator" (screen translator).
    pub app_mode: String,
    /// Global shortcut in Tauri accelerator syntax; empty disables it.
    pub hotkey: String,
    pub mouse_hotkey: MouseHotkey,
    pub ocr_engine: OcrEngineKind,
    pub ocr_scope: OcrScope,
    /// BCP 47 tag selecting the recognition model (see `ocr::languages`).
    pub ocr_language: String,
    pub use_gpu: bool,
    /// Key for the Google Lens endpoint; Lookitan ships none.
    pub lens_api_key: String,
    /// Size of the captured region around the pointer (region scope), in logical pixels.
    pub capture_width: u32,
    pub capture_height: u32,
    pub dictionaries: Vec<DictionaryConfig>,
    pub profiles: Vec<AppProfile>,
    pub autostart: bool,
    pub save_words_automatically: bool,
    /// Language of definitions and of AI explanations and translations.
    pub definition_language: String,
    pub translation_language: String,
    pub hide_word_translation: bool,
    /// AI options, owned by the front end (same shape as the yomitan-byok-ai `ai` options).
    pub ai: serde_json::Value,
    /// Anki options, owned by the front end (same shape as Yomitan's `anki` options).
    pub anki: serde_json::Value,
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            app_mode: "popup".into(),
            hotkey: "Alt+D".into(),
            mouse_hotkey: MouseHotkey::default(),
            ocr_engine: OcrEngineKind::Paddle,
            ocr_scope: OcrScope::Screen,
            ocr_language: "en-US".into(),
            use_gpu: true,
            lens_api_key: String::new(),
            capture_width: 900,
            capture_height: 260,
            dictionaries: builtin_dictionaries(),
            profiles: Vec::new(),
            autostart: false,
            save_words_automatically: false,
            definition_language: "en-US".into(),
            translation_language: "pt-BR".into(),
            hide_word_translation: false,
            ai: serde_json::Value::Null,
            anki: serde_json::Value::Null,
            theme: "system".into(),
        }
    }
}

pub fn builtin_dictionaries() -> Vec<DictionaryConfig> {
    vec![
        DictionaryConfig { id: "ai-word".into(), kind: DictionaryKind::AiWord, name: "AI word in context".into(), ..Default::default() },
        DictionaryConfig { id: "ai-context".into(), kind: DictionaryKind::AiContext, name: "Context translator".into(), ..Default::default() },
    ]
}

/// Settings saved by older versions have no built-in AI dictionaries; add them at the top.
fn migrate(mut s: Settings) -> Settings {
    for (pos, b) in builtin_dictionaries().into_iter().enumerate() {
        if !s.dictionaries.iter().any(|d| d.id == b.id) {
            s.dictionaries.insert(pos.min(s.dictionaries.len()), b);
        }
    }
    s
}

pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

pub fn load(config_dir: &Path) -> Settings {
    let path = settings_path(config_dir);
    let s = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("settings file {} is invalid ({e}); using defaults", path.display());
            Settings::default()
        }),
        Err(_) => Settings::default(),
    };
    migrate(s)
}

pub fn save(config_dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let path = settings_path(config_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}
