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
fn zipped_yomitan_is_unpacked_read_in_place_and_its_index_reused() {
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
    // As in Lookupper: the archive is unpacked into the app's content folder, then read from there.
    let installed = source::install(&source_zip, &directory.0.join("content"), "zip", &|_| {}).unwrap();
    assert_eq!(installed, directory.0.join("content").join("zip"));
    assert!(installed.join("Nested/term_bank_1.json").is_file(), "the bank stays as it was in the archive");
    let dictionary = Dictionary::open_cached("zip", &installed, &cache, &|entries| { progress.store(entries, Ordering::SeqCst); }).unwrap();
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
    assert!(installed.join("Nested/yomitan.lkidx").is_file(), "the headword index sits beside the banks");
    assert!(!cache.join("indexes").exists(), "nothing is copied into a database");
    drop(dictionary);
    let reopened = Dictionary::open_cached("zip", &installed, &cache, &|_| panic!("warm open re-indexed the banks")).unwrap();
    assert_eq!(reopened.find("ねこ").len(), 1);
    assert_eq!(fs::read(&source_zip).unwrap(), original, "import must never change source archive");
    assert!(source::install(&source_zip, &directory.0.join("content"), "zip", &|_| {}).is_err(), "installing twice is refused");
    assert!(source::is_installed_copy(&installed, &directory.0.join("content")));
    assert!(!source::is_installed_copy(&directory.0, &directory.0.join("content")));
}

#[test]
fn archive_extraction_rejects_traversal_and_windows_path_syntax() {
    let directory = Directory::new();
    let content = directory.0.join("content");
    for (ordinal, member) in ["../escaped.dsl", "/escaped.dsl", "C:/escaped.dsl", "folder\\escaped.dsl"].iter().enumerate() {
        let zip = directory.0.join(format!("unsafe-{ordinal}.zip"));
        archive(&zip, &[(member, b"word\n\tdefinition\n")]);
        assert!(source::install(&zip, &content, &format!("unsafe-{ordinal}"), &|_| {}).is_err(), "accepted unsafe ZIP member {member}");
        assert!(!content.join(format!("unsafe-{ordinal}")).exists(), "a refused archive leaves nothing installed");
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

#[test]
fn lookupper_style_archives_are_recognized_like_lookupper_does() {
    let directory = Directory::new();
    let content = directory.0.join("content");
    let cache = directory.0.join("cache");
    // GCIDE's distribution: CIDE.A–Z beside text files that are not dictionaries.
    let gcide = directory.0.join("gcide-0.53.zip");
    archive(&gcide, &[
        ("gcide-0.53/CIDE.A", b"<p><ent>Aback</ent><br/\n<hw>A*back\"</hw> <def>Toward the back.</def></p>\n"),
        ("gcide-0.53/CIDE.B", b"<p><ent>Bee</ent><br/\n<hw>Bee</hw> <def>An insect.</def></p>\n"),
        ("gcide-0.53/webfont.txt", b"WEBSTER FONTS"), ("gcide-0.53/README", b"read me"),
    ]);
    let installed = source::install(&gcide, &content, "gcide", &|_| {}).unwrap();
    let dictionary = Dictionary::open_cached("gcide", &installed, &cache, &|_| {}).unwrap();
    assert_eq!((dictionary.format.as_str(), dictionary.len()), ("gcide", 2));
    assert!(dictionary.entry_html(dictionary.find("bee")[0]).unwrap().contains("An insect."));
    assert!(installed.join("gcide-0.53/CIDE.A.lkidx").is_file(), "the index sits beside the CIDE files");

    // jmdict-simplified's release: one .json named by version.
    let jmdict = directory.0.join("jmdict-eng.zip");
    archive(&jmdict, &[("jmdict-eng-3.6.2.json", br#"{"version":"3.6.2","languages":["eng"],"dictDate":"2026-10-12","tags":{"n":"noun (common)"},"words":[{"id":"1","kanji":[{"common":true,"text":"\u732b","tags":[]}],"kana":[{"common":true,"text":"\u306d\u3053","tags":[],"appliesToKanji":["*"]}],"sense":[{"partOfSpeech":["n"],"appliesToKanji":["*"],"appliesToKana":["*"],"related":[],"antonym":[],"field":[],"dialect":[],"misc":[],"info":[],"languageSource":[],"gloss":[{"lang":"eng","gender":null,"type":null,"text":"cat"}]}]}]}"#)]);
    let installed = source::install(&jmdict, &content, "jmdict", &|_| {}).unwrap();
    let dictionary = Dictionary::open_cached("jmdict", &installed, &cache, &|_| {}).unwrap();
    assert_eq!(dictionary.format, "jmdict-simplified-json");
    let html = dictionary.entry_html(dictionary.find("ねこ")[0]).unwrap();
    assert!(html.contains("noun (common)") && html.contains("cat"), "{html}");
}

/// The published dictionaries themselves, when the workflow has downloaded them into
/// `LOOKITAN_REAL_DICTIONARIES` (one folder per dictionary, as an install unpacks them).
#[test]
fn real_published_dictionaries_are_indexed_and_read() {
    let Ok(root) = std::env::var("LOOKITAN_REAL_DICTIONARIES") else { return };
    let root = PathBuf::from(root);
    let cache = root.join("cache");
    for (folder, format, word, expected) in [
        ("jmdict", "jmdict-simplified-json", "猫", "cat"),
        ("kanjidic", "kanjidic2-json", "猫", "cat"),
        ("wordnet", "wordnet-lmf-xml", "cat", "feline mammal"),
        ("webster", "webster-text", "aback", "Toward the back"),
        ("gcide", "gcide", "aback", "Toward the back or rear"),
    ] {
        let started = std::time::Instant::now();
        let dictionary = Dictionary::open_cached(folder, &root.join(folder), &cache, &|_| {}).unwrap();
        let built = started.elapsed();
        assert_eq!(dictionary.format, format, "{folder}");
        let hits = dictionary.find(word);
        assert!(!hits.is_empty(), "{folder}: {word} not found");
        let html: String = hits.iter().map(|&h| dictionary.entry_html(h).unwrap()).collect();
        assert!(html.contains(expected), "{folder}: {word} lacks {expected:?}: {}", html.chars().take(2000).collect::<String>());
        drop(dictionary);
        let started = std::time::Instant::now();
        let reopened = Dictionary::open_cached(folder, &root.join(folder), &cache, &|_| panic!("{folder}: the index was rebuilt")).unwrap();
        println!("{folder}: {} entries, indexed in {built:?}, reopened in {:?}", reopened.len(), started.elapsed());
    }
}

#[test]
fn a_damaged_lkidx_is_rebuilt_and_a_dsl_is_read_where_it_is() {
    let directory = Directory::new();
    let source = directory.0.join("words.dsl");
    let cache = directory.0.join("cache");
    fs::write(&source, "#NAME \"Fixture\"\nword\n\tdefinition\n").unwrap();
    let dictionary = Dictionary::open_cached("lkidx", &source, &cache, &|_| {}).unwrap();
    assert_eq!(dictionary.format, "dsl");
    assert!(dictionary.entry_html(dictionary.find("word")[0]).unwrap().contains("definition"));
    drop(dictionary);
    assert!(!directory.0.join("words.dsl.lkidx").exists(), "the user's folder is not written to");
    let index = cache.join("indexes").join("lkidx.words.dsl.lkidx");
    assert!(index.is_file());
    fs::write(&index, b"LKIDX damaged").unwrap();
    let recovered = Dictionary::open_cached("lkidx", &source, &cache, &|_| {}).unwrap();
    assert_eq!(recovered.find("word").len(), 1, "a damaged index is rebuilt");
}

#[test]
fn batch_import_rolls_back_on_a_bad_dictionary_and_preserves_earlier_installs() {
    let directory = Directory::new();
    let cache = directory.0.join("cache");
    let content = cache.join("content");
    let source_zip = directory.0.join("batch.zip");
    archive(&source_zip, &[("A.dsl", b"#NAME \"Good\"\nword\n\tdefinition\n"), ("B.mdx", b"damaged")]);
    let old = content.join("old");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("keep"), "previous dictionary").unwrap();
    {
        let batch = source::install_batch(&source_zip, &content, "batch", &|_| {}).unwrap();
        let first = Dictionary::open_cached("batch-1", &batch.paths[0], &cache, &|_| {}).unwrap();
        assert!(Dictionary::open_cached("batch-2", &batch.paths[1], &cache, &|_| {}).is_err());
        drop(first); // Readers are released before the batch's rollback, including on Windows.
    }
    assert_eq!(fs::read_to_string(old.join("keep")).unwrap(), "previous dictionary");
    assert_eq!(fs::read_dir(&content).unwrap().count(), 1);
    assert!(source_zip.is_file());
}

#[test]
fn batch_import_groups_nested_yomitan_and_stardict_companions_without_hiding_siblings() {
    let directory = Directory::new();
    let archive_path = directory.0.join("mixed.zip");
    archive(&archive_path, &[
        ("one/index.json", br#"{"title":"One","format":3,"revision":"1"}"#),
        ("one/term_bank_1.json", b"[]"), ("one/resources/notes.txt", b"resource"),
        ("two/index.json", br#"{"title":"Two","format":3,"revision":"1"}"#),
        ("two/term_bank_1.json", b"[]"),
        ("three/words.ifo", b"StarDict's dict ifo file\nversion=2.4.2\n"),
        ("three/words.idx", b"index"), ("three/words.dict", b"data"),
        ("three/other.mdx", b"placeholder for discovery"), ("three/other.mdd", b"media"),
        ("four/data.noun", b"data"), ("four/index.noun", b"index"),
        ("readme.txt", b"not a dictionary"),
    ]);
    let content = directory.0.join("content");
    let batch = source::install_batch(&archive_path, &content, "mixed", &|_| {}).unwrap();
    assert_eq!(batch.paths.len(), 5);
    for path in &batch.paths { source::main_file(path).unwrap(); }
    let stardict = batch.paths.iter().find(|p| p.join("words.ifo").exists()).unwrap();
    assert!(stardict.join("words.idx").exists());
    assert!(!stardict.join("other.mdd").exists());
    let mdx = batch.paths.iter().find(|p| p.join("other.mdx").exists()).unwrap();
    assert!(mdx.join("other.mdd").exists());
    assert!(!mdx.join("words.ifo").exists());
    assert!(!mdx.join("words.idx").exists());
    let yomitan = batch.paths.iter().find(|p| p.join("resources/notes.txt").exists()).unwrap();
    assert_eq!(source::main_files(yomitan).unwrap().len(), 1);
    drop(batch);
    assert_eq!(fs::read_dir(&content).unwrap().count(), 0);
}

#[test]
fn a_single_archive_and_direct_main_keep_their_existing_storage_contract() {
    let directory = Directory::new();
    let content = directory.0.join("content");
    let archive_path = directory.0.join("single.zip");
    archive(&archive_path, &[("nested/words.dsl", b"#NAME \"One\"\nword\n\tdefinition\n")]);
    let mut batch = source::install_batch(&archive_path, &content, "single", &|_| {}).unwrap();
    assert_eq!(batch.paths, vec![content.join("single")]);
    assert!(batch.paths[0].join("nested/words.dsl").is_file());
    batch.commit();
    drop(batch);
    assert!(content.join("single").exists());
    assert!(source::install_batch(&archive_path, &content, "single", &|_| {}).is_err());
    let direct = directory.0.join("direct.dsl");
    fs::write(&direct, "word\n\tdefinition\n").unwrap();
    let batch = source::install_batch(&direct, &content, "direct", &|_| {}).unwrap();
    assert_eq!(batch.paths, vec![direct.canonicalize().unwrap()]);
    drop(batch);
    assert!(direct.exists(), "rollback never deletes a directly selected original");
}

/// Optional real-file regression, supplied only to a manually dispatched CI run. The dictionary
/// archive is never checked into Git. Synthetic MDX/MDD coverage above runs on both platforms.
#[test]
fn real_mdict_collection_installs_reopens_and_removes_independently() {
    let Ok(archive_path) = std::env::var("LOOKITAN_REAL_MDICT_ARCHIVE") else { return };
    let directory = Directory::new();
    let cache = directory.0.join("cache");
    let content = cache.join("content");
    let mut batch = source::install_batch(Path::new(&archive_path), &content, "real-pictures", &|_| {}).unwrap();
    assert_eq!(batch.paths.len(), 2, "the reported picturedict archive contains two dictionaries");
    for (i, path) in batch.paths.iter().enumerate() {
        let dict = Dictionary::open_cached(&format!("real-{i}"), path, &cache, &|_| {}).unwrap();
        assert!(dict.len() > 0);
        assert!(!dict.mdds.is_empty());
        let resource = dict.mdds[0].headword(0).unwrap();
        assert!(!dict.resource(resource).unwrap().is_empty(), "the companion's image must be served");
        println!("real archive: {}: {} entries, {} MDDs, resource {}", dict.name, dict.len(), dict.mdds.len(), resource);
    }
    batch.commit();
    let paths = batch.paths.clone();
    drop(batch);
    fs::remove_dir_all(&paths[0]).unwrap();
    let reopened = Dictionary::open_cached("real-second", &paths[1], &cache, &|_| {}).unwrap();
    let resource = reopened.mdds[0].headword(0).unwrap();
    assert!(!reopened.resource(resource).unwrap().is_empty());
    drop(reopened);
    fs::remove_dir_all(&paths[1]).unwrap();
    assert_eq!(fs::read_dir(&content).unwrap().count(), 0);
}
