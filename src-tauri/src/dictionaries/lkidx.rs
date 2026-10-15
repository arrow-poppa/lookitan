//! Sidecar headword index (`.lkidx`), the model Lookupper uses: the dictionary's own files stay
//! as they are and are read at lookup time; the index only records, for every headword, which
//! entry it names and where that entry's bytes are in the original files.
//!
//! Layout (little-endian):
//!   magic `LKIDX` + version u8 + 2 reserved bytes
//!   format id, title, description            (u32 length + UTF-8 each)
//!   source stamp: u32 file count, then per file: path relative to the dictionary folder
//!                 (u32 length + UTF-8), u64 size, u64 modification time in nanoseconds
//!   entries: u32 count, then per entry 24 bytes: u64 offset, u32 length, u16 file, u16 kind,
//!            u32 title start, u32 title length (into the title buffer)
//!   title buffer: u32 length + UTF-8
//!   headwords, as Lookupper's HeadwordIndex: u32 count, u32 byte length, the UTF-8 words
//!            (folded, sorted), u32 offsets[count + 1] into them, u32 entry ids[count]
//!
//! A missing, damaged or stale index (any source file changed) is rebuilt from the sources.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, ensure, Context, Result};
use memmap2::Mmap;

use super::folded;

const MAGIC: &[u8; 5] = b"LKIDX";
const VERSION: u8 = 1;
const ENTRY_BYTES: usize = 24;
/// Upper bounds that keep a damaged index from allocating without limit.
const MAX_ENTRIES: usize = 50_000_000;
const MAX_KEY_BYTES: usize = 1024;

/// Where an entry's bytes are: `length` bytes at `offset` of source file `file`; `kind` is the
/// reader's own tag (for Yomitan: term, kanji, term meta, kanji meta).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    pub file: u16,
    pub kind: u16,
    pub offset: u64,
    pub length: u32,
}

/// The files an index was built from, relative to the dictionary folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp(Vec<(String, u64, u64)>);

impl Stamp {
    pub fn of(root: &Path, files: &[PathBuf]) -> Result<Self> {
        let mut out = Vec::with_capacity(files.len());
        for file in files {
            let meta = fs::metadata(root.join(file)).with_context(|| format!("{}", file.display()))?;
            let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64).unwrap_or(0);
            out.push((file.to_string_lossy().replace('\\', "/"), meta.len(), modified));
        }
        Ok(Self(out))
    }

    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(name, _, _)| name.as_str())
    }
}

pub struct Builder {
    entries: Vec<(Location, u32, u32)>,
    titles: String,
    keys: Vec<(String, u32)>,
}

impl Builder {
    pub fn new() -> Self {
        Self { entries: Vec::new(), titles: String::new(), keys: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Adds an entry titled `title`, found under `title` and each of `aliases`.
    pub fn add(&mut self, title: &str, location: Location, aliases: &[&str]) -> Result<u32> {
        ensure!(self.entries.len() < MAX_ENTRIES, "dictionary has too many entries");
        let id = self.entries.len() as u32;
        let start = self.titles.len() as u32;
        self.titles.push_str(title);
        self.entries.push((location, start, title.len() as u32));
        for word in std::iter::once(title).chain(aliases.iter().copied()) {
            let key = folded(word);
            if !key.is_empty() && key.len() <= MAX_KEY_BYTES && !self.keys.iter().rev().take(4).any(|(k, i)| *i == id && *k == key) {
                self.keys.push((key, id));
            }
        }
        Ok(id)
    }

    /// Writes the index next to the dictionary, atomically: a reader never sees half a file.
    pub fn write(mut self, path: &Path, format: &str, title: &str, description: &str, stamp: &Stamp) -> Result<()> {
        self.keys.sort_unstable();
        self.keys.dedup();
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let tmp = path.with_extension(format!("lkidx.{}-{unique}.tmp", std::process::id()));
        let result = (|| -> Result<()> {
            let mut w = BufWriter::new(File::create(&tmp)?);
            w.write_all(MAGIC)?;
            w.write_all(&[VERSION, 0, 0])?;
            for text in [format, title, description] {
                put_str(&mut w, text)?;
            }
            w.write_all(&(stamp.0.len() as u32).to_le_bytes())?;
            for (name, size, modified) in &stamp.0 {
                put_str(&mut w, name)?;
                w.write_all(&size.to_le_bytes())?;
                w.write_all(&modified.to_le_bytes())?;
            }
            w.write_all(&(self.entries.len() as u32).to_le_bytes())?;
            for (loc, start, len) in &self.entries {
                w.write_all(&loc.offset.to_le_bytes())?;
                w.write_all(&loc.length.to_le_bytes())?;
                w.write_all(&loc.file.to_le_bytes())?;
                w.write_all(&loc.kind.to_le_bytes())?;
                w.write_all(&start.to_le_bytes())?;
                w.write_all(&len.to_le_bytes())?;
            }
            put_str(&mut w, &self.titles)?;
            let words: usize = self.keys.iter().map(|(k, _)| k.len()).sum();
            ensure!(words <= u32::MAX as usize, "dictionary headwords exceed 4 GiB");
            w.write_all(&(self.keys.len() as u32).to_le_bytes())?;
            w.write_all(&(words as u32).to_le_bytes())?;
            for (key, _) in &self.keys {
                w.write_all(key.as_bytes())?;
            }
            let mut at = 0u32;
            w.write_all(&at.to_le_bytes())?;
            for (key, _) in &self.keys {
                at += key.len() as u32;
                w.write_all(&at.to_le_bytes())?;
            }
            for (_, id) in &self.keys {
                w.write_all(&id.to_le_bytes())?;
            }
            w.into_inner().map_err(|e| anyhow!("{e}"))?.sync_all()?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        if fs::rename(&tmp, path).is_err() {
            // Windows does not rename over a file in some states; replace it in two steps.
            let _ = fs::remove_file(path);
            if let Err(e) = fs::rename(&tmp, path) {
                let _ = fs::remove_file(&tmp);
                return Err(e).context("save dictionary index");
            }
        }
        Ok(())
    }
}

fn put_str(w: &mut impl Write, text: &str) -> Result<()> {
    w.write_all(&(text.len() as u32).to_le_bytes())?;
    w.write_all(text.as_bytes())?;
    Ok(())
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len()).ok_or_else(|| anyhow!("dictionary index is truncated"))?;
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    fn str(&mut self) -> Result<&'a str> {
        let n = self.u32()? as usize;
        Ok(std::str::from_utf8(self.take(n)?)?)
    }
}

/// An opened index. It stays memory-mapped: lookups read only the pages they touch.
pub struct Index {
    map: Mmap,
    pub format: String,
    pub title: String,
    pub description: String,
    pub stamp: Stamp,
    entries: usize,
    entries_at: usize,
    titles_at: usize,
    words_count: usize,
    words_at: usize,
    offsets_at: usize,
    ids_at: usize,
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

impl Index {
    /// Opens `path` if it was built for `format` from exactly `stamp`'s files.
    pub fn open(path: &Path, format: &str, stamp: &Stamp) -> Result<Self> {
        let file = File::open(path)?;
        // SAFETY: the index belongs to Lookitan and is replaced only by rename, never in place.
        let map = unsafe { Mmap::map(&file)? };
        let mut c = Cursor { data: &map, at: 0 };
        ensure!(c.take(5)? == MAGIC && c.take(3)?[0] == VERSION, "not a Lookitan dictionary index");
        let (found_format, title, description) = (c.str()?.to_string(), c.str()?.to_string(), c.str()?.to_string());
        ensure!(found_format == format, "dictionary index is for another format");
        let files = c.u32()? as usize;
        ensure!(files <= u16::MAX as usize + 1, "dictionary index is damaged");
        let mut stamped = Vec::with_capacity(files);
        for _ in 0..files {
            stamped.push((c.str()?.to_string(), c.u64()?, c.u64()?));
        }
        let found_stamp = Stamp(stamped);
        ensure!(&found_stamp == stamp, "dictionary files changed since they were indexed");
        let entries = c.u32()? as usize;
        ensure!(entries <= MAX_ENTRIES, "dictionary index is damaged");
        let entries_at = c.at;
        c.take(entries.checked_mul(ENTRY_BYTES).ok_or_else(|| anyhow!("dictionary index is damaged"))?)?;
        let titles_len = c.u32()? as usize;
        let titles_at = c.at;
        std::str::from_utf8(c.take(titles_len)?)?;
        let words_count = c.u32()? as usize;
        let words_len = c.u32()? as usize;
        ensure!(words_count <= MAX_ENTRIES * 64, "dictionary index is damaged");
        let words_at = c.at;
        c.take(words_len)?;
        let offsets_at = c.at;
        c.take((words_count + 1) * 4)?;
        let ids_at = c.at;
        c.take(words_count * 4)?;
        let index = Self { map, format: found_format, title, description, stamp: found_stamp, entries, entries_at,
            titles_at, words_count, words_at, offsets_at, ids_at };
        // The arrays must describe the buffers they point into.
        let mut previous = 0;
        for i in 0..=words_count {
            let at = read_u32(&index.map, offsets_at + i * 4) as usize;
            ensure!(at >= previous && at <= words_len, "dictionary headword index is damaged");
            previous = at;
        }
        for i in 0..words_count {
            ensure!((read_u32(&index.map, ids_at + i * 4) as usize) < entries, "dictionary headword index is damaged");
        }
        for i in 0..entries {
            let at = entries_at + i * ENTRY_BYTES + 16;
            let (start, len) = (read_u32(&index.map, at) as usize, read_u32(&index.map, at + 4) as usize);
            ensure!(start.checked_add(len).is_some_and(|end| end <= titles_len), "dictionary entry titles are damaged");
        }
        Ok(index)
    }

    pub fn len(&self) -> usize {
        self.entries
    }

    fn word(&self, i: usize) -> &[u8] {
        let start = read_u32(&self.map, self.offsets_at + i * 4) as usize;
        let end = read_u32(&self.map, self.offsets_at + (i + 1) * 4) as usize;
        &self.map[self.words_at + start..self.words_at + end]
    }

    /// The ids of every entry found under `text` (folded), in index order, at most `limit`.
    pub fn find(&self, text: &str, limit: usize) -> Vec<u32> {
        let key = folded(text);
        if key.is_empty() {
            return Vec::new();
        }
        let key = key.as_bytes();
        let (mut lo, mut hi) = (0, self.words_count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.word(mid) < key { lo = mid + 1 } else { hi = mid }
        }
        let mut out = Vec::new();
        while lo < self.words_count && self.word(lo) == key && out.len() < limit {
            out.push(read_u32(&self.map, self.ids_at + lo * 4));
            lo += 1;
        }
        out
    }

    pub fn location(&self, id: u32) -> Option<Location> {
        let i = id as usize;
        if i >= self.entries {
            return None;
        }
        let at = self.entries_at + i * ENTRY_BYTES;
        let m = &self.map;
        Some(Location {
            offset: u64::from_le_bytes(m[at..at + 8].try_into().ok()?),
            length: read_u32(m, at + 8),
            file: u16::from_le_bytes(m[at + 12..at + 14].try_into().ok()?),
            kind: u16::from_le_bytes(m[at + 14..at + 16].try_into().ok()?),
        })
    }

    pub fn title(&self, id: u32) -> Option<&str> {
        let i = id as usize;
        if i >= self.entries {
            return None;
        }
        let at = self.entries_at + i * ENTRY_BYTES + 16;
        let (start, len) = (read_u32(&self.map, at) as usize, read_u32(&self.map, at + 4) as usize);
        std::str::from_utf8(&self.map[self.titles_at + start..self.titles_at + start + len]).ok()
    }
}

/// Reads `location`'s bytes from its source file.
pub fn read(root: &Path, stamp: &Stamp, location: Location, max: usize) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let name = stamp.files().nth(location.file as usize).ok_or_else(|| anyhow!("dictionary entry names an unknown file"))?;
    ensure!(location.length as usize <= max, "dictionary entry is too large");
    let mut file = File::open(root.join(name))?;
    file.seek(SeekFrom::Start(location.offset))?;
    let mut bytes = vec![0; location.length as usize];
    file.read_exact(&mut bytes).context("dictionary entry lies past the end of its file")?;
    Ok(bytes)
}

/// Opens the index at `path`, or builds it with `build` and opens the result.
pub fn open_or_build(path: &Path, format: &str, stamp: &Stamp,
    build: impl FnOnce() -> Result<(Builder, String, String)>) -> Result<Index> {
    match Index::open(path, format, stamp) {
        Ok(index) => return Ok(index),
        Err(e) if path.exists() => log::info!("re-indexing {}: {e:#}", path.display()),
        Err(_) => {}
    }
    let (builder, title, description) = build()?;
    if builder.len() == 0 {
        bail!("no dictionary entries found");
    }
    builder.write(path, format, &title, &description, stamp)?;
    Index::open(path, format, stamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trips_and_goes_stale_when_a_source_changes() {
        let dir = std::env::temp_dir().join(format!("lkt-lkidx-{}-{}", std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "cat: a small animal\nrun: to move fast\n").unwrap();
        let files = vec![PathBuf::from("a.txt")];
        let stamp = Stamp::of(&dir, &files).unwrap();
        let mut b = Builder::new();
        b.add("Cat", Location { file: 0, kind: 0, offset: 0, length: 20 }, &[]).unwrap();
        b.add("run", Location { file: 0, kind: 1, offset: 20, length: 18 }, &["running", "ran"]).unwrap();
        b.add("run", Location { file: 0, kind: 1, offset: 20, length: 3 }, &[]).unwrap();
        let path = dir.join("a.txt.lkidx");
        b.write(&path, "test", "Test", "A test", &stamp).unwrap();

        let index = Index::open(&path, "test", &stamp).unwrap();
        assert_eq!((index.len(), index.title.as_str(), index.words_count), (3, "Test", 5), "cat, ran, run twice, running");
        assert_eq!(index.find("CAT", 10), vec![0]);
        assert_eq!(index.find("run", 10), vec![1, 2], "homographs keep their order");
        assert_eq!(index.find("Running", 10), vec![1]);
        assert!(index.find("walk", 10).is_empty());
        assert_eq!(index.title(1), Some("run"));
        let loc = index.location(1).unwrap();
        assert_eq!(read(&dir, &index.stamp, loc, 1024).unwrap(), b"run: to move fast\n");
        assert!(Index::open(&path, "other", &stamp).is_err());
        // Windows refuses to rewrite a file while it is mapped.
        drop(index);

        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(dir.join("a.txt"), "changed\n").unwrap();
        let changed = Stamp::of(&dir, &files).unwrap();
        assert!(Index::open(&path, "test", &changed).is_err(), "a changed source makes the index stale");
        fs::write(&path, b"LKIDX\x01\0\0garbage").unwrap();
        assert!(Index::open(&path, "test", &stamp).is_err(), "a damaged index is rejected");
        fs::remove_dir_all(&dir).unwrap();
    }
}
