//! `cargo xtask dist`: builds a release archive for the current platform.
//!
//! The archive `target/dist/peekr-<version>-<os>-<arch>` (.zip on Windows, .tar.gz elsewhere)
//! contains a directory of the same name with the executable, the models, docs and license
//! files. The app finds `models/` next to its executable, so it runs right after unpacking.
//!
//! `target/dist/update/` holds the same files once more, gzip-compressed one by one, next to a
//! manifest of what they are. The app's updater downloads only the ones that differ from the
//! files it already has; see `src/update.rs`.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Serialize;
use zip::write::SimpleFileOptions;

use crate::{hash, process};

const PACKAGE: &str = "peekr";
/// Project files copied into the root of the archive.
const DOCS: &[&str] = &["README.md", "LICENSE"];
/// License files that are not generated, relative to the project root.
const ONNXRUNTIME_LICENSE: &str = "xtask/licenses/onnxruntime-LICENSE.txt";
/// Everything the updater uses is named with this prefix, so that the release page keeps it
/// apart from the archives people download by hand.
const UPDATE_PREFIX: &str = "update";

/// A file to pack: where it comes from and where it goes inside the archive.
struct Entry {
    source: PathBuf,
    name: String,
    executable: bool,
}

/// What a release consists of, read by the app's updater. Mirrors `Manifest` in `src/update.rs`.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Manifest {
    version: String,
    files: Vec<ManifestFile>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct ManifestFile {
    path: String,
    sha256: String,
    /// Of the compressed download, which is what the app's progress bar measures.
    size: u64,
    executable: bool,
    url: String,
}

pub fn package(root: &Path) -> Result<()> {
    let build = build(root)?;
    write_update_files(root, &build)?;

    let archive = if cfg!(windows) {
        let archive = build.dist_dir.join(format!("{}.zip", build.prefix));
        write_zip(&archive, &build.prefix, &build.entries)?;
        archive
    } else {
        let archive = build.dist_dir.join(format!("{}.tar.gz", build.prefix));
        write_tar_gz(&archive, &build.prefix, &build.entries)?;
        archive
    };

    let size = fs::metadata(&archive)?.len() as f64 / f64::from(1 << 20);
    eprintln!(
        "packed {} files into {} ({size:.1} MB)",
        build.entries.len(),
        archive.display()
    );

    Ok(())
}

/// A release build, with everything that goes into a package.
pub struct Build {
    pub version: String,
    /// The operating system and architecture the build is for, as `windows-x86_64`.
    platform: String,
    pub prefix: String,
    pub dist_dir: PathBuf,
    entries: Vec<Entry>,
}

impl Build {
    /// Copies the files into `target/dist/<prefix>/`, the layout they are installed in.
    pub fn stage(&self) -> Result<PathBuf> {
        let stage_dir = self.dist_dir.join(&self.prefix);
        if stage_dir.exists() {
            fs::remove_dir_all(&stage_dir).with_context(|| format!("clearing {}", stage_dir.display()))?;
        }

        for entry in &self.entries {
            let destination = stage_dir.join(&entry.name);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }

            fs::copy(&entry.source, &destination).with_context(|| format!("copying {}", entry.source.display()))?;
        }

        eprintln!("staged {} files in {}", self.entries.len(), stage_dir.display());

        Ok(stage_dir)
    }
}

pub fn build(root: &Path) -> Result<Build> {
    let version = package_version(root)?;
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let prefix = format!("{PACKAGE}-{version}-{platform}");

    let target_dir = crate::target_dir(root);
    let dist_dir = target_dir.join("dist");
    fs::create_dir_all(&dist_dir)?;

    eprintln!("building {PACKAGE} {version} in release mode");
    process::run(
        cargo()
            .args(["build", "--release", "--locked", "--package", PACKAGE])
            .current_dir(root),
    )?;

    let third_party_licenses = dist_dir.join("THIRD-PARTY-LICENSES.html");
    generate_third_party_licenses(root, &third_party_licenses)?;

    let binary = target_dir
        .join("release")
        .join(format!("{PACKAGE}{}", std::env::consts::EXE_SUFFIX));
    let entries = collect_entries(root, binary, third_party_licenses)?;

    Ok(Build {
        version,
        platform,
        prefix,
        dist_dir,
        entries,
    })
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")))
}

/// Reads `version` from the `[package]` section of the project's `Cargo.toml`.
pub fn package_version(root: &Path) -> Result<String> {
    package_field(root, "version")
}

/// Reads a string field from the `[package]` section of the project's `Cargo.toml`.
fn package_field(root: &Path, field: &str) -> Result<String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml"))?;
    let prefix = format!("{field} = \"");

    let value = manifest
        .lines()
        .skip_while(|line| line.trim() != "[package]")
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .find_map(|line| line.strip_prefix(&prefix)?.strip_suffix('"'));

    value
        .map(str::to_owned)
        .with_context(|| format!("no {field} in the [package] section of Cargo.toml"))
}

/// Writes what the updater needs into `target/dist/update/`: every file of the release on its
/// own, gzip-compressed, and a manifest saying where each one goes and what it hashes to.
///
/// The file names are the names they are uploaded to the GitHub release under, which is what
/// makes the URLs in the manifest predictable.
fn write_update_files(root: &Path, build: &Build) -> Result<()> {
    let repository = package_field(root, "repository")?;
    let platform = &build.platform;

    let update_dir = build.dist_dir.join(UPDATE_PREFIX);
    fs::create_dir_all(&update_dir)?;

    let mut files = Vec::with_capacity(build.entries.len());

    for entry in &build.entries {
        // The name has to be unique across the whole release, where every platform's files meet.
        let asset = format!("{UPDATE_PREFIX}-{platform}-{}.gz", entry.name.replace('/', "-"));
        let compressed = update_dir.join(&asset);
        let sha256 = compress(&entry.source, &compressed)?;

        files.push(ManifestFile {
            path: entry.name.clone(),
            sha256,
            size: fs::metadata(&compressed)?.len(),
            executable: entry.executable,
            url: format!("{repository}/releases/download/v{}/{asset}", build.version),
        });
    }

    let manifest = Manifest {
        version: build.version.clone(),
        files,
    };

    let path = update_dir.join(format!("{UPDATE_PREFIX}-{platform}.toml"));
    fs::write(&path, toml::to_string(&manifest)?)?;
    crate::signing::sign(&path)?;

    eprintln!("wrote {} update files and {}", manifest.files.len(), path.display());

    Ok(())
}

/// Gzip-compresses a file, returning the SHA-256 of its contents. The updater checks the file it
/// unpacks, not the download, so that a re-compression can never look like a corrupted release.
fn compress(source: &Path, destination: &Path) -> Result<String> {
    let input = File::open(source).with_context(|| format!("opening {}", source.display()))?;
    let mut output = GzEncoder::new(BufWriter::new(File::create(destination)?), Compression::best());

    let (_, sha256) = hash::copy_and_hash(input, &mut output)?;
    output.finish()?.flush()?;

    Ok(sha256)
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

    process::run(&mut command).context("cargo-about is required: cargo install cargo-about --features cli")
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
