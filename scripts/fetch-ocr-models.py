"""Downloads an optional OCR model set (src-tauri/src/ocr/models.rs) into a folder, as the app does
when the user picks its engine: from the pinned Hugging Face revision, each file checked against its
SHA-256. For CI's real-model tests; the installers do not carry these models.

Usage: python scripts/fetch-ocr-models.py <MODEL SET STATIC, e.g. MEIKI> <folder>
       python scripts/fetch-ocr-models.py SCREENAI:<windows|linux> <folder>
       python scripts/fetch-ocr-models.py SCREENAI-DIGEST:<windows|linux> -
Chrome ScreenAI's package comes from Chromium's CIPD instead: the instance its `latest` ref points to,
as src-tauri/src/ocr/screenai.rs downloads it; SCREENAI-DIGEST prints that instance's SHA-256.
"""
import hashlib
import pathlib
import re
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
name, out = sys.argv[1], pathlib.Path(sys.argv[2])


def cipd(method: str, body: dict) -> dict:
    """A CIPD pRPC call; its JSON answer follows an anti-XSSI line."""
    import json
    request = urllib.request.Request(
        "https://chrome-infra-packages.appspot.com/prpc/cipd.Repository/" + method,
        data=json.dumps(body).encode(),
        headers={"content-type": "application/json", "accept": "application/json"})
    answer = urllib.request.urlopen(request, timeout=60).read().decode()
    return json.loads(answer[answer.index("{"):])


def screenai_latest(platform: str) -> tuple[str, str]:
    """The CIPD package for `platform` (ocr/screenai.rs) and the SHA-256 of its latest instance."""
    source = (ROOT / "src-tauri/src/ocr/screenai.rs").read_text(encoding="utf-8")
    cfg = 'windows' if platform == 'windows' else 'target_os = "linux"'
    block = re.search(r"#\[cfg\(" + re.escape(cfg) + r"\)\]\s*static PACKAGE: Option<Package> = Some\(Package \{(.*?)\}\);", source, re.S)
    if not block:
        sys.exit(f"no ScreenAI package for {platform} in screenai.rs")
    package = re.search(r'name: "([^"]+)"', block.group(1)).group(1)
    instance = cipd("ResolveVersion", {"package": package, "version": "latest"})["instance"]
    digest = instance["hexDigest"]
    if instance["hashAlgo"] != "SHA256" or not re.fullmatch(r"[0-9a-f]{64}", digest):
        sys.exit("CIPD's latest ScreenAI is not a SHA-256 instance")
    return package, digest


def screenai(platform: str) -> None:
    """Chrome ScreenAI's latest CIPD instance for `platform`, unpacked in `out`."""
    import zipfile
    package, digest = screenai_latest(platform)
    if (out / "resources").is_dir() and (out / ".digest").is_file() and (out / ".digest").read_text() == digest:
        print("ScreenAI: cached")
        return
    url = cipd("GetInstanceURL", {"package": package, "instance": {"hashAlgo": "SHA256", "hexDigest": digest}})["signedUrl"]
    data = urllib.request.urlopen(url, timeout=600).read()
    size = len(data)
    if hashlib.sha256(data).hexdigest() != digest:
        sys.exit("the ScreenAI package does not match its SHA-256")
    out.mkdir(parents=True, exist_ok=True)
    archive = out / "screenai.zip"
    archive.write_bytes(data)
    with zipfile.ZipFile(archive) as z:
        z.extractall(out)
    archive.unlink()
    (out / ".digest").write_text(digest)
    print(f"ScreenAI {digest[:8]}: {size} bytes")


if name.startswith("SCREENAI-DIGEST:"):
    print(screenai_latest(name.split(":", 1)[1])[1])
    sys.exit(0)
if name.startswith("SCREENAI:"):
    screenai(name.split(":", 1)[1])
    sys.exit(0)
source = (ROOT / "src-tauri/src/ocr/models.rs").read_text(encoding="utf-8")
block = re.search(r"pub static " + re.escape(name) + r": ModelSet = ModelSet \{(.*?)\n\};", source, re.S)
if not block:
    sys.exit(f"no model set {name} in models.rs")
body = block.group(1)
repo = re.search(r'repo: "([^"]+)"', body).group(1)
release = "Source::GitHubRelease" in body
revision = re.search(r'revision: "([^"]+)"' if release else r'revision: "([0-9a-f]{40})"', body).group(1)
files = re.findall(r'name: "([^"]+)",\s*bytes: ([0-9_]+),\s*sha256: "([0-9a-f]{64})"', body)
if not files:
    sys.exit(f"{name} lists no files")
out.mkdir(parents=True, exist_ok=True)
for file, size, sha in files:
    # Saved under its last part, as the app does (`ModelFile::file`).
    path, size = out / file.rsplit("/", 1)[-1], int(size.replace("_", ""))
    if path.is_file() and path.stat().st_size == size and hashlib.sha256(path.read_bytes()).hexdigest() == sha:
        print(f"{file}: cached")
        continue
    url = (f"https://github.com/{repo}/releases/download/{revision}/{file}" if release
           else f"https://huggingface.co/{repo}/resolve/{revision}/{file}")
    data = urllib.request.urlopen(url, timeout=300).read()
    if len(data) != size or hashlib.sha256(data).hexdigest() != sha:
        sys.exit(f"{file} does not match its pinned size and SHA-256")
    path.write_bytes(data)
    print(f"{file}: {size} bytes")
