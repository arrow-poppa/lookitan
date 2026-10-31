// OneOCR on Linux, without oneocr.dll (Windows only): its models out of the user's imported
// oneocr.onemodel (ocr/onemodel.rs) run on Lookitan's ONNX Runtime, at the user's request on
// 2026-10-30. The pipeline is ours, after bropines/oneocr-onnx-python's (MIT) and measured against
// oneocr.dll on Windows on the same pictures (13 of them, nine scripts: the same text, line boxes
// at 0.86–0.97 IoU of the DLL's).
//
// The detector is SegLink-like, at three levels (strides 4, 8 and 16), each way (lines across,
// columns down): a pixel over its level's score (0.7, 0.8, 0.8, the model's configuration) is a
// piece of a line, `bbox_deltas` its four corners from the pixel's centre in units of 8 strides,
// and `link_scores` (its eight neighbours in raster order, the centre skipped) join pieces of one
// line. A line is its pieces along the axis through their centres, as tall as their median edges.
// Each level reads text up to about 8 strides tall (and a third) whole; taller text is found split
// there and whole a level up, so the finest level the text fits is kept, then the stronger way.
// Pieces of one line split at a word gap (big text) are joined.
//
// Each line is classified by script and read by that script group's recognizer: the OCR language's
// group unless the classifier puts another one ahead by `MARGIN` (a line of English in Russian text
// reads with the Latin recognizer, a short line does not switch on a whim). The recognizers read a
// line 60 px tall, blue-green-red over 255, a column turned a quarter left (its top at the left);
// each character sits on its CTC frame, as drawn (a right-to-left line is turned by `line_from`).
// The Rejection and Confidence models are not used.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use image::RgbaImage;
use ort::session::Session;
use ort::value::Tensor;
use parking_lot::Mutex;

use super::onemodel::OneModel;
use super::paddle::{cpu_session_from_memory, gpu_session_from_memory, BoxReader, Glyph, InferenceSession, TextBox};
use crate::settings::GpuPreference;

/// The detector's levels: stride, and the score a piece of text must pass.
const LEVELS: [(u32, f32); 3] = [(4, 0.7), (8, 0.8), (16, 0.8)];
const LINK: f32 = 0.5;
/// A level reads text whole up to this many times 8 strides tall.
const FIT: f32 = 1.35;
/// Boxes overlapping more than this of the smaller one are the same text.
const OVERLAP: f32 = 0.5;
/// Pieces of a line split at a gap narrower than their height are one line.
const GAP: f32 = 1.0;
/// The detector's input, at most this long a side (a larger capture is read smaller).
const DET_MAX_SIDE: u32 = 1920;
const LINE_HEIGHT: u32 = 60;
const MAX_WIDTH: u32 = 4096;
/// How far ahead of the language's group (in the classifier's logits) another group must be.
const MARGIN: f32 = 3.0;
/// Below this angle a line counts as level (as quad.rs's).
const LEVEL: f32 = 2.0 * std::f32::consts::PI / 180.0;
/// Recognizers kept loaded: the language's and the last other one.
const KEPT: usize = 2;
/// The classifier's classes, as the model's enums list the scripts (0: none).
const CLASSES: [Option<&str>; 10] = [
    None,
    Some("CJKPrinted"),
    Some("LatinPrintedV2"),
    Some("CyrillicMixed"),
    Some("ArabicMixed"),
    Some("DevanagariMixed"),
    Some("GreekMixed"),
    Some("ThaiMixed"),
    Some("HebrewPrinted"),
    Some("TamilPrinted"),
];

/// The script group that reads an OCR language (`ocr::LANGUAGES`).
pub fn group_of(tag: &str) -> &'static str {
    let tag = tag.to_ascii_lowercase();
    let lang = tag.split(['-', '_']).next().unwrap_or_default();
    match lang {
        "ja" | "zh" | "ko" => "CJKPrinted",
        "ru" | "uk" | "bg" | "be" | "mk" => "CyrillicMixed",
        "sr" if tag.contains("cyrl") => "CyrillicMixed",
        "el" => "GreekMixed",
        "ar" | "fa" | "ur" => "ArabicMixed",
        "he" | "iw" | "yi" => "HebrewPrinted",
        "hi" | "mr" | "ne" | "sa" => "DevanagariMixed",
        "th" => "ThaiMixed",
        "ta" => "TamilPrinted",
        _ => "LatinPrintedV2",
    }
}

/// A model's session, loaded from the model file (decrypted again for the CPU after a GPU failure).
fn load(model: &OneModel, name: &str, gpu: bool, choice: Option<GpuPreference>, stage: &'static str) -> Result<(Mutex<InferenceSession>, Arc<AtomicBool>)> {
    let bytes = model.read(name)?;
    let (session, on_gpu) = if gpu {
        match gpu_session_from_memory(&bytes, choice) {
            Ok(s) => (s, true),
            Err(e) => {
                log::warn!("Ocr.GpuFallback.{stage}: {e:#}");
                (cpu_session_from_memory(&bytes)?, false)
            }
        }
    } else {
        (cpu_session_from_memory(&bytes)?, false)
    };
    let on_gpu = Arc::new(AtomicBool::new(on_gpu));
    let session = InferenceSession { session: Some(session), on_gpu: on_gpu.clone(), cpu_path: Path::new(name).to_path_buf(), stage };
    Ok((Mutex::new(session), on_gpu))
}

/// One script group's recognizer and what its classes are.
struct GroupReader {
    group: &'static str,
    session: Mutex<InferenceSession>,
    /// Class `i`'s text: a character (or a letter and its marks), a space, or nothing (the blank).
    chars: Vec<Option<String>>,
    on_gpu: Arc<AtomicBool>,
}

impl GroupReader {
    fn new(model: &OneModel, group: &'static str, gpu: bool, choice: Option<GpuPreference>) -> Result<Self> {
        let (onnx, list) = model.recognizer(group).with_context(|| format!("oneocr.onemodel has no {group} recognizer"))?;
        let composites = match model.composites(group) {
            Some(name) => String::from_utf8_lossy(&model.read(name)?).into_owned(),
            None => String::new(),
        };
        let chars = classes(&String::from_utf8_lossy(&model.read(list)?), &composites)?;
        let (session, on_gpu) = load(model, onnx, gpu, choice, "OneOcrRec")?;
        log::info!("OneOCR {group} ready on {}, {} classes", if on_gpu.load(Ordering::Relaxed) { "GPU" } else { "CPU" }, chars.len());
        Ok(Self { group, session, chars, on_gpu })
    }
}

/// The classes of a `char2ind.txt` (each line a character, a space and its class; `<space>`,
/// `<trash_as_space>` and `<blank>` stand for themselves), private-use characters spelled out by the
/// `composite_chars_map` (each line one, a space and what it stands for).
fn classes(list: &str, composites: &str) -> Result<Vec<Option<String>>> {
    let spelled: std::collections::HashMap<&str, &str> =
        composites.lines().filter(|l| !l.starts_with("//")).filter_map(|l| l.split_once(' ')).collect();
    let mut chars: Vec<Option<String>> = Vec::new();
    for line in list.lines().filter(|l| !l.is_empty()) {
        let Some((ch, class)) = line.rsplit_once(' ') else { continue };
        let Ok(class) = class.parse::<usize>() else { continue };
        if class >= 1 << 20 {
            bail!("a class number out of range in char2ind.txt");
        }
        if chars.len() <= class {
            chars.resize(class + 1, None);
        }
        chars[class] = match ch {
            "<space>" | "<trash_as_space>" => Some(" ".into()),
            "<blank>" => None,
            ch => Some(spelled.get(ch).map_or(ch, |s| *s).to_string()),
        };
    }
    if chars.is_empty() {
        bail!("char2ind.txt lists no characters");
    }
    Ok(chars)
}

/// A piece of a line or a whole one the detector found, in the detector's input pixels.
#[derive(Clone, Debug)]
struct Found {
    /// Top left, top right, bottom right, bottom left (a column's top edge across its top).
    quad: [[f32; 2]; 4],
    score: f32,
    pieces: usize,
    down: bool,
    angle: f32,
    stride: u32,
}

impl Found {
    fn bounds(&self) -> (f32, f32, f32, f32) {
        let xs = self.quad.map(|p| p[0]);
        let ys = self.quad.map(|p| p[1]);
        let min = |v: [f32; 4]| v.into_iter().fold(f32::MAX, f32::min);
        let max = |v: [f32; 4]| v.into_iter().fold(f32::MIN, f32::max);
        (min(xs), min(ys), max(xs), max(ys))
    }

    /// Whether it is taller (a column: wider) than its level reads whole.
    fn too_big(&self) -> bool {
        let (x0, y0, x1, y1) = self.bounds();
        let across = if self.down { x1 - x0 } else { y1 - y0 };
        across > FIT * 8.0 * self.stride as f32
    }
}

pub struct OneOcrOnnx {
    model: OneModel,
    det: Mutex<InferenceSession>,
    det_on_gpu: Arc<AtomicBool>,
    scripts: Mutex<InferenceSession>,
    recs: Mutex<Vec<Arc<GroupReader>>>,
    gpu: bool,
    choice: Option<GpuPreference>,
}

impl OneOcrOnnx {
    /// The detector and the classifier of the model file at `path`; recognizers load when asked.
    pub fn new(path: &Path, gpu: bool, choice: Option<GpuPreference>) -> Result<Self> {
        let model = OneModel::open(path)?;
        let det_name = model.detector().context("oneocr.onemodel has no detector")?.to_string();
        let cls_name = model.classifier().context("oneocr.onemodel has no script classifier")?.to_string();
        let (det, det_on_gpu) = load(&model, &det_name, gpu, choice, "OneOcrDet")?;
        // The classifier is small: on the CPU, beside each line's recognizer run.
        let (scripts, _) = load(&model, &cls_name, false, choice, "OneOcrScript")?;
        log::info!("OneOCR (ONNX) ready, its detector on {}", if det_on_gpu.load(Ordering::Relaxed) { "GPU" } else { "CPU" });
        Ok(Self { model, det, det_on_gpu, scripts, recs: Mutex::new(Vec::new()), gpu, choice })
    }

    pub fn on_gpu(&self) -> bool {
        self.det_on_gpu.load(Ordering::Relaxed)
    }

    pub fn matches(&self, gpu: bool, choice: Option<GpuPreference>) -> bool {
        self.gpu == gpu && self.choice == choice
    }

    fn cpu_again(&self, name: Option<&str>) -> Result<Session> {
        let name = name.context("a OneOCR model to load")?;
        cpu_session_from_memory(&self.model.read(name)?)
    }

    /// Every line and column, in the image's pixels.
    pub fn detect_boxes(&self, img: &RgbaImage) -> Result<Vec<TextBox>> {
        let (w, h) = img.dimensions();
        if w < 8 || h < 8 {
            return Ok(Vec::new());
        }
        let scale = (DET_MAX_SIDE as f32 / w.max(h) as f32).min(1.0);
        let (sw, sh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
        let resized = if scale < 1.0 { image::imageops::resize(img, sw, sh, FilterType::Triangle) } else { img.clone() };
        let (pw, ph) = (sw.div_ceil(32).max(1) * 32, sh.div_ceil(32).max(1) * 32);
        // Red, green, blue, 0–255, padded with white.
        let plane = (pw * ph) as usize;
        let mut data = vec![255f32; 3 * plane];
        for (x, y, p) in resized.enumerate_pixels() {
            let i = (y * pw + x) as usize;
            for c in 0..3 {
                data[c * plane + i] = p[c] as f32;
            }
        }
        let input = ortx!(Tensor::from_array(([1usize, 3, ph as usize, pw as usize], data)));
        let info = ortx!(Tensor::from_array(([1usize, 3], vec![ph as f32, pw as f32, 1.0])));
        let det_name = self.model.detector().map(str::to_string);
        let maps = self.det.lock().run(|_| self.cpu_again(det_name.as_deref()), |s| {
            let outputs = ortx!(s.run(ort::inputs!["data" => input.view(), "im_info" => info.view()]));
            let mut maps = std::collections::HashMap::new();
            for way in ["hori", "vert"] {
                for level in 2..=4 {
                    for kind in ["scores", "link_scores", "bbox_deltas"] {
                        let name = format!("{kind}_{way}_fpn{level}");
                        let (shape, values) = ortx!(outputs[name.as_str()].try_extract_tensor::<f32>());
                        maps.insert(name, (shape[..].to_vec(), values.to_vec()));
                    }
                }
            }
            Ok(maps)
        })?;
        let mut found = Vec::new();
        for (down, way) in [(false, "hori"), (true, "vert")] {
            for (level, (stride, threshold)) in (2..=4).zip(LEVELS) {
                let get = |kind: &str| maps.get(&format!("{kind}_{way}_fpn{level}"));
                let (Some((shape, scores)), Some((_, links)), Some((_, deltas))) = (get("scores"), get("link_scores"), get("bbox_deltas")) else {
                    bail!("OneOCR's detector lacks its fpn{level} {way} maps");
                };
                if shape.len() != 4 {
                    bail!("unexpected OneOCR detector map shape {shape:?}");
                }
                let (gh, gw) = (shape[2] as usize, shape[3] as usize);
                if scores.len() < gh * gw || links.len() < 8 * gh * gw || deltas.len() < 8 * gh * gw {
                    bail!("OneOCR's detector maps are short");
                }
                for piece in components(scores, links, gh, gw, threshold) {
                    if piece.len() >= 2 {
                        found.push(line_of(&piece, scores, deltas, gh, gw, stride, down));
                    }
                }
            }
        }
        // The level first, each way on its own; then the stronger way where both found text.
        let level_first = |a: &Found, b: &Found| (a.too_big(), a.stride).cmp(&(b.too_big(), b.stride)).then(b.score.total_cmp(&a.score));
        let (across, down): (Vec<Found>, Vec<Found>) = found.into_iter().partition(|f| !f.down);
        let mut kept = suppress(across, level_first);
        kept.extend(suppress(down, level_first));
        let kept = join_words(suppress(kept, |a, b| b.score.total_cmp(&a.score)));
        let (wf, hf) = (w as f32, h as f32);
        let mut boxes: Vec<TextBox> = kept.iter().filter_map(|f| text_box(f, 1.0 / scale, wf, hf)).collect();
        boxes.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
        Ok(boxes)
    }

    /// Runs the detector once so the first lookup does not pay for graph setup.
    pub fn warm_up(&self) {
        let blank = RgbaImage::from_pixel(96, 64, image::Rgba([255, 255, 255, 255]));
        if let Err(e) = self.detect_boxes(&blank) {
            log::warn!("OneOCR detector warm-up failed: {e:#}");
        }
    }

    /// What reads the boxes, preferring the group of the OCR language `tag`.
    pub fn reader(&self, tag: &str) -> Reading<'_> {
        Reading { engine: self, prefer: group_of(tag) }
    }

    /// A group's recognizer, loaded when first asked (the least recently used beyond `KEPT` goes).
    fn recognizer(&self, group: &'static str) -> Result<Arc<GroupReader>> {
        let mut recs = self.recs.lock();
        if let Some(i) = recs.iter().position(|r| r.group == group) {
            let r = recs.remove(i);
            recs.insert(0, r.clone());
            return Ok(r);
        }
        recs.truncate(KEPT - 1);
        let r = Arc::new(GroupReader::new(&self.model, group, self.gpu, self.choice)?);
        recs.insert(0, r.clone());
        Ok(r)
    }

    /// The script group a line is in, the preferred one unless another is clearly ahead.
    fn script_of(&self, line: &[f32], width: usize, prefer: &'static str) -> Result<&'static str> {
        let input = ortx!(Tensor::from_array(([1usize, 3, LINE_HEIGHT as usize, width], line.to_vec())));
        let cls_name = self.model.classifier().map(str::to_string);
        let scores = self.scripts.lock().run(|_| self.cpu_again(cls_name.as_deref()), |s| {
            let outputs = ortx!(s.run(ort::inputs!["data" => input.view()]));
            let (_, scores) = ortx!(outputs["script_id_score"].try_extract_tensor::<f32>());
            Ok(scores.to_vec())
        })?;
        Ok(choose_group(&scores, prefer))
    }

    /// Reads one line lying across (its top up) with `group`'s recognizer: its characters, each on
    /// its frame's centre in `to_crop` crop pixels per input pixel.
    fn read_line(&self, line: Vec<f32>, width: usize, group: &'static str, to_crop: f32) -> Result<Vec<Glyph>> {
        let rec = self.recognizer(group)?;
        let input = ortx!(Tensor::from_array(([1usize, 3, LINE_HEIGHT as usize, width], line)));
        let lengths = ortx!(Tensor::from_array(([1usize], vec![(width / 4) as i32])));
        let (onnx, _) = self.model.recognizer(group).context("its recognizer")?;
        let onnx = onnx.to_string();
        let (frames, classes, logp) = rec.session.lock().run(|_| self.cpu_again(Some(&onnx)), |s| {
            let outputs = ortx!(s.run(ort::inputs!["data" => input.view(), "seq_lengths" => lengths.view()]));
            let (shape, logp) = ortx!(outputs[0].try_extract_tensor::<f32>());
            if shape.len() != 3 {
                bail!("unexpected OneOCR recognizer output shape {:?}", &shape[..]);
            }
            Ok((shape[0] as usize, shape[2] as usize, logp.to_vec()))
        })?;
        if frames == 0 || classes == 0 || logp.len() < frames * classes {
            return Ok(Vec::new());
        }
        let step = width as f32 / frames as f32;
        Ok(decode(&logp, frames, classes, &rec.chars, step * to_crop))
    }
}

/// The group the classifier's logits pick: the preferred one unless another is `MARGIN` ahead.
fn choose_group(scores: &[f32], prefer: &'static str) -> &'static str {
    let scores = &scores[scores.len().saturating_sub(CLASSES.len())..];
    let best = (0..scores.len()).max_by(|&a, &b| scores[a].total_cmp(&scores[b]));
    let Some(Some(group)) = best.map(|i| CLASSES.get(i).copied().flatten()) else { return prefer };
    let preferred = CLASSES.iter().position(|c| *c == Some(prefer)).and_then(|i| scores.get(i));
    match preferred {
        Some(p) if group != prefer && scores[best.unwrap()] - p < MARGIN => prefer,
        _ => group,
    }
}

/// Greedy CTC: repeats collapsed, blanks dropped; a character at each kept frame's centre
/// (`step` crop pixels a frame); spaces at the ends dropped.
fn decode(logp: &[f32], frames: usize, classes: usize, chars: &[Option<String>], step: f32) -> Vec<Glyph> {
    let mut glyphs: Vec<Glyph> = Vec::new();
    let mut prev = usize::MAX;
    for t in 0..frames {
        let row = &logp[t * classes..(t + 1) * classes];
        let (k, p) = row.iter().enumerate().fold((0, f32::MIN), |best, (i, &v)| if v > best.1 { (i, v) } else { best });
        if k != prev {
            if let Some(Some(text)) = chars.get(k) {
                let at = (t as f32 + 0.5) * step;
                for ch in text.chars() {
                    glyphs.push(Glyph { ch, from: at, to: at, score: p.exp() });
                }
            }
        }
        prev = k;
    }
    while glyphs.last().is_some_and(|g| g.ch.is_whitespace()) {
        glyphs.pop();
    }
    let lead = glyphs.iter().take_while(|g| g.ch.is_whitespace()).count();
    glyphs.drain(..lead);
    glyphs
}

/// The pieces over `threshold`, joined where their link to a neighbour passes `LINK`: each a list of
/// grid cells (row-major indices).
fn components(scores: &[f32], links: &[f32], gh: usize, gw: usize, threshold: f32) -> Vec<Vec<usize>> {
    const NEIGHBOURS: [(isize, isize); 8] = [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1)];
    let plane = gh * gw;
    let on = |i: usize| scores[i] > threshold;
    let mut parent: Vec<usize> = (0..plane).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for y in 0..gh {
        for x in 0..gw {
            let i = y * gw + x;
            if !on(i) {
                continue;
            }
            for (n, (dy, dx)) in NEIGHBOURS.iter().enumerate() {
                let (ny, nx) = (y as isize + dy, x as isize + dx);
                if ny < 0 || nx < 0 || ny >= gh as isize || nx >= gw as isize {
                    continue;
                }
                let j = ny as usize * gw + nx as usize;
                if on(j) && links[n * plane + i] > LINK {
                    let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                    if a != b {
                        parent[a] = b;
                    }
                }
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
    for i in (0..plane).filter(|&i| on(i)) {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    groups.into_values().collect()
}

/// The line (or column) a component's pieces make: along the axis through their centres, as long
/// as their ends and as tall (wide) as their median edges.
fn line_of(cells: &[usize], scores: &[f32], deltas: &[f32], gh: usize, gw: usize, stride: u32, down: bool) -> Found {
    let plane = gh * gw;
    let s = stride as f32;
    let pieces: Vec<[[f32; 2]; 4]> = cells
        .iter()
        .map(|&i| {
            let (cx, cy) = (((i % gw) as f32 + 0.5) * s, ((i / gw) as f32 + 0.5) * s);
            let d = |k: usize| deltas[k * plane + i] * 8.0 * s;
            [[cx + d(0), cy + d(1)], [cx + d(2), cy + d(3)], [cx + d(4), cy + d(5)], [cx + d(6), cy + d(7)]]
        })
        .collect();
    let centres: Vec<[f32; 2]> = pieces.iter().map(|q| [q.iter().map(|p| p[0]).sum::<f32>() / 4.0, q.iter().map(|p| p[1]).sum::<f32>() / 4.0]).collect();
    // The axis: least squares through the centres, across for a line, down for a column.
    let (along, off) = if down { (1, 0) } else { (0, 1) };
    let n = centres.len() as f32;
    let (ma, mo) = (centres.iter().map(|c| c[along]).sum::<f32>() / n, centres.iter().map(|c| c[off]).sum::<f32>() / n);
    let (sxx, sxy) = centres.iter().fold((0f32, 0f32), |(xx, xy), c| (xx + (c[along] - ma).powi(2), xy + (c[along] - ma) * (c[off] - mo)));
    let angle = if sxx > 1e-3 { (sxy / sxx).atan() } else { 0.0 };
    let (cos, sin) = (angle.cos(), angle.sin());
    // Into the axis frame: u along the line (or down the column), v across it.
    let to_frame = |p: [f32; 2]| -> (f32, f32) {
        if down {
            (p[1] * cos - p[0] * sin, p[0] * cos + p[1] * sin)
        } else {
            (p[0] * cos + p[1] * sin, -p[0] * sin + p[1] * cos)
        }
    };
    let from_frame = |u: f32, v: f32| -> [f32; 2] {
        if down {
            [v * cos - u * sin, u * cos + v * sin]
        } else {
            [u * cos - v * sin, u * sin + v * cos]
        }
    };
    let (mut u0, mut u1) = (f32::MAX, f32::MIN);
    let mut near = Vec::with_capacity(pieces.len());
    let mut far = Vec::with_capacity(pieces.len());
    for q in &pieces {
        let f = q.map(to_frame);
        for &(u, _) in &f {
            u0 = u0.min(u);
            u1 = u1.max(u);
        }
        // A line's top is its first two corners; a column's left side its first and last.
        let (a, b, c, d) = if down { (f[0].1, f[3].1, f[1].1, f[2].1) } else { (f[0].1, f[1].1, f[2].1, f[3].1) };
        near.push(a.min(b));
        far.push(c.max(d));
    }
    let (v0, v1) = (median(near), median(far));
    let quad = if down {
        [from_frame(u0, v0), from_frame(u0, v1), from_frame(u1, v1), from_frame(u1, v0)]
    } else {
        [from_frame(u0, v0), from_frame(u1, v0), from_frame(u1, v1), from_frame(u0, v1)]
    };
    let score = cells.iter().map(|&i| scores[i]).sum::<f32>() / cells.len() as f32;
    Found { quad, score, pieces: cells.len(), down, angle, stride }
}

fn median(mut v: Vec<f32>) -> f32 {
    v.sort_by(f32::total_cmp);
    match v.len() {
        0 => 0.0,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}

/// Keeps the first, in `order`, of boxes that overlap by more than `OVERLAP` of the smaller one.
fn suppress(mut found: Vec<Found>, order: impl Fn(&Found, &Found) -> std::cmp::Ordering) -> Vec<Found> {
    found.sort_by(&order);
    let mut kept: Vec<Found> = Vec::new();
    for f in found {
        let (x0, y0, x1, y1) = f.bounds();
        let area = ((x1 - x0) * (y1 - y0)).max(1e-6);
        let clash = kept.iter().any(|k| {
            let (a0, b0, a1, b1) = k.bounds();
            let inter = (x1.min(a1) - x0.max(a0)).max(0.0) * (y1.min(b1) - y0.max(b0)).max(0.0);
            inter / area.min(((a1 - a0) * (b1 - b0)).max(1e-6)) > OVERLAP
        });
        if !clash {
            kept.push(f);
        }
    }
    kept
}

/// Joins level pieces of one line split at a word gap: the same way, level, heights within a
/// third, centred on each other, the second starting after the first and the gap under a height.
fn join_words(mut found: Vec<Found>) -> Vec<Found> {
    'again: loop {
        for i in 0..found.len() {
            for j in 0..found.len() {
                let (a, b) = (&found[i], &found[j]);
                if i == j || a.down != b.down || a.angle.abs() > 0.05 || b.angle.abs() > 0.05 {
                    continue;
                }
                // Along and across the line: x and y, or y and x for a column.
                let flip = |(x0, y0, x1, y1): (f32, f32, f32, f32)| if a.down { (y0, x0, y1, x1) } else { (x0, y0, x1, y1) };
                let (ax0, ay0, ax1, ay1) = flip(a.bounds());
                let (bx0, by0, bx1, by1) = flip(b.bounds());
                let (ha, hb) = (ay1 - ay0, by1 - by0);
                let h = ha.min(hb);
                if !(0.75..1.33).contains(&(ha / hb.max(1e-6))) || ((ay0 + ay1) - (by0 + by1)).abs() / 2.0 > 0.25 * h {
                    continue;
                }
                if bx0 < ax0 || bx1 <= ax1 || bx0 - ax1 >= GAP * h {
                    continue;
                }
                let (u0, v0, u1, v1) = (ax0, ay0.min(by0), bx1, ay1.max(by1));
                let quad = if a.down {
                    [[v0, u0], [v1, u0], [v1, u1], [v0, u1]]
                } else {
                    [[u0, v0], [u1, v0], [u1, v1], [u0, v1]]
                };
                let joined = Found { quad, score: (a.score + b.score) / 2.0, pieces: a.pieces + b.pieces, ..a.clone() };
                let (lo, hi) = (i.min(j), i.max(j));
                found.remove(hi);
                found.remove(lo);
                found.push(joined);
                continue 'again;
            }
        }
        return found;
    }
}

/// A found line as a box in the image's pixels (`k` image pixels to one of the detector's input):
/// turned when it lies turned more than a hair; a column always upright.
fn text_box(f: &Found, k: f32, w: f32, h: f32) -> Option<TextBox> {
    let quad = f.quad.map(|p| [p[0] * k, p[1] * k]);
    let turned = !f.down && f.angle.abs() > LEVEL;
    let quad = turned.then(|| super::quad::rect_of(&quad).corners());
    let xs = f.quad.map(|p| p[0] * k);
    let ys = f.quad.map(|p| p[1] * k);
    let x0 = xs.into_iter().fold(f32::MAX, f32::min).max(0.0);
    let y0 = ys.into_iter().fold(f32::MAX, f32::min).max(0.0);
    let x1 = xs.into_iter().fold(f32::MIN, f32::max).min(w);
    let y1 = ys.into_iter().fold(f32::MIN, f32::max).min(h);
    (x1 - x0 >= 2.0 && y1 - y0 >= 2.0).then_some(TextBox { x0, y0, x1, y1, quad })
}

/// A crop as the classifier and the recognizers take it: 60 px tall, blue, green, red over 255;
/// its width, and crop pixels per input pixel.
fn line_input(crop: &RgbaImage) -> (Vec<f32>, usize, f32) {
    let (w, h) = crop.dimensions();
    let width = ((w as f32 * LINE_HEIGHT as f32 / h.max(1) as f32).round() as u32).clamp(16, MAX_WIDTH);
    let line = image::imageops::resize(crop, width, LINE_HEIGHT, FilterType::Triangle);
    let plane = (width * LINE_HEIGHT) as usize;
    let mut data = vec![0f32; 3 * plane];
    for (x, y, p) in line.enumerate_pixels() {
        let i = (y * width + x) as usize;
        for c in 0..3 {
            data[c * plane + i] = p[2 - c] as f32 / 255.0;
        }
    }
    (data, width as usize, w as f32 / width as f32)
}

/// OneOCR reading the boxes of a pass, preferring a script group (`OneOcrOnnx::reader`).
pub struct Reading<'a> {
    engine: &'a OneOcrOnnx,
    prefer: &'static str,
}

impl BoxReader for Reading<'_> {
    fn read(&self, crops: &[(&RgbaImage, bool)]) -> Result<Vec<Vec<Glyph>>> {
        let mut out = Vec::with_capacity(crops.len());
        for (crop, down) in crops {
            // A column is read lying down, its top at the left, as OneOCR's CJK recognizer reads it.
            let lying = down.then(|| image::imageops::rotate270(*crop));
            let (line, width, to_crop) = line_input(lying.as_ref().unwrap_or(crop));
            let group = self.engine.script_of(&line, width, self.prefer)?;
            out.push(self.engine.read_line(line, width, group, to_crop)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_language_prefers_its_script_group() {
        for (tag, group) in [
            ("ja-JP", "CJKPrinted"),
            ("ko-KR", "CJKPrinted"),
            ("zh-TW", "CJKPrinted"),
            ("ru-RU", "CyrillicMixed"),
            ("sr-Cyrl-RS", "CyrillicMixed"),
            ("sr-Latn-RS", "LatinPrintedV2"),
            ("el-GR", "GreekMixed"),
            ("ar-SA", "ArabicMixed"),
            ("he-IL", "HebrewPrinted"),
            ("hi-IN", "DevanagariMixed"),
            ("th-TH", "ThaiMixed"),
            ("ta-IN", "TamilPrinted"),
            ("pt-BR", "LatinPrintedV2"),
        ] {
            assert_eq!(group_of(tag), group, "{tag}");
        }
        for group in super::super::onemodel::GROUPS {
            assert!(CLASSES.contains(&Some(group)), "{group} is a class");
        }
    }

    #[test]
    fn another_script_wins_only_when_clearly_ahead() {
        let mut scores = [0f32; 10];
        scores[2] = 9.0; // Latin
        scores[3] = 7.0; // Cyrillic
        assert_eq!(choose_group(&scores, "CyrillicMixed"), "CyrillicMixed", "two ahead is not enough");
        scores[2] = 11.0;
        assert_eq!(choose_group(&scores, "CyrillicMixed"), "LatinPrintedV2");
        assert_eq!(choose_group(&scores, "LatinPrintedV2"), "LatinPrintedV2");
        let mut none = [0f32; 10];
        none[0] = 5.0;
        assert_eq!(choose_group(&none, "ThaiMixed"), "ThaiMixed", "no script: the preferred one");
    }

    #[test]
    fn char2ind_s_classes_are_characters_spaces_and_the_blank() {
        let chars = classes("! 1\n\u{f0000} 2\n<trash_as_space> 3\n<space> 0\n<blank> 4\n", "// map\n\u{f0000} Ё\u{304}\n").unwrap();
        assert_eq!(chars, vec![Some(" ".into()), Some("!".into()), Some("Ё\u{304}".into()), Some(" ".into()), None]);
        // a, blank, a, a, space, b: "a a b" would be "aa b".
        let table = vec![Some(" ".to_string()), Some("a".into()), Some("b".into()), None];
        let one_hot = |k: usize| (0..4).map(|i| if i == k { 0.0 } else { -9.0 }).collect::<Vec<f32>>();
        let logp: Vec<f32> = [0, 1, 3, 1, 1, 0, 2, 0].iter().flat_map(|&k| one_hot(k)).collect();
        let glyphs = decode(&logp, 8, 4, &table, 4.0);
        assert_eq!(glyphs.iter().map(|g| g.ch).collect::<String>(), "aa b");
        assert_eq!((glyphs[0].from, glyphs[1].from), (6.0, 14.0), "each on its frame's centre");
        assert!((glyphs[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn pieces_linked_across_make_one_level_line_and_a_split_one_is_joined() {
        // Two rows of a 2 × 6 grid on, linked left and right only.
        let (gh, gw) = (2usize, 6usize);
        let plane = gh * gw;
        let mut scores = vec![0f32; plane];
        for x in 0..6 {
            scores[x] = 0.9;
        }
        let mut links = vec![0f32; 8 * plane];
        for i in 0..plane {
            links[3 * plane + i] = 0.9;
            links[4 * plane + i] = 0.9;
        }
        let pieces = components(&scores, &links, gh, gw, 0.7);
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].len(), 6);
        // Each piece half a unit (16 px at stride 4) each way from its centre (2 to 22 across, 2 down).
        let mut deltas = vec![0f32; 8 * plane];
        for i in 0..plane {
            for (k, v) in [-0.5f32, -0.5, 0.5, -0.5, 0.5, 0.5, -0.5, 0.5].iter().enumerate() {
                deltas[k * plane + i] = *v;
            }
        }
        let line = line_of(&pieces[0], &scores, &deltas, gh, gw, 4, false);
        assert!(line.angle.abs() < 1e-4);
        let (x0, y0, x1, y1) = line.bounds();
        assert!((x0 - -14.0).abs() < 1e-3 && (x1 - 38.0).abs() < 1e-3, "{x0} {x1}");
        assert!((y0 - -14.0).abs() < 1e-3 && (y1 - 18.0).abs() < 1e-3, "{y0} {y1}");
        let word = |x0: f32, x1: f32| Found { quad: [[x0, 0.0], [x1, 0.0], [x1, 40.0], [x0, 40.0]], score: 0.9, pieces: 4, down: false, angle: 0.0, stride: 8 };
        let joined = join_words(vec![word(0.0, 100.0), word(120.0, 200.0)]);
        assert_eq!(joined.len(), 1, "a gap under the height");
        assert_eq!(joined[0].bounds(), (0.0, 0.0, 200.0, 40.0));
        assert_eq!(join_words(vec![word(0.0, 100.0), word(150.0, 200.0)]).len(), 2, "a gap over the height");
        let big = Found { quad: [[0.0, 0.0], [100.0, 0.0], [100.0, 50.0], [0.0, 50.0]], stride: 4, ..word(0.0, 100.0) };
        assert!(big.too_big(), "50 px is over 1.35 × 8 strides of 4");
        assert!(!word(0.0, 100.0).too_big());
    }

    /// The real model file in `LOOKITAN_ONEOCR` (CI fetches it as ocr/store.rs does, on Windows, and
    /// shares it with Linux through its cache) on the Russian and Japanese fixtures; skipped without it.
    #[test]
    #[ignore]
    fn real_oneocr_onnx_reads_the_fixtures() {
        use unicode_normalization::UnicodeNormalization;
        let Some(dir) = std::env::var_os("LOOKITAN_ONEOCR").map(std::path::PathBuf::from) else {
            eprintln!("LOOKITAN_ONEOCR is not set; skipped");
            return;
        };
        let path = dir.join("oneocr.onemodel");
        if !path.is_file() {
            eprintln!("{} is missing; skipped", path.display());
            return;
        }
        let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let library = resources.join("ort").join(if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" });
        ort::init_from(&library).expect("ONNX Runtime").with_name("lookitan-test").commit();
        let model = OneModel::open(&path).expect("open the model file");
        model.check().expect("every model OneOCR needs");
        let engine = OneOcrOnnx::new(&path, false, None).expect("load OneOCR's detector");
        let plain = |s: &str| s.nfkc().filter(|c| !c.is_whitespace()).collect::<Vec<char>>();
        let cer = |got: &str, want: &str| {
            let (a, b) = (plain(got), plain(want));
            let mut d: Vec<usize> = (0..=b.len()).collect();
            for i in 1..=a.len() {
                let mut prev = d[0];
                d[0] = i;
                for j in 1..=b.len() {
                    let cur = (d[j] + 1).min(d[j - 1] + 1).min(prev + usize::from(a[i - 1] != b[j - 1]));
                    prev = d[j];
                    d[j] = cur;
                }
            }
            d[b.len()] as f32 / b.len() as f32
        };
        let japanese = "今日はいい天気ですね。散歩に行きましょう！「本当に？」と彼女は小さな声で聞いた。セーブデータを読み込んでいます…東京駅から新幹線で大阪へ向かう。HPが足りない！回復アイテムを使いますか？".repeat(2);
        let fixtures: [(&[u8], &str, &str); 2] = [
            (include_bytes!("fixtures/ru_lines.png"), "ru-RU", "Съешь же ещё этих мягких французских булок ВСЁ ЕЩЁ ЗДЕСЬ, а ёлка — в лесу. Шелестят листья!"),
            (include_bytes!("fixtures/ja_lines.png"), "ja-JP", &japanese),
        ];
        for (png, tag, want) in fixtures {
            let img = image::load_from_memory(png).unwrap().to_rgba8();
            let started = std::time::Instant::now();
            let boxes = engine.detect_boxes(&img).expect("detect");
            let lines = super::super::paddle::recognize_boxes_with(&img, &boxes, &engine.reader(tag));
            let got: String = lines.iter().map(|l| l.text.as_str()).collect();
            let e = cer(&got, want);
            eprintln!("OneOCR (ONNX) {tag}: {} boxes, {} lines in {} ms, CER {:.1}%\n{got}", boxes.len(), lines.len(), started.elapsed().as_millis(), e * 100.0);
            assert!(e <= 0.10, "CER {e}");
        }
    }
}
