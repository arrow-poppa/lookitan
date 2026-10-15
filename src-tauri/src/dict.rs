// Installed dictionaries: an MDict entry file plus its companion resource files.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::Serialize;
use parking_lot::Mutex;

use crate::mdict::Mdict;
use crate::dictionaries::{source, store::Store};

enum Reader {
    Mdict(Mdict),
    Indexed(Store),
}

pub struct Dictionary {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    source_path: PathBuf,
    reader: Reader,
    pub format: String,
    pub description: String,
    pub warnings: Vec<String>,
    /// `name.mdd`, then `name.1.mdd`, `name.2.mdd`, ...
    pub mdds: Vec<Mdict>,
    hits: Mutex<VecDeque<(String, Vec<u32>)>>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EntryRef {
    pub dict_id: String,
    pub dict_name: String,
    pub headword: String,
    pub entry: u32,
    /// The form that matched (may differ from the word on screen, e.g. a singular).
    pub matched: String,
}

impl Dictionary {
    pub fn open_cached(id: &str, path: &Path, cache: &Path, progress: &dyn Fn(usize)) -> Result<Self> {
        if id.is_empty() || id.len() > 128 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err(anyhow!("invalid local dictionary identifier"));
        }
        let path = source::prepare(path, cache, id)?;
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut mdds = Vec::new();
        let mut warnings = Vec::new();
        let (reader, name, format, description) = if path.extension().map(|e| e.eq_ignore_ascii_case("mdx")).unwrap_or(false) {
            let mdx = Mdict::open_cached(&path, &cache.join("mdict"))?;
            let name = mdx.title.clone();
            for p in companion_mdds(&path)? {
                match Mdict::open_cached(&p, &cache.join("mdict")) {
                    Ok(m) => mdds.push(m),
                    Err(e) => warnings.push(format!("{}: {e:#}", p.display())),
                }
            }
            (Reader::Mdict(mdx), name, "MDict".to_string(), String::new())
        } else {
            let store = Store::open(&path, &cache.join("indexes"), id, progress)?;
            let name = store.name.clone();
            let format = store.format.clone();
            let description = store.description.clone();
            (Reader::Indexed(store), name, format, description)
        };
        Ok(Self { id: id.to_string(), name, dir, source_path: path, reader, format, description, warnings, mdds,
            hits: Mutex::new(VecDeque::new()) })
    }

    pub fn len(&self) -> usize {
        match &self.reader { Reader::Mdict(m) => m.len(), Reader::Indexed(s) => s.len() }
    }

    pub fn headword(&self, idx: u32) -> Option<String> {
        match &self.reader { Reader::Mdict(m) => m.headword(idx).map(str::to_string), Reader::Indexed(s) => s.headword(idx) }
    }

    pub fn find(&self, word: &str) -> Vec<u32> {
        // Keep the original query: a case-sensitive MDict must not share folded cache entries.
        if let Some((_, result)) = self.hits.lock().iter().find(|(k, _)| k == word) { return result.clone(); }
        let result = match &self.reader {
            Reader::Mdict(m) => m.find(word),
            Reader::Indexed(s) => s.find(word).unwrap_or_else(|e| { log::warn!("dictionary lookup: {e:#}"); Vec::new() }),
        };
        let result: Vec<_> = result.into_iter().take(crate::dictionaries::store::MAX_HITS).collect();
        let mut cache = self.hits.lock();
        if cache.len() >= 128 { cache.pop_front(); }
        cache.push_back((word.to_string(), result.clone()));
        result
    }

    pub fn stylesheets(&self) -> Vec<String> {
        let mut names = vec!["styles.css".to_string(), "style.css".to_string()];
        if let Reader::Mdict(m) = &self.reader {
            if let Some(stem) = m.path.file_stem() { names.insert(0, format!("{}.css", stem.to_string_lossy())); }
        }
        names.into_iter().filter(|name| self.resource(name).is_some()).collect()
    }

    /// Entry HTML, following `@@@LINK=` redirects.
    pub fn entry_html(&self, mut idx: u32) -> Result<String> {
        let mut visited = HashSet::new();
        for _ in 0..32 {
            if !visited.insert(idx) { return Err(anyhow!("dictionary redirect cycle")); }
            let text = match &self.reader {
                Reader::Mdict(m) => m.record_text(idx)?,
                Reader::Indexed(s) => s.record_text(idx)?,
            };
            let trimmed = text.trim();
            if let Some(target) = trimmed.strip_prefix("@@@LINK=") {
                let target = target.lines().next().unwrap_or("").trim();
                idx = *self.find(target).first().ok_or_else(|| anyhow!("broken link to {target}"))?;
                continue;
            }
            return Ok(text);
        }
        Err(anyhow!("too many redirects"))
    }

    /// A resource referenced by an entry: a file next to the .mdx first, then the .mdd files.
    pub fn resource(&self, rel: &str) -> Option<Vec<u8>> {
        let normalized = rel.replace('\\', "/");
        let rel = normalized.trim_start_matches('/');
        if rel.is_empty() || rel.contains(':') || rel.split('/').any(|part| part == "..") {
            return None;
        }
        source::resource(&self.dir, rel)
            .or_else(|| self.mdds.iter().find_map(|m| m.resource(rel)))
            .or_else(|| if self.format == "slob" {
                match &self.reader {
                    Reader::Indexed(store) => store.resource_reference(rel)
                        .and_then(|index| crate::dictionaries::slob::resource_at(&self.source_path, index)),
                    _ => None,
                }
            } else { None })
            .or_else(|| if self.format == "stardict" { source::resource(&self.dir, &format!("res/{rel}")) } else { None })
    }
}

fn companion_mdds(path: &Path) -> Result<Vec<PathBuf>> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        if !e.file_type()?.is_file() || !p.extension().map(|s| s.eq_ignore_ascii_case("mdd")).unwrap_or(false) { continue; }
        let name = p.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
        let number = if name == stem { Some(0) } else {
            name.strip_prefix(&format!("{stem}.")).and_then(|n| n.parse::<u64>().ok())
        };
        if let Some(n) = number { files.push((n, p)); }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    Ok(files.into_iter().map(|(_, p)| p).collect())
}

pub type DictList = Vec<Arc<Dictionary>>;

/// Looks up the candidate forms in every dictionary, in dictionary order. Within one dictionary the
/// first candidate that matches wins, so the form on screen beats a guessed base form.
pub fn lookup(dicts: &DictList, candidates: &[String]) -> Vec<EntryRef> {
    let mut out = Vec::new();
    for d in dicts {
        for cand in candidates {
            let hits = d.find(cand);
            if hits.is_empty() {
                continue;
            }
            for idx in hits {
                out.push(EntryRef {
                    dict_id: d.id.clone(),
                    dict_name: d.name.clone(),
                    headword: d.headword(idx).unwrap_or_else(|| cand.clone()),
                    entry: idx,
                    matched: cand.clone(),
                });
            }
            break;
        }
    }
    out
}

/// Surface and Unicode-normalized spellings only. The lookup engine expands these with the
/// configured recognition language; rules for English must not alter words in other languages.
pub fn candidate_forms(word: &str) -> Vec<String> {
    crate::dictionaries::morphology::candidates(word, "und")
}
