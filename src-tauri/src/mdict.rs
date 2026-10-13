// Reader for MDict dictionaries (.mdx entries and .mdd resources), format versions 1.x and 2.x.
//
// Layout (all integers big-endian unless noted):
//   header length (u32) | header XML in UTF-16LE | adler32 (u32 LE)
//   key section: counts, key-block index (zlib, optionally scrambled), key blocks
//   record section: counts, record-block index, record blocks
// Each key carries the offset of its record inside the concatenation of all decompressed
// record blocks; a record ends where the next key's record starts.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use memmap2::Mmap;
use parking_lot::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Utf16,
    Gb18030,
    Big5,
}

impl Encoding {
    fn from_label(label: &str, is_mdd: bool) -> Self {
        if is_mdd {
            return Encoding::Utf16;
        }
        match label.trim().to_ascii_lowercase().as_str() {
            "utf-16" | "utf16" | "utf-16le" => Encoding::Utf16,
            "gbk" | "gb2312" | "gb18030" => Encoding::Gb18030,
            "big5" | "big-5" => Encoding::Big5,
            _ => Encoding::Utf8,
        }
    }

    fn decode(self, bytes: &[u8]) -> String {
        let enc = match self {
            Encoding::Utf8 => encoding_rs::UTF_8,
            Encoding::Utf16 => encoding_rs::UTF_16LE,
            Encoding::Gb18030 => encoding_rs::GB18030,
            Encoding::Big5 => encoding_rs::BIG5,
        };
        enc.decode_without_bom_handling(bytes).0.into_owned()
    }
}

struct RecordBlock {
    file_offset: usize,
    comp_size: usize,
    decomp_offset: u64,
    decomp_size: u64,
}

struct Key {
    text: String,
    record_offset: u64,
}

pub struct Mdict {
    pub path: PathBuf,
    pub title: String,
    pub is_mdd: bool,
    mmap: Mmap,
    encoding: Encoding,
    keys: Vec<Key>,
    /// (normalized key, index into `keys`), sorted by normalized key.
    index: Vec<(String, u32)>,
    record_blocks: Vec<RecordBlock>,
    cache: Mutex<Vec<(usize, Arc<Vec<u8>>)>>,
}

const CACHE_BLOCKS: usize = 12;

pub fn normalize_key(s: &str) -> String {
    s.trim().to_lowercase()
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| anyhow!("offset overflow"))?;
        if end > self.data.len() {
            bail!("unexpected end of data at {} (+{n})", self.pos);
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    /// A count or size: 8 bytes in version 2 files, 4 bytes in version 1 files.
    fn num(&mut self, wide: bool) -> Result<u64> {
        if wide {
            self.u64()
        } else {
            Ok(self.u32()? as u64)
        }
    }
}

fn parse_header_attributes(text: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Find `name="value"`.
        if let Some(eq) = text[i..].find("=\"") {
            let eq = i + eq;
            let mut start = eq;
            while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
                start -= 1;
            }
            let name = &text[start..eq];
            let value_start = eq + 2;
            if let Some(end) = text[value_start..].find('"') {
                let value = &text[value_start..value_start + end];
                if !name.is_empty() {
                    attrs.insert(name.to_string(), value.to_string());
                }
                i = value_start + end + 1;
                continue;
            }
        }
        break;
    }
    attrs
}

fn unescape_xml(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn zlib_decompress(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected);
    flate2::read::ZlibDecoder::new(data).read_to_end(&mut out).context("zlib")?;
    Ok(out)
}

/// Decompresses one key or record block: 4-byte type (little-endian), 4-byte checksum, payload.
fn decompress_block(block: &[u8], expected: usize) -> Result<Vec<u8>> {
    if block.len() < 8 {
        bail!("block too short");
    }
    let kind = u32::from_le_bytes([block[0], block[1], block[2], block[3]]) & 0xff;
    let payload = &block[8..];
    match kind {
        0 => Ok(payload.to_vec()),
        1 => bail!("LZO-compressed MDict blocks are not supported yet"),
        2 => zlib_decompress(payload, expected),
        other => bail!("unknown MDict block compression {other}"),
    }
}

/// Undoes the key-block-index scrambling used when `Encrypted` has bit 2 set.
fn decrypt_key_block_index(data: &[u8]) -> Result<Vec<u8>> {
    use ripemd::{Digest, Ripemd128};
    if data.len() < 8 {
        bail!("key block index too short");
    }
    let mut hasher = Ripemd128::new();
    hasher.update(&data[4..8]);
    hasher.update([0x95u8, 0x36, 0x00, 0x00]);
    let key = hasher.finalize();
    let mut out = data[..8].to_vec();
    let mut previous: u8 = 0x36;
    for (i, &b) in data[8..].iter().enumerate() {
        let swapped = b.rotate_left(4);
        let t = swapped ^ previous ^ (i as u8) ^ key[i % key.len()];
        previous = b;
        out.push(t);
    }
    Ok(out)
}

impl Mdict {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the dictionary file is opened read-only and not expected to change while mapped.
        let mmap = unsafe { Mmap::map(&file)? };
        let is_mdd = path.extension().map(|e| e.eq_ignore_ascii_case("mdd")).unwrap_or(false);
        Self::parse(path.to_path_buf(), mmap, is_mdd)
    }

    fn parse(path: PathBuf, mmap: Mmap, is_mdd: bool) -> Result<Self> {
        let data: &[u8] = &mmap;
        let mut c = Cursor { data, pos: 0 };

        let header_len = c.u32()? as usize;
        let header_bytes = c.take(header_len)?;
        let _checksum = c.take(4)?;
        let header_text = if header_bytes.len() >= 2 && header_bytes[header_bytes.len() - 2..] == [0, 0] {
            encoding_rs::UTF_16LE.decode_without_bom_handling(header_bytes).0.into_owned()
        } else {
            String::from_utf8_lossy(header_bytes).into_owned()
        };
        let header_text = header_text.trim_end_matches('\0').to_string();
        let attrs = parse_header_attributes(&header_text);

        let version: f32 = attrs
            .get("GeneratedByEngineVersion")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(2.0);
        if version >= 3.0 {
            bail!("MDict format version {version} is not supported yet");
        }
        let wide = version >= 2.0;
        let encrypted: u32 = match attrs.get("Encrypted").map(|s| s.trim()) {
            None | Some("") | Some("No") => 0,
            Some("Yes") => 1,
            Some(other) => other.parse().unwrap_or(0),
        };
        if encrypted & 1 != 0 {
            bail!("this dictionary needs a registration key (Encrypted=1), which is not supported");
        }
        let encoding = Encoding::from_label(attrs.get("Encoding").map(String::as_str).unwrap_or(""), is_mdd);
        let title = attrs
            .get("Title")
            .map(|t| unescape_xml(t))
            .filter(|t| !t.trim().is_empty() && !t.contains("Title (No HTML code allowed)"))
            .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());

        // --- key section ---
        let num_key_blocks;
        let key_index_size;
        let key_blocks_size;
        let mut key_index_decomp_size = 0u64;
        if wide {
            num_key_blocks = c.u64()?;
            let _num_entries = c.u64()?;
            key_index_decomp_size = c.u64()?;
            key_index_size = c.u64()?;
            key_blocks_size = c.u64()?;
            let _checksum = c.take(4)?;
        } else {
            num_key_blocks = c.u32()? as u64;
            let _num_entries = c.u32()?;
            key_index_size = c.u32()? as u64;
            key_blocks_size = c.u32()? as u64;
        }

        let key_index_raw = c.take(key_index_size as usize)?;
        let key_index = if wide {
            if key_index_raw.len() < 8 || key_index_raw[..4] != [2, 0, 0, 0] {
                bail!("unexpected key block index compression");
            }
            let unscrambled;
            let src = if encrypted & 2 != 0 {
                unscrambled = decrypt_key_block_index(key_index_raw)?;
                &unscrambled[..]
            } else {
                key_index_raw
            };
            zlib_decompress(&src[8..], key_index_decomp_size as usize)?
        } else {
            key_index_raw.to_vec()
        };

        // Each index entry: entry count, first key, last key, compressed and decompressed sizes.
        let mut key_block_sizes = Vec::with_capacity(num_key_blocks as usize);
        {
            let mut ki = Cursor { data: &key_index, pos: 0 };
            let unit = if encoding == Encoding::Utf16 { 2 } else { 1 };
            let term = if wide { 1 } else { 0 };
            for _ in 0..num_key_blocks {
                let _entries = ki.num(wide)?;
                let head = if wide { ki.u16()? as usize } else { ki.u8()? as usize };
                ki.take((head + term) * unit)?;
                let tail = if wide { ki.u16()? as usize } else { ki.u8()? as usize };
                ki.take((tail + term) * unit)?;
                let comp = ki.num(wide)? as usize;
                let decomp = ki.num(wide)? as usize;
                key_block_sizes.push((comp, decomp));
            }
        }

        let key_blocks_start = c.pos;
        let mut keys = Vec::new();
        {
            let mut p = key_blocks_start;
            for &(comp, decomp) in &key_block_sizes {
                let block = data.get(p..p + comp).ok_or_else(|| anyhow!("key block out of range"))?;
                p += comp;
                let raw = decompress_block(block, decomp)?;
                split_key_block(&raw, wide, encoding, &mut keys)?;
            }
        }
        c.pos = key_blocks_start + key_blocks_size as usize;

        // --- record section ---
        let num_record_blocks = c.num(wide)?;
        let _num_entries = c.num(wide)?;
        let _record_index_size = c.num(wide)?;
        let _record_blocks_size = c.num(wide)?;
        let mut sizes = Vec::with_capacity(num_record_blocks as usize);
        for _ in 0..num_record_blocks {
            let comp = c.num(wide)?;
            let decomp = c.num(wide)?;
            sizes.push((comp, decomp));
        }
        let mut record_blocks = Vec::with_capacity(sizes.len());
        let mut file_offset = c.pos;
        let mut decomp_offset = 0u64;
        for (comp, decomp) in sizes {
            record_blocks.push(RecordBlock {
                file_offset,
                comp_size: comp as usize,
                decomp_offset,
                decomp_size: decomp,
            });
            file_offset += comp as usize;
            decomp_offset += decomp;
        }
        if file_offset > data.len() {
            bail!("record blocks extend past the end of the file");
        }

        let mut index: Vec<(String, u32)> =
            keys.iter().enumerate().map(|(i, k)| (normalize_key(&k.text), i as u32)).collect();
        index.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

        log::info!("loaded {} ({} keys, {} record blocks)", path.display(), keys.len(), record_blocks.len());
        Ok(Self {
            path,
            title,
            is_mdd,
            mmap,
            encoding,
            keys,
            index,
            record_blocks,
            cache: Mutex::new(Vec::new()),
        })
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn headword(&self, idx: u32) -> Option<&str> {
        self.keys.get(idx as usize).map(|k| k.text.as_str())
    }

    /// Every entry whose normalized key equals the normalized `word`, in file order.
    pub fn find(&self, word: &str) -> Vec<u32> {
        let key = normalize_key(word);
        if key.is_empty() {
            return Vec::new();
        }
        let start = self.index.partition_point(|(k, _)| k.as_str() < key.as_str());
        let mut out = Vec::new();
        for (k, i) in &self.index[start..] {
            if *k != key {
                break;
            }
            out.push(*i);
        }
        out
    }

    fn block_data(&self, block_idx: usize) -> Result<Arc<Vec<u8>>> {
        {
            let mut cache = self.cache.lock();
            if let Some(pos) = cache.iter().position(|(i, _)| *i == block_idx) {
                let entry = cache.remove(pos);
                let data = entry.1.clone();
                cache.insert(0, entry);
                return Ok(data);
            }
        }
        let b = &self.record_blocks[block_idx];
        let raw = self
            .mmap
            .get(b.file_offset..b.file_offset + b.comp_size)
            .ok_or_else(|| anyhow!("record block out of range"))?;
        let data = Arc::new(decompress_block(raw, b.decomp_size as usize)?);
        let mut cache = self.cache.lock();
        cache.insert(0, (block_idx, data.clone()));
        cache.truncate(CACHE_BLOCKS);
        Ok(data)
    }

    /// The raw bytes of entry `idx`.
    pub fn record_bytes(&self, idx: u32) -> Result<Vec<u8>> {
        let i = idx as usize;
        let start = self.keys.get(i).ok_or_else(|| anyhow!("no entry {idx}"))?.record_offset;
        let block_idx = self
            .record_blocks
            .partition_point(|b| b.decomp_offset + b.decomp_size <= start);
        let b = self.record_blocks.get(block_idx).ok_or_else(|| anyhow!("record offset {start} out of range"))?;
        let block_end = b.decomp_offset + b.decomp_size;
        let end = self
            .keys
            .get(i + 1)
            .map(|k| k.record_offset)
            .filter(|&e| e > start && e <= block_end)
            .unwrap_or(block_end);
        let data = self.block_data(block_idx)?;
        let from = (start - b.decomp_offset) as usize;
        let to = ((end - b.decomp_offset) as usize).min(data.len());
        Ok(data.get(from..to).unwrap_or_default().to_vec())
    }

    /// The text of entry `idx` (for .mdx files), without trailing NULs.
    pub fn record_text(&self, idx: u32) -> Result<String> {
        let bytes = self.record_bytes(idx)?;
        let text = self.encoding.decode(&bytes);
        Ok(text.trim_end_matches('\0').to_string())
    }

    /// Resource lookup for .mdd files, where keys look like `\images\a.png`.
    pub fn resource(&self, path: &str) -> Option<Vec<u8>> {
        let mut key = path.replace('/', "\\");
        if !key.starts_with('\\') {
            key.insert(0, '\\');
        }
        let hits = self.find(&key);
        let idx = *hits.first()?;
        self.record_bytes(idx).ok()
    }
}

fn split_key_block(raw: &[u8], wide: bool, encoding: Encoding, keys: &mut Vec<Key>) -> Result<()> {
    let id_width = if wide { 8 } else { 4 };
    let mut p = 0;
    while p + id_width <= raw.len() {
        let id_bytes = &raw[p..p + id_width];
        let record_offset = if wide {
            u64::from_be_bytes(id_bytes.try_into().unwrap())
        } else {
            u32::from_be_bytes(id_bytes.try_into().unwrap()) as u64
        };
        p += id_width;
        let text_start = p;
        let text_end;
        if encoding == Encoding::Utf16 {
            let mut q = p;
            loop {
                if q + 1 >= raw.len() {
                    text_end = raw.len();
                    p = raw.len();
                    break;
                }
                if raw[q] == 0 && raw[q + 1] == 0 {
                    text_end = q;
                    p = q + 2;
                    break;
                }
                q += 2;
            }
        } else {
            match raw[p..].iter().position(|&b| b == 0) {
                Some(n) => {
                    text_end = p + n;
                    p = p + n + 1;
                }
                None => {
                    text_end = raw.len();
                    p = raw.len();
                }
            }
        }
        let text = encoding.decode(&raw[text_start..text_end]);
        keys.push(Key { text, record_offset });
    }
    Ok(())
}
