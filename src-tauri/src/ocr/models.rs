// OCR models that are not in the installers: downloaded when the user picks the engine that reads
// with them, from a pinned Hugging Face revision, each file checked against its SHA-256 and moved
// into place only once whole. They live in the app's local data folder (`set_root`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};

pub struct ModelFile {
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

pub struct ModelSet {
    /// The folder's name, with the revision's first 8 characters after it.
    pub id: &'static str,
    /// The Hugging Face repository and the commit its files come from.
    pub repo: &'static str,
    pub revision: &'static str,
    pub files: &'static [ModelFile],
}

/// MeikiOCR's recognizers (rtr46, weights LGPL-3.0): one for lines, one for columns.
pub static MEIKI: ModelSet = ModelSet {
    id: "meiki-rec-v0",
    repo: "rtr46/meiki.txt.recognition.v0",
    revision: "a28cf5874dc2438ebb1c86336be26bcec51e3375",
    files: &[
        ModelFile {
            name: "meiki.text.rec.v0.960x32.onnx",
            bytes: 18_593_254,
            sha256: "3e96bc772fbee9717e536a6353032bb944c3382dd2f6960ef4890decda43b000",
        },
        ModelFile {
            name: "meiki.text.rec.v0.vertical.32x480.onnx",
            bytes: 12_872_961,
            sha256: "2c2a83a23bc3b7e6c63962175f507ecc6c5e85cc174f17bdec37d9bbd0bf895a",
        },
    ],
};

static ROOT: OnceLock<PathBuf> = OnceLock::new();

pub fn set_root(dir: PathBuf) {
    let _ = ROOT.set(dir);
}

impl ModelSet {
    pub fn dir(&self) -> Result<PathBuf> {
        let root = ROOT.get().context("the OCR models folder is not set")?;
        Ok(root.join(format!("{}-{}", self.id, &self.revision[..8])))
    }

    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    /// Whether every file is in place. Its SHA-256 was checked when it was downloaded; a file is
    /// moved into place only once whole, so its size tells it is there.
    pub fn installed(&self) -> bool {
        self.dir().is_ok_and(|dir| self.files.iter().all(|f| has(&dir.join(f.name), f.bytes)))
    }

    /// Downloads what is missing, reporting the bytes done of `bytes()`, and returns the folder.
    pub fn ensure(&self, progress: &dyn Fn(u64)) -> Result<PathBuf> {
        let dir = self.dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let mut done = 0;
        for f in self.files {
            let path = dir.join(f.name);
            if !has(&path, f.bytes) {
                // An interrupted download is started again.
                let part = dir.join(format!(".download-{}", f.name));
                let url = format!("https://huggingface.co/{}/resolve/{}/{}", self.repo, self.revision, f.name);
                let fetched = crate::segment::download(&url, &part, f.bytes, Some(f.sha256), &|n| progress(done + n));
                if let Err(e) = fetched {
                    let _ = std::fs::remove_file(&part);
                    return Err(e);
                }
                std::fs::rename(&part, &path).with_context(|| format!("move {} into place", f.name))?;
            }
            done += f.bytes;
            progress(done);
        }
        Ok(dir)
    }

    pub fn remove(&self) -> Result<()> {
        let dir = self.dir()?;
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("delete {}", dir.display()))?;
        }
        Ok(())
    }
}

fn has(path: &Path, bytes: u64) -> bool {
    path.metadata().is_ok_and(|m| m.is_file() && m.len() == bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_sets_are_pinned() {
        for set in [&MEIKI] {
            assert_eq!(set.revision.len(), 40);
            for f in set.files {
                assert_eq!(f.sha256.len(), 64);
                assert!(f.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
                assert!(f.bytes > 0);
            }
        }
        assert_eq!(MEIKI.bytes(), 31_466_215);
    }
}
