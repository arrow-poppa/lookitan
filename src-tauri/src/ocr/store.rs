// The Snipping Tool's package from Microsoft's servers, for OneOCR on a Windows without the Store's
// Snipping Tool (Windows 10, or one where it was removed), at the user's request on 2026-10-28, as
// LunaTranslator and GameSentenceMiner fetch it: store.rg-adguard.net (a third-party site) gives
// the Store's links to the package, and only OneOCR's three files are read out of the newest one,
// over HTTP ranges (the bundle is about 800 MB; the files, about 80 MB compressed).
//
// Microsoft serves those links over plain HTTP, so nothing is kept until it matches the package's
// own signature: AppxSignature.p7x must be a Microsoft Store signature by Microsoft Corporation
// under the Microsoft Root Certificate Authority 2011 (checked by Windows, through PowerShell), the
// AppxBlockMap.xml must have the SHA-256 that signature carries, and every 64 KiB block of each
// file the SHA-256 the block map gives it.

use std::collections::HashMap;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use base64::Engine as _;
use sha2::{Digest, Sha256};

const FINDER: &str = "https://store.rg-adguard.net/api/GetFiles";
const FAMILY: &str = "Microsoft.ScreenSketch_8wekyb3d8bbwe";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";
/// Microsoft's download servers, the only hosts a link may point to.
const HOST: &str = "dl.delivery.mp.microsoft.com";
/// The block map's blocks: 64 KiB of each file.
const BLOCK: u64 = 65_536;
/// Each range asked of the server.
const CHUNK: usize = 1 << 20;

/// Reads OneOCR's files out of the newest Snipping Tool package into `to`, each checked against
/// the package's signature, reporting the bytes written; returns the Snipping Tool's version.
pub fn fetch(to: &Path, progress: &dyn Fn(u64)) -> Result<String> {
    let client = reqwest::blocking::Client::builder().user_agent(USER_AGENT).no_gzip().timeout(Duration::from_secs(60)).build()?;
    let page = client
        .post(FINDER)
        .header("origin", "https://store.rg-adguard.net")
        .header("referer", "https://store.rg-adguard.net/")
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(format!("type=PackageFamilyName&url={FAMILY}&ring=Retail&lang=en-US"))
        .send()?
        .error_for_status()?
        .text()?;
    let link = newest_bundle(&page).context("o store.rg-adguard.net não deu o link do pacote do Recorte")?;
    let bundle = Remote::open(&client, &link)?;

    // The bundle holds a package for each architecture, stored as they are.
    let mut outer = zip::ZipArchive::new(BufReader::with_capacity(CHUNK, bundle.part(0, bundle.len))).context("read the Snipping Tool bundle")?;
    let arch = if cfg!(target_arch = "aarch64") { "_arm64.msix" } else { "_x64.msix" };
    let name = outer
        .file_names()
        .find(|n| n.starts_with("SnippingTool") && n.ends_with(arch))
        .map(str::to_string)
        .with_context(|| format!("the Snipping Tool bundle has no {arch} package"))?;
    let (start, len) = {
        let package = outer.by_name(&name)?;
        ensure!(package.compression() == zip::CompressionMethod::Stored, "the {arch} package is compressed in the bundle");
        (package.data_start(), package.size())
    };
    let version = name.split('_').nth(1).unwrap_or_default().to_string();
    let mut package = zip::ZipArchive::new(BufReader::with_capacity(CHUNK, bundle.part(start, len))).context("read the Snipping Tool package")?;

    let signature = member(&mut package, "AppxSignature.p7x", 1 << 20)?;
    let map = member(&mut package, "AppxBlockMap.xml", 64 << 20)?;
    let signed = signed_block_map_hash(&signature, to)?;
    ensure!(Sha256::digest(&map).as_slice() == signed.as_slice(), "the Snipping Tool's block map is not the one Microsoft signed");
    let files = block_map(&map)?;

    let mut done = 0;
    for f in super::oneocr::FILES {
        let (size, blocks) = files.get(&format!("SnippingTool\\{f}")).with_context(|| format!("the Snipping Tool package has no {f}"))?;
        let mut entry = package.by_name(&format!("SnippingTool/{f}"))?;
        let written = extract(&mut entry, &to.join(f), *size, blocks, &mut |n| {
            done += n;
            progress(done);
        });
        written.with_context(|| format!("{f} from the Snipping Tool {version}"))?;
    }
    Ok(version)
}

/// The newest Snipping Tool bundle among the finder's links, on Microsoft's servers only.
fn newest_bundle(page: &str) -> Option<String> {
    let mut newest: Option<(Vec<u32>, String)> = None;
    for piece in page.split("<a href=\"").skip(1) {
        let Some((link, rest)) = piece.split_once('"') else { continue };
        let Some(name) = rest.split_once('>').and_then(|(_, r)| r.split_once("</a>")).map(|(n, _)| n.trim()) else { continue };
        let Some(version) = name.strip_prefix("Microsoft.ScreenSketch_").and_then(|n| n.strip_suffix("_neutral_~_8wekyb3d8bbwe.msixbundle")) else {
            continue;
        };
        let Ok(version) = version.split('.').map(str::parse::<u32>).collect::<Result<Vec<_>, _>>() else { continue };
        let link = link.replace("&amp;", "&");
        let Ok(url) = reqwest::Url::parse(&link) else { continue };
        let host = url.host_str().unwrap_or_default();
        if !matches!(url.scheme(), "http" | "https") || !(host == HOST || host.ends_with(&format!(".{HOST}"))) {
            continue;
        }
        if newest.as_ref().map_or(true, |(v, _)| version > *v) {
            newest = Some((version, link));
        }
    }
    newest.map(|(_, link)| link)
}

/// A whole small member of a package.
fn member<R: Read + Seek>(archive: &mut zip::ZipArchive<R>, name: &str, max: u64) -> Result<Vec<u8>> {
    let mut file = archive.by_name(name).with_context(|| format!("the Snipping Tool package has no {name}"))?;
    ensure!(file.size() <= max, "{name} is too large");
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The SHA-256 of AppxBlockMap.xml that the package's signature carries, once Windows has found
/// the signature Microsoft's.
fn signed_block_map_hash(signature: &[u8], scratch: &Path) -> Result<Vec<u8>> {
    // The Store's signing certificates last days, so the chain is checked at a time within the
    // signer's validity, as the package was signed; its root must be Microsoft's own.
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }
Add-Type -AssemblyName System.Security
$b = [IO.File]::ReadAllBytes($env:LOOKITAN_P7X)
if ($b.Length -lt 8 -or [Text.Encoding]::ASCII.GetString($b, 0, 4) -ne 'PKCX') { throw 'not a package signature' }
$cms = New-Object System.Security.Cryptography.Pkcs.SignedCms
$cms.Decode($b[4..($b.Length - 1)])
$cms.CheckSignature($true)
if ($cms.ContentInfo.ContentType.Value -ne '1.3.6.1.4.1.311.2.1.4') { throw 'not an Authenticode signature' }
$cert = $cms.SignerInfos[0].Certificate
$chain = New-Object System.Security.Cryptography.X509Certificates.X509Chain
$chain.ChainPolicy.RevocationMode = 'NoCheck'
$chain.ChainPolicy.ExtraStore.AddRange($cms.Certificates)
$chain.ChainPolicy.VerificationTime = $cert.NotBefore.AddSeconds(($cert.NotAfter - $cert.NotBefore).TotalSeconds / 2)
if (-not $chain.Build($cert)) { throw 'the signature does not chain to a trusted root' }
if ($chain.ChainElements[$chain.ChainElements.Count - 1].Certificate.Thumbprint -ne '8F43288AD272F3103B6FB1428485EA3014C0BCFE') { throw 'not under the Microsoft Root Certificate Authority 2011' }
if ($cert.Subject -notmatch '(^|, )O=Microsoft Corporation(,|$)') { throw 'not signed by Microsoft Corporation' }
$eku = $cert.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.37' }
if (-not ($eku.EnhancedKeyUsages | Where-Object { $_.Value -eq '1.3.6.1.4.1.311.76.3.1' })) { throw 'not a Microsoft Store signature' }
[Convert]::ToBase64String($cms.ContentInfo.Content)
"#;
    let path = scratch.join("AppxSignature.p7x");
    std::fs::write(&path, signature)?;
    let script: Vec<u8> = SCRIPT.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let script = base64::engine::general_purpose::STANDARD.encode(script);
    let mut command = std::process::Command::new("powershell");
    command.args(["-NoProfile", "-NonInteractive", "-EncodedCommand", script.as_str()]).env("LOOKITAN_P7X", &path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let out = command.output().context("ask Windows to check the Snipping Tool's signature");
    let _ = std::fs::remove_file(&path);
    let out = out?;
    if !out.status.success() {
        bail!("a assinatura do pacote do Recorte não é da Microsoft: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let content = base64::engine::general_purpose::STANDARD.decode(String::from_utf8_lossy(&out.stdout).trim()).context("read the signed digests")?;
    signed_digest(&content, b"AXBM").context("the signature carries no block map hash")
}

/// One of the digests a package signature's content carries (`AXBM`: the block map's), each a tag
/// and 32 bytes.
fn signed_digest(content: &[u8], tag: &[u8; 4]) -> Option<Vec<u8>> {
    let at = content.windows(4).position(|w| w == tag)? + 4;
    content.get(at..at + 32).map(<[u8]>::to_vec)
}

/// Each file's size and its blocks' SHA-256, from AppxBlockMap.xml (names as it writes them,
/// with backslashes).
fn block_map(xml: &[u8]) -> Result<HashMap<String, (u64, Vec<[u8; 32]>)>> {
    use quick_xml::events::{BytesStart, Event};
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut files = HashMap::new();
    let mut current: Option<String> = None;
    let attribute = |tag: &BytesStart, name: &str, decoder: quick_xml::encoding::Decoder| -> Result<String> {
        let a = tag.try_get_attribute(name)?.with_context(|| format!("block map {} without {name}", String::from_utf8_lossy(tag.name().as_ref())))?;
        Ok(a.decode_and_unescape_value(decoder)?.into_owned())
    };
    loop {
        let event = reader.read_event_into(&mut buf).context("read the block map")?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(tag) | Event::Empty(tag) => match tag.name().as_ref() {
                b"BlockMap" => {
                    let method = attribute(&tag, "HashMethod", reader.decoder())?;
                    ensure!(method == "http://www.w3.org/2001/04/xmlenc#sha256", "the block map hashes with {method}");
                }
                b"File" => {
                    let name = attribute(&tag, "Name", reader.decoder())?;
                    let size = attribute(&tag, "Size", reader.decoder())?.parse::<u64>().context("block map file size")?;
                    files.insert(name.clone(), (size, Vec::new()));
                    current = (!empty).then_some(name);
                }
                b"Block" => {
                    let hash = base64::engine::general_purpose::STANDARD.decode(attribute(&tag, "Hash", reader.decoder())?)?;
                    let hash: [u8; 32] = hash.try_into().map_err(|_| anyhow::anyhow!("a block map hash is not SHA-256"))?;
                    let file = current.as_ref().and_then(|c| files.get_mut(c)).context("a block outside a file in the block map")?;
                    file.1.push(hash);
                }
                _ => {}
            },
            Event::End(tag) if tag.name().as_ref() == b"File" => current = None,
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(files)
}

/// Writes a member to `to`, every block checked against its hash as it is read.
fn extract(entry: &mut impl Read, to: &Path, size: u64, blocks: &[[u8; 32]], progress: &mut dyn FnMut(u64)) -> Result<()> {
    ensure!(blocks.len() as u64 == size.div_ceil(BLOCK), "the block map does not cover the file");
    let mut out = std::fs::File::create(to).with_context(|| format!("create {}", to.display()))?;
    let mut block = vec![0u8; BLOCK as usize];
    for (i, want) in blocks.iter().enumerate() {
        let n = (size - i as u64 * BLOCK).min(BLOCK) as usize;
        entry.read_exact(&mut block[..n])?;
        ensure!(Sha256::digest(&block[..n]).as_slice() == want, "block {i} is not the one Microsoft signed");
        out.write_all(&block[..n])?;
        progress(n as u64);
    }
    ensure!(entry.read(&mut [0u8; 1])? == 0, "longer than the block map says");
    out.flush()?;
    Ok(())
}

/// A part of a file on a server, read in ranges.
struct Remote {
    client: reqwest::blocking::Client,
    url: String,
    /// Where the part starts in the file.
    base: u64,
    len: u64,
    pos: u64,
}

impl Remote {
    fn open(client: &reqwest::blocking::Client, url: &str) -> Result<Self> {
        let head = client.head(url).send()?.error_for_status()?;
        let len = head
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .context("the Snipping Tool package has no size")?;
        Ok(Self { client: client.clone(), url: url.to_string(), base: 0, len, pos: 0 })
    }

    fn part(&self, base: u64, len: u64) -> Self {
        Self { client: self.client.clone(), url: self.url.clone(), base: self.base + base, len, pos: 0 }
    }
}

impl Read for Remote {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = (buf.len() as u64).min(self.len.saturating_sub(self.pos)) as usize;
        if n == 0 {
            return Ok(0);
        }
        let from = self.base + self.pos;
        let mut response = self
            .client
            .get(&self.url)
            .header(reqwest::header::RANGE, format!("bytes={from}-{}", from + n as u64 - 1))
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(std::io::Error::other)?;
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(std::io::Error::other("the server does not send parts of the package"));
        }
        let mut got = 0;
        while got < n {
            match response.read(&mut buf[got..n])? {
                0 => break,
                k => got += k,
            }
        }
        if got == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        self.pos += got as u64;
        Ok(got)
    }
}

impl Seek for Remote {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start"))?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_bundle_on_microsoft_s_servers_is_chosen() {
        let page = r#"<tr><td><a href="http://tlu.dl.delivery.mp.microsoft.com/files/a?P1=1&amp;P2=2" rel="noreferrer">Microsoft.ScreenSketch_2022.2607.23.0_neutral_~_8wekyb3d8bbwe.msixbundle</a></td></tr>
<tr><td><a href="http://tlu.dl.delivery.mp.microsoft.com/files/b?P1=1&amp;P2=2" rel="noreferrer">Microsoft.ScreenSketch_2022.2608.41.0_neutral_~_8wekyb3d8bbwe.msixbundle</a></td></tr>
<tr><td><a href="http://tlu.dl.delivery.mp.microsoft.com/files/c" rel="noreferrer">Microsoft.ScreenSketch_2022.2608.41.0_neutral_~_8wekyb3d8bbwe.BlockMap</a></td></tr>
<tr><td><a href="http://evil.example/x?.dl.delivery.mp.microsoft.com" rel="noreferrer">Microsoft.ScreenSketch_2099.1.1.0_neutral_~_8wekyb3d8bbwe.msixbundle</a></td></tr>
<tr><td><a href="http://tlu.dl.delivery.mp.microsoft.com.evil.example/d" rel="noreferrer">Microsoft.ScreenSketch_2099.1.2.0_neutral_~_8wekyb3d8bbwe.msixbundle</a></td></tr>
<tr><td><a href="http://tlu.dl.delivery.mp.microsoft.com/files/e" rel="noreferrer">Microsoft.ScreenSketch_2022.999.1.0_neutral_~_8wekyb3d8bbwe.msixbundle</a></td></tr>"#;
        assert_eq!(newest_bundle(page).as_deref(), Some("http://tlu.dl.delivery.mp.microsoft.com/files/b?P1=1&P2=2"));
        assert_eq!(newest_bundle("<html>no links</html>"), None);
    }

    #[test]
    fn the_block_map_gives_each_file_its_blocks_and_they_are_checked() {
        let data: Vec<u8> = (0..BLOCK + 10).map(|i| (i % 251) as u8).collect();
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(Sha256::digest(b));
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><BlockMap xmlns="http://schemas.microsoft.com/appx/2010/blockmap" HashMethod="http://www.w3.org/2001/04/xmlenc#sha256"><File Name="SnippingTool\oneocr.dll" Size="{}" LfhSize="60"><Block Hash="{}" Size="100"/><Block Hash="{}"/></File><File Name="empty.txt" Size="0" LfhSize="39"/></BlockMap>"#,
            data.len(),
            b64(&data[..BLOCK as usize]),
            b64(&data[BLOCK as usize..]),
        );
        let files = block_map(xml.as_bytes()).unwrap();
        let (size, blocks) = &files["SnippingTool\\oneocr.dll"];
        assert_eq!((*size, blocks.len()), (data.len() as u64, 2));
        assert_eq!(files["empty.txt"], (0, Vec::new()));

        let dir = std::env::temp_dir().join(format!("lookitan-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut written = 0;
        extract(&mut data.as_slice(), &dir.join("ok"), *size, blocks, &mut |n| written += n).unwrap();
        assert_eq!((std::fs::read(dir.join("ok")).unwrap(), written), (data.clone(), data.len() as u64));
        let mut changed = data.clone();
        changed[BLOCK as usize + 3] ^= 1;
        assert!(extract(&mut changed.as_slice(), &dir.join("bad"), *size, blocks, &mut |_| {}).is_err());
        let mut longer = data.clone();
        longer.push(0);
        assert!(extract(&mut longer.as_slice(), &dir.join("long"), *size, blocks, &mut |_| {}).is_err());
        let _ = std::fs::remove_dir_all(&dir);

        let other = xml.replace("xmlenc#sha256", "xmldsig#sha1");
        assert!(block_map(other.as_bytes()).is_err());
    }

    #[test]
    fn the_block_map_digest_is_found_in_the_signed_content() {
        let mut content = b"0\x82\x01\x04APPXAXPC".to_vec();
        content.extend([1u8; 32]);
        content.extend(b"AXBM");
        content.extend([7u8; 32]);
        assert_eq!(signed_digest(&content, b"AXBM"), Some(vec![7u8; 32]));
        assert_eq!(signed_digest(&content[..content.len() - 1], b"AXBM"), None);
        assert_eq!(signed_digest(b"none", b"AXBM"), None);
    }

    /// Fetches OneOCR's files from Microsoft's servers into `LOOKITAN_ONEOCR` as the app does,
    /// unless they are there already (CI caches them); skipped without it. Windows only (the
    /// signature is checked by Windows).
    #[test]
    #[ignore]
    fn real_store_fetches_oneocr() {
        let Some(dir) = std::env::var_os("LOOKITAN_ONEOCR").map(std::path::PathBuf::from) else {
            eprintln!("LOOKITAN_ONEOCR is not set; skipped");
            return;
        };
        if super::super::oneocr::FILES.iter().all(|f| dir.join(f).is_file()) {
            eprintln!("OneOCR's files are cached");
            return;
        }
        // Into a folder of its own first, so a failed fetch leaves nothing to cache.
        let partial = dir.with_extension("partial");
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial).unwrap();
        let started = std::time::Instant::now();
        let version = fetch(&partial, &|_| {}).expect("fetch OneOCR");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&partial, &dir).unwrap();
        let bytes: u64 = super::super::oneocr::FILES.iter().map(|f| std::fs::metadata(dir.join(f)).unwrap().len()).sum();
        eprintln!("Snipping Tool {version}: {bytes} bytes in {} s", started.elapsed().as_secs());
    }
}
