//! Surface view model: turns per-volume alert state into what the card
//! shows — sorted rows with value text, bar fill and level gaps, plus the
//! header title, state dot and halo flag. Pure; the painter and layout
//! consume [`SurfaceView`] and never look at alert state directly.

use crate::alerts::{Snooze, VolumeAlert, Zone, levels};
use crate::config::{GIB, Mode, Rule, SnoozeChoice, Timings, VolumeId};

/// One volume's inputs to [`build_view`].
#[derive(Clone, Copy, Debug)]
pub struct RowInput<'a> {
    pub id: &'a VolumeId,
    /// Drive letter column, e.g. `"C:"` (empty for mounts/shares).
    pub letter: &'a str,
    pub label: &'a str,
    pub rule: Rule,
    pub alert: &'a VolumeAlert,
}

/// How a row is styled (handoff "Row states").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Healthy,
    Warning,
    Dire,
    Unreachable,
    /// At-risk and snoozed; the choice drives the `Zz …` value text.
    Snoozed(SnoozeChoice),
}

/// One painted row.
#[derive(Clone, Debug, PartialEq)]
pub struct RowView {
    pub id: VolumeId,
    pub letter: String,
    pub label: String,
    /// `"14.8 GB"`, `"?"`, `"Zz 10 min"`, or `"—"` before the first sample.
    pub value: String,
    pub state: RowState,
    /// Runway bar fill, `free / threshold` clamped to 0..=1.
    pub fill: f32,
    /// Level-gap positions as fractions of the bar width.
    pub gaps: Vec<f32>,
    /// Zz button drawn (warning and dire rows, snoozed or not).
    pub show_snooze_btn: bool,
    /// Zz button drawn inverted (row is snoozed).
    pub snooze_active: bool,
}

/// Header state-dot colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    DireAccent,
    Warning,
    Muted,
}

/// Everything the card paints.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceView {
    pub title: String,
    pub dot: Dot,
    /// Every at-risk volume is snoozed (header Zz drawn inverted).
    pub snooze_all_active: bool,
    pub rows: Vec<RowView>,
    /// A non-snoozed volume is dire.
    pub halo: bool,
    pub mode: Mode,
}

const MIB: f64 = (1u64 << 20) as f64;

/// Free-space text: `"850 MB"` below 1 GB, `"14.8 GB"` below 100 GB,
/// `"245 GB"` below 1000 GB, else `"1.2 TB"` (binary units, as Explorer).
/// Range checks use the rounded value so `99.96 GB` reads `"100 GB"`.
pub fn format_free(bytes: u64) -> String {
    let mb = bytes as f64 / MIB;
    if mb.round() < 1024.0 {
        return format!("{mb:.0} MB");
    }
    let gb = bytes as f64 / GIB as f64;
    if (gb * 10.0).round() < 1000.0 {
        format!("{gb:.1} GB")
    } else if gb.round() < 1000.0 {
        format!("{gb:.0} GB")
    } else {
        format!("{:.1} TB", gb / 1024.0)
    }
}

fn fill_for(free: u64, rule: &Rule) -> f32 {
    if rule.threshold == 0 {
        return if free > 0 { 1.0 } else { 0.0 };
    }
    (free as f64 / rule.threshold as f64).clamp(0.0, 1.0) as f32
}

fn gaps_for(rule: &Rule) -> Vec<f32> {
    levels(rule)
        .into_iter()
        .filter(|&l| l != rule.threshold)
        .map(|l| (l as f64 / rule.threshold as f64) as f32)
        .collect()
}

/// The snooze choice in effect for an at-risk row, if snoozed at `now`.
fn snoozed_choice(a: &VolumeAlert, now: u64) -> Option<SnoozeChoice> {
    if !a.is_at_risk() || !a.is_snoozed(now) {
        return None;
    }
    Some(match a.snooze {
        Snooze::Until { choice, .. } => choice,
        Snooze::NextLevel | Snooze::Off => SnoozeChoice::NextLevel,
    })
}

fn row_view(r: &RowInput, level_gaps: bool, now: u64) -> RowView {
    let a = r.alert;
    let snoozed = snoozed_choice(a, now);
    let (state, value, fill, gaps) = match (snoozed, a.zone, a.free) {
        (Some(c), _, _) => (
            RowState::Snoozed(c),
            format!("Zz {}", c.label()),
            0.0,
            Vec::new(),
        ),
        (None, Zone::Unreachable, _) => (RowState::Unreachable, "?".into(), 0.0, Vec::new()),
        (None, _, None) => (RowState::Healthy, "—".into(), 0.0, Vec::new()),
        (None, zone, Some(free)) => {
            let state = match zone {
                Zone::Dire => RowState::Dire,
                Zone::Warning => RowState::Warning,
                _ => RowState::Healthy,
            };
            let gaps = if level_gaps {
                gaps_for(&r.rule)
            } else {
                Vec::new()
            };
            (state, format_free(free), fill_for(free, &r.rule), gaps)
        }
    };
    RowView {
        id: r.id.clone(),
        letter: r.letter.to_owned(),
        label: r.label.to_owned(),
        value,
        state,
        fill,
        gaps,
        show_snooze_btn: a.is_at_risk(),
        snooze_active: snoozed.is_some(),
    }
}

/// Builds the card's view. Rows sort worst zone first (snoozed rows by
/// their underlying zone), then by free space ascending, then label.
/// `snooze_label` is the configured duration for the all-snoozed title.
pub fn build_view(
    rows: &[RowInput],
    mode: Mode,
    snooze_label: &str,
    level_gaps: bool,
    now: u64,
    t: &Timings,
) -> SurfaceView {
    let mut sorted: Vec<&RowInput> = rows.iter().collect();
    sorted.sort_by(|a, b| {
        a.alert
            .zone
            .cmp(&b.alert.zone)
            .then(
                a.alert
                    .free
                    .unwrap_or(u64::MAX)
                    .cmp(&b.alert.free.unwrap_or(u64::MAX)),
            )
            .then(a.label.cmp(b.label))
    });

    let at_risk: Vec<&VolumeAlert> = rows
        .iter()
        .map(|r| r.alert)
        .filter(|a| a.is_at_risk())
        .collect();
    let awake: Vec<&&VolumeAlert> = at_risk.iter().filter(|a| !a.is_snoozed(now)).collect();
    let all_snoozed = !at_risk.is_empty() && awake.is_empty();
    let worst = awake.iter().map(|a| a.zone).min();

    let title = if all_snoozed {
        format!("Snoozed · {snooze_label}")
    } else {
        "Disk space".to_owned()
    };
    let dot = match worst {
        Some(Zone::Dire) => Dot::DireAccent,
        Some(Zone::Warning) => Dot::Warning,
        _ => Dot::Muted,
    };
    let rows = sorted
        .into_iter()
        .filter(|r| mode == Mode::Expanded || r.alert.show_in_minimized(now, t))
        .map(|r| row_view(r, level_gaps, now))
        .collect();

    SurfaceView {
        title,
        dot,
        snooze_all_active: all_snoozed,
        rows,
        halo: worst == Some(Zone::Dire),
        mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::Sample;

    const NOW: u64 = 1_000_000;

    fn rule() -> Rule {
        Rule {
            threshold: 20 * GIB,
            step: 5 * GIB,
            dire: 2 * GIB,
        }
    }

    fn alert(free: Option<u64>) -> VolumeAlert {
        let mut a = VolumeAlert::default();
        if let Some(f) = free {
            a.observe(Sample::Free(f), &rule(), 0, &Timings::default());
        }
        a
    }

    fn unreachable() -> VolumeAlert {
        let mut a = alert(Some(50 * GIB));
        a.observe(Sample::Unreachable, &rule(), NOW, &Timings::default());
        a
    }

    fn snoozed(free: u64, c: SnoozeChoice) -> VolumeAlert {
        let mut a = alert(Some(free));
        a.snooze(c, NOW);
        a
    }

    struct Fixture {
        ids: Vec<VolumeId>,
        labels: Vec<String>,
        alerts: Vec<VolumeAlert>,
    }

    impl Fixture {
        fn new(alerts: Vec<VolumeAlert>) -> Fixture {
            let n = alerts.len();
            Fixture {
                ids: (0..n).map(|i| VolumeId(format!("v{i}"))).collect(),
                labels: (0..n).map(|i| format!("L{i}")).collect(),
                alerts,
            }
        }

        fn inputs(&self) -> Vec<RowInput<'_>> {
            (0..self.alerts.len())
                .map(|i| RowInput {
                    id: &self.ids[i],
                    letter: "C:",
                    label: &self.labels[i],
                    rule: rule(),
                    alert: &self.alerts[i],
                })
                .collect()
        }

        fn view(&self, mode: Mode) -> SurfaceView {
            build_view(
                &self.inputs(),
                mode,
                "10 min",
                true,
                NOW,
                &Timings::default(),
            )
        }
    }

    fn gb(x: f64) -> u64 {
        (x * GIB as f64) as u64
    }

    #[test]
    fn format_free_ranges() {
        assert_eq!(format_free(0), "0 MB");
        assert_eq!(format_free(850 << 20), "850 MB");
        assert_eq!(format_free(gb(14.8)), "14.8 GB");
        assert_eq!(format_free(245 * GIB), "245 GB");
        assert_eq!(format_free(1229 * GIB), "1.2 TB");
    }

    #[test]
    fn format_free_boundaries_use_rounded_value() {
        assert_eq!(format_free(GIB - 1), "1.0 GB");
        assert_eq!(format_free(gb(99.97)), "100 GB");
        assert_eq!(format_free(gb(999.7)), "1.0 TB");
    }

    #[test]
    fn bar_fill_clamps() {
        let f = |free| fill_for(free, &rule());
        assert!((f(gb(14.8)) - 0.74).abs() < 1e-4);
        assert_eq!(f(50 * GIB), 1.0);
        assert_eq!(f(0), 0.0);
    }

    #[test]
    fn gaps_from_rule() {
        let g = gaps_for(&rule());
        assert_eq!(g, vec![0.75, 0.5, 0.25]);
        let r = Rule {
            threshold: 30 * GIB,
            step: 10 * GIB,
            dire: 2 * GIB,
        };
        let g = gaps_for(&r);
        assert_eq!(g.len(), 2);
        assert!((g[0] - 0.6667).abs() < 1e-3);
        assert!((g[1] - 0.3333).abs() < 1e-3);

        let fx = Fixture::new(vec![alert(Some(10 * GIB))]);
        let v = build_view(
            &fx.inputs(),
            Mode::Expanded,
            "10 min",
            false,
            NOW,
            &Timings::default(),
        );
        assert!(v.rows[0].gaps.is_empty());
        assert_eq!(fx.view(Mode::Expanded).rows[0].gaps.len(), 3);
    }

    #[test]
    fn rows_sorted_worst_first() {
        let fx = Fixture::new(vec![
            unreachable(),                           // v0
            alert(Some(80 * GIB)),                   // v1 healthy
            alert(Some(15 * GIB)),                   // v2 warning
            alert(Some(GIB)),                        // v3 dire
            alert(Some(12 * GIB)),                   // v4 warning, less free
            alert(Some(30 * GIB)),                   // v5 healthy, less free
            snoozed(GIB / 2, SnoozeChoice::OneHour), // v6 dire, snoozed
        ]);
        let order: Vec<String> = fx
            .view(Mode::Expanded)
            .rows
            .into_iter()
            .map(|r| r.id.0)
            .collect();
        assert_eq!(order, ["v6", "v3", "v4", "v2", "v5", "v1", "v0"]);
    }

    #[test]
    fn ties_break_by_label() {
        let mut fx = Fixture::new(vec![alert(Some(15 * GIB)), alert(Some(15 * GIB))]);
        fx.labels = vec!["b".into(), "a".into()];
        let rows = fx.view(Mode::Expanded).rows;
        assert_eq!(rows[0].label, "a");
    }

    #[test]
    fn row_values_and_states() {
        let fx = Fixture::new(vec![
            alert(Some(gb(14.8))),
            unreachable(),
            alert(None),
            snoozed(15 * GIB, SnoozeChoice::TenMin),
            snoozed(15 * GIB, SnoozeChoice::NextLevel),
        ]);
        let v = fx.view(Mode::Expanded);
        let by_id = |id: &str| v.rows.iter().find(|r| r.id.0 == id).unwrap().clone();

        let w = by_id("v0");
        assert_eq!((w.value.as_str(), w.state), ("14.8 GB", RowState::Warning));
        let u = by_id("v1");
        assert_eq!((u.value.as_str(), u.state), ("?", RowState::Unreachable));
        assert_eq!((u.fill, u.gaps.len()), (0.0, 0));
        let n = by_id("v2");
        assert_eq!((n.value.as_str(), n.state), ("—", RowState::Healthy));
        assert_eq!((n.fill, n.gaps.len()), (0.0, 0));
        let s = by_id("v3");
        assert_eq!(s.value, "Zz 10 min");
        assert_eq!(s.state, RowState::Snoozed(SnoozeChoice::TenMin));
        assert!(s.snooze_active && s.gaps.is_empty());
        let nl = by_id("v4");
        assert_eq!(nl.value, "Zz next level");
        assert_eq!(nl.state, RowState::Snoozed(SnoozeChoice::NextLevel));
    }

    #[test]
    fn minimized_hides_healthy_and_snoozed() {
        let fx = Fixture::new(vec![
            alert(Some(80 * GIB)),
            alert(Some(15 * GIB)),
            snoozed(12 * GIB, SnoozeChoice::TenMin),
            alert(None),
        ]);
        let v = fx.view(Mode::Minimized);
        let ids: Vec<&str> = v.rows.iter().map(|r| r.id.0.as_str()).collect();
        assert_eq!(ids, ["v1"]);
        assert_eq!(v.mode, Mode::Minimized);
        assert_eq!(fx.view(Mode::Expanded).rows.len(), 4);
    }

    #[test]
    fn title_when_all_snoozed() {
        let fx = Fixture::new(vec![
            alert(Some(80 * GIB)),
            snoozed(15 * GIB, SnoozeChoice::OneHour),
        ]);
        let v = build_view(
            &fx.inputs(),
            Mode::Minimized,
            "1 h",
            true,
            NOW,
            &Timings::default(),
        );
        assert_eq!(v.title, "Snoozed · 1 h");
        assert!(v.snooze_all_active);
        assert!(v.rows.is_empty(), "minimized + all snoozed → header only");

        let partly = Fixture::new(vec![
            alert(Some(15 * GIB)),
            snoozed(12 * GIB, SnoozeChoice::OneHour),
        ]);
        let v = partly.view(Mode::Expanded);
        assert_eq!(v.title, "Disk space");
        assert!(!v.snooze_all_active);

        let none_at_risk = Fixture::new(vec![alert(Some(80 * GIB))]);
        let v = none_at_risk.view(Mode::Expanded);
        assert_eq!(v.title, "Disk space");
        assert!(!v.snooze_all_active);
    }

    #[test]
    fn dot_ignores_snoozed() {
        let fx = Fixture::new(vec![
            snoozed(GIB, SnoozeChoice::TenMin),
            alert(Some(15 * GIB)),
        ]);
        assert_eq!(fx.view(Mode::Expanded).dot, Dot::Warning);
        let fx = Fixture::new(vec![snoozed(GIB, SnoozeChoice::TenMin)]);
        assert_eq!(fx.view(Mode::Expanded).dot, Dot::Muted);
        let fx = Fixture::new(vec![alert(Some(GIB)), alert(Some(15 * GIB))]);
        assert_eq!(fx.view(Mode::Expanded).dot, Dot::DireAccent);
    }

    #[test]
    fn halo_only_for_unsnoozed_dire() {
        assert!(
            Fixture::new(vec![alert(Some(GIB))])
                .view(Mode::Expanded)
                .halo
        );
        let snoozed_dire = Fixture::new(vec![snoozed(GIB, SnoozeChoice::TenMin)]);
        assert!(!snoozed_dire.view(Mode::Expanded).halo);
        assert!(
            !Fixture::new(vec![alert(Some(15 * GIB))])
                .view(Mode::Expanded)
                .halo
        );
    }

    #[test]
    fn expired_snooze_reads_as_awake() {
        let fx = Fixture::new(vec![snoozed(15 * GIB, SnoozeChoice::TenMin)]);
        let later = NOW + 10 * 60_000;
        let v = build_view(
            &fx.inputs(),
            Mode::Expanded,
            "10 min",
            true,
            later,
            &Timings::default(),
        );
        assert_eq!(v.rows[0].state, RowState::Warning);
        assert_eq!(v.title, "Disk space");
    }

    #[test]
    fn snooze_button_only_on_at_risk_rows() {
        let fx = Fixture::new(vec![
            alert(Some(80 * GIB)),
            alert(Some(15 * GIB)),
            alert(Some(GIB)),
            unreachable(),
            snoozed(15 * GIB, SnoozeChoice::TenMin),
        ]);
        let v = fx.view(Mode::Expanded);
        let btn = |id: &str| {
            let r = v.rows.iter().find(|r| r.id.0 == id).unwrap();
            (r.show_snooze_btn, r.snooze_active)
        };
        assert_eq!(btn("v0"), (false, false));
        assert_eq!(btn("v1"), (true, false));
        assert_eq!(btn("v2"), (true, false));
        assert_eq!(btn("v3"), (false, false));
        assert_eq!(btn("v4"), (true, true));
    }
}
