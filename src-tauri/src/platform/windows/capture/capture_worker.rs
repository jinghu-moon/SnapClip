//! Persistent capture worker decoupled from the overlay UI thread.
//!
//! The overlay thread must never block on WGC, BitBlt or device construction
//! (docs/11 §3.1/§3.3). `F5` therefore submits a [`StartRequest`] into a
//! capacity-1 mailbox and keeps pumping messages; this worker performs the
//! capture and hands back a [`PreparedFrame`] through a second capacity-1
//! mailbox, waking the overlay with a posted thread message.
//!
//! Cancellation never joins the worker: every request carries a `generation`
//! and results whose generation is no longer current are dropped here (which
//! releases the GPU reference immediately). A repeated F5 overwrites a pending
//! request instead of stacking it.
//!
//! The worker also owns the [`CaptureProviders`] — and therefore the D3D11
//! device — for the lifetime of the overlay, so consecutive sessions reuse
//! the same device instead of paying `D3D11CreateDevice` on every F5. Because
//! the renderer consumes the provider textures, the device must stay the one
//! shared through [`FrozenFrame::device`]; it may only be replaced after a
//! display or device change, which is also the moment the renderer is rebuilt.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Instant;

use windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW;

use super::monitor::CapturedMonitor;
use super::providers::{CaptureProviders, FrozenFrame};
use crate::capture::CaptureError;
use crate::capture::geometry::Point;

/// `WM_APP`-based notification posted by the worker when a result is ready.
/// The overlay drains the result mailbox when it sees one of these.
pub const FRAME_READY_MESSAGE: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 18;

/// Request handed from the overlay thread to the worker.
#[derive(Clone)]
pub struct StartRequest {
    /// Sessions and cancellations share one monotonic counter; stale results
    /// with any other generation are dropped.
    pub generation: u64,
    /// The monitor whose pixels the worker must freeze.
    pub monitor: CapturedMonitor,
    /// Physical screen coordinates at hotkey time, retained for the
    /// provider-latency acceptance checks.
    pub cursor_screen: Point,
    /// Submission timestamp; the worker logs the queue latency against it.
    pub requested_at: Instant,
    /// Overlay UI thread id to wake with [`FRAME_READY_MESSAGE`].
    pub notify_thread: u32,
}

/// Result handed back from the worker to the overlay thread.
///
/// `CapturedMonitor` is not `Debug`, so this type does not derive it either; the
/// overlay logs the fields it needs directly.
pub struct PreparedFrame {
    pub generation: u64,
    /// The frozen frame the overlay renders and the artifact crops later.
    pub frozen: FrozenFrame,
    /// The monitor the frame was captured from (renderer layout source).
    pub monitor: CapturedMonitor,
    /// `Instant` the provider returned; pairs with `requested_at`.
    pub captured_at: Instant,
}

/// A failed capture: the error plus the stage that produced it, so the
/// overlay can report the same `provider` string the sync path used.
#[derive(Debug)]
pub struct CaptureFailure {
    pub generation: u64,
    pub error: CaptureError,
    pub stage: &'static str,
}

enum WorkerEvent {
    Ready(Result<PreparedFrame, CaptureFailure>),
    /// Providers had to be (re)created on this request and failed; nothing
    /// was captured and nothing can be until the next session.
    InitializationFailed(CaptureError),
}

/// Capacity-1 request/result mailbox plus the generation counter.
///
/// Split from the thread so the protocol itself is unit-testable without a
/// GPU: overwrite semantics, stale-result discard, single consumption.
struct Mailbox {
    inner: Mutex<MailboxInner>,
    request_available: Condvar,
    current_generation: AtomicU64,
    stopping: AtomicBool,
    /// Display/device change invalidated providers and (for the overlay)
    /// generation-independent: the next request must rebuild.
    providers_dirty: AtomicBool,
}

#[derive(Default)]
struct MailboxInner {
    /// Capacity 1: a new request replaces the one not yet picked up.
    pending_request: Option<StartRequest>,
    /// In-flight request being processed right now, if any.
    active_request: Option<StartRequest>,
    /// Capacity 1: only the newest result is retained.
    result: Option<WorkerEvent>,
    providers: Option<CaptureProviders>,
}

impl Mailbox {
    fn new() -> Self {
        Self {
            inner: Mutex::new(MailboxInner::default()),
            request_available: Condvar::new(),
            current_generation: AtomicU64::new(0),
            stopping: AtomicBool::new(false),
            providers_dirty: AtomicBool::new(false),
        }
    }

    /// Allocate the next generation. Every start and every cancel advances it,
    /// so "current generation" uniquely identifies the live session.
    fn next_generation(&self) -> u64 {
        self.current_generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn current_generation(&self) -> u64 {
        self.current_generation.load(Ordering::Acquire)
    }

    /// Submit a request; returns the replaced request if the worker had not
    /// picked the previous one up yet.
    fn submit(&self, request: StartRequest) -> Option<StartRequest> {
        if self.stopping.load(Ordering::Acquire) {
            return None;
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let replaced = inner.pending_request.replace(request);
        drop(inner);
        self.request_available.notify_one();
        replaced
    }

    /// Mark every unconsumed result stale. The worker drops its own in-flight
    /// frame when it finishes; the overlay simply never sees it, and the GPU
    /// reference goes with it.
    fn invalidate(&self) {
        self.next_generation();
    }

    fn mark_providers_dirty(&self) {
        self.providers_dirty.store(true, Ordering::Release);
    }

    /// Block until the next request the worker must process. `None` means the
    /// worker should exit. Stale queued requests are discarded here as well.
    fn wait_request(&self) -> Option<StartRequest> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            while inner.pending_request.is_none() && !self.stopping.load(Ordering::Acquire) {
                inner = self
                    .request_available
                    .wait(inner)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            if self.stopping.load(Ordering::Acquire) {
                return None;
            }
            let request = inner.pending_request.take().expect("checked non-empty");
            if request.generation != self.current_generation() {
                // Superseded or cancelled while queued: never capture for it.
                continue;
            }
            inner.active_request = Some(request.clone());
            return Some(request);
        }
    }

    /// Publish a result unless its generation is stale; stale results are
    /// dropped (and their GPU references released) right here.
    fn complete(&self, request: &StartRequest, event: WorkerEvent) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.active_request = None;
        if request.generation == self.current_generation()
            && !self.stopping.load(Ordering::Acquire)
        {
            inner.result = Some(event);
            drop(inner);
            // Wake the overlay thread; it drains the mailbox in its message
            // pump. A failed post only means the notification is lost — the
            // next posted message (or the next session) still drains it, and
            // the session ends one way or the other via cancel/timeout paths.
            unsafe {
                PostThreadMessageW(request.notify_thread, FRAME_READY_MESSAGE, 0, 0);
            }
        }
        // `event` falls out of scope here: the texture (or error) is released.
    }

    fn ensure_providers(&self) -> Result<(), CaptureError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner.providers.is_some() && !self.providers_dirty.swap(false, Ordering::AcqRel) {
            return Ok(());
        }
        // Replace (not merely create) so a dirty flag rebuilds the device.
        inner.providers = Some(CaptureProviders::new()?);
        Ok(())
    }

    fn providers(&self) -> Option<CaptureProviders> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.providers.take()
    }

    fn put_providers(&self, providers: CaptureProviders) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.providers = Some(providers);
    }

    /// Non-blocking result drain, called from the overlay message pump.
    fn take_result(&self) -> Option<WorkerEvent> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.result.take()
    }
}

/// The worker handle owned by the overlay controller.
pub struct CaptureWorker {
    mailbox: Arc<Mailbox>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl CaptureWorker {
    pub fn new() -> Self {
        Self {
            mailbox: Arc::new(Mailbox::new()),
            thread: Mutex::new(None),
        }
    }

    /// Allocate the generation for a new session.
    pub fn next_generation(&self) -> u64 {
        self.mailbox.next_generation()
    }

    /// Mark any in-flight or queued result stale (Esc, right click, repeated
    /// F5, shutdown). Never waits for the worker.
    pub fn cancel(&self) {
        self.mailbox.invalidate();
    }

    /// Invalidate the device/providers after WM_DISPLAYCHANGE, WM_DPICHANGED
    /// or WM_DEVICECHANGE. The next request rebuilds them on the worker
    /// thread; whatever is in flight becomes stale through `cancel`.
    pub fn invalidate_providers(&self) {
        self.mailbox.mark_providers_dirty();
    }

    /// Start the worker thread if it is not running, then submit `request`.
    pub fn start(&self, request: StartRequest) -> Result<(), String> {
        self.ensure_running()?;
        if let Some(replaced) = self.mailbox.submit(request) {
            eprintln!(
                "[snapclip][capture] worker request superseded generation={} (queue was full)",
                replaced.generation
            );
        }
        Ok(())
    }

    fn ensure_running(&self) -> Result<(), String> {
        let mut slot = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let alive = slot.as_ref().map(|handle| !handle.is_finished()).unwrap_or(false);
        if alive {
            return Ok(());
        }
        // A finished (crashed) handle from a previous run is reaped here.
        if let Some(handle) = slot.take() {
            let _ = handle.join();
        }
        let mailbox = self.mailbox.clone();
        let thread = thread::Builder::new()
            .name("snapclip-capture-worker".into())
            .spawn(move || worker_main(mailbox))
            .map_err(|error| format!("spawn capture worker failed: {error}"))?;
        *slot = Some(thread);
        Ok(())
    }

    /// Drain one ready result. Results whose generation is no longer current
    /// are dropped here (releasing the texture) and reported as `None`.
    pub fn take_ready(&self) -> Option<Result<PreparedFrame, CaptureFailure>> {
        let event = self.mailbox.take_result()?;
        let current = self.mailbox.current_generation();
        match event {
            WorkerEvent::Ready(result) => {
                let stale = match &result {
                    Ok(frame) => frame.generation != current,
                    Err(failure) => failure.generation != current,
                };
                if stale {
                    eprintln!(
                        "[snapclip][capture] worker result discarded generation mismatch"
                    );
                    None
                } else {
                    Some(result)
                }
            }
            WorkerEvent::InitializationFailed(error) => Some(Err(CaptureFailure {
                generation: current,
                error,
                stage: "provider-init",
            })),
        }
    }

    /// Stop the thread and release the providers (and with them the last
    /// texture reference) on the worker side. The overlay has already dropped
    /// its renderer by the time this runs.
    pub fn shutdown(&self) {
        self.mailbox.invalidate();
        self.mailbox.stopping.store(true, Ordering::Release);
        // Notify while holding the lock: a parked `wait_request` re-checks
        // `stopping` when it wakes, and a worker that is about to wait cannot
        // miss the flag because it inspects it under the same lock.
        let _guard = self
            .mailbox
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.mailbox.request_available.notify_all();
        drop(_guard);
        let handle = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

fn worker_main(mailbox: Arc<Mailbox>) {
    loop {
        let Some(request) = mailbox.wait_request() else {
            return;
        };
        if let Err(error) = mailbox.ensure_providers() {
            eprintln!("[snapclip][capture] provider initialization failed: {error}");
            mailbox.complete(&request, WorkerEvent::InitializationFailed(error));
            continue;
        }
        // Providers are borrowed for the capture by take/put so no lock is
        // held across the (potentially hundreds of ms) capture itself.
        let Some(mut providers) = mailbox.providers() else {
            mailbox.complete(
                &request,
                WorkerEvent::InitializationFailed(CaptureError::ProviderUnavailable(
                    "capture providers vanished".into(),
                )),
            );
            continue;
        };
        let queue_ms = request.requested_at.elapsed().as_millis();
        eprintln!(
            "[snapclip][capture] worker start generation={} monitor={}x{} cursor=({},{}) queue_ms={}",
            request.generation,
            request.monitor.width(),
            request.monitor.height(),
            request.cursor_screen.x,
            request.cursor_screen.y,
            queue_ms
        );
        let started_at = Instant::now();
        let event = match providers.capture(&request.monitor) {
            Ok(frozen) => {
                eprintln!(
                    "[snapclip][capture] worker frame ready generation={} provider={} size={}x{} capture_ms={} total_ms={}",
                    request.generation,
                    frozen.frame.provider,
                    frozen.frame.width,
                    frozen.frame.height,
                    started_at.elapsed().as_millis(),
                    request.requested_at.elapsed().as_millis()
                );
                WorkerEvent::Ready(Ok(PreparedFrame {
                    generation: request.generation,
                    monitor: request.monitor.clone(),
                    captured_at: Instant::now(),
                    frozen,
                }))
            }
            Err(error) => {
                eprintln!(
                    "[snapclip][capture] worker capture failed generation={} elapsed_ms={} error={}",
                    request.generation,
                    started_at.elapsed().as_millis(),
                    error
                );
                let stage = if matches!(error, CaptureError::DeviceRemoved(_)) {
                    "device"
                } else {
                    "provider"
                };
                WorkerEvent::Ready(Err(CaptureFailure {
                    generation: request.generation,
                    error,
                    stage,
                }))
            }
        };
        mailbox.put_providers(providers);
        mailbox.complete(&request, event);
    }
}

#[cfg(test)]
mod tests {
    use super::{Mailbox, StartRequest, WorkerEvent};
    use crate::capture::CaptureError;
    use crate::capture::geometry::{MonitorLayout, Point};
    use crate::platform::windows::capture::monitor::CapturedMonitor;
    use std::time::Instant;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;

    fn request(generation: u64) -> StartRequest {
        StartRequest {
            generation,
            monitor: CapturedMonitor {
                handle: 1,
                layout: MonitorLayout {
                    bounds: crate::capture::geometry::Rect::new(0, 0, 1920, 1080),
                    work_area: crate::capture::geometry::Rect::new(0, 0, 1920, 1040),
                    dpi: 96,
                    primary: true,
                },
            },
            cursor_screen: Point::new(0, 0),
            requested_at: Instant::now(),
            notify_thread: unsafe { GetCurrentThreadId() },
        }
    }

    #[test]
    fn generations_increase_monotonically() {
        let mailbox = Mailbox::new();
        assert_eq!(mailbox.next_generation(), 1);
        assert_eq!(mailbox.next_generation(), 2);
        mailbox.invalidate();
        assert_eq!(mailbox.current_generation(), 3);
    }

    #[test]
    fn pending_request_is_overwritten_not_stacked() {
        let mailbox = Mailbox::new();
        assert!(mailbox.submit(request(1)).is_none());
        let replaced = mailbox.submit(request(2)).expect("capacity 1 replaces");
        assert_eq!(replaced.generation, 1);
    }

    #[test]
    fn stale_requests_are_never_handed_to_the_worker() {
        // Emulate the worker side without a GPU: a request queued for generation 1
        // must be skipped once generation 2 becomes current, and the worker must
        // only return when it picks up a matching request.
        let mailbox = std::sync::Arc::new(Mailbox::new());
        let stale = mailbox.next_generation(); // current -> 1
        mailbox.submit(request(stale));
        let fresh = mailbox.next_generation(); // current -> 2; the queued req is stale

        let waiter_mailbox = mailbox.clone();
        let waiter = std::thread::spawn(move || {
            waiter_mailbox.wait_request().map(|taken| taken.generation)
        });

        // Let the worker take and discard the stale request, then hand it a
        // matching one. Even if it has not started waiting yet, `submit` either
        // overwrites the stale request or wakes the loop, so the outcome is
        // deterministic: the worker returns the fresh generation.
        std::thread::sleep(std::time::Duration::from_millis(50));
        mailbox.submit(request(fresh));
        assert_eq!(waiter.join().unwrap(), Some(fresh));
    }

    #[test]
    fn results_with_a_stale_generation_are_dropped() {
        let mailbox = Mailbox::new();
        let req = request(5);
        mailbox.invalidate(); // generation moved on while the worker was busy
        mailbox.complete(&req, WorkerEvent::InitializationFailed(
            CaptureError::ProviderUnavailable("test".into()),
        ));
        // The event never reached the mailbox, so the overlay has nothing to
        // consume: no result, and no wake-up consumed anything either.
        assert!(mailbox.take_result().is_none());
    }

    #[test]
    fn results_with_the_current_generation_are_delivered_once() {
        let mailbox = Mailbox::new();
        let req = request(mailbox.next_generation());
        mailbox.complete(&req, WorkerEvent::InitializationFailed(
            CaptureError::ProviderUnavailable("test".into()),
        ));
        assert!(mailbox.take_result().is_some());
        assert!(mailbox.take_result().is_none(), "the mailbox holds one consumable result");
    }
}
