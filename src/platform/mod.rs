//! OS integration.
//!
//! Every platform module provides the same API, so that the rest of the app needs no `cfg`:
//!
//! - process setup: `attach_parent_console()`, `init_dpi_awareness()`, `single_instance()` so
//!   that only one tray app runs at a time, and `config_dir()` for per-user settings;
//! - the main thread: `EventLoop` and `Waker` block it until the app has something to do, and
//!   `configure_event_loop()` sets up the winit event loop that windows run in;
//! - screens: `cursor_position()` and `monitor_contains()` pick the monitor to work on,
//!   `screen_capture_access()` checks that the app may capture it, and `cover_monitor()` places
//!   an overlay window over it;
//! - windows: `prepare_window()` for every new window, `float_over_screen()` for an overlay,
//!   `use_own_frame()` for a window that draws its own frame, and `return_focus()` once the
//!   app's windows are gone;
//! - memory: `trim_working_set()` gives memory back to the system while idle;
//! - key names: `SUPER_KEY`, `ALT_KEY` and `COMMAND_KEY`, as the platform's keyboards label them.
//!
//! Desktop coordinates, which the cursor position and the screenshots' origins are in, are
//! physical pixels on Windows and Linux, and points on macOS, where the system works in points.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use self::linux::*;
#[cfg(target_os = "macos")]
pub use self::macos::*;
#[cfg(windows)]
pub use self::windows::*;

/// Whether a monitor, whose position and size winit reports in physical pixels, contains a
/// point in physical desktop coordinates.
#[cfg(not(target_os = "macos"))]
fn physical_bounds_contain(monitor: &winit::monitor::MonitorHandle, (x, y): (i32, i32)) -> bool {
    let (origin, area) = (monitor.position(), monitor.size());

    (origin.x..origin.x + area.width as i32).contains(&x) && (origin.y..origin.y + area.height as i32).contains(&y)
}
