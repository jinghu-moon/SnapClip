//! OCR engine contract.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone)]
pub enum OcrInput {
    Png(Arc<[u8]>),
}

#[derive(Debug, Clone)]
pub struct OcrText {
    pub text: String,
    pub layout: Option<String>,
    pub engine: &'static str,
}

#[derive(Debug, Clone)]
pub enum OcrError {
    LanguageUnavailable,
    Decode,
    #[allow(dead_code)]
    Timeout,
    Cancelled,
    #[allow(dead_code)]
    Engine(String),
}

impl std::fmt::Display for OcrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LanguageUnavailable => write!(f, "language unavailable"),
            Self::Decode => write!(f, "decode failed"),
            Self::Timeout => write!(f, "timeout"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Engine(message) => write!(f, "engine: {message}"),
        }
    }
}

impl OcrError {
    pub fn code(&self) -> crate::domain::OcrErrorCode {
        use crate::domain::OcrErrorCode;
        match self {
            Self::LanguageUnavailable => OcrErrorCode::LanguageUnavailable,
            Self::Decode => OcrErrorCode::DecodeFailed,
            Self::Timeout => OcrErrorCode::Timeout,
            Self::Cancelled => OcrErrorCode::Cancelled,
            Self::Engine(_) => OcrErrorCode::EngineFailed,
        }
    }
}

#[derive(Clone, Default)]
pub struct OcrCancel(Arc<AtomicBool>);

impl OcrCancel {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

pub trait OcrEngine: Send + Sync {
    fn name(&self) -> &'static str;
    fn is_available(&self) -> bool;
    fn recognize(&self, input: &OcrInput, cancel: &OcrCancel) -> Result<OcrText, OcrError>;
}
