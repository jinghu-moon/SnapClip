//! Clipboard history storage: one SQLite database plus the blob store (docs/23 T2.5).
//!
//! Moved here from `src-tauri/src/infrastructure/store/mod.rs`. The one thing that stayed
//! in the shell is the `From<StoreError> for IpcError` mapping, because turning a storage
//! failure into an IPC code is transport glue and this crate has no IPC.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::blob_store::ClipboardBlobStore as BlobStore;
use crate::StoreError;
use snapclip_model::{
    ClipSummary, HistoryPage, OcrErrorCode, OcrStatus, PayloadData, PayloadKind, PayloadRef,
    Publication,
};

const WRITE_QUEUE_CAPACITY: usize = 128;
const DEFAULT_PAGE_SIZE: u32 = 50;
const MAX_PAGE_SIZE: u32 = 100;
const PREVIEW_CHAR_LIMIT: usize = 500;
#[derive(Clone)]
pub struct Store {
    database_path: PathBuf,
    writer: SyncSender<WriterRequest>,
}

enum WriterRequest {
    SavePublication {
        publication: Publication,
        payloads: Vec<PayloadData>,
        response: mpsc::Sender<Result<(), StoreError>>,
    },
    EnqueueOcr {
        clip_id: String,
        content_hash: String,
        response: mpsc::Sender<Result<QueueDecision, StoreError>>,
    },
    ClaimOcrJob {
        clip_id: String,
        content_hash: String,
        response: mpsc::Sender<Result<Option<u32>, StoreError>>,
    },
    FinishOcrJob {
        clip_id: String,
        attempt: u32,
        outcome: OcrFinishOutcome,
        response: mpsc::Sender<Result<bool, StoreError>>,
    },
    ReleaseQueued {
        clip_id: String,
        attempt: u32,
        response: mpsc::Sender<Result<bool, StoreError>>,
    },
    ResetStaleOcrJobs {
        response: mpsc::Sender<Result<u32, StoreError>>,
    },
    ListOcrCandidates {
        filter: OcrCandidateFilter,
        limit: u32,
        response: mpsc::Sender<Result<Vec<OcrCandidate>, StoreError>>,
    },
    ReadPayloadBytes {
        content_hash: String,
        kind: PayloadKind,
        response: mpsc::Sender<Result<Vec<u8>, StoreError>>,
    },
    DeletePublication {
        clip_id: String,
        response: mpsc::Sender<Result<bool, StoreError>>,
    },
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
                writer: writer_tx,
            }),
            Err(message) => Err(StoreError::Initialization(message)),
        }
    }

    pub fn save_publication(
        &self,
        publication: Publication,
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

    pub fn enqueue_ocr(
        &self,
        clip_id: String,
        content_hash: String,
    ) -> Result<QueueDecision, StoreError> {
        self.send_request(|response| WriterRequest::EnqueueOcr {
            clip_id,
            content_hash,
            response,
        })
    }

    pub fn claim_ocr_job(
        &self,
        clip_id: String,
        content_hash: String,
    ) -> Result<Option<u32>, StoreError> {
        self.send_request(|response| WriterRequest::ClaimOcrJob {
            clip_id,
            content_hash,
            response,
        })
    }

    pub fn finish_ocr_job(
        &self,
        clip_id: String,
        attempt: u32,
        outcome: OcrFinishOutcome,
    ) -> Result<bool, StoreError> {
        self.send_request(|response| WriterRequest::FinishOcrJob {
            clip_id,
            attempt,
            outcome,
            response,
        })
    }

    pub fn release_queued(&self, clip_id: String, attempt: u32) -> Result<bool, StoreError> {
        self.send_request(|response| WriterRequest::ReleaseQueued {
            clip_id,
            attempt,
            response,
        })
    }

    pub fn reset_stale_ocr_jobs(&self) -> Result<u32, StoreError> {
        self.send_request(|response| WriterRequest::ResetStaleOcrJobs { response })
    }

    pub fn list_ocr_candidates(
        &self,
        filter: OcrCandidateFilter,
        limit: u32,
    ) -> Result<Vec<OcrCandidate>, StoreError> {
        self.send_request(|response| WriterRequest::ListOcrCandidates {
            filter,
            limit,
            response,
        })
    }

    pub fn read_payload_bytes(
        &self,
        content_hash: String,
        kind: PayloadKind,
    ) -> Result<Vec<u8>, StoreError> {
        self.send_request(|response| WriterRequest::ReadPayloadBytes {
            content_hash,
            kind,
            response,
        })
    }

    /// Remove one clip and everything only it was keeping alive.
    ///
    /// Returns whether a row was there to delete. The payload rows and their blobs are
    /// pruned afterwards, because a payload is content-addressed: the same bytes may still
    /// be referenced by another clip, and the store must not delete a file another row still
    /// needs. (This is why the delete is not "just a row delete".)
    pub fn delete_publication(&self, clip_id: String) -> Result<bool, StoreError> {
        self.send_request(|response| WriterRequest::DeletePublication { clip_id, response })
    }

    /// Read-only OCR status (separate connection is fine for SELECT).
    pub fn ocr_status_of(&self, clip_id: &str) -> Result<OcrStatusInfo, StoreError> {
        let connection = Connection::open_with_flags(
            &self.database_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        let (status, engine, updated_at, error_code): (
            String,
            Option<String>,
            Option<i64>,
            Option<String>,
        ) = connection.query_row(
            "SELECT ocr_status, ocr_engine, ocr_updated_at, ocr_error_code
                 FROM clip_search WHERE clip_id = ?1",
            [clip_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        Ok(OcrStatusInfo {
            status: OcrStatus::parse(&status).unwrap_or(OcrStatus::None),
            engine,
            updated_at,
            error_code: error_code.as_deref().and_then(OcrErrorCode::parse),
        })
    }

    fn send_request<T>(
        &self,
        build: impl FnOnce(mpsc::Sender<Result<T, StoreError>>) -> WriterRequest,
    ) -> Result<T, StoreError> {
        let (response_tx, response_rx) = mpsc::channel();
        self.writer
            .send(build(response_tx))
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
            "SELECT clips.id, clips.created_at_unix_ms, clips.primary_kind, clips.preview_text,
                    clips.source_app, clips.source_exe_path,
                    cs.ocr_status, cs.ocr_text, cs.ocr_layout, cs.ocr_engine, cs.ocr_updated_at, cs.ocr_error_code
             FROM clips
             LEFT JOIN clip_search cs ON cs.clip_id = clips.id
             WHERE (?1 IS NULL OR clips.id IN (
                 SELECT clip_id FROM clip_search
                 WHERE (?2 = 1 AND rowid IN (
                     SELECT rowid FROM clip_search_fts WHERE clip_search_fts MATCH ?3
                 )) OR (?2 = 0 AND (
                     text_content LIKE ?4 ESCAPE '\\'
                     OR ocr_text LIKE ?4 ESCAPE '\\'
                 ))
             ))
             AND (?5 IS NULL OR clips.created_at_unix_ms < ?5
                OR (clips.created_at_unix_ms = ?5 AND clips.id < ?6))
             AND (?7 IS NULL OR EXISTS (
                 SELECT 1 FROM clip_payloads cp
                 JOIN payloads p ON p.id = cp.payload_id
                 WHERE cp.clip_id = clips.id AND p.kind = ?7
             ))
             ORDER BY clips.created_at_unix_ms DESC, clips.id DESC
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
            let ocr_status_raw: Option<String> = row.get(6)?;
            let ocr_error_raw: Option<String> = row.get(11)?;
            summaries.push(ClipSummary {
                id: row.get(0)?,
                created_at_unix_ms: row.get(1)?,
                origin: snapclip_model::PublicationOrigin::Clipboard,
                primary_kind: parse_payload_kind(row.get::<_, String>(2)?.as_str())?,
                preview_text: row.get(3)?,
                source_app: row.get(4)?,
                source_exe_path: row.get(5)?,
                thumbnail: None,
                payloads: Vec::new(),
                ocr_status: ocr_status_raw
                    .as_deref()
                    .and_then(OcrStatus::parse)
                    .unwrap_or(OcrStatus::None),
                ocr_text: row.get(7)?,
                ocr_layout: row.get(8)?,
                ocr_engine: row.get(9)?,
                ocr_updated_at: row.get(10)?,
                ocr_error_code: ocr_error_raw.as_deref().and_then(OcrErrorCode::parse),
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
                            Some(snapclip_model::ImageDimensions { width, height })
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
            WriterRequest::EnqueueOcr {
                clip_id,
                content_hash,
                response,
            } => {
                let _ = response.send(enqueue_ocr(&connection, &clip_id, &content_hash));
            }
            WriterRequest::ClaimOcrJob {
                clip_id,
                content_hash,
                response,
            } => {
                let _ = response.send(claim_ocr_job(&connection, &clip_id, &content_hash));
            }
            WriterRequest::FinishOcrJob {
                clip_id,
                attempt,
                outcome,
                response,
            } => {
                let _ = response.send(finish_ocr_job(&connection, &clip_id, attempt, &outcome));
            }
            WriterRequest::ReleaseQueued {
                clip_id,
                attempt,
                response,
            } => {
                let _ = response.send(release_queued(&connection, &clip_id, attempt));
            }
            WriterRequest::ResetStaleOcrJobs { response } => {
                let _ = response.send(reset_stale_ocr_jobs(&connection));
            }
            WriterRequest::ListOcrCandidates {
                filter,
                limit,
                response,
            } => {
                let _ = response.send(list_ocr_candidates(&connection, filter, limit));
            }
            WriterRequest::ReadPayloadBytes {
                content_hash,
                kind,
                response,
            } => {
                let _ = response.send(read_payload_bytes(
                    &connection,
                    &blob_store,
                    &content_hash,
                    kind,
                ));
            }
            WriterRequest::DeletePublication { clip_id, response } => {
                let _ = response.send(delete_publication(&connection, &blob_store, &clip_id));
            }
        }
    }
}


mod artifact_repository;
mod clip_repository;
mod connection;
mod migration;
mod recognition_repository;

use artifact_repository::*;
use clip_repository::*;
use connection::*;
use migration::*;
use recognition_repository::*;

pub use recognition_repository::{
    OcrCandidate, OcrCandidateFilter, OcrFinishOutcome, OcrStatusInfo, QueueDecision,
};

// 过渡期转发（docs/23 T1.4）：这里原先还有一份私有实现，和 `clipboard_ingest` 的
// 那份重复。现在只有 `snapclip-model` 一份定义。
use snapclip_model::time::unix_time_ms;

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use snapclip_model::{
        ImageDimensions, OcrStatus, PayloadData, PayloadKind, PayloadRef, Publication,
        PublicationOrigin,
    };
    use rusqlite::Connection;

    use super::Store;

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            // Key on pid + wall-clock nanos + counter, and create with `create_dir`
            // (which fails when the path already exists) so a stale directory left
            // behind by a force-killed run can never be reused. Windows reuses pids,
            // so a pid+counter key alone occasionally collided with an old
            // `snapclip.db`, surfacing as spurious `UNIQUE constraint failed:
            // clips.id` errors in parallel runs.
            let pid = std::process::id();
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            loop {
                let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
                let path =
                    std::env::temp_dir().join(format!("snapclip-store-test-{pid}-{stamp}-{id}"));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create test store dir failed: {error}"),
                }
            }
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

    fn publication(id: &str, time: i64, payloads: &[PayloadData]) -> Publication {
        Publication {
            publication_id: id.into(),
            origin: PublicationOrigin::Clipboard,
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
    fn whitespace_text_does_not_hide_image_preview() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let text = payload(PayloadKind::Text, b" ", "blank-text");
        let image = payload(PayloadKind::Image, b"image-bytes", "blank-image");
        let payloads = vec![text, image];
        store
            .save_publication(publication("blank-text-image", 1, &payloads), payloads)
            .unwrap();
        let page = store.history_page(None, Some(1)).unwrap();
        assert_eq!(page.items[0].preview_text, None);
        assert!(
            page.items[0]
                .payloads
                .iter()
                .any(|p| p.kind == PayloadKind::Image)
        );
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
    fn deleting_a_clip_removes_its_row_its_search_entry_and_its_unshared_blob() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let doomed = payload(PayloadKind::Text, b"doomed text", "doomed-text");
        let kept = payload(PayloadKind::Text, b"kept text", "kept-text");
        store
            .save_publication(publication("doomed", 2, &[doomed]), vec![payload(PayloadKind::Text, b"doomed text", "doomed-text")])
            .unwrap();
        store
            .save_publication(publication("kept", 1, &[kept]), vec![payload(PayloadKind::Text, b"kept text", "kept-text")])
            .unwrap();

        assert!(store.delete_publication("doomed".into()).unwrap());
        // Deleting something that is already gone reports "nothing was there" rather than
        // failing: the caller may be racing the other shell's delete.
        assert!(!store.delete_publication("doomed".into()).unwrap());

        let page = store.history_page(None, None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, "kept");
        // The search index dropped the deleted row too, not just the table.
        let search = store
            .search_history_page("doomed".into(), None, None, None)
            .unwrap();
        assert!(search.items.is_empty());
        let search = store
            .search_history_page("kept".into(), None, None, None)
            .unwrap();
        assert_eq!(search.items.len(), 1);

        let connection = Connection::open(dir.0.join("data/snapclip.db")).unwrap();
        let payload_rows: i64 = connection
            .query_row("SELECT COUNT(*) FROM payloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(payload_rows, 1, "the deleted clip's payload row is gone");
        let blob_files = std::fs::read_dir(dir.0.join("payloads"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .flat_map(|entry| std::fs::read_dir(entry.path()).unwrap().flatten())
            .count();
        assert_eq!(blob_files, 1, "the deleted clip's bytes are gone");
    }

    #[test]
    fn deleting_one_clip_keeps_bytes_another_clip_still_references() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        // Two clips sharing the same image bytes (the store keys payloads by content hash).
        for (id, time) in [("first", 2), ("second", 1)] {
            let image = payload(PayloadKind::Image, b"shared image bytes", "shared-image");
            store
                .save_publication(
                    publication(id, time, std::slice::from_ref(&image)),
                    vec![image],
                )
                .unwrap();
        }
        assert!(store.delete_publication("first".into()).unwrap());

        // The surviving row can still read its bytes: the payload row and the blob are
        // content-addressed, so "delete" must prune, not blindly unlink.
        let page = store.history_page(None, None).unwrap();
        assert_eq!(page.items.len(), 1);
        let hash = blake3::hash(b"shared image bytes").to_hex().to_string();
        assert_eq!(
            store
                .read_payload_bytes(hash, PayloadKind::Image)
                .unwrap(),
            b"shared image bytes"
        );
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
        assert_eq!(max_version, 4);
    }

    #[test]
    fn ocr_state_machine_enqueue_claim_finish() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let payloads = vec![payload(PayloadKind::Image, b"fake-png-bytes", "ocr-img")];
        store
            .save_publication(publication("ocr-clip", 1, &payloads), payloads)
            .unwrap();

        let decision = store
            .enqueue_ocr(
                "ocr-clip".into(),
                blake3::hash(b"fake-png-bytes").to_hex().to_string(),
            )
            .unwrap();
        let attempt = match decision {
            crate::store::QueueDecision::Enqueued { attempt } => attempt,
            other => panic!("expected enqueued, got {other:?}"),
        };
        assert_eq!(attempt, 1);

        // Cannot enqueue while queued.
        let again = store
            .enqueue_ocr(
                "ocr-clip".into(),
                blake3::hash(b"fake-png-bytes").to_hex().to_string(),
            )
            .unwrap();
        assert_eq!(again, crate::store::QueueDecision::AlreadyPending);

        let claimed = store
            .claim_ocr_job(
                "ocr-clip".into(),
                blake3::hash(b"fake-png-bytes").to_hex().to_string(),
            )
            .unwrap();
        assert_eq!(claimed, Some(1));

        // Wrong attempt is ignored.
        let stale = store
            .finish_ocr_job(
                "ocr-clip".into(),
                99,
                crate::store::OcrFinishOutcome::Done {
                    text: "stale".into(),
                    layout: None,
                    engine: "test".into(),
                },
            )
            .unwrap();
        assert!(!stale);

        let ok = store
            .finish_ocr_job(
                "ocr-clip".into(),
                attempt,
                crate::store::OcrFinishOutcome::Done {
                    text: "你好 OCR".into(),
                    layout: Some(r#"[{"text":"你好 OCR"}]"#.into()),
                    engine: "test".into(),
                },
            )
            .unwrap();
        assert!(ok);

        let page = store
            .search_history_page("你".into(), None, None, None)
            .unwrap();
        assert_eq!(page.items.len(), 1, "single-char OCR search should hit");
        let page = store
            .search_history_page("你好".into(), None, None, None)
            .unwrap();
        assert_eq!(page.items.len(), 1, "two-char OCR search should hit");
        assert_eq!(page.items[0].ocr_status, OcrStatus::Done);
        assert_eq!(
            page.items[0].ocr_layout.as_deref(),
            Some(r#"[{"text":"你好 OCR"}]"#)
        );
    }

    #[test]
    fn ocr_reset_stale_requeues_running() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let payloads = vec![payload(PayloadKind::Image, b"img-bytes", "stale-img")];
        store
            .save_publication(publication("stale-clip", 1, &payloads), payloads)
            .unwrap();
        let hash = blake3::hash(b"img-bytes").to_hex().to_string();
        let attempt = match store
            .enqueue_ocr("stale-clip".into(), hash.clone())
            .unwrap()
        {
            crate::store::QueueDecision::Enqueued { attempt } => attempt,
            _ => panic!("enqueue"),
        };
        assert!(
            store
                .claim_ocr_job("stale-clip".into(), hash)
                .unwrap()
                .is_some()
        );
        let _ = attempt;
        let reset = store.reset_stale_ocr_jobs().unwrap();
        assert!(reset >= 1);
        let info = store.ocr_status_of("stale-clip").unwrap();
        assert_eq!(info.status, OcrStatus::None);
    }

    #[test]
    fn ocr_empty_text_is_done() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let payloads = vec![payload(PayloadKind::Image, b"empty-ocr", "empty-img")];
        store
            .save_publication(publication("empty-clip", 1, &payloads), payloads)
            .unwrap();
        let hash = blake3::hash(b"empty-ocr").to_hex().to_string();
        let attempt = match store
            .enqueue_ocr("empty-clip".into(), hash.clone())
            .unwrap()
        {
            crate::store::QueueDecision::Enqueued { attempt } => attempt,
            _ => panic!("enqueue"),
        };
        store.claim_ocr_job("empty-clip".into(), hash).unwrap();
        assert!(
            store
                .finish_ocr_job(
                    "empty-clip".into(),
                    attempt,
                    crate::store::OcrFinishOutcome::Done {
                        text: String::new(),
                        layout: None,
                        engine: "test".into(),
                    },
                )
                .unwrap()
        );
        let info = store.ocr_status_of("empty-clip").unwrap();
        assert_eq!(info.status, OcrStatus::Done);
    }

    #[test]
    fn ocr_text_and_image_publication_can_enqueue() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let text = payload(PayloadKind::Text, b"hello mixed", "mixed-text");
        let image = payload(PayloadKind::Image, b"mixed-image", "mixed-img");
        let payloads = vec![text, image];
        store
            .save_publication(publication("mixed-clip", 1, &payloads), payloads)
            .unwrap();

        let decision = store
            .enqueue_ocr(
                "mixed-clip".into(),
                blake3::hash(b"mixed-image").to_hex().to_string(),
            )
            .unwrap();
        assert!(matches!(
            decision,
            crate::store::QueueDecision::Enqueued { .. }
        ));
    }

    #[test]
    fn ocr_candidates_derive_hash_without_content_hash_column() {
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let payloads = vec![payload(PayloadKind::Image, b"derive-hash", "derive-img")];
        store
            .save_publication(publication("derive-clip", 1, &payloads), payloads)
            .unwrap();
        // Simulate legacy row: clear ocr_content_hash after save.
        {
            let connection = Connection::open(dir.0.join("data/snapclip.db")).unwrap();
            connection
                .execute(
                    "UPDATE clip_search SET ocr_content_hash = NULL WHERE clip_id = 'derive-clip'",
                    [],
                )
                .unwrap();
        }
        let candidates = store
            .list_ocr_candidates(crate::store::OcrCandidateFilter::None, 10)
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].content_hash,
            blake3::hash(b"derive-hash").to_hex().to_string()
        );
    }

    #[test]
    fn ocr_restart_none_is_requeued_then_claimable() {
        // Simulates: reset_stale → none → enqueue → claim (compensation path).
        let dir = TestDir::new();
        let store = Store::open(&dir.0).unwrap();
        let payloads = vec![payload(PayloadKind::Image, b"restart-img", "restart-p")];
        store
            .save_publication(publication("restart-clip", 1, &payloads), payloads)
            .unwrap();
        let hash = blake3::hash(b"restart-img").to_hex().to_string();
        let attempt = match store
            .enqueue_ocr("restart-clip".into(), hash.clone())
            .unwrap()
        {
            crate::store::QueueDecision::Enqueued { attempt } => attempt,
            _ => panic!("enqueue"),
        };
        store
            .claim_ocr_job("restart-clip".into(), hash.clone())
            .unwrap();
        store.reset_stale_ocr_jobs().unwrap();
        assert_eq!(
            store.ocr_status_of("restart-clip").unwrap().status,
            OcrStatus::None
        );
        // compensation: enqueue first (none→queued), then claim
        let attempt2 = match store
            .enqueue_ocr("restart-clip".into(), hash.clone())
            .unwrap()
        {
            crate::store::QueueDecision::Enqueued { attempt } => attempt,
            _ => panic!("re-enqueue"),
        };
        assert!(attempt2 > attempt);
        assert_eq!(
            store.claim_ocr_job("restart-clip".into(), hash).unwrap(),
            Some(attempt2)
        );
    }
}
