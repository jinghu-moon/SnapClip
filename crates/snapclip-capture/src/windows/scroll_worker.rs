//! The scroll capture thread (`docs/33`): monitor frames in, the long image and its thumbnail out.
//!
//! One thread owns the WGC monitor session and the [`Tracker`]. Every frame it takes is the newest
//! one delivered; only the selection is read back, measured against the reference and placed. The
//! overlay is woken with [`SCROLL_READY_MESSAGE`] and pulls a [`Report`]. The overlay never sees
//! itself in these frames (`WDA_EXCLUDEFROMCAPTURE`), which is why the monitor can be captured while
//! the panel sits on top of it.
//!
//! Auto mode is one wheel notch every [`AUTO_INTERVAL`], posted to the window under the selection's
//! centre ([`post_wheel`]); it turns itself off after [`AUTO_IDLE_NOTCHES`] notches in a row moved
//! nothing, which is what "the page has reached its end" looks like from outside the page. A window
//! that refuses the posted wheel (UIPI: it runs elevated) turns auto off at once, and the toolbar
//! says so, rather than ten silent failures reading as "the end of the page".

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
use crate::scroll::{Auto, Preview, Status, Step, Tracker};

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
    pub(crate) fn finish(mut self) -> Option<(u32, u32, Vec<u8>)> {
        self.join().map(Tracker::into_image)
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
    let mut last_target = None;
    let mut frames = 0u64;
    let mut quiet = 0u64;
    // Per-frame cost split, accumulated for the closing line: a real session must be able to say
    // where its time went without a rebuild and without flooding stderr.
    let mut read_total_us = 0u64;
    let mut push_total_us = 0u64;
    // `us` is `read_region` + `push`; keeping every sample lets the closing line report percentiles
    // and the part of `us` that is neither, which is the difference a mean alone hides.
    let mut total_us: Vec<u64> = Vec::new();
    let mut read_us_all: Vec<u64> = Vec::new();
    let mut published: Option<Status> = None;
    while !shared.stop.load(Ordering::Acquire) {
        if shared.auto.load(Ordering::Acquire) {
            let now = Instant::now();
            if idle_notches >= AUTO_IDLE_NOTCHES {
                eprintln!("[snapclip][scroll] auto ended: {idle_notches} notches moved nothing");
                shared.auto.store(false, Ordering::Release);
                wake(spec, shared);
            } else if now >= next_notch {
                if shared.after_notch(post_wheel(spec.aim, NOTCH_DOWN), &mut last_target) {
                    wake(spec, shared);
                }
                idle_notches += 1;
                next_notch = now + AUTO_INTERVAL;
            }
        } else {
            idle_notches = 0;
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
            let us = started.elapsed().as_micros();
            read_total_us += read_us as u64;
            push_total_us += push_us as u64;
            total_us.push(us as u64);
            read_us_all.push(read_us as u64);
            if verbose_timing() {
                eprintln!(
                    "[snapclip][scroll] frame={frames} read_us={read_us} push_us={push_us} \
                     total_us={us} bytes={}",
                    pixels.len()
                );
            }
            if matches!(step.estimate, Estimate::Shift { .. }) {
                idle_notches = 0;
            }
            let Outcome { news, moved } = outcome(&step, published);
            if news {
                eprintln!(
                    "[snapclip][scroll] frame={frames} {:?} placement={:?} rows={} status={:?} \
                     quiet_since_last={quiet} us={us}",
                    step.estimate,
                    step.placement,
                    tracker.image_rows(),
                    tracker.status(),
                );
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
        {
            let mut report = shared.report.lock().unwrap();
            report.preview = Some(tracker.preview(spec.preview_rows));
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
}
