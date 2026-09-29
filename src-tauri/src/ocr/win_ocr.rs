//! Windows.Media.Ocr engine (WinRT).
//!
//! Threading contract: call `init_apartment()` once on the OCR worker thread.
//! `recognize` runs on that worker; timeout/cancel calls `IAsyncOperation::Cancel`
//! then joins the await helper so no thread or WinRT op is leaked.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

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

/// Initialize WinRT/COM apartment on the long-lived OCR worker thread.
#[cfg(windows)]
pub fn init_apartment() -> Result<(), OcrError> {
    use ::windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|e| OcrError::Engine(e.to_string()))
}

#[cfg(not(windows))]
pub fn init_apartment() -> Result<(), OcrError> {
    Ok(())
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
    use std::sync::mpsc;
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
    writer.FlushAsync().map_err(map_engine)?.get().map_err(map_engine)?;
    drop(writer);
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
    let op = engine
        .RecognizeAsync(&bgra)
        .map_err(|e| OcrError::Engine(e.to_string()))?;

    // Await on one helper thread; on timeout/cancel call Cancel() then join — no leak.
    let await_op = op.clone();
    let (tx, rx) = mpsc::channel();
    let helper = std::thread::Builder::new()
        .name("snapclip-ocr-await".into())
        .spawn(move || {
            let result = await_op
                .get()
                .map_err(map_engine)
                .and_then(|result| result.Text().map_err(map_engine));
            let _ = tx.send(result);
        })
        .map_err(map_engine)?;

    let deadline = Instant::now() + Duration::from_secs(10);
    let outcome = loop {
        if cancel.is_cancelled() {
            let _ = op.Cancel();
            break Err(OcrError::Cancelled);
        }
        if Instant::now() >= deadline {
            let _ = op.Cancel();
            break Err(OcrError::Timeout);
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => break result,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(OcrError::Engine("ocr await dropped".into()))
            }
        }
    };

    // Always join the helper so the thread cannot outlive this call.
    let _ = helper.join();
    let text = outcome?.to_string();
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
