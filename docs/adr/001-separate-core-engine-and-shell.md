---
status: accepted
date: 2026-07-19
---

# Separate Core, engine, and shell

## Context and problem statement

The Python application relies on conventions to keep conversion logic independent of Tkinter. V3 needs dependency boundaries that prevent domain code, process code, and Tauri integration from becoming coupled.

## Decision drivers

* Enforce architecture through the crate graph
* Keep domain logic deterministic and directly testable
* Keep Tauri replaceable and presentation-only
* Avoid a workspace split by topic rather than dependency boundary

## Considered options

* One application crate
* Many topic-oriented crates
* Core, engine, and thin Tauri shell separated by dependency capability
* Run the engine as a separate daemon process (rejected: a tray-resident application would cover the same need at a fraction of the complexity)
* Ship a headless CLI alongside the GUI (rejected: ab-av1 is already the headless tool, while CRFty's product value lies in bulk estimates and statistics; the crate graph alone enforces the engine-to-GUI boundary)

## Decision outcome

Chosen option: **Core, engine, and thin shell**, because each boundary removes an entire category of accidental dependency.

`crfty-core` has no filesystem, process, clock, async-runtime, or UI dependency. `crfty-engine` may use processes and the filesystem but cannot depend on Tauri. The future shell may depend on both and contains only IPC and application wiring.

### Consequences

* Good: Engine-to-GUI coupling becomes a build error
* Good: Core behaviour can be tested without runtime or operating-system fixtures
* Bad: Cross-boundary data must be modeled explicitly

## More information

See `docs/ARCHITECTURE.md` (the workspace layout and dependency rule) and ADR-002.
