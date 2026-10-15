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

/// Sorted by English name, as the settings list shows them. Every language Lookupper reads, with
/// the same model; regional variants share it and differ only in the speech voice.
pub const LANGUAGES: &[LanguageModel] = &[
    LanguageModel { tag: "ar-SA", name: "Arabic", rec: "rec_arabic" },
    LanguageModel { tag: "bs-Latn-BA", name: "Bosnian (Latin)", rec: "rec_tiny" },
    LanguageModel { tag: "bg-BG", name: "Bulgarian", rec: "rec_cyrillic" },
    LanguageModel { tag: "zh-CN", name: "Chinese (Simplified, China)", rec: "rec_small" },
    LanguageModel { tag: "zh-HK", name: "Chinese (Traditional, Hong Kong)", rec: "rec_small" },
    LanguageModel { tag: "zh-TW", name: "Chinese (Traditional, Taiwan)", rec: "rec_small" },
    LanguageModel { tag: "hr-HR", name: "Croatian", rec: "rec_tiny" },
    LanguageModel { tag: "cs-CZ", name: "Czech", rec: "rec_tiny" },
    LanguageModel { tag: "da-DK", name: "Danish", rec: "rec_tiny" },
    LanguageModel { tag: "nl-NL", name: "Dutch", rec: "rec_tiny" },
    LanguageModel { tag: "en-GB", name: "English (United Kingdom)", rec: "rec_tiny" },
    LanguageModel { tag: "en-US", name: "English (United States)", rec: "rec_tiny" },
    LanguageModel { tag: "fi-FI", name: "Finnish", rec: "rec_tiny" },
    LanguageModel { tag: "fr-CA", name: "French (Canada)", rec: "rec_tiny" },
    LanguageModel { tag: "fr-FR", name: "French (France)", rec: "rec_tiny" },
    LanguageModel { tag: "de-DE", name: "German", rec: "rec_tiny" },
    LanguageModel { tag: "el-GR", name: "Greek", rec: "rec_el" },
    LanguageModel { tag: "hu-HU", name: "Hungarian", rec: "rec_tiny" },
    LanguageModel { tag: "id-ID", name: "Indonesian", rec: "rec_tiny" },
    LanguageModel { tag: "it-IT", name: "Italian", rec: "rec_tiny" },
    LanguageModel { tag: "ja-JP", name: "Japanese", rec: "rec_small" },
    LanguageModel { tag: "ko-KR", name: "Korean", rec: "rec_korean" },
    LanguageModel { tag: "la", name: "Latin", rec: "rec_tiny" },
    LanguageModel { tag: "nb-NO", name: "Norwegian Bokmål", rec: "rec_tiny" },
    LanguageModel { tag: "pl-PL", name: "Polish", rec: "rec_tiny" },
    LanguageModel { tag: "pt-BR", name: "Portuguese (Brazil)", rec: "rec_tiny" },
    LanguageModel { tag: "pt-PT", name: "Portuguese (Portugal)", rec: "rec_tiny" },
    LanguageModel { tag: "ro-RO", name: "Romanian", rec: "rec_small" },
    LanguageModel { tag: "ru-RU", name: "Russian", rec: "rec_cyrillic" },
    LanguageModel { tag: "sr-Cyrl-RS", name: "Serbian (Cyrillic)", rec: "rec_cyrillic" },
    LanguageModel { tag: "sr-Latn-RS", name: "Serbian (Latin)", rec: "rec_tiny" },
    LanguageModel { tag: "sk-SK", name: "Slovak", rec: "rec_tiny" },
    LanguageModel { tag: "sl-SI", name: "Slovenian", rec: "rec_tiny" },
    LanguageModel { tag: "es-MX", name: "Spanish (Mexico)", rec: "rec_tiny" },
    LanguageModel { tag: "es-ES", name: "Spanish (Spain)", rec: "rec_tiny" },
    LanguageModel { tag: "sv-SE", name: "Swedish", rec: "rec_tiny" },
    LanguageModel { tag: "tr-TR", name: "Turkish", rec: "rec_tiny" },
    LanguageModel { tag: "uk-UA", name: "Ukrainian", rec: "rec_cyrillic" },
];

/// The language an unknown tag falls back to.
pub const DEFAULT_LANGUAGE: &str = "en-US";

pub fn language(tag: &str) -> &'static LanguageModel {
    LANGUAGES
        .iter()
        .find(|l| l.tag.eq_ignore_ascii_case(tag))
        .or_else(|| LANGUAGES.iter().find(|l| l.tag == DEFAULT_LANGUAGE))
        .expect("the default language is in the table")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_are_sorted_unique_and_an_unknown_tag_reads_english() {
        let names: Vec<&str> = LANGUAGES.iter().map(|l| l.name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        let mut tags: Vec<String> = LANGUAGES.iter().map(|l| l.tag.to_ascii_lowercase()).collect();
        tags.sort();
        tags.dedup();
        assert_eq!(tags.len(), LANGUAGES.len());
        assert_eq!(language("xx-XX").tag, DEFAULT_LANGUAGE);
        assert_eq!(language("sr-latn-rs").rec, "rec_tiny");
        assert_eq!(language("sr-Cyrl-RS").rec, "rec_cyrillic");
    }
}
