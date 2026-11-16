//! Volume enumeration, selection and local free-space query.
//!
//! The pure core is [`select`]: given the connected volumes, the resolved
//! `extraPaths` and the config, it decides which volumes are watched. The
//! Win32 adapter [`Win32Volumes`] produces the [`RawVolume`] inputs behind
//! the [`VolumeSource`] seam; [`query_free`] reads one volume's free bytes.
//!
//! Local volumes are keyed by volume GUID path (`\\?\Volume{…}\`) so a rule
//! follows a USB device across drive letters; network folders have no GUID
//! and are keyed by [`normalize_network`] of their path.

use std::collections::HashSet;
use std::time::Instant;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    BusTypeUsb, CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR,
    STORAGE_PROPERTY_QUERY, StorageDeviceProperty,
};
use windows_core::PCWSTR;

use crate::config::{Config, VolumeId};

/// `GetDriveTypeW` result for removable media (USB sticks, card readers).
pub const DRIVE_REMOVABLE: u32 = 2;
/// `GetDriveTypeW` result for fixed disks (internal and USB HDDs alike).
pub const DRIVE_FIXED: u32 = 3;
/// `GetDriveTypeW` result for network drives.
pub const DRIVE_REMOTE: u32 = 4;

/// Label used when a volume has no name of its own (Explorer's wording).
const DEFAULT_LABEL: &str = "Local Disk";

/// How a volume's free space is queried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeKind {
    /// Local volume — cheap query, safe on the UI thread.
    Local,
    /// Network folder — may hang; probe on a worker thread.
    Network,
}

/// Storage bus of the device backing a volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Usb,
    Other,
}

/// A watched volume, as the rest of the app sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeInfo {
    pub id: VolumeId,
    pub kind: VolumeKind,
    /// Path handed to [`query_free`] (`C:\`, `C:\mnt\data\`, `\\nas\builds`).
    pub root: String,
    /// `"C:"`, or `""` for mount points and network folders.
    pub letter: String,
    /// `"Windows"`, `"\\nas\builds"`, `"C:\dev"`.
    pub label: String,
    /// `DRIVE_REMOVABLE` or on a USB bus.
    pub removable: bool,
}

/// A volume as reported by a [`VolumeSource`], before selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawVolume {
    pub id: VolumeId,
    pub root: String,
    pub letter: String,
    pub label: String,
    /// `GetDriveTypeW` result: [`DRIVE_REMOVABLE`], [`DRIVE_FIXED`], [`DRIVE_REMOTE`], …
    pub drive_type: u32,
    pub bus: Bus,
}

/// Seam over the OS volume list (ADR-0002).
pub trait VolumeSource {
    /// Every connected lettered volume (fixed and removable).
    fn connected(&self) -> Vec<RawVolume>;
    /// Resolves a configured extra path to the volume that holds it; a
    /// network path resolves to a [`DRIVE_REMOTE`] volume keyed by
    /// [`normalize_network`]. `None` when the path cannot be resolved.
    fn resolve_path(&self, path: &str) -> Option<RawVolume>;
}

// ── Pure core ─────────────────────────────────────────────────────────────────

/// Network key: lowercase, without trailing `\`.
pub fn normalize_network(path: &str) -> String {
    path.trim_end_matches('\\').to_lowercase()
}

/// The [`RawVolume`] for a network folder (or mapped network drive path).
pub fn network_raw(path: &str) -> RawVolume {
    let key = normalize_network(path);
    RawVolume {
        id: VolumeId(key.clone()),
        root: key.clone(),
        letter: String::new(),
        label: key,
        drive_type: DRIVE_REMOTE,
        bus: Bus::Other,
    }
}

/// Chooses the watched volumes: defaults + opt-ins − disabled, then extras
/// collapsed by id. Lettered volumes come first (by letter), then extras in
/// config order.
pub fn select(raw: &[RawVolume], extras: &[RawVolume], cfg: &Config) -> Vec<VolumeInfo> {
    let mut lettered: Vec<&RawVolume> = raw.iter().filter(|r| watched(r, cfg)).collect();
    lettered.sort_by(|a, b| a.letter.cmp(&b.letter));
    let mut seen: HashSet<&VolumeId> = HashSet::new();
    let mut out = Vec::new();
    for r in lettered.into_iter().chain(extras) {
        if cfg.entry_enabled(&r.id) == Some(false) {
            continue;
        }
        if !seen.insert(&r.id) {
            log::info!("volumes: extra {} collapsed into {}", r.label, r.id.0);
            continue;
        }
        out.push(VolumeInfo {
            id: r.id.clone(),
            kind: if r.drive_type == DRIVE_REMOTE {
                VolumeKind::Network
            } else {
                VolumeKind::Local
            },
            root: r.root.clone(),
            letter: r.letter.clone(),
            label: r.label.clone(),
            removable: is_removable(r),
        });
    }
    out
}

fn is_removable(r: &RawVolume) -> bool {
    r.drive_type == DRIVE_REMOVABLE || r.bus == Bus::Usb
}

/// Whether a connected volume is watched: the config entry wins, else
/// internal fixed volumes (plus USB/removable with `includeUsb`).
fn watched(r: &RawVolume, cfg: &Config) -> bool {
    cfg.entry_enabled(&r.id).unwrap_or_else(|| {
        if is_removable(r) {
            cfg.include_usb
        } else {
            r.drive_type == DRIVE_FIXED
        }
    })
}

// ── Win32 adapter ─────────────────────────────────────────────────────────────

/// Production [`VolumeSource`].
pub struct Win32Volumes;

impl VolumeSource for Win32Volumes {
    fn connected(&self) -> Vec<RawVolume> {
        // SAFETY: no arguments; returns a bitmask.
        let mask = unsafe { GetLogicalDrives() };
        (0..26u8)
            .filter(|i| mask & (1 << i) != 0)
            .filter_map(|i| lettered_volume(char::from(b'A' + i)))
            .collect()
    }

    fn resolve_path(&self, path: &str) -> Option<RawVolume> {
        if path.starts_with(r"\\") {
            return Some(network_raw(path));
        }
        let Some(mount) = volume_path_name(path) else {
            log::warn!("volumes: cannot resolve extra path {path}");
            return None;
        };
        if drive_type(&mount) == DRIVE_REMOTE {
            return Some(network_raw(path));
        }
        if let Some(letter) = drive_root_letter(&mount) {
            return lettered_volume(letter);
        }
        let guid = volume_guid(&mount)?;
        let bus = bus_type(guid.trim_end_matches('\\'));
        Some(RawVolume {
            id: VolumeId(guid),
            root: mount.clone(),
            letter: String::new(),
            label: path.to_string(),
            drive_type: drive_type(&mount),
            bus,
        })
    }
}

/// The fixed or removable volume at `letter:`, or `None` for other drive
/// types and volumes without a GUID (e.g. an empty card reader).
fn lettered_volume(letter: char) -> Option<RawVolume> {
    let root = format!("{letter}:\\");
    let dtype = drive_type(&root);
    if dtype != DRIVE_FIXED && dtype != DRIVE_REMOVABLE {
        return None;
    }
    let Some(guid) = volume_guid(&root) else {
        log::debug!("volumes: {root} has no volume GUID; skipped");
        return None;
    };
    Some(RawVolume {
        id: VolumeId(guid),
        letter: format!("{letter}:"),
        label: volume_label(&root),
        drive_type: dtype,
        bus: bus_type(&format!(r"\\.\{letter}:")),
        root,
    })
}

/// `Some('C')` when `mount` is exactly a drive root like `C:\`.
fn drive_root_letter(mount: &str) -> Option<char> {
    let b = mount.as_bytes();
    (b.len() == 3 && b[0].is_ascii_alphabetic() && &b[1..] == b":\\")
        .then(|| char::from(b[0].to_ascii_uppercase()))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn drive_type(root: &str) -> u32 {
    let w = wide(root);
    // SAFETY: `w` is NUL-terminated and outlives the call.
    unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) }
}

/// The mount point (with trailing `\`) of the volume containing `path`.
fn volume_path_name(path: &str) -> Option<String> {
    let w = wide(path);
    let mut buf = [0u16; 1024];
    // SAFETY: `w` is NUL-terminated; `buf` is a valid writable slice.
    unsafe { GetVolumePathNameW(PCWSTR(w.as_ptr()), &mut buf) }.ok()?;
    Some(from_wide(&buf))
}

/// `\\?\Volume{…}\` for a mount point that ends in `\`.
fn volume_guid(mount: &str) -> Option<String> {
    let w = wide(mount);
    let mut buf = [0u16; 64];
    // SAFETY: `w` is NUL-terminated; 50 chars is the documented maximum.
    match unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(w.as_ptr()), &mut buf) } {
        Ok(()) => Some(from_wide(&buf)),
        Err(e) => {
            log::debug!("volumes: no GUID for {mount}: {e}");
            None
        }
    }
}

fn volume_label(root: &str) -> String {
    let w = wide(root);
    let mut name = [0u16; 261];
    // SAFETY: `w` is NUL-terminated; `name` is a valid writable slice.
    let ok = unsafe {
        GetVolumeInformationW(PCWSTR(w.as_ptr()), Some(&mut name), None, None, None, None)
    }
    .is_ok();
    let label = from_wide(&name);
    if ok && !label.is_empty() {
        label
    } else {
        DEFAULT_LABEL.to_string()
    }
}

/// Bus type of the device behind `device` (`\\.\C:` or a GUID path without
/// trailing `\`). Opened with no access rights, so no elevation is needed.
/// Any failure reads as [`Bus::Other`].
fn bus_type(device: &str) -> Bus {
    let w = wide(device);
    // SAFETY: `w` is NUL-terminated; the handle is closed below.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    };
    let handle = match handle {
        Ok(h) => h,
        Err(e) => {
            log::debug!("volumes: open {device} failed: {e}");
            return Bus::Other;
        }
    };
    let bus = query_bus(handle, device);
    // SAFETY: `handle` came from a successful CreateFileW and is closed once.
    if let Err(e) = unsafe { CloseHandle(handle) } {
        log::debug!("volumes: close {device} failed: {e}");
    }
    bus
}

fn query_bus(handle: HANDLE, device: &str) -> Bus {
    #[repr(C, align(8))]
    struct Buf([u8; 1024]);
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut out = Buf([0; 1024]);
    let mut returned = 0u32;
    // SAFETY: input/output pointers and sizes describe live local buffers.
    let res = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some((&raw const query).cast()),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some((&raw mut out).cast()),
            size_of::<Buf>() as u32,
            Some(&mut returned),
            None,
        )
    };
    if let Err(e) = res {
        log::debug!("volumes: storage query on {device} failed: {e}");
        return Bus::Other;
    }
    let offset = std::mem::offset_of!(STORAGE_DEVICE_DESCRIPTOR, BusType);
    if (returned as usize) < offset + size_of::<i32>() {
        log::debug!("volumes: short storage descriptor ({returned} bytes) on {device}");
        return Bus::Other;
    }
    let bytes = [
        out.0[offset],
        out.0[offset + 1],
        out.0[offset + 2],
        out.0[offset + 3],
    ];
    if i32::from_ne_bytes(bytes) == BusTypeUsb.0 {
        Bus::Usb
    } else {
        Bus::Other
    }
}

/// Free bytes available to the caller at `root` (`lpFreeBytesAvailableToCaller`).
/// `None` when the query fails. Blocks for network paths — call those off
/// the UI thread.
pub fn query_free(root: &str) -> Option<u64> {
    let w = wide(root);
    let mut free = 0u64;
    let start = Instant::now();
    // SAFETY: `w` is NUL-terminated; `free` is a valid out pointer.
    let res = unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), None, None) };
    let us = start.elapsed().as_micros();
    match res {
        Ok(()) => {
            log::debug!("poll {root} {us}us free={free}");
            Some(free)
        }
        Err(e) => {
            log::debug!("poll {root} {us}us failed: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VolumeEntry;

    fn raw(letter: &str, guid: &str, drive_type: u32, bus: Bus) -> RawVolume {
        RawVolume {
            id: VolumeId(format!(r"\\?\Volume{{{guid}}}\")),
            root: format!("{letter}\\"),
            letter: letter.to_string(),
            label: DEFAULT_LABEL.to_string(),
            drive_type,
            bus,
        }
    }

    fn with_entry(id: &VolumeId, enabled: bool) -> Config {
        let mut cfg = Config::default();
        cfg.set_enabled(id, "test", enabled);
        cfg
    }

    fn letters(v: &[VolumeInfo]) -> Vec<&str> {
        v.iter().map(|i| i.letter.as_str()).collect()
    }

    #[test]
    fn fixed_internal_included() {
        let c = raw("C:", "c", DRIVE_FIXED, Bus::Other);
        let out = select(std::slice::from_ref(&c), &[], &Config::default());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, c.id);
        assert_eq!(out[0].kind, VolumeKind::Local);
        assert!(!out[0].removable);
    }

    #[test]
    fn usb_fixed_excluded_by_default() {
        let vols = [
            raw("C:", "c", DRIVE_FIXED, Bus::Other),
            raw("E:", "e", DRIVE_FIXED, Bus::Usb),
            raw("F:", "f", DRIVE_REMOVABLE, Bus::Other),
        ];
        let out = select(&vols, &[], &Config::default());
        assert_eq!(letters(&out), ["C:"]);
    }

    #[test]
    fn include_usb_adds_usb_and_removable() {
        let vols = [
            raw("F:", "f", DRIVE_REMOVABLE, Bus::Other),
            raw("C:", "c", DRIVE_FIXED, Bus::Other),
            raw("E:", "e", DRIVE_FIXED, Bus::Usb),
        ];
        let cfg = Config {
            include_usb: true,
            ..Config::default()
        };
        let out = select(&vols, &[], &cfg);
        assert_eq!(letters(&out), ["C:", "E:", "F:"]);
        assert!(out[1].removable && out[2].removable);
    }

    #[test]
    fn entry_enabled_true_forces_in() {
        let usb = raw("E:", "e", DRIVE_FIXED, Bus::Usb);
        let out = select(std::slice::from_ref(&usb), &[], &with_entry(&usb.id, true));
        assert_eq!(letters(&out), ["E:"]);
    }

    #[test]
    fn entry_disabled_forces_out() {
        let c = raw("C:", "c", DRIVE_FIXED, Bus::Other);
        let out = select(std::slice::from_ref(&c), &[], &with_entry(&c.id, false));
        assert!(out.is_empty());
    }

    #[test]
    fn mount_extra_collapses_into_parent() {
        let c = raw("C:", "c", DRIVE_FIXED, Bus::Other);
        // `C:\dev` resolves to C:'s own volume — same id.
        let dev = RawVolume {
            letter: String::new(),
            label: r"C:\dev".to_string(),
            ..c.clone()
        };
        let data = RawVolume {
            id: VolumeId(r"\\?\Volume{d}\".to_string()),
            root: r"C:\mnt\data\".to_string(),
            letter: String::new(),
            label: r"C:\mnt\data".to_string(),
            drive_type: DRIVE_FIXED,
            bus: Bus::Other,
        };
        let out = select(
            std::slice::from_ref(&c),
            &[dev, data.clone()],
            &Config::default(),
        );
        let ids: Vec<_> = out.iter().map(|v| &v.id).collect();
        assert_eq!(ids, [&c.id, &data.id]);
        assert_eq!(out[0].letter, "C:");
    }

    #[test]
    fn network_extra_keyed_by_normalized_path() {
        let c = raw("C:", "c", DRIVE_FIXED, Bus::Other);
        let extras = [network_raw(r"\\NAS\Builds\"), network_raw(r"\\nas\builds")];
        let out = select(&[c], &extras, &Config::default());
        assert_eq!(out.len(), 2, "duplicate network extras collapse");
        let net = &out[1];
        assert_eq!(net.id, VolumeId(r"\\nas\builds".to_string()));
        assert_eq!(net.kind, VolumeKind::Network);
        assert_eq!(net.root, r"\\nas\builds");
        assert_eq!(net.letter, "");
    }

    #[test]
    fn disabled_entry_drops_extra_too() {
        let net = network_raw(r"\\nas\builds");
        let mut cfg = Config::default();
        cfg.volumes.push(VolumeEntry {
            id: net.id.clone(),
            enabled: Some(false),
            ..VolumeEntry::default()
        });
        assert!(select(&[], &[net], &cfg).is_empty());
    }

    #[test]
    fn normalize_network_lowercases_and_trims() {
        assert_eq!(normalize_network(r"\\NAS\Builds\"), r"\\nas\builds");
        assert_eq!(normalize_network(r"\\nas\builds"), r"\\nas\builds");
        assert_eq!(normalize_network(r"Z:\Share\\"), r"z:\share");
    }

    #[test]
    fn drive_root_letter_only_matches_bare_roots() {
        assert_eq!(drive_root_letter(r"c:\"), Some('C'));
        assert_eq!(drive_root_letter(r"C:\dev\"), None);
        assert_eq!(drive_root_letter(r"\\?\Volume{x}\"), None);
    }

    #[test]
    #[ignore = "touches the real machine; run with --include-ignored"]
    fn win32_lists_c_drive() {
        let vols = Win32Volumes.connected();
        let c = vols.iter().find(|v| v.letter == "C:").expect("C: listed");
        assert!(c.id.0.starts_with(r"\\?\Volume{"), "{:?}", c.id);
        assert!(query_free(&c.root).unwrap_or(0) > 0);
        let dev = Win32Volumes.resolve_path(r"C:\Windows").expect("resolves");
        assert_eq!(dev.id, c.id);
    }
}
