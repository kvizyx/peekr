<h1 align="center">Peekr</h1>

<p align="center">
  <img src="assets/icon.svg" width="160" alt="Peekr icon: a hand-drawn magnifying glass">
</p>

Peekr is an offline cross-platform OCR desktop application that can extract text from screen and images. Recognition runs on local [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) models
(PP-OCRv6 and PP-OCRv5) via ONNX Runtime.


## Development guide

This section outlines the rules, guidelines, and technical details of the project.
It is intended for those who wants to participate in the project's development.

### RTK

Our [AGENTS.md](AGENTS.md) provides hints on using [RTK](https://www.rtk-ai.app/docs) when it's possible
to optimize token usage, so if you use agents, I strongly recommend you to set up [RTK](https://www.rtk-ai.app/docs) in your local environment.

## License

This project is licensed under the [MIT License](LICENSE).

The PaddleOCR models are provided by PaddlePaddle under the Apache License 2.0. Release archives
include the licenses of the models, ONNX Runtime and all dependencies.
