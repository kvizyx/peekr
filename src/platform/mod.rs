//! OS integration. Only Windows is implemented for now.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
