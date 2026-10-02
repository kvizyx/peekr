# Peekr - AGENTS.md

This document helps AI agents to work with this codebase. It explains its purpose, patterns and workflow.

## Project overview

This is an offline cross-platform OCR desktop application written in Rust with [egui](https://github.com/emilk/egui#why-immediate-mode)
and [orc](https://github.com/pykeio/ort) in its core. It can extract text from the screen and raw images. Recognition runs on local [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) models
(PP-OCRv6 and PP-OCRv5) via ONNX Runtime.
