//! `cargo xtask dist`: builds a release archive for the current platform.
//!
//! The archive `target/dist/peekr-<version>-<os>-<arch>` (.zip on Windows, .tar.gz elsewhere)
//! contains a directory of the same name with the executable, the models, docs and license
//! files. The app finds `models/` next to its executable, so it runs right after unpacking.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use zip::write::SimpleFileOptions;

const PACKAGE: &str = "peekr";
/// Project files copied into the root of the archive.
const DOCS: &[&str] = &["README.md", "LICENSE"];
/// License files that are not generated, relative to the project root.
const ONNXRUNTIME_LICENSE: &str = "xtask/licenses/onnxruntime-LICENSE.txt";

/// A file to pack: where it comes from and where it goes inside the archive.
struct Entry {
    source: PathBuf,
    name: String,
    executable: bool,
}

pub fn package(root: &Path) -> Result<()> {
    let version = package_version(root)?;
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let prefix = format!("{PACKAGE}-{version}-{platform}");

    let target_dir = crate::target_dir(root);
    let dist_dir = target_dir.join("dist");
    fs::create_dir_all(&dist_dir)?;

    eprintln!("building {PACKAGE} {version} in release mode");
    run(cargo()
        .args(["build", "--release", "--locked", "--package", PACKAGE])
        .current_dir(root))?;

    let third_party_licenses = dist_dir.join("THIRD-PARTY-LICENSES.html");
    generate_third_party_licenses(root, &third_party_licenses)?;

    let binary = target_dir
        .join("release")
        .join(format!("{PACKAGE}{}", std::env::consts::EXE_SUFFIX));
    let entries = collect_entries(root, binary, third_party_licenses)?;

    let archive = if cfg!(windows) {
        let archive = dist_dir.join(format!("{prefix}.zip"));
        write_zip(&archive, &prefix, &entries)?;
        archive
    } else {
        let archive = dist_dir.join(format!("{prefix}.tar.gz"));
        write_tar_gz(&archive, &prefix, &entries)?;
        archive
    };

    let size = fs::metadata(&archive)?.len() as f64 / f64::from(1 << 20);
    eprintln!(
        "packed {} files into {} ({size:.1} MB)",
        entries.len(),
        archive.display()
    );

    Ok(())
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")))
}

fn run(command: &mut Command) -> Result<()> {
    let status = command.status().with_context(|| format!("starting {command:?}"))?;

    if !status.success() {
        bail!("{command:?} failed with {status}");
    }

    Ok(())
}

/// Reads `version` from the `[package]` section of the project's `Cargo.toml`.
fn package_version(root: &Path) -> Result<String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml"))?;

    let version = manifest
        .lines()
        .skip_while(|line| line.trim() != "[package]")
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .find_map(|line| line.strip_prefix("version = \"")?.strip_suffix('"'));

    version
        .map(str::to_owned)
        .context("no version in the [package] section of Cargo.toml")
}

fn generate_third_party_licenses(root: &Path, output: &Path) -> Result<()> {
    eprintln!("collecting third-party licenses");

    let licenses_dir = root.join("xtask").join("licenses");
    let mut command = cargo();
    command
        .args(["about", "generate", "--locked", "--fail", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .arg("--config")
        .arg(licenses_dir.join("about.toml"))
        .arg("--output-file")
        .arg(output)
        .arg(licenses_dir.join("about.hbs"));

    run(&mut command).context("cargo-about is required: cargo install cargo-about --features cli")
}

fn collect_entries(root: &Path, binary: PathBuf, third_party_licenses: PathBuf) -> Result<Vec<Entry>> {
    let mut entries = vec![Entry {
        source: binary,
        name: format!("{PACKAGE}{}", std::env::consts::EXE_SUFFIX),
        executable: true,
    }];

    let models_dir = root.join("models");
    let models = model_files(&models_dir).context("the models are missing; run `cargo xtask models`")?;
    entries.extend(models.into_iter().map(|(source, name)| Entry {
        source,
        name,
        executable: false,
    }));

    for doc in DOCS {
        entries.push(Entry {
            source: root.join(doc),
            name: (*doc).to_owned(),
            executable: false,
        });
    }
    entries.push(Entry {
        source: root.join(ONNXRUNTIME_LICENSE),
        name: "licenses/onnxruntime-LICENSE.txt".to_owned(),
        executable: false,
    });
    entries.push(Entry {
        source: third_party_licenses,
        name: "licenses/THIRD-PARTY-LICENSES.html".to_owned(),
        executable: false,
    });

    if let Some(missing) = entries.iter().find(|entry| !entry.source.is_file()) {
        bail!("missing {}", missing.source.display());
    }

    Ok(entries)
}

/// The files the app needs at runtime: every model, its dictionary and the model license.
/// Download leftovers such as `inference.yml` are skipped.
fn model_files(models_dir: &Path) -> Result<Vec<(PathBuf, String)>> {
    let mut files = vec![(models_dir.join("LICENSE.txt"), "models/LICENSE.txt".to_owned())];

    for kind in ["det", "rec"] {
        let mut found = false;

        for model in sorted_dirs(&models_dir.join(kind))? {
            let id = model
                .file_name()
                .context("model directory without a name")?
                .to_string_lossy()
                .into_owned();

            for file in ["model.onnx", "dict.txt"] {
                let source = model.join(file);
                if source.is_file() {
                    files.push((source, format!("models/{kind}/{id}/{file}")));
                    found |= file == "model.onnx";
                }
            }
        }

        if !found {
            bail!("no {kind} model in {}", models_dir.display());
        }
    }

    Ok(files)
}

fn sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()?;

    dirs.retain(|path| path.is_dir());
    dirs.sort();

    Ok(dirs)
}

fn write_zip(archive: &Path, prefix: &str, entries: &[Entry]) -> Result<()> {
    let mut zip = zip::ZipWriter::new(BufWriter::new(File::create(archive)?));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(9));

    for entry in entries {
        zip.start_file(format!("{prefix}/{}", entry.name), options)?;
        io::copy(&mut File::open(&entry.source)?, &mut zip)?;
    }

    zip.finish()?;
    Ok(())
}

fn write_tar_gz(archive: &Path, prefix: &str, entries: &[Entry]) -> Result<()> {
    let encoder = GzEncoder::new(BufWriter::new(File::create(archive)?), Compression::best());
    let mut tar = tar::Builder::new(encoder);

    for entry in entries {
        let metadata = fs::metadata(&entry.source)?;
        let modified = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_secs();

        // Set the header explicitly: permissions of a checkout (e.g. from Windows) mean nothing.
        let mut header = tar::Header::new_gnu();
        header.set_size(metadata.len());
        header.set_mode(if entry.executable { 0o755 } else { 0o644 });
        header.set_mtime(modified);
        header.set_cksum();

        tar.append_data(
            &mut header,
            format!("{prefix}/{}", entry.name),
            File::open(&entry.source)?,
        )?;
    }

    tar.into_inner()?.finish()?;
    Ok(())
}
