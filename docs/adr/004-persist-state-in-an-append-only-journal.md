---
status: accepted
date: 2026-07-19
---

# Persist State in an Append-Only Journal

## Context and Problem Statement

Queue and history mutations must survive crashes without allowing the UI to display
state that was never made durable. The application state fits in memory and has one
writer, so a database would duplicate transaction ownership already enforced by the
driver.

## Decision Drivers

* Make durable transitions recoverable and inspectable
* Preserve privacy scrub and redaction behavior
* Maintain write-ahead ordering between persistence and UI deltas
* Avoid multiple storage writers and database migration machinery

## Considered Options

* Rewrite one JSON state file after each change
* Store history and queue in SQLite
* Append typed durable deltas to a single-writer journal

## Decision Outcome

Chosen option: **An append-only single-writer journal**, because it matches reducer
transactions while remaining recoverable, inspectable, and redactable.

The driver appends and syncs durable deltas before emitting them to the UI. Ephemeral
telemetry is represented separately and is never journaled. Compaction rewrites live
state through a crash-safe writer barrier.

### Consequences

* Good: State after restart equals the fold of durable deltas
* Good: Write-ahead ordering can be enforced by types and sequence tests
* Bad: CRFty owns journal framing, recovery, and compaction correctness

## More Information

See issue #33, sections 5 and 10, and ADR-002.
