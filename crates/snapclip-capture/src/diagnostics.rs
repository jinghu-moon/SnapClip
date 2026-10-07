//! Window-detection performance counters (docs/14 §10.2).
//!
//! The counters exist to answer one question with data instead of intuition: *is the
//! window-detection path cheap enough to run inside the overlay message loop?* The
//! design forbids `EnumWindows`/DWM work on the mouse path, so the numbers that matter
//! are the per-operation latencies (`hit_test`, `nearest_target`), the snapshot refresh
//! cost that must stay on the worker, and the queue/stale counters that prove the
//! mailbox is bounded and single-flight.
//!
//! The handle is a cheap `Clone` over shared atomics: the overlay thread and the
//! detection worker hold the same instance, so a refresh timed on the worker and a hit
//! test timed on the overlay land in one report.
//!
//! Logging is deliberately gated: [`WindowDetectionMetrics::is_verbose`] is `false`
//! unless a diagnostics build asks for it, so the counters never flood stderr during
//! normal use, and [`WindowDetectionMetrics::summary_line`] gives one line to print
//! when a session ends.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// Environment variable that turns the per-operation diagnostics lines on.
///
/// Only the *verbosity* is switchable; the counters are always recorded, so a session
/// that never sets this still produces a complete one-line summary at teardown.
pub const VERBOSE_ENV: &str = "SNAPCLIP_WIN_DETECT_VERBOSE";

/// `Duration` as whole microseconds, saturating instead of wrapping.
fn micros(elapsed: Duration) -> u64 {
    elapsed.as_micros().min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Default)]
struct Counters {
    /// Overlay presents (full-surface repaints) and how long each took.
    ///
    /// The overlay's cost model is *frames*, not rectangles: drawing four more rings is free, and
    /// scheduling four more full-screen repaints is not (docs/21 §5.22). These counters are what
    /// turns that claim into a number on a real machine.
    present_count: AtomicU64,
    present_last_us: AtomicU64,
    present_max_us: AtomicU64,
    /// The **first** present of a session, timed on its own.
    ///
    /// Real sessions reported max present times of 29 ms, 74 ms and 93 ms on a 4K surface while
    /// `last` stayed under 1.4 ms. Most of that is the first frame — effect creation, font upload,
    /// swap-chain warm-up — and separating it says whether the tail is one-off or recurring.
    present_first_us: AtomicU64,
    /// Presents slower than one 60 Hz frame (16 ms): the count that says "recurring".
    present_over_16ms: AtomicU64,
    /// Repaints the ③b chain fade produced (docs/21 §5.22). Bounded by design at eight per fade;
    /// this is the number that checks it.
    chain_fade_frames: AtomicU64,
    /// Repaints the capture box's walk colour produced (docs/21 §5.24, A3). Bounded by design at
    /// about six for the rise plus eight for the fall; this is the number that checks it.
    walk_frames: AtomicU64,
    snapshot_refresh_count: AtomicU64,
    snapshot_refresh_last_us: AtomicU64,
    snapshot_refresh_max_us: AtomicU64,
    snapshot_release_count: AtomicU64,
    snapshot_release_last_us: AtomicU64,
    snapshot_release_max_us: AtomicU64,
    hit_test_count: AtomicU64,
    hit_test_last_us: AtomicU64,
    hit_test_max_us: AtomicU64,
    nearest_target_count: AtomicU64,
    nearest_target_last_us: AtomicU64,
    nearest_target_max_us: AtomicU64,
    validate_count: AtomicU64,
    validate_last_us: AtomicU64,
    validate_max_us: AtomicU64,
    /// Gauge: candidate count of the most recent snapshot.
    candidate_count: AtomicU64,
    hover_target_switch_count: AtomicU64,
    stale_target_count: AtomicU64,
    hover_revalidate_stale_dropped_count: AtomicU64,
    worker_enqueued: AtomicU64,
    worker_dequeued: AtomicU64,
    worker_max_queue_depth: AtomicU64,
    worker_stale_result_dropped_count: AtomicU64,
    /// Gauge: mouse moves discarded because a newer position superseded them.
    mouse_move_coalesced_count: AtomicU64,
    // ── v2 refinement (docs/18 §10) ───────────────────────────────────────────
    refinement_submitted: AtomicU64,
    refinement_published: AtomicU64,
    refinement_empty: AtomicU64,
    refinement_last_us: AtomicU64,
    refinement_max_us: AtomicU64,
    refinement_msaa_attempts: AtomicU64,
    refinement_msaa_timeouts: AtomicU64,
    refinement_msaa_busy: AtomicU64,
    refinement_msaa_failures: AtomicU64,
    /// Latency histogram for published refinements: `<16 ms`, `<32 ms`, `<64 ms`, `<256 ms`,
    /// `>=256 ms`. Replaces guessing at a percentile from last/max alone.
    refinement_latency_buckets: [AtomicU64; 5],
    /// Windows sent to quarantine (their provider timed out or is hung).
    refinement_quarantine_added: AtomicU64,
    /// Queries answered from quarantine without touching a provider (the saving).
    refinement_quarantine_hit: AtomicU64,
    /// Queries the overlay abandoned because they outlived their in-flight budget.
    refinement_inflight_timeouts: AtomicU64,
    /// Answers the scheduler rejected because the question had moved on.
    refinement_superseded: AtomicU64,
    /// Answers held back as a downgrade awaiting a second dwell (docs/18 §13.3).
    refinement_downgrades_staged: AtomicU64,
    /// Queries issued for a position whose dwell expired while another query ran.
    refinement_follow_ups: AtomicU64,
    /// The provider's own point hit test replaced the walk's answer because it was finer.
    refinement_precision_adopted: AtomicU64,
    /// The provider answered, but not with a strictly finer box: the walk's answer stands.
    refinement_precision_not_finer: AtomicU64,
    /// The provider's hit test could not be used at all (see the last-precision line).
    refinement_precision_unavailable: AtomicU64,
}

/// A copyable reading of every counter, for assertions and one-line reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowDetectionReading {
    pub present_count: u64,
    pub present_last_us: u64,
    pub present_max_us: u64,
    pub present_first_us: u64,
    pub present_over_16ms: u64,
    pub chain_fade_frames: u64,
    pub walk_frames: u64,
    pub snapshot_refresh_count: u64,
    pub snapshot_refresh_last_us: u64,
    pub snapshot_refresh_max_us: u64,
    pub snapshot_release_count: u64,
    pub snapshot_release_last_us: u64,
    pub snapshot_release_max_us: u64,
    pub hit_test_count: u64,
    pub hit_test_last_us: u64,
    pub hit_test_max_us: u64,
    pub nearest_target_count: u64,
    pub nearest_target_last_us: u64,
    pub nearest_target_max_us: u64,
    pub validate_count: u64,
    pub validate_last_us: u64,
    pub validate_max_us: u64,
    pub candidate_count: u64,
    pub hover_target_switch_count: u64,
    pub stale_target_count: u64,
    pub hover_revalidate_stale_dropped_count: u64,
    pub worker_queue_depth: u64,
    pub worker_max_queue_depth: u64,
    pub worker_stale_result_dropped_count: u64,
    pub mouse_move_coalesced_count: u64,
    pub refinement_submitted: u64,
    pub refinement_published: u64,
    pub refinement_empty: u64,
    pub refinement_last_us: u64,
    pub refinement_max_us: u64,
    pub refinement_msaa_attempts: u64,
    pub refinement_msaa_timeouts: u64,
    pub refinement_msaa_busy: u64,
    pub refinement_msaa_failures: u64,
    pub refinement_latency_under_16ms: u64,
    pub refinement_latency_under_32ms: u64,
    pub refinement_latency_under_64ms: u64,
    pub refinement_latency_under_256ms: u64,
    pub refinement_latency_over_256ms: u64,
    pub refinement_quarantine_added: u64,
    pub refinement_quarantine_hit: u64,
    pub refinement_inflight_timeouts: u64,
    pub refinement_superseded: u64,
    pub refinement_downgrades_staged: u64,
    pub refinement_follow_ups: u64,
    pub refinement_precision_adopted: u64,
    pub refinement_precision_not_finer: u64,
    pub refinement_precision_unavailable: u64,
}

/// Shared diagnostics handle for the window-detection pipeline.
#[derive(Debug, Clone, Default)]
pub struct WindowDetectionMetrics {
    counters: Arc<Counters>,
    verbose: Arc<AtomicBool>,
    /// The most recent precision decision in words, so the one forced line per session
    /// (docs/21 §5.7) can say *why* the provider's finer box was or was not taken. Silent
    /// skips cost a diagnosis round: the state that matters is "the top-up ran and did
    /// nothing", and with verbose off there was nothing in the log to say so.
    last_precision: Arc<std::sync::Mutex<Option<String>>>,
    /// The same decision as a value, so the paint layer does not have to parse the note above
    /// (`0` = no query yet; otherwise the [`PrecisionOutcome`] discriminant).
    last_precision_outcome: Arc<AtomicU64>,
}

/// What the precision top-up did with the provider's own point hit test (docs/21 §5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecisionOutcome {
    /// The walk stopped above the innermost capturable box and the hit test's box became
    /// the answer.
    Adopted,
    /// The provider answered, but not with a strictly finer box: the walk's answer stands.
    NotFiner,
    /// The provider's hit test could not be used for this window (reason in the note).
    Unavailable,
}

impl WindowDetectionMetrics {
    pub fn new() -> Self {
        let metrics = Self::default();
        // Diagnostics switch (docs/14 §10.2): per-operation lines stay off by default so
        // the counters never flood stderr, and an acceptance probe can turn them on
        // without a rebuild.
        if std::env::var_os(VERBOSE_ENV).is_some() {
            metrics.set_verbose(true);
        }
        metrics
    }

    /// Whether per-operation diagnostics lines should be emitted.
    ///
    /// Default `false`; a profiling run or an acceptance probe turns it on. The
    /// counters are recorded either way, so enabling the flag at the end of a session
    /// still produces a complete reading.
    pub fn is_verbose(&self) -> bool {
        self.verbose.load(Ordering::Relaxed)
    }

    pub fn set_verbose(&self, verbose: bool) {
        self.verbose.store(verbose, Ordering::Relaxed);
    }

    /// Emit a diagnostics line unless it would flood the log.
    ///
    /// `force` is for once-per-session lines (session teardown); everything on the
    /// mouse or worker path passes `false`.
    pub fn log_line(&self, message: &str, force: bool) {
        if force || self.is_verbose() {
            eprintln!("[snapclip][win-detect] {message}");
        }
    }

    /// A worker completed a snapshot refresh and produced `candidates` windows.
    /// Record one overlay present (a full-surface repaint) and how long it took.
    ///
    /// Called by the overlay around `renderer.render(...)`, so the number covers the paint *and* the
    /// present. The first one of the session is recorded separately, and slow ones are counted: a
    /// 4K surface reported 29/74/93 ms tails while every other present stayed under 1.4 ms, and the
    /// two numbers together say whether that is warm-up or something that happens during use.
    pub fn record_present(&self, elapsed: Duration) {
        let micros = elapsed.as_micros() as u64;
        if self.counters.present_count.load(Ordering::Relaxed) == 0 {
            self.counters
                .present_first_us
                .store(micros, Ordering::Relaxed);
        }
        if micros > 16_000 {
            self.counters
                .present_over_16ms
                .fetch_add(1, Ordering::Relaxed);
        }
        record_timing(
            &self.counters.present_count,
            &self.counters.present_last_us,
            &self.counters.present_max_us,
            elapsed,
        );
    }

    /// One repaint produced by the ③b chain fade (docs/21 §5.22).
    pub fn record_chain_fade_frame(&self) {
        self.counters
            .chain_fade_frames
            .fetch_add(1, Ordering::Relaxed);
    }

    /// One repaint produced by the capture box's walk colour (docs/21 §5.24, A3).
    pub fn record_walk_frame(&self) {
        self.counters.walk_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_snapshot_refresh(&self, elapsed: Duration, candidates: usize) {
        record_timing(
            &self.counters.snapshot_refresh_count,
            &self.counters.snapshot_refresh_last_us,
            &self.counters.snapshot_refresh_max_us,
            elapsed,
        );
        self.counters
            .candidate_count
            .store(candidates as u64, Ordering::Relaxed);
    }

    /// A snapshot (and its indexes) was dropped at session end or invalidation.
    pub fn record_snapshot_release(&self, elapsed: Duration) {
        record_timing(
            &self.counters.snapshot_release_count,
            &self.counters.snapshot_release_last_us,
            &self.counters.snapshot_release_max_us,
            elapsed,
        );
    }

    /// One `WindowSnapshot::hit_test` call on the overlay thread.
    pub fn record_hit_test(&self, elapsed: Duration) {
        record_timing(
            &self.counters.hit_test_count,
            &self.counters.hit_test_last_us,
            &self.counters.hit_test_max_us,
            elapsed,
        );
    }

    /// One `WindowSnapshot::nearest_target` call (dwell timer expiry).
    pub fn record_nearest_target(&self, elapsed: Duration) {
        record_timing(
            &self.counters.nearest_target_count,
            &self.counters.nearest_target_last_us,
            &self.counters.nearest_target_max_us,
            elapsed,
        );
    }

    /// One worker-side `validate`/`revalidate_hover` of a single window.
    pub fn record_validate(&self, elapsed: Duration) {
        record_timing(
            &self.counters.validate_count,
            &self.counters.validate_last_us,
            &self.counters.validate_max_us,
            elapsed,
        );
    }

    /// The hovered window changed from one identity to a different one.
    pub fn record_hover_target_switch(&self) {
        self.counters
            .hover_target_switch_count
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A hover/confirm target was found to be stale (closed, cloaked, moved away).
    pub fn record_stale_target(&self) {
        self.counters
            .stale_target_count
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A worker re-validation result was discarded because epoch/HWND moved on.
    pub fn record_hover_revalidate_stale_dropped(&self) {
        self.counters
            .hover_revalidate_stale_dropped_count
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A request entered the detection mailbox (the mailbox keeps only the newest).
    pub fn record_worker_enqueued(&self) {
        let depth = self.counters.worker_enqueued.fetch_add(1, Ordering::Relaxed) + 1
            - self.counters.worker_dequeued.load(Ordering::Relaxed);
        self.counters
            .worker_max_queue_depth
            .fetch_max(depth, Ordering::Relaxed);
    }

    /// A request left the detection mailbox.
    pub fn record_worker_dequeued(&self) {
        // Saturate rather than wrap: a dequeue without a matching enqueue (a
        // cancelled session's shutdown path) must not report a huge queue depth.
        let _ = self
            .counters
            .worker_dequeued
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |dequeued| {
                let enqueued = self.counters.worker_enqueued.load(Ordering::Relaxed);
                (dequeued < enqueued).then_some(dequeued + 1)
            });
    }

    /// A worker result was dropped because a newer request had replaced it.
    pub fn record_worker_stale_result_dropped(&self) {
        self.counters
            .worker_stale_result_dropped_count
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A queued mouse position was superseded by a newer one before it was handled.
    pub fn record_mouse_move_coalesced(&self, dropped: u64) {
        self.counters
            .mouse_move_coalesced_count
            .fetch_add(dropped, Ordering::Relaxed);
    }

    /// A v2 refinement query was handed to the refinement worker.
    pub fn record_refinement_submitted(&self) {
        self.counters
            .refinement_submitted
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A refinement result was accepted and published (deep selection is live).
    pub fn record_refinement_published(&self, elapsed: Duration) {
        self.counters
            .refinement_published
            .fetch_add(1, Ordering::Relaxed);
        let micros = micros(elapsed);
        self.counters
            .refinement_last_us
            .store(micros, Ordering::Relaxed);
        self.counters
            .refinement_max_us
            .fetch_max(micros, Ordering::Relaxed);
        self.record_refinement_latency(elapsed);
    }

    /// Bucket one refinement latency.
    ///
    /// The buckets are the ones that matter for this feature: anything under 32 ms is
    /// imperceptible next to the 80 ms dwell, 32-64 ms starts to be felt when the cursor
    /// crosses several controls, and the top bucket is where quarantine earns its keep.
    pub fn record_refinement_latency(&self, elapsed: Duration) {
        let milliseconds = elapsed.as_millis();
        let index = match milliseconds {
            0..=15 => 0,
            16..=31 => 1,
            32..=63 => 2,
            64..=255 => 3,
            _ => 4,
        };
        self.counters.refinement_latency_buckets[index].fetch_add(1, Ordering::Relaxed);
    }

    /// A window was quarantined because its provider timed out or is hung.
    pub fn record_refinement_quarantine_added(&self) {
        self.counters
            .refinement_quarantine_added
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A query was answered straight from quarantine, without touching any provider.
    pub fn record_refinement_quarantine_hit(&self) {
        self.counters
            .refinement_quarantine_hit
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A refinement query came back with nothing usable (unsupported, cancelled, failed).
    ///
    /// This counter is what makes "deep selection never triggers" diagnosable without
    /// turning on the verbose log: a session with `refinement_submitted > 0` and
    /// `refinement_published == 0` means the provider answered but produced no path.
    pub fn record_refinement_empty(&self) {
        self.counters
            .refinement_empty
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A refinement query outlived its budget and was abandoned (docs/18 §3).
    ///
    /// Non-zero here means a provider did not return in time — the slot is released, deep
    /// selection degrades to the v1 frame for this position, and the wedged call is left behind
    /// rather than waited on.
    pub fn record_refinement_inflight_timeout(&self) {
        self.counters
            .refinement_inflight_timeouts
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A refinement answer arrived for a question the scheduler no longer asks.
    pub fn record_refinement_superseded(&self) {
        self.counters
            .refinement_superseded
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A deferred position was queried once the single-flight slot freed.
    pub fn record_refinement_follow_up(&self) {
        self.counters
            .refinement_follow_ups
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A shallower answer was staged instead of displayed, pending a second dwell.
    ///
    /// Together with `refinement_published` this tells "the provider never got deep" apart
    /// from "the provider got deep but the overlay did not adopt it".
    pub fn record_refinement_downgrade_staged(&self) {
        self.counters
            .refinement_downgrades_staged
            .fetch_add(1, Ordering::Relaxed);
    }

    /// An MSAA fallback query was actually handed to the provider.
    pub fn record_refinement_msaa_attempt(&self) {
        self.counters
            .refinement_msaa_attempts
            .fetch_add(1, Ordering::Relaxed);
    }

    /// An MSAA query exceeded its deadline (the window is quarantined until the next snapshot).
    pub fn record_refinement_msaa_timeout(&self) {
        self.counters
            .refinement_msaa_timeouts
            .fetch_add(1, Ordering::Relaxed);
    }

    /// An MSAA query was refused because the runner was still holding an abandoned call.
    ///
    /// Deliberately separate from timeouts: "busy" means retry, "timeout" means the provider is
    /// misbehaving, and conflating them would quarantine healthy windows.
    pub fn record_refinement_msaa_busy(&self) {
        self.counters
            .refinement_msaa_busy
            .fetch_add(1, Ordering::Relaxed);
    }

    /// The MSAA provider returned an error (no quarantine).
    pub fn record_refinement_msaa_failure(&self) {
        self.counters
            .refinement_msaa_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Read every counter.
    pub fn reading(&self) -> WindowDetectionReading {
        let counters = &self.counters;
        let load = |value: &AtomicU64| value.load(Ordering::Relaxed);
        WindowDetectionReading {
            present_count: load(&counters.present_count),
            present_last_us: load(&counters.present_last_us),
            present_max_us: load(&counters.present_max_us),
            present_first_us: load(&counters.present_first_us),
            present_over_16ms: load(&counters.present_over_16ms),
            chain_fade_frames: load(&counters.chain_fade_frames),
            walk_frames: load(&counters.walk_frames),
            snapshot_refresh_count: load(&counters.snapshot_refresh_count),
            snapshot_refresh_last_us: load(&counters.snapshot_refresh_last_us),
            snapshot_refresh_max_us: load(&counters.snapshot_refresh_max_us),
            snapshot_release_count: load(&counters.snapshot_release_count),
            snapshot_release_last_us: load(&counters.snapshot_release_last_us),
            snapshot_release_max_us: load(&counters.snapshot_release_max_us),
            hit_test_count: load(&counters.hit_test_count),
            hit_test_last_us: load(&counters.hit_test_last_us),
            hit_test_max_us: load(&counters.hit_test_max_us),
            nearest_target_count: load(&counters.nearest_target_count),
            nearest_target_last_us: load(&counters.nearest_target_last_us),
            nearest_target_max_us: load(&counters.nearest_target_max_us),
            validate_count: load(&counters.validate_count),
            validate_last_us: load(&counters.validate_last_us),
            validate_max_us: load(&counters.validate_max_us),
            candidate_count: load(&counters.candidate_count),
            hover_target_switch_count: load(&counters.hover_target_switch_count),
            stale_target_count: load(&counters.stale_target_count),
            hover_revalidate_stale_dropped_count: load(
                &counters.hover_revalidate_stale_dropped_count,
            ),
            worker_queue_depth: load(&counters.worker_enqueued)
                .saturating_sub(load(&counters.worker_dequeued)),
            worker_max_queue_depth: load(&counters.worker_max_queue_depth),
            worker_stale_result_dropped_count: load(&counters.worker_stale_result_dropped_count),
            mouse_move_coalesced_count: load(&counters.mouse_move_coalesced_count),
            refinement_submitted: load(&counters.refinement_submitted),
            refinement_published: load(&counters.refinement_published),
            refinement_empty: load(&counters.refinement_empty),
            refinement_last_us: load(&counters.refinement_last_us),
            refinement_max_us: load(&counters.refinement_max_us),
            refinement_msaa_attempts: load(&counters.refinement_msaa_attempts),
            refinement_msaa_timeouts: load(&counters.refinement_msaa_timeouts),
            refinement_msaa_busy: load(&counters.refinement_msaa_busy),
            refinement_msaa_failures: load(&counters.refinement_msaa_failures),
            refinement_latency_under_16ms: load(&counters.refinement_latency_buckets[0]),
            refinement_latency_under_32ms: load(&counters.refinement_latency_buckets[1]),
            refinement_latency_under_64ms: load(&counters.refinement_latency_buckets[2]),
            refinement_latency_under_256ms: load(&counters.refinement_latency_buckets[3]),
            refinement_latency_over_256ms: load(&counters.refinement_latency_buckets[4]),
            refinement_quarantine_added: load(&counters.refinement_quarantine_added),
            refinement_quarantine_hit: load(&counters.refinement_quarantine_hit),
            refinement_inflight_timeouts: load(&counters.refinement_inflight_timeouts),
            refinement_superseded: load(&counters.refinement_superseded),
            refinement_downgrades_staged: load(&counters.refinement_downgrades_staged),
            refinement_follow_ups: load(&counters.refinement_follow_ups),
            refinement_precision_adopted: load(&counters.refinement_precision_adopted),
            refinement_precision_not_finer: load(&counters.refinement_precision_not_finer),
            refinement_precision_unavailable: load(&counters.refinement_precision_unavailable),
        }
    }

    /// One line carrying every §10.2 metric, for session teardown and probes.
    pub fn summary_line(&self) -> String {
        let reading = self.reading();
        format!(
            "present={} present_us last={} max={} first={} over16ms={} chain_fade_frames={} \
             walk_frames={} \
             window_snapshot_refresh_us last={} max={} n={} \
             window_snapshot_release_us last={} max={} n={} \
             window_hit_test_us last={} max={} n={} \
             window_nearest_target_us last={} max={} n={} \
             window_validate_us last={} max={} n={} \
             candidate_count={} hover_target_switch_count={} stale_target_count={} \
             hover_revalidate_stale_dropped_count={} \
             window_worker_queue_depth={} window_worker_max_queue_depth={} \
             window_worker_stale_result_dropped_count={} \
             mouse_move_coalesced_count={} \
             refinement_submitted={} refinement_published={} refinement_empty={} \
             refinement_elapsed_us last={} max={} \
             refinement_msaa_attempts={} refinement_msaa_timeouts={} \
             refinement_msaa_busy={} refinement_msaa_failures={} \
             refinement_latency_buckets <16ms={} <32ms={} <64ms={} <256ms={} >=256ms={} \
             refinement_quarantine_added={} refinement_quarantine_hit={} \
             refinement_inflight_timeouts={} \
             refinement_superseded={} refinement_downgrades_staged={} \
             refinement_follow_ups={} \
             refinement_precision_adopted={} refinement_precision_not_finer={} \
             refinement_precision_unavailable={}",
            reading.present_count,
            reading.present_last_us,
            reading.present_max_us,
            reading.present_first_us,
            reading.present_over_16ms,
            reading.chain_fade_frames,
            reading.walk_frames,
            reading.snapshot_refresh_last_us,
            reading.snapshot_refresh_max_us,
            reading.snapshot_refresh_count,
            reading.snapshot_release_last_us,
            reading.snapshot_release_max_us,
            reading.snapshot_release_count,
            reading.hit_test_last_us,
            reading.hit_test_max_us,
            reading.hit_test_count,
            reading.nearest_target_last_us,
            reading.nearest_target_max_us,
            reading.nearest_target_count,
            reading.validate_last_us,
            reading.validate_max_us,
            reading.validate_count,
            reading.candidate_count,
            reading.hover_target_switch_count,
            reading.stale_target_count,
            reading.hover_revalidate_stale_dropped_count,
            reading.worker_queue_depth,
            reading.worker_max_queue_depth,
            reading.worker_stale_result_dropped_count,
            reading.mouse_move_coalesced_count,
            reading.refinement_submitted,
            reading.refinement_published,
            reading.refinement_empty,
            reading.refinement_last_us,
            reading.refinement_max_us,
            reading.refinement_msaa_attempts,
            reading.refinement_msaa_timeouts,
            reading.refinement_msaa_busy,
            reading.refinement_msaa_failures,
            reading.refinement_latency_under_16ms,
            reading.refinement_latency_under_32ms,
            reading.refinement_latency_under_64ms,
            reading.refinement_latency_under_256ms,
            reading.refinement_latency_over_256ms,
            reading.refinement_quarantine_added,
            reading.refinement_quarantine_hit,
            reading.refinement_inflight_timeouts,
            reading.refinement_superseded,
            reading.refinement_downgrades_staged,
            reading.refinement_follow_ups,
            reading.refinement_precision_adopted,
            reading.refinement_precision_not_finer,
            reading.refinement_precision_unavailable,
        )
    }

    /// Forget every counter. Called when a capture session ends so the next session's
    /// report is not polluted by the previous one.
    /// Record what the precision top-up decided for one query, and between which boxes.
    ///
    /// The note is the whole point: "the top-up ran and did nothing" is the state that is
    /// invisible in a non-verbose log, and it is the state a user reporting "elements inside
    /// this box are not recognized" is looking at.
    pub fn record_precision(&self, outcome: PrecisionOutcome, note: &str) {
        let (counter, verb) = match outcome {
            PrecisionOutcome::Adopted => (&self.counters.refinement_precision_adopted, "adopted"),
            PrecisionOutcome::NotFiner => (&self.counters.refinement_precision_not_finer, "not-finer"),
            PrecisionOutcome::Unavailable => (
                &self.counters.refinement_precision_unavailable,
                "unavailable",
            ),
        };
        counter.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut last) = self.last_precision.lock() {
            *last = Some(format!("{verb} {note}"));
        }
        self.last_precision_outcome
            .store(match outcome {
                PrecisionOutcome::Adopted => 1,
                PrecisionOutcome::NotFiner => 2,
                PrecisionOutcome::Unavailable => 3,
            }, Ordering::Relaxed);
    }

    /// The most recent precision decision, verbatim, for the per-session forensics line.
    pub fn last_precision(&self) -> Option<String> {
        self.last_precision.lock().ok().and_then(|last| last.clone())
    }

    /// The most recent precision decision as a value, for the paint layer (docs/21 §5.21).
    ///
    /// `None` until a query has run; `Unavailable` is what tells the preview that nothing could
    /// answer for this position, so the box it is showing is a fallback.
    pub fn last_precision_outcome(&self) -> Option<PrecisionOutcome> {
        match self.last_precision_outcome.load(Ordering::Relaxed) {
            1 => Some(PrecisionOutcome::Adopted),
            2 => Some(PrecisionOutcome::NotFiner),
            3 => Some(PrecisionOutcome::Unavailable),
            _ => None,
        }
    }

    pub fn reset(&self) {
        let counters = &self.counters;
        if let Ok(mut last) = self.last_precision.lock() {
            *last = None;
        }
        self.last_precision_outcome.store(0, Ordering::Relaxed);
        for counter in [
            &counters.present_count,
            &counters.present_last_us,
            &counters.present_max_us,
            &counters.present_first_us,
            &counters.present_over_16ms,
            &counters.chain_fade_frames,
            &counters.walk_frames,
            &counters.snapshot_refresh_count,
            &counters.snapshot_refresh_last_us,
            &counters.snapshot_refresh_max_us,
            &counters.snapshot_release_count,
            &counters.snapshot_release_last_us,
            &counters.snapshot_release_max_us,
            &counters.hit_test_count,
            &counters.hit_test_last_us,
            &counters.hit_test_max_us,
            &counters.nearest_target_count,
            &counters.nearest_target_last_us,
            &counters.nearest_target_max_us,
            &counters.validate_count,
            &counters.validate_last_us,
            &counters.validate_max_us,
            &counters.candidate_count,
            &counters.hover_target_switch_count,
            &counters.stale_target_count,
            &counters.hover_revalidate_stale_dropped_count,
            &counters.worker_enqueued,
            &counters.worker_dequeued,
            &counters.worker_max_queue_depth,
            &counters.worker_stale_result_dropped_count,
            &counters.mouse_move_coalesced_count,
            &counters.refinement_submitted,
            &counters.refinement_published,
            &counters.refinement_empty,
            &counters.refinement_last_us,
            &counters.refinement_max_us,
            &counters.refinement_msaa_attempts,
            &counters.refinement_msaa_timeouts,
            &counters.refinement_msaa_busy,
            &counters.refinement_msaa_failures,
            &counters.refinement_quarantine_added,
            &counters.refinement_quarantine_hit,
            &counters.refinement_latency_buckets[0],
            &counters.refinement_latency_buckets[1],
            &counters.refinement_latency_buckets[2],
            &counters.refinement_latency_buckets[3],
            &counters.refinement_latency_buckets[4],
            &counters.refinement_inflight_timeouts,
            &counters.refinement_superseded,
            &counters.refinement_downgrades_staged,
            &counters.refinement_follow_ups,
            &counters.refinement_precision_adopted,
            &counters.refinement_precision_not_finer,
            &counters.refinement_precision_unavailable,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
    }
}

fn record_timing(count: &AtomicU64, last: &AtomicU64, max: &AtomicU64, elapsed: Duration) {
    let micros = micros(elapsed);
    count.fetch_add(1, Ordering::Relaxed);
    last.store(micros, Ordering::Relaxed);
    max.fetch_max(micros, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timings_keep_the_latest_and_the_worst_case() {
        let metrics = WindowDetectionMetrics::new();
        metrics.record_hit_test(Duration::from_micros(30));
        metrics.record_hit_test(Duration::from_micros(12));
        metrics.record_hit_test(Duration::from_micros(90));
        let reading = metrics.reading();
        assert_eq!(reading.hit_test_count, 3);
        assert_eq!(reading.hit_test_last_us, 90, "last call wins");
        assert_eq!(reading.hit_test_max_us, 90, "max survives a faster call");

        // Other metrics stay untouched: the pipeline must not cross-report.
        assert_eq!(reading.nearest_target_count, 0);
        assert_eq!(reading.validate_count, 0);
        assert_eq!(reading.snapshot_refresh_count, 0);
    }

    #[test]
    fn a_snapshot_refresh_records_its_cost_and_the_candidate_gauge() {
        let metrics = WindowDetectionMetrics::new();
        metrics.record_snapshot_refresh(Duration::from_millis(3), 57);
        let reading = metrics.reading();
        assert_eq!(reading.snapshot_refresh_last_us, 3_000);
        assert_eq!(reading.candidate_count, 57, "gauge tracks the newest snapshot");

        // The gauge is replaced, not accumulated, when a later snapshot is smaller.
        metrics.record_snapshot_refresh(Duration::from_millis(1), 12);
        assert_eq!(metrics.reading().candidate_count, 12);
        assert_eq!(metrics.reading().snapshot_refresh_count, 2);
    }

    #[test]
    fn queue_depth_is_a_bounded_gauge() {
        let metrics = WindowDetectionMetrics::new();
        for _ in 0..3 {
            metrics.record_worker_enqueued();
        }
        assert_eq!(metrics.reading().worker_queue_depth, 3);
        assert_eq!(metrics.reading().worker_max_queue_depth, 3);

        // The overlay only keeps the newest request, so a burst collapses.
        metrics.record_worker_dequeued();
        metrics.record_worker_dequeued();
        assert_eq!(metrics.reading().worker_queue_depth, 1);
        assert_eq!(
            metrics.reading().worker_max_queue_depth,
            3,
            "the high-water mark is remembered after the queue drains"
        );

        // Draining an empty mailbox (shutdown racing a cancel) must not underflow.
        metrics.record_worker_dequeued();
        metrics.record_worker_dequeued();
        assert_eq!(metrics.reading().worker_queue_depth, 0);
    }

    #[test]
    fn the_summary_line_names_every_documented_metric() {
        let metrics = WindowDetectionMetrics::new();
        // The overlay's present counter: the number that says whether a repaint is cheap, and the
        // one the ring work is judged by (docs/21 §5.22).
        metrics.record_present(Duration::from_micros(3100));
        metrics.record_present(Duration::from_micros(1900));
        // A slow one is counted separately: the tail on a real 4K session was 93 ms, and the count
        // is what says whether that is warm-up or something recurring.
        metrics.record_present(Duration::from_micros(29_400));
        metrics.record_chain_fade_frame();
        metrics.record_snapshot_refresh(Duration::from_micros(500), 42);
        metrics.record_snapshot_release(Duration::from_micros(4));
        metrics.record_hit_test(Duration::from_micros(1));
        metrics.record_nearest_target(Duration::from_micros(2));
        metrics.record_validate(Duration::from_micros(3));
        metrics.record_hover_target_switch();
        metrics.record_stale_target();
        metrics.record_hover_revalidate_stale_dropped();
        metrics.record_worker_stale_result_dropped();
        metrics.record_mouse_move_coalesced(9);

        let line = metrics.summary_line();
        for expected in [
            "present=3",
            "present_us last=29400 max=29400 first=3100 over16ms=1",
            "chain_fade_frames=1",
            "window_snapshot_refresh_us",
            "window_snapshot_release_us",
            "window_hit_test_us",
            "window_nearest_target_us",
            "window_validate_us",
            "candidate_count=42",
            "hover_target_switch_count=1",
            "stale_target_count=1",
            "hover_revalidate_stale_dropped_count=1",
            "window_worker_queue_depth=",
            "window_worker_stale_result_dropped_count=1",
            "mouse_move_coalesced_count=9",
            "refinement_submitted=",
            "refinement_published=",
            "refinement_empty=",
            "refinement_elapsed_us",
            "refinement_msaa_attempts=",
            "refinement_msaa_timeouts=",
            "refinement_msaa_busy=",
            "refinement_msaa_failures=",
            "refinement_latency_buckets <16ms=",
            "refinement_quarantine_added=",
            "refinement_quarantine_hit=",
            "refinement_inflight_timeouts=",
            "refinement_superseded=",
            "refinement_downgrades_staged=",
            "refinement_follow_ups=",
        ] {
            assert!(line.contains(expected), "missing {expected} in: {line}");
        }
    }

    #[test]
    fn reset_clears_every_counter_for_the_next_session() {
        let metrics = WindowDetectionMetrics::new();
        metrics.record_snapshot_refresh(Duration::from_millis(2), 30);
        metrics.record_hit_test(Duration::from_micros(8));
        metrics.record_worker_enqueued();
        metrics.record_refinement_inflight_timeout();
        metrics.record_refinement_superseded();
        metrics.record_refinement_downgrade_staged();
        metrics.record_refinement_follow_up();
        metrics.reset();
        assert_eq!(metrics.reading(), WindowDetectionReading::default());
    }

    #[test]
    fn the_handle_is_shared_between_threads() {
        let metrics = WindowDetectionMetrics::new();
        let worker_side = metrics.clone();
        let handle = std::thread::spawn(move || {
            worker_side.record_snapshot_refresh(Duration::from_micros(700), 21);
        });
        handle.join().unwrap();
        // The refresh timed on the worker is visible to the overlay's handle.
        assert_eq!(metrics.reading().snapshot_refresh_last_us, 700);
        assert_eq!(metrics.reading().candidate_count, 21);
    }
}
