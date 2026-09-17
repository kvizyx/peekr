#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod capture;
mod cli;
mod clipboard;
mod config;
mod hotkey;
mod icon;
mod ocr;
mod overlay;
mod platform;
mod settings;
mod shortcut;
mod tray;
mod window;
mod worker;

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};

use crate::cli::{Args, Command};
use crate::ocr::models::{ModelStore, OcrConfig};

fn main() -> Result<()> {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if !raw_args.is_empty() {
        platform::attach_parent_console();
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    platform::init_dpi_awareness();

    let args = Args::parse(raw_args)?;
    let store = ModelStore::new(models_dir()?);

    match args.command {
        Command::Tray => {
            app::run_tray(store, args.ocr);
            Ok(())
        }
        Command::Capture => app::capture_once(store, args.ocr),
        Command::Image(path) => recognize_file(&path, &store, &args.ocr),
        Command::ListModels => {
            print_models(&store);
            Ok(())
        }
    }
}

#[expect(clippy::print_stdout, reason = "CLI mode prints the recognized text")]
fn recognize_file(path: &str, store: &ModelStore, config: &OcrConfig) -> Result<()> {
    let img = image::open(path).with_context(|| format!("opening {path}"))?.to_rgba8();
    let mut engine = ocr::OcrEngine::load(store, config)?;

    let started = Instant::now();
    let lines = engine.recognize(&img)?;
    log::info!("recognized {} boxes in {:?}", lines.len(), started.elapsed());

    println!("{}", ocr::assemble_text(&lines));
    Ok(())
}

#[expect(clippy::print_stdout, reason = "CLI mode prints the model list")]
fn print_models(store: &ModelStore) {
    let installed = |yes: bool| if yes { "installed" } else { "not installed" };

    println!("Detectors (the first installed one is the default):");
    for spec in ocr::models::DETECTORS {
        let status = installed(store.detector_path(spec.id).is_file());
        println!("  {:<18} {:<24} {status}", spec.id, spec.name);
    }

    println!("\nRecognizers (auto mode runs every installed one):");
    for spec in ocr::models::RECOGNIZERS {
        let status = installed(store.recognizer_path(spec.id).is_file());
        println!("  {:<18} {:<24} {status}", spec.id, spec.name);
        println!("  {:<18} {}", "", spec.languages);
    }
}

/// Looks for `models/` next to the executable, in the working directory and, in dev builds,
/// in the project root.
fn models_dir() -> Result<PathBuf> {
    let exe_dir = std::env::current_exe()?.parent().map(PathBuf::from);
    let project_dir = cfg!(debug_assertions).then(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    exe_dir
        .into_iter()
        .chain(std::env::current_dir().ok())
        .chain(project_dir)
        .map(|dir| dir.join("models"))
        .find(|dir| dir.join("det").is_dir() && dir.join("rec").is_dir())
        .context("models directory not found; run `cargo xtask models`")
}
