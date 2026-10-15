//! Yomitan dictionaries read in place, as Lookupper reads them: the bank files stay as they are,
//! and `yomitan.lkidx` beside them records where every term, kanji and meta row is. A lookup reads
//! only that row from its bank and renders it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, ensure, Context, Result};
use serde_json::value::RawValue;
use serde_json::Value;

use super::containers::{yomitan_banks, yomitan_index, yomitan_kanji_html, yomitan_meta_html, yomitan_tag_notes,
    yomitan_term_html, yomitan_term_keys};
use super::lkidx::{self, Builder, Index, Location, Stamp};

pub const FORMAT: &str = "yomitan";
/// The index's name, as Lookupper's: it lives in the dictionary folder.
pub const INDEX_FILE: &str = "yomitan.lkidx";
const TERM: u16 = 0;
const KANJI: u16 = 1;
const TERM_META: u16 = 2;
const KANJI_META: u16 = 3;
const BANKS: [(&str, u16); 4] = [("term_bank_", TERM), ("kanji_bank_", KANJI), ("term_meta_bank_", TERM_META), ("kanji_meta_bank_", KANJI_META)];
/// Largest bank read whole while indexing, and largest row read at lookup time.
const MAX_BANK: u64 = 256 * 1024 * 1024;
const MAX_ROW: usize = 16 * 1024 * 1024;

pub fn supports(dir: &Path) -> bool {
    dir.join("index.json").is_file()
}

pub struct Yomitan {
    root: PathBuf,
    index: Index,
    kinds: Vec<u16>,
    version: u64,
    tags: HashMap<String, String>,
}

impl Yomitan {
    /// Opens the dictionary in `dir`, building its index at `index_path` when it is missing or
    /// stale. `progress` receives the number of entries indexed so far.
    pub fn open(dir: &Path, index_path: &Path, progress: &dyn Fn(usize)) -> Result<Self> {
        let (title, description, version, index_json) = yomitan_index(&dir.join("index.json"))?;
        let mut files = Vec::new();
        let mut kinds = Vec::new();
        for (prefix, kind) in BANKS {
            for path in yomitan_banks(dir, prefix)? {
                files.push(PathBuf::from(path.file_name().ok_or_else(|| anyhow!("bank without a name"))?));
                kinds.push(kind);
            }
        }
        ensure!(!files.is_empty(), "No Yomitan banks in '{}'.", dir.display());
        ensure!(files.len() < u16::MAX as usize, "too many Yomitan banks");
        // index.json is stamped too: a new revision of the dictionary is indexed again.
        files.push(PathBuf::from("index.json"));
        let stamp = Stamp::of(dir, &files)?;
        let index = lkidx::open_or_build(index_path, FORMAT, &stamp, || {
            let builder = build(dir, &files[..kinds.len()], &kinds, progress)?;
            Ok((builder, title, description))
        })?;
        let tags = yomitan_tag_notes(dir, &index_json)?;
        Ok(Self { root: dir.to_path_buf(), index, kinds, version, tags })
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
        let bytes = lkidx::read(&self.root, &self.index.stamp, location, MAX_ROW)?;
        let row: Vec<Value> = serde_json::from_slice(&bytes).context("Yomitan row")?;
        let kind = self.kinds.get(location.file as usize).copied().unwrap_or(location.kind);
        match kind {
            TERM => yomitan_term_html(&row, self.version, &self.tags),
            KANJI => yomitan_kanji_html(&row, self.version, &self.tags),
            _ => Ok(yomitan_meta_html(&row, kind == KANJI_META)?.map(|(_, _, html)| html).unwrap_or_default()),
        }
    }
}

fn string(raw: Option<&&RawValue>) -> Option<String> {
    raw.and_then(|r| serde_json::from_str::<String>(r.get()).ok())
}

/// Reads every bank once and records each row's place in it.
fn build(dir: &Path, files: &[PathBuf], kinds: &[u16], progress: &dyn Fn(usize)) -> Result<Builder> {
    let mut builder = Builder::new();
    for (file, (name, &kind)) in files.iter().zip(kinds).enumerate() {
        let path = dir.join(name);
        let size = fs::metadata(&path)?.len();
        ensure!(size <= MAX_BANK, "Banco Yomitan excede 256 MiB: {}", path.display());
        let bytes = fs::read(&path)?;
        let text = std::str::from_utf8(&bytes).with_context(|| format!("{} não é UTF-8", path.display()))?;
        let base = text.as_ptr() as usize;
        let body = text.strip_prefix('\u{feff}').unwrap_or(text);
        let rows: Vec<&RawValue> = serde_json::from_str(body).with_context(|| format!("Banco inválido: {}", path.display()))?;
        for raw in rows {
            let json = raw.get();
            let location = Location { file: file as u16, kind, offset: (json.as_ptr() as usize - base) as u64, length: json.len() as u32 };
            ensure!(json.len() <= MAX_ROW, "Linha Yomitan excede 16 MiB em {}", path.display());
            let cells: Vec<&RawValue> = serde_json::from_str(json).with_context(|| format!("Linha inválida em {}", path.display()))?;
            match kind {
                TERM => {
                    let (word, reading) = yomitan_term_keys(&cells.iter().map(|c| serde_json::from_str(c.get()).unwrap_or(Value::Null)).take(2).collect::<Vec<_>>())?;
                    let aliases: Vec<&str> = if !reading.is_empty() && reading != word { vec![reading.as_str()] } else { Vec::new() };
                    builder.add(&word, location, &aliases)?;
                }
                KANJI => {
                    let word = string(cells.first()).ok_or_else(|| anyhow!("Kanji Yomitan sem caractere"))?;
                    builder.add(&word, location, &[])?;
                }
                _ => {
                    // Rows of a mode Lookupper does not show are left out of the index.
                    let row: Vec<Value> = serde_json::from_str(json)?;
                    if let Some((word, reading, _)) = yomitan_meta_html(&row, kind == KANJI_META)? {
                        let aliases: Vec<&str> = if !reading.is_empty() && reading != word { vec![reading.as_str()] } else { Vec::new() };
                        builder.add(&word, location, &aliases)?;
                    }
                }
            }
        }
        progress(builder.len());
    }
    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionaries::entry_link;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static ID: AtomicU64 = AtomicU64::new(0);
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("lookitan-yomitan-{}-{stamp}-{}", std::process::id(), ID.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn open(&self) -> Yomitan {
            Yomitan::open(&self.0, &self.0.join(INDEX_FILE), &|_| {}).unwrap()
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    fn html(d: &Yomitan, word: &str) -> Vec<String> {
        d.find(word, 16).into_iter().map(|id| d.entry_html(id).unwrap()).collect()
    }

    #[test]
    fn structured_content_tags_readings_kanji_and_links_render_from_the_banks() {
        let dir = TestDir::new();
        fs::write(dir.0.join("index.json"), r#"{"title":"Study Yomitan","format":3,"revision":"1"}"#).unwrap();
        fs::write(dir.0.join("tag_bank_1.json"), r#"[["n","partOfSpeech",0,"noun",0]]"#).unwrap();
        let terms = serde_json::json!([["猫", "ねこ", "n", "", 1, [{"type":"structured-content","content":{"tag":"div","data":{"content":"glossary"},"content":["cat <unsafe>",{"tag":"ruby","content":["猫",{"tag":"rt","content":"ねこ"}]},{"tag":"a","href":"?query=%E7%8A%AC","content":"dog"},{"tag":"img","path":"images/cat.png","width":2,"sizeUnits":"em"}]}}], 1, ""]]);
        fs::write(dir.0.join("term_bank_1.json"), terms.to_string()).unwrap();
        fs::write(dir.0.join("kanji_bank_1.json"), r#"[["猫","ビョウ","ねこ","n",["cat"],{"strokes":"11"}]]"#).unwrap();
        let d = dir.open();
        assert_eq!((d.title(), d.len()), ("Study Yomitan", 2));
        let found = html(&d, "ねこ");
        assert_eq!(found.len(), 1, "the reading finds the term");
        let term = &found[0];
        assert!(term.contains("cat &lt;unsafe&gt;"));
        assert!(term.contains("data-sc-content=\"glossary\""));
        assert!(term.contains("<ruby>猫<rt>ねこ</rt></ruby>"));
        assert!(term.contains(&entry_link("犬")));
        assert!(term.contains("src=\"images/cat.png\""));
        assert!(term.contains("width:2em"));
        assert!(term.contains("title=\"noun\""));
        let both = html(&d, "猫");
        assert_eq!(both.len(), 2, "term and kanji share the headword");
        assert!(both[1].contains("strokes</dt><dd>11"));
        assert_eq!(d.headword(0).as_deref(), Some("猫"));
        assert!(dir.0.join(INDEX_FILE).is_file(), "the index sits beside the banks");
    }

    #[test]
    fn frequency_and_pitch_dictionaries_become_articles() {
        let dir = TestDir::new();
        fs::write(dir.0.join("index.json"), r#"{"title":"Pitch and frequency","format":3,"revision":"1"}"#).unwrap();
        fs::write(dir.0.join("term_meta_bank_1.json"), r#"[
            ["猫","pitch",{"reading":"ねこ","pitches":[{"position":1}]}],
            ["橋","pitch",{"reading":"はし","pitches":[{"position":2}]}],
            ["今日","freq",{"reading":"きょう","frequency":{"value":12,"displayValue":"12㋕"}}],
            ["the","freq",5],
            ["猫","mystery",{}]
        ]"#).unwrap();
        fs::write(dir.0.join("kanji_meta_bank_1.json"), r#"[["猫","freq",{"value":1702}]]"#).unwrap();
        let d = dir.open();
        assert_eq!(d.len(), 5, "unknown modes are skipped");
        let cat = html(&d, "猫");
        assert_eq!(cat.len(), 2);
        assert!(cat[0].contains("Acento") && cat[0].contains("[1]"));
        assert!(cat[0].contains("<span style=\"border-top:1px solid currentColor;border-right:1px solid currentColor;\">ね</span>こ"), "{}", cat[0]);
        assert!(cat[1].contains("1702"));
        assert!(html(&d, "はし")[0].contains("は<span style=\"border-top:1px solid currentColor;border-right:1px solid currentColor;\">し</span>"), "odaka drops after the last mora");
        assert!(html(&d, "きょう")[0].contains("12㋕"));
        assert!(html(&d, "the")[0].contains("Frequência</span> 5"));
    }

    #[test]
    fn version_1_banks_and_a_changed_bank_reindex() {
        let dir = TestDir::new();
        fs::write(dir.0.join("index.json"), r#"{"title":"Old format","version":1,"revision":"1"}"#).unwrap();
        fs::write(dir.0.join("term_bank_1.json"), "\u{feff}[[\"word\",\"\",\"\",\"\",0,\"one\",\"two\"]]").unwrap();
        let d = dir.open();
        assert!(html(&d, "word")[0].contains("<li>one</li><li>two</li>"), "a BOM does not shift the offsets");
        drop(d);
        fs::write(dir.0.join("term_bank_1.json"), r#"[["other","","","",0,"three"]]"#).unwrap();
        let d = dir.open();
        assert!(d.find("word", 4).is_empty());
        assert!(html(&d, "other")[0].contains("three"));
    }
}
