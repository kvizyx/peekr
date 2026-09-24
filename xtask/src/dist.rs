//! `cargo xtask dist`: builds a release of peekr for this platform into `target/dist/release/`,
//! which is what goes up on the GitHub release.
//!
//! Velopack's `vpk` packs it into an installation that updates itself:
//!
//! - on Windows, `Setup.exe` and a portable zip;
//! - on Linux, an `AppImage`;
//! - everywhere, the full package of the release, a delta from the latest release when there is
//!   one, and the feed (`releases.<channel>.json`) that installed copies look for updates in.
//!
//! Linux also gets `peekr-<version>-<platform>.tar.gz`, for systems that cannot run an `AppImage`.
//! It holds the files and nothing else, and does not update itself.
//!
//! The channel is the platform, as `linux-aarch64`. Velopack puts the channel into the name of
//! everything it makes, so the builds for every platform can share one release.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;

const PACKAGE: &str = "peekr";
const TITLE: &str = "Peekr";
const AUTHORS: &str = "kvizyx";
/// Where the Windows installer puts shortcuts. The Startup folder among them, since a tray app
/// that is not running cannot be called up with its hotkey.
const SHORTCUTS: &str = "StartMenuRoot,Startup";
/// Size of the icon an `AppImage` shows in menus and file managers.
const LINUX_ICON_SIZE: u32 = 256;
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
    let build = build(root)?;
    let stage_dir = build.stage()?;

    let release_dir = build.dist_dir.join("release");
    recreate(&release_dir)?;

    velopack(root, &build, &stage_dir, &release_dir)?;

    if !cfg!(windows) {
        let archive = release_dir.join(format!("{}.tar.gz", build.prefix));
        write_tar_gz(&archive, &build.prefix, &build.entries)?;
        eprintln!("packed {} files into {}", build.entries.len(), archive.display());
    }

    eprintln!("the release is in {}", release_dir.display());
    Ok(())
}

/// A release build, with everything that goes into a package.
struct Build {
    version: String,
    /// The operating system and architecture the build is for, as `windows-x86_64`.
    platform: String,
    prefix: String,
    dist_dir: PathBuf,
    entries: Vec<Entry>,
}

impl Build {
    /// Copies the files into `target/dist/<prefix>/`, the layout they are installed in.
    fn stage(&self) -> Result<PathBuf> {
        let stage_dir = self.dist_dir.join(&self.prefix);
        recreate(&stage_dir)?;

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

fn build(root: &Path) -> Result<Build> {
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

    let binary = target_dir.join("release").join(executable());
    let entries = collect_entries(root, binary, third_party_licenses)?;

    Ok(Build {
        version,
        platform,
        prefix,
        dist_dir,
        entries,
    })
}

/// Packs the staged files with `vpk`, and copies what goes on the release into `release_dir`.
fn velopack(root: &Path, build: &Build, stage_dir: &Path, release_dir: &Path) -> Result<()> {
    let channel = build.platform.as_str();
    let work_dir = build.dist_dir.join("velopack");
    recreate(&work_dir)?;

    // A delta is made from the full package of the latest release, which has to be here for
    // that. The first release has none to make one from.
    let repository = package_field(root, "repository")?;
    let mut download = vpk();
    download
        .args(["download", "github", "--repoUrl", &repository, "--channel", channel])
        .arg("--outputDir")
        .arg(&work_dir);
    if let Some(token) = std::env::var_os("GITHUB_TOKEN") {
        download.arg("--token").arg(token);
    }
    if let Err(e) = run(&mut download) {
        eprintln!("no delta: {e:#}");
    }

    let icon = if cfg!(windows) {
        root.join("assets/icon.ico")
    } else {
        let icon = build.dist_dir.join("icon.png");
        crate::icon::png(root, LINUX_ICON_SIZE, &icon)?;
        icon
    };

    let mut pack = vpk();
    pack.args([
        "pack",
        "--packId",
        PACKAGE,
        "--packVersion",
        &build.version,
        "--packTitle",
        TITLE,
        "--packAuthors",
        AUTHORS,
        "--mainExe",
        &executable(),
        "--channel",
        channel,
        "--runtime",
        &runtime(),
    ])
    .arg("--packDir")
    .arg(stage_dir)
    .arg("--icon")
    .arg(&icon)
    .arg("--outputDir")
    .arg(&work_dir);
    if cfg!(windows) {
        pack.args(["--shortcuts", SHORTCUTS]);
    }
    run(&mut pack).context("vpk is required: dotnet tool install -g vpk")?;

    for entry in fs::read_dir(&work_dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();

        if is_published(name, &build.version) {
            fs::copy(&path, release_dir.join(name)).with_context(|| format!("copying {}", path.display()))?;
        }
    }

    Ok(())
}

/// Whether a file `vpk` made goes on the release. Left out are the full package of the release
/// before, which is only there to make the delta from, and the lists `vpk upload` works from,
/// which nothing installed reads.
fn is_published(name: &str, version: &str) -> bool {
    if Path::new(name)
        .extension()
        .is_some_and(|extension| extension == "nupkg")
    {
        return name.starts_with(&format!("{PACKAGE}-{version}-"));
    }

    !(name.starts_with("assets.") || name.starts_with("RELEASES"))
}

/// Runs a command, failing with what it was and how it failed if it does not succeed.
fn run(command: &mut Command) -> Result<()> {
    let status = command.status().with_context(|| format!("starting {command:?}"))?;

    if !status.success() {
        bail!("{command:?} failed with {status}");
    }

    Ok(())
}

fn vpk() -> Command {
    Command::new(std::env::var_os("VPK").unwrap_or_else(|| OsString::from("vpk")))
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")))
}

fn executable() -> String {
    format!("{PACKAGE}{}", std::env::consts::EXE_SUFFIX)
}

/// The .NET runtime identifier `vpk` knows this platform by, as `linux-arm64`.
fn runtime() -> String {
    let os = if cfg!(windows) { "win" } else { std::env::consts::OS };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };

    format!("{os}-{arch}")
}

/// Empties a directory, making it if it is not there.
fn recreate(dir: &Path) -> Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
    }

    Ok(fs::create_dir_all(dir)?)
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
        name: executable(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_carries_its_own_packages_and_what_installed_copies_read() {
        let published = |name| is_published(name, "0.4.1");

        for name in [
            "peekr-0.4.1-linux-x86_64-full.nupkg",
            "peekr-0.4.1-linux-x86_64-delta.nupkg",
            "releases.linux-x86_64.json",
            "peekr-linux-x86_64.AppImage",
            "peekr-windows-x86_64-Setup.exe",
            "peekr-windows-x86_64-Portable.zip",
        ] {
            assert!(published(name), "{name} goes on the release");
        }

        for name in [
            "peekr-0.4.0-linux-x86_64-full.nupkg",
            "assets.linux-x86_64.json",
            "RELEASES-linux-x86_64",
        ] {
            assert!(!published(name), "{name} stays behind");
        }
    }

    #[test]
    fn the_runtime_is_named_the_way_dotnet_names_it() {
        let runtime = runtime();

        assert!(
            ["win-x64", "linux-x64", "linux-arm64"].contains(&runtime.as_str()),
            "{runtime} is not one vpk knows"
        );
    }
}
