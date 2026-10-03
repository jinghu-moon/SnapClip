use super::{
    WindowsOcrEngine,
    engine::{OcrCancel, OcrEngine, OcrError, OcrInput, OcrText},
    rapid::RapidOcrEngine,
};

/// Local engine policy: PP-OCR when its verified model bundle exists,
/// otherwise the built-in Windows OCR engine.
pub struct OcrManager {
    rapid: RapidOcrEngine,
    windows: WindowsOcrEngine,
}

impl OcrManager {
    pub fn new(model_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            rapid: RapidOcrEngine::from_model_dir(model_dir),
            windows: WindowsOcrEngine::new(),
        }
    }
}

impl OcrEngine for OcrManager {
    fn name(&self) -> &'static str {
        "ocr-manager"
    }

    fn is_available(&self) -> bool {
        self.rapid.is_available() || self.windows.is_available()
    }

    fn recognize(&self, input: &OcrInput, cancel: &OcrCancel) -> Result<OcrText, OcrError> {
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
