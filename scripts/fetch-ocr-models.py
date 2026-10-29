"""Downloads an optional OCR model set (src-tauri/src/ocr/models.rs) into a folder, as the app does
when the user picks its engine: from the pinned Hugging Face revision, each file checked against its
SHA-256. For CI's real-model tests; the installers do not carry these models.

Usage: python scripts/fetch-ocr-models.py <MODEL SET STATIC, e.g. MEIKI> <folder>
       python scripts/fetch-ocr-models.py SCREENAI:<windows|linux> <folder>
Chrome ScreenAI's package comes from Chromium's CIPD instead, pinned in src-tauri/src/ocr/screenai.rs.
"""
import hashlib
import pathlib
import re
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
name, out = sys.argv[1], pathlib.Path(sys.argv[2])


def screenai(platform: str) -> None:
    """Chrome ScreenAI's pinned CIPD instance for `platform` (ocr/screenai.rs), unpacked in `out`."""
    import json
    import zipfile
    source = (ROOT / "src-tauri/src/ocr/screenai.rs").read_text(encoding="utf-8")
    cfg = 'windows' if platform == 'windows' else 'target_os = "linux"'
    block = re.search(r"#\[cfg\(" + re.escape(cfg) + r"\)\]\s*static PACKAGE: Option<Package> = Some\(Package \{(.*?)\}\);", source, re.S)
    if not block:
        sys.exit(f"no ScreenAI package for {platform} in screenai.rs")
    body = block.group(1)
    package = re.search(r'name: "([^"]+)"', body).group(1)
    digest = re.search(r'digest: "([0-9a-f]{64})"', body).group(1)
    size = int(re.search(r"bytes: ([0-9_]+)", body).group(1).replace("_", ""))
    if (out / "resources").is_dir() and (out / ".digest").is_file() and (out / ".digest").read_text() == digest:
        print("ScreenAI: cached")
        return
    request = urllib.request.Request(
        "https://chrome-infra-packages.appspot.com/prpc/cipd.Repository/GetInstanceURL",
        data=json.dumps({"package": package, "instance": {"hashAlgo": "SHA256", "hexDigest": digest}}).encode(),
        headers={"content-type": "application/json", "accept": "application/json"})
    answer = urllib.request.urlopen(request, timeout=60).read().decode()
    url = json.loads(answer[answer.index("{"):])["signedUrl"]
    data = urllib.request.urlopen(url, timeout=600).read()
    if len(data) != size or hashlib.sha256(data).hexdigest() != digest:
        sys.exit("the ScreenAI package does not match its pinned size and SHA-256")
    out.mkdir(parents=True, exist_ok=True)
    archive = out / "screenai.zip"
    archive.write_bytes(data)
    with zipfile.ZipFile(archive) as z:
        z.extractall(out)
    archive.unlink()
    (out / ".digest").write_text(digest)
    print(f"ScreenAI: {size} bytes")


if name.startswith("SCREENAI:"):
    screenai(name.split(":", 1)[1])
    sys.exit(0)
source = (ROOT / "src-tauri/src/ocr/models.rs").read_text(encoding="utf-8")
block = re.search(r"pub static " + re.escape(name) + r": ModelSet = ModelSet \{(.*?)\n\};", source, re.S)
if not block:
    sys.exit(f"no model set {name} in models.rs")
body = block.group(1)
repo = re.search(r'repo: "([^"]+)"', body).group(1)
revision = re.search(r'revision: "([0-9a-f]{40})"', body).group(1)
files = re.findall(r'name: "([^"]+)",\s*bytes: ([0-9_]+),\s*sha256: "([0-9a-f]{64})"', body)
if not files:
    sys.exit(f"{name} lists no files")
out.mkdir(parents=True, exist_ok=True)
for file, size, sha in files:
    path, size = out / file, int(size.replace("_", ""))
    if path.is_file() and path.stat().st_size == size and hashlib.sha256(path.read_bytes()).hexdigest() == sha:
        print(f"{file}: cached")
        continue
    url = f"https://huggingface.co/{repo}/resolve/{revision}/{file}"
    data = urllib.request.urlopen(url, timeout=300).read()
    if len(data) != size or hashlib.sha256(data).hexdigest() != sha:
        sys.exit(f"{file} does not match its pinned size and SHA-256")
    path.write_bytes(data)
    print(f"{file}: {size} bytes")
