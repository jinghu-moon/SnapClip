//! Clipboard user-action commands.
//!
//! These are the only commands allowed to touch the Windows clipboard. The capture
//! feature never routes through here.

use tauri::State;

use crate::app::internal_ipc;
use crate::domain::{IpcError, PayloadKind};
use crate::infrastructure::store::{Store, StoreError};

#[cfg(not(windows))]
use crate::domain::ErrorCode;

#[cfg(windows)]
#[tauri::command]
pub async fn copy_payload(
    store: State<'_, Store>,
    content_hash: String,
    kind: PayloadKind,
) -> Result<(), IpcError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = store.read_payload_bytes(content_hash, kind.clone())?;
        match kind {
            PayloadKind::Image => {
                let decoded = crate::infrastructure::image::decode_to_rgba8(&bytes)
                    .map_err(StoreError::InvalidPublication)?;
                let (width, height) = decoded.dimensions();
                let mut clipboard = arboard::Clipboard::new().map_err(|error| {
                    StoreError::InvalidPublication(format!("clipboard unavailable: {error}"))
                })?;
                clipboard
                    .set_image(arboard::ImageData {
                        width: width as usize,
                        height: height as usize,
                        bytes: std::borrow::Cow::Owned(decoded.into_raw()),
                    })
                    .map_err(|error| {
                        StoreError::InvalidPublication(format!("copy image failed: {error}"))
                    })?;
            }
            PayloadKind::Text | PayloadKind::Html | PayloadKind::Rtf | PayloadKind::Other => {
                let text = String::from_utf8(bytes).map_err(|error| {
                    StoreError::InvalidPublication(format!("text decode failed: {error}"))
                })?;
                let mut clipboard = arboard::Clipboard::new().map_err(|error| {
                    StoreError::InvalidPublication(format!("clipboard unavailable: {error}"))
                })?;
                clipboard.set_text(text).map_err(|error| {
                    StoreError::InvalidPublication(format!("copy text failed: {error}"))
                })?;
            }
            PayloadKind::Files => {
                return Err(StoreError::InvalidPublication(
                    "file payload copy is not supported yet".into(),
                ));
            }
        }
        // Mark the content so the monitor does not capture SnapClip's own write.
        crate::platform::windows::clipboard::mark_clipboard_excluded();
        Ok::<_, StoreError>(())
    })
    .await
    .map_err(internal_ipc)?
    .map_err(IpcError::from)
}

#[cfg(not(windows))]
#[tauri::command]
pub async fn copy_payload(
    _store: State<'_, Store>,
    _content_hash: String,
    _kind: PayloadKind,
) -> Result<(), IpcError> {
    Err(IpcError::new(
        ErrorCode::Unsupported,
        "clipboard copy is only available on Windows",
    ))
}

#[cfg(windows)]
#[tauri::command]
pub fn get_app_icon(
    state: tauri::State<'_, crate::icon::AppIconState>,
    exe_path: String,
) -> Result<String, IpcError> {
    crate::icon::get_app_icon(state.inner(), exe_path)
}

#[cfg(not(windows))]
#[tauri::command]
pub fn get_app_icon(_exe_path: String) -> Result<String, IpcError> {
    Err(IpcError::new(
        ErrorCode::Unsupported,
        "app icons are only available on Windows",
    ))
}
