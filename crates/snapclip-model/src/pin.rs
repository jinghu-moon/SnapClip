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
    width: u32,
    height: u32,
    rgba: Arc<Vec<u8>>,
}
impl PinImage {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        (width > 0
            && height > 0
            && u64::from(width)
                .checked_mul(u64::from(height))
                .and_then(|n| n.checked_mul(4))
                == Some(rgba.len() as u64))
        .then(|| Self {
            width,
            height,
            rgba: Arc::new(rgba),
        })
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
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
}
impl PinSnapshot {
    pub fn new(content: PinContent) -> Self {
        Self {
            content,
            text: None,
            sources: vec![],
            image: None,
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
        let image = |i: &PinImage| i.rgba().len();
        let primary = match &self.content {
            PinContent::Image(i) => image(i) * 2,
            PinContent::Document(d) => d.text.len() + d.html.as_ref().map_or(0, |h| h.len()),
            PinContent::Files(f) => f.iter().map(|f| f.path.len() + f.name.len()).sum(),
        };
        primary
            + self.text.as_ref().map_or(0, |t| t.len())
            + self.sources.iter().map(|(_, s)| s.len()).sum::<usize>()
            + self.image.as_ref().map_or(0, image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn images_require_exact_nonempty_pixels() {
        assert!(PinImage::new(0, 1, vec![]).is_none());
        assert!(PinImage::new(u32::MAX, u32::MAX, vec![]).is_none());
        assert!(PinImage::new(2, 2, vec![0; 15]).is_none());
        assert_eq!(
            PinImage::new(1, 1, vec![1, 2, 3, 4]).unwrap().rgba(),
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
