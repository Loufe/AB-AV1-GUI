//! The vendor boundary: FFmpeg discovery, pinned-manifest downloads, and
//! revision provenance. Discovery reports facts; the reducer owns
//! all gating of when tools may be swapped.

pub mod discovery;
pub mod download;
pub mod extract;
pub mod install;
pub mod manifest;
mod probe;
