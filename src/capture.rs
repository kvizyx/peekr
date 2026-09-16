use std::panic::{self, AssertUnwindSafe};

use anyhow::{Context, Result, anyhow};
use image::RgbaImage;
use xcap::Monitor;

use crate::platform;

pub struct Screenshot {
    pub image: RgbaImage,
    /// Monitor index as enumerated by the windowing system.
    pub monitor_index: usize,
}

/// Captures the whole monitor under the mouse cursor, or the primary monitor when the cursor
/// position is unknown (Wayland).
pub fn capture_active_monitor() -> Result<Screenshot> {
    // xcap's Wayland backend panics instead of returning an error when the compositor lacks a
    // protocol it needs (e.g. xdg-output v3), which would take the whole tray app down.
    panic::catch_unwind(AssertUnwindSafe(capture)).unwrap_or_else(|_| {
        Err(anyhow!(
            "screen capture is not supported by this display server; see the Linux section of the README"
        ))
    })
}

fn capture() -> Result<Screenshot> {
    let monitor = match platform::cursor_position() {
        Some((x, y)) => Monitor::from_point(x, y).context("no monitor under the cursor")?,
        None => primary_monitor()?,
    };

    let image = monitor.capture_image().context("capturing the screen")?;
    let monitor_index = platform::monitor_index(&monitor).unwrap_or(0);

    Ok(Screenshot { image, monitor_index })
}

fn primary_monitor() -> Result<Monitor> {
    let monitors = Monitor::all().context("listing monitors")?;
    let primary = monitors
        .iter()
        .position(|m| m.is_primary().unwrap_or(false))
        .unwrap_or(0);

    monitors.into_iter().nth(primary).context("no monitors found")
}
