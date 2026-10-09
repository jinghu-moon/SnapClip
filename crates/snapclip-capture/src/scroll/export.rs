//! The export seam of a scroll session (`docs/30` §17.7).
//!
//! A scroll session does not hand one image to an encoder; it hands **rows**. That is what makes
//! the memory ceiling independent of how far the page was scrolled (`G3`): the canvas stays in
//! `BandStore` (§22.1) and the encoder sees one band at a time, in the order the artifact needs.
//!
//! The port lives here rather than in [`super::ports`] on purpose. That file is "the platform
//! seams of a scroll session" — the seams a **Windows** implementation fills. This one is filled
//! by the **shell**, because which encoder to use is a decision about the output format, not a
//! fact about capturing (`DEV-2`). Keeping them apart is also what keeps `png` out of this crate:
//! the trait is ours, the implementation is the composition root's.
//!
//! Three constraints shape the signatures below, all from §17.7:
//!
//! 1. **The height is a precondition, not a later call.** `png`'s IHDR is fixed by
//!    `write_header()`, and `F-12` is the lesson that a container whose height is discovered while
//!    writing cannot be corrected afterwards. Putting `&ImageMeta` in `begin`'s signature makes
//!    "write a row before the height exists" **impossible to compile** rather than merely
//!    discouraged by a doc comment.
//! 2. **`write_rows` takes a row number.** Bands live in an LRU and can be spilled to disk, so a
//!    caller that needs to re-send an evicted band must be able to say *which* rows it is sending.
//!    **An interface that admits out-of-order writes plus an implementation that refuses them is one
//!    explicit trade-off, not an oversight**: the port has to be able to *name* a band's position
//!    (§22.1) while the artifact must never contain a gap, and a signature that could not express
//!    "these are rows 4000..5000" would force the caller to lie about it. So the number is a
//!    parameter and strictly-increasing is a rule the implementation enforces with an error rather
//!    than a silent reorder (`G12`).
//! 3. **`finish` carries an outcome.** Cancelling or hitting a budget mid-export must still leave a
//!    *complete* file — one the user can open — with the caller deciding whether to keep it.
//!
//! The rows are **packed BGRA8**: `width * 4` bytes each, no padding. The canvas has exactly one
//! pixel format and the observation layer already refuses anything but packed frames
//! (`ObservationError::NotPacked`), so an earlier revision's `stride` and `format` fields would
//! have been two ways to say the same thing.
//!
//! Like [`super::ports`], this file is a vocabulary, and part of it is read by code that does not
//! exist yet: `ExportError::Sink` is produced by the shell's encoder (`P4.02`) and
//! `ExportError::TooLarge`/`BeyondHeight`/`RowLength` by the checks `P4.04`/`P4.05` move into the
//! real sink. Narrowing the vocabulary to what today's call sites construct would make each of
//! those a breaking change later, for no gain now.
//!
//! Everything here is **`pub`**, and that is a `P4.02` correction rather than a preference. `P4.01`
//! declared the port `pub(crate)` and the shell could then not implement it at all — a port the
//! composition root cannot name is not a port. It is the same mistake `DEV-54` recorded for §27.1's
//! public boundary types. The visibility widening lives here, at the seam, rather than in
//! `lib.rs`: a re-export would have hidden which module actually owns the contract.
#![allow(dead_code)]

use super::observation::Axis;

/// Everything an export needs to know before the first row exists (`docs/30` §17.7).
///
/// Dimensions are `u64`, not `u32` (`P4.04`): the session's size domain is `u64` throughout, and a
/// `u32` field would make "reject a size that does not fit `u32`" impossible to even construct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageMeta {
    /// Pixels across the scrolling direction — the artifact's width.
    pub width: u64,
    /// Pixels along the scrolling direction — the artifact's height, fixed at `begin`.
    pub height: u64,
    /// The session's total scroll length, which may exceed `height` when the canvas was capped
    /// (§17.6 layer 2). Carried for diagnostics, not for encoding.
    pub length: u64,
    /// Which way the content was scrolled.
    pub axis: Axis,
    /// Device pixel ratio, for the consumer that has to turn rows back into CSS pixels.
    pub dpr: u32,
}

/// Why an export stopped before it wrote every row (`docs/30` §17.7 constraint 3, `P4.05`).
///
/// This is **not** `session::StopReason`. A session can stop for a dozen reasons and still export
/// every row it has; a `UserStopped` session produces a complete file of a partial canvas. These
/// three are the reasons an *export* itself was cut short, and they are the only three the writer
/// has to be able to finish anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortReason {
    /// The user cancelled while rows were still going out.
    Cancelled,
    /// The canvas hit its memory ceiling and the remaining rows do not exist.
    MemoryLimit,
    /// The export budget (§17.6 layer 3) was exhausted.
    ExportBudget,
}

/// What an export produced: the bytes, and how many rows actually made it in.
///
/// `rows` is the writer's own count, not the caller's — the writer is the only one that knows. It
/// can be smaller than [`ImageMeta::height`] exactly when `finish` was given an [`AbortReason`],
/// and the difference is what a UI reports as "N of M rows".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// The encoded artifact, ready to be written to a store.
    pub bytes: Vec<u8>,
    /// Rows present in `bytes`.
    pub rows: u64,
}

/// Everything that can go wrong while exporting rows (`docs/30` §26.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    /// A dimension does not fit the format's `u32` header field. Refused, **never truncated**
    /// (`P4.04`): a silently shortened image is worse than an error.
    TooLarge { width: u64, height: u64 },
    /// Rows arrived out of order, or a range was skipped. The expected number is the one the
    /// writer would have accepted.
    OutOfOrder { first_row: u64, expected: u64 },
    /// A range starts past the height declared at `begin`.
    BeyondHeight { first_row: u64, height: u64 },
    /// The byte slice is not a whole number of `width`-wide rows.
    RowLength { expected: u64, got: u64 },
    /// The implementation failed — a full disk, a closed handle. Carries the sink's own words.
    Sink(String),
}

/// Sentences, like [`super::observation::ObservationError`] and [`super::ports::FrameError`].
///
/// Added by `P4.03`, which is the first production call site of this port: the shell has to turn a
/// refusal into a message, and `CaptureError::EncodeFailed` takes a `String`. Without this the call
/// site would have had to format the `Debug` of an enum into something a user could read.
impl core::fmt::Display for ExportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExportError::TooLarge { width, height } => write!(
                f,
                "a {width}x{height} artifact does not fit the format's u32 header fields"
            ),
            ExportError::OutOfOrder { first_row, expected } => write!(
                f,
                "rows must arrive in order: expected row {expected}, got row {first_row}"
            ),
            ExportError::BeyondHeight { first_row, height } => write!(
                f,
                "row {first_row} is past the {height} rows declared at begin"
            ),
            ExportError::RowLength { expected, got } => write!(
                f,
                "rows are {expected} bytes each, and {got} is not a whole number of them"
            ),
            ExportError::Sink(message) => write!(f, "the export sink failed: {message}"),
        }
    }
}

/// Opens one export (`docs/30` §17.7).
///
/// `Send` because the sink is moved to the export worker; the writer it returns is not, because it
/// is created and consumed on that same thread and has no business crossing one.
pub trait RowBandSink: Send {
    /// Fixes the artifact's shape and returns the writer that fills it.
    ///
    /// The height is decided **here**, which is why a caller cannot reach [`RowBandWriter`] without
    /// having decided it.
    fn begin(&mut self, meta: &ImageMeta) -> Result<Box<dyn RowBandWriter>, ExportError>;
}

/// Receives the rows of one artifact, in strictly increasing order (`docs/30` §17.7).
pub trait RowBandWriter {
    /// Writes `rows` (a whole number of packed BGRA8 rows, `width * 4` bytes each) starting at
    /// `first_row`.
    ///
    /// `first_row` must equal the number of rows written so far. Skipping or rewinding is an
    /// error, not a reordering: a gap would produce an image that silently lies about its content.
    fn write_rows(&mut self, first_row: u64, rows: &[u8]) -> Result<(), ExportError>;

    /// Closes the artifact.
    ///
    /// `outcome` is `Some` when the export was cut short. The artifact is still closed properly —
    /// the caller decides whether to keep a short file, which is the only way "cancelled" and
    /// "over budget" can produce something a user can open rather than half a file.
    fn finish(self: Box<Self>, outcome: Option<AbortReason>) -> Result<Artifact, ExportError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `#[cfg(test)]` sink that emits a self-describing container instead of a PNG.
    ///
    /// `docs/30:3000` (`DEV-30`) is why this exists: `png` is deliberately not a dependency of this
    /// crate, so "the artifact decodes back" cannot be asserted against a real PNG here. What *can*
    /// be asserted here is the **contract** — that the shape is fixed at `begin`, that out-of-order
    /// rows are refused, and that an aborted export still closes the container. `P4.02`/`P4.05`
    /// repeat the decodability assertion against the real encoder in the shell.
    const MAGIC: [u8; 4] = *b"SNCB";
    const TERMINATOR: [u8; 4] = *b"DNEI";
    /// `MAGIC` + three `u64` fields, i.e. where the pixel rows start.
    const HEADER_LEN: u64 = 28;

    struct ContractSink;

    struct MemoryWriter {
        meta: ImageMeta,
        rows: Vec<u8>,
    }

    impl RowBandSink for ContractSink {
        fn begin(&mut self, meta: &ImageMeta) -> Result<Box<dyn RowBandWriter>, ExportError> {
            if meta.width > u64::from(u32::MAX) || meta.height > u64::from(u32::MAX) {
                return Err(ExportError::TooLarge {
                    width: meta.width,
                    height: meta.height,
                });
            }
            Ok(Box::new(MemoryWriter {
                meta: *meta,
                rows: Vec::new(),
            }))
        }
    }

    impl RowBandWriter for MemoryWriter {
        fn write_rows(&mut self, first_row: u64, rows: &[u8]) -> Result<(), ExportError> {
            let row_bytes = self.meta.width * 4;
            let written = self.rows.len() as u64 / row_bytes;
            if first_row != written {
                return Err(ExportError::OutOfOrder {
                    first_row,
                    expected: written,
                });
            }
            if rows.len() as u64 % row_bytes != 0 {
                return Err(ExportError::RowLength {
                    expected: row_bytes,
                    got: rows.len() as u64,
                });
            }
            let arriving = rows.len() as u64 / row_bytes;
            if written + arriving > self.meta.height {
                return Err(ExportError::BeyondHeight {
                    first_row,
                    height: self.meta.height,
                });
            }
            self.rows.extend_from_slice(rows);
            Ok(())
        }

        fn finish(self: Box<Self>, _outcome: Option<AbortReason>) -> Result<Artifact, ExportError> {
            let row_bytes = self.meta.width * 4;
            let rows = self.rows.len() as u64 / row_bytes;
            let mut bytes = Vec::with_capacity((HEADER_LEN + self.rows.len() as u64 + 4) as usize);
            bytes.extend_from_slice(&MAGIC);
            bytes.extend_from_slice(&self.meta.width.to_le_bytes());
            bytes.extend_from_slice(&self.meta.height.to_le_bytes());
            bytes.extend_from_slice(&rows.to_le_bytes());
            bytes.extend_from_slice(&self.rows);
            bytes.extend_from_slice(&TERMINATOR);
            Ok(Artifact { bytes, rows })
        }
    }

    /// The container's own header, read back the way a real decoder would.
    #[derive(Debug, PartialEq, Eq)]
    struct Header {
        width: u64,
        height: u64,
        rows: u64,
    }

    fn decode(artifact: &Artifact) -> Result<Header, String> {
        let bytes = &artifact.bytes;
        if (bytes.len() as u64) < HEADER_LEN + 4 {
            return Err(format!("too short to be a container: {} bytes", bytes.len()));
        }
        if bytes[..4] != MAGIC {
            return Err("the magic is missing".to_string());
        }
        if bytes[bytes.len() - 4..] != TERMINATOR {
            return Err("the terminator is missing, so the container was never closed".to_string());
        }
        let read = |at: usize| {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes[at..at + 8]);
            u64::from_le_bytes(buf)
        };
        let header = Header {
            width: read(4),
            height: read(12),
            rows: read(20),
        };
        let expected = HEADER_LEN + header.width * 4 * header.rows + 4;
        if bytes.len() as u64 != expected {
            return Err(format!(
                "the container declares {} rows of {} px but holds {} bytes, not {}",
                header.rows,
                header.width,
                bytes.len(),
                expected
            ));
        }
        Ok(header)
    }

    fn meta_of(width: u64, height: u64) -> ImageMeta {
        ImageMeta {
            width,
            height,
            length: height,
            axis: Axis::Vertical,
            dpr: 1,
        }
    }

    fn rows_of(width: u64, count: u64, seed: u8) -> Vec<u8> {
        (0..width * 4 * count)
            .map(|i| seed.wrapping_add(i as u8))
            .collect()
    }

    fn open(meta: &ImageMeta) -> Box<dyn RowBandWriter> {
        ContractSink
            .begin(meta)
            .expect("the contract sink accepts any size that fits the format")
    }

    #[test]
    fn the_height_must_be_known_before_the_first_row() {
        // The type makes the alternative unwritable: `RowBandWriter` has no method that could
        // establish or change the height, and the only way to obtain one is `begin(&ImageMeta)`,
        // which cannot be called without a height. What is observable is the consequence — the
        // container's height comes from `begin` and not from the number of rows that arrived.
        let meta = meta_of(2, 3);
        let mut writer = open(&meta);
        writer
            .write_rows(0, &rows_of(2, 1, 7))
            .expect("the first row of a three-row artifact");

        let artifact = writer
            .finish(Some(AbortReason::Cancelled))
            .expect("an aborted export still closes the container");
        let header = decode(&artifact).expect("the aborted container is still complete");

        assert_eq!(
            header.height, 3,
            "the height was declared at begin and cannot be revised by the rows that showed up"
        );
        assert_eq!(header.rows, 1, "one row was written");
        assert_eq!(artifact.rows, 1);
    }

    #[test]
    fn writing_rows_out_of_order_is_rejected() {
        let meta = meta_of(2, 4);
        let mut writer = open(&meta);
        writer.write_rows(0, &rows_of(2, 2, 1)).expect("rows 0-1");

        // A gap: the writer is at row 2 and is told to write row 3. Silently appending here would
        // produce an artifact whose rows do not correspond to the row numbers the caller asked for.
        assert_eq!(
            writer.write_rows(3, &rows_of(2, 1, 9)),
            Err(ExportError::OutOfOrder {
                first_row: 3,
                expected: 2
            }),
            "a skipped range must be refused, not appended"
        );

        // A rewind is the same failure from the other side.
        assert_eq!(
            writer.write_rows(0, &rows_of(2, 1, 9)),
            Err(ExportError::OutOfOrder {
                first_row: 0,
                expected: 2
            }),
            "rewinding must be refused too"
        );

        // And the refused writes left no trace.
        writer.write_rows(2, &rows_of(2, 2, 5)).expect("rows 2-3");
        let artifact = writer.finish(None).expect("a complete export");
        let header = decode(&artifact).expect("decodable");
        assert_eq!(header.rows, 4);
        assert_eq!(artifact.rows, 4);
    }

    #[test]
    fn finish_after_abort_still_produces_a_decodable_artifact() {
        for reason in [
            AbortReason::Cancelled,
            AbortReason::MemoryLimit,
            AbortReason::ExportBudget,
        ] {
            let meta = meta_of(3, 5);
            let mut writer = open(&meta);
            writer.write_rows(0, &rows_of(3, 2, 11)).expect("rows 0-1");

            let artifact = writer
                .finish(Some(reason))
                .unwrap_or_else(|error| panic!("{reason:?} must still close the artifact: {error:?}"));
            let header = decode(&artifact)
                .unwrap_or_else(|error| panic!("{reason:?} produced a broken container: {error}"));

            assert_eq!(header.width, 3, "{reason:?}");
            assert_eq!(header.height, 5, "{reason:?}");
            assert_eq!(header.rows, 2, "{reason:?} wrote two of five rows");
        }
    }
}
