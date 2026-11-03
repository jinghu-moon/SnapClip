//! Original-position screenshot geometry, independent of the GPUI renderer.
//! Preserves the prior drag/zoom/raster contracts without CPU resampling or GDI.
const ZOOM_MIN: f64 = 0.1;
const ZOOM_MAX: f64 = 10.;
const WHEEL_DELTA: i32 = 120;
const ZOOM_BOUND_EPSILON: f64 = 1e-12;
const TENTHS_PER_UNIT: f64 = 10.;
#[cfg(test)]
const ZOOM_STEP_TENTHS: i32 = 1;
const BORDER_DIP: i32 = 1;
#[cfg(test)]
pub(super) fn drag_position(
    anchor: (i32, i32),
    cursor_down: (i32, i32),
    cursor_now: (i32, i32),
) -> (i32, i32) {
    (
        anchor.0 + (cursor_now.0 - cursor_down.0),
        anchor.1 + (cursor_now.1 - cursor_down.1),
    )
}

#[cfg(test)]
pub(super) fn zoom_step(current: f64, direction: i32) -> f64 {
    if direction == 0 {
        return clamp_zoom(current);
    }
    let current = clamp_zoom(current);
    let tenths = (current * TENTHS_PER_UNIT).round();
    let next = tenths + f64::from(direction.signum() * ZOOM_STEP_TENTHS);
    clamp_zoom(zoom_from_tenths(next))
}

pub(super) fn zoom_from_tenths(tenths: f64) -> f64 {
    tenths / TENTHS_PER_UNIT
}

pub(super) fn clamp_zoom(zoom: f64) -> f64 {
    if zoom.is_nan() {
        return 1.0;
    }
    if zoom <= 0.0 {
        return ZOOM_MIN;
    }
    let clamped = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    if (clamped - ZOOM_MIN).abs() < ZOOM_BOUND_EPSILON {
        return ZOOM_MIN;
    }
    if (clamped - ZOOM_MAX).abs() < ZOOM_BOUND_EPSILON {
        return ZOOM_MAX;
    }
    clamped
}

pub(super) fn scaled_dimension(source: f64, zoom: f64) -> i32 {
    let scaled = (source * clamp_zoom(zoom)).round().max(1.0);
    // Keep the physical dimension positive even for an extreme zoom request.
    scaled.min(i32::MAX as f64) as i32
}

pub(super) fn window_size(
    image_width: u32,
    image_height: u32,
    zoom: f64,
    border: i32,
) -> (i32, i32) {
    (
        scaled_dimension(image_width as f64, zoom) + border * 2,
        scaled_dimension(image_height as f64, zoom) + border * 2,
    )
}

pub(super) fn image_point_at_client(
    image_width: u32,
    image_height: u32,
    zoom: f64,
    border: i32,
    client: (i32, i32),
) -> Option<(i32, i32)> {
    let zoom = clamp_zoom(zoom);
    let x = ((client.0 - border) as f64 / zoom).floor();
    let y = ((client.1 - border) as f64 / zoom).floor();
    if x < 0.0 || y < 0.0 || x >= f64::from(image_width) || y >= f64::from(image_height) {
        return None;
    }
    Some((x as i32, y as i32))
}

pub(super) fn image_point_to_client(zoom: f64, border: i32, image: (i32, i32)) -> (i32, i32) {
    let zoom = clamp_zoom(zoom);
    (
        (f64::from(image.0) * zoom).round() as i32 + border,
        (f64::from(image.1) * zoom).round() as i32 + border,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn zoom_with_cursor(
    zoom: f64,
    zoom_after: f64,
    position: (i32, i32),
    image_width: u32,
    image_height: u32,
    border: i32,
    cursor: (i32, i32),
) -> (i32, i32) {
    let cursor_client = (cursor.0 - position.0, cursor.1 - position.1);
    let Some(anchor) =
        image_point_at_client(image_width, image_height, zoom, border, cursor_client)
    else {
        return position;
    };
    let after = image_point_to_client(zoom_after, border, anchor);
    (
        position.0 + cursor_client.0 - after.0,
        position.1 + cursor_client.1 - after.1,
    )
}

pub(super) fn wheel_zoom(zoom: f64, remainder: i32, delta: i32) -> (f64, i32, i32) {
    // Platform deltas are untrusted. i64 avoids MIN.abs() overflow and division
    // avoids millions of per-notch iterations for one coalesced input event.
    let accumulated = i64::from(remainder) + i64::from(delta);
    let notches = accumulated / i64::from(WHEEL_DELTA);
    let remainder = (accumulated % i64::from(WHEEL_DELTA)) as i32;
    let zoom = if notches == 0 {
        zoom
    } else {
        zoom_from_tenths((clamp_zoom(zoom) * TENTHS_PER_UNIT).round() + notches as f64)
    };
    (
        if notches == 0 { zoom } else { clamp_zoom(zoom) },
        remainder,
        notches as i32,
    )
}

pub(super) fn border_px(dpi: u32) -> i32 {
    ((BORDER_DIP as f64) * (dpi as f64) / 96.0).round().max(1.0) as i32
}

pub(super) fn badge_label(zoom: f64) -> String {
    let zoom = clamp_zoom(zoom);
    let rounded = zoom.round();
    if zoom >= 10.0 || (zoom - rounded).abs() < 0.01 {
        format!("{rounded:.0}×")
    } else {
        format!("{zoom:.1}×")
    }
}

pub(super) fn scaled_radius(radius: i32, zoom: f64) -> i32 {
    if radius <= 0 {
        return 0;
    }
    (f64::from(radius) * zoom).round().max(0.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wheel_accepts_the_full_negative_platform_delta_without_overflow() {
        let (zoom, remainder, notches) = wheel_zoom(1., 0, i32::MIN);
        assert_eq!(zoom, 0.1);
        assert_eq!(remainder, i32::MIN % 120);
        assert_eq!(notches, i32::MIN / 120);
    }
    #[test]
    fn wheel_accepts_the_full_positive_platform_delta_without_per_notch_work() {
        let (zoom, remainder, notches) = wheel_zoom(1., 0, i32::MAX);
        assert_eq!(zoom, 10.);
        assert_eq!(remainder, i32::MAX % 120);
        assert_eq!(notches, i32::MAX / 120);
    }
    #[test]
    fn opposite_partial_wheel_motion_cancels_without_a_zoom_jump() {
        assert_eq!(wheel_zoom(1., 60, -60), (1., 0, 0));
        assert_eq!(wheel_zoom(1., -30, 29), (1., -1, 0));
    }
    #[test]
    fn a_zoom_limit_consumes_notches_so_reversal_is_immediate() {
        assert_eq!(wheel_zoom(10., 0, 240), (10., 0, 2));
        assert_eq!(wheel_zoom(10., 0, -120), (9.9, 0, -1));
        assert_eq!(wheel_zoom(0.1, 0, -240), (0.1, 0, -2));
        assert_eq!(wheel_zoom(0.1, 0, 120), (0.2, 0, 1));
    }
    #[test]
    fn invalid_nan_zoom_returns_to_one_to_one() {
        assert_eq!(clamp_zoom(f64::NAN), 1.);
    }
    #[test]
    fn nonpositive_zoom_is_the_minimum_including_negative_zero() {
        for z in [-0., 0., -3., f64::NEG_INFINITY] {
            assert_eq!(clamp_zoom(z), 0.1);
        }
    }
    #[test]
    fn positive_infinite_zoom_stops_at_the_upper_limit() {
        assert_eq!(clamp_zoom(f64::INFINITY), 10.);
    }
    #[test]
    fn every_tenth_is_reachable_without_accumulated_float_drift() {
        let mut z = 0.1;
        for tenth in 2..=100 {
            z = zoom_step(z, 1);
            assert_eq!(z, f64::from(tenth) / 10.);
        }
        for tenth in (1..100).rev() {
            z = zoom_step(z, -1);
            assert_eq!(z, f64::from(tenth) / 10.);
        }
    }
    #[test]
    fn source_radius_scales_with_image_zoom_and_never_becomes_negative() {
        assert_eq!(scaled_radius(-10, 10.), 0);
        assert_eq!(scaled_radius(0, 10.), 0);
        assert_eq!(scaled_radius(12, 0.5), 6);
        assert_eq!(scaled_radius(12, 2.), 24);
    }
    #[test]
    fn dpi_changes_only_chrome_and_not_source_image_physical_dimensions() {
        for dpi in [96, 120, 144, 192] {
            for z in [0.1, 0.5, 1., 2.3, 10.] {
                let b = border_px(dpi);
                let (w, h) = window_size(641, 479, z, b);
                assert_eq!(w - 2 * b, scaled_dimension(641., z));
                assert_eq!(h - 2 * b, scaled_dimension(479., z));
            }
        }
    }
    #[test]
    fn tiny_images_keep_one_physical_pixel_at_minimum_zoom() {
        assert_eq!(window_size(1, 1, 0.1, 1), (3, 3));
        assert_eq!(image_point_at_client(1, 1, 1., 1, (1, 1)), Some((0, 0)));
        assert_eq!(image_point_at_client(1, 1, 1., 1, (2, 1)), None);
    }
    #[test]
    fn zero_wheel_motion_does_not_snap_a_valid_fractional_start_zoom() {
        assert_eq!(wheel_zoom(0.375, 0, 0), (0.375, 0, 0));
    }
    #[test]
    fn cursor_anchoring_is_translation_invariant_on_negative_desktops() {
        let a = zoom_with_cursor(1., 1.1, (10, 20), 640, 480, 1, (111, 121));
        let b = zoom_with_cursor(1., 1.1, (-1910, -1060), 640, 480, 1, (-1809, -959));
        assert_eq!((a.0 - 1920, a.1 - 1080), b);
    }
    #[test]
    fn dragging_keeps_the_window_under_the_cursor() {
        // 位置必须等于锚点 + 总位移，而不是任一次中间结果。
        let anchor = (100, 200);
        let down = (500, 500);

        assert_eq!(drag_position(anchor, down, (501, 500)), (101, 200));
        assert_eq!(drag_position(anchor, down, (510, 505)), (110, 205));
        assert_eq!(drag_position(anchor, down, (530, 490)), (130, 190));

        // 同一个光标位置无论问几次都是同一个答案（幂等）：消息重复、合并、
        // 甚至整段丢失都不会留下累积误差。
        for _ in 0..5 {
            assert_eq!(drag_position(anchor, down, (530, 490)), (130, 190));
        }

        // 回到按下时那一点，窗口必须回到原地——这是"跟手"的最直白判据。
        assert_eq!(drag_position(anchor, down, down), anchor);
    }

    #[test]
    fn a_cumulative_drag_would_lose_the_cursor() {
        // 上一条只能证明这个公式对，证明不了「为什么必须给公式一个锚点」。
        // 故障出在消息处理循环里：窗口位置在每条消息之后被**改写**，如果下一
        // 条消息把它当基准再加上位移，误差就一次一次攒起来。这里把那个循环
        // 两种写法各跑一遍，差值就是当初屏幕上看到的现象。
        let anchor = (100, 200);
        let down = (500, 500);
        let path = [(511, 503), (543, 521), (600, 560), (688, 611), (720, 629)];

        let mut anchored = anchor;
        let mut cumulative = anchor;
        for cursor in path {
            anchored = drag_position(anchor, down, cursor);
            // 旧写法：`窗口 = 窗口 + (光标 − 按下点)`，基准是"窗口现在在哪"。
            cumulative = (
                cumulative.0 + (cursor.0 - down.0),
                cumulative.1 + (cursor.1 - down.1),
            );
        }

        // 光标一共只走了 220 像素，锚点写法把它老老实实翻译成窗口位移。
        assert_eq!(anchored, (320, 329));
        // 累加写法同时把每一段位移都重复计入，窗口多走了一个身位。
        assert_eq!(cumulative, (662, 524));
        // 而且差距随拖动距离单调放大：这正是"越拖越不跟手"。
        assert!(cumulative.0 - anchored.0 > 300);
        assert!(
            (cumulative.0 - anchored.0) > (543 - down.0) * 4,
            "拖动越久偏得越多"
        );
    }

    #[test]
    fn a_step_up_then_down_returns_to_the_same_value() {
        let mut slot = 1.0;
        for _ in 0..20 {
            let up = zoom_step(slot, 1);
            let back = zoom_step(up, -1);
            assert_eq!(back, slot, "从 {slot} 放大到 {up} 再缩小应当回到原值");
            slot = up;
        }
    }

    #[test]
    fn a_step_is_one_tenth() {
        assert!((zoom_step(1.0, 1) - 1.1).abs() < 1e-12);
        assert!((zoom_step(1.0, -1) - 0.9).abs() < 1e-12);
        // 100% 是基准，所以 1.0 必须是一个精确的档位。
        assert_eq!(zoom_step(0.9, 1), 1.0);

        let mut zoom = 1.0;
        for _ in 0..10 {
            zoom = zoom_step(zoom, 1);
        }
        assert_eq!(zoom, 2.0, "十档 0.1 就是 +1.0");
    }

    #[test]
    fn zoom_stops_at_its_bounds() {
        // 一直往上滚到底就是 ZOOM_MAX，再滚不动了。
        let mut high = 1.0;
        for _ in 0..500 {
            high = zoom_step(high, 1);
        }
        assert_eq!(high, ZOOM_MAX);
        let mut low = 1.0;
        for _ in 0..500 {
            low = zoom_step(low, -1);
        }
        assert_eq!(low, ZOOM_MIN);
        // `0` 档是恒等：滚轮没凑够一档时走的正是这条路。
        assert_eq!(zoom_step(2.5, 0), 2.5);
    }

    #[test]
    fn a_high_precision_wheel_accumulates_until_a_whole_notch() {
        let mut zoom = 1.0;
        let mut remainder = 0;
        for _ in 0..3 {
            let (next, rest, notches) = wheel_zoom(zoom, remainder, 30);
            zoom = next;
            remainder = rest;
            assert_eq!(notches, 0, "还没凑够一档就不该动");
            assert_eq!(zoom, 1.0);
        }
        assert_eq!(remainder, 90);
        let (next, rest, notches) = wheel_zoom(zoom, remainder, 30);
        assert_eq!(notches, 1);
        assert_eq!(rest, 0);
        assert!((next - 1.1).abs() < 1e-12, "实际 {next}");
    }

    #[test]
    fn one_wheel_message_can_carry_several_notches() {
        // 普通鼠标一次滚三格：`zDelta = 360`，必须走三档而不是一档。
        let (zoom, remainder, notches) = wheel_zoom(1.0, 0, 3 * WHEEL_DELTA);
        assert_eq!(notches, 3);
        assert_eq!(remainder, 0);
        let expected = 1.3;
        assert!((zoom - expected).abs() < 1e-12, "实际 {zoom}");
        // 反向同理。
        let (down, _, notches) = wheel_zoom(1.0, 0, -3 * WHEEL_DELTA);
        assert_eq!(notches, -3);
        assert!((down - 0.7).abs() < 1e-12, "实际 {down}");
    }

    #[test]
    fn zooming_keeps_the_pixel_under_the_cursor_still() {
        let (image_width, image_height) = (100u32, 80u32);
        let border = 1;
        let size = window_size(image_width, image_height, 1.0, border);
        assert_eq!(size, (102, 82));
        let position = (300, 200);
        // 光标压在窗口里 (40, 30) 处，也就是原图 (39, 29) 那个像素上。
        let cursor = (position.0 + 40, position.1 + 30);
        let anchor_before = image_point_at_client(image_width, image_height, 1.0, border, (40, 30));
        assert_eq!(anchor_before, Some((39, 29)));

        let zoom_after = 3.0;
        let moved = zoom_with_cursor(
            1.0,
            zoom_after,
            position,
            image_width,
            image_height,
            border,
            cursor,
        );
        // 放大 3 倍：窗口左上角要往左上退，好让那个像素留在光标底下。
        let cursor_client_after = (cursor.0 - moved.0, cursor.1 - moved.1);
        let anchor_after = image_point_at_client(
            image_width,
            image_height,
            zoom_after,
            border,
            cursor_client_after,
        );
        assert_eq!(
            anchor_after, anchor_before,
            "光标底下的原图像素在缩放前后必须是同一个"
        );
    }

    #[test]
    fn zooming_around_a_border_cursor_leaves_the_window_where_it_was() {
        // 光标在边框上，没有可以钉住的像素，退化成"左上角不动"。
        let position = (100, 100);
        let cursor = (100, 100);
        let moved = zoom_with_cursor(1.0, 2.0, position, 50, 50, 1, cursor);
        assert_eq!(moved, position);
    }

    #[test]
    fn image_point_outside_the_image_is_none() {
        // 边框上、图像外都不该给出锚点。
        assert_eq!(image_point_at_client(10, 10, 1.0, 1, (0, 5)), None);
        assert_eq!(image_point_at_client(10, 10, 1.0, 1, (5, 0)), None);
        assert_eq!(image_point_at_client(10, 10, 1.0, 1, (11, 5)), None);
        assert_eq!(image_point_at_client(10, 10, 1.0, 1, (5, 11)), None);
        assert_eq!(image_point_at_client(10, 10, 1.0, 1, (5, 5)), Some((4, 4)));
    }

    #[test]
    fn scaled_dimension_never_collapses_or_overflows() {
        // 无论怎么缩，一个方向上至少留 1 个像素——0 会让 `LayeredBuffer::new` 报错。
        assert_eq!(scaled_dimension(1.0, ZOOM_MIN), 1);
        assert_eq!(scaled_dimension(0.0, 1.0), 1);
        // 4K 图放到 10 倍是 38400，离 `i32` 还差得远——这条断言留在这里是为了钉住
        // "算式不能回绕"，而不是说这个窗口会被建出来：真正的上限是内存（38400² × 4
        // 字节的 DIB 不可能分配）。
        assert_eq!(scaled_dimension(3840.0, ZOOM_MAX), 38_400);
        // 真到溢出才夹住：`i32::MAX` 之上不许回绕成负数。
        assert_eq!(scaled_dimension(1.0e9, ZOOM_MAX), i32::MAX);
        // 正常情况按四舍五入。
        assert_eq!(scaled_dimension(10.0, 1.1), 11);
        assert_eq!(scaled_dimension(100.0, 1.5), 150);
    }

    #[test]
    fn window_size_is_image_times_zoom_plus_border() {
        assert_eq!(window_size(100, 50, 1.0, 1), (102, 52));
        assert_eq!(window_size(100, 50, 2.0, 1), (202, 102));
        assert_eq!(window_size(100, 50, 0.5, 1), (52, 27));
    }

    #[test]
    fn border_is_one_dip_rounded_with_a_floor_of_one_pixel() {
        // 1 DIP 折成像素：100% 是 1、150% 是 1.5 → 2、200% 是 2。
        assert_eq!(border_px(96), 1);
        assert_eq!(border_px(144), 2);
        assert_eq!(border_px(192), 2);
        // 低于 100% 的 DPI 会折出 0.5 及以下，但边框永远至少 1 像素：0 宽的边框等于
        // 图像直接糊到窗口边缘，那圈"贴图与底下的窗口分开"的标记就没了。
        assert_eq!(border_px(48), 1);
        assert!(border_px(0) >= 1);
    }

    #[test]
    fn the_badge_spells_the_zoom_the_way_the_magnifier_does() {
        // 整档不写小数。
        assert_eq!(badge_label(1.0), "1×");
        assert_eq!(badge_label(2.0), "2×");
        assert_eq!(badge_label(10.0), "10×");
        // 半档写一位。
        assert_eq!(badge_label(0.5), "0.5×");
        assert_eq!(badge_label(1.9), "1.9×");
        assert_eq!(badge_label(0.1), "0.1×");
        // 步长 0.1，所以一位小数永远够用；脏值显示钳过之后的。
        assert_eq!(badge_label(f64::NAN), "1×");
        assert_eq!(badge_label(99.0), "10×");
        // `0` 与负数不是"很小的倍数"，是"这不是一个倍数"，归到下限。
        assert_eq!(badge_label(0.0), "0.1×");
    }
}
