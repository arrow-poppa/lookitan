// The lines of manga text as PP-OCRv6 Manga's recognizer should get them, ported from Fushi
// (hajisensai/Fushi, GPL-3.0 as Lookitan; packages/fushi_engine/lib/ocr/ocr_line_layout.dart, main
// on 2026-10-30), at the user's request on 2026-10-30. Fushi reads manga with the same recognizer
// and measured these steps as what makes it work: without the furigana filter, PP-OCR's error on
// Japanese manga was 41–92% (it reads the furigana into the text).
//
// The detected boxes are grouped into clusters (Fushi has a block detector; Lookitan groups boxes
// that touch, give or take 0.6 of the thicker one's thickness), and in each:
// 1. its direction is voted by the clear lines' lengths (a line 1.5× longer than thick), so a wide
//    balloon of short columns is still vertical;
// 2. Japanese only: furigana is dropped, a line thinner than 0.6 of the cluster's length-weighted
//    75th-percentile thickness, or thinner than 0.82 of it and hugging a line at least 1.25× as
//    thick on the furigana side (right of a column; above a row, or below it when a row of text is
//    right above that one), across a gap of at most 0.35 of the thicker line, overlapping it along;
// 3. pieces of one column (or row) the detector split are joined, repeated until nothing joins:
//    overlapping across by half the thinner, at most 3× the thicker apart along; two side by side
//    (overlapping along by half the shorter) only when they overlap across by 0.8 (one line found
//    twice).
// Turned boxes are left as they are.

use super::paddle::TextBox;

const ORIENTATION_RATIO: f32 = 1.5;
const THIN_RATIO: f32 = 0.6;
const SIDE_RATIO: f32 = 0.82;
const THICKER: f32 = 1.25;
const HUG: f32 = 0.35;
const MAX_GAP_RATIO: f32 = 3.0;
const OVERLAP_RATIO: f32 = 0.5;
const SIDE_BY_SIDE_RATIO: f32 = 0.5;
const DUPLICATE_RATIO: f32 = 0.8;
/// Boxes this close (in the thicker one's thickness) are one cluster.
const CLUSTER_GAP: f32 = 0.6;

/// The boxes of a page tidied for PP-OCRv6 Manga: per cluster, furigana dropped (`japanese`) and
/// split columns joined. Top to bottom, as detectors give them.
pub fn tidy(boxes: Vec<TextBox>, japanese: bool) -> Vec<TextBox> {
    let (turned, level): (Vec<TextBox>, Vec<TextBox>) = boxes.into_iter().partition(|b| b.quad.is_some());
    let mut out = turned;
    for cluster in clusters(&level) {
        let lines: Vec<TextBox> = cluster.into_iter().map(|i| level[i]).collect();
        let bounds = crate::block::bounds(&lines).unwrap_or(lines[0]);
        let vertical = vote_vertical(&lines).unwrap_or(bounds.y1 - bounds.y0 > bounds.x1 - bounds.x0);
        let lines = if japanese { drop_ruby(lines, vertical) } else { lines };
        out.extend(merge_fragments(lines, vertical));
    }
    out.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
    out
}

fn width(b: &TextBox) -> f32 {
    b.x1 - b.x0
}

fn height(b: &TextBox) -> f32 {
    b.y1 - b.y0
}

/// Across the line: a column's width, a row's height.
fn thickness(b: &TextBox, vertical: bool) -> f32 {
    if vertical { width(b) } else { height(b) }
}

/// Along the line: a column's height, a row's width.
fn length(b: &TextBox, vertical: bool) -> f32 {
    if vertical { height(b) } else { width(b) }
}

/// The boxes that touch, give or take `CLUSTER_GAP` of the thicker one's thickness, by index.
fn clusters(boxes: &[TextBox]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..boxes.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for i in 0..boxes.len() {
        for j in i + 1..boxes.len() {
            let (a, b) = (&boxes[i], &boxes[j]);
            let thick = width(a).min(height(a)).max(width(b).min(height(b)));
            let gap = CLUSTER_GAP * thick;
            let near = a.x0 - gap <= b.x1 && b.x0 - gap <= a.x1 && a.y0 - gap <= b.y1 && b.y0 - gap <= a.y1;
            if near {
                let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                parent[ri] = rj;
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut slot: Vec<Option<usize>> = vec![None; boxes.len()];
    for i in 0..boxes.len() {
        let r = root(&mut parent, i);
        match slot[r] {
            Some(g) => groups[g].push(i),
            None => {
                slot[r] = Some(groups.len());
                groups.push(vec![i]);
            }
        }
    }
    groups
}

/// The direction the clear lines vote for, by their lengths; `None` without a clear line.
fn vote_vertical(lines: &[TextBox]) -> Option<bool> {
    let (mut vertical, mut horizontal) = (0f32, 0f32);
    for b in lines {
        if height(b) >= ORIENTATION_RATIO * width(b) {
            vertical += height(b);
        } else if width(b) >= ORIENTATION_RATIO * height(b) {
            horizontal += width(b);
        }
    }
    (vertical > 0.0 || horizontal > 0.0).then_some(vertical >= horizontal)
}

/// The 75th percentile of the lines' thickness, each weighted by its length (furigana comes in
/// many short pieces, which would otherwise pull it down to their own thickness).
fn weighted_p75(lines: &[TextBox], vertical: bool) -> f32 {
    let mut rows: Vec<(f32, f32)> = lines.iter().map(|b| (thickness(b, vertical), length(b, vertical).max(1.0))).collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f32 = rows.iter().map(|r| r.1).sum();
    let mut seen = 0.0;
    for &(thick, len) in &rows {
        seen += len;
        if seen >= 0.75 * total {
            return thick;
        }
    }
    rows.last().map_or(0.0, |r| r.0)
}

/// The lines without the furigana.
fn drop_ruby(lines: Vec<TextBox>, vertical: bool) -> Vec<TextBox> {
    if lines.len() < 2 {
        return lines;
    }
    let p75 = weighted_p75(&lines, vertical);
    let hugs = |gap: f32, base: f32| gap >= -HUG * base && gap <= HUG * base;
    // A row of text right above the row `base`, thicker than the furigana `ruby`.
    let text_above = |base: usize, ruby: usize, own: f32| {
        lines.iter().enumerate().any(|(k, above)| {
            k != base && k != ruby && height(above) >= own * THICKER && hugs(lines[base].y0 - above.y1, height(&lines[base]))
                && above.x1.min(lines[base].x1) - above.x0.max(lines[base].x0) > 0.0
        })
    };
    let beside_thicker = |i: usize| {
        let line = &lines[i];
        let own = thickness(line, vertical);
        lines.iter().enumerate().any(|(j, other)| {
            let base = thickness(other, vertical);
            if j == i || base < own * THICKER {
                return false;
            }
            let on_ruby_side = if vertical {
                hugs(line.x0 - other.x1, base)
            } else {
                hugs(other.y0 - line.y1, base) || (hugs(line.y0 - other.y1, base) && text_above(j, i, own))
            };
            let along = if vertical { line.y1.min(other.y1) - line.y0.max(other.y0) } else { line.x1.min(other.x1) - line.x0.max(other.x0) };
            on_ruby_side && along > 0.0
        })
    };
    let keep: Vec<bool> = (0..lines.len())
        .map(|i| {
            let own = thickness(&lines[i], vertical);
            !(own < THIN_RATIO * p75 || (own < SIDE_RATIO * p75 && beside_thicker(i)))
        })
        .collect();
    lines.into_iter().zip(keep).filter_map(|(b, k)| k.then_some(b)).collect()
}

/// The pieces of each column (or row) joined, until no two join.
fn merge_fragments(lines: Vec<TextBox>, vertical: bool) -> Vec<TextBox> {
    let mut current = lines;
    loop {
        let next = merge_once(&current, vertical);
        if next.len() == current.len() {
            return next;
        }
        current = next;
    }
}

fn merge_once(lines: &[TextBox], vertical: bool) -> Vec<TextBox> {
    // (across start, across end, along start, along end)
    let axes = |b: &TextBox| if vertical { (b.x0, b.x1, b.y0, b.y1) } else { (b.y0, b.y1, b.x0, b.x1) };
    let mut sorted = lines.to_vec();
    sorted.sort_by(|a, b| {
        let (a, b) = (axes(a), axes(b));
        (a.0 + a.1).total_cmp(&(b.0 + b.1))
    });
    let mergeable = |last: &TextBox, b: &TextBox| {
        let (l, r) = (axes(last), axes(b));
        let overlap = l.1.min(r.1) - l.0.max(r.0);
        let (tl, tr) = (thickness(last, vertical), thickness(b, vertical));
        let (thinner, thicker) = (tl.min(tr), tl.max(tr));
        let along = l.3.min(r.3) - l.2.max(r.2);
        // A negative overlap along is the gap between them.
        if -along > MAX_GAP_RATIO * thicker {
            return false;
        }
        let shorter = (l.3 - l.2).min(r.3 - r.2);
        let side_by_side = shorter > 0.0 && along > SIDE_BY_SIDE_RATIO * shorter;
        let needed = if side_by_side { DUPLICATE_RATIO } else { OVERLAP_RATIO };
        thinner > 0.0 && overlap >= needed * thinner
    };
    let mut merged: Vec<TextBox> = Vec::new();
    for b in sorted {
        // From the latest back: another column's piece can sit between two of one column.
        match (0..merged.len()).rev().find(|&i| mergeable(&merged[i], &b)) {
            Some(i) => {
                let m = &mut merged[i];
                (m.x0, m.y0, m.x1, m.y1) = (m.x0.min(b.x0), m.y0.min(b.y0), m.x1.max(b.x1), m.y1.max(b.y1));
            }
            None => merged.push(b),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x0: f32, y0: f32, x1: f32, y1: f32) -> TextBox {
        TextBox { x0, y0, x1, y1, quad: None }
    }

    #[test]
    fn a_furigana_column_beside_its_text_is_dropped_and_the_text_columns_stay() {
        // Two columns of text 30 wide, the right one with furigana 12 wide at its right.
        let lines = vec![b(100.0, 0.0, 130.0, 300.0), b(140.0, 0.0, 170.0, 280.0), b(171.0, 20.0, 183.0, 80.0), b(171.0, 150.0, 183.0, 200.0)];
        assert_eq!(vote_vertical(&lines), Some(true));
        let kept = tidy(lines, true);
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept.iter().all(|k| k.x1 - k.x0 == 30.0));
    }

    #[test]
    fn furigana_is_kept_out_of_japanese_only_and_a_wide_balloon_of_short_columns_is_vertical() {
        let lines = vec![b(100.0, 0.0, 130.0, 50.0), b(132.0, 0.0, 162.0, 50.0), b(164.0, 0.0, 194.0, 50.0), b(195.0, 10.0, 207.0, 40.0)];
        assert_eq!(vote_vertical(&lines), Some(true), "short columns still vote vertical");
        assert_eq!(tidy(lines.clone(), false).len(), 4, "Chinese: nothing is furigana");
        assert_eq!(tidy(lines, true).len(), 3);
    }

    #[test]
    fn a_column_the_detector_split_is_one_again_but_columns_side_by_side_stay_apart() {
        let lines = vec![b(100.0, 0.0, 130.0, 120.0), b(102.0, 135.0, 129.0, 300.0), b(140.0, 0.0, 170.0, 300.0)];
        let mut kept = tidy(lines, true);
        kept.sort_by(|a, b| a.x0.total_cmp(&b.x0));
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert_eq!((kept[0].y0, kept[0].y1), (0.0, 300.0));
        // Pieces far apart along stay apart (two panels at one height).
        let apart = tidy(vec![b(0.0, 0.0, 200.0, 30.0), b(400.0, 0.0, 600.0, 30.0)], true);
        assert_eq!(apart.len(), 2);
    }

    #[test]
    fn a_second_subtitle_row_is_not_furigana_unless_a_row_of_text_sits_above_its_text() {
        // Horizontal: a smaller row right under a row, nothing above: a second subtitle row.
        let rows = vec![b(0.0, 0.0, 400.0, 40.0), b(0.0, 42.0, 300.0, 70.0)];
        assert_eq!(tidy(rows, true).len(), 2);
        // Furigana above a row is dropped.
        let ruby = vec![b(20.0, 0.0, 120.0, 14.0), b(0.0, 16.0, 400.0, 56.0), b(0.0, 60.0, 380.0, 100.0)];
        assert_eq!(tidy(ruby, true).len(), 2);
    }
}
