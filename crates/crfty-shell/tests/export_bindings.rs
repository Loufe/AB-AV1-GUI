//! Regenerates the checked-in TypeScript bindings consumed by `ui/` (ADR-006).
//!
//! CI verifies freshness with `git diff --exit-code -- ui/src/lib/bindings.ts`
//! after the test suite runs, so a stale file fails the build rather than
//! silently drifting from the Rust types.
#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use specta_typescript::Typescript;

#[test]
fn export_bindings() {
    crfty_shell::specta_builder()
        .export(
            Typescript::new(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/src/lib/bindings.ts"),
        )
        .expect("export TypeScript bindings");
}
