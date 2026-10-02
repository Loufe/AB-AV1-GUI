# History storage workload

Status: working design note; the workload, candidate mappings, and rehearsals below are the input to History storage selection, and this note is deleted once a storage engine and physical schema are recorded

## Purpose and boundary

Storage selection compares two candidate engines, Turso and SQLite through rusqlite, on identical facts. This note fixes those facts: the executable scenarios with their expected results, the representative volumes, the operations to time, two logical-to-relational mappings, and two schema changes to rehearse. The logical model, including the browse and evidence query contract, is durable truth in `docs/HISTORY.md` under "Logical model". Translated observation identity and import semantics are ADR-025.

The [selected authority](history-storage.md) is one database containing an operational row log and History tables in a shared transaction, retaining operational fold and replay. This workload selects the engine and physical schema. Environment and content collectors, export, and the historical estimator's query strategy do not gate it.

## Executable scenarios

The scenarios are hand-maintained spec data under `crates/crfty-core/tests/fixtures/history/scenarios/`, run by `crates/crfty-core/tests/history_scenarios.rs`. Each file is a sequence of steps against an empty History, and every query step carries its expected result. A storage candidate passes when it executes every step and reproduces every expected result.

| Step | Meaning |
| --- | --- |
| `Record` | Commit one native terminal observation and its optional path row |
| `AbortedRecord` | A terminal commit that never became durable; nothing about it is visible afterwards |
| `Import` | Plan and commit one translated batch, or reject it whole, with the expected report |
| `Scrub` | Delete every path row |
| `Restart` | Close and reopen the store; every later result must equal the result before |
| `SetPathRecording` | Enable or disable path recording for later steps; it is writer input owned by settings, so it survives `Restart` and changes no revision |
| `Page` | One bounded browse query, optionally continuing from the previous page's cursor, with its expected identifiers, match count, continuation, and revision |
| `Detail` | One lookup by observation identifier with its expected recording sequence and paths; a found observation must read back exactly as its `Record` or `Import` step committed it |
| `Pairs` | One bounded read of prediction-pair evidence with its expected identifiers and continuation |

The expected sequences follow the gap-free rule in `docs/HISTORY.md`, so a candidate assigns them inside the committing transaction rather than from an engine counter such as SQLite `AUTOINCREMENT`, which a failed insert can advance. The test harness evaluates the steps against an in-memory reference so the expected results are proven self-consistent before any engine exists. That evaluator is test code only. The selected storage runs the same scenarios, and the in-memory reference is removed when it does.

The native scenarios cover repeated sources, a queue retry after failure, stopped and incomplete runs, a changed source at one path, a retained prediction pair among ineligible evidence, and recording with path recording disabled across a restart. The import scenarios cover sparse and pathless records, idempotent re-import, in-file and cross-import conflicts, whole-file rejection, scrub followed by re-import, and import with path recording disabled. The browse scenarios cover every order key in both directions, null-heavy ordering with ties, `Change` arithmetic that only exact truncation reproduces, keyset continuation including across a scrub, literal search containing `%` and `_`, and restart.

## Representative volumes

The decided guardrail is first paint under 100 milliseconds at 50,000 records. Selection builds that store with a deterministic generator from a fixed seed, in these proportions.

| Dimension | Volume |
| --- | --- |
| Observations | 50,000: 35,000 native and 15,000 translated |
| Native outcomes | 55 percent converted, 15 percent analyzed, 10 percent not worthwhile, 5 percent remuxed, 8 percent failed, 4 percent stopped, 3 percent incomplete |
| Translated outcomes | 70 percent converted, 20 percent not worthwhile, 10 percent analyzed |
| Sources | 70 percent of content keys observed once; the rest observed 2 to 20 times |
| Attempts | 1 to 11 per not-worthwhile observation; 0 to 5 failed fallback attempts per search |
| Assessment | 60 percent of native analyzed observations hold a matched search assessment; 60 percent of native converted observations hold matched search and encode assessments; all others are unassessed |
| Paths | 95 percent of native and 80 percent of translated observations hold a path row; converted and remuxed native rows also hold an output path |
| Absent facts | 10 percent of translated rows lack an update instant; 30 percent lack sizes or quality |

Timed operations, each reported as median and 95th percentile over repeated runs:

1. Reopen after restart and read the first default page, which is `Date` descending with the default outcome filter and a limit of 100. This is the storage part of first paint only; the 100 millisecond guardrail is measured end to end, through the shell bridge and the rendered view, once the History view is connected.
2. The first page under each order key in both directions.
3. The 400th consecutive page under `Date` descending, reached through cursors.
4. A search matching roughly 1 percent of rows.
5. A detail lookup by native and by translated identifier.
6. Reading every prediction pair in pages of 500.
7. One terminal recording, measured through its durability boundary.
8. Importing 15,000 new translated records as one batch, re-importing the same batch, and importing it again with 150 conflicting records.
9. A scrub of every path row.

Correctness under crash is proven separately from timing. A terminal recording and an import batch are each interrupted before and after their durability boundary, and the reopened store holds all or none of each. A driver-level terminal test restarts the operational row log and asserts the terminal and observation together; the storage-only `AbortedRecord` step does not prove this boundary. Failure points also cover startup recovery, uncertain import retry, and operational compaction.

Candidates must round-trip full-range `u64` facts and prove every numeric order and continuation without precision loss. Additional boundary cases include `Date` and recording sequence across the signed 64-bit limit and `Change` with before 1 and after `i64::MAX`, whose negative result exceeds signed 64-bit range. A populated History store with an unsupported operational payload must remain browsable while operational writes stop. Runtime-ID continuity includes reservations that have no History observation.

## Candidate mappings

Both mappings hold identical facts and pass the same scenarios. Neither uses an opaque payload column. Enumerations map to text discriminants, fallback attempts are ordered child rows, and path rows are a child table in both.

**M1, one observation table.** One row per observation carries the identity, recording sequence, outcome discriminant, source facts, instants, and every outcome's evidence as nullable columns. Row-level check constraints state which columns each outcome requires and forbids. The recording transaction also enforces cardinality across child rows, including the required nonempty not-worthwhile attempt list.

**M2, evidence tables.** One narrow observation row carries identity, sequence, outcome, source facts, and instants. Search, encode, remux, not-worthwhile, failure, and translated evidence live in one-to-at-most-one tables keyed by sequence.

| Concern | M1 | M2 |
| --- | --- | --- |
| Outcome shape integrity | Row checks enforce required and forbidden columns; child-row cardinality needs transaction-level enforcement | Evidence-row cardinality needs triggers or transaction-level validation; child-row cardinality needs transaction-level enforcement |
| Browse page | The observation row plus the path row | Also joins encode, remux, search, and translated evidence |
| Order-key indexes | `Date`, `Recorded`, `Before`, `After`, `Change`, `Quality`, and `Took` index one table; `File` indexes the path table | `After`, `Change`, `Quality`, and `Took` live in evidence tables, so no single index serves those ordered pages |
| Width | Wide rows, most columns null for any one outcome | Narrow rows |
| Additive field | One nullable or defaulted column | One column on the owning evidence table |

M1 is selected by this analysis and M2 is rejected without measurement. M1 enforces outcome-specific columns with row checks, while M2 needs cross-table enforcement for its evidence rows; both need cross-row enforcement for nonempty fallback attempts. Four of the eight orders also lose single-table index support under M2. M1's null density costs little in SQLite's record format. Selection runs both engines on M1.

## Rehearsed changes

Selection applies both changes to a populated store on each engine through the engine's migration path, then reruns every scenario. Interruption before and during each migration leaves either the intact previous schema or the complete new schema; assertions preserve identities, sequences, revision, and scrubbed-path absence.

**Additive field.** Native observations gain the decided throttling provenance flag. Rows recorded before the change migrate to not throttled, which is truthful because the flag marks deliberate throttling and no earlier run could be throttled. Identifiers, recording sequences, revisions, and every expected page stay unchanged.

**Contract correction.** Quality targets and scores gain their ADR-022 metric tag. Each stored target and score, including those inside fallback attempts and translated quality, gains its own metric discriminant beside it, and every existing value migrates to VMAF. Identifiers, sequences, revisions, and pages stay unchanged. The scenarios hold untagged values and cannot prove this migration alone, so the rehearsal adds explicit assertions that every stored target and score class reads back tagged VMAF: search results, failed fallback attempts, not-worthwhile requests and attempts, and translated quality and targets.

## Distillation

The selected engine, mapping, measured results, and transaction boundary are recorded in an ADR and `docs/ARCHITECTURE.md`. The scenario fixtures then run against the selected storage, and this note is deleted.
