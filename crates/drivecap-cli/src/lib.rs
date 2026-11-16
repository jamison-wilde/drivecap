//! # drivecap — always-on-top disk-space warner for Windows.
//!
//! Drivecap watches volume free space and shows a small, draggable,
//! always-on-top card that never takes focus. Warnings escalate as free
//! space falls past a threshold and each step below it, are debounced
//! against re-crossings, and can be snoozed. A tray icon is the entry
//! point while nothing is at risk.
//!
//! ## Runtime architecture
//!
//! A single `drivecap.exe` process runs after login (started via the
//! `HKCU\…\Run\Drivecap` key written by the MSI, as `drivecap.exe run`).
//! Pure, unit-tested cores hold every decision; thin Win32 adapters
//! execute the effects those cores return (see
//! `docs/adrs/ADR-0001-pure-controller-adapter-pattern.md`). Win32
//! surfaces that need mocking sit behind `Box<dyn Trait>` seams
//! (`docs/adrs/ADR-0002-trait-seams-via-box-dyn.md`).
//!
//! ## Module map
//!
//! **Plumbing** (ported from exbar):
//! - [`theme`] — DPI scaling and (uncached) dark-mode detection.
//! - [`mod@log`] — `log` crate sink writing to `%TEMP%\drivecap.log`.
//! - [`error`] — unified [`error::DrivecapError`] + [`error::DrivecapResult`].
//! - [`clock`] — [`clock::Clock`] time-source seam.
//! - [`paths`] — `~/.drivecap/` state layout.
//!
//! **Pure cores:**
//! - [`config`] — `config.json` schema, defaults, clamping, safe load/save.
//! - [`alerts`] — zones, warning levels, debounce, snooze, reminder schedule.
//! - [`palette`] — light/dark design tokens with config overrides.
//! - [`model`] — surface view model: sorted rows, title, state dot, halo.
//! - [`visibility`] — fade/hold/linger, hover rest, pinning and click-through.
//! - [`layout`] — card geometry, DPI metrics, hit testing, popup anchors.
//! - [`position`] — per-monitor position store, work-area clamping, resize anchoring.
//!
//! **Win32 adapters:**
//! - [`volumes`] — volume enumeration (GUID, label, USB bus), pure
//!   selection rules, local free-space query.
//!
//! The remaining pure cores (controller, menu) and Win32 adapters
//! (net probe, paint, card window, tray, popups) land in later
//! tasks; `CLAUDE.md` lists the planned set.
//!
//! ## Where to read next
//!
//! - `docs/adrs/README.md` — architectural decision records.
//! - `CLAUDE.md` (repo root) — operational gotchas, build loop,
//!   `windows = 0.61` quirks.

pub mod alerts;
pub mod clock;
pub mod config;
pub mod error;
pub mod layout;
pub mod log;
pub mod model;
pub mod palette;
pub mod paths;
pub mod position;
pub mod theme;
pub mod visibility;
pub mod volumes;
