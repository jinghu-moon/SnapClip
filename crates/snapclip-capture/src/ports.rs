//! The contracts the overlay needs from the rest of the process.
//!
//! Nothing here knows about the clipboard, SQLite, OCR or Tauri. Implementations are
//! supplied by the composition root. (The runtime that owns the platform controller is
//! a sibling: [`crate::runtime`].)

use snapclip_model::CaptureState;

use super::annotation::AnnotationCommand;
use super::geometry::MonitorLayout;
use super::{CaptureError, CaptureResult};

pub use crate::artifact::{CaptureService, PixelSliceSource, SelectionPixels};

/// Receives low-frequency lifecycle events. Deliberately a trait so the capture
/// module never depends on the shell's event bus.
pub trait CaptureEventSink: Send + Sync + 'static {
    fn on_started(&self, session_id: &str, layout: &MonitorLayout);
    fn on_state(&self, session_id: &str, state: CaptureState, layout: Option<&MonitorLayout>);
    fn on_completed(&self, artifact: &snapclip_model::CaptureArtifact);
    fn on_cancelled(&self, session_id: &str, reason: &str);
    fn on_failed(&self, session_id: Option<&str>, error: &CaptureError, provider: &str);

    /// 延时截图到点了：图已经在贴图槽位里，请外壳把它贴出去（docs/23 §14.55.8）。
    ///
    /// 这一条只说"贴"，不说"怎么贴"：`F6` 那条链路怎么贴图是外壳的事，捕获侧不该知道。
    fn on_pin_requested(&self, _session_id: &str) {}

    /// 用户要求保存：选区像素已经读回来了，请外壳把它交给保存面板（docs/41 S2 批次 B）。
    ///
    /// 与 [`Self::on_pin_requested`] 同形：这一条只说"存"，不说"怎么存"——面板长什么样、
    /// 存到哪、用什么格式，都是外壳的事，捕获侧不该知道。
    fn on_save_requested(&self, _session_id: &str, _pixels: &SelectionPixels) {}
}

/// A sink that drops everything. Useful for tests and headless runs.
pub struct NullEventSink;

impl CaptureEventSink for NullEventSink {
    fn on_started(&self, _session_id: &str, _layout: &MonitorLayout) {}
    fn on_state(&self, _session_id: &str, _state: CaptureState, _layout: Option<&MonitorLayout>) {}
    fn on_completed(&self, _artifact: &snapclip_model::CaptureArtifact) {}
    fn on_cancelled(&self, _session_id: &str, _reason: &str) {}
    fn on_failed(&self, _session_id: Option<&str>, _error: &CaptureError, _provider: &str) {}
}

/// The overlay/presentation implementation for the current platform.
///
/// `Send + Sync` is required because the rest of the process drives sessions from
/// ordinary threads of the shell; implementations are responsible for marshalling work onto
/// their own UI thread.
pub trait OverlayPlatform: Send + Sync + 'static {
    fn state(&self) -> CaptureState;

    /// `true` when a new session was accepted, `false` when one is already active.
    fn request_start(&self) -> CaptureResult<bool>;

    fn request_cancel(&self) -> CaptureResult<()>;

    fn request_confirm(&self) -> CaptureResult<()>;

    /// Deliver one low-frequency toolbar instruction to the running session's
    /// annotation document. The overlay's toolbar emits at most one per click, never
    /// on mouse-move or per pixel (docs/11 §7.1 "工具栏不进入像素管线"). The default
    /// rejects the call so only platforms with a real overlay need to override it.
    fn request_annotation(&self, command: AnnotationCommand) -> CaptureResult<()> {
        let _ = command;
        Err(CaptureError::Unsupported)
    }

    /// Use `preferences` from the next session on; a running session keeps what it started with.
    fn set_preferences(&self, preferences: crate::preferences::OverlayPreferences);

    /// The preferences a session starting now would use (docs/23 §14.53).
    ///
    /// The overlay's custom-region panel edits the display settings in place (docs/23 §14.54), and
    /// this is how such an edit gets out of the overlay: the shell asks when a capture ends and writes
    /// the answer into the settings file, so what the user changed in the panel outlives the process.
    /// `None` means the platform keeps no such state, and the shell's own copy is still the truth.
    fn preferences(&self) -> Option<crate::preferences::OverlayPreferences> {
        None
    }

    /// Hand over the last selection the user confirmed, if this platform still holds one.
    ///
    /// This is the seam for **pinning what was just captured**: the overlay is the only
    /// thing that has the pixels (the frozen frame's texture dies with the session, and
    /// the exported file is a re-decode), so a consumer asks for them instead of reading
    /// the screen again — which is exactly the round trip that cannot work while the
    /// overlay still covers the monitor.
    ///
    /// Taking is deliberate: the selection is megabytes, so a second reader would either
    /// copy it or race for it. The default returns `None`, which is the honest answer for
    /// platforms that do not keep one.
    fn take_selection(&self) -> Option<std::sync::Arc<SelectionPixels>> {
        None
    }

    fn shutdown(&self);
}

/// Writes short text to the system clipboard on behalf of the overlay.
///
/// The overlay has two clipboard affordances: pressing `C` copies the colour value under the
/// magnifier, and finishing a scroll capture copies the long image. Owning the clipboard is the
/// composition root's job, because whoever writes to it also has to mark the write as "ours" so
/// SnapClip's own clip monitor does not record it. Capture therefore declares the need and the shell
/// implements it — the alternative (capture calling into the clipboard module, as it
/// did before this port existed) is a dependency the seam must not have.
pub trait ClipboardWriter: Send + Sync + 'static {
    /// Copy `text`. Failures are the implementation's to log; they are never fatal.
    fn copy_text(&self, text: &str);

    /// Copy an image given as tightly packed, top-down BGRA rows (a finished scroll capture), and
    /// `png`, the file the same image was already written to, when there is one.
    ///
    /// Pixels, not only the file: the caller already holds them, and the image is SnapClip's own,
    /// so it must not go back through a decoder whose limits exist for untrusted input. The file,
    /// too, because encoding the image a second time for the clipboard's PNG format is the same
    /// work done twice (1.2 s for a 2148x42924 capture).
    fn copy_image(
        &self,
        width: u32,
        height: u32,
        bgra: &[u8],
        png: Option<&std::path::Path>,
    ) -> Result<(), String>;
}

/// Rows on demand, for pictures that are never held whole (docs/35 §13.4).
///
/// A scroll capture's long image can be tens of thousands of rows; materialising it to hand
/// over one buffer costs the whole image in memory. A writer that can stream asks for bands
/// instead, and the source decides whether the rows come from a file, a ring buffer or a
/// single contiguous image.
pub trait BandSource: Send {
    /// How many rows the picture has.
    fn rows(&self) -> u32;
    /// Fill `out` with `rows` rows starting at `first_row`, top-down, tightly packed BGRA.
    ///
    /// Returns false when the rows cannot be supplied; the writer then refuses the artifact
    /// rather than writing a short one.
    fn read_rows(&mut self, first_row: u32, rows: u32, out: &mut Vec<u8>) -> bool;
}

/// Everything about a picture except its pixels, so a banded one can be named, stamped and
/// placed exactly like a whole one.
#[derive(Debug, Clone, Copy)]
pub struct SelectionSpec {
    pub region: crate::geometry::Rect,
    pub pixel_format: snapclip_model::PixelFormat,
    pub captured_at_unix_ms: i64,
    pub origin: Option<snapclip_model::geometry::Point>,
    pub corner_radius: i32,
}

/// Turns a prepared selection into a durable artifact.
///
/// Owned by the composition root, because this is where encoding and writing meet: the
/// store that writes PNGs lives in `snapclip-history`, and capture must not depend on it
/// (docs/23 T2.4, and the dependency gate enforces exactly that). Capture's job ends at
/// "here are the pixels the user selected, and the frame they came from".
///
/// Implementations run on the export worker thread, never on the overlay thread.
pub trait ArtifactWriter: Send + Sync + 'static {
    fn write(
        &self,
        session_id: &str,
        prepared: &crate::artifact::SelectionPixels,
        dpi: u32,
        monitor_device_name: Option<String>,
    ) -> crate::CaptureResult<snapclip_model::CaptureArtifact>;

    /// Write a picture that never exists whole (docs/35 §13.4).
    ///
    /// There is no default: a writer must say what it does with rows. A writer that genuinely
    /// needs the whole picture can call [`whole_from_bands`] and delegate to
    /// [`ArtifactWriter::write`], but it has to say so at the call site, because the silent
    /// fallback is the 154 MB path this method exists to remove.
    fn write_bands(
        &self,
        session_id: &str,
        spec: &SelectionSpec,
        bands: &mut dyn BandSource,
        dpi: u32,
        monitor_device_name: Option<String>,
    ) -> crate::CaptureResult<snapclip_model::CaptureArtifact>;
}

/// Materialise a band source into a whole picture.
///
/// This used to be the **default** `write_bands`, which meant any implementor that did not care
/// silently fell back to holding the whole image: exactly the 154 MB path the banded writer
/// exists to remove. It is now an explicit call, so a writer that wants it has to say so
/// (docs/35 §16).
pub fn whole_from_bands(
    spec: &SelectionSpec,
    bands: &mut dyn BandSource,
) -> crate::CaptureResult<crate::artifact::SelectionPixels> {

    let width = u32::try_from(spec.region.width()).unwrap_or(0);
    let rows = bands.rows();
    if width == 0 || rows == 0 || u32::try_from(spec.region.height()).ok() != Some(rows) {
        return Err(crate::CaptureError::EncodeFailed(format!(
            "{}x{} is not a drawable region",
            spec.region.width(),
            spec.region.height()
        )));
    }
    let mut bgra = Vec::new();
    bgra.resize(rows as usize * width as usize * 4, 0);
    if !bands.read_rows(0, rows, &mut bgra) {
        return Err(crate::CaptureError::EncodeFailed(
            "band source could not supply its rows".into(),
        ));
    }
    let prepared = crate::artifact::SelectionPixels {
        frame: crate::session::CapturedFrame {
            width,
            height: rows,
            pixel_format: spec.pixel_format,
            captured_at_unix_ms: spec.captured_at_unix_ms,
            provider: "band-source",
            origin: spec.origin,
        },
        region: spec.region,
        bgra,
        origin: spec.origin,
        corner_radius: spec.corner_radius,
    };
        Ok(prepared)
}
