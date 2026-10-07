//! Clip and publication rows (docs/23 T2.5): extracted from the store facade.

use super::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HistoryCursor {
    pub(super) created_at_unix_ms: i64,
    pub(super) id: String,
}


pub(super) fn insert_publication(
    connection: &Connection,
    blob_store: &BlobStore,
    publication: Publication,
    payloads: Vec<PayloadData>,
) -> Result<(), StoreError> {
    if publication.payloads.is_empty() || publication.payloads.len() != payloads.len() {
        return Err(StoreError::InvalidPublication(
            "every publication payload must have matching bytes".into(),
        ));
    }
    for (expected, input) in publication.payloads.iter().zip(&payloads) {
        if expected != &input.payload
            || expected.size_bytes != input.bytes.len() as u64
            || expected.content_hash != blake3::hash(&input.bytes).to_hex().as_str()
        {
            return Err(StoreError::InvalidPublication(
                "payload reference does not match its content".into(),
            ));
        }
    }

    let mut stored_blobs = Vec::with_capacity(payloads.len());
    for input in &payloads {
        stored_blobs.push(blob_store.put(&input.bytes)?);
    }

    let primary = payloads
        .first()
        .ok_or_else(|| StoreError::InvalidPublication("publication has no payloads".into()))?;
    let preview_text = payloads
        .iter()
        .find(|input| {
            input.payload.kind == PayloadKind::Text
                && !String::from_utf8_lossy(&input.bytes).trim().is_empty()
        })
        .map(|input| truncate_chars(&String::from_utf8_lossy(&input.bytes), PREVIEW_CHAR_LIMIT));
    let search_text = payloads
        .iter()
        .find(|input| {
            input.payload.kind == PayloadKind::Text
                && !String::from_utf8_lossy(&input.bytes).trim().is_empty()
        })
        .map(|input| String::from_utf8_lossy(&input.bytes).into_owned());
    // MVP: at most one image participates in OCR — first Image payload is canonical.
    let canonical_image_hash = payloads
        .iter()
        .find(|input| input.payload.kind == PayloadKind::Image)
        .map(|input| input.payload.content_hash.clone());

    let transaction = connection.unchecked_transaction()?;
    transaction.execute(
        "INSERT INTO clips(id, created_at_unix_ms, primary_kind, preview_text, source_app, source_exe_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            publication.publication_id,
            publication.captured_at_unix_ms,
            payload_kind_name(&primary.payload.kind),
            preview_text,
            publication.source_app,
            publication.source_exe_path,
        ],
    )?;

    for (input, blob) in payloads.iter().zip(stored_blobs) {
        transaction.execute(
            "INSERT OR IGNORE INTO payloads(id, content_hash, kind, size_bytes, storage_path,
                mime_type, width, height)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                input.payload.payload_id,
                input.payload.content_hash,
                payload_kind_name(&input.payload.kind),
                i64::try_from(input.payload.size_bytes).map_err(|_| {
                    StoreError::InvalidPublication("payload exceeds SQLite integer range".into())
                })?,
                blob.relative_path,
                input.payload.mime_type,
                input
                    .payload
                    .image_dimensions
                    .as_ref()
                    .map(|value| value.width),
                input
                    .payload
                    .image_dimensions
                    .as_ref()
                    .map(|value| value.height),
            ],
        )?;
        let canonical_id: String = transaction.query_row(
            "SELECT id FROM payloads WHERE content_hash = ?1 AND kind = ?2",
            params![
                input.payload.content_hash,
                payload_kind_name(&input.payload.kind)
            ],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO clip_payloads(clip_id, payload_id, role) VALUES (?1, ?2, ?3)",
            params![
                publication.publication_id,
                canonical_id,
                payload_kind_name(&input.payload.kind),
            ],
        )?;
    }

    transaction.execute(
        "INSERT INTO clip_search(clip_id, text_content, ocr_text, ocr_status, ocr_content_hash)
         VALUES (?1, ?2, NULL, 'none', ?3)
         ON CONFLICT(clip_id) DO UPDATE SET
            text_content = excluded.text_content,
            ocr_content_hash = COALESCE(excluded.ocr_content_hash, clip_search.ocr_content_hash)",
        params![
            publication.publication_id,
            search_text,
            canonical_image_hash
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn payload_kind_name(kind: &PayloadKind) -> &'static str {
    match kind {
        PayloadKind::Text => "text",
        PayloadKind::Html => "html",
        PayloadKind::Rtf => "rtf",
        PayloadKind::Image => "image",
        PayloadKind::Files => "files",
        PayloadKind::Other => "other",
    }
}

pub(super) fn parse_payload_kind(value: &str) -> Result<PayloadKind, StoreError> {
    match value {
        "text" => Ok(PayloadKind::Text),
        "html" => Ok(PayloadKind::Html),
        "rtf" => Ok(PayloadKind::Rtf),
        "image" => Ok(PayloadKind::Image),
        "files" => Ok(PayloadKind::Files),
        "other" => Ok(PayloadKind::Other),
        _ => Err(StoreError::InvalidPublication(format!(
            "unknown payload kind: {value}"
        ))),
    }
}

pub(super) fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

