# Ochco

Cross-platform OCR utility that can extract text from screen and images. Recognition runs fully offline on [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) models
(PP-OCRv6 and PP-OCRv5) via ONNX Runtime.

## Installation

Download the archive for your system from
[Releases](https://github.com/kvizyx/ochco/releases), unpack it anywhere and run `ochco`. The
archive already contains the models, so everything works offline. Keep the `models` directory next
to the executable.

| System | Archive | Requirements |
| --- | --- | --- |
| Windows | `ochco-<version>-windows-x86_64.zip` | Windows 10 or 11, x64 |
| Linux | `ochco-<version>-linux-x86_64.tar.gz`<br>`ochco-<version>-linux-aarch64.tar.gz` | x86_64 or ARM64 with glibc 2.38+ (Ubuntu 24.04, Debian 13, Fedora 39 or newer) |

## Usage

Ochco lives in the system tray. Press **Win+Shift+O** (**Super+Shift+O** on Linux) or click the tray
icon and drag over the text. The recognized text appears next to the selection: press **Copy**
(or **Enter**, **Ctrl+C**) to put it on the clipboard, or drag again to select something else.
**Esc** or a right click closes the overlay.

To change the hotkey, open **Settings** from the tray menu. Settings are stored in
`%APPDATA%\ochco\config.toml` on Windows and `~/.config/ochco/config.toml` on Linux.

```bash
ochco --capture             # capture once and exit after copying, e.g. from a desktop shortcut
ochco --image picture.png   # recognize an image file and print the text
ochco --list-models         # show the models and the languages they cover
```

Text detection uses one model for all languages; recognition models cover different scripts. By
default ochco runs every installed recognizer and keeps the best reading of each line. You can pin
the models instead, e.g. `ochco --det pp-ocrv5-mobile --rec pp-ocrv5-eslav`.

### Linux notes

- **Hotkey.** The global hotkey works on X11 only. On Wayland, bind `ochco --capture` to a custom
  shortcut in your desktop settings.
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

## License

This project is licensed under the [MIT License](LICENSE).

The PaddleOCR models are provided by PaddlePaddle under the Apache License 2.0. Release archives
include the licenses of the models, ONNX Runtime and all Rust dependencies.
