# ADR-0003: MSI per-user install with `Run` key

**Status:** Accepted (since 2026-11-15)
**Context tag:** packaging

## Context

A disk-space warner only helps if it is running before the disk fills,
so it must auto-start at login. Installing per-machine requires UAC and
a higher-friction first run (SmartScreen on an unsigned installer). For
an unsigned personal tool that watches free space, a UAC prompt costs
more than it buys.

## Decision

Ship a WiX MSI with `Scope="perUser"`, install location
`%LOCALAPPDATA%\Drivecap\`, and
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Drivecap =
"[INSTALLDIR]drivecap.exe" run` for auto-start at login. The Finish
dialog's "Launch Drivecap now" checkbox starts it immediately via
WixShellExec (no arguments → `run`, the default subcommand). A Start
Menu shortcut (args `run`) re-launches it. `util:CloseApplication`
shuts down a running `drivecap.exe` during upgrade / uninstall so file
replacement doesn't fail. The UpgradeCode
`F04B1403-DE21-41D0-9AB5-3C30AA482CAD` is drivecap's own and fixed
forever.

## Alternatives considered

- **Per-machine MSI** — needs UAC. Higher friction than the tool warrants.
- **Bare-zip distribution** — the user wires up the `Run` key manually
  and has no uninstall story.
- **Windows Store packaging** — needs WinAppSDK packaging and a
  publisher account. Out of proportion for a personal tool.

## Consequences

- Positive: no UAC prompt; clean uninstall via Settings → Apps;
  auto-start built in; upgrades replace files cleanly because the
  running instance is shut down first.
- Negative: no Group Policy deployment story (not a goal); SmartScreen
  still warns on an unsigned MSI.
- Constraint: drivecap must never need admin rights at runtime (no
  services, no machine-wide hooks) or this decision stops holding.

## References

- Code: `crates/drivecap-cli/wix/main.wxs`, `scripts/build-msi.sh`.
- Operational note: `CLAUDE.md` § "MSI installer (WiX)".
