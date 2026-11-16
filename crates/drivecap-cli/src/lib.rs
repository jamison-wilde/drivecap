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
//! - [`raster`] — per-pixel card mask, premultiply, halo and border pulse.
//! - [`layout`] — card geometry, DPI metrics, hit testing, popup anchors.
//! - [`position`] — per-monitor position store, work-area clamping, resize anchoring.
//! - [`polling`] — watched-volume collection: scan reconcile, poll scheduling, snooze.
//! - [`controller`] — the app controller: inputs → effects (ADR-0001).
//! - [`menu`] — right-click menu model and item id → command mapping.
//!
//! **Win32 adapters:**
//! - [`volumes`] — volume enumeration (GUID, label, USB bus), pure
//!   selection rules, local free-space query.
//! - [`net_probe`] — worker-thread network free-space probe (3 s budget).
//! - [`paint`] — GDI card painter: fonts, 32-bpp DIB, `UpdateLayeredWindow`.
//! - [`card_wnd`] — the layered card window, wndproc, timers, message
//!   pump and [`card_wnd::apply_effects`].
//! - [`single_instance`] — named-mutex guard and the `Drivecap.Show`
//!   relaunch signal.
//! - [`tray`] — notification-area icon: runtime-drawn severity glyph,
//!   tooltip, version-4 callbacks.
//! - [`contextmenu`] — `TrackPopupMenu` wrapper (submenus, checks).
//! - [`popup_wnd`] — opacity / settings popups: geometry, slider,
//!   dismissal rules, the popup window class and its renderer.
//! - [`font`] — `ChooseFontW` font picker.
//!
//! ## Where to read next
//!
//! - `docs/adrs/README.md` — architectural decision records.
//! - `CLAUDE.md` (repo root) — operational gotchas, build loop,
//!   `windows = 0.61` quirks.

pub mod alerts;
pub mod card_wnd;
pub mod clock;
pub mod config;
pub mod contextmenu;
pub mod controller;
pub mod error;
pub mod font;
pub mod layout;
pub mod log;
pub mod menu;
pub mod model;
pub mod net_probe;
pub mod paint;
pub mod palette;
pub mod paths;
pub mod polling;
pub mod popup_wnd;
pub mod position;
pub mod raster;
pub mod single_instance;
pub mod theme;
pub mod tray;
pub mod visibility;
pub mod volumes;
