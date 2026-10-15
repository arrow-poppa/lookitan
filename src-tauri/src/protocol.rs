// The `lkt:` URI scheme that serves dictionary entries and their resources to the popup.
//   /e/<dict-id>/<entry>   entry HTML, wrapped with a <base> pointing at the resource route
//   /d/<dict-id>/<path>    a stylesheet, script, image or sound next to the .mdx or inside the .mdd
// On Windows the WebView reaches it as http://lkt.localhost/..., on Linux as lkt://localhost/....

use tauri::http::{Request, Response};
use tauri::{AppHandle, Manager};

use crate::AppState;

const ENTRY_SCRIPT: &str = include_str!("../../src/lib/dictionaries/article.js");

// Two looks. `lkt-themed`: the formats Lookitan lays out itself (Yomitan, DSL, StarDict, ...) are
// shown like the AI card, in the pop-up's font and colours on its own background, as Lookupper
// shows them. `lkt-own`: MDict and Slob articles bring stylesheets written for a white page, so
// they keep that page, inverted in the dark theme.
const ENTRY_STYLE: &str = "html,body{margin:0;padding:0;background:transparent;}\
html.lkt-own{color-scheme:light;background:#fff}\
@media (prefers-color-scheme:dark){html.lkt-own{filter:invert(.9) hue-rotate(180deg)}}\
html.lkt-themed{color-scheme:light dark;--text:#1b1b1b;--muted:#5f6066;--accent:#0a74c9}\
@media (prefers-color-scheme:dark){html.lkt-themed{--text:#f1f1f1;--muted:#a9abb1;--accent:#8fd3ff}}\
body.lkt-entry{overflow:hidden;word-wrap:break-word;}\
.lkt-own body{padding:6px 10px;font:14px/1.45 system-ui,-apple-system,'Segoe UI',sans-serif}\
.lkt-themed body{padding:2px 0 6px;color:var(--text);\
font:15px/1.5 'Segoe UI Variable','Segoe UI',system-ui,-apple-system,'Noto Sans',sans-serif}\
.lkt-themed a{color:var(--accent)}\
img{max-width:100%;height:auto;}\
.reading,.pronunciation{color:var(--muted,#888)} .tags{font-size:.85em;opacity:.7}\
dl{margin:.5em 0}dd{margin-left:1em}table{max-width:100%}pre{white-space:pre-wrap}\
.dsl-example{font-style:italic}.dsl-translation{color:#629d65}details{margin:.4em 0}\
.yomitan-headword{margin:0 0 4px;font-size:17px;font-weight:600;line-height:1.35}\
.yomitan-reading,.yomitan-tag{margin-left:6px;font-weight:400;color:var(--muted)}\
.yomitan-tag{word-break:keep-all;cursor:help}\
.yomitan-meta .yomitan-tag,.yomitan-frequency .yomitan-tag,.yomitan-pitch .yomitan-tag,.yomitan-ipa .yomitan-tag{margin:0 6px 0 0}\
.yomitan-senses{margin:0;padding:0;list-style-position:inside}.yomitan-senses>li{margin:0 0 4px}\
.yomitan-senses>li::marker{color:var(--muted)}\
.yomitan-readings,.yomitan-frequency,.yomitan-pitch,.yomitan-ipa{margin:2px 0 0}.yomitan-facts{margin-top:2px;font-size:13.5px;color:var(--muted)}";

/// The page an entry is served as: its HTML with the entry style and script, in the look its
/// dictionary calls for (see ENTRY_STYLE).
fn entry_document(dict_id: &str, html: &str, styles: &str, own_look: bool) -> String {
    let look = if own_look { "lkt-own" } else { "lkt-themed" };
    format!(
        "<!doctype html><html class=\"{look}\"><head><meta charset=\"utf-8\"><base href=\"/d/{dict_id}/\">\
         <style>{ENTRY_STYLE}</style>{styles}<script>{ENTRY_SCRIPT}</script></head>\
         <body class=\"lkt-entry\">{html}</body></html>"
    )
}

fn normalize_resource_urls(html: &str, id: &str) -> String {
    use std::sync::LazyLock;
    static ATTRS: LazyLock<regex::Regex> = LazyLock::new(||
        regex::Regex::new(r#"(?i)\b(src|href|poster)\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());
    ATTRS.replace_all(html, |c: &regex::Captures<'_>| {
        let raw = c.get(2).or_else(|| c.get(3)).map(|s| s.as_str()).unwrap_or("");
        let value = raw.replace('\\', "/");
        let value = if value.starts_with('/') && !value.starts_with("//") {
            format!("/d/{id}/{}", value.trim_start_matches('/'))
        } else { value };
        format!("{}=\"{}\"", &c[1], value.replace('"', "&quot;"))
    }).into_owned()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn mime(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "spx" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" | "aac" => "audio/mp4",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn respond(status: u16, content_type: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-cache")
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn not_found() -> Response<Vec<u8>> {
    respond(404, "text/plain", b"not found".to_vec())
}

pub fn handle(app: &AppHandle, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = percent_decode(request.uri().path());
    let mut parts = path.trim_start_matches('/').splitn(3, '/');
    let kind = parts.next().unwrap_or("");
    let dict_id = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    let state = app.state::<AppState>();
    match kind {
        // /c/<recognition>/frame.png: the captured frame shown behind a frozen overlay.
        "c" => {
            let id = dict_id.parse::<u64>().ok();
            // The overlay's frozen frame: exactly the image OCR read, so word boxes line up.
            return match crate::capture_png(&state, id, "frame") {
                Some(png) => respond(200, "image/png", png),
                None => not_found(),
            };
        }
        // /m/<file>: screenshot of a saved word.
        "m" => {
            return match state.cards.media(dict_id) {
                Some(bytes) => respond(200, mime(dict_id), bytes),
                None => not_found(),
            };
        }
        _ => {}
    }
    let dicts = state.dicts.read().clone();
    let Some(dict) = dicts.iter().find(|d| d.id == dict_id) else {
        return not_found();
    };
    match kind {
        "e" => {
            let Ok(idx) = rest.parse::<u32>() else { return not_found() };
            match dict.entry_html(idx) {
                Ok(html) => {
                    let styles = dict.stylesheets().into_iter().map(|name| format!("<link rel=\"stylesheet\" href=\"{}\">",
                        crate::dictionaries::escape_html(&name))).collect::<String>();
                    let html = normalize_resource_urls(&html, dict_id);
                    let doc = entry_document(dict_id, &html, &styles, dict.own_look());
                    respond(200, "text/html; charset=utf-8", doc.into_bytes())
                }
                Err(e) => respond(500, "text/plain; charset=utf-8", format!("{e:#}").into_bytes()),
            }
        }
        "d" => match dict.resource(rest) {
            Some(bytes) => respond(200, mime(rest), bytes),
            None => not_found(),
        },
        _ => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resources_resolve_inside_the_dictionary_and_cross_references_stay_clickable() {
        let html = r#"<img src="\Images\a.png"><link href='/style.css'><a href="entry://cat">cat</a><a href="sound://a.mp3">play</a>"#;
        let result = normalize_resource_urls(html, "d1");
        assert!(result.contains("src=\"/d/d1/Images/a.png\""));
        assert!(result.contains("href=\"/d/d1/style.css\""));
        assert!(result.contains("entry://cat"));
        assert!(result.contains("sound://a.mp3"));
    }

    #[test]
    fn entries_take_the_pop_up_look_unless_their_dictionary_brings_its_own() {
        let themed = entry_document("d1", "<p>x</p>", "", false);
        assert!(themed.starts_with("<!doctype html><html class=\"lkt-themed\">"), "{themed}");
        assert!(themed.contains("<body class=\"lkt-entry\"><p>x</p></body>"));
        let own = entry_document("d1", "<p>x</p>", "<link rel=\"stylesheet\" href=\"a.css\">", true);
        assert!(own.starts_with("<!doctype html><html class=\"lkt-own\">"), "{own}");
        assert!(own.contains("</style><link rel=\"stylesheet\" href=\"a.css\"><script>"));
    }
}
