// OCR engines. Both produce lines in the coordinate space of the captured image, with a
// horizontal span for every character so the word under the pointer can be picked.

pub mod lens;
pub mod cache;
pub mod ink;
mod lens_proto;
pub mod paddle;

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct OcrChar {
    pub ch: char,
    pub x0: f32,
    pub x1: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct OcrLine {
    pub text: String,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub chars: Vec<OcrChar>,
    /// The rows of the line's ink (`ink::measure`), tighter than the detected box; `None` when
    /// the engine gives no ink or the crop had none.
    pub ink_y: Option<(f32, f32)>,
}

impl OcrLine {
    /// The top and bottom of the line's text: its ink, or its box.
    pub fn text_rows(&self) -> (f32, f32) {
        self.ink_y.unwrap_or((self.y0, self.y1))
    }

    pub fn contains(&self, x: f32, y: f32, slack: f32) -> bool {
        x >= self.x0 - slack && x <= self.x1 + slack && y >= self.y0 - slack && y <= self.y1 + slack
    }
}

/// Recognition model for each OCR language, as in Lookupper's language table.
pub struct LanguageModel {
    pub tag: &'static str,
    pub name: &'static str,
    /// File stem in `models/` of the recognition model and its `.dict.json` character list.
    pub rec: &'static str,
}

pub const LANGUAGES: &[LanguageModel] = &[
    LanguageModel { tag: "en-US", name: "English (US)", rec: "rec_tiny" },
    LanguageModel { tag: "en-GB", name: "English (UK)", rec: "rec_tiny" },
    LanguageModel { tag: "pt-BR", name: "Português (Brasil)", rec: "rec_tiny" },
    LanguageModel { tag: "pt-PT", name: "Português (Portugal)", rec: "rec_tiny" },
    LanguageModel { tag: "es-ES", name: "Español (España)", rec: "rec_tiny" },
    LanguageModel { tag: "es-MX", name: "Español (México)", rec: "rec_tiny" },
    LanguageModel { tag: "fr-FR", name: "Français (France)", rec: "rec_tiny" },
    LanguageModel { tag: "fr-CA", name: "Français (Canada)", rec: "rec_tiny" },
    LanguageModel { tag: "de-DE", name: "Deutsch", rec: "rec_tiny" },
    LanguageModel { tag: "it-IT", name: "Italiano", rec: "rec_tiny" },
    LanguageModel { tag: "nl-NL", name: "Nederlands", rec: "rec_tiny" },
    LanguageModel { tag: "sv-SE", name: "Svenska", rec: "rec_tiny" },
    LanguageModel { tag: "da-DK", name: "Dansk", rec: "rec_tiny" },
    LanguageModel { tag: "nb-NO", name: "Norsk bokmål", rec: "rec_tiny" },
    LanguageModel { tag: "fi-FI", name: "Suomi", rec: "rec_tiny" },
    LanguageModel { tag: "pl-PL", name: "Polski", rec: "rec_tiny" },
    LanguageModel { tag: "cs-CZ", name: "Čeština", rec: "rec_tiny" },
    LanguageModel { tag: "sk-SK", name: "Slovenčina", rec: "rec_tiny" },
    LanguageModel { tag: "hu-HU", name: "Magyar", rec: "rec_tiny" },
    LanguageModel { tag: "hr-HR", name: "Hrvatski", rec: "rec_tiny" },
    LanguageModel { tag: "tr-TR", name: "Türkçe", rec: "rec_tiny" },
    LanguageModel { tag: "ro-RO", name: "Română", rec: "rec_small" },
    LanguageModel { tag: "ja-JP", name: "日本語", rec: "rec_small" },
    LanguageModel { tag: "zh-CN", name: "中文（简体）", rec: "rec_small" },
    LanguageModel { tag: "zh-TW", name: "中文（台灣）", rec: "rec_small" },
    LanguageModel { tag: "zh-HK", name: "中文（香港）", rec: "rec_small" },
    LanguageModel { tag: "ko-KR", name: "한국어", rec: "rec_korean" },
    LanguageModel { tag: "ru-RU", name: "Русский", rec: "rec_cyrillic" },
    LanguageModel { tag: "uk-UA", name: "Українська", rec: "rec_cyrillic" },
    LanguageModel { tag: "bg-BG", name: "Български", rec: "rec_cyrillic" },
    LanguageModel { tag: "ar-SA", name: "العربية", rec: "rec_arabic" },
    LanguageModel { tag: "el-GR", name: "Ελληνικά", rec: "rec_el" },
];

pub fn language(tag: &str) -> &'static LanguageModel {
    LANGUAGES.iter().find(|l| l.tag.eq_ignore_ascii_case(tag)).unwrap_or(&LANGUAGES[0])
}

/// Scripts written without spaces between words; the picker tries the longest dictionary match.
pub fn is_unspaced_script(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // Hiragana, Katakana
        | 0x31F0..=0x31FF // Katakana extensions
        | 0x3400..=0x4DBF // CJK extension A
        | 0x4E00..=0x9FFF // CJK unified ideographs
        | 0xF900..=0xFAFF // CJK compatibility ideographs
        | 0xFF66..=0xFF9F // half-width Katakana
        | 0x0E00..=0x0E7F // Thai
        | 0x0E80..=0x0EFF // Lao
        | 0x1000..=0x109F // Myanmar
        | 0x1780..=0x17FF // Khmer
    )
}
