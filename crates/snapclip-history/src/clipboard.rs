//! History policy over owned clipboard data: monitor exclusions and OCR downscaling stay here.
pub use snapclip_clipboard::windows::{
    ClipboardEvent, ClipboardUpdateListener, formats, source_app,
};
use snapclip_clipboard::windows::{formats::MAX_CLIPBOARD_BYTES, reader};
use snapclip_clipboard::{ImageEncoding, ReadPurpose};
use snapclip_model::{ImageDimensions, PayloadData, PayloadKind, PayloadRef};

pub struct ClipboardSnapshot {
    pub sequence: u32,
    pub owner: usize,
    pub foreground: usize,
    pub payloads: Vec<PayloadData>,
}
pub fn read_snapshot() -> Result<ClipboardSnapshot, String> {
    let snapshot = reader::read_snapshot(ReadPurpose::Monitor, None).map_err(|e| e.to_string())?;
    let mut captured = vec![];
    let mut total_size = 0;
    if !snapshot.is_excluded() {
        if let Some(text) = snapshot.text() {
            push_payload(
                &mut captured,
                &mut total_size,
                PayloadKind::Text,
                None,
                text.as_bytes().to_vec(),
            )?;
        }
        for (bytes, kind) in [
            (snapshot.html(), PayloadKind::Html),
            (snapshot.rtf(), PayloadKind::Rtf),
        ] {
            if let Some(bytes) = bytes {
                push_payload(&mut captured, &mut total_size, kind, None, bytes.to_vec())?;
            }
        }
        if !snapshot.files().is_empty() {
            push_payload(
                &mut captured,
                &mut total_size,
                PayloadKind::Files,
                None,
                serde_json::to_vec(snapshot.files()).map_err(|e| e.to_string())?,
            )?;
        }
        for (encoding, bytes) in snapshot.images() {
            let normalized = match encoding {
                ImageEncoding::Png => crate::clipboard_image::normalize_png(bytes),
                ImageEncoding::Dib => crate::clipboard_image::dib_to_png(bytes),
            };
            if let Ok((png, width, height)) = normalized {
                push_payload(
                    &mut captured,
                    &mut total_size,
                    PayloadKind::Image,
                    Some(ImageDimensions { width, height }),
                    png,
                )?;
                break;
            }
        }
    }
    Ok(ClipboardSnapshot {
        sequence: snapshot.completed_sequence(),
        owner: snapshot.owner(),
        foreground: snapshot.foreground(),
        payloads: captured,
    })
}
fn push_payload(
    captured: &mut Vec<PayloadData>,
    total_size: &mut usize,
    kind: PayloadKind,
    dimensions: Option<ImageDimensions>,
    bytes: Vec<u8>,
) -> Result<(), String> {
    *total_size = total_size
        .checked_add(bytes.len())
        .ok_or_else(|| "clipboard payload size overflow".to_string())?;
    if *total_size > MAX_CLIPBOARD_BYTES {
        return Err("clipboard snapshot exceeds the size limit".into());
    }
    // Sequence numbers change between reads, so identity is derived from the index
    // and content. The application layer assigns the final publication id.
    let index = captured.len();
    let payload = PayloadRef {
        payload_id: format!("payload-{index}-{}", blake3::hash(&bytes).to_hex()),
        content_hash: blake3::hash(&bytes).to_hex().to_string(),
        kind: kind.clone(),
        size_bytes: bytes.len() as u64,
        mime_type: Some(kind.default_mime_type().to_string()),
        image_dimensions: dimensions,
    };
    captured.push(PayloadData::new(payload, bytes));
    Ok(())
}
