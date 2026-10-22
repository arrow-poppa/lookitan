// The block of text under the pointer, found on the detected boxes before anything is recognized.
// In the automatic capture, as in Lookupper's pointer mode, the whole monitor is detected but only
// this block is recognized: the line under the pointer and the lines set close around it (a
// paragraph, a list item, a chat message). A pointer on no line finds no block, and neither does
// a block that reads as at most one letter or digit (`reads_as_text`): the lookup reads the whole
// screen instead. The thresholds are our own, and read a whole block where Lookupper's clustering
// reads part of it. A column (see `is_vertical_box`) is measured like a row turned on its side:
// by its width, with the columns beside it as the lines above and below.

use crate::ocr::{is_vertical_box, paddle::TextBox};

/// Slack around a box for the pointer to count as on it, in text sizes: across the line (above
/// and below a row) and past its ends.
const POINTER_SLACK_ACROSS: f32 = 0.2;
const POINTER_SLACK_ALONG: f32 = 0.5;
/// Size ratio above which two boxes are different text (a title over its body).
const MAX_HEIGHT_RATIO: f32 = 1.6;
/// Gap between two lines of one block, in text sizes. Tighter than a paragraph's (see words.rs),
/// so list items and separate messages stay apart.
const MAX_LINE_GAP: f32 = 0.6;
/// Gap between two boxes of one line (a line the detector split), in text sizes.
const MAX_ROW_GAP: f32 = 1.5;
/// Bounds the recognition work of one lookup.
const MAX_BLOCK_BOXES: usize = 40;

fn is_column(b: &TextBox) -> bool {
    is_vertical_box(b.x1 - b.x0, b.y1 - b.y0)
}

/// A box as `(along0, across0, along1, across1)`: as it is in the frame of a row, and with its
/// axes swapped in the frame of a column, whose length then runs along like a row's.
fn frame(b: &TextBox, column: bool) -> (f32, f32, f32, f32) {
    if column { (b.y0, b.x0, b.y1, b.x1) } else { (b.x0, b.y0, b.x1, b.y1) }
}

/// The text size of a box in a frame: its height across the line.
fn size(r: (f32, f32, f32, f32)) -> f32 {
    (r.3 - r.1).max(4.0)
}

/// Whether `b` belongs to the block of `a`: the line right above or below it (beside it, for a
/// column), overlapping it, or another piece of the same line, in a similar text size.
fn joins(a: &TextBox, b: &TextBox) -> bool {
    let column = is_column(a);
    let (a, b) = (frame(a, column), frame(b, column));
    let (ha, hb) = (size(a), size(b));
    if ha.max(hb) > MAX_HEIGHT_RATIO * ha.min(hb) {
        return false;
    }
    let h = (ha + hb) / 2.0;
    if a.3.min(b.3) - a.1.max(b.1) >= 0.5 * ha.min(hb) {
        return a.0.max(b.0) - a.2.min(b.2) <= MAX_ROW_GAP * h;
    }
    let gap = (b.1 - a.3).max(a.1 - b.3);
    let overlap = a.2.min(b.2) - a.0.max(b.0);
    gap <= MAX_LINE_GAP * h && overlap > 0.0
}

/// The box the pointer is on: it lies inside it, give or take the slack. Of overlapping boxes,
/// the one whose middle across the line is closest to the pointer.
fn under_pointer(boxes: &[TextBox], x: f32, y: f32) -> Option<usize> {
    // The pointer's place along and across a box's line.
    let point = |b: &TextBox| if is_column(b) { (y, x) } else { (x, y) };
    boxes
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            let r = frame(b, is_column(b));
            let (along, across) = point(b);
            let h = size(r);
            along >= r.0 - POINTER_SLACK_ALONG * h
                && along <= r.2 + POINTER_SLACK_ALONG * h
                && across >= r.1 - POINTER_SLACK_ACROSS * h
                && across <= r.3 + POINTER_SLACK_ACROSS * h
        })
        .min_by(|(_, a), (_, b)| {
            let d = |t: &TextBox| {
                let r = frame(t, is_column(t));
                (point(t).1 - (r.1 + r.3) / 2.0).abs()
            };
            d(a).total_cmp(&d(b))
        })
        .map(|(i, _)| i)
}

/// The indices of the boxes in the block under the pointer, the pointed line first; empty when the
/// pointer is on no line of text.
pub fn under(boxes: &[TextBox], x: f32, y: f32) -> Vec<usize> {
    let Some(start) = under_pointer(boxes, x, y) else { return Vec::new() };
    let mut members = vec![start];
    let mut seen = vec![false; boxes.len()];
    seen[start] = true;
    let mut k = 0;
    while k < members.len() && members.len() < MAX_BLOCK_BOXES {
        let a = boxes[members[k]];
        for (j, b) in boxes.iter().enumerate() {
            if !seen[j] && joins(&a, b) {
                seen[j] = true;
                members.push(j);
                if members.len() == MAX_BLOCK_BOXES {
                    break;
                }
            }
        }
        k += 1;
    }
    members
}

/// Whether the lines read from a block are text: more than one letter or digit in all, as
/// Lookupper requires (`0x140947790`: the lines with words, summing `char.IsLetterOrDigit`).
/// Otherwise (a stray mark, an icon, a lone symbol) it reads every box on the screen.
pub fn reads_as_text<'a>(lines: impl IntoIterator<Item = &'a str>) -> bool {
    lines.into_iter().flat_map(str::chars).filter(|c| c.is_alphanumeric()).nth(1).is_some()
}

/// The rectangle around some boxes.
pub fn bounds(boxes: &[TextBox]) -> Option<TextBox> {
    let first = *boxes.first()?;
    Some(boxes.iter().fold(first, |r, b| TextBox { x0: r.x0.min(b.x0), y0: r.y0.min(b.y0), x1: r.x1.max(b.x1), y1: r.y1.max(b.y1) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x0: f32, y0: f32, x1: f32, y1: f32) -> TextBox {
        TextBox { x0, y0, x1, y1 }
    }

    #[test]
    fn a_paragraph_is_one_block_and_the_next_list_item_another() {
        let boxes = [
            b(100.0, 100.0, 600.0, 120.0), // item 1, line 1
            b(100.0, 126.0, 580.0, 146.0), // item 1, line 2
            b(100.0, 170.0, 620.0, 190.0), // item 2 after a full line of space
            b(100.0, 196.0, 300.0, 216.0),
        ];
        assert_eq!(under(&boxes, 200.0, 135.0), [1, 0]);
        assert_eq!(under(&boxes, 150.0, 180.0), [2, 3]);
    }

    #[test]
    fn pieces_of_a_row_join_and_a_bigger_title_stays_apart() {
        // The third piece is 25 px away (within 1.5 of the 20 px height); the fourth is 40 px away.
        let boxes = [b(100.0, 40.0, 400.0, 80.0), b(100.0, 90.0, 300.0, 110.0), b(325.0, 90.0, 500.0, 110.0), b(540.0, 90.0, 700.0, 110.0)];
        assert_eq!(under(&boxes, 150.0, 100.0), [1, 2]);
    }

    #[test]
    fn the_pointer_off_every_line_finds_no_block() {
        let boxes = [b(100.0, 100.0, 600.0, 120.0)];
        assert!(under(&boxes, 300.0, 160.0).is_empty());
        assert!(under(&boxes, 900.0, 110.0).is_empty());
        // Just past the end of the line, or between its words, is still on it.
        assert_eq!(under(&boxes, 605.0, 110.0), [0]);
    }

    #[test]
    fn a_column_is_measured_by_its_width_not_its_length() {
        // The user's Google Images screenshot: two columns of a Chinese text 427 px long and a
        // column of another picture 465 px to the left; the pointer was on the grey beside them.
        // Measured by their length, the pointer was "on" a column 213 px away and the block took
        // columns 640 px apart.
        let boxes = [b(1326.0, 284.0, 1361.0, 712.0), b(1375.0, 284.0, 1412.0, 710.0), b(773.0, 217.0, 860.0, 635.0)];
        assert!(under(&boxes, 1137.0, 443.0).is_empty());
        assert_eq!(under(&boxes, 1340.0, 443.0), [0, 1], "neighbouring columns join, the far picture does not");
        assert_eq!(under(&boxes, 1320.0, 443.0), [0, 1], "within a fifth of the column's width");
        assert!(under(&boxes, 1317.0, 443.0).is_empty());
        assert_eq!(under(&boxes, 1340.0, 725.0), [0, 1], "and half its width past its end");
    }

    #[test]
    fn a_block_of_one_letter_or_digit_at_most_is_not_text() {
        assert!(reads_as_text(["今日", "は"]));
        assert!(reads_as_text(["a", "b"]), "counted over all the lines");
        assert!(!reads_as_text(["a"]));
        assert!(!reads_as_text(["→ ✓", "", "7 —"]));
        assert!(!reads_as_text(Vec::<&str>::new()));
    }

    #[test]
    fn bounds_cover_the_block() {
        let r = bounds(&[b(10.0, 20.0, 30.0, 40.0), b(5.0, 45.0, 25.0, 60.0)]).unwrap();
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (5.0, 20.0, 30.0, 60.0));
        assert!(bounds(&[]).is_none());
    }
}
