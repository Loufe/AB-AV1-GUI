# Event stream and IPC

Status: current delivery contract, with the selected replacement described separately

## Purpose and boundary

The synchronous engine driver owns application state. Tauri forwards typed requests and the ordered event stream; React owns presentation and interaction. ADR-002 defines driver ownership, ADR-004 the durability boundary, and ADR-006 generated bindings.

Production delivery combines direct command results with streamed snapshots and deltas. History's bounded request-driven browse contract is decided in [History](../HISTORY.md#browsing); the production view still derives the older per-content rows from the durable frontend snapshot. The feature-gated database candidates expose the decided queries independently of production IPC.

## One ordered stream

- Each connection subscribes through one `tauri::ipc::Channel`. The wire payload carries `Snapshot`, `Durable`, `Config`, and `Ephemeral` variants, plus engine health and shell lifecycle notices.
- Each event carries a per-connection sequence number assigned at send time. On a gap, the frontend resubscribes. A new subscription starts numbering at zero and replaces the frontend's baseline.
- Subscription replays the shell's current snapshot, Analysis generation, session, tools, aggregates, telemetry, and health on that same ordered channel. The shell's read model is maintained even without a subscribed webview.
- The engine's public event queue is bounded. Overflow disconnects the stream rather than blocking the writer or silently dropping individual domain events. The shell marks a disconnected engine stream fatal and requires an application restart; webview resubscription alone cannot repair that lost source.

## Publication and durability

| Payload | Carries | Publication rule |
| --- | --- | --- |
| Durable | Queue, run, media, analysis, output, and terminal facts | File-journal commit before publication |
| Config | Committed Settings | Settings persistence before publication |
| Ephemeral | Analysis generations, session, tools, telemetry, aggregates, command notices, and requested Statistics | Never journaled |
| Snapshot and health | Subscription baseline and engine condition | Ordered with the connection's other events |

Within an applied command, ordinary ephemerals precede durable deltas. Final telemetry and its clear therefore precede the terminal state they describe. Analysis updates and session aggregates publish after the durable facts they summarize. A command's reply follows its emitted events, although forwarding and webview delivery may still be pending when the caller receives that reply.

The adapter retains its latest telemetry, and the coordinator publishes changed progress and rate values. Terminal outcomes follow the owned operation's completion report. The shell forwards events as received; it has no timer or focus-driven telemetry flush.

## Commands and results

Commands return `Result<T, CommandError>` through generated bindings. Most queue and session mutations return acknowledgment. Some commands return typed payloads directly: path pickers return selections, discovery returns its generation identifier, Analysis enqueue returns `AnalysisQueueSummary`, and import, log scrub, and release checks return their summaries.

Analysis enqueue's direct summary is its sole summary. Direct file/folder addition instead returns acknowledgment and emits `QueueAddSummary` on the stream. Statistics requests also return acknowledgment and deliver their answer through an ephemeral event. Statistics answers are absent from subscription replay, so the view requests again after a snapshot.

Command failures cross the boundary as `{code, message}`, with a string code and an inspectable message. Analysis enqueue preserves the distinction between rejection and uncertain durability. Contextual command results own action feedback; streamed rejection notices provide an observability backstop.

Pending queue items may be added, removed, and reordered while a session runs, and the next claim observes the resulting order. The active item is immutable. Reorder cannot move pending work ahead of it, and operation/output editing is disabled for the run.

## Progress and frontend ownership

Rate smoothing is engine-side, shared by search, encode, and remux. It reports frames per second over a three-second sliding window and a progress-velocity `Option` for estimated time remaining. The estimate stays absent during warm-up, when remaining work is unknown, or until velocity is positive.

One frontend consumer routes the stream to stores. Pure TypeScript folds mirror the Rust durable, Settings, and Analysis folds and are checked against generated fixtures. The production frontend derives History rows from the durable snapshot with the corresponding projection fixtures. Progress has its own store so telemetry does not rerender domain subscribers. Rust sends typed facts; TypeScript owns presentation strings.

## Selected replacement

The replacement returns query payloads and meaningful mutation results directly, with typed rejection. After durable commit, Rust publishes a complete operational read model that the frontend replaces through one path for initial subscription, reconnect, and routine updates. This removes frontend durable reconstruction and response correlation.

History and Statistics remain bounded request-driven reads outside that operational model. Streams retain unsolicited runtime information: progress, telemetry, Analysis generations, tool availability, and notices. These are delivery requirements; the production frontend still uses the folds and mixed result paths described above.
