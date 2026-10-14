// The `lkt:` URI scheme that serves dictionary entries and their resources to the popup.
//   /e/<dict-id>/<entry>   entry HTML, wrapped with a <base> pointing at the resource route
//   /d/<dict-id>/<path>    a stylesheet, script, image or sound next to the .mdx or inside the .mdd
// On Windows the WebView reaches it as http://lkt.localhost/..., on Linux as lkt://localhost/....

use tauri::http::{Request, Response};
use tauri::{AppHandle, Manager};

use crate::AppState;

const ENTRY_SCRIPT: &str = r#"(function () {
  var parts = location.pathname.split('/');
  var dictId = parts[2];
  function post(m) { try { m.lookitan = true; parent.postMessage(m, '*'); } catch (e) {} }
  document.addEventListener('click', function (ev) {
    var a = ev.target && ev.target.closest ? ev.target.closest('a') : null;
    if (!a) return;
    var href = a.getAttribute('href') || '';
    if (href.indexOf('sound://') === 0) {
      ev.preventDefault();
      var p = href.slice(8).replace(/\\/g, '/').replace(/^\/+/, '');
      new Audio('/d/' + dictId + '/' + p.split('/').map(encodeURIComponent).join('/')).play().catch(function () {});
      return;
    }
    if (href.indexOf('entry://') === 0 || href.indexOf('bword://') === 0) {
      ev.preventDefault();
      var w = decodeURIComponent(href.slice(8));
      if (w.charAt(0) === '#') {
        var el = document.getElementsByName(w.slice(1))[0] || document.getElementById(w.slice(1));
        if (el) el.scrollIntoView();
        return;
      }
      post({type: 'lookup', text: w});
      return;
    }
    if (/^https?:/i.test(href)) { ev.preventDefault(); post({type: 'open', url: href}); }
  }, true);
  // The pop-up cannot read a cross-origin iframe's selection; send it for {popup-selection-text}.
  document.addEventListener('selectionchange', function () {
    var s = '';
    try { s = String(window.getSelection() || ''); } catch (e) {}
    post({type: 'selection', text: s});
  });
  function report() {
    post({type: 'height', path: location.pathname, height: Math.ceil(document.documentElement.scrollHeight)});
  }
  window.addEventListener('load', report);
  try { new ResizeObserver(report).observe(document.documentElement); } catch (e) {}
  setTimeout(report, 60);
})();"#;

const ENTRY_STYLE: &str = "html,body{margin:0;padding:0;background:transparent;}\
body.lkt-entry{padding:6px 10px;font:14px/1.45 system-ui,-apple-system,'Segoe UI',sans-serif;overflow:hidden;word-wrap:break-word;}\
img{max-width:100%;height:auto;}";

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
        .header("Cache-Control", "max-age=3600")
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
            return match crate::capture_png(&state, id, "whole") {
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
                    let doc = format!(
                        "<!doctype html><html><head><meta charset=\"utf-8\"><base href=\"/d/{dict_id}/\">\
                         <style>{ENTRY_STYLE}</style><script>{ENTRY_SCRIPT}</script></head>\
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
