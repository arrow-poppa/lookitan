use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

fn is_main(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    name == "index.json" || name == "data.noun" || name == "index.noun" || name == "cide.a"
        || (name.ends_with(".json") && super::jsondict::detect(path).is_some())
        || [".mdx", ".ifo", ".dsl", ".dsl.dz", ".slob", ".xml", ".xml.gz", ".xml.bz2", ".jsonl", ".jsonl.gz", ".jsonl.bz2", ".u8"]
            .iter().any(|s| name.ends_with(s))
        || ((name.ends_with(".txt") || name.ends_with(".txt.gz")) && !name.starts_with("readme") && !name.starts_with("license"))
}

/// All main files, in stable path order. Format companions and loose resource files are not
/// separate dictionaries. Selection is scoped to each directory, so sibling dictionaries survive.
fn discover_all(root: &Path) -> Result<Vec<PathBuf>> {
    let mut stack = vec![(root.to_path_buf(), 0)];
    let mut found = Vec::new();
    let mut visited = 0;
    while let Some((dir, depth)) = stack.pop() {
        for e in fs::read_dir(dir)? {
            let e = e?;
            let kind = e.file_type()?;
            visited += 1;
            if visited > 200_000 { bail!("too many files in dictionary archive"); }
            if kind.is_dir() && depth < 12 { stack.push((e.path(), depth + 1)); }
            else if kind.is_file() && is_main(&e.path()) { found.push(e.path()); }
        }
    }
    found.sort();
    found.dedup();
    let yomitan: Vec<_> = found.iter().filter(|p| p.file_name().map(|n| n == "index.json").unwrap_or(false))
        .filter_map(|p| p.parent().map(Path::to_path_buf)).collect();
    // Banks and resources below a Yomitan root belong to its index.
    found.retain(|p| !yomitan.iter().any(|dir| p.starts_with(dir) && p != &dir.join("index.json")));
    let specific: Vec<_> = found.iter().filter(|p| {
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        [".mdx", ".ifo", ".dsl", ".dsl.dz", ".slob"].iter().any(|s| name.ends_with(s))
            || ["index.json", "data.noun", "cide.a", "gcide.xml"].contains(&name.as_str())
            || (name.ends_with(".json") && super::jsondict::detect(p).is_some())
            || (name.ends_with(".xml") && super::wnlmf::detect(p))
            || (name.ends_with(".txt") && super::webster_text::detect(p))
    }).cloned().collect();
    found.retain(|p| specific.contains(p) || !specific.iter().any(|main| main.parent() == p.parent()));
    if found.is_empty() { bail!("nenhum dicionário compatível encontrado no arquivo"); }
    Ok(found)
}

/// Main files for a batch import. A directly selected main file bypasses folder discovery.
pub fn main_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_dir() { discover_all(path) } else if path.is_file() { Ok(vec![path.to_path_buf()]) } else {
        bail!("No dictionary folder at '{}'.", path.display())
    }
}

struct PendingDirectory { path: PathBuf, parent: PathBuf }
impl Drop for PendingDirectory {
    fn drop(&mut self) {
        // Both paths were created by us, and only this exact child may be recursively removed.
        if let (Ok(path), Ok(parent)) = (self.path.canonicalize(), self.parent.canonicalize()) {
            if path.parent() == Some(parent.as_path()) { let _ = fs::remove_dir_all(path); }
        }
    }
}

/// Marks an unpacked copy whose removal failed (Windows keeps files that are still mapped); it is
/// deleted at the next start.
pub const REMOVED_MARKER: &str = ".lookitan-removed";

/// Installs a dictionary the way Lookupper does: an archive (ZIP, RAR, 7z, TAR, or one gzip,
/// bzip2 or xz file; see archive.rs) is unpacked into its own folder, `<content>/<id>/`, and the
/// dictionary is then read from the unpacked files, which stay as they are. Other paths (a main
/// file or a folder chosen directly) are used in place and returned as they are.
pub fn install(path: &Path, content: &Path, id: &str, progress: &dyn Fn(u64)) -> Result<PathBuf> {
    let path = path.canonicalize().with_context(|| format!("open {}", path.display()))?;
    let Some(kind) = super::archive::kind(&path) else { return Ok(path) };
    fs::create_dir_all(content)?;
    let target = content.join(id);
    if target.join(REMOVED_MARKER).is_file() {
        // The same archive again, after a removal that had to wait for its files to be released.
        fs::remove_dir_all(&target).context("remove the earlier copy of this dictionary")?;
    }
    if target.exists() { bail!("este dicionário já está instalado"); }
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let pending = PendingDirectory { path: content.join(format!(".unpacking-{id}-{unique}")), parent: content.to_path_buf() };
    fs::create_dir(&pending.path)?;
    super::archive::unpack(&path, kind, &pending.path, progress)?;
    // Refuse archives with no main file before publishing the extracted copy.
    discover_all(&pending.path)?;
    fs::rename(&pending.path, &target).context("publish unpacked dictionary")?;
    Ok(target)
}

/// The main file of an installed dictionary: the path itself when it is a file, otherwise the
/// one dictionary in the folder.
pub fn main_file(path: &Path) -> Result<PathBuf> {
    let files = main_files(path)?;
    match files.as_slice() {
        [path] => Ok(path.clone()),
        _ => bail!("o arquivo contém vários dicionários; adicione cada arquivo principal separadamente"),
    }
}

/// Whether `path` is a folder Lookitan unpacked under `content`, which it may delete.
pub fn is_installed_copy(path: &Path, content: &Path) -> bool {
    match (path.canonicalize(), content.canonicalize()) {
        (Ok(path), Ok(content)) => path.parent() == Some(content.as_path()) && path.is_dir(),
        _ => false,
    }
}

/// A source is registered only after every dictionary has opened and settings were saved.
/// Dropping an unsuccessful batch rolls back only the directories created by this import.
pub struct InstalledBatch {
    pub paths: Vec<PathBuf>,
    copies: Vec<PathBuf>,
    content: PathBuf,
    committed: bool,
}

impl InstalledBatch {
    pub fn commit(&mut self) { self.committed = true; }
}

impl Drop for InstalledBatch {
    fn drop(&mut self) {
        if !self.committed {
            for path in &self.copies {
                let pending = PendingDirectory { path: path.clone(), parent: self.content.clone() };
                drop(pending);
                // A live reader elsewhere can keep a Windows file mapped; retry at startup.
                if path.is_dir() { let _ = fs::write(path.join(REMOVED_MARKER), b""); }
            }
        }
    }
}

fn mdd_companion(main: &Path, resource: &Path) -> bool {
    if !main.extension().map(|e| e.eq_ignore_ascii_case("mdx")).unwrap_or(false)
        || main.parent() != resource.parent() || !resource.extension().map(|e| e.eq_ignore_ascii_case("mdd")).unwrap_or(false) {
        return false;
    }
    let stem = main.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
    let name = resource.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
    name == stem || name.strip_prefix(&format!("{stem}.")).and_then(|n| n.parse::<u64>().ok()).is_some()
}

fn stardict_companion(main: &Path, resource: &Path) -> bool {
    if main.parent() != resource.parent() || !main.extension().map(|e| e.eq_ignore_ascii_case("ifo")).unwrap_or(false) {
        return false;
    }
    let stem = main.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
    let name = resource.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    ["idx", "idx.gz", "dict", "dict.dz", "syn"].iter().any(|ext| name == format!("{stem}.{ext}"))
}

#[derive(Default)]
struct CopyBudget { bytes: u64, members: u64 }
impl CopyBudget {
    fn charge(&mut self, bytes: u64) -> Result<()> {
        self.bytes = self.bytes.checked_add(bytes).context("batch size overflow")?;
        self.members += 1;
        if self.bytes > super::archive::MAX_TOTAL { bail!("a instalação em lote passa de 32 GiB"); }
        if self.members > super::archive::MAX_MEMBERS { bail!("a instalação em lote tem mais de 200 mil itens"); }
        Ok(())
    }
}

/// Preserve paths relative to this main file, but exclude other dictionaries and their known
/// companions. Loose resources are copied to give each installation independent ownership.
fn copy_dictionary(main: &Path, mains: &[PathBuf], target: &Path, budget: &mut CopyBudget) -> Result<()> {
    let root = main.parent().context("dictionary main file has no parent")?;
    let mut stack = vec![(root.to_path_buf(), target.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_symlink() { bail!("dictionary archive symlinks are unsupported"); }
            if kind.is_dir() {
                // A nested dictionary owns its directory; it is not a resource of this one.
                if mains.iter().any(|other| other != main && other.parent() == Some(path.as_path())) { continue; }
                budget.charge(0)?;
                let dest = to.join(entry.file_name());
                fs::create_dir(&dest)?;
                stack.push((path, dest));
            } else if kind.is_file() {
                if mains.iter().any(|other| other != main && (other == &path
                    || (other.extension().map(|e| e.eq_ignore_ascii_case("mdx")).unwrap_or(false) && mdd_companion(other, &path))
                    || stardict_companion(other, &path))) { continue; }
                // MDict never opens another name's MDD. Do not retain orphan resource databases.
                if main.extension().map(|e| e.eq_ignore_ascii_case("mdx")).unwrap_or(false)
                    && path.extension().map(|e| e.eq_ignore_ascii_case("mdd")).unwrap_or(false)
                    && !mdd_companion(main, &path) { continue; }
                budget.charge(entry.metadata()?.len())?;
                let dest = to.join(entry.file_name());
                if path == main || mdd_companion(main, &path) || stardict_companion(main, &path) {
                    // Exclusive large files move on the same filesystem: no second MDD copy.
                    fs::rename(&path, &dest)?;
                } else {
                    fs::copy(&path, &dest)?;
                }
            }
        }
    }
    Ok(())
}

/// Extract once, then install each discovered dictionary in its own removable directory. Direct
/// main files are read in place. Archives with one dictionary retain the previous directory layout.
pub fn install_batch(path: &Path, content: &Path, id: &str, progress: &dyn Fn(u64)) -> Result<InstalledBatch> {
    let source = path.canonicalize().with_context(|| format!("open {}", path.display()))?;
    let mut batch = InstalledBatch { paths: Vec::new(), copies: Vec::new(), content: content.to_path_buf(), committed: false };
    if super::archive::kind(&source).is_none() {
        batch.paths = main_files(&source)?;
        return Ok(batch);
    }
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let stage = install(&source, content, &format!(".unpacking-batch-{id}-{unique}"), progress)?;
    let pending = PendingDirectory { path: stage.clone(), parent: content.to_path_buf() };
    let mains = main_files(&stage)?;
    let mut budget = CopyBudget::default();
    for (index, main) in mains.iter().enumerate() {
        let target = content.join(if mains.len() == 1 { id.to_string() } else { format!("{id}-{}", index + 1) });
        if target.join(REMOVED_MARKER).is_file() { fs::remove_dir_all(&target).context("remove the earlier copy of this dictionary")?; }
        if target.exists() { bail!("este dicionário já está instalado"); }
        if mains.len() == 1 {
            fs::rename(&stage, &target).context("publish unpacked dictionary")?;
            batch.copies.push(target.clone());
        } else {
            budget.charge(0)?;
            fs::create_dir(&target)?;
            batch.copies.push(target.clone());
            copy_dictionary(main, &mains, &target, &mut budget)?;
        }
        batch.paths.push(target);
    }
    drop(pending);
    Ok(batch)
}

/// Resolve both MDict backslashes and web paths. Case-insensitive traversal also works on Linux.
/// Symlinks may only resolve inside the dictionary directory.
pub fn resource_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let normalized = relative.replace('\\', "/");
    let components: Vec<_> = normalized.trim_start_matches('/').split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    if components.is_empty() || components.iter().any(|p| *p == ".." || p.contains(':') || p.contains('\0')) { return None; }
    let root = root.canonicalize().ok()?;
    let mut current = root.clone();
    for part in components {
        let next = current.join(part);
        current = if next.exists() { next } else {
            fs::read_dir(&current).ok()?.flatten().find(|e| e.file_name().to_string_lossy().to_lowercase() == part.to_lowercase())?.path()
        };
        current = current.canonicalize().ok()?;
        if !current.starts_with(&root) { return None; }
    }
    current.is_file().then_some(current)
}

pub fn resource(root: &Path, relative: &str) -> Option<Vec<u8>> {
    let path = resource_path(root, relative)?;
    if fs::metadata(&path).ok()?.len() > 64 * 1024 * 1024 { return None; }
    fs::read(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distributing_shared_resources_cannot_expand_past_the_archive_budget() {
        let mut bytes = CopyBudget { bytes: super::super::archive::MAX_TOTAL, members: 0 };
        assert!(bytes.charge(1).is_err());
        let mut members = CopyBudget { bytes: 0, members: super::super::archive::MAX_MEMBERS };
        assert!(members.charge(0).is_err());
    }

    #[test]
    fn resources_use_dictionary_root_and_case_insensitive_nested_paths() {
        let root = std::env::temp_dir().join(format!("lkt-path-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(root.join("Images")).unwrap();
        fs::write(root.join("Images/Word.PNG"), b"image").unwrap();
        assert_eq!(resource(&root, "\\images\\word.png").unwrap(), b"image");
        assert!(resource(&root, "../secret").is_none());
        assert!(resource(&root, "C:/secret").is_none());
        assert!(resource(&root, "%2e%2e/secret").is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
