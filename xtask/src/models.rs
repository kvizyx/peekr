//! `cargo xtask models`: downloads the PaddleOCR ONNX models (Apache 2.0) into `models/`.
//!
//! Layout: `det/<id>/model.onnx` and `rec/<id>/model.onnx` + `dict.txt`, as expected by the app.
//! Every file is pinned to a revision and verified by SHA-256.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

macro_rules! rapidocr {
    ($path:literal) => {
        concat!(
            "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv5/",
            $path
        )
    };
}

macro_rules! paddleocr {
    ($path:literal) => {
        concat!(
            "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/0a8a6354f10388ecd601f9a86639dd3c44d95057/",
            $path
        )
    };
}

macro_rules! paddle_hub {
    ($repo:literal, $revision:literal, $file:literal) => {
        concat!(
            "https://huggingface.co/PaddlePaddle/",
            $repo,
            "/resolve/",
            $revision,
            "/",
            $file
        )
    };
}

struct ModelFile {
    url: &'static str,
    /// Path inside the models directory.
    path: &'static str,
    sha256: &'static str,
    /// PaddlePaddle ships the character dictionary inside `inference.yml`; when set, it is
    /// extracted into this path.
    dictionary: Option<&'static str>,
}

const FILES: &[ModelFile] = &[
    ModelFile {
        url: rapidocr!("det/ch_PP-OCRv5_det_mobile.onnx"),
        path: "det/pp-ocrv5-mobile/model.onnx",
        sha256: "4d97c44a20d30a81aad087d6a396b08f786c4635742afc391f6621f5c6ae78ae",
        dictionary: None,
    },
    ModelFile {
        url: paddle_hub!(
            "PP-OCRv6_small_det_onnx",
            "28fe5895c24fd108c19eb3e8479f4ab385fbfc62",
            "inference.onnx"
        ),
        path: "det/pp-ocrv6-small/model.onnx",
        sha256: "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e",
        dictionary: None,
    },
    ModelFile {
        url: rapidocr!("rec/eslav_PP-OCRv5_rec_mobile.onnx"),
        path: "rec/pp-ocrv5-eslav/model.onnx",
        sha256: "08705d6721849b1347d26187f15a5e362c431963a2a62bfff4feac578c489aab",
        dictionary: None,
    },
    ModelFile {
        url: paddleocr!("ppocr/utils/dict/ppocrv5_eslav_dict.txt"),
        path: "rec/pp-ocrv5-eslav/dict.txt",
        sha256: "3e95f1581557162870cacdba5af91a4c6be2890710d395b0c3c7578e7ee5e6eb",
        dictionary: None,
    },
    ModelFile {
        url: paddle_hub!(
            "PP-OCRv6_small_rec_onnx",
            "b8f84f0b80c529de40b4fbb3544b84fa7233a513",
            "inference.onnx"
        ),
        path: "rec/pp-ocrv6-small/model.onnx",
        sha256: "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634",
        dictionary: None,
    },
    ModelFile {
        url: paddle_hub!(
            "PP-OCRv6_small_rec_onnx",
            "b8f84f0b80c529de40b4fbb3544b84fa7233a513",
            "inference.yml"
        ),
        path: "rec/pp-ocrv6-small/inference.yml",
        sha256: "ab078671bb49f06228eadccd34f1bb501e157f7a047095ffb943ba81512c77d1",
        dictionary: Some("rec/pp-ocrv6-small/dict.txt"),
    },
    // Apache 2.0 requires shipping the license text together with the models.
    ModelFile {
        url: paddleocr!("LICENSE"),
        path: "LICENSE.txt",
        sha256: "3840c5c0c61c294264d2dd77b8777be6ddd90121ef4e0e64abcd22edea581d6e",
        dictionary: None,
    },
];

const CHUNK_SIZE: usize = 1 << 16;

pub fn download(models_dir: &Path) -> Result<()> {
    let mut failed = 0;

    // Keep going after a failure, so one unavailable mirror does not block the other files.
    for file in FILES {
        if let Err(e) = fetch(file, models_dir) {
            eprintln!("error {}: {e:#}", file.path);
            failed += 1;
        }
    }

    if failed > 0 {
        bail!("{failed} of {} files failed", FILES.len());
    }

    eprintln!("models are ready in {}", models_dir.display());
    Ok(())
}

fn fetch(file: &ModelFile, models_dir: &Path) -> Result<()> {
    let dest = models_dir.join(file.path);

    if dest.is_file() && sha256_of_file(&dest)? == file.sha256 {
        eprintln!("skip  {} (up to date)", file.path);
    } else {
        eprintln!("fetch {}", file.path);
        download_verified(file.url, &dest, file.sha256)?;
    }

    if let Some(dictionary) = file.dictionary {
        let symbols = extract_dictionary(&fs::read_to_string(&dest)?)?;
        fs::write(models_dir.join(dictionary), symbols.join("\n") + "\n")?;
        eprintln!("      extracted {} symbols into {dictionary}", symbols.len());
    }

    Ok(())
}

/// Downloads into a `.part` file and moves it into place only after the checksum matches,
/// so an interrupted or corrupted download never looks complete.
fn download_verified(url: &str, dest: &Path, expected_sha256: &str) -> Result<()> {
    let part = dest.with_extension("part");
    fs::create_dir_all(dest.parent().context("destination has no parent directory")?)?;

    let response = ureq::get(url)
        .header("User-Agent", "peekr-xtask")
        .call()
        .with_context(|| format!("requesting {url}"))?;
    let mut body = response.into_body().into_reader();

    let mut out = BufWriter::new(File::create(&part)?);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; CHUNK_SIZE];
    let mut size = 0;

    loop {
        let read = body.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        out.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        size += read;
    }
    out.flush()?;
    drop(out);

    let actual = hex(&hasher.finalize());
    if actual != expected_sha256 {
        fs::remove_file(&part)?;
        bail!("SHA-256 mismatch: expected {expected_sha256}, got {actual}");
    }

    fs::rename(&part, dest)?;
    eprintln!("      {}", format_size(size));

    Ok(())
}

fn format_size(bytes: usize) -> String {
    let bytes = bytes as f64;

    if bytes < f64::from(1 << 20) {
        format!("{:.1} KB", bytes / 1024.0)
    } else {
        format!("{:.1} MB", bytes / f64::from(1 << 20))
    }
}

fn sha256_of_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; CHUNK_SIZE];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        hasher.update(&buffer[..read]);
    }

    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            // Writing into a String cannot fail.
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Reads the `character_dict` list from a PaddleOCR `inference.yml`. The dictionary entries use
/// only plain and single-quoted YAML scalars, so a full YAML parser is not needed.
fn extract_dictionary(config: &str) -> Result<Vec<String>> {
    let entries = config
        .lines()
        .skip_while(|line| line.trim() != "character_dict:")
        .skip(1)
        .map_while(|line| line.strip_prefix("  - "));

    let symbols = entries.map(parse_scalar).collect::<Result<Vec<_>>>()?;
    if symbols.is_empty() {
        bail!("no character_dict in the model config");
    }

    Ok(symbols)
}

fn parse_scalar(raw: &str) -> Result<String> {
    if let Some(quoted) = raw.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')) {
        return Ok(quoted.replace("''", "'"));
    }

    if raw.starts_with('"') {
        bail!("double-quoted YAML scalars are not supported: {raw}");
    }

    Ok(raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_and_quoted_symbols() {
        let config =
            "PostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - '!'\n  - ''''\n  - a\n  - 日\nNext: 1\n";

        let symbols = extract_dictionary(config).expect("valid dictionary");

        assert_eq!(symbols, ["!", "'", "a", "日"]);
    }

    #[test]
    fn missing_dictionary_is_an_error() {
        assert!(extract_dictionary("PostProcess:\n  name: CTCLabelDecode\n").is_err());
    }

    #[test]
    fn hex_encodes_bytes() {
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
    }
}
