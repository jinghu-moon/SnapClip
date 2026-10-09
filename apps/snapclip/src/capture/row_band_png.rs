//! `PngRowBandSink` — the shell's implementation of the export port (`docs/31` P4.02).
//!
//! `docs/30` §17.7 puts the port in `snapclip-capture` and the encoder here, and the reason is a
//! dependency direction rather than taste: which encoder to use is a decision about the **output
//! format**, not a fact about capturing. `snapclip-capture` therefore never sees `png`, and this
//! crate is the only place that does.
//!
//! # Why the encoder writes into a shared buffer
//!
//! `docs/31` P4.02 prescribes `Encoder::stream_writer()` — the **borrowing** form — and then
//! `stream.finish()` followed by `writer.finish()`. That sequence cannot be written behind
//! `Box<dyn RowBandWriter>`, and the reason is worth writing down because it is a property of `png`
//! 0.18.1 rather than a preference:
//!
//! * `stream_writer(&mut self)` returns a `StreamWriter<'_>` that borrows the [`png::Writer`]. Inside
//!   one owned writer both would have to live in the same value, i.e. a self-referential struct, and
//!   the crates that make those safe (`self_cell`, `ouroboros`) are new dependencies (`N7`).
//! * `into_stream_writer(self)` does own it, but `StreamWriter::finish(self) -> Result<()>`
//!   (`png-0.18.1/src/encoder.rs:1606`) consumes the stream and **discards the inner `Writer`**, so
//!   there is no second value left to call `Writer::finish()` on.
//!
//! So `W` is a `'static` handle the sink keeps a clone of, `into_stream_writer()` owns the `Writer`,
//! and the bytes are read back out of the handle once the stream is finished. IEND still lands
//! exactly once: `impl Drop for Writer<W>` writes it (`encoder.rs:1115-1119`), which is the same
//! `write_iend()` that `Writer::finish()` would have called — only the error is dropped, and this
//! `Write` impl cannot fail. The cost is one uncontended lock per 4 KiB chunk, not per row, and the
//! encoder's own buffers are three rows plus that chunk (`StreamWriter`'s `prev_buf`/`curr_buf`/
//! `filtered_buf` are each one row), so the footprint does not grow with the image.
//!
//! # A short write cannot be a PNG
//!
//! The port allows `finish` to be called after fewer rows than `begin` was told about, and the
//! `#[cfg(test)]` contract sink in `export.rs` honours that. A PNG cannot: the height is in the
//! IHDR, which `write_header()` fixes before the first row (`F-12`), so an artifact carrying N rows
//! under a declared height of M is not a smaller image, it is a corrupt one. `finish` therefore
//! refuses a short write with `ExportError::Sink` instead of inventing rows or emitting a file that
//! pretends to be complete. The legitimate partial artifact — "this canvas is a prefix of the page"
//! — is produced by talking `begin` into the *shorter* height (`P1.21` trims the canvas before the
//! export starts), which is exactly what `AbortReason` labels.
//!
//! The refusal **names the reason it was given** (`P4.05`). That is not decoration: the reason
//! arrives as a parameter, so a message that omits it is the one place where the causal chain from
//! "the user pressed `Esc`" to "the file is short" gets broken — §26.2 rule 3 says a discarded result
//! is recorded together with its reason, and this is the only code that sees both.

use std::io::Write;
use std::sync::{Arc, Mutex};

use snapclip_capture::scroll::export::{
    AbortReason, Artifact, ExportError, ImageMeta, RowBandSink, RowBandWriter,
};

/// The buffer the encoder writes its chunks into, reachable from both sides of the stream.
///
/// `Arc<Mutex<_>>` rather than `Rc<RefCell<_>>`: `RowBandWriter` carries no `Send` bound either way,
/// but the lock is taken once per 4 KiB chunk and keeping the handle shareable costs nothing
/// measurable while keeping thread-move possible.
struct SharedVec(Arc<Mutex<Vec<u8>>>);

impl Write for SharedVec {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut sink = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sink.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn sink_error(error: impl std::fmt::Display) -> ExportError {
    ExportError::Sink(error.to_string())
}

/// Streams packed BGRA8 rows into a PNG artifact (`docs/30` §17.7).
///
/// The sink itself is configuration only, which is what lets it satisfy `RowBandSink: Send`; the
/// state that cannot cross a thread lives in the writer `begin` returns.
pub struct PngRowBandSink {
    compression: png::Compression,
    filter: png::Filter,
}

impl Default for PngRowBandSink {
    /// The parameters `P0.04` measured out of twelve combinations (`docs/30` §17.7.1).
    fn default() -> Self {
        Self {
            compression: png::Compression::Balanced,
            filter: png::Filter::Up,
        }
    }
}

impl PngRowBandSink {
    /// A sink using the parameters `P0.04` selected.
    pub fn new() -> Self {
        Self::default()
    }

    /// The zlib effort the sink hands the encoder.
    pub fn compression(&self) -> png::Compression {
        self.compression
    }

    /// The row filter the sink hands the encoder.
    pub fn filter(&self) -> png::Filter {
        self.filter
    }

    /// Overrides the row filter. `P4.02`'s test is the only caller; §17.7.1 fixes the default.
    pub fn set_filter(&mut self, filter: png::Filter) {
        self.filter = filter;
    }

    /// Overrides the compression level. Same standing as [`Self::set_filter`].
    pub fn set_compression(&mut self, compression: png::Compression) {
        self.compression = compression;
    }
}

impl RowBandSink for PngRowBandSink {
    fn begin(&mut self, meta: &ImageMeta) -> Result<Box<dyn RowBandWriter>, ExportError> {
        // `P4.04`: the size domain is `u64` and the format's is `u32`, so the conversion is checked
        // and refused. A truncating `as` here would produce an image that lies about its dimensions.
        let width = u32::try_from(meta.width).map_err(|_| ExportError::TooLarge {
            width: meta.width,
            height: meta.height,
        })?;
        let height = u32::try_from(meta.height).map_err(|_| ExportError::TooLarge {
            width: meta.width,
            height: meta.height,
        })?;

        let buffer = Arc::new(Mutex::new(Vec::new()));
        let mut encoder = png::Encoder::new(SharedVec(Arc::clone(&buffer)), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(self.compression);
        encoder.set_filter(self.filter);
        // The IHDR is written here, which is why the height is `begin`'s business (`F-12`).
        let writer = encoder.write_header().map_err(sink_error)?;
        let stream = writer.into_stream_writer().map_err(sink_error)?;

        Ok(Box::new(PngRowBandWriter {
            meta: *meta,
            buffer,
            stream: Some(stream),
            row: Vec::new(),
            next_row: 0,
        }))
    }
}

struct PngRowBandWriter {
    meta: ImageMeta,
    buffer: Arc<Mutex<Vec<u8>>>,
    /// `None` only after `finish`, which is the only thing that can take it.
    stream: Option<png::StreamWriter<'static, SharedVec>>,
    /// One row of RGBA scratch. This is the whole of the sink's per-row memory, and it is why the
    /// ceiling does not grow with the image (`G3`).
    row: Vec<u8>,
    next_row: u64,
}

impl RowBandWriter for PngRowBandWriter {
    fn write_rows(&mut self, first_row: u64, rows: &[u8]) -> Result<(), ExportError> {
        let row_bytes = self.meta.width * 4;
        let got = rows.len() as u64;
        if got % row_bytes != 0 {
            return Err(ExportError::RowLength {
                expected: row_bytes,
                got,
            });
        }
        if first_row != self.next_row {
            return Err(ExportError::OutOfOrder {
                first_row,
                expected: self.next_row,
            });
        }
        let count = got / row_bytes;
        if first_row + count > self.meta.height {
            return Err(ExportError::BeyondHeight {
                first_row,
                height: self.meta.height,
            });
        }

        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| ExportError::Sink("rows were written after the artifact closed".into()))?;
        // PNG has no BGRA, so the swap is the sink's job and it is done a row at a time: a whole
        // image's worth of RGBA in one buffer is the copy `P4.03` exists to delete.
        self.row.resize(row_bytes as usize, 0);
        for band in rows.chunks_exact(row_bytes as usize) {
            for (dst, src) in self
                .row
                .chunks_exact_mut(4)
                .zip(band.chunks_exact(4))
            {
                dst[0] = src[2];
                dst[1] = src[1];
                dst[2] = src[0];
                dst[3] = src[3];
            }
            stream.write_all(&self.row).map_err(sink_error)?;
        }
        self.next_row += count;
        Ok(())
    }

    fn finish(mut self: Box<Self>, outcome: Option<AbortReason>) -> Result<Artifact, ExportError> {
        if self.next_row != self.meta.height {
            // `docs/30` §26.2 rule 3: a discarded result is recorded together with its reason. The
            // reason is a parameter of this call, so the only way it can go missing is if this
            // message drops it — and "3 of 5 rows were written" on its own cannot be told apart from
            // a caller bug, a truncated file, or a session the user cancelled (`P4.05`).
            let why = match outcome {
                Some(reason) => reason.to_string(),
                None => "the export was cut short without a reason".to_string(),
            };
            return Err(ExportError::Sink(format!(
                "{why}; {} of {} rows were written, and a PNG's height is fixed in its header, so a \
                 short write cannot become a decodable file",
                self.next_row, self.meta.height
            )));
        }

        let stream = self
            .stream
            .take()
            .ok_or_else(|| ExportError::Sink("the artifact was already closed".into()))?;
        // `finish` flushes the trailing IDAT chunk; the `Writer` it owns writes IEND as it drops.
        stream.finish().map_err(sink_error)?;

        // The encoder dropped its handle with the stream, so this is normally the last one and the
        // bytes move out instead of being copied.
        let bytes = match Arc::try_unwrap(self.buffer) {
            Ok(buffer) => buffer
                .into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            Err(shared) => shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        };
        Ok(Artifact {
            bytes,
            rows: self.next_row,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapclip_capture::scroll::Axis;

    /// Packed BGRA8, the order the capture side hands rows over in (`export.rs`, §17.7).
    fn bgra(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                out.extend_from_slice(&[x as u8, y as u8, (x + y) as u8, 255]);
            }
        }
        out
    }

    /// The same pixels in the order a decoded RGBA8 PNG hands them back.
    fn rgba(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                out.extend_from_slice(&[(x + y) as u8, y as u8, x as u8, 255]);
            }
        }
        out
    }

    fn meta(width: u64, height: u64) -> ImageMeta {
        ImageMeta {
            width,
            height,
            length: height,
            axis: Axis::Vertical,
            dpr: 1,
        }
    }

    fn decode(bytes: &[u8]) -> (png::OutputInfo, Vec<u8>) {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        // Raised on purpose, and *not* because the default would fail: `DEV-7` measured a 101.7 MiB
        // artifact decoding fine under `png::Limits::default()`. The limit is a decoder-side budget
        // that ticks down as rows arrive, so raising it costs nothing and removes a variable from a
        // test whose subject is the encoder.
        decoder.set_limits(png::Limits { bytes: 1 << 30 });
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info, buf)
    }

    #[test]
    fn the_artifact_decodes_back_to_the_expected_pixels() {
        let (width, height) = (4u32, 6u32);
        let rows = bgra(width, height);
        let meta = meta(u64::from(width), u64::from(height));

        let mut sink = PngRowBandSink::new();
        let mut writer = sink.begin(&meta).unwrap();
        // Two bands, because the whole point of the port is that rows arrive as bands and not as one
        // buffer the size of the image.
        writer.write_rows(0, &rows[..(3 * 4 * 4) as usize]).unwrap();
        writer.write_rows(3, &rows[(3 * 4 * 4) as usize..]).unwrap();
        let artifact = writer.finish(None).unwrap();

        assert_eq!(artifact.rows, 6, "every row was written, so every row is reported");
        let (info, pixels) = decode(&artifact.bytes);
        assert_eq!((info.width, info.height), (width, height));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(
            pixels,
            rgba(width, height),
            "the sink converts packed BGRA rows to the RGBA the format has, one row at a time"
        );
    }

    #[test]
    fn the_selected_filter_is_the_one_p0_04_measured() {
        // `docs/30` §17.7.1: `P0.04` measured twelve combinations and picked these two. Assert the
        // choice is visible in the output rather than only in a constructor. `Filter::Up` subtracts
        // the pixel one row above, so a page whose bytes change *slowly* down the image leaves a
        // near-constant residual and has to beat `Filter::NoFilter` on size. (The first attempt at
        // this test used rows that were all *identical*, and there `NoFilter` won — deflate
        // back-references the repeated row better than it compresses a zero run. Vertical
        // correlation, not repetition, is what the filter is for.)
        let (width, height) = (64u32, 64u32);
        let mut rows = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rows.extend_from_slice(&[(x + 2 * y) as u8, y as u8, x as u8, 255]);
            }
        }
        let meta = meta(u64::from(width), u64::from(height));

        let with_up = {
            let mut sink = PngRowBandSink::new();
            let mut writer = sink.begin(&meta).unwrap();
            writer.write_rows(0, &rows).unwrap();
            writer.finish(None).unwrap()
        };
        let without = {
            let mut sink = PngRowBandSink::new();
            sink.set_filter(png::Filter::NoFilter);
            let mut writer = sink.begin(&meta).unwrap();
            writer.write_rows(0, &rows).unwrap();
            writer.finish(None).unwrap()
        };

        assert_eq!(PngRowBandSink::new().filter(), png::Filter::Up);
        // `png::Compression` has no `PartialEq`, so this is a pattern rather than an equality.
        assert!(
            matches!(
                PngRowBandSink::new().compression(),
                png::Compression::Balanced
            ),
            "P0.04 selected Compression::Balanced, got {:?}",
            PngRowBandSink::new().compression()
        );
        assert!(
            with_up.bytes.len() < without.bytes.len(),
            "Filter::Up must beat NoFilter on vertically uniform rows ({} vs {})",
            with_up.bytes.len(),
            without.bytes.len()
        );
    }

    /// `docs/30 §30.7`'s `u32` row reads "construct a size above `u32::MAX` → refused, not
    /// truncated". `ImageMeta` is the only place a dimension can be that large (a `Rect` holds
    /// `i32`), so the row is pinned here, at the sink that owns the format's limits.
    ///
    /// This was already true when `P4.04` opened: `begin` has converted with `u32::try_from` since
    /// `P4.02`, the task that made `ImageMeta` `u64`. It is therefore a **regression pin, not a
    /// RED** — the RED is `artifact_writer.rs`'s
    /// `an_oversized_dimension_is_rejected_before_the_first_byte`. "Before the first byte" is
    /// observable only by construction: the conversion is the first thing `begin` does, so
    /// `Encoder::new` and `write_header` are never reached for such a meta.
    #[test]
    fn an_oversized_dimension_is_refused_before_the_header() {
        for oversized in [meta(u64::from(u32::MAX) + 1, 1), meta(1, u64::from(u32::MAX) + 1)] {
            let error = match PngRowBandSink::new().begin(&oversized) {
                Ok(_) => panic!("{oversized:?} does not fit a PNG header and must be refused"),
                Err(error) => error,
            };
            assert_eq!(
                error,
                ExportError::TooLarge {
                    width: oversized.width,
                    height: oversized.height
                }
            );
            let message = error.to_string();
            assert!(
                message.contains(&oversized.width.to_string())
                    && message.contains(&oversized.height.to_string()),
                "the refusal must name the size it refused, got `{message}`"
            );
        }
    }

    /// `docs/30 §30.4`'s "上限三层" row reads "`MAX_LONG_IMAGE_PIXELS` injected small → `Partial`
    /// and **it is a legal PNG**". `P1.21` already pins the trimming; what that row adds for the
    /// encoder is that an export which was told to stop still closes its container, so the file the
    /// user is offered is one they can open.
    ///
    /// This was **already true** when `P4.05` opened, and it is the *correct* behaviour rather than a
    /// gap: when every declared row arrived, the outcome is a label on a complete file and the sink
    /// ignores it. What the sink did *not* do is the subject of
    /// [`Self::a_short_export_says_why_it_was_cut_short`] — this test is the pin that keeps the fix
    /// from turning "aborted" into "truncated".
    #[test]
    fn finish_with_abort_writes_a_complete_iend_and_the_file_decodes() {
        let (width, height) = (4u32, 5u32);
        let rows = bgra(width, height);
        let meta = meta(u64::from(width), u64::from(height));

        for reason in [
            AbortReason::Cancelled,
            AbortReason::MemoryLimit,
            AbortReason::ExportBudget,
        ] {
            let mut sink = PngRowBandSink::new();
            let mut writer = sink.begin(&meta).expect("a PNG of 4x5");
            writer.write_rows(0, &rows).expect("all five rows");
            let artifact = writer
                .finish(Some(reason))
                .unwrap_or_else(|error| panic!("{reason:?} must still close the artifact: {error}"));

            assert_eq!(
                artifact.rows, u64::from(height),
                "{reason:?}: every declared row arrived, so the file is complete"
            );
            let (info, pixels) = decode(&artifact.bytes);
            assert_eq!((info.width, info.height), (width, height), "{reason:?}");
            assert_eq!(info.color_type, png::ColorType::Rgba, "{reason:?}");
            assert_eq!(
                pixels,
                rgba(width, height),
                "{reason:?}: an aborted-but-complete export is byte-identical to an unlabelled one"
            );
        }
    }

    /// `docs/30 §30.4`'s "流式导出" row: "strictly increasing validation; out-of-order returns an
    /// error". A gap in a PNG's rows cannot be expressed — the height is in the IHDR — so the only
    /// honest answers are "refuse" or "lie".
    ///
    /// **A pin, not a RED**: `P4.02` wrote the check and `write_rows` has covered both a skip and a
    /// rewind since. `P4.05` pins it against the *real* encoder, which is where the row is about,
    /// and the port's own double covers the same contract in `export.rs`.
    #[test]
    fn a_skipped_row_range_is_an_error() {
        let (width, height) = (4u32, 6u32);
        let meta = meta(u64::from(width), u64::from(height));
        let mut sink = PngRowBandSink::new();
        let mut writer = sink.begin(&meta).expect("a PNG of 4x6");
        writer.write_rows(0, &bgra(width, 2)).expect("rows 0-1");

        assert_eq!(
            writer.write_rows(4, &bgra(width, 1)),
            Err(ExportError::OutOfOrder {
                first_row: 4,
                expected: 2
            }),
            "rows 2-3 never arrived, so row 4 must be refused rather than appended"
        );
        assert_eq!(
            writer.write_rows(1, &bgra(width, 1)),
            Err(ExportError::OutOfOrder {
                first_row: 1,
                expected: 2
            }),
            "rewinding is the same failure from the other side"
        );

        // The refused writes left no trace: the rest of the artifact is still writable in order.
        writer.write_rows(2, &bgra(width, 4)).expect("rows 2-5");
        let artifact = writer.finish(None).expect("a complete export");
        assert_eq!(artifact.rows, u64::from(height));
        let (info, _) = decode(&artifact.bytes);
        assert_eq!((info.width, info.height), (width, height));
    }

    /// `docs/30 §26.2` rule 3: "every discarded result is recorded **together with its reason**".
    ///
    /// A short write is refused (`§26.1`, and the module doc above argues why a PNG cannot be short).
    /// The refusal is correct; what it must not be is *silent about which abort it was given*. Three
    /// different things lead here — the user pressed `Esc`, the canvas hit its ceiling, the export
    /// budget ran out — and a message that says only "3 of 5 rows were written" forces the caller to
    /// guess, which is the guess §26.2 rule 3 exists to remove. The reason is already a parameter;
    /// today it is dropped on the floor (`_outcome`).
    #[test]
    fn a_short_export_says_why_it_was_cut_short() {
        for (reason, wording) in [
            (AbortReason::Cancelled, "cancelled"),
            (AbortReason::MemoryLimit, "memory"),
            (AbortReason::ExportBudget, "budget"),
        ] {
            let meta = meta(4, 5);
            let mut sink = PngRowBandSink::new();
            let mut writer = sink.begin(&meta).expect("a PNG of 4x5");
            writer.write_rows(0, &bgra(4, 3)).expect("rows 0-2");

            let error = match writer.finish(Some(reason)) {
                Ok(_) => panic!("{reason:?}: three of five rows is not a decodable PNG"),
                Err(error) => error,
            };
            let message = error.to_string();
            assert!(
                message.contains(wording),
                "the refusal must name the reason it was given ({reason:?}, so the word `{wording}`), \
                 got `{message}`"
            );
            assert!(
                message.contains('3') && message.contains('5'),
                "and it must still say how much arrived ({reason:?}), got `{message}`"
            );
        }
    }

    /// The shell is the first crate that can *name* the codes §26.3 fixes for the export path, so
    /// what this pins is the boundary: `ScrollDiagnosticCode` is a §27.1 `pub` type that lives in a
    /// `pub(crate)` module, and a `pub` enum inside a private module is unreachable from here until
    /// `scroll/mod.rs` re-exports it (the `Axis` precedent).
    ///
    /// The names are asserted here as well as in `session.rs` on purpose: the *log line* is what the
    /// shell emits, so the spelling is this crate's contract with `§26.4`'s channel.
    #[test]
    fn the_shell_can_name_the_export_diagnostics() {
        use snapclip_capture::scroll::ScrollDiagnosticCode;

        assert_eq!(
            ScrollDiagnosticCode::ExportTrimmed.as_str(),
            "export_trimmed"
        );
        assert_eq!(
            ScrollDiagnosticCode::ArtifactDiscarded.as_str(),
            "artifact_discarded"
        );
    }
}
