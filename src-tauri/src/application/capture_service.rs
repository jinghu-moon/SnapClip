//! Capture artifact production.
//!
//! Takes the frozen frame the overlay was showing, crops the confirmed selection
//! out of it and turns it into a durable [`CaptureArtifact`] — without ever
//! touching the clipboard, the store or OCR.
//!
//! The overlay owns the GPU texture, so it supplies pixels through
//! [`PixelSliceSource`]; the service owns everything after that (validation,
//! encoding, atomic file write). The two halves are deliberately separate methods
//! — [`CaptureService::prepare_selection`] is the only one that may touch the GPU,
//! so it runs on the overlay thread, while [`CaptureService::finish_artifact`] is
//! GPU-free and runs on the export worker.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::infrastructure::image;

use crate::capture::geometry::Rect;
use crate::capture::session::CapturedFrame;
use crate::capture::{CaptureError, CaptureResult};
use crate::domain::{CaptureArtifact, CapturePayload};

/// Supplies the pixels of the frozen frame that the overlay is displaying.
///
/// Implementations borrow their frame, so consumers take `&impl PixelSliceSource`
/// rather than a trait object: the capture path never needs to store one.
pub trait PixelSliceSource {
    /// Read BGRA8 pixels of `region` (monitor-local physical pixels, top-down,
    /// tightly packed) from the frozen frame.
    fn read_bgra(&self, region: Rect) -> CaptureResult<Vec<u8>>;
}

/// Encodes pixels into an artifact payload.
pub trait ArtifactEncoder: Send + Sync + 'static {
    fn encode_png(&self, width: u32, height: u32, bgra: &[u8]) -> CaptureResult<Vec<u8>>;
}

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

/// Directory where capture artifacts may be written. Supplied by the application
/// root so platform code never invents paths.
pub trait ArtifactDir: Send + Sync + 'static {
    fn artifact_dir(&self) -> PathBuf;
}

static ARTIFACT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Produces capture artifacts.
pub struct CaptureService<D, E> {
    artifacts: D,
    encoder: E,
}

/// The pixels of one validated selection, ready to be encoded.
///
/// Reading them is the only step that touches the frozen frame, and on the WGC path
/// that means the D3D11 immediate context — which the overlay's Direct2D rendering
/// uses too and which is single-threaded. So this is produced on the overlay thread
/// and handed to the export worker, which owns everything slower (encode, write).
#[derive(Debug, Clone)]
pub struct SelectionPixels {
    pub frame: CapturedFrame,
    /// Clipped, non-empty region the pixels cover.
    pub region: Rect,
    /// `region` sized, top-down, tightly packed.
    pub bgra: Vec<u8>,
}

impl<D, E> CaptureService<D, E>
where
    D: ArtifactDir,
    E: ArtifactEncoder,
{
    pub fn new(artifacts: D, encoder: E) -> Self {
        Self {
            artifacts,
            encoder,
        }
    }

    /// Validate `selection` and read exactly that region back from the frame.
    ///
    /// The readback is region-sized by construction: a 300x200 selection on a 4K
    /// monitor transfers 300·200·4 bytes, not the monitor (docs/11 §Phase 3).
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

    /// Encode prepared pixels and write them atomically.
    ///
    /// Deliberately free of any GPU or frame reference so it can run on the export
    /// worker while the overlay keeps pumping messages.
    pub fn finish_artifact(
        &self,
        session_id: &str,
        prepared: &SelectionPixels,
        dpi: u32,
        monitor_device_name: Option<String>,
    ) -> CaptureResult<CaptureArtifact> {
        let width = prepared.region.width() as u32;
        let height = prepared.region.height() as u32;
        let png = self.encoder.encode_png(width, height, &prepared.bgra)?;
        let path = self.write_artifact(session_id, &png)?;
        Ok(CaptureArtifact {
            session_id: session_id.to_string(),
            width,
            height,
            dpi,
            pixel_format: prepared.frame.pixel_format,
            captured_at_unix_ms: prepared.frame.captured_at_unix_ms,
            monitor_device_name,
            payload: CapturePayload::PngFile { path },
        })
    }

    /// Encode a prepared selection into PNG bytes without touching the filesystem.
    pub fn encode_selection(&self, prepared: &SelectionPixels) -> CaptureResult<Vec<u8>> {
        self.encoder.encode_png(
            prepared.region.width() as u32,
            prepared.region.height() as u32,
            &prepared.bgra,
        )
    }

    /// Write the PNG atomically into the artifact directory.
    pub fn write_artifact(&self, session_id: &str, png: &[u8]) -> CaptureResult<PathBuf> {
        let directory = self.artifacts.artifact_dir();
        fs::create_dir_all(&directory).map_err(|error| {
            CaptureError::EncodeFailed(format!("create artifact directory failed: {error}"))
        })?;
        let sequence = ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("{session_id}-{sequence}.png"));
        write_atomically(&path, png).map_err(|error| {
            CaptureError::EncodeFailed(format!("write artifact failed: {error}"))
        })?;
        Ok(path)
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

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("png.tmp");
    fs::write(&temporary, bytes)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::geometry::Rect;
    use crate::domain::PixelFormat;

    struct FixedDir(PathBuf);

    impl ArtifactDir for FixedDir {
        fn artifact_dir(&self) -> PathBuf {
            self.0.clone()
        }
    }

    struct CountingEncoder {
        calls: AtomicU64,
    }

    impl ArtifactEncoder for CountingEncoder {
        fn encode_png(&self, width: u32, height: u32, bgra: &[u8]) -> CaptureResult<Vec<u8>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            // Deterministic fake PNG: header + dimensions + first pixel.
            let mut png = vec![0x89, b'P', b'N', b'G'];
            png.extend_from_slice(&width.to_le_bytes());
            png.extend_from_slice(&height.to_le_bytes());
            png.extend_from_slice(&bgra[..4.min(bgra.len())]);
            Ok(png)
        }
    }

    /// Frame source whose pixels encode their own coordinates, so cropping is
    /// verifiable.
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

    /// The two pipeline halves run back to back, as the export worker does.
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
            "snapclip-capture-service-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn crops_the_selection_out_of_the_frozen_frame() {
        let dir = test_dir("crop");
        let service = CaptureService::new(
            FixedDir(dir.clone()),
            CountingEncoder {
                calls: AtomicU64::new(0),
            },
        );
        let pixels = GridPixels::new(100, 50);
        let prepared = service
            .prepare_selection(&frame(100, 50), Rect::new(10, 5, 13, 8), &pixels)
            .unwrap();
        assert_eq!(prepared.region, Rect::new(10, 5, 13, 8));
        let png = service.encode_selection(&prepared).unwrap();
        // 3x3 pixels, first pixel is (10, 5).
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
        assert_eq!(u32::from_le_bytes(png[4..8].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(png[8..12].try_into().unwrap()), 3);
        assert_eq!(&png[12..16], &[10, 5, 0, 255]);
        assert_eq!(
            pixels.reads.load(Ordering::Relaxed),
            1,
            "one selection must cost exactly one region readback"
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// The Phase 3 contract in service form: the bytes handed to the encoder are the
    /// selection's, never the frame's.
    #[test]
    fn prepare_reads_only_the_selection_bytes() {
        let dir = test_dir("region-bytes");
        let service = CaptureService::new(
            FixedDir(dir.clone()),
            CountingEncoder {
                calls: AtomicU64::new(0),
            },
        );
        let pixels = GridPixels::new(3840, 2160);
        let prepared = service
            .prepare_selection(&frame(3840, 2160), Rect::new(100, 100, 400, 300), &pixels)
            .unwrap();
        assert_eq!(prepared.bgra.len(), 300 * 200 * 4);
        assert_eq!(prepared.region.width(), 300);
        assert_eq!(prepared.region.height(), 200);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn selection_is_clipped_to_the_frame_before_readback() {
        let dir = test_dir("clip");
        let service = CaptureService::new(
            FixedDir(dir.clone()),
            CountingEncoder {
                calls: AtomicU64::new(0),
            },
        );
        let pixels = GridPixels::new(100, 50);
        let prepared = service
            .prepare_selection(&frame(100, 50), Rect::new(95, 45, 200, 200), &pixels)
            .unwrap();
        assert_eq!(prepared.region, Rect::new(95, 45, 100, 50));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_or_offscreen_selections_are_rejected() {
        let dir = test_dir("reject");
        let service = CaptureService::new(
            FixedDir(dir.clone()),
            CountingEncoder {
                calls: AtomicU64::new(0),
            },
        );
        let pixels = GridPixels::new(100, 50);
        for selection in [Rect::default(), Rect::new(500, 500, 600, 600)] {
            assert!(matches!(
                service.prepare_selection(&frame(100, 50), selection, &pixels),
                Err(CaptureError::InvalidState(_))
            ));
        }
        assert_eq!(
            pixels.reads.load(Ordering::Relaxed),
            0,
            "no readback must happen for a rejected selection"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn readback_of_the_wrong_size_is_rejected() {
        struct WrongPixels;
        impl PixelSliceSource for WrongPixels {
            fn read_bgra(&self, _region: Rect) -> CaptureResult<Vec<u8>> {
                Ok(vec![0; 4])
            }
        }
        let dir = test_dir("wrong-size");
        let service = CaptureService::new(
            FixedDir(dir.clone()),
            CountingEncoder {
                calls: AtomicU64::new(0),
            },
        );
        let error = service
            .prepare_selection(&frame(100, 50), Rect::new(0, 0, 10, 10), &WrongPixels)
            .unwrap_err();
        assert!(matches!(error, CaptureError::EncodeFailed(_)));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn artifact_is_written_to_the_artifact_directory() {
        let dir = test_dir("artifact");
        let service = CaptureService::new(
            FixedDir(dir.clone()),
            CountingEncoder {
                calls: AtomicU64::new(0),
            },
        );
        let pixels = GridPixels::new(100, 50);
        let artifact = produce(
            &service,
            "session-1",
            &frame(100, 50),
            Rect::new(1, 2, 6, 7),
            144,
            Some(r"\\.\DISPLAY2".into()),
            &pixels,
        )
        .unwrap();
        assert_eq!(artifact.session_id, "session-1");
        assert_eq!((artifact.width, artifact.height), (5, 5));
        assert_eq!(artifact.dpi, 144);
        assert_eq!(artifact.monitor_device_name.as_deref(), Some(r"\\.\DISPLAY2"));
        let path = artifact.png_path().unwrap().to_path_buf();
        assert!(path.starts_with(&dir));
        assert!(path.exists());
        assert_eq!(fs::read(&path).unwrap().len(), 12 + 4);
        // No temporary files may be left behind.
        let leftovers = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn artifacts_get_unique_paths_within_a_session() {
        let dir = test_dir("unique");
        let service = CaptureService::new(
            FixedDir(dir.clone()),
            CountingEncoder {
                calls: AtomicU64::new(0),
            },
        );
        let first = service.write_artifact("session-1", b"a").unwrap();
        let second = service.write_artifact("session-1", b"b").unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"a");
        assert_eq!(fs::read(&second).unwrap(), b"b");
        let _ = fs::remove_dir_all(dir);
    }

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
        assert_eq!(decoded.bytes(), &[1, 1, 0, 255, 2, 1, 0, 255, 1, 2, 0, 255, 2, 2, 0, 255]);
        let _ = fs::remove_dir_all(dir);
    }

    /// Section 8.1 of the tasklist: the whole capture pipeline must run with no
    /// clipboard, no database and no OCR model in the process.
    ///
    /// This test never touches `platform::windows::clipboard`, `Store` or the OCR
    /// module, and still produces a complete, readable artifact.
    #[test]
    fn capture_pipeline_is_independent_of_clipboard_store_and_ocr() {
        let dir = test_dir("isolated");
        let service = CaptureService::new(FixedDir(dir.clone()), PngArtifactEncoder);
        let pixels = GridPixels::new(32, 24);
        let frozen = CapturedFrame {
            width: 32,
            height: 24,
            pixel_format: PixelFormat::Bgra8Unorm,
            captured_at_unix_ms: 1_700_000_000_000,
            provider: "test",
        };

        let artifact = produce(
            &service,
            "capture-isolated",
            &frozen,
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
}
