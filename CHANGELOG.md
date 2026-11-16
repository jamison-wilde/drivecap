# Changelog

All notable changes to Drivecap are documented here. Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Always-on-top, never-focus-stealing stack card (layered window, per-pixel alpha) that warns when a watched volume drops below its threshold (default 20 GB), again at every step below it (default 5 GB), and stays pinned with a slow halo once a drive is dire (default 2 GB).
- Debounced level crossings: a level re-arms only after free space rises a full step above it.
- Periodic reminders that speed up from every 60 s at the threshold to every 10 s near dire.
- Per-row and snooze-all (10 min / 1 h / next level); new crossings and dire break through. Snooze-all hides the card immediately.
- Severity-coloured tray icon with tooltip; left-click shows the card, right-click opens Snooze, Volumes, Font…, Edit config.json and Quit.
- Opacity popup with live preview and settings popup (snooze length, font picker, config, quit).
- Watches internal fixed drives by default, excluding USB sticks and USB hard drives; opt volumes in or out from the Volumes menu. Rules follow a volume's GUID across drive letters.
- Extra mount points and network folders via `extraPaths`; network free space is probed off the UI thread with a 3 s budget, unreachable shares show `?`.
- Click-through while idle, interactive after the cursor rests on the card; drag to move, position remembered per monitor; DPI- and dark-mode-aware.
- `~/.drivecap/config.json` with defaults written on first run and live reload on edit; malformed files are never overwritten.
- `drivecap status` command, single-instance guard (relaunching shows the card), per-user MSI with login autostart.
