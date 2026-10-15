# Dictionary morphology data

This directory contains openly licensed language data used by Lookitan's
independent candidate generator. No Lookupper language pack, asset or code is
included. Original surface spelling wins before all proposed reductions.

## Yomitan literal transforms

- Copyright (C) 2024–2026 Yomitan Authors.
- License: **GPL-3.0-or-later**, under the project's included GPL license.
- Source: the user-selected Yomitan extension, version **26.6.15.0**, at
  `js/language/ja/japanese-transforms.js` and
  `js/language/en/english-transforms.js`.
- Canonical upstream locations:
  <https://github.com/yomidevs/yomitan/blob/master/ext/js/language/ja/japanese-transforms.js>
  and <https://github.com/yomidevs/yomitan/blob/master/ext/js/language/en/english-transforms.js>.
- The exact source SHA256 and source version are embedded in each JSON file.
  The upstream URLs identify the project; the local source hashes identify the
  precise input used here.
- Rebuild with `node generate-yomitan.mjs <extension-root>`.
- `ja-yomitan.json` contains all **834** literal suffix/whole-word rules and
  the condition masks resolved from Yomitan's condition hierarchy.
- `en-yomitan.json` contains **98** literal suffix/prefix rules. **39**
  contextual regex/phrasal-verb rules are intentionally omitted. This is not a
  claim of complete Yomitan language-transform parity.

The JSON is static data. The Rust runtime does not load or execute the source
JavaScript. Its bounded traversal is independently written, using the condition
contract that the next rule needs a condition shared with the previous rule's
output, except at the unconstrained starting surface.

## English lexical exceptions

The four `en-wordnet-*.exc` files are unchanged **Princeton WordNet 3.0**
morphology exception lists, redistributed by NLTK's WordNet repository.

- Copyright 2006 by Princeton University. All rights reserved.
- The complete permission and disclaimer are in `LICENSE-WordNet-3.0.txt`.
- Source repository, immutable commit, individual download URLs and SHA256
  digests are recorded in `en-wordnet-provenance.json`.
- See <https://wordnet.princeton.edu/license-and-commercial-use>.
- `fetch-wordnet.py` fetches those five public files at the recorded commit.

Exceptions are exact matches, preserve ambiguity and part of speech, and are
tried before heuristic suffix forms. WordNet collocations use underscores; no
automatic word splitting is implied by these files.

## Portuguese and coverage limits

Portuguese uses a small independent set of regular plural spelling reductions
and the `-mente` adjective base. It has no conjugation database and does not
claim to handle irregular Portuguese verbs. The spelling proposals are only
used for Portuguese language tags and require a real installed dictionary hit.
Japanese segmentation and Korean/Chinese language models are separate concerns;
these files do not supply those models. Unknown languages only receive Unicode
and punctuation normalization.

All requests are bounded to 256 candidates, 1024 traversal states, 12 steps,
256 Unicode characters and 1024 UTF-8 bytes per candidate/input.
