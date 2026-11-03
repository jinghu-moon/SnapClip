//! Prepared, immutable content shared by the clipboard producer and pin presenter.
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentMode {
    Text,
    Rich,
    Table,
    Code,
    Link,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSpan {
    text: String,
    bold: bool,
    italic: bool,
    underline: bool,
}
impl TextSpan {
    pub fn new(text: String, bold: bool, italic: bool, underline: bool) -> Self {
        Self {
            text,
            bold,
            italic,
            underline,
        }
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn is_bold(&self) -> bool {
        self.bold
    }
    pub fn is_italic(&self) -> bool {
        self.italic
    }
    pub fn is_underlined(&self) -> bool {
        self.underline
    }
}

#[derive(Debug, Clone)]
pub struct PinImage {
    source: crate::pixels::PixelSource,
    pixels: Arc<crate::pixels::PixelBuffer>,
}
impl PinImage {
    pub fn new(width: u32, height: u32, mut rgba: Vec<u8>) -> Option<Self> {
        let descriptor = crate::pixels::PixelDescriptor::bgra8();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let pixels =
            crate::pixels::PixelBuffer::from_tight(width, height, descriptor, rgba).ok()?;
        Self::from_pixels(
            crate::pixels::PixelSource::new(descriptor, 0),
            Arc::new(pixels),
        )
    }
    pub fn from_pixels(
        source: crate::pixels::PixelSource,
        pixels: Arc<crate::pixels::PixelBuffer>,
    ) -> Option<Self> {
        (source.descriptor() == pixels.descriptor()).then_some(Self { source, pixels })
    }
    pub fn pixel_source(&self) -> crate::pixels::PixelSource {
        self.source
    }
    pub fn pixels(&self) -> &Arc<crate::pixels::PixelBuffer> {
        &self.pixels
    }
    pub fn is_hdr(&self) -> bool {
        self.source.descriptor().encoding() == crate::pixels::PixelEncoding::ScRgb
    }
    /// Compatibility copying is always an explicit SDR derivative. Display and
    /// re-export read `pixels()`, never this buffer.
    pub fn sdr_rgba(&self) -> Result<Vec<u8>, &'static str> {
        let peak = if self.is_hdr() {
            crate::pixel_color::linear_rgba(&self.pixels)
                .filter(|p| p[3] > 0.0)
                .map(|p| p[0].max(p[1]).max(p[2]) * 80.0)
                .fold(100.0_f64, f64::max)
        } else {
            100.0
        };
        let mut bytes = crate::pixel_color::to_sdr_bgra8(&self.pixels, peak)?.into_bytes();
        for pixel in bytes.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        Ok(bytes)
    }
    pub fn width(&self) -> u32 {
        self.pixels.size()[0]
    }
    pub fn height(&self) -> u32 {
        self.pixels.size()[1]
    }
}

#[derive(Debug, Clone)]
pub struct PinTable {
    rows: Vec<Vec<String>>,
    header: bool,
}
impl PinTable {
    pub const MAX_CELLS: usize = 16_384;
    pub const MAX_COLUMNS: usize = 256;

    pub fn new(rows: Vec<Vec<String>>, header: bool) -> Option<Self> {
        let columns = rows.first()?.len();
        (columns > 0
            && columns <= Self::MAX_COLUMNS
            && rows
                .len()
                .checked_mul(columns)
                .is_some_and(|cells| cells <= Self::MAX_CELLS)
            && rows.iter().all(|row| row.len() == columns))
        .then_some(Self { rows, header })
    }
    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }
    pub fn has_header(&self) -> bool {
        self.header
    }
    pub fn columns(&self) -> usize {
        self.rows[0].len()
    }
}

#[derive(Debug, Clone)]
pub struct PinDocument {
    text: Arc<str>,
    mode: DocumentMode,
    html: Option<Arc<str>>,
    table: Option<PinTable>,
    notice: Option<Arc<str>>,
}
impl PinDocument {
    pub fn new(text: String, mode: DocumentMode) -> Self {
        Self {
            text: text.into(),
            mode,
            html: None,
            table: None,
            notice: None,
        }
    }
    pub fn with_html(mut self, html: String) -> Self {
        self.html = Some(html.into());
        self
    }
    pub fn with_table(mut self, table: PinTable) -> Self {
        self.table = Some(table);
        self
    }
    pub fn with_notice(mut self, notice: impl Into<Arc<str>>) -> Self {
        self.notice = Some(notice.into());
        self
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn mode(&self) -> DocumentMode {
        self.mode
    }
    /// Whitelisted, resource-free HTML prepared by the clipboard capability, never raw CF_HTML.
    pub fn html(&self) -> Option<&str> {
        self.html.as_deref()
    }
    pub fn table(&self) -> Option<&PinTable> {
        self.table.as_ref()
    }
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Image,
    Document,
    Audio,
    Video,
    Archive,
    Folder,
    Other,
}

#[derive(Debug, Clone)]
pub struct PinFile {
    path: String,
    name: String,
    kind: FileKind,
    bytes: Option<u64>,
    unavailable: Option<String>,
}
impl PinFile {
    pub fn new(path: String, name: String, kind: FileKind) -> Self {
        Self {
            path,
            name,
            kind,
            bytes: None,
            unavailable: None,
        }
    }
    pub fn with_size(mut self, bytes: u64) -> Self {
        self.bytes = Some(bytes);
        self
    }
    pub fn unavailable(mut self, reason: String) -> Self {
        self.unavailable = Some(reason);
        self
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn kind(&self) -> FileKind {
        self.kind
    }
    pub fn bytes(&self) -> Option<u64> {
        self.bytes
    }
    pub fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }
}

#[derive(Debug, Clone)]
pub enum PinContent {
    Image(PinImage),
    Document(PinDocument),
    Files(Arc<Vec<PinFile>>),
}

/// The same copied object and its actual alternative representations, never synthesized previews.
#[derive(Debug, Clone)]
pub struct PinSnapshot {
    content: PinContent,
    text: Option<Arc<str>>,
    sources: Vec<(String, Arc<[u8]>)>,
    image: Option<PinImage>,
    notice: Option<Arc<str>>,
}
impl PinSnapshot {
    pub fn new(content: PinContent) -> Self {
        Self {
            content,
            text: None,
            sources: vec![],
            image: None,
            notice: None,
        }
    }
    pub fn with_text(mut self, text: impl Into<Arc<str>>) -> Self {
        self.text = Some(text.into());
        self
    }
    pub fn with_source(mut self, name: String, source: impl Into<Arc<[u8]>>) -> Self {
        self.sources.push((name, source.into()));
        self
    }
    pub fn with_image(mut self, image: PinImage) -> Self {
        self.image = Some(image);
        self
    }
    pub fn content(&self) -> &PinContent {
        &self.content
    }
    pub fn with_notice(mut self, notice: impl Into<Arc<str>>) -> Self {
        self.notice = Some(notice.into());
        self
    }
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
    pub fn sources(&self) -> &[(String, Arc<[u8]>)] {
        &self.sources
    }
    pub fn image(&self) -> Option<&PinImage> {
        self.image.as_ref()
    }
    pub fn retained_bytes(&self) -> usize {
        let image = |i: &PinImage| i.pixels.as_bytes().len();
        let primary = match &self.content {
            PinContent::Image(i) => image(i) * 2,
            PinContent::Document(d) => d.text.len() + d.html.as_ref().map_or(0, |h| h.len()),
            PinContent::Files(f) => f.iter().map(|f| f.path.len() + f.name.len()).sum(),
        };
        primary
            + self.notice.as_ref().map_or(0, |s| s.len())
            + self.text.as_ref().map_or(0, |t| t.len())
            + self.sources.iter().map(|(_, s)| s.len()).sum::<usize>()
            + self.image.as_ref().map_or(0, image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hdr_pin_keeps_typed_source_and_uses_an_explicit_sdr_copy() {
        use crate::pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding, PixelSource};
        let descriptor = PixelDescriptor::new(
            crate::PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .unwrap();
        let bytes: Vec<u8> = [2.5_f32, 2.5, 2.5, 0.5]
            .into_iter()
            .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
            .collect();
        let source = PixelSource::new(descriptor, 19);
        let image = PinImage::from_pixels(
            source,
            Arc::new(PixelBuffer::from_tight(1, 1, descriptor, bytes.clone()).unwrap()),
        )
        .unwrap();
        assert_eq!(image.pixel_source(), source);
        assert_eq!(image.pixels().as_bytes(), bytes);
        assert_eq!(image.sdr_rgba().unwrap()[3], 128);
        assert_eq!(image.pixels().as_bytes(), bytes);
        assert!(
            PinImage::from_pixels(
                PixelSource::new(PixelDescriptor::bgra8(), 19),
                image.pixels().clone()
            )
            .is_none()
        );
    }
    #[test]
    fn images_require_exact_nonempty_pixels() {
        assert!(PinImage::new(0, 1, vec![]).is_none());
        assert!(PinImage::new(u32::MAX, u32::MAX, vec![]).is_none());
        assert!(PinImage::new(2, 2, vec![0; 15]).is_none());
        assert_eq!(
            PinImage::new(1, 1, vec![1, 2, 3, 4])
                .unwrap()
                .sdr_rgba()
                .unwrap(),
            &[1, 2, 3, 4]
        );
    }
    #[test]
    fn tables_keep_empty_cells_and_require_rectangular_rows() {
        let t = PinTable::new(vec![vec!["".into(), "a".into()]], false).unwrap();
        assert_eq!(t.columns(), 2);
        assert!(!t.has_header());
        assert!(PinTable::new(vec![vec!["a".into()], vec![]], false).is_none());
    }
    #[test]
    fn table_projection_is_bounded_without_truncating_rows_or_columns() {
        assert!(PinTable::new(vec![vec![String::new(); 256]; 64], false).is_some());
        assert!(PinTable::new(vec![vec![String::new(); 257]], false).is_none());
        assert!(PinTable::new(vec![vec![String::new(); 256]; 65], false).is_none());
    }
}
