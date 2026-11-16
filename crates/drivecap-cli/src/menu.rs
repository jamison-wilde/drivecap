//! Pure right-click menu model for the card and the tray (handoff
//! §Right-click menu, spec §7): the menu as data ([`MenuNode`]), and the
//! [`Command`] each item id stands for. `contextmenu` turns the nodes into
//! a Win32 popup menu; this module never touches Win32.
//!
//! Card: `Snooze ▸` · `Hide this volume` (over a row) · `Volumes ▸` ·
//! separator · `Font…` · `Edit config.json` · `Quit drivecap`. Tray: the
//! same without `Hide this volume`.

use crate::config::{SnoozeChoice, VolumeId};
use crate::controller::{Command, SnoozeTarget};
use crate::model::{RowState, SurfaceView};

/// Snooze ▸ items.
pub const ID_SNOOZE_10_MIN: u32 = 1;
pub const ID_SNOOZE_1_HOUR: u32 = 2;
pub const ID_SNOOZE_NEXT_LEVEL: u32 = 3;
pub const ID_HIDE: u32 = 10;
pub const ID_FONT: u32 = 20;
pub const ID_EDIT_CONFIG: u32 = 21;
pub const ID_QUIT: u32 = 22;
/// Volumes ▸ item `i` is `ID_VOLUME_BASE + i` (index into `connected`).
pub const ID_VOLUME_BASE: u32 = 100;

/// One menu entry.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuNode {
    Item {
        id: u32,
        label: String,
        checked: bool,
        enabled: bool,
    },
    Sub {
        label: String,
        enabled: bool,
        children: Vec<MenuNode>,
    },
    Separator,
}

/// The row a card menu was opened over: its index in the view at that
/// moment and its volume (the index can go stale while the menu is up).
#[derive(Clone, Debug, PartialEq)]
pub struct RowRef {
    pub index: usize,
    pub id: VolumeId,
}

/// Whether `Snooze ▸` is enabled, and whether its target is dire
/// ("Until next level" is then disabled).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnoozeSpec {
    pub enabled: bool,
    pub target_dire: bool,
}

/// `Snooze ▸` for the menu opened over row `row` (or over no row): a row
/// target is snoozable when it is at risk; the all-volumes target when any
/// row is — otherwise the item would only hide a tray-forced card for
/// nothing. "Until next level" is off for a dire row.
pub fn snooze_spec(view: &SurfaceView, row: Option<usize>) -> SnoozeSpec {
    let at_risk =
        |s: RowState| matches!(s, RowState::Warning | RowState::Dire | RowState::Snoozed(_));
    match row {
        Some(i) => view
            .rows
            .get(i)
            .map_or_else(SnoozeSpec::default, |r| SnoozeSpec {
                enabled: at_risk(r.state),
                target_dire: r.state == RowState::Dire,
            }),
        None => SnoozeSpec {
            enabled: view.rows.iter().any(|r| at_risk(r.state)),
            target_dire: false,
        },
    }
}

fn item(id: u32, label: &str, enabled: bool) -> MenuNode {
    MenuNode::Item {
        id,
        label: label.to_owned(),
        checked: false,
        enabled,
    }
}

fn snooze_sub(snooze: SnoozeSpec) -> MenuNode {
    MenuNode::Sub {
        label: "Snooze".into(),
        enabled: snooze.enabled,
        children: vec![
            item(ID_SNOOZE_10_MIN, "10 minutes", true),
            item(ID_SNOOZE_1_HOUR, "1 hour", true),
            item(
                ID_SNOOZE_NEXT_LEVEL,
                "Until next level",
                !snooze.target_dire,
            ),
        ],
    }
}

fn volumes_sub(connected: &[(VolumeId, String, bool)]) -> MenuNode {
    let children = (0u32..)
        .zip(connected)
        .map(|(i, (_, label, watched))| MenuNode::Item {
            id: ID_VOLUME_BASE + i,
            label: label.clone(),
            checked: *watched,
            enabled: true,
        })
        .collect();
    MenuNode::Sub {
        label: "Volumes".into(),
        enabled: !connected.is_empty(),
        children,
    }
}

/// The card's right-click menu; `row` is the row under the cursor.
pub fn card_menu(
    row: Option<&RowRef>,
    connected: &[(VolumeId, String, bool)],
    snooze: SnoozeSpec,
) -> Vec<MenuNode> {
    let mut m = vec![snooze_sub(snooze)];
    if row.is_some() {
        m.push(item(ID_HIDE, "Hide this volume", true));
    }
    m.extend([
        volumes_sub(connected),
        MenuNode::Separator,
        item(ID_FONT, "Font…", true),
        item(ID_EDIT_CONFIG, "Edit config.json", true),
        item(ID_QUIT, "Quit drivecap", true),
    ]);
    m
}

/// The tray icon's right-click menu: the card menu without row items.
pub fn tray_menu(connected: &[(VolumeId, String, bool)], snooze: SnoozeSpec) -> Vec<MenuNode> {
    card_menu(None, connected, snooze)
}

/// The command for a picked item id (`0` = dismissed → `None`). Snooze
/// targets the row's volume, else every at-risk volume.
pub fn command_for(
    id: u32,
    row: Option<&RowRef>,
    connected: &[(VolumeId, String, bool)],
) -> Option<Command> {
    let target = || row.map_or(SnoozeTarget::All, |r| SnoozeTarget::Volume(r.id.clone()));
    let snooze = |c| Some(Command::SnoozeWith(target(), c));
    match id {
        ID_SNOOZE_10_MIN => snooze(SnoozeChoice::TenMin),
        ID_SNOOZE_1_HOUR => snooze(SnoozeChoice::OneHour),
        ID_SNOOZE_NEXT_LEVEL => snooze(SnoozeChoice::NextLevel),
        ID_HIDE => row.map(|r| Command::HideRow(r.index)),
        ID_FONT => Some(Command::ChangeFont),
        ID_EDIT_CONFIG => Some(Command::EditConfig),
        ID_QUIT => Some(Command::Quit),
        v if v >= ID_VOLUME_BASE => {
            let (vid, label, _) = connected.get((v - ID_VOLUME_BASE) as usize)?;
            Some(Command::ToggleVolume {
                id: vid.clone(),
                label: label.clone(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;
    use crate::model::{Dot, RowView};

    fn connected() -> Vec<(VolumeId, String, bool)> {
        vec![
            (VolumeId("c".into()), "C: Windows".into(), true),
            (VolumeId("e".into()), "E: SanDisk 64GB".into(), false),
        ]
    }

    fn row_ref() -> RowRef {
        RowRef {
            index: 1,
            id: VolumeId("c".into()),
        }
    }

    const ON: SnoozeSpec = SnoozeSpec {
        enabled: true,
        target_dire: false,
    };

    fn labels(nodes: &[MenuNode]) -> Vec<&str> {
        nodes
            .iter()
            .map(|n| match n {
                MenuNode::Item { label, .. } | MenuNode::Sub { label, .. } => label.as_str(),
                MenuNode::Separator => "-",
            })
            .collect()
    }

    fn sub<'a>(nodes: &'a [MenuNode], name: &str) -> (&'a [MenuNode], bool) {
        nodes
            .iter()
            .find_map(|n| match n {
                MenuNode::Sub {
                    label,
                    enabled,
                    children,
                } if label == name => Some((children.as_slice(), *enabled)),
                _ => None,
            })
            .expect("submenu present")
    }

    fn item(nodes: &[MenuNode], want: u32) -> (bool, bool) {
        nodes
            .iter()
            .find_map(|n| match n {
                MenuNode::Item {
                    id,
                    checked,
                    enabled,
                    ..
                } if *id == want => Some((*checked, *enabled)),
                _ => None,
            })
            .expect("item present")
    }

    #[test]
    fn card_menu_with_row_has_hide() {
        let m = card_menu(Some(&row_ref()), &connected(), ON);
        assert_eq!(
            labels(&m),
            [
                "Snooze",
                "Hide this volume",
                "Volumes",
                "-",
                "Font…",
                "Edit config.json",
                "Quit drivecap"
            ]
        );
        let (snooze, _) = sub(&m, "Snooze");
        assert_eq!(labels(snooze), ["10 minutes", "1 hour", "Until next level"]);
    }

    #[test]
    fn card_menu_off_a_row_has_no_hide() {
        let m = card_menu(None, &connected(), ON);
        assert!(!labels(&m).contains(&"Hide this volume"));
    }

    #[test]
    fn tray_menu_has_no_hide() {
        let m = tray_menu(&connected(), ON);
        assert_eq!(
            labels(&m),
            [
                "Snooze",
                "Volumes",
                "-",
                "Font…",
                "Edit config.json",
                "Quit drivecap"
            ]
        );
    }

    #[test]
    fn next_level_disabled_for_dire() {
        let dire = SnoozeSpec {
            enabled: true,
            target_dire: true,
        };
        let m = card_menu(Some(&row_ref()), &connected(), dire);
        let (snooze, enabled) = sub(&m, "Snooze");
        assert!(enabled);
        assert_eq!(item(snooze, ID_SNOOZE_NEXT_LEVEL), (false, false));
        assert_eq!(item(snooze, ID_SNOOZE_10_MIN), (false, true));
        let m = card_menu(Some(&row_ref()), &connected(), ON);
        assert_eq!(
            item(sub(&m, "Snooze").0, ID_SNOOZE_NEXT_LEVEL),
            (false, true)
        );
    }

    #[test]
    fn snooze_disabled_when_nothing_to_snooze() {
        let m = tray_menu(&connected(), SnoozeSpec::default());
        assert!(!sub(&m, "Snooze").1);
    }

    #[test]
    fn volumes_submenu_checks_watched() {
        let m = card_menu(None, &connected(), ON);
        let (vols, enabled) = sub(&m, "Volumes");
        assert!(enabled);
        assert_eq!(labels(vols), ["C: Windows", "E: SanDisk 64GB"]);
        assert_eq!(item(vols, ID_VOLUME_BASE), (true, true));
        assert_eq!(item(vols, ID_VOLUME_BASE + 1), (false, true));
        // Nothing connected: the submenu is there but greyed.
        assert!(!sub(&card_menu(None, &[], ON), "Volumes").1);
    }

    #[test]
    fn command_ids_roundtrip() {
        use Command::SnoozeWith;
        use SnoozeChoice::{NextLevel, OneHour, TenMin};
        let c = connected();
        let row = row_ref();
        let vol = SnoozeTarget::Volume(VolumeId("c".into()));
        let cases = [
            (
                ID_SNOOZE_10_MIN,
                Some(&row),
                SnoozeWith(vol.clone(), TenMin),
            ),
            (
                ID_SNOOZE_1_HOUR,
                Some(&row),
                SnoozeWith(vol.clone(), OneHour),
            ),
            (ID_SNOOZE_NEXT_LEVEL, Some(&row), SnoozeWith(vol, NextLevel)),
            (
                ID_SNOOZE_10_MIN,
                None,
                SnoozeWith(SnoozeTarget::All, TenMin),
            ),
            (
                ID_SNOOZE_NEXT_LEVEL,
                None,
                SnoozeWith(SnoozeTarget::All, NextLevel),
            ),
            (ID_HIDE, Some(&row), Command::HideRow(1)),
            (ID_FONT, None, Command::ChangeFont),
            (ID_EDIT_CONFIG, None, Command::EditConfig),
            (ID_QUIT, None, Command::Quit),
            (
                ID_VOLUME_BASE + 1,
                None,
                Command::ToggleVolume {
                    id: VolumeId("e".into()),
                    label: "E: SanDisk 64GB".into(),
                },
            ),
        ];
        for (id, row, want) in cases {
            assert_eq!(command_for(id, row, &c), Some(want), "id {id}");
        }
    }

    #[test]
    fn unknown_or_dismissed_ids_are_no_command() {
        let c = connected();
        assert_eq!(command_for(0, None, &c), None);
        assert_eq!(command_for(ID_HIDE, None, &c), None);
        assert_eq!(command_for(ID_VOLUME_BASE + 2, None, &c), None);
        assert_eq!(command_for(999, None, &c), None);
    }

    fn view(states: &[RowState]) -> SurfaceView {
        let rows = states
            .iter()
            .enumerate()
            .map(|(i, &state)| RowView {
                id: VolumeId(i.to_string()),
                letter: String::new(),
                label: String::new(),
                value: String::new(),
                state,
                fill: 0.0,
                gaps: Vec::new(),
                show_snooze_btn: false,
                snooze_active: false,
            })
            .collect();
        SurfaceView {
            title: String::new(),
            dot: Dot::Muted,
            snooze_all_active: false,
            rows,
            halo: false,
            mode: Mode::Expanded,
        }
    }

    #[test]
    fn snooze_spec_follows_the_target() {
        let v = view(&[RowState::Dire, RowState::Warning, RowState::Healthy]);
        let dire = SnoozeSpec {
            enabled: true,
            target_dire: true,
        };
        assert_eq!(snooze_spec(&v, Some(0)), dire);
        assert_eq!(snooze_spec(&v, Some(1)), ON);
        assert_eq!(snooze_spec(&v, Some(2)), SnoozeSpec::default());
        // All at-risk volumes: dire ones fall back to a timed snooze.
        assert_eq!(snooze_spec(&v, None), ON);
        let healthy = view(&[RowState::Healthy, RowState::Unreachable]);
        assert_eq!(snooze_spec(&healthy, None), SnoozeSpec::default());
        let snoozed = view(&[RowState::Snoozed(SnoozeChoice::TenMin)]);
        assert_eq!(snooze_spec(&snoozed, None), ON);
    }
}
