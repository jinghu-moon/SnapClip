//! The probes: the browser snap-target probe, the Explorer rule probe and the live UIA
//! name dump. They share helpers defined between their test functions (probe_title,
//! RawWalk, ...), which is why they stay in one file instead of one file per probe.

use super::*;

    /// What a correct deep selection must publish in a real web page.
    ///
    /// Run explicitly — it launches its own Chromium on a throwaway profile over the demo page in
    /// `tests/fixtures/browser-element-demo.html`, so the user's browser state is untouched:
    ///
    /// ```text
    /// cargo test --lib browser_element_probe -- --ignored --nocapture
    /// ```
    ///
    /// The page publishes its own measured boxes in the window title (`?truth=1`), so the
    /// expectations cannot drift from the CSS: every row of the report compares our published
    /// rectangle against the browser's own measurement, and against the raw accessibility chain
    /// at that point. `optional` rows are printed but not asserted (rotated boxes, canvas, svg,
    /// iframe, shadow DOM, layout-only wrappers). It is deliberately **not** part of the default
    /// suite: it needs a browser and a visible window.
    #[test]
    #[ignore = "launches a browser; run explicitly with --ignored --nocapture"]
    fn browser_element_probe() {
        let Some(browser) = find_chromium() else {
            eprintln!("skipping: no Chromium-based browser found");
            return;
        };
        // A test process is DPI-unaware by default, so user32 virtualises `GetClientRect` to
        // logical pixels while DWM keeps reporting physical ones — the two disagreed by the
        // display scale and every computed probe point was wrong. The app does exactly this on
        // its overlay thread at startup.
        let _ = crate::windows::monitor::set_per_monitor_v2_awareness();
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/browser-element-demo.html");
        let Ok(source) = std::fs::read_to_string(&fixture_path) else {
            eprintln!("skipping: cannot read {}", fixture_path.display());
            return;
        };
        let Some(manifest) = manifest_of(&source) else {
            eprintln!("skipping: the fixture has no parsable manifest");
            return;
        };

        let dir = std::env::temp_dir().join("snapclip-browser-probe");
        let _ = std::fs::create_dir_all(&dir);
        // A reused profile makes Chromium restore the previous session: it ignores
        // `--window-size`, opens a "restore pages?" bubble, and the page then renders in a window
        // smaller than itself — which showed up as every probe point missing.
        let profile = dir.join("profile");
        let _ = std::fs::remove_dir_all(&profile);
        // The cross-origin frame's URL comes from a loopback server this test runs (docs/21 §5.20).
        let url = match serve_cross_origin_fixture() {
            Some(port) => format!(
                "file:///{}?truth=1&cross=127.0.0.1:{port}",
                fixture_path.display().to_string().replace('\\', "/")
            ),
            None => format!(
                "file:///{}?truth=1",
                fixture_path.display().to_string().replace('\\', "/")
            ),
        };
        let spilled = std::process::Command::new(&browser)
            .args([
                "--new-window",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-session-crashed-bubble",
                "--disable-features=Translate,TranslateUI,InfiniteSessionRestore",
                "--disable-background-networking",
                "--disable-popup-blocking",
                "--force-device-scale-factor=1",
                "--hide-scrollbars",
                // The requested size is the *outer* window, so it has to clear the fixture page
                // plus Chromium's frame and tab strip for the client area to contain 1600x1020.
                "--window-size=1800,1220",
                "--window-position=40,20",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(&url)
            .spawn();
        let Ok(mut child) = spilled else {
            eprintln!("skipping: cannot launch {browser}");
            return;
        };

        // Collect every candidate instead of taking the first match: a Chromium window left over
        // from an earlier probe run also carries the demo page (and its truth title) but keeps the
        // default — smaller — window size, and probing *that* window made every point miss.
        // Find the window first, and measure it *later*: `--window-size` is applied asynchronously,
        // so a client rect read while the window is still settling disagrees with the one the page
        // published its geometry against — which showed up as a viewport that could not be located.
        let deadline = std::time::Instant::now() + Duration::from_secs(40);
        let mut hwnd = None;
        while hwnd.is_none() && std::time::Instant::now() < deadline {
            pump(200);
            hwnd = win32::enumerate_cheap_candidates()
                .unwrap_or_default()
                .into_iter()
                .find(|probe| {
                    if probe.class_name != "Chrome_WidgetWin_1" {
                        return false;
                    }
                    let title = probe_title(probe.hwnd);
                    title.contains("SNAPCLIP_TRUTH:")
                        || title.contains("SNAPCLIP_ERROR:")
                        || title.contains("browser element demo")
                })
                .map(|probe| probe.hwnd);
        }
        let Some(hwnd) = hwnd else {
            let _ = child.kill();
            eprintln!("skipping: the demo page never appeared");
            return;
        };
        // Chromium throttles a background (or covered) tab's accessibility tree, so the probe window
        // is raised above whatever the terminal is showing. `SetForegroundWindow` alone fails here:
        // Windows only lets the foreground process call it, and the test binary is not it.
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                HWND_TOPMOST, SetForegroundWindow, SetWindowPos, SWP_SHOWWINDOW,
            };
            SetWindowPos(
                hwnd as *mut core::ffi::c_void,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_SHOWWINDOW | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
            );
            SetForegroundWindow(hwnd as *mut core::ffi::c_void);
        }
        // The tree materialises asynchronously after the window appears; give Chromium time to
        // answer for every fixture before measuring (see docs/18 §14.3 `AccessibilityPending`).
        pump(1500);

        let title = probe_title(hwnd);
        if let Some(message) = marker_payload(&title, "SNAPCLIP_ERROR:") {
            let _ = child.kill();
            panic!("the fixture page failed to build: {message}");
        }
        let (Some(truth), Some(frame), Some(client)) =
            (truth_of(&title), win32::frame_bounds(hwnd), client_origin_and_size(hwnd))
        else {
            let _ = child.kill();
            eprintln!("skipping: the demo page never reported its geometry");
            return;
        };
        // The cross-origin frame measures itself and reports over `postMessage` — its parent cannot
        // read it. That report is also the only proof the frame's content ever loaded, so the probe
        // refuses to measure without it: a still-blank frame answers the frame node for every point
        // inside it, which is indistinguishable from "cross-origin content is unreachable". That is
        // the wrong conclusion this fixture produced before the report existed (docs/21 §5.20).
        let mut truth = truth;
        if url.contains("cross=") {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !truth.contains_key("cross-ready") && std::time::Instant::now() < deadline {
                pump(250);
                if let Some(refreshed) = truth_of(&probe_title(hwnd)) {
                    truth = refreshed;
                }
            }
            if !truth.contains_key("cross-ready") {
                let _ = child.kill();
                panic!("the cross-origin frame never reported that its content loaded");
            }
        }
        println!(
            "[probe] page published {} boxes; frame/shadow ones: {:?}; cross-ready={:?}",
            truth.len(),
            truth
                .keys()
                .filter(|id| id.contains("iframe") || id.contains("shadow"))
                .collect::<Vec<_>>(),
            truth.get("cross-ready")
        );
        if client.width() < 1600 || client.height() < 1020 {
            let _ = child.kill();
            eprintln!(
                "skipping: the browser client area is {}x{}, smaller than the 1600x1020 page",
                client.width(),
                client.height()
            );
            return;
        }
        println!(
            "[probe] browser={browser} hwnd={hwnd} frame={frame:?} client={client:?}"
        );

        let metrics = WindowDetectionMetrics::new();
        metrics.set_verbose(true);
        let mut provider = UiaDeepSelectionProvider::new(metrics.clone());
        // The assertion loop resolves through the **product's** provider chain, not the UIA provider
        // alone: the MSAA second opinion lives in the composite (docs/21 §5.16), and a gate that
        // skipped it would report the old behaviour no matter what the product does.
        let mut pipeline = crate::windows::refinement_worker::FallbackDeepSelection::new(
            metrics.clone(),
            win32::HitTestPassThrough::default(),
            crate::window_detection::DEFAULT_ADOPT_TEXT_RUNS,
        );
        let walk = provider.automation().cloned().and_then(|auto| RawWalk::new(&auto));
        let hit_owner = provider.automation().cloned();
        // Fixture coordinates are viewport-relative, so the page origin — not the browser's
        // client origin — is what they are relative to. Retried, because the tree can still be
        // filling in while the window has already settled.
        let viewport = (0..50).find_map(|attempt| {
            if attempt > 0 {
                pump(200);
            }
            walk.as_ref()
                .and_then(|walk| walk.viewport_ready(hwnd, client, 8))
        });
        let Some(viewport) = viewport else {
            let _ = child.kill();
            eprintln!(
                "skipping: Chromium did not expose a populated page tree within 10 s — the probe \
                 window is probably occluded or throttled, which says nothing about the walk"
            );
            return;
        };
        println!("[probe] viewport={viewport:?}");

        let mut asserted = 0_usize;
        let mut passed = 0_usize;
        let mut retries = 0_usize;
        // Per-query latency of the **product** walk (backtracking, hollow look-through and the
        // child-window check all run here). The refinement budget is 1500 ms per query.
        let mut latencies: Vec<f64> = Vec::new();
        // Precision yardstick: how often the provider's own point hit test answers with a strictly
        // smaller box that still contains the point — a place our walk stopped above the innermost
        // capturable element.
        let mut provider_available = 0_usize;
        let mut provider_finer = 0_usize;
        // A finer box that was *not* adopted, and hit tests that answered nothing usable: both
        // mean the precision top-up is not doing its job, and both were invisible while the probe
        // only counted.
        let mut finer_not_adopted: Vec<String> = Vec::new();
        let mut provider_unusable: Vec<String> = Vec::new();
        // Published boxes that stick out of the window. A screenshot can only contain what is on
        // screen, so this is a gate for the layout-box shape (a tall node inside a scrollport)
        // whichever side produced the box - the walk or the provider's hit test.
        let mut off_window: Vec<String> = Vec::new();
        // Published paths that are not a containment chain. The ancestor walk (docs/21 §5.17) steps
        // through `path`, so a level that does not contain the next one would make "one level up"
        // jump to a box that does not contain the answer.
        let mut broken_chains: Vec<String> = Vec::new();
        // Rows that must resolve *below* their own box: the text-run adoption (docs/21 §5.19).
        let mut finer_than_self_failures: Vec<String> = Vec::new();
        let mut failures = Vec::new();
        let mut layout_drift = Vec::new();
        for fixture in &manifest {
            let Some(box_) = truth.get(&fixture.id).copied() else {
                println!("[probe] ?? {} not measured by the page", fixture.id);
                continue;
            };
            // The declared layout and the measured box must agree, otherwise the fixture is
            // describing something other than what it renders.
            // A fixture that is not shown measures to an all-zero box: that is the point of the
            // `none` cases, not a layout drift. Optional fixtures are exempt as well: a rotated
            // box is *supposed* to render outside the rectangle it was laid out in.
            if fixture.rect != [0, 0, 0, 0]
                && box_[2] > 0
                && box_[3] > 0
                && !fixture.optional
                && fixture
                    .rect
                    .iter()
                    .zip(box_.iter())
                    .any(|(declared, measured)| (declared - measured).abs() > 1)
            {
                layout_drift.push(format!(
                    "{} declared {:?} but rendered {box_:?}",
                    fixture.id, fixture.rect
                ));
            }
            // A hidden fixture measures to an all-zero box, so its *declared* layout is what says
            // where it would have been — that is the point the walk must not resolve to it.
            let own = if box_[2] > 0 && box_[3] > 0 {
                box_
            } else {
                fixture.rect
            };
            let [dx, dy] = fixture.probe.unwrap_or([own[2] / 2, own[3] / 2]);
            let point = Point::new(viewport.left + own[0] + dx, viewport.top + own[1] + dy);
            // DIAGNOSTIC: measure each fixture from a cold provider, to tell "the walk cannot get
            // there" apart from "a level was cached while Chromium's tree was still empty".
            if std::env::var_os("SNAPCLIP_PROBE_COLD").is_some() {
                provider.release();
                pipeline.release();
            }
            // Bounded retries: Chromium builds its accessibility tree lazily, so the first query
            // for a window can legitimately answer "nothing below the page" and a later one finds
            // the element. The product re-queries on every dwell, so the *eventual* answer is what
            // the user sees; measuring a first try would report a readiness artefact as a defect.
            let mut attempts = 0_usize;
            let mut published = None;
            let mut last_stop = None;
            let mut last_depth = 0_usize;
            let mut last_path: Option<Vec<PathLevel>> = None;
            for attempt in 0..4 {
                if attempt > 0 {
                    pump(250);
                }
                attempts += 1;
                let query_started = std::time::Instant::now();
                let outcome = pipeline
                    .resolve(&job(hwnd, point), frame, &QueryControl::refinement(&|| false));
                latencies.push(query_started.elapsed().as_secs_f64() * 1000.0);
                published = match &outcome {
                    RefinementOutcome::Target(target) => {
                        last_stop = Some(target.stop_reason);
                        last_depth = target.path.len();
                        last_path = Some(target.path.clone());
                        Some(target.screen_bounds)
                    }
                    RefinementOutcome::Empty(reason) => {
                        last_stop = Some(*reason);
                        last_depth = 0;
                        None
                    }
                };
                // An answer that covers the whole page means the walk never got below it: nothing
                // to accept yet. Area, not corners: the page node's frame sits one pixel inside the
                // viewport frame, so a containment test silently disabled every retry.
                let stuck_on_page = published.is_some_and(|rect| {
                    let published_area = i64::from(rect.width()) * i64::from(rect.height());
                    let viewport_area = i64::from(viewport.width()) * i64::from(viewport.height());
                    published_area * 100 >= viewport_area * 95
                });
                if !stuck_on_page {
                    break;
                }
            }
            retries += attempts.saturating_sub(1);
            if let Some(note) = metrics.last_precision() {
                // The composite's own decision for this query, uia/msaa both: the counters say
                // *whether* a finer answer was taken, this says what the transports answered.
                println!("[probe]   decision {}: {note}", fixture.id);
            }
            // The provider's own hit test, through the **production** code path, so the gate below
            // covers the mechanism the product actually runs (`provider_hit` + the adoption rule)
            // rather than a probe-local copy of it.
            match provider.provider_hit(hwnd, point, frame) {
                ProviderHit::Box {
                    bounds: hit_bounds,
                    control_type: hit_kind,
                    ..
                } => {
                    provider_available += 1;
                    let published_area = published
                        .map(|rect| i64::from(rect.width()) * i64::from(rect.height()))
                        .unwrap_or(i64::MAX);
                    let hit_area = i64::from(hit_bounds.width()) * i64::from(hit_bounds.height());
                    if hit_area < published_area {
                        provider_finer += 1;
                        println!(
                            "[probe]   provider is finer on {}: {}x{} at ({},{}) type={hit_kind} \
                             (walk {})",
                            fixture.id,
                            hit_bounds.width(),
                            hit_bounds.height(),
                            hit_bounds.left,
                            hit_bounds.top,
                            published
                                .map(|rect| format!("{}x{}", rect.width(), rect.height()))
                                .unwrap_or_else(|| "none".into())
                        );
                        // With the text-run preference on (docs/21 §5.19) the old exemption is gone:
                        // any strictly finer hit, text run included, has to have become the answer.
                        let exempt = !crate::window_detection::DEFAULT_ADOPT_TEXT_RUNS
                            && crate::window_detection::is_bare_text_control_type(hit_kind);
                        if !exempt {
                            finer_not_adopted.push(fixture.id.clone());
                        }
                    }
                }
                ProviderHit::Unusable(why) => provider_unusable.push(format!("{}: {why}", fixture.id)),
            }
            let expected = match fixture.expect.as_str() {
                "none" => None,
                "self" | "inside_self" | "covers_self" => Some((fixture.id.as_str(), own)),
                // `within:<id>`: the answer must sit inside the referenced fixture's box. Used where
                // the accessibility tree's own granularity is not stable from run to run — the
                // cross-origin frame answers either its own node or the button inside it, and both
                // are correct (docs/21 §5.20).
                other if other.starts_with("within:") => {
                    let target = &other["within:".len()..];
                    truth
                        .get(target)
                        .copied()
                        .map(|measured| (target, measured))
                }
                other => truth
                    .get(other)
                    .copied()
                    .map(|measured| (other, measured)),
            };
            let expected_rect = expected.map(|(_, measured)| {
                Rect::new(
                    viewport.left + measured[0],
                    viewport.top + measured[1],
                    viewport.left + measured[0] + measured[2],
                    viewport.top + measured[1] + measured[3],
                )
            });
            // For a fixture that must not be in the tree at all, the comparison target is the box
            // it would occupy — taken from the declared layout, since the page measures zeros.
            let judged = if fixture.expect == "none" {
                Some(Rect::new(
                    viewport.left + fixture.rect[0],
                    viewport.top + fixture.rect[1],
                    viewport.left + fixture.rect[0] + fixture.rect[2],
                    viewport.top + fixture.rect[1] + fixture.rect[3],
                ))
            } else {
                expected_rect
            };
            let (ok, detail) = judge(fixture.expect.as_str(), judged, published);
            if let Some(rect) = published
                && rect.intersect(frame) != rect
            {
                off_window.push(format!(
                    "{}: {rect:?} is not inside the window {frame:?}",
                    fixture.id
                ));
            }
            if let Some(path) = &last_path {
                // Two pixels of slack: the fixture's own measured box and the accessibility tree's
                // rectangle disagree by one pixel in places, and this gate is about the *shape* of
                // the chain, not about that measurement gap.
                const SLACK: i32 = 2;
                let slack = |outer: Rect, inner: Rect| {
                    inner.left >= outer.left - SLACK
                        && inner.top >= outer.top - SLACK
                        && inner.right <= outer.right + SLACK
                        && inner.bottom <= outer.bottom + SLACK
                };
                if path.is_empty() {
                    broken_chains.push(format!("{}: empty path", fixture.id));
                }
                if let Some(pair) = path
                    .windows(2)
                    .find(|pair| !slack(pair[0].rect, pair[1].rect))
                {
                    broken_chains.push(format!(
                        "{}: {:?} does not contain {:?} (path: {path:?})",
                        fixture.id, pair[0], pair[1]
                    ));
                }
                if path.last().map(|level| level.rect) != published {
                    broken_chains.push(format!(
                        "{}: path ends at {:?}, published {:?}",
                        fixture.id,
                        path.last(),
                        published
                    ));
                }
            }
            if fixture.finer_than_self {
                let own_box = Rect::new(
                    viewport.left + own[0],
                    viewport.top + own[1],
                    viewport.left + own[0] + own[2],
                    viewport.top + own[1] + own[3],
                );
                let ok = published
                    .is_some_and(|rect| own_box.contains_rect(rect) && rect.area() < own_box.area());
                if !ok {
                    finer_than_self_failures.push(format!(
                        "{}: published {:?} is not strictly inside its own box {own_box:?}",
                        fixture.id, published
                    ));
                }
            }
            let label = if fixture.optional {
                "opt"
            } else {
                asserted += 1;
                if ok {
                    passed += 1;
                } else {
                    failures.push(format!(
                        "{} (expect {}, got {})",
                        fixture.id,
                        fixture.expect,
                        published.map(|rect| format!("{rect:?}")).unwrap_or("none".into())
                    ));
                }
                "assert"
            };
            println!(
                "[probe] {label} {:<20} expect={:<16} expected={:<26} published={:<28} {}{} \
                 role={} name={}",
                fixture.id,
                fixture.expect,
                expected_rect
                    .map(|rect| format!(
                        "{}x{} @({},{})",
                        rect.width(),
                        rect.height(),
                        rect.left,
                        rect.top
                    ))
                    .unwrap_or("(not in the tree)".into()),
                published
                    .map(|rect| format!("{}x{} @({},{})", rect.width(), rect.height(), rect.left, rect.top))
                    .unwrap_or("none".into()),
                if ok { "OK  " } else { "MISS" },
                if ok {
                    String::new()
                } else {
                    format!(" ({detail}) ")
                },
                if fixture.role.is_empty() {
                    "-"
                } else {
                    fixture.role.as_str()
                },
                fixture.name
            );
            if !ok && let Some(rect) = published {
                println!(
                    "[probe]      got {rect:?} reason={last_stop:?} depth={last_depth} \
                     attempts={attempts} (viewport {viewport:?})"
                );
                if let Some(walk) = &walk {
                    // The system's own hit test: whichever element it names is the one the user
                    // would be pointing at, so it settles "the walk chose badly" against "the
                    // fixture sits under something else".
                    println!("[probe]      system hit test: {}", walk.hit_test(point));
                }
            }
            if let Some(walk) = &walk
                && !ok
            {
                walk.dump(hwnd, point, 9);
            }
        }

        // --- Does our own overlay hide the page from the provider's hit test? ---
        //
        // The precision top-up (docs/21 §5.7) asks `ElementFromPoint` what is under the cursor,
        // and the product asks that with the capture overlay covering the desktop. This phase
        // builds the same shape over the live fixture — a topmost, full-screen, tool-window overlay
        // owned by another thread — and reports who answers in four states. It is the measurement
        // §6 row 9 made from a branch that was rolled back, which is why the product-side failure it
        // would explain ("the top-up works here and does nothing there") went unseen for a round.
        if let Some(automation) = hit_owner.as_ref() {
            let sample = manifest
                .iter()
                .filter_map(|fixture| {
                    let measured = *truth.get(&fixture.id)?;
                    (measured[2] > 0 && measured[3] > 0).then(|| {
                        let [dx, dy] = fixture.probe.unwrap_or([measured[2] / 2, measured[3] / 2]);
                        (
                            fixture.id.clone(),
                            Point::new(
                                viewport.left + measured[0] + dx,
                                viewport.top + measured[1] + dy,
                            ),
                            Rect::new(
                                viewport.left + measured[0],
                                viewport.top + measured[1],
                                viewport.left + measured[0] + measured[2],
                                viewport.top + measured[1] + measured[3],
                            ),
                        )
                    })
                })
                .next();
            if let Some((id, point, expected)) = sample {
                // Returns whether the hit test answered our own stand-in: that is the state the
                // product is in whenever it asks with the overlay up.
                let answer = |label: &str| -> bool {
                    let hit = unsafe {
                        automation.ElementFromPoint(::windows::Win32::Foundation::POINT {
                            x: point.x,
                            y: point.y,
                        })
                    };
                    let (uia, ours) = match hit {
                        Ok(element) => {
                            let class = unsafe { element.CurrentClassName() }
                                .map(|name| name.to_string())
                                .unwrap_or_default();
                            let kind = unsafe { element.CurrentControlType() }
                                .map(|kind| kind.0)
                                .unwrap_or(0);
                            let bounds = to_rect(
                                unsafe { element.CurrentBoundingRectangle() }.unwrap_or_default(),
                            );
                            (
                                format!(
                                    "{}({},{})-({},{}) type={kind} class={class:?}",
                                    control_type_name(kind),
                                    bounds.left,
                                    bounds.top,
                                    bounds.right,
                                    bounds.bottom
                                ),
                                class == "SnapClipOverlayHitTestStandIn",
                            )
                        }
                        Err(error) => (format!("ElementFromPoint failed: {error}"), false),
                    };
                    // What the *window manager* thinks is at the point, and who owns the
                    // foreground: UIA answers on the page while the overlay is up, and the
                    // difference between the two answers is what says how to fix it.
                    let window_at = unsafe {
                        ::windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(
                            ::windows::Win32::Foundation::POINT {
                                x: point.x,
                                y: point.y,
                            },
                        )
                    };
                    let foreground = unsafe {
                        ::windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow()
                    };
                    println!(
                        "[overlay] {label}: UIA={uia} | WindowFromPoint={:?} | foreground={:?}",
                        describe_window(window_at.0 as isize),
                        describe_window(foreground.0 as isize)
                    );
                    // MSAA is a separate transport with its own hit test, so it needs its own
                    // answer to "does the overlay hide this one too?".
                    println!("[overlay] {label}: MSAA={}", report_msaa_hit(point));
                    ours
                };
                println!(
                    "[overlay] sample fixture={id} point=({},{}) fixture_box={}x{} at ({},{})",
                    point.x,
                    point.y,
                    expected.width(),
                    expected.height(),
                    expected.left,
                    expected.top
                );
                let page_answers = !answer("no overlay");
                assert!(
                    page_answers,
                    "the page must answer before any overlay exists, or this phase measures nothing"
                );
                match OverlayStandIn::create() {
                    Some(overlay) => {
                        println!(
                            "[overlay] stand-in window: {}",
                            describe_window(overlay.hwnd())
                        );
                        pump(300);
                        // The reproduction, and the reason this phase exists: a window carrying the
                        // overlay's shape *and* its own registered class is what UIA answers. A
                        // stand-in on a system class is skipped instead (measured), which is how the
                        // product-only failure stayed invisible for a round.
                        assert!(
                            answer("overlay topmost"),
                            "the stand-in must be what the hit test answers, or this phase no \
                             longer models the product"
                        );
                        overlay.set_style(OverlayStyle::Transparent);
                        let style_helped = !answer("overlay +WS_EX_TRANSPARENT");
                        println!(
                            "[overlay] verdict: WS_EX_TRANSPARENT alone {}",
                            if style_helped {
                                "lets the hit test through"
                            } else {
                                "does NOT let the hit test through"
                            }
                        );
                        overlay.set_style(OverlayStyle::LayeredTransparent);
                        answer("overlay +WS_EX_LAYERED|TRANSPARENT");
                        overlay.set_style(OverlayStyle::Visible);
                        let pass_through = overlay.pass_through.guard();
                        pump(150);
                        // The lever the product pulls (docs/21 §5.7), through the product's own
                        // plumbing: the guard is taken here, the stand-in's window procedure reads
                        // the flag. If Windows ever stops honouring it the precision top-up silently
                        // dies again, so fail here instead.
                        assert!(
                            !answer("overlay +HTTRANSPARENT on WM_NCHITTEST"),
                            "HTTRANSPARENT must let the hit test through to the page, or the \
                             precision top-up does nothing in the product"
                        );
                        drop(pass_through);
                        pump(150);
                        // …and the guard puts it back: the overlay owns the hit test again the
                        // moment the accessibility call it wrapped is over.
                        assert!(
                            answer("overlay after the pass-through guard is dropped"),
                            "the flag must be cleared again, or the overlay stops taking clicks"
                        );
                        overlay.punch_hole(point);
                        answer("overlay + a hole in its region at the point");
                        overlay.heal();
                        answer("overlay visible again");
                    }
                    None => println!("[overlay] the stand-in window could not be created"),
                }
            }
        }

        // Which source could give the user the box they are pointing at? The control view is what
        // the product uses; a layout-only `<div>` is the case that decides whether the raw view or
        // the document's TextPattern is worth more (docs/21 §10).
        if let Some(automation) = hit_owner.as_ref() {
            for id in [
                "plain-div",
                "meter-shell",
                "deep-item",
                "checkbox",
                "radio",
                "para",
                "code-box",
                "table-cell-1",
                "code-run",
                "code-pane",
            ] {
                let Some(measured) = truth.get(id).copied() else {
                    continue;
                };
                if measured[2] <= 0 || measured[3] <= 0 {
                    continue;
                }
                // Manifest entries carry their probe offset; ids that only exist inside a fixture
                // (a `pre` or a text run, for instance) are sampled at their centre.
                let [dx, dy] = manifest
                    .iter()
                    .find(|fixture| fixture.id == id)
                    .and_then(|fixture| fixture.probe)
                    .unwrap_or([measured[2] / 2, measured[3] / 2]);
                let point = Point::new(
                    viewport.left + measured[0] + dx,
                    viewport.top + measured[1] + dy,
                );
                report_sources(automation, hwnd, id, point);
            }
        }

        let _ = child.kill();
        println!(
            "[probe] asserted={asserted} passed={passed} failed={} slow_fixtures={retries}",
            asserted - passed,
        );
        println!(
            "[probe] precision: provider_hit_available={provider_available} \
             provider_hit_is_finer_on={provider_finer}"
        );
        for (label, offenders) in [
            ("NOT ADOPTED (the provider was finer and we published the coarser box)", &finer_not_adopted),
            ("UNUSABLE HIT TEST", &provider_unusable),
        ] {
            for offender in offenders {
                println!("[probe] {label}: {offender}");
            }
        }
        print_latency_summary("probe", &mut latencies);
        for failure in &failures {
            println!("[probe] FAIL {failure}");
        }
        for drift in &layout_drift {
            println!("[probe] LAYOUT {drift}");
        }
        // Optional rows never fail the run, but an asserted row must: the probe is the gate for
        // "web page element capture works", and a silently empty run would be worse than a
        // failing one.
        assert!(asserted > 0, "the fixture must assert something");
        assert!(
            layout_drift.is_empty(),
            "the fixture's declared layout does not match what it renders: {layout_drift:#?}"
        );
        assert!(
            failures.is_empty(),
            "{} of {asserted} asserted fixtures did not resolve to the expected box: {failures:#?}",
            asserted - passed
        );
        // The precision top-up and the window invariant are gates too, not counters: a strictly
        // finer box that we failed to adopt, a hit test that answered nothing usable, or a published
        // box that no screenshot could contain are each a real defect that used to be a printed line.
        assert!(
            finer_not_adopted.is_empty(),
            "{} sampling points published a coarser box although the provider's hit test was \
             strictly finer: {finer_not_adopted:#?}",
            finer_not_adopted.len()
        );
        assert!(
            provider_unusable.is_empty(),
            "{} sampling points could not use the provider's hit test at all: {provider_unusable:#?}",
            provider_unusable.len()
        );
        assert!(
            off_window.is_empty(),
            "{} published boxes extend outside the window, so no screenshot could contain them: \
             {off_window:#?}",
            off_window.len()
        );
        // The ancestor walk steps through `path`, so the chain has to hold everywhere, not only on
        // the rows that were asserted.
        assert!(
            broken_chains.is_empty(),
            "{} published paths are not containment chains ending at the published box: \
             {broken_chains:#?}",
            broken_chains.len()
        );
        assert!(
            finer_than_self_failures.is_empty(),
            "{} rows did not resolve to something strictly inside their own box, so the text run \
             under the cursor was not adopted: {finer_than_self_failures:#?}",
            finer_than_self_failures.len()
        );
    }

    /// Compare what we published against what the page measured.
    fn judge(expect: &str, expected: Option<Rect>, published: Option<Rect>) -> (bool, &'static str) {
        const TOLERANCE: i32 = 3;
        // `within:<id>` (see the expectation lookup): containment instead of equality, because the
        // accessibility tree's granularity inside a cross-origin frame is not stable run to run.
        if expect.starts_with("within:") {
            return match (expected, published) {
                (Some(expected), Some(published)) => {
                    let inside = published.left >= expected.left - TOLERANCE
                        && published.top >= expected.top - TOLERANCE
                        && published.right <= expected.right + TOLERANCE
                        && published.bottom <= expected.bottom + TOLERANCE;
                    (inside, "the answer must sit inside the referenced box")
                }
                (_, None) => (false, "nothing was published"),
                (None, _) => (false, "no expectation to compare"),
            };
        }
        match (expect, expected, published) {
            // `none` means the fixture is not in the tree at all, so the answer must be coarser
            // than its box — anything else proves we captured a hidden element.
            ("none", Some(expected), Some(published)) => {
                let covers = published.left <= expected.left
                    && published.top <= expected.top
                    && published.right >= expected.right
                    && published.bottom >= expected.bottom;
                (covers, "a hidden fixture must not be selected")
            }
            // Some elements are not exposed by the browser's accessibility tree at all, so the
            // honest assertion is the *shape* of the answer rather than its exact box:
            // `inside_self` — the walk went into the element (its text run counts), and
            // `covers_self` — the element is not exposed, so the nearest exposed ancestor must be
            // returned, as long as it is local rather than the whole page.
            ("inside_self", Some(expected), Some(published)) => {
                let inside = published.left >= expected.left - TOLERANCE
                    && published.top >= expected.top - TOLERANCE
                    && published.right <= expected.right + TOLERANCE
                    && published.bottom <= expected.bottom + TOLERANCE;
                (inside, "the answer must sit inside the element")
            }
            ("covers_self", Some(expected), Some(published)) => {
                let covers = published.left <= expected.left
                    && published.top <= expected.top
                    && published.right >= expected.right
                    && published.bottom >= expected.bottom;
                let published_area = i64::from(published.width()) * i64::from(published.height());
                let expected_area = i64::from(expected.width()) * i64::from(expected.height());
                (
                    covers && published_area <= expected_area * 3,
                    "a coarse answer may cover the element, but must stay local",
                )
            }
            (_, None, _) => (false, "no expectation to compare"),
            (_, Some(_), None) => (false, "nothing was published"),
            (_, Some(expected), Some(published)) => {
                let close = (published.left - expected.left).abs() <= TOLERANCE
                    && (published.top - expected.top).abs() <= TOLERANCE
                    && (published.right - expected.right).abs() <= TOLERANCE
                    && (published.bottom - expected.bottom).abs() <= TOLERANCE;
                (close, "published box != measured box")
            }
        }
    }

    /// Every name UIA offers at the cursor (docs/21 §5.24, B6).
    ///
    /// The label's *noun* comes from the control type; the *name* — if one is ever shown — comes from
    /// whichever of four places the page used: `aria-label`, a `title` on an icon-only control, an
    /// `alt`, or the text inside a link/button/heading/cell. A layout-only `<div>` has none, which is
    /// exactly where the label says `容器`.
    ///
    /// Put the cursor on the box you are curious about, then run:
    ///
    /// ```text
    /// cargo test --lib dump_uia_names_under_the_cursor -- --ignored --nocapture
    /// ```
    ///
    /// It prints the window, the ancestor chain with each level's type / name / class / rectangle,
    /// what the label would call the thing under the cursor, and then the source-comparison dump
    /// (control hit, MSAA hit, raw view, text range) for that same point.
    #[test]
    #[ignore = "probe: needs a window under the cursor; prints the names UIA exposes there"]
    fn dump_uia_names_under_the_cursor() {
        use ::windows::Win32::Foundation::POINT;
        use ::windows::Win32::UI::WindowsAndMessaging::{
            GA_ROOT, GetAncestor, GetCursorPos, WindowFromPoint,
        };

        let mut cursor = POINT::default();
        if unsafe { GetCursorPos(&mut cursor) }.is_err() {
            eprintln!("GetCursorPos failed; there is nothing to look at");
            return;
        }
        let at = POINT {
            x: cursor.x,
            y: cursor.y,
        };
        let root = unsafe { GetAncestor(WindowFromPoint(at), GA_ROOT) };
        if root.0.is_null() {
            eprintln!("no top-level window under the cursor");
            return;
        }
        let hwnd = root.0 as isize;
        let point = Point::new(cursor.x, cursor.y);
        let frame = win32::frame_bounds(hwnd).unwrap_or_default();
        println!(
            "[names] cursor=({},{}) window={} frame={}x{} at ({},{})",
            cursor.x,
            cursor.y,
            describe_window(hwnd),
            frame.width(),
            frame.height(),
            frame.left,
            frame.top
        );

        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let automation: Option<IUIAutomation> =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.ok();
        let Some(automation) = automation else {
            eprintln!("UI Automation is unavailable");
            return;
        };

        match unsafe { automation.ElementFromPoint(at) } {
            Ok(element) => {
                // What the label would show for this very box: the noun its control type maps to, and
                // the name the page gave it (empty when nobody gave it one).
                let kind = unsafe { element.CurrentControlType() }
                    .map(|kind| kind.0)
                    .unwrap_or(0);
                let level = level_kind_of_control_type(kind);
                let name = unsafe { element.CurrentName() }
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                println!(
                    "[names] label would say: {} ({}) + name {:?}",
                    level.noun_zh().unwrap_or("容器/元素"),
                    level.debug_name(),
                    name.chars().take(60).collect::<String>()
                );
                // …and the chain it sits in: every level the ancestor walk could publish, innermost
                // first. A level whose `name` is empty is one the label can only call a container.
                let walker = unsafe { automation.ControlViewWalker() }.ok();
                let mut current = Some(element);
                for depth in 0..24 {
                    let Some(node) = current else { break };
                    println!("[names]   {depth:>2} {}", describe_element(&node));
                    current = walker
                        .as_ref()
                        .and_then(|walker| unsafe { walker.GetParentElement(&node) }.ok());
                }
            }
            Err(error) => eprintln!("ElementFromPoint failed: {error}"),
        }
        report_sources(&automation, hwnd, "cursor", point);
    }

    /// Does a rule change make Explorer's answers coarser? Measure, do not eyeball.
    ///
    /// Runs the **product** walk over a grid of points inside a File Explorer window and prints the
    /// published rectangle and depth for each. Two runs of this — one per candidate-order rule —
    /// are what tells "this rule fixes Chromium without costing Explorer", which is exactly the
    /// question docs/18 §14.5 left open when the browser work was rolled back.
    ///
    /// ```text
    /// cargo test --lib explorer_rule_probe -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "opens a File Explorer window; run explicitly with --ignored --nocapture"]
    fn explorer_rule_probe() {
        let _ = crate::windows::monitor::set_per_monitor_v2_awareness();
        // **Deterministic content.** The grid metric depends entirely on what the window shows, so
        // pointing it at the user's own Explorer window made the numbers move on their own
        // (measured for the same code: 9/25, 11/25, 12/25 on different days and folders) — which is
        // exactly the confound that made a rule change look like a regression. The probe now opens
        // its own folder with a fixed set of files and measures *that* window.
        let fixture = std::env::temp_dir().join("snapclip-explorer-fixture");
        let _ = std::fs::create_dir_all(&fixture);
        for index in 0..24 {
            let file = fixture.join(format!("file-{index:02}.txt"));
            if !file.exists() {
                let _ = std::fs::write(&file, format!("snapclip fixture file {index}\n"));
            }
        }
        let fixture_title = fixture
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let _ = std::process::Command::new("explorer.exe")
            .arg(&fixture)
            .spawn();
        let deadline = std::time::Instant::now() + Duration::from_secs(25);
        let mut window = None;
        while window.is_none() && std::time::Instant::now() < deadline {
            pump(250);
            window = win32::enumerate_cheap_candidates()
                .unwrap_or_default()
                .into_iter()
                .find(|probe| {
                    probe.class_name == "CabinetWClass"
                        && probe_title(probe.hwnd).contains(&fixture_title)
                })
                .map(|probe| probe.hwnd);
        }
        let Some(hwnd) = window else {
            eprintln!("skipping: the deterministic Explorer fixture window never appeared");
            return;
        };
        let Some(frame) = win32::frame_bounds(hwnd) else {
            eprintln!("skipping: no frame bounds for the Explorer window");
            return;
        };
        let Some(client) = client_origin_and_size(hwnd) else {
            eprintln!("skipping: no client rect for the Explorer window");
            return;
        };
        println!("[explorer] hwnd={hwnd} frame={frame:?} client={client:?}");
        // Raised so the provider's hit test can see this window at all: a covered window is never
        // answered by `ElementFromPoint`, which would make the precision comparison below vacuous.
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos,
            };
            SetWindowPos(
                hwnd as *mut core::ffi::c_void,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        pump(400);

        let explorer_metrics = WindowDetectionMetrics::new();
        let mut provider = UiaDeepSelectionProvider::new(explorer_metrics.clone());
        // Resolve through the product's provider chain, not the UIA provider alone: the MSAA second
        // opinion (docs/21 §5.16) and the containment invariant (§5.17) both live there.
        let mut pipeline = crate::windows::refinement_worker::FallbackDeepSelection::new(
            explorer_metrics,
            win32::HitTestPassThrough::default(),
            crate::window_detection::DEFAULT_ADOPT_TEXT_RUNS,
        );
        let window_area = i64::from(client.width()) * i64::from(client.height());
        let mut areas = Vec::new();
        let mut latencies: Vec<f64> = Vec::new();
        let mut provider_finer = 0_usize;
        let mut provider_available = 0_usize;
        let mut broken_chains: Vec<String> = Vec::new();
        for fy in [30_i32, 40, 50, 60, 70] {
            for fx in [45_i32, 55, 65, 75, 85] {
                let point = Point::new(
                    client.left + client.width() * fx / 100,
                    client.top + client.height() * fy / 100,
                );
                let query_started = std::time::Instant::now();
                let outcome = pipeline.resolve(
                    &job(hwnd, point),
                    frame,
                    &QueryControl::refinement(&|| false),
                );
                latencies.push(query_started.elapsed().as_secs_f64() * 1000.0);
                let (rect, depth, reason) = match &outcome {
                    RefinementOutcome::Target(target) => (
                        target.screen_bounds,
                        target.path.len(),
                        target.stop_reason,
                    ),
                    RefinementOutcome::Empty(reason) => (Rect::default(), 0, *reason),
                };
                if let RefinementOutcome::Target(target) = &outcome {
                    // The ancestor walk steps through this chain (§5.17): each level has to contain
                    // the next, and the last one has to be the published box.
                    let slack = |outer: Rect, inner: Rect| {
                        inner.left >= outer.left - 2
                            && inner.top >= outer.top - 2
                            && inner.right <= outer.right + 2
                            && inner.bottom <= outer.bottom + 2
                    };
                    if let Some(pair) = target
                        .path
                        .windows(2)
                        .find(|pair| !slack(pair[0].rect, pair[1].rect))
                    {
                        broken_chains.push(format!(
                            "point=({},{}): {:?} does not contain {:?}",
                            point.x, point.y, pair[0], pair[1]
                        ));
                    }
                    if target.path.last().map(|level| level.rect) != Some(target.screen_bounds) {
                        broken_chains.push(format!(
                            "point=({},{}): path ends at {:?}, published {:?}",
                            point.x,
                            point.y,
                            target.path.last(),
                            target.screen_bounds
                        ));
                    }
                }
                let area = i64::from(rect.width()) * i64::from(rect.height());
                areas.push(area);
                // Precision yardstick: what the provider's own hit test answers, through the
                // production path. A hit box that is strictly smaller but still contains the point
                // is a place our walk stopped above the innermost capturable box — and the
                // adoption rule is supposed to remove exactly those.
                let hit = match provider.provider_hit(hwnd, point, frame) {
                    ProviderHit::Box {
                        bounds,
                        control_type,
                        ..
                    } => Some((bounds, control_type)),
                    ProviderHit::Unusable(_) => None,
                };
                if hit.is_some() {
                    provider_available += 1;
                }
                if let Some((hit_bounds, hit_kind)) = hit
                    && i64::from(hit_bounds.width()) * i64::from(hit_bounds.height()) < area
                {
                    provider_finer += 1;
                    println!(
                        "[explorer]   provider is finer: {}x{} at ({},{}) type={hit_kind}",
                        hit_bounds.width(),
                        hit_bounds.height(),
                        hit_bounds.left,
                        hit_bounds.top
                    );
                }
                println!(
                    "[explorer] point=({:>5},{:>5}) depth={:>2} box={}x{} at ({},{}) area_pct={:.1} \
                     reason={reason:?}",
                    point.x,
                    point.y,
                    depth,
                    rect.width(),
                    rect.height(),
                    rect.left,
                    rect.top,
                    (area as f64) * 100.0 / (window_area.max(1) as f64),
                );
            }
        }
        areas.sort_unstable();
        let median = areas[areas.len() / 2];
        let control_level = areas
            .iter()
            .filter(|area| **area * 5 < window_area)
            .count();
        println!(
            "[explorer] summary: median_area_pct={:.1} control_level_points={}/{} \
             (a coarser rule raises the median and lowers the count)",
            (median as f64) * 100.0 / (window_area.max(1) as f64),
            control_level,
            areas.len()
        );
        println!(
            "[explorer] precision: provider_hit_available={provider_available}/{} \
             provider_hit_is_finer_on={provider_finer}",
            areas.len()
        );
        print_latency_summary("explorer", &mut latencies);
        // The same containment invariant the browser gate asserts, on the other window class.
        assert!(
            broken_chains.is_empty(),
            "{} Explorer sampling points published a path that is not a containment chain ending \
             at the published box: {broken_chains:#?}",
            broken_chains.len()
        );
    }

    /// Title of a top-level window, used to identify the probe page.
    fn probe_title(hwnd: isize) -> String {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};
        let length = unsafe { GetWindowTextLengthW(hwnd as *mut core::ffi::c_void) };
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let written = unsafe {
            GetWindowTextW(
                hwnd as *mut core::ffi::c_void,
                buffer.as_mut_ptr(),
                buffer.len() as i32,
            )
        };
        String::from_utf16_lossy(&buffer[..written.max(0) as usize])
    }

    /// The client rectangle of `hwnd` in screen pixels.
    fn client_origin_and_size(hwnd: isize) -> Option<Rect> {
        use windows_sys::Win32::Foundation::{POINT as SYS_POINT, RECT as SYS_RECT};
        use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;
        let mut rect = SYS_RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if unsafe { GetClientRect(hwnd as *mut core::ffi::c_void, &mut rect) } == 0 {
            return None;
        }
        let mut origin = SYS_POINT { x: 0, y: 0 };
        if unsafe { ClientToScreen(hwnd as *mut core::ffi::c_void, &mut origin) } == 0 {
            return None;
        }
        Some(Rect::new(
            origin.x,
            origin.y,
            origin.x + (rect.right - rect.left),
            origin.y + (rect.bottom - rect.top),
        ))
    }

    struct RawWalk {
        request: IUIAutomationCacheRequest,
        automation: IUIAutomation,
    }

    impl RawWalk {
        fn new(automation: &IUIAutomation) -> Option<Self> {
            let request = unsafe { automation.CreateCacheRequest() }.ok()?;
            unsafe {
                request.AddProperty(UIA_BoundingRectanglePropertyId).ok()?;
                request.AddProperty(UIA_ControlTypePropertyId).ok()?;
                request
                    .AddProperty(::windows::Win32::UI::Accessibility::UIA_NamePropertyId)
                    .ok()?;
                request.SetTreeScope(TreeScope_Children).ok()?;
            }
            Some(Self {
                request,
                automation: automation.clone(),
            })
        }

        fn children(
            &self,
            element: &IUIAutomationElement,
        ) -> Vec<(IUIAutomationElement, Rect, i32, String)> {
            let mut out = Vec::new();
            let Ok(cached) = (unsafe { element.BuildUpdatedCache(&self.request) }) else {
                return out;
            };
            let Ok(array) = (unsafe { cached.GetCachedChildren() }) else {
                return out;
            };
            let count = unsafe { array.Length() }.ok().unwrap_or(0).max(0) as usize;
            for index in 0..count {
                let Ok(child) = (unsafe { array.GetElement(index as i32) }) else {
                    continue;
                };
                let Ok(rect) = (unsafe { child.CachedBoundingRectangle() }) else {
                    continue;
                };
                let kind = unsafe { child.CachedControlType() }.map(|kind| kind.0).unwrap_or(0);
                let name = unsafe { child.CachedName() }
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                out.push((child, to_rect(rect), kind, name));
            }
            out
        }

        /// The page's viewport rectangle, in screen pixels.
        ///
        /// Chromium draws its own frame, so the *client* area is the whole window — tabs and
        /// toolbar included — and the page starts lower down. Guessing that offset put every probe
        /// point on Chrome's reload button, so the rectangle is read from the tree instead: walk
        /// the window's **own subtree** for the node that spans the client's full width, touches
        /// its bottom edge, and starts *below* its top edge. Chromium's window and its tab strip
        /// fail the last condition; the web area satisfies all three (measured:
        /// `Pane(48,107)-(1832,1232)` inside a client of `(48,20)-(1832,1232)`).
        ///
        /// Searching the subtree rather than the desktop is deliberate: a point-based hit test
        /// returns whatever window is *on top* at that spot, which made this step depend on the
        /// probe window being foreground — it silently returned another window's element whenever
        /// the terminal was covering it.
        /// The viewport, but only once `want` nodes are exposed *inside* it.
        ///
        /// Chromium builds its accessibility tree lazily and throttles it while the tab is not
        /// active: reading too early returns a placeholder (`Pane(0,0)-(0,0)`, measured) and every
        /// probe point then resolves to the page rectangle — which looked like a product failure
        /// and was only a measurement that started too soon. Requiring real content under the
        /// viewport is what makes the probe's verdict trustworthy.
        fn viewport_ready(&self, hwnd: isize, client: Rect, want: usize) -> Option<Rect> {
            let root = unsafe { self.automation.ElementFromHandle(HWND(hwnd as *mut _)) }.ok()?;
            let mut frontier = std::collections::VecDeque::from([(root, 0_usize)]);
            let mut visited = 0_usize;
            let mut viewport = None;
            let mut inside = 0_usize;
            while let Some((element, depth)) = frontier.pop_front() {
                visited += 1;
                if visited > 3000 || depth > 14 {
                    return None;
                }
                for (child, bounds, _, _) in self.children(&element) {
                    if bounds.is_empty() {
                        continue;
                    }
                    let is_viewport = bounds.left == client.left
                        && bounds.right == client.right
                        && bounds.bottom == client.bottom
                        && bounds.top > client.top;
                    match viewport {
                        None => {
                            if is_viewport {
                                viewport = Some(bounds);
                            }
                            frontier.push_back((child, depth + 1));
                        }
                        Some(found) => {
                            let strictly_inside = bounds.left >= found.left
                                && bounds.top >= found.top
                                && bounds.right <= found.right
                                && bounds.bottom <= found.bottom;
                            if strictly_inside {
                                inside += 1;
                                frontier.push_back((child, depth + 1));
                                if inside >= want {
                                    return viewport;
                                }
                            }
                        }
                    }
                }
            }
            // Only a *populated* subtree is a trustworthy basis for a verdict.
            if inside >= want { viewport } else { None }
        }

        /// Explore every containing branch from the window root, breadth-limited and depth-limited.
        ///
        /// Following only the first containing child hides the interesting case: Chromium's window
        /// root offers two overlapping `Pane` siblings, and the first one is a dead leaf.
        fn dump(&self, hwnd: isize, point: Point, max_levels: usize) {
            let Ok(root) =
                (unsafe { self.automation.ElementFromHandle(HWND(hwnd as *mut _)) })
            else {
                println!("[probe] raw: ElementFromHandle failed");
                return;
            };
            self.explore(&root, point, 0, max_levels, "");
        }

        /// What the system itself says is under `point`.
        fn hit_test(&self, point: Point) -> String {
            let system_point = ::windows::Win32::Foundation::POINT {
                x: point.x,
                y: point.y,
            };
            let Ok(element) = (unsafe { self.automation.ElementFromPoint(system_point) }) else {
                return "n/a".into();
            };
            let name = unsafe { element.CurrentName() }
                .map(|name| name.to_string())
                .unwrap_or_default();
            let kind = unsafe { element.CurrentControlType() }
                .map(|kind| kind.0)
                .unwrap_or(0);
            let bounds = to_rect(unsafe { element.CurrentBoundingRectangle() }.unwrap_or_default());
            format!(
                "{}({},{})-({},{}) {:?}",
                control_type_name(kind),
                bounds.left,
                bounds.top,
                bounds.right,
                bounds.bottom,
                name.chars().take(24).collect::<String>()
            )
        }

        fn explore(
            &self,
            element: &IUIAutomationElement,
            point: Point,
            level: usize,
            max_levels: usize,
            indent: &str,
        ) {
            if level >= max_levels {
                return;
            }
            let children = self.children(element);
            let containing: Vec<_> = children
                .iter()
                .filter(|(_, bounds, _, _)| bounds.contains(point))
                .collect();
            println!(
                "[probe] raw {indent}#{level} children={} containing={}",
                children.len(),
                containing.len()
            );
            // When nothing contains the point any more, print what *is* there: that is where the
            // page's own boxes live, and whether they carry real rectangles decides whether deep
            // selection can reach them at all.
            if containing.is_empty() {
                for (_, bounds, kind, name) in children.iter().take(6) {
                    println!(
                        "[probe] raw {indent}  ( ) {}({},{})-({},{}) {:?}",
                        control_type_name(*kind),
                        bounds.left,
                        bounds.top,
                        bounds.right,
                        bounds.bottom,
                        name.chars().take(28).collect::<String>()
                    );
                }
                return;
            }
            // At most two branches per level: enough to show the dead leaf *and* the content
            // branch, without letting a big page walk itself to death.
            let branch = containing.len() <= 2;
            for (child, bounds, kind, name) in containing {
                println!(
                    "[probe] raw {indent}  -> {}({},{})-({},{}) {:?}",
                    control_type_name(*kind),
                    bounds.left,
                    bounds.top,
                    bounds.right,
                    bounds.bottom,
                    name.chars().take(28).collect::<String>()
                );
                if branch {
                    self.explore(child, point, level + 1, max_levels, &format!("{indent}    "));
                }
            }
        }
    }

