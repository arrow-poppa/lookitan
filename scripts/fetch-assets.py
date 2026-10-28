#!/usr/bin/env python3
"""Downloads the OCR models and the ONNX Runtime library into src-tauri/resources.

Everything comes from official sources at pinned versions:
  * PaddleOCR ONNX models from huggingface.co/PaddlePaddle (Apache-2.0), pinned by commit; each
    file is checked against the SHA-256 that Hugging Face reports for that commit.
  * PP-OCRv6 Tiny RU (Apache-2.0), an experimental Russian recognizer fine-tuned by metrika-labs
    from PP-OCRv6_tiny_rec: pinned by commit and by its own SHA-256 (MODEL_SHA256). Its file is
    model.onnx, and its rec_keys.txt must match the character list of its inference.yml.
  * ONNX Runtime (MIT): the 1.24.4 DirectML build from NuGet on Windows; on Linux, the native
    library of Microsoft's onnxruntime-webgpu 1.27.0 wheel from PyPI (CPU plus the WebGPU provider,
    with Dawn linked in), checked against the SHA-256 that PyPI reports.
The character list of each recognition model is extracted from its inference.yml.
  * Hunspell dictionaries for dictionary forms: the npm packages of wooorm/dictionaries, checked
    against pinned SHA-256s. de, en, es, fr, pt and ru are the releases Lookupper ships; the other
    recognition languages with a package are Lookitan's own addition.
    Each language's license file is kept beside its .aff/.dic.
  * Arabic: AyaSpell from LibreOffice's dictionaries (pinned commit, SHA-256 per file), and
    ar.words.tsv.gz made from the CAMeL Arabic Frequency Lists (pinned release, SHA-256 per zip).

Usage: python scripts/fetch-assets.py --platform windows|linux
"""

import argparse
import gzip
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
    # Experimental, for Russian only (ocr::RU_TINY). The repository was vladlinv/PP-OCRv6_tiny_rec_ru_onnx.
    "rec_ru_tiny": ("metrika-labs/PP-OCRv6_tiny_rec_cyrillic_onnx", "7b6f4f0f232eb5199e01961d3744a3828a905334"),
}
# Model files not named inference.onnx.
MODEL_FILES = {"rec_ru_tiny": "model.onnx"}
# SHA-256 a model must have whatever its repository reports (models outside PaddlePaddle's).
MODEL_SHA256 = {"rec_ru_tiny": "54feb4032b359561c224747547b1f0c44f9437aff39a98f47924b2c9ebc4002d"}
# Models whose rec_keys.txt (one character a line) must equal their inference.yml's character list.
MODEL_KEYS = {"rec_ru_tiny"}

# language -> (npm package, version, SHA-256 of the package tarball)
HUNSPELL = {
    "de": ("dictionary-de", "3.0.0", "50b5640b33c276a49cc15f426f2d31ee5faf397f4a27e92b1678e77f43f8d14a"),
    "en": ("dictionary-en", "4.0.0", "160cba5e6c0230d43d70b0c929e498566ff2c3c4bcf840318d98ce28e31ee756"),
    "es": ("dictionary-es", "4.0.0", "b46986527b23ff1a578d601db7f1e6dc80cc36b728fdcb6044b5212e12e202d5"),
    "fr": ("dictionary-fr", "3.0.0", "b20ab69249881bc36b1dcdfbfd2df226660a7cecc95f2cf122714008b491c78e"),
    "pt": ("dictionary-pt", "4.0.0", "644a05af5c6d2c16eea1093c7ac4c14b45637170cf5b3dd1c07ae0f13256b54d"),
    "ru": ("dictionary-ru", "3.0.0", "53fe1b0ff870c35b61dbc957116a15590458d145dbf89126ebf85a13adce0416"),
    # The other recognition languages with a wooorm/dictionaries package (Lookitan's own addition).
    "bg": ("dictionary-bg", "2.0.0", "b62c02b9db8cd8b378ced3ec17d3d90fd989369c1918d28ee37e189c23de8865"),
    "cs": ("dictionary-cs", "4.0.0", "119518d93ea373c4976818dc5366e15c28590f85a635f3d1b5a1bc962461e97a"),
    "da": ("dictionary-da", "6.0.0", "4fc4c6a355a9fdd6297a9366f7c236b5443aba6a3f5a4d4fc4c90ae73c63db24"),
    "el": ("dictionary-el", "4.0.0", "e219711ef6dbcbe85eb17001b95b49323c27c773de438125bf29968807527297"),
    "hr": ("dictionary-hr", "2.0.0", "5053f80171ce2e2fd3813b3c183d80bb966afd3d3e6572d4c7812b3ab20720d2"),
    "hu": ("dictionary-hu", "3.0.0", "6a98ce9777d63787fa224728a72e8b4e8f05355d0747eac404e0ebd229ec1a3b"),
    "it": ("dictionary-it", "2.0.0", "d004e1a9b9f41c262590164ba957bd75d376db8b9457e449b5dae60b4d020387"),
    "la": ("dictionary-la", "2.0.0", "18d622abc5086b21bb2b5baebe1545d9e74a829f8bd6d1eef33efbd32fa4c4d7"),
    "nb": ("dictionary-nb", "3.0.0", "48f8f690e286edb6f7e55170aedc5a944d64d4b374cbc10bc454c3ea9e73d6f0"),
    "nl": ("dictionary-nl", "2.0.0", "4e041c4622e51aa4dc1fd3a2cc2816c43808b26edc0c51511d4f716cf022d936"),
    "pl": ("dictionary-pl", "2.0.0", "03a119f53a76960dce47a7b8c801cc33c62aa9de0d9c9137c5fa77cc4f18325d"),
    "ro": ("dictionary-ro", "3.0.0", "8ba62190dd452deecb25357873d4c69d17731a6bd17adfe8577023c2029047e0"),
    "sk": ("dictionary-sk", "2.0.0", "675e9dbbe9aec28c3f625790050389ef9ac27a35671efa5861536cf1074c347b"),
    "sl": ("dictionary-sl", "3.0.0", "669c377e36c21967d9c44a81bd4db6861a0c0320588c3c57e890f77a93abbc2f"),
    "sr": ("dictionary-sr", "4.0.0", "25c506a46b984227bdc1a64bfb2e8afd918fa9f84bfe1b28cc9619cc772ed53b"),
    "sr-latn": ("dictionary-sr-latn", "4.0.0", "faf13302eb82e929e2e6519a33c2be880e53ae0ac11edd1467e73073b20d46e3"),
    "sv": ("dictionary-sv", "4.0.0", "bbecca87cf98c9902d0cd1b0f9b373ac6d1410a78ade851ea453a7b922e7164f"),
    "tr": ("dictionary-tr", "2.0.0", "b6d5a806dccccad937f7373809deef117ca8594a1f95eab4fe5be6c37b5d6ef1"),
    "uk": ("dictionary-uk", "4.0.0", "f499681a55b1b0e6c046a902964c1044b28d519bc243bd69ff21d548cfa95e79"),
}

# Arabic Hunspell: AyaSpell (GPL 2+/LGPL 2.1+/MPL 1.1), as LibreOffice ships it. The copy in
# munzirtaha/ayaspell has a broken .aff (a PFX block runs into the REP table).
AYASPELL_COMMIT = "951bb429a4d846699b02e7adb4fc6fca128c6bae"
AYASPELL_FILES = {
    "ar.aff": "cec30b8621001e49618feb05aec1984c5fcfbf7d2ec309901d5cbf66585217a3",
    "ar.dic": "2a3e5367f61c1583734db9d66734f5603e6be5c2d227cf5c5cd7e4ca586e34fe",
    "COPYING.txt": "73480d918cd78d655508e80aa88c022cbd53634524a0f714b5f2519f2cf19124",
    "AUTHORS.txt": "bade1d2d0d47dff5956c08732cdb9c02432d9b0b8016a56e27f58b8d7e842462",
    "README_ar.txt": "5ead684ae5211ccf615686959e24bd21ef6eebbb6195997529fda08b993f615e",
}
# CAMeL Arabic Frequency Lists (CC BY-SA 4.0): Modern Standard and dialectal Arabic.
CAMEL_RELEASE = "v1.0"
CAMEL_LISTS = {
    "MSA_freq_lists.tsv.zip": "99a6792501af8f1a8ed5209bfada961ae4fc076823e62b16883ef2e4c39b39a4",
    "DA_freq_lists.tsv.zip": "7684ad364cae59f23a917424b96377a188e5db8ced1d1a71bac1f3049ff7e0e5",
}
ARABIC_WORDS = "ar.words.tsv.gz"
ARABIC_MIN_COUNT = 50
# Counts under this are left out of the file: arabic.rs only ranks cuts by the forms above it.
ARABIC_RANKED = 500
CAMEL_NOTICE = """ar.words.tsv.gz is adapted from the CAMeL Arabic Frequency Lists v1.0
(https://github.com/CAMeL-Lab/Camel_Arabic_Frequency_Lists): the Modern Standard Arabic and the
Dialectal Arabic lists, counts added together, keeping forms seen at least 50 times. Licensed under
the Creative Commons Attribution-ShareAlike 4.0 International License
(https://creativecommons.org/licenses/by-sa/4.0/), as the original lists are.

Go Inoue, Bashar Alhafni, Nurpeiis Baimukan, Houda Bouamor, and Nizar Habash. 2021. The Interplay of
Variant, Size, and Task Type in Arabic Pre-trained Language Models. In Proceedings of the Sixth
Arabic Natural Language Processing Workshop, pages 92-104, Kyiv, Ukraine (Online). Association for
Computational Linguistics.
"""

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
        name = MODEL_FILES.get(stem, "inference.onnx")
        onnx = get(f"{HF}/{repo}/resolve/{rev}/{name}")
        want = expected.get(name)
        if want and sha256(onnx) != want:
            sys.exit(f"checksum mismatch for {repo} {name}")
        if stem in MODEL_SHA256 and sha256(onnx) != MODEL_SHA256[stem]:
            sys.exit(f"{repo} {name} is not the pinned model (sha256 {sha256(onnx)})")
        (MODELS / f"{stem}.onnx").write_bytes(onnx)
        if stem.startswith("rec_"):
            cfg = yaml.safe_load(get(f"{HF}/{repo}/resolve/{rev}/inference.yml"))
            if cfg["PostProcess"].get("name") != "CTCLabelDecode":
                sys.exit(f"{repo}: not a CTC recognizer")
            chars = cfg["PostProcess"]["character_dict"]
            chars = ["" if c is None else str(c) for c in chars]
            if stem in MODEL_KEYS:
                # One character a line; this repository has CRLF line ends.
                keys = get(f"{HF}/{repo}/resolve/{rev}/rec_keys.txt").decode("utf-8").replace("\r\n", "\n").split("\n")
                if keys[-1] == "":
                    keys.pop()
                if keys != chars:
                    sys.exit(f"{repo}: rec_keys.txt and inference.yml list different characters")
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


def fetch_arabic() -> None:
    """AyaSpell as LibreOffice ships it, and the word list segment/arabic.rs cuts runs with."""
    HUNSPELL_DIR.mkdir(parents=True, exist_ok=True)
    targets = ["ar.aff", "ar.dic", "ar.license.txt", ARABIC_WORDS, "ar.words.license.txt"]
    if all(have(HUNSPELL_DIR / n) for n in targets):
        print("Arabic: cached")
        return
    files = {}
    for name, want in AYASPELL_FILES.items():
        data = get(f"https://raw.githubusercontent.com/LibreOffice/dictionaries/{AYASPELL_COMMIT}/ar/{name}")
        if sha256(data) != want:
            sys.exit(f"checksum mismatch for LibreOffice dictionaries ar/{name}")
        files[name] = data
    (HUNSPELL_DIR / "ar.aff").write_bytes(files["ar.aff"])
    (HUNSPELL_DIR / "ar.dic").write_bytes(files["ar.dic"])
    (HUNSPELL_DIR / "ar.license.txt").write_bytes(
        b"AyaSpell Arabic dictionary, from LibreOffice dictionaries " + AYASPELL_COMMIT.encode() + b" (ar/).\n\n"
        + files["COPYING.txt"] + b"\n" + files["AUTHORS.txt"] + b"\n" + files["README_ar.txt"])
    # Every form seen at least ARABIC_MIN_COUNT times in the Modern Standard and the dialectal
    # lists together, sorted by code point (UTF-8 byte order, which arabic.rs searches in), with
    # its count when that is at least ARABIC_RANKED. Each
    # list runs from the most frequent form down; it is read while its counts are at least half
    # the minimum, so a form reaching it across both lists is seen on one side at least, and the
    # tens of millions of rare forms are never held.
    counts: dict[str, int] = {}
    for name, want in CAMEL_LISTS.items():
        data = get(f"https://github.com/CAMeL-Lab/Camel_Arabic_Frequency_Lists/releases/download/{CAMEL_RELEASE}/{name}")
        if sha256(data) != want:
            sys.exit(f"checksum mismatch for CAMeL {name}")
        with zipfile.ZipFile(io.BytesIO(data)) as z, z.open(z.namelist()[0]) as f:
            for line in io.TextIOWrapper(f, encoding="utf-8"):
                word, _, count = line.rstrip("\n").partition("\t")
                word, count = word.strip(), count.strip()
                if not count.isdigit():
                    continue
                if int(count) < ARABIC_MIN_COUNT // 2:
                    break
                if word:
                    counts[word] = counts.get(word, 0) + int(count)
        del data
    kept = 0
    with gzip.GzipFile(HUNSPELL_DIR / ARABIC_WORDS, "wb", compresslevel=9, mtime=0) as out:
        for w in sorted(counts):
            if counts[w] >= ARABIC_MIN_COUNT and "\t" not in w and "\n" not in w:
                n = counts[w]
                out.write((f"{w}\t{n}\n" if n >= ARABIC_RANKED else f"{w}\n").encode("utf-8"))
                kept += 1
    (HUNSPELL_DIR / "ar.words.license.txt").write_text(CAMEL_NOTICE, encoding="utf-8")
    print(f"Arabic: AyaSpell {AYASPELL_COMMIT[:7]}, CAMeL {CAMEL_RELEASE} ({kept} forms)")


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
    fetch_arabic()
    if args.platform == "windows":
        fetch_ort_windows()
    else:
        fetch_ort_linux()
    print("assets ready in", RES)


if __name__ == "__main__":
    main()
