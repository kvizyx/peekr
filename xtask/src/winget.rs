//! `cargo xtask winget [<version>]`: writes the manifests that publish peekr to the Windows
//! Package Manager, so users can run `winget install peekr`.
//!
//! The manifests go into `target/winget/manifests/k/kvizyx/Peekr/<version>/` and point at the
//! installer of that release on GitHub, whose SHA-256 is taken from the local build when it is
//! there and downloaded otherwise.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};

use crate::hash;

const IDENTIFIER: &str = "kvizyx.Peekr";
const PUBLISHER: &str = "kvizyx";
const REPOSITORY: &str = "https://github.com/kvizyx/peekr";
/// Schema the manifests are written against; `winget validate` accepts this one.
const MANIFEST_VERSION: &str = "1.6.0";
/// Velopack registers the app for "Apps & features" under its package id, and keeps the version
/// there current as it updates, which is what lets winget see what is installed.
const PRODUCT_CODE: &str = "peekr";
/// What `cargo xtask dist` names the Windows installer; the same in every release.
const INSTALLER: &str = "peekr-windows-x86_64-Setup.exe";

pub fn manifests(root: &Path, version: Option<&str>) -> Result<()> {
    let version = match version {
        Some(version) => version.trim_start_matches('v').to_owned(),
        None => crate::dist::package_version(root)?,
    };

    let url = format!("{REPOSITORY}/releases/download/v{version}/{INSTALLER}");
    let sha256 = installer_sha256(root, &version, &url)?.to_uppercase();

    let dir = crate::target_dir(root)
        .join("winget/manifests/k")
        .join(PUBLISHER)
        .join("Peekr")
        .join(&version);
    fs::create_dir_all(&dir)?;

    write(&dir.join(format!("{IDENTIFIER}.yaml")), &version_manifest(&version))?;
    write(
        &dir.join(format!("{IDENTIFIER}.installer.yaml")),
        &installer_manifest(&version, &url, &sha256),
    )?;
    write(
        &dir.join(format!("{IDENTIFIER}.locale.en-US.yaml")),
        &locale_manifest(&version),
    )?;

    eprintln!("wrote the manifests for {version} into {}", dir.display());
    eprintln!(
        "submit them with: wingetcreate submit --token <github token> {}",
        dir.display()
    );

    Ok(())
}

fn write(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

/// Hashes the installer built locally, when it is of this version, or the published one.
fn installer_sha256(root: &Path, version: &str, url: &str) -> Result<String> {
    let local = crate::target_dir(root).join("dist/release").join(INSTALLER);
    if local.is_file() && crate::dist::package_version(root)? == version {
        eprintln!("hashing {}", local.display());
        return hash::sha256_of_file(&local);
    }

    eprintln!("downloading {url}");
    let response = ureq::get(url)
        .header("User-Agent", "peekr-xtask")
        .call()
        .with_context(|| format!("requesting {url}"))?;

    hash::sha256(response.into_body().into_reader())
}

fn version_manifest(version: &str) -> String {
    format!(
        "# yaml-language-server: $schema=https://aka.ms/winget-manifest.version.{MANIFEST_VERSION}.schema.json
PackageIdentifier: {IDENTIFIER}
PackageVersion: {version}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: {MANIFEST_VERSION}
"
    )
}

fn installer_manifest(version: &str, url: &str, sha256: &str) -> String {
    format!(
        "# yaml-language-server: $schema=https://aka.ms/winget-manifest.installer.{MANIFEST_VERSION}.schema.json
PackageIdentifier: {IDENTIFIER}
PackageVersion: {version}
MinimumOSVersion: 10.0.0.0
InstallerType: exe
Scope: user
InstallModes:
- silent
InstallerSwitches:
  Silent: --silent
  SilentWithProgress: --silent
UpgradeBehavior: install
ProductCode: '{PRODUCT_CODE}'
ReleaseNotesUrl: {REPOSITORY}/releases/tag/v{version}
Installers:
- Architecture: x64
  InstallerUrl: {url}
  InstallerSha256: {sha256}
ManifestType: installer
ManifestVersion: {MANIFEST_VERSION}
"
    )
}

fn locale_manifest(version: &str) -> String {
    format!(
        "# yaml-language-server: $schema=https://aka.ms/winget-manifest.defaultLocale.{MANIFEST_VERSION}.schema.json
PackageIdentifier: {IDENTIFIER}
PackageVersion: {version}
PackageLocale: en-US
Publisher: {PUBLISHER}
PublisherUrl: https://github.com/{PUBLISHER}
PublisherSupportUrl: {REPOSITORY}/issues
PackageName: Peekr
PackageUrl: {REPOSITORY}
License: MIT
LicenseUrl: {REPOSITORY}/blob/main/LICENSE
ShortDescription: \"Offline screen OCR: select a region and copy the recognized text\"
Description: |-
  Peekr lives in the system tray. Press a hotkey, drag over any text on screen, and copy what it
  reads. Recognition runs fully offline on PaddleOCR models via ONNX Runtime, so nothing leaves
  the computer.
Moniker: peekr
Tags:
- ocr
- offline
- screenshot
- text-recognition
- tray
ManifestType: defaultLocale
ManifestVersion: {MANIFEST_VERSION}
"
    )
}
