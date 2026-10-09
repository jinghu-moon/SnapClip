//! `docs/31` `P4.07` (`E-MEM-1`): does the memory ceiling grow with the image?
//!
//! First principle **G3** (`docs/30` §2.1) says it must not. This is the measurement that makes
//! that a fact instead of a claim, and it is the one place where V2's structure differs from every
//! reference implementation at once: PixPin keeps a canvas in a single contiguous
//! `QImage::Format_RGB32` (`docs/26`), so its ceiling is `W × H × 4` and a 502,649-row capture is
//! a 2 GiB allocation. `docs/30` §22.6's table therefore ends with "同一上限" three times, and the
//! falsifiable form of that column is: **the peak across 10,000 / 30,000 / 100,000 rows differs by
//! at most 10%**.
//!
//! # Why one process per length
//!
//! `peak` is a process-global high-water mark: it can only be lowered by ending the process. A
//! three-length loop in one process would report the 100,000-row peak three times and the 10% band
//! would hold trivially — a measurement that cannot fail is not a measurement. `P0.03`'s device
//! (`perf_probe`) has the same problem and the same answer, and `tools/p4-07-memory.ps1` is the
//! same shape of driver.
//!
//! # Why the numbers are recorded as four columns and not one
//!
//! The methodology discipline §22.6 quotes from `benchmark-support/README.md` is
//! **"heap traffic cannot prove a drop in space (when the storage moved to an OS mapping)"**. Two
//! consequences, both of them visible in the report:
//!
//! * `allocated` (cumulative bytes ever handed out) and `peak` (the high-water mark) must be
//!   recorded **separately**, because a design that streams can have an enormous traffic and a
//!   small space at the same time. That is the shape of a good answer here, and one number could
//!   not tell the two apart.
//! * the **spill file size** must be its own column, because a canvas that stopped allocating
//!   might have moved its rows to disk rather than stopped needing them. `E-MEM-1`'s claim is
//!   about memory, and a probe that reported only memory would be reporting the half that flatters
//!   the design.
//!
//! # What is measured, and what is not
//!
//! The workload is the canvas itself — `RecoveredImage` appending one 120-row band per step, with
//! the production eviction rule (`relieve`, §17.5 ③) run after each one. 120 rows is not a round
//! number: it is the measured Chromium response to eight wheel notches (`P0.09`, §24.6.2), so the
//! step count below is the step count a real session would take.
//!
//! What this probe does **not** measure: the frame source (a WGC read-back is `P2.03`'s cost and a
//! step-lifetime buffer, not a resident one), the estimator (`P0.03`), and the export path
//! (`P4.02`/`P4.03`). Every one of those is measured elsewhere; adding them here would make the
//! number less attributable without making it more true.

use std::path::Path;

use crate::geometry::Rect;
use crate::scroll::alloc_probe;
use crate::scroll::canvas::{MemoryBudget, RecoveredImage, StepTally};
use crate::scroll::observation::{Axis, Observation};
use crate::scroll::perf_probe::append_json_line;

/// The canvas width §22.6's length rows are quoted at ("1500 px 宽").
const CROSS_LEN: u64 = 1500;

/// A 1080p viewport. §22.3's default budget is eight of these.
const EXTENT: u64 = 1080;

/// One step, in rows: the measured Chromium response to eight wheel notches (`P0.09`).
const STEP_ROWS: u64 = 120;

/// The three lengths `docs/31` P4.07 fixes.
const LENGTHS: [u64; 3] = [10_000, 30_000, 100_000];

/// The length the device self-check runs at.
///
/// It has to be a length that **spills**, and the arithmetic that fixes the smallest such length is
/// worth writing down: §22.3's budget is eight viewports — `1500 × 1080 × 4 × 8 = 51,840,000 B` — the
/// first frame writes a whole 1,080-row viewport (6,480,000 B) and every step after it writes a
/// 120-row band (720,000 B), so the store goes over budget during step 64, i.e. at 8,760 rows. A
/// shorter self-check would exercise the allocation pattern and not the eviction rule, which is the
/// rule the claim rests on — so the self-check runs at the shortest length the task fixes, and there
/// is no cheaper length that would still mean anything.
const SELF_CHECK_LENGTH: u64 = 10_000;

/// The band §22.6's claim is allowed to move within.
const PEAK_BAND: f64 = 0.10;

/// Which length this process should measure, for the one-length child mode.
const LENGTH_ENV: &str = "SNAPCLIP_MEM1_LENGTH";

/// Where a child appends its one JSON line, for the driver and for the parent test.
const OUT_ENV: &str = "SNAPCLIP_MEM1_OUT";

/// One length's measurement, in full.
///
/// Fourteen numbers rather than one, because the claim has four ways of being misread: `live` and
/// `peak` can be conflated, `allocated` can be dropped, the spill file can be ignored, and
/// `single_buffer` is the number the design is being compared against (what a contiguous canvas
/// would have to allocate for this length).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LengthReport {
    /// The process that produced it. `peak` is process-global, so this is what makes three reports
    /// three measurements instead of one repeated three times.
    pid: u32,
    /// The length the workload was asked for: one of [`LENGTHS`]. This is the independent variable,
    /// so it is the requested value and not the achieved one.
    length: u64,
    /// The rows the canvas actually ended up holding. Within one [`STEP_ROWS`] step of `length` at
    /// or above it, because the loop stops at the first step that reaches the target.
    rows: u64,
    cross_len: u64,
    extent: u64,
    budget: u64,
    steps: u64,
    /// `cross_len × rows × 4`: the single contiguous canvas §17.1 refuses to build.
    single_buffer: u64,
    /// Live bytes before the workload opened — the process baseline the peak is compared against.
    live_before: u64,
    live: u64,
    peak: u64,
    allocated: u64,
    allocations: u64,
    /// Canvas bytes resident at the end (`BandStore::resident_bytes`).
    resident: u64,
    /// Bytes in the spill file at the end (`BandStore::spill_file_bytes`, `P4.06`).
    spill_bytes: u64,
}

impl LengthReport {
    /// The high-water mark this workload is responsible for.
    ///
    /// The comparison in the parent test is on this and not on `peak`, because `peak` is the whole
    /// process's: the test harness, the argv block and the module's own constants are in it. Those
    /// are the same in all three children, so the band would hold either way — but subtracting the
    /// baseline is what makes the reported number attributable to the canvas, which is the thing
    /// `G3` is about.
    fn peak_over_baseline(&self) -> u64 {
        self.peak.saturating_sub(self.live_before)
    }

    fn json_line(&self) -> String {
        format!(
            "{{\"pid\":{},\"length\":{},\"rows\":{},\"cross_len\":{},\"extent\":{},\"budget\":{},\
             \"steps\":{},\"single_buffer\":{},\"live_before\":{},\"live\":{},\"peak\":{},\
             \"allocated\":{},\"allocations\":{},\"resident\":{},\"spill_bytes\":{}}}",
            self.pid,
            self.length,
            self.rows,
            self.cross_len,
            self.extent,
            self.budget,
            self.steps,
            self.single_buffer,
            self.live_before,
            self.live,
            self.peak,
            self.allocated,
            self.allocations,
            self.resident,
            self.spill_bytes,
        )
    }
}

/// Reads one integer out of a `json_line`.
///
/// A fixed-format reader, not a JSON parser: the only producer is [`LengthReport::json_line`] above,
/// and a parser would be a dependency and a second grammar for one line of fourteen integers. It
/// finds `"key":` and reads the digits after it.
fn field(line: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\":");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// A report as a child wrote it, or `None` for a line that is not one.
fn parse_report(line: &str) -> Option<LengthReport> {
    Some(LengthReport {
        pid: field(line, "pid")? as u32,
        length: field(line, "length")?,
        rows: field(line, "rows")?,
        cross_len: field(line, "cross_len")?,
        extent: field(line, "extent")?,
        budget: field(line, "budget")?,
        steps: field(line, "steps")?,
        single_buffer: field(line, "single_buffer")?,
        live_before: field(line, "live_before")?,
        live: field(line, "live")?,
        peak: field(line, "peak")?,
        allocated: field(line, "allocated")?,
        allocations: field(line, "allocations")?,
        resident: field(line, "resident")?,
        spill_bytes: field(line, "spill_bytes")?,
    })
}

/// A page-like viewport: runs of "ink" separated by "paper".
///
/// Neither constant nor random. A constant frame would make the band checksum a formality and the
/// spill file a single repeated page; a random one would make the spill file incompressible for a
/// reason that has nothing to do with the design. Text-like runs are what the canvas will actually
/// hold, and they are what a real session's spill file looks like.
fn viewport_frame() -> Observation {
    let cross = CROSS_LEN as usize;
    let rows = EXTENT as usize;
    let mut pixels = vec![0u8; cross * rows * 4];
    for row in 0..rows {
        let inked_line = row % 37 < 25;
        for column in 0..cross {
            let offset = (row * cross + column) * 4;
            let value = if inked_line && column % 11 < 7 {
                0x20u8
            } else {
                0xE8u8
            };
            pixels[offset] = value;
            pixels[offset + 1] = value;
            pixels[offset + 2] = value;
            pixels[offset + 3] = 0xFF;
        }
    }
    Observation::new(
        pixels,
        Rect::new(0, 0, CROSS_LEN as i32, EXTENT as i32),
        0,
        (CROSS_LEN as u32, EXTENT as u32),
        Axis::Vertical,
    )
    .expect("the viewport is strictly packed and its region agrees with its size")
}

/// Runs one length's workload and reports what it cost.
///
/// The workload is the canvas and nothing else: `start` for the first viewport, then one
/// `append_confirmed` of [`STEP_ROWS`] rows followed by the production eviction rule (`relieve`,
/// §17.5 ③) until the canvas is at least `length` rows long.
///
/// The frame is built **before** the measurement opens, and that placement is the one judgement
/// call here: a frame is a step-lifetime buffer owned by the frame source (§11.3, `P2.03`), so it
/// is not part of the canvas's resident set. Putting it inside would add 6.5 MB to all three
/// lengths — the same 6.5 MB, so the band would still hold — while making the number less
/// attributable.
fn measure_one_length(length: u64) -> LengthReport {
    let frame = viewport_frame();

    let before = alloc_probe::snapshot();
    alloc_probe::reset_peak();
    alloc_probe::reset_traffic();

    let budget = MemoryBudget::for_viewport(CROSS_LEN, EXTENT);
    let mut canvas = RecoveredImage::new(Axis::Vertical, CROSS_LEN, budget);
    canvas.start(&frame);
    let mut tally = StepTally {
        step: 1,
        committed: 1,
        discarded: 0,
    };

    let mut steps = 0u64;
    while canvas.primary_len() < length {
        canvas.append_confirmed(&frame, STEP_ROWS);
        // The viewport the next step will read, in content rows: the canvas ends at the bottom of
        // the viewport, so its top edge is `primary_len - extent` (§17.5 ③ protects exactly this).
        let position = (canvas.primary_len() as i64 - EXTENT as i64).max(0);
        canvas
            .relieve(Some((position, EXTENT)))
            .expect("one viewport must fit the budget §22.3 sized for eight of them");
        steps += 1;
        tally.step += 1;
        tally.committed += 1;
        canvas.assert_invariants(CROSS_LEN, tally);
    }

    let after = alloc_probe::snapshot();
    let rows = canvas.primary_len();
    LengthReport {
        pid: std::process::id(),
        length,
        rows,
        cross_len: CROSS_LEN,
        extent: EXTENT,
        budget: budget.total(),
        steps,
        single_buffer: CROSS_LEN * rows * 4,
        live_before: before.live as u64,
        live: after.live as u64,
        peak: after.peak as u64,
        allocated: after.allocated as u64,
        allocations: after.allocations as u64,
        resident: canvas.bands().resident_bytes(),
        spill_bytes: canvas.bands().spill_file_bytes(),
    }
}

/// The three lengths, each in a process of its own.
///
/// `peak` is process-global and can only be lowered by ending the process, so a loop here would
/// report the longest length's peak three times and the ≤10% band would hold because the three
/// numbers are the same number. Each child therefore runs one length and appends one JSON line to
/// `SNAPCLIP_MEM1_OUT`; the pid in that line is what ties it to the child this function started.
///
/// The result travels through a file rather than a pipe: the children's stdout is inherited (so a
/// failing child's output is still visible in the gate), and a captured pipe would be the one
/// thing a Windows sandbox refuses.
fn run_three_lengths() -> Vec<(u32, LengthReport)> {
    let exe = std::env::current_exe().expect("the test binary must be locatable");
    let out = std::env::temp_dir().join(format!("snapclip-mem1-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&out);

    let mut children = Vec::new();
    for length in LENGTHS {
        let child = std::process::Command::new(&exe)
            .args([
                "--exact",
                "scroll::mem_probe::tests::mem1_measures_one_length",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(LENGTH_ENV, length.to_string())
            .env(OUT_ENV, &out)
            .spawn()
            .expect("the test binary must be runnable");
        children.push((length, child));
    }

    let mut started = Vec::new();
    for (length, mut child) in children {
        let pid = child.id();
        let status = child.wait().expect("a started child must be waitable");
        assert!(
            status.success(),
            "the {length}-row measurement did not finish: {status}"
        );
        started.push((pid, length));
    }

    let text = std::fs::read_to_string(&out).unwrap_or_else(|error| {
        panic!(
            "no child wrote a line to {}: {error}",
            out.display()
        )
    });
    let _ = std::fs::remove_file(&out);
    let reports: Vec<LengthReport> = text.lines().filter_map(parse_report).collect();

    started
        .into_iter()
        .map(|(pid, length)| {
            let report = *reports
                .iter()
                .find(|report| report.pid == pid)
                .unwrap_or_else(|| {
                    panic!(
                        "the {length}-row child (pid {pid}) wrote no report; {} line(s) arrived from \
                         {:?}",
                        reports.len(),
                        reports.iter().map(|report| report.pid).collect::<Vec<_>>()
                    )
                });
            (pid, report)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The device self-check: three numbers that must mean three different things.
    ///
    /// The failure this guards against is not "the probe is slow" but "the probe is decorative" —
    /// a `live` that is really the peak, or an `allocated` that was never wired up and is quietly
    /// zero. Each assertion below is one of those confusions:
    ///
    /// * `live > 0` — the canvas is still holding something when the measurement ends.
    /// * `peak >= live` — a high-water mark cannot be below the level it is measured against.
    /// * `allocated > peak` — a streaming design's traffic exceeds its space. If this ever fails,
    ///   the counter is not counting, because the workload writes a band per step and the space it
    ///   needs is a small fraction of that.
    /// * `spill_bytes > 0` — the run actually crossed the budget. A probe whose canvas fit in the
    ///   budget would be measuring an allocation pattern, not the eviction rule that makes the
    ///   claim true.
    #[test]
    fn the_probe_reports_live_peak_and_allocated_separately() {
        let report = measure_one_length(SELF_CHECK_LENGTH);

        assert!(
            report.live > 0,
            "the canvas holds nothing at the end of the run: live = {}",
            report.live
        );
        assert!(
            report.peak >= report.live,
            "the high-water mark ({}) is below the level it is compared against ({})",
            report.peak,
            report.live
        );
        assert!(
            report.allocated > report.peak,
            "cumulative traffic ({}) did not exceed the high-water mark ({}): the workload writes \
             a {STEP_ROWS}-row band per step, so traffic and space cannot be the same number",
            report.allocated,
            report.peak
        );
        assert!(
            report.allocations > 0,
            "no allocation was recorded at all, so the allocator is not installed"
        );
        assert!(
            report.spill_bytes > 0,
            "the run never spilled ({} rows against a budget of {} B), so it did not exercise the \
             eviction rule this claim rests on",
            SELF_CHECK_LENGTH,
            report.budget
        );
    }

    /// The structural half, and the claim itself: three lengths, three processes, ≤ 10%.
    ///
    /// The process check is not ceremony. `peak` is process-global, so the cheapest way to make
    /// the 10% band hold is to run all three lengths in one process and let the longest one set
    /// the mark — the numbers would then agree because they are the same number. This test refuses
    /// that: every report carries the id of the process that produced it, and each id must match
    /// the child this test actually started.
    #[test]
    fn the_three_lengths_run_in_separate_processes() {
        let runs = run_three_lengths();
        assert_eq!(
            runs.len(),
            LENGTHS.len(),
            "expected one report per length, got {}",
            runs.len()
        );

        let mut seen_pids = std::collections::BTreeSet::new();
        let mut seen_lengths = std::collections::BTreeSet::new();
        for (child_pid, report) in &runs {
            assert_eq!(
                *child_pid, report.pid,
                "the report for {} rows was produced by process {} but was collected from {}",
                report.length, report.pid, child_pid
            );
            assert!(
                seen_pids.insert(report.pid),
                "process {} reported twice: the lengths did not run in separate processes",
                report.pid
            );
            assert!(
                seen_lengths.insert(report.length),
                "length {} was measured twice",
                report.length
            );
        }
        assert_eq!(
            seen_lengths,
            LENGTHS.iter().copied().collect(),
            "the three lengths measured are not the three the task fixes"
        );

        let peaks: Vec<u64> = runs
            .iter()
            .map(|(_, report)| report.peak_over_baseline())
            .collect();
        let low = *peaks.iter().min().expect("three peaks");
        let high = *peaks.iter().max().expect("three peaks");
        let spread = (high - low) as f64 / low as f64;
        assert!(
            spread <= PEAK_BAND,
            "the ceiling grows with the image: peaks {:?} B over baseline differ by {:.1}% \
             (allowed {:.0}%)",
            peaks,
            spread * 100.0,
            PEAK_BAND * 100.0
        );
    }

    /// One length, one process: the child mode [`run_three_lengths`] starts, and the mode the
    /// release driver `tools/p4-07-memory.ps1` runs.
    ///
    /// Ignored because it is the measurement itself, not a check of one: the gate runs the two
    /// tests above (the device self-check and the three-process structural check, which starts this
    /// one three times), and this entry point exists so that a driver can ask for a single length
    /// without the band assertion. Run it directly with
    /// `SNAPCLIP_MEM1_LENGTH=<rows> cargo test --release -p snapclip-capture --lib \
    /// mem1_measures_one_length -- --ignored --nocapture`.
    #[test]
    #[ignore = "P4.07 measures one length per process; drive it with tools/p4-07-memory.ps1"]
    fn mem1_measures_one_length() {
        let length = std::env::var(LENGTH_ENV)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(SELF_CHECK_LENGTH);
        let report = measure_one_length(length);
        let line = report.json_line();
        println!("[P4.07] {line}");
        if let Some(path) = std::env::var_os(OUT_ENV) {
            append_json_line(Path::new(&path), &line);
        }
        assert!(
            report.rows >= length && report.rows < length + STEP_ROWS,
            "the workload stopped at {} rows, which is not within one {STEP_ROWS}-row step of the \
             {length} it was asked for",
            report.rows
        );
    }
}
