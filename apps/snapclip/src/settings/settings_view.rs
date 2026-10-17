//! The settings page.
//!
//! Follows the Design Guides' "Forms and settings": a visible label per field, help text
//! beside the field it describes, a `Switch` for a setting that takes effect immediately and
//! radio buttons for a small set of choices (two `Radio`s rather than a `RadioGroup`, because
//! that component renames its children — docs/23 §14.52). The result of a change is shown next to
//! the control, not in a dialog.
//!
//! Copy note (Design Guides, interface language): the label names the object, the help line
//! says what the control does, and neither repeats the other.

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::radio::Radio;
use gpui_kit::component::switch::Switch;
use gpui_kit::*;

use snapclip_capture::units::{LabelDisplay, LengthUnit};

use super::model::{Settings, SettingsStore};

/// The settings after a change was saved; the shell forwards what capture needs.
#[derive(Debug, Clone)]
pub struct SettingsChanged(pub Settings);

/// The switches on the page. Each one writes exactly one setting, and the pairing is checked by
/// the compiler rather than by a string that has to match the widget id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Toggle {
    DeepSelectTextRuns,
    MaskBlur,
    ShortcutGuide,
    ShowSize,
    ShowCoordinates,
    ShowUnit,
}

/// The label units in the order the radio group shows them.
///
/// The group speaks in indices and the settings speak in units; keeping one list for both means
/// the pair cannot drift apart when a third unit arrives.
const LABEL_UNITS: [LengthUnit; 2] = [LengthUnit::Px, LengthUnit::Dp];

/// The id of the unit row: the widget's id and the key its result is shown under are the same.
const LABEL_UNIT_ID: &str = "label-unit";

pub struct SettingsView {
    store: SettingsStore,
    settings: Settings,
    /// Result of the last change, shown beside the control it describes.
    status: Option<(&'static str, String)>,
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

    /// Apply one switch and persist it.
    ///
    /// `Switch` is the control for a setting that takes effect immediately, so this writes
    /// straight through; a failed write is reported next to the control instead of being
    /// swallowed, because a setting the user believes is saved but is not is worse than an
    /// error message.
    fn set_switch(
        &mut self,
        toggle: Toggle,
        value: bool,
        id: &'static str,
        cx: &mut Context<Self>,
    ) {
        match toggle {
            Toggle::DeepSelectTextRuns => self.settings.deep_select_text_runs = value,
            Toggle::MaskBlur => self.settings.mask_blur = value,
            Toggle::ShortcutGuide => self.settings.shortcut_guide = value,
            Toggle::ShowSize => self.settings.show_size = value,
            Toggle::ShowCoordinates => self.settings.show_coordinates = value,
            Toggle::ShowUnit => self.settings.show_unit = value,
        }
        let result = if value { "已开启" } else { "已关闭" }.to_string();
        self.persist(id, result, cx);
    }

    /// Apply the label unit, which is a choice rather than a switch.
    fn set_length_unit(&mut self, unit: LengthUnit, cx: &mut Context<Self>) {
        self.settings.length_unit = unit;
        self.persist(LABEL_UNIT_ID, format!("已改为 {}", unit.suffix()), cx);
    }

    /// Adopt the display settings a capture ended with (docs/23 §14.53).
    ///
    /// The overlay's badge menu edits the same three values the page shows, and the shell hands the
    /// result back here when the session is over. Writing through this view — rather than to the
    /// file directly — is what keeps the page and the file from disagreeing, and what makes the
    /// existing `SettingsChanged` subscriber push the change back to capture for the next session.
    ///
    /// Returns whether anything changed, so the caller can stay quiet about an unchanged capture.
    pub fn adopt_label(&mut self, label: LabelDisplay, cx: &mut Context<Self>) -> bool {
        if !self.settings.adopt_label(label) {
            return false;
        }
        // The page's status line names the control the user touched; this change came from the
        // overlay, so it is reported under the unit row that the two changes have in common.
        self.persist(LABEL_UNIT_ID, "已从截图浮层更新".to_string(), cx);
        true
    }

    /// Write the settings out and report the result beside the control that changed.
    fn persist(&mut self, at: &'static str, result: String, cx: &mut Context<Self>) {
        let status = match self.store.save(&self.settings) {
            Ok(()) => {
                cx.emit(SettingsChanged(self.settings.clone()));
                result
            }
            Err(error) => format!("保存失败：{error}"),
        };
        self.status = Some((at, status));
        cx.notify();
    }

    /// One switch, its help line and, when it was the last one changed, the result.
    fn switch(
        &self,
        toggle: Toggle,
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
                        this.set_switch(toggle, *checked, id, cx);
                    })),
            )
            .child(div().text_xs().text_color(muted).child(help))
            .children(self.result(id, muted))
    }

    /// The label unit: one visible label, the two choices, the formula behind `dp` and, when it
    /// was the last control changed, the result.
    ///
    /// Two `Radio`s in a row rather than a `RadioGroup`: the group numbers its children by
    /// position and overwrites their ids, which leaves each choice unnamed in the accessibility
    /// tree and untargetable by tests, while the group's own base adds nothing beyond that
    /// numbering. Each radio here carries the unit it stands for as its id and writes it directly,
    /// so there is no index-to-unit table to keep in step.
    ///
    /// The help line carries the arithmetic because `dp` is not a word the user has to already
    /// know, and the scale is read from the display the capture happens on rather than guessed
    /// for the display this window happens to be on.
    fn unit_choice(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        div()
            .v_flex()
            .gap_1()
            .child(div().text_sm().child("标签单位"))
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .children(LABEL_UNITS.map(|unit| self.unit_radio(unit, cx))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("dp = px ÷ 屏幕缩放：150% 显示器上 1500 px = 1000 dp；尺寸和坐标一起换算"),
            )
            .children(self.result(LABEL_UNIT_ID, muted))
    }

    /// One unit choice: the unit it stands for is its id, and picking it writes that unit.
    fn unit_radio(&self, unit: LengthUnit, cx: &mut Context<Self>) -> Radio {
        let id = match unit {
            LengthUnit::Px => "label-unit-px",
            LengthUnit::Dp => "label-unit-dp",
        };
        Radio::new(id)
            .label(unit.suffix())
            .checked(unit == self.settings.length_unit)
            .on_click(cx.listener(move |this, _: &bool, _, cx| this.set_length_unit(unit, cx)))
    }

    /// The result of the last change, when that change was this control's.
    fn result(&self, at: &'static str, muted: Hsla) -> Option<impl IntoElement> {
        self.status
            .as_ref()
            .filter(|(changed, _)| *changed == at)
            .map(|(_, status)| div().text_xs().text_color(muted).child(status.clone()))
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
                Toggle::DeepSelectTextRuns,
                "deep-select-text-runs",
                self.settings.deep_select_text_runs,
                "吸附到文字行",
                "关闭后只吸附到盒子，不会单独选中一行文字",
                cx,
            ))
            .child(Self::section("截图界面", "从下一次截图开始生效", cx))
            .child(self.switch(
                Toggle::MaskBlur,
                "mask-blur",
                self.settings.mask_blur,
                "模糊选区外的画面",
                "关闭后选区外只变暗，不模糊；保存的图片不受影响",
                cx,
            ))
            .child(self.switch(
                Toggle::ShortcutGuide,
                "shortcut-guide",
                self.settings.shortcut_guide,
                "显示快捷键面板",
                "关闭后左下角不再列出快捷键",
                cx,
            ))
            .child(self.switch(
                Toggle::ShowSize,
                "label-size",
                self.settings.show_size,
                "标签显示尺寸",
                "关闭后标签只写坐标（尺寸标上就只剩位置）",
                cx,
            ))
            .child(self.switch(
                Toggle::ShowCoordinates,
                "label-coordinates",
                self.settings.show_coordinates,
                "标签显示坐标",
                "关闭后只显示尺寸；位置是选区左上角在屏幕上的物理像素",
                cx,
            ))
            .child(self.unit_choice(cx))
            .child(self.switch(
                Toggle::ShowUnit,
                "label-unit-suffix",
                self.settings.show_unit,
                "标签显示单位",
                "关闭后标签只写数字（1024×768），单位由左边的选项决定",
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
