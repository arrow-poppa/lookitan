//! Paragraphs as Lookupper finds them: the layout analyzer of its OCR (REA `0x140952880`,
//! decoded on 2026-10-21 and 2026-10-22), which groups a capture's lines into paragraphs.
//!
//! Each line is paired with the nearest line below it (`pairs`). A score adds up what speaks
//! against the two being one paragraph and takes off what speaks for it (`score`); under 1 they
//! are joined. Besides the lines' text and boxes, the score reads:
//! - regions (`regions`): runs of lines alike in size and place, each of a `Kind` (prose,
//!   list-like, centred, framed or other);
//! - the colour and the height of each line's ink, measured on the capture (`look`);
//! - frames: the parts of the capture that edges enclose, such as buttons, cards and bubbles
//!   (`frames`).
//!
//! Lines are compared in the frame words.rs's `upright` gives them: in a column of vertical text,
//! "below" is the next column to the left.

use image::RgbaImage;

use crate::ocr::paddle::is_cjk_class;
use crate::ocr::{Flow, OcrLine};

/// What the capture shows of a line (`look`): its ink, and the frame around it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    /// The rows (the columns of a vertical line) holding 85% of its ink, in capture pixels; 0 when
    /// not measured.
    pub ink: f32,
    /// The mean colour of its ink, RGB.
    pub color: Option<[f32; 3]>,
    /// The part of the capture between edges that holds it.
    pub frame: Option<Frame>,
}

/// A part of the capture that edges enclose (Lookupper's line fields `+0x40`, `+0x53`, `+0x54`
/// and `+0x78`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Which part: the same number for the lines of one `look` run that it holds.
    pub id: u32,
    /// Whether it fills at least 60% of the line's box.
    pub filled: bool,
    /// Whether it is under half the analyzed area: a box, not the background.
    pub small: bool,
    /// Its size, in capture pixels.
    pub w: f32,
    pub h: f32,
}

/// Japanese particles and okurigana a line ends in when its sentence goes on (`0x140953800`).
const JAPANESE_GOES_ON: &str = "てでとがはをにもへやのらどしつ";
/// Small kana and the long vowel mark, which never end a sentence either.
const SMALL_KANA: &str = "ーッャュョぁぃぅぇぉっゃゅょ";
/// Chinese function words a line ends in when its sentence goes on.
const CHINESE_GOES_ON: &str = "的和与及或而但因於";
/// Bullets that start a list item.
const BULLETS: &str = "•·●○◦▪▫■□◆◇‣⁃";

/// A line's features (Lookupper's analyzer line, `0x140952e20` and `0x140953340`).
struct Line {
    flow: Flow,
    /// Its box in the frame of `upright`: left, right, top, bottom.
    l: f32,
    r: f32,
    t: f32,
    b: f32,
    /// Its words along it, by start: Lookupper's OCR words, a character each in a line all CJK.
    words: Vec<(f32, f32)>,
    /// Its words between spaces.
    spaced_words: usize,
    /// Its kana, hanzi and hangul.
    cjk: usize,
    /// What its first letter or digit is.
    lower_start: bool,
    upper_start: bool,
    digit_start: bool,
    /// It starts as a list item: a bullet, a dash, ①, "1." or "a)".
    marker: bool,
    /// How it ends.
    sentence_end: bool,
    hyphen_end: bool,
    comma_end: bool,
    colon_end: bool,
    goes_on: bool,
    /// "Name: value".
    key_value: bool,
    /// More brackets opened than closed.
    open_bracket: bool,
    /// Two letters or more, all capitals.
    all_caps: bool,
    letter: bool,
    /// A letter with case.
    cased: bool,
    look: Look,
    region: usize,
}

impl Line {
    fn h(&self) -> f32 {
        self.b - self.t
    }

    fn w(&self) -> f32 {
        self.r - self.l
    }

    fn center(&self) -> f32 {
        (self.t + self.b) * 0.5
    }
}

/// How a region reads (`0x1409575c0`). Lookupper's numbers are 0 to 4, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Prose,
    List,
    Centred,
    Framed,
    Other,
}

/// A run of lines alike in size and place (`0x140956b60`, `0x140957320`), with what
/// `0x1409575c0` finds of it.
struct Region {
    /// Its lines, top to bottom.
    lines: Vec<usize>,
    kind: Kind,
    /// Median distance between its lines' centres (`+0x14`).
    pitch: f32,
    /// Median line height (`+0x18`).
    height: f32,
    /// Median gap between two words of a line (`+0x1c`).
    word_gap: f32,
    /// The rightmost line end, and the next one (`+0x20`, `+0x24`).
    edge: f32,
    second_edge: f32,
    /// Of the lines that end short of the edge, the share whose next line's first word would have
    /// fitted (`+0x28`).
    early: f32,
    /// Its lines start with a capital or a digit (`+0x30`).
    capitals: bool,
    /// A list with its markers (`+0x32`), and where its other lines start (`+0x2c`).
    list: bool,
    indent: Option<f32>,
}

/// Each line's paragraph, numbered in the lines' order.
pub fn paragraphs(lines: &[OcrLine]) -> Vec<usize> {
    let n = lines.len();
    let mut ls: Vec<Line> = lines.iter().map(features).collect();
    let pairs = pairs(&ls);
    let regions = regions(&mut ls, &pairs);
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let join = |parent: &mut [usize], a: usize, b: usize| {
        let (ra, rb) = (root(parent, a), root(parent, b));
        if ra != rb {
            parent[ra] = rb;
        }
    };
    for &(a, b) in &pairs {
        if score(&ls, &regions, a, b) < 1.0 {
            join(&mut parent, a, b);
        }
    }
    // Lookitan's own: pieces of one row that the detector split are one paragraph too.
    for a in 0..n {
        for b in a + 1..n {
            if row_pieces(&ls[a], &ls[b]) {
                join(&mut parent, a, b);
            }
        }
    }
    let mut ids: Vec<usize> = Vec::with_capacity(n);
    let mut roots: Vec<usize> = Vec::new();
    for i in 0..n {
        let r = root(&mut parent, i);
        let id = roots.iter().position(|&x| x == r).unwrap_or_else(|| {
            roots.push(r);
            roots.len() - 1
        });
        ids.push(id);
    }
    ids
}

/// Horizontal gap between two pieces of one row (a line the OCR split), in line heights.
const MAX_ROW_GAP: f32 = 1.5;
/// Height ratio above which two pieces of a row are different text.
const MAX_ROW_HEIGHT_RATIO: f32 = 1.6;

/// Whether `a` and `b` are pieces of one row, in a similar text size.
fn row_pieces(a: &Line, b: &Line) -> bool {
    let (ha, hb) = (a.h().max(4.0), b.h().max(4.0));
    a.flow == b.flow
        && ha.max(hb) <= MAX_ROW_HEIGHT_RATIO * ha.min(hb)
        && a.b.min(b.b) - a.t.max(b.t) >= 0.5 * ha.min(hb)
        && a.l.max(b.l) - a.r.min(b.r) <= MAX_ROW_GAP * (ha + hb) / 2.0
}

/// A line's box in the frame of words.rs's `upright`: left, right, top, bottom.
fn frame_box(l: &OcrLine) -> (f32, f32, f32, f32) {
    match l.flow {
        Flow::Across => (l.x0, l.x1, l.y0, l.y1),
        Flow::Down => (l.y0, l.y1, -l.x1, -l.x0),
        Flow::Up => (-l.y1, -l.y0, l.x0, l.x1),
    }
}

/// A digit, as .NET's `char.IsDigit` (Lookupper's category 8) reads one.
fn is_digit(c: char) -> bool {
    if (c as u32) < 0x100 {
        c.is_ascii_digit()
    } else {
        c.is_numeric()
    }
}

fn features(l: &OcrLine) -> Line {
    let (left, right, top, bottom) = frame_box(l);
    let text = l.text.trim();
    let cs: Vec<char> = text.chars().collect();
    let last = cs.last().copied();
    let mut line = Line {
        flow: l.flow,
        l: left,
        r: right,
        t: top,
        b: bottom,
        words: words(l),
        spaced_words: text.split(' ').filter(|w| !w.is_empty()).count(),
        cjk: cs.iter().filter(|&&c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF)).count(),
        lower_start: false,
        upper_start: false,
        digit_start: false,
        marker: list_marker(&cs),
        sentence_end: last.is_some_and(|c| ".!?。！？…".contains(c)),
        hyphen_end: last.is_some_and(|c| matches!(c, '-' | '\u{2010}' | '\u{2013}' | '\u{2014}')),
        comma_end: last.is_some_and(|c| ",;、，；·・".contains(c)),
        colon_end: matches!(last, Some(':' | '：')),
        goes_on: l.text.trim_end().chars().last().is_some_and(|c| JAPANESE_GOES_ON.contains(c) || SMALL_KANA.contains(c) || CHINESE_GOES_ON.contains(c)),
        key_value: cs.iter().position(|&c| c == ':' || c == '：').is_some_and(|i| i > 0 && i + 1 < cs.len() && cs[0].is_alphabetic()),
        open_bracket: cs.iter().map(|&c| match c {
            '(' | '[' | '{' | '《' | '「' | '『' | '（' => 1,
            ')' | ']' | '}' | '》' | '」' | '』' | '）' => -1,
            _ => 0,
        }).sum::<i32>() > 0,
        all_caps: {
            let letters: Vec<char> = cs.iter().copied().filter(|c| c.is_alphabetic()).collect();
            letters.len() > 1 && letters.iter().all(|c| c.is_uppercase())
        },
        letter: cs.iter().any(|c| c.is_alphabetic()),
        cased: cs.iter().any(|c| c.is_uppercase() || c.is_lowercase()),
        look: l.look,
        region: 0,
    };
    if let Some(&c) = cs.iter().find(|&&c| c.is_alphabetic() || is_digit(c)) {
        if is_digit(c) {
            line.digit_start = true;
        } else {
            line.lower_start = c.is_lowercase();
            line.upper_start = c.is_uppercase();
        }
    }
    line
}

/// A line's words along it, by start, as Lookupper's OCR words: runs between spaces, or a
/// character each in a line all CJK.
fn words(l: &OcrLine) -> Vec<(f32, f32)> {
    let span = |x0: f32, x1: f32| if l.flow == Flow::Up { (-x1, -x0) } else { (x0, x1) };
    let all_cjk = l.chars.iter().filter(|c| !c.ch.is_whitespace()).all(|c| is_cjk_class(c.ch));
    let mut out: Vec<(f32, f32)> = Vec::new();
    let mut run: Option<(f32, f32)> = None;
    for c in &l.chars {
        if c.ch.is_whitespace() {
            out.extend(run.take());
            continue;
        }
        let (a, b) = span(c.x0, c.x1);
        if all_cjk {
            out.push((a, b));
        } else {
            run = Some(run.map_or((a, b), |(s, e)| (s.min(a), e.max(b))));
        }
    }
    out.extend(run);
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Whether a line starts as a list item (`0x1409536e0`).
fn list_marker(cs: &[char]) -> bool {
    let Some(&c) = cs.first() else { return false };
    if BULLETS.contains(c) || ('\u{2460}'..='\u{2473}').contains(&c) {
        return true;
    }
    if matches!(c, '*' | '-' | '\u{2013}' | '\u{2014}') && cs.get(1) == Some(&' ') {
        return true;
    }
    cs.len() >= 3 && (c.is_alphabetic() || is_digit(c)) && matches!(cs[1], ')' | '.') && matches!(cs[2], ' ' | '-' | '\u{2013}')
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n % 2 == 0 { (v[n / 2 - 1] + v[n / 2]) * 0.5 } else { v[n / 2] }
}

/// The mean distance from the mean (`0x1409590c0`).
fn spread(v: &[f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    v.iter().map(|x| (x - mean).abs()).sum::<f32>() / v.len() as f32
}

/// Each line with the nearest line below it (`0x140959e80`): running the same way, its centre
/// lower, overlapping it across by a fifth of the narrower line (or 3 pixels), and at most 2.2
/// line heights down.
fn pairs(lines: &[Line]) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|&a, &b| lines[a].center().total_cmp(&lines[b].center()));
    let mut out = Vec::new();
    for (k, &a) in order.iter().enumerate() {
        let la = &lines[a];
        let mut best: (f32, Option<usize>) = (f32::MAX, None);
        for &b in &order[k + 1..] {
            let lb = &lines[b];
            if lb.flow != la.flow || lb.center() <= la.center() {
                continue;
            }
            let reach = la.h().min(lb.h()) * 2.2;
            if la.b + reach < lb.t {
                break;
            }
            let overlap = la.r.min(lb.r) - la.l.max(lb.l);
            if overlap < 0.2 * la.w().min(lb.w()) && overlap < 3.0 {
                continue;
            }
            let gap = lb.t - la.b;
            if gap <= reach && gap < best.0 {
                best = (gap, Some(b));
            }
        }
        if let Some(b) = best.1 {
            out.push((a, b));
        }
    }
    out
}

/// The larger of the two lines' height over the smaller, or of their ink's when both were
/// measured and that is less (`0x140957460`).
fn size_ratio(a: &Line, b: &Line) -> f32 {
    let (ha, hb) = (a.h(), b.h());
    let ratio = ha.max(hb) / ha.min(hb).max(1.0);
    let (ia, ib) = (a.look.ink, b.look.ink);
    if ia > 1.0 && ib > 1.0 { ratio.min(ia.max(ib) / ia.min(ib)) } else { ratio }
}

/// Whether two lines are in different frames that both fill them, one of them (or, with
/// `both_small`, each) a box rather than the background.
fn apart_frames(a: &Line, b: &Line, both_small: bool) -> bool {
    match (a.look.frame, b.look.frame) {
        (Some(fa), Some(fb)) => {
            fa.id != fb.id && fa.filled && fb.filled && if both_small { fa.small && fb.small } else { fa.small || fb.small }
        }
        _ => false,
    }
}

/// The regions (`0x140956b60`): each pair of lines close, overlapping and alike in size is linked,
/// the links make runs top to bottom, and a run is cut where its lines are much further apart than
/// usual. Then each region is read (`classify`). Sets each line's `region`.
fn regions(lines: &mut [Line], pairs: &[(usize, usize)]) -> Vec<Region> {
    let n = lines.len();
    let mut next: Vec<Option<usize>> = vec![None; n];
    for &(a, b) in pairs {
        let (la, lb) = (&lines[a], &lines[b]);
        // In one box, lines may be further apart and differ more in size.
        let boxed = matches!((la.look.frame, lb.look.frame), (Some(fa), Some(fb)) if fa.id == fb.id && fa.small);
        let h = la.h().min(lb.h());
        if lb.t - la.b > if boxed { 1.8 } else { 1.3 } * h {
            continue;
        }
        if la.r.min(lb.r) - la.l.max(lb.l) < 0.45 * la.w().min(lb.w()) {
            continue;
        }
        if size_ratio(la, lb) > if boxed { 1.6 } else { 1.35 } || apart_frames(la, lb, true) {
            continue;
        }
        next[a] = Some(b);
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| lines[a].center().total_cmp(&lines[b].center()));
    let mut seen = vec![false; n];
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for &i in &order {
        let mut run = Vec::new();
        let mut at = Some(i);
        while let Some(k) = at {
            if seen[k] {
                break;
            }
            seen[k] = true;
            run.push(k);
            at = next[k];
        }
        if !run.is_empty() {
            runs.push(run);
        }
    }
    let mut parts: Vec<Vec<usize>> = Vec::new();
    for run in runs {
        if run.len() <= 2 {
            parts.push(run);
            continue;
        }
        let steps: Vec<f32> = run.windows(2).map(|w| lines[w[1]].center() - lines[w[0]].center()).collect();
        let step = median(steps.clone());
        let height = median(run.iter().map(|&i| lines[i].h()).collect());
        let mut part = vec![run[0]];
        for k in 1..run.len() {
            let d = steps[k - 1];
            if d > 1.3 * step && d > step + 0.3 * height {
                parts.push(std::mem::take(&mut part));
            }
            part.push(run[k]);
        }
        parts.push(part);
    }
    let mut regions: Vec<Region> = Vec::new();
    for mut part in parts {
        part.sort_by(|&a, &b| lines[a].center().total_cmp(&lines[b].center()).then(a.cmp(&b)));
        for &i in &part {
            lines[i].region = regions.len();
        }
        regions.push(Region {
            lines: part,
            kind: Kind::Other,
            pitch: 0.0,
            height: 0.0,
            word_gap: 0.0,
            edge: 0.0,
            second_edge: 0.0,
            early: 0.0,
            capitals: false,
            list: false,
            indent: None,
        });
    }
    for r in &mut regions {
        classify(lines, r);
    }
    regions
}

/// How the line `a` ends against the region's right edge, the next line being `b`
/// (`0x140958a80`): 2 if it reaches the edge, 1 if `b`'s first word would not have fitted there
/// (a natural wrap), 0 if it would have (the line was ended).
fn fit(r: &Region, a: &Line, b: &Line) -> u8 {
    let mut edge = r.edge;
    // The longest line itself is measured against the next longest.
    if (a.r - edge).abs() < 0.01 {
        edge = r.second_edge;
    }
    if a.r + 0.01 >= edge {
        return 2;
    }
    let need = if b.cjk >= 2 || b.words.is_empty() { 2.5 * b.h() } else { b.words[0].1 - b.words[0].0 };
    if edge - a.r < need + r.word_gap { 1 } else { 0 }
}

/// Reads a region (`0x1409575c0`).
fn classify(lines: &[Line], r: &mut Region) {
    let ls: Vec<&Line> = r.lines.iter().map(|&i| &lines[i]).collect();
    r.height = median(ls.iter().map(|l| l.h()).collect());
    let mut rights: Vec<f32> = ls.iter().map(|l| l.r).collect();
    rights.sort_by(|a, b| b.total_cmp(a));
    let Some(&edge) = rights.first() else { return };
    r.edge = edge;
    r.second_edge = rights.get(1).copied().unwrap_or(edge);
    let gaps: Vec<f32> = ls
        .iter()
        .flat_map(|l| l.words.windows(2).map(|w| w[1].0 - w[0].1).filter(move |&g| g > 0.0 && g < l.h()))
        .collect();
    r.word_gap = if gaps.len() < 3 { r.height * 0.3 } else { median(gaps) };
    if ls.len() < 2 {
        return;
    }
    r.pitch = median(ls.windows(2).map(|w| w[1].center() - w[0].center()).collect());
    let lefts: Vec<f32> = ls.iter().map(|l| l.l).collect();
    let ends: Vec<f32> = ls.iter().map(|l| l.r).collect();
    let middles: Vec<f32> = ls.iter().map(|l| (l.l + l.r) * 0.5).collect();
    let (left, right, middle) = (spread(&lefts), spread(&ends), spread(&middles));
    let centred = ls.len() >= 3 && middle < 0.6 * left && middle < 0.6 * right && r.word_gap * 0.5 < left;
    let (mut short, mut early) = (0usize, 0usize);
    for w in ls.windows(2) {
        match fit(r, w[0], w[1]) {
            2 => {}
            0 => {
                short += 1;
                early += 1;
            }
            _ => short += 1,
        }
    }
    r.early = if short > 0 { early as f32 / short as f32 } else { 0.5 };
    let cased: Vec<&Line> = ls.iter().copied().filter(|l| l.cased).collect();
    let mixed_case = cased.len() >= 3 && (cased.iter().filter(|l| l.all_caps).count() as f32) < cased.len() as f32 * 0.6;
    let words = median(ls.iter().map(|l| l.spaced_words as f32).collect());
    r.capitals = mixed_case
        && !(r.early < 0.3 && words > 3.0)
        && cased.iter().filter(|l| l.upper_start || l.digit_start).count() as f32 >= cased.len() as f32 * 0.85;
    let lower = cased.len() >= 3 && cased.iter().filter(|l| l.lower_start).count() as f32 >= cased.len() as f32 * 0.5;
    let markers = ls.iter().filter(|l| l.marker).count();
    r.list = markers >= 2 || (ls.len() < 4 && markers > 0);
    if r.list {
        let rest: Vec<f32> = ls.iter().filter(|l| !l.marker).map(|l| l.l).collect();
        if !rest.is_empty() {
            r.indent = Some(median(rest));
        }
    }
    let framed = framed(&ls, r.height);
    let line_gaps: Vec<f32> = ls.windows(2).map(|w| w[1].t - w[0].b).collect();
    let spaced = !line_gaps.is_empty() && median(line_gaps) > 0.35 * r.height;
    let terse = words <= 2.0 && median(ls.iter().map(|l| l.cjk as f32).collect()) <= 4.0;
    let listed = (spaced && terse) || (ls.len() >= 3 && beside_a_column(lines, r));
    r.kind = if r.capitals {
        Kind::List
    } else if framed {
        Kind::Framed
    } else if lower {
        Kind::Prose
    } else if centred {
        Kind::Centred
    } else if listed {
        Kind::List
    } else if short > 0 && ls.len() >= 3 {
        if r.early <= 0.25 {
            Kind::Prose
        } else if r.early >= 0.35 {
            Kind::List
        } else {
            Kind::Other
        }
    } else {
        Kind::Other
    };
}

/// Whether one box holds the region and not much else: most of its lines are in it, none in
/// another box, and the region's text spans much of it.
fn framed(ls: &[&Line], height: f32) -> bool {
    let mut counts: Vec<(u32, usize)> = Vec::new();
    for f in ls.iter().filter_map(|l| l.look.frame) {
        match counts.iter_mut().find(|(id, _)| *id == f.id) {
            Some((_, c)) => *c += 1,
            None => counts.push((f.id, 1)),
        }
    }
    let Some(&(id, count)) = counts.iter().fold(None, |best: Option<&(u32, usize)>, c| match best {
        Some(b) if b.1 >= c.1 => Some(b),
        _ => Some(c),
    }) else {
        return false;
    };
    if (count as f32) < ls.len() as f32 * 0.7 || ls.iter().any(|l| l.look.frame.is_some_and(|f| f.id != id && f.small)) {
        return false;
    }
    let Some(f) = ls.iter().find_map(|l| l.look.frame.filter(|f| f.id == id)) else { return false };
    let span = |a: fn(&Line) -> f32, b: fn(&Line) -> f32| {
        ls.iter().map(|&l| b(l)).fold(f32::MIN, f32::max) - ls.iter().map(|&l| a(l)).fold(f32::MAX, f32::min)
    };
    let (along, across) = (span(|l| l.l, |l| l.r), span(|l| l.t, |l| l.b));
    // The frame's size is the capture's; a vertical line's frame is turned a quarter.
    let (x, y) = if ls[0].flow == Flow::Across { (along, across) } else { (across, along) };
    2.2 * x + 2.0 * height >= f.w && 2.2 * y + 2.0 * height >= f.h && x * y >= 0.3 * f.w * f.h
}

/// Whether most lines of the region have, to their right past its edge, a short line level with
/// them, and those short lines start in one column: a table or a form (`0x140958740`).
fn beside_a_column(lines: &[Line], r: &Region) -> bool {
    let edge = r.lines.iter().map(|&i| lines[i].r).fold(f32::MIN, f32::max);
    let mut starts: Vec<f32> = Vec::new();
    for &i in &r.lines {
        let a = &lines[i];
        let mut best = f32::INFINITY;
        for (j, b) in lines.iter().enumerate() {
            if j == i || b.flow != a.flow || b.spaced_words >= 4 || b.cjk >= 7 {
                continue;
            }
            let d = b.l - a.r;
            if (b.center() - a.center()).abs() <= 0.35 * a.h().min(b.h()) && b.l >= edge && d >= 0.0 && d <= 8.0 * a.h() && b.l < best {
                best = b.l;
            }
        }
        if best.is_finite() {
            starts.push(best);
        }
    }
    starts.len() > 2
        && starts.len() as f32 >= r.lines.len() as f32 * 0.6
        && spread(&starts) < 0.5 * median(r.lines.iter().map(|&i| lines[i].h()).collect())
}

/// What speaks against lines `a` and `b` (the nearest line below `a`) being one paragraph, less
/// what speaks for it (`0x140958b50`).
fn score(lines: &[Line], regions: &[Region], a: usize, b: usize) -> f32 {
    let (la, lb) = (&lines[a], &lines[b]);
    let h = la.h().min(lb.h());
    let gap = lb.t - la.b;
    let region = (la.region == lb.region).then(|| &regions[la.region]);
    let word_gap = region.map_or(0.3 * h, |r| r.word_gap);
    let mut s = 0.0;
    if lb.marker {
        s += 5.0;
    }
    let ratio = size_ratio(la, lb);
    if ratio > 1.7 {
        s += 7.0;
    } else if ratio > 1.45 {
        s += 4.0;
    } else if ratio > 1.3 && region.is_none_or(|r| r.kind != Kind::Framed) {
        s += 1.5;
    }
    if let (Some(ca), Some(cb)) = (la.look.color, lb.look.color) {
        let d = ((ca[0] - cb[0]).powi(2) + (ca[1] - cb[1]).powi(2) + (ca[2] - cb[2]).powi(2)).sqrt();
        if d > 100.0 {
            s += 3.0;
        } else if d > 55.0 {
            s += 1.5;
        }
    }
    if apart_frames(la, lb, false) {
        s += 3.0;
    }
    if let Some(r) = region {
        if r.pitch > 0.0 && lb.center() - la.center() > 1.35 * r.pitch && gap > 0.3 * h {
            s += 3.5;
        }
    }
    if gap > 1.6 * h {
        s += 5.0;
    } else if gap > 1.1 * h {
        s += 3.0;
    } else if gap > 0.8 * h {
        s += 1.5;
    }
    match region {
        None => s += 1.5,
        Some(r) => {
            let f = fit(r, la, lb);
            s += match r.kind {
                Kind::Prose => [0.5, -3.0, 0.0][f as usize],
                Kind::List => [3.0, 1.0, 1.5][f as usize] + if r.capitals { 2.5 } else { 0.0 },
                Kind::Centred => -1.5,
                Kind::Framed => -2.5,
                Kind::Other => if f == 0 { 2.5 } else { 0.5 },
            };
        }
    }
    if !la.letter && !lb.letter {
        s += 5.0;
    }
    if la.hyphen_end {
        s -= 4.0;
    }
    if la.comma_end {
        s -= 2.5;
    }
    if la.open_bracket {
        s -= 2.5;
    }
    if lb.lower_start {
        s -= 4.0;
    }
    // The next line of a list item, started where the item's text starts.
    if let Some(r) = region {
        if r.list && !lb.marker && r.indent.is_none_or(|x| (lb.l - x).abs() < 1.5 * word_gap) {
            s -= 4.0;
        }
    }
    if la.sentence_end && (lb.upper_start || lb.digit_start) && region.is_none_or(|r| r.kind != Kind::Prose) {
        s += 1.5;
    }
    if la.cjk > 1 && la.goes_on {
        s -= 3.0;
    }
    if la.colon_end {
        s += 2.0;
    }
    if la.key_value && lb.key_value {
        s += 3.0;
    }
    s
}

/// Measures each line's ink and finds its frame on `image`, the capture the lines were read from,
/// as Lookupper's analyzer does before pairing them (`0x1409538e0`). Lines already measured are
/// measured again, so the frames' numbers hold for all of them.
pub fn look(image: &RgbaImage, lines: &mut [OcrLine]) {
    for l in lines.iter_mut() {
        l.look = Look::default();
    }
    let heights: Vec<f32> = lines.iter().map(|l| {
        let (_, _, t, b) = frame_box(l);
        b - t
    }).collect();
    let height = median(heights);
    if height < 1.0 || image.width() == 0 || image.height() == 0 {
        return;
    }
    // Lookupper measures on the capture shrunk to lines 20 pixels high (never enlarged).
    let scale = (20.0 / height).clamp(0.08, 1.0);
    for l in lines.iter_mut() {
        ink(image, l, scale);
    }
    frames(image, lines, scale, height);
}

/// An integer rectangle: x, y, width, height.
type Rect = (i32, i32, i32, i32);

/// A line's box on the capture shrunk by `scale`, grown by `pad` on each side (`0x140953d90`).
fn scaled_rect(l: &OcrLine, scale: f32, pad: i32) -> Rect {
    let t = |v: f32| v as i32;
    (t(l.x0 * scale) - pad, t(l.y0 * scale) - pad, t((l.x1 - l.x0) * scale) + 2 * pad, t((l.y1 - l.y0) * scale) + 2 * pad)
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = (a.0.max(b.0), a.1.max(b.1));
    let (x1, y1) = ((a.0 + a.2).min(b.0 + b.2), (a.1 + a.3).min(b.1 + b.3));
    (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
}

/// The pixels of rectangle `r` of the capture shrunk by `scale` (at most 1), as OpenCV's
/// INTER_AREA makes them: each the mean of the capture under it, RGB.
fn shrunk(image: &RgbaImage, scale: f32, r: Rect) -> Vec<[u8; 3]> {
    let inv = 1.0 / scale;
    let spans = |from: i32, n: i32, limit: u32| -> Vec<Vec<(usize, f32)>> {
        (from..from + n)
            .map(|i| {
                let (a, b) = ((i as f32 * inv).max(0.0), ((i + 1) as f32 * inv).min(limit as f32));
                let mut cover = Vec::new();
                let mut p = a.floor();
                while p < b {
                    let part = (p + 1.0).min(b) - p.max(a);
                    if part > 0.0 {
                        cover.push((p as usize, part));
                    }
                    p += 1.0;
                }
                cover
            })
            .collect()
    };
    let (xs, ys) = (spans(r.0, r.2, image.width()), spans(r.1, r.3, image.height()));
    let raw = image.as_raw();
    let stride = image.width() as usize * 4;
    let mut out = Vec::with_capacity(xs.len() * ys.len());
    for ry in &ys {
        for rx in &xs {
            let (mut acc, mut total) = ([0.0f32; 3], 0.0f32);
            for &(py, wy) in ry {
                for &(px, wx) in rx {
                    let o = py * stride + px * 4;
                    let w = wx * wy;
                    for (k, a) in acc.iter_mut().enumerate() {
                        *a += raw[o + k] as f32 * w;
                    }
                    total += w;
                }
            }
            let mean = |v: f32| if total > 0.0 { (v / total).round().clamp(0.0, 255.0) as u8 } else { 0 };
            out.push([mean(acc[0]), mean(acc[1]), mean(acc[2])]);
        }
    }
    out
}

/// OpenCV's grey (`COLOR_BGR2GRAY`, its fixed-point weights).
fn grey(p: [u8; 3]) -> u8 {
    ((p[0] as u32 * 4899 + p[1] as u32 * 9617 + p[2] as u32 * 1868 + 8192) >> 14) as u8
}

/// Otsu's threshold, as OpenCV computes it: pixels above it are one class.
fn otsu(px: &[u8]) -> u8 {
    let mut hist = [0usize; 256];
    for &p in px {
        hist[p as usize] += 1;
    }
    let n = px.len() as f64;
    let mu: f64 = hist.iter().enumerate().map(|(i, &c)| i as f64 * c as f64).sum::<f64>() / n;
    let (mut q1, mut mu1, mut best, mut at) = (0.0f64, 0.0f64, 0.0f64, 0u8);
    for (i, &c) in hist.iter().enumerate() {
        let p = c as f64 / n;
        mu1 *= q1;
        q1 += p;
        let q2 = 1.0 - q1;
        if q1.min(q2) < f32::EPSILON as f64 || q1.max(q2) > 1.0 - f32::EPSILON as f64 {
            continue;
        }
        mu1 = (mu1 + i as f64 * p) / q1;
        let mu2 = (mu - q1 * mu1) / q2;
        let sigma = q1 * q2 * (mu1 - mu2) * (mu1 - mu2);
        if sigma > best {
            best = sigma;
            at = i as u8;
        }
    }
    at
}

/// A line's ink (`0x140954000`): its box split by Otsu's threshold, the smaller side being the
/// ink; the run of rows (columns, across a vertical line) holding 85% of it, and its mean colour.
fn ink(image: &RgbaImage, l: &mut OcrLine, scale: f32) {
    let bounds = (0, 0, (image.width() as f32 * scale).round() as i32, (image.height() as f32 * scale).round() as i32);
    let r = intersect(scaled_rect(l, scale, 0), bounds);
    if r.2 <= 2 || r.3 <= 3 {
        return;
    }
    let colors = shrunk(image, scale, r);
    let greys: Vec<u8> = colors.iter().map(|&p| grey(p)).collect();
    let t = otsu(&greys);
    let above = greys.iter().filter(|&&g| g > t).count();
    // The ink is the minority: dark text on a light ground, or light text on a dark one.
    let ink_above = 2 * above < greys.len();
    let mask: Vec<bool> = greys.iter().map(|&g| (g > t) == ink_above).collect();
    let count = mask.iter().filter(|&&m| m).count();
    if count <= 3 || count as f64 > greys.len() as f64 * 0.75 {
        return;
    }
    let (w, h) = (r.2 as usize, r.3 as usize);
    let across: Vec<i64> = if l.flow == Flow::Across {
        (0..h).map(|y| mask[y * w..(y + 1) * w].iter().filter(|&&m| m).count() as i64).collect()
    } else {
        (0..w).map(|x| (0..h).filter(|&y| mask[y * w + x]).count() as i64).collect()
    };
    let total: i64 = across.iter().sum();
    let need = (total as f64 * 0.85) as i64;
    let (mut best, mut sum, mut from) = (across.len(), 0i64, 0usize);
    for (to, &v) in across.iter().enumerate() {
        sum += v;
        while sum - across[from] >= need {
            sum -= across[from];
            from += 1;
        }
        if sum >= need && to - from + 1 < best {
            best = to - from + 1;
        }
    }
    l.look.ink = best as f32 / scale;
    let mut acc = [0.0f32; 3];
    for (p, _) in colors.iter().zip(&mask).filter(|(_, m)| **m) {
        for (a, &v) in acc.iter_mut().zip(p) {
            *a += v as f32;
        }
    }
    l.look.color = Some(acc.map(|v| v / count as f32));
}

/// A 3 × 3 median, the edge pixels repeated (OpenCV's `medianBlur`).
fn median3(g: &[u8], w: usize, h: usize) -> Vec<u8> {
    let at = |x: isize, y: isize| g[y.clamp(0, h as isize - 1) as usize * w + x.clamp(0, w as isize - 1) as usize];
    let mut out = vec![0u8; g.len()];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let mut v = [0u8; 9];
            let mut k = 0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    v[k] = at(x + dx, y + dy);
                    k += 1;
                }
            }
            v.sort_unstable();
            out[y as usize * w + x as usize] = v[4];
        }
    }
    out
}

/// OpenCV's BORDER_REFLECT_101 for index `i` of `n`.
fn reflect(i: isize, n: usize) -> usize {
    if n == 1 {
        0
    } else if i < 0 {
        (-i) as usize
    } else if i as usize >= n {
        2 * n - 2 - i as usize
    } else {
        i as usize
    }
}

/// Where the grey picture has edges: Scharr's gradients across and down, each made absolute
/// (at most 255), added, and over 40.
fn edges(g: &[u8], w: usize, h: usize) -> Vec<bool> {
    let at = |x: isize, y: isize| g[reflect(y, h) * w + reflect(x, w)] as i32;
    let mut out = vec![false; g.len()];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let gx = 3 * (at(x + 1, y - 1) - at(x - 1, y - 1)) + 10 * (at(x + 1, y) - at(x - 1, y)) + 3 * (at(x + 1, y + 1) - at(x - 1, y + 1));
            let gy = 3 * (at(x - 1, y + 1) - at(x - 1, y - 1)) + 10 * (at(x, y + 1) - at(x, y - 1)) + 3 * (at(x + 1, y + 1) - at(x + 1, y - 1));
            let sum = (gx.abs().min(255) + gy.abs().min(255)).min(255);
            out[y as usize * w + x as usize] = sum > 40;
        }
    }
    out
}

/// One connected part of a mask: its box and its size in pixels.
#[derive(Clone, Copy)]
struct Part {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    area: usize,
}

/// The 8-connected parts of `on`, numbered from 1 in the order they start, line by line (0 is off).
fn parts(on: &[bool], w: usize, h: usize) -> (Vec<u32>, Vec<Part>) {
    let mut labels = vec![0u32; on.len()];
    let mut parent: Vec<u32> = vec![0];
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !on[i] {
                continue;
            }
            let mut near = [0u32; 4];
            if x > 0 {
                near[0] = labels[i - 1];
            }
            if y > 0 {
                near[1] = labels[i - w];
                if x > 0 {
                    near[2] = labels[i - w - 1];
                }
                if x + 1 < w {
                    near[3] = labels[i - w + 1];
                }
            }
            let mut label = 0;
            for &n in near.iter().filter(|&&n| n > 0) {
                let r = root(&mut parent, n);
                if label == 0 {
                    label = r;
                } else if r != label {
                    let (lo, hi) = (r.min(label), r.max(label));
                    parent[hi as usize] = lo;
                    label = lo;
                }
            }
            if label == 0 {
                label = parent.len() as u32;
                parent.push(label);
            }
            labels[i] = label;
        }
    }
    let mut number = vec![0u32; parent.len()];
    let mut out: Vec<Part> = vec![Part { x0: 0, y0: 0, x1: 0, y1: 0, area: 0 }];
    for p in 1..parent.len() as u32 {
        let r = root(&mut parent, p);
        if r == p {
            number[p as usize] = out.len() as u32;
            out.push(Part { x0: usize::MAX, y0: usize::MAX, x1: 0, y1: 0, area: 0 });
        } else {
            number[p as usize] = number[r as usize];
        }
    }
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if labels[i] == 0 {
                continue;
            }
            let n = number[labels[i] as usize];
            labels[i] = n;
            let part = &mut out[n as usize];
            part.x0 = part.x0.min(x);
            part.y0 = part.y0.min(y);
            part.x1 = part.x1.max(x + 1);
            part.y1 = part.y1.max(y + 1);
            part.area += 1;
        }
    }
    (labels, out)
}

/// Finds the frame of each line (`0x140954ac0`): on the capture around the lines, shrunk to at
/// most 800 pixels, the edges outside the lines' boxes that are long enough (a box's border, not a
/// letter's) enclose parts of the capture. A line's frame is the part filling most of its box,
/// if that part holds the whole box.
fn frames(image: &RgbaImage, lines: &mut [OcrLine], scale: f32, height: f32) {
    let bounds = (0, 0, (image.width() as f32 * scale).round() as i32, (image.height() as f32 * scale).round() as i32);
    let rects: Vec<Rect> = lines.iter().map(|l| scaled_rect(l, scale, 0)).collect();
    let (Some(x0), Some(y0)) = (rects.iter().map(|r| r.0).min(), rects.iter().map(|r| r.1).min()) else { return };
    let x1 = rects.iter().map(|r| r.0 + r.2).max().unwrap_or(x0);
    let y1 = rects.iter().map(|r| r.1 + r.3).max().unwrap_or(y0);
    let margin = (height * 2.5 * scale) as i32;
    let crop = intersect((x0 - margin, y0 - margin, x1 - x0 + 2 * margin, y1 - y0 + 2 * margin), bounds);
    if crop.2 <= 7 || crop.3 <= 7 {
        return;
    }
    let shrink = (800.0 / crop.2.max(crop.3) as f32).min(1.0);
    let s = scale * shrink;
    let (ox, oy) = (crop.0 as f32 * shrink, crop.1 as f32 * shrink);
    let (w, h) = if shrink < 1.0 { ((crop.2 as f32 * shrink).round() as usize, (crop.3 as f32 * shrink).round() as usize) } else { (crop.2 as usize, crop.3 as usize) };
    if w <= 7 || h <= 7 {
        return;
    }
    let pixels = shrunk(image, s, (ox as i32, oy as i32, w as i32, h as i32));
    let greys: Vec<u8> = pixels.iter().map(|&p| grey(p)).collect();
    let mut edge = edges(&median3(&greys, w, h), w, h);
    // A line's box on this picture, grown by `pad`, within it.
    let area = (0, 0, w as i32, h as i32);
    let line_rect = |l: &OcrLine, pad: i32| {
        let r = scaled_rect(l, s, pad);
        intersect((r.0 - ox as i32, r.1 - oy as i32, r.2, r.3), area)
    };
    let mut in_line = vec![false; w * h];
    for l in lines.iter() {
        let r = line_rect(l, 1);
        for y in r.1..r.1 + r.3 {
            for x in r.0..r.0 + r.2 {
                in_line[y as usize * w + x as usize] = true;
            }
        }
    }
    // Keep the edges mostly outside the lines and at least 1.2 line heights long.
    let (labels, found) = parts(&edge, w, h);
    let mut outside = vec![0usize; found.len()];
    for i in 0..w * h {
        if labels[i] > 0 && !in_line[i] {
            outside[labels[i] as usize] += 1;
        }
    }
    let long = 1.2 * height * s;
    let keep: Vec<bool> = found
        .iter()
        .zip(&outside)
        .map(|(p, &o)| o as f32 > 0.25 * p.area as f32 && (p.x1 - p.x0).max(p.y1 - p.y0) as f32 >= long)
        .collect();
    for i in 0..w * h {
        if labels[i] > 0 && !keep[labels[i] as usize] {
            edge[i] = false;
        }
    }
    let open: Vec<u8> = edge.iter().map(|&e| if e { 0 } else { 255 }).collect();
    let open = median3(&open, w, h);
    let (labels, found) = parts(&open.iter().map(|&v| v > 0).collect::<Vec<bool>>(), w, h);
    let least = (4.0 * (height * s).powi(2)).max(16.0) as usize;
    let half = (0.5 * (w * h) as f32) as usize;
    for l in lines.iter_mut() {
        let r = line_rect(&*l, 1);
        if r.2 <= 0 || r.3 <= 0 {
            continue;
        }
        let mut counts: Vec<(u32, usize)> = Vec::new();
        for y in r.1..r.1 + r.3 {
            for x in r.0..r.0 + r.2 {
                let label = labels[y as usize * w + x as usize];
                if label == 0 || found[label as usize].area < least {
                    continue;
                }
                match counts.iter_mut().find(|(id, _)| *id == label) {
                    Some((_, c)) => *c += 1,
                    None => counts.push((label, 1)),
                }
            }
        }
        let Some(&(id, count)) = counts.iter().fold(None, |best: Option<&(u32, usize)>, c| match best {
            Some(b) if b.1 >= c.1 => Some(b),
            _ => Some(c),
        }) else {
            continue;
        };
        let p = found[id as usize];
        let tight = line_rect(&*l, 0);
        let (px0, py0, pw, ph) = (p.x0 as i32, p.y0 as i32, (p.x1 - p.x0) as i32, (p.y1 - p.y0) as i32);
        let holds = tight.2 <= 0
            || tight.3 <= 0
            || (px0 <= tight.0 + 1 && py0 <= tight.1 + 1 && tight.0 - 1 + tight.2 <= px0 + pw && tight.1 - 1 + tight.3 <= py0 + ph);
        if holds {
            l.look.frame = Some(Frame {
                id,
                filled: count as f32 >= 0.6 * (r.2 * r.3) as f32,
                small: p.area < half,
                w: pw as f32 / s,
                h: ph as f32 / s,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::OcrChar;

    fn line(text: &str, x0: f32, y0: f32) -> OcrLine {
        let chars: Vec<OcrChar> = text.chars().enumerate()
            .map(|(i, ch)| OcrChar { ch, x0: x0 + i as f32 * 10.0, x1: x0 + i as f32 * 10.0 + 10.0 })
            .collect();
        let n = text.chars().count() as f32;
        OcrLine { text: text.into(), x0, y0, x1: x0 + n * 10.0, y1: y0 + 20.0, chars, ink_y: None, flow: Flow::Across, tilt: None, look: Look::default() }
    }

    #[test]
    fn wrapped_prose_is_one_paragraph_and_a_title_and_a_far_line_are_apart() {
        let mut title = line("Chapter One", 100.0, 40.0);
        title.y1 = 80.0;
        let lines = [
            title,
            line("The quick brown fox jumps over the", 100.0, 100.0),
            line("lazy dog. A second sentence starts", 100.0, 125.0),
            line("here and ends here.", 100.0, 150.0),
            line("Another paragraph far below.", 100.0, 260.0),
        ];
        assert_eq!(paragraphs(&lines), [0, 1, 1, 1, 2]);
    }

    #[test]
    fn list_items_are_apart_and_an_item_keeps_its_next_line() {
        let lines = [line("• Apples are red and", 100.0, 100.0), line("green.", 120.0, 125.0), line("• Pears are yellow.", 100.0, 150.0)];
        assert_eq!(paragraphs(&lines), [0, 0, 1]);
    }

    #[test]
    fn whole_sentences_starting_with_capitals_are_a_list() {
        let lines: Vec<_> = (0..5).map(|i| line(&format!("Line {i} ends here with a few more words."), 100.0, 100.0 + i as f32 * 25.0)).collect();
        assert_eq!(paragraphs(&lines), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_line_ending_in_a_particle_goes_on_in_the_next_column() {
        let column = |text: &str, x0: f32| {
            let n = text.chars().count();
            let chars = text.chars().enumerate().map(|(i, ch)| OcrChar { ch, x0: 100.0 + i as f32 * 20.0, x1: 120.0 + i as f32 * 20.0 }).collect();
            OcrLine { text: text.into(), x0, y0: 100.0, x1: x0 + 20.0, y1: 100.0 + n as f32 * 20.0, chars, ink_y: None, flow: Flow::Down, tilt: None, look: Look::default() }
        };
        assert_eq!(paragraphs(&[column("晴れ。", 170.0), column("今日は", 200.0), line("Title", 100.0, 20.0)]), [0, 0, 1]);
    }

    #[test]
    fn line_marks_are_read_as_lookupper_reads_them() {
        let f = |text: &str| features(&line(text, 0.0, 0.0));
        assert!(f("1. First").marker && f("a) second").marker && f("• third").marker && f("- fourth").marker && f("① fifth").marker);
        assert!(!f("10. Tenth").marker && !f("-5 degrees").marker);
        assert!(f("Name: Value").key_value && !f("Name:").key_value && f("Name:").colon_end);
        assert!(f("THE END").all_caps && !f("The End").all_caps && !f("I").all_caps);
        assert!(f("(see below").open_bracket && !f("(see below)").open_bracket);
        assert!(f("3 more").digit_start && f("«hello»").lower_start && f("— Yes").upper_start);
        assert!(f("見ている").cjk == 4 && f("我们的").goes_on && f("ですが").goes_on && !f("です。").goes_on);
    }

    /// A white capture with two boxes side by side, framed by black borders 4 pixels wide, and a
    /// line in each: red ink 12 rows high in the left one, blue in the right one.
    fn two_boxes() -> (RgbaImage, Vec<OcrLine>) {
        let mut img = RgbaImage::from_pixel(400, 120, image::Rgba([255, 255, 255, 255]));
        for (bx0, bx1) in [(10u32, 190u32), (210, 390)] {
            for y in 10..110 {
                for x in bx0..bx1 {
                    if x < bx0 + 4 || x >= bx1 - 4 || y < 14 || y >= 106 {
                        img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                    }
                }
            }
        }
        for (x0, color) in [(34u32, [220u8, 20, 20]), (234, [20, 20, 220])] {
            for y in 46..58 {
                for x in x0..x0 + 132 {
                    img.put_pixel(x, y, image::Rgba([color[0], color[1], color[2], 255]));
                }
            }
        }
        let mut a = line("Left box", 30.0, 40.0);
        let mut b = line("Right box", 230.0, 40.0);
        for l in [&mut a, &mut b] {
            l.x1 = l.x0 + 140.0;
            l.y1 = 64.0;
        }
        (img, vec![a, b])
    }

    #[test]
    fn the_ink_is_measured_and_each_box_is_a_frame_of_its_own() {
        let (img, mut lines) = two_boxes();
        look(&img, &mut lines);
        let (a, b) = (lines[0].look, lines[1].look);
        assert!((a.ink - 12.0).abs() <= 2.5, "{a:?}");
        let red = a.color.unwrap();
        assert!(red[0] > 180.0 && red[1] < 80.0 && red[2] < 80.0, "{red:?}");
        let blue = b.color.unwrap();
        assert!(blue[2] > 180.0 && blue[0] < 80.0, "{blue:?}");
        let (fa, fb) = (a.frame.unwrap(), b.frame.unwrap());
        assert_ne!(fa.id, fb.id);
        assert!(fa.filled && fb.filled && fa.small && fb.small, "{fa:?} {fb:?}");
        assert!((fa.w - 170.0).abs() < 12.0, "the box inside its border: {fa:?}");
    }

    #[test]
    fn otsu_splits_two_levels_and_parts_are_numbered_in_order() {
        let px: Vec<u8> = (0..100).map(|i| if i < 30 { 40 } else { 200 }).collect();
        let t = otsu(&px);
        assert!((40..200).contains(&t));
        let on = [true, false, true, false, false, false, true, true, false];
        let (labels, found) = parts(&on, 3, 3);
        assert_eq!(labels, [1, 0, 2, 0, 0, 0, 3, 3, 0]);
        assert_eq!(found[3].area, 2);
    }
}
