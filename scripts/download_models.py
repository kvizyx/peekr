#!/usr/bin/env python3
# /// script
# requires-python = ">=3.8"
# dependencies = []
# ///
"""Downloads PP-OCRv5 ONNX models (Apache 2.0) into ./models.

Sources: RapidOCR model zoo on ModelScope, dictionaries from PaddleOCR on GitHub.
Uses only the standard library, requires Python 3.8+.
"""

import argparse
import hashlib
import sys
import urllib.request
from pathlib import Path

RAPID = "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv5"
PADDLE_DICT = (
    "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/"
    "0a8a6354f10388ecd601f9a86639dd3c44d95057/ppocr/utils/dict"
)

# (url, path inside the models directory, sha256)
FILES = [
    (
        f"{RAPID}/det/ch_PP-OCRv5_det_mobile.onnx",
        "det/det.onnx",
        "4d97c44a20d30a81aad087d6a396b08f786c4635742afc391f6621f5c6ae78ae",
    ),
    (
        f"{RAPID}/rec/eslav_PP-OCRv5_rec_mobile.onnx",
        "rec/eslav/rec.onnx",
        "08705d6721849b1347d26187f15a5e362c431963a2a62bfff4feac578c489aab",
    ),
    (
        f"{PADDLE_DICT}/ppocrv5_eslav_dict.txt",
        "rec/eslav/dict.txt",
        "3e95f1581557162870cacdba5af91a4c6be2890710d395b0c3c7578e7ee5e6eb",
    ),
]

CHUNK = 1 << 16


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(CHUNK), b""):
            digest.update(chunk)
    return digest.hexdigest()


def format_size(size: int) -> str:
    if size < 1 << 20:
        return f"{size / 1024:.1f} KB"
    return f"{size / (1 << 20):.1f} MB"


def download(url: str, dest: Path, expected_sha: str) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    # Write to a temporary file so an interrupted download never looks complete.
    part = dest.with_name(dest.name + ".part")
    request = urllib.request.Request(url, headers={"User-Agent": "ochco-model-downloader"})
    digest = hashlib.sha256()
    # A live progress line only makes sense in a terminal; in logs it would be one line per chunk.
    interactive = sys.stdout.isatty()
    with urllib.request.urlopen(request, timeout=60) as response, part.open("wb") as out:
        total = int(response.headers.get("Content-Length") or 0)
        done = 0
        for chunk in iter(lambda: response.read(CHUNK), b""):
            out.write(chunk)
            digest.update(chunk)
            done += len(chunk)
            if interactive:
                progress = f"{format_size(done)} / {format_size(total)}" if total else format_size(done)
                print(f"\r  {progress}   ", end="", flush=True)
    if interactive:
        print("\r", end="")
    print(f"  {format_size(done)}")

    actual = digest.hexdigest()
    if actual != expected_sha:
        part.unlink()
        raise RuntimeError(f"SHA256 mismatch for {dest.name}: expected {expected_sha}, got {actual}")
    part.replace(dest)


def main() -> int:
    default_dir = Path(__file__).resolve().parent.parent / "models"
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--models-dir", type=Path, default=default_dir, help=f"target directory (default: {default_dir})")
    args = parser.parse_args()

    failed = False
    for url, rel_path, sha in FILES:
        dest = args.models_dir / rel_path
        if dest.exists():
            if sha256_of(dest) == sha:
                print(f"skip  {rel_path} (up to date)")
                # Leftover from an interrupted earlier run.
                dest.with_name(dest.name + ".part").unlink(missing_ok=True)
                continue
            print(f"stale {rel_path} (checksum mismatch), downloading again")
        print(f"fetch {rel_path}")
        try:
            download(url, dest, sha)
        except Exception as e:  # keep going so one broken mirror does not block the rest
            sys.stdout.flush()  # keep the error after the lines that led to it when output is piped
            print(f"error {rel_path}: {e}", file=sys.stderr, flush=True)
            failed = True

    if failed:
        return 1
    print(f"done: {args.models_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
