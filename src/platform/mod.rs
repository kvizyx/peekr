//! OS integration.
//!
//! What every platform provides, with an implementation of its own, is re-exported here:
//!
//! - `config_dir()` for per-user settings,
//! - `single_instance()` so that only one tray app runs at a time,
//! - `cursor_position()` to pick the monitor to capture,
//! - `trim_working_set()` to give memory back to the system while idle,
//! - `EventLoop` and `Waker` to block the main thread until the app has something to do.
//!
//! What only one system needs stays in its own module, such as [`windows`], and is called from
//! behind a `cfg` of its own. That keeps a step that does not exist elsewhere from being passed off
//! as one that is done differently there, and adding a system means writing the list above and
//! nothing more.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
pub mod windows;

#[cfg(target_os = "linux")]
pub use self::linux::{EventLoop, Waker, config_dir, cursor_position, single_instance, trim_working_set};
#[cfg(windows)]
pub use self::windows::{EventLoop, Waker, config_dir, cursor_position, single_instance, trim_working_set};
