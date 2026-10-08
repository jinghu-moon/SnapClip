//! What the estimator is allowed to answer, and what the session is allowed to do with it
//! (`docs/30` §16.9, §16.10, §16.11; task `P1.04`).
//!
//! Three answers exist, and they are not three points on one axis:
//!
//! * `Confirmed` — the four gates in §16.4–§16.5 passed and the controller prior (§16.6) did not
//!   object. The canvas may change.
//! * `Uncertain` — a shift was measured, but the evidence does not carry a decision. The canvas
//!   must **not** change and the session must keep going (`docs/30` §16.10). This is the state that
//!   makes C2 possible: a single bad frame is not a reason to end a scroll session.
//! * `None` — a candidate existed and no gate accepted it, or the estimator walked into the
//!   undefined branch of §16.9 (a return value of exactly `±N/2`/`±M/2`, where the phase-correlation
//!   formulation divides by zero and the reference implementation falls back to an out-of-range
//!   value). A `None` is a **visible** answer, not a missing one (G12).
//!
//! The three states live in the `Status` enum together with the shift they measured, so a `None`
//! has nowhere to hide a value: there is no `d` field next to the flag, no `Option<i32>` accessor
//! and no `0`/`i32::MIN` sentinel. That is `docs/31` `P1.04`'s exit condition ② — a bare
//! `i32`-plus-flag would be two spellings of "no answer", and the second spelling is always the one
//! a caller forgets to check.
//!
//! `StepEffect` is the *only* place the §16.10 table is written down, so "do not commit the canvas
//! but keep the session" is one match arm instead of a rule every consumer re-derives.
//!
//! Not here yet: `scene_cut` (`P1.12` puts it in `Evidence`, so that it never becomes a
//! `StopReason`), the `ĝ` prior (`P1.08`), the full-resolution third layer (`P1.07`) and the gates
//! themselves (`P1.08`–`P1.11`); all of them feed the `score` this file's formulas name.
//!
//! `P1.06` adds §15.4's second layer: the first thing in the funnel allowed to *argue*, because it
//! is the first that compares two-dimensional structure (F-02 — a periodic carrier satisfies any
//! one-dimensional measure at several shifts at once).

// The first consumer of everything in this file is `P1.05` (layer 1) / `P1.06` (ZNCC) / `P1.12`
// (the session loop). Until then the module is exercised only by its own tests, and the crate's
// warning budget stays at zero (`docs/31` §4.1).
#![allow(dead_code)]

use crate::scroll::observation::ObservationView;

/// The §16.7 weights, in one place because `E-ACC-1` calibrates them (`docs/30` §16.11 marks every
/// one of these as a startup value, not a law).
///
/// `score` covers three orthogonal questions (does it match / how much better than not moving /
/// how spread out is the evidence) and sums to 1; `confidence` is the same three plus how far ahead
/// the winner is, and also sums to 1. Both were taken from the reference implementation's
/// `estimator.rs:640` and `:1248-1252` — adopted because the decomposition is orthogonal and
/// normalised, not because the reference wrote it.
const SCORE_ZNCC2D: f32 = 0.60;
const SCORE_GAIN: f32 = 0.25;
const SCORE_COVERAGE: f32 = 0.15;
const CONFIDENCE_ZNCC2D: f32 = 0.40;
const CONFIDENCE_COVERAGE: f32 = 0.25;
const CONFIDENCE_GAIN: f32 = 0.20;
const CONFIDENCE_MARGIN: f32 = 0.15;

/// `coverage` saturates at 12 bands (`docs/30` §16.7, from the reference implementation's `:632`).
/// Without the saturation a long band set would systematically outscore a short one.
const COVERAGE_SATURATION_TILES: f32 = 12.0;

/// What the estimator measured, with no decision of its own.
///
/// Every field is one of the terms §16.7's two formulas name. `coverage` is *derived* from `tiles`
/// (`min(1, tiles/12)`) rather than stored beside it, so the two can never disagree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Evidence {
    /// Full-resolution ZNCC of the winning candidate (`docs/30` §16.7). Only `P1.07`'s third layer
    /// measures at full resolution; §15.4 ③ makes that value the only one that may reach a
    /// `displacement`, so the second layer's own `zncc2d` is deliberately not this one.
    pub(crate) zncc2d: f32,
    /// Residual gain over "nothing moved" (`docs/30` §16.3, gate two — the only gate that must
    /// pass; measured by `P1.06`'s second layer, at its own resolution).
    pub(crate) gain: f32,
    /// `(score(best) − score(second)) / score(best)` (`docs/30` §16.5, gate four; `P1.10`).
    pub(crate) margin: f32,
    /// How many bands support the winner (`docs/30` §16.4, gate three; `P1.09` adds §16.4's
    /// independence rule, which is why the second layer counts adjacent tiles separately).
    pub(crate) tiles: u32,
}

impl Evidence {
    /// The share of the band budget the winner used, saturating at
    /// [`COVERAGE_SATURATION_TILES`] (`docs/30` §16.7).
    pub(crate) fn coverage(&self) -> f32 {
        coverage_of(self.tiles)
    }

    /// `docs/30` §16.7 — used for ranking, for gate four's `margin`, and as the comparison basis
    /// for the ORB second opinion (`P1.23`).
    pub(crate) fn score(&self) -> f32 {
        score_of(self.zncc2d, self.gain, self.coverage())
    }

    /// `docs/30` §16.7 — the number the session reports, and the one that decides `Uncertain`.
    pub(crate) fn confidence(&self) -> f32 {
        CONFIDENCE_ZNCC2D * self.zncc2d
            + CONFIDENCE_COVERAGE * self.coverage()
            + CONFIDENCE_GAIN * self.gain
            + CONFIDENCE_MARGIN * self.margin
    }
}

/// The three answers, each carrying the shift it measured — including the two that do not get to
/// change the canvas.
///
/// `Confirmed` and `Uncertain` carry a `d` because both measured one; `None` carries nothing,
/// which is what makes `docs/31` `P1.04`'s "no value in the `None` state" a fact of the type
/// rather than a rule in a comment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Status {
    /// Four gates passed and the controller prior did not object: the canvas may change.
    Confirmed { d: i32 },
    /// A shift was measured, but the evidence does not carry a decision: no commit, session
    /// continues, reference frame advances (`docs/30` §16.10).
    Uncertain { d: i32 },
    /// No candidate passed the gates, `d = 0` with mismatching row fingerprints, or the estimator
    /// hit §16.9's boundary value. Never commit, always continue.
    None,
}

/// What the session may do with an [`Evidence`]: the §16.10 table, in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepEffect {
    /// Append/prepend the observation and advance the reference frame.
    Commit,
    /// Leave the canvas alone, **keep the session alive**, advance the reference frame (C2).
    Continue,
    /// Do nothing at all, including not advancing the reference frame: the observation repeats the
    /// previous one row for row, so advancing would only lose the frame this step was measured
    /// against (`docs/30` §16.10's note). Decided before a [`Displacement`] exists — see
    /// [`Displacement::effect`], which never returns it.
    Skip,
}

/// A decision plus the evidence behind it. `pub(crate)` on purpose: `confidence` is calibrated
/// (`docs/30` §16.11), so it must not become a compatibility promise (`docs/30` §27.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Displacement {
    status: Status,
    confidence: f32,
    evidence: Evidence,
}

impl Displacement {
    /// A measured shift the gates accepted. `confidence` comes from the evidence, so it cannot
    /// disagree with it.
    pub(crate) fn confirmed(d: i32, evidence: Evidence) -> Self {
        Self {
            status: Status::Confirmed { d },
            confidence: evidence.confidence(),
            evidence,
        }
    }

    /// A measured shift the gates did not accept. Note the argument order matches
    /// [`Self::confirmed`] because both *did* measure a shift.
    pub(crate) fn uncertain(d: i32, evidence: Evidence) -> Self {
        Self {
            status: Status::Uncertain { d },
            confidence: evidence.confidence(),
            evidence,
        }
    }

    /// No answer — deliberately without a shift parameter.
    pub(crate) fn none(evidence: Evidence) -> Self {
        Self {
            status: Status::None,
            confidence: evidence.confidence(),
            evidence,
        }
    }

    /// Consumers read the shift by matching here; there is no `d() -> Option<i32>`, because that
    /// accessor would put the second spelling of "no answer" back into the API.
    pub(crate) fn status(&self) -> &Status {
        &self.status
    }

    pub(crate) fn confidence(&self) -> f32 {
        self.confidence
    }

    pub(crate) fn evidence(&self) -> &Evidence {
        &self.evidence
    }

    /// `docs/30` §16.10's `status → action` column. Only `Confirmed` commits; `Uncertain` and
    /// `None` both continue the session, which is the whole of C2 in one arm.
    pub(crate) fn effect(&self) -> StepEffect {
        match self.status {
            Status::Confirmed { .. } => StepEffect::Commit,
            Status::Uncertain { .. } | Status::None => StepEffect::Continue,
        }
    }
}

// --- layer 1 (`P1.05`) --------------------------------------------------------------------------

/// How many candidates layer 1 may propose: `K = 8` (`docs/30` §15.6, from the reference
/// implementation's `MAX_CANDIDATES`).
pub(crate) const CANDIDATE_LIMIT: usize = 8;

/// FNV-1a 64. `docs/30` §15.4 asks for a "64-bit fold" of a line without fixing the function; the
/// first implementation of it was `scroll/perf_probe.rs`'s `row_digest`, and this is that function
/// moved to the place layer 1 can share it. The `E-PERF-1` numbers in `docs/30` §23.3.1 were
/// measured on *this* fold, so replacing it with a cheaper, more collision-prone one is a
/// re-measurement, not a refactor.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[inline]
fn fold(state: u64, byte: u8) -> u64 {
    (state ^ byte as u64).wrapping_mul(FNV_PRIME)
}

/// The fold of one contiguous primary line — a row, for a vertical observation.
pub(crate) fn line_digest(bytes: &[u8]) -> u64 {
    let mut state = FNV_OFFSET;
    for &byte in bytes {
        state = fold(state, byte);
    }
    state
}

/// The same fold over a column of the observation's pixels. `P1.02`'s `T-AXIS-1` requires that a
/// transposed document's column digest to the original document's row, so the bytes are visited in
/// the same order — `B, G, R, A` of one pixel after another, top to bottom.
fn column_digest(pixels: &[u8], stride: usize, x: usize, height: u32) -> u64 {
    let mut state = FNV_OFFSET;
    for y in 0..height as usize {
        let at = y * stride + x * 4;
        for byte in &pixels[at..at + 4] {
            state = fold(state, *byte);
        }
    }
    state
}

/// The digest of every primary line of `view`, in order. `Axis` owns that branch (`P1.02`: a second
/// `match` on the axis is a second place for it to be wrong).
pub(crate) fn primary_digests(view: &ObservationView<'_>) -> Vec<u64> {
    let extent = view.primary_extent();
    let mut digests = Vec::with_capacity(extent as usize);
    if view.axis().is_vertical() {
        for index in 0..extent {
            digests.push(line_digest(view.row(index)));
        }
    } else {
        for index in 0..extent {
            digests.push(column_digest(
                view.pixels(),
                view.row_stride(),
                index as usize,
                view.height(),
            ));
        }
    }
    digests
}

/// How many primary lines agree *exactly* when `current` is attributed `shift` (`docs/30` §15.4).
///
/// The count is deliberately not normalised by the overlap here: `support` is raw evidence, and the
/// overlap it was measured over is what §16.2's gates normalise when (and only when) it becomes
/// part of a score. Normalising early would hide exactly the carrier ambiguity `P1.05`'s refactor
/// test is about.
pub(crate) fn support_at(previous: &[u64], current: &[u64], shift: i32) -> u32 {
    let overlap = current.len() as i64 - shift.unsigned_abs() as i64;
    if overlap <= 0 {
        return 0;
    }
    let previous_first = shift.max(0) as usize;
    let current_first = (-(shift as i64)).max(0) as usize;
    let mut support = 0;
    for index in 0..overlap as usize {
        if previous[previous_first + index] == current[current_first + index] {
            support += 1;
        }
    }
    support
}

/// One shift layer 1 proposes, with the number of primary lines that agree behind it.
///
/// It carries no score and no verdict: §16's gates are the first thing in the design allowed to
/// compare anything at all, and they act on ratios over `Evidence`, not on a line count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) d: i32,
    pub(crate) support: u32,
}

/// Up to [`CANDIDATE_LIMIT`] candidates, best first.
///
/// A fixed array rather than `ArrayVec`: `arrayvec` is only a transitive dependency of this
/// workspace, and N7/G11 forbid adding a crate dependency for it. Because the capacity is the
/// §15.6 `K`, insertion is a shift-by-one inside eight elements and the hot path never allocates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CandidateSet {
    items: [Candidate; CANDIDATE_LIMIT],
    len: usize,
}

impl CandidateSet {
    pub(crate) fn new() -> Self {
        Self {
            items: [Candidate { d: 0, support: 0 }; CANDIDATE_LIMIT],
            len: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Best first, so a caller that only looks at the first candidate still sees the ranking it is
    /// trusting rather than an unspoken one.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Candidate> {
        self.items[..self.len].iter()
    }

    /// Keeps the ranking `(support desc, |d| asc, d asc)`. The tie-break is what makes the answer
    /// deterministic on the carrier images where several shifts are equally supported (`P1.05`'s
    /// exit condition ②).
    fn insert(&mut self, candidate: Candidate) {
        let mut at = self.len;
        for (index, existing) in self.items[..self.len].iter().enumerate() {
            if ranks_before(candidate, *existing) {
                at = index;
                break;
            }
        }
        if at >= CANDIDATE_LIMIT {
            return;
        }
        let last = self.len.min(CANDIDATE_LIMIT - 1);
        for index in (at..last).rev() {
            self.items[index + 1] = self.items[index];
        }
        self.items[at] = candidate;
        if self.len < CANDIDATE_LIMIT {
            self.len += 1;
        }
    }
}

fn ranks_before(candidate: Candidate, existing: Candidate) -> bool {
    match candidate.support.cmp(&existing.support) {
        core::cmp::Ordering::Greater => true,
        core::cmp::Ordering::Less => false,
        core::cmp::Ordering::Equal => {
            (candidate.d.unsigned_abs(), candidate.d) < (existing.d.unsigned_abs(), existing.d)
        }
    }
}

/// `docs/30` §15.4's first layer: the cheap one-dimensional search that proposes where to look.
///
/// It takes `expected` (the controller's `n·ĝ`) and a half-width rather than a bare window: §15.6's
/// `W_search` *is* a half-width, and a window without a centre cannot express the automatic mode at
/// all — a 120 px step with `W_search = 36` would be outside its own search range.
///
/// The axis is read from the observations rather than passed in beside them: a second source for it
/// could disagree with the frames, which is the mistake `P1.02`'s `T-AXIS-1` was rewritten to stop
/// making. `d = 0` gets no exemption: §16.1's `d_0 = 0` is read here as "the zero-shift hypothesis
/// is always evaluated", which happens whenever the window covers it, and it then competes under the
/// same ranking rule as every other shift (hard-inserting it would both exceed the §15.6 `K` and
/// grant it a privilege no other candidate has).
pub(crate) fn candidates_1d(
    previous: &ObservationView<'_>,
    current: &ObservationView<'_>,
    expected: i32,
    window: i32,
) -> CandidateSet {
    let previous_digests = primary_digests(previous);
    let current_digests = primary_digests(current);
    let half = window.max(0);
    let mut candidates = CandidateSet::new();
    for shift in expected.saturating_sub(half)..=expected.saturating_add(half) {
        candidates.insert(Candidate {
            d: shift,
            support: support_at(&previous_digests, &current_digests, shift),
        });
    }
    candidates
}

// --- layer 2 (`P1.06`) --------------------------------------------------------------------------

/// §15.6's decimation: layer 2 reads a 4× area-averaged image, and §15.4 ③ keeps full resolution
/// for layer 3 alone.
pub(crate) const DOWNSAMPLE: u32 = 4;

/// §16.4's band width along the primary axis: 32 px. Layer 2 counts how many of these agree.
pub(crate) const TILE_ROWS: u32 = 32;

/// The shortest band layer 2 will score, in downsampled rows: §15.6's `H_match = max(16, H/2)` with
/// the decimation already applied to the 16.
const MATCH_MIN_ROWS: u32 = 16 / DOWNSAMPLE;

/// A tile's own ZNCC has to reach this before the tile counts as supporting the shift.
///
/// §16.4 replaces the reference implementation's "at least 8 inlier matches" with "the number of
/// bands whose inlier pixel ratio is at least 0.5". A *pixel-level* tolerance would break the
/// brightness invariance this layer is required to have (a contrast change moves absolute
/// residuals, not correlations), so the half is kept on the layer's own measure. `E-ACC-1` owns the
/// value — §16.11 lists every number in this family as a startup value.
const TILE_SUPPORT_ZNCC: f32 = 0.5;

/// Below this residual at zero shift there is nothing for `gain` to divide by.
const GAIN_RMSE_FLOOR: f32 = 1e-3;

/// `Y = (77R + 150G + 29B) >> 8` (§15.6). Pixels are BGRA (§2.2), so the channels arrive reversed.
#[inline]
fn luma(blue: u8, green: u8, red: u8) -> u32 {
    (77 * red as u32 + 150 * green as u32 + 29 * blue as u32) >> 8
}

/// A 4× area-averaged luma image, **oriented so that rows are primary lines**.
///
/// Layer 2 is a two-dimensional measure of a one-dimensional shift, so it needs the primary axis to
/// be the row axis wherever it came from: a horizontal observation is pooled transposed. This keeps
/// the axis branch in one function (`P1.02`'s lesson: a second branch is a second place to be
/// wrong), and it costs a transposed read only on the horizontal path, whose performance is already
/// deferred to `E-PERF-3`.
///
/// Pooling happens once per frame and every candidate is then scored against the same two images:
/// §15.4 ② prices the layer at `O(H_match·W/16)` **per candidate**, which only holds if the
/// decimation is not repeated inside the candidate loop. When `P1.13` introduces `Scratch`, these
/// two buffers become its fields so that a step needing both layer 2 and layer 3 pools only once.
struct Gray {
    /// Cross-axis extent in cells.
    width: u32,
    /// Primary-axis extent in cells.
    height: u32,
    data: Vec<u8>,
}

impl Gray {
    fn pooled(view: &ObservationView<'_>) -> Self {
        let vertical = view.axis().is_vertical();
        let (cross, primary) = if vertical {
            (view.width(), view.height())
        } else {
            (view.height(), view.width())
        };
        let width = cross / DOWNSAMPLE;
        let height = primary / DOWNSAMPLE;
        let mut data = Vec::with_capacity((width * height) as usize);
        for row in 0..height {
            for column in 0..width {
                let mut sum = 0;
                for cell_y in 0..DOWNSAMPLE {
                    for cell_x in 0..DOWNSAMPLE {
                        sum += luma_at(
                            view,
                            column * DOWNSAMPLE + cell_x,
                            row * DOWNSAMPLE + cell_y,
                            vertical,
                        );
                    }
                }
                let cells = DOWNSAMPLE * DOWNSAMPLE;
                data.push(((sum + cells / 2) / cells) as u8);
            }
        }
        Self {
            width,
            height,
            data,
        }
    }

    #[inline]
    fn at(&self, cross: u32, primary: u32) -> u32 {
        self.data[(primary * self.width + cross) as usize] as u32
    }
}

/// One pixel's luma, addressed in the frame's cross/primary terms.
#[inline]
fn luma_at(view: &ObservationView<'_>, cross: u32, primary: u32, vertical: bool) -> u32 {
    let (x, y) = if vertical {
        (cross, primary)
    } else {
        (primary, cross)
    };
    let at = y as usize * view.row_stride() + x as usize * 4;
    let pixels = view.pixels();
    luma(pixels[at], pixels[at + 1], pixels[at + 2])
}

/// Where the two frames are compared for one shift, in downsampled rows.
///
/// §15.1 says the `match_region` sits at one end of the content without fixing which. This takes
/// the **end of the overlap** in the previous frame's rows, which is the only choice that stays
/// inside both frames for either sign of `d` without a sign branch: for `d ≥ 0` it is the last rows
/// of `previous`, and for `d < 0` the last rows of `current`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MatchBand {
    previous_first: u32,
    current_first: u32,
    rows: u32,
}

/// The tallest band of `wanted` rows that both frames can support at `shift`, or `None` when they
/// have no overlap at all.
fn match_band(
    previous_height: u32,
    current_height: u32,
    shift: i32,
    wanted: u32,
) -> Option<MatchBand> {
    let end = (previous_height as i32).min(current_height as i32 + shift);
    let start = shift.max(0);
    let overlap = end - start;
    if overlap <= 0 {
        return None;
    }
    let rows = wanted.min(overlap as u32);
    let previous_first = end - rows as i32;
    Some(MatchBand {
        previous_first: previous_first as u32,
        current_first: (previous_first - shift) as u32,
        rows,
    })
}

/// The same band with the correspondence on the `current` side moved by `extra` rows, shortened so
/// that it stays inside the frame. Used to sample the correlation surface around a candidate
/// (§15.4 ②'s three-point difference).
fn probe_band(band: MatchBand, extra: i32) -> Option<MatchBand> {
    let rows = band.rows.checked_sub(extra.unsigned_abs())?;
    if rows == 0 {
        return None;
    }
    let first_row = (-extra).max(0) as u32;
    Some(MatchBand {
        previous_first: band.previous_first + first_row,
        current_first: (band.current_first as i32 + extra + first_row as i32) as u32,
        rows,
    })
}

/// Zero-mean normalised cross-correlation over `rows` cells of the band, starting at `first_row`.
///
/// This is F-06's formula, not `imageproc`'s: the mean is subtracted on **both** sides, which is
/// exactly what makes an affine change of the intensity scale invisible to it. The denominator is
/// the product of the two variances; when either is zero — a flat band, or a band one cell wide —
/// the correlation is undefined and the answer is `0`, never NaN (F-02, `P1.06`'s second case).
fn band_zncc(
    previous: &Gray,
    current: &Gray,
    band: MatchBand,
    first_row: u32,
    rows: u32,
) -> f32 {
    let width = previous.width.min(current.width);
    if width == 0 || rows == 0 {
        return 0.0;
    }
    let count = (width * rows) as f64;
    let (mut sum_previous, mut sum_current) = (0.0f64, 0.0f64);
    let (mut sum_previous_sq, mut sum_current_sq) = (0.0f64, 0.0f64);
    let mut sum_product = 0.0f64;
    for row in 0..rows {
        let previous_primary = band.previous_first + first_row + row;
        let current_primary = band.current_first + first_row + row;
        for column in 0..width {
            let left = previous.at(column, previous_primary) as f64;
            let right = current.at(column, current_primary) as f64;
            sum_previous += left;
            sum_current += right;
            sum_previous_sq += left * left;
            sum_current_sq += right * right;
            sum_product += left * right;
        }
    }
    let variance = (count * sum_previous_sq - sum_previous * sum_previous)
        * (count * sum_current_sq - sum_current * sum_current);
    if variance <= 0.0 {
        return 0.0;
    }
    let zncc = (count * sum_product - sum_previous * sum_current) / variance.sqrt();
    (zncc as f32).clamp(-1.0, 1.0)
}

/// Root-mean-square residual of the band, averaged over its cells.
fn band_rmse(previous: &Gray, current: &Gray, band: MatchBand) -> f32 {
    let width = previous.width.min(current.width);
    if width == 0 || band.rows == 0 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    for row in 0..band.rows {
        for column in 0..width {
            let difference = previous.at(column, band.previous_first + row) as f64
                - current.at(column, band.current_first + row) as f64;
            sum += difference * difference;
        }
    }
    ((sum / (width * band.rows) as f64).sqrt()) as f32
}

/// §16.3: `gain = 1 − RMSE(shift)/RMSE(0)` — the share of the *zero-shift* residual this shift
/// explains.
///
/// It is the design's content-independent measure (an absolute similarity is near 1 on a blank
/// page), and it is **invariant to a contrast scale** because both residuals carry it. When the two
/// frames are already identical at zero shift the ratio is `0/0`: the answer is `0` ("nothing to
/// gain") rather than NaN. The floor at `−1` is where the shift's residual is twice the zero
/// shift's, past which the number only exists to sort candidates — §16's gates reject it long
/// before.
fn gain_of(rmse_at_shift: f32, rmse_at_zero: f32) -> f32 {
    if rmse_at_zero <= GAIN_RMSE_FLOOR {
        return 0.0;
    }
    (1.0 - rmse_at_shift / rmse_at_zero).clamp(-1.0, 1.0)
}

/// How many 32 px tiles of the band agree with the shift (§16.4's band count, which replaces "at
/// least 8 inlier matches"; §16.7's `coverage` saturates the number at 12).
///
/// Only whole tiles count: a partially filled tile correlates over fewer cells, and comparing it to
/// the same threshold would make the tail of every band systematically weaker evidence. Adjacent
/// tiles count separately here — §16.4's independence rule (`|i − j| ≥ 2`) is gate three's, and
/// `P1.09` is where it lands.
fn supporting_tiles(previous: &Gray, current: &Gray, band: MatchBand) -> u32 {
    let tile = TILE_ROWS / DOWNSAMPLE;
    let mut count = 0;
    let mut first = 0;
    while first + tile <= band.rows {
        if band_zncc(previous, current, band, first, tile) >= TILE_SUPPORT_ZNCC {
            count += 1;
        }
        first += tile;
    }
    count
}

/// §15.4 ②'s three-point difference: `ZNCC(d−1) + ZNCC(d+1) − 2·ZNCC(d)`, sampled by moving the
/// correspondence inside the *same* band. Negative at a peak, and the more negative the sharper the
/// peak — which is how a carrier's pile of equally sharp peaks looks different from one real one.
fn curvature_of(previous: &Gray, current: &Gray, band: MatchBand) -> f32 {
    let centre = band_zncc(previous, current, band, 0, band.rows);
    let side = |extra: i32| match probe_band(band, extra) {
        Some(probed) => band_zncc(previous, current, probed, 0, probed.rows),
        None => 0.0,
    };
    side(-1) + side(1) - 2.0 * centre
}

/// `coverage(tiles) = min(1, tiles/12)` (§16.7). One implementation, used by `Evidence` and by the
/// scored candidates, so the two can never drift.
fn coverage_of(tiles: u32) -> f32 {
    (tiles as f32 / COVERAGE_SATURATION_TILES).min(1.0)
}

/// `score = 0.60·zncc2d + 0.25·gain + 0.15·coverage` (§16.7), likewise shared with [`Evidence`].
fn score_of(zncc2d: f32, gain: f32, coverage: f32) -> f32 {
    SCORE_ZNCC2D * zncc2d + SCORE_GAIN * gain + SCORE_COVERAGE * coverage
}

/// A candidate's shift on layer 2's grid, rounded to nearest.
///
/// Layer 2 answers at 4 px granularity, so two shifts inside one cell are *the same measurement* —
/// `P1.07` separates them on the full-resolution grid, which is what §15.4 ③ means by "the only
/// value that reaches the final `displacement`".
fn round_to_grid(shift: i32) -> i32 {
    let half = (DOWNSAMPLE / 2) as i32;
    if shift >= 0 {
        (shift + half) / DOWNSAMPLE as i32
    } else {
        -((-shift + half) / DOWNSAMPLE as i32)
    }
}

/// One candidate with what layer 2 measured about it, and nothing that decides anything.
///
/// It carries the **full-resolution** shift it was proposed at, next to measurements taken on the
/// decimated grid: the number the caller will one day commit is layer 3's, and keeping the proposal
/// here means layer 2 never has to convert back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScoredCandidate {
    pub(crate) d: i32,
    /// §16.7's `zncc2d` at this layer's resolution (§15.4 ③ reports the full-resolution one).
    pub(crate) zncc2d: f32,
    /// §16.3's residual gain over the zero shift.
    pub(crate) gain: f32,
    /// How many whole 32 px tiles agree (§16.4's band count; independence is `P1.09`'s).
    pub(crate) tiles: u32,
    /// The three-point difference of `zncc2d` around the candidate (§15.4 ②).
    pub(crate) curvature: f32,
    /// §16.7's combination, the key this set is ranked by.
    pub(crate) score: f32,
}

/// Up to [`CANDIDATE_LIMIT`] scored candidates, best first.
///
/// Same fixed-array shape and same reason as [`CandidateSet`], and the same determinism
/// requirement: ranked by `(score desc, curvature asc, |d| asc, d asc)`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScoredSet {
    items: [ScoredCandidate; CANDIDATE_LIMIT],
    len: usize,
}

impl ScoredSet {
    pub(crate) fn new() -> Self {
        Self {
            items: [ScoredCandidate {
                d: 0,
                zncc2d: 0.0,
                gain: 0.0,
                tiles: 0,
                curvature: 0.0,
                score: 0.0,
            }; CANDIDATE_LIMIT],
            len: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &ScoredCandidate> {
        self.items[..self.len].iter()
    }

    fn insert(&mut self, candidate: ScoredCandidate) {
        let mut at = self.len;
        for (index, existing) in self.items[..self.len].iter().enumerate() {
            if scored_ranks_before(candidate, *existing) {
                at = index;
                break;
            }
        }
        if at >= CANDIDATE_LIMIT {
            return;
        }
        let last = self.len.min(CANDIDATE_LIMIT - 1);
        for index in (at..last).rev() {
            self.items[index + 1] = self.items[index];
        }
        self.items[at] = candidate;
        if self.len < CANDIDATE_LIMIT {
            self.len += 1;
        }
    }
}

/// §16.7 says `score` is what ranks candidates; §15.4 ② says the three-point difference is what
/// tells a peak from a plateau, so it breaks an exact tie in `score` (sharper first). Then comes the
/// distance to the grid point this layer actually measured at: two candidates inside one 4 px cell
/// are one measurement, and the one that *is* that measurement is the honest thing to report (the
/// candidate closest to `round_to_grid(d)·4`). The shift tie-break last makes the order total and
/// deterministic — the same requirement `P1.05`'s [`CandidateSet`] answers for layer 1.
fn scored_ranks_before(candidate: ScoredCandidate, existing: ScoredCandidate) -> bool {
    match candidate.score.total_cmp(&existing.score) {
        core::cmp::Ordering::Greater => true,
        core::cmp::Ordering::Less => false,
        core::cmp::Ordering::Equal => match candidate.curvature.total_cmp(&existing.curvature) {
            core::cmp::Ordering::Less => true,
            core::cmp::Ordering::Greater => false,
            core::cmp::Ordering::Equal => {
                (grid_distance(candidate.d), candidate.d.unsigned_abs(), candidate.d)
                    < (grid_distance(existing.d), existing.d.unsigned_abs(), existing.d)
            }
        },
    }
}

/// How far a full-resolution shift is from the grid point layer 2 measured it at.
fn grid_distance(shift: i32) -> u32 {
    shift.abs_diff(round_to_grid(shift) * DOWNSAMPLE as i32)
}

/// `docs/30` §15.4's second layer: the first thing in the funnel allowed to *argue*, because it is
/// the first that compares two-dimensional structure (F-02).
///
/// It scores every candidate layer 1 proposed — not only the best one, which is what §15.4's
/// "对每一个候选算条带一致性" is for — and returns them ranked by §16.7's `score`. Candidates that a
/// finite frame cannot even hold (`match_band` returns `None`) are dropped rather than scored as
/// zero: "this shift was not measured" and "this shift measured badly" are different facts, and the
/// gates are entitled to tell them apart.
pub(crate) fn score_candidates_2d(
    previous: &ObservationView<'_>,
    current: &ObservationView<'_>,
    candidates: &CandidateSet,
) -> ScoredSet {
    let previous_gray = Gray::pooled(previous);
    let current_gray = Gray::pooled(current);
    let wanted = (previous_gray.height.min(current_gray.height) / 2).max(MATCH_MIN_ROWS);
    let mut scored = ScoredSet::new();
    for candidate in candidates.iter() {
        let shift = round_to_grid(candidate.d);
        let Some(band) = match_band(
            previous_gray.height,
            current_gray.height,
            shift,
            wanted,
        ) else {
            continue;
        };
        let zncc2d = band_zncc(&previous_gray, &current_gray, band, 0, band.rows);
        let tiles = supporting_tiles(&previous_gray, &current_gray, band);
        let gain = gain_of(
            band_rmse(&previous_gray, &current_gray, band),
            band_rmse(
                &previous_gray,
                &current_gray,
                match_band(
                    previous_gray.height,
                    current_gray.height,
                    0,
                    wanted,
                )
                .expect("the zero shift always overlaps a non-empty frame"),
            ),
        );
        let curvature = curvature_of(&previous_gray, &current_gray, band);
        scored.insert(ScoredCandidate {
            d: candidate.d,
            zncc2d,
            gain,
            tiles,
            curvature,
            score: score_of(zncc2d, gain, coverage_of(tiles)),
        });
    }
    scored
}

#[cfg(test)]
mod tests {
    use super::{
        CANDIDATE_LIMIT, Candidate, Displacement, Evidence, ScoredCandidate, ScoredSet, Status,
        StepEffect, candidates_1d, primary_digests, score_candidates_2d, support_at,
    };
    use crate::scroll::observation::{Axis, Observation};
    use crate::scroll::displacement::{grid_distance, round_to_grid, DOWNSAMPLE};
    use crate::scroll::testkit::{ScrollScript, StepSpec, Structure, TestImage};

    fn evidence() -> Evidence {
        Evidence {
            zncc2d: 0.90,
            gain: 0.40,
            margin: 0.30,
            tiles: 6,
        }
    }

    #[test]
    fn a_displacement_cannot_carry_a_value_in_the_none_state() {
        // `docs/31` §6 `P1.04`. The claim is a *type-level* one, so its executable form is a
        // `match`: this compiles today, with no arm reading a `d` that the `None` state does not
        // have — and it stops compiling the moment a fourth state appears or the shift is spelled
        // as a sentinel beside a flag (`docs/30` §16.10: `None` means "no gate accepted a
        // candidate", never "the shift happened to be zero").
        fn read(status: &Status) -> (Option<i32>, &'static str) {
            match status {
                Status::Confirmed { d } => (Some(*d), "confirmed"),
                Status::Uncertain { d } => (Some(*d), "uncertain"),
                Status::None => (None, "none"),
            }
        }

        let confirmed = Displacement::confirmed(120, evidence());
        let uncertain = Displacement::uncertain(-40, evidence());
        let none = Displacement::none(evidence());

        assert_eq!(read(confirmed.status()), (Some(120), "confirmed"));
        assert_eq!(read(uncertain.status()), (Some(-40), "uncertain"));
        assert_eq!(read(none.status()), (None, "none"));

        // Three answers, not three spellings of one.
        assert_ne!(confirmed.status(), uncertain.status());
        assert_ne!(uncertain.status(), none.status());

        // `docs/30` §16.10: only `Confirmed` changes the canvas; the other two keep the session
        // alive and advance the reference frame instead of committing.
        assert_eq!(confirmed.effect(), StepEffect::Commit);
        assert_eq!(uncertain.effect(), StepEffect::Continue);
        assert_eq!(none.effect(), StepEffect::Continue);
    }

    #[test]
    fn every_status_has_exactly_one_effect() {
        // `docs/30` §16.10's table, written out as data rather than as a second `match` over the
        // same enum: a copied `match` would only restate the implementation and pass no matter
        // which way the arms went (`P1.02`'s `T-AXIS-1` was exactly that mistake).
        let table = [
            (
                Displacement::confirmed(120, evidence()),
                StepEffect::Commit,
                "append/prepend, advance the reference frame, update the step estimate",
            ),
            (
                Displacement::uncertain(-40, evidence()),
                StepEffect::Continue,
                "no commit, keep the session, advance the reference frame, do not update the estimate",
            ),
            (
                Displacement::none(evidence()),
                StepEffect::Continue,
                "no commit, keep the session, advance the reference frame",
            ),
        ];

        for (displacement, expected, why) in table {
            assert_eq!(
                displacement.effect(),
                expected,
                "{why} (status {:?})",
                displacement.status()
            );
            // `Skip` is not reachable from a displacement: it belongs to the step that saw the
            // previous observation twice, row for row, and that is decided before any shift
            // exists (`docs/30` §16.10's note) — so `effect()` must never return it.
            assert_ne!(displacement.effect(), StepEffect::Skip);
        }

        // The two failure states agree on the action and differ only in the shift they carry.
        assert_eq!(table[1].0.effect(), table[2].0.effect());
        assert!(matches!(table[0].0.status(), Status::Confirmed { d } if *d == 120));
        assert!(matches!(table[1].0.status(), Status::Uncertain { d } if *d == -40));
        assert!(matches!(table[2].0.status(), Status::None));
    }

    #[test]
    fn the_two_formulas_use_the_documented_weights() {
        // `docs/30` §16.7 with §16.11's startup values, hand-computed here rather than
        // recomputed, so a change to a weight has to change this test too.
        let evidence = Evidence {
            zncc2d: 0.90,
            gain: 0.40,
            margin: 0.30,
            tiles: 6,
        };
        assert_eq!(evidence.coverage(), 0.5); // 6 of 12 bands: not saturated yet
        // 0.60·0.90 + 0.25·0.40 + 0.15·0.50
        assert!((evidence.score() - 0.715).abs() < 1e-6, "{}", evidence.score());
        // 0.40·0.90 + 0.25·0.50 + 0.20·0.40 + 0.15·0.30
        assert!(
            (evidence.confidence() - 0.61).abs() < 1e-6,
            "{}",
            evidence.confidence()
        );

        // Both weight sets sum to 1. That is what makes the two numbers comparable across sessions
        // and runs, and it is the first property `E-ACC-1` would break if it calibrated by hand.
        let perfect = Evidence {
            zncc2d: 1.0,
            gain: 1.0,
            margin: 1.0,
            tiles: 12,
        };
        assert!((perfect.score() - 1.0).abs() < 1e-6);
        assert!((perfect.confidence() - 1.0).abs() < 1e-6);

        // `coverage` is a pure function of the band count and saturates at 12 (`docs/30` §16.7):
        // a 500-band session must not outscore a 12-band one for the same match quality.
        assert_eq!(Evidence { tiles: 30, ..perfect }.coverage(), 1.0);
        assert_eq!(Evidence { tiles: 12, ..perfect }.coverage(), 1.0);
        assert_eq!(Evidence { tiles: 0, ..perfect }.coverage(), 0.0);
    }

    /// A document whose only structure is a 19-row carrier: every row is byte-identical to the row
    /// 19 rows away from it, so a one-dimensional search cannot tell a shift from that shift plus a
    /// multiple of 19. `band_height = 19` is what makes the period exact — `from_structures` hands
    /// the structure `y % band_height`, so nothing seams at the band boundary.
    fn periodic_document(structure: Structure) -> TestImage {
        TestImage::from_structures(640, 60 * 19, 7, 19, &[structure])
    }

    #[test]
    fn one_d_candidates_include_the_true_shift_on_a_periodic_image() {
        // Layer 1 is allowed to be wrong; it is not allowed to be silent (`docs/31` §6 `P1.05`).
        // On this document the true shift is the best-supported one, but it is not alone: the
        // carrier makes a family of shifts match every row they can compare.
        let image = periodic_document(Structure::HorizontalBars { period: 19 });
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(7)]);
        let previous = script.take(0);
        let current = script.take(1);
        let truth = script.truth(1);
        assert_eq!(truth, 7);

        let candidates = candidates_1d(&previous.view(), &current.view(), truth, 40);
        assert!(!candidates.is_empty(), "layer 1 proposed nothing");
        assert!(candidates.len() <= CANDIDATE_LIMIT);
        assert!(
            candidates.iter().any(|candidate| candidate.d == truth),
            "layer 1 lost the true shift {truth}: {:?}",
            candidates
                .iter()
                .map(|candidate| (candidate.d, candidate.support))
                .collect::<Vec<_>>()
        );

        // The imposter is not an artifact of a threshold: one carrier period away, *every* row the
        // shift can compare matches, exactly as it does at the true shift. That is what `support`
        // counts, and it is why this layer hands back a set rather than an answer.
        let previous_digests = primary_digests(&previous.view());
        let current_digests = primary_digests(&current.view());
        let extent = current.view().primary_extent() as i32;
        let imposter = truth + 19;
        assert!(
            candidates.iter().any(|candidate| candidate.d == imposter),
            "the carrier supports {imposter} perfectly and the layer dropped it"
        );
        assert_eq!(
            support_at(&previous_digests, &current_digests, truth),
            (extent - truth) as u32
        );
        assert_eq!(
            support_at(&previous_digests, &current_digests, imposter),
            (extent - imposter) as u32
        );

        // Exit condition ② also asks for determinism — including the order, which is where an
        // unstable sort or a hash-ordered container would show up.
        let again = candidates_1d(&previous.view(), &current.view(), truth, 40);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| (candidate.d, candidate.support))
                .collect::<Vec<_>>(),
            again
                .iter()
                .map(|candidate| (candidate.d, candidate.support))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn one_d_candidates_are_not_treated_as_a_verdict() {
        // `docs/30` §15.4: "它的输出永远只是候选，不是结论". Two properties make that true of the
        // *type* rather than of a convention, and both are pinned here:
        //
        //  * the destructuring below has no `..`, so the day `Candidate` grows a `confidence` or a
        //    `status`, this test stops compiling;
        //  * `support` is a *count of agreeing primary lines*, so there is nothing here a §16 gate
        //    could compare against a threshold — those act on ratios over `Evidence`.
        let image = periodic_document(Structure::HorizontalBars { period: 19 });
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(7)]);
        let previous = script.take(0);
        let current = script.take(1);
        let candidates = candidates_1d(&previous.view(), &current.view(), 7, 40);

        let Candidate { d: _, support } = *candidates
            .iter()
            .next()
            .expect("layer 1 proposed nothing to check");
        let extent = current.view().primary_extent();
        assert!(
            support <= extent,
            "support is a line count, not a score: {support} > {extent}"
        );
        for candidate in candidates.iter() {
            let Candidate { d, support } = *candidate;
            assert!(d.unsigned_abs() <= extent, "candidate {d} is out of range");
            assert!(support <= extent);
        }

        // A set, not a point: evidence that cannot separate shifts is reported as several of them.
        assert!(candidates.len() > 1, "one candidate is a verdict in disguise");
    }

    #[test]
    fn a_periodic_line_carrier_does_not_decide_the_step() {
        // Measured lesson from `scroll_probe.rs`: 19 px text rows produced correlation peaks
        // (`corr ≈ 0.6–0.8`) at shifts that are not multiples of the row pitch. The 1D layer sees
        // the same thing — a *family* of perfectly matching shifts — so normalising by the overlap
        // ties them, which is exactly why §16's gates need two-dimensional evidence.
        let image = periodic_document(Structure::TextRows { line: 19 });
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(7)]);
        let previous = script.take(0);
        let current = script.take(1);
        let candidates = candidates_1d(&previous.view(), &current.view(), 7, 40);
        assert!(
            candidates.iter().any(|candidate| candidate.d == 7),
            "the carrier hid the true shift: {:?}",
            candidates
                .iter()
                .map(|candidate| (candidate.d, candidate.support))
                .collect::<Vec<_>>()
        );

        let previous_digests = primary_digests(&previous.view());
        let current_digests = primary_digests(&current.view());
        let extent = current.view().primary_extent() as i32;
        let mut perfect_shifts = 0;
        for shift in [7i32, 7 + 19, 7 + 38, 7 - 19] {
            let overlap = (extent - shift.abs()) as u32;
            assert!(overlap > 0);
            assert_eq!(
                support_at(&previous_digests, &current_digests, shift),
                overlap,
                "shift {shift} is not a perfect match over its own overlap"
            );
            perfect_shifts += 1;
        }
        assert!(perfect_shifts >= 3);
    }

    /// A document whose bands cycle through five different structures, so that no single period
    /// explains a wrong shift: the carrier bands tie on their own, and the other bands separate
    /// them. This is the shape §15.4 ② exists for.
    fn mixed_document() -> TestImage {
        TestImage::from_structures(
            640,
            60 * 19,
            11,
            19,
            &[
                Structure::Checker { cell: 12 },
                Structure::TextRows { line: 19 },
                Structure::NoiseBlocks { cell: 8 },
                Structure::Gradient,
                Structure::HorizontalBars { period: 19 },
            ],
        )
    }

    /// An affine change of the intensity scale, applied to the colour channels (alpha is not
    /// intensity). This is what auto-exposure does between two frames of the same page.
    fn relight(frame: &Observation, contrast: f32, brightness: f32) -> Observation {
        let mut pixels = frame.pixels().to_vec();
        for pixel in pixels.chunks_exact_mut(4) {
            for channel in &mut pixel[..3] {
                let lit = *channel as f32 * contrast + brightness;
                *channel = lit.round().clamp(0.0, 255.0) as u8;
            }
        }
        Observation::new(
            pixels,
            frame.region(),
            frame.qpc(),
            frame.size(),
            frame.axis(),
        )
        .expect("a relit frame keeps the geometry of the frame it came from")
    }

    /// How far two scores may drift before the invariance claim is false: the relighting is applied
    /// in `f32` and then quantised back to `u8`, so the two frames differ by up to half a level per
    /// channel. This is a tolerance on the *quantisation*, not on the definition — the definition
    /// says the two scores are equal.
    const SCORE_TOLERANCE: f32 = 1e-2;

    /// `(d, score)` ordered by `d`, so two scored sets can be compared candidate by candidate even
    /// when their internal order is one of the things being asserted.
    fn scores_by_shift(set: &ScoredSet) -> Vec<(i32, f32)> {
        let mut pairs: Vec<(i32, f32)> = set
            .iter()
            .map(|candidate| (candidate.d, candidate.score))
            .collect();
        pairs.sort_by_key(|(shift, _)| *shift);
        pairs
    }

    fn rank_of(set: &ScoredSet) -> Vec<i32> {
        set.iter().map(|candidate| candidate.d).collect()
    }

    /// `docs/31` §6 `P1.06`'s first RED case.
    #[test]
    fn the_score_is_invariant_to_brightness_and_contrast() {
        // `docs/30` §16.7's `score` is built on ZNCC, whose defining property is that an affine
        // change of the intensity scale leaves it alone (F-06 is precisely the warning that a
        // correlation without mean subtraction does *not* have this property). Only `current` is
        // relit: the page brightening between two frames must not change which shift wins.
        let image = mixed_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);
        let candidates = candidates_1d(&previous.view(), &current.view(), 120, 40);
        assert!(candidates.len() > 1, "nothing to be invariant about");

        let baseline = score_candidates_2d(&previous.view(), &current.view(), &candidates);
        assert_eq!(
            baseline.iter().next().map(|candidate| candidate.d),
            Some(120),
            "the second layer did not put the true shift first: {:?}",
            scores_by_shift(&baseline)
        );

        for (contrast, brightness, why) in [
            (
                0.6f32,
                40.0f32,
                "a dimmer frame, i.e. contrast and offset together",
            ),
            (0.8, -10.0, "lower contrast"),
            (1.0, -20.0, "an offset only"),
        ] {
            let relit = relight(&current, contrast, brightness);
            let scored = score_candidates_2d(&previous.view(), &relit.view(), &candidates);
            assert_eq!(
                rank_of(&scored),
                rank_of(&baseline),
                "the ranking changed under {why}: {:?} then {:?}",
                scores_by_shift(&baseline),
                scores_by_shift(&scored)
            );
            // The assertion is over the term that is invariant *by construction*: `zncc2d` is
            // mean-free on both sides (F-06), so an affine change between the frames cannot move it.
            // The other two cannot be pinned and should not be: §16.3's `gain` is a ratio of
            // absolute residuals, and `tiles` counts threshold crossings of a tile-level ZNCC (a
            // dimmer frame requantises the pooled luma and *more* tiles clear the threshold —
            // measured here: 10 → 12 out of 12 at contrast 0.6/offset +40, which moved that
            // candidate's score 0.5817 → 0.5889 while the ranking stayed put). See `docs/30`
            // §15.4.2 for the numbers and for what that means for §16.5's margin.
            for (before, after) in baseline.iter().zip(scored.iter()) {
                assert_eq!(
                    before.d, after.d,
                    "the sets are not over the same shifts under {why}"
                );
                assert!(
                    (before.zncc2d - after.zncc2d).abs() < SCORE_TOLERANCE,
                    "the invariant part of the score moved under {why} at shift {}: zncc2d {} → {}, \
                     tiles {} → {}, gain {} → {}, score {} → {}",
                    before.d,
                    before.zncc2d,
                    after.zncc2d,
                    before.tiles,
                    after.tiles,
                    before.gain,
                    after.gain,
                    before.score,
                    after.score
                );
            }
        }
    }

    /// `docs/31` §6 `P1.06`'s second RED case — F-02's failure mode.
    #[test]
    fn uniformly_flat_input_scores_zero_instead_of_nan() {
        // A uniformly flat band makes ZNCC's denominator zero, and `0/0` is NaN. A NaN score would
        // not stay local: `NaN < x` is false, so it wins every comparison in §16 and the gates
        // would pass on evidence that does not exist. Zero says the honest thing instead.
        //
        // `Structure::Flat` is flat *within a band*: its level is a constant plus a term in the band
        // index, so a document made of several bands is a staircase, not a uniform surface. One band
        // as tall as the document is what "uniformly flat" costs here.
        let image = TestImage::from_structures(320, 80 * 19, 3, 80 * 19, &[Structure::Flat]);
        let mut script = ScrollScript::new(&image, 300, vec![StepSpec::move_by(10)]);
        let previous = script.take(0);
        let current = script.take(1);
        let candidates = candidates_1d(&previous.view(), &current.view(), 10, 8);
        let scored = score_candidates_2d(&previous.view(), &current.view(), &candidates);
        assert!(!scored.is_empty(), "the second layer scored nothing");

        for candidate in scored.iter() {
            // Destructured without `..`, so the field set is pinned (`P1.04`'s lesson: a second
            // spelling of "no answer" — here an `Option<f32>` — must break the build, not a test).
            let ScoredCandidate {
                d,
                zncc2d,
                gain,
                tiles,
                curvature,
                score,
            } = *candidate;
            assert!(
                zncc2d.is_finite()
                    && gain.is_finite()
                    && curvature.is_finite()
                    && score.is_finite(),
                "shift {d} produced a non-finite number: zncc2d {zncc2d}, gain {gain}, \
                 curvature {curvature}, score {score}"
            );
            assert_eq!(
                zncc2d, 0.0,
                "a flat band has nothing to correlate at shift {d}, so it must report zero"
            );
            assert_eq!(tiles, 0, "a flat band supports no shift, but {d} counted tiles");
            assert_eq!(score, 0.0, "flat input must score exactly zero at shift {d}");
        }
    }

    /// REFACTOR (`P1.06`): the boundary of what the second layer can do, pinned as a fact.
    ///
    /// This document is a stack of horizontal bars: its level depends on the primary coordinate
    /// only, so it carries **no** two-dimensional information, and a measure that reads both
    /// directions sees exactly what a one-dimensional one saw — the same picture at `7`, `26`, `45`
    /// and `−12`. F-02 is a statement about *real* content (text has vertical strokes, checkers vary
    /// in both directions) and layer 2 is what exploits it; a document that does not vary across the
    /// band stays ambiguous **on purpose**. §16.5's margin and §16.6's peak-family detection are
    /// what must reject it one layer up — the answer is not to loosen a threshold down here.
    #[test]
    fn a_document_that_varies_only_along_the_primary_axis_stays_ambiguous() {
        let image =
            TestImage::from_structures(640, 40 * 19, 7, 19, &[Structure::HorizontalBars { period: 19 }]);
        let mut script = ScrollScript::new(&image, 600, vec![StepSpec::move_by(7)]);
        let previous = script.take(0);
        let current = script.take(1);

        let candidates = candidates_1d(&previous.view(), &current.view(), 7, 40);
        let truth = 7;
        let aliases: [i32; 3] = [26, 45, -12];
        let previous_digests = primary_digests(&previous.view());
        let current_digests = primary_digests(&current.view());
        let extent = previous.height();
        for shift in aliases {
            // Each alias matches perfectly *over its own overlap*, which is exactly why a
            // one-dimensional measure cannot prefer one: the raw support differs only by the overlap
            // it had.
            assert_eq!(
                support_at(&previous_digests, &current_digests, shift),
                extent - shift.unsigned_abs(),
                "the fixture no longer describes a carrier: shift {shift} does not match its overlap"
            );
        }
        assert_eq!(
            support_at(&previous_digests, &current_digests, truth),
            extent - truth.unsigned_abs()
        );

        let scored = score_candidates_2d(&previous.view(), &current.view(), &candidates);
        let truth_score = scored
            .iter()
            .find(|candidate| candidate.d == truth)
            .expect("the truth is not among the scored candidates")
            .score;
        assert!(
            scored
                .iter()
                .filter(|candidate| aliases.contains(&candidate.d))
                .any(|candidate| candidate.score >= truth_score),
            "the second layer separated a document that has no cross-axis structure at all: \
             top is {:?}",
            scored
                .iter()
                .map(|candidate| (candidate.d, candidate.score))
                .collect::<Vec<_>>()
        );
    }

    /// REFACTOR (`P1.06`): one measurement per 4 px cell of this layer's own grid.
    ///
    /// `round_to_grid` means two candidates inside one cell are the *same* measurement — equal
    /// correlations, equal gain, equal tiles — and the one that *is* that measurement (the cell's
    /// own shift) is the one layer 2 reports. Separating them is §15.4 ③'s job at full resolution
    /// (`P1.07`), which is why this layer must not pretend it already has.
    #[test]
    fn shifts_inside_one_cell_share_one_measurement() {
        let image = mixed_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);
        let candidates = candidates_1d(&previous.view(), &current.view(), 120, 40);
        let scored = score_candidates_2d(&previous.view(), &current.view(), &candidates);

        let cell_of = |shift: i32| round_to_grid(shift) * DOWNSAMPLE as i32;
        let cell = cell_of(120);
        let members: Vec<ScoredCandidate> = scored
            .iter()
            .copied()
            .filter(|candidate| cell_of(candidate.d) == cell)
            .collect();
        assert!(
            members.len() > 1,
            "the fixture no longer lands several candidates in one cell: {:?}",
            scored
                .iter()
                .map(|candidate| (candidate.d, candidate.score))
                .collect::<Vec<_>>()
        );
        for member in &members {
            assert_eq!(
                (member.zncc2d, member.gain, member.tiles, member.curvature),
                (members[0].zncc2d, members[0].gain, members[0].tiles, members[0].curvature),
                "shift {} did not share its cell's measurement",
                member.d
            );
        }
        assert!(
            members.iter().any(|member| member.d == cell),
            "the cell's own shift {cell} was not among the candidates, so this case says nothing"
        );
        assert_eq!(
            scored.iter().next().map(|candidate| candidate.d),
            Some(cell),
            "the cell's own shift must win the tie it cannot otherwise break: {:?}",
            scored
                .iter()
                .map(|candidate| (candidate.d, grid_distance(candidate.d)))
                .collect::<Vec<_>>()
        );
    }

    /// REFACTOR (`P1.06`): §19.4's promise is that both axes share one estimator. The horizontal
    /// document is the vertical one transposed (`ScrollScript::horizontal` does that internally), so
    /// this case is what tells the pooling's axis branch from a second, divergent implementation.
    #[test]
    fn the_horizontal_axis_scores_through_the_same_code() {
        let image = mixed_document();
        let mut script = ScrollScript::horizontal(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);
        assert_eq!(previous.axis(), Axis::Horizontal);

        let candidates = candidates_1d(&previous.view(), &current.view(), 120, 40);
        let scored = score_candidates_2d(&previous.view(), &current.view(), &candidates);
        let best = scored
            .iter()
            .next()
            .expect("the second layer scored nothing on the horizontal axis");
        assert_eq!(
            best.d,
            120,
            "the horizontal axis did not find its own shift: {:?}",
            scored
                .iter()
                .map(|candidate| (candidate.d, candidate.score))
                .collect::<Vec<_>>()
        );
    }
}
