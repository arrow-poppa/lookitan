//! Korean words as Lookupper finds them (REA on 2026-10-21, `0x1401a00f0`, `0x1401a03c0`): Kiwi
//! 0.23.2 (Apache-2.0) through its C API, loaded at run time like Lookupper's `kiwi.dll`, with the
//! options Lookupper passes: `kiwi_init(model, 1 thread, KIWI_BUILD_DEFAULT, no dialects)` and
//! `kiwi_analyze_w(top_n 1, KIWI_MATCH_ALL_WITH_NORMALIZING, dialect cost 3, typo threshold 2.5)`.
//!
//! The library and the base model come pinned from Kiwi's GitHub release; only the library file
//! and the model are kept from the archives. Other platforms keep mecab-ko-dic (mod.rs).

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;

use super::Morpheme;

pub(super) const DIR: &str = "kiwi-0.23.2";
const RELEASE: &str = "https://github.com/bab2min/Kiwi/releases/download/v0.23.2";

/// A file of Kiwi's release: its name, size and SHA-256.
struct Asset {
    name: &'static str,
    bytes: u64,
    sha256: &'static str,
}

const MODEL: Asset = Asset { name: "kiwi_model_v0.23.2_base.tgz", bytes: 88_069_580, sha256: "15f35787ab07281688a321f5d6bc24a117008190d9fcccb11dca188e81eba814" };
const MODEL_PREFIX: &str = "models/cong/base/";

/// The library for this platform: the release archive, the file in it and its name on disk.
#[cfg(all(windows, target_arch = "x86_64"))]
const LIBRARY: Option<(Asset, &str, &str)> = Some((
    Asset { name: "kiwi_win_x64_v0.23.2.zip", bytes: 36_847_207, sha256: "9881e56ce96f9483bfe4baada42b5c73d97b840809c6143f0d8b346dc9c2981b" },
    "lib/kiwi.dll",
    "kiwi.dll",
));
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const LIBRARY: Option<(Asset, &str, &str)> = Some((
    Asset { name: "kiwi_lnx_x86_64_v0.23.2.tgz", bytes: 74_416_311, sha256: "0b6694a795891de22fb14ae46825403af02063450126282c18448d6562b97174" },
    "lib/libkiwi.so.0.23.2",
    "libkiwi.so",
));
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const LIBRARY: Option<(Asset, &str, &str)> = Some((
    Asset { name: "kiwi_lnx_aarch64_v0.23.2.tgz", bytes: 33_483_495, sha256: "7e093121a367087d21e7c696bcc69a505935b07798d1e95c87f3b66a646c124e" },
    "lib/libkiwi.so.0.23.2",
    "libkiwi.so",
));
#[cfg(not(any(all(windows, target_arch = "x86_64"), all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))))]
const LIBRARY: Option<(Asset, &str, &str)> = None;

/// Whether Kiwi's release has a build for this platform.
pub fn available() -> bool {
    LIBRARY.is_some()
}

/// About what the downloads weigh.
pub fn bytes() -> u64 {
    MODEL.bytes + LIBRARY.as_ref().map_or(0, |(a, _, _)| a.bytes)
}

const NOTICE: &str = "Kiwi 0.23.2 and its base model (https://github.com/bab2min/Kiwi), Copyright (c) 2017 Minchul Lee,\nunder the Apache License 2.0.\n";

/// Kiwi's folder (`model/` and the library), downloaded first if it is not there.
pub fn ensure(root: &Path, progress: &dyn Fn(u64)) -> Result<PathBuf> {
    let Some((library, member, file)) = LIBRARY.as_ref() else { bail!("Kiwi has no build for this platform") };
    let dir = root.join(DIR);
    if dir.join(file).is_file() && dir.join("model").join("cong.mdl").is_file() {
        return Ok(dir);
    }
    std::fs::create_dir_all(root)?;
    // lindera's ko-dic, which this replaces.
    let _ = std::fs::remove_dir_all(root.join("lindera-ko-dic-6.2.0"));
    for entry in std::fs::read_dir(root)?.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&format!(".building-{DIR}-")) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let building = root.join(format!(".building-{DIR}-{}", std::process::id()));
    std::fs::create_dir_all(building.join("model"))?;
    let built = (|| -> Result<()> {
        let archive = building.join(library.name);
        super::download(&format!("{RELEASE}/{}", library.name), &archive, library.bytes, Some(library.sha256), progress)?;
        let lib_path = building.join(file);
        let found = extract(&archive, &mut |name: &str, reader: &mut dyn Read| {
            if name == *member {
                std::io::copy(reader, &mut std::fs::File::create(&lib_path)?)?;
            }
            Ok(())
        })?;
        let _ = std::fs::remove_file(&archive);
        if !found.iter().any(|n| n.as_str() == *member) {
            bail!("{} has no {member}", library.name);
        }
        let archive = building.join(MODEL.name);
        super::download(&format!("{RELEASE}/{}", MODEL.name), &archive, MODEL.bytes, Some(MODEL.sha256), &|done| progress(library.bytes + done))?;
        let model = building.join("model");
        extract(&archive, &mut |name: &str, reader: &mut dyn Read| {
            if let Some(rest) = name.strip_prefix(MODEL_PREFIX).filter(|r| !r.is_empty() && !r.contains(['/', '\\'])) {
                std::io::copy(reader, &mut std::fs::File::create(model.join(rest))?)?;
            }
            Ok(())
        })?;
        let _ = std::fs::remove_file(&archive);
        if !model.join("cong.mdl").is_file() {
            bail!("{} has no {MODEL_PREFIX}cong.mdl", MODEL.name);
        }
        std::fs::write(building.join("NOTICE.txt"), NOTICE)?;
        Ok(())
    })();
    if let Err(e) = built {
        let _ = std::fs::remove_dir_all(&building);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&building, &dir).with_context(|| format!("move {DIR} into place"))?;
    Ok(dir)
}

/// Hands each regular file of a zip or .tgz to `each` by its path in the archive; returns the paths.
fn extract(path: &Path, each: &mut dyn FnMut(&str, &mut dyn Read) -> Result<()>) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let file = std::fs::File::open(path)?;
    if path.extension().is_some_and(|e| e == "zip") {
        let mut zip = zip::ZipArchive::new(file)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.is_file() {
                let name = entry.name().to_string();
                each(&name, &mut entry)?;
                names.push(name);
            }
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        for entry in tar.entries()? {
            let mut entry = entry?;
            if entry.header().entry_type().is_file() {
                let name = entry.path()?.to_string_lossy().trim_start_matches("./").to_string();
                each(&name, &mut entry)?;
                names.push(name);
            }
        }
    }
    Ok(names)
}

// Kiwi's C API (include/kiwi/capi.h of 0.23.2).

#[repr(C)]
struct AnalyzeOption {
    match_options: c_int,
    blocklist: *mut c_void,
    open_ending: c_int,
    allowed_dialects: c_int,
    dialect_cost: f32,
    typo_transformer: *mut c_void,
    typo_threshold: f32,
}

const KIWI_BUILD_DEFAULT: c_int = 15;
const KIWI_MATCH_ALL_WITH_NORMALIZING: c_int = 0x81003f;

type Init = unsafe extern "C" fn(*const c_char, c_int, c_int, c_int) -> *mut c_void;
type Close = unsafe extern "C" fn(*mut c_void) -> c_int;
type AnalyzeW = unsafe extern "C" fn(*mut c_void, *const u16, c_int, AnalyzeOption, *mut c_void) -> *mut c_void;
type WordNum = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type FormW = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> *const u16;
type Tag = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> *const c_char;
type Int = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
type Error = unsafe extern "C" fn() -> *const c_char;

pub struct Kiwi {
    handle: *mut c_void,
    close: Close,
    analyze: AnalyzeW,
    word_num: WordNum,
    form: FormW,
    tag: Tag,
    position: Int,
    length: Int,
    res_close: Close,
    /// One analysis at a time.
    busy: Mutex<()>,
    _library: libloading::Library,
}

// The handle is only used under `busy`, and Kiwi's analysis does not touch thread-local state.
unsafe impl Send for Kiwi {}
unsafe impl Sync for Kiwi {}

impl Drop for Kiwi {
    fn drop(&mut self) {
        unsafe { (self.close)(self.handle) };
    }
}

/// Kiwi with the library and model in `dir`.
pub fn open(dir: &Path) -> Result<Kiwi> {
    let Some((_, _, file)) = LIBRARY.as_ref() else { bail!("Kiwi has no build for this platform") };
    unsafe {
        let library = libloading::Library::new(dir.join(file)).with_context(|| format!("load {}", dir.join(file).display()))?;
        let init = *library.get::<Init>(b"kiwi_init\0")?;
        let error = *library.get::<Error>(b"kiwi_error\0")?;
        let model = CString::new(dir.join("model").to_string_lossy().into_owned())?;
        let handle = init(model.as_ptr(), 1, KIWI_BUILD_DEFAULT, 0);
        if handle.is_null() {
            let e = error();
            let e = if e.is_null() { "unknown error".into() } else { CStr::from_ptr(e).to_string_lossy() };
            return Err(anyhow!("Kiwi could not open its model: {e}"));
        }
        Ok(Kiwi {
            handle,
            close: *library.get::<Close>(b"kiwi_close\0")?,
            analyze: *library.get::<AnalyzeW>(b"kiwi_analyze_w\0")?,
            word_num: *library.get::<WordNum>(b"kiwi_res_word_num\0")?,
            form: *library.get::<FormW>(b"kiwi_res_form_w\0")?,
            tag: *library.get::<Tag>(b"kiwi_res_tag\0")?,
            position: *library.get::<Int>(b"kiwi_res_position\0")?,
            length: *library.get::<Int>(b"kiwi_res_length\0")?,
            res_close: *library.get::<Close>(b"kiwi_res_close\0")?,
            busy: Mutex::new(()),
            _library: library,
        })
    }
}

/// A Kiwi tag as korean.rs reads it: the Sejong tag without Kiwi's regular/irregular mark
/// (`VV-I` → `VV`); web and other special tokens read as foreign words.
pub fn sejong(tag: &str) -> &str {
    let tag = tag.split('-').next().unwrap_or(tag);
    if tag.starts_with("W_") || tag.starts_with("USER") { "SL" } else { tag }
}

impl Kiwi {
    /// The best analysis of `text`: each morpheme in its base form (했 → 하 + 었), at the
    /// characters it comes from (morphemes of one contracted syllable share them).
    pub fn morphemes(&self, text: &str) -> Vec<Morpheme> {
        let mut utf16: Vec<u16> = text.encode_utf16().collect();
        // A UTF-16 position to characters.
        let mut char_at = Vec::with_capacity(utf16.len() + 1);
        for (i, c) in text.chars().enumerate() {
            char_at.extend(std::iter::repeat_n(i, c.len_utf16()));
        }
        char_at.push(text.chars().count());
        utf16.push(0);
        let option = AnalyzeOption {
            match_options: KIWI_MATCH_ALL_WITH_NORMALIZING,
            blocklist: std::ptr::null_mut(),
            open_ending: 0,
            allowed_dialects: 0,
            dialect_cost: 3.0,
            typo_transformer: std::ptr::null_mut(),
            typo_threshold: 2.5,
        };
        let _busy = self.busy.lock();
        let mut out = Vec::new();
        unsafe {
            let res = (self.analyze)(self.handle, utf16.as_ptr(), 1, option, std::ptr::null_mut());
            if res.is_null() {
                log::warn!("Kiwi could not analyze the text");
                return out;
            }
            let at = |p: c_int| char_at[(p.max(0) as usize).min(char_at.len() - 1)];
            for i in 0..(self.word_num)(res, 0).max(0) {
                let form = (self.form)(res, 0, i);
                let tag = (self.tag)(res, 0, i);
                if form.is_null() || tag.is_null() {
                    continue;
                }
                let n = (0..).take_while(|&k| *form.add(k) != 0).count();
                let surface = String::from_utf16_lossy(std::slice::from_raw_parts(form, n));
                let tag = sejong(&CStr::from_ptr(tag).to_string_lossy()).to_string();
                let (p, l) = ((self.position)(res, 0, i), (self.length)(res, 0, i));
                out.push(Morpheme { start: at(p), end: at(p + l), surface, details: vec![tag] });
            }
            (self.res_close)(res);
        }
        out
    }
}
