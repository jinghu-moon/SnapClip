//! Capture pixel masters, version 1. Independent compressed row tiles preserve
//! original channel bits and allow bounded random-row reads. This is storage,
//! not an image codec: PNG exports and display proxies are separate artifacts.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::{Compression, bufread::ZlibDecoder, write::ZlibEncoder};
use snapclip_model::{ArtifactRef, CaptureMetadata, ImageDimensions, pixels::PixelBuffer};

use crate::{StoreError, artifact_store::CaptureArtifactStore};

const MAGIC: &[u8; 8] = b"SCPX\x01\0\0\0";
const TILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_READ_BYTES: usize = 64 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_TILES: u32 = 262_144;
const MIME: &str = "application/x-snapclip-pixels";
static NEXT_MASTER: AtomicU64 = AtomicU64::new(0);

fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::InvalidPublication(message.into())
}
fn geometry(metadata: &CaptureMetadata) -> Result<(usize, u32, u32), StoreError> {
    let row = metadata
        .source
        .descriptor()
        .row_bytes(metadata.width)
        .ok_or_else(|| invalid("pixel master row size overflow"))?;
    if row == 0 || row > TILE_BYTES || metadata.height == 0 {
        return Err(invalid(
            "pixel master requires nonempty dimensions and rows no larger than 4 MiB",
        ));
    }
    let tile_rows = (TILE_BYTES / row) as u32;
    let tiles = metadata.height.div_ceil(tile_rows);
    if tiles > MAX_TILES {
        return Err(invalid("pixel master tile index exceeds its budget"));
    }
    Ok((row, tile_rows, tiles))
}
struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn write_part(file: &mut File, hash: &mut blake3::Hasher, bytes: &[u8]) -> Result<(), StoreError> {
    file.write_all(bytes)?;
    hash.update(bytes);
    Ok(())
}

impl CaptureArtifactStore {
    /// Atomically publish a source-format master. The callback supplies one bounded
    /// tile at a time and must retain its source generation for the whole write.
    /// Failure removes this write's temporary file; it never clears user caches.
    pub fn write_pixel_master(
        &self,
        metadata: &CaptureMetadata,
        mut rows: impl FnMut(u32, u32) -> Result<PixelBuffer, StoreError>,
    ) -> Result<ArtifactRef, StoreError> {
        let (_, tile_rows, _) = geometry(metadata)?;
        let session = &metadata.session_id;
        if session.is_empty()
            || session.len() > 128
            || !session
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(invalid("pixel master session id is not a file name"));
        }
        let header = serde_json::to_vec(metadata).map_err(|e| invalid(e.to_string()))?;
        if header.len() > MAX_HEADER_BYTES {
            return Err(invalid("pixel master metadata exceeds its budget"));
        }
        fs::create_dir_all(self.root())?;
        let sequence = NEXT_MASTER.fetch_add(1, Ordering::Relaxed);
        let path = self
            .root()
            .join(format!("{session}-{}-{sequence}.scpx", std::process::id()));
        if path.exists() {
            return Err(invalid("pixel master destination already exists"));
        }
        let temporary = path.with_extension("scpx.tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let pending = PendingFile(temporary);
        let mut hash = blake3::Hasher::new();
        let written = (|| {
            write_part(&mut file, &mut hash, MAGIC)?;
            write_part(&mut file, &mut hash, &(header.len() as u32).to_le_bytes())?;
            write_part(&mut file, &mut hash, &header)?;
            write_part(&mut file, &mut hash, blake3::hash(&header).as_bytes())?;
            let mut first = 0;
            while first < metadata.height {
                let count = (metadata.height - first).min(tile_rows);
                let pixels = rows(first, count)?;
                if pixels.size() != [metadata.width, count]
                    || pixels.descriptor() != metadata.source.descriptor()
                {
                    return Err(invalid(
                        "pixel master tile differs from source geometry or encoding",
                    ));
                }
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
                encoder.write_all(pixels.as_bytes())?;
                let compressed = encoder.finish()?;
                write_part(
                    &mut file,
                    &mut hash,
                    &(compressed.len() as u32).to_le_bytes(),
                )?;
                write_part(
                    &mut file,
                    &mut hash,
                    blake3::hash(pixels.as_bytes()).as_bytes(),
                )?;
                write_part(&mut file, &mut hash, &compressed)?;
                first += count;
            }
            file.sync_all()?;
            file.stream_position().map_err(StoreError::from)
        })();
        // Windows cannot rename/delete a file while this write handle remains open.
        drop(file);
        let byte_len = written?;
        fs::rename(&pending.0, &path)?;
        Ok(ArtifactRef {
            absolute_path: path,
            mime: MIME.into(),
            dimensions: ImageDimensions {
                width: metadata.width,
                height: metadata.height,
            },
            byte_len,
            content_fingerprint: hash.finalize().to_hex().to_string(),
        })
    }
}

struct Tile {
    offset: u64,
    compressed_len: u32,
    hash: [u8; 32],
}
/// An open source master. Opening verifies the header and tile bounds; each row
/// read verifies the complete touched tiles. Untouched tiles are not decoded.
pub struct PixelMaster {
    file: File,
    metadata: CaptureMetadata,
    row_bytes: usize,
    tile_rows: u32,
    tiles: Vec<Tile>,
}
impl PixelMaster {
    /// Accept only the current version. Older development caches need explicit
    /// regeneration; there is no heuristic reader or automatic cache deletion.
    pub fn open(reference: &ArtifactRef) -> Result<Self, StoreError> {
        if reference.mime != MIME {
            return Err(invalid("artifact is not a pixel master"));
        }
        let mut file = File::open(&reference.absolute_path)?;
        let length = file.metadata()?.len();
        if length != reference.byte_len {
            return Err(invalid("pixel master length differs from its reference"));
        }
        // Verify encoded identity with bounded scratch; no decoded image allocation.
        let mut fingerprint = blake3::Hasher::new();
        let mut scratch = [0; 64 * 1024];
        loop {
            let read = file.read(&mut scratch)?;
            if read == 0 {
                break;
            }
            fingerprint.update(&scratch[..read]);
        }
        if fingerprint.finalize().to_hex().as_str() != reference.content_fingerprint {
            return Err(invalid(
                "pixel master fingerprint differs from its reference",
            ));
        }
        file.seek(SeekFrom::Start(0))?;
        let mut magic = [0; 8];
        file.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(invalid("unsupported pixel master version"));
        }
        let mut word = [0; 4];
        file.read_exact(&mut word)?;
        let header_len = u32::from_le_bytes(word) as usize;
        if header_len == 0 || header_len > MAX_HEADER_BYTES {
            return Err(invalid("invalid pixel master header size"));
        }
        let mut header = vec![0; header_len];
        file.read_exact(&mut header)?;
        let mut digest = [0; 32];
        file.read_exact(&mut digest)?;
        if blake3::hash(&header).as_bytes() != &digest {
            return Err(invalid("pixel master metadata checksum failed"));
        }
        let metadata: CaptureMetadata =
            serde_json::from_slice(&header).map_err(|e| invalid(e.to_string()))?;
        if reference.dimensions
            != (ImageDimensions {
                width: metadata.width,
                height: metadata.height,
            })
        {
            return Err(invalid("pixel master dimensions differ from its reference"));
        }
        let (row_bytes, tile_rows, tile_count) = geometry(&metadata)?;
        let mut tiles = Vec::with_capacity(tile_count as usize);
        for ix in 0..tile_count {
            file.read_exact(&mut word)?;
            let compressed_len = u32::from_le_bytes(word);
            let mut hash = [0; 32];
            file.read_exact(&mut hash)?;
            let offset = file.stream_position()?;
            let rows = (metadata.height - ix * tile_rows).min(tile_rows) as usize;
            let raw = rows * row_bytes;
            let end = offset
                .checked_add(u64::from(compressed_len))
                .ok_or_else(|| invalid("pixel master tile offset overflow"))?;
            if compressed_len < 6 || compressed_len as usize > raw + raw / 16 + 1024 || end > length
            {
                return Err(invalid("pixel master compressed tile exceeds its bounds"));
            }
            tiles.push(Tile {
                offset,
                compressed_len,
                hash,
            });
            file.seek(SeekFrom::Start(end))?;
        }
        if file.stream_position()? != length {
            return Err(invalid("pixel master has trailing data"));
        }
        Ok(Self {
            file,
            metadata,
            row_bytes,
            tile_rows,
            tiles,
        })
    }
    pub fn metadata(&self) -> &CaptureMetadata {
        &self.metadata
    }
    /// Materialize at most 64 MiB per request. Long exports iterate bands; this
    /// guard prevents an untrusted row count from allocating a whole long image.
    pub fn read_rows(&mut self, first: u32, count: u32) -> Result<PixelBuffer, StoreError> {
        let end = first
            .checked_add(count)
            .filter(|end| *end <= self.metadata.height)
            .ok_or_else(|| invalid("pixel master row range is out of bounds"))?;
        let size = (count as usize)
            .checked_mul(self.row_bytes)
            .filter(|size| *size > 0 && *size <= MAX_READ_BYTES)
            .ok_or_else(|| invalid("pixel master row read exceeds its allocation budget"))?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(size)
            .map_err(|e| invalid(e.to_string()))?;
        for ix in first / self.tile_rows..=(end - 1) / self.tile_rows {
            let tile = &self.tiles[ix as usize];
            self.file.seek(SeekFrom::Start(tile.offset))?;
            let mut compressed = vec![0; tile.compressed_len as usize];
            self.file.read_exact(&mut compressed)?;
            let tile_first = ix * self.tile_rows;
            let tile_count = (self.metadata.height - tile_first).min(self.tile_rows);
            let expected = tile_count as usize * self.row_bytes;
            let mut decoder = ZlibDecoder::new(compressed.as_slice());
            let mut bytes = vec![0; expected];
            decoder.read_exact(&mut bytes)?;
            // Probe EOF without growing a 4 MiB raw tile to an 8 MiB Vec.
            let mut extra = [0];
            if decoder.read(&mut extra)? != 0
                || decoder.total_in() != compressed.len() as u64
                || blake3::hash(&bytes).as_bytes() != &tile.hash
            {
                return Err(invalid(
                    "pixel master tile failed its size or checksum validation",
                ));
            }
            let pixels = PixelBuffer::from_tight(
                self.metadata.width,
                tile_count,
                self.metadata.source.descriptor(),
                bytes,
            )
            .map_err(invalid)?;
            let from = first.max(tile_first) - tile_first;
            let to = end.min(tile_first + tile_count) - tile_first;
            output.extend_from_slice(
                &pixels.as_bytes()[from as usize * self.row_bytes..to as usize * self.row_bytes],
            );
        }
        PixelBuffer::from_tight(
            self.metadata.width,
            count,
            self.metadata.source.descriptor(),
            output,
        )
        .map_err(invalid)
    }
}
