//! Card position: per-monitor persistence (`~/.drivecap/position.json`),
//! work-area clamping and resize anchoring. Pure — the card window
//! enumerates monitors into [`WorkArea`]s and does the file I/O.
//!
//! The store keeps, per monitor device name (`szDevice`, e.g.
//! `\\.\DISPLAY1`), the card's top-left offset from that monitor's
//! work-area origin in physical px, plus which monitor the card was on
//! last. Schema (camelCase):
//! ```json
//! { "lastMonitor": "\\\\.\\DISPLAY2", "perMonitor": { "\\\\.\\DISPLAY2": [180, 50] } }
//! ```

use crate::layout::Rect;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One monitor's work area (taskbar excluded), physical px.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkArea {
    /// `MONITORINFOEXW::szDevice`.
    pub monitor: String,
    pub rect: Rect,
    pub primary: bool,
}

/// Saved card positions, one per monitor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PositionStore {
    last_monitor: Option<String>,
    per_monitor: BTreeMap<String, (i32, i32)>,
}

impl PositionStore {
    pub fn from_json_str(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned())
    }

    /// Records the card's top-left `(card_x, card_y)` against the monitor
    /// containing the card's centre (nearest monitor if the centre is off
    /// every work area). No-op without work areas.
    ///
    /// Takes the card size, beyond the plan's `(x, y, areas)`, because the
    /// centre cannot be found from the corner alone.
    pub fn record(
        &mut self,
        card_x: i32,
        card_y: i32,
        card_w: i32,
        card_h: i32,
        areas: &[WorkArea],
    ) {
        let (cx, cy) = (card_x + card_w / 2, card_y + card_h / 2);
        let Some(area) = areas
            .iter()
            .find(|a| a.rect.contains(cx, cy))
            .or_else(|| areas.iter().min_by_key(|a| distance_sq(a.rect, cx, cy)))
        else {
            return;
        };
        let offset = (card_x - area.rect.x, card_y - area.rect.y);
        self.per_monitor.insert(area.monitor.clone(), offset);
        self.last_monitor = Some(area.monitor.clone());
    }

    /// Screen top-left for a `card_w × card_h` card: the last monitor's
    /// saved offset if that monitor is still attached, else the primary's
    /// saved offset, else bottom-right of the primary work area `inset`
    /// px in. Always clamped to the chosen work area.
    pub fn resolve(&self, card_w: i32, card_h: i32, areas: &[WorkArea], inset: i32) -> (i32, i32) {
        let saved = |a: &&WorkArea| self.per_monitor.contains_key(&a.monitor);
        let last = self
            .last_monitor
            .as_ref()
            .and_then(|m| areas.iter().find(|a| &a.monitor == m))
            .filter(saved);
        let Some(area) = last
            .or_else(|| areas.iter().find(|a| a.primary))
            .or_else(|| areas.first())
        else {
            return (0, 0);
        };
        let r = area.rect;
        let (x, y) = match self.per_monitor.get(&area.monitor) {
            Some(&(dx, dy)) => (r.x.saturating_add(dx), r.y.saturating_add(dy)),
            None => (r.right() - inset - card_w, r.bottom() - inset - card_h),
        };
        clamp_to_work_area(x, y, card_w, card_h, r)
    }
}

fn distance_sq(r: Rect, x: i32, y: i32) -> i64 {
    let dx = i64::from(x.clamp(r.x, r.right()) - x);
    let dy = i64::from(y.clamp(r.y, r.bottom()) - y);
    dx * dx + dy * dy
}

/// Clamps a `w × h` box's top-left into `work`; a box larger than the work
/// area pins to its left/top edge.
pub fn clamp_to_work_area(x: i32, y: i32, w: i32, h: i32, work: Rect) -> (i32, i32) {
    let cx = if w > work.w {
        work.x
    } else {
        x.clamp(work.x, work.right() - w)
    };
    let cy = if h > work.h {
        work.y
    } else {
        y.clamp(work.y, work.bottom() - h)
    };
    (cx, cy)
}

/// New top `y` after the card's height changes to `new_h`: keeps the
/// bottom edge when the old card's centre is in the lower half of `work`,
/// else the top edge, then clamps into `work`.
pub fn anchor_resize(old_card: Rect, new_h: i32, work: Rect) -> i32 {
    let centre = old_card.y + old_card.h / 2;
    let y = if centre >= work.y + work.h / 2 {
        old_card.bottom() - new_h
    } else {
        old_card.y
    };
    clamp_to_work_area(old_card.x, y, old_card.w, new_h, work).1
}

#[cfg(test)]
mod tests {
    use super::*;

    const D1: &str = r"\\.\DISPLAY1";
    const D2: &str = r"\\.\DISPLAY2";

    fn primary() -> WorkArea {
        WorkArea {
            monitor: D1.into(),
            rect: Rect::new(0, 0, 1920, 1040),
            primary: true,
        }
    }

    fn second() -> WorkArea {
        WorkArea {
            monitor: D2.into(),
            rect: Rect::new(1920, 0, 1920, 1040),
            primary: false,
        }
    }

    #[test]
    fn default_bottom_right_inset_12() {
        let s = PositionStore::default();
        assert_eq!(s.resolve(270, 145, &[primary()], 12), (1638, 883));
        assert_eq!(s.resolve(270, 145, &[second(), primary()], 12), (1638, 883));
    }

    #[test]
    fn no_work_areas_falls_back_to_origin() {
        assert_eq!(PositionStore::default().resolve(270, 145, &[], 12), (0, 0));
    }

    #[test]
    fn roundtrip_json() {
        let mut s = PositionStore::default();
        s.record(100, 200, 270, 145, &[primary(), second()]);
        s.record(2000, 50, 270, 145, &[primary(), second()]);
        let back = PositionStore::from_json_str(&s.to_json_string()).unwrap();
        assert_eq!(back, s);
        assert!(s.to_json_string().contains("perMonitor"));
    }

    #[test]
    fn empty_or_partial_json_loads_defaults() {
        assert_eq!(
            PositionStore::from_json_str("{}").unwrap(),
            PositionStore::default()
        );
        assert!(PositionStore::from_json_str("not json").is_err());
    }

    #[test]
    fn restores_per_monitor_offset() {
        let areas = [primary(), second()];
        let mut s = PositionStore::default();
        s.record(2100, 50, 270, 145, &areas);
        assert_eq!(s.per_monitor.get(D2), Some(&(180, 50)));
        assert_eq!(s.resolve(270, 145, &areas, 12), (2100, 50));
        // Same offset follows the monitor if it moves.
        let moved = WorkArea {
            rect: Rect::new(-1920, 0, 1920, 1040),
            ..second()
        };
        assert_eq!(s.resolve(270, 145, &[primary(), moved], 12), (-1740, 50));
    }

    #[test]
    fn record_uses_card_centre() {
        let mut s = PositionStore::default();
        // Top-left on DISPLAY1, centre on DISPLAY2.
        s.record(1800, 100, 270, 145, &[primary(), second()]);
        assert_eq!(s.last_monitor.as_deref(), Some(D2));
        assert_eq!(s.per_monitor.get(D2), Some(&(-120, 100)));
    }

    #[test]
    fn record_off_every_monitor_uses_nearest() {
        let mut s = PositionStore::default();
        s.record(5000, 100, 270, 145, &[primary(), second()]);
        assert_eq!(s.last_monitor.as_deref(), Some(D2));
    }

    #[test]
    fn missing_monitor_uses_default() {
        let mut s = PositionStore::default();
        s.record(2100, 50, 270, 145, &[primary(), second()]);
        assert_eq!(s.resolve(270, 145, &[primary()], 12), (1638, 883));
    }

    #[test]
    fn missing_last_monitor_uses_primary_saved_offset() {
        let mut s = PositionStore::default();
        s.record(100, 100, 270, 145, &[primary(), second()]);
        s.record(2100, 50, 270, 145, &[primary(), second()]);
        assert_eq!(s.resolve(270, 145, &[primary()], 12), (100, 100));
    }

    #[test]
    fn extreme_hand_edited_offset_saturates_then_clamps() {
        let s = PositionStore::from_json_str(
            r#"{"lastMonitor":"\\\\.\\DISPLAY2","perMonitor":{"\\\\.\\DISPLAY2":[2147483647,2147483647]}}"#,
        )
        .unwrap();
        let below = WorkArea {
            rect: Rect::new(1920, 10, 1920, 1040),
            ..second()
        };
        // 1920 + i32::MAX would overflow (debug panic / release wrap).
        assert_eq!(
            s.resolve(270, 145, &[below], 12),
            (1920 + 1920 - 270, 10 + 1040 - 145)
        );
    }

    #[test]
    fn offscreen_position_clamped() {
        let s = PositionStore::from_json_str(
            r#"{"lastMonitor":"\\\\.\\DISPLAY1","perMonitor":{"\\\\.\\DISPLAY1":[5000,5000]}}"#,
        )
        .unwrap();
        let (x, y) = s.resolve(270, 145, &[primary()], 12);
        assert_eq!((x, y), (1650, 895));
        assert!(primary().rect.contains(x, y));
    }

    #[test]
    fn clamp_oversize_pins_to_top_left() {
        let work = Rect::new(10, 20, 100, 100);
        assert_eq!(clamp_to_work_area(50, 50, 200, 200, work), (10, 20));
        assert_eq!(clamp_to_work_area(-5, -5, 10, 10, work), (10, 20));
        assert_eq!(clamp_to_work_area(30, 40, 10, 10, work), (30, 40));
    }

    #[test]
    fn anchor_bottom_in_lower_half() {
        let work = primary().rect;
        let old = Rect::new(1638, 883, 270, 145);
        assert_eq!(anchor_resize(old, 28, work), 883 + 145 - 28);
        // Bottom 1048 overhangs the work area: the grown card is clamped in.
        assert_eq!(
            anchor_resize(Rect::new(1638, 1020, 270, 28), 145, work),
            1040 - 145
        );
    }

    #[test]
    fn anchor_top_in_upper_half() {
        let work = primary().rect;
        let old = Rect::new(100, 50, 270, 145);
        assert_eq!(anchor_resize(old, 28, work), 50);
        assert_eq!(anchor_resize(old, 300, work), 50);
    }
}
