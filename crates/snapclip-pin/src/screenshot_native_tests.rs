//! Own-window native probe. Never captures the desktop, writes the clipboard,
//! moves the system cursor or changes Windows HDR/DPI settings.
use crate::{PinImage, PinManager, PinSelection};
use gpui_kit::{AppContext as _, RenderImageFormat};
use snapclip_model::{
    PixelFormat,
    geometry::Rect,
    pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::Duration,
};
use windows_sys::Win32::{Foundation::RECT, UI::WindowsAndMessaging::*};

#[test]
#[ignore = "Release own-window 20-cycle lifecycle and idle-render probe; no system input"]
fn native_twenty_hdr_windows_release_sources_and_do_not_render_while_idle() {
    if cfg!(debug_assertions) {
        panic!("run this measurement with --release");
    }
    let failures = Rc::new(RefCell::new(Vec::<String>::new()));
    let observed = failures.clone();
    gpui_kit::application().run(move |cx| {
        cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
        gpui_kit::init(cx);
        yue_component::init(cx, yue_component::YueTheme::new(
            yue_component::ThemeMode::Light, yue_component::Accent::Azure,
        )).unwrap();
        crate::init(cx);
        cx.spawn(async move |cx| {
            let mut latencies = Vec::new();
            for cycle in 0..20 {
                let begin = std::time::Instant::now();
                let (owner, window, view, ready, original) = cx.update(|cx| {
                    let descriptor = PixelDescriptor::new(PixelFormat::Rgba16Float, PixelEncoding::ScRgb, AlphaMode::Straight).unwrap();
                    let bytes = [-0.25_f32, 12.5, cycle as f32 / 20., 0.5]
                        .into_iter().flat_map(|v| half::f16::from_f32(v).to_le_bytes())
                        .collect::<Vec<_>>().repeat(64 * 32);
                    let pixels = Arc::new(PixelBuffer::from_tight(64, 32, descriptor, bytes).unwrap());
                    let original = Arc::downgrade(&pixels);
                    let image = PinImage::from_pixels(PixelSource::new(descriptor, cycle), pixels).unwrap();
                    let owner = cx.new(|_| PinManager::new().unwrap().0);
                    let ready = Rc::new(Cell::new(false));
                    let notify = ready.clone();
                    let id = PinManager::open_selection(PinSelection::new(Rect::new(16, 16, 80, 48), image),
                        &owner, cx, move || notify.set(true)).unwrap().id();
                    let (window, view, _) = owner.read(cx).screenshot_for_test(id);
                    (owner, window, view, ready, original)
                });
                for _ in 0..300 {
                    if ready.get() { break; }
                    cx.background_executor().timer(Duration::from_millis(10)).await;
                }
                if !ready.get() {
                    observed.borrow_mut().push(format!("cycle {cycle}: first frame timed out"));
                }
                latencies.push(begin.elapsed().as_secs_f64() * 1000.);
                let resource = window.update(cx, |_, window, cx| {
                    if cycle == 0 { eprintln!("native output snapshot: {:?}; scale={}", window.color_output(), window.scale_factor()); }
                    view.upgrade().and_then(|v|
                        v.read(cx).image.as_ref().map(|image| Arc::downgrade(image.image())))
                }).unwrap();
                if cycle == 19 {
                    // Let creation/receipt/bounds settle, then count the real view's
                    // render calls during an otherwise idle half-second interval.
                    cx.background_executor().timer(Duration::from_millis(200)).await;
                    let before = cx.update(|cx| view.upgrade().unwrap().read(cx).renders);
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    let after = cx.update(|cx| view.upgrade().unwrap().read(cx).renders);
                    eprintln!("idle screenshot view renders over 500ms: {}", after - before);
                    if before != after { observed.borrow_mut().push("idle screenshot kept rendering".into()); }
                }
                cx.update(|cx| owner.update(cx, |m, cx| m.close_all(cx)));
                cx.background_executor().timer(Duration::from_millis(80)).await;
                if resource.is_none_or(|weak| weak.upgrade().is_some()) {
                    observed.borrow_mut().push(format!("cycle {cycle}: display resource missing or retained after close"));
                }
                // Undo intentionally retains the original until its manager dies.
                // A retained undo image is not a GPU-resource leak.
                if original.upgrade().is_none() {
                    observed.borrow_mut().push(format!("cycle {cycle}: undo lost its original source"));
                }
                cx.update(|cx| {
                    for window in cx.windows() { _ = window.update(cx, |_, w, _| w.remove_window()); }
                });
                drop(owner);
                cx.background_executor().timer(Duration::from_millis(80)).await;
                cx.update(|_| ());
                if original.upgrade().is_some() || view.upgrade().is_some() {
                    observed.borrow_mut().push(format!("cycle {cycle}: dropped owner retained source/view"));
                }
            }
            latencies.sort_by(f64::total_cmp);
            eprintln!("64x32 synthetic HDR first-paint acknowledgement, 20 cycles: p50_ms={:.3} p95_ms={:.3}; not a 4K display benchmark", latencies[9], latencies[18]);
            eprintln!("lifecycle failures: {:?}", observed.borrow());
            cx.update(|cx| cx.quit());
        }).detach();
    });
    assert!(failures.borrow().is_empty(), "{:?}", failures.borrow());
}

#[test]
#[ignore = "creates small nonactivating F6 windows; exercises own HWNDs and own GPU targets only"]
fn native_hdr_surface_keeps_placement_alpha_focus_and_releases_its_render_image() {
    let failures = Rc::new(RefCell::new(Vec::<String>::new()));
    let observed = failures.clone();
    let before = unsafe { GetForegroundWindow() } as usize;
    gpui_kit::application().run(move |cx| {
        cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
        gpui_kit::init(cx);
        yue_component::init(
            cx,
            yue_component::YueTheme::new(
                yue_component::ThemeMode::Light,
                yue_component::Accent::Azure,
            ),
        )
        .unwrap();
        crate::init(cx);
        let descriptor = PixelDescriptor::new(
            PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap();
        let bytes = [-0.25_f32, 12.5, 0.5, 0.5]
            .into_iter()
            .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
            .collect::<Vec<_>>()
            .repeat(64 * 32);
        let pixels = Arc::new(PixelBuffer::from_tight(64, 32, descriptor, bytes).unwrap());
        let original = pixels.clone();
        let image = PinImage::from_pixels(PixelSource::new(descriptor, 91), pixels).unwrap();
        let (manager, _) = PinManager::new().unwrap();
        let owner = cx.new(|_| manager);
        let committed = Rc::new(Cell::new(0));
        let commit = committed.clone();
        let id = PinManager::open_selection(
            PinSelection::new(Rect::new(16, 16, 80, 48), image).radius(8),
            &owner,
            cx,
            move || commit.set(commit.get() + 1),
        )
        .unwrap()
        .id();
        if committed.get() != 0 {
            observed
                .borrow_mut()
                .push("source consumed before first frame".into());
        }
        let (window, view, _) = owner.read(cx).screenshot_for_test(id);
        let hwnd = window
            .update(cx, |_, w, _| crate::platform::hwnd(w).unwrap() as usize)
            .unwrap();
        cx.spawn(async move |cx| {
            let check = |condition: bool, message: &str| {
                if !condition {
                    observed.borrow_mut().push(message.into());
                }
            };
            for _ in 0..150 {
                if committed.get() == 1 {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
            }
            check(
                committed.get() == 1,
                "first production HDR frame did not acknowledge source",
            );
            check(
                unsafe { GetForegroundWindow() } as usize == before,
                "F6 creation stole foreground focus",
            );
            let style = unsafe { GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE) } as u32;
            check(
                style & (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST)
                    == (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST),
                "required F6 styles absent",
            );
            check(
                style & (WS_EX_APPWINDOW | WS_EX_LAYERED) == 0,
                "F6 retained app/layered GDI styles",
            );
            let mut native = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            unsafe {
                GetWindowRect(hwnd as _, &mut native);
            }
            check(
                (native.left, native.top) == (16, 16),
                "physical origin changed during creation",
            );
            let resource = window
                .update(cx, |_, w, cx| {
                    let view = view.upgrade().unwrap();
                    let v = view.read(cx);
                    let crate::PinContent::Image(source) = v.snapshot.content() else {
                        panic!("image")
                    };
                    check(
                        Arc::ptr_eq(source.pixels(), &original),
                        "F6 copied/replaced its original source allocation",
                    );
                    let image = v.image.as_ref().expect("prepared image");
                    check(
                        image.image().as_bytes(0).unwrap() == original.as_bytes(),
                        "HDR source bits changed before upload",
                    );
                    let b = super::geometry::border_px((w.scale_factor() * 96.).round() as u32);
                    check(
                        (native.right - native.left, native.bottom - native.top)
                            == (64 + 2 * b, 32 + 2 * b),
                        "physical source dimensions changed at one-to-one zoom",
                    );
                    let target = w.render_to_native_image().expect("own-window GPU readback");
                    let alpha = |x: usize, y: usize| {
                        let index = y * target.size().width.0 as usize + x;
                        if target.format() == RenderImageFormat::Bgra8Srgb {
                            target.as_bytes()[index * 4 + 3] as f32 / 255.
                        } else {
                            half::f16::from_le_bytes(
                                target.as_bytes()[index * 8 + 6..index * 8 + 8]
                                    .try_into()
                                    .unwrap(),
                            )
                            .to_f32()
                        }
                    };
                    check(alpha(0, 0) < 0.01, "rounded outer corner is opaque");
                    check(
                        (alpha(
                            target.size().width.0 as usize / 2,
                            target.size().height.0 as usize / 2,
                        ) - 0.5)
                            .abs()
                            < 0.01,
                        "Root/chrome painted an opaque background under the translucent source",
                    );
                    Arc::downgrade(image.image())
                })
                .unwrap();
            // Native own-window message, not SendInput. GPUI's real wheel path
            // must normalize Windows' line preference back to a tenth per notch.
            let cursor = (native.left + 12, native.top + 12);
            let packed = ((cursor.0 as u16 as u32) | ((cursor.1 as u16 as u32) << 16)) as isize;
            unsafe {
                SendMessageW(hwnd as _, WM_MOUSEWHEEL, (120_usize) << 16, packed);
            }
            cx.background_executor()
                .timer(Duration::from_millis(80))
                .await;
            window
                .update(cx, |_, _, cx| {
                    let v = view.upgrade().unwrap();
                    check(
                        v.read(cx).zoom == 1.1,
                        "native wheel did not advance exactly one tenth",
                    );
                    check(
                        Arc::ptr_eq(
                            v.read(cx).image.as_ref().unwrap().image(),
                            &resource.upgrade().unwrap(),
                        ),
                        "wheel recreated the display resource",
                    );
                })
                .unwrap();
            check(committed.get() == 1, "zoom consumed source a second time");
            owner.update(cx, |m, cx| m.close_all(cx));
            cx.background_executor()
                .timer(Duration::from_millis(80))
                .await;
            check(
                unsafe { IsWindow(hwnd as _) } == 0,
                "closing F6 retained its HWND",
            );
            check(
                resource.upgrade().is_none(),
                "closed F6 retained its display resource",
            );
            check(
                unsafe { GetForegroundWindow() } as usize == before,
                "F6 interaction stole focus",
            );
            cx.update(|cx| {
                // Undo feedback is itself a native window; close every probe
                // window before dropping the native application's dispatcher.
                for window in cx.windows() {
                    _ = window.update(cx, |_, w, _| w.remove_window());
                }
            });
            cx.background_executor()
                .timer(Duration::from_millis(80))
                .await;
            eprintln!("F6 native probe failures: {:?}", observed.borrow());
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    assert!(failures.borrow().is_empty(), "{:?}", failures.borrow());
}
