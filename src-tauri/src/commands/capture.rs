//! Capture commands.
//!
//! Low frequency by design: `F5`, `Esc` and `Enter` are handled by the native
//! overlay. These commands exist so the future toolbar and the status panel can drive
//! the same state machine without touching the hot path.

use tauri::State;

use snapclip_capture::annotation::AnnotationCommand;
use snapclip_capture::CaptureState;
use crate::domain::IpcError;

#[cfg(not(windows))]
use crate::domain::ErrorCode;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStateView {
    pub state: CaptureState,
}

#[cfg(windows)]
type RuntimeState<'a> =
    State<'a, std::sync::Arc<snapclip_capture::runtime::CaptureRuntime>>;

#[cfg(windows)]
fn runtime(
    state: &RuntimeState<'_>,
) -> std::sync::Arc<snapclip_capture::runtime::CaptureRuntime> {
    state.inner().clone()
}

/// Start a capture session. Returns `false` when one is already running.
#[cfg(windows)]
#[tauri::command]
pub fn capture_start(runtime_state: RuntimeState<'_>) -> Result<bool, IpcError> {
    runtime(&runtime_state)
        .start_capture()
        .map_err(IpcError::from)
}

#[cfg(windows)]
#[tauri::command]
pub fn capture_cancel(runtime_state: RuntimeState<'_>) -> Result<(), IpcError> {
    runtime(&runtime_state)
        .cancel_capture()
        .map_err(IpcError::from)
}

#[cfg(windows)]
#[tauri::command]
pub fn capture_confirm(runtime_state: RuntimeState<'_>) -> Result<(), IpcError> {
    runtime(&runtime_state)
        .confirm_capture()
        .map_err(IpcError::from)
}

#[cfg(windows)]
#[tauri::command]
pub fn capture_state(runtime_state: RuntimeState<'_>) -> CaptureStateView {
    CaptureStateView {
        state: runtime(&runtime_state).state(),
    }
}

/// Forward one coarse toolbar annotation command into the overlay's annotation
/// mailbox. Low frequency by design — a single call per toolbar click, never per
/// mouse move or pixel frame (docs/11 §7.1 "工具栏不进入像素管线").
#[cfg(windows)]
#[tauri::command]
pub fn capture_annotation(
    runtime_state: RuntimeState<'_>,
    command: AnnotationCommand,
) -> Result<(), IpcError> {
    runtime(&runtime_state)
        .annotation_command(command)
        .map_err(IpcError::from)
}

#[cfg(not(windows))]
#[tauri::command]
pub fn capture_start() -> Result<bool, IpcError> {
    Err(unsupported())
}

#[cfg(not(windows))]
#[tauri::command]
pub fn capture_cancel() -> Result<(), IpcError> {
    Err(unsupported())
}

#[cfg(not(windows))]
#[tauri::command]
pub fn capture_confirm() -> Result<(), IpcError> {
    Err(unsupported())
}

#[cfg(not(windows))]
#[tauri::command]
pub fn capture_state() -> CaptureStateView {
    CaptureStateView {
        state: CaptureState::Idle,
    }
}

#[cfg(not(windows))]
#[tauri::command]
pub fn capture_annotation(_command: AnnotationCommand) -> Result<(), IpcError> {
    Err(unsupported())
}

#[cfg(not(windows))]
fn unsupported() -> IpcError {
    IpcError::new(
        ErrorCode::Unsupported,
        "capture is only available on Windows",
    )
}
