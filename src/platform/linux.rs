use std::sync::mpsc::Receiver;

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

/// Console output works out of the box on Linux.
pub fn attach_parent_console() {}

/// X11 and Wayland report monitor geometry in physical pixels already.
pub fn init_dpi_awareness() {}

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
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config")))
}

/// X11 and Wayland offer no portable way to opt a window out of compositor animations.
pub fn disable_window_animations(_window: &winit::window::Window) {}

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

    #[expect(clippy::unused_self, reason = "same API as the Windows event loop")]
    pub fn waker(&self) -> Waker {
        Waker
    }

    /// Returns the next event, or `None` when the channel is closed.
    #[expect(clippy::unused_self, reason = "same API as the Windows event loop")]
    pub fn next<T>(&self, events: &Receiver<T>) -> Option<T> {
        events.recv().ok()
    }
}

/// Nothing to wake: [`EventLoop::next`] blocks on the channel itself.
#[derive(Debug, Clone, Copy)]
pub struct Waker;

impl Waker {
    #[expect(clippy::unused_self, reason = "same API as the Windows waker")]
    pub fn wake(self) {}
}
