//! OCR queue commands.

use tauri::State;

use crate::app::{OcrStatusView, internal_ipc};
use crate::domain::IpcError;
use crate::infrastructure::store::{OcrCandidateFilter, QueueDecision, Store, StoreError};
use crate::ocr::OcrEnqueuer;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrBatchResult {
    pub enqueued: u32,
    pub skipped: u32,
    pub failed: u32,
}

async fn run_batch(
    store: Store,
    enqueuer: OcrEnqueuer,
    filter: OcrCandidateFilter,
    limit: u32,
) -> Result<OcrBatchResult, IpcError> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut result = OcrBatchResult {
            enqueued: 0,
            skipped: 0,
            failed: 0,
        };
        for candidate in store.list_ocr_candidates(filter, limit)? {
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
        Ok::<_, StoreError>(result)
    })
    .await
    .map_err(internal_ipc)?
    .map_err(IpcError::from)
}

#[tauri::command]
pub async fn ocr_backfill(
    store: State<'_, Store>,
    enqueuer: State<'_, OcrEnqueuer>,
    limit: Option<u32>,
) -> Result<OcrBatchResult, IpcError> {
    run_batch(
        store.inner().clone(),
        enqueuer.inner().clone(),
        OcrCandidateFilter::None,
        limit.unwrap_or(50).clamp(1, 500),
    )
    .await
}

#[tauri::command]
pub async fn ocr_retry_failed(
    store: State<'_, Store>,
    enqueuer: State<'_, OcrEnqueuer>,
    limit: Option<u32>,
) -> Result<OcrBatchResult, IpcError> {
    run_batch(
        store.inner().clone(),
        enqueuer.inner().clone(),
        OcrCandidateFilter::Failed,
        limit.unwrap_or(20).clamp(1, 200),
    )
    .await
}

#[tauri::command]
pub async fn ocr_get_status(
    store: State<'_, Store>,
    clip_id: String,
) -> Result<OcrStatusView, IpcError> {
    let store = store.inner().clone();
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
