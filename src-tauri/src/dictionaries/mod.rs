// Independent dictionary readers. The installed Lookupper provides behavioral references;
// its implementation and its private index format are not dependencies of this engine.
pub mod containers;
pub mod dictzip;
pub mod hunspell;
pub mod inplace;
pub mod lkidx;
pub mod lexicons;
pub mod morphology;
pub mod slob;
pub mod source;
pub mod store;
pub mod yomitan;

#[cfg(test)]
mod engine_tests;

use anyhow::Result;

pub struct ImportInfo {
    pub name: String,
    pub format: &'static str,
    pub description: String,
}

/// Parsers emit one article at a time, so importing a dictionary never retains all its HTML.
pub trait ImportSink {
    fn add(&mut self, headword: &str, html: &str, aliases: &[String]) -> Result<()>;
    fn add_resource(&mut self, _key: &str, _reference: u32) -> Result<()> { Ok(()) }
    /// The entries added next come from `length` bytes at `offset` of source file `file` (offsets
    /// in the decompressed stream). Readers that report spans are read in place (inplace.rs).
    fn span(&mut self, _file: u16, _offset: u64, _length: u64) {}
}

pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        .replace('"', "&quot;").replace('\'', "&#39;")
}

pub fn entry_link(word: &str) -> String {
    let mut url = String::from("entry://");
    for b in word.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            url.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(url, "%{b:02X}");
        }
    }
    url
}

pub fn folded(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    text.trim().nfkc().flat_map(char::to_lowercase)
        .map(|c| if matches!(c, '\u{2018}' | '\u{2019}') { '\'' } else { c }).collect()
}
