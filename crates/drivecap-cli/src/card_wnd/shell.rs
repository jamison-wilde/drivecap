//! Shell integration for the card: the tray icon (created after the
//! window, refreshed on `UpdateTray` / theme / DPI change, re-added on
//! `TaskbarCreated`, callbacks routed to the controller), the card and
//! tray right-click menus, and the font dialog.
//!
//! `TrackPopupMenu` and `ChooseFontW` run modal loops, so both are
//! [`ShellOp`]s run after the state borrow ends; their result comes back
//! as an `Input::Cmd` through a fresh `with_state`.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};

use super::{AppState, WinOp, effects, with_state};
use crate::config::{FontSpec, VolumeId};
use crate::controller::{Command, Input};
use crate::menu::{self, MenuNode, RowRef};
use crate::palette::Palette;
use crate::tray::{self, Tray, TrayAction};
use crate::{contextmenu, font, theme};

/// A right-click menu to show: what it shows and what its ids refer to.
#[derive(Debug, PartialEq)]
pub(super) struct MenuOp {
    pt: POINT,
    nodes: Vec<MenuNode>,
    row: Option<RowRef>,
    connected: Vec<(VolumeId, String, bool)>,
}

/// Modal shell calls deferred until the state borrow ends.
#[derive(Debug, PartialEq)]
pub(super) enum ShellOp {
    Menu(MenuOp),
    /// `ChooseFontW` seeded with the current font.
    FontDialog(Option<FontSpec>),
}

/// Queues the right-click menu at screen `pt`: the card menu over row
/// `row`, or the row-less menu (card background, tray).
pub(super) fn queue_menu(st: &mut AppState, pt: POINT, row: Option<usize>) {
    let view = st.ctl.view(st.clock.now_unix_ms());
    let row = row.and_then(|index| {
        let id = view.rows.get(index)?.id.clone();
        Some(RowRef { index, id })
    });
    let snooze = menu::snooze_spec(&view, row.as_ref().map(|r| r.index));
    let connected = st.ctl.connected_volumes();
    let nodes = match &row {
        Some(r) => menu::card_menu(Some(r), &connected, snooze),
        None => menu::tray_menu(&connected, snooze),
    };
    st.deferred.push(WinOp::Shell(ShellOp::Menu(MenuOp {
        pt,
        nodes,
        row,
        connected,
    })));
}

/// A menu command against the rows as they are now: `HideRow` follows its
/// volume if the rows moved while the menu was open, and is dropped if the
/// volume is gone.
fn current_command(cmd: Command, row: Option<&RowRef>, ids: &[&VolumeId]) -> Option<Command> {
    match (cmd, row) {
        (Command::HideRow(_), Some(r)) => {
            let i = ids.iter().position(|id| **id == r.id)?;
            Some(Command::HideRow(i))
        }
        (cmd, _) => Some(cmd),
    }
}

/// Executes a deferred [`ShellOp`]; the state is not borrowed here.
pub(super) fn run(hwnd: HWND, op: ShellOp) {
    match op {
        ShellOp::Menu(m) => {
            let id = contextmenu::show(hwnd, m.pt, &m.nodes);
            let Some(cmd) = menu::command_for(id, m.row.as_ref(), &m.connected) else {
                return;
            };
            with_state(hwnd, |st| {
                let view = st.ctl.view(st.clock.now_unix_ms());
                let ids: Vec<_> = view.rows.iter().map(|r| &r.id).collect();
                if let Some(cmd) = current_command(cmd, m.row.as_ref(), &ids) {
                    effects::send(hwnd, st, Input::Cmd(cmd));
                }
            });
        }
        ShellOp::FontDialog(current) => {
            if let Some(spec) = font::choose_font(hwnd, current.as_ref()) {
                with_state(hwnd, |st| {
                    effects::send(hwnd, st, Input::Cmd(Command::FontChosen(spec)));
                });
            }
        }
    }
}

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
            TrayAction::Menu(pt) => queue_menu(st, pt, None),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Vec<VolumeId> {
        ["c", "d", "e"].map(|s| VolumeId(s.into())).into()
    }

    #[test]
    fn hide_follows_its_volume_when_rows_moved() {
        let row = RowRef {
            index: 0,
            id: VolumeId("e".into()),
        };
        let ids = ids();
        let now: Vec<_> = ids.iter().collect();
        assert_eq!(
            current_command(Command::HideRow(0), Some(&row), &now),
            Some(Command::HideRow(2))
        );
        assert_eq!(
            current_command(Command::HideRow(0), Some(&row), &now[..2]),
            None
        );
    }

    #[test]
    fn other_menu_commands_pass_through() {
        let ids = ids();
        let now: Vec<_> = ids.iter().collect();
        assert_eq!(
            current_command(Command::Quit, None, &now),
            Some(Command::Quit)
        );
    }
}
