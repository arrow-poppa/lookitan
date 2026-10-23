# Dictionary morphology data

This directory contains openly licensed language data used by Lookitan's
independent candidate generator. No Lookupper language pack, asset or code is
included. Original surface spelling wins before all proposed reductions.

## Yomitan literal transforms

- Copyright (C) 2024–2026 Yomitan Authors.
- License: **GPL-3.0-or-later**, under the project's included GPL license.
- Source: the yomidevs/yomitan repository at commit
  **d3c54c10264673eca0d66b970ef74cd5c8b048e3** (2026-10-22), files
  `ext/js/language/<code>/<name>-transforms.js` for ja, en, es, ar, uk and de.
  Before 2026-10-23, ja and en came from the user's extension 26.6.15.0.
- Canonical upstream location:
  <https://github.com/yomidevs/yomitan/tree/master/ext/js/language>.
- The exact source SHA256 and commit are embedded in each JSON file.
- Rebuild with `node generate-yomitan.mjs <yomitan-checkout>/ext`.
- Rule counts and what is left out:
  - `ja-yomitan.json`: all **928** suffix/whole-word rules, with the condition
    masks resolved from Yomitan's condition hierarchy.
  - `en-yomitan.json`: **98** suffix/prefix rules. One contextual
    (phrasal-verb) rule is left out.
  - `es-yomitan.json`: **518** suffix/whole-word rules, including the 239
    irregular forms (fui → ser/ir). The **27** stem-change regex rules
    (quiero → querer) are left out; Hunspell finds most of those forms.
  - `ar-yomitan.json`: **7157** rules. 6764 of them are Yomitan's
    prefix-and-suffix forms, kept with Yomitan's pattern (which uses lookbehind,
    hence `fancy-regex`), the literal text the pattern starts and ends with,
    and the characters cut and added at each end.
  - `uk-yomitan.json`: **2959** rules. Where Yomitan's `requireStem` asks for
    letters before an ending, the rule keeps its pattern (`.зі$`). **64**
    contextual regex rules are left out.
  - `de-yomitan.json`: only the **66** zu-infinitive rules
    (aufzumachen → aufmachen). Yomitan's German participles are regex rules;
    morphology.rs writes its own ge…t/ge…en rules, using these rules'
    separable prefixes.
- This is not a claim of complete Yomitan language-transform parity.

The generator reads each rule's literal text from the three helper functions it
substitutes. A rule made by a language's own helper (Arabic) is kept only when
its deinflection merely cuts characters at the ends and adds text there. It runs
the deinflection on runs of private-use characters to read off what is cut and
added; other contextual rules are left out.

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
