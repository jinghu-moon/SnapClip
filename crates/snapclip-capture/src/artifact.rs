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

use crate::geometry::Rect;
use crate::session::CapturedFrame;
use crate::{CaptureError, CaptureResult};

/// Supplies the pixels of the frozen frame that the overlay is displaying.
///
/// Implementations borrow their frame, so consumers take `&dyn PixelSliceSource` rather
/// than a stored trait object: the capture path never needs to keep one.
pub trait PixelSliceSource {
    /// Read BGRA8 pixels of `region` (monitor-local physical pixels, top-down,
    /// tightly packed) from the frozen frame.
    fn read_bgra(&self, region: Rect) -> CaptureResult<Vec<u8>>;
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
    pub bgra: Vec<u8>,
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
        let bgra = pixels.read_bgra(clipped)?;
        let expected = clipped.width() as usize * clipped.height() as usize * 4;
        if bgra.len() != expected {
            return Err(CaptureError::EncodeFailed(format!(
                "pixel readback returned {} bytes, expected {expected}",
                bgra.len()
            )));
        }
        Ok(SelectionPixels {
            frame: frame.clone(),
            region: clipped,
            bgra,
        })
    }
}

impl Default for CaptureService {
    fn default() -> Self {
        Self::new()
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
    use crate::geometry::Rect;
    use crate::session::CapturedFrame;
    use crate::{CaptureError, CaptureResult};
    use snapclip_model::PixelFormat;
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
        fn read_bgra(&self, region: Rect) -> CaptureResult<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let clipped = region.intersect(Rect::new(0, 0, self.width, self.height));
            let mut bytes = Vec::with_capacity((clipped.area() * 4) as usize);
            for y in clipped.top..clipped.bottom {
                for x in clipped.left..clipped.right {
                    bytes.extend_from_slice(&[x as u8, y as u8, 0, 255]);
                }
            }
            Ok(bytes)
        }
    }

    fn frame(width: u32, height: u32) -> CapturedFrame {
        CapturedFrame {
            width,
            height,
            pixel_format: PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: 1_700_000_000_000,
            provider: "test",
        }
    }

    fn prepared(frame: &CapturedFrame, selection: Rect, pixels: &dyn PixelSliceSource) -> SelectionPixels {
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
        assert_eq!(prepared.bgra.len(), 3 * 3 * 4);
        assert_eq!(&prepared.bgra[..4], &[10, 5, 0, 255]);
        assert_eq!(&prepared.bgra[prepared.bgra.len() - 4..], &[12, 7, 0, 255]);
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
        assert_eq!(prepared.bgra.len(), 300 * 200 * 4);
        assert_eq!(prepared.region.width(), 300);
        assert_eq!(prepared.region.height(), 200);
    }

    #[test]
    fn selection_is_clipped_to_the_frame_before_readback() {
        let pixels = GridPixels::new(100, 50);
        let prepared = prepared(&frame(100, 50), Rect::new(95, 45, 200, 200), &pixels);
        assert_eq!(prepared.region, Rect::new(95, 45, 100, 50));
        assert_eq!(prepared.bgra.len(), 5 * 5 * 4);
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
            fn read_bgra(&self, _region: Rect) -> CaptureResult<Vec<u8>> {
                Ok(vec![0; 4])
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
}
