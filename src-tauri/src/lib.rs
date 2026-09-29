pub mod domain;
#[cfg(windows)]
mod icon;
#[cfg(windows)]
mod platform;
pub mod store;

use tauri::Manager;

use crate::{
    domain::{HistoryPage, IpcError, PayloadKind},
    store::Store,
};

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

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let root = app.path().app_local_data_dir()?;
            app.manage(
                Store::open(root).map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?,
            );
            #[cfg(windows)]
            icon::init_state(app.handle())?;
            #[cfg(windows)]
            app.manage(platform::windows::clipboard::ClipboardMonitor::start(
                app.state::<Store>().inner().clone(),
            )?);
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet, history_page, get_app_icon])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
