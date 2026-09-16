//! Copying recognized text.
//!
//! On X11 and Wayland the clipboard has no storage of its own: the process that copied text has
//! to serve it to other applications. The tray app keeps one [`Clipboard`] alive for its whole
//! lifetime; one-shot captures wait until another application takes the clipboard over.

use anyhow::Result;

/// A clipboard handle that stays open between copies.
#[derive(Default)]
pub struct Clipboard {
    inner: Option<arboard::Clipboard>,
}

impl Clipboard {
    pub fn copy(&mut self, text: &str) -> Result<()> {
        let clipboard = match &mut self.inner {
            Some(clipboard) => clipboard,
            None => self.inner.insert(arboard::Clipboard::new()?),
        };

        clipboard.set_text(text)?;
        Ok(())
    }
}

/// Copies the text for a process that is about to exit.
///
/// On Linux this blocks until another application replaces the clipboard content (a clipboard
/// manager usually does so right away), so the text stays pasteable after the capture ends.
pub fn copy_before_exit(text: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use arboard::SetExtLinux as _;

        log::info!("serving the clipboard until another application replaces it");
        arboard::Clipboard::new()?.set().wait().text(text)?;
    }

    #[cfg(not(target_os = "linux"))]
    arboard::Clipboard::new()?.set_text(text)?;

    Ok(())
}
