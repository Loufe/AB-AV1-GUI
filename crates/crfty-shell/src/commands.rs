use std::path::PathBuf;

use crfty_core::{Operation, OutputTarget, QueueCommand, QueueItemId, SessionCommand, Settings};
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

#[tauri::command]
#[specta::specta]
fn queue_add(
    bridge: State<'_, Bridge>,
    input: PathBuf,
    operation: Operation,
    output_target: OutputTarget,
) -> Result<(), CommandError> {
    let item_id = bridge.allocate_item_id();
    bridge.submit_queue(QueueCommand::Add {
        item_id,
        input,
        operation,
        output_target,
    })
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

/// The complete command/event surface, shared by the running app and the
/// bindings-export test so the two can never drift.
#[must_use]
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![
        app_info,
        subscribe,
        queue_add,
        queue_remove,
        queue_move,
        start,
        stop_after_current,
        force_stop,
        set_settings,
    ])
}
