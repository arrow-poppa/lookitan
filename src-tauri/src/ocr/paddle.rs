// PP-OCRv6 / PP-OCRv5 text detection (DB) and recognition (CTC) on ONNX Runtime.
//
// Engine choice mirrors Lookupper: try the `small` detector on the GPU (DirectML on Windows, WebGPU
// on Linux) first and fall back to the `tiny` detector on the CPU with full graph optimization.
// Parameters (normalization, thresholds, unclip ratio) come from the official PaddlePaddle
// `inference.yml` of each model.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

    fn fake(gpu: bool) -> FakeSession {
        FakeSession { gpu, released: Arc::new(AtomicBool::new(false)) }
    }

    fn fake_session(gpu: bool) -> InferenceSession<FakeSession> {
        InferenceSession {
            gpu: gpu.then(|| fake(true)),
            cpu: (!gpu).then(|| fake(false)),
            gpu_path: gpu.then(|| PathBuf::from("det_small.onnx")),
            cpu_path: PathBuf::from("det_tiny.onnx"),
            on_gpu: Arc::new(AtomicBool::new(gpu)),
            stage: "Detection",
            gpu_failures: 0,
            gpu_retry_at: None,
        }
    }

    fn fails_on_gpu(s: &mut FakeSession) -> Result<u32> {
        if s.gpu {
            bail!("Add.176: 8007000E");
        }
        Ok(42)
    }

    #[test]
    fn gpu_failure_runs_this_input_on_cpu_and_retries_gpu_next_time() {
        let mut session = fake_session(true);
        let released = session.gpu.as_ref().unwrap().released.clone();
        let output = session
            .run(
                |_| panic!("the loaded GPU session is tried first"),
                |path| {
                    assert_eq!(path, Path::new("det_tiny.onnx"));
                    assert!(released.load(Ordering::Relaxed), "the failed GPU graph is released first");
                    Ok(fake(false))
                },
                fails_on_gpu,
            )
            .unwrap();
        assert_eq!(output, 42);
        assert!(!session.on_gpu.load(Ordering::Relaxed));
        assert!(session.gpu.is_none());

        // The next input goes back to the GPU and keeps the CPU session for later failures.
        let output = session
            .run(
                |path| {
                    assert_eq!(path, Path::new("det_small.onnx"));
                    Ok(fake(true))
                },
                |_| panic!("CPU session must be reused, not reloaded"),
                |s| Ok(if s.gpu { 7 } else { 0 }),
            )
            .unwrap();
        assert_eq!(output, 7);
        assert!(session.on_gpu.load(Ordering::Relaxed));
        assert_eq!(session.gpu_failures, 0);
        assert!(session.cpu.is_some());
    }

    #[test]
    fn successful_gpu_inference_does_not_load_cpu() {
        let mut session = fake_session(true);
        let out = session.run(|_| panic!("GPU is loaded"), |_| panic!("CPU must not load"), |_| Ok(42));
        assert_eq!(out.unwrap(), 42);
        assert!(session.on_gpu.load(Ordering::Relaxed));
        assert!(session.cpu.is_none());
    }

    #[test]
    fn repeated_gpu_failures_back_off_before_retrying() {
        let mut session = fake_session(true);
        for _ in 0..GPU_FAILURES_BEFORE_BACKOFF {
            session.run(|_| Ok(fake(true)), |_| Ok(fake(false)), fails_on_gpu).unwrap();
        }
        assert!(session.gpu_retry_at.is_some());
        // Inside the back-off window the GPU is not even created.
        let out = session.run(|_| panic!("GPU must wait"), |_| panic!("CPU is loaded"), fails_on_gpu);
        assert_eq!(out.unwrap(), 42);
        // Once the window has passed, the GPU is tried again.
        session.gpu_retry_at = Some(Instant::now() - Duration::from_secs(1));
        let out = session.run(|_| Ok(fake(true)), |_| panic!("CPU is loaded"), |s| Ok(s.gpu));
        assert!(out.unwrap());
        assert_eq!(session.gpu_failures, 0);
        assert!(session.gpu_retry_at.is_none());
    }

    #[test]
    fn gpu_that_cannot_load_falls_back_to_cpu_for_this_input() {
        let mut session = fake_session(true);
        session.gpu = None;
        let out = session.run(|_| bail!("no adapter"), |_| Ok(fake(false)), |s| Ok(s.gpu));
        assert!(!out.unwrap());
        assert_eq!(session.gpu_failures, 1);
    }

    #[test]
    fn cpu_error_is_returned_without_recreating_session() {
        let mut session = fake_session(false);
        let error = session
            .run(
                |_| panic!("GPU is off"),
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
            .run(|_| panic!("GPU is loaded"), |_| bail!("missing CPU model"), |_| -> Result<()> { bail!("GPU error") })
            .unwrap_err();
        assert!(format!("{error:#}").contains("missing CPU model"));
        assert!(session.cpu.is_none());
        session.run(|_| bail!("still failing"), |_| Ok(fake(false)), |_| Ok(())).unwrap();
        assert!(session.cpu.is_some());
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
    /// Left edge and width of the crop in capture pixels.
    x: u32,
    cw: u32,
    /// Width after resizing.
    tw: u32,
    image: RgbaImage,
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
    let tw = rec_width(cw, ch);
    let image = image::imageops::resize(&crop, tw, REC_HEIGHT, FilterType::Triangle);
    Some(RecCrop { b, x, cw, tw, image })
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

/// Always prefers the GPU. A failed GPU load or inference (for example running out of video
/// memory) releases the GPU session and runs only that input on the CPU; the next input loads the
/// GPU session again. After repeated failures in a row the GPU is retried every
/// `GPU_RETRY_BACKOFF` instead of on every input, so a broken driver does not slow each lookup.
/// Recovery stays inside the session lock so foreground and overlay OCR share it.
struct InferenceSession<S = Session> {
    gpu: Option<S>,
    cpu: Option<S>,
    /// `None` when the GPU is turned off in Settings.
    gpu_path: Option<PathBuf>,
    cpu_path: PathBuf,
    /// Whether the last input ran on the GPU.
    on_gpu: Arc<AtomicBool>,
    stage: &'static str,
    gpu_failures: u32,
    gpu_retry_at: Option<Instant>,
}

const GPU_FAILURES_BEFORE_BACKOFF: u32 = 3;
const GPU_RETRY_BACKOFF: Duration = Duration::from_secs(30);

impl<S> InferenceSession<S> {
    fn gpu_failed(&mut self, what: &str, error: &anyhow::Error) {
        log::warn!("Ocr.GpuFallback.{} {what}: {error:#}", self.stage);
        // Release the failed GPU graph and its allocations before the CPU model loads.
        drop(self.gpu.take());
        self.gpu_failures += 1;
        if self.gpu_failures >= GPU_FAILURES_BEFORE_BACKOFF {
            self.gpu_retry_at = Some(Instant::now() + GPU_RETRY_BACKOFF);
        }
    }

    fn run<T>(
        &mut self,
        create_gpu: impl FnOnce(&Path) -> Result<S>,
        create_cpu: impl FnOnce(&Path) -> Result<S>,
        mut infer: impl FnMut(&mut S) -> Result<T>,
    ) -> Result<T> {
        let gpu_due = self.gpu_retry_at.map_or(true, |at| Instant::now() >= at);
        if let (Some(path), true) = (self.gpu_path.clone(), gpu_due) {
            if self.gpu.is_none() {
                match create_gpu(&path) {
                    Ok(s) => self.gpu = Some(s),
                    Err(e) => self.gpu_failed("while loading", &e),
                }
            }
            if let Some(gpu) = self.gpu.as_mut() {
                match infer(gpu) {
                    Ok(output) => {
                        self.gpu_failures = 0;
                        self.gpu_retry_at = None;
                        self.on_gpu.store(true, Ordering::Relaxed);
                        return Ok(output);
                    }
                    Err(e) => self.gpu_failed("during inference", &e),
                }
            }
        }
        if self.cpu.is_none() {
            self.cpu = Some(create_cpu(&self.cpu_path).with_context(|| format!("{} CPU session", self.stage))?);
        }
        self.on_gpu.store(false, Ordering::Relaxed);
        infer(self.cpu.as_mut().unwrap())
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

#[cfg(windows)]
fn gpu_session(path: &Path) -> Result<Session> {
    use ort::ep::DirectML;
    let b = ortx!(Session::builder());
    let b = ortx!(b.with_optimization_level(GraphOptimizationLevel::Level1));
    let b = ortx!(b.with_memory_pattern(false));
    let b = ortx!(b.with_parallel_execution(false));
    let mut b = ortx!(b.with_execution_providers([DirectML::default().with_device_id(0).build().error_on_failure()]));
    let s = ortx!(b.commit_from_file(path));
    Ok(s)
}

// Linux has no DirectML; the bundled ONNX Runtime build carries the WebGPU provider, which runs on
// Vulkan through Dawn (Mesa RADV, ANV, NVIDIA).
#[cfg(target_os = "linux")]
fn gpu_session(path: &Path) -> Result<Session> {
    use ort::ep::webgpu::DawnBackendType;
    use ort::ep::WebGPU;
    let b = ortx!(Session::builder());
    let b = ortx!(b.with_optimization_level(GraphOptimizationLevel::Level1));
    let mut b = ortx!(b.with_execution_providers([WebGPU::default()
        .with_dawn_backend_type(DawnBackendType::Vulkan)
        .with_device_id(0)
        .build()
        .error_on_failure()]));
    let s = ortx!(b.commit_from_file(path));
    Ok(s)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn gpu_session(_path: &Path) -> Result<Session> {
    bail!("GPU inference is only available on Windows (DirectML) and Linux (WebGPU)")
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

        // Load the GPU sessions now so the first lookup does not pay for it; a GPU that fails here
        // is retried by InferenceSession on the next input.
        let load_gpu = |path: &Path, stage: &str| match gpu_session(path) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("Ocr.GpuFallback.{stage} while loading: {e:#}");
                None
            }
        };
        let det_gpu = if want_gpu { load_gpu(&det_small, "Detection") } else { None };
        let rec_gpu = if want_gpu { load_gpu(&rec_path, "Recognition") } else { None };
        let det_cpu = if det_gpu.is_none() { Some(cpu_session(&det_tiny)?) } else { None };
        let rec_cpu = if rec_gpu.is_none() { Some(cpu_session(&rec_path)?) } else { None };
        let (det_on_gpu, rec_on_gpu) = (det_gpu.is_some(), rec_gpu.is_some());
        let det_model = if det_on_gpu { "det_small" } else { "det_tiny" };
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
                gpu: det_gpu,
                cpu: det_cpu,
                gpu_path: want_gpu.then(|| det_small),
                cpu_path: det_tiny,
                on_gpu: det_on_gpu.clone(),
                stage: "Detection",
                gpu_failures: 0,
                gpu_retry_at: None,
            }),
            rec: Mutex::new(InferenceSession {
                gpu: rec_gpu,
                cpu: rec_cpu,
                gpu_path: want_gpu.then(|| rec_path.clone()),
                cpu_path: rec_path,
                on_gpu: rec_on_gpu.clone(),
                stage: "Recognition",
                gpu_failures: 0,
                gpu_retry_at: None,
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
            det.run(gpu_session, cpu_session, |session| {
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
        rec.run(gpu_session, cpu_session, |session| {
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
        let mut chars = Vec::with_capacity(decoded.len());
        for (i, (c, _)) in decoded.iter().enumerate() {
            let left = if i == 0 {
                b.x0.max(centers[0] - step_w * 2.0)
            } else {
                (centers[i - 1] + centers[i]) / 2.0
            };
            let right = if i + 1 == decoded.len() {
                b.x1.min(centers[i] + step_w * 2.0)
            } else {
                (centers[i] + centers[i + 1]) / 2.0
            };
            chars.push(OcrChar {
                ch: *c,
                x0: left,
                x1: right,
            });
        }
        let text: String = decoded.iter().map(|(c, _)| *c).collect();
        Some(OcrLine {
            text,
            x0: b.x0,
            y0: b.y0,
            x1: b.x1,
            y1: b.y1,
            chars,
        })
    }
}
