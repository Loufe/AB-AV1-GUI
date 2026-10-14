---
status: accepted
date: 2026-09-14
---

# Require user-supplied media tools

## Context and problem statement

V3 executes external FFmpeg and ffprobe binaries and stamps every analysis with tool revisions (`AnalysisProfile.{ab_av1,ffmpeg,encoder}_revision`). The earlier design had the application download a pinned, checksummed FFmpeg build and install it atomically. That design rested on an artifact the application could fetch for its whole supported lifetime, and upstream retention broke that premise. BtbN keeps daily builds for fourteen days and monthly builds for two years. The pinned archives returned HTTP 404 and a clean managed install could not complete. Every durable replacement, project-owned hosting or CI-built artifacts, carries redistribution and GPL source-availability obligations, plus an update-shipping burden, that a two-binary dependency does not justify. The question is how the application obtains trustworthy tools without operating a distribution channel, and how it knows which build it is running.

## Decision drivers

* A released application must work for its whole supported lifetime without a download the project cannot guarantee
* No redistribution of GPL builds, and no hosting or signing infrastructure to operate
* An analysis cache hit must describe measurements the current tools reproduce (ADR-007, extended to tool provenance)
* A tool the user chose explicitly must never be silently replaced by another one
* Conversion must never start on tools that cannot do the work; the failure has to be typed, path-safe, and actionable
* The reducer stays free of processes and clocks: discovery and verification are engine facts reported to it

## Considered options

* Repoint the managed download at a durable artifact the project hosts or builds
* Stay inside upstream monthly retention with a monitoring owner and a bounded support policy
* Require a user-installed FFmpeg, discovered and verified by the application

## Decision outcome

Chosen option: **require user-installed ab-av1, FFmpeg, and ffprobe, discovered and verified by the application**. This removes the network, archive, and update machinery along with the retention risk. It keeps the project out of binary redistribution, and replaces trust in a download with evidence from the tools themselves.

Mechanics, fixed by this record:

* The application never downloads, extracts, or installs media tools, and the dependency tree carries no archive or hashing crates for that purpose. The only network client is the manual release check.
* Discovery precedence per tool is the `CRFTY_AB_AV1`/`CRFTY_FFMPEG`/`CRFTY_FFPROBE` environment override, then the path configured in Settings, then a search of `PATH`. Both explicit tiers fail closed: a path that does not name a file reports the tool missing instead of falling through, so a deliberate choice is never silently replaced. Discovery spawns nothing, is infallible, runs at engine start, on `ToolsCommand::Rediscover`, and whenever the configured paths change, and reports `ToolAvailability` to the reducer as ephemeral state.
* Located tools are `Pending` until verified. At every session start, before the first claim, the worker runs a bounded, cancellable capability probe: ffprobe's JSON version document, a one-second synthetic `libsvtav1` encode, and a `libvmaf` comparison of two synthetic inputs, each judged by exit status with a per-step timeout. An additional bounded synthetic quality search verifies the official ab-av1 JSON protocol. The probe then asks FFmpeg for the help text of every hardware decoder CRFty can name; a decoder whose query fails or times out is treated as absent. Human-oriented output is carried only as a diagnostic. Every session re-probes because the files behind a location can change between sessions. Force-stop and shutdown cancel a running probe.
* The current probe copies ffprobe's JSON program version into both the `ffmpeg` and `encoder` revision fields. These are provenance proxies, not independently verified revisions of the executed FFmpeg and its runtime SVT-AV1 library. A changed proxy invalidates cached analyses (ADR-007), but FFmpeg and ffprobe may resolve to different builds, and an encoder update may leave that proxy unchanged. Independent FFmpeg and encoder identity remains required to close that reuse gap. The ab-av1 revision is a content hash of the selected executable bytes, not an identity of loaded libraries. Core reads the reported revisions and available decoders: the reducer composes them with the configured base execution and Settings into each claimed `JobSpec`, and a claim while tools are unverified is rejected.
* Missing tools reject session and Basic Scan starts with a path-free reason; the typed failures, which do carry paths, travel only on the stream so the Settings view can show exactly which file failed together with per-platform install guidance. A failed probe reports its capability and diagnostic, ends the session without reserving an item, and leaves the next start free to probe again. The queue, history, statistics, and settings stay fully usable without tools.
* The Settings paths are absolute paths validated by core; the frontend offers a native file picker and a re-check action but holds no discovery logic of its own. Discovery resolves environment overrides and PATH matches to absolute paths, because ab-av1 runs from a private workspace directory.
* CI installs official ab-av1 0.11.7 and a BtbN build at job time for the real-media contract suites instead of caching a pinned archive; those builds are test infrastructure, not a product artifact.

### Consequences

* Good: No artifact can expire underneath a released application, and no redistribution or hosting obligation exists
* Good: Provenance rests on behaviour the tools demonstrated, not on a checksum over bytes that may no longer be obtainable
* Good: The engine loses a network client, archive handling, and an install state machine, along with their cancellation and shutdown cases
* Bad: First-run setup is the user's job; the application can only explain what to install and where to point
* Bad: The revision proxy can invalidate unchanged tools or miss a changed FFmpeg or encoder, so capability verification alone does not prove exact reuse provenance
* Bad: Every session start spends a few seconds on two tiny synthetic encodes and a handful of decoder queries before the first claim

## More information

Related: ADR-003 (the official ab-av1 CLI adapter these binaries serve), ADR-007 (identity honesty), ADR-018 (the cancellation contract the probe joins). Implementation: `crates/crfty-engine/src/tools/`; contract tests in `crates/crfty-engine/tests/tool_availability.rs` and `crates/crfty-engine/tests/tools_native.rs`, the latter run by `.github/workflows/media-contract.yml`. Discovery and the probe are described in `docs/ARCHITECTURE.md`.
