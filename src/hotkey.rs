//! The global capture hotkey, which can be changed while the app runs.

use anyhow::{Context, Result, bail};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::shortcut::Shortcut;

pub struct GlobalHotkey {
    manager: GlobalHotKeyManager,
    registered: Option<(Shortcut, HotKey)>,
}

impl GlobalHotkey {
    /// `on_press` runs on the thread that delivers hotkey events (the Win32 message pump on
    /// Windows, a background X11 thread on Linux).
    pub fn new(on_press: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        let manager = GlobalHotKeyManager::new().context("creating the hotkey manager")?;

        // Only one hotkey is ever registered, so any press is the capture hotkey.
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state == HotKeyState::Pressed {
                on_press();
            }
        }));

        Ok(Self {
            manager,
            registered: None,
        })
    }

    pub fn current(&self) -> Option<Shortcut> {
        self.registered.map(|(shortcut, _)| shortcut)
    }

    /// Replaces the registered hotkey. If the new one cannot be registered (usually because
    /// another application holds it), the previous hotkey is restored and an error returned.
    pub fn register(&mut self, shortcut: Shortcut) -> Result<()> {
        if self.current() == Some(shortcut) {
            return Ok(());
        }

        let previous = self.current();
        self.unregister();

        let hotkey = shortcut.to_hotkey();
        if let Err(e) = self.manager.register(hotkey) {
            if let Some(previous) = previous {
                // Best effort: the previous hotkey was registered a moment ago.
                let _ = self.register(previous);
            }

            // The error names internal hotkey ids, which mean nothing to users; keep it in the log.
            log::debug!("registering {shortcut} failed: {e}");
            bail!("{shortcut} is already used by another application or the system");
        }

        self.registered = Some((shortcut, hotkey));
        Ok(())
    }

    pub fn unregister(&mut self) {
        if let Some((shortcut, hotkey)) = self.registered.take()
            && let Err(e) = self.manager.unregister(hotkey)
        {
            log::warn!("failed to unregister {shortcut}: {e}");
        }
    }
}
