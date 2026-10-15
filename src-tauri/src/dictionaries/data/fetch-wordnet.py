"""Fetch only WordNet 3.0's English morphology exception lists and license.

The NLTK WordNet repository redistributes the unchanged Princeton database.
The Git commit and every downloaded SHA256 are retained beside the files.
No application data, accounts or Lookupper resources are accessed.
"""
import hashlib
import json
from pathlib import Path
import urllib.request

ROOT = Path(__file__).resolve().parent
FILES = ["noun.exc", "verb.exc", "adj.exc", "adv.exc", "LICENSE"]


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent": "Lookitan-dictionary-data"})
    with urllib.request.urlopen(request, timeout=30) as response:
        data = response.read(1024 * 1024 + 1)
    if len(data) > 1024 * 1024:
        raise ValueError("Unexpectedly large morphology resource")
    return data


manifest_path = ROOT / "en-wordnet-provenance.json"
if manifest_path.exists():
    commit = json.loads(manifest_path.read_text(encoding="utf-8"))["sourceCommit"]
else:
    commits = json.loads(fetch("https://api.github.com/repos/nltk/wordnet/commits?path=wn/data/wordnet-3.0/verb.exc&per_page=1"))
    commit = commits[0]["sha"]
if len(commit) != 40 or any(c not in "0123456789abcdef" for c in commit):
    raise ValueError("Invalid source commit")
provenance = {
    "project": "Princeton WordNet 3.0",
    "copyright": "Copyright 2006 by Princeton University. All rights reserved.",
    "license": "WordNet-3.0",
    "licenseUrl": "https://wordnet.princeton.edu/license-and-commercial-use",
    "sourceRepository": "https://github.com/nltk/wordnet",
    "sourceCommit": commit,
    "files": [],
}
for filename in FILES:
    url = f"https://raw.githubusercontent.com/nltk/wordnet/{commit}/wn/data/wordnet-3.0/{filename}"
    data = fetch(url)
    target = "LICENSE-WordNet-3.0.txt" if filename == "LICENSE" else f"en-wordnet-{filename}"
    (ROOT / target).write_bytes(data)
    provenance["files"].append({"path": target, "url": url, "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)})
manifest_path.write_text(json.dumps(provenance, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(f"WordNet exceptions: {len(FILES) - 1} files, source commit {commit}")
