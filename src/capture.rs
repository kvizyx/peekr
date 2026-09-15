use anyhow::{Context, Result};
use image::RgbaImage;
use xcap::Monitor;

use crate::platform;

pub struct Screenshot {
    pub image: RgbaImage,
    /// Monitor index as enumerated by the windowing system.
    pub monitor_index: usize,
}

/// Captures the whole monitor under the mouse cursor.
pub fn capture_monitor_under_cursor() -> Result<Screenshot> {
    let (x, y) = platform::cursor_position();
    let monitor = Monitor::from_point(x, y).context("no monitor under the cursor")?;
    let image = monitor.capture_image().context("capturing the screen")?;
    let monitor_index = platform::monitor_index_at(x, y).unwrap_or(0);
    Ok(Screenshot { image, monitor_index })
}
