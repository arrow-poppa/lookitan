//! Dictionaries read in place, as Lookupper reads them: the files stay as they are, and a
//! `<main file>.lkidx` beside them records where every article is. A lookup reads only that
//! article and renders it.
//!
//! - DSL and StarDict (`.dz` files stay compressed: dictzip.rs reads one chunk).
//! - CC-CEDICT and Kaikki JSONL (one article per line), WordNet data files (one per synset line).
//! - Aard2/Slob (one per reference; resources such as images are indexed too, under `HIDDEN`).
//! - Lookupper's own choices of format: jmdict-simplified and kanjidic2 JSON (jsondict.rs),
//!   WN-LMF XML (wnlmf.rs), Gutenberg's Webster text (webster_text.rs) and GCIDE's CIDE.* files
//!   (cide.rs), recognized the way Lookupper recognizes them.
//! - XML: JMdict, JMnedict, KANJIDIC2, Webster and GCIDE_XML (one per entry element). The index
//!   also keeps the file's prologue (declaration, DOCTYPE, root start tag), so an entry is parsed
//!   again with the entities its DTD declares.
//!
//! Articles are rendered by running the format's reader again over the article's bytes; the index
//! remembers which of the entries read from those bytes a headword names (a DSL article and its
//! `@` subentries share their bytes). Files compressed with plain gzip or bzip2, which cannot be
//! read at an offset, are unpacked once into the app's cache.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{anyhow, ensure, Context, Result};

use super::containers::{default_name, dsl_entries, stardict_article, stardict_files, stardict_index, stardict_info, DslLines};
use super::dictzip;
use super::lexicons::{self, dictionary_name, filename, gcide_parts, import_cedict, import_kaikki, import_xml, wordnet_exceptions,
    wordnet_files, wordnet_line, GCIDE_END};
use super::lkidx::{self, Builder, Index, Location, Stamp, HIDDEN};
use super::{cide, jsondict, slob, webster_text, wnlmf};
use super::ImportSink;

pub const DSL: &str = "dsl";
pub const STARDICT: &str = "stardict";
pub const CEDICT: &str = "cedict";
pub const KAIKKI: &str = "kaikki";
pub const WORDNET: &str = "wordnet";
pub const SLOB: &str = "slob";
/// The index format of every XML dictionary; `InPlace::format` names the kind (jmdict, …).
const XML: &str = "xml";
const MAX_ARTICLE: u64 = 16 * 1024 * 1024;
/// `Location::kind` of the XML prologue entry, which no headword names.
const PROLOG: u16 = u16::MAX;
/// `Location::kind` of a Slob resource; its offset is the resource's reference number.
const RESOURCE: u16 = u16::MAX - 1;

fn ends_with_any(name: &str, suffixes: &[&str]) -> bool {
    suffixes.iter().any(|s| name.ends_with(s))
}

/// The in-place format of a main file, if any.
pub fn format_of(path: &Path) -> Option<&'static str> {
    let name = filename(path);
    if cide::detect(path) {
        Some(cide::FORMAT)
    } else if name.ends_with(".json") && jsondict::detect(path).is_some() {
        jsondict::detect(path)
    } else if name.ends_with(".xml") && wnlmf::detect(path) {
        Some(wnlmf::FORMAT)
    } else if name.ends_with(".txt") && webster_text::detect(path) {
        Some(webster_text::FORMAT)
    } else if ends_with_any(&name, &[".dsl", ".dsl.dz"]) {
        Some(DSL)
    } else if name.ends_with(".ifo") {
        Some(STARDICT)
    } else if name.ends_with(".slob") {
        Some(SLOB)
    } else if path.is_dir() || ["data.noun", "data.verb", "data.adj", "data.adv", "index.noun", "index.verb", "index.adj", "index.adv"].contains(&name.as_str()) {
        lexicons::supports(path).then_some(WORDNET)
    } else if ends_with_any(&name, &[".jsonl", ".jsonl.gz", ".jsonl.bz2", ".json"]) {
        Some(KAIKKI)
    } else if ends_with_any(&name, &[".xml", ".xml.gz", ".xml.bz2"]) {
        Some(XML)
    } else if ends_with_any(&name, &[".txt", ".u8", ".txt.gz", ".u8.gz"]) {
        Some(CEDICT)
    } else {
        None
    }
}

enum Kind {
    Dsl { decoding: (Option<bool>, &'static encoding_rs::Encoding) },
    StarDict { sequence: String },
    Cedict,
    Kaikki,
    /// The part of speech of each data file, by file number.
    WordNet { parts: Vec<&'static str> },
    Xml,
    /// Articles and resources are read from the .slob by reference number.
    Slob { path: PathBuf },
    JmdictJson { tags: HashMap<String, String> },
    KanjidicJson,
    WnLmf,
    WebsterText,
    Cide,
}

/// Where articles are read from.
enum Data {
    /// One file, compressed with dictzip or not (DSL, StarDict).
    Zip(dictzip::Source),
    /// Uncompressed files, by file number.
    Files(Vec<PathBuf>),
    /// GCIDE_XML: a prologue made from the root's DOCTYPE, then the 26 letter files.
    Joined { prefix: Vec<u8>, files: Vec<(PathBuf, u64)> },
}

impl Data {
    fn read(&self, location: Location) -> Result<Vec<u8>> {
        let (offset, length) = (location.offset, location.length as usize);
        match self {
            Data::Zip(source) => source.read_at(offset, length),
            Data::Files(files) => {
                let path = files.get(location.file as usize).ok_or_else(|| anyhow!("dictionary entry names an unknown file"))?;
                let mut file = File::open(path)?;
                file.seek(SeekFrom::Start(offset))?;
                let mut out = vec![0u8; length];
                file.read_exact(&mut out).context("dictionary entry lies past the end of its file")?;
                Ok(out)
            }
            Data::Joined { prefix, files } => {
                let mut out = Vec::with_capacity(length);
                let mut at = offset;
                let end = offset + length as u64;
                if at < prefix.len() as u64 {
                    let stop = end.min(prefix.len() as u64);
                    out.extend_from_slice(&prefix[at as usize..stop as usize]);
                    at = stop;
                }
                let mut base = prefix.len() as u64;
                for (path, size) in files {
                    if at >= end { break; }
                    if at < base + size {
                        let stop = end.min(base + size);
                        let mut file = File::open(path)?;
                        file.seek(SeekFrom::Start(at - base))?;
                        let mut piece = vec![0u8; (stop - at) as usize];
                        file.read_exact(&mut piece)?;
                        out.extend_from_slice(&piece);
                        at = stop;
                    }
                    base += size;
                }
                ensure!(out.len() == length, "dictionary entry lies past the end of its files");
                Ok(out)
            }
        }
    }
}

pub struct InPlace {
    index: Index,
    kind: Kind,
    data: Data,
    /// The entry holding an XML file's prologue.
    prolog: Option<u32>,
    pub format: &'static str,
}

/// Records each entry's article span instead of its HTML.
struct Spans<'a> {
    builder: &'a mut Builder,
    span: (u16, u64, u64),
    ordinal: u16,
    prolog: Option<u32>,
    progress: &'a dyn Fn(usize),
}

impl<'a> Spans<'a> {
    fn new(builder: &'a mut Builder, progress: &'a dyn Fn(usize)) -> Self {
        Self { builder, span: (0, 0, 0), ordinal: 0, prolog: None, progress }
    }
}

impl ImportSink for Spans<'_> {
    fn span(&mut self, file: u16, offset: u64, length: u64) {
        self.span = (file, offset, length);
        self.ordinal = 0;
    }

    fn add_resource(&mut self, key: &str, reference: u32) -> Result<()> {
        let location = Location { file: 0, kind: RESOURCE, offset: u64::from(reference), length: 0 };
        self.builder.add(&format!("{HIDDEN}{}", key.replace('\\', "/").trim_start_matches('/')), location, &[])?;
        Ok(())
    }

    fn prolog(&mut self, length: u64) {
        if self.prolog.is_none() && length <= MAX_ARTICLE {
            let location = Location { file: 0, kind: PROLOG, offset: 0, length: length as u32 };
            self.prolog = self.builder.add("", location, &[]).ok();
        }
    }

    fn add(&mut self, headword: &str, _html: &str, aliases: &[String]) -> Result<()> {
        let (file, offset, length) = self.span;
        ensure!(length <= MAX_ARTICLE, "Verbete excede 16 MiB");
        let location = Location { file, kind: self.ordinal, offset, length: length as u32 };
        let aliases: Vec<&str> = aliases.iter().map(String::as_str).collect();
        self.builder.add(headword, location, &aliases)?;
        self.ordinal = self.ordinal.saturating_add(1);
        if self.builder.len() % 1000 == 0 { (self.progress)(self.builder.len()); }
        Ok(())
    }
}

/// Collects the HTML of the entries read from one article's bytes.
#[derive(Default)]
struct Html(Vec<String>);

impl ImportSink for Html {
    fn add(&mut self, _headword: &str, html: &str, _aliases: &[String]) -> Result<()> {
        self.0.push(html.to_string());
        Ok(())
    }
}

fn reader(bytes: Vec<u8>) -> Box<dyn std::io::BufRead> {
    Box::new(Cursor::new(bytes))
}

fn name_of(path: &Path) -> PathBuf {
    PathBuf::from(path.file_name().unwrap_or_default())
}

/// A file compressed with gzip or bzip2 as an uncompressed copy in `cache` (made once, and again
/// when the original changes); any other file as itself.
fn plain_copy(path: &Path, cache: &Path, id: &str) -> Result<PathBuf> {
    let name = filename(path);
    let Some(stem) = name.strip_suffix(".gz").or_else(|| name.strip_suffix(".bz2")) else { return Ok(path.to_path_buf()) };
    let meta = fs::metadata(path)?;
    let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    let stamp = format!("{}\t{}", meta.len(), modified);
    let dir = cache.join("unpacked");
    fs::create_dir_all(&dir)?;
    let target = dir.join(format!("{id}.{stem}"));
    let stamp_path = dir.join(format!("{id}.{stem}.source"));
    if target.is_file() && fs::read_to_string(&stamp_path).ok().as_deref() == Some(stamp.as_str()) {
        return Ok(target);
    }
    let tmp = dir.join(format!("{id}.{stem}.tmp"));
    let mut out = File::create(&tmp)?;
    std::io::copy(&mut lexicons::open(path)?, &mut out).with_context(|| format!("unpack {}", path.display()))?;
    out.sync_all()?;
    drop(out);
    let _ = fs::remove_file(&target);
    fs::rename(&tmp, &target)?;
    fs::write(&stamp_path, stamp)?;
    Ok(target)
}

/// The root element of an XML file, from its first kilobytes.
fn xml_kind(path: &Path) -> Result<&'static str> {
    let mut head = vec![0u8; 256 * 1024];
    let n = File::open(path)?.read(&mut head)?;
    let text = String::from_utf8_lossy(&head[..n]);
    let mut rest = text.as_ref();
    while let Some(at) = rest.find('<') {
        rest = &rest[at + 1..];
        if rest.starts_with('?') || rest.starts_with('!') {
            // A DOCTYPE's internal subset holds declarations that also start with '<'.
            if rest.starts_with("!DOCTYPE") {
                if let Some(end) = rest.find("]>").or_else(|| rest.find('>')) { rest = &rest[end..]; }
            }
            continue;
        }
        let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':').collect();
        return Ok(match name.to_ascii_lowercase().as_str() {
            "jmdict" => "jmdict",
            "jmnedict" => "jmnedict",
            "kanjidic2" => "kanjidic2",
            _ => "webster",
        });
    }
    Ok("webster")
}

/// The closing tag of the root element a prologue opens.
fn closing_tag(prolog: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(prolog);
    let name: String = text.rfind('<').map(|at| text[at + 1..].chars().take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/').collect()).unwrap_or_default();
    format!("</{name}>").into_bytes()
}

impl InPlace {
    /// Opens `main`, building its index at `index_path` when it is missing or stale. `cache` holds
    /// uncompressed copies of gzip/bzip2 files.
    pub fn open(main: &Path, index_path: &Path, cache: &Path, id: &str, progress: &dyn Fn(usize)) -> Result<Self> {
        let format = format_of(main).ok_or_else(|| anyhow!("'{}' is in no format this engine reads.", main.display()))?;
        let root = main.parent().map(Path::to_path_buf).unwrap_or_default();
        match format {
            DSL => {
                let decoding = DslLines::new(main)?.decoding();
                let stamp = Stamp::of(&root, &[name_of(main)])?;
                let index = lkidx::open_or_build(index_path, DSL, &stamp, || {
                    let mut builder = Builder::new();
                    let info = dsl_entries(DslLines::new(main)?, default_name(main), &mut Spans::new(&mut builder, progress))?;
                    Ok((builder, info.name, info.description))
                })?;
                Ok(Self { index, kind: Kind::Dsl { decoding }, data: Data::Zip(dictzip::Source::open(main)?), prolog: None, format })
            }
            STARDICT => {
                let info = stardict_info(main)?;
                let (idx, dict, syn) = stardict_files(main)?;
                // The .dict is file 0, as the index's locations say.
                let mut files = vec![name_of(&dict), name_of(main), name_of(&idx)];
                files.extend(syn.as_deref().map(name_of));
                let stamp = Stamp::of(&root, &files)?;
                let index = lkidx::open_or_build(index_path, STARDICT, &stamp, || {
                    let data_len = data_length(&dict)?;
                    let mut builder = Builder::new();
                    stardict_index(main, &info, data_len, |word, offset, size, aliases| {
                        let aliases: Vec<&str> = aliases.iter().map(String::as_str).collect();
                        builder.add(word, Location { file: 0, kind: 0, offset, length: size }, &aliases)?;
                        if builder.len() % 1000 == 0 { progress(builder.len()); }
                        Ok(())
                    })?;
                    Ok((builder, info.name.clone(), info.description.clone()))
                })?;
                Ok(Self { index, kind: Kind::StarDict { sequence: info.sequence }, data: Data::Zip(dictzip::Source::open(&dict)?), prolog: None, format })
            }
            cide::FORMAT => {
                let files = cide::files(main);
                let names: Vec<PathBuf> = files.iter().map(|p| name_of(p)).collect();
                let stamp = Stamp::of(&root, &names)?;
                let index = lkidx::open_or_build(index_path, cide::FORMAT, &stamp, || cide::build(&files, progress))?;
                Ok(Self { index, kind: Kind::Cide, data: Data::Files(files), prolog: None, format })
            }
            jsondict::JMDICT | jsondict::KANJIDIC | wnlmf::FORMAT | webster_text::FORMAT => {
                let stamp = Stamp::of(&root, &[name_of(main)])?;
                let index = lkidx::open_or_build(index_path, format, &stamp, || match format {
                    wnlmf::FORMAT => wnlmf::build(main, progress),
                    webster_text::FORMAT => webster_text::build(main, progress),
                    _ => jsondict::build(main, format, progress),
                })?;
                let data = Data::Files(vec![main.to_path_buf()]);
                let kind = match format {
                    wnlmf::FORMAT => Kind::WnLmf,
                    webster_text::FORMAT => Kind::WebsterText,
                    jsondict::KANJIDIC => Kind::KanjidicJson,
                    _ => {
                        // The tags table is a hidden entry, read once.
                        let tags = index.find(&format!("{HIDDEN}tags"), 1).first().and_then(|&id| index.location(id))
                            .and_then(|l| data.read(l).ok()).map(|bytes| jsondict::tags(&bytes)).unwrap_or_default();
                        Kind::JmdictJson { tags }
                    }
                };
                Ok(Self { index, kind, data, prolog: None, format })
            }
            SLOB => {
                let stamp = Stamp::of(&root, &[name_of(main)])?;
                let index = lkidx::open_or_build(index_path, SLOB, &stamp, || {
                    let mut builder = Builder::new();
                    let info = slob::import(main, &mut Spans::new(&mut builder, progress))?;
                    Ok((builder, info.name, info.description))
                })?;
                Ok(Self { index, kind: Kind::Slob { path: main.to_path_buf() }, data: Data::Files(Vec::new()), prolog: None, format })
            }
            WORDNET => {
                let data = wordnet_files(main)?;
                ensure!(!data.is_empty(), "WordNet: não encontrei os arquivos data.noun/data.verb/data.adj/data.adv");
                let dir = data[0].1.parent().map(Path::to_path_buf).unwrap_or_default();
                let mut files: Vec<PathBuf> = data.iter().map(|(_, p)| name_of(p)).collect();
                for (pos, _) in &data {
                    let exc = PathBuf::from(format!("{pos}.exc"));
                    if dir.join(&exc).is_file() { files.push(exc); }
                }
                let stamp = Stamp::of(&dir, &files)?;
                let parts: Vec<&'static str> = data.iter().map(|(pos, _)| *pos).collect();
                let index = lkidx::open_or_build(index_path, WORDNET, &stamp, || {
                    let mut builder = Builder::new();
                    let mut sink = Spans::new(&mut builder, progress);
                    for (file, (pos, path)) in data.iter().enumerate() {
                        let exceptions = wordnet_exceptions(&dir.join(format!("{pos}.exc")))?;
                        let bytes = fs::read(path)?;
                        let mut at = 0u64;
                        for line in bytes.split_inclusive(|b| *b == b'\n') {
                            let start = at;
                            at += line.len() as u64;
                            let text = std::str::from_utf8(line).with_context(|| format!("{} não é UTF-8", path.display()))?;
                            // The distribution's copyright header lines start with spaces.
                            if text.starts_with(char::is_whitespace) || text.trim().is_empty() { continue; }
                            sink.span(file as u16, start, line.len() as u64);
                            wordnet_line(text, pos, &exceptions, &format!("{}:{start}", path.display()), &mut sink)?;
                        }
                    }
                    ensure!(builder.len() > 0, "WordNet: nenhum synset encontrado");
                    Ok((builder, "WordNet".to_string(), "WordNet: definições e sinônimos por synset".to_string()))
                })?;
                let paths = data.into_iter().map(|(_, p)| p).collect();
                Ok(Self { index, kind: Kind::WordNet { parts }, data: Data::Files(paths), prolog: None, format })
            }
            _ => {
                // CC-CEDICT, Kaikki and XML: one file, or GCIDE_XML's root and its letter files.
                if let Some((prefix, fragments)) = gcide_parts(main)? {
                    let dir = main.parent().map(Path::to_path_buf).unwrap_or_default();
                    let mut names = vec![PathBuf::from("gcide.xml")];
                    names.extend(fragments.iter().map(|p| name_of(p)));
                    let stamp = Stamp::of(&dir, &names)?;
                    let index = lkidx::open_or_build(index_path, XML, &stamp, || {
                        let mut builder = Builder::new();
                        import_xml(lexicons::gcide_input(main)?.ok_or_else(|| anyhow!("GCIDE incompleto"))?, &mut Spans::new(&mut builder, progress))?;
                        Ok((builder, dictionary_name(main), "GCIDE: Webster's Revised Unabridged e acréscimos".to_string()))
                    })?;
                    let files = fragments.into_iter().map(|p| { let size = fs::metadata(&p).map(|m| m.len()).unwrap_or(0); (p, size) }).collect();
                    let prolog = prolog_entry(&index);
                    return Ok(Self { index, kind: Kind::Xml, data: Data::Joined { prefix, files }, prolog, format: "webster" });
                }
                let plain = plain_copy(main, cache, id)?;
                let dir = plain.parent().map(Path::to_path_buf).unwrap_or_default();
                let stamp = Stamp::of(&dir, &[name_of(&plain)])?;
                let (kind, index_format, shown, description) = match format {
                    KAIKKI => (Kind::Kaikki, KAIKKI, KAIKKI, "Wiktionary/Kaikki JSONL: definições, formas, exemplos e pronúncias"),
                    CEDICT => (Kind::Cedict, CEDICT, CEDICT, "CC-CEDICT: chinês tradicional/simplificado, pinyin e definições"),
                    _ => (Kind::Xml, XML, xml_kind(&plain)?, "XML por entrada: JMdict, JMnedict, KANJIDIC2 ou Webster/GCIDE"),
                };
                let index = lkidx::open_or_build(index_path, index_format, &stamp, || {
                    let mut builder = Builder::new();
                    let input = lexicons::open(&plain)?;
                    let mut sink = Spans::new(&mut builder, progress);
                    match index_format {
                        KAIKKI => import_kaikki(input, &mut sink)?,
                        CEDICT => import_cedict(input, &mut sink)?,
                        _ => { import_xml(input, &mut sink)?; }
                    }
                    Ok((builder, dictionary_name(main), description.to_string()))
                })?;
                let prolog = prolog_entry(&index);
                Ok(Self { index, kind, data: Data::Files(vec![plain]), prolog, format: shown })
            }
        }
    }

    pub fn title(&self) -> &str {
        &self.index.title
    }

    pub fn description(&self) -> &str {
        &self.index.description
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// A resource a Slob article refers to (an image, a stylesheet), by its key.
    pub fn resource(&self, key: &str) -> Option<Vec<u8>> {
        let Kind::Slob { path } = &self.kind else { return None };
        let key = key.split(['#', '?']).next().unwrap_or(key).replace('\\', "/");
        let id = *self.index.find(&format!("{HIDDEN}{}", key.trim_start_matches('/')), 1).first()?;
        let location = self.index.location(id).filter(|l| l.kind == RESOURCE)?;
        slob::resource_at(path, location.offset as u32)
    }

    pub fn find(&self, word: &str, limit: usize) -> Vec<u32> {
        self.index.find(word, limit)
    }

    pub fn headword(&self, id: u32) -> Option<String> {
        self.index.title(id).map(str::to_string)
    }

    pub fn entry_html(&self, id: u32) -> Result<String> {
        let location = self.index.location(id).ok_or_else(|| anyhow!("no entry {id}"))?;
        ensure!(location.kind != PROLOG && location.kind != RESOURCE, "no entry {id}");
        if let Kind::Slob { path } = &self.kind {
            return slob::article(path, location.offset as u32);
        }
        let bytes = self.data.read(location)?;
        match &self.kind {
            Kind::JmdictJson { tags } => return jsondict::word_html(&bytes, tags),
            Kind::KanjidicJson => return jsondict::character_html(&bytes),
            Kind::WebsterText => return Ok(webster_text::article_html(&bytes)),
            Kind::Cide => return Ok(cide::article_html(&bytes)),
            Kind::WnLmf => {
                let entry = wnlmf::parse_entry(&bytes)?;
                let read = |key: &str| -> Option<Vec<u8>> {
                    let id = *self.index.find(key, 1).first()?;
                    self.data.read(self.index.location(id)?).ok()
                };
                return Ok(wnlmf::entry_html(&entry,
                    |synset| read(&wnlmf::synset_key(synset)).and_then(|b| wnlmf::parse_synset(&b).ok()),
                    |member| self.index.find(&wnlmf::entry_key(member), 1).first().and_then(|&e| self.index.title(e)).map(str::to_string)));
            }
            _ => {}
        }
        let mut html = Html::default();
        match &self.kind {
            Kind::StarDict { sequence } => return stardict_article(&bytes, sequence),
            Kind::Slob { .. } | Kind::JmdictJson { .. } | Kind::KanjidicJson | Kind::WnLmf | Kind::WebsterText | Kind::Cide => unreachable!("read above"),
            Kind::Dsl { decoding } => { dsl_entries(DslLines::over(bytes, *decoding), String::new(), &mut html)?; }
            Kind::Cedict => import_cedict(reader(bytes), &mut html)?,
            Kind::Kaikki => import_kaikki(reader(bytes), &mut html)?,
            Kind::WordNet { parts } => {
                let pos = parts.get(location.file as usize).copied().unwrap_or("noun");
                let text = String::from_utf8(bytes)?;
                wordnet_line(&text, pos, &HashMap::new(), "WordNet", &mut html)?;
            }
            Kind::Xml => {
                let prolog_id = self.prolog.ok_or_else(|| anyhow!("XML index without a prologue"))?;
                let prolog = self.data.read(self.index.location(prolog_id).ok_or_else(|| anyhow!("XML prologue missing"))?)?;
                let close = if matches!(self.data, Data::Joined { .. }) { GCIDE_END.to_vec() } else { closing_tag(&prolog) };
                let mut xml = prolog;
                xml.extend_from_slice(&bytes);
                xml.extend_from_slice(&close);
                import_xml(reader(xml), &mut html)?;
            }
        }
        html.0.into_iter().nth(location.kind as usize).ok_or_else(|| anyhow!("dictionary article changed since it was indexed"))
    }
}

fn prolog_entry(index: &Index) -> Option<u32> {
    // The prologue is added before any entry, so it is entry 0 when there is one.
    index.location(0).filter(|l| l.kind == PROLOG).map(|_| 0)
}

/// The decompressed size of a StarDict .dict(.dz), to check the index's offsets against.
fn data_length(dict: &Path) -> Result<u64> {
    if filename(dict).ends_with(".dz") {
        Ok(std::io::copy(&mut dictzip::Source::stream(dict)?, &mut std::io::sink())?)
    } else {
        Ok(fs::metadata(dict)?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionaries::entry_link;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static ID: AtomicU64 = AtomicU64::new(0);
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("lookitan-inplace-{}-{stamp}-{}", std::process::id(), ID.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    fn gzip(path: &Path, bytes: &[u8]) {
        let mut encoder = GzEncoder::new(File::create(path).unwrap(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap();
    }

    fn open(main: &Path) -> InPlace {
        let cache = main.parent().unwrap().join("cache");
        InPlace::open(main, &PathBuf::from(format!("{}.lkidx", main.display())), &cache, "t", &|_| {}).unwrap()
    }

    fn html(d: &InPlace, word: &str) -> Vec<String> {
        d.find(word, 16).into_iter().map(|id| d.entry_html(id).unwrap()).collect()
    }

    #[test]
    fn dsl_utf16_gzip_aliases_subentries_and_escaped_markup_are_read_in_place() {
        let dir = TestDir::new();
        let path = dir.0.join("demo.dsl.dz");
        let text = "#NAME \"Study DSL\"\ncat(s)\nfeline\n\t[b]A cat[/b] <script> & \\[literal\\]\n\t[ref]dog[/ref] [s]cat.wav[/s]\n\t@kitten\n\t[i]Young cat[/i]\n\t@\ndog\n\tDog\n";
        let mut bytes = vec![0xff, 0xfe];
        for unit in text.encode_utf16() { bytes.extend_from_slice(&unit.to_le_bytes()); }
        gzip(&path, &bytes);
        let d = open(&path);
        assert_eq!((d.title(), d.len()), ("Study DSL", 3));
        for word in ["cat", "cats", "feline"] {
            let found = html(&d, word);
            assert_eq!(found.len(), 1, "{word}");
            let cat = &found[0];
            assert!(cat.contains("<b>A cat</b>") && cat.contains("&lt;script&gt;") && cat.contains("[literal]"));
            assert!(cat.contains(&entry_link("dog")) && cat.contains("src=\"cat.wav\""));
        }
        assert!(html(&d, "kitten")[0].contains("<i>Young cat</i>"), "a subentry is found under its own headword");
        assert!(html(&d, "dog")[0].contains("Dog"));
        assert_eq!(d.headword(d.find("kitten", 1)[0]).as_deref(), Some("kitten"));
    }

    #[test]
    fn plain_dsl_is_reindexed_when_it_changes() {
        let dir = TestDir::new();
        let path = dir.0.join("plain.dsl");
        fs::write(&path, "#NAME \"Plain\"\nrun\nrunning\n\t[b]move fast[/b]\nrun\n\toperate a machine\n").unwrap();
        let d = open(&path);
        let run = html(&d, "ＲＵＮ");
        assert_eq!(run.len(), 2, "homographs are both kept");
        assert!(run[0].contains("<b>move fast</b>") && run[1].contains("operate a machine"));
        assert_eq!(html(&d, "running").len(), 1);
        assert!(dir.0.join("plain.dsl.lkidx").is_file());
        drop(d);
        fs::write(&path, "#NAME \"Plain\"\nwalk\n\tgo on foot\n").unwrap();
        let d = open(&path);
        assert!(d.find("run", 4).is_empty() && html(&d, "walk")[0].contains("go on foot"));
    }

    fn make_stardict(dir: &Path, compressed: bool, offset64: bool) -> PathBuf {
        let ifo = dir.join("demo.ifo");
        let definitions = b"k\xc3\xa6t\0small animaldog\0another animal";
        let second = b"k\xc3\xa6t\0small animal".len();
        let mut index = Vec::new();
        for (word, offset, size) in [("cat", 0, second), ("dog", second, definitions.len() - second)] {
            index.extend_from_slice(word.as_bytes()); index.push(0);
            if offset64 { index.extend_from_slice(&(offset as u64).to_be_bytes()); }
            else { index.extend_from_slice(&(offset as u32).to_be_bytes()); }
            index.extend_from_slice(&(size as u32).to_be_bytes());
        }
        fs::write(&ifo, format!("StarDict's dict ifo file\nversion=3.0.0\nbookname=Study StarDict\nwordcount=2\nsynwordcount=1\nidxoffsetbits={}\nidxfilesize={}\nsametypesequence=tm\n", if offset64 { 64 } else { 32 }, index.len())).unwrap();
        if compressed { gzip(&dir.join("demo.idx.gz"), &index); gzip(&dir.join("demo.dict.dz"), definitions); }
        else { fs::write(dir.join("demo.idx"), index).unwrap(); fs::write(dir.join("demo.dict"), definitions).unwrap(); }
        let mut synonyms = b"feline\0".to_vec(); synonyms.extend_from_slice(&0u32.to_be_bytes());
        fs::write(dir.join("demo.syn"), synonyms).unwrap();
        ifo
    }

    #[test]
    fn stardict_gzip_offsets64_and_synonyms_are_read_in_place() {
        for (compressed, offset64) in [(true, true), (false, false)] {
            let dir = TestDir::new();
            let ifo = make_stardict(&dir.0, compressed, offset64);
            let d = open(&ifo);
            assert_eq!((d.title(), d.len()), ("Study StarDict", 2));
            let cat = html(&d, "feline");
            assert!(cat[0].contains("kæt") && cat[0].contains("small animal"));
            assert!(html(&d, "dog")[0].contains("another animal"));
        }
    }

    #[test]
    fn stardict_rejects_offsets_past_its_data() {
        let dir = TestDir::new();
        let ifo = make_stardict(&dir.0, false, false);
        fs::write(dir.0.join("demo.dict"), b"short").unwrap();
        assert!(InPlace::open(&ifo, &dir.0.join("demo.ifo.lkidx"), &dir.0.join("cache"), "t", &|_| {}).is_err());
    }

    #[test]
    fn cedict_and_gzipped_kaikki_lines_are_read_in_place() {
        let dir = TestDir::new();
        let cedict = dir.0.join("cedict_ts.u8");
        fs::write(&cedict, "# CC-CEDICT\n學習 学习 [xue2 xi2] /to study/<script>/\n行 行 [xing2] /to walk/\n行 行 [hang2] /row/\n").unwrap();
        let d = open(&cedict);
        assert_eq!((d.format, d.len()), ("cedict", 3));
        let study = html(&d, "學習");
        assert!(study[0].contains("to study") && study[0].contains("&lt;script&gt;"));
        let row = html(&d, "行");
        assert!(row.len() == 2 && row[0].contains("to walk") && row[1].contains("row"));

        let kaikki = dir.0.join("words.jsonl.gz");
        gzip(&kaikki, br#"{"word":"cat","lang":"English","pos":"noun","forms":[{"form":"cats","tags":["plural"]}],"senses":[{"glosses":["a feline"]}]}
{"word":"dog","lang":"English","pos":"noun","senses":[{"glosses":["a canine"]}]}
"#);
        let d = open(&kaikki);
        assert_eq!(d.format, "kaikki");
        assert!(html(&d, "cats")[0].contains("a feline"));
        assert!(html(&d, "dog")[0].contains("a canine"));
        assert!(dir.0.join("cache").join("unpacked").join("t.words.jsonl").is_file(), "a gzip file is unpacked once into the cache");
    }

    #[test]
    fn xml_entries_are_parsed_again_with_their_dtd_entities() {
        let dir = TestDir::new();
        let jmdict = dir.0.join("JMdict_e.xml");
        fs::write(&jmdict, r#"<?xml version="1.0"?>
<!DOCTYPE JMdict [<!ENTITY n "noun (common)">]>
<JMdict>
<entry><ent_seq>1</ent_seq><k_ele><keb>猫</keb></k_ele><r_ele><reb>ねこ</reb></r_ele><sense><pos>&n;</pos><gloss>cat &amp; feline</gloss></sense></entry>
<entry><r_ele><reb>あ</reb></r_ele><sense><gloss>Ah</gloss></sense></entry>
</JMdict>"#).unwrap();
        let d = open(&jmdict);
        assert_eq!((d.format, d.len()), ("jmdict", 2));
        let cat = html(&d, "ねこ");
        assert!(cat[0].contains("noun (common)") && cat[0].contains("cat &amp; feline"), "{}", cat[0]);
        assert!(html(&d, "あ")[0].contains("Ah"));
        assert!(d.find("", 4).is_empty(), "the prologue is not an entry");
    }

    #[test]
    fn webster_articles_span_their_paragraphs_and_gcide_spans_its_letter_files() {
        let dir = TestDir::new();
        let webster = dir.0.join("webster.xml");
        fs::write(&webster, "<dictionary><p>Preface</p><p><ent>paronymous</ent><hw>Pa*ron&quot;y*mous</hw><def>Similar in sound.</def></p><p><def>A second sense.</def></p><p><ent>cat</ent><hw>Cat</hw><def>A small animal.</def></p></dictionary>").unwrap();
        let d = open(&webster);
        let word = html(&d, "paronymous");
        assert!(word[0].contains("Similar in sound.") && word[0].contains("A second sense."), "{}", word[0]);
        assert!(html(&d, "cat")[0].contains("A small animal."));

        let gcide = TestDir::new();
        fs::write(gcide.0.join("gcide.xml"), "<!DOCTYPE dictionary [<!ENTITY eacute '&#x00E9;'>]><dictionary></dictionary>").unwrap();
        for letter in b'a'..=b'z' {
            let body = match letter { b'a' => "<p><ent>cafe</ent><hw>Caf&eacute;</hw><def>A coffee house.</def></p>", b'b' => "<p><ent>bee</ent><hw>Bee</hw><def>An insect.</def></p>", _ => "" };
            fs::write(gcide.0.join(format!("gcide_{}.xml", char::from(letter))), body).unwrap();
        }
        let d = open(&gcide.0.join("gcide.xml"));
        assert!(html(&d, "Café")[0].contains("A coffee house."));
        assert!(html(&d, "bee")[0].contains("An insect."), "an entry in a later letter file");
    }

    #[test]
    fn wordnet_synset_lines_are_read_in_place() {
        let dir = TestDir::new();
        fs::write(dir.0.join("data.noun"), "  Copyright header\n00000001 05 n 02 cat 0 feline 0 000 | a small feline\n").unwrap();
        fs::write(dir.0.join("noun.exc"), "kitties cat\n").unwrap();
        fs::write(dir.0.join("data.verb"), "00000002 29 v 01 run 0 000 01 + 01 00 | move quickly\n").unwrap();
        let d = open(&dir.0.join("data.noun"));
        assert_eq!((d.format, d.len()), ("wordnet", 2));
        assert!(html(&d, "kitties")[0].contains("a small feline"));
        assert!(html(&d, "run")[0].contains("verb") && html(&d, "run")[0].contains("move quickly"));
    }
}
