//! Arabic words the recognizer read as one: it misses the narrow space after a word, mostly after
//! a letter that does not join the next (ا د ذ ر ز و ة ى), so "جماعةالضفادع" comes out for
//! "جماعة الضفادع". Lookupper keeps such a run as one word. Here a run that is not a known word is
//! cut in two known words, and the overlay, the pop-up, the dictionaries and the AI get the half
//! under the pointer.
//!
//! What is a word: a form seen at least 50 times in the CAMeL Arabic Frequency Lists (Modern
//! Standard and dialectal Arabic, CC BY-SA 4.0, so slang stays whole too), or one AyaSpell's Hunspell
//! dictionary accepts. Among the cuts into two such words, each of at least two letters, the one
//! whose halves are the most frequent wins, counting only forms seen at least 500 times: rare forms
//! there are mostly other run-together words. Measured on the user's page (2026-10-24): the nine
//! runs the recognizer made were all cut right, and none of 3000 rarer real words or 49 slang
//! words was cut. `ar.words.tsv.gz` (a form per line, sorted, with a tab and its count when that
//! is at least 500) is made by scripts/fetch-assets.py.

use std::io::Read;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use super::{Kind, Segment, Word};

/// Forms seen fewer times are not in the list.
const KNOWN: u32 = 50;
/// Forms seen fewer times count as unseen when two cuts are compared.
const RANKED: u32 = 500;
/// Shortest half of a cut, in letters.
const MIN_PART: usize = 2;
/// Shortest run tried, in letters. Every run the recognizer made on the user's pages had 6 letters
/// or more; a shorter unknown run is more often one word it misread (the screen's "وتصحيحه" read
/// "و حلحه", which حل + حه would cut), and is left whole for the AI to work out (2026-10-24).
const MIN_RUN: usize = 6;
/// Longest run tried, in letters.
const MAX_RUN: usize = 40;

/// The sorted list, kept as its file's text with the start of each line.
pub struct Frequencies {
    text: String,
    lines: Vec<u32>,
}

impl Frequencies {
    pub fn parse(text: String) -> Self {
        let mut lines = Vec::new();
        let mut at = 0usize;
        for line in text.split_inclusive('\n') {
            if !line.trim().is_empty() {
                lines.push(at as u32);
            }
            at += line.len();
        }
        Self { text, lines }
    }

    fn open(path: &Path) -> Option<Self> {
        let file = std::fs::File::open(path).ok()?;
        let mut text = String::new();
        flate2::read::GzDecoder::new(file).read_to_string(&mut text).ok()?;
        Some(Self::parse(text))
    }

    fn line(&self, start: u32) -> (&str, u32) {
        let rest = &self.text[start as usize..];
        let line = rest.split('\n').next().unwrap_or("").trim_end_matches('\r');
        match line.split_once('\t') {
            Some((word, count)) => (word, count.trim().parse().unwrap_or(KNOWN)),
            // A form in the list without its count was seen fewer than RANKED times.
            None => (line, KNOWN),
        }
    }

    /// How many times `word` was seen; 0 when it is not in the list.
    pub fn count(&self, word: &str) -> u32 {
        self.lines.binary_search_by(|&s| self.line(s).0.cmp(word)).map_or(0, |i| self.line(self.lines[i]).1)
    }
}

pub struct Arabic {
    freq: Frequencies,
    /// Whether Hunspell (AyaSpell) accepts a form.
    spelled: Box<dyn Fn(&str) -> bool + Send + Sync>,
}

impl Arabic {
    pub fn new(freq: Frequencies, spelled: Box<dyn Fn(&str) -> bool + Send + Sync>) -> Self {
        Self { freq, spelled }
    }

    fn known(&self, word: &str) -> bool {
        self.freq.count(word) >= KNOWN || (self.spelled)(word)
    }

    fn rank(&self, word: &str) -> u64 {
        let n = self.freq.count(word);
        if n >= RANKED { n as u64 } else { 0 }
    }

    /// Where to cut a run of letters (marks and tatweel left out), if it is two words.
    fn cut(&self, letters: &[char]) -> Option<usize> {
        if letters.len() < MIN_RUN.max(2 * MIN_PART) || letters.len() > MAX_RUN {
            return None;
        }
        let whole: String = letters.iter().collect();
        if self.known(&whole) {
            return None;
        }
        (MIN_PART..=letters.len() - MIN_PART)
            .filter_map(|i| {
                let (a, b): (String, String) = (letters[..i].iter().collect(), letters[i..].iter().collect());
                (self.known(&a) && self.known(&b)).then(|| (i, (self.rank(&a) + 1).saturating_mul(self.rank(&b) + 1)))
            })
            .max_by_key(|&(_, score)| score)
            .map(|(i, _)| i)
    }

    /// A piece of a word between spaces or punctuation, trimmed as words.rs trims a word (no
    /// punctuation or stray mark at its ends), then cut if it is two words.
    fn piece(&self, chars: &[char], mut start: usize, mut end: usize, out: &mut Vec<Word>) {
        while start < end && !is_letter(chars[start]) {
            start += 1;
        }
        while end > start && !is_letter(chars[end - 1]) {
            end -= 1;
        }
        if start < end {
            self.split(chars, start, end, out);
        }
    }

    /// The words of the run `chars[start..end]` (trimmed already): one, or two when it is cut.
    fn split(&self, chars: &[char], start: usize, end: usize, out: &mut Vec<Word>) {
        let word = |start, end| Word { start, end, kind: Kind::Word, lemmas: Vec::new() };
        // The letters and where each is; a mark stays with the letter before it.
        let letters: Vec<(usize, char)> = (start..end).filter(|&i| !is_mark(chars[i])).map(|i| (i, chars[i])).collect();
        if !letters.iter().any(|&(_, c)| is_arabic_letter(c)) {
            out.push(word(start, end));
            return;
        }
        let text: Vec<char> = letters.iter().map(|&(_, c)| c).collect();
        match self.cut(&text) {
            Some(i) => {
                let at = letters[i].0;
                out.push(word(start, at));
                out.push(word(at, end));
            }
            None => out.push(word(start, end)),
        }
    }
}

impl Segment for Arabic {
    fn words(&self, text: &str) -> Vec<Word> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i].is_whitespace() {
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            if !(start..i).any(|k| is_letter(chars[k])) {
                out.push(Word { start, end: i, kind: Kind::Punct, lemmas: Vec::new() });
                continue;
            }
            // A comma or full stop the recognizer read without the space after it ("الطيور،فلبت")
            // ends a word too, as in words.rs's pieces.
            let mut from = start;
            for k in start..=i {
                if k == i || breaks(chars[k]) {
                    self.piece(&chars, from, k, &mut out);
                    from = k + 1;
                }
            }
        }
        out
    }

    fn handles(&self, c: char) -> bool {
        is_arabic_letter(c)
    }

    fn name(&self) -> &'static str {
        "arabic-heuristic"
    }
}

/// Punctuation that ends a word even with no space after it.
fn breaks(c: char) -> bool {
    matches!(c, '،' | '؛' | '؟' | '.' | ',' | ';' | ':' | '!' | '?' | '«' | '»' | '(' | ')' | '"' | '۔')
}

fn is_mark(c: char) -> bool {
    unicode_normalization::char::is_combining_mark(c) || c == '\u{0640}'
}

fn is_letter(c: char) -> bool {
    c.is_alphanumeric() && !is_mark(c)
}

fn is_arabic_letter(c: char) -> bool {
    matches!(c as u32, 0x0620..=0x064A | 0x066E..=0x06D3 | 0x06FA..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08C9 | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)
        && !is_mark(c)
}

/// Whether `tag` is Arabic ("ar-SA").
pub fn is_arabic(tag: &str) -> bool {
    tag.split(['-', '_']).next().is_some_and(|l| l.eq_ignore_ascii_case("ar"))
}

static LOADED: OnceLock<Option<Arc<Arabic>>> = OnceLock::new();

/// The splitter, loaded on first use from the bundled list; `None` without it.
pub fn get() -> Option<Arc<Arabic>> {
    LOADED
        .get_or_init(|| {
            let path = crate::dictionaries::hunspell::bundled("ar.words.tsv.gz")?;
            let Some(freq) = Frequencies::open(&path) else {
                log::warn!("no Arabic word list at {}", path.display());
                return None;
            };
            let spelled = Box::new(|w: &str| crate::dictionaries::hunspell::check(w, "ar"));
            Some(Arc::new(Arabic::new(freq, spelled)))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn splitter() -> Arabic {
        let mut list: Vec<(&str, u32)> = vec![
            ("جماعة", 90_000), ("الضفادع", 4_000), ("داس", 3_000), ("العش", 2_500), ("الفيل", 20_000),
            ("فيلا", 6_000), ("يقترب", 9_000), ("في", 9_000_000), ("لايقترب", 300), ("شلونك", 60),
            ("شلون", 5_000), ("نك", 900),
        ];
        list.sort();
        // As the bundled file: no count under RANKED.
        let text: String = list.iter().map(|(w, n)| if *n >= RANKED { format!("{w}\t{n}\n") } else { format!("{w}\n") }).collect();
        Arabic::new(Frequencies::parse(text), Box::new(|w: &str| w == "كورتولموش"))
    }

    fn texts(s: &Arabic, text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        s.words(text).iter().map(|w| chars[w.start..w.end].iter().collect()).collect()
    }

    #[test]
    fn a_run_of_two_known_words_is_cut_between_them() {
        let s = splitter();
        assert_eq!(texts(&s, "جماعةالضفادع"), ["جماعة", "الضفادع"]);
        assert_eq!(texts(&s, "داسالعش، الفيل"), ["داس", "العش", "الفيل"], "punctuation is left out");
        assert_eq!(texts(&s, "الفيل،جماعة ، داس"), ["الفيل", "جماعة", "،", "داس"], "a comma with no space after it ends a word");
        // The most frequent halves win; a form seen under 500 times counts as unseen there.
        assert_eq!(texts(&s, "فيلايقترب"), ["فيلا", "يقترب"]);
    }

    #[test]
    fn known_words_slang_and_names_stay_whole() {
        let s = splitter();
        assert_eq!(texts(&s, "شلونك"), ["شلونك"], "in the list, though شلون + نك are words too");
        assert_eq!(texts(&s, "كورتولموش"), ["كورتولموش"], "Hunspell accepts it");
        assert_eq!(texts(&s, "الفيلمجهول"), ["الفيلمجهول"], "no cut into two known words");
        assert_eq!(texts(&s, "داسفي"), ["داسفي"], "under 6 letters a run stays whole, though داس + في are words");
    }

    #[test]
    fn marks_stay_with_their_letter_and_cuts_count_letters_only() {
        let s = splitter();
        // جماعةً الضَّفادعِ run together, with their marks.
        assert_eq!(texts(&s, "جماعةًالضَّفادعِ"), ["جماعةً", "الضَّفادع"], "a mark at the end is trimmed, as words.rs does");
        assert_eq!(texts(&s, "ًالفيل"), ["الفيل"], "a stray mark at the start is trimmed");
        assert!(!s.handles('،') && s.handles('ج') && !s.handles('a'));
        assert!(is_arabic("ar-SA") && !is_arabic("arn"));
    }

    /// With the bundled files (CI fetches them before the tests): the runs the recognizer made on
    /// the user's page are cut right, and its words and some slang stay whole.
    #[test]
    fn the_bundled_lists_cut_the_runs_the_recognizer_made() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/hunspell");
        let aya = crate::dictionaries::hunspell::Stemmer::open(&root.join("ar.aff"), &root.join("ar.dic"));
        let (Some(freq), Some(aya)) = (Frequencies::open(&root.join("ar.words.tsv.gz")), aya) else {
            eprintln!("no bundled Arabic files in {}; skipped", root.display());
            return;
        };
        let aya = parking_lot::Mutex::new(aya);
        let s = Arabic::new(freq, Box::new(move |w: &str| aya.lock().check(w)));
        for (run, cut) in [
            ("وتوسلتإليه", ["وتوسلت", "إليه"]), ("داسالعش", ["داس", "العش"]), ("أمرهاالى", ["أمرها", "الى"]),
            ("جماعةالضفادع", ["جماعة", "الضفادع"]), ("اليهاأن", ["اليها", "أن"]), ("وادعميق", ["واد", "عميق"]),
            ("فيلايقترب", ["فيلا", "يقترب"]), ("نقيقاعاليا", ["نقيقا", "عاليا"]), ("القبرةوالفيل", ["القبرة", "والفيل"]),
            // Read at the screen's own size (2026-10-24).
            ("فيهوفي", ["فيه", "وفي"]),
        ] {
            assert_eq!(texts(&s, run), cut, "{run}");
        }
        for word in "يحكى قبرة فباضت وفرخت الأيام فحلقت وتوسلت يحيد قليلا بأمرها وماتت الفراخ وفقأت الضفادع وتنق نقيقا \
                     أضلاعه ازيك عايز كده شلونك هيك بدي دلوقتي خلاص كتير عشان بتاعي اشوفك مابعرف حلحه"
            .split_whitespace()
        {
            assert_eq!(texts(&s, word), [word], "{word} stays whole");
        }
    }
}
