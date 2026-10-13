// Picks the word under the pointer from OCR lines.

use serde::Serialize;

use crate::ocr::{is_unspaced_script, OcrLine};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pick {
    pub word: String,
    /// Forms to look up, best first. For unspaced scripts these are prefixes starting at the pointer.
    pub candidates: Vec<String>,
    /// The line under the pointer joined with its neighbours, used as AI context.
    pub sentence: String,
    /// Word box in image coordinates.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
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

    let sentence = context(lines, line_idx);
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
    let word: String = if is_unspaced_script(chars[ci].ch) {
        candidates.first().cloned().unwrap_or_default()
    } else {
        chars[start..end].iter().map(|c| c.ch).collect()
    };
    Some(Pick {
        word,
        candidates,
        sentence,
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

/// The pointer line plus the lines of the same block directly above and below it.
fn context(lines: &[OcrLine], idx: usize) -> String {
    let target = &lines[idx];
    let h = (target.y1 - target.y0).max(4.0);
    let mut parts: Vec<&OcrLine> = lines
        .iter()
        .filter(|l| {
            let overlap = l.x1.min(target.x1) - l.x0.max(target.x0);
            let gap = if l.y0 > target.y1 { l.y0 - target.y1 } else if l.y1 < target.y0 { target.y0 - l.y1 } else { 0.0 };
            overlap > 0.0 && gap <= 1.6 * h
        })
        .collect();
    parts.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let mut out = String::new();
    for l in parts {
        let t = l.text.trim();
        if t.is_empty() {
            continue;
        }
        if out.ends_with('-') {
            out.pop();
        } else if !out.is_empty() && !t.starts_with(is_unspaced_script) {
            out.push(' ');
        }
        out.push_str(t);
    }
    out
}
