#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod ocr;
mod overlay;
mod platform;
mod tray;
mod worker;

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::tray::{Tray, TrayAction};
use crate::worker::OcrWorker;

/// Recognition model used until language selection lands in settings.
const DEFAULT_LANGUAGE: &str = "eslav";

const HOTKEY_LABEL: &str = "Ctrl+Alt+T";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        platform::attach_parent_console();
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    platform::init_dpi_awareness();

    match args.as_slice() {
        [] => run_tray_app(),
        [flag, path] if flag == "--image" => recognize_file(path),
        _ => bail!("usage: ochco [--image <file>]"),
    }
}

fn run_tray_app() -> Result<()> {
    let models = ocr::ModelPaths::in_dir(&models_dir()?, DEFAULT_LANGUAGE);
    let worker = OcrWorker::spawn(models, platform::Waker::for_current_thread());

    // The hotkey manager and the tray icon must live on the thread that pumps messages.
    let hotkeys = GlobalHotKeyManager::new()?;
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyT);
    if let Err(e) = hotkeys.register(hotkey) {
        log::error!("failed to register {HOTKEY_LABEL}: {e}; use the tray icon instead");
    }

    let tray = Tray::new(HOTKEY_LABEL)?;
    log::info!("ready: press {HOTKEY_LABEL} or click the tray icon");

    while platform::pump_message() {
        let mut capture = drain_hotkey_presses(hotkey);

        match tray.poll() {
            Some(TrayAction::Quit) => break,
            Some(TrayAction::Capture) => capture = true,
            None => {}
        }

        while let Some(status) = worker.poll_status() {
            tray.set_status(&status);
        }

        if capture {
            match capture_selection() {
                Ok(Some(region)) => worker.submit(region),
                Ok(None) => log::debug!("selection cancelled"),
                Err(e) => log::error!("capture failed: {e:#}"),
            }

            // Ignore hotkey presses that happened while the overlay was open.
            drain_hotkey_presses(hotkey);
        }
    }

    Ok(())
}

/// Consumes queued hotkey events and reports whether `hotkey` was pressed.
fn drain_hotkey_presses(hotkey: HotKey) -> bool {
    let mut pressed = false;
    while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
        pressed |= event.id == hotkey.id() && event.state == HotKeyState::Pressed;
    }
    pressed
}

fn capture_selection() -> Result<Option<image::RgbaImage>> {
    let shot = capture::capture_monitor_under_cursor()?;
    overlay::select_region(&shot)
}

#[expect(clippy::print_stdout, reason = "CLI mode prints the recognized text")]
fn recognize_file(path: &str) -> Result<()> {
    let img = image::open(path).with_context(|| format!("opening {path}"))?.to_rgba8();
    let mut engine = ocr::OcrEngine::load(&ocr::ModelPaths::in_dir(&models_dir()?, DEFAULT_LANGUAGE))?;

    let started = Instant::now();
    let lines = engine.recognize(&img)?;
    log::info!("recognized {} boxes in {:?}", lines.len(), started.elapsed());

    println!("{}", ocr::assemble_text(&lines));
    Ok(())
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
        .find(|dir| dir.join("det").is_dir())
        .context("models directory not found; run scripts/download_models.py")
}
