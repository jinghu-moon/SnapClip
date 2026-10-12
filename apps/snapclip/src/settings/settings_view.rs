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

/// The settings after a change was saved; the shell forwards what capture needs.
#[derive(Debug, Clone)]
pub struct SettingsChanged(pub Settings);

/// The switches on the page, which is also where a result is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    DeepSelectTextRuns,
    MaskBlur,
    ShortcutGuide,
}

pub struct SettingsView {
    store: SettingsStore,
    settings: Settings,
    /// Result of the last change, shown beside the control it describes.
    status: Option<(Field, String)>,
}

impl EventEmitter<SettingsChanged> for SettingsView {}

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
    fn set(&mut self, field: Field, value: bool, cx: &mut Context<Self>) {
        *match field {
            Field::DeepSelectTextRuns => &mut self.settings.deep_select_text_runs,
            Field::MaskBlur => &mut self.settings.mask_blur,
            Field::ShortcutGuide => &mut self.settings.shortcut_guide,
        } = value;
        let status = match self.store.save(&self.settings) {
            Ok(()) => {
                cx.emit(SettingsChanged(self.settings.clone()));
                if value { "已开启" } else { "已关闭" }.to_string()
            }
            Err(error) => format!("保存失败：{error}"),
        };
        self.status = Some((field, status));
        cx.notify();
    }

    /// One switch, its help line and, when it was the last one changed, the result.
    fn switch(
        &self,
        field: Field,
        id: &'static str,
        checked: bool,
        label: &'static str,
        help: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        div()
            .v_flex()
            .gap_1()
            .child(
                Switch::new(id)
                    .checked(checked)
                    .label(label)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        this.set(field, *checked, cx);
                    })),
            )
            .child(div().text_xs().text_color(muted).child(help))
            .children(
                self.status
                    .as_ref()
                    .filter(|(at, _)| *at == field)
                    .map(|(_, status)| div().text_xs().text_color(muted).child(status.clone())),
            )
    }

    fn section(title: &'static str, help: &'static str, cx: &Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_1()
            .child(div().text_sm().child(title))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(help),
            )
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, foreground, muted) =
            (theme.background, theme.foreground, theme.muted_foreground);
        div()
            .v_flex()
            .size_full()
            .gap_4()
            .p_4()
            .bg(background)
            .text_color(foreground)
            .child(Self::section(
                "吸附",
                "控制截图时鼠标所在内容如何被选为捕获目标",
                cx,
            ))
            .child(self.switch(
                Field::DeepSelectTextRuns,
                "deep-select-text-runs",
                self.settings.deep_select_text_runs,
                "吸附到文字行",
                "关闭后只吸附到盒子，不会单独选中一行文字",
                cx,
            ))
            .child(Self::section("截图界面", "从下一次截图开始生效", cx))
            .child(self.switch(
                Field::MaskBlur,
                "mask-blur",
                self.settings.mask_blur,
                "模糊选区外的画面",
                "关闭后选区外只变暗，不模糊；保存的图片不受影响",
                cx,
            ))
            .child(self.switch(
                Field::ShortcutGuide,
                "shortcut-guide",
                self.settings.shortcut_guide,
                "显示快捷键面板",
                "关闭后左下角不再列出快捷键",
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
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
        let dir =
            std::env::temp_dir().join(format!("snapclip-app-settings-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SettingsStore::new(&dir);
        store
            .save(&crate::settings::Settings {
                deep_select_text_runs: false,
                ..Default::default()
            })
            .expect("save");
        let view = SettingsView::new(SettingsStore::new(PathBuf::from(&dir)));
        assert!(!view.settings().deep_select_text_runs);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
