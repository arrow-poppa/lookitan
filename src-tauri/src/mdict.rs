// Independent reader for MDict 1.x/2.x (.mdx entries and .mdd resources).
// Format reference: https://github.com/zhansliu/writemdict/blob/master/fileformat.md
// Compact styles are triples (id, opening HTML, closing HTML); `id` in a record
// styles the text until the next numeric marker. No Lookupper code is used here.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use memmap2::Mmap;
use parking_lot::Mutex;

// Materialization limits, not file-size limits: multi-GB MDDs stay mapped.
// Reject hostile/exceptionally large declarations before allocating on low-RAM PCs.
const MAX_HEADER_BYTES: usize = 2 * 1024 * 1024;
const MAX_BLOCK_BYTES: usize = 32 * 1024 * 1024;
const MAX_PACKED_BLOCK_BYTES: usize = 40 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;
const MAX_INDEX_BYTES: usize = 128 * 1024 * 1024;
const MAX_KEYS: usize = 2_000_000;
const MAX_KEY_BYTES: usize = 4 * 1024 * 1024;
const MAX_BLOCKS: usize = 262_144;
const CACHE_BYTES: usize = 4 * 1024 * 1024;
const CACHE_BLOCKS: usize = 8;

#[path = "mdict_index.rs"]
mod index_cache;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding { Utf8, Utf16, Gb18030, Big5 }

impl Encoding {
    fn from_label(label: &str, is_mdd: bool) -> Self {
        if is_mdd { return Self::Utf16; }
        match label.trim().to_ascii_lowercase().as_str() {
            "utf-16" | "utf16" | "utf-16le" => Self::Utf16,
            "gbk" | "gb2312" | "gb18030" => Self::Gb18030,
            "big5" | "big-5" => Self::Big5,
            _ => Self::Utf8,
        }
    }

    fn decode(self, bytes: &[u8]) -> String {
        let encoding = match self {
            Self::Utf8 => encoding_rs::UTF_8,
            Self::Utf16 => encoding_rs::UTF_16LE,
            Self::Gb18030 => encoding_rs::GB18030,
            Self::Big5 => encoding_rs::BIG5,
        };
        encoding.decode_without_bom_handling(bytes).0.into_owned()
    }
}

#[derive(Clone, Copy)]
struct KeyPolicy { case_sensitive: bool, strip: bool }

impl KeyPolicy {
    fn from_header(attrs: &HashMap<String, String>, is_mdd: bool) -> Self {
        // Missing flags deliberately preserve this reader's historical behavior:
        // case-insensitive, punctuation retained. Readers disagree on absent StripKey;
        // never collapse distinct spellings unless the publisher requests it.
        // MDD punctuation is structural (paths/extensions), so never strip it.
        Self {
            case_sensitive: header_yes(attrs, "KeyCaseSensitive"),
            strip: !is_mdd && header_yes(attrs, "StripKey"),
        }
    }

    fn normalize(self, text: &str) -> String {
        let text = text.trim();
        let stripped;
        let text = if self.strip {
            // StripKey removes non-alphanumeric ASCII; non-ASCII is retained.
            // This is not Unicode punctuation removal or locale-specific folding.
            stripped = text.chars().filter(|c| !c.is_ascii() || c.is_ascii_alphanumeric()).collect::<String>();
            stripped.as_str()
        } else { text };
        if self.case_sensitive { text.to_string() } else { text.to_lowercase() }
    }
}

fn header_yes(attrs: &HashMap<String, String>, key: &str) -> bool {
    attrs.get(key).map(|s| s.trim().eq_ignore_ascii_case("yes") || s.trim() == "1").unwrap_or(false)
}

struct RecordBlock {
    file_offset: usize,
    comp_size: usize,
    decomp_offset: u64,
    decomp_size: u64,
}

struct Key {
    text: Arc<str>,
    // Share the allocation when the key is already normalized.
    normalized: Arc<str>,
    record_offset: u64,
}

#[derive(Default)]
struct BlockCache { entries: Vec<(usize, Arc<Vec<u8>>)>, bytes: usize }

impl BlockCache {
    fn get(&mut self, index: usize) -> Option<Arc<Vec<u8>>> {
        let position = self.entries.iter().position(|(i, _)| *i == index)?;
        let entry = self.entries.remove(position);
        let data = entry.1.clone();
        self.entries.insert(0, entry);
        Some(data)
    }

    fn insert(&mut self, index: usize, data: Arc<Vec<u8>>, budget: usize) {
        if let Some(position) = self.entries.iter().position(|(i, _)| *i == index) {
            self.bytes -= self.entries.remove(position).1.len();
        }
        if data.len() > budget { return; }
        while self.bytes > budget - data.len() || self.entries.len() >= CACHE_BLOCKS {
            if let Some((_, old)) = self.entries.pop() { self.bytes -= old.len(); }
            else { break; }
        }
        self.bytes += data.len();
        self.entries.insert(0, (index, data));
    }
}

pub struct Mdict {
    pub path: PathBuf,
    pub title: String,
    pub is_mdd: bool,
    /// Decoded XML attributes, for dictionary information/settings.
    pub metadata: HashMap<String, String>,
    mmap: Mmap,
    encoding: Encoding,
    policy: KeyPolicy,
    keys: Vec<Key>,
    /// Physical ordinals sorted by normalized key, stable within duplicate keys.
    index: Vec<u32>,
    /// Unique record starts; non-adjacent aliases may share a start.
    record_offsets: Vec<u64>,
    record_blocks: Vec<RecordBlock>,
    record_size: u64,
    styles: HashMap<String, (String, String)>,
    cache: Mutex<BlockCache>,
}

pub fn normalize_key(s: &str) -> String {
    KeyPolicy { case_sensitive: false, strip: false }.normalize(s)
}

fn bounded(value: u64, limit: usize, label: &str) -> Result<usize> {
    let size = usize::try_from(value).map_err(|_| anyhow!("{label} does not fit in memory"))?;
    if size > limit { bail!("{label} exceeds the reader limit ({size} > {limit} bytes/items)"); }
    Ok(size)
}

fn charge(used: &mut usize, amount: usize) -> Result<()> {
    *used = used.checked_add(amount).ok_or_else(|| anyhow!("index size overflow"))?;
    if *used > MAX_INDEX_BYTES { bail!("dictionary key index exceeds the {MAX_INDEX_BYTES}-byte memory limit"); }
    Ok(())
}

struct Cursor<'a> { data: &'a [u8], pos: usize }

impl<'a> Cursor<'a> {
    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(size).ok_or_else(|| anyhow!("offset overflow"))?;
        let bytes = self.data.get(self.pos..end).ok_or_else(|| anyhow!("unexpected end of data at {} (+{size})", self.pos))?;
        self.pos = end;
        Ok(bytes)
    }
    fn u8(&mut self) -> Result<u8> { Ok(self.take(1)?[0]) }
    fn u16(&mut self) -> Result<u16> { Ok(u16::from_be_bytes(self.take(2)?.try_into()?)) }
    fn u32(&mut self) -> Result<u32> { Ok(u32::from_be_bytes(self.take(4)?.try_into()?)) }
    fn u64(&mut self) -> Result<u64> { Ok(u64::from_be_bytes(self.take(8)?.try_into()?)) }
    fn num(&mut self, wide: bool) -> Result<u64> { if wide { self.u64() } else { Ok(self.u32()? as u64) } }
}

fn parse_header_attributes(text: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let bytes = text.as_bytes();
    let mut position = 0;
    while position < bytes.len() {
        if !(bytes[position].is_ascii_alphabetic() || bytes[position] == b'_') { position += 1; continue; }
        let start = position;
        while position < bytes.len() && (bytes[position].is_ascii_alphanumeric() || b"_:-".contains(&bytes[position])) { position += 1; }
        let name_end = position;
        while position < bytes.len() && bytes[position].is_ascii_whitespace() { position += 1; }
        if bytes.get(position) != Some(&b'=') { continue; }
        position += 1;
        while position < bytes.len() && bytes[position].is_ascii_whitespace() { position += 1; }
        let Some(&quote) = bytes.get(position).filter(|&&c| c == b'\'' || c == b'"') else { continue; };
        position += 1;
        let value_start = position;
        while position < bytes.len() && bytes[position] != quote { position += 1; }
        if position == bytes.len() { break; }
        attrs.insert(text[start..name_end].to_string(), unescape_xml(&text[value_start..position]));
        position += 1;
    }
    attrs
}

fn unescape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';').filter(|&end| end <= 16) else {
            out.push('&'); rest = &rest[1..]; continue;
        };
        let entity = &rest[1..end];
        let value = match entity {
            "lt" => Some('<'), "gt" => Some('>'), "quot" => Some('"'), "apos" => Some('\''), "amp" => Some('&'),
            _ => entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok()).or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok()))
                .and_then(char::from_u32),
        };
        if let Some(value) = value { out.push(value); }
        else { out.push_str(&rest[..=end]); }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn zlib_decompress(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    bounded(expected as u64, MAX_BLOCK_BYTES, "decompressed block")?;
    let mut out = Vec::new();
    out.try_reserve_exact(expected + 1).context("allocate decompression buffer")?;
    out.resize(expected + 1, 0);
    // One extra byte distinguishes an exact-size stream from an inflated size bomb;
    // StreamEnd also rejects truncated payloads that happen to emit the expected bytes.
    let mut decoder = flate2::Decompress::new(true);
    let status = decoder.decompress(data, &mut out, flate2::FlushDecompress::Finish).context("zlib")?;
    if status != flate2::Status::StreamEnd || decoder.total_out() != expected as u64 || decoder.total_in() != data.len() as u64 {
        bail!("invalid zlib block or decompressed size mismatch (expected {expected}, got {})", decoder.total_out());
    }
    out.truncate(expected);
    Ok(out)
}

fn decompress_block(block: &[u8], expected: usize) -> Result<Vec<u8>> {
    bounded(expected as u64, MAX_BLOCK_BYTES, "decompressed block")?;
    let kind = u32::from_le_bytes(block.get(..4).ok_or_else(|| anyhow!("block too short"))?.try_into()?);
    let payload = block.get(8..).ok_or_else(|| anyhow!("block too short"))?;
    match kind {
        0 if payload.len() == expected => Ok(payload.to_vec()),
        0 => bail!("uncompressed block size mismatch"),
        1 => {
            let mut out = Vec::new();
            out.try_reserve_exact(expected).context("allocate LZO block")?;
            out.resize(expected, 0);
            let actual = lzokay::decompress::decompress(payload, &mut out).map_err(|error| anyhow!("LZO: {error:?}"))?;
            if actual != expected { bail!("LZO block size mismatch: expected {expected}, got {actual}"); }
            Ok(out)
        }
        2 => zlib_decompress(payload, expected),
        other => bail!("unknown MDict block compression {other}"),
    }
}

fn decrypt_key_block_index(data: &[u8]) -> Result<Vec<u8>> {
    use ripemd::{Digest, Ripemd128};
    if data.len() < 8 { bail!("key block index too short"); }
    let mut hasher = Ripemd128::new();
    hasher.update(&data[4..8]);
    hasher.update([0x95u8, 0x36, 0x00, 0x00]);
    let key = hasher.finalize();
    let mut out = Vec::new();
    out.try_reserve_exact(data.len()).context("allocate key block index")?;
    out.extend_from_slice(&data[..8]);
    let mut previous = 0x36u8;
    for (i, &byte) in data[8..].iter().enumerate() {
        out.push(byte.rotate_left(4) ^ previous ^ i as u8 ^ key[i % key.len()]);
        previous = byte;
    }
    Ok(out)
}

impl Mdict {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: opened read-only; users must not modify an installed file while in use.
        let mmap = unsafe { Mmap::map(&file)? };
        let is_mdd = path.extension().map(|e| e.eq_ignore_ascii_case("mdd")).unwrap_or(false);
        Self::parse(path.to_path_buf(), mmap, is_mdd)
    }

    /// Like `open`, with an app-owned persistent index. Invalid, stale or unwritable
    /// caches never prevent opening the source dictionary; records remain in the MDX/MDD.
    pub fn open_cached(path: &Path, cache_directory: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: same read-only-file lifetime contract as `open`.
        let mmap = unsafe { Mmap::map(&file)? };
        let is_mdd = path.extension().map(|e| e.eq_ignore_ascii_case("mdd")).unwrap_or(false);
        Self::parse_cached(path.to_path_buf(), mmap, is_mdd, Some(cache_directory))
    }

    fn parse(path: PathBuf, mmap: Mmap, is_mdd: bool) -> Result<Self> {
        Self::parse_cached(path, mmap, is_mdd, None)
    }

    fn parse_cached(path: PathBuf, mmap: Mmap, is_mdd: bool, cache_directory: Option<&Path>) -> Result<Self> {
        let mut cursor = Cursor { data: &mmap, pos: 0 };
        let header_size = bounded(cursor.u32()? as u64, MAX_HEADER_BYTES, "header")?;
        let header_bytes = cursor.take(header_size)?;
        cursor.take(4)?; // Header Adler32; checksum policy unchanged, structural checks mandatory.
        let header = if header_bytes.ends_with(&[0, 0]) || header_bytes.starts_with(&[0xff, 0xfe]) || header_bytes.get(1) == Some(&0) {
            if header_bytes.len() % 2 != 0 { bail!("odd UTF-16 header length"); }
            Encoding::Utf16.decode(header_bytes)
        } else { String::from_utf8_lossy(header_bytes).into_owned() };
        let metadata = parse_header_attributes(header.trim_end_matches('\0'));
        let version = metadata.get("GeneratedByEngineVersion").map(String::as_str).unwrap_or("2.0")
            .trim().parse::<f32>().context("invalid MDict format version")?;
        if !version.is_finite() || !(1.0..3.0).contains(&version) { bail!("MDict format version {version} is not supported (only 1.x/2.x)"); }
        let wide = version >= 2.0;
        let encrypted = match metadata.get("Encrypted").map(|s| s.trim()) {
            None | Some("") => 0,
            Some(s) if s.eq_ignore_ascii_case("no") => 0,
            Some(s) if s.eq_ignore_ascii_case("yes") => 1,
            Some(s) => s.parse::<u32>().context("invalid MDict encryption flags")?,
        };
        if encrypted & 1 != 0 { bail!("this dictionary needs a registration key (Encrypted=1), which is not supported"); }
        if encrypted & !3 != 0 { bail!("unsupported MDict encryption flags {encrypted}"); }
        let encoding = Encoding::from_label(metadata.get("Encoding").map(String::as_str).unwrap_or(""), is_mdd);
        let policy = KeyPolicy::from_header(&metadata, is_mdd);
        let title = metadata.get("Title").filter(|s| !s.trim().is_empty() && !s.contains("Title (No HTML code allowed)"))
            .cloned().unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        let styles = parse_styles(metadata.get("StyleSheet").map(String::as_str).unwrap_or(""));

        let key_block_count = bounded(cursor.num(wide)?, MAX_BLOCKS, "key block count")?;
        let entry_count = bounded(cursor.num(wide)?, MAX_KEYS, "entry count")?;
        let key_index_unpacked = if wide { bounded(cursor.u64()?, MAX_BLOCK_BYTES, "key index")? } else { 0 };
        let key_index_size = bounded(cursor.num(wide)?, MAX_PACKED_BLOCK_BYTES, "packed key index")?;
        let key_blocks_size = usize::try_from(cursor.num(wide)?).context("key blocks size overflow")?;
        if wide { cursor.take(4)?; }
        let key_index_bytes = cursor.take(key_index_size)?;
        // Skip compressed keys on a warm open; their sidecar is loaded after the
        // record table establishes the valid offset range.
        let key_blocks_bytes = cursor.take(key_blocks_size)?;

        let record_count = bounded(cursor.num(wide)?, MAX_BLOCKS, "record block count")?;
        if bounded(cursor.num(wide)?, MAX_KEYS, "record entry count")? != entry_count { bail!("record entry count mismatch"); }
        let record_index_size = usize::try_from(cursor.num(wide)?).context("record index size overflow")?;
        let record_blocks_size = usize::try_from(cursor.num(wide)?).context("record blocks size overflow")?;
        let expected_index_size = record_count.checked_mul(if wide { 16 } else { 8 }).ok_or_else(|| anyhow!("record index size overflow"))?;
        if record_index_size != expected_index_size { bail!("record index size mismatch"); }
        let mut record_info = Cursor { data: cursor.take(record_index_size)?, pos: 0 };
        let record_start = cursor.pos;
        cursor.take(record_blocks_size)?;
        let mut record_blocks = Vec::new();
        record_blocks.try_reserve_exact(record_count).context("allocate record block index")?;
        let mut file_offset = record_start;
        let mut record_size = 0u64;
        for _ in 0..record_count {
            let packed = bounded(record_info.num(wide)?, MAX_PACKED_BLOCK_BYTES, "packed record block")?;
            let unpacked = bounded(record_info.num(wide)?, MAX_BLOCK_BYTES, "record block")? as u64;
            if packed < 8 { bail!("record block too short"); }
            record_blocks.push(RecordBlock { file_offset, comp_size: packed, decomp_offset: record_size, decomp_size: unpacked });
            file_offset = file_offset.checked_add(packed).ok_or_else(|| anyhow!("record file offset overflow"))?;
            record_size = record_size.checked_add(unpacked).ok_or_else(|| anyhow!("record offset overflow"))?;
        }
        if file_offset != cursor.pos { bail!("record blocks size mismatch"); }
        let identity = cache_directory.and_then(|_| index_cache::Identity::source(&path, mmap.len(), header_bytes, entry_count).ok());
        let cached = cache_directory.zip(identity.as_ref()).and_then(|(directory, identity)| index_cache::load(directory, identity, record_size));
        let (keys, index, record_offsets) = if let Some(cached) = cached {
            (cached.keys, cached.index, cached.offsets)
        } else {
            let keys = read_keys(wide, encrypted, encoding, policy, key_block_count, entry_count, key_index_unpacked, key_index_bytes, key_blocks_bytes)?;
            if keys.iter().any(|key| key.record_offset > record_size) { bail!("key record offset out of range"); }
            let mut index = Vec::new();
            index.try_reserve_exact(keys.len()).context("allocate search index")?;
            index.extend(0..keys.len() as u32);
            index.sort_unstable_by(|a, b| keys[*a as usize].normalized.cmp(&keys[*b as usize].normalized).then(a.cmp(b)));
            let mut record_offsets = Vec::new();
            record_offsets.try_reserve_exact(keys.len()).context("allocate record offsets")?;
            record_offsets.extend(keys.iter().map(|key| key.record_offset));
            record_offsets.sort_unstable();
            record_offsets.dedup();
            if let Some((directory, identity)) = cache_directory.zip(identity.as_ref()) {
                index_cache::save(directory, identity, &keys, &index, &record_offsets);
            }
            (keys, index, record_offsets)
        };
        log::info!("loaded {} ({} keys, {} record blocks)", path.display(), keys.len(), record_blocks.len());
        Ok(Self { path, title, is_mdd, metadata, mmap, encoding, policy, keys, index, record_offsets,
            record_blocks, record_size, styles, cache: Mutex::new(BlockCache::default()) })
    }

    pub fn len(&self) -> usize { self.keys.len() }
    pub fn headword(&self, index: u32) -> Option<&str> { self.keys.get(index as usize).map(|key| key.text.as_ref()) }

    pub fn find(&self, word: &str) -> Vec<u32> {
        let key = self.policy.normalize(word);
        if key.is_empty() { return Vec::new(); }
        let start = self.index.partition_point(|index| self.keys[*index as usize].normalized.as_ref() < key.as_str());
        self.index[start..].iter().copied().take_while(|index| self.keys[*index as usize].normalized.as_ref() == key).collect()
    }

    fn block_data(&self, index: usize) -> Result<Arc<Vec<u8>>> {
        // Serialize inflation: simultaneous iframe/audio requests cannot inflate the
        // same block twice or temporarily grow this file's cache concurrently.
        let mut cache = self.cache.lock();
        if let Some(data) = cache.get(index) { return Ok(data); }
        let block = self.record_blocks.get(index).ok_or_else(|| anyhow!("no record block {index}"))?;
        let end = block.file_offset.checked_add(block.comp_size).ok_or_else(|| anyhow!("record range overflow"))?;
        let raw = self.mmap.get(block.file_offset..end).ok_or_else(|| anyhow!("record block out of range"))?;
        let data = Arc::new(decompress_block(raw, bounded(block.decomp_size, MAX_BLOCK_BYTES, "record block")?)?);
        cache.insert(index, data.clone(), CACHE_BYTES);
        Ok(data)
    }

    pub fn record_bytes(&self, index: u32) -> Result<Vec<u8>> {
        let start = self.keys.get(index as usize).ok_or_else(|| anyhow!("no entry {index}"))?.record_offset;
        // Skip repeated starts: aliases own the same record, including aliases not
        // adjacent in alphabetical key order.
        let next = self.record_offsets.partition_point(|&offset| offset <= start);
        let end = self.record_offsets.get(next).copied().unwrap_or(self.record_size);
        let length = end.checked_sub(start).ok_or_else(|| anyhow!("invalid record range"))?;
        let length = bounded(length, MAX_RECORD_BYTES, "entry/resource")?;
        let mut out = Vec::new();
        out.try_reserve_exact(length).context("allocate entry/resource")?;
        let first = self.record_blocks.partition_point(|block| block.decomp_offset + block.decomp_size <= start);
        for index in first..self.record_blocks.len() {
            let block = &self.record_blocks[index];
            if block.decomp_offset >= end { break; }
            let from = start.max(block.decomp_offset) - block.decomp_offset;
            let to = end.min(block.decomp_offset + block.decomp_size) - block.decomp_offset;
            let data = self.block_data(index)?;
            let bytes = data.get(from as usize..to as usize).ok_or_else(|| anyhow!("record slice out of range"))?;
            out.extend_from_slice(bytes);
        }
        if out.len() != length { bail!("incomplete record: expected {length}, read {}", out.len()); }
        Ok(out)
    }

    pub fn record_text(&self, index: u32) -> Result<String> {
        let text = self.encoding.decode(&self.record_bytes(index)?);
        let text = text.trim_end_matches('\0');
        if self.styles.is_empty() || self.is_mdd { Ok(text.to_string()) }
        else { expand_styles(text, &self.styles) }
    }

    pub fn resource(&self, path: &str) -> Option<Vec<u8>> {
        let mut key = path.replace('/', "\\");
        if !key.starts_with('\\') { key.insert(0, '\\'); }
        self.record_bytes(*self.find(&key).first()?).ok()
    }
}

#[allow(clippy::too_many_arguments)]
fn read_keys(wide: bool, encrypted: u32, encoding: Encoding, policy: KeyPolicy, block_count: usize, entry_count: usize,
    index_unpacked: usize, index_bytes: &[u8], blocks: &[u8]) -> Result<Vec<Key>> {
    let key_index;
    let index_bytes = if wide {
        if index_bytes.get(..4) != Some(&[2, 0, 0, 0][..]) { bail!("unexpected key block index compression"); }
        let decrypted;
        let packed = if encrypted & 2 != 0 {
            decrypted = decrypt_key_block_index(index_bytes)?;
            decrypted.as_slice()
        } else { index_bytes };
        key_index = zlib_decompress(packed.get(8..).ok_or_else(|| anyhow!("key block index too short"))?, index_unpacked)?;
        key_index.as_slice()
    } else { index_bytes };
    let mut info = Cursor { data: index_bytes, pos: 0 };
    let mut data = Cursor { data: blocks, pos: 0 };
    let mut keys = Vec::new();
    let mut budget = 0usize;
    let unit = if encoding == Encoding::Utf16 { 2 } else { 1 };
    for _ in 0..block_count {
        let block_entries = bounded(info.num(wide)?, MAX_KEYS, "key block entry count")?;
        for _ in 0..2 {
            let length = if wide { info.u16()? as usize + 1 } else { info.u8()? as usize };
            info.take(length * unit)?;
        }
        let packed = bounded(info.num(wide)?, MAX_PACKED_BLOCK_BYTES, "packed key block")?;
        let unpacked = bounded(info.num(wide)?, MAX_BLOCK_BYTES, "key block")?;
        let block = decompress_block(data.take(packed)?, unpacked)?;
        let before = keys.len();
        split_key_block(&block, wide, encoding, policy, &mut keys, &mut budget)?;
        if keys.len() - before != block_entries { bail!("key block entry count mismatch"); }
    }
    if info.pos != index_bytes.len() || data.pos != blocks.len() || keys.len() != entry_count { bail!("key section size/count mismatch"); }
    Ok(keys)
}

fn split_key_block(raw: &[u8], wide: bool, encoding: Encoding, policy: KeyPolicy, keys: &mut Vec<Key>, budget: &mut usize) -> Result<()> {
    let mut cursor = Cursor { data: raw, pos: 0 };
    let unit = if encoding == Encoding::Utf16 { 2 } else { 1 };
    while cursor.pos < raw.len() {
        let record_offset = cursor.num(wide)?;
        let start = cursor.pos;
        while cursor.take(unit)?.iter().any(|&byte| byte != 0) {}
        bounded((cursor.pos - unit - start) as u64, MAX_KEY_BYTES, "dictionary key")?;
        let text = encoding.decode(&raw[start..cursor.pos - unit]);
        bounded(text.len() as u64, MAX_KEY_BYTES, "decoded dictionary key")?;
        let normalized = policy.normalize(&text);
        if keys.len() >= MAX_KEYS { bail!("dictionary has too many keys (limit {MAX_KEYS})"); }
        // Arc control blocks, physical keys, search ordinals and record starts.
        let mut allocation = std::mem::size_of::<Key>() + std::mem::size_of::<u32>() + std::mem::size_of::<u64>() + 16;
        allocation = allocation.checked_add(text.len()).ok_or_else(|| anyhow!("key size overflow"))?;
        if normalized != text { allocation = allocation.checked_add(normalized.len() + 16).ok_or_else(|| anyhow!("key size overflow"))?; }
        charge(budget, allocation)?;
        let text: Arc<str> = Arc::from(text);
        let normalized = if normalized.as_str() == text.as_ref() { text.clone() } else { Arc::from(normalized) };
        keys.try_reserve(1).context("allocate dictionary keys")?;
        keys.push(Key { text, normalized, record_offset });
    }
    Ok(())
}

fn parse_styles(text: &str) -> HashMap<String, (String, String)> {
    let mut styles = HashMap::new();
    let mut lines = text.split('\n').map(|line| line.trim_end_matches('\r'));
    while let Some(id) = lines.next() {
        let Some(open) = lines.next() else { break; };
        let Some(close) = lines.next() else { break; };
        let id = id.trim();
        if !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()) {
            styles.insert(id.to_string(), (open.to_string(), close.to_string()));
        }
    }
    styles
}

fn append_text(out: &mut String, text: &str) -> Result<()> {
    if out.len().checked_add(text.len()).filter(|&size| size <= MAX_RECORD_BYTES).is_none() { bail!("expanded entry exceeds the {MAX_RECORD_BYTES}-byte limit"); }
    out.try_reserve(text.len()).context("allocate expanded entry")?;
    out.push_str(text);
    Ok(())
}

fn append_styled(out: &mut String, text: &str, style: Option<&(String, String)>) -> Result<()> {
    let Some((open, close)) = style else { return append_text(out, text); };
    // Keep line endings outside the wrapper without trimming meaningful spaces.
    let (body, newline) = if let Some(body) = text.strip_suffix("\r\n") { (body, "\r\n") }
        else if let Some(body) = text.strip_suffix('\n') { (body, "\n") } else { (text, "") };
    append_text(out, open)?; append_text(out, body)?; append_text(out, close)?; append_text(out, newline)
}

fn expand_styles(text: &str, styles: &HashMap<String, (String, String)>) -> Result<String> {
    let mut out = String::new();
    let mut position = 0;
    let mut scan = 0;
    let mut active = None;
    let bytes = text.as_bytes();
    while let Some(relative) = text[scan..].find('`') {
        let start = scan + relative;
        let mut end = start + 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() { end += 1; }
        if end == start + 1 || bytes.get(end) != Some(&b'`') { scan = end; continue; }
        append_styled(&mut out, &text[position..start], active)?;
        active = styles.get(&text[start + 1..end]);
        if active.is_none() { append_text(&mut out, &text[start..=end])?; }
        position = end + 1;
        scan = position;
    }
    append_styled(&mut out, &text[position..], active)?;
    Ok(out)
}

#[cfg(test)]
#[path = "mdict_tests.rs"]
mod tests;
