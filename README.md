# Lookitan

Point at a word anywhere on screen (an app, a game, a video) and press a shortcut. Lookitan reads
the word with OCR and shows dictionary entries and an AI explanation in a pop-up next to it.

- **Fast local OCR:** PaddleOCR PP-OCRv6 on ONNX Runtime. It uses the GPU through DirectML on
  Windows and the CPU on Linux, and only reads the line under the pointer.
- **Offline dictionaries:** MDict (`.mdx` + `.mdd`), with styles, images and audio.
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

1. Open Settings from the tray icon and add a dictionary (`.mdx`) under **Dictionaries**.
2. Optionally, configure your AI provider under **AI**.
3. Point at a word and press the shortcut. The default is `Ctrl+Shift+Space`.

### KDE Plasma on Wayland

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
