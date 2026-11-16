//! GDI painter for the card. Renders into a 32-bpp top-down DIB in three
//! passes, then presents it with `UpdateLayeredWindow(ULW_ALPHA)`:
//!
//! 1. **Fills** ([`raster`], written straight into the DIB bits): card and
//!    row backgrounds, button backgrounds, grip, state dot, runway bars.
//!    Translucent tokens are pre-flattened with [`Palette::flat`] over the
//!    local background, so every pixel is opaque RGB.
//! 2. **GDI**: text and header/row glyphs over those opaque fills
//!    (`TRANSPARENT` background mode).
//! 3. **Alpha**: the 1 px card border, [`raster::apply_card_mask`] (alpha
//!    from the rounded-rect coverage, premultiply, zero the margin), then
//!    the dire halo or its pulsing-border fallback.
//!
//! [`Painter::draw`] does passes 1–3 without a window (unit-testable via
//! [`Painter::pixel`]); [`Painter::present`] pushes the result to a window.

mod fonts;
mod gdi;
mod glyphs;

use std::ffi::c_void;

use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
    DT_RIGHT, DeleteDC, DeleteObject, GdiFlush, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC,
    SelectObject, SetBkMode, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};

pub use fonts::{Fonts, GdiMeasure};
pub(crate) use gdi::text;
use glyphs::{glyph_chevron, glyph_gear, glyph_opacity};

use crate::config::Mode;
use crate::error::DrivecapResult;
use crate::layout::{CardLayout, HeaderButton, Hit, Metrics, Rect, TextStyle};
use crate::model::{Dot, RowState, RowView, SurfaceView};
use crate::palette::{DIRE_TRACK_ALPHA, GRIP_ALPHA, Palette, Rgb, Rgba};
use crate::raster;

/// Radii and strokes (logical px).
const CARD_RADIUS: i32 = 8;
const BUTTON_RADIUS: i32 = 4;
const BAR_RADIUS: i32 = 2;
const DASH: i32 = 3;
const DIRE_BORDER: i32 = 2;
const GRIP_DOT: i32 = 2;
const GRIP_PITCH_X: i32 = 3;
const GRIP_PITCH_Y: i32 = 4;
/// Per-draw inputs shared by the element painters.
struct Ctx<'a> {
    pal: &'a Palette,
    m: &'a Metrics,
    fonts: &'a Fonts,
    view: &'a SurfaceView,
    hover: Option<Hit>,
    active: Option<HeaderButton>,
}

/// Button fill (if any), glyph colour and the colour under the glyph.
struct ButtonLook {
    fill: Option<Rgb>,
    glyph: Rgb,
    under: Rgb,
}

impl Ctx<'_> {
    fn button_look(&self, hovered: bool, inverted: bool, bg: Rgb, glyph: Rgb) -> ButtonLook {
        if inverted {
            let p = self.pal;
            return ButtonLook {
                fill: Some(p.text),
                glyph: p.surface,
                under: p.text,
            };
        }
        if hovered {
            let h = self.pal.flat(self.pal.hover, bg);
            return ButtonLook {
                fill: Some(h),
                glyph,
                under: h,
            };
        }
        ButtonLook {
            fill: None,
            glyph,
            under: bg,
        }
    }

    fn header_look(&self, b: HeaderButton) -> ButtonLook {
        let inverted =
            self.active == Some(b) || (b == HeaderButton::SnoozeAll && self.view.snooze_all_active);
        let hovered = self.hover == Some(Hit::Button(b));
        self.button_look(hovered, inverted, self.pal.surface, self.pal.text)
    }

    fn row_snooze_look(&self, i: usize, rv: &RowView) -> ButtonLook {
        let c = row_colors(rv.state, self.pal);
        let hovered = self.hover == Some(Hit::RowSnooze(i));
        self.button_look(hovered, rv.snooze_active, c.bg, c.glyph)
    }
}

/// Row colours per handoff "Row states".
struct RowColors {
    bg: Rgb,
    letter: Rgb,
    label: Rgb,
    value: Rgb,
    glyph: Rgb,
}

fn row_colors(state: RowState, p: &Palette) -> RowColors {
    let (bg, letter, label, value) = match state {
        RowState::Healthy => (p.surface, p.text, p.text_secondary, p.text),
        RowState::Warning => (p.surface, p.text, p.text_secondary, p.warning_text),
        RowState::Dire => (p.dire, Rgb::WHITE, Rgb::WHITE, Rgb::WHITE),
        RowState::Unreachable | RowState::Snoozed(_) => (p.surface, p.muted, p.muted, p.muted),
    };
    let glyph = if state == RowState::Dire {
        Rgb::WHITE
    } else {
        p.text
    };
    RowColors {
        bg,
        letter,
        label,
        value,
        glyph,
    }
}

/// Pass 1, header: card background, grip, state dot, button fills.
fn fill_header(buf: &mut [u32], w: i32, h: i32, l: &CardLayout, cx: &Ctx) {
    let (p, m) = (cx.pal, cx.m);
    raster::fill_rounded(buf, w, h, &l.card, 0.0, p.surface);
    let grip = p.flat(Rgba::new(p.text, GRIP_ALPHA), p.surface);
    let (dot, px, py) = (m.px(GRIP_DOT), m.px(GRIP_PITCH_X), m.px(GRIP_PITCH_Y));
    let x0 = l.grip.x + (l.grip.w - (px + dot)) / 2;
    let y0 = l.grip.y + (l.grip.h - (2 * py + dot)) / 2;
    for (col, row) in (0..2).flat_map(|c| (0..3).map(move |r| (c, r))) {
        let r = Rect::new(x0 + col * px, y0 + row * py, dot, dot);
        raster::fill_rounded(buf, w, h, &r, 0.0, grip);
    }
    let dot_c = match cx.view.dot {
        Dot::DireAccent => p.dire_accent,
        Dot::Warning => p.warning,
        Dot::Muted => p.muted,
    };
    raster::fill_rounded(buf, w, h, &l.dot, l.dot.w as f32 / 2.0, dot_c);
    for (b, r) in &l.buttons {
        if let Some(c) = cx.header_look(*b).fill {
            raster::fill_rounded(buf, w, h, r, m.px(BUTTON_RADIUS) as f32, c);
        }
    }
}

/// Pass 1, one row: background, top border, Zz fill, runway bar.
fn fill_row(buf: &mut [u32], w: i32, h: i32, i: usize, l: &CardLayout, cx: &Ctx) {
    let (rl, rv) = (&l.rows[i], &cx.view.rows[i]);
    let bg = row_colors(rv.state, cx.pal).bg;
    raster::fill_rounded(buf, w, h, &rl.row, 0.0, bg);
    let line = Rect::new(rl.row.x, rl.row.y, rl.row.w, cx.m.px(1).max(1));
    raster::fill_rounded(buf, w, h, &line, 0.0, cx.pal.flat(cx.pal.border, bg));
    if rl.snooze.w > 0
        && let Some(c) = cx.row_snooze_look(i, rv).fill
    {
        let radius = cx.m.px(BUTTON_RADIUS) as f32;
        raster::fill_rounded(buf, w, h, &rl.snooze, radius, c);
    }
    fill_bar(buf, w, h, &rl.bar, rv, bg, cx);
}

/// Runway bar: track, fill to `rv.fill`, 1 px level gaps in `bg`; dashed
/// for unreachable, absent for snoozed.
fn fill_bar(buf: &mut [u32], w: i32, h: i32, bar: &Rect, rv: &RowView, bg: Rgb, cx: &Ctx) {
    let (p, m) = (cx.pal, cx.m);
    let (track, fill) = match rv.state {
        RowState::Snoozed(_) => return,
        RowState::Unreachable => {
            let dash = m.px(DASH).max(1);
            for x in (bar.x..bar.right()).step_by(2 * dash as usize) {
                let r = Rect::new(x, bar.y, dash.min(bar.right() - x), bar.h);
                raster::fill_rounded(buf, w, h, &r, 0.0, p.muted);
            }
            return;
        }
        RowState::Dire => (
            p.flat(Rgba::new(Rgb::WHITE, DIRE_TRACK_ALPHA), bg),
            Rgb::WHITE,
        ),
        RowState::Warning => (p.flat(p.track, bg), p.warning),
        RowState::Healthy => (p.flat(p.track, bg), p.muted),
    };
    let radius = m.px(BAR_RADIUS) as f32;
    raster::fill_rounded(buf, w, h, bar, radius, track);
    let fw = (bar.w as f32 * rv.fill.clamp(0.0, 1.0)).round() as i32;
    if fw > 0 {
        let r = Rect::new(bar.x, bar.y, fw, bar.h);
        raster::fill_rounded(buf, w, h, &r, radius, fill);
    }
    let gap_w = m.px(1).max(1);
    for g in &rv.gaps {
        let x = bar.x + (bar.w as f32 * g).round() as i32;
        if x > bar.x && x < bar.right() {
            let r = Rect::new(x, bar.y, gap_w, bar.h);
            raster::fill_rounded(buf, w, h, &r, 0.0, bg);
        }
    }
}

/// Pass 2, header: title and the four button glyphs.
fn draw_header_text(hdc: HDC, l: &CardLayout, cx: &Ctx) {
    let f = cx.fonts;
    let title_font = f.get(TextStyle::BodySemibold);
    let flags = DT_LEFT | DT_END_ELLIPSIS;
    text(hdc, &cx.view.title, l.title, title_font, cx.pal.text, flags);
    for (b, r) in l.buttons {
        let look = cx.header_look(b);
        match b {
            HeaderButton::SnoozeAll => {
                text(hdc, "Zz", r, f.get(TextStyle::Glyph), look.glyph, DT_CENTER)
            }
            HeaderButton::Opacity => glyph_opacity(hdc, r, &look, cx.m),
            HeaderButton::Settings => glyph_gear(hdc, r, &look, cx.m),
            HeaderButton::Chevron => {
                glyph_chevron(hdc, r, &look, cx.m, cx.view.mode == Mode::Expanded)
            }
        }
    }
}

/// Pass 2, one row: letter, label (ellipsised), value, Zz glyph.
fn draw_row_text(hdc: HDC, i: usize, l: &CardLayout, cx: &Ctx) {
    let (rl, rv, f) = (&l.rows[i], &cx.view.rows[i], cx.fonts);
    let c = row_colors(rv.state, cx.pal);
    let bold = f.get(TextStyle::BodyBold);
    text(hdc, &rv.letter, rl.letter, bold, c.letter, DT_LEFT);
    let body = f.get(TextStyle::Body);
    text(
        hdc,
        &rv.label,
        rl.label,
        body,
        c.label,
        DT_LEFT | DT_END_ELLIPSIS,
    );
    let semi = f.get(TextStyle::BodySemibold);
    text(hdc, &rv.value, rl.value, semi, c.value, DT_RIGHT);
    if rl.snooze.w > 0 {
        let look = cx.row_snooze_look(i, rv);
        text(
            hdc,
            "Zz",
            rl.snooze,
            f.get(TextStyle::Glyph),
            look.glyph,
            DT_CENTER,
        );
    }
}

/// Memory DC + 32-bpp top-down DIB section the card is drawn into.
pub struct Painter {
    hdc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    w: i32,
    h: i32,
}

impl Painter {
    /// Creates the memory DC; fails if GDI cannot allocate one.
    pub fn new() -> DrivecapResult<Painter> {
        // SAFETY: a memory DC compatible with the screen; deleted on drop.
        let hdc = unsafe { CreateCompatibleDC(None) };
        if hdc.is_invalid() {
            return Err(windows::core::Error::from_win32().into());
        }
        Ok(Painter {
            hdc,
            bmp: HBITMAP::default(),
            old: HGDIOBJ::default(),
            bits: std::ptr::null_mut(),
            w: 0,
            h: 0,
        })
    }

    /// Current DIB size in physical px.
    pub fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    /// Premultiplied BGRA pixel at `(x, y)`; 0 outside the DIB.
    pub fn pixel(&self, x: i32, y: i32) -> u32 {
        if self.bits.is_null() || x < 0 || y < 0 || x >= self.w || y >= self.h {
            return 0;
        }
        // SAFETY: bounds checked against the live DIB's w × h u32 pixels.
        unsafe { *self.bits.add((y * self.w + x) as usize) }
    }

    fn ensure_size(&mut self, w: i32, h: i32) -> DrivecapResult<()> {
        if w == self.w && h == self.h && !self.bits.is_null() {
            return Ok(());
        }
        self.release_bitmap();
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w.max(1),
                biHeight: -h.max(1),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: `bmi` describes a 32-bpp top-down DIB; `bits` receives
        // its pixel pointer, valid until the bitmap is deleted.
        let bmp =
            unsafe { CreateDIBSection(Some(self.hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)? };
        // SAFETY: selects the fresh DIB into our DC; `old` is restored in
        // `release_bitmap`.
        self.old = unsafe { SelectObject(self.hdc, bmp.into()) };
        (self.bmp, self.bits) = (bmp, bits as *mut u32);
        (self.w, self.h) = (w.max(1), h.max(1));
        Ok(())
    }

    fn release_bitmap(&mut self) {
        if self.bits.is_null() {
            return;
        }
        // SAFETY: deselects then deletes the DIB this painter created.
        unsafe {
            SelectObject(self.hdc, self.old);
            let _ = DeleteObject(self.bmp.into());
        }
        (self.bmp, self.bits, self.w, self.h) = (HBITMAP::default(), std::ptr::null_mut(), 0, 0);
    }

    fn buf(&mut self) -> &mut [u32] {
        // SAFETY: GDI's batch is flushed so the CPU sees its writes; the
        // DIB holds exactly w × h pixels and lives while `self` is borrowed.
        unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize)
        }
    }

    /// Generic canvas for other renderers (the popups): sizes the DIB to
    /// `w × h` and clears it. Fill through [`Painter::pixels`], draw GDI
    /// text on [`Painter::hdc`], then [`Painter::present`].
    pub fn begin(&mut self, w: i32, h: i32) -> DrivecapResult<()> {
        self.ensure_size(w, h)?;
        self.buf().fill(0);
        Ok(())
    }

    /// The memory DC holding the DIB.
    pub fn hdc(&self) -> HDC {
        self.hdc
    }

    /// The DIB pixels (GDI flushed first), `w × h` top-down.
    pub fn pixels(&mut self) -> &mut [u32] {
        self.buf()
    }

    /// Renders the card into the DIB. `halo_phase_ms` is `Some` while the
    /// view's halo is on; `halo_mode` picks the halo (`true`) or the 2 px
    /// pulsing-border fallback.
    #[allow(clippy::too_many_arguments)] // signature fixed by the controller ruling
    pub fn draw(
        &mut self,
        layout: &CardLayout,
        view: &SurfaceView,
        pal: &Palette,
        fonts: &Fonts,
        m: &Metrics,
        hover: Option<Hit>,
        active: Option<HeaderButton>,
        halo_phase_ms: Option<u64>,
        halo_mode: bool,
    ) -> DrivecapResult<()> {
        self.ensure_size(layout.window.w, layout.window.h)?;
        let cx = Ctx {
            pal,
            m,
            fonts,
            view,
            hover,
            active,
        };
        let (w, h, hdc) = (self.w, self.h, self.hdc);
        let rows = layout.rows.len().min(view.rows.len());
        let buf = self.buf();
        buf.fill(0);
        fill_header(buf, w, h, layout, &cx);
        (0..rows).for_each(|i| fill_row(buf, w, h, i, layout, &cx));
        // SAFETY: live DC; transparent text over the opaque fills above.
        unsafe { SetBkMode(hdc, TRANSPARENT) };
        draw_header_text(hdc, layout, &cx);
        (0..rows).for_each(|i| draw_row_text(hdc, i, layout, &cx));
        let buf = self.buf();
        let card = &layout.card;
        let radius = m.px(CARD_RADIUS) as f32;
        let line = m.px(1).max(1) as f32;
        raster::stroke_ring(buf, w, h, card, radius, line, pal.border);
        // The pulse border is blended before the mask so AA corners keep
        // their coverage alpha; the halo lives outside the card, after it.
        let accent = pal.dire_accent;
        if let (Some(p), false) = (halo_phase_ms, halo_mode) {
            let (a, bw) = (raster::border_pulse_alpha(p), m.px(DIRE_BORDER) as f32);
            raster::draw_pulse_border(buf, w, h, card, radius, accent, a, bw);
        }
        raster::apply_card_mask(buf, w, h, card, radius);
        if let (Some(p), true) = (halo_phase_ms, halo_mode) {
            raster::draw_halo(buf, w, h, card, radius, accent, p, m.s);
        }
        Ok(())
    }

    /// Pushes the DIB to `hwnd` at screen `(x, y)` with constant `alpha`.
    pub fn present(&self, hwnd: HWND, x: i32, y: i32, alpha: u8) -> DrivecapResult<()> {
        if self.bits.is_null() {
            return Ok(());
        }
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: alpha,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let (dst, src) = (POINT { x, y }, POINT::default());
        let size = SIZE {
            cx: self.w,
            cy: self.h,
        };
        // SAFETY: the screen DC is released before returning; `self.hdc`
        // has the premultiplied DIB selected; pointers are to locals that
        // outlive the call.
        let result = unsafe {
            let screen = GetDC(None);
            let r = UpdateLayeredWindow(
                hwnd,
                Some(screen),
                Some(&dst),
                Some(&size),
                Some(self.hdc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            ReleaseDC(None, screen);
            r
        };
        Ok(result?)
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        self.release_bitmap();
        // SAFETY: the DC was created in `new`; the DIB is already deselected.
        unsafe {
            let _ = DeleteDC(self.hdc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::gdi::wide;
    use super::*;
    use crate::config::FontSpec;
    use crate::config::{ColorOverrides, SnoozeChoice, VolumeId};
    use crate::layout;
    use crate::layout::TextMeasure;
    use windows_core::PCWSTR;

    fn row(letter: &str, label: &str, value: &str, state: RowState, fill: f32) -> RowView {
        let at_risk = matches!(
            state,
            RowState::Warning | RowState::Dire | RowState::Snoozed(_)
        );
        RowView {
            id: VolumeId(letter.into()),
            letter: letter.into(),
            label: label.into(),
            value: value.into(),
            state,
            fill,
            gaps: vec![0.25, 0.5, 0.75],
            show_snooze_btn: at_risk,
            snooze_active: matches!(state, RowState::Snoozed(_)),
        }
    }

    fn sample_view() -> SurfaceView {
        SurfaceView {
            title: "Disk space".into(),
            dot: Dot::DireAccent,
            snooze_all_active: false,
            rows: vec![
                row("E:", "SanDisk 64GB", "1.4 GB", RowState::Dire, 0.07),
                row("C:", "Windows", "14.8 GB", RowState::Warning, 0.74),
                row(
                    "D:",
                    "Dev",
                    "Zz 10 min",
                    RowState::Snoozed(SnoozeChoice::TenMin),
                    0.31,
                ),
                row("F:", "Data", "212 GB", RowState::Healthy, 1.0),
                row("Z:", r"\\nas\builds", "?", RowState::Unreachable, 0.0),
            ],
            halo: true,
            mode: Mode::Expanded,
        }
    }

    struct Scene {
        painter: Painter,
        layout: CardLayout,
        pal: Palette,
    }

    fn render_at(view: &SurfaceView, dark: bool, pct: u32, halo: Option<u64>, mode: bool) -> Scene {
        let m = Metrics::new(96, pct);
        let fonts = Fonts::new(None, &m).unwrap();
        let layout = layout::compute(view, &m, &GdiMeasure::new(&fonts).unwrap());
        let pal = Palette::new(dark, &ColorOverrides::default());
        let mut painter = Painter::new().unwrap();
        let hover = Some(Hit::RowSnooze(1));
        painter
            .draw(&layout, view, &pal, &fonts, &m, hover, None, halo, mode)
            .unwrap();
        Scene {
            painter,
            layout,
            pal,
        }
    }

    fn render(view: &SurfaceView, dark: bool, halo: Option<u64>, mode: bool) -> Scene {
        render_at(view, dark, 100, halo, mode)
    }

    fn opaque(c: Rgb) -> u32 {
        0xFF00_0000 | (u32::from(c.0) << 16) | (u32::from(c.1) << 8) | u32::from(c.2)
    }

    #[test]
    fn measure_widths_track_weight() {
        let fonts = Fonts::new(None, &Metrics::new(96, 100)).unwrap();
        let tm = GdiMeasure::new(&fonts).unwrap();
        let body = tm.width("Disk space", TextStyle::Body);
        assert!(body > 20, "body {body}");
        assert!(tm.width("Disk space", TextStyle::BodyBold) >= body);
        assert_eq!(tm.width("", TextStyle::Body), 0);
    }

    #[test]
    fn dib_matches_window_and_margin_is_clear() {
        let s = render(&sample_view(), false, None, true);
        let l = &s.layout;
        assert_eq!(s.painter.size(), (l.window.w, l.window.h));
        assert_eq!(s.painter.pixel(0, 0), 0);
        assert_eq!(s.painter.pixel(l.card.x - 1, l.card.y + 40), 0);
        // Rounded corner: the card's very corner pixel is not opaque.
        assert!(s.painter.pixel(l.card.x, l.card.y) >> 24 < 0xFF);
    }

    #[test]
    fn dire_row_background_is_dire() {
        let s = render(&sample_view(), false, None, true);
        let r = s.layout.rows[0].row;
        // Between the bar and the row's bottom: pure row background.
        let px = s.painter.pixel(r.x + r.w / 2, r.bottom() - 2);
        assert_eq!(px, opaque(s.pal.dire));
    }

    #[test]
    fn warning_bar_fill_track_and_gap() {
        let s = render(&sample_view(), false, None, true);
        let bar = s.layout.rows[1].bar;
        let y = bar.y + bar.h / 2;
        assert_eq!(s.painter.pixel(bar.x + 10, y), opaque(s.pal.warning));
        let gap = bar.x + (bar.w as f32 * 0.5).round() as i32;
        assert_eq!(s.painter.pixel(gap, y), opaque(s.pal.surface));
        let track = s.pal.flat(s.pal.track, s.pal.surface);
        assert_eq!(s.painter.pixel(bar.right() - 10, y), opaque(track));
    }

    #[test]
    fn hovered_row_snooze_uses_hover_fill() {
        let s = render(&sample_view(), false, None, true);
        let z = s.layout.rows[1].snooze;
        let hovered = s.pal.flat(s.pal.hover, s.pal.surface);
        assert_eq!(s.painter.pixel(z.x + 4, z.y + 2), opaque(hovered));
    }

    #[test]
    fn snoozed_row_has_no_bar_and_inverted_zz() {
        let s = render(&sample_view(), false, None, true);
        let rl = s.layout.rows[2];
        let y = rl.bar.y + rl.bar.h / 2;
        assert_eq!(s.painter.pixel(rl.bar.x + 10, y), opaque(s.pal.surface));
        let z = rl.snooze;
        assert_eq!(s.painter.pixel(z.x + 4, z.y + 2), opaque(s.pal.text));
    }

    #[test]
    fn unreachable_bar_is_dashed() {
        let s = render(&sample_view(), true, None, true);
        let bar = s.layout.rows[4].bar;
        let y = bar.y + bar.h / 2;
        assert_eq!(s.painter.pixel(bar.x + 1, y), opaque(s.pal.muted));
        assert_eq!(s.painter.pixel(bar.x + 4, y), opaque(s.pal.surface));
    }

    #[test]
    fn active_header_button_is_inverted() {
        let m = Metrics::new(96, 100);
        let fonts = Fonts::new(None, &m).unwrap();
        let view = sample_view();
        let layout = layout::compute(&view, &m, &GdiMeasure::new(&fonts).unwrap());
        let pal = Palette::new(false, &ColorOverrides::default());
        let mut p = Painter::new().unwrap();
        let gear = layout.buttons[HeaderButton::Settings as usize].1;
        let op = layout.buttons[HeaderButton::Opacity as usize].1;
        let hover = Some(Hit::Button(HeaderButton::Opacity));
        let active = Some(HeaderButton::Settings);
        p.draw(&layout, &view, &pal, &fonts, &m, hover, active, None, true)
            .unwrap();
        assert_eq!(p.pixel(gear.x + 3, gear.y + 2), opaque(pal.text));
        let hovered = pal.flat(pal.hover, pal.surface);
        assert_eq!(p.pixel(op.x + 3, op.y + 2), opaque(hovered));
    }

    #[test]
    fn halo_mode_paints_margin_border_mode_does_not() {
        let halo = render(&sample_view(), false, Some(850), true);
        let l = &halo.layout;
        let (mx, my) = (l.card.x - 2, l.card.y + l.card.h / 2);
        assert!(halo.painter.pixel(mx, my) >> 24 > 0, "halo in margin");
        let border = render(&sample_view(), false, Some(0), false);
        assert_eq!(border.painter.pixel(mx, my), 0);
        let edge = border.painter.pixel(l.card.x, my);
        assert_eq!(edge, opaque(border.pal.dire_accent));
    }

    #[test]
    fn redraw_at_new_size_reallocates() {
        let m = Metrics::new(96, 100);
        let fonts = Fonts::new(None, &m).unwrap();
        let pal = Palette::new(false, &ColorOverrides::default());
        let mut p = Painter::new().unwrap();
        let mut view = sample_view();
        for n in [5, 1] {
            view.rows.truncate(n);
            let layout = layout::compute(&view, &m, &GdiMeasure::new(&fonts).unwrap());
            p.draw(&layout, &view, &pal, &fonts, &m, None, None, None, true)
                .unwrap();
            assert_eq!(p.size(), (layout.window.w, layout.window.h));
        }
    }

    #[test]
    fn fonts_from_spec_scale_with_point_size() {
        let m = Metrics::new(96, 100);
        let spec = |pt| FontSpec {
            face: "Arial".into(),
            point_size: pt,
            weight: 400,
        };
        let (small, big) = (
            Fonts::new(Some(&spec(8)), &m).unwrap(),
            Fonts::new(Some(&spec(16)), &m).unwrap(),
        );
        let w = |f: &Fonts| {
            GdiMeasure::new(f)
                .unwrap()
                .width("Windows", TextStyle::Body)
        };
        assert!(w(&big) > w(&small) * 3 / 2);
    }

    /// Hidden layered window for the present() smoke test.
    fn test_window() -> HWND {
        use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, IDC_ARROW, LoadCursorW, RegisterClassExW, WNDCLASSEXW,
            WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_POPUP,
        };
        unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            std::panic::catch_unwind(|| {
                // SAFETY: forwards the unmodified message to the default proc.
                unsafe { DefWindowProcW(h, m, w, l) }
            })
            .unwrap_or_default()
        }
        let class: Vec<u16> = wide("DrivecapPaintTest").into_iter().chain([0]).collect();
        // SAFETY: class/window creation with a NUL-terminated name that
        // outlives the calls; re-registration failure is harmless.
        unsafe {
            let inst = GetModuleHandleW(None).unwrap();
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc),
                hInstance: inst.into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(&wc);
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW,
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                Some(inst.into()),
                None,
            )
            .unwrap()
        }
    }

    #[test]
    #[ignore = "creates a hidden window; run with --include-ignored"]
    fn render_offscreen_dire_row_pixel() {
        use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
        let hwnd = test_window();
        let s = render(&sample_view(), false, Some(600), true);
        s.painter.present(hwnd, -4000, -4000, 255).unwrap();
        let r = s.layout.rows[0].row;
        let px = s.painter.pixel(r.x + r.w / 2, r.bottom() - 2);
        assert_eq!(px, opaque(s.pal.dire));
        // SAFETY: destroys the window this test created.
        unsafe { DestroyWindow(hwnd).unwrap() };
    }

    /// Writes `./tmp/card-*.bmp` (composited over grey) for eyeballing.
    #[test]
    #[ignore = "writes BMPs to ./tmp for manual inspection"]
    fn dump_card_bmps() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let shots = [
            ("light", false, 100, Some(500), true),
            ("dark", true, 100, Some(500), true),
            ("light-150-border", false, 150, Some(0), false),
        ];
        for (name, dark, pct, halo, mode) in shots {
            let s = render_at(&sample_view(), dark, pct, halo, mode);
            write_bmp(&dir.join(format!("card-{name}.bmp")), &s.painter);
        }
    }

    fn write_bmp(path: &std::path::Path, p: &Painter) {
        let (w, h) = p.size();
        let data = (w * h * 4) as u32;
        let mut out = b"BM".to_vec();
        for v in [54 + data, 0, 54, 40] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&(-h).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        for v in [0u32, data, 2835, 2835, 0, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for y in 0..h {
            for x in 0..w {
                let px = p.pixel(x, y);
                let keep = 255 - (px >> 24);
                let ch = |sh: u32| (((px >> sh) & 0xFF) + (0x80 * keep + 127) / 255) as u8;
                out.extend_from_slice(&[ch(0), ch(8), ch(16), 0xFF]);
            }
        }
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn border_mode_corner_alpha_never_exceeds_coverage() {
        let s = render(&sample_view(), false, Some(0), false);
        let (c, radius) = (s.layout.card, CARD_RADIUS as f32);
        for y in c.y..c.y + CARD_RADIUS {
            for x in c.x..c.x + CARD_RADIUS {
                let cov = raster::rounded_rect_coverage(x as f32 + 0.5, y as f32 + 0.5, &c, radius);
                let max_a = (cov * 255.0).round() as u32;
                let a = s.painter.pixel(x, y) >> 24;
                assert!(a <= max_a, "({x},{y}) alpha {a} > coverage {max_a}");
            }
        }
    }
}
