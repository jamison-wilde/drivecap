//! Font picker: the standard `ChooseFontW` dialog, seeded with the card's
//! current font, returning a [`FontSpec`] (face, point size, weight).
//!
//! The dialog is modal and pumps messages: callers run [`choose_font`]
//! outside any `AppState` borrow. It is user-initiated, so its hook brings
//! it to the foreground (the card itself never activates).

use std::panic::AssertUnwindSafe;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    DEFAULT_CHARSET, GetDC, GetDeviceCaps, LOGFONTW, LOGPIXELSY, ReleaseDC,
};
use windows::Win32::UI::Controls::Dialogs::{
    CF_ENABLEHOOK, CF_INITTOLOGFONTSTRUCT, CF_NOVERTFONTS, CF_SCREENFONTS, CHOOSEFONTW, ChooseFontW,
};
use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, WM_INITDIALOG};

use crate::config::FontSpec;

/// `LOGFONTW.lfHeight` for `point_size` at `dpi_y`: negative (character
/// height), rounded like `MulDiv(pt, dpi, 72)`.
pub fn logfont_height(point_size: u32, dpi_y: i32) -> i32 {
    let pt = i64::from(point_size);
    -((pt * i64::from(dpi_y) + 36) / 72) as i32
}

/// The chosen font: face from `lfFaceName` (NUL-terminated), size from
/// `iPointSize` (tenths of a point, rounded, at least 1), weight from
/// `lfWeight` (`FW_DONTCARE` → 400).
pub fn spec_from(face: &[u16], point_tenths: i32, weight: i32) -> FontSpec {
    let end = face.iter().position(|&c| c == 0).unwrap_or(face.len());
    FontSpec {
        face: String::from_utf16_lossy(&face[..end]),
        point_size: ((point_tenths + 5) / 10).max(1) as u32,
        weight: if weight <= 0 {
            400
        } else {
            weight.min(1000) as u32
        },
    }
}

/// Seed `LOGFONTW` for `spec`; the face is truncated to fit `lfFaceName`.
fn logfont_for(spec: &FontSpec, dpi_y: i32) -> LOGFONTW {
    let mut lf = LOGFONTW {
        lfHeight: logfont_height(spec.point_size, dpi_y),
        lfWeight: spec.weight as i32,
        lfCharSet: DEFAULT_CHARSET,
        ..Default::default()
    };
    let max = lf.lfFaceName.len() - 1;
    for (dst, src) in lf
        .lfFaceName
        .iter_mut()
        .zip(spec.face.encode_utf16().take(max))
    {
        *dst = src;
    }
    lf
}

/// Vertical DPI of the screen DC (what `ChooseFontW` converts with).
fn screen_dpi_y() -> i32 {
    // SAFETY: the screen DC is released before returning.
    unsafe {
        let dc = GetDC(None);
        let dpi = GetDeviceCaps(Some(dc), LOGPIXELSY);
        ReleaseDC(None, dc);
        if dpi > 0 { dpi } else { 96 }
    }
}

/// Dialog hook: bring the (user-requested) dialog to the foreground.
unsafe extern "system" fn hook(dlg: HWND, msg: u32, _: WPARAM, _: LPARAM) -> usize {
    let r = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if msg == WM_INITDIALOG {
            // SAFETY: the dialog's own live window, during its init.
            let _ = unsafe { SetForegroundWindow(dlg) };
            return 1;
        }
        0
    }));
    r.unwrap_or_else(|_| {
        log::error!("font dialog hook panicked on msg {msg:#06x}");
        0
    })
}

/// Shows `ChooseFontW` owned by `owner`, seeded with `current` (else the
/// default Segoe UI 9 pt). `None` when cancelled or on failure.
pub fn choose_font(owner: HWND, current: Option<&FontSpec>) -> Option<FontSpec> {
    let fallback = FontSpec::default();
    let mut lf = logfont_for(current.unwrap_or(&fallback), screen_dpi_y());
    let mut cf = CHOOSEFONTW {
        lStructSize: size_of::<CHOOSEFONTW>() as u32,
        hwndOwner: owner,
        lpLogFont: &mut lf,
        Flags: CF_INITTOLOGFONTSTRUCT | CF_SCREENFONTS | CF_NOVERTFONTS | CF_ENABLEHOOK,
        lpfnHook: Some(hook),
        ..Default::default()
    };
    // SAFETY: `cf` is fully initialised and points at `lf`; both outlive
    // the modal call.
    let ok = unsafe { ChooseFontW(&mut cf) }.as_bool();
    if !ok {
        log::info!("font dialog: cancelled");
        return None;
    }
    let spec = spec_from(&lf.lfFaceName, cf.iPointSize, lf.lfWeight);
    log::info!(
        "font dialog: {} {} pt, weight {}",
        spec.face,
        spec.point_size,
        spec.weight
    );
    Some(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0, 0x41]).collect()
    }

    #[test]
    fn logfont_height_matches_muldiv() {
        assert_eq!(logfont_height(9, 96), -12);
        assert_eq!(logfont_height(9, 144), -18);
        assert_eq!(logfont_height(11, 96), -15); // 14.67 rounds to 15
    }

    #[test]
    fn spec_comes_from_logfont_and_tenths() {
        let s = spec_from(&wide("Consolas"), 105, 700);
        assert_eq!(
            s,
            FontSpec {
                face: "Consolas".into(),
                point_size: 11,
                weight: 700
            }
        );
        assert_eq!(spec_from(&wide("X"), 90, 0).weight, 400);
        assert_eq!(spec_from(&wide("X"), 2, 400).point_size, 1);
    }

    #[test]
    fn seed_logfont_round_trips() {
        let spec = FontSpec {
            face: "Segoe UI".into(),
            point_size: 9,
            weight: 600,
        };
        let lf = logfont_for(&spec, 96);
        assert_eq!(lf.lfHeight, -12);
        assert_eq!(spec_from(&lf.lfFaceName, 90, lf.lfWeight), spec);
        let long = FontSpec {
            face: "x".repeat(60),
            ..spec
        };
        assert_eq!(logfont_for(&long, 96).lfFaceName[31], 0);
    }
}
