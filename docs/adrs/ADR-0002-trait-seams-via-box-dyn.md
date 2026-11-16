# ADR-0002: Trait seams via `Box<dyn Trait>`

**Status:** Accepted (since 2026-11-15)
**Context tag:** architecture

## Context

The parts of drivecap that touch the outside world — the wall clock,
volume enumeration and free-space queries, the network-share probe,
config storage — are exactly the parts the alert and visibility logic
depends on. Called directly, none of them are unit-testable: each needs
real time passing, real disks filling, or a real SMB share going dark.

## Decision

Define one trait per external surface (first: `crate::clock::Clock`;
volume queries and the network probe follow). Production holds a
`Win32*` / `System*` impl as `Box<dyn …>` (or `Arc<dyn …>` where shared
with a worker thread); tests inject a mock that lives in the trait
file's `test_mocks` sub-module.

## Alternatives considered

- **Generics** (`<C: Clock, V: VolumeSource, …>`) — multiplies type
  parameters at every call site and is hostile to wndproc callbacks,
  which are `extern "system" fn` pointers and cannot carry generic
  parameters.
- **Function-pointer dispatch** — same problems as generics, less
  ergonomic to mock.

## Consequences

- Positive: mock-driven tests for time- and disk-dependent behaviour
  (`MockClock` with `set`/`advance`); clean Win32/logic split.
- Negative: one v-table dispatch per call — negligible at polling and
  interaction rates (seconds and clicks, not frames).
- Follow-on: enables [ADR-0001](ADR-0001-pure-controller-adapter-pattern.md)
  — the adapters execute controller effects through these seams.

## References

- Code: `crates/drivecap-cli/src/clock.rs` (`Clock`, `SystemClock`,
  `test_mocks::MockClock`).
