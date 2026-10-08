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
/// prototype's choice; a wider lane fold would be cheaper and collide more, and `P1.02` owns
/// that decision. Every cost quoted from this probe is therefore the cost of *this* digest.
fn row_digest(row: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in row {
        h ^= byte as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// How many rows agree exactly when the observation is attributed a displacement of `shift`
/// (`docs/30` §15.4 layer 1's "support").
fn support_at(previous: &[u64], current: &[u64], shift: i64) -> u32 {
    let height = current.len() as i64;
    let (previous_first, current_first) = if shift >= 0 { (shift, 0) } else { (0, -shift) };
    let overlap = height - shift.abs();
    if overlap <= 0 {
        return 0;
    }
    let mut support = 0u32;
    for index in 0..overlap {
        let p = previous[(previous_first + index) as usize];
        let c = current[(current_first + index) as usize];
        if p == c {
            support += 1;
        }
    }
    support
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
