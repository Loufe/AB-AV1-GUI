use std::path::PathBuf;

use crfty_core::{
    AnalysisIntent, CorruptionSignature, Operation, OutputTarget, ProjectionCommand, QueueCommand,
    QueueItemEdit, QueueItemId, SessionCommand, Settings, VendorCommand,
};
use serde::{Deserialize, Serialize};
use tauri::{State, ipc::Channel};
use tauri_specta::{Builder, collect_commands};

use crate::bridge::{
    Bridge, CommandError, ImportSummary, ReleaseSummary, ScrubSummary, ShellEvent,
};

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AppInfo {
    pub version: String,
}

/// Narrow native-picker intents exposed to the webview. The selected path is
/// returned without granting general filesystem plugin access.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, specta::Type)]
pub enum PathPickerKind {
    File,
    Files,
    Folder,
    Folders,
    HistoryImport,
}

#[tauri::command]
#[specta::specta]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// Open a native picker off the webview boundary. Cancellation is an accepted
/// empty list; errors remain reserved for command/bridge failures.
#[tauri::command]
#[specta::specta]
async fn pick_paths(
    kind: PathPickerKind,
    starting_directory: Option<PathBuf>,
) -> Result<Vec<PathBuf>, CommandError> {
    let mut picker = rfd::AsyncFileDialog::new();
    if let Some(directory) = starting_directory {
        picker = picker.set_directory(directory);
    }
    let selected = match kind {
        PathPickerKind::File => picker
            .pick_file()
            .await
            .into_iter()
            .map(|handle| handle.path().to_owned())
            .collect(),
        PathPickerKind::Files => picker
            .pick_files()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|handle| handle.path().to_owned())
            .collect(),
        PathPickerKind::Folder => picker
            .pick_folder()
            .await
            .into_iter()
            .map(|handle| handle.path().to_owned())
            .collect(),
        PathPickerKind::Folders => picker
            .pick_folders()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|handle| handle.path().to_owned())
            .collect(),
        PathPickerKind::HistoryImport => picker
            .add_filter("CRFty history export", &["json"])
            .pick_file()
            .await
            .into_iter()
            .map(|handle| handle.path().to_owned())
            .collect(),
    };
    Ok(selected)
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

/// Atomically replace the complete pending Queue order. The reducer rejects
/// stale or incomplete permutations without exposing intermediate moves.
#[tauri::command]
#[specta::specta]
fn queue_reorder_pending(
    bridge: State<'_, Bridge>,
    pending_order: Vec<QueueItemId>,
) -> Result<(), CommandError> {
    bridge.submit_queue(QueueCommand::ReorderPending { pending_order })
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

/// Ask for a fresh Statistics computation. The ack only confirms acceptance;
/// the payload arrives as a sequenced `Statistics` ephemeral on the stream
/// and is never replayed — re-request after (re)subscribing.
#[tauri::command]
#[specta::specta]
fn request_statistics(
    bridge: State<'_, Bridge>,
    utc_offset_minutes: i32,
) -> Result<(), CommandError> {
    bridge.submit_projection(ProjectionCommand::RequestStatistics { utc_offset_minutes })
}

/// Import a history file produced by the V2 converter script
/// (`docs/HISTORY_IMPORT.md`). Records are parked durably and adopted as
/// matching files are prepared.
#[tauri::command]
#[specta::specta]
fn import_history(bridge: State<'_, Bridge>, path: PathBuf) -> Result<ImportSummary, CommandError> {
    bridge.import_history(&path)
}

/// Anonymize every existing log file in place (Settings tab "Scrub Logs").
/// Irreversible; runs even when the anonymize-logs toggle is off.
#[tauri::command]
#[specta::specta]
fn scrub_logs(bridge: State<'_, Bridge>) -> Result<ScrubSummary, CommandError> {
    bridge.scrub_logs()
}

/// One-shot manual check of the GitHub releases API (#33 §12: no background
/// checking exists). Async so the blocking network call never runs on the
/// main thread; the release page URL stays shell-side — open it with
/// `open_release_page`.
#[tauri::command]
#[specta::specta]
async fn check_for_update(bridge: State<'_, Bridge>) -> Result<ReleaseSummary, CommandError> {
    let slot = bridge.release_url_slot();
    tauri::async_runtime::spawn_blocking(move || Bridge::check_for_update(&slot))
        .await
        .map_err(|error| {
            CommandError::new(
                "update_check_failed",
                format!("the update check task failed: {error}"),
            )
        })?
}

/// Open the release page recorded by the last successful `check_for_update`.
#[tauri::command]
#[specta::specta]
fn open_release_page(bridge: State<'_, Bridge>) -> Result<(), CommandError> {
    bridge.open_release_page()
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

/// Opens a file or folder with the operating system's default program.
///
/// Which path to act on (input, converted output) is frontend state, so the
/// path arrives explicitly. No domain state is involved: the call goes
/// straight to the engine, bypassing the reducer. Declared async so the
/// desktop hand-off (which can stall on a misbehaving handler) runs off the
/// main thread.
#[tauri::command]
#[specta::specta]
async fn open_path(path: PathBuf) -> Result<(), CommandError> {
    crfty_engine::os_actions::open_path(&path).map_err(CommandError::from)
}

/// Reveals a path selected in the system file manager. Same contract as
/// [`open_path`].
#[tauri::command]
#[specta::specta]
async fn reveal_in_file_manager(path: PathBuf) -> Result<(), CommandError> {
    crfty_engine::os_actions::reveal_path(&path).map_err(CommandError::from)
}

/// The complete command/event surface, shared by the running app and the
/// bindings-export test so the two can never drift.
///
/// `HistoryRow` never crosses IPC — the frontend derives rows from the
/// snapshot with a mirror of `crfty_core::history_rows` — but its type is
/// exported so the mirror consumes the generated definition instead of
/// hand-authoring a domain type.
#[must_use]
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            app_info,
            pick_paths,
            subscribe,
            queue_add_paths,
            queue_remove,
            queue_move,
            queue_reorder_pending,
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
            request_statistics,
            import_history,
            scrub_logs,
            check_for_update,
            open_release_page,
            acknowledge_corruption,
            open_path,
            reveal_in_file_manager,
        ])
        .typ::<crfty_core::HistoryRow>()
}
