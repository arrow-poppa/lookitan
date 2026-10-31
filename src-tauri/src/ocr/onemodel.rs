// OneOCR's model file (oneocr.onemodel, the Windows Snipping Tool's), read without oneocr.dll, for
// OneOCR on Linux (ocr/oneocr_onnx.rs; at the user's request on 2026-10-30): the user imports their
// own copy, never in the installers. Its format, as shiyori/oneocr-native documents it (its code is
// not used) and as checked on the Snipping Tool 11.2608's file:
//
// - the file: an encrypted index, then the body, each a little-endian u64 length and its bytes;
// - a record: AES-256-CBC (IV "Copyright @ OneO", PKCS #7 padding) with the SHA-256 of its key
//   material and its salt as key. The index's material is OneOCR's key (`KEY`, which oneocr.dll is
//   handed too); the others' is 16 bytes in the clear in the record, after its 16-byte salt (names
//   have no salt). The plain text starts with its size, its size + 24 and `MARKER` (the size and
//   that total are the clear bytes when the record carries them), then the payload;
// - the index: the configuration (a record), the resource count, then each resource's name (a
//   record), its offset and size in the body (one after another), and a 1.
//
// Resources are read and decrypted one at a time, when a model loads, from the file on disk.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use sha2::{Digest, Sha256};

/// The key OneOCR's model is opened with, as oneocr and owocr pass it to oneocr.dll.
pub(crate) const KEY: &str = r#"kj)TGtrK>f]b[Piow.gU+nC@s""""""4"#;
const IV: &[u8; 16] = b"Copyright @ OneO";
const MARKER: u64 = 0x25_2b08_1a4a;
/// Larger than any resource or index there is.
const MAX_BYTES: u64 = 1 << 30;
/// The recognizers' script groups, each `Character\<group>\…` in the file.
pub const GROUPS: [&str; 9] =
    ["LatinPrintedV2", "CJKPrinted", "CyrillicMixed", "ArabicMixed", "DevanagariMixed", "GreekMixed", "ThaiMixed", "HebrewPrinted", "TamilPrinted"];

type Decryptor = cbc::Decryptor<aes::Aes256>;

/// A resource: its name after `Model_Edge\` with `/` between folders, and where it lies in the file.
#[derive(Clone, Debug)]
struct Entry {
    name: String,
    at: u64,
    len: u64,
}

pub struct OneModel {
    path: PathBuf,
    entries: Vec<Entry>,
}

impl OneModel {
    /// Reads the file's index (its resources are read with `read`).
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let size = file.metadata()?.len();
        let index_len = u64_at(&mut file)?;
        ensure!(index_len <= MAX_BYTES && 8 + index_len + 8 <= size, "não é um oneocr.onemodel (índice fora do arquivo)");
        let mut index = vec![0u8; index_len as usize];
        file.read_exact(&mut index)?;
        let body_len = u64_at(&mut file)?;
        let body_at = 8 + index_len + 8;
        ensure!(body_at + body_len == size, "não é um oneocr.onemodel (tamanho inesperado)");
        let index = decrypt(&index, Some(KEY.as_bytes()), true).context("não é um oneocr.onemodel (o índice não abre com a chave do OneOCR)")?;
        let entries = parse_index(&index, body_at, body_len)?;
        Ok(Self { path: path.to_path_buf(), entries })
    }

    /// The resource whose name (after `Model_Edge\`, folders with `/`) matches, decrypted.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let e = self.entries.iter().find(|e| e.name == name).with_context(|| format!("oneocr.onemodel has no {name}"))?;
        let mut file = File::open(&self.path).with_context(|| format!("open {}", self.path.display()))?;
        file.seek(SeekFrom::Start(e.at))?;
        let mut bytes = vec![0u8; e.len as usize];
        file.read_exact(&mut bytes)?;
        decrypt(&bytes, None, true).with_context(|| format!("decrypt {name}"))
    }

    /// The first resource in `folder` (e.g. `Detector/`) ending with `suffix`.
    pub fn find(&self, folder: &str, suffix: &str) -> Option<&str> {
        self.entries.iter().find(|e| e.name.starts_with(folder) && e.name.ends_with(suffix)).map(|e| e.name.as_str())
    }

    /// The detector's model.
    pub fn detector(&self) -> Option<&str> {
        self.find("Detector/", ".onnx")
    }

    /// The script classifier's model.
    pub fn classifier(&self) -> Option<&str> {
        self.find("AuxMltCls/", ".onnx")
    }

    /// A script group's recognizer and its character list.
    pub fn recognizer(&self, group: &str) -> Option<(&str, &str)> {
        let folder = format!("Character/{group}/");
        Some((self.find(&folder, ".onnx")?, self.find(&folder, "/char2ind.txt")?))
    }

    /// A script group's composite characters (private-use characters standing for a letter and its
    /// marks), when it has any.
    pub fn composites(&self, group: &str) -> Option<&str> {
        self.find(&format!("Character/{group}/"), "/composite_chars_map")
    }

    /// What OneOCR on Linux needs: the detector, the classifier and every group's recognizer.
    pub fn check(&self) -> Result<()> {
        ensure!(self.detector().is_some(), "o arquivo não tem o detector do OneOCR");
        ensure!(self.classifier().is_some(), "o arquivo não tem o classificador de escrita do OneOCR");
        for group in GROUPS {
            ensure!(self.recognizer(group).is_some(), "o arquivo não tem o reconhecedor {group} do OneOCR");
        }
        Ok(())
    }
}

fn u64_at(file: &mut File) -> Result<u64> {
    let mut b = [0u8; 8];
    file.read_exact(&mut b).context("não é um oneocr.onemodel (curto demais)")?;
    Ok(u64::from_le_bytes(b))
}

/// Reads `data` from the start, as the index lays it out.
struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: u64) -> Result<&'a [u8]> {
        let n = usize::try_from(n).ok().filter(|n| *n <= self.data.len() - self.at).context("oneocr.onemodel's index is cut short")?;
        let b = &self.data[self.at..self.at + n];
        self.at += n;
        Ok(b)
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn blob(&mut self, max: u64) -> Result<&'a [u8]> {
        let n = self.u64()?;
        ensure!(n <= max, "oneocr.onemodel's index has a field too large");
        self.take(n)
    }
}

/// The resources the index lists, each where it lies in the file (the body starts at `body_at`).
fn parse_index(index: &[u8], body_at: u64, body_len: u64) -> Result<Vec<Entry>> {
    let mut c = Cursor { data: index, at: 0 };
    // The configuration, opened only to be sure the index is whole.
    decrypt(c.blob(MAX_BYTES)?, None, true).context("oneocr.onemodel's configuration")?;
    let count = c.u64()?;
    ensure!((1..=4096).contains(&count), "oneocr.onemodel lists {count} resources");
    let mut entries = Vec::with_capacity(count as usize);
    let mut next = 0u64;
    for i in 0..count {
        let name = decrypt(c.blob(16_384)?, None, false).with_context(|| format!("oneocr.onemodel's resource {i} name"))?;
        let name = String::from_utf8(name).ok().filter(|n| !n.is_empty() && !n.contains('\0')).with_context(|| format!("oneocr.onemodel's resource {i} has no name"))?;
        let (offset, len) = (c.u64()?, c.u64()?);
        let flag = c.take(1)?[0];
        ensure!(flag == 1 && offset == next && offset.checked_add(len).is_some_and(|end| end <= body_len), "oneocr.onemodel's resource {i} is out of place");
        next = offset + len;
        // `\\?\C:\…\Model_Edge\Detector\Universal\ONNX\….onnx` → `Detector/Universal/ONNX/….onnx`.
        let short = name.rsplit_once("Model_Edge\\").map_or(name.as_str(), |(_, s)| s).replace('\\', "/");
        ensure!(!entries.iter().any(|e: &Entry| e.name == short), "oneocr.onemodel lists {short} twice");
        entries.push(Entry { name: short, at: body_at + offset, len });
    }
    ensure!(next == body_len && c.at == index.len(), "oneocr.onemodel's index does not cover its body");
    Ok(entries)
}

/// A record's payload: its salt (when `salted`), its key material (`key`, or the 16 bytes in the
/// clear that follow), then AES-256-CBC.
fn decrypt(record: &[u8], key: Option<&[u8]>, salted: bool) -> Result<Vec<u8>> {
    let mut c = Cursor { data: record, at: 0 };
    let salt: &[u8] = if salted { c.take(16)? } else { &[] };
    let clear = match key {
        Some(_) => None,
        None => Some(c.take(16)?),
    };
    let material = key.or(clear).unwrap_or_default();
    let mut buffer = record[c.at..].to_vec();
    ensure!(!buffer.is_empty() && buffer.len() % 16 == 0, "a record of {} bytes is not AES blocks", buffer.len());
    let mut hash = Sha256::new();
    hash.update(material);
    hash.update(salt);
    let aes_key: [u8; 32] = hash.finalize().into();
    let plain = Decryptor::new(&aes_key.into(), IV.into()).decrypt_padded_mut::<Pkcs7>(&mut buffer).map_err(|_| anyhow::anyhow!("wrong key or a damaged record"))?;
    let word = |b: &[u8], i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
    let (size, total, marker, start) = match clear {
        Some(clear) => {
            ensure!(plain.len() >= 8, "a record too short");
            (word(clear, 0), word(clear, 8), word(plain, 0), 8)
        }
        None => {
            ensure!(plain.len() >= 24, "a record too short");
            (word(plain, 0), word(plain, 8), word(plain, 16), 24)
        }
    };
    if marker != MARKER || size > MAX_BYTES || total != size + 24 || start + size as usize != plain.len() {
        bail!("not a OneOCR record");
    }
    Ok(plain[start..].to_vec())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use cbc::cipher::BlockEncryptMut;

    /// A record as the file has it: `key` or 16 bytes of material in the clear.
    fn record(payload: &[u8], key: Option<&[u8]>, salt: Option<[u8; 16]>) -> Vec<u8> {
        let size = payload.len() as u64;
        let clear: [u8; 16] = {
            let mut c = [0u8; 16];
            c[..8].copy_from_slice(&size.to_le_bytes());
            c[8..].copy_from_slice(&(size + 24).to_le_bytes());
            c
        };
        let mut plain = Vec::new();
        if key.is_some() {
            plain.extend(size.to_le_bytes());
            plain.extend((size + 24).to_le_bytes());
        }
        plain.extend(MARKER.to_le_bytes());
        plain.extend(payload);
        let salt_bytes = salt.unwrap_or_default();
        let mut hash = Sha256::new();
        hash.update(key.unwrap_or(&clear));
        let salted: &[u8] = if salt.is_some() { &salt_bytes } else { &[] };
        hash.update(salted);
        let aes_key: [u8; 32] = hash.finalize().into();
        let n = plain.len();
        plain.resize(n + 16 - n % 16, 0);
        let sealed = cbc::Encryptor::<aes::Aes256>::new(&aes_key.into(), IV.into()).encrypt_padded_mut::<Pkcs7>(&mut plain, n).unwrap().to_vec();
        let mut out = Vec::new();
        if let Some(s) = salt {
            out.extend(s);
        }
        if key.is_none() {
            out.extend(clear);
        }
        out.extend(sealed);
        out
    }

    fn blob(b: &[u8]) -> Vec<u8> {
        let mut out = (b.len() as u64).to_le_bytes().to_vec();
        out.extend(b);
        out
    }

    /// A small file laid out as oneocr.onemodel, with these resources.
    pub(crate) fn file(resources: &[(&str, &[u8])]) -> Vec<u8> {
        let mut index = blob(&record(b"config", None, Some([7; 16])));
        index.extend((resources.len() as u64).to_le_bytes());
        let mut body = Vec::new();
        for (name, data) in resources {
            let sealed = record(data, None, Some([3; 16]));
            index.extend(blob(&record(format!("\\\\?\\C:\\b\\Model_Edge\\{name}").as_bytes(), None, None)));
            index.extend((body.len() as u64).to_le_bytes());
            index.extend((sealed.len() as u64).to_le_bytes());
            index.push(1);
            body.extend(sealed);
        }
        let mut out = blob(&record(&index, Some(KEY.as_bytes()), Some([9; 16])));
        out.extend(blob(&body));
        out
    }

    fn written(bytes: &[u8], name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("lookitan-onemodel-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_resource_reads_back_by_its_name_and_folder() {
        let bytes = file(&[("Detector\\Universal\\ONNX\\det.onnx", b"detector"), ("Character\\CJKPrinted\\char2ind.txt", "日 1\n".as_bytes())]);
        let path = written(&bytes, "ok");
        let model = OneModel::open(&path).unwrap();
        assert_eq!(model.detector(), Some("Detector/Universal/ONNX/det.onnx"));
        assert_eq!(model.read("Detector/Universal/ONNX/det.onnx").unwrap(), b"detector");
        assert_eq!(model.read(model.find("Character/CJKPrinted/", "/char2ind.txt").unwrap()).unwrap(), "日 1\n".as_bytes());
        assert!(model.read("Detector/none.onnx").is_err());
        assert!(model.check().is_err(), "no classifier nor recognizers");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_changed_byte_another_key_or_a_short_file_is_refused() {
        let bytes = file(&[("Detector\\a.onnx", b"a")]);
        let mut changed = bytes.clone();
        changed[40] ^= 1;
        let mut short = bytes.clone();
        short.truncate(bytes.len() - 3);
        for (name, b) in [("changed", changed), ("short", short), ("junk", b"MZ not a model at all".to_vec())] {
            let path = written(&b, name);
            assert!(OneModel::open(&path).is_err(), "{name}");
            let _ = std::fs::remove_file(path);
        }
        assert!(decrypt(&record(b"x", Some(b"another key"), Some([1; 16])), Some(KEY.as_bytes()), true).is_err());
        assert_eq!(decrypt(&record(b"x", Some(KEY.as_bytes()), Some([1; 16])), Some(KEY.as_bytes()), true).unwrap(), b"x");
    }
}
