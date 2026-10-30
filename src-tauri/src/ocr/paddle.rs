// PP-OCRv6 / PP-OCRv5 text detection (DB) and recognition (CTC) on ONNX Runtime.
//
// Engine choice mirrors Lookupper: try the `small` detector on the GPU (DirectML on Windows, WebGPU
// on Linux) first and fall back to the `tiny` detector on the CPU with full graph optimization.
// In the manual mode (Configurações › OCR, `EnginePlan::manual`) the user picks the detector and
// where each model runs; a model that fails on the GPU then goes on, the same model, on the CPU.
// Normalization comes from the official PaddlePaddle `inference.yml` of each model; the detector's
// border, binarization, unclip ratio and image size are Lookupper's.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use image::{Rgba, RgbaImage};
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use ort::session::Session;
use ort::value::Tensor;
use parking_lot::Mutex;

use super::{quad, ink, is_rtl_text, is_vertical_box, reverse_rtl, Flow, OcrChar, OcrLine, Tilt};
use crate::settings::GpuPreference;

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
    fn the_automatic_plan_is_lookuppers_and_the_manual_one_keeps_its_detector_on_the_cpu() {
        let auto = EnginePlan::automatic(true, None);
        assert_eq!(auto.det_models(), ("det_small", "det_tiny"), "small on the GPU, tiny when it falls back");
        assert!(auto.det_gpu && auto.rec_gpu);
        let off = EnginePlan::automatic(false, None);
        assert!(!off.det_gpu && !off.rec_gpu);
        let manual = EnginePlan { manual: true, det: "det_small", det_gpu: false, rec_gpu: true, gpu_choice: None };
        assert_eq!(manual.det_models(), ("det_small", "det_small"), "the chosen detector on either device");
        let tiny = EnginePlan { det: "det_tiny", det_gpu: true, ..manual };
        assert_eq!(tiny.det_models(), ("det_tiny", "det_tiny"));
    }

    #[test]
    fn the_box_threshold_follows_the_detector_and_the_image_size_its_device() {
        assert_eq!(box_thresh("det_small"), 0.45);
        assert_eq!(box_thresh("det_tiny"), 0.4);
        assert_eq!(det_max_side(true), DET_MAX_SIDE_GPU);
        assert_eq!(det_max_side(false), DET_MAX_SIDE_CPU);
    }

    #[test]
    fn pp_ocrv6_manga_s_recognizer_reads_with_its_width_rule_from_its_own_folder() {
        assert_eq!(db_params("det_small"), (DET_THRESH, 0.45, UNCLIP_RATIO));
        // Its wide-line rule: at most 640, or 2400 past 2000.
        assert_eq!(manga_rec_width(480, 48), 480);
        assert_eq!(manga_rec_width(1200, 48), 640, "squeezed");
        assert_eq!(manga_rec_width(2100, 48), 2100);
        assert_eq!(manga_rec_width(3000, 48), 2400);
        assert_eq!(rec_width(1200, 48), 1200, "the others keep their width");
        let (models, manga) = (Path::new("models"), Path::new("manga"));
        assert_eq!(model_path(models, Some(manga), MANGA_REC), manga.join("manga_rec_v0.2.onnx"));
        assert_eq!(model_path(models, Some(manga), "det_tiny"), models.join("det_tiny.onnx"));
    }

    /// PP-OCRv6 Manga's recognizer on det_small's boxes of the Japanese fixture (dark on white and
    /// light on dark blue; game text, not its manga), on the CPU, and on Windows also asked for the
    /// GPU (its detector was removed on 2026-10-30; comic-text-detector finds its lines in the app).
    /// Needs the fetched ONNX Runtime and models, and PP-OCRv6 Manga's in `LOOKITAN_MANGA` (CI
    /// downloads them; the test is skipped without them).
    #[test]
    #[ignore]
    fn real_manga_reads_the_japanese_fixture() {
        use unicode_normalization::UnicodeNormalization;
        let Some(dir) = std::env::var_os("LOOKITAN_MANGA").map(PathBuf::from) else {
            eprintln!("LOOKITAN_MANGA is not set; skipped");
            return;
        };
        let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let library = resources.join("ort").join(if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" });
        ort::init_from(&library).expect("ONNX Runtime").with_name("lookitan-test").commit();
        let want: Vec<char> = "今日はいい天気ですね。散歩に行きましょう！「本当に？」と彼女は小さな声で聞いた。セーブデータを読み込んでいます…東京駅から新幹線で大阪へ向かう。HPが足りない！回復アイテムを使いますか？".repeat(2).nfkc().filter(|c| !c.is_whitespace()).collect();
        let img = image::load_from_memory(include_bytes!("fixtures/ja_lines.png")).unwrap().to_rgba8();
        let devices: &[bool] = if cfg!(windows) { &[false, true] } else { &[false] };
        for &gpu in devices {
            let plan = EnginePlan { manual: true, det: "det_small", det_gpu: gpu, rec_gpu: gpu, gpu_choice: None };
            let ocr = PaddleOcr::load(&resources.join("models"), Some(&dir), PaddleUse { det: true, rec: Some(MANGA_REC), plan }).unwrap();
            let boxes = ocr.detect_boxes(&img).unwrap();
            let lines = ocr.recognize_boxes(&img, &boxes);
            let got: Vec<char> = lines.iter().map(|l| l.text.as_str()).collect::<String>().nfkc().filter(|c| !c.is_whitespace()).collect();
            let cer = edits(&got.iter().collect::<String>(), &want.iter().collect::<String>()) as f32 / want.len() as f32;
            eprintln!("PP-OCRv6 Manga, asked {}: det on {}, rec on {}: {} boxes, CER {:.1}%
{}", if gpu { "GPU" } else { "CPU" },
                if ocr.det_on_gpu() { "GPU" } else { "CPU" }, if ocr.rec_on_gpu() { "GPU" } else { "CPU" }, boxes.len(), cer * 100.0,
                lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("
"));
            assert!(cer <= 0.10, "CER {cer}");
        }
        // A detector-only load of the same detector shares the full one's session.
        let plan = EnginePlan { manual: true, det: "det_small", det_gpu: false, rec_gpu: false, gpu_choice: None };
        let full = Arc::new(PaddleOcr::load(&resources.join("models"), Some(&dir), PaddleUse { det: true, rec: Some(MANGA_REC), plan }).unwrap());
        let det_only = PaddleOcr::load_sharing(&resources.join("models"), Some(&dir), PaddleUse { det: true, rec: None, plan }, &[full.clone()]).unwrap();
        assert!(Arc::ptr_eq(&full.det, &det_only.det), "one detector session");
        assert_eq!(det_only.detect_boxes(&img).unwrap().len(), full.detect_boxes(&img).unwrap().len());
        let other = PaddleOcr::load_sharing(&resources.join("models"), Some(&dir), PaddleUse { det: true, rec: None, plan: EnginePlan { det: "det_tiny", ..plan } }, &[full.clone()]).unwrap();
        assert!(!Arc::ptr_eq(&full.det, &other.det), "another detector is its own");
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
        let batches = plan_batches(&boxes, |w| w);
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
        // Tiny RU's padding counts against the budget.
        let padded = |w| rec_padded_width(crate::ocr::RU_TINY, w);
        for batch in plan_batches(&boxes, padded) {
            let widest = padded(batch.iter().map(box_rec_width).max().unwrap());
            assert!(batch.len() == 1 || batch.len() as u32 * widest <= REC_BATCH_COLUMNS);
        }
    }

    #[test]
    fn only_tiny_ru_pads_its_runs_as_its_model_card_does() {
        for model in ["rec_tiny", "rec_small", "rec_cyrillic", "rec_arabic", "rec_el", "rec_korean"] {
            assert_eq!(rec_padded_width(model, 300), 300, "{model} reads tight");
        }
        assert_eq!(rec_padded_width(crate::ocr::RU_TINY, 16), 640, "at least 640 columns");
        assert_eq!(rec_padded_width(crate::ocr::RU_TINY, 427), 641, "half again as wide, rounded up");
        assert_eq!(rec_padded_width(crate::ocr::RU_TINY, 3200), 4800);
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
        let left = rec_crop(&img, b, Turn::Left, true, rec_width).unwrap();
        assert_eq!(left.image.height(), REC_HEIGHT);
        assert_eq!(left.tw, rec_width(100, 20), "as long as the column is tall");
        assert_eq!(box_rec_width(&b), left.tw);
        let ink = left.ink.unwrap();
        // Turned left: the top of the column is at the left, and its right side at the top.
        assert!(ink.cols[..30].iter().all(|&c| c) && !ink.cols[40..].iter().any(|&c| c), "{:?}", ink.cols);
        assert_eq!((ink.y0, ink.y1), (0, 8));
        // Turned right: the top is at the right, and the right side at the bottom.
        let ink = rec_crop(&img, b, Turn::Right, true, rec_width).unwrap().ink.unwrap();
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

    /// Edits between `a` and `b`, in characters.
    struct FakeReader(Vec<Glyph>);

    impl BoxReader for FakeReader {
        fn read(&self, crops: &[(&RgbaImage, bool)]) -> Result<Vec<Vec<Glyph>>> {
            Ok(crops.iter().map(|_| self.0.clone()).collect())
        }
    }

    #[test]
    fn another_recognizer_s_characters_get_their_spans_and_word_gaps_on_the_capture() {
        let mut img = RgbaImage::from_pixel(260, 40, Rgba([255, 255, 255, 255]));
        let blocks = [(20u32, 44u32), (50, 74), (200, 224)];
        for (x0, x1) in blocks {
            for x in x0..x1 {
                for y in 10..30 {
                    img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
                }
            }
        }
        let b = TextBox { x0: 10.0, y0: 5.0, x1: 240.0, y1: 35.0, quad: None };
        // Along the crop, which starts at x = 10.
        let glyph = |ch, from: f32| Glyph { ch, from, to: from + 24.0, score: 0.9 };
        let reader = FakeReader(vec![glyph('日', 10.0), glyph('本', 40.0), glyph('語', 190.0)]);
        let lines = recognize_boxes_with(&img, &[b], &reader);
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        // The wide gap before 語 is a space, as in PP-OCR's lines.
        assert_eq!(line.text, "日本 語");
        assert_eq!(line.flow, Flow::Across);
        let glyphs: Vec<&OcrChar> = line.chars.iter().filter(|c| !c.ch.is_whitespace()).collect();
        for (c, (x0, x1)) in glyphs.iter().zip(blocks) {
            let mid = (c.x0 + c.x1) / 2.0;
            assert!(mid > x0 as f32 && mid < x1 as f32, "{} at {}..{}", c.ch, c.x0, c.x1);
        }
        // A reader that reads nothing drops the box.
        assert!(recognize_boxes_with(&img, &[b], &FakeReader(Vec::new())).is_empty());
    }

    fn edits(a: &str, b: &str) -> usize {
        let b: Vec<char> = b.chars().collect();
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut prev = row[0];
            row[0] = i + 1;
            for (j, &cb) in b.iter().enumerate() {
                let next = (row[j + 1] + 1).min(row[j] + 1).min(prev + usize::from(ca != cb));
                prev = row[j + 1];
                row[j + 1] = next;
            }
        }
        row[b.len()]
    }

    /// Three Russian lines (fixtures/ru_lines.png, Arial 30 px, black on white) read with the
    /// recognizers and detectors the build ships, through ONNX Runtime. Needs `fetch-assets.py`.
    #[test]
    #[ignore = "needs the fetched models and ONNX Runtime; CI runs it"]
    fn real_russian_recognizers_read_the_fixture() {
        const LINES: [&str; 3] = ["Съешь же ещё этих мягких французских булок", "ВСЁ ЕЩЁ ЗДЕСЬ, а ёлка — в лесу.", "Шелестят листья!"];
        // Each line's ink, 6 px wider on every side, as the detector's unclipped boxes are.
        const BOXES: [(f32, f32, f32, f32); 3] = [(18.0, 21.0, 689.0, 62.0), (18.0, 73.0, 525.0, 118.0), (18.0, 134.0, 281.0, 168.0)];
        let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let library = resources.join("ort").join(if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" });
        ort::init_from(&library).expect("ONNX Runtime").with_name("lookitan-test").commit();
        let models = resources.join("models");
        let img = image::load_from_memory(include_bytes!("fixtures/ru_lines.png")).unwrap().to_rgba8();
        let boxes: Vec<TextBox> = BOXES.iter().map(|&(x0, y0, x1, y1)| TextBox { x0, y0, x1, y1, quad: None }).collect();
        let cer = |read: &[String]| {
            let (got, want) = (read.join("\n"), LINES.join("\n"));
            edits(&got, &want) as f32 / want.chars().count() as f32
        };

        // Tiny RU decodes with PP-OCRv5 Cyrillic's character list: ё and е, Ё and Е apart, no space.
        let dict: Vec<String> = serde_json::from_slice(&std::fs::read(models.join("rec_ru_tiny.dict.json")).unwrap()).unwrap();
        let cyrillic: Vec<String> = serde_json::from_slice(&std::fs::read(models.join("rec_cyrillic.dict.json")).unwrap()).unwrap();
        assert_eq!(dict, cyrillic);
        for c in "абвгдеёжзийклмнопрстуфхцчшщъыьэюяАБВГДЕЁЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯ.,;:!?-—«»()№".chars() {
            assert!(dict.iter().any(|d| *d == c.to_string()), "{c} is missing");
        }
        assert!(!dict.iter().any(|d| d == " " || d.is_empty()), "the space is the extra last class, not in the list");

        // On the Linux runner, which has no Vulkan, a WebGPU session that failed to start leaves the
        // test process to crash on exit after passing; the GPU and its CPU fallback are tried on
        // Windows (DirectML) only.
        let devices: &[bool] = if cfg!(windows) { &[false, true] } else { &[false] };
        for rec in ["rec_cyrillic", crate::ocr::RU_TINY] {
            for det in ["det_tiny", "det_small"] {
                for &gpu in devices {
                    let plan = EnginePlan { manual: true, det, det_gpu: gpu, rec_gpu: gpu, gpu_choice: None };
                    let ocr = PaddleOcr::new(&models, rec, plan).unwrap();
                    let at = format!("{rec}, {det}, asked {}: det on {}, rec on {}", if gpu { "GPU" } else { "CPU" },
                        if ocr.det_on_gpu() { "GPU" } else { "CPU" }, if ocr.rec_on_gpu() { "GPU" } else { "CPU" });
                    assert_eq!(ocr.rec_model, rec);
                    assert_eq!(ocr.det_model(), det, "the manual detector, on either device");

                    // The output has a class per character, the CTC blank and the space.
                    let pw = rec_padded_width(rec, 64) as usize;
                    let (_, classes, _) = ocr.run_rec(vec![0f32; 3 * REC_HEIGHT as usize * pw], 1, pw).unwrap();
                    assert_eq!(classes, dict.len() + 2, "{at}");

                    // Recognition only: one box at a time, then the three in one batch.
                    let single: Vec<String> =
                        boxes.iter().map(|&b| ocr.recognize_box(&img, b).unwrap().map(|l| l.text).unwrap_or_default()).collect();
                    let batched: Vec<String> = ocr.recognize_boxes_batched(&img, &boxes).into_iter().map(|l| l.text).collect();
                    // Detection and recognition.
                    let found = ocr.detect_boxes(&img).unwrap();
                    let full: Vec<String> = ocr.recognize_boxes(&img, &found).into_iter().map(|l| l.text).collect();
                    println!("{at}\n  single  {single:?}\n  batched {batched:?}\n  full    {full:?}");

                    assert_eq!(batched.len(), 3, "{at}");
                    if rec == "rec_cyrillic" {
                        assert_eq!(single, LINES, "{at}: PP-OCRv5 Cyrillic reads the lines as written, ё included");
                        assert_eq!(batched, LINES, "{at}");
                        assert!(cer(&full) <= 0.03, "{at}: {full:?}");
                    } else {
                        // Experimental: it tends to read ё as е and to run words together.
                        assert_eq!(single[0], LINES[0], "{at}");
                        assert_eq!(single[2], LINES[2], "{at}");
                        assert!(cer(&single) <= 0.10 && cer(&batched) <= 0.10, "{at}");
                        assert!(cer(&full) <= 0.15, "{at}: {full:?}");
                    }
                }
            }
        }
    }
}

/// Binarization of the probability map, as Lookupper (76 / 255) and PaddleOCR's default.
const DET_THRESH: f32 = 0.3;
const UNCLIP_RATIO: f32 = 1.35;
/// PP-OCRv6 Manga's recognizer (`models::MANGA`, downloaded): read from its own folder, with
/// rec_small's character list (the same file).
pub const MANGA_REC: &str = "manga_rec_v0.2";
/// White border around the detector's image, in detector pixels, as Lookupper's WhitePadding:
/// text touching the edge of a capture is still found whole.
const DET_PADDING: u32 = 20;
/// Longest side of the image the detector sees, as in Lookupper: the GPU detector gets more
/// pixels, so small text (dense CJK above all) is not shrunk as much.
const DET_MAX_SIDE_GPU: f32 = 1920.0;
const DET_MAX_SIDE_CPU: f32 = 1280.0;
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

/// The input width a recognizer run pads `widest` (its widest crop) to. PP-OCRv6 Tiny RU reads
/// worse on tight crops (words run together, short ones go astray): its model card pads every run
/// to at least 640 columns and half again as wide as its widest line. The other recognizers read
/// best tight, as PaddleOCR feeds them.
pub fn rec_padded_width(rec_model: &str, widest: u32) -> u32 {
    if rec_model == crate::ocr::RU_TINY { widest.saturating_mul(3).div_ceil(2).max(640) } else { widest }
}

/// Width of a crop once resized to the recognizer's input height.
fn rec_width(cw: u32, ch: u32) -> u32 {
    ((cw as f32 * REC_HEIGHT as f32 / ch as f32).round() as u32).clamp(16, 3200)
}

/// PP-OCRv6 Manga's recognizer width (its example's "wide-line rule"): at most 640, or 2400 for a
/// line naturally over 2000 wide; a longer line is squeezed.
fn manga_rec_width(cw: u32, ch: u32) -> u32 {
    let natural = (cw as f32 * REC_HEIGHT as f32 / ch as f32).round() as u32;
    natural.clamp(16, if natural > 2000 { 2400 } else { 640 })
}

fn box_rec_width(b: &TextBox) -> u32 {
    let (w, h) = b.size();
    // A vertical box is read on its side.
    if is_vertical_box(w, h) { rec_width(h as u32, w as u32) } else { rec_width(w as u32, h as u32) }
}

/// Groups boxes of similar width for batched recognition, narrowest first, so little of each
/// batch is padding. `padded` gives a run's input width from its widest crop
/// (`rec_padded_width`), which the column budget counts.
pub fn plan_batches(boxes: &[TextBox], padded: impl Fn(u32) -> u32) -> Vec<Vec<TextBox>> {
    let mut sorted = boxes.to_vec();
    sorted.sort_by_key(box_rec_width);
    let mut out = Vec::new();
    let mut batch: Vec<TextBox> = Vec::new();
    for b in sorted {
        // Sorted ascending, so this box sets the batch's padded width.
        let widest = padded(box_rec_width(&b));
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

/// With `resize` off, `image` is the turned crop at capture resolution (for a `BoxReader`).
/// `b`'s crop, turned; resized to the recognizer's height with `width` when `resize`.
fn rec_crop(img: &RgbaImage, b: TextBox, turn: Turn, resize: bool, width: fn(u32, u32) -> u32) -> Option<RecCrop> {
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
    if !resize {
        return Some(RecCrop { b, x, y, cw, ch, turn, tw: w, image: crop, ink });
    }
    let tw = width(w, h);
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

/// Which detector runs where, and where the recognizer runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnginePlan {
    /// Off: Lookupper's choice, `det_small` on the GPU (falling back to `det_tiny` on the CPU) and
    /// the recognizer wherever the detector ended up. On: the user's (Configurações › OCR).
    pub manual: bool,
    /// The detector in the manual mode: "det_small" or "det_tiny".
    pub det: &'static str,
    pub det_gpu: bool,
    /// Manual mode only; the automatic plan puts the recognizer where the detector is.
    pub rec_gpu: bool,
    pub gpu_choice: Option<GpuPreference>,
}

/// What a pass loads of PP-OCR (`Settings::paddle_use`): its detector, its recognizer `rec` (the
/// model's name), or both, on a plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaddleUse {
    pub det: bool,
    pub rec: Option<&'static str>,
    pub plan: EnginePlan,
}

impl EnginePlan {
    pub fn automatic(gpu: bool, gpu_choice: Option<GpuPreference>) -> Self {
        Self { manual: false, det: "det_small", det_gpu: gpu, rec_gpu: gpu, gpu_choice }
    }

    /// The detector tried on the GPU, and the one on the CPU (from the start or after a GPU failure).
    fn det_models(&self) -> (&'static str, &'static str) {
        if self.manual { (self.det, self.det) } else { ("det_small", "det_tiny") }
    }
}

/// The detector's box score threshold: the small model's maps are sharper (Lookupper's values,
/// which it pairs with the GPU and the CPU).
fn box_thresh(det_model: &str) -> f32 {
    match det_model {
        "det_small" => 0.45,
        _ => 0.4,
    }
}

/// The DB post-processing of a detector: binarization threshold, box score threshold, unclip
/// ratio.
fn db_params(det_model: &str) -> (f32, f32, f32) {
    (DET_THRESH, box_thresh(det_model), UNCLIP_RATIO)
}

/// Where a model's file is: PP-OCRv6 Manga's in its downloaded folder, the others in the app's.
fn model_path(models_dir: &Path, manga_dir: Option<&Path>, stem: &str) -> PathBuf {
    let dir = if stem.starts_with("manga_") { manga_dir.unwrap_or(models_dir) } else { models_dir };
    dir.join(format!("{stem}.onnx"))
}

/// Longest side of the detector's image on each device.
fn det_max_side(on_gpu: bool) -> f32 {
    if on_gpu { DET_MAX_SIDE_GPU } else { DET_MAX_SIDE_CPU }
}

/// What a detector was loaded as: the manual mode or Lookupper's plan, the model, whether it was
/// asked for the GPU, and which card. Two `PaddleOcr`s with the same one share its session.
type DetKey = (bool, &'static str, bool, Option<GpuPreference>);
/// What a recognizer was loaded as: the model, whether it was asked for the GPU, and which card.
type RecKey = (&'static str, bool, Option<GpuPreference>);

pub struct PaddleOcr {
    /// Shared with the other loaded `PaddleOcr`s that read with the same detector or recognizer
    /// (`load_sharing`), so one model is in memory once.
    det: Arc<Mutex<InferenceSession>>,
    rec: Arc<Mutex<InferenceSession>>,
    dict: Arc<Vec<String>>,
    det_key: Option<DetKey>,
    rec_key: Option<RecKey>,
    det_on_gpu: Arc<AtomicBool>,
    /// The detector on the GPU and on the CPU (`EnginePlan::det_models`).
    det_models: (&'static str, &'static str),
    pub rec_model: String,
    rec_on_gpu: Arc<AtomicBool>,
    /// Which models were loaded (`load`): a pass may need only one.
    has_det: bool,
    has_rec: bool,
    /// The recognizer's input width (`rec_width`; `manga_rec_width` for PP-OCRv6 Manga's).
    rec_width: fn(u32, u32) -> u32,
}

/// A successful GPU session load does not guarantee that a later, larger input will fit.
/// Keep recovery inside the session lock so foreground and overlay OCR share the CPU replacement.
pub(crate) struct InferenceSession<S = Session> {
    pub(crate) session: Option<S>,
    pub(crate) on_gpu: Arc<AtomicBool>,
    pub(crate) cpu_path: PathBuf,
    pub(crate) stage: &'static str,
}

impl<S> InferenceSession<S> {
    pub(crate) fn run<T>(
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

pub(crate) fn cpu_session(path: &Path) -> Result<Session> {
    // On Linux the session's worker threads take this thread's priority (platform/priority.rs).
    crate::platform::priority::raise_this_thread();
    let b = ortx!(Session::builder());
    let b = ortx!(b.with_optimization_level(GraphOptimizationLevel::Level3));
    let mut b = ortx!(b.with_intra_threads(threads()));
    let s = ortx!(b.commit_from_file(path));
    Ok(s)
}

// `choice` is the card the user picked (Configurações › OCR); `None` keeps the API's default.
pub(crate) fn gpu_session(path: &Path, choice: Option<GpuPreference>) -> Result<Session> {
    let mut b = gpu_builder(choice)?;
    Ok(ortx!(b.commit_from_file(path)))
}

#[cfg(windows)]
fn gpu_builder(choice: Option<GpuPreference>) -> Result<SessionBuilder> {
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
    Ok(ortx!(b.with_execution_providers([ep.build().error_on_failure()])))
}

// Linux has no DirectML; the bundled ONNX Runtime build carries the WebGPU provider, which runs on
// Vulkan through Dawn (Mesa RADV, ANV, NVIDIA).
// The WebGPU `deviceId` is ONNX Runtime's context id, not an adapter: the card is picked by Dawn
// from the power preference (high performance when unset). The context is shared by every session
// and released with the last one, so a new preference applies when the engine is rebuilt.
#[cfg(target_os = "linux")]
fn gpu_builder(choice: Option<GpuPreference>) -> Result<SessionBuilder> {
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
    Ok(ortx!(b.with_execution_providers([ep.build().error_on_failure()])))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn gpu_builder(_choice: Option<GpuPreference>) -> Result<SessionBuilder> {
    bail!("GPU inference is only available on Windows (DirectML) and Linux (WebGPU)")
}

impl PaddleOcr {
    pub fn new(models_dir: &Path, rec_stem: &'static str, plan: EnginePlan) -> Result<Self> {
        Self::load(models_dir, None, PaddleUse { det: true, rec: Some(rec_stem), plan })
    }

    /// PP-OCR with the models a pass uses (`PaddleUse`): one not loaded takes no memory, and would
    /// load on the CPU if asked for. PP-OCRv6 Manga's are read from `manga_dir`.
    pub fn load(models_dir: &Path, manga_dir: Option<&Path>, parts: PaddleUse) -> Result<Self> {
        Self::load_sharing(models_dir, manga_dir, parts, &[])
    }

    /// `load`, taking the detector or the recognizer from one of `loaded` that has the same one
    /// (model, device asked for, card): two engines on PP-OCR's det_small hold it once.
    pub fn load_sharing(models_dir: &Path, manga_dir: Option<&Path>, parts: PaddleUse, loaded: &[Arc<PaddleOcr>]) -> Result<Self> {
        let PaddleUse { det: has_det, rec, plan } = parts;
        let det_key: Option<DetKey> = has_det.then_some((plan.manual, plan.det, plan.det_gpu, plan.gpu_choice));
        let rec_key: Option<RecKey> = rec.map(|r| (r, if plan.manual { plan.rec_gpu } else { plan.det_gpu }, plan.gpu_choice));
        let shared_det = det_key.and_then(|k| loaded.iter().find(|o| o.det_key == Some(k)));
        let shared_rec = rec_key.and_then(|k| loaded.iter().find(|o| o.rec_key == Some(k)));
        let gpu_choice = plan.gpu_choice;
        let (det_gpu_model, det_cpu_model) = plan.det_models();
        let det_gpu_path = model_path(models_dir, manga_dir, det_gpu_model);
        let det_cpu_path = model_path(models_dir, manga_dir, det_cpu_model);
        let rec_stem = rec.unwrap_or_default();
        let rec_path = model_path(models_dir, manga_dir, rec_stem);
        let dict_path = models_dir.join(format!("{}.dict.json", if rec_stem == MANGA_REC { "rec_small" } else { rec_stem }));
        if has_det && !det_gpu_path.exists() {
            bail!("detection model not found: {}", det_gpu_path.display());
        }
        if rec.is_some() && !rec_path.exists() {
            bail!("recognition model not found: {}", rec_path.display());
        }

        let (det, det_on_gpu) = match shared_det {
            Some(other) => (other.det.clone(), other.det_on_gpu.clone()),
            None => {
                let (session, on_gpu) = if !has_det {
                    (None, false)
                } else if plan.det_gpu {
                    match gpu_session(&det_gpu_path, gpu_choice) {
                        Ok(s) => (Some(s), true),
                        Err(e) => {
                            log::warn!("Ocr.GpuFallback.Detection: {e:#}");
                            (Some(cpu_session(&det_cpu_path)?), false)
                        }
                    }
                } else {
                    (Some(cpu_session(&det_cpu_path)?), false)
                };
                let on_gpu = Arc::new(AtomicBool::new(on_gpu));
                let session = InferenceSession { session, on_gpu: on_gpu.clone(), cpu_path: det_cpu_path, stage: "Detection" };
                (Arc::new(Mutex::new(session)), on_gpu)
            }
        };
        let det_gpu_now = det_on_gpu.load(Ordering::Relaxed);
        // Automatic: the recognizer goes where the detector went; manual: where the user put it.
        let rec_wants_gpu = if plan.manual { plan.rec_gpu } else { det_gpu_now };
        let (rec_session, rec_on_gpu, dict) = match shared_rec {
            Some(other) => (other.rec.clone(), other.rec_on_gpu.clone(), other.dict.clone()),
            None => {
                let (session, on_gpu) = if rec.is_none() {
                    (None, false)
                } else if rec_wants_gpu {
                    match gpu_session(&rec_path, gpu_choice) {
                        Ok(s) => (Some(s), true),
                        Err(e) => {
                            log::warn!("Ocr.GpuFallback.Recognition: {e:#}");
                            (Some(cpu_session(&rec_path)?), false)
                        }
                    }
                } else {
                    (Some(cpu_session(&rec_path)?), false)
                };
                let dict: Vec<String> = if rec.is_some() {
                    serde_json::from_slice(&std::fs::read(&dict_path).with_context(|| format!("read {}", dict_path.display()))?)?
                } else {
                    Vec::new()
                };
                let on_gpu = Arc::new(AtomicBool::new(on_gpu));
                let session = InferenceSession { session, on_gpu: on_gpu.clone(), cpu_path: rec_path, stage: "Recognition" };
                (Arc::new(Mutex::new(session)), on_gpu, Arc::new(dict))
            }
        };
        let device = |gpu: bool| if gpu { "GPU" } else { "CPU" };
        let shared = |yes: bool| if yes { ", shared" } else { "" };
        let det_model = if det_gpu_now { det_gpu_model } else { det_cpu_model };
        log::info!(
            "OCR engine ready: {}, {}",
            if has_det { format!("{det_model} on {}{}", device(det_gpu_now), shared(shared_det.is_some())) } else { "no detector".into() },
            if rec.is_some() {
                format!("{rec_stem} on {}, {} characters{}", device(rec_on_gpu.load(Ordering::Relaxed)), dict.len(), shared(shared_rec.is_some()))
            } else {
                "no recognizer".into()
            },
        );
        Ok(Self {
            det,
            rec: rec_session,
            dict,
            det_key,
            rec_key,
            det_on_gpu,
            det_models: (det_gpu_model, det_cpu_model),
            rec_model: rec_stem.to_string(),
            rec_on_gpu,
            has_det,
            has_rec: rec.is_some(),
            rec_width: if rec_stem == MANGA_REC { manga_rec_width } else { rec_width },
        })
    }

    /// Whether its detector, and its recognizer, were loaded (`load`).
    pub fn has_det(&self) -> bool {
        self.has_det
    }

    pub fn has_rec(&self) -> bool {
        self.has_rec
    }

    pub fn det_on_gpu(&self) -> bool {
        self.det_on_gpu.load(Ordering::Relaxed)
    }

    fn det_max_side(&self) -> f32 {
        det_max_side(self.det_on_gpu())
    }

    pub fn rec_on_gpu(&self) -> bool {
        self.rec_on_gpu.load(Ordering::Relaxed)
    }

    /// `plan_batches` with this engine's recognizer padding.
    pub fn plan_batches(&self, boxes: &[TextBox]) -> Vec<Vec<TextBox>> {
        plan_batches(boxes, |w| rec_padded_width(&self.rec_model, w))
    }

    /// The detector in use: the GPU's while it runs there, else the CPU's (also after a GPU failure).
    pub fn det_model(&self) -> &'static str {
        if self.det_on_gpu() { self.det_models.0 } else { self.det_models.1 }
    }

    /// Runs the loaded models once so the first real lookup does not pay for graph setup.
    pub fn warm_up(&self) {
        let blank = RgbaImage::from_pixel(96, 64, Rgba([255, 255, 255, 255]));
        if self.has_det {
            if let Err(e) = self.detect(&blank) {
                log::warn!("detector warm-up failed: {e:#}");
            }
        }
        let b = TextBox {
            x0: 0.0,
            y0: 0.0,
            x1: 96.0,
            y1: 32.0,
            quad: None,
        };
        if self.has_rec {
            if let Err(e) = self.recognize_box(&blank, b) {
                log::warn!("recognizer warm-up failed: {e:#}");
            }
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
        // The padded size is a multiple of 32, so the detector needs no further resize.
        let round32 = |v: f32| -> u32 { (((v / 32.0).round() as u32) * 32).max(32) };
        let pw = round32(w as f32 * scale + (2 * pad) as f32);
        let ph = round32(h as f32 * scale + (2 * pad) as f32);
        let (rw, rh) = (pw - 2 * pad, ph - 2 * pad);
        let mut resized = RgbaImage::from_pixel(pw, ph, image::Rgba([255, 255, 255, 255]));
        image::imageops::replace(&mut resized, &image::imageops::resize(img, rw, rh, FilterType::Triangle), pad as i64, pad as i64);
        let (rw, rh, pad) = (rw as f32, rh as f32, pad as f32);
        // One pass over the whole image, as Lookupper: in bands, a column longer than a band's
        // overlap was seen only in pieces, and the tops of columns side by side read as a line.
        let mut boxes = self.detect_image(&resized)?;
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

    fn detect_image(&self, img: &RgbaImage) -> Result<Vec<TextBox>> {
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

        Ok(db_boxes(&prob, mw, mh, (w as f32 / mw as f32, h as f32 / mh as f32), (w as f32, h as f32), db_params(self.det_model())))
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
        let Some(crop) = rec_crop(img, b, turn, true, self.rec_width) else { return Ok(()) };
        let pw = rec_padded_width(&self.rec_model, crop.tw) as usize;
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
        for batch in self.plan_batches(boxes) {
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
        let crops: Vec<(usize, RecCrop)> = reads.iter().filter_map(|&(i, t)| rec_crop(img, boxes[i], t, true, self.rec_width).map(|c| (i, c))).collect();
        if crops.is_empty() {
            return Ok(());
        }
        let pw = rec_padded_width(&self.rec_model, crops.iter().map(|(_, c)| c.tw).max().unwrap_or(16)) as usize;
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
        // Each CTC step covers an equal slice of the padded input, so of the crop's length.
        let step_w = crop.length().1 as f32 / content_steps;
        line_from(crop, decoded, step_w, sure / read.max(1) as f32)
    }

}

/// The boxes of a DB detector's probability map (`mw` × `mh`): 4-connected kernels over `thresh`
/// whose mean is at least `box_thresh`, grown back by area × `unclip` / perimeter, in the image's
/// pixels (`scale` image pixels to one of the map's, at most `limit`). As Lookupper, a turned
/// kernel keeps its smallest rectangle. Shared with comic-text-detector (ctd.rs).
pub(crate) fn db_boxes(prob: &[f32], mw: usize, mh: usize, scale: (f32, f32), limit: (f32, f32), params: (f32, f32, f32)) -> Vec<TextBox> {
    let ((sx, sy), (w, h), (thresh, box_thresh, unclip)) = (scale, limit, params);
    let mut visited = vec![false; mh * mw];
    let mut boxes = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut pixels: Vec<(usize, usize)> = Vec::new();
    for start in 0..mh * mw {
        if visited[start] || prob[start] <= thresh {
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
                if !visited[j] && prob[j] > thresh {
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
            let q = r.grown(kw * kh * unclip / (2.0 * (kw + kh))).corners().map(|p| [p[0] * sx, p[1] * sy]);
            let (x0, y0, x1, y1) = quad::bounds(&q);
            let (x0, y0, x1, y1) = (x0.max(0.0), y0.max(0.0), x1.min(w), y1.min(h));
            if x1 - x0 >= 4.0 && y1 - y0 >= 4.0 {
                boxes.push(TextBox { x0, y0, x1, y1, quad: Some(q) });
            }
            continue;
        }
        if bw < 3.0 || bh < 3.0 {
            continue;
        }
        // DB predicts shrunk text kernels; grow them back by area * ratio / perimeter.
        let d = bw * bh * unclip / (2.0 * (bw + bh));
        let x0 = ((minx as f32 - d) * sx).max(0.0);
        let y0 = ((miny as f32 - d) * sy).max(0.0);
        let x1 = (((maxx + 1) as f32 + d) * sx).min(w);
        let y1 = (((maxy + 1) as f32 + d) * sy).min(h);
        if x1 - x0 >= 4.0 && y1 - y0 >= 4.0 {
            boxes.push(TextBox { x0, y0, x1, y1, quad: None });
        }
    }
    boxes
}

/// Recognizes the boxes with another recognizer than PP-OCR's (`BoxReader`), `READ_RUN` boxes
/// to a call; boxes without readable text are dropped. Columns are read top to bottom only.
pub fn recognize_boxes_with(img: &RgbaImage, boxes: &[TextBox], reader: &dyn BoxReader) -> Vec<OcrLine> {
    let mut lines = Vec::with_capacity(boxes.len());
    for run in boxes.chunks(READ_RUN) {
        let crops: Vec<RecCrop> = run.iter().filter_map(|b| rec_crop(img, *b, first_turns(b)[0], false, rec_width)).collect();
        // A column goes to the reader standing, as it is on the screen.
        let standing: Vec<Option<RgbaImage>> = crops
            .iter()
            .map(|c| (c.turn == Turn::Left).then(|| image::imageops::rotate90(&c.image)))
            .collect();
        let inputs: Vec<(&RgbaImage, bool)> =
            crops.iter().zip(&standing).map(|(c, u)| (u.as_ref().unwrap_or(&c.image), c.turn == Turn::Left)).collect();
        let read = match reader.read(&inputs) {
            Ok(read) => read,
            Err(e) => {
                log::warn!("recognition failed: {e:#}");
                continue;
            }
        };
        for (crop, glyphs) in crops.iter().zip(read) {
            if let Some(reading) = line_from_glyphs(crop, glyphs) {
                lines.push(reading.line);
            }
        }
    }
    lines.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
    lines
}

/// Boxes a `BoxReader` gets at once in `recognize_boxes_with`.
const READ_RUN: usize = 16;

/// A character another recognizer read (`BoxReader`), with where it lies along its crop, in the
/// crop's pixels (capture pixels, or the straightened crop's for a turned box), and its score.
#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub ch: char,
    pub from: f32,
    pub to: f32,
    pub score: f32,
}

/// A recognizer reading the detector's boxes instead of PP-OCR's (ocr/indra.rs). It gets each box's
/// crop at capture resolution, a line upright or (`true`) a column standing, and gives each crop's
/// characters in reading order.
pub trait BoxReader: Send + Sync {
    fn read(&self, crops: &[(&RgbaImage, bool)]) -> Result<Vec<Vec<Glyph>>>;
}

impl RecCrop {
    /// Where the line starts and how long it is in capture pixels, and its ends: across, or down a
    /// turned crop (from the top for `Turn::Left`; `Turn::Right` is mirrored in `line_from`).
    fn length(&self) -> (u32, u32, f32, f32) {
        let b = self.b;
        match (self.turn, b.quad.is_some()) {
            // A straightened crop: positions along it, mapped onto the capture in `line_from`.
            (Turn::Upright, true) => (0, self.cw, 0.0, self.cw as f32),
            (_, true) => (0, self.ch, 0.0, self.ch as f32),
            (Turn::Upright, false) => (self.x, self.cw, b.x0, b.x1),
            (_, false) => (self.y, self.ch, b.y0, b.y1),
        }
    }

    /// The crop's height across the line, in capture pixels.
    fn across(&self) -> f32 {
        (if self.turn == Turn::Upright { self.ch } else { self.cw }) as f32
    }
}

/// A `BoxReader`'s characters as PP-OCR's would be: each on the step of a 48-pixel-high recognizer
/// (a sixth of the line's height) where its centre lies, so word gaps, ink and spans are found as
/// for PP-OCR's lines. Its score is the mean of its characters'.
fn line_from_glyphs(crop: &RecCrop, glyphs: Vec<Glyph>) -> Option<Reading> {
    if glyphs.is_empty() {
        return None;
    }
    let step_w = (crop.across() / 6.0).max(1.0);
    let score = glyphs.iter().map(|g| g.score).sum::<f32>() / glyphs.len() as f32;
    let decoded = glyphs.iter().map(|g| (g.ch, (((g.from + g.to) / 2.0).max(0.0) / step_w) as usize)).collect();
    line_from(crop, decoded, step_w, score)
}

/// The line of `crop` from its characters, each with its recognizer step (`step_w` crop pixels
/// long): spaces at wide gaps, word edges snapped to the ink, spans mapped onto the capture.
fn line_from(crop: &RecCrop, decoded: Vec<(char, usize)>, step_w: f32, score: f32) -> Option<Reading> {
    {
        let b = crop.b;
        let (start, _, lo, hi) = crop.length();
        let decoded = split_wide_gaps(decoded);
        if decoded.iter().all(|(c, _)| c.is_whitespace()) {
            return None;
        }
        let across = crop.across();
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
        Some(Reading { line, score })
    }
}
