// Word audio from a local audio database, the format of local-audio-yomichan (MIT, the Anki
// add-on "Local Audio Server for Yomichan"), which Yomitan's local audio and Android apps read.
// Behavior only, no code taken. Two variants of the same SQLite file:
//   entries.db   table `entries (expression, reading, source, speaker, display, file)`; `file` is a
//                path under the source's folder next to the database, `<source>_files/<file>`
//   android.db   the same `entries`, plus `android (file, source, data BLOB)` holding the audio itself
// The file is opened read-only and never changed.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Serialize;

/// local-audio-yomichan's default source order (its default_config.json) and the names it shows,
/// with `%s` replaced by the row's `display`.
const SOURCES: &[(&str, &str)] = &[
    ("nhk16", "NHK16 %s"),
    ("shinmeikai8", "SMK8 %s"),
    ("forvo", "Forvo (%s)"),
    ("jpod", "JPod101"),
    ("jpod_alternate", "JPod101 Alt"),
    ("ozk5", "OZK5 %s"),
];

/// At most this many recordings per word, like a Yomitan audio menu.
const MAX_ENTRIES: usize = 30;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct AudioEntry {
    pub source: String,
    pub file: String,
    /// What the pop-up's audio menu shows, e.g. "NHK16 ヨム [1]" or "Forvo (speaker)".
    pub name: String,
}

pub struct AudioDb {
    dir: PathBuf,
    conn: Mutex<Connection>,
    /// The android.db variant, with the audio inside.
    blobs: bool,
}

fn display_name(source: &str, display: Option<&str>) -> String {
    let display = display.map(str::trim).filter(|d| !d.is_empty());
    match SOURCES.iter().find(|(id, _)| *id == source) {
        Some((_, pattern)) => match (pattern.contains("%s"), display) {
            (true, Some(d)) => pattern.replace("%s", d),
            (true, None) => pattern.replace("%s", "").replace(" ()", "").trim().to_string(),
            (false, _) => (*pattern).to_string(),
        },
        None => match display {
            Some(d) => format!("{source} {d}"),
            None => source.to_string(),
        },
    }
}

/// A relative path that stays inside its folder: no root, drive or `..`.
fn safe_relative(file: &str) -> Option<PathBuf> {
    let path = Path::new(file);
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

impl AudioDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .with_context(|| format!("não foi possível abrir {}", path.display()))?;
        let has = |table: &str| -> Result<bool> {
            Ok(conn.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |_| Ok(()))
                .optional()?.is_some())
        };
        if !has("entries")? {
            anyhow::bail!("{} não é um banco de áudio local (falta a tabela entries)", path.display());
        }
        let blobs = has("android")?;
        Ok(Self { dir: path.parent().map(Path::to_path_buf).unwrap_or_default(), conn: Mutex::new(conn), blobs })
    }

    /// The recordings of a word, best source first. Without a reading every reading matches; with
    /// one, rows for that reading and rows without any reading match, as local-audio-yomichan does.
    pub fn lookup(&self, term: &str, reading: Option<&str>) -> Result<Vec<AudioEntry>> {
        let term = term.trim();
        if term.is_empty() { return Ok(Vec::new()); }
        let reading = reading.map(str::trim).filter(|r| !r.is_empty());
        let order = SOURCES.iter().enumerate()
            .map(|(i, (id, _))| format!("WHEN '{id}' THEN {i}")).collect::<Vec<_>>().join(" ");
        let sql = format!(
            "SELECT source, file, display FROM entries \
             WHERE expression = ?1 AND (?2 IS NULL OR reading IS NULL OR reading = ?2) \
             ORDER BY CASE source {order} ELSE {} END, reading LIMIT {MAX_ENTRIES}",
            SOURCES.len());
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![term, reading], |row| {
            let source: String = row.get(0)?;
            let display: Option<String> = row.get(2)?;
            Ok(AudioEntry { name: display_name(&source, display.as_deref()), source, file: row.get(1)? })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// One recording's bytes: from the `android` table, or from `<source>_files/<file>` next to the
    /// database (local-audio-yomichan's folder layout).
    pub fn data(&self, source: &str, file: &str) -> Result<Option<Vec<u8>>> {
        if self.blobs {
            let conn = self.conn.lock();
            let found = conn.query_row("SELECT data FROM android WHERE file = ?1 AND source = ?2 LIMIT 1",
                params![file, source], |row| row.get::<_, Vec<u8>>(0)).optional()?;
            if found.is_some() { return Ok(found); }
        }
        let (Some(rel), Some(folder)) = (safe_relative(file), safe_relative(&format!("{source}_files"))) else {
            return Ok(None);
        };
        let path = self.dir.join(folder).join(rel);
        Ok(std::fs::read(&path).ok())
    }
}

/// The audio file extension for Anki's media name, from the stored file name.
pub fn extension(file: &str) -> &'static str {
    match file.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "ogg" | "oga" | "opus" => "ogg",
        "m4a" => "m4a",
        "aac" => "aac",
        "flac" => "flac",
        "wav" => "wav",
        _ => "mp3",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(dir: &Path, android: bool) -> PathBuf {
        let path = dir.join(if android { "android.db" } else { "entries.db" });
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (id integer PRIMARY KEY NOT NULL, expression text NOT NULL, reading text,
                source text NOT NULL, speaker text, display text, file text NOT NULL);
             INSERT INTO entries (expression, reading, source, display, file) VALUES
                ('読む', 'よむ', 'jpod', NULL, 'よむ - 読む.mp3'),
                ('読む', 'よむ', 'nhk16', 'ヨム [1]', 'audio/yomu.ogg'),
                ('読む', NULL, 'forvo', 'kyokotokyojapan', 'kyokotokyojapan/読む.mp3'),
                ('読む', 'とく', 'shinmeikai8', 'トク', 'media/toku.ogg'),
                ('読む', 'よむ', 'custom', NULL, 'x.mp3');").unwrap();
        if android {
            conn.execute_batch("CREATE TABLE android (id integer PRIMARY KEY NOT NULL, file text NOT NULL, source text NOT NULL, data blob NOT NULL);").unwrap();
            conn.execute("INSERT INTO android (file, source, data) VALUES ('audio/yomu.ogg', 'nhk16', ?1)", [b"OggS".to_vec()]).unwrap();
        }
        path
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lookitan-audio-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn recordings_come_in_local_audio_order_and_match_the_reading() {
        let dir = temp("order");
        let db = AudioDb::open(&sample(&dir, false)).unwrap();
        let all = db.lookup("読む", None).unwrap();
        let sources: Vec<&str> = all.iter().map(|e| e.source.as_str()).collect();
        assert_eq!(sources, ["nhk16", "shinmeikai8", "forvo", "jpod", "custom"]);
        assert_eq!(all[0].name, "NHK16 ヨム [1]");
        assert_eq!(all[2].name, "Forvo (kyokotokyojapan)");
        assert_eq!(all[3].name, "JPod101");
        assert_eq!(all[4].name, "custom");
        let yomu: Vec<String> = db.lookup("読む", Some("よむ")).unwrap().into_iter().map(|e| e.source).collect();
        assert_eq!(yomu, ["nhk16", "forvo", "jpod", "custom"], "another reading is left out, rows without one stay");
        assert!(db.lookup("書く", None).unwrap().is_empty());
        assert!(db.lookup("  ", None).unwrap().is_empty());
    }

    #[test]
    fn audio_comes_from_the_android_table_or_from_the_source_folder() {
        let dir = temp("data");
        let db = AudioDb::open(&sample(&dir, true)).unwrap();
        assert_eq!(db.data("nhk16", "audio/yomu.ogg").unwrap(), Some(b"OggS".to_vec()));
        std::fs::create_dir_all(dir.join("jpod_files")).unwrap();
        std::fs::write(dir.join("jpod_files").join("よむ - 読む.mp3"), b"ID3").unwrap();
        assert_eq!(db.data("jpod", "よむ - 読む.mp3").unwrap(), Some(b"ID3".to_vec()), "files beside the database");
        assert_eq!(db.data("jpod", "../android.db").unwrap(), None, "never outside the source folder");
        assert_eq!(db.data("../x", "a.mp3").unwrap(), None);
        assert_eq!(db.data("forvo", "missing.mp3").unwrap(), None);
    }

    #[test]
    fn a_file_without_the_entries_table_is_refused() {
        let dir = temp("refuse");
        let path = dir.join("other.db");
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE terms (term text);").unwrap();
        assert!(AudioDb::open(&path).is_err());
        assert_eq!(extension("a/b.OGG"), "ogg");
        assert_eq!(extension("x"), "mp3");
    }
}
