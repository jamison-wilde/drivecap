//! Card visibility state machine: fade in / hold / fade out, hover rest,
//! linger, pinning (dire) and click-through. Pure — the card window polls
//! the cursor, forwards mouse events and calls [`Visibility::tick`] on its
//! animation timer, then applies [`Visibility::alpha`] and
//! [`Visibility::click_through`].
//!
//! ## Alpha model
//!
//! Every alpha the card can be at is a linear mix of three levels — zero,
//! the idle opacity, and full — so a ramp stores its endpoints as weights
//! of those levels rather than as bytes. That lets the idle opacity (or the
//! opacity popup's live preview) change mid-ramp without a jump, and lets
//! `alpha()` stay `&self`. Ramps are linear. A ramp captures its duration
//! when it starts; the timings passed to `alpha`/`needs_fast_anim` are
//! accepted for call-site symmetry only.

use crate::config::Timings;

/// Why the surface is being shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowReason {
    /// A level crossing or dire entry (long hold).
    Crossing,
    /// Periodic warning-zone reminder.
    Reminder,
    /// A network folder became unreachable.
    Unreachable,
    /// Tray left-click: full opacity for the whole showing.
    Tray,
}

/// Visibility phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Hidden,
    FadingIn,
    Holding,
    FadingOut,
    Lingering,
}

/// Weights of the idle and full levels; the remainder is zero alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Mix {
    idle: f32,
    full: f32,
}

impl Mix {
    const ZERO: Mix = Mix {
        idle: 0.0,
        full: 0.0,
    };
    const IDLE: Mix = Mix {
        idle: 1.0,
        full: 0.0,
    };
    const FULL: Mix = Mix {
        idle: 0.0,
        full: 1.0,
    };

    fn lerp(self, to: Mix, f: f32) -> Mix {
        Mix {
            idle: self.idle + (to.idle - self.idle) * f,
            full: self.full + (to.full - self.full) * f,
        }
    }

    fn alpha(self, idle_alpha: u8) -> u8 {
        let a = self.idle * f32::from(idle_alpha) + self.full * 255.0;
        a.round().clamp(0.0, 255.0) as u8
    }
}

/// A linear ramp between two mixes.
#[derive(Clone, Copy, Debug)]
struct Ramp {
    from: Mix,
    to: Mix,
    start: u64,
    dur: u64,
}

impl Ramp {
    const AT_ZERO: Ramp = Ramp {
        from: Mix::ZERO,
        to: Mix::ZERO,
        start: 0,
        dur: 0,
    };

    fn end(&self) -> u64 {
        self.start.saturating_add(self.dur)
    }

    fn at(&self, now: u64) -> Mix {
        if now >= self.end() {
            self.to
        } else if now <= self.start {
            self.from
        } else {
            let f = (now - self.start) as f32 / self.dur as f32;
            self.from.lerp(self.to, f)
        }
    }
}

/// Card visibility, hover and click-through state.
#[derive(Clone, Debug)]
pub struct Visibility {
    phase: Phase,
    ramp: Ramp,
    fade_in_until: u64,
    hold_until: u64,
    linger_until: u64,
    hovered: bool,
    hover_since: Option<u64>,
    interactive: bool,
    pinned: bool,
    dragging: bool,
    tray_forced: bool,
    preview_opacity: Option<u8>,
    /// Set by [`Visibility::hide`]: ignore hover until the cursor leaves
    /// (or the next show), so a cursor resting on the card can't undo it.
    needs_leave: bool,
    /// Durations from the latest [`Timings`] seen, for the calls that take none.
    fade_in_ms: u64,
    fade_out_ms: u64,
}

impl Default for Visibility {
    fn default() -> Self {
        Self::new()
    }
}

impl Visibility {
    /// Hidden, click-through, nothing pending.
    pub fn new() -> Self {
        let t = Timings::default();
        Visibility {
            phase: Phase::Hidden,
            ramp: Ramp::AT_ZERO,
            fade_in_until: 0,
            hold_until: 0,
            linger_until: 0,
            hovered: false,
            hover_since: None,
            interactive: false,
            pinned: false,
            dragging: false,
            tray_forced: false,
            preview_opacity: None,
            needs_leave: false,
            fade_in_ms: t.fade_in_ms,
            fade_out_ms: t.fade_out_ms,
        }
    }

    /// Shows the card. Hold is `crossing_hold_ms` for crossings and tray
    /// clicks, `hold_ms` otherwise; it extends, never shortens, an active hold.
    pub fn show(&mut self, r: ShowReason, now: u64, t: &Timings) {
        self.capture(t);
        self.needs_leave = false;
        if r == ShowReason::Tray {
            self.tray_forced = true;
        }
        let hold = match r {
            ShowReason::Crossing | ShowReason::Tray => t.crossing_hold_ms,
            ShowReason::Reminder | ShowReason::Unreachable => t.hold_ms,
        };
        match self.phase {
            Phase::Hidden | Phase::FadingOut => {
                self.start_fade_in(now);
                self.hold_until = self.fade_in_until.saturating_add(hold);
            }
            Phase::FadingIn | Phase::Holding => {
                self.hold_until = self.hold_until.max(now.saturating_add(hold));
            }
            Phase::Lingering => {
                self.phase = Phase::Holding;
                self.hold_until = self.linger_until.max(now.saturating_add(hold));
            }
        }
        self.fade_toward_target(now);
    }

    /// Starts fading out now (tray click on a visible card). No-op while
    /// pinned, hidden or already fading out.
    pub fn hide(&mut self, now: u64) {
        if self.pinned || matches!(self.phase, Phase::Hidden | Phase::FadingOut) {
            return;
        }
        self.hovered = false;
        self.hover_since = None;
        self.dragging = false;
        self.needs_leave = true;
        self.begin_fade_out(now);
    }

    /// Pins (dire: visible, full alpha, interactive) or unpins (lingers,
    /// then fades). Idempotent: repeated calls with the same value are no-ops.
    pub fn set_pinned(&mut self, pinned: bool, now: u64, t: &Timings) {
        self.capture(t);
        if pinned == self.pinned {
            return;
        }
        self.pinned = pinned;
        if pinned {
            match self.phase {
                Phase::Hidden | Phase::FadingOut => {
                    self.start_fade_in(now);
                    self.hold_until = self.fade_in_until;
                }
                Phase::Lingering => self.phase = Phase::Holding,
                Phase::FadingIn | Phase::Holding => {}
            }
            self.fade_toward_target(now);
        } else {
            self.release(now, t);
        }
    }

    /// Polled cursor position. Resting inside for `hover_rest_ms` makes the
    /// card hovered and interactive, cancelling any fade-out; leaving starts
    /// the linger.
    pub fn cursor(&mut self, inside: bool, now: u64, t: &Timings) {
        self.capture(t);
        if !inside {
            self.hover_since = None;
            self.needs_leave = false;
            if self.hovered {
                self.hovered = false;
                self.release(now, t);
            }
            return;
        }
        if !self.is_visible() || self.needs_leave {
            self.hover_since = None;
            return;
        }
        let since = *self.hover_since.get_or_insert(now);
        if !self.hovered && now.saturating_sub(since) >= t.hover_rest_ms {
            self.hovered = true;
            self.grab(now);
        }
    }

    /// Mouse button pressed on the card: cancels any fade-out and holds.
    pub fn button_down(&mut self, now: u64) {
        if !self.is_visible() {
            return;
        }
        self.dragging = true;
        self.grab(now);
    }

    /// Mouse button released / drag finished: lingers unless still hovered.
    pub fn drag_end(&mut self, now: u64, t: &Timings) {
        self.capture(t);
        if self.dragging {
            self.dragging = false;
            self.release(now, t);
        }
    }

    /// Advances phases whose deadline has passed (catches up on late ticks).
    pub fn tick(&mut self, now: u64, t: &Timings) {
        self.capture(t);
        loop {
            match self.phase {
                Phase::FadingIn if now >= self.fade_in_until => self.phase = Phase::Holding,
                Phase::Holding if !self.held() && now >= self.hold_until => {
                    self.begin_fade_out(self.hold_until.max(self.ramp.start));
                }
                Phase::Lingering if now >= self.linger_until => {
                    self.begin_fade_out(self.linger_until.max(self.ramp.start));
                }
                Phase::FadingOut if now >= self.ramp.end() => {
                    self.phase = Phase::Hidden;
                    self.tray_forced = false;
                    self.interactive = false;
                    self.hovered = false;
                    self.hover_since = None;
                    self.dragging = false;
                    return;
                }
                _ => return,
            }
        }
    }

    /// Window alpha 0–255. `idle_pct` is the configured idle opacity;
    /// the opacity-popup preview, when set, replaces it.
    pub fn alpha(&self, now: u64, _t: &Timings, idle_pct: u8) -> u8 {
        if self.phase == Phase::Hidden {
            return 0;
        }
        let pct = u32::from(self.preview_opacity.unwrap_or(idle_pct).min(100));
        let idle_alpha = (pct * 255 / 100) as u8;
        self.ramp.at(now).alpha(idle_alpha)
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn is_visible(&self) -> bool {
        self.phase != Phase::Hidden
    }

    /// `WS_EX_TRANSPARENT` wanted: neither interactive nor pinned.
    pub fn click_through(&self) -> bool {
        !(self.interactive || self.pinned)
    }

    /// An alpha ramp is in progress (run the animation timer fast).
    pub fn needs_fast_anim(&self, now: u64, _t: &Timings) -> bool {
        self.is_visible()
            && (matches!(self.phase, Phase::FadingIn | Phase::FadingOut) || now < self.ramp.end())
    }

    /// True from `show(Tray)` until the card is hidden.
    pub fn tray_forced(&self) -> bool {
        self.tray_forced
    }

    /// Opacity-popup live preview (percent), or `None` to end it. While
    /// set, the card shows the preview level even when hovered (pinned
    /// keeps 255) and never lingers or fades out. Ending it resumes the
    /// normal rules: full alpha if still hovered, else the linger.
    pub fn set_preview_opacity(&mut self, v: Option<u8>, now: u64, t: &Timings) {
        self.capture(t);
        let was = self.preview_opacity.is_some();
        self.preview_opacity = v;
        if !self.is_visible() {
            return;
        }
        if v.is_some() {
            self.grab(now);
        } else if was {
            self.release(now, t);
            self.fade_toward_target(now);
        }
    }

    fn capture(&mut self, t: &Timings) {
        self.fade_in_ms = t.fade_in_ms;
        self.fade_out_ms = t.fade_out_ms;
    }

    /// Something keeps the card from lingering or fading out.
    fn held(&self) -> bool {
        self.hovered || self.dragging || self.pinned || self.preview_opacity.is_some()
    }

    /// Pinned beats the preview; the preview beats hover, drag and tray.
    fn target(&self) -> Mix {
        if self.pinned {
            Mix::FULL
        } else if self.preview_opacity.is_some() {
            Mix::IDLE
        } else if self.hovered || self.dragging || self.tray_forced {
            Mix::FULL
        } else {
            Mix::IDLE
        }
    }

    /// Ramps from the current alpha to the target over `fade_in_ms`,
    /// unless already heading there.
    fn fade_toward_target(&mut self, now: u64) {
        let to = self.target();
        if self.ramp.to != to {
            self.ramp = Ramp {
                from: self.ramp.at(now),
                to,
                start: now,
                dur: self.fade_in_ms,
            };
        }
    }

    fn start_fade_in(&mut self, now: u64) {
        self.phase = Phase::FadingIn;
        self.fade_in_until = now.saturating_add(self.fade_in_ms);
    }

    fn begin_fade_out(&mut self, at: u64) {
        self.phase = Phase::FadingOut;
        self.interactive = false;
        self.ramp = Ramp {
            from: self.ramp.at(at),
            to: Mix::ZERO,
            start: at,
            dur: self.fade_out_ms,
        };
    }

    /// Hover or button-down: interactive, full alpha, fade-out cancelled.
    fn grab(&mut self, now: u64) {
        self.interactive = true;
        if matches!(self.phase, Phase::FadingOut | Phase::Lingering) {
            self.phase = Phase::Holding;
        }
        self.fade_toward_target(now);
    }

    /// Hover, drag or pin ended: linger (never cutting a hold short) unless
    /// something else still holds the card.
    fn release(&mut self, now: u64, t: &Timings) {
        if self.held() || matches!(self.phase, Phase::Hidden | Phase::FadingOut) {
            return;
        }
        self.phase = Phase::Lingering;
        self.linger_until = now.saturating_add(t.linger_ms).max(self.hold_until);
        self.fade_toward_target(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: u8 = 50; // → alpha 127

    fn t() -> Timings {
        Timings::default()
    }

    fn shown(r: ShowReason) -> Visibility {
        let mut v = Visibility::new();
        v.show(r, 0, &t());
        v
    }

    fn at(v: &mut Visibility, now: u64) -> (Phase, u8) {
        v.tick(now, &t());
        (v.phase(), v.alpha(now, &t(), IDLE))
    }

    fn hover(v: &mut Visibility, from: u64) {
        v.cursor(true, from, &t());
        v.cursor(true, from + t().hover_rest_ms, &t());
    }

    #[test]
    fn new_is_hidden_and_click_through() {
        let v = Visibility::new();
        assert_eq!(v.phase(), Phase::Hidden);
        assert!(!v.is_visible() && v.click_through() && !v.tray_forced());
        assert_eq!(v.alpha(0, &t(), IDLE), 0);
    }

    #[test]
    fn crossing_timeline() {
        let mut v = shown(ShowReason::Crossing);
        assert_eq!(v.phase(), Phase::FadingIn);
        assert_eq!(v.alpha(0, &t(), IDLE), 0);
        assert!(v.alpha(100, &t(), IDLE).abs_diff(63) <= 2);
        assert_eq!(at(&mut v, 200), (Phase::Holding, 127));
        assert_eq!(at(&mut v, 6199).0, Phase::Holding);
        assert_eq!(at(&mut v, 6200), (Phase::FadingOut, 127));
        assert!(at(&mut v, 6500).1.abs_diff(63) <= 2);
        assert_eq!(at(&mut v, 6799).0, Phase::FadingOut);
        assert_eq!(at(&mut v, 6800), (Phase::Hidden, 0));
        assert!(v.click_through());
    }

    #[test]
    fn reminder_holds_4s() {
        let mut v = shown(ShowReason::Reminder);
        assert_eq!(at(&mut v, 4199).0, Phase::Holding);
        assert_eq!(at(&mut v, 4200).0, Phase::FadingOut);
    }

    #[test]
    fn late_tick_catches_up_to_hidden() {
        let mut v = shown(ShowReason::Reminder);
        assert_eq!(at(&mut v, 60_000), (Phase::Hidden, 0));
    }

    #[test]
    fn hover_rest_makes_interactive() {
        let mut v = shown(ShowReason::Crossing);
        v.tick(200, &t());
        v.cursor(true, 1000, &t());
        v.cursor(true, 1149, &t());
        assert!(v.click_through());
        v.cursor(true, 1150, &t());
        assert!(!v.click_through());
        assert!(v.alpha(1250, &t(), IDLE) > 127);
        assert_eq!(v.alpha(1350, &t(), IDLE), 255);
    }

    #[test]
    fn hover_ignored_while_hidden() {
        let mut v = Visibility::new();
        v.cursor(true, 0, &t());
        v.cursor(true, 1000, &t());
        assert!(v.click_through() && !v.is_visible());
    }

    #[test]
    fn hover_cancels_fade_out() {
        let mut v = shown(ShowReason::Reminder);
        assert_eq!(at(&mut v, 4300).0, Phase::FadingOut);
        hover(&mut v, 4300);
        assert_eq!(v.phase(), Phase::Holding);
        assert!(!v.click_through());
        assert_eq!(at(&mut v, 4650), (Phase::Holding, 255));
        assert_eq!(at(&mut v, 100_000), (Phase::Holding, 255));
    }

    #[test]
    fn leave_lingers_3s_then_fades() {
        let mut v = shown(ShowReason::Crossing);
        v.tick(200, &t());
        hover(&mut v, 1000);
        v.cursor(false, 5000, &t());
        assert_eq!(v.phase(), Phase::Lingering);
        assert!(!v.click_through(), "interactive until the linger ends");
        assert_eq!(at(&mut v, 5200), (Phase::Lingering, 127));
        assert_eq!(at(&mut v, 7999).0, Phase::Lingering);
        assert_eq!(at(&mut v, 8000).0, Phase::FadingOut);
        assert!(v.click_through());
    }

    #[test]
    fn early_leave_does_not_cut_a_hold_short() {
        let mut v = shown(ShowReason::Crossing);
        hover(&mut v, 300);
        v.cursor(false, 1000, &t());
        assert_eq!(at(&mut v, 6199).0, Phase::Lingering);
        assert_eq!(at(&mut v, 6200).0, Phase::FadingOut);
    }

    #[test]
    fn button_down_holds_and_drag_end_lingers() {
        let mut v = shown(ShowReason::Reminder);
        v.tick(4300, &t());
        v.button_down(4300);
        assert_eq!(v.phase(), Phase::Holding);
        assert!(!v.click_through());
        assert_eq!(at(&mut v, 20_000), (Phase::Holding, 255));
        v.drag_end(20_000, &t());
        assert_eq!(v.phase(), Phase::Lingering);
        assert_eq!(at(&mut v, 23_000).0, Phase::FadingOut);
    }

    #[test]
    fn drag_end_while_hovered_keeps_holding() {
        let mut v = shown(ShowReason::Reminder);
        hover(&mut v, 300);
        v.button_down(1000);
        v.drag_end(2000, &t());
        assert_eq!(at(&mut v, 50_000).0, Phase::Holding);
    }

    #[test]
    fn pinned_always_full_and_interactive() {
        let mut v = Visibility::new();
        v.set_pinned(true, 0, &t());
        assert!(v.is_visible() && !v.click_through());
        assert_eq!(at(&mut v, 200), (Phase::Holding, 255));
        assert_eq!(at(&mut v, 1_000_000), (Phase::Holding, 255));
        v.hide(1_000_000);
        assert_eq!(at(&mut v, 1_000_001).0, Phase::Holding);
        v.set_pinned(true, 1_000_001, &t());
        assert_eq!(v.alpha(1_000_001, &t(), IDLE), 255);
    }

    #[test]
    fn unpin_lingers() {
        let mut v = Visibility::new();
        v.set_pinned(true, 0, &t());
        v.tick(1000, &t());
        v.set_pinned(false, 1000, &t());
        assert_eq!(v.phase(), Phase::Lingering);
        assert_eq!(at(&mut v, 1200), (Phase::Lingering, 127));
        assert_eq!(at(&mut v, 4000).0, Phase::FadingOut);
        assert!(v.click_through());
    }

    #[test]
    fn tray_show_full_opacity_and_forced() {
        let mut v = shown(ShowReason::Tray);
        assert!(v.tray_forced());
        assert_eq!(at(&mut v, 200), (Phase::Holding, 255));
        assert_eq!(at(&mut v, 6200).0, Phase::FadingOut);
        assert!(v.tray_forced());
        assert_eq!(at(&mut v, 6800).0, Phase::Hidden);
        assert!(!v.tray_forced());
    }

    #[test]
    fn show_during_hold_extends_not_shortens() {
        let mut v = shown(ShowReason::Crossing); // holds to 6200
        v.show(ShowReason::Reminder, 1000, &t()); // would end 5000
        assert_eq!(at(&mut v, 6199).0, Phase::Holding);
        v.show(ShowReason::Reminder, 3000, &t()); // → 7000
        assert_eq!(at(&mut v, 6200).0, Phase::Holding);
        assert_eq!(at(&mut v, 6999).0, Phase::Holding);
        assert_eq!(at(&mut v, 7000).0, Phase::FadingOut);
    }

    #[test]
    fn show_during_fade_out_fades_back_in_from_current_alpha() {
        let mut v = shown(ShowReason::Reminder);
        v.tick(4500, &t()); // half-way out
        let mid = v.alpha(4500, &t(), IDLE);
        v.show(ShowReason::Reminder, 4500, &t());
        assert_eq!(v.phase(), Phase::FadingIn);
        assert_eq!(v.alpha(4500, &t(), IDLE), mid);
        assert_eq!(at(&mut v, 4700), (Phase::Holding, 127));
    }

    #[test]
    fn preview_opacity_overrides_idle() {
        let mut v = shown(ShowReason::Reminder);
        assert_eq!(at(&mut v, 200).1, 127);
        v.set_preview_opacity(Some(80), 200, &t());
        assert_eq!(v.alpha(200, &t(), IDLE), 204);
        v.set_preview_opacity(None, 200, &t());
        assert_eq!(v.alpha(200, &t(), IDLE), 127);
    }

    #[test]
    fn preview_shows_through_hover() {
        let mut v = shown(ShowReason::Reminder);
        hover(&mut v, 300); // full alpha by 650
        assert_eq!(at(&mut v, 1000).1, 255);
        v.set_preview_opacity(Some(30), 1000, &t());
        assert_eq!(at(&mut v, 1200), (Phase::Holding, 76));
        v.set_preview_opacity(Some(90), 1300, &t());
        assert_eq!(v.alpha(1300, &t(), IDLE), 229);
    }

    #[test]
    fn pinned_keeps_full_alpha_during_preview() {
        let mut v = Visibility::new();
        v.set_pinned(true, 0, &t());
        v.set_preview_opacity(Some(30), 0, &t());
        assert_eq!(at(&mut v, 500).1, 255);
    }

    #[test]
    fn no_fade_out_while_preview_set() {
        let mut v = shown(ShowReason::Reminder);
        hover(&mut v, 300);
        v.set_preview_opacity(Some(40), 1000, &t());
        v.cursor(false, 2000, &t());
        assert_eq!(at(&mut v, 60_000), (Phase::Holding, 102));
    }

    #[test]
    fn clearing_preview_outside_lingers() {
        let mut v = shown(ShowReason::Reminder);
        hover(&mut v, 300);
        v.set_preview_opacity(Some(40), 1000, &t());
        v.cursor(false, 2000, &t());
        v.set_preview_opacity(None, 10_000, &t());
        assert_eq!(v.phase(), Phase::Lingering);
        assert_eq!(at(&mut v, 12_999).0, Phase::Lingering);
        assert_eq!(at(&mut v, 13_000).0, Phase::FadingOut);
    }

    #[test]
    fn clearing_preview_while_hovered_returns_to_full() {
        let mut v = shown(ShowReason::Reminder);
        hover(&mut v, 300);
        v.set_preview_opacity(Some(40), 1000, &t());
        v.set_preview_opacity(None, 2000, &t());
        assert_eq!(at(&mut v, 2200), (Phase::Holding, 255));
    }

    #[test]
    fn hide_not_undone_until_cursor_leaves() {
        let mut v = shown(ShowReason::Reminder);
        hover(&mut v, 300);
        v.hide(1000);
        hover(&mut v, 1000);
        assert_eq!(v.phase(), Phase::FadingOut);
        assert_eq!(at(&mut v, 1600).0, Phase::Hidden);
    }

    #[test]
    fn hover_rearms_after_leave_or_show() {
        let mut v = shown(ShowReason::Reminder);
        hover(&mut v, 300);
        v.hide(1000);
        v.cursor(false, 1100, &t());
        hover(&mut v, 1200);
        assert!(!v.click_through(), "re-armed by leaving");

        let mut v = shown(ShowReason::Reminder);
        hover(&mut v, 300);
        v.hide(1000);
        v.show(ShowReason::Crossing, 1100, &t());
        hover(&mut v, 1200);
        assert!(!v.click_through(), "re-armed by a new show");
    }

    #[test]
    fn hide_fades_out_now() {
        let mut v = shown(ShowReason::Crossing);
        v.tick(1000, &t());
        v.hide(1000);
        assert_eq!(v.phase(), Phase::FadingOut);
        assert!(v.alpha(1300, &t(), IDLE).abs_diff(63) <= 2);
        assert_eq!(at(&mut v, 1600), (Phase::Hidden, 0));
    }

    #[test]
    fn fast_anim_only_while_ramping() {
        let mut v = shown(ShowReason::Reminder);
        assert!(v.needs_fast_anim(100, &t()));
        v.tick(200, &t());
        assert!(!v.needs_fast_anim(200, &t()));
        hover(&mut v, 1000);
        assert!(v.needs_fast_anim(1200, &t()));
        assert!(!v.needs_fast_anim(1350, &t()));
        v.hide(2000);
        assert!(v.needs_fast_anim(2100, &t()));
        v.tick(2600, &t());
        assert!(!v.needs_fast_anim(2600, &t()));
    }
}
