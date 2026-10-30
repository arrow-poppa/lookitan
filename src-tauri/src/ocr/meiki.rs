// MeikiOCR's recognizer (rtr46/meikiocr, code Apache-2.0, weights LGPL-3.0) reading the boxes of
// PP-OCR's detector, for Japanese. It is a character detector (D-FINE), not CTC: each character
// comes with its own box and score, at most 48 on a 960×32 line and 24 on a 32×480 column.
//
// As meikiocr does: the crop is scaled to the model's height (a column to its width), placed at
// the top left, divided by 255; characters scoring under 0.1 or starting in the padding are
// dropped, punctuation's scores count a fifth (owocr's factor) and overlapping characters keep the
// surest (1D suppression at 0.3 of the shorter one). Lookitan's own: the padding is the crop's
// background colour, not black (black padding made a bar or a "。" at the end of lines in our
// tests), and a line longer than 960 columns is read in overlapping windows, as meikiocr reads
// long columns, instead of being shrunk.
//
// On the GPU the models run with one change (`for_gpu`): DirectML multiplies a matrix by a 1-D
// weight wrongly (D-FINE's box integral, measured on 2026-10-28 with ONNX Runtime 1.24.4 on an AMD
// Radeon: every character wrong, no error), so that weight is given as a one-column matrix, which
// the Reshape after it turns into the same thing. The files on disk are not changed.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use image::RgbaImage;
use ort::value::Tensor;
use parking_lot::Mutex;

use super::models::MEIKI;
use super::paddle::{cpu_session, gpu_session_from_memory, BoxReader, Glyph, InferenceSession};
use crate::settings::GpuPreference;

/// The line model's input: 32 rows of 960 columns, at most 48 characters.
const LINE: (usize, usize) = (960, 32);
/// The column model's input: 480 rows of 32 columns, at most 24 characters.
const COLUMN: (usize, usize) = (32, 480);
/// A long column is read in windows of 420 rows overlapping by 64, as meikiocr does; a long line in
/// windows of the whole width overlapping by 128 columns (four characters).
const COLUMN_WINDOW: usize = 420;
const COLUMN_OVERLAP: usize = 64;
const LINE_OVERLAP: usize = 128;
/// Lines (or column windows) per model run, as meikiocr batches them.
const BATCH: usize = 8;
const MIN_SCORE: f32 = 0.1;
const PUNCT_FACTOR: f32 = 0.2;
const OVERLAP: f32 = 0.3;
/// Pairs the model reads the wrong way round (meikiocr's table), turned back.
const SWAPPED: [(char, char); 8] = [('傀', '儡'), ('冗', '談'), ('淘', '汰'), ('滂', '沱'), ('痙', '攣'), ('酩', '酊'), ('麺', '麭'), ('慟', '哭')];

pub struct MeikiRec {
    line: Mutex<InferenceSession>,
    column: Mutex<InferenceSession>,
    line_on_gpu: Arc<AtomicBool>,
}

/// One window of a crop, as the model sees it.
struct Window {
    crop: usize,
    /// Where the window starts along the crop, in model pixels.
    offset: usize,
    /// How far along the window the crop's pixels reach.
    content: usize,
}

impl MeikiRec {
    pub fn new(dir: &Path, gpu: bool, choice: Option<GpuPreference>) -> Result<Self> {
        let open = |name: &str, stage: &'static str| -> Result<(InferenceSession, Arc<AtomicBool>)> {
            let path = dir.join(name);
            if !path.is_file() {
                bail!("MeikiOCR model not found: {}", path.display());
            }
            let (session, on_gpu) = if gpu {
                let model = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
                let fixed = for_gpu(&model).unwrap_or_else(|e| {
                    log::warn!("MeikiOCR's {stage} model is used as it is on the GPU: {e:#}");
                    None
                });
                match gpu_session_from_memory(fixed.as_deref().unwrap_or(&model), choice) {
                    Ok(s) => (s, true),
                    Err(e) => {
                        log::warn!("Ocr.GpuFallback.{stage}: {e:#}");
                        (cpu_session(&path)?, false)
                    }
                }
            } else {
                (cpu_session(&path)?, false)
            };
            let on_gpu = Arc::new(AtomicBool::new(on_gpu));
            Ok((InferenceSession { session: Some(session), on_gpu: on_gpu.clone(), cpu_path: path, stage }, on_gpu))
        };
        let (line, line_on_gpu) = open(MEIKI.files[0].name, "MeikiLine")?;
        let (column, _) = open(MEIKI.files[1].name, "MeikiColumn")?;
        log::info!("MeikiOCR ready on {}", if line_on_gpu.load(Ordering::Relaxed) { "GPU" } else { "CPU" });
        Ok(Self { line: Mutex::new(line), column: Mutex::new(column), line_on_gpu })
    }

    pub fn on_gpu(&self) -> bool {
        self.line_on_gpu.load(Ordering::Relaxed)
    }

    /// Reads the crops of one kind (`down`: columns) and adds their characters to `found`.
    fn read_kind(&self, crops: &[(&RgbaImage, bool)], down: bool, found: &mut [Vec<Glyph>]) -> Result<()> {
        let (mw, mh) = if down { COLUMN } else { LINE };
        // Each crop scaled to the model's height (a column to its width), with its background.
        let mut scaled: Vec<Option<(RgbaImage, [f32; 3], f32)>> = Vec::with_capacity(crops.len());
        let mut windows: Vec<Window> = Vec::new();
        for (i, (crop, is_down)) in crops.iter().enumerate() {
            let (w, h) = crop.dimensions();
            if *is_down != down || w < 2 || h < 2 {
                scaled.push(None);
                continue;
            }
            let scale = if down { mw as f32 / w as f32 } else { mh as f32 / h as f32 };
            let (sw, sh) = if down {
                (mw as u32, ((h as f32 * scale).round() as u32).max(1))
            } else {
                (((w as f32 * scale).round() as u32).max(1), mh as u32)
            };
            let image = image::imageops::resize(*crop, sw, sh, FilterType::Triangle);
            let length = if down { sh as usize } else { sw as usize };
            let (window, overlap) = if down { (COLUMN_WINDOW, COLUMN_OVERLAP) } else { (mw, LINE_OVERLAP) };
            for offset in window_starts(length, window, overlap) {
                windows.push(Window { crop: i, offset, content: (length - offset).min(window) });
            }
            scaled.push(Some((image, background(crop), scale)));
        }
        for run in windows.chunks(BATCH) {
            // The model scales its boxes to this size: the input's own, so they come in its pixels.
            let sizes = ortx!(Tensor::from_array(([run.len(), 2], [mw as i64, mh as i64].repeat(run.len()))));
            let plane = mw * mh;
            let mut data = vec![0f32; run.len() * 3 * plane];
            for (k, win) in run.iter().enumerate() {
                let Some((image, fill, _)) = &scaled[win.crop] else { continue };
                let slot = &mut data[k * 3 * plane..(k + 1) * 3 * plane];
                for c in 0..3 {
                    slot[c * plane..(c + 1) * plane].fill(fill[c]);
                }
                let (iw, ih) = image.dimensions();
                for y in 0..mh {
                    for x in 0..mw {
                        // The window's pixel along the crop.
                        let (sx, sy) = if down { (x, y + win.offset) } else { (x + win.offset, y) };
                        if (down && y >= win.content) || (!down && x >= win.content) || sx >= iw as usize || sy >= ih as usize {
                            continue;
                        }
                        let p = image.get_pixel(sx as u32, sy as u32).0;
                        for c in 0..3 {
                            slot[c * plane + y * mw + x] = p[c] as f32 / 255.0;
                        }
                    }
                }
            }
            let input = ortx!(Tensor::from_array(([run.len(), 3, mh, mw], data)));
            let session = if down { &self.column } else { &self.line };
            let (count, codes, boxes, scores) = session.lock().run(cpu_session, |s| {
                let outputs = ortx!(s.run(ort::inputs![input.view(), sizes.view()]));
                let (shape, codes) = ortx!(outputs["char_codes"].try_extract_tensor::<i32>());
                let (_, boxes) = ortx!(outputs["boxes"].try_extract_tensor::<f32>());
                let (_, scores) = ortx!(outputs["scores"].try_extract_tensor::<f32>());
                if shape.len() != 2 || shape[0] as usize != run.len() {
                    bail!("unexpected MeikiOCR output shape {:?}", &shape[..]);
                }
                Ok((shape[1] as usize, codes.to_vec(), boxes.to_vec(), scores.to_vec()))
            })?;
            if boxes.len() < run.len() * count * 4 || scores.len() < run.len() * count {
                bail!("MeikiOCR returned too few boxes");
            }
            for (k, win) in run.iter().enumerate() {
                let Some((_, _, scale)) = &scaled[win.crop] else { continue };
                for j in 0..count {
                    let at = k * count + j;
                    let mut score = scores[at];
                    let b = &boxes[at * 4..at * 4 + 4];
                    let (a0, a1) = if down { (b[1], b[3]) } else { (b[0], b[2]) };
                    let Some(ch) = char::from_u32(codes[at] as u32).filter(|c| !c.is_control() && *c != '\u{FFFD}') else { continue };
                    // Characters that start in the padding are not on the crop.
                    if score < MIN_SCORE || a0 >= win.content as f32 || a1 <= a0 {
                        continue;
                    }
                    if is_punctuation(ch) {
                        score *= PUNCT_FACTOR;
                    }
                    let along = |v: f32| (v.min(win.content as f32) + win.offset as f32) / scale;
                    found[win.crop].push(Glyph { ch, from: along(a0), to: along(a1), score });
                }
            }
        }
        Ok(())
    }
}

impl BoxReader for MeikiRec {
    fn read(&self, crops: &[(&RgbaImage, bool)]) -> Result<Vec<Vec<Glyph>>> {
        let mut found: Vec<Vec<Glyph>> = crops.iter().map(|_| Vec::new()).collect();
        for down in [false, true] {
            if crops.iter().any(|(_, d)| *d == down) {
                self.read_kind(crops, down, &mut found)?;
            }
        }
        Ok(found.into_iter().map(suppress).collect())
    }
}

/// Where the windows of a crop `length` long start: one window when it fits, else windows
/// overlapping by `overlap`, the last one ending with the crop.
fn window_starts(length: usize, window: usize, overlap: usize) -> Vec<usize> {
    if length <= window {
        return vec![0];
    }
    let step = window - overlap;
    let mut starts: Vec<usize> = (0..).map(|k| k * step).take_while(|&s| s + window < length).collect();
    starts.push(length - window);
    starts
}

/// The crop's background: the median of its border pixels, as the model reads (0..1).
fn background(crop: &RgbaImage) -> [f32; 3] {
    let (w, h) = crop.dimensions();
    let mut border: Vec<[u8; 4]> = Vec::with_capacity(2 * (w + h) as usize);
    for x in 0..w {
        border.push(crop.get_pixel(x, 0).0);
        border.push(crop.get_pixel(x, h - 1).0);
    }
    for y in 0..h {
        border.push(crop.get_pixel(0, y).0);
        border.push(crop.get_pixel(w - 1, y).0);
    }
    let mut fill = [1.0f32; 3];
    for (c, out) in fill.iter_mut().enumerate() {
        let mut values: Vec<u8> = border.iter().map(|p| p[c]).collect();
        values.sort_unstable();
        if let Some(&m) = values.get(values.len() / 2) {
            *out = m as f32 / 255.0;
        }
    }
    fill
}

/// Unicode punctuation as the model's scores treat it: ASCII, general and CJK punctuation, and the
/// full-width forms of ASCII's.
fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(c as u32, 0x2010..=0x2027 | 0x2030..=0x205E | 0x3001..=0x3003 | 0x3008..=0x3011 | 0x3014..=0x301F | 0x30FB
            | 0xFF01..=0xFF0F | 0xFF1A..=0xFF20 | 0xFF3B..=0xFF40 | 0xFF5B..=0xFF65)
}

/// One crop's characters: of those overlapping by more than `OVERLAP` of the shorter one the
/// surest, in order along the crop.
fn suppress(mut glyphs: Vec<Glyph>) -> Vec<Glyph> {
    glyphs.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Glyph> = Vec::with_capacity(glyphs.len());
    for g in glyphs {
        let clashes = kept.iter().any(|k| {
            let inter = g.to.min(k.to) - g.from.max(k.from);
            inter > OVERLAP * (g.to - g.from).min(k.to - k.from)
        });
        if !clashes {
            kept.push(g);
        }
    }
    kept.sort_by(|a, b| a.from.total_cmp(&b.from));
    for i in 1..kept.len() {
        if SWAPPED.iter().any(|&(a, b)| kept[i - 1].ch == b && kept[i].ch == a) {
            let first = kept[i - 1].ch;
            kept[i - 1].ch = kept[i].ch;
            kept[i].ch = first;
        }
    }
    kept
}

// ---------------------------------------------------------------------------- the GPU's change

/// The model with every 1-D weight of a MatMul given as a one-column matrix, when only Reshapes
/// read those MatMuls' products (so nothing downstream changes); `None` when there is none. ONNX's
/// protobuf is read and written field by field, the rest kept byte for byte.
pub(super) fn for_gpu(model: &[u8]) -> Result<Option<Vec<u8>>> {
    // ModelProto.graph; GraphProto.node and .initializer; NodeProto.input, .output and .op_type;
    // TensorProto.dims and .name.
    const GRAPH: u64 = 7;
    const NODE: u64 = 1;
    const INITIALIZER: u64 = 5;
    const INPUT: u64 = 1;
    const OUTPUT: u64 = 2;
    const OP_TYPE: u64 = 4;
    const DIMS: u64 = 1;
    const NAME: u64 = 8;

    let top = fields(model)?;
    let graph = top.iter().find(|f| f.number == GRAPH && f.wire == 2).context("the model has no graph")?;
    let graph_fields = fields(graph.payload)?;
    let mut nodes: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    for f in graph_fields.iter().filter(|f| f.number == NODE && f.wire == 2) {
        let (mut op, mut inputs, mut outputs) = (String::new(), Vec::new(), Vec::new());
        for g in fields(f.payload)?.into_iter().filter(|g| g.wire == 2) {
            let text = String::from_utf8_lossy(g.payload).into_owned();
            match g.number {
                INPUT => inputs.push(text),
                OUTPUT => outputs.push(text),
                OP_TYPE => op = text,
                _ => {}
            }
        }
        nodes.push((op, inputs, outputs));
    }
    let mut readers: HashMap<&str, Vec<&str>> = HashMap::new();
    for (op, inputs, _) in &nodes {
        for i in inputs {
            readers.entry(i.as_str()).or_default().push(op.as_str());
        }
    }
    let only_reshapes = |value: &str| readers.get(value).is_some_and(|r| !r.is_empty() && r.iter().all(|op| *op == "Reshape"));
    let weights: HashSet<&str> = nodes
        .iter()
        .filter(|(op, inputs, _)| op == "MatMul" && inputs.len() == 2)
        .map(|(_, inputs, _)| inputs[1].as_str())
        .filter(|w| {
            readers[w].iter().all(|op| *op == "MatMul")
                && nodes
                    .iter()
                    .filter(|(op, inputs, _)| op == "MatMul" && inputs.iter().any(|i| i == w))
                    .all(|(_, inputs, outputs)| inputs[0] != *w && outputs.iter().all(|o| only_reshapes(o)))
        })
        .collect();

    let mut changed = false;
    let mut graph_out = Vec::with_capacity(graph.payload.len() + 64);
    for f in &graph_fields {
        if f.number == INITIALIZER && f.wire == 2 {
            let tensor = fields(f.payload)?;
            let name = tensor.iter().find(|t| t.number == NAME && t.wire == 2).map(|t| t.payload);
            let mut dims = Vec::new();
            for t in tensor.iter().filter(|t| t.number == DIMS) {
                match t.wire {
                    0 => dims.push(varint(t.payload, &mut 0)?),
                    2 => {
                        let mut at = 0;
                        while at < t.payload.len() {
                            dims.push(varint(t.payload, &mut at)?);
                        }
                    }
                    _ => bail!("a tensor's dims are not integers"),
                }
            }
            if dims.len() == 1 && name.is_some_and(|n| std::str::from_utf8(n).is_ok_and(|n| weights.contains(n))) {
                let mut out = Vec::with_capacity(f.payload.len() + 4);
                for d in [dims[0], 1] {
                    put_varint(&mut out, DIMS << 3);
                    put_varint(&mut out, d);
                }
                for t in tensor.iter().filter(|t| t.number != DIMS) {
                    out.extend_from_slice(t.raw);
                }
                put_field(&mut graph_out, INITIALIZER, &out);
                changed = true;
                continue;
            }
        }
        graph_out.extend_from_slice(f.raw);
    }
    if !changed {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(model.len() + 64);
    for f in &top {
        if f.number == GRAPH && f.wire == 2 {
            put_field(&mut out, GRAPH, &graph_out);
        } else {
            out.extend_from_slice(f.raw);
        }
    }
    Ok(Some(out))
}

/// A protobuf field: its number, wire type, its bytes as they are and its value's (a varint's, or
/// what a length-delimited field holds).
struct Field<'a> {
    number: u64,
    wire: u8,
    raw: &'a [u8],
    payload: &'a [u8],
}

fn fields(buf: &[u8]) -> Result<Vec<Field<'_>>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < buf.len() {
        let start = at;
        let key = varint(buf, &mut at)?;
        let wire = (key & 7) as u8;
        let value = at;
        let end = match wire {
            0 => {
                varint(buf, &mut at)?;
                at
            }
            1 => at + 8,
            2 => {
                let len = usize::try_from(varint(buf, &mut at)?)?;
                at.checked_add(len).context("a field longer than the model")?
            }
            5 => at + 4,
            _ => bail!("the model is not protobuf (wire type {wire})"),
        };
        if end > buf.len() {
            bail!("the model ends inside a field");
        }
        let payload = if wire == 2 { &buf[at..end] } else { &buf[value..end] };
        at = end;
        out.push(Field { number: key >> 3, wire, raw: &buf[start..end], payload });
    }
    Ok(out)
}

fn varint(buf: &[u8], at: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*at).context("the model ends inside a number")?;
        *at += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    bail!("a number in the model is too long")
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// A length-delimited field.
fn put_field(out: &mut Vec<u8>, number: u64, payload: &[u8]) {
    put_varint(out, number << 3 | 2);
    put_varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    type Value = std::result::Result<u64, Vec<u8>>;

    /// A protobuf message from its fields: a varint, or the bytes of a length-delimited one.
    fn message(fields: &[(u64, Value)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (n, v) in fields {
            match v {
                Ok(v) => {
                    put_varint(&mut out, n << 3);
                    put_varint(&mut out, *v);
                }
                Err(bytes) => put_field(&mut out, *n, bytes),
            }
        }
        out
    }

    fn text(s: &str) -> Value {
        Err(s.as_bytes().to_vec())
    }

    fn node(op: &str, inputs: &[&str], output: &str) -> Value {
        let mut f: Vec<(u64, Value)> = inputs.iter().map(|i| (1, text(i))).collect();
        f.push((2, text(output)));
        f.push((4, text(op)));
        Err(message(&f))
    }

    const RAW: [u8; 12] = [0, 0, 128, 63, 0, 0, 0, 64, 0, 0, 64, 64];

    fn tensor(name: &str, dims: &[u64]) -> Value {
        let mut f: Vec<(u64, Value)> = dims.iter().map(|d| (1, Ok(*d))).collect();
        f.push((2, Ok(1)));
        f.push((8, text(name)));
        f.push((9, Err(RAW.to_vec())));
        Err(message(&f))
    }

    /// The dims of each initializer of a model.
    fn dims_of(model: &[u8]) -> HashMap<String, Vec<u64>> {
        let top = fields(model).unwrap();
        let graph = top.iter().find(|f| f.number == 7).unwrap();
        let mut out = HashMap::new();
        for f in fields(graph.payload).unwrap().into_iter().filter(|f| f.number == 5) {
            let t = fields(f.payload).unwrap();
            let name = String::from_utf8(t.iter().find(|x| x.number == 8).unwrap().payload.to_vec()).unwrap();
            assert_eq!(t.iter().find(|x| x.number == 9).unwrap().payload, &RAW[..], "the weights are kept");
            out.insert(name, t.iter().filter(|x| x.number == 1).map(|x| varint(x.payload, &mut 0).unwrap()).collect());
        }
        out
    }

    #[test]
    fn a_1d_matmul_weight_read_only_through_reshapes_becomes_a_column_on_the_gpu() {
        let graph = message(&[
            (1, node("MatMul", &["x", "w"], "y")),
            (1, node("Reshape", &["y", "shape"], "z")),
            (1, node("MatMul", &["x", "w"], "y2")),
            (1, node("Reshape", &["y2", "shape"], "z2")),
            // Its product added to something: [n] and [n, 1] would broadcast differently.
            (1, node("MatMul", &["x", "v"], "u")),
            (1, node("Add", &["u", "b"], "out")),
            (2, text("graph")),
            (5, tensor("w", &[3])),
            (5, tensor("v", &[3])),
            (5, tensor("m", &[3, 2])),
        ]);
        let model = message(&[(1, Ok(8)), (7, Err(graph)), (8, Err(message(&[(2, Ok(16))])))]);
        let fixed = for_gpu(&model).unwrap().expect("w changes");
        let dims = dims_of(&fixed);
        assert_eq!(dims["w"], vec![3, 1]);
        assert_eq!(dims["v"], vec![3], "read by an Add");
        assert_eq!(dims["m"], vec![3, 2]);
        // The other fields are kept: the IR version and the opset.
        let (before, after) = (fields(&model).unwrap(), fields(&fixed).unwrap());
        assert_eq!(after[0].raw, before[0].raw);
        assert_eq!(after.last().unwrap().raw, before.last().unwrap().raw);
        // Nothing to change: none.
        let plain = message(&[(7, Err(message(&[(1, node("Add", &["a", "b"], "c")), (5, tensor("a", &[3]))])))]);
        assert!(for_gpu(&plain).unwrap().is_none());
        assert!(for_gpu(&[0x0f]).is_err(), "not protobuf");
    }

    fn g(ch: char, from: f32, to: f32, score: f32) -> Glyph {
        Glyph { ch, from, to, score }
    }

    /// PP-OCR's det_small boxes read by MeikiOCR on Japanese lines, dark on white and light on a
    /// dark blue panel. Needs the fetched ONNX Runtime and models, and MeikiOCR's models in
    /// `LOOKITAN_MEIKI` (CI downloads them; the test is skipped without them).
    #[test]
    #[ignore]
    fn real_meiki_reads_the_fixture_on_paddle_s_boxes() {
        use crate::ocr::paddle::{recognize_boxes_with, EnginePlan, PaddleOcr};
        use unicode_normalization::UnicodeNormalization;
        const LINES: [&str; 5] = [
            "今日はいい天気ですね。散歩に行きましょう！",
            "「本当に？」と彼女は小さな声で聞いた。",
            "セーブデータを読み込んでいます…",
            "東京駅から新幹線で大阪へ向かう。",
            "HPが足りない！回復アイテムを使いますか？",
        ];
        let Some(dir) = std::env::var_os("LOOKITAN_MEIKI").map(std::path::PathBuf::from) else {
            eprintln!("LOOKITAN_MEIKI is not set; skipped");
            return;
        };
        let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let library = resources.join("ort").join(if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" });
        ort::init_from(&library).expect("ONNX Runtime").with_name("lookitan-test").commit();
        let plan = EnginePlan { manual: true, det: "det_small", det_gpu: false, rec_gpu: false, gpu_choice: None };
        let paddle = PaddleOcr::new(&resources.join("models"), "rec_small", plan).unwrap();
        let meiki = MeikiRec::new(&dir, false, None).unwrap();
        let img = image::load_from_memory(include_bytes!("fixtures/ja_lines.png")).unwrap().to_rgba8();
        let boxes = paddle.detect_boxes(&img).unwrap();
        let lines = recognize_boxes_with(&img, &boxes, &meiki);
        // Full-width and half-width forms are the same word to the dictionaries.
        let plain = |s: &str| s.nfkc().filter(|c| !c.is_whitespace()).collect::<String>();
        let got = plain(&lines.iter().map(|l| l.text.as_str()).collect::<String>());
        let want = plain(&LINES.repeat(2).concat());
        let (a, b): (Vec<char>, Vec<char>) = (got.chars().collect(), want.chars().collect());
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
        let cer = d[b.len()] as f32 / b.len() as f32;
        let read: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        eprintln!("MeikiOCR on {} boxes: CER {:.1}%\n{}", boxes.len(), cer * 100.0, read.join("\n"));
        assert!((9..=12).contains(&lines.len()), "about a line per fixture line: {}", lines.len());
        assert!(cer <= 0.05, "CER {cer}");
        // Each character has a span inside its line, in order.
        for l in &lines {
            assert!(l.chars.windows(2).all(|w| w[0].x0 <= w[1].x0 + 0.5), "{}", l.text);
            assert!(l.chars.iter().all(|c| c.x0 >= l.x0 - 1.0 && c.x1 <= l.x1 + 1.0), "{}", l.text);
        }
        // The GPU's version of each model reads the same as the file does, here on the CPU.
        for (name, (w, h)) in [(MEIKI.files[0].name, LINE), (MEIKI.files[1].name, COLUMN)] {
            let model = std::fs::read(dir.join(name)).unwrap();
            let fixed = for_gpu(&model).unwrap().expect("the box integral's weight");
            let data: Vec<f32> = (0..3 * w * h).map(|i| ((i * 7919) % 255) as f32 / 255.0).collect();
            let read = |bytes: &[u8]| {
                let mut s = ort::session::Session::builder().unwrap().commit_from_memory(bytes).unwrap();
                let input = Tensor::from_array(([1usize, 3, h, w], data.clone())).unwrap();
                let sizes = Tensor::from_array(([1usize, 2], vec![w as i64, h as i64])).unwrap();
                let out = s.run(ort::inputs![input.view(), sizes.view()]).unwrap();
                let (_, codes) = out["char_codes"].try_extract_tensor::<i32>().unwrap();
                let (_, scores) = out["scores"].try_extract_tensor::<f32>().unwrap();
                (codes.to_vec(), scores.to_vec())
            };
            let (a, b) = (read(&model), read(&fixed));
            assert_eq!(a.0, b.0, "{name}");
            assert!(a.1.iter().zip(&b.1).all(|(x, y)| (x - y).abs() < 1e-4), "{name}");
        }
    }

    #[test]
    fn a_long_crop_is_read_in_overlapping_windows_ending_with_it() {
        assert_eq!(window_starts(500, 960, 128), vec![0]);
        assert_eq!(window_starts(960, 960, 128), vec![0]);
        assert_eq!(window_starts(2000, 960, 128), vec![0, 832, 1040]);
        assert_eq!(window_starts(1000, 420, 64), vec![0, 356, 580]);
    }

    #[test]
    fn overlapping_characters_keep_the_surest_and_swapped_pairs_turn_back() {
        let kept = suppress(vec![g('日', 0.0, 30.0, 0.9), g('目', 2.0, 31.0, 0.4), g('本', 32.0, 62.0, 0.8)]);
        assert_eq!(kept.iter().map(|g| g.ch).collect::<String>(), "日本");
        // Side by side, touching: both stay.
        assert_eq!(suppress(vec![g('a', 0.0, 10.0, 0.5), g('b', 8.0, 20.0, 0.5)]).len(), 2);
        let turned = suppress(vec![g('儡', 0.0, 30.0, 0.9), g('傀', 31.0, 60.0, 0.9)]);
        assert_eq!(turned.iter().map(|g| g.ch).collect::<String>(), "傀儡");
    }

    #[test]
    fn the_padding_is_the_crop_s_background() {
        let mut crop = RgbaImage::from_pixel(40, 10, image::Rgba([20, 30, 70, 255]));
        crop.put_pixel(20, 5, image::Rgba([255, 255, 255, 255]));
        let fill = background(&crop);
        assert!((fill[0] - 20.0 / 255.0).abs() < 1e-6 && (fill[2] - 70.0 / 255.0).abs() < 1e-6);
        assert!(is_punctuation('。') && is_punctuation('！') && is_punctuation('?') && !is_punctuation('あ'));
    }
}
