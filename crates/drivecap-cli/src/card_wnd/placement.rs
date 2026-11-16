//! Where the card sits: monitor work areas (`EnumDisplayMonitors`,
//! `MonitorFromRect`), first placement from `position.json`, resize
//! anchoring on `Relayout`, and the position file itself.

use std::panic::AssertUnwindSafe;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MONITORINFOEXW, MonitorFromRect,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows_core::BOOL;

use super::{AppState, compute_layout, refresh_resources};
use crate::layout::{CardLayout, Metrics, Rect};
use crate::paths;
use crate::position::{PositionStore, WorkArea, anchor_resize, clamp_to_work_area};

/// Default inset from the primary work area's bottom-right (logical px).
const DEFAULT_INSET: i32 = 12;

/// New card top-left after a resize to `new_w × new_h`: the edge nearer
/// the work area's middle stays put ([`anchor_resize`]), x is kept, and
/// the result is clamped into `work`.
pub(super) fn reanchor(old_card: Rect, new_w: i32, new_h: i32, work: Rect) -> (i32, i32) {
    let y = anchor_resize(old_card, new_h, work);
    clamp_to_work_area(old_card.x, y, new_w, new_h, work)
}

/// `Relayout`: refreshes fonts/metrics/palette, lays the view out afresh and
/// moves the card — first placement from the store, else re-anchored
/// against its monitor's work area. The caller repaints.
pub(super) fn relayout(hwnd: HWND, st: &mut AppState) {
    refresh_resources(hwnd, st);
    let view = st.ctl.view(st.clock.now_unix_ms());
    let Some(new) = compute_layout(st, &view) else {
        return;
    };
    let pos = match (st.card_pos, st.layout.as_ref()) {
        (Some((x, y)), Some(old)) => {
            let old_card = Rect::new(x, y, old.card.w, old.card.h);
            match work_area_for(old_card) {
                Some(work) => reanchor(old_card, new.card.w, new.card.h, work),
                None => (x, y),
            }
        }
        _ => initial_pos(&st.positions, &st.metrics, &new),
    };
    st.card_pos = Some(pos);
    st.layout = Some(new);
}

/// First placement: the saved per-monitor position, else bottom-right of
/// the primary work area, 12 px in.
pub(super) fn initial_pos(store: &PositionStore, m: &Metrics, l: &CardLayout) -> (i32, i32) {
    let areas = work_areas();
    let pos = store.resolve(l.card.w, l.card.h, &areas, m.px(DEFAULT_INSET));
    log::info!("card placed at {pos:?} ({} monitors)", areas.len());
    pos
}

/// Clamps a `w × h` card at `(x, y)` into the work area of the monitor it
/// overlaps most (nearest if none).
pub(super) fn clamp_card(x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
    match work_area_for(Rect::new(x, y, w, h)) {
        Some(work) => clamp_to_work_area(x, y, w, h, work),
        None => (x, y),
    }
}

fn to_rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top)
}

fn monitor_info(mon: HMONITOR) -> Option<MONITORINFOEXW> {
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `info` is a MONITORINFOEXW with cbSize set to match.
    let ok = unsafe { GetMonitorInfoW(mon, &mut info.monitorInfo) };
    ok.as_bool().then_some(info)
}

/// Work area of the monitor `card` overlaps most, nearest if none.
fn work_area_for(card: Rect) -> Option<Rect> {
    let r = RECT {
        left: card.x,
        top: card.y,
        right: card.right(),
        bottom: card.bottom(),
    };
    // SAFETY: `r` outlives the call.
    let mon = unsafe { MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST) };
    monitor_info(mon).map(|i| to_rect(i.monitorInfo.rcWork))
}

/// Every attached monitor's work area.
pub(super) fn work_areas() -> Vec<WorkArea> {
    let mut out: Vec<WorkArea> = Vec::new();
    // SAFETY: the callback only runs during this call; `out` outlives it.
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(enum_monitor),
            LPARAM(&mut out as *mut Vec<WorkArea> as isize),
        )
    };
    out
}

unsafe extern "system" fn enum_monitor(mon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let r = std::panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `data` is the `&mut Vec<WorkArea>` passed by `work_areas`.
        let out = unsafe { &mut *(data.0 as *mut Vec<WorkArea>) };
        if let Some(info) = monitor_info(mon) {
            let dev = &info.szDevice;
            let len = dev.iter().position(|&c| c == 0).unwrap_or(dev.len());
            out.push(WorkArea {
                monitor: String::from_utf16_lossy(&dev[..len]),
                rect: to_rect(info.monitorInfo.rcWork),
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
    }));
    if r.is_err() {
        log::error!("card: panic while enumerating monitors");
    }
    BOOL(1)
}

/// Reads `position.json`; missing or malformed → an empty store (logged).
pub(super) fn load_positions() -> PositionStore {
    let path = paths::position_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => PositionStore::from_json_str(&text).unwrap_or_else(|e| {
            log::warn!(
                "position: {} is malformed ({e}); using defaults",
                path.display()
            );
            PositionStore::default()
        }),
        Err(_) => PositionStore::default(),
    }
}

/// Writes `position.json` (failures logged).
pub(super) fn save_positions(store: &PositionStore) {
    let path = paths::position_path();
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, store.to_json_string()));
    if let Err(e) = written {
        log::warn!("position: cannot save {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect::new(0, 0, 1920, 1040);

    #[test]
    fn lower_half_card_keeps_its_bottom_edge() {
        let old = Rect::new(1600, 800, 270, 200);
        assert_eq!(reanchor(old, 270, 100, WORK), (1600, 900));
    }

    #[test]
    fn upper_half_card_keeps_its_top_edge() {
        let old = Rect::new(100, 50, 270, 200);
        assert_eq!(reanchor(old, 270, 400, WORK), (100, 50));
    }

    #[test]
    fn wider_card_is_clamped_into_the_work_area() {
        let old = Rect::new(1640, 50, 270, 200);
        assert_eq!(reanchor(old, 400, 200, WORK), (1520, 50));
    }
}
