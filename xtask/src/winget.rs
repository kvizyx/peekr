//! `cargo xtask winget [<version>]`: writes the manifests that publish peekr to the Windows
//! Package Manager, so users can run `winget install peekr`.
//!
//! The manifests go into `target/winget/manifests/k/kvizyx/Peekr/<version>/` and point at the
//! installer of that release on GitHub, whose SHA-256 is taken from the local build when it is
//! there and downloaded otherwise.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use anyhow::{Context as _, Result};
use sha2::{Digest, Sha256};

const IDENTIFIER: &str = "kvizyx.Peekr";
const PUBLISHER: &str = "kvizyx";
const REPOSITORY: &str = "https://github.com/kvizyx/peekr";
/// Schema the manifests are written against; `winget validate` accepts this one.
const MANIFEST_VERSION: &str = "1.6.0";
/// Inno Setup registers itself under this key, which lets winget see installed versions.
const PRODUCT_CODE: &str = "{6F3B0E4A-2C55-4F35-9C0C-6C8F63A0F3D1}_is1";

pub fn manifests(root: &Path, version: Option<&str>) -> Result<()> {
    let version = match version {
        Some(version) => version.trim_start_matches('v').to_owned(),
        None => crate::dist::package_version(root)?,
    };

    let installer = format!("peekr-{version}-windows-x86_64-setup.exe");
    let url = format!("{REPOSITORY}/releases/download/v{version}/{installer}");
    let sha256 = installer_sha256(root, &installer, &url)?.to_uppercase();

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

/// Hashes the installer built locally, or the published one when it is not around.
fn installer_sha256(root: &Path, installer: &str, url: &str) -> Result<String> {
    let local = crate::target_dir(root).join("dist").join(installer);
    if local.is_file() {
        eprintln!("hashing {}", local.display());
        return sha256_of_file(&local);
    }

    eprintln!("downloading {url}");
    let response = ureq::get(url)
        .header("User-Agent", "peekr-xtask")
        .call()
        .with_context(|| format!("requesting {url}"))?;

    let mut hasher = Sha256::new();
    let mut body = response.into_body().into_reader();
    let mut buffer = vec![0; 1 << 20];

    loop {
        let read = body.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        hasher.update(&buffer[..read]);
    }

    Ok(crate::models::hex(&hasher.finalize()))
}

fn sha256_of_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        hasher.update(&buffer[..read]);
    }

    Ok(crate::models::hex(&hasher.finalize()))
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
InstallerType: inno
Scope: user
InstallModes:
- interactive
- silent
- silentWithProgress
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
