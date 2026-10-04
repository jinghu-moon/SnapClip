//! Application composition root: builds the adapters, wires the services and hands
//! them to Tauri's managed state.
//!
//! Dependency direction at this layer:
//!
//! ```text
//! platform/windows/clipboard ─┐
//! platform/windows/capture   ─┼─> application services ─> domain/store
//! Tauri commands/events      ─┘
//! ```
//!
//! Nothing below this module knows that Tauri exists.

#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod clipboard;
pub mod ocr_queue;

use crate::domain::{IpcError, OcrErrorCode, OcrStatus};

/// Shared helper for command adapters that hit an internal failure.
pub fn internal_ipc(error: impl std::fmt::Display) -> IpcError {
    IpcError::new(crate::domain::ErrorCode::Internal, error.to_string())
}

/// Small view model for the OCR status command.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrStatusView {
    pub status: OcrStatus,
    pub engine: Option<String>,
    pub updated_at: Option<i64>,
    pub error_code: Option<OcrErrorCode>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_started = std::time::Instant::now();

    // DPI awareness has to be declared before anything creates a window or reads a
    // cursor position; once the Tauri window exists the declaration can no longer be
    // changed, and capture geometry would be reported in virtualised coordinates
    // (docs/14 §3). Doing it here means the overlay thread's own call is a no-op that
    // simply reports the context already in effect.
    #[cfg(windows)]
    match crate::platform::windows::capture::monitor::set_per_monitor_v2_awareness() {
        Ok(mode) => eprintln!("[snapclip][startup] dpi awareness={mode}"),
        Err(message) => {
            eprintln!("[snapclip][startup] dpi awareness declaration failed: {message}")
        }
    }

    tauri::Builder::default()
        .on_page_load(move |webview, payload| {
            eprintln!(
                "[snapclip][startup] webview page_load label={} event={:?} elapsed_ms={}",
                webview.label(),
                payload.event(),
                app_started.elapsed().as_millis()
            );
        })
        .setup(|app| {
            use tauri::Manager;
            let startup = std::time::Instant::now();
            eprintln!("[snapclip][startup] setup begin");

            let root = app.path().app_local_data_dir()?;
            let store = crate::infrastructure::store::Store::open(&root)
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
            eprintln!(
                "[snapclip][startup] store ready elapsed_ms={}",
                startup.elapsed().as_millis()
            );

            #[cfg(windows)]
            crate::icon::init_state(app.handle())?;
            eprintln!(
                "[snapclip][startup] icon state ready elapsed_ms={}",
                startup.elapsed().as_millis()
            );

            let engine = std::sync::Arc::new(crate::ocr::OcrManager::new(
                root.join("models/ocr/ppocrv6-medium"),
            )) as std::sync::Arc<dyn crate::ocr::OcrEngine>;
            let ocr = crate::ocr::OcrService::start(store.clone(), app.handle().clone(), engine);
            let enqueuer = ocr.enqueuer();
            eprintln!(
                "[snapclip][startup] ocr worker started elapsed_ms={}",
                startup.elapsed().as_millis()
            );

            app.manage(store.clone());
            app.manage(enqueuer.clone());
            // The service stops its worker when dropped, so it is kept alive for the
            // whole process lifetime behind an `Arc`.
            app.manage(std::sync::Arc::new(ocr));

            #[cfg(not(windows))]
            let _ = &root;

            #[cfg(windows)]
            capture::start(app.handle().clone(), &root)?;
            eprintln!(
                "[snapclip][startup] capture overlay ready elapsed_ms={}",
                startup.elapsed().as_millis()
            );

            // The handle owns the listener and the ingest worker. It must outlive
            // `setup`: letting it drop here would stop the pipeline and block on
            // `join`, freezing the app before its window can paint.
            #[cfg(windows)]
            app.manage(clipboard::start(app.handle().clone(), store, enqueuer)?);
            eprintln!(
                "[snapclip][startup] clipboard pipeline ready elapsed_ms={}",
                startup.elapsed().as_millis()
            );

            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            crate::commands::history::greet,
            crate::commands::history::history_page,
            crate::commands::history::image_payload_data_url,
            crate::commands::clipboard::copy_payload,
            crate::commands::clipboard::get_app_icon,
            crate::commands::ocr::ocr_backfill,
            crate::commands::ocr::ocr_retry_failed,
            crate::commands::ocr::ocr_get_status,
            crate::commands::capture::capture_start,
            crate::commands::capture::capture_cancel,
            crate::commands::capture::capture_confirm,
            crate::commands::capture::capture_state,
            crate::commands::capture::capture_annotation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
