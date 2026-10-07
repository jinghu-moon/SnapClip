//! The shell's implementation of capture's `ArtifactWriter` port (docs/23 T2.4, moved here
//! by P6).
//!
//! This is where the two halves of artifact production meet: capture hands over the pixels
//! the user selected, and history owns turning them into a file. Encoding lives in
//! `snapclip-history::image`, the write in `CaptureArtifactStore` — so capture never learns
//! what a PNG is, and history never learns what a selection is.
//!
//! It runs on the export worker thread, never on the overlay thread.

use std::path::PathBuf;

use snapclip_capture::artifact::SelectionPixels;
use snapclip_capture::ports::ArtifactWriter;
use snapclip_capture::{CaptureArtifact, CaptureError, CapturePayload, CaptureResult};
use snapclip_history::artifact_store::CaptureArtifactStore;
use snapclip_history::image::{Bgra8Image, encode_png};
use snapclip_model::{CaptureMetadata, CaptureOutput};

/// Encodes through `snapclip-history` and writes through its artifact store.
pub struct HistoryArtifactWriter {
    store: CaptureArtifactStore,
}

impl HistoryArtifactWriter {
    /// `root` is the directory artifacts land in (`<app local data>/artifacts/capture`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            store: CaptureArtifactStore::new(root),
        }
    }
}

impl ArtifactWriter for HistoryArtifactWriter {
    fn write(
        &self,
        session_id: &str,
        prepared: &SelectionPixels,
        dpi: u32,
        monitor_device_name: Option<String>,
    ) -> CaptureResult<CaptureArtifact> {
        let width = prepared.region.width() as u32;
        let height = prepared.region.height() as u32;
        let image = Bgra8Image::new(width, height, prepared.bgra.clone()).ok_or_else(|| {
            CaptureError::EncodeFailed(format!(
                "bgra buffer of {} bytes does not match {width}x{height}",
                prepared.bgra.len()
            ))
        })?;
        let png = encode_png(&image).map_err(CaptureError::EncodeFailed)?;

        let output = CaptureOutput {
            bytes: png,
            metadata: CaptureMetadata {
                session_id: session_id.to_string(),
                width,
                height,
                dpi,
                pixel_format: prepared.frame.pixel_format,
                captured_at_unix_ms: prepared.frame.captured_at_unix_ms,
                monitor_device_name,
            },
        };
        let reference = self
            .store
            .write(&output)
            .map_err(|error| CaptureError::EncodeFailed(error.to_string()))?;

        Ok(CaptureArtifact {
            session_id: session_id.to_string(),
            width,
            height,
            dpi,
            pixel_format: prepared.frame.pixel_format,
            captured_at_unix_ms: prepared.frame.captured_at_unix_ms,
            monitor_device_name: output.metadata.monitor_device_name.clone(),
            payload: CapturePayload::PngFile {
                path: reference.absolute_path,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::HistoryArtifactWriter;
    use snapclip_capture::artifact::{CaptureService, PixelSliceSource};
    use snapclip_capture::geometry::Rect;
    use snapclip_capture::ports::ArtifactWriter;
    use snapclip_capture::session::CapturedFrame;
    use snapclip_capture::CaptureResult;
    use snapclip_history::image::decode_to_bgra8;
    use snapclip_model::PixelFormat;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "snapclip-app-writer-{name}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    /// Frame source whose pixels encode their own coordinates, so cropping is verifiable.
    struct GridPixels {
        width: i32,
        height: i32,
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

    #[test]
    fn the_writer_hands_the_store_decodable_png_bytes() {
        let dir = root("png");
        let writer = HistoryArtifactWriter::new(dir.clone());
        let prepared = CaptureService::new()
            .prepare_selection(
                &frame(4, 4),
                Rect::new(1, 1, 3, 3),
                &GridPixels { width: 4, height: 4 },
            )
            .unwrap();

        let artifact = writer
            .write("capture-1-1", &prepared, 144, Some(r"\\.\DISPLAY1".into()))
            .unwrap();

        assert_eq!(artifact.session_id, "capture-1-1");
        assert_eq!((artifact.width, artifact.height), (2, 2));
        assert_eq!(artifact.dpi, 144);
        assert_eq!(artifact.monitor_device_name.as_deref(), Some(r"\\.\DISPLAY1"));

        // The file is a real PNG whose pixels are the selection's, in order.
        let path = artifact.png_path().unwrap();
        assert!(path.starts_with(&dir));
        let decoded = decode_to_bgra8(&fs::read(path).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
        assert_eq!(
            decoded.bytes(),
            &[1, 1, 0, 255, 2, 1, 0, 255, 1, 2, 0, 255, 2, 2, 0, 255]
        );
        let _ = fs::remove_dir_all(dir);
    }
}
