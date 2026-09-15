# Alpha delivery scope

Status: accepted delivery scope; requirements below define acceptance and do not claim shipped behaviour

## Purpose and boundary

The alpha is an installable Windows and Linux desktop application for exercising the complete analysis and conversion workflow. It retains the agreed runnable intermediate: durable observations for terminal work, History browsing, and import v1. This scope separates alpha acceptance from V3 feature completion; it does not remove the remaining V3 commitments.

GitHub owns work order, blockers, and acceptance evidence. The [alpha issues](https://github.com/Loufe/AB-AV1-GUI/issues?q=is%3Aissue%20is%3Aopen%20label%3Aalpha) track required work within the `v3.0` milestone. An alpha requirement closes only on executable evidence for its shipped behaviour.

## User workflow

A clean installation discovers user-supplied FFmpeg and ffprobe and verifies their capabilities before conversion. Users can select files or a folder, discover content, run Basic Scan, select rows, and enqueue Analyze or Convert. The first Analysis presentation is a flat table showing relative folders. Stable row identity preserves selection through streamed updates; replacing a generation cannot apply an action to stale selections.

Basic Scan shows observed metadata and current applicability. Unsupported pre-analysis estimates are absent. A completed quality search exposes ab-av1's predicted output size and duration for the applicable source and execution profile, labelled as predictions. Historical prediction evidence cannot upgrade a file's level or suppress required work. The historical estimator family and its evaluation contract remain as decided in `estimation.md`; alpha does not require that estimator.

The queue exposes progress, contextual errors, Stop After File, and Force Stop. After completion or restart, the user can inspect retained results and import v1 evidence through History. Basic keyboard access, focus handling, and visible failures belong to these workflows.

## Lifecycle and durable evidence

Every probe on the claim, search, encode, and output-verification paths has bounded execution and reachable cancellation. Cancellation settles owned tasks and contained processes before the engine advances; cleanup failure stays visible. Windows spawn failures leave no child or native handle behind. Required fixes may land through reviewed pinned dependencies without waiting for upstream acceptance or the complete shared supervision implementation.

Output promotion and source protection keep their existing transaction guarantees. Source changes during processing require an explicit output disposition and honest evidence. Assigned run identifiers are never reused. User cancellation, operation failure, and crash interruption have distinct outcomes. Reservation-only crash handling is resolved in the terminal contract before History adopts the identity.

The minimum History contract specifies stable observation identity, retries, terminal outcomes, source and execution facts, metric-tagged predictions and measurements, optional values, and import identity and conflicts. A reportable completion and its observation commit atomically. Discovery, Basic Scan, and queue skips do not create History observations. Failed, stopped, and incomplete work retain sparse evidence even if dedicated browsing filters arrive later.

Storage selection proves these writes, deterministic pages, restart recovery, strict idempotent import, and a single writer on Windows and Linux. Representative fixtures include missing evidence, repeated work, source changes, and imported records. Prediction and measurement pairs survive as separate facts. Research collectors, future bundle packaging, and the final estimator query strategy do not gate this minimum contract. Changes to operational durability require their own explicit decision; choosing History storage alone does not authorize that replacement.

## Installation and product truthfulness

CI-built alpha artifacts are exercised from an installed location, first with no media tools reachable and then with a user-installed FFmpeg configured through Settings. Tool discovery, the session-start capability probe, execution, restart, and native conversion must pass on both supported platforms, and the install guidance shown for each platform is followed on a clean machine.

Filesystem work and History import and queries leave the interface responsive. Command failures remain inspectable in context; the complete notices panel can follow. A privacy control is enabled only when recording and import enforce its promise. Until then it is disabled and explicitly unavailable. Existing anonymized logs and anonymous records retain their protections, and records without readable paths expose no path actions.

## Completion beyond alpha

V3 retains historical size and time estimation, approved environment and content collectors, Statistics over first-class observations, enriched import, History scrubbing, failed and stopped filters, background analysis, final Analysis presentation, IPC simplification, and supervision consolidation. Alpha work includes focused correctness tests; it does not wait for broad cleanup or the complete later acceptance suites.

Portable export and pooled research remain post-3.0. Any identity or import rule needed by alpha is owned by the minimum History contract, rather than deferred to the export bundle specification. Alpha validation must not depend on post-3.0 issue completion.
