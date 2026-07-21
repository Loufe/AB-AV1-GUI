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
pub mod config;
pub mod coordinator;
pub mod driver;
pub mod history_import;
pub mod journal;
pub mod lock;
pub mod logging;
pub mod media;
pub mod os_actions;
pub mod output;
pub mod rate;
pub mod release;
pub mod remux;
pub mod scan;
pub mod vendor;

mod failure;
mod filesystem;
mod power;
mod process;
