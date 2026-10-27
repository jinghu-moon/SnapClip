//! The length unit the overlay's readouts are measured in.
//!
//! A monitor has one physical pixel grid and one logical size: the same region is 1500 physical
//! pixels wide on a 150 % display and only 1000 units wide in the UI toolkit that drew it. Which
//! of the two a readout shows is the user's choice — a pixel is not the same amount of "size" on
//! two displays, so a screenshot's numbers only mean something once the unit is named.
//!
//! Windows calls the logical unit a DIP; the overlay's copy and the settings page call it `dp` —
//! one dp is one physical pixel at 96 dpi, so `dp = px ÷ scale`. The conversion lives here, once,
//! so that the label, the settings page's help text and the tests all mean the same quantity.

use serde::{Deserialize, Serialize};

use crate::geometry::{Point, Rect};

/// What the overlay's size and coordinate readouts are measured in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LengthUnit {
    /// Physical screen pixels: what the capture measures and what an exported image contains.
    Px,
    /// Device-independent pixels (`dp`): the size the same region has in a UI toolkit, which is
    /// what makes a selection on a scaled display comparable with a design.
    Dp,
}

impl LengthUnit {
    /// 两个单位，按分段器里的次序（量宽按它遍历，避免两处各写一份清单）。
    pub const ALL: [Self; 2] = [Self::Px, Self::Dp];

    /// 分段器里的次序：与 `RegionPanelLayout::unit` 的数组顺序一致（动画插值按它编号）。
    pub fn index(self) -> usize {
        match self {
            Self::Px => 0,
            Self::Dp => 1,
        }
    }
}

impl Default for LengthUnit {
    fn default() -> Self {
        Self::Px
    }
}

impl LengthUnit {
    /// The suffix a readout writes after the number.
    /// 分段器里显示的文本：**大写**。
    ///
    /// 与 [`LengthUnit::suffix`] 分开是有意的：角标里是 `913×549 dp`（设计稿的小写），而分段器
    /// 控件里是大写。两者必须各自只有一个来源——分段器的量宽与绘制都读这里，否则量的是小写、
    /// 画的是大写，文字会被挤掉（docs/23 §14.55.7）。
    pub fn segment_label(self) -> &'static str {
        match self {
            Self::Px => "PX",
            Self::Dp => "DP",
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Self::Px => "px",
            Self::Dp => "dp",
        }
    }

    /// `px` (a physical length on a monitor with this DPI) as this unit.
    pub fn from_px(self, px: i32, dpi: u32) -> i32 {
        match self {
            Self::Px => px,
            Self::Dp => px_to_dip(px, dpi),
        }
    }

    /// One of this unit's numbers as physical pixels on a monitor with this DPI.
    ///
    /// The other direction exists because the custom-region panel takes numbers the user typed in
    /// the chosen unit and has to hand the session physical pixels — every rectangle in this crate
    /// is stored in them (docs/23 §14.54).
    pub fn to_px(self, value: i32, dpi: u32) -> i32 {
        match self {
            Self::Px => value,
            Self::Dp => dip_to_px(value, dpi),
        }
    }
}

/// Physical pixels as DIPs at the given monitor DPI, rounded to whole DIPs.
///
/// Halfway values round away from zero. Coordinates are signed — a monitor left of the primary has
/// a negative x — and a readout that rounds the two sides of zero differently is harder to check
/// against a design than a symmetric one.
pub fn px_to_dip(px: i32, dpi: u32) -> i32 {
    // A monitor that reports 0 must never produce a number the user cannot recognise; 96 dpi is
    // the ratio the whole overlay's chrome falls back to as well.
    let dpi = i64::from(dpi.max(96));
    let scaled = i64::from(px) * 96;
    let half = dpi / 2;
    let rounded = if scaled < 0 { scaled - half } else { scaled + half };
    (rounded / dpi) as i32
}

/// How many physical pixels `dip` DIPs are at the given monitor DPI, rounded to whole pixels.
///
/// The exact inverse of [`px_to_dip`], including its halfway rule (away from zero) and its 96-dpi
/// floor, so a number the user typed and a number read back off the screen agree.
pub fn dip_to_px(dip: i32, dpi: u32) -> i32 {
    let dpi = i64::from(dpi.max(96));
    let scaled = i64::from(dip) * dpi;
    let half = 96 / 2;
    let rounded = if scaled < 0 { scaled - half } else { scaled + half };
    (rounded / 96) as i32
}

/// How the overlay's size badge reads: which unit, whether the origin is written, whether the
/// unit's own name is written.
///
/// The three travel together — the badge draws them, the custom-region panel's switches change them
/// and the settings page stores them — so they are one value rather than three flags threaded through
/// every call that reads, draws or changes the badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelDisplay {
    pub length_unit: LengthUnit,
    /// Write the selection's size. The badge's own reason for existing, but still a choice: a
    /// readout that only says *where* is what some users are after.
    pub show_size: bool,
    /// Write the selection's origin after its size.
    pub show_coordinates: bool,
    /// Write the unit's name after the numbers. Off leaves the numbers on their own (`928 × 714`).
    pub show_unit: bool,
}

impl Default for LabelDisplay {
    fn default() -> Self {
        // Pixels first, with all three items: the numbers are then the exported file's own size,
        // place on the desktop and unit, which is what a user can check against the result without
        // knowing their display scale.
        Self {
            length_unit: LengthUnit::Px,
            show_size: true,
            show_coordinates: true,
            show_unit: true,
        }
    }
}

/// 尺寸标上的三项内容，各自独立开关。
///
/// 标签说的就是三件事——多大、在哪、什么单位——所以它们是同一个枚举的三项，而不是三处各自
/// 记账的布尔：设置页、徽标菜单和自定义截图面板改的都是这一份（PixPin 用 3 位掩码存同一件事，
/// docs/28 §2.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelToggle {
    Size,
    Position,
    Unit,
}

impl LabelToggle {
    /// 面板与菜单里的顺序：左到右、上到下。
    pub const ALL: [Self; 3] = [Self::Size, Self::Position, Self::Unit];

    pub fn label(self) -> &'static str {
        match self {
            Self::Size => "尺寸",
            Self::Position => "位置",
            Self::Unit => "单位",
        }
    }

    /// 这一项此刻是否开着。
    pub fn on(self, label: LabelDisplay) -> bool {
        match self {
            Self::Size => label.show_size,
            Self::Position => label.show_coordinates,
            Self::Unit => label.show_unit,
        }
    }
}

impl LabelDisplay {
    /// The badge's whole sentence for `selection`, whose origin is `screen_origin` on the desktop.
    ///
    /// Pure formatting: no DirectWrite, no window, so the words the badge shows are pinned by tests
    /// rather than by a screenshot.
    pub fn text(self, selection: Rect, screen_origin: Point, dpi: u32) -> String {
        let unit = self.length_unit;
        let mut parts: Vec<String> = Vec::new();
        if self.show_size {
            parts.push(format!(
                "{}×{}{}",
                unit.from_px(selection.width(), dpi),
                unit.from_px(selection.height(), dpi),
                if self.show_unit {
                    format!(" {}", unit.suffix())
                } else {
                    String::new()
                },
            ));
        }
        if self.show_coordinates {
            parts.push(format!(
                "({}, {})",
                unit.from_px(selection.left + screen_origin.x, dpi),
                unit.from_px(selection.top + screen_origin.y, dpi),
            ));
        }
        // 三项都关掉就是没有说话可说：空串，调用方据此不画徽标（而不是画一个空盒子）。
        parts.join("  ")
    }

    /// 只切换一项可选标签的开合，其余标签保持原样（docs/23 §14.54）。
    pub fn set(self, toggle: LabelToggle, on: bool) -> Self {
        match toggle {
            LabelToggle::Size => Self {
                show_size: on,
                ..self
            },
            LabelToggle::Position => Self {
                show_coordinates: on,
                ..self
            },
            LabelToggle::Unit => Self {
                show_unit: on,
                ..self
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{dip_to_px, px_to_dip, LabelDisplay, LabelToggle, LengthUnit};
    use crate::geometry::{Point, Rect};

    #[test]
    fn px_to_dip_divides_by_the_scale_the_settings_page_names() {
        // 100 %: dp and px are the same number, which is what makes `Px` the default.
        assert_eq!(px_to_dip(1500, 96), 1500);
        // The values the user's own formula produces at 125 %, 150 % and 200 %.
        assert_eq!(px_to_dip(1500, 120), 1200);
        assert_eq!(px_to_dip(1500, 144), 1000);
        assert_eq!(px_to_dip(1500, 192), 750);
        // Non-exact ratios round to the nearest whole dp instead of truncating.
        assert_eq!(px_to_dip(1000, 144), 667);
        assert_eq!(px_to_dip(1, 192), 1);
        // Degenerate DPI is guarded rather than divided by.
        assert_eq!(px_to_dip(10, 0), 10);
    }

    #[test]
    fn px_to_dip_rounds_both_sides_of_zero_the_same_way() {
        // Coordinates are signed: a monitor left of the primary monitor has a negative x, and
        // `3 px` / `-3 px` must not disagree about which way half a dp rounds.
        assert_eq!(px_to_dip(-3, 192), -2);
        assert_eq!(px_to_dip(3, 192), 2);
        assert_eq!(px_to_dip(-1920, 144), -1280);
        assert_eq!(px_to_dip(-1, 192), -1);
    }

    #[test]
    fn a_whole_number_of_dp_survives_the_round_trip_at_every_scale_windows_uses() {
        // At 125/150/175/200 % a dp is 1.25/1.5/1.75/2 pixels, so a multiple of 4 dp is a whole
        // number of pixels and dividing by the scale has to come back exactly. Otherwise the label
        // and the box it describes would disagree about their own size as the display scale changes.
        for (dpi, percent) in [(96u32, 100i32), (120, 125), (144, 150), (168, 175), (192, 200)] {
            for dp in [4i32, 12, 100, 848] {
                assert_eq!(px_to_dip(dp * percent / 100, dpi), dp, "{dp} dp at {percent} %");
            }
        }
    }

    #[test]
    fn a_unit_converts_the_pixel_value_it_is_given() {
        assert_eq!(LengthUnit::Px.from_px(1920, 144), 1920);
        assert_eq!(LengthUnit::Dp.from_px(1920, 144), 1280);
        assert_eq!(LengthUnit::default(), LengthUnit::Px);
        assert_eq!(LengthUnit::Px.suffix(), "px");
        assert_eq!(LengthUnit::Dp.suffix(), "dp");
        // The settings file spells the unit the way the page does.
        assert_eq!(serde_json::to_string(&LengthUnit::Dp).unwrap(), "\"dp\"");
        assert_eq!(
            serde_json::from_str::<LengthUnit>("\"px\"").unwrap(),
            LengthUnit::Px
        );
    }

    #[test]
    fn the_label_writes_what_the_display_asks_for() {
        let selection = Rect::new(300, 150, 1800, 1150);
        let origin = Point::new(-1920, 0);
        let dp = LabelDisplay {
            length_unit: LengthUnit::Dp,
            show_size: true,
            show_coordinates: true,
            show_unit: true,
        };
        assert_eq!(dp.text(selection, origin, 144), "1000×667 dp  (-1080, 100)");
        // The unit's name is optional; the numbers are not.
        assert_eq!(
            LabelDisplay {
                show_unit: false,
                ..dp
            }
            .text(selection, origin, 144),
            "1000×667  (-1080, 100)"
        );
        // Dropping the origin leaves the size and the unit's name exactly as they were.
        assert_eq!(
            LabelDisplay {
                show_coordinates: false,
                ..dp
            }
            .text(selection, origin, 144),
            "1000×667 dp"
        );
        assert_eq!(
            LabelDisplay {
                show_unit: false,
                show_coordinates: false,
                ..dp
            }
            .text(selection, origin, 144),
            "1000×667"
        );
        // …and the default is physical pixels with both extras.
        assert_eq!(
            LabelDisplay::default().text(Rect::new(0, 0, 1500, 1000), Point::new(0, 0), 96),
            "1500×1000 px  (0, 0)"
        );
    }

    #[test]
    fn a_number_survives_the_round_trip_between_units() {
        // 面板里用户按当前单位敲的数字要变成像素，再读回来还得是同一个数字 —— 否则改一格
        // 就会把另一格也改掉。
        for dpi in [96u32, 120, 144, 192] {
            for value in [-1920, -1, 0, 1, 913, 1370, 3840] {
                let px = LengthUnit::Dp.to_px(value, dpi);
                assert_eq!(
                    LengthUnit::Dp.from_px(px, dpi),
                    value,
                    "dp→px→dp at {dpi} dpi changed {value}"
                );
            }
        }
        // px 是同一把尺子，两个方向都不动。
        assert_eq!(LengthUnit::Px.to_px(1920, 144), 1920);
        assert_eq!(LengthUnit::Px.from_px(1920, 144), 1920);
        // 两个自由函数就是这对换算本身（方法只是按单位分派）。
        assert_eq!(dip_to_px(913, 144), 1370);
        assert_eq!(px_to_dip(1370, 144), 913);
    }

    #[test]
    fn the_badge_can_drop_its_size_and_go_quiet() {
        let selection = Rect::new(300, 150, 1800, 1150);
        let origin = Point::new(-1920, 0);
        let dp = LabelDisplay {
            length_unit: LengthUnit::Dp,
            ..LabelDisplay::default()
        };
        // 只要位置：尺寸标变成一句"在哪"。
        let position_only = LabelDisplay {
            show_size: false,
            ..dp
        };
        assert_eq!(position_only.text(selection, origin, 144), "(-1080, 100)");
        // 只关单位：尺寸还在，名字没了。
        let no_unit = LabelDisplay {
            show_unit: false,
            ..dp
        };
        assert_eq!(no_unit.text(selection, origin, 144), "1000×667  (-1080, 100)");
        // 三项全关：没有话可说，调用方据此不画徽标（而不是画一个空盒子）。
        let quiet = LabelDisplay {
            show_size: false,
            show_coordinates: false,
            ..dp
        };
        assert_eq!(quiet.text(selection, origin, 144), "");
        // 三项是同一份枚举的三项，默认都开着。
        for toggle in LabelToggle::ALL {
            assert!(toggle.on(LabelDisplay::default()), "{:?} 默认开着", toggle);
            assert!(!toggle.label().is_empty());
        }
    }

    #[test]
    fn a_switch_changes_only_the_item_it_names() {
        let start = LabelDisplay::default();

        // 三项开关各写自己那一项，别的都不动。
        let no_origin = start.set(LabelToggle::Position, false);
        assert!(!no_origin.show_coordinates);
        assert_eq!(no_origin.length_unit, start.length_unit);
        assert!(no_origin.show_unit && no_origin.show_size);

        let no_unit = start.set(LabelToggle::Unit, false);
        assert!(!no_unit.show_unit);
        assert!(no_unit.show_coordinates);

        let no_size = start.set(LabelToggle::Size, false);
        assert!(!no_size.show_size);
        assert!(no_size.show_coordinates && no_size.show_unit);

        // 再写回 true 就是原来那一份：开关是可逆的，不留残余。
        assert_eq!(no_unit.set(LabelToggle::Unit, true), start);
        assert_eq!(no_size.set(LabelToggle::Size, true), start);
    }
}
