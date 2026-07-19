#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)
)]

//! Process and filesystem integration for CRFty.
//!
//! This crate may depend on [`crfty_core`] but cannot depend on Tauri or other
//! user-interface frameworks.

pub mod ab_av1;
pub mod coordinator;
pub mod driver;
pub mod journal;
pub mod media;
pub mod output;
