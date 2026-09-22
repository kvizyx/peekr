//! Hashing a file or a stream, or a download while it lands on disk, without writing the same
//! read loop for each.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

const CHUNK_SIZE: usize = 1 << 16;

/// The SHA-256 of a stream, hex-encoded.
pub fn sha256(reader: impl Read) -> Result<String> {
    copy_and_hash(reader, std::io::sink()).map(|(_, sha256)| sha256)
}

/// The SHA-256 of a file on disk, hex-encoded.
pub fn sha256_of_file(path: &Path) -> Result<String> {
    sha256(File::open(path).with_context(|| format!("opening {}", path.display()))?)
}

/// Copies a stream to a writer while hashing it, returning the byte count and the hex SHA-256.
///
/// `writer` takes `impl Write` rather than `&mut impl Write` so that a temporary sink works too;
/// passing `&mut destination` still leaves the caller holding it afterwards, since `&mut W`
/// implements `Write` whenever `W` does.
pub fn copy_and_hash(mut reader: impl Read, mut writer: impl Write) -> Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; CHUNK_SIZE];
    let mut size = 0u64;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        writer.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        size += read as u64;
    }

    Ok((size, hex::encode(hasher.finalize())))
}
