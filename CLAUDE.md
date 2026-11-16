# CLAUDE.md

Orientation for AI coding tools working in this repo.

## Project

**Drivecap** — a Rust Windows tray app that watches volume free space and shows a small, always-on-top, never-focus-stealing card with escalating, debounced, snoozable warnings. Split out of exbar; plumbing (DPI, dark mode, logging, error type) is copied, not shared. See `README.md` for user-facing details and `docs/superpowers/specs/2026-11-15-drivecap-design.md` for the design.

## Layout

Modules marked *(planned)* land in later v1 tasks (`docs/superpowers/plans/2026-11-15-drivecap-v1.md`).

```
drivecap/
├── Cargo.toml                          # workspace
├── crates/
│   └── drivecap-cli/                   # lib + bin (one binary)
│       ├── Cargo.toml                  # [lints.rust]/[clippy]/[rustdoc] gates
│       ├── build.rs                    # winres version metadata + PerMonitorV2 DPI manifest
│       ├── src/
│       │   ├── lib.rs                  # Crate-level rustdoc + pub mod declarations
│       │   ├── bin/
│       │   │   └── drivecap.rs         # CLI entry: `run` (default) and `status`
│       │   ├── theme.rs                # DPI scale, uncached dark-mode read
│       │   ├── log.rs                  # LogLevel + FileLogger (log crate) → %TEMP%\drivecap.log
│       │   ├── error.rs                # DrivecapError + DrivecapResult
│       │   ├── clock.rs                # Clock trait + SystemClock + MockClock (time-source seam)
│       │   ├── paths.rs                # Filesystem layout (~/.drivecap/)
│       │   ├── config.rs               # (planned) Config schema, defaults, load/save — ~/.drivecap/config.json
│       │   ├── alerts.rs               # (planned) Pure alert engine: zones, levels, debounce, snooze, schedule
│       │   ├── palette.rs              # (planned) Light/dark colour tokens
│       │   ├── model.rs                # (planned) Pure surface view model (rows, bar fill, value text)
│       │   ├── visibility.rs           # (planned) Pure fade / hold / linger / click-through state machine
│       │   ├── layout.rs               # (planned) Pure card layout + hit testing
│       │   ├── position.rs             # (planned) Position store + work-area clamping — ~/.drivecap/position.json
│       │   ├── volumes.rs              # (planned) Volume enumeration + local free-space query (FFI)
│       │   ├── net_probe.rs            # (planned) Network free-space probe worker (3 s budget)
│       │   ├── controller.rs           # (planned) Pure app controller: events in, Effects out
│       │   ├── raster.rs               # (planned) Pure raster helpers (premultiplied alpha, halo)
│       │   ├── paint.rs                # (planned) GDI painter → UpdateLayeredWindow
│       │   ├── card_wnd/               # Card window: mod (wndproc, pump, state), effects (apply_effects), input (mouse/drag), placement (monitors, position.json), render (GDI resources, paint, show/hide), shell (tray wiring)
│       │   ├── single_instance.rs      # Named-mutex guard + Drivecap.Show relaunch signal
│       │   ├── tray.rs                 # Shell_NotifyIconW tray icon: runtime-drawn severity glyph, tooltip, v4 callbacks
│       │   ├── menu.rs                 # (planned) Pure menu model
│       │   ├── contextmenu.rs          # (planned) TrackPopupMenu wrapper
│       │   ├── popup_wnd.rs            # (planned) Opacity / settings popups
│       │   └── font.rs                 # (planned) ChooseFontW picker
│       └── wix/
│           └── main.wxs                # WiX v4-schema installer definition (built with WiX v7)
├── scripts/
│   ├── build-msi.sh                    # invokes `wix build`
│   ├── doc-check.sh                    # RUSTDOCFLAGS="-D warnings" cargo doc gate
│   └── extract-changelog.sh            # release-notes extraction from CHANGELOG.md
├── docs/adrs/                          # architectural decision records
└── .github/
    └── workflows/
        └── ci.yml                      # GitHub Actions: lint, test, doc-check, build-msi
```

## Commands

All commands assume `cargo` is on PATH (`export PATH="$HOME/.cargo/bin:$PATH"` in git-bash).

- **Build:** `cargo build --release -p drivecap-cli`
- **Run unit tests:** `cargo test` (one module: `cargo test -p drivecap-cli <module>::`)
- **Run:** `./target/release/drivecap.exe [run|status] [--log-level error|warn|info|debug|trace]` — `run` is the default when no subcommand is given
- **Build MSI:** `./scripts/build-msi.sh` → `target/wix/drivecap-<version>-x64.msi` (requires WiX v7 — see "MSI installer")
- **Doc gate:** `./scripts/doc-check.sh` — runs `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace`
- **Pre-push gate:** `cargo fmt && cargo clippy --all-targets && cargo test && ./scripts/doc-check.sh` — all four must pass

## Architecture

### Pure controller + Win32 adapters (ADR-0001)

Every decision lives in a pure, unit-tested core with no Win32: `config`, `alerts`, `model`, `visibility`, `layout`, `raster`, `controller`, `menu`. The controller takes events (timer ticks, samples, pointer input, menu picks) and returns `Effect`s. Thin Win32 adapters — `volumes`, `net_probe`, `paint`, `card_wnd`, `tray`, `popup_wnd`, `contextmenu`, `font` — translate `WM_*` messages into events and execute the returned effects. New interaction logic follows the same split.

### Trait seams (ADR-0002)

Win32 surfaces that tests need to control sit behind traits held as `Box<dyn Trait>`, with a `Win32*` production impl and a mock in the trait file's `test_mocks` sub-module. First seam: `clock::Clock` (`SystemClock` / `MockClock`); volume queries and the network probe follow.

### Error handling and logging

`crate::error::DrivecapError` is the unified error type (`Win32`, `Io`, `Json`, `Config`); `DrivecapResult<T> = Result<T, DrivecapError>`. The `warn_on_err!` macro logs-and-continues on `Result`s where failing isn't appropriate (most Win32 one-shot calls).

Logging goes through the `log` crate. `FileLogger` in `log.rs` is the sink; verbosity comes from `--log-level` (default `info`).

### Packaging (ADR-0003)

Per-user MSI (no UAC) installs to `%LOCALAPPDATA%\Drivecap\`, writes the Run key `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Drivecap = "[INSTALLDIR]drivecap.exe" run`, and adds a Start Menu shortcut (args `run`).

## Gotchas

- **`windows` crate v0.61 quirks**:
  - `BOOL` is `windows_core::BOOL`, NOT `windows::Win32::Foundation::BOOL`
  - `GetSysColor` / `SYS_COLOR_INDEX` are in `Win32::Graphics::Gdi`, not `Win32::UI::WindowsAndMessaging`
  - Many APIs take `Option<HWND>` / `Option<HMENU>` / `Option<HINSTANCE>`
  - `DeleteObject` expects `HGDIOBJ`; convert with `.into()` from `HBRUSH` / `HPEN` / `HFONT`
- **`hCursor` on every window class**: `WNDCLASSEXW.hCursor` MUST be set (`IDC_ARROW`). Without it `DefWindowProc::WM_SETCURSOR` falls back to a null class cursor and the cursor "disappears" over our windows — hover events still fire, only the visible cursor is missing.
- **`run` must not show a console**: `drivecap.exe run` calls `FreeConsole()` first to detach from any inherited console (the MSI's post-install action otherwise opens a visible terminal). Don't add `println!` after `FreeConsole` — it silently no-ops. Release builds are also `windows_subsystem = "windows"`.
- **Catch panics at FFI boundaries**: every `extern "system"` callback body (wndproc, timer proc, enum callbacks) is wrapped in `std::panic::catch_unwind`; log the panic and return the default value. Unwinding across FFI is undefined behaviour.
- **Never take focus**: the card is `WS_POPUP` with `WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST` (+ `WS_EX_TRANSPARENT` while click-through). Only call `SetForegroundWindow`/`SetFocus` before `TrackPopupMenu` (required for menu dismissal) and for `ChooseFontW`.
- **DPI**: the PerMonitorV2 manifest in `build.rs` is applied before any user code runs. All UI pixel values pass through `theme::scale(px, dpi)` (× `config.scale_pct / 100`) — no hardcoded pixels.
- **Dark mode**: all colours come from the light/dark palette, chosen by `theme::read_dark_mode()` — never assume dark. The read is uncached so a theme switch repaints correctly; read it once per repaint, not per element.
- **State directory**: persisted state lives under `~/.drivecap/` (`config.json`, `position.json`); the log is `%TEMP%\drivecap.log`. Don't add state files elsewhere.
- **`WM_CLOSE` is the shutdown path**: the MSI's `util:CloseApplication` sends `WM_CLOSE` to the card and kills the process 500 ms later. `WM_CLOSE` and `Effect::Quit` share one handler (save `position.json`, `DestroyWindow`, `WM_DESTROY` → `PostQuitMessage(0)`) — keep it fast and never prompt or block in it.
- **Card state re-entrancy**: `card_wnd`'s `AppState` is a `RefCell` in `GWLP_USERDATA`, reached only via `with_state`. Calls that re-enter the wndproc with handled messages (`ShowWindow`, `SetCapture`/`ReleaseCapture`, `DestroyWindow`, `ShellExecuteW`, `TrackPopupMenu`, `ChooseFontW`) go through a deferred `WinOp`, never inline while the state is borrowed.
- **Never reuse exbar's UpgradeCode**: drivecap's MSI UpgradeCode is `F04B1403-DE21-41D0-9AB5-3C30AA482CAD`, fixed forever. Reusing exbar's would make one product's installer uninstall the other.

## Logging

All logs go to `%TEMP%\drivecap.log` with format `HH:MM:SS.mmm [LEVEL] pid=N message` (UTC). First diagnostic tool when something misbehaves:

```bash
cat "$TEMP/drivecap.log"
```

## MSI installer (WiX)

Defined in `crates/drivecap-cli/wix/main.wxs` (WiX v4 schema), built by `./scripts/build-msi.sh`, which invokes `wix build` directly (`cargo-wix` generates WiX v3 templates and can't drive WiX v7).

The MSI:
- **Per-user install** (`Scope="perUser"`) to `%LOCALAPPDATA%\Drivecap\` — no UAC prompt
- **Run key** at `HKCU\...\Run\Drivecap` so drivecap auto-starts at login
- **Start Menu shortcut** so users can re-launch it
- **Post-install launch** via the "Launch Drivecap now" checkbox (WixShellExec, no args → `run` default)
- **`util:CloseApplication`** shuts down a running `drivecap.exe` before file replacement on upgrade/uninstall

WiX v7 install (one-time per machine):
```
dotnet tool install --global wix
wix eula accept wix7
wix extension add --global WixToolset.Util.wixext
wix extension add --global WixToolset.UI.wixext
```

## Adding a new feature

1. Prefer inline `#[cfg(test)] mod tests`. Pure modules are fully unit-testable; Win32-touching code is tested through trait seams + mocks. Only Win32-API-heavy code (paint, wndproc dispatch) stays manual-smoke.
2. New state-machine logic: pure core + thin adapter (ADR-0001).
3. New Win32 surface needing tests: trait + `Win32*` impl + mock (ADR-0002).
4. Size caps: 1,500-line file / 150-line function, production and test lines counted separately; flag growth past 900 / 90.
5. Before pushing, run the pre-push gate above.
