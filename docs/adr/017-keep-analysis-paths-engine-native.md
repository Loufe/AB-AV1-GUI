---
status: proposed
date: 2026-07-21
---

# Keep Analysis Paths Engine-Native

## Context and Problem Statement

Level 0 must discover paths without probing or hashing them, retain arbitrary
native path spellings for later filesystem and process operations, and present
rows to a JSON/TypeScript frontend. The current `PathHash` implementation
canonicalizes and then calls `to_string_lossy`, making a display conversion
part of identity. The current cache binding also discards the filesystem ID
already observed by the engine and treats two absent modification times as a
size-only freshness match.

No portable metadata-only test can prove that file bytes are unchanged when a
writer preserves file ID, size, and modification time. The current
`ContentKey` is also sampled for large files and includes ffprobe-derived
metadata, so it is a probable-content identity rather than a bytewise proof.
The contract must state those limits instead of promising impossible
certainty.

## Decision Drivers

* Operational paths must not be reconstructed from lossy display strings
* Non-Unicode Unix paths and native Windows wide strings must not collide in
  Analysis identity
* Windows case, verbatim, UNC, reserved-name, and long-path behavior must defer
  to native filesystem operations rather than string rewriting
* Atomic replacement with preserved size and mtime must invalidate a cache hit
* Unknown and coarse timestamps must not become size-only freshness
* Replace-mode output recognition must happen before an old source binding is
  overwritten
* The contract must remain honest about sampled identity and adversarial
  metadata restoration

## Considered Options

* Use normalized display paths as row identity
* Use `PathHash` as Level 0 row identity
* Use `ContentKey` as row identity after probing and replace the row key
* Use opaque generation-scoped row IDs backed by an engine-native path registry
* Replace every persisted `PathBuf` with a tagged native byte/wide-unit schema

## Decision Outcome

Chosen option: **use opaque generation-scoped row IDs backed by an
engine-native path registry**, because a row needs stable identity before any
canonicalization, stat, probe, or content sample has occurred.

`AnalysisGenerationId` and `AnalysisRowId` form the public identity. The
engine assigns row IDs monotonically as deterministic breadth-first entries
are accepted and stores the untouched operational `PathBuf`, source root, and
parent relationship in a generation-scoped registry. Core and UI rows carry
only opaque IDs, parent IDs, display-only text, and facts. Every row-targeted
command echoes the generation and row ID; the engine resolves the native path
and rejects stale or unknown references.

Display text may be lossy only when explicitly marked as such and is never
round-tripped into an operation. `PathHash` is not computed at Level 0. When
Basic Scan needs it, Unix hashes canonical native `OsStr` bytes and Windows
hashes canonical native wide units under distinct schema prefixes. It does
not call `to_string_lossy` or blindly lowercase. Operational Windows paths
retain their verbatim spelling. Alternate path spellings may remain separate
rows and are deduplicated later by filesystem/content identity.

The existing JSON journal cannot serialize arbitrary non-Unicode `PathBuf`
values reversibly. This record does not silently widen that durable schema.
Discovery, Basic Scan, open, and reveal may operate through the native
registry, but a row whose path cannot round-trip through the durable path
representation receives a typed `PathNotPersistable` action reason for queue,
analysis, or conversion actions. Full end-to-end support requires a separate
cross-cutting decision that replaces persisted and IPC-visible `PathBuf`
fields with a tagged Unix-byte/Windows-wide-unit representation.

Freshness uses full destructive identity where available:

* No binding, a missing file, or any inspection error does not reuse cache.
* Equal file ID, size, and a reliable known mtime may reuse the cached media
  observation.
* Different file ID invalidates the cache even when size and mtime match.
* Different size or mtime invalidates the cache even when file ID matches.
* Unknown, recent, or conservatively classified coarse mtime is indeterminate
  and requires a full stable observation before reuse.
* A stable observation that returns an existing probable `ContentKey` joins
  that content record; a new key creates a new record.
* Parked adoption occurs only after a successful stable observation.

`PathBinding` therefore retains the observed `DestructiveIdentity` alongside
the probable `ContentKey`; `FileStamp` remains a weak enqueue/display fact.
The engine compares destructive identity before probing, after probing, and
after sampling. Any observed change produces a typed unstable-file result and
publishes no successful observation or adoption.

Replace-mode handling has strict precedence:

1. Resolve the current destructive identity.
2. Consult the old path binding and its native verdict.
3. Compare the current identity with the run's full settled output identity.
4. Recognize an exact match as the settled Level 3 output.
5. Otherwise apply ordinary cache freshness or full observation.
6. After observation, compare probable content identity with known settled
   output identities.
7. Only then update the path binding and invoke imported-history adoption.

Size/mtime equality alone never recognizes a settled output. The common
same-size/same-mtime atomic-replacement case is detected by file-ID change.
In-place mutation that restores file ID, size, and mtime is outside the
metadata fast path's guarantee and requires re-observation or full hashing to
prove. Large-file `ContentKey` equality remains explicitly probabilistic; the
UI and policy use terms such as probable duplicate rather than bytewise
identical.

### Consequences

* Good: Level 0 identity exists before any expensive filesystem work
* Good: Display conversion cannot change the file an action targets
* Good: Atomic replacement no longer hides behind equal size and mtime
* Good: Unknown timestamps no longer become size-only cache hits
* Good: Replace-mode outputs retain their Level 3 relationship
* Bad: The engine must retain a native registry for the current generation
* Bad: Some native paths are discoverable but intentionally ineligible for
  durable actions until a wider path-persistence change lands
* Bad: Metadata and sampled hashing still cannot prove absence of adversarial
  in-place changes

## More Information

See issues #28, #42, #51, #52, #53, #55, and #56; ADR-001, ADR-004, ADR-012,
and ADR-014.
