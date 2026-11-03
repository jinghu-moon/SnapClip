//! Capture, hosted by the GPUI shell (docs/23 P6).
//!
//! This module is the composition root for capture: it implements the ports
//! (`snapclip_capture::ports`) that the capture crate declares — the event sink and the
//! artifact writer — and starts the overlay. There is no second shell; the GPUI app in
//! `apps/snapclip` is the only process that hosts capture.

pub mod artifact_writer;

pub use artifact_writer::HistoryArtifactWriter;

/// Two-phase screenshot handoff: reserve first, consume after destination creation.
pub mod pin_source;

pub use pin_source::{PinSource, SelectionProvider};

pub mod pixel_export;
/// The scroll export port's encoder (`docs/31` P4.02, `docs/30` §17.7).
///
/// It lives in the shell and not in `snapclip-capture` because which encoder to use is a decision
/// about the **output format**, not a fact about capturing — and that direction is what keeps
/// `png` out of the capture crate's graph entirely.
pub mod row_band_bmp;
pub mod row_band_png;

/// What each save format can hold, as pure data (`docs/34` §13).
///
/// The scroll session needs this before it starts scrolling, so it is a value type with no encoder
/// behind it: asking "can WebP hold 30000 rows" must not open a codec.
// The capability model lives in `snapclip-capture` (the scroll session needs it before it can
// scroll); the encoders stay here. Re-exported so the sinks keep reading `super::save_format`.
pub use snapclip_capture::save_format;

/// Lossy export, split by what the Huffman optimiser costs in memory (`docs/34` §10–11).
///
/// Beside `row_band_png` rather than inside it: the two sinks share `ImageMeta`, `Artifact` and
/// `ExportError` from there, and nothing else.
pub mod row_band_jpeg;

/// Lossless WebP export (`docs/34` §9.1).
///
/// Whole-image, because `zenwebp` has no streaming form, and lossless only, because lossy WebP is
/// 4:2:0 by definition and that is what damages coloured text.
pub mod webp_sink;

/// Saving in a format other than PNG: the master is the contract, the conversion is derived
/// from it (`docs/34` §16).
///
/// It decodes row by row, so the JPEG sink decides between buffering the frame for the
/// Huffman optimiser and streaming band by band, and a scroll save never materialises the
/// 153 MiB frame.
pub mod convert;

pub use convert::{Conversion, convert, convert_to_file};

use std::path::Path;
use std::sync::{Arc, Mutex};

use snapclip_capture::artifact::CaptureService;
use snapclip_capture::ports::{ArtifactWriter, CaptureEventSink, ClipboardWriter};
use snapclip_capture::preferences::OverlayPreferences;
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
/// The returned [`PinSource`] is the shell's handle on "the picture the user just captured":
/// the writer remembers every selection whose screen position is known, and F6 (handled by
/// the pin host thread) takes it from there. It is returned rather than pushed somewhere
/// because the shell is the one that has to hand it to the pin host — this function is not
/// the place that knows pins exist.
///
/// `options` comes from the settings file, which is how a user preference reaches the window
/// walker; the overlay snapshots it at startup rather than re-reading on every frame.
/// `preferences` are the overlay's presentation and keys; later changes go through
/// `CaptureRuntime::set_preferences`.
pub fn start(
    app_local_data: &Path,
    history: Option<snapclip_history::store::Store>,
    events: EventBus,
    options: DetectionOptions,
    preferences: OverlayPreferences,
) -> Result<(Arc<CaptureRuntime>, Arc<PinSource>, Arc<CaptureEvents>), String> {
    // Per-Monitor V2 has to be declared before any window exists; the caller does that at the
    // very start of `run`. The overlay inherits the process context, so this only checks it.
    snapclip_capture::windows::monitor::set_per_monitor_v2_awareness()
        .map_err(|message| format!("DPI awareness: {message}"))?;

    let service = Arc::new(CaptureService::new());
    // 事件出口留一份具体类型在外面：延时截图到点时要给它接上"怎么贴图"，而那件事
    // 只有外壳知道（贴图管理器在半路上才起来，docs/23 §14.55.8）。
    let capture_events = Arc::new(CaptureEvents::new(events));
    let sink: Arc<dyn CaptureEventSink> = capture_events.clone();
    let clipboard: Arc<dyn ClipboardWriter> = Arc::new(SystemClipboardWriter);
    // Artifacts land in `<app local data>/artifacts/capture` (docs/11 §8.2); the writer is
    // the only thing that knows how to turn pixels into a file.
    let mut writer = HistoryArtifactWriter::new(app_local_data.join("artifacts").join("capture"));
    if let Some(history) = history {
        writer = writer.with_history(history);
    }
    let store: Arc<dyn ArtifactWriter> = Arc::new(writer);
    // The pin source hangs off the overlay's own selection slot (`OverlayShared::selection`),
    // which the overlay fills the moment a gesture commits one — not at export time. `F6`
    // pins "the picture I just selected", and the user never has to save for it to exist.
    let pin_source = Arc::new(PinSource::new());
    let writer: Arc<dyn ArtifactWriter> = store;

    let runtime = snapclip_capture::windows::overlay::WindowsOverlay::spawn_overlay(
        service,
        sink,
        clipboard,
        writer,
        options,
        preferences,
    )
    .map_err(|message| format!("capture overlay: {message}"))?;
    let runtime = CaptureRuntime::from_platform(Box::new(runtime));
    // Now that the runtime exists, hand the pin source the one thing it needs to read from
    // capture. It is wired here and not pushed from the overlay thread because the shell is
    // the only place that knows both halves exist.
    //
    // The same `Arc` goes to the caller, so the shell still owns the runtime's lifetime:
    // dropping the shell's copy is what shuts the overlay down (`CaptureRuntime::drop`).
    let runtime = Arc::new(runtime);
    pin_source.set_home(Arc::new(SelectionHome::new(runtime.clone())));
    Ok((runtime, pin_source, capture_events))
}

/// [`SelectionProvider`] 的宿主：把外壳持有的 [`CaptureRuntime`] 接在 [`PinSource`] 后面。
///
/// 运行时用 `Arc` 共享：外壳留一份（它 `Drop` 时要关掉覆盖层），贴图这边留一份。取选区
/// 时先克隆出 `Arc` 再解锁——选区槽位在平台那一侧，不在这个互斥锁里。
pub struct SelectionHome {
    runtime: Mutex<Option<Arc<CaptureRuntime>>>,
}

impl SelectionHome {
    pub fn new(runtime: Arc<CaptureRuntime>) -> Self {
        Self {
            runtime: Mutex::new(Some(runtime)),
        }
    }

    /// 外壳换掉运行时（例如重建捕获）时用。
    pub fn replace(&self, runtime: Arc<CaptureRuntime>) {
        *self
            .runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(runtime);
    }
}

impl SelectionProvider for SelectionHome {
    fn selection(&self) -> Option<Arc<snapclip_capture::artifact::SelectionPixels>> {
        self.runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?
            .selection()
    }
    fn consume_selection(
        &self,
        expected: &Arc<snapclip_capture::artifact::SelectionPixels>,
    ) -> Result<bool, String> {
        let Some(runtime) = self
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return Ok(false);
        };
        runtime
            .consume_selection(expected)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{SelectionHome, SelectionProvider, start};
    use crate::events::EventBus;
    use snapclip_capture::window_detection::DetectionOptions;
    use snapclip_model::CaptureState;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

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
        let root =
            std::env::temp_dir().join(format!("snapclip-app-capture-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (runtime, pin_source, _capture_events) = start(
            &root,
            None,
            EventBus::new(),
            DetectionOptions::default(),
            Default::default(),
        )
        .expect("the overlay should start on a desktop session");
        // Nothing is being selected until the user asks, so the honest state is Idle.
        assert_eq!(runtime.state(), CaptureState::Idle);
        // And nothing has been captured, so F6 has nothing to pin yet.
        assert!(pin_source.claim().is_none());
        // Dropping every handle shuts the overlay down (its own `Drop`), including the hotkey.
        // The pin source keeps the runtime alive by design, so both have to go.
        drop(pin_source);
        drop(runtime);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A platform that hands out one selection and counts the cancel requests it sees.
    struct FakeOverlay {
        selection: Mutex<Option<Arc<snapclip_capture::artifact::SelectionPixels>>>,
        cancels: Arc<AtomicUsize>,
    }

    impl snapclip_capture::ports::OverlayPlatform for FakeOverlay {
        fn state(&self) -> CaptureState {
            CaptureState::Selected
        }

        fn request_start(&self) -> snapclip_capture::CaptureResult<bool> {
            Ok(false)
        }

        fn request_cancel(&self) -> snapclip_capture::CaptureResult<()> {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn request_confirm(&self) -> snapclip_capture::CaptureResult<()> {
            Ok(())
        }

        fn set_preferences(&self, _preferences: snapclip_capture::preferences::OverlayPreferences) {
        }

        fn selection(&self) -> Option<Arc<snapclip_capture::artifact::SelectionPixels>> {
            self.selection
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        fn consume_selection(
            &self,
            expected: &Arc<snapclip_capture::artifact::SelectionPixels>,
        ) -> snapclip_capture::CaptureResult<bool> {
            let mut current = self.selection.lock().unwrap();
            if current
                .as_ref()
                .is_some_and(|source| Arc::ptr_eq(source, expected))
            {
                *current = None;
                self.cancels.fetch_add(1, Ordering::SeqCst);
                Ok(true)
            } else {
                Ok(false)
            }
        }

        fn shutdown(&self) {}
    }

    fn selection_pixels() -> Arc<snapclip_capture::artifact::SelectionPixels> {
        let frame = snapclip_capture::session::CapturedFrame {
            width: 4,
            height: 4,
            source: snapclip_model::pixels::PixelSource::new(
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                0,
            ),
            captured_at_unix_ms: 0,
            provider: "test",
            origin: Some(snapclip_model::geometry::Point::new(0, 0)),
        };
        Arc::new(snapclip_capture::artifact::SelectionPixels {
            corner_radius: 0,
            frame,
            region: snapclip_model::geometry::Rect::new(0, 0, 2, 2),
            pixels: snapclip_model::pixels::PixelBuffer::from_tight(
                2,
                2,
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                vec![0; 16],
            )
            .unwrap()
            .into(),
            origin: Some(snapclip_model::geometry::Point::new(0, 0)),
        })
    }

    /// How many cancel requests the platform behind `runtime` has seen.
    ///
    /// `CaptureRuntime` hands the platform out by value and does not give it back, so the
    /// counter is kept next to the runtime and shared through this cell instead of being
    /// read off the platform afterwards.
    fn fake_overlay() -> (Arc<snapclip_capture::CaptureRuntime>, Arc<AtomicUsize>) {
        let cancels = Arc::new(AtomicUsize::new(0));
        let runtime = Arc::new(snapclip_capture::CaptureRuntime::from_platform(Box::new(
            FakeOverlay {
                selection: Mutex::new(Some(selection_pixels())),
                cancels: Arc::clone(&cancels),
            },
        )));
        (runtime, cancels)
    }

    #[test]
    fn only_successful_creation_consumes_and_ends_the_capture_session() {
        let (runtime, cancels) = fake_overlay();
        let home = SelectionHome::new(runtime);
        let first = home.selection().unwrap();
        assert!(Arc::ptr_eq(&first, &home.selection().unwrap()));
        assert_eq!(
            cancels.load(Ordering::SeqCst),
            0,
            "reservation cannot end capture"
        );
        assert!(home.consume_selection(&first).unwrap());
        assert!(home.selection().is_none());
        assert_eq!(cancels.load(Ordering::SeqCst), 1);
        assert!(!home.consume_selection(&first).unwrap());
        assert_eq!(
            cancels.load(Ordering::SeqCst),
            1,
            "one successful destination, one cancellation"
        );
    }
}
