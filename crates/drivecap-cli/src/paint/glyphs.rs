//! Header glyphs (Builder's calls): opacity = circle outline with the
//! left half filled; gear = 8-tooth polygon with a hole; chevron =
//! two-segment polyline. Drawn with GDI over the button's opaque fill.

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{Ellipse, HDC, Polygon, Polyline};

use super::ButtonLook;
use super::gdi::select_paint;
use crate::layout::{Metrics, Rect};

/// Glyph stroke width: 1.5 logical px, rounded down, at least 1.
pub(super) fn stroke(m: &Metrics) -> i32 {
    (m.px(3) / 2).max(1)
}

/// Circle outline with its left half filled.
pub(super) fn glyph_opacity(hdc: HDC, r: Rect, look: &ButtonLook, m: &Metrics) {
    let d = m.px(12);
    let (x, y) = (r.x + (r.w - d) / 2, r.y + (r.h - d) / 2);
    let (cx, cy, rad) = (
        x as f32 + d as f32 / 2.0,
        y as f32 + d as f32 / 2.0,
        d as f32 / 2.0,
    );
    let half: Vec<POINT> = (0..=16)
        .map(|i| {
            let t = std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 16.0;
            POINT {
                x: (cx + rad * t.cos()).round() as i32,
                y: (cy - rad * t.sin()).round() as i32,
            }
        })
        .collect();
    {
        let _g = select_paint(hdc, Some(look.glyph), None);
        // SAFETY: live DC with a brush and null pen selected.
        unsafe {
            let _ = Polygon(hdc, &half);
        }
    }
    let _g = select_paint(hdc, None, Some((look.glyph, stroke(m))));
    // SAFETY: live DC with a null brush and owned pen selected.
    unsafe {
        let _ = Ellipse(hdc, x, y, x + d, y + d);
    }
}

/// 8-tooth gear polygon with a hole in the button's background colour.
pub(super) fn glyph_gear(hdc: HDC, r: Rect, look: &ButtonLook, m: &Metrics) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let (outer, inner) = (m.px(14) as f32 / 2.0, m.px(10) as f32 / 2.0);
    let at = |deg: f32, rad: f32| POINT {
        x: (cx + rad * deg.to_radians().cos()).round() as i32,
        y: (cy - rad * deg.to_radians().sin()).round() as i32,
    };
    let pts: Vec<POINT> = (0..8)
        .flat_map(|k| {
            let a = k as f32 * 45.0;
            [
                at(a - 14.0, inner),
                at(a - 8.0, outer),
                at(a + 8.0, outer),
                at(a + 14.0, inner),
            ]
        })
        .collect();
    let hole = m.px(5);
    let (hx, hy) = (cx.round() as i32 - hole / 2, cy.round() as i32 - hole / 2);
    let _g = select_paint(hdc, Some(look.glyph), None);
    // SAFETY: live DC with a brush and null pen selected.
    unsafe {
        let _ = Polygon(hdc, &pts);
    }
    let _h = select_paint(hdc, Some(look.under), None);
    // SAFETY: as above, with the hole brush now selected.
    unsafe {
        let _ = Ellipse(hdc, hx, hy, hx + hole + 1, hy + hole + 1);
    }
}

/// Two-segment chevron, pointing up when `up`.
pub(super) fn glyph_chevron(hdc: HDC, r: Rect, look: &ButtonLook, m: &Metrics, up: bool) {
    let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
    let (hw, rise) = (m.px(5), m.px(5) / 2);
    let hh = if up { rise } else { -rise };
    let pts = [
        POINT {
            x: cx - hw,
            y: cy + hh,
        },
        POINT { x: cx, y: cy - hh },
        POINT {
            x: cx + hw,
            y: cy + hh,
        },
    ];
    let _g = select_paint(hdc, None, Some((look.glyph, stroke(m))));
    // SAFETY: live DC with an owned pen selected.
    unsafe {
        let _ = Polyline(hdc, &pts);
    }
}
