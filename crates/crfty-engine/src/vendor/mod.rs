//! The vendor boundary: FFmpeg discovery, pinned-manifest downloads, and
//! revision provenance (issue #43). Discovery reports facts; the reducer owns
//! all gating of when tools may be swapped.

pub mod discovery;
pub mod manifest;
mod probe;
