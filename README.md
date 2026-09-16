# Ochco

Grab text from anywhere on your screen. Press a hotkey, drag over a video frame, an image or a
non-selectable UI, and the recognized text lands in your clipboard.

Recognition runs fully offline on [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) models
(PP-OCRv6 and PP-OCRv5) via ONNX Runtime. Supported languages: Russian, Ukrainian, Belarusian,
Bulgarian, English, Chinese, Japanese and 46 Latin-script languages (German, French, Spanish,
Polish, Czech and more). The language is detected automatically.

Runs on Windows and Linux (X11 and Wayland); macOS is planned.

## Getting started

Download the models (~42 MB, into `./models`):

```bash
uv run scripts/download_models.py
# or: python scripts/download_models.py  (Python 3.8+)
```

Build and run:

```bash
cargo run --release
```

ochco lives in the system tray. Press **Ctrl+Alt+T** (or click the tray icon), drag over the text,
and paste it anywhere. **Esc** or a right click cancels the selection.

To capture once without the tray app, e.g. from a shortcut configured in your desktop environment:

```bash
cargo run --release -- --capture
```

To recognize an image file without the GUI:

```bash
cargo run --release -- --image picture.png
```

## Linux

Build dependencies on Debian and Ubuntu:

```bash
sudo apt install pkg-config libclang-dev libpipewire-0.3-dev libxcb1-dev libxcb-randr0-dev libegl-dev libgbm-dev libwayland-dev
```

At runtime an X11 session also needs `libxkbcommon-x11-0`, which desktop installations already have.

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

## Models

Text detection uses one model for all languages; recognition models cover different scripts. By
default ochco runs every installed recognizer and keeps the best reading of each line. You can pin
the models instead:

```bash
cargo run --release -- --list-models
cargo run --release -- --det pp-ocrv5-mobile --rec pp-ocrv5-eslav
```

## Distribution

Ship the `ochco` executable together with the `models/` directory next to it. ONNX Runtime is linked
statically, so no other files are needed.

## License

This project is licensed under the [MIT License](LICENSE).

The PaddleOCR models are provided by PaddlePaddle under the Apache License 2.0.
