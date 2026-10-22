// OCR engines. Both produce lines in the coordinate space of the captured image, with a
// horizontal span for every character so the word under the pointer can be picked.

pub mod lens;
pub mod cache;
pub mod ink;
mod lens_proto;
pub mod paddle;
pub mod quad;

use serde::Serialize;

/// A character's span along its line: `x0..x1` across a horizontal line, and its `y0..y1` in
/// capture pixels on a vertical one (see `Flow`). On a turned line, in the line's frame (`Tilt`).
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
    /// Where the line's ink lies across it (`ink::measure`), tighter than the detected box: its
    /// rows, or the columns of a vertical line. `None` when the engine gives no ink or the crop
    /// had none.
    pub ink_y: Option<(f32, f32)>,
    /// Which way the text runs.
    pub flow: Flow,
    /// For a turned line (quad.rs), its frame: there its characters and ink are given, while
    /// `x0..y1` stay the box around it on the capture.
    pub tilt: Option<Tilt>,
}

/// A turned line's frame: the capture turned by `-angle` (radians) about its origin, where the
/// line stands upright (or, for a column, straight down) in `rect`, `(x0, y0, x1, y1)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Tilt {
    pub angle: f32,
    pub rect: (f32, f32, f32, f32),
}

impl Tilt {
    /// A capture point in this frame.
    pub fn to_frame(&self, x: f32, y: f32) -> (f32, f32) {
        let (s, c) = self.angle.sin_cos();
        (x * c + y * s, -x * s + y * c)
    }

    /// A point of this frame on the capture.
    pub fn to_capture(&self, x: f32, y: f32) -> (f32, f32) {
        let (s, c) = self.angle.sin_cos();
        (x * c - y * s, x * s + y * c)
    }
}

/// Which way a line's text runs on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Flow {
    /// A horizontal line.
    #[default]
    Across,
    /// Top to bottom: a column of CJK text, or text turned a quarter right. Its characters' spans
    /// are along y, going down.
    Down,
    /// Bottom to top: text turned a quarter left, as on many book spines and chart axes. Its
    /// characters' spans are along y, going up.
    Up,
}

impl OcrLine {
    pub fn vertical(&self) -> bool {
        self.flow != Flow::Across
    }
}

/// Whether a text box reads top to bottom: as tall as one and a half times its width, as in
/// PaddleOCR and Lookupper's recognizer, which turn such a crop on its side to read it.
pub fn is_vertical_box(width: f32, height: f32) -> bool {
    height >= 1.5 * width
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

/// Right-to-left letters, as Lookupper's `IsRtlCharacter`: U+0590–U+08FF (Hebrew, Arabic, Syriac,
/// Thaana, N'Ko and more) and the Hebrew and Arabic presentation forms.
pub fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Whether text reads right to left: more right-to-left characters than other letters, as
/// Lookupper decides for a recognized line and for a paragraph.
pub fn is_rtl_text(text: impl IntoIterator<Item = char>) -> bool {
    let (mut rtl, mut other) = (0usize, 0usize);
    for c in text {
        if is_rtl(c) {
            rtl += 1;
        } else if c.is_alphabetic() {
            other += 1;
        }
    }
    rtl > other
}

/// Turns a right-to-left line from the order it is drawn in, left to right, to the order it is
/// read in; the same turn takes it back. The recognizer reads Arabic as drawn, and Lookupper
/// reverses such a line a letter at a time, each letter with its marks. Here, as in PaddleOCR,
/// digits and left-to-right words also keep their own order ("سنة 2024", not "سنة 4202"), and
/// brackets turn with the text.
pub fn reverse_rtl(chars: &[OcrChar]) -> Vec<OcrChar> {
    // Each letter with the marks drawn on it.
    let mut clusters: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, c) in chars.iter().enumerate() {
        match clusters.last_mut() {
            Some(last) if attaches(c.ch) => last.end = i + 1,
            _ => clusters.push(i..i + 1),
        }
    }
    let base = |k: usize| chars[clusters[k].start].ch;
    let ltr = |c: char| c.is_numeric() || (c.is_alphanumeric() && !is_rtl(c));
    let joins = |c: char| matches!(c, ' ' | '.' | ',' | ':' | '/' | '%' | '+' | '-' | '*' | '\u{066B}' | '\u{066C}');
    // Runs of left-to-right text stay whole; any other letter is turned on its own.
    let mut segments: Vec<(std::ops::Range<usize>, bool)> = Vec::new();
    let mut k = 0;
    while k < clusters.len() {
        if !ltr(base(k)) {
            segments.push((clusters[k].clone(), false));
            k += 1;
            continue;
        }
        let mut end = k + 1;
        loop {
            let mut next = end;
            while next < clusters.len() && joins(base(next)) {
                next += 1;
            }
            if next < clusters.len() && ltr(base(next)) {
                end = next + 1;
            } else {
                break;
            }
        }
        segments.push((clusters[k].start..clusters[end - 1].end, true));
        k = end;
    }
    let mut out = Vec::with_capacity(chars.len());
    for (range, run) in segments.into_iter().rev() {
        for (j, c) in chars[range].iter().enumerate() {
            let mut c = c.clone();
            if !run && j == 0 {
                c.ch = mirrored(c.ch);
            }
            out.push(c);
        }
    }
    out
}

/// Characters drawn on the letter before them: combining marks (Arabic harakat, Hebrew points),
/// the zero-width joiners and variation selectors.
fn attaches(c: char) -> bool {
    unicode_normalization::char::is_combining_mark(c)
        || matches!(c, '\u{200C}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}')
}

fn mirrored(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        _ => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawn(text: &str) -> Vec<OcrChar> {
        text.chars().enumerate().map(|(i, ch)| OcrChar { ch, x0: i as f32 * 10.0, x1: i as f32 * 10.0 + 10.0 }).collect()
    }

    fn text(chars: &[OcrChar]) -> String {
        chars.iter().map(|c| c.ch).collect()
    }

    #[test]
    fn a_right_to_left_line_is_read_backwards_keeping_numbers_marks_and_positions() {
        // As drawn, left to right: "2024", a space, then سنة from its last letter.
        let line = drawn("2024 ةنس");
        assert!(is_rtl_text(line.iter().map(|c| c.ch)));
        let read = reverse_rtl(&line);
        assert_eq!(text(&read), "سنة 2024");
        assert_eq!((read[0].x0, read[7].x1), (70.0, 40.0), "each character keeps its place on screen");
        assert_eq!(text(&reverse_rtl(&read)), "2024 ةنس", "the same turn takes it back");
        // كَتَبَ drawn: each letter followed by its fatha, from the last letter.
        let marked = drawn("\u{628}\u{64E}\u{62A}\u{64E}\u{643}\u{64E}");
        assert_eq!(text(&reverse_rtl(&marked)), "\u{643}\u{64E}\u{62A}\u{64E}\u{628}\u{64E}");
        assert_eq!(text(&reverse_rtl(&drawn("(ةملك)"))), "(كلمة)", "brackets turn with the text");
        assert_eq!(text(&reverse_rtl(&drawn("ةحفص 3.14 ىلإ"))), "إلى 3.14 صفحة");
    }

    #[test]
    fn only_lines_mostly_in_right_to_left_letters_read_right_to_left() {
        assert!(!is_rtl_text("hello عالم world".chars()));
        assert!(is_rtl_text("مرحبا hi".chars()));
        assert!(!is_rtl_text("12345".chars()));
    }

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
