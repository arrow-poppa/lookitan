// Persistent user settings, stored as JSON in the app config directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OcrEngineKind {
    /// PP-OCRv6 on the local GPU (DirectML on Windows, WebGPU on Linux) or CPU.
    Paddle,
    /// Google Lens over the network.
    Lens,
}

/// Which video card OCR asks for when the user chooses one (`Settings::choose_gpu`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GpuPreference {
    /// The dedicated card on a machine with two (NVIDIA or AMD next to an Intel iGPU).
    #[default]
    HighPerformance,
    /// The integrated card, which draws less power.
    LowPower,
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
    /// Built-in: pictures of the word from Bing Images, searched with an AI-made query (as in
    /// Migaku's pop-up). The front end does the search; the backend only keeps its place and switch.
    AiImages,
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
    /// Only in settings saved while a global freeze option existed; `migrate()` makes it `None`.
    Default,
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

/// Game controller settings (gamepad.rs). Buttons go by their Xbox names (`gamepad::BUTTONS`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct GamepadSettings {
    /// Buttons held together to look up the word at the cursor; empty turns it off.
    pub hotkey: Vec<String>,
    /// The D-pad and the left stick move between the pop-up's controls (or the overlay's words),
    /// the right stick scrolls, and the four buttons below act. Off for a controller that
    /// already moves the mouse cursor (Steam Input), as Lookupper advises.
    pub navigation: bool,
    pub confirm: String,
    pub close: String,
    pub save: String,
    pub pronounce: String,
}

impl Default for GamepadSettings {
    fn default() -> Self {
        Self { hotkey: Vec::new(), navigation: true, confirm: "A".into(), close: "B".into(), save: "X".into(), pronounce: "Y".into() }
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
    pub gamepad: GamepadSettings,
    pub ocr_engine: OcrEngineKind,
    pub ocr_scope: OcrScope,
    /// Engine for whole-screen passes (the Screen scope, or a shortcut pressed away from text).
    /// `None` uses `ocr_engine`.
    pub screen_ocr_engine: Option<OcrEngineKind>,
    /// BCP 47 tag selecting the recognition model (see `ocr::languages`).
    pub ocr_language: String,
    pub use_gpu: bool,
    /// Off: the GPU API's own default (adapter 0 for DirectML, Dawn's default on Linux).
    /// On: OCR asks for the card in `gpu_preference`.
    pub choose_gpu: bool,
    pub gpu_preference: GpuPreference,
    /// Key for the Google Lens endpoint; Lookitan ships none.
    pub lens_api_key: String,
    /// Automatic capture, as in Lookupper's pointer mode: the whole monitor is captured and
    /// detected, and only the block of text under the pointer is recognized (see block.rs).
    /// When off, the fixed region below is read.
    pub capture_auto: bool,
    /// Size of the captured region around the pointer (region scope), in logical pixels.
    pub capture_width: u32,
    pub capture_height: u32,
    /// An entry this version cannot read (a kind from a newer or a test build) is left out, rather
    /// than making the whole file invalid and every setting fall back to its default.
    #[serde(deserialize_with = "readable_dictionaries")]
    pub dictionaries: Vec<DictionaryConfig>,
    pub profiles: Vec<AppProfile>,
    pub autostart: bool,
    pub save_words_automatically: bool,
    /// Language of definitions, of AI translations, and of the AI explanation ("Explicar").
    pub definition_language: String,
    pub translation_language: String,
    /// Empty in settings saved before it was its own setting; `migrate()` then copies the
    /// translation language, which explanations used.
    pub explanation_language: String,
    /// Shows the word's translation on the AI word card.
    pub show_word_translation: bool,
    pub hide_word_translation: bool,
    /// AI options, owned by the front end (same shape as the yomitan-byok-ai `ai` options).
    pub ai: serde_json::Value,
    /// Anki options, owned by the front end (same shape as Yomitan's `anki` options).
    pub anki: serde_json::Value,
    /// Word audio sources, owned by the front end (src/lib/audio.js): local audio databases,
    /// Yomitan-style custom URLs and the system voice.
    pub audio: serde_json::Value,
    pub theme: String,
}

impl Settings {
    /// The engine for a pass around the pointer or over the whole screen.
    pub fn engine_for(&self, whole_screen: bool) -> OcrEngineKind {
        if whole_screen { self.screen_ocr_engine.unwrap_or(self.ocr_engine) } else { self.ocr_engine }
    }

    /// The card OCR asks for, or `None` to leave the choice to the GPU API.
    pub fn gpu_choice(&self) -> Option<GpuPreference> {
        self.choose_gpu.then_some(self.gpu_preference)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            app_mode: "popup".into(),
            hotkey: "Alt+D".into(),
            mouse_hotkey: MouseHotkey::default(),
            gamepad: GamepadSettings::default(),
            ocr_engine: OcrEngineKind::Paddle,
            // Keep the original lookup's small capture; full-screen overlay remains opt-in.
            ocr_scope: OcrScope::Region,
            screen_ocr_engine: None,
            ocr_language: "en-US".into(),
            use_gpu: true,
            choose_gpu: false,
            gpu_preference: GpuPreference::HighPerformance,
            lens_api_key: String::new(),
            capture_auto: true,
            capture_width: 900,
            capture_height: 260,
            dictionaries: builtin_dictionaries(),
            profiles: Vec::new(),
            autostart: false,
            save_words_automatically: false,
            definition_language: "en-US".into(),
            translation_language: "pt-BR".into(),
            explanation_language: String::new(),
            show_word_translation: true,
            hide_word_translation: false,
            ai: serde_json::Value::Null,
            anki: serde_json::Value::Null,
            audio: serde_json::Value::Null,
            theme: "system".into(),
        }
    }
}

pub fn builtin_dictionaries() -> Vec<DictionaryConfig> {
    vec![
        DictionaryConfig { id: "ai-word".into(), kind: DictionaryKind::AiWord, name: "AI word in context".into(), ..Default::default() },
        DictionaryConfig { id: "ai-context".into(), kind: DictionaryKind::AiContext, name: "Context translator".into(), ..Default::default() },
        DictionaryConfig { id: "ai-images".into(), kind: DictionaryKind::AiImages, name: "Images".into(), ..Default::default() },
    ]
}

/// The dictionaries of a settings file, without the entries this version cannot read.
fn readable_dictionaries<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<DictionaryConfig>, D::Error> {
    let entries: Vec<serde_json::Value> = Vec::deserialize(deserializer)?;
    Ok(entries.into_iter().filter_map(|entry| match serde_json::from_value(entry) {
        Ok(d) => Some(d),
        Err(e) => {
            log::warn!("left out a dictionary entry this version cannot read ({e})");
            None
        }
    }).collect())
}

/// Settings saved by older versions have no built-in AI dictionaries; add them at the top.
pub fn migrate(mut s: Settings) -> Settings {
    // The definition and translation languages follow Lookupper's list: Latin American Spanish
    // and Chinese by script replaced the Mexican, mainland and Taiwanese tags.
    for tag in [&mut s.definition_language, &mut s.translation_language, &mut s.explanation_language] {
        if let Some(new) = renamed_language(tag.as_str()) {
            *tag = new.to_string();
        }
    }
    if s.explanation_language.is_empty() {
        s.explanation_language = s.translation_language.clone();
    }
    // The scope is no longer a setting: lookups read around the pointer, and a shortcut pressed
    // away from text reads the whole screen.
    s.ocr_scope = OcrScope::Region;
    // Freezing is chosen per app again, and "no freeze" is the default: profiles that followed
    // the short-lived global option do not freeze.
    for p in &mut s.profiles {
        if p.freeze == FreezeMode::Default { p.freeze = FreezeMode::None; }
    }
    for (pos, b) in builtin_dictionaries().into_iter().enumerate() {
        if !s.dictionaries.iter().any(|d| d.id == b.id) {
            s.dictionaries.insert(pos.min(s.dictionaries.len()), b);
        }
    }
    s
}

/// The tag that replaced an older definition or translation language tag.
fn renamed_language(tag: &str) -> Option<&'static str> {
    match tag {
        "es-MX" => Some("es-419"),
        "zh-CN" => Some("zh-Hans-CN"),
        "zh-TW" => Some("zh-Hant-TW"),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renamed_definition_and_translation_languages_are_migrated() {
        let s = migrate(Settings { definition_language: "zh-TW".into(), translation_language: "es-MX".into(), ..Settings::default() });
        assert_eq!(s.definition_language, "zh-Hant-TW");
        assert_eq!(s.translation_language, "es-419");
        let s = migrate(Settings::default());
        assert_eq!((s.definition_language.as_str(), s.translation_language.as_str()), ("en-US", "pt-BR"));
        // The explanation used the translation language before it had its own.
        assert_eq!(s.explanation_language, "pt-BR");
        let saved: Settings = serde_json::from_str(r#"{"translationLanguage": "es-419"}"#).unwrap();
        assert_eq!(migrate(saved).explanation_language, "es-419");
        let saved: Settings = serde_json::from_str(r#"{"translationLanguage": "es-419", "explanationLanguage": "en-US"}"#).unwrap();
        assert_eq!(migrate(saved).explanation_language, "en-US");
    }

    #[test]
    fn profiles_choose_freeze_and_default_to_no_freeze() {
        let saved: Settings = serde_json::from_str(r#"{"freezeOnLookup": true, "profiles": [
            {"executable": "game.exe", "freeze": "default"},
            {"executable": "video.exe", "freeze": "frame"},
            {"executable": "new.exe"}
        ]}"#).unwrap();
        let s = migrate(saved);
        assert_eq!(s.profiles[0].freeze, FreezeMode::None, "the old global option is gone");
        assert_eq!(s.profiles[1].freeze, FreezeMode::Frame);
        assert_eq!(s.profiles[2].freeze, FreezeMode::None);
    }

    #[test]
    fn images_dictionary_is_added_after_the_ai_ones_and_keeps_its_place() {
        let saved: Settings = serde_json::from_str(r#"{"dictionaries": [
            {"id": "ai-word", "kind": "ai-word"},
            {"id": "ai-context", "kind": "ai-context", "enabled": false},
            {"id": "local", "kind": "mdict", "path": "x.mdx"}
        ]}"#).unwrap();
        let s = migrate(saved);
        let ids: Vec<&str> = s.dictionaries.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["ai-word", "ai-context", "ai-images", "local"]);
        assert_eq!(s.dictionaries[2].kind, DictionaryKind::AiImages);
        assert!(s.dictionaries[2].enabled);
        let moved: Settings = serde_json::from_str(r#"{"dictionaries": [
            {"id": "ai-images", "kind": "ai-images", "enabled": false},
            {"id": "ai-word", "kind": "ai-word"},
            {"id": "ai-context", "kind": "ai-context"}
        ]}"#).unwrap();
        let s = migrate(moved);
        assert_eq!(s.dictionaries[0].id, "ai-images", "a moved or switched-off images entry stays as the user left it");
        assert!(!s.dictionaries[0].enabled);
        assert_eq!(s.dictionaries.len(), 3);
    }

    #[test]
    fn a_dictionary_of_an_unknown_kind_is_left_out_and_the_rest_is_kept() {
        let saved: Settings = serde_json::from_str(r#"{"definitionLanguage": "es-419", "dictionaries": [
            {"id": "ai-word", "kind": "ai-word"},
            {"id": "ai-context", "kind": "ai-context"},
            {"id": "ai-images", "kind": "ai-images"},
            {"id": "from-a-newer-build", "kind": "something-new", "enabled": true},
            {"id": "local", "kind": "mdict", "path": "x.mdx"}
        ]}"#).unwrap();
        assert_eq!(saved.definition_language, "es-419", "the other settings are kept");
        let ids: Vec<&str> = saved.dictionaries.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["ai-word", "ai-context", "ai-images", "local"]);
    }

    #[test]
    fn gpu_choice_is_off_by_default() {
        let old: Settings = serde_json::from_str(r#"{"useGpu": true}"#).unwrap();
        assert_eq!(old.gpu_choice(), None, "settings saved before the option keep the API default");
        let chosen: Settings = serde_json::from_str(r#"{"chooseGpu": true, "gpuPreference": "lowPower"}"#).unwrap();
        assert_eq!(chosen.gpu_choice(), Some(GpuPreference::LowPower));
    }
}
