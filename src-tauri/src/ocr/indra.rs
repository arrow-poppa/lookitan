// IndraOCR's recognizers (datalab-to/indra, code Apache-2.0; weights under the modified AI Pubs Open
// RAIL-M licence) reading the boxes of PP-OCR's detector, at the user's request on 2026-10-28: an
// experiment that keeps PP-OCR's detector to judge Indra's recognition alone. The weights are
// downloaded only after the user accepts their licence in Configurações › OCR, never bundled.
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

use super::paddle::{cpu_session, gpu_session, BoxReader, Glyph, InferenceSession};
use crate::settings::GpuPreference;

const LINE_HEIGHT: u32 = 32;
const PAD_X: f32 = 0.3;
const PAD_Y: f32 = 0.2;
const WIDTH_MULTIPLE: usize = 8;
const MAX_WIDTH: u32 = 4096;
const MIN_CONFIDENCE: f32 = 0.5;
/// Lines per model run, of similar width (as indra batches them).
const BATCH: usize = 16;

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
        let grey: Vec<u8> = crop.pixels().map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8).collect();
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
        if glyphs.is_empty() || sum / read.max(1) as f32 < MIN_CONFIDENCE {
            return Vec::new();
        }
        glyphs
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
