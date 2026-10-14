// The block of text under the pointer, found on the detected boxes before anything is recognized.
// In the automatic capture, as in Lookupper's pointer mode, the whole monitor is detected but only
// this block is recognized: the line under the pointer and the lines set close around it (a
// paragraph, a list item, a chat message). A pointer on no line finds no block, and the lookup
// reads the whole screen instead. The thresholds are our own.

use crate::ocr::paddle::TextBox;

/// Slack around a box for the pointer to count as on it, in box heights: above and below it, and
/// past its ends.
const POINTER_SLACK_Y: f32 = 0.2;
const POINTER_SLACK_X: f32 = 0.5;
/// Height ratio above which two boxes are different text (a title over its body).
const MAX_HEIGHT_RATIO: f32 = 1.6;
/// Vertical gap between two lines of one block, in line heights. Tighter than a paragraph's
/// (see words.rs), so list items and separate messages stay apart.
const MAX_LINE_GAP: f32 = 0.6;
/// Horizontal gap between two boxes of one row (a line the detector split), in line heights.
const MAX_ROW_GAP: f32 = 1.5;
/// Bounds the recognition work of one lookup.
const MAX_BLOCK_BOXES: usize = 40;

fn height(b: &TextBox) -> f32 {
    (b.y1 - b.y0).max(4.0)
}

fn same_row(a: &TextBox, b: &TextBox) -> bool {
    a.y1.min(b.y1) - a.y0.max(b.y0) >= 0.5 * height(a).min(height(b))
}

/// Whether `b` belongs to the block of `a`: the line right above or below it, overlapping it
/// horizontally, or another piece of the same row, in a similar text size.
fn joins(a: &TextBox, b: &TextBox) -> bool {
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

/// The box the pointer is on: it lies inside it, give or take the slack. Of overlapping boxes,
/// the one whose middle is closest to the pointer.
fn under_pointer(boxes: &[TextBox], x: f32, y: f32) -> Option<usize> {
    boxes
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            let h = height(b);
            x >= b.x0 - POINTER_SLACK_X * h
                && x <= b.x1 + POINTER_SLACK_X * h
                && y >= b.y0 - POINTER_SLACK_Y * h
                && y <= b.y1 + POINTER_SLACK_Y * h
        })
        .min_by(|(_, a), (_, b)| {
            let d = |t: &TextBox| (y - (t.y0 + t.y1) / 2.0).abs();
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
        let mut second = under(&boxes, 150.0, 180.0);
        second.sort();
        assert_eq!(second, [2, 3]);
    }

    #[test]
    fn pieces_of_a_row_join_and_a_bigger_title_stays_apart() {
        let boxes = [b(100.0, 40.0, 400.0, 80.0), b(100.0, 90.0, 300.0, 110.0), b(320.0, 90.0, 500.0, 110.0)];
        let mut block = under(&boxes, 150.0, 100.0);
        block.sort();
        assert_eq!(block, [1, 2]);
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
    fn bounds_cover_the_block() {
        let r = bounds(&[b(10.0, 20.0, 30.0, 40.0), b(5.0, 45.0, 25.0, 60.0)]).unwrap();
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (5.0, 20.0, 30.0, 60.0));
        assert!(bounds(&[]).is_none());
    }
}
