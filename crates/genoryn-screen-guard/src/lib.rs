//! Keeps a window's contents out of screenshots, screen recordings and screen
//! sharing while it shows a secret such as a recovery phrase.
//!
//! Windows supports this natively (`SetWindowDisplayAffinity`). Other systems
//! offer no such switch to applications, so [`protect`] reports
//! [`Protection::Unavailable`] there and the caller can say so plainly. No
//! protection of this kind stops a camera, a keylogger or malware that already
//! controls the computer.
#![deny(unsafe_op_in_unsafe_fn)]

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    /// Captures leave the window out entirely (Windows 10 version 2004 and later).
    Excluded,
    /// Captures show the window as a black area (earlier Windows versions).
    Blacked,
    /// This system cannot keep the window out of captures.
    Unavailable,
}

impl Protection {
    pub fn is_active(self) -> bool {
        self != Self::Unavailable
    }
}

/// Keeps `window` out of screen captures until [`release`] is called.
pub fn protect(window: &impl HasWindowHandle) -> Protection {
    match native(window) {
        #[cfg(windows)]
        Some(hwnd) => win32::protect(hwnd),
        _ => Protection::Unavailable,
    }
}

/// Lets `window` be captured normally again.
pub fn release(window: &impl HasWindowHandle) {
    #[cfg(windows)]
    if let Some(hwnd) = native(window) {
        win32::release(hwnd);
    }
    #[cfg(not(windows))]
    let _ = window;
}

fn native(window: &impl HasWindowHandle) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

#[cfg(windows)]
mod win32 {
    use super::Protection;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_MONITOR, WDA_NONE,
    };

    pub(super) fn protect(hwnd: isize) -> Protection {
        // SAFETY: `hwnd` is this process's own live top-level window, taken from
        // the windowing library for the current frame; the call only changes how
        // the system compositor may capture it and has no memory effects.
        unsafe {
            if SetWindowDisplayAffinity(hwnd as _, WDA_EXCLUDEFROMCAPTURE) != 0 {
                Protection::Excluded
            } else if SetWindowDisplayAffinity(hwnd as _, WDA_MONITOR) != 0 {
                Protection::Blacked
            } else {
                Protection::Unavailable
            }
        }
    }

    pub(super) fn release(hwnd: isize) {
        // SAFETY: as in `protect`.
        unsafe {
            SetWindowDisplayAffinity(hwnd as _, WDA_NONE);
        }
    }
}
