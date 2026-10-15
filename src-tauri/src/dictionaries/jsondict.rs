//! jmdict-simplified JSON (JMdict, JMnedict) and its kanjidic2 JSON, two of the formats Lookupper
//! reads (https://github.com/scriptin/jmdict-simplified). A file is one JSON object whose `words`
//! or `characters` array holds the entries; the index records each entry's place in the file, and
//! the `tags` table (abbreviation → description) is kept as a hidden entry and read on open.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use memmap2::Mmap;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::Value;

use super::lkidx::{Builder, Location, HIDDEN};
use super::{entry_link, escape_html};

pub const JMDICT: &str = "jmdict-simplified-json";
pub const KANJIDIC: &str = "kanjidic2-json";
/// `Location::kind` of the tags table.
pub const TAGS: u16 = 1;
const ENTRY: u16 = 0;

/// The format of a .json file, from its first 512 characters, as Lookupper decides it.
pub fn detect(path: &Path) -> Option<&'static str> {
    let mut head = [0u8; 512];
    let n = File::open(path).ok()?.read(&mut head).ok()?;
    let text = String::from_utf8_lossy(&head[..n]);
    if !text.contains("\"dictDate\"") {
        None
    } else if text.contains("\"characters\"") {
        Some(KANJIDIC)
    } else if text.contains("\"languages\"") {
        Some(JMDICT)
    } else {
        None
    }
}

#[derive(Deserialize)]
struct Root<'a> {
    #[serde(borrow, default)]
    tags: Option<&'a RawValue>,
    #[serde(borrow, default)]
    words: Vec<&'a RawValue>,
    #[serde(borrow, default)]
    characters: Vec<&'a RawValue>,
    #[serde(default)]
    version: Option<String>,
    #[serde(rename = "dictDate", default)]
    dict_date: Option<String>,
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

fn texts(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).into_iter().flatten()
        .filter_map(|v| v.get("text").and_then(Value::as_str)).map(str::to_string).collect()
}

/// Indexes the file; returns the builder, the dictionary's title and its description.
pub fn build(path: &Path, format: &str, progress: &dyn Fn(usize)) -> Result<(Builder, String, String)> {
    let file = File::open(path)?;
    // SAFETY: the file is only read, and a change makes the index stale (its stamp changes).
    let map = unsafe { Mmap::map(&file)? };
    let text = std::str::from_utf8(&map).with_context(|| format!("{} não é UTF-8", path.display()))?;
    let base = text.as_ptr() as usize;
    let root: Root = serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(text)).context("JSON do jmdict-simplified inválido")?;
    let at = |raw: &RawValue| -> Result<Location> {
        let json = raw.get();
        let length = u32::try_from(json.len()).map_err(|_| anyhow!("entrada JSON muito grande"))?;
        Ok(Location { file: 0, kind: ENTRY, offset: (json.as_ptr() as usize - base) as u64, length })
    };
    let mut builder = Builder::new();
    if let Some(tags) = root.tags {
        builder.add(&format!("{HIDDEN}tags"), Location { kind: TAGS, ..at(tags)? }, &[])?;
    }
    if format == KANJIDIC {
        for raw in &root.characters {
            let character: Value = serde_json::from_str(raw.get())?;
            let Some(literal) = character.get("literal").and_then(Value::as_str) else { continue };
            // Readings are shown but not indexed: a syllable would list thousands of characters.
            builder.add(literal, at(raw)?, &[])?;
            if builder.len() % 1000 == 0 { progress(builder.len()); }
        }
    } else {
        for raw in &root.words {
            let word: Value = serde_json::from_str(raw.get())?;
            let mut forms = texts(word.get("kanji"));
            forms.extend(texts(word.get("kana")));
            forms.dedup();
            let Some(title) = forms.first().cloned() else { continue };
            let aliases: Vec<&str> = forms[1..].iter().map(String::as_str).collect();
            builder.add(&title, at(raw)?, &aliases)?;
            if builder.len() % 1000 == 0 { progress(builder.len()); }
        }
    }
    let kind = if format == KANJIDIC { "KANJIDIC2" } else if root.words.first().map(|w| w.get().contains("\"translation\"")).unwrap_or(false) { "JMnedict" } else { "JMdict" };
    let title = format!("{kind} ({})", root.dict_date.unwrap_or_default());
    let description = format!("jmdict-simplified {}", root.version.unwrap_or_default());
    Ok((builder, title, description))
}

/// Reads the tags table.
pub fn tags(bytes: &[u8]) -> HashMap<String, String> {
    serde_json::from_slice(bytes).unwrap_or_default()
}

fn label(tag: &str, tags: &HashMap<String, String>) -> String {
    tags.get(tag).cloned().unwrap_or_else(|| tag.to_string())
}

fn small(labels: &[String]) -> String {
    if labels.is_empty() { String::new() } else { format!("<small>{}</small> ", escape_html(&labels.join("; "))) }
}

/// One JMdict or JMnedict word as HTML.
pub fn word_html(bytes: &[u8], tags: &HashMap<String, String>) -> Result<String> {
    let word: Value = serde_json::from_slice(bytes).context("entrada jmdict-simplified")?;
    let kanji = texts(word.get("kanji"));
    let kana = texts(word.get("kana"));
    let title = kanji.first().or(kana.first()).cloned().unwrap_or_default();
    let mut html = format!("<div class=\"jmdict-entry\"><h2>{}</h2>", escape_html(&title));
    if !kana.is_empty() && !kanji.is_empty() { html.push_str(&format!("<p class=\"reading\">{}</p>", escape_html(&kana.join("・")))); }
    if kanji.len() > 1 { html.push_str(&format!("<p>{}</p>", escape_html(&kanji[1..].join("・")))); }
    html.push_str("<ol>");
    // A sense with no part of speech has its predecessor's.
    let mut part_of_speech: Vec<String> = Vec::new();
    for sense in word.get("sense").and_then(Value::as_array).into_iter().flatten() {
        let pos = strings(sense.get("partOfSpeech"));
        if !pos.is_empty() { part_of_speech = pos; }
        let mut labels: Vec<String> = part_of_speech.iter().map(|t| label(t, tags)).collect();
        for field in ["field", "misc", "dialect"] {
            labels.extend(strings(sense.get(field)).iter().map(|t| label(t, tags)));
        }
        html.push_str("<li>");
        html.push_str(&small(&labels));
        let glosses: Vec<String> = sense.get("gloss").and_then(Value::as_array).into_iter().flatten().filter_map(|g| {
            let text = g.get("text").and_then(Value::as_str)?;
            let lang = g.get("lang").and_then(Value::as_str).unwrap_or("eng");
            Some(if lang == "eng" { text.to_string() } else { format!("[{lang}] {text}") })
        }).collect();
        html.push_str(&escape_html(&glosses.join("; ")));
        for (key, name) in [("appliesToKanji", "só"), ("appliesToKana", "só")] {
            let only: Vec<String> = strings(sense.get(key)).into_iter().filter(|s| s != "*").collect();
            if !only.is_empty() { html.push_str(&format!(" <small>({name} {})</small>", escape_html(&only.join(", ")))); }
        }
        for info in strings(sense.get("info")) { html.push_str(&format!("<p>{}</p>", escape_html(&info))); }
        for (key, name) in [("related", "Veja também"), ("antonym", "Antônimo")] {
            for target in sense.get(key).and_then(Value::as_array).into_iter().flatten() {
                let Some(first) = target.as_array().and_then(|t| t.first()).and_then(Value::as_str) else { continue };
                html.push_str(&format!("<p>{name}: <a href=\"{}\">{}</a></p>", entry_link(first), escape_html(first)));
            }
        }
        html.push_str("</li>");
    }
    // JMnedict names carry their translations instead of senses.
    for translation in word.get("translation").and_then(Value::as_array).into_iter().flatten() {
        let kinds: Vec<String> = strings(translation.get("type")).iter().map(|t| label(t, tags)).collect();
        let texts: Vec<String> = translation.get("translation").and_then(Value::as_array).into_iter().flatten()
            .filter_map(|t| t.get("text").and_then(Value::as_str)).map(str::to_string).collect();
        html.push_str(&format!("<li>{}{}</li>", small(&kinds), escape_html(&texts.join("; "))));
    }
    html.push_str("</ol></div>");
    Ok(html)
}

/// One kanjidic2 character as HTML.
pub fn character_html(bytes: &[u8]) -> Result<String> {
    let c: Value = serde_json::from_slice(bytes).context("caractere kanjidic2")?;
    let literal = c.get("literal").and_then(Value::as_str).unwrap_or("");
    let mut html = format!("<div class=\"kanjidic-entry\"><h2>{}</h2>", escape_html(literal));
    let mut readings: Vec<(String, Vec<String>)> = Vec::new();
    let mut meanings = Vec::new();
    let rm = c.get("readingMeaning");
    for group in rm.and_then(|r| r.get("groups")).and_then(Value::as_array).into_iter().flatten() {
        for r in group.get("readings").and_then(Value::as_array).into_iter().flatten() {
            let (Some(kind), Some(value)) = (r.get("type").and_then(Value::as_str), r.get("value").and_then(Value::as_str)) else { continue };
            match readings.iter_mut().find(|(k, _)| k == kind) {
                Some((_, values)) => values.push(value.to_string()),
                None => readings.push((kind.to_string(), vec![value.to_string()])),
            }
        }
        for m in group.get("meanings").and_then(Value::as_array).into_iter().flatten() {
            let Some(value) = m.get("value").and_then(Value::as_str) else { continue };
            let lang = m.get("lang").and_then(Value::as_str).unwrap_or("en");
            meanings.push(if lang == "en" { value.to_string() } else { format!("[{lang}] {value}") });
        }
    }
    for (kind, label) in [("ja_on", "On"), ("ja_kun", "Kun")] {
        if let Some((_, values)) = readings.iter().find(|(k, _)| k == kind) {
            html.push_str(&format!("<p class=\"reading\">{label}: {}</p>", escape_html(&values.join("、"))));
        }
    }
    let nanori = strings(rm.and_then(|r| r.get("nanori")));
    if !nanori.is_empty() { html.push_str(&format!("<p class=\"reading\">Nanori: {}</p>", escape_html(&nanori.join("、")))); }
    html.push_str("<ol>");
    for m in &meanings { html.push_str(&format!("<li>{}</li>", escape_html(m))); }
    html.push_str("</ol>");
    let misc = c.get("misc");
    let mut facts = Vec::new();
    if let Some(strokes) = misc.and_then(|m| m.get("strokeCounts")).and_then(Value::as_array).and_then(|s| s.first()) { facts.push(format!("Traços: {strokes}")); }
    if let Some(grade) = misc.and_then(|m| m.get("grade")).filter(|g| !g.is_null()) { facts.push(format!("Série: {grade}")); }
    if let Some(jlpt) = misc.and_then(|m| m.get("jlptLevel")).filter(|g| !g.is_null()) { facts.push(format!("JLPT: N{jlpt}")); }
    if let Some(freq) = misc.and_then(|m| m.get("frequency")).filter(|g| !g.is_null()) { facts.push(format!("Frequência: {freq}")); }
    for (kind, values) in &readings {
        if !matches!(kind.as_str(), "ja_on" | "ja_kun") { facts.push(format!("{kind}: {}", values.join(", "))); }
    }
    if !facts.is_empty() { html.push_str(&format!("<p class=\"tags\">{}</p>", escape_html(&facts.join(" · ")))); }
    html.push_str("</div>");
    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionaries::lkidx::{self, Stamp};
    use std::path::PathBuf;

    /// The start of jmdict-eng-common 3.6.2 (2026-10-12), cut to two words.
    const JMDICT_SAMPLE: &str = r#"{
"version": "3.6.2",
"languages": ["eng"],
"commonOnly": true,
"dictDate": "2026-10-12",
"dictRevisions": ["1.09"],
"tags": {"n":"noun (common) (futsuumeishi)","v5u":"Godan verb with 'u' ending","vt":"transitive verb","abbr":"abbreviation"},
"words": [
{"id":"1000160","kanji":[{"common":true,"text":"Ｔシャツ","tags":[]}],"kana":[{"common":true,"text":"ティーシャツ","tags":[],"appliesToKanji":["*"]}],"sense":[{"partOfSpeech":["n"],"appliesToKanji":["*"],"appliesToKana":["*"],"related":[],"antonym":[],"field":[],"dialect":[],"misc":[],"info":[],"languageSource":[],"gloss":[{"lang":"eng","gender":null,"type":null,"text":"T-shirt"},{"lang":"eng","gender":null,"type":null,"text":"tee shirt"}]}]},
{"id":"1001","kanji":[{"common":true,"text":"買う","tags":[]}],"kana":[{"common":true,"text":"かう","tags":[],"appliesToKanji":["*"]}],"sense":[{"partOfSpeech":["v5u","vt"],"appliesToKanji":["*"],"appliesToKana":["*"],"related":[["売る"]],"antonym":[],"field":[],"dialect":[],"misc":[],"info":[],"languageSource":[],"gloss":[{"lang":"eng","gender":null,"type":null,"text":"to buy"}]},{"partOfSpeech":[],"appliesToKanji":["*"],"appliesToKana":["*"],"related":[],"antonym":[],"field":[],"dialect":[],"misc":["abbr"],"info":[],"languageSource":[],"gloss":[{"lang":"eng","gender":null,"type":null,"text":"to value"}]}]}
]
}"#;

    const KANJIDIC_SAMPLE: &str = r#"{"version":"3.6.2","languages":["en"],"dictDate":"2026-10-12","fileVersion":4,"databaseVersion":"2026-285","characters":[{"literal":"亜","codepoints":[],"radicals":[],"misc":{"grade":8,"strokeCounts":[7],"variants":[],"frequency":1509,"radicalNames":[],"jlptLevel":1},"dictionaryReferences":[],"queryCodes":[],"readingMeaning":{"groups":[{"readings":[{"type":"pinyin","onType":null,"status":null,"value":"ya4"},{"type":"ja_on","onType":null,"status":null,"value":"ア"},{"type":"ja_kun","onType":null,"status":null,"value":"つ.ぐ"}],"meanings":[{"lang":"en","value":"Asia"},{"lang":"en","value":"rank next"}]}],"nanori":["や"]}}]}"#;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("lkt-jsondict-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn jmdict_words_and_tags_are_indexed_and_rendered_from_the_file() {
        let d = dir();
        let path = d.join("jmdict-eng-common-3.6.2.json");
        std::fs::write(&path, JMDICT_SAMPLE).unwrap();
        assert_eq!(detect(&path), Some(JMDICT));
        let stamp = Stamp::of(&d, &[PathBuf::from("jmdict-eng-common-3.6.2.json")]).unwrap();
        let index = lkidx::open_or_build(&d.join("x.lkidx"), JMDICT, &stamp, || build(&path, JMDICT, &|_| {})).unwrap();
        assert_eq!((index.len(), index.title.as_str()), (2, "JMdict (2026-10-12)"));
        let tags_id = index.find(&format!("{HIDDEN}tags"), 1)[0];
        let tags = tags(&lkidx::read(&d, &index.stamp, index.location(tags_id).unwrap(), 1 << 20).unwrap());
        let buy = index.find("かう", 4);
        assert_eq!(buy, index.find("買う", 4), "kana and kanji find the same word");
        let html = word_html(&lkidx::read(&d, &index.stamp, index.location(buy[0]).unwrap(), 1 << 20).unwrap(), &tags).unwrap();
        assert!(html.contains("<h2>買う</h2>") && html.contains("かう") && html.contains("to buy"), "{html}");
        assert!(html.contains("Godan verb with &#39;u&#39; ending; transitive verb"), "{html}");
        assert!(html.contains("abbreviation") && html.matches("transitive verb").count() == 2, "the second sense keeps its part of speech");
        assert!(html.contains(&entry_link("売る")));
        assert_eq!(index.find("ｔシャツ", 4).len(), 1, "full-width forms are folded");
        drop(index);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn kanjidic_characters_are_indexed_by_literal() {
        let d = dir();
        let path = d.join("kanjidic2-en-3.6.2.json");
        std::fs::write(&path, KANJIDIC_SAMPLE).unwrap();
        assert_eq!(detect(&path), Some(KANJIDIC));
        let (builder, title, _) = build(&path, KANJIDIC, &|_| {}).unwrap();
        assert_eq!((builder.len(), title.as_str()), (1, "KANJIDIC2 (2026-10-12)"));
        let html = character_html(KANJIDIC_SAMPLE[KANJIDIC_SAMPLE.find("{\"literal\"").unwrap()..KANJIDIC_SAMPLE.len() - 2].as_bytes()).unwrap();
        assert!(html.contains("On: ア") && html.contains("Kun: つ.ぐ") && html.contains("Nanori: や"), "{html}");
        assert!(html.contains("<li>Asia</li>") && html.contains("Traços: 7") && html.contains("JLPT: N1") && html.contains("pinyin: ya4"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
