//! 把「刚确认的那张截图」留给贴图：外壳这边的一个转发句柄。
//!
//! # 这里不存像素
//!
//! 像素存在捕获侧、覆盖层自己的共享槽位里（`OverlayShared::selection`），由
//! `OverlayController::publish_selection` 在**手势提交的那一刻**写入（拖拽抬手或吸附
//! 确认）——那时冻结帧还活着，是这些像素唯一的完整副本。
//!
//! 本模块的 [`PinSource`] 只是把那个槽位接成贴图线程能调用的
//! [`PinHome`]：`take_selection` 时向捕获侧要一次，不复制、不缓存、不留第二份。
//!
//! # 为什么曾经不是这样
//!
//! 第一版把「记住这张图」挂在 [`ArtifactWriter::write`] 的装饰器上，理由是
//! `prepared.bgra` 是这些像素在进程里唯一完整、已校验的副本。位置选错了：`write` 只在
//! 用户按 F9 保存时被调用，而用户的流程是「F5 框选 → F6 贴图」，从不保存，于是槽位永远
//! 是空的，F6 一路回答「还没有可贴的图」。写盘是保存那件事，F6 要的是「刚框出来」这件事。
//!
//! # 只留最近一次
//!
//! 槽位被更新的覆盖掉是正确行为：F6 贴的是「我刚截的那张」，不是「第一张」；旧的被
//! 覆盖时它的 `Arc` 一起释放，所以同时最多只多留一张图的内存。
//!
//! 取走语义见 [`PinHome::take_selection`]：贴出去之后槽位就空了，想再贴一次要重新截。
//!
//! # 拼接长图被跳过
//!
//! 滚动长图的 `origin` 是 `None`（见 `SelectionPixels::origin`），意思是「这块像素不
//! 属于屏幕上的任何位置」。没有位置就没地方把它钉回去，所以它不进槽位，F6 会说「还没有
//! 可贴的图」——这是实话。顺带也躲开了那张图的内存：`1920×300,000` 的 `bgra` 有 2.15 GiB。
//!
//! 这条规则由捕获侧的 `publish_selection` 统一执行（它调用的 `prepare_selection` 会回填
//! `origin`），本模块不再自己判断一次。
//!
//! # 曾经用 `(0, 0)` 当哨兵，把主显示器上的选区全丢了
//!
//! 第一版把「没有位置」写成 `origin == (0, 0)`。主显示器的原点**就是** `(0, 0)`——它是
//! 最常见的那块屏——于是每一次在主屏上框选的图都被当成拼接长图丢掉，F6 永远回答「还没有
//! 可贴的图」。`Option` 是唯一能区分「没有位置」和「位置就是原点」的表示。

use std::sync::{Arc, Mutex};

use snapclip_capture::artifact::SelectionPixels;

use crate::pin::{PinHome, PinSelection};

/// 捕获侧「刚提交的选区」槽位，贴图只需要这一个能力。
///
/// 定义在外壳而不是 [`crate::pin`]：这是捕获那边欠贴图的**一个**方法，不是贴图对捕获的
/// 一般依赖；把这个接口缩小到一个方法，才能让 `pin` 模块完全不认识 `snapclip-capture`。
pub trait TakeSelection: Send + Sync + 'static {
    /// 取走最近一次提交的选区；没有就返回 `None`。
    fn take_selection(&self) -> Option<Arc<SelectionPixels>>;
}

/// 贴图线程看到的「刚截的那张图」。
///
/// 被接上之前（`set_home` 还没调用）`take_selection` 诚实地返回 `None`：不知道去哪儿
/// 取，就等于没有图，而不是猜一张。
#[derive(Default)]
pub struct PinSource {
    home: Mutex<Option<Arc<dyn TakeSelection>>>,
}

impl PinSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// 把捕获运行时接上。外壳在拿到 [`CaptureRuntime`] 之后调用一次。
    pub fn set_home(&self, home: Arc<dyn TakeSelection>) {
        *self.home.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(home);
    }
}

impl PinHome for PinSource {
    fn take_selection(&self) -> Option<PinSelection> {
        let home = self
            .home
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()?;
        let selection = home.take_selection()?;
        // 屏幕上没有位置（拼接长图）——没有地方钉，也就不该贴。这是 `None` 而不是
        // `(0, 0)`：主显示器原点 `(0, 0)` 是一个真实的位置，两者必须能区分开。
        let region = selection.screen_region()?;
        if region.is_empty() {
            return None;
        }
        // 这里是唯一一次拷贝：`PinSelection` 要的是自己那份 `Arc<Vec<u8>>`（贴图窗口要
        // 独立的生命周期，不能用借来的切片，而 `SelectionPixels` 会随捕获侧的槽位一起走）。
        Some(PinSelection {
            region,
            pixels: Arc::new(selection.bgra.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapclip_model::geometry::{Point, Rect};

    /// 捕获侧槽位的替身：一个可读写的 `Arc<SelectionPixels>`。
    #[derive(Default)]
    struct FakeHome {
        slot: Mutex<Option<Arc<SelectionPixels>>>,
    }

    impl FakeHome {
        fn put(&self, prepared: SelectionPixels) {
            *self.slot.lock().unwrap() = Some(Arc::new(prepared));
        }
    }

    impl TakeSelection for FakeHome {
        fn take_selection(&self) -> Option<Arc<SelectionPixels>> {
            self.slot.lock().unwrap().take()
        }
    }

    fn prepared(origin: Option<Point>, region: Rect) -> SelectionPixels {
        SelectionPixels {
            frame: frame(200, 100),
            region,
            bgra: vec![7; region.width() as usize * region.height() as usize * 4],
            origin,
        }
    }

    #[test]
    fn the_slot_is_empty_until_something_is_published() {
        let source = PinSource::new();
        assert!(source.take_selection().is_none());
    }

    #[test]
    fn without_a_home_there_is_nothing_to_take() {
        // 没接上捕获侧时不该猜一张图出来。
        let source = PinSource::new();
        source.set_home(Arc::new(FakeHome::default()));
        assert!(source.take_selection().is_none());
    }

    #[test]
    fn taking_a_selection_leaves_the_slot_empty() {
        // 取走语义：槽位不是「最近一次」的缓存，而是「还没被贴的那一张」。
        let home = Arc::new(FakeHome::default());
        home.put(prepared(Some(Point::new(1000, 500)), Rect::new(10, 20, 40, 50)));
        let source = PinSource::new();
        source.set_home(home);
        let taken = source.take_selection().expect("槽位里有东西");
        assert_eq!(taken.region, Rect::new(1010, 520, 1040, 550));
        assert!(source.take_selection().is_none(), "第二次不该再拿到同一张");
    }

    #[test]
    fn a_newer_selection_replaces_an_older_one() {
        let home = Arc::new(FakeHome::default());
        let source = PinSource::new();
        source.set_home(home.clone());
        home.put(prepared(Some(Point::new(0, 1)), Rect::new(0, 0, 4, 4)));
        home.put(prepared(Some(Point::new(0, 1)), Rect::new(8, 8, 12, 12)));
        assert_eq!(source.take_selection().unwrap().region, Rect::new(8, 9, 12, 13));
    }

    #[test]
    fn a_stitched_image_has_no_place_on_screen_and_is_not_pinned() {
        // `origin == (0, 0)` 是拼接长图（见 `SelectionPixels::origin` 的说明）。
        // 它必须被跳过，否则 F6 会把一张不属于任何屏幕位置的图钉在 (0, 0)。
        let home = Arc::new(FakeHome::default());
        home.put(prepared(None, Rect::new(0, 0, 4, 4)));
        let source = PinSource::new();
        source.set_home(home);
        assert!(
            source.take_selection().is_none(),
            "没有屏幕位置的长图不该被贴出来"
        );
    }

    #[test]
    fn a_monitor_selection_keeps_its_pixels_and_screen_position() {
        let home = Arc::new(FakeHome::default());
        home.put(prepared(Some(Point::new(1000, 500)), Rect::new(10, 20, 40, 50)));
        let source = PinSource::new();
        source.set_home(home);
        let taken = source.take_selection().expect("显示器选区必须能贴");
        // 屏幕坐标 = 显示器原点 + 帧内选区，见 `SelectionPixels::screen_region`。
        assert_eq!(taken.region, Rect::new(1010, 520, 1040, 550));
        assert_eq!(taken.bytes().len(), 30 * 30 * 4);
    }

    /// 回归：主显示器的原点是 `(0, 0)`，那是一个**真实**的位置。
    ///
    /// 曾经用 `origin == (0, 0)` 表示「没有位置」，于是主屏上的每一次框选都被当成拼接长图
    /// 丢掉，F6 永远回答「还没有可贴的图」。用户的主屏就是 `3840x2160 at (0,0)`，所以这个
    /// 哨兵吃掉的不是边角情况，而是最常见的那一种。
    #[test]
    fn a_selection_at_the_primary_monitor_origin_is_still_pinned() {
        let home = Arc::new(FakeHome::default());
        home.put(prepared(Some(Point::new(0, 0)), Rect::new(10, 20, 40, 50)));
        let source = PinSource::new();
        source.set_home(home);
        let taken = source
            .take_selection()
            .expect("主显示器原点 (0, 0) 是真实位置，必须能贴");
        assert_eq!(taken.region, Rect::new(10, 20, 40, 50));
    }

    fn frame(width: u32, height: u32) -> snapclip_capture::session::CapturedFrame {
        snapclip_capture::session::CapturedFrame {
            width,
            height,
            pixel_format: snapclip_model::PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: 1_700_000_000_000,
            provider: "test",
            origin: Some(Point::new(0, 0)),
        }
    }
}
