//! Capture, hosted by the GPUI shell (docs/23 P6).
//!
//! This module is the composition root for capture: it implements the ports
//! (`snapclip_capture::ports`) that the capture crate declares — the event sink and the
//! artifact writer — and starts the overlay. There is no second shell; the GPUI app in
//! `apps/snapclip` is the only process that hosts capture.

pub mod artifact_writer;

pub use artifact_writer::HistoryArtifactWriter;

/// The scroll export port's encoder (`docs/31` P4.02, `docs/30` §17.7).
///
/// It lives in the shell and not in `snapclip-capture` because which encoder to use is a decision
/// about the **output format**, not a fact about capturing — and that direction is what keeps
/// `png` out of the capture crate's graph entirely.
pub mod row_band_png;

use std::path::Path;
use std::sync::Arc;

use snapclip_capture::artifact::CaptureService;
use snapclip_capture::ports::{ArtifactWriter, CaptureEventSink, ClipboardWriter};
use snapclip_capture::runtime::CaptureRuntime;
use snapclip_capture::window_detection::DetectionOptions;

use crate::adapters::CaptureEvents;
use crate::clipboard::SystemClipboardWriter;
use crate::events::EventBus;

/// Start the overlay and hand back the runtime the shell keeps for the session.
///
/// F5 is registered by the overlay itself (`RegisterHotKey` on its own thread), so hosting
/// capture is what gives the shell its screenshot entry point.
///
/// `options` comes from the settings file, which is how a user preference reaches the window
/// walker; the overlay snapshots it at startup rather than re-reading on every frame.
pub fn start(
    app_local_data: &Path,
    events: EventBus,
    options: DetectionOptions,
) -> Result<CaptureRuntime, String> {
    // Per-Monitor V2 has to be declared before any window exists; the caller does that at the
    // very start of `run`. The overlay inherits the process context, so this only checks it.
    snapclip_capture::windows::monitor::set_per_monitor_v2_awareness()
        .map_err(|message| format!("DPI awareness: {message}"))?;

    let service = Arc::new(CaptureService::new());
    let sink: Arc<dyn CaptureEventSink> = Arc::new(CaptureEvents::new(events));
    let clipboard: Arc<dyn ClipboardWriter> = Arc::new(SystemClipboardWriter);
    // Artifacts land in `<app local data>/artifacts/capture` (docs/11 §8.2); the writer is
    // the only thing that knows how to turn pixels into a file.
    let writer: Arc<dyn ArtifactWriter> = Arc::new(HistoryArtifactWriter::new(
        app_local_data.join("artifacts").join("capture"),
    ));

    let runtime = snapclip_capture::windows::overlay::WindowsOverlay::spawn_overlay(
        service, sink, clipboard, writer, options,
    )
    .map_err(|message| format!("capture overlay: {message}"))?;
    Ok(CaptureRuntime::from_platform(Box::new(runtime)))
}

#[cfg(test)]
mod tests {
    use super::start;
    use crate::events::EventBus;
    use snapclip_capture::window_detection::DetectionOptions;
    use snapclip_model::CaptureState;

    /// The shell can host capture: the overlay is created, F5 is registered by it, and the
    /// runtime goes away cleanly.
    ///
    /// Ignored by default: it takes the F5 hotkey and creates a real overlay window, which is
    /// fine on a desktop session and wrong in a batch run. Run with
    /// `cargo test -p snapclip-app --lib -- --ignored` (the machine must not already be running
    /// a SnapClip shell, or the hotkey is taken).
    #[test]
    #[ignore = "takes the F5 hotkey and creates a real overlay window"]
    fn the_shell_can_host_the_capture_overlay() {
        let root = std::env::temp_dir().join(format!(
            "snapclip-app-capture-host-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let runtime = start(&root, EventBus::new(), DetectionOptions::default())
            .expect("the overlay should start on a desktop session");
        // Nothing is being selected until the user asks, so the honest state is Idle.
        assert_eq!(runtime.state(), CaptureState::Idle);
        // Dropping the runtime shuts the overlay down (its own `Drop`), including the hotkey.
        drop(runtime);
        let _ = std::fs::remove_dir_all(&root);
    }
}
