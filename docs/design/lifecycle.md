# App Lifecycle

Status: design note; the cancellation section is pending the supervision unification proposed in ADR-018  
Owning issue: [#85](https://github.com/Loufe/AB-AV1-GUI/issues/85)  
Last updated: 2026-08-16

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

Two signals sharing one outcome fold:

- **Stop After File** sets a session flag, lets the active file finish its whole fallback/search/encode sequence, and prevents another claim.
- **Force Stop** cancels the child process immediately, waits for termination and cleanup, and folds `Stopped`.

The active item cannot be removed, reordered, or retyped during a run.

## Containment and guards

- The ab-av1 adapter runs inside `catch_unwind` because upstream contains production unwrap sites; panic recovery cancels children, reconciles staging, and reports worker failure. A driver panic stays fatal and relies on write-ahead recovery.
- Logging initializes before any durable-state work. A failing log sink reports once to stderr and the app keeps running: logging must never abort work.
- `keepawake` inhibits system sleep only while a session runs; the display may still turn off.
- The application release check is one-shot and user-initiated. No background checking exists and no setting enables it.

## Settings

One strict, atomically replaced `config.json`. Missing or invalid config is renamed `.invalid-<timestamp>` and the whole typed default is used; there is no per-field IPC, no legacy alias, no migration layer. `set_settings` validates the complete object, writes it atomically, then applies and emits it; failure changes neither live nor UI state. Encoding targets (the VMAF window, CRF step, SVT preset, and the pixel floor) are product constants, not settings. Queue contents and order are journal state; window geometry, tab, expansion, selection, and scroll are frontend session state and are never persisted.
