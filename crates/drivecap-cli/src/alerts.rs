//! Per-volume alert engine: zones, warning levels, debounce, snooze and
//! the reminder schedule. Pure — callers pass samples and `now` (unix ms)
//! and act on the returned [`AlertEvent`].
//!
//! ## Debounce (builder's call)
//!
//! A volume remembers the lowest level it was last warned at. Falling to
//! a lower level fires one [`AlertEvent::Crossing`], however many levels
//! the drop skipped. Level `L` re-arms only once free space rises above
//! `L + step`; re-arming the threshold level clears the memory entirely.
//! Dire entry re-arms once free rises above `dire + step`. Debounce state
//! is kept across unreachable periods, so a share that drops and recovers
//! does not re-warn the same level.

use crate::config::{Rule, SnoozeChoice, Timings};
use std::cmp::Ordering;

/// Volume state by severity. Sorting orders worst first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Zone {
    /// Free space above the threshold.
    #[default]
    Healthy,
    /// Threshold ≥ free > dire.
    Warning,
    /// Free ≤ dire.
    Dire,
    /// Network folder that failed its probe.
    Unreachable,
}

impl Zone {
    /// Sort key, worst first: Dire 0, Warning 1, Healthy 2, Unreachable 3.
    pub fn severity_rank(&self) -> u8 {
        match self {
            Zone::Dire => 0,
            Zone::Warning => 1,
            Zone::Healthy => 2,
            Zone::Unreachable => 3,
        }
    }
}

impl Ord for Zone {
    fn cmp(&self, other: &Self) -> Ordering {
        self.severity_rank().cmp(&other.severity_rank())
    }
}

impl PartialOrd for Zone {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One poll result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample {
    /// Free bytes available to the caller.
    Free(u64),
    Unreachable,
}

/// A volume's snooze state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Snooze {
    #[default]
    Off,
    /// Timed snooze ending at `at_ms`.
    Until { at_ms: u64, choice: SnoozeChoice },
    /// Until the next crossing or dire entry breaks through.
    NextLevel,
}

/// What the surface should do in response to a sample or tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertEvent {
    /// Fell past a new warning level.
    Crossing,
    /// Fell to or below dire.
    EnteredDire,
    /// Periodic warning-zone reminder is due.
    Reminder,
    /// Reachable → unreachable transition.
    BecameUnreachable,
}

/// Warning levels, descending: `threshold`, `threshold - step`, … while
/// the level stays above `dire`.
pub fn levels(rule: &Rule) -> Vec<u64> {
    let mut out = Vec::new();
    let mut level = rule.threshold;
    while level > rule.dire {
        out.push(level);
        match level.checked_sub(rule.step) {
            Some(next) if rule.step > 0 => level = next,
            _ => break,
        }
    }
    out
}

/// The level a free value sits at: the lowest level `>= free`, or `None`
/// outside (dire, threshold]. Equals the matching entry of [`levels`],
/// computed without materialising the list (hot path of every sample).
fn level_at(free: u64, rule: &Rule) -> Option<u64> {
    if free > rule.threshold || free <= rule.dire {
        return None;
    }
    if rule.step == 0 {
        return Some(rule.threshold);
    }
    let k = (rule.threshold - free) / rule.step;
    Some(rule.threshold - k * rule.step)
}

/// The last entry of [`levels`], computed arithmetically.
fn lowest_level(rule: &Rule) -> Option<u64> {
    if rule.threshold <= rule.dire {
        return None;
    }
    if rule.step == 0 {
        return Some(rule.threshold);
    }
    let k = (rule.threshold - rule.dire - 1) / rule.step;
    Some(rule.threshold - k * rule.step)
}

/// Zone for `free` bytes (never [`Zone::Unreachable`]).
pub fn zone_for(free: u64, rule: &Rule) -> Zone {
    if free > rule.threshold {
        Zone::Healthy
    } else if free > rule.dire {
        Zone::Warning
    } else {
        Zone::Dire
    }
}

/// Reminder interval: linear from `reminder_max_secs` at the threshold to
/// `reminder_min_secs` at dire, clamped outside that range.
pub fn reminder_interval_ms(free: u64, rule: &Rule, t: &Timings) -> u64 {
    let max = t.reminder_max_secs.saturating_mul(1000);
    let min = t
        .reminder_min_secs
        .min(t.reminder_max_secs)
        .saturating_mul(1000);
    if free >= rule.threshold {
        return max;
    }
    if free <= rule.dire {
        return min;
    }
    let span = u128::from(rule.threshold - rule.dire);
    let above = u128::from(free - rule.dire);
    min + (u128::from(max - min) * above / span) as u64
}

/// Alert state for one volume.
#[derive(Clone, Debug, Default)]
pub struct VolumeAlert {
    /// Last observed free bytes; `None` before the first sample or while unreachable.
    pub free: Option<u64>,
    pub zone: Zone,
    pub snooze: Snooze,
    /// Lowest level warned and not yet re-armed.
    last_warned: Option<u64>,
    dire_warned: bool,
    next_reminder_at: Option<u64>,
    unreachable_since: Option<u64>,
    /// Reminder interval as of the last warning-zone sample (for [`Self::wake`]).
    interval_ms: u64,
}

impl VolumeAlert {
    /// Records a sample and returns the event it triggers, if any.
    pub fn observe(&mut self, s: Sample, rule: &Rule, now: u64, t: &Timings) -> Option<AlertEvent> {
        let free = match s {
            Sample::Unreachable => return self.observe_unreachable(now),
            Sample::Free(f) => f,
        };
        self.free = Some(free);
        self.unreachable_since = None;
        self.zone = zone_for(free, rule);
        self.rearm(free, rule);

        let event = match self.zone {
            Zone::Dire => self.enter_dire(rule),
            Zone::Warning => self.cross_level(free, rule),
            _ => None,
        };
        if event.is_some() {
            self.snooze = Snooze::Off;
        }
        if self.zone == Zone::Dire && self.snooze == Snooze::NextLevel {
            // Re-entering dire without a fresh EnteredDire (not yet re-armed)
            // must still cap a "next level" snooze to a timed one.
            self.snooze(SnoozeChoice::NextLevel, now);
        }
        if self.zone == Zone::Warning {
            self.interval_ms = reminder_interval_ms(free, rule, t);
            let next = now.saturating_add(self.interval_ms);
            if event.is_some() {
                // The break-through just showed the surface; a reminder
                // right behind it would be redundant.
                self.next_reminder_at = Some(next);
            } else {
                self.next_reminder_at.get_or_insert(next);
            }
        } else {
            self.next_reminder_at = None;
        }
        event
    }

    /// Expires timed snoozes and fires due reminders.
    pub fn tick(&mut self, rule: &Rule, now: u64, t: &Timings) -> Option<AlertEvent> {
        let mut expired = false;
        if let Snooze::Until { at_ms, .. } = self.snooze
            && now >= at_ms
        {
            self.snooze = Snooze::Off;
            expired = true;
        }
        if self.zone != Zone::Warning || self.is_snoozed(now) {
            return None;
        }
        let due = self.next_reminder_at.is_some_and(|at| now >= at);
        if expired || due {
            self.reminder_shown(rule, now, t);
            return Some(AlertEvent::Reminder);
        }
        None
    }

    /// Snoozes this volume. A dire volume never gets "next level": it
    /// falls back to a 10-minute timed snooze.
    pub fn snooze(&mut self, choice: SnoozeChoice, now: u64) {
        let choice = if self.zone == Zone::Dire && choice == SnoozeChoice::NextLevel {
            SnoozeChoice::TenMin
        } else {
            choice
        };
        self.snooze = match choice.duration_ms() {
            Some(d) => Snooze::Until {
                at_ms: now.saturating_add(d),
                choice,
            },
            None => Snooze::NextLevel,
        };
    }

    /// Ends any snooze; a warning volume's next reminder is a full interval away.
    pub fn wake(&mut self, now: u64) {
        self.snooze = Snooze::Off;
        if self.zone == Zone::Warning {
            self.next_reminder_at = Some(now.saturating_add(self.interval_ms));
        }
    }

    /// Whether a snooze is in effect at `now`.
    pub fn is_snoozed(&self, now: u64) -> bool {
        match self.snooze {
            Snooze::Off => false,
            Snooze::Until { at_ms, .. } => now < at_ms,
            Snooze::NextLevel => true,
        }
    }

    /// Warning or Dire.
    pub fn is_at_risk(&self) -> bool {
        matches!(self.zone, Zone::Warning | Zone::Dire)
    }

    /// The surface was shown for any reason: push the next reminder a
    /// full interval out from `now`.
    pub fn reminder_shown(&mut self, rule: &Rule, now: u64, t: &Timings) {
        if self.zone == Zone::Warning
            && let Some(free) = self.free
        {
            self.interval_ms = reminder_interval_ms(free, rule, t);
            self.next_reminder_at = Some(now.saturating_add(self.interval_ms));
        }
    }

    /// Poll cadence: base for healthy/unreachable, the reminder scale in
    /// warning, `reminder_min_secs` when dire — never slower than base.
    pub fn poll_interval_ms(&self, rule: &Rule, base_secs: u64, t: &Timings) -> u64 {
        let base = base_secs.max(1).saturating_mul(1000);
        match (self.zone, self.free) {
            (Zone::Warning, Some(free)) => base.min(reminder_interval_ms(free, rule, t)),
            (Zone::Dire, _) => base.min(t.reminder_min_secs.max(1).saturating_mul(1000)),
            _ => base,
        }
    }

    /// Row visibility in minimized mode: un-snoozed at-risk volumes, and
    /// unreachable ones for `unreachable_drop_secs` after they dropped.
    pub fn show_in_minimized(&self, now: u64, t: &Timings) -> bool {
        if self.is_at_risk() {
            return !self.is_snoozed(now);
        }
        self.zone == Zone::Unreachable
            && self.unreachable_since.is_some_and(|since| {
                now.saturating_sub(since) < t.unreachable_drop_secs.saturating_mul(1000)
            })
    }

    fn observe_unreachable(&mut self, now: u64) -> Option<AlertEvent> {
        self.free = None;
        self.next_reminder_at = None;
        if self.zone == Zone::Unreachable {
            return None;
        }
        self.zone = Zone::Unreachable;
        self.unreachable_since = Some(now);
        Some(AlertEvent::BecameUnreachable)
    }

    /// Applies the re-arm rule for a rise in free space.
    fn rearm(&mut self, free: u64, rule: &Rule) {
        while let Some(l) = self.last_warned
            && free > l.saturating_add(rule.step)
        {
            // step == 0 (degenerate rule) clears instead of looping forever.
            let up = l.saturating_add(rule.step);
            self.last_warned = (rule.step > 0 && up <= rule.threshold).then_some(up);
        }
        if free > rule.dire.saturating_add(rule.step) {
            self.dire_warned = false;
        }
    }

    fn enter_dire(&mut self, rule: &Rule) -> Option<AlertEvent> {
        // Every level is passed on the way into dire; recovering to just
        // above dire must not re-fire the lowest one.
        if let Some(lowest) = lowest_level(rule) {
            self.last_warned = Some(self.last_warned.map_or(lowest, |l| l.min(lowest)));
        }
        if self.dire_warned {
            return None;
        }
        self.dire_warned = true;
        Some(AlertEvent::EnteredDire)
    }

    fn cross_level(&mut self, free: u64, rule: &Rule) -> Option<AlertEvent> {
        let current = level_at(free, rule)?;
        if self.last_warned.is_some_and(|l| current >= l) {
            return None;
        }
        self.last_warned = Some(current);
        Some(AlertEvent::Crossing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GIB;

    fn gb(x: f64) -> u64 {
        (x * GIB as f64) as u64
    }

    fn rule() -> Rule {
        Rule {
            threshold: 20 * GIB,
            step: 5 * GIB,
            dire: 2 * GIB,
        }
    }

    fn t() -> Timings {
        Timings::default()
    }

    /// Observes a free-space value (in GB) at time 0.
    fn obs(a: &mut VolumeAlert, free_gb: f64) -> Option<AlertEvent> {
        a.observe(Sample::Free(gb(free_gb)), &rule(), 0, &t())
    }

    #[test]
    fn levels_default() {
        assert_eq!(levels(&rule()), vec![20 * GIB, 15 * GIB, 10 * GIB, 5 * GIB]);
    }

    #[test]
    fn levels_stop_above_dire() {
        let r = Rule {
            dire: 5 * GIB,
            ..rule()
        };
        assert_eq!(levels(&r), vec![20 * GIB, 15 * GIB, 10 * GIB]);
        let r = Rule {
            threshold: 21 * GIB,
            ..rule()
        };
        assert_eq!(levels(&r), vec![21 * GIB, 16 * GIB, 11 * GIB, 6 * GIB]);
    }

    #[test]
    fn zone_boundaries() {
        assert_eq!(zone_for(20 * GIB, &rule()), Zone::Warning);
        assert_eq!(zone_for(20 * GIB + 1, &rule()), Zone::Healthy);
        assert_eq!(zone_for(2 * GIB, &rule()), Zone::Dire);
        assert_eq!(zone_for(0, &rule()), Zone::Dire);
    }

    #[test]
    fn zone_severity_order() {
        let mut z = vec![Zone::Unreachable, Zone::Healthy, Zone::Dire, Zone::Warning];
        z.sort();
        assert_eq!(
            z,
            vec![Zone::Dire, Zone::Warning, Zone::Healthy, Zone::Unreachable]
        );
    }

    #[test]
    fn reminder_interval_linear() {
        let r = rule();
        for (free, ms) in [
            (20 * GIB, 60_000),
            (2 * GIB, 10_000),
            (11 * GIB, 35_000),
            (50 * GIB, 60_000),
            (0, 10_000),
        ] {
            assert_eq!(reminder_interval_ms(free, &r, &t()), ms, "free {free}");
        }
    }

    #[test]
    fn first_sample_below_threshold_is_one_crossing() {
        let mut a = VolumeAlert::default();
        assert_eq!(obs(&mut a, 14.8), Some(AlertEvent::Crossing));
        assert_eq!(obs(&mut a, 14.7), None);
    }

    #[test]
    fn jitter_across_level_warns_once() {
        let mut a = VolumeAlert::default();
        let mut events = vec![obs(&mut a, 21.0)];
        for _ in 0..20 {
            events.push(obs(&mut a, 15.1));
            events.push(obs(&mut a, 14.9));
        }
        assert_eq!(events[0], None);
        assert_eq!(events[1], Some(AlertEvent::Crossing));
        assert_eq!(events[2], Some(AlertEvent::Crossing));
        assert_eq!(events.iter().flatten().count(), 2);
    }

    #[test]
    fn level_rearms_after_full_step() {
        let mut a = VolumeAlert::default();
        assert_eq!(obs(&mut a, 14.8), Some(AlertEvent::Crossing));
        assert_eq!(obs(&mut a, 20.1), None);
        assert_eq!(obs(&mut a, 14.9), Some(AlertEvent::Crossing));
    }

    #[test]
    fn level_does_not_rearm_short_of_full_step() {
        let mut a = VolumeAlert::default();
        assert_eq!(obs(&mut a, 14.8), Some(AlertEvent::Crossing));
        assert_eq!(obs(&mut a, 19.9), None);
        assert_eq!(obs(&mut a, 14.9), None);
    }

    #[test]
    fn threshold_rearms_only_a_step_above_it() {
        let mut a = VolumeAlert::default();
        assert_eq!(obs(&mut a, 19.0), Some(AlertEvent::Crossing));
        assert_eq!(obs(&mut a, 24.9), None);
        assert_eq!(obs(&mut a, 19.0), None);
        assert_eq!(obs(&mut a, 25.1), None);
        assert_eq!(obs(&mut a, 19.0), Some(AlertEvent::Crossing));
    }

    #[test]
    fn drop_through_two_levels_one_event() {
        let mut a = VolumeAlert::default();
        assert_eq!(obs(&mut a, 21.0), None);
        assert_eq!(obs(&mut a, 9.0), Some(AlertEvent::Crossing));
        assert_eq!(obs(&mut a, 9.5), None);
    }

    #[test]
    fn entering_dire_beats_crossing() {
        let mut a = VolumeAlert::default();
        assert_eq!(obs(&mut a, 6.0), Some(AlertEvent::Crossing));
        assert_eq!(obs(&mut a, 1.5), Some(AlertEvent::EnteredDire));
        assert_eq!(obs(&mut a, 2.5), None);
        assert_eq!(obs(&mut a, 1.9), None);
        assert_eq!(obs(&mut a, 7.1), None);
        assert_eq!(obs(&mut a, 1.9), Some(AlertEvent::EnteredDire));
    }

    #[test]
    fn crossing_breaks_snooze() {
        let mut a = VolumeAlert::default();
        obs(&mut a, 14.0);
        a.snooze(SnoozeChoice::NextLevel, 0);
        assert!(a.is_snoozed(0));
        assert_eq!(obs(&mut a, 9.9), Some(AlertEvent::Crossing));
        assert_eq!(a.snooze, Snooze::Off);
    }

    #[test]
    fn entering_dire_breaks_snooze() {
        let mut a = VolumeAlert::default();
        obs(&mut a, 4.0);
        a.snooze(SnoozeChoice::OneHour, 0);
        assert_eq!(obs(&mut a, 1.0), Some(AlertEvent::EnteredDire));
        assert_eq!(a.snooze, Snooze::Off);
    }

    #[test]
    fn dire_next_level_snooze_becomes_timed() {
        let mut a = VolumeAlert::default();
        obs(&mut a, 1.0);
        a.snooze(SnoozeChoice::NextLevel, 0);
        assert_eq!(
            a.snooze,
            Snooze::Until {
                at_ms: 600_000,
                choice: SnoozeChoice::TenMin
            }
        );
    }

    #[test]
    fn timed_snooze_expiry_triggers_reminder() {
        let mut a = VolumeAlert::default();
        obs(&mut a, 11.0);
        a.snooze(SnoozeChoice::TenMin, 0);
        assert_eq!(a.tick(&rule(), 599_999, &t()), None);
        assert_eq!(a.tick(&rule(), 600_000, &t()), Some(AlertEvent::Reminder));
        assert!(!a.is_snoozed(600_000));
    }

    #[test]
    fn reminder_due_after_interval() {
        let mut a = VolumeAlert::default();
        assert_eq!(obs(&mut a, 11.0), Some(AlertEvent::Crossing));
        a.reminder_shown(&rule(), 0, &t());
        assert_eq!(a.tick(&rule(), 34_999, &t()), None);
        assert_eq!(a.tick(&rule(), 35_000, &t()), Some(AlertEvent::Reminder));
    }

    #[test]
    fn reminder_reschedules_itself() {
        let mut a = VolumeAlert::default();
        obs(&mut a, 11.0);
        assert_eq!(a.tick(&rule(), 35_000, &t()), Some(AlertEvent::Reminder));
        assert_eq!(a.tick(&rule(), 35_001, &t()), None);
        assert_eq!(a.tick(&rule(), 70_000, &t()), Some(AlertEvent::Reminder));
    }

    #[test]
    fn snoozed_gets_no_reminder() {
        let mut a = VolumeAlert::default();
        obs(&mut a, 11.0);
        a.snooze(SnoozeChoice::NextLevel, 0);
        assert_eq!(a.tick(&rule(), 1_000_000, &t()), None);
    }

    #[test]
    fn dire_gets_no_reminder() {
        let mut a = VolumeAlert::default();
        obs(&mut a, 1.0);
        assert_eq!(a.tick(&rule(), 10_000_000, &t()), None);
    }

    #[test]
    fn wake_clears_snooze() {
        let mut a = VolumeAlert::default();
        obs(&mut a, 11.0);
        a.snooze(SnoozeChoice::OneHour, 0);
        a.wake(100);
        assert!(!a.is_snoozed(100));
        assert_eq!(a.tick(&rule(), 35_100, &t()), Some(AlertEvent::Reminder));
    }

    #[test]
    fn unreachable_transition_once() {
        let r = rule();
        let mut a = VolumeAlert::default();
        a.observe(Sample::Free(gb(50.0)), &r, 0, &t());
        let u = |a: &mut VolumeAlert| a.observe(Sample::Unreachable, &r, 0, &t());
        assert_eq!(u(&mut a), Some(AlertEvent::BecameUnreachable));
        assert_eq!(a.zone, Zone::Unreachable);
        assert_eq!(a.free, None);
        assert_eq!(u(&mut a), None);
        a.observe(Sample::Free(gb(50.0)), &r, 0, &t());
        assert_eq!(u(&mut a), Some(AlertEvent::BecameUnreachable));
    }

    #[test]
    fn debounce_survives_unreachable_period() {
        let mut a = VolumeAlert::default();
        assert_eq!(obs(&mut a, 14.0), Some(AlertEvent::Crossing));
        a.observe(Sample::Unreachable, &rule(), 0, &t());
        assert_eq!(obs(&mut a, 14.0), None);
    }

    #[test]
    fn unreachable_drops_from_minimized_after_300s() {
        let mut a = VolumeAlert::default();
        let since = 1_000;
        a.observe(Sample::Unreachable, &rule(), since, &t());
        assert!(a.show_in_minimized(since + 299_999, &t()));
        assert!(!a.show_in_minimized(since + 300_000, &t()));
    }

    #[test]
    fn minimized_shows_unsnoozed_at_risk_only() {
        let mut a = VolumeAlert::default();
        obs(&mut a, 50.0);
        assert!(!a.show_in_minimized(0, &t()));
        obs(&mut a, 11.0);
        assert!(a.is_at_risk());
        assert!(a.show_in_minimized(0, &t()));
        a.snooze(SnoozeChoice::TenMin, 0);
        assert!(!a.show_in_minimized(0, &t()));
    }

    #[test]
    fn poll_interval_by_zone() {
        let r = rule();
        let mut a = VolumeAlert::default();
        obs(&mut a, 50.0);
        assert_eq!(a.poll_interval_ms(&r, 60, &t()), 60_000);
        obs(&mut a, 11.0);
        assert_eq!(a.poll_interval_ms(&r, 60, &t()), 35_000);
        assert_eq!(a.poll_interval_ms(&r, 5, &t()), 5_000);
        obs(&mut a, 1.0);
        assert_eq!(a.poll_interval_ms(&r, 60, &t()), 10_000);
    }

    #[test]
    fn zero_step_rule_does_not_hang() {
        let r = Rule { step: 0, ..rule() };
        let mut a = VolumeAlert::default();
        assert_eq!(levels(&r), vec![20 * GIB]);
        assert_eq!(
            a.observe(Sample::Free(gb(14.0)), &r, 0, &t()),
            Some(AlertEvent::Crossing)
        );
        assert_eq!(a.observe(Sample::Free(gb(30.0)), &r, 0, &t()), None);
        assert_eq!(
            a.observe(Sample::Free(gb(14.0)), &r, 0, &t()),
            Some(AlertEvent::Crossing)
        );
    }

    #[test]
    fn huge_values_do_not_overflow() {
        let r = Rule {
            threshold: u64::MAX - 1,
            step: u64::MAX / 2,
            dire: 0,
        };
        let big = Timings {
            reminder_max_secs: u64::MAX,
            reminder_min_secs: u64::MAX,
            unreachable_drop_secs: u64::MAX,
            ..Timings::default()
        };
        let now = u64::MAX - 10;
        let mut a = VolumeAlert::default();
        a.observe(Sample::Free(u64::MAX - 5), &r, now, &big);
        a.observe(Sample::Free(u64::MAX / 2 + 1), &r, now, &big);
        a.observe(Sample::Free(u64::MAX), &r, now, &big);
        a.observe(Sample::Free(1), &r, now, &big);
        a.tick(&r, u64::MAX, &big);
        a.snooze(SnoozeChoice::OneHour, now);
        a.wake(now);
        a.reminder_shown(&r, now, &big);
        let _ = a.poll_interval_ms(&r, u64::MAX, &big);
        let _ = reminder_interval_ms(1, &r, &big);
        a.observe(Sample::Unreachable, &r, 0, &big);
        assert!(a.show_in_minimized(1_000, &big));
    }

    #[test]
    fn dire_reentry_with_next_level_snooze_becomes_timed() {
        let mut a = VolumeAlert::default();
        obs(&mut a, 6.0);
        obs(&mut a, 1.5);
        obs(&mut a, 2.5);
        a.snooze(SnoozeChoice::NextLevel, 0);
        assert_eq!(a.snooze, Snooze::NextLevel);
        assert_eq!(a.observe(Sample::Free(gb(1.9)), &rule(), 1_000, &t()), None);
        assert_eq!(
            a.snooze,
            Snooze::Until {
                at_ms: 601_000,
                choice: SnoozeChoice::TenMin
            }
        );
    }

    #[test]
    fn break_through_reschedules_reminder() {
        let mut a = VolumeAlert::default();
        assert_eq!(obs(&mut a, 11.0), Some(AlertEvent::Crossing));
        let ev = a.observe(Sample::Free(gb(9.0)), &rule(), 30_000, &t());
        assert_eq!(ev, Some(AlertEvent::Crossing));
        let next = 30_000 + reminder_interval_ms(gb(9.0), &rule(), &t());
        assert_eq!(a.tick(&rule(), 35_000, &t()), None);
        assert_eq!(a.tick(&rule(), next, &t()), Some(AlertEvent::Reminder));
    }

    #[test]
    fn arithmetic_levels_match_levels_vec() {
        let rules = [
            rule(),
            Rule {
                dire: 5 * GIB,
                ..rule()
            },
            Rule {
                threshold: 21 * GIB,
                ..rule()
            },
            Rule {
                threshold: 7,
                step: 3,
                dire: 0,
            },
            Rule {
                threshold: 10,
                step: 20,
                dire: 1,
            },
        ];
        for r in rules {
            let lv = levels(&r);
            assert_eq!(lowest_level(&r), lv.last().copied(), "{r:?}");
            for free in (r.dire + 1)..=r.threshold.min(r.dire + 40) {
                let want = lv.iter().copied().rfind(|&l| free <= l);
                assert_eq!(level_at(free, &r), want, "{r:?} free {free}");
            }
            for free in [r.threshold, r.threshold - 1, r.dire + 1] {
                let want = lv.iter().copied().rfind(|&l| free <= l);
                assert_eq!(level_at(free, &r), want, "{r:?} free {free}");
            }
        }
    }
}
