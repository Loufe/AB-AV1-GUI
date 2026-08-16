# CRFty Architecture

CRFty is a desktop application for quality-targeted AV1 conversion: it searches for the lowest CRF that still meets a VMAF target, then encodes at that CRF. V3 is a ground-up Rust and Tauri rewrite of the Python application retained on `main`, delivering that product's intentional behavior without its accidental semantics or compatibility machinery.

This file is an index and stays one. Each subsystem's behavior is contracted in its own document, each cross-module decision in an ADR, and nothing is specified in two places.

## Guiding principle

The Python application held its invariants together with documentation and discipline: rule files, thread-safety comment blocks, and review habits. V3 relocates each of those invariants into a mechanism that enforces it: the type system, a module boundary, the crate graph, the storage format, generated bindings, and lints. The reducer is the largest single instance, turning single-threaded ownership, mutation-implies-notification, and the queue-edit-during-a-run edge cases into the literal shape of the program. The goal is a system whose rules are self-enforcing, so that documentation describes the system rather than holding it together.

## Workspace

- `crfty-core`: pure domain. State, reducer, fold, policy, projections, and the journal codec, with no filesystem, process, clock, or async runtime.
- `crfty-engine`: external processes and filesystem I/O. Driver, ab-av1 adapter, scanning, output settlement, vendoring, logging. No Tauri.
- `crfty-shell`: the Tauri command and event bridge. No domain logic.
- `ui/`: the React frontend, consuming generated bindings only.

Crates split on hard dependency boundaries (no Tauri, no process), never by topic, per ADR-001 (separate core, engine, and shell). Every mutation has one owner, the synchronous driver and reducer, per ADR-002 (centralize mutation in a driver reducer). Each crate and `ui/` carries its own AGENTS.md with subsystem rules.

## Subsystems

| Subsystem | Specified in |
|---|---|
| Crate boundaries and language discipline | this file; ADR-001 (separate core, engine, and shell), ADR-005 (forbid first-party unsafe Rust) |
| Driver, reducer, and durable state | ADR-002 (centralize mutation in a driver reducer), ADR-004 (persist state in an append-only journal), ADR-009 (compact the journal into a snapshot head line), ADR-011 (acknowledge corruption by generation identity) |
| Eligibility and conversion policy | `docs/POLICY.md`; ADR-007 (pin the actual decode mode in the analysis identity), ADR-013 (filter queue adds at enqueue) |
| Analysis pipeline | `docs/ANALYSIS.md`; ADR-016 (scope analysis work to ephemeral generations), ADR-017 (keep analysis paths engine-native) |
| Job lifecycle, cancellation, and output settlement | `docs/design/lifecycle.md`; ADR-018 (unify job cancellation and completion; proposed) |
| ab-av1 adapter | ADR-003 (embed a pinned ab-av1 adapter) |
| Events and IPC | `docs/design/event-stream.md`; ADR-006 (generate IPC bindings with tauri-specta) |
| History and statistics | `docs/HISTORY.md`; ADR-015 (project imported history separately) |
| V2 history import | `docs/HISTORY_IMPORT.md`; ADR-015 (project imported history separately) |
| FFmpeg vendoring | ADR-010 (vendor pinned FFmpeg with checksummed atomic installs) |
| Privacy and logging | ADR-014 (scrub paths inside the log sink) |
| Single-instance ownership | ADR-008 (take an engine-owned data-directory lock) |
| Testing strategy and gates | `docs/TESTING.md` |
| Delivery sequence | `docs/PLAN.md` |
| Working research and design notes | `docs/design/` |

History ownership is engine-side and single-boundary: an observation becomes durable only by passing through the reducer and its journal commit, and every History or Statistics view is a pure projection of that durable state rather than an independently maintained total. `docs/HISTORY.md` owns the rest, including the observation model, the transaction boundary an observation commits under, the query surface that serves those views, and how an active view learns that a mutation invalidated what it is showing.

## Release boundary

V3 targets packaged Windows and Linux desktop use. The following stay outside this release:

- automatic installation updates (the manual release check remains),
- portable mode,
- tray behavior and native notifications,
- pause as a first-class state, and the resource automation behind it (user, low-disk, and low-battery triggers), since Stop covers the need,
- stall watchdogs,
- full-file duplicate confirmation,
- per-scene encoding.
