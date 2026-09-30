---
status: accepted
date: 2026-08-16
---

# Key durable facts by sampled content identity

## Context and problem statement

Analyses and verdicts are expensive to produce and they describe content, not locations, but the Python application keyed them by path hash: renaming or moving a file discarded its history, and two copies of the same video were searched twice. V3 needs one durable key for reusable facts, and choosing it means deciding what the key is derived from, how much of a multi-gigabyte file may be read to compute it, and what the resulting equality is allowed to authorize.

## Decision drivers

* A move or a rename must not discard an analysis or a verdict
* Content already judged once must not be searched again, wherever it now lives
* Identity cost must be bounded and independent of file size
* Durable facts must survive reinstall and data-directory relocation
* No destructive action may rest on a probabilistic claim

## Considered options

* Key durable facts by path, as the Python application did
* Key by a full-file hash, with collision confirmation
* Key by a sampled hash over a probe-derived header plus fixed regions of the file
* Key by a sampled hash salted per install

## Decision outcome

Chosen option: **a sampled content hash**, because it makes a moved file the same file at bounded cost, and because the facts it keys are advisory rather than destructive, which is what makes probabilistic equality an honest fit rather than a compromise.

Mechanics, fixed by this record:

* `ContentKey` is BLAKE2b with a 16-byte output over the domain separator `ck1`, then the file length, the probe-reported duration in milliseconds, the length-prefixed codec name, the width and the height, then content bytes. Its text form is `ck1:` followed by the digest in hex.
* A file of 4 MiB or less is hashed whole. A larger file contributes its first 256 KiB, 64 KiB at each quarter with the offset rounded down to a 4 KiB boundary, and its last 256 KiB, so any large file is identified by about 704 KiB of reads.
* Sampling is guarded on both sides. The sampled bytes come from one opened file, whose identity is observed before the first read and again after the last. A fresh path observation must match both, or the operation fails instead of producing a key. Inspection also requires that post-read identity to match the observation taken after the probe, so the key and its probe header describe one object.
* Durable facts live in `records`, a map from `ContentKey` to `FileRecord` (media metadata, the analysis index, the standing verdict, imported provenance). Locations live in `paths`, a map from `PathHash` to a binding of full filesystem identity plus the `ContentKey` it resolved to.
* The two hash spaces are distinct newtypes over distinct domain separators, `ck1` for content and `ph2` for canonical paths, so a location key and a content key cannot be interchanged or compared.
* Content equality may reuse an analysis and may skip queued work with a visible typed reason (`SkipReason::ProbableDuplicate`). It authorizes nothing destructive: promotion, overwrite, and original retirement each revalidate exact filesystem identity (file ID, size, modification time) immediately before acting, and any mismatch aborts the step. Each of those observations reads all its fields from one object, as described under observation limits.
* The digest carries no per-install or per-machine salt, so the same bytes yield the same key on every machine and after any reinstall.

### Observation limits

Exact filesystem identity means equality of the observed file ID, size, and modification time. Each observation reads those fields from one object:

* On Unix, one `stat` of the path supplies device, inode, size, and times. Opening the path instead would block on a named pipe and require read permission.
* On Windows, stable `Metadata` carries no file ID, so the path is opened once without data access, and both the metadata and the file ID are queried on that handle. The ID query comes from the `file-id` fork described below. It prefers the 128-bit `FileIdInfo` ID with its volume serial and falls back to the 64-bit file index only when the filesystem rejects `FileIdInfo` as unsupported.
* The fallback index carries less identifying information, especially on ReFS, and on File Allocation Table (FAT) volumes it follows the directory entry's position. The two ID variants never compare equal.

Change time is assessment evidence only. Unix `ctime` is recorded with each observation, but it never enters the content digest, inspection stability, or destructive identity, because renames, permission changes, and link-count changes move it without touching content. The safe Windows handle query exposes no `ChangeTime`, so Windows observations carry none.

These observations do not prove unchanged bytes throughout a run:

| Mutation or interval | What the observations can establish |
| --- | --- |
| Same-size write followed by restored `mtime` on Unix | Destructive identity matches; a changed `ctime` reveals the write when the filesystem records a later change time |
| Same-size write followed by restored last-write time on Windows | Every observed field can match |
| Write while another process holds a write handle on Windows | Last-write time may not update until that handle closes, and mapped-view writes may never update it |
| Write through a hardlink | The shared object's size, `mtime`, or Unix `ctime` can reveal it; equal fields cannot exclude it |
| Path replaced and restored between observations | Both observations can match and miss the intermediate object |
| Another object reusing a freed file ID with equal size and copied `mtime` | Destructive identity matches; on FAT a new file in a freed directory entry takes the old index |
| Filesystem reporting no usable modification time | Identity reduces to file ID and size |
| Symbolic link at the observed path | Observations describe the link target, while rename and deletion act on the link |
| Write after the final observation or between a guard and a path operation | The preceding check cannot detect a future change |
| Different bytes outside the sampled regions with matching probe header | The sampled content key matches; its equality is only probable |

The sampled key names observed content; it does not qualify every later result recorded under that key. Reusable results require the phase assessment defined in the [source-continuity contract](../design/source-continuity.md). That assessment is an alpha requirement not yet enforced by search publication or output planning.

### Consequences

* Good: A moved, renamed, or copied file keeps its analyses and its verdict, and a duplicate costs a skip rather than a search
* Good: Identity cost is bounded, so a large file is keyed as cheaply as a small one
* Good: A restored or migrated data directory still matches the media it describes
* Bad: Equality is probabilistic, because files differing only outside the sampled regions collide, so every destructive path must carry its own exact-identity check
* Bad: The header includes probe-reported duration, codec, and dimensions, so a probe reporting different values for unchanged bytes yields a different key and a re-analysis
* Bad: The key is deterministic and unsalted, so anyone holding a history file plus candidate media can test which media it describes; it identifies content rather than location, and it crosses IPC as the record map key, so it is not treated as a secret

## More information

Implementation: the digest and its stability guards are in `crates/crfty-engine/src/media.rs`; the key and identity types are in `crates/crfty-core/src/output.rs` and `crates/crfty-core/src/media.rs`; the state shape is `DurableState` in `crates/crfty-core/src/state.rs`; the duplicate rule is in `crates/crfty-core/src/policy.rs`; destructive revalidation is in `crates/crfty-engine/src/output.rs`.

The construction is pinned by golden fixtures (`ck1_matches_independent_golden_fixtures`), so changing the digest is a deliberate act with a visible diff rather than an accident. `crates/crfty-engine/tests/source_observation.rs` asserts the restored-timestamp and replace-and-restore limits on Windows and Linux.

The Windows file ID query uses `Loufe/notify` commit `33e1f19`, tagged `crfty-file-id-handle`. It forks notify's `file-id` crate at `74c0e72`, a base that already carries unreleased changes after the 0.2.3 release while keeping that version number. The fork commit adds a safe `get_file_id_from_file` over an opened handle and limits the fallback to `ERROR_INVALID_PARAMETER` and `ERROR_NOT_SUPPORTED`. It adds no unsafe code beyond upstream's. Return to a crates.io release once one exposes a handle-based query, or to the standard library once `MetadataExt::file_index` stabilizes.

Related: ADR-004 (the journal these records persist to), ADR-007 (what a reusable analysis must match beyond content), ADR-015 (the projections that read these records), and ADR-017 (row identity in Analysis, which restates the probabilistic limit for large files).

[Borg's change detection](https://borgbackup.readthedocs.io/en/1.4.5/usage/create.html) and [restic's metadata checks](https://restic.readthedocs.io/en/stable/040_backup.html#file-change-detection) explain why change time can detect writes that preserve modification time, and why metadata-only changes also trigger it. These precedents inform source observation; they do not change the sampled digest.

[Microsoft's file-time contract](https://learn.microsoft.com/en-us/windows/win32/sysinfo/file-times) documents delayed last-write updates while write handles remain open. Missing or matching timestamps therefore cannot establish an immutable input.
