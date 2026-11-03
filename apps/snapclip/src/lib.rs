//! SnapClip's GPUI shell (docs/23 P4).
//!
//! The shell is a **library** with a thin binary entry point (`main.rs`): integration
//! tests can only import a library, and the Coding Guides put the capability behind a
//! library boundary while the binary only composes it.
//!
//! Rules this crate follows, because the Coding Guides make them requirements:
//! `gpui_kit::init` runs first inside `run`, before any component-backed view exists;
//! each window's first level is `Root`; Yue shares the shell's GPUI/Kit runtime.

// Both traits are re-exported by `gpui-kit` itself (paths verified in gpui-base-0.7.1's
// lib.rs and gpui-component-0.7.1's lib.rs), which is what the one-dependency rule wants:
// `StyledExt` carries `v_flex`/`h_flex`, `ActiveTheme` carries `cx.theme()`.
use gpui_kit::base::StyledExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, Root, Theme, ThemeMode};
use gpui_kit::*;
use snapclip_pin::PinManager;
use std::sync::Arc;
use std::time::Duration;

pub mod adapters;
pub mod capture;
pub mod clipboard;
pub mod clipboard_ingest;
pub mod clipboard_pin;
pub mod events;
pub mod history;
pub mod save;
pub mod save_panel;
pub mod settings;
pub mod startup;
pub mod tray;

use history::icons::SourceIcons;
use history::model::HistoryState;
use history::view::HistoryView;

use events::EventBus;
use startup::{StartupOptions, subsystems};

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
fn spawn_tray_commands(
    commands: async_channel::Receiver<tray::TrayMessage>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) {
    cx.spawn_in(window, async move |shell, cx| {
        while let Ok(message) = commands.recv().await {
            let acted = shell.update_in(cx, |shell, window, cx| shell.command(message, window, cx));
            if acted.is_err() {
                // The shell is gone; there is nothing left to command.
                break;
            }
        }
    })
    .detach();
}

/// Poll the private native image host through the same registry as GPUI windows.
fn spawn_pin_events(pins: &Entity<PinManager>, cx: &mut Context<Shell>) {
    let pins = pins.downgrade();
    cx.spawn(async move |_shell, cx| {
        loop {
            if pins.update(cx, |pins, cx| pins.poll(cx)).is_err() {
                break;
            }
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
    /// The UI-thread pin registry and its private native image host. Failure to
    /// start this feature does not disable F5 capture or clipboard history.
    pins: Option<Entity<PinManager>>,
    pin_source: Option<Arc<capture::PinSource>>,
    clipboard_pins: Option<Entity<clipboard_pin::ClipboardPins>>,
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
        cx.subscribe(
            &settings,
            |this: &mut Self, _, event: &settings::SettingsChanged, _| {
                if let Some(capture) = &this.capture {
                    capture.set_preferences(event.0.overlay_preferences());
                }
            },
        )
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
        let (command_sender, command_receiver) = async_channel::unbounded();
        spawn_tray_commands(command_receiver, window, cx);
        let tray = match tray::Tray::start(command_sender.clone()) {
            Ok(tray) => Some(tray),
            Err(error) => {
                eprintln!("[snapclip-app] tray unavailable: {error}");
                None
            }
        };
        // Clipboard pins do not depend on a capture source, store or history listener.
        let pins = match PinManager::new() {
            Ok((manager, commands)) => {
                let pins = cx.new(|_| manager);
                clipboard_pin::drain_copies(&pins, commands, cx);
                spawn_pin_events(&pins, cx);
                Some(pins)
            }
            Err(error) => {
                eprintln!("[snapclip-app] pin unavailable: {error}");
                None
            }
        };
        let clipboard_pins = pins.as_ref().and_then(|manager| {
            match clipboard_pin::ClipboardPins::start(manager.clone(), cx) {
                Ok(reader) => Some(reader),
                Err(error) => {
                    eprintln!("[snapclip-app] clipboard pin unavailable: {error}");
                    None
                }
            }
        });
        // Delayed capture uses the same application command as F6. Capture only
        // requests pinning; this application thread consumes the selection.
        if let Some(events) = capture_events.as_ref() {
            {
                let commands = command_sender;
                events.set_pin_poster(Arc::new(move || {
                    commands
                        .try_send(tray::TrayMessage::new(tray::TrayCommand::PinLastSelection))
                        .map_err(|error| error.to_string())
                }));
            }
            // Capture hands pixels to the application. The Yue panel lives on the GPUI thread.
            let (requests, incoming) = async_channel::bounded::<crate::save_panel::Picture>(1);
            // Detached rather than bound to a local: a task that is dropped is cancelled, and
            // it takes the receiver with it, which is why the next save reported a closed
            // channel instead of opening a panel. Every other task in this file is detached
            // for the same reason.
            cx.spawn(async move |_shell, cx: &mut AsyncApp| {
                let mut panels = save_panel::PanelWindows::default();
                while let Ok(picture) = incoming.recv().await {
                    let opened = cx.update(|cx| panels.open(picture, cx));
                    if let Err(error) = opened {
                        eprintln!("[save-panel] the framework panel did not open: {error:?}");
                    }
                }
            })
            .detach();
            events.set_save_opener(std::sync::Arc::new(move |pixels| {
                pixels.validate().map_err(|error| error.to_string())?;
                let picture = crate::save_panel::Picture::from_shared_pixels(pixels.pixels.clone());
                requests
                    .try_send(picture)
                    .map_err(|error| format!("{error}"))
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
            pin_source,
            clipboard_pins,
            _label_sync,
        }
    }

    fn command(&mut self, message: tray::TrayMessage, window: &mut Window, cx: &mut Context<Self>) {
        match message.command {
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
            tray::TrayCommand::PinClipboard => {
                if let Some(reader) = &self.clipboard_pins {
                    reader.update(cx, |reader, cx| {
                        reader.request(message.sequence, message.position, cx)
                    });
                } else {
                    snapclip_pin::show_feedback("剪贴板贴图暂不可用", message.position, cx);
                }
            }
            tray::TrayCommand::PinLastSelection => self.pin_last_selection(message.position, cx),
            tray::TrayCommand::ShowPins | tray::TrayCommand::HidePins => {
                if let Some(pins) = &self.pins {
                    pins.update(cx, |pins, cx| {
                        pins.set_visible(message.command == tray::TrayCommand::ShowPins, cx)
                    });
                }
            }
            tray::TrayCommand::ClosePins => {
                if let Some(reader) = &self.clipboard_pins {
                    reader.update(cx, |reader, _| reader.cancel());
                }
                if let Some(pins) = &self.pins {
                    pins.update(cx, |pins, cx| pins.close_all(cx));
                }
            }
            tray::TrayCommand::UndoPin => {
                if let Some(pins) = &self.pins
                    && let Err(error) = PinManager::undo(pins, message.position, cx)
                {
                    snapclip_pin::show_feedback(error.to_string(), message.position, cx);
                }
            }
        }
    }

    fn pin_last_selection(&mut self, position: (i32, i32), cx: &mut Context<Self>) {
        let Some(selection) = self
            .pin_source
            .as_ref()
            .and_then(|source| source.take_selection())
        else {
            snapclip_pin::show_feedback("还没有可贴的截图，请先按 F5 框选", position, cx);
            return;
        };
        if let Some(pins) = &self.pins
            && let Err(error) = PinManager::open_selection(selection, pins, cx)
        {
            snapclip_pin::show_feedback(error.to_string(), position, cx);
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
        let Some(preferences) = self
            .capture
            .as_ref()
            .and_then(|capture| capture.preferences())
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
    pub fn pins(&self) -> Option<&Entity<PinManager>> {
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
            ))
            .child(div().flex_1())
            .child(
                Button::new("pin-clipboard")
                    .label("贴出剪贴板")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.command(
                            tray::TrayMessage::new(tray::TrayCommand::PinClipboard),
                            window,
                            cx,
                        );
                    })),
            );

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
        // The path field opens where the shell says, the same way a panel opened by a capture
        // does. Everything else is absent on purpose: there is no capture here, so there is no
        // picture and no size to show.
        let development = save::window::ShellOptions {
            directory: save::dirs::desktop(),
            ..Default::default()
        };
        match save::window::open(development) {
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

            let preview = std::env::var_os("SNAPCLIP_YUE_PREVIEW").is_some();
            if preview {
                Theme::change(ThemeMode::Dark, None, cx);
            }
            let dark = cx.theme().mode == ThemeMode::Dark;
            save_panel::init(cx, dark);
            snapclip_pin::init(cx);

            cx.spawn(async move |cx| {
                if preview {
                    let picture = save_panel::Picture::preview_example();
                    cx.update(|cx| save_panel::PanelWindows::default().open(picture, cx))
                        .expect("open Yue save panel");
                    return;
                }
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
