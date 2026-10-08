//! The canvas is a logical interval, a coverage invariant and a band store — **not a bitmap**
//! (`docs/30` §17.1, §17.2, §20.3; task `P1.17`).
//!
//! `docs/19` §8.1's one genuinely correct constraint survives: for every content pixel there is at
//! least one `Confirmed` observation that wrote it. V2 keeps that constraint and makes it
//! **assertable** — [`RecoveredImage::assert_invariants`] runs after every step (§20.3) — because V1
//! maintained the same idea through a single `next_pos` variable, where any partial commit broke it
//! silently. A rule that is only in the document is a liability; `CaptureState::Adjusting` is the
//! precedent (§5).
//!
//! Three things in here are deliberately **not** what §17.1's sketch says, and each one exists
//! because the sketch cannot make an invariant assertable (`docs/31` §0.6 `DEV-26`):
//!
//! * `CoverageMap` stores **one bit per row**, not just the interval's two ends. Invariant 3 ("the
//!   interval reaches the end") and invariant 4 ("there is no hole inside it") are different facts,
//!   and an interval cannot tell them apart: a write that covers rows 0..500 and 501..1000 leaves
//!   both ends looking right. The ends are therefore *derived* from the set — one source of truth,
//!   because two of them is exactly how a hole becomes invisible.
//! * The invariants are real `assert!`s, not `debug_assert!`s. §17.1 says `debug_assert`; a canvas
//!   with a hole is the one thing this design must never export (§20.3 lists the action for
//!   invariant 4 as "InternalError, and **no export**"), and the checks are O(1) plus one
//!   `O(n log n)` walk over the bands, once per step. Shipping a corrupted canvas because the build
//!   was a release build is the failure mode §20.3 was written to remove.
//! * `RecoveredImage` remembers the thread that created it, which is invariant 8 in its
//!   canvas-level form. `P0.02` measured that the production hand-off really does touch the
//!   immediate `ID3D11DeviceContext` from more than one thread (`docs/30` §21.3); the canvas is the
//!   same kind of object — the scroll driver owns it, the preview gets copies of rows, nobody else
//!   may call into it.
//!
//! Not here yet: the LRU and the disk spill (`P1.20`), the prepend/`Contained` cases (`P1.19`) and
//! the export sink (`P4.01`). Until then the module is exercised by its own tests and the crate's
//! warning budget stays at zero (`docs/31` §4.1).
#![allow(dead_code)]

use crate::scroll::observation::{Axis, Observation};

/// Bytes per pixel in a band: BGRA, the format every capture path produces (`docs/30` §17.5).
pub(crate) const BYTES_PER_PIXEL: u64 = 4;

/// How much of the viewport a step matches and writes: §17.2's `max(extent/2, extent/4 + shift)`.
///
/// The first term is the verifiability constraint — the new frame covers at most half the viewport,
/// so the overlap keeps at least half of the *wanted* band. The second term stops the overlap from
/// thinning when the step is large: without it a step of 0.4·extent would leave a 0.1·extent band
/// to measure `gain` and `coverage` on. `max` and not a sum, because only one of the two
/// constraints dominates at a time.
///
/// This is the **wanted** height. The rows a step can actually match are capped by the geometry —
/// `P1.05`'s `match_band` takes `min(wanted, overlap)` — and at `|shift| == extent` there is no
/// overlap at all: that is the `Lost` case (§16.5), not a band-height problem. The band is
/// therefore deliberately **not** clamped to `extent`: clamping it would break the very property
/// the second term exists to provide.
pub(crate) fn band_height(extent: u32, shift: i32) -> u32 {
    let half = extent / 2;
    let scaled = extent as i64 / 4 + shift as i64;
    if scaled <= half as i64 { half } else { scaled as u32 }
}

/// What a step did to the canvas (§17.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepWrite {
    /// Nothing: a duplicate frame (§16.3's `Skip` path). The canvas is byte-identical.
    Skipped,
    /// Nothing: the viewport moved, but it moved **inside** the rows the canvas already has
    /// (§17.4's `Contained`). Not the same as [`Self::Skipped`] — the content moved and the viewport
    /// follows it; there is simply nothing new to write. Treating it as an append would write the
    /// same content a second time, which is the one thing this branch exists to prevent.
    Contained,
    /// `rows` new rows were appended at `first_row` (content coordinates).
    Appended { first_row: u64, rows: u64 },
    /// `rows` new rows were prepended at row 0; every existing row moved down by `rows`.
    Prepended { rows: u64 },
}

/// One whole-width row band (§17.5). `rows` is BGRA in canvas order, `first_row` is its position on
/// the primary axis, and `fnv` is the checksum that `P1.20` fills in and every load verifies.
#[derive(Debug, Clone)]
pub(crate) struct Band {
    pub(crate) first_row: u64,
    pub(crate) rows: Vec<u8>,
    pub(crate) fnv: u64,
}

impl Band {
    /// How many primary-axis rows this band covers, from its byte length and the canvas width.
    pub(crate) fn row_count(&self, cross_len: u64) -> u64 {
        let row_bytes = cross_len * BYTES_PER_PIXEL;
        if row_bytes == 0 {
            return 0;
        }
        self.rows.len() as u64 / row_bytes
    }

    /// The first row *past* this band.
    pub(crate) fn end_row(&self, cross_len: u64) -> u64 {
        self.first_row + self.row_count(cross_len)
    }
}

/// The bands of one canvas: whole-width rows, plus the resident-byte budget they must fit in.
///
/// `P1.20` adds the LRU and the spill file. Until then every band stays resident, and invariant 7 is
/// what keeps "resident bytes" honest: `insert` does not evict, so an over-budget store is a state
/// that only [`RecoveredImage::assert_invariants`] refuses.
pub(crate) struct BandStore {
    pub(crate) bands: Vec<Band>,
    pub(crate) budget: u64,
    cross_len: u64,
}

impl BandStore {
    pub(crate) fn new(cross_len: u64, budget: u64) -> Self {
        Self {
            bands: Vec::new(),
            budget,
            cross_len,
        }
    }

    /// Adds a band. The band has to be a whole number of rows for this canvas and must not overlap
    /// what is already there — writing an overlapping band means the caller believes it knows where
    /// the content is and the canvas does not (invariant 6).
    pub(crate) fn insert(&mut self, band: Band) {
        let row_bytes = self.cross_len * BYTES_PER_PIXEL;
        assert!(
            row_bytes > 0 && band.rows.len() as u64 % row_bytes == 0,
            "a band must be a whole number of rows: {} bytes for a canvas {} px wide",
            band.rows.len(),
            self.cross_len
        );
        assert!(
            self.is_disjoint_from(&band),
            "band {}..{} overlaps a band that is already in the store",
            band.first_row,
            band.end_row(self.cross_len)
        );
        self.bands.push(band);
    }

    fn is_disjoint_from(&self, candidate: &Band) -> bool {
        self.bands.iter().all(|band| {
            candidate.first_row >= band.end_row(self.cross_len)
                || band.first_row >= candidate.end_row(self.cross_len)
        })
    }

    /// Invariant 6: no two bands share a row.
    pub(crate) fn is_disjoint(&self) -> bool {
        let mut ranges: Vec<(u64, u64)> = self
            .bands
            .iter()
            .map(|band| (band.first_row, band.end_row(self.cross_len)))
            .collect();
        ranges.sort_unstable();
        ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0)
    }

    /// Moves every band down by `rows`. Used by a prepend (`P1.19`): the content did not move, the
    /// canvas grew **above** it, so the coordinates of everything already stored shift by the number
    /// of rows that were inserted in front of them.
    pub(crate) fn shift_rows(&mut self, rows: u64) {
        if rows == 0 {
            return;
        }
        for band in &mut self.bands {
            band.first_row += rows;
        }
    }

    /// Invariant 7's left-hand side.
    pub(crate) fn resident_bytes(&self) -> u64 {
        self.bands.iter().map(|band| band.rows.len() as u64).sum()
    }

    pub(crate) fn bands(&self) -> &[Band] {
        &self.bands
    }
}

/// Which rows of the canvas have been written at least once (§17.1's coverage invariant).
///
/// One bit per row: a 100,000-row canvas is 12.5 KiB here, and **no pixels live in this type** — the
/// "canvas bitmap" of V1 does not exist. See the module doc for why the interval ends are derived.
pub(crate) struct CoverageMap {
    covered: Vec<u64>,
    first: Option<u64>,
    last_exclusive: u64,
    rows_covered: u64,
    pub(crate) stale_steps: u32,
}

impl CoverageMap {
    pub(crate) fn new(primary_len: u64) -> Self {
        Self {
            covered: vec![0; words_for(primary_len)],
            first: None,
            last_exclusive: 0,
            rows_covered: 0,
            stale_steps: 0,
        }
    }

    /// Marks `[start, end)` as written and returns how many rows were newly covered. Growing the
    /// bitmap is this method's business, so a caller cannot forget to size it.
    pub(crate) fn mark_range(&mut self, start: u64, end: u64) -> u64 {
        if start >= end {
            return 0;
        }
        self.ensure_rows(end);

        let mut added = 0;
        let mut row = start;
        while row < end {
            let word = (row / 64) as usize;
            let bit = row % 64;
            if bit == 0 && row + 64 <= end {
                added += 64 - self.covered[word].count_ones() as u64;
                self.covered[word] = u64::MAX;
                row += 64;
            } else {
                let mask = 1u64 << bit;
                if self.covered[word] & mask == 0 {
                    self.covered[word] |= mask;
                    added += 1;
                }
                row += 1;
            }
        }

        if added > 0 {
            self.first = Some(self.first.map_or(start, |first| first.min(start)));
            self.last_exclusive = self.last_exclusive.max(end);
            self.rows_covered += added;
        }
        added
    }

    pub(crate) fn is_covered(&self, row: u64) -> bool {
        let word = (row / 64) as usize;
        word < self.covered.len() && (self.covered[word] >> (row % 64)) & 1 == 1
    }

    /// Invariant 2's left-hand side: the lowest covered row, or 0 when nothing is covered.
    pub(crate) fn span_start(&self) -> u64 {
        self.first.unwrap_or(0)
    }

    /// Invariant 3's left-hand side: one past the highest covered row.
    pub(crate) fn span_end(&self) -> u64 {
        self.last_exclusive
    }

    pub(crate) fn rows_covered(&self) -> u64 {
        self.rows_covered
    }

    /// Invariant 4: the number of covered rows equals the width of the covered interval, so there is
    /// no gap inside it. O(1) — the count is maintained by [`Self::mark_range`].
    pub(crate) fn has_hole(&self) -> bool {
        self.rows_covered != self.span_end() - self.span_start()
    }

    fn ensure_rows(&mut self, rows: u64) {
        let words = words_for(rows);
        if self.covered.len() < words {
            self.covered.resize(words, 0);
        }
    }

    /// Makes room for `rows` rows **in front** of the ones already marked, shifting every existing
    /// bit up by `rows` and marking the new front as covered. A prepend (`P1.19`) is the only caller.
    ///
    /// The shift is a real bit shift over the whole map rather than a stored offset, because the
    /// coverage interval is anchored at 0 (invariant 2) and a bias field would put the anchor in two
    /// places at once. `last_exclusive` moves by `rows` and then [`Self::mark_range`] fills the front:
    /// the count of covered rows has to keep answering invariant 4 in O(1), so the shifted bits are
    /// never recounted — only the newly written front rows are.
    pub(crate) fn insert_rows_at_front(&mut self, rows: u64) {
        if rows == 0 {
            return;
        }
        let shift_words = (rows / 64) as usize;
        let shift_bits = (rows % 64) as u32;
        let old_words = self.covered.len();
        let new_words = old_words + words_for(rows);
        self.covered.resize(new_words, 0);
        // From the top down, so every source word is read before it is overwritten.
        for index in (0..new_words).rev() {
            let high = if index >= shift_words {
                self.covered[index - shift_words]
            } else {
                0
            };
            let low = if index > shift_words {
                self.covered[index - shift_words - 1]
            } else {
                0
            };
            self.covered[index] = if shift_bits == 0 {
                high
            } else {
                (high << shift_bits) | (low >> (64 - shift_bits))
            };
        }
        self.last_exclusive += rows;
        self.mark_range(0, rows);
    }
}

fn words_for(rows: u64) -> usize {
    rows.div_ceil(64) as usize
}

/// What one step did with the frame it was given. §20.3 invariant 5 is the statement that these
/// three numbers add up, and it is a *type* rather than three loose arguments so that a caller
/// cannot pass two of them by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StepTally {
    pub(crate) step: u64,
    pub(crate) committed: u64,
    pub(crate) discarded: u64,
}

/// The recovered image: a logical interval on the primary axis, the coverage of it, and the bands.
///
/// There is no `Vec<u8>` of the whole image anywhere in this type, which is the point (§17.1): a
/// 100,000-row canvas at 1500 px is 600 MB, and `docs/30` §17.5 requires memory to be independent of
/// the image's length.
pub(crate) struct RecoveredImage {
    pub(crate) axis: Axis,
    pub(crate) primary_len: u64,
    pub(crate) cross_len: u64,
    pub(crate) coverage: CoverageMap,
    pub(crate) bands: BandStore,
    owner: std::thread::ThreadId,
}

impl RecoveredImage {
    pub(crate) fn new(axis: Axis, cross_len: u64, budget: u64) -> Self {
        Self {
            axis,
            primary_len: 0,
            cross_len,
            coverage: CoverageMap::new(0),
            bands: BandStore::new(cross_len, budget),
            owner: std::thread::current().id(),
        }
    }

    pub(crate) fn axis(&self) -> Axis {
        self.axis
    }

    pub(crate) fn primary_len(&self) -> u64 {
        self.primary_len
    }

    pub(crate) fn cross_len(&self) -> u64 {
        self.cross_len
    }

    pub(crate) fn coverage(&self) -> &CoverageMap {
        &self.coverage
    }

    pub(crate) fn bands(&self) -> &BandStore {
        &self.bands
    }

    /// The first frame establishes the canvas: every row it shows is new content.
    ///
    /// This is the only call that writes a whole viewport at once, and §17.3's rule is vacuous here
    /// because there is nothing to overwrite yet.
    pub(crate) fn start(&mut self, frame: &Observation) {
        assert_eq!(
            self.primary_len, 0,
            "the canvas already has content: only the first frame creates it"
        );
        assert_eq!(
            frame.width() as u64,
            self.cross_len,
            "the frame's cross axis is not the canvas's — the canvas width is fixed for the session (invariant 1)"
        );
        let rows = frame.height() as u64;
        let row_bytes = (self.cross_len * BYTES_PER_PIXEL) as usize;
        assert_eq!(
            frame.pixels().len(),
            row_bytes * rows as usize,
            "the frame is not strictly packed: the canvas stores whole rows"
        );
        self.bands.insert(Band {
            first_row: 0,
            rows: frame.pixels().to_vec(),
            fnv: 0,
        });
        self.primary_len = rows;
        self.coverage = CoverageMap::new(rows);
        self.coverage.mark_range(0, rows);
    }

    /// Writes the rows one confirmed step adds below the canvas (§17.3): `[old end, old end + rows)`,
    /// taken from the **bottom** of the frame — the frame's viewport ends at the new content end, so
    /// the new content is the frame's last `rows` rows.
    ///
    /// The overlap is never rewritten. Three reasons, and none of them is "it is safer" (§17.3):
    /// the overlap is not the same thing in the two frames (a `fixed` header or an animation would
    /// be stamped into the canvas on every step, which is the one place where the region model and
    /// the write rule would disagree), a rewrite is how drift gets *into* the canvas (the residual
    /// gate is a frame-pair judgement, so nothing at the canvas level would stop "every step right,
    /// whole canvas wrong"), and first-writer-wins makes every row answer "which step produced me"
    /// — which is what `Review` and the undo of §19.6 are built on.
    ///
    /// The cost is written down too: a ±1 px error that passes all four gates is *frozen* into the
    /// seam instead of being repainted by a later frame. That trade is deliberate — rare and local
    /// (four gates plus the prior) against common and cumulative (§17.4's reference design is what
    /// keeps it from propagating).
    ///
    /// `rows` is how many rows are genuinely new. In the plain append it is the step's displacement
    /// `d`; a viewport that jumped further than the canvas' end adds only what fits, and
    /// [`ViewportState::apply`] is what computes that number. The direction is not this method's
    /// business: a prepend is [`Self::prepend_confirmed`], and the caller decides which one a step
    /// is (`ViewportState`).
    pub(crate) fn append_confirmed(&mut self, frame: &Observation, rows: u64) -> StepWrite {
        if rows == 0 {
            return StepWrite::Skipped;
        }
        let extent = frame.height() as u64;
        assert_eq!(
            frame.width() as u64,
            self.cross_len,
            "the frame's cross axis is not the canvas's (invariant 1)"
        );
        assert!(
            rows <= extent,
            "a step that adds {rows} rows cannot come out of a viewport of {extent} rows"
        );
        assert!(
            rows <= band_height(extent as u32, rows as i32) as u64,
            "the band ({}) does not contain the {rows} rows this step adds: §17.2's first term is what makes the write possible at all",
            band_height(extent as u32, rows as i32)
        );
        let first_row = self.primary_len;
        let row_bytes = (self.cross_len * BYTES_PER_PIXEL) as usize;
        let first_frame_row = (extent - rows) as usize * row_bytes;
        let new_rows = &frame.pixels()[first_frame_row..];
        assert_eq!(
            new_rows.len(),
            row_bytes * rows as usize,
            "the frame is not strictly packed: the canvas stores whole rows"
        );
        self.bands.insert(Band {
            first_row,
            rows: new_rows.to_vec(),
            fnv: 0,
        });
        self.primary_len += rows;
        self.coverage.mark_range(first_row, self.primary_len);
        StepWrite::Appended { first_row, rows }
    }

    /// Writes the rows a confirmed step adds **above** the canvas (§17.1's "both directions"): the
    /// frame's first `rows` rows, at content row 0, with every existing row moved down by `rows`.
    ///
    /// This is what makes §17.4's "the reference is always the canvas" affordable in both directions
    /// and what `F-02` is about: `next_pos = current_pos + signed_delta` — a negative displacement is
    /// a fact about the page, not an error. `Contained` is the other side of the same coin and is
    /// decided before this is called ([`ViewportState::apply`]).
    ///
    /// The half-viewport bound is §17.2's first term read in this direction: a prepend of more than
    /// half the viewport would leave less than half the frame overlapping the canvas, which is the
    /// geometry the design refuses on the append side too. It is an assertion rather than a silent
    /// trim because a caller that hands us one has skipped the gate that says `|d| <= extent`.
    pub(crate) fn prepend_confirmed(&mut self, frame: &Observation, rows: u64) -> StepWrite {
        if rows == 0 {
            return StepWrite::Contained;
        }
        let extent = frame.height() as u64;
        assert_eq!(
            frame.width() as u64,
            self.cross_len,
            "the frame's cross axis is not the canvas's (invariant 1)"
        );
        assert!(
            rows <= extent / 2,
            "a prepend of {rows} rows leaves less than half of the {extent}-row viewport overlapping the canvas, which is the geometry §17.2's first term rules out"
        );
        let row_bytes = (self.cross_len * BYTES_PER_PIXEL) as usize;
        let new_rows = &frame.pixels()[..rows as usize * row_bytes];
        assert_eq!(
            new_rows.len(),
            row_bytes * rows as usize,
            "the frame is not strictly packed: the canvas stores whole rows"
        );
        self.bands.shift_rows(rows);
        self.bands.insert(Band {
            first_row: 0,
            rows: new_rows.to_vec(),
            fnv: 0,
        });
        self.primary_len += rows;
        self.coverage.insert_rows_at_front(rows);
        StepWrite::Prepended { rows }
    }

    /// The largest position the viewport's top edge can take without leaving the canvas.
    pub(crate) fn max_position(&self, extent: u64) -> i64 {
        self.primary_len as i64 - extent as i64
    }

    /// A copy of `rows` primary-axis rows starting at `first_row`, reassembled across the bands.
    ///
    /// There is no single buffer to hand out (§17.1), so a reader asks for the range it needs: the
    /// export sink (`P4.01`), the preview and the tests all read through here. Rows the canvas does
    /// not have are a caller error, not a zero-filled range.
    pub(crate) fn rows(&self, first_row: u64, rows: u64) -> Vec<u8> {
        assert!(
            first_row + rows <= self.primary_len,
            "asked for rows {first_row}..{} but the canvas ends at {}",
            first_row + rows,
            self.primary_len
        );
        let row_bytes = (self.cross_len * BYTES_PER_PIXEL) as usize;
        let mut out = vec![0u8; row_bytes * rows as usize];
        let end = first_row + rows;
        for band in self.bands.bands() {
            let start = band.first_row.max(first_row);
            let stop = band.end_row(self.cross_len).min(end);
            if start >= stop {
                continue;
            }
            let src = (start - band.first_row) as usize * row_bytes;
            let dst = (start - first_row) as usize * row_bytes;
            let len = (stop - start) as usize * row_bytes;
            out[dst..dst + len].copy_from_slice(&band.rows[src..src + len]);
        }
        out
    }

    /// §20.3's eight invariants, checked after every step. The messages name the invariant by number
    /// so that a failure says *which* promise broke, and the test that constructs each violation
    /// checks exactly that.
    pub(crate) fn assert_invariants(&self, viewport_cross: u64, tally: StepTally) {
        assert_eq!(
            self.cross_len, viewport_cross,
            "invariant 1 (docs/30 §20.3): the cross axis is constant for the whole session — canvas {} px, viewport {} px",
            self.cross_len, viewport_cross
        );
        assert_eq!(
            self.coverage.span_start(),
            0,
            "invariant 2 (docs/30 §20.3): coverage must start at the top of the content, not at row {}",
            self.coverage.span_start()
        );
        assert_eq!(
            self.coverage.span_end(),
            self.primary_len,
            "invariant 3 (docs/30 §20.3): coverage must reach the end of the content — {} rows covered, content is {} rows",
            self.coverage.span_end(),
            self.primary_len
        );
        assert_eq!(
            self.coverage.rows_covered(),
            self.coverage.span_end() - self.coverage.span_start(),
            "invariant 4 (docs/30 §20.3): the coverage has a hole — {} of the {} rows inside {}..{} were written",
            self.coverage.rows_covered(),
            self.coverage.span_end() - self.coverage.span_start(),
            self.coverage.span_start(),
            self.coverage.span_end()
        );
        assert_eq!(
            tally.committed + tally.discarded,
            tally.step,
            "invariant 5 (docs/30 §20.3): every frame is committed or discarded — {} committed + {} discarded != {} frames",
            tally.committed,
            tally.discarded,
            tally.step
        );
        assert!(
            self.bands.is_disjoint(),
            "invariant 6 (docs/30 §20.3): two bands share a row: {:?}",
            self.bands
                .bands()
                .iter()
                .map(|band| (band.first_row, band.end_row(self.cross_len)))
                .collect::<Vec<_>>()
        );
        assert!(
            self.bands.resident_bytes() <= self.bands.budget,
            "invariant 7 (docs/30 §20.3): {} resident bytes over a budget of {}",
            self.bands.resident_bytes(),
            self.bands.budget
        );
        assert_eq!(
            std::thread::current().id(),
            self.owner,
            "invariant 8 (docs/30 §20.3, §21.3): the canvas has exactly one user thread — it was created on {:?}",
            self.owner
        );
    }
}

/// Where the viewport's top edge sits in the canvas' content coordinates, and the three things a
/// confirmed step can therefore be (§17.1, §17.4; `P1.19`).
///
/// This is the whole of `F-02`'s `next_pos = current_pos + signed_delta` made executable: a signed
/// displacement moves the viewport, and where it lands decides which of the three writes a step is.
/// The reference implementation's `offset` is the *viewport's* displacement, so its formula reads
/// `position − offset`; this module keeps the estimator's convention throughout (`d > 0` means the
/// content moved up, §15.1), which makes it `position + d` — one sign convention, one place to get
/// it wrong instead of two.
///
/// Nothing here decides *whether* a step is confirmed — that is the four gates' business (§16.1).
/// This type decides what a confirmed step **does**, which is why `Contained` is a first-class
/// answer rather than an append that happens to write nothing.
pub(crate) struct ViewportState {
    position: i64,
    extent: u32,
}

impl ViewportState {
    /// The viewport starts at the canvas' origin: the first frame **is** the canvas (§17.1).
    pub(crate) fn new(extent: u32) -> Self {
        Self {
            position: 0,
            extent,
        }
    }

    pub(crate) fn position(&self) -> i64 {
        self.position
    }

    pub(crate) fn extent(&self) -> u32 {
        self.extent
    }

    /// Applies one confirmed step: decides prepend / append / contained, writes through the canvas
    /// and advances the viewport. Returns what the step did.
    ///
    /// A step larger than the viewport is refused rather than trimmed: `|d| <= extent` is gate one's
    /// correctness constraint (§16.2), and a step that does not overlap at all has no evidence to
    /// confirm it in the first place (§16.5's `Lost`).
    pub(crate) fn apply(
        &mut self,
        canvas: &mut RecoveredImage,
        frame: &Observation,
        step: i32,
    ) -> StepWrite {
        assert_eq!(
            frame.height(),
            self.extent,
            "the viewport's extent is fixed for the session: a resized frame is a new session, not a step"
        );
        if step == 0 {
            return StepWrite::Skipped;
        }
        assert!(
            step.unsigned_abs() <= self.extent,
            "a step of {step} px does not overlap the {}-row viewport at all: §16.5's Lost case is not a write",
            self.extent
        );

        let candidate = self.position + step as i64;
        let max_position = canvas.max_position(self.extent as u64);
        if candidate < 0 {
            let rows = (-candidate) as u64;
            let write = canvas.prepend_confirmed(frame, rows);
            self.position = 0;
            write
        } else if candidate > max_position {
            let rows = (candidate - max_position) as u64;
            let write = canvas.append_confirmed(frame, rows);
            self.position = candidate;
            write
        } else {
            let bottom = candidate + self.extent as i64;
            assert!(
                bottom <= canvas.primary_len() as i64,
                "the viewport at {candidate} ends at {bottom}, past the canvas' {} rows, so this is an append and not a contained step",
                canvas.primary_len()
            );
            self.position = candidate;
            StepWrite::Contained
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Band, CoverageMap, RecoveredImage, StepTally, StepWrite, ViewportState, band_height,
    };
    use crate::geometry::Rect;
    use crate::scroll::displacement::{
        Scratch, Status, candidates_1d, refine_winner, score_candidates_2d, zero_shift_status,
    };
    use crate::scroll::observation::{Axis, Observation};
    use crate::scroll::testkit::{ScrollScript, StepSpec, Structure, TestImage};
    use std::any::Any;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    const CROSS: u64 = 200;
    const ROWS: u64 = 1000;
    const BAND_ROWS: u64 = 10;
    const BUDGET: u64 = 100_000;

    fn band(first_row: u64, rows: u64) -> Band {
        Band {
            first_row,
            rows: vec![0u8; (CROSS * 4 * rows) as usize],
            fnv: 0,
        }
    }

    struct State {
        image: RecoveredImage,
        viewport_cross: u64,
        tally: StepTally,
    }

    /// A state that satisfies all eight invariants, so that every case below has exactly one thing
    /// wrong with it.
    fn valid() -> State {
        let mut image = RecoveredImage::new(Axis::Vertical, CROSS, BUDGET);
        image.primary_len = ROWS;
        image.coverage.mark_range(0, ROWS);
        image.bands.bands = vec![band(0, BAND_ROWS)];
        State {
            image,
            viewport_cross: CROSS,
            tally: StepTally {
                step: 3,
                committed: 2,
                discarded: 1,
            },
        }
    }

    fn message(payload: Box<dyn Any + Send>) -> String {
        if let Some(text) = payload.downcast_ref::<String>() {
            text.clone()
        } else if let Some(text) = payload.downcast_ref::<&str>() {
            (*text).to_string()
        } else {
            String::from("<the panic payload was not text>")
        }
    }

    fn check(state: &State) -> String {
        let payload = catch_unwind(AssertUnwindSafe(|| {
            state.image.assert_invariants(state.viewport_cross, state.tally)
        }))
        .expect_err("the violation did not panic: the invariant is not asserted");
        message(payload)
    }

    #[test]
    fn assert_invariants_fires_on_each_of_the_eight_violations() {
        // The baseline has to be clean, otherwise "the violation fired" proves nothing about the
        // check that fired.
        let baseline = valid();
        baseline
            .image
            .assert_invariants(baseline.viewport_cross, baseline.tally);

        let mut cases: Vec<(usize, String)> = Vec::new();

        // 1 — the cross axis is constant for the whole session
        let mut state = valid();
        state.image.cross_len = CROSS + 1;
        cases.push((1, check(&state)));

        // 2 — the covered interval starts at the top of the content
        let mut state = valid();
        state.image.coverage = CoverageMap::new(ROWS);
        state.image.coverage.mark_range(5, ROWS);
        cases.push((2, check(&state)));

        // 3 — and it reaches the end
        let mut state = valid();
        state.image.coverage = CoverageMap::new(ROWS);
        state.image.coverage.mark_range(0, ROWS - 1);
        cases.push((3, check(&state)));

        // 4 — two-dimensional completeness: 0..500 plus 501.. is a hole at row 500. The interval
        // ends are both fine; only the *set* can see this, which is why the map stores one.
        let mut state = valid();
        state.image.coverage = CoverageMap::new(ROWS);
        state.image.coverage.mark_range(0, 500);
        state.image.coverage.mark_range(501, ROWS);
        cases.push((4, check(&state)));

        // 5 — every frame is either committed or discarded
        let mut state = valid();
        state.tally.discarded = 2;
        cases.push((5, check(&state)));

        // 6 — bands do not overlap
        let mut state = valid();
        state.image.bands.bands = vec![band(0, BAND_ROWS), band(5, BAND_ROWS)];
        cases.push((6, check(&state)));

        // 7 — resident bytes stay inside the budget
        let mut state = valid();
        state.image.bands.budget = (CROSS * 4 * BAND_ROWS) - 1;
        cases.push((7, check(&state)));

        // 8 — the canvas has exactly one user thread (§21.3, §22.5). This is the canvas-level form
        // of the rule `P0.02` measured: the check has to fire on the thread that does not own it.
        let state = valid();
        let payload = std::thread::spawn(move || {
            catch_unwind(AssertUnwindSafe(|| {
                state.image.assert_invariants(state.viewport_cross, state.tally)
            }))
            .expect_err("another thread used the canvas and nothing complained")
        })
        .join()
        .expect("the thread that does not own the canvas must panic, not abort");
        cases.push((8, message(payload)));

        assert_eq!(cases.len(), 8);
        for (index, text) in &cases {
            assert!(
                text.contains(&format!("invariant {index}")),
                "case {index} reported a different invariant: {text}"
            );
        }

        // Eight distinct messages, or the eight cases are really fewer cases.
        let mut texts: Vec<&String> = cases.iter().map(|(_, text)| text).collect();
        texts.sort();
        texts.dedup();
        assert_eq!(
            texts.len(),
            8,
            "two violations produced the same message: {cases:#?}"
        );
    }

    // --- the write path (§17.2, §17.3; task P1.18) ---

    const VIEWPORT: u32 = 900;
    const CROSS_PX: u32 = 320;
    const STEP: i32 = 120;
    const DOC_ROWS: u32 = 1900;
    /// §17.5's budget: eight viewports' worth of pixels.
    const VIEWPORT_BUDGET: u64 = 8 * CROSS_PX as u64 * VIEWPORT as u64 * 4;

    fn mixed() -> [Structure; 5] {
        [
            Structure::Checker { cell: 12 },
            Structure::TextRows { line: 19 },
            Structure::NoiseBlocks { cell: 8 },
            Structure::Gradient,
            Structure::HorizontalBars { period: 19 },
        ]
    }

    /// Two frames of one downward step. The second frame carries a moving region over its **top
    /// 60%** — inside the overlap — so that "kept the first frame's bytes" and "took the second
    /// frame's bytes" are different byte strings. The bottom `STEP` rows (the rows this step
    /// actually adds) are untouched by the overlay.
    fn two_frames() -> (Observation, Observation) {
        let image = TestImage::from_structures(CROSS_PX, DOC_ROWS, 11, 19, &mixed());
        let mut script = ScrollScript::new(
            &image,
            VIEWPORT,
            vec![StepSpec::move_by(STEP).with_dynamic(0.6)],
        );
        let first = script.take(0);
        let second = script.take(1);
        (first, second)
    }

    fn frame_rows(frame: &Observation, first_row: u32, end_row: u32) -> Vec<u8> {
        let row_bytes = (CROSS_PX * 4) as usize;
        frame.pixels()[first_row as usize * row_bytes..end_row as usize * row_bytes].to_vec()
    }

    #[test]
    fn only_the_new_rows_are_written() {
        let (first, second) = two_frames();
        let mut canvas = RecoveredImage::new(Axis::Vertical, CROSS_PX as u64, VIEWPORT_BUDGET);
        canvas.start(&first);

        // The Skip path (§16.3's duplicate detection) writes nothing at all.
        assert_eq!(canvas.append_confirmed(&second, 0), StepWrite::Skipped);
        assert_eq!(canvas.primary_len(), VIEWPORT as u64);
        assert_eq!(canvas.bands().bands().len(), 1);

        assert_eq!(
            canvas.append_confirmed(&second, STEP as u64),
            StepWrite::Appended {
                first_row: VIEWPORT as u64,
                rows: STEP as u64,
            }
        );
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: 2,
                committed: 2,
                discarded: 0,
            },
        );

        // The rows above the step's reach are the first frame's, untouched.
        assert_eq!(
            canvas.rows(0, STEP as u64),
            frame_rows(&first, 0, STEP as u32)
        );
        // The new rows are the *bottom* of the second frame: its viewport ends at the new content
        // end, so the content rows `[old_end, new_end)` are the frame's rows `[extent − step, extent)`.
        assert_eq!(
            canvas.rows(VIEWPORT as u64, STEP as u64),
            frame_rows(&second, VIEWPORT - STEP as u32, VIEWPORT)
        );

        // The overlap is where the two write rules differ, so first prove that they *do* differ
        // here: a row-overwriting composer would have put the second frame's rows 0..780 at content
        // rows 120..900, and the fixture made those two byte strings different.
        let kept = frame_rows(&first, STEP as u32, VIEWPORT);
        let overwritten = frame_rows(&second, 0, VIEWPORT - STEP as u32);
        assert_ne!(
            kept, overwritten,
            "the fixture produced identical bytes in the overlap, so this test cannot tell §17.3's rule from a row overwrite"
        );
        assert_eq!(canvas.rows(STEP as u64, (VIEWPORT - STEP as u32) as u64), kept);
    }

    #[test]
    fn band_height_never_drops_below_a_quarter_of_the_extent() {
        for extent in [16u32, 32, 100, 300, 900, 2160] {
            for shift in -(extent as i32)..=(extent as i32) {
                let band = band_height(extent, shift) as i64;
                // §17.2's first term: the new frame covers at most half the viewport.
                assert!(
                    band >= (extent / 2) as i64,
                    "extent {extent}, shift {shift}: band {band} is under extent/2"
                );
                // §17.3's verifiability constraint: whatever the step, the overlap keeps a quarter
                // of the viewport, so `gain` and `coverage` are measured on something.
                assert!(
                    band - shift as i64 >= (extent / 4) as i64,
                    "extent {extent}, shift {shift}: the overlap is {} but the band has to leave extent/4 = {}",
                    band - shift as i64,
                    extent / 4
                );
                // And the band has to be big enough to *contain* the rows the step adds.
                assert!(
                    band >= shift.max(0) as i64,
                    "extent {extent}, shift {shift}: the band is smaller than the new content"
                );
            }
        }

        // The two regimes and the crossover between them (extent 900: extent/4 = 225, extent/2 = 450).
        assert_eq!(band_height(900, 0), 450);
        assert_eq!(band_height(900, 225), 450);
        assert_eq!(band_height(900, 226), 451);
        assert_eq!(band_height(900, 900), 1125);
        assert_eq!(band_height(900, -300), 450);
    }

    /// Exit condition ③ in executable form: the `Skip` path is answered **before** the estimator.
    ///
    /// The funnel that puts the duplicate check in front of the estimator is `P3.09`'s; what can be
    /// proved today is that the check costs nothing — `Scratch::builds` is where the estimator's own
    /// work shows up (`P1.13`), and it stays at zero across a duplicate frame while the estimator's
    /// entry point does move it.
    #[test]
    fn a_duplicate_frame_is_answered_without_touching_the_estimator() {
        let image = TestImage::from_structures(CROSS_PX, DOC_ROWS, 11, 19, &mixed());
        let mut script = ScrollScript::new(&image, VIEWPORT, vec![StepSpec::repeat()]);
        let first = script.take(0);
        let duplicate = script.take(1);
        assert_eq!(
            first.pixels(),
            duplicate.pixels(),
            "the fixture did not repeat the frame, so there is nothing to skip"
        );

        let mut scratch = Scratch::new();
        assert_eq!(
            zero_shift_status(&first.view(), &duplicate.view()),
            Status::Confirmed { d: 0 }
        );
        assert_eq!(
            scratch.builds(),
            0,
            "the duplicate check went through the estimator"
        );

        // Counterfactual: the estimator's entry point does build images, so the counter above is
        // measuring the thing it claims to measure.
        let candidates = candidates_1d(&first.view(), &duplicate.view(), 0, 8);
        {
            let views = scratch.pool(&first.view(), &duplicate.view());
            let _ = score_candidates_2d(views.previous(), views.current(), &candidates);
        }
        assert!(
            scratch.builds() > 0,
            "the estimator did no work at all, so the counter proves nothing"
        );
    }

    // --- both directions, and the reference frame (§17.1, §17.4; task P1.19) ---

    /// The document's rows `[first, end)` as one packed buffer — what the canvas must equal when it
    /// has recovered a stretch of the document.
    fn document_rows(image: &TestImage, first: u32, end: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((end - first) as usize * CROSS_PX as usize * 4);
        for y in first..end {
            out.extend_from_slice(image.row(y));
        }
        out
    }

    /// One full pass of the estimator over two observations: layer 1 (`candidates_1d`), layer 2
    /// (`score_candidates_2d`) and layer 3 (`refine_winner`).
    ///
    /// The four gates are `P1.13`'s subject, and assembling them into a decision is the session's
    /// (`P3.09`). What this helper exists for is the **composition**: it insists on the exact integer
    /// so that the drift test fails when the canvas, not the estimator, is what moved.
    fn estimate_once(previous: &Observation, current: &Observation, expected: i32) -> Option<i32> {
        let mut scratch = Scratch::new();
        let candidates = candidates_1d(&previous.view(), &current.view(), expected, 8);
        if candidates.is_empty() {
            return None;
        }
        let scored = {
            let views = scratch.pool(&previous.view(), &current.view());
            score_candidates_2d(views.previous(), views.current(), &candidates)
        };
        let refined = {
            let views = scratch.full_resolution(&previous.view(), &current.view());
            refine_winner(views.previous(), views.current(), &scored)
        };
        refined.map(|winner| winner.d)
    }

    #[test]
    fn scrolling_up_produces_a_prepend_not_a_duplicate() {
        const DOC: u32 = 2400;
        const START: u32 = 400;
        let image = TestImage::from_structures(CROSS_PX, DOC, 11, 19, &mixed());
        // The capture starts **mid-document**. That is the only situation in which a canvas can be
        // extended upward at all: a canvas whose first row is the document's first row has nothing
        // above it, and a step that would go above it is refused by the document, not by us.
        let mut script = ScrollScript::starting_at(
            &image,
            VIEWPORT,
            START,
            vec![StepSpec::move_by(300), StepSpec::move_by(-600)],
        );
        let mut canvas = RecoveredImage::new(Axis::Vertical, CROSS_PX as u64, VIEWPORT_BUDGET);
        let mut viewport = ViewportState::new(VIEWPORT);
        canvas.start(&script.take(0));
        assert_eq!(viewport.position(), 0, "the first frame is the canvas' origin");

        let second = script.take(1);
        assert_eq!(
            viewport.apply(&mut canvas, &second, 300),
            StepWrite::Appended {
                first_row: VIEWPORT as u64,
                rows: 300
            }
        );
        assert_eq!(canvas.primary_len(), 1200);
        let before = canvas.rows(0, 1200);

        let third = script.take(2);
        assert_eq!(
            viewport.apply(&mut canvas, &third, -600),
            StepWrite::Prepended { rows: 300 }
        );
        assert_eq!(
            canvas.primary_len(),
            1500,
            "the canvas grew by the rows that were actually new, not by the step"
        );
        assert_eq!(viewport.position(), 0, "the viewport's top is the canvas' new top");

        // No duplicate: every row that used to be the canvas is still there, 300 rows further down.
        assert_eq!(canvas.rows(300, 1200), before);
        // The new content is the frame's **first** 300 rows: the frame's viewport now starts at the
        // new top, so "above" is the front of the frame and not the back.
        assert_eq!(canvas.rows(0, 300), frame_rows(&third, 0, 300));
        // And the canvas is the document, row for row: document rows 100..1600.
        assert_eq!(canvas.rows(0, 1500), document_rows(&image, 100, 1600));
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: 2,
                committed: 2,
                discarded: 0,
            },
        );
    }

    #[test]
    fn a_small_rollback_that_is_fully_covered_is_contained() {
        let image = TestImage::from_structures(CROSS_PX, DOC_ROWS, 11, 19, &mixed());
        let mut script = ScrollScript::new(
            &image,
            VIEWPORT,
            vec![StepSpec::move_by(120), StepSpec::move_by(-40)],
        );
        let mut canvas = RecoveredImage::new(Axis::Vertical, CROSS_PX as u64, VIEWPORT_BUDGET);
        let mut viewport = ViewportState::new(VIEWPORT);
        canvas.start(&script.take(0));

        let second = script.take(1);
        assert_eq!(
            viewport.apply(&mut canvas, &second, 120),
            StepWrite::Appended {
                first_row: VIEWPORT as u64,
                rows: 120
            }
        );
        let before = canvas.rows(0, 1020);

        let third = script.take(2);
        assert_ne!(
            frame_rows(&third, 0, VIEWPORT),
            frame_rows(&second, 0, VIEWPORT),
            "the fixture did not move the frame, so Contained would be vacuous"
        );
        assert_eq!(viewport.apply(&mut canvas, &third, -40), StepWrite::Contained);
        assert_eq!(canvas.primary_len(), 1020, "a contained step writes nothing");
        assert_eq!(canvas.rows(0, 1020), before);
        assert_eq!(viewport.position(), 80, "the viewport moved even though the canvas did not");
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: 2,
                committed: 2,
                discarded: 0,
            },
        );
    }

    #[test]
    fn one_hundred_confirmed_steps_leave_zero_drift() {
        const STEPS: usize = 100;
        const STEP_PX: u32 = 37;
        const DOC: u32 = STEPS as u32 * STEP_PX + VIEWPORT;
        let image = TestImage::from_structures(CROSS_PX, DOC, 11, 19, &mixed());
        let specs: Vec<StepSpec> = (0..STEPS).map(|_| StepSpec::move_by(STEP_PX as i32)).collect();
        let mut script = ScrollScript::new(&image, VIEWPORT, specs);
        let mut canvas = RecoveredImage::new(Axis::Vertical, CROSS_PX as u64, VIEWPORT_BUDGET);
        canvas.start(&script.take(0));
        let mut viewport = ViewportState::new(VIEWPORT);
        let mut expected = STEP_PX as i32;

        for k in 1..=STEPS {
            let current = script.take(k);
            let position = u64::try_from(viewport.position()).expect("the viewport went negative");
            // The reference is the **canvas**, never the previous frame (§17.4, N3). This is the
            // whole point of the test: if the composition ever wrote the wrong rows, the next step's
            // estimate would be made against them and the error would compound.
            let reference = Observation::new(
                canvas.rows(position, VIEWPORT as u64),
                Rect::new(0, 0, CROSS_PX as i32, VIEWPORT as i32),
                k as i64,
                (CROSS_PX, VIEWPORT),
                Axis::Vertical,
            )
            .expect("the reference viewport is packed and matches its region");
            let document_first = (k as u32 - 1) * STEP_PX;
            assert_eq!(
                reference.pixels(),
                document_rows(&image, document_first, document_first + VIEWPORT),
                "the canvas stopped being the document at step {k}"
            );

            let d = estimate_once(&reference, &current, expected)
                .unwrap_or_else(|| panic!("step {k}: the estimator had no answer at all"));
            assert_eq!(
                d, STEP_PX as i32,
                "step {k}: the estimator did not recover the scripted step"
            );
            assert_eq!(
                viewport.apply(&mut canvas, &current, d),
                StepWrite::Appended {
                    first_row: VIEWPORT as u64 + (k as u64 - 1) * STEP_PX as u64,
                    rows: STEP_PX as u64,
                }
            );
            expected = d;
        }

        assert_eq!(
            canvas.primary_len(),
            VIEWPORT as u64 + STEPS as u64 * STEP_PX as u64
        );
        assert_eq!(
            canvas.rows(0, canvas.primary_len()),
            document_rows(&image, 0, DOC),
            "the recovered image drifted away from the document"
        );
        canvas.assert_invariants(
            CROSS_PX as u64,
            StepTally {
                step: STEPS as u64,
                committed: STEPS as u64,
                discarded: 0,
            },
        );
    }
}
