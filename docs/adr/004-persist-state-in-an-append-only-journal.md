---
status: accepted
date: 2026-07-19
---

# Persist state in an append-only journal

## Context and problem statement

Queue and History mutations must survive crashes without allowing the UI to display state that was never made durable. A reportable terminal and its immutable observation must commit together. From 3.0.0 onward, History must survive released-version upgrades. The synchronous driver already owns transactions, and the storage engine must preserve that ownership.

## Decision drivers

* Make durable transitions recoverable and inspectable
* Preserve privacy scrub and redaction behaviour
* Maintain write-ahead ordering between persistence and UI deltas
* Commit operational completion and History through one authority
* Give released History data an explicit migratable physical format
* Retain the reducer, fold, and single writer

## Considered options

* One database containing an operational delta row log and relational History tables
* One database containing fully relational operational state and History
* File journal as authority with a rebuildable SQL History index
* File journal as authority with History held in memory
* Independently committed file journal and History database

## Decision outcome

Chosen option: **one database containing an append-only operational row log and relational History tables**. Each reducer batch is a log row, and reportable terminal facts enter History in the same database transaction. This preserves fold and replay while separating History's physical schema from serialized operational Rust types. Turso and rusqlite are compared on this topology; neither engine is selected yet.

The transaction commits the operational batch, observation and child evidence, optional path row, recording sequence, History revision, and any runtime-ID reservation. Only a durable commit permits publication. Ephemeral telemetry is never persisted. Compaction retains an operational snapshot and tail and leaves immutable observations intact (ADR-009). A database transaction lock does not replace the data-directory lock (ADR-008).

Runtime-ID continuity must survive independently of an incompatible operational snapshot. History alone cannot reconstruct it because reservation-only runs receive identities without observations. The SQL schema version and operational payload version are separate. Unsupported operational payloads leave History readable and reject operational writes; migration or an explicit safe reset must preserve IDs and resolve pending output before work resumes.

The current engine still implements the JSON-lines file journal and its recovery protocol. The database replacement must prove the shared transaction, compaction, degraded reads, and recovery rules before product integration. The engine and physical recovery protocol remain specified as open choices in [History storage](../design/history-storage.md).

### Consequences

* Good: State after restart equals the fold of durable deltas
* Good: Write-ahead ordering can be enforced by types and sequence tests
* Good: Completion and its observation have one durability boundary
* Good: History has an explicit schema that can migrate across released versions
* Bad: Operational persistence, compaction, and corruption recovery must be adapted and proven on the selected engine
* Bad: Serialized operational payloads still need a separate release-upgrade policy

## More information

See ADR-002 (driver ownership), ADR-008 (data-directory lock), ADR-009 (compaction), ADR-011 (corruption acknowledgment), [History](../HISTORY.md), and [the storage comparison workload](../design/history-storage-workload.md).
