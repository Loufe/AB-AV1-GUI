---
status: accepted
date: 2026-07-19
---

# Separate Core, Engine, and Shell

## Context and Problem Statement

The Python application relies on conventions to keep conversion logic independent
of Tkinter. V3 needs dependency boundaries that prevent domain code, process code,
and Tauri integration from becoming coupled.

## Decision Drivers

* Enforce architecture through the crate graph
* Keep domain logic deterministic and easy to test
* Keep Tauri replaceable and presentation-only
* Avoid a workspace split by topic rather than dependency boundary

## Considered Options

* One application crate
* Many topic-oriented crates
* Core, engine, and thin Tauri shell separated by dependency capability

## Decision Outcome

Chosen option: **Core, engine, and thin shell**, because each boundary removes an
entire category of accidental dependency.

`crfty-core` has no filesystem, process, clock, async-runtime, or UI dependency.
`crfty-engine` may use processes and the filesystem but cannot depend on Tauri. The
future shell may depend on both and contains only IPC and application wiring.

### Consequences

* Good: Engine-to-GUI coupling becomes a build error
* Good: Core behavior can be tested without runtime or operating-system fixtures
* Bad: Cross-boundary data must be modeled explicitly

## More Information

See issue #33, section 4, and ADR-002.
