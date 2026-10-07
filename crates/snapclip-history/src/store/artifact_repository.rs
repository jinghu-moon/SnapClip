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
