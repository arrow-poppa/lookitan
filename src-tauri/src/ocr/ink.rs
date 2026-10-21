// The ink of a recognized line: which pixels of its crop are text rather than background. The
// recognizer only says roughly where each character is (one CTC step per few pixels), so word
// edges and the line's height are snapped to this ink, as Lookupper's are.

/// Below this contrast between text and background the crop has no clear ink, and the
/// recognizer's estimates are kept.
const MIN_CONTRAST: u8 = 40;
/// A pixel is ink when it differs from the background by this share of the strongest contrast,
/// which keeps most of an anti-aliased stroke.
const INK_SHARE: f32 = 0.4;
/// A row belongs to the line's ink when it holds at least this share of the fullest row's ink,
/// so a descender reaching in from the line above does not count.
const ROW_SHARE: usize = 12;
/// How far past a line's first or last character centre its ink may reach, in CTC steps.
const END_REACH: f32 = 3.0;

/// The ink of a crop, in crop pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Ink {
    /// Whether each column has ink within the line's rows.
    pub cols: Vec<bool>,
    /// The rows of the line's ink, `y0..y1`.
    pub y0: u32,
    pub y1: u32,
}

/// Finds the ink in a crop given as luminance, row by row. `None` when the crop has no clear ink.
pub fn measure(lum: &[u8], w: usize, h: usize) -> Option<Ink> {
    if w == 0 || h == 0 || lum.len() < w * h {
        return None;
    }
    let lum = &lum[..w * h];
    // The background is the most common half of the crop: its median luminance.
    let mut hist = [0usize; 256];
    for &l in lum {
        hist[l as usize] += 1;
    }
    let (mut seen, mut background) = (0usize, 0u8);
    for (value, &count) in hist.iter().enumerate() {
        seen += count;
        if seen * 2 >= lum.len() {
            background = value as u8;
            break;
        }
    }
    let contrast = lum.iter().map(|&l| l.abs_diff(background)).max().unwrap_or(0);
    if contrast < MIN_CONTRAST {
        return None;
    }
    let threshold = ((contrast as f32 * INK_SHARE) as u8).max(MIN_CONTRAST / 2);
    let ink: Vec<bool> = lum.iter().map(|&l| l.abs_diff(background) >= threshold).collect();

    let rows: Vec<usize> = ink.chunks(w).map(|row| row.iter().filter(|&&i| i).count()).collect();
    let fullest = rows.iter().copied().max().unwrap_or(0);
    let inked: Vec<bool> = rows.iter().map(|&n| n > 0 && n * ROW_SHARE >= fullest).collect();
    // The line is the run of inked rows holding the most ink; a gap of a row or two (the dot
    // of an "i", a thin stroke) does not split it.
    let gap = (h / 20).max(1);
    let (mut best, mut best_ink) = (None, 0usize);
    let mut y = 0;
    while y < h {
        if !inked[y] {
            y += 1;
            continue;
        }
        let (start, mut end, mut total, mut blank) = (y, y, 0usize, 0usize);
        while y < h && blank <= gap {
            if inked[y] {
                end = y + 1;
                total += rows[y];
                blank = 0;
            } else {
                blank += 1;
            }
            y += 1;
        }
        if total > best_ink {
            best_ink = total;
            best = Some((start, end));
        }
        y = end;
    }
    let (y0, y1) = best?;
    let cols = (0..w).map(|x| (y0..y1).any(|y| ink[y * w + x])).collect();
    Some(Ink { cols, y0: y0 as u32, y1: y1 as u32 })
}

/// The horizontal span of each character of a line, in image pixels.
///
/// `centers` are the characters' CTC step centres and `blank` marks the spaces among them. The
/// line's box runs from `lo` to `hi`, one CTC step is `step` pixels wide, and the crop's first
/// column is at `origin`. Without ink, each boundary is halfway between two centres. With ink,
/// the boundary between two characters is the widest blank gap in the middle half of the way
/// between their centres, and the line's first and last characters end at its first and last ink,
/// so a word's box hugs its letters. A gap nearer a centre is inside a character (那, 你 or 川
/// over a busy background) and is not a boundary; with no gap in the middle the boundary stays
/// halfway.
pub fn spans(centers: &[f32], blank: &[bool], ink: Option<&Ink>, origin: f32, lo: f32, hi: f32, step: f32) -> Vec<(f32, f32)> {
    let n = centers.len();
    let mut out: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let left = if i == 0 { lo.max(centers[0] - step * 2.0) } else { (centers[i - 1] + centers[i]) / 2.0 };
            let right = if i + 1 == n { hi.min(centers[i] + step * 2.0) } else { (centers[i] + centers[i + 1]) / 2.0 };
            (left, right)
        })
        .collect();
    let Some(ink) = ink else { return out };
    let cols = &ink.cols;
    if cols.is_empty() {
        return out;
    }
    let last_col = cols.len() as isize - 1;
    let col = |x: f32| ((x - origin).floor() as isize).clamp(0, last_col) as usize;
    let at = |k: usize| origin + k as f32;
    let glyphs: Vec<usize> = (0..n).filter(|&i| !blank.get(i).copied().unwrap_or(false)).collect();

    for pair in glyphs.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        // The widest run of blank columns in the middle half between the two centres...
        let quarter = (centers[b] - centers[a]) / 4.0;
        let (from, to) = (col(centers[a] + quarter), col(centers[b] - quarter));
        let (mut run, mut best) = (None::<usize>, None::<(usize, usize)>);
        for k in from..=to {
            if !cols[k] {
                let start = *run.get_or_insert(k);
                if best.map_or(true, |(s, e)| k + 1 - start > e - s) {
                    best = Some((start, k + 1));
                }
            } else {
                run = None;
            }
        }
        let Some((mut s, mut e)) = best else { continue };
        // ...grown through the blank columns past a centre that fell beside a thin stroke, but
        // never past either character's own estimated far edge.
        let (floor, ceil) = (col(out[a].0), col(out[b].1));
        while s > floor && !cols[s - 1] {
            s -= 1;
        }
        while e <= ceil && e <= last_col as usize && !cols[e] {
            e += 1;
        }
        if s <= floor || e > ceil {
            continue;
        }
        out[a].1 = at(s);
        out[b].0 = at(e);
        // Spaces between them share the gap.
        let spaces = b - a - 1;
        if spaces > 0 {
            let width = (out[b].0 - out[a].1) / spaces as f32;
            for (j, i) in (a + 1..b).enumerate() {
                out[i] = (out[a].1 + width * j as f32, out[a].1 + width * (j + 1) as f32);
            }
        }
    }
    if let (Some(&first), Some(&last)) = (glyphs.first(), glyphs.last()) {
        let reach = step * END_REACH;
        let (from, to) = (col(lo.max(centers[first] - reach)), col(centers[first]));
        if let Some(k) = (from..=to).find(|&k| cols[k]) {
            out[first].0 = at(k);
        }
        let (from, to) = (col(centers[last]), col(hi.min(centers[last] + reach)));
        if let Some(k) = (from..=to).rev().find(|&k| cols[k]) {
            out[last].1 = at(k + 1);
        }
    }
    for span in &mut out {
        span.0 = span.0.clamp(lo, hi);
        span.1 = span.1.clamp(span.0, hi);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crop `w` x 20 with dark ink on white in the given columns and rows 5..15.
    fn crop(w: usize, ink_cols: &[std::ops::Range<usize>]) -> Vec<u8> {
        let mut lum = vec![250u8; w * 20];
        for y in 5..15 {
            for r in ink_cols {
                for x in r.clone() {
                    lum[y * w + x] = 20;
                }
            }
        }
        lum
    }

    #[test]
    fn ink_rows_skip_the_padding_and_a_descender_from_the_line_above() {
        let mut lum = crop(60, &[2..20, 30..55]);
        lum[25] = 20; // one pixel of the line above, over the gap
        let ink = measure(&lum, 60, 20).unwrap();
        assert_eq!((ink.y0, ink.y1), (5, 15));
        assert!(ink.cols[2] && ink.cols[19] && ink.cols[30]);
        assert!(!ink.cols[25], "the stray pixel is outside the line's rows");
        assert!(measure(&vec![128u8; 600], 60, 10).is_none(), "a flat crop has no ink");
    }

    #[test]
    fn word_edges_snap_to_the_gap_and_the_line_ends() {
        // "ab cd": glyphs at 10..20, 21..30, 40..50, 51..60 of a crop starting at x = 100.
        let lum = crop(80, &[10..20, 21..30, 40..50, 51..60]);
        let ink = measure(&lum, 80, 20).unwrap();
        let centers = [116.0, 125.0, 135.0, 145.0, 155.0];
        let blank = [false, false, true, false, false];
        let spans = spans(&centers, &blank, Some(&ink), 100.0, 100.0, 180.0, 4.0);
        assert_eq!(spans[0].0, 110.0, "the line starts at its first ink");
        assert_eq!((spans[0].1, spans[1].0), (120.0, 121.0), "letters split at the gap between them");
        assert_eq!((spans[1].1, spans[3].0), (130.0, 140.0), "the word ends where its ink ends");
        assert_eq!(spans[2], (130.0, 140.0), "the space fills the gap");
        assert_eq!(spans[4].1, 160.0, "the line ends at its last ink");
    }

    #[test]
    fn a_gap_inside_a_character_is_not_a_boundary() {
        // 那 is ink at 10..25 and 28..40 with its own gap between; 你 follows at 40..60 with no
        // blank column before it, as over a video behind a subtitle.
        let lum = crop(70, &[10..25, 28..60]);
        let ink = measure(&lum, 70, 20).unwrap();
        let spans = spans(&[126.0, 150.0], &[false, false], Some(&ink), 100.0, 100.0, 170.0, 6.0);
        assert_eq!((spans[0].1, spans[1].0), (138.0, 138.0), "the boundary stays halfway, not in 那's gap");
        assert_eq!(spans[0].0, 110.0);
    }

    #[test]
    fn without_ink_the_boundaries_stay_halfway() {
        let spans = spans(&[10.0, 20.0, 30.0], &[false, true, false], None, 0.0, 0.0, 40.0, 4.0);
        assert_eq!(spans, vec![(2.0, 15.0), (15.0, 25.0), (25.0, 38.0)]);
    }
}
