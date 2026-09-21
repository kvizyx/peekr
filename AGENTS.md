# AGENTS.md

This file provides guidance for LLM coding agents working with this repository (Peekr).

## Project overview

Peekr is an offline cross-platform OCR desktop application that can extract text from screen and images. Recognition runs on local [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) models
(PP-OCRv6 and PP-OCRv5) via ONNX Runtime.

This is a Rust project, it uses [egui](https://github.com/emilk/egui#why-immediate-mode) to implement the UI and
[orc](https://github.com/pykeio/ort) for PaddleOCR model inference.
