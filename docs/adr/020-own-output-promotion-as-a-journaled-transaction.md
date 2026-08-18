---
status: accepted
date: 2026-08-16
---

# Own output promotion as a journaled transaction

## Context and problem statement

An encode produces a new file while the input still exists, and in replace mode the new file takes the input's own path. A crash, a force stop, an adapter panic, or a failed encode can land at any byte of that process, and the Python application encoded toward the destination directly, which is what made its same-path replacement unsafe. V3 has to decide who owns the commit, where partial output lives, which files an interrupted run is allowed to delete on restart, and how restart recovery distinguishes a finished output from an abandoned fragment.

## Decision drivers

* An interruption must never leave the user without either the original file or a complete output
* A partial encode must never be mistaken for a finished one
* Recovery must decide what it owns from the journal plus the filesystem, never from a guess
* The encoder is a third-party tool and must not be trusted with commit semantics
* Hardlinked siblings of a replaced input must keep their bytes

## Considered options

* Encode directly to the final path, as the Python application did
* Delegate commit and overwrite semantics to the encoder, using ab-av1's overwrite-input handling and its temp registry
* Remove the original first, then rename the finished encode into place
* Own a staging-and-promotion transaction in the application, journaled at every step

## Decision outcome

Chosen option: **an application-owned journaled transaction**. It is the only option where an interruption always leaves either the old file or the new one, never neither or a blend, and where recovery can prove which paths it is authorized to touch.

Mechanics, fixed by this record:

* Staging sits in the destination directory, named `.<stem>.crfty-<run id>.part.<extension>`, created with exclusive create-new semantics and then synced. Sharing the destination's directory keeps promotion a rename inside one filesystem, and the embedded run id is what lets recovery claim a staging file whose identity was never journaled.
* Intent is journaled before the file exists. `OutputStarted` records input identity, staging path, final path, the destination preimage, and the replacement mode; only then is staging created and pinned by `StagingCreated`. A crash between the two leaves a file the journal knows how to abandon, and a crash before either leaves nothing behind.
* The encoder receives the staging path as its output and never the input or the final path, and ab-av1's overwrite-input argument is fixed to false, so no commit semantics are delegated to it.
* `OutputReady` records a verified staging artifact, where verification means an AV1 check with a minimum size rather than a size test alone, and the ready record is accepted only if it pins the same file id the staging record pinned.
* Promotion renames staging over the final path, syncs the parent directory where the platform supports it, and journals `OutputCommitted` only after the promoted file re-verifies to the staging content key and size. Because a rename replaces a single directory entry, hardlinked siblings of a replaced input keep the original bytes.
* Retiring the original is a separate two-step transition, `RetireOriginalIntent` then `OriginalRetired`, taken only in the retire-original replacement mode. A replacement whose destination is the input file itself is rejected at planning, so a same-path replacement can never schedule the deletion of its own output.
* Every rename and every removal is preceded by an exact `DestructiveIdentity` comparison (filesystem file id, size, modification time) on each path it touches: promotion revalidates staging and the destination preimage, retirement revalidates the committed output and the original, and staging removal revalidates the staging file. A mismatch aborts the step and settles the transaction as a conflict instead of acting.
* Every failure settles through one abandonment path: `AbandonStagingIntent` records the staging identity actually observed, the file is removed under that identity, and `Abandoned` is journaled. Encode-start failure, cancellation, encode failure, adapter panic, and internal error all route through it, so no failure mode has its own cleanup code.
* The recovery decision is pure. `recover_output` maps a transaction plus filesystem facts to one action, and the engine only executes what it returns; startup repeats this per unsettled transaction until it settles. A promotion that already happened is recognized rather than repeated, because a ready transaction whose staging is gone and whose destination carries the expected content key folds straight to committed. Without ffprobe an unsettled transaction is left untouched rather than settled blind.

### Consequences

* Good: An interruption at any byte leaves either the previous file or a complete new one, and a restart can tell which
* Good: Recovery removes only a staging path the journal named and retires only an original whose identity still matches, so unknown or changed files are never cleaned up
* Good: Replace mode preserves hardlinked siblings, verified against a real filesystem rather than argued from rename semantics
* Good: Encoder bugs cannot lose user data, because the encoder never holds the commit
* Bad: Every transition costs a journal append and a sync, and promotion costs a re-verification of the promoted file
* Bad: Parent-directory syncing is a no-op on Windows, so the durability of the rename itself rests on the platform there
* Bad: A conflict deliberately leaves both files in place and needs a human decision, so the safe outcome is not a self-healing one

## More information

Implementation: the transaction type, its state machine, delta validation, and the pure `recover_output` policy are in `crates/crfty-core/src/output.rs`; the filesystem actions and identity guards are in `crates/crfty-engine/src/output.rs`; the flow that drives it is `crates/crfty-engine/src/coordinator/output_flow.rs`, with failure routing in `coordinator/job.rs` and the startup sweep in `coordinator/recovery.rs`.

Crash behaviour has end-to-end coverage in `crates/crfty-engine/tests/durability.rs`. Cases include promotion with retirement, partial-staging recovery, staging left before its record became durable, identity-authorized abandonment, changed-destination conflict without deletion, pre-staging overwrite policy, restaging after failure, and hardlink-preserving same-path replacement.

Related records are ADR-004 for the journal these transitions append to, ADR-002 for the reducer that validates them, and ADR-019 for the promoted output's content identity. ADR-018 is adjacent but decides job cancellation and completion rather than this record's output commit.
