//! [`apply_effects`]: the only place controller effects become Win32 calls,
//! plus the inputs that feed it (ticks, probe results, config reloads) and
//! startup/quit.

use std::collections::VecDeque;
use std::path::Path;
use std::time::SystemTime;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, KillTimer, SW_SHOWNORMAL, SetTimer, SetWindowLongPtrW,
};
use windows_core::{HSTRING, PCWSTR, w};

use super::{
    AppState, TICK_MS, TIMER_ANIM, TIMER_TICK, WinOp, anim_interval, exstyle_for, placement,
    repaint, sync_shown,
};
use crate::config::Config;
use crate::controller::{AnimRate, Effect, Input};
use crate::{paths, volumes};

/// How often the tick checks `config.json`'s mtime.
const CONFIG_CHECK_MS: u64 = 2_000;

/// Executes `effects` in order. Effects that feed an input back
/// (`PollLocal` → `Sample`, `Rescan` → `VolumesScanned`) queue the
/// resulting effects behind the rest. `Relayout` and `Redraw` coalesce into
/// one repaint at the end; then the window's shown state follows the
/// controller. Re-entrant Win32 calls are deferred (module docs of
/// [`crate::card_wnd`]).
pub fn apply_effects(hwnd: HWND, st: &mut AppState, effects: Vec<Effect>) {
    let mut queue = VecDeque::from(effects);
    let (mut redraw, mut relayout) = (false, false);
    while let Some(effect) = queue.pop_front() {
        match effect {
            Effect::PollLocal { id, root } => {
                let free = volumes::query_free(&root);
                log::info!("poll {root}: {}", free_text(free));
                queue.extend(feed(st, Input::Sample { id, free }));
            }
            Effect::PollNetwork { id, root } => {
                log::info!("poll {root}: queued to the network probe");
                st.probe.request(id, root);
            }
            Effect::Rescan => queue.extend(rescan(st)),
            Effect::Redraw => redraw = true,
            Effect::Relayout => relayout = true,
            Effect::SetClickThrough(on) => {
                // SAFETY: our own window; style bits only.
                unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, exstyle_for(on).0 as isize) };
            }
            Effect::SetAnim(rate) => set_anim(hwnd, rate),
            Effect::UpdateTray => update_tray(st),
            Effect::SaveConfig => save_config(st),
            Effect::OpenConfig => st.deferred.push(WinOp::OpenConfig(paths::config_path())),
            Effect::ShowFontDialog => log::debug!("font dialog: not built yet (Task 14)"),
            Effect::ShowPopup(p) => log::debug!("popup {p:?}: not built yet (Task 14)"),
            Effect::Quit => quit(st),
        }
    }
    if relayout {
        placement::relayout(hwnd, st);
    }
    if relayout || redraw {
        repaint(hwnd, st);
    }
    sync_shown(st);
}

/// One input into the controller at the current time.
pub(super) fn feed(st: &mut AppState, input: Input) -> Vec<Effect> {
    let now = st.clock.now_unix_ms();
    st.ctl.handle(input, now)
}

/// [`feed`] then [`apply_effects`].
pub(super) fn send(hwnd: HWND, st: &mut AppState, input: Input) {
    let fx = feed(st, input);
    apply_effects(hwnd, st, fx);
}

/// After the window exists: the 1 s tick, the first scan and the first
/// poll round.
pub(super) fn start(hwnd: HWND, st: &mut AppState) {
    // SAFETY: our own window; WM_TIMER, no callback.
    if unsafe { SetTimer(Some(hwnd), TIMER_TICK, TICK_MS, None) } == 0 {
        log::error!("card: SetTimer(tick) failed; nothing will poll");
    }
    let mut fx = rescan(st);
    fx.extend(feed(st, Input::Tick));
    apply_effects(hwnd, st, fx);
}

/// The 1 s timer: controller tick, any probe results a dropped
/// `WM_APP_PROBE_DONE` left behind, and the config mtime check.
pub(super) fn on_tick(hwnd: HWND, st: &mut AppState) {
    let mut fx = feed(st, Input::Tick);
    fx.extend(probe_samples(st));
    fx.extend(check_config_reload(st));
    apply_effects(hwnd, st, fx);
}

/// `WM_APP_PROBE_DONE`: every finished network probe → `Sample`.
pub(super) fn drain_probe(hwnd: HWND, st: &mut AppState) {
    let fx = probe_samples(st);
    apply_effects(hwnd, st, fx);
}

fn probe_samples(st: &mut AppState) -> Vec<Effect> {
    let mut fx = Vec::new();
    for (id, free) in st.probe.drain() {
        log::info!("poll {}: {} (network)", id.0, free_text(free));
        fx.extend(feed(st, Input::Sample { id, free }));
    }
    fx
}

/// `WM_CLOSE` / `Effect::Quit`: save `position.json`, then destroy the
/// window (deferred; `WM_DESTROY` posts `WM_QUIT`).
pub(super) fn quit(st: &mut AppState) {
    log::info!("quitting");
    placement::save_positions(&st.positions);
    st.deferred.push(WinOp::Destroy);
}

/// Re-enumerates volumes, applies the config's selection rules and feeds
/// the result to the controller.
fn rescan(st: &mut AppState) -> Vec<Effect> {
    let connected = st.volumes.connected();
    let cfg = st.ctl.config();
    let extras: Vec<_> = cfg
        .extra_paths
        .iter()
        .filter_map(|p| st.volumes.resolve_path(p))
        .collect();
    let selected = volumes::select(&connected, &extras, cfg);
    log::info!(
        "rescan: watching {} of {} connected (+{} extra)",
        selected.len(),
        connected.len(),
        extras.len()
    );
    feed(
        st,
        Input::VolumesScanned {
            selected,
            connected,
        },
    )
}

fn set_anim(hwnd: HWND, rate: AnimRate) {
    match anim_interval(rate) {
        // SAFETY: our own window; re-arming an id replaces its interval.
        Some(ms) => unsafe {
            if SetTimer(Some(hwnd), TIMER_ANIM, ms, None) == 0 {
                log::warn!("card: SetTimer(anim {ms} ms) failed");
            }
        },
        // SAFETY: as above; killing an unset timer just fails.
        None => unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
        },
    }
}

/// Tray hook — Task 13 refreshes the tray icon from `ctl.tray_state`.
fn update_tray(st: &mut AppState) {
    if st.tray.is_none() {
        log::trace!("tray update: no tray yet (Task 13)");
    }
}

fn save_config(st: &mut AppState) {
    let path = paths::config_path();
    match st.ctl.config().save(&path) {
        Ok(()) => st.config_mtime = mtime(&path),
        Err(e) => log::warn!("config: save failed: {e}"),
    }
}

/// Every [`CONFIG_CHECK_MS`]: if `config.json`'s mtime moved since we last
/// saw it (our own saves update it), parse it and reload; a malformed file
/// is logged and the running config kept.
fn check_config_reload(st: &mut AppState) -> Vec<Effect> {
    let now = st.clock.now_unix_ms();
    if now.saturating_sub(st.last_config_check) < CONFIG_CHECK_MS {
        return Vec::new();
    }
    st.last_config_check = now;
    let path = paths::config_path();
    let seen = mtime(&path);
    if seen.is_none() || seen == st.config_mtime {
        return Vec::new();
    }
    st.config_mtime = seen;
    let parsed = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| Config::from_json_str(&text).map_err(|e| e.to_string()));
    match parsed {
        Ok(cfg) => {
            log::info!("config: {} changed; reloading", path.display());
            feed(st, Input::ConfigReloaded(cfg))
        }
        Err(e) => {
            log::warn!(
                "config: {} changed but is unusable ({e}); keeping the running config",
                path.display()
            );
            Vec::new()
        }
    }
}

pub(super) fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn free_text(free: Option<u64>) -> String {
    free.map_or_else(|| "unreachable".to_owned(), crate::model::format_free)
}

/// Opens `config.json` with its associated editor, else Notepad. Runs as a
/// deferred op: `ShellExecuteW` can pump messages.
pub(super) fn open_config(hwnd: HWND, path: &Path) {
    let file = HSTRING::from(path.as_os_str());
    // SAFETY: wide strings outlive the calls; results are only inspected.
    let r = unsafe {
        ShellExecuteW(
            Some(hwnd),
            w!("open"),
            &file,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if r.0 as isize > 32 {
        return;
    }
    log::info!("config: no handler for .json; opening in Notepad");
    let arg = HSTRING::from(format!("\"{}\"", path.display()));
    // SAFETY: as above.
    let r = unsafe {
        ShellExecuteW(
            Some(hwnd),
            w!("open"),
            w!("notepad.exe"),
            &arg,
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if r.0 as isize <= 32 {
        log::warn!(
            "config: could not open {} (ShellExecuteW {})",
            path.display(),
            r.0 as isize
        );
    }
}
