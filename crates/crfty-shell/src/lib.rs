//! CRFty's Tauri shell: IPC and application wiring only (ADR-001, ADR-006).
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)
)]

mod bridge;
mod commands;

pub use commands::specta_builder;

/// Runs the desktop application, exiting the process on startup failure so a
/// broken shell never lingers headless.
pub fn run() {
    let specta = specta_builder();
    let built = tauri::Builder::default()
        .invoke_handler(specta.invoke_handler())
        .setup(|app| {
            // A failed engine start degrades the bridge rather than aborting:
            // the window still opens and reports why nothing can run.
            let bridge = bridge::Bridge::start(app.handle());
            tauri::Manager::manage(app, bridge);
            Ok(())
        })
        .build(tauri::generate_context!());
    match built {
        Ok(app) => app.run(|app_handle, event| match event {
            // An active session makes closing a question, not an order
            // (#33 §12): the bridge defers the close to the frontend prompt,
            // which re-issues it once the session is idle.
            tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                if let Some(bridge) = tauri::Manager::try_state::<bridge::Bridge>(app_handle)
                    && bridge.handle_close_requested()
                {
                    api.prevent_close();
                }
            }
            // The event loop exits the process rather than unwinding, so
            // managed state is never dropped: this is the one place the
            // engine's threads are joined and the crash sentinel disarmed.
            tauri::RunEvent::Exit => {
                if let Some(bridge) = tauri::Manager::try_state::<bridge::Bridge>(app_handle) {
                    bridge.shutdown_engine();
                }
            }
            _ => {}
        }),
        Err(error) => {
            eprintln!("failed to start CRFty: {error}");
            std::process::exit(1);
        }
    }
}
