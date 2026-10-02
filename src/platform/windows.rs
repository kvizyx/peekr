use std::io;
use std::path::Path;
use std::ptr::null_mut;
use std::sync::mpsc::{Receiver, TryRecvError};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, FALSE, GetLastError, HANDLE, HWND, LPARAM,
    POINT, TRUE, WIN32_ERROR,
};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE, RegCloseKey,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, GetCurrentThreadId};
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GWL_STYLE, GetCursorPos, GetMessageW, GetWindowLongPtrW, HWND_BROADCAST, MSG, PostThreadMessageW,
    SMTO_ABORTIFHUNG, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageTimeoutW,
    SetWindowLongPtrW, SetWindowPos, TranslateMessage, WM_APP, WM_SETTINGCHANGE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
    WS_SYSMENU,
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

/// Puts `dir` on the user's `PATH`, unless it is there already. Consoles opened from then on find
/// the programs in it; one that is already open keeps the `PATH` it started with.
pub fn add_to_path(dir: &Path) {
    edit_user_path(dir, true);
}

/// Takes `dir` off the user's `PATH`.
pub fn remove_from_path(dir: &Path) {
    edit_user_path(dir, false);
}

fn edit_user_path(dir: &Path, present: bool) {
    if let Err(e) = try_edit_user_path(dir, present) {
        log::warn!("cannot update PATH: {e}");
    }
}

fn try_edit_user_path(dir: &Path, present: bool) -> io::Result<()> {
    let key = RegistryKey::open_environment()?;

    // Usually REG_EXPAND_SZ, so that entries such as `%USERPROFILE%\bin` keep working; whatever
    // it is, it stays.
    let (kind, path) = key.string("Path")?.unwrap_or((REG_EXPAND_SZ, String::new()));

    let Some(path) = edited_path(&path, &dir.to_string_lossy(), present) else {
        return Ok(());
    };
    key.set_string("Path", kind, &path)?;

    // Tells Explorer, and the consoles it starts, to read the environment again.
    let environment = wide("Environment");
    // SAFETY: the string is null-terminated and outlives the call; the result is not needed.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment.as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            1000,
            null_mut(),
        )
    };

    Ok(())
}

/// `path` with `dir` added or removed, or `None` when that would change nothing.
fn edited_path(path: &str, dir: &str, present: bool) -> Option<String> {
    // Windows treats paths case-insensitively, and does not care for a trailing separator.
    let normalized = |entry: &str| entry.trim().trim_end_matches('\\').to_lowercase();
    let dir_normalized = normalized(dir);

    let mut entries: Vec<&str> = path.split(';').filter(|entry| !entry.trim().is_empty()).collect();
    let listed = entries.iter().any(|entry| normalized(entry) == dir_normalized);

    match (present, listed) {
        (true, false) => entries.push(dir),
        (false, true) => entries.retain(|entry| normalized(entry) != dir_normalized),
        _ => return None,
    }

    Some(entries.join(";"))
}

/// `HKEY_CURRENT_USER\Environment`, closed when dropped.
struct RegistryKey(HKEY);

impl RegistryKey {
    fn open_environment() -> io::Result<Self> {
        let path = wide("Environment");
        let mut key: HKEY = null_mut();

        // SAFETY: the path is null-terminated, and `key` is only written on success.
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                &raw mut key,
            )
        };

        check(opened)?;
        Ok(Self(key))
    }

    /// A string value and its kind, or `None` when there is no such value.
    fn string(&self, name: &str) -> io::Result<Option<(REG_VALUE_TYPE, String)>> {
        let name = wide(name);
        let mut kind = 0;
        let mut bytes = 0;

        // SAFETY: the name is null-terminated; a null buffer asks for the size, written to `bytes`.
        let sized = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                null_mut(),
                &raw mut kind,
                null_mut(),
                &raw mut bytes,
            )
        };
        if sized == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(sized)?;

        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "PATH is not a string"));
        }

        // One unit of slack: a value stored without its terminator still has room for one.
        let mut buffer = vec![0u16; bytes as usize / size_of::<u16>() + 1];
        let mut bytes = size_of_val(buffer.as_slice()) as u32;

        // SAFETY: the buffer holds `bytes` writable bytes, which is what RegQueryValueExW is told.
        let read = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                null_mut(),
                &raw mut kind,
                buffer.as_mut_ptr().cast(),
                &raw mut bytes,
            )
        };
        check(read)?;

        buffer.truncate(bytes as usize / size_of::<u16>());
        let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());

        Ok(Some((kind, String::from_utf16_lossy(&buffer[..end]))))
    }

    fn set_string(&self, name: &str, kind: REG_VALUE_TYPE, value: &str) -> io::Result<()> {
        let name = wide(name);
        let value = wide(value);
        let bytes = size_of_val(value.as_slice()) as u32;

        // SAFETY: both strings are null-terminated and outlive the call, and `bytes` is the length
        // of `value` in bytes, its terminator included, which is what a string value expects.
        let set = unsafe { RegSetValueExW(self.0, name.as_ptr(), 0, kind, value.as_ptr().cast(), bytes) };

        check(set)
    }
}

impl Drop for RegistryKey {
    fn drop(&mut self) {
        // SAFETY: the key comes from RegOpenKeyExW and is closed exactly once.
        unsafe { RegCloseKey(self.0) };
    }
}

fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
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

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = r"C:\Users\me\AppData\Local\peekr\current";

    #[test]
    fn a_directory_is_added_once() {
        assert_eq!(
            edited_path(r"C:\tools;%USERPROFILE%\bin", DIR, true).as_deref(),
            Some(r"C:\tools;%USERPROFILE%\bin;C:\Users\me\AppData\Local\peekr\current")
        );
        assert_eq!(edited_path("", DIR, true).as_deref(), Some(DIR), "an empty PATH");

        let listed = r"C:\tools;c:\users\me\appdata\local\peekr\current\";
        assert_eq!(
            edited_path(listed, DIR, true),
            None,
            "already there, in another case and with a trailing separator"
        );
    }

    #[test]
    fn a_directory_is_removed_and_nothing_else_is() {
        let path = format!(r"C:\tools;{DIR};%USERPROFILE%\bin;");

        assert_eq!(
            edited_path(&path, DIR, false).as_deref(),
            Some(r"C:\tools;%USERPROFILE%\bin")
        );
        assert_eq!(edited_path(r"C:\tools", DIR, false), None, "not there to begin with");
    }
}
