// IndraOCR's recognizers (datalab-to/indra, code Apache-2.0; weights under the modified AI Pubs Open
// RAIL-M licence), at the user's request on 2026-10-28, and its line detector (LineNet, `IndraDet`),
// since 2026-10-29 IndraOCR's own detector (any other detector's boxes can be read too). The weights
// are downloaded only after the user accepts their licence in Configurações › OCR, never bundled.
//
// LineNet, as indra_ocr's `detect` (pipeline.py, det_post.py): the page grey, as ink (1 − grey/255),
// its long side at 1024 px, padded with paper (0) to a multiple of 32; its output, at half that size,
// is each line's core (a logit, a line where it is over 0, 4-connected, at least 2 pixels) and, at
// each core pixel, how far up and down the line's ink goes. A line is its core's mean top and
// bottom, its core's width grown by 0.15 of its height at each end. With 8 lines or more whose median
// height is under 9 px, the page is read again larger, for 12 px lines (at most 2048 px). Indra's
// page orientation, deskew and picture regions are not used: lines are horizontal.
//
// Only the recognizer group of the OCR language is loaded: `base` (Latin, Cyrillic, Greek; a window
// three lines tall), `cjk` or `rtl`. As indra_ocr's `crop_line` and greedy CTC decoding: the box
// padded by 0.3 of its height on each side and 0.2 above and below (1.6 in the base model's window),
// grey, inverted to an ink map (0 is paper; a light-on-dark line is read as it is), resized to the
// model's height, right-padded with 0 to a multiple of 8; lines under a mean confidence of 0.5 are
// dropped. Lookitan's own: outside the box is the fill indra uses beyond a page's edge (white, or
// black for a dark line), since a `BoxReader` gets the box only (it read as well as with the page
// around it in our tests), and the beam search with Indra's language model is not ported.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use image::{GrayImage, Luma, RgbaImage};
use ort::value::Tensor;
use parking_lot::Mutex;
use unicode_normalization::char::compose;

use super::paddle::{cpu_session, gpu_session, BoxReader, Glyph, InferenceSession, TextBox};
use crate::settings::GpuPreference;

const LINE_HEIGHT: u32 = 32;
const PAD_X: f32 = 0.3;
const PAD_Y: f32 = 0.2;
const WIDTH_MULTIPLE: usize = 8;
const MAX_WIDTH: u32 = 4096;
const MIN_CONFIDENCE: f32 = 0.5;
/// Lines per model run, of similar width (as indra batches them).
const BATCH: usize = 16;
const DET_LONG_SIDE: u32 = 1024;
const DET_MULTIPLE: u32 = 32;
/// Input pixels per pixel of LineNet's output.
const DET_STRIDE: f32 = 2.0;
const ADAPTIVE_MIN_H: f32 = 9.0;
const ADAPTIVE_TARGET_H: f32 = 12.0;
const ADAPTIVE_MAX_SIDE: f32 = 2048.0;
/// How much of its height a line's core lacks at each end.
const CORE_X_SHRINK: f32 = 0.15;

/// A pixel's grey, as OpenCV reads a picture in grey.
fn grey(p: &image::Rgba<u8>) -> u8 {
    ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8
}

/// A recognizer group of Indra's, and what it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// Latin, Cyrillic and Greek; its model sees a window three lines tall.
    Base,
    Cjk,
    /// Arabic, Persian, Urdu, Hebrew; it reads as drawn, right to left.
    Rtl,
}

impl Group {
    /// The group reading an OCR language (`ocr::LANGUAGES`).
    pub fn of(tag: &str) -> Self {
        match super::language(tag).rec {
            "rec_arabic" => Group::Rtl,
            // Hebrew, which PP-OCR does not read, Indra's right-to-left model does.
            super::NO_PPOCR if tag.to_ascii_lowercase().starts_with("he") => Group::Rtl,
            "rec_korean" => Group::Cjk,
            "rec_small" if !tag.starts_with("ro") => Group::Cjk,
            _ => Group::Base,
        }
    }

    fn model(self) -> &'static str {
        match self {
            Group::Base => "rec.onnx",
            Group::Cjk => "rec_cjk.onnx",
            Group::Rtl => "rec_rtl.onnx",
        }
    }

    /// Its character list in meta.json.
    fn vocab(self) -> &'static str {
        match self {
            Group::Base => "vocab",
            Group::Cjk => "vocab_cjk",
            Group::Rtl => "vocab_rtl",
        }
    }

    /// Lines of page the model sees, the line itself in the middle one.
    fn window(self) -> u32 {
        if self == Group::Base { 3 } else { 1 }
    }
}

pub struct IndraRec {
    group: Group,
    session: Mutex<InferenceSession>,
    /// Class `i + 1` is `tokens[i]`; 0 is the CTC blank.
    tokens: Vec<String>,
    on_gpu: Arc<AtomicBool>,
}

/// One crop as the model reads it.
struct Prepared {
    ink: GrayImage,
    /// Crop pixels to one of the model's input, and where the box starts in the padded crop.
    to_crop: f32,
    left: f32,
}

impl IndraRec {
    pub fn new(dir: &Path, group: Group, gpu: bool, choice: Option<GpuPreference>) -> Result<Self> {
        let path = dir.join(group.model());
        if !path.is_file() {
            bail!("IndraOCR model not found: {}", path.display());
        }
        let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("meta.json")).context("read IndraOCR's meta.json")?)?;
        // Each vocabulary is itself JSON, {"blank": 0, "chars": [...]}.
        let vocab: serde_json::Value = serde_json::from_str(meta[group.vocab()].as_str().context("IndraOCR's vocabulary")?)?;
        if vocab["blank"].as_u64().unwrap_or(0) != 0 {
            bail!("IndraOCR's vocabulary has its blank elsewhere than 0");
        }
        let tokens: Vec<String> = vocab["chars"].as_array().context("IndraOCR's characters")?.iter().map(|c| c.as_str().unwrap_or_default().to_string()).collect();
        let (session, on_gpu) = if gpu {
            match gpu_session(&path, choice) {
                Ok(s) => (s, true),
                Err(e) => {
                    log::warn!("Ocr.GpuFallback.Indra: {e:#}");
                    (cpu_session(&path)?, false)
                }
            }
        } else {
            (cpu_session(&path)?, false)
        };
        log::info!("IndraOCR {group:?} ready on {}, {} tokens", if on_gpu { "GPU" } else { "CPU" }, tokens.len());
        let on_gpu = Arc::new(AtomicBool::new(on_gpu));
        let session = InferenceSession { session: Some(session), on_gpu: on_gpu.clone(), cpu_path: path, stage: "Indra" };
        Ok(Self { group, session: Mutex::new(session), tokens, on_gpu })
    }

    pub fn group(&self) -> Group {
        self.group
    }

    pub fn on_gpu(&self) -> bool {
        self.on_gpu.load(Ordering::Relaxed)
    }

    /// The crop padded, grey, as ink, at the model's height (`crop_line`).
    fn prepare(&self, crop: &RgbaImage) -> Prepared {
        let (w, h) = crop.dimensions();
        let grey: Vec<u8> = crop.pixels().map(grey).collect();
        let dark = is_dark(&grey);
        let window = self.group.window();
        let pad_y = (window as f32 * (1.0 + 2.0 * PAD_Y) - 1.0) / 2.0;
        let (hf, wf) = (h as f32, w as f32);
        let (x0, y0) = ((-PAD_X * hf).floor(), (-pad_y * hf).floor());
        let (x1, y1) = ((wf + PAD_X * hf).ceil(), (hf + pad_y * hf).ceil());
        let (cw, ch) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let mut padded = GrayImage::from_pixel(cw.max(1), ch.max(1), Luma([if dark { 0 } else { 255 }]));
        for y in 0..h {
            for x in 0..w {
                padded.put_pixel(x + (-x0) as u32, y + (-y0) as u32, Luma([grey[(y * w + x) as usize]]));
            }
        }
        let height = LINE_HEIGHT * window;
        let width = ((cw as f32 * height as f32 / ch.max(1) as f32).round() as u32).clamp(WIDTH_MULTIPLE as u32, MAX_WIDTH);
        let mut ink = image::imageops::resize(&padded, width, height, FilterType::Triangle);
        // 0 is paper: dark text on light is inverted; a light-on-dark line is read as it is.
        if !dark {
            for p in ink.pixels_mut() {
                p.0[0] = 255 - p.0[0];
            }
        }
        Prepared { ink, to_crop: cw as f32 / width as f32, left: -x0 }
    }

    /// Reads crops already prepared, of similar width, in one run.
    fn run(&self, prepared: &[&Prepared]) -> Result<Vec<Vec<Glyph>>> {
        let height = (LINE_HEIGHT * self.group.window()) as usize;
        let widest = prepared.iter().map(|p| p.ink.width() as usize).max().unwrap_or(WIDTH_MULTIPLE);
        let padded = widest.div_ceil(WIDTH_MULTIPLE) * WIDTH_MULTIPLE;
        let mut data = vec![0f32; prepared.len() * height * padded];
        let mut widths = Vec::with_capacity(prepared.len());
        for (k, p) in prepared.iter().enumerate() {
            let slot = &mut data[k * height * padded..(k + 1) * height * padded];
            for (x, y, v) in p.ink.enumerate_pixels() {
                slot[y as usize * padded + x as usize] = v.0[0] as f32 / 255.0;
            }
            widths.push(p.ink.width() as i64);
        }
        let input = ortx!(Tensor::from_array(([prepared.len(), 1, height, padded], data)));
        let widths = ortx!(Tensor::from_array(([prepared.len()], widths)));
        let (steps, ids, conf, lengths) = self.session.lock().run(cpu_session, |s| {
            let outputs = ortx!(s.run(ort::inputs![input.view(), widths.view()]));
            let (shape, ids) = ortx!(outputs["ids"].try_extract_tensor::<i64>());
            let (_, conf) = ortx!(outputs["conf"].try_extract_tensor::<f32>());
            let (_, lengths) = ortx!(outputs["lengths"].try_extract_tensor::<i64>());
            if shape.len() != 2 || shape[0] as usize != prepared.len() {
                bail!("unexpected IndraOCR output shape {:?}", &shape[..]);
            }
            Ok((shape[1] as usize, ids.to_vec(), conf.to_vec(), lengths.to_vec()))
        })?;
        if steps == 0 || ids.len() < prepared.len() * steps || conf.len() < ids.len() {
            bail!("IndraOCR returned too few steps");
        }
        // Each step covers an equal slice of the padded input.
        let step_px = padded as f32 / steps as f32;
        Ok(prepared
            .iter()
            .enumerate()
            .map(|(k, p)| {
                let valid = (lengths.get(k).copied().unwrap_or(steps as i64).max(0) as usize).min(steps);
                let row = |t: usize| (ids[k * steps + t], conf[k * steps + t]);
                self.decode((0..valid).map(row), step_px, p)
            })
            .collect())
    }

    /// Greedy CTC: repeats collapsed, blanks dropped; each character where its step's centre lies
    /// on the crop. Combining sequences are composed (Hangul jamo into syllables, as indra's NFC).
    fn decode(&self, steps: impl Iterator<Item = (i64, f32)>, step_px: f32, p: &Prepared) -> Vec<Glyph> {
        let (mut glyphs, mut prev, mut sum, mut read) = (Vec::<Glyph>::new(), 0i64, 0f32, 0usize);
        for (t, (id, c)) in steps.enumerate() {
            if id != 0 && id != prev {
                if let Some(token) = usize::try_from(id - 1).ok().and_then(|i| self.tokens.get(i)) {
                    let at = (t as f32 + 0.5) * step_px * p.to_crop - p.left;
                    sum += c;
                    read += 1;
                    for ch in token.chars() {
                        match glyphs.last_mut().and_then(|g| compose(g.ch, ch).map(|composed| (g, composed))) {
                            Some((g, composed)) => g.ch = composed,
                            None => glyphs.push(Glyph { ch, from: at, to: at, score: c }),
                        }
                    }
                }
            }
            prev = id;
        }
        if glyphs.is_empty() || sum / (read.max(1) as f32) < MIN_CONFIDENCE {
            return Vec::new();
        }
        glyphs
    }
}

/// IndraOCR's line detector (LineNet, det.onnx).
pub struct IndraDet {
    session: Mutex<InferenceSession>,
    on_gpu: Arc<AtomicBool>,
}

impl IndraDet {
    pub fn new(dir: &Path, gpu: bool, choice: Option<GpuPreference>) -> Result<Self> {
        let path = dir.join("det.onnx");
        if !path.is_file() {
            bail!("IndraOCR's detector not found: {}", path.display());
        }
        let (session, on_gpu) = if gpu {
            match gpu_session(&path, choice) {
                Ok(s) => (s, true),
                Err(e) => {
                    log::warn!("Ocr.GpuFallback.IndraDet: {e:#}");
                    (cpu_session(&path)?, false)
                }
            }
        } else {
            (cpu_session(&path)?, false)
        };
        log::info!("IndraOCR's LineNet ready on {}", if on_gpu { "GPU" } else { "CPU" });
        let on_gpu = Arc::new(AtomicBool::new(on_gpu));
        let session = InferenceSession { session: Some(session), on_gpu: on_gpu.clone(), cpu_path: path, stage: "IndraDet" };
        Ok(Self { session: Mutex::new(session), on_gpu })
    }

    pub fn on_gpu(&self) -> bool {
        self.on_gpu.load(Ordering::Relaxed)
    }

    /// Runs the model once so the first real lookup does not pay for graph setup.
    pub fn warm_up(&self) {
        let blank = RgbaImage::from_pixel(96, 64, image::Rgba([255, 255, 255, 255]));
        if let Err(e) = self.detect_boxes(&blank) {
            log::warn!("IndraOCR detector warm-up failed: {e:#}");
        }
    }

    /// Every line, top to bottom, in the image's pixels.
    pub fn detect_boxes(&self, img: &RgbaImage) -> Result<Vec<TextBox>> {
        let (w, h) = img.dimensions();
        if w < 8 || h < 8 {
            return Ok(Vec::new());
        }
        let page = GrayImage::from_fn(w, h, |x, y| Luma([grey(img.get_pixel(x, y))]));
        let (mut lines, mut scale) = self.detect_at(&page, DET_LONG_SIDE)?;
        // Lines too small for the model: once more, larger.
        if lines.len() >= 8 {
            let height = median(lines.iter().map(|l| l[3] - l[1]).collect());
            if height < ADAPTIVE_MIN_H {
                let side = (ADAPTIVE_MAX_SIDE.min(DET_LONG_SIDE as f32 * ADAPTIVE_TARGET_H / height.max(1.0)) as u32) / DET_MULTIPLE * DET_MULTIPLE;
                if side > DET_LONG_SIDE {
                    (lines, scale) = self.detect_at(&page, side)?;
                }
            }
        }
        let (wf, hf) = (w as f32, h as f32);
        let mut boxes: Vec<TextBox> = lines
            .into_iter()
            .map(|[x0, y0, x1, y1]| TextBox {
                x0: (x0 / scale).clamp(0.0, wf),
                y0: (y0 / scale).clamp(0.0, hf),
                x1: (x1 / scale).clamp(0.0, wf),
                y1: (y1 / scale).clamp(0.0, hf),
                quad: None,
            })
            .filter(|b| b.x1 - b.x0 >= 1.0 && b.y1 - b.y0 >= 1.0)
            .collect();
        boxes.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
        Ok(boxes)
    }

    /// The lines of `page` with its long side at `long_side`, in those pixels, and that scale.
    fn detect_at(&self, page: &GrayImage, long_side: u32) -> Result<(Vec<[f32; 4]>, f32)> {
        let (w, h) = page.dimensions();
        let scale = long_side as f32 / w.max(h) as f32;
        let (nw, nh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
        let resized = if (nw, nh) == (w, h) { page.clone() } else { image::imageops::resize(page, nw, nh, FilterType::Triangle) };
        let (pw, ph) = (nw.div_ceil(DET_MULTIPLE) * DET_MULTIPLE, nh.div_ceil(DET_MULTIPLE) * DET_MULTIPLE);
        // Ink: 0 is paper, as the padding is.
        let mut data = vec![0f32; (pw * ph) as usize];
        for (x, y, p) in resized.enumerate_pixels() {
            data[(y * pw + x) as usize] = 1.0 - p.0[0] as f32 / 255.0;
        }
        let input = ortx!(Tensor::from_array(([1usize, 1, ph as usize, pw as usize], data)));
        let (dims, out) = self.session.lock().run(cpu_session, |s| {
            let outputs = ortx!(s.run(ort::inputs![input.view()]));
            let (shape, out) = ortx!(outputs[0].try_extract_tensor::<f32>());
            Ok((shape[..].to_vec(), out.to_vec()))
        })?;
        if dims.len() != 4 || dims[1] < 3 {
            bail!("unexpected IndraOCR detector output shape {dims:?}");
        }
        Ok((decode_lines(&out, dims[2] as usize, dims[3] as usize), scale))
    }
}

/// LineNet's lines from its output (core logits, then distances up and down, each `h` × `w`), in
/// its input's pixels: x0, y0, x1, y1.
fn decode_lines(out: &[f32], h: usize, w: usize) -> Vec<[f32; 4]> {
    let plane = h * w;
    if out.len() < 3 * plane {
        return Vec::new();
    }
    let (core, up, down) = (&out[..plane], &out[plane..2 * plane], &out[2 * plane..3 * plane]);
    let mut seen = vec![false; plane];
    let mut stack = Vec::new();
    let mut lines = Vec::new();
    for start in 0..plane {
        // A probability over 0.5 is a logit over 0.
        if seen[start] || core[start] <= 0.0 {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut left, mut right, mut count, mut top, mut bottom) = (usize::MAX, 0, 0usize, 0f32, 0f32);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            left = left.min(x);
            right = right.max(x);
            let centre = (y as f32 + 0.5) * DET_STRIDE;
            top += centre - up[i];
            bottom += centre + down[i];
            count += 1;
            let mut visit = |j: usize| {
                if !seen[j] && core[j] > 0.0 {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        if count < 2 {
            continue;
        }
        let (y0, y1) = (top / count as f32, bottom / count as f32);
        let grow = CORE_X_SHRINK * (y1 - y0).max(1.0);
        lines.push([left as f32 * DET_STRIDE - grow, y0, (right + 1) as f32 * DET_STRIDE + grow, y1]);
    }
    lines
}

/// The median, the mean of the middle two for an even count (as numpy's).
fn median(mut values: Vec<f32>) -> f32 {
    values.sort_by(f32::total_cmp);
    let n = values.len();
    match n {
        0 => 0.0,
        _ if n % 2 == 1 => values[n / 2],
        _ => (values[n / 2 - 1] + values[n / 2]) / 2.0,
    }
}

/// A light-on-dark line (indra's `is_dark_line`): the ground, the median, lies nearer the dark end
/// than the text does, so the bright tail is the longer one.
fn is_dark(grey: &[u8]) -> bool {
    if grey.is_empty() {
        return false;
    }
    let mut sorted = grey.to_vec();
    sorted.sort_unstable();
    let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q).round() as usize] as f32;
    let (lo, med, hi) = (at(0.02), at(0.5), at(0.98));
    hi - med > 2.0 * (med - lo) + 8.0
}

impl BoxReader for IndraRec {
    fn read(&self, crops: &[(&RgbaImage, bool)]) -> Result<Vec<Vec<Glyph>>> {
        // A column is read lying down, its top at the left (Indra reads lines only).
        let lying: Vec<Option<RgbaImage>> = crops.iter().map(|(c, down)| down.then(|| image::imageops::rotate270(*c))).collect();
        let prepared: Vec<Prepared> = crops.iter().zip(&lying).map(|((c, _), l)| self.prepare(l.as_ref().unwrap_or(c))).collect();
        let mut order: Vec<usize> = (0..prepared.len()).collect();
        order.sort_by_key(|&i| prepared[i].ink.width());
        let mut found: Vec<Vec<Glyph>> = crops.iter().map(|_| Vec::new()).collect();
        for run in order.chunks(BATCH) {
            let batch: Vec<&Prepared> = run.iter().map(|&i| &prepared[i]).collect();
            for (&i, glyphs) in run.iter().zip(self.run(&batch)?) {
                found[i] = glyphs;
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_language_reads_with_its_group() {
        assert_eq!(Group::of("ja-JP"), Group::Cjk);
        assert_eq!(Group::of("zh-TW"), Group::Cjk);
        assert_eq!(Group::of("ko-KR"), Group::Cjk);
        assert_eq!(Group::of("ar-SA"), Group::Rtl);
        assert_eq!(Group::of("he-IL"), Group::Rtl);
        assert_eq!(Group::of("ru-RU"), Group::Base);
        assert_eq!(Group::of("el-GR"), Group::Base);
        assert_eq!(Group::of("ro-RO"), Group::Base, "Romanian reads with rec_small but is Latin");
        assert_eq!(Group::of("pt-BR"), Group::Base);
        assert_eq!((Group::Base.window(), Group::Cjk.window()), (3, 1));
    }

    /// PP-OCR's det_small boxes read by IndraOCR: the Russian fixture with the base group (its
    /// three-line window), the Japanese one (dark on white and light on dark blue) with the CJK
    /// group. Needs the fetched ONNX Runtime and models, and Indra's in `LOOKITAN_INDRA` (CI
    /// downloads them; the test is skipped without them).
    #[test]
    #[ignore]
    fn real_indra_reads_the_fixtures_on_paddle_s_boxes() {
        use crate::ocr::paddle::{recognize_boxes_with, EnginePlan, PaddleOcr};
        use unicode_normalization::UnicodeNormalization;
        let Some(dir) = std::env::var_os("LOOKITAN_INDRA").map(std::path::PathBuf::from) else {
            eprintln!("LOOKITAN_INDRA is not set; skipped");
            return;
        };
        let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let library = resources.join("ort").join(if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" });
        ort::init_from(&library).expect("ONNX Runtime").with_name("lookitan-test").commit();
        let plan = EnginePlan { manual: true, det: "det_small", det_gpu: false, rec_gpu: false, gpu_choice: None };
        let paddle = PaddleOcr::new(&resources.join("models"), "rec_small", plan).unwrap();
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
        let fixtures: [(&[u8], Group, &str, f32); 2] = [
            // Indra reads ё as е: four of them.
            (include_bytes!("fixtures/ru_lines.png"), Group::Base, "Съешь же ещё этих мягких французских булок ВСЁ ЕЩЁ ЗДЕСЬ, а ёлка — в лесу. Шелестят листья!", 0.10),
            (include_bytes!("fixtures/ja_lines.png"), Group::Cjk, &japanese, 0.05),
        ];
        for (png, group, want, limit) in fixtures {
            let indra = IndraRec::new(&dir, group, false, None).unwrap();
            let img = image::load_from_memory(png).unwrap().to_rgba8();
            let boxes = paddle.detect_boxes(&img).unwrap();
            let started = std::time::Instant::now();
            let lines = recognize_boxes_with(&img, &boxes, &indra);
            let read: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
            let e = cer(&read.concat(), want);
            eprintln!("IndraOCR {group:?} on {} boxes in {} ms: CER {:.1}%\n{}", boxes.len(), started.elapsed().as_millis(), e * 100.0, read.join("\n"));
            assert!(e <= limit, "{group:?} CER {e}");
            for l in &lines {
                assert!(l.chars.iter().all(|c| c.x0 >= l.x0 - 1.0 && c.x1 <= l.x1 + 1.0), "{}", l.text);
            }
        }
    }

    /// LineNet's lines read by IndraOCR, on the same fixtures and with the same limits as PP-OCR's
    /// boxes above (measured with indra_ocr's code: CER 4.0% Russian, its ё read as е; 0.0%
    /// Japanese, 3 and 10 lines). On Windows the detector is also asked for the GPU (DirectML,
    /// which falls back to the CPU on the runner): its lines must be the same.
    #[test]
    #[ignore]
    fn real_indra_linenet_finds_the_fixtures_lines() {
        use crate::ocr::paddle::recognize_boxes_with;
        use unicode_normalization::UnicodeNormalization;
        let Some(dir) = std::env::var_os("LOOKITAN_INDRA").map(std::path::PathBuf::from) else {
            eprintln!("LOOKITAN_INDRA is not set; skipped");
            return;
        };
        let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let library = resources.join("ort").join(if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" });
        ort::init_from(&library).expect("ONNX Runtime").with_name("lookitan-test").commit();
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
        let fixtures: [(&[u8], Group, &str, usize, f32); 2] = [
            (include_bytes!("fixtures/ru_lines.png"), Group::Base, "Съешь же ещё этих мягких французских булок ВСЁ ЕЩЁ ЗДЕСЬ, а ёлка — в лесу. Шелестят листья!", 3, 0.10),
            (include_bytes!("fixtures/ja_lines.png"), Group::Cjk, &japanese, 10, 0.05),
        ];
        let det = IndraDet::new(&dir, false, None).unwrap();
        let gpu = cfg!(windows).then(|| IndraDet::new(&dir, true, None).unwrap());
        for (png, group, want, count, limit) in fixtures {
            let indra = IndraRec::new(&dir, group, false, None).unwrap();
            let img = image::load_from_memory(png).unwrap().to_rgba8();
            let started = std::time::Instant::now();
            let boxes = det.detect_boxes(&img).unwrap();
            let det_ms = started.elapsed().as_millis();
            let lines = recognize_boxes_with(&img, &boxes, &indra);
            let read: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
            let e = cer(&read.concat(), want);
            eprintln!("LineNet {group:?}: {} lines in {det_ms} ms, CER {:.1}%\n{}", boxes.len(), e * 100.0, read.join("\n"));
            assert_eq!(boxes.len(), count, "{group:?}");
            assert!(e <= limit, "{group:?} CER {e}");
            if let Some(gpu) = &gpu {
                let on = gpu.detect_boxes(&img).unwrap();
                eprintln!("LineNet asked for the GPU: on {}", if gpu.on_gpu() { "GPU" } else { "CPU" });
                assert_eq!(on.len(), boxes.len());
                for (a, b) in on.iter().zip(&boxes) {
                    assert!((a.x0 - b.x0).abs() < 1.0 && (a.y0 - b.y0).abs() < 1.0 && (a.x1 - b.x1).abs() < 1.0 && (a.y1 - b.y1).abs() < 1.0, "{a:?} {b:?}");
                }
            }
        }
    }

    #[test]
    fn linenet_s_cores_become_lines_grown_at_the_ends() {
        // 4 × 6 output: a core on rows 1 and 2, columns 1 to 4, its ink 3 above and 5 below each
        // pixel's centre; a lone core pixel at (5, 3) is not a line.
        let (h, w) = (4, 6);
        let mut out = vec![-1f32; 3 * h * w];
        for y in 1..=2 {
            for x in 1..=4 {
                out[y * w + x] = 2.0;
            }
        }
        out[3 * w + 5] = 2.0;
        for i in 0..h * w {
            out[h * w + i] = 3.0;
            out[2 * h * w + i] = 5.0;
        }
        let lines = decode_lines(&out, h, w);
        assert_eq!(lines.len(), 1);
        // Centres at 3 and 5: top (0 + 2) / 2, bottom (8 + 10) / 2; 8 tall, grown 1.2 at each end.
        let [x0, y0, x1, y1] = lines[0];
        assert!((x0 - 0.8).abs() < 1e-4 && (y0 - 1.0).abs() < 1e-4 && (x1 - 11.2).abs() < 1e-4 && (y1 - 9.0).abs() < 1e-4, "{:?}", lines[0]);
        assert_eq!(median(vec![3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), 2.5);
    }

    #[test]
    fn a_light_line_on_a_dark_ground_is_dark() {
        let mut dark = vec![20u8; 900];
        dark.extend(vec![240u8; 100]);
        assert!(is_dark(&dark));
        let mut light = vec![250u8; 900];
        light.extend(vec![10u8; 100]);
        assert!(!is_dark(&light));
    }
}
