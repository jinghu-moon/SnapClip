//! The settings page.
//!
//! Follows the Design Guides' "Forms and settings": a visible label per field, help text
//! beside the field it describes, and a `Switch` for a setting that takes effect
//! immediately. The result of a change is shown next to the control, not in a dialog.
//!
//! Copy note (Design Guides, interface language): the label names the object, the help line
//! says what turning it off does, and neither repeats the other.

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::switch::Switch;
use gpui_kit::*;

use super::model::{Settings, SettingsStore};

pub struct SettingsView {
    store: SettingsStore,
    settings: Settings,
    /// Result of the last change, shown beside the control it describes.
    status: Option<String>,
}

impl SettingsView {
    pub fn new(store: SettingsStore) -> Self {
        let settings = store.load();
        Self {
            store,
            settings,
            status: None,
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Apply one change and persist it.
    ///
    /// `Switch` is the control for a setting that takes effect immediately, so this writes
    /// straight through; a failed write is reported next to the control instead of being
    /// swallowed, because a setting the user believes is saved but is not is worse than an
    /// error message.
    fn set_deep_select_text_runs(&mut self, value: bool, cx: &mut Context<Self>) {
        self.settings.deep_select_text_runs = value;
        self.status = Some(match self.store.save(&self.settings) {
            Ok(()) => {
                if value {
                    "已开启".to_string()
                } else {
                    "已关闭".to_string()
                }
            }
            Err(error) => format!("保存失败：{error}"),
        });
        cx.notify();
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .v_flex()
            .size_full()
            .gap_4()
            .p_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(div().text_sm().child("吸附"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("控制截图时鼠标所在内容如何被选为捕获目标"),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        Switch::new("deep-select-text-runs")
                            .checked(self.settings.deep_select_text_runs)
                            .label("吸附到文字行")
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.set_deep_select_text_runs(*checked, cx);
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("关闭后只吸附到盒子，不会单独选中一行文字"),
                    )
                    .children(self.status.clone().map(|status| {
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(status)
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("设置文件：{}", self.store.path().display())),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsView;
    use crate::settings::SettingsStore;

    #[test]
    fn a_view_starts_from_the_file() {
        use std::path::PathBuf;
        let dir = std::env::temp_dir().join(format!(
            "snapclip-app-settings-view-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SettingsStore::new(&dir);
        store
            .save(&crate::settings::Settings {
                deep_select_text_runs: false,
            })
            .expect("save");
        let view = SettingsView::new(SettingsStore::new(PathBuf::from(&dir)));
        assert!(!view.settings().deep_select_text_runs);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
