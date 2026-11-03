//! Application composition for durable capture sources and full-precision replay.
//! The database stores references; windows retain immutable Arc pixels separately.
use snapclip_history::{pixel_master::PixelMaster, store::Store};
use snapclip_model::{CaptureMetadata, CaptureSource, pin::PinImage, pixels::PixelBuffer};
use std::{
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex},
};

/// Shared by clones of one capture panel; retries archive again only after failure.
pub(crate) struct CaptureArchive {
    metadata: CaptureMetadata,
    writer: crate::capture::HistoryArtifactWriter,
    stored: Mutex<bool>,
    events: Option<crate::events::EventBus>,
}
impl CaptureArchive {
    pub(crate) fn new(root: PathBuf, store: Store, metadata: CaptureMetadata) -> Self {
        Self {
            metadata,
            writer: crate::capture::HistoryArtifactWriter::new(root.join("artifacts/capture"))
                .with_history(store),
            stored: Mutex::new(false),
            events: None,
        }
    }
    pub(crate) fn with_events(mut self, events: crate::events::EventBus) -> Self {
        self.events = Some(events);
        self
    }
    pub(crate) fn persist(&self, pixels: &PixelBuffer) -> Result<(), String> {
        let mut stored = self.stored.lock().map_err(|e| e.to_string())?;
        if !*stored {
            self.writer
                .write_pixels(&self.metadata, pixels)
                .map_err(|e| e.to_string())?;
            *stored = true;
            if let Some(events) = &self.events {
                events.publish(snapclip_model::AppEvent::Capture(
                    snapclip_model::CaptureEvent {
                        session_id: self.metadata.session_id.clone(),
                        state: snapclip_model::CaptureState::Idle,
                        artifact: None,
                        error_code: None,
                        generation: 0,
                    },
                ));
            }
        }
        Ok(())
    }
}

/// Full source only. A row thumbnail is never a substitute on missing/corrupt or
/// over-budget input. A rejected interactive read leaves its durable master intact.
pub(crate) fn read_source(source: &CaptureSource) -> Result<PinImage, String> {
    const MAX_BYTES: u64 = 256 * 1024 * 1024;
    let metadata = &source.metadata;
    let row = metadata
        .source
        .descriptor()
        .row_bytes(metadata.width)
        .ok_or("history source row overflow")?;
    let len = (row as u64)
        .checked_mul(u64::from(metadata.height))
        .filter(|n| *n > 0 && *n <= MAX_BYTES)
        .ok_or(
            "原始图片超过 256 MiB 交互预算，无法打开或复制；历史保真源仍保留，缩略图不会代替原图",
        )?;
    let pixels = match source.artifact.mime.as_str() {
        "application/x-snapclip-pixels" => {
            let mut master = PixelMaster::open(&source.artifact).map_err(|e| e.to_string())?;
            if master.metadata() != metadata {
                return Err("history metadata differs from its master".into());
            }
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(len as usize)
                .map_err(|e| e.to_string())?;
            let band_rows = (4 * 1024 * 1024 / row).max(1) as u32;
            let mut first = 0;
            while first < metadata.height {
                let count = (metadata.height - first).min(band_rows);
                bytes.extend_from_slice(
                    master
                        .read_rows(first, count)
                        .map_err(|e| e.to_string())?
                        .as_bytes(),
                );
                first += count;
            }
            PixelBuffer::from_tight(
                metadata.width,
                metadata.height,
                metadata.source.descriptor(),
                bytes,
            )
            .map_err(str::to_owned)?
        }
        "image/png" => {
            if source.artifact.byte_len > 64 * 1024 * 1024 {
                return Err("history PNG exceeds 64 MiB encoded budget".into());
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&source.artifact.absolute_path)
                .map_err(|e| e.to_string())?
                .take(64 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() as u64 != source.artifact.byte_len
                || blake3::hash(&bytes).to_hex().as_str() != source.artifact.content_fingerprint
            {
                return Err("history PNG identity differs from its immutable reference".into());
            }
            let image = snapclip_pin::decode_image(&bytes)?;
            if image.pixels().size() != [metadata.width, metadata.height]
                || image.pixels().descriptor() != metadata.source.descriptor()
            {
                return Err("history PNG differs from its recorded capture".into());
            }
            return PinImage::from_pixels(metadata.source, image.pixels().clone())
                .ok_or("invalid history source encoding".into());
        }
        _ => return Err("unsupported history capture source".into()),
    };
    PinImage::from_pixels(metadata.source, Arc::new(pixels))
        .ok_or("invalid history source encoding".into())
}

/// Export from the durable source in bounded bands. Resolve the actual save
/// target before checking identity: save validation may trim the chosen stem.
pub(crate) fn export_source(
    source: &CaptureSource,
    path: &std::path::Path,
) -> Result<crate::save::service::SaveOutcome, String> {
    use crate::save::service::{ConflictPolicy, NoCancel, SaveRequest, commit_stream, target_of};
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
    {
        return Err("请选择 .png 文件名".into());
    }
    let request = SaveRequest {
        directory: path.parent().ok_or("保存路径无父目录")?.to_path_buf(),
        stem: path
            .file_stem()
            .ok_or("保存路径无文件名")?
            .to_string_lossy()
            .into_owned(),
        format: snapclip_capture::save_format::SaveFormat::Png,
        conflict: ConflictPolicy::Overwrite,
    };
    let target = target_of(&request).map_err(|e| e.message())?;
    if let (Ok(target), Ok(original)) = (
        target.canonicalize(),
        source.artifact.absolute_path.canonicalize(),
    ) && target == original
    {
        return Err("不能覆盖历史保真源，请选择其他路径".into());
    }
    commit_stream(&request, &mut NoCancel, |file| {
        crate::capture::pixel_export::write_capture_source_png(source, file)
            .map_err(std::io::Error::other)
    })
    .map_err(|e| e.message())
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
