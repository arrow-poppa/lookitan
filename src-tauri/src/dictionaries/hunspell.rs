//! Dictionary forms through Hunspell, as Lookupper's stemmer does. Lookupper has German, English,
//! Spanish, French, Portuguese and Russian; Lookitan adds every other recognition language with a
//! wooorm/dictionaries package. The affix and word files (for those six, the same releases
//! Lookupper ships) are fetched at build time by scripts/fetch-assets.py into
//! `resources/hunspell/<lang>.aff` and `<lang>.dic`. Arabic is AyaSpell, as LibreOffice ships it
//! (GPL/LGPL/MPL), which also tells segment/arabic.rs what is a word. Finnish and Indonesian have
//! none.
//!
//! Only one language is kept in memory: the recognition language. Loading the Portuguese files
//! takes a few tenths of a second, so `warm` runs where the dictionaries load.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::Mutex;

/// Languages with a bundled Hunspell dictionary.
pub const LANGUAGES: [&str; 26] = [
    "de", "en", "es", "fr", "pt", "ru",
    "ar", "bg", "cs", "da", "el", "hr", "hu", "it", "la", "nb", "nl", "pl", "ro", "sk", "sl", "sr", "sr-latn", "sv", "tr", "uk",
];
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

    /// Whether the dictionary accepts `word`.
    pub fn check(&self, word: &str) -> bool {
        let word = word.trim();
        !word.is_empty() && word.len() <= MAX_WORD_BYTES && !word.contains('\0')
            && matches!(self.0.check(word), hunspell_rs::CheckResult::FoundInDictionary)
    }
}

/// Where the bundled `.aff`/`.dic` files are; set once at start-up.
pub fn set_root(dir: PathBuf) {
    let _ = ROOT.set(dir);
}

/// The bundled dictionary for a BCP 47 tag ("pt-BR" uses "pt", "sr-Latn-RS" "sr-latn"). Bosnian
/// uses the Croatian one: its only npm package is not wooorm's and declares no encoding.
fn language(tag: &str) -> Option<&'static str> {
    let tag = tag.to_ascii_lowercase();
    let mut parts = tag.split(['-', '_']);
    let base = parts.next().unwrap_or("");
    let key = match base {
        "sr" if parts.any(|p| p == "latn") => "sr-latn",
        "bs" => "hr",
        _ => base,
    };
    LANGUAGES.iter().copied().find(|l| *l == key)
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

/// Whether the language's dictionary accepts `word`; false without one.
pub fn check(word: &str, tag: &str) -> bool {
    with_stemmer(tag, |s| s.check(word)).unwrap_or(false)
}

/// A bundled file beside the dictionaries, when it is there.
pub fn bundled(name: &str) -> Option<PathBuf> {
    ROOT.get().map(|root| root.join(name)).filter(|p| p.is_file())
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
        assert!(stemmer.check("walking") && stemmer.check("house") && !stemmer.check("xyzzy") && !stemmer.check(""));
        assert!(stemmer.stems("a\0b").is_empty());
        assert!(Stemmer::open(&dir.join("missing.aff"), &dic).is_none());
        assert_eq!(language("pt-BR"), Some("pt"));
        assert_eq!(language("ja-JP"), None);
        assert_eq!(language("sr-Cyrl-RS"), Some("sr"));
        assert_eq!(language("sr-Latn-RS"), Some("sr-latn"));
        assert_eq!(language("bs-Latn-BA"), Some("hr"));
        assert_eq!(language("nb-NO"), Some("nb"));
        assert_eq!(language("fi-FI"), None);
        assert_eq!(language("ar-SA"), Some("ar"));
        drop(stemmer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// With the bundled files (CI fetches them before the tests), each added language finds the
    /// dictionary form of an inflected word.
    #[test]
    fn bundled_dictionaries_find_dictionary_forms() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/hunspell");
        if !root.join("uk.dic").is_file() {
            eprintln!("no bundled Hunspell files in {}; skipped", root.display());
            return;
        }
        let cases = [
            ("bg", "книгата", "книга"), ("cs", "ženy", "žena"), ("it", "gatti", "gatto"), ("nl", "huizen", "huis"),
            ("pl", "kotami", "kot"), ("sv", "husen", "hus"), ("tr", "evler", "ev"), ("uk", "книги", "книга"),
        ];
        let mut wrong = Vec::new();
        for (lang, word, form) in cases {
            let stems = Stemmer::open(&root.join(format!("{lang}.aff")), &root.join(format!("{lang}.dic")))
                .map(|s| s.stems(word));
            if !stems.as_ref().is_some_and(|s| s.iter().any(|x| x == form)) {
                wrong.push(format!("{lang} {word}: {stems:?}, expected {form}"));
            }
        }
        for lang in LANGUAGES {
            if Stemmer::open(&root.join(format!("{lang}.aff")), &root.join(format!("{lang}.dic"))).is_none() {
                wrong.push(format!("{lang}: no files"));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }
}
