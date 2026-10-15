// A versioned, persistent index for imported formats. SQLite owns the headword index and
// articles; a lookup reads only its matching rows, not the dictionary's complete word list.
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use super::{containers, folded, lexicons, slob, ImportSink};

const SCHEMA_VERSION: &str = "lookitan-dictionary-1";
const MAX_ARTICLE: usize = 16 * 1024 * 1024;
pub const MAX_HITS: usize = 128;
static PUBLISH: Mutex<()> = Mutex::new(());

pub struct Store {
    db: Mutex<Connection>,
    pub name: String,
    pub format: String,
    pub description: String,
    count: usize,
}

fn open_readonly(path: &Path, stamp: &str) -> Result<Store> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.execute_batch("PRAGMA cache_size=-512; PRAGMA mmap_size=0; PRAGMA query_only=ON;")?;
    let meta = |name: &str| -> Result<String> {
        Ok(db.query_row("SELECT value FROM metadata WHERE name=?1", [name], |r| r.get(0))?)
    };
    if meta("version")? != SCHEMA_VERSION || meta("source")? != stamp {
        bail!("outdated dictionary index");
    }
    let name = meta("name")?;
    let format = meta("format")?;
    let description = meta("description")?;
    let count: usize = meta("count")?.parse()?;
    // A complete marker alone does not make a damaged SQLite file usable.
    let tables: i64 = db.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('entries','headwords','resources')", [], |r| r.get(0))?;
    if tables != 3 || count == 0 { bail!("incomplete dictionary index"); }
    db.prepare("SELECT id,headword,html FROM entries LIMIT 1")?;
    db.prepare("SELECT word,entry FROM headwords LIMIT 1")?;
    db.prepare("SELECT name,reference FROM resources LIMIT 1")?;
    Ok(Store { db: Mutex::new(db), name, format, description, count })
}

struct Writer<'a> {
    db: &'a Connection,
    count: u32,
    progress: &'a dyn Fn(usize),
}

impl ImportSink for Writer<'_> {
    fn add(&mut self, headword: &str, html: &str, aliases: &[String]) -> Result<()> {
        if headword.trim().is_empty() { return Ok(()); }
        if html.len() > MAX_ARTICLE || headword.len() > 64 * 1024 {
            bail!("dictionary article exceeds the 16 MiB limit");
        }
        self.count = self.count.checked_add(1).ok_or_else(|| anyhow!("too many dictionary entries"))?;
        self.db.prepare_cached("INSERT INTO entries(id,headword,html) VALUES(?1,?2,?3)")?
            .execute(params![self.count, headword, html])?;
        let mut insert = self.db.prepare_cached("INSERT OR IGNORE INTO headwords(word,entry) VALUES(?1,?2)")?;
        for word in std::iter::once(headword).chain(aliases.iter().map(String::as_str)) {
            let key = folded(word);
            if !key.is_empty() { insert.execute(params![key, self.count])?; }
        }
        if self.count % 500 == 0 { (self.progress)(self.count as usize); }
        Ok(())
    }

    fn add_resource(&mut self, key: &str, reference: u32) -> Result<()> {
        let key = key.replace('\\', "/");
        self.db.prepare_cached("INSERT OR IGNORE INTO resources(name,reference) VALUES(?1,?2)")?
            .execute(params![key.trim_start_matches('/'), reference])?;
        Ok(())
    }
}

/// Remove only the fresh temporary index owned by this import; never the source dictionary.
struct Pending(PathBuf);
impl Drop for Pending {
    fn drop(&mut self) { let _ = fs::remove_file(&self.0); }
}

impl Store {
    pub fn open(source: &Path, cache: &Path, id: &str, progress: &dyn Fn(usize)) -> Result<Self> {
        fs::create_dir_all(cache)?;
        let stamp = super::source::fingerprint(source)?;
        let path = cache.join(format!("{}-{}-v1.sqlite", id, super::source::stamp_id(&stamp)));
        if path.is_file() {
            if let Ok(store) = open_readonly(&path, &stamp) { return Ok(store); }
        }
        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let pending = Pending(cache.join(format!(".{id}-{unique}.sqlite.tmp")));
        let db = Connection::open(&pending.0)?;
        db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA cache_size=-4096; PRAGMA temp_store=FILE;
            CREATE TABLE metadata(name TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE entries(id INTEGER PRIMARY KEY,headword TEXT NOT NULL,html TEXT NOT NULL);
            CREATE TABLE headwords(word TEXT NOT NULL,entry INTEGER NOT NULL,PRIMARY KEY(word,entry)) WITHOUT ROWID;
            CREATE TABLE resources(name TEXT PRIMARY KEY,reference INTEGER NOT NULL) WITHOUT ROWID;
            BEGIN IMMEDIATE;")?;
        let mut writer = Writer { db: &db, count: 0, progress };
        let info = if containers::supports(source) {
            containers::import(source, &mut writer)?
        } else if lexicons::supports(source) {
            lexicons::import(source, &mut writer)?
        } else if slob::supports(source) {
            slob::import(source, &mut writer)?
        } else {
            bail!("unsupported dictionary format: {}", source.display());
        };
        if writer.count == 0 { bail!("no dictionary entries found in {}", source.display()); }
        let count = writer.count.to_string();
        for (name, value) in [("version", SCHEMA_VERSION), ("source", stamp.as_str()),
            ("name", info.name.as_str()), ("format", info.format), ("description", info.description.as_str()), ("count", count.as_str())] {
            db.execute("INSERT INTO metadata VALUES(?1,?2)", params![name, value])?;
        }
        let count = writer.count as usize;
        drop(writer);
        db.execute_batch("COMMIT; PRAGMA optimize;")?;
        drop(db);
        let _publish = PUBLISH.lock();
        if path.is_file() {
            if let Ok(store) = open_readonly(&path, &stamp) { return Ok(store); }
        }
        // No source is overwritten. Preserve a damaged cache for diagnostics before rebuilding it.
        if path.exists() { fs::rename(&path, path.with_extension(format!("invalid-{unique}")))?; }
        fs::rename(&pending.0, &path).context("finish dictionary index")?;
        progress(count);
        open_readonly(&path, &stamp)
    }

    pub fn len(&self) -> usize { self.count }

    pub fn find(&self, text: &str) -> Result<Vec<u32>> {
        let db = self.db.lock();
        let mut query = db.prepare_cached("SELECT entry FROM headwords WHERE word=?1 ORDER BY entry LIMIT ?2")?;
        let rows = query.query_map(params![folded(text), MAX_HITS], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<u32>>>()?)
    }

    pub fn headword(&self, id: u32) -> Option<String> {
        self.db.lock().query_row("SELECT headword FROM entries WHERE id=?1", [id], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn record_text(&self, id: u32) -> Result<String> {
        Ok(self.db.lock().query_row("SELECT html FROM entries WHERE id=?1", [id], |r| r.get(0))?)
    }

    pub fn resource_reference(&self, path: &str) -> Option<u32> {
        let path = path.replace('\\', "/");
        self.db.lock().query_row("SELECT reference FROM resources WHERE name=?1", [path.trim_start_matches('/')], |r| r.get(0))
            .optional().ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_index_preserves_homographs_aliases_and_rebuilds_changed_sources() {
        let dir = std::env::temp_dir().join(format!("lkt-index-{}-{}", std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("test.dsl");
        fs::write(&source, "#NAME \"Fixture\"\nrun\nrunning\n\t[b]move[/b]\nrun\n\toperate\n").unwrap();
        let cache = dir.join("cache");
        let store = Store::open(&source, &cache, "test", &|_| {}).unwrap();
        let hits = store.find("ＲＵＮ").unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(store.find("running").unwrap(), vec![hits[0]]);
        assert!(store.record_text(hits[0]).unwrap().contains("move"));
        drop(store);
        let reopened = Store::open(&source, &cache, "test", &|_| panic!("cache was not reused")).unwrap();
        assert_eq!(reopened.len(), 2);
        drop(reopened);
        fs::write(&source, "#NAME \"Changed\"\nwalk\n\tgo on foot\n").unwrap();
        let changed = Store::open(&source, &cache, "test", &|_| {}).unwrap();
        assert!(changed.find("run").unwrap().is_empty());
        assert_eq!(changed.find("walk").unwrap().len(), 1);
        drop(changed);
        fs::remove_dir_all(&dir).unwrap();
    }
}
