// Bing's visual search OCR as a cloud OCR engine, at the user's request on 2026-10-28, as owocr's
// "Bing" engine reads it (credited there to @bropines): the bing.com website's own endpoints, not a
// public API, so it may change or stop without notice. Each lookup uploads the capture to Microsoft.
//
// Two requests, as owocr makes them: the image goes up as the image search's upload form (base64,
// at most `MAX_BYTES`), which answers with a redirect holding an `insightsToken`; then the
// knowledge API is asked for the OCR skill on that token. A plain client was enough when checked on
// 2026-10-28 (owocr presents itself as Chrome; Lookitan sends Chrome's user agent only).
//
// The answer's lines and words come as quadrilaterals in fractions of the image, read like OneOCR's
// (`oneocr::to_lines`).

use std::io::Cursor;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use base64::Engine;
use image::imageops::FilterType;
use image::RgbaImage;
use serde_json::Value;

use super::oneocr::{snap_level_lines, to_lines, Line, Word};
use super::OcrLine;

const SEARCH: &str = "https://www.bing.com/images/search?view=detailv2&iss=sbiupload";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
/// The upload form's limit on the encoded image, as owocr keeps to it.
const MAX_BYTES: usize = 767_772;
/// Sides it takes, as owocr scales them.
const MIN_SIDE: u32 = 50;
const MAX_SIDE: u32 = 4000;

pub struct BingOcr {
    client: reqwest::blocking::Client,
}

impl BingOcr {
    pub fn new() -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
            // Its redirects carry the token.
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { client })
    }

    pub fn recognize(&self, img: &RgbaImage) -> Result<Vec<OcrLine>> {
        let (w, h) = img.dimensions();
        let started = Instant::now();
        let image = base64::engine::general_purpose::STANDARD.encode(encode(img)?);
        let (host, token) = self.upload(&image)?;
        let body = serde_json::json!({
            "imageInfo": {"imageInsightsToken": token, "source": "Url"},
            "knowledgeRequest": {"invokedSkills": ["OCR"], "index": 1},
        });
        let (boundary, form) = multipart(&[("knowledgeRequest", &body.to_string(), Some("application/json"))]);
        let response = self
            .client
            .post(format!("https://{host}/images/api/custom/knowledge"))
            .header("origin", "https://www.bing.com")
            .header("referer", format!("https://www.bing.com/images/search?view=detailV2&insightstoken={token}"))
            .header("content-type", format!("multipart/form-data; boundary={boundary}"))
            .body(form)
            .send()
            .context("Bing request")?;
        let status = response.status();
        if !status.is_success() {
            bail!("o Bing respondeu HTTP {status}");
        }
        let answer: Value = serde_json::from_slice(&response.bytes()?).context("read Bing's answer")?;
        log::info!("Bing answered in {} ms", started.elapsed().as_millis());
        let mut lines = to_lines(&bing_lines(&answer, w as f32, h as f32), 1.0);
        snap_level_lines(img, &mut lines);
        Ok(lines)
    }

    /// Uploads the image; returns the host that answered and the image's token.
    fn upload(&self, image: &str) -> Result<(String, String)> {
        let mut url = reqwest::Url::parse(SEARCH)?;
        // A regional host may answer first with a redirect to itself.
        for _ in 0..3 {
            let (boundary, form) = multipart(&[("imgurl", "", None), ("cbir", "sbi", None), ("imageBin", image, None)]);
            let response = self
                .client
                .post(url.clone())
                .header("origin", "https://www.bing.com")
                .header("content-type", format!("multipart/form-data; boundary={boundary}"))
                .body(form)
                .send()
                .context("Bing upload")?;
            if response.status() != reqwest::StatusCode::FOUND {
                bail!("o Bing respondeu HTTP {} ao envio da imagem", response.status());
            }
            let location = response.headers().get("location").and_then(|l| l.to_str().ok()).context("Bing gave no redirect")?;
            let next = url.join(location)?;
            if let Some((_, token)) = next.query_pairs().find(|(k, _)| k == "insightsToken") {
                return Ok((url.host_str().unwrap_or("www.bing.com").to_string(), token.into_owned()));
            }
            if !location.starts_with("https://") {
                bail!("Bing gave no image token");
            }
            url = next;
        }
        bail!("o Bing redirecionou demais")
    }
}

/// The image as Bing takes it: PNG, 50 to 4000 pixels a side, shrunk until it fits `MAX_BYTES`.
fn encode(img: &RgbaImage) -> Result<Vec<u8>> {
    let (w, h) = img.dimensions();
    let mut scale = if w.max(h) > MAX_SIDE { MAX_SIDE as f32 / w.max(h) as f32 } else if w.min(h) < MIN_SIDE { MIN_SIDE as f32 / w.min(h) as f32 } else { 1.0 };
    for _ in 0..8 {
        let (sw, sh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
        let sized = if (sw, sh) == (w, h) { img.clone() } else { image::imageops::resize(img, sw, sh, FilterType::Lanczos3) };
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(sized).write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).context("encode PNG")?;
        if png.len() <= MAX_BYTES {
            return Ok(png);
        }
        scale *= if w.max(h) as f32 * scale > 2000.0 { 0.6 } else { 0.75 };
    }
    bail!("a captura é grande demais para o Bing")
}

/// A form of `(name, value, content type)` parts, with its boundary.
fn multipart(parts: &[(&str, &str, Option<&str>)]) -> (String, Vec<u8>) {
    let boundary = format!("lookitan{:016x}{:016x}", rand_u64(), rand_u64());
    let mut body = Vec::new();
    for (name, value, kind) in parts {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n").as_bytes());
        if let Some(kind) = kind {
            body.extend_from_slice(format!("Content-Type: {kind}\r\n").as_bytes());
        }
        body.extend_from_slice(format!("\r\n{value}\r\n").as_bytes());
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.finish()
}

/// The answer's lines, in pixels of a `w` × `h` image: the `##TextRecognition` tag's
/// `ImageKnowledge/TextRecognitionAction`, its regions' lines and their words. Nothing when Bing
/// found no text.
pub fn bing_lines(answer: &Value, w: f32, h: f32) -> Vec<Line> {
    let corners = |b: &Value| -> [f32; 8] {
        let mut c = [0f32; 8];
        for (i, corner) in ["topLeft", "topRight", "bottomRight", "bottomLeft"].iter().enumerate() {
            c[2 * i] = b[corner]["x"].as_f64().unwrap_or(0.0) as f32 * w;
            c[2 * i + 1] = b[corner]["y"].as_f64().unwrap_or(0.0) as f32 * h;
        }
        c
    };
    let mut out = Vec::new();
    let tags = answer["tags"].as_array().map(Vec::as_slice).unwrap_or_default();
    for tag in tags.iter().filter(|t| t["displayName"] == "##TextRecognition") {
        let actions = tag["actions"].as_array().map(Vec::as_slice).unwrap_or_default();
        for action in actions.iter().filter(|a| a["_type"] == "ImageKnowledge/TextRecognitionAction") {
            for region in action["data"]["regions"].as_array().map(Vec::as_slice).unwrap_or_default() {
                for line in region["lines"].as_array().map(Vec::as_slice).unwrap_or_default() {
                    let words = line["words"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                        .iter()
                        .map(|word| Word { text: word["text"].as_str().unwrap_or_default().to_string(), corners: corners(&word["boundingBox"]), confidence: 1.0 })
                        .collect();
                    out.push(Line { text: line["text"].as_str().unwrap_or_default().to_string(), corners: corners(&line["boundingBox"]), words });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bing_s_fractions_become_pixels_and_words_lines() {
        // The shape Bing answered with on 2026-10-28 (a synthetic test image, 520 × 90).
        let quad = |x0: f64, y0: f64, x1: f64, y1: f64| serde_json::json!({"topLeft": {"x": x0, "y": y0}, "topRight": {"x": x1, "y": y0}, "bottomRight": {"x": x1, "y": y1}, "bottomLeft": {"x": x0, "y": y1}});
        let answer = serde_json::json!({"tags": [
            {"displayName": "##Other", "actions": []},
            {"displayName": "##TextRecognition", "actions": [{"_type": "ImageKnowledge/TextRecognitionAction", "data": {"regions": [{"lines": [
                {"text": "Hello from", "boundingBox": quad(0.1, 0.2, 0.6, 0.6), "words": [
                    {"text": "Hello", "boundingBox": quad(0.1, 0.2, 0.3, 0.6)},
                    {"text": "from", "boundingBox": quad(0.4, 0.2, 0.6, 0.6)}]}]}]}}]}]});
        let lines = bing_lines(&answer, 500.0, 100.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].words.len(), 2);
        assert!((lines[0].corners[0] - 50.0).abs() < 1e-3 && (lines[0].corners[5] - 60.0).abs() < 1e-3);
        let read = to_lines(&lines, 1.0);
        assert_eq!(read[0].text, "Hello from");
        assert!((read[0].chars[0].x0 - 50.0).abs() < 1e-3);
        assert!(bing_lines(&serde_json::json!({}), 500.0, 100.0).is_empty(), "no text, no lines");
    }

    #[test]
    fn the_upload_form_has_its_parts_and_small_images_grow() {
        let (boundary, body) = multipart(&[("cbir", "sbi", None), ("knowledgeRequest", "{}", Some("application/json"))]);
        let body = String::from_utf8(body).unwrap();
        assert!(body.starts_with(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"cbir\"\r\n\r\nsbi\r\n")));
        assert!(body.contains("Content-Type: application/json\r\n\r\n{}\r\n"));
        assert!(body.ends_with(&format!("--{boundary}--\r\n")));
        let png = encode(&RgbaImage::from_pixel(40, 20, image::Rgba([255, 255, 255, 255]))).unwrap();
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (100, 50), "the short side grows to 50");
    }
}
