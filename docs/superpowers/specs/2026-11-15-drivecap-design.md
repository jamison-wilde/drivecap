# Drivecap — design

**Date:** 2026-11-15 · **Status:** approved 2026-11-15 (tray icon chosen for §7); plan: `docs/superpowers/plans/2026-11-15-drivecap-v1.md`
**Origin:** brainstormed inside the exbar repo as an exbar feature, then split into its own project (see §3).

## 1. Intent

Rust builds (`target/` trees, worktrees, cargo registry) silently fill drives until a build fails. Drivecap gives **early, escalating awareness** of an approaching full volume — knowledge only, no cleanup tooling in v1. Success: the user always knows a drive is getting tight before it bites, without the warning becoming a nag or wallpaper.

## 2. Goals / non-goals / anti-goals

- **Goals:** watch volumes; warn at a threshold and every step below it; escalate as free space falls; debounce re-crossings; small, draggable, always-on-top surface visible over any app.
- **Non-goals (v1):** finding/deleting what used the space (a later direction — leave room); toasts; modal dialogs; a full settings window; % thresholds.
- **Anti-goals:** kitchen-sink scope (the reason exbar was split out — see §3); stealing focus; eating clicks over the user's work; a surface so constant it becomes invisible; nagging about a re-crossed level.

## 3. Approaches considered

| | Approach | Verdict |
|---|---|---|
| A | Separate subsystem inside exbar's process, own window + modules | Viable, but still grows exbar's scope |
| B | Hang off exbar's `ToolbarState` like Recent Folders | Rejected: `toolbar.rs` (1,484) / `wndproc.rs` (1,137) would blow their caps; surface wouldn't exist until Explorer first foregrounds |
| C | Separate process, same installer | Rejected: two-process IPC for little gain |
| **D** | **Separate project `drivecap`** | **Chosen by user.** Little real overlap with exbar (only plumbing: DPI, dark mode, logging, error type, layered-window paint). Avoids the QTTabBar kitchen-sink trap. Plumbing is **copied, not shared** — a shared crate would couple release cycles for ~300 lines. |

## 4. Behaviour (decided)

### Zones (per volume; defaults threshold 20 GB, step 5 GB, dire 2 GB)

| Zone | Surface behaviour |
|---|---|
| Healthy (free > threshold) | Never shown on its account. |
| Level crossing (threshold, then each step: 20/15/10/5) | Fade in immediately (200 ms), hold 6 s, fade out 600 ms. |
| Warning (threshold ≥ free > dire) | Periodic reminder: fade in, hold 4 s, fade out. Interval scales **linearly with free space from 60 s (at threshold) to 10 s (at dire)**. |
| Dire (free ≤ dire) | Stays visible at full opacity. |

- **Debounce:** per volume, remember the last level warned. Free space rising above a level and falling back to it does not re-warn that level. (Builder: define the reset rule — e.g. a level re-arms only after free rises a full step above it. Mark as your call.)
- **Unreachable network folder:** transition reachable → unreachable fades the surface in once and shows the row as `?`, including in minimized mode; after **5 min** (config) the row drops out of minimized mode (stays in expanded). Re-arms if it recovers and drops again.

### Polling (decided)

- Base interval **60 s** (config, floor 1 s) for healthy volumes.
- Warning-zone volumes poll on the same 60 → 10 s scale as reminders; dire volumes every 10 s. No change-based backoff (an idle drive can suddenly start filling).
- Measured cost: `GetDiskFreeSpaceExW`-equivalent 20–50 µs per local volume — UI-thread timer is fine for local volumes.
- **Network folders** probe on a worker thread (pattern: exbar `reachability_probe.rs`, 3 s budget), never the UI thread — dead SMB shares block ~30 s.
- **Verify during build:** the query should not spin up a parked HDD (NTFS answers from in-memory free-cluster count). Test with a parked spinning drive; if it does wake drives, add backoff for healthy HDDs only.
- Log per-poll timing to the log file so dogfooding can tune the default.

### Volumes (decided)

- **Default set:** internal fixed volumes. USB sticks **and USB HDDs** excluded (USB HDDs report `DRIVE_FIXED`; detect bus type via `IOCTL_STORAGE_QUERY_PROPERTY` → `BusTypeUsb`).
- **Extra:** mount points (e.g. `C:\dev`) and network folders. A configured path that is not actually a separate volume collapses into its parent volume's row (no double counting). Note: on the dev machine `C:\dev` currently reports C:'s free space.
- **Per-volume rules:** threshold / step / dire in GB, or `enabled: false`. GB only — % fails on both 4 TB and 128 GB drives.
- **Keying:** by **volume GUID** (`\\?\Volume{…}\`, `GetVolumeNameForVolumeMountPointW`) so a rule follows a USB device across letters. GUID is per-machine (fine). Config stores the GUID plus a human label (`"E: SanDisk 64GB"`) for readability; match on GUID only. Network folders have no GUID → keyed by path. A UI/menu path to add a rule from a list of connected volumes (no GUID typing).

### Snooze (decided)

- Per-row and snooze-all; duration choice 10 min / 1 h / Next level (default 10 min).
- **Break-through:** a new level crossing, or entering dire, clears that volume's snooze.
- A dire volume **can** be snoozed, but **only for a timed duration — never "Next level"** (caps silence on a dire drive).
- Snoozed surface does not fade in for periodic reminders.

## 5. Surface (decided — see `docs/design/`)

Visual spec = **option 2a, stack card** in `docs/design/handoff/README.md` (tokens, sizes, states, timings, halo, popups, click-through rules). Screenshots in `docs/design/screenshots/`. That handoff was written assuming drivecap lived inside exbar; **these overrides apply:**

| Handoff says | Drivecap does |
|---|---|
| "sibling of the exbar toolbar", toolbar font/grip | Standalone app; keep the look (it's a family resemblance), default font `DEFAULT_GUI_FONT` |
| Enabled toggle in exbar's `+` menu; off by default | Installing drivecap is the opt-in. "Disable disk warnings" in settings popup/right-click becomes **Quit drivecap** (or pause — builder's call, mark it) |
| `~/.exbar/config.json` | `~/.drivecap/config.json` |
| Decisions to confirm (minimized drops unreachable; dire snoozable; default bottom-right) | Resolved: unreachable shown for 5 min then dropped (§4); dire snoozable for timed durations only; default bottom-right of primary work area, 12 px inset |

## 6. Constraints

- Rust 2024 edition, `windows` 0.61, GDI, layered top-level window (`WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, topmost). Never takes focus.
- All px through DPI scale; all colours branch on dark mode; work-area clamping; per-monitor position persistence.
- Same engineering conventions as exbar: pure-controller + Win32-adapter split, trait seams with mocks, `catch_unwind` at FFI boundaries, size caps (1,500-line file / 150-line function, prod and test counted separately).
- Per-user MSI (WiX v7), Run-key autostart, no UAC.

## 7. Entry point while nothing is at risk (decided: tray icon)

User chose **tray icon** (2026-11-15).

- `Shell_NotifyIconW` icon, always present while drivecap runs. Glyph is drawn at runtime (small disk shape) coloured by the worst non-snoozed severity: `muted` healthy, `warning`, `dire-accent`; snoozed-all uses `muted` with a "Zz" mark. Tooltip: worst volume + free space (e.g. `Drivecap — C: 14.8 GB free`), or `Drivecap — all drives healthy`.
- **Left-click:** shows the card in **expanded** mode (for that showing only) at full opacity: fade in 200 ms, hold 6 s, then normal hover/linger rules; stays while dire. Left-click again while shown hides it.
- **Right-click:** the card's right-click menu minus row items (Snooze ▸ · Volumes ▸ · Font… · Edit config.json · Quit drivecap).
- Re-add the icon on the `TaskbarCreated` broadcast (Explorer restart).
- Start Menu re-launch of a running instance signals the running one (single-instance guard) to behave like a tray left-click, then exits.

## 8. Left to the builder (mark each as "builder's call" in the plan)

Debounce re-arm rule; module names; exact config schema shape; glyphs for header icons; halo vs. pulsing-border fallback; CLI subcommand names; single-instance mechanism.
