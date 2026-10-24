// PP-OCRv6 / PP-OCRv5 text detection (DB) and recognition (CTC) on ONNX Runtime.
//
// Engine choice mirrors Lookupper: try the `small` detector on the GPU (DirectML on Windows, WebGPU
// on Linux) first and fall back to the `tiny` detector on the CPU with full graph optimization.
// Normalization comes from the official PaddlePaddle `inference.yml` of each model; the detector's
// border, binarization, unclip ratio and image size are Lookupper's.

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

use super::{quad, ink, is_rtl_text, is_vertical_box, reverse_rtl, Flow, OcrChar, OcrLine, Tilt};
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
        let line = |w: f32| TextBox { x0: 0.0, y0: 0.0, x1: w, y1: 48.0, quad: None };
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
            quad: None,
        }];
        merge_band_boxes(
            &mut boxes,
            vec![
                TextBox {
                    x0: 18.0,
                    x1: 222.0,
                    y0: 480.0,
                    y1: 524.0,
                    quad: None,
                },
                TextBox {
                    x0: 260.0,
                    x1: 460.0,
                    y0: 480.0,
                    y1: 524.0,
                    quad: None,
                },
                TextBox {
                    x0: 20.0,
                    x1: 220.0,
                    y0: 550.0,
                    y1: 590.0,
                    quad: None,
                },
            ],
        );
        assert_eq!(boxes.len(), 3);
        assert_eq!(
            (boxes[0].x0, boxes[0].x1, boxes[0].y0, boxes[0].y1),
            (18.0, 222.0, 480.0, 524.0)
        );
    }

    #[test]
    fn a_gap_wider_than_the_line_s_own_spacing_splits_words() {
        let text = |d: Vec<(char, usize)>| split_wide_gaps(d).into_iter().map(|(c, _)| c).collect::<String>();
        // "ArquivoEditar" with letters 3 steps apart and 20 steps between the two labels.
        let mut d: Vec<(char, usize)> = "Arquivo".chars().enumerate().map(|(i, c)| (c, 3 * i)).collect();
        d.extend("Editar".chars().enumerate().map(|(i, c)| (c, 38 + 3 * i)));
        assert_eq!(text(d.clone()), "Arquivo Editar");
        let space = split_wide_gaps(d).into_iter().find(|&(c, _)| c == ' ').unwrap();
        assert_eq!(space.1, (18 + 38) / 2, "the space sits in the middle of the gap");
        // Gaps up to 2.5 times the median, and never more than 5 steps, stay one word.
        assert_eq!(text("abcdef".chars().enumerate().map(|(i, c)| (c, 4 * i + if i > 3 { 6 } else { 0 })).collect()), "abcdef");
        assert_eq!(text(vec![('a', 0), ('b', 1), ('c', 2), ('d', 7)]), "abcd", "a gap of 5 is not over the floor");
        assert_eq!(text(vec![('a', 0), ('b', 1), ('c', 2), ('d', 8)]), "abc d");
        // A space the recognizer read is kept, and none is added next to it.
        assert_eq!(text(vec![('a', 0), (' ', 20), ('b', 40)]), "a b");
    }

    #[test]
    fn only_a_turned_kernel_gets_a_turned_rectangle() {
        let upright: Vec<(usize, usize)> = (10..40).flat_map(|x| (5..9).map(move |y| (x, y))).collect();
        assert!(turned_kernel(&upright, 5, 8).is_none(), "an upright line keeps its plain box");
        // A bar 40 long and 4 tall, rising 1 in 4.
        let tilted: Vec<(usize, usize)> = (0..40).flat_map(|x| (0..4).map(move |y| (x, y + 10 - x / 4))).collect();
        let r = turned_kernel(&tilted, 0, 13).expect("turned");
        assert!(r.u[1] < 0.0 && r.half_w > 4.0 * r.half_h, "{r:?}");
        let b = TextBox { x0: 0.0, y0: 0.0, x1: 50.0, y1: 50.0, quad: Some(r.corners()) };
        let (w, h) = b.size();
        assert!(w > 4.0 * h, "the size of a turned box is its own, not its bounds': {w} x {h}");
        assert_eq!(first_turns(&b), [Turn::Upright]);
    }

    #[test]
    fn a_vertical_box_is_read_on_its_side_both_ways() {
        // A white 20 x 100 column at (40, 10), inked in its top 30 rows and right 8 columns.
        let img = RgbaImage::from_fn(100, 200, |x, y| {
            let inked = (52..60).contains(&x) && (10..40).contains(&y);
            if inked { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) }
        });
        let b = TextBox { x0: 40.0, y0: 10.0, x1: 60.0, y1: 110.0, quad: None };
        assert_eq!(first_turns(&b), [Turn::Left], "a tall box is read top to bottom first");
        let left = rec_crop(&img, b, Turn::Left).unwrap();
        assert_eq!(left.image.height(), REC_HEIGHT);
        assert_eq!(left.tw, rec_width(100, 20), "as long as the column is tall");
        assert_eq!(box_rec_width(&b), left.tw);
        let ink = left.ink.unwrap();
        // Turned left: the top of the column is at the left, and its right side at the top.
        assert!(ink.cols[..30].iter().all(|&c| c) && !ink.cols[40..].iter().any(|&c| c), "{:?}", ink.cols);
        assert_eq!((ink.y0, ink.y1), (0, 8));
        // Turned right: the top is at the right, and the right side at the bottom.
        let ink = rec_crop(&img, b, Turn::Right).unwrap().ink.unwrap();
        assert!(ink.cols[70..].iter().all(|&c| c) && !ink.cols[..60].iter().any(|&c| c), "{:?}", ink.cols);
        assert_eq!((ink.y0, ink.y1), (12, 20));
        assert_eq!(first_turns(&TextBox { x0: 0.0, y0: 0.0, x1: 100.0, y1: 40.0, quad: None }), [Turn::Upright]);
    }

    #[test]
    fn a_short_vertical_box_is_a_column_and_is_read_bottom_to_top_only_when_needed() {
        // 期許。 in a 50 x 94 box: read upright, the squeezed column gave only 期, as sure as 期許.
        let short = TextBox { x0: 0.0, y0: 0.0, x1: 50.0, y1: 94.0, quad: None };
        assert_eq!(first_turns(&short), [Turn::Left], "a short column is read on its side, as in Lookupper");
        let tall = TextBox { x0: 0.0, y0: 0.0, x1: 20.0, y1: 100.0, quad: None };
        assert!(!needs_right(&tall, Some(0.95)), "read well top to bottom: no second reading");
        assert!(needs_right(&tall, Some(0.4)) && needs_right(&tall, None));
        assert!(!needs_right(&TextBox { x0: 0.0, y0: 0.0, x1: 100.0, y1: 20.0, quad: None }, None), "never a horizontal box");
        assert!(better(0.9, Some(0.5)));
        assert!(!better(0.5, Some(0.5)), "a tie keeps top to bottom");
    }
}

/// Binarization of the probability map, as Lookupper (76 / 255) and PaddleOCR's default.
const DET_THRESH: f32 = 0.3;
const UNCLIP_RATIO: f32 = 1.35;
/// White border around the detector's image, in detector pixels, as Lookupper's WhitePadding:
/// text touching the edge of a capture is still found whole.
const DET_PADDING: u32 = 20;
/// Longest side of the image the detector sees, as in Lookupper: the GPU detector gets more
/// pixels, so small text (dense CJK above all) is not shrunk as much.
const DET_MAX_SIDE_GPU: f32 = 1920.0;
const DET_MAX_SIDE_CPU: f32 = 1280.0;
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

/// A gap between two characters wider than this many times the line's median gap (in recognizer
/// steps), within `WORD_GAP_MIN..=WORD_GAP_MAX` steps, splits words, as in Lookupper's recognizer.
const WORD_GAP_FACTOR: f64 = 2.5;
const WORD_GAP_MIN: usize = 5;
const WORD_GAP_MAX: usize = 20;
/// The split for a line of fewer than two characters (no gap to measure).
const WORD_GAP_DEFAULT: usize = 8;

/// Puts a space where the recognizer saw a wide gap with none, as Lookupper's recognizer splits
/// its words (REA: `0x14094c7a0`): two labels or table cells in one detected box are two words,
/// not one ("ArquivoEditar"). `decoded` holds each character with its recognizer step.
fn split_wide_gaps(decoded: Vec<(char, usize)>) -> Vec<(char, usize)> {
    let mut gaps: Vec<usize> = decoded.windows(2).map(|w| w[1].1 - w[0].1).collect();
    gaps.sort_unstable();
    let limit = if gaps.is_empty() {
        WORD_GAP_DEFAULT
    } else {
        ((gaps[gaps.len() / 2] as f64 * WORD_GAP_FACTOR) as usize).clamp(WORD_GAP_MIN, WORD_GAP_MAX)
    };
    let mut out: Vec<(char, usize)> = Vec::with_capacity(decoded.len());
    for (c, t) in decoded {
        if let Some(&(prev, at)) = out.last() {
            if !prev.is_whitespace() && !c.is_whitespace() && t - at > limit {
                out.push((' ', (at + t) / 2));
            }
        }
        out.push((c, t));
    }
    out
}

/// Whether Lookupper's recognizer counts `c` as CJK (REA `0x140950be0`): CJK radicals, symbols
/// and punctuation, kana, bopomofo, enclosed CJK, CJK compatibility and ideographs. Hangul is a
/// class of its own.
pub(crate) fn is_cjk_class(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x2EFF | 0x3000..=0x312F | 0x3190..=0x31BF | 0x31F0..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

/// Puts a space where a word the recognizer read in one piece has a wide blank gap in its ink, as
/// Lookupper does (REA `0x140950d00`; see `ink::word_cuts`). Lookupper leaves right-to-left lines
/// alone, and so are CJK runs here: its analyzer rejoins their characters with no space anyway.
/// `decoded` holds each character with its recognizer step, `step_w` is a step's width in crop
/// pixels and `across` the crop's height.
fn split_at_ink_gaps(decoded: Vec<(char, usize)>, ink: &ink::Ink, step_w: f32, across: f32) -> Vec<(char, usize)> {
    if is_rtl_text(decoded.iter().map(|(c, _)| *c)) {
        return decoded;
    }
    let centers: Vec<f32> = decoded.iter().map(|(_, t)| (*t as f32 + 0.5) * step_w).collect();
    // Words as Lookupper's recognizer makes them: runs of one class between spaces.
    let mut words: Vec<std::ops::Range<usize>> = Vec::new();
    let mut checked: Vec<bool> = Vec::new();
    for (i, (c, _)) in decoded.iter().enumerate() {
        if c.is_whitespace() {
            continue;
        }
        let cjk = is_cjk_class(*c);
        match words.last_mut() {
            Some(w) if w.end == i && is_cjk_class(decoded[w.start].0) == cjk => w.end = i + 1,
            _ => {
                words.push(i..i + 1);
                checked.push(!cjk);
            }
        }
    }
    let cuts = ink::word_cuts(ink, &centers, &words, across);
    // The line's gaps between words use every word; only the others are cut.
    let cuts: Vec<usize> = cuts.into_iter().filter(|&i| words.iter().zip(&checked).any(|(w, &ok)| ok && w.contains(&i))).collect();
    if cuts.is_empty() {
        return decoded;
    }
    let mut out = Vec::with_capacity(decoded.len() + cuts.len());
    for (i, d) in decoded.iter().enumerate() {
        if cuts.contains(&i) {
            out.push((' ', (decoded[i - 1].1 + d.1) / 2));
        }
        out.push(*d);
    }
    out
}

/// Width of a crop once resized to the recognizer's input height.
fn rec_width(cw: u32, ch: u32) -> u32 {
    ((cw as f32 * REC_HEIGHT as f32 / ch as f32).round() as u32).clamp(16, 3200)
}

fn box_rec_width(b: &TextBox) -> u32 {
    let (w, h) = b.size();
    // A vertical box is read on its side.
    if is_vertical_box(w, h) { rec_width(h as u32, w as u32) } else { rec_width(w as u32, h as u32) }
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

/// How a crop is turned before it is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turn {
    /// As it is: a horizontal line.
    Upright,
    /// A quarter left, so the top of a column is at the left: text read top to bottom, as
    /// PaddleOCR and Lookupper's recognizer read every vertical box.
    Left,
    /// A quarter right, so the bottom is at the left: text read bottom to top. Neither PaddleOCR's
    /// usual pipeline nor Lookupper tries it.
    Right,
}

/// A vertical box under this many times as tall as wide holds one or two characters; it may be a
/// narrow letter standing alone ("I", "1", "!"), so it is also read upright.

/// Below this mean confidence, a vertical box read top to bottom is also read bottom to top.
const TURN_CONFIDENT: f32 = 0.85;

/// The turns a box is read in first: upright for a horizontal box; top to bottom for a vertical
/// one, however short, as in Lookupper. `Turn::Right` comes later, only when needed
/// (`needs_right`).
fn first_turns(b: &TextBox) -> &'static [Turn] {
    let (w, h) = b.size();
    if !is_vertical_box(w, h) {
        &[Turn::Upright]
    } else {
        &[Turn::Left]
    }
}

/// Whether a vertical box whose best reading so far scored `best` is also read bottom to top.
fn needs_right(b: &TextBox, best: Option<f32>) -> bool {
    let (w, h) = b.size();
    is_vertical_box(w, h) && best.is_none_or(|score| score < TURN_CONFIDENT)
}

/// Whether a reading scoring `new` beats the best so far.
fn better(new: f32, best: Option<f32>) -> bool {
    best.is_none_or(|score| new > score)
}

/// A box's line as read in one turn, with the mean confidence of its characters.
struct Reading {
    line: OcrLine,
    score: f32,
}

/// Keeps `new` in `best` if it is the better reading of its box.
fn keep(best: &mut Option<Reading>, new: Reading) {
    if better(new.score, best.as_ref().map(|r| r.score)) {
        *best = Some(new);
    }
}

/// A text box cut out of the capture, turned as asked, and resized to the recognizer's height.
struct RecCrop {
    b: TextBox,
    /// Left edge, top, width and height of the crop in capture pixels; for a turned box (`b.quad`),
    /// 0, 0 and the size of its straightened crop.
    x: u32,
    y: u32,
    cw: u32,
    ch: u32,
    turn: Turn,
    /// Width after resizing.
    tw: u32,
    image: RgbaImage,
    /// The ink of the turned crop at capture resolution, to snap the word edges to.
    ink: Option<ink::Ink>,
}

fn rec_crop(img: &RgbaImage, b: TextBox, turn: Turn) -> Option<RecCrop> {
    let (iw, ih) = img.dimensions();
    let (x, y, cw, ch, mut crop) = if let Some(q) = &b.quad {
        // A turned box is straightened first, as Lookupper's recognizer does.
        let (w, h) = quad::crop_size(q);
        let (cw, ch) = (w.round().max(2.0) as u32, h.round().max(2.0) as u32);
        (0, 0, cw, ch, quad::warp(img, q, cw, ch))
    } else {
        let x = (b.x0.floor().max(0.0) as u32).min(iw.saturating_sub(1));
        let y = (b.y0.floor().max(0.0) as u32).min(ih.saturating_sub(1));
        let cw = ((b.x1.ceil() as u32).min(iw)).saturating_sub(x);
        let ch = ((b.y1.ceil() as u32).min(ih)).saturating_sub(y);
        if cw < 2 || ch < 2 {
            return None;
        }
        (x, y, cw, ch, image::imageops::crop_imm(img, x, y, cw, ch).to_image())
    };
    match turn {
        Turn::Upright => {}
        Turn::Left => crop = image::imageops::rotate270(&crop),
        Turn::Right => crop = image::imageops::rotate90(&crop),
    }
    let (w, h) = crop.dimensions();
    let lum: Vec<u8> = crop.pixels().map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8).collect();
    let ink = ink::measure(&lum, w as usize, h as usize);
    let tw = rec_width(w, h);
    let image = image::imageops::resize(&crop, tw, REC_HEIGHT, FilterType::Triangle);
    Some(RecCrop { b, x, y, cw, ch, turn, tw, image, ink })
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

/// A detected text box: the axis-aligned rectangle around it, and for a turned line its corners
/// (top left, top right, bottom right, bottom left; see quad.rs).
#[derive(Clone, Copy, Debug)]
pub struct TextBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub quad: Option<quad::Quad>,
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
    // The lines above and below, or the columns beside a vertical one.
    let (tw, th) = target.size();
    let vertical = is_vertical_box(tw, th);
    let across = |b: &TextBox| if vertical { (b.y0, b.y1, b.x0, b.x1) } else { (b.x0, b.x1, b.y0, b.y1) };
    let (t0, t1, s0, s1) = across(&target);
    let h = (s1 - s0).max(4.0);
    let near = |b: &TextBox| {
        let (a0, a1, b0, b1) = across(b);
        let overlap = a1.min(t1) - a0.max(t0);
        let gap = if b0 > s1 {
            b0 - s1
        } else if b1 < s0 {
            s0 - b1
        } else {
            0.0
        };
        overlap > 0.0 && gap <= 1.6 * h
    };
    let mut focus: Vec<TextBox> = boxes.iter().copied().filter(near).collect();
    // Match the original lookup's context selection, always keeping the pointed-at line.
    if vertical {
        // Columns read from the right.
        focus.sort_by(|a, b| b.x1.total_cmp(&a.x1));
    } else {
        focus.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    }
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

    /// Width along the line and height across it: the turned rectangle's for a turned box.
    pub fn size(&self) -> (f32, f32) {
        let (w, h) = match &self.quad {
            Some(q) => quad::crop_size(q),
            None => (self.x1 - self.x0, self.y1 - self.y0),
        };
        (w.max(1.0), h.max(1.0))
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
            // A turned line found in both bands: one rectangle in the frame of the first piece.
            let corners = |t: &TextBox| t.quad.unwrap_or([[t.x0, t.y0], [t.x1, t.y0], [t.x1, t.y1], [t.x0, t.y1]]);
            a.quad = match (a.quad, b.quad) {
                (None, None) => None,
                (Some(q), _) => Some(quad::merged(&q, &corners(&b))),
                (None, Some(q)) => Some(quad::merged(&q, &corners(&*a))),
            };
            a.x0 = a.x0.min(b.x0);
            a.y0 = a.y0.min(b.y0);
            a.x1 = a.x1.max(b.x1);
            a.y1 = a.y1.max(b.y1);
        } else {
            boxes.push(b);
        }
    }
}

/// The smallest rectangle around a text kernel's pixels (`pixels`, rows `miny..=maxy`), when it is
/// turned; `None` for an upright kernel, which keeps its plain box.
fn turned_kernel(pixels: &[(usize, usize)], miny: usize, maxy: usize) -> Option<quad::Rect> {
    // The hull needs only each row's ends: the outer corners of its first and last pixels.
    let mut ends = vec![(usize::MAX, 0usize); maxy - miny + 1];
    for &(x, y) in pixels {
        let e = &mut ends[y - miny];
        *e = (e.0.min(x), e.1.max(x));
    }
    let corners: Vec<quad::Point> = ends
        .iter()
        .enumerate()
        .filter(|(_, e)| e.0 != usize::MAX)
        .flat_map(|(i, &(a, b))| {
            let (y, a, b) = ((miny + i) as f32, a as f32, (b + 1) as f32);
            [[a, y], [b, y], [a, y + 1.0], [b, y + 1.0]]
        })
        .collect();
    quad::min_area_rect(corners).filter(quad::Rect::rotated)
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
    // On Linux the session's worker threads take this thread's priority (platform/priority.rs).
    crate::platform::priority::raise_this_thread();
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
    // Dawn's and the session's threads take this thread's priority (platform/priority.rs).
    crate::platform::priority::raise_this_thread();
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

    fn det_max_side(&self) -> f32 {
        if self.det_on_gpu() { DET_MAX_SIDE_GPU } else { DET_MAX_SIDE_CPU }
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
            quad: None,
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
        let scale = (self.det_max_side() / w.max(h) as f32).min(1.0);
        let pad = DET_PADDING;
        // The padded size is a multiple of 32, so the bands need no further resize.
        let round32 = |v: f32| -> u32 { (((v / 32.0).round() as u32) * 32).max(32) };
        let pw = round32(w as f32 * scale + (2 * pad) as f32);
        let ph = round32(h as f32 * scale + (2 * pad) as f32);
        let (rw, rh) = (pw - 2 * pad, ph - 2 * pad);
        let mut resized = RgbaImage::from_pixel(pw, ph, image::Rgba([255, 255, 255, 255]));
        image::imageops::replace(&mut resized, &image::imageops::resize(img, rw, rh, FilterType::Triangle), pad as i64, pad as i64);
        let (rw, rh, pad) = (rw as f32, rh as f32, pad as f32);
        let mut boxes = Vec::new();
        let mut y = 0;
        loop {
            let height = DET_BAND_HEIGHT.min(ph - y);
            let band = image::imageops::crop_imm(&resized, 0, y, pw, height).to_image();
            let mut found = self.detect_band(&band)?;
            for b in &mut found {
                b.y0 += y as f32;
                b.y1 += y as f32;
                if let Some(q) = &mut b.quad {
                    for p in q.iter_mut() {
                        p[1] += y as f32;
                    }
                }
            }
            merge_band_boxes(&mut boxes, found);
            if y + height == ph {
                break;
            }
            y += DET_BAND_HEIGHT - DET_BAND_OVERLAP;
        }
        // The rest of OCR and the overlay continue to use captured-image coordinates.
        let sx = w as f32 / rw;
        let sy = h as f32 / rh;
        for b in &mut boxes {
            b.x0 = (b.x0 - pad).clamp(0.0, rw) * sx;
            b.x1 = (b.x1 - pad).clamp(0.0, rw) * sx;
            b.y0 = (b.y0 - pad).clamp(0.0, rh) * sy;
            b.y1 = (b.y1 - pad).clamp(0.0, rh) * sy;
            if let Some(q) = &mut b.quad {
                for p in q.iter_mut() {
                    *p = [(p[0] - pad) * sx, (p[1] - pad) * sy];
                }
            }
        }
        boxes.retain(|b| b.x1 - b.x0 >= 1.0 && b.y1 - b.y0 >= 1.0);
        Ok(boxes)
    }

    fn detect_band(&self, img: &RgbaImage) -> Result<Vec<TextBox>> {
        let (w, h) = img.dimensions();
        if w < 8 || h < 8 {
            return Ok(Vec::new());
        }
        let longest = w.max(h) as f32;
        let max_side = self.det_max_side() + (2 * DET_PADDING) as f32;
        let scale = if longest > max_side {
            max_side / longest
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
        let mut pixels: Vec<(usize, usize)> = Vec::new();
        for start in 0..mh * mw {
            if visited[start] || prob[start] <= DET_THRESH {
                continue;
            }
            visited[start] = true;
            stack.push(start);
            let (mut minx, mut miny, mut maxx, mut maxy) = (usize::MAX, usize::MAX, 0usize, 0usize);
            let mut sum = 0f32;
            let mut count = 0usize;
            pixels.clear();
            while let Some(i) = stack.pop() {
                let (x, y) = (i % mw, i / mw);
                pixels.push((x, y));
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
            if count == 0 || sum / (count as f32) < box_thresh {
                continue;
            }
            // As Lookupper, the kernel's smallest rectangle, which is turned with a turned line.
            if let Some(r) = turned_kernel(&pixels, miny, maxy) {
                if 2.0 * r.half_w.min(r.half_h) < 3.0 {
                    continue;
                }
                let (kw, kh) = (2.0 * r.half_w, 2.0 * r.half_h);
                let q = r.grown(kw * kh * UNCLIP_RATIO / (2.0 * (kw + kh))).corners().map(|p| [p[0] * sx, p[1] * sy]);
                let (x0, y0, x1, y1) = quad::bounds(&q);
                let (x0, y0, x1, y1) = (x0.max(0.0), y0.max(0.0), x1.min(w as f32), y1.min(h as f32));
                if x1 - x0 >= 4.0 && y1 - y0 >= 4.0 {
                    boxes.push(TextBox { x0, y0, x1, y1, quad: Some(q) });
                }
                continue;
            }
            if bw < 3.0 || bh < 3.0 {
                continue;
            }
            // DB predicts shrunk text kernels; grow them back by area * ratio / perimeter.
            let d = bw * bh * UNCLIP_RATIO / (2.0 * (bw + bh));
            let x0 = ((minx as f32 - d) * sx).max(0.0);
            let y0 = ((miny as f32 - d) * sy).max(0.0);
            let x1 = (((maxx + 1) as f32 + d) * sx).min(w as f32);
            let y1 = (((maxy + 1) as f32 + d) * sy).min(h as f32);
            if x1 - x0 >= 4.0 && y1 - y0 >= 4.0 {
                boxes.push(TextBox { x0, y0, x1, y1, quad: None });
            }
        }
        Ok(boxes)
    }

    fn recognize_box(&self, img: &RgbaImage, b: TextBox) -> Result<Option<OcrLine>> {
        let mut best: Option<Reading> = None;
        for &turn in first_turns(&b) {
            self.read_turn(img, b, turn, &mut best)?;
        }
        if needs_right(&b, best.as_ref().map(|r| r.score)) {
            self.read_turn(img, b, Turn::Right, &mut best)?;
        }
        Ok(best.map(|r| r.line))
    }

    /// Reads box `b` turned `turn`, and keeps the reading in `best` if it is better.
    fn read_turn(&self, img: &RgbaImage, b: TextBox, turn: Turn, best: &mut Option<Reading>) -> Result<()> {
        let Some(crop) = rec_crop(img, b, turn) else { return Ok(()) };
        let pw = crop.tw as usize;
        let mut data = vec![0f32; 3 * REC_HEIGHT as usize * pw];
        fill_rec_input(&mut data, &crop.image, pw);
        let (steps, classes, probs) = self.run_rec(data, 1, pw)?;
        if let Some(reading) = self.decode_line(&crop, &probs, steps, classes, pw) {
            keep(best, reading);
        }
        Ok(())
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
        let mut best: Vec<Option<Reading>> = boxes.iter().map(|_| None).collect();
        let first: Vec<(usize, Turn)> =
            boxes.iter().enumerate().flat_map(|(i, b)| first_turns(b).iter().map(move |&t| (i, t))).collect();
        self.read_batch(img, boxes, &first, &mut best)?;
        // Vertical boxes not read well top to bottom, bottom to top.
        let right: Vec<(usize, Turn)> = boxes
            .iter()
            .enumerate()
            .filter(|&(i, b)| needs_right(b, best[i].as_ref().map(|r| r.score)))
            .map(|(i, _)| (i, Turn::Right))
            .collect();
        if !right.is_empty() {
            self.read_batch(img, boxes, &right, &mut best)?;
        }
        Ok(best.into_iter().flatten().map(|r| r.line).collect())
    }

    /// Reads `boxes[i]` turned `t` for every `(i, t)` of `reads` in one recognizer run, and keeps
    /// in `best[i]` the better reading.
    fn read_batch(&self, img: &RgbaImage, boxes: &[TextBox], reads: &[(usize, Turn)], best: &mut [Option<Reading>]) -> Result<()> {
        let crops: Vec<(usize, RecCrop)> = reads.iter().filter_map(|&(i, t)| rec_crop(img, boxes[i], t).map(|c| (i, c))).collect();
        if crops.is_empty() {
            return Ok(());
        }
        let pw = crops.iter().map(|(_, c)| c.tw).max().unwrap_or(16) as usize;
        let slot = 3 * REC_HEIGHT as usize * pw;
        let mut data = vec![0f32; crops.len() * slot];
        for (k, (_, c)) in crops.iter().enumerate() {
            fill_rec_input(&mut data[k * slot..(k + 1) * slot], &c.image, pw);
        }
        let (steps, classes, probs) = self.run_rec(data, crops.len(), pw)?;
        let per_item = steps * classes;
        if probs.len() < per_item * crops.len() {
            bail!("recognizer returned {} values for {} lines", probs.len(), crops.len());
        }
        for (k, (i, c)) in crops.iter().enumerate() {
            if let Some(reading) = self.decode_line(c, &probs[k * per_item..(k + 1) * per_item], steps, classes, pw) {
                keep(&mut best[*i], reading);
            }
        }
        Ok(())
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

    /// CTC-decodes one crop's output rows into a line with per-character spans, and the mean
    /// confidence of its characters.
    fn decode_line(&self, crop: &RecCrop, probs: &[f32], steps: usize, classes: usize, padded_w: usize) -> Option<Reading> {
        if steps == 0 || classes == 0 {
            return None;
        }
        let b = crop.b;
        // Where the line starts and how long it is in capture pixels, and its ends: across, or
        // down a turned crop (from the top for `Turn::Left`; `Turn::Right` is mirrored below).
        let (start, len, lo, hi) = match (crop.turn, b.quad.is_some()) {
            // A straightened crop: positions along it, mapped onto the capture below.
            (Turn::Upright, true) => (0, crop.cw, 0.0, crop.cw as f32),
            (_, true) => (0, crop.ch, 0.0, crop.ch as f32),
            (Turn::Upright, false) => (crop.x, crop.cw, b.x0, b.x1),
            (_, false) => (crop.y, crop.ch, b.y0, b.y1),
        };
        // In a batch, the steps past this crop's own width only see padding.
        let content_steps = steps as f32 * crop.tw as f32 / padded_w as f32;
        let valid = (content_steps.ceil() as usize).clamp(1, steps);
        // Index 0 is the CTC blank; a trailing extra class is the space character.
        let has_space = classes == self.dict.len() + 2;

        let mut decoded: Vec<(char, usize)> = Vec::new();
        let (mut sure, mut read) = (0f32, 0usize);
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
                sure += best_p;
                read += 1;
            }
            prev = best;
        }
        let decoded = split_wide_gaps(decoded);
        if decoded.iter().all(|(c, _)| c.is_whitespace()) {
            return None;
        }

        // Each CTC step covers an equal slice of the padded input, so of the crop's length.
        let step_w = len as f32 / content_steps;
        // The crop's height across the line, in capture pixels.
        let across = (if crop.turn == Turn::Upright { crop.ch } else { crop.cw }) as f32;
        let decoded = match &crop.ink {
            Some(ink) => split_at_ink_gaps(decoded, ink, step_w, across),
            None => decoded,
        };
        let centers: Vec<f32> = decoded
            .iter()
            .map(|(_, t)| start as f32 + (*t as f32 + 0.5) * step_w)
            .collect();
        let blank: Vec<bool> = decoded.iter().map(|(c, _)| c.is_whitespace()).collect();
        // Word edges snap to the ink of the line, as in Lookupper.
        let mut spans = ink::spans(&centers, &blank, crop.ink.as_ref(), start as f32, lo, hi, step_w);
        // A turned line is given in its own frame, where it stands upright (see `Tilt`): the
        // straightened crop's positions map onto that frame's rectangle.
        let tilt = b.quad.as_ref().map(|q| {
            let angle = (q[1][1] - q[0][1]).atan2(q[1][0] - q[0][0]);
            let frame = Tilt { angle, rect: (0.0, 0.0, 0.0, 0.0) };
            let (x0, y0) = frame.to_frame(q[0][0], q[0][1]);
            let (x1, _) = frame.to_frame(q[1][0], q[1][1]);
            let (_, y1) = frame.to_frame(q[3][0], q[3][1]);
            Tilt { angle, rect: (x0, y0, x1, y1) }
        });
        if let Some(t) = &tilt {
            let (x0, y0, x1, y1) = t.rect;
            let at = |s: f32| match crop.turn {
                Turn::Upright => x0 + (x1 - x0) * s / crop.cw as f32,
                Turn::Left => y0 + (y1 - y0) * s / crop.ch as f32,
                Turn::Right => y1 - (y1 - y0) * s / crop.ch as f32,
            };
            for span in &mut spans {
                let (a, c) = (at(span.0), at(span.1));
                *span = (a.min(c), a.max(c));
            }
        } else if crop.turn == Turn::Right {
            // The crop's left was the bottom of the line: its characters go up.
            let mirror = 2.0 * crop.y as f32 + crop.ch as f32;
            for span in &mut spans {
                *span = (mirror - span.1, mirror - span.0);
            }
        }
        let mut chars: Vec<OcrChar> = decoded.iter().zip(spans).map(|((c, _), (x0, x1))| OcrChar { ch: *c, x0, x1 }).collect();
        // The recognizer reads right-to-left text as drawn; like Lookupper, turn it to reading order.
        if crop.turn == Turn::Upright && is_rtl_text(chars.iter().map(|c| c.ch)) {
            chars = reverse_rtl(&chars);
        }
        let text: String = chars.iter().map(|c| c.ch).collect();
        // A turned crop's rows are the columns of the line: from its right side when turned left,
        // from its left side when turned right.
        let ink_y = crop.ink.as_ref().map(|ink| {
            let (r0, r1) = (ink.y0 as f32, ink.y1 as f32);
            if let Some(t) = &tilt {
                // In the turned line's frame, from the straightened crop's rows.
                let (x0, y0, x1, y1) = t.rect;
                let across = |k: f32| match crop.turn {
                    Turn::Upright => y0 + (y1 - y0) * k / crop.ch as f32,
                    Turn::Left => x1 - (x1 - x0) * k / crop.cw as f32,
                    Turn::Right => x0 + (x1 - x0) * k / crop.cw as f32,
                };
                let (a, c) = (across(r0), across(r1));
                return (a.min(c), a.max(c));
            }
            match crop.turn {
                Turn::Upright => (crop.y as f32 + r0, crop.y as f32 + r1),
                Turn::Left => {
                    let right = (crop.x + crop.cw) as f32;
                    (right - r1, right - r0)
                }
                Turn::Right => (crop.x as f32 + r0, crop.x as f32 + r1),
            }
        });
        let flow = match crop.turn {
            Turn::Upright => Flow::Across,
            Turn::Left => Flow::Down,
            Turn::Right => Flow::Up,
        };
        let line = OcrLine { text, x0: b.x0, y0: b.y0, x1: b.x1, y1: b.y1, chars, ink_y, flow, tilt, look: Default::default() };
        Some(Reading { line, score: sure / read.max(1) as f32 })
    }
}
