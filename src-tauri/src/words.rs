// Picks the word under the pointer from OCR lines.

use serde::Serialize;

use crate::ocr::{is_unspaced_script, OcrLine};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pick {
    pub word: String,
    /// Forms to look up, best first. For unspaced scripts these are prefixes starting at the pointer.
    pub candidates: Vec<String>,
    /// The sentence holding the word, cut from its paragraph (AI context and Anki's {sentence}).
    pub sentence: String,
    /// Indices of the lines the sentence spans, for the "sentence" screenshot.
    #[serde(skip)]
    pub sentence_lines: Vec<usize>,
    /// Word box in image coordinates.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Pick {
    /// True when the pointer is on the picked word itself, not merely near its line. A shortcut
    /// pressed away from text opens the whole-screen overlay instead of a pop-up.
    pub fn is_under(&self, px: f32, py: f32) -> bool {
        let pad = ((self.y1 - self.y0) * 0.25).max(4.0);
        px >= self.x0 - pad && px <= self.x1 + pad && py >= self.y0 - pad && py <= self.y1 + pad
    }
}

/// A clickable word rectangle for the overlay, in image coordinates.
#[derive(Clone, Debug, Serialize)]
pub struct WordBox {
    pub text: String,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Every word of every line as a rectangle. Runs of unspaced-script characters (CJK, Thai) form
/// one rectangle; a click inside it still picks the longest dictionary match from the click point.
pub fn word_boxes(lines: &[OcrLine]) -> Vec<WordBox> {
    let mut out = Vec::new();
    for line in lines {
        let chars = &line.chars;
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i].ch;
            let unspaced = is_unspaced_script(c);
            if !(c.is_alphanumeric() || unspaced) {
                i += 1;
                continue;
            }
            let start = i;
            i += 1;
            while i < chars.len() {
                let d = chars[i].ch;
                let same = if unspaced { is_unspaced_script(d) } else { is_word_char(d) && !is_unspaced_script(d) };
                if !same {
                    break;
                }
                i += 1;
            }
            let mut end = i;
            while end > start + 1 && !chars[end - 1].ch.is_alphanumeric() && !unspaced {
                end -= 1;
            }
            let text: String = chars[start..end].iter().map(|c| c.ch).collect();
            if !text.trim().is_empty() {
                out.push(WordBox { text, x0: chars[start].x0, y0: line.y0, x1: chars[end - 1].x1, y1: line.y1 });
            }
        }
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '’' || c == '-' || c == '·'
}

pub fn pick(lines: &[OcrLine], px: f32, py: f32) -> Option<Pick> {
    let line_idx = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.chars.is_empty())
        .min_by(|(_, a), (_, b)| dist(a, px, py).total_cmp(&dist(b, px, py)))
        .map(|(i, _)| i)?;
    let line = &lines[line_idx];
    if dist(line, px, py) > (line.y1 - line.y0).max(12.0) * 1.5 {
        return None;
    }

    let chars = &line.chars;
    // Character whose span contains the pointer, else the nearest one.
    let ci = chars
        .iter()
        .position(|c| px >= c.x0 && px <= c.x1)
        .unwrap_or_else(|| {
            chars
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    let da = ((a.x0 + a.x1) / 2.0 - px).abs();
                    let db = ((b.x0 + b.x1) / 2.0 - px).abs();
                    da.total_cmp(&db)
                })
                .map(|(i, _)| i)
                .unwrap_or(0)
        });
    let mut ci = ci;
    // Pointer on a space or punctuation: prefer the word just to its left, then to its right.
    if !is_word_char(chars[ci].ch) && !is_unspaced_script(chars[ci].ch) {
        if ci > 0 && is_word_char(chars[ci - 1].ch) {
            ci -= 1;
        } else if ci + 1 < chars.len() && is_word_char(chars[ci + 1].ch) {
            ci += 1;
        } else {
            return None;
        }
    }

    let (start, end, candidates) = if is_unspaced_script(chars[ci].ch) {
        let mut end = ci;
        while end < chars.len() && end - ci < 16 && (is_unspaced_script(chars[end].ch) || chars[end].ch.is_alphanumeric()) {
            end += 1;
        }
        let run: Vec<char> = chars[ci..end].iter().map(|c| c.ch).collect();
        let candidates: Vec<String> = (1..=run.len()).rev().map(|n| run[..n].iter().collect()).collect();
        (ci, end, candidates)
    } else {
        let mut start = ci;
        while start > 0 && is_word_char(chars[start - 1].ch) {
            start -= 1;
        }
        let mut end = ci + 1;
        while end < chars.len() && is_word_char(chars[end].ch) {
            end += 1;
        }
        // Trim hyphens and apostrophes at the edges ("-word'" -> "word").
        while start < end && !chars[start].ch.is_alphanumeric() {
            start += 1;
        }
        while end > start && !chars[end - 1].ch.is_alphanumeric() {
            end -= 1;
        }
        if start >= end {
            return None;
        }
        let word: String = chars[start..end].iter().map(|c| c.ch).collect();
        let mut candidates = crate::dict::candidate_forms(&word);
        // "well-known" -> also try "well" and "known" if the compound is not in the dictionary.
        if word.contains('-') {
            for part in word.split('-') {
                if !part.is_empty() {
                    candidates.extend(crate::dict::candidate_forms(part));
                }
            }
        }
        (start, end, candidates)
    };
    let unspaced = is_unspaced_script(chars[ci].ch);
    let word: String = if unspaced {
        candidates.first().cloned().unwrap_or_default()
    } else {
        chars[start..end].iter().map(|c| c.ch).collect()
    };
    // For unspaced scripts the run past the pointer is only a guess; the pointer character
    // is enough to find the sentence.
    let span = if unspaced { ci..ci + 1 } else { start..end };
    let (sentence, sentence_lines) = sentence_of(lines, line_idx, span);
    Some(Pick {
        word,
        candidates,
        sentence,
        sentence_lines,
        x0: chars[start].x0,
        y0: line.y0,
        x1: chars[end.saturating_sub(1).max(start)].x1,
        y1: line.y1,
    })
}

fn dist(l: &OcrLine, x: f32, y: f32) -> f32 {
    let dx = if x < l.x0 { l.x0 - x } else if x > l.x1 { x - l.x1 } else { 0.0 };
    let dy = if y < l.y0 { l.y0 - y } else if y > l.y1 { y - l.y1 } else { 0.0 };
    // Vertical distance matters more: a line beside the pointer is rarely the intended one.
    dx + dy * 3.0
}

// Like Lookupper, the context of a word is its paragraph, and its sentence is cut from it at the
// sentence marks (see sentence.rs). Paragraphs are grown from the pointer line.

/// Vertical gap between two lines of one paragraph, in line heights.
const MAX_LINE_GAP: f32 = 1.0;
/// Height ratio above which two lines are different text (a title over its body).
const MAX_HEIGHT_RATIO: f32 = 1.6;
/// Horizontal gap between two pieces of one row (a line the OCR split), in line heights.
const MAX_ROW_GAP: f32 = 1.5;
/// Bounds the paragraph search on a full screen of text.
const MAX_PARAGRAPH_LINES: usize = 60;
/// Longest sentence kept whole. Longer text without sentence marks (menus, a wall of
/// unpunctuated text) falls back to the pointer line and the lines directly around it.
const MAX_SENTENCE_CHARS: usize = 400;

fn height(l: &OcrLine) -> f32 {
    (l.y1 - l.y0).max(4.0)
}

fn same_row(a: &OcrLine, b: &OcrLine) -> bool {
    a.y1.min(b.y1) - a.y0.max(b.y0) >= 0.5 * height(a).min(height(b))
}

/// Whether `b` continues the paragraph of `a`: the line right above or below it in the same
/// column, or another piece of the same row, in a similar text size.
fn joins(a: &OcrLine, b: &OcrLine) -> bool {
    let (ha, hb) = (height(a), height(b));
    if ha.max(hb) > MAX_HEIGHT_RATIO * ha.min(hb) {
        return false;
    }
    let h = (ha + hb) / 2.0;
    if same_row(a, b) {
        return a.x0.max(b.x0) - a.x1.min(b.x1) <= MAX_ROW_GAP * h;
    }
    let gap = (b.y0 - a.y1).max(a.y0 - b.y1);
    let overlap = a.x1.min(b.x1) - a.x0.max(b.x0);
    gap <= MAX_LINE_GAP * h && overlap > 0.0
}

/// The lines of the paragraph holding `idx`, in reading order: rows top to bottom, the pieces of
/// a row left to right.
fn paragraph(lines: &[OcrLine], idx: usize) -> Vec<usize> {
    let mut members = vec![idx];
    let mut seen = vec![false; lines.len()];
    seen[idx] = true;
    let mut k = 0;
    while k < members.len() && members.len() < MAX_PARAGRAPH_LINES {
        let a = &lines[members[k]];
        for (j, b) in lines.iter().enumerate() {
            if !seen[j] && !b.text.trim().is_empty() && joins(a, b) {
                seen[j] = true;
                members.push(j);
            }
        }
        k += 1;
    }
    members.sort_by(|&a, &b| lines[a].y0.total_cmp(&lines[b].y0));
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for m in members {
        match rows.last_mut() {
            Some(row) if same_row(&lines[row[0]], &lines[m]) => row.push(m),
            _ => rows.push(vec![m]),
        }
    }
    rows.into_iter()
        .flat_map(|mut row| {
            row.sort_by(|&a, &b| lines[a].x0.total_cmp(&lines[b].x0));
            row
        })
        .collect()
}

/// Where a line's text sits in the joined paragraph: `len` characters from `at`, starting with
/// the line's character `first` (its leading spaces are dropped).
struct Piece {
    line: usize,
    at: usize,
    first: usize,
    len: usize,
}

/// The paragraph as one text: lines joined with a space (none before an unspaced script), and a
/// word hyphenated across a line break joined again.
fn join(lines: &[OcrLine], order: &[usize]) -> (Vec<char>, Vec<Piece>) {
    let mut text: Vec<char> = Vec::new();
    let mut pieces: Vec<Piece> = Vec::new();
    for &li in order {
        let l = &lines[li];
        let cs: Vec<char> = if l.chars.is_empty() { l.text.chars().collect() } else { l.chars.iter().map(|c| c.ch).collect() };
        let Some(first) = cs.iter().position(|c| !c.is_whitespace()) else { continue };
        let last = cs.iter().rposition(|c| !c.is_whitespace()).map_or(first, |i| i + 1);
        let n = text.len();
        if n >= 2 && text[n - 1] == '-' && text[n - 2].is_alphabetic() && cs[first].is_lowercase() {
            text.pop();
            if let Some(p) = pieces.last_mut() {
                p.len -= 1;
            }
        } else if n > 0 && !is_unspaced_script(cs[first]) {
            text.push(' ');
        }
        pieces.push(Piece { line: li, at: text.len(), first, len: last - first });
        text.extend_from_slice(&cs[first..last]);
    }
    (text, pieces)
}

/// The sentence holding characters `span` of line `idx`, and the lines it spans.
fn sentence_of(lines: &[OcrLine], idx: usize, span: std::ops::Range<usize>) -> (String, Vec<usize>) {
    let order = paragraph(lines, idx);
    let (text, pieces) = join(lines, &order);
    let Some(p) = pieces.iter().find(|p| p.line == idx) else { return (lines[idx].text.trim().to_string(), vec![idx]) };
    let ws = p.at + span.start.saturating_sub(p.first).min(p.len);
    let we = (p.at + span.end.saturating_sub(p.first).min(p.len)).max(ws + 1);
    let ranges = crate::sentence::split(&text);
    let mut hit = ranges.iter().filter(|r| r.start < we && r.end > ws);
    let first = hit.next().cloned().unwrap_or(0..text.len());
    let (mut from, mut to) = (first.start, hit.last().map_or(first.end, |r| r.end));
    if to - from > MAX_SENTENCE_CHARS {
        // No usable sentence marks: the pointer row and the rows right above and below it.
        let h = height(&lines[idx]);
        let near: Vec<&Piece> = pieces
            .iter()
            .filter(|q| {
                let l = &lines[q.line];
                l.y1 >= lines[idx].y0 - MAX_LINE_GAP * h && l.y0 <= lines[idx].y1 + MAX_LINE_GAP * h
            })
            .collect();
        from = near.iter().map(|q| q.at).min().unwrap_or(p.at);
        to = near.iter().map(|q| q.at + q.len).max().unwrap_or(p.at + p.len);
    }
    while from < to && text[from].is_whitespace() {
        from += 1;
    }
    while to > from && text[to - 1].is_whitespace() {
        to -= 1;
    }
    let used = pieces.iter().filter(|q| q.len > 0 && q.at < to && q.at + q.len > from).map(|q| q.line).collect();
    (text[from..to].iter().collect(), used)
}

#[cfg(test)]
mod tests {
    use super::pick;
    use crate::ocr::{OcrChar, OcrLine};

    fn line(text: &str, x0: f32, y0: f32) -> OcrLine {
        let chars: Vec<OcrChar> = text.chars().enumerate()
            .map(|(i, ch)| OcrChar { ch, x0: x0 + i as f32 * 10.0, x1: x0 + i as f32 * 10.0 + 10.0 })
            .collect();
        OcrLine { text: text.into(), x0, y0, x1: x0 + text.len() as f32 * 10.0, y1: y0 + 20.0, chars }
    }

    #[test]
    fn pointer_on_a_word_is_under_it() {
        let lines = [line("hello world", 100.0, 100.0)];
        let p = pick(&lines, 165.0, 110.0).unwrap();
        assert_eq!(p.word, "world");
        assert!(p.is_under(165.0, 110.0));
    }

    #[test]
    fn sentence_is_cut_from_the_paragraph_across_lines() {
        let lines = [
            line("The quick brown fox jumps over the", 100.0, 100.0),
            line("lazy dog. A second sentence starts", 100.0, 125.0),
            line("here and ends here.", 100.0, 150.0),
            line("Another paragraph far below.", 100.0, 260.0),
        ];
        let p = pick(&lines, 145.0, 110.0).unwrap();
        assert_eq!(p.word, "quick");
        assert_eq!(p.sentence, "The quick brown fox jumps over the lazy dog.");
        assert_eq!(p.sentence_lines, [0, 1]);
        let p = pick(&lines, 225.0, 135.0).unwrap();
        assert_eq!(p.word, "second");
        assert_eq!(p.sentence, "A second sentence starts here and ends here.");
        assert_eq!(p.sentence_lines, [1, 2]);
        let p = pick(&lines, 105.0, 270.0).unwrap();
        assert_eq!(p.sentence, "Another paragraph far below.");
        assert_eq!(p.sentence_lines, [3]);
    }

    #[test]
    fn hyphenated_words_rejoin_and_a_bigger_title_stays_apart() {
        let mut title = line("Chapter One", 100.0, 40.0);
        title.y1 = 80.0;
        let lines = [title, line("It was a good exam-", 100.0, 100.0), line("ple of work. Next one.", 100.0, 125.0)];
        let p = pick(&lines, 195.0, 110.0).unwrap();
        assert_eq!(p.word, "good");
        assert_eq!(p.sentence, "It was a good example of work.");
        assert_eq!(p.sentence_lines, [1, 2]);
    }

    #[test]
    fn pieces_of_one_row_are_read_left_to_right() {
        let lines = [line("my friend. Bye.", 240.0, 100.0), line("Hello there,", 100.0, 100.0)];
        let p = pick(&lines, 105.0, 110.0).unwrap();
        assert_eq!(p.word, "Hello");
        assert_eq!(p.sentence, "Hello there, my friend.");
        assert_eq!(p.sentence_lines, [1, 0]);
    }

    #[test]
    fn unspaced_scripts_split_at_their_own_marks() {
        let lines = [line("今日は晴れ。明日は雨。", 100.0, 100.0)];
        let p = pick(&lines, 165.0, 110.0).unwrap();
        assert_eq!(p.sentence, "明日は雨。");
    }

    #[test]
    fn text_without_sentence_marks_falls_back_to_the_lines_around_the_pointer() {
        let lines: Vec<_> = (0..30).map(|i| line("menu item without any marks here", 100.0, 100.0 + i as f32 * 25.0)).collect();
        let p = pick(&lines, 105.0, 100.0 + 15.0 * 25.0 + 10.0).unwrap();
        assert_eq!(p.sentence_lines, [14, 15, 16]);
    }

    #[test]
    fn pointer_near_but_off_the_text_is_not_under_it() {
        let lines = [line("hello world", 100.0, 100.0)];
        // Below the line, within pick's 1.5-line tolerance but clearly off the word.
        let p = pick(&lines, 130.0, 128.0).unwrap();
        assert!(!p.is_under(130.0, 128.0));
        // Past the end of the line.
        let p = pick(&lines, 240.0, 110.0).unwrap();
        assert!(!p.is_under(240.0, 110.0));
    }
}
