//! Phase 0 of `docs/35`: which of the three "moves a lot of bytes" sites owns the per-frame
//! spike, and what the baseline actually is.
//!
//! The plan's §2.2 lists three candidates — `grow()`'s capacity doubling, the whole-canvas shift a
//! `Prepended` frame performs, and the `band` overwrite every placed frame does — and says plainly
//! that nothing measured distinguishes them yet. So this run does not change any behaviour: it
//! drives the real `Tracker` over the same synthetic page in four configurations and reports, per
//! frame, how much each site moved, next to the wall clock, the counting allocator's peak and the
//! process working set.
//!
//! The four configurations form a 2x2: {down only, down with upward rounds} x {default canvas,
//! canvas preallocated to the row cap}. The first axis isolates the prepend shift (a down-only run
//! never prepends), the second isolates `grow` (a preallocated canvas never reallocates). If one
//! axis removes the spike and the other does not, the attribution is decided.

use crate::alloc_probe;
use crate::rss;
use crate::scroll::{frame, page, ROWS, STEP, VIEW, WIDTH};
use snapclip_capture::scroll::probe::{Frame, ScrollProbe};

/// How many frames one run pushes. `docs/35`'s baseline run is 48 pushes after the first frame
/// (49 frames, 29 760 rows), so a variant with upward rounds still stops near the same height.
const PUSHES: usize = 48;

/// Per-frame readings for one configuration.
struct Run {
    name: &'static str,
    stitch_us: Vec<u64>,
    /// The frame that took longest, and what it was doing.
    slowest: Option<Frame>,
    frames: usize,
    rows: u32,
    prepended: usize,
    grow_moves: u32,
    grow_bytes: u64,
    shift_bytes: u64,
    band_bytes: u64,
    peak_extra: usize,
    rss_now: usize,
    rss_peak: usize,
    checksum: u64,
    correct: bool,
}

/// The offsets the viewport visits.
///
/// A downward-only run never prepends, and scrolling back up *inside* the captured image is
/// `Contained` rather than `Prepended` — the canvas is taller than the viewport by then. The
/// prepend path is only reached by scrolling above the frame the session started from, so the
/// mixed preset scrolls down to grow the canvas, then upward past its own start.
enum Preset {
    Down,
    /// Start mid-page so there is page above the first frame, scroll down `down` steps then up
    /// `up` steps. Every upward step past the starting offset prepends: the viewport is showing
    /// rows the canvas has never held, so the canvas shifts down to make room at the top.
    DownThenUpPastStart {
        start: usize,
        down: usize,
        up: usize,
    },
}

fn tops(preset: Preset) -> (usize, Vec<usize>) {
    let mut tops = Vec::new();
    let down = |tops: &mut Vec<usize>, top: &mut usize, steps: usize| {
        for _ in 0..steps {
            if *top + STEP <= ROWS.saturating_sub(VIEW) {
                *top += STEP;
                tops.push(*top);
            }
        }
    };
    match preset {
        Preset::Down => {
            let mut top = 0usize;
            down(&mut tops, &mut top, PUSHES);
            (0, tops)
        }
        Preset::DownThenUpPastStart {
            start,
            down: steps_down,
            up,
        } => {
            let mut top = start;
            down(&mut tops, &mut top, steps_down.min(PUSHES));
            for _ in 0..up {
                // Below `STEP`, stop: the page cannot show a negative offset, and a clamp here
                // would silently turn the remaining upward steps into no-ops.
                if top < STEP {
                    break;
                }
                top -= STEP;
                tops.push(top);
            }
            let remaining = PUSHES.saturating_sub(tops.len());
            down(&mut tops, &mut top, remaining);
            (start, tops)
        }
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn stats(values: &[u64]) -> (u64, u64, u64, u64) {
    if values.is_empty() {
        return (0, 0, 0, 0);
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    (at(0.50), at(0.95), at(0.99), *sorted.last().unwrap_or(&0))
}

fn run(name: &'static str, page: &[u8], start: usize, tops: &[usize], preallocate: bool) -> Run {
    let first = frame(page, WIDTH, start, VIEW);
    let mut probe = ScrollProbe::new(&first, WIDTH as u32, VIEW as u32, WIDTH as u32 / 8);
    if preallocate {
        probe.preallocate(ScrollProbe::row_cap(WIDTH as u32, VIEW as u32));
    }
    let (mut stitch_us, mut slowest, mut prepended) = (Vec::new(), None, 0);
    for &top in tops {
        let next = frame(page, WIDTH, top, VIEW);
        let report = probe.push(&next);
        stitch_us.push(report.stitch_us);
        if report.prepended {
            prepended += 1;
        }
        if slowest.is_none_or(|worst: Frame| report.stitch_us > worst.stitch_us) {
            slowest = Some(report);
        }
    }
    let counters = probe.counters();
    let top = probe.top() + start as i64;
    let (_, height, canvas) = probe.into_image();
    let row = WIDTH * 4;
    // Canvas row `skip` is page row `base`; rows before the page (negative content coordinates)
    // only exist when the session scrolled above its first frame.
    let skip = (-top).max(0) as usize;
    let base = top.max(0) as usize;
    let rows = height as usize;
    let page_end = (base + rows.saturating_sub(skip)).min(ROWS);
    let expected = &page[base * row..page_end * row];
    let got = &canvas[(skip * row).min(canvas.len())..];
    let correct = got.len() == expected.len() && got == expected;
    let working = rss::working_set().unwrap_or((0, 0));
    Run {
        name,
        frames: stitch_us.len() + 1,
        stitch_us,
        slowest,
        rows: height,
        prepended,
        grow_moves: counters.grow_moves,
        grow_bytes: counters.grow_bytes,
        shift_bytes: counters.shift_bytes,
        band_bytes: counters.band_bytes,
        peak_extra: 0,
        rss_now: working.0,
        rss_peak: working.1,
        checksum: fnv1a(&canvas),
        correct,
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The whole phase-0 report.
pub fn report() -> String {
    let mut out = String::new();
    out.push_str("scroll stitching attribution and baseline (docs/35 phase 0)\n");
    out.push_str(&format!(
        "page {WIDTH}x{ROWS} · viewport {WIDTH}x{VIEW} · {STEP} px per push · {PUSHES} pushes\n\n"
    ));

    // One page for all four runs: building it per run would put another copy of it in the working
    // set and make the RSS column contradict itself.
    let page = page(WIDTH, ROWS);
    let (down_start, down) = tops(Preset::Down);
    // Start eight steps into the page so there is page above the first frame: 20 down steps grow
    // the canvas, then 22 up steps cross the starting offset and prepend twice.
    let (mixed_start, mixed) = tops(Preset::DownThenUpPastStart {
        start: 8 * STEP,
        down: 20,
        up: 22,
    });

    let mut runs = Vec::new();
    for (label, start, offsets, preallocated) in [
        ("down, default canvas", down_start, &down, false),
        ("down, preallocated", down_start, &down, true),
        ("down then up past the start, default canvas", mixed_start, &mixed, false),
        ("down then up past the start, preallocated", mixed_start, &mixed, true),
    ] {
        let (mut result, peak) =
            alloc_probe::peak_extra(|| run(label, &page, start, offsets, preallocated));
        result.peak_extra = peak;
        runs.push(result);
    }

    out.push_str("run | frames | rows | p50 | p95 | p99 | max | prepends | grow moves | grow MB | shift MB | band MB | peak alloc MB | RSS now MB | RSS peak MB | checksum | exact\n");
    out.push_str("--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---\n");
    for run in &runs {
        let (p50, p95, p99, max) = stats(&run.stitch_us);
        out.push_str(&format!(
            "{} | {} | {} | {:.2} | {:.2} | {:.2} | **{:.2}** | {} | {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:016x} | {}\n",
            run.name,
            run.frames,
            run.rows,
            p50 as f64 / 1000.0,
            p95 as f64 / 1000.0,
            p99 as f64 / 1000.0,
            max as f64 / 1000.0,
            run.prepended,
            run.grow_moves,
            mb(run.grow_bytes),
            mb(run.shift_bytes),
            mb(run.band_bytes),
            mb(run.peak_extra as u64),
            mb(run.rss_now as u64),
            mb(run.rss_peak as u64),
            run.checksum,
            run.correct,
        ));
    }

    out.push_str("\nwhat the slowest frame in each run was doing:\n");
    for run in &runs {
        if let Some(frame) = run.slowest {
            out.push_str(&format!(
                "  {:<24} {:>7.2} ms · {:<8} · grow {} move(s)/{:.1} MB · shift {:.1} MB · band {:.1} MB · rows {}\n",
                run.name,
                frame.stitch_us as f64 / 1000.0,
                if frame.prepended {
                    "prepend"
                } else if frame.appended {
                    "append"
                } else {
                    "contained"
                },
                frame.grow_moves,
                mb(frame.grow_bytes),
                mb(frame.shift_bytes),
                mb(frame.band_bytes),
                frame.image_rows,
            ));
        }
    }

    out.push_str("\nreading: the preallocated runs have no grow moves by construction, so if their max is\n");
    out.push_str("as high as the default one's the spike is not reallocation. A down-only run never\n");
    out.push_str("prepends, so if it has no spike the shift cannot be the cause either.\n\n");

    // The verdict is derived from the readings, not asserted in advance.
    let default_down = &runs[0];
    let prealloc_down = &runs[1];
    let default_mixed = &runs[2];
    let prealloc_mixed = &runs[3];
    let max_of = |run: &Run| stats(&run.stitch_us).3;
    let verdict = if max_of(default_down) < 4_000 && max_of(default_mixed) < 4_000 {
        "no spike above 4 ms in any run: nothing to attribute this time".to_string()
    } else if max_of(prealloc_down) < 4_000 && max_of(default_down) < 4_000 {
        // Down-only is quiet either way; the spike must be the mixed run's.
        if max_of(prealloc_mixed) + 2_000 < max_of(default_mixed) {
            "shifting is the spike, and reallocation makes it worse on top".to_string()
        } else {
            "the spike belongs to the prepended shift".to_string()
        }
    } else if max_of(prealloc_down) + 2_000 < max_of(default_down) {
        "reallocation is the spike: preallocating removes it".to_string()
    } else {
        "neither axis removes the spike: attribute it to the band copy or something outside this table"
            .to_string()
    };
    out.push_str(&format!("verdict: {verdict}\n"));
    out
}
