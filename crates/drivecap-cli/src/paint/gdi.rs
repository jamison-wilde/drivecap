//! Shared GDI plumbing for the painter: colour/string conversion and a
//! select-restore-delete guard so no GDI object outlives its use.

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DRAW_TEXT_FORMAT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    DeleteObject, DrawTextW, GetStockObject, HDC, HFONT, HGDIOBJ, NULL_BRUSH, NULL_PEN, PS_SOLID,
    SelectObject, SetTextColor,
};

use crate::layout::Rect;
use crate::palette::Rgb;

pub(super) fn colorref(c: Rgb) -> COLORREF {
    COLORREF(u32::from(c.0) | (u32::from(c.1) << 8) | (u32::from(c.2) << 16))
}

pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub(super) fn win_rect(r: Rect) -> RECT {
    RECT {
        left: r.x,
        top: r.y,
        right: r.right(),
        bottom: r.bottom(),
    }
}

/// Selects a GDI object into a DC for the guard's lifetime; on drop the
/// previous object is restored and, when `owned`, this one is deleted.
pub(super) struct Selected {
    hdc: HDC,
    old: HGDIOBJ,
    obj: HGDIOBJ,
    owned: bool,
}

impl Selected {
    pub(super) fn new(hdc: HDC, obj: HGDIOBJ, owned: bool) -> Selected {
        // SAFETY: `hdc` is a live DC owned by the caller; `obj` is a valid
        // pen/brush/font handle (or a stock object) for the guard's life.
        let old = unsafe { SelectObject(hdc, obj) };
        Selected {
            hdc,
            old,
            obj,
            owned,
        }
    }
}

impl Drop for Selected {
    fn drop(&mut self) {
        // SAFETY: restores the object this guard displaced, after which
        // `obj` is no longer selected and (when owned) safe to delete.
        unsafe {
            SelectObject(self.hdc, self.old);
            if self.owned {
                let _ = DeleteObject(self.obj);
            }
        }
    }
}

/// Owned brush (`None` → null brush) and pen (`None` → null pen).
pub(super) fn select_paint(
    hdc: HDC,
    brush: Option<Rgb>,
    pen: Option<(Rgb, i32)>,
) -> (Selected, Selected) {
    // SAFETY: stock objects need no cleanup; created objects are deleted
    // by the `Selected` guards that own them.
    unsafe {
        let b = match brush {
            Some(c) => Selected::new(hdc, CreateSolidBrush(colorref(c)).into(), true),
            None => Selected::new(hdc, GetStockObject(NULL_BRUSH), false),
        };
        let p = match pen {
            Some((c, w)) => Selected::new(hdc, CreatePen(PS_SOLID, w, colorref(c)).into(), true),
            None => Selected::new(hdc, GetStockObject(NULL_PEN), false),
        };
        (b, p)
    }
}

/// Single-line text in `r`, vertically centred.
pub(crate) fn text(hdc: HDC, s: &str, r: Rect, font: HFONT, color: Rgb, align: DRAW_TEXT_FORMAT) {
    if s.is_empty() || r.w <= 0 {
        return;
    }
    let _font = Selected::new(hdc, font.into(), false);
    let mut rc = win_rect(r);
    let mut ws = wide(s);
    // SAFETY: `hdc` is the painter's live DC; buffers outlive the call.
    unsafe {
        SetTextColor(hdc, colorref(color));
        DrawTextW(
            hdc,
            &mut ws,
            &mut rc,
            align | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
        );
    }
}
