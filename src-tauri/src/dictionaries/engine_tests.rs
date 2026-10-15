// Cross-module tests: real parsers -> persistent index -> Dictionary lookup/resources.
// All input is generated here; no downloaded dictionaries or Lookupper assets.
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicU64, AtomicUsize, Ordering}};

use crate::dict::{self, Dictionary};
use super::source;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("lkt-engine-{}-{time}-{sequence}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

fn archive(path: &Path, files: &[(&str, &[u8])]) {
    let mut archive = zip::ZipWriter::new(File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for &(name, bytes) in files {
        archive.start_file(name, options).unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap();
}

#[test]
fn dsl_lookup_preserves_homographs_aliases_candidate_precedence_and_dictionary_order() {
    let directory = Directory::new();
    let first = directory.0.join("first.dsl");
    let second = directory.0.join("second.dsl");
    fs::write(&first, "#NAME \"First\"\nrun\nrunning\n\t[b]move fast[/b]\nrun\n\toperate a machine\n").unwrap();
    fs::write(&second, "#NAME \"Second\"\nrun\n\ta sequence\n").unwrap();
    let cache = directory.0.join("cache");
    let one = Arc::new(Dictionary::open_cached("first", &first, &cache, &|_| {}).unwrap());
    let two = Arc::new(Dictionary::open_cached("second", &second, &cache, &|_| {}).unwrap());
    assert_eq!(one.len(), 2);
    let run = one.find("ＲＵＮ");
    assert_eq!(run.len(), 2, "NFKC matching must retain both homographs");
    assert_eq!(one.find("running"), vec![run[0]]);
    assert!(one.entry_html(run[0]).unwrap().contains("<b>move fast</b>"));
    let hits = dict::lookup(&vec![two.clone(), one.clone()], &["running".into(), "run".into()]);
    assert_eq!(hits.len(), 2, "first matching form wins independently in each dictionary");
    assert_eq!(hits[0].dict_id, "second");
    assert_eq!(hits[0].matched, "run");
    assert_eq!(hits[1].dict_id, "first");
    assert_eq!(hits[1].matched, "running");
    drop(hits); drop(one); drop(two);
    let reopened = Dictionary::open_cached("first", &first, &cache, &|_| panic!("warm open reimported DSL")).unwrap();
    assert_eq!(reopened.find("run").len(), 2);
}

#[test]
fn zipped_yomitan_is_indexed_with_reading_aliases_structured_resources_and_reused() {
    let directory = Directory::new();
    let source_zip = directory.0.join("study.zip");
    let index = br#"{"title":"Engine fixture","format":3,"revision":"one"}"#;
    let bank = r#"[["猫","ねこ","n","",1,["a cat",{"type":"image","path":"Images/cat.svg","alt":"cat"},{"type":"structured-content","content":{"tag":"a","href":"?query=犬","content":"dog"}}],1,""]]"#;
    archive(&source_zip, &[
        ("Nested/index.json", index), ("Nested/term_bank_1.json", bank.as_bytes()),
        ("Nested/styles.css", b".yomitan-entry{color:green}"),
        ("Nested/Images/cat.svg", b"<svg xmlns='http://www.w3.org/2000/svg'></svg>"),
        ("README.txt", b"synthetic archive"),
    ]);
    let original = fs::read(&source_zip).unwrap();
    let progress = AtomicUsize::new(0);
    let cache = directory.0.join("cache");
    let dictionary = Dictionary::open_cached("zip", &source_zip, &cache, &|entries| { progress.store(entries, Ordering::SeqCst); }).unwrap();
    assert_eq!(dictionary.name, "Engine fixture");
    assert_eq!(dictionary.format, "yomitan");
    assert_eq!(progress.load(Ordering::SeqCst), 1);
    let hits = dictionary.find("猫");
    assert_eq!(dictionary.find("ねこ"), hits);
    let html = dictionary.entry_html(hits[0]).unwrap();
    assert!(html.contains("a cat"));
    assert!(html.contains("Images/cat.svg"));
    assert!(html.contains("entry://%E7%8A%AC"));
    assert!(dictionary.stylesheets().contains(&"styles.css".to_string()));
    assert_eq!(dictionary.resource("\\images\\CAT.SVG"), Some(b"<svg xmlns='http://www.w3.org/2000/svg'></svg>".to_vec()));
    assert!(dictionary.resource("../study.zip").is_none());
    assert!(dictionary.resource("C:/Windows/win.ini").is_none());
    drop(dictionary);
    let reopened = Dictionary::open_cached("zip", &source_zip, &cache, &|_| panic!("warm ZIP reimported articles")).unwrap();
    assert_eq!(reopened.find("ねこ").len(), 1);
    assert_eq!(fs::read(&source_zip).unwrap(), original, "import must never change source archive");
}

#[test]
fn archive_extraction_rejects_traversal_and_windows_path_syntax() {
    let directory = Directory::new();
    let cache = directory.0.join("cache");
    for (ordinal, member) in ["../escaped.dsl", "/escaped.dsl", "C:/escaped.dsl", "folder\\escaped.dsl"].iter().enumerate() {
        let zip = directory.0.join(format!("unsafe-{ordinal}.zip"));
        archive(&zip, &[(member, b"word\n\tdefinition\n")]);
        assert!(source::prepare(&zip, &cache, &format!("unsafe-{ordinal}")).is_err(), "accepted unsafe ZIP member {member}");
    }
    assert!(!directory.0.join("escaped.dsl").exists());
}

#[test]
fn nested_resources_are_case_insensitive_and_stay_inside_the_dictionary_root() {
    let directory = Directory::new();
    let root = directory.0.join("Dictionary");
    fs::create_dir_all(root.join("Images")).unwrap();
    fs::write(root.join("Images/Word.PNG"), b"png fixture").unwrap();
    fs::write(directory.0.join("private.txt"), b"outside dictionary").unwrap();
    assert_eq!(source::resource(&root, "\\images\\word.png"), Some(b"png fixture".to_vec()));
    assert!(source::resource(&root, "../private.txt").is_none());
    assert!(source::resource(&root, "images/../../private.txt").is_none());
    assert!(source::resource(&root, "C:\\private.txt").is_none());
    assert!(source::resource(&root, "images/word.png\0.txt").is_none());
    #[cfg(unix)] {
        std::os::unix::fs::symlink(directory.0.join("private.txt"), root.join("outside.txt")).unwrap();
        assert!(source::resource(&root, "outside.txt").is_none());
    }
}

#[test]
fn a_complete_metadata_marker_cannot_hide_missing_sqlite_tables() {
    let directory = Directory::new();
    let source = directory.0.join("source.dsl");
    let cache = directory.0.join("cache");
    fs::write(&source, "#NAME \"Fixture\"\nword\n\tdefinition\n").unwrap();
    let dictionary = Dictionary::open_cached("broken", &source, &cache, &|_| {}).unwrap();
    assert_eq!(dictionary.find("word").len(), 1);
    drop(dictionary);
    let database = fs::read_dir(cache.join("indexes")).unwrap().map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "sqlite")).unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute_batch("DROP TABLE headwords;").unwrap();
    drop(connection);
    let recovered = Dictionary::open_cached("broken", &source, &cache, &|_| {}).unwrap();
    assert_eq!(recovered.find("word").len(), 1, "damaged complete indexes must rebuild");
}

#[test]
fn simultaneous_imports_publish_one_usable_index_without_renaming_a_live_winner() {
    let directory = Directory::new();
    let source = directory.0.join("source.dsl");
    let cache = directory.0.join("cache");
    let mut text = String::from("#NAME \"Concurrent fixture\"\n");
    for ordinal in 0..600 { text.push_str(&format!("word{ordinal}\n\tdefinition {ordinal}\n")); }
    fs::write(&source, text).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2).map(|_| {
        let source = source.clone();
        let cache = cache.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            // Synchronize callers, allowing the engine to serialize imports internally.
            barrier.wait();
            let dictionary = Dictionary::open_cached("same", &source, &cache, &|_| {}).unwrap();
            assert_eq!(dictionary.find("word599").len(), 1);
            dictionary
        })
    }).collect();
    let dictionaries: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    for dictionary in &dictionaries { assert_eq!(dictionary.find("word0").len(), 1); }
}
