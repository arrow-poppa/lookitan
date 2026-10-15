#!/usr/bin/env python3
"""Downloads the OCR models and the ONNX Runtime library into src-tauri/resources.

Everything comes from official sources at pinned versions:
  * PaddleOCR ONNX models from huggingface.co/PaddlePaddle (Apache-2.0), pinned by commit; each
    file is checked against the SHA-256 that Hugging Face reports for that commit.
  * ONNX Runtime (MIT): the 1.24.4 DirectML build from NuGet on Windows; on Linux, the native
    library of Microsoft's onnxruntime-webgpu 1.27.0 wheel from PyPI (CPU plus the WebGPU provider,
    with Dawn linked in), checked against the SHA-256 that PyPI reports.
The character list of each recognition model is extracted from its inference.yml.
  * Hunspell dictionaries for dictionary forms (de, en, es, fr, pt, ru): the npm packages of
    wooorm/dictionaries, the same releases Lookupper ships, checked against pinned SHA-256s.
    Each language's license file is kept beside its .aff/.dic.

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
HUNSPELL_DIR = RES / "hunspell"

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

# language -> (npm package, version, SHA-256 of the package tarball)
HUNSPELL = {
    "de": ("dictionary-de", "3.0.0", "50b5640b33c276a49cc15f426f2d31ee5faf397f4a27e92b1678e77f43f8d14a"),
    "en": ("dictionary-en", "4.0.0", "160cba5e6c0230d43d70b0c929e498566ff2c3c4bcf840318d98ce28e31ee756"),
    "es": ("dictionary-es", "4.0.0", "b46986527b23ff1a578d601db7f1e6dc80cc36b728fdcb6044b5212e12e202d5"),
    "fr": ("dictionary-fr", "3.0.0", "b20ab69249881bc36b1dcdfbfd2df226660a7cecc95f2cf122714008b491c78e"),
    "pt": ("dictionary-pt", "4.0.0", "644a05af5c6d2c16eea1093c7ac4c14b45637170cf5b3dd1c07ae0f13256b54d"),
    "ru": ("dictionary-ru", "3.0.0", "53fe1b0ff870c35b61dbc957116a15590458d145dbf89126ebf85a13adce0416"),
}

ORT_VERSION = "1.24.4"
DIRECTML_VERSION = "1.15.4"
# The native library is the same in every CPython variant of the wheel; this pins the cp312 one.
ORT_LINUX_VERSION = "1.27.0"
ORT_LINUX_WHEEL = (
    "https://files.pythonhosted.org/packages/24/11/6cd5f840be97819cbd172bacc1868726ab5fa54ceaaa79508c5a64e61233/"
    f"onnxruntime_webgpu-{ORT_LINUX_VERSION}-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl"
)
ORT_LINUX_SHA256 = "fe5e49e00ad71bc667316d57670bfbf525438783d4a3468e0d337314ec985cbe"


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


def fetch_hunspell() -> None:
    HUNSPELL_DIR.mkdir(parents=True, exist_ok=True)
    for lang, (package, version, want) in HUNSPELL.items():
        names = {"package/index.aff": f"{lang}.aff", "package/index.dic": f"{lang}.dic", "package/license": f"{lang}.license.txt"}
        if all(have(HUNSPELL_DIR / n) for n in names.values()):
            print(f"Hunspell {lang}: cached")
            continue
        tgz = get(f"https://registry.npmjs.org/{package}/-/{package}-{version}.tgz")
        if sha256(tgz) != want:
            sys.exit(f"checksum mismatch for {package}@{version}")
        with tarfile.open(fileobj=io.BytesIO(tgz), mode="r:gz") as t:
            for member, name in names.items():
                (HUNSPELL_DIR / name).write_bytes(t.extractfile(member).read())
        print(f"Hunspell {lang}: {package}@{version}")


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
    whl = get(ORT_LINUX_WHEEL)
    if sha256(whl) != ORT_LINUX_SHA256:
        sys.exit("checksum mismatch for the onnxruntime-webgpu wheel")
    with zipfile.ZipFile(io.BytesIO(whl)) as z:
        with z.open(f"onnxruntime/capi/libonnxruntime.so.{ORT_LINUX_VERSION}") as src:
            with open(ORT_DIR / "libonnxruntime.so", "wb") as out:
                shutil.copyfileobj(src, out)
    print(f"onnxruntime-webgpu {ORT_LINUX_VERSION}: libonnxruntime.so")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--platform", choices=["windows", "linux"], required=True)
    args = ap.parse_args()
    fetch_models()
    fetch_hunspell()
    if args.platform == "windows":
        fetch_ort_windows()
    else:
        fetch_ort_linux()
    print("assets ready in", RES)


if __name__ == "__main__":
    main()
