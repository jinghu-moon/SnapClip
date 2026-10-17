//! The scroll session on the overlay's side (`docs/33`).
//!
//! The capture thread ([`ScrollWorker`]) measures and stitches; this half only starts it, shows what
//! it reports, forwards the user's controls and turns its image into an export. While a session
//! runs the overlay is cut down to the panel and the selection's frame — the very frame a plain
//! capture draws, without its handles and size label: the page inside the frame is uncovered, so it
//! keeps painting and takes the wheel itself.

use super::*;
use crate::scroll::layout::{self, Button, Layout};
use crate::scroll::{Auto, Status};
use crate::windows::scroll_worker::{ScrollWorker, Spec};
use crate::windows::win::d2d::{ScrollPanelView, frame_band};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

/// The provider name an exported scroll image carries.
const SCROLL_PROVIDER: &str = "wgc-scroll";

/// Diagnostics switch: what the panel is about to show, as bytes.
///
/// A thumbnail that looks wrong on screen and right in the saved file has two possible homes —
/// the bytes the capture thread published, and the way the panel draws them. This line separates
/// the two: it hashes the buffer the upload is built from, says which of its rows land where, and
/// writes a few of those rows out, so the same rows can be recomputed from the saved image and
/// compared. Off unless asked for; hashing a 240×700 buffer per published frame is not free.
const SCROLL_TRACE_ENV: &str = "SNAPCLIP_SCROLL_TRACE";

fn preview_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os(SCROLL_TRACE_ENV).is_some())
}

/// One line per published thumbnail: the geometry the panel is about to draw, and three of the
/// buffer's rows in full.
///
/// `image_row` is where the buffer's row 0 sits among **all** the thumbnail's rows: the buffer is
/// only the tail window the panel asks for, so the rows sampled here are the ones the long image
/// has at `image_row + row`. `first`/`mid`/`last` are BGRA, one hex byte per channel, so a row that
/// repeats with a period the image does not have — the shape of the reported fault — is visible in
/// the log itself, without a photograph of the screen.
fn trace_preview(preview: &crate::scroll::Preview, layout: &Layout) {
    let well = layout.well;
    let sample = |row: u32| -> String {
        let row_width = preview.width as usize * 4;
        let from = row as usize * row_width;
        if preview.width == 0 || preview.rows == 0 || from + row_width > preview.bytes.len() {
            return "-".into();
        }
        preview.bytes[from..from + row_width]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    let mid = preview.rows / 2;
    let last = preview.rows.saturating_sub(1);
    let image_row = (preview.viewport.1 as i64 - preview.rows as i64).max(0) as u32;
    eprintln!(
        "[snapclip][scroll] thumb={}x{} image_rows={} viewport=({},{}) well={}x{}@{},{} \
         image_row={} row0@{} row_mid@{} row_last@{} row0={} row_mid={} row_last={}",
        preview.width,
        preview.rows,
        preview.image_rows,
        preview.viewport.0,
        preview.viewport.1,
        well.width(),
        well.height(),
        well.left,
        well.top,
        image_row,
        image_row,
        image_row + mid,
        image_row + last,
        sample(0),
        sample(mid),
        sample(last),
    );
    // Bounded on purpose: a dump is a file write, and a fast scroll publishes hundreds of previews.
    // The first few show how the picture grows; after that only a real change in size is news, so a
    // long session leaves a handful of files instead of hundreds of megabytes of them.
    static DUMPS: AtomicU32 = AtomicU32::new(0);
    static LAST_ROWS: AtomicU32 = AtomicU32::new(u32::MAX);
    let rows = preview.rows;
    let last = LAST_ROWS.swap(rows, Ordering::Relaxed);
    let worth_it = last == u32::MAX
        || rows.abs_diff(last) >= 64
        || DUMPS.load(Ordering::Relaxed) < 4;
    if worth_it {
        DUMPS.fetch_add(1, Ordering::Relaxed);
    }
    if worth_it {
        if let Some(path) = write_preview_bmp(preview, image_row) {
            eprintln!("[snapclip][scroll] thumb_dump={path}");
        }
    }
}

/// Writes the exact buffer the panel is about to draw as a top-down BMP, and returns its path.
///
/// Three sampled rows can prove a repeat inside those rows; they cannot show a stripe that runs
/// down columns the samples miss. This writes the whole window at 1:1 so it can be looked at (and
/// diffed against the saved image) instead of inferred. BMP over PNG because the bytes on screen
/// are BGRA rows already — a 14-byte file header and a 40-byte info header is the whole encoder.
fn write_preview_bmp(preview: &crate::scroll::Preview, image_row: u32) -> Option<String> {
    let (width, height) = (preview.width as usize, preview.rows as usize);
    if width == 0 || height == 0 || preview.bytes.len() < width * height * 4 {
        return None;
    }
    let pixels = width * height * 4;
    let mut bmp = Vec::with_capacity(54 + pixels);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((54 + pixels) as u32).to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    // Negative height: the rows go top-down, which is the order the buffer is already in.
    bmp.extend_from_slice(&(-(height as i32)).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&(pixels as u32).to_le_bytes());
    bmp.extend_from_slice(&[0u8; 16]);
    for row in 0..height {
        let from = row * width * 4;
        bmp.extend_from_slice(&preview.bytes[from..from + width * 4]);
    }
    let path = std::env::temp_dir().join(format!("snapclip-thumb-{image_row}-{width}x{height}.bmp"));
    std::fs::write(&path, &bmp).ok()?;
    Some(path.display().to_string())
}

/// One running scroll session.
pub(super) struct ScrollRun {
    pub(super) worker: ScrollWorker,
    /// The id of the capture session the selection came from; the export carries it.
    pub(super) session_id: String,
    /// The selection, in overlay (monitor-local) pixels.
    pub(super) selection: Rect,
    /// The monitor's work area, in overlay pixels.
    work: Rect,
    dpi: u32,
    pub(super) layout: Layout,
    /// The uploaded thumbnail's `(width, rows)`.
    thumb: (u32, u32),
    viewport: (u32, u32),
    image_rows: u32,
    status: Option<Status>,
    auto: Auto,
    failed: bool,
    /// The keys the buttons name, as bound when the session started.
    keys: [String; 2],
}

impl ScrollRun {
    fn new(
        worker: ScrollWorker,
        session_id: String,
        selection: Rect,
        work: Rect,
        dpi: u32,
        keymap: &crate::keymap::Keymap,
    ) -> Self {
        Self {
            worker,
            session_id,
            selection,
            work,
            dpi,
            layout: layout::layout(selection, work, 0, dpi),
            thumb: (0, 0),
            viewport: (0, 0),
            image_rows: 0,
            status: None,
            auto: Auto::Off,
            failed: false,
            keys: layout::BUTTONS.map(|button| keymap.primary_key_text(button.action())),
        }
    }

    pub(super) fn view(&self) -> ScrollPanelView {
        let auto = self.auto == Auto::On;
        ScrollPanelView {
            layout: self.layout,
            thumb: self.thumb,
            viewport: self.viewport,
            size: layout::size_text(self.selection.width(), self.image_rows),
            status: layout::status_text(self.status, self.failed),
            buttons: std::array::from_fn(|index| {
                let label = layout::BUTTONS[index].label(auto);
                match self.keys[index].as_str() {
                    "" => label.to_owned(),
                    key => format!("{key} {label}"),
                }
            }),
        }
    }

    /// The panel follows the thumbnail's height; `true` when the layout moved.
    fn relayout(&mut self) -> bool {
        let next = layout::layout(self.selection, self.work, self.thumb.1 as i32, self.dpi);
        let moved = next != self.layout;
        self.layout = next;
        moved
    }

    /// The overlay's window region: the panel with its shadow, and the band the selection's frame
    /// is painted in. Everything inside the frame is a hole.
    pub(super) fn region(&self) -> Vec<Rect> {
        region_rects(&self.layout, self.selection, frame_band(self.dpi))
    }
}

/// Every rectangle of the overlay that stays while a session runs: the panel's footprint, and the
/// four sides of the band `band` pixels wide just inside `selection`'s edge, which is where the
/// frame is painted ([`frame_band`]). The frame covers that band in a plain capture too; the
/// selection's interior is never covered.
pub(super) fn region_rects(layout: &Layout, selection: Rect, band: i32) -> Vec<Rect> {
    let Rect {
        left,
        top,
        right,
        bottom,
    } = selection;
    let t = band.min(selection.width() / 2).min(selection.height() / 2);
    vec![
        layout.footprint,
        Rect::new(left, top, right, top + t),
        Rect::new(left, bottom - t, right, bottom),
        Rect::new(left, top + t, left + t, bottom - t),
        Rect::new(right - t, top + t, right, bottom - t),
    ]
}

impl OverlayController {
    /// `Enter` on an `F7` session: end the capture session and start scrolling its selection.
    ///
    /// A refusal (no committed selection, too short) keeps the capture session, so the user can fix
    /// the selection or take a plain screenshot instead.
    pub(super) fn begin_scroll_session(&mut self) {
        if self.scroll.is_some() {
            return;
        }
        let Some(layout) = self.monitor_layout.clone() else {
            eprintln!("[snapclip][scroll] refused: this session has no monitor layout");
            return;
        };
        let session_id = self.session_id();
        let start = match self.session.begin_scroll() {
            Ok(start) => start,
            Err(error) => {
                eprintln!("[snapclip][scroll] refused: {error}");
                return;
            }
        };
        self.scroll_entry_armed = false;
        // The capture session is over, but its window stays: everything that would react to the
        // pointer as if a selection were still being made is switched off.
        self.disarm_dwell();
        self.disarm_hover_timer();
        self.disarm_refinement();
        self.set_preview_target(None);
        self.hover_target = None;
        self.gesture.reset();
        self.hint = None;

        let selection = start.selection;
        let centre = selection.center();
        let aim = Point::new(centre.x + layout.bounds.left, centre.y + layout.bounds.top);
        let monitor = match monitor::captured_monitor_at(aim) {
            Ok(monitor) => monitor,
            Err(error) => {
                eprintln!("[snapclip][scroll] monitor lookup failed: {error}");
                let error = CaptureError::MonitorUnavailable(error);
                self.sink.on_failed(Some(&session_id), &error, "scroll");
                self.release_session();
                self.sink.on_state(&session_id, CaptureState::Idle, None);
                return;
            }
        };
        let spec = Spec {
            monitor: monitor.handle,
            region: selection,
            aim,
            thumb_width: layout::thumb_width(start.dpi) as u32,
            preview_rows: layout::well_capacity(selection, layout.local_work_area(), start.dpi)
                as u32,
            notify_thread: unsafe { GetCurrentThreadId() },
        };
        eprintln!(
            "[snapclip][scroll] session={session_id} selection={} aim=({},{}) dpi={}",
            describe_rect(selection),
            aim.x,
            aim.y,
            start.dpi
        );
        let run = ScrollRun::new(
            ScrollWorker::start(spec),
            session_id,
            selection,
            layout.local_work_area(),
            start.dpi,
            &self.preferences.keymap,
        );
        if let Err(error) = hotkey::register_scoped(self.window, hotkey::CANCEL_HOTKEY) {
            // Esc on the overlay and the panel's cancel button still work.
            eprintln!("[snapclip][scroll] Esc hotkey unavailable: {error:?}");
        }
        self.set_window_shape(&run.region());
        self.scroll = Some(run);
        self.invalidate();
    }

    /// The capture thread published something: pull it, show it.
    pub(super) fn on_scroll_ready(&mut self) {
        let Some(run) = self.scroll.as_mut() else {
            return;
        };
        let report = run.worker.take_report();
        if let Some(preview) = report.preview {
            let mut preview = preview;
            if let Some(renderer) = self.renderer.as_mut()
                && let Err(error) = renderer.update_scroll_preview(&preview)
            {
                eprintln!("[snapclip][scroll] preview upload failed: {error}");
            }
            if preview_trace_enabled() {
                trace_preview(&preview, &run.layout);
            }
            run.thumb = (preview.width, preview.rows);
            run.viewport = preview.viewport;
            run.image_rows = preview.image_rows;
            // The pixels are in D2D's bitmap now, so the capture thread can gather the next
            // thumbnail straight into this buffer instead of allocating a new one per frame.
            run.worker.recycle_preview(std::mem::take(&mut preview.bytes));
        }
        run.status = report.status;
        run.auto = report.auto;
        if let Some(failure) = report.failure
            && !run.failed
        {
            eprintln!("[snapclip][scroll] the capture thread ended: {failure}");
            run.failed = true;
        }
        if run.relayout() {
            let region = run.region();
            self.set_window_shape(&region);
        }
        self.invalidate();
    }

    pub(super) fn on_scroll_button(&mut self, button: Button) {
        eprintln!("[snapclip][scroll] button {button:?}");
        match button {
            Button::Auto => self.toggle_scroll_auto(),
            Button::Finish => self.finish_scroll_session("button"),
        }
    }

    /// The auto button or its key: start or stop scrolling by itself.
    pub(super) fn toggle_scroll_auto(&mut self) {
        if let Some(run) = self.scroll.as_mut() {
            let on = !run.worker.auto();
            run.worker.set_auto(on);
            run.auto = if on { Auto::On } else { Auto::Off };
            self.invalidate();
        }
    }

    /// `F9`, `Enter` or the finish button: stop, and export the long image to the history, the
    /// desktop and the clipboard.
    pub(super) fn finish_scroll_session(&mut self, reason: &str) {
        let Some(run) = self.end_scroll() else {
            return;
        };
        let session_id = run.session_id.clone();
        let dpi = run.dpi;
        let image = run.worker.finish();
        self.release_session();
        let Some((width, rows, bgra)) = image else {
            eprintln!("[snapclip][scroll] finished reason={reason}: no frame ever arrived");
            let error = CaptureError::CaptureFailed("the scroll session captured nothing".into());
            self.sink
                .on_failed(Some(&session_id), &error, SCROLL_PROVIDER);
            self.sink.on_state(&session_id, CaptureState::Idle, None);
            return;
        };
        eprintln!("[snapclip][scroll] finished reason={reason} image={width}x{rows}");
        let prepared = SelectionPixels {
            frame: CapturedFrame {
                width,
                height: rows,
                pixel_format: PixelFormat::Bgra8Unorm,
                captured_at_unix_ms: snapclip_model::unix_time_ms(),
                provider: SCROLL_PROVIDER,
                // A stitched long image is a composed picture, not a monitor's frozen frame, so
                // it has no screen position to report. `None` is that statement — and it is not
                // `(0, 0)`, which is the primary monitor's real origin (see
                // `SelectionPixels::origin`). The pin source reads `None` as "this one cannot be
                // pinned back onto the desktop".
                origin: None,
            },
            region: Rect::new(0, 0, width as i32, rows as i32),
            bgra,
            origin: None,
        };
        let job = ExportJob {
            generation: 0,
            session_id: session_id.clone(),
            prepared,
            dpi,
            monitor_device_name: None,
            notify_thread: unsafe { GetCurrentThreadId() },
            executor: Self::scroll_executor(
                self.writer.clone(),
                self.clipboard.clone(),
                desktop_dir(),
            ),
        };
        match self.export_worker.submit(job) {
            Ok(Some(_)) => self.scroll_export = Some(session_id),
            Ok(None) | Err(_) => {
                let error = CaptureError::EncodeFailed("export worker unavailable".into());
                eprintln!("[snapclip][scroll] export submit failed session={session_id}");
                self.sink
                    .on_failed(Some(&session_id), &error, SCROLL_PROVIDER);
                self.sink.on_state(&session_id, CaptureState::Idle, None);
            }
        }
    }

    /// The scroll export, run on the export thread: the history write, then the desktop copy and the
    /// clipboard (`docs/33`). Both copies work from what this thread already holds — the file it
    /// just wrote and the pixels in the job — so the overlay thread never touches an image that can
    /// be hundreds of megapixels, and the image is never decoded back. A failed copy is logged and
    /// does not withhold the other: the history entry exists either way.
    fn scroll_executor(
        writer: Arc<dyn ArtifactWriter>,
        clipboard: Arc<dyn ClipboardWriter>,
        desktop: Result<std::path::PathBuf, String>,
    ) -> ExportExecutor {
        let write = Self::export_executor(writer);
        Box::new(move |job: &ExportJob| {
            let artifact = write(job)?;
            if let Some(path) = artifact.png_path() {
                let copied = desktop
                    .clone()
                    .and_then(|desktop| copy_into(path, &desktop, &job.session_id));
                match copied {
                    Ok(destination) => eprintln!(
                        "[snapclip][scroll] desktop copy path={}",
                        destination.display()
                    ),
                    Err(error) => eprintln!("[snapclip][scroll] desktop copy failed: {error}"),
                }
            }
            let image = &job.prepared;
            let started = Instant::now();
            match clipboard.copy_image(
                image.frame.width,
                image.frame.height,
                &image.bgra,
                artifact.png_path(),
            ) {
                Ok(()) => eprintln!(
                    "[snapclip][scroll] clipboard copy done ms={}",
                    started.elapsed().as_millis()
                ),
                Err(error) => eprintln!("[snapclip][scroll] clipboard copy failed: {error}"),
            }
            Ok(artifact)
        })
    }

    /// Take the running session out of the controller and give the window back its whole area.
    /// The caller decides what happens to the worker: `finish` keeps the image, dropping discards it.
    pub(super) fn end_scroll(&mut self) -> Option<ScrollRun> {
        let run = self.scroll.take()?;
        hotkey::unregister_scoped(self.window, hotkey::CANCEL_HOTKEY);
        self.set_window_region(&[]);
        Some(run)
    }
}

/// The user's desktop folder.
///
/// USERPROFILE\Desktop is not authoritative: Windows permits the shell desktop to be redirected to
/// OneDrive or another folder. Ask the shell for the configured Known Folder so the path we log is
/// the path Explorer actually presents to the user.
fn desktop_dir() -> Result<std::path::PathBuf, String> {
    unsafe {
        use windows_sys::Win32::System::Com::CoTaskMemFree;
        use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath};
        let mut raw = null_mut();
        let result = SHGetKnownFolderPath(&FOLDERID_Desktop, 0, null_mut(), &mut raw);
        if result < 0 || raw.is_null() {
            return Err(format!(
                "SHGetKnownFolderPath(FOLDERID_Desktop) failed: HRESULT 0x{result:08x}"
            ));
        }
        let mut length = 0usize;
        while *raw.add(length) != 0 {
            length += 1;
        }
        let path = std::path::PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(
            raw, length,
        )));
        CoTaskMemFree(raw.cast());
        Ok(path)
    }
}

/// Copy the written image into `directory` as `SnapClip-{session_id}.png`.
fn copy_into(
    source: &std::path::Path,
    directory: &std::path::Path,
    session_id: &str,
) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    let destination = directory.join(format!("SnapClip-{session_id}.png"));
    std::fs::copy(source, &destination).map_err(|error| {
        format!(
            "copy {} -> {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::{frame_band, region_rects};
    use crate::geometry::Rect;
    use crate::scroll::layout::layout;

    mod export {
        use std::path::PathBuf;
        use std::sync::{Arc, Mutex};

        use super::super::{CapturedFrame, ExportJob, OverlayController, SCROLL_PROVIDER};
        use crate::artifact::SelectionPixels;
        use crate::geometry::Rect;
        use crate::ports::{ArtifactWriter, ClipboardWriter};
        use crate::{CaptureError, CaptureResult, PixelFormat};
        use snapclip_model::{CaptureArtifact, CapturePayload};

        /// Writes the raw bytes where the history store would put the PNG.
        struct FileWriter {
            path: PathBuf,
            fails: bool,
        }

        impl ArtifactWriter for FileWriter {
            fn write(
                &self,
                session_id: &str,
                prepared: &SelectionPixels,
                dpi: u32,
                monitor_device_name: Option<String>,
            ) -> CaptureResult<CaptureArtifact> {
                if self.fails {
                    return Err(CaptureError::EncodeFailed("disk full".into()));
                }
                std::fs::write(&self.path, b"png").unwrap();
                Ok(CaptureArtifact {
                    session_id: session_id.into(),
                    width: prepared.frame.width,
                    height: prepared.frame.height,
                    dpi,
                    pixel_format: PixelFormat::Bgra8Unorm,
                    captured_at_unix_ms: 0,
                    monitor_device_name,
                    payload: CapturePayload::PngFile {
                        path: self.path.clone(),
                    },
                })
            }
        }

        /// Records what reached the clipboard: the size, where the pixels lived, and the PNG.
        #[derive(Default)]
        struct Recorder(Mutex<Vec<(u32, u32, usize, usize, Option<PathBuf>)>>);

        impl ClipboardWriter for Recorder {
            fn copy_text(&self, _text: &str) {}

            fn copy_image(
                &self,
                width: u32,
                height: u32,
                bgra: &[u8],
                png: Option<&std::path::Path>,
            ) -> Result<(), String> {
                self.0.lock().unwrap().push((
                    width,
                    height,
                    bgra.as_ptr() as usize,
                    bgra.len(),
                    png.map(PathBuf::from),
                ));
                Ok(())
            }
        }

        fn job(width: u32, rows: u32) -> ExportJob {
            ExportJob {
                generation: 1,
                session_id: "s1".into(),
                prepared: SelectionPixels {
                    frame: CapturedFrame {
                        width,
                        height: rows,
                        pixel_format: PixelFormat::Bgra8Unorm,
                        captured_at_unix_ms: 0,
                        provider: SCROLL_PROVIDER,
                        origin: None,
                    },
                    region: Rect::new(0, 0, width as i32, rows as i32),
                    bgra: vec![0; width as usize * rows as usize * 4],
                    origin: None,
                },
                dpi: 144,
                monitor_device_name: None,
                notify_thread: 0,
                executor: Box::new(|_| unreachable!()),
            }
        }

        fn scratch(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "snapclip-scroll-export-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        /// The field failure: a 1250x25826 image (32 MP, over the decoder's 24 MP cap for untrusted
        /// input) never reached the clipboard, because the clipboard step decoded the PNG back. The
        /// clipboard must get the job's own pixels — the very buffer, not a decoded copy — with the
        /// written PNG beside them (not a second encode), and the desktop must get the written file.
        #[test]
        fn a_long_image_reaches_history_desktop_and_clipboard_from_its_own_pixels() {
            let dir = scratch("long");
            let clipboard = Arc::new(Recorder::default());
            let executor = OverlayController::scroll_executor(
                Arc::new(FileWriter {
                    path: dir.join("history.png"),
                    fails: false,
                }),
                clipboard.clone(),
                Ok(dir.join("desktop")),
            );
            let job = job(1250, 25826);
            let artifact = executor(&job).expect("the history write succeeds");
            assert_eq!((artifact.width, artifact.height), (1250, 25826));
            assert_eq!(
                std::fs::read(dir.join("desktop").join("SnapClip-s1.png")).unwrap(),
                b"png"
            );
            assert_eq!(
                *clipboard.0.lock().unwrap(),
                vec![(
                    1250,
                    25826,
                    job.prepared.bgra.as_ptr() as usize,
                    job.prepared.bgra.len(),
                    Some(dir.join("history.png"))
                )]
            );
            std::fs::remove_dir_all(&dir).unwrap();
        }

        /// A desktop that cannot be resolved does not withhold the clipboard or the history entry.
        #[test]
        fn a_missing_desktop_still_reaches_the_clipboard() {
            let dir = scratch("nodesk");
            let clipboard = Arc::new(Recorder::default());
            let executor = OverlayController::scroll_executor(
                Arc::new(FileWriter {
                    path: dir.join("history.png"),
                    fails: false,
                }),
                clipboard.clone(),
                Err("no desktop".into()),
            );
            assert!(executor(&job(64, 64)).is_ok());
            assert_eq!(clipboard.0.lock().unwrap().len(), 1);
            std::fs::remove_dir_all(&dir).unwrap();
        }

        /// No history entry, no copies: a failed write is the export's failure, and nothing else runs.
        #[test]
        fn a_failed_write_copies_nothing() {
            let dir = scratch("fail");
            let clipboard = Arc::new(Recorder::default());
            let executor = OverlayController::scroll_executor(
                Arc::new(FileWriter {
                    path: dir.join("history.png"),
                    fails: true,
                }),
                clipboard.clone(),
                Ok(dir.join("desktop")),
            );
            assert!(executor(&job(64, 64)).is_err());
            assert!(clipboard.0.lock().unwrap().is_empty());
            assert!(!dir.join("desktop").exists());
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    /// The hole is the whole point: no rectangle of the window may cover the selection's interior,
    /// or the page under it would be occluded (and stop painting) and that part would not take the
    /// wheel. The frame's band is the one part of the selection the window keeps — the band a plain
    /// capture paints its frame in as well.
    #[test]
    fn the_window_region_covers_only_the_frame_band_of_the_selection() {
        let selection = Rect::new(400, 200, 1200, 800);
        let work = Rect::new(0, 0, 1920, 1040);
        for dpi in [96, 144, 192] {
            let band = frame_band(dpi);
            let interior = Rect::new(
                selection.left + band,
                selection.top + band,
                selection.right - band,
                selection.bottom - band,
            );
            let placed = layout(selection, work, 300, dpi);
            let rects = region_rects(&placed, selection, band);
            for rect in &rects {
                assert!(
                    rect.intersect(interior).is_empty(),
                    "{rect:?} covers part of {interior:?} at {dpi} dpi"
                );
            }
            assert!(rects[1..].iter().all(|rect| selection.contains_rect(*rect)));
            assert!(rects.contains(&placed.footprint));
            assert!(placed.footprint.intersect(selection).is_empty());
        }
    }

    /// The band runs round the whole edge, corners included, and nothing past it.
    #[test]
    fn the_frame_band_runs_round_the_selection_edge() {
        let selection = Rect::new(10, 20, 110, 220);
        let placed = layout(selection, Rect::new(0, 0, 1920, 1040), 50, 96);
        let rects = region_rects(&placed, selection, 2);
        let covered = |x, y| {
            rects
                .iter()
                .any(|rect| rect.contains(crate::geometry::Point::new(x, y)))
        };
        for (x, y) in [
            (10, 20),
            (109, 219),
            (11, 100),
            (108, 100),
            (60, 21),
            (60, 218),
        ] {
            assert!(covered(x, y), "({x},{y}) is not on the frame");
        }
        for (x, y) in [(9, 100), (12, 100), (60, 22), (110, 100), (60, 220)] {
            assert!(!covered(x, y), "({x},{y}) is off the frame but covered");
        }
        // A selection thinner than two bands is covered whole, never past its edge.
        let thin = Rect::new(10, 20, 13, 220);
        for rect in &region_rects(&placed, thin, 2)[1..] {
            assert!(thin.contains_rect(*rect), "{rect:?}");
        }
    }
}
