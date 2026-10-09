//! Behavioural unit tests: quarantine, cancellation, the real pipeline and the cache.

use super::*;

    #[test]
    fn an_unknown_window_is_quarantined_and_never_invents_geometry() {
        let mut provider = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        let control = QueryControl::refinement(&|| false);
        // A fabricated handle: UIA cannot resolve it.
        let outcome = provider.resolve(
            &job(0xDEAD_BEEF, Point::new(10, 10)),
            Rect::new(0, 0, 100, 100),
            &control,
        );
        assert_eq!(outcome, RefinementOutcome::Empty(StopReason::Unsupported));
        // Second attempt hits the quarantine and returns immediately.
        assert_eq!(
            provider.resolve(
                &job(0xDEAD_BEEF, Point::new(10, 10)),
                Rect::new(0, 0, 100, 100),
                &control
            ),
            RefinementOutcome::Empty(StopReason::Unsupported)
        );
        // A new snapshot generation retries.
        provider.release();
        assert!(provider.quarantined.is_empty());
    }

    #[test]
    fn a_cancelled_query_stops_before_touching_the_tree() {
        let mut provider = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        let control = QueryControl::refinement(&|| true);
        // Cancellation is checked before the first descent, so an already-abandoned query
        // must not walk (and must not publish a deep target).
        match provider.resolve(
            &job(0xDEAD_BEEF, Point::new(10, 10)),
            Rect::new(0, 0, 100, 100),
            &control,
        ) {
            RefinementOutcome::Empty(_) => {}
            RefinementOutcome::Target(target) => {
                assert_eq!(target.stop_reason, StopReason::Cancelled);
            }
        }
    }

    #[test]
    #[ignore = "L3 (docs/31 D-14): creates a real window and walks the UI Automation tree; run with --ignored --test-threads=1 on a live desktop"]
    fn the_real_pipeline_still_answers_in_the_second_capture_session() {
        // End-to-end over the real accessibility stack: scheduler → refinement worker → UIA.
        // Two sessions run back to back on **one** worker, exactly as two F5 presses do. The
        // scheduler resets its id space between them; the worker must adopt the new ids, or the
        // second session's answer is rejected as stale and deep selection dies silently
        // (docs/18 §12.14).
        use crate::window_detection::deep::RefinementScheduler;
        use crate::window_detection::model::WindowIdentity;
        use crate::windows::refinement_worker::RefinementWorker;

        let fixture = FixtureWindow::create()
            .expect("this test needs an interactive window station (docs/31 D-14)");
        let bounds = Rect::new(200, 200, 560, 460);
        let point = Point::new(300, 300);
        let identity = WindowIdentity::new(fixture.handle(), std::process::id(), 0x5E7);
        let metrics = WindowDetectionMetrics::new();
        // Wait until the fixture is registered with UIA *before* the pipeline runs: the point of
        // this test is the id plumbing between sessions, so a cold-start "no tree yet" answer
        // must not be mistaken for it.
        {
            let mut warmup = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
            let outcome = resolve_when_ready(&mut warmup, fixture.handle(), point, bounds);
            assert!(
                matches!(outcome, RefinementOutcome::Target(_)),
                "the fixture must be resolvable before the pipeline is exercised, got {outcome:?}"
            );
        }
        let worker = RefinementWorker::new(
            0,
            win32::HitTestPassThrough::default(),
            crate::window_detection::DEFAULT_ADOPT_TEXT_RUNS,
            metrics,
        );
        let mut scheduler = RefinementScheduler::new();

        for (session, epoch) in [(1u32, 1u64), (2, 2)] {
            let actions = scheduler.on_cursor_moved(epoch, Some(identity), point);
            assert!(actions.arm_dwell);
            let job = scheduler
                .on_dwell_due()
                .unwrap_or_else(|| panic!("session {session} must issue a query"));
            worker.request(job, bounds);

            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut accepted = false;
            while std::time::Instant::now() < deadline {
                // The fixture window lives on *this* thread, so it has to keep pumping while the
                // refinement worker calls into it — exactly what the overlay does while it waits.
                pump(10);
                let Some(result) = worker.take_result() else {
                    continue;
                };
                if result.request != job.request {
                    continue;
                }
                let RefinementOutcome::Target(target) = result.outcome else {
                    panic!("session {session} produced no target: {:?}", result.outcome);
                };
                assert_eq!(target.window, identity);
                assert!(!target.screen_bounds.is_empty());
                accepted = scheduler.on_result(result.request, result.epoch, *target);
                break;
            }
            assert!(
                accepted,
                "session {session}'s answer must still be the current question"
            );
            assert!(scheduler.cached().is_some());

            // Session teardown, then the next F5: both sides start over.
            if session == 1 {
                scheduler.reset();
                worker.retire();
            }
        }
    }

    #[test]
    #[ignore = "L3 (docs/31 D-14): creates a real window and walks the UI Automation tree; run with --ignored --test-threads=1 on a live desktop"]
    fn a_real_window_yields_a_path_that_starts_at_the_window_frame() {
        let fixture = FixtureWindow::create()
            .expect("this test needs an interactive window station (docs/31 D-14)");
        let mut provider = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        assert!(
            provider.automation().is_some(),
            "this test needs UI Automation, which this environment does not expose (docs/31 D-14)"
        );
        let bounds = Rect::new(200, 200, 560, 460);
        let outcome = resolve_when_ready(&mut provider, fixture.handle(), Point::new(280, 280), bounds);
        let RefinementOutcome::Target(target) = outcome else {
            panic!("a live window must produce a target, got {outcome:?}");
        };
        assert_eq!(target.window.hwnd, fixture.handle());
        assert_eq!(
            target.path[0].rect, bounds,
            "the path always starts at the window frame"
        );
        assert_eq!(
            target.path.last().map(|level| level.rect),
            Some(target.screen_bounds)
        );
        assert!(!target.screen_bounds.is_empty());
        // The published rectangle must be inside the window's visible area.
        assert!(!target.screen_bounds.intersect(bounds).is_empty());
    }

    /// An empty expansion must not be remembered (docs/21 §5.6).
    ///
    /// Chromium materialises its accessibility tree lazily: a node answers `raw=0` (or with the
    /// placeholder `Pane(0,0)-(0,0)`) and lists its children a moment later. Caching that answer
    /// pinned the whole snapshot generation to "this node has no children", so every query published
    /// the coarse ancestor — one of the "sometimes stays on the parent box" causes. Reading it again
    /// on the next query is what makes the walk recover on its own.
    #[test]
    #[ignore = "L3 (docs/31 D-14): creates a real window and walks the UI Automation tree; run with --ignored --test-threads=1 on a live desktop"]
    fn an_empty_uia_level_is_never_remembered() {
        let fixture = FixtureWindow::create()
            .expect("this test needs an interactive window station (docs/31 D-14)");
        let mut provider = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        assert!(
            provider.automation().is_some(),
            "this test needs UI Automation, which this environment does not expose (docs/31 D-14)"
        );
        let bounds = Rect::new(200, 200, 560, 460);
        let outcome = resolve_when_ready(&mut provider, fixture.handle(), Point::new(280, 280), bounds);
        assert!(
            matches!(outcome, RefinementOutcome::Target(_)),
            "a live window must still resolve, got {outcome:?}"
        );
        let empty = provider
            .children
            .values()
            .filter(|level| level.children.is_empty() && level.hollow.is_empty())
            .count();
        assert_eq!(
            empty, 0,
            "a level that listed nothing must be re-read, not cached for the generation"
        );
    }

    #[test]
    #[ignore = "L3 (docs/31 D-14): creates a real window and walks the UI Automation tree; run with --ignored --test-threads=1 on a live desktop"]
    fn expanded_levels_are_reused_within_a_generation_and_dropped_across_them() {
        let fixture = FixtureWindow::create()
            .expect("this test needs an interactive window station (docs/31 D-14)");
        let mut provider = UiaDeepSelectionProvider::new(WindowDetectionMetrics::new());
        assert!(
            provider.automation().is_some(),
            "this test needs UI Automation, which this environment does not expose (docs/31 D-14)"
        );
        let bounds = Rect::new(200, 200, 560, 460);
        let control = QueryControl::refinement(&|| false);

        let _ = resolve_when_ready(&mut provider, fixture.handle(), Point::new(280, 280), bounds);
        let expanded = provider.cached_levels();
        assert_eq!(provider.cached_epoch(), Some(1));
        if expanded == 0 {
            // A window whose tree UIA exposes nothing has no level to cache; the reuse rule
            // then simply has nothing to do.
            return;
        }

        // Same generation, another control: the already-walked upper levels are reused, so
        // the table does not grow by re-fetching them.
        let _ = provider.resolve(&job(fixture.handle(), Point::new(300, 300)), bounds, &control);
        assert_eq!(
            provider.cached_epoch(),
            Some(1),
            "the same generation keeps its table"
        );
        assert!(
            provider.cached_levels() >= expanded,
            "a second query may add levels but never loses the cached ones"
        );

        // A rebuilt snapshot invalidates the table before anything else happens.
        let mut next = job(fixture.handle(), Point::new(280, 280));
        next.epoch = 2;
        let _ = provider.resolve(&next, bounds, &control);
        assert_eq!(
            provider.cached_epoch(),
            Some(2),
            "the table is rebuilt for the new generation"
        );
        assert!(
            provider.cached_levels() <= expanded,
            "levels from the previous generation cannot survive"
        );
    }
