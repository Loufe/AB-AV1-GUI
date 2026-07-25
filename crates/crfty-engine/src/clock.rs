//! The engine's single wall-clock reading. Core is deterministic and has
//! no clock, so every durable command payload that needs "now" takes it
//! from here.

use std::time::{SystemTime, UNIX_EPOCH};

use crfty_core::UnixMillis;

/// Wall-clock instant for durable command payloads; core has no clock. A
/// pre-epoch system clock degrades to zero rather than failing the run.
pub(crate) fn now_millis() -> UnixMillis {
    UnixMillis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            }),
    )
}
