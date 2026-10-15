// App-owned MDict key sidecars. Never caches entry HTML, record blocks or media.
// Every integer is little-endian; incompatible schemas use a new magic/version.
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use anyhow::{anyhow, bail, Context, Result};

use super::{charge, Key, MAX_INDEX_BYTES, MAX_KEYS, MAX_KEY_BYTES};

const MAGIC: &[u8; 8] = b"LKTMDX02";
const OFFSET: u64 = 0xcbf29ce484222325;
const PRIME: u64 = 0x100000001b3;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn hash(mut state: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes { state = (state ^ byte as u64).wrapping_mul(PRIME); }
    state
}

pub(super) struct Identity {
    path: String,
    size: u64,
    modified_seconds: u64,
    modified_nanos: u32,
    header_hash: u64,
    entries: usize,
}

impl Identity {
    pub(super) fn source(path: &Path, mapped_size: usize, header: &[u8], entries: usize) -> Result<Self> {
        let metadata = fs::metadata(path)?;
        if metadata.len() != mapped_size as u64 { bail!("dictionary size changed during open"); }
        let modified = metadata.modified()?.duration_since(UNIX_EPOCH)?;
        let path = fs::canonicalize(path)?.to_string_lossy().into_owned();
        if path.len() > 64 * 1024 { bail!("dictionary path too long for key cache"); }
        Ok(Self { path, size: metadata.len(), modified_seconds: modified.as_secs(), modified_nanos: modified.subsec_nanos(),
            header_hash: hash(OFFSET, header), entries })
    }

    pub(super) fn file(&self, directory: &Path) -> PathBuf {
        directory.join(format!("{:016x}.lki", hash(OFFSET, self.path.as_bytes())))
    }
}

pub(super) struct CachedIndex {
    pub(super) keys: Vec<Key>,
    pub(super) index: Vec<u32>,
    pub(super) offsets: Vec<u64>,
}

struct Reader { inner: BufReader<File>, hash: u64 }

impl Reader {
    fn bytes(&mut self, buffer: &mut [u8]) -> Result<()> {
        self.inner.read_exact(buffer)?;
        self.hash = hash(self.hash, buffer);
        Ok(())
    }
    fn u32(&mut self) -> Result<u32> { let mut bytes = [0; 4]; self.bytes(&mut bytes)?; Ok(u32::from_le_bytes(bytes)) }
    fn u64(&mut self) -> Result<u64> { let mut bytes = [0; 8]; self.bytes(&mut bytes)?; Ok(u64::from_le_bytes(bytes)) }
    fn text(&mut self, limit: usize, budget: &mut usize) -> Result<String> {
        let size = self.u32()? as usize;
        if size > limit { bail!("cached key/path exceeds limit"); }
        charge(budget, size)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).context("allocate cached key")?;
        bytes.resize(size, 0);
        self.bytes(&mut bytes)?;
        String::from_utf8(bytes).context("invalid cached UTF-8")
    }
}

pub(super) fn load(directory: &Path, identity: &Identity, record_size: u64) -> Option<CachedIndex> {
    let file = identity.file(directory);
    match read(&file, identity, record_size) {
        Ok(index) => { log::debug!("loaded MDict key cache {}", file.display()); Some(index) }
        Err(error) => { log::debug!("rebuild MDict key cache {}: {error:#}", file.display()); None }
    }
}

fn read(path: &Path, identity: &Identity, record_size: u64) -> Result<CachedIndex> {
    let file = File::open(path)?;
    if file.metadata()?.len() > (MAX_INDEX_BYTES + 1024 * 1024) as u64 { bail!("key cache too large"); }
    let mut reader = Reader { inner: BufReader::new(file), hash: OFFSET };
    let mut magic = [0; 8];
    reader.bytes(&mut magic)?;
    if &magic != MAGIC { bail!("unsupported key cache schema"); }
    let mut budget = 0;
    if reader.text(64 * 1024, &mut budget)? != identity.path
        || reader.u64()? != identity.size
        || reader.u64()? != identity.modified_seconds
        || reader.u32()? != identity.modified_nanos
        || reader.u64()? != identity.header_hash
        || reader.u32()? as usize != identity.entries { bail!("stale key cache identity"); }
    if identity.entries > MAX_KEYS { bail!("too many cached keys"); }
    // Include all fixed-size indexes and the Arc control blocks before reserving.
    let fixed = identity.entries.checked_mul(std::mem::size_of::<Key>() + 4 + 8 + 16).ok_or_else(|| anyhow!("cache index size overflow"))?;
    charge(&mut budget, fixed)?;
    let mut keys = Vec::new();
    keys.try_reserve_exact(identity.entries).context("allocate cached keys")?;
    for _ in 0..identity.entries {
        let record_offset = reader.u64()?;
        if record_offset > record_size { bail!("cached record offset out of range"); }
        let text: Arc<str> = Arc::from(reader.text(MAX_KEY_BYTES, &mut budget)?);
        let mut tag = [0];
        reader.bytes(&mut tag)?;
        let normalized = match tag[0] {
            0 => text.clone(),
            1 => { charge(&mut budget, 16)?; Arc::from(reader.text(MAX_KEY_BYTES * 3, &mut budget)?) }
            _ => bail!("invalid cached key tag"),
        };
        keys.push(Key { text, normalized, record_offset });
    }
    let mut index: Vec<u32> = Vec::new();
    index.try_reserve_exact(keys.len()).context("allocate cached search order")?;
    for _ in 0..keys.len() {
        let ordinal = reader.u32()?;
        if ordinal as usize >= keys.len() { bail!("cached key ordinal out of range"); }
        if let Some(&previous) = index.last() {
            let ordering = keys[previous as usize].normalized.cmp(&keys[ordinal as usize].normalized).then(previous.cmp(&ordinal));
            if !ordering.is_lt() { bail!("cached search order is not a strict permutation"); }
        }
        index.push(ordinal);
    }
    let count = reader.u32()? as usize;
    if count > keys.len() { bail!("too many cached record starts"); }
    let mut offsets: Vec<u64> = Vec::new();
    offsets.try_reserve_exact(count).context("allocate cached record starts")?;
    for _ in 0..count {
        let offset = reader.u64()?;
        if offset > record_size || offsets.last().is_some_and(|&previous| previous >= offset) { bail!("cached record starts are invalid"); }
        offsets.push(offset);
    }
    if keys.iter().any(|key| offsets.binary_search(&key.record_offset).is_err()) { bail!("cached record start missing"); }
    let mut checksum = [0; 8];
    reader.inner.read_exact(&mut checksum)?;
    if u64::from_le_bytes(checksum) != reader.hash { bail!("key cache checksum mismatch"); }
    let mut extra = [0];
    if reader.inner.read(&mut extra)? != 0 { bail!("trailing key cache data"); }
    Ok(CachedIndex { keys, index, offsets })
}

struct Writer { inner: BufWriter<File>, hash: u64 }

impl Writer {
    fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.inner.write_all(bytes)?;
        self.hash = hash(self.hash, bytes);
        Ok(())
    }
    fn u32(&mut self, value: u32) -> Result<()> { self.bytes(&value.to_le_bytes()) }
    fn u64(&mut self, value: u64) -> Result<()> { self.bytes(&value.to_le_bytes()) }
    fn text(&mut self, text: &str) -> Result<()> {
        self.u32(u32::try_from(text.len()).context("cached string too long")?)?;
        self.bytes(text.as_bytes())
    }
}

pub(super) fn save(directory: &Path, identity: &Identity, keys: &[Key], index: &[u32], offsets: &[u64]) {
    if let Err(error) = write(directory, identity, keys, index, offsets) { log::warn!("could not save MDict key cache: {error:#}"); }
}

fn write(directory: &Path, identity: &Identity, keys: &[Key], index: &[u32], offsets: &[u64]) -> Result<()> {
    fs::create_dir_all(directory)?;
    let destination = identity.file(directory);
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = destination.with_extension(format!("{}.{}.tmp", std::process::id(), sequence));
    let file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    let result = (|| {
        let mut writer = Writer { inner: BufWriter::new(file), hash: OFFSET };
        writer.bytes(MAGIC)?;
        writer.text(&identity.path)?;
        writer.u64(identity.size)?;
        writer.u64(identity.modified_seconds)?;
        writer.u32(identity.modified_nanos)?;
        writer.u64(identity.header_hash)?;
        writer.u32(identity.entries as u32)?;
        for key in keys {
            writer.u64(key.record_offset)?;
            writer.text(&key.text)?;
            if Arc::ptr_eq(&key.text, &key.normalized) { writer.bytes(&[0])?; }
            else { writer.bytes(&[1])?; writer.text(&key.normalized)?; }
        }
        for &ordinal in index { writer.u32(ordinal)?; }
        writer.u32(offsets.len() as u32)?;
        for &offset in offsets { writer.u64(offset)?; }
        writer.inner.write_all(&writer.hash.to_le_bytes())?;
        writer.inner.flush()?;
        writer.inner.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&temporary, &destination)?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&temporary); }
    result
}
