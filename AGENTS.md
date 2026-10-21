# Lookitan: notes for coding agents

- 2026-10-20 (Claude, `tests`): "Imagem AI" explains a text model asked for a picture, and shows "Qualidade" only with OpenAI.
Read this file before you change anything. When your work changes something described here, update the file in the same commit. Add a dated line to the log at the bottom.

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
- **Google Lens key:** never in the source. Builds get it from the GitHub Actions secret `LOOKITAN_LENS_API_KEY` (Chromium's public Lens key, as owocr and chrome-lens-ocr send it), read at compile time with `option_env!` in ocr/lens.rs, at the user's request on 2026-10-21. It still ends up inside the binary. A key the user pastes in Settings › OCR is sent instead; a build without the secret (local, a fork's pull request) needs the user's key. Never embed any other key.
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
- **Scan result:** the pop-up opens only when the pointer is on a word (`Pick::is_under`): inside the word's drawn box, give or take `UNDER_SLACK` (10% of the box height, at least `UNDER_SLACK_MIN` = 3 capture pixels). The space between words or lines is off any word. Otherwise the automatic capture outlines the block of text (see block.rs), and otherwise the scan re-captures the whole monitor and shows the focused overlay of clickable words with no pop-up, as Lookupper does. Also like Lookupper, that whole-screen overlay appears only after the background pass has read the whole screen (`deferred` in `run_scan`), not progressively; Right after the whole-screen capture, `show_overlay_window` raises the overlay transparent (body class `idle`, only the 0.01-alpha shade), so clicks and the scroll wheel cannot move the screen away from the captured frame while OCR runs; the words appear with the final `emit_overlay` (immediately on Linux, with a fade elsewhere). A click or Esc meanwhile cancels: `overlay_close` bumps `scan_seq`, which stops the background pass and makes `run_scan` return early. `Recognition::scan` keeps the overlay's move/resize handler from redrawing an older lookup while a new scan is under way. An empty click on the overlay calls `overlay_dismiss`: in a whole-screen lookup with the pop-up pinned ("Fixar"), it hides only the pop-up and keeps the words clickable, and the next empty click closes the overlay (Lookupper's behavior, which the user wants only when pinned). Otherwise it closes the overlay and the pop-up at once. Esc or X in the pop-up always closes both (`hide_popup`). The "Área lida" setting is gone: `migrate()` forces `ocrScope` to Region, and the capture-region size is always editable.
- **Pop-up placement (`popup_position` in lib.rs, both platforms):** WinUI's TeachingTip targeted placement, which is what Lookupper's lookup popup uses. That popup is `Lookupper.Lookups.Views.WordTeachingTip`, a UserControl holding a `TeachingTip` named `TeachingTipControl` (style `LookupTeachingTipStyle`). `OverlayView.AddWordTeachingTip` adds it to the full-monitor overlay, aimed at a rectangle on the word. Its XAML in `resources.pri` sets no `PreferredPlacement`. The algorithm was read from microsoft-ui-xaml `controls/dev/TeachingTip/TeachingTip.cpp` (`DetermineEffectivePlacementTargeted`, `GetPlacementFallbackOrder`, `PositionTargetedPopup`) on 2026-10-21:
  - Modes are tried in order, and the first whose fit test passes wins (`PLACEMENT_ORDER`). Top/Bottom need the card centred on the word to fit, so a word near a side gets Left or Right before any shifted variant (TopLeft…RightBottom). That is the user's "passou" screenshot.
  - Sizes from TeachingTip_themeresources.xaml: the card sits in a grid with an 8 px strip on every side (`TeachingTipTailShortSideLength`), and the 20×10 tail is drawn in the strip facing the word. So the card is 8 px from the word; Lookitan has no tail but keeps the gap. In the shifted variants the word's centre is 28 px from the grid edge (`MinimumTipEdgeToTailCenter` = 8 + 10 + 10), i.e. 20 px into the card. Bounds: the whole monitor (the overlay window). `ShouldConstrainToRootBounds` must be true, since with false WinUI always returns Top.
  - Open question: WinUI's Auto order starts with Top; a Bottom preference swaps Top and Bottom. Lookupper's code-behind could set either, and the screenshots so far fit both. Lookitan keeps Bottom first, as before. Check by looking up a word in Lookupper with room both above and below.
  - Where WinUI would not open the tip at all, Lookitan still shows it, centred below or above and kept on the monitor.
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
- **Overlay entry animation:** Linux skips the whole-screen overlay body opacity fade to avoid a one-frame loss of dimming in WebKitGTK. The user confirmed that the no-fade build resolves the flicker. The dim canvas, word highlights, blank-before-hide handshake and popup animations remain; Windows keeps the 220 ms fade. The `with-fade` branch preserves the previous behavior for reference.
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
  - With little contrast, under 40 levels, the old halfway estimates stay.
  - Google Lens lines are snapped too (`snap_to_ink` in ocr/lens.rs, since 2026-10-21): `ink::measure` on the line's box in the capture gives `ink_y`, and each word's ends move to its first and last ink column within 0.15 line heights of Lens's word box (`INK_REACH`), never past halfway to the next word. Vertical lines keep Lens's boxes.
- **OCR languages (`ocr::LANGUAGES`):** the same 38 entries and models as Lookupper's table.
  - Latin scripts use `rec_tiny`, except Romanian. Romanian, Japanese and Chinese use `rec_small`. Korean, Arabic and Greek use their own PP-OCRv5 models. Russian, Ukrainian, Bulgarian and Serbian (Cyrillic) use `rec_cyrillic`.
  - Names are in English and the list is sorted by them. An unknown tag falls back to `DEFAULT_LANGUAGE` (en-US).
  - Regional variants (en-US/en-GB, es-ES/es-MX, pt-BR/pt-PT, fr-FR/fr-CA, the three Chinese) read the same.
  - The tag also sets the pop-up's speech voice (`speak` in popup/main.js) and is saved on word cards. That is why the variants were kept, while Lookupper shows one Spanish.
- **Definition and translation languages (`LANGUAGES` in src/main/ui.js):** Lookupper's 58 entries: the 53 languages it translates into, plus en-US/en-GB, es-ES/es-419, pt-BR/pt-PT, Serbian Cyrillic/Latin and Chinese Simplified/Traditional. They are named in English. The AI prompt names them with `Intl.DisplayNames` in English (`languageName` in word-card.js). `settings::migrate` turns the older tags `es-MX`, `zh-CN` and `zh-TW` into `es-419`, `zh-Hans-CN` and `zh-Hant-TW`.
- **OCR engines:** `ocrEngine` reads around the pointer; `screenOcrEngine` reads whole-screen passes (`null` means "same as `ocrEngine`"). `Settings::engine_for(whole_screen)` picks one.
- **Google Lens:** chosen in Configurações › OCR › Motor de OCR. CI builds work without a key (the `LOOKITAN_LENS_API_KEY` secret; Chromium's key and YomiNinja's default both answered 200 on 2026-10-21); the user's key, if pasted, overrides it. If Lens fails, the scan falls back to PP-OCRv6. Each lookup uploads the capture to Google, shrunk only past 3 megapixels (`fit`, as owocr), with `surface` SURFACE_CHROMIUM. Lookupper has no Lens at all: its exe has no `googleapis`, `crupload` or `lens` strings (checked 2026-10-21); its 1920 px is the local detector's resize. When Lens fails, that pass is read by PP-OCRv6 and the next lookup tries Lens again. The reason is shown in Portuguese: in the pop-up (`notice`), or, when no pop-up opened (a whole-screen lookup), in the overlay's `#lens-notice` strip at the top, which a word click hides.
- **KDE Wayland:**
  - The cursor comes from the KWin script over D-Bus (`io.github.arrowpoppa.Lookitan` `/Cursor`).
  - Capture uses `org.kde.KWin.ScreenShot2.CaptureArea`, which is authorized only when Lookitan runs from the installed `.desktop` entry.
  - The windows run under XWayland.
  - On Wayland, the global shortcut is a system shortcut that runs `lookitan --scan`.
- **Pictures ("Imagens", built-in dictionary `ai-images`, `DictionaryKind::AiImages`):** Migaku's pop-up image tab, by behavior (its extension was read, no code or text copied).
  - The AI turns the word into an English search query with its own prompts, `ai.imageQuerySystemPrompt` and `ai.imageQueryPrompt` (Migaku's "Image query" system/user pair). The system prompt replaces the shared `systemPrompt` for this request only (`explain(..., {system})`). Variables: `{{target}}`, `{{sentence}}` (the sentence, not the paragraph), `{{wordLanguage}}` (the OCR language), `{{definitionLanguage}}`, `{{translationLanguage}}`. `cleanQuery` keeps the answer's first line. If the AI fails, is not configured, or `ai.imageQueryUseAi` is off, the word itself is searched, as Migaku does on failure. The answer is cached like the other AI answers (kind `images`).
  - Search: `GET https://www.bing.com/images/async?q=…&first=0&count=100&adlt=strict` through tauri-plugin-http (Bing sends no CORS headers), Migaku's exact parameters. Bing has had no free image API since 2025. Each result's `m` attribute is JSON: `murl` (the picture), `turl` (Bing's thumbnail), `t`, `purl`. The strip shows `turl`; the viewer and Anki use `murl`, falling back to `turl`. Migaku itself keeps only `murl` (regex `murl&quot;:&quot;(.*?)&quot;`), which `parseBingImages` still uses as a fallback. The first 10 are shown. An empty answer is asked once more, because Bing now and then answers with nothing. No User-Agent spoofing: Migaku rotates five browser UAs, and that was not needed in tests on 2026-10-20. Migaku's `IMAGE_QUERY_PREFIX`/`SUFFIX` are hidden env keys and are not ported. Meme mode is ported: a "Modo meme" switch under the strip (`ai.imageMemeMode`, `withMemeMode`) appends " meme" to the search and searches again at once, reusing the cached AI query. The pop-up saves only that key (`saveMemeMode`: fresh `get_settings`, then `save_settings`), and the main window copies it on `settings-changed`, so its own saves do not undo it.
  - Pop-up: like Explicar, the section shows "Mostrar imagens" and loads on click, unless `ai.autoShowImages` is on. Pictures sit in a sideways strip (156×140 tiles; the wheel scrolls it while it can). The check mark in the top-right corner appears on hover and stays on a checked picture. Only a click on it checks the picture; a click anywhere else opens the viewer over the pop-up. The viewer moves to the neighbours with the arrows at the picture's sides, ← →, or by dragging the picture sideways at least `SWIPE_PX` (60 px; pointer events, so mouse, pen and touch). Esc or a click outside closes it, and the click that ends a drag does not count as one. Its bar has the position (2/10), "Marcar para o Anki" and the source page. A picture that fails to load is removed, as in Migaku.
  - Anki: `{image}` holds the checked pictures, each downloaded, scaled to at most 800 px (`ANKI_IMAGE_MAX_SIDE`; Migaku scales to 800 px wide, as webp) as JPEG, and stored as `lookitan_image_<n>_<date>.jpg`. GIFs, and pictures the WebView cannot decode, are stored as they are. Nothing is checked by default, so `{image}` stays empty unless the user checks something. `suggestMarker` gives `{image}` to fields named image/picture/photo, and `{screenshot}` only to "screenshot".
- **Generated pictures ("Imagem AI", `ai-image-gen`): removed on 2026-10-20 at the user's request, in favour of the Migaku-style "Imagens" only.** The whole feature is in the `image-ai-backup` branch: named prompts, OpenAI/Gemini/OpenRouter image models, `{ai-image}`, pictures kept in IndexedDB `lookitan-image-gen`. Nothing on `tests` names it any more. Settings saved meanwhile still load through a general rule: `readable_dictionaries` (settings.rs) leaves out any dictionary entry of a kind this version cannot read. Without it, serde would make the whole file invalid and `load()` would fall back to the defaults. Its keys in `ai` (`imageGen*`, `autoGenerateImages`) are ignored. Its `sectionProviders` block goes on the next save, since `normalizeAiOptions` rebuilds that object.
- **AI providers per dictionary:** each AI dictionary in the Dicionários tab has its own provider block, and a change there affects only that dictionary. The block holds the provider, its accounts (key, model, thinking, routing, extra JSON), temperature, streaming and "Cancelar pedidos pendentes" (`PROVIDER_FIELDS`), plus a system prompt for the card and the translator. "IA — palavra no contexto" keeps its block at the top of `ai`, as in the fork. The context translator and "Imagens" keep theirs in `ai.sectionProviders['ai-context' | 'ai-images']`. `normalizeAiOptions` fills a missing block with a copy of the word card's, so older settings keep working with the same key. Requests use `aiOptionsFor(ai, id)`: the shared prompts plus that dictionary's block. The popup's `optionsOf(kind)` maps card/explain → ai-word, context → ai-context and images → ai-images. Cancelling follows each dictionary's own switch (`keyKinds`, `keepRunning`). Each dictionary also has its own automatic switch: `autoGenerateOnLookup` (card), `autoExplain`, `autoTranslateContext` (which starts from `autoGenerateOnLookup` for older settings) and `autoShowImages`. Settings files strip and restore `apiKey` at any depth, including in `sectionProviders`.
- **Pop-up order (Dicionários tab):** drag a row (grip on the left) or press Alt+↑/↓ on it. The list uses pointer events (`sortable`, `dropIndex` in dicts-tab.js), not HTML drag and drop. Tauri's native file drop handler is on by default (`dragDropEnabled`), and it keeps WebView2 from delivering HTML drag events; WebKitGTK is unreliable with them too. That is why the old `draggable` rows could not be moved. Do not go back to `dragstart`/`drop`. The row moves among the others while dragged, and on release `update_dictionary(moveTo)` saves the order.
- **Trackers in the pop-up header (`btn-saved`, `btn-anki-notes`):** two counters, added without removing any button, as the user asked. They follow Yomitan's "view added note" button and Lookupper's bookmark, which changes colour once a word is saved.
  - Saved in the app: the bookmark-filled counter is the number of saved cards whose `word` or `dictionaryForm` matches the lookup's word, the AI card's headword or the first entry's headword (case-insensitive; `savedCardsOf`). A click opens `.saved-view` over the pop-up, like the image viewer. It lists those cards newest first, each whole as the Palavras tab has it (`savedCardHtml`): IPA, CEFR, part of speech, labels, the form on screen, translation, definition, explanation, the paragraph with the word marked, context translation, the full-width screenshot, date and window, plus the dictionaries' entries folded in `<details>` (`cleanGlossaryHtml`). Each card can open its Anki note or be deleted. Esc or X closes it.
  - Anki: the cards counter is the number of notes `findNoteIds` finds, plus the note just added. It runs one duplicate-check query per form of the word (as found, the first entry's headword, the AI card's once there; `wordFormList`) in one request, and does not wait for the AI card. Only the newest check of a lookup is shown (`ankiNotesCheck`). Before 2026-10-20 it waited for the AI and used only its headword, so it showed late, or never when the AI request was replaced. The query must use the note type's first field, as Anki's duplicate check does. The settings come back from Rust with object keys sorted (serde_json has no `preserve_order` here), so `cardFormats[0].fields`' first key is the alphabetically first field. Before the fix, the counter searched e.g. an empty "Audio" and showed the same number for every word. `orderNoteFields` (notes.js) now reorders the note by `getModelFieldNames` (asked once per pop-up session per note type, again after `settings-changed`). The popup skips the query when that first field is empty, and the "open in Anki" path of the ✓ button does the same. A click opens them in Anki's browser (`guiBrowseNotes`). It shows only with the Anki integration on and "Verificar duplicatas" on. Without AnkiConnect there is no counter and no notice.
  - Both refresh on render, after saving or adding, and whenever `refreshAnkiButton` runs, since the AI card can change the headword.
- **Overlay:** no hint pill ("Clique em outra palavra · Esc para fechar"). It showed only on pointer lookups with a pop-up, and the user asked for it to go.
- **Word audio (Configurações › Áudio, `settings.audio`, owned by src/lib/audio.js):** an ordered list of sources, tried top to bottom as in Yomitan's audio sources.
  - `local-db` is a local audio database in local-audio-yomichan's format (MIT; behavior only): SQLite `entries (expression, reading, source, speaker, display, file)`. The android.db variant adds `android (file, source, data BLOB)` holding the audio; entries.db has the files in `<source>_files/<file>` next to it. audio.rs opens it read-only (rusqlite, back for this only). It keeps the last `AUDIO_DBS_OPEN` (3) databases used open, newest first, and the oldest closes when a fourth is opened. Its read settings come from yomitan-audio-fast (MIT): query_only, temp_store MEMORY, cache 4 MB, mmap 64 MB. The last 256 lookups per database are kept in memory. That project's own audio pack and Rust index are not ported, because Lookitan reads through IPC and has no HTTP hop to optimize. It matches `expression` plus the reading (rows without a reading always match) and orders by local-audio's source order (nhk16, shinmeikai8, forvo, jpod, jpod_alternate, ozk5). Names follow its display patterns ("NHK16 %s", "Forvo (%s)"). `audio_data` never leaves the source folder (no `..`).
  - Manatan's public repo (checked 2026-10-20) has no audio .db of its own; its /audio endpoint returns online provider URLs, and "one-click local audio setup" is on its roadmap. This is the db people use with it and Yomitan.
  - `custom` is Yomitan's Custom URL (the template answers with the audio); `custom-json` is Custom URL (JSON) (answers `{type: "audioSourceList", audioSources: [{name, url}]}`). Templates take `{term}`, `{reading}` (the term when there is none, since Lookitan has no reading yet) and `{language}` (the OCR language's base tag). URLs are fetched through tauri-plugin-http and played from a Blob URL, so http sources work from both platforms' WebView origins. Only `audio/*` (or octet-stream) answers are kept.
  - `tts` is the system voice, the default and only source of new settings. The user asked to keep it alongside the others.
  - Yomitan's built-in sources (yomidevs/yomitan `ext/js/media/audio-downloader.js`, GPL-3.0; the behavior is reimplemented, not copied). All of them stay; the user plans more Japanese support later.
    - `jpod101`: `assets.languagepod101.com/dictionary/japanese/audiomp3.php?kanji=&kana=`, Japanese only. It needs the reading for a word written in kanji (kanji alone returns the "not available" clip). The pop-up has no reading yet, so `japaneseReading` asks the Jisho API (`/api/v1/search/words`, exact `word` match, 200 cached). The "not available" clip (SHA-256 `ae6398b5…`) counts as no audio, as in Yomitan.
    - `language-pod-101`: POST `dictionary_reference` to `www.<language>pod101.com` or `<language>class101.com` (Yomitan's pod/class list). Rows are `.dc-result-row` and are parsed with regexes (no DOMParser in tests). Japanese rows are checked against `.dc-vocab_kana`, other languages against `.dc-vocab`.
    - `jisho`: `<audio id="audio_<term>:<reading>">` on the search page; any reading when none is known. Jisho answers 403 without a browser User-Agent, so its requests send one.
    - `lingua-libre` and `wiktionary`: Wikimedia Commons search plus imageinfo, with Yomitan's filename checks. They send a descriptive User-Agent, as Wikimedia asks. `ISO639_3` maps the OCR language to Lingua Libre's categories. Commons serves .ogg as `application/ogg`, which `audioFile` accepts.
    - Yomitan's `text-to-speech-reading` is left out: without a reading it would be the same as `tts`.
    - `findAudio` asks every source at the same time (`Promise.all`) and keeps the list order, so a slow site does not hold up the rest. Checked live on 2026-10-20: 読む gave JPod101, Jisho and LanguagePod101 audio; livro (pt), livre (fr) and water (en) also returned audio.
  - Configurações › Áudio is laid out like Yomitan's audio sources. Each row has ↑/↓ buttons (`moveButtons`), and Alt+↑/↓ works too. The user asked for arrows instead of dragging; the Dicionários list still drags. The section folds under its first row, "Áudio da palavra", which shows the order (`collapser('audio')`, like Anki). A local database also lists the sources inside the file (`audio_db_sources`, `subsources: [{name, enabled}]`), which have the same arrows and can be switched off one by one, as Fushi (GPL-3.0) does. `mergeSubsources` keeps saved ones in place, adds new ones at the end and drops gone ones. `audio_lookup(order)` filters and ranks by that list; an empty list means all sources in the default order.
  - The panel normalizes `settings.audio` once and edits it in place. Re-normalizing per row had detached the rows from the saved object, so changing a type or adding a source did nothing (fixed 2026-10-20).
  - Fushi's lesson: never write to the user's .db. It created indexes in the original file; Lookitan only opens it read-only.
  - Context speaker: the word card's "Contexto" and the Tradutor de contexto each have a speaker (`speak-sentence`). It reads the original text with the system voice (the audio sources only have words); a second click stops it.
  - Context speaker: the word card's "Contexto" and the Tradutor de contexto each have a speaker (`speak-sentence`). It reads the original text with the voice chosen in "Voz para ler o contexto" (`settings.audio.context = {type: 'tts' | 'edge-tts', voice}`): the system voice by default, or an Edge neural voice that falls back to the system voice if it fails. A second click stops it.
  - `edge-tts`: Microsoft Edge's "Read aloud" neural voices, as Fushi (GPL-3.0, `packages/fushi_engine/lib/tts/providers/edge_tts_provider.dart`) offers them. This is the protocol of rany2/edge-tts, reimplemented in `src-tauri/src/edge_tts.rs` (tungstenite 0.30 with rustls and reqwest's ring provider, plus sha2).
    - It is a websocket with the Edge token, `Sec-MS-GEC` and `Sec-MS-GEC-Version=2-<Edge version>`, plus Edge's User-Agent and the read-aloud extension's Origin. It must live in Rust: a WebView cannot set those headers, and the service refuses the connection without them.
    - Since 2026-10-13 the token is v2 (edge-tts 7.3, `drm.py`): Edge's keyed SHA-256 of the Windows file time (rounded to 5 minutes) plus the token, with its CS64 checksum, as `<16 hex>-<64 hex>`. `edge_tts::gec` has it; its test holds edge-tts 7.3.1's values.
    - Hosts, measured 2026-10-20: `speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1` (`TrustedClientToken=`) takes nearly every v2 connection (7 of 8) and refuses v1 (403); `api.msedgeservices.com/tts/cognitiveservices/websocket/v1` (`Ocp-Apim-Subscription-Key=`) takes only about a third of them with either token. Each piece tries the old host, then the new one, then the old one again (`ATTEMPTS`). The voice list is on the old host.
    - A 403 syncs the clock from Microsoft's Date header and retries once. Texts over 600 characters are read in pieces cut after punctuation. The last 24 recordings are kept in memory.
    - Commands: `edge_tts_voices` and `edge_tts_speak(text, voice, language)`. `Voice` renames only on deserialize (`ShortName` → `id`…). Renaming both ways sent Microsoft's names to the front end, and the voice list stayed on "Carregando as vozes…" (fixed 2026-10-20); the panel now accepts both shapes and shows an error instead of hanging. An empty voice means the language's default voice (Nanami for ja, Francisca for pt-BR…, else the first one of the locale). Entries are `{kind: 'edge', text, voice, language}`, and `audioFile` reads them as MP3, so Anki's `{audio}` can store them.
    - Anki `{sentence-audio}` (`sentenceAudioFile` in audio.js): the sentence, never the paragraph, whatever "Contexto no card" says, as the user asked. It is read by the Edge voice of "Voz para ler o contexto" and stored as `[sound:lookitan_sentence_audio_<date>.mp3]`. speechSynthesis cannot write a file, so with the system voice chosen it uses the language's default Edge voice. It is fetched only for a real note whose fields use it, never for duplicate checks, and also works from the Words tab (`noteContextFromCard(card, order, settings)`). `suggestMarker` gives it to fields named like "Sentence Audio" before the `{audio}` rule. The Rust cache means a sentence already played is not fetched again.
    - The settings list every Edge voice, not only the OCR language's, as the user asked. They are grouped by language (`<optgroup>`, named in Portuguese) and have a preview button that speaks a sample in the voice's own language. "Automática" is the OCR language's default voice.
    - Not compiled locally: this machine's Rust is the GNU toolchain without dlltool. The protocol was checked with a Node script that sends the same messages: 読む, a sentence and livro all came back as MP3 in about 1 s.
  - Pop-up: the AI card's speaker plays the first recording that plays. `others` sets how the rest are shown: 'menu' (default), a right click on the speaker listing every recording found (`audio-menu`), or 'buttons', one small button per recording next to the speaker (`audio-buttons`). "Tocar o áudio automaticamente" (`autoPlay`) plays on lookup. `{audio}` in Anki stores the recording last played, else the first readable one (never the system voice), as `[sound:lookitan_audio_<date>.<ext>]`. `suggestMarker` gives `{audio}` to fields named audio/sound.
- **AI cache:** same semantics as the fork. It has a 60 s TTL, joins in-flight requests and never caches partial or failed answers. `cancelPendingExcept` drops requests the pop-up no longer shows.
- **Anki:** uses Yomitan's `anki` options shape and field markers, through AnkiConnect.
  - tauri-plugin-http always sends `Origin: http://tauri.localhost` (Windows) or `tauri://localhost` (Linux). AnkiConnect allows only `http://localhost`, 127.0.0.1 and browser-extension origins by default (that is why Yomitan works) and answers 403 to anything else.
  - Field markers follow Yomitan: `{glossary}`, `-brief` (no parenthesis), `-no-dictionary` (tags only), `-plain`, `-plain-no-dictionary`, `{glossary-first…}`, `{single-glossary-<kebab name>}` with the same five variants (Yomitan's `getKebabCase`), and `{popup-selection-text}`. The AI card is the dictionary `IA` (tags = part of speech + labels). Definitions follow the user's dictionary order (`dictionaryOrder`). `{ai-explanation}` is rendered Markdown and `{ai-explanation-plain}` is plain text. `{ai-card}` (HTML) and `{ai-card-plain}` export the whole AI card (headword, IPA, CEFR, part of speech, labels, translation, definition, and the explanation if it was generated) without the context, which goes in `{sentence}`. If a field uses `{ai-explanation…}` and Explain was not run, the "+" button generates it first.
  - MDict entries are cross-origin iframes, so `ENTRY_SCRIPT` posts `{type: 'selection'}` messages and the pop-up keeps the latest non-empty selection.
  - An AnkiConnect that is not running gives no notice while looking up words: the duplicate check's error goes only to the "+" button's tooltip. An error after clicking "+" is still shown in the pop-up.
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

- 2026-10-21 (Claude, `main`/`edge-tts`): at the user's request `main` and `edge-tts` were fast-forwarded to `tests`, and a backup copy of `tests` was pushed to the private repo `arrow-poppa/lookitan-old` as branch `backup/tests-2026-10-21` (its own `main` left alone). Right after, the user had `without-fade` fast-forwarded to `tests` too.
- 2026-10-21 (Claude, `tests`): Google Lens read badly because Lookitan shrank every capture to 1000 px and 1 MP (YomiNinja's limits): a whole 1920 × 1080 screen was halved. On a synthetic 5-language test screen Lens read 5/5 lines at full size and 2/5 (12 px text) or 3/5 (15 px) shrunk. Now only captures over 3 MP shrink, with Lanczos, as owocr, and the request sends `surface` SURFACE_CHROMIUM (4) as Chromium does.
- 2026-10-21 (Claude, `tests`): at the user's request the built-in Lens key left the source: CI passes the `LOOKITAN_LENS_API_KEY` secret to the build, and `option_env!` reads it.
- 2026-10-21 (Claude, `tests`): at the user's request a whole-screen lookup also says when Google Lens failed: `Recognition.notice` goes to the overlay, which shows it at the top of the screen while no pop-up is open. The message is now in Portuguese.
- 2026-10-21 (Claude, `tests`): at the user's request Configurações › OCR › Região de captura offers only Automático and Personalizada; the fixed-size formats (Original 900 × 260, 16:9, 4:3, 1:1) were removed. A saved fixed size shows as Personalizada with its size.
- 2026-10-21 (Claude, `tests`): at the user's request Google Lens works without pasting a key: it sends Chromium's public Lens key, and the user's key still overrides it. The hard constraint was changed to match.
- 2026-10-21 (Claude, `tests`): with Google Lens the word highlight did not hug the ink as with the local models: Lens lines had no `ink_y` and kept Lens's loose word boxes. `snap_to_ink` now measures the ink of each Lens line in the capture.
- 2026-10-21 (Claude, `main`/`without-fade`): at the user's request, `main` and `without-fade` were fast-forwarded to `tests` again, with the Edge v2 token fix.
- 2026-10-21 (Claude, `tests`): the Edge voices failed most of the time (and so did `{sentence-audio}`, whose failure left the field empty without a word): the service moved to the v2 Sec-MS-GEC on 2026-10-13. Lookitan now sends v2 to `speech.platform.bing.com` first, with `api.msedgeservices.com` and one more try as fallbacks, and the pop-up says when a note was added without the sentence audio.
- 2026-10-21 (Claude, `main`/`without-fade`): at the user's request, `main` and `without-fade` were fast-forwarded to `tests`, with the WinUI pop-up placement.
- 2026-10-21 (Claude, `tests`): pop-up placement is now WinUI's TeachingTip algorithm, read from its source, as Lookupper's popup uses it (REA/strings showed the TeachingTip and its XAML). The gap is 8 px, a word near a side gets the popup beside it before a shifted one, and the shifted variants keep the word 20 px into the card.
- 2026-10-21 (Claude, `tests`): on small screens the pop-up goes beside the word (left, then right) when it fits neither below nor above, as Lookupper's, instead of covering the word.
- 2026-10-21 (Claude, `tests`): the pop-up is centred on the word, as Lookupper's, instead of starting 24 px to its left (`popup_position`, with tests).
- 2026-10-21 (Claude, `tests`/`main`): at the user's request, the version they use (the `edge-tts` build) went to `main`. `tests` and `main` were fast-forwarded to `edge-tts` (`6508a5a`), so the Edge voices and `{sentence-audio}` are part of `tests`. Work continues on `tests` only.
- 2026-10-21 (Claude, `tests`): at the user's request, the last traces of "Imagem AI" were cleaned from their own machine: its entry, keys and provider block in settings.json, and its IndexedDB. The Migaku-style "Imagens" and its AI query were left as they were. `image-prompts` was deleted, since `image-ai-backup` holds the same commit.
- 2026-10-20 (Claude, `tests`): no code names "Imagem AI" any more, at the user's request. Settings files load with a general rule instead: a dictionary entry of an unknown kind is left out (`readable_dictionaries`), where before the whole file fell back to the defaults.
- 2026-10-20 (Claude, `tests`): "Imagem AI" taken out at the user's request; only the Migaku-style "Imagens" stays. The feature is kept whole in `image-ai-backup`. The saved cards view showing whole cards, which came with it, stays. Older settings load: the kind is still read and then dropped.
- 2026-10-20 (Claude, `edge-tts`): Anki marker `{sentence-audio}`, the sentence read by an Edge voice. Also merged `tests` (counters for saved cards and Anki notes) into `edge-tts`.
- 2026-10-20 (Claude, `edge-tts`): fixed the Edge voice list never loading (serde renamed the fields when sending too). "Testar as fontes" shows its result under the description and says which source played and which failed.
- 2026-10-20 (Claude, `edge-tts`): Edge neural voices as a word audio source and as the voice that reads the context (`edge_tts.rs`), on their own branch at the user's request.
- 2026-10-20 (Claude, `tests`): the pop-up's buttons say "Gerar imagem AI" and "Gerar mais imagens AI", as the user asked.
- 2026-10-20 (Claude, `tests`): the generated pictures dictionary is shown as "Imagem AI" (it was "Imagens IA"), the name the user asked for, so it is not confused with "Imagens".
- 2026-10-20 (Claude, `tests`): "Imagem AI" no longer disappears when it is not set up: it says what is missing. The first copy of an OpenRouter card gets `google/gemini-2.5-flash-image` and none of the text routing.
- 2026-10-20 (Claude, `tests`): "Imagem AI" with Personalizado: thinking (mode, effort, manual value) and OpenRouter routing, as in the text provider, sent in the chat request for pictures.
- 2026-10-20 (Claude, `tests`): Anki marker `{ai-image}` for the checked "Imagem AI" pictures, at the user's request; without it they still go to `{image}`.
- 2026-10-20 (Claude, `tests`): "Imagem AI" pictures are kept on disk (IndexedDB, 100 pictures), so they come back after the app is closed. The saved cards view in the pop-up shows each card whole; the screenshot was squeezed to 120 px and several fields were missing.
- 2026-10-20 (Claude, `tests`): new "Imagem AI" dictionary (`ai-image-gen`):
  - Pictures come from an image model with the user's own prompt and provider: OpenAI, Gemini/Imagen, or OpenRouter and compatible servers.
  - 1 picture per click by default; "Gerar mais" adds more.
  - Same strip, viewer and Anki check as "Imagens": the strip and viewer now take a gallery.
- 2026-10-20 (Claude, `tests`): the Anki notes counter showed a fixed number: it searched the alphabetically first field (settings keys come back sorted) instead of the note type's first field. It now orders fields by `modelFieldNames`.
- 2026-10-20 (Claude, `tests`): the Anki notes counter shows at once and finds every form of the word; older, slower checks can no longer overwrite newer ones (also for the saved-cards counter).
- 2026-10-20 (Claude, `tests`): pop-up trackers: counters for this word's saved cards (click: they open in the pop-up) and its Anki notes (click: Anki's browser).
- 2026-10-20 (Claude, `tests`): Yomitan's built-in audio sources (JapanesePod101, LanguagePod101, Jisho, Lingua Libre, Wiktionary), asked in parallel. Audio sources now move with ↑/↓ buttons instead of dragging. The Áudio section collapses. A speaker reads the context sentence.
- 2026-10-20 (Claude, `tests`): Configurações › Áudio reworked like Yomitan, with drag to reorder and a per-database source order like Fushi. The pop-up can show the other recordings as a menu or as buttons. Also fixed: changing a source type and "Adicionar" did nothing.

- 2026-10-20 (Claude, `tests`): at most 3 local audio databases stay open (LRU), with yomitan-audio-fast's SQLite read settings and a 256-lookup memory cache per database.

- 2026-10-20 (Claude, `tests`): word audio sources in Configurações › Áudio: local audio .db (local-audio-yomichan entries.db/android.db, `audio.rs`, rusqlite 0.37 bundled), Yomitan Custom URL and Custom URL (JSON), and the system voice. Also the speaker menu, auto-play and the `{audio}` Anki marker. Before this, at the user's request, `main` and `without-fade` were fast-forwarded to `tests` at `cc69412`.

- 2026-10-20 (Claude, `tests`): the enlarged picture zooms, up to 8×, keeping the point under the pointer in place (`zoomBy`, `applyPictureZoom`). Ways to zoom: the mouse wheel, a double click (toggles 2.5×), the − / + buttons in its bar, and the + − 0 keys. With a picture enlarged, those keys zoom the picture instead of the pop-up. While zoomed, a drag moves the picture around instead of going to the next one (`gestures`, which replaced `swipe`).

- 2026-10-20 (Claude, `tests`): the context translator has "Ocultar a tradução" (`ai.hideContextTranslation`, off by default). The translation is blurred until clicked, like `hideWordTranslation`, and stays shown for that lookup once clicked (`lookup.contextRevealed`), including while it streams.

- 2026-10-20 (Claude, `tests`): the pop-up order in the Dicionários tab can be changed again: the rows are dragged with pointer events (HTML drag and drop never reached the WebView, because of Tauri's native drop handler), or moved with Alt+↑/↓.

- 2026-10-20 (Claude, `tests`): at the user's request:
  - Every AI dictionary has its own provider block, changing only its own section. The context translator and "Imagens" start from a copy of the word card's (`sectionProviders`, `aiOptionsFor`), and the translator has its own automatic switch and system prompt.
  - The image viewer has side arrows and swipe.
  - Migaku's meme mode switch is in the pop-up.
  - The overlay hint pill is gone.
  - An unreachable AnkiConnect no longer shows a notice in the pop-up.
  - The popup-flows test loader caches module promises, like ui-startup's.

- 2026-10-20 (Claude, `tests`): built-in "Imagens" dictionary (`ai-images`), Migaku's image tab by behavior. The AI image query uses its own system and user prompts; pictures come from Bing Images, shown on click or automatically. On hover a check mark appears in the corner and checks the picture for Anki (`{image}`); a click enlarges the picture. `migrate()` adds the entry after the AI ones. The ui-startup test loader now caches module promises: the new diamond import (dicts-tab → images.js and defaults.js → images.js) made it link a module twice now and then.

- 2026-10-16 (Codex, `tests`): restore the Linux overlay no-fade change from `4b3aa31`, confirmed by the user to fix the flicker. Preserve the previous fade version at `3dcd0f0` in the user-requested `with-fade` branch; Windows keeps its fade.

- 2026-10-15 (Claude, `tests`): articles in the formats Lookitan lays out itself (all but MDict and Slob, `Dictionary::own_look`) take the pop-up's look, like the AI card: its font, text and muted colours on the pop-up background, no invert filter (`html.lkt-themed` in protocol.rs ENTRY_STYLE). MDict/Slob keep a white page, inverted in the dark theme inside the frame (`html.lkt-own`). Yomitan tags are plain muted text and senses number inline, as in Lookupper.
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
