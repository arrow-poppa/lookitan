// Optional cloud OCR through Google Lens, following the request YomiNinja (GPL-3.0) sends from
// `google_lens_ocr_grpc.adapter.ts`: a protobuf `LensOverlayServerRequest` posted to
// `lensfrontend-pa.googleapis.com/v1/crupload`. This is not an official Google API; it can change
// or stop working at any time, and every lookup uploads the captured region to Google.
// Without a key of the user's, it sends the key the build was given in LOOKITAN_LENS_API_KEY (the
// GitHub Actions secret of the same name, Chromium's public Lens key as owocr and chrome-lens-ocr
// send it), so the key is not in the source. A key pasted in Configurações › OCR is sent instead;
// a build without the secret (local, or a fork's pull request) needs the user's key.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::Cursor;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use image::RgbaImage;
use parking_lot::Mutex;
use prost::Message;

use super::ink;
use super::lens_proto as pb;
use super::{OcrChar, OcrLine};

const ENDPOINT: &str = "https://lensfrontend-pa.googleapis.com/v1/crupload";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36";
/// The build's key for Lens, used when the user has none (see the top of this file).
const BUILT_IN_API_KEY: Option<&str> = option_env!("LOOKITAN_LENS_API_KEY");
const MAX_DIMENSION: u32 = 1000;
const MAX_AREA: u64 = 1_000_000;
const CACHE_SECONDS: u64 = 30;

pub struct LensOcr {
    client: reqwest::blocking::Client,
    cache: Mutex<Option<(u64, Instant, Vec<OcrLine>)>>,
}

fn random_u64() -> u64 {
    let mut h = RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

fn image_hash(img: &RgbaImage) -> u64 {
    // Fixed keys so equal captures hash equally across calls.
    let mut h = std::hash::BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default().build_hasher();
    h.write_u32(img.width());
    h.write_u32(img.height());
    h.write(img.as_raw());
    h.finish()
}

/// Shrinks the capture to at most 1000 px on the longest side and one megapixel, like YomiNinja.
fn fit(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut nw = w as f64;
    let mut nh = h as f64;
    let longest = nw.max(nh);
    if longest > MAX_DIMENSION as f64 {
        let s = MAX_DIMENSION as f64 / longest;
        nw *= s;
        nh *= s;
    }
    if nw * nh > MAX_AREA as f64 {
        let s = (MAX_AREA as f64 / (nw * nh)).sqrt();
        nw *= s;
        nh *= s;
    }
    let (nw, nh) = ((nw.floor() as u32).max(1), (nh.floor() as u32).max(1));
    if nw == w && nh == h {
        img.clone()
    } else {
        image::imageops::resize(img, nw, nh, FilterType::Triangle)
    }
}

impl LensOcr {
    pub fn new() -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self { client, cache: Mutex::new(None) })
    }

    pub fn recognize(&self, img: &RgbaImage, api_key: &str, language_tag: &str) -> Result<Vec<OcrLine>> {
        let api_key = match (api_key.trim(), BUILT_IN_API_KEY.map(str::trim)) {
            ("", Some(built_in)) if !built_in.is_empty() => built_in,
            ("", _) => bail!("Google Lens needs an API key (Settings › OCR): this build came without one"),
            (key, _) => key,
        };
        let key = image_hash(img);
        if let Some((k, at, lines)) = self.cache.lock().as_ref() {
            if *k == key && at.elapsed().as_secs() < CACHE_SECONDS {
                return Ok(lines.clone());
            }
        }

        let small = fit(img);
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(small.clone())
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .context("encode PNG")?;

        let (language, region) = match language_tag.split_once('-') {
            Some((l, r)) => (l.to_string(), r.to_string()),
            None => (language_tag.to_string(), String::new()),
        };
        let mut analytics = Vec::with_capacity(16);
        analytics.extend_from_slice(&random_u64().to_be_bytes());
        analytics.extend_from_slice(&random_u64().to_be_bytes());
        let request = pb::ServerRequest {
            objects_request: Some(pb::ObjectsRequest {
                request_context: Some(pb::RequestContext {
                    request_id: Some(pb::RequestId {
                        uuid: random_u64(),
                        sequence_id: 0,
                        image_sequence_id: 0,
                        analytics_id: analytics,
                        routing_info: Some(pb::RoutingInfo::default()),
                    }),
                    client_context: Some(pb::ClientContext {
                        platform: pb::PLATFORM_WEB,
                        locale_context: Some(pb::LocaleContext { language, region, time_zone: "UTC".into() }),
                        client_filters: Some(pb::AppliedFilters {
                            filter: vec![pb::AppliedFilter { filter_type: pb::FILTER_AUTO }],
                        }),
                    }),
                }),
                image_data: Some(pb::ImageData {
                    payload: Some(pb::ImagePayload { image_bytes: png }),
                    image_metadata: Some(pb::ImageMetadata { width: small.width() as i32, height: small.height() as i32 }),
                }),
            }),
        };

        let started = Instant::now();
        let response = self
            .client
            .post(ENDPOINT)
            .header("content-type", "application/x-protobuf")
            .header("x-goog-api-key", api_key)
            .header("accept-language", "en-US,en;q=0.9")
            .body(request.encode_to_vec())
            .send()
            .context("Google Lens request")?;
        let status = response.status();
        let body = response.bytes()?;
        if !status.is_success() {
            bail!("Google Lens answered HTTP {status}");
        }
        let decoded = pb::ServerResponse::decode(body.as_ref()).context("decode Google Lens response")?;
        if let Some(err) = decoded.error.as_ref() {
            if err.error_type != 0 {
                bail!("Google Lens error type {}", err.error_type);
            }
        }
        log::info!("Google Lens answered in {} ms", started.elapsed().as_millis());

        let mut lines = to_lines(&decoded, img.width() as f32, img.height() as f32);
        for line in &mut lines {
            snap_to_ink(img, line);
        }
        *self.cache.lock() = Some((key, Instant::now(), lines.clone()));
        Ok(lines)
    }
}

fn rect(g: &Option<pb::Geometry>, w: f32, h: f32) -> Option<(f32, f32, f32, f32)> {
    let b = g.as_ref()?.bounding_box.as_ref()?;
    let x0 = (b.center_x - b.width / 2.0) * w;
    let y0 = (b.center_y - b.height / 2.0) * h;
    Some((x0, y0, x0 + b.width * w, y0 + b.height * h))
}

/// How far past Lens's word box its ink is looked for, in line heights.
const INK_REACH: f32 = 0.15;

/// Snaps a Lens line to the ink of the capture, as the local recognizer's lines are (ocr/ink.rs):
/// the line's ink rows go in `ink_y`, and each word's ends move to its first and last ink column,
/// near Lens's word box and never past halfway to the next word. Spaces fill the gaps between
/// words. Lens's boxes are loose and a little off, so without this the highlight did not hug the
/// word. Vertical lines and crops with no clear ink keep Lens's boxes.
fn snap_to_ink(img: &RgbaImage, line: &mut OcrLine) {
    let (iw, ih) = img.dimensions();
    let height = line.y1 - line.y0;
    if height <= 0.0 || height > (line.x1 - line.x0) * 1.5 {
        return;
    }
    let reach = height * INK_REACH;
    let x = ((line.x0 - reach).floor().max(0.0) as u32).min(iw.saturating_sub(1));
    let y = (line.y0.floor().max(0.0) as u32).min(ih.saturating_sub(1));
    let cw = (((line.x1 + reach).ceil().max(0.0) as u32).min(iw)).saturating_sub(x);
    let ch = ((line.y1.ceil().max(0.0) as u32).min(ih)).saturating_sub(y);
    if cw < 2 || ch < 2 {
        return;
    }
    let crop = image::imageops::crop_imm(img, x, y, cw, ch).to_image();
    let lum: Vec<u8> = crop.pixels().map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8).collect();
    let Some(found) = ink::measure(&lum, cw as usize, ch as usize) else { return };
    line.ink_y = Some((y as f32 + found.y0 as f32, y as f32 + found.y1 as f32));

    let cols = &found.cols;
    let last_col = cols.len() as isize - 1;
    let col = |at: f32| ((at - x as f32).floor() as isize).clamp(0, last_col) as usize;
    // The words: runs of characters that are not spaces.
    let mut words: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < line.chars.len() {
        if line.chars[i].ch.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < line.chars.len() && !line.chars[i].ch.is_whitespace() {
            i += 1;
        }
        words.push((start, i));
    }
    let ends: Vec<(f32, f32)> = words.iter().map(|&(a, b)| (line.chars[a].x0, line.chars[b - 1].x1)).collect();
    let mut snapped = ends.clone();
    for (k, &(x0, x1)) in ends.iter().enumerate() {
        let middle = (x0 + x1) / 2.0;
        let lo = if k == 0 { x0 - reach } else { (x0 - reach).max((ends[k - 1].1 + x0) / 2.0) };
        let hi = if k + 1 == ends.len() { x1 + reach } else { (x1 + reach).min((x1 + ends[k + 1].0) / 2.0) };
        let first = (col(lo)..=col(middle)).find(|&c| cols[c]);
        let last = (col(middle)..=col(hi)).rev().find(|&c| cols[c]);
        if let (Some(first), Some(last)) = (first, last) {
            snapped[k] = (x as f32 + first as f32, x as f32 + last as f32 + 1.0);
        }
    }
    for (k, &(a, b)) in words.iter().enumerate() {
        let (x0, x1) = snapped[k];
        let step = (x1 - x0) / (b - a) as f32;
        for (j, c) in line.chars[a..b].iter_mut().enumerate() {
            c.x0 = x0 + step * j as f32;
            c.x1 = x0 + step * (j as f32 + 1.0);
        }
        // The spaces after the word share the gap to the next one.
        let next = words.get(k + 1).map_or(line.chars.len(), |w| w.0);
        if next > b {
            let to = snapped.get(k + 1).map_or(x1, |w| w.0).max(x1);
            let width = (to - x1) / (next - b) as f32;
            for (j, c) in line.chars[b..next].iter_mut().enumerate() {
                c.x0 = x1 + width * j as f32;
                c.x1 = x1 + width * (j as f32 + 1.0);
            }
        }
    }
}

/// Converts paragraphs/lines/words to lines with per-character spans. Lens returns word boxes;
/// characters are spread evenly across their word so the same pointer-to-word logic applies.
fn to_lines(resp: &pb::ServerResponse, w: f32, h: f32) -> Vec<OcrLine> {
    let mut out = Vec::new();
    let Some(layout) = resp.objects_response.as_ref().and_then(|o| o.text.as_ref()).and_then(|t| t.text_layout.as_ref())
    else {
        return out;
    };
    for para in &layout.paragraphs {
        let vertical = para.writing_direction == pb::WRITING_TOP_TO_BOTTOM;
        for line in &para.lines {
            let mut words: Vec<&pb::Word> = line.words.iter().collect();
            if !vertical {
                words.sort_by(|a, b| {
                    let ax = a.geometry.as_ref().and_then(|g| g.bounding_box.as_ref()).map(|b| b.center_x).unwrap_or(0.0);
                    let bx = b.geometry.as_ref().and_then(|g| g.bounding_box.as_ref()).map(|b| b.center_x).unwrap_or(0.0);
                    ax.total_cmp(&bx)
                });
            }
            let mut text = String::new();
            let mut chars = Vec::new();
            for word in words {
                let (wx0, _, wx1, _) = rect(&word.geometry, w, h).unwrap_or((0.0, 0.0, 0.0, 0.0));
                let n = word.plain_text.chars().count().max(1) as f32;
                let step = (wx1 - wx0) / n;
                for (i, c) in word.plain_text.chars().enumerate() {
                    chars.push(OcrChar { ch: c, x0: wx0 + step * i as f32, x1: wx0 + step * (i as f32 + 1.0) });
                    text.push(c);
                }
                if let Some(sep) = word.text_separator.as_deref() {
                    for c in sep.chars() {
                        chars.push(OcrChar { ch: c, x0: wx1, x1: wx1 });
                        text.push(c);
                    }
                }
            }
            let Some((x0, y0, x1, y1)) = rect(&line.geometry, w, h) else { continue };
            if !text.trim().is_empty() {
                out.push(OcrLine { text, x0, y0, x1, y1, chars, ink_y: None });
            }
        }
    }
    out.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white capture with dark blocks as letters, at `x0..x1`, rows 104..116.
    fn capture(words: &[(u32, u32)]) -> RgbaImage {
        RgbaImage::from_fn(400, 200, |px, py| {
            let inked = (104..116).contains(&py) && words.iter().any(|&(a, b)| (a..b).contains(&px));
            if inked { image::Rgba([20, 20, 20, 255]) } else { image::Rgba([250, 250, 250, 255]) }
        })
    }

    fn lens_line(words: &[(&str, f32, f32)]) -> OcrLine {
        let mut text = String::new();
        let mut chars = Vec::new();
        for (k, &(word, x0, x1)) in words.iter().enumerate() {
            let step = (x1 - x0) / word.chars().count() as f32;
            for (i, c) in word.chars().enumerate() {
                chars.push(OcrChar { ch: c, x0: x0 + step * i as f32, x1: x0 + step * (i as f32 + 1.0) });
                text.push(c);
            }
            if k + 1 < words.len() {
                chars.push(OcrChar { ch: ' ', x0: x1, x1 });
                text.push(' ');
            }
        }
        OcrLine { text, x0: words[0].1, y0: 98.0, x1: words.last().unwrap().2, y1: 122.0, chars, ink_y: None }
    }

    #[test]
    fn lens_words_are_snapped_to_their_ink_like_the_local_models() {
        // Lens's boxes are loose and a little shifted; the letters are at 100..160 and 180..230.
        let img = capture(&[(100, 160), (180, 230)]);
        let mut line = lens_line(&[("hello", 96.0, 166.0), ("world", 172.0, 228.0)]);
        snap_to_ink(&img, &mut line);
        assert_eq!(line.ink_y, Some((104.0, 116.0)), "the rows of the ink");
        let (first, last) = (&line.chars[0], &line.chars[4]);
        assert_eq!((first.x0, last.x1), (100.0, 160.0));
        // The second word reaches its ink past Lens's box, up to the reach.
        assert_eq!((line.chars[6].x0, line.chars[10].x1), (180.0, 230.0));
        assert_eq!((line.chars[5].x0, line.chars[5].x1), (160.0, 180.0), "the space fills the gap");
    }

    #[test]
    fn a_lens_line_without_clear_ink_keeps_its_boxes() {
        let img = RgbaImage::from_pixel(400, 200, image::Rgba([250, 250, 250, 255]));
        let mut line = lens_line(&[("hello", 96.0, 166.0)]);
        let before = line.clone();
        snap_to_ink(&img, &mut line);
        assert_eq!(format!("{line:?}"), format!("{before:?}"));
    }
}
