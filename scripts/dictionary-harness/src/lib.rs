// Compile the application's actual readers without Tauri, OCR, models or a desktop session.
// This target is run on GitHub Actions; no Rust builds run on the user's low-memory PC.
#[path = "../../../src-tauri/src/dictionaries/mod.rs"]
pub mod dictionaries;
#[path = "../../../src-tauri/src/mdict.rs"]
pub mod mdict;
#[path = "../../../src-tauri/src/dict.rs"]
pub mod dict;
