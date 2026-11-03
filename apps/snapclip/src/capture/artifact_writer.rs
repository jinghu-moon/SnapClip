//! The shell's implementation of capture's `ArtifactWriter` port (docs/23 T2.4, moved here
//! by P6).
//!
//! This is where the two halves of artifact production meet: capture hands over the pixels
//! the user selected, and history owns turning them into a file. Encoding lives in
//! [`super::row_band_png`], the write in
//! `CaptureArtifactStore` — so capture never learns what a PNG is, and history never learns
//! what a selection is.
//!
//! It runs on the export worker thread, never on the overlay thread.
//!
//! # Rows, not an image (`P4.03`)
//!
//! Until `P4.03` this function handed one whole image to an encoder, and the chain held four
//! copies of the pixels (`docs/30 §22.1`): the old `prepared.bgra.clone()`, the BGRA→RGBA buffer the old
//! encoder built, the second `Vec` `RgbaImage::from_raw` took of it, and the encoder's own working
//! set. At `1920×300,000` one copy is 2.15 GiB, so "four copies" was not slowness, it was an export
//! that cannot finish (`F-07`).
//!
//! The encoder is shaped to make that impossible rather than merely discouraged:
//! `PngRowBandWriter::write_rows` takes a **slice** of rows, so the encoder is fed the caller's
//! pixels and never needs a second whole image. Measured by
//! `apps/snapclip/tests/export_path_copies.rs` at `1280×20,000`: the path now allocates **zero**
//! buffers at least as large as the image, against six before (`docs/30 §22.4`'s "all four copies
//! eliminated", and `§30.7`'s row).

use std::path::PathBuf;

use snapclip_capture::artifact::SelectionPixels;
use snapclip_capture::ports::ArtifactWriter;
use snapclip_capture::{CaptureArtifact, CaptureError, CapturePayload, CaptureResult};
use snapclip_history::artifact_store::CaptureArtifactStore;
use snapclip_model::{CaptureMetadata, CaptureOutput};

use super::row_band_png::{ImageMeta, PngRowBandSink};

/// Encodes through `snapclip-history` and writes through its artifact store.
pub struct HistoryArtifactWriter {
    store: CaptureArtifactStore,
    history: Option<snapclip_history::store::Store>,
}

impl HistoryArtifactWriter {
    /// `root` is the directory artifacts land in (`<app local data>/artifacts/capture`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            store: CaptureArtifactStore::new(root),
            history: None,
        }
    }

    /// The shell injects its one database writer; tests/export tools may omit history.
    pub fn with_history(mut self, history: snapclip_history::store::Store) -> Self {
        self.history = Some(history);
        self
    }

    /// Archive a panel's unchanged original, independently of its chosen export.
    pub fn write_pixels(
        &self,
        metadata: &CaptureMetadata,
        pixels: &snapclip_model::pixels::PixelBuffer,
    ) -> CaptureResult<CaptureArtifact> {
        if pixels.size() != [metadata.width, metadata.height]
            || pixels.descriptor() != metadata.source.descriptor()
        {
            return Err(CaptureError::EncodeFailed(
                "capture pixels differ from archive metadata".into(),
            ));
        }
        let width =
            i32::try_from(metadata.width).map_err(|e| CaptureError::EncodeFailed(e.to_string()))?;
        let height = i32::try_from(metadata.height)
            .map_err(|e| CaptureError::EncodeFailed(e.to_string()))?;
        let spec = snapclip_capture::ports::SelectionSpec {
            region: snapclip_model::Rect::new(0, 0, width, height),
            source: metadata.source,
            captured_at_unix_ms: metadata.captured_at_unix_ms,
            origin: None,
            corner_radius: 0,
        };
        let mut bands = WholeImage {
            bytes: pixels.as_bytes(),
            rows: metadata.height,
            source: metadata.source,
        };
        self.write_bands(
            &metadata.session_id,
            &spec,
            &mut bands,
            metadata.dpi,
            metadata.monitor_device_name.clone(),
        )
    }

    fn publish_capture(
        &self,
        metadata: &CaptureMetadata,
        reference: &snapclip_model::ArtifactRef,
        thumbnail: Option<snapclip_history::thumbnail::CaptureThumbnail>,
    ) -> CaptureResult<()> {
        if let (Some(history), Some(thumbnail)) = (&self.history, thumbnail) {
            let name = reference
                .absolute_path
                .file_name()
                .ok_or_else(|| {
                    CaptureError::EncodeFailed("capture artifact has no file name".into())
                })?
                .to_string_lossy();
            let id = format!("capture-{}", blake3::hash(name.as_bytes()).to_hex());
            history
                .save_capture(
                    id,
                    snapclip_model::CaptureSource {
                        metadata: metadata.clone(),
                        artifact: reference.clone(),
                    },
                    thumbnail.finish().map_err(CaptureError::EncodeFailed)?,
                )
                .map_err(|e| {
                    CaptureError::EncodeFailed(format!(
                        "capture source was written but history registration failed; original retained at {}: {e}",
                        reference.absolute_path.display()
                    ))
                })?;
        }
        Ok(())
    }
}

/// A picture that already sits in one buffer, offered to the streaming path as a source.
///
/// The whole-image entry point is this source with one band, so both entry points run the
/// same encoding loop (docs/35 §13.4).
struct WholeImage<'a> {
    bytes: &'a [u8],
    rows: u32,
    source: snapclip_model::pixels::PixelSource,
}

impl snapclip_capture::ports::BandSource for WholeImage<'_> {
    fn source(&self) -> snapclip_model::pixels::PixelSource {
        self.source
    }
    fn rows(&self) -> u32 {
        self.rows
    }

    fn read_rows(&mut self, first_row: u32, rows: u32, out: &mut Vec<u8>) -> bool {
        let start = first_row as usize * self.row_bytes();
        let len = rows as usize * self.row_bytes();
        match self.bytes.get(start..start + len) {
            Some(slice) => {
                out.clear();
                out.extend_from_slice(slice);
                true
            }
            None => false,
        }
    }
}

impl WholeImage<'_> {
    fn row_bytes(&self) -> usize {
        if self.rows == 0 {
            0
        } else {
            self.bytes.len() / self.rows as usize
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
        let (width, height) = checked_size(prepared.region)?;
        // `Bgra8Image::new` used to be the length check; it went with the copy it was attached
        // to. Same comparison, same words — `P4.01`'s pixels are tightly packed `region`-sized
        // BGRA.
        prepared.validate()?;
        let expected = u64::from(width)
            * u64::from(height)
            * prepared.pixels.descriptor().bytes_per_pixel() as u64;
        let bytes = prepared.pixels.as_bytes();
        if expected != bytes.len() as u64 {
            return Err(CaptureError::EncodeFailed(format!(
                "pixel buffer of {} bytes does not match {width}x{height}",
                bytes.len()
            )));
        }
        let spec = snapclip_capture::ports::SelectionSpec {
            region: prepared.region,
            source: prepared.source(),
            captured_at_unix_ms: prepared.frame.captured_at_unix_ms,
            origin: prepared.origin,
            corner_radius: prepared.corner_radius,
        };
        let mut whole = WholeImage {
            bytes,
            rows: height,
            source: prepared.source(),
        };
        self.write_bands(session_id, &spec, &mut whole, dpi, monitor_device_name)
    }

    /// Streams the rows into the encoder one band at a time (docs/35 §13.4).
    ///
    /// The only buffer this needs is one band plus whatever the encoder holds, so a scroll
    /// capture's thirty thousand rows never become one `Vec` on the way to a file. The
    /// encoded PNG is still a `Vec` in `Artifact`, but that is the finished file (megabytes),
    /// not the picture (hundreds).
    fn write_bands(
        &self,
        session_id: &str,
        spec: &snapclip_capture::ports::SelectionSpec,
        bands: &mut dyn snapclip_capture::ports::BandSource,
        dpi: u32,
        monitor_device_name: Option<String>,
    ) -> CaptureResult<snapclip_model::CaptureArtifact> {
        let (width, height) = checked_size(spec.region)?;
        if bands.source() != spec.source {
            return Err(CaptureError::EncodeFailed(
                "band source differs from its frozen generation".into(),
            ));
        }
        if bands.rows() != height {
            return Err(CaptureError::EncodeFailed(format!(
                "band source has {} rows but the region is {height} tall",
                bands.rows()
            )));
        }

        let metadata = CaptureMetadata {
            session_id: session_id.into(),
            width,
            height,
            dpi,
            source: spec.source,
            captured_at_unix_ms: spec.captured_at_unix_ms,
            monitor_device_name,
        };
        let mut thumbnail = self
            .history
            .as_ref()
            .map(|_| snapclip_history::thumbnail::CaptureThumbnail::new(&metadata))
            .transpose()
            .map_err(|e| CaptureError::EncodeFailed(e.into()))?;

        // HDR/linear/associated source bits belong to a pixel master, never PNG8.
        // Ordinary SDR keeps its existing streaming PNG path and memory behavior.
        if spec.source.descriptor() != snapclip_model::pixels::PixelDescriptor::bgra8() {
            let reference = self
                .store
                .write_pixel_master(&metadata, |first, count| {
                    let mut bytes = Vec::new();
                    if bands.source() != spec.source
                        || !bands.read_rows(first, count, &mut bytes)
                        || bands.source() != spec.source
                    {
                        return Err(snapclip_history::StoreError::InvalidPublication(
                            "source changed or refused a master tile".into(),
                        ));
                    }
                    let pixels = snapclip_model::pixels::PixelBuffer::from_tight(
                        width,
                        count,
                        spec.source.descriptor(),
                        bytes,
                    )
                    .map_err(|message| {
                        snapclip_history::StoreError::InvalidPublication(message.into())
                    })?;
                    if let Some(thumbnail) = &mut thumbnail {
                        thumbnail.push(first, &pixels).map_err(|e| {
                            snapclip_history::StoreError::InvalidPublication(e.into())
                        })?;
                    }
                    Ok(pixels)
                })
                .map_err(|error| CaptureError::EncodeFailed(error.to_string()))?;
            self.publish_capture(&metadata, &reference, thumbnail)?;
            return Ok(CaptureArtifact {
                session_id: session_id.into(),
                width,
                height,
                dpi,
                source: spec.source,
                captured_at_unix_ms: spec.captured_at_unix_ms,
                monitor_device_name: metadata.monitor_device_name,
                payload: CapturePayload::PixelMaster { reference },
            });
        }

        let meta = ImageMeta {
            width: u64::from(width),
            height: u64::from(height),
        };

        let mut sink = PngRowBandSink::new();
        let mut writer = sink
            .begin(&meta)
            .map_err(|error| CaptureError::EncodeFailed(error.to_string()))?;

        let row_bytes = width as usize * 4;
        // About four megabytes a band: big enough to keep the call count low, small enough
        // that the band never becomes the picture.
        let band_rows = ((4 << 20) / row_bytes.max(1)).clamp(1, 1024) as u32;
        let mut scratch: Vec<u8> = Vec::new();
        let mut first = 0u32;
        while first < height {
            let count = (height - first).min(band_rows);
            scratch.resize(count as usize * row_bytes, 0);
            if !bands.read_rows(first, count, &mut scratch) || bands.source() != spec.source {
                return Err(CaptureError::EncodeFailed(format!(
                    "band source failed at row {first} of {height}"
                )));
            }
            writer
                .write_rows(u64::from(first), &scratch)
                .map_err(|error| CaptureError::EncodeFailed(error.to_string()))?;
            if let Some(thumbnail) = &mut thumbnail {
                let pixels = snapclip_model::pixels::PixelBuffer::from_tight(
                    width,
                    count,
                    spec.source.descriptor(),
                    std::mem::take(&mut scratch),
                )
                .map_err(|e| CaptureError::EncodeFailed(e.into()))?;
                thumbnail
                    .push(first, &pixels)
                    .map_err(|e| CaptureError::EncodeFailed(e.into()))?;
                scratch = pixels.into_bytes();
            }
            first += count;
        }

        let artifact = writer
            .finish()
            .map_err(|error| CaptureError::EncodeFailed(error.to_string()))?;

        let output = CaptureOutput {
            bytes: artifact.bytes,
            metadata,
        };
        let reference = self
            .store
            .write(&output)
            .map_err(|error| CaptureError::EncodeFailed(error.to_string()))?;

        self.publish_capture(&output.metadata, &reference, thumbnail)?;
        Ok(CaptureArtifact {
            session_id: session_id.to_string(),
            width,
            height,
            dpi,
            source: spec.source,
            captured_at_unix_ms: spec.captured_at_unix_ms,
            monitor_device_name: output.metadata.monitor_device_name.clone(),
            payload: CapturePayload::PngFile {
                path: reference.absolute_path,
            },
        })
    }
}

/// The export port's size domain is `u64` (`docs/30 §17.7`), so a region's signed dimensions
/// are widened, never narrowed; truncating and refusing are two different failures
/// (`docs/30 §26.1.1`).
fn checked_size(region: snapclip_capture::geometry::Rect) -> CaptureResult<(u32, u32)> {
    match (
        u32::try_from(region.width()),
        u32::try_from(region.height()),
    ) {
        (Ok(width), Ok(height)) if width > 0 && height > 0 => Ok((width, height)),
        _ => Err(CaptureError::EncodeFailed(format!(
            "{}x{} at ({}, {}) is not a drawable region",
            region.width(),
            region.height(),
            region.left,
            region.top
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::HistoryArtifactWriter;
    use snapclip_capture::CaptureResult;
    use snapclip_capture::artifact::{CaptureService, PixelSliceSource, SelectionPixels};
    use snapclip_capture::geometry::Rect;
    use snapclip_capture::ports::ArtifactWriter;
    use snapclip_capture::session::CapturedFrame;
    use snapclip_history::image::decode_to_bgra8;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// A band source that cannot supply its rows must fail the export **before** anything is
    /// written, because the alternative is publishing a picture with a hole in it (docs/36 §12.1).
    #[test]
    fn a_band_source_that_fails_writes_no_artifact() {
        struct Broken;
        impl snapclip_capture::ports::BandSource for Broken {
            fn source(&self) -> snapclip_model::pixels::PixelSource {
                snapclip_model::pixels::PixelSource::new(
                    snapclip_model::pixels::PixelDescriptor::bgra8(),
                    0,
                )
            }
            fn rows(&self) -> u32 {
                4
            }
            fn read_rows(&mut self, _first: u32, _rows: u32, _out: &mut Vec<u8>) -> bool {
                false
            }
        }

        let path = root("broken-bands");
        let writer = HistoryArtifactWriter::new(path.clone());
        let spec = snapclip_capture::ports::SelectionSpec {
            region: Rect::new(0, 0, 4, 4),
            source: snapclip_model::pixels::PixelSource::new(
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                0,
            ),
            captured_at_unix_ms: 0,
            origin: None,
            corner_radius: 0,
        };
        let error = writer
            .write_bands("broken-bands", &spec, &mut Broken, 96, None)
            .expect_err("a band source that cannot read must fail the export");
        assert!(
            format!("{error}").contains("band source failed"),
            "the error should name the read, not the encoder: {error}"
        );
        let written = fs::read_dir(&path).map(|dir| dir.count()).unwrap_or(0);
        assert_eq!(
            written, 0,
            "an artifact was written for a picture that could not be read"
        );
    }

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
        fn source(&self) -> snapclip_model::pixels::PixelSource {
            snapclip_model::pixels::PixelSource::new(
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                0,
            )
        }
        fn read_pixels(&self, region: Rect) -> CaptureResult<snapclip_model::pixels::PixelBuffer> {
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
            // These tests are about encoding, not about where the pixels came from.
            origin: Some(snapclip_model::geometry::Point::new(0, 0)),
        }
    }

    /// A scroll capture's long image arrives as one `SelectionPixels` covering the whole image,
    /// and comes back out with every row in its place.
    #[test]
    fn a_long_image_becomes_a_png_with_every_row_in_place() {
        let dir = root("scroll-long");
        let writer = HistoryArtifactWriter::new(dir.clone());
        let (width, height) = (4u32, 600u32);
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for _ in 0..width {
                bgra.extend_from_slice(&[y as u8, y as u8, y as u8, 255]);
            }
        }
        let prepared = SelectionPixels {
            corner_radius: 0,
            frame: frame(width, height),
            region: Rect::new(0, 0, width as i32, height as i32),
            pixels: snapclip_model::pixels::PixelBuffer::from_tight(
                width,
                height,
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                bgra,
            )
            .unwrap()
            .into(),
            origin: Some(snapclip_model::geometry::Point::new(0, 0)),
        };

        let artifact = writer.write("scroll-1-1", &prepared, 144, None).unwrap();

        assert_eq!((artifact.width, artifact.height), (4, 600));
        assert_eq!(artifact.dpi, 144);
        let path = artifact.png_path().unwrap();
        assert!(path.starts_with(&dir));
        let decoded = decode_to_bgra8(&fs::read(path).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 600));
        let bytes = decoded.bytes();
        assert_eq!(&bytes[300 * 16..300 * 16 + 4], &[44, 44, 44, 255]);
        assert_eq!(&bytes[599 * 16..599 * 16 + 4], &[0x57, 0x57, 0x57, 255]);
    }

    #[test]
    fn the_writer_hands_the_store_decodable_png_bytes() {
        let dir = root("png");
        let writer = HistoryArtifactWriter::new(dir.clone());
        let prepared = CaptureService::new()
            .prepare_selection(
                &frame(4, 4),
                Rect::new(1, 1, 3, 3),
                &GridPixels {
                    width: 4,
                    height: 4,
                },
            )
            .unwrap();

        let artifact = writer
            .write("capture-1-1", &prepared, 144, Some(r"\\.\DISPLAY1".into()))
            .unwrap();

        assert_eq!(artifact.session_id, "capture-1-1");
        assert_eq!((artifact.width, artifact.height), (2, 2));
        assert_eq!(artifact.dpi, 144);
        assert_eq!(
            artifact.monitor_device_name.as_deref(),
            Some(r"\\.\DISPLAY1")
        );

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
            corner_radius: 0,
            frame: frame(4, 4),
            region: Rect::new(0, 0, -5, 2),
            pixels: snapclip_model::pixels::PixelBuffer::from_tight(
                4,
                4,
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                vec![0; 64],
            )
            .unwrap()
            .into(),
            origin: Some(snapclip_model::geometry::Point::new(0, 0)),
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

    /// docs/35 §13.4 (stage 3b): the banded entry point and the whole-image one are the same
    /// encoder loop, so they must produce the same file.
    #[test]
    fn a_banded_picture_writes_the_same_png_as_a_whole_one() {
        struct Rows {
            width: u32,
            rows: u32,
        }

        impl snapclip_capture::ports::BandSource for Rows {
            fn source(&self) -> snapclip_model::pixels::PixelSource {
                snapclip_model::pixels::PixelSource::new(
                    snapclip_model::pixels::PixelDescriptor::bgra8(),
                    0,
                )
            }
            fn rows(&self) -> u32 {
                self.rows
            }

            fn read_rows(&mut self, first_row: u32, rows: u32, out: &mut Vec<u8>) -> bool {
                out.clear();
                for y in first_row..first_row + rows {
                    if y >= self.rows {
                        return false;
                    }
                    for x in 0..self.width {
                        out.extend_from_slice(&[x as u8, y as u8, 0, 255]);
                    }
                }
                true
            }
        }

        let (width, height) = (320u32, 200u32);
        let directory = root("banded-eq");
        let writer = HistoryArtifactWriter::new(directory.clone());
        let mut bgra = Vec::new();
        for y in 0..height {
            for x in 0..width {
                bgra.extend_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        let prepared = SelectionPixels {
            corner_radius: 0,
            frame: frame(width, height),
            region: Rect::new(0, 0, width as i32, height as i32),
            pixels: snapclip_model::pixels::PixelBuffer::from_tight(
                width,
                height,
                snapclip_model::pixels::PixelDescriptor::bgra8(),
                bgra,
            )
            .unwrap()
            .into(),
            origin: None,
        };

        let whole = writer
            .write("eq-whole", &prepared, 96, None)
            .expect("whole export");
        let spec = snapclip_capture::ports::SelectionSpec {
            region: prepared.region,
            source: prepared.frame.source,
            captured_at_unix_ms: prepared.frame.captured_at_unix_ms,
            origin: prepared.origin,
            corner_radius: prepared.corner_radius,
        };
        let mut bands = Rows {
            width,
            rows: height,
        };
        let banded = writer
            .write_bands("eq-banded", &spec, &mut bands, 96, None)
            .expect("banded export");

        let bytes_of = |artifact: &snapclip_model::CaptureArtifact| {
            let path = artifact.png_path().expect("a PNG payload");
            std::fs::read(path).expect("the artifact is readable")
        };
        assert_eq!(
            bytes_of(&whole),
            bytes_of(&banded),
            "the banded entry point wrote a different file than the whole-image one",
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn hdr_selection_publishes_a_lossless_master_without_a_png8_conversion() {
        use snapclip_model::pixels::{
            AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource,
        };
        let directory = root("hdr-master");
        let descriptor = PixelDescriptor::new(
            snapclip_model::PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap();
        let source = PixelSource::new(descriptor, 41);
        let bytes = [0, 0xbc, 0, 0x49, 0, 0, 0, 0x38].repeat(12);
        let mut frame = frame(4, 3);
        frame.source = source;
        let selection = SelectionPixels {
            frame,
            region: Rect::new(0, 0, 4, 3),
            pixels: PixelBuffer::from_tight(4, 3, descriptor, bytes.clone())
                .unwrap()
                .into(),
            origin: None,
            corner_radius: 0,
        };
        let result = HistoryArtifactWriter::new(&directory)
            .write("hdr-selection", &selection, 144, None)
            .unwrap();
        assert_eq!(result.source, source);
        assert!(result.png_path().is_none());
        let snapclip_model::CapturePayload::PixelMaster { reference } = &result.payload else {
            panic!("HDR must retain a source-format master")
        };
        let mut master = snapclip_history::pixel_master::PixelMaster::open(reference).unwrap();
        assert_eq!(master.metadata().source, source);
        assert_eq!(master.read_rows(0, 3).unwrap().as_bytes(), bytes);
        assert_eq!(
            selection.pixels.as_bytes(),
            bytes,
            "persistence must not mutate the source"
        );
        drop(master);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn an_archive_registration_failure_reports_the_retained_original_path() {
        use snapclip_model::pixels::{
            AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource,
        };
        let directory = root("registration-failure");
        let history = snapclip_history::store::Store::open(directory.join("history")).unwrap();
        // A source outside history's owned directory must be rejected, but the
        // caller must still be told exactly where its immutable original lives.
        let artifacts = directory.join("separate-artifacts");
        let descriptor = PixelDescriptor::new(
            snapclip_model::PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap();
        let bytes = [0, 0xb4, 0, 0x49, 0, 0, 0, 0x38].to_vec();
        let pixels = PixelBuffer::from_tight(1, 1, descriptor, bytes.clone()).unwrap();
        let metadata = snapclip_model::CaptureMetadata {
            session_id: "failed-registration".into(),
            width: 1,
            height: 1,
            dpi: 144,
            source: PixelSource::new(descriptor, 93),
            captured_at_unix_ms: 42,
            monitor_device_name: None,
        };
        let error = HistoryArtifactWriter::new(&artifacts)
            .with_history(history)
            .write_pixels(&metadata, &pixels)
            .unwrap_err();
        let paths: Vec<_> = fs::read_dir(&artifacts)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(paths.len(), 1);
        assert!(error.to_string().contains("history registration failed"));
        assert!(error.to_string().contains(&paths[0].display().to_string()));
        let reference = snapclip_model::ArtifactRef {
            absolute_path: paths[0].clone(),
            mime: "application/x-snapclip-pixels".into(),
            dimensions: snapclip_model::ImageDimensions {
                width: 1,
                height: 1,
            },
            byte_len: fs::metadata(&paths[0]).unwrap().len(),
            content_fingerprint: blake3::hash(&fs::read(&paths[0]).unwrap())
                .to_hex()
                .to_string(),
        };
        let mut master = snapclip_history::pixel_master::PixelMaster::open(&reference).unwrap();
        assert_eq!(master.metadata(), &metadata);
        assert_eq!(master.read_rows(0, 1).unwrap().as_bytes(), bytes);
        assert_eq!(pixels.as_bytes(), bytes);
        drop(master);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn changed_source_generation_cannot_publish_a_master() {
        use snapclip_capture::ports::{BandSource, SelectionSpec};
        use snapclip_model::pixels::{AlphaMode, PixelDescriptor, PixelEncoding, PixelSource};
        struct Changing(PixelSource);
        impl BandSource for Changing {
            fn source(&self) -> PixelSource {
                self.0
            }
            fn rows(&self) -> u32 {
                3
            }
            fn read_rows(&mut self, _: u32, count: u32, bytes: &mut Vec<u8>) -> bool {
                *bytes = [0, 0xbc, 0, 0x49, 0, 0, 0, 0x3c].repeat(4 * count as usize);
                self.0 = PixelSource::new(self.0.descriptor(), 42);
                true
            }
        }
        let directory = root("changed-master");
        let source = PixelSource::new(
            PixelDescriptor::new(
                snapclip_model::PixelFormat::Rgba16Float,
                PixelEncoding::ScRgb,
                AlphaMode::Straight,
            )
            .unwrap(),
            41,
        );
        let spec = SelectionSpec {
            region: Rect::new(0, 0, 4, 3),
            source,
            captured_at_unix_ms: 1,
            origin: None,
            corner_radius: 0,
        };
        assert!(
            HistoryArtifactWriter::new(&directory)
                .write_bands("changed-source", &spec, &mut Changing(source), 96, None)
                .is_err()
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn rounded_opaque_hdr_selection_records_the_derived_alpha_without_relabeling_the_frame() {
        use snapclip_model::pixels::{
            AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource,
        };
        let directory = root("rounded-hdr-master");
        let descriptor = PixelDescriptor::new(
            snapclip_model::PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Opaque,
        )
        .unwrap();
        let source = PixelSource::new(descriptor, 41);
        let mut frame = frame(20, 20);
        frame.source = source;
        let pixels = PixelBuffer::from_tight(
            20,
            20,
            descriptor,
            [0, 0xbc, 0, 0x49, 0, 0, 0, 0x3c].repeat(400),
        )
        .unwrap();
        let selection = SelectionPixels {
            frame,
            region: Rect::new(0, 0, 20, 20),
            origin: None,
            corner_radius: 6,
            pixels: snapclip_capture::artifact::round_pixel_corners(pixels, 6)
                .unwrap()
                .into(),
        };
        let result = HistoryArtifactWriter::new(&directory)
            .write("rounded-hdr-selection", &selection, 144, None)
            .unwrap();
        assert_eq!(
            selection.frame.source, source,
            "the acquisition definition is immutable"
        );
        assert_eq!(result.source.generation(), source.generation());
        assert_eq!(result.source.descriptor().alpha(), AlphaMode::Straight);
        let snapclip_model::CapturePayload::PixelMaster { reference } = &result.payload else {
            panic!("HDR must retain a source-format master")
        };
        let mut master = snapclip_history::pixel_master::PixelMaster::open(reference).unwrap();
        assert_eq!(master.metadata().source, result.source);
        assert_eq!(
            master.read_rows(0, 20).unwrap().as_bytes(),
            selection.pixels.as_bytes()
        );
        drop(master);
        let _ = fs::remove_dir_all(directory);
    }
}
