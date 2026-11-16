//! The card's fonts and the GDI-backed [`TextMeasure`].

use std::ffi::c_void;

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_GUI_FONT, DeleteDC, DeleteObject, GetObjectW, GetStockObject, GetTextExtentPoint32W,
    HDC, HFONT, LOGFONTW, OUT_DEFAULT_PRECIS,
};
use windows_core::PCWSTR;

use super::gdi::{Selected, wide};
use crate::config::FontSpec;
use crate::layout::{Metrics, TextMeasure, TextStyle};

/// Default body text height (logical px) when no `FontSpec` is set.
const BODY_PX: f32 = 12.0;

/// The card's fonts: body, semibold, bold, small and glyph.
pub struct Fonts {
    body: HFONT,
    semibold: HFONT,
    bold: HFONT,
    small: HFONT,
    glyph: HFONT,
}

impl Fonts {
    /// Face from `spec` or the `DEFAULT_GUI_FONT` face. Heights: body
    /// `12·s` px (or `point_size·96/72·s`), small body−1, glyph body−2.
    /// Weights: body `spec.weight` (400), semibold ≥ 600, bold ≥ 700,
    /// glyph 600.
    pub fn new(spec: Option<&FontSpec>, m: &Metrics) -> Fonts {
        let face = match spec {
            Some(f) => wide(&f.face).into_iter().chain([0]).collect(),
            None => default_gui_face(),
        };
        let body_px = spec.map_or(BODY_PX, |f| f.point_size as f32 * 96.0 / 72.0);
        let weight = spec.map_or(400, |f| f.weight as i32);
        let mk = |logical: f32, w: i32| create_font(&face, -((logical * m.s).round() as i32), w);
        Fonts {
            body: mk(body_px, weight),
            semibold: mk(body_px, weight.max(600)),
            bold: mk(body_px, weight.max(700)),
            small: mk(body_px - 1.0, weight),
            glyph: mk(body_px - 2.0, 600),
        }
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
        for f in [self.body, self.semibold, self.bold, self.small, self.glyph] {
            // SAFETY: each font was created by `Fonts::new`, is owned only
            // here, and is never left selected (guards restore on drop).
            unsafe {
                let _ = DeleteObject(f.into());
            }
        }
    }
}

/// NUL-terminated face name of `DEFAULT_GUI_FONT`, or `Segoe UI`.
fn default_gui_face() -> Vec<u16> {
    let mut lf = LOGFONTW::default();
    // SAFETY: stock objects need no cleanup; GetObjectW writes at most
    // `size_of::<LOGFONTW>()` bytes into `lf`.
    let n = unsafe {
        GetObjectW(
            GetStockObject(DEFAULT_GUI_FONT),
            size_of::<LOGFONTW>() as i32,
            Some(&mut lf as *mut LOGFONTW as *mut c_void),
        )
    };
    if n == 0 || lf.lfFaceName[0] == 0 {
        return wide("Segoe UI").into_iter().chain([0]).collect();
    }
    lf.lfFaceName.to_vec()
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
    pub fn new(fonts: &'a Fonts) -> GdiMeasure<'a> {
        // SAFETY: a memory DC compatible with the screen; deleted on drop.
        let hdc = unsafe { CreateCompatibleDC(None) };
        GdiMeasure { fonts, hdc }
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
