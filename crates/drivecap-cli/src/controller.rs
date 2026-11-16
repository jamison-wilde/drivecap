//! The app controller: a pure state machine that turns [`Input`]s (timer
//! ticks, poll results, mouse and tray events, menu commands) into
//! [`Effect`]s for the Win32 adapter to execute (ADR-0001). It owns the
//! config, per-volume alert state, the card's [`Visibility`] and the open
//! popup, and wires them together:
//!
//! - **Polling:** each watched volume carries a `next_poll_at`; `Tick`
//!   emits [`Effect::PollLocal`] / [`Effect::PollNetwork`] for due ones and
//!   a `Sample` reschedules on the volume's zone cadence.
//! - **Showing:** any crossing, dire entry, reminder or unreachable drop
//!   shows the card, and every warning volume's reminder is pushed a full
//!   interval out so a reminder never trails a showing.
//! - **Pinning:** the card is pinned while any non-snoozed volume is dire.
//! - **Change detection:** the controller remembers the last view, alpha,
//!   click-through, tray state and animation rate it reported and emits
//!   [`Effect::Relayout`], [`Effect::Redraw`], [`Effect::SetClickThrough`],
//!   [`Effect::UpdateTray`] and [`Effect::SetAnim`] only on change.
//! - **Saving:** [`Effect::SaveConfig`] is never emitted while the config
//!   is not writable (malformed file on disk — builder's call: never
//!   overwrite it until it parses again).

use crate::alerts::{AlertEvent, Sample, Zone};
use crate::config::{Config, FontSpec, Mode, SnoozeChoice, VolumeId};
use crate::model::{Dot, RowInput, SurfaceView, build_view, format_free};
use crate::polling::Volumes;
use crate::visibility::{Phase, ShowReason, Visibility};
use crate::volumes::{RawVolume, VolumeInfo, VolumeKind};

/// Periodic volume rescan interval.
pub const RESCAN_INTERVAL_MS: u64 = 300_000;

/// What a snooze command applies to.
#[derive(Clone, Debug, PartialEq)]
pub enum SnoozeTarget {
    /// Every at-risk volume.
    All,
    Volume(VolumeId),
}

/// A user command from a menu, header button or popup.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Toggle the snooze of the `i`th row of [`Controller::view`].
    SnoozeRow(usize),
    /// Header Zz: snooze every at-risk volume, or wake them all if all are snoozed.
    SnoozeAll,
    SnoozeWith(SnoozeTarget, SnoozeChoice),
    ToggleMode,
    SetSnoozeChoice(SnoozeChoice),
    /// Volumes ▸ checkbox: watch or stop watching a connected volume.
    ToggleVolume {
        id: VolumeId,
        label: String,
    },
    /// "Hide this volume" on the `i`th row.
    HideRow(usize),
    OpenOpacityPopup,
    OpenSettingsPopup,
    ClosePopups,
    SetOpacityPreview(u8),
    CommitOpacity(u8),
    ChangeFont,
    FontChosen(FontSpec),
    EditConfig,
    Quit,
}

/// Everything the adapter feeds the controller.
#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "one short-lived Input per message; keeps the planned ConfigReloaded(Config) interface"
)]
pub enum Input {
    /// Animation / poll timer.
    Tick,
    /// Result of a volume enumeration: the watched set and every connected volume.
    VolumesScanned {
        selected: Vec<VolumeInfo>,
        connected: Vec<RawVolume>,
    },
    /// A poll result; `None` = query failed / unreachable.
    Sample {
        id: VolumeId,
        free: Option<u64>,
    },
    /// Polled cursor position relative to the card.
    Cursor {
        inside: bool,
    },
    ButtonDown,
    DragEnd,
    TrayClick,
    /// A second instance was launched (behaves like a tray click).
    Relaunched,
    ConfigReloaded(Config),
    Cmd(Command),
}

/// Open popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Popup {
    Opacity,
    Settings,
}

/// Animation timer rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimRate {
    Off,
    /// 50 ms: visible, nothing ramping.
    Slow50,
    /// 16 ms: an alpha ramp or the dire halo is animating.
    Fast16,
}

/// Work for the adapter.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Query a local volume on the UI thread and send back a `Sample`.
    PollLocal {
        id: VolumeId,
        root: String,
    },
    /// Hand a network folder to the probe worker.
    PollNetwork {
        id: VolumeId,
        root: String,
    },
    /// Re-enumerate volumes and send back `VolumesScanned`.
    Rescan,
    /// Repaint with the current view and alpha.
    Redraw,
    /// Recompute the card geometry (row set, mode or font changed), then repaint.
    Relayout,
    SetClickThrough(bool),
    SetAnim(AnimRate),
    UpdateTray,
    SaveConfig,
    OpenConfig,
    ShowFontDialog,
    ShowPopup(Option<Popup>),
    Quit,
}

/// Tray icon state.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayState {
    pub dot: Dot,
    /// Every at-risk volume is snoozed ("Zz" mark).
    pub all_snoozed: bool,
    pub tooltip: String,
}

/// The pure app controller.
pub struct Controller {
    cfg: Config,
    writable: bool,
    vols: Volumes,
    connected: Vec<(VolumeId, String, bool)>,
    vis: Visibility,
    popup: Option<Popup>,
    last_rescan: u64,
    // Last state reported to the adapter, for change detection.
    last_view: SurfaceView,
    last_alpha: u8,
    last_click_through: bool,
    last_tray: TrayState,
    last_anim: AnimRate,
}

impl Controller {
    /// A controller with no volumes yet; emits nothing. The adapter sends
    /// `Input::VolumesScanned` first.
    pub fn new(cfg: Config, writable: bool, now: u64) -> Self {
        let mut c = Controller {
            cfg,
            writable,
            vols: Volumes::default(),
            connected: Vec::new(),
            vis: Visibility::new(),
            popup: None,
            last_rescan: now,
            last_view: SurfaceView {
                title: String::new(),
                dot: Dot::Muted,
                snooze_all_active: false,
                rows: Vec::new(),
                halo: false,
                mode: Mode::Expanded,
            },
            last_alpha: 0,
            last_click_through: true,
            last_tray: TrayState {
                dot: Dot::Muted,
                all_snoozed: false,
                tooltip: String::new(),
            },
            last_anim: AnimRate::Off,
        };
        c.last_view = c.view(now);
        c.last_alpha = c.alpha(now);
        c.last_click_through = c.vis.click_through();
        c.last_tray = c.tray_state(now);
        c
    }

    /// Applies one input and returns the effects it produces.
    pub fn handle(&mut self, input: Input, now: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        match input {
            Input::Tick => self.tick(now, &mut fx),
            Input::VolumesScanned {
                selected,
                connected,
            } => {
                self.scanned(selected, connected, now);
            }
            Input::Sample { id, free } => self.sample(&id, free, now),
            Input::Cursor { inside } => self.vis.cursor(inside, now, &self.cfg.timings),
            Input::ButtonDown => self.vis.button_down(now),
            Input::DragEnd => self.vis.drag_end(now, &self.cfg.timings),
            Input::TrayClick | Input::Relaunched => self.tray_click(now),
            Input::ConfigReloaded(cfg) => {
                self.cfg = cfg;
                self.writable = true;
                self.vols.apply_rules(&self.cfg);
                fx.extend([Effect::Rescan, Effect::Relayout]);
            }
            Input::Cmd(cmd) => self.command(cmd, now, &mut fx),
        }
        self.finish(now, &mut fx);
        fx
    }

    /// The card's view model. Tray-forced showings are always expanded.
    pub fn view(&self, now: u64) -> SurfaceView {
        let rows: Vec<RowInput> = self
            .vols
            .iter()
            .map(|v| RowInput {
                id: &v.info.id,
                letter: &v.info.letter,
                label: &v.info.label,
                rule: v.rule,
                alert: &v.alert,
            })
            .collect();
        let mode = if self.vis.tray_forced() {
            Mode::Expanded
        } else {
            self.cfg.mode
        };
        build_view(
            &rows,
            mode,
            self.cfg.snooze.label(),
            self.cfg.level_gaps,
            now,
            &self.cfg.timings,
        )
    }

    /// Card window alpha (0–255).
    pub fn alpha(&self, now: u64) -> u8 {
        self.vis
            .alpha(now, &self.cfg.timings, self.cfg.idle_opacity_pct)
    }

    pub fn visibility(&self) -> &Visibility {
        &self.vis
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn popup(&self) -> Option<Popup> {
        self.popup
    }

    /// Connected volumes for the Volumes ▸ menu: id, `"E: SanDisk 64GB"`
    /// label, and whether it is currently watched.
    pub fn connected_volumes(&self) -> Vec<(VolumeId, String, bool)> {
        self.connected.clone()
    }

    /// Tray glyph colour, snoozed mark and tooltip (worst awake at-risk volume).
    pub fn tray_state(&self, now: u64) -> TrayState {
        let at_risk: Vec<_> = self.vols.iter().filter(|v| v.alert.is_at_risk()).collect();
        let worst = at_risk
            .iter()
            .filter(|v| !v.alert.is_snoozed(now))
            .min_by_key(|v| (v.alert.zone, v.alert.free.unwrap_or(u64::MAX)));
        let all_snoozed = !at_risk.is_empty() && worst.is_none();
        let (dot, tooltip) = match worst {
            Some(v) => {
                let name = if v.info.letter.is_empty() {
                    &v.info.label
                } else {
                    &v.info.letter
                };
                let free = format_free(v.alert.free.unwrap_or(0));
                let dot = if v.alert.zone == Zone::Dire {
                    Dot::DireAccent
                } else {
                    Dot::Warning
                };
                (dot, format!("Drivecap — {name} {free} free"))
            }
            None if all_snoozed => (Dot::Muted, "Drivecap — snoozed".to_owned()),
            None => (Dot::Muted, "Drivecap — all drives healthy".to_owned()),
        };
        TrayState {
            dot,
            all_snoozed,
            tooltip,
        }
    }

    fn tick(&mut self, now: u64, fx: &mut Vec<Effect>) {
        for info in self.vols.take_due(&self.cfg, now) {
            let (id, root) = (info.id, info.root);
            fx.push(match info.kind {
                VolumeKind::Local => Effect::PollLocal { id, root },
                VolumeKind::Network => Effect::PollNetwork { id, root },
            });
        }
        let events = self.vols.tick_alerts(&self.cfg.timings, now);
        for ev in events {
            self.on_event(ev, now);
        }
        self.vis.tick(now, &self.cfg.timings);
        if now.saturating_sub(self.last_rescan) >= RESCAN_INTERVAL_MS {
            // Also stamped here so a slow scan is not re-requested every tick.
            self.last_rescan = now;
            fx.push(Effect::Rescan);
        }
    }

    fn scanned(&mut self, selected: Vec<VolumeInfo>, connected: Vec<RawVolume>, now: u64) {
        self.vols.reconcile(selected, &self.cfg, now);
        self.connected = connected
            .into_iter()
            .map(|r| {
                let label = format!("{} {}", r.letter, r.label).trim().to_owned();
                let watched = self.vols.contains(&r.id);
                (r.id, label, watched)
            })
            .collect();
        self.last_rescan = now;
    }

    fn sample(&mut self, id: &VolumeId, free: Option<u64>, now: u64) {
        let s = free.map_or(Sample::Unreachable, Sample::Free);
        // Samples for volumes dropped by a rescan are ignored.
        if let Some(ev) = self.vols.observe(id, s, &self.cfg, now) {
            self.on_event(ev, now);
        }
    }

    fn on_event(&mut self, ev: AlertEvent, now: u64) {
        let reason = match ev {
            AlertEvent::Crossing | AlertEvent::EnteredDire => ShowReason::Crossing,
            AlertEvent::Reminder => ShowReason::Reminder,
            AlertEvent::BecameUnreachable => ShowReason::Unreachable,
        };
        self.show(reason, now);
    }

    fn show(&mut self, reason: ShowReason, now: u64) {
        self.vis.show(reason, now, &self.cfg.timings);
        self.vols.reminders_shown(&self.cfg.timings, now);
    }

    /// Tray left-click / relaunch: hide a showing card, else force-show it.
    fn tray_click(&mut self, now: u64) {
        if self.vis.is_visible() && self.vis.phase() != Phase::FadingOut {
            self.vis.hide(now);
        } else {
            self.show(ShowReason::Tray, now);
        }
    }

    fn command(&mut self, cmd: Command, now: u64, fx: &mut Vec<Effect>) {
        match cmd {
            Command::SnoozeRow(i) => {
                if let Some(id) = self.row_id(i, now) {
                    self.vols.toggle_snooze(&id, self.cfg.snooze, now);
                }
            }
            Command::SnoozeAll => self.vols.toggle_snooze_all(self.cfg.snooze, now),
            Command::SnoozeWith(SnoozeTarget::All, choice) => self.vols.snooze_all(choice, now),
            Command::SnoozeWith(SnoozeTarget::Volume(id), choice) => {
                self.vols.snooze(&id, choice, now);
            }
            Command::ToggleMode => {
                self.cfg.mode = match self.cfg.mode {
                    Mode::Expanded => Mode::Minimized,
                    Mode::Minimized => Mode::Expanded,
                };
                fx.extend([Effect::SaveConfig, Effect::Relayout]);
            }
            Command::SetSnoozeChoice(choice) => {
                self.cfg.snooze = choice;
                fx.extend([Effect::SaveConfig, Effect::Redraw]);
            }
            Command::ToggleVolume { id, label } => {
                let watched = self.vols.contains(&id);
                self.cfg.set_enabled(&id, &label, !watched);
                fx.extend([Effect::SaveConfig, Effect::Rescan]);
            }
            Command::HideRow(i) => {
                if let Some(id) = self.row_id(i, now) {
                    let label = self.entry_label(&id);
                    self.cfg.set_enabled(&id, &label, false);
                    self.vols.remove(&id);
                    fx.extend([Effect::SaveConfig, Effect::Relayout]);
                }
            }
            Command::ChangeFont => fx.push(Effect::ShowFontDialog),
            Command::FontChosen(spec) => {
                self.cfg.font = Some(spec);
                fx.extend([Effect::SaveConfig, Effect::Relayout]);
            }
            Command::EditConfig => fx.push(Effect::OpenConfig),
            Command::Quit => fx.push(Effect::Quit),
            popup_cmd => self.popup_command(popup_cmd, now, fx),
        }
    }

    fn popup_command(&mut self, cmd: Command, now: u64, fx: &mut Vec<Effect>) {
        let t = &self.cfg.timings;
        match cmd {
            Command::OpenOpacityPopup => {
                self.popup = Some(Popup::Opacity);
                self.vis
                    .set_preview_opacity(Some(self.cfg.idle_opacity_pct), now, t);
                fx.push(Effect::ShowPopup(self.popup));
            }
            Command::OpenSettingsPopup => {
                self.popup = Some(Popup::Settings);
                self.vis.set_preview_opacity(None, now, t);
                fx.push(Effect::ShowPopup(self.popup));
            }
            Command::SetOpacityPreview(v) => {
                self.vis.set_preview_opacity(Some(opacity_pct(v)), now, t);
            }
            Command::CommitOpacity(v) => {
                self.cfg.idle_opacity_pct = opacity_pct(v);
                self.close_popup(now, fx);
                fx.push(Effect::SaveConfig);
            }
            Command::ClosePopups => self.close_popup(now, fx),
            _ => {}
        }
    }

    fn close_popup(&mut self, now: u64, fx: &mut Vec<Effect>) {
        self.popup = None;
        self.vis.set_preview_opacity(None, now, &self.cfg.timings);
        fx.push(Effect::ShowPopup(None));
    }

    /// Id of the `i`th row of the current view.
    fn row_id(&self, i: usize, now: u64) -> Option<VolumeId> {
        self.view(now).rows.get(i).map(|r| r.id.clone())
    }

    /// Human label for a config entry: the Volumes ▸ label, else letter + label.
    fn entry_label(&self, id: &VolumeId) -> String {
        if let Some((_, label, _)) = self.connected.iter().find(|(c, _, _)| c == id) {
            return label.clone();
        }
        self.vols
            .get(id)
            .map(|v| {
                format!("{} {}", v.info.letter, v.info.label)
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default()
    }

    /// Re-pins, then emits whatever the adapter must re-apply.
    fn finish(&mut self, now: u64, fx: &mut Vec<Effect>) {
        let pinned = self.vols.any_awake_dire(now);
        self.vis.set_pinned(pinned, now, &self.cfg.timings);

        let view = self.view(now);
        let relayout = view.mode != self.last_view.mode
            || view.rows.len() != self.last_view.rows.len()
            || view
                .rows
                .iter()
                .zip(&self.last_view.rows)
                .any(|(a, b)| a.id != b.id);
        if relayout {
            push_once(fx, Effect::Relayout);
        }
        let alpha = self.alpha(now);
        if relayout || view != self.last_view || alpha != self.last_alpha {
            push_once(fx, Effect::Redraw);
        }
        self.last_view = view;
        self.last_alpha = alpha;

        let ct = self.vis.click_through();
        if ct != self.last_click_through {
            self.last_click_through = ct;
            fx.push(Effect::SetClickThrough(ct));
        }
        let tray = self.tray_state(now);
        if tray != self.last_tray {
            self.last_tray = tray;
            fx.push(Effect::UpdateTray);
        }
        let anim = self.anim_rate(now);
        if anim != self.last_anim {
            self.last_anim = anim;
            fx.push(Effect::SetAnim(anim));
        }
        if !self.writable {
            fx.retain(|e| *e != Effect::SaveConfig);
        }
    }

    fn anim_rate(&self, now: u64) -> AnimRate {
        if self.vis.needs_fast_anim(now, &self.cfg.timings) || self.last_view.halo {
            AnimRate::Fast16
        } else if self.vis.is_visible() || self.popup.is_some() {
            AnimRate::Slow50
        } else {
            AnimRate::Off
        }
    }
}

/// Idle opacity as stored: rounded to a multiple of 5, clamped 20–100.
fn opacity_pct(v: u8) -> u8 {
    let rounded = (u16::from(v) + 2) / 5 * 5;
    rounded.clamp(20, 100) as u8
}

fn push_once(fx: &mut Vec<Effect>, e: Effect) {
    if !fx.contains(&e) {
        fx.push(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::Snooze;
    use crate::config::GIB;
    use crate::volumes::{Bus, DRIVE_FIXED};

    const T0: u64 = 1_000_000_000;

    fn gb(v: f64) -> u64 {
        (v * GIB as f64) as u64
    }

    fn c_id() -> VolumeId {
        VolumeId(r"\\?\Volume{c}\".into())
    }

    fn c_info(letter: &str) -> VolumeInfo {
        VolumeInfo {
            id: c_id(),
            kind: VolumeKind::Local,
            root: format!("{letter}\\"),
            letter: letter.into(),
            label: "Windows".into(),
            removable: false,
        }
    }

    fn nas_info() -> VolumeInfo {
        VolumeInfo {
            id: VolumeId(r"\\nas\builds".into()),
            kind: VolumeKind::Network,
            root: r"\\nas\builds".into(),
            letter: String::new(),
            label: r"\\nas\builds".into(),
            removable: false,
        }
    }

    fn raw_of(i: &VolumeInfo) -> RawVolume {
        RawVolume {
            id: i.id.clone(),
            root: i.root.clone(),
            letter: i.letter.clone(),
            label: i.label.clone(),
            drive_type: DRIVE_FIXED,
            bus: Bus::Other,
        }
    }

    fn scan(c: &mut Controller, vols: Vec<VolumeInfo>, now: u64) -> Vec<Effect> {
        let connected = vols.iter().map(raw_of).collect();
        c.handle(
            Input::VolumesScanned {
                selected: vols,
                connected,
            },
            now,
        )
    }

    fn ctl_with(cfg: Config, writable: bool) -> Controller {
        let mut c = Controller::new(cfg, writable, T0);
        scan(&mut c, vec![c_info("C:")], T0);
        c
    }

    fn ctl() -> Controller {
        ctl_with(Config::default(), true)
    }

    fn sample(c: &mut Controller, free_gb: f64, now: u64) -> Vec<Effect> {
        let free = Some(gb(free_gb));
        c.handle(Input::Sample { id: c_id(), free }, now)
    }

    fn cmd(c: &mut Controller, cmd: Command, now: u64) -> Vec<Effect> {
        c.handle(Input::Cmd(cmd), now)
    }

    /// Ticks every 50 ms through `(from, to]`, collecting effects.
    fn tick_through(c: &mut Controller, from: u64, to: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        let mut now = from;
        while now < to {
            now = (now + 50).min(to);
            fx.extend(c.handle(Input::Tick, now));
        }
        fx
    }

    fn polled_c(fx: &[Effect]) -> bool {
        fx.iter()
            .any(|e| matches!(e, Effect::PollLocal { id, .. } if *id == c_id()))
    }

    #[test]
    fn startup_below_threshold_shows_card() {
        let mut c = ctl();
        let fx = sample(&mut c, 14.5, T0);
        assert!(c.visibility().is_visible());
        assert!(fx.contains(&Effect::Redraw));
    }

    #[test]
    fn healthy_sample_stays_hidden() {
        let mut c = ctl();
        sample(&mut c, 50.0, T0);
        tick_through(&mut c, T0, T0 + 1_000);
        assert!(!c.visibility().is_visible());
    }

    #[test]
    fn polls_scheduled_by_zone() {
        let mut c = ctl();
        let fx = c.handle(Input::Tick, T0);
        assert!(fx.contains(&Effect::PollLocal {
            id: c_id(),
            root: "C:\\".into()
        }));
        sample(&mut c, 50.0, T0);
        assert!(!polled_c(&c.handle(Input::Tick, T0 + 59_999)));
        let t1 = T0 + 60_000;
        assert!(polled_c(&c.handle(Input::Tick, t1)));
        sample(&mut c, 11.0, t1);
        assert!(!polled_c(&c.handle(Input::Tick, t1 + 34_999)));
        assert!(polled_c(&c.handle(Input::Tick, t1 + 35_000)));
    }

    #[test]
    fn network_volume_polls_via_network_effect() {
        let mut c = Controller::new(Config::default(), true, T0);
        scan(&mut c, vec![nas_info()], T0);
        let fx = c.handle(Input::Tick, T0);
        assert!(fx.contains(&Effect::PollNetwork {
            id: nas_info().id,
            root: r"\\nas\builds".into()
        }));
        assert!(!fx.iter().any(|e| matches!(e, Effect::PollLocal { .. })));
    }

    #[test]
    fn dire_pins_and_snooze_unpins() {
        let mut c = ctl();
        let fx = sample(&mut c, 1.0, T0);
        assert!(!c.visibility().click_through(), "dire card is pinned");
        assert!(fx.contains(&Effect::SetClickThrough(false)));
        let fx = cmd(&mut c, Command::SnoozeRow(0), T0 + 1_000);
        assert!(c.visibility().click_through(), "snoozed dire unpins");
        assert!(fx.contains(&Effect::SetClickThrough(true)));
        assert_eq!(
            c.vols.get(&c_id()).unwrap().alert.snooze,
            Snooze::Until {
                at_ms: T0 + 1_000 + 600_000,
                choice: SnoozeChoice::TenMin
            }
        );
    }

    #[test]
    fn snooze_all_toggles() {
        let mut c = ctl();
        sample(&mut c, 11.0, T0);
        cmd(&mut c, Command::SnoozeAll, T0 + 10);
        assert!(c.view(T0 + 10).snooze_all_active);
        cmd(&mut c, Command::SnoozeAll, T0 + 20);
        assert!(!c.view(T0 + 20).snooze_all_active);
        assert!(!c.vols.get(&c_id()).unwrap().alert.is_snoozed(T0 + 20));
    }

    #[test]
    fn snooze_with_uses_given_choice() {
        let mut c = ctl();
        sample(&mut c, 11.0, T0);
        let target = SnoozeTarget::Volume(c_id());
        cmd(
            &mut c,
            Command::SnoozeWith(target, SnoozeChoice::OneHour),
            T0,
        );
        assert_eq!(
            c.vols.get(&c_id()).unwrap().alert.snooze,
            Snooze::Until {
                at_ms: T0 + 3_600_000,
                choice: SnoozeChoice::OneHour
            }
        );
    }

    #[test]
    fn rescan_removes_vanished_and_ignores_late_sample() {
        let mut c = ctl();
        sample(&mut c, 50.0, T0);
        let fx = scan(&mut c, Vec::new(), T0 + 10);
        assert!(fx.contains(&Effect::Relayout));
        assert!(c.view(T0 + 10).rows.is_empty());
        let fx = sample(&mut c, 1.0, T0 + 20);
        assert!(fx.is_empty(), "late sample produced {fx:?}");
        assert!(!c.visibility().is_visible());
    }

    #[test]
    fn rescan_keeps_alert_state_for_reappearing_id() {
        let mut c = ctl();
        sample(&mut c, 14.5, T0);
        tick_through(&mut c, T0, T0 + 10_000);
        assert!(!c.visibility().is_visible());
        scan(&mut c, vec![c_info("E:")], T0 + 10_000);
        sample(&mut c, 14.5, T0 + 11_000);
        assert!(!c.visibility().is_visible(), "same level re-warned");
        assert_eq!(c.view(T0 + 11_000).rows[0].letter, "E:");
    }

    #[test]
    fn hide_row_disables_and_saves() {
        let mut c = ctl();
        sample(&mut c, 50.0, T0);
        let fx = cmd(&mut c, Command::HideRow(0), T0 + 10);
        assert_eq!(c.config().entry_enabled(&c_id()), Some(false));
        assert_eq!(c.config().volumes[0].label, "C: Windows");
        assert!(fx.contains(&Effect::SaveConfig));
        assert!(c.view(T0 + 10).rows.is_empty());
    }

    #[test]
    fn row_commands_out_of_range_are_noops() {
        let mut c = ctl();
        sample(&mut c, 11.0, T0);
        assert!(cmd(&mut c, Command::HideRow(5), T0).is_empty());
        assert!(cmd(&mut c, Command::SnoozeRow(5), T0).is_empty());
        assert!(c.config().volumes.is_empty());
    }

    #[test]
    fn toggle_volume_flips_watch_and_rescans() {
        let mut c = ctl();
        let (id, label, watched) = c.connected_volumes().remove(0);
        assert_eq!((label.as_str(), watched), ("C: Windows", true));
        let fx = cmd(&mut c, Command::ToggleVolume { id, label }, T0);
        assert_eq!(c.config().entry_enabled(&c_id()), Some(false));
        assert!(fx.contains(&Effect::SaveConfig) && fx.contains(&Effect::Rescan));
    }

    #[test]
    fn not_writable_never_emits_save() {
        let mut c = ctl_with(Config::default(), false);
        sample(&mut c, 50.0, T0);
        let cmds = [
            Command::ToggleMode,
            Command::SetSnoozeChoice(SnoozeChoice::OneHour),
            Command::CommitOpacity(80),
            Command::FontChosen(FontSpec::default()),
            Command::ToggleVolume {
                id: c_id(),
                label: "C: Windows".into(),
            },
            Command::HideRow(0),
        ];
        for k in cmds {
            let fx = cmd(&mut c, k.clone(), T0 + 10);
            assert!(!fx.contains(&Effect::SaveConfig), "{k:?} saved");
        }
    }

    #[test]
    fn tray_click_forces_expanded_then_reverts() {
        let cfg = Config {
            mode: Mode::Minimized,
            ..Config::default()
        };
        let mut c = ctl_with(cfg, true);
        sample(&mut c, 50.0, T0);
        assert!(
            c.view(T0).rows.is_empty(),
            "healthy row hidden when minimized"
        );
        let fx = c.handle(Input::TrayClick, T0 + 10);
        assert_eq!(c.view(T0 + 10).mode, Mode::Expanded);
        assert_eq!(c.view(T0 + 10).rows.len(), 1);
        assert!(fx.contains(&Effect::Relayout));
        let fx = tick_through(&mut c, T0 + 10, T0 + 10_000);
        assert!(!c.visibility().is_visible());
        assert_eq!(c.view(T0 + 10_000).mode, Mode::Minimized);
        assert!(fx.contains(&Effect::Relayout));
    }

    #[test]
    fn tray_click_while_visible_hides() {
        let mut c = ctl();
        c.handle(Input::TrayClick, T0);
        tick_through(&mut c, T0, T0 + 300);
        assert_eq!(c.visibility().phase(), Phase::Holding);
        c.handle(Input::Relaunched, T0 + 300);
        assert_eq!(c.visibility().phase(), Phase::FadingOut);
        tick_through(&mut c, T0 + 300, T0 + 1_000);
        assert!(!c.visibility().is_visible());
    }

    #[test]
    fn opacity_preview_and_commit() {
        let mut c = ctl();
        sample(&mut c, 14.5, T0);
        tick_through(&mut c, T0, T0 + 300);
        assert_eq!(c.alpha(T0 + 300), 127, "idle 50 %");
        let fx = cmd(&mut c, Command::OpenOpacityPopup, T0 + 300);
        assert_eq!(c.popup(), Some(Popup::Opacity));
        assert!(fx.contains(&Effect::ShowPopup(Some(Popup::Opacity))));
        cmd(&mut c, Command::SetOpacityPreview(80), T0 + 400);
        assert_eq!(c.alpha(T0 + 400), 204, "preview 80 %");
        assert_eq!(c.config().idle_opacity_pct, 50, "preview does not commit");
        let fx = cmd(&mut c, Command::CommitOpacity(80), T0 + 500);
        assert_eq!(c.config().idle_opacity_pct, 80);
        assert!(fx.contains(&Effect::SaveConfig));
        assert!(fx.contains(&Effect::ShowPopup(None)));
        assert_eq!(c.popup(), None);
    }

    #[test]
    fn close_popups_discards_preview_without_saving() {
        let mut c = ctl();
        cmd(&mut c, Command::OpenOpacityPopup, T0);
        cmd(&mut c, Command::SetOpacityPreview(90), T0);
        let fx = cmd(&mut c, Command::ClosePopups, T0 + 10);
        assert_eq!(c.popup(), None);
        assert_eq!(c.config().idle_opacity_pct, 50);
        assert!(fx.contains(&Effect::ShowPopup(None)));
        assert!(!fx.contains(&Effect::SaveConfig));
    }

    #[test]
    fn commit_opacity_rounds_and_clamps() {
        let mut c = ctl();
        cmd(&mut c, Command::CommitOpacity(83), T0);
        assert_eq!(c.config().idle_opacity_pct, 85);
        cmd(&mut c, Command::CommitOpacity(3), T0);
        assert_eq!(c.config().idle_opacity_pct, 20);
        cmd(&mut c, Command::CommitOpacity(250), T0);
        assert_eq!(c.config().idle_opacity_pct, 100);
    }

    #[test]
    fn config_reload_reapplies_rules() {
        let mut c = ctl();
        sample(&mut c, 50.0, T0);
        let mut cfg = Config::default();
        cfg.defaults.threshold_gb = 100.0;
        let fx = c.handle(Input::ConfigReloaded(cfg), T0 + 10);
        assert!(fx.contains(&Effect::Rescan) && fx.contains(&Effect::Relayout));
        sample(&mut c, 50.0, T0 + 20);
        assert_eq!(c.vols.get(&c_id()).unwrap().alert.zone, Zone::Warning);
    }

    #[test]
    fn config_reload_makes_unwritable_config_writable() {
        let mut c = ctl_with(Config::default(), false);
        c.handle(Input::ConfigReloaded(Config::default()), T0);
        let fx = cmd(&mut c, Command::ToggleMode, T0);
        assert!(fx.contains(&Effect::SaveConfig));
    }

    #[test]
    fn tooltip_text_variants() {
        let mut c = ctl();
        let s = c.tray_state(T0);
        assert_eq!(s.tooltip, "Drivecap — all drives healthy");
        assert_eq!((s.dot, s.all_snoozed), (Dot::Muted, false));
        let fx = sample(&mut c, 14.8, T0);
        assert!(fx.contains(&Effect::UpdateTray));
        let s = c.tray_state(T0);
        assert_eq!(s.tooltip, "Drivecap — C: 14.8 GB free");
        assert_eq!(s.dot, Dot::Warning);
        cmd(&mut c, Command::SnoozeAll, T0);
        let s = c.tray_state(T0);
        assert_eq!(s.tooltip, "Drivecap — snoozed");
        assert!(s.all_snoozed);
    }

    #[test]
    fn tooltip_uses_label_for_letterless_volume() {
        let mut c = Controller::new(Config::default(), true, T0);
        scan(&mut c, vec![nas_info()], T0);
        let free = Some(gb(1.0));
        c.handle(
            Input::Sample {
                id: nas_info().id,
                free,
            },
            T0,
        );
        let s = c.tray_state(T0);
        assert_eq!(s.tooltip, r"Drivecap — \\nas\builds 1.0 GB free");
        assert_eq!(s.dot, Dot::DireAccent);
    }

    #[test]
    fn anim_rate_off_when_hidden() {
        let mut c = ctl();
        let fx = sample(&mut c, 14.5, T0);
        assert!(fx.contains(&Effect::SetAnim(AnimRate::Fast16)));
        let fx = tick_through(&mut c, T0, T0 + 1_000);
        assert!(fx.contains(&Effect::SetAnim(AnimRate::Slow50)));
        let fx = tick_through(&mut c, T0 + 1_000, T0 + 10_000);
        assert!(!c.visibility().is_visible());
        assert!(fx.contains(&Effect::SetAnim(AnimRate::Off)));
        assert_eq!(c.anim_rate(T0 + 10_000), AnimRate::Off);
    }

    #[test]
    fn tick_requests_rescan_every_five_minutes() {
        let mut c = ctl();
        assert!(
            !c.handle(Input::Tick, T0 + 299_999)
                .contains(&Effect::Rescan)
        );
        assert!(
            c.handle(Input::Tick, T0 + 300_000)
                .contains(&Effect::Rescan)
        );
        assert!(
            !c.handle(Input::Tick, T0 + 300_050)
                .contains(&Effect::Rescan)
        );
    }

    #[test]
    fn simple_commands_map_to_effects() {
        let mut c = ctl();
        assert_eq!(
            cmd(&mut c, Command::ChangeFont, T0),
            [Effect::ShowFontDialog]
        );
        assert_eq!(cmd(&mut c, Command::EditConfig, T0), [Effect::OpenConfig]);
        assert_eq!(cmd(&mut c, Command::Quit, T0), [Effect::Quit]);
        let fx = cmd(&mut c, Command::ToggleMode, T0);
        assert_eq!(c.config().mode, Mode::Minimized);
        assert!(fx.contains(&Effect::SaveConfig) && fx.contains(&Effect::Relayout));
    }
}
