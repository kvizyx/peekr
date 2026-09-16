# Ochco

Grab text from anywhere on your screen. Press a hotkey, drag over a video frame, an image or a
non-selectable UI, and the recognized text lands in your clipboard.

Recognition runs fully offline on [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) models
(PP-OCRv6 and PP-OCRv5) via ONNX Runtime. Supported languages: Russian, Ukrainian, Belarusian,
Bulgarian, English, Chinese, Japanese and 46 Latin-script languages (German, French, Spanish,
Polish, Czech and more). The language is detected automatically.

Windows only for now; macOS and Linux are planned.

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

To recognize an image file without the GUI:

```bash
cargo run --release -- --image picture.png
```

## Models

Text detection uses one model for all languages; recognition models cover different scripts. By
default ochco runs every installed recognizer and keeps the best reading of each line. You can pin
the models instead:

```bash
cargo run --release -- --list-models
cargo run --release -- --det pp-ocrv5-mobile --rec pp-ocrv5-eslav
```

## Distribution

Ship `ochco.exe` together with the `models/` directory next to it. ONNX Runtime is linked statically,
so no other files are needed.

## License

This project is licensed under the [MIT License](LICENSE).

The PaddleOCR models are provided by PaddlePaddle under the Apache License 2.0.
