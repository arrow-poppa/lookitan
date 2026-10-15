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

- Feature branches named `claude/…` are Claude's experiments; they reach `main` only when the user approves.
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
  - GPU card: off by default (`chooseGpu`), DirectML takes adapter 0 and Linux takes Dawn's default (high performance). With "Escolher a placa de vídeo" on, `gpuPreference` (`highPerformance`/`lowPower`) goes to DirectML's performance preference or to the WebGPU EP's `powerPreference`. The WebGPU `deviceId` is ONNX Runtime's context id, not an adapter index. The Linux packages depend on the Vulkan loader (`vulkan-loader`, `libvulkan1`).
  - The models are the same PP-OCRv6/PP-OCRv5 weights as Lookupper, from the official PaddlePaddle exports. Lookupper's copies are a different ONNX conversion (det at opset 11 instead of 14).
  - `DET_MAX_SIDE` is 1280, and whole-screen OCR runs in overlapping bands.
  - Recognition has two phases: the lines near the pointer first, one run per line, then the rest in the background for the overlay.
  - The background whole-screen pass is batched: `plan_batches` groups lines of similar width (at most 16 lines and 12,000 padded columns per run), pads them with zeros like PaddleOCR, and decodes each line only over its own CTC steps. A failed batch retries line by line.
- **Scan result:** the pop-up opens only when the pointer is on a word (`Pick::is_under`, with a pad of a quarter of the line height). Otherwise the scan re-captures the whole monitor and shows the focused overlay of clickable words with no pop-up, as Lookupper does. Also like Lookupper, that whole-screen overlay appears only after the background pass has read the whole screen (`deferred` in `run_scan`), not progressively; Right after the whole-screen capture, `show_overlay_window` raises the overlay transparent (body class `idle`, only the 0.01-alpha shade), so clicks and the scroll wheel cannot move the screen away from the captured frame while OCR runs; the words fade in with the final `emit_overlay`. A click or Esc meanwhile cancels: `overlay_close` bumps `scan_seq`, which stops the background pass and makes `run_scan` return early. `Recognition::scan` keeps the overlay's move/resize handler from redrawing an older lookup while a new scan is under way. An empty click on the overlay calls `overlay_dismiss`: in a whole-screen lookup with the pop-up pinned ("Fixar"), it hides only the pop-up and keeps the words clickable, and the next empty click closes the overlay (Lookupper's behavior, which the user wants only when pinned). Otherwise it closes the overlay and the pop-up at once. Esc or X in the pop-up always closes both (`hide_popup`). The "Área lida" setting is gone: `migrate()` forces `ocrScope` to Region, and the capture-region size is always editable.
- **Freeze:** per app, as in Lookupper: each profile in Configurações › Jogos e programas has `freeze` = `none` ("Sem congelar", the default: the screen stays live and only the words are highlighted), `frame` ("Congelar": the captured frame covers the screen; on whole-screen lookups it is blurred with sharp word cut-outs) or `process` ("Pausar o programa"). There is no global freeze option; the short-lived `freezeOnLookup` key is ignored, and `migrate()` turns saved `default` profiles into `none`.
- **Sentence (branch `claude/overlay-spotlight`):** like Lookupper, the context is the word's paragraph, and `Pick.sentence` is the sentence cut from it.
  - `words::paragraph` grows from the pointer line. It adds lines right above or below in the same column (gap ≤ 1 line height, height ratio ≤ 1.6) and pieces of the same row.
  - `sentence::split` applies Lookupper's rules (see the module header). Text longer than 400 characters without sentence marks falls back to the pointer row ±1.
  - `Pick.sentence_lines` frames the "sentence" screenshot (`focus_lines`).
  - `Pick.context` is the whole paragraph, as Lookupper shows and sends it to its AI. Past 1500 characters it keeps the whole sentences nearest the word's. `context_at` locates the word in it.
  - Who reads what (`aiText` in the popup): the AI word card and Explicar read `context`, and so does their `{{sentence}}` placeholder. The context translator reads only `sentence`. Anki's `{sentence}` and cloze markers follow `anki.contextMode` (Anki › "Contexto no card"): `sentence` (the default) or `paragraph`. With `paragraph`, a lookup without a paragraph falls back to the sentence. Saved words keep `context` too, so exports from the Words tab follow the option.
  - Lookupper's own paragraph thresholds (XpressOCR's layout analyzer) are not decoded; ours are our own.
- **Automatic capture (`captureAuto`, the default, "Automático" in Configurações › OCR › Região de captura):** as in Lookupper's pointer mode.
  - The whole monitor is captured and detected, but only the block of text under the pointer is recognized (`block::under` on the detected boxes, `Reading::Block` in `ocr_pass`). The block is the pointed line plus the lines close around it: vertical gap ≤ 0.6 line height, row pieces ≤ 1.5, height ratio ≤ 1.6, at most 40 boxes. These thresholds are our own.
  - Pointer on a word of the block: the pop-up opens, with no background pass.
  - Pointer on the block but off its words: no pop-up. The overlay shows only that block's words and outlines the block (`Recognition.block`, `#block` in overlay.html), as Lookupper does.
  - Pointer on no line: the whole-screen overlay, on the same capture.
  - Choosing a fixed region turns it off (`captureAuto: false`).
- **Hiding the overlay and the pop-up (Linux):** a hidden WebKitGTK window shows its last painted frame again when it reappears, which flashed the previous lookup over the screen at every OCR. `hide_blank` first sends `overlay-closed`/`popup-blank` with a token, waits (≤150 ms, on its own thread) for the page's `webview_blank` after two animation frames, then hides; `run_scan` joins it before the capture. A per-window counter (`Blanking::order`) keeps a late hide from taking down a newer lookup. On KDE the overlay stays fullscreen between lookups on the same monitor instead of leaving and re-entering fullscreen. Windows hides at once, as before.
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
  - Field markers follow Yomitan: `{glossary}`, `-brief` (no parenthesis), `-no-dictionary` (tags only), `-plain`, `-plain-no-dictionary`, `{glossary-first…}`, `{single-glossary-<kebab name>}` with the same five variants (Yomitan's `getKebabCase`), and `{popup-selection-text}`. The AI card is the dictionary `IA` (tags = part of speech + labels). Definitions follow the user's dictionary order (`dictionaryOrder`). `{ai-explanation}` is rendered Markdown and `{ai-explanation-plain}` is plain text. `{ai-card}` (HTML) and `{ai-card-plain}` export the whole AI card (headword, IPA, CEFR, part of speech, labels, translation, definition, and the explanation if it was generated) without the context, which goes in `{sentence}`. If a field uses `{ai-explanation…}` and Explain was not run, the "+" button generates it first.
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

- 2026-10-15 (Claude, branch `claude/project-thread-x83872`): optional video card choice for OCR (Configurações › OCR › "Escolher a placa de vídeo", off by default) for machines with two cards; dedicated or integrated, on Windows and Linux. The `.rpm` and `.deb` now require the Vulkan loader, without which WebGPU fell back to the CPU silently.

- 2026-10-15 (Claude, branch `claude/project-thread-o09n7y` from `claude/overlay-spotlight`): the screen flashed at every lookup on Linux. The overlay and the pop-up are now hidden only after they paint an empty frame (`hide_blank`, `webview_blank`), and the KDE overlay no longer leaves fullscreen between lookups on the same monitor. Windows unchanged.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): automatic capture, as in Lookupper's pointer mode. It detects the whole monitor and recognizes only the block under the pointer (`block.rs`, `Reading`). Pressed off the block's words, it outlines the block and shows only its words. It is the default (`Settings.capture_auto`), and the capture selector has "Automático". Lookupper findings behind it: the capture is on demand, through a kept-open DXGI duplication that is drained with a 0 ms timeout (not a background loop). The box under the cursor is a rotated-box test with the short side widened 20% per side; with no such box, everything is recognized.
- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): Anki option "Contexto no card ({sentence})" (`anki.contextMode`): just the sentence, as before, or the whole paragraph sent to the AI. It applies to `{sentence}` and `{cloze-*}` in `buildNote`.
- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): the AI word card, Explicar and the pop-up's "Contexto" box use the word's whole paragraph (`Pick.context`, `LookupPayload.context`/`contextAt`), as in Lookupper. The context translator and Anki keep the sentence. `lookup_text` carries the context to lookups typed in the pop-up.
- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): with "Cancelar pedidos pendentes" on, closing the pop-up now aborts its pending AI requests too, like Yomitan (where hiding the pop-up clears it). Before, only a new word did. This covers Esc/X, losing focus while unpinned, overlay dismissal, and a lookup that ends in the overlay without a pop-up. Rust emits `popup-hidden` (`dismiss_popup`/`popup_hidden`); the pop-up calls `responses.cancelPending()`. A new scan's own hide does not emit, so looking up the same word again keeps its request.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): sentence detection like Lookupper. The context was the pointer line ±1; it is now the sentence cut from the word's paragraph (`sentence.rs`, `words::paragraph`/`sentence_of`). The "sentence" screenshot frames that sentence's lines. The rules come from a disassembly of Lookupper's `ParseSentences`; behavior only, no code copied.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): scrolling during a whole-screen lookup's OCR shifted the screen under the words. The overlay is now raised transparent right after the capture and holds the input until the words fade in; a click or Esc meanwhile cancels the lookup.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): the two-step dismissal now applies only when the pop-up is pinned, and the first empty click really hides the pinned pop-up (before, a pinned pop-up stayed up and the focused overlay drew its dim over it). The backend decides in `overlay_dismiss`; the overlay no longer tracks `popupOpen`.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): at the user's request, Esc or X in the pop-up closes the whole-screen overlay too; only an empty click on the overlay keeps it (entry below).

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): whole-screen overlay dismissal like Lookupper: the first empty click closes the word's pop-up and keeps the words; the second closes the overlay. Esc/X in the pop-up keep the overlay too (`hide_popup` checks `Recognition::spotlight`).

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): a whole-screen lookup (`spotlight`) fades the overlay in (body opacity 0→1, 220 ms ease-out, Web Animations) so the screen darkens smoothly. `close_overlay` emits `overlay-closed`, which sets the body's opacity to 0, so a reopened overlay never flashes the previous lookup.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): removed the global "Congelar a tela na consulta" option (and the "Padrão" profile mode) at the user's request. Freezing is chosen per profile again, like Lookupper's per-app freeze menu, with "Sem congelar" as the default. `FreezeMode::Default` stays only so settings saved with it still load; `migrate()` maps it to `None`.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): the user saw the screenshot cover the screen with "Congelar a tela na consulta" off, most likely from a profile left on "Congelar o quadro". The frame is now controlled only by the global option; profiles lose the "frame" choice, and saved `frame` profiles become `default`.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): reverted 9db00bf (overlay dimmed at once, words when done) at the user's request; the whole-screen overlay again appears only after the whole OCR is done, as in Lookupper (the a847e53 behavior below).

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): global "Congelar a tela na consulta" on/off (`freezeOnLookup`), with a "Padrão" profile freeze mode (`FreezeMode::Default`) that follows it. Profiles saved as "Sem congelar" before this were the auto-created default, so `migrate()` moves them to "Padrão" once.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): the whole-screen overlay without a pop-up waits for the whole OCR, like Lookupper, instead of filling in word by word. The background pass shows it at the end; a new scan bumps `AppState::scan_seq`, which stops the old pass and keeps its overlay hidden. With a pop-up, the overlay still appears at once and fills in.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): `{ai-card}` and `{ai-card-plain}` Anki markers export the whole AI card without its context (the context is the note's `{sentence}`). They do not generate the explanation; it is included only if Explain already ran.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): whole-screen lookups on a live screen ("Sem congelar") are dimmed slightly too, like Lookupper. A `#dim` canvas fills black at 0.15 alpha, the same darkness as the frozen frame's brightness(0.85), with holes cleared over the words. A live screen cannot be blurred from the webview, so blur stays frozen-frame only.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): the frozen, blurred screen of whole-screen lookups now depends on the app profile, as in Lookupper's "No freeze" menu. With "Sem congelar" (the default) the screen stays live under the yellow highlights; with "Congelar o quadro" it is frozen, and on whole-screen lookups also blurred and dimmed with sharp cut-outs.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): whole-screen lookups (`spotlight`) show the frame lightly blurred again (CSS blur 2.5 px, brightness 0.85: a slight dim the user asked for), with words as sharp cut-outs and the yellow highlights drawn on top via `::after`. Lookupper does this with OpenCV `imgproc_GaussianBlur` (OpenCvSharp) and `CroppedImage`; its exact kernel and sigma are not decompiled yet. The earlier 7 px blur with dimming looked wrong to the user.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): popup open/close animation like Lookupper's `WordTeachingTip`, a WinUI TeachingTip. It uses WinUI's values: scale from 0.01 to 1 in 300 ms with cubic-bezier(0.1, 0.9, 0.2, 1) from the edge facing the word, sent by `show_popup` as the `popup-open` event. Close: 200 ms with cubic-bezier(0.7, 0, 1, 0.5). The backend still hides the popup at once before a new scan.

- 2026-10-14 (Claude, branch `claude/overlay-spotlight`): Lookupper-style overlay, matched to the user's screenshots. The screen is not blurred or dimmed and word boxes are invisible. Hover and words already looked up in this overlay (`seen`) get a faint yellow (0.26); the word being looked up gets a strong yellow (0.5). A whole-screen lookup without a pop-up (`spotlight`) also hides the hint pill. A first blurred/cut-out attempt was rejected.
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
