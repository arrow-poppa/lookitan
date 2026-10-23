//! Bounded lookup candidates, independently implemented for Lookitan.
//!
//! Japanese, English, Spanish, Arabic, Ukrainian and German rules are derived
//! from Yomitan's GPL-3.0-or-later descriptors (Copyright 2024-2026 Yomitan
//! Authors); see data/README.md and the source hashes in data/*-yomitan.json.
//! This is a bounded Rust implementation of condition-aware rule traversal, not
//! a Lookupper language-pack port. English exceptions are Princeton WordNet 3.0
//! data, with its license retained.

use super::folded;
use fancy_regex::Regex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::OnceLock;

const MAX_CANDIDATES: usize = 256;
const MAX_WORD_BYTES: usize = 1024;
const MAX_WORD_CHARS: usize = 256;
const MAX_STATES: usize = 1024;
const MAX_STEPS: u8 = 12;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuleData {
    condition_masks: HashMap<String, u64>,
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    kind: RuleKind,
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
    /// Yomitan's own pattern, when the literal ending is not the whole test: a minimum stem
    /// (Ukrainian) or letters around the affixes (Arabic). Arabic has thousands, so each is
    /// compiled the first time a word passes the rule's literal test.
    #[serde(default)]
    pattern: Option<String>,
    #[serde(skip)]
    compiled: OnceLock<Option<Regex>>,
    /// An affix rule: the literal text its pattern starts and ends with, the characters it cuts
    /// from the front and the back, and the text it adds there.
    #[serde(default)]
    starts: String,
    #[serde(default)]
    ends: String,
    #[serde(default)]
    cut: [usize; 2],
    #[serde(default)]
    add: [String; 2],
    input: u64,
    output: u64,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RuleKind { Suffix, Prefix, Whole, Affix }

impl Rule {
    /// Whether `word` matches the rule's pattern; true without one. A pattern that does not
    /// compile matches nothing (the embedded data's test compiles them all).
    fn pattern_matches(&self, word: &str) -> bool {
        let Some(source) = &self.pattern else { return true };
        self.compiled.get_or_init(|| Regex::new(source).ok()).as_ref()
            .is_some_and(|re| re.is_match(word).unwrap_or(false))
    }
}

/// The rules of a Yomitan language code; English for any other.
fn rules(language: &str) -> &'static RuleData {
    static DATA: [OnceLock<RuleData>; 6] = [const { OnceLock::new() }; 6];
    let (index, source) = match language {
        "ja" => (0, include_str!("data/ja-yomitan.json")),
        "es" => (2, include_str!("data/es-yomitan.json")),
        "ar" => (3, include_str!("data/ar-yomitan.json")),
        "uk" => (4, include_str!("data/uk-yomitan.json")),
        "de" => (5, include_str!("data/de-yomitan.json")),
        _ => (1, include_str!("data/en-yomitan.json")),
    };
    DATA[index].get_or_init(|| serde_json::from_str(source).expect("validated embedded Yomitan morphology data"))
}

struct Candidates {
    values: Vec<String>,
    seen: HashSet<String>,
}

impl Candidates {
    fn new() -> Self { Self { values: Vec::new(), seen: HashSet::new() } }
    fn add(&mut self, word: &str) -> bool {
        if self.values.len() >= MAX_CANDIDATES || !valid_length(word) || word.trim().is_empty() {
            return false;
        }
        if self.seen.insert(word.to_string()) {
            self.values.push(word.to_string());
            true
        } else { false }
    }
    fn full(&self) -> bool { self.values.len() >= MAX_CANDIDATES }
}

fn valid_length(word: &str) -> bool {
    !word.is_empty() && word.len() <= MAX_WORD_BYTES && word.chars().take(MAX_WORD_CHARS + 1).count() <= MAX_WORD_CHARS
}

/// The original surface is always first for a bounded nonempty input. Unknown
/// language tags receive Unicode/punctuation normalization only; English and
/// Portuguese suffixes are never applied to another language's text.
pub fn candidates(word: &str, language: &str) -> Vec<String> {
    if !valid_length(word) || word.trim().is_empty() { return Vec::new(); }
    let mut result = Candidates::new();
    result.add(word);
    result.add(word.trim());
    let clean = word.trim().trim_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '\'' | '\u{2018}' | '\u{2019}'));
    result.add(clean);
    let normalized = folded(clean);
    result.add(&normalized);
    // Remove quotation apostrophes as a second candidate, preserving possessive
    // forms above so parents' and children's still enter the morphology graph.
    let unquoted = normalized.trim_matches(|c: char| !c.is_alphanumeric());
    result.add(unquoted);
    let language = language.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    match language.as_str() {
        "ja" | "jpn" => {
            transform(&normalized, rules("ja"), false, &mut result);
            if unquoted != normalized { transform(unquoted, rules("ja"), false, &mut result); }
            let hiragana: String = unquoted.chars().map(|c| {
                if ('\u{30a1}'..='\u{30f6}').contains(&c) { char::from_u32(c as u32 - 0x60).unwrap_or(c) }
                else { c }
            }).collect();
            if hiragana != unquoted {
                result.add(&hiragana);
                transform(&hiragana, rules("ja"), false, &mut result);
            }
        }
        "en" | "eng" => {
            if english_text(&normalized) { transform(&normalized, rules("en"), true, &mut result); }
            if unquoted != normalized && english_text(unquoted) { transform(unquoted, rules("en"), true, &mut result); }
        }
        "pt" | "por" => {
            portuguese(&normalized, &mut result);
            if unquoted != normalized { portuguese(unquoted, &mut result); }
        }
        "es" | "spa" | "ar" | "ara" | "uk" | "ukr" | "de" | "deu" | "ger" => {
            let code = match language.as_str() { "spa" => "es", "ara" => "ar", "ukr" => "uk", "deu" | "ger" => "de", code => code };
            let data = rules(code);
            transform(&normalized, data, false, &mut result);
            if unquoted != normalized { transform(unquoted, data, false, &mut result); }
            if code == "de" { german(&normalized, data, &mut result); }
        }
        _ => {}
    }
    result.values
}

fn english_text(word: &str) -> bool {
    word.bytes().any(|b| b.is_ascii_alphabetic()) && word.bytes().all(|b| b.is_ascii_alphabetic() || matches!(b, b'\'' | b'-' | b' '))
}

fn apply(rule: &Rule, word: &str) -> Option<String> {
    let candidate = match rule.kind {
        RuleKind::Suffix => format!("{}{}", word.strip_suffix(&rule.from)?, rule.to),
        RuleKind::Prefix => format!("{}{}", rule.to, word.strip_prefix(&rule.from)?),
        RuleKind::Whole => { if word != rule.from { return None; } rule.to.clone() }
        RuleKind::Affix => {
            if !word.starts_with(&rule.starts) || !word.ends_with(&rule.ends) { return None; }
            let chars: Vec<char> = word.chars().collect();
            let [front, back] = rule.cut;
            if front + back > chars.len() { return None; }
            let stem: String = chars[front..chars.len() - back].iter().collect();
            format!("{}{stem}{}", rule.add[0], rule.add[1])
        }
    };
    // The literal test passed; Yomitan's pattern may still ask for more around it.
    if !rule.pattern_matches(word) { return None; }
    (candidate != word && valid_length(&candidate) && !candidate.trim().is_empty()).then_some(candidate)
}

fn transform(start: &str, data: &RuleData, english: bool, result: &mut Candidates) {
    if !valid_length(start) { return; }
    let mut queue = VecDeque::from([(start.to_string(), 0u64, 0u8)]);
    let mut visited = HashSet::from([(start.to_string(), 0u64)]);
    while let Some((word, conditions, depth)) = queue.pop_front() {
        if result.full() || depth >= MAX_STEPS { break; }
        if english {
            // Exact lexical exceptions precede guessed suffix reductions.
            if let Some(exceptions) = english_exceptions().get(word.as_str()) {
                for &(lemma, part) in exceptions {
                    let part_mask = data.condition_masks.get(part).copied().unwrap_or(0);
                    if conditions != 0 && conditions & part_mask == 0 { continue; }
                    result.add(lemma);
                    if visited.len() < MAX_STATES && visited.insert((lemma.to_string(), part_mask)) {
                        queue.push_back((lemma.to_string(), part_mask, depth + 1));
                    }
                }
            }
        }
        for rule in &data.rules {
            if conditions != 0 && conditions & rule.input == 0 { continue; }
            let Some(candidate) = apply(rule, &word) else { continue };
            // Avoid stripping English suffixes down to one-letter guesses.
            if english && candidate.chars().filter(|c| c.is_ascii_alphabetic()).count() < 2 { continue; }
            result.add(&candidate);
            if result.full() { return; }
            if visited.len() < MAX_STATES && visited.insert((candidate.clone(), rule.output)) {
                queue.push_back((candidate, rule.output, depth + 1));
            }
        }
    }
}

type ExceptionMap = HashMap<&'static str, Vec<(&'static str, &'static str)>>;

fn english_exceptions() -> &'static ExceptionMap {
    static EXCEPTIONS: OnceLock<ExceptionMap> = OnceLock::new();
    EXCEPTIONS.get_or_init(|| {
        let mut map = ExceptionMap::new();
        // Yomitan's n includes singular/plural noun conditions; v includes its
        // phrasal-verb subtype. Exceptions retain their known part of speech.
        for (source, part) in [
            (include_str!("data/en-wordnet-verb.exc"), "v"),
            (include_str!("data/en-wordnet-noun.exc"), "n"),
            (include_str!("data/en-wordnet-adj.exc"), "adj"),
            (include_str!("data/en-wordnet-adv.exc"), "adv"),
        ] {
            for line in source.lines() {
                let mut fields = line.split_whitespace();
                let Some(surface) = fields.next() else { continue };
                for lemma in fields {
                    let entry = map.entry(surface).or_default();
                    if !entry.contains(&(lemma, part)) { entry.push((lemma, part)); }
                }
            }
        }
        map
    })
}

fn portuguese(word: &str, result: &mut Candidates) {
    if !word.chars().all(|c| c.is_alphabetic() || c == '-') || word.chars().count() < 3 { return; }
    // Conservative plural spelling rules, not a verb conjugator. Ambiguous
    // -ões/-ães/-ãos and -is forms offer alternatives only after the surface.
    // The installed dictionaries decide which proposed spelling actually exists.
    const PLURALS: &[(&str, &str)] = &[
        ("ões", "ão"), ("ães", "ão"), ("ãos", "ão"),
        ("ais", "al"), ("éis", "el"), ("eis", "el"), ("eis", "il"), ("óis", "ol"),
        ("ois", "ol"), ("uis", "ul"), ("is", "il"), ("ns", "m"),
        ("res", "r"), ("zes", "z"), ("s", ""),
    ];
    let mut bases = Vec::new();
    for (suffix, replacement) in PLURALS {
        if let Some(stem) = word.strip_suffix(suffix) {
            if !stem.is_empty() && stem.chars().count() + replacement.chars().count() >= 2 {
                let base = format!("{stem}{replacement}");
                result.add(&base);
                bases.push(base);
            }
        }
    }
    // -mente attaches to the feminine adjective, e.g. claramente -> clara.
    // Do not rewrite every final a into o; lexical gender is not predictable.
    for base in std::iter::once(word).chain(bases.iter().map(String::as_str)) {
        if let Some(stem) = base.strip_suffix("mente").filter(|s| s.chars().count() >= 3) { result.add(stem); }
    }
}

/// German past participles, which Yomitan finds with patterns rather than literal rules: ge…t
/// gives …en and …n (gemacht -> machen, gearbeitet -> arbeiten), ge…en gives …en (gesehen ->
/// sehen), each also after a separable prefix (angerufen -> anrufen). The separable prefixes are
/// the ones Yomitan's zu-infinitive rules take off.
fn german(word: &str, data: &RuleData, result: &mut Candidates) {
    if !word.chars().all(char::is_alphabetic) { return; }
    let prefixes = data.rules.iter().filter(|r| matches!(r.kind, RuleKind::Prefix)).map(|r| r.to.as_str());
    for prefix in std::iter::once("").chain(prefixes) {
        let Some(rest) = word.strip_prefix(prefix).and_then(|r| r.strip_prefix("ge")) else { continue };
        if let Some(stem) = rest.strip_suffix('t').filter(|s| s.chars().count() >= 2) {
            result.add(&format!("{prefix}{stem}en"));
            result.add(&format!("{prefix}{stem}n"));
        }
        if rest.ends_with("en") && rest.chars().count() >= 4 {
            result.add(&format!("{prefix}{rest}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(word: &str, language: &str, lemma: &str) {
        assert!(candidates(word, language).iter().any(|s| s == lemma), "{word} ({language}) should include {lemma}");
    }

    #[test]
    fn original_surface_precedes_unicode_and_punctuation_normalization() {
        let values = candidates("“Ｒｕｎｎｉｎｇ!”", "en-US");
        assert_eq!(values[0], "“Ｒｕｎｎｉｎｇ!”");
        assert!(values.contains(&"running".to_string()));
        assert!(values.contains(&"run".to_string()));
        has("children’s", "en", "child");
        has("parents’", "en", "parent");
    }

    #[test]
    fn english_regular_and_attested_irregular_forms() {
        for (word, lemma) in [("running", "run"), ("stopped", "stop"), ("studies", "study"), ("lying", "lie"), ("written", "write"), ("went", "go"), ("children", "child"), ("mice", "mouse"), ("better", "good"), ("were", "be")] {
            has(word, "en-GB", lemma);
        }
    }

    #[test]
    fn japanese_chains_respect_yomitan_conditions() {
        for (word, lemma) in [("食べました", "食べる"), ("読んでいる", "読む"), ("行った", "行く"), ("高くなかった", "高い"), ("勉強しませんでした", "勉強する"), ("食べさせられた", "食べる")] {
            has(word, "ja-JP", lemma);
        }
        has("ﾀﾍﾞﾏｼﾀ", "ja", "たべる");
        // Forms the rules of Yomitan's October 2026 source added.
        for (word, lemma) in [("食べやしない", "食べる"), ("行きやしません", "行く"), ("書かう", "書く")] {
            has(word, "ja-JP", lemma);
        }
    }

    #[test]
    fn spanish_arabic_and_ukrainian_follow_yomitan() {
        for (word, lemma) in [("fui", "ser"), ("fui", "ir"), ("puesto", "poner"), ("hecho", "hacer"), ("dijo", "decir"), ("hablando", "hablar"), ("vendrá", "venir")] {
            has(word, "es-MX", lemma);
        }
        for (word, lemma) in [("الكتاب", "كتاب"), ("كتبت", "كتب"), ("يكتبون", "كتب"), ("والمدرسة", "مدرسة")] {
            has(word, "ar-SA", lemma);
        }
        for (word, lemma) in [("книги", "книга"), ("пішов", "піти"), ("написала", "написати"), ("людей", "людина"), ("кращий", "добрий")] {
            has(word, "uk-UA", lemma);
        }
        // Ukrainian's minimum stem: an ending is not taken off a word that is only the ending.
        assert!(!candidates("зі", "uk").contains(&"зь".to_string()));
    }

    #[test]
    fn german_participles_and_zu_infinitives() {
        for (word, lemma) in [("gemacht", "machen"), ("gearbeitet", "arbeiten"), ("gesehen", "sehen"), ("angerufen", "anrufen"), ("aufzumachen", "aufmachen")] {
            has(word, "de-DE", lemma);
        }
    }

    #[test]
    fn portuguese_plural_rules_are_scoped_and_bounded() {
        for (word, lemma) in [("ações", "ação"), ("pães", "pão"), ("papéis", "papel"), ("animais", "animal"), ("homens", "homem"), ("claramente", "clara")] {
            has(word, "pt-BR", lemma);
        }
        assert!(!candidates("running", "pt").contains(&"run".to_string()));
        assert!(!candidates("ações", "en").contains(&"ação".to_string()));
    }

    #[test]
    fn unrelated_languages_never_receive_english_or_portuguese_rules() {
        for tag in ["fr-FR", "de-DE", "ru", "ko", "zh-Hans-CN", "", "invalid"] {
            assert_eq!(candidates("running", tag), vec!["running"]);
            assert_eq!(candidates("animais", tag), vec!["animais"]);
        }
    }

    #[test]
    fn candidate_growth_and_input_length_are_limited() {
        for word in ["食べさせられていませんでした", "running's", "unhappiest", "読みました"] {
            let values = candidates(word, if word.is_ascii() { "en" } else { "ja" });
            assert!(values.len() <= MAX_CANDIDATES);
            assert_eq!(values.len(), values.iter().collect::<HashSet<_>>().len());
            assert!(values.iter().all(|s| valid_length(s)));
        }
        assert!(candidates(&"a".repeat(1025), "en").is_empty());
        assert!(candidates(&"語".repeat(257), "ja").is_empty());
        assert!(candidates("  ", "en").is_empty());
    }

    #[test]
    fn embedded_data_has_the_expected_provenance_and_rules() {
        for (language, count) in [("ja", 928), ("en", 98), ("es", 518), ("ar", 7157), ("uk", 2959), ("de", 66)] {
            let data = rules(language);
            assert_eq!(data.rules.len(), count, "{language}");
            // A literal rule has its text; an affix rule has its pattern. Every pattern compiles.
            for r in &data.rules {
                match r.kind {
                    RuleKind::Affix => assert!(r.pattern.is_some(), "{language}"),
                    _ => assert!(!r.from.is_empty(), "{language}"),
                }
                if let Some(source) = &r.pattern {
                    assert!(Regex::new(source).is_ok(), "{language}: {source}");
                }
            }
        }
        assert!(english_exceptions().len() > 4000);
    }
}
