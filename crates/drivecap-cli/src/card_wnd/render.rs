//! Painting the card: GDI resources that follow DPI, scale, font and
//! theme; a fresh layout per paint; draw + present at the card position;
//! and keeping `ShowWindow` in step with the controller's alpha.

use windows::Win32::Foundation::HWND;

use super::{AppState, ScaleKey, WinOp, input, placement};
use crate::controller::Popup;
use crate::layout::{self, CardLayout, HeaderButton, Metrics};
use crate::model::SurfaceView;
use crate::paint::{Fonts, GdiMeasure};
use crate::palette::Palette;
use crate::{theme, warn_on_err};

/// Dire halo / pulse period.
const HALO_PERIOD_MS: u64 = 2_400;

/// Header button drawn as active: the open popup's button, else the
/// header Zz while every at-risk volume is snoozed.
fn active_button(popup: Option<Popup>, snooze_all_active: bool) -> Option<HeaderButton> {
    match popup {
        Some(Popup::Opacity) => Some(HeaderButton::Opacity),
        Some(Popup::Settings) => Some(HeaderButton::Settings),
        None => snooze_all_active.then_some(HeaderButton::SnoozeAll),
    }
}

/// Halo phase in `[0, HALO_PERIOD_MS)`, measured from when the halo came
/// on; `None` (and the epoch reset) while it is off.
fn halo_phase(epoch: &mut Option<u64>, halo: bool, now: u64) -> Option<u64> {
    if !halo {
        *epoch = None;
        return None;
    }
    let start = *epoch.get_or_insert(now);
    Some(now.saturating_sub(start) % HALO_PERIOD_MS)
}

/// Rebuilds metrics and fonts when the DPI, scale or font changed, and the
/// palette always (theme or colour overrides may have changed).
pub(super) fn refresh_resources(hwnd: HWND, st: &mut AppState) {
    let cfg = st.ctl.config();
    st.palette = Palette::new(st.dark, &cfg.colors);
    let key = ScaleKey {
        dpi: theme::get_dpi(hwnd),
        scale_pct: cfg.scale_pct,
        font: cfg.font.clone(),
    };
    if key == st.scale_key {
        return;
    }
    let metrics = Metrics::new(key.dpi, key.scale_pct);
    match Fonts::new(key.font.as_ref(), &metrics) {
        Ok(fonts) => {
            log::info!("card: rescaled to {} dpi, {} %", key.dpi, key.scale_pct);
            st.fonts = fonts;
            st.metrics = metrics;
            st.scale_key = key;
        }
        Err(e) => log::warn!("card: cannot rebuild fonts ({e}); keeping the old ones"),
    }
}

/// A fresh layout of the current view with the current fonts.
pub(super) fn compute_layout(st: &AppState, view: &SurfaceView) -> Option<CardLayout> {
    match GdiMeasure::new(&st.fonts) {
        Ok(tm) => Some(layout::compute(view, &st.metrics, &tm)),
        Err(e) => {
            log::warn!("card: no measuring DC: {e}");
            None
        }
    }
}

/// Draws the current view and presents it at the card position with the
/// controller's alpha. Places the card first if it never was.
pub(super) fn repaint(hwnd: HWND, st: &mut AppState) {
    let now = st.clock.now_unix_ms();
    let view = st.ctl.view(now);
    let Some(layout) = compute_layout(st, &view) else {
        return;
    };
    let (cx, cy) = *st
        .card_pos
        .get_or_insert_with(|| placement::initial_pos(&st.positions, &st.metrics, &layout));
    let interactive = !st.ctl.visibility().click_through();
    st.hover = input::hover_for(st.cursor, &layout, interactive);
    let active = active_button(st.ctl.popup(), view.snooze_all_active);
    let halo = halo_phase(&mut st.halo_epoch, view.halo, now);
    let drawn = st.painter.draw(
        &layout,
        &view,
        &st.palette,
        &st.fonts,
        &st.metrics,
        st.hover,
        active,
        halo,
        st.ctl.config().dire_halo,
    );
    if let Err(e) = drawn {
        log::warn!("card: draw failed: {e}");
        return;
    }
    let alpha = st.ctl.alpha(now);
    warn_on_err!(
        st.painter
            .present(hwnd, cx - layout.card.x, cy - layout.card.y, alpha)
    );
    st.layout = Some(layout);
}

/// Matches `ShowWindow` to the controller: shown once alpha is above
/// zero, hidden once the visibility phase is `Hidden`.
pub(super) fn sync_shown(st: &mut AppState) {
    let alpha = st.ctl.alpha(st.clock.now_unix_ms());
    if alpha > 0 && !st.shown {
        st.shown = true;
        st.deferred.push(WinOp::Show);
    } else if st.shown && !st.ctl.visibility().is_visible() {
        st.shown = false;
        st.deferred.push(WinOp::Hide);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_popup_marks_its_button_active() {
        assert_eq!(
            active_button(Some(Popup::Opacity), true),
            Some(HeaderButton::Opacity)
        );
        assert_eq!(
            active_button(Some(Popup::Settings), false),
            Some(HeaderButton::Settings)
        );
    }

    #[test]
    fn all_snoozed_marks_header_zz_active() {
        assert_eq!(active_button(None, true), Some(HeaderButton::SnoozeAll));
        assert_eq!(active_button(None, false), None);
    }

    #[test]
    fn halo_phase_counts_from_when_the_halo_came_on() {
        let mut epoch = None;
        assert_eq!(halo_phase(&mut epoch, true, 10_000), Some(0));
        assert_eq!(halo_phase(&mut epoch, true, 11_000), Some(1_000));
        assert_eq!(halo_phase(&mut epoch, true, 12_500), Some(100));
    }

    #[test]
    fn halo_off_resets_the_phase() {
        let mut epoch = Some(1_000);
        assert_eq!(halo_phase(&mut epoch, false, 5_000), None);
        assert_eq!(epoch, None);
        assert_eq!(halo_phase(&mut epoch, true, 7_000), Some(0));
    }
}
