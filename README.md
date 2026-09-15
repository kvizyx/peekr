# Ochco

Grab text from anywhere on your screen. Press a hotkey, drag over a video frame, an image or a
non-selectable UI, and the recognized text lands in your clipboard.

Recognition runs fully offline on [PP-OCRv5](https://github.com/PaddlePaddle/PaddleOCR) models via
ONNX Runtime. Currently supports Russian, Ukrainian, Belarusian, Bulgarian and English.

Windows only for now; macOS and Linux are planned.

## Getting started

Download the models (~13 MB, into `./models`):

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

To recognize an image file without the GUI:

```bash
cargo run --release -- --image picture.png
```

## Distribution

Ship `ochco.exe` together with the `models/` directory next to it. ONNX Runtime is linked statically,
so no other files are needed.

## License

This project is licensed under the [MIT License](LICENSE).

The PP-OCRv5 models are provided by PaddlePaddle under the Apache License 2.0.
