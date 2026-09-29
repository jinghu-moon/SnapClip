//! Windows.Media.Ocr engine (WinRT) with cancellation.

use super::engine::{OcrCancel, OcrEngine, OcrError, OcrInput, OcrText};

pub struct WindowsOcrEngine;

impl OcrEngine for WindowsOcrEngine {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn is_available(&self) -> bool {
        #[cfg(windows)]
        {
            engine_create().is_ok()
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
    use std::time::{Duration, Instant};
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

    // Await on a helper thread so this worker can honor cancel/timeout.
    // On cancel/timeout we stop waiting; the WinRT operation is left to finish
    // in the background (no IAsyncInfo::Cancel binding on this windows crate version).
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("snapclip-ocr-await".into())
        .spawn(move || {
            let _ = tx.send(op.get().map(|result| result.Text().map(|t| t.to_string())));
        })
        .map_err(map_engine)?;

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if cancel.is_cancelled() {
            return Err(OcrError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(OcrError::Timeout);
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(Ok(Ok(text))) => {
                return Ok(OcrText {
                    text,
                    engine: "windows",
                })
            }
            Ok(Ok(Err(_))) => return Err(OcrError::Engine("read text failed".into())),
            Ok(Err(_)) => return Err(OcrError::Engine("recognize failed".into())),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(OcrError::Engine("ocr await dropped".into()))
            }
        }
    }
}

#[cfg(windows)]
fn map_engine<E: std::fmt::Display>(error: E) -> OcrError {
    OcrError::Engine(error.to_string())
}

#[cfg(not(windows))]
fn recognize_png(_png: &[u8], _cancel: &OcrCancel) -> Result<OcrText, OcrError> {
    Err(OcrError::Engine("windows only".into()))
}
