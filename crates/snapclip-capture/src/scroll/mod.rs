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

pub(crate) mod estimate;
pub(crate) mod layout;
pub(crate) mod stitch;
pub(crate) mod thumb;

use estimate::{Estimate, FrameSig, estimate};
use stitch::{Placement, Stitcher};
use thumb::Thumb;

/// The long image is capped at this many selection heights (the reference implementation's cap).
pub(crate) const MAX_SCREENS: u32 = 32;
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
) -> (crate::save_format::SaveFormat, Option<crate::save_format::FormatLimit>) {
    let rows = row_limit(width, view, MAX_BYTES);
    match preferred.can_encode(u64::from(width), u64::from(rows)) {
        Ok(()) => (preferred, None),
        Err(limit) => (crate::save_format::SaveFormat::Png, Some(limit)),
    }
}

#[derive(Debug)]
pub(crate) struct Tracker {
    width: u32,
    view: u32,
    reference: FrameSig,
    stitcher: Stitcher,
    thumb: Thumb,
    status: Status,
    /// Scratch: how long the last `push`'s thumbnail refresh took, in microseconds.
    last_thumb_us: u64,
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
        let stitcher = Stitcher::new(first, width, view, max_rows);
        let mut thumb = Thumb::new(width, thumb_width);
        thumb.refresh(stitcher.canvas(), view, 0, 0, view);
        Self {
            width,
            view,
            reference: FrameSig::of(first, width, view),
            stitcher,
            thumb,
            status: Status::Tracking,
            last_thumb_us: 0,
        }
    }

    pub(crate) fn status(&self) -> Status {
        self.status
    }

    pub(crate) fn image_rows(&self) -> u32 {
        self.stitcher.rows()
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
                    // frame is still measured against the last frame that was placed.
                    self.status = Status::Full;
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
        self.thumb
            .refresh(self.stitcher.canvas(), rows, top, from, to);
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

    /// The long image: `(width, rows, bgra)`.
    pub(crate) fn into_image(self) -> (u32, u32, Vec<u8>) {
        (
            self.width,
            self.stitcher.rows(),
            self.stitcher.into_canvas(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::estimate::tests::{page, shift_of, window};
    use super::estimate::{FrameSig, estimate};
    use super::stitch::Placement;
    use super::{MAX_SCREENS, Status, Tracker, row_limit};

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
        let (width, rows, image) = tracker.into_image();
        assert_eq!((width, rows), (W, top + H));
        assert!(
            image == truth(&page, top, tick),
            "the image must equal the page with one header and one footer"
        );
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
        // 38399 rows that `MAX_SCREENS` alone would allow.
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
    }

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

        /// The row cap the product would apply to this selection.
        pub fn row_cap(width: u32, view: u32) -> u32 {
            super::row_limit(width, view, super::MAX_BYTES)
        }

        /// The screen cap that a tall-enough viewport would run into first, in rows.
        pub fn screen_cap(view: u32) -> u32 {
            super::MAX_SCREENS * view
        }

        pub fn push(&mut self, frame: &[u8]) -> Frame {
            let started = std::time::Instant::now();
            let step = self.tracker.push(frame);
            let total_us = started.elapsed().as_micros() as u64;
            let status = self.tracker.status();
            Frame {
                stitch_us: total_us.saturating_sub(self.tracker.last_thumb_us()),
                thumb_us: self.tracker.last_thumb_us(),
                image_rows: self.tracker.image_rows(),
                placed: step.placement.is_some(),
                tracking: status == Status::Tracking,
                lost: status == Status::Lost,
                full: status == Status::Full,
            }
        }

        pub fn image_rows(&self) -> u32 {
            self.tracker.image_rows()
        }

        pub fn canvas(&self) -> &[u8] {
            self.tracker.stitcher.canvas()
        }

        /// The long image: `(width, rows, bgra)`.
        pub fn into_image(self) -> (u32, u32, Vec<u8>) {
            self.tracker.into_image()
        }
    }
}

#[cfg(test)]
mod scroll_format_tests {
    use super::{MAX_SCREENS, scroll_format};
    use crate::save_format::SaveFormat;

    #[test]
    fn webp_is_lowered_before_a_tall_scroll_starts() {
        // 32 x 960 rows is past 16383, so this capture can never be a single WebP.
        let (effective, why) = scroll_format(SaveFormat::Webp, 1280, 960);
        assert_eq!(effective, SaveFormat::Png);
        assert!(why.is_some());
    }

    #[test]
    fn a_short_view_keeps_webp_and_a_tall_one_does_not() {
        // 32 x 480 = 15360 rows, inside the container.
        assert_eq!(scroll_format(SaveFormat::Webp, 1280, 480), (SaveFormat::Webp, None));
        // 32 x 512 = 16384 rows, one past it.
        let (effective, why) = scroll_format(SaveFormat::Webp, 1, 512);
        assert_eq!(effective, SaveFormat::Png);
        assert!(why.is_some());
        // 32 x 511 = 16352 rows, still inside.
        assert_eq!(scroll_format(SaveFormat::Webp, 1, 511), (SaveFormat::Webp, None));
    }

    #[test]
    fn png_is_never_lowered_and_jpeg_only_past_its_own_limit() {
        assert_eq!(scroll_format(SaveFormat::Png, 3840, 2100), (SaveFormat::Png, None));
        // 32 x 960 = 30720 rows, well inside JPEG's 65535.
        assert_eq!(scroll_format(SaveFormat::Jpeg, 1280, 960), (SaveFormat::Jpeg, None));
        // 32 x 2100 = 67200 rows, past it.
        let (effective, why) = scroll_format(SaveFormat::Jpeg, 1280, 2100);
        assert_eq!(effective, SaveFormat::Png);
        assert!(why.is_some());
        assert_eq!(MAX_SCREENS, 32);
    }
}
