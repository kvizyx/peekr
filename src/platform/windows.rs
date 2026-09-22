use std::ptr::null_mut;
use std::sync::mpsc::{Receiver, TryRecvError};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, FALSE, GetLastError, HANDLE, HWND, POINT, TRUE,
};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, GetCurrentThreadId};
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GWL_STYLE, GetCursorPos, GetMessageW, GetWindowLongPtrW, MSG, PostThreadMessageW,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos,
    TranslateMessage, WM_APP, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU,
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

/// Held for as long as this process is the running instance.
pub struct InstanceLock(HANDLE);

impl Drop for InstanceLock {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle comes from CreateMutexW and is closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Marks this process as the running instance, or returns `None` when another one already is.
///
/// A named mutex in the `Local\` namespace, so it is per-session: two users on the same machine
/// each get their own peekr, and Windows releases the name when the process ends however it ends.
pub fn single_instance() -> Option<InstanceLock> {
    let name = wide(r"Local\peekr-single-instance");

    // SAFETY: the name is a valid null-terminated wide string; default security attributes.
    let mutex = unsafe { CreateMutexW(null_mut(), FALSE, name.as_ptr()) };

    // SAFETY: no pointers involved.
    if !mutex.is_null() && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: the handle comes from CreateMutexW and is not used again.
        unsafe { CloseHandle(mutex) };
        return None;
    }

    // A failed CreateMutexW leaves a null handle: the app runs on, it just cannot tell whether
    // it is alone.
    Some(InstanceLock(mutex))
}

/// Where the installer records this app for "Apps & features", and where winget reads the
/// installed version from. The Inno Setup `AppId`, as in `xtask/installer/peekr.iss`.
const UNINSTALL_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{6F3B0E4A-2C55-4F35-9C0C-6C8F63A0F3D1}_is1";

/// Tells the system that the installation in `dir` is at `version` now.
///
/// The app replaces its own files, which leaves what the installer wrote about it untouched:
/// "Apps & features" would go on showing the version that was installed, and winget, which reads
/// the same value, would go on offering an upgrade to a version already in place — or install an
/// older one over a newer one.
///
/// Only the copy the entry describes may correct it. A missing entry means this copy was unpacked
/// rather than installed, and one that points elsewhere belongs to an installation of its own,
/// which a portable copy running beside it has no business renaming.
pub fn record_installed_version(dir: &std::path::Path, version: &str) {
    let path = wide(UNINSTALL_KEY);
    let mut key: HKEY = null_mut();

    // SAFETY: the path is a valid null-terminated wide string, and `key` is only written on success.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            &raw mut key,
        )
    };
    if opened != ERROR_SUCCESS {
        return;
    }

    if registry_string(key, "InstallLocation").is_some_and(|installed| is_same_dir(&installed, dir)) {
        set_registry_string(key, "DisplayVersion", version);
        // Inno writes the name with the version in it, so it has to follow along.
        set_registry_string(key, "DisplayName", &format!("Peekr {version}"));
    }

    // SAFETY: the key comes from RegOpenKeyExW and is closed exactly once.
    unsafe { RegCloseKey(key) };
}

/// Whether a path the installer recorded, which ends in a separator, names `dir`.
fn is_same_dir(recorded: &str, dir: &std::path::Path) -> bool {
    let recorded = std::path::Path::new(recorded.trim_end_matches(std::path::is_separator));

    match (recorded.canonicalize(), dir.canonicalize()) {
        (Ok(recorded), Ok(dir)) => recorded == dir,
        // An installation that is no longer there is not this one.
        _ => false,
    }
}

fn registry_string(key: HKEY, name: &str) -> Option<String> {
    let name = wide(name);
    let mut kind = 0;
    let mut bytes = 0;

    // SAFETY: the name is null-terminated; a null buffer asks for the size, written to `bytes`.
    let sized = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            null_mut(),
            &raw mut kind,
            null_mut(),
            &raw mut bytes,
        )
    };
    if sized != ERROR_SUCCESS || kind != REG_SZ || bytes == 0 {
        return None;
    }

    // One unit of slack: a value stored without its terminator still has room for one.
    let mut buffer = vec![0u16; bytes as usize / size_of::<u16>() + 1];
    let mut bytes = size_of_val(buffer.as_slice()) as u32;

    // SAFETY: the buffer holds `bytes` writable bytes, which is what RegQueryValueExW is told.
    let read = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            null_mut(),
            &raw mut kind,
            buffer.as_mut_ptr().cast(),
            &raw mut bytes,
        )
    };
    if read != ERROR_SUCCESS {
        return None;
    }

    buffer.truncate(bytes as usize / size_of::<u16>());
    let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());

    Some(String::from_utf16_lossy(&buffer[..end]))
}

fn set_registry_string(key: HKEY, name: &str, value: &str) {
    let wide_name = wide(name);
    let value = wide(value);
    let bytes = size_of_val(value.as_slice()) as u32;

    // SAFETY: both strings are null-terminated and outlive the call, and `bytes` is the length of
    // `value` in bytes, including its terminator, which is what REG_SZ expects.
    let set = unsafe { RegSetValueExW(key, wide_name.as_ptr(), 0, REG_SZ, value.as_ptr().cast(), bytes) };

    if set != ERROR_SUCCESS {
        log::warn!("cannot record {name} in the registry: error {set}");
    }
}

/// A null-terminated UTF-16 string, the only kind the `W` functions take.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Hands a window that draws its own frame the rest of what the system would draw for it: rounds
/// its corners by `radius` physical pixels, and takes away the caption it no longer has.
///
/// A window region is the only way to round a window whose pixels are opaque; Windows 11 rounds
/// frames itself, but only by its own small radius, and only where the system frame is still on.
///
/// The caption matters because an undecorated window on Windows keeps its caption styles: the
/// frame is hidden by the compositor rather than taken away. The system's move loop still paints
/// what those styles promise, so the minimise and close buttons flash over a window being dragged.
pub fn use_own_frame(window: &winit::window::Window, radius: u32) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };

    // SAFETY: the handle belongs to a live window; reading and writing its style is safe.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let caption = (WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX) as isize;

        SetWindowLongPtrW(hwnd, GWL_STYLE, style & !caption);
        SetWindowPos(
            hwnd,
            null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }

    let size = window.inner_size();
    let (width, height) = (size.width as i32, size.height as i32);
    let diameter = 2 * radius as i32;

    // SAFETY: no pointers involved; the corners are exclusive, hence the extra pixel.
    let region = unsafe { CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter) };
    if region.is_null() {
        return;
    }

    // SAFETY: the handle belongs to a live window, and the region is a fresh one, which
    // SetWindowRgn takes ownership of: it must not be deleted here.
    unsafe { SetWindowRgn(hwnd, region, TRUE) };
}

/// The Win32 handle behind a winit window.
fn hwnd(window: &winit::window::Window) -> Option<HWND> {
    use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };

    Some(handle.hwnd.get() as HWND)
}

/// Stops Windows from animating the window when it is shown and hidden. The animation scales and
/// fades the window, which for a screen-sized overlay looks like the whole screen jumping.
pub fn disable_window_animations(window: &winit::window::Window) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };

    let disabled: windows_sys::core::BOOL = TRUE;

    // SAFETY: the handle belongs to a live window, and `disabled` is a BOOL that outlives the call,
    // which is what DWMWA_TRANSITIONS_FORCEDISABLED expects.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED as u32,
            (&raw const disabled).cast(),
            size_of::<windows_sys::core::BOOL>() as u32,
        );
    }
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
