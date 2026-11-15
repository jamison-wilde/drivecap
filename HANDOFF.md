# Drivecap — session handoff (2026-11-15)

Start here in a fresh session opened at `c:\dev\claude\drivecap`.

## What drivecap is

A standalone Windows always-on-top disk-space warner (tray icon still undecided — spec §7), split out of the exbar project (`c:\dev\claude\exbar`) to keep exbar focused. Full design: **`docs/superpowers/specs/2026-11-15-drivecap-design.md`** — read it first.

## State of this repo

- `git init` done by the user. No commits yet. **No remote yet** — GitHub remote is added after v1 is ready. Never push without an explicit ask.
- Already in place:
  - `.gitignore` (target, tmp/, .worktrees/, *.local.md, .claude/, proptest-regressions)
  - `docs/superpowers/specs/2026-11-15-drivecap-design.md` — consolidated design (status: awaiting user approval)
  - `docs/design/design-request.md` — original design request
  - `docs/design/handoff/` — designer handoff README (option 2a spec), `Disk Space Warnings.dc.html` + `support.js` (open in a browser; section 2 is final)
  - `docs/design/screenshots/` — 3 screenshots of option 2a (expanded/snoozed, opacity + settings popups, minimized idle)
- Claude memory for this project was seeded from exbar's (`~/.claude/projects/c--dev-claude-drivecap/memory/`): branches not worktrees, install via MSI, never push without asking, no version bump on merge, no `gh` CLI, don't kill running processes for routine ops, skip brainstorming for simple fixes.

## Next steps (in order)

1. **Get the user's answer to the open question** in spec §7 (tray icon / Start Menu re-launch / both). Update the spec.
2. **Get the user's approval of the spec**, then invoke `superpowers:writing-plans`. Plan header needs the "Touched files" table and an Execution policy block (see `c:\dev\claude\CLAUDE.md`).
3. The plan's first phase is the **scaffold port** below; later phases build the feature.

## Scaffold port from exbar

Copy, then rename `exbar`→`drivecap` / `Exbar`→`Drivecap` throughout. Exbar paths are relative to `c:\dev\claude\exbar`.

| From exbar | To drivecap | Notes |
|---|---|---|
| `Cargo.toml` | `Cargo.toml` | workspace, member `crates/drivecap-cli` |
| `crates/exbar-cli/Cargo.toml` | `crates/drivecap-cli/Cargo.toml` | Keep `[lints.*]` gates, edition 2024, `windows` 0.61. Version `0.1.0`. Drop deps/features not needed (COM/Shell/Ole/DataExchange likely); add `Win32_System_Ioctl` / `Win32_Devices_*` as needed for bus-type query, `Win32_UI_Controls_Dialogs` for `ChooseFontW`, `Win32_UI_Shell` for tray (`Shell_NotifyIconW`) if chosen. Description string. |
| `crates/exbar-cli/build.rs` | same | winres metadata |
| `crates/exbar-cli/src/bin/exbar.rs` | `src/bin/drivecap.rs` | Keep: clap CLI, `FreeConsole`, logger init, `OleInitialize`-free message pump. Drop: WinEvent hook, install/uninstall. Subcommands e.g. `run` (production, from Run key) + `status`. Add single-instance guard (named mutex). |
| `src/theme.rs` (92) | same | DPI scale + dark-mode detection |
| `src/log.rs` (114) | same | log file → `%TEMP%\drivecap.log` |
| `src/error.rs` (77) | same | `ExbarError` → `DrivecapError`, trim variants |
| `src/clock.rs` (79) | same | Clock trait + mock — needed for snooze/reminder timing tests |
| `src/contextmenu.rs` (73) | same | right-click menu |
| `src/paths.rs` (126) | same | `~/.drivecap/`; drop legacy migration |
| `src/position.rs` (398) | adapt | per-monitor position + `clamp_to_work_area`; schema differs (per monitor, not per target kind) |
| `src/reachability_probe.rs` (154) | adapt | worker-thread probe with 3 s budget → network free-space query |
| `src/paint.rs` / `submenu_wnd.rs` | reference only | layered-window + per-pixel alpha + dark palette patterns |
| `crates/exbar-cli/wix/main.wxs` | `crates/drivecap-cli/wix/main.wxs` | **New UpgradeCode: `F04B1403-DE21-41D0-9AB5-3C30AA482CAD`** — fixed forever, never reuse exbar's. Run key `HKCU\...\Run\Drivecap = "...drivecap.exe" run`, `util:CloseApplication` target `drivecap.exe`, `Software\Drivecap`, install dir `%LOCALAPPDATA%\Drivecap`, Start Menu shortcut, "Launch Drivecap now" checkbox. |
| `scripts/build-msi.sh`, `doc-check.sh`, `extract-changelog.sh` | `scripts/` | rename paths/names |
| `.github/workflows/ci.yml` | same | lint / test / doc-check / build-msi on PR + tag |
| `CLAUDE.md` | `CLAUDE.md` | Rewrite for drivecap; keep Gotchas that apply (windows 0.61 quirks, `hCursor` on window class, `FreeConsole` + no `println!`, `catch_unwind` at FFI, DPI/dark-mode rules, pre-push gate) |
| `docs/adrs/ADR-0003`, `0004`, `0006` | `docs/adrs/` | pure-controller pattern, trait seams, per-user MSI — still apply |
| `README.md`, `CHANGELOG.md` | new | write fresh |

## Commands (once scaffolded)

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release -p drivecap-cli
cargo test
cargo fmt && cargo clippy --all-targets && cargo test && ./scripts/doc-check.sh   # pre-push gate, all four must pass
./scripts/build-msi.sh    # → target/wix/drivecap-<version>-x64.msi; user installs the MSI to test
```

WiX v7 is already installed on this machine (used by exbar). Use node, not python, for scripts; temp files in `./tmp/`; no Windows paths in bash `ls`.

## Facts established during the brainstorm

- Free-space query cost on this machine: C: 48 µs, D: 21 µs, E: 31 µs per call (Node `statfsSync` benchmark).
- C: had 14.5 GB free at the time — under the default 20 GB threshold, so v1 will warn immediately on first run.
- `C:\dev` reported C:'s free space — likely not a separate mount on this machine.
