# Design request — Disk-space warning surface

**Date:** 2026-11-15 · **Status:** request for visual/interaction design · **Feature:** Disk Space Warnings (opt-in, off by default)

## 1. Why this exists

The user builds Rust. `target/` trees, worktrees, and the cargo registry silently eat tens of GB until a build fails or Windows complains. The goal is **early, escalating awareness** of an approaching full disk — not cleanup tooling (that's a later direction). Success = the user always knows a drive is getting tight *before* it bites, without the warning becoming wallpaper or a nag.

## 2. Behavior already decided (design must express these)

| Zone | Trigger | What the surface does |
|---|---|---|
| Healthy | free > threshold (default 20 GB) | Nothing. Never visible. |
| Level crossing | free drops past threshold or each step below it (default every 5 GB: 20/15/10/5) | Fades in **immediately**, holds, fades out. A re-crossed level already warned is debounced (no repeat). |
| Warning | between threshold and dire | Periodic reminder fade-ins; interval shrinks linearly with free space, **60 s → 10 s** (configurable). |
| Dire | free ≤ dire level (default 2 GB) | Stays visible (or re-shows at the 10 s floor) — effectively persistent. |

- Scope: internal fixed drives by default; USB sticks / USB HDDs excluded unless opted in. Extra mount points (`C:\dev`) and network folders can be added.
- Per-volume rules (threshold / step / dire, in GB — no %) keyed by volume ID so a rule follows a USB device across letters. Display uses a human label (`E: SanDisk 64GB`).
- Multiple volumes can be at risk at once — the surface must show more than one.
- Independent of the Explorer toolbar: visible over **any** foreground app (always-on-top), not just Explorer.

## 3. What we want designed

A small always-on-top warning surface. Please propose 2–3 directions; starting points below are suggestions, not constraints.

### Candidate directions

1. **Pill** — compact rounded capsule per at-risk volume: `C:  14.8 GB` + thin fill bar. Idle at ~50% opacity; full opacity on hover.
2. **Stack card** — one card, one row per at-risk volume, sorted worst-first; severity color per row; header row shows overall state.
3. **Edge gauge** — slim vertical/horizontal bar docked to a screen edge, segments per volume; expands to labels on hover.

### Interaction requirements

- **Draggable** anywhere (or via a grip consistent with the toolbar's dotted grip — designer's call). Position persists, per monitor.
- **Hover/press holds it:** mouse-over or mouse-down cancels the fade-out and keeps it solid so a drag can complete; after release it lingers a few seconds before fading.
- **Snooze:** a snooze affordance (e.g. 💤 icon revealed on hover) hides it for N minutes (default 10). A new level crossing or entering dire **breaks through** a snooze. Optional extra choices via right-click: 10 min / 1 h / until next level.
- **Right-click menu** (proposal): Snooze ▸ · Hide this volume · Font… · Settings (edit config) · Disable disk warnings.
- Click-through vs. interactive is part of the design question — an idle surface at 50% that still eats clicks over someone's editor may annoy; consider click-through when idle and interactive on hover.

### Visual states to design

- Fade-in, hold, fade-out (timings as proposals).
- Warning vs. dire (dire must be unmistakable — color, weight, maybe a slow pulse; avoid seizure-y flashing).
- Single volume vs. 3+ volumes.
- Snoozed (if anything is shown at all) and hover/pressed/dragging.
- Network volume whose share is unreachable (greyed, "?" value).
- Light and dark mode.

### Configurability (user wants this)

- **Font face, size, weight** — via the standard Windows font picker (`ChooseFont`, the "font box") from the right-click menu; stored in config.
- **Colors** — warning color, dire color, text, background; defaults for light and dark.
- **Size/scale** and **idle opacity** (default 50%).
- Fade/hold durations.
Please mark which knobs are first-class (menu) vs. config-file-only.

## 4. Constraints

- Win32 layered top-level window (`WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, topmost), GDI rendering — same stack as the existing toolbar and submenu popups. No WinUI/XAML, no web view. Per-pixel alpha is available; animation is timer-driven.
- Must **never steal focus** from the app being used.
- All pixel values DPI-scaled; all colors branch on dark mode. Multi-monitor and work-area clamping like the toolbar.
- Current toolbar font is `DEFAULT_GUI_FONT`; design may diverge but should feel like a sibling of the exbar toolbar.
- Enable/disable lives in the `+` button right-click menu (next to "Enable Recent Folders"); detailed settings in `~/.exbar/config.json`. No full settings window exists yet.

## 5. Out of scope

- Cleanup actions (finding/deleting `target/` folders) — possible later direction; leave room, don't design it.
- Windows toast notifications and modal dialogs — rejected in favor of a custom surface.

## 6. Deliverables requested

- 2–3 directions as mockups (light + dark, warning + dire, 1 and 3 volumes).
- Recommended direction with interaction spec: states, timings, hit areas, click-through rules, snooze flow.
- List of configurable properties with defaults.

## 7. Open questions for the designer

- Where does it appear by default (corner? near the tray? near the toolbar?) before the user drags it?
- One surface for all volumes, or one per volume?
- In dire mode, persistent vs. re-show-every-10 s — which reads better?
- Does a snoozed surface leave a trace (tiny dot/badge) so the user knows warnings are suppressed?
