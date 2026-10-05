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
}

/// A copyable reading of every counter, for assertions and one-line reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowDetectionReading {
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
}

/// Shared diagnostics handle for the window-detection pipeline.
#[derive(Debug, Clone, Default)]
pub struct WindowDetectionMetrics {
    counters: Arc<Counters>,
    verbose: Arc<AtomicBool>,
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
        }
    }

    /// One line carrying every §10.2 metric, for session teardown and probes.
    pub fn summary_line(&self) -> String {
        let reading = self.reading();
        format!(
            "window_snapshot_refresh_us last={} max={} n={} \
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
             refinement_msaa_busy={} refinement_msaa_failures={}",
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
        )
    }

    /// Forget every counter. Called when a capture session ends so the next session's
    /// report is not polluted by the previous one.
    pub fn reset(&self) {
        let counters = &self.counters;
        for counter in [
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
