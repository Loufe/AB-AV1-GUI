---
status: accepted
date: 2026-07-19
---

# Persist state in an append-only journal

## Context and problem statement

Queue and history mutations must survive crashes without allowing the UI to display state that was never made durable. The application state fits in memory and has one writer, so a database would duplicate transaction ownership already enforced by the driver.

## Decision drivers

* Make durable transitions recoverable and inspectable
* Preserve privacy scrub and redaction behaviour
* Maintain write-ahead ordering between persistence and UI deltas
* Avoid multiple storage writers and database migration machinery

## Considered options

* Rewrite one JSON state file after each change
* Store history and queue in SQLite
* Append typed durable deltas to a single-writer journal
* Treat the journal as a full event-sourced source of truth (rejected: the journal is a storage format and any command log is a debugging record; upsert-last-wins stays the storage semantic, and promoting either into event sourcing buys replay semantics nothing here needs)

## Decision outcome

Chosen option: **An append-only single-writer journal**, because it matches reducer transactions while remaining recoverable, inspectable, and redactable.

The driver appends and syncs durable deltas before emitting them to the UI. Ephemeral telemetry is represented separately and is never journaled. Compaction rewrites live state through a crash-safe writer barrier.

### Consequences

* Good: State after restart equals the fold of durable deltas
* Good: Write-ahead ordering can be enforced by types and sequence tests
* Bad: CRFty owns journal framing, recovery, and compaction correctness

## More information

See `docs/ARCHITECTURE.md` (the driver loop this journal is a law of) and ADR-002. The format's later decisions are ADR-009 (compaction) and ADR-011 (corruption acknowledgment).
