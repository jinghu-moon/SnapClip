//! The shell's implementation of capture's `ArtifactWriter` port (docs/23 T2.4, moved here
//! by P6).
//!
//! This is where the two halves of artifact production meet: capture hands over the pixels
//! the user selected, and history owns turning them into a file. Encoding lives in
//! [`super::row_band_png`] behind `snapclip-capture`'s `RowBandSink` port, the write in
//! `CaptureArtifactStore` — so capture never learns what a PNG is, and history never learns
//! what a selection is.
//!
//! It runs on the export worker thread, never on the overlay thread.
//!
//! # Rows, not an image (`P4.03`)
//!
//! Until `P4.03` this function handed one whole image to an encoder, and the chain held four
//! copies of the pixels (`docs/30 §22.1`): `prepared.bgra.clone()`, the BGRA→RGBA buffer the old
//! encoder built, the second `Vec` `RgbaImage::from_raw` took of it, and the encoder's own working
//! set. At `1920×300,000` one copy is 2.15 GiB, so "four copies" was not slowness, it was an export
//! that cannot finish (`F-07`).
//!
//! The port in `docs/30 §17.7` is shaped to make that impossible rather than merely discouraged:
//! `RowBandWriter::write_rows` takes a **slice** of rows, so the encoder is fed the caller's
//! pixels and never needs a second whole image. Measured by
//! `apps/snapclip/tests/export_path_copies.rs` at `1280×20,000`: the path now allocates **zero**
//! buffers at least as large as the image, against six before (`docs/30 §22.4`'s "all four copies
//! eliminated", and `§30.7`'s row).

use std::path::PathBuf;

use snapclip_capture::artifact::SelectionPixels;
use snapclip_capture::ports::ArtifactWriter;
use snapclip_capture::scroll::Axis;
use snapclip_capture::scroll::export::{ImageMeta, RowBandSink};
use snapclip_capture::{CaptureArtifact, CaptureError, CapturePayload, CaptureResult};
use snapclip_history::artifact_store::CaptureArtifactStore;
use snapclip_model::{CaptureMetadata, CaptureOutput};

use super::row_band_png::PngRowBandSink;

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
        // The export port's size domain is `u64` (`docs/30 §17.7`), so the region's signed
        // dimensions are **widened**, never narrowed. A bare truncating `u32` cast wraps a negative
        // dimension into a size the format has no room for, and the refusal then names a number the
        // caller never supplied (`P4.04`, `D-12`). One checked conversion, before anything is drawn:
        // this is `docs/30 §26.1`'s "refused before `begin`", and it is the same check the sink
        // makes for the scroll path's benefit — `ImageMeta` is `u64`, the PNG header is not.
        //
        // `docs/30 §26.1.1` is the statement worth keeping in view while editing this: **truncating
        // and refusing are two different failures.** Truncating continues with a number of its own
        // choosing and leaves a valid PNG whose dimensions are not the ones that were asked for —
        // the error surfaces downstream, where the coordinates no longer line up and the evidence is
        // gone. Refusing writes no byte at all and keeps "the size I got is the size I asked for"
        // true in both the type and the value.
        let (width, height) = match (
            u32::try_from(prepared.region.width()),
            u32::try_from(prepared.region.height()),
        ) {
            (Ok(width), Ok(height)) if width > 0 && height > 0 => (width, height),
            _ => {
                return Err(CaptureError::EncodeFailed(format!(
                    "{}x{} at ({}, {}) is not a drawable region",
                    prepared.region.width(),
                    prepared.region.height(),
                    prepared.region.left,
                    prepared.region.top
                )))
            }
        };
        // `Bgra8Image::new` used to be the length check; it went with the copy it was attached to.
        // Same comparison, same words — `P4.01`'s pixels are tightly packed `region`-sized BGRA.
        let expected = u64::from(width) * u64::from(height) * 4;
        if expected != prepared.bgra.len() as u64 {
            return Err(CaptureError::EncodeFailed(format!(
                "bgra buffer of {} bytes does not match {width}x{height}",
                prepared.bgra.len()
            )));
        }

        let meta = ImageMeta {
            width: u64::from(width),
            height: u64::from(height),
            // Not the scroll path: there is no canvas that could have been capped, so the artifact
            // is as long as it is tall.
            length: u64::from(height),
            // Nor is there a scene that scrolled. `Vertical` is the reading PNG itself has —
            // row-major, top-down — not a claim about movement.
            axis: Axis::Vertical,
            // `dpr` is for the scroll preview's CSS-pixel consumers (§17.7); an ordinary screenshot
            // has none, and its DPI is carried in `CaptureMetadata::dpi` below. `1` therefore means
            // "not applicable" rather than being a guess at the display scale.
            dpr: 1,
        };

        let mut sink = PngRowBandSink::new();
        let mut writer = sink
            .begin(&meta)
            .map_err(|error| CaptureError::EncodeFailed(error.to_string()))?;
        // One call, not a band loop: the ordinary path already has every row in one contiguous
        // buffer, and `write_rows` takes a slice, so slicing it into bands would be machinery for
        // the scroll path's LRU that this path does not have — with no measurement behind it.
        writer
            .write_rows(0, &prepared.bgra)
            .map_err(|error| CaptureError::EncodeFailed(error.to_string()))?;
        let artifact = writer
            .finish(None)
            .map_err(|error| CaptureError::EncodeFailed(error.to_string()))?;

        let output = CaptureOutput {
            bytes: artifact.bytes,
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
    use snapclip_capture::artifact::{CaptureService, PixelSliceSource, SelectionPixels};
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

    /// `P4.04` / `D-12`: a dimension that cannot be drawn must be **refused**, and the refusal must
    /// be about the region the caller handed over.
    ///
    /// `Rect` carries `i32`, so a dimension cannot leave the representable range by being *large* —
    /// the widest rectangle is `i32::MAX` wide, which `u32` holds without complaint. The way it
    /// leaves is by being **negative**, i.e. an inverted rectangle. A truncating `u32` cast turns
    /// `-5` into `4294967291`, so the writer reports a size of `4294967291x2` that no caller ever
    /// asked for, and the honest reason — "this region is not a drawable size" — is never said.
    /// That is the silent garbling `docs/30 §22.1` calls out, and it is why the narrowing is
    /// deleted rather than guarded: the value the error names must be the value the caller supplied.
    #[test]
    fn an_oversized_dimension_is_rejected_before_the_first_byte() {
        let dir = root("oversized");
        let writer = HistoryArtifactWriter::new(dir.clone());
        let prepared = SelectionPixels {
            frame: frame(4, 4),
            region: Rect::new(0, 0, -5, 2),
            bgra: Vec::new(),
        };

        let error = writer
            .write("capture-1-1", &prepared, 144, None)
            .expect_err("an inverted region is not an image");

        let message = error.to_string();
        assert!(
            message.contains("-5"),
            "the refusal must name the region that was supplied, got `{message}`"
        );
        assert!(
            !message.contains("4294967291"),
            "4294967291 is the number the narrowing invented, not the one the caller asked for: `{message}`"
        );
        assert!(
            !dir.exists(),
            "the refusal must land before the first byte is written"
        );
    }
}
