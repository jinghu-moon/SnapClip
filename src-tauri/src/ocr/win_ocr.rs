//! Windows.Media.Ocr engine (WinRT).
//!
//! Threading contract: call `init_apartment()` once on the OCR worker thread.
//! All WinRT image decoding and OCR calls stay on that same apartment, matching
//! the Windows OCR usage pattern used by clipvault.

use std::sync::OnceLock;

use super::engine::{OcrCancel, OcrEngine, OcrError, OcrInput, OcrText};

pub struct WindowsOcrEngine {
    available: OnceLock<bool>,
}

impl Default for WindowsOcrEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsOcrEngine {
    pub fn new() -> Self {
        Self {
            available: OnceLock::new(),
        }
    }
}

/// RAII COM apartment for the OCR worker thread.
pub struct ComApartment {
    _private: (),
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            ::windows::Win32::System::Com::CoUninitialize();
        }
    }
}

/// Initialize WinRT/COM apartment on the long-lived OCR worker thread.
#[cfg(windows)]
pub fn init_apartment() -> Result<ComApartment, OcrError> {
    use ::windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|e| OcrError::Engine(e.to_string()))?;
    Ok(ComApartment { _private: () })
}

#[cfg(not(windows))]
pub fn init_apartment() -> Result<ComApartment, OcrError> {
    Ok(ComApartment { _private: () })
}

impl OcrEngine for WindowsOcrEngine {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn is_available(&self) -> bool {
        #[cfg(windows)]
        {
            *self.available.get_or_init(|| engine_create().is_ok())
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    fn recognize(&self, input: &OcrInput, cancel: &OcrCancel) -> Result<OcrText, OcrError> {
        if cancel.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        let OcrInput::Png(png) = input;
        recognize_png(png, cancel)
    }
}

#[cfg(windows)]
fn engine_create() -> Result<::windows::Media::Ocr::OcrEngine, OcrError> {
    ::windows::Media::Ocr::OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|_| OcrError::LanguageUnavailable)
}

#[cfg(windows)]
fn recognize_png(png: &[u8], cancel: &OcrCancel) -> Result<OcrText, OcrError> {
    use ::windows::Graphics::Imaging::{BitmapDecoder, BitmapPixelFormat, SoftwareBitmap};
    use ::windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

    if cancel.is_cancelled() {
        return Err(OcrError::Cancelled);
    }

    let stream = InMemoryRandomAccessStream::new().map_err(map_engine)?;
    let writer = DataWriter::CreateDataWriter(&stream).map_err(map_engine)?;
    writer.WriteBytes(png).map_err(map_engine)?;
    writer
        .StoreAsync()
        .map_err(map_engine)?
        .get()
        .map_err(map_engine)?;
    writer
        .FlushAsync()
        .map_err(map_engine)?
        .get()
        .map_err(map_engine)?;
    // DataWriter closes its attached output stream when dropped. Detach it
    // first, otherwise BitmapDecoder sees 0x80000013 (object closed).
    let _output_stream = writer.DetachStream().map_err(map_engine)?;
    stream.Seek(0).map_err(map_engine)?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(map_engine)?
        .get()
        .map_err(|_| OcrError::Decode)?;
    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(map_engine)?
        .get()
        .map_err(|_| OcrError::Decode)?;
    let bgra = SoftwareBitmap::Convert(&bitmap, BitmapPixelFormat::Bgra8).map_err(map_engine)?;

    if cancel.is_cancelled() {
        return Err(OcrError::Cancelled);
    }

    let engine = engine_create()?;
    let result = engine
        .RecognizeAsync(&bgra)
        .map_err(|e| OcrError::Engine(format!("RecognizeAsync: {e}")))?
        .get()
        .map_err(|e| OcrError::Engine(format!("RecognizeAsync.get: {e}")))?;
    if cancel.is_cancelled() {
        return Err(OcrError::Cancelled);
    }
    let text = result
        .Text()
        .map_err(|e| OcrError::Engine(format!("OcrResult.Text: {e}")))?
        .to_string();
    Ok(OcrText {
        text,
        engine: "windows",
    })
}

#[cfg(windows)]
fn map_engine<E: std::fmt::Display>(error: E) -> OcrError {
    OcrError::Engine(error.to_string())
}

#[cfg(not(windows))]
fn recognize_png(_png: &[u8], _cancel: &OcrCancel) -> Result<OcrText, OcrError> {
    Err(OcrError::Engine("windows only".into()))
}
