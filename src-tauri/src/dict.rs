// Installed dictionaries: an MDict entry file plus its companion resource files.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::mdict::Mdict;

pub struct Dictionary {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    pub mdx: Mdict,
    /// `name.mdd`, then `name.1.mdd`, `name.2.mdd`, ...
    pub mdds: Vec<Mdict>,
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
    pub fn open(id: &str, mdx_path: &Path) -> Result<Self> {
        let mdx = Mdict::open(mdx_path)?;
        let dir = mdx_path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = mdx_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut mdds = Vec::new();
        let first = dir.join(format!("{stem}.mdd"));
        if first.exists() {
            match Mdict::open(&first) {
                Ok(m) => mdds.push(m),
                Err(e) => log::warn!("{}: {e:#}", first.display()),
            }
            for n in 1..32 {
                let p = dir.join(format!("{stem}.{n}.mdd"));
                if !p.exists() {
                    break;
                }
                match Mdict::open(&p) {
                    Ok(m) => mdds.push(m),
                    Err(e) => log::warn!("{}: {e:#}", p.display()),
                }
            }
        }
        Ok(Self { id: id.to_string(), name: mdx.title.clone(), dir, mdx, mdds })
    }

    /// Entry HTML, following `@@@LINK=` redirects.
    pub fn entry_html(&self, mut idx: u32) -> Result<String> {
        for _ in 0..6 {
            let text = self.mdx.record_text(idx)?;
            let trimmed = text.trim();
            if let Some(target) = trimmed.strip_prefix("@@@LINK=") {
                let target = target.lines().next().unwrap_or("").trim();
                idx = *self.mdx.find(target).first().ok_or_else(|| anyhow!("broken link to {target}"))?;
                continue;
            }
            return Ok(text);
        }
        Err(anyhow!("too many redirects"))
    }

    /// A resource referenced by an entry: a file next to the .mdx first, then the .mdd files.
    pub fn resource(&self, rel: &str) -> Option<Vec<u8>> {
        let rel = rel.trim_start_matches(['/', '\\']);
        if rel.is_empty() || rel.split(['/', '\\']).any(|part| part == "..") {
            return None;
        }
        let direct = self.dir.join(rel);
        if direct.is_file() {
            return std::fs::read(direct).ok();
        }
        // Case-insensitive match for a plain file name next to the dictionary.
        if !rel.contains(['/', '\\']) {
            if let Ok(rd) = std::fs::read_dir(&self.dir) {
                for e in rd.flatten() {
                    if e.file_name().to_string_lossy().eq_ignore_ascii_case(rel) {
                        return std::fs::read(e.path()).ok();
                    }
                }
            }
        }
        self.mdds.iter().find_map(|m| m.resource(rel))
    }
}

pub type DictList = Vec<Arc<Dictionary>>;

/// Looks up the candidate forms in every dictionary, in dictionary order. Within one dictionary the
/// first candidate that matches wins, so the form on screen beats a guessed base form.
pub fn lookup(dicts: &DictList, candidates: &[String]) -> Vec<EntryRef> {
    let mut out = Vec::new();
    for d in dicts {
        for cand in candidates {
            let hits = d.mdx.find(cand);
            if hits.is_empty() {
                continue;
            }
            for idx in hits.into_iter().take(8) {
                out.push(EntryRef {
                    dict_id: d.id.clone(),
                    dict_name: d.name.clone(),
                    headword: d.mdx.headword(idx).unwrap_or(cand).to_string(),
                    entry: idx,
                    matched: cand.clone(),
                });
            }
            break;
        }
    }
    out
}

/// Forms to try for a word read off the screen: as read, without surrounding punctuation, and a few
/// common inflection reductions (plural, past, gerund, comparative) for alphabetic scripts.
pub fn candidate_forms(word: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        let s = s.trim().to_string();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    let cleaned: String = word
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_string();
    push(word.to_string());
    push(cleaned.clone());
    let lower = cleaned.to_lowercase();
    push(lower.clone());
    for suffix in ["'s", "’s", "s'", "s’"] {
        if let Some(base) = lower.strip_suffix(suffix) {
            push(base.to_string());
        }
    }
    let alpha = lower.chars().all(|c| c.is_alphabetic() || c == '-' || c == '\'');
    if alpha && lower.chars().count() > 3 {
        let rules: &[(&str, &str)] = &[
            ("ies", "y"),
            ("ied", "y"),
            ("ves", "f"),
            ("ves", "fe"),
            ("es", ""),
            ("s", ""),
            ("ing", ""),
            ("ing", "e"),
            ("ed", ""),
            ("ed", "e"),
            ("d", ""),
            ("er", ""),
            ("est", ""),
            ("ly", ""),
            ("ier", "y"),
            ("iest", "y"),
            ("ões", "ão"),
            ("ães", "ão"),
            ("ais", "al"),
            ("éis", "el"),
            ("is", "il"),
            ("ns", "m"),
            ("as", "a"),
            ("os", "o"),
        ];
        for (suffix, repl) in rules {
            if let Some(stem) = lower.strip_suffix(suffix) {
                if stem.chars().count() >= 2 {
                    push(format!("{stem}{repl}"));
                    // running -> run, stopped -> stop
                    let chars: Vec<char> = stem.chars().collect();
                    if (*suffix == "ing" || *suffix == "ed" || *suffix == "er" || *suffix == "est") && chars.len() >= 3 {
                        let n = chars.len();
                        if chars[n - 1] == chars[n - 2] && !"aeiou".contains(chars[n - 1]) {
                            push(chars[..n - 1].iter().collect());
                        }
                    }
                }
            }
        }
    }
    out
}
