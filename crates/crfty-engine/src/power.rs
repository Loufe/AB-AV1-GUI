//! System sleep inhibition while a session runs.
//!
//! `keepawake` replaces V2's direct `SetThreadExecutionState` call (#33 §12).
//! Only system sleep is inhibited — the display may still turn off, matching
//! V2's `ES_SYSTEM_REQUIRED`-without-`ES_DISPLAY_REQUIRED` behavior. Windows
//! inhibition is per-thread (`ES_CONTINUOUS`), so the guard must be created
//! and dropped on the session worker thread itself.

/// Inhibits system sleep until the returned guard drops. Failure is logged
/// and inhibition is skipped: a conversion must never abort because power
/// management is unavailable (e.g. no D-Bus session bus on Linux).
pub(crate) fn inhibit_sleep() -> Option<keepawake::KeepAwake> {
    match keepawake::Builder::default()
        .idle(true)
        .reason("Converting video")
        .app_name("CRFty")
        .app_reverse_domain("io.github.loufe.crfty")
        .create()
    {
        Ok(guard) => Some(guard),
        Err(error) => {
            tracing::warn!("sleep inhibition unavailable; continuing without it: {error}");
            None
        }
    }
}
