//! Adapter from the OCR worker queue to the clipboard ingest contract.
//!
//! Keeps `application::clipboard_ingest` free of OCR types while still letting the
//! clipboard pipeline hand images to the OCR worker.

use crate::application::clipboard_ingest::OcrQueue;
use crate::ocr::OcrEnqueuer;

impl OcrQueue for OcrEnqueuer {
    fn try_enqueue(&self, clip_id: &str, content_hash: &str) -> bool {
        OcrEnqueuer::try_enqueue(self, clip_id, content_hash)
    }
}