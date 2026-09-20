# AGENTS.md

This file provides guidance for LLM coding agents working with this repository (Peekr).

## Project overview

Peekr is an offline cross-platform OCR desktop application that can extract text from screen and images. Recognition runs on local [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) models
(PP-OCRv6 and PP-OCRv5) via ONNX Runtime.

This is a Rust project, it uses [egui](https://github.com/emilk/egui#why-immediate-mode) to implement the UI and
[orc](https://github.com/pykeio/ort) for PaddleOCR model inference.


## Command usage

To optimize tokens usage, use [RTK](https://github.com/rtk-ai/rtk) (if available) to compress command output.

### Environment check

Before using RTK, verify whether it's available in the current environment with two commands:

- `rtk --version` (should show `rtk <version>`)

- `rtk gain` (should show the savings dashboard or a message stating that there is no tracking data yet, if it points to
  a different tool or stating that command not found, treat it as unavailable too)

If RTK is not available, fall back seamlessly to standard command usage without failing the task
or attempting to install RTK.

If RTK is available, you must follow [RTK.md](RTK.md) for command usage and preferred commands.