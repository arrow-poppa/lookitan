//! Webster's Unabridged Dictionary (1913) as Project Gutenberg's plain text (ebook #29765), one of
//! the formats Lookupper reads. Between the Gutenberg start and end markers, an article begins
//! with its headword line in capitals (variants separated by `; `), after a blank line and
//! followed by the line with the pronounced form; it runs until the next headword line.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use memmap2::Mmap;

use super::lkidx::{Builder, Location};
use super::escape_html;

pub const FORMAT: &str = "webster-text";
const START: &[u8] = b"*** START OF THE PROJECT GUTENBERG";
const END: &[u8] = b"*** END OF THE PROJECT GUTENBERG";

/// Whether `path` is a Gutenberg text: the start marker within its first 8 KiB, as Lookupper
/// checks.
pub fn detect(path: &Path) -> bool {
    let mut head = vec![0u8; 8192];
    let Ok(n) = File::open(path).and_then(|mut f| f.read(&mut head)) else { return false };
    head[..n].windows(START.len()).any(|w| w == START)
}

fn trim_line(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// A headword line: capitals, digits and the punctuation of compound headwords only.
fn is_headword(line: &str) -> bool {
    line.chars().any(|c| c.is_alphabetic())
        && line.chars().all(|c| (c.is_alphabetic() && !c.is_lowercase()) || c.is_ascii_digit() || " ;-'.,&".contains(c))
        && line.len() < 120
        // Roman numerals and abbreviations that open a paragraph ("II.") are not headwords.
        && !line.ends_with('.')
}

/// "ABATIS; ABATTIS" → ["Abatis", "Abattis"].
fn variants(line: &str) -> Vec<String> {
    line.split(';').map(str::trim).filter(|s| !s.is_empty()).map(|s| {
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i == 0 { out.push(c); } else { out.extend(c.to_lowercase()); }
        }
        out
    }).collect()
}

/// Indexes the file; returns the builder, a title and a description.
pub fn build(path: &Path, progress: &dyn Fn(usize)) -> Result<(Builder, String, String)> {
    let file = File::open(path)?;
    // SAFETY: the file is only read, and a change makes the index stale (its stamp changes).
    let map = unsafe { Mmap::map(&file)? };
    let bytes: &[u8] = &map;
    let start = bytes.windows(START.len()).position(|w| w == START).context("Webster: marcador de início do Gutenberg ausente")?;
    let body_start = start + bytes[start..].iter().position(|b| *b == b'\n').map(|p| p + 1).unwrap_or(0);
    let body_end = bytes[body_start..].windows(END.len()).position(|w| w == END).map(|p| body_start + p).unwrap_or(bytes.len());
    // Line starts in the body.
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut at = body_start;
    while at < body_end {
        let end = bytes[at..body_end].iter().position(|b| *b == b'\n').map(|p| at + p + 1).unwrap_or(body_end);
        lines.push((at, end));
        at = end;
    }
    let text = |i: usize| -> String { String::from_utf8_lossy(trim_line(&bytes[lines[i].0..lines[i].1])).into_owned() };
    let mut heads: Vec<usize> = Vec::new();
    for i in 0..lines.len() {
        let line = text(i);
        let blank_before = i == 0 || text(i - 1).trim().is_empty();
        let followed = i + 1 < lines.len() && !text(i + 1).trim().is_empty();
        if blank_before && followed && is_headword(line.trim()) { heads.push(i); }
    }
    let mut builder = Builder::new();
    for (n, &head) in heads.iter().enumerate() {
        let from = lines[head].0;
        let to = heads.get(n + 1).map(|&next| lines[next].0).unwrap_or(body_end);
        let names = variants(text(head).trim());
        let Some(title) = names.first() else { continue };
        let aliases: Vec<&str> = names[1..].iter().map(String::as_str).collect();
        builder.add(title, Location { file: 0, kind: 0, offset: from as u64, length: u32::try_from(to - from)? }, &aliases)?;
        if builder.len() % 1000 == 0 { progress(builder.len()); }
    }
    Ok((builder, "Webster's Unabridged Dictionary (1913)".to_string(), "Project Gutenberg".to_string()))
}

/// An article as HTML: the pronounced form, then its paragraphs, with the Defn:/Etym:/Syn./Note:
/// labels marked.
pub fn article_html(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    let mut lines = text.lines();
    let head = lines.next().unwrap_or("").trim();
    let mut html = format!("<div class=\"webster-entry\"><h2>{}</h2>", escape_html(variants(head).join("; ").as_str()));
    let rest: Vec<&str> = lines.collect();
    let body = rest.join("\n");
    for (i, paragraph) in body.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).enumerate() {
        let joined = paragraph.split('\n').map(str::trim).collect::<Vec<_>>().join(" ");
        let mut p = escape_html(&joined);
        for label in ["Defn:", "Etym:", "Syn.", "Note:"] {
            p = p.replace(label, &format!("<b>{label}</b>"));
        }
        if i == 0 { html.push_str(&format!("<p class=\"pronunciation\">{p}</p>")); } else { html.push_str(&format!("<p>{p}</p>")); }
    }
    html.push_str("</div>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionaries::lkidx::{self, Stamp};
    use std::path::PathBuf;

    /// The shape of Project Gutenberg's ebook #29765, with CRLF line ends as it is published.
    const SAMPLE: &str = "The Project Gutenberg eBook of Webster's Unabridged Dictionary\r\n\r\n*** START OF THE PROJECT GUTENBERG EBOOK WEBSTER'S UNABRIDGED DICTIONARY ***\r\n\r\n\r\nABACK\r\nA*back\", adv. Etym: [Pref. a- + back; AS. on b\u{e6}c at, on, or toward\r\nthe back. See Back.]\r\n\r\n1. Toward the back or rear; backward.\r\n\r\nABATIS; ABATTIS\r\nAb\"a*tis, Ab\"at*tis, n. Etym: [F. abatis.] (Mil.)\r\n\r\nDefn: A means of defense formed by felled trees.\r\n\r\n*** END OF THE PROJECT GUTENBERG EBOOK WEBSTER'S UNABRIDGED DICTIONARY ***\r\nLicense text\r\n";

    #[test]
    fn headword_lines_start_articles_that_are_read_from_the_text() {
        let d = std::env::temp_dir().join(format!("lkt-webster-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("pg29765.txt");
        std::fs::write(&path, SAMPLE).unwrap();
        assert!(detect(&path));
        let stamp = Stamp::of(&d, &[PathBuf::from("pg29765.txt")]).unwrap();
        let index = lkidx::open_or_build(&d.join("p.lkidx"), FORMAT, &stamp, || build(&path, &|_| {})).unwrap();
        assert_eq!(index.len(), 2);
        let read = |id: u32| lkidx::read(&d, &index.stamp, index.location(id).unwrap(), 1 << 20).unwrap();
        let aback = article_html(&read(index.find("aback", 1)[0]));
        assert!(aback.contains("<h2>Aback</h2>") && aback.contains("Toward the back or rear") && aback.contains("<b>Etym:</b>"), "{aback}");
        assert!(!aback.contains("ABATIS"), "an article stops at the next headword");
        let abattis = index.find("abattis", 1);
        assert_eq!(abattis, index.find("abatis", 1), "variants share the article");
        let html = article_html(&read(abattis[0]));
        assert!(html.contains("<b>Defn:</b> A means of defense") && !html.contains("END OF THE PROJECT"), "{html}");
        assert!(!is_headword("Defn: A means") && !is_headword("1.") && is_headword("ABATIS; ABATTIS"));
        drop(index);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
