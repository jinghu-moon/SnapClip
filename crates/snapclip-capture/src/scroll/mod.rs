//! Scroll capture: one long image from a selection the user (or the auto-scroller) scrolls.
//!
//! Everything here is pure arithmetic over BGRA byte buffers; the capture thread that feeds it
//! frames lives in `windows/scroll_worker.rs`. The design is derived in `docs/33`:
//!
//! * [`estimate`] — how far the content moved between two frames, computed for every frame;
//! * [`stitch`] — the long image, grown by exactly the rows never seen before;
//! * [`thumb`] — the growing thumbnail beside the selection, updated incrementally;
//! * [`layout`] — where the panel goes and what it says.
//!
//! [`Tracker`] strings the first three together: one call per captured frame.

pub(crate) mod backed;
pub(crate) mod estimate;
pub(crate) mod layout;
pub(crate) mod stitch;
pub mod store;
pub(crate) mod thumb;

use estimate::{Estimate, FrameSig, estimate};
use backed::BackedCanvas;
use stitch::{CanvasStore, Placement, Stitcher};
use thumb::Thumb;

/// The long image is capped at this many selection heights (the reference implementation's cap).
/// How many viewport heights an automatic scroll may reach.
///
/// This used to be the memory ceiling: the old stitcher held the whole picture, so one more screen
/// was one more screen of RAM. The canvas is now a bounded edge ring with sealed tiles in a temp
/// file (docs/35), so raising this buys rows with temp disk and time, not with resident memory:
/// at an 808 row viewport, 48 screens is 38 784 rows and roughly 190 MB of tiles.
pub(crate) const MAX_SCREENS: u32 = 48;
/// The long image is also capped at this many bytes of canvas. A row cap alone is not a resource
/// policy: 32 selection heights is 130 MB for a 1241×821 selection but 1.06 GB for a full-screen
/// one on a 3840×2160 monitor, and growing the canvas momentarily holds the old and the new
/// allocation. A few hundred megapixels is already far past any real page, so the byte cap is
/// what actually bounds a session.
pub(crate) const MAX_BYTES: usize = 512 * 1024 * 1024;
/// The shortest selection a session can scroll: after a scroll, two frames must still share
/// [`estimate::MIN_OVERLAP`] rows, so a shorter one could never be measured.
pub(crate) const MIN_VIEW: u32 = 2 * estimate::MIN_OVERLAP;

/// What the tracker can say about the session as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    /// The last frame was placed or found still.
    Tracking,
    /// The last frame could not be located against the reference (scrolled too far in one frame,
    /// or repeated content). The reference is kept, so scrolling back recovers.
    Lost,
    /// The image reached [`MAX_SCREENS`] and stops growing.
    Full,
    /// The backing store could not write (a full disk, a vanished temp directory). The image
    /// stops growing, but the reason is not the row cap and the panel must not say it is
    /// (docs/35 §12.4).
    Storage,
}

/// How many frames may pass between two scroll log lines when nothing about the status changes.
///
/// The per-frame line is a diagnostic, not a heartbeat. At a few hundred frames a second one line
/// per frame costs more than the frame itself: the first run on a real machine sat at 22 ms on the
/// ninetyfifth percentile with console I/O as the largest suspect. Status changes still print
/// immediately, because those are the events a diagnosis needs.
pub(crate) const LOG_EVERY_FRAMES: u64 = 60;

/// Whether the scroll loop should print the line for `frame`.
///
/// A status change always prints, the first frame always prints, and otherwise every
/// [`LOG_EVERY_FRAMES`] frames. Pure so the rule can be tested without a console.
pub(crate) fn should_log(frame: u64, status: Status, previous: Option<Status>) -> bool {
    previous.map_or(true, |prev| prev != status) || frame % LOG_EVERY_FRAMES == 0
}

/// Automatic scrolling, as the panel shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Auto {
    #[default]
    Off,
    On,
    /// The window under the selection refused the wheel (`ERROR_ACCESS_DENIED`: it runs at a higher
    /// integrity level, and UIPI drops posted input from below). Auto turned itself off; the user's
    /// own wheel still reaches it. Cleared when auto is turned on again.
    Refused,
}

/// What one frame did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Step {
    pub estimate: Estimate,
    pub placement: Option<Placement>,
}

/// The thumbnail rows the panel shows, and where the viewport is among them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Preview {
    pub width: u32,
    pub rows: u32,
    pub bytes: Vec<u8>,
    /// The viewport's thumbnail rows `[start, end)`, relative to this window.
    pub viewport: (u32, u32),
    /// Height of the long image so far, in canvas rows.
    pub image_rows: u32,
}

/// How tall the canvas may grow: whichever of the screen cap and the byte budget is smaller, in
/// rows. The byte side is rounded down, and one screen is always allowed even when a single frame
/// is bigger than the budget (a session must at least hold the frame it started from).
fn row_limit(width: u32, view: u32, max_bytes: usize) -> u32 {
    let row = width.max(1) as usize * 4;
    let by_bytes = max_bytes / row;
    let rows = by_bytes.min(MAX_SCREENS as usize * view as usize);
    rows.max(view as usize).min(u32::MAX as usize) as u32
}

/// The format a scroll capture will actually save with, and why it was lowered if it was.
///
/// The question is answered **before** the worker starts, because the worker is what allocates the
/// canvas: a WebP the container can never hold must be refused when zero rows exist, not after
/// thirty seconds of scrolling (`docs/34` §15). The row count is the *possible* maximum
/// ([`row_limit`]), not what the page happens to contain.
pub(crate) fn scroll_format(
    preferred: crate::save_format::SaveFormat,
    width: u32,
    view: u32,
) -> (
    crate::save_format::SaveFormat,
    Option<crate::save_format::Lowering>,
) {
    use crate::save_format::{within_jpeg_frame_budget, Lowering, SaveFormat};

    let rows = row_limit(width, view, MAX_BYTES);
    if let Err(limit) = preferred.can_encode(u64::from(width), u64::from(rows)) {
        return (SaveFormat::Png, Some(Lowering::Format(limit)));
    }
    // JPG could hold these rows, but its only correct arm holds the whole frame (`docs/34` §17), and
    // a scroll capture is exactly the image that must not be held (`docs/34` §18).
    if preferred == SaveFormat::Jpeg && !within_jpeg_frame_budget(u64::from(width), u64::from(rows))
    {
        return (
            SaveFormat::Png,
            Some(Lowering::FrameBudget {
                pixels: u64::from(width) * u64::from(rows),
                max: crate::save_format::JPEG_FRAME_BUDGET_PIXELS,
            }),
        );
    }
    (preferred, None)
}

#[derive(Debug)]
pub(crate) struct Tracker {
    width: u32,
    view: u32,
    reference: FrameSig,
    stitcher: Stitcher<BackedCanvas>,
    thumb: Thumb,
    status: Status,
    /// Scratch: how long the last `push`'s thumbnail refresh took, in microseconds.
    last_thumb_us: u64,
    /// Scratch for the band the thumbnail reads: a frame reuses it instead of allocating the same
    /// half megabyte every time (docs/35 §5.2: this is part of the working budget).
    band: Vec<u8>,
}

impl Tracker {
    /// `first` is the selection's first frame, `width × view` BGRA; `thumb_width` the widest
    /// thumbnail the panel shows.
    pub(crate) fn new(first: &[u8], width: u32, view: u32, thumb_width: u32) -> Self {
        Self::with_rows(
            first,
            width,
            view,
            thumb_width,
            row_limit(width, view, MAX_BYTES),
        )
    }

    /// The same tracker with an explicit canvas row cap, so a test can drive the byte budget
    /// without allocating half a gigabyte.
    fn with_rows(first: &[u8], width: u32, view: u32, thumb_width: u32, max_rows: u32) -> Self {
        let stitcher = Stitcher::with_store(
            BackedCanvas::new(first, width, view, max_rows),
            width,
            view,
        );
        let mut thumb = Thumb::new(width, thumb_width);
        let mut first_frame = vec![0u8; (width * view * 4) as usize];
        stitcher
            .store()
            .read_rows(0, view, &mut first_frame)
            .expect("the first frame was just written to the store");
        thumb.refresh(&first_frame, 0, view, 0, 0, view);
        Self {
            width,
            view,
            reference: FrameSig::of(first, width, view),
            stitcher,
            thumb,
            status: Status::Tracking,
            last_thumb_us: 0,
            band: Vec::new(),
        }
    }

    pub(crate) fn status(&self) -> Status {
        self.status
    }

    pub(crate) fn image_rows(&self) -> u32 {
        self.stitcher.rows()
    }

    /// The selection's width, in pixels.
    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    /// How long the last `push`'s thumbnail refresh took, in microseconds. `push` is the capture
    /// thread's critical path, and the thumbnail is the largest part of it that scales with the
    /// image, so it is reported next to `push_us` rather than guessed at from the code.
    pub(crate) fn last_thumb_us(&self) -> u64 {
        self.last_thumb_us
    }

    /// Measure `frame` against the reference and place it.
    pub(crate) fn push(&mut self, frame: &[u8]) -> Step {
        let current = FrameSig::of(frame, self.width, self.view);
        let estimate = estimate(&self.reference, &current);
        let placement = match estimate {
            Estimate::Shift { dy, .. } => {
                let placement = self.stitcher.place(frame, dy);
                match placement {
                    Placement::Appended { band, .. } => {
                        let rows = self.stitcher.rows();
                        self.refresh_thumb(rows - band, rows);
                    }
                    Placement::Prepended { band, .. } => self.refresh_thumb(0, band),
                    Placement::Contained | Placement::Full => {}
                }
                if placement == Placement::Full {
                    // The reference stays: the viewport did not move in the image, so the next
                    // frame is still measured against the last frame that was placed. A refusal
                    // with a store error behind it is a full disk, not the row cap; saying "the
                    // limit" there would send the user looking for a limit that did not arrive.
                    self.status = if self.stitcher.storage_error().is_some() {
                        Status::Storage
                    } else {
                        Status::Full
                    };
                } else {
                    self.reference = current;
                    self.status = Status::Tracking;
                }
                Some(placement)
            }
            Estimate::Still { .. } => {
                // Follow what changed in place (a caret, an animation) so it never piles up into
                // a frame too different to measure.
                self.reference = current;
                if self.status != Status::Full {
                    self.status = Status::Tracking;
                }
                None
            }
            Estimate::Unknown { .. } => {
                if self.status != Status::Full {
                    self.status = Status::Lost;
                }
                None
            }
        };
        Step {
            estimate,
            placement,
        }
    }

    fn refresh_thumb(&mut self, from: u32, to: u32) {
        let (rows, top) = (self.stitcher.rows(), self.stitcher.top());
        let started = std::time::Instant::now();
        // A thumbnail row averages `k` canvas rows, so the band plus one thumbnail row either side
        // is everything the refresh can read (docs/35 §3.5, I4). Asking for the whole canvas would
        // defeat the point of the backing store.
        let k = self.thumb.factor();
        let first = from.saturating_sub(k);
        let last = (to + k).min(rows);
        let bytes = ((last - first) as usize) * (self.width as usize * 4);
        self.band.clear();
        self.band.resize(bytes, 0);
        if let Err(error) = self.stitcher.store().read_rows(first, last - first, &mut self.band) {
            // A stale preview is a cosmetic problem; a zeroed one would look like content.
            eprintln!("[snapclip][scroll] the preview cannot be refreshed: {error}");
            return;
        }
        self.thumb.refresh(&self.band, first, rows, top, from, to);
        self.last_thumb_us = started.elapsed().as_micros() as u64;
    }

    /// At most `max_rows` thumbnail rows: all of them when they fit, otherwise the window whose
    /// bottom is the viewport's bottom (the tail while scrolling down).
    ///
    /// `bytes` is the output buffer: a publishing session hands back the buffer of the previous
    /// frame, so the 616 KiB window is gathered into an existing allocation instead of a new one
    /// every frame (17 us against 121 us at the field size - an allocation that size goes out to the
    /// kernel and back). The returned `Preview` owns the buffer; a caller that wants it again takes
    /// it out of `Preview::bytes` when it is done with the pixels.
    pub(crate) fn fill_preview(&self, max_rows: u32, mut bytes: Vec<u8>) -> Preview {
        let k = i64::from(self.thumb.factor());
        let first = self.thumb.first();
        let total = self.thumb.rows();
        let content_top = self.stitcher.top() + i64::from(self.stitcher.pos());
        let view_start = (content_top.div_euclid(k) - first).clamp(0, i64::from(total)) as u32;
        let view_end = ((content_top + i64::from(self.view) + k - 1).div_euclid(k) - first)
            .clamp(0, i64::from(total)) as u32;
        let shown = max_rows.clamp(1, total.max(1)).min(total);
        let end = view_end.clamp(shown, total);
        let start = end - shown;
        let window = self.thumb.window(start, shown);
        bytes.clear();
        bytes.extend_from_slice(window);
        Preview {
            width: self.thumb.width(),
            rows: shown,
            bytes,
            viewport: (
                view_start.saturating_sub(start).min(shown),
                view_end.saturating_sub(start).min(shown),
            ),
            image_rows: self.stitcher.rows(),
        }
    }

    /// Walk the long image as row bands, first row first, so a consumer can write it out without
    /// ever holding the whole image (docs/35 §13). `f` gets `(first_row, bgra)` and returns
    /// whether to keep going.
    ///
    /// Gated with the probe until the exporter reads from it: an interface ahead of its consumer
    /// is dead code, and the stage that wires the exporter is the one that lifts this.
    #[cfg(test)]
    pub(crate) fn for_each_band(
        &self,
        band_rows: u32,
        mut f: impl FnMut(u32, &[u8]) -> bool,
    ) -> Result<(), crate::scroll::store::RowReadError> {
        let rows = self.stitcher.rows();
        let row_bytes = self.width as usize * 4;
        let band_rows = band_rows.max(1);
        let mut buffer = Vec::new();
        let mut first = 0;
        while first < rows {
            let count = (rows - first).min(band_rows);
            buffer.resize(count as usize * row_bytes, 0);
            self.stitcher.store().read_rows(first, count, &mut buffer)?;
            if !f(first, &buffer) {
                return Ok(());
            }
            first += count;
        }
        Ok(())
    }

    /// How many rows the long image has.
    pub(crate) fn rows(&self) -> u32 {
        self.stitcher.rows()
    }

    /// Fill `out` with `rows` rows starting at `first_row`, top-down, tightly packed BGRA.
    /// False when the store cannot supply them, so a consumer refuses the artifact rather
    /// than writing a short one (docs/35 §15).
    pub(crate) fn read_rows(
        &self,
        first_row: u32,
        rows: u32,
        out: &mut [u8],
    ) -> Result<(), crate::scroll::store::RowReadError> {
        let row_bytes = self.width as usize * 4;
        if rows == 0
            || first_row + rows > self.stitcher.rows()
            || out.len() != rows as usize * row_bytes
        {
            return Err(crate::scroll::store::RowReadError::OutOfRange);
        }
        self.stitcher.store().read_rows(first_row, rows, out)
    }

    /// Arm one failing seal, so a full disk can be tested without one (docs/35 §12.4).
    #[cfg(test)]
    pub(crate) fn fail_seals(&mut self) {
        self.stitcher.fail_seals();
    }

    /// Test hook: break the sealed tile file (docs/36 §12).
    #[cfg(test)]
    pub(crate) fn break_tile_file(&mut self) {
        self.stitcher.break_tile_file();
    }

    /// The long image: `(width, rows, bgra)`.
    pub(crate) fn into_image(self) -> Result<(u32, u32, Vec<u8>), crate::scroll::store::RowReadError> {
        let rows = self.stitcher.rows();
        let width = self.width;
        let mut store = self.stitcher.into_store();
        Ok((width, rows, store.take_all()?))
    }

    /// What the image costs, as its store sees it (docs/35 §5.1).
    pub(crate) fn budgets(&self) -> crate::scroll::store::Budgets {
        self.stitcher.budgets()
    }

    /// What the canvas has moved so far (docs/35 §2.2). Measurement only.
    #[cfg(feature = "test-support")]
    pub(crate) fn move_counters(&self) -> stitch::MoveCounters {
        self.stitcher.counters()
    }
}

#[cfg(test)]
pub(crate) use tests::test_bands;

#[cfg(test)]
mod tests {
    /// Bands backed by a real tracker over a synthetic page, for tests that need a long image to
    /// hand the exporter (`docs/35 §15`). The step sizes mirror `the_bands_add_up_to_the_whole_image`
    /// so the estimator keeps tracking instead of calling a jump.
    pub(crate) fn test_bands() -> ScrollBands {
        let page = page(W, 5000, 22);
        let mut tracker = Tracker::new(&frame_at(&page, 0, 0), W, H, 40);
        for (tick, top) in [64u32, 128, 192, 256, 320].into_iter().enumerate() {
            tracker.push(&frame_at(&page, top, tick as u32 + 1));
            assert_eq!(tracker.status(), Status::Tracking, "the sequence must keep tracking");
        }
        ScrollBands::new(tracker)
    }


    /// The store used to zero the rows it could not read and still report success, which is how a
    /// picture ends up with a hole in it (docs/36 §12.1).
    ///
    /// This test is written the wrong way round **on purpose**: it first proves that the hole
    /// happens, because "the code fills zeros" is not proof that a user would ever get one. Once
    /// the reader reports failure it is flipped to demand the failure.
    #[test]
    fn a_broken_tile_file_is_not_reported_as_success() {
        use crate::ports::BandSource;

        let mut bands = test_bands();
        let rows = bands.rows();
        let row_bytes = bands.width() as usize * 4;
        let mut patient = vec![0u8; rows as usize * row_bytes];
        assert!(bands.read_rows(0, rows, &mut patient), "a healthy read");
        assert!(
            patient.chunks(4).any(|px| px != &patient[..4]),
            "the page is uniform, so a zeroed row would be indistinguishable"
        );

        // The file a crash or a cleaner would take away.
        bands.break_tile_file();

        let mut after = vec![0u8; rows as usize * row_bytes];
        let ok = bands.read_rows(0, rows, &mut after);
        // The fix: the port refuses, so the writer refuses the artifact instead of publishing a
        // picture with a hole in it. Before the fix this assertion was `assert!(ok)` plus a
        // comparison that showed the returned rows were zeros.
        assert!(!ok, "the port must refuse rows it could not read");
    }

    use super::estimate::tests::{page, shift_of, window};
    use super::estimate::{FrameSig, estimate};
    use super::stitch::Placement;
    use super::{MAX_SCREENS, Status, Tracker, row_limit};
    use super::ScrollBands;

    const W: u32 = 120;
    const H: u32 = 200;
    const HEADER: u32 = 30;
    const FOOTER: u32 = 20;

    /// A small deterministic generator, so the scroll sequence is "random" but reproducible.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, below: u32) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) % u64::from(below)) as u32
        }
    }

    /// The viewport at page offset `top`, with a fixed header and footer and, optionally, a
    /// blinking caret that differs on every frame (the "live page" fact).
    fn frame_at(page: &[u8], top: u32, tick: u32) -> Vec<u8> {
        let mut frame = window(page, W, top, H);
        let row = (W * 4) as usize;
        for y in (0..HEADER).chain(H - FOOTER..H) {
            for x in 0..W {
                let at = y as usize * row + x as usize * 4;
                frame[at..at + 4].copy_from_slice(&[x as u8, y as u8, 99, 255]);
            }
        }
        // The caret sits in the header, so it never touches page content the truth compares.
        let caret = 10 + (tick % 2) * 40;
        for y in 5..20 {
            let at = y as usize * row + caret as usize * 4;
            frame[at..at + 8].copy_from_slice(&[0, 0, 0, 255, 0, 0, 0, 255]);
        }
        frame
    }

    fn truth(page: &[u8], last: u32, last_tick: u32) -> Vec<u8> {
        let row = (W * 4) as usize;
        let mut out = frame_at(page, 0, 0)[..HEADER as usize * row].to_vec();
        out.extend_from_slice(&page[HEADER as usize * row..(last + H - FOOTER) as usize * row]);
        out.extend_from_slice(&frame_at(page, last, last_tick)[(H - FOOTER) as usize * row..]);
        out
    }

    #[test]
    fn a_random_live_scroll_of_a_long_page_stitches_to_the_page_byte_for_byte() {
        let page = page(W, 5000, 21);
        let mut rng = Lcg(7);
        let mut tracker = Tracker::new(&frame_at(&page, 0, 0), W, H, 40);
        let (mut top, mut tick) = (0u32, 0u32);
        // Steps a wheel and a smooth scroll produce: zero (a live but unscrolled frame), a few
        // rows, a notch. Never more than the content area can overlap.
        while top < 3000 {
            tick += 1;
            top += [0, 1, 3, 17, 40, 64, 90][rng.next(7) as usize];
            tracker.push(&frame_at(&page, top, tick));
            assert_eq!(tracker.status(), Status::Tracking, "lost at offset {top}");
        }
        assert_eq!(tracker.image_rows(), top + H);
        let (width, rows, image) = tracker.into_image().expect("the tiles are intact");
        assert_eq!((width, rows), (W, top + H));
        assert!(
            image == truth(&page, top, tick),
            "the image must equal the page with one header and one footer"
        );
    }

    #[test]
    fn the_band_source_reproduces_the_whole_image() {
        let (width, rows, whole) = test_bands().into_image().expect("the tiles are intact");
        assert_eq!(width, W);
        let mut bands = test_bands();
        let total = crate::ports::BandSource::rows(&bands);
        assert_eq!(total, rows, "the two trackers must agree on the height");
        let mut replayed = Vec::new();
        let mut first = 0u32;
        while first < total {
            let count = (total - first).min(64);
            let mut out = Vec::new();
            assert!(
                crate::ports::BandSource::read_rows(&mut bands, first, count, &mut out),
                "bands must supply row {first}"
            );
            replayed.extend_from_slice(&out);
            first += count;
        }
        assert_eq!(replayed, whole, "band replay must equal the whole image");
    }

    #[test]
    fn a_failed_seal_is_reported_as_storage_not_as_the_row_cap() {
        let page = page(W, 5000, 22);
        let mut tracker = Tracker::new(&frame_at(&page, 0, 0), W, H, 40);
        tracker.fail_seals();
        // Scroll far enough that a tile has to be sealed, which is where the armed failure fires.
        // Small steps, like a real wheel: a single 700-row jump is what the estimator calls a
        // jump, and `Lost` would end the loop before any tile was sealed.
        let mut pushes = 0;
        for (tick, top) in (1..=18u32).map(|n| n * 64).enumerate() {
            tracker.push(&frame_at(&page, top, tick as u32 + 1));
            pushes += 1;
            let status = tracker.status();
            if status == Status::Full || status == Status::Storage {
                break;
            }
            assert_eq!(status, Status::Tracking, "the sequence must keep tracking");
        }
        assert_eq!(
            tracker.status(),
            Status::Storage,
            "a failed write must not be reported as the row cap (after {pushes} pushes)"
        );
        assert!(tracker.stitcher.storage_error().is_some());
    }

    #[test]
    fn the_bands_add_up_to_the_whole_image() {
        let page = page(W, 5000, 22);
        let build = || {
            let mut tracker = Tracker::new(&frame_at(&page, 0, 0), W, H, 40);
            for (tick, top) in [64u32, 128, 192, 256, 320].into_iter().enumerate() {
                tracker.push(&frame_at(&page, top, tick as u32 + 1));
            }
            tracker
        };
        let mut banded = Vec::new();
        {
            let tracker = build();
            tracker
                .for_each_band(64, |first, bytes| {
                    assert_eq!(
                        first as usize * W as usize * 4,
                        banded.len(),
                        "the bands arrive in order, without gaps"
                    );
                    banded.extend_from_slice(bytes);
                    true
                })
                .expect("the tiles are intact");
        }
        let (width, rows, image) = build().into_image().expect("the tiles are intact");
        assert_eq!(width, W);
        assert_eq!(banded.len(), image.len(), "no band was skipped");
        assert_eq!(banded, image, "the bands are the image, in order");
        assert!(rows > 0);
    }

    #[test]
    fn a_jump_is_reported_as_lost_and_scrolling_back_recovers() {
        let page = page(W, 5000, 22);
        let mut tracker = Tracker::new(&frame_at(&page, 0, 0), W, H, 40);
        tracker.push(&frame_at(&page, 60, 1));
        tracker.push(&frame_at(&page, 600, 2));
        assert_eq!(tracker.status(), Status::Lost);
        tracker.push(&frame_at(&page, 100, 3));
        assert_eq!(tracker.status(), Status::Tracking);
        assert_eq!(tracker.image_rows(), 100 + H);
    }

    #[test]
    fn the_image_stops_at_its_cap_and_says_so() {
        let page = page(W, MAX_SCREENS * H + 2 * H, 23);
        let mut tracker = Tracker::new(&frame_at(&page, 0, 0), W, H, 40);
        let mut top = 0;
        while top + 80 + H <= page.len() as u32 / (W * 4) {
            top += 80;
            tracker.push(&frame_at(&page, top, 0));
        }
        assert_eq!(tracker.status(), Status::Full);
        assert!(tracker.image_rows() <= MAX_SCREENS * H);
    }

    /// A row cap is not a byte cap: a selection wide enough to make 32 screens cost more than the
    /// budget must stop on the budget, not on the screen count.
    #[test]
    fn the_byte_budget_can_be_the_cap_that_stops_the_image() {
        const BUDGET: usize = 64 * 1024 * 1024;
        const VIEW: u32 = 1200;
        let (w, view) = (1024u32, VIEW);
        // 1024 × 4 = 4096 bytes a row, so the budget is 64 MiB / 4096 = 16384 rows, well below the
        // 57600 rows that `MAX_SCREENS` alone would allow.
        assert_eq!(row_limit(w, view, BUDGET), 16384);
        assert!(row_limit(w, view, BUDGET) < MAX_SCREENS * view);

        let page = page(w, 17_580, 91);
        // `Tracker::new` uses the real budget, so the test drives the cap through the same row
        // limit the tracker would compute: `MAX_BYTES / row` min the screen cap.
        let limit = row_limit(w, view, BUDGET);
        let mut tracker = Tracker::with_rows(&window(&page, w, 0, view), w, view, 40, limit);
        // Push until one more 100-row step would not fit: `limit` binds before the page ends.
        let mut step = 0;
        while tracker.image_rows() + 100 <= limit {
            step += 1;
            tracker.push(&window(&page, w, step * 100, view));
        }
        // 16300 rows is where the budget stops the image: the next 100-row push would pass the
        // 16384-row limit, so it is refused and the canvas and the status both say so.
        assert_eq!(tracker.image_rows(), 16300, "step {step}");
        assert!(tracker.image_rows() <= limit);
        let refused = tracker.push(&window(&page, w, (step + 1) * 100, view));
        assert_eq!(refused.placement, Some(Placement::Full));
        assert_eq!(tracker.status(), Status::Full);
        assert_eq!(tracker.image_rows(), 16300, "a refused push changes nothing");
        let still_refused = tracker.push(&window(&page, w, (step + 2) * 100, view));
        assert_eq!(still_refused.placement, Some(Placement::Full));
    }

    #[test]
    fn the_preview_shows_the_tail_while_scrolling_down_and_marks_the_viewport() {
        let page = page(W, 5000, 24);
        // W = 120 into 40 → factor 3, thumbnail rows are 3 canvas rows each.
        let mut tracker = Tracker::new(&frame_at(&page, 0, 0), W, H, 40);
        let first = tracker.fill_preview(1000, Vec::new());
        assert_eq!((first.width, first.rows), (40, 67));
        assert_eq!(first.viewport, (0, 67));
        for top in (60..=900).step_by(60) {
            tracker.push(&frame_at(&page, top, 0));
        }
        let preview = tracker.fill_preview(100, Vec::new());
        assert_eq!(preview.rows, 100);
        assert_eq!(preview.bytes.len(), (40 * 100 * 4) as usize);
        assert_eq!(preview.image_rows, 900 + H);
        // The viewport is the last H rows, so it ends at the window's bottom.
        assert_eq!(preview.viewport, (100 - 67, 100));
    }

    /// Cost evidence for the whole per-frame path at the field size (`docs/33` §4.1).
    ///
    /// One `push` is `FrameSig::of` (8·H hashes over the selection) + `estimate` (the vote) +
    /// `stitcher.place` + the thumbnail band; `preview` is what the capture thread sends to the
    /// overlay. Reported, not asserted — debug builds and loaded machines vary too much for a
    /// threshold, and the split below is what says where a real frame goes. The split repeats
    /// `push`'s own sequence (`estimate`/`place`/`refresh_thumb` are crate-private), so it is a
    /// guide to the total above it, not a second implementation.
    #[test]
    #[ignore = "timing report: run with --release --include-ignored --nocapture"]
    fn the_cost_of_one_frame_at_the_field_size() {
        let (w, h) = (2148u32, 1344u32);
        let page = page(w, 8 * h, 31);
        let mut tracker = Tracker::new(&window(&page, w, 0, h), w, h, 160);
        // 6400 rows down in 97-row steps, then back up: both the append and the prepend path.
        let mut tops: Vec<u32> = (1..=66).map(|step| step * 97).collect();
        tops.extend((0..33).rev().map(|step| step * 97));

        let mut frame_us = Vec::new();
        let mut signature_us = Vec::new();
        let mut estimate_us = Vec::new();
        let mut place_us = Vec::new();
        let mut thumb_us = Vec::new();
        let mut preview_us = Vec::new();
        let mut preview_bytes = 0;
        // The publishing session's own recycling: the preview buffer goes back in on every frame.
        let mut recycled: Vec<u8> = Vec::new();
        for top in tops {
            let frame = window(&page, w, top, h);
            let started = std::time::Instant::now();
            tracker.push(&frame);
            frame_us.push(started.elapsed().as_micros());
            thumb_us.push(tracker.last_thumb_us as u128);

            let started = std::time::Instant::now();
            let signature = FrameSig::of(&frame, w, h);
            signature_us.push(started.elapsed().as_micros());
            let started = std::time::Instant::now();
            let measured = estimate(&tracker.reference, &signature);
            estimate_us.push(started.elapsed().as_micros());
            let started = std::time::Instant::now();
            let placement = tracker.stitcher.place(&frame, shift_of(measured).unwrap_or(0));
            place_us.push(started.elapsed().as_micros());
            if let Placement::Appended { band, .. } = placement {
                let rows = tracker.stitcher.rows();
                tracker.refresh_thumb(rows - band, rows);
            }
            let started = std::time::Instant::now();
            let preview = tracker.fill_preview(160, std::mem::take(&mut recycled));
            preview_bytes = preview.bytes.len();
            recycled = preview.bytes;
            preview_us.push(started.elapsed().as_micros());
        }
        let stats = |name: &str, costs: &mut Vec<u128>| {
            costs.sort_unstable();
            println!(
                "frame {w}x{h} {name}: p50_us={} p95_us={} max_us={} samples={}",
                costs[costs.len() / 2],
                costs[costs.len() * 95 / 100],
                costs[costs.len() - 1],
                costs.len()
            );
        };
        println!("preview bytes={preview_bytes}");
        stats("signature", &mut signature_us);
        stats("estimate", &mut estimate_us);
        stats("place", &mut place_us);
        stats("thumb", &mut thumb_us);
        stats("preview", &mut preview_us);
        stats("push", &mut frame_us);
    }

    #[test]
    fn the_preview_follows_the_viewport_back_up() {
        let page = page(W, 5000, 25);
        let mut tracker = Tracker::new(&frame_at(&page, 0, 0), W, H, 40);
        for top in (60..=900).step_by(60) {
            tracker.push(&frame_at(&page, top, 0));
        }
        for top in (0..=840).rev().step_by(60) {
            tracker.push(&frame_at(&page, top, 0));
        }
        let preview = tracker.fill_preview(100, Vec::new());
        // Back at the top: the window starts at the image's first row.
        assert_eq!(preview.viewport.0, 0);
        assert_eq!(preview.image_rows, 900 + H);
    }
}

/// The measurement surface `tools/codec-bench --scroll` drives (docs/34 §12).
///
/// Compiled only with `--features test-support`, so the product build is untouched: the code path
/// underneath is the same `Tracker` the capture thread pushes frames into. It exists because the
/// cost that matters for a long scroll is the *canvas*, not the encoder, and that number can only
/// be honest if the real stitcher is the thing being measured.
#[cfg(feature = "test-support")]
pub mod probe {
    use super::stitch::CanvasStore;
    use super::{Status, Tracker};

    /// What one pushed frame cost, and where the canvas ended up.
    #[derive(Debug, Clone, Copy)]
    pub struct Frame {
        /// Time in `Tracker::push` with the thumbnail refresh subtracted.
        pub stitch_us: u64,
        /// Time the thumbnail refresh took.
        pub thumb_us: u64,
        pub image_rows: u32,
        pub placed: bool,
        pub tracking: bool,
        pub lost: bool,
        pub full: bool,
        /// What this one frame moved, so the slowest frame can be attributed rather than guessed
        /// (docs/35 §7.1): the canvas reallocated, shifted for a prepend, or overwrote a band.
        pub grow_moves: u32,
        pub grow_bytes: u64,
        pub shift_bytes: u64,
        pub band_bytes: u64,
        pub appended: bool,
        pub prepended: bool,
    }

    /// The three move sites' running totals (docs/35 §2.2).
    pub use super::store::MoveCounters;

    /// Drives the real tracker either at the product's own limits or at an explicit row cap.
    pub struct ScrollProbe {
        tracker: Tracker,
    }

    impl ScrollProbe {
        /// The product's own limits: `MAX_SCREENS` and `MAX_BYTES`.
        pub fn new(first: &[u8], width: u32, view: u32, thumb_width: u32) -> Self {
            Self {
                tracker: Tracker::new(first, width, view, thumb_width),
            }
        }

        /// An explicit canvas row cap, for measuring where the cap bites.
        pub fn with_row_cap(
            first: &[u8],
            width: u32,
            view: u32,
            thumb_width: u32,
            max_rows: u32,
        ) -> Self {
            Self {
                tracker: Tracker::with_rows(first, width, view, thumb_width, max_rows),
            }
        }

        /// Content coordinate of canvas row 0: negative once the session scrolled above the frame
        /// it started from, which is how a measurement compares a canvas with the page (docs/35).
        pub fn top(&self) -> i64 {
            self.tracker.stitcher.top()
        }

        /// The row cap the product would apply to this selection.
        pub fn row_cap(width: u32, view: u32) -> u32 {
            super::row_limit(width, view, super::MAX_BYTES)
        }

        /// The screen cap that a tall-enough viewport would run into first, in rows.
        pub fn screen_cap(view: u32) -> u32 {
            super::MAX_SCREENS * view
        }

        /// Kept for the phase 0 attribution run, which measured the *old* store: the backed store
        /// has nothing to reserve, so this is a no-op there (docs/35-baseline.md keeps its table).
        pub fn preallocate(&mut self, _rows: u32) {}

        /// What the canvas has moved so far: the three attribution candidates (docs/35 §2.2).
        pub fn counters(&self) -> super::stitch::MoveCounters {
            self.tracker.move_counters()
        }

        pub fn push(&mut self, frame: &[u8]) -> Frame {
            let before = self.tracker.move_counters();
            let started = std::time::Instant::now();
            let step = self.tracker.push(frame);
            let total_us = started.elapsed().as_micros() as u64;
            let after = self.tracker.move_counters();
            let status = self.tracker.status();
            Frame {
                stitch_us: total_us.saturating_sub(self.tracker.last_thumb_us()),
                thumb_us: self.tracker.last_thumb_us(),
                image_rows: self.tracker.image_rows(),
                placed: step.placement.is_some(),
                tracking: status == Status::Tracking,
                lost: status == Status::Lost,
                full: status == Status::Full,
                grow_moves: after.grow_moves - before.grow_moves,
                grow_bytes: after.grow_bytes - before.grow_bytes,
                shift_bytes: after.shift_bytes - before.shift_bytes,
                band_bytes: after.band_bytes - before.band_bytes,
                appended: matches!(step.placement, Some(super::stitch::Placement::Appended { .. })),
                prepended: matches!(
                    step.placement,
                    Some(super::stitch::Placement::Prepended { .. })
                ),
            }
        }

        pub fn image_rows(&self) -> u32 {
            self.tracker.image_rows()
        }

        /// `(seal count, largest seal in rows, bytes written)` for the seal-cost measurement
        /// (docs/35 §17.1).
        pub fn seal_stats(&self) -> (u64, u32, u64) {
            self.tracker.stitcher.store().seal_stats()
        }

        /// The four budgets the store answers for, so a run can report what the image cost
        /// (docs/35 §5.1): `(pixel_rows, pixel_bytes, working_bytes, temp_bytes)`.
        pub fn budgets(&self) -> (u32, u64, u64, u64) {
            let budgets = self.tracker.budgets();
            (
                budgets.pixel_rows,
                budgets.pixel_bytes,
                budgets.working_bytes,
                budgets.temp_bytes,
            )
        }

        /// The whole image, gathered from wherever its rows live.
        pub fn image(&self) -> Vec<u8> {
            let rows = self.tracker.stitcher.rows();
            let mut out = vec![0u8; (self.tracker.width() * rows * 4) as usize];
            self.tracker.stitcher.store().read_rows(0, rows, &mut out).expect("rows");
            out
        }

        /// The long image: `(width, rows, bgra)`.
        pub fn into_image(self) -> Result<(u32, u32, Vec<u8>), crate::scroll::store::RowReadError> {
            self.tracker.into_image()
        }
    }
}

#[cfg(test)]
mod scroll_format_tests {
    use super::{MAX_SCREENS, scroll_format};
    use crate::save_format::{Lowering, SaveFormat};

    #[test]
    fn webp_is_lowered_before_a_tall_scroll_starts() {
        // 32 x 960 rows is past 16383, so this capture can never be a single WebP.
        let (effective, why) = scroll_format(SaveFormat::Webp, 1280, 960);
        assert_eq!(effective, SaveFormat::Png);
        assert!(matches!(why, Some(Lowering::Format(_))), "{why:?}");
    }

    #[test]
    fn a_short_view_keeps_webp_and_a_tall_one_does_not() {
        // The potential rows are MAX_SCREENS times the viewport, so the WebP boundary moves with
        // the screen cap: with the cap at 48, a viewport of 341 is the tallest that still fits the
        // format's 16383 rows. Derived here rather than written down, so the next cap change reads
        // as a boundary shift and not as a broken test.
        let fits = 16_383 / super::MAX_SCREENS as u32; // 341
        assert_eq!(
            scroll_format(SaveFormat::Webp, 1280, fits),
            (SaveFormat::Webp, None)
        );
        let (effective, why) = scroll_format(SaveFormat::Webp, 1, fits + 1);
        assert_eq!(effective, SaveFormat::Png);
        assert!(why.is_some());
    }

    #[test]
    fn png_is_never_lowered() {
        assert_eq!(scroll_format(SaveFormat::Png, 3840, 2100), (SaveFormat::Png, None));
        assert_eq!(scroll_format(SaveFormat::Png, 1280, 960), (SaveFormat::Png, None));
    }

    #[test]
    fn jpeg_is_lowered_when_the_frame_would_not_fit_its_budget() {
        // MAX_SCREENS x 480 rows of 640 pixels stays inside the 16 MP JPG frame budget.
        assert_eq!(scroll_format(SaveFormat::Jpeg, 640, 480), (SaveFormat::Jpeg, None));
        // The same rows at 1280 wide is 19.7 MP, so the JPG save path would have to hold the frame.
        let (effective, why) = scroll_format(SaveFormat::Jpeg, 1280, 480);
        assert_eq!(effective, SaveFormat::Png);
        assert!(matches!(why, Some(Lowering::FrameBudget { .. })), "{why:?}");
        // 32 x 960 = 30720 rows: the scroll capture this rule exists for, and it stays PNG.
        let (effective, why) = scroll_format(SaveFormat::Jpeg, 1280, 960);
        assert_eq!(effective, SaveFormat::Png);
        assert!(matches!(why, Some(Lowering::FrameBudget { .. })), "{why:?}");
    }

    #[test]
    fn jpeg_still_reports_the_formats_own_limit_first() {
        // MAX_SCREENS x 2100 rows is past JPEG's 65535 per side: a format limit, not a budget.
        let (effective, why) = scroll_format(SaveFormat::Jpeg, 1280, 2100);
        assert_eq!(effective, SaveFormat::Png);
        assert!(matches!(why, Some(Lowering::Format(_))), "{why:?}");
        assert_eq!(MAX_SCREENS, 48);
    }

    #[test]
    fn the_scroll_log_prints_on_a_change_and_then_every_sixty_frames() {
        use super::{should_log, Status};
        // 首帧必打（还没有"上一次状态"）。
        assert!(should_log(2, Status::Tracking, None));
        // 状态变化必打，不论帧号。
        assert!(should_log(3, Status::Lost, Some(Status::Tracking)));
        assert!(should_log(4, Status::Full, Some(Status::Lost)));
        assert!(should_log(5, Status::Storage, Some(Status::Full)));
        // 状态不变时按帧号节流，且第 60 / 120 帧会打。
        assert!(!should_log(6, Status::Full, Some(Status::Full)));
        assert!(!should_log(59, Status::Full, Some(Status::Full)));
        assert!(should_log(60, Status::Full, Some(Status::Full)));
        assert!(!should_log(61, Status::Full, Some(Status::Full)));
        assert!(should_log(120, Status::Full, Some(Status::Full)));
        // 一帧最多打一行：纯函数，同一输入只有一个答案。
        assert_eq!(
            should_log(120, Status::Full, Some(Status::Full)),
            should_log(120, Status::Full, Some(Status::Full))
        );
    }
}

/// A finished long image handed to the exporter as row bands, so thirty thousand rows reach the
/// writer without the whole picture ever being in memory (docs/35 §15).
///
/// The tracker already owns a temp tile file and a bounded edge ring; this wrapper adds only the
/// width and rows a consumer needs to size its bands.
pub(crate) struct ScrollBands {
    tracker: Tracker,
}

impl ScrollBands {
    pub(crate) fn new(tracker: Tracker) -> Self {
        Self { tracker }
    }

    pub(crate) fn width(&self) -> u32 {
        self.tracker.width()
    }

    /// The whole picture, for the one consumer that cannot stream: the clipboard wants a
    /// contiguous bitmap, and about 145 MB at thirty thousand rows is what that costs. It is the
    /// clipboard's price, not a leak (docs/35 §15).
    pub(crate) fn into_image(
        self,
    ) -> Result<(u32, u32, Vec<u8>), crate::scroll::store::RowReadError> {
        self.tracker.into_image()
    }

    /// Test hook: break the sealed tile file behind these bands (docs/36 §12).
    #[cfg(test)]
    pub(crate) fn break_tile_file(&mut self) {
        self.tracker.break_tile_file();
    }
}

impl crate::ports::BandSource for ScrollBands {
    fn rows(&self) -> u32 {
        self.tracker.rows()
    }

    fn read_rows(&mut self, first_row: u32, rows: u32, out: &mut Vec<u8>) -> bool {
        let row_bytes = self.tracker.width() as usize * 4;
        out.resize(rows as usize * row_bytes, 0);
        match self.tracker.read_rows(first_row, rows, out) {
            Ok(()) => true,
            Err(error) => {
                // The writer refuses the artifact on `false`; saying so here is what makes a broken
                // tile file a failed export instead of a picture with a hole (docs/36 §12.1).
                eprintln!(
                    "[snapclip][scroll] rows {first_row}..+{rows} cannot be read: {error}"
                );
                false
            }
        }
    }
}
