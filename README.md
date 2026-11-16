# Drivecap

Drivecap is a small Windows tray app that warns you before a drive fills
up. Build trees, worktrees and package caches quietly eat disk space
until a build fails; drivecap watches your volumes and shows a small,
always-on-top card — one that never steals focus or eats clicks — when
free space drops below a threshold and again at each step below it,
escalating as space gets tight and staying put when a drive is nearly
full. Re-crossing a level you were already warned about stays quiet,
and any warning can be snoozed.

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
to `%TEMP%\drivecap.log`.

## Using it

- The tray icon is always present while drivecap runs, coloured by the
  worst drive; hover it for the lowest free space.
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

`drivecap status` prints the watched volumes and their free space.

## License

MIT — see `LICENSE`.
