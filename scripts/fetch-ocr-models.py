"""Downloads an optional OCR model set (src-tauri/src/ocr/models.rs) into a folder, as the app does
when the user picks its engine: from the pinned Hugging Face revision, each file checked against its
SHA-256. For CI's real-model tests; the installers do not carry these models.

Usage: python scripts/fetch-ocr-models.py <MODEL SET STATIC, e.g. MEIKI> <folder>
"""
import hashlib
import pathlib
import re
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
name, out = sys.argv[1], pathlib.Path(sys.argv[2])
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
