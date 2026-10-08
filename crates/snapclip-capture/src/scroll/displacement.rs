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

#[cfg(test)]
mod tests {
    use super::{Displacement, Evidence, Status, StepEffect};

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
}
