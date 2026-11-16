//! The opacity and settings popups (handoff §Opacity popup, §Settings
//! popup): their geometry, hit testing, slider mapping and dismissal
//! rules (pure), and the shared `DrivecapPopup` window class.
//!
//! A popup is its own layered, never-activating, topmost window drawn at
//! alpha 255 — it never inherits the card's alpha — owned by the card.
//! Its `GWLP_USERDATA` holds the card's `HWND`; mouse messages go to the
//! card's state through `card_wnd::popup_mouse`, so the card's
//! re-entrancy rules cover popups too. Drawing lives in [`render`].
//!
//! Popups never take focus, so Esc and outside clicks are polled by the
//! card's animation timer ([`DismissPoll`]), not received.

pub mod render;

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_ESCAPE, VK_LBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, IDC_ARROW, LoadCursorW,
    MA_NOACTIVATE, RegisterClassExW, SetWindowLongPtrW, WM_CAPTURECHANGED, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows_core::{PCWSTR, w};

use crate::config::SnoozeChoice;
use crate::controller::{Command, Popup};
use crate::error::DrivecapResult;
use crate::layout::{Metrics, Rect, TextMeasure, TextStyle};

const POPUP_CLASS: PCWSTR = w!("DrivecapPopup");
static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Slider range and step (handoff: 20–100 % in steps of 5).
const SLIDER_MIN: u8 = 20;
const SLIDER_MAX: u8 = 100;
const SLIDER_STEP: u8 = 5;

// Logical px (handoff).
const OPACITY_PAD_X: i32 = 10;
const OPACITY_LABEL_Y: i32 = 8;
const LINE_H: i32 = 16;
const OPACITY_SLIDER_Y: i32 = 28;
const SLIDER_H: i32 = 20;
/// Thumb radius; the track is inset by it so the thumb centre spans it.
pub const THUMB_R: i32 = 8;
const SETTINGS_PAD: i32 = 10;
const GROUP_GAP: i32 = 10;
const LABEL_GAP: i32 = 4;
const CHIP_PAD_X: i32 = 8;
const CHIP_H: i32 = 22;
const CHIP_GAP: i32 = 4;
const FONT_NAME_H: i32 = 18;
const CHANGE_H: i32 = 24;
const ITEM_INSET: i32 = 4;
const ITEM_H: i32 = 26;
const BOTTOM_PAD: i32 = 6;

/// Chip labels, in order.
pub const CHIPS: [(SnoozeChoice, &str); 3] = [
    (SnoozeChoice::TenMin, "10 min"),
    (SnoozeChoice::OneHour, "1 h"),
    (SnoozeChoice::NextLevel, "Next level"),
];
pub const CHANGE_LABEL: &str = "Change…";

/// What a point in a popup lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupHit {
    /// The opacity slider row (track and thumb).
    Slider,
    Chip(SnoozeChoice),
    Change,
    EditConfig,
    Quit,
}

/// Opacity popup rectangles (popup-local physical px).
#[derive(Clone, Debug, PartialEq)]
pub struct OpacityGeom {
    pub size: (i32, i32),
    /// "Idle opacity" left, the value right-aligned, same line.
    pub label: Rect,
    /// Whole slider row (hit area).
    pub slider: Rect,
    /// Span of the thumb centre: `slider` inset by the thumb radius.
    pub track: Rect,
}

/// Settings popup rectangles (popup-local physical px).
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsGeom {
    pub size: (i32, i32),
    pub snooze_label: Rect,
    pub chips: [(SnoozeChoice, Rect); 3],
    pub font_label: Rect,
    pub font_name: Rect,
    pub change: Rect,
    pub divider: Rect,
    pub edit: Rect,
    pub quit: Rect,
}

/// Either popup's geometry.
#[derive(Clone, Debug, PartialEq)]
pub enum PopupGeom {
    Opacity(OpacityGeom),
    Settings(SettingsGeom),
}

impl PopupGeom {
    pub fn size(&self) -> (i32, i32) {
        match self {
            PopupGeom::Opacity(g) => g.size,
            PopupGeom::Settings(g) => g.size,
        }
    }

    /// The target under popup-local `(x, y)`.
    pub fn hit(&self, x: i32, y: i32) -> Option<PopupHit> {
        let targets: Vec<(PopupHit, Rect)> = match self {
            PopupGeom::Opacity(g) => vec![(PopupHit::Slider, g.slider)],
            PopupGeom::Settings(g) => g
                .chips
                .iter()
                .map(|(c, r)| (PopupHit::Chip(*c), *r))
                .chain([
                    (PopupHit::Change, g.change),
                    (PopupHit::EditConfig, g.edit),
                    (PopupHit::Quit, g.quit),
                ])
                .collect(),
        };
        targets
            .into_iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(h, _)| h)
    }
}

/// Opacity popup laid out in `w × h` (from `layout::opacity_popup_rect`).
pub fn opacity_geometry(w: i32, h: i32, m: &Metrics) -> OpacityGeom {
    let pad = m.px(OPACITY_PAD_X);
    let label = Rect::new(pad, m.px(OPACITY_LABEL_Y), w - 2 * pad, m.px(LINE_H));
    let slider = Rect::new(pad, m.px(OPACITY_SLIDER_Y), w - 2 * pad, m.px(SLIDER_H));
    let r = m.px(THUMB_R);
    let track = Rect::new(slider.x + r, slider.y, slider.w - 2 * r, slider.h);
    OpacityGeom {
        size: (w, h),
        label,
        slider,
        track,
    }
}

/// Settings popup laid out `w` wide; its height follows the content
/// (handoff: padding 10, 10 px between groups).
pub fn settings_geometry(w: i32, m: &Metrics, tm: &dyn TextMeasure) -> SettingsGeom {
    let pad = m.px(SETTINGS_PAD);
    let button_w = |s: &str| tm.width(s, TextStyle::Body) + 2 * m.px(CHIP_PAD_X) + 2;
    let mut y = pad;
    let snooze_label = Rect::new(pad, y, w - 2 * pad, m.px(LINE_H));
    y += m.px(LINE_H) + m.px(LABEL_GAP);
    let mut x = pad;
    let chips = CHIPS.map(|(c, label)| {
        let r = Rect::new(x, y, button_w(label), m.px(CHIP_H));
        x = r.right() + m.px(CHIP_GAP);
        (c, r)
    });
    y += m.px(CHIP_H) + m.px(GROUP_GAP);
    let group_h = m.px(LINE_H) + m.px(FONT_NAME_H);
    let cw = button_w(CHANGE_LABEL);
    let change = Rect::new(
        w - pad - cw,
        y + (group_h - m.px(CHANGE_H)) / 2,
        cw,
        m.px(CHANGE_H),
    );
    let text_w = change.x - m.px(GROUP_GAP) - pad;
    let font_label = Rect::new(pad, y, text_w, m.px(LINE_H));
    let font_name = Rect::new(pad, y + m.px(LINE_H), text_w, m.px(FONT_NAME_H));
    y += group_h + m.px(GROUP_GAP);
    let divider = Rect::new(pad, y, w - 2 * pad, m.px(1).max(1));
    y = divider.bottom() + m.px(LABEL_GAP);
    let inset = m.px(ITEM_INSET);
    let edit = Rect::new(inset, y, w - 2 * inset, m.px(ITEM_H));
    let quit = Rect::new(inset, edit.bottom(), w - 2 * inset, m.px(ITEM_H));
    SettingsGeom {
        size: (w, quit.bottom() + m.px(BOTTOM_PAD)),
        snooze_label,
        chips,
        font_label,
        font_name,
        change,
        divider,
        edit,
        quit,
    }
}

/// PURE: slider value at popup-local `x`: 20..=100 in steps of 5 across
/// `track` (the thumb-centre span), clamped at both ends.
pub fn slider_value(x: i32, track: Rect) -> u8 {
    let frac = if track.w > 0 {
        ((x - track.x) as f32 / track.w as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let span = f32::from(SLIDER_MAX - SLIDER_MIN);
    let steps = (frac * span / f32::from(SLIDER_STEP)).round() as u8;
    SLIDER_MIN + steps * SLIDER_STEP
}

/// Thumb centre x for `value` on `track` (inverse of [`slider_value`]).
pub fn thumb_x(value: u8, track: Rect) -> i32 {
    let v = value.clamp(SLIDER_MIN, SLIDER_MAX) - SLIDER_MIN;
    let span = f32::from(SLIDER_MAX - SLIDER_MIN);
    track.x + (track.w as f32 * f32::from(v) / span).round() as i32
}

/// What a click on a settings target does. Change…, Edit and Quit close
/// the popup first; a chip leaves it open to show the new choice.
pub fn settings_commands(hit: PopupHit) -> Vec<Command> {
    match hit {
        PopupHit::Chip(c) => vec![Command::SetSnoozeChoice(c)],
        PopupHit::Change => vec![Command::ClosePopups, Command::ChangeFont],
        PopupHit::EditConfig => vec![Command::ClosePopups, Command::EditConfig],
        PopupHit::Quit => vec![Command::ClosePopups, Command::Quit],
        PopupHit::Slider => Vec::new(),
    }
}

/// Closing a popup without a click inside it: the opacity popup commits
/// the slider's value; the settings popup just closes.
pub fn dismiss_command(kind: Popup, value: u8) -> Command {
    match kind {
        Popup::Opacity => Command::CommitOpacity(value),
        Popup::Settings => Command::ClosePopups,
    }
}

/// One `GetAsyncKeyState` reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeySample {
    /// Held now.
    pub down: bool,
    /// Pressed since the previous read (catches a click between ticks,
    /// and a release + re-press while held at both reads).
    pub tapped: bool,
}

/// Esc / left / right button press edges between animation ticks.
#[derive(Clone, Debug, Default)]
pub struct DismissPoll {
    held: [bool; 3],
}

impl DismissPoll {
    /// Press edges for `[Esc, left, right]` since the last call.
    pub fn edges(&mut self, keys: [KeySample; 3]) -> [bool; 3] {
        let mut edges = [false; 3];
        for ((edge, held), k) in edges.iter_mut().zip(&mut self.held).zip(keys) {
            *edge = k.tapped || (k.down && !*held);
            *held = k.down;
        }
        edges
    }
}

/// PURE: does this tick's input dismiss the popup? Esc does; a button
/// press does unless the cursor (screen px) is over the popup or over the
/// header button that opened it (that button's own click toggles it).
pub fn dismisses(edges: [bool; 3], cursor: (i32, i32), popup: Rect, opener: Rect) -> bool {
    let [esc, left, right] = edges;
    let (x, y) = cursor;
    esc || ((left || right) && !popup.contains(x, y) && !opener.contains(x, y))
}

/// Reads Esc and both mouse buttons.
pub fn sample_keys() -> [KeySample; 3] {
    let read = |vk: VIRTUAL_KEY| {
        // SAFETY: no pointers.
        let v = unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16;
        KeySample {
            down: v & 0x8000 != 0,
            tapped: v & 1 != 0,
        }
    };
    [read(VK_ESCAPE), read(VK_LBUTTON), read(VK_RBUTTON)]
}

/// Mouse input on a popup, popup-local px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupMouse {
    Down(i32, i32),
    Move(i32, i32),
    Up(i32, i32),
    CaptureLost,
}

/// Creates a hidden popup window owned by `card` (sized and shown when
/// first presented). Runs outside any state borrow.
pub fn create(card: HWND) -> DrivecapResult<HWND> {
    // SAFETY: no pointers; this module's own handle.
    let hinstance = unsafe { GetModuleHandleW(PCWSTR::null()) }?;
    if !CLASS_REGISTERED.load(Ordering::Relaxed) {
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            // SAFETY: a stock system cursor.
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
            lpszClassName: POPUP_CLASS,
            ..Default::default()
        };
        // SAFETY: `class` is fully initialised and outlives the call.
        if unsafe { RegisterClassExW(&class) } == 0 {
            return Err(windows::core::Error::from_win32().into());
        }
        CLASS_REGISTERED.store(true, Ordering::Relaxed);
    }
    let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
    // SAFETY: registered class; the wndproc ignores the window until its
    // user data (the card) is set below.
    let hwnd = unsafe {
        CreateWindowExW(
            ex,
            POPUP_CLASS,
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            Some(card),
            None,
            Some(hinstance.into()),
            None,
        )
    }?;
    // SAFETY: our new window's user-data slot.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, card.0 as isize) };
    Ok(hwnd)
}

/// Signed client coordinates from a mouse message's `lParam`.
fn lparam_point(lp: LPARAM) -> (i32, i32) {
    let v = lp.0 as u32;
    (
        i32::from(v as u16 as i16),
        i32::from((v >> 16) as u16 as i16),
    )
}

fn mouse_event(msg: u32, lp: LPARAM) -> Option<PopupMouse> {
    let (x, y) = lparam_point(lp);
    match msg {
        WM_LBUTTONDOWN => Some(PopupMouse::Down(x, y)),
        WM_MOUSEMOVE => Some(PopupMouse::Move(x, y)),
        WM_LBUTTONUP => Some(PopupMouse::Up(x, y)),
        WM_CAPTURECHANGED => Some(PopupMouse::CaptureLost),
        _ => None,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let r = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if msg == WM_MOUSEACTIVATE {
            return Some(LRESULT(MA_NOACTIVATE as isize));
        }
        // SAFETY: reads our own window's user-data slot (the card HWND).
        let card = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
        let ev = mouse_event(msg, lp)?;
        if card != 0 {
            crate::card_wnd::popup_mouse(HWND(card as _), hwnd, ev);
        }
        Some(LRESULT(0))
    }));
    match r {
        Ok(Some(res)) => res,
        Ok(None) => {
            // SAFETY: default handling for our own window.
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        Err(_) => {
            log::error!("popup wndproc panicked on msg {msg:#06x}");
            // SAFETY: as above.
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACK: Rect = Rect::new(18, 28, 164, 20);

    #[test]
    fn slider_steps_of_5() {
        assert_eq!(slider_value(TRACK.x, TRACK), 20);
        assert_eq!(slider_value(TRACK.right(), TRACK), 100);
        // 37 % across: 20 + 0.37·80 = 49.6 → 50.
        let x = TRACK.x + (TRACK.w as f32 * 0.37).round() as i32;
        assert_eq!(slider_value(x, TRACK), 50);
        for x in TRACK.x..=TRACK.right() {
            assert_eq!(slider_value(x, TRACK) % 5, 0);
        }
    }

    #[test]
    fn slider_clamps_outside_the_track() {
        assert_eq!(slider_value(-500, TRACK), 20);
        assert_eq!(slider_value(5_000, TRACK), 100);
        assert_eq!(slider_value(10, Rect::new(10, 0, 0, 0)), 20);
    }

    #[test]
    fn thumb_position_inverts_the_slider() {
        for v in (20..=100).step_by(5) {
            assert_eq!(slider_value(thumb_x(v, TRACK), TRACK), v);
        }
    }

    struct Fixed;
    impl TextMeasure for Fixed {
        fn width(&self, text: &str, _: TextStyle) -> i32 {
            7 * text.chars().count() as i32
        }
    }

    #[test]
    fn opacity_hits_the_slider_row_only() {
        let g = PopupGeom::Opacity(opacity_geometry(200, 56, &Metrics::new(96, 100)));
        assert_eq!(g.hit(100, 38), Some(PopupHit::Slider));
        assert_eq!(g.hit(100, 14), None);
        assert_eq!(g.size(), (200, 56));
    }

    #[test]
    fn settings_targets_are_hit_and_stacked() {
        let m = Metrics::new(96, 100);
        let s = settings_geometry(230, &m, &Fixed);
        let g = PopupGeom::Settings(s.clone());
        for (c, r) in s.chips {
            assert_eq!(g.hit(r.x + 1, r.y + 1), Some(PopupHit::Chip(c)));
        }
        let at = |r: Rect| g.hit(r.x + r.w / 2, r.y + r.h / 2);
        assert_eq!(at(s.change), Some(PopupHit::Change));
        assert_eq!(at(s.edit), Some(PopupHit::EditConfig));
        assert_eq!(at(s.quit), Some(PopupHit::Quit));
        assert_eq!(at(s.font_name), None);
        // Chips left to right, 4 px apart; the change button right-aligned.
        assert_eq!(s.chips[1].1.x, s.chips[0].1.right() + 4);
        assert_eq!(s.change.right(), 230 - 10);
        assert!(s.divider.y > s.change.bottom() && s.edit.y > s.divider.y);
        assert_eq!(s.size, (230, s.quit.bottom() + 6));
    }

    #[test]
    fn settings_scale_with_dpi() {
        let one = settings_geometry(230, &Metrics::new(96, 100), &Fixed);
        let two = settings_geometry(460, &Metrics::new(192, 100), &Fixed);
        assert!(two.size.1 >= 2 * one.size.1 - 4);
    }

    #[test]
    fn settings_clicks_map_to_commands() {
        assert_eq!(
            settings_commands(PopupHit::Chip(SnoozeChoice::OneHour)),
            [Command::SetSnoozeChoice(SnoozeChoice::OneHour)]
        );
        assert_eq!(
            settings_commands(PopupHit::Change),
            [Command::ClosePopups, Command::ChangeFont]
        );
        assert_eq!(
            settings_commands(PopupHit::EditConfig),
            [Command::ClosePopups, Command::EditConfig]
        );
        assert_eq!(
            settings_commands(PopupHit::Quit),
            [Command::ClosePopups, Command::Quit]
        );
        assert!(settings_commands(PopupHit::Slider).is_empty());
    }

    #[test]
    fn dismissal_commits_opacity_or_closes_settings() {
        assert_eq!(
            dismiss_command(Popup::Opacity, 65),
            Command::CommitOpacity(65)
        );
        assert_eq!(dismiss_command(Popup::Settings, 65), Command::ClosePopups);
    }

    const UP: KeySample = KeySample {
        down: false,
        tapped: false,
    };
    const HELD: KeySample = KeySample {
        down: true,
        tapped: false,
    };
    const PRESSED: KeySample = KeySample {
        down: true,
        tapped: true,
    };
    const TAPPED: KeySample = KeySample {
        down: false,
        tapped: true,
    };

    #[test]
    fn a_held_button_is_one_edge() {
        let mut p = DismissPoll::default();
        assert_eq!(p.edges([UP, PRESSED, UP]), [false, true, false]);
        assert_eq!(p.edges([UP, HELD, UP]), [false, false, false]);
        assert_eq!(p.edges([UP, UP, UP]), [false, false, false]);
        // A click entirely between two ticks still counts.
        assert_eq!(p.edges([TAPPED, UP, TAPPED]), [true, false, true]);
    }

    #[test]
    fn release_and_repress_between_ticks_is_an_edge() {
        let mut p = DismissPoll::default();
        assert_eq!(p.edges([UP, PRESSED, UP]), [false, true, false]);
        // Down at both samples, but pressed again in between.
        assert_eq!(p.edges([UP, PRESSED, UP]), [false, true, false]);
        assert_eq!(p.edges([UP, HELD, UP]), [false, false, false]);
    }

    #[test]
    fn escape_or_outside_press_dismisses() {
        let popup = Rect::new(100, 100, 200, 56);
        let opener = Rect::new(250, 70, 22, 22);
        let outside = (10, 10);
        assert!(dismisses([true, false, false], (150, 120), popup, opener));
        assert!(dismisses([false, true, false], outside, popup, opener));
        assert!(dismisses([false, false, true], outside, popup, opener));
        assert!(!dismisses([false, true, false], (150, 120), popup, opener));
        assert!(!dismisses([false, true, true], (255, 75), popup, opener));
        assert!(!dismisses([false, false, false], outside, popup, opener));
    }
}
