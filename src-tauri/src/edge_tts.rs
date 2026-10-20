//! Microsoft Edge's "Read aloud" neural voices, for word audio and reading the context aloud.
//!
//! Unofficial: the protocol of rany2/edge-tts, as Fushi (GPL-3.0) uses it; behavior only, nothing
//! copied. A websocket with the Edge client's token, a Sec-MS-GEC hash of the clock and an Edge
//! version, answering MP3. Since 2026-10-13 the service wants the v2 Sec-MS-GEC (edge-tts 7.3):
//! with the old v1 hash speech.platform.bing.com answers 403 and Edge 155's host
//! (api.msedgeservices.com, the token as `Ocp-Apim-Subscription-Key`) takes only some of the
//! connections. With v2 the old host takes nearly all, so it is tried first and Edge 155's host
//! after it. Microsoft can change or throttle it at any time, so the settings call it an
//! unofficial online service, and a failure falls back to the next audio source.

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
/// The synthesis hosts, in the order tried: host, path, the token's parameter name.
const SYNTH_HOSTS: [(&str, &str, &str); 2] = [
    ("speech.platform.bing.com", "/consumer/speech/synthesize/readaloud/edge/v1", "TrustedClientToken"),
    ("api.msedgeservices.com", "/tts/cognitiveservices/websocket/v1", "Ocp-Apim-Subscription-Key"),
];
/// Connections tried per piece, going round the hosts: the service drops some at random.
const ATTEMPTS: usize = 3;
const VOICES_URL: &str = "https://speech.platform.bing.com/consumer/speech/synthesize/readaloud/voices/list";
const TIMEOUT: Duration = Duration::from_secs(20);
/// Longer texts are read in pieces of at most this many characters, split after punctuation.
const PIECE_CHARS: usize = 600;
/// Recordings kept in memory, so a word or sentence played again is not fetched again.
const KEPT: usize = 24;

/// One voice of the list, as the settings show it. Read with Microsoft's field names, sent to the
/// front end as `id`, `locale`, `gender` and `name`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Voice {
    #[serde(rename(deserialize = "ShortName"))]
    pub id: String,
    #[serde(rename(deserialize = "Locale"))]
    pub locale: String,
    #[serde(rename(deserialize = "Gender"), default)]
    pub gender: String,
    #[serde(rename(deserialize = "FriendlyName"), default)]
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

/// Sec-MS-GEC, version 2: the Windows file time (100 ns ticks, rounded down to 5 minutes)
/// followed by the token, hashed by Edge's keyed SHA-256 and its CS64 checksum of that hash, as
/// "<CS64, 16 hex>-<SHA-256, 64 hex>" in upper case. The constants are Edge's, as edge-tts 7.3
/// (drm.py) reads them; the code below is plain SHA-256 rounds over them.
pub fn sec_ms_gec(unix: i64) -> String {
    let mut seconds = unix + 11_644_473_600;
    seconds -= seconds.rem_euclid(300);
    let data = format!("{}{TOKEN}", seconds as u128 * 10_000_000).into_bytes();
    let digest = gec::sha256(&data);
    let (low, high) = gec::cs64(&data, &digest);
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02X}")).collect::<String>();
    format!("{}{}-{}", hex(&low.to_le_bytes()), hex(&high.to_le_bytes()), hex(&digest))
}

mod gec {
    //! SHA-256 keyed by a secret 57-byte prefix, whose first block comes as a partly hashed state
    //! and schedule, and Edge's CS64 checksum.

    const INITIAL: [u32; 8] = [0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19];
    const STATE: [u32; 8] = [0xFD9FBEA2, 0x91243CE7, 0xBE499094, 0xCD2E1E0A, 0x6C006F49, 0x04BD35AB, 0xFEB0A7E4, 0x03EFFBD2];
    const SCHEDULE: [u32; 14] = [0xEB24BF9E, 0x1558691F, 0xA0C09EE2, 0x0CFA32BA, 0xA622EE8C, 0xD1DD9491, 0xC0DD25A0,
        0xF9E70AB6, 0xF0C31573, 0x7058CE4E, 0x7188E606, 0x00E5AB33, 0xD258B060, 0x72766564];
    const K: [u32; 64] = [
        0x428A2F98, 0x71374491, 0xB5C0FBCF, 0xE9B5DBA5, 0x3956C25B, 0x59F111F1, 0x923F82A4, 0xAB1C5ED5,
        0xD807AA98, 0x12835B01, 0x243185BE, 0x550C7DC3, 0x72BE5D74, 0x80DEB1FE, 0x9BDC06A7, 0xC19BF174,
        0xE49B69C1, 0xEFBE4786, 0x0FC19DC6, 0x240CA1CC, 0x2DE92C6F, 0x4A7484AA, 0x5CB0A9DC, 0x76F988DA,
        0x983E5152, 0xA831C66D, 0xB00327C8, 0xBF597FC7, 0xC6E00BF3, 0xD5A79147, 0x06CA6351, 0x14292967,
        0x27B70A85, 0x2E1B2138, 0x4D2C6DFC, 0x53380D13, 0x650A7354, 0x766A0ABB, 0x81C2C92E, 0x92722C85,
        0xA2BFE8A1, 0xA81A664B, 0xC24B8B70, 0xC76C51A3, 0xD192E819, 0xD6990624, 0xF40E3585, 0x106AA070,
        0x19A4C116, 0x1E376C08, 0x2748774C, 0x34B0BCB5, 0x391C0CB3, 0x4ED8AA4A, 0x5B9CCA4F, 0x682E6FF3,
        0x748F82EE, 0x78A5636F, 0x84C87814, 0x8CC70208, 0x90BEFFFA, 0xA4506CEB, 0xBEF9A3F7, 0xC67178F2,
    ];
    /// Round 14's constant in the first block, and the adjustments of its schedule.
    const ADJUSTMENT: u32 = 0xC9DC06A7;
    const SIGMA0_ADJUSTMENT: u32 = 0x059C0B80;
    const SIGMA1_ADJUSTMENT: u32 = 0x000B92C0;
    const CS64_COEFFICIENTS: [(u32, u32); 14] = [
        (0xE82E91B1, 0xB32700D0), (0x44DE24C6, 0x88DFF51D), (0x3BEB65BC, 0x29514038), (0xC89C5933, 0xB328CBDF),
        (0x8D1EC211, 0xD405B4D5), (0x314EF5D5, 0x26E55F3C), (0xE92491B1, 0x607801C6), (0xF0A5B9D5, 0x82B2C1FB),
        (0x06064CF3, 0xFA51A3DA), (0xDA79FA60, 0x207CC464), (0xBED20560, 0x3C3C74F3), (0x89F9C9F8, 0xD2D14B61),
        (0x35FA38C6, 0xBB04A2C1), (0xCB49E3C6, 0xC5626167),
    ];
    const CS64_FACTOR: u32 = 0x82893933;
    const CS64_ADJUSTMENT0: u32 = 0xF2F90B1A;
    const CS64_ADJUSTMENT1: u32 = 0x0B1A0000;

    fn sigma0(x: u32) -> u32 { x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3) }
    fn sigma1(x: u32) -> u32 { x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10) }
    fn rol16(x: u32) -> u32 { x.rotate_left(16) }

    fn expand(w: &mut [u32; 64], from: usize) {
        for i in from..64 {
            w[i] = sigma1(w[i - 2]).wrapping_add(w[i - 7]).wrapping_add(sigma0(w[i - 15])).wrapping_add(w[i - 16]);
        }
    }

    /// SHA-256's rounds `from..64` from the registers `start`, added to `state`.
    fn compress(state: [u32; 8], start: [u32; 8], w: &[u32; 64], from: usize, k: &[u32; 64]) -> [u32; 8] {
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = start;
        for i in from..64 {
            let t1 = h.wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add(g ^ (e & (f ^ g))).wrapping_add(k[i]).wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22)).wrapping_add((a & b) | (c & (a | b)));
            (h, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        let mut out = state;
        for (o, v) in out.iter_mut().zip([a, b, c, d, e, f, g, h]) { *o = o.wrapping_add(v); }
        out
    }

    /// The state after the first block: the secret prefix and the first 7 bytes of `data`.
    fn first_block(data: &[u8]) -> [u32; 8] {
        let mut w = [0u32; 64];
        w[14] = u32::from_be_bytes([0, data[0], data[1], data[2]]);
        w[15] = u32::from_be_bytes([data[3], data[4], data[5], data[6]]);
        for i in 16..30 {
            let mut v = SCHEDULE[i - 16].wrapping_add(if i == 16 { sigma1(w[i - 2]) ^ SIGMA1_ADJUSTMENT } else { sigma1(w[i - 2]) });
            if i >= 21 { v = v.wrapping_add(w[i - 7]); }
            if i == 29 { v = v.wrapping_add(sigma0(w[14]) ^ SIGMA0_ADJUSTMENT); }
            w[i] = v;
        }
        w[30] = sigma1(w[28]).wrapping_add(w[23]).wrapping_add(sigma0(w[15])).wrapping_add(w[14])
            .wrapping_add(ADJUSTMENT).wrapping_sub(K[14]);
        expand(&mut w, 31);
        let mut k = K;
        k[14] = ADJUSTMENT;
        compress(INITIAL, STATE, &w, 14, &k)
    }

    /// The keyed hash of `data` (at least 7 bytes).
    pub fn sha256(data: &[u8]) -> [u8; 32] {
        let length = 57 + data.len();
        let mut padded = data[7..].to_vec();
        padded.push(0x80);
        padded.resize(padded.len() + (55 - length as i64).rem_euclid(64) as usize, 0);
        padded.extend_from_slice(&(length as u64 * 8).to_be_bytes());
        let mut state = first_block(data);
        for block in padded.chunks(64) {
            let mut w = [0u32; 64];
            for (i, word) in block.chunks(4).enumerate() { w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]); }
            expand(&mut w, 16);
            state = compress(state, state, &w, 0, &K);
        }
        let mut out = [0u8; 32];
        for (chunk, v) in out.chunks_mut(4).zip(state) { chunk.copy_from_slice(&v.to_be_bytes()); }
        out
    }

    /// Edge's CS64 checksum of `data`, keyed by the first 8 bytes of its hash.
    pub fn cs64(data: &[u8], digest: &[u8; 32]) -> (u32, u32) {
        let key_a = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) | 1;
        let key_b = u32::from_le_bytes([digest[4], digest[5], digest[6], digest[7]]) | 1;
        let factors = [key_a.wrapping_mul(CS64_FACTOR), key_b.wrapping_mul(CS64_FACTOR)];
        let mut terms: Vec<u32> = CS64_COEFFICIENTS.iter().enumerate().map(|(i, &(c, _))| c.wrapping_mul(factors[i % 2])).collect();
        let mut offsets: Vec<u32> = CS64_COEFFICIENTS.iter().map(|&(_, o)| o).collect();
        // A zero byte and the data, as little-endian words, padded to an even count.
        let mut bytes = vec![0u8];
        bytes.extend_from_slice(data);
        bytes.resize(bytes.len().div_ceil(8) * 8, 0);
        for (i, word) in bytes.chunks(4).enumerate() {
            let word = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
            let even = i % 2 == 0;
            let mut term = word.wrapping_mul(if even { key_a } else { key_b });
            let mut offset = rol16(word).wrapping_mul(if even { 0xEF0569FB } else { 0xC31713DB });
            if i == 0 {
                term = term.wrapping_add(CS64_ADJUSTMENT0.wrapping_mul(factors[0]));
                offset = offset.wrapping_add(CS64_ADJUSTMENT1);
            }
            terms.push(term);
            offsets.push(offset);
        }
        let (mut swap_acc, mut swap_last, mut state, mut acc) = (0u32, 0u32, 0u32, 0u32);
        for i in (0..terms.len()).step_by(2) {
            let (x, y) = (terms[i], terms[i + 1]);
            let q = rol16(offsets[i].wrapping_add(x)).wrapping_mul(0x76F50F34);
            swap_acc = swap_acc.wrapping_add(rol16(q).wrapping_mul(0xC3EFEA97).wrapping_add(q));
            let q = rol16(offsets[i + 1].wrapping_add(y)).wrapping_mul(0x1881CE3C);
            swap_last = rol16(q).wrapping_mul(0x35BD1EC9).wrapping_add(q);
            swap_acc = swap_acc.wrapping_add(swap_last);
            state = state.wrapping_mul(key_a).wrapping_add(x);
            for m in [0xCF98B111u32, 0x87085B9F, 0x12CEB96D, 0x257E1D83] { state = rol16(state).wrapping_mul(m); }
            acc = acc.wrapping_add(state);
            state = state.wrapping_mul(key_b).wrapping_add(y);
            for m in [0xA27416F5u32, 0xD38396FF, 0x7C932B89, 0xBFA49F69] { state = rol16(state).wrapping_mul(m); }
            acc = acc.wrapping_add(state);
        }
        (swap_last ^ state, swap_acc ^ acc)
    }
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

fn synthesize_piece(text: &str, voice: &str, (host, path, key): (&str, &str, &str)) -> Result<Vec<u8>> {
    let url = format!("wss://{host}{path}?{key}={TOKEN}&Sec-MS-GEC={}&Sec-MS-GEC-Version=2-{CHROMIUM}&ConnectionId={}",
        sec_ms_gec(now()), request_id());
    let mut request = url.into_client_request()?;
    let headers = request.headers_mut();
    headers.insert("Pragma", HeaderValue::from_static("no-cache"));
    headers.insert("Cache-Control", HeaderValue::from_static("no-cache"));
    headers.insert("Origin", HeaderValue::from_static(ORIGIN));
    headers.insert("Accept-Language", HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert("User-Agent", HeaderValue::from_str(&user_agent())?);
    headers.insert("Cookie", HeaderValue::from_str(&format!("muid={};", request_id().to_uppercase()))?);

    let address = (host, 443).to_socket_addrs()?.next().context("no address for the voice service")?;
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

/// One piece, trying the hosts in turn; the first 403 also sets the clock from Microsoft's.
fn synthesize_once(text: &str, voice: &str) -> Result<Vec<u8>> {
    let mut clock_synced = false;
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        let host = SYNTH_HOSTS[attempt % SYNTH_HOSTS.len()];
        match synthesize_piece(text, voice, host) {
            Ok(audio) => return Ok(audio),
            Err(e) => {
                if e.is::<Rejected>() && !clock_synced { clock_synced = sync_clock(); }
                log::warn!("Edge voice, attempt {} on {}: {e:#}", attempt + 1, host.0);
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("the voice service did not answer")))
}

/// The voices Edge offers (fetched once per run).
pub fn voices() -> Result<Vec<Voice>> {
    let cache = VOICES.get_or_init(|| Mutex::new(None));
    if let Some(list) = cache.lock().as_ref() { return Ok(list.clone()); }
    let url = format!("{VOICES_URL}?trustedclienttoken={TOKEN}&Sec-MS-GEC={}&Sec-MS-GEC-Version=2-{CHROMIUM}", sec_ms_gec(now()));
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
    }

    #[test]
    fn sec_ms_gec_is_edge_tts_7_3s_v2_token() {
        // Values from edge-tts 7.3.1's generate_sec_ms_gec_v2.
        assert_eq!(sec_ms_gec(0), "52BC7A17B50DFEC9-28C4394775C8751CD2F6859C030C06DB1489ACAB3B3A7651831FC26895177ED6");
        assert_eq!(sec_ms_gec(1_700_000_000), "2CD23A2CA0B2595A-F3C846C370E05F9CC83ACC5F995C5F5D87A4D0F14CE8936A0F902E64099E38A2");
        assert_eq!(sec_ms_gec(1_792_499_696), "DC588E2A6D03411A-AFC50DB6E15D1D6EBFDE7CAB419EC4897730487706D2964B657EAE2D2EC5A514");
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

    #[test]
    fn voices_are_read_with_microsofts_names_and_sent_with_ours() {
        let list: Vec<Voice> = serde_json::from_str(r#"[{"Name":"x","ShortName":"ja-JP-NanamiNeural","Gender":"Female","Locale":"ja-JP","FriendlyName":"Nanami"}]"#).unwrap();
        let sent = serde_json::to_value(&list[0]).unwrap();
        assert_eq!(sent["id"], "ja-JP-NanamiNeural");
        assert_eq!(sent["locale"], "ja-JP");
        assert_eq!(sent["gender"], "Female");
    }
}
