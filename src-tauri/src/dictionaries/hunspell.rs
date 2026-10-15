//! Dictionary forms through Hunspell, as Lookupper's stemmer does, for German, English, Spanish,
//! French, Portuguese and Russian. The affix and word files are the same upstream releases that
//! Lookupper ships (wooorm/dictionaries on npm), fetched at build time by scripts/fetch-assets.py
//! into `resources/hunspell/<lang>.aff` and `<lang>.dic`.
//!
//! Only one language is kept in memory: the recognition language. Loading the Portuguese files
//! takes a few tenths of a second, so `warm` runs where the dictionaries load.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::Mutex;

/// Languages with a bundled Hunspell dictionary.
pub const LANGUAGES: [&str; 6] = ["de", "en", "es", "fr", "pt", "ru"];
const MAX_WORD_BYTES: usize = 256;
const MAX_STEMS: usize = 16;

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static LOADED: Mutex<Option<(&'static str, Option<Stemmer>)>> = Mutex::new(None);

pub struct Stemmer(hunspell_rs::Hunspell);

// The Hunspell handle is used by one thread at a time: every call goes through `LOADED`'s lock.
unsafe impl Send for Stemmer {}

impl Stemmer {
    /// Opens an affix and word file pair. `None` when either file is missing.
    pub fn open(aff: &Path, dic: &Path) -> Option<Self> {
        // On Windows `canonicalize` adds the `\\?\` prefix, which makes Hunspell open the path
        // as UTF-8 instead of in the ANSI code page.
        let aff = aff.canonicalize().ok().filter(|p| p.is_file())?;
        let dic = dic.canonicalize().ok().filter(|p| p.is_file())?;
        Some(Self(hunspell_rs::Hunspell::new(aff.to_str()?, dic.to_str()?)))
    }

    /// The dictionary forms of `word`, other than the word itself, in Hunspell's order.
    pub fn stems(&self, word: &str) -> Vec<String> {
        let word = word.trim();
        if word.is_empty() || word.len() > MAX_WORD_BYTES || word.contains('\0') {
            return Vec::new();
        }
        let mut out: Vec<String> = Vec::new();
        for stem in self.0.stem(word) {
            if !stem.is_empty() && stem != word && !out.contains(&stem) {
                out.push(stem);
            }
            if out.len() >= MAX_STEMS {
                break;
            }
        }
        out
    }
}

/// Where the bundled `.aff`/`.dic` files are; set once at start-up.
pub fn set_root(dir: PathBuf) {
    let _ = ROOT.set(dir);
}

/// The bundled dictionary for a BCP 47 tag ("pt-BR" uses "pt").
fn language(tag: &str) -> Option<&'static str> {
    let base = tag.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    LANGUAGES.iter().copied().find(|l| *l == base)
}

fn with_stemmer<T>(tag: &str, f: impl FnOnce(&Stemmer) -> T) -> Option<T> {
    let lang = language(tag)?;
    let root = ROOT.get()?;
    let mut loaded = LOADED.lock();
    if loaded.as_ref().map(|(l, _)| *l) != Some(lang) {
        let stemmer = Stemmer::open(&root.join(format!("{lang}.aff")), &root.join(format!("{lang}.dic")));
        if stemmer.is_none() {
            log::warn!("no Hunspell dictionary for {lang} in {}", root.display());
        }
        *loaded = Some((lang, stemmer));
    }
    loaded.as_ref().and_then(|(_, s)| s.as_ref()).map(f)
}

/// Loads the recognition language's dictionary ahead of the first lookup.
pub fn warm(tag: &str) {
    with_stemmer(tag, |_| ());
}

/// The dictionary forms of `word` in the language `tag`; empty for other languages.
pub fn stems(word: &str, tag: &str) -> Vec<String> {
    with_stemmer(tag, |s| s.stems(word)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_follow_the_affix_rules_and_skip_the_word_itself() {
        let dir = std::env::temp_dir().join(format!("lkt-hunspell-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (aff, dic) = (dir.join("en.aff"), dir.join("en.dic"));
        std::fs::write(&aff, "SET UTF-8\nSFX S Y 1\nSFX S 0 s .\nSFX G Y 2\nSFX G 0 ing [^e]\nSFX G e ing e\n").unwrap();
        std::fs::write(&dic, "3\nwalk/SG\nmake/G\nhouse/S\n").unwrap();
        let stemmer = Stemmer::open(&aff, &dic).unwrap();
        assert_eq!(stemmer.stems("walking"), vec!["walk"]);
        assert_eq!(stemmer.stems("making"), vec!["make"]);
        assert_eq!(stemmer.stems("houses"), vec!["house"]);
        assert!(stemmer.stems("house").is_empty(), "the word itself is not repeated");
        assert!(stemmer.stems("xyzzy").is_empty());
        assert!(stemmer.stems("a\0b").is_empty());
        assert!(Stemmer::open(&dir.join("missing.aff"), &dic).is_none());
        assert_eq!(language("pt-BR"), Some("pt"));
        assert_eq!(language("ja-JP"), None);
        drop(stemmer);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
