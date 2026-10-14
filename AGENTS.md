# Lookitan: notes for coding agents

Read this file before you change anything. When your work changes something described here, update the file in the same commit. Add a dated line to the log at the bottom.

## What this is

Lookitan is a pop-up dictionary for Windows and Linux. The user points at a word anywhere on screen and presses a shortcut. OCR reads the screen, and a pop-up shows MDict entries and AI cards.

- It reproduces the *behavior* of Lookupper, a Windows-only app.
- The AI layer is "bring your own key" (BYOK). It is ported from `arrow-poppa/yomitan-byok-ai`, branch `feature/byok-ai-explanations`.
- License: GPL-3.0-only. Third-party credits are in `NOTICE.md`.

The user is on **Fedora with KDE Plasma on Wayland**. Linux, KDE Wayland and the `.rpm` package are first-class targets. The UI text is in Portuguese (pt-BR).

## Branches: who owns what

| Branch | Owner | Rule |
|---|---|---|
| `main` | Claude (Claude Code) | Claude commits here. |
| `codex/restore-stable-lookup` | Codex | Codex commits here. |

- Do not commit to the other agent's branch.
- To take the other agent's work, merge its branch into yours. Fast-forward when possible. Do not rebase or force-push a shared branch.
- On 2026-10-13, `main` was fast-forwarded to the codex tip `9ce4339`, so both branches started from the same commit.

## Hard constraints

- **Behavior clone only.** Never copy Lookupper's decompiled code, assets, branding, UI text or servers (`gateway.lookupper.com` and the others).
- **Never embed a Google Lens API key.** The user pastes their own in Settings › OCR.
- **No local Rust builds.** The user's PC has little RAM. Rust compiles and installers come from GitHub Actions only. Locally, `npm run build` (Vite) and `npm test` are fine.
- **Never put the user's email in commits, requests or files.** Commits use the GitHub noreply address set in the repo's local git config.
- **Installers:** Windows ships the NSIS `.exe` only, with no `.msi`. Linux ships `.deb`, `.rpm` and `.AppImage`.

## Layout

```
index.html, popup.html, overlay.html   three webviews (main window, pop-up, click-other-words overlay)
src/main/      main window: Palavras / Dicionários / Configurações tabs, Anki panel, store.js (settings)
src/popup/     pop-up: AI word card, context translator, Explain, MDict iframes, Anki and save buttons
src/overlay/   word rectangles over a frozen frame; a click calls `overlay_pick`
src/lib/ai/    ai-provider.js (ported from the fork), explain.js, word-card.js (prompts), response-cache.js
src/lib/anki/  AnkiConnect client and note building (ported from Yomitan)
src-tauri/src/
  lib.rs         AppState, run_scan (capture → OCR → pick → overlay and pop-up), Tauri commands, setup
  settings.rs    Settings (camelCase JSON), built-in AI dictionaries, migrate()
  mdict.rs       MDict .mdx/.mdd reader; dict.rs does lookup and inflection candidates
  ocr/paddle.rs  PP-OCRv6 det+rec through ONNX Runtime (DirectML on Windows, WebGPU on Linux, CPU fallback)
  ocr/lens.rs    optional Google Lens cloud OCR (protobuf to lensfrontend-pa.googleapis.com/v1/crupload)
  ocr/cache.rs   per-frame cache of boxes and recognized lines
  platform/      cursor, capture and foreground app; kwin.rs is the KDE Wayland relay and ScreenShot2
  protocol.rs    `lkt:` URI scheme (/e entry HTML, /d dictionary resource, /c capture frame, /m saved media)
  cards.rs       saved words (words.json plus media/)
src-tauri/resources/kwin/lookitan-cursor.js   KWin script; sends the cursor, screen and active window over D-Bus (MaruPop approach, LGPL-3.0)
src-tauri/linux/lookitan.desktop              needs `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2` and `Actions=scan;`
scripts/fetch-assets.py   pinned OCR models (Hugging Face) and ONNX Runtime; checks SHA-256
scripts/*.test.mjs        Node tests (`npm test`); scripts/check-linux-rpm.py checks the RPM .desktop
.github/workflows/build.yml   Windows and Ubuntu 24.04 matrix; runs tests, then tauri-action, then uploads installers
```

## Key behavior

- **OCR:**
  - The detector tries det_small on the GPU first. If GPU inference fails, it releases the GPU session and retries on the CPU with det_tiny. Foreground and overlay OCR share that CPU session.
  - GPU first on both platforms (DirectML on Windows, WebGPU/Vulkan on Linux); the user runs Lookitan on Windows and on Fedora KDE. After a GPU failure the stage stays on the CPU until restart. A "retry the GPU on the next input" variant was tried in 5ab8870 and reverted at the user's request as less safe; do not reintroduce it without asking.
  - The models are the same PP-OCRv6/PP-OCRv5 weights as Lookupper, from the official PaddlePaddle exports. Lookupper's copies are a different ONNX conversion (det at opset 11 instead of 14).
  - `DET_MAX_SIDE` is 1280, and whole-screen OCR runs in overlapping bands.
  - Recognition has two phases: the lines near the pointer first, one run per line, then the rest in the background for the overlay.
  - The background whole-screen pass is batched: `plan_batches` groups lines of similar width (at most 16 lines and 12,000 padded columns per run), pads them with zeros like PaddleOCR, and decodes each line only over its own CTC steps. A failed batch retries line by line.
- **Scan result:** the pop-up opens only when the pointer is on a word (`Pick::is_under`, with a pad of a quarter of the line height). Otherwise the scan re-captures the whole monitor and shows the focused overlay of clickable words with no pop-up, as Lookupper does. The "Área lida" setting is gone: `migrate()` forces `ocrScope` to Region, and the capture-region size is always editable.
- **OCR engines:** `ocrEngine` reads around the pointer; `screenOcrEngine` reads whole-screen passes (`null` means "same as `ocrEngine`"). `Settings::engine_for(whole_screen)` picks one.
- **Google Lens:** chosen in Configurações › OCR › Motor de OCR, and it needs the user's key. If Lens fails, the scan falls back to PP-OCRv6. Each lookup uploads the capture to Google.
- **KDE Wayland:**
  - The cursor comes from the KWin script over D-Bus (`io.github.arrowpoppa.Lookitan` `/Cursor`).
  - Capture uses `org.kde.KWin.ScreenShot2.CaptureArea`, which is authorized only when Lookitan runs from the installed `.desktop` entry.
  - The windows run under XWayland.
  - On Wayland, the global shortcut is a system shortcut that runs `lookitan --scan`.
- **AI cache:** same semantics as the fork. It has a 60 s TTL, joins in-flight requests and never caches partial or failed answers. `cancelPendingExcept` drops requests the pop-up no longer shows.
- **Anki:** uses Yomitan's `anki` options shape and field markers, through AnkiConnect.
  - tauri-plugin-http always sends `Origin: http://tauri.localhost` (Windows) or `tauri://localhost` (Linux). AnkiConnect allows only `http://localhost`, 127.0.0.1 and browser-extension origins by default (that is why Yomitan works) and answers 403 to anything else.
  - Field markers follow Yomitan: `{glossary}`, `-brief` (no parenthesis), `-no-dictionary` (tags only), `-plain`, `-plain-no-dictionary`, `{glossary-first…}`, `{single-glossary-<kebab name>}` with the same five variants (Yomitan's `getKebabCase`), and `{popup-selection-text}`. The AI card is the dictionary `IA` (tags = part of speech + labels). Definitions follow the user's dictionary order (`dictionaryOrder`). `{ai-explanation}` is rendered Markdown and `{ai-explanation-plain}` is plain text. If a field uses `{ai-explanation…}` and Explain was not run, the "+" button generates it first.
  - MDict entries are cross-origin iframes, so `ENTRY_SCRIPT` posts `{type: 'selection'}` messages and the pop-up keeps the latest non-empty selection.
  - On a 403, `AnkiConnect._invoke` calls `requestPermission` once, with a 120 s timeout because Anki shows a Yes/No prompt, then retries. On "Yes", AnkiConnect adds the origin to `webCorsOriginList` for good.

## Working

- **Tests:**
  - `npm test` runs the frontend contract tests.
  - `cargo test --manifest-path src-tauri/Cargo.toml --lib` runs on CI only.
  - Add a test when you fix a regression.
- **CI:** every push to `main` and every PR builds both platforms. To check one run, use `gh run list --limit 1` and `gh run view <id> --log-failed`.
- **Commit messages:** imperative subject, then a body that explains why.

## Log

Newest first. One line per change that matters to the next agent.

- 2026-10-14 (Claude): Anki markers `{ai-card}` and `{ai-card-plain}` export the whole AI word card (headword, IPA, CEFR, part of speech, translation, definition, and the explanation when generated) without the context, which stays in `{sentence}`.

- 2026-10-14 (Claude): Anki's "A tela inteira" screenshot was only the region around the pointer once the scope became Region. `run_scan` now also captures the whole monitor (`Recognition.screen`) when Anki is on with `screenshotArea: whole`. `capture_png` areas are "sentence", "frame" (the image OCR read, used by the overlay route) and "whole". When merging `claude/overlay-spotlight`, its fast-PNG branch must apply to "frame".

- 2026-10-14 (Claude): the Anki section and the game/program profiles card collapse with a chevron, like Lookupper (`collapser()` in ui.js; the state is kept in localStorage per window).

- 2026-10-14 (Claude): Yomitan's per-dictionary and popup-selection Anki markers, plus `{ai-explanation-plain}`; Markdown helpers moved to `src/lib/markdown.js`.

- 2026-10-14 (Claude): fixed Anki always failing with 403: Lookitan now asks AnkiConnect for permission (`requestPermission`) and retries.

- 2026-10-14 (Claude): `showWordTranslation` (default on) turns the word translation on the AI card on or off. The card prompt has a `{{translationField}}` marker; `cardPrompt()` fills it with `, "translation": <wordTranslationPrompt>` when the option is on and with nothing when it is off, so the model spends no tokens on it. `normalizeAiOptions` migrates older saved card prompts by replacing the old default translation clause with the marker.

- 2026-10-14 (Claude): reverted 5ab8870 (GPU retry after a failure); the original fallback is back, as the user prefers.

- 2026-10-14 (Claude): removed the "Área lida" (scope) setting at the user's request; the scope is always Region.

- 2026-10-14 (Claude): batched recognition for the whole-screen background pass only (`recognize_boxes_batched`); the pointer lines are unchanged.

- 2026-10-14 (Claude): separate OCR engines for the region around the pointer and for whole-screen passes (`screenOcrEngine`).

- 2026-10-13 (Claude): a shortcut pressed away from text now opens the whole-screen word overlay without a pop-up (`ocr_pass` and `Pick::is_under` in lib.rs/words.rs).

- 2026-10-13 (Claude): fast-forwarded `main` to the codex tip `9ce4339`. That tip contains:
  - Codex's work: restored the stable region lookup, fixed the WebView startup race, added GPU→CPU retry on inference failure, fixed KDE capture and DPI, added bounded AI/OCR caches and automatic Explain, and added the test suite.
  - The Claude cloud session's Linux WebGPU provider.
  - It also added AGENTS.md.
- 2026-10-13 (Claude): `5187d23`: AI response cache like yomitan-byok-ai; CI asset cache.
- 2026-10-13 (Claude): `cdd01c2`: Lookupper-style UI, clickable overlay, mouse shortcuts, app profiles and Anki.
