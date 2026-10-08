//! `docs/31` `P0.03` (`E-PERF-1`): what does one displacement estimate cost, before anyone
//! optimizes it? First principle **F-08** — the project has no matching-cost data at all, so
//! every "should we optimize this" decision so far has been intuition, which `AGENTS.md`
//! item 6 forbids.
//!
//! # Scope, and why it is incomplete on purpose
//!
//! The task presupposes `P1.02`–`P1.05` (the four layers of `docs/30` §15.4). None of them
//! exists yet, so the tasklist's own fallback applies: measure the **layer-1 prototype**
//! (per-row 64-bit digest → 1D candidate search → top-`K`) and label the result
//! *incomplete*. Combinations ②(1+2) ③(1+2+3) ④(+ORB) are recorded as **not obtained**
//! rather than approximated, because a prototype of layers 2–4 would be replaced by `P1.03`–
//! `P1.05` anyway and its numbers would silently become the target `§23.3` is waiting for.
//!
//! The device therefore answers exactly four things:
//!
//! 1. how long the layer-1 pass takes per step, split into **digest** and **1D search**, at
//!    three viewports and over 1000 scripted steps;
//! 2. whether the scripted displacement actually shows up in the candidate set (a sanity
//!    floor: a cheap estimator that never finds the truth is not a cheap estimator);
//! 3. the allocator high-water mark of the pass (`CountingAllocator`);
//! 4. the same pass with the one optimization the first result suggests — **digest only the
//!    rows the frame actually revealed and reuse the overlapping rows' digests** — so the
//!    recommendation that goes to `P1.02` is itself measured rather than argued. The variant
//!    is measured, not adopted: nothing here is on the product path.
//!
//! # Deliberate deviations from the task's device description
//!
//! * The synthetic sequence is generated **here** instead of by `P1.01`'s testkit (which does
//!   not exist). The document is procedural — row `y` is a pure function of `y` — so the
//!   known truth moves with the script instead of with a file on disk.
//! * Frame production (`BitBlt`'s stand-in: `SyntheticSequence::advance_frame`) is **outside**
//!   the timed region. This experiment is about the estimator; capture cost is `P0.05`'s and
//!   `P0.06`'s subject.
//! * One process per scenario: the probe reads `SNAPCLIP_PERF1_VIEWPORT` and
//!   `SNAPCLIP_PERF1_VARIANT` and appends one JSON line, so `tools/p0-03-matching-cost.ps1`
//!   can run each of the six scenarios in a fresh process.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::geometry::Rect;
use crate::scroll::displacement::{
    self, GateOutcome, Scratch, Status, candidates_1d, gate_geometry, gate_margin,
    gate_residual_gain, gate_support, is_verifiable, margin_of, outside_cell_second, refine_winner,
    score_candidates_2d,
};
use crate::scroll::observation::{Axis, Observation};
use crate::scroll::orb;

/// Rows a single scripted step advances the document viewport by.
///
/// 120 px is the measured Chromium response to eight wheel notches (`P0.09`, `docs/30`
/// §24.6.2), so every cost below is quoted for a step size the injection path actually
/// produces rather than for a round number.
const STEP_ROWS: i64 = 120;

/// Candidate shifts searched on each side of the expected step (`docs/30` §15.4 layer 1 is
/// centered on `n·ĝ`; the width of that window is the layer's own parameter, `±32` here).
const SEARCH_ROWS: i64 = 32;

/// `docs/30` §15.4 layer 1 emits the top `K` candidates; §16.6 fixes `K = 8`.
const CANDIDATES: usize = 8;

/// The three viewports `docs/31` P0.03 fixes for this experiment.
#[derive(Clone, Copy)]
struct Viewport {
    label: &'static str,
    width: u32,
    height: u32,
}

const VIEWPORT_1080P: Viewport = Viewport { label: "1080p", width: 1920, height: 1080 };
const VIEWPORT_1440P: Viewport = Viewport { label: "1440p", width: 2560, height: 1440 };
const VIEWPORT_4K: Viewport = Viewport { label: "4k", width: 3840, height: 2160 };

// ---------------------------------------------------------------------------------------
// The synthetic sequence
// ---------------------------------------------------------------------------------------

/// A procedural document: `row(y)` is a pure function of `y`, so the scripted sequence needs
/// no file and no random number generator to stay reproducible.
struct VirtualDocument {
    width: u32,
}

impl VirtualDocument {
    fn new(width: u32) -> Self {
        Self { width }
    }

    /// Writes document row `y` into `out`.
    ///
    /// The structure is deliberately text-like: 36-row bands alternate three background
    /// values, one border row opens each band, rows 6–25 of a band carry a dithered "text"
    /// pattern, a vertical accent stripe runs down the left third and a scrollbar rail sits
    /// on the right edge. What matters for this experiment is only that **two different rows
    /// have different digests** and that a real displacement produces hundreds of exact row
    /// matches, which is what a candidate pass feeds on.
    fn row(&self, y: u32, out: &mut [u8]) {
        debug_assert_eq!(out.len() as u32, self.width, "a document row is one scanline wide");
        let width = self.width;
        let base: u8 = [216, 238, 198][((y / 36) % 3) as usize];
        let border = y % 36 == 0;
        let text = (6..26).contains(&(y % 36));
        let right_edge = width.saturating_sub(6);
        let text_edge = width.saturating_sub(40);
        for (index, slot) in out.iter_mut().enumerate() {
            let x = index as u32;
            *slot = if x >= right_edge {
                208
            } else if (x + 7) % 211 < 3 {
                base.wrapping_sub(48)
            } else if border {
                base.wrapping_sub(64)
            } else if text && (40..text_edge).contains(&x) {
                let h = hash(x / 2, y, 0x9E37);
                if h % 5 < 2 {
                    base.wrapping_sub(96)
                } else {
                    base.wrapping_sub((h % 7) as u8)
                }
            } else {
                base.wrapping_sub((hash(x / 3, y, 0x1234) % 4) as u8)
            };
        }
    }
}

/// Integer mixing used as the document's ink: no `rand` dependency, no state.
fn hash(x: u32, y: u32, salt: u32) -> u32 {
    let mut v = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B) ^ salt;
    v ^= v >> 15;
    v = v.wrapping_mul(0x2545_F491);
    v ^ (v >> 13)
}

/// A scripted scroll: `steps` steps of `step_rows` rows each over a `viewport`-sized window.
struct SyntheticSequence {
    viewport: Viewport,
    steps: usize,
    step_rows: i64,
    document: VirtualDocument,
}

impl SyntheticSequence {
    /// Builds the script. The document is procedural and unbounded, so the script is the only
    /// state: `steps` frames, each advanced by `step_rows`, over `viewport`-sized windows.
    fn scripted(viewport: Viewport, steps: usize, step_rows: i64) -> Self {
        debug_assert!(
            step_rows > 0 && (step_rows as u32) < viewport.height,
            "a step has to move the viewport without skipping past it, or frames stop overlapping"
        );
        Self {
            viewport,
            steps,
            step_rows,
            document: VirtualDocument::new(viewport.width),
        }
    }

    fn len(&self) -> usize {
        self.steps
    }

    /// Fills `out` — `viewport.height` scanlines — with document rows `[first, first + height)`.
    fn fill(&self, first: u32, out: &mut [u8]) {
        let width = self.viewport.width as usize;
        for (row, chunk) in out.chunks_exact_mut(width).enumerate() {
            self.document.row(first + row as u32, chunk);
        }
    }

    /// Produces frame `n ≥ 1`, reusing the `height - step_rows` rows it shares with frame
    /// `n - 1` and generating only the newly revealed ones — the shape a real frame source
    /// has, and the reason this loop can afford 1000 steps at 4K.
    fn advance_frame(&self, n: usize, previous: &[u8], out: &mut [u8]) {
        let width = self.viewport.width as usize;
        let height = self.viewport.height as usize;
        let step = self.step_rows as usize;
        debug_assert!(step > 0 && step < height, "a step must overlap the previous frame");
        let first = n as u32 * self.step_rows as u32;
        out[..(height - step) * width].copy_from_slice(&previous[step * width..]);
        for row in (height - step)..height {
            self.document
                .row(first + row as u32, &mut out[row * width..(row + 1) * width]);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Layer 1
// ---------------------------------------------------------------------------------------

/// The layer-1 row digest: 64-bit FNV-1a over the row, `O(W)` and position sensitive.
///
/// `docs/30` §15.4 specifies a "64-bit fold" without fixing the function. FNV-1a is this
/// prototype's choice, and `P1.05` promoted it to the shipped one (`displacement::line_digest`):
/// the same bytes, the same constant, one implementation. Every cost quoted from this probe is
/// therefore the cost of the function the session actually runs.
fn row_digest(row: &[u8]) -> u64 {
    crate::scroll::displacement::line_digest(row)
}

/// How many rows agree exactly when the observation is attributed a displacement of `shift`
/// (`docs/30` §15.4 layer 1's "support"). Delegates to the shipped scan so the probe cannot drift
/// from the funnel; a shift that is not a displacement at all supports nothing.
fn support_at(previous: &[u64], current: &[u64], shift: i64) -> u32 {
    match i32::try_from(shift) {
        Ok(shift) => crate::scroll::displacement::support_at(previous, current, shift),
        Err(_) => 0,
    }
}

/// The top-`k` shifts by support, searched in `[expected - search, expected + search]`.
/// Ties keep the smaller shift so the candidate set is deterministic.
fn candidate_shifts(
    previous: &[u64],
    current: &[u64],
    expected: i64,
    search: i64,
    k: usize,
) -> Vec<i64> {
    let mut scored: Vec<(u32, i64)> = Vec::with_capacity((2 * search + 1) as usize);
    for shift in (expected - search)..=(expected + search) {
        scored.push((support_at(previous, current, shift), shift));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(k);
    scored.into_iter().map(|(_, shift)| shift).collect()
}

// ---------------------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Distribution {
    p50_us: u64,
    p95_us: u64,
    max_us: u64,
}

fn distribution(mut samples: Vec<u64>) -> Distribution {
    if samples.is_empty() {
        return Distribution::default();
    }
    samples.sort_unstable();
    let percentile = |p: usize| samples[((samples.len() - 1) * p / 100).min(samples.len() - 1)];
    Distribution {
        p50_us: percentile(50),
        p95_us: percentile(95),
        max_us: *samples.last().expect("a non-empty sample set has a last element"),
    }
}

struct SequenceReport {
    viewport: &'static str,
    steps: usize,
    truth_steps: usize,
    top_k_hits: usize,
    digest: Distribution,
    search: Distribution,
    total: Distribution,
    live_bytes: usize,
    peak_bytes: usize,
}

impl SequenceReport {
    /// The report an empty script produces: zero steps, zero cost. Not a special case for the
    /// test's benefit — a run with no steps genuinely has no distribution.
    fn empty(viewport: &'static str) -> Self {
        Self {
            viewport,
            steps: 0,
            truth_steps: 0,
            top_k_hits: 0,
            digest: Distribution::default(),
            search: Distribution::default(),
            total: Distribution::default(),
            live_bytes: live_bytes(),
            peak_bytes: peak_bytes(),
        }
    }

    fn top_k_hit_rate(&self) -> f64 {
        if self.truth_steps == 0 {
            return 0.0;
        }
        self.top_k_hits as f64 / self.truth_steps as f64
    }

    /// A layer's share of the step, taken on the median rather than the mean so one preempted
    /// step cannot move it.
    fn share(&self, layer: Distribution) -> f64 {
        if self.total.p50_us == 0 {
            return 0.0;
        }
        layer.p50_us as f64 / self.total.p50_us as f64
    }

    fn json_line(&self, combo: &str) -> String {
        format!(
            concat!(
                "{{\"kind\":\"combo\",\"combo\":\"{}\",\"viewport\":\"{}\",\"steps\":{},",
                "\"truth_steps\":{},\"top_k_hits\":{},\"top_k_hit_rate\":{:.4},",
                "\"p50_us\":{},\"p95_us\":{},\"max_us\":{},",
                "\"layers\":{{\"digest\":{{\"p50_us\":{},\"p95_us\":{},\"max_us\":{},\"share\":{:.4}}},",
                "\"search_1d\":{{\"p50_us\":{},\"p95_us\":{},\"max_us\":{},\"share\":{:.4}}}}},",
                "\"alloc\":{{\"live_bytes\":{},\"peak_bytes\":{}}},\"profile\":\"release\"}}"
            ),
            combo,
            self.viewport,
            self.steps,
            self.truth_steps,
            self.top_k_hits,
            self.top_k_hit_rate(),
            self.total.p50_us,
            self.total.p95_us,
            self.total.max_us,
            self.digest.p50_us,
            self.digest.p95_us,
            self.digest.max_us,
            self.share(self.digest),
            self.search.p50_us,
            self.search.p95_us,
            self.search.max_us,
            self.share(self.search),
            self.live_bytes,
            self.peak_bytes,
        )
    }
}

/// Runs the layer-1 pass over `sequence` and reports its cost distribution.
///
/// Timed region: **digest + 1D search only**. `advance_frame` (the frame source's stand-in) is
/// deliberately outside it, and the allocator peak is reset once the working buffers exist so
/// `peak - live` answers "what does the pass allocate on top of its inputs".
fn measure(sequence: &SyntheticSequence) -> SequenceReport {
    measure_variant(sequence, Variant::FullFrameDigest)
}

/// Which layer-1 implementation the run measures.
///
/// `FullFrameDigest` is the straightforward reading of `docs/30` §15.4 ("digest the rows of
/// the new observation"): every step digests all `H` rows again. `RevealedRowsOnly` is the
/// same algorithm with one observation applied — consecutive frames of a scroll overlap by
/// `H - step_rows` rows **whose bytes are identical**, so their digests cannot have changed
/// and the previous step's values can be shifted into place instead of recomputed. It is
/// measured here because the first variant's result (digest dominates the step) makes this the
/// question `P1.02` has to answer; it is not adopted by the probe.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    FullFrameDigest,
    RevealedRowsOnly,
}

impl Variant {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "full" => Some(Self::FullFrameDigest),
            "revealed" => Some(Self::RevealedRowsOnly),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::FullFrameDigest => "layer1-full",
            Self::RevealedRowsOnly => "layer1-revealed",
        }
    }
}

fn measure_variant(sequence: &SyntheticSequence, variant: Variant) -> SequenceReport {
    let viewport = sequence.viewport;
    if sequence.len() == 0 {
        return SequenceReport::empty(viewport.label);
    }

    let width = viewport.width as usize;
    let height = viewport.height as usize;
    let step = sequence.step_rows;
    let reveal = step as usize;

    let mut previous = vec![0u8; width * height];
    let mut current = vec![0u8; width * height];
    sequence.fill(0, &mut previous);
    let mut previous_digests = vec![0u64; height];
    let mut current_digests = vec![0u64; height];
    // The first frame's digests are input, not work: a real session digests its first
    // observation once and from then on every step's frame is digested exactly once.
    for (row, digest) in previous_digests.iter_mut().enumerate() {
        *digest = row_digest(&previous[row * width..(row + 1) * width]);
    }

    let mut digest_samples = Vec::with_capacity(sequence.len());
    let mut search_samples = Vec::with_capacity(sequence.len());
    let mut total_samples = Vec::with_capacity(sequence.len());
    let mut top_k_hits = 0usize;

    reset_peak();

    for n in 1..=sequence.len() {
        sequence.advance_frame(n, &previous, &mut current);

        let started = Instant::now();
        match variant {
            Variant::FullFrameDigest => {
                for (row, digest) in current_digests.iter_mut().enumerate() {
                    *digest = row_digest(&current[row * width..(row + 1) * width]);
                }
            }
            Variant::RevealedRowsOnly => {
                // Rows `[0, H - step)` of this frame are rows `[step, H)` of the previous one,
                // byte for byte, so their digests move with them.
                current_digests[..height - reveal].copy_from_slice(&previous_digests[reveal..]);
                for row in (height - reveal)..height {
                    current_digests[row] = row_digest(&current[row * width..(row + 1) * width]);
                }
            }
        }
        let digested = Instant::now();
        let candidates = candidate_shifts(&previous_digests, &current_digests, step, SEARCH_ROWS, CANDIDATES);
        let finished = Instant::now();

        if candidates.contains(&step) {
            top_k_hits += 1;
        }
        digest_samples.push(elapsed_us(digested - started));
        search_samples.push(elapsed_us(finished - digested));
        total_samples.push(elapsed_us(finished - started));

        std::mem::swap(&mut previous, &mut current);
        std::mem::swap(&mut previous_digests, &mut current_digests);
    }

    SequenceReport {
        viewport: viewport.label,
        steps: sequence.len(),
        truth_steps: sequence.len(),
        top_k_hits,
        digest: distribution(digest_samples),
        search: distribution(search_samples),
        total: distribution(total_samples),
        live_bytes: live_bytes(),
        peak_bytes: peak_bytes(),
    }
}

fn elapsed_us(duration: Duration) -> u64 {
    duration.as_micros() as u64
}

// ---------------------------------------------------------------------------------------
// The production funnel (the four combinations `docs/31` P0.03 asks for)
// ---------------------------------------------------------------------------------------
//
// The section above measures a **layer-1 prototype**; this one measures the shipped funnel
// (`displacement.rs` layers 1–3 plus `orb.rs`), because P1.02–P1.07 and P1.23 have landed and
// `docs/30` §36.2 (`OQ-8`) owes the four-combination rerun to exactly this device.
//
// Four deliberate decisions, each of which changes what the numbers mean:
//
// * **Frame production and `Observation` construction are outside the timed region.** The
//   frame source is `P0.05`/`P0.06`'s subject, and an `Observation` must exist before the
//   funnel can be called at all. The probe therefore pays one frame copy per step that the
//   session does not pay in that shape (a capture readback *is* the copy), and it is not
//   counted. What is counted is the estimate.
// * **The prior is the truth.** `expected` is the scripted step, so the search window is
//   centred correctly on every step. This measures *cost*, not accuracy — accuracy is
//   `E-ACC-1`'s device. A real prior drifts, and a drifted prior widens the window (§16.6).
// * **The search window is the production formula** (`docs/30` §15.6): `max(4, ceil(0.3·n·ĝ))`
//   = 36 at a 120 px step, where the prototype above used a fixed 32. Two percent wider, and
//   the prototype's rows stay reproducible as they were measured.
// * **ORB runs only when §15.4 ④ says so**, and its cost is reported as *per-vote × trigger
//   rate* rather than folded into every step. On a sequence where the vote never triggers, a
//   forced sample is taken anyway (`SNAPCLIP_PERF1_ORB_VOTES`) so the fourth combination has a
//   number instead of a blank, and forced samples are kept **out of** the step total because
//   production would not have paid them.

/// Which combination of the funnel a run measures. `DigestOnly` is not one of the four: it is
/// the attribution device for the layer-1 result (digest vs 1D search), because `candidates_1d`
/// digests both sides internally and one timing cannot be split after the fact.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Combo {
    DigestOnly,
    Layer1,
    Layer12,
    Layer123,
    Layer123Orb,
}

impl Combo {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "digest" => Some(Self::DigestOnly),
            "l1" => Some(Self::Layer1),
            "l12" => Some(Self::Layer12),
            "l123" => Some(Self::Layer123),
            "l123-orb" => Some(Self::Layer123Orb),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::DigestOnly => "funnel-digest",
            Self::Layer1 => "funnel-l1",
            Self::Layer12 => "funnel-l12",
            Self::Layer123 => "funnel-l123",
            Self::Layer123Orb => "funnel-l123-orb",
        }
    }

    /// How many of layers 1–3 this combination runs.
    fn depth(self) -> u8 {
        match self {
            Self::DigestOnly | Self::Layer1 => 1,
            Self::Layer12 => 2,
            Self::Layer123 | Self::Layer123Orb => 3,
        }
    }

    fn votes(self) -> bool {
        matches!(self, Self::Layer123Orb)
    }
}

/// `docs/30` §15.6: `W_search = max(4, ceil(0.3·n·ĝ))`.
fn production_search_window(expected: i32) -> i32 {
    let scaled = (expected.unsigned_abs() as f64 * 0.3).ceil() as i32;
    scaled.max(4)
}

/// `docs/30` §16.6: the prior's interval is `[n·ĝ(1−κ), n·ĝ(1+κ)]` with `κ = 0.5`.
fn inside_prior(shift: i32, expected: i32) -> bool {
    let magnitude = expected.unsigned_abs() as f32;
    let shift = shift.unsigned_abs() as f32;
    shift >= magnitude * (1.0 - displacement::PRIOR_KAPPA) && shift <= magnitude * (1.0 + displacement::PRIOR_KAPPA)
}

/// What one step's funnel produced, for the report's tallies.
#[derive(Default)]
struct FunnelTally {
    confirmed: u32,
    uncertain: u32,
    none: u32,
    wrong: u32,
    candidates_hit: u32,
    refined_hit: u32,
    orb_triggers: u32,
    orb_forced: u32,
}

struct FunnelReport {
    viewport: &'static str,
    combo: Combo,
    steps: usize,
    tally: FunnelTally,
    layer1: Distribution,
    layer2: Distribution,
    layer3: Distribution,
    orb: Distribution,
    total: Distribution,
    live_bytes: usize,
    peak_bytes: usize,
}

impl FunnelReport {
    fn empty(viewport: &'static str, combo: Combo) -> Self {
        Self {
            viewport,
            combo,
            steps: 0,
            tally: FunnelTally::default(),
            layer1: Distribution::default(),
            layer2: Distribution::default(),
            layer3: Distribution::default(),
            orb: Distribution::default(),
            total: Distribution::default(),
            live_bytes: live_bytes(),
            peak_bytes: peak_bytes(),
        }
    }

    fn candidate_rate(&self) -> f64 {
        if self.steps == 0 {
            return 0.0;
        }
        self.tally.candidates_hit as f64 / self.steps as f64
    }

    fn refined_rate(&self) -> f64 {
        if self.steps == 0 {
            return 0.0;
        }
        self.tally.refined_hit as f64 / self.steps as f64
    }

    fn confirmed_rate(&self) -> f64 {
        if self.steps == 0 {
            return 0.0;
        }
        self.tally.confirmed as f64 / self.steps as f64
    }

    fn orb_trigger_rate(&self) -> f64 {
        if self.steps == 0 {
            return 0.0;
        }
        self.tally.orb_triggers as f64 / self.steps as f64
    }

    fn json_line(&self) -> String {
        let stage = |name: &str, value: Distribution, total: Distribution| {
            format!(
                "\"{name}\":{{\"p50_us\":{},\"p95_us\":{},\"max_us\":{},\"share\":{:.4}}}",
                value.p50_us,
                value.p95_us,
                value.max_us,
                if total.p50_us == 0 { 0.0 } else { value.p50_us as f64 / total.p50_us as f64 }
            )
        };
        let layers = [
            stage("layer1", self.layer1, self.total),
            stage("layer2", self.layer2, self.total),
            stage("layer3", self.layer3, self.total),
            stage("orb", self.orb, self.total),
        ]
        .join(",");
        format!(
            concat!(
                "{{\"kind\":\"combo\",\"combo\":\"{}\",\"viewport\":\"{}\",\"steps\":{},",
                "\"candidates_hit_rate\":{:.4},\"refined_hit_rate\":{:.4},",
                "\"confirmed_rate\":{:.4},\"wrong\":{},\"uncertain\":{},\"none\":{},",
                "\"orb_triggers\":{},\"orb_trigger_rate\":{:.4},\"orb_forced\":{},",
                "\"p50_us\":{},\"p95_us\":{},\"max_us\":{},",
                "\"layers\":{{{}}},",
                "\"alloc\":{{\"live_bytes\":{},\"peak_bytes\":{}}},\"profile\":\"release\"}}"
            ),
            self.combo.label(),
            self.viewport,
            self.steps,
            self.candidate_rate(),
            self.refined_rate(),
            self.confirmed_rate(),
            self.tally.wrong,
            self.tally.uncertain,
            self.tally.none,
            self.tally.orb_triggers,
            self.orb_trigger_rate(),
            self.tally.orb_forced,
            self.total.p50_us,
            self.total.p95_us,
            self.total.max_us,
            layers,
            self.live_bytes,
            self.peak_bytes,
        )
    }
}

/// Runs the shipped funnel over `sequence` and reports where the step's time goes.
///
/// The verdict chain (`docs/30` §16.1's four gates, then `is_verifiable`) is executed but not
/// optimised: it is a handful of comparisons next to a full-resolution correlation, and running
/// it keeps the tallies honest about what the timed region actually did.
fn measure_funnel(sequence: &SyntheticSequence, combo: Combo) -> FunnelReport {
    let viewport = sequence.viewport;
    if sequence.len() == 0 {
        return FunnelReport::empty(viewport.label, combo);
    }

    let width = viewport.width as usize;
    let height = viewport.height as usize;
    let size = (viewport.width, viewport.height);
    let region = Rect::new(0, 0, viewport.width as i32, viewport.height as i32);
    let extent = viewport.height;
    let expected = sequence.step_rows as i32;
    let window = production_search_window(expected);
    let truth = sequence.step_rows as i32;
    let forced_budget = env_usize("SNAPCLIP_PERF1_ORB_VOTES", 8) as u32;

    // The sequence produces one luma byte per pixel (it was written for the digest probe). The
    // funnel wants a strictly packed BGRA observation, so the expansion happens here — outside the
    // timed region, together with the frame production it belongs to. Replicating the byte into
    // B/G/R makes §15.6's `(77R + 150G + 29B) >> 8` return exactly the document's value.
    let mut previous_doc = vec![0u8; width * height];
    let mut current_doc = vec![0u8; width * height];
    let mut previous_pixels = vec![0u8; width * height * 4];
    let mut current_pixels = vec![0u8; width * height * 4];
    sequence.fill(0, &mut previous_doc);
    expand_bgra(&previous_doc, &mut previous_pixels);
    let mut previous = Observation::new(previous_pixels.clone(), region, 0, size, Axis::Vertical)
        .expect("the probe's own frame is strictly packed");

    let mut scratch = Scratch::new();
    let mut tally = FunnelTally::default();
    let mut layer1_samples = Vec::with_capacity(sequence.len());
    let mut layer2_samples = Vec::new();
    let mut layer3_samples = Vec::new();
    let mut orb_samples = Vec::new();
    let mut total_samples = Vec::with_capacity(sequence.len());

    reset_peak();

    for n in 1..=sequence.len() {
        sequence.advance_frame(n, &previous_doc, &mut current_doc);
        expand_bgra(&current_doc, &mut current_pixels);
        // Outside the timed region: the frame source's stand-in and the observation the funnel
        // needs as input. `previous` is moved forward, so exactly one copy per step is paid.
        let current = Observation::new(current_pixels.clone(), region, n as i64, size, Axis::Vertical)
            .expect("the probe's own frame is strictly packed");

        let started = Instant::now();
        // `DigestOnly` is layer 1's attribution, not a fifth combination: it stops after the two
        // `primary_digests` calls that `candidates_1d` makes internally, because one timed call
        // cannot be split afterwards. It must therefore *skip* the candidate search — timing both
        // would report two digest passes plus the search as "the digest cost".
        let candidates = if combo == Combo::DigestOnly {
            let digests = displacement::primary_digests(&previous.view());
            std::hint::black_box(digests.len());
            let digests = displacement::primary_digests(&current.view());
            std::hint::black_box(digests.len());
            None
        } else {
            Some(candidates_1d(&previous.view(), &current.view(), expected, window))
        };
        let after_layer1 = Instant::now();
        let scored = if combo.depth() >= 2 {
            let candidates = candidates.as_ref().expect("layer 2 needs layer 1's candidates");
            let views = scratch.pool(&previous.view(), &current.view());
            Some(score_candidates_2d(views.previous(), views.current(), candidates))
        } else {
            None
        };
        let after_layer2 = Instant::now();
        let refined = match (combo.depth() >= 3, scored.as_ref()) {
            (true, Some(scored)) => {
                let views = scratch.full_resolution(&previous.view(), &current.view());
                refine_winner(views.previous(), views.current(), scored)
            }
            _ => None,
        };
        let after_layer3 = Instant::now();

        // The verdict chain, so the tallies describe a real decision rather than a stub.
        let (status, answer, margin) = match &scored {
            None => (Status::None, 0, 1.0f32),
            Some(scored) => match scored.iter().next().copied() {
                None => (Status::None, 0, 1.0f32),
                Some(best) => {
                    let margin = margin_of(best.score, outside_cell_second(scored, best.d).map(|c| c.score));
                    let answer = refined.map(|r| r.d).unwrap_or(best.d);
                    let gates = [
                        gate_geometry(best.d, extent),
                        gate_residual_gain(best.gain),
                        gate_support(best.tiles),
                        gate_margin(best.d, margin),
                    ];
                    let rejection = gates.iter().find_map(|outcome| match outcome {
                        GateOutcome::Reject(rejection) => Some(*rejection),
                        GateOutcome::Pass => None,
                    });
                    let status = match rejection {
                        Some(rejection) => rejection.status(),
                        None if is_verifiable(best.d, extent) => Status::Confirmed { d: answer },
                        None => Status::Uncertain { d: answer },
                    };
                    (status, answer, margin.unwrap_or(1.0))
                }
            },
        };

        // ORB: §15.4 ④'s trigger. A vote is quoted as per-vote × trigger rate, so a triggered vote
        // is timed here and subtracted from the step total — the combination's deterministic cost
        // is layers 1–3, and the second opinion is the thing that may or may not happen on top.
        let mut orb_elapsed = Duration::ZERO;
        if combo.votes() && combo.depth() >= 3 {
            let triggered = orb::should_run(margin, inside_prior(answer, expected), tally.uncertain);
            if triggered {
                let vote_started = Instant::now();
                let vote = orb::vote(&previous.view(), &current.view(), answer);
                std::hint::black_box(vote);
                orb_elapsed = vote_started.elapsed();
                orb_samples.push(elapsed_us(orb_elapsed));
                tally.orb_triggers += 1;
            }
        }
        let finished = Instant::now();

        if let Some(candidates) = candidates.as_ref() {
            tally.candidates_hit += u32::from(candidates.iter().any(|candidate| candidate.d == truth));
        }
        if combo.depth() >= 3 {
            tally.refined_hit += u32::from(refined.map(|r| r.d) == Some(truth));
        }
        match status {
            Status::Confirmed { d } => {
                tally.confirmed += 1;
                if d != truth {
                    tally.wrong += 1;
                }
            }
            Status::Uncertain { .. } => tally.uncertain += 1,
            Status::None => tally.none += 1,
        }

        layer1_samples.push(elapsed_us(after_layer1 - started));
        if combo.depth() >= 2 {
            layer2_samples.push(elapsed_us(after_layer2 - after_layer1));
        }
        if combo.depth() >= 3 {
            layer3_samples.push(elapsed_us(after_layer3 - after_layer2));
        }
        total_samples.push(elapsed_us(finished - started) - elapsed_us(orb_elapsed));

        std::mem::swap(&mut previous_doc, &mut current_doc);
        std::mem::swap(&mut previous_pixels, &mut current_pixels);

        // The trigger never fired, so there is no production vote to quote — but "ORB costs
        // nothing because it never runs" is not a measurement. Force `SNAPCLIP_PERF1_ORB_VOTES`
        // votes on this last frame pair, after every sample for this step has been taken: a vote is
        // heavy enough (hundreds of milliseconds) that running them mid-sequence would depress the
        // other layers' numbers through cache and clock effects, and the per-vote figure does not
        // need to come from step `n` to be the per-vote figure. `orb_forced` says how many of the
        // samples are these, not production, votes.
        if n == sequence.len()
            && combo.votes()
            && combo.depth() >= 3
            && orb_samples.is_empty()
        {
            for _ in 0..forced_budget {
                let vote_started = Instant::now();
                let vote = orb::vote(&previous.view(), &current.view(), expected);
                std::hint::black_box(vote);
                orb_samples.push(elapsed_us(vote_started.elapsed()));
                tally.orb_forced += 1;
            }
        }

        previous = current;
    }

    FunnelReport {
        viewport: viewport.label,
        combo,
        steps: sequence.len(),
        tally,
        layer1: distribution(layer1_samples),
        layer2: distribution(layer2_samples),
        layer3: distribution(layer3_samples),
        orb: distribution(orb_samples),
        total: distribution(total_samples),
        live_bytes: live_bytes(),
        peak_bytes: peak_bytes(),
    }
}

/// Expands the sequence's one-luma-byte-per-pixel frame into a strictly packed BGRA observation.
///
/// The sequence predates the funnel probe: it was written for the digest measurement, where one
/// byte per pixel was the whole point (the digest reads bytes, not channels). The funnel needs the
/// observation the shipped code takes, so the two forms have to meet somewhere; they meet here,
/// outside every timed region, because the frame source — not the estimator — owns this cost.
fn expand_bgra(luma: &[u8], bgra: &mut [u8]) {
    assert_eq!(bgra.len(), luma.len() * 4, "one luma byte per pixel, four bytes per pixel out");
    for (pixel, value) in bgra.chunks_exact_mut(4).zip(luma.iter().copied()) {
        pixel[0] = value;
        pixel[1] = value;
        pixel[2] = value;
        pixel[3] = 0xFF;
    }
}

// ---------------------------------------------------------------------------------------
// Allocator accounting
// ---------------------------------------------------------------------------------------

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

fn live_bytes() -> usize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

fn peak_bytes() -> usize {
    PEAK_BYTES.load(Ordering::Relaxed)
}

/// Moves the high-water mark to the current live total, so later `peak - live` measures what
/// the timed region itself added.
fn reset_peak() {
    PEAK_BYTES.store(live_bytes(), Ordering::Relaxed);
}

struct CountingAllocator;

impl CountingAllocator {
    fn record_growth(bytes: usize) {
        let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
        PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
    }

    fn record_shrink(bytes: usize) {
        LIVE_BYTES.fetch_sub(bytes, Ordering::Relaxed);
    }
}

// SAFETY: every method forwards to `System` unchanged; the counters are the only added work
// and they use relaxed atomics, which cannot allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            Self::record_growth(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            Self::record_growth(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        Self::record_shrink(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() {
            if new_size >= layout.size() {
                Self::record_growth(new_size - layout.size());
            } else {
                Self::record_shrink(layout.size() - new_size);
            }
        }
        new_pointer
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// ---------------------------------------------------------------------------------------
// Driver surface
// ---------------------------------------------------------------------------------------

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn append_json_line(path: &Path, line: &str) {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|error| panic!("cannot open {} for append: {error}", path.display()));
    writeln!(file, "{line}")
        .unwrap_or_else(|error| panic!("cannot append to {}: {error}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_reports_zero_for_an_empty_sequence() {
        let sequence = SyntheticSequence::scripted(VIEWPORT_1080P, 0, STEP_ROWS);
        let report = measure(&sequence);

        assert_eq!(report.steps, 0, "an empty sequence must report zero steps");
        assert_eq!(report.truth_steps, 0, "an empty sequence has no scripted truth");
        assert_eq!(report.total.max_us, 0, "an empty sequence must report a zero maximum");
        assert_eq!(report.top_k_hits, 0, "an empty sequence can hit nothing");
    }

    #[test]
    fn the_probe_reproduces_a_known_synthetic_step_count() {
        let sequence = SyntheticSequence::scripted(VIEWPORT_1080P, 25, STEP_ROWS);
        let report = measure(&sequence);

        assert_eq!(report.steps, 25, "the report must carry the scripted step count");
        assert_eq!(report.truth_steps, 25, "every step has a scripted displacement to compare against");
        assert_eq!(
            report.top_k_hits, 25,
            "the scripted shift must land in the candidate set on every step, otherwise the \
             digest/search pair is measuring something other than a displacement estimate"
        );
    }

    /// One scenario per process (`docs/31` P0.03, "每场景独立进程"), driven by
    /// `tools/p0-03-matching-cost.ps1`.
    #[test]
    #[ignore = "P0.03: one scenario per process in release; drive it with tools/p0-03-matching-cost.ps1"]
    fn perf1_measures_one_viewport() {
        let label = std::env::var("SNAPCLIP_PERF1_VIEWPORT").unwrap_or_else(|_| "1080p".to_string());
        let viewport = match label.as_str() {
            "1080p" => VIEWPORT_1080P,
            "1440p" => VIEWPORT_1440P,
            "4k" => VIEWPORT_4K,
            other => panic!("unknown SNAPCLIP_PERF1_VIEWPORT {other}"),
        };
        let variant_name = std::env::var("SNAPCLIP_PERF1_VARIANT").unwrap_or_else(|_| "full".to_string());
        let variant = Variant::from_name(&variant_name)
            .unwrap_or_else(|| panic!("unknown SNAPCLIP_PERF1_VARIANT {variant_name}"));
        let steps = env_usize("SNAPCLIP_PERF1_STEPS", 1000);
        let step_rows = env_usize("SNAPCLIP_PERF1_STEP_ROWS", STEP_ROWS as usize) as i64;

        let sequence = SyntheticSequence::scripted(viewport, steps, step_rows);
        let report = measure_variant(&sequence, variant);
        let line = report.json_line(variant.label());
        println!("{line}");
        if let Some(path) = std::env::var_os("SNAPCLIP_PERF1_OUT") {
            append_json_line(Path::new(&path), &line);
        }

        assert_eq!(report.steps, steps, "the report must carry the scripted step count");
        assert!(
            report.top_k_hit_rate() > 0.99,
            "the scripted shift must land in the top-{CANDIDATES} candidates: hit rate {:.4}",
            report.top_k_hit_rate()
        );
    }

    /// One funnel scenario per process, driven by `tools/p0-03-matching-cost.ps1` with
    /// `SNAPCLIP_PERF1_COMBO` in `{digest, l1, l12, l123, l123-orb}`.
    #[test]
    #[ignore = "P0.03: one scenario per process in release; drive it with tools/p0-03-matching-cost.ps1"]
    fn perf1_measures_one_funnel_combo() {
        let label = std::env::var("SNAPCLIP_PERF1_VIEWPORT").unwrap_or_else(|_| "1080p".to_string());
        let viewport = match label.as_str() {
            "1080p" => VIEWPORT_1080P,
            "1440p" => VIEWPORT_1440P,
            "4k" => VIEWPORT_4K,
            other => panic!("unknown SNAPCLIP_PERF1_VIEWPORT {other}"),
        };
        let combo_name = std::env::var("SNAPCLIP_PERF1_COMBO").unwrap_or_else(|_| "l1".to_string());
        let combo = Combo::from_name(&combo_name)
            .unwrap_or_else(|| panic!("unknown SNAPCLIP_PERF1_COMBO {combo_name}"));
        let steps = env_usize("SNAPCLIP_PERF1_STEPS", 1000);
        let step_rows = env_usize("SNAPCLIP_PERF1_STEP_ROWS", STEP_ROWS as usize) as i64;

        let sequence = SyntheticSequence::scripted(viewport, steps, step_rows);
        let report = measure_funnel(&sequence, combo);
        let line = report.json_line();
        println!("{line}");
        if let Some(path) = std::env::var_os("SNAPCLIP_PERF1_OUT") {
            append_json_line(Path::new(&path), &line);
        }

        assert_eq!(report.steps, steps, "the report must carry the scripted step count");
        if combo == Combo::DigestOnly {
            // The attribution combo stops before the candidate search, so it has no candidates and
            // no rate to report. Pinning that here keeps the two meanings of "no candidates"
            // (never searched vs searched and found nothing) from being confused in the output.
            assert_eq!(
                report.tally.candidates_hit, 0,
                "the digest attribution must not search for candidates"
            );
            return;
        }
        assert_eq!(
            report.candidate_rate(),
            1.0,
            "the scripted shift must reach the candidate set on every step, otherwise the \
             combination is timing something other than a displacement estimate"
        );
        if combo.depth() >= 3 {
            assert_eq!(
                report.refined_rate(),
                1.0,
                "the third layer must land on the scripted shift on every step"
            );
            assert_eq!(
                report.tally.wrong, 0,
                "a confirmed step that disagrees with the script is a wrong answer, and this \
                 sequence is designed to be answerable"
            );
        }
    }

    /// The device's own floor for the funnel: no steps, no cost. Not a special case for the
    /// test's benefit — a run with no steps genuinely has no distribution.
    #[test]
    fn the_funnel_probe_reports_zero_for_an_empty_sequence() {
        let sequence = SyntheticSequence::scripted(VIEWPORT_1080P, 0, STEP_ROWS);
        let report = measure_funnel(&sequence, Combo::Layer123);

        assert_eq!(report.steps, 0, "an empty sequence must report zero steps");
        assert_eq!(report.total.max_us, 0, "an empty sequence must report a zero maximum");
        assert_eq!(report.tally.confirmed, 0, "an empty sequence can confirm nothing");
        assert_eq!(report.candidate_rate(), 0.0, "and it has no hit rate to report");
    }

    /// The funnel device must recover the scripted step before any of its timings are worth
    /// quoting — a cheap funnel that loses the truth is not a cheap funnel.
    #[test]
    fn the_funnel_probe_recovers_the_scripted_step() {
        let sequence = SyntheticSequence::scripted(VIEWPORT_1080P, 25, STEP_ROWS);
        let report = measure_funnel(&sequence, Combo::Layer123);

        assert_eq!(report.steps, 25, "the report must carry the scripted step count");
        assert_eq!(
            report.tally.candidates_hit, 25,
            "the scripted shift must reach the candidate set on every step"
        );
        assert_eq!(
            report.tally.refined_hit, 25,
            "the third layer must land on the scripted shift on every step"
        );
        assert_eq!(report.tally.wrong, 0, "and no confirmed step may disagree with the script");
    }

    /// The attribution combo must measure layer 1's digest without also paying for the search it
    /// is meant to attribute: the two numbers have to differ in exactly one thing.
    #[test]
    fn the_digest_attribution_is_layer_one_without_the_search() {
        let sequence = SyntheticSequence::scripted(VIEWPORT_1080P, 5, STEP_ROWS);
        let digest = measure_funnel(&sequence, Combo::DigestOnly);
        let layer1 = measure_funnel(&sequence, Combo::Layer1);

        assert_eq!(
            digest.tally.candidates_hit, 0,
            "the digest attribution must not search for candidates"
        );
        assert_eq!(
            digest.tally.confirmed, 0,
            "with no candidates there is nothing to confirm, and the report must say so rather \
             than borrow layer 1's verdict"
        );
        assert_eq!(
            layer1.tally.candidates_hit, 5,
            "layer 1 itself must find the scripted shift on every step"
        );
        assert!(
            digest.layer1.p50_us > 0,
            "the digest attribution still has to be timed: a zero here means the combo measured \
             nothing at all"
        );
    }

    /// The variant is only worth quoting if it produces the same candidate set as the
    /// straightforward one; a cheaper digest that loses the truth is not a cheaper digest.
    #[test]
    fn the_revealed_rows_variant_finds_the_same_scripted_shift() {
        let sequence = SyntheticSequence::scripted(VIEWPORT_1080P, 25, STEP_ROWS);
        let full = measure_variant(&sequence, Variant::FullFrameDigest);
        let revealed = measure_variant(&sequence, Variant::RevealedRowsOnly);

        assert_eq!(revealed.steps, full.steps, "both variants run the same script");
        assert_eq!(
            revealed.top_k_hits, full.top_k_hits,
            "reusing the overlapping rows' digests must not change which candidates are found: \
             full hit {} of {}, revealed hit {} of {}",
            full.top_k_hits, full.truth_steps, revealed.top_k_hits, revealed.truth_steps
        );
        assert_eq!(revealed.top_k_hits, 25, "and both must find the scripted shift everywhere");
    }
}
