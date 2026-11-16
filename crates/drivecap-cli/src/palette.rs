//! Design tokens for the card (handoff "Design tokens" table), light and
//! dark, with optional `#RRGGBB` overrides from `config.json`.
//!
//! Translucent tokens (`border`, `track`, `hover`) are [`Rgba`]; GDI paints
//! them either through per-pixel alpha or pre-flattened with
//! [`Palette::flat`] over the colour underneath. Alpha percentages from the
//! handoff are rounded with `pct * 255 / 100` (16 % → 41).

use crate::config::{ColorOverrides, PaletteOverride};

/// Opaque 8-bit RGB colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Pure white (dire-row content).
    pub const WHITE: Rgb = Rgb(0xFF, 0xFF, 0xFF);
    /// Pure black.
    pub const BLACK: Rgb = Rgb(0, 0, 0);

    /// From `0xRRGGBB`.
    pub const fn hex(v: u32) -> Rgb {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Parses `#RRGGBB`; `None` for anything else.
    pub fn parse(s: &str) -> Option<Rgb> {
        let digits = s.strip_prefix('#')?;
        if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(digits, 16).ok().map(Rgb::hex)
    }
}

/// RGB plus 8-bit alpha (255 = opaque).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgba {
    pub rgb: Rgb,
    pub a: u8,
}

impl Rgba {
    /// `rgb` at `a`.
    pub const fn new(rgb: Rgb, a: u8) -> Rgba {
        Rgba { rgb, a }
    }
}

/// Alpha for 35 % — the dire row's bar track (white).
pub const DIRE_TRACK_ALPHA: u8 = 89;
/// Alpha for 55 % — the header grip dots (text colour).
pub const GRIP_ALPHA: u8 = 140;

/// Every colour the card paints with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub surface: Rgb,
    pub border: Rgba,
    pub text: Rgb,
    pub text_secondary: Rgb,
    pub muted: Rgb,
    pub track: Rgba,
    pub hover: Rgba,
    pub warning: Rgb,
    pub warning_text: Rgb,
    pub dire: Rgb,
    pub dire_accent: Rgb,
}

const DIRE: Rgb = Rgb::hex(0xC42B1C);
const DIRE_ACCENT: Rgb = Rgb::hex(0xE5372B);

const LIGHT: Palette = Palette {
    surface: Rgb::hex(0xF9F9F9),
    border: Rgba::new(Rgb::BLACK, 41),
    text: Rgb::hex(0x1B1B1B),
    text_secondary: Rgb::hex(0x575757),
    muted: Rgb::hex(0x6B6B6B),
    track: Rgba::new(Rgb::BLACK, 36),
    hover: Rgba::new(Rgb::BLACK, 15),
    warning: Rgb::hex(0xC77800),
    warning_text: Rgb::hex(0x8A5100),
    dire: DIRE,
    dire_accent: DIRE_ACCENT,
};

const DARK: Palette = Palette {
    surface: Rgb::hex(0x2B2B2B),
    border: Rgba::new(Rgb::WHITE, 41),
    text: Rgb::hex(0xFFFFFF),
    text_secondary: Rgb::hex(0xC8C8C8),
    muted: Rgb::hex(0xA0A0A0),
    track: Rgba::new(Rgb::WHITE, 46),
    hover: Rgba::new(Rgb::WHITE, 20),
    warning: Rgb::hex(0xF5B83D),
    warning_text: Rgb::hex(0xF5B83D),
    dire: DIRE,
    dire_accent: DIRE_ACCENT,
};

impl Palette {
    /// The light or dark token set with that theme's overrides applied.
    /// Unparseable override strings are ignored (config load already
    /// drops them; this is belt and braces).
    pub fn new(dark: bool, ov: &ColorOverrides) -> Palette {
        let (mut p, o) = if dark {
            (DARK, &ov.dark)
        } else {
            (LIGHT, &ov.light)
        };
        p.apply(o);
        p
    }

    fn apply(&mut self, o: &PaletteOverride) {
        let slots: [(&mut Rgb, &Option<String>); 6] = [
            (&mut self.warning, &o.warning),
            (&mut self.warning_text, &o.warning_text),
            (&mut self.dire, &o.dire),
            (&mut self.dire_accent, &o.dire_accent),
            (&mut self.text, &o.text),
            (&mut self.surface, &o.surface),
        ];
        for (slot, value) in slots {
            if let Some(c) = value.as_deref().and_then(Rgb::parse) {
                *slot = c;
            }
        }
    }

    /// Alpha-composites `c` over the opaque `over` (source-over, rounded).
    pub fn flat(&self, c: Rgba, over: Rgb) -> Rgb {
        let mix = |src: u8, dst: u8| -> u8 {
            let a = u32::from(c.a);
            ((u32::from(src) * a + u32::from(dst) * (255 - a) + 127) / 255) as u8
        };
        Rgb(
            mix(c.rgb.0, over.0),
            mix(c.rgb.1, over.1),
            mix(c.rgb.2, over.2),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_overrides() -> ColorOverrides {
        ColorOverrides::default()
    }

    #[test]
    fn light_tokens_exact() {
        let p = Palette::new(false, &no_overrides());
        assert_eq!(p.surface, Rgb::hex(0xF9F9F9));
        assert_eq!(p.border, Rgba::new(Rgb(0, 0, 0), 41));
        assert_eq!(p.text, Rgb::hex(0x1B1B1B));
        assert_eq!(p.text_secondary, Rgb::hex(0x575757));
        assert_eq!(p.muted, Rgb::hex(0x6B6B6B));
        assert_eq!(p.track, Rgba::new(Rgb(0, 0, 0), 36));
        assert_eq!(p.hover, Rgba::new(Rgb(0, 0, 0), 15));
        assert_eq!(p.warning, Rgb::hex(0xC77800));
        assert_eq!(p.warning_text, Rgb::hex(0x8A5100));
        assert_eq!(p.dire, Rgb::hex(0xC42B1C));
        assert_eq!(p.dire_accent, Rgb::hex(0xE5372B));
    }

    #[test]
    fn dark_tokens_exact() {
        let p = Palette::new(true, &no_overrides());
        assert_eq!(p.surface, Rgb::hex(0x2B2B2B));
        assert_eq!(p.border, Rgba::new(Rgb(255, 255, 255), 41));
        assert_eq!(p.text, Rgb::hex(0xFFFFFF));
        assert_eq!(p.text_secondary, Rgb::hex(0xC8C8C8));
        assert_eq!(p.muted, Rgb::hex(0xA0A0A0));
        assert_eq!(p.track, Rgba::new(Rgb(255, 255, 255), 46));
        assert_eq!(p.hover, Rgba::new(Rgb(255, 255, 255), 20));
        assert_eq!(p.warning, Rgb::hex(0xF5B83D));
        assert_eq!(p.warning_text, Rgb::hex(0xF5B83D));
        assert_eq!(p.dire, Rgb::hex(0xC42B1C));
        assert_eq!(p.dire_accent, Rgb::hex(0xE5372B));
    }

    #[test]
    fn handoff_percentages_round_to_these_alphas() {
        let a = |pct: u32| (pct * 255 + 50) / 100;
        assert_eq!(a(35), u32::from(DIRE_TRACK_ALPHA));
        assert_eq!(a(55), u32::from(GRIP_ALPHA));
    }

    #[test]
    fn override_replaces_token() {
        let mut ov = no_overrides();
        ov.dark.warning = Some("#123456".into());
        ov.dark.surface = Some("#abcdef".into());
        ov.light.text = Some("#010203".into());
        let dark = Palette::new(true, &ov);
        assert_eq!(dark.warning, Rgb(0x12, 0x34, 0x56));
        assert_eq!(dark.surface, Rgb(0xAB, 0xCD, 0xEF));
        assert_eq!(
            dark.text,
            Rgb::hex(0xFFFFFF),
            "light override must not leak"
        );
        let light = Palette::new(false, &ov);
        assert_eq!(light.text, Rgb(1, 2, 3));
        assert_eq!(light.warning, Rgb::hex(0xC77800));
    }

    #[test]
    fn invalid_override_is_ignored() {
        let mut ov = no_overrides();
        ov.light.dire = Some("red".into());
        assert_eq!(Palette::new(false, &ov).dire, Rgb::hex(0xC42B1C));
    }

    #[test]
    fn flat_composites_over_background() {
        let p = Palette::new(false, &no_overrides());
        let white = Rgb::WHITE;
        assert_eq!(p.flat(Rgba::new(Rgb::BLACK, 0), white), white);
        assert_eq!(p.flat(Rgba::new(Rgb::BLACK, 255), white), Rgb::BLACK);
        // 16 % black over white: 255 * 214 / 255 = 214.
        assert_eq!(p.flat(p.border, white), Rgb(214, 214, 214));
    }
}
