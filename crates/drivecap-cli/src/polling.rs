//! The watched-volume collection: per-volume alert state plus poll
//! scheduling, reconciled against each volume scan. Pure — the
//! [`crate::controller`] drives it and turns its results into effects.
//!
//! - **Reconcile:** a rescan keeps the alert state of every id still
//!   present (so a USB drive that comes back on a new letter is not
//!   re-warned), adds new ids due for an immediate poll, and drops ids
//!   that vanished — samples for them are then ignored.
//! - **Scheduling:** a volume is due when `next_poll_at <= now`. Taking it
//!   provisionally schedules the next poll one interval out (so a slow
//!   network probe is not re-requested every tick); its sample then
//!   reschedules from the sample time on the zone's cadence
//!   ([`VolumeAlert::poll_interval_ms`]).
//! - **Snooze:** only at-risk volumes are snoozed; waking is always allowed.

use crate::alerts::{AlertEvent, Sample, VolumeAlert, Zone};
use crate::config::{Config, Rule, SnoozeChoice, Timings, VolumeId};
use crate::volumes::VolumeInfo;
use std::collections::BTreeMap;

/// One watched volume.
#[derive(Clone, Debug)]
pub struct VolState {
    pub info: VolumeInfo,
    pub rule: Rule,
    pub alert: VolumeAlert,
    /// Unix ms at which the volume is next due for a poll.
    pub next_poll_at: u64,
}

impl VolState {
    fn poll_interval_ms(&self, cfg: &Config) -> u64 {
        self.alert
            .poll_interval_ms(&self.rule, cfg.poll_interval_secs, &cfg.timings)
    }
}

/// Watched volumes keyed by id.
#[derive(Clone, Debug, Default)]
pub struct Volumes {
    map: BTreeMap<VolumeId, VolState>,
}

impl Volumes {
    /// Applies a scan result (see the module docs).
    pub fn reconcile(&mut self, selected: Vec<VolumeInfo>, cfg: &Config, now: u64) {
        let mut next = BTreeMap::new();
        for info in selected {
            let rule = cfg.rule_for(&info.id);
            let state = match self.map.remove(&info.id) {
                Some(old) => VolState { info, rule, ..old },
                None => VolState {
                    info,
                    rule,
                    alert: VolumeAlert::default(),
                    next_poll_at: now,
                },
            };
            next.insert(state.info.id.clone(), state);
        }
        self.map = next;
    }

    /// Recomputes every volume's rule from `cfg` (config reload) and makes
    /// each due now, so the new rule applies on the next tick rather than
    /// up to a full poll interval later.
    pub fn apply_rules(&mut self, cfg: &Config, now: u64) {
        for v in self.map.values_mut() {
            v.rule = cfg.rule_for(&v.info.id);
            v.next_poll_at = now;
        }
    }

    /// Volumes in id order.
    pub fn iter(&self) -> impl Iterator<Item = &VolState> {
        self.map.values()
    }

    pub fn get(&self, id: &VolumeId) -> Option<&VolState> {
        self.map.get(id)
    }

    pub fn contains(&self, id: &VolumeId) -> bool {
        self.map.contains_key(id)
    }

    pub fn remove(&mut self, id: &VolumeId) {
        self.map.remove(id);
    }

    /// Volumes due at `now`; each is provisionally rescheduled one interval out.
    pub fn take_due(&mut self, cfg: &Config, now: u64) -> Vec<VolumeInfo> {
        let mut due = Vec::new();
        for v in self.map.values_mut() {
            if v.next_poll_at <= now {
                v.next_poll_at = now.saturating_add(v.poll_interval_ms(cfg));
                due.push(v.info.clone());
            }
        }
        due
    }

    /// Records a sample and reschedules the volume's next poll. Unknown
    /// ids (dropped by a rescan) are ignored.
    pub fn observe(
        &mut self,
        id: &VolumeId,
        s: Sample,
        cfg: &Config,
        now: u64,
    ) -> Option<AlertEvent> {
        let v = self.map.get_mut(id)?;
        let ev = v.alert.observe(s, &v.rule, now, &cfg.timings);
        v.next_poll_at = now.saturating_add(v.poll_interval_ms(cfg));
        ev
    }

    /// Runs every volume's alert tick (snooze expiry, reminders).
    pub fn tick_alerts(&mut self, t: &Timings, now: u64) -> Vec<AlertEvent> {
        self.map
            .values_mut()
            .filter_map(|v| v.alert.tick(&v.rule, now, t))
            .collect()
    }

    /// The card was shown: push every warning volume's reminder out.
    pub fn reminders_shown(&mut self, t: &Timings, now: u64) {
        for v in self.map.values_mut() {
            if v.alert.zone == Zone::Warning {
                v.alert.reminder_shown(&v.rule, now, t);
            }
        }
    }

    /// Snoozes one at-risk volume with `choice`.
    pub fn snooze(&mut self, id: &VolumeId, choice: SnoozeChoice, now: u64) {
        if let Some(v) = self.map.get_mut(id)
            && v.alert.is_at_risk()
        {
            v.alert.snooze(choice, now);
        }
    }

    /// Row Zz: wakes a snoozed volume, else snoozes an at-risk one.
    pub fn toggle_snooze(&mut self, id: &VolumeId, choice: SnoozeChoice, now: u64) {
        match self.map.get_mut(id) {
            Some(v) if v.alert.is_snoozed(now) => v.alert.wake(now),
            Some(_) => self.snooze(id, choice, now),
            None => {}
        }
    }

    /// Snoozes every awake at-risk volume with `choice`.
    pub fn snooze_all(&mut self, choice: SnoozeChoice, now: u64) {
        for v in self.map.values_mut() {
            if v.alert.is_at_risk() && !v.alert.is_snoozed(now) {
                v.alert.snooze(choice, now);
            }
        }
    }

    /// Header Zz: wakes every at-risk volume if all are snoozed, else
    /// snoozes the awake ones.
    pub fn toggle_snooze_all(&mut self, choice: SnoozeChoice, now: u64) {
        let mut at_risk = self
            .map
            .values()
            .filter(|v| v.alert.is_at_risk())
            .peekable();
        let any = at_risk.peek().is_some();
        if any && at_risk.all(|v| v.alert.is_snoozed(now)) {
            for v in self.map.values_mut().filter(|v| v.alert.is_at_risk()) {
                v.alert.wake(now);
            }
        } else {
            self.snooze_all(choice, now);
        }
    }

    /// A non-snoozed volume is dire (the card is pinned).
    pub fn any_awake_dire(&self, now: u64) -> bool {
        self.map
            .values()
            .any(|v| v.alert.zone == Zone::Dire && !v.alert.is_snoozed(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::Snooze;
    use crate::config::GIB;
    use crate::volumes::VolumeKind;

    const T0: u64 = 1_000_000;

    fn info(id: &str, letter: &str) -> VolumeInfo {
        VolumeInfo {
            id: VolumeId(id.into()),
            kind: VolumeKind::Local,
            root: format!("{letter}\\"),
            letter: letter.into(),
            label: "Disk".into(),
            removable: false,
        }
    }

    fn vid(s: &str) -> VolumeId {
        VolumeId(s.into())
    }

    fn scanned(ids: &[(&str, &str)]) -> Volumes {
        let mut v = Volumes::default();
        let sel = ids.iter().map(|(i, l)| info(i, l)).collect();
        v.reconcile(sel, &Config::default(), T0);
        v
    }

    #[test]
    fn new_volumes_are_due_immediately() {
        let mut v = scanned(&[("a", "C:"), ("b", "D:")]);
        let due: Vec<_> = v.take_due(&Config::default(), T0);
        assert_eq!(due.len(), 2);
        assert!(v.take_due(&Config::default(), T0 + 1).is_empty());
    }

    #[test]
    fn sample_reschedules_on_zone_cadence() {
        let cfg = Config::default();
        let mut v = scanned(&[("a", "C:")]);
        v.observe(&vid("a"), Sample::Free(50 * GIB), &cfg, T0);
        assert_eq!(v.get(&vid("a")).unwrap().next_poll_at, T0 + 60_000);
        v.observe(&vid("a"), Sample::Free(GIB), &cfg, T0);
        assert_eq!(v.get(&vid("a")).unwrap().next_poll_at, T0 + 10_000);
    }

    #[test]
    fn reconcile_keeps_state_updates_info_and_drops_vanished() {
        let cfg = Config::default();
        let mut v = scanned(&[("a", "C:"), ("b", "D:")]);
        v.observe(&vid("a"), Sample::Free(14 * GIB), &cfg, T0);
        v.reconcile(vec![info("a", "E:")], &cfg, T0 + 5);
        let a = v.get(&vid("a")).unwrap();
        assert_eq!(a.info.letter, "E:");
        assert_eq!(a.alert.free, Some(14 * GIB));
        assert!(!v.contains(&vid("b")));
        assert_eq!(v.observe(&vid("b"), Sample::Free(GIB), &cfg, T0), None);
    }

    #[test]
    fn healthy_volume_is_never_snoozed() {
        let cfg = Config::default();
        let mut v = scanned(&[("a", "C:")]);
        v.observe(&vid("a"), Sample::Free(50 * GIB), &cfg, T0);
        v.toggle_snooze(&vid("a"), SnoozeChoice::TenMin, T0);
        v.snooze_all(SnoozeChoice::TenMin, T0);
        assert_eq!(v.get(&vid("a")).unwrap().alert.snooze, Snooze::Off);
    }

    #[test]
    fn toggle_snooze_all_snoozes_then_wakes_only_at_risk() {
        let cfg = Config::default();
        let mut v = scanned(&[("a", "C:"), ("b", "D:")]);
        v.observe(&vid("a"), Sample::Free(11 * GIB), &cfg, T0);
        v.observe(&vid("b"), Sample::Free(50 * GIB), &cfg, T0);
        v.toggle_snooze_all(SnoozeChoice::OneHour, T0);
        assert!(v.get(&vid("a")).unwrap().alert.is_snoozed(T0));
        assert!(!v.get(&vid("b")).unwrap().alert.is_snoozed(T0));
        v.toggle_snooze_all(SnoozeChoice::OneHour, T0 + 1);
        assert!(!v.get(&vid("a")).unwrap().alert.is_snoozed(T0 + 1));
    }

    #[test]
    fn any_awake_dire_ignores_snoozed() {
        let cfg = Config::default();
        let mut v = scanned(&[("a", "C:")]);
        v.observe(&vid("a"), Sample::Free(GIB), &cfg, T0);
        assert!(v.any_awake_dire(T0));
        v.toggle_snooze(&vid("a"), SnoozeChoice::TenMin, T0);
        assert!(!v.any_awake_dire(T0));
    }
}
