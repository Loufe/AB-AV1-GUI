# Event Stream and IPC

Status: design note; records the contract the accepted ADRs compose into  
Owning issue: [#78](https://github.com/Loufe/AB-AV1-GUI/issues/78)  
Last updated: 2026-08-16

## Purpose and boundary

How engine state reaches the UI and how commands come back. The decisions behind this design are ADR-002 (driver reducer), ADR-004 (durable/ephemeral split), and ADR-006 (generated bindings); this document records the contract they compose into.

It does not settle whether History should be served by request/response rather than folded in the frontend. That question is open in the owning issue.

## One ordered stream

- One `tauri::ipc::Channel` per connection. Deltas flow down, commands flow up, nothing else: the frontend never queries the driver synchronously, and UI-only facts (viewport, expansion, selection) live in the frontend, entering the engine only as command payloads.
- The wire enum is `Durable | Ephemeral | Snapshot`, and durable deltas are exactly the journal line types: no translation layer exists between storage and IPC.
- Every delta carries a per-connection sequence number assigned at send time. The transport is ordered by construction, so the number is a tripwire, not a recovery protocol: on a gap the UI requests a fresh snapshot. A webview reload is a new connection (subscribe, snapshot, numbering restarts at 0).
- Rehydration is snapshot-in-stream: on (re)connect the UI sends one command and the driver answers with a snapshot delta in the same ordered stream as every other delta, so no subscribe-then-fetch race window exists.

## Three lanes

| Lane | Carries | Rule |
|---|---|---|
| Durable state | claim/phase start, analysis checkpoint, output transitions, terminal outcome | Journal before UI, never coalesced |
| Lossless runtime | terminal success/error, cancellation acknowledgment, panic, bounded stderr context | Never coalesced, one reducer command each |
| Coalesced telemetry | search status, encode progress, activity text | Latest value per key at ~100 ms, never journaled, droppable |

- Coalescing holds at most the latest ephemeral per key per tick, and any durable delta flushes held ephemerals first: a stale progress tick can never arrive after the completion that supersedes it. Window focus or visibility regain also flushes, so a tabbed-away UI catches up instantly.
- The forwarder is a shell-side task draining the driver's outbound channel onto the wire even with no live webview (discards are recovered by the reconnect snapshot). A blocked outbound channel must never stall the driver.

## Commands

- The command surface is small and thin. Every command returns `Result<(), CommandError>`, acknowledgment or error, never domain state: all data flows down the one stream. That return type makes "commands up, deltas down" a type-level fact.
- Command failures cross the boundary as `{code, message}`. The code enum stays small and consumer-driven: a variant exists only while the UI branches on it.
- Queue edits while a session runs: pending items may be added, removed, and reordered, and the next claim observes the resulting order. The active item is immutable, reorder cannot move pending work ahead of it, and operation/output editing is disabled for the run.

## Progress hygiene

Rate smoothing is engine-side, one implementation serving the search, encode, and remux phases: fps over a ~3 s sliding window, and ETA a progress-velocity `Option` that is absent during warm-up (early velocity readings swing wildly while encoder pipelines fill) and whenever remaining work is unknown. Absence is always the type, never a sentinel value.

## Frontend fold

- UI state is a fold over keyed deltas: one subscription feeds pure TS fold functions mirroring `crfty_core::fold`, writing normalized flat stores (records, runs, queue order). Progress lives in a separate store so telemetry ticks touch no tree subscription.
- Trees are memoized derivations over the flat stores; no imperative tree-update code exists.
- Golden fixtures exported by the Rust fold (the oracle) are replayed by the TS fold tests, proving the two folds agree. Adding a delta variant breaks the TS build until the UI handles it.
- Rust sends facts (bytes, seconds, floats, levels); TS owns presentation strings.
