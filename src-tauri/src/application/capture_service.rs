//! Capture artifact production — the parts that stay in the composition root.
//!
//! The service itself (validation, crop, atomic write) and its ports now live in
//! `snapclip_capture::artifact`; what remains here is the **PNG encoder**, because it is
//! the one piece that needs `infrastructure::image`, which stays in the shell for the
//! whole of P1 (see the delivery protocol in docs/23 §5). T2.4 moves encoding and
//! writing into `snapclip-history` together, in one step.
//!
//! Callers name the ports through `snapclip_capture::artifact` directly; this module is
//! only the encoder (T1.10 deleted the forwarder that used to live here).

use crate::infrastructure::image;
use snapclip_capture::artifact::ArtifactEncoder;
use snapclip_capture::{CaptureError, CaptureResult};

/// BGRA → PNG encoder backed by [`crate::infrastructure::image`].
pub struct PngArtifactEncoder;

impl ArtifactEncoder for PngArtifactEncoder {
    fn encode_png(&self, width: u32, height: u32, bgra: &[u8]) -> CaptureResult<Vec<u8>> {
        let image = image::Bgra8Image::new(width, height, bgra.to_vec()).ok_or_else(|| {
            CaptureError::EncodeFailed(format!(
                "bgra buffer of {} bytes does not match {width}x{height}",
                bgra.len()
            ))
        })?;
        image::encode_png(&image).map_err(CaptureError::EncodeFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::PngArtifactEncoder;
    use crate::infrastructure::image;
    use snapclip_capture::artifact::{
        ArtifactDir, ArtifactEncoder, CaptureService, PixelSliceSource,
    };
    use snapclip_capture::geometry::Rect;
    use snapclip_capture::session::CapturedFrame;
    use snapclip_capture::{
        CaptureArtifact, CapturePayload, CaptureResult, PixelFormat,
    };
    use std::fs;
    use std::path::PathBuf;

    struct FixedDir(PathBuf);

    impl ArtifactDir for FixedDir {
        fn artifact_dir(&self) -> PathBuf {
            self.0.clone()
        }
    }

    /// Frame source whose pixels encode their own coordinates, so cropping is verifiable.
    struct GridPixels {
        width: i32,
        height: i32,
    }

    impl GridPixels {
        fn new(width: i32, height: i32) -> Self {
            Self { width, height }
        }
    }

    impl PixelSliceSource for GridPixels {
        fn read_bgra(&self, region: Rect) -> CaptureResult<Vec<u8>> {
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

    fn produce<D: ArtifactDir, E: ArtifactEncoder>(
        service: &CaptureService<D, E>,
        session_id: &str,
        frame: &CapturedFrame,
        selection: Rect,
        dpi: u32,
        device_name: Option<String>,
        pixels: &dyn PixelSliceSource,
    ) -> CaptureResult<CaptureArtifact> {
        let prepared = service.prepare_selection(frame, selection, pixels)?;
        service.finish_artifact(session_id, &prepared, dpi, device_name)
    }

    fn test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "snapclip-png-encoder-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// The shell's encoder is the real thing (the crate's own tests use a fake), so this
    /// is where "BGRA in, decodable PNG out" is actually proven.
    #[test]
    fn real_png_encoder_round_trips_the_selection() {
        let dir = test_dir("png");
        let service = CaptureService::new(FixedDir(dir.clone()), PngArtifactEncoder);
        let pixels = GridPixels::new(4, 4);
        let prepared = service
            .prepare_selection(&frame(4, 4), Rect::new(1, 1, 3, 3), &pixels)
            .unwrap();
        assert_eq!(prepared.region, Rect::new(1, 1, 3, 3));
        let png = service.encode_selection(&prepared).unwrap();
        let decoded = image::decode_to_bgra8(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
        assert_eq!(
            decoded.bytes(),
            &[1, 1, 0, 255, 2, 1, 0, 255, 1, 2, 0, 255, 2, 2, 0, 255]
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Section 8.1 of the tasklist: the whole capture pipeline must run with no
    /// clipboard, no database and no OCR model in the process.
    ///
    /// The structural half of that claim (the capture crate depends on none of them) is
    /// enforced by the `cargo tree` gate in T1.9; this test proves the runtime half by
    /// producing a complete, readable artifact without any of them.
    #[test]
    fn capture_pipeline_is_independent_of_clipboard_store_and_ocr() {
        let dir = test_dir("isolated");
        let service = CaptureService::new(FixedDir(dir.clone()), PngArtifactEncoder);
        let pixels = GridPixels::new(32, 24);

        let artifact = produce(
            &service,
            "capture-isolated",
            &frame(32, 24),
            Rect::new(4, 4, 20, 16),
            120,
            None,
            &pixels,
        )
        .unwrap();

        assert_eq!(artifact.session_id, "capture-isolated");
        assert_eq!((artifact.width, artifact.height), (16, 12));
        assert_eq!(artifact.dpi, 120);

        // The artifact is a self-contained file: decode it back from disk.
        let path = artifact.png_path().unwrap();
        let bytes = fs::read(path).unwrap();
        let decoded = image::decode_to_bgra8(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (16, 12));
        // The pixel grid encodes its own coordinates, so cropping is verifiable.
        assert_eq!(&decoded.bytes()[..4], &[4, 4, 0, 255]);
        let _ = fs::remove_dir_all(dir);
    }

    /// The forwarder covers the payload type the store reads back, so keep one test that
    /// names it through this module rather than through `snapclip_capture` directly.
    #[test]
    fn artifact_payload_is_still_a_png_file_path() {
        let artifact = CaptureArtifact {
            session_id: "s".into(),
            width: 1,
            height: 1,
            dpi: 96,
            pixel_format: PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: 0,
            monitor_device_name: None,
            payload: CapturePayload::PngFile {
                path: PathBuf::from("x.png"),
            },
        };
        assert!(artifact.png_path().is_some());
    }
}
