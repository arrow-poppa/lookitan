# Lookitan

Point at a word anywhere on screen (an app, a game, a video) and press a shortcut. Lookitan reads
the word with OCR and shows dictionary entries and an AI explanation in a pop-up next to it.

- **Fast local OCR:** PaddleOCR PP-OCRv6 on ONNX Runtime. It uses the GPU through DirectML on
  Windows and through WebGPU on Vulkan on Linux (experimental), falls back to the CPU, and only
  reads the line under the pointer.
- **Offline dictionaries:** MDict, Yomitan, StarDict, DSL, Aard2/Slob, CC-CEDICT, JMdict,
  JMnedict, KANJIDIC2, Kaikki, Webster/GCIDE XML and WordNet, with persistent indexes,
  styles, images and audio. See [formats and compatibility](docs/dictionary-engine.md).
- **Your own AI:** OpenAI, Gemini, DeepSeek or any OpenAI-compatible endpoint (OpenRouter,
  Ollama, LM Studio), with your key, your model, and your own prompt and system prompt.
- **Optional Google Lens OCR:** a cloud engine you can switch to in Settings › OCR.
- **Windows and Linux:** Windows 10/11, Linux X11, and KDE Plasma 6 on Wayland. Packages for
  Windows (`.exe`), Fedora and openSUSE (`.rpm`), Debian and Ubuntu (`.deb`), and any
  distribution (`.AppImage`).

Lookitan reproduces the behavior of the Lookupper app with its own code. It contains no code,
branding or assets from Lookupper.

## Install

Download the installer for your system from
[Releases](https://github.com/arrow-poppa/lookitan/releases), or the latest build from the
[Actions](https://github.com/arrow-poppa/lookitan/actions) tab (artifacts `lookitan-windows`,
`lookitan-linux`).

| System | File |
| --- | --- |
| Windows | `Lookitan_*_x64-setup.exe` |
| Fedora, openSUSE | `Lookitan-*.x86_64.rpm` (`sudo dnf install ./Lookitan-*.x86_64.rpm`) |
| Debian, Ubuntu, KDE neon | `lookitan_*_amd64.deb` |
| Other Linux | `Lookitan_*_amd64.AppImage` |

## Use

1. Open Settings from the tray icon and add a local dictionary or a ZIP under **Dictionaries**.
   Select the main file (`.mdx`, `.ifo`, `.dsl`, `index.json`, etc.) and keep its resources beside it.
2. Optionally, configure your AI provider under **AI**.
3. Point at a word and press the shortcut. The default is `Alt+D`.

In **Settings › OCR**, choose **Region around the cursor** to use the original lookup.
Select a region preset or set its width and height. **Original (900 × 260)** restores the
previous capture size. These settings are saved automatically. **Full screen** remains
available for the clickable overlay.

For **Anki**, enable the integration under **Settings › Anki**, select the deck and note type,
and map its fields (for example `{expression}` and `{glossary}`). The popup opens this setup
when it is incomplete and shows connection errors directly. AI word cards, explanations and
context translations respect the streaming option under **Dictionaries › AI word in context**.

In that panel, **Explicar automaticamente ao consultar** chooses whether the extra Explain
section opens automatically after the word card or waits for a click on **Explicar**.
The refresh button beside Explain regenerates that explanation without regenerating the card.
Completed AI responses are cached locally for 24 hours (up to 100 responses), including after
restarting the app. The key includes the word, sentence, provider, model and prompts; changing
streaming or automatic display switches does not discard responses. Different contexts need
different responses. Errors and cancelled/incomplete responses are never retained.

OCR detection and successfully recognized groups are reused for identical captured pixels and
OCR settings, in a small three-frame memory cache. Changing the text/frame triggers fresh OCR;
the cache does not retain additional screenshot copies. Dictionary record blocks are also cached.

### KDE Plasma on Wayland

Use the installed `.deb` or `.rpm` package and launch Lookitan from the application menu.
The KWin capture authorization comes from its installed desktop entry; the AppImage does not
currently provide that installation step. Capture/relay failures are shown explicitly instead
of falling back to an X11 capture of a Wayland desktop. The cursor relay keeps a heartbeat when
the pointer is stationary, and regions support monitors with negative desktop coordinates.

Wayland does not let apps read the global pointer position or grab global keys. Lookitan does
two things there:

- It loads a small KWin script that reports the pointer position over D-Bus.
- It captures the region through `org.kde.KWin.ScreenShot2`. KWin allows this without a dialog
  when Lookitan is installed from the `.rpm` or `.deb`, because the package's `.desktop` file
  declares the interface.

For the shortcut, add a custom one in **System Settings › Shortcuts › Add New › Command** that
runs:

```
lookitan --scan
```

## Build

Builds run on GitHub Actions (`.github/workflows/build.yml`). To build locally you need Node.js 22,
Rust stable and the [Tauri prerequisites](https://tauri.app/start/prerequisites/). Then:

```
python scripts/fetch-assets.py --platform windows   # or linux
npm ci
npx tauri icon app-icon.svg
npx tauri build
```

## License

GPL-3.0-only. See [NOTICE.md](NOTICE.md) for third-party components.
