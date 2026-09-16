//! The tray application and one-shot captures.

use std::sync::mpsc::{Sender, channel};

use anyhow::{Context, Result};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use image::RgbaImage;

use crate::ocr::models::{ModelStore, OcrConfig};
use crate::ocr::{self, OcrEngine};
use crate::platform::{EventLoop, Waker};
use crate::tray::{Tray, TrayAction};
use crate::worker::OcrWorker;
use crate::{capture, clipboard, overlay};

const HOTKEY_LABEL: &str = "Ctrl+Alt+T";

/// Everything the main thread reacts to. Hotkey, tray and worker callbacks run on their own
/// threads (or inside the Win32 message pump) and only send events.
enum AppEvent {
    Capture,
    Quit,
    Status(String),
}

#[derive(Clone)]
struct EventSender {
    sender: Sender<AppEvent>,
    waker: Waker,
}

impl EventSender {
    fn send(&self, event: AppEvent) {
        // The receiver is gone only when the app is shutting down.
        let _ = self.sender.send(event);
        self.waker.wake();
    }
}

pub fn run_tray(store: ModelStore, config: OcrConfig) {
    let event_loop = EventLoop::new();
    let (sender, events) = channel();
    let sender = EventSender {
        sender,
        waker: event_loop.waker(),
    };

    let worker = OcrWorker::spawn(store, config, {
        let sender = sender.clone();
        move |status| sender.send(AppEvent::Status(status))
    });

    // Both must stay alive for the whole run; either may be unavailable on Linux
    // (no X11 on Wayland, no tray host on GNOME without the AppIndicator extension).
    let _hotkey = register_hotkey(&sender)
        .inspect_err(|e| log::warn!("global hotkey {HOTKEY_LABEL} is unavailable: {e:#}"))
        .ok();
    let tray = create_tray(&sender)
        .inspect_err(|e| log::warn!("tray icon is unavailable: {e:#}"))
        .ok();

    log::info!("ready: press {HOTKEY_LABEL} or click the tray icon");

    while let Some(event) = event_loop.next(&events) {
        match event {
            AppEvent::Quit => break,
            AppEvent::Status(status) => {
                if let Some(tray) = &tray {
                    tray.set_status(&status);
                }
            }
            AppEvent::Capture => {
                worker.prepare();

                match capture_selection() {
                    Ok(Some(region)) => worker.submit(region),
                    Ok(None) => log::debug!("selection cancelled"),
                    Err(e) => log::error!("capture failed: {e:#}"),
                }

                // Capture requests that arrived while the overlay was open are stale.
                let pending: Vec<_> = events.try_iter().collect();
                for event in pending.into_iter().filter(|e| !matches!(e, AppEvent::Capture)) {
                    sender.send(event);
                }
            }
        }
    }
}

/// Captures a region, recognizes it, copies the text and exits. Meant to be bound to a
/// system-wide shortcut on desktops where the app cannot register one itself (Wayland).
pub fn capture_once(store: ModelStore, config: OcrConfig) -> Result<()> {
    // Loading takes a fraction of a second, so it runs while the user is selecting.
    let loader = std::thread::spawn(move || OcrEngine::load(&store, &config));

    let Some(region) = capture_selection()? else {
        log::info!("selection cancelled");
        return Ok(());
    };

    let mut engine = loader.join().map_err(|_| anyhow::anyhow!("model loading panicked"))??;
    let text = ocr::assemble_text(&engine.recognize(&region)?);
    drop(engine);

    if text.is_empty() {
        log::info!("no text found");
        return Ok(());
    }

    log::info!("copied to clipboard:\n{text}");
    clipboard::copy_before_exit(&text)
}

fn capture_selection() -> Result<Option<RgbaImage>> {
    let shot = capture::capture_active_monitor()?;
    overlay::select_region(&shot)
}

fn register_hotkey(sender: &EventSender) -> Result<GlobalHotKeyManager> {
    let manager = GlobalHotKeyManager::new().context("creating the hotkey manager")?;
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyT);
    manager.register(hotkey).context("registering the hotkey")?;

    let sender = sender.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == hotkey.id() && event.state == HotKeyState::Pressed {
            sender.send(AppEvent::Capture);
        }
    }));

    Ok(manager)
}

fn create_tray(sender: &EventSender) -> Result<Tray> {
    let sender = sender.clone();

    Tray::new(HOTKEY_LABEL, move |action| {
        sender.send(match action {
            TrayAction::Capture => AppEvent::Capture,
            TrayAction::Quit => AppEvent::Quit,
        });
    })
}
