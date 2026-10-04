use super::{
    WindowsOcrEngine,
    engine::{OcrCancel, OcrEngine, OcrError, OcrInput, OcrText},
};
// The ONNX / PP-OCR adapter is optional: `rapid-ocr-rs` is mid-way through breaking
// changes, so its `ocr-rapid` feature is OFF by default (see Cargo.toml). Everything
// here compiles without it and falls back to the built-in Windows OCR engine.
#[cfg(feature = "ocr-rapid")]
use super::rapid::RapidOcrEngine;

/// Local engine policy: PP-OCR when its verified model bundle exists,
/// otherwise the built-in Windows OCR engine.
///
/// The `rapid` field only exists under the `ocr-rapid` feature. A default build
/// carries just the Windows engine, so the unstable `rapid-ocr-rs` API never
/// enters the compile graph.
pub struct OcrManager {
    #[cfg(feature = "ocr-rapid")]
    rapid: RapidOcrEngine,
    windows: WindowsOcrEngine,
}

impl OcrManager {
    pub fn new(model_dir: impl Into<std::path::PathBuf>) -> Self {
        let windows = WindowsOcrEngine::new();
        #[cfg(feature = "ocr-rapid")]
        {
            Self {
                rapid: RapidOcrEngine::from_model_dir(model_dir),
                windows,
            }
        }
        #[cfg(not(feature = "ocr-rapid"))]
        {
            // `model_dir` only parameterises the ONNX engine; drop it while that
            // engine is disabled so callers and this signature stay unchanged.
            let _ = model_dir;
            Self { windows }
        }
    }
}

impl OcrEngine for OcrManager {
    fn name(&self) -> &'static str {
        "ocr-manager"
    }

    fn is_available(&self) -> bool {
        #[cfg(feature = "ocr-rapid")]
        if self.rapid.is_available() {
            return true;
        }
        self.windows.is_available()
    }

    fn recognize(&self, input: &OcrInput, cancel: &OcrCancel) -> Result<OcrText, OcrError> {
        #[cfg(feature = "ocr-rapid")]
        if self.rapid.is_available() {
            match self.rapid.recognize(input, cancel) {
                Ok(result) => return Ok(result),
                Err(error) if matches!(error, OcrError::Cancelled) => return Err(error),
                Err(error) => eprintln!("[snapclip][ocr] rapidocr fallback: {error}"),
            }
        }
        self.windows.recognize(input, cancel)
    }
}
