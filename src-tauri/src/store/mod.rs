mod blob;

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
};

use rusqlite::{Connection, OpenFlags, params, params_from_iter};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{
    ClipSummary, ClipboardPublication, ErrorCode, HistoryPage, IpcError, PayloadKind, PayloadRef,
};

pub use blob::{BlobRef, BlobStore};

const WRITE_QUEUE_CAPACITY: usize = 128;
const DEFAULT_PAGE_SIZE: u32 = 50;
const MAX_PAGE_SIZE: u32 = 100;
const PREVIEW_CHAR_LIMIT: usize = 500;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("database operation failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid history cursor")]
    InvalidCursor,
    #[error("invalid publication: {0}")]
    InvalidPublication(String),
    #[error("writer thread is unavailable")]
    WriterUnavailable,
    #[error("store initialization failed: {0}")]
    Initialization(String),
    #[error("store operation failed: {0}")]
    Internal(String),
}

impl From<StoreError> for IpcError {
    fn from(error: StoreError) -> Self {
        let code = match error {
            StoreError::InvalidCursor | StoreError::InvalidPublication(_) => {
                ErrorCode::InvalidArgument
            }
            _ => ErrorCode::Storage,
        };
        Self {
            code,
            message: Some(error.to_string()),
            trace_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PayloadData {
    pub payload: PayloadRef,
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
pub struct Store {
    database_path: PathBuf,
    blob_store: BlobStore,
    writer: SyncSender<WriterRequest>,
}

enum WriterRequest {
    SavePublication {
        publication: ClipboardPublication,
        payloads: Vec<PayloadData>,
        response: mpsc::Sender<Result<(), StoreError>>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryCursor {
    created_at_unix_ms: i64,
    id: String,
}

impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let root = root.as_ref().to_path_buf();
        let database_dir = root.join("data");
        fs::create_dir_all(&database_dir)?;

        let database_path = database_dir.join("snapclip.db");
        let blob_store = BlobStore::new(root.join("payloads"));
        let (writer_tx, writer_rx) = mpsc::sync_channel(WRITE_QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let writer_blob_store = blob_store.clone();
        let writer_database_path = database_path.clone();

        thread::Builder::new()
            .name("snapclip-db-writer".into())
            .spawn(move || {
                let connection = match open_writer(&writer_database_path) {
                    Ok(connection) => connection,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };

                if let Err(error) = sweep_orphans(&connection, &writer_blob_store) {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }

                if ready_tx.send(Ok(())).is_err() {
                    return;
                }

                writer_loop(connection, writer_blob_store, writer_rx);
            })
            .map_err(StoreError::Io)?;

        match ready_rx.recv().map_err(|_| StoreError::WriterUnavailable)? {
            Ok(()) => Ok(Self {
                database_path,
                blob_store,
                writer: writer_tx,
            }),
            Err(message) => Err(StoreError::Initialization(message)),
        }
    }

    pub fn blob_store(&self) -> &BlobStore {
        &self.blob_store
    }

    pub fn save_publication(
        &self,
        publication: ClipboardPublication,
        payloads: Vec<PayloadData>,
    ) -> Result<(), StoreError> {
        let (response_tx, response_rx) = mpsc::channel();
        self.writer
            .send(WriterRequest::SavePublication {
                publication,
                payloads,
                response: response_tx,
            })
            .map_err(|_| StoreError::WriterUnavailable)?;

        response_rx
            .recv()
            .map_err(|_| StoreError::WriterUnavailable)?
    }

    pub fn history_page(
        &self,
        cursor: Option<String>,
        requested_limit: Option<u32>,
    ) -> Result<HistoryPage, StoreError> {
        self.history_page_matching(None, None, cursor, requested_limit)
    }

    pub fn search_history_page(
        &self,
        query: String,
        kind: Option<PayloadKind>,
        cursor: Option<String>,
        requested_limit: Option<u32>,
    ) -> Result<HistoryPage, StoreError> {
        let query = query.trim().chars().take(256).collect::<String>();
        self.history_page_matching(
            (!query.is_empty()).then_some(query),
            kind,
            cursor,
            requested_limit,
        )
    }

    fn history_page_matching(
        &self,
        query: Option<String>,
        kind: Option<PayloadKind>,
        cursor: Option<String>,
        requested_limit: Option<u32>,
    ) -> Result<HistoryPage, StoreError> {
        let cursor = cursor
            .map(|value| serde_json::from_str::<HistoryCursor>(&value))
            .transpose()
            .map_err(|_| StoreError::InvalidCursor)?;
        let limit = requested_limit
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let connection = Connection::open_with_flags(
            &self.database_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;

        let query_mode = query
            .as_ref()
            .map(|value| i64::from(value.chars().count() >= 3));
        let fts_query = query
            .as_ref()
            .map(|value| format!("\"{}\"", value.replace('"', "\"\"")));
        let like_query = query.as_ref().map(|value| {
            let escaped = value
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        });
        let mut statement = connection.prepare(
            "SELECT id, created_at_unix_ms, primary_kind, preview_text, source_app, source_exe_path
             FROM clips
             WHERE (?1 IS NULL OR id IN (
                 SELECT clip_id FROM clip_search
                 WHERE (?2 = 1 AND rowid IN (
                     SELECT rowid FROM clip_search_fts WHERE clip_search_fts MATCH ?3
                 )) OR (?2 = 0 AND text_content LIKE ?4 ESCAPE '\\')
             ))
             AND (?5 IS NULL OR created_at_unix_ms < ?5
                OR (created_at_unix_ms = ?5 AND id < ?6))
             AND (?7 IS NULL OR EXISTS (
                 SELECT 1 FROM clip_payloads cp
                 JOIN payloads p ON p.id = cp.payload_id
                 WHERE cp.clip_id = clips.id AND p.kind = ?7
             ))
             ORDER BY created_at_unix_ms DESC, id DESC
             LIMIT ?8",
        )?;
        let cursor_time = cursor.as_ref().map(|value| value.created_at_unix_ms);
        let cursor_id = cursor.as_ref().map(|value| value.id.as_str()).unwrap_or("");
        let mut rows = statement.query(params![
            query,
            query_mode,
            fts_query,
            like_query,
            cursor_time,
            cursor_id,
            kind.as_ref().map(payload_kind_name),
            i64::from(limit + 1)
        ])?;
        let mut summaries = Vec::new();
        while let Some(row) = rows.next()? {
            summaries.push(ClipSummary {
                id: row.get(0)?,
                created_at_unix_ms: row.get(1)?,
                primary_kind: parse_payload_kind(row.get::<_, String>(2)?.as_str())?,
                preview_text: row.get(3)?,
                source_app: row.get(4)?,
                source_exe_path: row.get(5)?,
                thumbnail: None,
                payloads: Vec::new(),
            });
        }

        let has_more = summaries.len() > limit as usize;
        summaries.truncate(limit as usize);
        if !summaries.is_empty() {
            let placeholders = (1..=summaries.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT cp.clip_id, p.id, p.content_hash, p.kind, p.size_bytes, p.mime_type,
                        p.width, p.height, cp.role
                 FROM clip_payloads cp
                 JOIN payloads p ON p.id = cp.payload_id
                 WHERE cp.clip_id IN ({placeholders})
                 ORDER BY cp.clip_id, cp.role, p.id"
            );
            let indexes = summaries
                .iter()
                .enumerate()
                .map(|(index, summary)| (summary.id.clone(), index))
                .collect::<HashMap<_, _>>();
            let mut payload_statement = connection.prepare(&sql)?;
            let mut payload_rows = payload_statement.query(params_from_iter(
                summaries.iter().map(|summary| summary.id.as_str()),
            ))?;
            while let Some(row) = payload_rows.next()? {
                let clip_id: String = row.get(0)?;
                let payload = PayloadRef {
                    payload_id: row.get(1)?,
                    content_hash: row.get(2)?,
                    kind: parse_payload_kind(row.get::<_, String>(3)?.as_str())?,
                    size_bytes: row.get::<_, i64>(4)?.try_into().map_err(|_| {
                        StoreError::Internal("negative payload size in database".into())
                    })?,
                    mime_type: row.get(5)?,
                    image_dimensions: match (
                        row.get::<_, Option<u32>>(6)?,
                        row.get::<_, Option<u32>>(7)?,
                    ) {
                        (Some(width), Some(height)) => {
                            Some(crate::domain::ImageDimensions { width, height })
                        }
                        _ => None,
                    },
                };
                let role: String = row.get(8)?;
                let summary = &mut summaries[indexes[&clip_id]];
                if role == "thumbnail" {
                    summary.thumbnail = Some(payload.clone());
                }
                summary.payloads.push(payload);
            }
        }

        let next_cursor = if has_more {
            summaries
                .last()
                .map(|summary| {
                    serde_json::to_string(&HistoryCursor {
                        created_at_unix_ms: summary.created_at_unix_ms,
                        id: summary.id.clone(),
                    })
                })
                .transpose()
                .map_err(|error| StoreError::Internal(error.to_string()))?
        } else {
            None
        };

        Ok(HistoryPage {
            items: summaries,
            next_cursor,
        })
    }
}

fn writer_loop(connection: Connection, blob_store: BlobStore, receiver: Receiver<WriterRequest>) {
    for request in receiver {
        match request {
            WriterRequest::SavePublication {
                publication,
                payloads,
                response,
            } => {
                let result = insert_publication(&connection, &blob_store, publication, payloads);
                let _ = response.send(result);
            }
        }
    }
}

fn sweep_orphans(connection: &Connection, blob_store: &BlobStore) -> Result<(), StoreError> {
    let mut statement = connection.prepare("SELECT storage_path FROM payloads")?;
    let referenced = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<HashSet<_>, _>>()?;
    blob_store.remove_orphans(&referenced)?;
    Ok(())
}

fn open_writer(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    migrate(&connection)?;
    Ok(connection)
}

fn migrate(connection: &Connection) -> Result<(), StoreError> {
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
            CREATE TRIGGER clip_search_au AFTER UPDATE ON clip_search BEGIN
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

    transaction.commit()?;
    Ok(())
}

fn insert_publication(
    connection: &Connection,
    blob_store: &BlobStore,
    publication: ClipboardPublication,
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
        .find(|input| input.payload.kind == PayloadKind::Text)
        .map(|input| truncate_chars(&String::from_utf8_lossy(&input.bytes), PREVIEW_CHAR_LIMIT));
    let search_text = payloads
        .iter()
        .find(|input| input.payload.kind == PayloadKind::Text)
        .map(|input| String::from_utf8_lossy(&input.bytes).into_owned());

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
        "INSERT INTO clip_search(clip_id, text_content, ocr_text) VALUES (?1, ?2, NULL)
         ON CONFLICT(clip_id) DO UPDATE SET text_content = excluded.text_content",
        params![publication.publication_id, search_text],
    )?;
    transaction.commit()?;
    Ok(())
}

fn payload_kind_name(kind: &PayloadKind) -> &'static str {
    match kind {
        PayloadKind::Text => "text",
        PayloadKind::Html => "html",
        PayloadKind::Rtf => "rtf",
        PayloadKind::Image => "image",
        PayloadKind::Files => "files",
        PayloadKind::Other => "other",
    }
}

fn parse_payload_kind(value: &str) -> Result<PayloadKind, StoreError> {
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

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn unix_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use crate::domain::{ClipboardPublication, ImageDimensions, PayloadKind, PayloadRef};
    use rusqlite::Connection;

    use super::{PayloadData, Store};

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "snapclip-store-test-{}-{}",
                std::process::id(),
                TEST_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn payload(kind: PayloadKind, bytes: &[u8], id: &str) -> PayloadData {
        PayloadData {
            payload: PayloadRef {
                payload_id: id.into(),
                content_hash: blake3::hash(bytes).to_hex().to_string(),
                kind,
                size_bytes: bytes.len() as u64,
                mime_type: None,
                image_dimensions: None,
            },
            bytes: bytes.to_vec(),
        }
    }

    fn publication(id: &str, time: i64, payloads: &[PayloadData]) -> ClipboardPublication {
        ClipboardPublication {
            publication_id: id.into(),
            captured_at_unix_ms: time,
            source_app: Some("test".into()),
            source_exe_path: Some(r"C:\Apps\test.exe".into()),
            payloads: payloads.iter().map(|value| value.payload.clone()).collect(),
        }
    }

    #[test]
    fn saves_all_formats_and_returns_stable_cursor_pages() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        for (id, time) in [("clip-c", 10), ("clip-b", 10), ("clip-a", 10)] {
            let text = payload(PayloadKind::Text, b"hello world", &format!("text-{id}"));
            let image = payload(PayloadKind::Image, b"image bytes", "shared-image");
            let mut image = image;
            image.payload.image_dimensions = Some(ImageDimensions {
                width: 2,
                height: 3,
            });
            let payloads = vec![text, image];
            store
                .save_publication(publication(id, time, &payloads), payloads)
                .unwrap();
        }

        let first = store.history_page(None, Some(2)).unwrap();
        assert_eq!(first.items.len(), 2);
        assert!(first.items.iter().all(|clip| clip.payloads.len() == 2));
        assert!(
            first
                .items
                .iter()
                .all(|clip| clip.preview_text.as_deref() == Some("hello world"))
        );

        let second = store.history_page(first.next_cursor, Some(2)).unwrap();
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].id, "clip-a");
        assert!(second.next_cursor.is_none());

        let connection = Connection::open(dir.0.join("data/snapclip.db")).unwrap();
        let unique_blobs: i64 = connection
            .query_row("SELECT COUNT(*) FROM payloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(unique_blobs, 2);
        let indexed_matches: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM clip_search_fts WHERE clip_search_fts MATCH 'hello'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexed_matches, 3);
    }

    #[test]
    fn history_search_uses_trigram_and_short_query_fallback() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let payloads = vec![
            payload(PayloadKind::Text, "截图搜索".as_bytes(), "search-text"),
            payload(PayloadKind::Image, b"image bytes", "search-image"),
        ];
        store
            .save_publication(publication("searchable-clip", 1, &payloads), payloads)
            .unwrap();

        let trigram = store
            .search_history_page("截图搜索".into(), None, None, None)
            .unwrap();
        assert_eq!(trigram.items.len(), 1);
        assert_eq!(trigram.items[0].id, "searchable-clip");

        let short = store
            .search_history_page("图".into(), None, None, None)
            .unwrap();
        assert_eq!(short.items.len(), 1);

        let no_match = store
            .search_history_page("不存在".into(), None, None, None)
            .unwrap();
        assert!(no_match.items.is_empty());

        let text_only = store
            .search_history_page(String::new(), Some(PayloadKind::Text), None, None)
            .unwrap();
        assert_eq!(text_only.items.len(), 1);
        assert_eq!(text_only.items[0].primary_kind, PayloadKind::Text);

        let image_only = store
            .search_history_page(String::new(), Some(PayloadKind::Image), None, None)
            .unwrap();
        assert_eq!(image_only.items.len(), 1);
        assert_eq!(image_only.items[0].primary_kind, PayloadKind::Text);
    }

    #[test]
    fn identical_bytes_keep_distinct_semantic_payloads() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let payloads = vec![
            payload(PayloadKind::Text, b"same bytes", "text-payload"),
            payload(PayloadKind::Html, b"same bytes", "html-payload"),
        ];
        store
            .save_publication(publication("multi-kind", 1, &payloads), payloads)
            .unwrap();

        let page = store.history_page(None, None).unwrap();
        let clip = &page.items[0];
        assert_eq!(clip.payloads.len(), 2);
        assert_ne!(clip.payloads[0].payload_id, clip.payloads[1].payload_id);
        assert_eq!(clip.payloads[0].content_hash, clip.payloads[1].content_hash);
    }

    #[test]
    fn rejects_cursor_with_invalid_json() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        assert!(store.history_page(Some("bad cursor".into()), None).is_err());
    }

    #[test]
    fn persists_source_app_and_exe_path() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let payloads = vec![payload(PayloadKind::Text, b"from vscode", "source-text")];
        store
            .save_publication(publication("source-clip", 1, &payloads), payloads)
            .unwrap();

        let page = store.history_page(None, None).unwrap();
        assert_eq!(page.items[0].source_app.as_deref(), Some("test"));
        assert_eq!(
            page.items[0].source_exe_path.as_deref(),
            Some(r"C:\Apps\test.exe")
        );
    }

    #[test]
    fn migration_upgrades_existing_v1_database() {
        let dir = TestDir::new();
        let db_path = dir.0.join("data/snapclip.db");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        {
            let connection = Connection::open(&db_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE schema_migrations (
                        version INTEGER PRIMARY KEY,
                        applied_at_unix_ms INTEGER NOT NULL
                    );
                    CREATE TABLE clips (
                        id TEXT PRIMARY KEY,
                        created_at_unix_ms INTEGER NOT NULL,
                        primary_kind TEXT NOT NULL,
                        preview_text TEXT,
                        source_app TEXT
                    );
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
                    CREATE TRIGGER clip_search_au AFTER UPDATE ON clip_search BEGIN
                        INSERT INTO clip_search_fts(clip_search_fts, rowid, text_content, ocr_text)
                        VALUES ('delete', old.rowid, old.text_content, old.ocr_text);
                        INSERT INTO clip_search_fts(rowid, text_content, ocr_text)
                        VALUES (new.rowid, new.text_content, new.ocr_text);
                    END;
                    INSERT INTO schema_migrations(version, applied_at_unix_ms) VALUES (1, 0);",
                )
                .unwrap();
        }

        let store = Store::open(&dir.0).unwrap();
        let payloads = vec![payload(PayloadKind::Text, b"legacy", "legacy-text")];
        store
            .save_publication(publication("legacy-clip", 1, &payloads), payloads)
            .unwrap();
        let page = store.history_page(None, None).unwrap();
        assert_eq!(
            page.items[0].source_exe_path.as_deref(),
            Some(r"C:\Apps\test.exe")
        );

        let connection = Connection::open(&db_path).unwrap();
        let max_version: i64 = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(max_version, 2);
    }
}
