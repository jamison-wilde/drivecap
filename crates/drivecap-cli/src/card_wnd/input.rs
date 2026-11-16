//! Mouse input on the card: press → click mapping, the manual drag, cursor
//! polling for hover, and the right-click hook.
//!
//! The card is click-through most of the time, so hover is driven by the
//! animation timer's `GetCursorPos` poll, not `WM_MOUSEMOVE`. Button
//! messages only arrive while it is interactive. Every `WM_LBUTTONDOWN`
//! sends `Input::ButtonDown` and captures the mouse; its button-up (or a
//! capture loss) always sends the balancing `Input::DragEnd`.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use super::effects::{apply_effects, feed, send};
use super::{AppState, DragStart, WinOp, placement, popups, repaint, shell};
use crate::controller::{Command, Effect, Input};
use crate::layout::{CardLayout, HeaderButton, Hit, hit_test};
use crate::theme;

/// The command a press on `down` released on `up` performs: header
/// buttons and row Zz act on a release over the same target (a standard
/// click); everything else is not a click.
pub(super) fn click_command(down: Hit, up: Hit) -> Option<Command> {
    if down != up {
        return None;
    }
    match down {
        Hit::Button(HeaderButton::SnoozeAll) => Some(Command::SnoozeAll),
        Hit::Button(HeaderButton::Opacity) => Some(Command::OpenOpacityPopup),
        Hit::Button(HeaderButton::Settings) => Some(Command::OpenSettingsPopup),
        Hit::Button(HeaderButton::Chevron) => Some(Command::ToggleMode),
        Hit::RowSnooze(i) => Some(Command::SnoozeRow(i)),
        Hit::Grip | Hit::HeaderEmpty | Hit::Row(_) | Hit::Outside => None,
    }
}

/// A press here starts a manual drag: the grip or empty header.
pub(super) fn starts_drag(hit: Hit) -> bool {
    matches!(hit, Hit::Grip | Hit::HeaderEmpty)
}

/// What the cursor (window coordinates) hovers, shown only while the card
/// is interactive — a click-through card cannot be clicked, so it should
/// not look clickable.
pub(super) fn hover_for(
    cursor: Option<(i32, i32)>,
    layout: &CardLayout,
    interactive: bool,
) -> Option<Hit> {
    let (x, y) = cursor.filter(|_| interactive)?;
    Some(hit_test(layout, x, y)).filter(|h| *h != Hit::Outside)
}

fn cursor_pos() -> Option<POINT> {
    let mut p = POINT::default();
    // SAFETY: `p` is a valid out-pointer.
    unsafe { GetCursorPos(&mut p) }.ok().map(|()| p)
}

/// Anim-timer poll: records the cursor in window coordinates and reports
/// whether it is over the card (the halo margin does not count).
pub(super) fn poll_cursor(st: &mut AppState) -> bool {
    let (Some(p), Some((cx, cy)), Some(l)) = (cursor_pos(), st.card_pos, st.layout.as_ref()) else {
        st.cursor = None;
        return false;
    };
    let local = (p.x - (cx - l.card.x), p.y - (cy - l.card.y));
    st.cursor = Some(local);
    l.card.contains(local.0, local.1)
}

fn hit_at(st: &AppState, x: i32, y: i32) -> Hit {
    st.layout
        .as_ref()
        .map_or(Hit::Outside, |l| hit_test(l, x, y))
}

pub(super) fn on_button_down(hwnd: HWND, st: &mut AppState, x: i32, y: i32) {
    let hit = hit_at(st, x, y);
    st.press = Some(hit);
    st.drag = match (starts_drag(hit), cursor_pos(), st.card_pos) {
        (true, Some(cursor), Some(card)) => Some(DragStart { cursor, card }),
        _ => None,
    };
    st.deferred.push(WinOp::Capture);
    send(hwnd, st, Input::ButtonDown);
}

/// While dragging: move the card by the cursor's delta from the press.
pub(super) fn on_mouse_move(hwnd: HWND, st: &mut AppState) {
    let (Some(d), Some(p)) = (st.drag.as_ref(), cursor_pos()) else {
        return;
    };
    st.card_pos = Some((d.card.0 + p.x - d.cursor.x, d.card.1 + p.y - d.cursor.y));
    repaint(hwnd, st);
}

pub(super) fn on_button_up(hwnd: HWND, st: &mut AppState, x: i32, y: i32) {
    let Some(down) = st.press.take() else {
        return;
    };
    st.deferred.push(WinOp::Release);
    let mut fx = Vec::new();
    if st.drag.take().is_some() {
        fx.extend(finish_drag(hwnd, st));
    } else if let Some(cmd) = click_command(down, hit_at(st, x, y)) {
        for c in popups::header_commands(st, cmd) {
            fx.extend(feed(st, Input::Cmd(c)));
        }
    }
    fx.extend(feed(st, Input::DragEnd));
    apply_effects(hwnd, st, fx);
}

/// Capture taken away mid-press (another window grabbed it): end the press
/// without a click. Our own `ReleaseCapture` lands here after the press
/// was already taken, and is a no-op.
pub(super) fn on_capture_lost(hwnd: HWND, st: &mut AppState) {
    if st.press.take().is_none() {
        return;
    }
    let mut fx = Vec::new();
    if st.drag.take().is_some() {
        fx.extend(finish_drag(hwnd, st));
    }
    fx.extend(feed(st, Input::DragEnd));
    apply_effects(hwnd, st, fx);
}

/// Clamps the dropped card into its monitor's work area, records it
/// against that monitor and saves `position.json`. Returns the repaint —
/// a `Relayout` when the drop landed on a monitor of another DPI (the
/// `WM_DPICHANGED` rescale may have been dropped or re-clamped mid-drag).
fn finish_drag(hwnd: HWND, st: &mut AppState) -> Vec<Effect> {
    let (Some((x, y)), Some(l)) = (st.card_pos, st.layout.as_ref()) else {
        return vec![Effect::Redraw];
    };
    let (w, h) = (l.card.w, l.card.h);
    let (x, y) = placement::clamp_card(x, y, w, h);
    st.card_pos = Some((x, y));
    st.positions.record(x, y, w, h, &placement::work_areas());
    placement::save_positions(&st.positions);
    log::info!("card dragged to ({x}, {y})");
    if theme::get_dpi(hwnd) != st.scale_key.dpi {
        vec![Effect::Relayout]
    } else {
        vec![Effect::Redraw]
    }
}

/// `WM_RBUTTONUP` at screen `pt` over `hit`: queues the card menu (row
/// items when over a row). `TrackPopupMenu` runs a modal loop, so it is a
/// deferred op, never called here while `st` is borrowed.
pub(super) fn on_context_menu(st: &mut AppState, pt: POINT, hit: Hit) {
    let row = match hit {
        Hit::Row(i) | Hit::RowSnooze(i) => Some(i),
        _ => None,
    };
    shell::queue_menu(st, pt, row);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;
    use crate::layout::{Metrics, TextMeasure, TextStyle, compute};
    use crate::model::{Dot, SurfaceView};

    struct Fixed;
    impl TextMeasure for Fixed {
        fn width(&self, text: &str, _: TextStyle) -> i32 {
            text.len() as i32 * 7
        }
    }

    fn header_only() -> CardLayout {
        let view = SurfaceView {
            title: "Snoozed".into(),
            dot: Dot::Muted,
            snooze_all_active: false,
            rows: Vec::new(),
            halo: false,
            mode: Mode::Minimized,
        };
        compute(&view, &Metrics::new(96, 100), &Fixed)
    }

    #[test]
    fn hover_reports_the_target_under_an_interactive_cursor() {
        let l = header_only();
        let (_, chevron) = l.buttons[3];
        let at = Some((chevron.x + 1, chevron.y + 1));
        assert_eq!(
            hover_for(at, &l, true),
            Some(Hit::Button(HeaderButton::Chevron))
        );
    }

    #[test]
    fn click_through_card_shows_no_hover() {
        let l = header_only();
        let at = Some((l.grip.x, l.grip.y));
        assert_eq!(hover_for(at, &l, false), None);
    }

    #[test]
    fn cursor_in_the_halo_margin_hovers_nothing() {
        let l = header_only();
        assert_eq!(hover_for(Some((0, 0)), &l, true), None);
        assert_eq!(hover_for(None, &l, true), None);
    }

    #[test]
    fn header_buttons_map_to_commands() {
        let b = Hit::Button;
        let click = |h| click_command(b(h), b(h));
        assert_eq!(click(HeaderButton::SnoozeAll), Some(Command::SnoozeAll));
        assert_eq!(
            click(HeaderButton::Opacity),
            Some(Command::OpenOpacityPopup)
        );
        assert_eq!(
            click(HeaderButton::Settings),
            Some(Command::OpenSettingsPopup)
        );
        assert_eq!(click(HeaderButton::Chevron), Some(Command::ToggleMode));
    }

    #[test]
    fn row_snooze_maps_to_its_row() {
        assert_eq!(
            click_command(Hit::RowSnooze(2), Hit::RowSnooze(2)),
            Some(Command::SnoozeRow(2))
        );
    }

    #[test]
    fn release_outside_the_pressed_target_is_no_click() {
        let zz = Hit::Button(HeaderButton::SnoozeAll);
        assert_eq!(click_command(zz, Hit::Button(HeaderButton::Opacity)), None);
        assert_eq!(click_command(zz, Hit::Outside), None);
        assert_eq!(click_command(Hit::RowSnooze(1), Hit::RowSnooze(0)), None);
    }

    #[test]
    fn rows_grip_and_header_are_not_clicks() {
        for h in [Hit::Row(0), Hit::Grip, Hit::HeaderEmpty, Hit::Outside] {
            assert_eq!(click_command(h, h), None);
        }
    }

    #[test]
    fn only_grip_and_empty_header_start_a_drag() {
        assert!(starts_drag(Hit::Grip));
        assert!(starts_drag(Hit::HeaderEmpty));
        for h in [
            Hit::Row(0),
            Hit::RowSnooze(0),
            Hit::Button(HeaderButton::Chevron),
            Hit::Outside,
        ] {
            assert!(!starts_drag(h));
        }
    }
}
