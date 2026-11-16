//! Shell integration for the card: the tray icon (created after the
//! window, refreshed on `UpdateTray` / theme / DPI change, re-added on
//! `TaskbarCreated`, callbacks routed to the controller).

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};

use super::{AppState, effects, with_state};
use crate::controller::Input;
use crate::palette::Palette;
use crate::theme;
use crate::tray::{self, Tray, TrayAction};

/// The tray glyph is drawn for the taskbar's (Windows) theme, which can
/// differ from the apps theme the card follows.
fn taskbar_palette(st: &AppState) -> Palette {
    Palette::new(theme::read_taskbar_dark_mode(), &st.ctl.config().colors)
}

/// After the card window exists: add the tray icon.
pub(super) fn create_tray(hwnd: HWND, st: &mut AppState) {
    let state = st.ctl.tray_state(st.clock.now_unix_ms());
    match Tray::add(hwnd, &state, &taskbar_palette(st), tray::taskbar_dpi()) {
        Ok(t) => st.tray = Some(t),
        Err(e) => log::error!("tray: cannot create the icon: {e}"),
    }
}

/// `Effect::UpdateTray` (also theme and DPI changes): redraws the icon
/// when its colour, mark, size or tooltip changed.
pub(super) fn update_tray(st: &mut AppState) {
    let state = st.ctl.tray_state(st.clock.now_unix_ms());
    let pal = taskbar_palette(st);
    if let Some(t) = st.tray.as_mut() {
        t.update(&state, &pal, tray::taskbar_dpi());
    }
}

/// `TaskbarCreated`: Explorer restarted — add the icon again.
pub(super) fn readd_tray(st: &mut AppState) {
    if let Some(t) = st.tray.as_mut() {
        log::info!("tray: TaskbarCreated; re-adding the icon");
        t.readd();
    }
}

/// `WM_APP_TRAY`: left click → `Input::TrayClick`; context menu → the
/// tray menu at the anchor.
pub(super) fn on_tray(hwnd: HWND, wp: WPARAM, lp: LPARAM) {
    with_state(hwnd, |st| {
        let now = st.clock.now_unix_ms();
        let Some(action) = st
            .tray
            .as_mut()
            .and_then(|t| t.on_callback(wp.0, lp.0, now))
        else {
            return;
        };
        match action {
            TrayAction::Click => effects::send(hwnd, st, Input::TrayClick),
            TrayAction::Menu(pt) => log::debug!("tray menu at {pt:?}: Task 14"),
        }
    });
}
