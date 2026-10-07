//! Clipboard image OCR: pluggable engines, single worker, cancel-aware.

mod engine;
mod events;
mod manager;
// ONNX / PP-OCR engine adapter. Compiled only with the `ocr-rapid` feature, which
// is OFF by default because `rapid-ocr-rs` is mid-refactor with an unstable API.
// The built-in Windows OCR engine (`win_ocr`) stays available regardless.
#[cfg(feature = "ocr-rapid")]
mod rapid;
mod win_ocr;
mod worker;

pub use engine::OcrEngine;
pub use events::OcrEventSink;
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
