//! OCR job queue and results (docs/23 T2.5): extracted from the store facade.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueDecision {
    Enqueued { attempt: u32 },
    AlreadyPending,
    NotImage,
    NotFound,
}

#[derive(Debug, Clone)]
pub enum OcrFinishOutcome {
    Done {
        text: String,
        layout: Option<String>,
        engine: String,
    },
    Failed {
        error_code: OcrErrorCode,
        engine: Option<String>,
    },
    Skipped {
        error_code: OcrErrorCode,
        engine: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrCandidateFilter {
    None,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrCandidate {
    pub clip_id: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrStatusInfo {
    pub status: OcrStatus,
    pub engine: Option<String>,
    pub updated_at: Option<i64>,
    pub error_code: Option<OcrErrorCode>,
}


pub(super) fn enqueue_ocr(
    connection: &Connection,
    clip_id: &str,
    content_hash: &str,
) -> Result<QueueDecision, StoreError> {
    // Image presence is payload-based (publication may be text+image).
    let has_image: bool = connection
        .query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM clip_payloads cp
                 JOIN payloads p ON p.id = cp.payload_id
                 WHERE cp.clip_id = ?1 AND p.kind = 'image'
             )",
            [clip_id],
            |row| row.get::<_, i64>(0).map(|v| v != 0),
        )
        .optional()?
        .unwrap_or(false);
    if !has_image {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM clips WHERE id = ?1)",
                [clip_id],
                |row| row.get::<_, i64>(0).map(|v| v != 0),
            )
            .unwrap_or(false);
        return if exists {
            Ok(QueueDecision::NotImage)
        } else {
            Ok(QueueDecision::NotFound)
        };
    }

    let status: Option<String> = connection
        .query_row(
            "SELECT ocr_status FROM clip_search WHERE clip_id = ?1",
            [clip_id],
            |row| row.get(0),
        )
        .optional()?;

    let status = status.unwrap_or_else(|| OcrStatus::None.as_str().to_string());
    match status.as_str() {
        "queued" | "running" | "done" => return Ok(QueueDecision::AlreadyPending),
        "failed" | "skipped" | "none" => {}
        _ => return Ok(QueueDecision::NotFound),
    }

    let updated = connection.execute(
        "UPDATE clip_search
         SET ocr_status = 'queued',
             ocr_attempt = ocr_attempt + 1,
             ocr_content_hash = ?2,
             ocr_error_code = NULL,
             ocr_updated_at = ?3
         WHERE clip_id = ?1 AND ocr_status = ?4",
        params![clip_id, content_hash, unix_time_ms(), status.as_str()],
    )?;
    if updated == 0 {
        connection.execute(
            "INSERT INTO clip_search(clip_id, text_content, ocr_text, ocr_status, ocr_attempt, ocr_content_hash, ocr_updated_at)
             VALUES (?1, NULL, NULL, 'queued', 1, ?2, ?3)
             ON CONFLICT(clip_id) DO UPDATE SET
                ocr_status = 'queued',
                ocr_attempt = clip_search.ocr_attempt + 1,
                ocr_content_hash = excluded.ocr_content_hash,
                ocr_error_code = NULL,
                ocr_updated_at = excluded.ocr_updated_at",
            params![clip_id, content_hash, unix_time_ms()],
        )?;
    }
    let attempt: u32 = connection.query_row(
        "SELECT ocr_attempt FROM clip_search WHERE clip_id = ?1",
        [clip_id],
        |row| row.get(0),
    )?;
    Ok(QueueDecision::Enqueued { attempt })
}

pub(super) fn claim_ocr_job(
    connection: &Connection,
    clip_id: &str,
    content_hash: &str,
) -> Result<Option<u32>, StoreError> {
    let updated = connection.execute(
        "UPDATE clip_search
         SET ocr_status = 'running', ocr_updated_at = ?3
         WHERE clip_id = ?1 AND ocr_status = 'queued' AND ocr_content_hash = ?2",
        params![clip_id, content_hash, unix_time_ms()],
    )?;
    if updated == 0 {
        return Ok(None);
    }
    let attempt: u32 = connection.query_row(
        "SELECT ocr_attempt FROM clip_search WHERE clip_id = ?1",
        [clip_id],
        |row| row.get(0),
    )?;
    Ok(Some(attempt))
}

pub(super) fn finish_ocr_job(
    connection: &Connection,
    clip_id: &str,
    attempt: u32,
    outcome: &OcrFinishOutcome,
) -> Result<bool, StoreError> {
    let now = unix_time_ms();
    let updated = match outcome {
        OcrFinishOutcome::Done {
            text,
            layout,
            engine,
        } => connection.execute(
            "UPDATE clip_search
             SET ocr_status = 'done', ocr_text = ?3, ocr_layout = ?4, ocr_engine = ?5,
                 ocr_error_code = NULL, ocr_updated_at = ?6
             WHERE clip_id = ?1 AND ocr_status = 'running' AND ocr_attempt = ?2",
            params![clip_id, attempt, text, layout, engine, now],
        )?,
        OcrFinishOutcome::Failed { error_code, engine } => connection.execute(
            "UPDATE clip_search
             SET ocr_status = 'failed', ocr_engine = ?3, ocr_error_code = ?4, ocr_updated_at = ?5
             WHERE clip_id = ?1 AND ocr_status = 'running' AND ocr_attempt = ?2",
            params![clip_id, attempt, engine, error_code.as_str(), now],
        )?,
        OcrFinishOutcome::Skipped { error_code, engine } => connection.execute(
            "UPDATE clip_search
             SET ocr_status = 'skipped', ocr_engine = ?3, ocr_error_code = ?4, ocr_updated_at = ?5
             WHERE clip_id = ?1 AND ocr_status = 'running' AND ocr_attempt = ?2",
            params![clip_id, attempt, engine, error_code.as_str(), now],
        )?,
    };
    Ok(updated == 1)
}

pub(super) fn release_queued(
    connection: &Connection,
    clip_id: &str,
    attempt: u32,
) -> Result<bool, StoreError> {
    let updated = connection.execute(
        "UPDATE clip_search
         SET ocr_status = 'none', ocr_updated_at = ?3
         WHERE clip_id = ?1 AND ocr_status = 'queued' AND ocr_attempt = ?2",
        params![clip_id, attempt, unix_time_ms()],
    )?;
    Ok(updated == 1)
}

pub(super) fn reset_stale_ocr_jobs(connection: &Connection) -> Result<u32, StoreError> {
    let updated = connection.execute(
        "UPDATE clip_search
         SET ocr_status = 'none', ocr_updated_at = ?1
         WHERE ocr_status IN ('queued', 'running')",
        [unix_time_ms()],
    )?;
    Ok(updated as u32)
}

pub(super) fn list_ocr_candidates(
    connection: &Connection,
    filter: OcrCandidateFilter,
    limit: u32,
) -> Result<Vec<OcrCandidate>, StoreError> {
    let status = match filter {
        OcrCandidateFilter::None => "none",
        OcrCandidateFilter::Failed => "failed",
    };
    // Derive hash when ocr_content_hash is missing (legacy rows / reset after crash).
    let mut statement = connection.prepare(
        "SELECT cs.clip_id,
                COALESCE(
                  cs.ocr_content_hash,
                  (SELECT p.content_hash FROM clip_payloads cp
                    JOIN payloads p ON p.id = cp.payload_id
                    WHERE cp.clip_id = cs.clip_id AND p.kind = 'image'
                    ORDER BY p.rowid, p.id LIMIT 1)
                ) AS content_hash
         FROM clip_search cs
         WHERE cs.ocr_status = ?1
           AND EXISTS (
             SELECT 1 FROM clip_payloads cp
             JOIN payloads p ON p.id = cp.payload_id
             WHERE cp.clip_id = cs.clip_id AND p.kind = 'image'
           )
           AND COALESCE(
                 cs.ocr_content_hash,
                 (SELECT p.content_hash FROM clip_payloads cp
                   JOIN payloads p ON p.id = cp.payload_id
                   WHERE cp.clip_id = cs.clip_id AND p.kind = 'image'
                   ORDER BY p.rowid, p.id LIMIT 1)
               ) IS NOT NULL
         ORDER BY cs.ocr_updated_at IS NULL, cs.ocr_updated_at
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![status, i64::from(limit)], |row| {
        Ok(OcrCandidate {
            clip_id: row.get(0)?,
            content_hash: row.get(1)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}
