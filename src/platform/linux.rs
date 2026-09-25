use std::sync::mpsc::Receiver;

use anyhow::Result;
use winit::dpi::PhysicalPosition;
use winit::event_loop::EventLoopBuilder;
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowAttributes};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

use crate::capture::Screenshot;

pub use super::unix::{attach_parent_console, single_instance};

pub const SUPER_KEY: &str = "Super";
pub const ALT_KEY: &str = "Alt";
pub const COMMAND_KEY: &str = "Ctrl";

/// X11 and Wayland report monitor geometry in physical pixels already.
pub fn init_dpi_awareness() {}

/// winit's defaults suit Linux.
pub fn configure_event_loop(_builder: &mut EventLoopBuilder<()>) {}

/// Returns memory freed by the allocator back to the system.
pub fn trim_working_set() {
    #[cfg(target_env = "gnu")]
    {
        // SAFETY: `malloc_trim` has no preconditions.
        unsafe { libc::malloc_trim(0) };
    }
}

/// Directory for per-user settings (`$XDG_CONFIG_HOME`, or `~/.config`).
pub fn config_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| Some(super::unix::home_dir()?.join(".config")))
}

/// Neither X11 nor Wayland lets an opaque window be given a shape of its own, so a window that
/// draws its own frame keeps its corners square here.
pub fn use_own_frame(_window: &Window, _radius: u32) {}

/// X11 and Wayland offer no portable way to opt a window out of compositor animations, and
/// nothing else needs doing.
pub fn prepare_window(_window: &Window) {}

/// A fullscreen window is above everything already.
pub fn float_over_screen(_window: &Window) {}

/// The window manager hands focus on by itself.
pub fn return_focus() {}

/// xcap reads the screen through X11 or the desktop portal, which asks the user itself.
#[expect(
    clippy::unnecessary_wraps,
    reason = "same API as macOS, where the user can deny access"
)]
pub fn screen_capture_access() -> Result<()> {
    Ok(())
}

/// Makes the window cover the monitor the screenshot was taken from. Wayland does not let
/// clients position windows, so fullscreen is the only way to get there.
///
/// xcap and winit list monitors in different orders, so the monitor is found by its output name,
/// then by position, and only then by index when both see the same number of monitors.
pub fn cover_monitor(
    attributes: WindowAttributes,
    monitors: &[MonitorHandle],
    screen_count: usize,
    index: usize,
    shot: &Screenshot,
) -> WindowAttributes {
    let monitor = monitors
        .iter()
        .find(|m| {
            m.name()
                .is_some_and(|name| shot.monitor_name.as_deref() == Some(&*name))
        })
        .or_else(|| {
            monitors.iter().find(|m| {
                let position = m.position();
                (position.x, position.y) == shot.origin
            })
        })
        .or_else(|| (monitors.len() == screen_count).then(|| monitors.get(index)).flatten())
        .cloned();

    if monitor.is_none() {
        log::warn!(
            "no monitor matches {:?} at {:?}; the overlay opens on the current one",
            shot.monitor_name,
            shot.origin
        );
    }

    // X11 window managers put a fullscreen window on the monitor it was mapped on, so it starts
    // out there as well.
    attributes
        .with_position(PhysicalPosition::new(shot.origin.0, shot.origin.1))
        .with_fullscreen(Some(Fullscreen::Borderless(monitor)))
}

/// Whether the monitor contains a point in desktop coordinates.
pub fn monitor_contains(monitor: &MonitorHandle, point: (i32, i32)) -> bool {
    super::physical_bounds_contain(monitor, point)
}

/// Cursor position in virtual-desktop pixels, if the display server exposes it.
///
/// Wayland does not let clients query the global cursor position, and XWayland only knows it
/// while the pointer is over an X11 window, so on Wayland this returns `None`.
pub fn cursor_position() -> Option<(i32, i32)> {
    if is_wayland_session() {
        return None;
    }

    let (conn, screen_index) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen_index)?.root;
    let pointer = conn.query_pointer(root).ok()?.reply().ok()?;

    Some((i32::from(pointer.root_x), i32::from(pointer.root_y)))
}

fn is_wayland_session() -> bool {
    match std::env::var("XDG_SESSION_TYPE") {
        Ok(session) if !session.is_empty() => session == "wayland",
        _ => std::env::var_os("WAYLAND_DISPLAY").is_some(),
    }
}

/// Blocks the main thread until the app has an event to handle. The tray (D-Bus) and the
/// global hotkey (X11) run their own threads on Linux, so there is nothing to pump.
pub struct EventLoop;

impl EventLoop {
    pub fn new() -> Self {
        Self
    }

    #[expect(clippy::unused_self, reason = "same API as the other platforms' event loops")]
    pub fn waker(&self) -> Waker {
        Waker
    }

    /// Returns the next event, or `None` when the channel is closed.
    #[expect(clippy::unused_self, reason = "same API as the other platforms' event loops")]
    pub fn next<T>(&self, events: &Receiver<T>) -> Option<T> {
        events.recv().ok()
    }
}

/// Nothing to wake: [`EventLoop::next`] blocks on the channel itself.
#[derive(Debug, Clone, Copy)]
pub struct Waker;

impl Waker {
    #[expect(clippy::unused_self, reason = "same API as the other platforms' wakers")]
    pub fn wake(self) {}
}
