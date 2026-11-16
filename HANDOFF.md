# Drivecap — session handoff (2026-11-15, v1 built)

Start here in a fresh session opened at `c:\dev\claude\drivecap`.

## What drivecap is

A standalone Windows tray app + always-on-top "stack card" disk-space warner, split out of exbar. Design: `docs/superpowers/specs/2026-11-15-drivecap-design.md` (approved; tray icon chosen for §7). Visual spec: `docs/design/handoff/README.md` (option 2a). Implementation plan (executed): `docs/superpowers/plans/2026-11-15-drivecap-v1.md`.

## State

- Branch `feat/v1` off `main` holds all of v1 (Tasks 1–15). Not merged, not pushed. Remote (later, at v1 release): https://github.com/jamison-wilde/drivecap — never push without an explicit ask.
- Gate green: `cargo fmt && cargo clippy --all-targets && cargo test && ./scripts/doc-check.sh`.
- Live checks done by the user: card placement/fade (Task 12); tray, menus, popups, snooze-all hide (Task 13–14); chevron on a tray-shown card fixed after the live check.
- MSI: `./scripts/build-msi.sh` → `target/wix/drivecap-0.1.0-x64.msi`.

## Dogfood checklist (Task 15 — record results here)

- [ ] Install MSI; "Launch Drivecap now" checkbox starts it; login autostart via Run key; Start Menu shortcut re-shows the card when already running.
- [ ] Explorer restart (`TaskbarCreated`) → tray icon returns.
- [ ] Dark ↔ light toggle mid-run repaints card + tray.
- [ ] 150 % DPI monitor + cross-monitor drag; position restored per monitor after restart.
- [ ] Card never steals focus while typing; click-through when idle. Font dialog: focus returns to your app?
- [ ] USB stick / USB HDD: excluded by default; Volumes ▸ opt-in follows the GUID across letters.
- [ ] Network folder in `extraPaths`: unplug → `?` row, drops from minimized after 5 min, no UI freeze.
- [ ] Parked HDD (spec §4): watch for spin-up on polls; if it wakes, file follow-up for healthy-HDD backoff.
- [ ] Dire: set `direGb` above current free in config → halo, pinned, Next level greyed, snooze timed only.
- [ ] `drivecap status` from cmd/PowerShell prints (AttachConsole path in release build).
- [ ] MSI upgrade over a running instance closes it cleanly (WM_CLOSE) — no ghost tray icon.

## Known follow-ups (deferred minors from reviews)

See the "minor (deferred)" lines summarised in the final review; notable: probes are sequential (N dead shares delay others ~3 s each); non-collapsing drive-root extra labels show the path instead of the volume name; installer WM_CLOSE quits immediately even with the font dialog open.

## Commands

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release -p drivecap-cli
cargo test
cargo fmt && cargo clippy --all-targets && cargo test && ./scripts/doc-check.sh   # pre-push gate
./scripts/build-msi.sh    # → target/wix/drivecap-<version>-x64.msi; user installs the MSI to test
./target/release/drivecap.exe --log-level debug run    # global flags go before the subcommand
```

Use node, not python, for scripts; temp files in `./tmp/`; no Windows paths in bash `ls`. A drivecap test instance may be closed with graceful `taskkill //im drivecap.exe` (no /f) to free the exe for rebuilds.
