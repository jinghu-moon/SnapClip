//! Clipboard image OCR: pluggable engines, single worker, cancel-aware.

mod engine;
mod manager;
mod rapid;
mod win_ocr;
mod worker;

pub use engine::OcrEngine;
pub use manager::OcrManager;
pub use win_ocr::{WindowsOcrEngine, init_apartment};
pub use worker::{OcrEnqueuer, OcrService, OcrServiceHandle};
