---
status: accepted
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
Basic Scan or queue discovery needs it, `ph2` hashes canonical native Unix
`OsStr` bytes or Windows wide units under a platform domain separator. It does
not call `to_string_lossy` or lowercase. Operational Windows paths retain
their verbatim spelling. Alternate Level-0 spellings may remain separate rows
and are joined later by filesystem/probable-content identity.

| Path case | Discovery/native registry | `ph2` / identity behavior | Durable action behavior |
| --- | --- | --- | --- |
| Unix Unicode | Supported | Canonical `OsStr` bytes | Supported |
| Unix non-Unicode | Supported; display is marked lossy when needed | Raw canonical bytes; distinct invalid byte sequences do not collapse through replacement characters | Queue/analyze/convert deferred unless the path round-trips through the current JSON `PathBuf` representation |
| Windows Unicode | Supported | Canonical native wide units | Supported |
| Windows unpaired wide units | Native registry retains them; display is lossy | Wide units are hashed directly | Durable action deferred under the same round-trip rule |
| Windows case variants | No manual case folding | Filesystem canonicalization decides whether spellings converge | Original operational spelling remains native |
| Verbatim and UNC paths | Retained as native `PathBuf` values | Canonical native result is hashed; prefixes are not rewritten as display text | Supported when the OS operation and JSON round trip support the path |
| Reserved-name spellings | Never synthesized or string-normalized | Existing filesystem object only; ordinary OS errors surface | No alias or fallback is invented |
| Long paths | No application length limit | Hash input is the full canonical native path | OS/configuration support is authoritative |
| Symlink / Windows reparse entry during traversal | Entry may be shown, but directory traversal does not follow it | A directly targeted file canonicalizes to its target identity | #55 uses `symlink_metadata`/reparse detection to prevent traversal cycles |

The existing JSON journal cannot serialize arbitrary non-Unicode `PathBuf`
values reversibly. This record does not silently widen that durable schema.
Discovery and Basic Scan can operate through the native registry. #55 must add
the typed `PathNotPersistable` action result before exposing queue, analysis,
conversion, open, or reveal commands for a row that cannot round trip through
their current boundary. Full durable support remains a separate cross-cutting
decision replacing persisted/IPC `PathBuf` fields with tagged
Unix-byte/Windows-wide-unit data. This ADR does not claim that later schema is
implemented.

Freshness uses full destructive identity. `TimestampReliability` is an engine
fact: core never guesses filesystem granularity or consults a clock. The
engine conservatively classifies a missing mtime as `Unknown` and an
exact-second, future, or at-most-two-seconds-old mtime as `CoarseOrRecent`;
false negatives cause re-observation rather than stale reuse. Queue discovery
carries this judgment with its identity, so its optimization obeys the same
gate as Basic Scan.

| Current condition | Pre-observation decision | After stable observation |
| --- | --- | --- |
| No path binding (new path) | Reobserve: `NoBinding` | New probable key creates a record; existing key joins it |
| Unchanged file id, size, reliable known mtime | Reuse cached observation | No work |
| Touched: same file id/size, changed mtime | Reobserve: `ModifiedTimeChanged` | Join/create by probable key |
| Replaced atomically: changed file id, same size/mtime | Reobserve: `FileIdentityChanged` | Join/create by probable key |
| Changed size, same file id | Reobserve: `SizeChanged` | Join/create by probable key |
| Moved path with no binding | Reobserve: `NoBinding` | Usually joins the existing probable key; add the new full binding |
| Duplicated path with no binding | Reobserve: `NoBinding` | Usually joins the existing probable key; retain both bindings |
| Unknown mtime (including `None`) | Reobserve: `UnknownTimestamp` | Publish only a stable observation |
| Known but coarse or recent mtime | Reobserve: `CoarseOrRecentTimestamp` | Publish only a stable observation |
| File changes between initial identity and post-probe identity | No success | Typed `ChangedAfterProbe`; publish no observation/adoption |
| File changes during content sampling | No success | Typed `ChangedDuringSampling`; publish no observation/adoption |
| Exact settled replace-output identity with reliable known mtime | `RecognizeSettledOutput`, before consulting the stale source binding | Preserve Level 3/output relationship |
| Settled identity with unknown/coarse/recent mtime | Reobserve; do not recognize by metadata shortcut | Compare probable output content only after stability |
| Missing file | `Missing` | Row becomes unavailable; no cache reuse |
| Stat/identity inspection failure | `Unavailable` | Surface failure; no cache reuse |

`PathBinding` retains the observed `DestructiveIdentity` alongside the
probable `ContentKey`; queue discovery also carries full destructive identity
rather than a weak `FileStamp`. The engine compares destructive identity
before probing, after probing, and after sampling. `ObservationStability` is
the typed core outcome; the current media adapter maps instability to
`io::ErrorKind::Interrupted`, and #55/#56 carry it as a typed row failure.
Because `PathBinding` is durable and `ph2` deliberately replaces the old path
namespace, this change advances the journal schema to 14; schema mismatch is
reported before payload decoding rather than treating an older binding shape
as corruption.

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
