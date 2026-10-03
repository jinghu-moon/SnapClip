//! Adapter for the reusable PP-OCR ONNX crate.

use std::{path::PathBuf, sync::Mutex};

use rapid_ocr_rs::{
    EngineConfig, GenericOcrInput, GenericOcrOutput, LangDet, LangRec, ModelConfig, ModelType,
    OcrEngine as GenericOcrEngine, OcrVersion, RapidOcrEngine as RapidEngine,
};

use super::engine::{OcrCancel, OcrEngine, OcrError, OcrInput, OcrText};

pub struct RapidOcrEngine {
    model_dir: PathBuf,
    engine: Mutex<Option<RapidEngine>>,
}

impl RapidOcrEngine {
    pub fn from_model_dir(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            model_dir: model_dir.into(),
            engine: Mutex::new(None),
        }
    }

    fn model_paths(&self) -> (PathBuf, PathBuf, PathBuf) {
        (
            self.model_dir.join("PP-OCRv6_det_medium.onnx"),
            self.model_dir.join("PP-OCRv6_rec_medium.onnx"),
            self.model_dir.join("ppocrv6_dict.txt"),
        )
    }

    fn build(&self) -> Result<RapidEngine, OcrError> {
        let (det, rec, dict) = self.model_paths();
        for path in [&det, &rec, &dict] {
            if !path.is_file() {
                return Err(OcrError::Engine(format!(
                    "model file missing: {}",
                    path.display()
                )));
            }
        }
        let manifest_path = self.model_dir.join("manifest.json");
        if manifest_path.is_file() {
            let manifest: rapid_ocr_rs::ModelManifest = serde_json::from_slice(
                &std::fs::read(&manifest_path)
                    .map_err(|e| OcrError::Engine(format!("read model manifest: {e}")))?,
            )
            .map_err(|e| OcrError::Engine(format!("parse model manifest: {e}")))?;
            manifest
                .validate_files(&self.model_dir)
                .map_err(|e| OcrError::Engine(format!("model manifest validation: {e}")))?;
        }
        let mut config = EngineConfig::default();
        config.global.use_det = true;
        config.global.use_cls = false;
        config.global.use_rec = true;
        config.global.max_side_len = 2048;
        config.det.lang = LangDet::Multi;
        config.det.ocr_version = OcrVersion::PPocrV6;
        config.det.model_type = ModelType::Medium;
        config.det.model_path = Some(det);
        config.det.allow_download = false;
        config.rec.model = ModelConfig {
            lang: LangRec::Ch,
            ocr_version: OcrVersion::PPocrV6,
            model_type: ModelType::Medium,
            model_path: Some(rec),
            rec_keys_path: Some(dict),
            allow_download: false,
        };
        RapidEngine::new(config).map_err(|e| OcrError::Engine(e.to_string()))
    }

    fn ensure_engine(&self) -> Result<(), OcrError> {
        let mut guard = self.engine.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(self.build()?);
        }
        Ok(())
    }
}

impl OcrEngine for RapidOcrEngine {
    fn name(&self) -> &'static str {
        "rapidocr"
    }

    fn is_available(&self) -> bool {
        let files_present = self.model_paths().0.is_file()
            && self.model_paths().1.is_file()
            && self.model_paths().2.is_file();
        if !files_present {
            return false;
        }
        let manifest_path = self.model_dir.join("manifest.json");
        if !manifest_path.is_file() {
            return true;
        }
        let Ok(bytes) = std::fs::read(manifest_path) else {
            return false;
        };
        let Ok(manifest) = serde_json::from_slice::<rapid_ocr_rs::ModelManifest>(&bytes) else {
            return false;
        };
        manifest.validate_files(&self.model_dir).is_ok()
    }

    fn recognize(&self, input: &OcrInput, cancel: &OcrCancel) -> Result<OcrText, OcrError> {
        if cancel.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        let OcrInput::Png(png) = input;
        self.ensure_engine()?;
        let mut guard = self.engine.lock().unwrap_or_else(|e| e.into_inner());
        let engine = guard
            .as_mut()
            .ok_or_else(|| OcrError::Engine("rapidocr engine not initialized".into()))?;
        let result: GenericOcrOutput = GenericOcrEngine::recognize(
            engine,
            rapid_ocr_rs::OcrRequest {
                input: GenericOcrInput::Encoded(png),
                roi: None,
                scale_hint: None,
                preprocess: Default::default(),
                use_cls: false,
                return_word_box: false,
                return_single_char_box: false,
            },
        )
        .map_err(|e| OcrError::Engine(e.to_string()))?;
        if cancel.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        let layout = serde_json::to_string(&result.lines).ok();
        Ok(OcrText {
            text: result.text,
            layout,
            engine: "rapidocr",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn missing_model_directory_is_unavailable() {
        let engine = RapidOcrEngine::from_model_dir(Path::new("missing-ocr-models"));
        assert!(!engine.is_available());
    }
}
