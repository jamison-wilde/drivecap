# Architecture Decision Records

Short, focused records of the non-obvious architectural decisions in
drivecap. Each ADR answers a single question of the form "why is this
*not* the obvious thing?" — five-minute reads for a future maintainer.

## Index

| #    | Title                                                          | Status   | One-liner                                                         |
|------|----------------------------------------------------------------|----------|-------------------------------------------------------------------|
| 0001 | [Pure controller + Win32 adapter pattern](ADR-0001-pure-controller-adapter-pattern.md) | Accepted | Why every decision lives in pure modules with thin Win32 adapters. |
| 0002 | [Trait seams via Box<dyn Trait>](ADR-0002-trait-seams-via-box-dyn.md)    | Accepted | Why Win32 surfaces are abstracted behind dyn-trait seams, not generics. |
| 0003 | [MSI per-user install with Run key](ADR-0003-msi-per-user-install.md)    | Accepted | Why drivecap ships as a per-user MSI with `HKCU\…\Run` instead of per-machine. |

## Status values

- **Accepted** — the decision is in force.
- **Superseded by ADR-NNNN** — replaced by a later decision; kept for history.
- **Deprecated** — no longer in force; nothing replaces it.

The initial three carry over decisions proven in exbar, the project
drivecap was split out of; their dates reflect when drivecap adopted them.

## Adding a new ADR

Use the next sequential number (0004, 0005, …) and the same template
(Status / Context / Decision / Alternatives considered / Consequences /
References). Add a row to the index above. Keep the body under 60 lines.
