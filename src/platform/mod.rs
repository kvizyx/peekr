//! OS integration.
//!
//! Every platform module provides the same API:
//!
//! - `attach_parent_console()` and `init_dpi_awareness()` for process setup,
//! - `cursor_position()` to pick the monitor to capture,
//! - `EventLoop` and `Waker` to block the main thread until the app has something to do,
//! - `trim_working_set()` to give memory back to the system while idle,
//! - `disable_window_animations()` so windows appear and disappear instantly,
//! - `config_dir()` for per-user settings,
//! - `single_instance()` so that only one tray app runs at a time,
//! - `use_own_frame()` for a window that draws its own frame.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use self::linux::*;
#[cfg(windows)]
pub use self::windows::*;
