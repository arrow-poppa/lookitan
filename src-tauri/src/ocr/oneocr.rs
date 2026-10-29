// Microsoft's OneOCR, the OCR of Windows 11's Snipping Tool, as an OCR engine (Windows only), at the
// user's request on 2026-10-28, as owocr uses it: its three files (oneocr.dll, oneocr.onemodel and
// Microsoft's own onnxruntime.dll) are copied from the Snipping Tool installed on this computer
// (`Get-AppxPackage Microsoft.ScreenSketch`) into the app's local data; they are Microsoft's, never
// in the installers nor fetched from anywhere else.
//
// It runs in a helper process (ocr/helper.rs): oneocr.dll needs its own onnxruntime.dll, which
// cannot share a process with Lookitan's (the loader would hand it the one already loaded). The
// helper loads oneocr.dll with its folder searched first, so Microsoft's copy is the one it gets.
// Its answers are JSON: each line's text and corners, and each word's with its confidence.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

use super::helper::Helper;
use super::{align, is_rtl_text, Flow, OcrLine, Tilt};

pub const FILES: [&str; 3] = ["oneocr.dll", "oneocr.onemodel", "onnxruntime.dll"];
/// Its sides must be 50 to 10,000 pixels.
const MIN_SIDE: u32 = 50;
const MAX_SIDE: u32 = 10_000;
/// Lines turned less than this read as level (as quad.rs's `UPRIGHT_ANGLE`).
const LEVEL: f32 = 2.0 * std::f32::consts::PI / 180.0;

pub fn supported() -> bool {
    cfg!(windows)
}

fn dir() -> Result<PathBuf> {
    Ok(super::models::root()?.join("oneocr"))
}

pub fn installed() -> bool {
    dir().is_ok_and(|d| FILES.iter().all(|f| d.join(f).is_file()))
}

/// The Snipping Tool version the files were copied from.
pub fn version() -> Option<String> {
    std::fs::read_to_string(dir().ok()?.join("version.txt")).ok().map(|v| v.trim().to_string())
}

/// Copies the files from the Snipping Tool installed here, reporting the bytes copied.
pub fn install(progress: &dyn Fn(u64)) -> Result<()> {
    if !supported() {
        bail!("o OneOCR existe só no Windows");
    }
    let location = snipping_tool()?;
    let source = location.join("SnippingTool");
    let version = location
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split('_').nth(1))
        .unwrap_or_default()
        .to_string();
    let root = super::models::root()?;
    std::fs::create_dir_all(&root)?;
    let copying = root.join(format!(".copying-oneocr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&copying);
    std::fs::create_dir_all(&copying)?;
    let mut done = 0;
    for f in FILES {
        let from = source.join(f);
        if !from.is_file() {
            let _ = std::fs::remove_dir_all(&copying);
            bail!("o Recorte instalado não tem {f} (é preciso o Windows 11 com o Recorte atualizado)");
        }
        let copied = std::fs::copy(&from, copying.join(f)).with_context(|| format!("copy {}", from.display()));
        match copied {
            Ok(n) => done += n,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&copying);
                return Err(e);
            }
        }
        progress(done);
    }
    std::fs::write(copying.join("version.txt"), &version)?;
    let home = dir()?;
    let _ = std::fs::remove_dir_all(&home);
    std::fs::rename(&copying, &home).context("move OneOCR into place")?;
    log::info!("OneOCR copied from the Snipping Tool {version}");
    Ok(())
}

/// The Snipping Tool's install folder, as owocr finds it.
fn snipping_tool() -> Result<PathBuf> {
    let mut command = std::process::Command::new("powershell");
    command.args(["-NoProfile", "-NonInteractive", "-Command", "(Get-AppxPackage Microsoft.ScreenSketch).InstallLocation"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let out = command.output().context("ask Windows where the Snipping Tool is")?;
    let location = String::from_utf8_lossy(&out.stdout).lines().map(str::trim).find(|l| !l.is_empty()).map(PathBuf::from);
    match location {
        Some(path) if path.is_dir() => Ok(path),
        _ => bail!("o Recorte (Snipping Tool) não está instalado"),
    }
}

pub fn remove() -> Result<()> {
    let home = dir()?;
    if home.exists() {
        std::fs::remove_dir_all(&home).with_context(|| format!("delete {}", home.display()))?;
    }
    Ok(())
}

// ------------------------------------------------------------------------------------ the answer

/// A quadrilateral: top left, top right, bottom right, bottom left, in image pixels.
type Corners = [f32; 8];

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub corners: Corners,
    pub words: Vec<Word>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub corners: Corners,
    pub confidence: f32,
}

// ------------------------------------------------------------------------------------ the helper

/// The helper process (`helper::main`): loads OneOCR from `dir` and answers until stdin closes.
#[cfg(windows)]
pub fn serve(dir: &Path) -> Result<()> {
    use std::ffi::{c_char, c_void, CStr, CString};

    /// OneOCR's image: 8-bit BGRA, packed.
    #[repr(C, packed)]
    struct Image {
        kind: i32,
        width: i32,
        height: i32,
        reserved: i32,
        step: i64,
        data: *const u8,
    }
    type Handle = *mut c_void;
    // The key OneOCR's model is opened with, as oneocr and owocr pass it.
    const KEY: &str = r#"kj)TGtrK>f]b[Piow.gU+nC@s""""""4"#;

    // Its folder first, so oneocr.dll gets the onnxruntime.dll next to it.
    let path = dir.join(FILES[0]);
    let library: libloading::Library = unsafe {
        libloading::os::windows::Library::load_with_flags(&path, libloading::os::windows::LOAD_WITH_ALTERED_SEARCH_PATH)
    }
    .with_context(|| format!("load {}", path.display()))?
    .into();
    macro_rules! f {
        ($name:literal, $t:ty) => {
            *unsafe { library.get::<$t>(concat!($name, "\0").as_bytes()) }.context($name)?
        };
    }
    let create_init = f!("CreateOcrInitOptions", unsafe extern "C" fn(*mut Handle) -> i32);
    let delay_load = f!("OcrInitOptionsSetUseModelDelayLoad", unsafe extern "C" fn(Handle, u8) -> i32);
    let create_pipeline = f!("CreateOcrPipeline", unsafe extern "C" fn(*const c_char, *const c_char, Handle, *mut Handle) -> i32);
    let create_options = f!("CreateOcrProcessOptions", unsafe extern "C" fn(*mut Handle) -> i32);
    let max_lines = f!("OcrProcessOptionsSetMaxRecognitionLineCount", unsafe extern "C" fn(Handle, i32) -> i32);
    let run = f!("RunOcrPipeline", unsafe extern "C" fn(Handle, *const Image, Handle, *mut Handle) -> i32);
    let line_count = f!("GetOcrLineCount", unsafe extern "C" fn(Handle, *mut i64) -> i32);
    let line_at = f!("GetOcrLine", unsafe extern "C" fn(Handle, i64, *mut Handle) -> i32);
    let line_text = f!("GetOcrLineContent", unsafe extern "C" fn(Handle, *mut *const c_char) -> i32);
    let line_box = f!("GetOcrLineBoundingBox", unsafe extern "C" fn(Handle, *mut *const Corners) -> i32);
    let word_count = f!("GetOcrLineWordCount", unsafe extern "C" fn(Handle, *mut i64) -> i32);
    let word_at = f!("GetOcrWord", unsafe extern "C" fn(Handle, i64, *mut Handle) -> i32);
    let word_text = f!("GetOcrWordContent", unsafe extern "C" fn(Handle, *mut *const c_char) -> i32);
    let word_box = f!("GetOcrWordBoundingBox", unsafe extern "C" fn(Handle, *mut *const Corners) -> i32);
    let word_confidence = f!("GetOcrWordConfidence", unsafe extern "C" fn(Handle, *mut f32) -> i32);
    let release = f!("ReleaseOcrResult", unsafe extern "C" fn(Handle));

    let (mut init, mut pipeline, mut options): (Handle, Handle, Handle) = (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
    let model = CString::new(dir.join(FILES[1]).to_string_lossy().as_bytes())?;
    let key = CString::new(KEY)?;
    unsafe {
        if create_init(&mut init) != 0 || delay_load(init, 0) != 0 {
            bail!("OneOCR's options could not be made");
        }
        let code = create_pipeline(model.as_ptr(), key.as_ptr(), init, &mut pipeline);
        if code != 0 {
            bail!("OneOCR's model could not be opened (error {code})");
        }
        if create_options(&mut options) != 0 || max_lines(options, 1000) != 0 {
            bail!("OneOCR's process options could not be made");
        }
    }
    let text = |p: *const c_char| if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() };
    let corners = |p: *const Corners| if p.is_null() { [0.0; 8] } else { unsafe { std::ptr::read_unaligned(p) } };

    super::helper::serve((1, 0), MAX_SIDE, |w, h, rgba| {
        // OneOCR reads BGRA, at least 50 pixels a side: a smaller image is padded with white.
        let (pw, ph) = (w.max(MIN_SIDE), h.max(MIN_SIDE));
        let mut bgra = vec![255u8; pw as usize * ph as usize * 4];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let (i, o) = ((y * w as usize + x) * 4, (y * pw as usize + x) * 4);
                bgra[o..o + 4].copy_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], 255]);
            }
        }
        let image = Image { kind: 3, width: pw as i32, height: ph as i32, reserved: 0, step: pw as i64 * 4, data: bgra.as_ptr() };
        let mut result: Handle = std::ptr::null_mut();
        if unsafe { run(pipeline, &image, options, &mut result) } != 0 || result.is_null() {
            return None;
        }
        let mut lines = Vec::new();
        let mut n = 0i64;
        unsafe {
            if line_count(result, &mut n) == 0 {
                for i in 0..n {
                    let mut line: Handle = std::ptr::null_mut();
                    if line_at(result, i, &mut line) != 0 || line.is_null() {
                        continue;
                    }
                    let (mut t, mut b, mut words) = (std::ptr::null(), std::ptr::null(), 0i64);
                    let _ = line_text(line, &mut t);
                    let _ = line_box(line, &mut b);
                    let _ = word_count(line, &mut words);
                    let mut out = Line { text: text(t), corners: corners(b), words: Vec::new() };
                    for j in 0..words {
                        let mut word: Handle = std::ptr::null_mut();
                        if word_at(line, j, &mut word) != 0 || word.is_null() {
                            continue;
                        }
                        let (mut t, mut b, mut c) = (std::ptr::null(), std::ptr::null(), 0f32);
                        let _ = word_text(word, &mut t);
                        let _ = word_box(word, &mut b);
                        let _ = word_confidence(word, &mut c);
                        out.words.push(Word { text: text(t), corners: corners(b), confidence: c });
                    }
                    lines.push(out);
                }
            }
            release(result);
        }
        serde_json::to_vec(&lines).ok()
    })
}

#[cfg(not(windows))]
pub fn serve(_dir: &Path) -> Result<()> {
    bail!("OneOCR exists on Windows only")
}

// ---------------------------------------------------------------------------------- the app side

pub struct OneOcr {
    helper: Helper,
}

impl Default for OneOcr {
    fn default() -> Self {
        Self { helper: Helper::new("OneOCR", "oneocr") }
    }
}

impl OneOcr {
    /// Reads the whole image: its lines in capture pixels, each word on its own box.
    pub fn recognize(&self, img: &RgbaImage) -> Result<Vec<OcrLine>> {
        if !supported() {
            bail!("ele existe só no Windows");
        }
        if !installed() {
            bail!("os arquivos dele não foram copiados do Recorte (Configurações › OCR)");
        }
        let (answer, k) = self.helper.ask(&dir()?, img)?;
        let Some(bytes) = answer else { bail!("ele não leu esta imagem") };
        let lines: Vec<Line> = serde_json::from_slice(&bytes).context("read OneOCR's answer")?;
        let mut out = to_lines(&lines, k);
        // Word edges snap to the ink, as Lens's do: level lines drawn left to right only.
        for line in out.iter_mut().filter(|l| l.tilt.is_none() && !is_rtl_text(l.chars.iter().map(|c| c.ch))) {
            super::lens::snap_to_ink(img, line);
        }
        Ok(out)
    }

    /// Ends the helper now (before its files are deleted).
    pub fn stop(&self) {
        self.helper.stop();
    }
}

/// OneOCR's lines in capture pixels (`k` capture pixels to one of the image read). A word's letters
/// share its box evenly (OneOCR gives a box per word, and per character in CJK); spaces between
/// words sit at the end of the word before them. A line turned over 2° is given in its own frame
/// (`Tilt`); a tall line of words stacked down it is a column (`Flow::Down`).
pub fn to_lines(lines: &[Line], k: f32) -> Vec<OcrLine> {
    let mut out = Vec::new();
    for line in lines {
        if line.text.trim().is_empty() {
            continue;
        }
        let p: Vec<(f32, f32)> = (0..4).map(|i| (line.corners[2 * i] * k, line.corners[2 * i + 1] * k)).collect();
        let x0 = p.iter().map(|q| q.0).fold(f32::MAX, f32::min);
        let x1 = p.iter().map(|q| q.0).fold(f32::MIN, f32::max);
        let y0 = p.iter().map(|q| q.1).fold(f32::MAX, f32::min);
        let y1 = p.iter().map(|q| q.1).fold(f32::MIN, f32::max);
        let centre = |c: &Corners| ((c[0] + c[2] + c[4] + c[6]) * k / 4.0, (c[1] + c[3] + c[5] + c[7]) * k / 4.0);
        let spread = |axis: fn((f32, f32)) -> f32| {
            let v: Vec<f32> = line.words.iter().map(|w| axis(centre(&w.corners))).collect();
            v.iter().copied().fold(f32::MIN, f32::max) - v.iter().copied().fold(f32::MAX, f32::min)
        };
        let column = super::is_vertical_box(x1 - x0, y1 - y0) && (line.words.len() < 2 || spread(|c| c.1) > spread(|c| c.0));
        let flow = if column { Flow::Down } else { Flow::Across };
        let angle = (p[1].1 - p[0].1).atan2(p[1].0 - p[0].0);
        let tilt = (!column && angle.abs() > LEVEL).then(|| {
            let frame = Tilt { angle, rect: (0.0, 0.0, 0.0, 0.0) };
            let (fx0, fy0) = frame.to_frame(p[0].0, p[0].1);
            let (fx1, _) = frame.to_frame(p[1].0, p[1].1);
            let (_, fy1) = frame.to_frame(p[3].0, p[3].1);
            Tilt { angle, rect: (fx0, fy0, fx1, fy1) }
        });
        let mut glyphs: Vec<(char, f32, f32)> = Vec::new();
        for word in &line.words {
            let c: Vec<(f32, f32)> = (0..4).map(|i| (word.corners[2 * i] * k, word.corners[2 * i + 1] * k)).collect();
            // Where the word starts and ends along the line.
            let (from, to) = match (&tilt, flow) {
                (Some(t), _) => (t.to_frame(c[0].0, c[0].1).0, t.to_frame(c[1].0, c[1].1).0),
                (None, Flow::Down) => (c.iter().map(|q| q.1).fold(f32::MAX, f32::min), c.iter().map(|q| q.1).fold(f32::MIN, f32::max)),
                (None, _) => (c.iter().map(|q| q.0).fold(f32::MAX, f32::min), c.iter().map(|q| q.0).fold(f32::MIN, f32::max)),
            };
            let letters: Vec<char> = word.text.chars().collect();
            let step = (to - from) / letters.len().max(1) as f32;
            // A right-to-left word's letters, in reading order, run right to left.
            let rtl = flow == Flow::Across && is_rtl_text(letters.iter().copied());
            for (i, ch) in letters.iter().enumerate() {
                let (a, b) = if rtl { (to - step * (i + 1) as f32, to - step * i as f32) } else { (from + step * i as f32, from + step * (i + 1) as f32) };
                glyphs.push((*ch, a, b));
            }
        }
        let chars = align(&line.text, &glyphs);
        if chars.iter().all(|c| c.ch.is_whitespace()) {
            continue;
        }
        let text: String = chars.iter().map(|c| c.ch).collect();
        out.push(OcrLine { text, x0, y0, x1, y1, chars, ink_y: None, flow, tilt, look: Default::default() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Corners {
        [x0, y0, x1, y0, x1, y1, x0, y1]
    }

    fn word(text: &str, x0: f32, y0: f32, x1: f32, y1: f32) -> Word {
        Word { text: text.into(), corners: rect(x0, y0, x1, y1), confidence: 0.99 }
    }

    #[test]
    fn words_share_their_boxes_and_spaces_sit_between_them() {
        let line = Line { text: "Hi you".into(), corners: rect(10.0, 5.0, 70.0, 25.0), words: vec![word("Hi", 10.0, 5.0, 30.0, 25.0), word("you", 40.0, 5.0, 70.0, 25.0)] };
        let lines = to_lines(&[line], 2.0);
        let l = &lines[0];
        assert_eq!(l.text, "Hi you");
        assert_eq!((l.x0, l.y0, l.x1, l.y1), (20.0, 10.0, 140.0, 50.0));
        assert_eq!(l.flow, Flow::Across);
        assert!(l.tilt.is_none());
        let spans: Vec<(char, f32, f32)> = l.chars.iter().map(|c| (c.ch, c.x0, c.x1)).collect();
        assert_eq!(spans[0], ('H', 20.0, 40.0));
        assert_eq!(spans[2], (' ', 60.0, 60.0));
        assert_eq!(spans[3], ('y', 80.0, 100.0));
        // The JSON the helper writes reads back the same.
        let json = serde_json::to_vec(&vec![Line { text: "a".into(), corners: rect(0.0, 0.0, 1.0, 1.0), words: Vec::new() }]).unwrap();
        assert_eq!(serde_json::from_slice::<Vec<Line>>(&json).unwrap()[0].text, "a");
    }

    #[test]
    fn a_column_runs_down_a_right_to_left_word_runs_left_and_a_turned_line_keeps_its_frame() {
        let column = Line { text: "日本".into(), corners: rect(10.0, 10.0, 30.0, 60.0), words: vec![word("日", 10.0, 10.0, 30.0, 30.0), word("本", 10.0, 35.0, 30.0, 55.0)] };
        let arabic = Line { text: "من".into(), corners: rect(0.0, 0.0, 40.0, 20.0), words: vec![word("من", 0.0, 0.0, 40.0, 20.0)] };
        let (s, c) = 20f32.to_radians().sin_cos();
        let turned = Line {
            text: "ab".into(),
            corners: [0.0, 0.0, 40.0 * c, 40.0 * s, 40.0 * c - 10.0 * s, 40.0 * s + 10.0 * c, -10.0 * s, 10.0 * c],
            words: vec![Word { text: "ab".into(), corners: [0.0, 0.0, 40.0 * c, 40.0 * s, 40.0 * c - 10.0 * s, 40.0 * s + 10.0 * c, -10.0 * s, 10.0 * c], confidence: 0.9 }],
        };
        let lines = to_lines(&[column, arabic, turned], 1.0);
        assert_eq!(lines[0].flow, Flow::Down);
        assert_eq!((lines[0].chars[1].x0, lines[0].chars[1].x1), (35.0, 55.0));
        assert_eq!((lines[1].chars[0].ch, lines[1].chars[0].x0, lines[1].chars[0].x1), ('م', 20.0, 40.0), "the first letter on the right");
        let t = lines[2].tilt.expect("turned 20°");
        assert!((t.angle - 20f32.to_radians()).abs() < 1e-4);
        assert!((lines[2].chars[1].x1 - lines[2].chars[0].x0 - 40.0).abs() < 1e-3, "along the line's frame");
    }
}
