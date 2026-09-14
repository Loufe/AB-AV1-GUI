# CRFty

CRFty is a desktop application for quality-targeted AV1 conversion. It analyzes video content, selects encoding parameters that meet a perceptual quality target, and manages batch analysis, conversion, history, and statistics.

This branch contains the Rust rewrite planned for V3. The current Python application remains on [`main`](https://github.com/Loufe/AB-AV1-GUI/tree/main) until the rewrite reaches feature parity. The design record lives in [docs/PLAN.md](docs/PLAN.md) and [docs/adr/](docs/adr/).

## Status

The rewrite has its workspace foundation, pinned ab-av1 integration, durable job coordinator, Tauri shell, and web UI. Queue claims, content-keyed media records, analysis reuse, hardware-decode selection, analysis and encode lifecycle, force cancellation, atomic journal replay, MKV-only lossless remux for existing AV1, output promotion, and crash recovery have unit and real-process contract coverage. The event stream publishes each command's ephemeral deltas before its durable deltas, so a finished item's final telemetry and telemetry clear always precede its finish event. FFmpeg is user-supplied: the engine discovers it from an environment override, Settings paths, or PATH and verifies the encoder and quality filter with a capability probe at every session start (ADR-023). It starts without FFmpeg or ffprobe: missing or incapable tools surface as typed availability on the stream, with install guidance in Settings, and gate media sessions while the queue, history, and settings stay fully usable. The frontend folds that stream into its stores against golden fixtures generated from the Rust fold. The durable domain model includes structured failure facts, wall-clock run instants with monotonic phase spans, and evidence-carrying success outcomes derived from the settled output ledger, including crash recovery. It also includes expanded probe metadata, content verdicts with derived lineage and a frozen reuse policy, per-item analysis intent, and the hardware→software retry ladders for search and encode. The queue command surface is complete. Batch adds expand folders through the engine scanner and filter ineligible files at enqueue into one typed summary (ADR-013). Decided verdicts and content duplicates short-circuit at claim as visible skipped rows, while items support per-item edit, retry, clear, and clear-completed. Sessions publish running aggregate totals and live speed/ETA telemetry, and open/reveal desktop actions round out the shell commands. The bounded public event stream severs observably on overflow instead of blocking the driver, with reconnect-and-refold as the recovery. Queue, History, Statistics, and Settings have production views over the current model. Analysis has generation-scoped streaming discovery and the bounded, cancellable [Basic Scan pipeline](docs/ANALYSIS.md) in the engine, while its production view remains a disabled empty state.

The [alpha scope](docs/design/alpha.md) defines the first installable workflow and its reliability requirements. Known probe cancellation, Windows spawn cleanup, and terminal identity defects remain. The [alpha issues](https://github.com/Loufe/AB-AV1-GUI/issues?q=is%3Aissue%20is%3Aopen%20label%3Aalpha) identify the acceptance gate and required work; [the plan](docs/PLAN.md) records the branch state and remaining V3 direction.

## Workspace

- `crates/crfty-core`: pure domain logic; no processes, filesystem, clock, or UI
- `crates/crfty-engine`: process and filesystem integration, including the isolated ab-av1 adapter; no Tauri dependency
- `crates/crfty-shell`: thin Tauri bridge between the engine's command and event surface and the webview; no domain logic
- `ui/`: Vite + React + TypeScript + Tailwind frontend, pnpm-managed (see `ui/README.md`)

## Development

Install the current stable Rust toolchain with Rustfmt and Clippy, then run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo deny check
```

CRFty is licensed under GPL-3.0-or-later. See [LICENSE](LICENSE).
