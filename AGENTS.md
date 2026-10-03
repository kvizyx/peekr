# Peekr - AGENTS.md

This document helps AI agents to work with this codebase. It explains its purpose, patterns and workflows.

## Project overview

This is an offline cross-platform OCR desktop application. It can extract text from the screen and raw images. Recognition runs on local [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) models
(PP-OCRv6 and PP-OCRv5) via ONNX Runtime.

## Stack

- [Rust](https://rust-lang.org)
- [egui](https://github.com/emilk/egui) - GUI
- [ort](https://github.com/pykeio/ort) - Inference for ONNX our models
