# Drivecap

Drivecap is a small Windows tray app that warns you before a drive fills
up. Build trees, worktrees and package caches quietly eat disk space
until a build fails; drivecap watches your volumes and shows a small,
always-on-top card — one that never steals focus and never eats clicks
while idle — when free space drops below a threshold and again at each
step below it, escalating as space gets tight and staying put when a
drive is nearly full. Re-crossing a level you were already warned about
stays quiet, and any warning can be snoozed. A dire card is interactive
by design: it stays clickable (snooze, drag, menus) for as long as a
drive is dire and not snoozed, instead of turning click-through.

## Install

Download `drivecap-<version>-x64.msi` and run it. It installs per-user
to `%LOCALAPPDATA%\Drivecap\` (no admin prompt), starts drivecap at
login, adds a Start Menu shortcut, and offers to launch it right away.
Uninstall from Settings → Apps.

To build the MSI yourself: `./scripts/build-msi.sh` (needs Rust and
WiX v7 with the Util and UI extensions) → `target/wix/drivecap-<version>-x64.msi`.

## Configuration

Settings live in `~/.drivecap/config.json` (`%USERPROFILE%\.drivecap\`),
written with defaults on first run: per volume, warn at 20 GB free, then
every 5 GB below that, dire at 2 GB. Edit it by hand or via the tray
menu's **Edit config.json**; drivecap picks up changes automatically.
The card's position is saved in `~/.drivecap/position.json`. Logs go
to `%TEMP%\drivecap.log`; at startup a log over 5 MB moves to
`drivecap.log.1`, replacing the previous one.

## Using it

- The tray icon is always present while drivecap runs, coloured by the
  worst drive; hover it for the worst at-risk drive's free space (or
  "all drives healthy").
- **Left-click** the tray icon to show the card (expanded); click again
  to hide it.
- **Right-click** the tray icon (or the card) for Snooze, Volumes, Font,
  Edit config.json and Quit drivecap.
- Drag the card to move it; it remembers where you put it, per monitor.
- Card header buttons: **Zz** snoozes every at-risk drive and hides the
  card (click again to wake them), the half-filled circle sets idle
  opacity, the gear opens settings (snooze length, font, config, quit),
  and the chevron switches between all drives and warned drives only.
- Each warning row has its own **Zz** to snooze just that drive. A new
  level crossing, or a drive going dire, breaks through any snooze.
- **Hide this volume** (right-click a row) stops watching it. Lettered
  drives come back via **Volumes ▸**; network folders and mount points
  from `extraPaths` are not listed there, so un-hide one by deleting its
  `"enabled": false` entry under `volumes` in `config.json` (or setting
  it to `true`).

`drivecap status` prints the watched volumes and their free space.

## License

MIT — see `LICENSE`.
