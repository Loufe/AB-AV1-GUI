---
status: accepted
date: 2026-07-19
---

# Generate IPC Bindings with tauri-specta

## Context and Problem Statement

The frontend must never hand-author IPC or domain types (issue #33 §11): every
cross-boundary type is generated from Rust, and the delta stream must be ordered.
The shell crate that owns this boundary needs a bindings toolchain, a transport,
and a placement inside the workspace.

## Decision Drivers

* IPC drift must be a compile error, not a review concern
* The delta stream requires strict ordering; Tauri's event system does not
  guarantee it under rapid emission
* specta and tauri-specta are release candidates and change between RCs
* Generated output must be verifiable in CI, not trusted to dev-time habits
* The shell contains wiring only (ADR-001); reconnect handling must not grow
  domain logic

## Considered Options

* tauri-specta over one `tauri::ipc::Channel` stream
* tauri-specta typed events for the delta stream
* TauRPC
* Hand-wired string commands over ts-rs types (Yaak's approach)

## Decision Outcome

Chosen option: **tauri-specta over one `tauri::ipc::Channel` stream**, because
Channels are ordered by construction and specta types a `Channel<T>` in exactly
the position this design uses (a command argument), while typed events inherit
the event system's ordering weakness and TauRPC would replace the whole command
surface to improve a position we do not use.

Mechanics, fixed by this record:

* The shell is `crates/crfty-shell`, an ordinary workspace member under the
  workspace lint regime. The Tauri CLI is pointed at it with `--config`; no
  `src-tauri/` directory exists.
* `tauri`, `tauri-specta`, `specta`, and `specta-typescript` are exact-pinned
  (`=`) and bumped only deliberately, in lockstep.
* `crfty-core` derives `specta::Type` on wire-reachable types. specta is pure
  type reflection with no filesystem, process, clock, async, or UI dependency,
  so this does not violate ADR-001. Ephemeral delta types gain `Serialize` for
  IPC only; the journal codec still accepts `DurableDelta` alone, so journaling
  an ephemeral remains unrepresentable (ADR-004).
* Bindings are exported by a deterministic `export_bindings` test in the shell
  crate into `ui/src/lib/bindings.ts`, which is committed. CI regenerates and
  fails on `git diff`. The file is excluded from oxfmt/oxlint so it stays
  byte-identical to generator output. (Handy exports only during dev runs and
  has no freshness gate; issue #33 §16 records the lesson.)
* The wire payload is `Snapshot | Durable | Ephemeral | Degraded | EngineFatal`,
  wrapped with a per-connection sequence number assigned by the shell forwarder.
  Driver effects never cross the boundary.
* Reconnect uses snapshot-in-stream: the forwarder thread maintains a read model
  by applying `crfty_core::fold` — the same pure fold the frontend runs — and a
  new subscription receives that state as its first message, ordered by the
  single forwarder thread. Moving snapshot emission into the driver itself
  remains an open alternative; the wire contract would not change.

### Consequences

* Good: Adding a delta variant breaks the TypeScript build until handled
* Good: Ordering is structural (one channel, one forwarder) rather than a rule
* Good: Stale bindings cannot merge
* Bad: A release-candidate toolchain must be pinned and bumped by hand
* Bad: The forwarder holds a second folded copy of durable state

## More Information

See issue #33 §11 and §16, ADR-001, ADR-002, ADR-004, and tauri-specta issue
#198 (Channel support is limited to command arguments — the one position used).
