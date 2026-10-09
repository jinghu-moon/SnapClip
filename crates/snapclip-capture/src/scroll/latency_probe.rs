//! `docs/31` `P5.06` (`E-PERF-4`, `docs/30` §23.1/§23.3): what the preview's derivation costs.
//!
//! First principle **F-08** says the preview is a *consumer*: its slowness must not become the
//! capture loop's slowness. That is a statement about where the work happens, and §19.2 makes it a
//! statement about *how much* work there is: the preview is **windowed** — it derives the thumbnail
//! of one window and, in the words of §19.2, "窗口之外**不生成任何像素**". So the cost to measure
//! is `O(新增面积)`, and §23.1 asks for the number directly: *"测 `O(新增面积)` 降采样在 1500×540
//! 新增条带上的耗时"*.
//!
//! # The two kinds of cost
//!
//! * `COST_BAND` — [`thumbnail_rows`] alone, on a band the size §23.1 names. This is the marginal
//!   cost of one more step's worth of rows, and it is what the 10 Hz ceiling in §19.3 constraint 4
//!   is spending.
//! * `COST_WINDOW` — [`refresh_window`] end to end on a real canvas: `drop_previews` + the canvas
//!   read + the box filter + the insert. The difference between the two is the read and the store
//!   bookkeeping, and the *reason* to record both is that §19.2's window is rebuilt from scratch on
//!   every refresh: a number that only measured the filter would hide the part that is not the
//!   filter.
//!
//! # What this probe does not measure, and why that is `E-PERF-4`'s other half
//!
//! `E-PERF-4`'s second question is the *consumer's* cost — "测 5/10/20/30 Hz 下覆盖层线程的最大同步
//! 工作时间" — and that needs a D2D device, so it is measured in
//! `crates/snapclip-capture/src/windows/win/d2d/tests.rs` (`the_overlay_thread_never_blocks_longer_
//! than_eight_ms`) rather than here. The dependency rule of §28.4 is what splits the two: `scroll/`
//! may not name the platform, so a probe that needs a renderer cannot live in this directory. The
//! same file cannot measure both halves, and the two halves are reported side by side in
//! `docs/30` §23.3.3.
//!
//! # Why the derivation is not called from the loop yet
//!
//! [`refresh_window`] has no production caller today. `preview.rs`'s module doc assigns it to
//! "`P5.03`'s overlay, which asks for the window it is about to draw", and that assignment cannot be
//! carried out: the function needs `&mut RecoveredImage` (the canvas), the canvas belongs to the
//! driver thread (`ScrollSession::canvas_mut`), and §21.2's forbidden list for the overlay thread
//! names "降采样大图" explicitly. Measuring the cost here is what makes the eventual wiring a
//! decision with a number attached; this probe is the number.

use std::path::Path;
use std::time::Instant;

use crate::geometry::Rect;
use crate::scroll::canvas::{MemoryBudget, RecoveredImage};
use crate::scroll::observation::{Axis, Observation};
use crate::scroll::perf_probe::append_json_line;
use crate::scroll::preview::{preview_scale, refresh_window, thumbnail_rows};

/// The band §23.1 names, in rows: 1500 × 540 is the "新增条带" its question is about.
const BAND_ROWS: u64 = 540;

/// The widths to measure at, and why these two: 1500 is §22.6's 内容尺寸 and 3840 is this machine's
/// display width at 150% scaling (`docs/30` §24.1.1), i.e. the widest canvas a user here can
/// produce. `preview_scale` maps them to 12 and 30, so the two rows differ by 6.25× in output
/// pixels and by 25× in input pixels per thumbnail pixel — the scale factor is the whole story of
/// this cost.
const CROSS_LENS: [u64; 2] = [1500, 3840];

/// How many timed iterations each measurement runs. The probe is release-only and `#[ignore]`d, so
/// this is not a gate's budget: it is what makes a P95 mean something on a machine with a
/// background load.
const ITERATIONS: u32 = 30;

/// `CostReport::kind` for [`thumbnail_rows`] alone.
const COST_BAND: u64 = 1;

/// `CostReport::kind` for [`refresh_window`] end to end.
const COST_WINDOW: u64 = 2;

/// Which kind `perf4_measures_the_costs` was asked for, when it is driven one kind per process.
const KIND_ENV: &str = "SNAPCLIP_PERF4_KIND";

/// The width `perf4_measures_the_costs` was asked for.
const CROSS_ENV: &str = "SNAPCLIP_PERF4_CROSS_LEN";

/// Where the JSON lines go.
const OUT_ENV: &str = "SNAPCLIP_PERF4_OUT";

/// `kind` for the driver's own latencies, which are measured in
/// `loop_control::tests::latency_measures_one_run` and written with [`DriverLatencyReport`].
pub(crate) const DRIVER_LATENCY_KIND: u64 = 3;

/// The driver thread's two latencies for one run (§23.2's `Scroll response` and `Stop latency`).
///
/// A third `kind` on the same JSONL file rather than a second file: one script reads all of
/// `E-PERF-4`'s numbers, and `docs/30` §23.3.3 reports them in one table. `stop_ns` has no P
/// distribution because a session is stopped once.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DriverLatencyReport {
    pub pid: u32,
    pub cross_len: u64,
    pub extent: u64,
    /// Steps the loop took before it stopped.
    pub steps: u32,
    /// Injections the actuator saw. `injects >= steps` when the last step was injected but the
    /// session ended before its announcement.
    pub injects: u32,
    pub scroll_p50_ns: u64,
    pub scroll_p95_ns: u64,
    pub scroll_max_ns: u64,
    /// `ScrollController::stop()` → the driver published `Ended`. See the case's doc for what this
    /// endpoint is and is not.
    pub stop_ns: u64,
    /// `docs/30` §23.3's threshold for `scroll_p95_ns`, carried in the record so that the comparison
    /// travels with the number instead of living only in a reader's head.
    pub threshold_ns: u64,
    pub meets_threshold: bool,
    /// Updates the port dropped during the run, from `PreviewStream::dropped`.
    ///
    /// Recorded because it is the metric behind a `P5.06` finding: a dropped update is normal for
    /// every kind that a later one supersedes, and it was **not** acceptable for the terminal one
    /// (a consumer waiting on `Ended` waited forever). This number staying nonzero is expected; what
    /// must never happen again is the terminal update being part of it.
    pub preview_dropped: u64,
}

impl DriverLatencyReport {
    pub(crate) fn json_line(&self) -> String {
        format!(
            "{{\"pid\":{},\"kind\":{},\"cross_len\":{},\"extent\":{},\"steps\":{},\"injects\":{},\
             \"scroll_p50_ns\":{},\"scroll_p95_ns\":{},\"scroll_max_ns\":{},\"stop_ns\":{},\
             \"threshold_ns\":{},\"meets_threshold\":{},\"preview_dropped\":{}}}",
            self.pid,
            DRIVER_LATENCY_KIND,
            self.cross_len,
            self.extent,
            self.steps,
            self.injects,
            self.scroll_p50_ns,
            self.scroll_p95_ns,
            self.scroll_max_ns,
            self.stop_ns,
            self.threshold_ns,
            self.meets_threshold,
            self.preview_dropped,
        )
    }
}

/// One measurement, in nanoseconds, plus the shape of the work it timed.
#[derive(Debug, Clone, Copy)]
struct CostReport {
    pid: u32,
    /// [`COST_BAND`] or [`COST_WINDOW`] — numeric so that one fixed-format reader serves both.
    kind: u64,
    cross_len: u64,
    /// `preview_scale(cross_len)`: the box filter's edge, and the whole story of this cost.
    scale: u32,
    /// Input rows read per update: [`BAND_ROWS`] for the band kind, the canvas' length for the window
    /// kind.
    rows: u64,
    /// Derived bytes per update — the *output* side of `O(新增面积)`. Taken from the store
    /// (`BandStore::resident_preview_bytes`) rather than computed, so a window refresh that derived
    /// nothing cannot report a plausible non-zero number.
    px_per_update: u64,
    iterations: u32,
    p50_ns: u64,
    p95_ns: u64,
    max_ns: u64,
}

impl CostReport {
    fn json_line(&self) -> String {
        format!(
            "{{\"pid\":{},\"kind\":{},\"cross_len\":{},\"scale\":{},\"rows\":{},\
             \"px_per_update\":{},\"iterations\":{},\"p50_ns\":{},\"p95_ns\":{},\"max_ns\":{}}}",
            self.pid,
            self.kind,
            self.cross_len,
            self.scale,
            self.rows,
            self.px_per_update,
            self.iterations,
            self.p50_ns,
            self.p95_ns,
            self.max_ns,
        )
    }
}

/// `(p50, p95, max)` of a sample set, in place.
///
/// The index is `ceil(n × q) − 1`, which is the nearest-rank definition, and the definition matters
/// for a p95 over 30 samples: it selects the 29th of 30, i.e. the second slowest — not the maximum
/// (that is `max`) and not the median. A 30-sample p95 is a weak statistic and it is reported as
/// one: `max` is the number that decides whether a frame budget was blown.
///
/// Shared with the two other `P5.06` cases (`loop_control`'s driver run and the renderer's budget)
/// on purpose: three copies of a percentile definition is three chances for the threshold and the
/// number to disagree about what `p95` means.
pub(crate) fn percentiles(samples: &mut [u64]) -> (u64, u64, u64) {
    assert!(!samples.is_empty(), "a distribution needs samples");
    samples.sort_unstable();
    let last = samples.len() - 1;
    let nearest_rank = |fraction: f64| -> usize {
        let raw = (samples.len() as f64 * fraction).ceil() as usize;
        raw.saturating_sub(1).min(last)
    };
    (
        samples[nearest_rank(0.50)],
        samples[nearest_rank(0.95)],
        samples[last],
    )
}

/// A page-like BGRA buffer: text-like rows with enough high-frequency content that the box filter
/// cannot be short-circuited by constant data.
fn page_rows(cross_len: u64, rows: u64, seed: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (cross_len * rows * 4) as usize];
    let mut state = seed | 1;
    for row in 0..rows {
        for column in 0..cross_len {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let base = ((row * cross_len + column) * 4) as usize;
            pixels[base] = state as u8;
            pixels[base + 1] = (state >> 8) as u8;
            pixels[base + 2] = (state >> 16) as u8;
            pixels[base + 3] = 0xFF;
        }
    }
    pixels
}

/// The marginal cost of one more step's rows: [`thumbnail_rows`] on a [`BAND_ROWS`]-row band.
fn measure_band(cross_len: u64, iterations: u32) -> CostReport {
    let scale = preview_scale(cross_len);
    let band = page_rows(cross_len, BAND_ROWS, 0x5EED);

    let mut derived = thumbnail_rows(&band, cross_len, scale);
    let mut samples = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        let start = Instant::now();
        derived = thumbnail_rows(&band, cross_len, scale);
        samples.push(start.elapsed().as_nanos() as u64);
    }

    let (p50_ns, p95_ns, max_ns) = percentiles(&mut samples);
    CostReport {
        pid: std::process::id(),
        kind: COST_BAND,
        cross_len,
        scale,
        rows: BAND_ROWS,
        px_per_update: derived.len() as u64,
        iterations,
        p50_ns,
        p95_ns,
        max_ns,
    }
}

/// How many steps of [`BAND_ROWS`] the window measurement appends before measuring, and why eight:
/// the window is 540 rows and the origin viewport is 1080, so a canvas of 1080 + 8 × 120 = 2040 rows
/// is past the point where the window the loop would ask for (`primary_len − 540`) is inside the
/// canvas — the refresh is then a real one and not a clamp.
const WINDOW_CANVAS_STEPS: u64 = 8;

/// The cost of a refresh end to end: `drop_previews` + the canvas read + the box filter + the insert.
fn measure_window(cross_len: u64, iterations: u32) -> CostReport {
    let scale = preview_scale(cross_len);
    let extent = 1080u64;
    let budget = MemoryBudget::for_viewport(cross_len, extent);
    let frame_pixels = page_rows(cross_len, extent, 0x5EED);
    let frame = Observation::new(
        frame_pixels,
        Rect::from_origin_size(
            crate::geometry::Point::new(0, 0),
            cross_len as i32,
            extent as i32,
        ),
        0,
        (cross_len as u32, extent as u32),
        Axis::Vertical,
    )
    .expect("the window fixture is a packed viewport");

    let mut canvas = RecoveredImage::new(Axis::Vertical, cross_len, budget);
    canvas.start(&frame);
    for _ in 0..WINDOW_CANVAS_STEPS {
        canvas.append_confirmed(&frame, 120);
    }

    let first_row = canvas.primary_len() - BAND_ROWS;
    let mut derived_rows = refresh_window(&mut canvas, scale, first_row, BAND_ROWS)
        .expect("the window is inside the canvas it was asked for");
    let mut samples = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        let start = Instant::now();
        derived_rows = refresh_window(&mut canvas, scale, first_row, BAND_ROWS)
            .expect("the window is inside the canvas it was asked for");
        samples.push(start.elapsed().as_nanos() as u64);
    }

    assert!(
        derived_rows > 0,
        "a {BAND_ROWS}-row window at scale {scale} derived no whole thumbnail rows"
    );
    let (p50_ns, p95_ns, max_ns) = percentiles(&mut samples);
    CostReport {
        pid: std::process::id(),
        kind: COST_WINDOW,
        cross_len,
        scale,
        rows: canvas.primary_len(),
        px_per_update: canvas.bands().resident_preview_bytes(),
        iterations,
        p50_ns,
        p95_ns,
        max_ns,
    }
}

/// The device self-check: one iteration of each kind, cheap enough to run in the gate.
///
/// It is a self-check and not a measurement: a single iteration under a debug build says nothing
/// about the production cost. What it does say is that both kinds produce a **non-zero** number and
/// that the window kind actually left a preview band in the store — a probe whose derivation
/// silently returned `Ok(0)` (the window was not aligned to whole thumbnail rows) would report a
/// beautiful 0 ns and prove nothing.
#[test]
fn the_probe_reports_a_band_cost_and_a_window_cost() {
    let band = measure_band(CROSS_LENS[0], 1);
    assert!(band.max_ns > 0, "a box filter over 810,000 pixels took no time: {band:?}");
    assert!(
        band.px_per_update > 0,
        "the band measurement did not derive anything: {band:?}"
    );

    let window = measure_window(CROSS_LENS[0], 1);
    assert!(
        window.max_ns > 0,
        "the window refresh took no time: {window:?}"
    );
    assert!(
        window.px_per_update > 0,
        "the window refresh left no thumbnail rows in the canvas: {window:?}"
    );
}

/// The measured costs, one JSON line each.
///
/// Release-only and `#[ignore]`d, driven by `tools/p5-06-latency.ps1`; it prints its lines so the
/// gate log keeps them, and appends them to `SNAPCLIP_PERF4_OUT` when the driver asked for a file.
#[test]
#[ignore = "P5.06 measures release-only costs; drive it with tools/p5-06-latency.ps1"]
fn perf4_measures_the_costs() {
    let kinds: Vec<u64> = match std::env::var(KIND_ENV) {
        Ok(value) => value
            .split(',')
            .filter_map(|kind| kind.trim().parse().ok())
            .collect(),
        Err(_) => vec![COST_BAND, COST_WINDOW],
    };
    let cross_lens: Vec<u64> = match std::env::var(CROSS_ENV) {
        Ok(value) => value
            .split(',')
            .filter_map(|cross| cross.trim().parse().ok())
            .collect(),
        Err(_) => CROSS_LENS.to_vec(),
    };

    for cross_len in cross_lens {
        for kind in &kinds {
            let report = match *kind {
                COST_BAND => measure_band(cross_len, ITERATIONS),
                COST_WINDOW => measure_window(cross_len, ITERATIONS),
                other => panic!("unknown cost kind {other}"),
            };
            let line = report.json_line();
            println!("[P5.06] {line}");
            if let Some(path) = std::env::var_os(OUT_ENV) {
                append_json_line(Path::new(&path), &line);
            }
        }
    }
}
