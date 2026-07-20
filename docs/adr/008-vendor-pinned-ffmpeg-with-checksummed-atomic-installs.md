---
status: accepted
date: 2026-07-20
---

# Vendor Pinned FFmpeg with Checksummed Atomic Installs

## Context and Problem Statement

V3 executes external FFmpeg/ffprobe binaries and stamps every analysis with
tool revisions (`AnalysisProfile.{ab_av1,ffmpeg,encoder}_revision`). The
application must obtain those binaries for users who have none, know exactly
which build it is running, and never end up in a state where an interrupted or
malicious download leaves it with broken or attacker-controlled tools. The
question is how downloads are trusted, how installs replace each other, and
which tool wins when several sources (user-pinned, app-managed, system) exist.

## Decision Drivers

* An analysis cache hit must describe measurements the current tools reproduce
  (ADR-007 reasoning, extended to tool provenance)
* A user who pinned a binary must never be silently switched to another one
* No install step may ever leave the active tool set half-replaced
* Downloaded archives are untrusted input: traversal, symlinks, and
  decompression bombs must be rejected before anything executes
* Distribution should not require operating signing infrastructure

## Considered Options

* Compiled-in per-OS manifest (pinned URL + SHA-256) as the trust anchor
* Signature verification (minisign) over a remotely fetched manifest
* Install into place with rollback on failure, instead of staged promotion

## Decision Outcome

Chosen option: **compiled-in SHA-256 manifest with staged atomic installs**,
because a hash pinned at compile time authenticates content end-to-end with no
key management, and a promote-only-when-complete install can be interrupted at
any byte without touching the active tools.

Mechanics, fixed by this record:

* The manifest (`vendor/manifest.rs`) pins one BtbN autobuild per OS — tag,
  build id, URL, SHA-256, archive layout, and a decompressed-size cap — the
  same build `media-contract.yml` tests against. Updating FFmpeg is a code
  change that ships through CI, never a runtime poll; `update_available` is a
  local comparison of the installed version against the compiled-in manifest.
* The pinned SHA-256 is the trust anchor. TLS provides transport privacy only:
  the HTTPS client (reqwest/rustls with the `ring` provider) verifies servers
  against operating-system trust roots via `rustls-platform-verifier` — a
  deviation from the originally planned bundled webpki roots, which reqwest
  0.13 no longer offers — and this is acceptable precisely because content
  authenticity never rests on the transport. No signatures: a signing key
  adds infrastructure without adding security over a hash compiled into the
  binary that the user already trusts by running it.
* The archive is streamed to `<vendor_root>/staging/` and hashed during the
  stream; extraction begins only after the digest matches. Extraction is
  fail-closed: only the two manifest-named binaries are written, entries with
  traversal, absolute paths, backslashes, link types, or case-colliding names
  are rejected, and decompressed output is capped by the manifest.
* Promotion is staged and atomic: the extracted install is renamed into
  `installs/<version>/`, then `current.json` is atomically replaced and
  fsynced. The previous install is untouched until the new record is durable
  and pruned only afterwards, best-effort. The one non-additive step —
  clearing a same-version directory left by a broken earlier install —
  happens while `current.json` still names the previous version. Discovery
  deletes stale staging debris on every run.
* Discovery precedence per tool: explicit `CRFTY_FFMPEG`/`CRFTY_FFPROBE` env
  paths, then the managed install, then PATH. An explicit path that is not a
  file is reported `Missing` — fail-closed, no fallthrough. Managed revisions
  come from install metadata (no spawn); system/explicit tools are probed via
  ffprobe's JSON version document, and that FFmpeg version also stands in as
  the encoder revision — no machine-readable SVT-AV1 version exists, so any
  FFmpeg change conservatively invalidates cached analyses (ADR-007).
* Tools swap only while idle: the reducer rejects `Install` unless the
  session is idle with no active run, and rejects `Start` mid-install. A
  claimed job's revisions are frozen into its `JobSpec`; a session snapshots
  the tool set once at start.
* Concurrent instances: staging is per-process and promotion is atomic, so
  the worst case for racing installs of the same manifest is a last-writer
  win over identical content. The real guard is the single-instance data-dir
  lock deferred to #33; until it lands this posture is accepted.
* XZ decoding uses the pure-Rust `lzma-rs`. If it ever fails on a BtbN
  stream, the accepted fallback is the C `liblzma` binding — a dependency
  risk on par with other C-backed crates already in the tree, not a change
  to the first-party unsafe rule (ADR-005).

### Consequences

* Good: Content authenticity is independent of TLS roots, mirrors, and CDNs
* Good: Kill the process at any point during an install — the active tool
  set is either the old one or the new one, never a mixture
* Good: Provenance is honest per source; user-pinned tools are never
  silently substituted
* Bad: Shipping a new FFmpeg build requires a release; users cannot receive
  newer builds without one
* Bad: System/explicit encoder revisions are a proxy, so upgrading system
  FFmpeg re-analyzes even when SVT-AV1 is unchanged
* Bad: Until #33, two concurrent instances can redundantly re-download the
  same archive

## More Information

Related: ADR-003 (pinned ab-av1 adapter; its revision constant is verified
against `Cargo.lock`), ADR-005 (unsafe policy), ADR-007 (identity honesty).
Implementation: `crates/crfty-engine/src/vendor/`; acceptance tests in
`crates/crfty-engine/tests/vendor_{download,extract,install,native}.rs` and
`.github/workflows/media-contract.yml`. Issue #43 is the narrative record.
