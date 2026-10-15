// Picks the word under the pointer from OCR lines.

use serde::Serialize;

use crate::ocr::{is_unspaced_script, OcrLine};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pick {
    pub word: String,
    /// Forms to look up, best first. For unspaced scripts these are prefixes starting at the pointer.
    pub candidates: Vec<String>,
    /// The sentence holding the word, cut from its paragraph (context translation and Anki's
    /// {sentence}).
    pub sentence: String,
    /// The paragraph holding the word, as Lookupper shows and sends it to the AI.
    pub context: String,
    /// Where the word starts in `context`, in characters.
    pub context_at: usize,
    /// Indices of the lines the sentence spans, for the "sentence" screenshot.
    #[serde(skip)]
    pub sentence_lines: Vec<usize>,
    /// Word box in image coordinates, snapped to the ink of the line (see ocr::ink).
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// The rows of the word's detected line box, which the pointer test uses.
    #[serde(skip)]
    pub line_y: (f32, f32),
}

impl Pick {
    /// True when the pointer is on the picked word itself, not merely near its line. A shortcut
    /// pressed away from text opens the whole-screen overlay instead of a pop-up.
    pub fn is_under(&self, px: f32, py: f32) -> bool {
        // The test covers the line's detected rows, not only its ink, and reaches about half a
        // space past the ink on each side, as it did before word edges were snapped.
        let (y0, y1) = self.line_y;
        let h = y1 - y0;
        let pad = (h * 0.25).max(4.0);
        let side = (h * 0.4).max(4.0);
        px >= self.x0 - side && px <= self.x1 + side && py >= y0 - pad && py <= y1 + pad
    }
}

/// A clickable word rectangle for the overlay, in image coordinates.
#[derive(Clone, Debug, Serialize)]
pub struct WordBox {
    pub text: String,
    /// The drawn box, close to the ink.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// The area that takes the pointer, `(x0, y0, x1, y1)`: the line's whole detected height,
    /// and sideways halfway to the neighbouring words, so the words of a line are easy to hit.
    pub hit: (f32, f32, f32, f32),
}

/// How far the hit area reaches past the first and last word of a line, in line heights.
const HIT_END: f32 = 0.25;

/// Room left around the ink, as a share of its height, so a word's box is not tight on its
/// letters: above and below, and to the sides.
const INK_PAD_Y: f32 = 0.12;
const INK_PAD_X: f32 = 0.08;

/// The top and bottom of a line's text: its ink with a little room, never past the detected
/// box; or the whole box when there is no ink.
fn text_rows(line: &OcrLine) -> (f32, f32) {
    match line.ink_y {
        Some((a, b)) => {
            let pad = (b - a) * INK_PAD_Y;
            ((a - pad).max(line.y0), (b + pad).min(line.y1))
        }
        None => (line.y0, line.y1),
    }
}

/// How far a word's box reaches past its letters at the sides.
fn pad_x(line: &OcrLine) -> f32 {
    line.ink_y.map_or(0.0, |(a, b)| (b - a) * INK_PAD_X)
}

/// Every word of every line as a rectangle. Runs of unspaced-script characters (CJK, Thai) form
/// one rectangle; a click inside it still picks the longest dictionary match from the click point.
pub fn word_boxes(lines: &[OcrLine]) -> Vec<WordBox> {
    let mut out: Vec<WordBox> = Vec::new();
    for line in lines {
        let chars = &line.chars;
        let first = out.len();
        let pad = pad_x(line);
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
                let (y0, y1) = text_rows(line);
                out.push(WordBox {
                    text,
                    x0: (chars[start].x0 - pad).max(line.x0),
                    y0,
                    x1: (chars[end - 1].x1 + pad).min(line.x1),
                    y1,
                    hit: (0.0, 0.0, 0.0, 0.0),
                });
            }
        }
        // Neighbours that the room made touch share the space between them.
        for k in first + 1..out.len() {
            if out[k - 1].x1 > out[k].x0 {
                let mid = (out[k - 1].x1 + out[k].x0) / 2.0;
                out[k - 1].x1 = mid;
                out[k].x0 = mid;
            }
        }
        let reach = (line.y1 - line.y0) * HIT_END;
        for k in first..out.len() {
            let left = if k > first { (out[k - 1].x1 + out[k].x0) / 2.0 } else { (out[k].x0 - reach).max(line.x0) };
            let right = if k + 1 < out.len() { (out[k].x1 + out[k + 1].x0) / 2.0 } else { (out[k].x1 + reach).min(line.x1) };
            out[k].hit = (left, line.y0, right, line.y1);
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
    let Context { sentence, sentence_lines, context, context_at } = context_of(lines, line_idx, span);
    let (y0, y1) = text_rows(line);
    let pad = pad_x(line);
    Some(Pick {
        word,
        candidates,
        sentence,
        sentence_lines,
        context,
        context_at,
        x0: (chars[start].x0 - pad).max(line.x0),
        y0,
        x1: (chars[end.saturating_sub(1).max(start)].x1 + pad).min(line.x1),
        y1,
        line_y: (line.y0, line.y1),
    })
}

fn dist(l: &OcrLine, x: f32, y: f32) -> f32 {
    let dx = if x < l.x0 { l.x0 - x } else if x > l.x1 { x - l.x1 } else { 0.0 };
    let dy = if y < l.y0 { l.y0 - y } else if y > l.y1 { y - l.y1 } else { 0.0 };
    // Vertical distance matters more: a line beside the pointer is rarely the intended one.
    dx + dy * 3.0
}

// Like Lookupper, the context of a word is its paragraph, and its sentence is cut from it at the
// sentence marks (see sentence.rs). Paragraphs are grown from the pointer line. The AI gets the
// paragraph; the context translation and Anki get the sentence.

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
/// Longest paragraph kept whole as context. A longer one keeps the sentences nearest the word's.
const MAX_CONTEXT_CHARS: usize = 1500;

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

/// What surrounds a picked word.
struct Context {
    sentence: String,
    sentence_lines: Vec<usize>,
    context: String,
    context_at: usize,
}

/// The sentence and the paragraph holding characters `span` of line `idx`.
fn context_of(lines: &[OcrLine], idx: usize, span: std::ops::Range<usize>) -> Context {
    let order = paragraph(lines, idx);
    let (text, pieces) = join(lines, &order);
    let Some(p) = pieces.iter().find(|p| p.line == idx) else {
        let line = lines[idx].text.trim().to_string();
        return Context { sentence: line.clone(), sentence_lines: vec![idx], context: line, context_at: 0 };
    };
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
    let (a, b) = around(&text, &ranges, from, to);
    Context {
        sentence: text[from..to].iter().collect(),
        sentence_lines: used,
        context: text[a..b].iter().collect(),
        context_at: ws.clamp(a, b) - a,
    }
}

/// The span of the paragraph `text` used as context for the sentence `from..to`: all of it, or,
/// past MAX_CONTEXT_CHARS, the whole sentences nearest it on both sides that fit.
fn around(text: &[char], ranges: &[std::ops::Range<usize>], from: usize, to: usize) -> (usize, usize) {
    let (mut a, mut b) = (0, text.len());
    if b > MAX_CONTEXT_CHARS {
        (a, b) = (from, to);
        let mut before = ranges.iter().rev().filter(|r| r.end <= from).peekable();
        let mut after = ranges.iter().filter(|r| r.start >= to).peekable();
        loop {
            let left = before.peek().filter(|r| b - r.start <= MAX_CONTEXT_CHARS).map(|r| r.start);
            if let Some(s) = left {
                a = s;
                before.next();
            }
            let right = after.peek().filter(|r| r.end - a <= MAX_CONTEXT_CHARS).map(|r| r.end);
            if let Some(e) = right {
                b = e;
                after.next();
            }
            if left.is_none() && right.is_none() {
                break;
            }
        }
    }
    while a < b && text[a].is_whitespace() {
        a += 1;
    }
    while b > a && text[b - 1].is_whitespace() {
        b -= 1;
    }
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::pick;
    use crate::ocr::{OcrChar, OcrLine};

    fn line(text: &str, x0: f32, y0: f32) -> OcrLine {
        let chars: Vec<OcrChar> = text.chars().enumerate()
            .map(|(i, ch)| OcrChar { ch, x0: x0 + i as f32 * 10.0, x1: x0 + i as f32 * 10.0 + 10.0 })
            .collect();
        OcrLine { text: text.into(), x0, y0, x1: x0 + text.len() as f32 * 10.0, y1: y0 + 20.0, chars, ink_y: None }
    }

    #[test]
    fn word_boxes_take_the_rows_of_the_ink_and_the_pointer_test_the_line() {
        let mut l = line("hello world", 100.0, 100.0);
        l.ink_y = Some((104.0, 116.0));
        let boxes = super::word_boxes(std::slice::from_ref(&l));
        // 12% of the ink's 12 rows of room on each side.
        assert!((boxes[1].y0 - 102.56).abs() < 0.01 && (boxes[1].y1 - 117.44).abs() < 0.01, "{:?}", boxes[1]);
        let p = pick(std::slice::from_ref(&l), 165.0, 101.0).unwrap();
        assert!((p.y0 - 102.56).abs() < 0.01 && (p.y1 - 117.44).abs() < 0.01);
        assert!(p.is_under(165.0, 101.0), "the line's padding still counts as on the word");
    }

    #[test]
    fn room_around_the_ink_stays_in_the_line_and_apart_from_the_next_word() {
        let mut l = line("hello world", 100.0, 100.0);
        l.ink_y = Some((100.0, 120.0));
        let boxes = super::word_boxes(std::slice::from_ref(&l));
        assert_eq!((boxes[0].y0, boxes[0].y1), (100.0, 120.0), "never past the detected box");
        assert_eq!(boxes[0].x0, 100.0, "nor past the line's ends");
        // The words' own edges are 150 and 160; the room (1.6) leaves the gap between them.
        assert!((boxes[0].x1 - 151.6).abs() < 0.01 && (boxes[1].x0 - 158.4).abs() < 0.01, "{boxes:?}");
        l.chars[4].x1 = 159.0; // a gap so narrow that the rooms would meet
        let narrow = super::word_boxes(std::slice::from_ref(&l));
        assert!(narrow[0].x1 <= narrow[1].x0 + 1e-3, "{narrow:?}");
    }

    #[test]
    fn the_hit_area_covers_the_line_and_meets_the_next_word_halfway() {
        let mut l = line("hello world", 100.0, 100.0);
        l.ink_y = Some((104.0, 116.0));
        let boxes = super::word_boxes(std::slice::from_ref(&l));
        let (a, b) = (boxes[0].hit, boxes[1].hit);
        assert_eq!((a.1, a.3), (100.0, 120.0), "the whole detected height");
        assert_eq!(a.0, 100.0, "never past the line's start");
        assert_eq!(a.2, b.0, "no gap between the two words' areas");
        assert!(a.2 > boxes[0].x1 && a.2 < boxes[1].x0, "the areas meet inside the space");
        assert_eq!(b.2, 210.0, "never past the line's end");
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
    fn context_is_the_whole_paragraph_with_the_word_located() {
        let lines = [
            line("The quick brown fox jumps over the", 100.0, 100.0),
            line("lazy dog. A second sentence starts", 100.0, 125.0),
            line("here and ends here.", 100.0, 150.0),
            line("Another paragraph far below.", 100.0, 260.0),
        ];
        let p = pick(&lines, 225.0, 135.0).unwrap();
        assert_eq!(p.sentence, "A second sentence starts here and ends here.");
        assert_eq!(p.context, "The quick brown fox jumps over the lazy dog. A second sentence starts here and ends here.");
        let at: String = p.context.chars().skip(p.context_at).take(6).collect();
        assert_eq!(at, "second");
    }

    #[test]
    fn a_long_paragraph_keeps_the_whole_sentences_nearest_the_word() {
        let lines: Vec<_> = (0..60).map(|i| line(&format!("Line {i:02} ends here with a few more words."), 100.0, 100.0 + i as f32 * 25.0)).collect();
        let p = pick(&lines, 105.0, 100.0 + 30.0 * 25.0 + 10.0).unwrap();
        assert_eq!(p.sentence, "Line 30 ends here with a few more words.");
        assert!(p.context.chars().count() <= super::MAX_CONTEXT_CHARS);
        assert!(p.context.starts_with("Line ") && p.context.ends_with("more words."));
        let at: String = p.context.chars().skip(p.context_at).take(4).collect();
        assert_eq!(at, "Line");
        assert!(p.context.contains("Line 25 ") && p.context.contains("Line 35 "));
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
