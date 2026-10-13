# Third-party notices

Lookitan is licensed under the GNU General Public License version 3 (see `LICENSE`). It includes
or adapts the following work.

| Component | Use in Lookitan | License |
| --- | --- | --- |
| [yomitan-byok-ai](https://github.com/arrow-poppa/yomitan-byok-ai) (Yomitan fork) | `src/lib/ai/ai-provider.js` and the AI option set | GPL-3.0-or-later |
| [Yomitan](https://github.com/yomidevs/yomitan) | `src/lib/ai/json.js`, `src/lib/ai/object-utilities.js` | GPL-3.0-or-later |
| [MaruPop](https://github.com/marunine/marupop) | KWin cursor relay approach; `src-tauri/resources/kwin/lookitan-cursor.js` is adapted from its `marupopcursor` script | LGPL-3.0-only (`src-tauri/resources/kwin/LICENSE-LGPL-3.0.txt`) |
| [YomiNinja](https://github.com/matt-m-o/YomiNinja) | Request and response handling of the optional Google Lens engine (`src-tauri/src/ocr/lens.rs`) | GPL-3.0 |
| [Chromium](https://chromium.googlesource.com/chromium/src/+/main/third_party/lens_server_proto/) | Field numbers of the Lens Overlay protocol messages (`src-tauri/src/ocr/lens_proto.rs`) | BSD-3-Clause |
| [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) PP-OCRv6 and PP-OCRv5 models | OCR models, downloaded at build time from huggingface.co/PaddlePaddle | Apache-2.0 |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime) 1.24.4 and DirectML 1.15.4 | Inference runtime, downloaded at build time | MIT |
| [Tauri](https://tauri.app) and its plugins | Application framework | MIT or Apache-2.0 |

Rust and npm dependencies keep their own licenses, listed in `src-tauri/Cargo.lock` and
`package-lock.json`.

Lookitan reproduces the behavior of Lookupper (by Aleksei Vekhov) with independent code. It does not
contain Lookupper's code, user-interface text, assets, branding or server endpoints.
