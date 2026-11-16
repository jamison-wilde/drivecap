//! Popup drawing into a [`Painter`] canvas, the card's three passes at
//! popup scale: opaque fills ([`raster`]), GDI text, then the 1 px border
//! and the rounded mask. Always presented at alpha 255.

use windows::Win32::Graphics::Gdi::{
    DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_RIGHT, HDC, SetBkMode, TRANSPARENT,
};

use super::{CHANGE_LABEL, CHIPS, OpacityGeom, PopupGeom, PopupHit, SettingsGeom, thumb_x};
use crate::config::SnoozeChoice;
use crate::error::DrivecapResult;
use crate::layout::{Metrics, Rect, TextStyle};
use crate::paint::{Fonts, Painter, text};
use crate::palette::{Palette, Rgb};
use crate::raster;

/// Radii and strokes (logical px).
const POPUP_RADIUS: i32 = 8;
const BUTTON_RADIUS: i32 = 4;
const BAR_H: i32 = 4;
const BAR_RADIUS: i32 = 2;
/// Thumb's white ring around its muted centre.
const THUMB_RING: i32 = 3;
/// Text inset inside a menu-like item (beyond the item's own inset).
const ITEM_TEXT_X: i32 = 6;

/// Everything a popup shows besides its geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupScene {
    /// Slider value, percent.
    pub opacity: u8,
    pub snooze: SnoozeChoice,
    /// "Segoe UI, 9 pt".
    pub font: String,
    pub hover: Option<PopupHit>,
}

/// Draws the popup described by `g` and `sc` into `p` (sized to `g`).
pub fn draw(
    p: &mut Painter,
    g: &PopupGeom,
    sc: &PopupScene,
    pal: &Palette,
    fonts: &Fonts,
    m: &Metrics,
) -> DrivecapResult<()> {
    let (w, h) = g.size();
    p.begin(w, h)?;
    let full = Rect::new(0, 0, w, h);
    let buf = p.pixels();
    raster::fill_rounded(buf, w, h, &full, 0.0, pal.surface);
    match g {
        PopupGeom::Opacity(o) => fill_opacity(buf, (w, h), o, sc, pal, m),
        PopupGeom::Settings(s) => fill_settings(buf, (w, h), s, sc, pal, m),
    }
    let hdc = p.hdc();
    // SAFETY: the painter's live DC; transparent text over the fills.
    unsafe { SetBkMode(hdc, TRANSPARENT) };
    match g {
        PopupGeom::Opacity(o) => text_opacity(hdc, o, sc, pal, fonts),
        PopupGeom::Settings(s) => text_settings(hdc, s, sc, pal, fonts, m),
    }
    let buf = p.pixels();
    let radius = m.px(POPUP_RADIUS) as f32;
    raster::stroke_ring(buf, w, h, &full, radius, m.px(1).max(1) as f32, pal.border);
    raster::apply_card_mask(buf, w, h, &full, radius);
    Ok(())
}

fn hover_fill(pal: &Palette) -> Rgb {
    pal.flat(pal.hover, pal.surface)
}

/// Slider: the bar filled in `text` up to the thumb, `track` after it,
/// and a white-ringed thumb (darker while hovered).
fn fill_opacity(
    buf: &mut [u32],
    (w, h): (i32, i32),
    o: &OpacityGeom,
    sc: &PopupScene,
    pal: &Palette,
    m: &Metrics,
) {
    let cy = o.track.y + o.track.h / 2;
    let bar_h = m.px(BAR_H).max(2);
    let bar = Rect::new(o.track.x, cy - bar_h / 2, o.track.w, bar_h);
    let r = m.px(BAR_RADIUS) as f32;
    raster::fill_rounded(buf, w, h, &bar, r, pal.flat(pal.track, pal.surface));
    let tx = thumb_x(sc.opacity, o.track);
    let done = Rect::new(bar.x, bar.y, tx - bar.x, bar.h);
    raster::fill_rounded(buf, w, h, &done, r, pal.text);
    let tr = m.px(super::THUMB_R);
    let thumb = Rect::new(tx - tr, cy - tr, 2 * tr, 2 * tr);
    raster::fill_rounded(buf, w, h, &thumb, tr as f32, Rgb::WHITE);
    raster::stroke_ring(buf, w, h, &thumb, tr as f32, 1.0, pal.border);
    let ring = m.px(THUMB_RING);
    let core = Rect::new(
        thumb.x + ring,
        thumb.y + ring,
        thumb.w - 2 * ring,
        thumb.h - 2 * ring,
    );
    let hot = sc.hover == Some(PopupHit::Slider);
    let core_c = if hot { pal.text_secondary } else { pal.muted };
    raster::fill_rounded(buf, w, h, &core, (tr - ring) as f32, core_c);
}

fn text_opacity(hdc: HDC, o: &OpacityGeom, sc: &PopupScene, pal: &Palette, fonts: &Fonts) {
    let small = fonts.get(TextStyle::Small);
    text(
        hdc,
        "Idle opacity",
        o.label,
        small,
        pal.text_secondary,
        DT_LEFT,
    );
    let value = format!("{}%", sc.opacity);
    let semi = fonts.get(TextStyle::BodySemibold);
    text(hdc, &value, o.label, semi, pal.text, DT_RIGHT);
}

/// A bordered button: inverted when `selected`, hover-filled when hot.
#[allow(clippy::too_many_arguments)] // buffer triple + geometry + state + paint
fn fill_button(
    buf: &mut [u32],
    (w, h): (i32, i32),
    r: &Rect,
    selected: bool,
    hot: bool,
    pal: &Palette,
    m: &Metrics,
) {
    let radius = m.px(BUTTON_RADIUS) as f32;
    if selected {
        raster::fill_rounded(buf, w, h, r, radius, pal.text);
        return;
    }
    if hot {
        raster::fill_rounded(buf, w, h, r, radius, hover_fill(pal));
    }
    let line = m.px(1).max(1) as f32;
    raster::stroke_ring(buf, w, h, r, radius, line, pal.border);
}

fn fill_settings(
    buf: &mut [u32],
    size: (i32, i32),
    s: &SettingsGeom,
    sc: &PopupScene,
    pal: &Palette,
    m: &Metrics,
) {
    let (w, h) = size;
    for (c, r) in &s.chips {
        let hot = sc.hover == Some(PopupHit::Chip(*c));
        fill_button(buf, size, r, *c == sc.snooze, hot, pal, m);
    }
    let change_hot = sc.hover == Some(PopupHit::Change);
    fill_button(buf, size, &s.change, false, change_hot, pal, m);
    let divider = pal.flat(pal.border, pal.surface);
    raster::fill_rounded(buf, w, h, &s.divider, 0.0, divider);
    let radius = m.px(BUTTON_RADIUS) as f32;
    for (hit, r) in [(PopupHit::EditConfig, &s.edit), (PopupHit::Quit, &s.quit)] {
        if sc.hover == Some(hit) {
            raster::fill_rounded(buf, w, h, r, radius, hover_fill(pal));
        }
    }
}

fn text_settings(
    hdc: HDC,
    s: &SettingsGeom,
    sc: &PopupScene,
    pal: &Palette,
    fonts: &Fonts,
    m: &Metrics,
) {
    let (small, body) = (fonts.get(TextStyle::Small), fonts.get(TextStyle::Body));
    text(
        hdc,
        "Snooze for",
        s.snooze_label,
        small,
        pal.text_secondary,
        DT_LEFT,
    );
    for ((c, r), (_, label)) in s.chips.iter().zip(CHIPS) {
        let color = if *c == sc.snooze {
            pal.surface
        } else {
            pal.text
        };
        text(hdc, label, *r, body, color, DT_CENTER);
    }
    text(
        hdc,
        "Font",
        s.font_label,
        small,
        pal.text_secondary,
        DT_LEFT,
    );
    let ellipsis = DT_LEFT | DT_END_ELLIPSIS;
    text(hdc, &sc.font, s.font_name, body, pal.text, ellipsis);
    text(hdc, CHANGE_LABEL, s.change, body, pal.text, DT_CENTER);
    let dx = m.px(ITEM_TEXT_X);
    for (label, r) in [("Edit config.json", s.edit), ("Quit drivecap", s.quit)] {
        let inner = Rect::new(r.x + dx, r.y, r.w - 2 * dx, r.h);
        text(hdc, label, inner, body, pal.text, DT_LEFT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ColorOverrides;
    use crate::paint::GdiMeasure;
    use crate::popup_wnd::{opacity_geometry, settings_geometry};

    fn scene(hover: Option<PopupHit>) -> PopupScene {
        PopupScene {
            opacity: 45,
            snooze: SnoozeChoice::TenMin,
            font: "Segoe UI, 9 pt".into(),
            hover,
        }
    }

    fn render(g: &PopupGeom, sc: &PopupScene, dark: bool) -> Painter {
        let m = Metrics::new(96, 100);
        let pal = Palette::new(dark, &ColorOverrides::default());
        let fonts = Fonts::new(None, &m).unwrap();
        let mut p = Painter::new().unwrap();
        draw(&mut p, g, sc, &pal, &fonts, &m).unwrap();
        p
    }

    fn settings() -> PopupGeom {
        let m = Metrics::new(96, 100);
        let fonts = Fonts::new(None, &m).unwrap();
        let tm = GdiMeasure::new(&fonts).unwrap();
        PopupGeom::Settings(settings_geometry(230, &m, &tm))
    }

    fn rgb(px: u32) -> Rgb {
        Rgb((px >> 16) as u8, (px >> 8) as u8, px as u8)
    }

    #[test]
    fn popup_is_opaque_with_rounded_corners() {
        let g = PopupGeom::Opacity(opacity_geometry(200, 56, &Metrics::new(96, 100)));
        let p = render(&g, &scene(None), false);
        assert_eq!(p.size(), (200, 56));
        assert_eq!(p.pixel(0, 0) >> 24, 0, "corner outside the radius");
        assert_eq!(p.pixel(100, 20) >> 24, 0xFF, "body fully opaque");
    }

    #[test]
    fn selected_chip_is_inverted() {
        let g = settings();
        let PopupGeom::Settings(s) = &g else {
            unreachable!()
        };
        let pal = Palette::new(false, &ColorOverrides::default());
        let p = render(&g, &scene(None), false);
        let (sel, other) = (s.chips[0].1, s.chips[1].1);
        assert_eq!(rgb(p.pixel(sel.x + 2, sel.y + 2)), pal.text);
        assert_eq!(rgb(p.pixel(other.x + 2, other.y + 2)), pal.surface);
    }

    #[test]
    fn hovered_item_gets_the_hover_fill() {
        let g = settings();
        let PopupGeom::Settings(s) = &g else {
            unreachable!()
        };
        let pal = Palette::new(true, &ColorOverrides::default());
        let p = render(&g, &scene(Some(PopupHit::Quit)), true);
        let at = (s.quit.x + 2, s.quit.y + s.quit.h / 2);
        assert_eq!(rgb(p.pixel(at.0, at.1)), pal.flat(pal.hover, pal.surface));
    }

    /// Writes `./tmp/popup-{opacity,settings}-{light,dark}.bmp` (convert
    /// with `node tmp/bmp2png.mjs tmp/popup-*.bmp`).
    #[test]
    #[ignore = "writes ./tmp/popup-*.bmp for manual review"]
    fn dump_popup_bmps() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let opacity = PopupGeom::Opacity(opacity_geometry(200, 56, &Metrics::new(96, 100)));
        let hover = Some(PopupHit::EditConfig);
        for dark in [false, true] {
            let theme = if dark { "dark" } else { "light" };
            for (name, g, h) in [
                ("opacity", &opacity, None),
                ("settings", &settings(), hover),
            ] {
                let p = render(g, &scene(h), dark);
                write_bmp(&dir.join(format!("popup-{name}-{theme}.bmp")), &p, dark);
            }
        }
    }

    /// 32-bpp top-down BMP of the premultiplied DIB over a neutral ground.
    fn write_bmp(path: &std::path::Path, p: &Painter, dark: bool) {
        let (w, h) = p.size();
        let bg: u32 = if dark { 0x10 } else { 0xD8 };
        let mut out = Vec::new();
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(54 + 4 * w * h).to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&(-h).to_le_bytes());
        out.extend_from_slice(&[1, 0, 32, 0]);
        out.extend_from_slice(&[0u8; 24]);
        for y in 0..h {
            for x in 0..w {
                let px = p.pixel(x, y);
                let a = px >> 24;
                for shift in [0, 8, 16] {
                    out.push(((px >> shift & 0xFF) + bg * (255 - a) / 255) as u8);
                }
                out.push(255);
            }
        }
        std::fs::write(path, out).unwrap();
    }
}
