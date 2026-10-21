// Picks the word under the pointer from OCR lines.

use serde::Serialize;

use crate::ocr::{is_rtl_text, is_unspaced_script, Flow, OcrChar, OcrLine};
use crate::segment::Segment;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pick {
    pub word: String,
    /// Forms to look up, best first. For a word an analyzer found (segment.rs), its dictionary
    /// forms, then the word as written; otherwise, for unspaced scripts, prefixes starting at the
    /// pointer.
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
    /// The word is in a vertical line, so its box is tall.
    #[serde(skip)]
    pub vertical: bool,
}

/// How far past a word's drawn box the pointer still counts as on it: a share of the box's
/// height (its width in a vertical line), so it scales with the text, and at least a few capture
/// pixels.
const UNDER_SLACK: f32 = 0.1;
const UNDER_SLACK_MIN: f32 = 3.0;

impl Pick {
    /// True when the pointer is on the picked word itself: inside its drawn box, which already
    /// leaves a little room around the ink. Pressed in the space between words or lines, the
    /// shortcut is off any word: the automatic capture then outlines the block of text, and a
    /// region capture opens the whole-screen overlay instead of a pop-up.
    pub fn is_under(&self, px: f32, py: f32) -> bool {
        let across = if self.vertical { self.x1 - self.x0 } else { self.y1 - self.y0 };
        let slack = (across * UNDER_SLACK).max(UNDER_SLACK_MIN);
        px >= self.x0 - slack && px <= self.x1 + slack && py >= self.y0 - slack && py <= self.y1 + slack
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

/// The left and right of characters `start..end` of a line, with the room at the sides. In
/// right-to-left text the first character is the rightmost.
fn span_x(line: &OcrLine, start: usize, end: usize) -> (f32, f32) {
    let pad = pad_x(line);
    let chars = &line.chars[start..end.max(start + 1)];
    let x0 = chars.iter().map(|c| c.x0).fold(f32::INFINITY, f32::min);
    let x1 = chars.iter().map(|c| c.x1).fold(f32::NEG_INFINITY, f32::max);
    ((x0 - pad).max(line.x0), (x1 + pad).min(line.x1))
}

/// Every word of every line as a rectangle. With an analyzer for the language (segment.rs), its
/// words: each word of Japanese or Chinese text, a Korean word with its particles. Otherwise runs
/// of unspaced-script characters (CJK, Thai) form one rectangle, and a click inside it still picks
/// the longest dictionary match from the click point.
pub fn word_boxes(lines: &[OcrLine], seg: Option<&dyn Segment>) -> Vec<WordBox> {
    let lines: Vec<OcrLine> = lines.iter().map(upright).collect();
    let lines = &lines[..];
    let analyzed = seg.map(|s| analyzed_spans(lines, s)).unwrap_or_default();
    let mut out: Vec<WordBox> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        if noise_line(lines, li) {
            continue;
        }
        let chars = &line.chars;
        let first = out.len();
        let spans = match analyzed.get(li) {
            Some(Some(spans)) => spans.clone(),
            _ => scanned_spans(line),
        };
        for (start, end) in spans {
            let text: String = chars[start..end].iter().map(|c| c.ch).collect();
            if !text.trim().is_empty() {
                let (y0, y1) = text_rows(line);
                let (x0, x1) = span_x(line, start, end);
                out.push(WordBox { text, x0, y0, x1, y1, hit: (0.0, 0.0, 0.0, 0.0) });
            }
        }
        // Left to right on screen, whichever way the line reads.
        out[first..].sort_by(|a, b| a.x0.total_cmp(&b.x0));
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
        if line.vertical() {
            for b in &mut out[first..] {
                (b.x0, b.y0, b.x1, b.y1) = on_screen(line.flow, (b.x0, b.y0, b.x1, b.y1));
                b.hit = on_screen(line.flow, b.hit);
            }
        }
    }
    out
}

// A vertical line is picked as a horizontal one: `upright` turns it a quarter to lie the way it
// reads. A line running down (a column of CJK text) gets its top at the left and its right side at
// the top, so columns read right to left become rows read top to bottom. A line running up (text
// turned a quarter left) gets its bottom at the left and its left side at the top, so lines read
// left to right become rows read top to bottom. Its characters' spans, along the line, become
// spans across, so paragraphs, sentences and word boxes need nothing else. Boxes found there are
// turned back with `on_screen`.

/// `line`, turned to lie horizontal if it is vertical.
fn upright(line: &OcrLine) -> OcrLine {
    let mut out = line.clone();
    match line.flow {
        Flow::Across => {}
        Flow::Down => {
            (out.x0, out.y0, out.x1, out.y1) = (line.y0, -line.x1, line.y1, -line.x0);
            out.ink_y = line.ink_y.map(|(a, b)| (-b, -a));
        }
        Flow::Up => {
            (out.x0, out.y0, out.x1, out.y1) = (-line.y1, line.x0, -line.y0, line.x1);
            for c in &mut out.chars {
                (c.x0, c.x1) = (-c.x1, -c.x0);
            }
        }
    }
    out
}

/// A capture point in the frame of `upright` for a line running `flow`.
fn upright_point(flow: Flow, x: f32, y: f32) -> (f32, f32) {
    match flow {
        Flow::Across => (x, y),
        Flow::Down => (y, -x),
        Flow::Up => (-y, x),
    }
}

/// A box `(x0, y0, x1, y1)` from the frame of `upright` for a line running `flow` back on the
/// capture.
fn on_screen(flow: Flow, (x0, y0, x1, y1): (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    match flow {
        Flow::Across => (x0, y0, x1, y1),
        Flow::Down => (-y1, x0, -y0, x1),
        Flow::Up => (y0, -x1, y1, -x0),
    }
}

/// A line's words as character ranges. As in Lookupper's recognizer, a word is a run of characters
/// without a space, cut where unspaced-script text (CJK, Thai) starts or ends; Lookupper then trims
/// what is not a letter or digit from its ends, so "(U.S.A.)," is "U.S.A", and keeps a run of
/// punctuation alone ("？", "—") whole. A run of unspaced-script characters is one range.
fn scanned_spans(line: &OcrLine) -> Vec<(usize, usize)> {
    let chars = &line.chars;
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].ch.is_whitespace() {
            i += 1;
            continue;
        }
        let unspaced = is_unspaced_script(chars[i].ch);
        let start = i;
        i += 1;
        while i < chars.len() && !chars[i].ch.is_whitespace() && is_unspaced_script(chars[i].ch) == unspaced {
            i += 1;
        }
        out.push(if unspaced { (start, i) } else { word_span(chars, start, i) });
    }
    out
}

/// Characters `start..end` without what is not a letter or digit at their ends.
fn trimmed(chars: &[OcrChar], mut start: usize, mut end: usize) -> Option<(usize, usize)> {
    while start < end && !chars[start].ch.is_alphanumeric() {
        start += 1;
    }
    while end > start && !chars[end - 1].ch.is_alphanumeric() {
        end -= 1;
    }
    (start < end).then_some((start, end))
}

/// Characters `start..end` as Lookupper's `GetTrimmedWordAndRect` makes them a word: trimmed, or
/// whole when nothing would be left ("？").
fn word_span(chars: &[OcrChar], start: usize, end: usize) -> (usize, usize) {
    trimmed(chars, start, end).unwrap_or((start, end))
}

/// Whether a character belongs to a word of a spaced script: anything but a space and the
/// unspaced scripts.
fn in_word(c: char) -> bool {
    !c.is_whitespace() && !is_unspaced_script(c)
}

/// The pieces of a word between the characters that are neither letters, digits nor apostrophes
/// ("well-known", or "end.Next" with a space the recognizer missed), the one holding character `ci`
/// first. Empty for a word in one piece.
fn parts(chars: &[OcrChar], start: usize, end: usize, ci: usize) -> Vec<String> {
    let joins = |c: char| c.is_alphanumeric() || matches!(c, '\'' | '’' | '·');
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut i = start;
    while i < end {
        if !joins(chars[i].ch) {
            i += 1;
            continue;
        }
        let from = i;
        while i < end && joins(chars[i].ch) {
            i += 1;
        }
        if let Some(piece) = trimmed(chars, from, i) {
            pieces.push(piece);
        }
    }
    if pieces.len() < 2 {
        return Vec::new();
    }
    let held = pieces.iter().position(|&(a, b)| a <= ci && ci < b).unwrap_or(0);
    let first = pieces.remove(held);
    std::iter::once(first).chain(pieces).map(|(a, b)| chars[a..b].iter().map(|c| c.ch).collect()).collect()
}

/// Whether a paragraph is noise, as Lookupper leaves out: no letter at all ("—", "•"), or a single
/// Latin or Cyrillic letter (a stray mark read as "l" or "о"). Lookupper also leaves out a number
/// alone; Lookitan keeps any paragraph with a digit, at the user's request.
fn is_noise(lines: &[OcrLine], order: &[usize]) -> bool {
    let chars = || order.iter().flat_map(|&li| lines[li].chars.iter().map(|c| c.ch));
    if chars().any(|c| c.is_numeric()) {
        return false;
    }
    let mut letters = chars().filter(|c| c.is_alphabetic());
    match (letters.next(), letters.next()) {
        (None, _) => true,
        (Some(c), None) => matches!(c as u32, 0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x24F | 0x400..=0x52F),
        _ => false,
    }
}

/// Whether line `idx` is in a noise paragraph. A line with two letters or a digit never is.
fn noise_line(lines: &[OcrLine], idx: usize) -> bool {
    let line = &lines[idx].chars;
    line.iter().filter(|c| c.ch.is_alphabetic()).nth(1).is_none()
        && !line.iter().any(|c| c.ch.is_numeric())
        && is_noise(lines, &paragraph(lines, idx))
}

/// Whether the analyzer reads this line.
fn analyzed_line(line: &OcrLine, seg: &dyn Segment) -> bool {
    line.chars.iter().any(|c| seg.handles(c.ch))
}

/// The words of every line the analyzer reads, as character ranges. Like Lookupper, each paragraph
/// is analyzed as one text, so a word broken across two lines is one word with a piece on each.
fn analyzed_spans(lines: &[OcrLine], seg: &dyn Segment) -> Vec<Option<Vec<(usize, usize)>>> {
    let mut out: Vec<Option<Vec<(usize, usize)>>> = vec![None; lines.len()];
    for i in 0..lines.len() {
        if out[i].is_some() || !analyzed_line(&lines[i], seg) {
            continue;
        }
        let (text, pieces) = join(lines, &paragraph(lines, i));
        let words = seg.words(&text.iter().collect::<String>());
        for p in &pieces {
            if out[p.line].is_some() || !analyzed_line(&lines[p.line], seg) {
                continue;
            }
            let spans = words.iter().filter_map(|w| piece_span(p, w.start, w.end)).collect();
            out[p.line] = Some(spans);
        }
    }
    out
}

/// The characters `start..end` of a joined paragraph that are on piece `p`'s line, as a range of
/// that line's characters.
fn piece_span(p: &Piece, start: usize, end: usize) -> Option<(usize, usize)> {
    let (a, b) = (start.max(p.at), end.min(p.at + p.len));
    (a < b).then(|| (p.first + a - p.at, p.first + b - p.at))
}

/// The longest run tried from the pointer in unspaced scripts, and from the start of an analyzed
/// word for one the dictionaries split otherwise.
const MAX_RUN: usize = 16;

/// The analyzed word holding character `ci` of line `idx`: its paragraph is analyzed whole, so the
/// word may start before the pointer, or on the line above.
fn pick_analyzed(lines: &[OcrLine], idx: usize, ci: usize, seg: &dyn Segment) -> Option<Pick> {
    let line = &lines[idx];
    let (text, pieces) = join(lines, &paragraph(lines, idx));
    let p = pieces.iter().find(|p| p.line == idx)?;
    if ci < p.first || ci >= p.first + p.len {
        return None;
    }
    let at = p.at + ci - p.first;
    let w = seg.words(&text.iter().collect::<String>()).into_iter().find(|w| w.start <= at && at < w.end)?;
    let (start, end) = piece_span(p, w.start, w.end)?;
    let word: String = text[w.start..w.end].iter().collect();
    let mut candidates: Vec<String> = Vec::new();
    let run: Vec<char> = word.chars().take(MAX_RUN).collect();
    let shorter = run.first().is_some_and(|&c| is_unspaced_script(c));
    for c in w.lemmas.into_iter().chain([word.clone()]).chain((1..run.len()).rev().filter(|_| shorter).map(|n| run[..n].iter().collect::<String>())) {
        if !candidates.contains(&c) {
            candidates.push(c);
        }
    }
    let Context { sentence, sentence_lines, context, context_at } = context_of(lines, idx, start..end);
    let (y0, y1) = text_rows(line);
    let (x0, x1) = span_x(line, start, end);
    Some(Pick { word, candidates, sentence, sentence_lines, context, context_at, x0, y0, x1, y1, vertical: false })
}

/// The word under the pointer, found through `seg` when it reads that word's script.
pub fn pick(lines: &[OcrLine], px: f32, py: f32, seg: Option<&dyn Segment>) -> Option<Pick> {
    let lines: Vec<OcrLine> = lines.iter().map(upright).collect();
    let near = |l: &OcrLine| {
        let (x, y) = upright_point(l.flow, px, py);
        dist(l, x, y)
    };
    let line_idx = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.chars.is_empty())
        .min_by(|(_, a), (_, b)| near(a).total_cmp(&near(b)))
        .map(|(i, _)| i)?;
    let flow = lines[line_idx].flow;
    let (px, py) = upright_point(flow, px, py);
    let mut pick = pick_upright(&lines, line_idx, px, py, seg)?;
    (pick.x0, pick.y0, pick.x1, pick.y1) = on_screen(flow, (pick.x0, pick.y0, pick.x1, pick.y1));
    pick.vertical = flow != Flow::Across;
    Some(pick)
}

/// The word under the pointer `px, py` on line `line_idx`, all in the frame of `upright`.
fn pick_upright(lines: &[OcrLine], line_idx: usize, px: f32, py: f32, seg: Option<&dyn Segment>) -> Option<Pick> {
    let line = &lines[line_idx];
    if dist(line, px, py) > (line.y1 - line.y0).max(12.0) * 1.5 || noise_line(lines, line_idx) {
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
    // Off any word: a space. Punctuation alone ("？", "—") is a word, as in Lookupper.
    let off = |i: usize| chars[i].ch.is_whitespace();
    // Pointer off a word: prefer the word just before it, then the one after.
    if off(ci) {
        if ci > 0 && !off(ci - 1) {
            ci -= 1;
        } else if ci + 1 < chars.len() && !off(ci + 1) {
            ci += 1;
        } else {
            return None;
        }
    }

    if let Some(seg) = seg.filter(|s| s.handles(chars[ci].ch)) {
        if let Some(pick) = pick_analyzed(lines, line_idx, ci, seg) {
            return Some(pick);
        }
    }

    let (start, end, candidates) = if is_unspaced_script(chars[ci].ch) {
        let mut end = ci;
        while end < chars.len() && end - ci < MAX_RUN && (is_unspaced_script(chars[end].ch) || chars[end].ch.is_alphanumeric()) {
            end += 1;
        }
        let run: Vec<char> = chars[ci..end].iter().map(|c| c.ch).collect();
        let candidates: Vec<String> = (1..=run.len()).rev().map(|n| run[..n].iter().collect()).collect();
        (ci, end, candidates)
    } else {
        // The word as scanned_spans cuts it: the run without a space, trimmed ("(word)," -> "word").
        let mut start = ci;
        while start > 0 && in_word(chars[start - 1].ch) {
            start -= 1;
        }
        let mut end = ci + 1;
        while end < chars.len() && in_word(chars[end].ch) {
            end += 1;
        }
        let (start, end) = word_span(chars, start, end);
        let word: String = chars[start..end].iter().map(|c| c.ch).collect();
        let mut candidates = crate::dict::candidate_forms(&word);
        // "well-known" -> also "known" (the part under the pointer) and "well", if the dictionary
        // lacks the compound.
        for part in parts(chars, start, end, ci) {
            candidates.extend(crate::dict::candidate_forms(&part));
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
    let (x0, x1) = span_x(line, start, end);
    Some(Pick { word, candidates, sentence, sentence_lines, context, context_at, x0, y0, x1, y1, vertical: false })
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
    // Lines running different ways are in different frames (see `upright`), and different text.
    if a.flow != b.flow {
        return false;
    }
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
/// a row left to right, or right to left in a right-to-left paragraph (as Lookupper's
/// `IsRtlParagraph` decides).
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
    let rtl = is_rtl_text(members.iter().flat_map(|&m| lines[m].text.chars()));
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
            if rtl {
                row.reverse();
            }
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
        if hyphenated(&text, cs[first]) {
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

/// Whether a paragraph's text so far ends in the first part of a word hyphenated across the line
/// break, and `next` starts its second part. As Lookupper decides: a hyphen (-, U+2010, U+2011)
/// after a letter, and then a lowercase letter, or a capital after a word with no lowercase
/// ("EXAM-" "PLE", but not "Anglo-" "Saxon").
fn hyphenated(text: &[char], next: char) -> bool {
    let n = text.len();
    if n < 2 || !matches!(text[n - 1], '-' | '\u{2010}' | '\u{2011}') || !text[n - 2].is_alphabetic() {
        return false;
    }
    let start = text.iter().rposition(|c| c.is_whitespace()).map_or(0, |i| i + 1);
    next.is_lowercase() || (next.is_uppercase() && !text[start..].iter().any(|c| c.is_lowercase()))
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
    use crate::ocr::{Flow, OcrChar, OcrLine};
    use crate::segment::{Kind, Segment, Word};

    fn line(text: &str, x0: f32, y0: f32) -> OcrLine {
        let chars: Vec<OcrChar> = text.chars().enumerate()
            .map(|(i, ch)| OcrChar { ch, x0: x0 + i as f32 * 10.0, x1: x0 + i as f32 * 10.0 + 10.0 })
            .collect();
        OcrLine { text: text.into(), x0, y0, x1: x0 + text.len() as f32 * 10.0, y1: y0 + 20.0, chars, ink_y: None, flow: Flow::Across }
    }

    #[test]
    fn word_boxes_take_the_rows_of_the_ink_and_the_pointer_test_the_line() {
        let mut l = line("hello world", 100.0, 100.0);
        l.ink_y = Some((104.0, 116.0));
        let boxes = super::word_boxes(std::slice::from_ref(&l), None);
        // 12% of the ink's 12 rows of room on each side.
        assert!((boxes[1].y0 - 102.56).abs() < 0.01 && (boxes[1].y1 - 117.44).abs() < 0.01, "{:?}", boxes[1]);
        let p = pick(std::slice::from_ref(&l), 165.0, 101.0, None).unwrap();
        assert!((p.y0 - 102.56).abs() < 0.01 && (p.y1 - 117.44).abs() < 0.01);
        assert!(p.is_under(165.0, 110.0));
        assert!(!p.is_under(165.0, 99.0), "above the word's box and its slack is off the word");
    }

    #[test]
    fn the_space_between_two_words_is_off_both() {
        let lines = [line("hello world", 100.0, 100.0)];
        let p = pick(&lines, 155.0, 110.0, None).unwrap();
        assert_eq!(p.word, "hello", "the space still picks a word, for the overlay");
        assert!(!p.is_under(155.0, 110.0), "but the shortcut there is off the word");
    }

    #[test]
    fn room_around_the_ink_stays_in_the_line_and_apart_from_the_next_word() {
        let mut l = line("hello world", 100.0, 100.0);
        l.ink_y = Some((100.0, 120.0));
        let boxes = super::word_boxes(std::slice::from_ref(&l), None);
        assert_eq!((boxes[0].y0, boxes[0].y1), (100.0, 120.0), "never past the detected box");
        assert_eq!(boxes[0].x0, 100.0, "nor past the line's ends");
        // The words' own edges are 150 and 160; the room (1.6) leaves the gap between them.
        assert!((boxes[0].x1 - 151.6).abs() < 0.01 && (boxes[1].x0 - 158.4).abs() < 0.01, "{boxes:?}");
        l.chars[4].x1 = 159.0; // a gap so narrow that the rooms would meet
        let narrow = super::word_boxes(std::slice::from_ref(&l), None);
        assert!(narrow[0].x1 <= narrow[1].x0 + 1e-3, "{narrow:?}");
    }

    #[test]
    fn the_hit_area_covers_the_line_and_meets_the_next_word_halfway() {
        let mut l = line("hello world", 100.0, 100.0);
        l.ink_y = Some((104.0, 116.0));
        let boxes = super::word_boxes(std::slice::from_ref(&l), None);
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
        let p = pick(&lines, 165.0, 110.0, None).unwrap();
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
        let p = pick(&lines, 145.0, 110.0, None).unwrap();
        assert_eq!(p.word, "quick");
        assert_eq!(p.sentence, "The quick brown fox jumps over the lazy dog.");
        assert_eq!(p.sentence_lines, [0, 1]);
        let p = pick(&lines, 225.0, 135.0, None).unwrap();
        assert_eq!(p.word, "second");
        assert_eq!(p.sentence, "A second sentence starts here and ends here.");
        assert_eq!(p.sentence_lines, [1, 2]);
        let p = pick(&lines, 105.0, 270.0, None).unwrap();
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
        let p = pick(&lines, 225.0, 135.0, None).unwrap();
        assert_eq!(p.sentence, "A second sentence starts here and ends here.");
        assert_eq!(p.context, "The quick brown fox jumps over the lazy dog. A second sentence starts here and ends here.");
        let at: String = p.context.chars().skip(p.context_at).take(6).collect();
        assert_eq!(at, "second");
    }

    #[test]
    fn a_long_paragraph_keeps_the_whole_sentences_nearest_the_word() {
        let lines: Vec<_> = (0..60).map(|i| line(&format!("Line {i:02} ends here with a few more words."), 100.0, 100.0 + i as f32 * 25.0)).collect();
        let p = pick(&lines, 105.0, 100.0 + 30.0 * 25.0 + 10.0, None).unwrap();
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
        let p = pick(&lines, 195.0, 110.0, None).unwrap();
        assert_eq!(p.word, "good");
        assert_eq!(p.sentence, "It was a good example of work.");
        assert_eq!(p.sentence_lines, [1, 2]);
    }

    #[test]
    fn a_line_end_hyphen_joins_as_lookupper_decides() {
        let sentence = |a: &str, b: &str| pick(&[line(a, 100.0, 100.0), line(b, 100.0, 125.0)], 105.0, 110.0, None).unwrap().sentence;
        assert_eq!(sentence("an exam\u{2010}", "ple here."), "an example here.");
        assert_eq!(sentence("THE EXAM-", "PLE IS HERE."), "THE EXAMPLE IS HERE.");
        assert_eq!(sentence("the Anglo-", "Saxon king."), "the Anglo- Saxon king.", "a capital after lowercase starts a new word");
    }

    #[test]
    fn a_word_is_the_run_without_spaces_trimmed_at_its_ends() {
        let lines = [line("in the (U.S.A.), end.Next one", 100.0, 100.0)];
        // On the "(" and on the "S".
        assert_eq!(pick(&lines, 175.0, 110.0, None).unwrap().word, "U.S.A");
        assert_eq!(pick(&lines, 205.0, 110.0, None).unwrap().word, "U.S.A");
        // On the "N".
        let p = pick(&lines, 315.0, 110.0, None).unwrap();
        assert_eq!(p.word, "end.Next");
        let at = |w: &str| p.candidates.iter().position(|c| c == w).unwrap();
        assert!(at("Next") < at("end"), "the part under the pointer first: {:?}", p.candidates);
        let texts: Vec<String> = super::word_boxes(&lines, None).into_iter().map(|b| b.text).collect();
        assert_eq!(texts, ["in", "the", "U.S.A", "end.Next", "one"]);
        // Punctuation standing alone is a word of its own, as in Lookupper.
        assert_eq!(pick(&[line("今日は雨。", 100.0, 100.0)], 145.0, 110.0, None).unwrap().word, "。");
        let texts: Vec<String> = super::word_boxes(&[line("你好吗？ well — yes", 100.0, 100.0)], None).into_iter().map(|b| b.text).collect();
        assert_eq!(texts, ["你好吗", "？", "well", "—", "yes"]);
    }

    #[test]
    fn a_paragraph_of_a_single_latin_or_cyrillic_letter_is_noise() {
        assert!(pick(&[line("a", 100.0, 100.0)], 105.0, 110.0, None).is_none());
        assert!(super::word_boxes(&[line("l", 100.0, 100.0)], None).is_empty());
        // A number alone still looks up, unlike in Lookupper.
        assert_eq!(pick(&[line("2024", 100.0, 100.0)], 115.0, 110.0, None).unwrap().word, "2024");
        assert_eq!(pick(&[line("A1", 100.0, 100.0)], 105.0, 110.0, None).unwrap().word, "A1");
        assert_eq!(super::word_boxes(&[line("١٢٣", 100.0, 100.0)], None).len(), 1);
        let lines = [line("a", 100.0, 100.0), line("cat sat", 100.0, 125.0)];
        assert_eq!(pick(&lines, 105.0, 110.0, None).unwrap().word, "a", "a letter in a paragraph of text");
        assert_eq!(pick(&[line("日", 100.0, 100.0)], 105.0, 110.0, None).unwrap().word, "日");
        // A paragraph with no letter and no digit is noise, as in Lookupper.
        assert!(pick(&[line("— •", 100.0, 100.0)], 105.0, 110.0, None).is_none());
        assert!(super::word_boxes(&[line("？", 100.0, 100.0)], None).is_empty());
    }

    /// A right-to-left line in reading order, its first character at the right end `x1`.
    fn rtl_line(text: &str, x1: f32, y0: f32) -> OcrLine {
        let n = text.chars().count();
        let chars: Vec<OcrChar> = text.chars().enumerate()
            .map(|(i, ch)| OcrChar { ch, x0: x1 - (i + 1) as f32 * 10.0, x1: x1 - i as f32 * 10.0 })
            .collect();
        OcrLine { text: text.into(), x0: x1 - n as f32 * 10.0, y0, x1, y1: y0 + 20.0, chars, ink_y: None, flow: Flow::Across }
    }

    #[test]
    fn right_to_left_words_and_paragraphs_read_from_the_right() {
        let l = rtl_line("مرحبا بالعالم", 300.0, 100.0);
        let boxes = super::word_boxes(std::slice::from_ref(&l), None);
        let texts: Vec<&str> = boxes.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["بالعالم", "مرحبا"], "boxes left to right on screen");
        assert!(boxes[0].x0 < boxes[0].x1 && boxes[0].x1 <= boxes[1].x0, "{boxes:?}");
        let p = pick(std::slice::from_ref(&l), 285.0, 110.0, None).unwrap();
        assert_eq!(p.word, "مرحبا");
        assert_eq!((p.x0, p.x1), (250.0, 300.0));
        // Two pieces of one row: the right one is read first.
        let lines = [rtl_line("بالعالم", 170.0, 200.0), rtl_line("مرحبا", 250.0, 200.0)];
        let p = pick(&lines, 105.0, 210.0, None).unwrap();
        assert_eq!(p.word, "بالعالم");
        assert_eq!(p.sentence, "مرحبا بالعالم");
        assert_eq!(p.sentence_lines, [1, 0]);
    }

    /// A vertical line 20 wide at `x0`, its characters 20 tall from `y0` down.
    fn column(text: &str, x0: f32, y0: f32) -> OcrLine {
        let n = text.chars().count();
        let chars: Vec<OcrChar> = text.chars().enumerate()
            .map(|(i, ch)| OcrChar { ch, x0: y0 + i as f32 * 20.0, x1: y0 + (i + 1) as f32 * 20.0 })
            .collect();
        OcrLine { text: text.into(), x0, y0, x1: x0 + 20.0, y1: y0 + n as f32 * 20.0, chars, ink_y: None, flow: Flow::Down }
    }

    #[test]
    fn a_vertical_line_has_a_word_per_box_down_the_column() {
        let lines = [column("日本語を勉強しています。", 200.0, 100.0)];
        let seg = japanese();
        let boxes = super::word_boxes(&lines, Some(&seg));
        let texts: Vec<&str> = boxes.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["日本語", "を", "勉強しています", "。"], "not the whole column as one");
        assert_eq!((boxes[0].x0, boxes[0].y0, boxes[0].x1, boxes[0].y1), (200.0, 100.0, 220.0, 160.0));
        assert_eq!((boxes[1].hit.1, boxes[1].hit.3), (160.0, 180.0), "the hit areas follow the column down");
        assert_eq!((boxes[1].hit.0, boxes[1].hit.2), (200.0, 220.0));
        // On 強, the sixth character.
        let p = pick(&lines, 210.0, 205.0, Some(&seg)).unwrap();
        assert_eq!(p.word, "勉強しています");
        assert_eq!((p.x0, p.y0, p.x1, p.y1), (200.0, 180.0, 220.0, 320.0));
        assert!(p.is_under(210.0, 205.0) && !p.is_under(240.0, 205.0));
        assert_eq!(pick(&lines, 210.0, 125.0, None).unwrap().word, "本語を勉強しています", "longest match down from the pointer");
    }

    #[test]
    fn vertical_columns_read_from_the_right() {
        let lines = [column("晴れ。", 170.0, 100.0), column("今日は", 200.0, 100.0), line("Title", 100.0, 20.0)];
        let p = pick(&lines, 180.0, 110.0, None).unwrap();
        assert_eq!(p.sentence, "今日は晴れ。");
        assert_eq!(p.sentence_lines, [1, 0]);
        assert_eq!(pick(&lines, 105.0, 30.0, None).unwrap().word, "Title", "a row is apart from the columns");
    }

    /// A line running up 20 wide at `x0`, its characters 20 tall from `bottom` up.
    fn rising(text: &str, x0: f32, bottom: f32) -> OcrLine {
        let n = text.chars().count();
        let chars: Vec<OcrChar> = text.chars().enumerate()
            .map(|(i, ch)| OcrChar { ch, x0: bottom - (i + 1) as f32 * 20.0, x1: bottom - i as f32 * 20.0 })
            .collect();
        OcrLine { text: text.into(), x0, y0: bottom - n as f32 * 20.0, x1: x0 + 20.0, y1: bottom, chars, ink_y: None, flow: Flow::Up }
    }

    #[test]
    fn text_turned_a_quarter_left_reads_up_the_screen_and_its_lines_left_to_right() {
        let lines = [rising("goes on.", 125.0, 400.0), rising("hello world", 100.0, 400.0)];
        let boxes = super::word_boxes(&lines[1..], None);
        let texts: Vec<&str> = boxes.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["hello", "world"]);
        assert_eq!((boxes[1].x0, boxes[1].y0, boxes[1].x1, boxes[1].y1), (100.0, 180.0, 120.0, 280.0));
        // On the "w", near the top.
        let p = pick(&lines, 110.0, 270.0, None).unwrap();
        assert_eq!(p.word, "world");
        assert_eq!((p.x0, p.y0, p.x1, p.y1), (100.0, 180.0, 120.0, 280.0));
        assert_eq!(p.sentence, "hello world goes on.");
        assert_eq!(p.sentence_lines, [1, 0]);
    }

    #[test]
    fn pieces_of_one_row_are_read_left_to_right() {
        let lines = [line("my friend. Bye.", 240.0, 100.0), line("Hello there,", 100.0, 100.0)];
        let p = pick(&lines, 105.0, 110.0, None).unwrap();
        assert_eq!(p.word, "Hello");
        assert_eq!(p.sentence, "Hello there, my friend.");
        assert_eq!(p.sentence_lines, [1, 0]);
    }

    #[test]
    fn unspaced_scripts_split_at_their_own_marks() {
        let lines = [line("今日は晴れ。明日は雨。", 100.0, 100.0)];
        let p = pick(&lines, 165.0, 110.0, None).unwrap();
        assert_eq!(p.sentence, "明日は雨。");
    }

    #[test]
    fn text_without_sentence_marks_falls_back_to_the_lines_around_the_pointer() {
        let lines: Vec<_> = (0..30).map(|i| line("menu item without any marks here", 100.0, 100.0 + i as f32 * 25.0)).collect();
        let p = pick(&lines, 105.0, 100.0 + 15.0 * 25.0 + 10.0, None).unwrap();
        assert_eq!(p.sentence_lines, [14, 15, 16]);
    }

    /// An analyzer that knows the words of the test's text, in order, with their lemmas.
    struct Words(Vec<(&'static str, Vec<&'static str>)>);

    impl Segment for Words {
        fn words(&self, text: &str) -> Vec<Word> {
            let chars: Vec<char> = text.chars().collect();
            let mut out = Vec::new();
            let mut at = 0;
            for (word, lemmas) in &self.0 {
                let w: Vec<char> = word.chars().collect();
                if let Some(start) = (at..(chars.len() + 1).saturating_sub(w.len())).find(|&i| chars[i..i + w.len()] == w[..]) {
                    let kind = if w.iter().any(|c| c.is_alphanumeric()) { Kind::Word } else { Kind::Punct };
                    out.push(Word { start, end: start + w.len(), kind, lemmas: lemmas.iter().map(|s| s.to_string()).collect() });
                    at = start + w.len();
                }
            }
            out
        }

        fn handles(&self, c: char) -> bool {
            crate::ocr::is_unspaced_script(c)
        }
    }

    fn japanese() -> Words {
        Words(vec![("日本語", vec![]), ("を", vec![]), ("勉強しています", vec!["勉強する", "勉強"]), ("。", vec![])])
    }

    #[test]
    fn an_analyzed_word_is_picked_whole_from_any_of_its_characters() {
        let lines = [line("日本語を勉強しています。", 100.0, 100.0)];
        let seg = japanese();
        // On 強, the word's second character.
        let p = pick(&lines, 155.0, 110.0, Some(&seg)).unwrap();
        assert_eq!(p.word, "勉強しています");
        assert_eq!(p.candidates[..4], ["勉強する", "勉強", "勉強しています", "勉強していま"]);
        assert_eq!(p.candidates.last().map(String::as_str), Some("勉"));
        assert_eq!((p.x0, p.x1), (140.0, 210.0), "the box is the word's, not the run past the pointer");
        assert_eq!(p.sentence, "日本語を勉強しています。");
        assert!(p.is_under(155.0, 110.0));
        // Without an analyzer, the longest match runs from the pointer.
        assert_eq!(pick(&lines, 155.0, 110.0, None).unwrap().word, "強しています");
    }

    #[test]
    fn a_paragraph_is_analyzed_whole_so_a_word_broken_across_lines_is_one_word() {
        let lines = [line("日本語を勉", 100.0, 100.0), line("強しています。", 100.0, 125.0)];
        let seg = japanese();
        let boxes = super::word_boxes(&lines, Some(&seg));
        let texts: Vec<&str> = boxes.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["日本語", "を", "勉", "強しています", "。"], "a box per word, a piece on each line");
        let p = pick(&lines, 115.0, 135.0, Some(&seg)).unwrap();
        assert_eq!(p.word, "勉強しています");
        assert_eq!((p.x0, p.x1), (100.0, 160.0), "the piece on the pointer's line");
        assert_eq!(super::word_boxes(&lines, None).len(), 3, "without an analyzer, a box per run, and the 。");
    }

    #[test]
    fn pointer_near_but_off_the_text_is_not_under_it() {
        let lines = [line("hello world", 100.0, 100.0)];
        // Below the line, within pick's 1.5-line tolerance but clearly off the word.
        let p = pick(&lines, 130.0, 128.0, None).unwrap();
        assert!(!p.is_under(130.0, 128.0));
        // Past the end of the line.
        let p = pick(&lines, 240.0, 110.0, None).unwrap();
        assert!(!p.is_under(240.0, 110.0));
    }
}
