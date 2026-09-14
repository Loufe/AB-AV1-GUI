# Testing strategy

The semantic load is carried by pure-domain tests in `crfty-core` and real-process contract tests in `crfty-engine`. Everything else exists to keep those two honest: generated artifacts are diffed rather than trusted, the frontend mirrors of Rust logic are checked against fixtures exported from Rust, and process behaviour is proven against actual child processes rather than mocks of them.

## Gates

Three CI workflows, mirroring what a commit is expected to pass locally.

- `.github/workflows/rust.yml`, on every push and pull request against `rewrite`, on Ubuntu and Windows: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --all-features --locked`, then a `git diff --exit-code` over the three generated artifacts. A separate Linux step greps `crates/**/*.rs` and fails the build on any `allow(` attribute. A supply-chain job runs `cargo deny check`.
- `.github/workflows/ui.yml`, on changes under `ui/`: `pnpm lint`, `pnpm format:check`, `pnpm typecheck`, `pnpm knip`, `pnpm test`, `pnpm build`, with Chromium installed for the browser test project.
- `.github/workflows/media-contract.yml`, on changes to `Cargo.lock`, core or engine sources, or engine tests, on Ubuntu and Windows: installs a current BtbN FFmpeg build at job time and runs the `#[ignore]`d real-media and native-tool contract suites against it. Because the ab-av1 revision is pinned in `Cargo.lock`, changing that revision triggers this suite.

Lint discipline extends to tests. Tests run under the same workspace lints as production code, `#[allow(...)]` is forbidden outright, the narrowest item-scoped `#[expect(..., reason = "...")]` is the only escape, and unsafe code stays forbidden everywhere including tests.

## Layers

- Pure domain, `crfty-core`. `src/tests.rs` is the large suite covering the reducer, policy, journal encode and replay, the output ledger and recovery, queue administration, session aggregates, and import adoption. Per-module `#[cfg(test)]` blocks cover analysis identity and reuse validation, estimation, history, projections, failure facts, and time.
- Properties, via `proptest`. `replay_equals_live_fold` generates queue command sequences, journals the emitted durable deltas, and asserts the replayed state equals the live durable state, which is the reducer's central property. Estimation and projection modules carry their own property tests.
- Engine units. Roughly thirty `#[cfg(test)]` modules across `crfty-engine`, including output path resolution, privacy scrubbing, log rolling, config, media parsing, rate tracking, tool discovery and version parsing, and the analysis runtime.
- Engine contracts, `crates/crfty-engine/tests/`. Real files, real child processes, real journals. Described below.
- Shell. `tests/export_bindings.rs` is not an assertion but a generator: it exports `ui/src/lib/bindings.ts` from the Rust types via tauri-specta, and CI fails if the committed file moved.
- Frontend, `ui/`. Vitest with two projects: `node` for pure logic in `*.test.ts`, and `browser` for `*.browser.test.tsx` rendering real components under Playwright Chromium against a Tauri IPC stand-in in `src/test/browser/`.

## Engine contract suites

Each file is a contract with a stated subject, not a grab bag.

- `durability.rs`, the largest suite, covers data-directory locking, journal replay, torn-tail truncation, byte-identical preservation of a corrupt journal, corruption acknowledgement and recovery, compaction into a snapshot head, group commit, and the crash sentinel. It also covers persist-before-emit ordering across restart, lossy telemetry coalescing under pressure while the terminal value wins, event-stream overflow, and history import parking and adoption. Output-ledger coverage includes promotion, original retirement, partial-staging recovery, identity-authorized abandonment, conflicts without deletion, pre-staging overwrite policy, and hardlink-preserving same-path replacement.
- `ab_av1_process.rs`: the native adapter's process contract and the hardware-to-software retry ladder, driven by a fixture binary rather than real media.
- `ab_av1_real_media.rs`: `#[ignore]`d and gated behind the `contract-test-fixture` feature. One test drives real search, encode, cancellation, adapter panic, and a subsequent successful job through the coordinator, which is where cancellation, child cleanup, panic containment, and reuse after every failure mode are actually proven.
- `remux_process.rs` and `process_supervisor.rs`: process trees. Concurrent draining of large stdout and stderr streams, diagnostic tail retention, spawn failure, timeout terminating the process group, descendants that keep or close their pipes, cancellation before spawn, and cancellation terminating the native process tree and joining its readers.
- `analysis_basic_scan.rs`: the bounded probe pool streams results, cancellation drops queued work and joins running probes, and one probe failure becomes typed scrubbed row state without failing its siblings.
- `tool_availability.rs`: the media tool contract. The engine starts, replays, and serves non-media commands with no tools located; startup recovery defers unsettled output transactions rather than settling blind; discovery honours its precedence (environment override, Settings path, `PATH`) and fails closed on explicit tiers; the session-start capability probe verifies fixture tools and records their revisions on the claim, refuses to reserve when a capability is missing, and is terminated promptly by shutdown while hanging.
- `tools_native.rs`: `#[ignore]`d. Real FFmpeg copied into a directory whose path contains spaces and non-ASCII characters, configured through Settings paths with no `PATH`, probed at session start, and run through a full conversion; the probed revisions must be the provenance on the claim.

## Fixture conventions

Golden fixtures are exported from Rust, committed, and freshness-gated. `cargo test -p crfty-core --test export_fold_fixtures` writes `ui/src/lib/store/fold-fixtures.json`; `cargo test -p crfty-core --test export_projection_fixtures` writes `ui/src/lib/projection/projection-fixtures.json`; `cargo test -p crfty-shell --test export_bindings` writes `ui/src/lib/bindings.ts`. Regenerate whenever the corresponding semantics change; CI diffs all three.

Those fixtures are what make the frontend mirrors safe. `ui/src/lib/store/fold.ts` mirrors `crfty_core::fold` and `ui/src/lib/projection/history-rows.ts` mirrors `crfty_core::history_rows`, each verified against its golden file. Semantics change in Rust first, then are ported.

`ui/src/lib/format/parity-fixtures.json` is the exception: hand-maintained spec data freezing V2 display semantics, with no regeneration path. Editing it is a deliberate, reviewed semantic change.

Process fixtures replace tools rather than mocking them. The `crfty-contract-fixture` and `crfty-process-fixture` binaries, both behind the `contract-test-fixture` feature, stand in for FFmpeg, ffprobe, and misbehaving children so process lifecycle can be tested deterministically and without media. The contract fixture answers the capability probe from a marker file beside it, so incapable and hanging tools are tested without media, and the real-tool suites locate binaries through `CRFTY_FFMPEG` and `CRFTY_FFPROBE`.

Policy is tested rule by rule against `evaluate_eligibility`, `select_analysis`, `evaluate_enqueue`, and `select_job_action` directly. Coverage includes the post-rotation pixel floor, AV1 container decisions, exact-then-lowest-qualifying target selection, decode-mode reuse identity, verdict freshness, enqueue and claim gating, absent-fact fail-open behaviour, and the `NotWorthwhile` fallback-floor rule.

V2 is a read-only oracle, consulted through `git show main:<path>` and frozen as committed fixtures when a semantic is worth keeping. Generation scripts are throwaway and never committed; frozen fixtures are hand-maintained spec data afterwards. The only committed Python is `tools/export_history_v3.py` and its test, run with `uvx pytest tools/test_export_history_v3.py` and never imported by the build.

## Rules

- Pure logic requires focused tests in the same change.
- Process behaviour requires real-process contract tests in addition to unit tests.
- Human-oriented process output is never parsed as an application contract, so it is never the thing a test asserts on.
- Prefer making an invalid state unrepresentable over testing that it does not occur. Two implementations kept equivalent by a test harness is a defect to eliminate; the Rust-to-TypeScript mirrors are tolerated only because a generated golden fixture, not review, keeps them equal.
