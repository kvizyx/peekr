//! The tray application and one-shot captures.

use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::Result;

use crate::clipboard::Clipboard;
use crate::config::Config;
use crate::hotkey::GlobalHotkey;
use crate::ocr::models::{ModelStore, OcrConfig};
use crate::platform::{EventLoop, Waker};
use crate::shortcut::Shortcut;
use crate::tray::{Tray, TrayAction};
use crate::worker::OcrWorker;
use crate::{capture, clipboard, overlay, settings};

/// Everything the main thread reacts to. Hotkey, tray and worker callbacks run on their own
/// threads (or inside the Win32 message pump) and only send events.
enum AppEvent {
    Capture,
    OpenSettings,
    Quit,
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

pub fn run_tray(store: ModelStore, ocr_config: OcrConfig) {
    let event_loop = EventLoop::new();
    let (sender, events) = channel();
    let sender = EventSender {
        sender,
        waker: event_loop.waker(),
    };

    let worker = OcrWorker::spawn(store, ocr_config);
    // Kept open for the app's lifetime: on Linux the copied text lives only as long as this handle.
    let mut clipboard = Clipboard::default();

    let mut config = Config::load();

    // Both may be unavailable on Linux: no global hotkeys on Wayland, no tray host on GNOME
    // without the AppIndicator extension.
    let mut hotkey = create_hotkey(&sender, &config);
    let tray = create_tray(&sender, hotkey.as_ref().and_then(GlobalHotkey::current))
        .inspect_err(|e| log::warn!("tray icon is unavailable: {e:#}"))
        .ok();

    match hotkey.as_ref().and_then(GlobalHotkey::current) {
        Some(shortcut) => log::info!("ready: press {shortcut} or click the tray icon"),
        None => log::info!("ready: click the tray icon or run `peekr --capture`"),
    }

    while let Some(event) = event_loop.next(&events) {
        match event {
            AppEvent::Capture => {
                let copy = &mut |text: &str| {
                    clipboard.copy(text)?;
                    log::info!(
                        "copied to clipboard:
{text}"
                    );
                    Ok(())
                };

                if let Err(e) = capture_text(&worker, copy) {
                    log::error!("capture failed: {e:#}");
                }

                drop_stale_requests(&events, &sender);
            }
            AppEvent::OpenSettings => {
                open_settings(&mut config, hotkey.as_mut());

                if let Some(tray) = &tray {
                    tray.set_hotkey(hotkey.as_ref().and_then(GlobalHotkey::current));
                }

                drop_stale_requests(&events, &sender);
            }
            AppEvent::Quit => break,
        }
    }
}

/// Captures a region, shows the recognized text and exits once it is copied or the overlay is
/// closed. Meant to be bound to a system-wide shortcut on desktops where the app cannot register
/// one itself (Wayland).
pub fn capture_once(store: ModelStore, config: OcrConfig) -> Result<()> {
    let worker = OcrWorker::spawn(store, config);
    let mut copied = None;

    capture_text(&worker, &mut |text| {
        copied = Some(text.to_owned());
        Ok(())
    })?;

    // Unloads the models before possibly waiting for the clipboard below.
    drop(worker);

    let Some(text) = copied else {
        log::info!("closed without copying");
        return Ok(());
    };

    log::info!("copied to clipboard:\n{text}");
    clipboard::copy_before_exit(&text)
}

/// Shows the overlay over the active monitor (every monitor on Wayland); the models load while
/// the user is selecting.
fn capture_text(worker: &OcrWorker, copy: overlay::CopyText<'_>) -> Result<()> {
    worker.prepare();

    let screens = capture::capture_screens()?;
    overlay::capture_text(&screens, &|image| worker.recognize(image), copy)
}

/// Drops capture and settings requests that arrived while a window was open; keeps the rest.
fn drop_stale_requests(events: &Receiver<AppEvent>, sender: &EventSender) {
    let pending: Vec<_> = events.try_iter().collect();

    for event in pending {
        if !matches!(event, AppEvent::Capture | AppEvent::OpenSettings) {
            sender.send(event);
        }
    }
}

fn create_hotkey(sender: &EventSender, config: &Config) -> Option<GlobalHotkey> {
    let sender = sender.clone();
    let mut hotkey = GlobalHotkey::new(move || sender.send(AppEvent::Capture))
        .inspect_err(|e| log::warn!("global hotkeys are unavailable: {e:#}"))
        .ok()?;

    if let Err(e) = hotkey.register(config.hotkey) {
        log::warn!("{e:#}; choose another hotkey in the settings");
    }

    Some(hotkey)
}

fn create_tray(sender: &EventSender, hotkey: Option<Shortcut>) -> Result<Tray> {
    let sender = sender.clone();

    Tray::new(hotkey, move |action| {
        sender.send(match action {
            TrayAction::Capture => AppEvent::Capture,
            TrayAction::OpenSettings => AppEvent::OpenSettings,
            TrayAction::Quit => AppEvent::Quit,
        });
    })
}

/// Shows the settings window. Every recorded hotkey is checked and saved right away, and
/// becomes active once the window closes.
fn open_settings(config: &mut Config, mut hotkey: Option<&mut GlobalHotkey>) {
    // An active hotkey would start a capture instead of being recorded.
    if let Some(hotkey) = hotkey.as_mut() {
        hotkey.unregister();
    }

    let current = config.hotkey;
    let result = settings::edit_hotkey(current, &mut |candidate| {
        // Registering proves that no other application holds the shortcut.
        if let Some(hotkey) = hotkey.as_mut() {
            hotkey.register(candidate)?;
            hotkey.unregister();
        }

        config.hotkey = candidate;
        config.save()
    });

    if let Err(e) = result {
        log::error!("{e:#}");
    }

    if let Some(hotkey) = hotkey
        && let Err(e) = hotkey.register(config.hotkey)
    {
        log::warn!("{e:#}");
    }
}
