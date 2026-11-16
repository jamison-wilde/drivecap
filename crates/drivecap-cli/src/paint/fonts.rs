//! The card's fonts and the GDI-backed [`TextMeasure`].

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateFontW, DEFAULT_CHARSET,
    DeleteDC, DeleteObject, GetTextExtentPoint32W, HDC, HFONT, OUT_DEFAULT_PRECIS,
};
use windows_core::PCWSTR;

use super::gdi::{Selected, wide};
use crate::config::FontSpec;
use crate::error::DrivecapResult;
use crate::layout::{Metrics, TextMeasure, TextStyle};

/// The card's fonts: body, semibold, bold, small and glyph.
pub struct Fonts {
    body: HFONT,
    semibold: HFONT,
    bold: HFONT,
    small: HFONT,
    glyph: HFONT,
}

impl Fonts {
    /// Face, size and weight from `spec`, else [`FontSpec::default()`]
    /// (Segoe UI 9 pt, 400). Heights: body `point_size·96/72·s` px (12·s
    /// at 9 pt), small body−1, glyph body−2. Weights: body `spec.weight`,
    /// semibold ≥ 600, bold ≥ 700, glyph 600. Fails if GDI cannot create
    /// a font; any fonts already made are deleted.
    pub fn new(spec: Option<&FontSpec>, m: &Metrics) -> DrivecapResult<Fonts> {
        let fallback = FontSpec::default();
        let spec = spec.unwrap_or(&fallback);
        let face: Vec<u16> = wide(&spec.face).into_iter().chain([0]).collect();
        let body = spec.point_size as f32 * 96.0 / 72.0;
        let weight = spec.weight as i32;
        let plan = [
            (body, weight),
            (body, weight.max(600)),
            (body, weight.max(700)),
            (body - 1.0, weight),
            (body - 2.0, 600),
        ];
        let mut made: Vec<HFONT> = Vec::with_capacity(plan.len());
        for (logical, w) in plan {
            let f = create_font(&face, -((logical * m.s).round() as i32), w);
            if f.is_invalid() {
                let err = windows::core::Error::from_win32();
                made.into_iter().for_each(delete_font);
                return Err(err.into());
            }
            made.push(f);
        }
        Ok(Fonts {
            body: made[0],
            semibold: made[1],
            bold: made[2],
            small: made[3],
            glyph: made[4],
        })
    }

    pub fn get(&self, style: TextStyle) -> HFONT {
        match style {
            TextStyle::Body => self.body,
            TextStyle::BodySemibold => self.semibold,
            TextStyle::BodyBold => self.bold,
            TextStyle::Small => self.small,
            TextStyle::Glyph => self.glyph,
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        [self.body, self.semibold, self.bold, self.small, self.glyph]
            .into_iter()
            .for_each(delete_font);
    }
}

/// Deletes a font owned by [`Fonts`] that is not selected anywhere.
fn delete_font(f: HFONT) {
    // SAFETY: each font was created by `Fonts::new`, is owned only by it,
    // and is never left selected (guards restore on drop).
    unsafe {
        let _ = DeleteObject(f.into());
    }
}

fn create_font(face: &[u16], height: i32, weight: i32) -> HFONT {
    // SAFETY: `face` is NUL-terminated and outlives the call.
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            PCWSTR(face.as_ptr()),
        )
    }
}

/// [`TextMeasure`] backed by GDI and a private memory DC.
pub struct GdiMeasure<'a> {
    fonts: &'a Fonts,
    hdc: HDC,
}

impl<'a> GdiMeasure<'a> {
    /// Creates the private measuring DC; fails if GDI cannot allocate one.
    pub fn new(fonts: &'a Fonts) -> DrivecapResult<GdiMeasure<'a>> {
        // SAFETY: a memory DC compatible with the screen; deleted on drop.
        let hdc = unsafe { CreateCompatibleDC(None) };
        if hdc.is_invalid() {
            return Err(windows::core::Error::from_win32().into());
        }
        Ok(GdiMeasure { fonts, hdc })
    }
}

impl TextMeasure for GdiMeasure<'_> {
    fn width(&self, text: &str, style: TextStyle) -> i32 {
        let _font = Selected::new(self.hdc, self.fonts.get(style).into(), false);
        let mut size = SIZE::default();
        // SAFETY: `self.hdc` is live; the slice and `size` outlive the call.
        let ok = unsafe { GetTextExtentPoint32W(self.hdc, &wide(text), &mut size) };
        if ok.as_bool() { size.cx } else { 0 }
    }
}

impl Drop for GdiMeasure<'_> {
    fn drop(&mut self) {
        // SAFETY: the DC was created in `new` and nothing remains selected.
        unsafe {
            let _ = DeleteDC(self.hdc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use windows::Win32::Graphics::Gdi::{GetObjectW, LOGFONTW};

    fn logfont(f: HFONT) -> LOGFONTW {
        let mut lf = LOGFONTW::default();
        // SAFETY: GetObjectW writes at most size_of::<LOGFONTW>() bytes into lf.
        unsafe {
            GetObjectW(
                f.into(),
                size_of::<LOGFONTW>() as i32,
                Some(&mut lf as *mut LOGFONTW as *mut c_void),
            )
        };
        lf
    }

    #[test]
    fn default_is_segoe_ui_9pt() {
        let fonts = Fonts::new(None, &Metrics::new(96, 100)).unwrap();
        let lf = logfont(fonts.get(TextStyle::Body));
        let end = lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(32);
        assert_eq!(String::from_utf16_lossy(&lf.lfFaceName[..end]), "Segoe UI");
        assert_eq!(lf.lfHeight, -12);
        assert_eq!(logfont(fonts.get(TextStyle::BodySemibold)).lfWeight, 600);
    }
}
