//! Notification-area icon (spec §7): a small disk glyph drawn at runtime
//! in the worst awake severity's colour, a "Zz" mark while everything at
//! risk is snoozed, and the controller's tooltip.
//!
//! [`glyph_pixels`], [`tip_units`] and [`decode`] are pure; [`Tray`] is the
//! thin `Shell_NotifyIconW` adapter (`NOTIFYICON_VERSION_4`, callback
//! [`WM_APP_TRAY`]). The card window owns the [`Tray`]; dropping it
//! removes the icon.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, DeleteObject, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetSystemMetricsForDpi, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NOTIFY_ICON_MESSAGE, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, HICON, ICONINFO, SM_CXSMICON, WM_CONTEXTMENU, WM_LBUTTONUP,
};

use crate::card_wnd::WM_APP_TRAY;
use crate::controller::TrayState;
use crate::error::DrivecapResult;
use crate::layout::Rect;
use crate::model::Dot;
use crate::palette::{Palette, Rgb};
use crate::raster::rounded_rect_coverage;
use crate::theme;

/// Our one icon's id.
const TRAY_UID: u32 = 1;
/// `szTip` holds 128 UTF-16 units including the terminating NUL.
const TIP_MAX: usize = 127;
/// `NIN_SELECT | NINF_KEY`: Enter/Space on the focused icon.
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
/// A click reported by the other channel within this window is the same click.
const DEDUPE_MS: u64 = 500;

/// What a tray callback asks the card for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrayAction {
    /// Left click (or keyboard select): `Input::TrayClick`.
    Click,
    /// Context menu at this screen point.
    Menu(POINT),
}

/// Which callback reported a left click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClickSource {
    ButtonUp,
    Select,
}

/// Version-4 icons report one left click as `WM_LBUTTONUP` *and*
/// `NIN_SELECT`. The first one counts; the other one, arriving within
/// 500 ms, is dropped. Two reports from the same channel (a
/// double-click) are two clicks.
#[derive(Debug, Default)]
pub struct ClickDedupe {
    last: Option<(ClickSource, u64)>,
}

impl ClickDedupe {
    fn accept(&mut self, src: ClickSource, now: u64) -> bool {
        if let Some((prev, at)) = self.last
            && prev != src
            && now.saturating_sub(at) < DEDUPE_MS
        {
            // The pair is consumed: the next report starts afresh.
            self.last = None;
            return false;
        }
        self.last = Some((src, now));
        true
    }
}

/// Decodes a version-4 tray callback: `LOWORD(lParam)` is the event and,
/// for `WM_CONTEXTMENU`, `wParam` carries the anchor point.
pub fn decode(wp: usize, lp: isize, now: u64, d: &mut ClickDedupe) -> Option<TrayAction> {
    let event = (lp as u32) & 0xFFFF;
    match event {
        WM_LBUTTONUP => d
            .accept(ClickSource::ButtonUp, now)
            .then_some(TrayAction::Click),
        NIN_SELECT => d
            .accept(ClickSource::Select, now)
            .then_some(TrayAction::Click),
        NIN_KEYSELECT => Some(TrayAction::Click),
        WM_CONTEXTMENU => {
            let v = wp as u32;
            Some(TrayAction::Menu(POINT {
                x: i32::from(v as u16 as i16),
                y: i32::from((v >> 16) as u16 as i16),
            }))
        }
        _ => None,
    }
}

/// Glyph colour for the tray dot (spec §7): healthy `muted`, `warning`,
/// dire `dire_accent`.
pub fn dot_color(dot: Dot, pal: &Palette) -> Rgb {
    match dot {
        Dot::Muted => pal.muted,
        Dot::Warning => pal.warning,
        Dot::DireAccent => pal.dire_accent,
    }
}

/// `s` as at most 127 UTF-16 units (`szTip` less its NUL), never
/// splitting a surrogate pair.
pub fn tip_units(s: &str) -> Vec<u16> {
    let mut out = Vec::with_capacity(TIP_MAX);
    for c in s.chars() {
        let mut buf = [0u16; 2];
        let units = c.encode_utf16(&mut buf);
        if out.len() + units.len() > TIP_MAX {
            break;
        }
        out.extend_from_slice(units);
    }
    out
}

/// Glyph stroke width (px): 1 at 16 px, scaling with the icon.
fn stroke(n: i32) -> i32 {
    (n as f32 / 16.0).round().max(1.0) as i32
}

/// The disk body's rectangle: the lower part of the icon when the Zz
/// mark needs the top.
fn body_rect(n: i32, zz: bool) -> Rect {
    let f = |k: f32| (n as f32 * k).round() as i32;
    let lw = stroke(n);
    let (y, h) = if zz {
        (f(0.42), f(0.50))
    } else {
        (f(0.22), f(0.56))
    };
    Rect::new(lw, y, n - 2 * lw, h)
}

/// Straight-alpha source-over of `c` at coverage `a` onto `dst`.
fn over(dst: u32, c: Rgb, a: f32) -> u32 {
    if a <= 0.0 {
        return dst;
    }
    let a = a.min(1.0);
    let da = (dst >> 24) as f32 / 255.0 * (1.0 - a);
    let oa = a + da;
    let ch = |src: u8, shift: u32| {
        let d = ((dst >> shift) & 0xFF) as f32;
        ((f32::from(src) * a + d * da) / oa).round() as u32
    };
    let alpha = (oa * 255.0).round() as u32;
    (alpha << 24) | (ch(c.0, 16) << 16) | (ch(c.1, 8) << 8) | ch(c.2, 0)
}

/// Coverage of a `width`-px stroke along segment `a`–`b` at `(px, py)`.
fn segment_coverage(px: f32, py: f32, a: (f32, f32), b: (f32, f32), width: f32) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = (dx * dx + dy * dy).max(f32::EPSILON);
    let t = (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0);
    let d = (px - (a.0 + t * dx)).hypot(py - (a.1 + t * dy));
    (width / 2.0 + 0.5 - d).clamp(0.0, 1.0)
}

/// The strokes of the glyph in `outline`: platter line, LED dot, and the
/// "Zz" mark when `zz`. Endpoints sit on pixel centres so straight runs
/// are crisp.
fn strokes(n: i32, body: Rect, zz: bool) -> Vec<((f32, f32), (f32, f32))> {
    let c = |v: f32| v.floor() + 0.5;
    let f = |k: f32| c(n as f32 * k);
    let (bx, bw) = (body.x as f32, body.w as f32);
    let line_y = c(body.y as f32 + body.h as f32 * 0.68);
    let led = c(bx + bw * 0.80);
    let mut s = vec![
        ((c(bx + bw * 0.16), line_y), (c(bx + bw * 0.58), line_y)),
        ((led, line_y), (led, line_y)),
    ];
    if zz {
        for (x0, x1, y0, y1) in [(0.30, 0.62, 0.06, 0.36), (0.68, 0.90, 0.20, 0.36)] {
            let (x0, x1, y0, y1) = (f(x0), f(x1), f(y0), f(y1));
            s.extend([
                ((x0, y0), (x1, y0)),
                ((x1, y0), (x0, y1)),
                ((x0, y1), (x1, y1)),
            ]);
        }
    }
    s
}

/// PURE: the `size × size` tray glyph as straight-alpha `0xAARRGGBB`
/// pixels, top-down — a disk silhouette (rounded body in `dot`, `outline`
/// edge and platter line with an LED dot) plus a "Zz" mark above it when
/// `zz`.
pub fn glyph_pixels(size: i32, dot: Rgb, outline: Rgb, zz: bool) -> Vec<u32> {
    let n = size.max(1);
    let lw = stroke(n);
    let body = body_rect(n, zz);
    let inner = Rect::new(body.x + lw, body.y + lw, body.w - 2 * lw, body.h - 2 * lw);
    let radius = (n as f32 * 0.14).max(2.0);
    let lines = strokes(n, body, zz);
    let mut buf = vec![0u32; (n * n) as usize];
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let i = (y * n + x) as usize;
            buf[i] = over(
                buf[i],
                outline,
                rounded_rect_coverage(px, py, &body, radius),
            );
            let fill = rounded_rect_coverage(px, py, &inner, radius - lw as f32);
            buf[i] = over(buf[i], dot, fill);
            let ink = lines
                .iter()
                .map(|&(a, b)| segment_coverage(px, py, a, b, lw as f32))
                .fold(0.0, f32::max);
            buf[i] = over(buf[i], outline, ink);
        }
    }
    buf
}

/// Effective DPI of the primary monitor (the notification area's), 96 on
/// failure.
pub fn taskbar_dpi() -> u32 {
    // SAFETY: no pointers in; a primary monitor always exists.
    let mon = unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) };
    let (mut x, mut y) = (0u32, 0u32);
    // SAFETY: `x`/`y` are valid out-pointers for the call.
    match unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut x, &mut y) } {
        Ok(()) if x > 0 => x,
        _ => 96,
    }
}

/// Small-icon size at `dpi`.
fn icon_size(dpi: u32) -> i32 {
    // SAFETY: no pointers.
    let n = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    if n > 0 { n } else { theme::scale(16, dpi) }
}

/// An alpha icon from straight-alpha pixels (the mask is ignored by the
/// shell for 32-bpp icons but must exist).
fn make_icon(px: &[u32], n: i32) -> DrivecapResult<HICON> {
    let mask_bytes = vec![0u8; ((n as usize).div_ceil(16) * 2) * n as usize];
    // SAFETY: `px` holds n×n 32-bpp pixels and `mask_bytes` n rows of
    // word-aligned 1-bpp bits; both outlive the calls. The bitmaps are
    // deleted after CreateIconIndirect copied them.
    unsafe {
        let color = CreateBitmap(n, n, 1, 32, Some(px.as_ptr().cast()));
        let mask = CreateBitmap(n, n, 1, 1, Some(mask_bytes.as_ptr().cast()));
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        Ok(icon?)
    }
}

/// What the current icon was drawn from; equal keys skip the update.
#[derive(Clone, Debug, PartialEq)]
struct IconKey {
    dot: Rgb,
    outline: Rgb,
    zz: bool,
    size: i32,
    tip: String,
}

impl IconKey {
    fn new(st: &TrayState, pal: &Palette, dpi: u32) -> IconKey {
        IconKey {
            dot: dot_color(st.dot, pal),
            outline: pal.text,
            zz: st.all_snoozed,
            size: icon_size(dpi),
            tip: st.tooltip.clone(),
        }
    }

    fn icon(&self) -> DrivecapResult<HICON> {
        let px = glyph_pixels(self.size, self.dot, self.outline, self.zz);
        make_icon(&px, self.size)
    }
}

/// The notification-area icon. Dropping it deletes the icon.
pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    key: IconKey,
    /// `NIM_ADD` succeeded (Explorer may not be up yet at login; the
    /// `TaskbarCreated` broadcast or the next update retries).
    added: bool,
    clicks: ClickDedupe,
}

impl Tray {
    /// Draws the icon and adds it (`NIM_ADD` + `NIM_SETVERSION(4)`). Fails
    /// only if the icon cannot be built; a refused `NIM_ADD` is logged and
    /// retried later.
    pub fn add(hwnd: HWND, st: &TrayState, pal: &Palette, dpi: u32) -> DrivecapResult<Tray> {
        let key = IconKey::new(st, pal, dpi);
        let icon = key.icon()?;
        let mut tray = Tray {
            hwnd,
            icon,
            key,
            added: false,
            clicks: ClickDedupe::default(),
        };
        tray.readd();
        Ok(tray)
    }

    /// Redraws and modifies the icon when anything it shows changed.
    pub fn update(&mut self, st: &TrayState, pal: &Palette, dpi: u32) {
        let key = IconKey::new(st, pal, dpi);
        if key == self.key && self.added {
            return;
        }
        let redraw = (key.dot, key.outline, key.zz, key.size)
            != (self.key.dot, self.key.outline, self.key.zz, self.key.size);
        // The shell keeps its own copy, so the old icon goes once replaced.
        let retired = if redraw {
            key.icon()
                .inspect_err(|e| log::warn!("tray: cannot draw icon ({e}); keeping the old one"))
                .ok()
                .map(|icon| std::mem::replace(&mut self.icon, icon))
        } else {
            None
        };
        self.key = key;
        if self.added {
            if !self.notify(NIM_MODIFY) {
                log::warn!("tray: NIM_MODIFY failed");
            }
        } else {
            self.readd();
        }
        retired.into_iter().for_each(destroy_icon);
    }

    /// (Re-)adds the icon — after `TaskbarCreated` (Explorer restarted).
    pub fn readd(&mut self) {
        self.added = self.notify(NIM_ADD);
        if !self.added {
            log::warn!("tray: NIM_ADD failed; retrying on TaskbarCreated / next update");
            return;
        }
        if !self.set_version() {
            log::warn!("tray: NIM_SETVERSION(4) failed");
        }
        log::info!("tray icon added");
    }

    /// Decodes a [`WM_APP_TRAY`] callback.
    pub fn on_callback(&mut self, wp: usize, lp: isize, now: u64) -> Option<TrayAction> {
        decode(wp, lp, now, &mut self.clicks)
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_UID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        let tip = tip_units(&self.key.tip);
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        nid
    }

    fn notify(&self, msg: NOTIFY_ICON_MESSAGE) -> bool {
        let nid = self.data();
        // SAFETY: `nid` is fully initialised and outlives the call.
        unsafe { Shell_NotifyIconW(msg, &nid) }.as_bool()
    }

    fn set_version(&self) -> bool {
        let mut nid = self.data();
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // SAFETY: as in `notify`.
        unsafe { Shell_NotifyIconW(NIM_SETVERSION, &nid) }.as_bool()
    }
}

fn destroy_icon(icon: HICON) {
    // SAFETY: an icon this module created and no longer hands to the shell.
    if let Err(e) = unsafe { DestroyIcon(icon) } {
        log::debug!("tray: DestroyIcon failed: {e}");
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if self.added && !self.notify(NIM_DELETE) {
            log::debug!("tray: NIM_DELETE failed");
        }
        destroy_icon(self.icon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ColorOverrides;

    const DOT: Rgb = Rgb(0xC7, 0x78, 0x00);
    const INK: Rgb = Rgb(0x1B, 0x1B, 0x1B);

    fn opaque(c: Rgb) -> u32 {
        0xFF00_0000 | (u32::from(c.0) << 16) | (u32::from(c.1) << 8) | u32::from(c.2)
    }

    fn body_centre(n: i32, zz: bool) -> usize {
        let b = body_rect(n, zz);
        let (x, y) = (b.x + b.w / 2, b.y + b.h / 3);
        (y * n + x) as usize
    }

    #[test]
    fn glyph_uses_severity_color() {
        for (n, zz) in [(16, false), (32, false), (16, true), (24, true)] {
            let px = glyph_pixels(n, DOT, INK, zz);
            assert_eq!(px[body_centre(n, zz)], opaque(DOT), "size {n}, zz {zz}");
        }
    }

    #[test]
    fn glyph_has_outline_and_transparent_corner() {
        let px = glyph_pixels(32, DOT, INK, false);
        assert_eq!(px[0] >> 24, 0, "top-left corner is transparent");
        let b = body_rect(32, false);
        let edge = (b.y + b.h / 2) * 32 + b.x;
        assert_eq!(px[edge as usize], opaque(INK), "left edge is outline");
    }

    #[test]
    fn glyph_zz_differs() {
        let plain = glyph_pixels(16, DOT, INK, false);
        let zz = glyph_pixels(16, DOT, INK, true);
        assert_ne!(plain, zz);
        // The mark sits above the body, in the outline colour.
        let top = &zz[..(body_rect(16, true).y * 16) as usize];
        assert!(top.iter().any(|&p| p == opaque(INK)));
    }

    #[test]
    fn glyph_size_matches() {
        for n in [16, 20, 24, 32] {
            assert_eq!(glyph_pixels(n, DOT, INK, false).len(), (n * n) as usize);
        }
    }

    #[test]
    fn tooltip_truncates_to_127() {
        let long = "x".repeat(300);
        assert_eq!(tip_units(&long).len(), 127);
        assert_eq!(tip_units("Drivecap — C: 14.8 GB free").len(), 26);
    }

    #[test]
    fn tooltip_never_splits_a_surrogate_pair() {
        // 126 units, then a 2-unit emoji straddling the limit.
        let s = format!("{}😀", "x".repeat(126));
        let t = tip_units(&s);
        assert_eq!(t.len(), 126);
        assert!(String::from_utf16(&t).is_ok());
    }

    #[test]
    fn severity_maps_to_tokens() {
        let p = Palette::new(false, &ColorOverrides::default());
        assert_eq!(dot_color(Dot::Muted, &p), p.muted);
        assert_eq!(dot_color(Dot::Warning, &p), p.warning);
        assert_eq!(dot_color(Dot::DireAccent, &p), p.dire_accent);
    }

    const UP: isize = WM_LBUTTONUP as isize;
    const SEL: isize = NIN_SELECT as isize;

    #[test]
    fn button_up_and_select_are_one_click() {
        let mut d = ClickDedupe::default();
        assert_eq!(decode(0, UP, 1_000, &mut d), Some(TrayAction::Click));
        assert_eq!(decode(0, SEL, 1_010, &mut d), None);
        // Either order.
        assert_eq!(decode(0, SEL, 5_000, &mut d), Some(TrayAction::Click));
        assert_eq!(decode(0, UP, 5_010, &mut d), None);
    }

    #[test]
    fn select_alone_and_slow_pairs_both_count() {
        let mut d = ClickDedupe::default();
        assert_eq!(decode(0, SEL, 1_000, &mut d), Some(TrayAction::Click));
        assert_eq!(
            decode(0, UP, 1_000 + DEDUPE_MS, &mut d),
            Some(TrayAction::Click)
        );
        // Same channel twice (double-click): two clicks.
        assert_eq!(decode(0, SEL, 9_000, &mut d), Some(TrayAction::Click));
        assert_eq!(decode(0, SEL, 9_100, &mut d), Some(TrayAction::Click));
    }

    #[test]
    fn context_menu_anchor_comes_from_wparam() {
        let mut d = ClickDedupe::default();
        // High word of lParam (icon id) is ignored; x = -5, y = 1040.
        let wp = ((1040u32 << 16) | 0xFFFB) as usize;
        let lp = ((1 << 16) | WM_CONTEXTMENU) as isize;
        assert_eq!(
            decode(wp, lp, 0, &mut d),
            Some(TrayAction::Menu(POINT { x: -5, y: 1040 }))
        );
        let other = (1 << 16) | 0x0200; // WM_MOUSEMOVE
        assert_eq!(decode(0, other, 0, &mut d), None);
    }

    #[test]
    fn keyboard_select_is_a_click() {
        let mut d = ClickDedupe::default();
        assert_eq!(
            decode(0, NIN_KEYSELECT as isize, 0, &mut d),
            Some(TrayAction::Click)
        );
    }

    /// Renders the glyph at 32 px into `./tmp/tray-*.bmp` for eyeballing.
    #[test]
    #[ignore = "writes ./tmp/tray-*.bmp for manual review"]
    fn dump_tray_glyphs() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
        std::fs::create_dir_all(&dir).unwrap();
        for dark in [false, true] {
            let p = Palette::new(dark, &ColorOverrides::default());
            for (name, dot, zz) in [
                ("healthy", Dot::Muted, false),
                ("warning", Dot::Warning, false),
                ("dire", Dot::DireAccent, false),
                ("snoozed", Dot::Muted, true),
            ] {
                let px = glyph_pixels(32, dot_color(dot, &p), p.text, zz);
                let bg = if dark { 0x20 } else { 0xE8 };
                let theme = if dark { "dark" } else { "light" };
                let path = dir.join(format!("tray-{name}-{theme}.bmp"));
                write_bmp(&path, &px, 32, bg);
            }
        }
    }

    /// 32-bpp top-down BMP (what `tmp/bmp2png.mjs` reads) of
    /// straight-alpha pixels composited over grey `bg`.
    fn write_bmp(path: &std::path::Path, px: &[u32], n: i32, bg: u32) {
        let size = 54 + 4 * n * n;
        let mut out = Vec::with_capacity(size as usize);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(-n).to_le_bytes());
        out.extend_from_slice(&[1, 0, 32, 0]);
        out.extend_from_slice(&[0u8; 24]);
        for &p in px {
            let a = p >> 24;
            for shift in [0, 8, 16] {
                let c = (p >> shift) & 0xFF;
                out.push(((c * a + bg * (255 - a)) / 255) as u8);
            }
            out.push(255);
        }
        std::fs::write(path, out).unwrap();
    }
}
