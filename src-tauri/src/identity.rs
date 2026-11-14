//! The app's identifier was `io.github.arrowpoppa.lookitan` until 2026-11-14, when the user asked
//! for `app.lookitan` (their GitHub name in every folder path). Tauri names the config, data,
//! local data and cache folders after the identifier, so each folder of the old identifier is
//! moved to the new name once, at start, before anything opens it; then the absolute paths the
//! JSON files keep (installed dictionaries, for example) are rewritten to the new folders.

use std::fs;
use std::path::{Path, PathBuf};

pub const OLD_IDENTIFIER: &str = "io.github.arrowpoppa.lookitan";

/// Moves `old` to `new`. When `new` already exists (an earlier attempt that Windows half
/// blocked), what it lacks is moved over entry by entry, and `old` is removed once empty; an entry
/// both have stays in `old`, the new one winning. Returns whether anything moved.
pub fn move_folder(old: &Path, new: &Path) -> std::io::Result<bool> {
    if !old.is_dir() || old == new { return Ok(false); }
    if !new.exists() {
        if let Some(parent) = new.parent() { fs::create_dir_all(parent)?; }
        if fs::rename(old, new).is_ok() { return Ok(true); }
        fs::create_dir_all(new)?;
    }
    let mut moved = false;
    for entry in fs::read_dir(old)? {
        let entry = entry?;
        let target = new.join(entry.file_name());
        if target.symlink_metadata().is_ok() {
            log::warn!("{} is in both {} and {}; keeping the new one", entry.file_name().to_string_lossy(), old.display(), new.display());
            continue;
        }
        match fs::rename(entry.path(), &target) {
            Ok(()) => moved = true,
            Err(e) => log::warn!("move {} to {}: {e}", entry.path().display(), target.display()),
        }
    }
    let _ = fs::remove_dir(old);
    Ok(moved)
}

/// `text` with every old folder replaced by its new one, as the paths appear in JSON (backslashes
/// escaped, for Windows). Only whole folder names are replaced: the old path must be followed by a
/// separator or the end of the string.
pub fn rewrite_paths(text: &str, moves: &[(PathBuf, PathBuf)]) -> String {
    let mut out = text.to_string();
    for (old, new) in moves {
        let quoted = |path: &Path| serde_json::to_string(&path.to_string_lossy()).unwrap_or_default();
        let (old, new) = (quoted(old), quoted(new));
        // Without the closing quote: the path inside a JSON string.
        let (old, new) = (&old[..old.len().saturating_sub(1)], &new[..new.len().saturating_sub(1)]);
        let (old, new) = (old.trim_start_matches('"'), new.trim_start_matches('"'));
        if old.is_empty() { continue; }
        let mut result = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(at) = rest.find(old) {
            let after = &rest[at + old.len()..];
            let whole = after.starts_with('"') || after.starts_with('/') || after.starts_with("\\\\");
            result.push_str(&rest[..at]);
            result.push_str(if whole { new } else { old });
            rest = after;
        }
        result.push_str(rest);
        out = result;
    }
    out
}

/// Moves the old identifier's folders to the new ones (`dirs`: config, data, local data, cache, as
/// Tauri gives them), then rewrites the paths in the JSON files directly in each new folder.
pub fn migrate(dirs: &[PathBuf]) {
    let mut unique: Vec<&PathBuf> = Vec::new();
    for dir in dirs { if !unique.contains(&dir) { unique.push(dir); } }
    let mut moves = Vec::new();
    for new in &unique {
        let old = new.with_file_name(OLD_IDENTIFIER);
        match move_folder(&old, new) {
            Ok(true) => {
                log::info!("moved {} to {}", old.display(), new.display());
                moves.push((old, new.to_path_buf()));
            }
            Ok(false) => {}
            Err(e) => log::warn!("move {} to {}: {e}", old.display(), new.display()),
        }
    }
    if moves.is_empty() { return; }
    for dir in &unique {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") || !path.is_file() { continue; }
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let rewritten = rewrite_paths(&text, &moves);
            if rewritten == text { continue; }
            let temporary = path.with_extension("json.migrating");
            let written = fs::write(&temporary, &rewritten).and_then(|()| fs::rename(&temporary, &path));
            match written {
                Ok(()) => log::info!("updated the folders in {}", path.display()),
                Err(e) => { let _ = fs::remove_file(&temporary); log::warn!("update {}: {e}", path.display()); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

    fn temp(name: &str) -> Temp {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("lkt-identity-{name}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    #[test]
    fn the_old_folders_move_to_the_new_identifier_and_settings_follow_them() {
        let base = temp("move");
        let config = base.0.join("config").join("app.lookitan");
        let data = base.0.join("data").join("app.lookitan");
        let old_config = config.with_file_name(OLD_IDENTIFIER);
        let old_data = data.with_file_name(OLD_IDENTIFIER);
        fs::create_dir_all(old_data.join("dictionaries/content/d1")).unwrap();
        fs::write(old_data.join("dictionaries/content/d1/words.mdx"), b"mdx").unwrap();
        fs::create_dir_all(&old_config).unwrap();
        let installed = old_data.join("dictionaries").join("content").join("d1");
        let elsewhere = base.0.join("Downloads").join("other.mdx");
        let settings = serde_json::json!({"dictionaries": [
            {"id": "d1", "path": installed.to_string_lossy()},
            {"id": "d2", "path": elsewhere.to_string_lossy()},
        ]}).to_string();
        fs::write(old_config.join("settings.json"), &settings).unwrap();

        migrate(&[config.clone(), data.clone(), data.clone()]);
        assert!(!old_config.exists() && !old_data.exists(), "nothing is left under the old name");
        assert_eq!(fs::read(data.join("dictionaries/content/d1/words.mdx")).unwrap(), b"mdx");
        let saved: serde_json::Value = serde_json::from_str(&fs::read_to_string(config.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["dictionaries"][0]["path"], data.join("dictionaries").join("content").join("d1").to_string_lossy().into_owned());
        assert_eq!(saved["dictionaries"][1]["path"], elsewhere.to_string_lossy().into_owned(), "a path elsewhere is left alone");

        // A second start finds nothing to move and changes nothing.
        let before = fs::read_to_string(config.join("settings.json")).unwrap();
        migrate(&[config.clone(), data.clone()]);
        assert_eq!(fs::read_to_string(config.join("settings.json")).unwrap(), before);
    }

    #[test]
    fn a_half_moved_folder_is_completed_without_overwriting_the_new_files() {
        let base = temp("merge");
        let old = base.0.join(OLD_IDENTIFIER);
        let new = base.0.join("app.lookitan");
        fs::create_dir_all(old.join("ocr-models")).unwrap();
        fs::write(old.join("ocr-models/model.onnx"), b"model").unwrap();
        fs::write(old.join("words.json"), b"old words").unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("words.json"), b"new words").unwrap();
        assert!(move_folder(&old, &new).unwrap());
        assert_eq!(fs::read(new.join("ocr-models/model.onnx")).unwrap(), b"model");
        assert_eq!(fs::read(new.join("words.json")).unwrap(), b"new words", "the new folder's file wins");
        assert_eq!(fs::read(old.join("words.json")).unwrap(), b"old words", "and the old one is not lost");
        assert!(!move_folder(&base.0.join("missing"), &new).unwrap());
    }

    #[test]
    fn paths_are_rewritten_only_as_whole_folders_in_both_path_styles() {
        let moves = [(PathBuf::from("/home/u/.local/share/io.github.arrowpoppa.lookitan"), PathBuf::from("/home/u/.local/share/app.lookitan"))];
        let text = r#"{"a":"/home/u/.local/share/io.github.arrowpoppa.lookitan/dictionaries/x","b":"/home/u/.local/share/io.github.arrowpoppa.lookitan","c":"/home/u/.local/share/io.github.arrowpoppa.lookitan-backup/y"}"#;
        assert_eq!(rewrite_paths(text, &moves),
            r#"{"a":"/home/u/.local/share/app.lookitan/dictionaries/x","b":"/home/u/.local/share/app.lookitan","c":"/home/u/.local/share/io.github.arrowpoppa.lookitan-backup/y"}"#);
        let windows = [(PathBuf::from(r"C:\Users\u\AppData\Local\io.github.arrowpoppa.lookitan"), PathBuf::from(r"C:\Users\u\AppData\Local\app.lookitan"))];
        let text = r#"{"a":"C:\\Users\\u\\AppData\\Local\\io.github.arrowpoppa.lookitan\\dictionaries\\x"}"#;
        assert_eq!(rewrite_paths(text, &windows), r#"{"a":"C:\\Users\\u\\AppData\\Local\\app.lookitan\\dictionaries\\x"}"#);
    }
}
