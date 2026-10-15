// The `lkt:` URI scheme that serves dictionary entries and their resources to the popup.
//   /e/<dict-id>/<entry>   entry HTML, wrapped with a <base> pointing at the resource route
//   /d/<dict-id>/<path>    a stylesheet, script, image or sound next to the .mdx or inside the .mdd
// On Windows the WebView reaches it as http://lkt.localhost/..., on Linux as lkt://localhost/....

use tauri::http::{Request, Response};
use tauri::{AppHandle, Manager};

use crate::AppState;

const ENTRY_SCRIPT: &str = include_str!("../../src/lib/dictionaries/article.js");

const ENTRY_STYLE: &str = "html,body{margin:0;padding:0;background:transparent;}\
body.lkt-entry{padding:6px 10px;font:14px/1.45 system-ui,-apple-system,'Segoe UI',sans-serif;overflow:hidden;word-wrap:break-word;}\
img{max-width:100%;height:auto;}\
.reading,.pronunciation{color:#888} .tags{font-size:.85em;opacity:.7}\
dl{margin:.5em 0}dd{margin-left:1em}table{max-width:100%}pre{white-space:pre-wrap}\
.dsl-example{font-style:italic}.dsl-translation{color:#629d65}details{margin:.4em 0}";

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
                    let doc = format!(
                        "<!doctype html><html><head><meta charset=\"utf-8\"><base href=\"/d/{dict_id}/\">\
                         <style>{ENTRY_STYLE}</style>{styles}<script>{ENTRY_SCRIPT}</script></head>\
                         <body class=\"lkt-entry\">{html}</body></html>"
                    );
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
}
