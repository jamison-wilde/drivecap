//! `~/.drivecap/config.json` schema, spec defaults, clamping and safe
//! load/save.
//!
//! Every struct is `#[serde(default)]` so partial JSON works and unknown
//! keys are ignored. [`Config::from_json_str`] and [`Config::load`] always
//! [`normalize`](Config) the result: out-of-range numbers are clamped and
//! invalid rules are replaced, so the rest of the app never sees a value
//! it has to re-check. A file that fails to parse is never overwritten
//! ([`LoadOutcome::writable`] is `false`) — the user's hand edits survive
//! until they fix them.

use crate::error::{DrivecapError, DrivecapResult};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;

/// One gibibyte. "GB" in config and UI means GiB, matching Explorer.
pub const GIB: u64 = 1 << 30;

/// Largest accepted rule value in GiB (1 PiB); larger values are invalid.
pub const MAX_GB: f64 = 1e6;
/// Smallest accepted step in GiB.
pub const MIN_STEP_GB: f64 = 0.01;
/// Most steps a rule may span between dire and threshold; more is not
/// meaningful for a bar that draws a gap per level.
pub const MAX_LEVELS: u64 = 1_000;
/// Ceiling for every seconds-valued setting (one day).
pub const MAX_SECS: u64 = 86_400;
/// Ceiling for every millisecond animation timing (one hour).
pub const MAX_MS: u64 = 3_600_000;

/// Stable volume key: `\\?\Volume{guid}\` for local volumes, or the
/// lowercase network path without a trailing `\`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VolumeId(pub String);

/// A validated warning rule in bytes. Invariant: `step > 0`, `dire < threshold`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    pub threshold: u64,
    pub step: u64,
    pub dire: u64,
}

/// Snooze duration choice offered in the menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnoozeChoice {
    #[default]
    #[serde(rename = "10min")]
    TenMin,
    #[serde(rename = "1h")]
    OneHour,
    /// Until the next level crossing (timed 10 min on a dire volume).
    #[serde(rename = "nextLevel")]
    NextLevel,
}

impl SnoozeChoice {
    /// Menu label.
    pub fn label(&self) -> &'static str {
        match self {
            SnoozeChoice::TenMin => "10 min",
            SnoozeChoice::OneHour => "1 h",
            SnoozeChoice::NextLevel => "next level",
        }
    }

    /// Timed duration, or `None` for [`SnoozeChoice::NextLevel`].
    pub fn duration_ms(&self) -> Option<u64> {
        match self {
            SnoozeChoice::TenMin => Some(10 * 60_000),
            SnoozeChoice::OneHour => Some(60 * 60_000),
            SnoozeChoice::NextLevel => None,
        }
    }
}

/// Card display mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Expanded,
    Minimized,
}

/// User-chosen card font (`ChooseFontW` result).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FontSpec {
    pub face: String,
    pub point_size: u32,
    pub weight: u32,
}

impl Default for FontSpec {
    fn default() -> Self {
        FontSpec {
            face: "Segoe UI".into(),
            point_size: 9,
            weight: 400,
        }
    }
}

/// Animation and scheduling timings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Timings {
    pub fade_in_ms: u64,
    pub hold_ms: u64,
    pub crossing_hold_ms: u64,
    pub fade_out_ms: u64,
    pub linger_ms: u64,
    pub hover_rest_ms: u64,
    /// Reminder interval at the threshold.
    pub reminder_max_secs: u64,
    /// Reminder interval at dire (also the dire poll interval).
    pub reminder_min_secs: u64,
    /// How long an unreachable row stays in minimized mode.
    pub unreachable_drop_secs: u64,
}

impl Timings {
    /// Caps every value so the engine's ms arithmetic cannot overflow.
    fn clamp(&mut self) {
        for ms in [
            &mut self.fade_in_ms,
            &mut self.hold_ms,
            &mut self.crossing_hold_ms,
            &mut self.fade_out_ms,
            &mut self.linger_ms,
            &mut self.hover_rest_ms,
        ] {
            *ms = (*ms).min(MAX_MS);
        }
        self.unreachable_drop_secs = self.unreachable_drop_secs.min(MAX_SECS);
        self.reminder_min_secs = self.reminder_min_secs.clamp(1, MAX_SECS);
        self.reminder_max_secs = self
            .reminder_max_secs
            .clamp(self.reminder_min_secs, MAX_SECS);
    }
}

impl Default for Timings {
    fn default() -> Self {
        Timings {
            fade_in_ms: 200,
            hold_ms: 4000,
            crossing_hold_ms: 6000,
            fade_out_ms: 600,
            linger_ms: 3000,
            hover_rest_ms: 150,
            reminder_max_secs: 60,
            reminder_min_secs: 10,
            unreachable_drop_secs: 300,
        }
    }
}

/// Per-volume config entry. `None` fields inherit [`Config::defaults`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VolumeEntry {
    pub id: VolumeId,
    /// Human-readable label; never used for matching.
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold_gb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_gb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dire_gb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// A rule as written in config, in GiB.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuleGb {
    pub threshold_gb: f64,
    pub step_gb: f64,
    pub dire_gb: f64,
}

impl Default for RuleGb {
    fn default() -> Self {
        RuleGb {
            threshold_gb: 20.0,
            step_gb: 5.0,
            dire_gb: 2.0,
        }
    }
}

impl RuleGb {
    fn is_valid(&self) -> bool {
        let finite = [self.threshold_gb, self.step_gb, self.dire_gb]
            .iter()
            .all(|v| v.is_finite());
        let in_range = self.step_gb >= MIN_STEP_GB
            && self.step_gb <= MAX_GB
            && self.threshold_gb <= MAX_GB
            && self.dire_gb >= 0.0
            && self.dire_gb < self.threshold_gb;
        // Re-check after the byte conversion truncates.
        finite && in_range && {
            let r = self.to_bytes();
            r.step > 0 && r.dire < r.threshold && (r.threshold - r.dire) / r.step <= MAX_LEVELS
        }
    }

    fn to_bytes(self) -> Rule {
        let b = |gb: f64| (gb * GIB as f64) as u64;
        Rule {
            threshold: b(self.threshold_gb),
            step: b(self.step_gb),
            dire: b(self.dire_gb),
        }
    }
}

/// Optional `#RRGGBB` overrides for one palette's tokens.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PaletteOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dire: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dire_accent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
}

impl PaletteOverride {
    fn drop_invalid(&mut self) {
        for slot in [
            &mut self.warning,
            &mut self.warning_text,
            &mut self.dire,
            &mut self.dire_accent,
            &mut self.text,
            &mut self.surface,
        ] {
            if slot.as_deref().is_some_and(|s| !is_hex_color(s)) {
                log::warn!("config: ignoring colour override {slot:?} (want #RRGGBB)");
                *slot = None;
            }
        }
    }
}

fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Colour overrides for both themes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ColorOverrides {
    pub light: PaletteOverride,
    pub dark: PaletteOverride,
}

/// The whole `config.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Healthy-volume poll interval (floor 1 s).
    pub poll_interval_secs: u64,
    pub defaults: RuleGb,
    pub volumes: Vec<VolumeEntry>,
    /// Mount points and network folders to watch beyond the default set.
    pub extra_paths: Vec<String>,
    pub include_usb: bool,
    /// Idle card opacity, 20–100 in steps of 5.
    #[serde(deserialize_with = "de_lenient_pct")]
    pub idle_opacity_pct: u8,
    pub snooze: SnoozeChoice,
    pub mode: Mode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<FontSpec>,
    pub timings: Timings,
    pub colors: ColorOverrides,
    /// Extra UI scale on top of DPI, 50–300.
    pub scale_pct: u32,
    pub dire_halo: bool,
    pub level_gaps: bool,
}

/// Accepts any JSON number for a percentage so `1000` clamps instead of
/// failing the whole file on `u8` overflow.
fn de_lenient_pct<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    let v = f64::deserialize(d)?;
    Ok(v.round().clamp(0.0, 255.0) as u8)
}

impl Default for Config {
    fn default() -> Self {
        Config {
            poll_interval_secs: 60,
            defaults: RuleGb::default(),
            volumes: Vec::new(),
            extra_paths: Vec::new(),
            include_usb: false,
            idle_opacity_pct: 50,
            snooze: SnoozeChoice::TenMin,
            mode: Mode::Expanded,
            font: None,
            timings: Timings::default(),
            colors: ColorOverrides::default(),
            scale_pct: 100,
            dire_halo: true,
            level_gaps: true,
        }
    }
}

/// Result of [`Config::load`].
#[derive(Debug)]
pub struct LoadOutcome {
    pub config: Config,
    /// `false` when the file exists but could not be read or parsed —
    /// callers must not save over it.
    pub writable: bool,
}

impl Config {
    /// Parses and normalizes config JSON.
    pub fn from_json_str(s: &str) -> Result<Config, serde_json::Error> {
        let mut c: Config = serde_json::from_str(s)?;
        c.normalize();
        Ok(c)
    }

    /// Pretty-printed JSON.
    pub fn to_json_string(&self) -> String {
        // Serializing plain data structs with String keys cannot fail.
        serde_json::to_string_pretty(self).unwrap_or_else(|_| String::from("{}"))
    }

    /// Loads `path`. Missing → defaults, written to disk. Unreadable or
    /// malformed → defaults, `writable = false`, file left untouched.
    pub fn load(path: &Path) -> LoadOutcome {
        match std::fs::read_to_string(path) {
            Ok(text) => match Config::from_json_str(&text) {
                Ok(config) => LoadOutcome {
                    config,
                    writable: true,
                },
                Err(e) => {
                    log::warn!(
                        "config: {} is malformed ({e}); running on defaults",
                        path.display()
                    );
                    LoadOutcome {
                        config: Config::default(),
                        writable: false,
                    }
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let config = Config::default();
                if let Err(e) = config.save(path) {
                    log::warn!("config: could not write defaults: {e}");
                }
                LoadOutcome {
                    config,
                    writable: true,
                }
            }
            Err(e) => {
                log::warn!(
                    "config: cannot read {} ({e}); running on defaults",
                    path.display()
                );
                LoadOutcome {
                    config: Config::default(),
                    writable: false,
                }
            }
        }
    }

    /// Atomic save: writes a `.tmp` sibling, then renames over `path`.
    pub fn save(&self, path: &Path) -> DrivecapResult<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| DrivecapError::io(dir, e))?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        std::fs::write(&tmp, self.to_json_string()).map_err(|e| DrivecapError::io(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            DrivecapError::io(path, e)
        })
    }

    /// The validated rule for `id`: entry fields override
    /// [`Config::defaults`] per field; an invalid merge yields the defaults.
    pub fn rule_for(&self, id: &VolumeId) -> Rule {
        let merged = self.merged_rule(id);
        let rule = if merged.is_valid() {
            merged
        } else {
            self.defaults
        };
        rule.to_bytes()
    }

    /// The entry's `enabled` flag, if it has an entry that sets one.
    pub fn entry_enabled(&self, id: &VolumeId) -> Option<bool> {
        self.entry(id).and_then(|e| e.enabled)
    }

    /// Sets `enabled` on the entry for `id`, creating it if absent.
    pub fn set_enabled(&mut self, id: &VolumeId, label: &str, enabled: bool) {
        match self.volumes.iter_mut().find(|e| &e.id == id) {
            Some(e) => {
                e.enabled = Some(enabled);
                e.label = label.to_string();
            }
            None => self.volumes.push(VolumeEntry {
                id: id.clone(),
                label: label.to_string(),
                enabled: Some(enabled),
                ..VolumeEntry::default()
            }),
        }
    }

    fn entry(&self, id: &VolumeId) -> Option<&VolumeEntry> {
        self.volumes.iter().find(|e| &e.id == id)
    }

    fn merged_rule(&self, id: &VolumeId) -> RuleGb {
        let d = self.defaults;
        match self.entry(id) {
            None => d,
            Some(e) => RuleGb {
                threshold_gb: e.threshold_gb.unwrap_or(d.threshold_gb),
                step_gb: e.step_gb.unwrap_or(d.step_gb),
                dire_gb: e.dire_gb.unwrap_or(d.dire_gb),
            },
        }
    }

    /// Clamps ranges and replaces invalid rules/colours, warning once each.
    fn normalize(&mut self) {
        self.poll_interval_secs = self.poll_interval_secs.clamp(1, MAX_SECS);
        let pct = (u32::from(self.idle_opacity_pct) + 2) / 5 * 5;
        self.idle_opacity_pct = pct.clamp(20, 100) as u8;
        self.scale_pct = self.scale_pct.clamp(50, 300);
        self.timings.clamp();
        if let Some(font) = &mut self.font {
            font.point_size = font.point_size.clamp(6, 72);
        }
        if !self.defaults.is_valid() {
            log::warn!(
                "config: invalid defaults {:?}; using 20/5/2 GB",
                self.defaults
            );
            self.defaults = RuleGb::default();
        }
        for e in &self.volumes {
            if !self.merged_rule(&e.id).is_valid() {
                log::warn!("config: invalid rule for {:?}; using defaults", e.id.0);
            }
        }
        self.colors.light.drop_invalid();
        self.colors.dark.drop_invalid();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> VolumeId {
        VolumeId(s.to_string())
    }

    #[test]
    fn defaults_match_spec() {
        let c = Config::default();
        assert_eq!(c.poll_interval_secs, 60);
        assert_eq!(
            c.rule_for(&id("any")),
            Rule {
                threshold: 20 * GIB,
                step: 5 * GIB,
                dire: 2 * GIB
            }
        );
        assert_eq!(c.idle_opacity_pct, 50);
        assert_eq!(c.snooze, SnoozeChoice::TenMin);
        assert_eq!(c.mode, Mode::Expanded);
        let t = &c.timings;
        assert_eq!(
            (
                t.fade_in_ms,
                t.hold_ms,
                t.crossing_hold_ms,
                t.fade_out_ms,
                t.linger_ms,
                t.hover_rest_ms
            ),
            (200, 4000, 6000, 600, 3000, 150)
        );
        assert_eq!((t.reminder_max_secs, t.reminder_min_secs), (60, 10));
        assert_eq!(t.unreachable_drop_secs, 300);
        assert_eq!(c.scale_pct, 100);
        assert!(c.dire_halo && c.level_gaps);
        assert!(!c.include_usb);
    }

    #[test]
    fn empty_object_parses_to_defaults() {
        assert_eq!(Config::from_json_str("{}").unwrap(), Config::default());
    }

    #[test]
    fn clamps_out_of_range() {
        let c =
            Config::from_json_str(r#"{"pollIntervalSecs":0,"idleOpacityPct":7,"scalePct":10000}"#)
                .unwrap();
        assert_eq!(
            (c.poll_interval_secs, c.idle_opacity_pct, c.scale_pct),
            (1, 20, 300)
        );
        let c = Config::from_json_str(r#"{"idleOpacityPct":53}"#).unwrap();
        assert_eq!(c.idle_opacity_pct, 55);
    }

    #[test]
    fn entry_overrides_rule_fields() {
        let c = Config::from_json_str(r#"{"volumes":[{"id":"X","thresholdGb":50}]}"#).unwrap();
        assert_eq!(
            c.rule_for(&id("X")),
            Rule {
                threshold: 50 * GIB,
                step: 5 * GIB,
                dire: 2 * GIB
            }
        );
    }

    #[test]
    fn invalid_rule_falls_back_to_defaults() {
        let c = Config::from_json_str(
            r#"{"volumes":[
                {"id":"A","direGb":30,"thresholdGb":20},
                {"id":"B","stepGb":0},
                {"id":"C","thresholdGb":-1}
            ]}"#,
        )
        .unwrap();
        let d = Config::default().rule_for(&id("none"));
        for v in ["A", "B", "C"] {
            assert_eq!(c.rule_for(&id(v)), d, "volume {v}");
        }
    }

    #[test]
    fn invalid_defaults_fall_back_to_builtin_rule() {
        let c = Config::from_json_str(r#"{"defaults":{"stepGb":0}}"#).unwrap();
        assert_eq!(c.defaults, RuleGb::default());
    }

    #[test]
    fn malformed_json_is_not_writable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{ nope").unwrap();
        let out = Config::load(&path);
        assert!(!out.writable);
        assert_eq!(out.config, Config::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ nope");
    }

    #[test]
    fn missing_file_writes_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.json");
        let out = Config::load(&path);
        assert!(out.writable);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(Config::from_json_str(&text).unwrap(), Config::default());
    }

    #[test]
    fn save_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "old").unwrap();
        let c = Config {
            poll_interval_secs: 5,
            ..Config::default()
        };
        c.save(&path).unwrap();
        let back = Config::load(&path);
        assert_eq!(back.config.poll_interval_secs, 5);
        assert!(!dir.path().join("config.json.tmp").exists());
    }

    #[test]
    fn roundtrip_preserves_values() {
        let mut c = Config {
            poll_interval_secs: 30,
            defaults: RuleGb {
                threshold_gb: 40.0,
                step_gb: 10.0,
                dire_gb: 4.5,
            },
            extra_paths: vec!["C:\\dev".into()],
            include_usb: true,
            idle_opacity_pct: 75,
            snooze: SnoozeChoice::NextLevel,
            mode: Mode::Minimized,
            font: Some(FontSpec {
                face: "Consolas".into(),
                point_size: 11,
                weight: 700,
            }),
            scale_pct: 150,
            dire_halo: false,
            level_gaps: false,
            ..Config::default()
        };
        c.timings.hold_ms = 5000;
        c.colors.dark.warning = Some("#FFAA00".into());
        c.volumes.push(VolumeEntry {
            id: id("\\\\?\\Volume{abc}\\"),
            label: "E: SanDisk".into(),
            threshold_gb: Some(8.0),
            step_gb: None,
            dire_gb: Some(1.0),
            enabled: Some(false),
        });
        let back = Config::from_json_str(&c.to_json_string()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn set_enabled_upserts_entry() {
        let mut c = Config::default();
        c.set_enabled(&id("X"), "E: Stick", false);
        assert_eq!(c.entry_enabled(&id("X")), Some(false));
        assert_eq!(c.volumes.len(), 1);
        c.set_enabled(&id("X"), "E: Stick", true);
        assert_eq!(c.entry_enabled(&id("X")), Some(true));
        assert_eq!(c.volumes.len(), 1);
        assert_eq!(c.entry_enabled(&id("Y")), None);
    }

    #[test]
    fn set_enabled_keeps_rule_overrides() {
        let mut c = Config::from_json_str(r#"{"volumes":[{"id":"X","thresholdGb":50}]}"#).unwrap();
        c.set_enabled(&id("X"), "E:", false);
        assert_eq!(c.volumes[0].threshold_gb, Some(50.0));
    }

    #[test]
    fn snooze_choice_serde_names() {
        for (c, s) in [
            (SnoozeChoice::TenMin, "\"10min\""),
            (SnoozeChoice::OneHour, "\"1h\""),
            (SnoozeChoice::NextLevel, "\"nextLevel\""),
        ] {
            assert_eq!(serde_json::to_string(&c).unwrap(), s);
            assert_eq!(serde_json::from_str::<SnoozeChoice>(s).unwrap(), c);
        }
    }

    #[test]
    fn snooze_choice_labels_and_durations() {
        assert_eq!(SnoozeChoice::TenMin.label(), "10 min");
        assert_eq!(SnoozeChoice::OneHour.label(), "1 h");
        assert_eq!(SnoozeChoice::NextLevel.label(), "next level");
        assert_eq!(SnoozeChoice::TenMin.duration_ms(), Some(600_000));
        assert_eq!(SnoozeChoice::OneHour.duration_ms(), Some(3_600_000));
        assert_eq!(SnoozeChoice::NextLevel.duration_ms(), None);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let c = Config::from_json_str(r#"{"bogus":1,"pollIntervalSecs":9}"#).unwrap();
        assert_eq!(c.poll_interval_secs, 9);
    }

    #[test]
    fn invalid_color_override_is_dropped() {
        let c = Config::from_json_str(
            r##"{"colors":{"light":{"warning":"orange","dire":"#112233"}}}"##,
        )
        .unwrap();
        assert_eq!(c.colors.light.warning, None);
        assert_eq!(c.colors.light.dire.as_deref(), Some("#112233"));
    }

    #[test]
    fn tiny_step_is_invalid() {
        let c = Config::from_json_str(r#"{"volumes":[{"id":"X","stepGb":1e-10}]}"#).unwrap();
        assert_eq!(
            c.rule_for(&id("X")),
            Config::default().rule_for(&id("none"))
        );
        let c = Config::from_json_str(r#"{"defaults":{"stepGb":1e-10}}"#).unwrap();
        assert_eq!(c.defaults, RuleGb::default());
    }

    #[test]
    fn huge_rule_values_are_invalid() {
        let c = Config::from_json_str(
            r#"{"volumes":[{"id":"A","thresholdGb":1e12},{"id":"B","stepGb":1e12}]}"#,
        )
        .unwrap();
        let d = Config::default().rule_for(&id("none"));
        assert_eq!(c.rule_for(&id("A")), d);
        assert_eq!(c.rule_for(&id("B")), d);
    }

    #[test]
    fn huge_timings_are_clamped() {
        let c = Config::from_json_str(
            r#"{"pollIntervalSecs":18446744073709551615,"timings":{
                "reminderMaxSecs":18446744073709551615,"reminderMinSecs":18446744073709551615,
                "unreachableDropSecs":18446744073709551615,"holdMs":18446744073709551615}}"#,
        )
        .unwrap();
        let t = &c.timings;
        assert_eq!(c.poll_interval_secs, MAX_SECS);
        assert_eq!(
            (
                t.reminder_max_secs,
                t.reminder_min_secs,
                t.unreachable_drop_secs
            ),
            (MAX_SECS, MAX_SECS, MAX_SECS)
        );
        assert_eq!(t.hold_ms, MAX_MS);
    }

    #[test]
    fn failed_rename_removes_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep"), "x").unwrap();
        assert!(Config::default().save(&path).is_err());
        assert!(!dir.path().join("config.json.tmp").exists());
    }

    #[test]
    fn level_count_capped_at_max_levels() {
        let c = Config::from_json_str(
            r#"{"volumes":[
                {"id":"OK","thresholdGb":1000,"stepGb":1,"direGb":0},
                {"id":"BAD","thresholdGb":1001,"stepGb":1,"direGb":0}
            ]}"#,
        )
        .unwrap();
        assert_eq!(c.rule_for(&id("OK")).threshold, 1000 * GIB);
        assert_eq!(
            c.rule_for(&id("BAD")),
            Config::default().rule_for(&id("none"))
        );
    }

    #[test]
    fn font_point_size_clamps_to_6_through_72() {
        let c = Config::from_json_str(r#"{"font":{"pointSize":0}}"#).unwrap();
        assert_eq!(c.font.unwrap().point_size, 6);
        let c = Config::from_json_str(r#"{"font":{"pointSize":5000}}"#).unwrap();
        assert_eq!(c.font.unwrap().point_size, 72);
        let c = Config::from_json_str(r#"{"font":{"pointSize":11}}"#).unwrap();
        assert_eq!(c.font.unwrap().point_size, 11);
    }
}
