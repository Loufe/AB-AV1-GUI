# App lifecycle

Status: design note; the cancellation section is pending the supervision unification recorded in ADR-018

## Purpose and boundary

Startup, shutdown, cancellation, and the always-on guards. Related decisions: ADR-005 (no first-party unsafe), ADR-008 (data-directory lock), ADR-011 (corruption acknowledgment), ADR-018 (unified job cancellation and completion, proposed).

## Startup order

1. Resolve paths and initialize tracing with the configured privacy filters
2. Take the engine-owned data-directory lock
3. Arm the crash sentinel
4. Load settings
5. Fold the journal
6. Inspect ledger-authorized filesystem facts and append/fsync recovery deltas
7. Start the driver and the ab-av1 adapter
8. Mount commands; UI snapshot

Tool availability and version checks run after the window appears. The lock precedes settings load and journal fold so a second instance never reads (let alone writes) shared durable state. The sentinel is armed while holding the lock and before any durable state is touched, and disarmed only on clean driver shutdown: a leftover sentinel makes the next boot report an abnormal shutdown. That report is informational, scoped to the whole run, because journal replay already restored durable state.

## Shutdown and close

Closing during active work prompts rather than hiding to a tray; there is no system tray and there are no native notifications. The shell defers the close, the frontend owns the prompt, and every choice except "keep converting" arms a quit that re-issues the close once the session reaches idle. Shutdown never relabels ordinary Stop as active-file cancellation.

## Cancellation

The [Queue contract](../QUEUE.md#stopping-and-containment) owns shipped Stop After File, Force Stop, worker completion, and process-containment behaviour, including current cancellation limits. [ADR-018, unify job cancellation and completion](../adr/018-unify-job-cancellation-and-completion.md) selects the pending supervision replacement.

## Interrupted work

The [Queue contract](../QUEUE.md#restart-and-interrupted-work) owns reservation release, prepared-run recovery, terminal classification, and retry after interruption. [ADR-020, own output promotion as a journaled transaction](../adr/020-own-output-promotion-as-a-journaled-transaction.md) owns recovery's filesystem authority.

A definitive preparation rejection records its reason as an internal failure before the worker reports the fatal protocol error. Unknown durability, disconnection, or an unexpected reply does not authorize another terminal write.

## Containment and guards

- The ab-av1 adapter runs inside `catch_unwind` because upstream contains production unwrap sites; panic recovery cancels children, reconciles staging, and reports worker failure. A driver panic stays fatal and relies on write-ahead recovery.
- Logging initializes before any durable-state work. A failing log sink reports once to stderr and the app keeps running: logging must never abort work.
- `keepawake` inhibits system sleep only while a session runs; the display may still turn off.
- The application release check is one-shot and user-initiated. No background checking exists and no setting enables it.

## Settings

One strict, atomically replaced `config.json`. Missing or invalid config is renamed `.invalid-<timestamp>` and the whole typed default is used; there is no per-field IPC, no legacy alias, no migration layer. `set_settings` validates the complete object, writes it atomically, then applies and emits it; failure changes neither live nor UI state. Encoding targets (the VMAF window, CRF step, SVT preset, and the pixel floor) are product constants, not settings. Queue contents and order are journal state; window geometry, tab, expansion, selection, and scroll are frontend session state and are never persisted.
