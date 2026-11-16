//! Display theme helpers: DPI scaling and dark-mode detection.
//!
//! All UI pixel values must pass through [`scale`] so the card renders
//! correctly on high-DPI displays. All theme colors must branch on
//! [`read_dark_mode`] — never assume dark.
//!
//! Unlike exbar (which cached the answer for the process lifetime),
//! [`read_dark_mode`] is **uncached**: drivecap repaints on theme change
//! (`WM_SETTINGCHANGE` / `ImmersiveColorSet`), so every call re-reads the
//! registry. The read is a single `RegGetValueW` — cheap enough for
//! paint-time use, but callers should still read once per repaint, not
//! per element.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::core::w;

/// Scale a logical pixel value by the given DPI.
/// 96 DPI = 100% scaling.
pub fn scale(logical_px: i32, dpi: u32) -> i32 {
    ((i64::from(logical_px) * i64::from(dpi)) / 96) as i32
}

/// Query the system "apps use dark mode" setting. Uncached: reads the
/// registry on every call so a theme switch takes effect on the next
/// repaint. Returns true if apps are set to dark mode; any read failure
/// is treated as light.
pub fn read_dark_mode() -> bool {
    let mut data: u32 = 1; // default to light (1 = light)
    let mut size = std::mem::size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut std::ffi::c_void),
            Some(&mut size),
        )
    };
    // data == 0 means dark mode; data == 1 means light mode
    let dark = result.is_ok() && data == 0;
    log::debug!(
        "theme: AppsUseLightTheme registry read ok={} raw_value={} dark_mode={}",
        result.is_ok(),
        data,
        dark
    );
    dark
}

/// Get the DPI for a specific window. Returns 96 if the call fails.
pub fn get_dpi(hwnd: HWND) -> u32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 { 96 } else { dpi }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_at_100_percent() {
        assert_eq!(scale(24, 96), 24);
    }

    #[test]
    fn scale_at_150_percent() {
        assert_eq!(scale(24, 144), 36);
    }

    #[test]
    fn scale_at_200_percent() {
        assert_eq!(scale(24, 192), 48);
    }

    #[test]
    fn scale_at_125_percent() {
        // 24 * 120 / 96 = 30
        assert_eq!(scale(24, 120), 30);
    }

    #[test]
    fn scale_handles_negative() {
        assert_eq!(scale(-4, 144), -6);
    }

    #[test]
    fn read_dark_mode_does_not_panic() {
        // Actual value depends on the system setting; repeated calls must
        // all succeed (uncached — each one re-reads the registry).
        let _ = read_dark_mode();
        let _ = read_dark_mode();
    }

    #[test]
    fn get_dpi_falls_back_to_96_for_null_hwnd() {
        assert_eq!(get_dpi(HWND::default()), 96);
    }
}
