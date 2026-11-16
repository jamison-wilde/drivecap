//! The card window: a layered, never-activating `WS_POPUP`, its wndproc,
//! timers and the message pump. Win32 messages become
//! [`Input`]s for the pure [`Controller`]; the
//! [`Effect`]s it returns run through
//! [`apply_effects`], the only place effects become Win32 calls
//! (ADR-0001).
//!
//! Submodules, by responsibility:
//! - `effects` — [`apply_effects`]: polling, rescans, config save/reload,
//!   timers, click-through, quit.
//! - `input` — mouse: button press/click mapping, manual drag, cursor
//!   polling, the right-click hook.
//! - `placement` — monitors and work areas, first placement, resize
//!   anchoring, `position.json`.
//! - `render` — GDI resources, per-paint layout, draw + present, and
//!   `ShowWindow` following the alpha.
//! - `popups` — opacity / settings popups: open, place, input, dismissal.
//! - `shell` — the tray icon, right-click menus and the font dialog.
//!
//! ## State ownership and re-entrancy
//!
//! [`AppState`] lives in a `Box<RefCell<AppState>>` leaked into
//! `GWLP_USERDATA` at `WM_CREATE` and reclaimed at `WM_NCDESTROY`. Every
//! handler reaches it through `with_state`, which takes the only
//! `&mut AppState` via `try_borrow_mut`. Win32 calls that can synchronously
//! re-enter the wndproc *with a message we handle* — `ShowWindow`,
//! `SetWindowPos`, `SetCapture`/`ReleaseCapture` (`WM_CAPTURECHANGED`),
//! `DestroyWindow` (`WM_NCDESTROY` frees the state), `ShellExecuteW`
//! (may pump messages), `TrackPopupMenu` and `ChooseFontW` (modal loops),
//! and popup window create/show/capture/destroy — are never made while
//! the state is borrowed: handlers push a `WinOp` onto
//! `AppState::deferred`, and `with_state` runs those after the borrow
//! ends. Popup windows reach the same state through the card's `HWND`
//! (`popup_mouse` → `with_state`). Calls left inline (`UpdateLayeredWindow`,
//! `SetWindowLongPtrW(GWL_EXSTYLE)`, `SetTimer`) only re-enter with
//! messages that go straight to `DefWindowProcW`; should one ever arrive
//! for a handled message, `try_borrow_mut` fails and the message is
//! dropped (logged at debug) instead of aliasing the state.
//! `WM_DPICHANGED` (sent from inside a move) is re-posted as
//! `WM_APP_RESCALE` so the rescale always runs from the queue.

mod effects;
mod input;
mod placement;
mod popups;
mod render;
mod shell;

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DBT_DEVICEARRIVAL, DBT_DEVICEREMOVECOMPLETE, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, HWND_TOPMOST, IDC_ARROW,
    LoadCursorW, MA_NOACTIVATE, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SPI_SETWORKAREA, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WM_APP, WM_CAPTURECHANGED, WM_CLOSE, WM_CREATE, WM_DESTROY, WM_DEVICECHANGE,
    WM_DISPLAYCHANGE, WM_DPICHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_NCDESTROY, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows_core::{PCWSTR, w};

pub use effects::apply_effects;
pub(crate) use popups::popup_mouse;
use render::{compute_layout, refresh_resources, repaint, sync_shown};

use crate::clock::{Clock, SystemClock};
use crate::config::{Config, FontSpec};
use crate::controller::{AnimRate, Controller, Effect, Input};
use crate::error::{DrivecapError, DrivecapResult};
use crate::layout::{self, CardLayout, Hit, Metrics};
use crate::net_probe::{ProbeWorker, Win32FreeProbe};
use crate::paint::{Fonts, Painter};
use crate::palette::Palette;
use crate::position::PositionStore;
use crate::tray::Tray;
use crate::volumes::{VolumeSource, Win32Volumes};
use crate::{paths, theme, warn_on_err};

/// Window class of the card; [`crate::single_instance::signal_primary`]
/// finds the running instance by it.
pub const CARD_CLASS: PCWSTR = w!("DrivecapCard");
/// Posted by the probe worker after queueing a result.
pub const WM_APP_PROBE_DONE: u32 = WM_APP + 1;
/// Tray icon callback message.
pub const WM_APP_TRAY: u32 = WM_APP + 2;
/// Re-posted `WM_DPICHANGED`: rescale from the queue, never mid-move.
const WM_APP_RESCALE: u32 = WM_APP + 3;

const TIMER_TICK: usize = 1;
const TIMER_ANIM: usize = 2;
const TICK_MS: u32 = 1_000;

/// Registered `Drivecap.Show` message id (0 until [`run`] registers it).
static SHOW_MSG: AtomicU32 = AtomicU32::new(0);
/// Registered `TaskbarCreated` message id (Explorer restarted).
static TASKBAR_MSG: AtomicU32 = AtomicU32::new(0);

/// Win32 calls deferred until the state borrow ends (see module docs).
#[derive(Debug, PartialEq)]
enum WinOp {
    /// `SW_SHOWNOACTIVATE` + re-assert `HWND_TOPMOST`.
    Show,
    Hide,
    Capture,
    Release,
    OpenConfig(PathBuf),
    Shell(shell::ShellOp),
    Popup(popups::PopupOp),
    Destroy,
}

/// Where a manual drag started: cursor (screen) and card top-left.
struct DragStart {
    cursor: POINT,
    card: (i32, i32),
}

/// What the fonts and metrics were built for.
#[derive(Clone, Debug, PartialEq)]
struct ScaleKey {
    dpi: u32,
    scale_pct: u32,
    font: Option<FontSpec>,
}

/// Everything the card window owns. Lives in `GWLP_USERDATA`.
pub struct AppState {
    pub ctl: Controller,
    painter: Painter,
    fonts: Fonts,
    /// Layout of the last paint (hit testing between paints).
    layout: Option<CardLayout>,
    metrics: Metrics,
    scale_key: ScaleKey,
    palette: Palette,
    dark: bool,
    positions: PositionStore,
    probe: ProbeWorker,
    volumes: Box<dyn VolumeSource>,
    clock: Box<dyn Clock>,
    /// `config.json` mtime last seen (stamped after our own saves).
    config_mtime: Option<SystemTime>,
    last_config_check: u64,
    /// Card (not window) top-left, screen px; `None` until first placed.
    card_pos: Option<(i32, i32)>,
    /// Last polled cursor, window coordinates.
    cursor: Option<(i32, i32)>,
    hover: Option<Hit>,
    halo_epoch: Option<u64>,
    /// `ShowWindow` state as we last set it.
    shown: bool,
    /// Hit under the last `WM_LBUTTONDOWN`, until its button-up.
    press: Option<Hit>,
    drag: Option<DragStart>,
    /// Notification-area icon; `None` until created (or if it failed).
    tray: Option<Tray>,
    /// The open popup window, once created.
    popup: Option<popups::PopupWin>,
    modal: shell::ModalState,
    deferred: Vec<WinOp>,
}

impl AppState {
    /// Loads config and positions, builds the GDI resources for the
    /// window's DPI and starts the probe worker (which posts
    /// [`WM_APP_PROBE_DONE`] to `hwnd`).
    fn new(hwnd: HWND) -> DrivecapResult<AppState> {
        let clock: Box<dyn Clock> = Box::new(SystemClock::new());
        let now = clock.now_unix_ms();
        let cfg_path = paths::config_path();
        let outcome = Config::load(&cfg_path);
        let cfg = outcome.config;
        let scale_key = ScaleKey {
            dpi: theme::get_dpi(hwnd),
            scale_pct: cfg.scale_pct,
            font: cfg.font.clone(),
        };
        let metrics = Metrics::new(scale_key.dpi, scale_key.scale_pct);
        let fonts = Fonts::new(cfg.font.as_ref(), &metrics)?;
        let dark = theme::read_dark_mode();
        let palette = Palette::new(dark, &cfg.colors);
        // HWND is not Send: the worker gets the raw value.
        let raw = hwnd.0 as isize;
        let notify = Box::new(move || {
            // SAFETY: no pointers; a stale HWND only makes the post fail.
            let posted = unsafe {
                PostMessageW(
                    Some(HWND(raw as _)),
                    WM_APP_PROBE_DONE,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
            warn_on_err!(posted);
        });
        Ok(AppState {
            ctl: Controller::new(cfg, outcome.writable, now),
            painter: Painter::new()?,
            fonts,
            layout: None,
            metrics,
            scale_key,
            palette,
            dark,
            positions: placement::load_positions(),
            probe: ProbeWorker::spawn(Box::new(Win32FreeProbe::new()), notify),
            volumes: Box::new(Win32Volumes),
            clock,
            config_mtime: effects::mtime(&cfg_path),
            last_config_check: now,
            card_pos: None,
            cursor: None,
            hover: None,
            halo_epoch: None,
            shown: false,
            press: None,
            drag: None,
            tray: None,
            popup: None,
            modal: shell::ModalState::default(),
            deferred: Vec::new(),
        })
    }
}

/// Ex-style for the card: layered, tool window, never activating, topmost,
/// plus `WS_EX_TRANSPARENT` while click-through.
pub fn exstyle_for(click_through: bool) -> WINDOW_EX_STYLE {
    let base = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
    if click_through {
        base | WS_EX_TRANSPARENT
    } else {
        base
    }
}

/// Creates the card window and pumps messages until `WM_QUIT`. The UI
/// thread is a COM STA for its lifetime (`ShellExecuteW`, and Task 14's
/// `ChooseFontW`, expect one).
pub fn run() -> DrivecapResult<()> {
    // SAFETY: first COM call on this thread; balanced by CoUninitialize.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if let Err(e) = com.ok() {
        log::warn!("card: CoInitializeEx failed ({e}); continuing without COM");
    }
    let result = create_and_pump();
    if com.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above, same thread.
        unsafe { CoUninitialize() };
    }
    result
}

fn create_and_pump() -> DrivecapResult<()> {
    // SAFETY: no pointers; this module's own handle.
    let hinstance = unsafe { GetModuleHandleW(PCWSTR::null()) }?;
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance.into(),
        // SAFETY: a stock system cursor.
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
        lpszClassName: CARD_CLASS,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialised and outlives the call.
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(windows::core::Error::from_win32().into());
    }
    // SAFETY: static wide strings.
    unsafe {
        SHOW_MSG.store(
            RegisterWindowMessageW(w!("Drivecap.Show")),
            Ordering::Relaxed,
        );
        TASKBAR_MSG.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::Relaxed,
        );
    }
    // SAFETY: registered class; WM_CREATE builds the state.
    let hwnd = unsafe {
        CreateWindowExW(
            exstyle_for(true),
            CARD_CLASS,
            w!("Drivecap"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
    }?;
    // SAFETY: reads our own window's user-data slot.
    if unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } == 0 {
        // WM_CREATE did not install the state; never pump a zombie window.
        // SAFETY: our own window.
        unsafe { warn_on_err!(DestroyWindow(hwnd)) };
        return Err(DrivecapError::Config(
            "card window has no state after WM_CREATE".into(),
        ));
    }
    with_state(hwnd, |st| effects::start(hwnd, st));
    log::info!("card window up; pumping");
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a valid out-pointer for the whole loop.
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 == 0 {
            return Ok(());
        }
        if r.0 == -1 {
            return Err(windows::core::Error::from_win32().into());
        }
        // SAFETY: `msg` was filled by GetMessageW.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Runs `f` with the window's state borrowed, then the deferred Win32
/// calls with it released. `false` when there is no state, or it is
/// already borrowed (a re-entrant message — dropped).
fn with_state(hwnd: HWND, f: impl FnOnce(&mut AppState)) -> bool {
    // SAFETY: reads our own window's user-data slot.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<AppState>;
    if ptr.is_null() {
        return false;
    }
    // SAFETY: set from `Box::into_raw` in WM_CREATE and freed only in
    // WM_NCDESTROY, which needs DestroyWindow — deferred below, after the
    // last use of `cell`.
    let cell = unsafe { &*ptr };
    let ops = match cell.try_borrow_mut() {
        Ok(mut st) => {
            f(&mut st);
            std::mem::take(&mut st.deferred)
        }
        Err(_) => {
            log::debug!("card: re-entrant message dropped");
            return false;
        }
    };
    ops_to_run(ops).into_iter().for_each(|op| run_op(hwnd, op));
    true
}

/// The deferred ops to actually run: everything up to and including the
/// first `Destroy` — after it the window (and the state) are gone.
fn ops_to_run(mut ops: Vec<WinOp>) -> Vec<WinOp> {
    if let Some(i) = ops.iter().position(|op| *op == WinOp::Destroy) {
        ops.truncate(i + 1);
    }
    ops
}

/// Executes one deferred call. The state is not borrowed here.
fn run_op(hwnd: HWND, op: WinOp) {
    // SAFETY (all arms): our own live window; no pointers kept.
    match op {
        WinOp::Show => unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let flags = SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE;
            warn_on_err!(SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags));
        },
        WinOp::Hide => unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        },
        WinOp::Capture => unsafe {
            SetCapture(hwnd);
        },
        WinOp::Release => unsafe { warn_on_err!(ReleaseCapture()) },
        WinOp::OpenConfig(path) => effects::open_config(hwnd, &path),
        WinOp::Shell(op) => shell::run(hwnd, op),
        WinOp::Popup(op) => popups::run(hwnd, op),
        WinOp::Destroy => unsafe { warn_on_err!(DestroyWindow(hwnd)) },
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let r = std::panic::catch_unwind(AssertUnwindSafe(|| on_message(hwnd, msg, wp, lp)));
    r.unwrap_or_else(|p| {
        let what = p
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        log::error!("card wndproc panicked on msg {msg:#06x}: {what}");
        if msg == WM_CREATE {
            // Fail the CreateWindowExW rather than leave a stateless window.
            return LRESULT(-1);
        }
        // SAFETY: default handling for our own window.
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    })
}

/// Signed client coordinates from a mouse message's `lParam`.
fn lparam_point(lp: LPARAM) -> (i32, i32) {
    let v = lp.0 as u32;
    (
        i32::from(v as u16 as i16),
        i32::from((v >> 16) as u16 as i16),
    )
}

fn on_message(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let handled = LRESULT(0);
    match msg {
        WM_CREATE => return create_state(hwnd),
        WM_NCDESTROY => free_state(hwnd),
        WM_DESTROY => {
            // SAFETY: no pointers; ends this thread's pump.
            unsafe { PostQuitMessage(0) };
            return handled;
        }
        WM_CLOSE => {
            // SAFETY: reads our own window's user-data slot.
            let has_state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } != 0;
            if !has_state {
                // SAFETY: our own window; no state to save.
                unsafe { warn_on_err!(DestroyWindow(hwnd)) };
            } else if !with_state(hwnd, effects::quit) {
                // State busy (re-entrant close): retry from the queue
                // rather than destroy the window under a live borrow.
                // SAFETY: no pointers; posted to ourselves.
                warn_on_err!(unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) });
            }
            return handled;
        }
        WM_TIMER => {
            match wp.0 {
                TIMER_TICK => with_state(hwnd, |st| effects::on_tick(hwnd, st)),
                TIMER_ANIM => with_state(hwnd, |st| on_anim(hwnd, st)),
                _ => false,
            };
            return handled;
        }
        WM_APP_PROBE_DONE => {
            with_state(hwnd, |st| effects::drain_probe(hwnd, st));
            return handled;
        }
        WM_DPICHANGED => {
            // SAFETY: no pointers; posted to ourselves.
            warn_on_err!(unsafe { PostMessageW(Some(hwnd), WM_APP_RESCALE, WPARAM(0), LPARAM(0)) });
            return handled;
        }
        WM_APP_RESCALE | WM_DISPLAYCHANGE => {
            with_state(hwnd, |st| {
                apply_effects(hwnd, st, vec![Effect::Relayout, Effect::UpdateTray]);
            });
            return handled;
        }
        WM_SETTINGCHANGE => {
            on_setting_change(hwnd, wp, lp);
            return handled;
        }
        WM_DEVICECHANGE => {
            let ev = wp.0 as u32;
            if ev == DBT_DEVICEARRIVAL || ev == DBT_DEVICEREMOVECOMPLETE {
                with_state(hwnd, |st| apply_effects(hwnd, st, vec![Effect::Rescan]));
            }
            return LRESULT(1);
        }
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEMOVE | WM_CAPTURECHANGED | WM_RBUTTONUP => {
            on_mouse(hwnd, msg, lparam_point(lp));
            return handled;
        }
        WM_APP_TRAY => {
            shell::on_tray(hwnd, wp, lp);
            return handled;
        }
        m if m != 0 && m == TASKBAR_MSG.load(Ordering::Relaxed) => {
            with_state(hwnd, shell::readd_tray);
            return handled;
        }
        m if m != 0 && m == SHOW_MSG.load(Ordering::Relaxed) => {
            log::info!("relaunch signal received");
            with_state(hwnd, |st| effects::send(hwnd, st, Input::Relaunched));
            return handled;
        }
        _ => {}
    }
    // SAFETY: default handling for our own window.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn on_mouse(hwnd: HWND, msg: u32, (x, y): (i32, i32)) {
    with_state(hwnd, |st| match msg {
        WM_LBUTTONDOWN => input::on_button_down(hwnd, st, x, y),
        WM_LBUTTONUP => input::on_button_up(hwnd, st, x, y),
        WM_MOUSEMOVE => input::on_mouse_move(hwnd, st),
        WM_CAPTURECHANGED => input::on_capture_lost(hwnd, st),
        _ => {
            let hit = st
                .layout
                .as_ref()
                .map_or(Hit::Outside, |l| layout::hit_test(l, x, y));
            let mut pt = POINT { x, y };
            // SAFETY: our own window; `pt` is a valid in/out pointer.
            let _ = unsafe { ClientToScreen(hwnd, &mut pt) };
            input::on_context_menu(st, pt, hit);
        }
    });
}

/// Theme switch → new palette, repaint, tray; work-area change → re-clamp.
fn on_setting_change(hwnd: HWND, wp: WPARAM, lp: LPARAM) {
    let area = PCWSTR(lp.0 as *const u16);
    // SAFETY: for WM_SETTINGCHANGE a non-null lParam is a NUL-terminated string.
    let immersive =
        !area.is_null() && unsafe { area.to_string() }.is_ok_and(|s| s == "ImmersiveColorSet");
    if immersive {
        with_state(hwnd, |st| {
            st.dark = theme::read_dark_mode();
            refresh_resources(hwnd, st);
            apply_effects(hwnd, st, vec![Effect::Redraw, Effect::UpdateTray]);
        });
    } else if wp.0 as u32 == SPI_SETWORKAREA.0 {
        with_state(hwnd, |st| apply_effects(hwnd, st, vec![Effect::Relayout]));
    }
}

/// WM_CREATE: build the state and hand it to the window; `-1` fails the
/// `CreateWindowExW`.
fn create_state(hwnd: HWND) -> LRESULT {
    match AppState::new(hwnd) {
        Ok(st) => {
            let ptr = Box::into_raw(Box::new(RefCell::new(st)));
            // SAFETY: ownership moves to the window until WM_NCDESTROY.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize) };
            LRESULT(0)
        }
        Err(e) => {
            log::error!("card: cannot create state: {e}");
            LRESULT(-1)
        }
    }
}

/// WM_NCDESTROY: take the state back and drop it.
fn free_state(hwnd: HWND) {
    // SAFETY: our own window; the slot is cleared before the drop so no
    // later message can reach freed memory.
    let ptr = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut RefCell<AppState>;
    if ptr.is_null() {
        return;
    }
    // SAFETY: the pointer came from `Box::into_raw` in `create_state`.
    let cell = unsafe { Box::from_raw(ptr) };
    if cell.try_borrow_mut().is_err() {
        // Destroyed from inside a borrow (should be impossible: Destroy is
        // deferred). Leak rather than free under a live reference.
        log::error!("card: state still borrowed at WM_NCDESTROY; leaking it");
        std::mem::forget(cell);
    }
}

/// Animation timer: polls the cursor (the open popup counts as the card),
/// advances the controller, polls popup dismissal, and repaints when
/// anything visible moved (alpha, hover, the halo).
fn on_anim(hwnd: HWND, st: &mut AppState) {
    let inside = input::poll_cursor(st) | popups::cursor_inside(st);
    let mut fx = effects::feed(st, Input::Cursor { inside });
    fx.extend(effects::feed(st, Input::Tick));
    let interactive = !st.ctl.visibility().click_through();
    let hover = st
        .layout
        .as_ref()
        .and_then(|l| input::hover_for(st.cursor, l, interactive));
    if hover != st.hover || st.halo_epoch.is_some() {
        fx.push(Effect::Redraw);
    }
    fx.extend(popups::poll(st));
    apply_effects(hwnd, st, fx);
}

/// Anim timer interval for a controller rate; `None` = timer off.
fn anim_interval(rate: AnimRate) -> Option<u32> {
    match rate {
        AnimRate::Off => None,
        AnimRate::Slow50 => Some(50),
        AnimRate::Fast16 => Some(16),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_runs_after_destroy() {
        let ops = vec![WinOp::Release, WinOp::Destroy, WinOp::Show, WinOp::Capture];
        assert_eq!(ops_to_run(ops), vec![WinOp::Release, WinOp::Destroy]);
    }

    #[test]
    fn ops_without_destroy_all_run() {
        let ops = vec![WinOp::Show, WinOp::Capture];
        assert_eq!(ops_to_run(ops), vec![WinOp::Show, WinOp::Capture]);
    }

    #[test]
    fn effects_to_exstyle() {
        let base = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
        assert_eq!(exstyle_for(false), base);
        assert_eq!(exstyle_for(true), base | WS_EX_TRANSPARENT);
    }

    #[test]
    fn lparam_point_sign_extends_both_halves() {
        let lp = LPARAM(((0xFFFEu32 << 16) | 0xFFFF) as isize);
        assert_eq!(lparam_point(lp), (-1, -2));
        assert_eq!(lparam_point(LPARAM((20 << 16) | 10)), (10, 20));
    }

    #[test]
    fn anim_rates_map_to_timer_intervals() {
        assert_eq!(anim_interval(AnimRate::Off), None);
        assert_eq!(anim_interval(AnimRate::Slow50), Some(50));
        assert_eq!(anim_interval(AnimRate::Fast16), Some(16));
    }
}
