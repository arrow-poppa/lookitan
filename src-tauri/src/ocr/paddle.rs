// PP-OCRv6 / PP-OCRv5 text detection (DB) and recognition (CTC) on ONNX Runtime.
//
// Engine choice mirrors Lookupper: try the `small` detector on the GPU (DirectML) first and fall
// back to the `tiny` detector on the CPU with full graph optimization. Parameters (normalization,
// thresholds, unclip ratio) come from the official PaddlePaddle `inference.yml` of each model.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use image::imageops::FilterType;
use image::{Rgba, RgbaImage};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use parking_lot::Mutex;

use super::{OcrChar, OcrLine};

macro_rules! ortx {
    ($e:expr) => {
        $e.map_err(|e| anyhow!("{e}"))?
    };
}

const DET_THRESH: f32 = 0.2;
const UNCLIP_RATIO: f32 = 1.4;
/// Full-HD and QHD screens are detected at (almost) native size so small UI text survives.
const DET_MAX_SIDE: f32 = 2048.0;
const REC_HEIGHT: u32 = 48;

#[derive(Clone, Copy, Debug)]
pub struct TextBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Splits detected boxes into the line under the pointer with its neighbours (recognized first,
/// so the pop-up appears at once) and every other box (recognized afterwards for the overlay).
pub fn split_focus(boxes: &[TextBox], fx: f32, fy: f32) -> (Vec<TextBox>, Vec<TextBox>) {
    let Some(target) = boxes
        .iter()
        .copied()
        .filter(|b| b.distance_to(fx, fy) <= 48.0)
        .min_by(|a, b| a.distance_to(fx, fy).total_cmp(&b.distance_to(fx, fy)))
    else {
        return (Vec::new(), boxes.to_vec());
    };
    let h = target.height().max(4.0);
    let near = |b: &TextBox| {
        let overlap = b.x1.min(target.x1) - b.x0.max(target.x0);
        let gap = if b.y0 > target.y1 { b.y0 - target.y1 } else if b.y1 < target.y0 { target.y0 - b.y1 } else { 0.0 };
        overlap > 0.0 && gap <= 1.6 * h
    };
    let (mut focus, rest): (Vec<TextBox>, Vec<TextBox>) = boxes.iter().copied().partition(|b| near(b));
    focus.sort_by(|a, b| a.distance_to(fx, fy).total_cmp(&b.distance_to(fx, fy)));
    let mut rest = rest;
    if focus.len() > 7 {
        rest.extend(focus.split_off(7));
    }
    (focus, rest)
}

impl TextBox {
    fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn distance_to(&self, x: f32, y: f32) -> f32 {
        let dx = if x < self.x0 { self.x0 - x } else if x > self.x1 { x - self.x1 } else { 0.0 };
        let dy = if y < self.y0 { self.y0 - y } else if y > self.y1 { y - self.y1 } else { 0.0 };
        (dx * dx + dy * dy).sqrt()
    }
}

pub struct PaddleOcr {
    det: Mutex<Session>,
    rec: Mutex<Session>,
    dict: Vec<String>,
    box_thresh: f32,
    pub det_model: &'static str,
    pub det_on_gpu: bool,
    pub rec_model: String,
    pub rec_on_gpu: bool,
}

fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 4)
}

fn cpu_session(path: &Path) -> Result<Session> {
    let b = ortx!(Session::builder());
    let b = ortx!(b.with_optimization_level(GraphOptimizationLevel::Level3));
    let mut b = ortx!(b.with_intra_threads(threads()));
    let s = ortx!(b.commit_from_file(path));
    Ok(s)
}

#[cfg(windows)]
fn gpu_session(path: &Path) -> Result<Session> {
    use ort::ep::DirectML;
    let b = ortx!(Session::builder());
    let b = ortx!(b.with_optimization_level(GraphOptimizationLevel::Level1));
    let b = ortx!(b.with_memory_pattern(false));
    let mut b = ortx!(b.with_execution_providers([DirectML::default().with_device_id(0).build().error_on_failure()]));
    let s = ortx!(b.commit_from_file(path));
    Ok(s)
}

#[cfg(not(windows))]
fn gpu_session(_path: &Path) -> Result<Session> {
    bail!("GPU inference is only available on Windows (DirectML)")
}

impl PaddleOcr {
    pub fn new(models_dir: &Path, rec_stem: &str, want_gpu: bool) -> Result<Self> {
        let det_small = models_dir.join("det_small.onnx");
        let det_tiny = models_dir.join("det_tiny.onnx");
        let rec_path = models_dir.join(format!("{rec_stem}.onnx"));
        let dict_path = models_dir.join(format!("{rec_stem}.dict.json"));
        if !rec_path.exists() {
            bail!("recognition model not found: {}", rec_path.display());
        }

        let (det, det_model, det_on_gpu) = if want_gpu {
            match gpu_session(&det_small) {
                Ok(s) => (s, "det_small", true),
                Err(e) => {
                    log::warn!("Ocr.GpuFallback.Detection: {e:#}");
                    (cpu_session(&det_tiny)?, "det_tiny", false)
                }
            }
        } else {
            (cpu_session(&det_tiny)?, "det_tiny", false)
        };
        let (rec, rec_on_gpu) = if det_on_gpu {
            match gpu_session(&rec_path) {
                Ok(s) => (s, true),
                Err(e) => {
                    log::warn!("Ocr.GpuFallback.Recognition: {e:#}");
                    (cpu_session(&rec_path)?, false)
                }
            }
        } else {
            (cpu_session(&rec_path)?, false)
        };
        let dict: Vec<String> = serde_json::from_slice(
            &std::fs::read(&dict_path).with_context(|| format!("read {}", dict_path.display()))?,
        )?;
        log::info!(
            "OCR engine ready: {det_model} on {}, {rec_stem} on {}, {} characters",
            if det_on_gpu { "GPU" } else { "CPU" },
            if rec_on_gpu { "GPU" } else { "CPU" },
            dict.len()
        );
        Ok(Self {
            det: Mutex::new(det),
            rec: Mutex::new(rec),
            dict,
            box_thresh: if det_on_gpu { 0.45 } else { 0.4 },
            det_model,
            det_on_gpu,
            rec_model: rec_stem.to_string(),
            rec_on_gpu,
        })
    }

    /// Runs both models once so the first real lookup does not pay for graph setup.
    pub fn warm_up(&self) {
        let blank = RgbaImage::from_pixel(96, 64, Rgba([255, 255, 255, 255]));
        if let Err(e) = self.detect(&blank) {
            log::warn!("detector warm-up failed: {e:#}");
        }
        let b = TextBox { x0: 0.0, y0: 0.0, x1: 96.0, y1: 32.0 };
        if let Err(e) = self.recognize_box(&blank, b) {
            log::warn!("recognizer warm-up failed: {e:#}");
        }
    }

    /// Detects every text box, top to bottom.
    pub fn detect_boxes(&self, img: &RgbaImage) -> Result<Vec<TextBox>> {
        let mut boxes = self.detect(img)?;
        boxes.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
        Ok(boxes)
    }

    /// Recognizes the given boxes; boxes without readable text are dropped.
    pub fn recognize_boxes(&self, img: &RgbaImage, boxes: &[TextBox]) -> Vec<OcrLine> {
        let mut lines = Vec::with_capacity(boxes.len());
        for b in boxes {
            match self.recognize_box(img, *b) {
                Ok(Some(line)) => lines.push(line),
                Ok(None) => {}
                Err(e) => log::warn!("recognition failed: {e:#}"),
            }
        }
        lines.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
        lines
    }

    fn detect(&self, img: &RgbaImage) -> Result<Vec<TextBox>> {
        let (w, h) = img.dimensions();
        if w < 8 || h < 8 {
            return Ok(Vec::new());
        }
        let longest = w.max(h) as f32;
        let scale = if longest > DET_MAX_SIDE { DET_MAX_SIDE / longest } else { 1.0 };
        let round32 = |v: f32| -> u32 { (((v / 32.0).round() as u32) * 32).max(32) };
        let rw = round32(w as f32 * scale);
        let rh = round32(h as f32 * scale);
        let resized = image::imageops::resize(img, rw, rh, FilterType::Triangle);

        let plane = (rw * rh) as usize;
        let mut data = vec![0f32; plane * 3];
        // BGR channel order with the ImageNet mean/std listed in the model's inference.yml.
        for (i, p) in resized.pixels().enumerate() {
            let [r, g, b, _] = p.0;
            data[i] = (b as f32 / 255.0 - 0.485) / 0.229;
            data[plane + i] = (g as f32 / 255.0 - 0.456) / 0.224;
            data[2 * plane + i] = (r as f32 / 255.0 - 0.406) / 0.225;
        }
        let input = ortx!(Tensor::from_array(([1usize, 3, rh as usize, rw as usize], data)));

        let (mh, mw, prob) = {
            let mut det = self.det.lock();
            let outputs = ortx!(det.run(ort::inputs![input]));
            let (shape, prob) = ortx!(outputs[0].try_extract_tensor::<f32>());
            if shape.len() != 4 {
                bail!("unexpected detector output shape {:?}", &shape[..]);
            }
            (shape[2] as usize, shape[3] as usize, prob.to_vec())
        };

        let sx = w as f32 / mw as f32;
        let sy = h as f32 / mh as f32;
        let mut visited = vec![false; mh * mw];
        let mut boxes = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        for start in 0..mh * mw {
            if visited[start] || prob[start] <= DET_THRESH {
                continue;
            }
            visited[start] = true;
            stack.push(start);
            let (mut minx, mut miny, mut maxx, mut maxy) = (usize::MAX, usize::MAX, 0usize, 0usize);
            let mut sum = 0f32;
            let mut count = 0usize;
            while let Some(i) = stack.pop() {
                let (x, y) = (i % mw, i / mw);
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
                sum += prob[i];
                count += 1;
                let mut visit = |j: usize, stack: &mut Vec<usize>| {
                    if !visited[j] && prob[j] > DET_THRESH {
                        visited[j] = true;
                        stack.push(j);
                    }
                };
                if x > 0 {
                    visit(i - 1, &mut stack);
                }
                if x + 1 < mw {
                    visit(i + 1, &mut stack);
                }
                if y > 0 {
                    visit(i - mw, &mut stack);
                }
                if y + 1 < mh {
                    visit(i + mw, &mut stack);
                }
            }
            let bw = (maxx - minx + 1) as f32;
            let bh = (maxy - miny + 1) as f32;
            if bw < 3.0 || bh < 3.0 || count == 0 || sum / (count as f32) < self.box_thresh {
                continue;
            }
            // DB predicts shrunk text kernels; grow them back by area * ratio / perimeter.
            let d = bw * bh * UNCLIP_RATIO / (2.0 * (bw + bh));
            let x0 = ((minx as f32 - d) * sx).max(0.0);
            let y0 = ((miny as f32 - d) * sy).max(0.0);
            let x1 = (((maxx + 1) as f32 + d) * sx).min(w as f32);
            let y1 = (((maxy + 1) as f32 + d) * sy).min(h as f32);
            if x1 - x0 >= 4.0 && y1 - y0 >= 4.0 {
                boxes.push(TextBox { x0, y0, x1, y1 });
            }
        }
        Ok(boxes)
    }

    fn recognize_box(&self, img: &RgbaImage, b: TextBox) -> Result<Option<OcrLine>> {
        let (iw, ih) = img.dimensions();
        let x = (b.x0.floor().max(0.0) as u32).min(iw.saturating_sub(1));
        let y = (b.y0.floor().max(0.0) as u32).min(ih.saturating_sub(1));
        let cw = ((b.x1.ceil() as u32).min(iw)).saturating_sub(x);
        let ch = ((b.y1.ceil() as u32).min(ih)).saturating_sub(y);
        if cw < 2 || ch < 2 {
            return Ok(None);
        }
        let crop = image::imageops::crop_imm(img, x, y, cw, ch).to_image();
        let tw = ((cw as f32 * REC_HEIGHT as f32 / ch as f32).round() as u32).clamp(16, 3200);
        let resized = image::imageops::resize(&crop, tw, REC_HEIGHT, FilterType::Triangle);
        let plane = (tw * REC_HEIGHT) as usize;
        let mut data = vec![0f32; plane * 3];
        for (i, p) in resized.pixels().enumerate() {
            let [r, g, bl, _] = p.0;
            data[i] = (bl as f32 / 255.0 - 0.5) / 0.5;
            data[plane + i] = (g as f32 / 255.0 - 0.5) / 0.5;
            data[2 * plane + i] = (r as f32 / 255.0 - 0.5) / 0.5;
        }
        let input = ortx!(Tensor::from_array(([1usize, 3, REC_HEIGHT as usize, tw as usize], data)));

        let (steps, classes, probs) = {
            let mut rec = self.rec.lock();
            let outputs = ortx!(rec.run(ort::inputs![input]));
            let (shape, probs) = ortx!(outputs[0].try_extract_tensor::<f32>());
            if shape.len() != 3 {
                bail!("unexpected recognizer output shape {:?}", &shape[..]);
            }
            (shape[1] as usize, shape[2] as usize, probs.to_vec())
        };
        if steps == 0 || classes == 0 {
            return Ok(None);
        }
        // Index 0 is the CTC blank; a trailing extra class is the space character.
        let has_space = classes == self.dict.len() + 2;

        let mut decoded: Vec<(char, usize)> = Vec::new();
        let mut prev = 0usize;
        for t in 0..steps {
            let row = &probs[t * classes..(t + 1) * classes];
            let mut best = 0usize;
            let mut best_p = f32::MIN;
            for (k, &p) in row.iter().enumerate() {
                if p > best_p {
                    best_p = p;
                    best = k;
                }
            }
            if best != 0 && best != prev {
                let s: &str = if best - 1 < self.dict.len() {
                    &self.dict[best - 1]
                } else if has_space {
                    " "
                } else {
                    ""
                };
                for c in s.chars() {
                    decoded.push((c, t));
                }
            }
            prev = best;
        }
        if decoded.iter().all(|(c, _)| c.is_whitespace()) {
            return Ok(None);
        }

        // Each CTC step covers an equal slice of the crop width.
        let step_w = cw as f32 / steps as f32;
        let centers: Vec<f32> = decoded.iter().map(|(_, t)| x as f32 + (*t as f32 + 0.5) * step_w).collect();
        let mut chars = Vec::with_capacity(decoded.len());
        for (i, (c, _)) in decoded.iter().enumerate() {
            let left = if i == 0 { b.x0.max(centers[0] - step_w * 2.0) } else { (centers[i - 1] + centers[i]) / 2.0 };
            let right = if i + 1 == decoded.len() {
                b.x1.min(centers[i] + step_w * 2.0)
            } else {
                (centers[i] + centers[i + 1]) / 2.0
            };
            chars.push(OcrChar { ch: *c, x0: left, x1: right });
        }
        let text: String = decoded.iter().map(|(c, _)| *c).collect();
        Ok(Some(OcrLine { text, x0: b.x0, y0: b.y0, x1: b.x1, y1: b.y1, chars }))
    }
}
