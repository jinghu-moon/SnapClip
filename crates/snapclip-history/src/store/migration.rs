//! Schema migrations (docs/23 T2.5).

use super::*;


pub(super) fn migrate(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at_unix_ms INTEGER NOT NULL
        );",
    )?;
    let current: i64 = connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    let now = unix_time_ms();
    let transaction = connection.unchecked_transaction()?;

    if current < 1 {
        transaction.execute_batch(
            "CREATE TABLE clips (
                id TEXT PRIMARY KEY,
                created_at_unix_ms INTEGER NOT NULL,
                primary_kind TEXT NOT NULL,
                preview_text TEXT,
                source_app TEXT
            );
            CREATE INDEX clips_created_id ON clips(created_at_unix_ms DESC, id DESC);

            CREATE TABLE payloads (
                id TEXT PRIMARY KEY,
                content_hash TEXT NOT NULL,
                kind TEXT NOT NULL,
                size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
                storage_path TEXT NOT NULL,
                mime_type TEXT,
                width INTEGER,
                height INTEGER,
                UNIQUE(content_hash, kind)
            );

            CREATE TABLE clip_payloads (
                clip_id TEXT NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
                payload_id TEXT NOT NULL REFERENCES payloads(id),
                role TEXT NOT NULL,
                PRIMARY KEY (clip_id, payload_id, role)
            );
            CREATE INDEX clip_payloads_payload_id ON clip_payloads(payload_id);

            CREATE TABLE clip_search (
                clip_id TEXT PRIMARY KEY REFERENCES clips(id) ON DELETE CASCADE,
                text_content TEXT,
                ocr_text TEXT
            );
            CREATE VIRTUAL TABLE clip_search_fts USING fts5(
                text_content, ocr_text,
                content='clip_search',
                content_rowid='rowid',
                tokenize='trigram'
            );
            CREATE TRIGGER clip_search_ai AFTER INSERT ON clip_search BEGIN
                INSERT INTO clip_search_fts(rowid, text_content, ocr_text)
                VALUES (new.rowid, new.text_content, new.ocr_text);
            END;
            CREATE TRIGGER clip_search_ad AFTER DELETE ON clip_search BEGIN
                INSERT INTO clip_search_fts(clip_search_fts, rowid, text_content, ocr_text)
                VALUES ('delete', old.rowid, old.text_content, old.ocr_text);
            END;
            CREATE TRIGGER clip_search_au AFTER UPDATE OF text_content, ocr_text ON clip_search BEGIN
                INSERT INTO clip_search_fts(clip_search_fts, rowid, text_content, ocr_text)
                VALUES ('delete', old.rowid, old.text_content, old.ocr_text);
                INSERT INTO clip_search_fts(rowid, text_content, ocr_text)
                VALUES (new.rowid, new.text_content, new.ocr_text);
            END;",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at_unix_ms) VALUES (1, ?1)",
            [now],
        )?;
    }

    if current < 2 {
        transaction.execute_batch(
            "ALTER TABLE clips ADD COLUMN source_exe_path TEXT;
             CREATE INDEX clips_source_app ON clips(source_app, created_at_unix_ms DESC);",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at_unix_ms) VALUES (2, ?1)",
            [now],
        )?;
    }

    if current < 3 {
        transaction.execute_batch(
            "ALTER TABLE clip_search ADD COLUMN ocr_status TEXT NOT NULL DEFAULT 'none';
             ALTER TABLE clip_search ADD COLUMN ocr_engine TEXT;
             ALTER TABLE clip_search ADD COLUMN ocr_attempt INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE clip_search ADD COLUMN ocr_content_hash TEXT;
             ALTER TABLE clip_search ADD COLUMN ocr_error_code TEXT;
             ALTER TABLE clip_search ADD COLUMN ocr_updated_at INTEGER;
             CREATE INDEX clip_search_ocr_status ON clip_search(ocr_status);
             DROP TRIGGER IF EXISTS clip_search_au;
             CREATE TRIGGER clip_search_au AFTER UPDATE OF text_content, ocr_text ON clip_search BEGIN
                 INSERT INTO clip_search_fts(clip_search_fts, rowid, text_content, ocr_text)
                 VALUES ('delete', old.rowid, old.text_content, old.ocr_text);
                 INSERT INTO clip_search_fts(rowid, text_content, ocr_text)
                 VALUES (new.rowid, new.text_content, new.ocr_text);
             END;",
        )?;
        // Backfill canonical image hash for legacy clips so backfill/compensation work.
        transaction.execute_batch(
            "UPDATE clip_search
             SET ocr_content_hash = (
               SELECT p.content_hash FROM clip_payloads cp
               JOIN payloads p ON p.id = cp.payload_id
               WHERE cp.clip_id = clip_search.clip_id AND p.kind = 'image'
               ORDER BY p.rowid, p.id LIMIT 1
             )
             WHERE ocr_content_hash IS NULL
               AND EXISTS (
                 SELECT 1 FROM clip_payloads cp
                 JOIN payloads p ON p.id = cp.payload_id
                 WHERE cp.clip_id = clip_search.clip_id AND p.kind = 'image'
               );",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at_unix_ms) VALUES (3, ?1)",
            [now],
        )?;
    }

    if current < 4 {
        transaction.execute_batch("ALTER TABLE clip_search ADD COLUMN ocr_layout TEXT;")?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at_unix_ms) VALUES (4, ?1)",
            [now],
        )?;
    }

    transaction.commit()?;
    Ok(())
}
