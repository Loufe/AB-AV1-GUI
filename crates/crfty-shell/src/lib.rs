//! CRFty's Tauri shell: IPC and application wiring only (ADR-001, ADR-006).
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)
)]

mod commands;

pub use commands::specta_builder;

/// Runs the desktop application, exiting the process on startup failure so a
/// broken shell never lingers headless.
pub fn run() {
    let specta = specta_builder();
    let built = tauri::Builder::default()
        .invoke_handler(specta.invoke_handler())
        .build(tauri::generate_context!());
    match built {
        Ok(app) => app.run(|_app_handle, _event| {}),
        Err(error) => {
            eprintln!("failed to start CRFty: {error}");
            std::process::exit(1);
        }
    }
}
