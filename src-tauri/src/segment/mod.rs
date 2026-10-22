//! Words of Japanese, Chinese and Korean text, as Lookupper finds them with its Sudachi, jieba and
//! Kiwi analyzers: the paragraph under the pointer is split into words, so the pop-up looks up the
//! whole word the pointer is on (by its dictionary form, 食べさせられた → 食べる) and the overlay
//! outlines each word. Without an analyzer, words.rs falls back to the longest dictionary match from
//! the pointer.
//!
//! Japanese goes through Sudachi with Lookupper's SudachiDict (sudachidict.rs), Chinese through
//! jieba-rs with Lookupper's dictionary (chinese.rs). Korean goes through lindera 6.2 (MIT) with
//! the prebuilt mecab-ko-dic (Apache-2.0) of its GitHub release, whose Sejong tags are the ones
//! Kiwi uses. All are downloaded on first use, like Lookupper's models, and checked against pinned
//! SHA-256s where the source allows it. Each dictionary keeps its NOTICE.txt. Only the recognition
//! language's dictionary is loaded; Sudachi's and lindera's are memory-mapped. How morphemes form
//! words (japanese.rs, korean.rs) is Lookitan's own code, written to match what Lookupper shows.

mod chinese;
mod japanese;
mod korean;
mod sudachidict;

use std::borrow::Cow;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::{Mutex, RwLock};
use sha2::{Digest, Sha256};

/// What a word is, for the overlay and the lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A content word with what is attached to it (a verb with its endings, a noun with a suffix).
    Word,
    /// A particle or an auxiliary on its own.
    Function,
    /// A number, with its counter.
    Number,
    /// Punctuation and symbols: not a word.
    Punct,
}

/// A word of the analyzed text, in characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
    /// Dictionary forms to look up before the word as written, best first.
    pub lemmas: Vec<String>,
}

/// A morpheme from the analyzer, in characters, with its dictionary's detail fields.
#[derive(Clone, Debug)]
pub struct Morpheme {
    pub start: usize,
    pub end: usize,
    pub surface: String,
    pub details: Vec<String>,
}

impl Morpheme {
    fn detail(&self, i: usize) -> &str {
        self.details.get(i).map_or("*", String::as_str)
    }
}

/// Splits text into words.
pub trait Segment: Send + Sync {
    fn words(&self, text: &str) -> Vec<Word>;
    /// Whether a word with this character is found through the analyzer.
    fn handles(&self, c: char) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Japanese,
    Chinese,
    Korean,
}

impl Lang {
    /// The analyzed language for a BCP 47 tag ("zh-Hant-TW" is Chinese).
    pub fn of(tag: &str) -> Option<Lang> {
        match tag.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase().as_str() {
            "ja" => Some(Lang::Japanese),
            "zh" => Some(Lang::Chinese),
            "ko" => Some(Lang::Korean),
            _ => None,
        }
    }

    /// lindera's dictionary, for the languages that use one.
    fn pack(self) -> Option<&'static Pack> {
        PACKS.iter().find(|p| p.lang == self)
    }

    /// What Settings calls the language's dictionary.
    fn label(self) -> &'static str {
        match self {
            Lang::Japanese => "Sudachi",
            Lang::Chinese => "jieba + CC-CEDICT",
            Lang::Korean => self.pack().map_or("", |p| p.label),
        }
    }

    /// About what its download weighs.
    fn bytes(self) -> u64 {
        match self {
            Lang::Japanese => sudachidict::WHEEL_BYTES,
            Lang::Chinese => chinese::BYTES,
            Lang::Korean => self.pack().map_or(0, |p| p.bytes),
        }
    }

    /// Its folder, downloaded and unpacked first if it is not there.
    fn ensure(self, root: &Path, progress: &dyn Fn(u64)) -> Result<PathBuf> {
        match (self, self.pack()) {
            (Lang::Japanese, _) => sudachidict::ensure(root, progress),
            (Lang::Chinese, _) => chinese::ensure(root, progress),
            (_, Some(pack)) => ensure(root, pack, progress),
            (_, None) => bail!("no dictionary for {self:?}"),
        }
    }

    /// Kana and CJK ideographs for Japanese and Chinese, Hangul for Korean.
    pub fn handles(self, c: char) -> bool {
        match self {
            Lang::Japanese | Lang::Chinese => matches!(c as u32,
                0x3040..=0x30FF | 0x31F0..=0x31FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF66..=0xFF9F),
            Lang::Korean => matches!(c as u32, 0xAC00..=0xD7A3 | 0x1100..=0x11FF | 0x3130..=0x318F),
        }
    }
}

/// Groups morphemes into words.
fn group(lang: Lang, text: &[char], morphemes: &[Morpheme]) -> Vec<Word> {
    match lang {
        Lang::Japanese => japanese::words(text, morphemes),
        Lang::Korean => korean::words(text, morphemes),
        Lang::Chinese => morphemes.iter().map(chinese_word).collect(),
    }
}

/// jieba's words are dictionary words already, as in Lookupper, whose lemma is the word itself.
fn chinese_word(m: &Morpheme) -> Word {
    let punct = !m.surface.chars().any(char::is_alphanumeric);
    Word { start: m.start, end: m.end, kind: if punct { Kind::Punct } else { Kind::Word }, lemmas: Vec::new() }
}

// ---------------------------------------------------------------------------------------------
// The analyzers

enum Engine {
    Lindera(lindera::segmenter::Segmenter),
    Jieba(jieba_rs::Jieba),
    Sudachi(sudachidict::Tokenizer),
}

pub struct Analyzer {
    lang: Lang,
    engine: Engine,
}

impl Analyzer {
    pub fn open(lang: Lang, dir: &Path) -> Result<Self> {
        let engine = match lang {
            Lang::Chinese => Engine::Jieba(chinese::open(dir)?),
            Lang::Japanese => Engine::Sudachi(sudachidict::open(dir)?),
            Lang::Korean => {
                let dictionary = lindera::dictionary::load_fs_dictionary(dir).map_err(|e| anyhow!("{e}"))?;
                Engine::Lindera(lindera::segmenter::Segmenter::new(lindera::mode::Mode::Normal, dictionary, None))
            }
        };
        Ok(Self { lang, engine })
    }

    fn morphemes(&self, text: &str) -> Vec<Morpheme> {
        let segmenter = match &self.engine {
            Engine::Lindera(segmenter) => segmenter,
            // jieba's `cut` with its HMM, as Lookupper's; its offsets are in characters already.
            Engine::Jieba(jieba) => {
                return jieba
                    .cut(text, true)
                    .into_iter()
                    .map(|t| Morpheme { start: t.start, end: t.end, surface: t.word.to_string(), details: Vec::new() })
                    .collect();
            }
            // Split mode C, as Lookupper's; details as japanese.rs reads them.
            Engine::Sudachi(tokenizer) => {
                use sudachi::analysis::{Mode, Tokenize};
                let list = match tokenizer.tokenize(text, Mode::C, false) {
                    Ok(list) => list,
                    Err(e) => {
                        log::warn!("word analysis failed: {e}");
                        return Vec::new();
                    }
                };
                return list
                    .iter()
                    .map(|m| {
                        let surface = m.surface().to_string();
                        let mut details = vec![surface.clone()];
                        details.extend(m.part_of_speech().iter().cloned());
                        details.extend([m.reading_form(), m.normalized_form(), m.dictionary_form()].map(str::to_string));
                        Morpheme { start: m.begin_c(), end: m.end_c(), surface, details }
                    })
                    .collect();
            }
        };
        let mut tokens = match segmenter.segment(Cow::Borrowed(text)) {
            Ok(tokens) => tokens,
            Err(e) => {
                log::warn!("word analysis failed: {e}");
                return Vec::new();
            }
        };
        // Byte offsets to character offsets.
        let mut char_at = vec![0usize; text.len() + 1];
        let mut n = 0;
        for (b, _) in text.char_indices() {
            char_at[b] = n;
            n += 1;
        }
        char_at[text.len()] = n;
        tokens
            .iter_mut()
            .map(|t| {
                let (start, end) = (char_at[t.byte_start.min(text.len())], char_at[t.byte_end.min(text.len())]);
                let surface = t.surface.to_string();
                let details = t.details().into_iter().map(str::to_string).collect();
                Morpheme { start, end, surface, details }
            })
            .collect()
    }
}

impl Segment for Analyzer {
    fn words(&self, text: &str) -> Vec<Word> {
        let chars: Vec<char> = text.chars().collect();
        group(self.lang, &chars, &self.morphemes(text))
    }

    fn handles(&self, c: char) -> bool {
        self.lang.handles(c)
    }
}

// ---------------------------------------------------------------------------------------------
// Dictionaries: downloaded once into the local data folder, one loaded at a time

/// The lindera release the dictionaries come from; they must match the lindera crate's version.
const RELEASE: &str = "https://github.com/lindera/lindera/releases/download/v6.2.0";
const VERSION: &str = "6.2.0";

struct Pack {
    lang: Lang,
    /// The zip's name and the folder inside it.
    name: &'static str,
    /// What Settings calls it.
    label: &'static str,
    sha256: &'static str,
    bytes: u64,
}

static PACKS: [Pack; 1] = [
    Pack { lang: Lang::Korean, name: "lindera-ko-dic", label: "mecab-ko-dic", bytes: 19_876_022,
        sha256: "3458c54ce21618693221ad9af2a49f547b40f0e50a7e19ca20cfa781673f3ed0" },
];

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static LOADED: RwLock<Option<(Lang, Arc<Analyzer>)>> = RwLock::new(None);
/// What Settings shows for the recognition language's analyzer.
static STATUS: RwLock<Option<(Lang, String)>> = RwLock::new(None);
/// One download or load at a time.
static WARMING: Mutex<()> = Mutex::new(());

/// Where the dictionaries are kept (`<local data>/segmenters`); set once at start-up.
pub fn set_root(dir: PathBuf) {
    let _ = ROOT.set(dir);
}

/// The analyzer for the recognition language, once it is loaded.
pub fn current(tag: &str) -> Option<Arc<Analyzer>> {
    let lang = Lang::of(tag)?;
    LOADED.read().as_ref().filter(|(l, _)| *l == lang).map(|(_, a)| a.clone())
}

/// The analyzer's state for Settings › OCR, for languages that have one.
pub fn status(tag: &str) -> Option<String> {
    let lang = Lang::of(tag)?;
    let label = lang.label();
    match STATUS.read().as_ref() {
        Some((l, s)) if *l == lang => Some(s.clone()),
        _ => Some(format!("separação de palavras ({label}): aguardando")),
    }
}

fn set_status(lang: Lang, text: String, changed: &dyn Fn()) {
    *STATUS.write() = Some((lang, text));
    changed();
}

/// Downloads the recognition language's dictionary if needed and loads it, dropping the previous
/// language's. Blocks; `changed` runs whenever the status changes.
pub fn warm(tag: &str, changed: &dyn Fn()) {
    let Some(lang) = Lang::of(tag) else {
        *LOADED.write() = None;
        return;
    };
    let _gate = WARMING.lock();
    if LOADED.read().as_ref().is_some_and(|(l, _)| *l == lang) {
        return;
    }
    *LOADED.write() = None;
    let Some(root) = ROOT.get() else { return };
    let (label, bytes) = (lang.label(), lang.bytes());
    match lang
        .ensure(root, &|done| {
            let percent = (done * 100 / bytes.max(1)).min(100);
            set_status(lang, format!("separação de palavras ({label}): baixando {percent}% de {} MB", bytes / 1_000_000), changed);
        })
        .and_then(|dir| {
            set_status(lang, format!("separação de palavras ({label}): carregando…"), changed);
            Analyzer::open(lang, &dir)
        }) {
        Ok(analyzer) => {
            *LOADED.write() = Some((lang, Arc::new(analyzer)));
            set_status(lang, format!("separação de palavras ({label}): pronta"), changed);
        }
        Err(e) => {
            log::warn!("{label} unavailable: {e:#}");
            set_status(lang, format!("separação de palavras ({label}) indisponível: {e:#}"), changed);
        }
    }
}

/// The unpacked dictionary folder, downloaded and unpacked first if it is not there.
fn ensure(root: &Path, pack: &Pack, progress: &dyn Fn(u64)) -> Result<PathBuf> {
    let home = root.join(format!("{}-{VERSION}", pack.name));
    let dir = home.join(pack.name);
    if dir.join("metadata.json").is_file() {
        return Ok(dir);
    }
    std::fs::create_dir_all(root)?;
    // What an interrupted unpacking left behind.
    for entry in std::fs::read_dir(root)?.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&format!(".unpacking-{}-", pack.name)) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let zip = root.join(format!(".download-{}.zip", pack.name));
    download(&format!("{RELEASE}/{}-{VERSION}.zip", pack.name), &zip, pack.bytes, Some(pack.sha256), progress)?;
    let unpacking = root.join(format!(".unpacking-{}-{}", pack.name, std::process::id()));
    std::fs::create_dir_all(&unpacking)?;
    let unpacked = crate::dictionaries::archive::unpack(&zip, crate::dictionaries::archive::Kind::Zip, &unpacking, &|_| {});
    let _ = std::fs::remove_file(&zip);
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&unpacking);
        return Err(e);
    }
    if !unpacking.join(pack.name).join("metadata.json").is_file() {
        let _ = std::fs::remove_dir_all(&unpacking);
        bail!("{} has no {}/metadata.json", pack.name, pack.name);
    }
    let _ = std::fs::remove_dir_all(&home);
    std::fs::rename(&unpacking, &home).with_context(|| format!("move {} into place", pack.name))?;
    Ok(dir)
}

/// Downloads `url` to `to`. With a pinned SHA-256 the file must be exactly `bytes` long and match
/// it; without one, `bytes` is only the most it may weigh.
fn download(url: &str, to: &Path, bytes: u64, sha256: Option<&str>, progress: &dyn Fn(u64)) -> Result<()> {
    let client = reqwest::blocking::Client::builder().timeout(DOWNLOAD_TIMEOUT).build()?;
    let mut response = client.get(url).send()?.error_for_status()?;
    let mut file = File::create(to).with_context(|| format!("create {}", to.display()))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let (mut done, mut reported) = (0u64, 0u64);
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > bytes {
            bail!("{url} is larger than expected");
        }
        hash.update(&buf[..n]);
        file.write_all(&buf[..n])?;
        if done - reported >= bytes / 50 {
            reported = done;
            progress(done);
        }
    }
    file.flush()?;
    drop(file);
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if let Some(sha256) = sha256 {
        if done != bytes || digest != sha256 {
            let _ = std::fs::remove_file(to);
            bail!("{url} did not match its pinned SHA-256");
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A morpheme from its text and the detail fields lindera gives for it (comma-separated, as
    /// in the dictionary's CSV), at `start` characters.
    pub fn m(start: usize, surface: &str, details: &str) -> Morpheme {
        Morpheme {
            start,
            end: start + surface.chars().count(),
            surface: surface.to_string(),
            details: details.split(',').map(str::to_string).collect(),
        }
    }

    /// Lays morphemes end to end; a surface written with a leading space has a space before it,
    /// as Korean words do.
    pub fn sentence(parts: &[(&str, &str)]) -> (Vec<char>, Vec<Morpheme>) {
        let mut text = String::new();
        let mut out = Vec::new();
        for &(surface, details) in parts {
            let surface = match surface.strip_prefix(' ') {
                Some(rest) => {
                    text.push(' ');
                    rest
                }
                None => surface,
            };
            out.push(m(text.chars().count(), surface, details));
            text.push_str(surface);
        }
        (text.chars().collect(), out)
    }

    /// The words as "text→lemma/lemma" (kinds other than Word marked with ·f, ·n, ·p).
    pub fn show(text: &[char], words: &[Word]) -> Vec<String> {
        words
            .iter()
            .map(|w| {
                let surface: String = text[w.start..w.end].iter().collect();
                let kind = match w.kind {
                    Kind::Word => "",
                    Kind::Function => "·f",
                    Kind::Number => "·n",
                    Kind::Punct => "·p",
                };
                if w.lemmas.is_empty() { format!("{surface}{kind}") } else { format!("{surface}{kind}→{}", w.lemmas.join("/")) }
            })
            .collect()
    }

    #[test]
    fn chinese_words_are_jieba_words_and_punctuation_is_not_a_word() {
        let (text, ms) = sentence(&[("我们", ""), ("在", ""), ("北京大学", ""), ("，", ""), ("ABC", "")]);
        assert_eq!(show(&text, &group(Lang::Chinese, &text, &ms)), ["我们", "在", "北京大学", "，·p", "ABC"]);
    }

    /// Downloads the three dictionaries into `LOOKITAN_SEGMENTERS` (CI caches it) and reads a
    /// sentence of each language with them.
    #[test]
    #[ignore = "downloads about 105 MB; CI runs it"]
    fn real_dictionaries() {
        let root = PathBuf::from(std::env::var("LOOKITAN_SEGMENTERS").expect("LOOKITAN_SEGMENTERS is the download folder"));
        let check = |lang: Lang, text: &str, expected: &[&str]| {
            let dir = lang.ensure(&root, &|_| {}).unwrap();
            let analyzer = Analyzer::open(lang, &dir).unwrap();
            let chars: Vec<char> = text.chars().collect();
            assert_eq!(show(&chars, &analyzer.words(text)), expected, "{text}");
        };
        check(Lang::Japanese, "食べさせられなかった本を読んでいる田中さんたち。",
            &["食べさせられなかった→食べる", "本", "を·f", "読んでいる→読む", "田中さんたち→田中", "。·p"]);
        check(Lang::Japanese, "日本語を勉強しています",
            &["日本語", "を·f", "勉強しています→勉強する/勉強"]);
        // JoinKatakanaOov joins an unknown katakana word, and the lemma is the dictionary form
        // (いる, not the normalized 居る).
        check(Lang::Japanese, "ネットフリックスで見た", &["ネットフリックス", "で·f", "見た→見る"]);
        check(Lang::Japanese, "家にいましたと思った", &["家", "に·f", "いました→いる", "と·f", "思った→思う"]);
        check(Lang::Korean, "오늘은 날씨가 좋아서 친구들과 함께 산책을 했습니다.",
            &["오늘은→오늘", "날씨가→날씨", "좋아서→좋다", "친구들과→친구들/친구", "함께", "산책을→산책", "했습니다→하다", ".·p"]);
        check(Lang::Chinese, "我们在北京大学学习中文。", &["我们", "在", "北京大学", "学习", "中文", "。·p"]);
        // Words only CC-CEDICT has stay whole, as in Lookupper (jieba's dict.txt.big alone gives
        // 緬因/貓吧, 最後/一根/稻草 and 3D/打印机).
        check(Lang::Chinese, "我們去看緬因貓吧", &["我們", "去", "看", "緬因貓", "吧"]);
        check(Lang::Chinese, "這是最後一根稻草", &["這是", "最後一根稻草"]);
        check(Lang::Chinese, "他用3D打印机做了一个模型", &["他用", "3D打印机", "做", "了", "一个", "模型"]);
        // A second call finds the merged dictionary and downloads nothing.
        let dir = Lang::Chinese.ensure(&root, &|_| panic!("downloaded again")).unwrap();
        assert!(dir.join("dict.txt").is_file() && dir.join("NOTICE.txt").is_file());
    }

    #[test]
    fn languages_with_an_analyzer_and_their_pinned_packs() {
        assert_eq!(Lang::of("ja-JP"), Some(Lang::Japanese));
        assert_eq!(Lang::of("zh-Hant-TW"), Some(Lang::Chinese));
        assert_eq!(Lang::of("ko"), Some(Lang::Korean));
        assert_eq!(Lang::of("pt-BR"), None);
        for p in &PACKS {
            assert_eq!(p.sha256.len(), 64);
            assert!(p.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        }
        assert!(status("en-US").is_none());
        assert!(status("ja").unwrap().contains("Sudachi"));
        assert!(status("zh-CN").unwrap().contains("CC-CEDICT"));
    }
}
