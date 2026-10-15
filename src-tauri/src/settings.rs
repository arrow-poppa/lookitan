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
    pub dictionaries: Vec<DictionaryConfig>,
    pub profiles: Vec<AppProfile>,
    pub autostart: bool,
    pub save_words_automatically: bool,
    /// Language of definitions and of AI explanations and translations.
    pub definition_language: String,
    pub translation_language: String,
    /// Shows the word's translation on the AI word card.
    pub show_word_translation: bool,
    pub hide_word_translation: bool,
    /// AI options, owned by the front end (same shape as the yomitan-byok-ai `ai` options).
    pub ai: serde_json::Value,
    /// Anki options, owned by the front end (same shape as Yomitan's `anki` options).
    pub anki: serde_json::Value,
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
            show_word_translation: true,
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
pub fn migrate(mut s: Settings) -> Settings {
    // The definition and translation languages follow Lookupper's list: Latin American Spanish
    // and Chinese by script replaced the Mexican, mainland and Taiwanese tags.
    for tag in [&mut s.definition_language, &mut s.translation_language] {
        if let Some(new) = renamed_language(tag.as_str()) {
            *tag = new.to_string();
        }
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
    fn gpu_choice_is_off_by_default() {
        let old: Settings = serde_json::from_str(r#"{"useGpu": true}"#).unwrap();
        assert_eq!(old.gpu_choice(), None, "settings saved before the option keep the API default");
        let chosen: Settings = serde_json::from_str(r#"{"chooseGpu": true, "gpuPreference": "lowPower"}"#).unwrap();
        assert_eq!(chosen.gpu_choice(), Some(GpuPreference::LowPower));
    }
}
