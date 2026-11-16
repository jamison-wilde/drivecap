# ADR-0001: Pure controller + Win32 adapter pattern

**Status:** Accepted (since 2026-11-15)
**Context tag:** architecture

## Context

Drivecap's behaviour is almost entirely timing and state: per-volume
zones, level crossings, debounce re-arming, snooze break-through,
reminder intervals, fade/hold/linger, click-through, hover. Written
inline in a wndproc, those rules can only be tested by driving a real
message pump with real timers and real disks — in practice, by manual
smoke. Exbar learned this the hard way: its first wndproc mixed state
transitions, capture and paint into one large match that could not be
tested until it was split.

## Decision

Every decision lives in a pure module with no Win32 — `config`,
`alerts`, `model`, `visibility`, `layout`, `raster`, `controller`,
`menu`. The controller takes events (ticks, samples, pointer input,
menu picks) and returns a closed set of `Effect`s. Thin Win32 adapters
(`volumes`, `net_probe`, `paint`, `card_wnd`, `tray`, `popup_wnd`,
`contextmenu`, `font`) translate messages into events and execute the
returned effects. New interaction logic follows the same split.

## Alternatives considered

- **Logic inline in the wndproc** — untestable except by manual smoke;
  high cognitive load per change.
- **A third-party state-machine crate** (`statig`, `rust-fsm`) — adds a
  dependency and a DSL for a handful of states each. Hand-rolled enums
  are clearer at this scale.

## Consequences

- Positive: the scenarios that matter (free space jittering across a
  level, a USB drive vanishing mid-poll, a dire drive snoozed) are unit
  tests with a mock clock, not dogfooding sessions; adapters stay
  near-trivial to read.
- Negative: two layers per subsystem; the `Effect` enum must stay
  closed, and a variant the adapter forgets is a runtime gap rather
  than a compile error in the controller.
- Follow-on: [ADR-0002](ADR-0002-trait-seams-via-box-dyn.md) supplies
  the seams the adapters execute through.

## References

- Spec: `docs/superpowers/specs/2026-11-15-drivecap-design.md` §6.
- Plan: `docs/superpowers/plans/2026-11-15-drivecap-v1.md`.
