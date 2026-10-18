//! The scroll capture thread (`docs/33`): monitor frames in, the long image and its thumbnail out.
//!
//! One thread owns the WGC monitor session and the [`Tracker`]. Every frame it takes is the newest
//! one delivered; only the selection is read back, measured against the reference and placed. The
//! overlay is woken with [`SCROLL_READY_MESSAGE`] and pulls a [`Report`]. The overlay never sees
//! itself in these frames (`WDA_EXCLUDEFROMCAPTURE`), which is why the monitor can be captured while
//! the panel sits on top of it.
//!
//! Auto mode posts one wheel notch to the window under the selection's centre ([`post_wheel`]),
//! and then waits for the page to show it ([`observe`]): a notch is a request, not a result, so
//! the next one is only sent once the frame it produced has been measured. [`AUTO_IDLE_NOTCHES`]
//! notches in a row that stayed unanswered for [`OBSERVE_TIMEOUT`] turn auto off, which is what
//! "the page has reached its end" looks like from outside the page; a page that keeps repainting
//! without moving is named as such in the log instead. A window that refuses the posted wheel
//! (UIPI: it runs elevated) turns auto off at once, and the toolbar says so, rather than ten
//! silent failures reading as "the end of the page".

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{PtInRect, ScreenToClient};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, ChildWindowFromPointEx, GW_HWNDNEXT,
    GWL_EXSTYLE, GetClassNameW, GetTopWindow, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsWindowVisible, PostMessageW, PostThreadMessageW, WM_MOUSEWHEEL,
    WS_EX_LAYERED, WS_EX_TRANSPARENT,
};

use super::overlay::SCROLL_READY_MESSAGE;
use super::win::d3d11::GraphicsDevice;
use super::win::wgc::{WgcFrame, WgcSession};
use crate::geometry::{Point, Rect};
use crate::scroll::estimate::Estimate;
use crate::scroll::stitch::Placement;
use crate::scroll::{should_log, Auto, Preview, Status, Step, Tracker};

/// One auto notch every this long: slow enough for a page to repaint before the next one.
pub(crate) const AUTO_INTERVAL: Duration = Duration::from_millis(200);

/// Diagnostics switch: split every frame into read / push so a real session can say where its
/// time goes. Reading the clock per frame is not free, so it stays off unless asked for.
const SCROLL_VERBOSE_ENV: &str = "SNAPCLIP_SCROLL_VERBOSE";

/// Whether [`SCROLL_VERBOSE_ENV`] is set. Resolved once per session: a switch read per frame
/// would itself be a per-frame cost.
fn verbose_timing() -> bool {
    static VERBOSE: OnceLock<bool> = OnceLock::new();
    *VERBOSE.get_or_init(|| std::env::var_os(SCROLL_VERBOSE_ENV).is_some())
}

/// Auto mode ends after this many notches in a row moved nothing.
pub(crate) const AUTO_IDLE_NOTCHES: u32 = 10;
/// One wheel notch towards the user (`WHEEL_DELTA`, scrolling the page down).
pub(crate) const NOTCH_DOWN: i32 = -120;
/// How long one wait for a frame lasts, so the stop flag and the auto clock are looked at often.
const FRAME_WAIT: Duration = Duration::from_millis(20);
/// How long a notch waits for the page to show it. Posting a notch is a request, not a result: a
/// slow page, a remote session, or a program that repaints lazily can take longer than the interval
/// between notches to show the previous one, and counting that as "the page did not move" ends auto
/// mode on a page that still has content below. Five notches' worth of slack is past the point
/// where the page is merely slow, and well under a user's patience.
const OBSERVE_TIMEOUT: Duration = Duration::from_millis(1000);

/// Environment override for the notch spacing, read only in builds that carry the measurement
/// tools (`test-support`). The cadence is a *time* constant, not a frame count: auto mode posts a
/// notch no sooner than [`AUTO_INTERVAL`] after the previous one and judges it by the first frame
/// that lands at least one page latency later, so the cost of a capture is notches multiplied by
/// the larger of those two. `docs/39` section 10 measures the three candidate spacings.
#[cfg(feature = "test-support")]
const AUTO_INTERVAL_ENV: &str = "SCROLL_AUTO_INTERVAL_MS";

/// A millisecond override, clamped so a typo cannot post notches in a tight loop.
///
/// Zero, a negative number, a non-numeric string and anything above five seconds are all rejected:
/// the first would spin the wheel as fast as the loop runs, the last is slower than any page needs.
#[cfg(any(test, feature = "test-support"))]
fn parse_ms_override(raw: &str) -> Option<Duration> {
    let ms: u64 = raw.trim().parse().ok()?;
    (20..=5_000).contains(&ms).then(|| Duration::from_millis(ms))
}

/// [`AUTO_INTERVAL`], or the override when a measurement build was asked for one.
///
/// Resolved once per session, like [`verbose_timing`]: reading the environment per notch would be a
/// per-notch cost, and the value must not change under a running session.
fn auto_interval() -> Duration {
    #[cfg(feature = "test-support")]
    {
        static OVERRIDE: OnceLock<Option<Duration>> = OnceLock::new();
        if let Some(interval) = *OVERRIDE.get_or_init(|| {
            std::env::var(AUTO_INTERVAL_ENV)
                .ok()
                .and_then(|raw| parse_ms_override(&raw))
        }) {
            return interval;
        }
    }
    AUTO_INTERVAL
}

/// One notch that has been posted and is waiting for the page to show it.
#[derive(Debug, Clone, Copy)]
struct Pending {
    /// When the observation window closes if nothing has moved by then.
    until: Instant,
    /// Whether any frame arrived at all inside the window, which is what separates "no frames" from
    /// "frames that did not move".
    saw_frame: bool,
}

/// What one posted notch did, as far as the capture thread can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Observation {
    /// The page moved: the notch (or the user's own wheel) displaced the frame.
    Moved,
    /// The window closed without a displacement. `saw_frame` says whether anything was redrawn.
    NoMove { saw_frame: bool },
}

/// What the overlay pulls on every wake-up.
#[derive(Debug, Clone, Default)]
pub(crate) struct Report {
    /// The thumbnail, when it changed since the last report.
    pub preview: Option<Preview>,
    /// `None` until the first frame arrived.
    pub status: Option<Status>,
    pub auto: Auto,
    /// Set once, when the thread ended on an error; the session cannot continue.
    pub failure: Option<String>,
}

/// Everything the thread needs, decided by the overlay before it starts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Spec {
    /// The `HMONITOR` the selection is on.
    pub monitor: isize,
    /// The selection in monitor-local physical pixels (the WGC monitor frame's space).
    pub region: Rect,
    /// The selection's centre in virtual-desktop physical pixels: where the wheel is aimed.
    pub aim: Point,
    /// The widest thumbnail the panel shows, in pixels.
    pub thumb_width: u32,
    /// The most thumbnail rows the panel shows.
    pub preview_rows: u32,
    /// The overlay's thread, woken with [`SCROLL_READY_MESSAGE`].
    pub notify_thread: u32,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    auto: AtomicBool,
    /// The last auto run ended because the target refused the wheel.
    refused: AtomicBool,
    /// Set when a wake-up is posted and cleared when the overlay takes the report, so a burst of
    /// frames is one message rather than one per frame.
    woken: AtomicBool,
    report: Mutex<Report>,
    /// The buffer the overlay has finished with. The capture thread gathers the next thumbnail into
    /// it rather than allocating one per frame; see `Tracker::fill_preview`. Empty when the overlay
    /// has not handed one back, which is the case for the first frame of a session and nothing else.
    recycled: Mutex<Vec<u8>>,
}

pub(crate) struct ScrollWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<Option<Tracker>>>,
    aim: Point,
}

impl ScrollWorker {
    pub(crate) fn start(spec: Spec) -> Self {
        let shared = Arc::new(Shared::default());
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("snapclip-scroll".into())
                .spawn(move || run(spec, &shared))
                .ok()
        };
        if thread.is_none() {
            shared.report.lock().unwrap().failure = Some("the scroll thread did not start".into());
        }
        Self {
            shared,
            thread,
            aim: spec.aim,
        }
    }

    /// The latest state; the thumbnail only when it changed since the last call.
    pub(crate) fn take_report(&self) -> Report {
        self.shared.woken.store(false, Ordering::Release);
        let mut report = self.shared.report.lock().unwrap();
        Report {
            preview: report.preview.take(),
            status: report.status,
            auto: self.shared.auto_state(),
            failure: report.failure.clone(),
        }
    }

    /// Give the thumbnail's buffer back for the next frame to fill. The bytes are already in D2D's
    /// bitmap by the time this is called, so the capture thread may write over them.
    pub(crate) fn recycle_preview(&self, mut bytes: Vec<u8>) {
        bytes.clear();
        *self.shared.recycled.lock().unwrap() = bytes;
    }

    pub(crate) fn auto(&self) -> bool {
        self.shared.auto.load(Ordering::Acquire)
    }

    pub(crate) fn set_auto(&self, on: bool) {
        self.shared.set_auto(on);
    }

    /// A wheel the overlay received itself (the pointer was over the panel or the toolbar): the
    /// page under the selection is what the user meant to scroll.
    pub(crate) fn forward_wheel(&self, delta: i32) {
        if let Err(error) = post_wheel(self.aim, delta) {
            eprintln!("[snapclip][scroll] wheel forward failed: {error}");
        }
    }

    /// Stop and hand over the long image `(width, rows, bgra)`; `None` when no frame ever arrived.
    ///
    /// The budgets are printed once here because this is the only point that knows what the image
    /// cost (docs/35 §5.1): working memory is the two edge buffers, temporary storage is the tile
    /// file, and the file size belongs to the encoder. A session that ends with a full disk or a
    /// failed seal has its reason on stderr already; this line says how close it came.
    pub(crate) fn finish(mut self) -> Option<crate::scroll::ScrollBands> {
        let tracker = self.join()?;
        let budgets = tracker.budgets();
        eprintln!(
            "[snapclip][scroll] image {}x{}: working {:.1} MB ({} rows), temp {:.1} MB",
            tracker.width(),
            budgets.pixel_rows,
            budgets.working_bytes as f64 / 1e6,
            budgets.pixel_rows,
            budgets.temp_bytes as f64 / 1e6,
        );
        Some(crate::scroll::ScrollBands::new(tracker))
    }

    fn join(&mut self) -> Option<Tracker> {
        self.shared.stop.store(true, Ordering::Release);
        self.thread.take()?.join().ok().flatten()
    }
}

impl Drop for ScrollWorker {
    /// Cancelling is dropping: the thread stops and the image is thrown away.
    fn drop(&mut self) {
        self.join();
    }
}

impl Shared {
    fn set_auto(&self, on: bool) {
        eprintln!("[snapclip][scroll] auto={on}");
        self.refused.store(false, Ordering::Release);
        self.auto.store(on, Ordering::Release);
    }

    fn auto_state(&self) -> Auto {
        if self.auto.load(Ordering::Acquire) {
            Auto::On
        } else if self.refused.load(Ordering::Acquire) {
            Auto::Refused
        } else {
            Auto::Off
        }
    }

    /// What one auto notch's outcome does to auto mode. `last` is the window the previous notch
    /// went to, so the log names each new target once. Returns `true` when auto turned itself off.
    fn after_notch(&self, posted: Result<Target, WheelError>, last: &mut Option<isize>) -> bool {
        match posted {
            Ok(target) => {
                if *last != Some(target.hwnd) {
                    eprintln!("[snapclip][scroll] auto target {target}");
                    *last = Some(target.hwnd);
                }
                false
            }
            Err(WheelError::Denied(target)) => {
                eprintln!(
                    "[snapclip][scroll] auto ended: {target} refused the wheel (access denied: it \
                     runs at a higher integrity level)"
                );
                self.auto.store(false, Ordering::Release);
                self.refused.store(true, Ordering::Release);
                true
            }
            Err(error) => {
                eprintln!("[snapclip][scroll] auto notch failed: {error}");
                false
            }
        }
    }
}

fn run(spec: Spec, shared: &Shared) -> Option<Tracker> {
    let mut tracker = None;
    if let Err(error) = capture(&spec, shared, &mut tracker) {
        eprintln!("[snapclip][scroll] failed: {error}");
        shared.auto.store(false, Ordering::Release);
        shared.report.lock().unwrap().failure = Some(error);
        wake(&spec, shared);
    }
    tracker
}

fn capture(spec: &Spec, shared: &Shared, tracker: &mut Option<Tracker>) -> Result<(), String> {
    let device = GraphicsDevice::create()?;
    let mut session = WgcSession::open_monitor(&device, spec.monitor).map_err(|e| e.to_string())?;
    let (width, view) = (spec.region.width() as u32, spec.region.height() as u32);
    eprintln!(
        "[snapclip][scroll] started monitor_frame={:?} region={:?} aim=({},{}) options={:?} {}",
        session.size(),
        spec.region,
        spec.aim.x,
        spec.aim.y,
        session.options(),
        session.diagnostics().join("; ")
    );
    let mut next_notch = Instant::now();
    let mut idle_notches = 0;
    let mut pending: Option<Pending> = None;
    let mut last_target = None;
    let mut frames = 0u64;
    let mut quiet = 0u64;
    // Measurement only (docs/39 section 3): a throttled log can only report the frames that had
    // news, so a session where most frames moved nothing looks, from the log alone, like one
    // where every frame did. These counts close that gap and print once, at the end.
    #[cfg(any(test, feature = "test-support"))]
    let mut placement_stats = crate::scroll::stats::PlacementStats::default();
    // Per-frame cost split, accumulated for the closing line: a real session must be able to say
    // where its time went without a rebuild and without flooding stderr.
    let mut read_total_us = 0u64;
    let mut push_total_us = 0u64;
    // `us` is `read_region` + `push`; keeping every sample lets the closing line report percentiles
    // and the part of `us` that is neither, which is the difference a mean alone hides.
    let mut total_us: Vec<u64> = Vec::new();
    let mut read_us_all: Vec<u64> = Vec::new();
    let mut published: Option<Status> = None;
    // Whether the frame the loop measured last moved the image: this is the evidence a pending
    // notch is judged by, so auto mode reads the page instead of the clock.
    let mut frame_moved = false;
    while !shared.stop.load(Ordering::Acquire) {
        if shared.auto.load(Ordering::Acquire) {
            let now = Instant::now();
            // A notch is a request; the page has to show it before the next one is worth sending.
            // `pending` is only resolved here, from the frames the loop has already measured, so a
            // slow page gets its window instead of being counted as "moved nothing".
            let observed = pending
                .as_mut()
                .and_then(|flight| observe(flight, frame_moved, now));
            if let Some(observed) = observed {
                pending = None;
                match idle_after(observed, &mut idle_notches) {
                    Idle::Waiting | Idle::Moved => {}
                    Idle::GiveUp => {
                        eprintln!(
                            "[snapclip][scroll] auto ended: {idle_notches} notches moved nothing \
                             within {} ms ({})",
                            OBSERVE_TIMEOUT.as_millis(),
                            match observed {
                                Observation::NoMove { saw_frame: true } =>
                                    "the page repainted without moving",
                                _ => "no new frame arrived",
                            }
                        );
                        shared.auto.store(false, Ordering::Release);
                        wake(spec, shared);
                    }
                }
            } else if pending.is_none() && now >= next_notch {
                if shared.after_notch(post_wheel(spec.aim, NOTCH_DOWN), &mut last_target) {
                    wake(spec, shared);
                }
                pending = Some(Pending {
                    until: now + OBSERVE_TIMEOUT,
                    saw_frame: false,
                });
                next_notch = now + auto_interval();
            }
        } else {
            idle_notches = 0;
            pending = None;
        }

        let Some(mut frame) = session.next_frame(FRAME_WAIT).map_err(|e| e.to_string())? else {
            continue;
        };
        // Only the newest frame matters; the ones it replaces were never shown to anyone.
        while let Some(newer) = session
            .next_frame(Duration::ZERO)
            .map_err(|e| e.to_string())?
        {
            frame = newer;
        }
        let started = Instant::now();
        let pixels = read_region(&device, &frame, spec.region)?;
        drop(frame);
        let read_us = started.elapsed().as_micros();
        frames += 1;
        let tracker = match tracker {
            Some(tracker) => tracker,
            None => {
                eprintln!("[snapclip][scroll] first frame {width}x{view}");
                tracker.insert(Tracker::new(&pixels, width, view, spec.thumb_width))
            }
        };
        if frames > 1 {
            let pushed = Instant::now();
            let step = tracker.push(&pixels);
            let push_us = pushed.elapsed().as_micros();
            #[cfg(any(test, feature = "test-support"))]
            placement_stats.record(&step.estimate, step.placement.as_ref());
            let us = started.elapsed().as_micros();
            read_total_us += read_us as u64;
            push_total_us += push_us as u64;
            total_us.push(us as u64);
            read_us_all.push(read_us as u64);
            #[cfg(any(test, feature = "test-support"))]
            if frames % 1000 == 0 {
                // The three counters that grow with frames rather than with rows: the two
                // per-frame sample vectors and the image itself. Whatever tracks `ws` is the
                // owner of the rise (docs/39 section 3).
                eprintln!(
                    "[snapclip][scroll] mem: frame={frames} ws={} timings={} reads={} rows={}",
                    super::proc_mem::working_set_mb(),
                    total_us.len(),
                    read_us_all.len(),
                    tracker.image_rows(),
                );
            }
            if verbose_timing() {
                eprintln!(
                    "[snapclip][scroll] frame={frames} read_us={read_us} push_us={push_us} \
                     thumb_us={} total_us={us} bytes={}",
                    tracker.last_thumb_us(),
                    pixels.len()
                );
            }
            // This frame is the evidence the notch in flight is judged by. A Shift is the page
            // showing the notch; any frame at all is the page repainting, which is worth telling
            // apart from a page that stopped answering. A repaint also buys the notch more time:
            // the window is "no change for a second", so a page that keeps drawing gets to finish
            // the scroll it started.
            if let Some(flight) = pending.as_mut() {
                flight.saw_frame = true;
                flight.until = Instant::now() + OBSERVE_TIMEOUT;
            }
            frame_moved = matches!(step.estimate, Estimate::Shift { .. });
            let Outcome { news, moved } = outcome(&step, published);
            if news {
                // 逐帧那条日志是诊断，不是心跳：一帧一行比一帧本身还贵（docs/36）。
                if should_log(frames, tracker.status(), published) {
                    eprintln!(
                        "[snapclip][scroll] frame={frames} {:?} placement={:?} rows={} status={:?} \
                         quiet_since_last={quiet} us={us}",
                        step.estimate,
                        step.placement,
                        tracker.image_rows(),
                        tracker.status(),
                    );
                }
                quiet = 0;
            } else {
                quiet += 1;
            }
            if !moved && published == Some(tracker.status()) {
                // Nothing the panel shows changed.
                continue;
            }
        }
        published = Some(tracker.status());
        if tracker.status() == Status::Full && shared.auto.swap(false, Ordering::AcqRel) {
            eprintln!("[snapclip][scroll] auto ended: the image is full");
        }
        // The buffer the overlay finished with, if it has handed one back: the next thumbnail is
        // gathered into it instead of into a fresh 616 KiB allocation. Taking it out from under the
        // lock leaves the slot empty while the gathering runs, which is what the overlay expects.
        let recycled = std::mem::take(&mut *shared.recycled.lock().unwrap());
        let preview = tracker.fill_preview(spec.preview_rows, recycled);
        {
            let mut report = shared.report.lock().unwrap();
            report.preview = Some(preview);
            report.status = Some(tracker.status());
        }
        wake(spec, shared);
    }
    let pushed = frames.saturating_sub(1).max(1);
    // `rest` is the part of `us` that neither the readback nor the tracker accounts for, so a frame
    // whose cost is not where the code says it should be is visible in one line.
    let rest_us: Vec<u64> = total_us
        .iter()
        .zip(&read_us_all)
        .map(|(total, read)| total.saturating_sub(*read))
        .collect();
    eprintln!(
        "[snapclip][scroll] stopped frames={frames} read_mean_us={} push_mean_us={} \
         read_us={} push_us={}",
        read_total_us / pushed,
        push_total_us / pushed,
        stats(&read_us_all),
        stats(&rest_us)
    );
    #[cfg(any(test, feature = "test-support"))]
    eprintln!("[snapclip][scroll] {}", placement_stats.line());
    Ok(())
}

/// `mean/p50/p95/max` of a sample set, in microseconds. Microsecond resolution means `push` is
/// usually below one tick, which is why the empty set has to read as zeros rather than divide by it.
fn stats(samples: &[u64]) -> String {
    if samples.is_empty() {
        return "mean=0 p50=0 p95=0 max=0".into();
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let at = |share: f64| sorted[((sorted.len() - 1) as f64 * share).round() as usize];
    let mean = sorted.iter().sum::<u64>() / sorted.len() as u64;
    format!(
        "mean={mean} p50={} p95={} max={}",
        at(0.50),
        at(0.95),
        sorted[sorted.len() - 1]
    )
}

/// What one measured frame means outside the tracker.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    /// Worth a log line.
    news: bool,
    /// The image or the viewport changed, so the panel has something new to show.
    moved: bool,
}

/// A still frame is not news. Neither is a full image's shift past the first: the page moves on and
/// the image does not, so it would otherwise log, rebuild the preview and wake the overlay on every
/// frame for as long as the user keeps scrolling.
fn outcome(step: &Step, published: Option<Status>) -> Outcome {
    let full = step.placement == Some(Placement::Full);
    Outcome {
        news: match step.estimate {
            Estimate::Still { .. } => false,
            Estimate::Shift { .. } => !full || published != Some(Status::Full),
            Estimate::Unknown { .. } => true,
        },
        moved: step.placement.is_some() && !full,
    }
}

/// What one resolved observation does to the run of notches that moved nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Idle {
    /// The page moved: the run is over.
    Moved,
    /// It did not move, but the page has not run out of chances yet.
    Waiting,
    /// Enough notches in a row moved nothing: auto mode stops.
    GiveUp,
}

fn idle_after(observed: Observation, idle_notches: &mut u32) -> Idle {
    match observed {
        Observation::Moved => {
            *idle_notches = 0;
            Idle::Moved
        }
        Observation::NoMove { .. } => {
            *idle_notches += 1;
            if *idle_notches >= AUTO_IDLE_NOTCHES {
                Idle::GiveUp
            } else {
                Idle::Waiting
            }
        }
    }
}

/// Judge the notch in flight. `Some(outcome)` when its window is over: either the page moved
/// inside it, or the deadline passed. `None` while it is still worth waiting for.
fn observe(pending: &mut Pending, moved: bool, now: Instant) -> Option<Observation> {
    if moved {
        return Some(Observation::Moved);
    }
    if now >= pending.until {
        return Some(Observation::NoMove {
            saw_frame: pending.saw_frame,
        });
    }
    None
}

fn wake(spec: &Spec, shared: &Shared) {
    if !shared.woken.swap(true, Ordering::AcqRel) {
        unsafe { PostThreadMessageW(spec.notify_thread, SCROLL_READY_MESSAGE, 0, 0) };
    }
}

/// The selection's pixels out of one monitor frame: one GPU→CPU copy of the selection only.
fn read_region(device: &GraphicsDevice, frame: &WgcFrame, region: Rect) -> Result<Vec<u8>, String> {
    let (frame_w, frame_h) = frame.size();
    if region.left < 0 || region.top < 0 || region.right > frame_w || region.bottom > frame_h {
        return Err(format!(
            "the selection {region:?} is not inside the {frame_w}x{frame_h} monitor frame"
        ));
    }
    let (width, height) = (region.width() as u32, region.height() as u32);
    let pixels = device.read_back_region_bgra(
        frame.texture(),
        region.left as u32,
        region.top as u32,
        width,
        height,
    )?;
    if pixels.len() != width as usize * height as usize * 4 {
        return Err(format!(
            "the region readback returned {} bytes for {width}x{height}",
            pixels.len()
        ));
    }
    Ok(pixels)
}

/// The window a wheel went to, for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub hwnd: isize,
    pub class: String,
    pub process: u32,
}

impl Target {
    fn of(window: HWND) -> Self {
        let mut process = 0;
        unsafe { GetWindowThreadProcessId(window, &mut process) };
        let mut class = [0u16; 256];
        let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
        Self {
            hwnd: window as isize,
            class: String::from_utf16_lossy(&class[..length.max(0) as usize]),
            process,
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "hwnd=0x{:x} class={} pid={}",
            self.hwnd, self.class, self.process
        )
    }
}

#[derive(Debug)]
pub(crate) enum WheelError {
    /// No window of another process is under the point.
    NoWindow(Point),
    /// `ERROR_ACCESS_DENIED`: UIPI dropped the message, because the target runs at a higher
    /// integrity level than we do. Posting again cannot succeed.
    Denied(Target),
    Failed(Target, std::io::Error),
}

impl WheelError {
    /// `PostMessageW` failed on `target` with `error`.
    fn post_failed(target: Target, error: std::io::Error) -> Self {
        if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) {
            Self::Denied(target)
        } else {
            Self::Failed(target, error)
        }
    }
}

impl std::fmt::Display for WheelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoWindow(at) => write!(f, "no window under ({}, {})", at.x, at.y),
            Self::Denied(target) => write!(f, "PostMessageW to {target} was denied"),
            Self::Failed(target, error) => write!(f, "PostMessageW to {target} failed: {error}"),
        }
    }
}

/// Post one wheel message to the window under `aim` (virtual-desktop pixels), skipping this
/// process's windows. Returns the window it went to.
///
/// The window is found by walking the Z order, not by `WindowFromPoint`: the overlay is ours and
/// may cover the point, and the cursor must not move. Then down through the children, because the
/// innermost window is the one that scrolls (the reference implementation's rule, `docs/33`).
pub(crate) fn post_wheel(aim: Point, delta: i32) -> Result<Target, WheelError> {
    let point = POINT { x: aim.x, y: aim.y };
    let own = unsafe { GetCurrentProcessId() };
    let pass_through = (WS_EX_LAYERED | WS_EX_TRANSPARENT) as isize;
    let mut target: HWND = std::ptr::null_mut();
    let mut window = unsafe { GetTopWindow(std::ptr::null_mut()) };
    while !window.is_null() {
        let mut process = 0;
        unsafe { GetWindowThreadProcessId(window, &mut process) };
        let style = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
        let mut bounds = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if process != own
            && unsafe { IsWindowVisible(window) } != 0
            && unsafe { IsWindowEnabled(window) } != 0
            && style & pass_through != pass_through
            && unsafe { GetWindowRect(window, &mut bounds) } != 0
            && unsafe { PtInRect(&bounds, point) } != 0
        {
            target = window;
            break;
        }
        window = unsafe { GetWindow(window, GW_HWNDNEXT) };
    }
    if target.is_null() {
        return Err(WheelError::NoWindow(aim));
    }
    loop {
        let mut client = point;
        if unsafe { ScreenToClient(target, &mut client) } == 0 {
            break;
        }
        let child = unsafe {
            ChildWindowFromPointEx(
                target,
                client,
                CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT,
            )
        };
        if child.is_null() || child == target {
            break;
        }
        target = child;
    }
    let wparam = (delta as u16 as usize) << 16;
    let lparam = ((aim.y as u16 as u32) << 16 | aim.x as u16 as u32) as isize;
    if unsafe { PostMessageW(target, WM_MOUSEWHEEL, wparam, lparam) } == 0 {
        let error = std::io::Error::last_os_error();
        return Err(WheelError::post_failed(Target::of(target), error));
    }
    Ok(Target::of(target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scroll::estimate::tests::{page, window};

    fn target(hwnd: isize) -> Target {
        Target {
            hwnd,
            class: "Chrome_RenderWidgetHostHWND".into(),
            process: 42,
        }
    }

    /// `docs/archive/30` §24.6.2 measured it: a posted wheel to an elevated window fails with
    /// `ERROR_ACCESS_DENIED`. Only that error means "this window will never take it".
    #[test]
    fn only_access_denied_is_a_refusal() {
        let denied = WheelError::post_failed(
            target(1),
            std::io::Error::from_raw_os_error(ERROR_ACCESS_DENIED as i32),
        );
        assert!(matches!(denied, WheelError::Denied(ref t) if t.hwnd == 1));
        // ERROR_INVALID_WINDOW_HANDLE: the window went away; the next notch looks again.
        let gone = WheelError::post_failed(target(1), std::io::Error::from_raw_os_error(1400));
        assert!(matches!(gone, WheelError::Failed(..)));
    }

    /// A refusal ends auto at once and the toolbar says why; turning auto on again clears it.
    /// Before, a refused notch was logged and counted as a notch that moved nothing, so ten of them
    /// later auto ended as if the page had reached its end.
    #[test]
    fn a_refused_wheel_turns_auto_off_and_says_so() {
        let shared = Shared::default();
        shared.set_auto(true);
        assert_eq!(shared.auto_state(), Auto::On);
        let mut last = None;
        assert!(!shared.after_notch(Err(WheelError::NoWindow(Point::new(0, 0))), &mut last));
        assert_eq!(shared.auto_state(), Auto::On, "no window is not a refusal");
        assert!(shared.after_notch(Err(WheelError::Denied(target(7))), &mut last));
        assert_eq!(shared.auto_state(), Auto::Refused);
        shared.set_auto(true);
        assert_eq!(shared.auto_state(), Auto::On);
        shared.set_auto(false);
        assert_eq!(shared.auto_state(), Auto::Off);
    }

    /// The target is remembered so the log names it when auto starts and whenever it changes.
    #[test]
    fn a_delivered_notch_keeps_auto_on_and_remembers_the_target() {
        let shared = Shared::default();
        shared.set_auto(true);
        let mut last = None;
        assert!(!shared.after_notch(Ok(target(3)), &mut last));
        assert_eq!(last, Some(3));
        assert!(!shared.after_notch(Ok(target(4)), &mut last));
        assert_eq!(last, Some(4));
        assert_eq!(shared.auto_state(), Auto::On);
    }

    /// The log line's target is read from the real window: the desktop's class is `#32769`.
    #[test]
    fn a_target_names_its_class_and_process() {
        let desktop = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow() };
        let found = Target::of(desktop);
        assert_eq!(found.class, "#32769");
        assert_eq!(found.hwnd, desktop as isize);
        assert!(found.to_string().starts_with("hwnd=0x"));
        assert!(found.to_string().contains("class=#32769 pid="));
    }

    /// The field log: once the image was full, every frame of a still-scrolling page was a shift
    /// that placed nothing, and each one logged a line (3800 of them) and woke the overlay.
    #[test]
    fn a_full_image_is_news_once_and_never_moves_the_panel() {
        let shift = Estimate::Shift {
            dy: 40,
            votes: 100,
            second: 0,
        };
        let full = Step {
            estimate: shift,
            placement: Some(Placement::Full),
        };
        assert_eq!(
            outcome(&full, Some(Status::Tracking)),
            Outcome {
                news: true,
                moved: false
            }
        );
        assert_eq!(
            outcome(&full, Some(Status::Full)),
            Outcome {
                news: false,
                moved: false
            }
        );
    }

    /// The rest is unchanged: a placed shift is news and moves the panel, a still frame is neither,
    /// an unknown one is news but moves nothing.
    #[test]
    fn placed_still_and_unknown_frames_keep_their_meaning() {
        let shift = Estimate::Shift {
            dy: 40,
            votes: 100,
            second: 0,
        };
        for placement in [
            Placement::Appended {
                growth: 40,
                band: 400,
            },
            Placement::Prepended {
                growth: 40,
                band: 400,
            },
            Placement::Contained,
        ] {
            let step = Step {
                estimate: shift,
                placement: Some(placement),
            };
            for published in [Some(Status::Tracking), Some(Status::Full)] {
                assert_eq!(
                    outcome(&step, published),
                    Outcome {
                        news: true,
                        moved: true
                    }
                );
            }
        }
        let still = Step {
            estimate: Estimate::Still {
                votes: 100,
                unmatched: 0,
            },
            placement: None,
        };
        assert_eq!(
            outcome(&still, Some(Status::Tracking)),
            Outcome {
                news: false,
                moved: false
            }
        );
        let unknown = Step {
            estimate: Estimate::Unknown {
                best: 0,
                votes: 0,
                second: 0,
                unmatched: 500,
            },
            placement: None,
        };
        assert_eq!(
            outcome(&unknown, Some(Status::Tracking)),
            Outcome {
                news: true,
                moved: false
            }
        );
    }

    fn pending(until: Instant) -> Pending {
        Pending {
            until,
            saw_frame: false,
        }
    }

    /// A notch is judged by what the page did, not by the clock: the moment a frame shows the
    /// scroll, the notch is over, however early that is.
    #[test]
    fn a_notch_ends_the_instant_a_frame_shows_it() {
        let now = Instant::now();
        for age in [Duration::ZERO, Duration::from_millis(1), Duration::from_millis(30)] {
            let mut flight = pending(now + OBSERVE_TIMEOUT);
            assert_eq!(observe(&mut flight, true, now + age), Some(Observation::Moved));
        }
    }

    /// A page with nothing to give stops auto, and the timeout says which of the two nothings it
    /// was: a page that never drew anything at all, or one that kept drawing without moving.
    #[test]
    fn a_notch_that_times_out_reports_what_the_page_did() {
        let now = Instant::now();
        let mut silent = pending(now + OBSERVE_TIMEOUT);
        assert_eq!(observe(&mut silent, false, now), None, "the window is still open");
        assert_eq!(
            observe(&mut silent, false, now + OBSERVE_TIMEOUT),
            Some(Observation::NoMove { saw_frame: false }),
            "the deadline is inclusive: at the deadline the notch is over"
        );
        let mut repainted = Pending {
            until: now + OBSERVE_TIMEOUT,
            saw_frame: true,
        };
        assert_eq!(
            observe(&mut repainted, false, now + OBSERVE_TIMEOUT),
            Some(Observation::NoMove { saw_frame: true })
        );
    }

    /// The field case this replaced: a page that takes longer than a second to draw the notch, or
    /// draws it slowly. Repaints keep buying the notch time, and it is still only counted as idle
    /// after the page has been quiet for the whole window.
    #[test]
    fn a_page_that_keeps_repainting_gets_to_finish_the_scroll_it_started() {
        let mut flight = pending(Instant::now() + OBSERVE_TIMEOUT);
        // A repaint arrives with two thirds of the window spent, and buys a fresh window.
        let late = flight.until - Duration::from_millis(300);
        flight.saw_frame = true;
        flight.until = late + OBSERVE_TIMEOUT;
        assert_eq!(
            observe(&mut flight, false, late),
            None,
            "the page is still drawing; an old deadline must not end the notch"
        );
        assert_eq!(observe(&mut flight, true, late + Duration::from_millis(200)), Some(Observation::Moved));
    }

    /// The override is a measurement tool, so its parser has to refuse the values that would make
    /// the measurement meaningless: zero or negative would spin the wheel every loop iteration, a
    /// non-number is a typo, and anything past five seconds is slower than the page it measures.
    /// A rejected value must fall back to the shipped cadence rather than fail the session.
    #[test]
    fn an_auto_interval_override_is_clamped_and_falls_back_on_nonsense() {
        assert_eq!(parse_ms_override("200"), Some(Duration::from_millis(200)));
        assert_eq!(parse_ms_override(" 60 "), Some(Duration::from_millis(60)));
        assert_eq!(parse_ms_override("20"), Some(Duration::from_millis(20)));
        assert_eq!(parse_ms_override("5000"), Some(Duration::from_millis(5000)));
        for nonsense in ["0", "-1", "19", "5001", "", "fast", "1.5", "200ms"] {
            assert_eq!(parse_ms_override(nonsense), None, "{nonsense:?} must be refused");
        }
    }

    /// The closed loop sets the cadence from the page. The page side is a model; the arithmetic is
    /// the loop's: a notch is sent no sooner than `AUTO_INTERVAL` after the previous one, and it is
    /// judged by the first frame that lands at least `page_latency` after it -- one frame later
    /// than the old clock, which posted every `AUTO_INTERVAL` and read no frame at all.
    #[test]
    fn the_closed_loop_sets_the_cadence_from_the_page() {
        /// One auto session in virtual time: frames arrive every `frame`, and the frame that shows
        /// the notch posted at `notch_at` arrives when `at - notch_at >= page_latency`. Notches
        /// that time out are judged `NoMove`, exactly like the real loop.
        fn notches(frame: Duration, page_latency: Duration) -> Vec<Duration> {
            let mut next_notch = Duration::ZERO;
            let mut pending: Option<(Duration, Duration)> = None;
            let mut frame_moved = false;
            let mut posted = Vec::new();
            let mut next_frame_at = frame;
            for _ in 0..10_000 {
                let at = next_frame_at;
                next_frame_at = at + frame;
                // The loop judges the notch in flight before it looks for the next frame, and the
                // frame it measured is the one that arrived a frame ago.
                if pending
                    .filter(|&(notch_at, deadline)| frame_moved || at >= notch_at + deadline)
                    .is_some()
                {
                    pending = None;
                }
                if pending.is_none() && at >= next_notch {
                    posted.push(at);
                    if posted.len() == 10 {
                        break;
                    }
                    pending = Some((at, OBSERVE_TIMEOUT));
                    next_notch = at + AUTO_INTERVAL;
                }
                frame_moved = pending.is_some_and(|(notch_at, _)| at >= notch_at + page_latency);
            }
            posted
        }

        // A page whose answer lands one frame after the notch. Every notch is judged by that frame,
        // so the cadence is the interval plus one frame -- or one frame per notch, whichever is
        // longer, since auto may not post twice inside the interval either.
        let fast = notches(Duration::from_millis(40), Duration::from_millis(40));
        assert_eq!(
            fast.windows(2).map(|pair| pair[1] - pair[0]).collect::<Vec<_>>(),
            vec![AUTO_INTERVAL; 9],
            "a 40 ms page keeps the interval, and the frame it waits for fits inside it"
        );

        // The clock the loop replaced, on the same page: a notch every `AUTO_INTERVAL`, unchanged.
        assert_eq!(
            (0..10).map(|i| AUTO_INTERVAL * i).collect::<Vec<_>>(),
            (0..10).map(|i| Duration::from_millis(200 * i)).collect::<Vec<_>>()
        );

        // A page that answers only when the window is already over is slower than the interval, so
        // the answer -- not the clock -- is what spaces the notches.
        let late = notches(Duration::from_millis(40), OBSERVE_TIMEOUT);
        assert_eq!(
            late.windows(2).map(|pair| pair[1] - pair[0]).collect::<Vec<_>>(),
            vec![OBSERVE_TIMEOUT; 9],
            "a page that takes a window to answer is read at the window"
        );

        // One frame every five seconds: a page that never answers. Ten windows, then the loop gives
        // up on the run -- the count `idle_after` is handed below.
        let offline = notches(Duration::from_millis(5_000), OBSERVE_TIMEOUT);
        assert_eq!(
            offline.windows(2).map(|pair| pair[1] - pair[0]).collect::<Vec<_>>(),
            vec![Duration::from_millis(5_000); 9],
            "a single frame per five seconds is unanswered, and the timeout is what advances"
        );
        let mut idle_notches = 0;
        for _ in 0..10 {
            idle_after(Observation::NoMove { saw_frame: false }, &mut idle_notches);
        }
        assert_eq!(idle_notches, AUTO_IDLE_NOTCHES, "ten unanswered notches is the give-up run");
    }

    /// Ten notches in a row that moved nothing is the end of auto, and one that moves is a fresh
    /// start -- the run, not the total, is what stops it.
    #[test]
    fn auto_gives_up_only_after_a_run_of_idle_notches() {
        let mut idle_notches = 0;
        for i in 1..AUTO_IDLE_NOTCHES {
            assert_eq!(
                idle_after(Observation::NoMove { saw_frame: true }, &mut idle_notches),
                Idle::Waiting,
                "notch {i} is not yet the end"
            );
        }
        assert_eq!(
            idle_after(Observation::NoMove { saw_frame: false }, &mut idle_notches),
            Idle::GiveUp
        );
        assert_eq!(idle_notches, AUTO_IDLE_NOTCHES, "the log names the run it ended on");
        assert_eq!(idle_after(Observation::Moved, &mut idle_notches), Idle::Moved);
        assert_eq!(idle_notches, 0, "a page that answers is not the page that stops auto");
    }

    /// The overlay hands the thumbnail's buffer back on every wake-up, and the next frame gathers
    /// into that same allocation instead of a fresh one (`fill_preview`'s own numbers: 121 us per
    /// allocation against 17 us into a recycled buffer at 616 KiB). What this pins is that the buffer
    /// actually survives the round trip and still grows: a slot that silently dropped it would put
    /// the cost straight back without failing anything else.
    #[test]
    fn the_preview_buffer_survives_the_round_trip_to_the_overlay() {
        const W: u32 = 40;
        const H: u32 = 80;
        /// The thumbnail is the panel's width, whatever the frame's width is.
        const PANEL: u32 = 200;
        const SHOWN: u32 = 40;
        const WIDER: u32 = 160;
        let worker = ScrollWorker {
            shared: Arc::new(Shared::default()),
            thread: None,
            aim: Point::new(10, 20),
        };
        let page = page(W, 1000, 29);
        let mut tracker = Tracker::new(&window(&page, W, 0, H), W, H, PANEL);
        for top in (40..=600).step_by(40) {
            tracker.push(&window(&page, W, top, H));
        }
        assert!(
            tracker.image_rows() > WIDER * 4,
            "the window has to be able to fill: {} canvas rows",
            tracker.image_rows()
        );

        // First round trip: the overlay takes the report and hands the buffer back emptied.
        let preview = tracker.fill_preview(SHOWN, Vec::new());
        let row = (preview.width * 4) as usize;
        assert_eq!(preview.bytes.len(), row * SHOWN as usize);
        assert_eq!(preview.width, PANEL, "the thumbnail is as wide as the panel");
        {
            let mut report = worker.shared.report.lock().unwrap();
            report.preview = Some(preview);
        }
        let held = worker.take_report().preview.expect("the report carries it").bytes;
        let capacity = held.capacity();
        worker.recycle_preview(held);

        // The next frame wants more rows than the recycled buffer holds, so the pool hands over what
        // it has and the gather grows it. What the recycled path has to prove is that the panel sees
        // exactly what a fresh allocation would have shown: the same bytes, no stale prefix and no
        // short fill. (Whether the allocator reuses the old block in place is its business; asserting
        // a pointer here would assert the arena's state, not this code.)
        let recycled = std::mem::take(&mut *worker.shared.recycled.lock().unwrap());
        assert_eq!(recycled.capacity(), capacity, "emptied, not shrunk");
        assert!(recycled.capacity() < row * WIDER as usize);
        let again = tracker.fill_preview(WIDER, recycled);
        assert_eq!(again.bytes.len(), row * WIDER as usize);
        assert_eq!(
            again,
            tracker.fill_preview(WIDER, Vec::new()),
            "the recycled gather is the fresh gather"
        );
        assert!(
            again
                .bytes
                .chunks_exact(row)
                .all(|line| line.chunks_exact(4).any(|pixel| pixel != [0, 0, 0, 255])),
            "every row of the window is page content, not leftover zeros"
        );
        worker.recycle_preview(again.bytes);
        let spare = std::mem::take(&mut *worker.shared.recycled.lock().unwrap());
        assert_eq!(spare.capacity(), capacity.max(row * WIDER as usize));
        assert!(spare.is_empty(), "the overlay's buffer goes back empty and ready");
    }
}
