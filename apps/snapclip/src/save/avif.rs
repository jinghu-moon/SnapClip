//! HDR AVIF uses the shared CPU codec and the existing atomic commit service.
use super::{SaveError, SaveOutcome, SaveRequest, service};
use snapclip_avif::{Stop, StopToken};
use snapclip_model::pixels::PixelBuffer;
use std::io::{self, Write};

pub(crate) fn write_pixels(
    pixels: &PixelBuffer,
    output: &mut impl Write,
    stop: StopToken,
) -> io::Result<()> {
    let [width, _] = pixels.size();
    snapclip_avif::write_hdr(
        &mut crate::capture::pixel_export::PixelBands(pixels),
        width,
        &std::env::temp_dir(),
        output,
        stop,
    )
    .map(|_| ())
    .map_err(snapclip_avif::Error::into_io)
}

pub(crate) fn write_source(
    source: &snapclip_model::CaptureSource,
    output: &mut impl Write,
    stop: StopToken,
) -> io::Result<()> {
    if source.artifact.dimensions.width != source.metadata.width
        || source.artifact.dimensions.height != source.metadata.height
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "history source geometry differs from its reference",
        ));
    }
    if !snapclip_avif::within_export_geometry(
        u64::from(source.metadata.width),
        u64::from(source.metadata.height),
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "AVIF grid geometry budget exceeded",
        ));
    }
    let master = snapclip_history::pixel_master::PixelMaster::open(&source.artifact)
        .map_err(io::Error::other)?;
    if master.metadata() != &source.metadata {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "history master metadata differs from its reference",
        ));
    }
    snapclip_avif::write_hdr(
        &mut crate::capture::pixel_export::MasterBands(master),
        source.metadata.width,
        &std::env::temp_dir(),
        output,
        stop,
    )
    .map(|_| ())
    .map_err(snapclip_avif::Error::into_io)
}

pub(crate) struct Limited<W> {
    inner: W,
    remaining: u64,
}
impl<W> Limited<W> {
    pub(crate) fn new(inner: W, limit: Option<u64>) -> Self {
        Self {
            inner,
            remaining: limit.unwrap_or(u64::MAX),
        }
    }
}
impl<W: Write> Write for Limited<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "AVIF 超出体积上限，请缩小尺寸或提高上限",
            ));
        }
        let count = self.inner.write(bytes)?;
        self.remaining -= count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
struct Cancel(StopToken);
impl service::Cancel for Cancel {
    fn cancelled(&mut self) -> bool {
        self.0.check().is_err()
    }
}
pub(crate) fn save_pixels(
    pixels: &PixelBuffer,
    request: &SaveRequest,
    limit: Option<u64>,
    stop: StopToken,
) -> Result<SaveOutcome, SaveError> {
    super::caps::check(
        request.format,
        u64::from(pixels.size()[0]),
        u64::from(pixels.size()[1]),
    )
    .map_err(SaveError::Capability)?;
    if request.format != snapclip_capture::save_format::SaveFormat::Avif {
        return Err(SaveError::UnsupportedFormat(request.format));
    }
    service::commit_stream(request, &mut Cancel(stop.clone()), |file| {
        write_pixels(pixels, &mut Limited::new(file, limit), stop)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapclip_avif::{Error, Unstoppable};
    use snapclip_model::{
        PixelFormat,
        pixels::{AlphaMode, PixelDescriptor, PixelEncoding},
    };
    #[test]
    fn hdr_atomic_save_decodes_and_failures_keep_the_old_target_and_clean_partials() {
        let dir = std::env::temp_dir().join(format!("snapclip-avif-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let descriptor = PixelDescriptor::new(
            PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap();
        let bytes = [12.5_f32, 12.5, 12.5, 0.5]
            .into_iter()
            .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
            .collect();
        let pixels = PixelBuffer::from_tight(1, 1, descriptor, bytes).unwrap();
        let original = pixels.as_bytes().to_vec();
        let request = SaveRequest {
            directory: dir.clone(),
            stem: "hdr".into(),
            format: snapclip_capture::save_format::SaveFormat::Avif,
            conflict: service::ConflictPolicy::Overwrite,
        };
        let target = dir.join("hdr.avif");
        let mut preview = Vec::new();
        let mut limited = Limited::new(&mut preview, Some(5));
        limited.write_all(b"1234").unwrap();
        assert_eq!(
            limited.write_all(b"56").unwrap_err().kind(),
            io::ErrorKind::FileTooLarge
        );
        assert_eq!(
            preview, b"1234",
            "compressed previews must not grow past their limit"
        );
        std::fs::write(&target, b"old").unwrap();
        assert!(save_pixels(&pixels, &request, Some(8), StopToken::new(Unstoppable)).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        let stop = snapclip_avif::Stopper::new();
        stop.cancel();
        assert!(matches!(
            save_pixels(&pixels, &request, None, stop.into()),
            Err(SaveError::Cancelled)
        ));
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        let saved = save_pixels(&pixels, &request, None, StopToken::new(Unstoppable)).unwrap();
        let encoded = std::fs::read(&saved.path).unwrap();
        let decoded = snapclip_avif::decode(&encoded, &Unstoppable).unwrap();
        let r = half::f16::from_le_bytes(decoded.as_bytes()[..2].try_into().unwrap()).to_f32();
        assert!((r - 12.5).abs() < 0.08);
        assert_eq!(saved.bytes, encoded.len() as u64);
        let bad = PixelBuffer::from_tight(1, 1, PixelDescriptor::bgra8(), vec![0; 4]).unwrap();
        assert!(save_pixels(&bad, &request, None, StopToken::new(Unstoppable)).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), encoded);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert_eq!(pixels.as_bytes(), original);
        assert_eq!(
            Error::Cancelled.into_io().kind(),
            io::ErrorKind::Interrupted
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
