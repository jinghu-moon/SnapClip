//! The long image: frames placed at the content position the estimator measured.
//!
//! The stitcher decides *where* a frame goes; a [`CanvasStore`] decides *how* those rows are held
//! (docs/35 phase 1). Today the only store is one contiguous BGRA buffer, `width` pixels wide, so
//! this module behaves exactly as it did when it owned that buffer itself. `pos` is the canvas row
//! the top of the viewport currently shows. A measured shift moves `pos`; when the viewport leaves
//! the canvas, the store grows by exactly the rows that were never seen before.
//!
//! Growth rewrites a **band** at the growing edge rather than only the new rows: a fixed footer
//! (or, scrolling up, a fixed header) sits at that edge of every frame, and rewriting at least
//! `growth + H/4` rows from the newest frame is what replaces the previous copy of it with real
//! content. Only the last copy survives, where it belongs. The opposite edge is never written, so
//! a fixed bar on that side appears once. The band is the reference's choice (`snow_shot`'s
//! `compositor.rs`): `max(H/2, growth + H/4)`, at most `H`.

pub(crate) use super::store::CanvasStore;
#[cfg(test)]
pub(crate) use super::store::MemoryCanvas;
pub(crate) use super::store::Budgets;
#[cfg(feature = "test-support")]
pub(crate) use super::store::MoveCounters;

/// What one measured shift did to the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    /// The canvas grew at the bottom by `growth` rows; its last `band` rows were rewritten.
    Appended { growth: u32, band: u32 },
    /// The canvas grew at the top by `growth` rows; its first `band` rows were rewritten.
    Prepended { growth: u32, band: u32 },
    /// The viewport moved inside rows that already exist.
    Contained,
    /// Growing would pass the row cap, or the memory for it could not be had; nothing changed.
    Full,
}

/// Places frames into a [`CanvasStore`].
#[derive(Debug)]
pub(crate) struct Stitcher<C: CanvasStore> {
    width: u32,
    view: u32,
    store: C,
    pos: u32,
    /// Content coordinate of canvas row 0: the first frame's top is 0, and every prepend lowers it.
    top: i64,
}

#[cfg(test)]
impl Stitcher<super::backed::BackedCanvas> {
    /// Arm one failing seal, so a full disk can be tested without one (docs/35 §12.4).
    pub(crate) fn fail_seals(&mut self) {
        self.store.fail_seals();
    }
}

impl<C: CanvasStore> Stitcher<C> {
    /// A stitcher over an existing store (docs/35 phase 2 hands in a different one).
    pub(crate) fn with_store(store: C, width: u32, view: u32) -> Self {
        debug_assert_eq!(store.rows(), view, "a store starts with exactly one frame");
        Self {
            width,
            view,
            store,
            pos: 0,
            top: 0,
        }
    }

    pub(crate) fn rows(&self) -> u32 {
        self.store.rows()
    }

    /// The canvas row the viewport's top shows.
    pub(crate) fn pos(&self) -> u32 {
        self.pos
    }

    /// Content coordinate of canvas row 0.
    pub(crate) fn top(&self) -> i64 {
        self.top
    }

    /// What this image costs, as the store sees it (docs/35 §5.1).
    /// Why the store's last write failed, if it did (docs/35 §12.4).
    pub(crate) fn storage_error(&self) -> Option<&str> {
        self.store.storage_error()
    }

    pub(crate) fn budgets(&self) -> Budgets {
        self.store.budgets()
    }

    /// What the three move sites have done so far, when the store has them (docs/35 §2.2).
    #[cfg(feature = "test-support")]
    pub(crate) fn counters(&self) -> MoveCounters {
        self.store.counters().unwrap_or_default()
    }

    /// The store, for phase 2 and for tests that drive one directly.
    pub(crate) fn store(&self) -> &C {
        &self.store
    }

    /// The store, by value, so the caller can ask it for the finished image.
    pub(crate) fn into_store(self) -> C {
        self.store
    }

    /// Place `frame`, whose content moved by `dy` relative to the frame placed last
    /// (positive: the page scrolled down).
    pub(crate) fn place(&mut self, frame: &[u8], dy: i32) -> Placement {
        debug_assert_eq!(frame.len(), (self.width * self.view * 4) as usize);
        let row = (self.width * 4) as usize;
        let view = self.view as i64;
        let cand = i64::from(self.pos) + i64::from(dy);
        let known = i64::from(self.store.rows());
        if cand + view > known {
            let growth = (cand + view - known) as u32;
            let band = self.band(growth) as usize;
            // The last `band` rows of the newest frame replace the previous copy of the edge.
            if !self.store.append(growth, &frame[(self.view as usize - band) * row..]) {
                return Placement::Full;
            }
            self.pos = self.store.rows() - self.view;
            Placement::Appended {
                growth,
                band: band as u32,
            }
        } else if cand < 0 {
            let growth = (-cand) as u32;
            let band = self.band(growth) as usize;
            if !self.store.prepend(growth, &frame[..band * row]) {
                return Placement::Full;
            }
            self.pos = 0;
            self.top -= i64::from(growth);
            Placement::Prepended {
                growth,
                band: band as u32,
            }
        } else {
            self.pos = cand as u32;
            Placement::Contained
        }
    }

    fn band(&self, growth: u32) -> u32 {
        (self.view / 2).max(growth + self.view / 4).min(self.view)
    }
}

/// The in-memory store is the only one there is, and these are the calls the rest of the crate made
/// when the stitcher owned the buffer (docs/35 phase 1: no caller changes).
#[cfg(test)]
impl Stitcher<MemoryCanvas> {
    /// Start from the first frame over the oracle store. Tests only: the product hands in a
    /// [`BackedCanvas`](super::backed::BackedCanvas) through [`Stitcher::with_store`].
    #[cfg(test)]
    pub(crate) fn new(first: &[u8], width: u32, view: u32, max_rows: u32) -> Self {
        Self::with_store(MemoryCanvas::new(first, width, view, max_rows), width, view)
    }

    #[cfg(test)]
    pub(crate) fn canvas(&self) -> &[u8] {
        self.store.slice()
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn preallocate(&mut self, rows: u32) {
        self.store.preallocate(rows);
    }

    #[cfg(test)]
    pub(crate) fn canvas_len(&self) -> usize {
        self.store.slice().len()
    }

    #[cfg(test)]
    pub(crate) fn canvas_capacity(&self) -> usize {
        self.store.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::{CanvasStore, MemoryCanvas, Placement, Stitcher};
    use crate::scroll::estimate::tests::{page, window};
    use crate::scroll::estimate::{Estimate, FrameSig, estimate};

    const W: u32 = 96;
    const H: u32 = 160;
    const HEADER: u32 = 24;
    const FOOTER: u32 = 18;

    /// What the viewport shows at page offset `top`: the page, under a fixed header and footer.
    fn frame_at(page: &[u8], top: u32) -> Vec<u8> {
        let mut frame = window(page, W, top, H);
        let row = (W * 4) as usize;
        for y in 0..HEADER {
            for x in 0..W {
                let at = y as usize * row + x as usize * 4;
                frame[at..at + 4].copy_from_slice(&[x as u8, y as u8, 7, 255]);
            }
        }
        for y in H - FOOTER..H {
            for x in 0..W {
                let at = y as usize * row + x as usize * 4;
                frame[at..at + 4].copy_from_slice(&[y as u8, x as u8, 201, 255]);
            }
        }
        frame
    }

    /// The image a perfect stitch of offsets `0..=last` must produce: the header once, the page
    /// rows the viewport's content area ever showed, and the footer once at the bottom.
    fn truth(page: &[u8], last: u32) -> Vec<u8> {
        let row = (W * 4) as usize;
        let first = frame_at(page, 0);
        let final_frame = frame_at(page, last);
        let mut out = first[..HEADER as usize * row].to_vec();
        out.extend_from_slice(&page[HEADER as usize * row..(last + H - FOOTER) as usize * row]);
        out.extend_from_slice(&final_frame[(H - FOOTER) as usize * row..]);
        out
    }

    /// Drive estimator + stitcher over a sequence of page offsets, the way the worker does.
    fn run(page: &[u8], offsets: &[u32]) -> Stitcher<super::MemoryCanvas> {
        let first = frame_at(page, offsets[0]);
        let mut stitcher = Stitcher::new(&first, W, H, 32 * H);
        let mut reference = FrameSig::of(&first, W, H);
        for &top in &offsets[1..] {
            let frame = frame_at(page, top);
            let current = FrameSig::of(&frame, W, H);
            match estimate(&reference, &current) {
                Estimate::Shift { dy, .. } => {
                    stitcher.place(&frame, dy);
                    reference = current;
                }
                Estimate::Still { .. } => reference = current,
                Estimate::Unknown { .. } => {}
            }
        }
        stitcher
    }

    /// The three counters must name the site that actually moved bytes, because the plan's
    /// attribution rests on them (docs/35 §7.1, phase 0).
    #[cfg(feature = "test-support")]
    #[test]
    fn the_counters_name_the_site_that_moved_the_canvas() {
        let page = page(W, 4000, 21);

        // A downward run copies bands and may reallocate, but never shifts.
        let mut down = Stitcher::new(&window(&page, W, 0, H), W, H, 32 * H);
        for top in (60..=600).step_by(60) {
            down.place(&window(&page, W, top, H), 60);
        }
        let appended = down.counters();
        assert_eq!(appended.shift_moves, 0, "a downward run never prepends");
        assert_eq!(appended.shift_bytes, 0);
        assert!(appended.band_copies >= 10, "{appended:?}");
        assert!(appended.band_bytes > 0);
        assert!(appended.grow_moves >= 1, "the canvas doubles: {appended:?}");
        assert!(appended.grow_bytes > 0);

        // Preallocating the cap removes the reallocation without changing the result.
        let mut preallocated = Stitcher::new(&window(&page, W, 0, H), W, H, 32 * H);
        preallocated.preallocate(32 * H);
        for top in (60..=600).step_by(60) {
            preallocated.place(&window(&page, W, top, H), 60);
        }
        let reserved = preallocated.counters();
        assert_eq!(reserved.grow_moves, 0, "no reallocation after reserving: {reserved:?}");
        assert_eq!(reserved.shift_moves, 0);
        assert!(preallocated.canvas() == down.canvas(), "the pixels are the same");

        // An upward step past the canvas top is the shift.
        let mut up = Stitcher::new(&window(&page, W, 1000, H), W, H, 32 * H);
        up.place(&window(&page, W, 950, H), -50);
        let prepended = up.counters();
        assert_eq!(prepended.shift_moves, 1, "{prepended:?}");
        assert_eq!(prepended.shift_bytes, (W * H * 4) as u64);
    }

    #[test]
    fn a_scroll_down_reproduces_the_page_with_one_header_and_one_footer() {
        let page = page(W, 4000, 11);
        // Uneven steps, including the small in-between offsets a smooth scroll produces.
        // Every step stays inside the content area's overlap, as consecutive frames do.
        let offsets = [
            0, 3, 40, 41, 97, 150, 151, 210, 283, 340, 400, 470, 471, 530,
        ];
        let stitcher = run(&page, &offsets);
        assert_eq!(stitcher.rows(), 530 + H);
        assert!(
            stitcher.canvas() == truth(&page, 530).as_slice(),
            "the stitched image must equal the page byte for byte"
        );
    }

    #[test]
    fn scrolling_back_up_and_down_again_changes_nothing_already_seen() {
        let page = page(W, 4000, 12);
        let offsets = [0, 60, 120, 70, 20, 90, 150, 210];
        let stitcher = run(&page, &offsets);
        assert_eq!(stitcher.rows(), 210 + H);
        assert_eq!(stitcher.pos(), 210);
        assert!(stitcher.canvas() == truth(&page, 210).as_slice());
    }

    #[test]
    fn a_jump_the_estimator_cannot_measure_is_skipped_and_the_session_recovers() {
        let page = page(W, 4000, 13);
        // 80 -> 280 leaves no overlap, so it is Unknown; the reference stays at 80, and the next
        // frame (160) is again measurable against it.
        let offsets = [0, 80, 280, 160, 230];
        let stitcher = run(&page, &offsets);
        assert_eq!(stitcher.rows(), 230 + H);
        assert!(stitcher.canvas() == truth(&page, 230).as_slice());
    }

    #[test]
    fn scrolling_up_from_the_middle_grows_the_canvas_at_the_top() {
        let page = page(W, 4000, 14);
        let first = window(&page, W, 1000, H);
        let mut stitcher = Stitcher::new(&first, W, H, 32 * H);
        let up = window(&page, W, 950, H);
        assert_eq!(
            stitcher.place(&up, -50),
            Placement::Prepended {
                growth: 50,
                band: 90
            }
        );
        assert_eq!(stitcher.pos(), 0);
        assert_eq!(stitcher.top(), -50);
        let row = (W * 4) as usize;
        assert!(stitcher.canvas() == &page[950 * row..(1000 + H as usize) * row]);
    }

    #[test]
    fn the_canvas_stops_growing_at_its_cap() {
        let page = page(W, 4000, 15);
        let first = window(&page, W, 0, H);
        let mut stitcher = Stitcher::new(&first, W, H, H + 100);
        assert_eq!(
            stitcher.place(&window(&page, W, 80, H), 80),
            Placement::Appended {
                growth: 80,
                band: 120
            }
        );
        assert_eq!(
            stitcher.place(&window(&page, W, 160, H), 80),
            Placement::Full
        );
        assert_eq!(stitcher.rows(), H + 80);
        assert_eq!(stitcher.pos(), 80);
    }

    /// How much the canvas really moves over a field-sized session. The field log's rising append
    /// cost (68 ms at 1.4k rows, 123 ms at 42k) is not reallocation: at 1323x36000 the canvas moves
    /// six times over 390 appends, for 35.7 MB copied in total. So the per-append cost that does rise
    /// with the image is elsewhere — the prepend path shifts the whole canvas down by the growth
    /// band by construction (`copy_within`, stitch.rs:117). Reported, not asserted.
    #[test]
    #[ignore = "timing report: run with --release --ignored --nocapture"]
    fn the_canvas_moves_a_logarithmic_number_of_times_at_the_field_size() {
        const W: u32 = 1323;
        const H: u32 = 900;
        const MAX_ROWS: u32 = 40 * H;
        let page = page(W, MAX_ROWS, 5);
        let mut stitcher = Stitcher::new(&window(&page, W, 0, H), W, H, MAX_ROWS);
        let mut moves = 0u32;
        let mut moved = 0u64;
        let mut capacity = stitcher.canvas_capacity();
        let mut appends = 0u32;
        let mut top = 0;
        while top + 90 + H <= MAX_ROWS {
            top += 90;
            let before = stitcher.canvas_len();
            if !matches!(
                stitcher.place(&window(&page, W, top, H), 90),
                Placement::Appended { .. }
            ) {
                break;
            }
            appends += 1;
            let now = stitcher.canvas_capacity();
            if now != capacity {
                moves += 1;
                moved += before as u64;
                capacity = now;
            }
        }
        println!(
            "canvas {W}x{}: {appends} appends, {moves} moves, {:.1} MB copied, peak {:.1} MB",
            stitcher.rows(),
            moved as f64 / 1e6,
            capacity as f64 / 1e6,
        );
    }

    /// The field log: every frame cost more the longer the image got (68 ms at 1.4k rows, 123 ms
    /// at 42k). `grow` is what keeps that from being an allocation per notch: the canvas moves
    /// O(log n) times, not once per append, and never reserves past the cap. The append cost that
    /// still rises with the image is the prepend shift, not this.
    #[test]
    fn a_long_session_moves_the_canvas_a_logarithmic_number_of_times() {
        let page = page(W, 32 * H + H, 5);
        let mut stitcher = Stitcher::new(&window(&page, W, 0, H), W, H, 32 * H);
        let mut moves = 0;
        let mut capacity = stitcher.canvas_capacity();
        let mut top = 0;
        while top + 9 + H <= 32 * H {
            top += 9;
            assert!(matches!(
                stitcher.place(&window(&page, W, top, H), 9),
                Placement::Appended { .. }
            ));
            if stitcher.canvas_capacity() != capacity {
                moves += 1;
                capacity = stitcher.canvas_capacity();
            }
        }
        assert!(moves <= 8, "{moves} reallocations over {} appends", top / 9);
        assert!(stitcher.canvas_capacity() <= (32 * H * W * 4) as usize);
        assert_eq!(
            stitcher.canvas(),
            &page[..(stitcher.rows() * W * 4) as usize]
        );
    }

    /// Performance evidence at the field size (2148x1344, 90-row notches, up to the 32-screen cap):
    /// the cost of one append, early and late in a session. Reported, not asserted.
    #[test]
    #[ignore = "timing report: run with --release --ignored --nocapture"]
    fn the_cost_of_an_append_at_the_field_size() {
        let (width, view) = (2148u32, 1344u32);
        let frame = vec![0x5au8; (width * view * 4) as usize];
        let mut stitcher = Stitcher::new(&frame, width, view, 32 * view);
        let mut costs = Vec::new();
        while let Placement::Appended { .. } = {
            let started = std::time::Instant::now();
            let placed = stitcher.place(&frame, 90);
            costs.push(started.elapsed().as_micros());
            placed
        } {}
        let mean = |part: &[u128]| part.iter().sum::<u128>() / part.len() as u128;
        println!(
            "appends={} first50_mean_us={} last50_mean_us={} max_us={} rows={}",
            costs.len() - 1,
            mean(&costs[..50]),
            mean(&costs[costs.len() - 51..costs.len() - 1]),
            costs.iter().max().unwrap(),
            stitcher.rows()
        );
    }

    // ── Phase 1: the store seam must not change what the stitcher does ──────────────────────────
    //
    // The oracle below is the placement logic exactly as it stood before the seam. It shares no
    // code with the implementation on purpose: a reference that called into the thing under test
    // would prove nothing (docs/35 phase 1, §8).

    struct Reference {
        canvas: Vec<u8>,
        width: u32,
        view: u32,
        max_rows: u32,
        rows: u32,
        pos: u32,
        top: i64,
    }

    impl Reference {
        fn new(first: &[u8], width: u32, view: u32, max_rows: u32) -> Self {
            Self {
                canvas: first.to_vec(),
                width,
                view,
                max_rows: max_rows.max(view),
                rows: view,
                pos: 0,
                top: 0,
            }
        }

        fn place(&mut self, frame: &[u8], dy: i32) -> Placement {
            let row = (self.width * 4) as usize;
            let view = self.view as i64;
            let cand = i64::from(self.pos) + i64::from(dy);
            if cand + view > i64::from(self.rows) {
                let growth = (cand + view - i64::from(self.rows)) as u32;
                if self.rows + growth > self.max_rows || !self.grow_to(growth, row) {
                    return Placement::Full;
                }
                self.rows += growth;
                let band = self.band(growth) as usize;
                let to = self.rows as usize * row;
                self.canvas[to - band * row..to]
                    .copy_from_slice(&frame[(self.view as usize - band) * row..]);
                self.pos = self.rows - self.view;
                Placement::Appended {
                    growth,
                    band: band as u32,
                }
            } else if cand < 0 {
                let growth = (-cand) as u32;
                let known = self.canvas.len();
                if self.rows + growth > self.max_rows || !self.grow_to(growth, row) {
                    return Placement::Full;
                }
                self.rows += growth;
                self.canvas.copy_within(..known, growth as usize * row);
                let band = self.band(growth) as usize;
                self.canvas[..band * row].copy_from_slice(&frame[..band * row]);
                self.pos = 0;
                self.top -= i64::from(growth);
                Placement::Prepended {
                    growth,
                    band: band as u32,
                }
            } else {
                self.pos = cand as u32;
                Placement::Contained
            }
        }

        fn grow_to(&mut self, growth: u32, row: usize) -> bool {
            reference_grow(
                &mut self.canvas,
                growth as usize * row,
                self.max_rows as usize * row,
            )
        }

        fn band(&self, growth: u32) -> u32 {
            (self.view / 2).max(growth + self.view / 4).min(self.view)
        }
    }

    fn reference_grow(canvas: &mut Vec<u8>, bytes: usize, limit: usize) -> bool {
        let needed = canvas.len().saturating_add(bytes);
        let wanted = needed.max(canvas.capacity().saturating_mul(2).min(limit));
        if canvas.try_reserve_exact(wanted - canvas.len()).is_err() {
            return false;
        }
        canvas.resize(canvas.len() + bytes, 0);
        true
    }

    /// Which store the subject runs on: the phase 1 buffer or the phase 2 backed one. The oracle
    /// is the same frozen reference either way, so an equal result means the backing store changed
    /// nothing but where the rows live (docs/35 §11.2, §7.2).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Store {
        Memory,
        Backed,
    }

    /// Drive one shift sequence through the store-based stitcher and through the oracle, and
    /// require the same placement, position, content origin and pixels after every single frame.
    /// Returns `(frames, final rows)`.
    fn equivalence_with(store: Store, start: u32, shifts: &[i32], max_rows: u32) -> (usize, u32) {
        let page = page(W, 6000, 21);
        let first = frame_at(&page, start);
        let inner: Box<dyn CanvasStore> = match store {
            Store::Memory => Box::new(MemoryCanvas::new(&first, W, H, max_rows)),
            Store::Backed => Box::new(super::super::backed::BackedCanvas::new(&first, W, H, max_rows)),
        };
        let mut subject = Stitcher::with_store(inner, W, H);
        let mut oracle = Reference::new(&first, W, H, max_rows);
        let mut top = i64::from(start);
        let row = (W * 4) as usize;
        for (index, &dy) in shifts.iter().enumerate() {
            let next = (top + i64::from(dy)).clamp(0, 5600);
            top = next;
            let frame = frame_at(&page, next as u32);
            let placed = subject.place(&frame, dy);
            let expected = oracle.place(&frame, dy);
            assert_eq!(placed, expected, "{store:?}: placement differs at shift {index} ({dy})");
            assert_eq!(subject.pos(), oracle.pos, "{store:?}: viewport row at shift {index}");
            assert_eq!(subject.top(), oracle.top, "{store:?}: content origin at shift {index}");
            assert_eq!(subject.rows(), oracle.rows, "{store:?}: image height at shift {index}");
            let rows = subject.rows();
            let mut pixels = vec![0u8; rows as usize * row];
            subject.store().read_rows(0, rows, &mut pixels);
            assert!(
                pixels == oracle.canvas,
                "{store:?}: pixels differ at shift {index} ({dy})"
            );
        }
        (shifts.len() + 1, subject.rows())
    }

    fn equivalence(start: u32, shifts: &[i32], max_rows: u32) -> (usize, u32) {
        equivalence_with(Store::Memory, start, shifts, max_rows)
    }

    #[test]
    fn a_pure_downward_sequence_matches_the_oracle() {
        let shifts = [60; 12];
        let (frames, rows) = equivalence(0, &shifts, 32 * H);
        assert_eq!((frames, rows), (13, 12 * 60 + H));
    }

    #[test]
    fn a_pure_upward_sequence_matches_the_oracle() {
        let shifts = [-50; 8];
        let (frames, rows) = equivalence(1000, &shifts, 32 * H);
        assert_eq!((frames, rows), (9, 8 * 50 + H));
    }

    #[test]
    fn several_direction_reversals_match_the_oracle() {
        // Every step stays inside the page, and the run starts away from the top so the upward
        // moves reach the prepend path rather than being contained (docs/35 phase 0).
        let shifts = [60, 60, -70, -50, 80, -90, 40, -30, -30, 50, -110, 70];
        let (frames, rows) = equivalence(1000, &shifts, 64 * H);
        assert!(frames == 13 && rows >= H, "{frames} frames, {rows} rows");
    }

    #[test]
    fn contained_moves_match_the_oracle_and_write_nothing() {
        let shifts = [10, -10, 0, 5, -5, 3, -3];
        let (_, _) = equivalence(500, &shifts, 32 * H);
    }

    #[test]
    fn a_sequence_that_hits_the_row_cap_matches_the_oracle() {
        let shifts = [80, 80, 80, 80];
        let (_, rows) = equivalence(0, &shifts, H + 160);
        assert_eq!(rows, H + 160, "growth stops at the cap, and both agree on where");
    }

    /// The same six sequences over the backed store: same placements, same pixels, so the tile
    /// file and the two rings are invisible to the image (docs/35 §7.2, stage 2's exit condition).
    #[test]
    fn the_backed_store_matches_the_oracle_on_every_sequence() {
        let cases: [(u32, Vec<i32>, u32); 5] = [
            (0, vec![60; 12], 32 * H),
            (1000, vec![-50; 8], 32 * H),
            (1000, vec![60, 60, -70, -50, 80, -90, 40, -30, -30, 50, -110, 70], 64 * H),
            (500, vec![10, -10, 0, 5, -5, 3, -3], 32 * H),
            (0, vec![80, 80, 80, 80], H + 160),
        ];
        for (start, shifts, max_rows) in cases {
            let memory = equivalence_with(Store::Memory, start, &shifts, max_rows);
            let backed = equivalence_with(Store::Backed, start, &shifts, max_rows);
            assert_eq!(memory, backed, "the two stores disagree for {shifts:?}");
        }
    }

    #[test]
    fn a_move_inside_known_rows_writes_nothing() {
        let page = page(W, 4000, 16);
        let first = window(&page, W, 0, H);
        let mut stitcher = Stitcher::new(&first, W, H, 32 * H);
        stitcher.place(&window(&page, W, 100, H), 100);
        let before = stitcher.canvas().to_vec();
        // A frame that differs from the page would show up if it were written.
        let garbage = vec![9u8; (W * H * 4) as usize];
        assert_eq!(stitcher.place(&garbage, -40), Placement::Contained);
        assert_eq!(stitcher.pos(), 60);
        assert!(stitcher.canvas() == before.as_slice());
    }
}
