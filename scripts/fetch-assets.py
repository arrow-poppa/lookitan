#!/usr/bin/env python3
"""Downloads the OCR models and the ONNX Runtime library into src-tauri/resources.

Everything comes from official sources at pinned versions:
  * PaddleOCR ONNX models from huggingface.co/PaddlePaddle (Apache-2.0), pinned by commit; each
    file is checked against the SHA-256 that Hugging Face reports for that commit.
  * ONNX Runtime 1.24.4 (MIT): the DirectML build from NuGet on Windows, the official release
    archive on Linux (checked against the GitHub release digest).
The character list of each recognition model is extracted from its inference.yml.

Usage: python scripts/fetch-assets.py --platform windows|linux
"""

import argparse
import hashlib
import io
import json
import shutil
import sys
import tarfile
import time
import urllib.error
import urllib.request
import zipfile
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
RES = ROOT / "src-tauri" / "resources"
MODELS = RES / "models"
ORT_DIR = RES / "ort"

HF = "https://huggingface.co"

# target stem -> (repository, commit)
MODEL_REPOS = {
    "det_small": ("PaddlePaddle/PP-OCRv6_small_det_onnx", "28fe5895c24fd108c19eb3e8479f4ab385fbfc62"),
    "det_tiny": ("PaddlePaddle/PP-OCRv6_tiny_det_onnx", "2ba1506c0380b8f0b03dd142459aac66d4421f6c"),
    "rec_small": ("PaddlePaddle/PP-OCRv6_small_rec_onnx", "b8f84f0b80c529de40b4fbb3544b84fa7233a513"),
    "rec_tiny": ("PaddlePaddle/PP-OCRv6_tiny_rec_onnx", "2612ab37152ae0a677521bae4e1e3d4fb4cf7c30"),
    "rec_cyrillic": ("PaddlePaddle/cyrillic_PP-OCRv5_mobile_rec_onnx", "2cef88145434beb8afa9dd82d77d799eb1ad7b29"),
    "rec_arabic": ("PaddlePaddle/arabic_PP-OCRv5_mobile_rec_onnx", "14aaedcd75825982689ecf5cd64ab33ee083215a"),
    "rec_el": ("PaddlePaddle/el_PP-OCRv5_mobile_rec_onnx", "8152b89d2ee0e1d4c92decab75fe75e5e3d836b6"),
    "rec_korean": ("PaddlePaddle/korean_PP-OCRv5_mobile_rec_onnx", "5c6f574b8e2230adf4287b33e736d71b9fabd28e"),
}

ORT_VERSION = "1.24.4"
DIRECTML_VERSION = "1.15.4"
ORT_LINUX_SHA256 = "3a211fbea252c1e66290658f1b735b772056149f28321e71c308942cdb54b747"


def get(url: str) -> bytes:
    """Downloads with retries; Hugging Face answers 429 when many builds fetch at once."""
    delay = 5
    for attempt in range(7):
        req = urllib.request.Request(url, headers={"User-Agent": "lookitan-build"})
        try:
            with urllib.request.urlopen(req, timeout=300) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code not in (429, 500, 502, 503, 504) or attempt == 6:
                raise
            wait = int(e.headers.get("Retry-After") or delay)
            print(f"HTTP {e.code} for {url}; retrying in {wait}s")
            time.sleep(wait)
            delay = min(delay * 2, 120)
        except urllib.error.URLError:
            if attempt == 6:
                raise
            time.sleep(delay)
            delay = min(delay * 2, 120)
    raise RuntimeError("unreachable")


def have(path: Path) -> bool:
    return path.exists() and path.stat().st_size > 0


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def fetch_models() -> None:
    MODELS.mkdir(parents=True, exist_ok=True)
    for stem, (repo, rev) in MODEL_REPOS.items():
        if have(MODELS / f"{stem}.onnx") and (stem.startswith("det_") or have(MODELS / f"{stem}.dict.json")):
            print(f"{stem}: cached")
            continue
        info = json.loads(get(f"{HF}/api/models/{repo}/revision/{rev}?blobs=true"))
        expected = {s["rfilename"]: (s.get("lfs") or {}).get("sha256") for s in info.get("siblings", [])}
        onnx = get(f"{HF}/{repo}/resolve/{rev}/inference.onnx")
        want = expected.get("inference.onnx")
        if want and sha256(onnx) != want:
            sys.exit(f"checksum mismatch for {repo} inference.onnx")
        (MODELS / f"{stem}.onnx").write_bytes(onnx)
        if stem.startswith("rec_"):
            cfg = yaml.safe_load(get(f"{HF}/{repo}/resolve/{rev}/inference.yml"))
            chars = cfg["PostProcess"]["character_dict"]
            chars = ["" if c is None else str(c) for c in chars]
            (MODELS / f"{stem}.dict.json").write_text(json.dumps(chars, ensure_ascii=False), encoding="utf-8")
            print(f"{stem}: {len(onnx) / 1e6:.1f} MB, {len(chars)} characters")
        else:
            print(f"{stem}: {len(onnx) / 1e6:.1f} MB")


def fetch_ort_windows() -> None:
    ORT_DIR.mkdir(parents=True, exist_ok=True)
    if have(ORT_DIR / "onnxruntime.dll") and have(ORT_DIR / "DirectML.dll"):
        print("ONNX Runtime: cached")
        return
    pkg = get(
        "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/"
        f"{ORT_VERSION}/microsoft.ml.onnxruntime.directml.{ORT_VERSION}.nupkg"
    )
    print(f"Microsoft.ML.OnnxRuntime.DirectML {ORT_VERSION}: sha256 {sha256(pkg)}")
    with zipfile.ZipFile(io.BytesIO(pkg)) as z:
        (ORT_DIR / "onnxruntime.dll").write_bytes(z.read("runtimes/win-x64/native/onnxruntime.dll"))
    dml = get(
        "https://api.nuget.org/v3-flatcontainer/microsoft.ai.directml/"
        f"{DIRECTML_VERSION}/microsoft.ai.directml.{DIRECTML_VERSION}.nupkg"
    )
    print(f"Microsoft.AI.DirectML {DIRECTML_VERSION}: sha256 {sha256(dml)}")
    with zipfile.ZipFile(io.BytesIO(dml)) as z:
        (ORT_DIR / "DirectML.dll").write_bytes(z.read("bin/x64-win/DirectML.dll"))


def fetch_ort_linux() -> None:
    ORT_DIR.mkdir(parents=True, exist_ok=True)
    if have(ORT_DIR / "libonnxruntime.so"):
        print("ONNX Runtime: cached")
        return
    name = f"onnxruntime-linux-x64-{ORT_VERSION}"
    tgz = get(f"https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/{name}.tgz")
    if sha256(tgz) != ORT_LINUX_SHA256:
        sys.exit("checksum mismatch for the ONNX Runtime Linux archive")
    with tarfile.open(fileobj=io.BytesIO(tgz), mode="r:gz") as t:
        member = t.getmember(f"{name}/lib/libonnxruntime.so.{ORT_VERSION}")
        src = t.extractfile(member)
        with open(ORT_DIR / "libonnxruntime.so", "wb") as out:
            shutil.copyfileobj(src, out)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--platform", choices=["windows", "linux"], required=True)
    args = ap.parse_args()
    fetch_models()
    if args.platform == "windows":
        fetch_ort_windows()
    else:
        fetch_ort_linux()
    print("assets ready in", RES)


if __name__ == "__main__":
    main()
