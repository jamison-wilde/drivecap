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
/// `Drivecap.Show` message to the window of class [`CARD_CLASS`]. The
/// primary may still be starting (both launched at login), so the window
/// is looked for up to 10 times, 100 ms apart.
pub fn signal_primary() {
    // SAFETY: static wide string.
    let msg = unsafe { RegisterWindowMessageW(w!("Drivecap.Show")) };
    let find = || {
        // SAFETY: static class name; FindWindowW only reads it.
        unsafe { FindWindowW(CARD_CLASS, PCWSTR::null()) }.ok()
    };
    match retry(FIND_ATTEMPTS, find, || std::thread::sleep(FIND_PAUSE)) {
        Some(hwnd) => {
            // SAFETY: posting to another process's window; no pointers.
            if let Err(e) = unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0)) } {
                log::warn!("single_instance: PostMessageW(Drivecap.Show) failed: {e}");
            }
        }
        None => log::info!("single_instance: no card window to signal after {FIND_ATTEMPTS} tries"),
    }
}

/// How many times [`signal_primary`] looks for the card window.
const FIND_ATTEMPTS: u32 = 10;
/// Pause between looks (10 × 100 ms ≈ 1 s).
const FIND_PAUSE: std::time::Duration = std::time::Duration::from_millis(100);

/// Calls `find` up to `attempts` times, sleeping `pause` between misses.
fn retry<T>(
    attempts: u32,
    mut find: impl FnMut() -> Option<T>,
    mut pause: impl FnMut(),
) -> Option<T> {
    for i in 0..attempts {
        if let Some(v) = find() {
            return Some(v);
        }
        if i + 1 < attempts {
            pause();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_returns_the_first_hit_without_pausing() {
        let mut pauses = 0;
        assert_eq!(retry(10, || Some(7), || pauses += 1), Some(7));
        assert_eq!(pauses, 0);
    }

    #[test]
    fn retry_keeps_trying_until_the_window_appears() {
        let (mut calls, mut pauses) = (0, 0);
        let found = retry(
            10,
            || {
                calls += 1;
                (calls == 4).then_some("hwnd")
            },
            || pauses += 1,
        );
        assert_eq!(found, Some("hwnd"));
        assert_eq!((calls, pauses), (4, 3));
    }

    #[test]
    fn retry_gives_up_after_its_attempts() {
        let (mut calls, mut pauses) = (0, 0);
        let found: Option<()> = retry(
            10,
            || {
                calls += 1;
                None
            },
            || pauses += 1,
        );
        assert_eq!(found, None);
        assert_eq!((calls, pauses), (10, 9));
    }

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
