# Lookitan: notes for coding agents

Read this file before you change anything. When your work changes something described here, update the file in the same commit. Add a dated line to the log at the bottom.

## What this is

Lookitan is a pop-up dictionary for Windows and Linux. The user points at a word anywhere on screen and presses a shortcut. OCR reads the screen, and a pop-up shows MDict entries and AI cards.

- It reproduces the *behavior* of Lookupper, a Windows-only app.
- The AI layer is "bring your own key" (BYOK). It is ported from `arrow-poppa/yomitan-byok-ai`, branch `feature/byok-ai-explanations`.
- License: GPL-3.0-only. Third-party credits are in `NOTICE.md`.

The user is on **Fedora with KDE Plasma on Wayland**. Linux, KDE Wayland and the `.rpm` package are first-class targets. The UI text is in Portuguese (pt-BR).

## Branches: who owns what

Since 2026-10-15 the repository has only two branches:

| Branch | Who writes to it | Rule |
|---|---|---|
| `tests` | Every agent: Claude Code, Claude in the cloud, Codex | All work is committed and pushed here. |
| `main` | Only the user | The user merges `tests` into `main` after testing it. |

- **Never commit, push, merge or open a PR into `main`.** That includes Claude in the cloud, which by default creates its own `claude/…` branch: check out `tests` instead.
- **Do not create other branches** unless the user asks.
- **Sharing `tests`:** before working, and again before pushing, run `git fetch` and integrate what the other agents pushed. Merge, or rebase your own unpushed commits on top. Never force-push or reset over someone else's commits.
- **Commit e-mail:** commit only with `184733469+arrow-poppa@users.noreply.github.com` (or Claude's `noreply@anthropic.com`), never a personal address.

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
  mdict.rs       MDict .mdx/.mdd reader; mdict_index.rs persists keys; dict.rs coordinates readers/resources
  dictionaries/ format readers read in place with .lkidx headword indexes, archive install/resource paths, morphology
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

- **Local dictionaries (0.2):** see `docs/dictionary-engine.md` for formats, evidence and limits.
  The settings wire tag `kind: mdict` is retained for all local readers to keep existing settings
  compatible; `DictInfo.format` identifies the actual format. Do not use this tag to infer file format.
  **Storage follows Lookupper's model (user's decision, 2026-10-15): no database.** Importing a ZIP
  unpacks it into `<local data>/dictionaries/content/<id>/` (`source::install`; ZIP, RAR, 7z, TAR or gz/bz2/xz, `dictionaries/archive.rs`), and the files are
  read from there unchanged; removing the dictionary deletes that copy. A main file chosen directly
  is read in place. MDict reads its own key index on every open (no sidecar). Every other format
  keeps a headword index beside its files, `<main>.lkidx` (`dictionaries/lkidx.rs`: Lookupper's
  HeadwordIndex layout plus each entry's offset/length in the source; in the app cache for a file
  used in place), and renders the entry from the source bytes at lookup time: Yomitan
  (`yomitan.lkidx`, `dictionaries/yomitan.rs`) and the rest in `dictionaries/inplace.rs`, which
  re-runs the format's reader over the entry's span. `.dz` is read by chunk (`dictzip.rs`); plain
  gzip/bzip2 lexicon files are unpacked once into the cache. There is no SQLite any more. Lookupper's formats: mdict, yomitan, stardict, slob, dsl, cc-cedict, gcide,
  webster-text, wordnet-lmf-xml, jmdict-simplified-json, kanjidic2-json, kaikki-jsonl; Codex's
  other readers stay as extras. All
  source/AI sections follow one configured popup order. Anki retains all homographs per source.
  MDict block caches are bounded by bytes; do not restore a count-only cache. Imports/loads are
  serialized in AppState, with atomic index publication and safe ZIP extraction. Cleanup at start
  deletes only `.unpacking-*` folders and copies marked `.lookitan-removed`, never unmarked ones.
  Morphology uses the OCR language. Order of forms: on-screen spelling, then Hunspell stems
  (de/en/es/fr/pt/ru, `dictionaries/hunspell.rs`; files in `resources/hunspell/` from
  `fetch-assets.py`, one language loaded at a time), then Yomitan JA/EN rules + WordNet EN
  exceptions and conservative PT rules. Original Lookupper language packs/algorithms are not copied.
  Run reader tests on GitHub Actions (`dictionary-engine` / `scripts/dictionary-harness`), never
  compile Rust locally. Dependency lockfile resolution without compiling is allowed.

- **OCR:**
  - The detector tries det_small on the GPU first. If GPU inference fails, it releases the GPU session and retries on the CPU with det_tiny. Foreground and overlay OCR share that CPU session.
  - GPU first on both platforms (DirectML on Windows, WebGPU/Vulkan on Linux); the user runs Lookitan on Windows and on Fedora KDE. After a GPU failure the stage stays on the CPU until restart. A "retry the GPU on the next input" variant was tried in 5ab8870 and reverted at the user's request as less safe; do not reintroduce it without asking.
  - GPU card: off by default (`chooseGpu`), DirectML takes adapter 0 and Linux takes Dawn's default (high performance). With "Escolher a placa de vídeo" on, `gpuPreference` (`highPerformance`/`lowPower`) goes to DirectML's performance preference or to the WebGPU EP's `powerPreference`. The WebGPU `deviceId` is ONNX Runtime's context id, not an adapter index. The Linux packages depend on the Vulkan loader (`vulkan-loader`, `libvulkan1`).
  - The models are the same PP-OCRv6/PP-OCRv5 weights as Lookupper, from the official PaddlePaddle exports. Lookupper's copies are a different ONNX conversion (det at opset 11 instead of 14).
  - `DET_MAX_SIDE` is 1280, and whole-screen OCR runs in overlapping bands.
  - Recognition has two phases: the lines near the pointer first, one run per line, then the rest in the background for the overlay.
  - The background whole-screen pass is batched: `plan_batches` groups lines of similar width (at most 16 lines and 12,000 padded columns per run), pads them with zeros like PaddleOCR, and decodes each line only over its own CTC steps. A failed batch retries line by line.
- **Scan result:** the pop-up opens only when the pointer is on a word (`Pick::is_under`): inside the word's drawn box, give or take `UNDER_SLACK` (10% of the box height, at least `UNDER_SLACK_MIN` = 3 capture pixels). The space between words or lines is off any word. Otherwise the automatic capture outlines the block of text (see block.rs), and otherwise the scan re-captures the whole monitor and shows the focused overlay of clickable words with no pop-up, as Lookupper does. Also like Lookupper, that whole-screen overlay appears only after the background pass has read the whole screen (`deferred` in `run_scan`), not progressively; Right after the whole-screen capture, `show_overlay_window` raises the overlay transparent (body class `idle`, only the 0.01-alpha shade), so clicks and the scroll wheel cannot move the screen away from the captured frame while OCR runs; the words fade in with the final `emit_overlay`. A click or Esc meanwhile cancels: `overlay_close` bumps `scan_seq`, which stops the background pass and makes `run_scan` return early. `Recognition::scan` keeps the overlay's move/resize handler from redrawing an older lookup while a new scan is under way. An empty click on the overlay calls `overlay_dismiss`: in a whole-screen lookup with the pop-up pinned ("Fixar"), it hides only the pop-up and keeps the words clickable, and the next empty click closes the overlay (Lookupper's behavior, which the user wants only when pinned). Otherwise it closes the overlay and the pop-up at once. Esc or X in the pop-up always closes both (`hide_popup`). The "Área lida" setting is gone: `migrate()` forces `ocrScope` to Region, and the capture-region size is always editable.
- **Freeze:** per app, as in Lookupper: each profile in Configurações › Jogos e programas has `freeze` = `none` ("Sem congelar", the default: the screen stays live and only the words are highlighted), `frame` ("Congelar": the captured frame covers the screen; on whole-screen lookups it is blurred with sharp word cut-outs) or `process` ("Pausar o programa"). There is no global freeze option; the short-lived `freezeOnLookup` key is ignored, and `migrate()` turns saved `default` profiles into `none`.
- **Sentence:** like Lookupper, the context is the word's paragraph, and `Pick.sentence` is the sentence cut from it.
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
- **Floating windows on Windows:** `platform::win::keep_tool_window` makes the overlay and the pop-up tool windows (`WS_EX_TOOLWINDOW`, no `WS_EX_APPWINDOW`), as Lookupper's are.
  - Chromium's occlusion tracker stops drawing a browser window that other windows seem to cover fully, which made a video under the whole-screen overlay go black. `gfx::IsWindowVisibleAndFullyOpaque` ignores tool windows.
  - tao rewrites the extended style on state changes. A `SetWindowSubclass` handler for `WM_STYLECHANGING` keeps the bit.
  - Linux has no such tracker; the user checked that videos stay visible there.
- **Settings files (Configurações › Aplicativo › Exportar e importar):** `src/lib/settings-file.js` writes every setting, including the `ai` and `anki` options, without any key named `apiKey` or `lensApiKey`.
  - On import, `keepSecrets` puts this computer's keys back wherever the file has none, so a plain copy of `settings.json` also imports.
  - `import_settings` (lib.rs) migrates the settings and drops MDict dictionaries whose file is not on this computer, returning their names. It keeps the dictionaries installed here that the file lacks, then applies the settings like `save_settings`. The main window then reloads.
- **Word edges (`ocr/ink.rs`), as in Lookupper's "word edges are snapped to the ink of the line":**
  - CTC gives each character only a step centre. `ink::measure` finds the crop's ink: pixels far from the median background, in the run of rows holding the most ink.
  - `ink::spans` moves each boundary between two characters to the widest blank gap between their centres, and the line's ends to its first and last ink. Spaces fill the gap between words.
  - `OcrLine.ink_y` holds the ink rows; word boxes and the pick use them (`text_rows` in words.rs).
  - Boxes keep a little room around the ink (`INK_PAD_Y` 12% of the ink height above and below, `INK_PAD_X` 8% at the sides), never past the detected line box, and neighbours that touch share the gap. Raise these two constants if the highlight still looks tight.
  - Drawn box and hit area are separate, so the tight box does not make words hard to hit. `WordBox.hit` (`OverlayWord.hit`, `[x, y, w, h]`) spans the line's whole detected height and reaches halfway to the neighbouring words, or 0.25 × line height past a line's ends. In the overlay, `.w` is that area and takes the clicks and the hover, and its `.ink` child (`pointer-events: none`) draws the highlight and the cut-out. The pointer mode never used these boxes.
  - `Pick::is_under` tests the drawn box, the same one the user sees highlighted. Before 2026-10-15 it reached a quarter of the line height past the word box, which then spanned the whole line height and half the spaces. That made the space between words and lines count as on a word, so the block outline almost never appeared.
  - With little contrast, under 40 levels, the old halfway estimates stay. Google Lens lines have no ink.
- **OCR languages (`ocr::LANGUAGES`):** the same 38 entries and models as Lookupper's table.
  - Latin scripts use `rec_tiny`, except Romanian. Romanian, Japanese and Chinese use `rec_small`. Korean, Arabic and Greek use their own PP-OCRv5 models. Russian, Ukrainian, Bulgarian and Serbian (Cyrillic) use `rec_cyrillic`.
  - Names are in English and the list is sorted by them. An unknown tag falls back to `DEFAULT_LANGUAGE` (en-US).
  - Regional variants (en-US/en-GB, es-ES/es-MX, pt-BR/pt-PT, fr-FR/fr-CA, the three Chinese) read the same.
  - The tag also sets the pop-up's speech voice (`speak` in popup/main.js) and is saved on word cards. That is why the variants were kept, while Lookupper shows one Spanish.
- **Definition and translation languages (`LANGUAGES` in src/main/ui.js):** Lookupper's 58 entries: the 53 languages it translates into, plus en-US/en-GB, es-ES/es-419, pt-BR/pt-PT, Serbian Cyrillic/Latin and Chinese Simplified/Traditional. They are named in English. The AI prompt names them with `Intl.DisplayNames` in English (`languageName` in word-card.js). `settings::migrate` turns the older tags `es-MX`, `zh-CN` and `zh-TW` into `es-419`, `zh-Hans-CN` and `zh-Hant-TW`.
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
- **CI:** every push to `tests` or `main` and every PR builds both platforms. To check one run, use `gh run list --limit 1` and `gh run view <id> --log-failed`.
- **Commit messages:** imperative subject, then a body that explains why.

## Log

Newest first. One line per change that matters to the next agent.

- 2026-10-15 (Claude, `tests`): Yomitan articles laid out like Lookupper's (headword line with muted reading and grey tags, numbered senses, glossary line breaks run together, rules not shown); entry frames have no box, a hairline between entries. Anki: `anki.aiCardMode` for {ai-card}/{ai-card-plain}: 'card' (default) is the AI part alone, 'dictionaries' adds the dictionaries' entries after it; the context is never in it.
- 2026-10-15 (Claude, `tests`): imports also take RAR (RAR4/RAR5, volume sets), 7z, TAR (+gz/bz2/xz) and single gz/bz2/xz files, like Lookupper (`dictionaries/archive.rs`; pure-Rust rars, sevenz-rust2, tar, lzma-rust2).
- 2026-10-15 (Claude, `tests`): Lookupper's own format variants added, detected as Lookupper detects them: jmdict-simplified/kanjidic2 JSON (`jsondict.rs`), WN-LMF (`wnlmf.rs`), Gutenberg Webster text (`webster_text.rs`), GCIDE CIDE.* (`cide.rs`, entity table from GCIDE's webfont.txt).
- 2026-10-15 (Claude, `tests`): every format read in place with `.lkidx` (DSL, StarDict, CC-CEDICT, Kaikki, WordNet, XML, Slob); SQLite store and rusqlite removed.
- 2026-10-15 (Claude, `tests`): dictionaries stored the Lookupper way: ZIPs unpacked into `content/<id>/` and read in place, `.lkidx` headword index (Yomitan first), no MDict sidecar; other formats still on SQLite until ported.
- 2026-10-15 (Claude, `tests`): Hunspell dictionary forms (`dictionaries/hunspell.rs`, hunspell-rs bundled) for de/en/es/fr/pt/ru with Lookupper's same wooorm/dictionaries releases, fetched by `fetch-assets.py`; tried after the on-screen spelling and before the rule guesses.
- 2026-10-15 (Codex, `tests`): independent multi-format dictionary engine and persistent SQLite/MDict indexes, safe ZIP/resources, MDict block/alias/style fixes, bounded language-aware JA/EN/PT candidates, popup source order and complete homograph Anki export. Format/evidence limits in `docs/dictionary-engine.md`; actual reader tests run on Actions through `scripts/dictionary-harness`.
- 2026-10-15 (Claude, `tests`): definition and translation languages match Lookupper's 53 (58 with variants), named in English.
- 2026-10-15 (Claude, `tests`): OCR languages match Lookupper's 38. Serbian (Latin and Cyrillic), Slovenian, Bosnian (Latin), Latin and Indonesian were added; names are in English.
- 2026-10-15 (Claude, `tests`): the shortcut counts as on a word only inside its drawn box (`Pick::is_under`), so a pointer in the space between words or lines outlines the block instead of looking up the nearest word.
- 2026-10-15 (Claude, `tests`): overlay words take the pointer over a wider hit area than their drawn box, because the ink-tight boxes had become hard to hover and click.
- 2026-10-15 (Claude, `tests`): the snapped word boxes got a little room around the ink, because they looked too small.
- 2026-10-15 (Claude, `tests`): word edges and heights snap to the ink of the line (`ocr/ink.rs`).
- 2026-10-15 (Claude): `claude/overlay-spotlight` was renamed `tests`, and every other branch except `main` was deleted (all were already contained in it). From now on every agent works on `tests`, and only the user merges it into `main`.
- 2026-10-15 (Claude): the repository was recreated so that no commit shows a personal e-mail. The old one is now the private `arrow-poppa/lookitan-old`, with PRs #1–#3.
  - Every branch was rewritten with the GitHub noreply address, so hashes before this entry differ from the ones quoted below. The file contents are unchanged.
  - Main tips: `052b339` → `19468c4`; codex `9ce4339` → `db0c4d1`.
  - Clones made before this must `git fetch --prune` and reset their branches to the new `origin/*`.
  - Commit only with `184733469+arrow-poppa@users.noreply.github.com`.
- 2026-10-15 (Claude, branch `claude/overlay-spotlight`): settings export and import without the API keys (`settings-file.js`, `settings-transfer.js`, `import_settings` and `read_text_file` in lib.rs).
- 2026-10-15 (Claude, branch `claude/overlay-spotlight`): on Windows, the overlay and the pop-up are kept tool windows (`keep_tool_window`, which adds the `Win32_UI_Shell` feature), so the browser no longer goes black under a lookup (Chromium occlusion tracking). No change on Linux.
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
