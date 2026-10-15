//! DSL and StarDict dictionaries read in place, as Lookupper reads them: the files stay as they
//! are (`.dz` ones compressed), and `<main file>.lkidx` beside them records where every article
//! is. A lookup reads only that article, through dictzip.rs when the file is compressed.
//!
//! DSL articles are rendered by running the DSL reader again over the article's bytes; the index
//! remembers which of the article's entries (the article itself or one of its `@` subentries) a
//! headword names.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use super::containers::{default_name, dsl_entries, stardict_article, stardict_files, stardict_index, stardict_info, DslLines};
use super::dictzip;
use super::lkidx::{self, Builder, Index, Location, Stamp};
use super::ImportSink;

pub const DSL: &str = "dsl";
pub const STARDICT: &str = "stardict";
const MAX_ARTICLE: u64 = 16 * 1024 * 1024;

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().to_lowercase()
}

/// The in-place format of a main file, if any.
pub fn format_of(path: &Path) -> Option<&'static str> {
    let name = file_name(path);
    if name.ends_with(".dsl") || name.ends_with(".dsl.dz") {
        Some(DSL)
    } else if name.ends_with(".ifo") {
        Some(STARDICT)
    } else {
        None
    }
}

enum Kind {
    Dsl { decoding: (Option<bool>, &'static encoding_rs::Encoding) },
    StarDict { sequence: String },
}

pub struct InPlace {
    index: Index,
    kind: Kind,
    /// The file the articles are read from (the .dsl or the .dict), compressed or not.
    data: dictzip::Source,
    pub format: &'static str,
}

/// Records each entry's article span instead of its HTML.
struct Spans<'a> {
    builder: &'a mut Builder,
    span: (u16, u64, u64),
    ordinal: u16,
    progress: &'a dyn Fn(usize),
}

impl ImportSink for Spans<'_> {
    fn span(&mut self, file: u16, offset: u64, length: u64) {
        self.span = (file, offset, length);
        self.ordinal = 0;
    }

    fn add(&mut self, headword: &str, _html: &str, aliases: &[String]) -> Result<()> {
        let (file, offset, length) = self.span;
        anyhow::ensure!(length <= MAX_ARTICLE, "Verbete DSL excede 16 MiB");
        let location = Location { file, kind: self.ordinal, offset, length: length as u32 };
        let aliases: Vec<&str> = aliases.iter().map(String::as_str).collect();
        self.builder.add(headword, location, &aliases)?;
        self.ordinal = self.ordinal.saturating_add(1);
        if self.builder.len() % 1000 == 0 { (self.progress)(self.builder.len()); }
        Ok(())
    }
}

/// Collects the HTML of the entries of one article.
#[derive(Default)]
struct Html(Vec<String>);

impl ImportSink for Html {
    fn add(&mut self, _headword: &str, html: &str, _aliases: &[String]) -> Result<()> {
        self.0.push(html.to_string());
        Ok(())
    }
}

impl InPlace {
    /// Opens `main`, building its index at `index_path` when it is missing or stale.
    pub fn open(main: &Path, index_path: &Path, progress: &dyn Fn(usize)) -> Result<Self> {
        let format = format_of(main).ok_or_else(|| anyhow!("'{}' is in no format this engine reads.", main.display()))?;
        let root = main.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = |p: &Path| PathBuf::from(p.file_name().unwrap_or_default());
        match format {
            DSL => {
                let decoding = DslLines::new(main)?.decoding();
                let stamp = Stamp::of(&root, &[name(main)])?;
                let index = lkidx::open_or_build(index_path, DSL, &stamp, || {
                    let mut builder = Builder::new();
                    let info = {
                        let mut sink = Spans { builder: &mut builder, span: (0, 0, 0), ordinal: 0, progress };
                        dsl_entries(DslLines::new(main)?, default_name(main), &mut sink)?
                    };
                    Ok((builder, info.name, info.description))
                })?;
                Ok(Self { index, kind: Kind::Dsl { decoding }, data: dictzip::Source::open(main)?, format })
            }
            _ => {
                let info = stardict_info(main)?;
                let (idx, dict, syn) = stardict_files(main)?;
                // The .dict is file 0, as the index's locations say.
                let mut files = vec![name(&dict), name(main), name(&idx)];
                files.extend(syn.as_deref().map(name));
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
                Ok(Self { index, kind: Kind::StarDict { sequence: info.sequence }, data: dictzip::Source::open(&dict)?, format })
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

    pub fn find(&self, word: &str, limit: usize) -> Vec<u32> {
        self.index.find(word, limit)
    }

    pub fn headword(&self, id: u32) -> Option<String> {
        self.index.title(id).map(str::to_string)
    }

    pub fn entry_html(&self, id: u32) -> Result<String> {
        let location = self.index.location(id).ok_or_else(|| anyhow!("no entry {id}"))?;
        let bytes = self.data.read_at(location.offset, location.length as usize)?;
        match &self.kind {
            Kind::StarDict { sequence } => stardict_article(&bytes, sequence),
            Kind::Dsl { decoding } => {
                let mut html = Html::default();
                dsl_entries(DslLines::over(bytes, *decoding), String::new(), &mut html)?;
                html.0.into_iter().nth(location.kind as usize).ok_or_else(|| anyhow!("DSL article changed since it was indexed"))
            }
        }
    }
}

/// The decompressed size of a StarDict .dict(.dz), to check the index's offsets against.
fn data_length(dict: &Path) -> Result<u64> {
    if file_name(dict).ends_with(".dz") {
        Ok(std::io::copy(&mut dictzip::Source::stream(dict)?, &mut std::io::sink())?)
    } else {
        Ok(std::fs::metadata(dict)?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionaries::entry_link;
    use flate2::{write::GzEncoder, Compression};
    use std::fs::{self, File};
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
        InPlace::open(main, &main.with_extension("lkidx"), &|_| {}).unwrap()
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
        assert!(path.with_extension("lkidx").is_file());
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
        assert!(InPlace::open(&ifo, &ifo.with_extension("lkidx"), &|_| {}).is_err());
    }
}
