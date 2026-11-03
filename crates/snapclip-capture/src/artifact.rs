//! The GPU-side half of artifact production (docs/23 T2.4).
//!
//! Takes the frozen frame the overlay is showing, validates the confirmed selection,
//! crops that region out and hands back [`SelectionPixels`]. That is where capture's job
//! ends: turning pixels into a PNG and putting that file somewhere belongs to storage, so
//! the composition root supplies a [`crate::ports::ArtifactWriter`] and this crate never
//! learns what a PNG is.
//!
//! The two halves used to be two methods on one service (`prepare_selection` on the
//! overlay thread, `finish_artifact` on the export worker). Only the first survives here;
//! the second is the writer port, implemented by the shell over `snapclip-history`'s
//! encoder and [`CaptureArtifactStore`]. The split is why the overlay thread still owns
//! the readback: the frozen texture lives on the single-threaded D3D11 immediate context.
//!
//! [`CaptureArtifactStore`]: https://docs.rs/snapclip-history

use crate::geometry::{Point, Rect};
use crate::session::CapturedFrame;
use crate::{CaptureError, CaptureResult};
use snapclip_model::pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelSource};

/// Supplies the pixels of the frozen frame that the overlay is displaying.
///
/// Implementations borrow their frame, so consumers take `&dyn PixelSliceSource` rather
/// than a stored trait object: the capture path never needs to keep one.
pub trait PixelSliceSource {
    /// Immutable definition of the pixels being read, including source generation.
    fn source(&self) -> PixelSource;
    /// Read source pixels, preserving their precision, encoding and alpha mode.
    fn read_pixels(&self, region: Rect) -> CaptureResult<PixelBuffer>;
}

/// The pixels of one validated selection, ready to be handed over.
///
/// Reading them is the only step that touches the frozen frame, and on the WGC path that
/// means the D3D11 immediate context — which the overlay's Direct2D rendering uses too
/// and which is single-threaded. So this is produced on the overlay thread and handed to
/// the export worker, which owns everything slower (encode, write).
#[derive(Debug, Clone)]
pub struct SelectionPixels {
    pub frame: CapturedFrame,
    /// Clipped, non-empty region the pixels cover.
    pub region: Rect,
    /// `region` sized, top-down, tightly packed.
    pub pixels: PixelBuffer,
    /// Where `frame`'s own `(0, 0)` sits on the virtual desktop, in physical pixels.
    ///
    /// `region` is expressed in the frame's coordinates, and a frame is one monitor, so
    /// this is the monitor's origin — the only thing that turns a selection into a place
    /// on screen. Consumers that show the selection *inside* the overlay never need it;
    /// one that puts it back on the desktop (pinning it) cannot work without it.
    ///
    /// `None` means "this picture is not a monitor's frozen frame and has no place on
    /// screen", which is what a stitched scroll image reports. It is **not** the same as
    /// `Some((0, 0))`: the primary monitor's origin is a real place, and an `Option` is
    /// the only way to tell the two apart. A sentinel `(0, 0)` for "unknown" silently
    /// discarded every selection made on the primary monitor — the monitor at the origin
    /// is the common case, so the sentinel broke the common case.
    pub origin: Option<Point>,
    /// 成图四角的圆角半径（`region` 自己的物理像素，0 = 直角）。
    ///
    /// 面板里的圆角是**成图的属性**，必须跟着这块像素一起走：写盘的人用它遮罩，
    /// 贴图的人用它把自己的合成结果（图像 + 边框）遮罩。两者都用同一份覆盖率算法
    /// （`round_corners`），不存在第二套观感。
    pub corner_radius: i32,
}

impl SelectionPixels {
    /// Definition of the selected content, including a corner mask's alpha.
    /// The frozen frame retains its acquisition definition; this derived content
    /// keeps the acquisition generation without relabeling the original frame.
    pub fn source(&self) -> PixelSource {
        PixelSource::new(self.pixels.descriptor(), self.frame.source.generation())
    }

    /// Validate the source/pixel/geometry seam before handing bytes to a consumer.
    pub fn validate(&self) -> CaptureResult<()> {
        let frozen = self.frame.source.descriptor();
        let content = self.pixels.descriptor();
        // A corner mask can introduce straight alpha into opaque pixels. It
        // cannot change channel layout, color encoding or alpha association.
        let alpha_matches = content.alpha() == frozen.alpha()
            || (frozen.alpha() == AlphaMode::Opaque && content.alpha() == AlphaMode::Straight);
        if content.format() != frozen.format()
            || content.encoding() != frozen.encoding()
            || !alpha_matches
            || self.pixels.size() != [self.region.width() as u32, self.region.height() as u32]
            || self.region.is_empty()
        {
            return Err(CaptureError::EncodeFailed(
                "selection pixels do not match their frozen source or geometry".into(),
            ));
        }
        Ok(())
    }

    /// Explicit boundary for consumers still restricted to straight-alpha SDR BGRA8.
    /// This never tone maps, unpremultiplies or quantizes HDR pixels.
    pub fn sdr_bgra(&self) -> CaptureResult<&[u8]> {
        self.validate()?;
        let descriptor = self.pixels.descriptor();
        if descriptor.format() != PixelDescriptor::bgra8().format()
            || descriptor.encoding() != PixelDescriptor::bgra8().encoding()
            || descriptor.alpha() == AlphaMode::Premultiplied
        {
            return Err(CaptureError::EncodeFailed("this consumer requires straight-alpha SDR BGRA8; high-precision source is preserved".into()));
        }
        Ok(self.pixels.as_bytes())
    }
    /// `region` translated back to virtual-desktop physical pixels, or `None` when this
    /// picture has no screen position (see [`SelectionPixels::origin`]).
    pub fn screen_region(&self) -> Option<Rect> {
        let origin = self.origin?;
        Some(Rect::new(
            self.region.left + origin.x,
            self.region.top + origin.y,
            self.region.right + origin.x,
            self.region.bottom + origin.y,
        ))
    }
}

/// Validates a selection and reads it back from the frozen frame.
///
/// Stateless: it holds no encoder and no directory, because neither is capture's to own.
pub struct CaptureService;

impl CaptureService {
    pub fn new() -> Self {
        Self
    }

    /// Validate `selection` and read exactly that region back from the frame.
    ///
    /// The readback is region-sized by construction: a 300x200 selection on a 4K monitor
    /// transfers 300·200·4 bytes, not the monitor (docs/11 §Phase 3).
    pub fn prepare_selection(
        &self,
        frame: &CapturedFrame,
        selection: Rect,
        pixels: &dyn PixelSliceSource,
    ) -> CaptureResult<SelectionPixels> {
        let clipped = validate(frame, selection)?;
        if pixels.source() != frame.source {
            return Err(CaptureError::InvalidState(
                "pixel source differs from the frozen frame generation".into(),
            ));
        }
        let buffer = pixels.read_pixels(clipped)?;
        if pixels.source() != frame.source || buffer.descriptor() != frame.source.descriptor() {
            return Err(CaptureError::InvalidState(
                "pixel source changed or returned a different definition during readback".into(),
            ));
        }
        let prepared = SelectionPixels {
            origin: frame.origin,
            // 面板的圆角由发布路径（overlay::session）填，这里作为产线默认。
            corner_radius: 0,
            frame: frame.clone(),
            region: clipped,
            pixels: buffer,
        };
        prepared.validate()?;
        Ok(prepared)
    }
}

impl Default for CaptureService {
    fn default() -> Self {
        Self::new()
    }
}

/// The most a corner may round: the panel caps it, and this caps it again so a caller that
/// hands over a stale value cannot punch a hole through a narrow selection.
pub const MAX_CORNER_RADIUS_PX: u32 = 128;

/// Round the four corners of a tightly packed BGRA image, leaving the corners transparent.
///
/// Straight alpha: the colour channels are untouched and only the alpha falls off, because
/// the pixels leave here as BGRA bytes (PNG and the clipboard both read them that way). A
/// radius of zero returns before touching a byte, so the no-rounding path is byte-identical
/// to the readback.
///
/// Coverage comes from 4x4 supersampling inside the radius square: the aliasing of a mask
/// this small is exactly what would make a rounded window corner look like a staircase.
pub fn round_corners(bgra: &mut [u8], width: u32, height: u32, radius: u32) {
    visit_corners(width, height, radius, |index, cov| {
        let index = index * 4 + 3;
        bgra[index] = (u16::from(bgra[index]) * u16::from(cov) / 255) as u8;
    });
}

/// Round a source-format buffer without changing its RGB precision or encoding.
/// Associated color channels follow alpha; opaque input becomes straight alpha.
pub fn round_pixel_corners(pixels: PixelBuffer, radius: u32) -> CaptureResult<PixelBuffer> {
    let [width, height] = pixels.size();
    if radius == 0 {
        return Ok(pixels);
    }
    let descriptor = pixels.descriptor();
    let mut bytes = pixels.into_bytes();
    let associated = descriptor.alpha() == AlphaMode::Premultiplied;
    visit_corners(width, height, radius, |index, cov| {
        match descriptor.format() {
            snapclip_model::PixelFormat::Bgra8Unorm => {
                let pixel = &mut bytes[index * 4..][..4];
                for channel in if associated {
                    &mut pixel[..]
                } else {
                    &mut pixel[3..]
                } {
                    *channel = (u16::from(*channel) * u16::from(cov) / 255) as u8;
                }
            }
            snapclip_model::PixelFormat::Rgba16Float => {
                let pixel = &mut bytes[index * 8..][..8];
                let start = if associated { 0 } else { 3 };
                for channel in start..4 {
                    let at = channel * 2;
                    let value = half::f16::from_le_bytes([pixel[at], pixel[at + 1]]).to_f32();
                    pixel[at..at + 2].copy_from_slice(
                        &half::f16::from_f32(value * f32::from(cov) / 255.).to_le_bytes(),
                    );
                }
            }
        }
    });
    let descriptor = PixelDescriptor::new(
        descriptor.format(),
        descriptor.encoding(),
        if descriptor.alpha() == AlphaMode::Opaque {
            AlphaMode::Straight
        } else {
            descriptor.alpha()
        },
    )
    .map_err(|message| CaptureError::EncodeFailed(message.into()))?;
    PixelBuffer::from_tight(width, height, descriptor, bytes)
        .map_err(|message| CaptureError::EncodeFailed(message.into()))
}

fn visit_corners(width: u32, height: u32, radius: u32, mut apply: impl FnMut(usize, u8)) {
    // 与框的绘制、手柄布局共用一个上限（docs/23 §14.55.21）。
    let radius = radius.min(crate::geometry::radius_cap(width as i32, height as i32) as u32);
    if radius == 0 || width == 0 || height == 0 {
        return;
    }
    let r = radius as f32;
    let samples = 4u32;
    let mut coverage = vec![255u8; (radius * radius) as usize];
    for y in 0..radius {
        for x in 0..radius {
            let mut inside = 0u32;
            for sy in 0..samples {
                for sx in 0..samples {
                    // Sub-pixel centre, measured from the corner's outer edge.
                    let px = x as f32 + (sx as f32 + 0.5) / samples as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / samples as f32;
                    // Distance from the corner circle's centre (r, r): inside the round
                    // means inside the quarter disc, not merely inside the corner block.
                    let dx = r - px;
                    let dy = r - py;
                    if dx * dx + dy * dy <= r * r {
                        inside += 1;
                    }
                }
            }
            coverage[(y * radius + x) as usize] = ((inside * 255) / (samples * samples)) as u8;
        }
    }
    for y in 0..radius {
        for x in 0..radius {
            let cov = coverage[(y * radius + x) as usize];
            if cov == 255 {
                continue;
            }
            for (x, y) in [
                (x, y),
                (width - 1 - x, y),
                (x, height - 1 - y),
                (width - 1 - x, height - 1 - y),
            ] {
                apply(y as usize * width as usize + x as usize, cov);
            }
        }
    }
}

/// Validate the selection against the frozen frame, clipping it to the frame.
pub fn validate(frame: &CapturedFrame, selection: Rect) -> CaptureResult<Rect> {
    if selection.is_empty() {
        return Err(CaptureError::InvalidState(
            "selection is empty; nothing to capture".into(),
        ));
    }
    let clipped = selection.intersect(frame.rect());
    if clipped.is_empty() {
        return Err(CaptureError::InvalidState(
            "selection does not overlap the captured monitor".into(),
        ));
    }
    Ok(clipped)
}

#[cfg(test)]
mod tests {
    use super::{CaptureService, PixelSliceSource, SelectionPixels, validate};
    use crate::geometry::{Point, Rect};
    use crate::session::CapturedFrame;
    use crate::{CaptureError, CaptureResult};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Frame source whose pixels encode their own coordinates, so cropping is verifiable.
    struct GridPixels {
        width: i32,
        height: i32,
        reads: AtomicU64,
    }

    impl GridPixels {
        fn new(width: i32, height: i32) -> Self {
            Self {
                width,
                height,
                reads: AtomicU64::new(0),
            }
        }
    }

    impl PixelSliceSource for GridPixels {
        fn source(&self) -> snapclip_model::pixels::PixelSource {
            snapclip_model::pixels::PixelSource::new(
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                0,
            )
        }
        fn read_pixels(&self, region: Rect) -> CaptureResult<snapclip_model::pixels::PixelBuffer> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let clipped = region.intersect(Rect::new(0, 0, self.width, self.height));
            let mut bytes = Vec::with_capacity((clipped.area() * 4) as usize);
            for y in clipped.top..clipped.bottom {
                for x in clipped.left..clipped.right {
                    bytes.extend_from_slice(&[x as u8, y as u8, 0, 255]);
                }
            }
            Ok(snapclip_model::pixels::PixelBuffer::from_tight(
                clipped.width() as u32,
                clipped.height() as u32,
                self.source().descriptor(),
                bytes,
            )
            .unwrap())
        }
    }

    fn frame(width: u32, height: u32) -> CapturedFrame {
        CapturedFrame {
            width,
            height,
            source: snapclip_model::pixels::PixelSource::new(
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                0,
            ),
            captured_at_unix_ms: 1_700_000_000_000,
            provider: "test",
            // A frame is one monitor; its own coordinates start at (0, 0) whatever monitor
            // it came from. These tests never put a selection back on screen, so the
            // primary monitor's origin is the honest value here.
            origin: Some(Point::new(0, 0)),
        }
    }

    fn prepared(
        frame: &CapturedFrame,
        selection: Rect,
        pixels: &dyn PixelSliceSource,
    ) -> SelectionPixels {
        CaptureService::new()
            .prepare_selection(frame, selection, pixels)
            .unwrap()
    }

    #[test]
    fn crops_the_selection_out_of_the_frozen_frame() {
        let pixels = GridPixels::new(100, 50);
        let prepared = prepared(&frame(100, 50), Rect::new(10, 5, 13, 8), &pixels);
        assert_eq!(prepared.region, Rect::new(10, 5, 13, 8));
        // 3x3 pixels, tightly packed, top-down, first pixel is (10, 5).
        assert_eq!(prepared.pixels.as_bytes().len(), 3 * 3 * 4);
        assert_eq!(&prepared.pixels.as_bytes()[..4], &[10, 5, 0, 255]);
        assert_eq!(
            &prepared.pixels.as_bytes()[prepared.pixels.as_bytes().len() - 4..],
            &[12, 7, 0, 255]
        );
        assert_eq!(
            pixels.reads.load(Ordering::Relaxed),
            1,
            "one selection must cost exactly one region readback"
        );
    }

    /// The Phase 3 contract in service form: the bytes read are the selection's, never the
    /// frame's.
    #[test]
    fn prepare_reads_only_the_selection_bytes() {
        let pixels = GridPixels::new(3840, 2160);
        let prepared = prepared(&frame(3840, 2160), Rect::new(100, 100, 400, 300), &pixels);
        assert_eq!(prepared.pixels.as_bytes().len(), 300 * 200 * 4);
        assert_eq!(prepared.region.width(), 300);
        assert_eq!(prepared.region.height(), 200);
    }

    #[test]
    fn selection_is_clipped_to_the_frame_before_readback() {
        let pixels = GridPixels::new(100, 50);
        let prepared = prepared(&frame(100, 50), Rect::new(95, 45, 200, 200), &pixels);
        assert_eq!(prepared.region, Rect::new(95, 45, 100, 50));
        assert_eq!(prepared.pixels.as_bytes().len(), 5 * 5 * 4);
    }

    #[test]
    fn empty_or_offscreen_selections_are_rejected() {
        let pixels = GridPixels::new(100, 50);
        for selection in [Rect::default(), Rect::new(500, 500, 600, 600)] {
            assert!(matches!(
                CaptureService::new().prepare_selection(&frame(100, 50), selection, &pixels),
                Err(CaptureError::InvalidState(_))
            ));
        }
        assert_eq!(
            pixels.reads.load(Ordering::Relaxed),
            0,
            "no readback must happen for a rejected selection"
        );
    }

    #[test]
    fn readback_of_the_wrong_size_is_rejected() {
        struct WrongPixels;
        impl PixelSliceSource for WrongPixels {
            fn source(&self) -> snapclip_model::pixels::PixelSource {
                snapclip_model::pixels::PixelSource::new(
                    snapclip_model::pixels::PixelDescriptor::bgra8(),
                    0,
                )
            }
            fn read_pixels(
                &self,
                _region: Rect,
            ) -> CaptureResult<snapclip_model::pixels::PixelBuffer> {
                Ok(snapclip_model::pixels::PixelBuffer::from_tight(
                    1,
                    1,
                    self.source().descriptor(),
                    vec![0; 4],
                )
                .unwrap())
            }
        }
        let error = CaptureService::new()
            .prepare_selection(&frame(100, 50), Rect::new(0, 0, 10, 10), &WrongPixels)
            .unwrap_err();
        assert!(matches!(error, CaptureError::EncodeFailed(_)));
    }

    #[test]
    fn validate_clips_rather_than_rejecting_a_partly_offscreen_selection() {
        let clipped = validate(&frame(100, 50), Rect::new(-10, -10, 10, 10)).unwrap();
        assert_eq!(clipped, Rect::new(0, 0, 10, 10));
    }
    #[test]
    fn rounding_a_corner_only_touches_the_corner() {
        use super::round_corners;
        let (w, h) = (20u32, 20u32);
        let mut pixels = vec![7u8; (w * h * 4) as usize];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let before = pixels.clone();
        round_corners(&mut pixels, w, h, 0);
        assert_eq!(pixels, before, "半径 0 必须逐字节不变");

        round_corners(&mut pixels, w, h, 6);
        assert_eq!(pixels[3], 0, "左上角整像素落在圆外 ⇒ 全透明");
        let centre = (((h / 2) * w + w / 2) * 4 + 3) as usize;
        assert_eq!(pixels[centre], 255, "中心不透明");
        // 圆弧穿过的那些格子要出现中间 alpha —— 抗锯齿的证据。别赌某一格：扫角块，
        // 只要存在"部分覆盖"就算通过（弧落在哪一像素取决于半径与取整）。
        let mut partial = 0;
        for y in 0..6u32 {
            for x in 0..6u32 {
                let alpha = pixels[((y * w + x) * 4 + 3) as usize];
                if alpha > 0 && alpha < 255 {
                    partial += 1;
                }
            }
        }
        assert!(partial > 0, "圆的边界应有部分覆盖的像素（抗锯齿）");
        // 颜色通道不动（直通 alpha）。
        assert_eq!(pixels[0], 7);
    }

    #[test]
    fn a_radius_wider_than_half_the_short_side_is_clamped() {
        use super::{MAX_CORNER_RADIUS_PX, round_corners};
        // 宽度要够：上限是 `min(128, 短边/4)`，8 宽会把半径压到 2，探针就测不到"圆的边界"了。
        let (w, h) = (24u32, 40u32);
        let mut pixels = vec![255u8; (w * h * 4) as usize];
        round_corners(&mut pixels, w, h, MAX_CORNER_RADIUS_PX);
        let centre = (((h / 2) * w + w / 2) * 4 + 3) as usize;
        assert_eq!(pixels[centre], 255, "夹到短边一半后中心仍在圆内");
    }
}
