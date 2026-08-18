# ab-av1 library lifecycle and cancellation research

Status: completed research basis for proposed ADR-021  
Upstream coordination: [alexheretic/ab-av1#371](https://github.com/alexheretic/ab-av1/issues/371)  
Tracking issue: [#104](https://github.com/Loufe/AB-AV1-GUI/issues/104)  
Implementation validation: [#105](https://github.com/Loufe/AB-AV1-GUI/issues/105)  
Related CRFty decisions: [ADR-003](../adr/003-embed-a-pinned-ab-av1-adapter.md), [ADR-018](../adr/018-unify-job-cancellation-and-completion.md), [ADR-021](../adr/021-drive-ab-av1-through-an-owned-operation.md)

## Purpose and boundary

This note records an upstream-first investigation of the lifecycle boundary ab-av1 would need to support reliable in-process use by CRFty. It separates the selected public operation shape and confirmed resource-ownership requirements from CRFty's private cancellation and worker-supervision design.

The source review used a fresh clone of alexheretic/ab-av1 at v0.11.6 commit [`629cfaa`](https://github.com/alexheretic/ab-av1/tree/629cfaa) without adding or consulting the CRFty fork while deriving the findings. The existing `Loufe/ab-av1` prototype was compared only afterward and is treated as exploratory evidence rather than the source of the design.

This is a research artifact, not an implementation specification or authority to open an upstream PR. It recommends a public lifecycle, progress boundary, and process-management direction; proposed ADR-021 owns the CRFty decision, while implementation and real-process validation belong in separately tracked issues.

## Current conclusions

1. CRFty's use of `tokio_util::sync::CancellationToken` is an application-internal choice and does not require ab-av1 to expose or depend publicly on that type.
2. ab-av1 needs an awaited terminal lifecycle for embedded operations. Returning `Cancelled` or another terminal outcome must mean internal producers have stopped, every owned process leader has been reaped, the selected platform containment unit has received and acknowledged its strongest available termination operation, and explicit temporary cleanup has been attempted. POSIX cannot let an ordinary parent reap arbitrary grandchildren, so descendant reaping must not be promised beyond the active platform mechanism.
3. Drop behavior is a last-resort safety net, not proof of terminal cleanup. Rust has no asynchronous `Drop`, Tokio process reaping after kill-on-drop is best effort, and fallible temporary cleanup cannot report errors from a destructor.
4. Every task and subprocess started by an operation needs an accountable owner. Dropping an ordinary Tokio `JoinHandle` detaches its task, and dropping a Tokio child does not kill it unless kill-on-drop is enabled.
5. Cancellation must first prevent internal producers from starting more subprocesses, then terminate active containment units, await internal task termination and child reaping, and finally clean temporary state. Draining a global child registry before detached producers stop creates a spawn-after-cleanup race.
6. Process and temporary ownership should be operation-local rather than process-global. This prevents one operation from finalizing another operation's resources and avoids forcing a permanent global one-job policy merely because the current CLI has global registries.
7. ab-av1 should accept either a mechanism-neutral cancellation signal or an ab-av1-owned operation control API. Requiring `CancellationToken` in the public API would make `tokio-util` part of ab-av1's public dependency and semver surface without being necessary for CRFty integration.
8. The recommended public API is a caller-driven high-level operation that accepts a generic shutdown future, emits typed progress through a synchronous observer, and returns only after settlement. An explicit background operation handle is unnecessary for ab-av1's finite pipeline; CRFty may create its own handle by spawning the operation inside its private runtime.
9. The current CRFty fork does not prove the complete lifecycle contract. Its explicit global finalization and process containment work are useful experiments, but the upstream sample producer can detach and the sample-copy FFmpeg path is not registered with that global finalizer.
10. `process-wrap` is the preferred containment building block, not a complete lifecycle abstraction. ab-av1 must wrap it in an operation-owned `ManagedChild` whose awaited path force-kills and waits for the whole containment unit and whose Drop fallback synchronously starts whole-unit termination.

## Terminology

Tokio defines cancellation safety in terms of whether dropping and recreating a future loses progress or data. CRFty also needs a different property: cancelling or dropping an ab-av1 operation must not leak tasks, processes, temporary ownership, or false terminal evidence. This note calls the latter **cancellation cleanup safety** or **resource-safe cancellation** to avoid claiming the stronger restart-safe property for an encode pipeline with intentional partial work.

**Cancellation requested** means sticky intent to stop. It does not prove that work stopped.

**Operation terminal** means the operation-specific shutdown path completed and returned an outcome after its cleanup contract.

**Task settlement** means every internally owned task stopped and its completion was observed rather than detached.

**Process settlement** means every owned process containment unit received the required termination operation, every direct child leader was awaited and reaped, and stronger platform-specific evidence was observed when available. A Windows Job Object can report that its active-process count reached zero, and a delegated Linux cgroup can report an empty subtree; a POSIX process group can broadcast an uncatchable signal but cannot let the caller reap or conclusively observe arbitrary non-child descendants.

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
* Termination is followed by an awaited reap of every direct child and by the strongest descendant-drain evidence the platform mechanism supplies.
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

## Selected public API family

### Caller-driven operation with a shutdown future and observer

Conceptually:

```rust
let outcome = engine
    .crf_search(request, token.cancelled(), |event| report(event))
    .await?;
```

The operation owns the race between completion and shutdown, stops internal producers, settles processes and tasks, cleans temporary state, and only then returns `Completed` or `Cancelled`. The cancellation input can be any `Future<Output = ()>`, allowing the CLI to pass an OS-signal future and CRFty to pass its private token future. The observer is synchronous, receives typed non-terminal events, and must return promptly; CRFty can forward events into its latest-value telemetry channel without making Tokio channels part of ab-av1's public API.

This pattern is established by axum's [`with_graceful_shutdown`](https://docs.rs/axum/latest/axum/serve/struct.WithGracefulShutdown.html) and tonic's [`serve_with_shutdown`](https://docs.rs/tonic/latest/tonic/transport/server/struct.Server.html#method.serve_with_shutdown). Those server APIs reinforce the mechanism-neutral signal shape, although ab-av1 is a finite media pipeline rather than a server and still needs typed progress and partial-output policy.

This family is selected because:

* The normal API cannot forget a separate finalization call.
* CLI and library cancellation can share one operation root.
* ab-av1 owns completion-versus-cancellation precedence and cleanup ordering.
* No background task or remote-control actor is required merely to provide cancellation.
* A synchronous observer does not bind the public API to a channel implementation, executor-owned stream, or asynchronous backpressure policy.
* Terminal results remain separate from progress, so dropping or coalescing telemetry cannot manufacture completion.

Accepted costs:

* A slow observer delays output parsing and can indirectly stall a child with full pipes, so the API must document the observer as non-blocking and CRFty must only perform bounded in-memory publication inside it.
* Dropping the entire returned future still bypasses awaited cleanup and relies on the Drop fallback.
* The signature and concrete future bounds become part of the public semver API.

### Rejected alternative: explicit operation handle

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

Watchexec's [`Job`](https://docs.rs/watchexec-supervisor/latest/watchexec_supervisor/job/struct.Job.html) is a mature example of a background supervisor with explicit ordered controls, stop, wait, restart, and signaling. [`tokio-process-tools`](https://docs.rs/tokio-process-tools/latest/tokio_process_tools/) likewise exposes explicit wait, cancel, abort, terminate, and kill operations and treats automatic Drop cleanup as a fallback. That shape is appropriate when the library owns a long-lived service, restart policy, or independently controlled background actor.

It is rejected for the initial ab-av1 API because a background operation generally requires spawning and therefore binds behavior to an executor and task ownership model; independently droppable event, control, and join handles multiply abandonment states; and restart, pause, and service-manager semantics are not requirements of a finite encode/search operation. CRFty already owns the runtime thread and application-level job handle, so reproducing that layer upstream would split supervision authority.

### Rejected alternative: public stream plus manual finalization

Conceptually, this is the current CRFty prototype shape: consume a command stream, drop it, and call a separate global `finish_job()` or `cancel_job()` function.

* The compiler does not connect a stream to the correct global finalization call.
* A caller can forget cleanup or overlap operations that share global state.
* Async cleanup cannot be guaranteed by Drop.
* Detached producers can outlive stream drop.
* Global process and temporary registries force restrictions that are not inherent to ab-av1's algorithms.

This option follows the existing typed CRF-search seam and requires fewer initial changes, but those advantages do not compensate for its unenforced lifecycle. It remains only a temporary CRFty adapter boundary until the selected operation API exists.

## External project evidence

### Evidence reinforcing the requirements

* Tokio's [`select!`](https://docs.rs/tokio/latest/tokio/macro.select.html) documentation explains that losing branches are cancelled by dropping their futures and requires authors to reason about every `.await` boundary.
* Tokio's [graceful-shutdown guidance](https://tokio.rs/tokio/topics/shutdown) separates detecting shutdown, notifying work, and waiting for tasks to finish. This supports CRFty's private token and worker-ledger design but does not require ab-av1 to publish the token type.
* Tokio's [`JoinHandle`](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html) documentation confirms that dropping a handle detaches the task, directly matching the upstream sample-producer risk.
* Tokio's [`JoinSet`](https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html) owns its tasks, aborts them on Drop, and can abort and await them through `shutdown()`, providing a maintained building block for internal structured ownership.
* Cargo-mutants' [process-management design](https://github.com/sourcefrog/cargo-mutants/blob/main/DESIGN.md) documents why terminating only an immediate child leaks nested test processes and why Unix process groups plus signal forwarding are necessary.
* [`process-wrap`](https://docs.rs/process-wrap/latest/process_wrap/) provides composable Tokio and standard process wrappers, Unix groups or sessions, Windows Job Objects, and a Tokio kill-on-drop shim. Its documentation describes it as the more flexible successor to `command-group` and retains the `command-group` test suite. Its explicit kill calls wrapper `start_kill()` and then `wait()`.
* [`command-group::AsyncGroupChild`](https://docs.rs/command-group/latest/command_group/struct.AsyncGroupChild.html) provides process-group kill and wait but warns that a cancelled asynchronous wait can leave its underlying blocking wait active, so process-wrapper selection needs an explicit wait-cancellation audit.
* [`processkit`](https://docs.rs/processkit/latest/processkit/) provides a broader Tokio runner with cancellation, capture, streaming, and kernel-backed containment. It reports whether Linux obtained cgroup v2 or fell back to a process group, which is more honest than claiming identical containment on every host.
* [`tempfile::TempDir`](https://docs.rs/tempfile/latest/tempfile/struct.TempDir.html) distinguishes best-effort Drop cleanup from explicit fallible `close()`, matching the required dual cleanup contract.

### Evidence limiting or refuting a single obvious API

* axum and tonic demonstrate generic shutdown futures, but they do not provide ab-av1's progress-stream, output-commit, or temporary-file semantics.
* Watchexec demonstrates that an explicit `Job` can be the correct public abstraction when restart, remote control, priorities, or multiple signals matter. Its feature set is broader than ab-av1 presently needs.
* SQLx documents that Rust's lack of async Drop requires an explicit [`Pool::close`](https://docs.rs/sqlx/latest/sqlx/pool/struct.Pool.html#method.close) for deterministic cleanup even though Drop handles local fallback cleanup. This refutes relying on an operation future's destructor as the complete contract.
* `tokio-process-tools` provides a correctness-focused process lifecycle but its automatic asynchronous Drop termination requires a multithreaded Tokio runtime. CRFty currently embeds ab-av1 on a current-thread runtime, so it is evidence and a possible component rather than an assumed drop-in solution.
* [`async-scoped`](https://docs.rs/async-scoped/latest/async_scoped/) documents the caveats and even unsafe surface involved in guaranteeing non-`'static` async scopes across cancellation. A narrow ab-av1 design should prefer ordinary owned futures and tasks over importing a generalized scoped-concurrency model without need.
* `process-wrap`'s Tokio `KillOnDrop` configures the underlying Tokio child. Its Unix `ProcessGroupChild` overrides `start_kill()` and `wait()` but has no Drop implementation that invokes group termination, so bare `KillOnDrop` is not a whole-process-group Drop guarantee on Unix. ab-av1 needs its own `ManagedChild` Drop guard around the wrapper.
* `process-wrap`'s Unix group wait uses `waitpid(-pgid, ...)`. POSIX wait calls can reap only children of the caller, so this does not prove that arbitrary grandchildren were reaped; after group SIGKILL and leader wait, orphaned grandchildren are the responsibility of their parent or the host subreaper.
* `processkit` supplies more of the desired lifecycle directly, including cgroup v2 when available, but its supervision, pipelines, limits, retries, capture policies, and public `CancellationToken` integration are substantially broader than ab-av1 needs. Its own platform and container documentation explains both that typical Linux systemd or container placement can prevent cgroup controller use and force a process-group fallback and that an ordinary process cannot reap an orphaned grandchild that was reparented to PID 1, so adopting it would not create a universal stronger guarantee.

## Selected lifecycle contracts

### Process containment and termination

Use `process-wrap`'s Tokio frontend behind a private ab-av1 `ManagedCommand` and `ManagedChild`. On Unix, each FFmpeg or FFprobe child is the leader of a fresh process group. On Windows, each child is assigned to a Job Object before it is resumed, and GUI consumers can compose the no-window creation flag before the Job Object wrapper. ab-av1 exposes none of these dependency types publicly.

Every external command path, including sample copy, the retry with generated timestamps, FFmpeg version discovery, and FFprobe, must use this abstraction. The awaited cancellation path calls whole-unit force termination and then waits for the direct child leader and owned pipe tasks. On Windows it also waits for Job Object completion. On Unix, successful group SIGKILL plus leader reap is the available contract; it does not falsely claim that the caller reaped non-child descendants. Concurrent stdout and stderr drains remain owned until EOF or deliberate closure, so termination cannot return while pipe-reader work is detached.

Cancellation uses immediate force termination rather than a graceful signal interval. ab-av1 writes disposable samples and caller-selected staging output; CRFty never promotes a cancelled staging file. Sending `q`, SIGINT, or SIGTERM would add platform-dependent latency and produce a finalized partial file that is still discarded. A later opt-in graceful policy can be additive, but it is not part of the initial contract.

The `ManagedChild` Drop implementation synchronously starts whole-unit termination and then relinquishes the handle to the runtime's best-effort reaper. Drop cannot promise awaited settlement or report failure. The ordinary operation path must therefore call an explicit consuming `terminate_and_wait()` or `wait()` before returning. `#[must_use]`, private constructors, and a denied `unused_must_use` lint make accidental abandonment visible without pretending Rust has asynchronous Drop.

`processkit` is the strongest alternative if ab-av1 later needs observable cgroup containment, limits, or a complete process runner. It is not selected initially because `process-wrap` provides the narrow group/job primitives needed by a trusted pinned FFmpeg and FFprobe toolchain, preserves ab-av1's existing streaming parser, and avoids importing service supervision and runner policy. FFmpeg and FFprobe are not expected to daemonize or intentionally escape their group; if that trust assumption changes, the boundary must move to cgroup, subreaper, container, or equivalent externally owned containment. Direct platform bindings are rejected because the project would reproduce reviewed Job Object, process-group, wait, and wrapper-ordering code.

### Cancellation and terminal precedence

The operation has one linearization point: production of its domain result after the final child has exited and any staging file has reached the state ab-av1 promises. A biased root race gives that completed domain result precedence when it and cancellation are observed in the same poll. Once the domain result linearizes, cancellation no longer changes the disposition; mandatory task, process, and temporary settlement still runs before the result is returned.

If cancellation is observed before the domain result linearizes, the operation closes process registration, broadcasts its private internal cancellation signal, terminates managed children, joins internal tasks, cleans its operation-local temporary state, and returns `Outcome::Cancelled`. A plain cancelled outcome therefore means settlement succeeded.

An operation or cancellation followed by failed process settlement or explicit temporary cleanup returns an error that preserves both the initiating disposition and every settlement failure. Cleanup failure is never collapsed into `Cancelled`, and a cleanup error never erases the earlier encode or search failure. Exact Rust variant names remain a PR-level naming choice, but the information model is fixed: primary disposition plus ordered settlement failures.

### Task ownership, FFprobe, and runtime support

Replace the detached `spawn_local` sample producer with operation-owned work that has a sticky private cancellation signal, a retained join authority, and an abort-on-drop fallback. Normal cancellation is cooperative and awaited; abort exists only for abandonment or panic fallback after every subprocess future has its own `ManagedChild` guard. No ordinary `JoinHandle` may leave the operation context without a unique join owner.

Remove the public or internal requirement for `LocalSet`. Library futures and internal tasks should be `Send` and run on either a current-thread or multithreaded Tokio runtime supplied by the caller. ab-av1 does not create a runtime in its library API; the CLI remains free to use `#[tokio::main(flavor = "current_thread")]`.

Replace the synchronous `ffprobe` crate call with the managed asynchronous command path and parse its machine-readable output. Tool paths are immutable engine configuration rather than PATH mutation or thread-local dynamic scope. Cache identity derives from the actual configured FFmpeg build. Short filesystem/cache critical sections may use blocking tasks only when their join authority is retained; cancellation waits for an already-running blocking section rather than claiming it was aborted.

### Temporary ownership and concurrency

Each operation creates a unique temporary namespace and registry. Keep policy belongs to the request, explicit cleanup reports all failed removals, and Drop only attempts best-effort cleanup. No operation calls a process-global `clean_all()` or can remove another operation's files.

The library contract permits overlapping operations because operation-local ownership makes overlap correct, but it does not promise fairness or manage a resource budget. CRFty continues to enforce one active ab-av1 job in its own coordinator. Supporting overlap at the boundary avoids encoding CRFty's product policy or the current global-registry limitation into ab-av1's semver API.

### Stable package and API surface

Add a library target to the existing `ab-av1` package rather than creating a second published package. Keep CLI parsing and presentation behind the default `cli` feature so `cargo install ab-av1` retains its behavior while `default-features = false` library consumers avoid Clap, `indicatif`, terminal detection, signal handling, logger initialization, and process exit.

Expose an immutable `Engine` or equivalent built from explicit FFmpeg, FFprobe, temporary-root, and cache configuration. Expose narrow search and encode request builders, typed non-terminal event enums, typed successful results, `Outcome<T>`, and a structured operation error. Keep fields private and use `#[non_exhaustive]` only on reported enums that are expected to grow; builders avoid making every request-field addition a semver break.

Keep `process-wrap`, Tokio synchronization types, `indicatif`, Clap, and cache implementation types private. Run [`cargo-semver-checks`](https://github.com/obi1kenobi/cargo-semver-checks) against the previous release and review a [`cargo-public-api`](https://github.com/Enselic/cargo-public-api) diff before publishing. Use Clippy's [`disallowed_methods`](https://doc.rust-lang.org/clippy/lint_configuration.html) configuration to forbid direct `tokio::process::Command::spawn`, `tokio::task::spawn_local`, and unowned task spawning outside the private operation/process modules; this turns the ownership boundary into a checked convention instead of relying only on review.

The terms that most accurately describe the direction are **structured concurrency**, **cooperative cancellation**, **resource-safe cancellation**, **process-tree containment**, **explicit asynchronous close**, **RAII fallback**, and **capability-based ownership**. The API is not fully structured concurrency in the language-level sense because Tokio tasks remain `'static` and Drop cannot await, but every child task and process is nested under one accountable operation lifecycle.

## Reviewable upstream patch sequence

1. Add the library target and default `cli` feature, move terminal rendering behind that feature, and keep `cargo install` behavior unchanged without publishing command internals.
2. Introduce private engine/toolchain configuration and route FFmpeg version discovery and FFprobe through explicit configured paths.
3. Introduce `ManagedCommand` and `ManagedChild` over `process-wrap`, then migrate every FFmpeg and FFprobe spawn, including sample-copy retries, while preserving the current parsers.
4. Replace global temporary state with an operation-local temporary namespace and make explicit cleanup fallible.
5. Replace the detached sample producer with owned, cancellable, joined work and remove the `LocalSet` requirement.
6. Add narrow request, event, result, outcome, and error types plus the caller-driven operation methods; adapt the CLI to those same methods.
7. Remove the prototype's global `finish_job()` and `cancel_job()` surface once CRFty consumes the owned operation boundary.

The patch touches package/module boundaries, command presentation, every subprocess construction site, temporary ownership, sample production, cache/tool identity, and the CRFty adapter. It is a lifecycle refactor rather than a cancellation-token parameter addition. The sequence above keeps each review centered on one ownership boundary and leaves the real-process validation matrix in issue #105.

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

[ADR-003](../adr/003-embed-a-pinned-ab-av1-adapter.md) accepted an embedded, pinned ab-av1 adapter. Its core integration direction remains compatible with this research, but its statement that the real-process prototype demonstrated the complete lifecycle is too broad: the proof did not cover the detached sample producer and unmanaged sample-copy FFmpeg path. Because accepted ADRs are immutable, proposed ADR-021 refines that boundary instead of rewriting or superseding ADR-003.

[ADR-018](../adr/018-unify-job-cancellation-and-completion.md) concerns CRFty's private cancellation, terminal-report, telemetry, and worker-ownership contract. It requires an awaited ab-av1 terminal lifecycle without permanently naming the prototype's `finish_job()` or `cancel_job()` functions. Proposed ADR-021 selects the generic cancellation-future boundary between that private supervisor and ab-av1.

## Residual maintainer choices

The architecture no longer depends on unresolved lifecycle choices. Upstream review may still choose exact type and method names, callback argument ownership, builder ergonomics, event granularity, cache backend, and whether `process-wrap` is accepted or its narrow behavior is implemented another way. Any substitute must preserve the selected operation, containment, Drop-fallback, settlement, race-precedence, toolchain, temporary-ownership, concurrency, and semver contracts.

The maintainer may also prefer a separate published core package instead of the selected same-package library target. That packaging choice is acceptable if the CLI still consumes the identical operation implementation and library consumers do not inherit terminal dependencies. It does not reopen the lifecycle decision.

## Coordination boundary

CRFty's private supervision remains in ADR-018 and issue #85. Proposed ADR-021 records the selected adapter boundary. Issue #105 owns implementation and real-process contract validation; neither test implementation nor its results are research completion criteria. Upstream issue #371 owns maintainer coordination and any eventual pull-request authorization.

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
* [`process-wrap` Tokio child wrapper source](https://github.com/watchexec/process-wrap/blob/0c3d820/src/tokio/core.rs)
* [`process-wrap` Unix process-group source](https://github.com/watchexec/process-wrap/blob/0c3d820/src/tokio/process_group.rs)
* [`processkit`](https://docs.rs/processkit/latest/processkit/)
* [`command-group`](https://docs.rs/command-group/latest/command_group/struct.AsyncGroupChild.html)
* [`tokio-process-tools`](https://docs.rs/tokio-process-tools/latest/tokio_process_tools/)
* [`tempfile`](https://docs.rs/tempfile/latest/tempfile/)
* [Cargo SemVer compatibility](https://doc.rust-lang.org/cargo/reference/semver.html)
* [`cargo-semver-checks`](https://github.com/obi1kenobi/cargo-semver-checks)
* [Clippy lint configuration](https://doc.rust-lang.org/clippy/lint_configuration.html)
