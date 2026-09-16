#!/usr/bin/env python3
# /// script
# requires-python = ">=3.8"
# dependencies = []
# ///
"""Downloads the PaddleOCR ONNX models (Apache 2.0) into ./models.

Detection (shared by all languages):
  pp-ocrv5-mobile   PP-OCRv5 mobile
  pp-ocrv6-small    PP-OCRv6 small

Recognition:
  pp-ocrv5-eslav    Russian, Ukrainian, Belarusian, Bulgarian, English
  pp-ocrv6-small    Chinese, Japanese, English and 46 Latin-script languages

Sources: RapidOCR model zoo on ModelScope, PaddleOCR on GitHub, PaddlePaddle on Hugging Face.
Uses only the standard library, requires Python 3.8+.
"""

import argparse
import hashlib
import sys
import urllib.request
from pathlib import Path
from typing import List, NamedTuple, Optional

RAPID = "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv5"
PADDLE_DICT = (
    "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/"
    "0a8a6354f10388ecd601f9a86639dd3c44d95057/ppocr/utils/dict"
)
HF_V6_DET = "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_det_onnx/resolve/28fe5895c24fd108c19eb3e8479f4ab385fbfc62"
HF_V6_REC = "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_rec_onnx/resolve/b8f84f0b80c529de40b4fbb3544b84fa7233a513"

CHUNK = 1 << 16


class File(NamedTuple):
    url: str
    path: str
    sha256: str
    # PaddlePaddle ships the character dictionary inside the inference config;
    # when set, the dictionary is extracted from this file into `dict_path`.
    dict_path: Optional[str] = None


FILES = [
    File(
        f"{RAPID}/det/ch_PP-OCRv5_det_mobile.onnx",
        "det/pp-ocrv5-mobile/model.onnx",
        "4d97c44a20d30a81aad087d6a396b08f786c4635742afc391f6621f5c6ae78ae",
    ),
    File(
        f"{HF_V6_DET}/inference.onnx",
        "det/pp-ocrv6-small/model.onnx",
        "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e",
    ),
    File(
        f"{RAPID}/rec/eslav_PP-OCRv5_rec_mobile.onnx",
        "rec/pp-ocrv5-eslav/model.onnx",
        "08705d6721849b1347d26187f15a5e362c431963a2a62bfff4feac578c489aab",
    ),
    File(
        f"{PADDLE_DICT}/ppocrv5_eslav_dict.txt",
        "rec/pp-ocrv5-eslav/dict.txt",
        "3e95f1581557162870cacdba5af91a4c6be2890710d395b0c3c7578e7ee5e6eb",
    ),
    File(
        f"{HF_V6_REC}/inference.onnx",
        "rec/pp-ocrv6-small/model.onnx",
        "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634",
    ),
    File(
        f"{HF_V6_REC}/inference.yml",
        "rec/pp-ocrv6-small/inference.yml",
        "ab078671bb49f06228eadccd34f1bb501e157f7a047095ffb943ba81512c77d1",
        dict_path="rec/pp-ocrv6-small/dict.txt",
    ),
]


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


def parse_yaml_scalar(raw: str) -> str:
    """Parses the scalar forms used in PaddleOCR dictionaries: plain and single-quoted."""
    if len(raw) >= 2 and raw[0] == raw[-1] == "'":
        return raw[1:-1].replace("''", "'")

    if raw.startswith('"'):
        raise ValueError(f"double-quoted YAML scalars are not supported: {raw}")

    return raw


def extract_dictionary(config: Path) -> List[str]:
    """Reads the `PostProcess.character_dict` list from a PaddleOCR `inference.yml`."""
    symbols: List[str] = []
    in_dict = False

    for line in config.read_text(encoding="utf-8").splitlines():
        if line.strip() == "character_dict:":
            in_dict = True
            continue

        if not in_dict:
            continue

        # Dictionary items are indented list entries; anything else ends the list.
        if not line.startswith("  - "):
            break
        symbols.append(parse_yaml_scalar(line[len("  - "):]))

    if not symbols:
        raise ValueError(f"no character_dict found in {config}")

    return symbols


def write_dictionary(config: Path, dest: Path) -> None:
    symbols = extract_dictionary(config)

    if any("\n" in s for s in symbols):
        raise ValueError("dictionary symbols must not contain newlines")

    with dest.open("w", encoding="utf-8", newline="\n") as out:
        out.write("\n".join(symbols) + "\n")
    print(f"  extracted {len(symbols)} symbols into {dest.name}")


def fetch(file: File, models_dir: Path) -> None:
    dest = models_dir / file.path

    if dest.exists() and sha256_of(dest) == file.sha256:
        print(f"skip  {file.path} (up to date)")
        # Leftover from an interrupted earlier run.
        dest.with_name(dest.name + ".part").unlink(missing_ok=True)
    else:
        if dest.exists():
            print(f"stale {file.path} (checksum mismatch), downloading again")
        print(f"fetch {file.path}")
        download(file.url, dest, file.sha256)

    if file.dict_path is not None:
        write_dictionary(dest, models_dir / file.dict_path)


def main() -> int:
    default_dir = Path(__file__).resolve().parent.parent / "models"
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--models-dir", type=Path, default=default_dir, help=f"target directory (default: {default_dir})")
    args = parser.parse_args()

    failed = False
    for file in FILES:
        try:
            fetch(file, args.models_dir)
        except Exception as e:  # keep going so one broken mirror does not block the rest
            sys.stdout.flush()  # keep the error after the lines that led to it when output is piped
            print(f"error {file.path}: {e}", file=sys.stderr, flush=True)
            failed = True

    if failed:
        return 1

    print(f"done: {args.models_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
