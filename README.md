# CRFty

CRFty is a desktop application for quality-targeted AV1 conversion. It analyzes
video content, selects encoding parameters that meet a perceptual quality target,
and manages batch analysis, conversion, history, and statistics.

This branch contains the Rust rewrite planned for V3. The current Python
application remains on [`main`](https://github.com/Loufe/AB-AV1-GUI/tree/main)
until the rewrite reaches feature parity. The design record and scope freeze live
in [issue #33](https://github.com/Loufe/AB-AV1-GUI/issues/33).

## Status

The rewrite has its workspace foundation, pinned ab-av1 integration, durable
job coordinator, Tauri shell, and web UI. Queue claims, content-keyed media
records, analysis reuse, hardware-decode selection, analysis/encode lifecycle,
force cancellation, atomic journal replay, MKV-only lossless remux for existing
AV1, output promotion, and crash recovery are implemented and covered by unit
and real-process contract tests. The event stream publishes each command's
ephemeral deltas before its durable deltas, so a finished item's final
telemetry and telemetry clear always precede its finish event. The engine
starts without FFmpeg or ffprobe: missing tools surface as typed availability
on the stream and gate media sessions while the queue, history, and settings
stay fully usable. The frontend folds that stream into its stores against
golden fixtures generated from the Rust fold. The durable domain model is
complete (issue #38): structured failure facts, wall-clock run instants with
monotonic phase spans, evidence-carrying success outcomes derived from the
settled output ledger (including crash recovery), expanded probe metadata,
content verdicts with derived lineage and a frozen reuse policy, per-item
analysis intent, and the hardware→software retry ladders for search and
encode. The queue command surface is complete (issue #41): batch adds expand
folders through the engine scanner and filter ineligible files at enqueue
into one typed summary (ADR-013), decided verdicts and content duplicates
short-circuit at claim as visible skipped rows, and items support per-item
edit, retry, clear, and clear-completed. Sessions publish running aggregate
totals and live speed/ETA telemetry, open/reveal desktop actions round out
the shell commands, and the public event stream is bounded — on overflow it
severs observably instead of blocking the driver, with reconnect-and-refold
as the recovery. What remains is growing the views over the store — History,
Statistics, Analysis, and the final Queue integration.

## Workspace

- `crates/crfty-core`: pure domain logic; no processes, filesystem, clock, or UI
- `crates/crfty-engine`: process and filesystem integration, including the
  isolated ab-av1 adapter; no Tauri dependency
- `crates/crfty-shell`: thin Tauri bridge between the engine's command and
  event surface and the webview; no domain logic
- `ui/`: Vite + React + TypeScript + Tailwind frontend, pnpm-managed (see
  `ui/README.md`)

## Development

Install the current stable Rust toolchain with Rustfmt and Clippy, then run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo deny check
cargo vet
```

CRFty is licensed under GPL-3.0-or-later. See [LICENSE](LICENSE).
