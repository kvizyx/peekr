use std::ptr::null_mut;
use std::sync::mpsc::{Receiver, TryRecvError};

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThreadId};
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetCursorPos, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_APP,
};

/// Lets a GUI-subsystem executable print to the console it was started from.
pub fn attach_parent_console() {
    // SAFETY: no pointers involved; failure (no parent console) is harmless.
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

/// Screen capture and window placement must work in physical pixels on every monitor.
pub fn init_dpi_awareness() {
    // SAFETY: no pointers involved; fails only if awareness was already set, which is fine.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// Asks Windows to move this process's pages out of physical memory. Pages that are needed
/// again are faulted back in, so this only frees RAM held by idle state.
pub fn trim_working_set() {
    // SAFETY: the current process handle is always valid; no pointers involved.
    unsafe { K32EmptyWorkingSet(GetCurrentProcess()) };
}

/// Directory for per-user settings (`%APPDATA%`).
pub fn config_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(std::path::PathBuf::from)
}

/// Cursor position in physical virtual-desktop coordinates.
pub fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };

    // SAFETY: `point` is a valid, writable POINT for the duration of the call.
    let ok = unsafe { GetCursorPos(&raw mut point) };

    (ok != 0).then_some((point.x, point.y))
}

/// Blocks the main thread until the app has an event to handle, pumping the Win32 messages that
/// drive the tray icon and the global hotkey meanwhile.
pub struct EventLoop {
    thread_id: u32,
}

impl EventLoop {
    /// Must be created on the thread that owns the tray icon and the hotkey manager.
    pub fn new() -> Self {
        // SAFETY: no pointers involved.
        let thread_id = unsafe { GetCurrentThreadId() };

        Self { thread_id }
    }

    pub fn waker(&self) -> Waker {
        Waker {
            thread_id: self.thread_id,
        }
    }

    /// Returns the next event, or `None` when the channel is closed or `WM_QUIT` arrives.
    #[expect(clippy::unused_self, reason = "same API as the Linux event loop")]
    pub fn next<T>(&self, events: &Receiver<T>) -> Option<T> {
        loop {
            match events.try_recv() {
                Ok(event) => return Some(event),
                Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => {}
            }

            if !pump_message() {
                return None;
            }
        }
    }
}

/// Wakes up [`EventLoop::next`] after an event was sent from another thread.
#[derive(Debug, Clone, Copy)]
pub struct Waker {
    thread_id: u32,
}

impl Waker {
    pub fn wake(self) {
        // SAFETY: no pointers involved; posting to a finished thread just fails.
        unsafe { PostThreadMessageW(self.thread_id, WM_APP, 0, 0) };
    }
}

/// Blocks until a message arrives on this thread and dispatches it.
/// Tray and hotkey callbacks run from here. Returns `false` on `WM_QUIT`.
fn pump_message() -> bool {
    // SAFETY: MSG is a plain C struct; all-zero bytes are a valid value.
    let mut msg: MSG = unsafe { std::mem::zeroed() };

    // SAFETY: `msg` is valid and writable; a null HWND means "any window of this thread".
    let ret = unsafe { GetMessageW(&raw mut msg, null_mut(), 0, 0) };
    if ret <= 0 {
        return false;
    }

    // SAFETY: `msg` was just filled in by GetMessageW.
    unsafe {
        TranslateMessage(&raw const msg);
        DispatchMessageW(&raw const msg);
    }

    true
}
