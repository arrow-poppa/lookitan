// The block of text under the pointer, found on the detected boxes before anything is recognized.
// In the automatic capture, as in Lookupper's pointer mode, the whole monitor is detected but only
// this block is recognized. A pointer on no box finds no block, and neither does a block that
// reads as at most one letter or digit (`reads_as_text`): the lookup reads the whole screen
// instead. The rules and thresholds are Lookupper's (REA: XpressOCR's `0x1409505c0`, its
// pointer test `0x140950940` and its clustering `0x14094b8c0`).

use std::collections::VecDeque;

use crate::ocr::paddle::TextBox;

/// The pointer is on a box when it lies inside it with the box's short side (the text's size)
/// widened by this share on each side; there is no slack past the ends of a line or a column.
const POINTER_SLACK: f32 = 0.2;
/// Size ratio (of the boxes' short sides) above which two boxes are different text, unless the
/// smaller one is ruby: above a row, or right of a column.
const MAX_SIZE_RATIO: f64 = 1.6;
/// How far a box reaches for others: this share of its short side across the text (sideways for
/// a row, up and down for a column) and of its height up and down.
const REACH_ACROSS: f64 = 0.7;
const REACH_DOWN: f64 = 0.5;

/// A box in whole pixels, as OpenCV's `RotatedRect.BoundingRect()` gives it to Lookupper: its
/// far edges rounded up and counted in.
#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

fn rect(b: &TextBox) -> Rect {
    let (x, y) = (b.x0.floor() as i32, b.y0.floor() as i32);
    Rect { x, y, w: b.x1.ceil() as i32 - x + 1, h: b.y1.ceil() as i32 - y + 1 }
}

/// The first box the pointer is on.
fn under_pointer(boxes: &[TextBox], x: f32, y: f32) -> Option<usize> {
    boxes.iter().position(|b| {
        let (w, h) = (b.x1 - b.x0, b.y1 - b.y0);
        let (sx, sy) = if w < h { (POINTER_SLACK * w, 0.0) } else { (0.0, POINTER_SLACK * h) };
        x >= b.x0 - sx && x <= b.x1 + sx && y >= b.y0 - sy && y <= b.y1 + sy
    })
}

/// Whether box `a` takes `b` into its block: `b` touches `a` grown by its reach, and is of a similar
/// size, or is ruby of `a` (smaller, above a row or right of a column), or `a` is ruby of it.
fn joins(a: Rect, b: Rect) -> bool {
    let vertical = a.w < a.h;
    let across = (if vertical { a.w } else { a.h } as f64 * REACH_ACROSS) as i32;
    let down = (a.h as f64 * REACH_DOWN) as i32;
    let (x, y, w, h) = (a.x - across, a.y - down, a.w + 2 * across, a.h + 2 * down);
    if !(x < b.x + b.w && b.x < x + w && y < b.y + b.h && b.y < y + h) {
        return false;
    }
    let (sa, sb) = (a.w.min(a.h) as f64, b.w.min(b.h) as f64);
    if sb * MAX_SIZE_RATIO < sa {
        return if vertical { a.x <= b.x } else { b.y <= a.y };
    }
    if sb > sa * MAX_SIZE_RATIO {
        return if vertical { b.x <= a.x } else { a.y <= b.y };
    }
    true
}

/// The block of every box, as Lookupper labels them: in order, each box not yet in a block starts
/// one, which grows through every box its members take.
fn blocks(boxes: &[TextBox]) -> Vec<usize> {
    let rects: Vec<Rect> = boxes.iter().map(rect).collect();
    let mut label = vec![usize::MAX; boxes.len()];
    let mut next = 0;
    for i in 0..boxes.len() {
        if label[i] != usize::MAX {
            continue;
        }
        label[i] = next;
        let mut queue = VecDeque::from([i]);
        while let Some(k) = queue.pop_front() {
            for j in 0..boxes.len() {
                if label[j] == usize::MAX && joins(rects[k], rects[j]) {
                    label[j] = next;
                    queue.push_back(j);
                }
            }
        }
        next += 1;
    }
    label
}

/// The indices of the boxes in the block under the pointer, the pointed box first; empty when the
/// pointer is on no box.
pub fn under(boxes: &[TextBox], x: f32, y: f32) -> Vec<usize> {
    let Some(start) = under_pointer(boxes, x, y) else { return Vec::new() };
    let label = blocks(boxes);
    std::iter::once(start).chain((0..boxes.len()).filter(|&j| j != start && label[j] == label[start])).collect()
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
    fn close_pieces_of_a_row_join_and_a_bigger_title_stays_apart() {
        let boxes = [b(100.0, 40.0, 400.0, 80.0), b(100.0, 90.0, 300.0, 110.0), b(310.0, 90.0, 500.0, 110.0), b(540.0, 90.0, 700.0, 110.0)];
        // The third piece is 10 px away (within 0.7 of the 21 px height); the fourth is 40 px away.
        assert_eq!(under(&boxes, 150.0, 100.0), [1, 2]);
    }

    #[test]
    fn ruby_joins_the_text_it_reads_but_a_small_caption_below_does_not() {
        let boxes = [b(100.0, 100.0, 400.0, 140.0), b(120.0, 85.0, 200.0, 98.0), b(120.0, 145.0, 200.0, 158.0)];
        assert_eq!(under(&boxes, 300.0, 120.0), [0, 1]);
        // A column with ruby to its right.
        let column = [b(100.0, 100.0, 140.0, 400.0), b(142.0, 120.0, 155.0, 200.0), b(85.0, 120.0, 98.0, 200.0)];
        assert_eq!(under(&column, 120.0, 300.0), [0, 1]);
    }

    #[test]
    fn the_pointer_off_every_box_finds_no_block() {
        let boxes = [b(100.0, 100.0, 600.0, 120.0)];
        assert!(under(&boxes, 300.0, 160.0).is_empty());
        assert!(under(&boxes, 900.0, 110.0).is_empty());
        assert!(under(&boxes, 605.0, 110.0).is_empty(), "no slack past the end of a line");
        // A fifth of the line's height above or below it is still on it.
        assert_eq!(under(&boxes, 300.0, 123.0), [0]);
    }

    #[test]
    fn a_column_is_measured_by_its_width_not_its_length() {
        // The user's Google Images screenshot: two columns of a Chinese text 427 px long and a
        // column of another picture 465 px to the left; the pointer was on the grey beside them.
        let boxes = [b(1326.0, 284.0, 1361.0, 712.0), b(1375.0, 284.0, 1412.0, 710.0), b(773.0, 217.0, 860.0, 635.0)];
        assert!(under(&boxes, 1137.0, 443.0).is_empty());
        assert_eq!(under(&boxes, 1340.0, 443.0), [0, 1], "neighbouring columns join, the far picture does not");
        assert_eq!(under(&boxes, 1320.0, 443.0), [0, 1], "within a fifth of the column's width");
        assert!(under(&boxes, 1317.0, 443.0).is_empty());
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
