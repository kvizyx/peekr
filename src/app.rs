//! The tray application and one-shot captures.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
use crate::{capture, clipboard, overlay, settings, window};

/// Everything the main thread reacts to. Hotkey, tray and worker callbacks run on their own
/// threads (or inside the Win32 message pump, or the AppKit one) and only send events.
enum AppEvent {
    Capture,
    OpenSettings,
    Quit,
}

#[derive(Clone)]
struct EventSender {
    sender: Sender<AppEvent>,
    waker: Waker,
    /// Set when an event is sent and cleared once the main thread picks it up. A window that is
    /// open at that moment watches this and closes, since only one window runs at a time.
    waiting: Arc<AtomicBool>,
}

impl EventSender {
    fn send(&self, event: AppEvent) {
        self.waiting.store(true, Ordering::Release);

        // The receiver is gone only when the app is shutting down.
        let _ = self.sender.send(event);
        self.waker.wake();
    }
}

pub fn run_tray(store: ModelStore, ocr_config: OcrConfig, mut config: Config) {
    window::init();

    let event_loop = EventLoop::new();
    let (sender, events) = channel();
    let waiting = Arc::new(AtomicBool::new(false));
    let sender = EventSender {
        sender,
        waker: event_loop.waker(),
        waiting: Arc::clone(&waiting),
    };

    let worker = OcrWorker::spawn(store, ocr_config);
    // Kept open for the app's lifetime: on Linux the copied text lives only as long as this handle.
    let mut clipboard = Clipboard::default();

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

    // Whether the settings window was closed only to let an event through, and should come back.
    let mut reopen_settings = false;

    while let Some(event) = event_loop.next(&events) {
        waiting.store(false, Ordering::Release);

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

                if reopen_settings {
                    reopen_settings = false;
                    sender.send(AppEvent::OpenSettings);
                }
            }
            AppEvent::OpenSettings => {
                let interrupted = || waiting.load(Ordering::Acquire);
                open_settings(&mut config, hotkey.as_mut(), &interrupted);

                if let Some(tray) = &tray {
                    tray.set_hotkey(hotkey.as_ref().and_then(GlobalHotkey::current));
                }

                // The window closed on its own, so anything that piled up meanwhile is stale.
                reopen_settings = interrupted();
                if !reopen_settings {
                    drop_stale_requests(&events, &sender);
                }
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

/// Shows the settings window, which stays open until the user closes it or `interrupted`
/// reports that another request is waiting. The hotkey keeps working meanwhile: it is released
/// only while a new one is being recorded, where it would fire instead of being recorded.
fn open_settings(config: &mut Config, hotkey: Option<&mut GlobalHotkey>, interrupted: &dyn Fn() -> bool) {
    let current = config.clone();
    let config = RefCell::new(config);
    let hotkey = RefCell::new(hotkey);

    let mut apply = |candidate: Shortcut| {
        // Registering proves that no other application holds the shortcut, and leaves it active.
        if let Some(hotkey) = hotkey.borrow_mut().as_mut() {
            hotkey.register(candidate)?;
        }

        let mut config = config.borrow_mut();
        config.hotkey = candidate;
        config.save()
    };

    let mut recording = |recording: bool| {
        let mut borrowed = hotkey.borrow_mut();
        let Some(hotkey) = borrowed.as_mut() else {
            return;
        };

        if recording {
            hotkey.unregister();
        } else if let Err(e) = hotkey.register(config.borrow().hotkey) {
            log::warn!("{e:#}");
        }
    };

    let mut set_updates = |enabled: bool| {
        let mut config = config.borrow_mut();
        config.updates = enabled;

        if let Err(e) = config.save() {
            log::error!("{e:#}");
        }
    };

    let handlers = settings::Handlers {
        apply: &mut apply,
        recording: &mut recording,
        set_updates: &mut set_updates,
        interrupted,
    };

    if let Err(e) = settings::open(&current, handlers) {
        log::error!("{e:#}");
    }
}
