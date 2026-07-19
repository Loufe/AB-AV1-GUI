#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)
)]

//! Pure domain logic for CRFty.
//!
//! This crate cannot depend on processes, filesystems, clocks, async runtimes,
//! or user-interface frameworks.
