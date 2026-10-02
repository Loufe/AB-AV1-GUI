# History storage

Status: working design; the shared database authority and released History preservation are decided, while the engine, physical schema, and recovery protocol remain open

The selected authority is one database containing an operational delta row log and relational History tables. The synchronous driver remains the only writer, and operational fold and replay remain in core (ADR-004). The current engine still uses the file journal. Turso and rusqlite are compared on the selected topology and the M1 mapping; the engine and final physical schema remain open.

## Authority and rationale

The [History logical model](../HISTORY.md#logical-model) requires a reportable terminal, observation, optional path row, recording sequence, and revision to become durable together. The database transaction also persists the operational reducer batch. Only successful durable commit permits publication. Startup terminals and recovered output use the same validated transition and observation construction as live completion.

History must survive released-version upgrades from 3.0.0. Relational tables give its immutable facts a physical format that can migrate independently of operational Rust payloads. An operational row log preserves the current reducer and replay model while avoiding a full relational rewrite of queue, run, and output state. The engine comparison must still prove the costs of replacing the file journal, including replay, compaction, corruption handling, and installed packaging.

| Alternative | Consequence |
| --- | --- |
| Fully relational operational state and History | Provides one authority, but requires replacing operational replay and re-establishing its recovery invariants |
| File journal with History in memory | Can commit terminal and observation together, but couples released History upgrades to journal-format migrations and retains the full History working set in memory |
| File journal with a derived SQL index | Can remain one authority if the index is verified and rebuilt from the journal; adds catch-up, repair, and snapshot costs |
| Independent journal and History commits | Leaves a crash window between two authoritative commits and fails the atomic terminal contract |

The release requirement favours a separate relational History format; it does not prove that journal migrations are impossible or that the selected design is faster. Measurements on the journal alternatives are optional supporting evidence. Both engine candidates use the selected database topology.

## Commit and identity

A terminal transaction writes the operational batch, observation and ordered child evidence, optional path row, sequence, and revision. The writer allocates gap-free sequences within that transaction. Aborted recording consumes nothing; duplicate-only import changes neither sequence nor revision. Failed durable writes retain the driver's stop-and-recover behaviour and publish no durable event.

Runtime IDs must never be reused. Their high-water mark must survive independently of serialized operational snapshots and advance atomically with a reservation. It cannot be rebuilt from History alone: reservation-only runs have assigned IDs but no observation. Operational log sequence, History recording sequence, and runtime IDs are distinct values.

Imports validate the entire input before planning. Duplicate and conflict decisions use the writer's current store view in the serialized committing operation, with any earlier plan revalidated. They compare immutable translated facts rather than optional paths. Re-import after scrub restores no path. One import batch is one transaction, and translated identity makes an uncertain commit safe to retry.

The settings batch barrier remains in force. Recording and import use the path setting that successfully took effect before their batch; a failed settings write leaves the prior setting effective.

## Exact facts and queries

Every validated observation must round-trip exactly. Storage must represent the existing full `u64` fact domain without introducing a terminal-time range failure. A narrower product domain requires a separate reason and defined handling at every input and production boundary, including facts first known after a run starts. JavaScript number limits do not establish a file-size or duration limit; the IPC representation remains a separate contract choice.

Full-`u64` order values include `Date`, `Before`, `After`, `Took`, and recording sequence, which is also every order's tie-break value. An eight-byte big-endian Binary Large Object (BLOB) is a candidate exact sortable representation. `Change` uses the core's signed 128-bit arithmetic and truncation; a 16-byte big-endian BLOB with its sign bit flipped is a candidate sortable representation. Both engines must prove comparison, indexing, missing values, ties, and cursor continuation over the extremes. The physical encoding is selected from that evidence.

A non-order `u64` may be bit-reinterpreted as signed `i64` only when decoding and SQL constraints, comparisons, and arithmetic honour that encoding. Sequence and revision metadata require exact storage and checked increment inside the writer transaction. Outcome checks and child cardinality preserve the typed shape; read-back and migration reject invalid shapes rather than coercing them.

One read transaction supplies page rows, total match count, continuation, and revision. Readers release it after each page. File-name ordering and literal search use the core normalization contract; scrub removes path-derived sort and search values. Direction and absent-value handling must preserve sequence-descending ties in both directions. Indexes require a query plan and measured benefit.

## Versions and degraded reads

The SQL schema version and serialized operational payload version are separate. From 3.0.0 onward, retained History migrations preserve identities, immutable facts, recording sequences, revision, and scrubbed-path absence. Pre-release data may require a fresh directory; migration rehearsals do not create an obligation to retain pre-release decoders.

When an operational payload is unsupported, the newer build must still browse History and read detail while rejecting operational writes. A populated-store test with an unsupported operational payload proves this separation. Full release migration of queue state, saved analyses, and pending output transactions remains open. Before operational work resumes, migration or explicit safe reset must retain runtime-ID continuity and safely settle pending output. Automatic reset is not a recovery policy.

## Persistence and recovery

Operational compaction stores a snapshot and deletes its covered log rows in one transaction at the existing quiescent writer barrier (ADR-009). It changes no immutable History facts. The comparison measures row-log replay, snapshot size, and compaction failure behaviour rather than copying file-journal thresholds without evidence.

The current file recovery protocol binds discard to an unreadable suffix and preserves an archive (ADR-011). The database replacement must define malformed-row and engine-corruption behaviour, generation identity, evidence preservation, surviving reads, and explicit discard. It must not claim file-style valid-prefix recovery without proving it. An unsupported payload version is an upgrade limitation, not authorization to discard data.

The dedicated data-directory lock is acquired before settings or database reads and held for the engine lifetime (ADR-008). Database locking does not replace it. Installed Windows and Linux builds must prove contention and release after process death.

Durability settings must support publication after durable commit. Process aborts before and after COMMIT prove application atomicity and restart; they do not prove survival after loss of the operating system cache. The comparison records the pinned engine's durability guarantees, configuration, and checkpoint behaviour separately. Physical erasure remains outside the [logical scrub promise](../HISTORY.md#scrub-is-a-logical-transform-not-erasure).

## Engine evidence and open choices

The [storage workload](history-storage-workload.md) fixes M1, executable scenarios, 50,000 observations, timed operations, and two migration rehearsals. Rusqlite is the first prototype because its synchronous API fits the driver. Turso uses the same facts and scenario harness through an adapter; candidate code is temporary and the unselected implementation is removed after selection.

The `history-storage-spike` feature exposes the candidate store in [crfty-engine](../../crates/crfty-engine/src/history_store/mod.rs). Both adapters execute one shared SQL mapping. A static field description maps observation facts to primitive columns and fallback evidence to ordered child rows, with required and inactive fields enforced by individual row checks. JSON is an in-memory mapping step; observations are never persisted as opaque JSON payloads. Numeric facts use eight-byte big-endian BLOBs; Change uses a 16-byte sign-adjusted encoding. Path rows retain tagged platform-native units independently of the facts.

The candidate transaction writes core-encoded operational batches and their terminal observations together, and returns a durability token after COMMIT. The same pure checked batch fold serves live reduction, replay, and candidate writes. It captures observations and separate path rows at each reportable terminal in delta order, using the run's frozen prepared media facts. The writer applies path recording to those rows and allocates sequences inside the transaction. Runtime-ID reservations have separate exact metadata. Operational compaction replaces the snapshot and removes the covered rows in one transaction. An unsupported operational payload leaves History queries available and refuses operational writes. The application driver still uses its file journal; its publication, settings persistence, and startup recovery paths require integration proof before engine selection.

The [scenario runner](../../crates/crfty-engine/tests/history_storage.rs) executes every committed History scenario on both engines. Focused tests cover physical row checks, exact numbers and continuation across signed integer limits, sequence exhaustion, sparse evidence, and platform-native paths. The [process fixture](../../crates/crfty-engine/tests/history_storage_crash.rs) aborts before and after terminal, import, and compaction commits and verifies reopened state, import retry, and lock release. These tests run in the workspace's all-features gate; they establish process-crash behaviour, while power-loss guarantees and installed-package behaviour remain separate evidence.

Driver-level crash tests restart and assert that the terminal and its observation are present together or absent together. They include startup recovery, uncertain import retry, and compaction. Migration interruption leaves an intact previous or complete new schema and preserves all existing query results. Explicit migration assertions cover every quality target and score class.

The remaining choices are the engine, exact SQL schema and numeric encoding, measured indexes and compaction policy, and database corruption/recovery protocol. Evidence includes median and p95 timings, hardware and OS, build mode, cache state, database and write-ahead log sizes, durability settings, SQL plans, and installed Windows/Linux dependencies and behaviour. The 100 ms first-paint guardrail is verified end to end when the History view is wired.
