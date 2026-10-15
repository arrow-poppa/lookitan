//! GCIDE, the GNU Collaborative International Dictionary of English, in its distribution files
//! CIDE.A … CIDE.Z: the form Lookupper reads. An article is a `<p>` holding `<ent>` headwords,
//! plus the paragraphs after it up to the next one with `<ent>`. The markup is SGML-like: tags
//! such as `<hw>…</hw>`, character entities written `<name/`, and old font escapes `\'xx`.
//!
//! The character table comes from GCIDE's webfont.txt (GPL-3.0-or-later, © Patrick J. Cassidy and
//! the GCIDE contributors); vowel marks it describes by pattern (`<xmac/`, `<xcr/`, `<xsdot/`, …)
//! are composed with Unicode combining marks.

use std::path::{Path, PathBuf};

use anyhow::{ensure, Result};
use unicode_normalization::UnicodeNormalization;

use super::lkidx::{Builder, Location};
use super::{entry_link, escape_html};

pub const FORMAT: &str = "gcide";

/// The CIDE.A … CIDE.Z files beside `path`, in order.
pub fn files(path: &Path) -> Vec<PathBuf> {
    let dir = if path.is_dir() { path } else { path.parent().unwrap_or(Path::new(".")) };
    (b'A'..=b'Z').map(|c| dir.join(format!("CIDE.{}", c as char))).filter(|p| p.is_file()).collect()
}

/// Whether `path` is (or is beside) CIDE.A, which is how Lookupper recognizes GCIDE.
pub fn detect(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    (path.is_dir() || name.starts_with("CIDE.")) && !files(path).is_empty()
}

/// webfont.txt's entities, by name.
const ENTITIES: &[(&str, &str)] = &[
    ("Cced", "Ç"), ("uum", "ü"), ("eacute", "é"), ("acir", "â"), ("aum", "ä"), ("agrave", "à"), ("aring", "å"),
    ("cced", "ç"), ("ecir", "ê"), ("eum", "ë"), ("egrave", "è"), ("ium", "ï"), ("icir", "î"), ("igrave", "ì"),
    ("Aum", "Ä"), ("Eacute", "É"), ("ae", "æ"), ("AE", "Æ"), ("ocir", "ô"), ("oum", "ö"), ("ograve", "ò"),
    ("ucir", "û"), ("ugrave", "ù"), ("yum", "ÿ"), ("Oum", "Ö"), ("Uum", "Ü"), ("pound", "£"), ("aacute", "á"),
    ("iacute", "í"), ("oacute", "ó"), ("uacute", "ú"), ("ntil", "ñ"), ("Ntil", "Ñ"), ("frac23", "⅔"), ("frac13", "⅓"),
    ("sec", "″"), ("frac12", "½"), ("frac14", "¼"), ("frac34", "¾"), ("?", "(?)"), ("hand", "☞"), ("bprime", "″"),
    ("prime", "′"), ("rdquo", "”"), ("sect", "§"), ("ldquo", "“"), ("amac", "ā"), ("lsquo", "‘"), ("rsquo", "’"),
    ("nsm", "ṉ"), ("sharp", "♯"), ("flat", "♭"), ("th", "th"), ("imac", "ī"), ("emac", "ē"), ("dsdot", "ḍ"),
    ("nsdot", "ṇ"), ("tsdot", "ṭ"), ("ecr", "ĕ"), ("icr", "ĭ"), ("ocr", "ŏ"), ("OE", "Œ"), ("oe", "œ"),
    ("omac", "ō"), ("umac", "ū"), ("ocar", "ǒ"), ("aemac", "ǣ"), ("oemac", "œ̄"), ("ucr", "ŭ"), ("acr", "ă"),
    ("cre", "˘"), ("ymac", "ȳ"), ("adot", "ȧ"), ("edh", "ð"), ("thorn", "þ"), ("atil", "ã"), ("ndot", "ṅ"),
    ("rsdot", "ṛ"), ("yogh", "ȝ"), ("mdash", "—"), ("divide", "÷"), ("deg", "°"), ("middot", "•"), ("root", "√"),
    ("dag", "†"), ("ddag", "‡"), ("para", "¶"), ("gt", ">"), ("lt", "<"), ("schwa", "ə"), ("rarr", "→"),
    ("and", "and"), ("or", "or"), ("oomac", "o͞o"), ("oocr", "o͝o"), ("asterism", "⁂"), ("ounceap", "℥"),
    ("sigmat", "ς"), ("digamma", "ϝ"), ("br", "\n"),
];

/// Greek letters, named in some entities.
const GREEK: &[(&str, char)] = &[
    ("alpha", 'α'), ("beta", 'β'), ("gamma", 'γ'), ("delta", 'δ'), ("epsilon", 'ε'), ("zeta", 'ζ'), ("eta", 'η'),
    ("theta", 'θ'), ("iota", 'ι'), ("kappa", 'κ'), ("lambda", 'λ'), ("mu", 'μ'), ("nu", 'ν'), ("xi", 'ξ'),
    ("omicron", 'ο'), ("pi", 'π'), ("rho", 'ρ'), ("sigma", 'σ'), ("tau", 'τ'), ("upsilon", 'υ'), ("phi", 'φ'),
    ("chi", 'χ'), ("psi", 'ψ'), ("omega", 'ω'),
];

/// The old font escapes `\'xx` that webfont.txt maps, by code.
const ESCAPES: &[(u8, &str)] = &[
    (0x80, "Ç"), (0x81, "ü"), (0x82, "é"), (0x83, "â"), (0x84, "ä"), (0x85, "à"), (0x86, "å"), (0x87, "ç"),
    (0x88, "ê"), (0x89, "ë"), (0x8a, "è"), (0x8b, "ï"), (0x8c, "î"), (0x8d, "ì"), (0x8e, "Ä"), (0x8f, "Å"),
    (0x90, "É"), (0x91, "æ"), (0x92, "Æ"), (0x93, "ô"), (0x94, "ö"), (0x95, "ò"), (0x96, "û"), (0x97, "ù"),
    (0x98, "ÿ"), (0x99, "Ö"), (0x9a, "Ü"), (0x9c, "£"), (0xa0, "á"), (0xa1, "í"), (0xa2, "ó"), (0xa3, "ú"),
    (0xa4, "ñ"), (0xa5, "Ñ"), (0xa6, "⅔"), (0xa7, "⅓"), (0xa9, "″"), (0xab, "½"), (0xac, "¼"), (0xb0, "(?)"),
    (0xb4, "↑"), (0xb5, "☞"), (0xb6, "″"), (0xb7, "′"), (0xb8, "”"), (0xba, "‖"), (0xbc, "§"), (0xbd, "“"),
    (0xbe, "ā"), (0xbf, "‘"), (0xc0, "ṉ"), (0xc1, "♯"), (0xc2, "♭"), (0xc3, "–"), (0xc4, "―"), (0xc5, "t"),
    (0xc6, "ī"), (0xc7, "ē"), (0xc8, "ḍ"), (0xc9, "ṇ"), (0xca, "ṭ"), (0xcb, "ĕ"), (0xcc, "ĭ"), (0xce, "ŏ"),
    (0xcf, "‐"), (0xd0, "—"), (0xd1, "Œ"), (0xd2, "œ"), (0xd3, "ō"), (0xd4, "ū"), (0xd5, "ǒ"), (0xd6, "ǣ"),
    (0xd7, "œ̄"), (0xd8, "‖"), (0xdc, "ŭ"), (0xdd, "ă"), (0xde, "˘"), (0xdf, "ȳ"), (0xe0, "ā"), (0xe1, "ē"),
    (0xe2, "ī"), (0xe3, "ō"), (0xe4, "ū"), (0xe5, "ȧ"), (0xe6, "μ"), (0xe7, "h"), (0xeb, "ð"), (0xed, "þ"),
    (0xee, "ã"), (0xef, "ṅ"), (0xf0, "ṛ"), (0xf4, "ȝ"), (0xf5, "—"), (0xf6, "÷"), (0xf7, "≈"), (0xf8, "°"),
    (0xf9, "•"), (0xfa, "·"), (0xfb, "√"),
];

/// A `<name/` entity as text.
fn entity(name: &str) -> String {
    if let Some((_, c)) = ENTITIES.iter().find(|(n, _)| *n == name) { return c.to_string(); }
    if let Some((_, c)) = GREEK.iter().find(|(n, _)| *n == name) { return c.to_string(); }
    // Marked letters: the letter, then the mark (webfont.txt's patterns).
    let marks: &[(&str, char)] = &[
        ("acute", '\u{301}'), ("grave", '\u{300}'), ("circ", '\u{302}'), ("cir", '\u{302}'), ("uml", '\u{308}'),
        ("um", '\u{308}'), ("til", '\u{303}'), ("smac", '\u{304}'), ("mac", '\u{304}'), ("cr", '\u{306}'),
        ("sdot", '\u{323}'), ("dot", '\u{307}'), ("ring", '\u{30a}'), ("car", '\u{30c}'), ("ced", '\u{327}'),
        ("sl", '\u{304}'), ("dd", '\u{324}'),
    ];
    let mut chars = name.chars();
    if let Some(letter) = chars.next().filter(|c| c.is_alphabetic()) {
        let rest = chars.as_str();
        if rest == "it" { return letter.to_string(); }
        if let Some((_, mark)) = marks.iter().find(|(suffix, _)| *suffix == rest) {
            return [letter, *mark].iter().collect::<String>().nfc().collect();
        }
    }
    String::new()
}

/// How a tag is shown: HTML to open and close it, or hidden.
fn tag(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "ent" | "source" => return None,
        "p" => ("<p>", "</p>"),
        "hw" => ("<b class=\"gcide-hw\">", "</b>"),
        "pr" => ("<span class=\"pronunciation\">", "</span>"),
        "pos" | "fld" | "sd" | "ets" | "xex" | "qex" | "ex" | "it" | "i" | "grk" | "spn" | "altname" | "contr" | "plw" | "conjf" | "wf" => ("<i>", "</i>"),
        "def" | "def2" | "cd" => ("<span class=\"gcide-def\">", "</span>"),
        "sn" | "b" | "col" | "mcol" | "mhw" => ("<b>", "</b>"),
        "ety" => ("<span class=\"gcide-ety\">", "</span>"),
        "q" => ("<span class=\"gcide-q\">", "</span>"),
        "au" | "qau" => ("<span class=\"gcide-au\">", "</span>"),
        "rj" => (" <span class=\"gcide-rj\">", "</span>"),
        "mark" | "usage" => ("<small>", "</small>"),
        "cs" | "syn" | "note" | "centered" => ("<div>", "</div>"),
        "sc" => ("<span style=\"font-variant:small-caps\">", "</span>"),
        _ => ("", ""),
    })
}

/// Converts GCIDE markup to HTML; `plain` returns only the text (for headwords).
fn convert(source: &str, plain: bool) -> String {
    let mut out = String::new();
    let mut hidden = 0usize;
    // While inside <hw>, `*` marks syllables and `"`/`` ` `` the main and secondary stress.
    let mut in_hw = false;
    // The text of the <er> cross reference being read, which becomes a link.
    let mut reference: Option<String> = None;
    let mut rest = source;
    let push = |out: &mut String, reference: &mut Option<String>, hidden: usize, text: &str| {
        if hidden > 0 { return; }
        if let Some(r) = reference.as_mut() { r.push_str(text); return; }
        if plain { out.push_str(text); } else { out.push_str(&escape_html(text)); }
    };
    while !rest.is_empty() {
        if let Some(comment) = rest.strip_prefix("<--") {
            rest = comment.find("-->").map(|e| &comment[e + 3..]).unwrap_or("");
            continue;
        }
        if let Some(after) = rest.strip_prefix('<') {
            let closing = after.starts_with('/');
            let body = if closing { &after[1..] } else { after };
            let name_len = body.find(|c: char| !(c.is_ascii_alphanumeric() || c == '?')).unwrap_or(body.len());
            let name = &body[..name_len];
            let tail = &body[name_len..];
            if !name.is_empty() && tail.starts_with('>') {
                rest = &tail[1..];
                match (closing, name) {
                    (false, "hw") => in_hw = true,
                    (true, "hw") => in_hw = false,
                    _ => {}
                }
                if name == "er" && !plain {
                    if closing {
                        if let Some(text) = reference.take() {
                            let target = text.trim();
                            if hidden == 0 { out.push_str(&format!("<a href=\"{}\">{}</a>", entry_link(target), escape_html(target))); }
                        }
                    } else if hidden == 0 {
                        reference = Some(String::new());
                    }
                    continue;
                }
                match tag(name) {
                    None => { if closing { hidden = hidden.saturating_sub(1); } else { hidden += 1; } }
                    Some((open, close)) if !plain && hidden == 0 => out.push_str(if closing { close } else { open }),
                    _ => {}
                }
                continue;
            }
            if !closing && !name.is_empty() && tail.starts_with('/') {
                rest = &tail[1..];
                let text = entity(name);
                if text == "\n" {
                    if !plain && hidden == 0 && reference.is_none() { out.push_str("<br>"); }
                } else {
                    push(&mut out, &mut reference, hidden, &text);
                }
                continue;
            }
            push(&mut out, &mut reference, hidden, "<");
            rest = after;
            continue;
        }
        if let Some(escape) = rest.strip_prefix("\\'") {
            if let Some(code) = escape.get(..2).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                let text = ESCAPES.iter().find(|(c, _)| *c == code).map(|(_, t)| *t).unwrap_or("");
                push(&mut out, &mut reference, hidden, text);
                rest = &escape[2..];
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        rest = &rest[c.len_utf8()..];
        let text = match c {
            '*' if in_hw => "·".to_string(),
            '"' if in_hw => "″".to_string(),
            '`' if in_hw => "′".to_string(),
            '\n' => " ".to_string(),
            c => c.to_string(),
        };
        push(&mut out, &mut reference, hidden, &text);
    }
    out
}

/// The headwords of a paragraph: the text of its `<ent>` elements.
fn headwords(paragraph: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = paragraph;
    while let Some(start) = rest.find("<ent>") {
        let after = &rest[start + 5..];
        let Some(end) = after.find("</ent>") else { break };
        let word = convert(&after[..end], true).trim().to_string();
        if !word.is_empty() && !out.contains(&word) { out.push(word); }
        rest = &after[end + 6..];
    }
    out
}

/// Indexes the CIDE files; returns the builder, a title and a description.
pub fn build(files: &[PathBuf], progress: &dyn Fn(usize)) -> Result<(Builder, String, String)> {
    ensure!(!files.is_empty(), "No CIDE.* file in the dictionary folder.");
    let mut builder = Builder::new();
    for (file, path) in files.iter().enumerate() {
        let bytes = std::fs::read(path)?;
        let text = String::from_utf8_lossy(&bytes);
        // The article being read: its start, its end so far and its headwords.
        let mut article: Option<(usize, usize, Vec<String>)> = None;
        let flush = |builder: &mut Builder, article: &mut Option<(usize, usize, Vec<String>)>| -> Result<()> {
            if let Some((start, end, words)) = article.take() {
                if let Some(title) = words.first() {
                    let aliases: Vec<&str> = words[1..].iter().map(String::as_str).collect();
                    builder.add(title, Location { file: file as u16, kind: 0, offset: start as u64, length: u32::try_from(end - start)? }, &aliases)?;
                }
            }
            Ok(())
        };
        let mut at = 0;
        while let Some(found) = text[at..].find("<p>") {
            let start = at + found;
            let Some(close) = text[start..].find("</p>") else { break };
            let end = start + close + 4;
            let words = headwords(&text[start..end]);
            if !words.is_empty() {
                flush(&mut builder, &mut article)?;
                article = Some((start, end, words));
            } else if let Some(a) = article.as_mut() {
                a.1 = end;
            }
            at = end;
        }
        flush(&mut builder, &mut article)?;
        progress(builder.len());
    }
    Ok((builder, "GNU Collaborative International Dictionary of English".to_string(), "GCIDE (Webster 1913 e acréscimos)".to_string()))
}

/// An article as HTML.
pub fn article_html(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let title = headwords(&text).into_iter().next().unwrap_or_default();
    format!("<div class=\"gcide-entry\"><h2>{}</h2>{}</div>", escape_html(&title), convert(&text, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionaries::lkidx::{self, Stamp};

    /// From CIDE.A of GCIDE 0.53.
    const SAMPLE: &str = "<-- This file is part 1 of the GNU version of GCIDE -->\n\n\
<p><ent>Aback</ent><br/\n<hw>A*back\"</hw> <pr>(<adot/*b<acr/k\")</pr>, <pos>adv.</pos> <ety>[Pref. <ets>a-</ets> + <ets>back</ets>; AS. <ets>on b<ae/c</ets> at, on, or toward the back. See <er>Back</er>.]</ety> <sn>1.</sn> <def>Toward the back or rear; backward.</def> <ldquo/Therewith <xex>aback</xex> she started.<rdquo/  <rj><au>Chaucer.</au></rj><br/\n[<source>1913 Webster</source>]</p>\n\n\
<p><sn>2.</sn> <def>Behind; in the rear.</def>  <rj><au>Knolles.</au></rj><br/\n[<source>1913 Webster</source>]</p>\n\n\
<p><ent>Caf<eacute/</ent><br/\n<hw>Caf<eacute/</hw> <def>A coffee house; a caf\\'82 in the French manner.</def></p>\n";

    #[test]
    fn articles_run_until_the_next_headword_and_entities_become_characters() {
        let d = std::env::temp_dir().join(format!("lkt-cide-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("CIDE.A"), SAMPLE).unwrap();
        std::fs::write(d.join("CIDE.B"), "<p><ent>Bee</ent><br/\n<hw>Bee</hw> <def>An insect.</def></p>\n").unwrap();
        assert!(detect(&d.join("CIDE.A")));
        let files = files(&d.join("CIDE.A"));
        assert_eq!(files.len(), 2);
        let names: Vec<PathBuf> = files.iter().map(|p| PathBuf::from(p.file_name().unwrap())).collect();
        let stamp = Stamp::of(&d, &names).unwrap();
        let index = lkidx::open_or_build(&d.join("c.lkidx"), FORMAT, &stamp, || build(&files, &|_| {})).unwrap();
        assert_eq!(index.len(), 3);
        let read = |id: u32| lkidx::read(&d, &index.stamp, index.location(id).unwrap(), 1 << 20).unwrap();
        let aback = article_html(&read(index.find("aback", 1)[0]));
        assert!(aback.contains("<h2>Aback</h2>") && aback.contains("A·back″") && aback.contains("Behind; in the rear."), "{aback}");
        assert!(aback.contains("ȧ") && aback.contains("ă") && aback.contains("bæc") && aback.contains("“Therewith"), "{aback}");
        assert!(aback.contains(&entry_link("Back")) && !aback.contains("1913 Webster") && !aback.contains("<ent>"), "{aback}");
        let cafe = article_html(&read(index.find("Café", 1)[0]));
        assert!(cafe.contains("a café in the French manner"), "escapes map to characters: {cafe}");
        assert!(article_html(&read(index.find("bee", 1)[0])).contains("An insect."), "a later CIDE file");
        assert_eq!(entity("omac"), "ō");
        assert_eq!(entity("esl"), "ē");
        assert_eq!(entity("usdot"), "ụ");
        drop(index);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
