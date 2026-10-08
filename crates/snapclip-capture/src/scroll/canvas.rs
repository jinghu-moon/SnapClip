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
//! Not here yet: the write path (`P1.18`: `band_height` and old-pixel-wins commits), the LRU and the
//! disk spill (`P1.20`), and the export sink (`P4.01`). Until then the module is exercised by its own
//! tests and the crate's warning budget stays at zero (`docs/31` §4.1).
#![allow(dead_code)]

use crate::scroll::observation::Axis;

/// Bytes per pixel in a band: BGRA, the format every capture path produces (`docs/30` §17.5).
pub(crate) const BYTES_PER_PIXEL: u64 = 4;

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

#[cfg(test)]
mod tests {
    use super::{Band, CoverageMap, RecoveredImage, StepTally};
    use crate::scroll::observation::Axis;
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
}
