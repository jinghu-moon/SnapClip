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

    /// 圆角上限：`min(128 px, 短边 / 4)`，与框的绘制、手柄布局、图像遮罩共用同一份
    /// （`geometry::radius_cap`，docs/23 §14.55.21）。
    pub fn radius_max(self) -> i32 {
        crate::geometry::radius_cap(self.width, self.height)
    }
}


impl Region {
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

    /// 以当前宽为基准套一个比例；放不下就**按比例缩小**（比例不许被撑破）。
    ///
    /// 位置不动，最后交给 [`Region::inside`] 挪进屏幕：它只挪不改尺寸，所以缩小后的比例仍然成立。
    pub fn fitted_ratio(self, ratio: Ratio, screen: Rect) -> Self {
        let (rw, rh) = (i64::from(ratio.w.max(1)), i64::from(ratio.h.max(1)));
        let mut width = i64::from(self.width.max(1));
        let mut height = ((width * rh + rw / 2) / rw).max(1);
        let max_w = i64::from(screen.width().max(1));
        let max_h = i64::from(screen.height().max(1));
        if width > max_w || height > max_h {
            // 两轴各自能放大的倍数取小者，再按这个倍数缩 —— 只动宽，高由比例重算。
            let shrink = (max_w as f64 / width as f64).min(max_h as f64 / height as f64);
            width = ((width as f64 * shrink).floor() as i64).clamp(1, max_w);
            height = ((width * rh + rw / 2) / rw).max(1).min(max_h);
        }
        Self {
            width: width as i32,
            height: height as i32,
            ..self
        }
        .inside(screen)
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

/// 一个**比例规格**：面板管的是"框的形状"，不是一个具体像素尺寸。
///
/// 尺寸预设（`500×500` / `480p` …）已经删掉：这个面板的核心是精确设定，用户要一个死数字
/// 时自己敲比翻预设更快；真正跨截图稳定的是**规格**本身（docs/23 §14.55.10 的设计依据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ratio {
    pub name: &'static str,
    pub w: i32,
    pub h: i32,
}

/// 菜单第一项，也是"不锁比例"的唯一表达。
pub const RATIO_FREE_LABEL: &str = "无比例";

/// 圆角上限（像素）：再大也只会把短边切成两半，没有意义（docs/23 §14.55.19）。
pub const RADIUS_MAX_PX: i32 = 128;

/// 菜单里除「无比例」之外的项，顺序就是下拉顺序。
pub const RATIOS: [Ratio; 8] = [
    Ratio { name: "1:1", w: 1, h: 1 },
    Ratio { name: "4:3", w: 4, h: 3 },
    Ratio { name: "3:4", w: 3, h: 4 },
    Ratio { name: "16:9", w: 16, h: 9 },
    Ratio { name: "9:16", w: 9, h: 16 },
    Ratio { name: "3:2", w: 3, h: 2 },
    Ratio { name: "2:3", w: 2, h: 3 },
    Ratio { name: "21:9", w: 21, h: 9 },
];

/// 比例菜单的项数：「无比例」+ [`RATIOS`]。
pub const RATIO_MENU_LEN: usize = RATIOS.len() + 1;

impl Ratio {
    /// 下拉里的第几项：`None`（无比例）是 0，其余按 [`RATIOS`] 的顺序 +1。
    pub fn menu_index(ratio: Option<Ratio>) -> usize {
        ratio
            .and_then(|ratio| RATIOS.iter().position(|item| *item == ratio))
            .map_or(0, |index| index + 1)
    }

    /// 第 `index` 项对应的比例：0 就是「无比例」。
    pub fn at_menu(index: usize) -> Option<Ratio> {
        index
            .checked_sub(1)
            .and_then(|index| RATIOS.get(index).copied())
    }

    /// 显示文案：`无比例` 或比例名。
    pub fn label(ratio: Option<Ratio>) -> &'static str {
        ratio.map_or(RATIO_FREE_LABEL, |ratio| ratio.name)
    }

    /// 交换宽高之后的对应比例。找不到对应项就返回 `None`（例如 `21:9` 没有 `9:21`）。
    pub fn flipped(self) -> Option<Ratio> {
        RATIOS
            .iter()
            .copied()
            .find(|item| item.w == self.h && item.h == self.w)
    }
}

/// 这张截图的落点。默认 `Copy`：与"确定"一直以来的行为一致（复制 + 入历史）。
///
/// `Save` 还没有：保存面板还没做，**选了没反应的选项比没有更糟**（docs/23 §14.55.14）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Copy,
    Pin,
}

impl Destination {
    pub const ALL: [Self; 2] = [Self::Copy, Self::Pin];

    pub fn label(self) -> &'static str {
        match self {
            Self::Copy => "复制",
            Self::Pin => "贴图",
        }
    }

    /// 页脚读数里的动词：`将复制 …` / `将贴图 …`。
    pub fn verb(self) -> &'static str {
        self.label()
    }

    pub fn index(self) -> usize {
        match self {
            Self::Copy => 0,
            Self::Pin => 1,
        }
    }
}

/// 面板上的焦点槽，按 Tab 顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSlot {
    Unit,
    Ratio,
    Swap,
    Width,
    Height,
    X,
    Y,
    Center,
    Origin,
    Destination,
    Delay,
    ShowSize,
    ShowPosition,
    ShowUnit,
    /// 图片卡的卡头（整行是开关）。
    Image,
    /// 圆角滑动条。
    Radius,
    Cancel,
    Start,
}

impl PanelSlot {
    pub const ORDER: [Self; 18] = [
        // 卡头在数字之前（比例按钮与交换按钮都在尺寸卡头里），所以 Tab 从比例开始；
        // 默认焦点另说（见 `RegionPanel::new`）。
        Self::Ratio,
        Self::Swap,
        Self::Width,
        Self::Height,
        Self::X,
        Self::Y,
        Self::Center,
        Self::Origin,
        Self::Destination,
        Self::Delay,
        Self::ShowSize,
        Self::ShowPosition,
        Self::ShowUnit,
        Self::Unit,
        // 图片卡在下面，所以 Tab 在单位之后、页脚之前（docs/23 §14.55.19）。
        Self::Image,
        Self::Radius,
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
    /// 圆角滑块的大步进（±8）。
    PageUp,
    PageDown,
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
    /// 圆角改了（像素）；控制器把它交给成图遮罩。
    Radius(u32),
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
    /// 选中的比例；`None` 表示「无比例」（不锁）。
    ratio: Option<Ratio>,
    /// 这张截图往哪儿去。
    destination: Destination,
    /// 图片卡是否展开（默认收起：面板保持紧凑）。
    image_open: bool,
    /// 成图四角的圆角（像素，0 = 直角）。
    radius_px: u32,
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
            destination: Destination::Copy,
            image_open: false,
            radius_px: 0,
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

    /// 选中的比例（`None` = 无比例）。
    pub fn ratio(&self) -> Option<Ratio> {
        self.ratio
    }

    /// 这张截图的落点。
    pub fn image_open(&self) -> bool {
        self.image_open
    }

    pub fn radius_px(&self) -> u32 {
        self.radius_px
    }

    /// 卡头右侧的现状摘要（收起时显示）：`无圆角` / `圆角 12`（跟随当前单位）。
    pub fn image_summary(&self, dpi: u32) -> String {
        if self.radius_px == 0 {
            return "无圆角".to_string();
        }
        let shown = match self.unit {
            LengthUnit::Px => self.radius_px as i32,
            LengthUnit::Dp => crate::units::px_to_dip(self.radius_px as i32, dpi),
        };
        format!("圆角 {shown}")
    }

    /// 值块里那个数字（跟随当前单位）。
    pub fn radius_text(&self, dpi: u32) -> String {
        let shown = match self.unit {
            LengthUnit::Px => self.radius_px as i32,
            LengthUnit::Dp => crate::units::px_to_dip(self.radius_px as i32, dpi),
        };
        shown.to_string()
    }

    /// 卡头就是开关：展开 / 收起图片卡。
    pub fn toggle_image(&mut self) -> PanelChange {
        self.image_open = !self.image_open;
        PanelChange::Repaint
    }

    /// 圆角：先夹到 `region.radius_max()` 再记下。范围只有这一处，滑动条与键盘都走它。
    pub fn set_radius(&mut self, radius: u32, region: Region) -> PanelChange {
        let clamped = (radius as i32).clamp(0, region.radius_max()) as u32;
        self.radius_px = clamped;
        PanelChange::Radius(clamped)
    }

    /// 圆角滑块上的键盘：←↓ 减、→↑ 加、PgUp/PgDn ±8、Home/End 到两端。
    ///
    /// 只在这个槽上生效；返回 `None` 表示"这个键不归滑块"（Tab、数字、Esc 照常走后面的分支）。
    fn radius_key(&mut self, key: PanelKey, region: Region) -> Option<PanelChange> {
        let max = region.radius_max() as u32;
        let next = match key {
            PanelKey::Left | PanelKey::Down => self.radius_px.saturating_sub(1),
            PanelKey::Right | PanelKey::Up => (self.radius_px + 1).min(max),
            PanelKey::PageUp => (self.radius_px + 8).min(max),
            PanelKey::PageDown => self.radius_px.saturating_sub(8),
            PanelKey::Home => 0,
            PanelKey::End => max,
            _ => return None,
        };
        Some(self.set_radius(next, region))
    }

    pub fn destination(&self) -> Destination {
        self.destination
    }

    /// 页脚读数：这一张会去哪、出多大（**没有校验错误时**显示的那句）。
    pub fn readout(&self, region: Region, dpi: u32) -> String {
        let verb = self.destination.verb();
        match self.unit {
            LengthUnit::Px => format!("将{verb} {}×{} px", region.width, region.height),
            LengthUnit::Dp => format!(
                "将{verb} {}×{} dp → {}×{} px",
                region.shown(RegionField::Width, LengthUnit::Dp, dpi),
                region.shown(RegionField::Height, LengthUnit::Dp, dpi),
                region.width,
                region.height
            ),
        }
    }

    /// 点了去向分段器里的哪一段。
    pub fn choose_destination(&mut self, destination: Destination) -> PanelChange {
        self.focus_slot(PanelSlot::Destination);
        self.destination = destination;
        PanelChange::Repaint
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
            Some(ratio) => region.with_ratio(field, px, (ratio.w, ratio.h)),
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
        self.menu = Some(index.min(RATIO_MENU_LEN - 1));
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
                    let count = RATIO_MENU_LEN;
                    let next = if key == PanelKey::Down {
                        (index + 1) % count
                    } else {
                        (index + count - 1) % count
                    };
                    self.menu = Some(next);
                    PanelChange::Repaint
                }
                PanelKey::Enter => {
                    let picked = Ratio::at_menu(index.min(RATIO_MENU_LEN - 1));
                    self.menu = None;
                    self.ratio = picked;
                    // 选「无比例」只解耦，尺寸不动；选了比例就以**当前宽**为基准套上去，
                    // 放不下时按比例缩小（比例不许被撑破）。
                    let next = match picked {
                        Some(ratio) => region.fitted_ratio(ratio, self.screen),
                        None => region,
                    };
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

        if self.focus == PanelSlot::Radius {
            if let Some(change) = self.radius_key(key, region) {
                return change;
            }
        }

        match key {
            PanelKey::Escape => PanelChange::Cancel,
            PanelKey::Tab | PanelKey::BackTab => {
                let mut next = self.focus.step(key == PanelKey::Tab);
                // 圆角行收起时不存在：Tab 跳过它（不然焦点会落到一个看不见的滑块上）。
                if next == PanelSlot::Radius && !self.image_open {
                    next = next.step(key == PanelKey::Tab);
                }
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
                PanelSlot::Ratio => {
                    // 从当前选中的那一项开始：按 Enter 打开再 Enter，不该把比例换成 1:1。
                    self.menu = Some(Ratio::menu_index(self.ratio));
                    PanelChange::Repaint
                }
                PanelSlot::Image => self.toggle_image(),
                // 滑块上按 Enter 什么都不做：那是"确定"键，不该在调圆角时误提交。
                PanelSlot::Radius => PanelChange::None,
                PanelSlot::Swap => {
                    // 只交换宽高两个数；比例非无比例时一起翻转，免得出现"锁 16:9 却显示 9:16"。
                    // `21:9` 没有 `9:21` 对应项 ⇒ 退回无比例（唯一自洽的选择）。
                    // 越界不自动夹：交给页脚红字，与打字路径一致（不悄悄挪选区）。
                    if let Some(ratio) = self.ratio {
                        self.ratio = ratio.flipped();
                    }
                    let next = region.swapped();
                    PanelChange::Region(next)
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
            | PanelKey::Down
            | PanelKey::PageUp
            | PanelKey::PageDown => PanelChange::None,
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
    fn the_image_summary_follows_the_unit_and_says_when_there_is_none() {
        let mut panel = panel();
        assert_eq!(panel.image_summary(144), "无圆角");
        panel.set_radius(12, region());
        // 12 px @150% ⇒ 8 dp。
        assert_eq!(panel.image_summary(144), "圆角 8");
        assert_eq!(panel.radius_text(144), "8");
        panel.set_unit(LengthUnit::Px);
        assert_eq!(panel.image_summary(144), "圆角 12");
        assert_eq!(panel.radius_text(144), "12");
    }

    #[test]
    fn the_radius_is_clamped_to_the_short_side() {
        let mut panel = panel();
        // 上限是 `min(128, 短边 / 4)`（不是 /2）：45° 的弧点必须留在边把手命中区之外，
        // 否则角把手与边把手会挤在一处（docs/23 §14.55.21）。
        // 1370×824 ⇒ 短边/4 = 206 ⇒ 封顶 128。
        assert_eq!(region().radius_max(), 128);
        panel.set_radius(999, region());
        assert_eq!(panel.radius_px(), 128);
        // 短边先到顶：1000×300 ⇒ 75；100×40 ⇒ 10。
        let wide = Region { width: 1000, height: 300, x: 0, y: 0 };
        assert_eq!(wide.radius_max(), 75);
        let small = Region { width: 100, height: 40, x: 0, y: 0 };
        assert_eq!(small.radius_max(), 10);
        panel.set_radius(999, small);
        assert_eq!(panel.radius_px(), 10);
    }

    #[test]
    fn the_slider_keys_step_and_stop_at_the_ends() {
        let mut panel = panel();
        // 点一下滑块把它聚焦（点数字格以外的地方也一样：`click` 只负责聚焦 + Enter）。
        panel.click(PanelSlot::Radius, region(), 96);
        assert_eq!(panel.focus(), PanelSlot::Radius);
        assert_eq!(panel.key(PanelKey::Enter, region(), 96), PanelChange::None, "滑块不提交");
        assert_eq!(panel.key(PanelKey::Right, region(), 96), PanelChange::Radius(1));
        assert_eq!(panel.key(PanelKey::Up, region(), 96), PanelChange::Radius(2));
        assert_eq!(panel.key(PanelKey::PageUp, region(), 96), PanelChange::Radius(10));
        assert_eq!(panel.key(PanelKey::PageDown, region(), 96), PanelChange::Radius(2));
        assert_eq!(panel.key(PanelKey::Left, region(), 96), PanelChange::Radius(1));
        assert_eq!(panel.key(PanelKey::Down, region(), 96), PanelChange::Radius(0));
        assert_eq!(panel.key(PanelKey::Down, region(), 96), PanelChange::Radius(0), "0 是地板");
        assert_eq!(panel.key(PanelKey::End, region(), 96), PanelChange::Radius(128));
        assert_eq!(panel.key(PanelKey::Home, region(), 96), PanelChange::Radius(0));
    }

    #[test]
    fn tab_skips_the_slider_while_the_card_is_closed() {
        let mut panel = panel();
        panel.click(PanelSlot::Image, region(), 96);
        assert!(panel.image_open(), "点卡头就是打开");
        panel.key(PanelKey::Enter, region(), 96);
        assert!(!panel.image_open(), "再按一次收起");
        panel.key(PanelKey::Tab, region(), 96);
        assert_eq!(panel.focus(), PanelSlot::Cancel, "收起时跳过圆角，直接到取消");
        panel.toggle_image();
        panel.click(PanelSlot::Image, region(), 96);
        panel.toggle_image();
        assert!(panel.image_open());
        panel.key(PanelKey::Tab, region(), 96);
        assert_eq!(panel.focus(), PanelSlot::Radius, "展开后 Tab 落到滑块上");
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

    /// 菜单里按 `Down` 走 `steps` 次再从菜单项取比例（0 是无比例）。
    fn pick_menu(panel: &mut RegionPanel, region: Region, steps: usize) -> Region {
        panel.click(PanelSlot::Ratio, region, 96);
        for _ in 0..steps {
            panel.key(PanelKey::Down, region, 96);
        }
        match panel.key(PanelKey::Enter, region, 96) {
            PanelChange::Region(next) => next,
            other => panic!("选比例要给新区域，拿到 {other:?}"),
        }
    }

    #[test]
    fn a_ratio_follows_the_edited_side_and_free_lets_them_go() {
        let mut panel = panel();
        let mut region = region(); // 1370×824
        assert!(!panel.locked(), "默认是无比例");

        // 16:9：以当前宽为基准算高。
        region = pick_menu(&mut panel, region, 4);
        assert_eq!(panel.ratio().map(|ratio| ratio.name), Some("16:9"));
        assert!(panel.locked());
        assert_eq!((region.width, region.height), (1370, 771), "1370×9/16 四舍五入");

        // 改宽：高跟着比例走。
        region = retype(&mut panel, region, PanelSlot::Width, "800");
        assert_eq!(region.height, 450, "800×9/16");

        // 改高：宽跟着回来。
        region = retype(&mut panel, region, PanelSlot::Height, "900");
        assert_eq!(region.width, 1600, "900×16/9");

        // 选「无比例」只解耦，尺寸不动。菜单打开时停在当前项（16:9 = 第 4 项），
        // 走 5 步回卷到第 0 项。
        region = pick_menu(&mut panel, region, 5);
        assert!(!panel.locked());
        assert_eq!((region.width, region.height), (1600, 900), "解耦不动尺寸");
        let height = region.height;
        region = retype(&mut panel, region, PanelSlot::Width, "300");
        assert_eq!(region.width, 300);
        assert_eq!(region.height, height, "没锁就不动另一格");
    }

    #[test]
    fn swapping_flips_the_ratio_and_falls_back_when_there_is_no_counterpart() {
        // 1:1 换完还是 1:1 —— 按菜单顺序它是第 1 项。
        let mut panel = panel();
        let region = region();
        let region = pick_menu(&mut panel, region, 1);
        let PanelChange::Region(swapped) = panel.click(PanelSlot::Swap, region, 96) else {
            panic!("交换要给新区域");
        };
        assert_eq!((swapped.width, swapped.height), (region.height, region.width));
        assert_eq!(panel.ratio().map(|ratio| ratio.name), Some("1:1"));

        // 16:9 → 9:16。
        let mut other = RegionPanel::new(LengthUnit::Dp, Delay::ALL[0], screen(), 8);
        let start = Region {
            width: 1370,
            height: 824,
            x: 1325,
            y: 632,
        };
        let region = pick_menu(&mut other, start, 4);
        let PanelChange::Region(_) = other.click(PanelSlot::Swap, region, 96) else {
            panic!("交换要给新区域");
        };
        assert_eq!(other.ratio().map(|ratio| ratio.name), Some("9:16"));

        // 21:9 没有 9:21 对应项 ⇒ 退回无比例（唯一自洽的选择）。
        let mut third = RegionPanel::new(LengthUnit::Dp, Delay::ALL[0], screen(), 8);
        let region = pick_menu(&mut third, start, 8);
        assert_eq!(third.ratio().map(|ratio| ratio.name), Some("21:9"));
        let _ = third.click(PanelSlot::Swap, region, 96);
        assert_eq!(third.ratio(), None);
    }

    #[test]
    fn swap_does_not_move_the_region_back_into_the_screen() {
        // 交换只换两个数：越界交给页脚红字，与打字路径一致（不悄悄挪选区）。
        let mut panel = panel();
        let region = Region {
            width: 500,
            height: 500,
            x: 3840 - 500,
            y: 2160 - 500,
        };
        let PanelChange::Region(swapped) = panel.click(PanelSlot::Swap, region, 96) else {
            panic!("交换要给新区域");
        };
        assert_eq!((swapped.x, swapped.y), (region.x, region.y), "位置一毫米都没动");
    }

    #[test]
    fn the_destination_defaults_to_copy_and_drives_the_footer_readout() {
        let mut panel = RegionPanel::new(LengthUnit::Px, Delay::ALL[0], screen(), 8);
        let region = region();
        assert_eq!(panel.destination(), Destination::Copy, "默认与确定一直以来的行为一致");
        assert_eq!(panel.readout(region, 96), "将复制 1370×824 px");

        panel.choose_destination(Destination::Pin);
        assert_eq!(panel.destination(), Destination::Pin);
        assert_eq!(panel.readout(region, 96), "将贴图 1370×824 px");

        // DP 模式把换算写出来（这才是 dp 那一侧的信息量）。
        let dp = RegionPanel::new(LengthUnit::Dp, Delay::ALL[0], screen(), 8);
        assert_eq!(dp.readout(region, 144), "将复制 913×549 dp → 1370×824 px");
    }

    #[test]
    fn the_ratio_button_shows_free_until_a_ratio_is_picked() {
        let mut panel = panel();
        let region = region();
        assert_eq!(Ratio::label(panel.ratio()), RATIO_FREE_LABEL);
        let region = pick_menu(&mut panel, region, 4);
        assert_eq!(Ratio::label(panel.ratio()), "16:9");
        // 菜单里打勾的项跟着走。
        assert_eq!(Ratio::menu_index(panel.ratio()), 4);
        assert_eq!(Ratio::at_menu(4).map(|ratio| ratio.name), Some("16:9"));
        assert_eq!(Ratio::at_menu(0), None, "第 0 项是无比例，没有「0:0」这种伪值");
        assert_eq!(RATIO_MENU_LEN, 9);
        let _ = region;
    }

    #[test]
    fn tab_walks_the_panel_in_reading_order_and_wraps() {
        let mut panel = panel();
        // 圆角行只在图片卡展开时存在，所以这一圈要先把卡打开（收起时 Tab 会跳过它，
        // 那是另一条测试 `tab_skips_the_slider_while_the_card_is_closed` 的事）。
        panel.toggle_image();
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
        // 走完一圈停在「起点 Width 的前一项」，再 Tab 回到起点。
        assert_eq!(panel.focus(), PanelSlot::Swap);
        panel.key(PanelKey::Tab, region, 96);
        assert_eq!(panel.focus(), PanelSlot::Width, "走完一圈回到起点");
        panel.key(PanelKey::BackTab, region, 96);
        assert_eq!(panel.focus(), PanelSlot::Swap, "反向回卷回来的那一项");
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
        // Tab 落到 X：直接就能敲，不必先按一下。走到 X 为止（不写死步数：Tab 顺序会变）。
        while panel.focus() != PanelSlot::X {
            panel.key(PanelKey::Tab, region, 96);
        }
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
    fn the_ratio_menu_opens_picks_and_lets_escape_through_first() {
        let mut panel = panel();
        let region = region();
        panel.click(PanelSlot::Ratio, region, 96);
        assert_eq!(panel.menu(), Some(0), "点一下展开，键盘停在「无比例」上");
        let PanelChange::Region(picked) = panel.key(PanelKey::Enter, region, 96) else {
            panic!("选中一项要给新区域");
        };
        assert_eq!((picked.width, picked.height), (1370, 824), "无比例不动尺寸");
        assert_eq!(panel.menu(), None, "选完收起");

        // 键盘：Enter 打开、上下键挪、Enter 选中；打开时停在**当前**比例上。
        let mut keys = RegionPanel::new(LengthUnit::Dp, Delay::ALL[0], screen(), 8);
        let picked = pick_menu(&mut keys, picked, 4);
        keys.click(PanelSlot::Ratio, picked, 96);
        assert_eq!(keys.menu(), Some(4), "当前是 16:9，就从它开始");
        assert_eq!(keys.key(PanelKey::Up, picked, 96), PanelChange::Repaint);
        assert_eq!(keys.menu(), Some(3), "3:4");
        let PanelChange::Region(picked) = keys.key(PanelKey::Enter, picked, 96) else {
            panic!("选中一项要给新区域");
        };
        assert_eq!(keys.ratio().map(|ratio| ratio.name), Some("3:4"));
        assert!(picked.height > picked.width, "3:4 是竖的");
        assert_eq!(keys.menu(), None);
        // Esc 先收下拉，再按一次才是取消这次截图。
        panel.click(PanelSlot::Ratio, region, 96);
        assert_eq!(panel.key(PanelKey::Escape, region, 96), PanelChange::Repaint);
        assert_eq!(panel.menu(), None);
        assert_eq!(panel.key(PanelKey::Escape, region, 96), PanelChange::Cancel);
    }

    #[test]
    fn a_ratio_shrinks_to_fit_the_screen_and_keeps_its_proportion() {
        let mut panel = panel();
        // 区域贴在右下角且很宽：套 21:9 会放不下，应当**按比例缩小并挪进来**，而不是报错。
        let region = Region {
            width: 3600,
            height: 2000,
            x: 3840 - 3600,
            y: 2160 - 2000,
        };
        let picked = pick_menu(&mut panel, region, 8);
        assert_eq!(panel.ratio().map(|ratio| ratio.name), Some("21:9"));
        assert!(picked.width <= 3840 && picked.height <= 2160, "必须在小屏幕里");
        let lhs = i64::from(picked.width) * 9;
        let rhs = i64::from(picked.height) * 21;
        assert!(
            (lhs - rhs).abs() <= 21,
            "21:9 的比例要保住（{}×{}）",
            picked.width,
            picked.height
        );
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
