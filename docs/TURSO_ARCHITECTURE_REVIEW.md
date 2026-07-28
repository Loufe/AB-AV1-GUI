# Turso durable-state architecture review

Status: recommended implementation direction, subject to the bounded engine
proof in [Adoption gates](#adoption-gates). This is not yet an accepted ADR and
does not describe the currently implemented journal.

Related issues: #89, #79, #84, #78, #77, #57, #51, and #81.

## Conclusion

The cleanest Turso direction is:

- one local SQLite-format database in ordinary WAL mode;
- one connection owned by the existing engine driver thread;
- `BEGIN IMMEDIATE` transactions for durable commands;
- relational current-state tables, not an event log and not a serialized
  `DurableState` blob;
- a first-class, append-oriented `history_records` table with no foreign key to
  operational state;
- settings in the same database;
- transaction-local Rust domain objects, with no long-lived Rust mirror of
  durable state;
- pure Rust transition rules followed by SQL writes and commit;
- effects, replies, runtime-state changes, and UI publication only after a
  successful commit;
- full operational read-model snapshots for the frontend, while History stays
  request-driven and publishes only revision/count invalidation.

This retains the useful part of ADR-002—the engine driver is the only mutation
authority—while removing the journal, replay, compaction, and frontend fold as
additional state machines.

Turso has the SQL features this design needs today. The remaining risk is the
engine's beta maturity and exact local Rust behavior, not a missing CRUD
feature. CRFty should therefore pin one Turso release and prove the narrow
workload it will actually use. It should not build a storage abstraction,
dual-write path, or fallback engine in anticipation of a future switch.

## Why this is narrower than “put everything in a database”

The database is authoritative memory for durable backend facts. Rust still
needs temporary typed values while evaluating a command, and it still needs
memory for facts that cannot be persisted meaningfully:

- child process handles and process groups;
- cancellation handles;
- live telemetry and progress;
- tool discovery and availability;
- Analysis generations;
- session counters and coordination;
- channels, subscribers, and pending replies.

Those values describe the running process, not the durable product model. They
remain in an explicit `RuntimeState`. Durable values are loaded into a
transaction-scoped working set and dropped after commit/publication. Turso's
page cache provides the durable read cache; CRFty does not add another one
without measurement.

The frontend continues to hold a UI read model because rendering is its job.
It does not become a second durable authority.

## Process and ownership model

```text
Tauri command / worker result
            |
            v
  bounded driver channel
            |
            v
  crfty-driver OS thread
  + current-thread Tokio runtime
  + one Turso Database/Connection
  + RuntimeState
            |
            v
  BEGIN IMMEDIATE
    load required aggregate rows
    validate typed working set
    run pure Rust transition
    write resulting rows
    insert History when reportable
    advance revisions / allocate IDs
  COMMIT
            |
            +--> apply RuntimeState change
            +--> publish operational snapshot
            +--> publish History revision/count
            +--> send reply
            `--> launch filesystem/process effects
```

Turso's Rust API is async. That does not justify making the engine or reducer
generally async. The existing dedicated driver thread creates one
current-thread Tokio runtime and uses it to drive `Builder::new_local`,
queries, and transaction completion. The public driver channel remains
synchronous.

Use one connection initially. CRFty already prevents a second application
process with its data-directory lock, and its writes are serialized by the
driver. MVCC, concurrent writers, a connection pool, and read replicas solve
problems CRFty does not have.

## Command transaction contract

### Durable commands

For every durable command:

1. Begin an immediate write transaction.
2. Load the smallest coherent aggregate set needed by the command.
3. Convert rows to typed core values and run the same structural validators
   used by transitions.
4. Evaluate the transition without mutating long-lived runtime state.
5. Write complete resulting aggregate rows and owned child rows.
6. Allocate any IDs in the transaction.
7. If the command completes a reportable operation, insert its
   `HistoryRecord` in this same transaction.
8. Advance `operational_revision`, and `history_revision` when History changed.
9. Fully consume or drop every statement/row iterator.
10. Commit.
11. Only after commit, apply runtime changes and publish replies, effects, and
    read models.

An ordinary command failure rolls back and returns a typed rejection. A commit
or fsync error is different: the driver must treat durability as unknown, stop
accepting mutations, and require restart/reconciliation. It must not retry the
command automatically.

### Ephemeral commands

Commands that affect only `RuntimeState` do not open a transaction. A command
that has both durable and runtime consequences returns a candidate runtime
change from the pure transition and applies it after commit. Runtime state can
therefore never get ahead of durable state.

### External filesystem effects

A database transaction cannot include a filesystem rename or deletion. The
existing output ledger remains the correct protocol:

1. commit the intent/current output state;
2. perform the filesystem effect;
3. inspect identity;
4. commit the next output state.

On restart, unsettled `output_transactions` are reconciled against the
filesystem exactly as they are today. The difference is that each ledger state
is a current row, not one more event that must be replayed. No generic outbox
or database change log is needed.

## Durable model on disk

The schema should be relational at the places where identity, lifecycle,
querying, or integrity matter. Avoid both extremes:

- do not normalize Rust value objects into dozens of tables merely because SQL
  can;
- do not hide evolving domain aggregates in JSON/CBOR payloads merely to avoid
  writing row mappings.

The proposed boundary is:

| Area | Tables | Storage rule |
|---|---|---|
| Schema/app metadata | `schema_migrations`, `app_meta`, `id_allocators` | Ordinary columns and constraints. |
| Settings | `settings`, `scan_extensions` | Ordinary columns; native paths use the common path codec. |
| Queue | `queue_items` | One row per item; position and lifecycle are columns. |
| Current file identity | `path_bindings` | One row per path hash; destructive identity and content key are columns. |
| Current content | `file_records`, `audio_streams` | Media facts are columns; audio is an owned child collection. |
| Reusable analysis | `analysis_profiles`, `analysis_results`, `analysis_attempts` | Profile identity and measurements are queryable columns; attempts are owned children. |
| Runs | `conversion_runs`, `run_phase_spans`, run-owned analysis/evidence tables as needed | Frozen execution facts and terminal evidence are columns; repeated runs remain distinct. |
| Output recovery | `output_transactions` | One row per run, including tagged paths, identities, replacement mode, current ledger state, and conflict detail. |
| Standing decision | `file_verdicts` | A content key points at its current decisive native run. It does not copy statistical summaries. |
| History | `history_records`, `history_audio_codecs` | Independent observation rows optimized for History, Statistics, and Estimation. |

`STRICT` tables, `NOT NULL`, `CHECK`, `UNIQUE`, and owned-child foreign keys
should reject structurally impossible rows. Foreign keys are appropriate for
owned rows such as audio streams, attempts, and phase spans. They should not
create deletion chains between independent aggregate roots.

In particular:

- deleting a queue item does not delete its run;
- deleting or replacing current content does not delete a run or History;
- deleting a run is not a normal cascading operation;
- History has no foreign key to any operational table.

### What not to persist

Do not add:

- a durable delta/event table;
- serialized whole-application snapshots;
- replay sequence numbers;
- cached Statistics totals;
- estimator bucket labels;
- frontend presentation rows;
- runtime process/session state.

`DurableDelta` may survive briefly as an in-process transition result during
the cutover, but it is no longer serialized, versioned, replayed, or sent to
TypeScript. Its eventual name should describe a current transaction change,
not a durable log record.

## Removing current model conflation

The relational cutover is an opportunity to remove duplicated facts rather
than reproducing the current `DurableState` byte for byte.

### Queue outcome versus run outcome

The terminal operation outcome belongs to `conversion_runs`. A queue row needs
its lifecycle and current/last run ID; the queue read model can join the run's
outcome. It does not need a second serialized copy of that outcome.

### Standing verdict versus historical observation

`file_verdicts` answers one operational question: which decisive native run
currently describes this content? It can be represented by the content key and
source run, with the run supplying outcome and evidence.

`history_records` answers a different question: what statistical observation
happened at a point in time? It is never derived by walking through the
standing verdict.

The estimator reads `history_records` directly. It never reaches History
through `FileRecord`, `Verdict`, `ConversionRun`, or `OutputTransaction`.

### Imported provenance

The parked/adopted import model disappears. The disposable V2 Python
translator emits the current strict History interchange format, and V3 imports
those observations directly. No V2 parser, parked inbox, adoption rule, or
backwards-compatibility shim remains in Rust.

## Exact History storage contract

One row is one immutable statistical observation, except for the explicitly
permitted readable-path scrub.

Representative `history_records` columns:

```text
history_id TEXT PRIMARY KEY
happened_at_ms INTEGER NULL
outcome_kind TEXT NOT NULL

source_path_kind TEXT NULL
source_path BLOB NULL
source_path_hash TEXT NOT NULL

input_size_bytes INTEGER NULL
duration_ms INTEGER NULL
width INTEGER NULL
height INTEGER NULL
video_codec TEXT NULL
container TEXT NULL

output_size_bytes INTEGER NULL
analysis_duration_ms INTEGER NULL
encoding_duration_ms INTEGER NULL
remux_duration_ms INTEGER NULL
crf_thousandths INTEGER NULL
vmaf_hundredths INTEGER NULL
requested_target INTEGER NULL
successful_target INTEGER NULL
fallback_floor INTEGER NULL
preset INTEGER NULL
decode_mode TEXT NULL
ab_av1_revision TEXT NULL
ffmpeg_revision TEXT NULL
encoder_revision TEXT NULL
```

The final schema should use outcome-specific `CHECK` constraints where they
remain readable. Absence is `NULL`; there is no separate “unavailable” value.
There is no generic `label` column. A History row's UI label is derived:

```text
readable source path exists -> decoded/display form
otherwise                   -> source_path_hash anonymous label
```

The path hash is retained whether or not a readable path is stored. Privacy-on
recording writes `NULL` to the two readable-path columns at insertion time.
Native observations have a known event time. A translated observation whose
source time is unknown keeps `happened_at_ms = NULL`; import time is not
substituted. History ordering places unknown times after known times and uses
`history_id` as the stable tie-breaker.
Scrub History performs only:

```sql
UPDATE history_records
SET source_path_kind = NULL, source_path = NULL
WHERE source_path IS NOT NULL;
```

The actual statement may add a defensive `source_path_kind IS NOT NULL`
predicate, but the semantic operation is the same. The transaction advances
`history_revision`. It does not modify IDs, hashes, measurements, outcomes,
Statistics, or Estimation, and it makes no physical-erasure promise.

Native History IDs should be deterministic from the durable run ID, for
example `native:<run_id>`. The primary key then enforces exactly one
observation for a reportable native run without a foreign key. Imported
History IDs come from the interchange record:

- unseen ID: insert;
- same ID and identical typed contents: no-op;
- same ID and different contents: reject as a conflict.

Repeated conversions have different run IDs and therefore create different
History rows even when their source hash is the same.

Useful initial indexes are deliberately few:

- `(happened_at_ms DESC, history_id)` for stable keyset pagination;
- `(outcome_kind, happened_at_ms DESC, history_id)` for History filters;
- `(outcome_kind, video_codec, preset, decode_mode)` for estimator cohort
  selection.

Resolution buckets and medians remain Rust policy. The query returns eligible
measurements; Rust assigns current buckets and computes medians. Changing an
estimator policy therefore does not require rewriting stored observations.

## Native path encoding

SQL `TEXT` cannot losslessly represent every native path. All operational
paths and optional readable History paths use one explicit tagged codec:

```text
kind = unix_bytes       data = raw OsStr bytes
kind = windows_utf16le  data = raw UTF-16 code units in little-endian order
```

Never derive the representation from Rust's undocumented internal `OsStr`
layout, and never call `to_string_lossy()` before persistence.

On the producing platform, the codec reconstructs the exact `PathBuf`. On the
other platform, the bytes remain round-trippable and inspectable but are not
eligible for filesystem operations. A copied database may still expose
History through an escaped/display representation; queued/output work with a
foreign path kind is blocked with a typed “path belongs to another platform”
error.

SQLite `INTEGER` is signed. Values that can use all of Rust's `u64`/`u128`
range—filesystem device/inode/volume/file IDs in particular—must use
fixed-width blobs or another exact encoding. File sizes and durations may use
checked non-negative `INTEGER` columns with an explicit `i64` bound.

There is a separate Turso API concern: `Builder::new_local` currently takes
`&str`, so the database file's own location cannot be passed as an arbitrary
non-Unicode `Path`. This does not compromise stored media paths, but it must be
tested against CRFty's platform data-directory resolution and either accepted
as an explicit launch limitation or resolved before adoption.

## Settings

Settings should move into the database.

Keeping `config.json` would preserve a second persistence mechanism, a second
schema-evolution story, separate atomic-write/quarantine code, and an early
read path solely for logger configuration. Human editability is not lost in a
meaningful way: ordinary SQLite tools can inspect/update the settings table,
while the supported editing surface remains the app.

The logger startup order becomes:

1. initialize a path-free, privacy-conservative stderr sink;
2. open the database and load settings;
3. configure the requested log sink and privacy policy;
4. only then perform path-bearing discovery or work.

If settings cannot be read, startup diagnostics must remain anonymized by
default. No separate bootstrap settings file should be introduced.

Settings have no special atomic relationship with a conversion completion,
but sharing the same store removes machinery and lets all released durable
data use one migration policy.

## Revisions, IDs, and frontend publication

`app_meta` contains at least:

- `operational_revision`;
- `history_revision`.

These are invalidation/publication tokens, not cached aggregates.
The database identifier lives in SQLite's `PRAGMA application_id`, not in a
second ordinary column.

`id_allocators(kind PRIMARY KEY, next_value)` allocates queue, claim, and run
IDs with a checked `UPDATE ... RETURNING` inside the same transaction that
first uses the ID. Allocation therefore belongs to the engine, survives
deletion of the highest row, and cannot race shell-side atomics.

After a commit:

- build and emit the current operational read-model snapshot;
- emit History revision and current count when History changed;
- answer History pages through driver requests with filter/sort/keyset cursor;
- never send the whole History collection to TypeScript;
- never ask TypeScript to fold durable deltas.

This makes the backend part of #78 part of the persistence boundary rather
than a later optimization.

## Database initialization and durability

On every connection, set and then verify:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
```

Use a bounded busy timeout for diagnostic clarity even though the process lock
and single connection should prevent ordinary lock contention. Do not enable
experimental MVCC, multiprocess WAL, encryption, generated columns, custom
types, CDC, or in-place vacuum.

A successful commit is durable in the WAL; checkpointing is not part of the
command's publication barrier. CRFty should initially rely on Turso's ordinary
WAL maintenance and perform an explicit checkpoint only at a verified
maintenance boundary such as orderly close or a database export. It should
not checkpoint every command or invent a threshold before the realistic-scale
test shows a need.

The live store may include the main database and WAL sidecars. A raw file copy
while the app is running is not a supported export. Human inspection uses an
ordinary SQLite browser/CLI after the app closes, or a future SQL/CSV History
export. There is no cloud, account, replication, or network requirement.

Turso currently documents WAL mode, `synchronous=FULL`, foreign keys,
`user_version`, `application_id`, `integrity_check`, `quick_check`, and
`wal_checkpoint`. Its compatibility table marks the ordinary transaction,
table, index, constraint, join, grouping, sorting, and limit operations used
here as supported. Foreign-key enforcement is off by default, which is why
connection initialization must enable and verify it.

## Schema evolution

The database has one current schema at runtime. Migrations are forward-only
data transformations, not old domain APIs kept alive:

- `PRAGMA application_id` rejects an unrelated SQLite file;
- `schema_migrations(version, checksum, applied_at_ms)` records every released
  step;
- embedded SQL migrations run in order inside a transaction;
- a database newer than the application is refused without mutation;
- skipped application releases run every missing migration in order;
- a migration failure leaves the previous committed schema/data intact;
- the application opens operationally only after migrations and structural
  validation succeed.

Before V3 is released, migrations may be squashed into schema version 1.
After release, persisted user data is a supported product surface and forward
migrations are mandatory. This requires narrowing the repository's current
“no migration helpers” rule: it continues to ban runtime API/config/history
compatibility shims, but it cannot ban transformations of released durable
data.

V2 conversion remains wholly outside this mechanism. The disposable Python
translator reads V2 and emits the current strict History interchange file.
Rust never opens a V2 history file or journal.

Avoid JSON domain payloads as a way to evade this policy. They still evolve,
but move transformations out of visible SQL schema into retained serde shapes
and custom rewrite code. Small text/blob leaf values are fine; whole queue
items, runs, outputs, settings, or `DurableState` are not.

## Startup, abnormal shutdown, and corruption

Three conditions remain distinct:

### Unsupported schema

The database is structurally valid but its recorded migration version is newer
than this application. Refuse to open it for mutation and tell the user to use
a compatible/newer CRFty version.

### Abnormal prior shutdown

The existing crash sentinel reports that the prior process did not disarm
cleanly. Turso performs WAL recovery when opening. CRFty then validates its
operational rows and reconciles unsettled output transactions. A sentinel by
itself is not database corruption and does not justify a full integrity scan.

### Database corruption or non-database input

Turso 0.7.1 exposes structured `Corrupt` and `NotAdb` errors, and supports
`quick_check`/`integrity_check`. On such an error:

- stop all mutations;
- preserve the database and sidecars untouched;
- show a distinct database-corruption failure;
- offer inspect/reveal and an explicitly confirmed reset;
- do not pretend a valid event prefix can be replayed;
- do not automatically salvage rows into a new authority.

Integrity checks belong in diagnostics, fault tests, or follow-up handling
after a database error. Running them on every startup would add cost without
improving the normal WAL recovery contract.

The process-wide data-directory lock and crash sentinel remain useful. The
journal generation signature, valid-prefix acknowledgement, and compaction
recovery protocol do not.

## Turso fit and deliberately unused features

As of Turso 0.7.1:

- `Builder::new_local` opens a local database;
- `Database::connect` produces a connection;
- `Connection::transaction_with_behavior` and
  `TransactionBehavior::Immediate` express the needed transaction;
- transactions roll back by default and require explicit async `commit`;
- the Rust error type distinguishes busy, constraint, read-only, full,
  not-a-database, corruption, and I/O errors;
- Turso documents SQLite file-format compatibility and cross-platform support.

CRFty does not require Turso's cloud service, embedded replication, concurrent
MVCC writes, CDC, vector search, materialized views, custom indexes, or online
backup features.

Current compatibility gaps that are irrelevant to this schema include
recursive CTEs, advanced window frames, generated columns, `WITHOUT ROWID`,
and in-place `VACUUM`. Scrub History is a logical `UPDATE`, not physical
erasure, so vacuum is not a privacy requirement.

The relevant cautions are:

- Turso still labels the engine beta;
- its SQL compatibility is partial outside the subset above;
- the Rust API is young and async;
- every active write statement must be fully finished/dropped before another
  write or commit on the same connection;
- ordinary WAL behavior, Windows packaging, checkpoint/close behavior, and
  database-path encoding must be proven with the pinned release.

Pinning the crate makes API churn a normal deliberate dependency update. The
larger concern is correctness of acknowledged local transactions, so the
adoption test emphasizes crashes and restart truth rather than API aesthetics.

Primary references:

- [Turso 0.7.1 Rust API](https://docs.rs/turso/0.7.1/turso/)
- [Turso transaction API](https://docs.rs/turso/0.7.1/turso/transaction/struct.Transaction.html)
- [Turso compatibility matrix](https://github.com/tursodatabase/turso/blob/main/COMPAT.md)
- [Turso PRAGMA reference](https://docs.turso.tech/sql-reference/pragmas)
- [Turso repository/readiness warning](https://github.com/tursodatabase/turso)
- [SQLite application file format](https://www.sqlite.org/appfileformat.html)
- [SQLite WAL documentation](https://www.sqlite.org/wal.html)

## Direct cutover

There should be no dual write and no runtime journal importer.

Delete:

- `JournalWriter` and journal framing/envelopes;
- journal replay/fold code;
- journal schema sequence/version types;
- snapshot-head compaction and replacement retry machinery;
- torn-tail/valid-prefix corruption acknowledgement;
- serialized `DurableDelta` compatibility surface;
- JSON `ConfigStore`;
- shell-owned durable ID atomics;
- frontend durable delta folding;
- parked/adopted imported-history state and provenance.

Retain or reshape:

- the driver thread and bounded command boundary;
- pure Rust transition validation;
- the data-directory lock;
- the crash sentinel;
- output identity/reconciliation logic;
- path privacy helpers;
- strict History import parsing;
- process/session runtime state.

Add focused engine modules rather than a generic persistence framework:

```text
storage/
  mod.rs            connection ownership and transaction entry
  schema.rs         current schema and forward migrations
  path_codec.rs     tagged native-path and integer identity codecs
  operational.rs    aggregate loading/writing and snapshots
  history.rs        insert/import/query/scrub/estimator cohorts
  settings.rs       settings reads/writes
```

Hand-written parameterized SQL and small row-mapping functions are preferable
to an ORM or a repository trait. They make the exact schema, transactions, and
Turso behavior visible and keep a future engine switch local without paying
for a speculative abstraction today.

## Issue and ADR impact

Recommended sequencing:

1. Complete #89's Turso adoption proof and record the decision.
2. Supersede ADR-004, ADR-009, and the journal-specific portion of ADR-011;
   revise the ownership/path/evolution consequences in ADR-002 and ADR-017.
3. Land the database cutover together with #79's first-class History model,
   #84's engine-owned durable IDs/settings, and #78's post-commit operational
   snapshot publication. Keeping the old journal/import/frontend folds during
   this cutover creates throwaway compatibility work.
4. Build #77 directly over database History queries.
5. Move #57 estimation to History cohort queries.
6. Implement #51 as the narrow History path update above.
7. Close #81 as superseded when replay disappears, retaining its useful
   invariant cases as core transition and database integration tests.

ADR-012 and ADR-015's projection/adoption premise is superseded by #79.
`docs/HISTORY_IMPORT.md` must be rewritten when #79 lands; its current parked
inbox accurately describes implemented code but not the proposed system.

## Adoption gates

Do not write the replacement ADR until one disposable Turso spike against the
pinned crate proves all of the following on Linux and Windows:

1. **Build and location**
   - packaged app opens its real application-data path;
   - behavior for a non-Unicode database directory is explicitly resolved;
   - ordinary Unicode, spaces, and long Windows paths work.
2. **Transactions**
   - representative queue/reserve/prepare/output/finish changes commit;
   - run settlement and History insertion are indivisible;
   - constraint and statement failures roll back the complete transaction;
   - every statement lifecycle is compatible with sequential writes.
3. **Crash truth**
   - kill before commit: old state;
   - kill during commit/fsync: restart reports one coherent state;
   - kill after commit before publication: restart reports new state;
   - output-ledger boundaries reconcile exactly once.
4. **Migrations**
   - DDL and data transformation roll back on failure;
   - v1 can skip to v3;
   - a future schema is refused;
   - kill during migration preserves a coherent schema.
5. **WAL and interoperability**
   - `synchronous=FULL` and foreign keys read back as enabled;
   - WAL growth/checkpoint behavior is recorded under realistic writes;
   - clean close/checkpoint is readable by an ordinary SQLite tool;
   - restart with WAL sidecars present restores committed rows.
6. **History**
   - 100,000 observations paginate and select estimator cohorts acceptably;
   - repeated source hashes remain separate observations;
   - deleting all operational rows leaves History unchanged;
   - identical/conflicting import IDs behave as specified;
   - scrub changes only readable source-path columns.
7. **Path fidelity**
   - Unix non-UTF-8 bytes and Windows unpaired UTF-16 units round-trip through
     the storage codec;
   - foreign-platform tagged paths are preserved but never executed.
8. **Failure classification**
   - future schema, `NotAdb`, `Corrupt`, constraint, full/read-only, busy, and
     I/O failures reach distinct application errors where Turso exposes them.

This spike should use the proposed SQL directly and be discarded after its
results are recorded. It is evidence for the ADR, not the beginning of a
generic storage layer.

## Review result

Nothing in the current product model requires a custom journal, multiple
plaintext files, a key/value index layer, event sourcing, or a server. A local
relational current-state database removes more bespoke correctness machinery
than it adds, while making History's actual lifecycle natural.

Turso is a credible implementation of that architecture. The proposal should
advance to the bounded adoption proof above. If the proof fails on local
durability, Windows packaging, or database-path handling, bundled SQLite via
`rusqlite` remains the same architecture with a different engine; the domain
and schema decision need not be reopened.
