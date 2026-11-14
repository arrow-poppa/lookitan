//! Dictionary colours made for a white page, shown on the pop-up's own background.
//!
//! Every colour a dictionary sets (in its stylesheets, `<style>` blocks, `style` attributes and the
//! old `color`/`bgcolor` attributes) becomes `light-dark(original, dark)`: the light theme keeps the
//! dictionary's own colours, the dark theme gets a readable colour of the same hue. Near-white
//! backgrounds become transparent in both, so an entry sits on the pop-up's background instead of
//! a white (or inverted) page. Pictures are never touched. The original declaration stays in front
//! of the rewritten one, so a WebView without `light-dark()` still shows the dictionary's colours.
//!
//! This replaced (2026-11-14, at the user's request) the old invert filter over MDict and Slob
//! pages; it costs one pass over the text when an entry or stylesheet is served, and keeps nothing.

/// What a colour paints: text-like marks (text, borders, shadows) or an area behind them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role { Text, Background }

/// Backgrounds at least this light are the page itself and become transparent.
const PAGE_LIGHTNESS: f32 = 0.85;
/// Dark-theme text is at least this light (HSL lightness) and at most `TEXT_MAX`.
const TEXT_MIN: f32 = 0.62;
const TEXT_MAX: f32 = 0.92;

fn role_of(property: &str) -> Option<Role> {
    let p = property.strip_prefix("-webkit-").unwrap_or(property);
    Some(match p {
        "background" | "background-color" => Role::Background,
        "color" | "border" | "border-color" | "border-top" | "border-right" | "border-bottom" | "border-left"
        | "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color"
        | "border-block" | "border-inline" | "border-block-color" | "border-inline-color"
        | "outline" | "outline-color" | "text-decoration" | "text-decoration-color" | "text-emphasis"
        | "text-emphasis-color" | "column-rule" | "column-rule-color" | "caret-color" | "fill" | "stroke"
        | "box-shadow" | "text-shadow" | "text-fill-color" | "text-stroke" | "text-stroke-color"
        | "stop-color" | "flood-color" | "lighting-color" | "accent-color" => Role::Text,
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Rgba { r: f32, g: f32, b: f32, a: f32 }

fn hsl(c: Rgba) -> (f32, f32, f32) {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    let l = (max + min) / 2.0;
    if max == min { return (0.0, 0.0, l); }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == c.r { (c.g - c.b) / d + if c.g < c.b { 6.0 } else { 0.0 } }
        else if max == c.g { (c.b - c.r) / d + 2.0 } else { (c.r - c.g) / d + 4.0 };
    (h / 6.0, s, l)
}

fn from_hsl(h: f32, s: f32, l: f32, a: f32) -> Rgba {
    if s == 0.0 { return Rgba { r: l, g: l, b: l, a }; }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let channel = |mut t: f32| {
        if t < 0.0 { t += 1.0; }
        if t > 1.0 { t -= 1.0; }
        if t < 1.0 / 6.0 { p + (q - p) * 6.0 * t }
        else if t < 0.5 { q }
        else if t < 2.0 / 3.0 { p + (q - p) * (2.0 / 3.0 - t) * 6.0 }
        else { p }
    };
    Rgba { r: channel(h + 1.0 / 3.0), g: channel(h), b: channel(h - 1.0 / 3.0), a }
}

/// The dark-theme colour, or None when the colour is the page itself (transparent in both themes).
fn dark(c: Rgba, role: Role) -> Option<Rgba> {
    let (h, s, l) = hsl(c);
    match role {
        // Dark text for a white page becomes light text of the same hue; light text stays.
        Role::Text => Some(if l < TEXT_MIN { from_hsl(h, s, (1.0 - l).clamp(TEXT_MIN, TEXT_MAX), c.a) } else { c }),
        Role::Background if c.a > 0.0 && l >= PAGE_LIGHTNESS => None,
        // A light tint (a highlighted box) becomes a dark tint of the same hue; dark areas stay.
        Role::Background if l >= 0.5 => Some(from_hsl(h, s * 0.6, 0.12 + (1.0 - l) * 0.35, c.a)),
        Role::Background => Some(c),
    }
}

fn css_color(c: Rgba) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    if c.a >= 1.0 { format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b)) }
    else { format!("rgba({},{},{},{})", byte(c.r), byte(c.g), byte(c.b), (c.a.clamp(0.0, 1.0) * 1000.0).round() / 1000.0) }
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|v| v as f32 * 17.0 / 255.0);
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
    match hex.len() {
        3 => Some(Rgba { r: digit(0)?, g: digit(1)?, b: digit(2)?, a: 1.0 }),
        4 => Some(Rgba { r: digit(0)?, g: digit(1)?, b: digit(2)?, a: digit(3)? }),
        6 => Some(Rgba { r: pair(0)?, g: pair(2)?, b: pair(4)?, a: 1.0 }),
        8 => Some(Rgba { r: pair(0)?, g: pair(2)?, b: pair(4)?, a: pair(6)? }),
        _ => None,
    }
}

/// A number, or a percentage of `scale` (`255` for rgb channels, `1` for alpha and hsl).
fn number(text: &str, percent_of: f32) -> Option<f32> {
    let text = text.trim();
    if let Some(p) = text.strip_suffix('%') { return p.trim().parse::<f32>().ok().map(|v| v / 100.0 * percent_of); }
    text.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn parse_function(name: &str, args: &str) -> Option<Rgba> {
    let (main, slash_alpha) = match args.split_once('/') { Some((m, a)) => (m, Some(a)), None => (args, None) };
    let parts: Vec<&str> = main.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).collect();
    let alpha = |extra: Option<&str>| -> Option<f32> {
        match slash_alpha.or(extra) { Some(a) => number(a, 1.0).map(|v| v.clamp(0.0, 1.0)), None => Some(1.0) }
    };
    match name {
        "rgb" | "rgba" if parts.len() == 3 || parts.len() == 4 => Some(Rgba {
            r: number(parts[0], 255.0)? / 255.0, g: number(parts[1], 255.0)? / 255.0, b: number(parts[2], 255.0)? / 255.0,
            a: alpha(parts.get(3).copied())?,
        }),
        "hsl" | "hsla" if parts.len() == 3 || parts.len() == 4 => {
            let hue = parts[0].trim();
            let h = if let Some(v) = hue.strip_suffix("deg") { v.parse::<f32>().ok()? / 360.0 }
                else if let Some(v) = hue.strip_suffix("turn") { v.parse::<f32>().ok()? }
                else if let Some(v) = hue.strip_suffix("rad") { v.parse::<f32>().ok()? / std::f32::consts::TAU }
                else { hue.parse::<f32>().ok()? / 360.0 };
            let s = number(parts[1], 1.0)?.clamp(0.0, 1.0);
            let l = number(parts[2], 1.0)?.clamp(0.0, 1.0);
            Some(from_hsl(h.rem_euclid(1.0), s, l, alpha(parts.get(3).copied())?))
        }
        _ => None,
    }
}

fn named(name: &str) -> Option<Rgba> {
    let index = NAMED.binary_search_by(|(n, _)| n.cmp(&name)).ok()?;
    let v = NAMED[index].1;
    Some(Rgba { r: (v >> 16 & 0xff) as f32 / 255.0, g: (v >> 8 & 0xff) as f32 / 255.0, b: (v & 0xff) as f32 / 255.0, a: 1.0 })
}

/// A single colour value (an attribute such as `bgcolor`, or one token of a declaration).
fn parse_color(text: &str) -> Option<Rgba> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix('#') { return parse_hex(hex); }
    if let Some(open) = lower.find('(') {
        let args = lower[open + 1..].strip_suffix(')')?;
        return parse_function(&lower[..open], args);
    }
    named(&lower)
}

/// The replacement for one colour token: `light-dark(token, dark)`, `transparent` for the page
/// background, or None to leave it.
fn replacement(token: &str, role: Role) -> Option<String> {
    let color = parse_color(token)?;
    match dark(color, role) {
        None => Some("transparent".to_string()),
        Some(d) if d == color => None,
        Some(d) => Some(format!("light-dark({token},{})", css_color(d))),
    }
}

fn is_ident(c: char) -> bool { c.is_ascii_alphanumeric() || c == '-' || c == '_' || !c.is_ascii() }

/// A declaration value with its colour tokens replaced; None when nothing changed. `url(...)`,
/// strings and `var(...)` are left as they are.
fn rewrite_value(value: &str, role: Role) -> Option<String> {
    let mut out = String::with_capacity(value.len() + 32);
    let mut changed = false;
    let mut rest = value;
    while let Some(c) = rest.chars().next() {
        if c == '"' || c == '\'' {
            let end = rest[1..].find(c).map(|p| p + 2).unwrap_or(rest.len());
            out.push_str(&rest[..end]);
            rest = &rest[end..];
        } else if c == '#' {
            let len = 1 + rest[1..].find(|ch: char| !is_ident(ch)).unwrap_or(rest.len() - 1);
            let token = &rest[..len];
            match replacement(token, role) { Some(r) => { out.push_str(&r); changed = true; } None => out.push_str(token) }
            rest = &rest[len..];
        } else if is_ident(c) {
            let len = rest.find(|ch: char| !is_ident(ch)).unwrap_or(rest.len());
            let word = &rest[..len];
            let lower = word.to_ascii_lowercase();
            if rest[len..].starts_with('(') {
                // An unclosed call is copied as it is.
                let Some(close) = matching_paren(&rest[len..]).map(|p| len + p + 1) else { out.push_str(rest); break; };
                let call = &rest[..close];
                if matches!(lower.as_str(), "rgb" | "rgba" | "hsl" | "hsla") {
                    match replacement(call, role) { Some(r) => { out.push_str(&r); changed = true; } None => out.push_str(call) }
                } else if matches!(lower.as_str(), "url" | "var" | "env" | "light-dark" | "image-set" | "attr") {
                    out.push_str(call);
                } else {
                    // Gradients and other functions: their colour arguments are rewritten.
                    out.push_str(&rest[..=len]);
                    let inner = &rest[len + 1..close - 1];
                    match rewrite_value(inner, role) { Some(r) => { out.push_str(&r); changed = true; } None => out.push_str(inner) }
                    out.push(')');
                }
                rest = &rest[close..];
            } else {
                match if word.starts_with('-') { None } else { replacement(word, role) } {
                    Some(r) => { out.push_str(&r); changed = true; }
                    None => out.push_str(word),
                }
                rest = &rest[len..];
            }
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    changed.then_some(out)
}

/// The position of the `)` that closes the `(` at the start of `text`.
fn matching_paren(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = None;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => { depth = depth.checked_sub(1)?; if depth == 0 { return Some(i); } }
            _ => {}
        }
    }
    None
}

/// One declaration (`name: value`, without its `;`), rewritten as the original followed by the
/// rewritten one; None when it has no colour to change.
fn rewrite_declaration(declaration: &str) -> Option<String> {
    let colon = declaration.find(':')?;
    let name = strip_comments(&declaration[..colon]).trim().to_ascii_lowercase();
    if name.starts_with("--") { return None; }
    let role = role_of(&name)?;
    let rewritten = rewrite_value(&declaration[colon + 1..], role)?;
    Some(format!("{};{name}:{}", declaration.trim_end(), rewritten.trim()))
}

fn strip_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start + 2..].find("*/").map(|end| &rest[start + 2 + end + 2..]).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// A stylesheet with its colours rewritten (see the module comment). Selectors, at-rule preludes,
/// comments and strings are copied as they are.
pub fn rewrite_css(css: &str) -> String {
    let mut out = String::with_capacity(css.len() + css.len() / 4);
    let bytes = css.as_bytes();
    let (mut depth, mut parens, mut start, mut i) = (0usize, 0usize, 0usize, 0usize);
    let flush = |out: &mut String, segment: &str, declaration: bool| {
        match if declaration { rewrite_declaration(segment) } else { None } {
            Some(rewritten) => out.push_str(&rewritten),
            None => out.push_str(segment),
        }
    };
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = css[i + 2..].find("*/").map(|p| i + 2 + p + 2).unwrap_or(bytes.len());
                continue;
            }
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < bytes.len() && bytes[i] != q { if bytes[i] == b'\\' { i += 1; } i += 1; }
                i += 1;
                continue;
            }
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b'{' if parens == 0 => {
                out.push_str(&css[start..=i]);
                depth += 1;
                start = i + 1;
            }
            b'}' if parens == 0 => {
                flush(&mut out, &css[start..i], depth > 0);
                out.push('}');
                depth = depth.saturating_sub(1);
                start = i + 1;
            }
            b';' if parens == 0 => {
                flush(&mut out, &css[start..i], depth > 0);
                out.push(';');
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let end = start.min(css.len());
    flush(&mut out, &css[end..], depth > 0);
    out
}

/// A `style` attribute's declarations with their colours rewritten.
pub fn rewrite_declarations(style: &str) -> String {
    rewrite_css(&format!("x{{{style}}}")).strip_prefix("x{").and_then(|s| s.strip_suffix('}')).map(str::to_string)
        .unwrap_or_else(|| style.to_string())
}

/// An entry's HTML with the colours of its `<style>` blocks, `style` attributes and `color` /
/// `bgcolor` attributes rewritten. Text, comments and scripts are copied as they are.
pub fn rewrite_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + html.len() / 8);
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map(|p| p + 3).unwrap_or(rest.len());
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        if !rest[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
            out.push('<');
            rest = &rest[1..];
            continue;
        }
        let end = tag_end(rest);
        let tag = &rest[..end];
        out.push_str(&rewrite_tag(tag));
        rest = &rest[end..];
        let name: String = tag[1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        if name == "style" || name == "script" {
            let close = format!("</{name}");
            let body_end = find_ascii_ci(rest, &close).unwrap_or(rest.len());
            if name == "style" { out.push_str(&rewrite_css(&rest[..body_end])); } else { out.push_str(&rest[..body_end]); }
            rest = &rest[body_end..];
        }
    }
    out.push_str(rest);
    out
}

fn find_ascii_ci(haystack: &str, needle: &str) -> Option<usize> {
    let lower = needle.to_ascii_lowercase();
    haystack.as_bytes().windows(needle.len()).position(|w| w.eq_ignore_ascii_case(lower.as_bytes()))
}

/// The length of the tag at the start of `text` (through its `>`), skipping quoted values.
fn tag_end(text: &str) -> usize {
    let mut quote = None;
    for (i, c) in text.char_indices().skip(1) {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return i + 1,
            _ => {}
        }
    }
    text.len()
}

struct Attribute<'a> { name: String, value: &'a str, quote: Option<char>, start: usize, end: usize }

fn attributes(tag: &str) -> Vec<Attribute<'_>> {
    let mut out = Vec::new();
    let bytes = tag.as_bytes();
    let mut i = 1 + tag[1..].find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(tag.len() - 1);
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') { i += 1; }
        if i >= bytes.len() || bytes[i] == b'>' { break; }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'=' | b'>' | b'/') { i += 1; }
        let name = tag[start..i].to_ascii_lowercase();
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() { j += 1; }
        if j < bytes.len() && bytes[j] == b'=' {
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() { j += 1; }
            let (value, quote, end) = match bytes.get(j) {
                Some(&q) if q == b'"' || q == b'\'' => {
                    let close = tag[j + 1..].find(q as char).map(|p| j + 1 + p).unwrap_or(tag.len());
                    (&tag[j + 1..close], Some(q as char), (close + 1).min(tag.len()))
                }
                _ => {
                    let close = tag[j..].find(|c: char| c.is_whitespace() || c == '>').map(|p| j + p).unwrap_or(tag.len());
                    (&tag[j..close], None, close)
                }
            };
            out.push(Attribute { name, value, quote, start, end });
            i = end;
        } else {
            if name.is_empty() { i += 1; continue; }
            out.push(Attribute { name, value: "", quote: None, start, end: i });
        }
    }
    out
}

fn rewrite_tag(tag: &str) -> String {
    let attrs = attributes(tag);
    let style = attrs.iter().find(|a| a.name == "style");
    let mut extra = String::new();
    let mut removed = Vec::new();
    for a in &attrs {
        let property = match a.name.as_str() { "color" => "color", "bgcolor" => "background-color", _ => continue };
        if parse_color(a.value).is_none() { continue; }
        extra.push_str(&format!("{property}:{};", a.value.trim()));
        removed.push((a.start, a.end));
    }
    let new_style = match style {
        Some(s) => {
            let joined = format!("{extra}{}", s.value);
            let rewritten = rewrite_declarations(&joined);
            if rewritten == s.value && extra.is_empty() { None } else { Some((rewritten, Some((s.start, s.end, s.quote)))) }
        }
        None if !extra.is_empty() => Some((rewrite_declarations(&extra), None)),
        None => None,
    };
    let Some((style_value, place)) = new_style else { return tag.to_string() };
    let quote = place.and_then(|(_, _, q)| q).filter(|q| !style_value.contains(*q)).unwrap_or(if style_value.contains('"') { '\'' } else { '"' });
    let attribute = format!("style={quote}{style_value}{quote}");
    let mut out = String::with_capacity(tag.len() + attribute.len());
    let mut at = 0;
    let mut cuts: Vec<(usize, usize, Option<&str>)> = removed.iter().map(|&(s, e)| (s, e, None)).collect();
    match place {
        Some((s, e, _)) => cuts.push((s, e, Some(attribute.as_str()))),
        None => {
            // A new style attribute goes right after the tag name.
            let name_end = 1 + tag[1..].find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(tag.len() - 1);
            cuts.push((name_end, name_end, None));
            cuts.sort_by_key(|c| c.0);
            for (s, e, replacement) in cuts {
                out.push_str(&tag[at..s]);
                if s == name_end && e == name_end && replacement.is_none() { out.push(' '); out.push_str(&attribute); }
                else if let Some(r) = replacement { out.push_str(r); }
                at = e;
            }
            out.push_str(&tag[at..]);
            return out;
        }
    }
    cuts.sort_by_key(|c| c.0);
    for (s, e, replacement) in cuts {
        out.push_str(&tag[at..s]);
        if let Some(r) = replacement { out.push_str(r); }
        at = e;
    }
    out.push_str(&tag[at..]);
    out
}

/// CSS named colours, sorted by name (binary search).
const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4), ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc), ("bisque", 0xffe4c4), ("black", 0x000000), ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a), ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e), ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c),
    ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b), ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b), ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc), ("darkred", 0x8b0000), ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b), ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969), ("dimgrey", 0x696969), ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22), ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700), ("goldenrod", 0xdaa520), ("gray", 0x808080), ("green", 0x008000),
    ("greenyellow", 0xadff2f), ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa), ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6), ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3), ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a), ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32), ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff), ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa), ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371), ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585), ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1), ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead), ("navy", 0x000080), ("oldlace", 0xfdf5e6), ("olive", 0x808000), ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500), ("orangered", 0xff4500), ("orchid", 0xda70d6), ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee), ("palevioletred", 0xdb7093), ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f),
    ("pink", 0xffc0cb), ("plum", 0xdda0dd), ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("rebeccapurple", 0x663399),
    ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1), ("saddlebrown", 0x8b4513), ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee), ("sienna", 0xa0522d), ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd), ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f), ("steelblue", 0x4682b4), ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347), ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3), ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00), ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_colours_are_sorted_for_the_binary_search() {
        assert!(NAMED.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(named("navy"), Some(Rgba { r: 0.0, g: 0.0, b: 128.0 / 255.0, a: 1.0 }));
    }

    #[test]
    fn dark_text_turns_light_with_its_hue_and_the_white_page_turns_transparent() {
        let css = "body{background:#fff;color:#000}\n.pos { color: darkblue !important; border-bottom: 1px solid #c00 }";
        let out = rewrite_css(css);
        assert!(out.contains("background:#fff;background:transparent"), "{out}");
        assert!(out.contains("color:#000;color:light-dark(#000,#ebebeb)"), "{out}");
        assert!(out.contains("color: darkblue !important;color:light-dark(darkblue,"), "{out}");
        assert!(out.contains("border-bottom: 1px solid #c00;border-bottom:1px solid light-dark(#c00,"), "{out}");
        assert!(out.starts_with("body{") && out.contains(".pos {"), "selectors are kept: {out}");
    }

    #[test]
    fn selectors_strings_urls_comments_and_variables_are_left_alone() {
        let css = "#red, a.white:hover { content: \"black; red\"; background: url(\"white.png\") no-repeat; color: var(--x, red) }\n/* color: black; */ @media (prefers-color-scheme: dark) { .x { width: 1px } } .font { white-space: nowrap }";
        let out = rewrite_css(css);
        assert!(out.starts_with("#red, a.white:hover {"), "{out}");
        assert!(out.contains("content: \"black; red\""), "{out}");
        assert!(out.contains("url(\"white.png\") no-repeat;"), "no colour token outside url: {out}");
        assert!(out.contains("color: var(--x, red)") && !out.contains("var(--x, light-dark"), "{out}");
        assert!(out.contains("/* color: black; */"), "{out}");
        assert!(out.contains(".font { white-space: nowrap }"), "{out}");
    }

    #[test]
    fn a_data_url_with_semicolons_does_not_split_its_declaration() {
        let css = "@font-face{font-family:x;src:url(data:font/woff;base64,AAAA)}.a{color:#333}";
        let out = rewrite_css(css);
        assert!(out.contains("src:url(data:font/woff;base64,AAAA)}"), "{out}");
        assert!(out.contains(".a{color:#333;color:light-dark(#333,"), "{out}");
    }

    #[test]
    fn functional_colours_gradients_and_light_backgrounds() {
        let out = rewrite_css(".a{background:linear-gradient(#ffffff, rgb(255 220 150));color:rgba(0,0,0,.5);background-color:hsl(60 100% 80%)}");
        assert!(out.contains("linear-gradient(transparent, light-dark(rgb(255 220 150),"), "{out}");
        assert!(out.contains("color:light-dark(rgba(0,0,0,.5),rgba("), "alpha is kept: {out}");
        assert!(out.contains("background-color:light-dark(hsl(60 100% 80%),#"), "a light tint becomes a dark tint: {out}");
        assert_eq!(rewrite_css(".b{color:white;background:#202020}"), ".b{color:white;background:#202020}", "already dark-friendly");
    }

    #[test]
    fn html_attributes_and_style_blocks_are_rewritten() {
        let html = "<p style=\"color:#000\">a</p><font color=\"#000080\">b</font><td bgcolor=white class=x>c</td><style>.t{color:black}</style><script>var s='<b style=\"color:#000\">';</script><!-- <i style=\"color:#000\"> -->";
        let out = rewrite_html(html);
        assert!(out.contains("<p style=\"color:#000;color:light-dark(#000,#ebebeb)\">"), "{out}");
        assert!(out.contains("<font style=\"color:#000080;color:light-dark(#000080,"), "{out}");
        assert!(!out.contains("color=\"#000080\""), "{out}");
        assert!(out.contains("<td style=\"background-color:white;background-color:transparent;\"") && out.contains("class=x>"), "the white cell becomes transparent: {out}");
        assert!(out.contains("<style>.t{color:black;color:light-dark(black,#ebebeb)}</style>"), "{out}");
        assert!(out.contains("<script>var s='<b style=\"color:#000\">';</script>"), "scripts are copied: {out}");
        assert!(out.contains("<!-- <i style=\"color:#000\"> -->"), "comments are copied: {out}");
        assert_eq!(rewrite_html("<p class=a>plain &lt; text</p>"), "<p class=a>plain &lt; text</p>");
    }

    #[test]
    fn an_existing_style_keeps_the_last_word_over_a_colour_attribute() {
        let out = rewrite_html("<font color=red style='font-weight:bold;color:#008000'>x</font>");
        assert!(out.contains("style='color:red;color:light-dark(red,"), "{out}");
        assert!(out.contains("font-weight:bold;color:#008000;color:light-dark(#008000,"), "{out}");
        assert!(!out.contains("color=red"), "{out}");
    }
}
