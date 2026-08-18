---
status: accepted
date: 2026-08-18
---

# Drive ab-av1 Through an Owned Operation

## Context and Problem Statement

ADR-003 selected an in-process pinned ab-av1 adapter, and ADR-018 defines CRFty's private job supervision contract. The current fork exposes typed command streams followed by global `finish_job()` or `cancel_job()` calls. That boundary cannot connect the stream to the correct cleanup authority through ownership, and upstream ab-av1 can detach its sample producer while sample-copy FFmpeg remains outside global finalization.

CRFty needs a regular semver library boundary that preserves typed progress while making a terminal result mean that ab-av1's tasks, process trees, pipes, and temporary state have settled. The boundary must not require ab-av1 to expose CRFty's private `CancellationToken` or duplicate CRFty's background job supervisor.

## Decision Drivers

* Make incorrect lifecycle composition difficult through Rust ownership and private constructors
* Keep cancellation intent distinct from acknowledged terminal settlement
* Preserve typed progress without exposing Clap, `indicatif`, Tokio channels, or terminal I/O
* Make CLI and library consumers execute the same core operation
* Contain FFmpeg and FFprobe process trees and reap every direct child without overstating POSIX descendant-reaping authority
* Keep tool paths, temporary files, and cleanup authority operation-local
* Support CRFty's current-thread Tokio runtime without requiring `LocalSet`
* Keep the public dependency and semver surface narrow

## Considered Options

* Caller-driven async operation with a generic shutdown future and synchronous observer
* Background operation handle with event, control, and join capabilities
* Public command stream followed by manual global finalization
* Continue treating the ab-av1 CLI and NDJSON as the supported process boundary

## Decision Outcome

Chosen option: **Caller-driven async operation with a generic shutdown future and synchronous observer**, because ab-av1 performs finite one-shot work and should own its internal lifecycle without also owning CRFty's executor task, remote-control handle, or application policy.

The conceptual boundary is an immutable configured engine whose search and encode methods consume a typed request, a caller-supplied `Future<Output = ()>` cancellation signal, and a promptly returning synchronous observer for typed non-terminal events. The method returns a typed outcome only after operation-local settlement. CRFty passes its private token's cancellation future and forwards observer events into its own latest-value telemetry; the ab-av1 CLI passes its OS-signal future and renders the same events.

ab-av1 creates a private sticky cancellation signal for its internal workers. Every producer retains a unique join owner and an abort-on-drop fallback; normal cancellation signals and joins rather than detaching. The `spawn_local` sample producer and `LocalSet` requirement are removed, and public operation futures support ordinary current-thread and multithreaded Tokio runtimes supplied by the caller.

Every FFmpeg and FFprobe invocation uses a private operation-owned managed process abstraction. The proposed implementation uses `process-wrap`: a fresh process group on Unix, a Job Object assigned before resume on Windows, explicit whole-unit kill followed by direct-child wait, concurrent pipe drainage, and an ab-av1 `ManagedChild` Drop guard that synchronously starts whole-unit termination. `process-wrap`'s Tokio `KillOnDrop` is not sufficient by itself on Unix because it does not call the process-group wrapper's `start_kill()` from Drop.

The terminal contract follows the strongest evidence each platform can supply. Windows waits for Job Object completion. Unix broadcasts SIGKILL to the process group and reaps FFmpeg or FFprobe itself, but an ordinary POSIX parent cannot reap arbitrary grandchildren; orphan reaping belongs to their parent or the host subreaper. This limitation is accepted for the trusted pinned FFmpeg and FFprobe toolchain, which is not expected to daemonize or escape its group, and must be revisited if the child threat model broadens.

Cancellation force-terminates the containment unit immediately. ab-av1's samples and CRFty's staging output are disposable on cancellation, so graceful FFmpeg finalization would add platform-specific behavior and latency without producing an artifact CRFty can promote. A graceful policy may be added later as an explicit opt-in.

The domain disposition, whether success or operation failure, has precedence if it and cancellation are observed in the same root poll. Once that disposition linearizes, later cancellation cannot reclassify it or hide a real operation error, but mandatory settlement still runs. Cancellation observed before that point returns `Cancelled` only when settlement succeeds. Process, pipe, task, or temporary-cleanup failure returns a structured error that preserves both the initiating disposition and all settlement failures.

Engine configuration contains explicit FFmpeg and FFprobe paths, temporary root, cache location, and toolchain identity. Every operation owns a unique temporary namespace and registry. The library permits overlapping operations to be correct but does not schedule or budget them; CRFty continues to enforce its product policy of one active ab-av1 job.

The existing `ab-av1` package gains a library target, while CLI parsing and presentation remain behind the default `cli` feature. Public request construction uses builders with private fields. Public events, results, outcomes, and errors do not expose Clap, `indicatif`, `process-wrap`, Tokio synchronization types, cache types, stdout, stderr, logger initialization, or process exit.

Direct process spawning and unowned task spawning outside ab-av1's private lifecycle modules are denied with Clippy `disallowed_methods`; lifecycle handles and terminal results are `#[must_use]`; and releases review `cargo-public-api` output and run `cargo-semver-checks`. These checks reinforce the ownership boundary but do not replace real-process validation tracked by issue #105.

### Consequences

* Good: The normal public method cannot forget or mismatch a separate global finalization call.
* Good: CRFty and the CLI can use different cancellation mechanisms without adding `CancellationToken` to ab-av1's public API.
* Good: Terminal outcomes acquire a precise task, process, pipe, and temporary-settlement meaning.
* Good: Operation-local state makes overlap safe without forcing CRFty to run more than one encode.
* Good: A narrow observer preserves typed progress while keeping channels and executor ownership private to each consumer.
* Good: The process wrapper and lint boundary make direct unsupervised FFmpeg and FFprobe spawns reviewable violations.
* Bad: A slow or blocking observer can stall parsing and indirectly fill child pipes, so consumers must only do bounded in-memory publication in the callback.
* Bad: Dropping the entire operation future cannot perform awaited cleanup; the managed-child and temporary Drop paths remain best-effort fallbacks.
* Bad: The change reaches package structure, command presentation, every subprocess path, sample task ownership, temporary state, tool configuration, cache identity, and the CRFty adapter.
* Bad: `process-wrap` supplies containment primitives rather than the complete operation protocol, so ab-av1 still owns a focused lifecycle layer.

## More Information

This record owns the operation-lifecycle boundary, ADR-003 owns the adapter choice, and ADR-018 owns CRFty-private supervision. The evidence, counterexamples, `process-wrap` Drop caveat, public API comparison, race rules, and reviewable upstream patch sequence are in [`docs/design/ab-av1-library-lifecycle-research.md`](../design/ab-av1-library-lifecycle-research.md).

Coordination: [CRFty issue #104](https://github.com/Loufe/AB-AV1-GUI/issues/104), [upstream ab-av1 issue #371](https://github.com/alexheretic/ab-av1/issues/371), and implementation-validation [issue #105](https://github.com/Loufe/AB-AV1-GUI/issues/105).
