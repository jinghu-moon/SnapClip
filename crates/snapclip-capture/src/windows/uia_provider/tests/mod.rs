//! Test infrastructure shared by the uia_provider tests (docs/23 T1.7).
//!
//! Fixture windows, the overlay stand-in and the probes reporting helpers live here,
//! so the probe and unit-test files only hold their own #[test] entries.

    use super::*;
    // Only the source-comparison probe needs these: the raw view and the document's text pattern.
    use ::windows::Win32::UI::Accessibility::{
        IUIAutomationTextPattern, TreeScope_Descendants, UIA_TextPatternId,
    };
    use ::windows::Win32::UI::WindowsAndMessaging::{
        DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, SW_SHOWNA, ShowWindow,
        TranslateMessage, WS_POPUP, WS_VISIBLE, CreateWindowExW, WINDOW_EX_STYLE,
        GWL_EXSTYLE, GetSystemMetrics, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE,
        SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, WS_EX_LAYERED, WS_EX_NOREDIRECTIONBITMAP,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    };
    use ::windows::core::w;
    use std::time::Duration;
    fn job(hwnd: isize, point: Point) -> RefinementJob {
        RefinementJob {
            request: crate::window_detection::model::RequestGate::new().issue(),
            window: crate::window_detection::model::WindowIdentity::new(hwnd, 1, 2),
            epoch: 1,
            point,
        }
    }

    fn pump(millis: u64) {
        let deadline = std::time::Instant::now() + Duration::from_millis(millis);
        let mut message = MSG::default();
        while std::time::Instant::now() < deadline {
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                let _ = unsafe { TranslateMessage(&message) };
                unsafe { DispatchMessageW(&message) };
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Resolve `point` in `hwnd`, waiting out the fixture's registration with the system.
    ///
    /// A window that was just created is registered with the compositor **and** with the UIA
    /// core provider asynchronously, so a query landing in the first milliseconds can be told
    /// the handle is unresolvable. That answer is indistinguishable from "this window has no
    /// tree", and it made these fixture tests flaky (measured once in ~16 suite runs, right
    /// after a cold rebuild) — the fixed 60 ms pump was a weaker version of this same wait.
    ///
    /// Retrying does not weaken any assertion: a genuine regression still fails, after the
    /// deadline, on exactly the assertion it failed on before. The quarantine a failed attempt
    /// leaves behind is released between attempts, because a cold start is not the window's
    /// fault.
    fn resolve_when_ready(
        provider: &mut UiaDeepSelectionProvider,
        hwnd: isize,
        point: Point,
        bounds: Rect,
    ) -> RefinementOutcome {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let control = QueryControl::refinement(&|| false);
            let outcome = provider.resolve(&job(hwnd, point), bounds, &control);
            if !matches!(outcome, RefinementOutcome::Empty(StopReason::Unsupported))
                || std::time::Instant::now() >= deadline
            {
                return outcome;
            }
            provider.release();
            pump(25);
        }
    }

    /// A real top-level window owned by this test process, so UIA has a tree to walk.
    struct FixtureWindow(HWND);

    impl FixtureWindow {
        fn create() -> Option<Self> {
            let window = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("SnapClip uia fixture"),
                    WS_POPUP | WS_VISIBLE,
                    140,
                    140,
                    360,
                    260,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .ok()?;
            let _ = unsafe { ShowWindow(window, SW_SHOWNA) };
            pump(60);
            Some(Self(window))
        }

        fn handle(&self) -> isize {
            self.0 .0 as isize
        }
    }

    impl Drop for FixtureWindow {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
            pump(20);
        }
    }

    /// What the stand-in's owning thread applies to its window.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum OverlayStyle {
        /// Exactly the flags the capture overlay is created with (docs/14 §7).
        Visible,
        /// Plus `WS_EX_TRANSPARENT`: what the product sets for the duration of a hit test
        /// (docs/21 §5.7).
        Transparent,
        /// Plus `WS_EX_LAYERED` as well — the combination that is click-through for real, and
        /// therefore the fallback if the transparent flag alone is not enough.
        LayeredTransparent,
    }

    /// The capture overlay's window shape on its own thread, without a renderer.
    ///
    /// A point hit test has no notion of Z order, so "who answers while our overlay covers the
    /// desktop?" is a question about style, visibility and stacking, not about pixels: a window
    /// with the overlay's own flags models it faithfully. Created and re-styled by its own thread,
    /// as the product does.
    struct OverlayStandIn {
        window: isize,
        /// Shared with the stand-in's window procedure. The probe takes a guard from it exactly as
        /// the refinement worker does in the product, so the lab exercises the real read path.
        pass_through: win32::HitTestPassThrough,
        mode: std::sync::Arc<std::sync::atomic::AtomicU8>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    /// Class name and title of the stand-in's own window class.
    ///
    /// The class matters: a stand-in on the system `STATIC` class was skipped by
    /// `ElementFromPoint` even while it was the topmost window, the foreground window and
    /// `WindowFromPoint`'s own answer — so UIA is not asking the window manager, and a system
    /// class is not the same animal as the overlay's registered class.
    const STAND_IN_CLASS: ::windows::core::PCWSTR = w!("SnapClipOverlayHitTestStandIn");
    const STAND_IN_TITLE: ::windows::core::PCWSTR = w!("SnapClip overlay hit-test stand-in");

    thread_local! {
        /// The stand-in thread's copy of the pass-through flag, exactly as the overlay's window
        /// procedure holds one: a worker thread sets the flag, the owning thread reads it here.
        static STAND_IN_PASS_THROUGH: std::cell::RefCell<win32::HitTestPassThrough> =
            std::cell::RefCell::new(win32::HitTestPassThrough::default());
    }

    /// Replace the stand-in thread's pass-through flag (called on the thread that owns the window).
    fn set_stand_in_pass_through(flag: win32::HitTestPassThrough) {
        STAND_IN_PASS_THROUGH.with(|slot| *slot.borrow_mut() = flag);
    }

    /// Whether the stand-in's window procedure should let the hit test through.
    ///
    /// `try_borrow`, because this runs inside a window procedure: a reentrant `WM_NCHITTEST` must
    /// answer "the overlay owns the point", never panic.
    fn stand_in_pass_through_active() -> bool {
        STAND_IN_PASS_THROUGH
            .with(|slot| slot.try_borrow().map(|flag| flag.is_active()).unwrap_or(false))
    }

    unsafe extern "system" fn stand_in_proc(
        window: ::windows::Win32::Foundation::HWND,
        message: u32,
        wparam: ::windows::Win32::Foundation::WPARAM,
        lparam: ::windows::Win32::Foundation::LPARAM,
    ) -> ::windows::Win32::Foundation::LRESULT {
        use ::windows::Win32::UI::WindowsAndMessaging::{
            DefWindowProcW, HTCLIENT, HTTRANSPARENT, WM_NCHITTEST,
        };
        if message == WM_NCHITTEST {
            return if stand_in_pass_through_active() {
                ::windows::Win32::Foundation::LRESULT(HTTRANSPARENT as isize)
            } else {
                ::windows::Win32::Foundation::LRESULT(HTCLIENT as isize)
            };
        }
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    impl OverlayStandIn {
        /// `None` when the window or its thread could not be created.
        fn create() -> Option<Self> {
            use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
            let mode = std::sync::Arc::new(AtomicU8::new(OverlayStyle::Visible as u8));
            let stop = std::sync::Arc::new(AtomicBool::new(false));
            // Created here, cloned into the stand-in thread: the two sides of the product's flag,
            // one seen by the caller and one read by the window procedure.
            let pass_through = win32::HitTestPassThrough::default();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel::<isize>();
            let thread = {
                let mode = std::sync::Arc::clone(&mode);
                let stop = std::sync::Arc::clone(&stop);
                let thread_pass_through = pass_through.clone();
                std::thread::spawn(move || {
                    set_stand_in_pass_through(thread_pass_through);
                    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(1);
                    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(1);
                    // A registered class, exactly like the overlay's: see STAND_IN_CLASS.
                    let window_class = ::windows::Win32::UI::WindowsAndMessaging::WNDCLASSW {
                        lpfnWndProc: Some(stand_in_proc),
                        lpszClassName: STAND_IN_CLASS,
                        ..Default::default()
                    };
                    unsafe {
                        ::windows::Win32::UI::WindowsAndMessaging::RegisterClassW(&window_class)
                    };
                    let created = unsafe {
                        CreateWindowExW(
                            WINDOW_EX_STYLE(
                                (WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP).0,
                            ),
                            STAND_IN_CLASS,
                            STAND_IN_TITLE,
                            WS_POPUP | WS_VISIBLE,
                            0,
                            0,
                            width,
                            height,
                            None,
                            None,
                            None,
                            None,
                        )
                    };
                    let Ok(window) = created else {
                        let _ = ready_tx.send(0);
                        return;
                    };
                    unsafe {
                        let _ = SetWindowPos(
                            window,
                            Some(HWND_TOPMOST),
                            0,
                            0,
                            width,
                            height,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                    }
                    let _ = ready_tx.send(window.0 as isize);
                    let base = (WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP).0
                        as isize;
                    let mut applied = OverlayStyle::Visible;
                    let mut message = MSG::default();
                    while !stop.load(Ordering::Relaxed) {
                        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                            let _ = unsafe { TranslateMessage(&message) };
                            unsafe { DispatchMessageW(&message) };
                        }
                        let wanted = match mode.load(Ordering::Relaxed) {
                            value if value == OverlayStyle::Transparent as u8 => {
                                OverlayStyle::Transparent
                            }
                            value if value == OverlayStyle::LayeredTransparent as u8 => {
                                OverlayStyle::LayeredTransparent
                            }
                            _ => OverlayStyle::Visible,
                        };
                        if wanted != applied {
                            let style = match wanted {
                                OverlayStyle::Visible => base,
                                OverlayStyle::Transparent => base | WS_EX_TRANSPARENT.0 as isize,
                                OverlayStyle::LayeredTransparent => {
                                    base | WS_EX_TRANSPARENT.0 as isize | WS_EX_LAYERED.0 as isize
                                }
                            };
                            unsafe { SetWindowLongPtrW(window, GWL_EXSTYLE, style) };
                            applied = wanted;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let _ = unsafe { DestroyWindow(window) };
                })
            };
            match ready_rx.recv_timeout(Duration::from_secs(10)) {
                Ok(window) if window != 0 => Some(Self {
                    window,
                    pass_through,
                    mode,
                    stop,
                    thread: Some(thread),
                }),
                _ => {
                    stop.store(true, Ordering::Relaxed);
                    let _ = thread.join();
                    None
                }
            }
        }

        /// Hand the owning thread a new style and let it commit before the next hit test.
        fn set_style(&self, style: OverlayStyle) {
            self.mode
                .store(style as u8, std::sync::atomic::Ordering::Relaxed);
            pump(150);
        }

        fn hwnd(&self) -> isize {
            self.window
        }

        /// Lever 2: cut a few pixels out of the window's *region* around `point`.
        ///
        /// A window region is what defines a shaped window for hit testing, so if anything in the
        /// hit-test chain honours the shape, the point falls through to the page. Screen and window
        /// coordinates coincide here: the stand-in sits at (0,0) and covers the primary screen.
        fn punch_hole(&self, point: Point) {
            use windows_sys::Win32::Graphics::Gdi::{
                CombineRgn, CreateRectRgn, DeleteObject, RGN_DIFF, SetWindowRgn,
            };
            const HOLE: i32 = 2;
            unsafe {
                let full = CreateRectRgn(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
                let hole = CreateRectRgn(
                    point.x - HOLE,
                    point.y - HOLE,
                    point.x + HOLE,
                    point.y + HOLE,
                );
                CombineRgn(full, full, hole, RGN_DIFF);
                DeleteObject(hole);
                SetWindowRgn(self.window as *mut core::ffi::c_void, full, 1);
            }
            pump(250);
        }

        /// Drop the region again (`hwnd, NULL` = "no shape").
        fn heal(&self) {
            use windows_sys::Win32::Graphics::Gdi::SetWindowRgn;
            unsafe {
                SetWindowRgn(self.window as *mut core::ffi::c_void, std::ptr::null_mut(), 1);
            }
            pump(250);
        }
    }

    impl Drop for OverlayStandIn {
        fn drop(&mut self) {
            self.stop
                .store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// One fixture from the demo page's manifest.
    #[derive(Debug, serde::Deserialize)]
    struct Fixture {
        id: String,
        /// Layout input for the absolutely positioned fixtures; generated children are measured
        /// by the page instead.
        #[serde(default)]
        rect: [i32; 4],
        #[serde(default)]
        role: String,
        #[serde(default)]
        name: String,
        /// `"self"`, another fixture id, or `"none"` (see the fixture file's header).
        expect: String,
        /// Offset inside the fixture's own box; defaults to its centre.
        #[serde(default)]
        probe: Option<[i32; 2]>,
        #[serde(default)]
        optional: bool,
        /// The answer must be **strictly inside** this fixture's own box, not the box itself: the
        /// assertion for "the text run under the cursor was adopted" (docs/21 §5.19).
        #[serde(default)]
        finer_than_self: bool,
    }

    /// The fixture manifest, embedded in the demo page as a JSON script block.
    fn manifest_of(source: &str) -> Option<Vec<Fixture>> {
        let start = source.find("\"manifest\">")? + "\"manifest\">".len();
        let end = source[start..].find("</script>")? + start;
        serde_json::from_str(&source[start..end]).ok()
    }

    /// The page's measured boxes, as published in the window title.
    ///
    /// The browser appends its own suffix to the window title (`"… - Google Chrome"`), so the
    /// payload is located by its marker and cut at the matching brace rather than assumed to be
    /// the whole string. Our payload holds no strings, so counting braces is enough.
    fn truth_of(title: &str) -> Option<std::collections::HashMap<String, [i32; 4]>> {
        let json = marker_payload(title, "SNAPCLIP_TRUTH:")?;
        let start = json.find('{')?;
        let mut depth = 0_i32;
        for (index, character) in json[start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return serde_json::from_str(&json[start..=start + index]).ok();
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Everything after `marker` in `title`, or `None` when the marker is absent.
    fn marker_payload<'a>(title: &'a str, marker: &str) -> Option<&'a str> {
        title.find(marker).map(|at| &title[at + marker.len()..])
    }

    /// `Type(x,y)-(x,y) class="…" control=… content=…` for one element.
    fn describe_element(element: &IUIAutomationElement) -> String {
        let kind = unsafe { element.CurrentControlType() }
            .map(|kind| kind.0)
            .unwrap_or(0);
        let rect = to_rect(unsafe { element.CurrentBoundingRectangle() }.unwrap_or_default());
        let class = unsafe { element.CurrentClassName() }
            .map(|name| name.to_string())
            .unwrap_or_default();
        // The accessible name is the property the product's label would show (docs/21 §5.24 B6), so
        // every dump that already describes an element says what the page calls it.
        let name = unsafe { element.CurrentName() }
            .map(|name| name.to_string())
            .unwrap_or_default();
        let control = unsafe { element.CurrentIsControlElement() }
            .map(|value| value.as_bool())
            .unwrap_or(false);
        let content = unsafe { element.CurrentIsContentElement() }
            .map(|value| value.as_bool())
            .unwrap_or(false);
        format!(
            "{}({},{})-({},{}) class={class:?} name={:?} control={control} content={content}",
            control_type_name(kind),
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            name.chars().take(60).collect::<String>()
        )
    }

    /// The integer inside a `VT_I4` variant, if that is what it holds.
    fn variant_i4(variant: &::windows::Win32::System::Variant::VARIANT) -> Option<i32> {
        use ::windows::Win32::System::Variant::VT_I4;
        let vt = unsafe { variant.Anonymous.Anonymous.vt };
        (vt == VT_I4).then(|| unsafe { variant.Anonymous.Anonymous.Anonymous.lVal })
    }

    /// The `vt` field of a variant.
    fn variant_vt(
        variant: &::windows::Win32::System::Variant::VARIANT,
    ) -> ::windows::Win32::System::Variant::VARENUM {
        unsafe { variant.Anonymous.Anonymous.vt }
    }

    /// MSAA's own hit test, the way PixPin asks for it on Chrome (its `UiSpy` carries
    /// `AccessibleObjectFromWindow`, `accLocation` and the literal string `chrome.exe`).
    ///
    /// `AccessibleObjectFromPoint` runs `accHitTest` recursively until an object answers
    /// `CHILDID_SELF`; in Chromium that ends in `BrowserAccessibilityWin::accHitTest` ->
    /// `CachingAsyncHitTest`, i.e. the **renderer's** hit test, where `ElementFromPoint` is answered
    /// by rectangle comparison. If the two disagree, the finer answer is the renderer's.
    fn report_msaa_hit(point: Point) -> String {
        use ::windows::Win32::UI::Accessibility::{AccessibleObjectFromPoint, IAccessible};
        let mut accessible: Option<IAccessible> = None;
        let mut child: ::windows::Win32::System::Variant::VARIANT =
            unsafe { std::mem::zeroed() };
        let called = unsafe {
            AccessibleObjectFromPoint(
                ::windows::Win32::Foundation::POINT {
                    x: point.x,
                    y: point.y,
                },
                &mut accessible,
                &mut child,
            )
        };
        let Some(accessible) = called.ok().and(accessible) else {
            return "failed".into();
        };
        let role = unsafe { accessible.get_accRole(&child) }
            .ok()
            .and_then(|variant| variant_i4(&variant))
            .unwrap_or(-1);
        let name = unsafe { accessible.get_accName(&child) }
            .ok()
            .map(|name| name.to_string())
            .unwrap_or_default();
        let (mut left, mut top, mut width, mut height) = (0, 0, 0, 0);
        let located = unsafe {
            accessible.accLocation(&mut left, &mut top, &mut width, &mut height, &child)
        }
        .is_ok();
        format!(
            "role=0x{role:x} name={:?} {}{width}x{height} at ({left},{top})",
            name.chars().take(24).collect::<String>(),
            if located { "" } else { "NO-RECT " }
        )
    }

    /// The window's **own** accessible object answering `accHitTest`, the way `UiSpy.dll` does it for
    /// Chrome (`AccessibleObjectFromWindow` + `accLocation`, plus a literal `chrome.exe` branch).
    ///
    /// This is the same renderer hit test as `AccessibleObjectFromPoint`, minus the part that decides
    /// *whose* window the point belongs to: no global hit test is taken, so the capture overlay —
    /// which MSAA answers for every point (measured: `HTTRANSPARENT` fixes UIA but not MSAA) — never
    /// enters the picture.
    fn report_msaa_window_hit(hwnd: isize, point: Point) -> String {
        use ::windows::Win32::System::Variant::{
            VARIANT, VARIANT_0, VARIANT_0_0, VT_DISPATCH, VT_I4,
        };
        use ::windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
        use ::windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;
        let started = std::time::Instant::now();
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        let called = unsafe {
            AccessibleObjectFromWindow(
                HWND(hwnd as *mut core::ffi::c_void),
                OBJID_CLIENT.0 as u32,
                &IAccessible::IID,
                &mut raw,
            )
        };
        if called.is_err() || raw.is_null() {
            return "AccessibleObjectFromWindow failed".into();
        }
        let mut current: IAccessible = unsafe { IAccessible::from_raw(raw) };
        let mut depth = 0;
        loop {
            let Ok(hit) = (unsafe { current.accHitTest(point.x, point.y) }) else {
                break;
            };
            let vt = variant_vt(&hit);
            if vt == VT_I4 {
                break; // CHILDID_SELF: this object is the answer.
            }
            if vt != VT_DISPATCH {
                break;
            }
            let dispatch = unsafe { (*hit.Anonymous.Anonymous.Anonymous.pdispVal).clone() };
            let Some(dispatch) = dispatch else { break };
            let Ok(next) = dispatch.cast::<IAccessible>() else {
                break;
            };
            current = next;
            depth += 1;
            if depth > 32 {
                break;
            }
        }
        // CHILDID_SELF: ask this object about itself rather than one of its children.
        let child = VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_I4,
                    ..Default::default()
                }),
            },
        };
        let role = unsafe { current.get_accRole(&child) }
            .ok()
            .and_then(|variant| variant_i4(&variant))
            .unwrap_or(-1);
        let name = unsafe { current.get_accName(&child) }
            .ok()
            .map(|name| name.to_string())
            .unwrap_or_default();
        let (mut left, mut top, mut width, mut height) = (0, 0, 0, 0);
        let located = unsafe {
            current.accLocation(&mut left, &mut top, &mut width, &mut height, &child)
        }
        .is_ok();
        format!(
            "role=0x{role:x} name={:?} depth={depth} {}{width}x{height} at ({left},{top}) {} ms",
            name.chars().take(24).collect::<String>(),
            if located { "" } else { "NO-RECT " },
            started.elapsed().as_millis()
        )
    }

    /// What every candidate source says is under `point` (docs/21 §10).
    ///
    /// Three sources are compared, because the product's ceiling is set by which one it can use:
    ///
    /// * the **control view** — what `ElementFromPoint` and the current walk use. Chromium prunes
    ///   layout-only `<div>`s out of it, which is exactly the box the user is asking for;
    /// * the **raw view** — documented by Chromium as a *superset* of the control view, so an
    ///   "ignored" node (`role=kIgnored`, nameless) may still be there with its geometry;
    /// * the document's **TextPattern** — `RangeFromPoint` answers with the text at that point, which
    ///   can be finer than any node the tree exposes.
    fn report_sources(automation: &IUIAutomation, hwnd: isize, label: &str, point: Point) {
        let screen = ::windows::Win32::Foundation::POINT {
            x: point.x,
            y: point.y,
        };
        println!("[sources] {label} point=({},{})", point.x, point.y);
        let hit = unsafe { automation.ElementFromPoint(screen) }.ok();
        match &hit {
            Some(element) => println!("[sources]   control hit : {}", describe_element(element)),
            None => println!("[sources]   control hit : failed"),
        }
        println!("[sources]   msaa hit    : {}", report_msaa_hit(point));
        println!(
            "[sources]   msaa window : {}",
            report_msaa_window_hit(hwnd, point)
        );

        // The raw view. Read every property straight off the node: batching the request returned
        // empty rectangles for every node (a `FindAllBuildCache` + `TreeScope_Descendants` combination
        // Chromium does not fill in), which made the first version of this measurement useless. The
        // cost printed here is also the reason a production descent has to stay bounded rather than
        // enumerate a page.
        let raw = (|| -> Option<()> {
            let root = unsafe { automation.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void)) }
                .ok()?;
            let condition = unsafe { automation.RawViewCondition() }.ok()?;
            let started = std::time::Instant::now();
            let all = unsafe { root.FindAll(TreeScope_Descendants, &condition) }.ok()?;
            let count = unsafe { all.Length() }.ok().unwrap_or(0).max(0) as usize;
            let mut with_rect = 0_usize;
            let mut containing = 0_usize;
            let mut raw_only = 0_usize;
            let mut smallest: Option<(Rect, i32, String, bool)> = None;
            for index in 0..count {
                let Ok(element) = (unsafe { all.GetElement(index as i32) }) else {
                    continue;
                };
                let rect = to_rect(unsafe { element.CurrentBoundingRectangle() }.unwrap_or_default());
                if rect.is_empty() {
                    continue;
                }
                with_rect += 1;
                let control = unsafe { element.CurrentIsControlElement() }
                    .map(|value| value.as_bool())
                    .unwrap_or(false);
                if !control {
                    raw_only += 1;
                }
                if !rect.contains(point) {
                    continue;
                }
                containing += 1;
                if smallest
                    .as_ref()
                    .map(|(best, ..)| rect.area() < best.area())
                    .unwrap_or(true)
                {
                    let kind = unsafe { element.CurrentControlType() }
                        .map(|kind| kind.0)
                        .unwrap_or(0);
                    let class = unsafe { element.CurrentClassName() }
                        .map(|name| name.to_string())
                        .unwrap_or_default();
                    smallest = Some((rect, kind, class, control));
                }
            }
            println!(
                "[sources]   raw view    : {count} nodes, {with_rect} with a rectangle, {raw_only} \
                 raw-only, {containing} contain the point, {} ms",
                started.elapsed().as_millis()
            );
            if let Some((rect, kind, class, control)) = smallest {
                println!(
                    "[sources]   raw smallest: {}({},{})-({},{}) class={class:?} \
                     control={control}{}",
                    control_type_name(kind),
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom,
                    if control { "" } else { "  <- RAW-ONLY" }
                );
            }
            Some(())
        })();
        if raw.is_none() {
            println!("[sources]   raw view    : unavailable");
        }

        // TextPattern: walk up from the hit until a text provider answers, then ask it what text is
        // under the cursor.
        let mut current = hit.clone();
        for depth in 0..8 {
            let Some(element) = current else { break };
            let pattern = unsafe {
                element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            };
            let range = pattern.ok().and_then(|text| {
                unsafe { text.RangeFromPoint(::windows::Win32::Foundation::POINT {
                    x: point.x,
                    y: point.y,
                }) }
                .ok()
            });
            if let Some(range) = range {
                let snippet = unsafe { range.GetText(60) }
                    .map(|text| text.to_string())
                    .unwrap_or_default();
                let enclosing = unsafe { range.GetEnclosingElement() }.ok();
                let described = enclosing
                    .as_ref()
                    .map(describe_element)
                    .unwrap_or_else(|| "none".into());
                println!(
                    "[sources]   text range  : depth={depth} enclosing={described} \
                     text={:?}",
                    snippet.chars().take(24).collect::<String>()
                );
                return;
            }
            current = unsafe { automation.ControlViewWalker() }
                .ok()
                .and_then(|walker| unsafe { walker.GetParentElement(&element) }.ok());
        }
        println!("[sources]   text range  : no TextPattern on the hit or its ancestors");
    }

    /// `class="…" WxH at (x,y) visible=…` for a window handle, or `none` for "no window".
    fn describe_window(hwnd: isize) -> String {
        if hwnd == 0 {
            return "none".into();
        }
        let class = win32::class_name(hwnd).unwrap_or_default();
        let rect = win32::frame_bounds(hwnd).unwrap_or_default();
        format!(
            "{class:?} {}x{} at ({},{}) visible={}",
            rect.width(),
            rect.height(),
            rect.left,
            rect.top,
            win32::is_window_visible(hwnd)
        )
    }

    /// n / p50 / p95 / max of the product walk's per-query latency, in milliseconds.
    /// The correctness gates say nothing about cost, and the walk grew (backtracking, look-through,
    /// a `ChildWindowFromPointEx` per window-backed candidate), so every probe reports it: the
    /// refinement budget is 1500 ms per query, and a regression here is as real as a wrong box.
    fn print_latency_summary(label: &str, latencies: &mut Vec<f64>) {
        if latencies.is_empty() {
            return;
        }
        latencies.sort_by(f64::total_cmp);
        let pick = |percent: usize| {
            let index = (latencies.len() * percent).div_ceil(100).saturating_sub(1);
            latencies[index.min(latencies.len() - 1)]
        };
        println!(
            "[{label}] latency_ms n={} p50={:.1} p95={:.1} max={:.1}",
            latencies.len(),
            pick(50),
            pick(95),
            latencies[latencies.len() - 1]
        );
    }

    /// Standard install locations of a Chromium-based browser on Windows.
    fn find_chromium() -> Option<String> {
        let roots = [
            std::env::var("ProgramFiles").ok(),
            std::env::var("ProgramFiles(x86)").ok(),
            std::env::var("LOCALAPPDATA").ok(),
        ];
        let relatives = [
            r"Microsoft\Edge\Application\msedge.exe",
            r"Google\Chrome\Application\chrome.exe",
        ];
        roots
            .iter()
            .flatten()
            .flat_map(|root| relatives.iter().map(move |rel| format!("{root}\\{rel}")))
            .find(|path| std::path::Path::new(path).is_file())
    }

    /// The control types that matter when reading the probe output.
    /// The log's spelling of a control type.
    ///
    /// The ids live in one place now (`level_kind_of_control_type`, which the walk uses to publish
    /// what each level *is*); this is that vocabulary's English view, so a log line and the label can
    /// never disagree about what the provider said.
    fn control_type_name(kind: i32) -> &'static str {
        level_kind_of_control_type(kind).debug_name()
    }

    /// The page the cross-origin fixture frame loads (docs/21 §5.20).
    ///
    /// One button at a fixed place with `margin: 0`. The frame measures itself and reports over
    /// `postMessage`, which is both the box its parent cannot read *and* the only proof that its
    /// content ever loaded: without that report a blank frame answers "the frame node" for every
    /// point inside it, which reads exactly like "cross-origin content is unreachable" — the wrong
    /// conclusion this fixture produced before the report existed.
    const CROSS_ORIGIN_FIXTURE: &str = "<!doctype html><body style=\"margin:0\">\
<button id=\"cross-button\" style=\"position:absolute;left:40px;top:40px;width:160px;\
height:48px\">Cross Button</button>\
<script>\
fetch('/cross-ping');\
const box = document.getElementById('cross-button').getBoundingClientRect();\
parent.postMessage({snapclip: 'cross-ready', html: document.body.innerHTML.length,\
 button: [box.left, box.top, box.width, box.height]}, '*');\
</script></body>";

    /// Serve [`CROSS_ORIGIN_FIXTURE`] on a loopback port and keep serving until the process ends.
    ///
    /// The fixture page is loaded over `file://`, so `http://127.0.0.1:<port>` is a real second
    /// origin: the parent cannot reach into the frame through `contentDocument`, which is what makes
    /// this the cross-origin case rather than the same-origin one `iframe-box` covers.
    fn serve_cross_origin_fixture() -> Option<u16> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                // Every request is logged by path: the probe refuses to measure without the child's
                // report, and this says whether "no report" means the frame never asked for the page,
                // the page was served but its script never ran, or the message was lost on the way
                // back (docs/21 §5.20).
                let mut buffer = [0_u8; 1024];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_owned();
                eprintln!("[probe] cross-origin server: {path}");
                if path != "/cross.html" {
                    // The child's own liveness ping: proof its script ran, independent of whether
                    // the message back to the parent arrives.
                    let _ = stream.write_all(
                        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    let _ = stream.flush();
                    continue;
                }
                let body = CROSS_ORIGIN_FIXTURE.as_bytes();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
            }
        });
        Some(port)
    }

mod probes;
mod unit;
