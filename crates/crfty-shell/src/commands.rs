use std::path::PathBuf;

use crfty_core::{
    AnalysisIntent, CorruptionSignature, Operation, OutputTarget, QueueCommand, QueueItemEdit,
    QueueItemId, SessionCommand, Settings, VendorCommand,
};
use serde::Serialize;
use tauri::{State, ipc::Channel};
use tauri_specta::{Builder, collect_commands};

use crate::bridge::{Bridge, CommandError, ShellEvent};

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AppInfo {
    pub version: String,
}

#[tauri::command]
#[specta::specta]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

#[tauri::command]
#[specta::specta]
fn subscribe(bridge: State<'_, Bridge>, channel: Channel<ShellEvent>) -> Result<(), CommandError> {
    bridge.subscribe(channel);
    Ok(())
}

/// Adds files and folders in one batch: folders expand through the engine
/// scanner (filtered by the configured scan extensions), directly selected
/// files pass through unfiltered. The outcome arrives as one
/// `QueueAddSummary` on the stream.
#[tauri::command]
#[specta::specta]
fn queue_add_paths(
    bridge: State<'_, Bridge>,
    inputs: Vec<PathBuf>,
    operation: Operation,
    intent: AnalysisIntent,
    output_target: OutputTarget,
) -> Result<(), CommandError> {
    bridge.queue_add_paths(inputs, operation, intent, output_target)
}

#[tauri::command]
#[specta::specta]
fn queue_remove(bridge: State<'_, Bridge>, item_id: QueueItemId) -> Result<(), CommandError> {
    bridge.submit_queue(QueueCommand::Remove { item_id })
}

#[tauri::command]
#[specta::specta]
fn queue_move(
    bridge: State<'_, Bridge>,
    item_id: QueueItemId,
    before: Option<QueueItemId>,
) -> Result<(), CommandError> {
    bridge.submit_queue(QueueCommand::Move { item_id, before })
}

#[tauri::command]
#[specta::specta]
fn queue_clear(bridge: State<'_, Bridge>) -> Result<(), CommandError> {
    bridge.submit_queue(QueueCommand::Clear)
}

#[tauri::command]
#[specta::specta]
fn queue_clear_completed(bridge: State<'_, Bridge>) -> Result<(), CommandError> {
    bridge.submit_queue(QueueCommand::ClearCompleted)
}

#[tauri::command]
#[specta::specta]
fn queue_retry(bridge: State<'_, Bridge>, item_id: QueueItemId) -> Result<(), CommandError> {
    bridge.submit_queue(QueueCommand::Retry { item_id })
}

#[tauri::command]
#[specta::specta]
fn queue_edit(
    bridge: State<'_, Bridge>,
    item_id: QueueItemId,
    patch: QueueItemEdit,
) -> Result<(), CommandError> {
    bridge.submit_queue(QueueCommand::Edit { item_id, patch })
}

#[tauri::command]
#[specta::specta]
fn start(bridge: State<'_, Bridge>) -> Result<(), CommandError> {
    bridge.submit_session(SessionCommand::Start)
}

#[tauri::command]
#[specta::specta]
fn stop_after_current(bridge: State<'_, Bridge>) -> Result<(), CommandError> {
    bridge.submit_session(SessionCommand::StopAfterCurrent)
}

#[tauri::command]
#[specta::specta]
fn force_stop(bridge: State<'_, Bridge>) -> Result<(), CommandError> {
    bridge.submit_session(SessionCommand::ForceStop)
}

#[tauri::command]
#[specta::specta]
fn set_settings(bridge: State<'_, Bridge>, settings: Settings) -> Result<(), CommandError> {
    bridge.submit_settings(settings)
}

#[tauri::command]
#[specta::specta]
fn vendor_install(bridge: State<'_, Bridge>) -> Result<(), CommandError> {
    bridge.submit_vendor(VendorCommand::Install)
}

#[tauri::command]
#[specta::specta]
fn vendor_check(bridge: State<'_, Bridge>) -> Result<(), CommandError> {
    bridge.submit_vendor(VendorCommand::Check)
}

/// Consent to discard a corrupt journal tail. The signature must echo the
/// one delivered on the `Degraded` payload — the driver rejects anything
/// else, so a stale acknowledgement can never discard fresher bytes.
#[tauri::command]
#[specta::specta]
fn acknowledge_corruption(
    bridge: State<'_, Bridge>,
    signature: CorruptionSignature,
) -> Result<(), CommandError> {
    bridge.acknowledge_corruption(signature)
}

/// The complete command/event surface, shared by the running app and the
/// bindings-export test so the two can never drift.
#[must_use]
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![
        app_info,
        subscribe,
        queue_add_paths,
        queue_remove,
        queue_move,
        queue_clear,
        queue_clear_completed,
        queue_retry,
        queue_edit,
        start,
        stop_after_current,
        force_stop,
        set_settings,
        vendor_install,
        vendor_check,
        acknowledge_corruption,
    ])
}
