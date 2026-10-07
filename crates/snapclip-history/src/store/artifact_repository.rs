//! Payload bytes and blob housekeeping (docs/23 T2.5).

use super::*;


pub(super) fn read_payload_bytes(
    connection: &Connection,
    blob_store: &BlobStore,
    content_hash: &str,
    kind: PayloadKind,
) -> Result<Vec<u8>, StoreError> {
    let exists: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM payloads WHERE content_hash = ?1 AND kind = ?2",
            params![content_hash, payload_kind_name(&kind)],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Err(StoreError::InvalidPublication(
            "payload not found for content hash".into(),
        ));
    }
    // Content-addressed read; never trust external paths.
    blob_store.read(content_hash)
}

pub(super) fn sweep_orphans(connection: &Connection, blob_store: &BlobStore) -> Result<(), StoreError> {
    let mut statement = connection.prepare("SELECT storage_path FROM payloads")?;
    let referenced = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<HashSet<_>, _>>()?;
    blob_store.remove_orphans(&referenced)?;
    Ok(())
}

/// Delete one clip, then everything only it was keeping alive.
///
/// Order matters: `clips` first (its `clip_payloads` and `clip_search` rows cascade, and the
/// FTS delete trigger fires), then the payload rows nothing references any more, then the
/// blob files those rows pointed at. Skipping the middle step would leak a payload row per
/// deleted clip forever, and skipping the last would leak its bytes.
pub(super) fn delete_publication(
    connection: &Connection,
    blob_store: &BlobStore,
    clip_id: &str,
) -> Result<bool, StoreError> {
    let transaction = connection.unchecked_transaction()?;
    let removed = transaction.execute("DELETE FROM clips WHERE id = ?1", params![clip_id])?;
    if removed == 0 {
        // Nothing was there; the cursor/report path does not need to know more than that.
        return Ok(false);
    }
    transaction.execute(
        "DELETE FROM payloads WHERE id NOT IN (SELECT payload_id FROM clip_payloads)",
        [],
    )?;
    transaction.commit()?;
    sweep_orphans(connection, blob_store)?;
    Ok(true)
}
