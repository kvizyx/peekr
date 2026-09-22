//! Assembling a release: downloading its manifests, signing them, uploading the signatures, and
//! checking that every file each platform needs is actually there.
//!
//! This is the part of a release that does not need a person, only the key only a person has.
//! Publishing the draft stays a separate, deliberate step; see RELEASE.md.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::process::{capture, run};
use crate::signing;

const PLATFORMS: &[&str] = &["windows-x86_64", "linux-x86_64", "linux-aarch64"];

/// Signs the manifests of `tag`, uploads the signatures, and checks the release is complete.
///
/// What is left after this is `gh release edit <tag> --draft=false`.
pub fn release(root: &Path, tag: &str) -> Result<()> {
    let dir = std::env::temp_dir().join(format!("peekr-release-{tag}"));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    run(
        gh(["release", "download", tag, "--pattern", "update-*.toml", "--clobber"])
            .arg("--dir")
            .arg(&dir),
    )?;

    let manifests: Vec<_> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension() == Some(OsStr::new("toml")))
        .collect();

    if manifests.is_empty() {
        bail!("{tag} has no update-*.toml manifests to sign; did the build finish?");
    }

    let manifest_strs: Vec<&str> = manifests
        .iter()
        .map(|path| path.to_str().context("a temp path is not utf-8"))
        .collect::<Result<_>>()?;
    signing::sign_all(root, &manifest_strs)?;

    let mut upload = gh(["release", "upload", tag, "--clobber"]);
    for manifest in &manifests {
        upload.arg(format!("{}.sig", manifest.display()));
    }
    run(&mut upload)?;

    check(tag)
}

/// Confirms every platform has its manifest, signature, payload files and archive.
fn check(tag: &str) -> Result<()> {
    let version = tag.strip_prefix('v').unwrap_or(tag);
    let listing = capture(&mut gh([
        "release",
        "view",
        tag,
        "--json",
        "assets",
        "--jq",
        ".assets[].name",
    ]))?;
    let names: BTreeSet<&str> = listing.lines().collect();

    let mut missing = Vec::new();
    for platform in PLATFORMS {
        for name in [format!("update-{platform}.toml"), format!("update-{platform}.toml.sig")] {
            if !names.contains(name.as_str()) {
                missing.push(name);
            }
        }

        let payload_prefix = format!("update-{platform}-");
        let has_payload = names
            .iter()
            .any(|name| name.starts_with(&payload_prefix) && Path::new(name).extension() == Some(OsStr::new("gz")));
        if !has_payload {
            missing.push(format!("{payload_prefix}*.gz"));
        }

        let archive = if platform.starts_with("windows") {
            format!("peekr-{version}-{platform}.zip")
        } else {
            format!("peekr-{version}-{platform}.tar.gz")
        };
        if !names.contains(archive.as_str()) {
            missing.push(archive);
        }
    }

    let installer = format!("peekr-{version}-windows-x86_64-setup.exe");
    if !names.contains(installer.as_str()) {
        missing.push(installer);
    }

    if !missing.is_empty() {
        bail!("{tag} is missing: {}", missing.join(", "));
    }

    eprintln!("{tag} has everything each platform needs, signed and ready to publish.");
    Ok(())
}

fn gh<I, S>(args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("gh");
    command.args(args);
    command
}
