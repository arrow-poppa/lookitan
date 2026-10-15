// Optional cloud OCR through Google Lens, following the request YomiNinja (GPL-3.0) sends from
// `google_lens_ocr_grpc.adapter.ts`: a protobuf `LensOverlayServerRequest` posted to
// `lensfrontend-pa.googleapis.com/v1/crupload`. This is not an official Google API; it can change
// or stop working at any time, and every lookup uploads the captured region to Google.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::Cursor;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use image::RgbaImage;
use parking_lot::Mutex;
use prost::Message;

use super::lens_proto as pb;
use super::{OcrChar, OcrLine};

const ENDPOINT: &str = "https://lensfrontend-pa.googleapis.com/v1/crupload";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36";
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
        if api_key.trim().is_empty() {
            bail!("Google Lens needs an API key (Settings › OCR)");
        }
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
            .header("x-goog-api-key", api_key.trim())
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

        let lines = to_lines(&decoded, img.width() as f32, img.height() as f32);
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
