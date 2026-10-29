// Chrome's Screen AI OCR (Google's on-device library, the one Chrome reads images and PDFs with) as
// an OCR engine for any language: it finds each line's script and language itself, and gives every
// character its own box.
//
// The library comes from Chromium's CIPD package `chromium/third_party/screen-ai/<platform>`, as
// owocr fetches it (at the user's request on 2026-10-28), pinned to one instance whose SHA-256 is
// the zip's, and downloaded only when the user picks the engine; it is not in the installers. The
// package carries the licences of what it is built from, none of its own: it is Google's.
//
// It runs in a helper process (ocr/helper.rs), as Chrome runs it in a utility process: it has no
// shutdown and can be initialized once per process. Its answers are the serialized
// `VisualAnnotation`.

use std::ffi::{c_char, c_void, CStr};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use image::RgbaImage;
use prost::Message;

use super::helper::Helper;
use super::{align, Flow, OcrLine, Tilt};

const CIPD: &str = "https://chrome-infra-packages.appspot.com/prpc/cipd.Repository/GetInstanceURL";
/// Chrome needs 140.3 or later (`whitespace_bounding_box`); the pinned packages are 153.05.
const MIN_VERSION: (u32, u32) = (140, 3);
/// Lines turned less than this read as level (as quad.rs's `UPRIGHT_ANGLE`).
const LEVEL: f32 = 2.0 * std::f32::consts::PI / 180.0;

struct Package {
    name: &'static str,
    digest: &'static str,
    bytes: u64,
    library: &'static str,
}

// Version 153.05, registered 2026-10-18.
#[cfg(windows)]
static PACKAGE: Option<Package> = Some(Package {
    name: "chromium/third_party/screen-ai/windows-amd64",
    digest: "cec1c343edb1047a01a7962d28a6bc3390d78635ed88aae585c7fcdf823224af",
    bytes: 76_409_162,
    library: "chrome_screen_ai.dll",
});
#[cfg(target_os = "linux")]
static PACKAGE: Option<Package> = Some(Package {
    name: "chromium/third_party/screen-ai/linux",
    digest: "cd837ed33b7098f1c65f22c4618c79a98dc86e50b58fd7f2d3cae94cf9ce36ac",
    bytes: 78_301_068,
    library: "libchromescreenai.so",
});
#[cfg(not(any(windows, target_os = "linux")))]
static PACKAGE: Option<Package> = None;

pub fn supported() -> bool {
    PACKAGE.is_some()
}

pub fn bytes() -> u64 {
    PACKAGE.as_ref().map_or(0, |p| p.bytes)
}

fn package() -> Result<&'static Package> {
    PACKAGE.as_ref().context("Chrome ScreenAI is not available on this system")
}

/// The package's folder: `resources/` in it holds the library and its models.
fn dir() -> Result<PathBuf> {
    Ok(super::models::root()?.join(format!("screenai-{}", &package()?.digest[..8])))
}

fn resources() -> Result<PathBuf> {
    Ok(dir()?.join("resources"))
}

pub fn installed() -> bool {
    package().is_ok_and(|p| resources().is_ok_and(|r| r.join(p.library).is_file()))
}

/// Downloads and unpacks the package, reporting the bytes done of `bytes()`.
pub fn ensure(progress: &dyn Fn(u64)) -> Result<()> {
    if installed() {
        return Ok(());
    }
    let p = package()?;
    let root = super::models::root()?;
    std::fs::create_dir_all(&root)?;
    // What an interrupted unpacking left behind.
    for entry in std::fs::read_dir(&root)?.flatten() {
        if entry.file_name().to_string_lossy().starts_with(".unpacking-screenai-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let zip = root.join(".download-screenai.zip");
    let fetched = signed_url().and_then(|url| crate::segment::download(&url, &zip, p.bytes, Some(p.digest), progress));
    if let Err(e) = fetched {
        let _ = std::fs::remove_file(&zip);
        return Err(e);
    }
    let unpacking = root.join(format!(".unpacking-screenai-{}", std::process::id()));
    std::fs::create_dir_all(&unpacking)?;
    let unpacked = crate::dictionaries::archive::unpack(&zip, crate::dictionaries::archive::Kind::Zip, &unpacking, &|_| {});
    let _ = std::fs::remove_file(&zip);
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&unpacking);
        return Err(e);
    }
    if !unpacking.join("resources").join(p.library).is_file() {
        let _ = std::fs::remove_dir_all(&unpacking);
        bail!("the ScreenAI package has no resources/{}", p.library);
    }
    let home = dir()?;
    let _ = std::fs::remove_dir_all(&home);
    std::fs::rename(&unpacking, &home).context("move ScreenAI into place")?;
    Ok(())
}

pub fn remove() -> Result<()> {
    let home = dir()?;
    if home.exists() {
        std::fs::remove_dir_all(&home).with_context(|| format!("delete {}", home.display()))?;
    }
    Ok(())
}

/// CIPD's signed download URL for the pinned instance.
fn signed_url() -> Result<String> {
    let p = package()?;
    let body = serde_json::json!({"package": p.name, "instance": {"hashAlgo": "SHA256", "hexDigest": p.digest}});
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build()?;
    let text = client
        .post(CIPD)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .body(body.to_string())
        .send()?
        .error_for_status()?
        .text()?;
    // pRPC's JSON answers start with an anti-XSSI line.
    let json: serde_json::Value = serde_json::from_str(text.trim_start_matches(")]}'").trim_start()).context("read CIPD's answer")?;
    json["signedUrl"].as_str().map(str::to_string).context("CIPD gave no download URL")
}

// ------------------------------------------------------------------------------------ the helper

/// Skia's `SkBitmap` as the library reads it on 64-bit systems (as owocr lays it out): no pixel
/// ref, the pixels, their row bytes, no colour space, RGBA_8888 (4), opaque (1), the size.
#[repr(C)]
struct SkBitmap {
    pixel_ref: *mut c_void,
    pixels: *const c_void,
    row_bytes: usize,
    color_space: *mut c_void,
    color_type: i32,
    alpha_type: i32,
    width: i32,
    height: i32,
    flags: u32,
}

/// The library's folder, for its file callbacks (they get no context).
static FILES: OnceLock<PathBuf> = OnceLock::new();

/// A file the library asks for, inside its folder only.
fn library_file(name: *const c_char) -> Option<PathBuf> {
    if name.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(name) }.to_str().ok()?;
    let relative = Path::new(name.trim_start_matches(['/', '\\']));
    if relative.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(FILES.get()?.join(relative))
}

extern "C" fn file_size(name: *const c_char) -> u32 {
    library_file(name).and_then(|p| p.metadata().ok()).map_or(0, |m| m.len().min(u32::MAX as u64) as u32)
}

extern "C" fn file_content(name: *const c_char, size: u32, buffer: *mut c_char) {
    let Some(data) = library_file(name).and_then(|p| std::fs::read(p).ok()) else { return };
    if buffer.is_null() {
        return;
    }
    let n = data.len().min(size as usize);
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buffer as *mut u8, n) };
}

type PerformFn = unsafe extern "C" fn(*const SkBitmap, *mut u32) -> *mut c_char;
type FreeFn = unsafe extern "C" fn(*mut c_char);

/// The library loaded and initialized, in this process (the helper's, or a test's).
struct Loaded {
    perform: PerformFn,
    free: FreeFn,
    version: (u32, u32),
    side: u32,
    // Kept loaded while its functions are called.
    _library: libloading::Library,
}

fn load(dir: &Path) -> Result<Loaded> {
    let p = package()?;
    let _ = FILES.set(dir.to_path_buf());
    let path = dir.join(p.library);
    #[cfg(unix)]
    let library: libloading::Library = unsafe {
        use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_LAZY};
        Library::open(Some(&path), RTLD_LAZY | RTLD_GLOBAL)
    }
    .with_context(|| format!("load {}", path.display()))?
    .into();
    #[cfg(not(unix))]
    let library = unsafe { libloading::Library::new(&path) }.with_context(|| format!("load {}", path.display()))?;

    type SizeFn = extern "C" fn(*const c_char) -> u32;
    type ContentFn = extern "C" fn(*const c_char, u32, *mut c_char);
    // Every symbol first: an older library without one fails here, cleanly.
    let (version, set_files, init, light_mode, max_side, perform, free) = unsafe {
        (
            *library.get::<unsafe extern "C" fn(*mut u32, *mut u32)>(b"GetLibraryVersion\0")?,
            *library.get::<unsafe extern "C" fn(SizeFn, ContentFn)>(b"SetFileContentFunctions\0")?,
            *library.get::<unsafe extern "C" fn() -> bool>(b"InitOCRUsingCallback\0")?,
            *library.get::<unsafe extern "C" fn(bool)>(b"SetOCRLightMode\0")?,
            *library.get::<unsafe extern "C" fn() -> u32>(b"GetMaxImageDimension\0")?,
            *library.get::<PerformFn>(b"PerformOCR\0")?,
            *library.get::<FreeFn>(b"FreeLibraryAllocatedCharArray\0")?,
        )
    };
    let (mut major, mut minor) = (0u32, 0u32);
    unsafe { version(&mut major, &mut minor) };
    if (major, minor) < MIN_VERSION {
        bail!("ScreenAI {major}.{minor} is older than {}.{}", MIN_VERSION.0, MIN_VERSION.1);
    }
    unsafe { set_files(file_size, file_content) };
    if !unsafe { init() } {
        bail!("ScreenAI did not initialize");
    }
    unsafe { light_mode(false) };
    let side = unsafe { max_side() };
    Ok(Loaded { perform, free, version: (major, minor), side, _library: library })
}

impl Loaded {
    /// The serialized annotation of RGBA `pixels`, `w` by `h`, or `None` when the library read nothing.
    fn read(&self, w: u32, h: u32, pixels: &[u8]) -> Option<Vec<u8>> {
        assert_eq!(pixels.len(), w as usize * h as usize * 4);
        let bitmap = SkBitmap {
            pixel_ref: std::ptr::null_mut(),
            pixels: pixels.as_ptr() as *const c_void,
            row_bytes: w as usize * 4,
            color_space: std::ptr::null_mut(),
            color_type: 4,
            alpha_type: 1,
            width: w as i32,
            height: h as i32,
            flags: 0,
        };
        let mut len = 0u32;
        let result = unsafe { (self.perform)(&bitmap, &mut len) };
        if result.is_null() {
            return None;
        }
        // The library's buffer, freed only by the library.
        let bytes = unsafe { std::slice::from_raw_parts(result as *const u8, len as usize) }.to_vec();
        unsafe { (self.free)(result) };
        Some(bytes)
    }
}

/// The helper process (`helper::main`): loads the library from `dir` and answers until stdin closes.
pub fn serve(dir: &Path) -> Result<()> {
    let library = load(dir)?;
    super::helper::serve(library.version, library.side, |w, h, pixels| library.read(w, h, pixels))
}

// ---------------------------------------------------------------------------------- the app side

pub struct ScreenAi {
    helper: Helper,
}

impl Default for ScreenAi {
    fn default() -> Self {
        Self { helper: Helper::new("Chrome ScreenAI", "screenai") }
    }
}

impl ScreenAi {
    /// Reads the whole image: its lines in capture pixels, each character on its own box.
    pub fn recognize(&self, img: &RgbaImage) -> Result<Vec<OcrLine>> {
        if !installed() {
            bail!("os arquivos dele não estão baixados (Configurações › OCR)");
        }
        let (answer, k) = self.helper.ask(&resources()?, img)?;
        let Some(bytes) = answer else { bail!("a biblioteca não leu esta imagem") };
        let annotation = VisualAnnotation::decode(bytes.as_slice()).context("decode ScreenAI's answer")?;
        Ok(to_lines(&annotation, k))
    }

    /// Ends the helper now (before its files are deleted).
    pub fn stop(&self) {
        self.helper.stop();
    }
}

// ------------------------------------------------------------------------------- the annotation

// The fields Lookitan reads of services/screen_ai/proto/chrome_screen_ai.proto (Chromium,
// BSD-style licence); prost skips the others.

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct VisualAnnotation {
    #[prost(message, repeated, tag = "2")]
    pub lines: Vec<LineBox>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct LineBox {
    #[prost(message, repeated, tag = "1")]
    pub words: Vec<WordBox>,
    #[prost(message, optional, tag = "2")]
    pub bounding_box: Option<Rect>,
    #[prost(string, tag = "3")]
    pub utf8_string: String,
    #[prost(string, tag = "4")]
    pub language: String,
    /// `Direction`: 1 left to right, 2 right to left, 3 top to bottom.
    #[prost(int32, tag = "7")]
    pub direction: i32,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WordBox {
    #[prost(message, repeated, tag = "1")]
    pub symbols: Vec<SymbolBox>,
    #[prost(message, optional, tag = "2")]
    pub bounding_box: Option<Rect>,
    #[prost(string, tag = "3")]
    pub utf8_string: String,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SymbolBox {
    #[prost(message, optional, tag = "1")]
    pub bounding_box: Option<Rect>,
    #[prost(string, tag = "2")]
    pub utf8_string: String,
}

/// A box in image pixels: its top left corner, its size, and its turn in degrees clockwise about
/// that corner.
#[derive(Clone, Copy, PartialEq, ::prost::Message)]
pub struct Rect {
    #[prost(int32, tag = "1")]
    pub x: i32,
    #[prost(int32, tag = "2")]
    pub y: i32,
    #[prost(int32, tag = "3")]
    pub width: i32,
    #[prost(int32, tag = "4")]
    pub height: i32,
    #[prost(float, tag = "5")]
    pub angle: f32,
}

const TOP_TO_BOTTOM: i32 = 3;

/// The annotation's lines in capture pixels (`k` capture pixels to one of the image read). A level
/// line keeps its characters' boxes across (down a column); a turned one is given in its own frame
/// (`Tilt`). Its text is the library's, each character on its symbol's span; the spaces between
/// words, which no symbol has, sit at the end of the character before them.
pub fn to_lines(annotation: &VisualAnnotation, k: f32) -> Vec<OcrLine> {
    let mut out = Vec::new();
    for line in &annotation.lines {
        let Some(r) = line.bounding_box else { continue };
        if line.utf8_string.trim().is_empty() {
            continue;
        }
        let flow = if line.direction == TOP_TO_BOTTOM { Flow::Down } else { Flow::Across };
        let angle = r.angle.to_radians();
        let corners = |b: &Rect| -> [(f32, f32); 4] {
            let (s, c) = b.angle.to_radians().sin_cos();
            let (x, y, w, h) = (b.x as f32 * k, b.y as f32 * k, b.width as f32 * k, b.height as f32 * k);
            [(x, y), (x + w * c, y + w * s), (x + w * c - h * s, y + w * s + h * c), (x - h * s, y + h * c)]
        };
        let bounds = |p: [(f32, f32); 4]| {
            let xs = p.map(|q| q.0);
            let ys = p.map(|q| q.1);
            (xs.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MAX, f32::min),
             xs.iter().copied().fold(f32::MIN, f32::max), ys.iter().copied().fold(f32::MIN, f32::max))
        };
        let (x0, y0, x1, y1) = bounds(corners(&r));
        let tilt = (angle.abs() > LEVEL).then(|| {
            let frame = Tilt { angle, rect: (0.0, 0.0, 0.0, 0.0) };
            let (fx, fy) = frame.to_frame(r.x as f32 * k, r.y as f32 * k);
            Tilt { angle, rect: (fx, fy, fx + r.width as f32 * k, fy + r.height as f32 * k) }
        });
        // Each symbol's span along the line: in the line's frame when it is turned.
        let span = |b: &Rect| -> (f32, f32) {
            match &tilt {
                Some(t) => {
                    let (fx, fy) = t.to_frame(b.x as f32 * k, b.y as f32 * k);
                    let (from, len) = if flow == Flow::Down { (fy, b.height as f32 * k) } else { (fx, b.width as f32 * k) };
                    (from, from + len)
                }
                None => {
                    let (a, c, d, e) = bounds(corners(b));
                    if flow == Flow::Down { (c, e) } else { (a, d) }
                }
            }
        };
        let mut glyphs: Vec<(char, f32, f32)> = Vec::new();
        for word in &line.words {
            for symbol in &word.symbols {
                let Some(b) = symbol.bounding_box else { continue };
                let (from, to) = span(&b);
                let letters: Vec<char> = symbol.utf8_string.chars().collect();
                // A symbol of several characters (a letter and its marks) shares its span.
                let step = (to - from) / letters.len().max(1) as f32;
                for (i, ch) in letters.into_iter().enumerate() {
                    glyphs.push((ch, from + step * i as f32, from + step * (i + 1) as f32));
                }
            }
        }
        let chars = align(&line.utf8_string, &glyphs);
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

    fn rect(x: i32, y: i32, width: i32, height: i32, angle: f32) -> Option<Rect> {
        Some(Rect { x, y, width, height, angle })
    }

    fn symbol(s: &str, x: i32, y: i32, w: i32, h: i32) -> SymbolBox {
        SymbolBox { bounding_box: rect(x, y, w, h, 0.0), utf8_string: s.into() }
    }

    #[test]
    fn symbols_give_each_character_its_span_and_spaces_sit_between_words() {
        let line = LineBox {
            words: vec![
                WordBox { symbols: vec![symbol("N", 10, 5, 10, 20), symbol("ã", 20, 5, 9, 20), symbol("o", 30, 5, 9, 20)], ..Default::default() },
                WordBox { symbols: vec![symbol("há", 50, 5, 20, 20)], ..Default::default() },
            ],
            bounding_box: rect(10, 5, 60, 20, 0.0),
            utf8_string: "Não há".into(),
            language: "pt".into(),
            direction: 1,
        };
        let lines = to_lines(&VisualAnnotation { lines: vec![line] }, 2.0);
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert_eq!(l.text, "Não há");
        assert_eq!((l.x0, l.y0, l.x1, l.y1), (20.0, 10.0, 140.0, 50.0), "capture pixels, twice the image's");
        assert!(l.tilt.is_none() && l.flow == Flow::Across);
        let spans: Vec<(char, f32, f32)> = l.chars.iter().map(|c| (c.ch, c.x0, c.x1)).collect();
        assert_eq!(spans[0], ('N', 20.0, 40.0));
        assert_eq!(spans[3], (' ', 78.0, 78.0), "the space at the end of the word before it");
        // "há" is one symbol: its two characters share it.
        assert_eq!(spans[4], ('h', 100.0, 120.0));
        assert_eq!(spans[5], ('á', 120.0, 140.0));
    }

    #[test]
    fn a_column_runs_down_and_a_turned_line_keeps_its_frame() {
        let column = LineBox {
            words: vec![WordBox { symbols: vec![symbol("日", 10, 10, 20, 20), symbol("本", 10, 32, 20, 20)], ..Default::default() }],
            bounding_box: rect(10, 10, 20, 42, 0.0),
            utf8_string: "日本".into(),
            direction: TOP_TO_BOTTOM,
            ..Default::default()
        };
        let turned = LineBox {
            words: vec![WordBox { symbols: vec![SymbolBox { bounding_box: rect(100, 100, 20, 20, 30.0), utf8_string: "A".into() }], ..Default::default() }],
            bounding_box: rect(100, 100, 40, 20, 30.0),
            utf8_string: "A".into(),
            direction: 1,
            ..Default::default()
        };
        let lines = to_lines(&VisualAnnotation { lines: vec![column, turned] }, 1.0);
        assert_eq!(lines[0].flow, Flow::Down);
        assert_eq!((lines[0].chars[1].x0, lines[0].chars[1].x1), (32.0, 52.0), "y spans down the column");
        let t = lines[1].tilt.expect("turned 30°");
        assert!((t.angle - 30f32.to_radians()).abs() < 1e-5);
        let c = &lines[1].chars[0];
        assert!((c.x0 - t.rect.0).abs() < 1e-3 && (c.x1 - c.x0 - 20.0).abs() < 1e-3, "in the line's frame");
    }

    #[test]
    fn symbols_that_do_not_spell_the_text_are_the_line_and_odd_paths_are_refused() {
        let chars = align("ab", &[('x', 0.0, 5.0)]);
        assert_eq!(chars.iter().map(|c| c.ch).collect::<String>(), "x");
        let _ = FILES.set(PathBuf::from("models"));
        let path = |s: &str| library_file(std::ffi::CString::new(s).unwrap().as_ptr());
        assert_eq!(path("/gocr/a.tflite"), Some(PathBuf::from("models").join("gocr/a.tflite")));
        assert_eq!(path("../secret"), None);
    }

    /// The pinned library itself, loaded in this process, on the Russian and Japanese fixtures.
    /// Needs the package unpacked in `LOOKITAN_SCREENAI` (its `resources/` folder; CI fetches it);
    /// skipped without it. Run it alone: the library can be initialized once per process.
    #[test]
    #[ignore]
    fn real_screenai_reads_the_fixtures() {
        use unicode_normalization::UnicodeNormalization;
        let Some(dir) = std::env::var_os("LOOKITAN_SCREENAI").map(PathBuf::from) else {
            eprintln!("LOOKITAN_SCREENAI is not set; skipped");
            return;
        };
        let _ = FILES.set(dir.clone());
        let library = load(&dir).expect("load ScreenAI");
        assert!(library.version >= MIN_VERSION, "{:?}", library.version);
        let plain = |s: &str| s.nfkc().filter(|c| !c.is_whitespace()).collect::<String>();
        let cer = |got: &str, want: &str| {
            let (a, b): (Vec<char>, Vec<char>) = (plain(got).chars().collect(), plain(want).chars().collect());
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
        let fixtures: [(&[u8], &str); 2] = [
            (include_bytes!("fixtures/ru_lines.png"), "Съешь же ещё этих мягких французских булок ВСЁ ЕЩЁ ЗДЕСЬ, а ёлка — в лесу. Шелестят листья!"),
            (include_bytes!("fixtures/ja_lines.png"), &japanese),
        ];
        for (png, want) in fixtures {
            let img = image::load_from_memory(png).unwrap().to_rgba8();
            let started = std::time::Instant::now();
            let bytes = library.read(img.width(), img.height(), img.as_raw()).expect("an annotation");
            let lines = to_lines(&VisualAnnotation::decode(bytes.as_slice()).unwrap(), 1.0);
            let got: String = lines.iter().map(|l| l.text.as_str()).collect();
            let e = cer(&got, want);
            eprintln!("ScreenAI {:?}: {} lines in {} ms, CER {:.1}%\n{got}", library.version, lines.len(), started.elapsed().as_millis(), e * 100.0);
            assert!(e <= 0.06, "CER {e}");
            for l in &lines {
                assert!(l.chars.iter().all(|c| c.x0 <= c.x1), "{}", l.text);
                assert!(l.x0 < l.x1 && l.y0 < l.y1 && l.x1 <= img.width() as f32 + 1.0 && l.y1 <= img.height() as f32 + 1.0);
            }
        }
    }

    #[test]
    fn the_pinned_packages_are_whole_instances() {
        if let Ok(p) = package() {
            assert_eq!(p.digest.len(), 64);
            assert!(p.digest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
            assert!(p.bytes > 50_000_000);
        }
        assert_eq!(std::mem::size_of::<SkBitmap>(), 56, "the layout owocr passes");
    }
}
