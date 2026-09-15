use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetCursorPos, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_APP,
};

/// Lets a GUI-subsystem executable print to the console it was started from.
pub fn attach_parent_console() {
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

/// Screen capture and window placement must work in physical pixels on every monitor.
pub fn init_dpi_awareness() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// Cursor position in physical virtual-desktop coordinates.
pub fn cursor_position() -> (i32, i32) {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

/// Index of the monitor containing the point, in the same order winit enumerates monitors.
pub fn monitor_index_at(x: i32, y: i32) -> Option<usize> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> i32 {
        let monitors = unsafe { &mut *(data as *mut Vec<HMONITOR>) };
        monitors.push(monitor);
        1
    }

    let mut monitors: Vec<HMONITOR> = Vec::new();
    unsafe {
        EnumDisplayMonitors(null_mut(), null_mut(), Some(collect), &mut monitors as *mut _ as LPARAM);
    }
    let target = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    monitors.iter().position(|&m| m == target)
}

/// Wakes up [`pump_message`] on the given thread from another thread.
#[derive(Clone, Copy)]
pub struct Waker {
    thread_id: u32,
}

impl Waker {
    pub fn for_current_thread() -> Self {
        Self { thread_id: unsafe { GetCurrentThreadId() } }
    }

    pub fn wake(&self) {
        unsafe { PostThreadMessageW(self.thread_id, WM_APP, 0, 0) };
    }
}

/// Blocks until a message arrives on this thread and dispatches it.
/// Tray and hotkey callbacks run from here. Returns `false` on `WM_QUIT`.
pub fn pump_message() -> bool {
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    let ret = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
    if ret <= 0 {
        return false;
    }
    unsafe {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    true
}
