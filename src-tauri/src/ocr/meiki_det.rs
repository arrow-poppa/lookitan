// MeikiOCR's line detectors (rtr46/meiki.text.detect.v0, v0.1, weights LGPL-3.0), D-FINE trained on
// Japanese game text: 960×544, or 320×192, faster and coarser. Downloaded when chosen
// (`models::MEIKI_DET`), at the user's request on 2026-10-30.
//
// As meikiocr's `run_detection`: the part read is scaled to fit the model's input (truncated to
// whole pixels), placed at the top left on black, divided by 255; `orig_target_sizes` is the input
// over that scale, so the boxes come in the part's pixels; boxes scoring over 0.5 are kept, clamped
// to the part. Lookitan's input is RGB, as its recognizers' (meikiocr's OpenCV image is BGR; the
// detectors found the same lines either way in our tests). A run finds at most 64 boxes.
//
// Where it reads, as marupop (marunine/marupop, LGPL-3.0) drives it: the automatic capture never
// gives it the whole monitor. It starts with 480×270 around the pointer (scaled with the monitor's
// height over 1080) and doubles that, up to 1600×900, while the pointer is on no line or the block
// under it touches the rectangle's edge, as long as the lines found would still be large enough at
// the model's input. marupop stops at 0.044 of the input's height (24 px at 960×544); measured here
// on 2026-10-30 (a page of 20 px Japanese, horizontal and vertical, on growing canvases) the
// 960×544 detector found every line down to about 7 px at its input and the 320×192 one down to
// 5 px, so the limits are 8 and 6 px. A larger part (the whole screen, a large fixed region) is read
// in tiles, none scaled under half (960×544) or 0.4 (320×192), and the pieces of a line two tiles
// cut are joined.
//
// On the GPU the model is changed as MeikiOCR's recognizers are (`meiki::for_gpu`): DirectML found
// one wrong box, or none, without it, and the same boxes as the CPU with it (2026-10-30, AMD Radeon).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use image::RgbaImage;
use ort::value::Tensor;
use parking_lot::Mutex;

use super::meiki::for_gpu;
use super::models::MEIKI_DET;
use super::paddle::{cpu_session, gpu_session_from_memory, InferenceSession, TextBox};
use crate::settings::GpuPreference;

const THRESHOLD: f32 = 0.5;
/// marupop's first rectangle around the pointer and its largest, at a 1080-pixel-high monitor.
const START: (f32, f32) = (480.0, 270.0);
const LARGEST: (f32, f32) = (1600.0, 900.0);
/// How close to the rectangle's edge a block is taken to go on past it (marupop's 4 px).
const EDGE: f32 = 4.0;
/// A tile overlaps the next by this part of it, so a line two tiles cut is whole in neither but
/// its pieces overlap.
const TILE_OVERLAP: f32 = 0.125;

/// A rectangle of the image, in its pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Area {
    /// `size` around the pointer, inside an image `w`×`h` (as large as the image at most).
    fn around(px: f32, py: f32, size: (f32, f32), w: f32, h: f32) -> Area {
        let (aw, ah) = (size.0.min(w), size.1.min(h));
        Area { x: (px - aw / 2.0).clamp(0.0, w - aw), y: (py - ah / 2.0).clamp(0.0, h - ah), w: aw, h: ah }
    }
}

pub struct MeikiDet {
    session: Mutex<InferenceSession>,
    on_gpu: Arc<AtomicBool>,
    /// The model's input, width and height.
    input: (usize, usize),
    /// The smallest scale a part is read at; a larger part is tiled.
    min_scale: f32,
    /// The smallest line height (a column's width) that is still found at the input, in its pixels.
    min_line: f32,
}

impl MeikiDet {
    /// The 960×544 detector, or with `small` the 320×192 one.
    pub fn new(dir: &Path, small: bool, gpu: bool, choice: Option<GpuPreference>) -> Result<Self> {
        let file = &MEIKI_DET.files[usize::from(small)];
        let path = dir.join(file.file());
        if !path.is_file() {
            bail!("MeikiOCR detector not found: {}", path.display());
        }
        let stage = if small { "MeikiDetSmall" } else { "MeikiDet" };
        let session = if gpu {
            let model = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            let fixed = for_gpu(&model).unwrap_or_else(|e| {
                log::warn!("MeikiOCR's detector is used as it is on the GPU: {e:#}");
                None
            });
            match gpu_session_from_memory(fixed.as_deref().unwrap_or(&model), choice) {
                Ok(s) => Some(s),
                Err(e) => {
                    log::warn!("Ocr.GpuFallback.{stage}: {e:#}");
                    None
                }
            }
        } else {
            None
        };
        let on_gpu = session.is_some();
        let session = match session {
            Some(s) => s,
            None => cpu_session(&path)?,
        };
        log::info!("MeikiOCR's {} detector ready on {}", if small { "320×192" } else { "960×544" }, if on_gpu { "GPU" } else { "CPU" });
        let on_gpu = Arc::new(AtomicBool::new(on_gpu));
        let session = InferenceSession { session: Some(session), on_gpu: on_gpu.clone(), cpu_path: path, stage };
        let (input, min_scale, min_line) = if small { ((320, 192), 0.4, 6.0) } else { ((960, 544), 0.5, 8.0) };
        Ok(Self { session: Mutex::new(session), on_gpu, input, min_scale, min_line })
    }

    pub fn on_gpu(&self) -> bool {
        self.on_gpu.load(Ordering::Relaxed)
    }

    /// Its input, as Settings names it.
    pub fn size_name(&self) -> &'static str {
        if self.input.0 == 320 { "320×192" } else { "960×544" }
    }

    /// Runs the model once so the first real lookup does not pay for graph setup.
    pub fn warm_up(&self) {
        let blank = RgbaImage::from_pixel(96, 64, image::Rgba([255, 255, 255, 255]));
        if let Err(e) = self.detect_boxes(&blank) {
            log::warn!("MeikiOCR detector warm-up failed: {e:#}");
        }
    }

    /// Every line of the image, top to bottom.
    pub fn detect_boxes(&self, img: &RgbaImage) -> Result<Vec<TextBox>> {
        let (w, h) = (img.width() as f32, img.height() as f32);
        let mut boxes = self.read_area(img, Area { x: 0.0, y: 0.0, w, h })?;
        boxes.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
        Ok(boxes)
    }

    /// The lines around the pointer, as marupop finds them: the first rectangle, grown while the
    /// pointer is on no line or its block goes on past the rectangle. The last rectangle that had a
    /// block under the pointer gives the lines; with none, the last one read.
    pub fn detect_around(&self, img: &RgbaImage, px: f32, py: f32) -> Result<Vec<TextBox>> {
        let (w, h) = (img.width() as f32, img.height() as f32);
        let k = (h / 1080.0).max(1.0);
        let largest = (LARGEST.0 * k, LARGEST.1 * k);
        let mut area = Area::around(px, py, (START.0 * k, START.1 * k), w, h);
        let mut found: Option<Vec<TextBox>> = None;
        loop {
            let mut boxes = self.read_area(img, area)?;
            boxes.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
            let block = crate::block::bounds(&crate::block::under(&boxes, px, py).into_iter().map(|i| boxes[i]).collect::<Vec<_>>());
            let on_text = block.is_some();
            let cut = block.is_some_and(|b| goes_past(&b, area, w, h, EDGE * k));
            let next = grow(area, px, py, largest, w, h).filter(|next| self.still_found(&boxes, *next));
            if on_text {
                found = Some(boxes.clone());
            }
            match next {
                Some(next) if !on_text || cut => area = next,
                _ => return Ok(found.unwrap_or(boxes)),
            }
        }
    }

    /// Whether the lines found so far would still be found reading `next`: their median height (a
    /// column's width) at its scale is at least `min_line`. With no line yet, it grows.
    fn still_found(&self, boxes: &[TextBox], next: Area) -> bool {
        let mut sizes: Vec<f32> = boxes.iter().map(|b| (b.x1 - b.x0).min(b.y1 - b.y0)).filter(|s| *s > 0.0).collect();
        if sizes.is_empty() {
            return true;
        }
        sizes.sort_by(f32::total_cmp);
        sizes[sizes.len() / 2] * self.scale_of(next) >= self.min_line
    }

    /// The scale `area` is read at: fitted to the input, or a tile's when it is tiled.
    fn scale_of(&self, area: Area) -> f32 {
        let (iw, ih) = (self.input.0 as f32, self.input.1 as f32);
        (iw / area.w).min(ih / area.h).max(self.min_scale)
    }

    /// The lines in `area`: one run, or a run per tile when fitting it would scale it under
    /// `min_scale`, the pieces of lines the tiles cut joined.
    fn read_area(&self, img: &RgbaImage, area: Area) -> Result<Vec<TextBox>> {
        let (iw, ih) = (self.input.0 as f32, self.input.1 as f32);
        let (tw, th) = (iw / self.min_scale, ih / self.min_scale);
        if area.w <= tw && area.h <= th {
            return self.run(img, area);
        }
        let mut boxes = Vec::new();
        for y in tile_starts(area.y, area.h, th) {
            for x in tile_starts(area.x, area.w, tw) {
                boxes.extend(self.run(img, Area { x, y, w: tw.min(area.w), h: th.min(area.h) })?);
            }
        }
        Ok(join_pieces(boxes))
    }

    /// One run of the model on `area`, its boxes in the image's pixels.
    fn run(&self, img: &RgbaImage, area: Area) -> Result<Vec<TextBox>> {
        let (x0, y0) = (area.x.max(0.0) as u32, area.y.max(0.0) as u32);
        let w = (area.w as u32).min(img.width().saturating_sub(x0));
        let h = (area.h as u32).min(img.height().saturating_sub(y0));
        if w < 2 || h < 2 {
            return Ok(Vec::new());
        }
        let (mw, mh) = self.input;
        let scale = (mw as f32 / w as f32).min(mh as f32 / h as f32);
        let (rw, rh) = (((w as f32 * scale) as u32).clamp(1, mw as u32), ((h as f32 * scale) as u32).clamp(1, mh as u32));
        let part = image::imageops::crop_imm(img, x0, y0, w, h).to_image();
        let resized = image::imageops::resize(&part, rw, rh, FilterType::Triangle);
        let plane = mw * mh;
        let mut data = vec![0f32; 3 * plane];
        for (x, y, p) in resized.enumerate_pixels() {
            let i = y as usize * mw + x as usize;
            for c in 0..3 {
                data[c * plane + i] = p[c] as f32 / 255.0;
            }
        }
        let input = ortx!(Tensor::from_array(([1usize, 3, mh, mw], data)));
        let sizes = ortx!(Tensor::from_array(([1usize, 2], vec![(mw as f32 / scale) as i64, (mh as f32 / scale) as i64])));
        let (found, scores) = self.session.lock().run(cpu_session, |s| {
            let outputs = ortx!(s.run(ort::inputs![input.view(), sizes.view()]));
            let (_, boxes) = ortx!(outputs["boxes"].try_extract_tensor::<f32>());
            let (_, scores) = ortx!(outputs["scores"].try_extract_tensor::<f32>());
            Ok((boxes.to_vec(), scores.to_vec()))
        })?;
        if found.len() < scores.len() * 4 {
            bail!("MeikiOCR's detector returned too few boxes");
        }
        let (fw, fh) = (w as f32, h as f32);
        Ok(scores
            .iter()
            .enumerate()
            .filter(|(_, s)| **s > THRESHOLD)
            .filter_map(|(i, _)| {
                let b = &found[i * 4..i * 4 + 4];
                let (bx0, by0, bx1, by1) = (b[0].clamp(0.0, fw), b[1].clamp(0.0, fh), b[2].clamp(0.0, fw), b[3].clamp(0.0, fh));
                (bx1 - bx0 >= 1.0 && by1 - by0 >= 1.0)
                    .then(|| TextBox { x0: x0 as f32 + bx0, y0: y0 as f32 + by0, x1: x0 as f32 + bx1, y1: y0 as f32 + by1, quad: None })
            })
            .collect())
    }
}

/// Whether block `b` reaches within `margin` of an edge of `area` that is not the image's own: the
/// text may go on past it.
fn goes_past(b: &TextBox, area: Area, w: f32, h: f32, margin: f32) -> bool {
    (area.x > 0.0 && b.x0 - area.x <= margin)
        || (area.y > 0.0 && b.y0 - area.y <= margin)
        || (area.x + area.w < w && area.x + area.w - b.x1 <= margin)
        || (area.y + area.h < h && area.y + area.h - b.y1 <= margin)
}

/// marupop's next rectangle: each side doubled (at most `largest`, and the image), about the same
/// middle, moved inside the image and onto the pointer. `None` once it cannot grow.
fn grow(area: Area, px: f32, py: f32, largest: (f32, f32), w: f32, h: f32) -> Option<Area> {
    let (nw, nh) = ((area.w * 2.0).min(largest.0).min(w), (area.h * 2.0).min(largest.1).min(h));
    if nw <= area.w && nh <= area.h {
        return None;
    }
    let (cx, cy) = (area.x + area.w / 2.0, area.y + area.h / 2.0);
    let x = (cx - nw / 2.0).clamp(0.0, w - nw).max(px - nw).min(px);
    let y = (cy - nh / 2.0).clamp(0.0, h - nh).max(py - nh).min(py);
    Some(Area { x: x.clamp(0.0, w - nw), y: y.clamp(0.0, h - nh), w: nw, h: nh })
}

/// Where tiles `tile` long start along a side `length` long from `from`: evenly spread, the first
/// at its start and the last at its end, overlapping by at least `TILE_OVERLAP` of a tile.
fn tile_starts(from: f32, length: f32, tile: f32) -> Vec<f32> {
    if length <= tile {
        return vec![from];
    }
    let step = tile * (1.0 - TILE_OVERLAP);
    let n = ((length - tile) / step).ceil() as usize + 1;
    (0..n).map(|i| from + (length - tile) * i as f32 / (n - 1) as f32).collect()
}

/// Joins the pieces of a line two tiles cut: boxes that overlap and share most of a row (or of a
/// column) become one.
fn join_pieces(mut boxes: Vec<TextBox>) -> Vec<TextBox> {
    let same_line = |a: &TextBox, b: &TextBox| {
        let (ox, oy) = (a.x1.min(b.x1) - a.x0.max(b.x0), a.y1.min(b.y1) - a.y0.max(b.y0));
        ox > 0.0 && oy > 0.0 && (oy >= 0.6 * (a.y1 - a.y0).min(b.y1 - b.y0) || ox >= 0.6 * (a.x1 - a.x0).min(b.x1 - b.x0))
    };
    let mut i = 0;
    while i < boxes.len() {
        let mut joined = false;
        let mut j = i + 1;
        while j < boxes.len() {
            if same_line(&boxes[i], &boxes[j]) {
                let b = boxes.swap_remove(j);
                let a = &mut boxes[i];
                (a.x0, a.y0, a.x1, a.y1) = (a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1));
                joined = true;
            } else {
                j += 1;
            }
        }
        // A grown box may now overlap one it was checked against before.
        if !joined {
            i += 1;
        }
    }
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x0: f32, y0: f32, x1: f32, y1: f32) -> TextBox {
        TextBox { x0, y0, x1, y1, quad: None }
    }

    #[test]
    fn the_rectangle_doubles_about_its_middle_inside_the_image_and_on_the_pointer() {
        let start = Area::around(100.0, 100.0, START, 2560.0, 1440.0);
        assert_eq!(start, Area { x: 0.0, y: 0.0, w: 480.0, h: 270.0 }, "moved inside the image");
        let next = grow(start, 100.0, 100.0, LARGEST, 2560.0, 1440.0).unwrap();
        assert_eq!(next, Area { x: 0.0, y: 0.0, w: 960.0, h: 540.0 });
        let mid = Area::around(1280.0, 720.0, START, 2560.0, 1440.0);
        let grown = grow(mid, 1280.0, 720.0, LARGEST, 2560.0, 1440.0).unwrap();
        assert_eq!((grown.x + grown.w / 2.0, grown.y + grown.h / 2.0), (1280.0, 720.0), "about the same middle");
        let last = grow(grown, 1280.0, 720.0, LARGEST, 2560.0, 1440.0).unwrap();
        assert_eq!((last.w, last.h), (1600.0, 900.0), "at most the largest");
        assert!(grow(last, 1280.0, 720.0, LARGEST, 2560.0, 1440.0).is_none());
        // A small image: as large as it.
        let small = Area::around(50.0, 50.0, START, 300.0, 200.0);
        assert_eq!((small.w, small.h), (300.0, 200.0));
        assert!(grow(small, 50.0, 50.0, LARGEST, 300.0, 200.0).is_none());
    }

    #[test]
    fn a_block_at_the_rectangle_s_edge_goes_on_past_it_unless_the_edge_is_the_image_s() {
        let area = Area { x: 100.0, y: 0.0, w: 480.0, h: 270.0 };
        assert!(goes_past(&b(102.0, 50.0, 300.0, 70.0), area, 2560.0, 1440.0, 4.0), "the left edge");
        assert!(goes_past(&b(200.0, 50.0, 300.0, 268.0), area, 2560.0, 1440.0, 4.0), "the bottom edge");
        assert!(!goes_past(&b(200.0, 0.0, 300.0, 70.0), area, 2560.0, 1440.0, 4.0), "the top is the image's");
        assert!(!goes_past(&b(200.0, 50.0, 300.0, 70.0), area, 2560.0, 1440.0, 4.0));
    }

    #[test]
    fn tiles_cover_the_side_overlapping_and_the_pieces_of_a_cut_line_are_joined() {
        assert_eq!(tile_starts(0.0, 1000.0, 1920.0), vec![0.0]);
        let starts = tile_starts(0.0, 2560.0, 1920.0);
        assert_eq!(starts, vec![0.0, 640.0]);
        let starts = tile_starts(10.0, 5000.0, 1000.0);
        assert!(starts.windows(2).all(|w| w[1] - w[0] <= 1000.0 * (1.0 - TILE_OVERLAP) + 0.01));
        assert_eq!((starts[0], *starts.last().unwrap()), (10.0, 4010.0));
        // A row cut by two tiles, a column cut by two, a line beside them and one below.
        let joined = join_pieces(vec![
            b(1800.0, 100.0, 1920.0, 130.0),
            b(1700.0, 101.0, 2100.0, 131.0),
            b(500.0, 1000.0, 530.0, 1088.0),
            b(501.0, 960.0, 529.0, 1300.0),
            b(1800.0, 140.0, 2100.0, 170.0),
            b(10.0, 10.0, 200.0, 40.0),
        ]);
        assert_eq!(joined.len(), 4, "{joined:?}");
        assert!(joined.iter().any(|t| (t.x0, t.x1) == (1700.0, 2100.0) && (t.y0, t.y1) == (100.0, 131.0)));
        assert!(joined.iter().any(|t| (t.y0, t.y1) == (960.0, 1300.0)));
    }

    /// The real 960×544 and 320×192 detectors on the Japanese fixture: every line, on the whole
    /// image and around a pointer. On Windows each is also asked for the GPU (DirectML, which
    /// falls back to the CPU on the runner): the same lines. Needs the fetched ONNX Runtime and
    /// the models in `LOOKITAN_MEIKI_DET` (CI downloads them; the test is skipped without them).
    #[test]
    #[ignore]
    fn real_meiki_det_finds_the_fixture_s_lines() {
        let Some(dir) = std::env::var_os("LOOKITAN_MEIKI_DET").map(std::path::PathBuf::from) else {
            eprintln!("LOOKITAN_MEIKI_DET is not set; skipped");
            return;
        };
        let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let library = resources.join("ort").join(if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" });
        ort::init_from(&library).expect("ONNX Runtime").with_name("lookitan-test").commit();
        let img = image::load_from_memory(include_bytes!("fixtures/ja_lines.png")).unwrap().to_rgba8();
        for small in [false, true] {
            let det = MeikiDet::new(&dir, small, false, None).unwrap();
            let started = std::time::Instant::now();
            let boxes = det.detect_boxes(&img).unwrap();
            eprintln!("MeikiOCR detector (small: {small}): {} lines in {} ms: {boxes:?}", boxes.len(), started.elapsed().as_millis());
            let mut rows: Vec<f32> = boxes.iter().filter(|b| b.x1 - b.x0 > 20.0).map(|b| (b.y0 + b.y1) / 2.0).collect();
            rows.sort_by(f32::total_cmp);
            rows.dedup_by(|a, b| (*a - *b).abs() < 12.0);
            assert!(rows.len() >= 10, "{rows:?}");
            for t in &boxes {
                assert!(t.x0 >= 0.0 && t.y0 >= 0.0 && t.x1 <= img.width() as f32 && t.y1 <= img.height() as f32, "{t:?}");
            }
            // Around a pointer on the first line: that line is among those found.
            let first = boxes[0];
            let (px, py) = ((first.x0 + first.x1) / 2.0, (first.y0 + first.y1) / 2.0);
            let around = det.detect_around(&img, px, py).unwrap();
            assert!(!crate::block::under(&around, px, py).is_empty(), "{around:?}");
            if cfg!(windows) {
                let gpu = MeikiDet::new(&dir, small, true, None).unwrap();
                eprintln!("MeikiOCR detector asked for the GPU: on {}", if gpu.on_gpu() { "GPU" } else { "CPU" });
                assert_eq!(gpu.detect_boxes(&img).unwrap().len(), boxes.len());
            }
        }
    }
}
