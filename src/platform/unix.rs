//! What Linux and macOS do the same way.

/// Held for as long as this process is the running instance.
pub struct InstanceLock(
    /// Nothing reads it: holding the file open is what holds the lock, and closing it, whenever
    /// and however that happens, is what releases it.
    #[expect(dead_code, reason = "the open file is the lock")]
    Option<std::fs::File>,
);

/// Marks this process as the running instance, or returns `None` when another one already is.
///
/// A `flock` on a file in the per-user runtime directory (`$XDG_RUNTIME_DIR` on Linux, `$TMPDIR`
/// on macOS): the kernel drops it when the process ends, however it ends, so a crash cannot leave
/// the app locked out of starting again.
pub fn single_instance() -> Option<InstanceLock> {
    use std::os::fd::AsRawFd as _;

    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(std::env::temp_dir, std::path::PathBuf::from);

    // Without a lock file the app runs on; it just cannot tell whether it is alone.
    let Ok(file) = std::fs::File::create(dir.join("peekr.lock")) else {
        return Some(InstanceLock(None));
    };

    // SAFETY: the descriptor is valid for as long as the file is open, which is as long as the
    // lock this returns is held.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Some(InstanceLock(Some(file)));
    }

    // Only a lock somebody else holds means another instance is up. Anything else, such as a
    // filesystem that does not support locking, is no reason to refuse to start.
    let taken = std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock;

    (!taken).then_some(InstanceLock(None))
}

/// Console output works out of the box: the process writes to the terminal it was started from.
pub fn attach_parent_console() {}

/// The home directory, where per-user files live.
pub fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(std::path::PathBuf::from)
}
