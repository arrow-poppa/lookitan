//! The archives a dictionary is imported from, unpacked like Lookupper unpacks them (it uses
//! SharpCompress): ZIP, RAR (RAR4 and RAR5, also split into volumes), 7z, TAR (plain or with gzip,
//! bzip2 or xz) and a single file compressed with gzip, bzip2 or xz. Members are streamed to disk,
//! never held whole in memory; paths that leave the folder, links and devices are refused or
//! skipped, and the total size and member count are bounded.

use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context, Result};

const MAX_TOTAL: u64 = 32 * 1024 * 1024 * 1024;
const MAX_MEMBERS: u64 = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression { None, Gzip, Bzip2, Xz }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zip,
    Rar,
    SevenZip,
    Tar(Compression),
    /// One compressed file, unpacked under its name without the compression suffix.
    Single(Compression),
}

/// The archive kind of `path`, by its name; `None` for a dictionary file read in place.
pub fn kind(path: &Path) -> Option<Kind> {
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    let ends = |suffixes: &[&str]| suffixes.iter().any(|s| name.ends_with(s));
    Some(if ends(&[".zip"]) {
        Kind::Zip
    } else if ends(&[".rar"]) {
        Kind::Rar
    } else if ends(&[".7z"]) {
        Kind::SevenZip
    } else if ends(&[".tar"]) {
        Kind::Tar(Compression::None)
    } else if ends(&[".tar.gz", ".tgz"]) {
        Kind::Tar(Compression::Gzip)
    } else if ends(&[".tar.bz2", ".tbz", ".tbz2"]) {
        Kind::Tar(Compression::Bzip2)
    } else if ends(&[".tar.xz", ".txz"]) {
        Kind::Tar(Compression::Xz)
    } else if ends(&[".gz"]) {
        Kind::Single(Compression::Gzip)
    } else if ends(&[".bz2"]) {
        Kind::Single(Compression::Bzip2)
    } else if ends(&[".xz"]) {
        Kind::Single(Compression::Xz)
    } else {
        return None;
    })
}

fn decompressed(file: File, compression: Compression) -> Box<dyn Read> {
    let file = BufReader::new(file);
    match compression {
        Compression::None => Box::new(file),
        Compression::Gzip => Box::new(flate2::read::MultiGzDecoder::new(file)),
        Compression::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(file)),
        Compression::Xz => Box::new(lzma_rust2::XzReader::new(file, true)),
    }
}

/// How much has been unpacked, shared with writers handed to the RAR decoder.
#[derive(Clone, Default)]
struct Budget {
    bytes: Arc<AtomicU64>,
    members: Arc<AtomicU64>,
}

impl Budget {
    fn member(&self) -> Result<()> {
        ensure!(self.members.fetch_add(1, Ordering::Relaxed) < MAX_MEMBERS, "o arquivo tem mais de 200 mil itens");
        Ok(())
    }
}

/// A file being unpacked; it stops once the budget is spent.
struct Limited {
    file: File,
    budget: Budget,
}

impl Write for Limited {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let total = self.budget.bytes.fetch_add(data.len() as u64, Ordering::Relaxed) + data.len() as u64;
        if total > MAX_TOTAL {
            return Err(std::io::Error::other("o dicionário descompactado passa de 32 GiB"));
        }
        self.file.write_all(data)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Where member `name` goes below `root`: separators may be `/` or `\`, and a name that leaves
/// the folder (`..`, a drive or an absolute path) is refused. `None` for the folder itself.
fn member_path(root: &Path, name: &str) -> Result<Option<PathBuf>> {
    let name = name.replace('\\', "/");
    ensure!(!name.contains('\0') && !name.starts_with('/'), "caminho inseguro no arquivo: {name}");
    let mut path = root.to_path_buf();
    let mut any = false;
    for part in name.split('/').filter(|p| !p.is_empty() && *p != ".") {
        ensure!(part != ".." && !part.contains(':'), "caminho inseguro no arquivo: {name}");
        path.push(part);
        any = true;
    }
    Ok(any.then_some(path))
}

fn create(path: &Path, budget: &Budget) -> Result<Limited> {
    budget.member()?;
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let file = File::options().create_new(true).write(true).open(path).with_context(|| format!("criar {}", path.display()))?;
    Ok(Limited { file, budget: budget.clone() })
}

/// Unpacks `path` (of kind `kind`) into the empty folder `root`; `progress` receives the bytes
/// unpacked so far.
pub fn unpack(path: &Path, kind: Kind, root: &Path, progress: &dyn Fn(u64)) -> Result<()> {
    let budget = Budget::default();
    let report = |budget: &Budget| progress(budget.bytes.load(Ordering::Relaxed));
    match kind {
        Kind::Zip => {
            let mut archive = zip::ZipArchive::new(File::open(path)?).context("o arquivo não é um ZIP legível")?;
            ensure!(archive.len() as u64 <= MAX_MEMBERS, "o arquivo tem mais de 200 mil itens");
            for i in 0..archive.len() {
                let mut file = archive.by_index(i)?;
                let member = file.enclosed_name().ok_or_else(|| anyhow!("unsafe dictionary archive path"))?.to_path_buf();
                if file.name().contains('\\') || member.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
                    || member.to_string_lossy().contains(':') {
                    bail!("unsafe dictionary archive path");
                }
                if file.unix_mode().map(|mode| mode & 0o170000 == 0o120000).unwrap_or(false) { bail!("dictionary archive symlinks are unsupported"); }
                let output = root.join(member);
                if file.is_dir() { fs::create_dir_all(output)?; continue; }
                let mut out = create(&output, &budget)?;
                std::io::copy(&mut file, &mut out)?;
                out.flush()?;
                report(&budget);
            }
        }
        Kind::Rar => {
            let volumes = rar_volumes(path)?;
            let archives = volumes.iter()
                .map(|v| rars::ArchiveReader::read_path(v).map_err(|e| anyhow!("{}: {e}", v.display())))
                .collect::<Result<Vec<_>>>()?;
            let open = |meta: &rars::ExtractedEntryMeta| -> rars::Result<Box<dyn Write>> {
                let relative = rars::entry_relative_path(meta.name_bytes())?;
                let name = relative.to_string_lossy().into_owned();
                let target = member_path(root, &name).map_err(|e| std::io::Error::other(e.to_string()))?;
                let Some(target) = target else { return Ok(Box::new(std::io::sink())) };
                if meta.is_directory {
                    fs::create_dir_all(&target)?;
                    return Ok(Box::new(std::io::sink()));
                }
                let out = create(&target, &budget).map_err(|e| std::io::Error::other(e.to_string()))?;
                report(&budget);
                Ok(Box::new(out))
            };
            let result = if archives.len() == 1 {
                archives[0].extract_to(None, open)
            } else {
                rars::extract_volumes_to(&archives, None, open)
            };
            result.map_err(|e| anyhow!("RAR: {e}"))?;
        }
        Kind::SevenZip => {
            let mut reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
                .map_err(|e| anyhow!("7z: {e}"))?;
            reader.for_each_entries(|entry, data| {
                let target = member_path(root, entry.name()).map_err(|e| std::io::Error::other(e.to_string()))?;
                let Some(target) = target else { return Ok(true) };
                if entry.is_directory() {
                    fs::create_dir_all(&target)?;
                    return Ok(true);
                }
                let mut out = create(&target, &budget).map_err(|e| std::io::Error::other(e.to_string()))?;
                std::io::copy(data, &mut out)?;
                out.flush()?;
                report(&budget);
                Ok(true)
            }).map_err(|e| anyhow!("7z: {e}"))?;
        }
        Kind::Tar(compression) => {
            let mut archive = tar::Archive::new(decompressed(File::open(path)?, compression));
            for entry in archive.entries().context("o arquivo não é um TAR legível")? {
                let mut entry = entry?;
                let name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
                let Some(target) = member_path(root, &name)? else { continue };
                match entry.header().entry_type() {
                    tar::EntryType::Directory => fs::create_dir_all(&target)?,
                    tar::EntryType::Regular | tar::EntryType::Continuous => {
                        let mut out = create(&target, &budget)?;
                        std::io::copy(&mut entry, &mut out)?;
                        out.flush()?;
                        report(&budget);
                    }
                    // Links, devices and metadata records are not dictionary files.
                    _ => {}
                }
            }
        }
        Kind::Single(compression) => {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            let cut = [".gz", ".bz2", ".xz"].iter().find(|s| lower.ends_with(*s)).map(|s| s.len()).unwrap_or(0);
            let stem = &name[..name.len() - cut];
            ensure!(!stem.is_empty(), "arquivo compactado sem nome");
            let mut out = create(&root.join(stem), &budget)?;
            std::io::copy(&mut decompressed(File::open(path)?, compression), &mut out)
                .with_context(|| format!("descompactar {}", path.display()))?;
            out.flush()?;
            report(&budget);
        }
    }
    Ok(())
}

/// The volumes of a RAR set, in order, from any one of them: `name.part1.rar`, `name.part2.rar`,
/// … or `name.rar`, `name.r00`, `name.r01`, ….
fn rar_volumes(path: &Path) -> Result<Vec<PathBuf>> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let lower = name.to_lowercase();
    if let Some(stem) = lower.strip_suffix(".rar") {
        if let Some(at) = stem.rfind(".part") {
            let digits = &stem[at + 5..];
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                let base = &name[..at];
                let width = digits.len();
                let mut volumes = Vec::new();
                for n in 1.. {
                    let volume = dir.join(format!("{base}.part{n:0width$}.rar"));
                    if !volume.is_file() { break; }
                    volumes.push(volume);
                }
                ensure!(!volumes.is_empty(), "Falta o primeiro volume do RAR: {base}.part{:0width$}.rar", 1);
                ensure!(volumes.iter().any(|v| v.file_name() == path.file_name()), "Falta um volume do RAR antes de {name}");
                return Ok(volumes);
            }
        }
        let mut volumes = vec![path.to_path_buf()];
        let base = &name[..name.len() - 4];
        for n in 0..1000 {
            let volume = dir.join(format!("{base}.r{n:02}"));
            if !volume.is_file() { break; }
            volumes.push(volume);
        }
        return Ok(volumes);
    }
    Ok(vec![path.to_path_buf()])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            use std::sync::atomic::AtomicU64;
            static ID: AtomicU64 = AtomicU64::new(0);
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("lkt-archive-{}-{stamp}-{}", std::process::id(), ID.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn archives_are_recognized_by_name() {
        for (name, expected) in [
            ("a.zip", Some(Kind::Zip)), ("a.RAR", Some(Kind::Rar)), ("a.part2.rar", Some(Kind::Rar)), ("a.7z", Some(Kind::SevenZip)),
            ("a.tar", Some(Kind::Tar(Compression::None))), ("a.tgz", Some(Kind::Tar(Compression::Gzip))),
            ("gcide-0.53.tar.xz", Some(Kind::Tar(Compression::Xz))), ("a.tar.bz2", Some(Kind::Tar(Compression::Bzip2))),
            ("wn.xml.gz", Some(Kind::Single(Compression::Gzip))), ("a.jsonl.bz2", Some(Kind::Single(Compression::Bzip2))),
            ("a.mdx", None), ("a.dsl.dz", None), ("a.ifo", None),
        ] {
            assert_eq!(kind(Path::new(name)), expected, "{name}");
        }
    }

    #[test]
    fn member_paths_stay_inside_the_folder() {
        let root = Path::new("root");
        assert_eq!(member_path(root, "dict\\words.dsl").unwrap(), Some(root.join("dict").join("words.dsl")));
        assert_eq!(member_path(root, "./a//b").unwrap(), Some(root.join("a").join("b")));
        assert_eq!(member_path(root, "./").unwrap(), None);
        for bad in ["../x", "a/../../x", "/etc/passwd", "C:/x", "a\0b"] {
            assert!(member_path(root, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn tar_xz_gz_and_single_files_unpack() {
        let dir = TestDir::new();
        // A tar with a folder, a file, a symlink (skipped) and an unsafe path (refused).
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        builder.append_data(&mut header, "gcide-0.53/", &[][..]).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        builder.append_data(&mut header, "gcide-0.53/CIDE.A", &b"<p/>\n"[..]).unwrap();
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        builder.append_link(&mut link, "gcide-0.53/escape", "/etc/passwd").unwrap();
        let tar = builder.into_inner().unwrap();

        let plain = dir.0.join("g.tar");
        fs::write(&plain, &tar).unwrap();
        let gz = dir.0.join("g.tar.gz");
        let mut e = flate2::write::GzEncoder::new(File::create(&gz).unwrap(), flate2::Compression::default());
        e.write_all(&tar).unwrap();
        e.finish().unwrap();
        let xz = dir.0.join("g.tar.xz");
        let mut w = lzma_rust2::XzWriter::new(File::create(&xz).unwrap(), lzma_rust2::XzOptions::default()).unwrap();
        w.write_all(&tar).unwrap();
        w.finish().unwrap();
        for (archive, out) in [(&plain, "p"), (&gz, "g"), (&xz, "x")] {
            let root = dir.0.join(out);
            fs::create_dir(&root).unwrap();
            unpack(archive, kind(archive).unwrap(), &root, &|_| {}).unwrap();
            assert_eq!(fs::read(root.join("gcide-0.53/CIDE.A")).unwrap(), b"<p/>\n", "{}", archive.display());
            assert!(!root.join("gcide-0.53/escape").exists(), "links are not unpacked");
        }

        let single = dir.0.join("english-wordnet-2025.xml.gz");
        let mut e = flate2::write::GzEncoder::new(File::create(&single).unwrap(), flate2::Compression::default());
        e.write_all(b"<LexicalResource/>").unwrap();
        e.finish().unwrap();
        let root = dir.0.join("s");
        fs::create_dir(&root).unwrap();
        unpack(&single, kind(&single).unwrap(), &root, &|_| {}).unwrap();
        assert_eq!(fs::read(root.join("english-wordnet-2025.xml")).unwrap(), b"<LexicalResource/>");
    }

    #[test]
    fn seven_zip_unpacks() {
        let dir = TestDir::new();
        let source = dir.0.join("src");
        fs::create_dir_all(source.join("dict")).unwrap();
        fs::write(source.join("dict").join("words.dsl"), "#NAME \"7z\"\nword\n\tdefinition\n").unwrap();
        let archive = dir.0.join("dict.7z");
        sevenz_rust2::compress_to_path(&source, &archive).unwrap();
        let root = dir.0.join("out");
        fs::create_dir(&root).unwrap();
        unpack(&archive, Kind::SevenZip, &root, &|_| {}).unwrap();
        assert!(fs::read_to_string(root.join("dict").join("words.dsl")).unwrap().contains("definition"));
    }

    /// Bytes that do not compress, so a small volume size splits them.
    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x2545F491u32;
        (0..len).map(|_| { state ^= state << 13; state ^= state >> 17; state ^= state << 5; state as u8 }).collect()
    }

    #[test]
    fn rar4_rar5_and_split_rar5_unpack() {
        let dir = TestDir::new();
        let words: Vec<u8> = (0..3000).map(|i| format!("word{i}\n\tdefinition {i}\n")).collect::<String>().into_bytes();
        for (name, version) in [("old.rar", rars::ArchiveVersion::Rar29), ("new.rar", rars::ArchiveVersion::Rar50)] {
            let mut builder = rars::Builder::new(version);
            builder.add_directory(b"dict".to_vec(), None, None).unwrap();
            builder.add_bytes(b"dict/words.dsl".to_vec(), words.clone(), None, None).unwrap();
            let archive = dir.0.join(name);
            builder.write_to_path(&archive, None).unwrap();
            let root = dir.0.join(format!("out-{name}"));
            fs::create_dir(&root).unwrap();
            unpack(&archive, kind(&archive).unwrap(), &root, &|_| {}).unwrap();
            assert_eq!(fs::read(root.join("dict").join("words.dsl")).unwrap(), words, "{name}");
        }
        let sound = noise(150_000);
        let mut builder = rars::Builder::new(rars::ArchiveVersion::Rar50).volume_size(Some(40_000));
        builder.add_bytes(b"dict/words.dsl".to_vec(), words.clone(), None, None).unwrap();
        builder.add_bytes(b"dict/res/cat.wav".to_vec(), sound.clone(), None, None).unwrap();
        let volumes = builder.build_volumes(None).unwrap();
        assert!(volumes.len() > 2, "the fixture is a real volume set");
        for (i, volume) in volumes.iter().enumerate() {
            fs::write(dir.0.join(format!("set.part{}.rar", i + 1)), volume).unwrap();
        }
        let root = dir.0.join("out-set");
        fs::create_dir(&root).unwrap();
        // Any volume can be chosen; the set is found from it.
        unpack(&dir.0.join("set.part2.rar"), Kind::Rar, &root, &|_| {}).unwrap();
        assert_eq!(fs::read(root.join("dict").join("words.dsl")).unwrap(), words);
        assert_eq!(fs::read(root.join("dict").join("res").join("cat.wav")).unwrap(), sound);
        fs::remove_file(dir.0.join("set.part2.rar")).unwrap();
        let root = dir.0.join("out-missing");
        fs::create_dir(&root).unwrap();
        assert!(unpack(&dir.0.join("set.part1.rar"), Kind::Rar, &root, &|_| {}).is_err(), "a missing volume is an error");
    }

    #[test]
    fn rar_volume_sets_are_found_from_any_volume() {
        let dir = TestDir::new();
        for n in 1..=3 { fs::write(dir.0.join(format!("oald.part{n:02}.rar")), b"").unwrap(); }
        let volumes = rar_volumes(&dir.0.join("oald.part02.rar")).unwrap();
        assert_eq!(volumes.iter().map(|v| v.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>(),
            ["oald.part01.rar", "oald.part02.rar", "oald.part03.rar"]);
        fs::write(dir.0.join("old.rar"), b"").unwrap();
        fs::write(dir.0.join("old.r00"), b"").unwrap();
        assert_eq!(rar_volumes(&dir.0.join("old.rar")).unwrap().len(), 2);
        fs::write(dir.0.join("lone.part2.rar"), b"").unwrap();
        assert!(rar_volumes(&dir.0.join("lone.part2.rar")).is_err(), "a missing first volume is reported");
    }
}
