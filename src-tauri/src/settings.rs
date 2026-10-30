// Persistent user settings, stored as JSON in the app config directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// An engine this build does not know (a newer version's, or another platform's in an imported
/// file) reads as `Paddle`, so the rest of the settings still load.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase", from = "String")]
pub enum OcrEngineKind {
    /// PP-OCRv6 on the local GPU (DirectML on Windows, WebGPU on Linux) or CPU.
    Paddle,
    /// Google Lens over the network.
    Lens,
    /// PP-OCR's detector with MeikiOCR's recognizer (ocr/meiki.rs), for Japanese; its models are
    /// downloaded when it is chosen.
    Meiki,
    /// Chrome's Screen AI library (ocr/screenai.rs), any language, in a helper process; its files
    /// are downloaded when it is chosen.
    ScreenAi,
    /// Microsoft's OneOCR from the Snipping Tool (ocr/oneocr.rs), Windows only, in a helper
    /// process; its files are copied from the installed Snipping Tool when it is chosen.
    OneOcr,
    /// Bing's visual search OCR over the network (ocr/bing.rs).
    Bing,
    /// IndraOCR's own detector (LineNet) and recognizer (ocr/indra.rs); its models are downloaded
    /// once the user accepts their licence.
    Indra,
    /// The custom engine ("Personalizado"): the detector and the recognizer the user picked, each
    /// from any local engine, to try them together (`Settings::ocr_models["custom"]`).
    Custom,
    /// The custom engine as the whole screen's selector reads with it: its own choice
    /// (`ocr_models["customScreen"]`, else the pointer's). Never stored: `engine_for` gives it for
    /// `Custom` over the whole screen, so each selector can try its own mix.
    CustomScreen,
    /// PP-OCRv6 Manga (`models::MANGA`): PP-OCR's pipeline with its detector and recognizer
    /// fine-tuned on manga, for Japanese and Chinese; its models are downloaded when it is chosen.
    Manga,
}

impl OcrEngineKind {
    /// Its name in Settings (`ocr_models`) and in the UI's lists.
    pub fn id(self) -> &'static str {
        match self {
            Self::Paddle => "paddle",
            Self::Lens => "lens",
            Self::Meiki => "meiki",
            Self::ScreenAi => "screenai",
            Self::OneOcr => "oneocr",
            Self::Bing => "bing",
            Self::Indra => "indra",
            Self::Custom => "custom",
            Self::CustomScreen => "customScreen",
            Self::Manga => "manga",
        }
    }

    /// Whether it reads with a detector and a recognizer of Lookitan's (the others read the whole
    /// image they get by themselves).
    pub fn is_local(self) -> bool {
        matches!(self, Self::Paddle | Self::Meiki | Self::Indra | Self::Custom | Self::CustomScreen | Self::Manga)
    }

    /// The detectors it can be set to: each engine its own (MeikiOCR's or PP-OCRv6's for MeikiOCR;
    /// PP-OCRv6 Manga's and comic-text-detector for PP-OCRv6 Manga), the custom engine any
    /// local one, and a page engine, whose detector only finds the block of text under the pointer,
    /// any local one or its own (the whole capture read, the block under the pointer kept).
    pub fn offers(self, det: Detector) -> bool {
        match self {
            Self::Indra => det == Detector::LineNet,
            Self::Paddle => matches!(det, Detector::PaddleTiny | Detector::PaddleSmall),
            Self::Meiki => matches!(det, Detector::PaddleTiny | Detector::PaddleSmall | Detector::MeikiDet | Detector::MeikiDetSmall),
            Self::Manga => matches!(det, Detector::MangaDet | Detector::Ctd),
            Self::Custom | Self::CustomScreen => det != Detector::Page,
            Self::Lens | Self::ScreenAi | Self::OneOcr | Self::Bing => true,
        }
    }
}

/// What reads a pass's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recognizer {
    /// PP-OCR's model of the OCR language (`Settings::rec_model`).
    Paddle,
    Meiki,
    Indra,
    /// PP-OCRv6 Manga's (`paddle::MANGA_REC`), PP-OCR's recognizer fine-tuned on manga.
    Manga,
    /// A page engine (Lens, ScreenAI, OneOCR, Bing), which finds and reads the text by itself.
    Page,
}

/// What finds a local engine's lines, or a page engine's block under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detector {
    PaddleTiny,
    PaddleSmall,
    /// IndraOCR's own (`ocr::indra::IndraDet`), downloaded with its recognizers.
    LineNet,
    /// PP-OCRv6 Manga's (`paddle::MANGA_DET`), PP-OCR's tiny detector fine-tuned on manga.
    MangaDet,
    /// comic-text-detector's line map (`ocr::ctd::CtdDet`), downloaded when chosen.
    Ctd,
    /// MeikiOCR's 960×544 detector (`ocr::meiki_det::MeikiDet`), and its 320×192 one, downloaded
    /// when chosen. Around the pointer they read a rectangle that grows, as marupop does.
    MeikiDet,
    MeikiDetSmall,
    /// A page engine's own: it reads the whole capture and the block under the pointer is kept.
    Page,
}

impl Detector {
    const ALL: [Detector; 8] = [
        Detector::PaddleTiny,
        Detector::PaddleSmall,
        Detector::LineNet,
        Detector::MangaDet,
        Detector::Ctd,
        Detector::MeikiDet,
        Detector::MeikiDetSmall,
        Detector::Page,
    ];

    /// Its name in `ModelChoice::det`.
    pub fn id(self) -> &'static str {
        match self {
            Detector::PaddleTiny => "paddle:det_tiny",
            Detector::PaddleSmall => "paddle:det_small",
            Detector::LineNet => "indra:linenet",
            Detector::MangaDet => "manga:det",
            Detector::Ctd => "ctd:comic",
            Detector::MeikiDet => "meiki:det960",
            Detector::MeikiDetSmall => "meiki:det320",
            Detector::Page => "page:own",
        }
    }

    fn of(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.id() == id)
    }

    /// PP-OCR's model, for a detector PP-OCR runs (PP-OCRv6's and PP-OCRv6 Manga's).
    pub fn paddle_model(self) -> Option<&'static str> {
        match self {
            Detector::PaddleTiny => Some("det_tiny"),
            Detector::PaddleSmall => Some("det_small"),
            Detector::MangaDet => Some(crate::ocr::paddle::MANGA_DET),
            Detector::LineNet | Detector::Ctd | Detector::MeikiDet | Detector::MeikiDetSmall | Detector::Page => None,
        }
    }
}

/// What a pass with an engine reads with (`Settings::models`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Models {
    pub det: Detector,
    pub det_gpu: bool,
    pub rec: Recognizer,
    pub rec_gpu: bool,
    /// None chosen, on PP-OCRv6's detector: Lookupper's choice (`EnginePlan::automatic`), det_small
    /// on the GPU and det_tiny on the CPU, the recognizer where the detector is.
    pub automatic: bool,
}

/// An engine's models as the user chose them (Configurações › OCR, under its selector): the
/// detector (`Detector::id`), the recognizer (the custom engine's only: `"paddle"`, the language's
/// PP-OCR model; `"meiki"`; `"indra"`; `"manga"`) and whether each runs on the GPU.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelChoice {
    pub det: String,
    pub det_gpu: bool,
    pub rec: String,
    pub rec_gpu: bool,
}

impl Default for ModelChoice {
    fn default() -> Self {
        Self { det: "paddle:det_small".into(), det_gpu: true, rec: "paddle".into(), rec_gpu: true }
    }
}

impl From<String> for OcrEngineKind {
    fn from(name: String) -> Self {
        match name.as_str() {
            "paddle" => Self::Paddle,
            "lens" => Self::Lens,
            "meiki" => Self::Meiki,
            "screenai" => Self::ScreenAi,
            "oneocr" => Self::OneOcr,
            "bing" => Self::Bing,
            "indra" => Self::Indra,
            "custom" => Self::Custom,
            "manga" => Self::Manga,
            other => {
                log::warn!("unknown OCR engine {other:?}; using PP-OCRv6");
                Self::Paddle
            }
        }
    }
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
    /// the right stick scrolls, and the actions below act. Off for a controller that
    /// already moves the mouse cursor (Steam Input), as Lookupper advises.
    pub navigation: bool,
    /// Each action's button, or buttons held together; empty leaves the action off. Settings
    /// saved before combinations had one name each, read as a list of one.
    #[serde(deserialize_with = "combo")]
    pub confirm: Vec<String>,
    #[serde(deserialize_with = "combo")]
    pub close: Vec<String>,
    #[serde(deserialize_with = "combo")]
    pub save: Vec<String>,
    #[serde(deserialize_with = "combo")]
    pub pronounce: Vec<String>,
    /// Generates the AI explanation ("Explicar"); Lookupper has no such action.
    #[serde(deserialize_with = "combo")]
    pub explain: Vec<String>,
    /// Sends the word to Anki (the pop-up's Anki button); "save" keeps it in Palavras.
    #[serde(deserialize_with = "combo")]
    pub anki: Vec<String>,
    /// Switches the pictures' meme mode (the pop-up's "Modo meme"); no button by default.
    #[serde(deserialize_with = "combo")]
    pub meme: Vec<String>,
}

impl Default for GamepadSettings {
    fn default() -> Self {
        let one = |b: &str| vec![b.to_string()];
        Self { hotkey: Vec::new(), navigation: true, confirm: one("A"), close: one("B"), save: one("X"), pronounce: one("Y"), explain: one("RB"), anki: one("LB"), meme: Vec::new() }
    }
}

fn combo<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany { One(String), Many(Vec<String>) }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(b) if b.is_empty() => Vec::new(),
        OneOrMany::One(b) => vec![b],
        OneOrMany::Many(buttons) => buttons,
    })
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
    /// Languages that read with `rec_tiny` and were switched to `rec_small` (Configurações › OCR,
    /// PP-OCRv6's "Reconhecimento"), each language on its own.
    pub small_recognizer: Vec<String>,
    /// Languages switched to their experimental recognizer (`ocr::experimental_rec`; only Russian,
    /// to PP-OCRv6 Tiny RU), each on its own.
    pub experimental_recognizer: Vec<String>,
    /// Where the engines run that the user has chosen no models for (`Settings::models`).
    pub use_gpu: bool,
    /// Settings saved while the models were chosen in one manual mode ("Escolher modelos
    /// manualmente") and its PP-OCRv6 choices: `migrate` moves them into `ocr_models` and drops
    /// them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_manual: Option<bool>,
    #[serde(skip_serializing)]
    pub manual_det: String,
    #[serde(skip_serializing)]
    pub manual_det_gpu: bool,
    #[serde(skip_serializing)]
    pub manual_rec_gpu: bool,
    /// Each engine's models as the user chose them, by `OcrEngineKind::id`; an engine without
    /// reads with its own (`Settings::models`).
    pub ocr_models: std::collections::BTreeMap<String, ModelChoice>,
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
    /// Profiles of every setting but the saved words (Configurações › "Perfil"), chosen by hand;
    /// each keeps only what it changes from the global settings (see `sync_profile`).
    pub settings_profiles: Vec<SettingsProfile>,
    /// The profile in use; empty for the global settings.
    pub active_settings_profile: String,
    /// The global settings, as `snapshot` writes them, while a profile is in use.
    pub global_settings: serde_json::Value,
}

/// A named set of settings: what it changes from the global settings, as `snapshot` writes them.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct SettingsProfile {
    pub id: String,
    pub name: String,
    pub overrides: serde_json::Value,
}

impl Settings {
    /// The engine for a pass around the pointer or over the whole screen.
    /// Whether either engine reads with MeikiOCR's recognizer (the custom one may).
    pub fn wants_meiki(&self) -> bool {
        self.reads_with(Recognizer::Meiki)
    }

    /// Whether the pointer's or the whole screen's engine reads with `recognizer`.
    pub fn reads_with(&self, recognizer: Recognizer) -> bool {
        [self.engine_for(false), self.engine_for(true)].into_iter().any(|k| self.recognizer_for(k) == recognizer)
    }

    /// The engines a lookup runs Lookitan's models for: the selectors' local engines, and the
    /// pointer's page engine in the automatic capture, whose detector finds the block under the
    /// pointer.
    pub fn engines_in_use(&self) -> Vec<OcrEngineKind> {
        let mut kinds = Vec::new();
        for (kind, whole_screen) in [(self.engine_for(false), false), (self.engine_for(true), true)] {
            if (kind.is_local() || (!whole_screen && self.capture_auto)) && !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
        kinds
    }

    /// Whether an engine in use finds its lines (or its block) with `det`.
    pub fn detects_with(&self, det: Detector) -> bool {
        self.engines_in_use().into_iter().any(|k| self.models(k).det == det)
    }

    /// What `kind`'s pass loads of PP-OCR, if anything: its detector, its recognizer or both
    /// (`ensure_paddle`). Nothing else of PP-OCR is loaded: no engine falls back to it.
    pub fn paddle_use(&self, kind: OcrEngineKind) -> Option<crate::ocr::paddle::PaddleUse> {
        let m = self.models(kind);
        let det = m.det.paddle_model().is_some();
        let rec = match m.rec {
            Recognizer::Paddle => Some(self.rec_model()),
            Recognizer::Manga => Some(crate::ocr::paddle::MANGA_REC),
            Recognizer::Meiki | Recognizer::Indra | Recognizer::Page => None,
        };
        if !det && rec.is_none() {
            return None;
        }
        let mut plan = self.plan_for(kind);
        // A part not loaded does not tell two uses apart.
        if !det {
            (plan.det, plan.det_gpu) = ("det_small", false);
        }
        if rec.is_none() {
            plan.rec_gpu = false;
        }
        Some(crate::ocr::paddle::PaddleUse { det, rec, plan })
    }

    /// The PP-OCR uses the engines in use need, once each; any other loaded one is unused.
    pub fn paddle_configs(&self) -> Vec<crate::ocr::paddle::PaddleUse> {
        let mut configs = Vec::new();
        for kind in self.engines_in_use() {
            if let Some(config) = self.paddle_use(kind) {
                if !configs.contains(&config) {
                    configs.push(config);
                }
            }
        }
        configs
    }

    /// The models `kind` reads with: those chosen for it (`ocr_models`; the custom engine's start
    /// from `ModelChoice::default`), or its own: IndraOCR's LineNet and recognizer, PP-OCRv6
    /// Manga's detector and recognizer, MeikiOCR's 960×544 detector and recognizer (since
    /// 2026-10-30), PP-OCRv6's detector as Lookupper picks it for the others
    /// (`Models::automatic`), each where `use_gpu` says. A detector the engine does not offer (`OcrEngineKind::offers`) reads as its own; only
    /// the custom engine chooses its recognizer. The GPU only where OCR has one (Windows, Linux).
    pub fn models(&self, kind: OcrEngineKind) -> Models {
        let gpu = cfg!(any(windows, target_os = "linux"));
        let own_rec = match kind {
            OcrEngineKind::Meiki => Recognizer::Meiki,
            OcrEngineKind::Indra => Recognizer::Indra,
            OcrEngineKind::Manga => Recognizer::Manga,
            OcrEngineKind::Paddle | OcrEngineKind::Custom | OcrEngineKind::CustomScreen => Recognizer::Paddle,
            OcrEngineKind::Lens | OcrEngineKind::ScreenAi | OcrEngineKind::OneOcr | OcrEngineKind::Bing => Recognizer::Page,
        };
        let on_gpu = gpu && self.use_gpu;
        let own = Models {
            det: match kind {
                OcrEngineKind::Indra => Detector::LineNet,
                OcrEngineKind::Manga => Detector::MangaDet,
                OcrEngineKind::Meiki => Detector::MeikiDet,
                _ if on_gpu => Detector::PaddleSmall,
                _ => Detector::PaddleTiny,
            },
            det_gpu: on_gpu,
            rec: own_rec,
            rec_gpu: on_gpu,
            automatic: !matches!(kind, OcrEngineKind::Indra | OcrEngineKind::Manga | OcrEngineKind::Meiki),
        };
        let custom = matches!(kind, OcrEngineKind::Custom | OcrEngineKind::CustomScreen);
        // The whole screen's custom mix starts as the pointer's.
        let saved = self.ocr_models.get(kind.id()).or_else(|| if kind == OcrEngineKind::CustomScreen { self.ocr_models.get("custom") } else { None });
        let chosen = match saved {
            Some(c) => c.clone(),
            None if custom => ModelChoice::default(),
            None => return own,
        };
        let rec = match (custom, chosen.rec.as_str()) {
            (true, "meiki") => Recognizer::Meiki,
            (true, "indra") => Recognizer::Indra,
            (true, "manga") => Recognizer::Manga,
            _ => own_rec,
        };
        Models {
            det: Detector::of(&chosen.det).filter(|&d| kind.offers(d)).unwrap_or(own.det),
            det_gpu: gpu && chosen.det_gpu,
            rec,
            rec_gpu: gpu && chosen.rec_gpu,
            automatic: false,
        }
    }

    /// What reads `kind`'s text.
    pub fn recognizer_for(&self, kind: OcrEngineKind) -> Recognizer {
        self.models(kind).rec
    }

    /// Whether either engine is Chrome ScreenAI.
    pub fn wants_screenai(&self) -> bool {
        self.ocr_engine == OcrEngineKind::ScreenAi || self.screen_ocr_engine == Some(OcrEngineKind::ScreenAi)
    }

    /// Whether either engine is `kind`.
    pub fn wants(&self, kind: OcrEngineKind) -> bool {
        self.ocr_engine == kind || self.screen_ocr_engine == Some(kind)
    }

    pub fn engine_for(&self, whole_screen: bool) -> OcrEngineKind {
        match (whole_screen, self.screen_ocr_engine.unwrap_or(self.ocr_engine)) {
            (false, _) => self.ocr_engine,
            (true, OcrEngineKind::Custom) => OcrEngineKind::CustomScreen,
            (true, kind) => kind,
        }
    }

    /// The card OCR asks for, or `None` to leave the choice to the GPU API.
    pub fn gpu_choice(&self) -> Option<GpuPreference> {
        self.choose_gpu.then_some(self.gpu_preference)
    }

    /// The OCR language's PP-OCR recognition model (`ocr::rec_model`): its own, or Small for a
    /// language on Tiny (`small_recognizer`), or the experimental one (`experimental_recognizer`).
    pub fn rec_model(&self) -> &'static str {
        crate::ocr::rec_model(&self.ocr_language, &self.small_recognizer, &self.experimental_recognizer)
    }

    /// PP-OCRv6's plan (`plan_for`).
    pub fn engine_plan(&self) -> crate::ocr::paddle::EnginePlan {
        self.plan_for(OcrEngineKind::Paddle)
    }

    /// Which of PP-OCR's detectors runs where for `kind`, and where PP-OCR's recognizer runs
    /// (`models`): Lookupper's choice while none is chosen, else the one chosen.
    pub fn plan_for(&self, kind: OcrEngineKind) -> crate::ocr::paddle::EnginePlan {
        let m = self.models(kind);
        if m.automatic {
            return crate::ocr::paddle::EnginePlan::automatic(m.det_gpu, self.gpu_choice());
        }
        crate::ocr::paddle::EnginePlan {
            manual: true,
            det: m.det.paddle_model().unwrap_or("det_small"),
            det_gpu: m.det_gpu,
            rec_gpu: m.rec_gpu,
            gpu_choice: self.gpu_choice(),
        }
    }

    /// Where `kind`'s recognizer runs: its own choice, or where the detector is in Lookupper's.
    pub fn rec_gpu_for(&self, kind: OcrEngineKind) -> bool {
        let m = self.models(kind);
        if m.automatic { m.det_gpu } else { m.rec_gpu }
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
            small_recognizer: Vec::new(),
            experimental_recognizer: Vec::new(),
            use_gpu: true,
            ocr_manual: None,
            manual_det: "det_small".into(),
            manual_det_gpu: true,
            manual_rec_gpu: true,
            ocr_models: Default::default(),
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
            settings_profiles: Vec::new(),
            active_settings_profile: String::new(),
            global_settings: serde_json::Value::Null,
        }
    }
}

// ------------------------------------------------------------------ settings profiles
//
// The settings in use are always those of the profile chosen (or the global ones): the rest of
// the app reads them as before. Before each save, `sync_profile` keeps what they change from the
// global settings in the profile in use (or, with Global in use, keeps them as the global
// settings). `choose_profile` lays another profile's changes on the global settings.

/// What a profile never holds: the profiles themselves, and the app profiles, which Lookitan adds
/// on its own for every app used.
const NOT_IN_PROFILE: [&str; 4] = ["settingsProfiles", "activeSettingsProfile", "globalSettings", "profiles"];

/// The settings a profile compares: every field but those, with the dictionaries as their ids
/// and switches in order (their files are the same in every profile).
fn snapshot(s: &Settings) -> serde_json::Value {
    let mut value = serde_json::to_value(s).unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::Object(map) = &mut value {
        for key in NOT_IN_PROFILE {
            map.remove(key);
        }
        let dictionaries = s.dictionaries.iter().map(|d| serde_json::json!({"id": d.id, "enabled": d.enabled})).collect();
        map.insert("dictionaries".into(), serde_json::Value::Array(dictionaries));
    }
    value
}

/// What `live` changes from `base`, key by key into objects.
fn changes(live: &serde_json::Value, base: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let serde_json::Value::Object(map) = live {
        for (key, value) in map {
            match (value, base.get(key)) {
                (serde_json::Value::Object(_), Some(inner @ serde_json::Value::Object(_))) => {
                    let inner = changes(value, inner);
                    if inner.as_object().is_some_and(|m| !m.is_empty()) {
                        out.insert(key.clone(), inner);
                    }
                }
                (value, other) if other != Some(value) => {
                    out.insert(key.clone(), value.clone());
                }
                _ => {}
            }
        }
    }
    serde_json::Value::Object(out)
}

/// `base` with `over` laid on it, objects merged key by key.
fn layered(base: &serde_json::Value, over: &serde_json::Value) -> serde_json::Value {
    let mut out = base.clone();
    if let (serde_json::Value::Object(target), serde_json::Value::Object(over)) = (&mut out, over) {
        for (key, value) in over {
            let merged = match target.get(key) {
                Some(inner @ serde_json::Value::Object(_)) if value.is_object() => layered(inner, value),
                _ => value.clone(),
            };
            target.insert(key.clone(), merged);
        }
    }
    out
}

/// The installed dictionaries in the order and with the switches of `order` (`{id, enabled}`);
/// the ones it does not name keep their place after them.
fn arranged(installed: &[DictionaryConfig], order: Option<&serde_json::Value>) -> Vec<DictionaryConfig> {
    let wanted = order.and_then(|v| v.as_array()).map(Vec::as_slice).unwrap_or_default();
    let mut out: Vec<DictionaryConfig> = Vec::new();
    for entry in wanted {
        let (Some(id), Some(enabled)) = (entry.get("id").and_then(|v| v.as_str()), entry.get("enabled").and_then(|v| v.as_bool())) else { continue };
        if let Some(d) = installed.iter().find(|d| d.id == id && !out.iter().any(|o| o.id == id)) {
            out.push(DictionaryConfig { enabled, ..d.clone() });
        }
    }
    for d in installed {
        if !out.iter().any(|o| o.id == d.id) {
            out.push(d.clone());
        }
    }
    out
}

/// Keeps what the settings in use change in the profile in use, or, with Global in use, keeps
/// them as the global settings. Runs before each save.
pub fn sync_profile(s: &mut Settings) {
    let live = snapshot(s);
    match s.settings_profiles.iter().position(|p| p.id == s.active_settings_profile) {
        Some(i) => {
            if s.global_settings.is_null() {
                s.global_settings = live.clone();
            }
            s.settings_profiles[i].overrides = changes(&live, &s.global_settings);
        }
        None => {
            s.active_settings_profile.clear();
            s.global_settings = live;
        }
    }
}

/// `s` switched to the profile `id` (empty or unknown: Global). What the settings in use change
/// stays in the profile (or Global) they leave.
pub fn choose_profile(mut s: Settings, id: &str) -> Settings {
    sync_profile(&mut s);
    let profile = s.settings_profiles.iter().find(|p| p.id == id && !id.is_empty());
    let target = match profile {
        Some(p) => layered(&s.global_settings, &p.overrides),
        None => s.global_settings.clone(),
    };
    let active = profile.map(|p| p.id.clone()).unwrap_or_default();
    let mut next = serde_json::from_value::<Settings>(target.clone()).map(migrate).unwrap_or_else(|e| {
        log::warn!("profile settings could not be read ({e}); keeping the settings in use");
        s.clone()
    });
    next.dictionaries = arranged(&s.dictionaries, target.get("dictionaries"));
    next.profiles = s.profiles;
    next.settings_profiles = s.settings_profiles;
    next.global_settings = s.global_settings;
    next.active_settings_profile = active;
    next
}

/// `s` with a new profile named `name`, in use, that starts from the global settings.
pub fn create_profile(s: Settings, name: &str) -> (Settings, String) {
    let mut s = s;
    sync_profile(&mut s);
    let mut n = s.settings_profiles.len() + 1;
    let id = loop {
        let id = format!("p{n}");
        if !s.settings_profiles.iter().any(|p| p.id == id) { break id; }
        n += 1;
    };
    let name = if name.trim().is_empty() { format!("Perfil {}", s.settings_profiles.len() + 1) } else { name.trim().to_string() };
    s.settings_profiles.push(SettingsProfile { id: id.clone(), name, overrides: serde_json::json!({}) });
    (choose_profile(s, &id), id)
}

/// `s` without the profile `id`; Global is used if it was in use.
pub fn delete_profile(s: Settings, id: &str) -> Settings {
    let mut s = if s.active_settings_profile == id { choose_profile(s, "") } else { s };
    s.settings_profiles.retain(|p| p.id != id);
    s
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
    // Each engine has its own models (2026-10-29): the manual mode's PP-OCRv6 choices become
    // PP-OCRv6's own; out of it, the engines' choices and the recognizer switches were not read,
    // and are dropped.
    if let Some(manual) = s.ocr_manual.take() {
        if manual {
            if !s.ocr_models.contains_key("paddle") {
                let det = if s.manual_det == "det_tiny" { Detector::PaddleTiny } else { Detector::PaddleSmall };
                let choice = ModelChoice { det: det.id().into(), det_gpu: s.manual_det_gpu, rec: "paddle".into(), rec_gpu: s.manual_rec_gpu };
                s.ocr_models.insert("paddle".into(), choice);
            }
        } else {
            for kind in ["paddle", "meiki", "indra"] {
                s.ocr_models.remove(kind);
            }
            s.small_recognizer.clear();
            s.experimental_recognizer.clear();
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
    // The profile in use (or Global) keeps what the settings in use change.
    let mut settings = settings.clone();
    sync_profile(&mut settings);
    std::fs::write(&tmp, serde_json::to_vec_pretty(&settings)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary(id: &str, enabled: bool) -> DictionaryConfig {
        DictionaryConfig { id: id.into(), kind: DictionaryKind::Mdict, path: format!("{id}.mdx"), name: id.into(), enabled }
    }

    #[test]
    fn a_profile_keeps_only_what_it_changes_and_global_comes_back_as_it_was() {
        let mut s = Settings { ocr_language: "en-US".into(), hotkey: "Alt+D".into(), ..Default::default() };
        s.dictionaries.push(dictionary("jmdict", false));
        s.dictionaries.push(dictionary("oxford", true));
        s.ai = serde_json::json!({"provider": "openai", "openai": {"apiKey": "test-only-key", "model": "gpt-x"}});
        let (mut s, id) = create_profile(s, "Japonês");
        assert_eq!(s.active_settings_profile, id);
        assert_eq!(s.ocr_language, "en-US", "a new profile starts from the global settings");
        // Edited in the profile, as the front end saves it.
        s.ocr_language = "ja-JP".into();
        s.ai["openai"]["model"] = "gpt-ja".into();
        s.dictionaries.iter_mut().find(|d| d.id == "jmdict").unwrap().enabled = true;
        sync_profile(&mut s);
        let overrides = &s.settings_profiles[0].overrides;
        assert_eq!(overrides["ocrLanguage"], "ja-JP");
        assert_eq!(overrides["ai"], serde_json::json!({"openai": {"model": "gpt-ja"}}), "only what changed, key by key");
        assert!(overrides.get("hotkey").is_none());
        let global = choose_profile(s, "");
        assert_eq!(global.active_settings_profile, "");
        assert_eq!(global.ocr_language, "en-US");
        assert_eq!(global.ai["openai"]["model"], "gpt-x");
        assert!(!global.dictionaries.iter().find(|d| d.id == "jmdict").unwrap().enabled, "the dictionaries' switches come back too");
        // A change in Global reaches the profile where the profile leaves it alone.
        let mut global = global;
        global.hotkey = "Alt+Q".into();
        let ja = choose_profile(global, &id);
        assert_eq!((ja.ocr_language.as_str(), ja.hotkey.as_str()), ("ja-JP", "Alt+Q"));
        assert_eq!(ja.ai["openai"]["apiKey"], "test-only-key");
        assert!(ja.dictionaries.iter().find(|d| d.id == "jmdict").unwrap().enabled);
        let back = delete_profile(ja, &id);
        assert!(back.settings_profiles.is_empty() && back.active_settings_profile.is_empty());
        assert_eq!(back.ocr_language, "en-US", "deleting the profile in use goes back to Global");
    }

    #[test]
    fn app_profiles_and_new_dictionaries_are_the_same_in_every_profile() {
        let (mut s, id) = create_profile(Settings::default(), "");
        assert_eq!(s.settings_profiles[0].name, "Perfil 1");
        s.profiles.push(AppProfile { executable: "game.exe".into(), ..Default::default() });
        s.dictionaries.push(dictionary("new", true));
        let global = choose_profile(s, "");
        assert_eq!(global.profiles.len(), 1, "app profiles are not part of a profile");
        assert!(global.dictionaries.iter().any(|d| d.id == "new"), "a dictionary installed in a profile is installed for all");
        let again = choose_profile(global, &id);
        assert_eq!(again.profiles.len(), 1);
    }

    #[test]
    fn an_unknown_ocr_engine_reads_as_paddle_and_keeps_the_other_settings() {
        let saved: Settings = serde_json::from_str(r#"{"ocrEngine": "someday", "screenOcrEngine": "lens", "ocrLanguage": "ja-JP"}"#).unwrap();
        assert_eq!(saved.ocr_engine, OcrEngineKind::Paddle);
        assert_eq!(saved.screen_ocr_engine, Some(OcrEngineKind::Lens));
        assert_eq!(saved.ocr_language, "ja-JP");
        assert_eq!(serde_json::to_value(OcrEngineKind::Lens).unwrap(), "lens");
        let meiki: Settings = serde_json::from_str(r#"{"screenOcrEngine": "meiki"}"#).unwrap();
        assert!(meiki.wants_meiki() && meiki.engine_for(true) == OcrEngineKind::Meiki && meiki.engine_for(false) == OcrEngineKind::Paddle);
        let screenai: Settings = serde_json::from_str(r#"{"ocrEngine": "screenai"}"#).unwrap();
        assert!(screenai.wants_screenai() && !screenai.wants_meiki());
        assert_eq!(serde_json::to_value(OcrEngineKind::ScreenAi).unwrap(), "screenai");
        let oneocr: Settings = serde_json::from_str(r#"{"screenOcrEngine": "oneocr"}"#).unwrap();
        assert!(oneocr.wants(OcrEngineKind::OneOcr) && !oneocr.wants(OcrEngineKind::Lens));
        assert_eq!(serde_json::to_value(OcrEngineKind::OneOcr).unwrap(), "oneocr");
        assert_eq!(serde_json::from_str::<OcrEngineKind>(r#""bing""#).unwrap(), OcrEngineKind::Bing);
        assert_eq!(serde_json::from_str::<OcrEngineKind>(r#""indra""#).unwrap(), OcrEngineKind::Indra);
    }

    #[test]
    fn gamepad_actions_saved_as_one_button_read_as_a_list() {
        let saved: Settings = serde_json::from_str(r#"{"gamepad": {"confirm": "RB", "close": ["LB", "B"], "save": ""}}"#).unwrap();
        assert_eq!(saved.gamepad.confirm, vec!["RB"]);
        assert_eq!(saved.gamepad.close, vec!["LB", "B"]);
        assert!(saved.gamepad.save.is_empty());
        assert_eq!(saved.gamepad.explain, vec!["RB"], "missing actions take their default");
        assert_eq!(saved.gamepad.anki, vec!["LB"]);
        assert!(saved.gamepad.meme.is_empty());
    }

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
    fn an_engine_without_chosen_models_reads_with_its_own() {
        let gpu = cfg!(any(windows, target_os = "linux"));
        let old: Settings = serde_json::from_str(r#"{"useGpu": true}"#).unwrap();
        assert_eq!(old.engine_plan(), crate::ocr::paddle::EnginePlan::automatic(gpu, None), "PP-OCRv6: Lookupper's choice");
        assert_eq!(old.models(OcrEngineKind::Paddle).det, if gpu { Detector::PaddleSmall } else { Detector::PaddleTiny });
        let indra = old.models(OcrEngineKind::Indra);
        assert_eq!((indra.det, indra.rec, indra.automatic), (Detector::LineNet, Recognizer::Indra, false), "IndraOCR: its own detector");
        assert_eq!((indra.det_gpu, indra.rec_gpu), (gpu, gpu));
        let cpu: Settings = serde_json::from_str(r#"{"useGpu": false}"#).unwrap();
        assert_eq!(cpu.models(OcrEngineKind::Paddle).det, Detector::PaddleTiny, "on the CPU, the tiny one");
        assert!(!cpu.models(OcrEngineKind::Indra).det_gpu);
        let meiki = old.models(OcrEngineKind::Meiki);
        assert_eq!((meiki.det, meiki.rec, meiki.automatic), (Detector::MeikiDet, Recognizer::Meiki, false), "MeikiOCR: its own detector");
        assert_eq!(old.paddle_use(OcrEngineKind::Meiki), None, "no PP-OCR for it");
        assert_eq!(old.recognizer_for(OcrEngineKind::OneOcr), Recognizer::Page);
    }

    #[test]
    fn the_manual_mode_s_choices_become_pp_ocrv6_s_own() {
        let gpu = cfg!(any(windows, target_os = "linux"));
        let manual = migrate(serde_json::from_str(r#"{"ocrManual": true, "manualDet": "det_tiny", "manualDetGpu": false, "manualRecGpu": true, "useGpu": false,
            "smallRecognizer": ["pt-BR"], "ocrModels": {"indra": {"det": "paddle:det_small", "detGpu": true, "rec": "indra", "recGpu": false}}}"#).unwrap());
        assert_eq!(manual.ocr_manual, None);
        let plan = manual.engine_plan();
        assert!(plan.manual);
        assert_eq!(plan.det, "det_tiny");
        assert!(!plan.det_gpu, "the detector on the CPU");
        assert_eq!(plan.rec_gpu, gpu, "the recognizer on the GPU, though useGpu is off");
        assert_eq!(manual.small_recognizer, ["pt-BR"], "its recognizer switches are kept");
        let indra = manual.models(OcrEngineKind::Indra);
        assert_eq!(indra.det, Detector::LineNet, "a PP-OCRv6 detector chosen for IndraOCR reads as its own");
        assert!(!indra.rec_gpu);
        let saved = serde_json::to_string(&manual).unwrap();
        assert!(!saved.contains("ocrManual") && !saved.contains("manualDet"), "{saved}");
        let odd = migrate(serde_json::from_str(r#"{"ocrManual": true, "manualDet": "det_huge"}"#).unwrap());
        assert_eq!(odd.engine_plan().det, "det_small", "an unknown detector is the small one");
        // Out of the manual mode, nothing chosen was read.
        let auto = migrate(serde_json::from_str(r#"{"ocrManual": false, "smallRecognizer": ["pt-BR"], "ocrLanguage": "pt-BR",
            "ocrModels": {"indra": {"det": "indra:linenet", "detGpu": false}, "custom": {"det": "paddle:det_tiny"}}}"#).unwrap());
        assert_eq!(auto.rec_model(), "rec_tiny");
        assert_eq!(auto.ocr_models.keys().collect::<Vec<_>>(), ["custom"], "the custom engine's were always read");
        assert_eq!(auto.engine_plan(), crate::ocr::paddle::EnginePlan::automatic(gpu, None));
    }

    #[test]
    fn each_engine_has_its_own_models_and_the_custom_one_mixes_them() {
        use crate::ocr::paddle::PaddleUse;
        let gpu = cfg!(any(windows, target_os = "linux"));
        let mut s: Settings = serde_json::from_str(r#"{"ocrEngine": "meiki", "ocrLanguage": "ja-JP"}"#).unwrap();
        assert!(s.wants_meiki());
        s.ocr_models.insert("meiki".into(), ModelChoice { det: "paddle:det_small".into(), det_gpu: true, rec: "meiki".into(), rec_gpu: false });
        s.ocr_models.insert("paddle".into(), ModelChoice { det: "paddle:det_tiny".into(), det_gpu: true, rec: "paddle".into(), rec_gpu: true });
        assert_eq!((s.plan_for(OcrEngineKind::Meiki).det, s.engine_plan().det), ("det_small", "det_tiny"));
        assert!(!s.rec_gpu_for(OcrEngineKind::Meiki), "MeikiOCR's recognizer on the CPU");
        s.ocr_models.get_mut("meiki").unwrap().det = "meiki:det320".into();
        assert_eq!(s.models(OcrEngineKind::Meiki).det, Detector::MeikiDetSmall);
        assert!(s.detects_with(Detector::MeikiDetSmall) && !s.detects_with(Detector::MeikiDet));
        s.ocr_models.get_mut("meiki").unwrap().det = "indra:linenet".into();
        assert_eq!(s.models(OcrEngineKind::Meiki).det, Detector::MeikiDet, "MeikiOCR offers PP-OCRv6's detectors and its own; another reads as its own");
        // The custom engine: any detector with any recognizer.
        s.ocr_engine = OcrEngineKind::Custom;
        assert_eq!(s.recognizer_for(OcrEngineKind::Custom), Recognizer::Paddle, "never set up: PP-OCR's recognizer");
        s.ocr_models.insert("custom".into(), ModelChoice { det: "paddle:det_tiny".into(), det_gpu: false, rec: "indra".into(), rec_gpu: true });
        let plan = s.plan_for(OcrEngineKind::Custom);
        assert!(plan.manual && plan.det == "det_tiny" && !plan.det_gpu && plan.rec_gpu == gpu);
        assert_eq!(s.recognizer_for(OcrEngineKind::Custom), Recognizer::Indra);
        assert!(!s.wants_meiki());
        assert_eq!(s.paddle_configs(), vec![PaddleUse { det: true, rec: None, plan: crate::ocr::paddle::EnginePlan { rec_gpu: false, ..plan } }],
            "PP-OCR's detector alone: IndraOCR reads");
        s.ocr_models.get_mut("custom").unwrap().det = "indra:linenet".into();
        assert!(s.paddle_configs().is_empty(), "no PP-OCR at all");
        assert!(s.detects_with(Detector::LineNet) && s.reads_with(Recognizer::Indra));
        s.ocr_models.get_mut("custom").unwrap().rec = "paddle".into();
        assert_eq!(s.paddle_configs().iter().map(|u| (u.det, u.rec)).collect::<Vec<_>>(), [(false, Some("rec_small"))], "its recognizer alone");
        s.ocr_models.get_mut("custom").unwrap().rec = "meiki".into();
        assert!(s.wants_meiki(), "the custom engine reads with MeikiOCR");
        let again: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!((again.ocr_engine, again.ocr_models.clone()), (OcrEngineKind::Custom, s.ocr_models.clone()));
        // A page engine loads PP-OCR's detector only to find the pointer's block in the automatic
        // capture, never to read in its place.
        s.ocr_engine = OcrEngineKind::OneOcr;
        s.screen_ocr_engine = Some(OcrEngineKind::ScreenAi);
        assert_eq!(s.paddle_configs().iter().map(|u| (u.det, u.rec)).collect::<Vec<_>>(), [(true, None)]);
        s.capture_auto = false;
        assert!(s.paddle_configs().is_empty(), "a fixed region needs no detector");
        s.ocr_engine = OcrEngineKind::Paddle;
        s.screen_ocr_engine = None;
        assert_eq!(s.paddle_configs().len(), 1, "one, when both selectors read as PP-OCRv6");
        assert!(!s.reads_with(Recognizer::Meiki) && !s.reads_with(Recognizer::Indra) && !s.detects_with(Detector::LineNet));
        s.screen_ocr_engine = Some(OcrEngineKind::Indra);
        assert!(s.reads_with(Recognizer::Indra) && s.detects_with(Detector::LineNet));
        assert_eq!(serde_json::from_str::<OcrEngineKind>(r#""custom""#).unwrap(), OcrEngineKind::Custom);
    }

    #[test]
    fn the_custom_engine_has_a_mix_for_each_selector_the_whole_screen_s_starting_as_the_pointer_s() {
        let mut s: Settings = serde_json::from_str(r#"{"ocrEngine": "custom", "ocrLanguage": "ja-JP"}"#).unwrap();
        assert_eq!((s.engine_for(false), s.engine_for(true)), (OcrEngineKind::Custom, OcrEngineKind::CustomScreen));
        s.ocr_models.insert("custom".into(), ModelChoice { det: "manga:det".into(), det_gpu: true, rec: "meiki".into(), rec_gpu: true });
        assert_eq!(s.models(OcrEngineKind::CustomScreen), s.models(OcrEngineKind::Custom), "never set: the pointer's mix");
        assert!(s.wants_meiki() && !s.reads_with(Recognizer::Indra));
        s.ocr_models.insert("customScreen".into(), ModelChoice { det: "paddle:det_small".into(), det_gpu: true, rec: "indra".into(), rec_gpu: false });
        assert_eq!((s.models(OcrEngineKind::Custom).det, s.recognizer_for(OcrEngineKind::Custom)), (Detector::MangaDet, Recognizer::Meiki));
        assert_eq!((s.models(OcrEngineKind::CustomScreen).det, s.recognizer_for(OcrEngineKind::CustomScreen)), (Detector::PaddleSmall, Recognizer::Indra));
        assert!(s.wants_meiki() && s.reads_with(Recognizer::Indra), "each selector's recognizer is in use");
        // Another engine over the whole screen: its mix is unused, and so are its models.
        s.screen_ocr_engine = Some(OcrEngineKind::Paddle);
        assert_eq!(s.engine_for(true), OcrEngineKind::Paddle);
        assert!(!s.reads_with(Recognizer::Indra));
        let again: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!((again.ocr_engine, again.ocr_models.clone()), (OcrEngineKind::Custom, s.ocr_models.clone()));
    }

    #[test]
    fn pp_ocrv6_manga_reads_with_its_own_models_or_comic_text_detector() {
        use crate::ocr::paddle::{MANGA_DET, MANGA_REC};
        let mut s: Settings = serde_json::from_str(r#"{"ocrEngine": "manga", "ocrLanguage": "ja-JP"}"#).unwrap();
        let m = s.models(OcrEngineKind::Manga);
        assert_eq!((m.det, m.rec, m.automatic), (Detector::MangaDet, Recognizer::Manga, false));
        let parts = s.paddle_configs();
        assert_eq!(parts.iter().map(|u| (u.det, u.rec, u.plan.det)).collect::<Vec<_>>(), [(true, Some(MANGA_REC), MANGA_DET)]);
        s.ocr_models.insert("manga".into(), ModelChoice { det: "ctd:comic".into(), det_gpu: true, rec: "manga".into(), rec_gpu: true });
        assert!(s.detects_with(Detector::Ctd));
        assert_eq!(s.paddle_configs().iter().map(|u| (u.det, u.rec)).collect::<Vec<_>>(), [(false, Some(MANGA_REC))], "its recognizer alone");
        s.ocr_models.get_mut("manga").unwrap().det = "paddle:det_small".into();
        assert_eq!(s.models(OcrEngineKind::Manga).det, Detector::MangaDet, "PP-OCRv6's detectors are not its own");
        // The custom engine takes any of them; only a page engine reads by itself.
        s.ocr_engine = OcrEngineKind::Custom;
        s.ocr_models.insert("custom".into(), ModelChoice { det: "ctd:comic".into(), det_gpu: false, rec: "manga".into(), rec_gpu: true });
        assert_eq!((s.models(OcrEngineKind::Custom).det, s.recognizer_for(OcrEngineKind::Custom)), (Detector::Ctd, Recognizer::Manga));
        s.ocr_models.get_mut("custom").unwrap().det = "page:own".into();
        assert_eq!(s.models(OcrEngineKind::Custom).det, s.models(OcrEngineKind::Paddle).det, "a page engine's own reads as PP-OCRv6's");
        assert_eq!(serde_json::from_str::<OcrEngineKind>(r#""manga""#).unwrap(), OcrEngineKind::Manga);
    }

    #[test]
    fn a_page_engine_finds_the_pointer_s_block_with_its_own_reading_when_chosen() {
        let mut s: Settings = serde_json::from_str(r#"{"ocrEngine": "oneocr", "screenOcrEngine": "screenai"}"#).unwrap();
        assert_eq!(s.paddle_configs().len(), 1, "PP-OCRv6's detector finds the block");
        s.ocr_models.insert("oneocr".into(), ModelChoice { det: "page:own".into(), ..ModelChoice::default() });
        assert_eq!(s.models(OcrEngineKind::OneOcr).det, Detector::Page);
        assert!(s.paddle_configs().is_empty(), "nothing of PP-OCR");
    }

    #[test]
    fn gpu_choice_is_off_by_default() {
        let old: Settings = serde_json::from_str(r#"{"useGpu": true}"#).unwrap();
        assert_eq!(old.gpu_choice(), None, "settings saved before the option keep the API default");
        let chosen: Settings = serde_json::from_str(r#"{"chooseGpu": true, "gpuPreference": "lowPower"}"#).unwrap();
        assert_eq!(chosen.gpu_choice(), Some(GpuPreference::LowPower));
    }

    #[test]
    fn only_a_tiny_language_switched_to_small_reads_with_small() {
        let old: Settings = serde_json::from_str(r#"{"ocrLanguage": "pt-BR"}"#).unwrap();
        assert_eq!(old.rec_model(), "rec_tiny", "settings saved before the option keep tiny");
        let mut s: Settings = serde_json::from_str(r#"{"ocrLanguage": "pt-BR", "smallRecognizer": ["pt-br"]}"#).unwrap();
        assert_eq!(s.rec_model(), "rec_small");
        s.ocr_language = "de-DE".into();
        assert_eq!(s.rec_model(), "rec_tiny", "the choice is per language");
        s.small_recognizer = vec!["ar-SA".into(), "ja-JP".into()];
        s.ocr_language = "ar-SA".into();
        assert_eq!(s.rec_model(), "rec_arabic", "a language with its own model keeps it");
        s.ocr_language = "ja-JP".into();
        assert_eq!(s.rec_model(), "rec_small");
    }

    #[test]
    fn russian_reads_with_cyrillic_unless_tiny_ru_is_chosen() {
        let old: Settings = serde_json::from_str(r#"{"ocrLanguage": "ru-RU"}"#).unwrap();
        assert_eq!(old.rec_model(), "rec_cyrillic", "settings saved before the option keep PP-OCRv5 Cyrillic");
        let mut s: Settings = serde_json::from_str(r#"{"ocrLanguage": "ru-RU", "experimentalRecognizer": ["ru-RU"]}"#).unwrap();
        assert_eq!(s.rec_model(), "rec_ru_tiny");
        let again: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(again.rec_model(), "rec_ru_tiny", "the choice survives a save");
        for tag in ["uk-UA", "bg-BG", "sr-Cyrl-RS"] {
            s.ocr_language = tag.into();
            s.experimental_recognizer = vec![tag.into()];
            assert_eq!(s.rec_model(), "rec_cyrillic", "{tag}");
        }
    }
}
