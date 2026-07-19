# CRFty

CRFty is a desktop application for quality-targeted AV1 conversion. It analyzes
video content, selects encoding parameters that meet a perceptual quality target,
and manages batch analysis, conversion, history, and statistics.

This branch contains the Rust rewrite planned for V3. The current Python
application remains on [`main`](https://github.com/Loufe/AB-AV1-GUI/tree/main)
until the rewrite reaches feature parity. The design record and scope freeze live
in [issue #33](https://github.com/Loufe/AB-AV1-GUI/issues/33).

## Status

The rewrite is at its foundation stage. The workspace establishes dependency and
safety boundaries; it does not yet contain application behavior or a user
interface.

## Workspace

- `crates/crfty-core`: pure domain logic; no processes, filesystem, clock, or UI
- `crates/crfty-engine`: process and filesystem integration; no Tauri dependency
- Tauri shell and web UI: added only after the engine boundary is proven

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
