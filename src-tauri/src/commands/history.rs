//! History and payload read commands.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use tauri::State;

use crate::app::internal_ipc;
use crate::domain::{HistoryPage, IpcError, PayloadKind};
use crate::infrastructure::store::Store;

#[tauri::command]
pub fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
pub async fn history_page(
    store: State<'_, Store>,
    query: Option<String>,
    kind: Option<PayloadKind>,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<HistoryPage, IpcError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || match query {
        Some(query) => store.search_history_page(query, kind, cursor, limit),
        None if kind.is_some() => store.search_history_page(String::new(), kind, cursor, limit),
        None => store.history_page(cursor, limit),
    })
    .await
    .map_err(internal_ipc)?
    .map_err(IpcError::from)
}

#[tauri::command]
pub async fn image_payload_data_url(
    store: State<'_, Store>,
    content_hash: String,
) -> Result<String, IpcError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = store.read_payload_bytes(content_hash, PayloadKind::Image)?;
        Ok::<_, crate::infrastructure::store::StoreError>(format!(
            "data:image/png;base64,{}",
            BASE64.encode(bytes)
        ))
    })
    .await
    .map_err(internal_ipc)?
    .map_err(IpcError::from)
}
