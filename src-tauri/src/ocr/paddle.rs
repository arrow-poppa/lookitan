// PP-OCRv6 / PP-OCRv5 text detection (DB) and recognition (CTC) on ONNX Runtime.
//
// Engine choice mirrors Lookupper: try the `small` detector on the GPU (DirectML on Windows, WebGPU
// on Linux) first and fall back to the `tiny` detector on the CPU with full graph optimization.
// Parameters (normalization, thresholds, unclip ratio) come from the official PaddlePaddle
// `inference.yml` of each model.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use image::imageops::FilterType;
use image::{Rgba, RgbaImage};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use parking_lot::Mutex;

use super::{ink, OcrChar, OcrLine};
use crate::settings::GpuPreference;

macro_rules! ortx {
    ($e:expr) => {
        $e.map_err(|e| anyhow!("{e}"))?
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeSession {
        gpu: bool,
        released: Arc<AtomicBool>,
    }

    impl Drop for FakeSession {
        fn drop(&mut self) {
            if self.gpu {
                self.released.store(true, Ordering::Relaxed);
            }
        }
    }

    fn fake_session(gpu: bool) -> InferenceSession<FakeSession> {
        InferenceSession {
            session: Some(FakeSession {
                gpu,
                released: Arc::new(AtomicBool::new(false)),
            }),
            on_gpu: Arc::new(AtomicBool::new(gpu)),
            cpu_path: PathBuf::from("det_tiny.onnx"),
            stage: "Detection",
        }
    }

    #[test]
    fn gpu_failure_releases_session_then_retries_and_keeps_cpu() {
        let mut session = fake_session(true);
        let released = session.session.as_ref().unwrap().released.clone();
        let output = session
            .run(
                |path| {
                    assert_eq!(path, Path::new("det_tiny.onnx"));
                    assert!(released.load(Ordering::Relaxed));
                    Ok(FakeSession {
                        gpu: false,
                        released: released.clone(),
                    })
                },
                |s| {
                    if s.gpu {
                        bail!("Add.176: 8007000E");
                    }
                    Ok(42)
                },
            )
            .unwrap();
        assert_eq!(output, 42);
        assert!(!session.on_gpu.load(Ordering::Relaxed));
        assert_eq!(
            session
                .run(|_| panic!("CPU session must be reused"), |_| Ok(7))
                .unwrap(),
            7
        );
    }

    #[test]
    fn successful_gpu_inference_does_not_load_cpu() {
        let mut session = fake_session(true);
        assert_eq!(session.run(|_| panic!("CPU must not load"), |_| Ok(42)).unwrap(), 42);
        assert!(session.on_gpu.load(Ordering::Relaxed));
    }

    #[test]
    fn cpu_error_is_returned_without_recreating_session() {
        let mut session = fake_session(false);
        let error = session
            .run(
                |_| panic!("CPU must not reload"),
                |_| -> Result<()> { bail!("CPU error") },
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "CPU error");
    }

    #[test]
    fn failed_cpu_load_can_be_retried_on_the_next_input() {
        let mut session = fake_session(true);
        let error = session
            .run(|_| bail!("missing CPU model"), |_| -> Result<()> { bail!("GPU error") })
            .unwrap_err();
        assert!(format!("{error:#}").contains("missing CPU model"));
        assert!(session.session.is_none());
        assert!(!session.on_gpu.load(Ordering::Relaxed));
        session
            .run(
                |_| {
                    Ok(FakeSession {
                        gpu: false,
                        released: Arc::new(AtomicBool::new(false)),
                    })
                },
                |_| Ok(()),
            )
            .unwrap();
    }

    #[test]
    fn batches_group_similar_widths_within_limits() {
        let line = |w: f32| TextBox { x0: 0.0, y0: 0.0, x1: w, y1: 48.0 };
        let mut boxes: Vec<TextBox> = (0..40).map(|i| line(100.0 + (i % 5) as f32 * 200.0)).collect();
        boxes.push(line(3000.0));
        let batches = plan_batches(&boxes);
        assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), boxes.len());
        let mut last_widest = 0;
        for batch in &batches {
            let widths: Vec<u32> = batch.iter().map(box_rec_width).collect();
            let widest = *widths.iter().max().unwrap();
            assert!(batch.len() <= REC_BATCH_MAX);
            assert!(batch.len() == 1 || batch.len() as u32 * widest <= REC_BATCH_COLUMNS);
            // Narrowest first, so padding inside a batch stays small.
            assert!(widths.iter().min().unwrap() >= &last_widest);
            last_widest = widest;
        }
        assert_eq!(batches.last().unwrap().len(), 1, "the 3000 px line runs alone");
    }

    #[test]
    fn padded_input_keeps_the_crop_left_aligned_and_pads_with_zero() {
        let crop = RgbaImage::from_pixel(2, REC_HEIGHT, Rgba([255, 255, 255, 255]));
        let pw = 5;
        let mut slot = vec![0f32; 3 * REC_HEIGHT as usize * pw];
        fill_rec_input(&mut slot, &crop, pw);
        let plane = REC_HEIGHT as usize * pw;
        for c in 0..3 {
            for y in 0..REC_HEIGHT as usize {
                let row = &slot[c * plane + y * pw..c * plane + (y + 1) * pw];
                assert_eq!(row, &[1.0, 1.0, 0.0, 0.0, 0.0]);
            }
        }
    }

    #[test]
    fn overlapping_bands_merge_seam_text_but_preserve_separate_lines() {
        let mut boxes = vec![TextBox {
            x0: 20.0,
            x1: 220.0,
            y0: 480.0,
            y1: 512.0,
        }];
        merge_band_boxes(
            &mut boxes,
            vec![
                TextBox {
                    x0: 18.0,
                    x1: 222.0,
                    y0: 480.0,
                    y1: 524.0,
                },
                TextBox {
                    x0: 260.0,
                    x1: 460.0,
                    y0: 480.0,
                    y1: 524.0,
                },
                TextBox {
                    x0: 20.0,
                    x1: 220.0,
                    y0: 550.0,
                    y1: 590.0,
                },
            ],
        );
        assert_eq!(boxes.len(), 3);
        assert_eq!(
            (boxes[0].x0, boxes[0].x1, boxes[0].y0, boxes[0].y1),
            (18.0, 222.0, 480.0, 524.0)
        );
    }
}

const DET_THRESH: f32 = 0.2;
const UNCLIP_RATIO: f32 = 1.4;
/// Same detector limit as the original working lookup.
const DET_MAX_SIDE: f32 = 1280.0;
// Bound intermediate tensors without shrinking small UI text further. Adjacent bands overlap
// so text crossing a seam can still be detected as a complete line.
const DET_BAND_HEIGHT: u32 = 512;
const DET_BAND_OVERLAP: u32 = 64;
const REC_HEIGHT: u32 = 48;
/// Most lines per batched recognizer run (whole-screen OCR only). PaddleOCR's `rec_batch_num`
/// is 6; one GPU run costs about the same for a few more lines.
const REC_BATCH_MAX: usize = 16;
/// Upper bound on lines times padded width in one batch, which bounds GPU memory per run.
const REC_BATCH_COLUMNS: u32 = 12_000;

/// Width of a crop once resized to the recognizer's input height.
fn rec_width(cw: u32, ch: u32) -> u32 {
    ((cw as f32 * REC_HEIGHT as f32 / ch as f32).round() as u32).clamp(16, 3200)
}

fn box_rec_width(b: &TextBox) -> u32 {
    rec_width((b.x1 - b.x0).max(1.0) as u32, (b.y1 - b.y0).max(1.0) as u32)
}

/// Groups boxes of similar width for batched recognition, narrowest first, so little of each
/// batch is padding.
pub fn plan_batches(boxes: &[TextBox]) -> Vec<Vec<TextBox>> {
    let mut sorted = boxes.to_vec();
    sorted.sort_by_key(box_rec_width);
    let mut out = Vec::new();
    let mut batch: Vec<TextBox> = Vec::new();
    for b in sorted {
        // Sorted ascending, so this box sets the batch's padded width.
        let widest = box_rec_width(&b);
        if !batch.is_empty() && (batch.len() >= REC_BATCH_MAX || (batch.len() as u32 + 1) * widest > REC_BATCH_COLUMNS) {
            out.push(std::mem::take(&mut batch));
        }
        batch.push(b);
    }
    if !batch.is_empty() {
        out.push(batch);
    }
    out
}

/// A text box cut out of the capture and resized to the recognizer's height.
struct RecCrop {
    b: TextBox,
    /// Left edge, top and width of the crop in capture pixels.
    x: u32,
    y: u32,
    cw: u32,
    /// Width after resizing.
    tw: u32,
    image: RgbaImage,
    /// The ink of the crop at capture resolution, to snap the word edges to.
    ink: Option<ink::Ink>,
}

fn rec_crop(img: &RgbaImage, b: TextBox) -> Option<RecCrop> {
    let (iw, ih) = img.dimensions();
    let x = (b.x0.floor().max(0.0) as u32).min(iw.saturating_sub(1));
    let y = (b.y0.floor().max(0.0) as u32).min(ih.saturating_sub(1));
    let cw = ((b.x1.ceil() as u32).min(iw)).saturating_sub(x);
    let ch = ((b.y1.ceil() as u32).min(ih)).saturating_sub(y);
    if cw < 2 || ch < 2 {
        return None;
    }
    let crop = image::imageops::crop_imm(img, x, y, cw, ch).to_image();
    let lum: Vec<u8> = crop.pixels().map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8).collect();
    let ink = ink::measure(&lum, cw as usize, ch as usize);
    let tw = rec_width(cw, ch);
    let image = image::imageops::resize(&crop, tw, REC_HEIGHT, FilterType::Triangle);
    Some(RecCrop { b, x, y, cw, tw, image, ink })
}

/// Writes one crop into its slot of a recognizer input (3 planes of REC_HEIGHT x `padded_w`,
/// BGR). Columns past the crop stay 0, the padding PaddleOCR uses after normalization.
fn fill_rec_input(slot: &mut [f32], crop: &RgbaImage, padded_w: usize) {
    let plane = REC_HEIGHT as usize * padded_w;
    for (x, y, p) in crop.enumerate_pixels() {
        let i = y as usize * padded_w + x as usize;
        let [r, g, bl, _] = p.0;
        slot[i] = (bl as f32 / 255.0 - 0.5) / 0.5;
        slot[plane + i] = (g as f32 / 255.0 - 0.5) / 0.5;
        slot[2 * plane + i] = (r as f32 / 255.0 - 0.5) / 0.5;
    }
}

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
        let gap = if b.y0 > target.y1 {
            b.y0 - target.y1
        } else if b.y1 < target.y0 {
            target.y0 - b.y1
        } else {
            0.0
        };
        overlap > 0.0 && gap <= 1.6 * h
    };
    let mut focus: Vec<TextBox> = boxes.iter().copied().filter(near).collect();
    // Match the original lookup's context selection, always keeping the pointed-at line.
    focus.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    focus.truncate(6);
    let same_box = |a: &TextBox, b: &TextBox| a.x0 == b.x0 && a.y0 == b.y0 && a.x1 == b.x1 && a.y1 == b.y1;
    if !focus.iter().any(|b| same_box(b, &target)) {
        focus.push(target);
    }
    let rest = boxes
        .iter()
        .copied()
        .filter(|b| !focus.iter().any(|f| same_box(f, b)))
        .collect();
    (focus, rest)
}

impl TextBox {
    fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn distance_to(&self, x: f32, y: f32) -> f32 {
        let dx = if x < self.x0 {
            self.x0 - x
        } else if x > self.x1 {
            x - self.x1
        } else {
            0.0
        };
        let dy = if y < self.y0 {
            self.y0 - y
        } else if y > self.y1 {
            y - self.y1
        } else {
            0.0
        };
        (dx * dx + dy * dy).sqrt()
    }
}

fn merge_band_boxes(boxes: &mut Vec<TextBox>, band: Vec<TextBox>) {
    let previous = boxes.len();
    for b in band {
        let duplicate = boxes[..previous].iter_mut().find(|a| {
            let overlap_x = a.x1.min(b.x1) - a.x0.max(b.x0);
            let overlap_y = a.y1.min(b.y1) - a.y0.max(b.y0);
            overlap_x > 0.5 * (a.x1 - a.x0).min(b.x1 - b.x0) && overlap_y > 0.5 * a.height().min(b.height())
        });
        if let Some(a) = duplicate {
            a.x0 = a.x0.min(b.x0);
            a.y0 = a.y0.min(b.y0);
            a.x1 = a.x1.max(b.x1);
            a.y1 = a.y1.max(b.y1);
        } else {
            boxes.push(b);
        }
    }
}

pub struct PaddleOcr {
    det: Mutex<InferenceSession>,
    rec: Mutex<InferenceSession>,
    dict: Vec<String>,
    det_on_gpu: Arc<AtomicBool>,
    pub rec_model: String,
    rec_on_gpu: Arc<AtomicBool>,
}

/// A successful GPU session load does not guarantee that a later, larger input will fit.
/// Keep recovery inside the session lock so foreground and overlay OCR share the CPU replacement.
struct InferenceSession<S = Session> {
    session: Option<S>,
    on_gpu: Arc<AtomicBool>,
    cpu_path: PathBuf,
    stage: &'static str,
}

impl<S> InferenceSession<S> {
    fn run<T>(
        &mut self,
        create_cpu: impl FnOnce(&Path) -> Result<S>,
        mut infer: impl FnMut(&mut S) -> Result<T>,
    ) -> Result<T> {
        let mut create_cpu = Some(create_cpu);
        if self.session.is_none() {
            self.session = Some(create_cpu.take().unwrap()(&self.cpu_path)?);
        }
        match infer(self.session.as_mut().unwrap()) {
            Ok(output) => Ok(output),
            Err(gpu_error) if self.on_gpu.load(Ordering::Relaxed) => {
                log::warn!("Ocr.GpuFallback.{} during inference: {gpu_error:#}", self.stage);
                // Release the failed GPU graph and its allocations before loading the CPU model.
                drop(self.session.take());
                self.on_gpu.store(false, Ordering::Relaxed);
                self.session = Some(
                    create_cpu.take().unwrap()(&self.cpu_path)
                        .with_context(|| format!("{} CPU recovery after GPU failure ({gpu_error:#})", self.stage))?,
                );
                infer(self.session.as_mut().unwrap())
                    .with_context(|| format!("{} CPU retry after GPU failure ({gpu_error:#})", self.stage))
            }
            Err(error) => Err(error),
        }
    }
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 4)
}

fn cpu_session(path: &Path) -> Result<Session> {
    let b = ortx!(Session::builder());
    let b = ortx!(b.with_optimization_level(GraphOptimizationLevel::Level3));
    let mut b = ortx!(b.with_intra_threads(threads()));
    let s = ortx!(b.commit_from_file(path));
    Ok(s)
}

// `choice` is the card the user picked (Configurações › OCR); `None` keeps the API's default.
#[cfg(windows)]
fn gpu_session(path: &Path, choice: Option<GpuPreference>) -> Result<Session> {
    use ort::ep::directml::PerformancePreference;
    use ort::ep::DirectML;
    let b = ortx!(Session::builder());
    let b = ortx!(b.with_optimization_level(GraphOptimizationLevel::Level1));
    let b = ortx!(b.with_memory_pattern(false));
    let b = ortx!(b.with_parallel_execution(false));
    // A device id overrides the preference, so it is set only when the user chose nothing.
    let ep = match choice {
        None => DirectML::default().with_device_id(0),
        Some(GpuPreference::HighPerformance) => DirectML::default().with_performance_preference(PerformancePreference::HighPerformance),
        Some(GpuPreference::LowPower) => DirectML::default().with_performance_preference(PerformancePreference::MinimumPower),
    };
    let mut b = ortx!(b.with_execution_providers([ep.build().error_on_failure()]));
    let s = ortx!(b.commit_from_file(path));
    Ok(s)
}

// Linux has no DirectML; the bundled ONNX Runtime build carries the WebGPU provider, which runs on
// Vulkan through Dawn (Mesa RADV, ANV, NVIDIA).
// The WebGPU `deviceId` is ONNX Runtime's context id, not an adapter: the card is picked by Dawn
// from the power preference (high performance when unset). The context is shared by every session
// and released with the last one, so a new preference applies when the engine is rebuilt.
#[cfg(target_os = "linux")]
fn gpu_session(path: &Path, choice: Option<GpuPreference>) -> Result<Session> {
    use ort::ep::webgpu::DawnBackendType;
    use ort::ep::{ArbitrarilyConfigurableExecutionProvider, WebGPU};
    let b = ortx!(Session::builder());
    let b = ortx!(b.with_optimization_level(GraphOptimizationLevel::Level1));
    let mut ep = WebGPU::default().with_dawn_backend_type(DawnBackendType::Vulkan).with_device_id(0);
    if let Some(choice) = choice {
        let value = match choice {
            GpuPreference::HighPerformance => "high-performance",
            GpuPreference::LowPower => "low-power",
        };
        ep = ep.with_arbitrary_config("ep.webgpuexecutionprovider.powerPreference", value);
    }
    let mut b = ortx!(b.with_execution_providers([ep.build().error_on_failure()]));
    let s = ortx!(b.commit_from_file(path));
    Ok(s)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn gpu_session(_path: &Path, _choice: Option<GpuPreference>) -> Result<Session> {
    bail!("GPU inference is only available on Windows (DirectML) and Linux (WebGPU)")
}

impl PaddleOcr {
    pub fn new(models_dir: &Path, rec_stem: &str, want_gpu: bool, gpu_choice: Option<GpuPreference>) -> Result<Self> {
        let det_small = models_dir.join("det_small.onnx");
        let det_tiny = models_dir.join("det_tiny.onnx");
        let rec_path = models_dir.join(format!("{rec_stem}.onnx"));
        let dict_path = models_dir.join(format!("{rec_stem}.dict.json"));
        if !rec_path.exists() {
            bail!("recognition model not found: {}", rec_path.display());
        }

        let (det, det_model, det_on_gpu) = if want_gpu {
            match gpu_session(&det_small, gpu_choice) {
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
            match gpu_session(&rec_path, gpu_choice) {
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
        let det_on_gpu = Arc::new(AtomicBool::new(det_on_gpu));
        let rec_on_gpu = Arc::new(AtomicBool::new(rec_on_gpu));
        Ok(Self {
            det: Mutex::new(InferenceSession {
                session: Some(det),
                on_gpu: det_on_gpu.clone(),
                cpu_path: det_tiny,
                stage: "Detection",
            }),
            rec: Mutex::new(InferenceSession {
                session: Some(rec),
                on_gpu: rec_on_gpu.clone(),
                cpu_path: rec_path,
                stage: "Recognition",
            }),
            dict,
            det_on_gpu,
            rec_model: rec_stem.to_string(),
            rec_on_gpu,
        })
    }

    pub fn det_on_gpu(&self) -> bool {
        self.det_on_gpu.load(Ordering::Relaxed)
    }

    pub fn rec_on_gpu(&self) -> bool {
        self.rec_on_gpu.load(Ordering::Relaxed)
    }

    pub fn det_model(&self) -> &'static str {
        if self.det_on_gpu() {
            "det_small"
        } else {
            "det_tiny"
        }
    }

    /// Runs both models once so the first real lookup does not pay for graph setup.
    pub fn warm_up(&self) {
        let blank = RgbaImage::from_pixel(96, 64, Rgba([255, 255, 255, 255]));
        if let Err(e) = self.detect(&blank) {
            log::warn!("detector warm-up failed: {e:#}");
        }
        let b = TextBox {
            x0: 0.0,
            y0: 0.0,
            x1: 96.0,
            y1: 32.0,
        };
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
        let scale = (DET_MAX_SIDE / w.max(h) as f32).min(1.0);
        let round32 = |v: f32| -> u32 { (((v / 32.0).round() as u32) * 32).max(32) };
        let rw = round32(w as f32 * scale);
        let rh = round32(h as f32 * scale);
        let resized = image::imageops::resize(img, rw, rh, FilterType::Triangle);
        let mut boxes = Vec::new();
        let mut y = 0;
        loop {
            let height = DET_BAND_HEIGHT.min(rh - y);
            let band = image::imageops::crop_imm(&resized, 0, y, rw, height).to_image();
            let mut found = self.detect_band(&band)?;
            for b in &mut found {
                b.y0 += y as f32;
                b.y1 += y as f32;
            }
            merge_band_boxes(&mut boxes, found);
            if y + height == rh {
                break;
            }
            y += DET_BAND_HEIGHT - DET_BAND_OVERLAP;
        }
        // The rest of OCR and the overlay continue to use captured-image coordinates.
        let sx = w as f32 / rw as f32;
        let sy = h as f32 / rh as f32;
        for b in &mut boxes {
            b.x0 *= sx;
            b.x1 *= sx;
            b.y0 *= sy;
            b.y1 *= sy;
        }
        Ok(boxes)
    }

    fn detect_band(&self, img: &RgbaImage) -> Result<Vec<TextBox>> {
        let (w, h) = img.dimensions();
        if w < 8 || h < 8 {
            return Ok(Vec::new());
        }
        let longest = w.max(h) as f32;
        let scale = if longest > DET_MAX_SIDE {
            DET_MAX_SIDE / longest
        } else {
            1.0
        };
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
            det.run(cpu_session, |session| {
                let outputs = ortx!(session.run(ort::inputs![input.view()]));
                let (shape, prob) = ortx!(outputs[0].try_extract_tensor::<f32>());
                if shape.len() != 4 {
                    bail!("unexpected detector output shape {:?}", &shape[..]);
                }
                Ok((shape[2] as usize, shape[3] as usize, prob.to_vec()))
            })?
        };

        let box_thresh = if self.det_on_gpu() { 0.45 } else { 0.4 };
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
            if bw < 3.0 || bh < 3.0 || count == 0 || sum / (count as f32) < box_thresh {
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
        let Some(crop) = rec_crop(img, b) else { return Ok(None) };
        let pw = crop.tw as usize;
        let mut data = vec![0f32; 3 * REC_HEIGHT as usize * pw];
        fill_rec_input(&mut data, &crop.image, pw);
        let (steps, classes, probs) = self.run_rec(data, 1, pw)?;
        Ok(self.decode_line(&crop, &probs, steps, classes, pw))
    }

    /// Recognizes the boxes in as few recognizer runs as `plan_batches` allows. A batch that
    /// fails is retried one line at a time. Used for the whole-screen overlay.
    pub fn recognize_boxes_batched(&self, img: &RgbaImage, boxes: &[TextBox]) -> Vec<OcrLine> {
        let mut lines = Vec::with_capacity(boxes.len());
        for batch in plan_batches(boxes) {
            match self.recognize_batch(img, &batch) {
                Ok(found) => lines.extend(found),
                Err(e) => {
                    log::warn!("batched recognition failed, retrying line by line: {e:#}");
                    lines.extend(self.recognize_boxes(img, &batch));
                }
            }
        }
        lines.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
        lines
    }

    fn recognize_batch(&self, img: &RgbaImage, boxes: &[TextBox]) -> Result<Vec<OcrLine>> {
        let crops: Vec<RecCrop> = boxes.iter().filter_map(|b| rec_crop(img, *b)).collect();
        if crops.is_empty() {
            return Ok(Vec::new());
        }
        let pw = crops.iter().map(|c| c.tw).max().unwrap_or(16) as usize;
        let slot = 3 * REC_HEIGHT as usize * pw;
        let mut data = vec![0f32; crops.len() * slot];
        for (i, c) in crops.iter().enumerate() {
            fill_rec_input(&mut data[i * slot..(i + 1) * slot], &c.image, pw);
        }
        let (steps, classes, probs) = self.run_rec(data, crops.len(), pw)?;
        let per_item = steps * classes;
        if probs.len() < per_item * crops.len() {
            bail!("recognizer returned {} values for {} lines", probs.len(), crops.len());
        }
        Ok(crops
            .iter()
            .enumerate()
            .filter_map(|(i, c)| self.decode_line(c, &probs[i * per_item..(i + 1) * per_item], steps, classes, pw))
            .collect())
    }

    /// Runs the recognizer on `n` crops padded to `padded_w`; returns (steps, classes, probs).
    fn run_rec(&self, data: Vec<f32>, n: usize, padded_w: usize) -> Result<(usize, usize, Vec<f32>)> {
        let input = ortx!(Tensor::from_array(([n, 3, REC_HEIGHT as usize, padded_w], data)));
        let mut rec = self.rec.lock();
        rec.run(cpu_session, |session| {
            let outputs = ortx!(session.run(ort::inputs![input.view()]));
            let (shape, probs) = ortx!(outputs[0].try_extract_tensor::<f32>());
            if shape.len() != 3 || shape[0] as usize != n {
                bail!("unexpected recognizer output shape {:?}", &shape[..]);
            }
            Ok((shape[1] as usize, shape[2] as usize, probs.to_vec()))
        })
    }

    /// CTC-decodes one crop's output rows into a line with per-character spans.
    fn decode_line(&self, crop: &RecCrop, probs: &[f32], steps: usize, classes: usize, padded_w: usize) -> Option<OcrLine> {
        if steps == 0 || classes == 0 {
            return None;
        }
        let (b, x, cw) = (crop.b, crop.x, crop.cw);
        // In a batch, the steps past this crop's own width only see padding.
        let content_steps = steps as f32 * crop.tw as f32 / padded_w as f32;
        let valid = (content_steps.ceil() as usize).clamp(1, steps);
        // Index 0 is the CTC blank; a trailing extra class is the space character.
        let has_space = classes == self.dict.len() + 2;

        let mut decoded: Vec<(char, usize)> = Vec::new();
        let mut prev = 0usize;
        for t in 0..valid {
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
            return None;
        }

        // Each CTC step covers an equal slice of the padded input, so of the crop width.
        let step_w = cw as f32 / content_steps;
        let centers: Vec<f32> = decoded
            .iter()
            .map(|(_, t)| x as f32 + (*t as f32 + 0.5) * step_w)
            .collect();
        let blank: Vec<bool> = decoded.iter().map(|(c, _)| c.is_whitespace()).collect();
        // Word edges snap to the ink of the line, as in Lookupper.
        let spans = ink::spans(&centers, &blank, crop.ink.as_ref(), x as f32, b.x0, b.x1, step_w);
        let chars = decoded.iter().zip(spans).map(|((c, _), (x0, x1))| OcrChar { ch: *c, x0, x1 }).collect();
        let text: String = decoded.iter().map(|(c, _)| *c).collect();
        let ink_y = crop.ink.as_ref().map(|ink| (crop.y as f32 + ink.y0 as f32, crop.y as f32 + ink.y1 as f32));
        Some(OcrLine {
            text,
            x0: b.x0,
            y0: b.y0,
            x1: b.x1,
            y1: b.y1,
            chars,
            ink_y,
        })
    }
}
