//! `TrackPopupMenu` wrapper (adapted from exbar): builds a Win32 popup
//! menu — submenus, checks, greyed items — from [`MenuNode`]s, runs it
//! modally and returns the picked id.
//!
//! The modal loop pumps messages: callers run [`show`] outside any
//! `AppState` borrow (a deferred op in `card_wnd`).

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, HMENU, MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED,
    MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW, SetForegroundWindow, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};
use windows_core::PCWSTR;

use crate::menu::MenuNode;

/// Menu text for `s`: `&` doubled so labels like `"R&D"` show literally
/// instead of turning the next letter into a mnemonic; NUL-terminated.
fn menu_text(s: &str) -> Vec<u16> {
    s.replace('&', "&&")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

fn flags(enabled: bool, checked: bool) -> MENU_ITEM_FLAGS {
    let mut f = MF_STRING;
    if !enabled {
        f |= MF_GRAYED;
    }
    if checked {
        f |= MF_CHECKED;
    }
    f
}

fn append(menu: HMENU, f: MENU_ITEM_FLAGS, id: usize, label: &str) -> bool {
    let text = menu_text(label);
    // SAFETY: live menu; `text` is NUL-terminated and outlives the call.
    unsafe { AppendMenuW(menu, f, id, PCWSTR(text.as_ptr())) }.is_ok()
}

/// A popup menu for `nodes`, submenus attached (destroying the root
/// destroys them). `None` if the menu cannot be created.
fn build(nodes: &[MenuNode]) -> Option<HMENU> {
    // SAFETY: no pointers; the caller destroys the menu.
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    for node in nodes {
        match node {
            MenuNode::Item {
                id,
                label,
                checked,
                enabled,
            } => {
                append(menu, flags(*enabled, *checked), *id as usize, label);
            }
            MenuNode::Sub {
                label,
                enabled,
                children,
            } => {
                let Some(sub) = build(children) else { continue };
                if !append(
                    menu,
                    flags(*enabled, false) | MF_POPUP,
                    sub.0 as usize,
                    label,
                ) {
                    // SAFETY: not attached to `menu`, so still ours to free.
                    unsafe { warn_destroy(sub) };
                }
            }
            MenuNode::Separator => {
                // SAFETY: live menu; separators take no text.
                let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
            }
        }
    }
    Some(menu)
}

/// # Safety
/// `menu` must be a live menu owned by the caller and not attached to
/// another menu.
unsafe fn warn_destroy(menu: HMENU) {
    // SAFETY: per the function contract.
    if let Err(e) = unsafe { DestroyMenu(menu) } {
        log::debug!("contextmenu: DestroyMenu failed: {e}");
    }
}

/// Shows `nodes` at screen `pt` owned by `owner` and returns the picked
/// item id, or 0 when dismissed. `SetForegroundWindow(owner)` first and a
/// `WM_NULL` after are required for the menu to dismiss on an outside
/// click — the one sanctioned foreground call in drivecap.
pub fn show(owner: HWND, pt: POINT, nodes: &[MenuNode]) -> u32 {
    let Some(menu) = build(nodes) else {
        log::warn!("contextmenu: cannot create the menu");
        return 0;
    };
    // SAFETY: `owner` is our live window; `menu` is the live menu built
    // above, destroyed (with its submenus) once the modal loop returns.
    unsafe {
        let _ = SetForegroundWindow(owner);
        let picked = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            Some(0),
            owner,
            None,
        );
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        warn_destroy(menu);
        picked.0 as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ampersands_are_literal() {
        let t = menu_text("R&D");
        assert_eq!(String::from_utf16(&t[..t.len() - 1]).unwrap(), "R&&D");
        assert_eq!(t.last(), Some(&0));
    }

    #[test]
    fn flags_grey_and_check() {
        assert_eq!(flags(true, false), MF_STRING);
        assert_eq!(flags(false, false), MF_STRING | MF_GRAYED);
        assert_eq!(flags(true, true), MF_STRING | MF_CHECKED);
    }

    #[test]
    fn builds_nested_menus_without_showing_them() {
        let nodes = crate::menu::tray_menu(
            &[(crate::config::VolumeId("c".into()), "C: X".into(), true)],
            crate::menu::SnoozeSpec::default(),
        );
        let menu = build(&nodes).expect("menu");
        // SAFETY: the root menu built above, never shown.
        unsafe { warn_destroy(menu) };
    }
}
