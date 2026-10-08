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
//! `StopReason`) and the `ĝ` prior (`P1.08`); both feed the `score` this file's formulas name.

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
    /// Full-resolution ZNCC of the winning candidate (`docs/30` §16.7; filled by `P1.06`).
    pub(crate) zncc2d: f32,
    /// Residual gain over "nothing moved" (`docs/30` §16.4, gate three; filled by `P1.05`).
    pub(crate) gain: f32,
    /// `(score(best) − score(second)) / score(best)` (`docs/30` §16.5, gate four; `P1.05`).
    pub(crate) margin: f32,
    /// How many independent bands support the winner (`docs/30` §16.3/§16.11).
    pub(crate) tiles: u32,
}

impl Evidence {
    /// The share of the band budget the winner used, saturating at
    /// [`COVERAGE_SATURATION_TILES`] (`docs/30` §16.7).
    pub(crate) fn coverage(&self) -> f32 {
        (self.tiles as f32 / COVERAGE_SATURATION_TILES).min(1.0)
    }

    /// `docs/30` §16.7 — used for ranking, for gate four's `margin`, and as the comparison basis
    /// for the ORB second opinion (`P1.23`).
    pub(crate) fn score(&self) -> f32 {
        SCORE_ZNCC2D * self.zncc2d + SCORE_GAIN * self.gain + SCORE_COVERAGE * self.coverage()
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

#[cfg(test)]
mod tests {
    use super::{
        CANDIDATE_LIMIT, Candidate, Displacement, Evidence, Status, StepEffect, candidates_1d,
        primary_digests, support_at,
    };
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
}
