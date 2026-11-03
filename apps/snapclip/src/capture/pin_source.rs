//! Two-phase handoff of the most recently confirmed screenshot to F6.
//!
//! Capture owns the slot. A claim shares its immutable pixels and prevents duplicate
//! in-flight creation; it does not consume the slot or end the capture session.
//! Only a real Created acknowledgement commits the claim. Dropping it after rejection,
//! asynchronous failure or cancellation releases the reservation and leaves a retry.
//! Completion compares the exact source Arc, so an old window cannot consume a newer
//! selection. Stitched captures have no screen origin and cannot be pinned in place.
use snapclip_capture::artifact::SelectionPixels;
use snapclip_pin::PinSelection;
use std::sync::{Arc, Mutex, Weak};

/// Application-owned seam; neither capture nor pin depends on the other feature.
pub trait SelectionProvider: Send + Sync + 'static {
    fn selection(&self) -> Option<Arc<SelectionPixels>>;
    /// Consume only this selection after successful creation. A replaced source returns false.
    fn consume_selection(&self, expected: &Arc<SelectionPixels>) -> Result<bool, String>;
}

#[derive(Default)]
pub struct PinSource {
    home: Mutex<Option<Arc<dyn SelectionProvider>>>,
    pending: Mutex<Vec<Weak<SelectionPixels>>>,
}
impl PinSource {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_home(&self, home: Arc<dyn SelectionProvider>) {
        *self.home.lock().unwrap_or_else(|e| e.into_inner()) = Some(home);
    }
    pub fn has_pending(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|entry| entry.strong_count() != 0)
    }
    /// Reserve the current screenshot without taking its only retry path away.
    pub fn claim(self: &Arc<Self>) -> Option<SelectionClaim> {
        let home = self
            .home
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        let captured = home.selection()?;
        let region = captured.screen_region()?;
        if region.is_empty() {
            return None;
        }
        let image =
            snapclip_pin::PinImage::from_pixels(captured.source(), captured.pixels.clone())?;
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|entry| entry.strong_count() != 0);
        if pending
            .iter()
            .any(|entry| entry.ptr_eq(&Arc::downgrade(&captured)))
        {
            return None;
        }
        pending.push(Arc::downgrade(&captured));
        Some(SelectionClaim {
            selection: PinSelection::new(region, image).radius(captured.corner_radius),
            captured,
            home,
            source: self.clone(),
        })
    }
}

/// One in-flight creation. Commit on Created; all failure paths drop it.
pub struct SelectionClaim {
    selection: PinSelection,
    captured: Arc<SelectionPixels>,
    home: Arc<dyn SelectionProvider>,
    source: Arc<PinSource>,
}
impl SelectionClaim {
    pub fn selection(&self) -> &PinSelection {
        &self.selection
    }
    pub fn commit(self) -> Result<bool, String> {
        self.home.consume_selection(&self.captured)
    }
}
impl Drop for SelectionClaim {
    fn drop(&mut self) {
        let expected = Arc::downgrade(&self.captured);
        self.source
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|entry| !entry.ptr_eq(&expected));
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

    impl SelectionProvider for FakeHome {
        fn selection(&self) -> Option<Arc<SelectionPixels>> {
            self.slot.lock().unwrap().clone()
        }
        fn consume_selection(&self, expected: &Arc<SelectionPixels>) -> Result<bool, String> {
            let mut slot = self.slot.lock().unwrap();
            if slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, expected))
            {
                *slot = None;
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }

    fn prepared(origin: Option<Point>, region: Rect) -> SelectionPixels {
        SelectionPixels {
            corner_radius: 0,
            frame: frame(200, 100),
            region,
            pixels: snapclip_model::pixels::PixelBuffer::from_tight(
                region.width() as u32,
                region.height() as u32,
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                vec![7; region.width() as usize * region.height() as usize * 4],
            )
            .unwrap()
            .into(),
            origin,
        }
    }

    #[test]
    fn the_slot_is_empty_until_something_is_published() {
        let source = Arc::new(PinSource::new());
        assert!(source.claim().is_none());
    }

    #[test]
    fn without_a_home_there_is_nothing_to_take() {
        // 没接上捕获侧时不该猜一张图出来。
        let source = Arc::new(PinSource::new());
        source.set_home(Arc::new(FakeHome::default()));
        assert!(source.claim().is_none());
    }

    #[test]
    fn taking_a_selection_leaves_the_slot_empty() {
        // 取走语义：槽位不是「最近一次」的缓存，而是「还没被贴的那一张」。
        let home = Arc::new(FakeHome::default());
        home.put(prepared(
            Some(Point::new(1000, 500)),
            Rect::new(10, 20, 40, 50),
        ));
        let source = Arc::new(PinSource::new());
        source.set_home(home);
        let taken = source.claim().expect("槽位里有东西");
        assert_eq!(taken.selection().region(), Rect::new(1010, 520, 1040, 550));
        assert!(source.claim().is_none(), "创建进行中不能重复贴出同一张");
        assert!(taken.commit().unwrap());
        assert!(source.claim().is_none(), "成功后只消费一次");
    }

    #[test]
    fn a_rejected_pin_keeps_the_selection_available_for_retry() {
        let home = Arc::new(FakeHome::default());
        home.put(prepared(Some(Point::new(0, 0)), Rect::new(0, 0, 4, 4)));
        let source = Arc::new(PinSource::new());
        source.set_home(home.clone());
        // Window creation can reject HDR, exhaust its budget or fail asynchronously.
        // Handing it the pixels is not evidence that a window was created.
        let rejected = source.claim().unwrap();
        drop(rejected);
        assert!(
            home.slot.lock().unwrap().is_some(),
            "failed creation must not consume the screenshot"
        );
        assert!(!source.has_pending());
        assert!(
            source.claim().is_some(),
            "retry must obtain the same source"
        );
    }

    #[test]
    fn a_newer_selection_replaces_an_older_one() {
        let home = Arc::new(FakeHome::default());
        let source = Arc::new(PinSource::new());
        source.set_home(home.clone());
        home.put(prepared(Some(Point::new(0, 1)), Rect::new(0, 0, 4, 4)));
        home.put(prepared(Some(Point::new(0, 1)), Rect::new(8, 8, 12, 12)));
        assert_eq!(
            source.claim().unwrap().selection().region(),
            Rect::new(8, 9, 12, 13)
        );
    }

    #[test]
    fn a_stitched_image_has_no_place_on_screen_and_is_not_pinned() {
        // 只有 origin=None 才表示拼接长图，(0, 0) 是有效的屏幕位置。
        // 它必须被跳过，否则 F6 会把一张不属于任何屏幕位置的图钉在 (0, 0)。
        let home = Arc::new(FakeHome::default());
        home.put(prepared(None, Rect::new(0, 0, 4, 4)));
        let source = Arc::new(PinSource::new());
        source.set_home(home);
        assert!(source.claim().is_none(), "没有屏幕位置的长图不该被贴出来");
    }

    #[test]
    fn a_monitor_selection_keeps_its_pixels_and_screen_position() {
        let home = Arc::new(FakeHome::default());
        home.put(prepared(
            Some(Point::new(1000, 500)),
            Rect::new(10, 20, 40, 50),
        ));
        let source = Arc::new(PinSource::new());
        source.set_home(home);
        let taken = source.claim().expect("显示器选区必须能贴");
        // 屏幕坐标 = 显示器原点 + 帧内选区，见 `SelectionPixels::screen_region`。
        assert_eq!(taken.selection().region(), Rect::new(1010, 520, 1040, 550));
        assert_eq!(
            taken.selection().image().pixels().as_bytes().len(),
            30 * 30 * 4
        );
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
        let source = Arc::new(PinSource::new());
        source.set_home(home);
        let taken = source
            .claim()
            .expect("主显示器原点 (0, 0) 是真实位置，必须能贴");
        assert_eq!(taken.selection().region(), Rect::new(10, 20, 40, 50));
    }

    #[test]
    fn hdr_selection_transfers_shared_pixels_and_keeps_generation_and_alpha() {
        use snapclip_model::pixels::{
            AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource,
        };
        let descriptor = PixelDescriptor::new(
            snapclip_model::PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap();
        let bytes = [-0.25_f32, 2.5, 0.5, 0.5]
            .into_iter()
            .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
            .collect();
        let pixels = Arc::new(PixelBuffer::from_tight(1, 1, descriptor, bytes).unwrap());
        let mut frame = frame(1, 1);
        frame.source = PixelSource::new(descriptor, 91);
        let home = Arc::new(FakeHome::default());
        home.put(SelectionPixels {
            frame,
            region: Rect::new(0, 0, 1, 1),
            pixels: pixels.clone(),
            origin: Some(Point::new(-100, 0)),
            corner_radius: 1,
        });
        let source = Arc::new(PinSource::new());
        source.set_home(home);
        let selection = source.claim().unwrap();
        assert_eq!(selection.selection().region(), Rect::new(-100, 0, -99, 1));
        assert!(Arc::ptr_eq(selection.selection().image().pixels(), &pixels));
        assert_eq!(
            selection.selection().image().pixel_source().generation(),
            91
        );
        assert!(selection.selection().image().is_hdr());
        assert!(source.claim().is_none());
        assert!(selection.commit().unwrap());
        assert!(source.claim().is_none());
    }

    #[test]
    fn a_new_selection_can_be_claimed_while_an_older_creation_is_pending() {
        let home = Arc::new(FakeHome::default());
        let source = Arc::new(PinSource::new());
        source.set_home(home.clone());
        home.put(prepared(Some(Point::new(0, 0)), Rect::new(0, 0, 4, 4)));
        let old = source.claim().unwrap();
        home.put(prepared(Some(Point::new(0, 0)), Rect::new(8, 8, 12, 12)));
        let new = source.claim().unwrap();
        assert!(
            !old.commit().unwrap(),
            "old completion cannot consume newer pixels"
        );
        assert!(source.claim().is_none(), "newer creation is still reserved");
        drop(new);
        assert!(source.claim().is_some(), "failed newer creation may retry");
    }

    fn frame(width: u32, height: u32) -> snapclip_capture::session::CapturedFrame {
        snapclip_capture::session::CapturedFrame {
            width,
            height,
            source: snapclip_model::pixels::PixelSource::new(
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                0,
            ),
            captured_at_unix_ms: 1_700_000_000_000,
            provider: "test",
            origin: Some(Point::new(0, 0)),
        }
    }
}
