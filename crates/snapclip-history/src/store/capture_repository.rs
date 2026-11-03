//! Durable source references are separate from disposable SDR row thumbnails.
use super::*;
use crate::{artifact_store::CaptureArtifactStore, pixel_master::PixelMaster};
use snapclip_model::{CaptureSource, ImageDimensions, PublicationOrigin};
use std::io::Read;

pub(super) fn insert_capture(
    connection: &Connection,
    blobs: &BlobStore,
    artifacts: &CaptureArtifactStore,
    id: String,
    source: CaptureSource,
    thumbnail: Vec<u8>,
) -> Result<(), StoreError> {
    let invalid = |m: &str| StoreError::InvalidPublication(m.into());
    let metadata = &source.metadata;
    if id.is_empty()
        || id.len() > 256
        || metadata.width == 0
        || metadata.height == 0
        || source.artifact.dimensions
            != (ImageDimensions {
                width: metadata.width,
                height: metadata.height,
            })
    {
        return Err(invalid("capture identity or geometry is invalid"));
    }
    artifacts.verify_owned(&source.artifact)?;
    match source.artifact.mime.as_str() {
        "application/x-snapclip-pixels" => {
            let master = PixelMaster::open(&source.artifact)?;
            if master.metadata() != metadata {
                return Err(invalid(
                    "capture metadata differs from the immutable master",
                ));
            }
        }
        "image/png" => {
            if metadata.source.descriptor() != snapclip_model::pixels::PixelDescriptor::bgra8() {
                return Err(invalid("PNG8 source must be explicitly sRGB BGRA8"));
            }
            artifacts.verify(&source.artifact)?;
            let mut header = [0; 26];
            fs::File::open(&source.artifact.absolute_path)?.read_exact(&mut header)?;
            if crate::image::png_dimensions(&header) != Some((metadata.width, metadata.height))
                || header[24] != 8
            {
                return Err(invalid(
                    "PNG source geometry or depth differs from capture metadata",
                ));
            }
        }
        _ => return Err(invalid("unsupported capture source format")),
    }
    let Some((width, height)) = crate::image::png_dimensions(&thumbnail) else {
        return Err(invalid("capture thumbnail must be a real SDR PNG"));
    };
    if thumbnail.len() > 1024 * 1024 || width > 128 || height > 128 || thumbnail.get(24) != Some(&8)
    {
        return Err(invalid("capture thumbnail exceeds its SDR row budget"));
    }
    // Complete decode rejects truncated data before any row is published. Re-encode
    // our small SDR derivative to prevent carrying arbitrary metadata into the UI.
    let decoded =
        crate::image::decode_to_rgba8(&thumbnail).map_err(StoreError::InvalidPublication)?;
    let thumbnail = crate::image::encode_rgba_png(decoded.as_raw(), width, height)
        .map_err(StoreError::InvalidPublication)?;
    let hash = blake3::hash(&thumbnail).to_hex().to_string();
    let payload = PayloadRef {
        payload_id: format!("{id}-thumbnail"),
        content_hash: hash,
        kind: PayloadKind::Image,
        size_bytes: thumbnail.len() as u64,
        mime_type: Some("image/png".into()),
        image_dimensions: Some(ImageDimensions { width, height }),
    };
    let publication = Publication {
        publication_id: id,
        origin: PublicationOrigin::Capture,
        captured_at_unix_ms: metadata.captured_at_unix_ms,
        source_app: Some("SnapClip".into()),
        source_exe_path: None,
        payloads: vec![payload.clone()],
    };
    insert_publication(
        connection,
        blobs,
        publication,
        vec![PayloadData {
            payload,
            bytes: thumbnail,
        }],
        Some(&source),
    )
}

/// Called after the last SQLite owner was deleted. Never delete a replaced file
/// or an external path; a live decoded Arc remains owned by its window.
pub(super) fn remove_unreferenced(
    connection: &Connection,
    artifacts: &CaptureArtifactStore,
    source: &CaptureSource,
) -> Result<(), StoreError> {
    let retained: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM capture_sources WHERE artifact_path = ?1)",
        [source.artifact.absolute_path.to_string_lossy()],
        |row| row.get(0),
    )?;
    if !retained && source.artifact.absolute_path.exists() {
        artifacts.verify_owned(&source.artifact)?;
        artifacts.verify(&source.artifact)?;
        fs::remove_file(&source.artifact.absolute_path)?;
    }
    Ok(())
}
