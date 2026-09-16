# Ochco

An offline cross-platform OCR utility that can extract text from screen and images, powered by
PP-OCRv5 and PP-OCRv6 models via ONNX Runtime.

Recognition runs fully offline on [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) models
(PP-OCRv6 and PP-OCRv5) via ONNX Runtime. Supported languages: Russian, Ukrainian, Belarusian,
Bulgarian, English, Chinese, Japanese and 46 Latin-script languages (German, French, Spanish,
Polish, Czech and more). The language is detected automatically.

Runs on Windows and Linux (X11 and Wayland); macOS is planned.

## Installation

Download the archive for your system from
[Releases](https://github.com/kvizyx/ochco/releases), unpack it anywhere and run `ochco`. The
archive already contains the models, so everything works offline. Keep the `models` directory next
to the executable.

| System | Archive | Requirements |
| --- | --- | --- |
| Windows | `ochco-<version>-windows-x86_64.zip` | Windows 10 or 11, x64 |
| Linux | `ochco-<version>-linux-x86_64.tar.gz`<br>`ochco-<version>-linux-aarch64.tar.gz` | x86_64 or ARM64 with glibc 2.38+ (Ubuntu 24.04, Debian 13, Fedora 39 or newer) |

Windows SmartScreen may warn about an unrecognized app on first launch, because release builds are
not code-signed yet.

## Usage

ochco lives in the system tray. Press **Ctrl+Alt+T** (or click the tray icon), drag over the text,
and paste it anywhere. **Esc** or a right click cancels the selection.

```bash
ochco --capture             # capture once and exit, e.g. from a desktop shortcut
ochco --image picture.png   # recognize an image file and print the text
ochco --list-models         # show the models and the languages they cover
```

Text detection uses one model for all languages; recognition models cover different scripts. By
default ochco runs every installed recognizer and keeps the best reading of each line. You can pin
the models instead, e.g. `ochco --det pp-ocrv5-mobile --rec pp-ocrv5-eslav`.

### Linux notes

- **Hotkey.** The global hotkey works on X11 only. On Wayland, bind `ochco --capture` to a custom
  shortcut in your desktop settings. GNOME on X11 uses Ctrl+Alt+T for the terminal, so bind
  `--capture` there as well.
- **Tray icon.** Uses the StatusNotifierItem protocol (KDE, Cinnamon, XFCE, most tiling setups). On
  GNOME it needs the AppIndicator extension; without it ochco still runs and captures on the hotkey.
- **Screen capture on Wayland** goes through the GNOME Shell or xdg-desktop-portal screenshot APIs
  and needs XWayland to list monitors. Without a cursor position on Wayland, the primary monitor
  is captured.
- **Clipboard.** Linux clipboards live in the process that copied the text, so `--capture` keeps
  running in the background until something else is copied (a clipboard manager takes over
  right away).
- **Runtime libraries.** `libgbm` and, on X11, `libxkbcommon-x11` are needed; desktop installations
  already have them.

## Building from source

Download the models (~42 MB, into `./models`):

```bash
cargo xtask models
```

On Linux, install the build dependencies first (Debian and Ubuntu):

```bash
sudo apt install pkg-config libclang-dev libpipewire-0.3-dev libxcb1-dev libxcb-randr0-dev libegl-dev libgbm-dev libwayland-dev
```

Build and run:

```bash
cargo run --release
```

## Releasing

1. Bump `version` in `Cargo.toml`, commit and push.
2. Tag the commit with the same version and push the tag:

   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```

3. The [Release workflow](.github/workflows/release.yml) builds the Windows and Linux archives and
   attaches them to a draft release. Review it on GitHub, edit the notes and publish.

To check the archives before tagging, run the Release workflow manually (Actions → Release → Run
workflow): it builds everything and keeps the archives as run artifacts without creating a release.

To build the archive for your platform locally, run `cargo xtask dist` (needs
[cargo-about](https://github.com/EmbarkStudios/cargo-about): `cargo install cargo-about --features cli`).
The archive lands in `target/dist`.

## License

This project is licensed under the [MIT License](LICENSE).

The PaddleOCR models are provided by PaddlePaddle under the Apache License 2.0. Release archives
include the licenses of the models, ONNX Runtime and all Rust dependencies.
