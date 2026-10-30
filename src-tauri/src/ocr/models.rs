// OCR models that are not in the installers: downloaded when the user picks the engine that reads
// with them, from a pinned Hugging Face revision (or a GitHub release), each file checked against
// its SHA-256 and moved into place only once whole. They live in the app's local data folder
// (`set_root`), each set in a folder of its own, its files side by side.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};

pub struct ModelFile {
    /// Its path in the repository or the release; it is saved under its last part.
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

impl ModelFile {
    /// Its name in the set's folder.
    pub fn file(&self) -> &'static str {
        self.name.rsplit('/').next().unwrap_or(self.name)
    }
}

/// Where a set's files are published.
pub enum Source {
    /// A Hugging Face repository at a commit (`revision`).
    HuggingFace,
    /// A GitHub repository's release (`revision` is its tag); the SHA-256s pin the files.
    GitHubRelease,
}

pub struct ModelSet {
    /// The folder's name, with the revision's first 8 characters after it.
    pub id: &'static str,
    /// The repository and the commit (or release) its files come from.
    pub repo: &'static str,
    pub revision: &'static str,
    pub source: Source,
    pub files: &'static [ModelFile],
}

/// MeikiOCR's recognizers (rtr46, weights LGPL-3.0): one for lines, one for columns.
pub static MEIKI: ModelSet = ModelSet {
    id: "meiki-rec-v0",
    repo: "rtr46/meiki.txt.recognition.v0",
    revision: "a28cf5874dc2438ebb1c86336be26bcec51e3375",
    source: Source::HuggingFace,
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

/// IndraOCR's line detector (LineNet, since 2026-10-29) and its recognizers for the groups
/// Lookitan's languages read with (datalab-to, commit
/// bd02167f, `main` on 2026-10-28; its files are byte for byte those of tag v1.1.0, a1283c4b, which
/// indra-ocr 1.1.2 pins, checked on 2026-10-29; the folder keeps its v1.1.0 name; weights under the modified AI Pubs Open RAIL-M licence, shipped with
/// them as its §4 asks), and their vocabularies. Downloaded only after the user accepts it.
pub static INDRA: ModelSet = ModelSet {
    id: "indra-ocr-v1.1.0",
    repo: "datalab-to/indra-ocr",
    revision: "bd02167fdf2005a19962cc15188f8496a2c87d3b",
    source: Source::HuggingFace,
    files: &[
        ModelFile { name: "det.onnx", bytes: 389_948, sha256: "f2bd4cba23e9c328cf7ec824634e423e191a2c6aacbbf35c62bd79dd70f62f59" },
        ModelFile { name: "rec.onnx", bytes: 5_042_161, sha256: "475972a2710be4a3cbfeb2bc7f3624ffd563de07de42476fc7a290d4e1ece484" },
        ModelFile { name: "rec_cjk.onnx", bytes: 10_176_018, sha256: "4a035516a17e62cdf33a4fe2d88077654cb8841cff024685cdef4e351694ef50" },
        ModelFile { name: "rec_rtl.onnx", bytes: 4_812_710, sha256: "468796bd684486a8388c30c8b4ed2a26e3eeb1f8ff02831c24c2e12c79a2a834" },
        ModelFile { name: "meta.json", bytes: 211_869, sha256: "4ce4c94b46f5d644033c02e718322014ff4081270f385038c571912b56696339" },
        ModelFile { name: "LICENSE", bytes: 14_750, sha256: "50c88a018a7ee322e89f6b09401b237beb0cd331685879405b3db48993950a5f" },
    ],
};

/// PP-OCRv6 Manga v0.2 (Kellenok/PP-OCRv6_manga, Apache-2.0; `main` on 2026-10-30, whose v0.2
/// files are those of its 2026-09-28 commit): PP-OCRv6's tiny detector and small recognizer
/// fine-tuned on manga, webtoons and illustrations, Japanese and Chinese. The recognizer reads with
/// rec_small's character list (its ppocrv6_dict.txt is the same file), so the list is not
/// downloaded.
pub static MANGA: ModelSet = ModelSet {
    id: "ppocrv6-manga-v0.2",
    repo: "Kellenok/PP-OCRv6_manga",
    revision: "62375557b5b0a4b2bc8e05484a97c7ea1df7229e",
    source: Source::HuggingFace,
    files: &[
        ModelFile { name: "det/manga_det_v0.2.onnx", bytes: 1_816_954, sha256: "d132078c46e292b226fb5a2ca52a7612ad319262dfdf493e7d8e3be435295978" },
        ModelFile { name: "rec/manga_rec_v0.2.onnx", bytes: 21_167_540, sha256: "de12c84c63e62c80339e882e675983d886670dcb6f0147e1ed041afd6fa81888" },
    ],
};

/// comic-text-detector's ONNX model (dmMaze/comic-text-detector, GPL-3.0), where its README points:
/// manga-image-translator's release beta-0.2.1 (GPL-3.0). A release is not pinned by a commit, so
/// its SHA-256 (taken on 2026-10-30) pins it.
pub static CTD: ModelSet = ModelSet {
    id: "comic-text-detector",
    repo: "zyddnys/manga-image-translator",
    revision: "beta-0.2.1",
    source: Source::GitHubRelease,
    files: &[ModelFile { name: "comictextdetector.pt.onnx", bytes: 94_669_756, sha256: "1a86ace74961413cbd650002e7bb4dcec4980ffa21b2f19b86933372071d718f" }],
};

static ROOT: OnceLock<PathBuf> = OnceLock::new();

pub fn set_root(dir: PathBuf) {
    let _ = ROOT.set(dir);
}

/// The folder every optional model set lives in.
pub fn root() -> Result<PathBuf> {
    ROOT.get().cloned().context("the OCR models folder is not set")
}

impl ModelSet {
    pub fn dir(&self) -> Result<PathBuf> {
        Ok(root()?.join(format!("{}-{}", self.id, self.revision.get(..8).unwrap_or(self.revision))))
    }

    /// Where a file is downloaded from.
    fn url(&self, f: &ModelFile) -> String {
        match self.source {
            Source::HuggingFace => format!("https://huggingface.co/{}/resolve/{}/{}", self.repo, self.revision, f.name),
            Source::GitHubRelease => format!("https://github.com/{}/releases/download/{}/{}", self.repo, self.revision, f.name),
        }
    }

    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    /// Whether every file is in place. Its SHA-256 was checked when it was downloaded; a file is
    /// moved into place only once whole, so its size tells it is there.
    pub fn installed(&self) -> bool {
        self.dir().is_ok_and(|dir| self.files.iter().all(|f| has(&dir.join(f.file()), f.bytes)))
    }

    /// Whether some of its files are in place: downloaded once, by a version with fewer files.
    pub fn partly_installed(&self) -> bool {
        self.dir().is_ok_and(|dir| self.files.iter().any(|f| has(&dir.join(f.file()), f.bytes)))
    }

    /// Downloads what is missing, reporting the bytes done of `bytes()`, and returns the folder.
    pub fn ensure(&self, progress: &dyn Fn(u64)) -> Result<PathBuf> {
        let dir = self.dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let mut done = 0;
        for f in self.files {
            let path = dir.join(f.file());
            if !has(&path, f.bytes) {
                // An interrupted download is started again.
                let part = dir.join(format!(".download-{}", f.file()));
                let fetched = crate::segment::download(&self.url(f), &part, f.bytes, Some(f.sha256), &|n| progress(done + n));
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
        for set in [&MEIKI, &INDRA, &MANGA, &CTD] {
            if matches!(set.source, Source::HuggingFace) {
                assert_eq!(set.revision.len(), 40);
            }
            for f in set.files {
                assert_eq!(f.sha256.len(), 64);
                assert!(f.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
                assert!(f.bytes > 0);
            }
        }
        assert_eq!(MEIKI.bytes(), 31_466_215);
        assert_eq!(INDRA.bytes(), 20_647_456);
        assert_eq!(MANGA.bytes(), 22_984_494);
        assert_eq!(MANGA.files[1].file(), "manga_rec_v0.2.onnx", "saved without its folder");
        assert_eq!(CTD.url(&CTD.files[0]), "https://github.com/zyddnys/manga-image-translator/releases/download/beta-0.2.1/comictextdetector.pt.onnx");
    }
}
