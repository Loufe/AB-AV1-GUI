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

Chosen option: **an application-owned journaled transaction**. Staging preserves the input while encoding, and the journal records the intent and artifact facts used for recovery. This protects the application's commit sequence; it does not prevent external writers from changing or deleting files.

Mechanics, fixed by this record:

* Staging sits in the destination directory, named `.<stem>.crfty-<run id>.part.<extension>`, created with exclusive create-new semantics and then synced. Sharing the destination's directory keeps promotion a rename inside one filesystem, and the embedded run id is what lets recovery claim a staging file whose identity was never journaled.
* Intent is journaled before the file exists. `OutputStarted` records input identity, staging path, final path, the destination preimage, and the replacement mode; only then is staging created and pinned by `StagingCreated`. A crash between the two leaves a file the journal knows how to abandon, and a crash before either leaves nothing behind.
* The encoder receives the staging path as its output and never the input or the final path, and ab-av1's overwrite-input argument is fixed to false, so no commit semantics are delegated to it.
* `OutputReady` records a verified staging artifact, where verification means an AV1 check with a minimum size rather than a size test alone, and the ready record is accepted only if it pins the same file id the staging record pinned.
* Promotion renames staging over the final path, syncs the parent directory where the platform supports it, and journals `OutputCommitted` only after the promoted file re-verifies to the staging content key and size. Because a rename replaces a single directory entry, hardlinked siblings of a replaced input keep the original bytes.
* Retiring the original is a separate two-step transition, `RetireOriginalIntent` then `OriginalRetired`, taken only in the retire-original replacement mode. Planning rejects that mode when the observed destination identity equals the input identity. Same-path Matroska replacement uses `KeepOriginal`, meaning no separate retirement step; it does not preserve the original pathname's contents.
* Every rename and every removal is preceded by an exact `DestructiveIdentity` comparison (filesystem file id, size, modification time) on each path it touches: promotion revalidates staging and the destination preimage, retirement revalidates the committed output and the original, and staging removal revalidates the staging file. A mismatch aborts the step and settles the transaction as a conflict instead of acting.
* Staging cleanup uses one abandonment path: `AbandonStagingIntent` records the observed identity, removal rechecks that identity, and `Abandoned` is journaled. Partial staging must retain its recorded file id. Cancellation can also abandon ready staging, but only under an exact match with its verified identity. `Ready` alone does not prove staging remains: promotion may have renamed it before the commit record.
* Cancellation reports `Stopped` only after successful abandonment. If verification is interrupted after promotion, or staging ownership or cleanup cannot be established, files are preserved and the existing output-conflict failure remains visible. No fresh probe bypasses cancellation, and no later retirement or automatic recovery is scheduled for a settled conflict.
* The recovery decision is pure. `recover_output` maps a transaction plus filesystem facts to one action, and the engine only executes what it returns; startup repeats this per unsettled transaction until it settles. A promotion that already happened is recognized rather than repeated, because a ready transaction whose staging is gone and whose destination carries the expected content key folds straight to committed. Without ffprobe an unsettled transaction is left untouched rather than settled blind.

### Source assessment and retained artifacts

Artifact verification, source continuity, and destructive authorization are separate judgments. The [source-continuity contract](../design/source-continuity.md) defines their required integration across search, encode or remux, retries, and settlement. These source gates and the typed retained-artifact representation are alpha requirements, not shipped guarantees of the transaction above.

The prepared source baseline remains authoritative. Detected changes or unavailable required evidence prevent ordinary success and reusable result publication. A verified artifact is retained with a typed reason when source assessment blocks settlement. Its identity and ownership must survive recovery and compaction without automatic promotion or abandonment.

Source facts needed to qualify completion become durable before promotion can replace the input path. A recovered absence after authorized retirement satisfies a filesystem disposition; it does not establish who deleted the source or prove source continuity during encoding. Fresh path checks still authorize each destructive step, subject to ADR-019's observation limits.

### Consequences

* Good: Staging avoids truncating the source during encoding, and the ledger distinguishes interrupted publication from unfinished output
* Good: Recovery targets journaled paths and refuses detected identity mismatches before destructive actions
* Good: Replace mode preserves hardlinked siblings, verified against a real filesystem rather than argued from rename semantics
* Good: The encoder does not own promotion or original retirement
* Bad: Every transition costs a journal append and a sync, and promotion costs a re-verification of the promoted file
* Bad: Parent-directory syncing is a no-op on Windows, so the durability of the rename itself rests on the platform there
* Bad: A conflict deliberately leaves both files in place and needs a human decision, so the safe outcome is not a self-healing one

## More information

Implementation: the transaction type, its state machine, delta validation, and the pure `recover_output` policy are in `crates/crfty-core/src/output.rs`; the filesystem actions and identity guards are in `crates/crfty-engine/src/output.rs`; the flow that drives it is `crates/crfty-engine/src/coordinator/output_flow.rs`, with failure routing in `coordinator/job.rs` and the startup sweep in `coordinator/recovery.rs`.

Crash behaviour has end-to-end coverage in `crates/crfty-engine/tests/durability.rs`. Cases include promotion with retirement, partial-staging recovery, staging left before its record became durable, identity-authorized abandonment, changed-destination conflict without deletion, pre-staging overwrite policy, restaging after failure, and hardlink-preserving same-path replacement.

Related records are ADR-004 for the journal these transitions append to, ADR-002 for the reducer that validates them, and ADR-019 for the promoted output's content identity. ADR-018 is adjacent but decides job cancellation and completion rather than this record's output commit.

[rsync's source-removal policy](https://download.samba.org/pub/rsync/rsync.1#--remove-source-files) separates transfer completion from deletion and refuses removal after observed source changes. [Syncthing's synchronization design](https://docs.syncthing.net/users/syncing) separates staged files from published files and preserves conflict copies. These precedents support separate disposition and evidence judgments; neither substitutes for CRFty's journal recovery contract.
