//! Streaming readers for published, openly documented dictionary interchange formats.
//! No network access, external entity loading, or executable dictionary markup is used.

use super::{entry_link, escape_html, ImportSink};
use anyhow::{ensure, Context, Result};
use quick_xml::{events::Event, Reader};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Cursor, Read},
    path::Path,
};

pub(super) const MAX_RECORD: usize = 8 * 1024 * 1024;
const MAX_DTD: usize = 1024 * 1024;
const MAX_DEPTH: usize = 96;

pub fn supports(path: &Path) -> bool {
    let filename = filename(path);
    path.is_dir() && ["noun", "verb", "adj", "adv"].iter().any(|pos| path.join(format!("data.{pos}")).is_file())
        || is_wordnet_file(&filename)
        || [".txt", ".u8", ".jsonl", ".jsonl.gz", ".jsonl.bz2", ".json", ".xml", ".xml.gz", ".xml.bz2", ".txt.gz", ".u8.gz"]
            .iter().any(|suffix| filename.ends_with(suffix))
}

pub(super) fn filename(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase()
}

pub(super) fn dictionary_name(path: &Path) -> String {
    let mut name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    for suffix in [".bz2", ".gz", ".xml", ".jsonl", ".json", ".txt", ".u8"] {
        if name.to_ascii_lowercase().ends_with(suffix) { name.truncate(name.len() - suffix.len()); }
    }
    if name.is_empty() { "Dicionário".into() } else { name }
}

pub(super) fn open(path: &Path) -> Result<Box<dyn BufRead>> {
    let file = File::open(path).with_context(|| format!("Não foi possível abrir {}", path.display()))?;
    let filename = filename(path);
    let stream: Box<dyn Read> = if filename.ends_with(".gz") {
        Box::new(flate2::read::MultiGzDecoder::new(file))
    } else if filename.ends_with(".bz2") {
        Box::new(bzip2::read::MultiBzDecoder::new(file))
    } else { Box::new(file) };
    Ok(Box::new(BufReader::new(stream)))
}

/// GCIDE_XML's published root uses external entities for its 26 local fragments.
/// We deliberately do NOT resolve XML external entities. Instead this known format
/// is assembled from a fixed allowlist of sibling basenames, confined to its folder.
pub(super) fn gcide_input(path: &Path) -> Result<Option<Box<dyn BufRead>>> {
    let Some((prefix, fragments)) = gcide_parts(path)? else { return Ok(None) };
    let mut stream: Box<dyn Read> = Box::new(Cursor::new(prefix));
    for fragment in fragments {
        stream = Box::new(stream.chain(File::open(fragment)?));
    }
    stream = Box::new(stream.chain(Cursor::new(GCIDE_END)));
    Ok(Some(Box::new(BufReader::new(stream))))
}

pub(super) const GCIDE_END: &[u8] = b"</dictionary>";

/// The pieces GCIDE_XML is read as: the root's DOCTYPE with a `<dictionary>` start tag, then the
/// 26 letter files; `GCIDE_END` closes it. `None` when `path` is not a GCIDE_XML file.
pub(super) fn gcide_parts(path: &Path) -> Result<Option<(Vec<u8>, Vec<std::path::PathBuf>)>> {
    let file = filename(path);
    if file != "gcide.xml" && !(file.starts_with("gcide_") && file.ends_with(".xml")) { return Ok(None); }
    let directory = path.parent().context("GCIDE sem diretório")?.canonicalize()?;
    let root = directory.join("gcide.xml");
    if !root.is_file() || !directory.join("gcide_a.xml").is_file() { return Ok(None); }
    let confined = |file: &Path| -> Result<std::path::PathBuf> {
        let resolved = file.canonicalize()?;
        ensure!(resolved.starts_with(&directory), "GCIDE: fragmento fora da pasta do dicionário");
        Ok(resolved)
    };
    let mut metadata = String::new();
    File::open(confined(&root)?)?.take((MAX_DTD + 1) as u64).read_to_string(&mut metadata)?;
    ensure!(metadata.len() <= MAX_DTD, "GCIDE: metadados excedem 1 MiB");
    let mut reader = Reader::from_str(&metadata);
    let mut prefix = String::new();
    loop {
        match reader.read_event()? {
            Event::DocType(dtd) => { prefix = format!("<!DOCTYPE {}>", dtd.decode()?); break; }
            Event::Eof => break,
            _ => {},
        }
    }
    prefix.push_str("<dictionary>");
    let mut fragments = Vec::new();
    for letter in b'a'..=b'z' {
        let fragment = directory.join(format!("gcide_{}.xml", char::from(letter)));
        // A root without all of its letter files is an incomplete dictionary, not
        // a successfully imported zero/partial-entry dictionary.
        fragments.push(confined(&fragment).with_context(|| format!("Fragmento GCIDE ausente: {}", fragment.display()))?);
    }
    Ok(Some((prefix.into_bytes(), fragments)))
}

/// `read_line` with a hard limit, including for compressed input. Returns the bytes read (0 at
/// the end), so line readers know where each record is.
fn line(reader: &mut dyn BufRead, out: &mut String) -> Result<usize> {
    out.clear();
    let count = reader.take((MAX_RECORD + 1) as u64).read_line(out)?;
    ensure!(count <= MAX_RECORD, "Registro excede o limite de 8 MiB");
    Ok(count)
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut result = Vec::new();
    for value in values {
        let value = value.trim().to_owned();
        if !value.is_empty() && !result.contains(&value) { result.push(value); }
    }
    result
}

fn heading(word: &str) -> String { format!("<h2>{}</h2>", escape_html(word)) }
fn paragraph(text: &str) -> String { format!("<p>{}</p>", escape_html(text)) }
fn link(word: &str) -> String { format!("<a href=\"{}\">{}</a>", entry_link(word), escape_html(word)) }
fn list(values: &[String]) -> String {
    let mut html = String::from("<ol>");
    for value in values { html.push_str(&format!("<li>{}</li>", escape_html(value))); }
    html.push_str("</ol>");
    html
}

pub(super) fn import_cedict(mut reader: Box<dyn BufRead>, sink: &mut dyn ImportSink) -> Result<()> {
    let mut text = String::new();
    let mut count = 0;
    let mut row = 0;
    let mut at = 0u64;
    loop {
        let read = line(&mut *reader, &mut text)?;
        if read == 0 { break; }
        let start = at;
        at += read as u64;
        row += 1;
        let text = text.trim().trim_start_matches('\u{feff}');
        if text.is_empty() || text.starts_with('#') { continue; }
        sink.span(0, start, read as u64);
        let (words, rest) = text.split_once('[').with_context(|| format!("Linha {row}: esperado CC-CEDICT com [pinyin] /definição/"))?;
        let (pinyin, rest) = rest.split_once(']').context("CC-CEDICT: pinyin sem fechamento ]")?;
        // V2 marks revised readings with double brackets; V1 and V2 coexist in downloads.
        let pinyin = pinyin.trim_start_matches('[');
        let rest = rest.trim_start_matches(']').trim();
        let words: Vec<_> = words.split_whitespace().collect();
        ensure!(words.len() == 2 && rest.starts_with('/') && rest.ends_with('/'), "Linha {row}: entrada CC-CEDICT inválida");
        let definitions = unique(rest[1..rest.len() - 1].split('/').map(str::to_owned));
        ensure!(!definitions.is_empty(), "Linha {row}: definição CC-CEDICT vazia");
        let simplified = words[1];
        let aliases = unique([words[0].to_owned()]);
        let mut html = heading(simplified);
        if words[0] != simplified { html.push_str(&paragraph(words[0])); }
        html.push_str(&paragraph(pinyin));
        html.push_str(&list(&definitions));
        sink.add(simplified, &html, &aliases)?;
        count += 1;
    }
    ensure!(count > 0, "Nenhuma entrada CC-CEDICT encontrada; esperado UTF-8 Traditional Simplified [pinyin] /gloss/");
    Ok(())
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).into_iter().flatten()
        .filter_map(Value::as_str).map(str::to_owned).collect()
}

pub(super) fn import_kaikki(mut reader: Box<dyn BufRead>, sink: &mut dyn ImportSink) -> Result<()> {
    let mut text = String::new();
    let mut count = 0;
    let mut row = 0;
    let mut at = 0u64;
    loop {
        let read = line(&mut *reader, &mut text)?;
        if read == 0 { break; }
        let start = at;
        at += read as u64;
        row += 1;
        let text = text.trim().trim_start_matches('\u{feff}');
        if text.is_empty() { continue; }
        sink.span(0, start, read as u64);
        ensure!(!text.starts_with('['), "Kaikki requer JSONL (um objeto por linha), não um array JSON");
        let entry: Value = serde_json::from_str(text).with_context(|| format!("JSONL inválido na linha {row}"))?;
        let word = entry.get("word").and_then(Value::as_str).filter(|w| !w.trim().is_empty())
            .with_context(|| format!("Linha {row}: não é uma entrada Kaikki (campo word ausente)"))?;
        let mut html = heading(word);
        let subtitle = [entry.get("lang").and_then(Value::as_str), entry.get("pos").and_then(Value::as_str)]
            .into_iter().flatten().collect::<Vec<_>>().join(" · ");
        if !subtitle.is_empty() { html.push_str(&paragraph(&subtitle)); }
        if let Some(etymology) = entry.get("etymology_text").and_then(Value::as_str) { html.push_str(&paragraph(etymology)); }
        if let Some(sounds) = entry.get("sounds").and_then(Value::as_array) {
            let pronunciations = unique(sounds.iter().filter_map(|s| s.get("ipa").and_then(Value::as_str)).map(str::to_owned));
            if !pronunciations.is_empty() { html.push_str(&paragraph(&pronunciations.join(" · "))); }
        }
        html.push_str("<ol>");
        if let Some(senses) = entry.get("senses").and_then(Value::as_array) {
            for sense in senses {
                let mut glosses = strings(sense.get("glosses"));
                if glosses.is_empty() { glosses = strings(sense.get("raw_glosses")); }
                html.push_str("<li>");
                let tags = strings(sense.get("tags"));
                if !tags.is_empty() { html.push_str(&format!("<small>{}</small> ", escape_html(&tags.join(", ")))); }
                html.push_str(&escape_html(&glosses.join("; ")));
                for field in ["form_of", "alt_of"] {
                    if let Some(forms) = sense.get(field).and_then(Value::as_array) {
                        for form in forms {
                            if let Some(base) = form.get("word").and_then(Value::as_str) {
                                html.push_str(&format!("<p>→ {}</p>", link(base)));
                            }
                        }
                    }
                }
                if let Some(examples) = sense.get("examples").and_then(Value::as_array) {
                    for example in examples {
                        if let Some(text) = example.get("text").and_then(Value::as_str) {
                            html.push_str(&format!("<blockquote>{}", escape_html(text)));
                            if let Some(translation) = example.get("translation").and_then(Value::as_str) {
                                html.push_str(&paragraph(translation));
                            }
                            html.push_str("</blockquote>");
                        }
                    }
                }
                html.push_str("</li>");
            }
        }
        html.push_str("</ol>");
        let aliases = unique(entry.get("forms").and_then(Value::as_array).into_iter().flatten()
            .filter(|f| !strings(f.get("tags")).iter().any(|t| matches!(t.as_str(), "romanization" | "transliteration")))
            .filter_map(|f| f.get("form").and_then(Value::as_str)).map(str::to_owned));
        sink.add(word, &html, &aliases)?;
        count += 1;
    }
    ensure!(count > 0, "Nenhuma entrada Kaikki JSONL encontrada");
    Ok(())
}

#[derive(Default, Debug)]
struct Node {
    name: String,
    attrs: HashMap<String, String>,
    // Ordered content preserves inline emphasis and punctuation when flattened.
    parts: Vec<Part>,
}
#[derive(Debug)]
enum Part { Text(String), Child(Node) }
impl Node {
    fn push_text(&mut self, text: String) {
        if let Some(Part::Text(previous)) = self.parts.last_mut() { previous.push_str(&text); }
        else { self.parts.push(Part::Text(text)); }
    }
    fn children(&self) -> impl Iterator<Item = &Node> {
        self.parts.iter().filter_map(|p| if let Part::Child(n) = p { Some(n) } else { None })
    }
    fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children().filter(move |n| n.name == name)
    }
    fn text(&self) -> String {
        fn append(node: &Node, value: &mut String) {
            for part in &node.parts {
                match part {
                    Part::Text(t) => value.push_str(t),
                    Part::Child(n) => {
                        let block = matches!(n.name.as_str(), "p" | "br" | "def" | "hw" | "ent" | "pos" | "sn");
                        if block { value.push(' '); }
                        append(n, value);
                        if block { value.push(' '); }
                    }
                }
            }
        }
        let mut value = String::new();
        append(self, &mut value);
        value.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    fn first(&self, name: &str) -> String {
        self.children().find(|n| n.name == name).map(Node::text).unwrap_or_default()
    }
    fn all(&self, name: &str, values: &mut Vec<String>) {
        for child in self.children() {
            if child.name == name { values.push(child.text()); } else { child.all(name, values); }
        }
    }
}

/// Only literal, internal general entities are retained, never SYSTEM/PUBLIC or parameters.
/// Values are not recursively expanded: this also prevents exponential entity expansion.
fn dtd_entities(dtd: &str, entities: &mut HashMap<String, String>) -> Result<()> {
    ensure!(dtd.len() <= MAX_DTD, "DTD excede o limite de 1 MiB");
    for declaration in dtd.split("<!ENTITY").skip(1) {
        let declaration = declaration.trim_start();
        if declaration.starts_with('%') { continue; }
        let Some(space) = declaration.find(char::is_whitespace) else { continue; };
        let (name, rest) = declaration.split_at(space);
        let rest = rest.trim_start();
        let Some(quote) = rest.chars().next().filter(|c| *c == '\'' || *c == '"') else { continue; };
        let Some(end) = rest[1..].find(quote) else { continue; };
        let value = &rest[1..end + 1];
        ensure!(entities.len() < 8192 && value.len() <= 16384, "Limite de entidades XML excedido");
        let mut decoded = String::new();
        let mut remaining = value;
        while let Some((before, after)) = remaining.split_once('&') {
            decoded.push_str(before);
            if let Some((entity, tail)) = after.split_once(';') {
                if let Some(c) = primitive_reference(entity) { decoded.push(c); }
                else { decoded.push_str(&format!("[{entity}]")); }
                remaining = tail;
            } else { decoded.push('&'); remaining = after; break; }
        }
        decoded.push_str(remaining);
        entities.insert(name.into(), decoded);
    }
    Ok(())
}

fn primitive_reference(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'), "apos" => Some('\''), "quot" => Some('"'),
        _ if name.starts_with("#x") => u32::from_str_radix(&name[2..], 16).ok().and_then(char::from_u32),
        _ if name.starts_with('#') => name[1..].parse::<u32>().ok().and_then(char::from_u32),
        _ => None,
    }
}

fn reference(name: &str, entities: &HashMap<String, String>) -> Result<String> {
    if let Some(c) = primitive_reference(name) { return Ok(c.to_string()); }
    if let Some(value) = entities.get(name) { return Ok(value.clone()); }
    // Keep a readable label for unresolved domain entities; never read a URI or local file.
    ensure!(!name.starts_with('#'), "Referência numérica XML inválida: &{name};");
    Ok(format!("[{name}]"))
}

// quick-xml buffers one token. Bound the bytes consumed by a read_event call as
// well, so even an unterminated giant text/comment token cannot fill memory.
struct XmlInput { input: Box<dyn BufRead>, remaining: usize }
impl Read for XmlInput {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() { return Ok(0); }
        if self.remaining == 0 { return Err(std::io::Error::other("Token XML excede 8 MiB")); }
        let limit = output.len().min(self.remaining);
        let count = self.input.read(&mut output[..limit])?;
        self.remaining -= count;
        Ok(count)
    }
}
impl BufRead for XmlInput {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.remaining == 0 { return Err(std::io::Error::other("Token XML excede 8 MiB")); }
        let bytes = self.input.fill_buf()?;
        Ok(&bytes[..bytes.len().min(self.remaining)])
    }
    fn consume(&mut self, amount: usize) {
        self.remaining = self.remaining.saturating_sub(amount);
        self.input.consume(amount);
    }
}

pub(super) fn import_xml(input: Box<dyn BufRead>, sink: &mut dyn ImportSink) -> Result<&'static str> {
    let mut reader = Reader::from_reader(XmlInput { input, remaining: MAX_RECORD });
    // Where the entry being read started, for `sink.span`.
    let mut entry_start = 0u64;
    let mut buffer = Vec::new();
    let mut entities = HashMap::new();
    let mut stack: Vec<Node> = Vec::new();
    let mut format = "webster";
    let mut count = 0;
    let mut article_bytes = 0;
    let mut depth = 0usize;
    let mut webster: Option<WebsterArticle> = None;
    loop {
        reader.get_mut().remaining = MAX_RECORD;
        let before = reader.buffer_position();
        let event = reader.read_event_into(&mut buffer).context("XML inválido (é necessário XML UTF-8 bem formado)")?;
        let token_size = event.len();
        ensure!(token_size <= MAX_RECORD, "Token XML excede o limite de 8 MiB");
        let empty = matches!(&event, Event::Empty(_));
        if !stack.is_empty() { article_bytes += token_size; }
        match event {
            Event::Start(tag) | Event::Empty(tag) => {
                let name = String::from_utf8(tag.name().as_ref().to_vec()).context("Nome XML não UTF-8")?.to_ascii_lowercase();
                if depth == 0 {
                    format = match name.as_str() { "jmdict" => "jmdict", "jmnedict" => "jmnedict", "kanjidic2" => "kanjidic2", _ => "webster" };
                    if !empty { sink.prolog(reader.buffer_position()); }
                }
                let entry = match format {
                    "kanjidic2" => name == "character",
                    "webster" => name == "entry" || name == "p",
                    _ => name == "entry",
                };
                if !stack.is_empty() || entry {
                    if stack.is_empty() { article_bytes = 0; entry_start = before; }
                    // Include node/container overhead, so tiny tags cannot grow an
                    // article tree far beyond the record budget.
                    article_bytes += token_size + 192;
                    let mut node = Node { name, ..Node::default() };
                    for attr in tag.attributes() {
                        let attr = attr.context("Atributo XML inválido")?;
                        let key = String::from_utf8(attr.key.as_ref().to_vec())?;
                        let value = attr.decode_and_unescape_value(reader.decoder())?.into_owned();
                        article_bytes += key.len() + value.len();
                        node.attrs.insert(key, value);
                    }
                    if empty {
                        if let Some(parent) = stack.last_mut() { parent.parts.push(Part::Child(node)); }
                        else {
                            let span = (entry_start, reader.buffer_position());
                            finish_xml(&node, format, span, sink, &mut webster, &mut count)?;
                        }
                    } else { stack.push(node); }
                }
                if !empty { depth += 1; }
                ensure!(depth <= MAX_DEPTH, "XML excede a profundidade máxima ({MAX_DEPTH})");
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).context("Fechamento XML inesperado")?;
                if let Some(node) = stack.pop() {
                    if let Some(parent) = stack.last_mut() { parent.parts.push(Part::Child(node)); }
                    else {
                        let span = (entry_start, reader.buffer_position());
                        finish_xml(&node, format, span, sink, &mut webster, &mut count)?;
                    }
                }
            }
            Event::Text(text) => {
                if let Some(node) = stack.last_mut() {
                    let text = text.decode()?.into_owned();
                    article_bytes += text.len();
                    node.push_text(text);
                }
            }
            Event::CData(text) => {
                if let Some(node) = stack.last_mut() {
                    let text = text.decode()?.into_owned();
                    article_bytes += text.len();
                    node.push_text(text);
                }
            }
            Event::GeneralRef(entity) => {
                if let Some(node) = stack.last_mut() {
                    let name = std::str::from_utf8(entity.as_ref())?;
                    let value = reference(name, &entities)?;
                    article_bytes += value.len();
                    node.push_text(value);
                }
            }
            Event::DocType(dtd) => { dtd_entities(&dtd.decode()?, &mut entities)?; }
            Event::Eof => break,
            _ => {},
        }
        ensure!(article_bytes <= MAX_RECORD, "Entrada XML excede o limite de 8 MiB");
        buffer.clear();
    }
    flush_webster(&mut webster, sink, &mut count)?;
    ensure!(depth == 0 && stack.is_empty(), "XML terminou antes de fechar a entrada");
    ensure!(count > 0, "Nenhuma entrada XML reconhecida (JMdict/JMnedict: entry; KANJIDIC2: character; Webster: entry/hw)");
    Ok(format)
}

fn finish_xml(node: &Node, format: &str, span: (u64, u64), sink: &mut dyn ImportSink, pending: &mut Option<WebsterArticle>, count: &mut usize) -> Result<()> {
    match format {
        "jmdict" | "jmnedict" => { sink.span(0, span.0, span.1 - span.0); render_japanese(node, sink)?; *count += 1; }
        "kanjidic2" => { sink.span(0, span.0, span.1 - span.0); render_kanji(node, sink)?; *count += 1; }
        _ => collect_webster(node, span, pending, sink, count)?,
    }
    Ok(())
}

fn render_japanese(node: &Node, sink: &mut dyn ImportSink) -> Result<()> {
    let mut words = Vec::new();
    let mut readings = Vec::new();
    node.all("keb", &mut words);
    node.all("reb", &mut readings);
    let aliases = unique(words.iter().chain(readings.iter()).cloned());
    let word = aliases.first().context("Entrada JMdict/JMnedict sem keb/reb")?;
    let mut html = heading(word);
    if !readings.is_empty() { html.push_str(&paragraph(&readings.join(" · "))); }
    if words.len() > 1 { html.push_str(&paragraph(&words[1..].join(" · "))); }
    html.push_str("<ol>");
    let mut inherited_pos = Vec::new();
    for sense in node.children().filter(|n| n.name == "sense" || n.name == "trans") {
        html.push_str("<li>");
        let mut labels: Vec<_> = sense.children().filter(|n| matches!(n.name.as_str(), "pos" | "name_type" | "field" | "misc" | "dial"))
            .map(Node::text).collect();
        let pos: Vec<_> = sense.named("pos").map(Node::text).collect();
        if !pos.is_empty() { inherited_pos = pos; }
        else { labels.extend(inherited_pos.iter().cloned()); }
        if !labels.is_empty() { html.push_str(&format!("<small>{}</small> ", escape_html(&labels.join("; ")))); }
        let mut glosses = Vec::new();
        for gloss in sense.children().filter(|n| n.name == "gloss" || n.name == "trans_det") {
            let text = gloss.text();
            let language = gloss.attrs.get("xml:lang").map(String::as_str).unwrap_or("eng");
            glosses.push(if language == "eng" { text } else { format!("[{language}] {text}") });
        }
        html.push_str(&escape_html(&glosses.join("; ")));
        for info in sense.children().filter(|n| matches!(n.name.as_str(), "s_inf" | "stagk" | "stagr" | "xref" | "ant" | "trans_det")) {
            if info.name == "xref" || info.name == "ant" {
                let text = info.text();
                let target = text.split('・').next().unwrap_or(&text);
                html.push_str(&format!("<p>{}: {}</p>", escape_html(&info.name), link(target)));
            } else if info.name != "trans_det" { html.push_str(&paragraph(&format!("{}: {}", info.name, info.text()))); }
        }
        html.push_str("</li>");
    }
    html.push_str("</ol>");
    sink.add(word, &html, &aliases)
}

fn render_kanji(node: &Node, sink: &mut dyn ImportSink) -> Result<()> {
    let word = node.first("literal");
    ensure!(!word.is_empty(), "Caractere KANJIDIC2 sem literal");
    let mut html = heading(&word);
    let mut readings = Vec::new();
    let mut meanings = Vec::new();
    fn visit(node: &Node, readings: &mut Vec<String>, meanings: &mut Vec<String>) {
        for child in node.children() {
            match child.name.as_str() {
                "reading" => readings.push(format!("{}: {}", child.attrs.get("r_type").map(String::as_str).unwrap_or("reading"), child.text())),
                "meaning" => {
                    let language = child.attrs.get("m_lang").map(String::as_str).unwrap_or("en");
                    let text = child.text();
                    meanings.push(if language == "en" { text } else { format!("[{language}] {text}") });
                }
                "nanori" => readings.push(format!("nanori: {}", child.text())),
                _ => visit(child, readings, meanings),
            }
        }
    }
    visit(node, &mut readings, &mut meanings);
    html.push_str(&paragraph(&readings.join(" · ")));
    html.push_str(&list(&meanings));
    let mut strokes = Vec::new();
    node.all("stroke_count", &mut strokes);
    if !strokes.is_empty() { html.push_str(&paragraph(&format!("Traços: {}", strokes.join(", ")))); }
    // Kana readings are displayed but not indexed as kanji aliases: this would produce
    // thousands of unrelated character results when looking up a common syllable.
    sink.add(&word, &html, &[])
}

/// A Webster/GCIDE article: its paragraphs run until the next headword, so its span runs from
/// its first paragraph to its last.
struct WebsterArticle { word: String, aliases: Vec<String>, html: String, span: (u64, u64) }

fn flush_webster(pending: &mut Option<WebsterArticle>, sink: &mut dyn ImportSink, count: &mut usize) -> Result<()> {
    if let Some(entry) = pending.take() {
        sink.span(0, entry.span.0, entry.span.1 - entry.span.0);
        sink.add(&entry.word, &entry.html, &entry.aliases)?;
        *count += 1;
    }
    Ok(())
}

fn collect_webster(node: &Node, span: (u64, u64), pending: &mut Option<WebsterArticle>, sink: &mut dyn ImportSink, count: &mut usize) -> Result<()> {
    let mut words = Vec::new();
    node.all("hw", &mut words);
    if words.is_empty() { node.all("headword", &mut words); }
    let mut canonical = Vec::new();
    node.all("ent", &mut canonical);
    let words = unique(canonical.into_iter().chain(words.into_iter().map(|w| w.replace(['*', '"', '`'], ""))));
    if let Some(word) = words.first() {
        flush_webster(pending, sink, count)?;
        *pending = Some(WebsterArticle { word: word.clone(), aliases: words.clone(), html: heading(word), span });
    } else if node.name == "entry" {
        anyhow::bail!("XML não reconhecido como Webster/GCIDE: entry sem hw/headword");
    }
    if let Some(entry) = pending {
        entry.span.1 = span.1;
        entry.html.push_str(&paragraph(&node.text()));
        ensure!(entry.html.len() <= MAX_RECORD, "Artigo Webster/GCIDE excede 8 MiB");
    }
    Ok(())
}

fn is_wordnet_file(filename: &str) -> bool {
    ["data.noun", "data.verb", "data.adj", "data.adv", "index.noun", "index.verb", "index.adj", "index.adv"].contains(&filename)
}

/// The WordNet data files beside `path`, with their part of speech, in a fixed order (the order
/// the in-place reader numbers them in).
pub(super) fn wordnet_files(path: &Path) -> Result<Vec<(&'static str, std::path::PathBuf)>> {
    let directory = if path.is_dir() { path } else { path.parent().context("WordNet sem diretório")? };
    Ok(["noun", "verb", "adj", "adv"].into_iter().map(|pos| (pos, directory.join(format!("data.{pos}")))).filter(|(_, p)| p.is_file()).collect())
}

/// One synset line of a WordNet data file as an entry; `exceptions` add irregular forms.
pub(super) fn wordnet_line(text: &str, pos: &str, exceptions: &HashMap<String, Vec<String>>, place: &str, sink: &mut dyn ImportSink) -> Result<()> {
    let row = place;
    let (record, gloss) = text.split_once('|').with_context(|| format!("{row}: synset sem gloss"))?;
    let mut fields = record.split_whitespace();
    let offset = fields.next().context("WordNet: offset ausente")?;
    ensure!(offset.len() == 8 && offset.bytes().all(|c| c.is_ascii_digit()), "{row}: offset inválido");
    let _lex_file = fields.next().context("WordNet: lex_filenum ausente")?;
    let kind = fields.next().context("WordNet: ss_type ausente")?;
    ensure!(["n", "v", "a", "s", "r"].contains(&kind), "WordNet: ss_type inválido");
    let words = usize::from_str_radix(fields.next().context("WordNet: w_cnt ausente")?, 16)?;
    ensure!(words > 0 && words <= 4096, "WordNet: contagem de palavras inválida");
    let mut aliases = Vec::new();
    for _ in 0..words {
        let word = fields.next().context("WordNet: synset truncado")?;
        let _lex_id = fields.next().context("WordNet: lex_id ausente")?;
        // Adjective syntactic markers are metadata, not part of the lemma.
        let word = ["(ip)", "(p)", "(a)"].iter().find_map(|m| word.strip_suffix(m)).unwrap_or(word);
        aliases.push(word.replace('_', " "));
    }
    let pointer_count = fields.next().context("WordNet: p_cnt ausente")?.parse::<usize>()?;
    ensure!(pointer_count <= 65536, "WordNet: p_cnt inválido");
    for _ in 0..pointer_count {
        for _ in 0..4 { fields.next().context("WordNet: relação de synset truncada")?; }
    }
    if kind == "v" {
        let frame_count = fields.next().context("WordNet: f_cnt ausente")?.parse::<usize>()?;
        ensure!(frame_count <= 256, "WordNet: f_cnt inválido");
        for _ in 0..frame_count {
            ensure!(fields.next() == Some("+"), "WordNet: frame verbal sem marcador +");
            fields.next().context("WordNet: frame_num ausente")?;
            fields.next().context("WordNet: word_num ausente")?;
        }
    }
    ensure!(fields.next().is_none(), "WordNet: campos inesperados no synset");
    let mut aliases = unique(aliases);
    let word = aliases.first().context("WordNet: synset sem lema")?;
    let mut html = heading(word);
    html.push_str(&format!("<p>{pos} · {}</p>", aliases.iter().map(|w| link(w)).collect::<Vec<_>>().join(", ")));
    html.push_str(&paragraph(gloss.trim()));
    let irregular = aliases.iter().filter_map(|lemma| exceptions.get(lemma)).flatten().cloned().collect::<Vec<_>>();
    aliases.extend(irregular);
    let aliases = unique(aliases);
    let word = aliases.first().context("WordNet: synset sem lema")?;
    sink.add(word, &html, &aliases)?;
    Ok(())
}

pub(super) fn wordnet_exceptions(path: &Path) -> Result<HashMap<String, Vec<String>>> {
    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    if !path.is_file() { return Ok(result); }
    let mut reader = open(path)?;
    let mut text = String::new();
    let mut bytes = 0;
    while line(&mut *reader, &mut text)? > 0 {
        bytes += text.len() + 128;
        ensure!(bytes <= MAX_RECORD, "Tabela de exceções WordNet excede 8 MiB");
        let mut fields = text.split_whitespace();
        let Some(surface) = fields.next() else { continue; };
        let mut found = false;
        for lemma in fields {
            bytes += lemma.len() + surface.len() + 128;
            ensure!(bytes <= MAX_RECORD, "Tabela de exceções WordNet excede 8 MiB");
            result.entry(lemma.replace('_', " ")).or_default().push(surface.replace('_', " "));
            found = true;
        }
        ensure!(found, "WordNet: exceção sem lema");
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[derive(Default)]
    struct Memory(Vec<(String, String, Vec<String>)>);
    impl ImportSink for Memory {
        fn add(&mut self, word: &str, html: &str, aliases: &[String]) -> Result<()> {
            self.0.push((word.into(), html.into(), aliases.to_vec())); Ok(())
        }
    }
    fn input(value: &str) -> Box<dyn BufRead> { Box::new(Cursor::new(value.as_bytes().to_vec())) }

    #[test]
    fn cedict_indexes_both_scripts_and_escapes_gloss() {
        let mut sink = Memory::default();
        import_cedict(input("# comment\n學習 学习 [xue2 xi2] /to study/<script>alert(1)/\n"), &mut sink).unwrap();
        assert_eq!(sink.0[0].0, "学习");
        assert_eq!(sink.0[0].2, ["學習"]);
        assert!(sink.0[0].1.contains("&lt;script&gt;"));
        assert!(!sink.0[0].1.contains("<script>"));
        let mut v2 = Memory::default();
        import_cedict(input("學習 学习 [[xue2xi2]] /to study/\n"), &mut v2).unwrap();
        assert!(v2.0[0].1.contains("xue2xi2"));
    }

    #[test]
    fn kaikki_keeps_forms_and_examples() {
        let mut sink = Memory::default();
        import_kaikki(input(r#"{"word":"cat","lang":"English","pos":"noun","forms":[{"form":"cats","tags":["plural"]}],"senses":[{"glosses":["a feline"],"examples":[{"text":"A cat.","translation":"Um gato."}]}]}"#), &mut sink).unwrap();
        assert_eq!(sink.0[0].2, ["cats"]);
        assert!(sink.0[0].1.contains("Um gato."));
        assert!(import_kaikki(input("[{\"word\":\"cat\"}]"), &mut sink).is_err());
    }

    #[test]
    fn jmdict_resolves_dtd_and_indexes_readings() {
        let xml = r#"<?xml version="1.0"?><!DOCTYPE JMdict [<!ENTITY n "noun (common)">]><JMdict><entry><ent_seq>1</ent_seq><k_ele><keb>猫</keb></k_ele><r_ele><reb>ねこ</reb></r_ele><sense><pos>&n;</pos><gloss>cat &amp; feline</gloss><gloss xml:lang="por">gato</gloss></sense></entry><entry><r_ele><reb>あ</reb></r_ele><sense><gloss>Ah</gloss></sense></entry></JMdict>"#;
        let mut sink = Memory::default();
        assert_eq!(import_xml(input(xml), &mut sink).unwrap(), "jmdict");
        assert_eq!(sink.0.len(), 2);
        assert_eq!(sink.0[0].0, "猫");
        assert!(sink.0[0].2.contains(&"ねこ".into()));
        assert!(sink.0[0].1.contains("noun (common)"));
        assert!(sink.0[0].1.contains("cat &amp; feline"));
        assert!(sink.0[0].1.contains("[por] gato"));
    }

    #[test]
    fn jmnedict_and_kanji() {
        let mut sink = Memory::default();
        import_xml(input("<JMnedict><entry><k_ele><keb>東京</keb></k_ele><r_ele><reb>とうきょう</reb></r_ele><trans><name_type>place</name_type><trans_det>Tokyo</trans_det></trans></entry></JMnedict>"), &mut sink).unwrap();
        assert!(sink.0[0].1.contains("Tokyo"));
        import_xml(input("<kanjidic2><character><literal>猫</literal><reading_meaning><rmgroup><reading r_type=\"ja_on\">ビョウ</reading><meaning>cat</meaning><meaning m_lang=\"pt\">gato</meaning></rmgroup></reading_meaning></character></kanjidic2>"), &mut sink).unwrap();
        assert!(sink.0[1].1.contains("ja_on: ビョウ"));
        assert!(sink.0[1].1.contains("[pt] gato"));
    }

    #[test]
    fn webster_flattens_inline_content_without_running_markup() {
        let mut sink = Memory::default();
        import_xml(input("<dictionary><entry><hw>Cat</hw><pos>n.</pos><def>A <it>small</it> animal.</def></entry></dictionary>"), &mut sink).unwrap();
        assert_eq!(sink.0[0].0, "Cat");
        assert!(sink.0[0].1.contains("A small animal."));
        let mut sink = Memory::default();
        import_xml(input("<dictionary><p>Preface</p><p><ent>paronymous</ent><hw>Pa*ron&quot;y*mous</hw><def>Similar in sound.</def></p><p><def>A second sense.</def></p><p><ent>cat</ent><hw>Cat</hw><def>A small animal.</def></p></dictionary>"), &mut sink).unwrap();
        assert_eq!(sink.0.len(), 2);
        assert_eq!(sink.0[0].0, "paronymous");
        assert!(sink.0[0].1.contains("A second sense."));
        assert!(sink.0[0].2.contains(&"Paronymous".into()));
    }

    #[test]
    fn external_entities_are_never_dereferenced() {
        let xml = r#"<!DOCTYPE dictionary [<!ENTITY leak SYSTEM "file:///C:/Windows/win.ini">]><dictionary><entry><hw>test</hw><def>&leak;</def></entry></dictionary>"#;
        let mut sink = Memory::default();
        import_xml(input(xml), &mut sink).unwrap();
        assert!(sink.0[0].1.contains("[leak]"));
        assert!(!sink.0[0].1.contains("file:///"));
        assert!(import_xml(input("<JMdict><entry><r_ele><reb>unfinished"), &mut sink).is_err());
    }

    #[test]
    fn record_limits_and_unknown_formats_are_rejected() {
        let mut sink = Memory::default();
        assert!(import_cedict(input("plain text"), &mut sink).is_err());
        assert!(import_xml(input("<root><entry>plain</entry></root>"), &mut sink).is_err());
        assert!(import_kaikki(input("{}"), &mut sink).is_err());
        let mut oversized = Cursor::new(vec![b'a'; MAX_RECORD + 1]);
        assert!(line(&mut oversized, &mut String::new()).is_err());
        let giant_token = format!("<JMdict><!--{}", "a".repeat(MAX_RECORD + 1));
        assert!(import_xml(input(&giant_token), &mut sink).is_err());
    }

    struct TempDirectory(std::path::PathBuf);
    impl TempDirectory {
        fn new(label: &str) -> Self {
            let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let directory = std::env::temp_dir().join(format!("lookitan-{label}-{}-{nonce}", std::process::id()));
            std::fs::create_dir_all(&directory).unwrap();
            Self(directory)
        }
    }
    impl Drop for TempDirectory {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn gcide_uses_fixed_local_fragments_and_dtd_characters() {
        let directory = TempDirectory::new("gcide");
        std::fs::write(directory.0.join("gcide.xml"), "<!DOCTYPE dictionary [<!ENTITY eacute '&#x00E9;'><!ENTITY evil SYSTEM 'file:///secret'>]><dictionary>&evil;</dictionary>").unwrap();
        for letter in b'a'..=b'z' {
            std::fs::write(directory.0.join(format!("gcide_{}.xml", char::from(letter))), if letter == b'a' { "<p><ent>cafe</ent><hw>Caf&eacute;</hw><def>A coffee house.</def></p>" } else { "" }).unwrap();
        }
        let mut sink = Memory::default();
        let path = directory.0.join("gcide.xml");
        import_xml(gcide_input(&path).unwrap().unwrap(), &mut sink).unwrap();
        assert_eq!(sink.0.len(), 1);
        assert!(sink.0[0].2.contains(&"Café".into()));
        assert!(!sink.0[0].1.contains("secret"));
    }
}
