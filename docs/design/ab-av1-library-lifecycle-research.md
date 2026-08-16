# ab-av1 library lifecycle and cancellation research

Status: living research note; not an accepted design or implementation specification  
Upstream coordination: [alexheretic/ab-av1#371](https://github.com/alexheretic/ab-av1/issues/371)  
Tracking issue: [#104](https://github.com/Loufe/AB-AV1-GUI/issues/104)  
Implementation validation: [#105](https://github.com/Loufe/AB-AV1-GUI/issues/105)  
Related CRFty decisions: [ADR-003](adr/003-embed-a-pinned-ab-av1-adapter.md), [ADR-018](adr/018-unify-job-cancellation-and-completion.md)  
Last updated: 2026-08-16

## Purpose and boundary

This note records an upstream-first investigation of the lifecycle boundary ab-av1 would need to support reliable in-process use by CRFty. It separates confirmed resource-ownership requirements from the still-undecided public Rust API and from CRFty's private cancellation and worker-supervision design.

The source review used a fresh clone of alexheretic/ab-av1 at v0.11.6 commit [`629cfaa`](https://github.com/alexheretic/ab-av1/tree/629cfaa) without adding or consulting the CRFty fork while deriving the findings. The existing `Loufe/ab-av1` prototype was compared only afterward and is treated as exploratory evidence rather than the source of the design.

This is a research artifact. It does not authorize an upstream PR, select a process-management crate, settle the public progress API, or define implementation-test work. Implementation and real-process validation belong in separately tracked issues.

## Current conclusions

1. CRFty's use of `tokio_util::sync::CancellationToken` is an application-internal choice and does not require ab-av1 to expose or depend publicly on that type.
2. ab-av1 needs an awaited terminal lifecycle for embedded operations. Returning `Cancelled` or another terminal outcome must mean internal producers have stopped, owned process trees have terminated and been reaped, and explicit temporary cleanup has been attempted.
3. Drop behavior is a last-resort safety net, not proof of terminal cleanup. Rust has no asynchronous `Drop`, Tokio process reaping after kill-on-drop is best effort, and fallible temporary cleanup cannot report errors from a destructor.
4. Every task and subprocess started by an operation needs an accountable owner. Dropping an ordinary Tokio `JoinHandle` detaches its task, and dropping a Tokio child does not kill it unless kill-on-drop is enabled.
5. Cancellation must first prevent internal producers from starting more subprocesses, then terminate active containment units, await internal task termination and child reaping, and finally clean temporary state. Draining a global child registry before detached producers stop creates a spawn-after-cleanup race.
6. Process and temporary ownership should be operation-local rather than process-global. This prevents one operation from finalizing another operation's resources and avoids forcing a permanent global one-job policy merely because the current CLI has global registries.
7. ab-av1 should accept either a mechanism-neutral cancellation signal or an ab-av1-owned operation control API. Requiring `CancellationToken` in the public API would make `tokio-util` part of ab-av1's public dependency and semver surface without being necessary for CRFty integration.
8. Two public API families remain credible: a high-level operation that accepts a generic shutdown future and owns cleanup internally, or an explicit operation handle with an awaited cancellation method. The research does not yet select between them.
9. The current CRFty fork does not prove the complete lifecycle contract. Its explicit global finalization and process containment work are useful experiments, but the upstream sample producer can detach and the sample-copy FFmpeg path is not registered with that global finalizer.

## Terminology

Tokio defines cancellation safety in terms of whether dropping and recreating a future loses progress or data. CRFty also needs a different property: cancelling or dropping an ab-av1 operation must not leak tasks, processes, temporary ownership, or false terminal evidence. This note calls the latter **cancellation cleanup safety** or **resource-safe cancellation** to avoid claiming the stronger restart-safe property for an encode pipeline with intentional partial work.

**Cancellation requested** means sticky intent to stop. It does not prove that work stopped.

**Operation terminal** means the operation-specific shutdown path completed and returned an outcome after its cleanup contract.

**Task settlement** means every internally owned task stopped and its completion was observed rather than detached.

**Process settlement** means every owned process containment unit was terminated when required and its leader and descendants were reaped according to the platform contract.

**Drop fallback** means synchronous or best-effort protection used when a caller abandons the normal awaited lifecycle. It cannot report the same guarantees as an awaited terminal path.

## Upstream execution model at v0.11.6

### Package and CLI boundary

The upstream package is binary-only. [`Cargo.toml`](https://github.com/alexheretic/ab-av1/blob/629cfaa/Cargo.toml) has no library target, and [`src/main.rs`](https://github.com/alexheretic/ab-av1/blob/629cfaa/src/main.rs) declares command and implementation modules directly.

The CLI creates a current-thread Tokio runtime and `LocalSet`, constructs one command future, and selects between that future and `tokio::signal::ctrl_c()`. It then drops the `LocalSet`, waits for globally registered processes, and cleans globally registered temporary files. Signal handling, process cleanup, temporary cleanup, terminal rendering, logging initialization, and process exit are therefore application-root responsibilities rather than an existing reusable operation abstraction.

### Typed and terminal-coupled operation seams

[`crf_search::run`](https://github.com/alexheretic/ab-av1/blob/629cfaa/src/command/crf_search.rs) already returns a typed stream of search updates, while its CLI wrapper owns `indicatif` rendering and human or JSON output.

[`sample_encode::run`](https://github.com/alexheretic/ab-av1/blob/629cfaa/src/command/sample_encode.rs) also returns typed updates.

[`encode::run`](https://github.com/alexheretic/ab-av1/blob/629cfaa/src/command/encode.rs) accepts an `indicatif::ProgressBar` and prints terminal output directly, so a library boundary requires separating encode events and results from CLI presentation.

The Clap-derived command argument structures are CLI types. Publishing the complete `command` module would expose parsing and terminal concerns as a large semver API; a stable library should instead expose a deliberately selected request, event, result, and error surface.

### Internal task ownership

[`sample_encode::run`](https://github.com/alexheretic/ab-av1/blob/629cfaa/src/command/sample_encode.rs) starts the copy-sample producer with `tokio::task::spawn_local` and stores its ordinary `JoinHandle`. The handle is awaited only after the receiving loop ends normally.

Tokio documents that dropping a [`JoinHandle`](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html) detaches the task and allows it to continue in the background. Dropping the surrounding sample or search stream can therefore abandon the join authority while the producer continues preparing samples.

Tokio's [`JoinSet`](https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html) aborts its tasks on drop and provides `shutdown()` to abort and wait for termination. `tokio-util` also provides [`AbortOnDropHandle`](https://docs.rs/tokio-util/latest/tokio_util/task/struct.AbortOnDropHandle.html). Either may help implement owned internal work, but an aborting handle alone does not settle subprocesses unless the subprocess future is itself resource-safe.

### Subprocess ownership

Streamed encode, VMAF, and XPSNR commands call `kill_on_drop(true)`, but [`process::child`](https://github.com/alexheretic/ab-av1/blob/629cfaa/src/process/child.rs) registers their streams in a process-global collection only when an `AddOnDropChunkStream` is dropped. The global `wait()` function owns Ctrl-C behavior and may return without a deterministic terminate-and-reap result.

[`sample::copy`](https://github.com/alexheretic/ab-av1/blob/629cfaa/src/sample.rs) invokes FFmpeg through `tokio::process::Command::output()` without kill-on-drop or registration in the global running-process collection. If the detached sample producer is cancelled while awaiting this command, neither the task nor the FFmpeg child is covered by the current global finalization convention.

[`ffprobe::probe`](https://github.com/alexheretic/ab-av1/blob/629cfaa/src/ffprobe.rs) calls the synchronous `ffprobe` crate API and hardcodes PATH resolution. It can block the current-thread runtime while probing and cannot participate in prompt asynchronous cancellation without a different execution path or a documented latency bound.

Tokio documents that a child process continues after its handle is dropped unless kill-on-drop is enabled, and that kill-on-drop reaping is only best effort. When stronger guarantees are required, Tokio recommends explicitly awaiting `kill()` or `wait()`. See [`tokio::process`](https://docs.rs/tokio/latest/tokio/process/) and the [`Command::kill_on_drop` source documentation](https://github.com/tokio-rs/tokio/blob/master/tokio/src/process/mod.rs).

### Temporary ownership

[`temporary.rs`](https://github.com/alexheretic/ab-av1/blob/629cfaa/src/temporary.rs) stores all temporary paths in a process-global map and uses one process-scoped random subdirectory. Cleanup ignores removal errors.

The [`tempfile`](https://docs.rs/tempfile/latest/tempfile/) crate demonstrates the useful dual contract: RAII deletion on Drop is a fallback, while explicit [`TempDir::close`](https://docs.rs/tempfile/latest/tempfile/struct.TempDir.html#method.close) reports cleanup failure. An ab-av1 operation needs the same distinction whether implemented with `tempfile` or an internal equivalent.

### Tool selection and cache identity

FFmpeg and FFprobe are selected through process PATH, and the sample cache hashes a process-global lazily queried `SvtAv1EncApp --version`. A long-lived embedding that supplies managed FFmpeg and FFprobe binaries needs explicit tool configuration associated with the operation or engine rather than process-global PATH mutation or thread-local state.

Upstream issue [#350](https://github.com/alexheretic/ab-av1/issues/350) describes stale sample-cache results after the FFmpeg-embedded SVT version changes. A configurable toolchain and a correct cache identity are related because the executed toolchain, not an unrelated process-global executable lookup, must determine cache compatibility.

## Required upstream properties

The following requirements are independent of the final public API shape.

### Owned internal work

* An operation owns every producer, parser, cache task, and subprocess it starts.
* No ordinary `JoinHandle` may be discarded or detached as part of cancellation.
* Cancellation prevents further process creation before active process termination begins.
* An awaited terminal outcome observes internal task termination.

### Owned subprocesses

* Every asynchronous FFmpeg and FFprobe invocation goes through one managed process abstraction.
* Registration or ownership begins immediately after spawn, not when a stream is later dropped.
* Cancellation targets the entire containment unit, not only the immediate child.
* Termination is followed by an awaited reap.
* stdout and stderr are drained or closed without a pipe deadlock.
* Drop provides best-effort containment, while the awaited terminal path returns process-settlement failures.

### Owned temporary state

* Each operation has a unique temporary namespace and registry.
* Cleanup cannot consume resources belonging to another operation.
* Explicit cleanup reports failures.
* Drop cleanup is best effort and does not manufacture a successful terminal result.
* Keep semantics are explicit operation configuration rather than inferred by a process-global command enum.

### Stable library boundary

* Public requests are domain types rather than Clap parser structures.
* Public events and results do not require `indicatif`, terminal detection, stdout, or stderr.
* Public errors distinguish cancellation, operation failure, process-settlement failure, temporary-cleanup failure, and combined failures where callers need different policy.
* The CLI uses the same core operation boundary as library consumers so the library path does not become an untested second implementation.
* The public API does not require `tokio_util::sync::CancellationToken`; CRFty may still pass `CancellationToken::cancelled()` to a generic signal boundary or translate the token into an ab-av1 control operation.

## Candidate public API families

### Option A: high-level operation with a shutdown future

Conceptually:

```rust
let outcome = engine
    .crf_search(request, token.cancelled(), |event| report(event))
    .await?;
```

The operation owns the race between completion and shutdown, stops internal producers, settles processes and tasks, cleans temporary state, and only then returns `Completed` or `Cancelled`. The cancellation input can be any `Future<Output = ()>`, allowing the CLI to pass an OS-signal future and CRFty to pass its private token future.

This pattern is established by axum's [`with_graceful_shutdown`](https://docs.rs/axum/latest/axum/serve/struct.WithGracefulShutdown.html) and tonic's [`serve_with_shutdown`](https://docs.rs/tonic/latest/tonic/transport/server/struct.Server.html#method.serve_with_shutdown). Those server APIs reinforce the mechanism-neutral signal shape, although ab-av1 is a finite media pipeline rather than a server and still needs typed progress and partial-output policy.

Advantages:

* The normal API cannot forget a separate finalization call.
* CLI and library cancellation can share one operation root.
* ab-av1 owns completion-versus-cancellation precedence and cleanup ordering.
* No background task or remote-control actor is required merely to provide cancellation.

Costs and unresolved questions:

* Progress needs a callback, channel, `Sink`, or another mechanism that does not reintroduce detached background work.
* A synchronous callback cannot provide asynchronous backpressure.
* Dropping the entire returned future still bypasses awaited cleanup and relies on the Drop fallback.
* The signature and concrete future bounds become part of the public semver API.

### Option B: explicit operation handle

Conceptually:

```rust
let mut operation = engine.crf_search(request)?;
while let Some(event) = operation.next_event().await? {
    report(event);
}
let result = operation.wait().await?;
```

Cancellation from another task could use a separate control capability or a consuming terminal method:

```rust
operation.cancel().await?;
```

Watchexec's [`Job`](https://docs.rs/watchexec-supervisor/latest/watchexec_supervisor/job/struct.Job.html) is a mature example of a background supervisor with explicit ordered controls, stop, wait, restart, and signaling. [`tokio-process-tools`](https://docs.rs/tokio-process-tools/latest/tokio_process_tools/) likewise exposes explicit wait, cancel, abort, terminate, and kill operations and treats automatic Drop cleanup as a fallback.

Advantages:

* Remote cancellation and explicit terminal acknowledgement are natural.
* Progress can be streamed independently from the terminal result.
* The lifecycle can later support graceful escalation or richer controls.

Costs and unresolved questions:

* A background operation generally requires spawning and therefore binds behavior to an executor and task ownership model.
* The API must define what happens when the event receiver, control handle, or operation owner is dropped independently.
* It can overbuild service-manager semantics for a finite one-shot pipeline.
* Multiple handles can obscure which object owns the unique right to await terminal cleanup unless capabilities are deliberately separated.

### Option C: public stream plus manual finalization

Conceptually, this is the current CRFty prototype shape: consume a command stream, drop it, and call a separate global `finish_job()` or `cancel_job()` function.

Advantages:

* It follows the existing typed CRF-search stream seam.
* It requires a smaller initial change to internal command functions.

Costs:

* The compiler does not connect a stream to the correct global finalization call.
* A caller can forget cleanup or overlap operations that share global state.
* Async cleanup cannot be guaranteed by Drop.
* Detached producers can outlive stream drop.
* Global process and temporary registries force restrictions that are not inherent to ab-av1's algorithms.

This option should not be treated as the default upstream design. It remains useful as a temporary adapter boundary while a safer operation contract is developed.

## External project evidence

### Evidence reinforcing the requirements

* Tokio's [`select!`](https://docs.rs/tokio/latest/tokio/macro.select.html) documentation explains that losing branches are cancelled by dropping their futures and requires authors to reason about every `.await` boundary.
* Tokio's [graceful-shutdown guidance](https://tokio.rs/tokio/topics/shutdown) separates detecting shutdown, notifying work, and waiting for tasks to finish. This supports CRFty's private token and worker-ledger design but does not require ab-av1 to publish the token type.
* Tokio's [`JoinHandle`](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html) documentation confirms that dropping a handle detaches the task, directly matching the upstream sample-producer risk.
* Tokio's [`JoinSet`](https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html) owns its tasks, aborts them on Drop, and can abort and await them through `shutdown()`, providing a maintained building block for internal structured ownership.
* Cargo-mutants' [process-management design](https://github.com/sourcefrog/cargo-mutants/blob/main/DESIGN.md) documents why terminating only an immediate child leaks nested test processes and why Unix process groups plus signal forwarding are necessary.
* [`process-wrap`](https://docs.rs/process-wrap/latest/process_wrap/) provides composable Tokio and standard process wrappers, Unix groups or sessions, Windows Job Objects, and kill-on-drop. Its documentation describes it as the more flexible successor to `command-group`.
* [`command-group::AsyncGroupChild`](https://docs.rs/command-group/latest/command_group/struct.AsyncGroupChild.html) provides process-group kill and wait but warns that a cancelled asynchronous wait can leave its underlying blocking wait active, so process-wrapper selection needs an explicit wait-cancellation audit.
* [`tempfile::TempDir`](https://docs.rs/tempfile/latest/tempfile/struct.TempDir.html) distinguishes best-effort Drop cleanup from explicit fallible `close()`, matching the required dual cleanup contract.

### Evidence limiting or refuting a single obvious API

* axum and tonic demonstrate generic shutdown futures, but they do not provide ab-av1's progress-stream, output-commit, or temporary-file semantics.
* Watchexec demonstrates that an explicit `Job` can be the correct public abstraction when restart, remote control, priorities, or multiple signals matter. Its feature set is broader than ab-av1 presently needs.
* SQLx documents that Rust's lack of async Drop requires an explicit [`Pool::close`](https://docs.rs/sqlx/latest/sqlx/pool/struct.Pool.html#method.close) for deterministic cleanup even though Drop handles local fallback cleanup. This refutes relying on an operation future's destructor as the complete contract.
* `tokio-process-tools` provides a correctness-focused process lifecycle but its automatic asynchronous Drop termination requires a multithreaded Tokio runtime. CRFty currently embeds ab-av1 on a current-thread runtime, so it is evidence and a possible component rather than an assumed drop-in solution.
* [`async-scoped`](https://docs.rs/async-scoped/latest/async_scoped/) documents the caveats and even unsafe surface involved in guaranteeing non-`'static` async scopes across cancellation. A narrow ab-av1 design should prefer ordinary owned futures and tasks over importing a generalized scoped-concurrency model without need.

## Assessment of the existing CRFty prototype

The prototype lives in the `Loufe/ab-av1` `lib-target` and `crfty-library-api` branches and currently backs CRFty's pinned dependency.

Useful exploratory work includes:

* separating library and binary targets;
* emitting typed full-encode updates;
* adding explicit successful and cancelled finalization functions;
* returning process and temporary cleanup errors together;
* immediately registering streamed child processes;
* adding process-group or Windows Job Object containment through `command-group`.

The relevant experimental commits are [`e2c8a69`](https://github.com/Loufe/ab-av1/commit/e2c8a69), [`e8f692b`](https://github.com/Loufe/ab-av1/commit/e8f692b), [`a7205b9`](https://github.com/Loufe/ab-av1/commit/a7205b9), and [`8bde517`](https://github.com/Loufe/ab-av1/commit/8bde517).

The prototype is not an upstream-ready lifecycle design because:

* running processes and temporary files remain global;
* correct use depends on an unenforced one-job-at-a-time convention;
* tool paths are stored in a thread-local across asynchronous work;
* the spawned sample producer is not cancelled and joined when its containing stream is dropped;
* sample-copy FFmpeg uses an unmanaged `.output()` path and can outlive global finalization;
* callers must manually pair stream drop with the correct global finalization function;
* the public surface exposes command internals and Clap-derived types rather than a narrow semver API.

The prototype may supply implementation pieces after each is independently justified, but its overall shape must not be used as evidence that the lifecycle contract has already been proved.

## Relationship to CRFty decisions

[ADR-003](adr/003-embed-a-pinned-ab-av1-adapter.md) accepted an embedded, pinned ab-av1 adapter. Its core integration direction remains compatible with this research, but its statement that the real-process prototype demonstrated the complete lifecycle is too broad: the proof did not cover the detached sample producer and unmanaged sample-copy FFmpeg path. Because accepted ADRs are immutable, this correction belongs here and in a later related or superseding ADR rather than a rewrite of ADR-003.

[ADR-018](adr/018-unify-job-cancellation-and-completion.md) concerns CRFty's private cancellation, terminal-report, telemetry, and worker-ownership contract. It should require an awaited ab-av1 terminal lifecycle without permanently naming the prototype's `finish_job()` or `cancel_job()` functions. The choice between a generic cancellation future and an explicit ab-av1 operation handle is a separate external-boundary decision.

## Open decisions

1. Should the primary library API be a high-level operation with a generic shutdown future or an explicit operation handle with awaited cancellation?
2. Should progress use a synchronous observer, asynchronous `Sink`, caller-provided channel, or operation-owned event stream?
3. Must multiple operations be supported concurrently, explicitly rejected, or left unspecified in the initial library contract?
4. Should process containment use `process-wrap`, `command-group` with focused fixes, another maintained process runner, or a narrow internal wrapper over platform APIs?
5. What graceful signal, grace interval, and forceful escalation policy should apply to FFmpeg on Unix and Windows?
6. How should synchronous FFprobe and cache work participate in cancellation without binding the library to a caller-owned runtime configuration?
7. What terminal result represents cancellation followed by a cleanup or process-settlement failure?
8. What wins when operation completion and cancellation become ready in the same poll?
9. Which request, event, result, and error types are stable enough for a regular semver library API?
10. Should the initial library API document current-thread `LocalSet` requirements or remove `spawn_local` and support ordinary Tokio runtimes before stabilization?

## Documentation and issue actions

* Keep CRFty's private supervision decision in ADR-018 and issue #85.
* Track the upstream ab-av1 operation boundary in [issue #104](https://github.com/Loufe/AB-AV1-GUI/issues/104), linked to upstream issue #371.
* Track implementation and real-process contract tests in [issue #105](https://github.com/Loufe/AB-AV1-GUI/issues/105) rather than treating them as research deliverables.
* Update upstream issue #371 with the confirmed hazards, mechanism-neutral requirements, both credible API families, and an explicit statement that no public `CancellationToken` dependency is requested.
* Write a new ADR only after the public lifecycle and process-ownership decisions are made. Relate it to ADR-003 and ADR-018, and supersede ADR-003 only if the fundamental embedded-adapter decision changes.

## Primary sources

* [ab-av1 v0.11.6 source at `629cfaa`](https://github.com/alexheretic/ab-av1/tree/629cfaa)
* [ab-av1 issue #371: library interface discussion](https://github.com/alexheretic/ab-av1/issues/371)
* [Tokio graceful shutdown](https://tokio.rs/tokio/topics/shutdown)
* [Tokio cancellation safety in `select!`](https://docs.rs/tokio/latest/tokio/macro.select.html#cancellation-safety)
* [Tokio `JoinHandle`](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html)
* [Tokio `JoinSet`](https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html)
* [Tokio process cancellation and reaping](https://docs.rs/tokio/latest/tokio/process/)
* [Cargo public dependencies](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html)
* [axum graceful shutdown](https://docs.rs/axum/latest/axum/serve/struct.WithGracefulShutdown.html)
* [tonic shutdown future](https://docs.rs/tonic/latest/tonic/transport/server/struct.Server.html#method.serve_with_shutdown)
* [SQLx explicit asynchronous close](https://docs.rs/sqlx/latest/sqlx/pool/struct.Pool.html#method.close)
* [Watchexec process supervisor](https://docs.rs/watchexec-supervisor/latest/watchexec_supervisor/)
* [cargo-mutants process-tree design](https://github.com/sourcefrog/cargo-mutants/blob/main/DESIGN.md)
* [`process-wrap`](https://docs.rs/process-wrap/latest/process_wrap/)
* [`command-group`](https://docs.rs/command-group/latest/command_group/struct.AsyncGroupChild.html)
* [`tokio-process-tools`](https://docs.rs/tokio-process-tools/latest/tokio_process_tools/)
* [`tempfile`](https://docs.rs/tempfile/latest/tempfile/)
