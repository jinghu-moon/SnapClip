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

pub mod adapters;
pub mod capture;
pub mod clipboard;
pub mod clipboard_ingest;
pub mod events;
pub mod history;
pub mod settings;
pub mod tray;

use history::icons::SourceIcons;
use history::model::HistoryState;
use history::view::HistoryView;

use events::EventBus;

/// The same database and cache directories the Tauri host writes while both shells exist
/// (docs/23 T4.9: they share only the capability crates and the data).
pub fn app_data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("com.seeyuer.snapclip")
}

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
    tray: &tray::Tray,
    window: &mut Window,
    cx: &mut Context<Shell>,
) {
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
    capture: Option<snapclip_capture::CaptureRuntime>,
    /// The clipboard pipeline. Holding this is what keeps it running; dropping it stops the
    /// listener and joins the worker, so it must live as long as the shell.
    clipboard: Option<snapclip_history::ingest::ClipboardIngestHandle>,
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
                        Ok(cx.new(|cx| {
                            HistoryView::new(state, icons, events.clone(), window, cx)
                        })),
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
        let detection_options = settings_store.load().detection_options();
        let settings = cx.new(|_| settings::SettingsView::new(settings::SettingsStore::new(&data)));
        // Capture is hosted here from P6 on: F5, the overlay and the export chain belong to
        // this process, which is what lets the old Tauri shell be deleted.
        let capture = match capture::start(&data, events.clone(), detection_options) {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                eprintln!("[snapclip-app] capture unavailable: {error}");
                None
            }
        };
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
        Self {
            history,
            settings,
            page: Page::History,
            events,
            tray,
            capture,
            clipboard,
        }
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
    pub fn capture(&self) -> Option<&snapclip_capture::CaptureRuntime> {
        self.capture.as_ref()
    }

    /// Whether the clipboard pipeline is running. Holding it is what keeps it alive, so this
    /// is also how anything outside can tell the shell is collecting clips at all.
    pub fn is_collecting_clips(&self) -> bool {
        self.clipboard.is_some()
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
                    .child("Clipboard history is unavailable")
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
pub fn run() {
    // Per-Monitor V2 has to be declared before anything creates a window or reads a cursor
    // position; once a window exists the declaration can no longer be changed and capture
    // geometry would be reported in virtualised coordinates (docs/14 §3). This is why it is
    // here and not in `Shell::new` — GPUI opens its window inside `run`, below.
    #[cfg(windows)]
    match snapclip_capture::windows::monitor::set_per_monitor_v2_awareness() {
        Ok(mode) => eprintln!("[snapclip-app][startup] dpi awareness={mode}"),
        Err(message) => {
            eprintln!("[snapclip-app][startup] dpi awareness declaration failed: {message}")
        }
    }

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            // MUST be first: component-backed views need the component runtime.
            gpui_kit::init(cx);

            cx.spawn(async move |cx| {
                cx.open_window(WindowOptions::default(), |window, cx| {
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
