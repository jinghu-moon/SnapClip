pub mod domain;
#[cfg(windows)]
mod icon;
mod ocr;
#[cfg(windows)]
mod platform;
pub mod store;

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use tauri::Manager;

use crate::{
    domain::{HistoryPage, IpcError, OcrErrorCode, OcrStatus, PayloadKind},
    ocr::{OcrService, OcrServiceHandle, WindowsOcrEngine},
    store::{OcrCandidateFilter, QueueDecision, Store},
};

#[cfg(windows)]
use std::borrow::Cow;

#[tauri::command]
async fn history_page(
    state: tauri::State<'_, Store>,
    query: Option<String>,
    kind: Option<PayloadKind>,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<HistoryPage, IpcError> {
    let store = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || match query {
        Some(query) => store.search_history_page(query, kind, cursor, limit),
        None if kind.is_some() => store.search_history_page(String::new(), kind, cursor, limit),
        None => store.history_page(cursor, limit),
    })
    .await
    .map_err(|error| IpcError {
        code: crate::domain::ErrorCode::Internal,
        message: Some(error.to_string()),
        trace_id: None,
    })?
    .map_err(IpcError::from)
}

#[tauri::command]
async fn image_payload_data_url(
    state: tauri::State<'_, Store>,
    content_hash: String,
) -> Result<String, IpcError> {
    let store = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = store.read_payload_bytes(content_hash, PayloadKind::Image)?;
        Ok::<_, crate::store::StoreError>(format!("data:image/png;base64,{}", BASE64.encode(bytes)))
    })
    .await
    .map_err(internal_ipc)?
    .map_err(IpcError::from)
}

#[cfg(windows)]
#[tauri::command]
async fn copy_payload(
    state: tauri::State<'_, Store>,
    content_hash: String,
    kind: PayloadKind,
) -> Result<(), IpcError> {
    let store = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = store.read_payload_bytes(content_hash, kind.clone())?;
        match kind {
            PayloadKind::Image => {
                let decoded = image::load_from_memory(&bytes)
                    .map_err(|error| {
                        crate::store::StoreError::InvalidPublication(format!(
                            "image decode failed: {error}"
                        ))
                    })?
                    .to_rgba8();
                let (width, height) = decoded.dimensions();
                let mut clipboard = arboard::Clipboard::new().map_err(|error| {
                    crate::store::StoreError::InvalidPublication(format!(
                        "clipboard unavailable: {error}"
                    ))
                })?;
                clipboard
                    .set_image(arboard::ImageData {
                        width: width as usize,
                        height: height as usize,
                        bytes: Cow::Owned(decoded.into_raw()),
                    })
                    .map_err(|error| {
                        crate::store::StoreError::InvalidPublication(format!(
                            "copy image failed: {error}"
                        ))
                    })?;
            }
            PayloadKind::Text | PayloadKind::Html | PayloadKind::Rtf | PayloadKind::Other => {
                let text = String::from_utf8(bytes).map_err(|error| {
                    crate::store::StoreError::InvalidPublication(format!(
                        "text decode failed: {error}"
                    ))
                })?;
                let mut clipboard = arboard::Clipboard::new().map_err(|error| {
                    crate::store::StoreError::InvalidPublication(format!(
                        "clipboard unavailable: {error}"
                    ))
                })?;
                clipboard.set_text(text).map_err(|error| {
                    crate::store::StoreError::InvalidPublication(format!(
                        "copy text failed: {error}"
                    ))
                })?;
            }
            PayloadKind::Files => {
                return Err(crate::store::StoreError::InvalidPublication(
                    "file payload copy is not supported yet".into(),
                ));
            }
        }
        crate::platform::windows::clipboard::mark_clipboard_excluded();
        Ok::<_, crate::store::StoreError>(())
    })
    .await
    .map_err(internal_ipc)?
    .map_err(IpcError::from)
}

#[cfg(not(windows))]
#[tauri::command]
async fn copy_payload(
    _state: tauri::State<'_, Store>,
    _content_hash: String,
    _kind: PayloadKind,
) -> Result<(), IpcError> {
    Err(IpcError {
        code: crate::domain::ErrorCode::Unsupported,
        message: Some("clipboard copy is only available on Windows".into()),
        trace_id: None,
    })
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OcrBatchResult {
    enqueued: u32,
    skipped: u32,
    failed: u32,
}

#[tauri::command]
async fn ocr_backfill(
    state: tauri::State<'_, Store>,
    ocr: tauri::State<'_, OcrServiceHandle>,
    limit: Option<u32>,
) -> Result<OcrBatchResult, IpcError> {
    let store = state.inner().clone();
    let enqueuer = ocr.enqueuer();
    tauri::async_runtime::spawn_blocking(move || {
        let limit = limit.unwrap_or(50).clamp(1, 500);
        let mut result = OcrBatchResult {
            enqueued: 0,
            skipped: 0,
            failed: 0,
        };
        for candidate in store.list_ocr_candidates(OcrCandidateFilter::None, limit)? {
            match store.enqueue_ocr(candidate.clip_id.clone(), candidate.content_hash.clone())? {
                QueueDecision::Enqueued { attempt } => {
                    if enqueuer.try_enqueue(&candidate.clip_id, &candidate.content_hash) {
                        result.enqueued += 1;
                    } else {
                        let _ = store.release_queued(candidate.clip_id, attempt);
                        result.skipped += 1;
                    }
                }
                QueueDecision::AlreadyPending | QueueDecision::NotImage => {
                    result.skipped += 1;
                }
                QueueDecision::NotFound => result.failed += 1,
            }
        }
        Ok::<_, crate::store::StoreError>(result)
    })
    .await
    .map_err(internal_ipc)?
    .map_err(IpcError::from)
}

#[tauri::command]
async fn ocr_retry_failed(
    state: tauri::State<'_, Store>,
    ocr: tauri::State<'_, OcrServiceHandle>,
    limit: Option<u32>,
) -> Result<OcrBatchResult, IpcError> {
    let store = state.inner().clone();
    let enqueuer = ocr.enqueuer();
    tauri::async_runtime::spawn_blocking(move || {
        let limit = limit.unwrap_or(20).clamp(1, 200);
        let mut result = OcrBatchResult {
            enqueued: 0,
            skipped: 0,
            failed: 0,
        };
        for candidate in store.list_ocr_candidates(OcrCandidateFilter::Failed, limit)? {
            match store.enqueue_ocr(candidate.clip_id.clone(), candidate.content_hash.clone())? {
                QueueDecision::Enqueued { attempt } => {
                    if enqueuer.try_enqueue(&candidate.clip_id, &candidate.content_hash) {
                        result.enqueued += 1;
                    } else {
                        let _ = store.release_queued(candidate.clip_id, attempt);
                        result.skipped += 1;
                    }
                }
                QueueDecision::AlreadyPending | QueueDecision::NotImage => {
                    result.skipped += 1;
                }
                QueueDecision::NotFound => result.failed += 1,
            }
        }
        Ok::<_, crate::store::StoreError>(result)
    })
    .await
    .map_err(internal_ipc)?
    .map_err(IpcError::from)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OcrStatusView {
    status: OcrStatus,
    engine: Option<String>,
    updated_at: Option<i64>,
    error_code: Option<OcrErrorCode>,
}

#[tauri::command]
async fn ocr_get_status(
    state: tauri::State<'_, Store>,
    clip_id: String,
) -> Result<OcrStatusView, IpcError> {
    let store = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.ocr_status_of(&clip_id))
        .await
        .map_err(internal_ipc)?
        .map(|info| OcrStatusView {
            status: info.status,
            engine: info.engine,
            updated_at: info.updated_at,
            error_code: info.error_code,
        })
        .map_err(IpcError::from)
}

#[cfg(windows)]
use icon::get_app_icon;

#[cfg(not(windows))]
#[tauri::command]
fn get_app_icon(_exe_path: String) -> Result<String, IpcError> {
    Err(IpcError {
        code: crate::domain::ErrorCode::Unsupported,
        message: Some("app icons are only available on Windows".into()),
        trace_id: None,
    })
}

fn internal_ipc(error: impl std::fmt::Display) -> IpcError {
    IpcError {
        code: crate::domain::ErrorCode::Internal,
        message: Some(error.to_string()),
        trace_id: None,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let root = app.path().app_local_data_dir()?;
            let store =
                Store::open(root).map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
            app.manage(store.clone());
            #[cfg(windows)]
            icon::init_state(app.handle())?;

            let engine = Arc::new(WindowsOcrEngine::new()) as Arc<dyn ocr::OcrEngine>;
            let ocr_handle = OcrService::start(store.clone(), app.handle().clone(), engine);
            let enqueuer = ocr_handle.enqueuer();
            app.manage(ocr_handle);

            #[cfg(windows)]
            app.manage(platform::windows::clipboard::ClipboardMonitor::start(
                store,
                Some(enqueuer),
                app.handle().clone(),
            )?);
            #[cfg(not(windows))]
            drop(enqueuer);
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            history_page,
            image_payload_data_url,
            copy_payload,
            get_app_icon,
            ocr_backfill,
            ocr_retry_failed,
            ocr_get_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
