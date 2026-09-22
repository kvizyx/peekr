//! `cargo xtask installer`: builds the Windows installer with Inno Setup.
//!
//! The installer `target/dist/peekr-<version>-windows-x86_64-setup.exe` contains the same files
//! as the archive. It installs per user, so it asks for no administrator rights.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};

use crate::dist;

/// Where Inno Setup puts its compiler when it is not on `PATH`.
const DEFAULT_PATHS: &[&str] = &[
    r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    r"C:\Program Files\Inno Setup 6\ISCC.exe",
];

pub fn build(root: &Path) -> Result<()> {
    if !cfg!(windows) {
        bail!("the installer can only be built on Windows");
    }

    let compiler = find_compiler()?;
    let build = dist::build(root)?;
    let stage_dir = build.stage()?;

    let name = format!("{}-setup", build.prefix);
    let script = root.join("xtask/installer/peekr.iss");

    eprintln!("building the installer with {}", compiler.display());
    let mut command = Command::new(&compiler);
    command
        .arg(format!("/DVersion={}", build.version))
        .arg(format!("/DStageDir={}", stage_dir.display()))
        .arg(format!("/DOutputDir={}", build.dist_dir.display()))
        .arg(format!("/DOutputName={name}"))
        .arg(format!("/DIconFile={}", root.join("assets/icon.ico").display()))
        .arg(&script);

    crate::process::run(&mut command)?;

    let installer = build.dist_dir.join(format!("{name}.exe"));
    let size = fs::metadata(&installer)?.len() as f64 / f64::from(1 << 20);
    eprintln!("built {} ({size:.1} MB)", installer.display());

    Ok(())
}

/// Looks for the Inno Setup compiler in `ISCC`, on `PATH` and where its installer puts it.
fn find_compiler() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ISCC") {
        return Ok(PathBuf::from(path));
    }

    if Command::new("iscc").arg("/?").status().is_ok() {
        return Ok(PathBuf::from("iscc"));
    }

    DEFAULT_PATHS
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .context("Inno Setup 6 is required: install it from https://jrsoftware.org/isdl.php or with `winget install JRSoftware.InnoSetup`, or point ISCC at its compiler")
}
