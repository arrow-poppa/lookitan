# Lookitan: notes for coding agents

Read this file before you change anything, and the area doc (below, "Area docs") of whatever you touch. When your work changes something an area doc or this file describes, update it in the same commit. Add a dated line at the top of `docs/log.md`.

## What this is

Lookitan is a pop-up dictionary for Windows and Linux. The user points at a word anywhere on screen and presses a shortcut. OCR reads the screen, and a pop-up shows MDict entries and AI cards.

- It reproduces the *behavior* of Lookupper, a Windows-only app.
- The AI layer is "bring your own key" (BYOK). It is ported from `arrow-poppa/yomitan-byok-ai`, branch `feature/byok-ai-explanations`.
- License: GPL-3.0-only. Third-party credits are in `NOTICE.md`.

The user is on **Fedora with KDE Plasma on Wayland**. Linux, KDE Wayland and the `.rpm` package are first-class targets. The UI text is in Portuguese (pt-BR).

## Branches: who owns what

Since 2026-10-16 the repository has these branches:

| Branch | Who writes to it | Rule |
|---|---|---|
| `tests` | Every agent: Claude Code, Claude in the cloud, Codex | All work is committed and pushed here. |
| `main` | Only the user | The user merges `tests` into `main` after testing it. |
| `with-fade` | Snapshot requested by the user | Preserves the overlay fade version at `3dcd0f0`; do not update unless the user asks. |
| `image-ai-backup` | Snapshot requested by the user | The "Imagem AI" generated pictures, as they were when the user asked to take them out of `tests` (`634bd8e`; the `image-prompts` branch it grew in was deleted on 2026-10-21). Do not update unless the user asks. |
| `without-fade` | Only the user, or an agent the user asks | The user has it updated together with `main` (fast-forwarded to `tests` on 2026-10-20 and three times on 2026-10-21). |
| `edge-tts` | Nobody: merged | The Edge neural voices, tested apart from 2026-10-20. On 2026-10-21 the user had it brought into `tests` and `main` (both fast-forwarded to `6508a5a`), so the voices are part of `tests` now. Do not commit here; work on `tests`. |

- **Never commit, push, merge or open a PR into `main`.** That includes Claude in the cloud, which by default creates its own `claude/…` branch: check out `tests` instead.
- **Do not create other branches** unless the user asks.
- **Sharing `tests`:** before working, and again before pushing, run `git fetch` and integrate what the other agents pushed. Merge, or rebase your own unpushed commits on top. Never force-push or reset over someone else's commits.
- **Commit e-mail:** commit only with `184733469+arrow-poppa@users.noreply.github.com` (or Claude's `noreply@anthropic.com`), never a personal address.

## Hard constraints

- **Behavior clone only.** Never copy Lookupper's decompiled code, assets, branding, UI text or servers (`gateway.lookupper.com` and the others).
- **Google Lens key:** never in the source (OneOCR's model key, `onemodel::KEY` in ocr/onemodel.rs, is no service key: it is the fixed string the library opens its own model file with, published in oneocr and owocr; added on 2026-10-28 with the engine the user asked for, and since 2026-10-31 also what decrypts the user's imported `oneocr.onemodel` on Linux). Builds get it from the GitHub Actions secret `LOOKITAN_LENS_API_KEY` (Chromium's public Lens key, as owocr and chrome-lens-ocr send it), read at compile time with `option_env!` in ocr/lens.rs, at the user's request on 2026-10-21. It still ends up inside the binary. A key the user pastes in Settings › OCR is sent instead; a build without the secret (local, a fork's pull request) needs the user's key. Never embed any other key.
- **No local Rust builds.** The user's PC has little RAM. Rust compiles and installers come from GitHub Actions only. Locally, `npm run build` (Vite) and `npm test` are fine.
- **Never put the user's email in commits, requests or files.** Commits use the GitHub noreply address set in the repo's local git config.
- **Installers:** Windows ships the NSIS `.exe` only, with no `.msi`. Linux ships `.deb`, `.rpm` and `.AppImage`.

## Layout

```
index.html, popup.html, overlay.html   three webviews (main window, pop-up, click-other-words overlay)
src/main/      main window: Palavras / Dicionários / Configurações tabs, Anki panel, store.js (settings)
src/popup/     pop-up: AI word card, context translator, Explain, pictures, MDict iframes, Anki and save buttons
src/lib/audio.js   word audio sources (local audio .db, Yomitan's built-in sources, custom URL / URL JSON, system voice); src/main/audio-panel.js is Configurações › Áudio
src-tauri/src/audio.rs  reads local-audio-yomichan databases (entries.db / android.db) read-only with rusqlite
src/lib/images.js  the "Imagens" dictionary: image query prompts, Bing Images search and parsing, picture download for Anki
src/overlay/   word rectangles over a frozen frame; a click calls `overlay_pick`
src/lib/ai/    ai-provider.js (ported from the fork), explain.js, word-card.js (prompts), response-cache.js
src/lib/anki/  AnkiConnect client and note building (ported from Yomitan)
src-tauri/src/
  lib.rs         AppState, run_scan (capture → OCR → pick → overlay and pop-up), Tauri commands, setup
  settings.rs    Settings (camelCase JSON), built-in AI dictionaries, migrate()
  mdict.rs       MDict .mdx/.mdd reader; a .mdx's keys in a memory-mapped `.lkkeys` sidecar; dict.rs coordinates readers/resources
  dictionaries/ format readers read in place with .lkidx headword indexes, archive install/resource paths, morphology
  ocr/paddle.rs  PP-OCRv6 det+rec through ONNX Runtime (DirectML on Windows, WebGPU on Linux, CPU fallback)
  ocr/lens.rs    optional Google Lens cloud OCR (protobuf to lensfrontend-pa.googleapis.com/v1/crupload)
  ocr/cache.rs   per-frame cache of boxes and recognized lines
  segment/       words of Japanese, Chinese and Korean text (Sudachi, jieba-rs, Kiwi; downloaded on first use), and Arabic runs cut in two (arabic.rs); words.rs picks with it
  layout.rs      paragraphs as Lookupper's layout analyzer finds them, with the ink and frames measured on the capture
  platform/      cursor, capture and foreground app; kwin.rs is the KDE Wayland relay and ScreenShot2; memory.rs gives freed memory back on Linux; locale.rs runs code in the "C" locale
                 mouse_hotkey.rs: the mouse shortcut's logic (no I/O, tested everywhere); linux_mouse/: its Linux backend (evdev.rs ioctls, setup.rs the udev rule, mod.rs the thread)
  popup_size.rs  the pop-up's last size (popup-size.json beside settings.json)
  protocol.rs    `lkt:` URI scheme (/e entry HTML, /d dictionary resource, /c capture frame, /m saved media)
  cards.rs       saved words (words.json plus media/)
  gamepad.rs     game controllers (gilrs: XInput on Windows, evdev on Linux): look-up combination and navigation
src-tauri/resources/kwin/lookitan-cursor.js   KWin script; sends the cursor, screen and active window over D-Bus (MaruPop approach, LGPL-3.0)
src-tauri/linux/lookitan.desktop              needs `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2` and `Actions=scan;`
scripts/fetch-assets.py   OCR models (Hugging Face; pinned but Tiny RU, fetched at its latest commit) and ONNX Runtime; checks SHA-256
scripts/*.test.mjs        Node tests (`npm test`); scripts/check-linux-rpm.py checks the RPM .desktop
docs/                     area notes for agents ("Area docs" below) and the log
.github/workflows/build.yml   Windows and Ubuntu 24.04 matrix; runs tests, then tauri-action, then uploads installers
```
## Area docs

Each area's behavior, its reasons and what was measured live in its own file. Read the one for what you are about to touch; update it in the same commit when your work changes what it says.

- [`docs/ocr.md`](docs/ocr.md): OCR engines and their models (PP-OCRv6, IndraOCR, PP-OCRv6 Manga, comic-text-detector, ScreenAI, OneOCR, Lens, Bing), GPU/CPU sessions, memory, model pins and downloads, OCR languages, ink-snapped word edges. Read before changing `src-tauri/src/ocr/`, `scripts/fetch-assets.py`, `scripts/fetch-ocr-models.py`, Configurações › OCR.
- [`docs/text-analysis.md`](docs/text-analysis.md): Words of other scripts and RTL, vertical text, Japanese/Chinese/Korean analyzers, paragraphs (Lookupper's layout analyzer), sentences and the context sent to the AI. Read before changing `words.rs`, `segment/`, `layout.rs`, `sentence.rs`, RTL in `ocr/mod.rs`.
- [`docs/lookup-flow.md`](docs/lookup-flow.md): What a scan ends in (pop-up, block outline, whole-screen overlay), the automatic capture, pop-up placement (WinUI TeachingTip), freeze modes, hiding windows on Linux, overlay animation, pop-up size. Read before changing `run_scan`, `block.rs`, `src/overlay/`, the pop-up window, `popup_position`, `popup_size.rs`.
- [`docs/platform.md`](docs/platform.md): KDE Wayland (KWin relay, ScreenShot2), mouse-button shortcuts on Linux (evdev, uinput, udev rule), tool windows on Windows, game controllers. Read before changing `src-tauri/src/platform/`, `gamepad.rs`, `resources/kwin/`, Configurações › Atalhos.
- [`docs/ai.md`](docs/ai.md): The AI word card's fields and prompts, OCR-error notes, meaning in context, display forms, card hints, longer units, empty prompts, providers per dictionary, prompt caching, the AI cache, the "Imagens" pictures, definition and translation languages. Read before changing `src/lib/ai/`, `src/lib/images.js`, the AI parts of `src/popup/`, the Dicionários tab.
- [`docs/anki-audio-words.md`](docs/anki-audio-words.md): Anki (AnkiConnect, field markers), word audio sources and Edge voices, the Palavras tab, the pop-up's saved/Anki counters, the Dicionários order, settings export and import. Read before changing `src/lib/anki/`, `src/lib/audio.js`, `audio.rs`, `edge_tts.rs`, `src/main/words-tab.js`, `settings-file.js`.
- [`docs/dictionary-engine.md`](docs/dictionary-engine.md): Local dictionaries: storage in place (no database), `.lkidx` indexes, formats, morphology and candidate forms, reader tests on Actions. Read before changing `dictionaries/`, `mdict*.rs`, `dict.rs`, the Dicionários import.
- [`docs/log.md`](docs/log.md): Every change that matters to the next agent, newest first, since 2026-10-13. Before reverting or redoing something: it may have been tried and changed at the user's request.

## Working

- **Tests:**
  - `npm test` runs the frontend contract tests.
  - `cargo test --manifest-path src-tauri/Cargo.toml --lib` runs on CI only.
  - Add a test when you fix a regression.
- **CI:** every push to `tests` or `main` and every PR builds both platforms. To check one run, use `gh run list --limit 1` and `gh run view <id> --log-failed`.
- **Commit messages:** imperative subject, then a body that explains why.

## Log

The log is in [`docs/log.md`](docs/log.md), newest first: add one line at its top per change that matters to the next agent (date, agent, branch, what and why, the tests).
