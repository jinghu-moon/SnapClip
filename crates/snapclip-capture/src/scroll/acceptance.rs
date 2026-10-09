//! `E-ACC-1` as a gate: the acceptance scan (task `P1.24`).
//!
//! The scan is the machine-checkable form of G1, the only zero-tolerance rule in the design:
//!
//! > the rate of **wrong and `Confirmed`** must be zero.
//!
//! Everything else is allowed to be a refusal. `Uncertain`, `None` and a scene cut all end in "no
//! write", which is a page the user can scroll again; a wrong `Confirmed` writes wrong pixels into a
//! canvas the user is told is correct.
//!
//! **The judgement is byte equality with the generator's truth** (`docs/30` §29.3): the fixture is a
//! pure function of its seed, and the script says which document rows each frame shows, so the
//! expected canvas is the document itself. No baseline images, no `insta`, and therefore no "update
//! the baseline" step that could freeze a bug in place.
//!
//! The corpus is `docs/30` §29.3's scan grid, narrowed to the dimensions the layers can actually see
//! today: `|d|` (both signs), the period ratio `P/|d|` (the alias generator), the dynamic share and
//! the noise amplitude. `P` is the period of the document's periodic band; every case is a page that
//! alternates a periodic band with a noise band, which is what makes a *correct* answer possible
//! while an *alias* is still available.
//!
//! Two layers of corpus, per the task's REFACTOR clause: [`smoke_corpus`] runs in every `cargo test`
//! (the gate has to be on the default path or it gets skipped), and [`full_corpus`] is `#[ignore]`d
//! because 5,376 cases is a release-mode, minutes-long job.
//!
//! This file is test-only. The production entry point that chains the layers and the gates is the
//! session assembly (`P3.09`), which does not exist yet — which is also why [`decide`] here is a
//! second assembly next to `P1.13`'s ablation harness. They answer different questions (which gate
//! earns its place vs. does the whole funnel ever lie), and the two helpers they *share* —
//! `margin_of` and `outside_cell_second` — were promoted to the production half of `displacement.rs`
//! rather than copied, so §16.5's rival rule has one implementation.

use crate::scroll::canvas::{MemoryBudget, RecoveredImage, ViewportState};
use crate::scroll::displacement::{
    GateOutcome, Scratch, Status, candidates_1d, gate_geometry, gate_margin, gate_residual_gain,
    gate_support, has_peak_family, is_verifiable, manual_window, margin_of, match_rows,
    outside_cell_second, refine_winner, score_candidates_2d,
};
use crate::scroll::observation::{Axis, Observation, ObservationView};
use crate::scroll::session::ScrollSession;
use crate::scroll::testkit::{ScrollScript, StepSpec, Structure, TestImage};

/// The synthetic page width. Narrow on purpose: every scan case costs a whole document, and the
/// vertical axis is where the evidence lives.
const SCAN_WIDTH: u32 = 320;

/// §29.3's viewport. 900 px is above §16.4.1's 448 px floor for gate three, so a case is never
/// refused for being too small to have independent supporters.
const SCAN_VIEWPORT: u32 = 900;

/// Document rows above the first viewport. It has to exceed the largest `|d|` in the corpus so a
/// negative step still has document to scroll back into.
const SCAN_MARGIN: u32 = 700;

/// The cell of the aperiodic band. `P1.23` measured 8 as the cell that gives the descriptor matcher
/// something to match; the estimator's layers want the same thing.
const NOISE_CELL: u32 = 8;

/// §15.6's search window: `max(4, ceil(0.3 · n · ĝ))`, with `n · ĝ` = the expected step.
///
/// The scan hands the funnel a **correct** expectation (`expected = truth`). That is deliberate: the
/// prior's accuracy is `P1.14`'s subject, and what this gate asks is whether the funnel, given a
/// search window that contains the answer, ever confirms something else. The window still reaches
/// the aliases — at `P/|d| = 1` and `2` the alias is inside `0.3 · |d|` — which is the point of
/// scanning the period ratio at all.
fn search_window(shift: i32) -> i32 {
    let scaled = (shift.unsigned_abs() as f32) * 0.3;
    (scaled.ceil() as i32).max(4)
}

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

    const NONE: Self = Self {
        geometry: false,
        gain: false,
        support: false,
        margin: false,
    };
}

/// How much of §15.4's funnel runs. `FIRST_ONLY` is not a degraded mode anyone would ship; it is the
/// control that proves the scan can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Funnel {
    /// Layer 2 (the 4× pooled two-dimensional ZNCC) is allowed to measure the candidates.
    two_d: bool,
    /// Layer 3 (full-resolution ±1 refinement) is allowed to move the winner.
    refine: bool,
    gates: GateMask,
}

impl Funnel {
    const FULL: Self = Self {
        two_d: true,
        refine: true,
        gates: GateMask::ALL,
    };

    /// Layer 1's candidate set, its support-ranked winner, and nothing else.
    ///
    /// It keeps `is_verifiable` — §16.2's overlap ratio is part of what "this answer is usable" means
    /// rather than a gate — but consults none of the four gates.
    ///
    /// This used to be the control that proved the scan could fail: the ranking's tie-break compared
    /// `|d|` against zero, so on a page whose period divides the step the zero shift won on support
    /// ties and the answer was confidently wrong. `P1.24` changed that key to `|d − expected|` (and
    /// the corpus's aliases are outside the search window's centre), so layer 1 alone is now *exact*
    /// on this corpus and the control has to break a later layer — see
    /// `the_scan_catches_a_funnel_that_lost_the_last_layer`, which asserts both facts.
    const FIRST_ONLY: Self = Self {
        two_d: false,
        refine: false,
        gates: GateMask::NONE,
    };
}

/// §16.1's funnel, assembled for the scan.
///
/// The gate order and the split between "judge the refined answer" (gate one, §16.2.1) and "judge
/// the measured candidate" (gates two, three, four) follow `P1.13`'s ablation harness; the reasons
/// are recorded there.
fn decide(
    scratch: &mut Scratch,
    previous: &ObservationView<'_>,
    current: &ObservationView<'_>,
    expected: i32,
    window: i32,
    funnel: Funnel,
) -> Status {
    let candidates = candidates_1d(previous, current, expected, window);
    if candidates.is_empty() {
        return Status::None;
    }
    let extent = previous.primary_extent();

    if !funnel.two_d {
        let best = candidates
            .iter()
            .next()
            .expect("the set was just checked to be non-empty");
        return if is_verifiable(best.d, extent) {
            Status::Confirmed { d: best.d }
        } else {
            Status::Uncertain { d: best.d }
        };
    }

    let scored = {
        let views = scratch.pool(previous, current);
        score_candidates_2d(views.previous(), views.current(), &candidates)
    };
    let Some(best) = scored.iter().next().copied() else {
        return Status::None;
    };
    let answer = if funnel.refine {
        let views = scratch.full_resolution(previous, current);
        refine_winner(views.previous(), views.current(), &scored)
            .map(|refined| refined.d)
            .unwrap_or(best.d)
    } else {
        best.d
    };

    if funnel.gates.geometry && gate_geometry(answer, extent) != GateOutcome::Pass {
        return Status::None;
    }
    if funnel.gates.gain && gate_residual_gain(best.gain) != GateOutcome::Pass {
        return Status::None;
    }
    if funnel.gates.support && gate_support(best.tiles) != GateOutcome::Pass {
        return Status::None;
    }
    if funnel.gates.margin {
        let rival = outside_cell_second(&scored, best.d).map(|second| second.ranked());
        if let GateOutcome::Reject(rejection) =
            gate_margin(best.d, margin_of(best.ranked(), rival))
        {
            return match rejection.status() {
                Status::Uncertain { .. } => Status::Uncertain { d: answer },
                other => other,
            };
        }
    }
    if is_verifiable(answer, extent) {
        Status::Confirmed { d: answer }
    } else {
        Status::Uncertain { d: answer }
    }
}

/// §16.6's manual route: no prior, the search centred on zero, and a peak family answered
/// `Uncertain` before the gates get a say.
///
/// The scan keeps its own copy of this for the same reason it keeps its own [`decide`]: a harness
/// that calls the production entry point cannot be run against a deliberately wrong configuration,
/// and the family check has to be *on the path* or it is never exercised. The production entry point
/// is `estimate()` with `Prior::None` (§27.3), which `P3.09` assembles.
///
/// The family outranks gates two, three and four on purpose. Those gates ask whether *this* candidate
/// is good enough; the family says the page cannot single one out, which is a different question and
/// the one §16.6 answers directly. Gate one is not outranked — geometry is a correctness constraint
/// on the number itself (§16.2), and an ambiguity is no reason to report a shift the canvas must not
/// act on.
fn manual_status(
    scratch: &mut Scratch,
    previous: &ObservationView<'_>,
    current: &ObservationView<'_>,
) -> Status {
    let extent = previous.primary_extent();
    let window = manual_window(match_rows(extent, extent, 1));
    let candidates = candidates_1d(previous, current, 0, window);
    if candidates.is_empty() {
        return Status::None;
    }
    let scored = {
        let views = scratch.pool(previous, current);
        score_candidates_2d(views.previous(), views.current(), &candidates)
    };
    if has_peak_family(&scored) {
        let answer = {
            let views = scratch.full_resolution(previous, current);
            refine_winner(views.previous(), views.current(), &scored)
        }
        .map(|refined| refined.d)
        .unwrap_or_else(|| {
            scored
                .iter()
                .next()
                .expect("a family needs candidates")
                .d
        });
        return if gate_geometry(answer, extent) == GateOutcome::Pass {
            Status::Uncertain { d: answer }
        } else {
            Status::None
        };
    }
    decide(scratch, previous, current, 0, window, Funnel::FULL)
}

/// One scan case: a page, a viewport, and one scripted step with known truth.
struct Case {
    name: String,
    image: TestImage,
    /// Document row the first viewport's top sits at.
    start: u32,
    spec: StepSpec,
}

impl Case {
    fn truth(&self) -> i32 {
        self.spec.delta
    }

    /// The frame noise the fixture injects. It is the tolerance the byte comparison gets: a captured
    /// frame with noise writes noisy pixels, and demanding byte equality with a noise-free document
    /// would demand that the write path invent the clean version. What the comparison still catches is
    /// anything a wrong offset produces, which is far larger than `sigma`.
    fn noise(&self) -> u8 {
        self.spec.noise
    }
}

/// The page every case is built on: a periodic band of period `P`, then a noise band, repeating.
///
/// The period is the alias generator; the noise band is what makes a correct answer possible at all.
/// `band_height = max(P, 64)` is forced by the fixture's own definition — `TestImage` evaluates
/// `y % band_height` (`testkit.rs:179`), so a period larger than the band would be invisible.
fn scan_case(shift: i32, ratio: f32, dynamic: f32, noise: u8, seed: u32) -> Case {
    let magnitude = shift.unsigned_abs();
    let period = ((magnitude as f32) * ratio).round().max(2.0) as u32;
    let band = period.max(64);
    let height = SCAN_MARGIN + SCAN_VIEWPORT + magnitude.max(0) + SCAN_MARGIN;
    let image = TestImage::from_structures(
        SCAN_WIDTH,
        height,
        seed,
        band,
        &[
            Structure::HorizontalBars { period },
            Structure::NoiseBlocks { cell: NOISE_CELL },
        ],
    );

    // §18.2's moving region is anchored at the **top** of the frame (`overlay_moving_region`), and
    // that is deliberate: the region model measures the match band, which is the *bottom* of the
    // overlap, so a moving region there is invisible to the matcher. A **prepend** writes the frame's
    // leading rows — exactly that region. With a moving overlay, a prepend's bytes therefore cannot
    // equal the document by construction, and asserting that they do would demand that the write path
    // recover pixels the fixture replaced on purpose. What stops those rows from being written is the
    // time model's tile skip (§18.2, `P2`/`P3`), which does not exist yet; so the corpus paints no
    // overlay on a step that scrolls up, and `a_moving_region_over_the_revealed_rows_is_written_verbatim`
    // pins today's behaviour instead of hiding it.
    let painted = if shift > 0 { dynamic } else { 0.0 };

    Case {
        name: format!("d={shift} P/|d|={ratio} dyn={painted} sigma={noise}"),
        image,
        start: SCAN_MARGIN,
        spec: StepSpec::move_by(shift)
            .with_dynamic(painted)
            .with_noise(noise),
    }
}

/// The scan grid, restricted to the parameter lists given.
fn corpus(shifts: &[i32], ratios: &[f32], dynamics: &[f32], noises: &[u8]) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut seed = 21u32;
    for &shift in shifts {
        for &ratio in ratios {
            for &dynamic in dynamics {
                for &noise in noises {
                    cases.push(scan_case(shift, ratio, dynamic, noise, seed));
                    seed = seed.wrapping_add(1);
                }
            }
        }
    }
    cases
}

/// The subset that runs on every `cargo test`: seconds, and it still contains both signs, all four
/// period ratios, a moving region and noise.
fn smoke_corpus() -> Vec<Case> {
    let mut cases = corpus(&[7, 40, 120, -7, -40, -120], &[0.5, 1.0, 2.0, 4.0], &[0.0], &[0]);
    // The noisy half is a second block rather than a cross product: every combination of noise with
    // every ratio would multiply the smoke subset by four for a dimension the first block already
    // covers on its own.
    cases.extend(corpus(
        &[7, -7, 120, -120],
        &[1.0, 4.0],
        &[0.3],
        &[5],
    ));
    cases
}

/// §29.3's full grid: `|d| ∈ {1..40, 100, 500}` × sign × `P/|d| ∈ {0.5, 1, 2, 4}` × dynamic
/// `{0, 10, 30, 60}%` × `σ ∈ {0, 2, 5, 10}`.
fn full_corpus() -> Vec<Case> {
    let mut shifts: Vec<i32> = (1..=40).collect();
    shifts.extend([100, 500]);
    let mut signed = Vec::with_capacity(shifts.len() * 2);
    for shift in shifts {
        signed.push(shift);
        signed.push(-shift);
    }
    corpus(&signed, &[0.5, 1.0, 2.0, 4.0], &[0.0, 0.1, 0.3, 0.6], &[0, 2, 5, 10])
}

/// What one case produced.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Tally {
    steps: u64,
    /// `Status::Confirmed`.
    confirmed: u64,
    /// `Uncertain` or `None` — allowed, and the only other outcome there is.
    refused: u64,
    /// Confirmed with a `d` that is not the truth. **This is G1's numerator.**
    wrong: u64,
    /// The canvas disagrees with the document. Independent of `wrong`: on a page whose period divides
    /// the step an alias writes *identical* pixels, so a case can be wrong without being visibly
    /// wrong, and a case can only be byte-wrong if something else is broken.
    bytes_wrong: u64,
}

impl Tally {
    fn merge(&mut self, other: Self) {
        self.steps += other.steps;
        self.confirmed += other.confirmed;
        self.refused += other.refused;
        self.wrong += other.wrong;
        self.bytes_wrong += other.bytes_wrong;
    }

    /// G1's metric.
    fn error_rate(&self) -> f64 {
        self.wrong as f64 / self.steps.max(1) as f64
    }

    /// Recorded, never optimised (the task's exit condition 3): a funnel that refuses everything has
    /// a perfect error rate, so the coverage is what says whether the corpus was answered at all.
    fn coverage(&self) -> f64 {
        self.confirmed as f64 / self.steps.max(1) as f64
    }
}

/// The document rows the canvas should hold after `applied` was written.
fn expected_span(start: u32, extent: u32, applied: i32) -> (u64, u64) {
    if applied >= 0 {
        (start as u64, extent as u64 + applied as u64)
    } else {
        let prepended = applied.unsigned_abs() as u64;
        (start as u64 - prepended, extent as u64 + prepended)
    }
}

fn truth_rows(image: &TestImage, first: u64, end: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(((end - first) as usize) * SCAN_WIDTH as usize * 4);
    for y in first..end {
        out.extend_from_slice(image.row(y as u32));
    }
    out
}

/// The canvas against the document, with the fixture's frame noise as the only tolerance.
///
/// `add_noise` (`testkit.rs:560`) adds one value in `[-sigma, sigma]` to the three colour channels of
/// every pixel of a stepped frame, and the fixture's frame 0 carries none — so the initial viewport
/// is exact and the written rows are within `sigma`. Alpha is never touched, so it is compared
/// exactly. At `sigma == 0` this is byte equality, which is `§29.3`'s judgement.
fn rows_match(actual: &[u8], expected: &[u8], sigma: u8) -> bool {
    if actual.len() != expected.len() {
        return false;
    }
    let tolerance = sigma as i32;
    actual
        .iter()
        .zip(expected)
        .enumerate()
        .all(|(index, (&got, &want))| {
            if index % 4 == 3 {
                got == want
            } else {
                (got as i32 - want as i32).abs() <= tolerance
            }
        })
}

/// Run one case end to end: estimate, decide, write, and compare the canvas with the document.
fn run_case(case: &Case, funnel: Funnel) -> Tally {
    run_case_with(case, funnel, case.truth())
}

/// As [`run_case`], but the search centre is given rather than taken from the script.
///
/// The scan's own cases always hand the funnel a **correct** prior, because the prior's accuracy is
/// `P1.14`'s subject. The control needs the other configuration — a dead prior — which is a real
/// production state (§16.6's manual mode, and the first step of every session before `ĝ` is learned).
fn run_case_with(case: &Case, funnel: Funnel, expected: i32) -> Tally {
    let truth = case.truth();
    let extent = case.viewport_extent();
    let mut script = ScrollScript::starting_at(
        &case.image,
        extent,
        case.start,
        vec![case.spec],
    );
    let first = script.take(0);
    let second = script.take(1);

    let mut scratch = Scratch::new();
    let status = decide(
        &mut scratch,
        &first.view(),
        &second.view(),
        expected,
        search_window(truth),
        funnel,
    );

    finish_case(case, &first, &second, status)
}

/// §13.4's manual route over the same case: the same step, decided with no prior and the search
/// centred on zero. This is the whole difference — the loop, the canvas and the write path are the
/// ones [`run_case_with`] already exercises.
fn run_manual_case(case: &Case) -> Tally {
    let mut script = ScrollScript::starting_at(
        &case.image,
        case.viewport_extent(),
        case.start,
        vec![case.spec],
    );
    let first = script.take(0);
    let second = script.take(1);

    let mut scratch = Scratch::new();
    let status = manual_status(&mut scratch, &first.view(), &second.view());
    finish_case(case, &first, &second, status)
}

/// Turn a step's status into a tally, including the byte comparison of what was written.
fn finish_case(case: &Case, first: &Observation, second: &Observation, status: Status) -> Tally {
    let truth = case.truth();
    let extent = case.viewport_extent();
    let mut canvas = RecoveredImage::new(
        Axis::Vertical,
        SCAN_WIDTH as u64,
        MemoryBudget::for_viewport(SCAN_WIDTH as u64, extent as u64),
    );
    canvas.start(&first);
    let mut viewport = ViewportState::new(extent);

    let mut tally = Tally {
        steps: 1,
        ..Tally::default()
    };
    let mut applied = None;
    match status {
        Status::Confirmed { d } => {
            tally.confirmed = 1;
            if d != truth {
                tally.wrong = 1;
            }
            // Gate one is what stops a session from acting on a shift the canvas cannot honour
            // (§16.2). With the gates off the answer can be such a shift, and writing it would only
            // trip the write path's own assertion — the answer is already counted as wrong.
            if d.unsigned_abs() <= extent {
                viewport
                    .apply(&mut canvas, &second, d)
                    .expect("the budget is eight viewports");
                applied = Some(d);
            }
        }
        Status::Uncertain { .. } | Status::None => tally.refused = 1,
    }

    if let Some(applied) = applied {
        let (doc_first, doc_len) = expected_span(case.start, extent, applied);
        let actual = canvas.rows(0, canvas.primary_len()).expect("resident");
        let expected = truth_rows(&case.image, doc_first, doc_first + doc_len);
        if !rows_match(&actual, &expected, case.noise()) {
            tally.bytes_wrong = 1;
        }
    }
    tally
}

impl Case {
    fn viewport_extent(&self) -> u32 {
        SCAN_VIEWPORT
    }
}

fn scan(cases: &[Case], funnel: Funnel) -> Tally {
    let mut tally = Tally::default();
    for case in cases {
        tally.merge(run_case(case, funnel));
    }
    tally
}

/// As [`scan`], through §13.4's manual route instead of the prior-driven one.
fn scan_manual(cases: &[Case]) -> Tally {
    let mut tally = Tally::default();
    for case in cases {
        tally.merge(run_manual_case(case));
    }
    tally
}

fn report(label: &str, tally: &Tally) {
    println!(
        "{label}: steps {} confirmed {} ({:.1}% coverage) refused {} wrong {} bytes_wrong {} error_rate {:.4}",
        tally.steps,
        tally.confirmed,
        tally.coverage() * 100.0,
        tally.refused,
        tally.wrong,
        tally.bytes_wrong,
        tally.error_rate()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The control: a scan that cannot fail is not a gate.
    ///
    /// Two broken funnels prove the tally can see a wrong answer, and both were **measured** on this
    /// corpus rather than assumed (2026-10-08, `cargo test -p snapclip-capture --lib acceptance`):
    ///
    /// | funnel | confirmed | `wrong` |
    /// |---|---|---|
    /// | `FULL` | 29/32 (90.6%) | 0 |
    /// | `FULL`, layer 3 removed | 29/32 | **4** |
    /// | `FULL`, dead prior (`expected = 0`) | 7/32 | **7** |
    /// | `FIRST_ONLY` | 32/32 | 0 |
    /// | `FULL`, gates off | 32/32 | 0 |
    ///
    /// The last two rows are why this control had to be rewritten. The first version of it asserted
    /// that `FIRST_ONLY` fails — that the zero shift ties with the truth on support and wins on a
    /// smaller `|d|`. `P1.24` then found and fixed exactly that tie-break (`candidates_1d` was ranking
    /// by distance from *zero* instead of from the search centre, which is what let the candidate set
    /// drift off the prior entirely), and with it fixed `FIRST_ONLY` is exact on this corpus. That is
    /// not luck: the page alternates a periodic band with an aperiodic one, so at the truth *every*
    /// row digest matches while any alias matches only the periodic half, which makes the truth's
    /// support strictly maximal. The design's alias problem needs a page whose **matching region** is
    /// periodic — i.e. no aperiodic band inside the overlap — and this corpus deliberately does not
    /// build one.
    ///
    /// What is load-bearing here is layer 3, and that is the non-vacuity the scan needs: drop the
    /// full-resolution refinement and four cases are confirmed one pixel short of the truth, because
    /// layer 2 measures on a 4 px grid and its winner is the cell's grid point. Note also that
    /// **the gates are not what makes the answer right** — with all four off, `wrong` stays 0; the
    /// gates refuse, they do not correct (which is `P1.13`'s ablation result restated).
    #[test]
    fn the_scan_catches_a_funnel_that_lost_the_last_layer() {
        let mut without_refine = Funnel::FULL;
        without_refine.refine = false;
        let broken = scan(&smoke_corpus(), without_refine);
        report("layer 1 + layer 2, no refinement", &broken);
        assert!(
            broken.wrong > 0,
            "removing layer 3 changed nothing: the scan cannot see a wrong answer, so the acceptance \
             case below would be vacuous"
        );
        assert!(
            broken.confirmed > 0,
            "the control confirmed nothing, so it did not exercise the path it is the control for"
        );

        // The other broken funnel: no prior at all. §16.6's manual mode and the first step of every
        // session both run without one, and there the window does not even contain the truth.
        let mut dead_prior = Tally::default();
        for case in &smoke_corpus() {
            dead_prior.merge(run_case_with(case, Funnel::FULL, 0));
        }
        report("full funnel, dead prior", &dead_prior);
        assert!(
            dead_prior.wrong > 0,
            "a dead prior confirmed nothing wrong, which would mean the prior is not load-bearing"
        );

        // Two more controls, and they are here because they *fail to fail* — which is the measured
        // answer to "is the gate measuring what it claims?". Layer 1 alone is exact on this corpus:
        // the truth has strictly maximal support (every row digest agrees) while an alias only
        // matches the periodic half of the page, so the control has to break a *later* layer to have
        // teeth. And with all four gates off the answer is still never wrong: gates reject
        // candidates, they never correct one (`P1.13`'s ablation says the same thing about the
        // gates on the estimator side).
        let first_only = scan(&smoke_corpus(), Funnel::FIRST_ONLY);
        report("layer 1 only", &first_only);
        assert_eq!(
            first_only.wrong, 0,
            "layer 1 confirmed a wrong shift on this corpus, so the corpus changed: the controls \
             above would then be measuring layer 1 rather than the funnel"
        );

        let mut no_gates = Funnel::FULL;
        no_gates.gates = GateMask::NONE;
        let ungated = scan(&smoke_corpus(), no_gates);
        report("full funnel, gates off", &ungated);
        assert_eq!(
            ungated.wrong, 0,
            "a gate changed the answer rather than only rejecting: gates decide, they do not measure"
        );
    }

    /// The gate itself: the whole funnel, over the smoke corpus, never confirms a wrong shift.
    #[test]
    fn the_full_funnel_never_confirms_a_wrong_shift() {
        let tally = scan(&smoke_corpus(), Funnel::FULL);
        report("full funnel", &tally);
        assert_eq!(
            tally.wrong, 0,
            "{} of {} steps were confirmed with the wrong displacement (G1)",
            tally.wrong, tally.steps
        );
        assert_eq!(
            tally.bytes_wrong, 0,
            "{} canvases disagreed with the document",
            tally.bytes_wrong
        );
        assert!(
            tally.confirmed > 0,
            "every step was refused: zero errors by refusing everything is not the property G1 asks \
             for, and it would hide a funnel that can no longer answer"
        );
        assert_eq!(
            tally.confirmed + tally.refused,
            tally.steps,
            "every step has to be either confirmed or refused"
        );
    }

    /// The corpus itself is checked, so a broken case cannot pass as a passing case: the script has
    /// to be buildable, the truth has to be non-zero, and the page has to be long enough for both
    /// frames.
    #[test]
    fn every_scan_case_is_a_real_step() {
        let cases = smoke_corpus();
        assert!(
            cases.len() >= 32,
            "the smoke corpus shrank to {} cases; it is the gate's only corpus on the default path",
            cases.len()
        );
        for case in &cases {
            assert_ne!(case.truth(), 0, "{}: a zero step is a repeat, not a scan case", case.name);
            let extent = case.viewport_extent();
            let start = case.start as i64 + case.truth() as i64;
            assert!(
                start >= 0 && (start as u64) + extent as u64 <= case.image.height() as u64,
                "{}: the step leaves the document",
                case.name
            );
        }
    }

    /// What the write path does today when a moving region covers the rows a prepend reveals.
    ///
    /// The corpus deliberately does not paint a moving overlay on a step that scrolls up (see
    /// `scan_case`); this case is the reason written down as an executable statement. The answer is
    /// still right — the overlay sits at the top of the frame, outside the match band — and the
    /// canvas still differs, because `P1`'s write path copies the frame's leading rows verbatim.
    ///
    /// When §18.2's time model lands and stops writing rows the region model has classified as
    /// moving, this test must fail and be rewritten. That is the whole point of asserting today's
    /// behaviour rather than leaving it unstated.
    #[test]
    fn a_moving_region_over_the_revealed_rows_is_written_verbatim() {
        let mut case = scan_case(-120, 1.0, 0.0, 0, 31);
        // The corpus strips the overlay from a step that scrolls up; this case is the one place it is
        // put back, because the point is to assert what the write path does with it.
        case.spec = StepSpec::move_by(-120).with_dynamic(0.3);
        let tally = run_case(&case, Funnel::FULL);
        report("prepend under a moving overlay", &tally);
        assert_eq!(
            tally.confirmed, 1,
            "the matcher should still answer: the moving region is at the top, the band is at the \
             bottom"
        );
        assert_eq!(
            tally.wrong, 0,
            "the displacement itself is right; the overlay is not a matching problem"
        );
        assert_eq!(
            tally.bytes_wrong, 1,
            "the canvas was expected to differ here: if it now matches, the write path stopped \
             copying a moving region and this case has to be rewritten (§18.2)"
        );
    }

    /// `docs/30` §29.3's full grid. Minutes, release, and deliberately not on the default path —
    /// but it is the same code as the gate, so the two cannot disagree about what "correct" means.
    #[test]
    #[ignore = "E-ACC-1 full scan: 5376 cases, run in release"]
    fn the_full_scan_reports_its_coverage() {
        let cases = full_corpus();
        let tally = scan(&cases, Funnel::FULL);
        report("full scan", &tally);
        assert_eq!(tally.steps as usize, cases.len());
        assert_eq!(tally.wrong, 0, "the full scan confirmed a wrong shift");
        assert_eq!(tally.bytes_wrong, 0, "the full scan wrote a wrong canvas");
    }

    /// §16.6: a page that repeats is exactly the case the prior exists for, and without one the
    /// honest answer is `Uncertain`.
    ///
    /// The fixture is periodic by construction: one structure, and a band height equal to its period
    /// (`HorizontalBars` reads `y % period` inside the band, `testkit.rs:92`), so the document repeats
    /// every 37 rows. A 37 px step leaves the frame *pixel-identical*, so `−37`, `0` and `+37` explain
    /// it equally well — all three inside the 68 px manual window, which is what makes the family
    /// check reachable here rather than at the edge of the search.
    #[test]
    fn a_manual_route_refuses_a_periodic_page_instead_of_guessing() {
        let image = TestImage::from_structures(
            SCAN_WIDTH,
            1900,
            5,
            37,
            &[Structure::HorizontalBars { period: 37 }],
        );
        let mut script = ScrollScript::new(&image, SCAN_VIEWPORT, vec![StepSpec::move_by(37)]);
        let first = script.take(0);
        let second = script.take(1);

        let mut scratch = Scratch::new();
        let status = manual_status(&mut scratch, &first.view(), &second.view());
        println!("periodic page, manual route: {status:?}");
        assert!(
            matches!(status, Status::Uncertain { .. }),
            "a periodic page must be reported as an ambiguity, got {status:?}"
        );
    }

    /// **This records a defect, not a contract.**
    ///
    /// `P3.06`'s exit condition ③ — "a manual sequence never confirms a wrong shift" — does **not**
    /// hold yet, and this is the second of its two mechanisms. A row digest is an exact byte
    /// comparison, so on a frame with noise *no* row matches across frames: every `support` is 0 and
    /// the candidate ranking falls back to its first tie-break, `|d − expected|`. The manual route has
    /// no `expected`, so the set degenerates to the eight shifts nearest **zero** — the failure
    /// `P1.24` found and fixed for the prior-driven route by centring it on the prior. The truth is
    /// then not even scored, and one of the eight wins.
    ///
    /// If this test starts failing because the answer became correct, or became a refusal, delete it
    /// and update `docs/30` §16.6.2: the blocker has been fixed.
    #[test]
    fn the_prior_free_route_is_wrong_on_a_noisy_frame_today() {
        // σ = 5, |d| = 7, period 28: the noisy half of the smoke corpus (seed 48).
        let case = scan_case(-7, 4.0, 0.0, 5, 48);
        let mut script =
            ScrollScript::starting_at(&case.image, SCAN_VIEWPORT, case.start, vec![case.spec]);
        let first = script.take(0);
        let second = script.take(1);

        let mut scratch = Scratch::new();
        let status = manual_status(&mut scratch, &first.view(), &second.view());
        assert_eq!(case.truth(), -7);
        assert_eq!(
            status,
            Status::Confirmed { d: -6 },
            "the measured defect: the truth (−7) is not in the degenerate candidate set"
        );
    }

    /// `P3.06`'s exit condition ③ is **unmet**, and this is its record on the default path so that the
    /// defect cannot go quiet.
    ///
    /// Measured over `E-ACC-1`'s smoke corpus (32 cases) with the design's manual window
    /// (`manual_window(450) = 68` px). The two mechanisms behind the wrong answers are different, and
    /// both are recorded in `docs/30` §16.6.2:
    ///
    /// * an alias of the page's own period landing **inside** the window — the rival that would have
    ///   cost it the margin gate is outside the search, so gate four cannot see it;
    /// * a frame with noise, where no row digest matches at all and the prior-free candidate ranking
    ///   degenerates to the shifts nearest zero ([`the_prior_free_route_is_wrong_on_a_noisy_frame_today`]).
    ///
    /// | subset | confirmed | refused | wrong | bytes_wrong | error rate |
    /// |---|---|---|---|---|---|
    /// | σ = 0 (24 cases) | 17 (70.8%) | 7 | **2** | 0 | 0.0833 |
    /// | σ = 5 (8 cases) | 2 (25.0%) | 6 | **2** | 1 | 0.2500 |
    /// | all (32 cases) | 19 (59.4%) | 13 | **4** | 1 | 0.1250 |
    ///
    /// Widening the window to the whole verifiable range was measured as the obvious remedy and is
    /// **not** one: it removes both σ = 0 errors (the rivals become visible, so the margin gate
    /// refuses) but collapses coverage from 19 to 5 confirmations, because the candidate set — still
    /// capped at `CANDIDATE_LIMIT = 8` — fills with aliases. See `docs/30` §36.2.
    #[test]
    fn the_manual_route_reports_its_coverage_and_its_defect_today() {
        let cases = smoke_corpus();
        let mut clean = Tally::default();
        let mut noisy = Tally::default();
        for case in &cases {
            let tally = run_manual_case(case);
            if case.noise() == 0 {
                clean.merge(tally);
            } else {
                noisy.merge(tally);
            }
        }
        report("manual route, sigma = 0", &clean);
        report("manual route, sigma = 5", &noisy);
        let tally = scan_manual(&cases);
        report("manual route", &tally);

        assert_eq!(tally.steps, 32);
        assert_eq!(
            tally.wrong, 4,
            "the number `P3.06` is blocked on: if it changed, read §16.6.2 before updating this"
        );
        assert_eq!(tally.bytes_wrong, 1);
        assert!(
            tally.confirmed > 0,
            "the manual route must still answer what it can: a route that refuses everything is not \
             a degraded route, it is a missing one"
        );
    }

    /// §13.4: manual mode is the same session, with `n = 0` and no prior.
    ///
    /// A step inside the manual window is confirmed and the canvas tracks it — the estimator is not
    /// degraded, it simply has less to go on. This is the half of the manual route that works; the
    /// half that does not is [`the_prior_free_route_is_wrong_on_a_noisy_frame_today`].
    #[test]
    fn a_manual_session_tracks_the_non_zero_steps() {
        // 7 px is inside the manual window (`0.15 · 450 = 68` px).
        let case = scan_case(7, 1.0, 0.0, 0, 11);
        let mut session = ScrollSession::new(
            Axis::Vertical,
            SCAN_WIDTH as u64,
            MemoryBudget::for_viewport(SCAN_WIDTH as u64, SCAN_VIEWPORT as u64),
        );
        assert!(
            session.follow(),
            "a session drives the scroll until the user says otherwise"
        );
        session.set_follow(false);
        assert!(session.manual(), "not following is what manual mode is");

        let mut script =
            ScrollScript::starting_at(&case.image, SCAN_VIEWPORT, case.start, vec![case.spec]);
        let first = script.take(0);
        let second = script.take(1);
        session.start(&first);

        let mut scratch = Scratch::new();
        let status = manual_status(&mut scratch, &first.view(), &second.view());
        assert_eq!(
            status,
            Status::Confirmed { d: 7 },
            "a step inside the manual window is answerable without a prior"
        );

        let mut viewport = ViewportState::new(SCAN_VIEWPORT);
        viewport
            .apply(session.canvas_mut(), &second, 7)
            .expect("the budget is eight viewports");
        session.record_step();
        session.record_committed();
        let committed = session.canvas().primary_len();
        let actual = session
            .canvas_mut()
            .rows(0, committed)
            .expect("resident");
        let (doc_first, doc_len) = expected_span(case.start, SCAN_VIEWPORT, 7);
        let expected = truth_rows(&case.image, doc_first, doc_first + doc_len);
        assert!(
            rows_match(&actual, &expected, case.noise()),
            "the canvas must track the step the manual route confirmed"
        );
    }
}
