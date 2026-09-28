pub mod domain;
pub mod store;

use tauri::Manager;

use crate::{
    domain::{HistoryPage, IpcError},
    store::Store,
};

#[tauri::command]
async fn history_page(
    state: tauri::State<'_, Store>,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<HistoryPage, IpcError> {
    let store = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.history_page(cursor, limit))
        .await
        .map_err(|error| IpcError {
            code: crate::domain::ErrorCode::Internal,
            message: Some(error.to_string()),
            trace_id: None,
        })?
        .map_err(IpcError::from)
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let root = app.path().app_local_data_dir()?;
            app.manage(
                Store::open(root).map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?,
            );
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet, history_page])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
