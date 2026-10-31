// comic-text-detector (dmMaze/comic-text-detector, GPL-3.0, as Lookitan; the model
// manga-image-translator publishes, release beta-0.2.1) as a line detector for manga and comics, at
// the user's request on 2026-10-30. It is downloaded when chosen, never bundled (`models::CTD`).
//
// As its `TextDetector` with the ONNX model: the image in RGB / 255, letterboxed into 1024 × 1024
// (scaled to fit, padded with black at the right and bottom); of its three outputs only `det`, the
// DBNet line map (channel 0: probability), is read, with its `SegDetectorRepresenter`'s threshold
// 0.3, box threshold 0.6 and unclip ratio 1.5 through PP-OCR's DB decoding (`paddle::db_boxes`). Its
// text blocks (`blk`) and mask (`seg`) are not used: Lookitan groups lines into blocks itself.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Result};
use image::imageops::FilterType;
use image::RgbaImage;
use ort::value::Tensor;
use parking_lot::Mutex;

use super::paddle::{cpu_session, db_boxes, gpu_session, infer, InferenceSession, TextBox};
use crate::settings::GpuPreference;

pub const FILE: &str = "comictextdetector.pt.onnx";
const INPUT: u32 = 1024;
const THRESH: f32 = 0.3;
const BOX_THRESH: f32 = 0.6;
const UNCLIP: f32 = 1.5;

pub struct CtdDet {
    session: Mutex<InferenceSession>,
    on_gpu: Arc<AtomicBool>,
}

impl CtdDet {
    pub fn new(dir: &Path, gpu: bool, choice: Option<GpuPreference>) -> Result<Self> {
        let path = dir.join(FILE);
        if !path.is_file() {
            bail!("comic-text-detector not found: {}", path.display());
        }
        let (session, on_gpu) = if gpu {
            match gpu_session(&path, choice) {
                Ok(s) => (s, true),
                Err(e) => {
                    log::warn!("Ocr.GpuFallback.Ctd: {e:#}");
                    (cpu_session(&path)?, false)
                }
            }
        } else {
            (cpu_session(&path)?, false)
        };
        log::info!("comic-text-detector ready on {}", if on_gpu { "GPU" } else { "CPU" });
        let on_gpu = Arc::new(AtomicBool::new(on_gpu));
        let session = InferenceSession { session: Some(session), on_gpu: on_gpu.clone(), cpu_path: path, stage: "Ctd" };
        Ok(Self { session: Mutex::new(session), on_gpu })
    }

    pub fn on_gpu(&self) -> bool {
        self.on_gpu.load(Ordering::Relaxed)
    }

    /// Runs the model once so the first real lookup does not pay for graph setup.
    pub fn warm_up(&self) {
        let blank = RgbaImage::from_pixel(96, 64, image::Rgba([255, 255, 255, 255]));
        if let Err(e) = self.detect_boxes(&blank) {
            log::warn!("comic-text-detector warm-up failed: {e:#}");
        }
    }

    /// Every line, top to bottom, in the image's pixels.
    pub fn detect_boxes(&self, img: &RgbaImage) -> Result<Vec<TextBox>> {
        let (w, h) = img.dimensions();
        if w < 8 || h < 8 {
            return Ok(Vec::new());
        }
        let side = INPUT as f32;
        let r = (side / w as f32).min(side / h as f32);
        let (nw, nh) = (((w as f32 * r).round() as u32).clamp(1, INPUT), ((h as f32 * r).round() as u32).clamp(1, INPUT));
        let resized = image::imageops::resize(img, nw, nh, FilterType::Triangle);
        let plane = (INPUT * INPUT) as usize;
        // Black padding: 0 in every channel.
        let mut data = vec![0f32; 3 * plane];
        for (x, y, p) in resized.enumerate_pixels() {
            let i = (y * INPUT + x) as usize;
            for c in 0..3 {
                data[c * plane + i] = p[c] as f32 / 255.0;
            }
        }
        let input = ortx!(Tensor::from_array(([1usize, 3, INPUT as usize, INPUT as usize], data)));
        let (dims, map) = self.session.lock().run(cpu_session, |s, on_cpu| {
            let outputs = infer(s, ort::inputs![input.view()], on_cpu)?;
            let (shape, map) = ortx!(outputs["det"].try_extract_tensor::<f32>());
            Ok((shape[..].to_vec(), map.to_vec()))
        })?;
        if dims.len() != 4 || dims[1] < 1 {
            bail!("unexpected comic-text-detector output shape {dims:?}");
        }
        let (mh, mw) = (dims[2] as usize, dims[3] as usize);
        // Map pixels to the letterboxed input's, then to the image's.
        let scale = ((side / mw as f32) / r, (side / mh as f32) / r);
        let mut boxes = db_boxes(&map[..mh * mw], mw, mh, scale, (w as f32, h as f32), (THRESH, BOX_THRESH, UNCLIP));
        boxes.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
        Ok(boxes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real model on the Japanese fixture (dark on white, light on dark blue): it finds every
    /// line, though it splits a line of game text into pieces (measured in Python before porting:
    /// 21 pieces for its 10 lines). On Windows it is also asked for the GPU (DirectML, which falls
    /// back to the CPU on the runner): its lines must be the same. Needs the fetched ONNX Runtime
    /// and the model in `LOOKITAN_CTD` (CI downloads it; the test is skipped without it).
    #[test]
    #[ignore]
    fn real_ctd_finds_the_fixture_s_lines() {
        let Some(dir) = std::env::var_os("LOOKITAN_CTD").map(std::path::PathBuf::from) else {
            eprintln!("LOOKITAN_CTD is not set; skipped");
            return;
        };
        let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let library = resources.join("ort").join(if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" });
        ort::init_from(&library).expect("ONNX Runtime").with_name("lookitan-test").commit();
        let img = image::load_from_memory(include_bytes!("fixtures/ja_lines.png")).unwrap().to_rgba8();
        let det = CtdDet::new(&dir, false, None).unwrap();
        let started = std::time::Instant::now();
        let boxes = det.detect_boxes(&img).unwrap();
        eprintln!("comic-text-detector: {} pieces in {} ms: {boxes:?}", boxes.len(), started.elapsed().as_millis());
        // The fixture's 10 lines, by their middles: each has a piece across it.
        let mut rows: Vec<f32> = boxes.iter().filter(|b| b.x1 - b.x0 > 20.0).map(|b| (b.y0 + b.y1) / 2.0).collect();
        rows.sort_by(f32::total_cmp);
        rows.dedup_by(|a, b| (*a - *b).abs() < 12.0);
        assert!(rows.len() >= 10, "{rows:?}");
        for b in &boxes {
            assert!(b.x0 >= 0.0 && b.y0 >= 0.0 && b.x1 <= img.width() as f32 && b.y1 <= img.height() as f32, "{b:?}");
        }
        if cfg!(windows) {
            let gpu = CtdDet::new(&dir, true, None).unwrap();
            let on = gpu.detect_boxes(&img).unwrap();
            eprintln!("comic-text-detector asked for the GPU: on {}", if gpu.on_gpu() { "GPU" } else { "CPU" });
            assert_eq!(on.len(), boxes.len());
        }
    }
}
