//! App icon extraction: win-icon-extractor cache + in-memory data-URL cache.

use std::{collections::HashMap, path::PathBuf, sync::Mutex};

use base64::Engine;
use tauri::Manager;
use win_icon_extractor::{IconCache, ImageFormat};

use crate::domain::IpcError;

pub struct AppIconState {
    icon_cache: IconCache,
    /// exe_path → data:image/png;base64,...
    data_url_cache: Mutex<HashMap<String, String>>,
}

impl AppIconState {
    pub fn new(cache_dir: PathBuf) -> Result<Self, String> {
        let icon_cache = IconCache::builder(cache_dir)
            .format(ImageFormat::Png)
            .build()
            .map_err(|error| format!("init icon cache failed: {error}"))?;
        Ok(Self {
            icon_cache,
            data_url_cache: Mutex::new(HashMap::new()),
        })
    }
}

/// Returns a `data:image/png;base64,...` URL for the given executable.
#[tauri::command]
pub fn get_app_icon(
    state: tauri::State<'_, AppIconState>,
    exe_path: String,
) -> Result<String, IpcError> {
    let exe_path = exe_path.trim();
    if exe_path.is_empty() {
        return Err(IpcError {
            code: crate::domain::ErrorCode::InvalidArgument,
            message: Some("exe path is empty".into()),
            trace_id: None,
        });
    }

    if let Ok(cache) = state.data_url_cache.lock() {
        if let Some(data_url) = cache.get(exe_path) {
            return Ok(data_url.clone());
        }
    }

    let cached_file = state
        .icon_cache
        // 32px source renders crisply in the 16px list row.
        .extract_to_file_sized(exe_path, 32)
        .map_err(|error| IpcError {
            code: crate::domain::ErrorCode::NotFound,
            message: Some(format!("extract icon failed: {error}")),
            trace_id: None,
        })?;
    let png_bytes = std::fs::read(&cached_file).map_err(|error| IpcError {
        code: crate::domain::ErrorCode::Internal,
        message: Some(format!("read cached icon failed: {error}")),
        trace_id: None,
    })?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    let data_url = format!("data:image/png;base64,{encoded}");

    if let Ok(mut cache) = state.data_url_cache.lock() {
        cache.insert(exe_path.to_string(), data_url.clone());
    }
    Ok(data_url)
}

pub fn init_state(app: &tauri::AppHandle) -> Result<(), String> {
    let cache_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| format!("resolve icon cache dir failed: {error}"))?
        .join("icon_cache");
    app.manage(
        AppIconState::new(cache_dir).map_err(|error| format!("init icon state failed: {error}"))?,
    );
    Ok(())
}
