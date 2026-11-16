//! The card's side of the opacity and settings popups: opening and
//! closing them on `Effect::ShowPopup`, placing them under the header
//! (clamped to the work area), slider and settings input, and — every
//! animation tick while one is open — hover and the Esc / outside-click
//! dismissal poll.
//!
//! Popup windows are created, shown, captured and destroyed through
//! deferred [`PopupOp`]s; their mouse messages reach the state through
//! [`popup_mouse`] → `with_state`, exactly like the card's own.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::SetCapture;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GetCursorPos, HWND_TOPMOST, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetWindowPos, ShowWindow,
};

use super::{AppState, ScaleKey, WinOp, effects, placement, with_state};
use crate::config::FontSpec;
use crate::controller::{Command, Effect, Input, Popup};
use crate::layout::{self, CardLayout, HeaderButton, Metrics, Rect};
use crate::paint::{Fonts, GdiMeasure, Painter};
use crate::palette::Palette;
use crate::popup_wnd::render::{self, PopupScene};
use crate::popup_wnd::{self, DismissPoll, PopupGeom, PopupHit, PopupMouse};
use crate::warn_on_err;

/// Popup window calls deferred until the state borrow ends.
#[derive(Debug, PartialEq)]
pub(super) enum PopupOp {
    /// Create the window, then attach it to the state.
    Create(Popup),
    /// `SW_SHOWNOACTIVATE` + `HWND_TOPMOST`.
    Show(HWND),
    /// Slider drag.
    Capture(HWND),
    Destroy(HWND),
}

/// What a popup was last drawn from; an equal key skips the redraw.
#[derive(PartialEq)]
struct Drawn {
    scene: PopupScene,
    rect: Rect,
    palette: Palette,
    scale: ScaleKey,
}

/// The open popup.
pub(super) struct PopupWin {
    hwnd: HWND,
    kind: Popup,
    /// Screen rectangle.
    rect: Rect,
    /// Screen rectangle of the header button that opened it.
    opener: Rect,
    geom: Option<PopupGeom>,
    painter: Painter,
    /// Slider value (percent): previewed live, committed on close.
    value: u8,
    hover: Option<PopupHit>,
    press: Option<PopupHit>,
    dragging: bool,
    poll: DismissPoll,
    drawn: Option<Drawn>,
}

/// "Segoe UI, 9 pt" for the settings popup's font line.
fn font_label(spec: Option<&FontSpec>) -> String {
    let fallback = FontSpec::default();
    let f = spec.unwrap_or(&fallback);
    format!("{}, {} pt", f.face, f.point_size)
}

/// A header-button click while a popup is open: the same button closes
/// its popup (the opacity one committing the value); switching from the
/// opacity popup to settings commits first.
fn toggle_commands(cmd: Command, open: Option<Popup>, value: u8) -> Vec<Command> {
    match (&cmd, open) {
        (Command::OpenOpacityPopup, Some(Popup::Opacity)) => vec![Command::CommitOpacity(value)],
        (Command::OpenSettingsPopup, Some(Popup::Settings)) => vec![Command::ClosePopups],
        (Command::OpenSettingsPopup, Some(Popup::Opacity)) => {
            vec![Command::CommitOpacity(value), cmd]
        }
        _ => vec![cmd],
    }
}

/// [`toggle_commands`] against the current state.
pub(super) fn header_commands(st: &AppState, cmd: Command) -> Vec<Command> {
    let value = st
        .popup
        .as_ref()
        .map_or(st.ctl.config().idle_opacity_pct, |p| p.value);
    toggle_commands(cmd, st.ctl.popup(), value)
}

/// `Effect::ShowPopup`: close whatever is open unless it is the wanted
/// one, then create the wanted one (deferred).
pub(super) fn show(st: &mut AppState, want: Option<Popup>) {
    if want.is_some() && st.popup.as_ref().map(|p| p.kind) == want {
        return;
    }
    if let Some(p) = st.popup.take() {
        if p.dragging {
            st.deferred.push(WinOp::Release);
        }
        st.deferred.push(WinOp::Popup(PopupOp::Destroy(p.hwnd)));
    }
    if let Some(kind) = want {
        st.deferred.push(WinOp::Popup(PopupOp::Create(kind)));
    }
}

/// Executes a deferred [`PopupOp`]; the state is not borrowed here.
pub(super) fn run(card: HWND, op: PopupOp) {
    match op {
        PopupOp::Create(kind) => create(card, kind),
        // SAFETY: a live popup window of ours; no pointers.
        PopupOp::Show(h) => unsafe {
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            let flags = SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE;
            warn_on_err!(SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, flags));
        },
        // SAFETY: as above.
        PopupOp::Capture(h) => unsafe {
            SetCapture(h);
        },
        // SAFETY: a popup window of ours, already detached from the state.
        PopupOp::Destroy(h) => unsafe { warn_on_err!(DestroyWindow(h)) },
    }
}

fn create(card: HWND, kind: Popup) {
    match popup_wnd::create(card) {
        Ok(h) => {
            if !with_state(card, |st| attach(card, st, h, kind)) {
                // SAFETY: our new popup; the state never saw it.
                unsafe { warn_on_err!(DestroyWindow(h)) };
            }
        }
        Err(e) => {
            log::warn!("popup: cannot create the window: {e}");
            with_state(card, |st| abandon(card, st));
        }
    }
}

/// The window could not be made: let the controller forget the popup.
fn abandon(card: HWND, st: &mut AppState) {
    effects::send(card, st, Input::Cmd(Command::ClosePopups));
}

/// A freshly created popup window joins the state — unless the
/// controller moved on while it was being created.
fn attach(card: HWND, st: &mut AppState, hwnd: HWND, kind: Popup) {
    if st.ctl.popup() != Some(kind) || st.popup.is_some() {
        st.deferred.push(WinOp::Popup(PopupOp::Destroy(hwnd)));
        return;
    }
    let painter = match Painter::new() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("popup: no painter: {e}");
            st.deferred.push(WinOp::Popup(PopupOp::Destroy(hwnd)));
            return abandon(card, st);
        }
    };
    let mut poll = DismissPoll::default();
    // Prime: the click that opened the popup is not a dismissal.
    poll.edges(popup_wnd::sample_keys());
    st.popup = Some(PopupWin {
        hwnd,
        kind,
        rect: Rect::default(),
        opener: Rect::default(),
        geom: None,
        painter,
        value: st.ctl.config().idle_opacity_pct,
        hover: None,
        press: None,
        dragging: false,
        poll,
        drawn: None,
    });
    refresh(st);
    st.deferred.push(WinOp::Popup(PopupOp::Show(hwnd)));
}

/// The popup's inner geometry for the card's metrics and fonts (the
/// settings popup measures its chips and button labels).
fn geometry(kind: Popup, l: &CardLayout, m: &Metrics, fonts: &Fonts) -> Option<PopupGeom> {
    Some(match kind {
        Popup::Opacity => {
            let r = layout::opacity_popup_rect(l, m);
            PopupGeom::Opacity(popup_wnd::opacity_geometry(r.w, r.h, m))
        }
        Popup::Settings => {
            let width = layout::settings_popup_rect(l, m, 0).w;
            let tm = GdiMeasure::new(fonts).ok()?;
            PopupGeom::Settings(popup_wnd::settings_geometry(width, m, &tm))
        }
    })
}

/// Where the popup sits on the card (window coordinates) and the header
/// button that opened it.
fn anchor(kind: Popup, l: &CardLayout, m: &Metrics, g: &PopupGeom) -> (Rect, HeaderButton) {
    match kind {
        Popup::Opacity => (layout::opacity_popup_rect(l, m), HeaderButton::Opacity),
        Popup::Settings => (
            layout::settings_popup_rect(l, m, g.size().1),
            HeaderButton::Settings,
        ),
    }
}

/// Lays the open popup out against the card's current layout, places it
/// (screen, clamped to the work area) and redraws it if anything it shows
/// changed. Called after every card repaint and popup input; the geometry
/// is only rebuilt when the card's scale or font changed.
pub(super) fn refresh(st: &mut AppState) {
    let (Some(l), Some((cx, cy)), Some(w)) = (st.layout.as_ref(), st.card_pos, st.popup.as_mut())
    else {
        return;
    };
    let m = &st.metrics;
    let same_scale = w.drawn.as_ref().is_some_and(|d| d.scale == st.scale_key);
    let cached = w.geom.clone().filter(|_| same_scale);
    let Some(geom) = cached.or_else(|| geometry(w.kind, l, m, &st.fonts)) else {
        return;
    };
    let (local, button) = anchor(w.kind, l, m, &geom);
    let (ox, oy) = (cx - l.card.x, cy - l.card.y);
    let (x, y) = placement::clamp_card(local.x + ox, local.y + oy, local.w, local.h);
    w.rect = Rect::new(x, y, local.w, local.h);
    let b = l.buttons[button as usize].1;
    w.opener = Rect::new(b.x + ox, b.y + oy, b.w, b.h);
    let cfg = st.ctl.config();
    let drawn = Drawn {
        scene: PopupScene {
            opacity: w.value,
            snooze: cfg.snooze,
            font: font_label(cfg.font.as_ref()),
            hover: w.hover,
        },
        rect: w.rect,
        palette: st.palette,
        scale: st.scale_key.clone(),
    };
    let stale = w.drawn.as_ref() != Some(&drawn);
    if stale {
        let r = render::draw(
            &mut w.painter,
            &geom,
            &drawn.scene,
            &st.palette,
            &st.fonts,
            m,
        );
        match r {
            Ok(()) => warn_on_err!(w.painter.present(w.hwnd, x, y, 255)),
            Err(e) => log::warn!("popup: draw failed: {e}"),
        }
        w.drawn = Some(drawn);
    }
    w.geom = Some(geom);
}

/// Opacity slider at popup-local `x`: the new value, if it moved.
fn slide(w: &mut PopupWin, x: i32) -> Option<Command> {
    let Some(PopupGeom::Opacity(g)) = &w.geom else {
        return None;
    };
    let v = popup_wnd::slider_value(x, g.track);
    (v != w.value).then(|| {
        w.value = v;
        Command::SetOpacityPreview(v)
    })
}

/// Mouse input on popup `popup` (the popup wndproc's entry point).
pub(crate) fn popup_mouse(card: HWND, popup: HWND, ev: PopupMouse) {
    with_state(card, |st| on_mouse(card, st, popup, ev));
}

fn on_mouse(card: HWND, st: &mut AppState, popup: HWND, ev: PopupMouse) {
    let Some(w) = st.popup.as_mut().filter(|w| w.hwnd == popup) else {
        return;
    };
    let hit = |w: &PopupWin, x, y| w.geom.as_ref().and_then(|g| g.hit(x, y));
    let mut cmds = Vec::new();
    match ev {
        PopupMouse::Down(x, y) => {
            w.press = hit(w, x, y);
            if w.press == Some(PopupHit::Slider) {
                w.dragging = true;
                st.deferred.push(WinOp::Popup(PopupOp::Capture(popup)));
                cmds.extend(slide(w, x));
            }
        }
        PopupMouse::Move(x, _) if w.dragging => cmds.extend(slide(w, x)),
        PopupMouse::Move(x, y) => w.hover = hit(w, x, y),
        PopupMouse::Up(x, y) => {
            let over = hit(w, x, y);
            if std::mem::take(&mut w.dragging) {
                st.deferred.push(WinOp::Release);
            } else if let Some(h) = w.press.take().filter(|h| Some(*h) == over) {
                cmds = popup_wnd::settings_commands(h);
            }
        }
        PopupMouse::CaptureLost => w.dragging = false,
    }
    let mut fx = Vec::new();
    for c in cmds {
        fx.extend(effects::feed(st, Input::Cmd(c)));
    }
    effects::apply_effects(card, st, fx);
    refresh(st);
}

fn cursor() -> Option<POINT> {
    let mut p = POINT::default();
    // SAFETY: `p` is a valid out-pointer.
    unsafe { GetCursorPos(&mut p) }.ok().map(|()| p)
}

/// The cursor is over the open popup (counts as hovering the card, so the
/// card stays up while the popup is used).
pub(super) fn cursor_inside(st: &AppState) -> bool {
    match (st.popup.as_ref(), cursor()) {
        (Some(w), Some(p)) => w.rect.contains(p.x, p.y),
        _ => false,
    }
}

/// Animation tick while a popup is open: hover from the cursor, then the
/// dismissal poll (Esc, or a press outside popup and opener; also when
/// the card has faded away). Returns the controller's effects.
pub(super) fn poll(st: &mut AppState) -> Vec<Effect> {
    let Some(w) = st.popup.as_mut() else {
        return Vec::new();
    };
    let at = cursor().map_or((i32::MIN, i32::MIN), |p| (p.x, p.y));
    if !w.dragging {
        let local = (at.0 - w.rect.x, at.1 - w.rect.y);
        let inside = w.rect.contains(at.0, at.1);
        w.hover = w
            .geom
            .as_ref()
            .filter(|_| inside)
            .and_then(|g| g.hit(local.0, local.1));
    }
    let edges = w.poll.edges(popup_wnd::sample_keys());
    let gone = !st.ctl.visibility().is_visible();
    if gone || popup_wnd::dismisses(edges, at, w.rect, w.opener) {
        let cmd = popup_wnd::dismiss_command(w.kind, w.value);
        return effects::feed(st, Input::Cmd(cmd));
    }
    refresh(st);
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_button_closes_its_popup() {
        let open = Some(Popup::Opacity);
        assert_eq!(
            toggle_commands(Command::OpenOpacityPopup, open, 65),
            [Command::CommitOpacity(65)]
        );
        assert_eq!(
            toggle_commands(Command::OpenSettingsPopup, Some(Popup::Settings), 50),
            [Command::ClosePopups]
        );
    }

    #[test]
    fn switching_from_opacity_commits_first() {
        assert_eq!(
            toggle_commands(Command::OpenSettingsPopup, Some(Popup::Opacity), 70),
            [Command::CommitOpacity(70), Command::OpenSettingsPopup]
        );
    }

    #[test]
    fn other_clicks_pass_through() {
        for open in [None, Some(Popup::Settings)] {
            assert_eq!(
                toggle_commands(Command::OpenOpacityPopup, open, 50),
                [Command::OpenOpacityPopup]
            );
        }
        assert_eq!(
            toggle_commands(Command::ToggleMode, Some(Popup::Opacity), 50),
            [Command::ToggleMode]
        );
    }

    #[test]
    fn font_line_shows_face_and_size() {
        assert_eq!(font_label(None), "Segoe UI, 9 pt");
        let f = FontSpec {
            face: "Consolas".into(),
            point_size: 11,
            weight: 400,
        };
        assert_eq!(font_label(Some(&f)), "Consolas, 11 pt");
    }
}
