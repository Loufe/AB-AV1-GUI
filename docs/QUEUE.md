# Queue contract

Status: shipped queue behaviour; History storage and job supervision replacements remain pending

## Ownership

The synchronous driver owns application state. The pure reducer decides queue membership, order, job preparation, and terminal validity; the durable fold applies those decisions. The engine coordinator owns the session worker, media inspection, adapter calls, output settlement, and cancellation. The shell submits commands and forwards events. Queue selection, folder grouping, drag feedback, and focus belong to the frontend.

The boundaries follow [ADR-002, centralize mutation in a driver reducer](adr/002-centralize-mutation-in-a-driver-reducer.md). Enqueue and item-administration decisions follow [ADR-013, filter queue adds at enqueue](adr/013-filter-queue-adds-at-enqueue.md). The [event-stream contract](design/event-stream.md) owns delivery and resubscription.

Queue membership and order survive restart through the current file journal. The driver persists durable changes before publishing them or replying successfully. Session state, progress, and session aggregates are ephemeral. The selected database replacement in [ADR-004, persist state in an append-only journal](adr/004-persist-state-in-an-append-only-journal.md) and the observation model in [History](HISTORY.md) do not describe shipped queue storage yet.

## Shape and identities

The queue has a finished prefix, at most one active item, and a queued tail. Active means `Reserved`, `Claimed`, or `Running`. The worker always reserves the first queued item; finishing it extends the finished prefix. Reorder changes only the queued tail. Retry moves a finished item to the tail.

A `QueueItemId` identifies the row across edits and retries. The reducer allocates it when an add is accepted and retains the highest allocated ID after removal, compaction, and restart. Each reservation durably allocates a fresh `ClaimId` and `RunId` from a separate retained high-water mark. Removing an item, releasing a reservation, compaction, and restart do not reclaim those runtime IDs. Exhaustion rejects the complete add batch or reservation without mutating the queue. A prepared run retains its own lineage after its queue item is removed or retried.

Worker commands carry the item, claim, and run identities. Preparation, start, analysis recording, and terminal recording validate the matching active state. Stale commands cannot finish a later retry, and an already terminal run cannot receive another outcome.

## Enqueue

`AddMany` receives ordered requests with an input path, operation, analysis intent, output target, overwrite decision, and optional filesystem facts. In the current path-add bridge, the engine expands files and folders using configured extensions; the shell submits the resulting requests. Callers never allocate item IDs. Execution settings are resolved later, at preparation.

The reducer validates every output target and rejects the whole batch on an invalid target or exhausted item IDs. Paths that cannot round-trip through the JSON journal contribute `PathNotPersistable` to the summary. Duplicate detection compares input paths against unfinished queue items and requests already accepted in the batch. A duplicate contributes `AlreadyQueued` to the summary. Finished rows do not block a new add.

For a path not already unfinished, the reducer invokes `evaluate_enqueue` over the supplied identity and cached content facts. [Policy's verdict and freshness rules](POLICY.md#verdicts-and-freshness) own the evaluation order, exclusions, and `Refresh` bypass; [media eligibility](POLICY.md#media-eligibility) owns resolution and container decisions. Missing facts or unreliable timestamps leave the request queueable. Analyze adds remain available even when an analysis is cached, because qualifying reuse is selected at preparation.

Accepted requests append `Queued` rows in request order. Exclusions create no row and no terminal outcome. One ephemeral `QueueAddSummary` contains the added count and counts by typed `SkipReason`; the frontend renders it as neutral action feedback. A request accepted with weaker facts can still become a visible skip after preparation observes the source.

Enqueue uses filesystem facts supplied by the engine and performs no media probing in core.

Analysis actions enqueue selected rows or all published files from one current generation through the same path and ID rules. The engine supplies native paths, and the reducer uses committed output Settings and each scanned row's operational eligibility. Discovered files can enqueue before Basic Scan finishes. This action returns an `AnalysisQueueSummary` with typed selection, scan, and policy exclusions; it emits no `QueueAddSummary` on the stream. The [Analysis queue-action contract](ANALYSIS.md#queue-actions) owns generation checks, discovery order, identity deduplication, and exclusion details.

## Item administration and reorder

| Command | Shipped rule |
|---|---|
| `Edit` | Idle session and queued item only. Resolves a per-item patch for operation, intent, output target, and overwrite. An unchanged patch emits no durable change. |
| `Retry` | Finished item only. Preserves its item ID, resets it to `Queued`, and moves it to the end. An unpatched retry is allowed during a session; any supplied patch requires idle, even if its values are unchanged. Previous runs remain intact. |
| `RemoveMany` | Requires a nonempty, unique set of existing queued or finished item IDs. Validates the complete set, then removes it atomically in queue order. Active items cannot be removed. |
| `Clear` | Idle only. Removes queued and finished items. |
| `ClearCompleted` | Removes finished items except `Failed` and `Incomplete`, including stopped and skipped items. Allowed during a session. |
| `ReorderPending` | Requires exactly every queued ID, once each. Unknown, active, finished, duplicate, or omitted IDs reject the complete order. An unchanged order emits no durable change. |

Add, remove, unpatched retry, and reorder remain available during a session within these rules. A prepared job's parameters cannot be edited, and active or finished rows never participate in reorder. Removing a finished queue row does not remove its run or content facts.

Frontend moves produce one complete pending order for the reducer to validate. A selected block keeps its authoritative relative order. The drag adapter translates dnd-kit's optimistic single-row index into a block destination, following D6 in the [UI verdicts](design/ui-verdicts.md). Folder headers represent contiguous presentation groups; they do not add durable hierarchy. Drag feedback does not write engine state, and a rejected or superseded submission returns to the authoritative order.

## Session and run sequence

`Start` requires an idle session and located media tools. It sets the session to `Running`, resets session aggregates, and asks the supervisor to start one worker. The worker holds one tool-path snapshot for the session. In production it runs the bounded capability probe before reserving anything, as specified by [ADR-023, require a user-supplied FFmpeg toolchain](adr/023-require-a-user-supplied-ffmpeg-toolchain.md). Probe failure or cancellation finishes the session without reserving an item.

The supervisor joins any previous session worker before replacing it. The worker processes one item at a time:

1. `ReserveNext` requires a running session with no active item. It selects the first queued row and persists its reservation and fresh identities. An empty queue returns no job.
2. The worker opens the run's cancellation scope and inspects the input. Probe failure is logged and preparation continues without reusable observation facts. Force Stop during inspection abandons the reservation as stopped.
3. `PrepareReserved` validates the reservation and composes execution from current driver-owned settings, verified tools, the observed codec, and the item's overwrite decision. It chooses the action and any reusable analysis, persists changed media facts and the immutable `JobSpec`, creates the run, and marks the item `Claimed`.
4. `Started` persists the start timestamp and marks the matching prepared item `Running`.
5. The coordinator executes the prepared action, records successful analysis when needed, settles any output transaction, and submits one terminal command.
6. The worker attempts the next reservation. An empty queue or stopping session ends the loop. `Finished` returns the session to idle only when no item remains active.

Preparation selects `Skip`, `Analyze`, `Encode`, or `Remux` using the observed content, current policy, analysis intent, and resolved execution. A qualifying cached analysis can satisfy Analyze immediately or let Encode omit search. A fresh successful search is recorded durably before Analyze completes or Encode starts.

The current preparation path can also adopt matching parked import facts. That shipped path is described in [History import](HISTORY_IMPORT.md); [ADR-025, import history as keyed translated observations](adr/025-import-history-as-keyed-translated-observations.md) selects its replacement.

## Phases and output boundary

Queue state and job phase answer different questions. `Running` includes preparation, search, output verification, and settlement; phase updates are progress facts, not additional durable queue states.

| Prepared action | Coordinator path |
|---|---|
| Skip | Preparing, then the prepared skip reason becomes a terminal outcome. |
| Analyze | Preparing, optional Analyzing, then Finalizing and Analyzed. |
| Encode | Preparing and destination checks, optional Analyzing, staging creation, Encoding, Verifying, then Finalizing and output settlement. |
| Remux | Preparing and destination checks, staging creation, Remuxing, Verifying, then Finalizing and output settlement. |

[Policy's quality target rules](POLICY.md#analysis-reuse-and-quality-targets) own the search ladder and not-worthwhile decision; its [hardware-decode rules](POLICY.md#hardware-decode) own software retries. The coordinator uses those rules within the same prepared run. The requested `JobSpec` stays unchanged; retained analysis and completion evidence identify the decode mode actually used.

Output intent is durable before staging is created. A successful process result enters verification and finalization; it becomes Converted or Remuxed only after the output transaction proves successful settlement. Cancellation or failure settles staging before the terminal command. Cleanup conflicts remain failures. [ADR-020, own output promotion as a journaled transaction](adr/020-own-output-promotion-as-a-journaled-transaction.md) owns promotion, retirement, and identity-based deletion rules.

The terminal command carries the outcome, completion time, monotonic phase spans, and optional final progress. It validates action-specific evidence and refuses an unsettled output transaction. The durable fold finishes both the queue row and its prepared run together. Progress is lossy; phase spans arrive through the terminal command and do not derive from telemetry. Final progress and its clear publish before the durable finish; updated session aggregates publish afterward.

## Stopping and containment

Stop After File changes `Running` to `StopAfterCurrent`. It sends no cancellation effect. A reservation already made continues through inspection, preparation, search, encode or remux, and settlement. The next `ReserveNext` is refused. If Stop arrives before the first reservation, the capability probe completes and no item is claimed.

Force Stop is accepted from `Running` or `StopAfterCurrent`. It sets `ForceStopping` and sends cancellation to the active run, or to session work when no run exists. The supervisor latches that intent: a probe, run scope, or adapter registered afterward observes it on registration. The worker awaits a report and output settlement rather than treating the request itself as completion. Both stopping states remain non-idle until `Finished` confirms there is no active item.

The current cancellation registry connects three mechanisms: `ProcessCancellation` for supervised probes, the ab-av1 watch-channel handle for search and encode, and the remux cancellation handle. Supervised processes and remux use `ContainedChild`; Unix uses a process group and Windows a Job Object with hidden process creation. Cleanup terminates the containment unit and waits for the direct child, and supervised process readers are joined. This does not imply that a Unix parent reaps arbitrary grandchildren.

CRFty supervises quality search as an official ab-av1 CLI operation and encoding as a direct FFmpeg operation. Direct media and capability probes have deadlines and bounded captures; individual upstream internal probes remain opaque. The [probe boundary](AB_AV1_PROBES.md) and [containment contract](PROCESS_CONTAINMENT.md) record the guarantees and platform limits.

A clean user cancellation records `Stopped` only with absent or safely abandoned output. A cleanup failure remains `Failed`; cancellation after promotion cannot authorize deletion of an identity-mismatched artifact. A completed and settled run can retain success even if Force Stop arrives during its final settlement. Shutdown force-cancels registered work and joins the session worker. [ADR-018, unify job cancellation and completion](adr/018-unify-job-cancellation-and-completion.md) selects the pending shared supervision implementation; [ADR-021, drive ab-av1 through an owned operation](adr/021-own-media-operation-processes.md) selects the pending dependency boundary.

## Restart and interrupted work

Startup releases a reservation with no prepared run back to its existing queue position. Its runtime IDs stay consumed. Release is not a terminal outcome and does not start processing.

Prepared interruptions are reconciled with the output ledger before terminal recording. A successfully settled output proves Converted or Remuxed with `RecoveredAtStartup` evidence; a conflict produces Failed. Absent or abandoned output produces Incomplete: completion was not recorded, without attributing the interruption to a crash or user cancellation. Recorded analysis survives but cannot establish completion by itself. Recovery uses its actual decision time and supplies no invented phase spans or live size evidence.

When unsettled output requires unavailable media tools, recovery leaves the item active and its transaction intact for a later startup with tools. It does not settle blindly. Incomplete and failed rows remain available for inspection and explicit retry. Only a user Force Stop authorizes Stopped; restart never uses it as a generic interruption label.

## Source and verification

| Contract area | Implementation and existing coverage |
|---|---|
| Enqueue, administration, and worker transitions | [Core reducer](../crates/crfty-core/src/reducer.rs), [policy](../crates/crfty-core/src/policy.rs), [state and fold](../crates/crfty-core/src/state.rs), and [core decision tests](../crates/crfty-core/src/tests.rs) |
| Persist before publication | [Driver](../crates/crfty-engine/src/driver.rs) and [durability contracts](../crates/crfty-engine/tests/durability.rs) |
| Session sequence and phases | [Session worker](../crates/crfty-engine/src/coordinator/session.rs), [job dispatch](../crates/crfty-engine/src/coordinator/job.rs), and [adapter process contracts](../crates/crfty-engine/tests/ab_av1_process.rs) |
| Output and restart | [Output flow](../crates/crfty-engine/src/coordinator/output_flow.rs), [startup recovery](../crates/crfty-engine/src/coordinator/recovery.rs), and [durability contracts](../crates/crfty-engine/tests/durability.rs) |
| Cancellation and process containment | [Supervisor](../crates/crfty-engine/src/coordinator/supervision.rs), [process supervision contracts](../crates/crfty-engine/tests/process_supervisor.rs), and [remux process contracts](../crates/crfty-engine/tests/remux_process.rs) |
| Path-add boundary | [Shell bridge](../crates/crfty-shell/src/bridge.rs) and [engine input expansion](../crates/crfty-engine/src/scan.rs) |
| Frontend reorder | [Interaction planner](../ui/src/features/queue/queue-interaction-planner.ts), [drag adapter](../ui/src/features/queue/queue-dnd-adapter.ts), and [queue interaction tests](../ui/src/features/queue/queue-view-interactions.browser.test.tsx) |

The [testing strategy](TESTING.md) owns commands and broader suite coverage. Passing those suites does not establish the pending lifecycle fixes or installed alpha acceptance.
