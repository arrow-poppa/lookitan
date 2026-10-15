//! Independent importers for documented dictionary containers.
//!
//! Format references (no Lookupper implementation is used):
//! - https://github.com/huzheng001/stardict-3/blob/master/dict/doc/StarDictFileFormat
//! - https://github.com/yomidevs/yomitan/tree/master/ext/data/schemas
//! - https://fr7.abbyy.com/lingvo11/Guide/Guide_English.pdf (DSL specification)
//!
//! Imports are bounded by one article/JSON bank, not the complete dictionary.
//! The caller owns archive extraction and the transaction receiving these rows.

use super::{entry_link, escape_html, ImportInfo, ImportSink};
use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use encoding_rs::{Encoding, UTF_8, WINDOWS_1250, WINDOWS_1251, WINDOWS_1252};
use flate2::read::MultiGzDecoder;
use serde::de::{DeserializeSeed, Error as _, SeqAccess, Visitor};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

const MAX_LINE: usize = 4 * 1024 * 1024;
const MAX_ARTICLE: usize = 16 * 1024 * 1024;
const MAX_JSON_BANK: usize = 32 * 1024 * 1024;
const MAX_HEADER: usize = 1024 * 1024;
const MAX_HEADWORD: usize = 16 * 1024;
const MAX_BANKS: usize = 100_000;
const MAX_INDEX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_UNPACKED_DICT: u64 = 32 * 1024 * 1024 * 1024;
const MAX_ALIAS_MEMORY: usize = 32 * 1024 * 1024;
const MAX_VARIANTS: usize = 64;
const MAX_DEPTH: usize = 64;

pub(super) fn default_name(path: &Path) -> String {
    path.file_stem().unwrap_or_default().to_string_lossy().trim_end_matches(".dsl").to_string()
}

fn push_escaped_char(output: &mut String, c: char) {
    match c {
        '&' => output.push_str("&amp;"),
        '<' => output.push_str("&lt;"),
        '>' => output.push_str("&gt;"),
        '"' => output.push_str("&quot;"),
        '\'' => output.push_str("&#39;"),
        _ => output.push(c),
    }
}

fn append_text(output: &mut String, source: &str) -> Result<()> {
    for c in source.chars() {
        if c == '\n' { output.push_str("<br>"); }
        else { push_escaped_char(output, c); }
        ensure!(output.len() <= MAX_ARTICLE, "HTML do verbete excede 16 MiB");
    }
    Ok(())
}

fn add_entry(sink: &mut dyn ImportSink, word: &str, html: &str, aliases: &[String]) -> Result<()> {
    let word = word.trim();
    ensure!(!word.is_empty(), "Verbete sem palavra");
    ensure!(word.len() <= MAX_HEADWORD, "Palavra excede o limite de 16 KiB");
    ensure!(html.len() <= MAX_ARTICLE, "Verbete excede o limite de 16 MiB");
    ensure!(aliases.iter().all(|s| s.len() <= MAX_HEADWORD), "Alias muito longo");
    sink.add(word, html, aliases)
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("Abrir {}", path.display()))?;
    ensure!(file.metadata()?.len() <= limit as u64, "Arquivo excede o limite de {} MiB: {}", limit / 1024 / 1024, path.display());
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "Arquivo cresceu além do limite: {}", path.display());
    Ok(bytes)
}

fn gzip_or_file(path: &Path) -> Result<Box<dyn Read>> {
    let file = File::open(path).with_context(|| format!("Abrir {}", path.display()))?;
    let compressed = path.extension().is_some_and(|s| s.eq_ignore_ascii_case("gz") || s.eq_ignore_ascii_case("dz"));
    Ok(if compressed { Box::new(MultiGzDecoder::new(file)) } else { Box::new(file) })
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{stem}{suffix}"))
}

/// A relative resource path; the resource server additionally canonicalizes it.
fn resource_path(value: &str) -> Option<String> {
    let value = value.trim().replace('\\', "/");
    if value.is_empty() || value.len() > MAX_HEADWORD || value.starts_with('/') || value.contains([':', '\0', '\r', '\n']) {
        return None;
    }
    if value.split('/').any(|p| p == "..") {
        return None;
    }
    Some(value)
}

fn media_html(path: &str, prefix: &str) -> String {
    let Some(path) = resource_path(path) else { return escape_html(path) };
    let source = escape_html(&format!("{prefix}{path}"));
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "svg" | "webp" | "avif" | "bmp" => format!("<img src=\"{source}\" alt=\"{}\" loading=\"lazy\">", escape_html(&path)),
        "mp3" | "wav" | "ogg" | "oga" | "opus" | "flac" | "m4a" | "aac" | "spx" => format!("<audio controls preload=\"none\" src=\"{source}\"></audio>"),
        "mp4" | "webm" => format!("<video controls preload=\"none\" src=\"{source}\"></video>"),
        _ => format!("<a href=\"{source}\">{}</a>", escape_html(&path)),
    }
}

// DSL -----------------------------------------------------------------------

pub(super) struct DslLines {
    input: BufReader<Box<dyn Read>>,
    utf16: Option<bool>, // true = little endian
    encoding: &'static Encoding,
    total: u64,
}

impl DslLines {
    /// Lines of `bytes`, a span of a DSL file decoded like the whole file (`decoding`).
    pub(super) fn over(bytes: Vec<u8>, decoding: (Option<bool>, &'static Encoding)) -> Self {
        let input: Box<dyn Read> = Box::new(std::io::Cursor::new(bytes));
        Self { input: BufReader::new(input), utf16: decoding.0, encoding: decoding.1, total: 0 }
    }

    /// The text encoding: UTF-16 (`Some(little endian)`) or the 8-bit encoding.
    pub(super) fn decoding(&self) -> (Option<bool>, &'static Encoding) {
        (self.utf16, self.encoding)
    }

    pub(super) fn new(path: &Path) -> Result<Self> {
        let mut input = BufReader::with_capacity(16 * 1024, gzip_or_file(path)?);
        let prefix = input.fill_buf()?;
        let (utf16, bom) = if prefix.starts_with(&[0xff, 0xfe]) {
            (Some(true), 2)
        } else if prefix.starts_with(&[0xfe, 0xff]) {
            (Some(false), 2)
        } else {
            (None, if prefix.starts_with(&[0xef, 0xbb, 0xbf]) { 3 } else { 0 })
        };
        // ANSI DSL declares its codepage in the ASCII header. Inspect only the
        // buffered prefix; never read the dictionary into memory for detection.
        let sniff = String::from_utf8_lossy(prefix);
        let page = sniff.lines().find(|l| l.trim_start().starts_with("#SOURCE_CODE_PAGE")).unwrap_or("");
        let encoding = if page.contains("Cyrillic") || page.contains("1251") {
            WINDOWS_1251
        } else if page.contains("EasternEuropean") || page.contains("1250") {
            WINDOWS_1250
        } else if page.contains("Latin") || page.contains("1252") {
            WINDOWS_1252
        } else {
            UTF_8
        };
        input.consume(bom);
        Ok(Self { input, utf16, encoding, total: bom as u64 })
    }

    fn next(&mut self) -> Result<Option<String>> {
        let mut bytes = Vec::new();
        if let Some(little) = self.utf16 {
            loop {
                let mut pair = [0u8; 2];
                let n = self.input.read(&mut pair[..1])?;
                if n == 0 { break; }
                self.input.read_exact(&mut pair[1..]).context("DSL UTF-16 truncado")?;
                let unit = if little { u16::from_le_bytes(pair) } else { u16::from_be_bytes(pair) };
                bytes.extend_from_slice(&pair);
                ensure!(bytes.len() <= MAX_LINE, "Linha DSL excede 4 MiB");
                if unit == 10 { break; }
            }
        } else {
            // read_until alone can allocate an unbounded line on corrupt input.
            loop {
                let available = self.input.fill_buf()?;
                if available.is_empty() { break; }
                let end = available.iter().position(|b| *b == b'\n').map(|p| p + 1);
                let n = end.unwrap_or(available.len());
                ensure!(bytes.len() + n <= MAX_LINE, "Linha DSL excede 4 MiB");
                bytes.extend_from_slice(&available[..n]);
                self.input.consume(n);
                if end.is_some() { break; }
            }
        }
        if bytes.is_empty() { return Ok(None); }
        self.total += bytes.len() as u64;
        ensure!(self.total <= MAX_UNPACKED_DICT, "DSL descompactado excede 32 GiB");
        let line = if let Some(little) = self.utf16 {
            let units: Vec<u16> = bytes.chunks_exact(2).map(|p| if little { u16::from_le_bytes([p[0], p[1]]) } else { u16::from_be_bytes([p[0], p[1]]) }).collect();
            String::from_utf16(&units).context("Codificação UTF-16 inválida no DSL")?
        } else {
            let (decoded, _, errors) = self.encoding.decode(&bytes);
            ensure!(!errors, "Codificação DSL inválida; salve em UTF-8/UTF-16 ou declare #SOURCE_CODE_PAGE");
            decoded.into_owned()
        };
        Ok(Some(line.trim_end_matches(['\r', '\n']).to_string()))
    }
}

fn without_dsl_comments(line: &str, in_comment: &mut bool) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if !*in_comment && c == '\\' {
            out.push(c);
            if let Some(c) = chars.next() { out.push(c); }
        } else if !*in_comment && c == '{' && chars.peek() == Some(&'{') {
            chars.next();
            *in_comment = true;
        } else if *in_comment && c == '}' && chars.peek() == Some(&'}') {
            chars.next();
            *in_comment = false;
        } else if !*in_comment {
            out.push(c);
        }
    }
    out
}

fn dsl_headwords(text: &str) -> Vec<String> {
    // Optional parenthesized parts produce a bounded set of lookup variants.
    // Unsorted {...} sections are excluded from keys, per the DSL specification.
    let mut variants = vec![String::new()];
    let mut optional = String::new();
    let mut depth = 0usize;
    let mut unsorted = 0usize;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(c) = chars.next() {
                if depth > 0 { optional.push(c); } else if unsorted == 0 { for s in &mut variants { s.push(c); } }
            }
            continue;
        }
        if c == '{' { unsorted += 1; continue; }
        if c == '}' { unsorted = unsorted.saturating_sub(1); continue; }
        if unsorted > 0 { continue; }
        match c {
            '(' => { depth += 1; if depth > 1 { optional.push(c); } }
            ')' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let n = variants.len();
                    if n < MAX_VARIANTS {
                        for i in 0..n.min(MAX_VARIANTS - n) { variants.push(format!("{}{}", variants[i], optional)); }
                    }
                    optional.clear();
                } else { optional.push(c); }
            }
            _ if depth > 0 => optional.push(c),
            _ => { for s in &mut variants { s.push(c); } }
        }
    }
    variants.retain(|s| !s.trim().is_empty());
    for s in &mut variants { *s = s.trim().to_string(); }
    variants.sort();
    variants.dedup();
    variants
}

/// Reads DSL articles from `lines`. Before each article's entries, `sink.span` receives the
/// article's place in the decompressed file, so the article can be read again in place.
pub(super) fn dsl_entries(mut lines: DslLines, mut name: String, sink: &mut dyn ImportSink) -> Result<ImportInfo> {
    let mut description = String::new();
    let mut headings: Vec<String> = Vec::new();
    let mut body = String::new();
    let mut comment = false;
    let mut start = 0u64;
    loop {
        let at = lines.total;
        let Some(line) = lines.next()? else { break };
        let line = without_dsl_comments(&line, &mut comment);
        if line.trim().is_empty() { continue; }
        if line.starts_with('#') && headings.is_empty() {
            let (key, val) = line.split_once(char::is_whitespace).unwrap_or((&line, ""));
            let val = val.trim().trim_matches('"');
            if key == "#NAME" { name = val.to_string(); }
            if key == "#INDEX_LANGUAGE" || key == "#CONTENTS_LANGUAGE" {
                if !description.is_empty() { description.push_str(" → "); }
                description.push_str(val);
            }
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if !headings.is_empty() {
                ensure!(body.len() + line.len() + 1 <= MAX_ARTICLE, "Verbete DSL excede 16 MiB");
                body.push_str(line.trim_start());
                body.push('\n');
            }
        } else {
            if !body.is_empty() {
                sink.span(0, start, at - start);
                emit_dsl(sink, &headings, &body)?;
                headings.clear();
                body.clear();
            }
            if headings.is_empty() { start = at; }
            ensure!(line.len() <= MAX_HEADWORD, "Cabeçalho DSL excede 16 KiB");
            for heading in dsl_headwords(&line) {
                ensure!(headings.len() < 1024, "Muitos cabeçalhos no mesmo verbete DSL");
                ensure!(heading.len() <= MAX_HEADWORD, "Cabeçalho DSL muito longo");
                if !headings.contains(&heading) { headings.push(heading); }
            }
        }
    }
    ensure!(!comment, "Comentário DSL não terminado");
    if !headings.is_empty() && !body.is_empty() {
        sink.span(0, start, lines.total - start);
        emit_dsl(sink, &headings, &body)?;
    }
    Ok(ImportInfo { name, format: "dsl", description })
}

fn emit_dsl(sink: &mut dyn ImportSink, headings: &[String], body: &str) -> Result<()> {
    let Some(first) = headings.first() else { return Ok(()) };
    let html = format!("<div class=\"dsl-entry\">{}</div>", render_dsl(body, first)?);
    add_entry(sink, first, &html, &headings[1..])?;
    let mut subheads: Vec<String> = Vec::new();
    let mut subbody = String::new();
    let mut count = 0usize;
    for line in body.lines().chain(std::iter::once("@")) {
        if let Some(heading) = line.trim_start().strip_prefix('@') {
            if !subbody.is_empty() && !subheads.is_empty() {
                count += 1;
                ensure!(count <= 1024, "Muitos subverbetes DSL");
                let html = render_dsl(&subbody, &subheads[0])?;
                add_entry(sink, &subheads[0], &html, &subheads[1..])?;
                subheads.clear();
                subbody.clear();
            }
            if !heading.trim().is_empty() {
                let expanded = expand_dsl_tildes(heading, first, MAX_HEADWORD)?;
                subheads.extend(dsl_headwords(&expanded));
                ensure!(subheads.len() <= 1024, "Muitos cabeçalhos de subverbete DSL");
            }
        } else if !subheads.is_empty() {
            subbody.push_str(line);
            subbody.push('\n');
        }
    }
    Ok(())
}

fn expand_dsl_tildes(source: &str, headword: &str, limit: usize) -> Result<String> {
    let mut output = String::new();
    for c in source.chars() {
        if c == '~' {
            ensure!(output.len() + headword.len() <= limit, "Referência DSL excede o limite");
            output.push_str(headword);
        } else {
            ensure!(output.len() + c.len_utf8() <= limit, "Referência DSL excede o limite");
            output.push(c);
        }
    }
    Ok(output)
}

fn render_dsl(source: &str, headword: &str) -> Result<String> {
    fn part(source: &str, headword: &str, depth: usize) -> Result<String> {
        ensure!(depth <= MAX_DEPTH, "DSL aninhado além de 64 níveis");
        let mut out = String::new();
        let mut stack: Vec<(&str, String)> = Vec::new();
        let mut p = 0;
        while p < source.len() {
            ensure!(out.len() <= MAX_ARTICLE, "HTML DSL excede 16 MiB");
            let rest = &source[p..];
            let c = rest.chars().next().unwrap();
            if c == '\\' {
                p += 1;
                if let Some(c) = source[p..].chars().next() { push_escaped_char(&mut out, c); p += c.len_utf8(); }
                continue;
            }
            if rest.starts_with("<<") {
                if let Some(end) = rest[2..].find(">>") {
                    let target = expand_dsl_tildes(&rest[2..end + 2], headword, MAX_HEADWORD)?;
                    out.push_str(&format!("<a href=\"{}\">{}</a>", entry_link(target.trim()), escape_html(target.trim())));
                    p += end + 4;
                    continue;
                }
            }
            if c == '[' {
                if let Some(end) = rest.find(']') {
                    let tag = &rest[1..end];
                    ensure!(tag.len() <= MAX_HEADWORD, "Tag DSL muito longa");
                    let key = tag.split(|c: char| c.is_whitespace() || c == '=').next().unwrap_or("");
                    if matches!(key, "ref" | "url" | "s") {
                        let close = format!("[/{key}]");
                        if let Some(e) = rest[end + 1..].find(&close) {
                            let value = &rest[end + 1..end + 1 + e];
                            let target = expand_dsl_tildes(value, headword, MAX_HEADWORD)?;
                            match key {
                                "ref" => out.push_str(&format!("<a href=\"{}\">{}</a>", entry_link(target.trim()), part(value, headword, depth + 1)?)),
                                "url" => {
                                    let url = tag.strip_prefix("url").unwrap_or("").trim().strip_prefix('=').unwrap_or(target.trim()).trim_matches('"');
                                    if url.starts_with("https://") || url.starts_with("http://") { out.push_str(&format!("<a href=\"{}\">{}</a>", escape_html(url), part(value, headword, depth + 1)?)); } else { out.push_str(&escape_html(value)); }
                                }
                                _ => out.push_str(&media_html(target.trim(), "")),
                            }
                            p += end + 1 + e + close.len();
                            continue;
                        }
                    }
                    let (name, open, close) = match key {
                        "b" => ("b", "<b>".to_string(), "</b>"),
                        "i" => ("i", "<i>".to_string(), "</i>"),
                        "u" => ("u", "<u>".to_string(), "</u>"),
                        "sub" => ("sub", "<sub>".to_string(), "</sub>"),
                        "sup" => ("sup", "<sup>".to_string(), "</sup>"),
                        "*" => ("*", "<details class=\"dsl-optional\"><summary>…</summary>".to_string(), "</details>"),
                        "c" => {
                            let color = tag.strip_prefix('c').unwrap_or("").trim();
                            let css = if color.chars().all(|c| c.is_ascii_alphanumeric() || c == '#') && !color.is_empty() { format!(" style=\"color:{}\"", escape_html(color)) } else { String::new() };
                            ("c", format!("<span class=\"dsl-color\"{css}>"), "</span>")
                        }
                        "ex" | "com" | "p" | "trn" | "t" | "!trs" | "lang" => (key, format!("<span class=\"dsl-{}\">", escape_html(key.trim_start_matches('!'))), "</span>"),
                        _ if key.starts_with('m') && key.len() == 2 && key.as_bytes()[1].is_ascii_digit() => ("m", format!("<div class=\"dsl-indent\" style=\"margin-left:{}em\">", key.as_bytes()[1] - b'0'), "</div>"),
                        _ => ("", String::new(), ""),
                    };
                    if let Some(closing) = key.strip_prefix('/') {
                        if let Some(i) = stack.iter().rposition(|(name, _)| *name == closing) {
                            while stack.len() > i { out.push_str(&stack.pop().unwrap().1); }
                        }
                    } else if !name.is_empty() && stack.len() < MAX_DEPTH {
                        out.push_str(&open);
                        stack.push((name, close.to_string()));
                    }
                    // Unrecognized DSL annotations are non-rendering markup.
                    p += end + 1;
                    continue;
                }
            }
            match c {
                '~' => out.push_str(&escape_html(headword)),
                '\n' => out.push_str("<br>"),
                _ => push_escaped_char(&mut out, c),
            }
            p += c.len_utf8();
        }
        while let Some((_, close)) = stack.pop() { out.push_str(&close); }
        ensure!(out.len() <= MAX_ARTICLE, "HTML DSL excede 16 MiB");
        Ok(out)
    }
    part(source, headword, 0)
}

// StarDict ------------------------------------------------------------------

/// Created with create_new, removed on every return path, and never placed next
/// to the user's source dictionary. Close the handle before unlinking on Windows.
fn zero_string(reader: &mut dyn BufRead, max: usize) -> Result<Option<(String, usize)>> {
    let mut bytes = Vec::new();
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            ensure!(bytes.is_empty(), "Índice StarDict truncado antes do terminador");
            return Ok(None);
        }
        let end = buf.iter().position(|b| *b == 0);
        let n = end.unwrap_or(buf.len());
        ensure!(bytes.len() + n <= max, "Palavra muito longa no índice StarDict");
        bytes.extend_from_slice(&buf[..n]);
        reader.consume(n + usize::from(end.is_some()));
        if end.is_some() { break; }
    }
    let size = bytes.len() + 1;
    let word = String::from_utf8(bytes).context("Palavra StarDict não é UTF-8")?;
    ensure!(!word.is_empty(), "Palavra vazia no índice StarDict");
    Ok(Some((word, size)))
}

fn read_be32(reader: &mut dyn Read) -> Result<u32> {
    let mut buf = [0; 4];
    reader.read_exact(&mut buf).context("Índice StarDict truncado")?;
    Ok(u32::from_be_bytes(buf))
}

fn stardict_aliases(path: &Path) -> Result<(HashMap<u32, Vec<String>>, u64)> {
    let path = sibling(path, ".syn");
    if !path.is_file() { return Ok((HashMap::new(), 0)); }
    let mut reader = BufReader::new(File::open(path)?);
    let mut aliases: HashMap<u32, Vec<String>> = HashMap::new();
    let mut memory = 0usize;
    let mut count = 0u64;
    while let Some((alias, _)) = zero_string(&mut reader, MAX_HEADWORD)? {
        let entry = read_be32(&mut reader)?;
        memory += alias.len() + 128;
        ensure!(memory <= MAX_ALIAS_MEMORY, "Aliases StarDict excedem o limite de memória de 32 MiB");
        aliases.entry(entry).or_default().push(alias);
        count += 1;
    }
    Ok((aliases, count))
}

pub(super) struct StarDictInfo {
    pub name: String,
    pub description: String,
    /// `sametypesequence`: the field types when every entry has the same ones.
    pub sequence: String,
    offset64: bool,
    count: u64,
    idx_size: u64,
    synonyms: Option<u64>,
}

/// The .ifo header of a StarDict dictionary.
pub(super) fn stardict_info(path: &Path) -> Result<StarDictInfo> {
    let header = String::from_utf8(read_bounded(path, MAX_HEADER)?)?;
    let mut lines = header.split(['\r', '\n']);
    ensure!(lines.next().unwrap_or("").trim_end_matches('\r') == "StarDict's dict ifo file", "Este .ifo não é um dicionário StarDict normal");
    let props: HashMap<&str, &str> = lines.filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim(), v.trim())).collect();
    let get = |key| props.get(key).copied().unwrap_or("");
    ensure!(matches!(get("version"), "2.4.2" | "3.0.0"), "Versão StarDict não suportada");
    let count: u64 = get("wordcount").parse().context("wordcount inválido no StarDict")?;
    let idx_size: u64 = get("idxfilesize").parse().context("idxfilesize inválido no StarDict")?;
    ensure!(idx_size <= MAX_INDEX_BYTES, "Índice StarDict excede 2 GiB");
    ensure!(count <= u32::MAX as u64, "Contagem StarDict fora do intervalo de 32 bits");
    let offset_bits = if get("idxoffsetbits").is_empty() { "32" } else { get("idxoffsetbits") };
    ensure!(matches!(offset_bits, "32" | "64"), "idxoffsetbits deve ser 32 ou 64");
    let sequence = get("sametypesequence");
    ensure!(sequence.bytes().all(|b| b.is_ascii_alphabetic()), "sametypesequence inválido");
    Ok(StarDictInfo {
        name: if get("bookname").is_empty() { default_name(path) } else { get("bookname").to_string() },
        description: get("description").to_string(),
        sequence: sequence.to_string(),
        offset64: offset_bits == "64",
        count,
        idx_size,
        synonyms: if get("synwordcount").is_empty() { None } else { Some(get("synwordcount").parse().context("synwordcount inválido no StarDict")?) },
    })
}

/// The files of a StarDict dictionary beside its .ifo: index, data and (if any) synonyms.
pub(super) fn stardict_files(path: &Path) -> Result<(PathBuf, PathBuf, Option<PathBuf>)> {
    let idx = [".idx", ".idx.gz"].iter().map(|x| sibling(path, x)).find(|p| p.is_file())
        .ok_or_else(|| anyhow!("No .idx file next to '{}'.", path.display()))?;
    let dict = [".dict", ".dict.dz"].iter().map(|x| sibling(path, x)).find(|p| p.is_file())
        .ok_or_else(|| anyhow!("No .dict file next to '{}'.", path.display()))?;
    let syn = Some(sibling(path, ".syn")).filter(|p| p.is_file());
    Ok((idx, dict, syn))
}

/// Every entry of the .idx: its word, its place in the decompressed .dict and its synonyms.
pub(super) fn stardict_index(path: &Path, info: &StarDictInfo, data_len: u64,
    mut entry: impl FnMut(&str, u64, u32, &[String]) -> Result<()>) -> Result<()> {
    let (mut aliases, alias_count) = stardict_aliases(path)?;
    if let Some(expected) = info.synonyms {
        ensure!(expected == alias_count, "Contagem de aliases StarDict incorreta");
    }
    let (idx_path, _, _) = stardict_files(path)?;
    let mut idx = BufReader::new(gzip_or_file(&idx_path)?);
    let mut count = 0u64;
    let mut index_bytes = 0u64;
    while let Some((word, n)) = zero_string(&mut idx, MAX_HEADWORD)? {
        let offset = if info.offset64 {
            let mut buf = [0; 8];
            idx.read_exact(&mut buf)?;
            u64::from_be_bytes(buf)
        } else { read_be32(&mut idx)? as u64 };
        let size = read_be32(&mut idx)?;
        index_bytes += n as u64 + if info.offset64 { 12 } else { 8 };
        ensure!(index_bytes <= MAX_INDEX_BYTES && count < info.count, "Índice StarDict maior que a contagem declarada");
        ensure!(size as usize <= MAX_ARTICLE, "Verbete StarDict excede 16 MiB");
        ensure!(offset.checked_add(size as u64).is_some_and(|end| end <= data_len), "Offset StarDict fora do arquivo .dict");
        let entry_aliases = aliases.remove(&(count as u32)).unwrap_or_default();
        entry(&word, offset, size, &entry_aliases)?;
        count += 1;
    }
    ensure!(count == info.count && index_bytes == info.idx_size, "Contagem ou tamanho do índice StarDict não confere");
    ensure!(aliases.is_empty(), "Alias StarDict aponta para um verbete inexistente");
    Ok(())
}

pub(super) fn stardict_article(bytes: &[u8], sequence: &str) -> Result<String> {
    let mut html = String::new();
    let mut p = 0usize;
    let fixed = sequence.as_bytes();
    let mut field = 0usize;
    while if fixed.is_empty() { p < bytes.len() } else { field < fixed.len() } {
        let kind = if fixed.is_empty() {
            let kind = bytes[p]; p += 1; kind
        } else { fixed[field] };
        ensure!(kind.is_ascii_alphabetic(), "Tipo StarDict inválido");
        let last = !fixed.is_empty() && field + 1 == fixed.len();
        let content = if last {
            let value = &bytes[p..]; p = bytes.len(); value
        } else if kind.is_ascii_uppercase() {
            ensure!(p + 4 <= bytes.len(), "Comprimento de campo StarDict truncado");
            let size = u32::from_be_bytes(bytes[p..p + 4].try_into().unwrap()) as usize;
            p += 4;
            ensure!(size <= bytes.len() - p, "Campo binário StarDict truncado");
            let value = &bytes[p..p + size]; p += size; value
        } else {
            let end = bytes[p..].iter().position(|b| *b == 0).ok_or_else(|| anyhow!("Campo StarDict sem terminador"))?;
            let value = &bytes[p..p + end]; p += end + 1; value
        };
        if kind == b'W' || kind == b'P' {
            let mime = if kind == b'W' { "audio/wav" } else if content.starts_with(b"\x89PNG") { "image/png" } else if content.starts_with(b"\xff\xd8") { "image/jpeg" } else if content.starts_with(b"GIF8") { "image/gif" } else { "application/octet-stream" };
            if kind == b'W' { html.push_str(&format!("<audio controls preload=\"none\" src=\"data:{mime};base64,{}\"></audio>", STANDARD.encode(content))); }
            else { html.push_str(&format!("<img alt=\"\" src=\"data:{mime};base64,{}\">", STANDARD.encode(content))); }
        } else if kind.is_ascii_lowercase() {
            let text = std::str::from_utf8(content).context("Texto StarDict não é UTF-8")?.trim_end_matches('\0');
            let body = match kind {
                b'h' | b'g' => safe_markup(text)?,
                b'x' => safe_markup_kind(text, true)?,
                b'r' => text.lines().filter_map(|line| line.split_once(':')).map(|(_, rel)| media_html(rel, "res/")).collect::<Vec<_>>().join("<br>"),
                _ => escape_html(text).replace('\n', "<br>"),
            };
            html.push_str(&format!("<div class=\"stardict-field stardict-{}\">{body}</div>", kind as char));
        }
        ensure!(html.len() <= MAX_ARTICLE, "HTML StarDict excede 16 MiB");
        field += 1;
    }
    ensure!(p == bytes.len(), "Dados restantes no verbete StarDict");
    Ok(html)
}

// HTML fields are dictionary data, not executable application code. Preserve
// the useful formatting subset and discard script/event handlers and embeds.
fn safe_markup(source: &str) -> Result<String> {
    safe_markup_kind(source, false)
}

fn safe_markup_kind(source: &str, xdxf: bool) -> Result<String> {
    let mut out = String::new();
    let mut p = 0usize;
    while p < source.len() {
        ensure!(out.len() <= MAX_ARTICLE, "HTML StarDict excede 16 MiB");
        if source.as_bytes()[p] == b'<' {
            if let Some(end) = source[p..].find('>') {
                let raw = &source[p + 1..p + end];
                let closing = raw.starts_with('/');
                let content = raw.trim_start_matches('/').trim();
                let tag = content.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_ascii_lowercase();
                if matches!(tag.as_str(), "kref" | "rref") && !closing {
                    let close = format!("</{tag}>");
                    if let Some(n) = source[p + end + 1..].find(&close) {
                        let value = &source[p + end + 1..p + end + 1 + n];
                        ensure!(value.len() <= MAX_HEADWORD, "Referência XDXF muito longa");
                        if tag == "kref" {
                            let attrs = html_attributes(&content[tag.len()..]);
                            let target = attrs.iter().find(|(k, _)| k == "k").map(|(_, v)| v.as_str()).unwrap_or(value);
                            out.push_str(&format!("<a href=\"{}\">{}</a>", entry_link(target), escape_html(value)));
                        } else {
                            out.push_str(&media_html(value, "res/"));
                        }
                        p += end + 1 + n + close.len();
                        continue;
                    }
                }
                if matches!(tag.as_str(), "script" | "style" | "iframe" | "object" | "embed") && !closing {
                    let close = format!("</{tag}");
                    if let Some(n) = source[p + end + 1..].to_ascii_lowercase().find(&close) {
                        let tail = p + end + 1 + n;
                        p = source[tail..].find('>').map(|n| tail + n + 1).unwrap_or(source.len());
                        continue;
                    }
                    break;
                }
                let mapped = match tag.as_str() { "k" => "b", "tr" if xdxf => "span", "dtrn" | "def" | "ex" | "co" | "abr" | "c" => "span", _ => tag.as_str() };
                if matches!(mapped, "a" | "abbr" | "b" | "strong" | "i" | "em" | "u" | "s" | "sub" | "sup" | "small" | "span" | "div" | "p" | "br" | "hr" | "ol" | "ul" | "li" | "dl" | "dt" | "dd" | "table" | "thead" | "tbody" | "tr" | "td" | "th" | "ruby" | "rt" | "rp" | "img" | "audio" | "video" | "source" | "details" | "summary" | "blockquote") {
                    out.push('<');
                    if closing { out.push('/'); }
                    out.push_str(mapped);
                    if !closing {
                        for (key, value) in html_attributes(&content[tag.len()..]) {
                            let value = match key.as_str() {
                                "href" => {
                                    if let Some(target) = value.strip_prefix("bword://") { Some(entry_link(&percent_decode_url(target, false))) }
                                    else if value.starts_with("entry://") || value.starts_with("https://") || value.starts_with("http://") || value.starts_with('#') { Some(value) }
                                    else { resource_path(&value) }
                                }
                                "src" => resource_path(&value),
                                "style" => Some(safe_inline_css(&value)),
                                "class" | "title" | "alt" | "colspan" | "rowspan" | "width" | "height" | "lang" => Some(value),
                                _ => None,
                            };
                            if let Some(value) = value { out.push_str(&format!(" {key}=\"{}\"", escape_html(&value))); }
                        }
                        if matches!(mapped, "audio" | "video") { out.push_str(" controls preload=\"none\""); }
                    }
                    out.push('>');
                }
                p += end + 1;
                continue;
            }
        }
        let c = source[p..].chars().next().unwrap();
        // Retain legitimate entities without interpreting any markup from them.
        if c == '&' {
            if let Some(end) = source[p..].find(';').filter(|n| *n <= 16) {
                let entity = &source[p + 1..p + end];
                if !entity.is_empty() && entity.chars().all(|c| c.is_ascii_alphanumeric() || c == '#') {
                    out.push_str(&source[p..p + end + 1]); p += end + 1; continue;
                }
            }
        }
        push_escaped_char(&mut out, c);
        p += c.len_utf8();
    }
    ensure!(out.len() <= MAX_ARTICLE, "HTML StarDict excede 16 MiB");
    Ok(out)
}

fn html_attributes(mut source: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    while !source.trim_start().is_empty() && attrs.len() < 64 {
        source = source.trim_start();
        let n = source.find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_').unwrap_or(source.len());
        if n == 0 { source = &source[source.chars().next().unwrap().len_utf8()..]; continue; }
        let key = source[..n].to_ascii_lowercase();
        source = source[n..].trim_start();
        if !source.starts_with('=') { continue; }
        source = source[1..].trim_start();
        let Some(first) = source.chars().next() else { break };
        let value;
        if first == '\'' || first == '"' {
            source = &source[1..];
            let end = source.find(first).unwrap_or(source.len());
            value = source[..end].to_string();
            source = &source[(end + usize::from(end < source.len())).min(source.len())..];
        } else {
            let end = source.find(char::is_whitespace).unwrap_or(source.len());
            value = source[..end].trim_end_matches('/').to_string();
            source = &source[end..];
        }
        // Attribute values are HTML, so decode their references before validating
        // them and escaping exactly once when serializing the resulting markup.
        let value = quick_xml::escape::unescape(&value).map(|v| v.into_owned()).unwrap_or_else(|_| value.clone());
        attrs.push((key, value));
    }
    attrs
}

fn safe_css_value(value: &str) -> bool {
    value.len() < 2048 && !value.contains([';', '{', '}', '<', '>', '\\', '"', '\''])
        && !value.to_ascii_lowercase().contains("url(")
        && !value.to_ascii_lowercase().contains("expression")
        && !value.to_ascii_lowercase().contains("@import")
}

fn safe_inline_css(value: &str) -> String {
    value.split(';').filter_map(|p| p.split_once(':')).filter_map(|(k, v)| {
        let k = k.trim().to_ascii_lowercase();
        let allowed = matches!(k.as_str(), "color" | "background-color" | "font-size" | "font-weight" | "font-style" | "font-family" | "text-decoration" | "text-decoration-line" | "text-align" | "vertical-align" | "white-space" | "margin" | "margin-left" | "padding" | "padding-left" | "border" | "border-color" | "border-style" | "border-width" | "border-radius" | "display" | "width" | "height" | "max-width" | "max-height");
        (allowed && safe_css_value(v)).then(|| format!("{k}:{}", v.trim()))
    }).collect::<Vec<_>>().join(";")
}

// Yomitan -------------------------------------------------------------------

fn json_file(path: &Path, max: usize) -> Result<Value> {
    serde_json::from_slice(&read_bounded(path, max)?).with_context(|| format!("JSON inválido: {}", path.display()))
}

/// Walk a JSON array a row at a time. serde_json validates each row and retains
/// its normal nesting limit; a bank is never materialized as one giant Value.
fn json_rows(path: &Path, mut callback: impl FnMut(Value) -> Result<()>) -> Result<()> {
    struct Rows<'a, F>(&'a mut F);
    impl<'de, F: FnMut(Value) -> Result<()>> Visitor<'de> for Rows<'_, F> {
        type Value = ();
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("um banco JSON contendo um array")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> std::result::Result<(), A::Error> {
            while let Some(row) = sequence.next_element::<Value>()? {
                (self.0)(row).map_err(A::Error::custom)?;
            }
            Ok(())
        }
    }
    impl<'de, F: FnMut(Value) -> Result<()>> DeserializeSeed<'de> for Rows<'_, F> {
        type Value = ();
        fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> std::result::Result<(), D::Error> {
            deserializer.deserialize_seq(self)
        }
    }
    let file = File::open(path)?;
    ensure!(file.metadata()?.len() <= MAX_JSON_BANK as u64, "Banco JSON excede 32 MiB: {}", path.display());
    let mut decoder = serde_json::Deserializer::from_reader(BufReader::new(file.take(MAX_JSON_BANK as u64 + 1)));
    Rows(&mut callback).deserialize(&mut decoder).with_context(|| format!("Banco inválido: {}", path.display()))?;
    decoder.end()?;
    Ok(())
}

fn banks(dir: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() { continue; }
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(n) = name.strip_prefix(prefix).and_then(|s| s.strip_suffix(".json")).and_then(|s| s.parse::<u64>().ok()) {
            ensure!(files.len() < MAX_BANKS, "Mais de 100 mil bancos no dicionário Yomitan");
            files.push((n, entry.path()));
        }
    }
    files.sort_by_key(|(n, _)| *n);
    Ok(files.into_iter().map(|(_, p)| p).collect())
}

/// A Yomitan dictionary's index.json: its title, description and format version.
pub(super) fn yomitan_index(path: &Path) -> Result<(String, String, u64, Value)> {
    let index = json_file(path, MAX_HEADER)?;
    let name = index.get("title").and_then(Value::as_str).filter(|s| !s.trim().is_empty()).ok_or_else(|| anyhow!("Yomitan index.json sem title"))?.to_string();
    let version = index.get("format").or_else(|| index.get("version")).and_then(Value::as_u64).unwrap_or(0);
    ensure!(matches!(version, 1..=3), "Formato Yomitan não suportado: {version}");
    let description = index.get("description").and_then(Value::as_str).unwrap_or("").to_string();
    Ok((name, description, version, index))
}

/// The bank files of a Yomitan dictionary folder, in bank order.
pub(super) fn yomitan_banks(dir: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    banks(dir, prefix)
}

pub(super) fn yomitan_tag_notes(dir: &Path, index: &Value) -> Result<HashMap<String, String>> {
    yomitan_tags(dir, index)
}

/// The word and reading of a term row, without rendering it (for indexing).
pub(super) fn yomitan_term_keys(row: &[Value]) -> Result<(String, String)> {
    let word = value_str(row.first()).ok_or_else(|| anyhow!("Termo Yomitan sem palavra"))?;
    let reading = value_str(row.get(1)).unwrap_or("");
    ensure!(word.len() <= MAX_HEADWORD && reading.len() <= MAX_HEADWORD, "Palavra/leitura Yomitan excede 16 KiB");
    Ok((word.to_string(), reading.to_string()))
}

/// One term row as HTML.
pub(super) fn yomitan_term_html(row: &[Value], version: u64, tags: &HashMap<String, String>) -> Result<String> {
    ensure!(row.len() >= if version == 1 { 5 } else { 8 }, "Termo Yomitan incompleto");
    let (word, reading) = yomitan_term_keys(row)?;
    for i in [2, 3, 7] { ensure!(value_str(row.get(i)).unwrap_or("").len() <= MAX_HEADWORD, "Tags Yomitan excedem 16 KiB"); }
    let mut html = format!("<div class=\"yomitan-entry\"><div class=\"yomitan-headword\"><b>{}</b>", escape_html(&word));
    if !reading.is_empty() && reading != word { html.push_str(&format!(" <span class=\"yomitan-reading\">【{}】</span>", escape_html(&reading))); }
    html.push_str("</div>");
    html.push_str(&tag_html(value_str(row.get(2)).unwrap_or(""), tags));
    if version > 1 { html.push_str(&tag_html(value_str(row.get(7)).unwrap_or(""), tags)); }
    if let Some(rules) = value_str(row.get(3)).filter(|s| !s.is_empty()) { html.push_str(&format!("<span class=\"yomitan-rules\">{}</span>", escape_html(rules))); }
    html.push_str("<ol class=\"yomitan-glossary\">");
    let glosses = if version == 1 { &row[5..] } else { row[5].as_array().ok_or_else(|| anyhow!("Glossário Yomitan inválido"))?.as_slice() };
    for gloss in glosses {
        html.push_str("<li>");
        render_yomitan(gloss, &mut html, 0)?;
        html.push_str("</li>");
        ensure!(html.len() <= MAX_ARTICLE, "Verbete Yomitan excede 16 MiB");
    }
    html.push_str("</ol></div>");
    Ok(html)
}

/// One kanji row as HTML.
pub(super) fn yomitan_kanji_html(row: &[Value], version: u64, tags: &HashMap<String, String>) -> Result<String> {
    ensure!(row.len() >= if version == 1 { 4 } else { 6 }, "Kanji Yomitan incompleto");
    let word = value_str(row.first()).ok_or_else(|| anyhow!("Kanji Yomitan sem caractere"))?;
    ensure!(word.len() <= MAX_HEADWORD, "Kanji Yomitan muito longo");
    for i in 1..=3 { ensure!(value_str(row.get(i)).unwrap_or("").len() <= MAX_HEADWORD, "Leitura/tag kanji muito longa"); }
    let mut html = format!("<div class=\"yomitan-kanji\"><b>{}</b>", escape_html(word));
    for (i, label) in [(1, "On"), (2, "Kun")] {
        if let Some(readings) = value_str(row.get(i)).filter(|s| !s.is_empty()) { html.push_str(&format!("<div class=\"yomitan-reading\">{label}: {}</div>", escape_html(readings))); }
    }
    html.push_str(&tag_html(value_str(row.get(3)).unwrap_or(""), tags));
    html.push_str("<ul>");
    let meanings = if version == 1 { &row[4..] } else { row[4].as_array().ok_or_else(|| anyhow!("Significados kanji inválidos"))?.as_slice() };
    for meaning in meanings { html.push_str("<li>"); render_yomitan(meaning, &mut html, 0)?; html.push_str("</li>"); }
    html.push_str("</ul>");
    if version > 1 {
        if let Some(stats) = row[5].as_object() {
            html.push_str("<dl class=\"yomitan-stats\">");
            for (k, v) in stats {
                let value = v.as_str().map(str::to_string).unwrap_or_else(|| if v.is_number() { v.to_string() } else { String::new() });
                html.push_str(&format!("<dt>{}</dt><dd>{}</dd>", escape_html(k), escape_html(&value)));
            }
            html.push_str("</dl>");
        }
    }
    html.push_str("</div>");
    Ok(html)
}

/// The word, reading and HTML of a term_meta_bank or kanji_meta_bank row (frequency, pitch
/// accent, IPA), which Lookupper shows as Frequency and Pitch accent articles. `None` for modes
/// it does not show.
pub(super) fn yomitan_meta_html(row: &[Value], kanji: bool) -> Result<Option<(String, String, String)>> {
    let (Some(word), Some(mode), Some(data)) = (value_str(row.first()), value_str(row.get(1)), row.get(2)) else {
        bail!("Metadado Yomitan incompleto");
    };
    ensure!(word.len() <= MAX_HEADWORD, "Palavra Yomitan excede 16 KiB");
    let Some((reading, body)) = yomitan_meta(mode, data, kanji) else { return Ok(None) };
    let mut html = format!("<div class=\"yomitan-meta\"><div class=\"yomitan-headword\"><b>{}</b>", escape_html(word));
    if !reading.is_empty() && reading != word { html.push_str(&format!(" <span class=\"yomitan-reading\">【{}】</span>", escape_html(&reading))); }
    html.push_str("</div>");
    html.push_str(&body);
    html.push_str("</div>");
    ensure!(html.len() <= MAX_ARTICLE, "Verbete Yomitan excede 16 MiB");
    Ok(Some((word.to_string(), reading, html)))
}

/// The reading and HTML of a term_meta_bank or kanji_meta_bank row; `None` for unknown modes.
fn yomitan_meta(mode: &str, data: &Value, kanji: bool) -> Option<(String, String)> {
    let reading = if kanji { "" } else { data.get("reading").and_then(Value::as_str).unwrap_or("") };
    let html = match mode {
        "freq" => {
            let value = if kanji || data.get("reading").is_none() { data } else { data.get("frequency")? };
            format!("<div class=\"yomitan-frequency\"><span class=\"yomitan-tag\">Frequência</span> {}</div>", escape_html(&frequency_text(value)?))
        }
        "pitch" if !kanji => {
            let mut html = String::new();
            for pitch in data.get("pitches")?.as_array()?.iter().take(MAX_VARIANTS) {
                let accent = pitch_html(reading, pitch.get("position")?)?;
                html.push_str(&format!("<div class=\"yomitan-pitch\"><span class=\"yomitan-tag\">Acento</span> {accent}"));
                if let Some(tags) = pitch.get("tags").and_then(Value::as_array) {
                    for tag in tags.iter().filter_map(Value::as_str) { html.push_str(&format!(" <span class=\"yomitan-tag\">{}</span>", escape_html(tag))); }
                }
                html.push_str("</div>");
            }
            html
        }
        "ipa" if !kanji => {
            let mut html = String::new();
            for item in data.get("transcriptions")?.as_array()?.iter().take(MAX_VARIANTS) {
                let ipa = item.get("ipa").and_then(Value::as_str)?;
                html.push_str(&format!("<div class=\"yomitan-ipa\"><span class=\"yomitan-tag\">IPA</span> {}</div>", escape_html(ipa)));
            }
            html
        }
        _ => return None,
    };
    (!html.is_empty()).then(|| (reading.to_string(), html))
}

/// A frequency as Yomitan shows it: `displayValue` when given, otherwise the value.
fn frequency_text(value: &Value) -> Option<String> {
    match value {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("displayValue").and_then(Value::as_str).map(str::to_string)
            .or_else(|| o.get("value").and_then(frequency_text)),
        _ => None,
    }
}

/// Splits kana into morae: a small kana joins the one before it.
fn morae(reading: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in reading.chars() {
        if "ゃゅょぁぃぅぇぉゎャュョァィゥェォヮ".contains(c) && !out.is_empty() {
            out.last_mut().unwrap().push(c);
        } else {
            out.push(c.to_string());
        }
    }
    out
}

/// The reading drawn with a line over its high morae and a mark at the downstep, followed by the
/// downstep number. `position` is the downstep mora (0 = heiban) or a pattern of H and L.
fn pitch_html(reading: &str, position: &Value) -> Option<String> {
    let morae = morae(reading);
    let high: Vec<bool> = if let Some(n) = position.as_u64() {
        let n = n as usize;
        (0..morae.len()).map(|i| if n == 0 { i > 0 } else if n == 1 { i == 0 } else { i > 0 && i < n }).collect()
    } else {
        position.as_str()?.chars().take(morae.len()).map(|c| c == 'H' || c == 'h').collect()
    };
    let mut html = String::from("<span class=\"yomitan-pitch-reading\">");
    for (i, mora) in morae.iter().enumerate() {
        let up = high.get(i).copied().unwrap_or(false);
        let drop = up && !high.get(i + 1).copied().unwrap_or(position.as_u64() == Some(0));
        let mut style = String::new();
        if up { style.push_str("border-top:1px solid currentColor;"); }
        if drop { style.push_str("border-right:1px solid currentColor;"); }
        if style.is_empty() { html.push_str(&escape_html(mora)); }
        else { html.push_str(&format!("<span style=\"{style}\">{}</span>", escape_html(mora))); }
    }
    html.push_str("</span>");
    if let Some(n) = position.as_u64() { html.push_str(&format!(" [{n}]")); }
    Some(html)
}

fn value_str(value: Option<&Value>) -> Option<&str> { value.and_then(Value::as_str) }

fn yomitan_tags(dir: &Path, index: &Value) -> Result<HashMap<String, String>> {
    let mut tags = HashMap::new();
    let mut memory = 0;
    if let Some(meta) = index.get("tagMeta").and_then(Value::as_object) {
        for (name, data) in meta { tags.insert(name.clone(), data.get("notes").and_then(Value::as_str).unwrap_or("").to_string()); }
    }
    for path in banks(dir, "tag_bank_")? {
        json_rows(&path, |row| {
            if let Some(row) = row.as_array() {
                if let Some(name) = value_str(row.first()) {
                    let notes = value_str(row.get(3)).unwrap_or("");
                    memory += name.len() + notes.len() + 96;
                    ensure!(memory <= MAX_ALIAS_MEMORY, "Tags Yomitan excedem o limite de 32 MiB");
                    tags.insert(name.to_string(), notes.to_string());
                }
            }
            Ok(())
        })?;
    }
    Ok(tags)
}

fn tag_html(names: &str, tags: &HashMap<String, String>) -> String {
    names.split_whitespace().map(|name| format!("<span class=\"yomitan-tag\" title=\"{}\">{}</span> ", escape_html(tags.get(name).map(String::as_str).unwrap_or("")), escape_html(name))).collect::<String>()
}

fn render_yomitan(value: &Value, html: &mut String, depth: usize) -> Result<()> {
    ensure!(depth <= MAX_DEPTH, "Conteúdo Yomitan aninhado além de 64 níveis");
    ensure!(html.len() <= MAX_ARTICLE, "Verbete Yomitan excede 16 MiB");
    match value {
        Value::String(s) => append_text(html, s)?,
        Value::Array(a) => for v in a { render_yomitan(v, html, depth + 1)?; },
        Value::Object(obj) => {
            let kind = obj.get("type").and_then(Value::as_str).unwrap_or("");
            if kind == "text" { if let Some(s) = obj.get("text") { render_yomitan(s, html, depth + 1)?; } return Ok(()); }
            if kind == "structured-content" { if let Some(s) = obj.get("content") { render_yomitan(s, html, depth + 1)?; } return Ok(()); }
            let tag = if kind == "image" { "img" } else { obj.get("tag").and_then(Value::as_str).unwrap_or("span") };
            if tag == "img" { html.push_str(&yomitan_image(value)); return Ok(()); }
            let tag = match tag { "br" | "ruby" | "rt" | "rp" | "table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th" | "span" | "div" | "ol" | "ul" | "li" | "details" | "summary" | "a" => tag, _ => "span" };
            html.push('<'); html.push_str(tag);
            for key in ["title", "lang"] {
                if let Some(v) = obj.get(key).and_then(Value::as_str).filter(|s| s.len() <= MAX_HEADWORD) { html.push_str(&format!(" {key}=\"{}\"", escape_html(v))); }
            }
            if tag == "a" {
                if let Some(href) = obj.get("href").and_then(Value::as_str).and_then(yomitan_href) { html.push_str(&format!(" href=\"{}\"", escape_html(&href))); }
            }
            if tag == "details" && obj.get("open").and_then(Value::as_bool) == Some(true) { html.push_str(" open"); }
            for (json, attr) in [("colSpan", "colspan"), ("rowSpan", "rowspan")] {
                if let Some(v) = obj.get(json).and_then(Value::as_u64).filter(|n| *n > 0 && *n <= 1000) { html.push_str(&format!(" {attr}=\"{v}\"")); }
            }
            if let Some(data) = obj.get("data").and_then(Value::as_object) {
                for (key, v) in data {
                    if key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !key.is_empty() {
                        if let Some(v) = v.as_str().filter(|s| s.len() <= MAX_HEADWORD) { html.push_str(&format!(" {}=\"{}\"", data_attribute(key), escape_html(v))); }
                    }
                }
            }
            if let Some(style) = obj.get("style").and_then(Value::as_object) {
                let mut css = String::new();
                for (key, v) in style {
                    let allowed = matches!(key.as_str(), "fontStyle" | "fontWeight" | "fontSize" | "color" | "background" | "backgroundColor" | "textDecorationLine" | "textDecorationStyle" | "textDecorationColor" | "borderColor" | "borderStyle" | "borderRadius" | "borderWidth" | "clipPath" | "verticalAlign" | "textAlign" | "textEmphasis" | "textShadow" | "margin" | "marginTop" | "marginLeft" | "marginRight" | "marginBottom" | "padding" | "paddingTop" | "paddingLeft" | "paddingRight" | "paddingBottom" | "wordBreak" | "whiteSpace" | "cursor" | "listStyleType");
                    if !allowed { continue; }
                    let value = if let Some(s) = v.as_str() { s.to_string() } else if let Some(a) = v.as_array() { a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ") } else if v.is_number() { if key.starts_with("margin") || key.starts_with("padding") { format!("{v}em") } else { v.to_string() } } else { continue };
                    if safe_css_value(&value) {
                        let kebab: String = key.chars().flat_map(|c| if c.is_ascii_uppercase() { vec!['-', c.to_ascii_lowercase()] } else { vec![c] }).collect();
                        css.push_str(&format!("{kebab}:{value};"));
                    }
                }
                if !css.is_empty() { html.push_str(&format!(" style=\"{}\"", escape_html(&css))); }
            }
            html.push('>');
            if let Some(content) = obj.get("content") { render_yomitan(content, html, depth + 1)?; }
            if tag != "br" { html.push_str("</"); html.push_str(tag); html.push('>'); }
        }
        Value::Null => {}
        _ => html.push_str(&escape_html(&value.to_string())),
    }
    ensure!(html.len() <= MAX_ARTICLE, "Verbete Yomitan excede 16 MiB");
    Ok(())
}

/// The attribute Yomitan gives a structured-content `data` key: `dataset["sc" + Key]`, so
/// `content` becomes `data-sc-content` and `wordClass` becomes `data-sc-word-class`.
fn data_attribute(key: &str) -> String {
    let mut name = String::from("data-sc-");
    for (i, c) in key.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 { name.push('-'); }
        name.push(c.to_ascii_lowercase());
    }
    name
}

fn percent_decode(value: &str) -> String {
    percent_decode_url(value, true)
}

fn percent_decode_url(value: &str, plus_space: bool) -> String {
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(a), Some(b)) = ((bytes[i + 1] as char).to_digit(16), (bytes[i + 2] as char).to_digit(16)) { out.push((a * 16 + b) as u8); i += 3; continue; }
        }
        out.push(if plus_space && bytes[i] == b'+' { b' ' } else { bytes[i] }); i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn yomitan_href(value: &str) -> Option<String> {
    if value.len() > MAX_HEADWORD { return None; }
    if let Some(query) = value.strip_prefix('?') {
        let word = query.split('&').filter_map(|s| s.split_once('=')).find(|(k, _)| *k == "query").map(|(_, v)| percent_decode(v))?;
        return Some(entry_link(&word));
    }
    (value.starts_with("https://") || value.starts_with("http://")).then(|| value.to_string())
}

fn yomitan_image(value: &Value) -> String {
    let Some(path) = value.get("path").and_then(Value::as_str).and_then(resource_path) else { return String::new() };
    let mut html = format!("<img loading=\"lazy\" src=\"{}\"", escape_html(&path));
    for attr in ["title", "alt"] {
        if let Some(s) = value.get(attr).and_then(Value::as_str).filter(|s| s.len() <= MAX_HEADWORD) { html.push_str(&format!(" {attr}=\"{}\"", escape_html(s))); }
    }
    if let Some(data) = value.get("data").and_then(Value::as_object) {
        for (key, val) in data {
            if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                if let Some(val) = val.as_str().filter(|s| s.len() <= MAX_HEADWORD) { html.push_str(&format!(" {}=\"{}\"", data_attribute(key), escape_html(val))); }
            }
        }
    }
    let mut css = "max-width:100%;object-fit:contain;".to_string();
    let unit = if value.get("sizeUnits").and_then(Value::as_str) == Some("em") { "em" } else { "px" };
    for name in ["width", "height"] {
        if let Some(n) = value.get(name).and_then(Value::as_f64).filter(|n| n.is_finite() && *n >= 0.0 && *n <= 100_000.0) { css.push_str(&format!("{name}:{n}{unit};")); }
    }
    if let Some(mode) = value.get("imageRendering").and_then(Value::as_str).filter(|s| matches!(*s, "auto" | "pixelated" | "crisp-edges")) { css.push_str(&format!("image-rendering:{mode};")); }
    else if value.get("pixelated").and_then(Value::as_bool) == Some(true) { css.push_str("image-rendering:pixelated;"); }
    for (key, prop) in [("verticalAlign", "vertical-align"), ("border", "border"), ("borderRadius", "border-radius")] {
        if let Some(s) = value.get(key).and_then(Value::as_str).filter(|s| safe_css_value(s)) { css.push_str(&format!("{prop}:{s};")); }
    }
    html.push_str(&format!(" style=\"{}\">", escape_html(&css)));
    if let Some(description) = value.get("description").and_then(Value::as_str).filter(|s| s.len() <= MAX_HEADWORD) { html = format!("<figure>{html}<figcaption>{}</figcaption></figure>", escape_html(description)); }
    if value.get("collapsible").and_then(Value::as_bool) == Some(true) || value.get("collapsed").and_then(Value::as_bool) == Some(true) {
        let open = if value.get("collapsed").and_then(Value::as_bool) == Some(true) { "" } else { " open" };
        html = format!("<details class=\"yomitan-image\"{open}><summary>▧</summary>{html}</details>");
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsl_comments_do_not_eat_escaped_braces() {
        let mut inside = false;
        assert_eq!(without_dsl_comments("a{{hidden", &mut inside), "a");
        assert!(inside);
        assert_eq!(without_dsl_comments("hidden}}b\\{c\\}", &mut inside), "b\\{c\\}");
        assert!(!inside);
        assert_eq!(dsl_headwords("read(ing)"), vec!["read", "reading"]);
    }

    #[test]
    fn stardict_articles_need_their_terminators() {
        assert!(stardict_article(b"mno terminating nul", "").is_err());
        assert!(stardict_article(b"a\0b", "tm").is_ok());
    }

    #[test]
    fn html_fields_keep_formatting_and_remove_active_content() {
        let html = safe_markup("<b>Safe &amp; clear</b><script>bad()</script><img src=\"pic.png\" onerror=\"bad()\"><a href=\"javascript:bad()\">x</a>").unwrap();
        assert!(html.contains("<b>Safe &amp; clear</b>"));
        assert!(html.contains("src=\"pic.png\""));
        assert!(!html.contains("script"));
        assert!(!html.contains("bad()"));
        assert!(!html.contains("onerror"));
    }

    #[test]
    fn html_attribute_entities_are_escaped_once_and_bword_targets_decode_once() {
        let html = safe_markup(r#"<img src="a&amp;b.png" title="x &#38; y"><img src="a&#x26;b.png"><a href="https://example.test/?a=1&amp;b=2">external</a><a href="bword://foo%20bar">word</a><a href="bword://C++">plus</a><a href="bword://literal%2520">percent</a><a href="&#x6a;avascript:bad()">unsafe</a>"#).unwrap();
        assert_eq!(html.matches("src=\"a&amp;b.png\"").count(), 2);
        assert!(html.contains("title=\"x &amp; y\""));
        assert!(html.contains("href=\"https://example.test/?a=1&amp;b=2\""));
        assert!(!html.contains("&amp;amp;"));
        assert!(html.contains(&format!("href=\"{}\"", entry_link("foo bar"))));
        assert!(html.contains(&format!("href=\"{}\"", entry_link("C++"))));
        assert!(html.contains(&format!("href=\"{}\"", entry_link("literal%20"))));
        assert!(!html.contains("javascript:"));
    }

    #[test]
    fn yomitan_helpers_and_resource_paths() {
        assert_eq!(data_attribute("wordClass"), "data-sc-word-class");
        assert_eq!(morae("きょうと"), vec!["きょ", "う", "と"]);
        assert!(resource_path("../outside.png").is_none());
        assert!(resource_path("C:\\secret.png").is_none());
        assert!(yomitan_href("javascript:alert(1)").is_none());
        assert!(!safe_css_value("url(https://example.test)"));
    }
}
