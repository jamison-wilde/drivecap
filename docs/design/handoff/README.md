# Handoff: Disk Space Warning Surface (exbar)

## Overview

A small always-on-top surface that warns when a watched volume is running low on free space. It is opt-in, off by default, and sits beside the existing exbar toolbar as a sibling window. This document specifies the chosen design: option **2a** in `Disk Space Warnings.dc.html`.

The behaviour rules (zones, level crossings, reminder intervals, per-volume rules) come from the original design request, `2026-11-15-disk-space-warnings-design-request.md`, and are restated here only where the design depends on them.

## About the design files

`Disk Space Warnings.dc.html` is a **design reference written in HTML**. It is not code to port. The target is the existing exbar stack: a Win32 layered top-level window (`WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, topmost), GDI rendering, per-pixel alpha, timer-driven animation. Recreate the look and behaviour there using the toolbar's existing patterns for DPI scaling, dark-mode branching, work-area clamping and position persistence.

Open the file in a browser (keep `support.js` next to it). Section **2** at the top is the final design; every specimen in it is clickable. Section **1** below holds the three earlier explorations (pill, stack card, edge gauge) and is context only.

## Fidelity

High-fidelity for layout, colour, type sizes and states. All pixel values below are at 100% scale (96 DPI) and must be DPI-scaled. The HTML uses Segoe UI 12 px as a stand-in for `DEFAULT_GUI_FONT`; use the real toolbar font by default.

Two things are approximated in HTML and need judgement in GDI:

- Drop shadows. Use whatever the toolbar popups already do; a 1 px border alone is acceptable.
- The header icons (opacity, gear, chevron) are quick sketches. Replace them with glyphs that match the toolbar's icon style.

## The surface

One window shows all volumes. It has two modes, switched by the chevron in the header.

- **Expanded**: one row for every configured drive and mount, including healthy ones.
- **Minimized**: the same card, but only rows currently in warning or dire. Snoozed, healthy and unreachable rows are omitted.

The surface is never shown while every volume is healthy. Healthy rows are extra context once it is visible for another reason.

### Card

- Width 270. Height is header plus rows.
- Background `surface`, 1 px border `border`, corner radius 8.
- Rows are sorted worst first: dire, warning, healthy, unreachable.

### Header (height 28)

Left to right, vertically centred, left padding 9, right padding 3:

1. **Grip**: 6 × 12 dotted grip (2 columns × 3 rows of 2 px dots, 3 × 4 px pitch), text colour at 55% alpha. Match the toolbar's dotted grip. 6 px gap after it.
2. **State dot**: 8 px circle. Colour is the worst non-snoozed severity: `dire-accent`, `warning`, or `muted` when everything at risk is snoozed. 6 px gap after it.
3. **Title**: semibold. "Disk space", or "Snoozed · 10 min" (using the configured duration) when everything at risk is snoozed.
4. Four **icon buttons**, right-aligned, each 22 × 22 with radius 4 and 2 px gaps:
   - **Zz**: snooze all (text glyph, 10 px semibold)
   - **Opacity**: half-filled circle; opens the opacity popup
   - **Gear**: opens the settings popup
   - **Chevron**: points up when expanded ("Show warned drives only"), down when minimized ("Show all drives")

Icon button states: hover fills with `hover`. Active (popup open, or snooze-all engaged) inverts: background `text`, glyph `surface`.

### Row

Each row has a 1 px top border in `border`. Padding: 4 top, 6 right, 8 bottom, 10 left. Two lines:

**Line 1** (20 px tall, 6 px column gap):

| Column | Width | Content |
|---|---|---|
| Letter | 24 | `C:` bold |
| Label | flexible | Human label (`Windows`, `SanDisk 64GB`, `\\nas\builds`), regular weight, ellipsis when too long |
| Value | auto | Free space (`14.8 GB`), semibold, tabular digits |
| Snooze | 20 | 20 × 20 "Zz" button, radius 4. Only on warning and dire rows; the column stays reserved on other rows so values align. |

**Line 2**: the runway bar, 3 px below line 1. Height 3, radius 2, full row width less 4 px on the right.

### Runway bar

The bar shows runway, not disk usage. Fill width = `free / threshold`, clamped to 0–100%. With the default 20 GB threshold, 14.8 GB free fills 74%.

Gaps mark the step levels. Each is 1 px wide in the row's background colour, placed at `level / threshold`. With defaults (15 / 10 / 5 GB) that is 75%, 50% and 25%. Each level crossing is then visible as the fill passing a gap. Compute gap positions from the volume's own rule, not fixed quarters.

### Row states

| State | Row background | Letter | Label | Value | Bar fill | Bar track |
|---|---|---|---|---|---|---|
| Healthy | `surface` | `text` | `text-secondary` | `text` | `muted`, 100% | `track` |
| Warning | `surface` | `text` | `text-secondary` | `warning-text` | `warning` | `track` |
| Dire | `dire` | white | white | white | white | white at 35% alpha |
| Unreachable | `surface` | `muted` | `muted` | `?` in `muted` | none | dashed: 3 px `muted`, 3 px gap |
| Snoozed | `surface` | `muted` | `muted` | `Zz 10 min` in `muted` | none | none |

A snoozed row's Zz button is drawn in the active (inverted) style. The value text uses the configured duration: `Zz 10 min`, `Zz 1 h` or `Zz next level`.

### Dire treatment

- The dire row is solid `dire` red with white content, in both light and dark mode.
- While any non-snoozed volume is dire, the whole card shows a slow halo: an outline in `dire-accent` that grows from 0 to 8 px outside the card edge while fading from 60% to 0% alpha over about 1.7 s, then rests until 2.4 s and repeats. There is no opacity flashing of the content.
- The window needs transparent margin around the card to draw the halo.
- If the halo is costly in GDI, a 2 px `dire-accent` border whose alpha eases between 100% and 40% on the same 2.4 s period is an acceptable substitute.

### Opacity popup

Opened by the opacity button. A separate topmost popup anchored under the header, right-aligned to the opacity button. It must not inherit the card's alpha.

- 200 wide, padding 8 × 10, radius 8, `surface` background, 1 px `border`.
- Line 1: "Idle opacity" (11 px, `text-secondary`) on the left, the current value ("50%", semibold, `text`) on the right.
- Line 2: a slider, 20–100% in steps of 5.
- While the popup is open the card is drawn at the slider's value, so the user sees the result live.
- Closes on click outside or Esc. The value is written to config on close.

### Settings popup

Opened by the gear. Same popup style, 230 wide, padding 10, 10 px between groups.

1. **Snooze for** (11 px label), then three choices in a row with 4 px gaps: `10 min`, `1 h`, `Next level`. Each is padded 3 × 8 with a 1 px border and radius 4. The selected one is inverted (background `text`, label `surface`). This sets the duration used by every one-click snooze.
2. **Font** (11 px label) with the current face and size beneath ("Segoe UI, 9 pt"), and a `Change…` button on the right that opens the standard `ChooseFont` dialog.
3. A 1 px divider.
4. `Edit config.json`: opens `~/.exbar/config.json`.
5. `Disable disk warnings`: same effect as the toggle in the `+` button's right-click menu.

### Right-click menu

Right-click anywhere on the card: Snooze ▸ (10 minutes / 1 hour / Until next level) · Hide this volume (when over a row) · Font… · Settings (edit config) · Disable disk warnings. Use the toolbar's existing submenu popups.

## Interactions and behaviour

### Visibility and timing

| Event | Behaviour |
|---|---|
| Level crossing | Fade in 200 ms, hold 6 s, fade out 600 ms |
| Periodic reminder (warning zone) | Fade in 200 ms, hold 4 s, fade out 600 ms. Interval runs 60 s → 10 s, linear with free space. |
| Dire | Stays visible at full opacity |
| Hover or mouse-down | Cancels any fade-out and goes to full opacity |
| Mouse leaves or drag ends | Lingers 3 s, then fades out |

"Idle opacity" (default 50%) is the alpha used while the surface is showing but the cursor is not over it. Hovering raises it to 100% over 200 ms.

### Click-through

- Click-through while idle or fading (`WS_EX_TRANSPARENT`).
- Becomes interactive once the cursor has rested inside the window bounds for 150 ms, polled on the existing animation timer. It stays interactive until the linger ends.
- Always interactive while dire.
- Never activates or takes focus.

### Dragging

Drag by the grip or any empty header area. Position persists per monitor and is clamped to the work area, as the toolbar does. Default position before the first drag: bottom-right of the primary work area, 12 px in from both edges.

### Snooze

- **Row Zz**: snoozes that volume for the configured duration. Clicking it again wakes the volume.
- **Header Zz**: snoozes every warning and dire volume. Clicking it again wakes them all.
- When every at-risk volume is snoozed, the title reads "Snoozed · <duration>" and the state dot turns `muted`. In minimized mode the card is then header-only. In expanded mode the rows stay, with the snoozed ones greyed.
- A snoozed surface does not fade in for periodic reminders.
- **Break-through**: a new level crossing, or a volume entering dire, clears that volume's snooze and shows the surface immediately.
- A volume that is already dire can be snoozed. Only a further crossing breaks through.
- "Next level" has no timer. It lasts until the break-through rule fires.

### Minimize and expand

The chevron toggles the mode, and the mode persists in config. Changing mode resizes the window. Keep the bottom edge anchored when the surface is in the lower half of the work area and the top edge anchored otherwise, so it does not jump under the cursor.

## State

Per volume (keyed by volume ID): free bytes, reachable flag, last level warned (for debounce), snoozed-until (a timestamp, or "next level"), hidden flag.

Per surface: mode (expanded or minimized), position per monitor, visibility phase (hidden, fading in, holding, fading out, lingering), hover and drag flags, which popup is open.

Derived:

- severity per volume: healthy, warning, dire or unreachable
- worst non-snoozed severity, for the state dot and the halo
- "all at-risk volumes snoozed", for the title and the header Zz active state
- the row list: every volume when expanded, non-snoozed warning and dire volumes when minimized

## Configurable properties

**First-class (reachable from the surface or its menus)**

| Property | Default | Where |
|---|---|---|
| Idle opacity | 50% | Opacity popup |
| Snooze duration | 10 min | Settings popup, right-click ▸ Snooze |
| Font face, size, weight | toolbar font | Settings popup, right-click ▸ Font… |
| Expanded or minimized | expanded | Header chevron |
| Hide a volume | none hidden | Right-click a row |
| Enabled | off | `+` button menu, settings popup, right-click |

**Config file only (`~/.exbar/config.json`)**

| Property | Default |
|---|---|
| Per-volume threshold / step / dire (GB) | 20 / 5 / 2 |
| Watched volumes, extra mount points, network folders | internal fixed drives |
| Colours: warning, dire, text, background (light and dark sets) | see tokens |
| Scale | 100% |
| Fade in / hold / crossing hold / fade out / linger | 200 ms / 4 s / 6 s / 600 ms / 3 s |
| Reminder interval range | 60 s → 10 s |
| Dire pulse on or off | on |
| Level gaps in the bar on or off | on |
| Position per monitor | bottom-right, 12 px inset |

## Design tokens

| Token | Light | Dark |
|---|---|---|
| `surface` | `#F9F9F9` | `#2B2B2B` |
| `border` | black, 16% alpha | white, 16% alpha |
| `text` | `#1B1B1B` | `#FFFFFF` |
| `text-secondary` | `#575757` | `#C8C8C8` |
| `muted` | `#6B6B6B` | `#A0A0A0` |
| `track` | black, 14% alpha | white, 18% alpha |
| `hover` | black, 6% alpha | white, 8% alpha |
| `warning` (bar fill, state dot) | `#C77800` | `#F5B83D` |
| `warning-text` (value) | `#8A5100` | `#F5B83D` |
| `dire` (row background) | `#C42B1C` | `#C42B1C` |
| `dire-accent` (state dot, halo) | `#E5372B` | `#E5372B` |

Type: toolbar font. In the mock, body is 12 px, small labels 11 px, Zz glyph 10 px. Weights are regular, semibold (title, values) and bold (drive letter).

Radii: card and popups 8, buttons and chips 4, bar 2.

## Decisions worth confirming

These were proposed during design and not explicitly signed off:

- Unreachable network shares are omitted from minimized mode.
- A dire volume can be snoozed.
- Default position is the bottom-right of the primary work area.

## Assets

None. The grip is drawn, the icons are placeholders, and "Zz" is text.

## Files

- `Disk Space Warnings.dc.html`: the design reference. Section 2 (option 2a) is final.
- `support.js`: runtime needed to open the file above.
- `2026-11-15-disk-space-warnings-design-request.md`: the original request with the behaviour rules.
