---
status: accepted
date: 2026-07-19
---

# Centralize Mutation in a Driver Reducer

## Context and Problem Statement

The application coordinates UI commands, scans, long-running conversions, durable
history, and high-rate telemetry. Sharing mutable containers would require lock
ordering and notification rules to remain correct by convention.

## Decision Drivers

* Make state ownership structural rather than documentary
* Preserve command ordering and read-your-writes behavior
* Keep durable transitions lossless without letting telemetry block child pipes
* Make state transitions deterministic and replayable

## Considered Options

* Lock-based shared state
* An actor framework and multiple state owners
* One synchronous driver owning state and applying a pure reducer

## Decision Outcome

Chosen option: **One synchronous driver plus a pure reducer**, because it eliminates
lock ordering and makes mutation, persistence, and notification a single ordered
flow.

Commands enter through a bounded lossless channel. High-rate telemetry uses a
separate coalescing path. The reducer performs no I/O or clock access and returns
deltas, effects, and replies for the driver to interpret.

### Consequences

* Good: No shared mutable application state or deadlock analysis
* Good: Reducer sequence tests can cover queue and session edge cases
* Bad: Long I/O must be expressed as effects and return results as commands

## More Information

See issue #33, section 5, and ADR-001.
