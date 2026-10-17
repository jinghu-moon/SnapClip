//! 自定义截图面板的值与规则（docs/23 §14.54）。
//!
//! 面板是"精确指定一块区域"的入口：四个数字、一个单位、一个延时，加上浮层尺寸标能显示的三项。
//! 这里只有值和方法——没有窗口、没有 D2D——所以"数字是什么意思""按一个键改成什么""什么时候拒绝"
//! 由测试钉住，而不是靠截图。
//!
//! 坐标系是用户的坐标系：和尺寸标、导出产物用的是同一套**桌面像素**，所以主屏左边那块屏上的
//! 负 x 是一个普通数字，不是错误。会话内部的选区是**本屏像素**，换算只发生在控制器调用
//! `snap_to` 的那一处。

use crate::geometry::{Point, Rect};
use crate::units::{LabelToggle, LengthUnit};

/// 面板上四个数字里的一格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionField {
    Width,
    Height,
    X,
    Y,
}

impl RegionField {
    /// 阅读顺序：两张卡的顺序，也是 Tab 走四个数值格的顺序。
    pub const ALL: [Self; 4] = [Self::Width, Self::Height, Self::X, Self::Y];

    pub fn label(self) -> &'static str {
        match self {
            Self::Width => "宽",
            Self::Height => "高",
            Self::X => "X",
            Self::Y => "Y",
        }
    }

    /// 这一格对应的焦点槽（布局的命中测试从矩形回到槽，走的就是这条对应）。
    pub fn slot(self) -> PanelSlot {
        match self {
            Self::Width => PanelSlot::Width,
            Self::Height => PanelSlot::Height,
            Self::X => PanelSlot::X,
            Self::Y => PanelSlot::Y,
        }
    }
}

/// 一块区域，桌面像素：用户看到、输入、核对的就是这一组数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
}

impl Region {
    /// 会话选区（本屏像素）对应的桌面区域。
    pub fn from_rect(rect: Rect, origin: Point) -> Self {
        Self {
            width: rect.width(),
            height: rect.height(),
            x: rect.left + origin.x,
            y: rect.top + origin.y,
        }
    }

    /// 换回本屏像素的矩形。
    pub fn to_rect(self, origin: Point) -> Rect {
        Rect::from_origin_size(
            Point::new(self.x - origin.x, self.y - origin.y),
            self.width,
            self.height,
        )
    }

    /// `field` 在 `unit` 下的读数。
    pub fn shown(self, field: RegionField, unit: LengthUnit, dpi: u32) -> i32 {
        unit.from_px(self.get(field), dpi)
    }

    pub fn get(self, field: RegionField) -> i32 {
        match field {
            RegionField::Width => self.width,
            RegionField::Height => self.height,
            RegionField::X => self.x,
            RegionField::Y => self.y,
        }
    }

    /// 改一格的像素值。宽高至少 1：一个 0 宽的选区不是区域，只是还没开始选。
    pub fn set(self, field: RegionField, value: i32) -> Self {
        match field {
            RegionField::Width => Self {
                width: value.max(1),
                ..self
            },
            RegionField::Height => Self {
                height: value.max(1),
                ..self
            },
            RegionField::X => Self { x: value, ..self },
            RegionField::Y => Self { y: value, ..self },
        }
    }

    pub fn swapped(self) -> Self {
        Self {
            width: self.height,
            height: self.width,
            ..self
        }
    }

    /// 在本屏里居中。
    pub fn centered(self, screen: Rect) -> Self {
        Self {
            x: screen.left + (screen.width() - self.width) / 2,
            y: screen.top + (screen.height() - self.height) / 2,
            ..self
        }
    }

    /// 贴到本屏左上角。
    pub fn at_origin(self, screen: Rect) -> Self {
        Self {
            x: screen.left,
            y: screen.top,
            ..self
        }
    }

    /// 按锁定时的比例改一格：改宽就跟着改高，改高就跟着改宽（docs/28 §2.2 的"固定比例"）。
    pub fn with_ratio(self, field: RegionField, value: i32, ratio: (i32, i32)) -> Self {
        let (rw, rh) = (ratio.0.max(1), ratio.1.max(1));
        match field {
            RegionField::Width => {
                let width = value.max(1);
                let height = ((i64::from(width) * i64::from(rh)) / i64::from(rw)).max(1) as i32;
                Self {
                    width,
                    height,
                    ..self
                }
            }
            RegionField::Height => {
                let height = value.max(1);
                let width = ((i64::from(height) * i64::from(rw)) / i64::from(rh)).max(1) as i32;
                Self {
                    width,
                    height,
                    ..self
                }
            }
            RegionField::X | RegionField::Y => self.set(field, value),
        }
    }

    /// 把区域挪进 `screen`：换预设尺寸之后仍然完整可见。
    ///
    /// 只挪不改尺寸：预设给的是大小，位置是用户的；尺寸本身放不下时应当由校验说出来，
    /// 而不是悄悄缩水。
    pub fn inside(self, screen: Rect) -> Self {
        let max_x = (screen.right - self.width).max(screen.left);
        let max_y = (screen.bottom - self.height).max(screen.top);
        Self {
            x: self.x.clamp(screen.left, max_x),
            y: self.y.clamp(screen.top, max_y),
            ..self
        }
    }

    /// 这一组数能不能用；能用就是 `None`。
    pub fn check(self, screen: Rect, minimum: i32) -> Option<RegionProblem> {
        let minimum = minimum.max(1);
        if self.width < minimum || self.height < minimum {
            return Some(RegionProblem::TooSmall);
        }
        if self.width > screen.width()
            || self.height > screen.height()
            || self.x < screen.left
            || self.y < screen.top
            || self.x + self.width > screen.right
            || self.y + self.height > screen.bottom
        {
            return Some(RegionProblem::Outside {
                max_width: screen.width(),
                max_height: screen.height(),
            });
        }
        None
    }
}

/// 一组数被拒绝的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionProblem {
    /// 小于本屏允许的最小边长。
    TooSmall,
    /// 放不进本屏。
    Outside { max_width: i32, max_height: i32 },
}

impl RegionProblem {
    /// 要描红的那几格 —— 由错误本身决定，不替用户挑一格。
    pub fn fields(self) -> &'static [RegionField] {
        match self {
            Self::TooSmall => &[RegionField::Width, RegionField::Height],
            // 放不下既可能是尺寸也可能是位置，两处都标出来。
            Self::Outside { .. } => &RegionField::ALL,
        }
    }

    /// 页脚那一句话：发生了什么 + 怎么改。
    pub fn message(self, unit: LengthUnit, dpi: u32) -> String {
        match self {
            Self::TooSmall => {
                format!("宽和高要大于 {}", 0.max(unit.from_px(1, dpi)))
            }
            Self::Outside {
                max_width,
                max_height,
            } => format!(
                "本屏放不下：宽 ≤ {}、高 ≤ {}",
                unit.from_px(max_width, dpi),
                unit.from_px(max_height, dpi)
            ),
        }
    }
}

/// 截图延时：立即，或等一会儿让画面先稳定下来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delay(u32);

impl Delay {
    /// 面板给的四档（与 PixPin 的自定义截图面板同构，docs/28 §2.1）。
    pub const ALL: [Self; 4] = [Self(0), Self(3), Self(5), Self(10)];

    /// 分段器里的次序：与 `RegionPanelLayout::delay` 的数组顺序一致（动画插值按它编号）。
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|item| *item == self).unwrap_or(0)
    }

    pub fn seconds(self) -> u32 {
        self.0
    }

    pub fn label(self) -> &'static str {
        match self.0 {
            0 => "0",
            3 => "3s",
            5 => "5s",
            _ => "10s",
        }
    }
}

/// 内置的预设尺寸（PixPin 的默认四项，docs/28 §2.2）。
///
/// 名称与尺寸成对出现，所以改名不会错位；`500×500` 那一项没有名字，用尺寸当名字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    pub width: i32,
    pub height: i32,
}

pub const PRESETS: [Preset; 4] = [
    Preset {
        name: "500×500",
        width: 500,
        height: 500,
    },
    Preset {
        name: "480p",
        width: 640,
        height: 480,
    },
    Preset {
        name: "720p",
        width: 1280,
        height: 720,
    },
    Preset {
        name: "1080p",
        width: 1920,
        height: 1080,
    },
];

/// 当前区域命中的预设，没有命中就是 `None`（面板显示"自定义"）。
pub fn preset_for(region: Region) -> Option<&'static Preset> {
    PRESETS
        .iter()
        .find(|preset| preset.width == region.width && preset.height == region.height)
}

/// 面板上的焦点槽，按 Tab 顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSlot {
    Unit,
    Width,
    Height,
    Swap,
    Lock,
    X,
    Y,
    Center,
    Origin,
    Preset,
    Delay,
    ShowSize,
    ShowPosition,
    ShowUnit,
    Cancel,
    Start,
}

impl PanelSlot {
    pub const ORDER: [Self; 16] = [
        // 页头那一组在最上面，所以 Tab 从它开始；默认焦点另说（见 `RegionPanel::new`）。
        Self::Unit,
        Self::Width,
        Self::Height,
        Self::Swap,
        Self::Lock,
        Self::X,
        Self::Y,
        Self::Center,
        Self::Origin,
        Self::Preset,
        Self::Delay,
        Self::ShowSize,
        Self::ShowPosition,
        Self::ShowUnit,
        Self::Cancel,
        Self::Start,
    ];

    pub fn field(self) -> Option<RegionField> {
        match self {
            Self::Width => Some(RegionField::Width),
            Self::Height => Some(RegionField::Height),
            Self::X => Some(RegionField::X),
            Self::Y => Some(RegionField::Y),
            _ => None,
        }
    }

    pub fn toggle(self) -> Option<LabelToggle> {
        match self {
            Self::ShowSize => Some(LabelToggle::Size),
            Self::ShowPosition => Some(LabelToggle::Position),
            Self::ShowUnit => Some(LabelToggle::Unit),
            _ => None,
        }
    }

    /// 下一个 / 上一个槽，两端回卷。
    pub fn step(self, forward: bool) -> Self {
        let index = Self::ORDER.iter().position(|slot| *slot == self).unwrap_or(0);
        let count = Self::ORDER.len();
        let next = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        Self::ORDER[next]
    }
}

impl crate::units::LabelToggle {
    /// 这一项对应的焦点槽。
    pub fn slot(self) -> PanelSlot {
        match self {
            Self::Size => PanelSlot::ShowSize,
            Self::Position => PanelSlot::ShowPosition,
            Self::Unit => PanelSlot::ShowUnit,
        }
    }
}

/// 一个字符落成哪种面板按键（字符入口用：`WM_CHAR`，中文输入法组合结束后送来的也是字符）。
///
/// 只认数字，而且是**全角也认**：中文输入法在全角状态下送来的是 `１`（U+FF10..）。如果只认 ASCII，
/// 用户在中文输入法下敲数字会一个字都进不来——这正是这个缺陷的真实形态（docs/23 §14.55）。
/// 别在这里开口子：数字格的规则（只数字、7 位以内）在 [`RegionPanel::key`] 的 `Digit` 分支里。
pub fn panel_key_from_char(character: char) -> Option<PanelKey> {
    let code = character as u32;
    let digit = match code {
        0x30..=0x39 => code - 0x30,
        0xFF10..=0xFF19 => code - 0xFF10,
        _ => return None,
    };
    Some(PanelKey::Digit(digit as u8))
}

/// 一次按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKey {
    Tab,
    BackTab,
    Left,
    Right,
    Up,
    Down,
    Enter,
    Escape,
    Backspace,
    /// 删光标右侧一个字符。
    Delete,
    /// 光标移到缓冲首/尾。
    Home,
    End,
    Digit(u8),
}

/// 面板处理完一次输入之后，控制器要做的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelChange {
    /// 什么都不用做。
    None,
    /// 面板自己的外观变了（焦点、下拉、单位、延时）：重画即可。
    Repaint,
    /// 区域变了：会话的选区跟着走。
    Region(Region),
    /// 浮层尺寸标的一项开关翻面（值在偏好里，不在面板里）。
    Toggle(LabelToggle),
    /// 就按这个区域截。
    Confirm(Region),
    /// 离开面板，并取消这次截图。
    Cancel,
}

/// 正在输入的格子：数字缓冲，以及下一个数字是替换还是追加。
#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldText {
    field: RegionField,
    text: String,
    /// 插入点在**缓冲内的字节位置**（0..=len）。
    ///
    /// 缓冲只装 ASCII 数字（输入在入口就过滤了），所以字节位置与字符位置一致。始终 clamp 在
    /// 0..=len：越界的 caret 会让插入/删除落到错误的位置。
    caret: usize,
}

/// 面板的全部交互状态。
///
/// 四个数字**不在这里**：它们就是会话的选区（`Region` 每帧由控制器算出来传进来），
/// 所以"面板显示的"和"会被截的"不可能各说一套。
#[derive(Debug, Clone, PartialEq)]
pub struct RegionPanel {
    unit: LengthUnit,
    delay: Delay,
    /// 锁定时的宽高比；`None` 表示未锁。
    ratio: Option<(i32, i32)>,
    focus: PanelSlot,
    editing: Option<FieldText>,
    /// 预设下拉展开时的键盘高亮项；`None` 表示收起。
    menu: Option<usize>,
    screen: Rect,
    minimum: i32,
}

impl RegionPanel {
    pub fn new(unit: LengthUnit, delay: Delay, screen: Rect, minimum: i32) -> Self {
        Self {
            unit,
            delay,
            ratio: None,
            focus: PanelSlot::Width,
            editing: None,
            menu: None,
            screen,
            minimum: minimum.max(1),
        }
    }

    pub fn unit(&self) -> LengthUnit {
        self.unit
    }

    pub fn delay(&self) -> Delay {
        self.delay
    }

    pub fn locked(&self) -> bool {
        self.ratio.is_some()
    }

    pub fn focus(&self) -> PanelSlot {
        self.focus
    }

    /// 展开中的预设下拉，以及键盘停在那一项。
    pub fn menu(&self) -> Option<usize> {
        self.menu
    }

    /// 页脚那句话（校验失败时）。
    ///
    /// 按**此刻**的区域现算，而不是存一份：面板开着的时候用户也可能在外面拖选区，
    /// 存下来的那句会陈旧（docs/23 §14.54）。
    pub fn status(&self, region: Region, dpi: u32) -> Option<String> {
        self.problem(region)
            .map(|problem| problem.message(self.unit, dpi))
    }

    /// 正在输入的那一格，没有就是 `None`。
    pub fn editing(&self) -> Option<RegionField> {
        self.editing.as_ref().map(|edit| edit.field)
    }

    /// 本屏的桌面矩形（校验与"居中"都用它）。
    pub fn screen(&self) -> Rect {
        self.screen
    }

    /// 某一格此刻该显示的文本：正在输入就显示输入缓冲，否则显示换算后的读数。
    pub fn field_text(&self, field: RegionField, region: Region, dpi: u32) -> String {
        if let Some(edit) = self.editing.as_ref().filter(|edit| edit.field == field) {
            return edit.text.clone();
        }
        region.shown(field, self.unit, dpi).to_string()
    }

    /// 当前区域为什么不能用；能用就是 `None`。绘制用它决定描红哪几格。
    pub fn problem(&self, region: Region) -> Option<RegionProblem> {
        region.check(self.screen, self.minimum)
    }

    /// 换一个显示单位：读数跟着换，区域本身不动。
    pub fn set_unit(&mut self, unit: LengthUnit) {
        self.unit = unit;
        self.editing = None;
    }

    /// 聚焦某个槽：离开输入格就把输入缓冲丢掉（数字在按的时候已经提交了）。
    fn focus_slot(&mut self, slot: PanelSlot) {
        if self.focus.field() != slot.field() {
            self.editing = None;
        }
        self.focus = slot;
    }

    /// 聚焦到一个数值格：缓冲里放当前读数，**光标落在末尾**。
    ///
    /// 以前这里是"整格待替换"（第一个数字替换全部），用户报的就是它的两个后果：退格一次清空、
    /// 光标无法定位。现在是普通文本框语义（docs/23 §14.55）。
    fn begin_edit(&mut self, region: Region, dpi: u32) {
        if let Some(field) = self.focus.field() {
            let text = region.shown(field, self.unit, dpi).to_string();
            let caret = text.len();
            self.editing = Some(FieldText { field, text, caret });
        }
    }

    /// 正在输入那一格的插入点位置（没在输入就是 0）。视图用它画光标。
    pub fn caret(&self) -> usize {
        self.editing.as_ref().map_or(0, |edit| edit.caret)
    }

    /// 把输入缓冲变成区域的像素值；缓冲不是数字就保持原样。
    fn commit(&mut self, region: Region, dpi: u32) -> Option<Region> {
        let edit = self.editing.as_ref()?;
        let value: i32 = edit.text.parse().ok()?;
        let field = edit.field;
        let px = self.unit.to_px(value, dpi);
        let next = match self.ratio {
            Some(ratio) => region.with_ratio(field, px, ratio),
            None => region.set(field, px),
        };
        // 缓冲换成提交后的读数：用户看到的数和他敲的数在两种单位下都一致。
        // 光标**留在用户放的地方**（只 clamp）：不然在中间插一位就会跳到末尾。
        let text = next.shown(field, self.unit, dpi).to_string();
        let caret = self.editing.as_ref().map_or(0, |edit| edit.caret).min(text.len());
        self.editing = Some(FieldText { field, text, caret });
        Some(next)
    }

    /// 鼠标点了下拉里的第几项：键盘高亮挪过去，再按一次 Enter 的语义。
    pub fn pick(&mut self, index: usize, region: Region, dpi: u32) -> PanelChange {
        self.menu = Some(index.min(PRESETS.len() - 1));
        self.key(PanelKey::Enter, region, dpi)
    }

    /// 鼠标点了某个槽（点在哪一格由布局算出来）。
    /// 点了延时分段器里的哪一段：分段器的语义就是"点哪段选哪段"（不再走"整组 + Enter"的绕路）。
    ///
    /// 键盘在组内换段仍然走 [`RegionPanel::key`]，两条路都落在同一个 `delay` 上。
    pub fn choose_delay(&mut self, delay: Delay) -> PanelChange {
        self.focus_slot(PanelSlot::Delay);
        self.delay = delay;
        PanelChange::Repaint
    }

    pub fn click(&mut self, slot: PanelSlot, region: Region, dpi: u32) -> PanelChange {
        if self.menu.is_some() {
            // 下拉开着：点哪都先收起下拉，别顺手把别的槽也按了。
            self.menu = None;
            return PanelChange::Repaint;
        }
        if let Some(_field) = slot.field() {
            self.focus_slot(slot);
            self.begin_edit(region, dpi);
            
            return PanelChange::Repaint;
        }
        self.focus_slot(slot);
        self.key(PanelKey::Enter, region, dpi)
    }

    /// 一次按键。
    pub fn key(&mut self, key: PanelKey, region: Region, dpi: u32) -> PanelChange {
        // 下拉展开时，上下键挪高亮、Enter 选中、Esc 收起，别的键让位。
        if let Some(index) = self.menu {
            return match key {
                PanelKey::Up | PanelKey::Down => {
                    let count = PRESETS.len();
                    let next = if key == PanelKey::Down {
                        (index + 1) % count
                    } else {
                        (index + count - 1) % count
                    };
                    self.menu = Some(next);
                    PanelChange::Repaint
                }
                PanelKey::Enter => {
                    let preset = PRESETS[index.min(PRESETS.len() - 1)];
                    self.menu = None;
                    let next = region
                        .set(RegionField::Width, preset.width)
                        .set(RegionField::Height, preset.height)
                        .inside(self.screen);
                    
                    PanelChange::Region(next)
                }
                PanelKey::Escape | PanelKey::Tab | PanelKey::BackTab => {
                    self.menu = None;
                    PanelChange::Repaint
                }
                _ => PanelChange::None,
            };
        }

        // 输入格先吃数字、退格/Delete 与光标移动。
        if self.editing.is_some() {
            match key {
                PanelKey::Digit(digit) => {
                    if let Some(edit) = self.editing.as_mut() {
                        if edit.text.len() < 7 {
                            let insert_at = edit.caret.min(edit.text.len());
                            edit.text.insert(insert_at, char::from(b'0' + digit.min(9)));
                            edit.caret = insert_at + 1;
                        }
                    }
                    return match self.commit(region, dpi) {
                        Some(next) => {
                            
                            PanelChange::Region(next)
                        }
                        None => {
                            
                            PanelChange::Repaint
                        }
                    };
                }
                PanelKey::Backspace => {
                    if let Some(edit) = self.editing.as_mut() {
                        // 删光标左侧**一个**字符；已经在最左边就什么都不做。
                        if edit.caret > 0 {
                            edit.caret -= 1;
                            edit.text.remove(edit.caret);
                        }
                    }
                    if self.editing.as_ref().is_some_and(|edit| edit.text.is_empty()) {
                        // 空输入不改区域：留着空格子等用户接着敲。
                        
                        return PanelChange::Repaint;
                    }
                    return match self.commit(region, dpi) {
                        Some(next) => {
                            
                            PanelChange::Region(next)
                        }
                        None => PanelChange::Repaint,
                    };
                }
                PanelKey::Delete => {
                    if let Some(edit) = self.editing.as_mut() {
                        // 删光标右侧一个字符；光标在末尾就什么都不做。
                        if edit.caret < edit.text.len() {
                            edit.text.remove(edit.caret);
                        }
                    }
                    if self
                        .editing
                        .as_ref()
                        .is_some_and(|edit| edit.text.is_empty())
                    {
                        return PanelChange::Repaint;
                    }
                    return match self.commit(region, dpi) {
                        Some(next) => PanelChange::Region(next),
                        None => PanelChange::Repaint,
                    };
                }
                PanelKey::Left | PanelKey::Right | PanelKey::Home | PanelKey::End => {
                    // 数字格里的左右键挪光标（槽间切换交给 Tab）；别的槽不经过这里（那时没有输入格）。
                    if let Some(edit) = self.editing.as_mut() {
                        let len = edit.text.len();
                        edit.caret = match key {
                            PanelKey::Left => edit.caret.saturating_sub(1),
                            PanelKey::Right => (edit.caret + 1).min(len),
                            PanelKey::Home => 0,
                            _ => len,
                        };
                    }
                    return PanelChange::Repaint;
                }
                _ => {}
            }
        }

        match key {
            PanelKey::Escape => PanelChange::Cancel,
            PanelKey::Tab | PanelKey::BackTab => {
                let next = self.focus.step(key == PanelKey::Tab);
                self.focus_slot(next);
                self.begin_edit(region, dpi);
                PanelChange::Repaint
            }
            PanelKey::Left | PanelKey::Right => {
                let forward = key == PanelKey::Right;
                if self.focus == PanelSlot::Delay {
                    let index = Delay::ALL
                        .iter()
                        .position(|delay| *delay == self.delay)
                        .unwrap_or(0);
                    let count = Delay::ALL.len();
                    let next = if forward {
                        (index + 1) % count
                    } else {
                        (index + count - 1) % count
                    };
                    self.delay = Delay::ALL[next];
                    return PanelChange::Repaint;
                }
                if self.focus == PanelSlot::Unit {
                    // 两组二选一的分段控件：方向键在组内换，不把焦点挪走。
                    let next = match self.unit {
                        LengthUnit::Px => LengthUnit::Dp,
                        LengthUnit::Dp => LengthUnit::Px,
                    };
                    self.set_unit(next);
                    return PanelChange::Repaint;
                }
                let next = self.focus.step(forward);
                self.focus_slot(next);
                self.begin_edit(region, dpi);
                PanelChange::Repaint
            }
            PanelKey::Enter => match self.focus {
                PanelSlot::Cancel => PanelChange::Cancel,
                PanelSlot::Preset => {
                    // 从当前命中的那一项开始，而不是永远从第一项：键盘按 Enter 打开再 Enter，
                    // 不该把区域换成 500×500。
                    self.menu = Some(
                        preset_for(region)
                            .and_then(|preset| {
                                PRESETS.iter().position(|item| item.name == preset.name)
                            })
                            .unwrap_or(0),
                    );
                    PanelChange::Repaint
                }
                PanelSlot::Swap => {
                    let next = region.swapped();
                    
                    PanelChange::Region(next)
                }
                PanelSlot::Lock => {
                    self.ratio = match self.ratio {
                        Some(_) => None,
                        None => Some((region.width.max(1), region.height.max(1))),
                    };
                    PanelChange::Repaint
                }
                PanelSlot::Center => {
                    let next = region.centered(self.screen);
                    
                    PanelChange::Region(next)
                }
                PanelSlot::Origin => {
                    let next = region.at_origin(self.screen);
                    
                    PanelChange::Region(next)
                }
                slot => match slot.toggle() {
                    Some(toggle) => PanelChange::Toggle(toggle),
                    None => PanelChange::Confirm(region),
                },
            },
            PanelKey::Digit(_)
            | PanelKey::Backspace
            | PanelKey::Delete
            | PanelKey::Home
            | PanelKey::End
            | PanelKey::Up
            | PanelKey::Down => {
                PanelChange::None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::LabelDisplay;

    /// 4K 屏，桌面原点 (0,0)：150% 下正是 2560×1440 dp 的那块屏。
    fn screen() -> Rect {
        Rect::from_origin_size(Point::new(0, 0), 3840, 2160)
    }

    /// 面板默认值：913×549 dp @150% = 1370×824 px，位置 (1325,632) px。
    fn region() -> Region {
        Region {
            width: 1370,
            height: 824,
            x: 1325,
            y: 632,
        }
    }

    fn panel() -> RegionPanel {
        RegionPanel::new(LengthUnit::Dp, Delay::ALL[0], screen(), 8)
    }

    #[test]
    fn the_four_numbers_read_back_the_selection_in_the_chosen_unit() {
        let origin = Point::new(-1920, 200);
        let rect = Rect::from_origin_size(Point::new(300, 150), 1370, 824);
        let desktop = Region::from_rect(rect, origin);
        // 主屏左边的显示器：负坐标是普通数字。
        assert_eq!(desktop.x, -1620);
        assert_eq!(desktop.y, 350);
        assert_eq!(desktop.shown(RegionField::Width, LengthUnit::Px, 144), 1370);
        assert_eq!(desktop.shown(RegionField::Width, LengthUnit::Dp, 144), 913);
        assert_eq!(desktop.shown(RegionField::X, LengthUnit::Dp, 144), -1080);
        assert_eq!(desktop.to_rect(origin), rect, "换回本屏像素要回到原矩形");
    }

    #[test]
    fn typing_appends_at_the_caret_and_backspace_deletes_one_character() {
        // dpi 96：px 与 dp 同号，数字读起来就是乘积本身，断言不必心算。
        let mut panel = panel();
        let mut region = region();
        panel.click(PanelSlot::Width, region, 96);
        assert_eq!(panel.editing(), Some(RegionField::Width));
        assert_eq!(panel.field_text(RegionField::Width, region, 96), "1370");
        // 聚焦**不再清空**：光标落在末尾（"1370" 是 4 个字符）。
        assert_eq!(panel.caret(), 4, "聚焦后光标应当在末尾");

        // 敲数字是追加，不是替换整格。
        let PanelChange::Region(next) = panel.key(PanelKey::Digit(5), region, 96) else {
            panic!("改宽应当给新区域");
        };
        assert_eq!(next.width, 13705);
        region = next;
        assert_eq!(panel.field_text(RegionField::Width, region, 96), "13705");

        // 退格只删**一个**字符（用户报的 bug：以前一次清空）。
        let PanelChange::Region(next) = panel.key(PanelKey::Backspace, region, 96) else {
            panic!("退格应当给新区域");
        };
        assert_eq!(next.width, 1370, "一次退格只删一位");
        region = next;
        assert_eq!(panel.field_text(RegionField::Width, region, 96), "1370");

        // 一位一位删到底：1370 → 137 → 13 → 1
        for expected in [137, 13, 1] {
            let PanelChange::Region(next) = panel.key(PanelKey::Backspace, region, 96) else {
                panic!("退格应当给新区域");
            };
            assert_eq!(next.width, expected);
            region = next;
        }
        // 删空之后只重画：区域停在最后那个合法的数上；光标已在 0，再按也不动。
        assert_eq!(
            panel.key(PanelKey::Backspace, region, 96),
            PanelChange::Repaint,
            "删空之后只重画"
        );
        assert_eq!(panel.field_text(RegionField::Width, region, 96), "");
        assert_eq!(region.width, 1);
        assert_eq!(panel.caret(), 0);
        assert_eq!(
            panel.key(PanelKey::Backspace, region, 96),
            PanelChange::Repaint,
            "光标在 0 时退格什么都不做"
        );
        assert_eq!(region.width, 1);
    }

    #[test]
    fn the_caret_moves_and_a_digit_lands_where_it_sits() {
        let mut panel = panel();
        let mut region = region();
        panel.click(PanelSlot::Width, region, 96); // 缓冲 "1370"，光标在 4
        panel.key(PanelKey::Home, region, 96);
        assert_eq!(panel.caret(), 0, "Home 移到开头");
        let PanelChange::Region(next) = panel.key(PanelKey::Digit(9), region, 96) else {
            panic!("改宽应当给新区域");
        };
        region = next;
        assert_eq!(panel.field_text(RegionField::Width, region, 96), "91370");
        assert_eq!(region.width, 91370);
        assert_eq!(panel.caret(), 1, "插入之后光标在插入字符右边");
        panel.key(PanelKey::End, region, 96);
        assert_eq!(panel.caret(), 5, "End 移到末尾");
        panel.key(PanelKey::Left, region, 96);
        assert_eq!(panel.caret(), 4);
        panel.key(PanelKey::Right, region, 96);
        assert_eq!(panel.caret(), 5);
        // 光标在末尾：Delete 没有右侧可删。
        panel.key(PanelKey::Delete, region, 96);
        assert_eq!(panel.field_text(RegionField::Width, region, 96), "91370");
        assert_eq!(region.width, 91370);
        // 回到中间再 Delete，删掉右侧那一位。
        panel.key(PanelKey::Home, region, 96);
        panel.key(PanelKey::Right, region, 96);
        panel.key(PanelKey::Delete, region, 96);
        assert_eq!(panel.field_text(RegionField::Width, region, 96), "9370");
    }

    #[test]
    fn the_caret_never_ends_up_outside_the_buffer() {
        // 缓冲会被"提交后的读数"换掉（前导 0 被归一化、单位换算后位数会变），所以光标必须一直
        // clamp 在 0..=len：越界的 caret 会让下一次插入/删除落到错误的位置。
        let mut panel = panel();
        let mut region = region();
        panel.click(PanelSlot::Width, region, 96);
        for key in [
            PanelKey::Digit(0),
            PanelKey::Home,
            PanelKey::Digit(0),
            PanelKey::Digit(0),
            PanelKey::End,
            PanelKey::Backspace,
            PanelKey::Delete,
            PanelKey::Left,
            PanelKey::Left,
        ] {
            let change = panel.key(key, region, 96);
            if let PanelChange::Region(next) = change {
                region = next;
            }
            let text = panel.field_text(RegionField::Width, region, 96);
            assert!(
                panel.caret() <= text.len(),
                "光标 {} 落在缓冲 {:?} 之外",
                panel.caret(),
                text
            );
        }
    }

    /// 把某一格改成指定数字：点进去 → 一位一位退格清空 → 再敲新数字。
    ///
    /// 面板现在是普通文本框语义（聚焦落在末尾、光标可移），所以测试里"直接敲一个数字就替换整格"
    /// 的写法不再成立；清空必须真的逐字符删（docs/23 §14.55）。
    fn retype(
        panel: &mut RegionPanel,
        region: Region,
        slot: PanelSlot,
        digits: &str,
    ) -> Region {
        let mut region = region;
        panel.click(slot, region, 96);
        for _ in 0..8 {
            if let PanelChange::Region(next) = panel.key(PanelKey::Backspace, region, 96) {
                region = next;
            }
        }
        for digit in digits.chars().filter_map(|ch| ch.to_digit(10)) {
            if let PanelChange::Region(next) = panel.key(PanelKey::Digit(digit as u8), region, 96) {
                region = next;
            }
        }
        region
    }

    #[test]
    fn a_locked_ratio_follows_the_edited_side() {
        let mut panel = panel();
        let mut region = region(); // 1370×824
        panel.click(PanelSlot::Lock, region, 96);
        assert!(panel.locked());

        // 改宽：高按锁定那一刻的比例跟着走（1370:824）。
        region = retype(&mut panel, region, PanelSlot::Width, "1");
        assert_eq!((region.width, region.height), (1, 1), "1 宽对应的高不足 1，兜底到 1");

        // 改高：宽按比例回来，宽仍大于高。
        region = retype(&mut panel, region, PanelSlot::Height, "6");
        assert_eq!(region.height, 6);
        assert!(region.width > region.height, "比例是锁定那一刻的");

        // 解锁之后两格各走各的。
        panel.click(PanelSlot::Lock, region, 96);
        assert!(!panel.locked());
        let height = region.height;
        region = retype(&mut panel, region, PanelSlot::Width, "3");
        assert_eq!(region.width, 3);
        assert_eq!(region.height, height, "没锁就不动另一格");
    }

    #[test]
    fn tab_walks_the_panel_in_reading_order_and_wraps() {
        let mut panel = panel();
        let region = region();
        // 默认焦点落在最可能要改的那一格，不一定是 ORDER 的第一项。
        assert_eq!(panel.focus(), PanelSlot::Width);
        let mut seen = vec![panel.focus()];
        for _ in 1..PanelSlot::ORDER.len() {
            panel.key(PanelKey::Tab, region, 96);
            seen.push(panel.focus());
        }
        // 一圈走遍每个槽各一次，顺序就是 ORDER 的轮转。
        let index = PanelSlot::ORDER
            .iter()
            .position(|slot| *slot == PanelSlot::Width)
            .expect("默认焦点在表里");
        let expected: Vec<PanelSlot> = (0..PanelSlot::ORDER.len())
            .map(|step| PanelSlot::ORDER[(index + step) % PanelSlot::ORDER.len()])
            .collect();
        assert_eq!(seen, expected);
        // 走完一圈停在 Unit（表里的第一项），再 Tab 回到起点。
        assert_eq!(panel.focus(), PanelSlot::Unit);
        panel.key(PanelKey::Tab, region, 96);
        assert_eq!(panel.focus(), PanelSlot::Width, "走完一圈回到起点");
        panel.key(PanelKey::BackTab, region, 96);
        assert_eq!(panel.focus(), PanelSlot::Unit, "反向回卷到表尾的那一项");
    }

    #[test]
    fn the_unit_group_switches_with_the_arrows_without_losing_the_focus() {
        let mut panel = panel();
        let region = region();
        panel.click(PanelSlot::Unit, region, 96);
        assert_eq!(panel.focus(), PanelSlot::Unit);
        assert_eq!(panel.unit(), LengthUnit::Dp);
        assert_eq!(panel.key(PanelKey::Right, region, 96), PanelChange::Repaint);
        assert_eq!(panel.unit(), LengthUnit::Px);
        panel.key(PanelKey::Right, region, 96);
        assert_eq!(panel.unit(), LengthUnit::Dp, "两项之间来回，越界回卷");
        panel.key(PanelKey::Left, region, 96);
        assert_eq!(panel.unit(), LengthUnit::Px);
        assert_eq!(panel.focus(), PanelSlot::Unit, "方向键换的是组内选项，不是焦点");
    }

    #[test]
    fn a_field_starts_editing_as_soon_as_the_focus_lands_on_it() {
        let mut panel = panel();
        let region = region();
        // Tab 落到 X：直接就能敲，不必先按一下。
        for _ in 0..4 {
            panel.key(PanelKey::Tab, region, 96);
        }
        assert_eq!(panel.focus(), PanelSlot::X);
        assert_eq!(panel.editing(), Some(RegionField::X));
        assert_eq!(panel.field_text(RegionField::X, region, 96), "1325");
    }

    #[test]
    fn the_delay_group_switches_with_the_arrows() {
        let mut panel = panel();
        let region = region();
        panel.click(PanelSlot::Delay, region, 96);
        assert_eq!(panel.delay().seconds(), 0);
        panel.key(PanelKey::Right, region, 96);
        assert_eq!(panel.delay().seconds(), 3);
        panel.key(PanelKey::Right, region, 96);
        assert_eq!(panel.delay().seconds(), 5);
        panel.key(PanelKey::Left, region, 96);
        assert_eq!(panel.delay().seconds(), 3);
        panel.key(PanelKey::Left, region, 96);
        panel.key(PanelKey::Left, region, 96);
        assert_eq!(panel.delay().seconds(), 10, "从 0 往左回卷到最后一档");
    }

    #[test]
    fn the_dropdown_opens_picks_and_lets_escape_through_first() {
        let mut panel = panel();
        let region = region();
        panel.click(PanelSlot::Preset, region, 96);
        assert_eq!(panel.menu(), Some(0), "点一下展开，键盘停在第一项");
        let PanelChange::Region(picked) = panel.key(PanelKey::Enter, region, 96) else {
            panic!("选中一项要给新区域");
        };
        assert_eq!((picked.width, picked.height), (500, 500));
        assert_eq!(panel.menu(), None, "选完收起");

        // 键盘：Enter 打开、上下键挪、Enter 选中；打开时停在当前命中项上。
        let mut keys = RegionPanel::new(LengthUnit::Dp, Delay::ALL[0], screen(), 8);
        keys.click(PanelSlot::Preset, picked, 96);
        assert_eq!(keys.menu(), Some(0), "当前是 500×500，就从它开始");
        assert_eq!(keys.key(PanelKey::Down, picked, 96), PanelChange::Repaint);
        assert_eq!(keys.menu(), Some(1));
        keys.key(PanelKey::Down, picked, 96);
        assert_eq!(keys.menu(), Some(2), "720p");
        let PanelChange::Region(picked) = keys.key(PanelKey::Enter, picked, 96) else {
            panic!("选中一项要给新区域");
        };
        assert_eq!((picked.width, picked.height), (1280, 720));
        assert_eq!(keys.menu(), None);
        // Esc 先收下拉，再按一次才是取消这次截图。
        panel.click(PanelSlot::Preset, region, 96);
        assert_eq!(panel.key(PanelKey::Escape, region, 96), PanelChange::Repaint);
        assert_eq!(panel.menu(), None);
        assert_eq!(panel.key(PanelKey::Escape, region, 96), PanelChange::Cancel);
    }

    #[test]
    fn a_preset_lands_inside_the_screen() {
        let mut panel = panel();
        // 区域贴在右下角：换 1080p 会放不下，应当被挪进来而不是报错。
        let region = Region {
            width: 500,
            height: 500,
            x: 3840 - 500,
            y: 2160 - 500,
        };
        panel.click(PanelSlot::Preset, region, 96);
        for _ in 0..3 {
            let index = panel.menu().expect("下拉开着");
            panel.menu = Some(index + 1);
        }
        let PanelChange::Region(picked) = panel.key(PanelKey::Enter, region, 96) else {
            panic!("选中一项要给新区域");
        };
        assert_eq!((picked.width, picked.height), (1920, 1080));
        assert_eq!((picked.x, picked.y), (3840 - 1920, 2160 - 1080));
        assert_eq!(panel.problem(picked), None, "挪进来之后就是合法的");
    }

    #[test]
    fn an_off_screen_region_is_refused_with_the_range_to_use() {
        let panel = panel();
        // 1920 宽放在 x=3000：3840 宽的屏放不下。
        let region = Region {
            width: 1920,
            height: 1080,
            x: 3000,
            y: 0,
        };
        let problem = panel.problem(region).expect("放不下应当被拒绝");
        assert_eq!(
            problem,
            RegionProblem::Outside {
                max_width: 3840,
                max_height: 2160
            }
        );
        assert_eq!(problem.fields(), &RegionField::ALL, "尺寸和位置都可能要改");
        assert_eq!(
            problem.message(LengthUnit::Dp, 144),
            "本屏放不下：宽 ≤ 2560、高 ≤ 1440"
        );

        // 太小也一样被拒绝，而且只标尺寸两格。
        let tiny = Region {
            width: 4,
            height: 400,
            x: 0,
            y: 0,
        };
        let problem = panel.problem(tiny).expect("太小应当被拒绝");
        assert_eq!(problem.fields(), &[RegionField::Width, RegionField::Height]);
        assert_eq!(problem.message(LengthUnit::Px, 96), "宽和高要大于 1");
    }

    #[test]
    fn clicking_a_delay_segment_selects_that_segment() {
        let mut panel = panel();
        for delay in Delay::ALL {
            panel.choose_delay(delay);
            assert_eq!(panel.delay(), delay, "点哪段就该选哪段");
        }
        assert_eq!(panel.focus(), PanelSlot::Delay);
    }

    #[test]
    fn a_refused_region_shows_up_in_the_footer_not_only_in_red() {
        let panel = panel();
        let tiny = Region {
            width: 4,
            height: 4,
            x: 0,
            y: 0,
        };
        // 页脚那句由此刻的区域现算，所以这里只断言它算得对。
        assert_eq!(
            panel.status(tiny, 96).as_deref(),
            Some("宽和高要大于 1")
        );
        assert_eq!(panel.status(region(), 96), None);
    }

    #[test]
    fn center_and_origin_place_the_region() {
        let mut panel = panel();
        let region = region();
        let PanelChange::Region(centered) = panel.click(PanelSlot::Center, region, 96) else {
            panic!("居中要给新区域");
        };
        assert_eq!(centered.x, (3840 - region.width) / 2);
        assert_eq!(centered.y, (2160 - region.height) / 2);
        let PanelChange::Region(home) = panel.click(PanelSlot::Origin, centered, 96) else {
            panic!("移到原点要给新区域");
        };
        assert_eq!((home.x, home.y), (0, 0));
    }

    #[test]
    fn the_three_label_toggles_are_the_panels_own_three_bits() {
        let mut panel = panel();
        let region = region();
        let label = LabelDisplay::default();
        assert!(LabelToggle::Size.on(label) && LabelToggle::Position.on(label));
        assert!(LabelToggle::Unit.on(label));
        assert_eq!(
            PanelSlot::ShowPosition.toggle(),
            Some(LabelToggle::Position)
        );
        panel.click(PanelSlot::ShowUnit, region, 96);
        assert_eq!(
            panel.key(PanelKey::Enter, region, 96),
            PanelChange::Toggle(LabelToggle::Unit)
        );
        // 三个开关就是三项，顺序与面板从左到右一致。
        assert_eq!(
            LabelToggle::ALL.map(|toggle| toggle.label()),
            ["尺寸", "位置", "单位"]
        );
    }

    #[test]
    fn enter_confirms_from_a_field_and_cancel_wins_over_everything() {
        let mut panel = panel();
        let region = region();
        panel.click(PanelSlot::Width, region, 96);
        assert_eq!(
            panel.key(PanelKey::Enter, region, 96),
            PanelChange::Confirm(region),
            "Enter 在任何数值格上都是开始截图"
        );
        panel.click(PanelSlot::Cancel, region, 96);
        assert_eq!(panel.key(PanelKey::Enter, region, 96), PanelChange::Cancel);
        assert_eq!(panel.key(PanelKey::Escape, region, 96), PanelChange::Cancel);
    }

    #[test]
    fn switching_the_unit_re_reads_the_same_region() {
        let mut panel = panel();
        let region = region();
        assert_eq!(panel.field_text(RegionField::Width, region, 144), "913");
        panel.set_unit(LengthUnit::Px);
        assert_eq!(panel.field_text(RegionField::Width, region, 144), "1370");
        assert_eq!(panel.unit(), LengthUnit::Px);
        assert_eq!(panel.editing(), None, "换单位丢掉半截输入，避免把旧单位的数字当新单位的");
    }

    #[test]
    fn a_region_on_a_monitor_left_of_the_primary_is_ordinary() {
        // 桌面原点在左屏的时候：
        let screen = Rect::from_origin_size(Point::new(-1920, 0), 1920, 1080);
        let panel = RegionPanel::new(LengthUnit::Dp, Delay::ALL[0], screen, 8);
        let region = Region {
            width: 800,
            height: 600,
            x: -1800,
            y: 100,
        };
        assert_eq!(panel.problem(region), None);
        // 越出这块屏才是问题。
        let outside = Region {
            width: 800,
            height: 600,
            x: 0,
            y: 100,
        };
        assert!(panel.problem(outside).is_some());
    }
}

#[cfg(test)]
mod char_entry_tests {
    use super::{PanelKey, panel_key_from_char};

    fn full_width(digit: u8) -> char {
        char::from_u32(0xFF10 + digit as u32).unwrap()
    }

    #[test]
    fn an_ascii_digit_and_its_full_width_twin_are_the_same_key() {
        // 英文输入法送 '7'，中文输入法（全角）送 '７'——两条路必须落到同一个按键上。
        for digit in 0u8..=9 {
            let ascii = char::from(b'0' + digit);
            assert_eq!(panel_key_from_char(ascii), Some(PanelKey::Digit(digit)));
            assert_eq!(
                panel_key_from_char(full_width(digit)),
                Some(PanelKey::Digit(digit))
            );
        }
    }

    #[test]
    fn anything_that_is_not_a_digit_is_refused() {
        // 负号、小数点、字母、空格、控制字符都不进数字格（数字格的规则没变，见 `RegionPanel::key`）。
        let refused = [
            '-', '.', 'a', 'Z', ' ', '中',
            char::from_u32(8).unwrap(),
            char::from_u32(13).unwrap(),
            char::from_u32(27).unwrap(),
        ];
        for character in refused {
            assert_eq!(panel_key_from_char(character), None, "{character:?}");
        }
    }
}
