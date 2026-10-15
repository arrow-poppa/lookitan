//! WordNet in the Global WordNet Association's WN-LMF XML (Open English WordNet and others), one
//! of the formats Lookupper reads. Lexical entries are indexed under their lemma and forms; synsets
//! are hidden entries found by id. A lookup reads the entry, then each of its senses' synsets.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::lkidx::{Builder, Location, HIDDEN};
use super::{entry_link, escape_html};

pub const FORMAT: &str = "wordnet-lmf-xml";
pub const SYNSET: u16 = 1;
const ENTRY: u16 = 0;

/// Whether `path` is a WN-LMF file: `<LexicalResource` within its first kilobyte, as Lookupper
/// checks.
pub fn detect(path: &Path) -> bool {
    let mut head = [0u8; 1024];
    let Ok(n) = File::open(path).and_then(|mut f| f.read(&mut head)) else { return false };
    head[..n].windows(16).any(|w| w == b"<LexicalResource")
}

fn attribute(tag: &BytesStart, name: &[u8]) -> Option<String> {
    tag.attributes().flatten().find(|a| a.key.as_ref() == name)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

/// The key a synset is found under.
pub fn synset_key(id: &str) -> String {
    format!("{HIDDEN}s:{id}")
}

/// The key a lexical entry is found under by id (to name a synset's members).
pub fn entry_key(id: &str) -> String {
    format!("{HIDDEN}e:{id}")
}

/// Indexes the file; returns the builder, the lexicon's label and a description.
pub fn build(path: &Path, progress: &dyn Fn(usize)) -> Result<(Builder, String, String)> {
    let mut reader = Reader::from_reader(BufReader::new(File::open(path)?));
    let mut buffer = Vec::new();
    let mut builder = Builder::new();
    let (mut label, mut description) = (String::from("WordNet"), String::new());
    // The entry being read: its start, id, lemma and forms.
    let mut entry: Option<(u64, String, String, Vec<String>)> = None;
    let mut synset: Option<(u64, String)> = None;
    loop {
        let before = reader.buffer_position();
        let event = reader.read_event_into(&mut buffer).context("WN-LMF XML inválido")?;
        match &event {
            Event::Start(tag) | Event::Empty(tag) => match tag.name().as_ref() {
                b"Lexicon" => {
                    label = attribute(tag, b"label").unwrap_or(label);
                    description = format!("WN-LMF {} {}", attribute(tag, b"language").unwrap_or_default(), attribute(tag, b"version").unwrap_or_default()).trim().to_string();
                }
                b"LexicalEntry" => entry = Some((before, attribute(tag, b"id").unwrap_or_default(), String::new(), Vec::new())),
                b"Lemma" => if let Some(e) = entry.as_mut() { e.2 = attribute(tag, b"writtenForm").unwrap_or_default(); },
                b"Form" => if let Some(e) = entry.as_mut() { e.3.extend(attribute(tag, b"writtenForm")); },
                b"Synset" => {
                    let id = attribute(tag, b"id").unwrap_or_default();
                    if matches!(event, Event::Empty(_)) {
                        let length = (reader.buffer_position() - before) as u32;
                        builder.add(&synset_key(&id), Location { file: 0, kind: SYNSET, offset: before, length }, &[])?;
                    } else {
                        synset = Some((before, id));
                    }
                }
                _ => {}
            },
            Event::End(tag) => match tag.name().as_ref() {
                b"LexicalEntry" => {
                    if let Some((start, id, lemma, forms)) = entry.take() {
                        let length = u32::try_from(reader.buffer_position() - start)?;
                        if !lemma.is_empty() {
                            let key = entry_key(&id);
                            let mut aliases: Vec<&str> = forms.iter().map(String::as_str).collect();
                            aliases.push(&key);
                            builder.add(&lemma, Location { file: 0, kind: ENTRY, offset: start, length }, &aliases)?;
                            if builder.len() % 1000 == 0 { progress(builder.len()); }
                        }
                    }
                }
                b"Synset" => {
                    if let Some((start, id)) = synset.take() {
                        let length = u32::try_from(reader.buffer_position() - start)?;
                        builder.add(&synset_key(&id), Location { file: 0, kind: SYNSET, offset: start, length }, &[])?;
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok((builder, label, description))
}

/// What a lexical entry says: its lemma, part of speech, pronunciations and senses' synsets.
pub struct Entry {
    pub lemma: String,
    pub pos: String,
    pub pronunciations: Vec<String>,
    pub synsets: Vec<String>,
}

pub fn parse_entry(bytes: &[u8]) -> Result<Entry> {
    let mut reader = Reader::from_reader(bytes);
    let mut buffer = Vec::new();
    let mut entry = Entry { lemma: String::new(), pos: String::new(), pronunciations: Vec::new(), synsets: Vec::new() };
    let mut in_pronunciation = false;
    loop {
        match reader.read_event_into(&mut buffer).context("entrada WN-LMF")? {
            Event::Start(tag) | Event::Empty(tag) => match tag.name().as_ref() {
                b"Lemma" => {
                    entry.lemma = attribute(&tag, b"writtenForm").unwrap_or_default();
                    entry.pos = attribute(&tag, b"partOfSpeech").unwrap_or_default();
                }
                b"Sense" => entry.synsets.extend(attribute(&tag, b"synset")),
                b"Pronunciation" => in_pronunciation = true,
                _ => {}
            },
            Event::Text(text) if in_pronunciation => entry.pronunciations.push(text.decode()?.into_owned()),
            Event::End(tag) if tag.name().as_ref() == b"Pronunciation" => in_pronunciation = false,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(entry)
}

/// What a synset says: its definitions, examples and member entry ids.
pub struct Synset {
    pub definitions: Vec<String>,
    pub examples: Vec<String>,
    pub members: Vec<String>,
}

pub fn parse_synset(bytes: &[u8]) -> Result<Synset> {
    let mut reader = Reader::from_reader(bytes);
    let mut buffer = Vec::new();
    let mut synset = Synset { definitions: Vec::new(), examples: Vec::new(), members: Vec::new() };
    let mut field: Option<bool> = None; // Some(true) in a Definition, Some(false) in an Example
    loop {
        match reader.read_event_into(&mut buffer).context("synset WN-LMF")? {
            Event::Start(tag) => match tag.name().as_ref() {
                b"Synset" => synset.members = attribute(&tag, b"members").unwrap_or_default().split_whitespace().map(str::to_string).collect(),
                b"Definition" => { field = Some(true); synset.definitions.push(String::new()); }
                b"Example" => { field = Some(false); synset.examples.push(String::new()); }
                _ => {}
            },
            Event::Empty(tag) if tag.name().as_ref() == b"Synset" => {
                synset.members = attribute(&tag, b"members").unwrap_or_default().split_whitespace().map(str::to_string).collect();
            }
            Event::Text(text) => {
                let text = text.decode()?;
                match field {
                    Some(true) => if let Some(d) = synset.definitions.last_mut() { d.push_str(&text); },
                    Some(false) => if let Some(e) = synset.examples.last_mut() { e.push_str(&text); },
                    None => {}
                }
            }
            Event::GeneralRef(entity) => {
                let name = std::str::from_utf8(entity.as_ref())?;
                let c = match name { "amp" => "&", "lt" => "<", "gt" => ">", "apos" => "'", "quot" => "\"", _ => "" };
                match field {
                    Some(true) => if let Some(d) = synset.definitions.last_mut() { d.push_str(c); },
                    Some(false) => if let Some(e) = synset.examples.last_mut() { e.push_str(c); },
                    None => {}
                }
            }
            Event::End(tag) if matches!(tag.name().as_ref(), b"Definition" | b"Example") => field = None,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(synset)
}

/// The part of speech's name.
fn pos_name(pos: &str) -> &str {
    match pos { "n" => "noun", "v" => "verb", "a" | "s" => "adjective", "r" => "adverb", "c" => "conjunction", "p" => "adposition", other => other }
}

/// An entry as HTML; `synset` reads a synset by id, `member` names a member entry.
pub fn entry_html(entry: &Entry, synset: impl Fn(&str) -> Option<Synset>, member: impl Fn(&str) -> Option<String>) -> String {
    let mut html = format!("<div class=\"wordnet-entry\"><h2>{}</h2><p class=\"tags\">{}</p>", escape_html(&entry.lemma), escape_html(pos_name(&entry.pos)));
    if !entry.pronunciations.is_empty() {
        html.push_str(&format!("<p class=\"pronunciation\">/{}/</p>", escape_html(&entry.pronunciations.join("/, /"))));
    }
    html.push_str("<ol>");
    for id in &entry.synsets {
        let Some(s) = synset(id) else { continue };
        html.push_str("<li>");
        html.push_str(&escape_html(&s.definitions.join("; ")));
        let synonyms: Vec<String> = s.members.iter().filter_map(|m| member(m)).filter(|m| *m != entry.lemma).collect();
        if !synonyms.is_empty() {
            let links: Vec<String> = synonyms.iter().map(|w| format!("<a href=\"{}\">{}</a>", entry_link(w), escape_html(w))).collect();
            html.push_str(&format!(" <small>({})</small>", links.join(", ")));
        }
        for example in &s.examples { html.push_str(&format!("<blockquote>{}</blockquote>", escape_html(example))); }
        html.push_str("</li>");
    }
    html.push_str("</ol></div>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionaries::lkidx::{self, Stamp};
    use std::path::PathBuf;

    /// An excerpt of Open English WordNet 2025 in WN-LMF 1.3.
    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE LexicalResource SYSTEM "http://globalwordnet.github.io/schemas/WN-LMF-1.3.dtd">
<LexicalResource xmlns:dc="https://globalwordnet.github.io/schemas/dc/">
  <Lexicon id="oewn" label="Open English Wordnet" language="en" email="e@example.org" license="https://creativecommons.org/licenses/by/4.0" version="2025" url="https://github.com/globalwordnet/english-wordnet">
    <LexicalEntry id="oewn-cat-n">
      <Lemma writtenForm="cat" partOfSpeech="n"/>
      <Form writtenForm="cats"/>
      <Sense id="oewn-cat__1.05.00.." synset="oewn-02124272-n"/>
    </LexicalEntry>
    <LexicalEntry id="oewn-true_cat-n">
      <Lemma writtenForm="true cat" partOfSpeech="n"/>
      <Sense id="oewn-true_cat__1.05.00.." synset="oewn-02124272-n"/>
    </LexicalEntry>
    <Synset id="oewn-02124272-n" ili="i46593" members="oewn-cat-n oewn-true_cat-n" partOfSpeech="n" lexfile="noun.animal">
      <Definition>feline mammal usually having thick soft fur and no ability to roar</Definition>
      <SynsetRelation relType="hypernym" target="oewn-02123649-n"/>
      <Example>Don&apos;t pull the cat&apos;s tail</Example>
    </Synset>
  </Lexicon>
</LexicalResource>"#;

    #[test]
    fn entries_and_synsets_are_indexed_and_joined_at_lookup() {
        let d = std::env::temp_dir().join(format!("lkt-wnlmf-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("english-wordnet-2025.xml");
        std::fs::write(&path, SAMPLE).unwrap();
        assert!(detect(&path));
        let stamp = Stamp::of(&d, &[PathBuf::from("english-wordnet-2025.xml")]).unwrap();
        let index = lkidx::open_or_build(&d.join("w.lkidx"), FORMAT, &stamp, || build(&path, &|_| {})).unwrap();
        assert_eq!((index.len(), index.title.as_str()), (2, "Open English Wordnet"), "synsets are hidden entries");
        let read = |id: u32| lkidx::read(&d, &index.stamp, index.location(id).unwrap(), 1 << 20).unwrap();
        let cat = index.find("cats", 4);
        assert_eq!(cat, index.find("cat", 4));
        let entry = parse_entry(&read(cat[0])).unwrap();
        let html = entry_html(&entry,
            |id| index.find(&synset_key(id), 1).first().map(|&s| parse_synset(&read(s)).unwrap()),
            |id| index.find(&entry_key(id), 1).first().and_then(|&e| index.title(e).map(str::to_string)));
        assert!(html.contains("<h2>cat</h2>") && html.contains("noun") && html.contains("feline mammal"), "{html}");
        assert!(html.contains(&entry_link("true cat")), "synonyms come from the synset's members: {html}");
        assert!(html.contains("Don&#39;t pull the cat&#39;s tail"), "{html}");
        drop(index);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
