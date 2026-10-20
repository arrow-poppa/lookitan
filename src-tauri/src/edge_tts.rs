//! Microsoft Edge's "Read aloud" neural voices, for word audio and reading the context aloud.
//!
//! Unofficial: the protocol of rany2/edge-tts, as Fushi (GPL-3.0) uses it; behavior only, nothing
//! copied. A websocket with the Edge client's token, a Sec-MS-GEC hash of the clock and an Edge
//! version, answering MP3. Since 2026-10 the old synthesis host (speech.platform.bing.com) answers
//! 403 everywhere; Edge 155's host takes the same token as `Ocp-Apim-Subscription-Key`. The voice
//! list is still on the old host. Microsoft can change or throttle it at any time, so the settings
//! call it an unofficial online service, and a failure falls back to the next audio source.

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::Message;

const TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
/// The Edge version the client says it is (follows edge-tts; bump it with the token if refused).
const CHROMIUM: &str = "143.0.3650.75";
const ORIGIN: &str = "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold";
const SYNTH_HOST: &str = "api.msedgeservices.com";
const SYNTH_PATH: &str = "/tts/cognitiveservices/websocket/v1";
const VOICES_URL: &str = "https://speech.platform.bing.com/consumer/speech/synthesize/readaloud/voices/list";
const TIMEOUT: Duration = Duration::from_secs(20);
/// Longer texts are read in pieces of at most this many characters, split after punctuation.
const PIECE_CHARS: usize = 600;
/// Recordings kept in memory, so a word or sentence played again is not fetched again.
const KEPT: usize = 24;

/// One voice of the list, as the settings show it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Voice {
    #[serde(rename = "ShortName")]
    pub id: String,
    #[serde(rename = "Locale")]
    pub locale: String,
    #[serde(rename = "Gender", default)]
    pub gender: String,
    #[serde(rename = "FriendlyName", default)]
    pub name: String,
}

/// Seconds the local clock is behind Microsoft's (set from the server's Date after a 403).
static SKEW: AtomicI64 = AtomicI64::new(0);
static VOICES: OnceLock<Mutex<Option<Vec<Voice>>>> = OnceLock::new();
static RECORDINGS: OnceLock<Mutex<Vec<((String, String), Vec<u8>)>>> = OnceLock::new();

fn user_agent() -> String {
    let major = CHROMIUM.split('.').next().unwrap_or(CHROMIUM);
    format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36 Edg/{major}.0.0.0")
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) + SKEW.load(Ordering::Relaxed)
}

/// Sec-MS-GEC: SHA-256 of the Windows file time (100 ns ticks, rounded down to 5 minutes) followed
/// by the token, in upper-case hex.
pub fn sec_ms_gec(unix: i64) -> String {
    let mut seconds = unix + 11_644_473_600;
    seconds -= seconds.rem_euclid(300);
    let ticks = seconds as u128 * 10_000_000;
    Sha256::digest(format!("{ticks}{TOKEN}").as_bytes()).iter().map(|b| format!("{b:02X}")).collect()
}

/// 32 hex digits, unique per request.
fn request_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let seed = format!("{nanos}-{}-{}", COUNTER.fetch_add(1, Ordering::Relaxed), std::process::id());
    Sha256::digest(seed.as_bytes())[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// The X-Timestamp the Edge client sends: JavaScript's Date#toString in UTC.
fn js_date(unix: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{} {} {day:02} {year} {:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)",
        DAYS[days.rem_euclid(7) as usize], MONTHS[(month - 1) as usize], secs / 3600, secs / 60 % 60, secs % 60)
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\'', "&apos;").replace('"', "&quot;")
}

/// The text in pieces of at most PIECE_CHARS characters, cut after the last punctuation or space.
fn pieces(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.trim().chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + PIECE_CHARS).min(chars.len());
        if end < chars.len() {
            if let Some(cut) = (start + 1..=end).rev().find(|&i| "。．.!?！？\n、,，;； ".contains(chars[i - 1])) { end = cut; }
        }
        let piece: String = chars[start..end].iter().collect();
        if !piece.trim().is_empty() { out.push(piece.trim().to_string()); }
        start = end;
    }
    out
}

/// The handshake was refused (403): usually the clock is off and the token expired.
#[derive(Debug)]
struct Rejected;
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("o serviço de voz da Microsoft recusou a conexão (403)") }
}
impl std::error::Error for Rejected {}

fn synthesize_piece(text: &str, voice: &str) -> Result<Vec<u8>> {
    let url = format!("wss://{SYNTH_HOST}{SYNTH_PATH}?Ocp-Apim-Subscription-Key={TOKEN}&Sec-MS-GEC={}&Sec-MS-GEC-Version=1-{CHROMIUM}&ConnectionId={}",
        sec_ms_gec(now()), request_id());
    let mut request = url.into_client_request()?;
    let headers = request.headers_mut();
    headers.insert("Pragma", HeaderValue::from_static("no-cache"));
    headers.insert("Cache-Control", HeaderValue::from_static("no-cache"));
    headers.insert("Origin", HeaderValue::from_static(ORIGIN));
    headers.insert("Accept-Language", HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert("User-Agent", HeaderValue::from_str(&user_agent())?);
    headers.insert("Cookie", HeaderValue::from_str(&format!("muid={};", request_id().to_uppercase()))?);

    let address = (SYNTH_HOST, 443).to_socket_addrs()?.next().context("no address for the voice service")?;
    let stream = TcpStream::connect_timeout(&address, TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let (mut socket, _) = match tungstenite::client_tls(request, stream) {
        Ok(connected) => connected,
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response))) if response.status() == 403 => return Err(Rejected.into()),
        Err(e) => bail!("voice service: {e}"),
    };

    let stamp = js_date(now());
    socket.send(Message::text(format!(
        "X-Timestamp:{stamp}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n\
         {{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"false\"}},\
         \"outputFormat\":\"audio-24khz-48kbitrate-mono-mp3\"}}}}}}}}\r\n")))?;
    socket.send(Message::text(format!(
        "X-RequestId:{}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{stamp}Z\r\nPath:ssml\r\n\r\n\
         <speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'><voice name='{}'>\
         <prosody pitch='+0Hz' rate='+0%' volume='+0%'>{}</prosody></voice></speak>",
        request_id(), escape_xml(voice), escape_xml(text))))?;

    let mut audio = Vec::new();
    loop {
        let message = match socket.read() {
            Ok(message) => message,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => bail!("the voice service did not answer in time"),
            Err(e) => bail!("voice service: {e}"),
        };
        match message {
            Message::Text(text) if text.as_str().contains("Path:turn.end") => break,
            Message::Binary(data) if data.len() >= 2 => {
                let header = usize::from(u16::from_be_bytes([data[0], data[1]]));
                if data.len() >= 2 + header && String::from_utf8_lossy(&data[2..2 + header]).contains("Path:audio") {
                    audio.extend_from_slice(&data[2 + header..]);
                }
            }
            Message::Close(_) => bail!("the voice service closed the connection early"),
            _ => {}
        }
    }
    let _ = socket.close(None);
    if audio.is_empty() { bail!("the voice service sent no audio"); }
    Ok(audio)
}

/// After a 403: take the clock from Microsoft's Date header, as edge-tts does.
fn sync_clock() -> bool {
    let Ok(client) = reqwest::blocking::Client::builder().timeout(TIMEOUT).build() else { return false };
    let Ok(response) = client.head("https://speech.platform.bing.com/").send() else { return false };
    let Some(server) = response.headers().get("date").and_then(|v| v.to_str().ok()).and_then(parse_http_date) else { return false };
    let local = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    SKEW.store(server - local, Ordering::Relaxed);
    true
}

/// "Tue, 20 Oct 2026 12:34:56 GMT" as Unix seconds.
fn parse_http_date(text: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() < 5 { return None; }
    let day: i64 = parts[1].parse().ok()?;
    let month = MONTHS.iter().position(|m| *m == parts[2])? as i64 + 1;
    let year: i64 = parts[3].parse().ok()?;
    let time: Vec<i64> = parts[4].split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    if time.len() != 3 { return None; }
    // Howard Hinnant's days_from_civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + time[0] * 3600 + time[1] * 60 + time[2])
}

fn synthesize_once(text: &str, voice: &str) -> Result<Vec<u8>> {
    match synthesize_piece(text, voice) {
        Err(e) if e.is::<Rejected>() && sync_clock() => synthesize_piece(text, voice),
        other => other,
    }
}

/// The voices Edge offers (fetched once per run).
pub fn voices() -> Result<Vec<Voice>> {
    let cache = VOICES.get_or_init(|| Mutex::new(None));
    if let Some(list) = cache.lock().as_ref() { return Ok(list.clone()); }
    let url = format!("{VOICES_URL}?trustedclienttoken={TOKEN}&Sec-MS-GEC={}&Sec-MS-GEC-Version=1-{CHROMIUM}", sec_ms_gec(now()));
    let client = reqwest::blocking::Client::builder().timeout(TIMEOUT).user_agent(user_agent()).build()?;
    let response = client.get(url).header("Origin", ORIGIN).send()?;
    if !response.status().is_success() { bail!("voice list: HTTP {}", response.status()); }
    let mut list: Vec<Voice> = serde_json::from_str(&response.text()?)?;
    list.sort_by(|a, b| a.locale.cmp(&b.locale).then(a.id.cmp(&b.id)));
    *cache.lock() = Some(list.clone());
    Ok(list)
}

/// A well-known voice per language, so a source works before a voice is chosen.
fn default_voice(language: &str) -> Option<String> {
    let base = language.split(['-', '_']).next().unwrap_or("").to_lowercase();
    let known = match base.as_str() {
        "ja" => "ja-JP-NanamiNeural",
        "en" => "en-US-AriaNeural",
        "pt" => if language.eq_ignore_ascii_case("pt-PT") { "pt-PT-RaquelNeural" } else { "pt-BR-FranciscaNeural" },
        "es" => "es-ES-ElviraNeural",
        "fr" => "fr-FR-DeniseNeural",
        "de" => "de-DE-KatjaNeural",
        "it" => "it-IT-ElsaNeural",
        "zh" => "zh-CN-XiaoxiaoNeural",
        "ko" => "ko-KR-SunHiNeural",
        "ru" => "ru-RU-SvetlanaNeural",
        _ => "",
    };
    if !known.is_empty() { return Some(known.to_string()); }
    let list = voices().ok()?;
    list.iter().find(|v| v.locale.eq_ignore_ascii_case(language))
        .or_else(|| list.iter().find(|v| v.locale.to_lowercase().starts_with(&format!("{base}-"))))
        .map(|v| v.id.clone())
}

/// `text` read by `voice` (or the language's default voice) as MP3. Recent recordings are kept.
pub fn speak(text: &str, voice: &str, language: &str) -> Result<Vec<u8>> {
    let voice = if voice.trim().is_empty() { default_voice(language).ok_or_else(|| anyhow!("nenhuma voz do Edge para {language}"))? } else { voice.trim().to_string() };
    let key = (voice.clone(), text.trim().to_string());
    let cache = RECORDINGS.get_or_init(|| Mutex::new(Vec::new()));
    if let Some(found) = cache.lock().iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()) { return Ok(found); }
    let mut audio = Vec::new();
    for piece in pieces(text) { audio.extend(synthesize_once(&piece, &voice)?); }
    if audio.is_empty() { bail!("nothing to read"); }
    let mut kept = cache.lock();
    kept.retain(|(k, _)| *k != key);
    kept.insert(0, (key, audio.clone()));
    kept.truncate(KEPT);
    Ok(audio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sec_ms_gec_rounds_to_five_minutes() {
        assert_eq!(sec_ms_gec(1_700_000_000), sec_ms_gec(1_700_000_000 - 1_700_000_000 % 300));
        assert_ne!(sec_ms_gec(1_700_000_000), sec_ms_gec(1_700_000_000 + 300));
        assert_eq!(sec_ms_gec(0).len(), 64);
        assert!(sec_ms_gec(0).chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
    }

    #[test]
    fn dates_match_javascript_and_http() {
        assert_eq!(js_date(0), "Thu Jan 01 1970 00:00:00 GMT+0000 (Coordinated Universal Time)");
        assert_eq!(js_date(1_792_499_696), "Tue Oct 20 2026 12:34:56 GMT+0000 (Coordinated Universal Time)");
        assert_eq!(parse_http_date("Tue, 20 Oct 2026 12:34:56 GMT"), Some(1_792_499_696));
        assert_eq!(parse_http_date("nonsense"), None);
    }

    #[test]
    fn long_text_is_read_in_pieces_cut_after_punctuation() {
        assert_eq!(pieces("  読む  "), ["読む"]);
        let sentence = "あ".repeat(400) + "。";
        let parts = pieces(&sentence.repeat(3));
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.ends_with('。') && p.chars().count() <= PIECE_CHARS));
        assert_eq!(escape_xml("a<b & 'c'"), "a&lt;b &amp; &apos;c&apos;");
    }
}
