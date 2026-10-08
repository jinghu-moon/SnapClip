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
//! `StopReason`) and the `ĝ` prior (`P1.14`); both feed the `score` this file's formulas name.
//!
//! `P1.06` adds §15.4's second layer: the first thing in the funnel allowed to *argue*, because it
//! is the first that compares two-dimensional structure (F-02 — a periodic carrier satisfies any
//! one-dimensional measure at several shifts at once).
//!
//! `P1.07` adds §15.4 ③'s third and last layer. It is the first thing allowed to *decide a number*:
//! everything below it produces sets or measurements, and everything above it decides what to do
//! with one. It reads full resolution, its whole freedom is `±1` integer pixel ([`REFINE_NEIGHBOURHOOD`]),
//! and it answers with [`Refined`] — an `i32` and one correlation, with no field for a subpixel
//! offset. N3 says why: the observation is an integer pixel grid, so a fractional shift would be an
//! interpolation believed as a measurement.
//!
//! `P1.08` adds the first of §16.1's gates ([`gate_geometry`]) together with §16.9's ban on the
//! `±N/2` boundary, and keeps the verifiability floor ([`RHO_MIN`]) as a separate item: "how much
//! shift is physically possible" is a fact about overlap with no parameters, "how much overlap is
//! enough evidence" is a calibratable preference. V1 conflated them (`docs/30` §16.2), and the
//! conflation is exactly what makes a hard gate drift when someone tunes confidence.
//!
//! `P1.09` adds §16.1's second gate ([`gate_residual_gain`]) together with the thing that has to
//! exist for it to be callable: [`residual_gain`] answers `Option<f32>`, because at a zero shift the
//! §16.3 ratio is `0/0` and "undefined" would otherwise be flattened into `0.0` — a number a ranking
//! and a gate would treat as an ordinary measurement. That is why the module's "no sentinel" rule
//! (`P1.04`) has a second instance here: the type carries the distinction, and
//! [`zero_shift_status`] is the §16.3 fingerprint path the undefined case is routed to instead.
//!
//! `P1.10` adds §16.1's third gate ([`gate_support`]) and, more importantly, what "independent"
//! means in it: [`independent_support`] counts agreeing tiles only when they are **two apart**, so a
//! single wide patch cannot masquerade as four witnesses. That is also what §16.7's `coverage` is
//! defined on, so the number the score reads and the number the gate reads are the same number.
//!
//! `P1.11` adds §16.1's fourth and last gate ([`gate_margin`]). It is the only gate that reports
//! `Uncertain` instead of `None`, because the thing it rejects is a *measurement* — the winner and a
//! rival both scored, they just did not separate — and §16.10 gives those two answers different
//! actions. §16.6's manual-mode peak-family rule is the same threshold seen from the other side
//! (`1 − 0.85 == MIN_MARGIN`), so no second detector is built for it.
//!
//! `P1.12` adds §16.8's scene cut ([`is_scene_cut`], [`SceneCut`]), which is the estimator's only
//! statement about *the page* rather than about the frames: no candidate aligns, so the content was
//! rearranged and waiting is the right answer. The fact is carried on [`Evidence`], never in
//! [`Status`] — a changed page is not a failed session (C2), and the type is what guarantees the
//! session cannot end on one.

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

/// §16.8's first condition: the zero-shift similarity below which the frames differ *at rest*.
///
/// `E-ACC-1` calibrates the number; what is fixed is that it is a *similarity* floor applied before
/// any candidate is examined, which is why it can be measured on one band instead of on all of them.
pub(crate) const SCENE_CUT_SIMILARITY: f32 = 0.50;

/// §16.8's second condition: the alignment error above which a candidate counts as "did not align".
///
/// Also `E-ACC-1`'s. Both conditions must hold, and this one is asked of *every* candidate: one
/// candidate that aligns means the frames do belong to the same page.
pub(crate) const SCENE_CUT_ALIGNMENT_ERROR: f32 = 0.60;

/// The share of a band's tiles whose alignment error is averaged (§16.8's `alignment_error`).
///
/// Taken from the reference implementation's trimmed mean (`estimator.rs:559`: it keeps the lowest
/// 75% of the weight and drops the rest). The reason to trim at all is asymmetry: a page that
/// scrolled *and* had a popup open has most of its tiles aligning, and one region that legitimately
/// disagrees should not decide whether the page changed. The share is fixed rather than calibrated
/// because it is a robustness choice, not a threshold — `E-ACC-1` has nothing to tune here.
const ALIGNMENT_RETAINED: f32 = 0.75;

/// How many consecutive scene cuts make §16.8 decay the tile model instead of tolerating the frame.
///
/// Fixed at 3 (the reference implementation's `:984` resets on the third). One rearrangement should
/// not change what we believe about which region scrolls; three in a row is a different page.
pub(crate) const SCENE_CUT_DECAY_STREAK: u8 = 3;

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
    /// `(score(best) − score(second)) / score(best)` (`docs/30` §16.5, gate four; `P1.11`).
    pub(crate) margin: f32,
    /// How many **independent** bands support the winner (`docs/30` §16.4, gate three; the
    /// `|i − j| ≥ 2` rule landed in `P1.10`, so this is the count gate three compares against
    /// [`MIN_TILES`] and the one §16.7's `coverage` is defined on).
    pub(crate) tiles: u32,
    /// How many consecutive frames have looked like a different page (`docs/30` §16.8; `P1.12`).
    ///
    /// It lives here rather than beside the `status` for the reason §16.7 gives: a frame that shows
    /// a different page is a fact about *the page*, so it must not be a member of the enum the
    /// session uses to end itself. `Evidence` is the widest type every frame carries, which is
    /// exactly where "here is something the consumer should know but not act on by stopping" belongs.
    pub(crate) scene_cut: SceneCut,
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

    /// `docs/30` §16.8's consecutiveness, for the consumer that reacts to it (the tile model's
    /// decay, `§18.2`). The `P1.12` REFACTOR keeps it out of `Status` on purpose.
    pub(crate) fn scene_cut(&self) -> SceneCut {
        self.scene_cut
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
///
/// This is the **matching evidence** tile, i.e. the unit §16.4 counts supporters in. It is not the
/// temporal model's tile (§18.2's texture/update unit, which decides what to rewrite). Both are
/// 32 px because both came from the same reference estimator; they are separate identifiers so that
/// moving one does not silently move the other, and [`TILE_INDEPENDENCE_GAP`] carries the same note.
pub(crate) const TILE_ROWS: u32 = 32;

/// §15.6's `H_match` floor in **full-resolution** primary rows: 16, because a one-dimensional
/// sequence needs enough samples before "half the extent" means anything.
///
/// The floor is quoted in full-resolution units because two layers read it at two resolutions; each
/// divides by its own scale ([`match_rows`]) rather than keeping a second copy of the 16 that could
/// drift away from this one.
const MATCH_MIN_ROWS: u32 = 16;

/// §15.6's `H_match = max(16, H/2)` expressed in the units of the images about to be compared:
/// `scale` is how many pixels one cell covers ([`DOWNSAMPLE`] for layer 2, `1` for layer 3).
///
/// The `max(1)` is not decoration: a frame shorter than `2·scale` would otherwise ask for a band of
/// zero rows, and "no band" is spelled `None` by [`match_band`], not by an empty one.
fn match_rows(previous_height: u32, current_height: u32, scale: u32) -> u32 {
    (previous_height.min(current_height) / 2).max((MATCH_MIN_ROWS / scale).max(1))
}

/// A tile's own ZNCC has to reach this before the tile counts as supporting the shift.
///
/// §16.4 replaces the reference implementation's "at least 8 inlier matches" with "the number of
/// bands whose inlier pixel ratio is at least 0.5". A *pixel-level* tolerance would break the
/// brightness invariance this layer is required to have (a contrast change moves absolute
/// residuals, not correlations), so the half is kept on the layer's own measure. `E-ACC-1` owns the
/// value — §16.11 lists every number in this family as a startup value.
const TILE_SUPPORT_ZNCC: f32 = 0.5;

/// §16.4's independence distance, in tiles: two supporters are independent evidence of the same shift
/// only when their indices differ by at least this much (`|i − j| ≥ 2`).
///
/// Adjacent tiles share a border of pixels and, more importantly, share the *content* that produced
/// the correlation — a run of `m` agreeing tiles is `ceil(m/2)` pieces of evidence, not `m`. The gap
/// is a property of the rule, not a tunable, so it is a constant and not a parameter: a caller that
/// could set it to `1` could make gate three accept a single wide patch.
///
/// This grid is the **matching evidence** grid, and it is deliberately a different name from the
/// temporal model's tile grid (§18.2's `region.rs`-style texture/update tiles). Both are `32` px
/// today because both were taken from the same reference estimator, and that coincidence is exactly
/// why they must not share an identifier: changing one must not silently change the other.
const TILE_INDEPENDENCE_GAP: u32 = 2;

/// Below this residual at zero shift there is nothing for `gain` to divide by.
const GAIN_RMSE_FLOOR: f32 = 1e-3;

/// `Y = (77R + 150G + 29B) >> 8` (§15.6). Pixels are BGRA (§2.2), so the channels arrive reversed.
#[inline]
fn luma(blue: u8, green: u8, red: u8) -> u32 {
    (77 * red as u32 + 150 * green as u32 + 29 * blue as u32) >> 8
}

/// An area-averaged luma image at some integral scale, **oriented so that rows are primary lines**.
///
/// The two layers above layer 1 both measure a one-dimensional shift two-dimensionally, so they
/// need the primary axis to be the row axis wherever it came from: a horizontal observation is
/// pooled transposed. This keeps the axis branch in one function (`P1.02`'s lesson: a second branch
/// is a second place to be wrong), and it costs a transposed read only on the horizontal path,
/// whose performance is already deferred to `E-PERF-3`.
///
/// Pooling happens once per frame and every candidate is then scored against the same two images:
/// §15.4 ② prices layer 2 at `O(H_match·W/16)` **per candidate**, which only holds if the
/// decimation is not repeated inside the candidate loop. Layer 3 reads the same frames at scale 1,
/// so a step that needs both layers builds four images. [`Scratch`] owns them and reuses their
/// allocations across steps (`P1.13`).
///
/// It is `pub(crate)` rather than private because the layer functions below take it: they are the
/// crate's scroll API, and they take the *already pooled* image so that a step pools each scale
/// exactly once no matter how many layers ask.
pub(crate) struct Gray {
    /// Cross-axis extent in cells.
    width: u32,
    /// Primary-axis extent in cells.
    height: u32,
    data: Vec<u8>,
}

impl Gray {
    /// An empty image, sized by the first [`Self::scaled_into`]. The buffers of a [`Scratch`] start
    /// here and are reused from then on.
    fn empty() -> Self {
        Self {
            width: 0,
            height: 0,
            data: Vec::new(),
        }
    }

    /// Layer 2's image: §15.6's 4× decimation.
    fn pooled(view: &ObservationView<'_>) -> Self {
        Self::scaled(view, DOWNSAMPLE)
    }

    /// The same image at any integral scale, `1` included: an area average over `scale × scale`
    /// cells, which for `scale == 1` is simply that pixel's luma. Layer 2 reads it at
    /// [`DOWNSAMPLE`] and layer 3 at full resolution, and both go through this one loop, so the
    /// orientation rule ("rows are primary lines") and the rounding rule exist exactly once.
    fn scaled(view: &ObservationView<'_>, scale: u32) -> Self {
        let mut gray = Self::empty();
        gray.scaled_into(view, scale);
        gray
    }

    /// [`Self::scaled`] writing into an existing image, so the allocation survives the step
    /// (`docs/30` §22.5): the first step of a session pays for the buffers, the rest reuse them.
    fn scaled_into(&mut self, view: &ObservationView<'_>, scale: u32) {
        let vertical = view.axis().is_vertical();
        let (cross, primary) = if vertical {
            (view.width(), view.height())
        } else {
            (view.height(), view.width())
        };
        self.width = cross / scale;
        self.height = primary / scale;
        self.data.clear();
        self.data.reserve((self.width * self.height) as usize);
        for row in 0..self.height {
            for column in 0..self.width {
                let mut sum = 0;
                for cell_y in 0..scale {
                    for cell_x in 0..scale {
                        sum += luma_at(
                            view,
                            column * scale + cell_x,
                            row * scale + cell_y,
                            vertical,
                        );
                    }
                }
                let cells = scale * scale;
                self.data.push(((sum + cells / 2) / cells) as u8);
            }
        }
    }

    #[inline]
    fn at(&self, cross: u32, primary: u32) -> u32 {
        self.data[(primary * self.width + cross) as usize] as u32
    }
}

/// The two derived images of one step, borrowed from the [`Scratch`] that owns them.
///
/// The borrow is the ownership rule: nothing here can be handed to another thread while the step
/// that built it is running, which is exactly what §22.5 asks for.
pub(crate) struct Views<'a> {
    previous: &'a Gray,
    current: &'a Gray,
}

impl Views<'_> {
    pub(crate) fn previous(&self) -> &Gray {
        self.previous
    }

    pub(crate) fn current(&self) -> &Gray {
        self.current
    }
}

/// The four derived images a step can read, kept between steps so a session pays for the
/// allocations once (`docs/30` §22.5).
///
/// It is owned by the thread that drives the session and passed by `&mut`, which is the whole
/// mechanism: §22.5 forbids pooling these buffers across threads (a cross-thread pool is a lock, and
/// §21.1 has already established that there is exactly one scroll thread), so the type must not be
/// shareable by construction. Today it holds the two scales the layers read — layer 2 and the scene
/// cut at 4×, layer 3 at full resolution — and not yet §22.5's prefix sums or gradient map, because
/// neither has a consumer: `band_zncc` accumulates its sums in one pass per tile, and the gradient
/// map belongs to the sub-pixel refinement that `docs/30` §36 keeps as `N3`.
///
/// **There is no cache and no key.** An earlier version of this type hit "the same frame" by
/// comparing the buffer address, the length, the `qpc` and the axis, and the gate ablation caught
/// what that is worth: the answers for one case changed when the heap layout changed, because the
/// allocator hands a freed frame's address to the next frame and the fixture restarts `qpc` at zero
/// for every script. Address, length and timestamp are not an identity — the *step* is. So a caller
/// builds each scale once per step and keeps the [`Views`] alive for as long as it needs them; a
/// second call rebuilds, which is visible in [`Self::builds`] rather than silently wrong.
pub(crate) struct Scratch {
    /// `[0]`/`[1]` are the previous/current frames at [`DOWNSAMPLE`], `[2]`/`[3]` at scale 1.
    slots: [Gray; 4],
    builds: u32,
}

/// Layer 2 and the scene cut both read the frames at [`DOWNSAMPLE`].
const SLOT_POOLED_PREVIOUS: usize = 0;
const SLOT_POOLED_CURRENT: usize = 1;
/// Layer 3 reads them at scale 1.
const SLOT_FULL_PREVIOUS: usize = 2;
const SLOT_FULL_CURRENT: usize = 3;

impl Scratch {
    pub(crate) fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| Gray::empty()),
            builds: 0,
        }
    }

    /// Layer 2's two images (§15.6's 4× decimation).
    ///
    /// One call per step, shared by the second layer, gate two and the scene cut: that sharing is
    /// why the logarithms below take `&Gray` instead of the observations they came from.
    pub(crate) fn pool(
        &mut self,
        previous: &ObservationView<'_>,
        current: &ObservationView<'_>,
    ) -> Views<'_> {
        self.fill(SLOT_POOLED_PREVIOUS, previous, DOWNSAMPLE);
        self.fill(SLOT_POOLED_CURRENT, current, DOWNSAMPLE);
        Views {
            previous: &self.slots[SLOT_POOLED_PREVIOUS],
            current: &self.slots[SLOT_POOLED_CURRENT],
        }
    }

    /// Layer 3's two images (full resolution).
    pub(crate) fn full_resolution(
        &mut self,
        previous: &ObservationView<'_>,
        current: &ObservationView<'_>,
    ) -> Views<'_> {
        self.fill(SLOT_FULL_PREVIOUS, previous, 1);
        self.fill(SLOT_FULL_CURRENT, current, 1);
        Views {
            previous: &self.slots[SLOT_FULL_PREVIOUS],
            current: &self.slots[SLOT_FULL_CURRENT],
        }
    }

    fn fill(&mut self, index: usize, view: &ObservationView<'_>, scale: u32) {
        self.slots[index].scaled_into(view, scale);
        self.builds += 1;
    }

    /// How many images had to be built. `#[cfg(test)]` because its only consumer is the test that
    /// turns "the buffers are reused across steps" from a claim about the code's shape into an
    /// assertion.
    #[cfg(test)]
    fn builds(&self) -> u32 {
        self.builds
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
/// page), and it is **invariant to a contrast scale** because both residuals carry it.
///
/// The answer is an `Option` because the denominator can be undefined: when the two frames are
/// already identical at zero shift the ratio is `0/0`, and §16.3 routes that case to the fingerprint
/// path instead of calling gate two. Returning `0.0` there — which this function used to do — made
/// "undefined" indistinguishable from "this shift gains nothing", so a caller could rank the zero
/// shift and gate it like a measurement. `None` is what keeps that case out of the funnel, and it is
/// the second instance of `P1.04`'s rule that an absent answer gets a type rather than a sentinel.
///
/// The floor at `−1` is where the shift's residual is twice the zero shift's, past which the number
/// only exists to sort candidates — §16's gates reject it long before.
fn residual_gain(rmse_at_shift: f32, rmse_at_zero: f32) -> Option<f32> {
    if rmse_at_zero <= GAIN_RMSE_FLOOR {
        return None;
    }
    Some((1.0 - rmse_at_shift / rmse_at_zero).clamp(-1.0, 1.0))
}

/// What a candidate's `gain` becomes when the ratio is undefined, **for ranking only**.
///
/// §16.7's `score` needs a number for every candidate, and the zero shift is a candidate like any
/// other. `0.0` is the honest standing: the shift gains nothing over "nothing moved" because there is
/// nothing there to gain. The mapping lives here, at the one place a number is needed, so the
/// definition above stays `Option` and gate two never sees the substitution.
const GAIN_UNDEFINED_FOR_RANKING: f32 = 0.0;

/// How many of the band's 32 px tiles are **independent** evidence for the shift (§16.4's band count,
/// which replaces "at least 8 inlier matches"; §16.7's `coverage` saturates the number at 12).
///
/// Takes the indices of the tiles that cleared [`TILE_SUPPORT_ZNCC`], in increasing order, and returns
/// the size of the largest subset whose members are pairwise at least [`TILE_INDEPENDENCE_GAP`] apart.
///
/// Greedy from the first supporter is exact for pairwise- separated points on a line: taking the
/// earliest tile that is still allowed never costs a later one — any solution that skips an available
/// tile can be rewritten to start at it without moving the rest closer together. Hence no
/// combinatorics, no allocation, one pass.
fn independent_support(tiles: impl Iterator<Item = u32>) -> u32 {
    let mut count = 0;
    let mut next_allowed = 0;
    for index in tiles {
        if index >= next_allowed {
            count += 1;
            next_allowed = index + TILE_INDEPENDENCE_GAP;
        }
    }
    count
}

/// How many independent 32 px tiles of the band agree with the shift.
///
/// Only whole tiles count: a partially filled tile correlates over fewer cells, and comparing it to
/// the same threshold would make the tail of every band systematically weaker evidence. The
/// independence rule (§16.4's `|i − j| ≥ 2`) is applied here rather than reported separately, because
/// every consumer — the ranking's `tiles`, `Evidence`'s `coverage`, gate three — needs the same
/// number, and §16.7 defines `coverage` on independent supporters.
fn supporting_tiles(previous: &Gray, current: &Gray, band: MatchBand) -> u32 {
    let tile = TILE_ROWS / DOWNSAMPLE;
    let tile_count = band.rows / tile;
    independent_support((0..tile_count).filter(|start| {
        let first = start * tile;
        band_zncc(previous, current, band, first, tile) >= TILE_SUPPORT_ZNCC
    }))
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
    /// How many **independent** 32 px tiles agree (§16.4's band count, after that section's
    /// `|i − j| ≥ 2` rule). §16.7's `coverage` is defined on this number, so it is the independent
    /// count here and not the raw tile count.
    pub(crate) tiles: u32,
    /// The three-point difference of `zncc2d` around the candidate (§15.4 ②).
    pub(crate) curvature: f32,
    /// §16.7's combination, the key this set is ranked by.
    pub(crate) score: f32,
    /// Layer 1's support for this shift, carried unchanged from [`Candidate`].
    ///
    /// Every candidate inside one 4 px cell is measured at the cell's own grid point, so their
    /// `zncc2d`, `gain`, `tiles` and `curvature` are identical — this layer *cannot* separate them
    /// and must not pretend to. Layer 1 can: its row-digest support is computed per shift, so the
    /// member that actually aligns the page has more of it. The tie-break below uses that evidence
    /// instead of a grid heuristic, which is what keeps the winner inside layer 3's ±1
    /// neighbourhood (`P1.07`'s sweep: without it every shift ≡ 2 (mod 4) came back 1 px off).
    pub(crate) support: u32,
}

/// Up to [`CANDIDATE_LIMIT`] scored candidates, best first.
///
/// Same fixed-array shape and same reason as [`CandidateSet`], and the same determinism
/// requirement: ranked by `(score desc, curvature asc, support desc, |d| asc, d asc)`.
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
                support: 0,
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
/// tells a peak from a plateau, so it breaks an exact tie in `score` (sharper first).
///
/// Then comes layer 1's own support: an exact `score` tie between two candidates means this layer
/// measured them identically — which is the normal case **inside one 4 px cell**, where every member
/// is measured at the same grid point — and the only evidence that still separates them is the
/// one-dimensional support layer 1 computed per shift. Only then, if even that ties, does the
/// distance to the grid point this layer measured at break the tie (the candidate closest to
/// `round_to_grid(d)·4` is the honest thing to report when nothing else speaks), and the shift
/// tie-break last makes the order total and deterministic — the same requirement `P1.05`'s
/// [`CandidateSet`] answers for layer 1.
fn scored_ranks_before(candidate: ScoredCandidate, existing: ScoredCandidate) -> bool {
    match candidate.score.total_cmp(&existing.score) {
        core::cmp::Ordering::Greater => true,
        core::cmp::Ordering::Less => false,
        core::cmp::Ordering::Equal => match candidate.curvature.total_cmp(&existing.curvature) {
            core::cmp::Ordering::Less => true,
            core::cmp::Ordering::Greater => false,
            core::cmp::Ordering::Equal => match candidate.support.cmp(&existing.support) {
                core::cmp::Ordering::Greater => true,
                core::cmp::Ordering::Less => false,
                core::cmp::Ordering::Equal => {
                    (grid_distance(candidate.d), candidate.d.unsigned_abs(), candidate.d)
                        < (grid_distance(existing.d), existing.d.unsigned_abs(), existing.d)
                }
            },
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
    previous_gray: &Gray,
    current_gray: &Gray,
    candidates: &CandidateSet,
) -> ScoredSet {
    let wanted = match_rows(previous_gray.height, current_gray.height, DOWNSAMPLE);
    // The zero shift's band is the denominator of every candidate's `gain`, and it is the same band
    // for all of them: built once, outside the loop, so the "same region on both sides" property of
    // §16.3 is a fact of the code rather than of the reader's attention (`docs/31` `P1.09` REFACTOR).
    let zero_band = match_band(previous_gray.height, current_gray.height, 0, wanted)
        .expect("the zero shift always overlaps a non-empty frame");
    let rmse_at_zero = band_rmse(previous_gray, current_gray, zero_band);
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
        let zncc2d = band_zncc(previous_gray, current_gray, band, 0, band.rows);
        let tiles = supporting_tiles(previous_gray, current_gray, band);
        let gain = residual_gain(band_rmse(previous_gray, current_gray, band), rmse_at_zero)
            .unwrap_or(GAIN_UNDEFINED_FOR_RANKING);
        let curvature = curvature_of(previous_gray, current_gray, band);
        scored.insert(ScoredCandidate {
            d: candidate.d,
            zncc2d,
            gain,
            tiles,
            curvature,
            score: score_of(zncc2d, gain, coverage_of(tiles)),
            support: candidate.support,
        });
    }
    scored
}

// --- layer 3 (`P1.07`) --------------------------------------------------------------------------

/// §15.4 ③'s refinement neighbourhood: the winner and its two integer neighbours, in the order the
/// tie-break prefers them. N3 is why there is no fourth entry and no fraction: the evidence is a
/// pixel grid, so a subpixel offset would be interpolation wearing the clothes of a measurement.
pub(crate) const REFINE_NEIGHBOURHOOD: [i32; 3] = [-1, 0, 1];

/// What the funnel's last layer hands on: one integer shift and the full-resolution correlation
/// that won it.
///
/// There is deliberately no field for a score, a status, a curvature or a subpixel offset.
/// Assembling evidence (`P1.10`) and deciding (`P1.11`) happen *above* this layer, and the single
/// number the canvas will act on is `d`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Refined {
    /// The shift, in whole pixels of the frame's primary axis.
    pub(crate) d: i32,
    /// §16.7's `zncc2d` of that shift at full resolution — §15.4 ③'s "the only number that reaches
    /// the gates".
    pub(crate) zncc2d: f32,
}

/// Does `shift` with `zncc2d` displace the incumbent? Ties go to the shift nearer the winner, and
/// an exact tie keeps the one already held, so the three-element iteration order above is the
/// complete tie-break (`P1.05`'s lesson: an order that is not total is not deterministic).
fn refine_ranks_before(shift: i32, zncc2d: f32, winner: i32, incumbent: Refined) -> bool {
    match zncc2d.total_cmp(&incumbent.zncc2d) {
        core::cmp::Ordering::Greater => true,
        core::cmp::Ordering::Less => false,
        core::cmp::Ordering::Equal => shift.abs_diff(winner) < incumbent.d.abs_diff(winner),
    }
}

/// §15.4 ③'s third layer: full resolution, integer, and only `±1` around layer 2's winner.
///
/// Layer 2 measures on a [`DOWNSAMPLE`]-pixel grid, so every candidate inside one cell is *one*
/// measurement and the layer reports the grid point it measured at. This layer re-measures that
/// winner's own neighbourhood at full resolution and keeps whichever shift correlates best, which
/// is how a 119 px step stops being reported as the 120 px grid point it was rounded to, and what
/// `P1.06`'s `shifts_inside_one_cell_share_one_measurement` said was deliberately left to here.
///
/// It is also the last layer: above it, evidence is assembled and gates decide, but no further
/// measurement is possible.
///
/// `None` when layer 2 scored nothing: there is no winner to refine, and that is a state rather
/// than a zero shift (`P1.04`).
pub(crate) fn refine_winner(
    previous_gray: &Gray,
    current_gray: &Gray,
    scored: &ScoredSet,
) -> Option<Refined> {
    let winner = scored.iter().next()?.d;
    let wanted = match_rows(previous_gray.height, current_gray.height, 1);
    let mut refined: Option<Refined> = None;
    for step in REFINE_NEIGHBOURHOOD {
        let shift = winner + step;
        // A neighbour can leave the frame entirely (a shift equal to the extent does not hit this
        // layer, but a one-row frame can): the other two still get their say.
        let Some(band) = match_band(previous_gray.height, current_gray.height, shift, wanted) else {
            continue;
        };
        let zncc2d = band_zncc(previous_gray, current_gray, band, 0, band.rows);
        let better = match refined {
            None => true,
            Some(incumbent) => refine_ranks_before(shift, zncc2d, winner, incumbent),
        };
        if better {
            refined = Some(Refined { d: shift, zncc2d });
        }
    }
    refined
}

// ── the gates (`P1.08`) ──────────────────────────────────────────────────────────────────────────
//
// §16.1 runs four gates over the candidate set and the candidates that survive all of them are what
// `Confirmed` means. They answer uniformly — a gate decides, it does not measure — so the assembly
// (`P1.12`) can hold them in one place and read one type.

/// What a gate said (`docs/30` §16.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateOutcome {
    Pass,
    Reject(GateRejection),
}

/// Why a gate refused. One variant per gate keeps a diagnosis from collapsing into `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateRejection {
    /// §16.2: `|d| > viewport_extent`. The two frames share no row, so there is nothing to measure.
    OutsideViewport,
    /// §16.9: `|d| == extent / 2`. The landing point of every undefined and every wraparound path.
    BannedHalf,
    /// §16.3: `gain < MIN_RESIDUAL_GAIN`. The shift explains no more of the residual than standing
    /// still did, so the peak is a coincidence of the content rather than a movement.
    ResidualGainTooSmall,
    /// §16.4: fewer than [`MIN_TILES`] independent tiles support the shift. It may still be the true
    /// shift — but it is corroborated in one place only, and one place is what a periodic page also
    /// produces.
    TooFewSupporters,
    /// §16.5: `margin < MIN_MARGIN`. A rival came within 15% of the winner, so "the best candidate"
    /// is not a choice the evidence makes — the classic case being a periodic page, where the rival is
    /// the same content a period away. Carries the winner because this is the one rejection that
    /// reports a *measured* shift (`Status::Uncertain`), not an absent measurement.
    MarginTooSmall { best: i32 },
}

impl GateRejection {
    /// What §16.10 says a rejection means for the session.
    ///
    /// The first four are `None` rather than `Uncertain`. `Uncertain` means "a shift was measured and
    /// the evidence does not carry a decision"; those mean the measurement is not a measurement —
    /// there was nothing to compare, the estimator left its defined domain, the best candidate gained
    /// nothing over not moving, or nothing independent corroborates it. §16.9 wants that distinction
    /// visible, so it is a method instead of a sentence each call site would spell differently.
    ///
    /// [`Self::MarginTooSmall`] is the exception, and the reason the method exists rather than a
    /// constant: the winner *was* measured, twice, and the two measurements disagree about which
    /// shift they describe. §16.10's row for that is `Uncertain` — the session continues and the
    /// canvas does not move — which is exactly why the variant carries its `best`.
    pub(crate) const fn status(self) -> Status {
        match self {
            Self::OutsideViewport
            | Self::BannedHalf
            | Self::ResidualGainTooSmall
            | Self::TooFewSupporters => Status::None,
            Self::MarginTooSmall { best } => Status::Uncertain { d: best },
        }
    }
}

/// §16.2's geometry gate: the closed interval `|d| <= viewport_extent`, plus §16.9's ban on `±N/2`.
///
/// The interval is closed because it is a fact, not a preference: at `|d| == extent` exactly one row
/// of the previous observation is still on screen (a measurable overlap), at `extent + 1` none is.
/// It takes the viewport extent and nothing else — no search window, no prior, no content — because
/// §16.2 wants it to hold even when §16.6's `ĝ` hands the search a nonsensical centre.
///
/// The ban is applied after the interval, and it is deliberately the *truncated* half (`11 / 2 == 5`)
/// rather than "an odd extent has no half": the value being banned is a specific fallback landing
/// point, and §16.9 requires it be named rather than silently skipped. A viewport with no half at
/// all (`extent <= 1`) must still be able to report `d == 0`, which is the one answer §16.3's
/// fingerprint path exists to confirm.
pub(crate) fn gate_geometry(d: i32, viewport_extent: u32) -> GateOutcome {
    // `i32::MIN` is the shift no session produces but a caller can pass; widening first makes the
    // comparison total instead of a panic in a gate that is supposed to be the last line of defence.
    let shift = i64::from(d);
    let extent = i64::from(viewport_extent);

    if shift.abs() > extent {
        return GateOutcome::Reject(GateRejection::OutsideViewport);
    }

    let half = extent / 2;
    if extent >= 2 && shift.abs() == half {
        return GateOutcome::Reject(GateRejection::BannedHalf);
    }

    GateOutcome::Pass
}

/// The smallest remaining overlap that still counts as evidence (`docs/30` §16.1, §14.2's `ρ*`).
///
/// Deliberately **not** the same item as [`gate_geometry`]: the gate is a fact about overlap with no
/// parameter, this is a preference about how much evidence suffices, and §16.11 marks it
/// calibratable (`E-ACC-1`, range 0.30–0.40).
pub(crate) const RHO_MIN: f32 = 0.35;

/// [`RHO_MIN`] in permille, which is what [`is_verifiable`] actually compares.
///
/// The floor is an integer comparison on purpose: `extent = 100, |d| = 65` is exactly 350‰ and must
/// pass, and a float `>=` on the rounded quotient is the kind of boundary a calibration run would
/// move by accident.
pub(crate) const RHO_MIN_PERMILLE: i64 = 350;

/// §16.1's verifiability floor: `overlap_ratio >= RHO_MIN` with `overlap_ratio = (extent − |d|) / extent`.
///
/// This is the constraint §16.2's note calls "credibility", and it is the reason a `d` can pass the
/// hard gate and still be reported as `Uncertain` (§16.10).
pub(crate) fn is_verifiable(d: i32, viewport_extent: u32) -> bool {
    let extent = i64::from(viewport_extent);
    if extent <= 0 {
        // No viewport, no evidence: refuse to call a degenerate observation verifiable.
        return false;
    }
    let overlap = (extent - i64::from(d).abs()).max(0);
    overlap * 1000 >= RHO_MIN_PERMILLE * extent
}

/// The smallest residual gain that counts as movement (`docs/30` §16.3).
///
/// The magnitude comes from the reference implementation (§6 N5, R16) and is a startup value:
/// §16.11 marks it calibratable and `E-ACC-1` owns the number. What is *not* calibratable is that
/// this gate exists — F-03's whole point is that "a peak was found" and "the peak beats standing
/// still" are different claims, and only the second one is about movement.
pub(crate) const MIN_RESIDUAL_GAIN: f32 = 0.15;

/// §16.3's gate two, the only gate that must pass: `gain >= 0.15`, closed at the floor.
///
/// It takes the measured `gain` rather than the frames, because [the measurement](residual_gain) and
/// the decision are separate: `d_best == 0` produces **no** `gain` at all, and a gate that could be
/// called with `None` would have to invent a policy for it here. §16.3's answer lives in
/// [`zero_shift_status`] instead, so this function never sees that case.
pub(crate) fn gate_residual_gain(gain: f32) -> GateOutcome {
    if gain >= MIN_RESIDUAL_GAIN {
        GateOutcome::Pass
    } else {
        GateOutcome::Reject(GateRejection::ResidualGainTooSmall)
    }
}

/// How many **independent** tiles must support a shift before it counts as spatially corroborated
/// (`docs/30` §16.4).
///
/// F-02 is the reason the number is not `1`: a single patch agreeing is what a periodic page, a
/// repeated row, or one high-contrast widget produces — the shift is right there and wrong
/// everywhere else. Four is a startup value from the reference implementation (§6 N5) and §16.11
/// marks it calibratable; `E-ACC-1` owns the number, and the ablation matrix (`P1.13`) is what
/// decides whether the gate earns its place at all.
pub(crate) const MIN_TILES: u32 = 4;

/// §16.4's gate three: `supporters >= MIN_TILES`, closed at the floor.
///
/// It takes the count that [`supporting_tiles`] produced, so "how many tiles agree" and "how much
/// spatial corroboration is enough" are separable. A page with no structure yields `0` for every
/// candidate, which is §30.3's 低纹理 row in its estimator half: the shift is not corroborated
/// anywhere, so the candidate is dropped (§16.1) and a session that drops all of them reports `None`
/// rather than a confident zero.
pub(crate) fn gate_support(supporters: u32) -> GateOutcome {
    if supporters >= MIN_TILES {
        GateOutcome::Pass
    } else {
        GateOutcome::Reject(GateRejection::TooFewSupporters)
    }
}

/// The smallest separation between the winner and its runner-up that counts as a decision
/// (`docs/30` §16.5).
///
/// F-03 is the reason there is a number at all: on a periodic page every alias of the true shift
/// scores the same, so "the best candidate" is an artefact of `minMaxLoc` rather than a measurement.
/// The magnitude is a startup value taken from the reference implementation (§6 N5, R16); §16.11
/// marks it calibratable and `E-ACC-1` owns it. What is not calibratable is that the gate exists —
/// N2 asks for "不确定 rather than a confident period multiple", and this is the only gate that can
/// say it.
pub(crate) const MIN_MARGIN: f32 = 0.15;

/// The score scale below which §16.5's ratio has no meaning: `0 / 0` is not a margin.
///
/// It is deliberately *not* `0.0`. Two candidates with no evidence at all score exactly zero
/// everywhere, and `(0 − 0) / 0` would be `NaN` — a value that poisons every comparison it reaches
/// (§2.1 F-02's `0/0 → NaN` failure, in its smallest form). The floor turns that into the answer the
/// type already has: no ratio.
const MARGIN_SCORE_FLOOR: f32 = 1e-3;

/// §16.5's ratio, in one place: `(score(best) − score(second)) / score(best)`.
///
/// Both arguments are §16.7's **composed** score, never a bare `zncc2d`: §16.5 says so explicitly,
/// because "one candidate leads by 0.02 in ZNCC but by 0.4 in residual gain" would otherwise be
/// reported as ambiguous.
///
/// `None` means there is no ratio to compute, and both causes are the same absence:
/// [`second`] missing (a lone candidate has nothing to be confused with) or the winner's own score
/// having no scale to divide by. The second cause cannot arise through the pipeline — passing gate
/// two means `score >= SCORE_GAIN · MIN_RESIDUAL_GAIN = 0.0375`, an order of magnitude above
/// [`MARGIN_SCORE_FLOOR`] — and [`gate_margin`] says what the answer is when it appears anyway.
fn margin_of(best: f32, second: Option<f32>) -> Option<f32> {
    if best <= MARGIN_SCORE_FLOOR {
        return None;
    }
    second.map(|second| (best - second) / best)
}

/// §16.5's gate four: `margin >= MIN_MARGIN`, closed at the floor, plus what to do without a margin.
///
/// It takes the winner's shift as well as the margin because this rejection has to report *which*
/// shift the evidence could not choose: §16.10's `Uncertain` carries a `d`, and the only place that
/// knows it is the candidate the ranking picked.
///
/// A missing margin ([`None`]) passes. That is not a loophole: gate four exists to reject a winner
/// that has a *rival*, and with a single candidate there is nothing to reject it in favour of.
/// Inventing `margin = 1.0` for that case — the other way to write it — would feed a fabricated
/// confidence term to §16.7, and every session's candidate set is the whole search window, so the
/// case is degenerate rather than common.
pub(crate) fn gate_margin(best: i32, margin: Option<f32>) -> GateOutcome {
    match margin {
        Some(margin) if margin >= MIN_MARGIN => GateOutcome::Pass,
        Some(_) => GateOutcome::Reject(GateRejection::MarginTooSmall { best }),
        None => GateOutcome::Pass,
    }
}

/// §16.3's ratio measured on two frames at a given shift, in one place.
///
/// `shift` arrives in full-resolution primary-axis pixels and the measurement is taken on layer 2's
/// grid, because that is where the ranking takes it: §16.7 documents `Evidence.gain` as the second
/// layer's number, so gate two must not read a differently rounded region than the score did (that
/// mismatch is exactly what this function exists to prevent). A caller asking about one shift gets
/// the same *definition of the region* (`match_band` on both sides) as [`score_candidates_2d`].
///
/// `None` means the ratio is undefined — the frames are identical at zero shift — and it is the
/// signal to take the fingerprint path.
pub(crate) fn residual_gain_at(
    previous_gray: &Gray,
    current_gray: &Gray,
    shift: i32,
) -> Option<f32> {
    let wanted = match_rows(previous_gray.height, current_gray.height, DOWNSAMPLE);
    let zero_band = match_band(previous_gray.height, current_gray.height, 0, wanted)
        .expect("the zero shift always overlaps a non-empty frame");
    let shift_band = match_band(
        previous_gray.height,
        current_gray.height,
        round_to_grid(shift),
        wanted,
    )?;
    residual_gain(
        band_rmse(previous_gray, current_gray, shift_band),
        band_rmse(previous_gray, current_gray, zero_band),
    )
}

/// §16.3's duplicate detection: what a zero shift means when gate two cannot be called.
///
/// `Confirmed { d: 0 }` only if every primary line of the two frames is byte-identical; otherwise
/// `None`, because the frames differ and this path measured nothing. The comparison is deliberately
/// the byte-level digest and not a correlation: a relit but unmoved page is reported as "changed",
/// which costs a frame (no canvas row is committed) instead of risking a duplicated one — the same
/// conservative direction §16.5's margin errs in.
pub(crate) fn zero_shift_status(
    previous: &ObservationView<'_>,
    current: &ObservationView<'_>,
) -> Status {
    if previous.size() != current.size() || previous.axis() != current.axis() {
        return Status::None;
    }
    if primary_digests(previous) == primary_digests(current) {
        Status::Confirmed { d: 0 }
    } else {
        Status::None
    }
}

/// The variance of one side of a tile, in the same units [`band_zncc`] uses before it takes a square
/// root: `count·Σx² − (Σx)²`. Positive iff the side has structure.
fn band_variance(gray: &Gray, first: u32, first_row: u32, rows: u32) -> f64 {
    let count = (gray.width * rows) as f64;
    let (mut sum, mut sum_sq) = (0.0f64, 0.0f64);
    for row in 0..rows {
        for column in 0..gray.width {
            let value = gray.at(column, first + first_row + row) as f64;
            sum += value;
            sum_sq += value * value;
        }
    }
    count * sum_sq - sum * sum
}

/// Whether a tile says anything about alignment — that is, whether either side of it has structure.
///
/// This is not an optimisation. [`band_zncc`] reports `0.0` for a band whose variance is zero, and
/// `1 − 0.0` is the *worst possible* alignment error, so a tile of a solid white page would
/// otherwise be the strongest evidence that the page changed. The reference implementation draws the
/// same line with a calibrated texture floor (`estimator.rs:543`: `texture < 0.05`); ours asks the
/// sharper, threshold-free question "is the variance exactly zero", which is the same distinction
/// §16.3's `gain` makes between "undefined" and "measured as no gain".
///
/// The test is "either side" rather than "both": a tile that is flat in one frame and structured in
/// the other has genuinely failed to align, and that is worth counting.
fn tile_carries_evidence(
    previous: &Gray,
    current: &Gray,
    band: MatchBand,
    first_row: u32,
    rows: u32,
) -> bool {
    band_variance(previous, band.previous_first, first_row, rows) > 0.0
        || band_variance(current, band.current_first, first_row, rows) > 0.0
}

/// §16.8's trimmed mean: the average of the lowest `retained` share of the tile errors, `None` when
/// there is nothing to average.
///
/// `None` is the load-bearing part. The reference implementation returns `(1.0, 1.0)` when it
/// collected no samples (`estimator.rs:570`) — the worst possible error — and on a page with no
/// structure at all that answer is read as "the page changed". Absence of evidence is not a scene
/// cut here: unmeasurable tiles produce no samples, and the caller treats "nothing to average" as a
/// veto rather than as a maximum (DEV-21).
fn alignment_error_of(samples: &mut Vec<f32>, retained: f32) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(f32::total_cmp);
    let keep = ((samples.len() as f32 * retained).ceil() as usize).clamp(1, samples.len());
    Some(samples[..keep].iter().sum::<f32>() / keep as f32)
}

/// §16.8's `alignment_error(d)`: how badly the frames fail to align at `shift`, on layer 2's grid.
///
/// Layer 2's resolution is the right one for the question — this is a *binary* decision ("does any
/// candidate align anywhere"), not the number that reaches a `displacement`, which §15.4 ③ reserves
/// for layer 3 — and it is the same grid the candidates were scored on, so a candidate that looked
/// good to the ranking is judged in the units it won with.
///
/// This entry point pools the two frames itself; [`alignment_error_at`] is the form that takes the
/// images, which is what [`is_scene_cut`] uses so that asking about *every* candidate pools once.
pub(crate) fn alignment_error(
    previous: &ObservationView<'_>,
    current: &ObservationView<'_>,
    shift: i32,
) -> Option<f32> {
    alignment_error_at(
        &Gray::pooled(previous),
        &Gray::pooled(current),
        shift,
    )
}

/// [`alignment_error`] on two already-pooled images.
fn alignment_error_at(previous: &Gray, current: &Gray, shift: i32) -> Option<f32> {
    let wanted = match_rows(previous.height, current.height, DOWNSAMPLE);
    let band = match_band(
        previous.height,
        current.height,
        round_to_grid(shift),
        wanted,
    )?;
    let tile = TILE_ROWS / DOWNSAMPLE;
    let mut samples = Vec::new();
    let mut first = 0;
    while first + tile <= band.rows {
        if tile_carries_evidence(previous, current, band, first, tile) {
            samples.push(1.0 - band_zncc(previous, current, band, first, tile));
        }
        first += tile;
    }
    alignment_error_of(&mut samples, ALIGNMENT_RETAINED)
}

/// §16.8's first condition: `zncc2d(d_0 = 0)`, measured directly rather than looked up.
///
/// It is computed here instead of being read off the candidate set because the zero shift is not
/// guaranteed to be a candidate: the search window is built around the prior's expectation
/// (`P1.14`), and "did the frames change at rest" is a question about the frames, not about what the
/// prior happened to propose. `0.0` for frames with no overlap and for a structureless band is the
/// same answer `band_zncc` gives — "nothing correlates" — and §16.8's floor turns both into "they
/// differ", which the second condition then gets to refute.
pub(crate) fn zero_shift_similarity(
    previous: &ObservationView<'_>,
    current: &ObservationView<'_>,
) -> f32 {
    zero_shift_similarity_at(&Gray::pooled(previous), &Gray::pooled(current))
}

/// [`zero_shift_similarity`] on two already-pooled images.
fn zero_shift_similarity_at(previous: &Gray, current: &Gray) -> f32 {
    let wanted = match_rows(previous.height, current.height, DOWNSAMPLE);
    match match_band(previous.height, current.height, 0, wanted) {
        Some(band) => band_zncc(previous, current, band, 0, band.rows),
        None => 0.0,
    }
}

/// §16.8's scene cut: the frames differ at zero shift **and** no candidate aligns.
///
/// The two conditions answer different questions and are asked cheapest-first — one band at zero
/// shift, then one tile sweep per candidate — which is also why the reference implementation only
/// evaluates it in the branch where the winner was already rejected (`estimator.rs:1285`). We do not
/// need that coupling: the first condition alone is enough to rule out a frame whose content matched.
///
/// The two frames are pooled **once** per step, by the caller's [`Scratch`], rather than by each
/// condition: pooling is `O(W·H/16)` and the second condition would otherwise run it per candidate,
/// which would make the question cost more than the whole second layer it is asking about. Gate two,
/// the second layer and the scene cut share that one pass, and layer 3's full-resolution images are a
/// second one — four pools per step, not the eight the three call sites used to do.
///
/// Two ways to be true by accident are closed here explicitly:
///
/// - **An empty candidate set.** `∀ i` over no candidates is vacuously true, and a frame whose
///   candidates were all dropped is the *absence* of evidence — §16.10 spells that `None`.
/// - **An unmeasurable candidate.** A candidate whose band has no structure at all yields no
///   samples, and "cannot be measured" must not be read as "misaligned", because that is how a blank
///   page or a page with one flat band would be declared a scene cut.
pub(crate) fn is_scene_cut(
    previous_gray: &Gray,
    current_gray: &Gray,
    scored: &ScoredSet,
) -> bool {
    if scored.iter().next().is_none() {
        return false;
    }
    if zero_shift_similarity_at(previous_gray, current_gray) >= SCENE_CUT_SIMILARITY {
        return false;
    }
    scored.iter().all(|candidate| {
        match alignment_error_at(previous_gray, current_gray, candidate.d) {
            Some(error) => error > SCENE_CUT_ALIGNMENT_ERROR,
            None => false,
        }
    })
}

/// §16.8's `scene_cut_streak`: how many consecutive frames failed to align anywhere.
///
/// Deliberately not a field of [`Status`]. A changed page is a fact about the page, and the session's
/// own question ("did anything move?") has already been answered — `Uncertain`, since a winner was
/// scored — so this type exists to tell the *model* whether to keep trusting its tiles, not to give
/// the session a reason to end. `docs/30` §16.7 puts it on [`Evidence`] for the same reason, and
/// §20.4's rule that `MatchFailed` is not a `StopReason` is the same decision on a different type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SceneCut {
    consecutive: u8,
}

impl SceneCut {
    /// No scene cut has been seen yet — the state a session starts in.
    pub(crate) const fn none() -> Self {
        Self { consecutive: 0 }
    }

    pub(crate) const fn consecutive(&self) -> u8 {
        self.consecutive
    }

    /// The streak's whole transition rule: a scene cut extends it, and any other frame ends it.
    ///
    /// Saturating rather than wrapping: the count is compared against a small threshold, so a page
    /// that never comes back must be a large number and not (after 256 frames) a fresh streak. The
    /// reset is unconditional — "consecutive" is what §16.8 asks about, so one aligned frame is
    /// enough, however long the run was.
    pub(crate) const fn observe(self, detected: bool) -> Self {
        if detected {
            Self {
                consecutive: self.consecutive.saturating_add(1),
            }
        } else {
            Self::none()
        }
    }

    /// What to do about it, in the only two shapes this fact has (§16.8's table).
    pub(crate) const fn action(&self) -> SceneCutAction {
        if self.consecutive >= SCENE_CUT_DECAY_STREAK {
            SceneCutAction::DecayModel
        } else {
            SceneCutAction::Tolerate
        }
    }
}

/// The two responses §16.8 allows to a scene cut. There is no third.
///
/// `DecayModel` is a request, not a number: §16.8's `decay_toward_neutral(0.05)` and the model reset
/// are the tile model's own business (§18.2), because the rate is a property of how much a tile's
/// history is trusted — this module does not own the model and so does not own its decay constant.
/// What this module owns is the *decision to decay*, which is what the streak is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SceneCutAction {
    /// The frame was not used and the session carries on as if it had not happened (1–2 in a row).
    Tolerate,
    /// Three in a row: the tiles we are matching against describe a page that is gone.
    DecayModel,
}

#[cfg(test)]
mod tests {
    use super::{
        CANDIDATE_LIMIT, Candidate, CandidateSet, Displacement, Evidence, GateOutcome, GateRejection,
        Gray,
        MARGIN_SCORE_FLOOR, MIN_MARGIN, MIN_RESIDUAL_GAIN, MIN_TILES, RHO_MIN, RHO_MIN_PERMILLE,
        SCENE_CUT_ALIGNMENT_ERROR, SCENE_CUT_DECAY_STREAK, SCENE_CUT_SIMILARITY, SCORE_GAIN, Scratch,
        SceneCut, SceneCutAction, ScoredCandidate, ScoredSet, Status, StepEffect,
        TILE_INDEPENDENCE_GAP, alignment_error, band_zncc, candidates_1d, gate_geometry, gate_margin,
        gate_residual_gain, gate_support, independent_support, is_scene_cut, is_verifiable, margin_of,
        match_band, match_rows, primary_digests, residual_gain, residual_gain_at, score_candidates_2d,
        support_at, zero_shift_similarity, zero_shift_status,
    };
    use crate::scroll::observation::{Axis, Observation, ObservationView};
    use crate::scroll::displacement::{
        DOWNSAMPLE, REFINE_NEIGHBOURHOOD, Refined, TILE_ROWS, TILE_SUPPORT_ZNCC, coverage_of,
        grid_distance, refine_winner, round_to_grid,
    };
    use crate::scroll::testkit::{ScrollScript, StepSpec, Structure, TestImage};

    fn evidence() -> Evidence {
        Evidence {
            zncc2d: 0.90,
            gain: 0.40,
            margin: 0.30,
            tiles: 6,
            scene_cut: SceneCut::none(),
        }
    }

    // The layers take images that a caller has already pooled, because a step pools each scale once
    // and hands the result to every layer that wants it. A test that asks a single question is
    // allowed to build its own `Scratch` for that question, and these four wrappers keep the call
    // sites about the question rather than about the buffer.
    fn scored_once(
        previous: &Observation,
        current: &Observation,
        candidates: &CandidateSet,
    ) -> ScoredSet {
        let mut scratch = Scratch::new();
        let views = scratch.pool(&previous.view(), &current.view());
        score_candidates_2d(views.previous(), views.current(), candidates)
    }

    fn refined_once(
        previous: &Observation,
        current: &Observation,
        scored: &ScoredSet,
    ) -> Option<Refined> {
        let mut scratch = Scratch::new();
        let views = scratch.full_resolution(&previous.view(), &current.view());
        refine_winner(views.previous(), views.current(), scored)
    }

    fn gain_once(previous: &Observation, current: &Observation, shift: i32) -> Option<f32> {
        let mut scratch = Scratch::new();
        let views = scratch.pool(&previous.view(), &current.view());
        residual_gain_at(views.previous(), views.current(), shift)
    }

    fn is_scene_cut_once(previous: &Observation, current: &Observation, scored: &ScoredSet) -> bool {
        let mut scratch = Scratch::new();
        let views = scratch.pool(&previous.view(), &current.view());
        is_scene_cut(views.previous(), views.current(), scored)
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
            scene_cut: SceneCut::none(),
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
            scene_cut: SceneCut::none(),
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

        let baseline = scored_once(&previous, &current, &candidates);
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
            let scored = scored_once(&previous, &relit, &candidates);
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
        let scored = scored_once(&previous, &current, &candidates);
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
                support,
            } = *candidate;
            let _ = support;
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

        let scored = scored_once(&previous, &current, &candidates);
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
    /// correlations, equal gain, equal tiles — so this layer cannot rank them and must not pretend
    /// it can. Separating them is §15.4 ③'s job at full resolution (`P1.07`), which refines ±1
    /// around the winner; the winner therefore has to be the member the page aligns with, and that
    /// is decided by layer 1's support (`P1.07` measured why: on the grid heuristic every shift
    /// ≡ 2 (mod 4) came back 1 px off).
    #[test]
    fn shifts_inside_one_cell_share_one_measurement() {
        let image = mixed_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);
        let candidates = candidates_1d(&previous.view(), &current.view(), 120, 40);
        let scored = scored_once(&previous, &current, &candidates);

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
        // Support is *not* part of that shared measurement: it is layer 1's, computed per shift, and
        // it is the only thing in this set that can tell one member of the cell from another. That
        // is what `P1.07` needs — a ±1 refinement around the winner can only reach the truth if the
        // winner is already the member the page actually aligns with.
        assert!(
            members.iter().any(|member| member.support != members[0].support),
            "the fixture no longer gives layer 1 anything to separate the cell with: {:?}",
            members
                .iter()
                .map(|member| (member.d, member.support))
                .collect::<Vec<_>>()
        );
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
        let scored = scored_once(&previous, &current, &candidates);
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

    /// Full-resolution correlation of the band at `shift`, recomputed here instead of read off
    /// [`Refined`], so the third layer is checked against §15.4 ③'s definition rather than against
    /// itself. Shares only the module's primitives ([`Gray`], [`match_band`], [`band_zncc`]).
    fn full_resolution_zncc(
        previous: &ObservationView<'_>,
        current: &ObservationView<'_>,
        shift: i32,
    ) -> f32 {
        let previous_gray = Gray::scaled(previous, 1);
        let current_gray = Gray::scaled(current, 1);
        let wanted = match_rows(previous_gray.height, current_gray.height, 1);
        let Some(band) = match_band(
            previous_gray.height,
            current_gray.height,
            shift,
            wanted,
        ) else {
            return f32::NEG_INFINITY;
        };
        band_zncc(&previous_gray, &current_gray, band, 0, band.rows)
    }

    /// `docs/31` §6 `P1.07`'s first RED case.
    #[test]
    fn refinement_only_moves_the_winner_by_one_pixel() {
        // Layer 2 measures on its own 4 px grid, so a shift of 119 and a shift of 120 are the *same*
        // measurement and it reports the grid point it actually measured at (`P1.06`'s
        // `shifts_inside_one_cell_share_one_measurement`). §15.4 ③'s third layer is what hands the
        // truth back: full resolution, integer, and never further than ±1 from what layer 2
        // proposed. That neighbourhood is the whole of its authority — F-04/N3: the observation is
        // an integer pixel grid, so anything finer would be invented rather than observed.
        let image = mixed_document();
        for truth in [119, 37] {
            let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(truth)]);
            let previous = script.take(0);
            let current = script.take(1);
            let candidates = candidates_1d(&previous.view(), &current.view(), truth, 8);
            let scored = scored_once(&previous, &current, &candidates);
            let winner = scored
                .iter()
                .next()
                .expect("the second layer scored nothing on the mixed document")
                .d;
            let refined = refined_once(&previous, &current, &scored)
                .expect("layer 2 proposed a shift, so layer 3 has something to refine");

            assert!(
                refined.d.abs_diff(winner) <= 1,
                "truth {truth}: the third layer moved the winner from {winner} to {}, but its \
                 neighbourhood is ±1 — a wider move is a second search, not a refinement",
                refined.d
            );

            // The point of the layer: it separates what the grid could not. 119 and 37 both round
            // to a cell whose grid point is 1 px away from them, so layer 2 can only say 120 and
            // 36 — the third layer is the first thing in the funnel allowed to say the truth.
            assert_eq!(
                refined.d, truth,
                "truth {truth}: layer 2 said {winner} and layer 3 said {}, so the ±1 neighbourhood \
                 did not recover the shift the grid flattened",
                refined.d
            );

            // And the shift it reports really is the best full-resolution correlation inside that
            // neighbourhood — recomputed independently, then read off `Refined`.
            let best_inside = REFINE_NEIGHBOURHOOD
                .iter()
                .map(|step| winner + step)
                .max_by(|left, right| {
                    full_resolution_zncc(&previous.view(), &current.view(), *left)
                        .total_cmp(&full_resolution_zncc(&previous.view(), &current.view(), *right))
                })
                .expect("the neighbourhood is not empty");
            assert_eq!(
                refined.d, best_inside,
                "truth {truth}: the third layer did not take the argmax of the full-resolution \
                 neighbourhood"
            );
            assert!(
                (refined.zncc2d - full_resolution_zncc(&previous.view(), &current.view(), truth))
                    .abs()
                    < 1e-6,
                "truth {truth}: the reported correlation {} is not the one this shift has",
                refined.zncc2d
            );
        }
    }

    /// `docs/31` §6 `P1.07`'s second RED case.
    #[test]
    fn the_final_value_is_an_integer() {
        // F-04's claim is a *type-level* one, so its executable form is a destructuring without
        // `..`: the funnel's last word has exactly two fields today, and a future `subpixel: f32`
        // next to `d` would break this build instead of quietly re-introducing an interpolation
        // that the evidence cannot support (N3).
        assert_eq!(
            REFINE_NEIGHBOURHOOD,
            [-1, 0, 1],
            "the third layer's freedom is ±1 integer pixel and nothing else"
        );

        let image = mixed_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);
        let candidates = candidates_1d(&previous.view(), &current.view(), 120, 8);
        let scored = scored_once(&previous, &current, &candidates);
        let refined = refined_once(&previous, &current, &scored)
            .expect("layer 2 proposed a shift, so layer 3 has something to refine");

        let Refined { d, zncc2d } = refined;
        let _: i32 = d;
        assert_eq!(d, 120, "a grid-aligned step must survive the refinement untouched");
        assert!(
            zncc2d.is_finite() && (-1.0..=1.0).contains(&zncc2d),
            "a correlation outside [-1, 1] is not a correlation: {zncc2d}"
        );

        // No scored candidate means nothing was measured, and the third layer says so with `None`
        // rather than with a zero shift (`P1.04`'s rule: `None` is a state, not a value).
        assert!(
            refined_once(&previous, &current, &ScoredSet::new()).is_none(),
            "an empty scored set has no winner to refine"
        );
    }

    /// `docs/31` §6 `P1.07`'s exit condition ③: `docs/30` §30.3's "integer shifts (both signs,
    /// 1..40)" row, in the part of it that exists at this layer.
    ///
    /// The gates (§16.1–§16.5) are `P1.08`–`P1.11`, so what can be asked today is narrower and
    /// sharper: for every integer shift the funnel can *measure*, the number that comes out of the
    /// third layer is the truth. A ±1 neighbourhood can only do that if the winner it refines
    /// around is already within one pixel of the truth — which is what makes this the test that
    /// decides whether layer 2's cell tie-break and layer 3's authority fit together.
    #[test]
    fn every_integer_shift_from_one_to_forty_comes_back_exactly() {
        let image = mixed_document();
        let mut failures: Vec<(i32, Option<i32>)> = Vec::new();
        for magnitude in 1..=40 {
            for direction in [1i32, -1] {
                let truth = magnitude * direction;
                // The script starts at the top of the document, so a backward step needs room to
                // come back up: one 40 px step first, and the pair is the second and third frames.
                let steps = if direction > 0 {
                    vec![StepSpec::move_by(truth)]
                } else {
                    vec![StepSpec::move_by(40), StepSpec::move_by(truth)]
                };
                let mut script = ScrollScript::new(&image, 900, steps);
                // `take` is sequential (`docs/31` §0.6 `DEV-9` note 3): the fixture hands out the
                // stream in order, so the third frame is only reachable through the second.
                let first = script.take(0);
                let second = script.take(1);
                let (previous, current) = if direction > 0 {
                    (first, second)
                } else {
                    let third = script.take(2);
                    (second, third)
                };
                let candidates = candidates_1d(&previous.view(), &current.view(), truth, 8);
                let scored = scored_once(&previous, &current, &candidates);
                let measured = refined_once(&previous, &current, &scored);
                match measured {
                    Some(refined) if refined.d == truth => {}
                    other => failures.push((truth, other.map(|refined| refined.d))),
                }
            }
        }
        assert!(
            failures.is_empty(),
            "shifts that did not come back: {failures:?}"
        );
    }

    /// REFACTOR (`P1.07`): the tie the second layer cannot break is broken by the first layer's
    /// evidence, not by a grid heuristic.
    ///
    /// A shift of 2 (or 6, or −2) sits two pixels from the grid point its cell is measured at, so
    /// ranking the cell's members by distance to that grid point hands layer 3 a winner its ±1
    /// neighbourhood cannot reach. Measured before the fix (`P1.07`'s sweep): `[(2, Some(1)),
    /// (-2, Some(-3)), (6, Some(5)), …]` — every |d| ≡ 2 (mod 4), each exactly 1 px off. Layer 1's
    /// support is computed per shift and does see the difference, so it is the one that decides,
    /// and what layer 3 then needs from layer 2 is only that its winner is within one pixel.
    #[test]
    fn the_cell_tie_is_broken_by_the_first_layers_evidence() {
        let image = mixed_document();
        for truth in [2, 6, -2, -6] {
            let steps = if truth > 0 {
                vec![StepSpec::move_by(truth)]
            } else {
                vec![StepSpec::move_by(40), StepSpec::move_by(truth)]
            };
            let mut script = ScrollScript::new(&image, 900, steps);
            let first = script.take(0);
            let second = script.take(1);
            let (previous, current) = if truth > 0 {
                (first, second)
            } else {
                let third = script.take(2);
                (second, third)
            };
            let candidates = candidates_1d(&previous.view(), &current.view(), truth, 8);
            let scored = scored_once(&previous, &current, &candidates);
            let winner = scored
                .iter()
                .next()
                .expect("the second layer scored nothing")
                .d;
            let grid_point = round_to_grid(winner) * DOWNSAMPLE as i32;

            // The counterfactual, as an assertion: the cell's grid point — what a distance-to-grid
            // rule would have reported — is out of the third layer's reach for this shift.
            assert_eq!(
                (grid_point - truth).abs(),
                2,
                "shift {truth}: this case only means something while its cell's grid point \
                 ({grid_point}) sits two pixels away"
            );
            // The fix: the winner layer 2 actually reports is inside the ±1 neighbourhood.
            assert!(
                winner.abs_diff(truth) <= 1,
                "shift {truth}: layer 2 picked {winner}, which is out of layer 3's ±1 reach"
            );
            assert_eq!(
                refined_once(&previous, &current, &scored).map(|refined| refined.d),
                Some(truth),
                "shift {truth}: the third layer did not recover the truth from {winner}"
            );
        }
    }

    #[test]
    fn the_gate_accepts_exactly_viewport_extent_and_rejects_one_more() {
        // `docs/31` §6 `P1.08`; `docs/30` §16.2. The interval is closed, and the reason is a fact
        // about overlap rather than a preference about confidence: at `|d| == extent` exactly one
        // pixel row of the previous frame is still on screen, at `extent + 1` none is, so no
        // measurement can exist — that is what makes this a *correctness* constraint.
        const EXTENT: u32 = 10;
        for d in [EXTENT as i32, -(EXTENT as i32)] {
            assert_eq!(gate_geometry(d, EXTENT), GateOutcome::Pass, "d = {d}");
        }
        for d in [EXTENT as i32 + 1, -(EXTENT as i32) - 1] {
            assert_eq!(
                gate_geometry(d, EXTENT),
                GateOutcome::Reject(GateRejection::OutsideViewport),
                "d = {d}"
            );
        }

        // The gate reads the viewport extent and nothing else — no search window, no prior — so a
        // broken search centre (§16.6's `ĝ`) can only waste candidates, never open a hole here.
        assert_eq!(gate_geometry(0, 0), GateOutcome::Pass);
    }

    #[test]
    fn the_gate_never_returns_half_the_dimension() {
        // `docs/31` §6 `P1.08`; `docs/30` §16.9. `±N/2`/`±M/2` is where every undefined and every
        // wraparound path lands (the phase-correlation fallback returned `(-N/2, -M/2)`), so it is
        // refused even though it passes the geometry gate above.
        assert_eq!(
            gate_geometry(5, 10),
            GateOutcome::Reject(GateRejection::BannedHalf)
        );
        assert_eq!(
            gate_geometry(-5, 10),
            GateOutcome::Reject(GateRejection::BannedHalf)
        );

        // The answer is `None`, not `Uncertain`: the estimator walked into a branch it must not
        // trust, and §16.10 wants that visible (G12) instead of looking like a hesitant match.
        assert_eq!(GateRejection::BannedHalf.status(), Status::None);
        assert_eq!(GateRejection::OutsideViewport.status(), Status::None);

        // An odd extent has no exact half, so the rule names the truncation instead of quietly
        // skipping itself (`11 / 2 == 5`).
        assert_eq!(
            gate_geometry(5, 11),
            GateOutcome::Reject(GateRejection::BannedHalf)
        );

        // A viewport with no half at all must not ban `d == 0`, which would make "the frame did not
        // move" unreportable — the one answer §16.3's fingerprint path exists to confirm.
        for extent in [0, 1] {
            assert_eq!(gate_geometry(0, extent), GateOutcome::Pass, "extent {extent}");
        }
    }

    #[test]
    fn the_verifiability_floor_is_a_different_number_from_the_hard_gate() {
        // `docs/31` §6 `P1.08` REFACTOR; `docs/30` §16.1. V1 (and the first draft of `docs/25` R17)
        // spoke of "the cap" and "the credibility" as one thing. They are two quantities: §16.2 is a
        // fact about overlap (`|d| <= extent`, zero parameters), `RHO_MIN` is how much overlap must
        // remain to count as evidence (calibratable, §16.11).
        const EXTENT: u32 = 100;

        // A shift that passes the hard gate and is still not verifiable: 34% of the viewport left.
        assert_eq!(gate_geometry(66, EXTENT), GateOutcome::Pass);
        assert!(!is_verifiable(66, EXTENT));

        // 35% is the documented target overlap (`docs/30` §14.2: `ρ* = 0.35`) and the floor.
        assert!(is_verifiable(65, EXTENT));
        assert!(!is_verifiable(66, EXTENT));
        assert_eq!(RHO_MIN, 0.35);
        assert_eq!(
            (RHO_MIN * 1000.0).round() as i64,
            RHO_MIN_PERMILLE,
            "the exposed float and the integer the floor actually compares must be one number"
        );

        // And the hard gate's own edge is not verifiable at all: one row left.
        assert_eq!(gate_geometry(100, EXTENT), GateOutcome::Pass);
        assert!(!is_verifiable(100, EXTENT));
        assert_eq!(
            gate_geometry(101, EXTENT),
            GateOutcome::Reject(GateRejection::OutsideViewport)
        );
    }

    /// The same geometry as [`mixed_document`], different content: what the viewport shows after a
    /// navigation, and the fixture for "two frames that have nothing to do with each other".
    fn unrelated_document() -> TestImage {
        TestImage::from_structures(
            640,
            60 * 19,
            13,
            19,
            &[
                Structure::NoiseBlocks { cell: 8 },
                Structure::Gradient,
                Structure::Checker { cell: 12 },
            ],
        )
    }

    #[test]
    fn a_peak_that_is_no_better_than_standing_still_is_rejected() {
        // `docs/31` §6 `P1.09`; `docs/30` §16.3 (F-03). Gate two is the only gate that must pass, and
        // what it refuses is the leap "a peak exists" ⇒ "the peak is a measurement": a candidate that
        // explains no more of the zero-shift residual than standing still did is not a movement.
        //
        // The ratio is *relative* — an absolute similarity is near 1 on a blank page — so it can be
        // pinned on numbers first, and the floor is closed (§16.3 says `gain ≥ 0.15`).
        assert_eq!(residual_gain(0.20, 0.20), Some(0.0));
        assert_eq!(residual_gain(0.14, 0.20), Some(0.30));
        assert_eq!(
            gate_residual_gain(0.0),
            GateOutcome::Reject(GateRejection::ResidualGainTooSmall)
        );
        assert_eq!(
            gate_residual_gain(MIN_RESIDUAL_GAIN - 0.01),
            GateOutcome::Reject(GateRejection::ResidualGainTooSmall)
        );
        assert_eq!(gate_residual_gain(MIN_RESIDUAL_GAIN), GateOutcome::Pass);
        assert_eq!(GateRejection::ResidualGainTooSmall.status(), Status::None);

        // Positive control, so the gate is not a blanket "no": two frames of a real page, translated
        // by a shift the estimator is given.
        let image = mixed_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);
        let gain = gain_once(&previous, &current, 120)
            .expect("a translated page must have a measurable residual ratio");
        assert!(
            gain > MIN_RESIDUAL_GAIN,
            "the true shift explained only {gain} of the zero-shift residual"
        );
        assert_eq!(gate_residual_gain(gain), GateOutcome::Pass);

        // The physical form of the same claim: two frames of the same geometry and unrelated content.
        // Every alignment is as bad as no alignment, so the strongest candidate explains nothing.
        // §16.8's scene cut is how a session reaches this state without a fixture.
        let other = unrelated_document();
        let mut first_script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let mut second_script = ScrollScript::new(&other, 900, vec![StepSpec::move_by(120)]);
        let previous = first_script.take(0);
        let current = second_script.take(0);
        let gain = gain_once(&previous, &current, 120)
            .expect("neither frame is empty, so the ratio exists");
        assert!(
            gain < MIN_RESIDUAL_GAIN,
            "unrelated frames claimed a gain of {gain}, which gate two must refuse"
        );
        assert_eq!(
            gate_residual_gain(gain),
            GateOutcome::Reject(GateRejection::ResidualGainTooSmall)
        );
    }

    #[test]
    fn when_the_winner_is_zero_the_gain_is_undefined_and_the_fingerprint_path_runs() {
        // `docs/31` §6 `P1.09`; `docs/30` §16.3's `d_best == 0` case. The ratio is `0/0`, and
        // "undefined" is a different answer from "no gain": a `0.0` would let the zero shift be
        // ranked and gated like a measured movement. That is why `residual_gain` returns an
        // `Option` and this case is routed to the fingerprints — byte-identical primary lines are the
        // only way to say "the page really did not move".
        assert_eq!(residual_gain(0.0, 0.0), None);
        // The floor is `GAIN_RMSE_FLOOR` (1e-3): a residual that small is quantisation dust, not a
        // denominator. Above it the ratio is ordinary and real.
        assert_eq!(residual_gain(0.0, 1e-9), None);
        assert_eq!(residual_gain(0.0, 1e-2), Some(1.0));

        // The page did not move: §16.3's duplicate detection is the *only* way a zero shift can be
        // confirmed, so it must confirm it.
        let image = mixed_document();
        let mut still = ScrollScript::new(&image, 900, vec![StepSpec::move_by(0)]);
        let previous = still.take(0);
        let current = still.take(1);
        assert_eq!(
            primary_digests(&previous.view()),
            primary_digests(&current.view()),
            "the fixture did not hand out an unchanged frame, so this case proves nothing"
        );
        assert_eq!(
            zero_shift_status(&previous.view(), &current.view()),
            Status::Confirmed { d: 0 }
        );

        // A frame that differs is *not* a confirmed zero: the fingerprints differ, so §16.3 says
        // `None` and no canvas row is committed on this evidence. Note the second case — the same
        // page, relit — which the byte-level fingerprint also reports as a change: that is the
        // conservative direction (a missed zero costs a frame; a false zero costs a duplicated row).
        let other = unrelated_document();
        let mut other_script = ScrollScript::new(&other, 900, vec![StepSpec::move_by(0)]);
        let different = other_script.take(0);
        assert_eq!(
            zero_shift_status(&previous.view(), &different.view()),
            Status::None
        );
        let relit = relight(&current, 1.0, 6.0);
        assert_eq!(
            zero_shift_status(&previous.view(), &relit.view()),
            Status::None
        );
    }

    #[test]
    fn the_gate_and_the_ranking_measure_the_same_region() {
        // `docs/31` §6 `P1.09` REFACTOR, as an equality rather than a comment. The ratio has two
        // inputs that are easy to get subtly wrong: which region each residual covers, and what unit
        // the shift is in (layer 2 measures on `DOWNSAMPLE`-pixel cells). The first version of
        // `residual_gain_at` passed full-resolution pixels into a pooled grid, and the true shift of
        // a 120 px move came back with a gain of **0.019** — below the floor, on a page that had
        // visibly moved. So: the gate's number for a shift and the ranking's number for that same
        // shift are one number, for every candidate the ranking kept.
        let image = mixed_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);

        let candidates = candidates_1d(&previous.view(), &current.view(), 120, 8);
        let scored = scored_once(&previous, &current, &candidates);
        assert!(!scored.is_empty(), "the ranking kept no candidate at all");
        for candidate in scored.iter() {
            assert_eq!(
                gain_once(&previous, &current, candidate.d),
                Some(candidate.gain),
                "gate two and the ranking disagree about shift {}",
                candidate.d
            );
        }
    }

    #[test]
    fn four_tiles_that_touch_each_other_count_as_two() {
        // `docs/31` §6 `P1.10`; `docs/30` §16.4. The task book's name says "count as one"; the rule
        // the same section writes is pairwise — two tiles are independent only if `|i − j| ≥ 2` —
        // and for four consecutive tiles the largest pairwise-separated subset is `{0, 2}`, i.e.
        // **two**. The cluster reading ("a run is one supporter") would make the gate unreachable:
        // a single contiguous matching region of twenty tiles would count 1 < 4 and every correct
        // match on a real page would be rejected. The name's claim survives in the part that
        // matters: four touching tiles must not carry four supports, and four is the gate.
        assert_eq!(independent_support([0, 1, 2, 3].into_iter()), 2);
        // Spaced two apart, the same four tiles *are* four pieces of evidence.
        assert_eq!(independent_support([0, 2, 4, 6].into_iter()), 4);
        assert_eq!(independent_support([1, 3, 5, 7].into_iter()), 4);
        // Six touching tiles are three: the greedy count is the size of the largest independent set,
        // not the number of runs and not the number of tiles.
        assert_eq!(independent_support([0, 1, 2, 3, 4, 5].into_iter()), 3);
        // A non-supporting tile does not break the spacing: what is excluded is *adjacency to an
        // accepted supporter*, not proximity to a gap.
        assert_eq!(independent_support([0, 3, 4, 5, 6].into_iter()), 3);
        assert_eq!(independent_support([0, 1].into_iter()), 1);
        assert_eq!(independent_support([7].into_iter()), 1);
        assert_eq!(independent_support([].into_iter()), 0);
        // The floor and the gap together imply a minimum band: `MIN_TILES` pairwise-separated tiles
        // need `2·MIN_TILES − 1 = 7` whole tiles, i.e. 224 full-resolution band rows, i.e. a viewport
        // of about 448 primary rows (`H_match = H/2`, §15.6). Below that no page can satisfy gate
        // three — a design consequence for `E-ACC-1` to weigh, not an accident to discover later.
        assert_eq!(independent_support(0..7), MIN_TILES);
        assert_eq!(independent_support(0..6), MIN_TILES - 1);
    }

    #[test]
    fn a_single_patch_supporter_is_rejected() {
        // `docs/31` §6 `P1.10`; `docs/30` §16.4 (F-02). One patch agreeing is what a periodic or a
        // single-feature page produces: the shift is right there and wrong everywhere else, so the
        // count of *independent* supporters is the difference between "measured" and "measured in
        // one place". The floor is `MIN_TILES = 4`, closed: exactly four independent supporters pass.
        assert_eq!(MIN_TILES, 4);
        assert_eq!(gate_support(0), GateOutcome::Reject(GateRejection::TooFewSupporters));
        assert_eq!(gate_support(1), GateOutcome::Reject(GateRejection::TooFewSupporters));
        assert_eq!(
            gate_support(MIN_TILES - 1),
            GateOutcome::Reject(GateRejection::TooFewSupporters)
        );
        assert_eq!(gate_support(MIN_TILES), GateOutcome::Pass);
        assert_eq!(gate_support(MIN_TILES + 1), GateOutcome::Pass);
        assert_eq!(GateRejection::TooFewSupporters.status(), Status::None);
        // §16.4's gap is part of the rule, not a tunable: one tile apart is adjacency, and the
        // reference implementation's tile size is 32 px in both places only because the same
        // estimator carries it (`P1.10` REFACTOR: this grid is the *matching evidence* grid).
        assert_eq!(TILE_INDEPENDENCE_GAP, 2);
    }

    #[test]
    fn a_low_texture_page_has_no_independent_supporters() {
        // `docs/31` §6 `P1.10` exit condition ②; `docs/30` §30.3's "低纹理" row (L1 part: the
        // estimator's answer). A page with no structure has no tile that clears §16.4's threshold,
        // so the count is zero for every candidate and gate three rejects all of them — the session
        // ends up with `None` (§16.1: every candidate dropped), not with a confident zero.
        let image = TestImage::from_structures(320, 80 * 19, 3, 80 * 19, &[Structure::Flat]);
        let mut script = ScrollScript::new(&image, 300, vec![StepSpec::move_by(10)]);
        let previous = script.take(0);
        let current = script.take(1);
        let candidates = candidates_1d(&previous.view(), &current.view(), 10, 8);
        let scored = scored_once(&previous, &current, &candidates);
        assert!(!scored.is_empty(), "the second layer scored nothing");
        for candidate in scored.iter() {
            assert_eq!(
                candidate.tiles, 0,
                "a flat band supported shift {} with {} tiles",
                candidate.d, candidate.tiles
            );
            assert_eq!(
                gate_support(candidate.tiles),
                GateOutcome::Reject(GateRejection::TooFewSupporters),
                "shift {} passed gate three on a page with no structure",
                candidate.d
            );
        }
    }

    /// How many whole tiles agree with a shift, **without** §16.4's independence rule.
    ///
    /// The rule's whole point is that this number is not the evidence count, so the tests that make
    /// the claim have to be able to see both. Deliberately a local copy of the old loop: if
    /// [`supporting_tiles`] ever stops applying [`independent_support`], the tests below keep
    /// comparing 14 against 7 instead of comparing a number against itself.
    fn raw_supporting_tiles(previous: &Gray, current: &Gray, shift: i32) -> u32 {
        let wanted = match_rows(previous.height, current.height, DOWNSAMPLE);
        let band = match_band(previous.height, current.height, shift, wanted).expect("overlaps");
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

    #[test]
    fn a_fully_supporting_band_keeps_only_its_independent_tiles() {
        // `docs/31` §6 `P1.10`; the measurement the design section quotes. On the mixed document at
        // the true shift every whole tile of the band agrees — the raw count is the whole band (14
        // tiles of 8 cells for a 900 px viewport) — and the independent count is exactly half of it:
        // `ceil(14/2) = 7`. A run of agreeing tiles is `ceil(m/2)` pieces of evidence, so the gate
        // at `MIN_TILES = 4` is met only because the band is 14 tiles wide and not 6.
        let image = mixed_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);

        let previous_gray = Gray::pooled(&previous.view());
        let current_gray = Gray::pooled(&current.view());
        assert_eq!(
            raw_supporting_tiles(&previous_gray, &current_gray, round_to_grid(120)),
            14,
            "the fixture no longer fills the whole band at the true shift"
        );

        let candidates = candidates_1d(&previous.view(), &current.view(), 120, 8);
        let scored = scored_once(&previous, &current, &candidates);
        let winner = scored.iter().next().expect("the true shift is a candidate");
        assert_eq!(winner.d, 120);
        assert_eq!(
            winner.tiles, 7,
            "the ranking is still counting adjacent tiles as separate evidence"
        );
        // §16.7's `coverage` is defined on the independent count, and it is *not* saturated here:
        // `7/12`. A 900 px viewport cannot reach 12 independent supporters at all — that needs a
        // band of 23 tiles, i.e. ~1400 px of viewport — so `coverage` stays partial on ordinary
        // windows and the term is a soft weight rather than a switch.
        assert!((coverage_of(winner.tiles) - 7.0 / 12.0).abs() < 1e-6);
        assert!(coverage_of(winner.tiles) < 1.0);
    }

    #[test]
    fn a_gradient_page_ties_on_correlation_and_leaves_ambiguity_to_gate_four() {
        // `docs/31` §6 `P1.10`; `docs/30` §30.3's 低纹理 row. The row has two halves and they are
        // different gates: a page with *no* structure is gate three's (the test above), while a
        // smooth ramp correlates perfectly at **every** shift — zero-mean correlation is blind to the
        // constant offset a ramp shift produces — so it passes gate two with a real gain and is
        // ambiguous rather than unsupported. That ambiguity is §16.5's margin, i.e. `P1.11`.
        //
        // Gate three rejects it here for a *second* reason, and the test says so instead of leaving
        // it implied: this fixture's viewport is 300 px, so its band is 4 whole tiles and even a
        // perfectly supported shift yields `ceil(4/2) = 2` independent supporters.
        let gradient = TestImage::from_structures(320, 80 * 19, 5, 80 * 19, &[Structure::Gradient]);
        let mut script = ScrollScript::new(&gradient, 300, vec![StepSpec::move_by(10)]);
        let previous = script.take(0);
        let current = script.take(1);

        let previous_gray = Gray::pooled(&previous.view());
        let current_gray = Gray::pooled(&current.view());
        assert_eq!(
            raw_supporting_tiles(&previous_gray, &current_gray, round_to_grid(10)),
            4,
            "a ramp should fill every whole tile of this band"
        );

        let candidates = candidates_1d(&previous.view(), &current.view(), 10, 8);
        let scored = scored_once(&previous, &current, &candidates);
        assert!(!scored.is_empty());
        // The measurement, as measured (2026-10-08): eight candidates in two families —
        // `d = 6..9` at `zncc2d = 0.999986231` with `gain = 0.573598564`, and `d = 10..13` at
        // `zncc2d = 0.999984145` with `gain = 0.539433837` (the true shift 10 is in the second one).
        // The ramp is a *tie*, not a ranking: the residual-correlation difference between the
        // families is 2.1e-6 and comes from integer rounding of the ramp rather than from
        // displacement evidence. What separates them at all is the gain term — `0.25 · 0.0342`, i.e.
        // ~8.7e-3 of score, or a `margin` of ~8.7e-3 against §16.5's `MIN_MARGIN = 0.15`.
        // So this page is `Uncertain` through gate four (`P1.11`), and gate three cannot say so:
        // it counts supporters, and every candidate has the same count.
        assert_eq!(scored.len(), 8);
        let best = *scored.iter().next().expect("eight candidates");
        assert!((best.zncc2d - 0.999_986_231).abs() < 1e-6, "{}", best.zncc2d);
        for candidate in scored.iter() {
            assert!(
                candidate.zncc2d > 0.9999,
                "a ramp stopped correlating at shift {} ({})",
                candidate.d,
                candidate.zncc2d
            );
            assert_eq!(
                gate_residual_gain(candidate.gain),
                GateOutcome::Pass,
                "the ramp shift {} should still gain over standing still",
                candidate.d
            );
            assert_eq!(candidate.tiles, 2);
            assert_eq!(
                gate_support(candidate.tiles),
                GateOutcome::Reject(GateRejection::TooFewSupporters)
            );
            // An order of magnitude inside `MIN_MARGIN = 0.15`: on this page no shift can clear
            // gate four, which is the answer the design wants.
            assert!(
                (candidate.score - best.score).abs() / best.score < 0.02,
                "shift {} is not inside the tie: {} against {}",
                candidate.d,
                candidate.score,
                best.score
            );
        }
    }

    /// `docs/30` §30.3's 二维周期 row: a board whose period is far shorter than the search window, so
    /// a whole family of aliases lands inside it. `cell = 8` gives a period of 16 px, while the search
    /// half-width below is 40 px — the aliases at `120 ± 16` and `120 ± 32` are all candidates.
    fn periodic_board_document() -> TestImage {
        TestImage::from_structures(640, 60 * 19, 9, 19, &[Structure::Checker { cell: 8 }])
    }

    #[test]
    fn equal_scoring_modes_report_uncertain() {
        // `docs/31` §6 `P1.11`; `docs/30` §16.5 and §30.3's 二维周期（棋盘）row. A two-dimensional
        // board of period `p` makes `d` and `d ± p` the *same* observation, so the scored set holds a
        // family of equally good modes and no amount of correlation separates them. Gate four is the
        // only place that fact can be reported, and reporting it is the point: `D`'s answer for this
        // page is `Uncertain`, not `None` — a shift was measured, the evidence just does not pick one.
        let image = periodic_board_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);

        let candidates = candidates_1d(&previous.view(), &current.view(), 120, 40);
        let scored = scored_once(&previous, &current, &candidates);
        let best = *scored.iter().next().expect("the true shift is a candidate");
        let second = *scored
            .iter()
            .nth(1)
            .expect("a periodic page has a second mode");

        // The measurement, as measured (2026-10-08): `best = 120` at `score = 0.9375` and
        // `second = 139` at `score = 0.8373216`. The runner-up is not the alias the arithmetic
        // predicts (`120 + 16 = 136`) but another member of the same family, 19 px away — and that is
        // the useful part: on this page the second mode is **89.3%** of the winner, i.e. above §16.6's
        // `0.85 · score(best)` family threshold, so "peak family" and "margin below `MIN_MARGIN`" are
        // two sentences about one measurement.
        assert_eq!(best.d, 120, "the true shift must win on a periodic page");
        assert!(
            second.score > 0.85 * best.score,
            "the runner-up at {} is only {} — this is no longer the family §16.5 and §16.6 describe",
            second.d,
            second.score
        );
        let margin = margin_of(best.score, Some(second.score)).expect("both scores are positive");
        assert!(
            margin < MIN_MARGIN,
            "the period separated {} ({}) from {} ({}) by {margin}, which gate four would accept as a decision",
            best.d,
            best.score,
            second.d,
            second.score
        );
        assert_eq!(
            gate_margin(best.d, Some(margin)),
            GateOutcome::Reject(GateRejection::MarginTooSmall { best: best.d })
        );
        assert_eq!(
            GateRejection::MarginTooSmall { best: best.d }.status(),
            Status::Uncertain { d: best.d },
            "an ambiguous winner is still a measurement, so gate four's rejection is `Uncertain`"
        );
    }

    #[test]
    fn a_two_pixel_margin_is_not_enough() {
        // `docs/31` §6 `P1.11`. Layer two measures a shift at the grid point of a 4 px cell, so the
        // candidates inside one cell carry *the same* measurement — identical `zncc2d`, `gain`,
        // `tiles` and `score` — and each of them sits at most 2 px from the shift that was measured.
        // Gate four sees a margin of zero and must not confirm any of them; a rival in the next cell
        // is a real rival.
        let image = mixed_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);

        let candidates = candidates_1d(&previous.view(), &current.view(), 120, 40);
        let scored = scored_once(&previous, &current, &candidates);
        let best = *scored.iter().next().expect("the true shift is a candidate");
        let second = *scored
            .iter()
            .nth(1)
            .expect("a candidate set has a runner-up");
        assert!(
            best.d.abs_diff(second.d) <= 2,
            "the runner-up is {} px from {} — this fixture no longer puts both in one cell",
            best.d.abs_diff(second.d),
            best.d
        );

        let tied = margin_of(best.score, Some(second.score)).expect("both scores are positive");
        // The measurement, as measured (2026-10-08): `best = 120` at `0.9375`, `second = 119` at
        // `0.9375` — one pixel apart, so the two scores are *bitwise* equal and the tie is total.
        assert_eq!(best.d, 120);
        assert_eq!(second.d, 119);
        assert_eq!(
            tied, 0.0,
            "{} ({}) and {} ({}) are in one cell and must share one measurement",
            best.d, best.score, second.d, second.score
        );
        assert_eq!(
            gate_margin(best.d, Some(tied)),
            GateOutcome::Reject(GateRejection::MarginTooSmall { best: best.d })
        );

        // The other side of the same gate, from the same measurement: a candidate one cell away is a
        // real rival. As measured (2026-10-08): `117` scores `0.53167087`, so the margin is
        // `0.4328844` — nearly three times the threshold — and gate four passes it.
        let rival = *scored
            .iter()
            .find(|candidate| candidate.d == 117)
            .expect("117 is inside the search window");
        let decided = margin_of(best.score, Some(rival.score)).expect("both scores are positive");
        assert!(
            decided >= MIN_MARGIN,
            "{} is a whole cell from {} but only scored {decided} better",
            rival.d,
            best.d
        );
        assert!((decided - 0.432_884_4).abs() < 1e-6, "{decided}");
        assert_eq!(gate_margin(best.d, Some(decided)), GateOutcome::Pass);

        // §16.5's floor is closed, the same way §16.3's and §16.4's are — asserted on the gate, not
        // by rebuilding the rival's score from the threshold. The reconstruction does not survive
        // `f32`: `1.0 - MIN_MARGIN` is the **exact** midpoint between `0.84999996` and `0.85000002`
        // (ulp 5.96e-8 at that exponent), ties-to-even rounds it *up* to `0.85000002`, and the ratio
        // it produces is `0.14999998` — below the floor it was built from. That is a fact about
        // binary floats rather than about gate four, so the boundary is pinned where the comparison
        // actually happens.
        assert_eq!(gate_margin(best.d, Some(MIN_MARGIN)), GateOutcome::Pass);
        assert_eq!(
            gate_margin(best.d, Some(MIN_MARGIN - 1e-6)),
            GateOutcome::Reject(GateRejection::MarginTooSmall { best: best.d })
        );
    }

    #[test]
    fn no_rival_and_no_scale_are_both_not_an_ambiguity() {
        // `docs/31` §6 `P1.11`'s REFACTOR: the D-class pair §30.3 lists — the boundary value and the
        // 2D periodic page — land in *different* states, and neither depends on gate one's window.
        // The boundary value is gate one's rejection and reports `None`; the ambiguity is gate four's
        // and reports `Uncertain`.
        assert_eq!(GateRejection::BannedHalf.status(), Status::None);
        assert_eq!(
            GateRejection::MarginTooSmall { best: 7 }.status(),
            Status::Uncertain { d: 7 }
        );

        // A lone candidate has no rival to be confused with. `None` here is not a measurement of
        // uniqueness — it is the absence of a comparison, and gate four is about a *rival*.
        assert_eq!(margin_of(1.0, None), None);
        assert_eq!(gate_margin(120, None), GateOutcome::Pass);

        // `0 / 0` is not a margin either. The second `None` is unreachable through the pipeline, and
        // the arithmetic is why: a candidate that passed gate two scores at least
        // `SCORE_GAIN · MIN_RESIDUAL_GAIN = 0.25 · 0.15 = 0.0375`, well above the floor at which the
        // ratio stops being computable. This assertion is the whole of that argument.
        assert_eq!(margin_of(0.0, Some(0.0)), None);
        assert_eq!(gate_margin(120, margin_of(0.0, Some(0.0))), GateOutcome::Pass);
        assert!(SCORE_GAIN * MIN_RESIDUAL_GAIN > MARGIN_SCORE_FLOOR);

        // §16.6's manual-mode peak-family rule — "a second mode above `0.85 · score(best)` is a
        // family" — is the same number as this gate: `1 − 0.85 == MIN_MARGIN`. So there is one rule
        // with two spellings of its threshold, not two rules, and no second detector is built.
        assert!((1.0 - 0.85_f32 - MIN_MARGIN).abs() < 1e-6);
    }

    #[test]
    fn a_single_rearranged_frame_stays_uncertain_and_the_session_continues() {
        // `docs/31` §6 `P1.12`; `docs/30` §16.8, §16.10, §30.3 (`scene cut ×1–2`). Two frames with
        // the same geometry and unrelated content are what the viewport shows after a navigation.
        // §16.8 asks two independent questions about them, and the cheap one is asked first: the page
        // differs *at zero shift*, and then *no candidate aligns* — not "the best one is not good
        // enough", which is gate four's question and a different fact (F-10's whole point).
        let image = mixed_document();
        let other = unrelated_document();
        let mut first = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let mut second = ScrollScript::new(&other, 900, vec![StepSpec::move_by(120)]);
        let previous = first.take(0);
        let current = second.take(0);

        let similarity = zero_shift_similarity(&previous.view(), &current.view());
        assert!(
            similarity < SCENE_CUT_SIMILARITY,
            "two unrelated frames looked alike at zero shift ({similarity}); the fixture is wrong"
        );
        // Measured 2026-10-08 (`docs/Temp/p112-measurements.log`): similarity `0.09400253`; the
        // winner is `d = −19` with `alignment_error = 0.8133493`, and all eight candidates land in
        // `0.8133…0.8564` against a threshold of `0.60`. Nothing aligns anywhere, which is what
        // §16.8's second condition is asking — not "the winner is weak" (that is gate four's
        // question, and gate four would have answered it on this frame too).

        let candidates = candidates_1d(&previous.view(), &current.view(), 0, 40);
        let scored = scored_once(&previous, &current, &candidates);
        assert!(scored.len() > 1, "the fixture produced too few candidates to test");
        assert!(
            is_scene_cut_once(&previous, &current, &scored),
            "unrelated frames were not recognised as a scene cut"
        );

        // The second condition really is about *every* candidate, winner included: on a scene cut
        // there is nothing to align to, which is why the answer is "the page changed" and not
        // "the winner is doubtful".
        for candidate in scored.iter() {
            let error = alignment_error(&previous.view(), &current.view(), candidate.d)
                .expect("unrelated frames have structure on both sides");
            assert!(
                error > SCENE_CUT_ALIGNMENT_ERROR,
                "candidate {} aligned at error {error}",
                candidate.d
            );
        }

        // 1–2 consecutive scene cuts are tolerated: the streak counts them, and asks for nothing.
        let once = SceneCut::none().observe(true);
        let twice = once.observe(true);
        assert_eq!(SceneCut::none().consecutive(), 0);
        assert_eq!(once.consecutive(), 1);
        assert_eq!(twice.consecutive(), 2);
        assert_eq!(once.action(), SceneCutAction::Tolerate);
        assert_eq!(twice.action(), SceneCutAction::Tolerate);

        // §30.3's row for ×1–2: `Uncertain`, and the session continues. `Uncertain` and not `None`
        // for the same reason gate four's ambiguity is not `None`: a shift *was* measured, the frame
        // is just not trusted. `effect()` is §16.10's canvas column in one place, so "the canvas is
        // not written and the session goes on" is asserted rather than re-derived.
        let winner = scored.iter().next().expect("checked above").d;
        let evidence = Evidence {
            scene_cut: twice,
            ..evidence()
        };
        let displacement = Displacement::uncertain(winner, evidence);
        assert!(matches!(displacement.status(), Status::Uncertain { .. }));
        assert_eq!(displacement.effect(), StepEffect::Continue);
        assert_eq!(displacement.evidence().scene_cut().consecutive(), 2);
    }

    #[test]
    fn three_consecutive_scene_cuts_decay_the_model_but_do_not_stop() {
        // `docs/31` §6 `P1.12`; `docs/30` §16.8's streak table and §30.3 (`scene cut ×≥3`). The
        // third consecutive cut is where the *model* reacts (`decay_toward_neutral(0.05)` + reset,
        // §18.2) — and that is all that happens: a changed page is a fact about the page, so there is
        // no shape here for stopping the session.
        let tolerated = SceneCut::none().observe(true).observe(true);
        assert_eq!(tolerated.consecutive(), 2);
        assert_eq!(tolerated.action(), SceneCutAction::Tolerate);

        let decaying = tolerated.observe(true);
        assert_eq!(decaying.consecutive(), SCENE_CUT_DECAY_STREAK);
        assert_eq!(decaying.action(), SceneCutAction::DecayModel);

        // "Consecutive" is the load-bearing word: one frame that aligns resets the streak, and a
        // frame that is *not* a scene cut while already at zero leaves it at zero.
        assert_eq!(decaying.observe(false), SceneCut::none());
        assert_eq!(decaying.observe(false).consecutive(), 0);
        assert_eq!(SceneCut::none().observe(false), SceneCut::none());

        // The counter saturates rather than wrapping: a page that never comes back is one number,
        // and `u8::MAX` scene cuts decay the model exactly as three do.
        let mut stuck = SceneCut::none();
        for _ in 0..600 {
            stuck = stuck.observe(true);
        }
        assert_eq!(stuck.consecutive(), u8::MAX);
        assert_eq!(stuck.action(), SceneCutAction::DecayModel);

        // §16.8's "永不因 scene cut 终止会话" is a property of the type, so its executable form is an
        // exhaustive `match`: every action the streak can ask for continues the session, and adding a
        // stop would mean adding an arm here — which is the review this REFACTOR asks for.
        let continues = match decaying.action() {
            SceneCutAction::Tolerate => StepEffect::Continue,
            SceneCutAction::DecayModel => StepEffect::Continue,
        };
        assert_eq!(continues, StepEffect::Continue);

        // Exit condition ③ (`StopReason` has 11 variants) is not checkable in this file today: the
        // scroll `StopReason` does not exist yet — it lands with `P3.07`/`P3.09` (DEV-21). What is
        // checkable now is the claim it stands for, which the `match` above states.
    }

    #[test]
    fn a_candidate_that_cannot_be_measured_and_a_blank_page_are_both_not_scene_cuts() {
        // The two ways to make `is_scene_cut` true by accident, both of which would turn "I cannot
        // tell" into "the page changed" — the one direction §16.8 must never err in, because
        // `scene_cut ×≥3` decays the model.
        let image = mixed_document();
        let other = unrelated_document();
        let mut first = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let mut second = ScrollScript::new(&other, 900, vec![StepSpec::move_by(120)]);
        let previous = first.take(0);
        let current = second.take(0);

        // 1. `∀ i` over an empty set is vacuously true. A frame whose candidates were all dropped is
        //    not evidence that the page changed; it is the absence of evidence, and §16.10 has a
        //    state for that (`None`).
        assert!(
            !is_scene_cut_once(&previous, &current, &ScoredSet::new()),
            "an empty candidate set was read as a scene cut"
        );

        // 2. A blank page. `band_zncc` returns `0.0` when a band has no variance, which is below
        //    §16.8's 0.50 similarity floor — so the first condition holds on a solid colour — and the
        //    reference implementation's `(1.0, 1.0)` fallback for "no samples" (`estimator.rs:570`)
        //    would then report the *worst possible* alignment error for a page with nothing in it.
        //    Ours cannot: a tile with no variance carries no alignment evidence, so `alignment_error`
        //    is `None`, and an unmeasurable candidate blocks the claim (DEV-21).
        let flat = TestImage::from_structures(320, 80 * 19, 3, 80 * 19, &[Structure::Flat]);
        let mut script = ScrollScript::new(&flat, 300, vec![StepSpec::move_by(10)]);
        let flat_previous = script.take(0);
        let flat_current = script.take(1);
        assert_eq!(
            zero_shift_similarity(&flat_previous.view(), &flat_current.view()),
            0.0,
            "a solid page was expected to have no corr in either direction"
        );
        for shift in [0, 10, 20, -10] {
            assert_eq!(
                alignment_error(&flat_previous.view(), &flat_current.view(), shift),
                None,
                "a blank page produced an alignment error at {shift}"
            );
        }
        let candidates = candidates_1d(&flat_previous.view(), &flat_current.view(), 10, 8);
        let scored = scored_once(&flat_previous, &flat_current, &candidates);
        assert!(
            !is_scene_cut_once(&flat_previous, &flat_current, &scored),
            "a blank page was read as a scene cut"
        );
    }

    #[test]
    fn a_page_that_did_move_is_not_a_scene_cut() {
        // The other side of the same boundary: a real step is not "the page changed". The alignment
        // error at the true shift is ~0, which is what makes `alignment_error` a measurement of this
        // step rather than a fixed number.
        let image = mixed_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);

        let aligned = alignment_error(&previous.view(), &current.view(), 120)
            .expect("both frames have structure");
        assert!(
            aligned <= SCENE_CUT_ALIGNMENT_ERROR,
            "the true shift aligned at error {aligned}"
        );

        // Measured 2026-10-08: the true shift gives `0.0`, a 4 px miss gives `0.40313148` (116) and
        // `0.3820177` (124), and unrelated shifts give `0.9353355` (0) and `1.0045342` (60). Two
        // things follow. First, the number is `1 − ZNCC` and therefore lives in `[0, 2]`, not in
        // `[0, 1]` — a threshold like §16.8's `0.60` is a *position on that scale*, not a fraction
        // of the band. Second, `zero_shift_similarity` on this frame is `−0.014104286`: **below**
        // §16.8's similarity floor. So for any page that actually moved, the first condition holds
        // and the second is what keeps a real step out of the scene cut — the first is a cheap veto
        // for a page that did *not* change (the next test), not the discriminating one.
        assert!(aligned.abs() < 1e-6, "the true shift should align exactly: {aligned}");

        let candidates = candidates_1d(&previous.view(), &current.view(), 120, 40);
        let scored = scored_once(&previous, &current, &candidates);
        assert!(
            !is_scene_cut_once(&previous, &current, &scored),
            "a document that scrolled 120 px was read as a scene cut"
        );
    }

    #[test]
    fn a_frame_that_did_not_change_is_not_a_scene_cut() {
        // What §16.8's first condition is for. A viewport that was re-rendered without scrolling is
        // identical at zero shift, so the frame is not "a different page" and the expensive second
        // condition never runs — which is the whole reason the cheap question is asked first.
        let image = mixed_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(0)]);
        let previous = script.take(0);
        let current = script.take(1);

        let similarity = zero_shift_similarity(&previous.view(), &current.view());
        assert!(
            similarity >= SCENE_CUT_SIMILARITY,
            "two identical frames measured {similarity} at zero shift"
        );

        let candidates = candidates_1d(&previous.view(), &current.view(), 0, 40);
        let scored = scored_once(&previous, &current, &candidates);
        assert!(
            !is_scene_cut_once(&previous, &current, &scored),
            "a page that did not change was read as a scene cut"
        );
    }

    // ---- P1.13: the scratch, and the ablation that decides whether each gate earns its place -----

    /// Which of §16.1's four gates the funnel is allowed to consult.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct GateMask {
        geometry: bool,
        gain: bool,
        support: bool,
        margin: bool,
    }

    impl GateMask {
        const ALL: Self = Self {
            geometry: true,
            gain: true,
            support: true,
            margin: true,
        };

        fn without(self, gate: Gate) -> Self {
            match gate {
                Gate::Geometry => Self {
                    geometry: false,
                    ..self
                },
                Gate::Gain => Self { gain: false, ..self },
                Gate::Support => Self {
                    support: false,
                    ..self
                },
                Gate::Margin => Self {
                    margin: false,
                    ..self
                },
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Gate {
        Geometry,
        Gain,
        Support,
        Margin,
    }

    impl Gate {
        const ALL: [Self; 4] = [Self::Geometry, Self::Gain, Self::Support, Self::Margin];

        fn name(self) -> &'static str {
            match self {
                Self::Geometry => "geometry",
                Self::Gain => "gain",
                Self::Support => "support",
                Self::Margin => "margin",
            }
        }
    }

    /// The search half-width the ablation runs with: the manual-mode degradation of §16.6, which is
    /// the wider of the two, so a gate is never let off by a narrow window.
    const ABLATION_WINDOW: i32 = 40;

    /// §16.1's funnel with the gates switched, so one of them can be removed and the answer watched.
    ///
    /// It is a *test-only* composition on purpose: the production entry point that chains the layers
    /// and the gates belongs to the session assembly, which does not exist yet. What this harness
    /// has to be is faithful, which took one measurement to learn: **gate one judges the third
    /// layer's answer, and the other three judge the candidate the second layer measured.** Geometry
    /// is a statement about the number the canvas would act on, and `mixed-450` proved the two can
    /// differ — the ranking's winner was 449 while the refined answer was 450, §16.9's banned
    /// boundary value, so a geometry gate reading `best.d` would confirm a shift the design forbids.
    /// Gain, support and margin are per-candidate measurements (`P1.09`/`P1.10`/`P1.11`) and have no
    /// meaning at the refined pixel, so they keep reading `best`.
    ///
    /// It does **not** include §16.8's scene cut: that is a statement about the page, not one of the
    /// four gates, and the ablation would read a scene cut's refusal as the gates' work.
    fn decide(
        scratch: &mut Scratch,
        previous: &ObservationView<'_>,
        current: &ObservationView<'_>,
        expected: i32,
        window: i32,
        mask: GateMask,
    ) -> Status {
        let candidates = candidates_1d(previous, current, expected, window);
        if candidates.is_empty() {
            return Status::None;
        }
        let scored = {
            let views = scratch.pool(previous, current);
            score_candidates_2d(views.previous(), views.current(), &candidates)
        };
        let Some(best) = scored.iter().next().copied() else {
            return Status::None;
        };
        let extent = previous.primary_extent();
        let answer = {
            let views = scratch.full_resolution(previous, current);
            refine_winner(views.previous(), views.current(), &scored)
        }
        .map(|refined| refined.d)
        .unwrap_or(best.d);

        if mask.geometry && gate_geometry(answer, extent) != GateOutcome::Pass {
            return Status::None;
        }
        if mask.gain && gate_residual_gain(best.gain) != GateOutcome::Pass {
            return Status::None;
        }
        if mask.support && gate_support(best.tiles) != GateOutcome::Pass {
            return Status::None;
        }
        if mask.margin {
            // The rival has to come from **outside the winner's cell**: the second layer measures one
            // value per 4 px cell (`P1.06`), so two members of the same cell share their score
            // exactly and reading one of them as a rival reports the grid's resolution as ambiguity.
            // Which member of the cell wins is the first layer's `support`, which `P1.07` made the
            // ranking's tie-break; the ambiguity gate asks about the *page*, not about the grid.
            let rival = outside_cell_second(&scored, best.d).map(|second| second.score);
            if let GateOutcome::Reject(rejection) = gate_margin(best.d, margin_of(best.score, rival)) {
                // A rejection carries the status of the gate that produced it (`P1.11`), and its
                // ambiguity arm names the candidate the gate judged — the ranking's winner. What the
                // funnel would *report* is the third layer's answer, and §16.10's `Uncertain` is "the
                // shift we would have used, untrusted": those are different questions and they differ
                // by at most a pixel, so the reported number is the refined one.
                return match rejection.status() {
                    Status::Uncertain { .. } => Status::Uncertain { d: answer },
                    other => other,
                };
            }
        }
        // The number that gets confirmed is the number that is checked: gate one judges `answer`
        // (§16.2.1), and so must §16.2.1's overlap ratio, or a refine that walked to the frame's own
        // edge would be confirmed on the strength of the grid point it walked away from.
        if is_verifiable(answer, extent) {
            Status::Confirmed { d: answer }
        } else {
            Status::Uncertain { d: answer }
        }
    }

    /// The strongest candidate that is **not** in the winner's cell — the only rival the second
    /// layer can actually distinguish.
    fn outside_cell_second(scored: &ScoredSet, winner: i32) -> Option<ScoredCandidate> {
        let cell = round_to_grid(winner);
        scored
            .iter()
            .find(|candidate| round_to_grid(candidate.d) != cell)
            .copied()
    }

    /// One synthetic step the funnel is asked about, with the answer a session would need.
    struct AblationCase {
        name: &'static str,
        image: TestImage,
        viewport: u32,
        step: i32,
        horizontal: bool,
    }

    fn mixed_structures() -> [Structure; 5] {
        [
            Structure::Checker { cell: 12 },
            Structure::TextRows { line: 19 },
            Structure::NoiseBlocks { cell: 8 },
            Structure::Gradient,
            Structure::HorizontalBars { period: 19 },
        ]
    }

    /// The corpus the ablation runs on: every structure the fixture can build, both axes, steps from
    /// one pixel to past what the viewport can support, and one step *at* §16.9's boundary value.
    fn ablation_cases() -> Vec<AblationCase> {
        // 100 bands tall, not 60: the corpus asks about steps up to 890 px, and a vertical script
        // must keep `offset + viewport` inside the document (`testkit.rs:359`).
        let mixed = || TestImage::from_structures(640, 100 * 19, 11, 19, &mixed_structures());
        let step = |name, step| AblationCase {
            name,
            image: mixed(),
            viewport: 900,
            step,
            horizontal: false,
        };
        let mut cases = vec![
            step("mixed-7", 7),
            step("mixed-37", 37),
            step("mixed-120", 120),
            step("mixed-240", 240),
            step("mixed-450", 450),
            step("mixed-600", 600),
            step("mixed-890", 890),
            // §16.4.1's floor: a 300 px viewport yields 4 pooled tiles, so at most 2 independent
            // ones — gate three cannot pass, whatever the page says. This is the case that makes gate
            // three load-bearing on its own: the step is 120 and the match is exact (`gain` 1.0), so
            // nothing else stands between it and a Confirmation.
            AblationCase {
                name: "small-120",
                image: mixed(),
                viewport: 300,
                step: 120,
                horizontal: false,
            },
            // §16.9's boundary value: answerable by overlap, forbidden as an answer.
            AblationCase {
                name: "mixed-h-120",
                image: mixed(),
                viewport: 900,
                step: 120,
                horizontal: true,
            },
        ];
        cases.push(AblationCase {
            name: "text-19",
            image: TestImage::from_structures(640, 60 * 19, 5, 19, &[Structure::TextRows { line: 19 }]),
            viewport: 900,
            step: 19,
            horizontal: false,
        });
        cases.push(AblationCase {
            name: "text-38",
            image: TestImage::from_structures(640, 60 * 19, 5, 19, &[Structure::TextRows { line: 19 }]),
            viewport: 900,
            step: 38,
            horizontal: false,
        });
        cases.push(AblationCase {
            name: "checker-24",
            image: TestImage::from_structures(640, 60 * 19, 3, 19, &[Structure::Checker { cell: 12 }]),
            viewport: 900,
            step: 24,
            horizontal: false,
        });
        cases.push(AblationCase {
            name: "checker-120",
            image: TestImage::from_structures(640, 60 * 19, 3, 19, &[Structure::Checker { cell: 12 }]),
            viewport: 900,
            step: 120,
            horizontal: false,
        });
        cases.push(AblationCase {
            name: "noise-4",
            image: TestImage::from_structures(640, 60 * 19, 4, 19, &[Structure::NoiseBlocks { cell: 8 }]),
            viewport: 900,
            step: 4,
            horizontal: false,
        });
        cases.push(AblationCase {
            name: "noise-120",
            image: TestImage::from_structures(640, 60 * 19, 4, 19, &[Structure::NoiseBlocks { cell: 8 }]),
            viewport: 900,
            step: 120,
            horizontal: false,
        });
        cases.push(AblationCase {
            name: "gradient-10",
            image: TestImage::from_structures(640, 60 * 19, 6, 19, &[Structure::Gradient]),
            viewport: 900,
            step: 10,
            horizontal: false,
        });
        cases.push(AblationCase {
            name: "flat-10",
            image: TestImage::from_structures(640, 60 * 19, 6, 60 * 19, &[Structure::Flat]),
            viewport: 900,
            step: 10,
            horizontal: false,
        });
        cases
    }

    /// Whether a session is *entitled* to an answer for this step. Three of the design's own
    /// prohibitions cap it, and a confirmation outside them is a wrong answer however close to the
    /// true step it lands:
    ///
    /// - §16.2.1's overlap ratio (`is_verifiable`);
    /// - §16.9's boundary value, `|d| == N/2`;
    /// - §16.4.1's tile floor: gate three needs `MIN_TILES` independent tiles two apart, which a
    ///   viewport shorter than ~448 px cannot supply. A viewport that small must not be answered
    ///   from, so an answer from it is wrong even when the step it names is the true one.
    fn answerable(step: i32, extent: u32) -> bool {
        is_verifiable(step, extent)
            && !(extent >= 2 && step.unsigned_abs() == extent / 2)
            && enough_tiles_for_gate_three(extent)
    }

    /// §16.4.1's floor expressed through the mechanism rather than as the 448 px it works out to: the
    /// pooled band is `match_rows(extent, extent, DOWNSAMPLE)` rows tall, a tile is
    /// `TILE_ROWS / DOWNSAMPLE` of them, and gate three wants `MIN_TILES` tiles with a gap of
    /// `TILE_INDEPENDENCE_GAP` between them.
    fn enough_tiles_for_gate_three(extent: u32) -> bool {
        let rows = match_rows(extent, extent, DOWNSAMPLE);
        let tiles = rows / (TILE_ROWS / DOWNSAMPLE);
        independent_support(0..tiles) >= MIN_TILES
    }

    /// The funnel's answer for every case under one gate mask.
    fn ablate(mask: GateMask) -> Vec<Status> {
        let mut scratch = Scratch::new();
        let cases = ablation_cases();
        cases
            .iter()
            .map(|case| {
                let steps = vec![StepSpec::move_by(case.step)];
                let mut script = if case.horizontal {
                    ScrollScript::horizontal(&case.image, case.viewport, steps)
                } else {
                    ScrollScript::new(&case.image, case.viewport, steps)
                };
                let previous = script.take(0);
                let current = script.take(1);
                decide(
                    &mut scratch,
                    &previous.view(),
                    &current.view(),
                    case.step,
                    ABLATION_WINDOW,
                    mask,
                )
            })
            .collect()
    }

    /// `(wrong confirmations, refusals of answerable cases)` for one mask.
    fn tally(cases: &[AblationCase], answers: &[Status]) -> (usize, usize) {
        let mut wrong = 0;
        let mut refused = 0;
        for (case, answer) in cases.iter().zip(answers) {
            let entitled = answerable(case.step, case.viewport);
            match answer {
                Status::Confirmed { d } => {
                    if !(entitled && d.abs_diff(case.step) <= 1) {
                        wrong += 1;
                    }
                }
                Status::Uncertain { .. } | Status::None => {
                    if entitled {
                        refused += 1;
                    }
                }
            }
        }
        (wrong, refused)
    }

    #[test]
    fn closing_any_gate_changes_the_error_rate() {
        // §16.12: close each gate in turn and watch what happens to the answers. The metric is
        // "wrong and Confirmed" — a confirmation of a shift the design does not entitle a session to
        // (`answerable`) or one more than a pixel off the truth — plus the refusals, so that a gate
        // which only trades refusals for refusals cannot look load-bearing.
        let cases = ablation_cases();
        let baseline = ablate(GateMask::ALL);
        let (baseline_wrong, baseline_refused) = tally(&cases, &baseline);

        // §16.12's exit condition ② is that every row can say *what* went wrong once its gate is
        // closed, and "what" is a per-case claim. So the baseline is printed case by case, with the
        // entitlement the metric uses, rather than only as the three numbers.
        let mut detail = String::from("case         step  extent  entitled  status\n");
        for (case, status) in cases.iter().zip(&baseline) {
            detail.push_str(&format!(
                "{:<12} {:<5} {:<7} {:<9} {status:?}\n",
                case.name,
                case.step,
                case.viewport,
                answerable(case.step, case.viewport),
            ));
        }

        let mut table = format!(
            "{:<10} {:>6} {:>8} {:>7}   cases that changed\n",
            "closed", "wrong", "refused", "delta"
        );
        let mut rows: Vec<(Gate, usize, Vec<String>)> = Vec::new();
        for gate in Gate::ALL {
            let answers = ablate(GateMask::ALL.without(gate));
            let (wrong, refused) = tally(&cases, &answers);
            let changed: Vec<String> = cases
                .iter()
                .zip(&answers)
                .zip(&baseline)
                .filter(|((_, now), before)| now != before)
                .map(|((case, now), before)| format!("{} {before:?} -> {now:?}", case.name))
                .collect();
            table.push_str(&format!(
                "{:<10} {wrong:>6} {refused:>8} {:>7}   {}\n",
                gate.name(),
                format!("{:+}", wrong as i32 - baseline_wrong as i32),
                changed.join("; ")
            ));
            rows.push((gate, wrong, changed));
        }
        let pair = ablate(GateMask::ALL.without(Gate::Gain).without(Gate::Support));
        let (pair_wrong, _) = tally(&cases, &pair);
        table.push_str(&format!(
            "{:<10} {pair_wrong:>6} {:>8} {:>7}   gain+support\n",
            "gain+sup",
            "-",
            format!("{:+}", pair_wrong as i32 - baseline_wrong as i32),
        ));
        table.push_str(&format!(
            "baseline: {baseline_wrong} wrong, {baseline_refused} refused, {} cases\n",
            cases.len()
        ));
        println!("{table}{detail}");

        assert_eq!(
            baseline_wrong, 0,
            "the four gates together confirm a wrong shift on their own corpus:\n{table}{detail}"
        );

        // A gate that raises the wrong count earns its place. One that does not must still *change*
        // something (otherwise it is inert and should be deleted) and must carry its reason here —
        // the reason is the deliverable §16.12 asks for, so it is asserted, not merely written down.
        const MASKED: [(Gate, &str); 3] = [
            (
                Gate::Geometry,
                "mixed-450 goes None -> Uncertain{d:449}: gate one is the only gate that sees the \
                 number the third layer actually produced (the ranking's winner was 449, the refined \
                 answer the banned boundary 450), and it is a correctness constraint from §16.2 — \
                 `|d| > extent` is not evidence about the page.",
            ),
            (
                Gate::Gain,
                "text-19 and text-38 go None -> Uncertain{d:0}: without gate two a band that cannot \
                 be measured reaches the ambiguity gate, which then hands back a candidate whose shift \
                 is zero on a page that moved 19 px. On flat-10 it is masked by gate three — see the \
                 gain+sup row, which is where the pair earns its place.",
            ),
            (
                Gate::Support,
                "this is the one gate whose removal makes the funnel *more* willing to answer: the \
                 refused count falls 9 -> 8 because small-120 (an entitled case) goes None -> \
                 Confirmed{d:120}, its true step, while mixed-890 flips only inside the states that are \
                 not entitled anyway. It stays because the question it decides is not \"is this number \
                 right\" but \"is this evidence about the page or about one blob\" (§16.4): \
                 `MIN_TILES` independent tiles two apart, which §16.4.1 caps at a viewport of about \
                 448 px. `wrong` cannot see the difference — a commit from a single patch that happens \
                 to be right is still a commit §16.4 forbids — so its retention reason is the mechanism \
                 plus the refusal it makes, and this is the honest limit of the metric: gate four is \
                 the only gate that moves `wrong` on this corpus.",
            ),
        ];
        for (gate, wrong, changed) in &rows {
            assert!(
                *wrong >= baseline_wrong,
                "closing gate {} made the funnel *less* wrong-confirming, which means it is not a \
                 filter at all:\n{table}{detail}",
                gate.name()
            );
            assert!(
                !changed.is_empty(),
                "closing gate {} changed no case in the corpus: it is inert, so delete it or give it \
                 a case that exercises it:\n{table}{detail}",
                gate.name()
            );
            if *wrong == baseline_wrong {
                assert!(
                    MASKED.iter().any(|(masked, _)| masked == gate),
                    "gate {} raises no wrong confirmation and carries no reason to stay:\n{table}{detail}",
                    gate.name()
                );
            }
        }
        assert!(
            pair_wrong > baseline_wrong,
            "gate two and gate three are individually masked by each other, so the pair has to do the \
             work — closing both must let a wrong confirmation out:\n{table}{detail}"
        );
    }

    #[test]
    fn the_scratch_reuses_its_buffers_across_steps() {
        let image = mixed_document();
        let mut script = ScrollScript::new(&image, 900, vec![StepSpec::move_by(120)]);
        let previous = script.take(0);
        let current = script.take(1);
        let mut scratch = Scratch::new();

        let first = {
            let views = scratch.pool(&previous.view(), &current.view());
            (
                views.previous().data.as_ptr(),
                views.current().data.as_ptr(),
                views.previous().data.clone(),
            )
        };
        assert_eq!(scratch.builds(), 2, "one pooled image per frame");
        assert_eq!(
            first.2,
            Gray::pooled(&previous.view()).data,
            "the pooled image does not hold the frame it was pooled from"
        );

        let second = {
            let views = scratch.pool(&previous.view(), &current.view());
            (
                views.previous().data.as_ptr(),
                views.current().data.as_ptr(),
            )
        };
        assert_eq!(
            second,
            (first.0, first.1),
            "the pooled buffers were reallocated between two steps"
        );
        assert_eq!(
            scratch.builds(),
            4,
            "a second call re-pools: there is no cache key to hide behind, because an address, a \
             length and a timestamp are not an identity (see the note on `Scratch`)"
        );

        let _ = scratch.full_resolution(&previous.view(), &current.view());
        assert_eq!(
            scratch.builds(),
            6,
            "layer three reads the same two frames at scale one"
        );

        // The next step's frames are different ones and the buffers are still the same allocations: a
        // session's memory is bounded by its first step, not by the length of the scroll (§22.5's
        // reason to have a `Scratch` at all). Reuse is only useful if the reused buffer holds the
        // *new* frame, so that is asserted against a fresh pooling rather than trusted.
        let mut next = ScrollScript::new(&image, 900, vec![StepSpec::move_by(80)]);
        let first_next = next.take(0);
        let second_next = next.take(1);
        let (reused, contents) = {
            let views = scratch.pool(&first_next.view(), &second_next.view());
            (
                (views.previous().data.as_ptr(), views.current().data.as_ptr()),
                views.previous().data.clone(),
            )
        };
        assert_eq!(
            reused,
            (first.0, first.1),
            "the pooled buffer was reallocated instead of reused"
        );
        assert_eq!(
            contents,
            Gray::pooled(&first_next.view()).data,
            "the reused buffer still holds the previous step's pixels"
        );
        assert_eq!(scratch.builds(), 8);
    }
}
