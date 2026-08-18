---
status: accepted
date: 2026-08-16
---

# Unify Job Cancellation and Completion

## Context and Problem Statement

The engine currently represents the same one-shot supervision contract with four cancellation mechanisms and two nearly identical job handles:

* ab-av1 jobs use a Tokio watch channel, `CancellationHandle`, and `JobHandle<T>`
* remux jobs use a standard MPSC channel, `RemuxCancellationHandle`, and `RemuxHandle`
* synchronous supervised processes use `ProcessCancellation` over `Arc<AtomicBool>`
* vendor installation uses a resettable shared `Arc<AtomicBool>` alongside a separately owned thread handle

The coordinator adapts the ab-av1 and remux handles through `ActiveJobCancellation` and maintains two polling loops with the same result, timeout, telemetry, and cancellation behavior. Each implementation must independently get pre-registration cancellation, channel disconnection, cancel-on-drop, terminal cleanup, worker termination, and subprocess settlement right.

The ab-av1 library lifecycle is a related but separate external-boundary decision selected by [ADR-021](021-drive-ab-av1-through-an-owned-operation.md), with evidence in [issue #104](https://github.com/Loufe/AB-AV1-GUI/issues/104) and [`docs/design/ab-av1-library-lifecycle-research.md`](../design/ab-av1-library-lifecycle-research.md). ADR-021 gives ab-av1 a generic shutdown future and keeps its owned settlement inside that finite operation; this record decides how CRFty produces that signal, owns the executor worker, and publishes the terminal report.

Four guarantees must remain distinct:

1. **Cancellation requested** is sticky intent that a worker must observe.
2. **Operation completed** means operation-specific cleanup finished and a terminal domain report was produced.
3. **Worker terminated** means the thread or task body and its destructors exited and its unique join authority observed termination.
4. **Subprocess containment settled** means the process group, cgroup, or Windows Job Object was terminated and reaped according to the platform contract.

Conflating these events can publish a false terminal outcome, detach a worker, leak descendants, or promote partial telemetry into durable analysis or conversion evidence. A cancellation primitive alone cannot prove cleanup, termination, or process settlement.

This design is an application of **structured job supervision**: cooperative cancellation, explicit completion, owned worker lifetimes, RAII cleanup, and process-tree containment. It borrows the parent/child lifetime discipline of structured concurrency without claiming that a Tokio-only task scope can supervise CRFty's standard threads and external processes.

## Decision Drivers

* A cancellation requested before worker registration must not be lost.
* Cancellation must be idempotent, sticky, cloneable internally, and observable from synchronous threads and Tokio tasks.
* Every job must receive a fresh cancellation identity that cannot be cleared or reused for later work.
* Cancellation authority and cancellation observation should be different capabilities in the public and worker-facing APIs.
* Exactly one component may publish the terminal report, and worker code must not be able to publish it before operation cleanup returns.
* Dropping an unfinished public job handle must request cancellation.
* A timeout must retain ownership and must not disarm cancel-on-drop.
* Result-channel disconnection must remain an infrastructure failure, not success or cancellation.
* Telemetry is latest-value, best-effort, and non-authoritative until a valid terminal report commits complete evidence.
* Cancellation must remain responsive while progress or telemetry is busy.
* Shutdown must account for every worker and retain the unique right to join every joinable thread or task.
* Process-tree containment and operation-specific cleanup must remain intact.
* The compiler should prevent cloned terminal publishers, accidentally cloned join authority, new jobs after shutdown begins, and direct worker access to cancellation authority where practical.
* The shared abstraction must not pre-build graceful/restart policy that the engine does not currently expose.

## Considered Options

* Keep the four cancellation mechanisms and two handles.
* Introduce a local cancellation trait over the existing watch, MPSC, and atomic implementations.
* Compose Tokio's maintained cancellation, oneshot, watch, and task-tracking primitives behind a small CRFty-specific job API while retaining operation-specific workers.
* Replace `command-group` with the more composable `process-wrap` while retaining CRFty's process supervision and reporting.
* Replace most of CRFty's process layer with the opinionated `processkit` runner and containment model.
* Adopt an application-wide Tokio subsystem or structured-task crate and move the coordinator and all workers into its lifecycle model.

## Decision Outcome

Chosen option: **compose `CancellationToken`, `oneshot`, `watch`, and `TaskTracker` behind a small capability-oriented CRFty job API while retaining operation-specific workers**, because these maintained primitives implement the generic synchronization contracts while CRFty retains only its domain reports, cleanup sequencing, worker ownership, and process policy.

The primitive composition is the selected direction. The process implementation and final join-ownership boundary remain open implementation decisions recorded below.

### Common Contract

Each started job creates a fresh `tokio_util::sync::CancellationToken`. The raw token remains private. A controller-facing cancellation capability can request cancellation, while a worker-facing context exposes only observation through `is_cancelled()` and `cancelled()`. This prevents ordinary worker code from acquiring broader cancellation authority merely because `CancellationToken` itself is cloneable and bidirectional.

The common `JobHandle<Report, Telemetry>` is non-`Clone`, marked `#[must_use]`, and owns:

* a controller-facing cancellation capability
* a `tokio::sync::oneshot::Receiver<Report>` for the exactly-once terminal report
* a `tokio::sync::watch::Receiver<Option<Telemetry>>` for the latest telemetry snapshot
* cancel-on-drop state implemented with the token's `DropGuard`

Tokio's oneshot receiver supports asynchronous await, synchronous `try_recv()`, and synchronous `blocking_recv()`, so one channel works for the current coordinator and any future async caller. Its sender is not cloneable, which gives the terminal report one producer by construction. Dropping the sender without sending remains a disconnected-channel infrastructure failure.

Tokio's watch channel directly models latest-value telemetry and works across synchronous and asynchronous producers. The public handle returns a cloned snapshot and never exposes a long-lived `watch::Ref`, because a retained borrow holds the channel read lock and can block producers. Replacing the shared mutex removes telemetry poisoning from the common contract; losing telemetry remains non-fatal and never manufactures a terminal outcome.

The worker does not receive the terminal oneshot sender. A private spawn wrapper receives a closure shaped conceptually as `FnOnce(JobContext<Telemetry>) -> Report`, invokes it, and sends the returned report only after the closure returns. Operation-specific cleanup therefore precedes report publication by construction. A worker panic drops the sender, which the handle classifies as an infrastructure failure rather than cancellation or success.

Receiving a valid terminal report disarms cancel-on-drop. A timeout does not. Bounded polling borrows `&mut self` so it cannot consume and silently discard the live handle. A final blocking or asynchronous wait consumes `self`.

Synchronous workers call `is_cancelled()` at their existing interruption points. Async workers select on `cancelled()`, with cancellation ahead of progress/update branches when a continuously ready stream could otherwise delay shutdown. The current ab-av1 operation must not be wrapped wholesale in `CancellationToken::run_until_cancelled`, because that helper drops the wrapped future and is safe only when the future is cancellation-safe. The current fork provisionally uses explicit `cancel_job()` and `finish_job()` calls, but the durable CRFty requirement is mechanism-neutral: the adapter must await the selected ab-av1 terminal lifecycle and publish no domain report before its task, process, and temporary-state settlement contract returns.

`tokio_util::task::TaskTracker` provides the shared worker-lifetime ledger. Tokio tasks are spawned or wrapped through the tracker. A non-cloneable local wrapper around `TaskTrackerToken` can be moved into a standard thread so a panic or normal return removes that worker from the ledger when the token drops. This ledger proves that registered worker bodies released their permits; it does not replace retaining standard `JoinHandle`s or observing Tokio `JoinError`s.

The supervisor API should use private construction and, if practical, typestate such as `Supervisor<Accepting>` and `Supervisor<Closing>`. `TaskTracker::close()` allows its wait future to resolve but does not prevent new tasks from being added, so CRFty must prevent post-shutdown registration through its own API rather than assume the tracker enforces that policy.

`ActiveCancellation` stores the active job cancellation capability directly. Its existing run-scoped registration gate remains responsible for a force-stop that races before registration. The adapter-only `ActiveJobCancellation` enum is removed. `CancelMode` is also removed while force is the only mode. If graceful and forced stopping become separate product policies, they must be introduced as monotonic escalation rather than ordinary messages that can regress or be reordered.

The coordinator uses one generic monitoring loop for terminal report polling and telemetry snapshots. Operation-specific closures translate telemetry into session progress and rates. Cancellation does not itself mean `Stopped`, `Cancelled`, or `Idle`; the operation closure returns the validated domain report after its cleanup.

### Compiler and Static-Analysis Enforcement

Rust's ownership model can enforce that the terminal sender and join authority are not duplicated, but Rust cannot prove that a cooperative worker checks its token or that an external descendant really exited. The common API therefore uses the compiler for ownership invariants and contract tests for temporal and operating-system behavior.

The enforcement is:

* keep `JobHandle`, join authority, worker permits, and terminal senders non-`Clone`
* mark `JobHandle` and important completion results `#[must_use]`, and deny `unused_must_use` in engine code
* expose cancellation source and cancellation observation as separate wrapper types
* keep channel senders and constructors private so only the spawn wrapper can publish a terminal report
* make final wait consume the handle while timeout polling borrows it
* encode accepting-versus-closing supervisor state in the API if it remains understandable at call sites
* retain ordinary `JoinHandle`s even when `TaskTracker` accounts for the worker body
* use deterministic barrier-based race tests for the composed protocol and Loom only if CRFty introduces its own low-level atomic state machine

`#[must_use]` is a lint and can be explicitly suppressed, so it is not linear typing. The design relies on Rust's affine ownership, private constructors, capability wrappers, RAII, and tests together rather than claiming the compiler can enforce cooperative cancellation.

### Process Containment Direction

The cancellation token does not replace process containment. Token observation must trigger a process owner that kills the correct containment unit, drains or closes pipes without deadlock, waits or reaps the leader and descendants, joins reader threads, and only then returns the domain report.

Three process-layer choices remain viable:

* Keep `ContainedChild` over `command-group`, with focused fixes and contract tests.
* Migrate to `process-wrap`, which describes itself as the more flexible successor to `command-group` and composes standard-library and Tokio commands with process groups, sessions, Windows Job Objects, and Tokio kill-on-drop.
* Spike `processkit`, which provides cancellation-aware streaming and run-to-completion APIs, Windows Job Objects, Linux cgroup v2 with process-group fallback, BSD process reapers or process groups, kill-on-drop, and an observable containment mechanism.

`processkit` is the only researched option that could materially change the architecture rather than merely replace a wrapper. Linux cgroup v2 can contain descendants that escape a POSIX process group by calling `setsid()`, but the crate remains async-first and falls back when a suitable cgroup is unavailable. Adoption requires proving that it preserves ab-av1 and FFmpeg streaming parsers, Windows packaging and Job Object behavior, Linux fallback behavior, cancellation precedence, and exact kill/wait/reap ordering.

`process-wrap` is the conservative alternative if CRFty wants a supported successor to `command-group` without adopting a full process runner. It does not itself provide the terminal-report or worker-supervision contract.

The process choice in this record concerns CRFty-owned direct processes. ADR-021 separately places internal process ownership inside ab-av1 so its library operation cannot detach sample producers or leave sample-copy FFmpeg outside terminal settlement.

### Counterexamples and Failure History

* **Detached worker after apparently successful cancellation:** Dropping either a standard-library or Tokio `JoinHandle` detaches its worker. A handle that only cancels on drop can therefore leave a remux, vendor, or runtime worker executing after its owner has disappeared. The supervisor must retain unique join authority and shutdown must separately prove worker termination.
* **Cleanup skipped by cancellation race:** `CancellationToken::run_until_cancelled` drops the wrapped future when cancellation wins and is biased toward future completion on a simultaneous ready poll. Wrapping the current ab-av1 operation wholesale could skip its provisional explicit finalization or classify a completion/cancellation tie according to helper polling order rather than CRFty policy.
* **Detached ab-av1 sample producer:** Upstream `sample_encode::run` uses `spawn_local` for sample production, and dropping its ordinary `JoinHandle` detaches the task. That producer invokes sample-copy FFmpeg through an unmanaged `.output()` path, so draining the prototype's global child registry can race with later process creation. ADR-021 requires the upstream lifecycle to stop producers before terminating containment units, reaping direct children, and completing the strongest descendant settlement the platform can prove.
* **Blocking task that ignores abort:** Tokio documents that an already running `spawn_blocking` task cannot be aborted and may keep runtime shutdown waiting indefinitely. Moving the vendor downloader into `spawn_blocking` would change scheduling without making its blocking read interruptible.
* **Stalled vendor download:** A worker blocked inside a synchronous Reqwest read cannot observe the token until the I/O call returns or its timeout fires. Cancellation latency is therefore bounded by the configured network timeout, not by token wakeup latency.
* **Premature terminal report:** If operation code owns the report sender, it can send `Cancelled` before subprocess settlement or finalization and then fail or panic during cleanup. Returning a report to a private outer sender prevents this ordering error by construction.
* **False shutdown completion:** `TaskTracker::close()` does not forbid new tracked tasks. Exposing the raw tracker could allow registration after shutdown appears empty, so the supervisor must close registration in its own private state machine or typestate API.
* **Progress-starved cancellation:** Watchexec encountered filesystem event pressure that made Ctrl-C and SIGTERM ineffective. A monitor that treats progress and cancellation with equal unbounded traffic can reproduce the same class of starvation; cancellation needs explicit priority.
* **Escaped Unix descendant:** cargo-nextest documented nested tools creating their own process groups and surviving the parent's group-oriented signal path. A descendant that calls `setsid()` can escape POSIX process-group containment; only a stronger mechanism such as a delegated Linux cgroup covers that case.
* **Windows pipe deadlock:** `command-group` documents that synchronous `wait_with_output` reads stdout before stderr on Windows and can block when both are piped. CRFty's synchronous output path must be audited independently of cancellation unification.
* **Reset ABA:** Clearing a shared vendor atomic for a later installation can make a delayed observer of the earlier job see a non-cancelled state again. Fresh one-shot tokens remove this resettable identity problem.
* **Lost owner on timeout:** A polling API that consumes the handle on timeout can drop the only cancel-on-drop guard or join authority. Timeout polling must retain the handle.
* **Channel closure mistaken for cancellation:** A panic before report publication drops the sender. Treating disconnection as ordinary cancellation would hide an infrastructure failure and could commit incomplete evidence.

### Open Implementation Decisions

1. **Join ownership:** Prefer one engine supervisor as the exclusive owner of all joinable worker handles because ab-av1 currently uses a shared runtime thread, but decide whether remux and vendor jobs should instead carry per-job join capabilities. The public `JobHandle` must not claim to prove thread termination unless it actually joins.
2. **Terminal report meaning:** The selected meaning is “operation cleanup and process settlement completed,” while the worker ledger and join owner separately prove executor/thread termination. Confirm that every domain consumer can tolerate this small distinction.
3. **Completion-versus-cancellation precedence:** Define a race table for simultaneous completion, cancellation, timeout, channel disconnection, and cleanup failure. Do not inherit the answer accidentally from `select!` branch order or `run_until_cancelled` fairness.
4. **Cleanup failure after cancellation:** Decide whether this becomes a cancellation report carrying cleanup evidence, a common infrastructure error, or a driver-fatal error. A plain `Cancelled` outcome must not hide a process that could not be terminated or reaped.
5. **Shutdown timeout policy:** Decide whether an unresponsive vendor thread blocks shutdown, is deliberately abandoned until process exit, or escalates to application termination. No cancellation crate can safely kill an arbitrary Rust thread.
6. **Cancellation hierarchy:** Prefer independent fresh per-job tokens for now. Decide whether application shutdown later needs a parent token with child job tokens, noting that cancellation across a token tree is not observed atomically while `cancel()` is in progress.
7. **Process library:** Run a focused `processkit` spike and compare it with `process-wrap` plus the existing `ContainedChild` before accepting a process-layer choice.
8. **Cancellation latency requirement:** Define the acceptable bound for process polling and vendor network reads, then test it with a deliberately stalled server and real subprocess fixtures.
9. **Sync versus async process I/O:** Decide whether fixing concurrent pipe drainage and responsive cancellation justifies moving process execution to Tokio or whether dedicated synchronous reader threads remain the smaller design.
10. **Typestate scope:** Decide whether `Supervisor<Accepting>` to `Supervisor<Closing>` materially clarifies coordinator code; otherwise enforce the same transition with a private runtime state and focused tests.

### Consequences

* Good: CRFty delegates cancellation, exactly-once delivery, latest-value telemetry, and worker accounting to maintained primitives instead of reimplementing them.
* Good: Private capability types and non-cloneable resources let the compiler prevent several invalid lifecycle operations.
* Good: Pre-registration cancellation, drop cancellation, timeouts, channel failure, and worker shutdown have one testable vocabulary.
* Good: Vendor cancellation can no longer be cleared by resetting shared state for a later job.
* Good: Sync and async workers observe the same sticky job identity.
* Good: Partial telemetry cannot masquerade as completion or durable evidence.
* Good: Process containment remains a separately testable authority and can evolve without changing job reports.
* Bad: `crfty-engine` takes direct dependencies on Tokio synchronization and `tokio-util` runtime features.
* Bad: The shared handle remains generic over operation-specific report and telemetry types.
* Bad: A local composition layer is still required because no established crate covers CRFty's mix of standard threads, Tokio tasks, domain reports, blocking HTTP, and external process trees.
* Bad: Cooperative blocking operations remain bounded by their polling or I/O timeout before they can observe the token.
* Bad: Stronger process containment may require an async process-layer migration and platform-specific fallback behavior.
* Bad: Compile-time ownership cannot prove cancellation responsiveness or operating-system cleanup; adversarial integration tests remain necessary.

## Verification Strategy

The common job-contract tests must cover pre-cancel, cloned and idempotent cancel, successful completion disarming drop, timeout retaining ownership and drop cancellation, handle drop, sender disconnection, worker panic, final telemetry snapshot, cancellation while telemetry is continuously ready, force-before-registration, fresh vendor tokens, and new-job rejection after shutdown begins.

Implementation validation must cover remux cancellation followed by parser and reader cleanup, vendor cancellation during a stalled response body, a child that spawns a grandchild, a Unix descendant that calls `setsid()`, Windows Job Object settlement, both stdout and stderr filling concurrently, and driver shutdown proving every registered worker is joined or explicitly classified under the chosen timeout policy. ab-av1-specific real-process lifecycle tests are tracked separately in [issue #105](https://github.com/Loufe/AB-AV1-GUI/issues/105) and are not research completion criteria for this record.

Use ordinary barriers and controllable fake workers for protocol races. Loom is appropriate only if CRFty adds a custom atomic state machine; it cannot directly model real network calls or operating-system processes.

## More Information

See issue #85 and ADR-015, which retains the rule that statistics and prediction provenance derive from validated facts; cancellation telemetry is not such a fact. ADR-021 selects the upstream ab-av1 operation boundary, its research is recorded in [`docs/design/ab-av1-library-lifecycle-research.md`](../design/ab-av1-library-lifecycle-research.md) and [issue #104](https://github.com/Loufe/AB-AV1-GUI/issues/104), and its implementation-validation contract is tracked separately in [issue #105](https://github.com/Loufe/AB-AV1-GUI/issues/105).

Implementation locations at the time this decision was recorded:

* `crates/crfty-engine/src/ab_av1/runtime.rs`
* `crates/crfty-engine/src/remux.rs`
* `crates/crfty-engine/src/process_supervisor.rs`
* `crates/crfty-engine/src/coordinator/supervision.rs`
* `crates/crfty-engine/src/coordinator/vendor_task.rs`
* `crates/crfty-engine/src/process.rs`
* `crates/crfty-engine/src/vendor/download.rs`

Primary implementation and failure-history references:

* Tokio's [`CancellationToken` documentation](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html) defines sticky sync/async observation, child-token behavior, `DropGuard`, simultaneous completion fairness, and the cancellation-safety limitation of `run_until_cancelled`.
* Tokio's [`oneshot::Receiver` documentation](https://docs.rs/tokio/latest/tokio/sync/oneshot/struct.Receiver.html) documents one-value delivery, sender-drop errors, cancel-safe awaiting, `try_recv()`, and `blocking_recv()`.
* Tokio's [`watch` documentation](https://docs.rs/tokio/latest/tokio/sync/watch/) defines a thread-safe channel that retains only the latest value and warns that long-lived borrows can block producers.
* Tokio's [`TaskTracker` documentation](https://docs.rs/tokio-util/latest/tokio_util/task/struct.TaskTracker.html) explicitly separates asking tasks to cancel from waiting until tracked tasks and destructors have exited; [`TaskTrackerToken`](https://docs.rs/tokio-util/latest/tokio_util/task/task_tracker/struct.TaskTrackerToken.html) is a transferable lifetime permit removed on drop.
* Tokio's [`JoinHandle` documentation](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html) and the standard library's [`JoinHandle` documentation](https://doc.rust-lang.org/std/thread/struct.JoinHandle.html) state that dropping a join handle detaches the task or thread.
* Tokio's [`spawn_blocking` documentation](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html) states that already-running blocking tasks cannot be aborted and can delay runtime shutdown indefinitely.
* The Rust Reference's [`must_use` documentation](https://doc.rust-lang.org/reference/attributes/diagnostics.html#the-must-use-attribute) defines the compiler lint used to flag discarded lifecycle handles and results.
* The [`tokio-util` changelog](https://github.com/tokio-rs/tokio/blob/master/tokio-util/CHANGELOG.md) records a waker-condition fix, a complete token rewrite to fix a memory leak, and the later addition of `TaskTracker`, demonstrating that a reusable cancellation primitive is not a trivial watch or atomic wrapper.
* Reqwest's blocking [`ClientBuilder::timeout` documentation](https://docs.rs/reqwest/latest/reqwest/blocking/struct.ClientBuilder.html#method.timeout) states that the configured timeout covers connect, read, and write operations, which bounds but does not instantly interrupt synchronous vendor I/O.
* Watchexec issue [#241](https://github.com/watchexec/watchexec/issues/241) reports filesystem event pressure making Ctrl-C and SIGTERM ineffective. Its current [`watchexec-supervisor` design](https://docs.rs/watchexec-supervisor/latest/watchexec_supervisor/) uses a dedicated `Job`, ordered controls, and three control priorities so stop and restart work is not starved by ordinary traffic.
* Watchexec's [supervision redesign history](https://docs.rs/crate/watchexec/latest/source/CHANGELOG.md) explains why nested `Outcome` combinators for stop and restart were replaced by explicit `Job` operations and records fixes applying kill-on-drop to grouped commands.
* cargo-nextest's [signal-handling design](https://nexte.st/docs/design/architecture/signal-handling/) separates signal broadcast, a grace period, forced termination, Unix process-group forwarding, and Windows Job Objects. Discussion [#2482](https://github.com/nextest-rs/nextest/discussions/2482) documents how immediate fail-fast exposed leaked subprocesses when nested tools created their own process groups.
* Cargo's [job-management module](https://doc.rust-lang.org/stable/nightly-rustc/cargo/util/job/index.html) explains why Windows Job Objects are required to make parent cancellation terminate a complete descendant tree.
* [`command-group::GroupChild::wait_with_output`](https://docs.rs/command-group/5.0.1/command_group/struct.GroupChild.html#method.wait_with_output) documents its Windows risk when both stdout and stderr are piped.
* The [`process-wrap` documentation](https://docs.rs/process-wrap/latest/process_wrap/) describes it as a composable successor to `command-group` with standard-library and Tokio frontends, process groups, sessions, Windows Job Objects, and Tokio kill-on-drop.
* The [`processkit` documentation](https://docs.rs/processkit/latest/processkit/) describes cancellation-aware streaming and capture, Windows Job Objects, Linux cgroup v2 with process-group fallback, platform caveats, and observable containment mechanisms.
* Loom's [`model` documentation](https://docs.rs/loom/latest/loom/) explains its deterministic exploration of synchronization interleavings and its requirement that modeled nondeterminism use Loom types.
