//! Single-instance guard. A named mutex decides which process owns the
//! card; a second launch (Start Menu shortcut, a second Run-key start,
//! the MSI's "Launch now") posts the registered `Drivecap.Show` message to
//! the primary's card window and exits 0. The primary treats it as
//! [`Input::Relaunched`](crate::controller::Input::Relaunched).

use std::os::windows::io::{FromRawHandle, OwnedHandle};

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};
use windows_core::{HSTRING, PCWSTR, w};

use crate::card_wnd::CARD_CLASS;

/// Outcome of [`acquire`].
#[derive(Debug)]
pub enum Instance {
    /// This process owns the mutex; keep the handle alive while running.
    Primary(OwnedHandle),
    /// Another process already holds it.
    Secondary,
}

/// Opens or creates the mutex `name`. An already-existing mutex means
/// another instance is running. If the named mutex cannot be created the
/// process runs as primary on an unnamed one (better two cards than
/// none); if no mutex can be created at all, it reports `Secondary`.
pub fn acquire(name: &str) -> Instance {
    let wide = HSTRING::from(name);
    // SAFETY: `wide` outlives the call; default security, not owned.
    let handle = match unsafe { CreateMutexW(None, false, &wide) } {
        Ok(h) => h,
        Err(e) => {
            log::warn!("single_instance: CreateMutexW({name}) failed ({e}); running anyway");
            return unguarded();
        }
    };
    // SAFETY: no pointers; reads this thread's last-error, set by CreateMutexW.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: `handle` is ours and closed exactly once here.
        let _ = unsafe { CloseHandle(handle) };
        return Instance::Secondary;
    }
    // SAFETY: a fresh, valid handle that nothing else owns.
    Instance::Primary(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

/// Primary without a guard: holds an unnamed mutex so the variant still
/// carries a handle.
fn unguarded() -> Instance {
    // SAFETY: unnamed mutex; the returned handle owns it.
    match unsafe { CreateMutexW(None, false, PCWSTR::null()) } {
        // SAFETY: a fresh, valid handle that nothing else owns.
        Ok(h) => Instance::Primary(unsafe { OwnedHandle::from_raw_handle(h.0) }),
        Err(e) => {
            log::error!("single_instance: cannot create any mutex ({e}); exiting");
            Instance::Secondary
        }
    }
}

/// Tells the running instance to show its card: posts the registered
/// `Drivecap.Show` message to the window of class
/// [`CARD_CLASS`]. No-op (logged) when the primary has no window yet.
pub fn signal_primary() {
    // SAFETY: static wide strings; FindWindowW only reads them.
    let msg = unsafe { RegisterWindowMessageW(w!("Drivecap.Show")) };
    match unsafe { FindWindowW(CARD_CLASS, PCWSTR::null()) } {
        Ok(hwnd) => {
            // SAFETY: posting to another process's window; no pointers.
            if let Err(e) = unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0)) } {
                log::warn!("single_instance: PostMessageW(Drivecap.Show) failed: {e}");
            }
        }
        Err(e) => log::info!("single_instance: no card window to signal ({e})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_is_secondary() {
        let name = format!("Local\\Drivecap-Test-{}-second", std::process::id());
        let first = acquire(&name);
        assert!(matches!(first, Instance::Primary(_)));
        assert!(matches!(acquire(&name), Instance::Secondary));
        drop(first);
    }

    #[test]
    fn released_mutex_can_be_reacquired() {
        let name = format!("Local\\Drivecap-Test-{}-release", std::process::id());
        drop(acquire(&name));
        assert!(matches!(acquire(&name), Instance::Primary(_)));
    }
}
