use std::panic::{self, AssertUnwindSafe};

use anyhow::{Context, Result, anyhow};
use image::RgbaImage;
use xcap::Monitor;

use crate::platform;

pub struct Screenshot {
    pub image: RgbaImage,
    /// Top-left corner of the monitor in physical virtual-desktop pixels.
    pub origin: (i32, i32),
    /// Output name reported by the display server (e.g. `DP-1`). It identifies the monitor for
    /// the window system too, whose monitor list is not in the same order as xcap's.
    #[cfg(not(windows))]
    pub monitor_name: Option<String>,
}

/// Captures the monitor under the mouse cursor. When the cursor position is unknown (Wayland),
/// captures every monitor, so the user can select text on any of them.
pub fn capture_screens() -> Result<Vec<Screenshot>> {
    // xcap's Wayland backend panics instead of returning an error when the compositor lacks a
    // protocol it needs (e.g. xdg-output v3), which would take the whole tray app down.
    panic::catch_unwind(AssertUnwindSafe(capture)).unwrap_or_else(|_| {
        Err(anyhow!(
            "screen capture is not supported by this display server; see the Linux section of the README"
        ))
    })
}

fn capture() -> Result<Vec<Screenshot>> {
    if let Some((x, y)) = platform::cursor_position() {
        let monitor = Monitor::from_point(x, y).context("no monitor under the cursor")?;
        return Ok(vec![screenshot(&monitor)?]);
    }

    let monitors = Monitor::all().context("listing monitors")?;
    let mut screenshots = Vec::with_capacity(monitors.len());
    let mut last_error = None;

    for monitor in &monitors {
        match screenshot(monitor) {
            Ok(shot) => screenshots.push(shot),
            Err(e) => {
                log::warn!("skipping monitor {:?}: {e:#}", monitor.name().ok());
                last_error = Some(e);
            }
        }
    }

    match last_error {
        Some(e) if screenshots.is_empty() => Err(e),
        _ if screenshots.is_empty() => Err(anyhow!("no monitors found")),
        _ => Ok(screenshots),
    }
}

fn screenshot(monitor: &Monitor) -> Result<Screenshot> {
    let image = monitor.capture_image().context("capturing the screen")?;

    Ok(Screenshot {
        image,
        origin: (monitor.x().unwrap_or(0), monitor.y().unwrap_or(0)),
        #[cfg(not(windows))]
        monitor_name: monitor.name().ok(),
    })
}
