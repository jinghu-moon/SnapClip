//! Clipboard image OCR: pluggable engines, single worker, cancel-aware.

mod engine;
mod manager;
mod rapid;
mod win_ocr;
mod worker;

pub use engine::OcrEngine;
pub use manager::OcrManager;
pub use win_ocr::{WindowsOcrEngine, init_apartment};
pub use worker::{OcrEnqueuer, OcrService};

#[cfg(test)]
mod tests {
    use super::{WindowsOcrEngine, init_apartment};

    /// The Windows OCR engine stays reachable as an alternative to the ONNX engine.
    #[test]
    fn windows_ocr_engine_is_available() {
        let _ = init_apartment();
        let _ = std::mem::size_of::<WindowsOcrEngine>();
    }
}
