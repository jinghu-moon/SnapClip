//! Deadline-bounded execution for accessibility provider calls (docs/18 §12.2).
//!
//! MSAA calls into another process's provider and can block indefinitely; a COM call cannot be
//! interrupted from outside. The reference selector therefore runs each query on a *detached*
//! worker with a deadline, and distinguishes three outcomes that mean very different things:
//!
//! * the call finished in time → use it;
//! * the deadline passed → **quarantine the window** (its provider is misbehaving);
//! * the runner was already at capacity → **retry later, never quarantine** — being busy says
//!   nothing about the window.
//!
//! Capacity is what keeps abandoned calls from turning into an unbounded thread leak: a call
//! that timed out keeps holding its slot until it actually returns.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// What a deadline-bounded call produced.
#[derive(Debug)]
pub enum TimedOutcome<T> {
    /// The work finished before the deadline.
    Completed(T),
    /// The deadline passed. The work is abandoned and may still be running.
    TimedOut,
    /// The runner was at capacity, so nothing was attempted.
    Busy,
}

/// Runs provider calls with a deadline and bounded concurrency.
#[derive(Debug, Clone)]
pub struct TimedCallRunner {
    max_in_flight: usize,
    in_flight: Arc<AtomicUsize>,
}

impl TimedCallRunner {
    /// `max_in_flight` is the number of *abandoned* calls that may pile up before new requests
    /// are refused. One is usually enough: a second misbehaving provider is not worth more
    /// threads, and refusing is cheap because the caller simply retries on the next dwell.
    pub fn new(max_in_flight: usize) -> Self {
        Self {
            max_in_flight: max_in_flight.max(1),
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Slots currently held, including abandoned calls that have not returned yet.
    ///
    /// Only diagnostics and tests read this today; the MSAA provider acts on the outcome
    /// instead of inspecting the counter.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn max_in_flight(&self) -> usize {
        self.max_in_flight
    }

    /// Run `work`, waiting at most `timeout` for it.
    pub fn run<T, F>(&self, work: F, timeout: Duration) -> TimedOutcome<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        // Reserve a slot first: admission control must be decided before any thread is made.
        let mut current = self.in_flight.load(Ordering::SeqCst);
        loop {
            if current >= self.max_in_flight {
                return TimedOutcome::Busy;
            }
            match self.in_flight.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }

        let slot = InFlightSlot {
            counter: Arc::clone(&self.in_flight),
        };
        let (sender, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("snapclip-msaa-call".into())
            .spawn(move || {
                // The slot is released when the closure returns, however long that takes.
                let _slot = slot;
                let value = work();
                let _ = sender.send(value);
            });
        if spawned.is_err() {
            // Could not spawn: release the slot immediately and report it as busy so the
            // caller retries instead of quarantining the window for our own failure.
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            return TimedOutcome::Busy;
        }

        match receiver.recv_timeout(timeout) {
            Ok(value) => TimedOutcome::Completed(value),
            Err(_) => TimedOutcome::TimedOut,
        }
    }
}

impl Default for TimedCallRunner {
    fn default() -> Self {
        Self::new(1)
    }
}

/// Releases the admission slot when the (possibly abandoned) call finally finishes.
struct InFlightSlot {
    counter: Arc<AtomicUsize>,
}

impl Drop for InFlightSlot {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn a_call_that_finishes_in_time_is_returned() {
        let runner = TimedCallRunner::new(1);
        let outcome = runner.run(|| 7u32, Duration::from_secs(5));
        assert!(matches!(outcome, TimedOutcome::Completed(7)));
        // The slot is released as soon as the call returns.
        let deadline = Instant::now() + Duration::from_secs(2);
        while runner.in_flight() != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(runner.in_flight(), 0);
    }

    #[test]
    fn a_call_past_its_deadline_is_abandoned_but_keeps_its_slot() {
        let runner = TimedCallRunner::new(1);
        let outcome = runner.run(
            || {
                std::thread::sleep(Duration::from_millis(300));
                1u32
            },
            Duration::from_millis(20),
        );
        assert!(matches!(outcome, TimedOutcome::TimedOut));
        // Admission control: the abandoned call still occupies its slot, so a second request
        // is refused instead of spawning another thread (this is what bounds the leak).
        assert_eq!(runner.in_flight(), 1, "the abandoned call still holds its slot");
        assert!(matches!(
            runner.run(|| 2u32, Duration::from_millis(5)),
            TimedOutcome::Busy
        ));

        // Once the abandoned call finishes, the slot frees up and work is admitted again.
        let deadline = Instant::now() + Duration::from_secs(2);
        while runner.in_flight() != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(runner.in_flight(), 0);
        assert!(matches!(
            runner.run(|| 3u32, Duration::from_secs(2)),
            TimedOutcome::Completed(3)
        ));
    }

    #[test]
    fn the_runner_never_exceeds_its_capacity() {
        let runner = TimedCallRunner::new(2);
        assert_eq!(runner.max_in_flight(), 2);
        let blocked = Duration::from_millis(200);
        let first = runner.run(move || std::thread::sleep(blocked), Duration::from_millis(10));
        let second = runner.run(move || std::thread::sleep(blocked), Duration::from_millis(10));
        assert!(matches!(first, TimedOutcome::TimedOut));
        assert!(matches!(second, TimedOutcome::TimedOut));
        assert_eq!(runner.in_flight(), 2);
        // Third request while both slots are taken.
        assert!(matches!(
            runner.run(|| (), Duration::from_millis(5)),
            TimedOutcome::Busy
        ));
    }
}
