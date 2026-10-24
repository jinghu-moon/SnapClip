//! SnapClip's GPUI shell (docs/23 P4).
//!
//! The shell is a **library** with a thin binary entry point (`main.rs`): integration
//! tests can only import a library, and the Coding Guides put the capability behind a
//! library boundary while the binary only composes it.
//!
//! Rules this crate follows, because the Coding Guides make them requirements:
//! `gpui_kit::init` runs first inside `run`, before any component-backed view exists;
//! each window's first level is `Root`; the UI comes from `gpui_kit` alone.

// Both traits are re-exported by `gpui-kit` itself (paths verified in gpui-base-0.7.1's
// lib.rs and gpui-component-0.7.1's lib.rs), which is what the one-dependency rule wants:
// `StyledExt` carries `v_flex`/`h_flex`, `ActiveTheme` carries `cx.theme()`.
use gpui_kit::base::StyledExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, Root};
use gpui_kit::*;
use std::sync::Arc;
use std::time::Duration;

pub mod adapters;
pub mod capture;
pub mod clipboard;
pub mod clipboard_ingest;
pub mod events;
pub mod history;
pub mod pin;
pub mod save;
pub mod settings;
pub mod startup;
pub mod tray;

use history::icons::SourceIcons;
use history::model::HistoryState;
use history::view::HistoryView;

use events::EventBus;
use startup::{subsystems, StartupOptions};

/// The same database and cache directories the Tauri host writes while both shells exist
/// (docs/23 T4.9: they share only the capability crates and the data).
pub fn app_data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("com.seeyuer.snapclip")
}

/// The shell window's size in logical pixels.
///
/// The old Tauri window was 320×600; the product moved to 360×640 when the shell became
/// native, because a narrower window forced the history rows to spend too much of their width
/// on chrome. The size is a product decision, not a rendering default, so it lives here.
const WINDOW_WIDTH: f32 = 360.0;
const WINDOW_HEIGHT: f32 = 640.0;
const WINDOW_TITLE: &str = "SnapClip";

/// One navigation entry: a button plus the rule that marks it active.
fn nav_item(active: bool, accent: Hsla, border: Hsla, content: AnyElement) -> AnyElement {
    div()
        .pb_1()
        .border_b_2()
        .border_color(if active { accent } else { border })
        .child(content)
        .into_any_element()
}

/// Drain the tray's commands on the UI thread.
///
/// The tray thread never touches a window; this task is the only place a [`tray::TrayCommand`]
/// becomes a window action. `spawn_in` is what makes that possible: the task can reach the
/// window it was created from.
fn spawn_tray_commands(tray: &tray::Tray, window: &mut Window, cx: &mut Context<Shell>) {
    let commands = tray.commands();
    cx.spawn_in(window, async move |shell, cx| {
        while let Some(command) = commands.recv().await.ok() {
            let acted = shell.update_in(cx, |_shell, window, cx| match command {
                // Windows has no "hide a window" in GPUI's portable surface, and minimising is
                // what a tray user means by "put it away": the taskbar button goes with it.
                tray::TrayCommand::ToggleWindow => {
                    if window.is_window_active() {
                        window.minimize_window();
                    } else {
                        window.activate_window();
                    }
                    cx.notify();
                }
                tray::TrayCommand::Quit => cx.quit(),
            });
            if acted.is_err() {
                // The shell is gone; there is nothing left to command.
                break;
            }
        }
    })
    .detach();
}

/// Drain what the pin host reports back.
///
/// The pin host owns every pin window and handles F6 on its own thread, so the shell never
/// sees a pin as a command. The only thing the host has to say is "a pin I was asked for did
/// not appear", and it has to be said even though nothing user-visible depends on it yet: the
/// channel is bounded, so an undrained one would eventually report "queue full" for a
/// perfectly healthy system.
///
/// A 50 ms poll is deliberate. This loop costs one timer wake-up every 50 ms on the
/// background executor, and the alternative (a Windows message to the shell's window) would
/// add a second way to reach this state for no behavioural gain.
fn spawn_pin_events(pins: &pin::PinManager, window: &mut Window, cx: &mut Context<Shell>) {
    let pins = pins.clone();
    cx.spawn_in(window, async move |shell, cx| {
        loop {
            let report = pins.drain_events();
            if report.any() {
                // 说人话：创建、关闭、没能创建是三件事。只有第三件需要担心
                //（"F6 没反应"的那种失败），前两件是正常生命周期（docs/23 §14.55.9.6）。
                let mut parts = Vec::new();
                if report.created > 0 {
                    parts.push(format!("创建 {} 张", report.created));
                }
                if report.closed > 0 {
                    parts.push(format!("已关闭 {} 张", report.closed));
                }
                if report.failed > 0 {
                    parts.push(format!("{} 张没能创建", report.failed));
                }
                eprintln!("[snapclip-app] pin: {}", parts.join("、"));
                let acted = shell.update_in(cx, |_shell, _window, cx| cx.notify());
                if acted.is_err() {
                    // The shell is gone; nothing left to report to.
                    return;
                }
            }
            // `BackgroundExecutor::timer` is the sleep this crate has access to: GPUI's
            // `Timer` is not re-exported through `gpui_kit`, and pulling `gpui` in as a
            // second direct dependency for one sleep is not worth it.
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
        }
    })
    .detach();
}

/// The shell's root view: the history screen, or the reason it could not open.
///
/// A shell that panics because a database file is busy is worse than one that says so, and
/// the Tauri host may be holding the same file until P6 removes it.
pub struct Shell {
    history: Result<Entity<HistoryView>, String>,
    settings: Entity<settings::SettingsView>,
    page: Page,
    /// The shell's event channel. It is created here, at the composition root, and handed to
    /// whatever publishes (the capability adapters) and whatever listens (the screens).
    events: EventBus,
    /// The notification-area icon. `None` when Windows refused it, which is a degraded shell
    /// (no tray) rather than a failed start — the window is still there.
    tray: Option<tray::Tray>,
    /// The hosted capture overlay. `None` when the overlay or its hotkey could not start,
    /// which is reported and survived: the rest of the shell still works.
    capture: Option<Arc<snapclip_capture::CaptureRuntime>>,
    /// The clipboard pipeline. Holding this is what keeps it running; dropping it stops the
    /// listener and joins the worker, so it must live as long as the shell.
    clipboard: Option<snapclip_history::ingest::ClipboardIngestHandle>,
    /// The pin host: one thread that owns every pinned-image window. `None` when that
    /// thread could not be started, which is reported and survived — F5 capture does not
    /// depend on it.
    pins: Option<pin::PinManager>,
    /// Drains the capture events the overlay publishes, so a display setting changed on the size
    /// badge reaches the settings file (docs/23 §14.53). Held because dropping the task stops it.
    _label_sync: Task<()>,
}

/// Which capability the window is showing. A desktop shell keeps navigation persistent, so
/// the switcher lives in the window, not in a menu (Design Guides, "Layout patterns").
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    History,
    Settings,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let data = app_data_dir();
        let events = EventBus::new();
        // One store for the whole shell: the clipboard pipeline and the history screen then
        // share a writer thread instead of racing two of them on the same SQLite file.
        let (history, store) = match snapclip_history::store::Store::open(&data) {
            Ok(store) => match HistoryState::with_store(store.clone()) {
                Ok(state) => match SourceIcons::new(data.join("icons")) {
                    Ok(icons) => (
                        Ok(cx.new(|cx| HistoryView::new(state, icons, events.clone(), window, cx))),
                        Some(store),
                    ),
                    Err(error) => (Err(format!("icons: {error}")), Some(store)),
                },
                Err(error) => (Err(format!("history store: {error}")), Some(store)),
            },
            Err(error) => (Err(format!("history store: {error}")), None),
        };
        // The settings page is independent of the store, so it exists even when history
        // cannot open — that is also how the user can see *why* something is wrong.
        let settings_store = settings::SettingsStore::new(&data);
        // Read once for the capture start below; the settings page keeps its own copy so a
        // later edit is written to the same file.
        let initial = settings_store.load();
        let settings = cx.new(|_| settings::SettingsView::new(settings::SettingsStore::new(&data)));
        // A saved change reaches the overlay from its next session on.
        cx.subscribe(&settings, |this: &mut Self, _, event: &settings::SettingsChanged, _| {
            if let Some(capture) = &this.capture {
                capture.set_preferences(event.0.overlay_preferences());
            }
        })
        .detach();
        // Capture is hosted here from P6 on: F5, the overlay and the export chain belong to
        // this process, which is what lets the old Tauri shell be deleted.
        //
        // `pin_source` is the other half of F6: capture remembers the selection it just wrote
        // (pixels plus their place on the desktop), and the pin host takes it from there.
        // Capture does not know pins exist; the shell is what joins the two.
        let (capture, pin_source, capture_events) = match capture::start(
            &data,
            events.clone(),
            initial.detection_options(),
            initial.overlay_preferences(),
        ) {
            Ok((runtime, pin_source, capture_events)) => {
                (Some(runtime), Some(pin_source), Some(capture_events))
            }
            Err(error) => {
                eprintln!("[snapclip-app] capture unavailable: {error}");
                (None, None, None)
            }
        };
        // The overlay's size badge carries its own menu for the display settings this page shows
        // (docs/23 §14.53). The overlay edits its session's copy and publishes it where the next
        // session reads it; this drain is what notices and writes it down, so a change made on the
        // badge survives a restart instead of lasting only until the app is closed. It reads
        // through the settings page rather than the file, so the page and the file cannot disagree.
        let mut stream = events.subscribe();
        let _label_sync = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            while let Some(event) = stream.next().await {
                if !matches!(event, snapclip_model::events::AppEvent::Capture(_)) {
                    continue;
                }
                // The change happens inside `Entity::update`, which is the rule that keeps
                // rendering single-threaded — the same shape the history screen uses.
                if this
                    .update(cx, |shell, cx| shell.adopt_overlay_label(cx))
                    .is_err()
                {
                    // The shell is gone; stop draining rather than spinning on a closed bus.
                    break;
                }
            }
        });
        // The clipboard pipeline publishes `AppEvent::Clipboard` on the bus above, which is
        // what makes the history screen refresh by itself while it is open.
        let clipboard = match store.clone() {
            Some(store) => match clipboard_ingest::start(store, events.clone()) {
                Ok(handle) => Some(handle),
                Err(error) => {
                    eprintln!("[snapclip-app] clipboard pipeline unavailable: {error}");
                    None
                }
            },
            None => None,
        };
        // The shell's windows are reachable from here on: the tray is the entry point that
        // T4.1.1 recorded as missing, and its commands arrive on their own thread.
        let tray = match tray::Tray::start() {
            Ok(tray) => {
                spawn_tray_commands(&tray, window, cx);
                Some(tray)
            }
            Err(error) => {
                eprintln!("[snapclip-app] tray unavailable: {error}");
                None
            }
        };
        // Pinned images outlive any screen: F6 is handled entirely on the pin host's own
        // thread, so the shell only owns the host and drains the events it reports back.
        //
        // Without a capture there is no source, so pinning is not started at all: a host that
        // could never be given a picture is a thread and a hotkey that do nothing.
        let pins = match pin_source {
            Some(pin_source) => match pin::PinManager::start(pin_source) {
                Ok(pins) => {
                    spawn_pin_events(&pins, window, cx);
                    Some(pins)
                }
                Err(error) => {
                    eprintln!("[snapclip-app] pin unavailable: {error}");
                    None
                }
            },
            None => None,
        };
        // 延时截图到点要贴图：把那件事接到与 `F6` **完全相同**的一条路上——往贴图宿主
        // 线程投一条 `WM_HOTKEY`。捕获侧只说"请贴图"，怎么贴由这里决定（docs/23 §14.55.8）。
        if let Some(events) = capture_events.as_ref() {
            if let Some(pins) = pins.as_ref() {
                let poster = pins.pin_poster();
                events.set_pin_poster(std::sync::Arc::new(move || {
                    poster.post().map_err(|error| error.to_string())
                }));
            }
            // 用户按 `Ctrl+S` 之后：捕获侧只说"要开保存面板，并把这张选区图给你"，怎么开由这里
            // 决定。句柄存在闭包捕获的槽里；替换走 `replace_panel`，它**先关旧的再存新的**——
            // 顺序是契约：窗口句柄在窗口销毁后会被系统复用，先开新再关旧可能把 `WM_CLOSE`
            // 打到刚开的那张上（那个 bug 的真机症状是"日志说已打开、人却看不见"）。
            let save_panels: std::sync::Arc<
                std::sync::Mutex<Option<save::window::ShellHandle>>,
            > = std::sync::Arc::new(std::sync::Mutex::new(None));
            // One record for every panel this shell opens: the spec remembers the name template and
            // the number across saves (`:201`), and a panel is opened per capture, so neither can
            // live inside a panel.
            let save_remembered = std::sync::Arc::new(std::sync::Mutex::new(save::Remembered::default()));
            events.set_save_opener(std::sync::Arc::new(move |pixels| {
                let width = (pixels.region.right - pixels.region.left) as u32;
                let height = (pixels.region.bottom - pixels.region.top) as u32;
                let options = save::window::ShellOptions {
                    preview: Some((pixels.bgra.clone(), width, height)),
                    capture_size: Some((width, height)),
                    remembered: save_remembered.clone(),
                    ..Default::default()
                };
                // 先关旧、再建新、最后存 —— 顺序是契约，`replace_panel` 的文档与两条测试
                // 写明了理由（句柄会被系统复用）。
                save::window::replace_panel(
                    &save_panels,
                    || save::window::open(options).map_err(|error| format!("{error:?}")),
                    |previous| previous.close(),
                )?;
                Ok(())
            }));
        }
        Self {
            history,
            settings,
            page: Page::History,
            events,
            tray,
            capture,
            clipboard,
            pins,
            _label_sync,
        }
    }

    /// Take the display settings the overlay is holding, if the badge's menu changed them
    /// (docs/23 §14.53).
    ///
    /// Called on every capture event, which is every moment the overlay could have moved them; the
    /// write itself happens only when a value actually differs, so a capture the user never touched
    /// costs one comparison. The settings page is the writer, so the change is saved, the page is
    /// updated, and the page's own `SettingsChanged` subscriber hands it back to capture for the
    /// next session — one path for both places the setting can be changed from.
    fn adopt_overlay_label(&mut self, cx: &mut Context<Self>) {
        let Some(preferences) = self.capture.as_ref().and_then(|capture| capture.preferences())
        else {
            return;
        };
        let _ = self
            .settings
            .update(cx, |view, cx| view.adopt_label(preferences.label, cx));
    }

    /// The shell's event channel. The composition root is the only place that creates it,
    /// and everything that publishes or listens on it gets it from here.
    pub fn events(&self) -> EventBus {
        self.events.clone()
    }

    /// The notification-area icon, when Windows accepted one.
    ///
    /// Read by whoever needs to know the shell has an entry point that is not the window
    /// itself; holding the value is what keeps the icon alive, since dropping it removes the
    /// icon and joins the message loop.
    pub fn tray(&self) -> Option<&tray::Tray> {
        self.tray.as_ref()
    }

    /// The hosted capture overlay, when the shell could start one.
    pub fn capture(&self) -> Option<&Arc<snapclip_capture::CaptureRuntime>> {
        self.capture.as_ref()
    }

    /// Whether the clipboard pipeline is running. Holding it is what keeps it alive, so this
    /// is also how anything outside can tell the shell is collecting clips at all.
    pub fn is_collecting_clips(&self) -> bool {
        self.clipboard.is_some()
    }

    /// The hosted pin manager, when the shell could start one.
    ///
    /// Read by whoever needs to know pins are reachable at all; holding the value is what
    /// keeps the pin host thread alive, the same way [`Self::tray`] keeps the icon alive.
    pub fn pins(&self) -> Option<&pin::PinManager> {
        self.pins.as_ref()
    }
}
impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let navigation = div()
            .h_flex()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.background)
            // The active page is marked with a rule under the button rather than a button
            // variant: this version's `Button` styles are not reachable without guessing a
            // trait, and "state must be visible" is satisfied just as clearly this way.
            .child(nav_item(
                self.page == Page::History,
                theme.primary,
                theme.border,
                Button::new("nav-history")
                    .label("剪贴板历史")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::History;
                        cx.notify();
                    }))
                    .into_any_element(),
            ))
            .child(nav_item(
                self.page == Page::Settings,
                theme.primary,
                theme.border,
                Button::new("nav-settings")
                    .label("设置")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::Settings;
                        cx.notify();
                    }))
                    .into_any_element(),
            ));

        let body = match self.page {
            Page::Settings => self.settings.clone().into_any_element(),
            Page::History => match &self.history {
                Ok(view) => view.clone().into_any_element(),
                Err(reason) => div()
                    .v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .bg(cx.theme().background)
                    .text_color(cx.theme().foreground)
                    .child("剪贴板历史不可用")
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(reason.clone()),
                    )
                    .into_any_element(),
            },
        };

        div()
            .v_flex()
            .size_full()
            .bg(theme.background)
            .child(navigation)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

/// Start the shell. The binary entry point is a one-liner on purpose.
pub fn run(options: StartupOptions) {
    // Per-Monitor V2 has to be declared before anything creates a window or reads a cursor
    // position; once a window exists the declaration can no longer be changed and capture
    // geometry would be reported in virtualised coordinates (docs/14 §3). This is why it is
    // here and not in `Shell::new` — GPUI opens its window inside `run`, below. The development
    // panel below is a window too, which is the other reason this stays first.
    #[cfg(windows)]
    match snapclip_capture::windows::monitor::set_per_monitor_v2_awareness() {
        Ok(mode) => eprintln!("[snapclip-app][startup] dpi awareness={mode}"),
        Err(message) => {
            eprintln!("[snapclip-app][startup] dpi awareness declaration failed: {message}")
        }
    }

    // The development entry (`docs/41` S2) adds a window beside the launch and never replaces it
    // (`startup.rs` records why: replacing it once took F5, F7 and F9 with it). Holding the handle
    // on this frame is what keeps the panel's thread and its window alive for as long as the
    // application runs; dropping it closes the panel.
    let _development_panel = if subsystems(options).save_panel {
        match save::window::open(save::window::ShellOptions::default()) {
            Ok(panel) => {
                eprintln!("[save-panel] development entry: the panel opens beside the shell");
                Some(panel)
            }
            Err(error) => {
                eprintln!("[save-panel] development entry failed: {error:?}");
                None
            }
        }
    } else {
        None
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            // MUST be first: component-backed views need the component runtime.
            gpui_kit::init(cx);

            cx.spawn(async move |cx| {
                // A fixed 360×640 window with a real title: the title is what the taskbar and
                // Alt-Tab show, and `WindowOptions::default()` leaves it unnamed.
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.0), px(0.0)),
                        size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)),
                    ))),
                    titlebar: Some(TitlebarOptions {
                        title: Some(WINDOW_TITLE.into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                cx.open_window(options, |window, cx| {
                    let shell = cx.new(|cx| Shell::new(window, cx));
                    // `Root` is the window's first-level child; it owns overlays,
                    // notifications and modal focus restoration.
                    cx.new(|cx| Root::new(shell, window, cx))
                })
                .expect("failed to open the SnapClip shell window");
            })
            .detach();
        });
}
