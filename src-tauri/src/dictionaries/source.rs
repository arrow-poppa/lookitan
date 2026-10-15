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

fn discover(root: &Path) -> Result<PathBuf> {
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
    // A Yomitan archive's bank/resource files belong to its index, not to separate dictionaries.
    let yomitan: Vec<_> = found.iter().filter(|p| p.file_name().map(|n| n == "index.json").unwrap_or(false)).collect();
    if yomitan.len() == 1 { return Ok(yomitan[0].to_path_buf()); }
    // StarDict's data and index files describe one dictionary.
    let ifos: Vec<_> = found.iter().filter(|p| p.extension().map(|e| e.eq_ignore_ascii_case("ifo")).unwrap_or(false)).collect();
    if ifos.len() == 1 { return Ok(ifos[0].to_path_buf()); }
    // A file only these formats use wins over loose text or XML files beside it (a read-me.txt in
    // an MDict archive is not a CC-CEDICT dictionary).
    let distinct: Vec<_> = found.iter().filter(|p| {
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        [".mdx", ".dsl", ".dsl.dz", ".slob"].iter().any(|s| name.ends_with(s)) || name == "cide.a"
            || (name.ends_with(".json") && super::jsondict::detect(p).is_some())
            || (name.ends_with(".xml") && super::wnlmf::detect(p))
            || (name.ends_with(".txt") && super::webster_text::detect(p))
    }).collect();
    if distinct.len() == 1 { return Ok(distinct[0].to_path_buf()); }
    let gcide: Vec<_> = found.iter().filter(|p| p.file_name().map(|n| n == "gcide.xml").unwrap_or(false)).collect();
    if gcide.len() == 1 { return Ok(gcide[0].to_path_buf()); }
    if let Some(data) = found.iter().find(|p| p.file_name().map(|n| n == "data.noun").unwrap_or(false)) {
        return Ok(data.clone());
    }
    found.dedup();
    match found.as_slice() {
        [path] => Ok(path.clone()),
        [] => bail!("nenhum dicionário compatível encontrado no arquivo"),
        _ => bail!("o arquivo contém vários dicionários; extraia-o e adicione cada arquivo principal separadamente"),
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
    // An archive that holds several dictionaries, or none, is refused before it is kept.
    discover(&pending.path)?;
    fs::rename(&pending.path, &target).context("publish unpacked dictionary")?;
    Ok(target)
}

/// The main file of an installed dictionary: the path itself when it is a file, otherwise the
/// one dictionary in the folder.
pub fn main_file(path: &Path) -> Result<PathBuf> {
    if path.is_dir() { discover(path) } else if path.is_file() { Ok(path.to_path_buf()) } else {
        bail!("No dictionary folder at '{}'.", path.display())
    }
}

/// Whether `path` is a folder Lookitan unpacked under `content`, which it may delete.
pub fn is_installed_copy(path: &Path, content: &Path) -> bool {
    match (path.canonicalize(), content.canonicalize()) {
        (Ok(path), Ok(content)) => path.parent() == Some(content.as_path()) && path.is_dir(),
        _ => false,
    }
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
