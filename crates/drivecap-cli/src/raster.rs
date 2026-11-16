//! Pure per-pixel helpers for the card's 32-bpp buffer. Pixels are `u32`
//! BGRA in memory (`0xAARRGGBB` as a little-endian word). Before
//! [`apply_card_mask`] the buffer holds opaque RGB with the alpha byte
//! ignored (GDI leaves it 0); afterwards it is premultiplied BGRA ready
//! for `UpdateLayeredWindow`.
//!
//! Coordinates passed to [`rounded_rect_coverage`] are pixel centres
//! (`x + 0.5`), so integer-aligned straight edges come out crisp and only
//! curves get the 1-px antialiased ramp.

use crate::layout::Rect;
use crate::palette::{Rgb, Rgba};

/// Halo cycle length.
pub const HALO_PERIOD_MS: u64 = 2400;
/// Halo grow-and-fade span; the rest of the period is a rest.
pub const HALO_ACTIVE_MS: u64 = 1700;
/// Halo peak alpha (60 %).
const HALO_PEAK_ALPHA: f32 = 153.0;
/// Halo ring's full outward travel (logical px).
const HALO_TRAVEL: f32 = 8.0;
/// Pulsing-border alpha floor (40 %).
const PULSE_MIN_ALPHA: f32 = 102.0;

/// Signed distance from `(px, py)` to the rounded rectangle's edge:
/// negative inside, positive outside. `radius` is clamped to half the
/// shorter side.
fn rounded_rect_sdf(px: f32, py: f32, r: &Rect, radius: f32) -> f32 {
    let (hw, hh) = (r.w as f32 / 2.0, r.h as f32 / 2.0);
    let rad = radius.clamp(0.0, hw.min(hh).max(0.0));
    let qx = (px - (r.x as f32 + hw)).abs() - (hw - rad);
    let qy = (py - (r.y as f32 + hh)).abs() - (hh - rad);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    outside + qx.max(qy).min(0.0) - rad
}

/// Fraction (0..=1) of the pixel centred at `(px, py)` covered by the
/// rounded rectangle, with a 1-px linear ramp across the edge.
pub fn rounded_rect_coverage(px: f32, py: f32, r: &Rect, radius: f32) -> f32 {
    (0.5 - rounded_rect_sdf(px, py, r, radius)).clamp(0.0, 1.0)
}

/// Halo alpha `dist_outside` px beyond the card edge at `phase_ms` (looped
/// over [`HALO_PERIOD_MS`]). For `p ≤ 1700` the ring reaches
/// `8·s·p/1700` px out at alpha `153·(1 − p/1700)`, with a 1-px soft outer
/// edge; the remainder of the period is transparent.
pub fn halo_alpha(dist_outside: f32, phase_ms: u64, s: f32) -> u8 {
    let p = phase_ms % HALO_PERIOD_MS;
    if p >= HALO_ACTIVE_MS {
        return 0;
    }
    let t = p as f32 / HALO_ACTIVE_MS as f32;
    let extent = HALO_TRAVEL * s * t;
    let edge = (extent + 1.0 - dist_outside.max(0.0)).clamp(0.0, 1.0);
    (HALO_PEAK_ALPHA * (1.0 - t) * edge).round() as u8
}

/// Fallback border alpha: cosine ease 255 → 102 → 255 over 2.4 s.
pub fn border_pulse_alpha(phase_ms: u64) -> u8 {
    let t = (phase_ms % HALO_PERIOD_MS) as f32 / HALO_PERIOD_MS as f32;
    let k = 0.5 + 0.5 * (std::f32::consts::TAU * t).cos();
    (PULSE_MIN_ALPHA + (255.0 - PULSE_MIN_ALPHA) * k).round() as u8
}

fn channels(px: u32) -> [u32; 4] {
    [px >> 24, (px >> 16) & 0xFF, (px >> 8) & 0xFF, px & 0xFF]
}

fn pack(a: u32, r: u32, g: u32, b: u32) -> u32 {
    (a.min(255) << 24) | (r.min(255) << 16) | (g.min(255) << 8) | b.min(255)
}

/// `c · a / 255`, rounded.
fn scale(c: u32, a: u32) -> u32 {
    (c * a + 127) / 255
}

/// Opaque RGB pixel → premultiplied BGRA with alpha `coverage · 255`.
pub fn premultiply(px: u32, coverage: f32) -> u32 {
    let a = (coverage.clamp(0.0, 1.0) * 255.0).round() as u32;
    let [_, r, g, b] = channels(px);
    pack(a, scale(r, a), scale(g, a), scale(b, a))
}

/// Calls `f(index, x_centre, y_centre)` for every buffer pixel inside `r`
/// grown by `grow` px on each side, clipped to the buffer.
fn each_pixel(w: i32, h: i32, r: &Rect, grow: i32, mut f: impl FnMut(usize, f32, f32)) {
    let (x0, y0) = ((r.x - grow).max(0), (r.y - grow).max(0));
    let (x1, y1) = ((r.right() + grow).min(w), (r.bottom() + grow).min(h));
    for y in y0..y1 {
        for x in x0..x1 {
            f((y * w + x) as usize, x as f32 + 0.5, y as f32 + 0.5);
        }
    }
}

/// Rewrites alpha from the card's rounded-rect coverage and premultiplies;
/// every pixel outside the card becomes fully transparent.
pub fn apply_card_mask(buf: &mut [u32], w: i32, h: i32, card: &Rect, radius: f32) {
    each_pixel(w, h, &Rect::new(0, 0, w, h), 0, |i, x, y| {
        let cov = rounded_rect_coverage(x, y, card, radius);
        buf[i] = if cov > 0.0 {
            premultiply(buf[i], cov)
        } else {
            0
        };
    });
}

/// `color` at alpha `a`, premultiplied, composited under (`under`) or
/// over the premultiplied `dst`.
fn composite(dst: u32, color: Rgb, a: u32, under: bool) -> u32 {
    let src = [
        a,
        scale(color.0.into(), a),
        scale(color.1.into(), a),
        scale(color.2.into(), a),
    ];
    let d = channels(dst);
    let (top, bottom) = if under { (d, src) } else { (src, d) };
    let keep = 255 - top[0];
    let mix = |k: usize| top[k] + scale(bottom[k], keep);
    pack(mix(0), mix(1), mix(2), mix(3))
}

/// Paints the dire halo into the transparent margin around `card`, behind
/// the card's antialiased edge. The buffer must already be masked.
#[allow(clippy::too_many_arguments)] // signature fixed by the task brief
pub fn draw_halo(
    buf: &mut [u32],
    w: i32,
    h: i32,
    card: &Rect,
    radius: f32,
    color: Rgb,
    phase_ms: u64,
    s: f32,
) {
    let reach = (HALO_TRAVEL * s).ceil() as i32 + 2;
    each_pixel(w, h, card, reach, |i, x, y| {
        let d = rounded_rect_sdf(x, y, card, radius);
        if d <= -0.5 {
            return;
        }
        let a = halo_alpha(d, phase_ms, s);
        if a > 0 {
            buf[i] = composite(buf[i], color, a.into(), true);
        }
    });
}

/// Coverage of the band between `r`'s edge and `width` px inside it.
fn ring_coverage(px: f32, py: f32, r: &Rect, radius: f32, width: f32) -> f32 {
    let d = rounded_rect_sdf(px, py, r, radius);
    ((0.5 - d).clamp(0.0, 1.0) - (0.5 - d - width).clamp(0.0, 1.0)).max(0.0)
}

/// Fallback dire treatment: a `width`-px band just inside the card edge in
/// `color` at `alpha`, composited over the masked (premultiplied) card.
#[allow(clippy::too_many_arguments)] // mirrors draw_halo's shape
pub fn draw_pulse_border(
    buf: &mut [u32],
    w: i32,
    h: i32,
    card: &Rect,
    radius: f32,
    color: Rgb,
    alpha: u8,
    width: f32,
) {
    each_pixel(w, h, card, 0, |i, x, y| {
        let cov = ring_coverage(x, y, card, radius, width);
        if cov > 0.0 {
            let a = (cov * f32::from(alpha)).round() as u32;
            buf[i] = composite(buf[i], color, a, false);
        }
    });
}

/// Pre-mask: blends `color` into the RGB channels at alpha `a`.
fn blend_rgb(dst: u32, color: Rgb, a: u32) -> u32 {
    let [_, r, g, b] = channels(dst);
    let mix = |c: u8, d: u32| scale(c.into(), a) + scale(d, 255 - a);
    pack(0, mix(color.0, r), mix(color.1, g), mix(color.2, b))
}

/// Pre-mask: fills a rounded rectangle with `color`, antialiased by coverage.
pub fn fill_rounded(buf: &mut [u32], w: i32, h: i32, r: &Rect, radius: f32, color: Rgb) {
    each_pixel(w, h, r, 0, |i, x, y| {
        let cov = rounded_rect_coverage(x, y, r, radius);
        if cov > 0.0 {
            buf[i] = blend_rgb(buf[i], color, (cov * 255.0).round() as u32);
        }
    });
}

/// Pre-mask: strokes a `width`-px band just inside `r`'s rounded edge
/// with a translucent colour (the card border).
#[allow(clippy::too_many_arguments)] // buffer triple + geometry + paint
pub fn stroke_ring(
    buf: &mut [u32],
    w: i32,
    h: i32,
    r: &Rect,
    radius: f32,
    width: f32,
    color: Rgba,
) {
    each_pixel(w, h, r, 0, |i, x, y| {
        let cov = ring_coverage(x, y, r, radius, width);
        if cov > 0.0 {
            let a = (cov * f32::from(color.a)).round() as u32;
            buf[i] = blend_rgb(buf[i], color.rgb, a);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect = Rect::new(10, 10, 40, 20);

    #[test]
    fn coverage_inside_outside_edge() {
        assert_eq!(rounded_rect_coverage(30.5, 20.5, &R, 4.0), 1.0);
        assert_eq!(rounded_rect_coverage(5.5, 20.5, &R, 4.0), 0.0);
        // First pixel inside the left edge is full, the one before it empty.
        assert_eq!(rounded_rect_coverage(10.5, 20.5, &R, 4.0), 1.0);
        assert_eq!(rounded_rect_coverage(9.5, 20.5, &R, 4.0), 0.0);
        // A sample exactly on the edge is half covered.
        assert!((rounded_rect_coverage(10.0, 20.5, &R, 4.0) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn corner_is_antialiased() {
        // The very corner pixel lies outside the radius-8 arc.
        assert_eq!(rounded_rect_coverage(10.5, 10.5, &R, 8.0), 0.0);
        // Somewhere on the arc a pixel is partially covered.
        let partial = (0..8).any(|i| {
            let c = rounded_rect_coverage(10.5 + i as f32, 10.5 + i as f32 * 0.4, &R, 8.0);
            c > 0.05 && c < 0.95
        });
        assert!(partial);
        // Without a radius the corner pixel is full.
        assert_eq!(rounded_rect_coverage(10.5, 10.5, &R, 0.0), 1.0);
    }

    #[test]
    fn halo_peak_and_rest() {
        let peak = halo_alpha(0.0, 0, 1.0);
        assert!((152..=153).contains(&peak), "peak {peak}");
        assert_eq!(halo_alpha(0.0, 1700, 1.0), 0);
        assert_eq!(halo_alpha(0.0, 2000, 1.0), 0);
        // Mid-cycle: inside the ring, partly faded; beyond it, nothing.
        let mid = halo_alpha(1.0, 850, 1.0);
        assert!(mid > 60 && mid < 90, "mid {mid}");
        assert_eq!(halo_alpha(7.0, 850, 1.0), 0);
        // Loops every 2400 ms.
        assert_eq!(halo_alpha(0.0, 2400, 1.0), peak);
        // Extent scales with s.
        assert!(halo_alpha(7.0, 850, 2.0) > 0);
    }

    #[test]
    fn border_pulse_bounds() {
        assert_eq!(border_pulse_alpha(0), 255);
        assert_eq!(border_pulse_alpha(1200), 102);
        assert_eq!(border_pulse_alpha(2400), 255);
        for p in (0..4800).step_by(37) {
            let a = border_pulse_alpha(p);
            assert!((102..=255).contains(&a), "phase {p} alpha {a}");
        }
    }

    #[test]
    fn mask_premultiplies() {
        let (w, h) = (8, 8);
        let mut buf = vec![0x00FF_8040u32; (w * h) as usize];
        apply_card_mask(&mut buf, w, h, &Rect::new(2, 2, 4, 4), 0.0);
        assert_eq!(buf[(3 * w + 3) as usize], 0xFFFF_8040);
        assert_eq!(buf[0], 0, "outside the card is zeroed");
        let half = premultiply(0x00FF_8040, 0.5);
        assert_eq!(half >> 24, 0x80);
        let ch = |px: u32, sh: u32| (px >> sh) & 0xFF;
        assert!(ch(half, 16).abs_diff(0x80) <= 1);
        assert!(ch(half, 8).abs_diff(0x40) <= 1);
        assert!(ch(half, 0).abs_diff(0x20) <= 1);
    }

    #[test]
    fn fill_rounded_blends_by_coverage() {
        let (w, h) = (20, 20);
        let mut buf = vec![0u32; (w * h) as usize];
        fill_rounded(
            &mut buf,
            w,
            h,
            &Rect::new(0, 0, 20, 20),
            6.0,
            Rgb(0xFF, 0, 0),
        );
        assert_eq!(buf[(10 * w + 10) as usize] & 0x00FF_FFFF, 0x00FF_0000);
        assert_eq!(buf[0] & 0x00FF_FFFF, 0, "corner stays background");
    }

    #[test]
    fn halo_paints_outside_only() {
        let (w, h) = (40, 40);
        let card = Rect::new(8, 8, 24, 24);
        let mut buf = vec![0x00F9_F9F9u32; (w * h) as usize];
        apply_card_mask(&mut buf, w, h, &card, 0.0);
        let inside = buf[(20 * w + 20) as usize];
        draw_halo(&mut buf, w, h, &card, 0.0, Rgb(0xE5, 0x37, 0x2B), 850, 1.0);
        assert_eq!(buf[(20 * w + 20) as usize], inside, "card untouched");
        let ring = buf[(20 * w + 6) as usize]; // 1.5 px left of the card
        assert!(ring >> 24 > 0, "ring pixel has alpha");
        assert_eq!(buf[0], 0, "beyond the ring stays clear");
    }

    #[test]
    fn pulse_border_rings_card_edge() {
        let (w, h) = (40, 40);
        let card = Rect::new(8, 8, 24, 24);
        let mut buf = vec![0x00F9_F9F9u32; (w * h) as usize];
        apply_card_mask(&mut buf, w, h, &card, 0.0);
        draw_pulse_border(&mut buf, w, h, &card, 0.0, Rgb(0xE5, 0x37, 0x2B), 255, 2.0);
        assert_eq!(buf[(20 * w + 8) as usize], 0xFFE5_372B);
        assert_eq!(buf[(20 * w + 9) as usize], 0xFFE5_372B);
        assert_eq!(buf[(20 * w + 10) as usize], 0xFFF9_F9F9);
        assert_eq!(buf[(20 * w + 7) as usize], 0);
    }

    #[test]
    fn border_ring_blends_translucent_colour() {
        let (w, h) = (20, 20);
        let mut buf = vec![0x00FF_FFFFu32; (w * h) as usize];
        let border = Rgba::new(Rgb(0, 0, 0), 128);
        stroke_ring(&mut buf, w, h, &Rect::new(0, 0, 20, 20), 0.0, 1.0, border);
        let edge = buf[(10 * w) as usize] & 0xFF;
        assert!(edge.abs_diff(0x7F) <= 1, "edge {edge:#x}");
        assert_eq!(buf[(10 * w + 1) as usize] & 0x00FF_FFFF, 0x00FF_FFFF);
    }
}
