//! Development tasks for ochco, run with `cargo xtask <task>` (the alias is in `.cargo/config.toml`).
//!
//! Status output goes to stderr, like cargo's own.

mod dist;
mod icon;
mod models;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage: cargo xtask <task>

tasks:
  models   download the OCR models into ./models (skips files that are up to date)
  icon     render assets/icon.svg into assets/icon.png, which the app embeds
  dist     build a release archive for this platform into target/dist
           (needs downloaded models and cargo-about: cargo install cargo-about --features cli)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["models"] => models::download(&project_root().join("models")),
        ["icon"] => icon::render(project_root()),
        ["dist"] => dist::package(project_root()),
        [] | ["help" | "--help" | "-h"] => {
            eprintln!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("unknown arguments: {args:?}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn project_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in a subdirectory of the project")
}

/// Cargo's target directory, honoring `CARGO_TARGET_DIR`.
fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from)
}
