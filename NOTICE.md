# Third-party notices

Lookitan is licensed under the GNU General Public License version 3 (see `LICENSE`). It includes
or adapts the following work.

| Component | Use in Lookitan | License |
| --- | --- | --- |
| [yomitan-byok-ai](https://github.com/arrow-poppa/yomitan-byok-ai) (Yomitan fork) | `src/lib/ai/ai-provider.js` and the AI option set | GPL-3.0-or-later |
| [Harbor](https://github.com/arrow-poppa/streaming) (branch `claude/harbor-immersion-controller`) | PlayStation HID report reading (`src-tauri/src/gamepad/playstation.rs`), evdev code table (`gamepad.rs`) and PlayStation button names (`src/lib/gamepad-buttons.js`), adapted | MIT |
| [Yomitan](https://github.com/yomidevs/yomitan) | `src/lib/ai/json.js`, `src/lib/ai/object-utilities.js` | GPL-3.0-or-later |
| Yomitan Authors, © 2024–2026 | Japanese, English, Spanish, Arabic, Ukrainian and German (zu-infinitive) declarative morphology rules, independently traversed in Rust; source commit/hashes and generator in `src-tauri/src/dictionaries/data/` | GPL-3.0-or-later |
| [Princeton WordNet 3.0](https://wordnet.princeton.edu/) | English exception lists; original data, full license and immutable-source hashes in `src-tauri/src/dictionaries/data/` | WordNet 3.0 license (© 2006 Princeton University) |
| [Hunspell](https://github.com/hunspell/hunspell) 1.7, through [hunspell-rs](https://github.com/drahnr/hunspell-rs) | Dictionary forms (stems) of looked-up words, compiled into the app | MPL-1.1, GPL-2.0-or-later or LGPL-2.1-or-later (used under the GPL); bindings Apache-2.0 |
| [wooorm/dictionaries](https://github.com/wooorm/dictionaries) `dictionary-de` 3.0.0, `-en` 4.0.0, `-es` 4.0.0, `-fr` 3.0.0, `-pt` 4.0.0, `-ru` 3.0.0, `-bg` 2.0.0, `-cs` 4.0.0, `-da` 6.0.0, `-el` 4.0.0, `-hr` 2.0.0, `-hu` 3.0.0, `-it` 2.0.0, `-la` 2.0.0, `-nb` 3.0.0, `-nl` 2.0.0, `-pl` 2.0.0, `-ro` 3.0.0, `-sk` 2.0.0, `-sl` 3.0.0, `-sr` 4.0.0, `-sr-latn` 4.0.0, `-sv` 4.0.0, `-tr` 2.0.0, `-uk` 4.0.0 | Hunspell `.aff`/`.dic` files, downloaded unmodified at build time; each license ships beside its files as `<lang>.license.txt` | de: GPL-2.0 or GPL-3.0; en: MIT and BSD; es: GPL-3.0, LGPL-3.0 or MPL-1.1; fr: MPL-2.0; pt: LGPL-3.0 or MPL-2.0; ru: BSD-3-Clause; bg, da, el, hu, ro, sk: GPL-2.0, LGPL-2.1 or MPL-1.1; cs, la, nb: GPL-2.0; hr: LGPL-2.1 or SISSL; it, uk: GPL-3.0; nl: BSD-3-Clause or CC-BY-3.0; pl: GPL-3.0, LGPL-3.0 or MPL-2.0; sl: GPL-3.0 or LGPL-2.1; sr, sr-latn: GPL-2.0, LGPL-2.1, MPL-1.1 or CC-BY-SA-3.0; sv: LGPL-3.0; tr: MIT (licenses as the npm packages declare them) |
| [AyaSpell](https://ayaspell.sourceforge.net/) Arabic dictionary, as [LibreOffice dictionaries](https://github.com/LibreOffice/dictionaries/tree/951bb429a4d846699b02e7adb4fc6fca128c6bae/ar) ship it (commit 951bb42) | Arabic Hunspell `ar.aff`/`ar.dic`, downloaded unmodified at build time: dictionary forms, and which forms are words when a run of Arabic letters is cut (`src-tauri/src/segment/arabic.rs`); license and authors ship as `ar.license.txt` | GPL-2.0-or-later, LGPL-2.1-or-later or MPL-1.1 (used under the GPL) |
| [CAMeL Arabic Frequency Lists](https://github.com/CAMeL-Lab/Camel_Arabic_Frequency_Lists) v1.0 (Inoue, Alhafni, Baimukan, Bouamor and Habash, 2021, *The Interplay of Variant, Size, and Task Type in Arabic Pre-trained Language Models*, WANLP) | `ar.words.tsv.gz`, made at build time from the Modern Standard and Dialectal Arabic lists (counts added, forms seen at least 50 times, counts kept from 500): which forms are words and how to cut a run of Arabic letters; the notice ships as `ar.words.license.txt` | CC-BY-SA-4.0 (the adapted list too) |
| [GCIDE](https://www.gnu.org/software/gcide/) webfont.txt | Character table of the GCIDE reader (`src-tauri/src/dictionaries/cide.rs`) | GPL-3.0-or-later |
| [MaruPop](https://github.com/marunine/marupop) | KWin cursor relay approach; `src-tauri/resources/kwin/lookitan-cursor.js` is adapted from its `marupopcursor` script | LGPL-3.0-only (`src-tauri/resources/kwin/LICENSE-LGPL-3.0.txt`) |
| [YomiNinja](https://github.com/matt-m-o/YomiNinja) | Request and response handling of the optional Google Lens engine (`src-tauri/src/ocr/lens.rs`) | GPL-3.0 |
| [Chromium](https://chromium.googlesource.com/chromium/src/+/main/third_party/lens_server_proto/) | Field numbers of the Lens Overlay protocol messages (`src-tauri/src/ocr/lens_proto.rs`) | BSD-3-Clause |
| [lindera](https://github.com/lindera/lindera) 6.2 | Word analysis of Japanese, Chinese and Korean text (`src-tauri/src/segment/`) | MIT |
| [SudachiDict](https://github.com/WorksApplications/SudachiDict) (core), [jieba](https://github.com/fxsjy/jieba) with [CC-CEDICT](https://cc-cedict.org/), [mecab-ko-dic](https://bitbucket.org/eunjeon/mecab-ko-dic) | lindera's prebuilt dictionaries, downloaded unmodified from its GitHub release on first use, each with its NOTICE.txt | SudachiDict and mecab-ko-dic: Apache-2.0; jieba: MIT; CC-CEDICT: CC BY-SA 4.0 |
| [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) PP-OCRv6 and PP-OCRv5 models | OCR models, downloaded at build time from huggingface.co/PaddlePaddle | Apache-2.0 |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime) 1.24.4 and DirectML 1.15.4 | Inference runtime, downloaded at build time | MIT |
| [Tauri](https://tauri.app) and its plugins | Application framework | MIT or Apache-2.0 |

Rust and npm dependencies keep their own licenses, listed in `src-tauri/Cargo.lock` and
`package-lock.json`.

Lookitan reproduces the behavior of Lookupper (by Aleksei Vekhov) with independent code. It does not
contain Lookupper's code, user-interface text, assets, branding or server endpoints.
