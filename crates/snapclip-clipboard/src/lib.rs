//! Clipboard I/O and bounded content preparation. No UI, database, capture or OCR dependency.
mod classify;
pub mod document;
pub mod image;
pub mod rtf;
#[cfg(windows)]
pub mod windows;
mod writer;

pub use classify::{prepare, prepare_files};
use snapclip_model::pin::{PinContent, PinImage, PinSnapshot};
use std::sync::Arc;
pub use writer::{Channels, ClipboardImage, SystemClipboard};

pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 24_000_000;
pub const MAX_TEXT_CHARS: usize = 200_000;
pub const MAX_FILES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPurpose {
    Monitor,
    ExplicitPin,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageEncoding {
    Png,
    Dib,
}
#[derive(Debug, Clone)]
pub struct Snapshot {
    sequence: u32,
    completed_sequence: u32,
    owner: usize,
    foreground: usize,
    text: Option<String>,
    html: Option<Vec<u8>>,
    rtf: Option<Vec<u8>>,
    files: Vec<String>,
    images: Vec<(ImageEncoding, Vec<u8>)>,
    oversized_image: bool,
    virtual_files: bool,
    has_formats: bool,
    excluded: bool,
}
impl Snapshot {
    pub fn new(sequence: u32) -> Self {
        Self {
            sequence,
            completed_sequence: sequence,
            owner: 0,
            foreground: 0,
            text: None,
            html: None,
            rtf: None,
            files: vec![],
            images: vec![],
            oversized_image: false,
            virtual_files: false,
            has_formats: false,
            excluded: false,
        }
    }
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }
    pub fn with_html(mut self, bytes: Vec<u8>) -> Self {
        self.html = Some(bytes);
        self
    }
    pub fn with_rtf(mut self, bytes: Vec<u8>) -> Self {
        self.rtf = Some(bytes);
        self
    }
    pub fn with_files(mut self, files: Vec<String>) -> Self {
        self.files = files;
        self
    }
    pub fn with_virtual_files(mut self, present: bool) -> Self {
        self.virtual_files = present;
        self
    }
    pub fn with_image(mut self, encoding: ImageEncoding, bytes: Vec<u8>) -> Self {
        self.images.push((encoding, bytes));
        self
    }
    pub fn sequence(&self) -> u32 {
        self.sequence
    }
    pub fn completed_sequence(&self) -> u32 {
        self.completed_sequence
    }
    pub fn owner(&self) -> usize {
        self.owner
    }
    pub fn foreground(&self) -> usize {
        self.foreground
    }
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
    pub fn html(&self) -> Option<&[u8]> {
        self.html.as_deref()
    }
    pub fn rtf(&self) -> Option<&[u8]> {
        self.rtf.as_deref()
    }
    pub fn files(&self) -> &[String] {
        &self.files
    }
    pub fn images(&self) -> &[(ImageEncoding, Vec<u8>)] {
        &self.images
    }
    pub fn is_excluded(&self) -> bool {
        self.excluded
    }
    pub fn byte_len(&self) -> usize {
        self.text.as_ref().map_or(0, String::len)
            + self.html.as_ref().map_or(0, Vec::len)
            + self.rtf.as_ref().map_or(0, Vec::len)
            + self.files.iter().map(String::len).sum::<usize>()
            + self
                .images
                .iter()
                .map(|(_, bytes)| bytes.len())
                .sum::<usize>()
    }
}

#[derive(Debug, Clone)]
pub struct PreparedSnapshot {
    snapshot: Arc<Snapshot>,
    content: PinContent,
    image: Option<PinImage>,
    fingerprint: [u8; 32],
}
impl PreparedSnapshot {
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn content(&self) -> &PinContent {
        &self.content
    }
    pub fn image(&self) -> Option<&PinImage> {
        self.image.as_ref()
    }
    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
    pub fn into_pin(self) -> PinSnapshot {
        let mut pin = PinSnapshot::new(self.content);
        if let Some(text) = &self.snapshot.text {
            pin = pin.with_text(text.clone());
        }
        for (name, bytes) in [("HTML", &self.snapshot.html), ("RTF", &self.snapshot.rtf)] {
            if let Some(bytes) = bytes {
                pin = pin.with_source(name.into(), bytes.clone());
            }
        }
        if let Some(image) = self
            .image
            .filter(|_| !matches!(pin.content(), PinContent::Image(_)))
        {
            pin = pin.with_image(image);
        }
        pin
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    Empty,
    Busy,
    Changed,
    Unsupported,
    VirtualFiles,
    TooLarge,
    InvalidImage,
    InvalidDocument,
    Platform(String),
}
impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Empty => "剪贴板中没有可贴出的内容",
            Self::Busy => "无法读取剪贴板，请稍后重试",
            Self::Changed => "剪贴板已变化，请重新按 F3",
            Self::Unsupported => "当前剪贴板格式暂不支持",
            Self::VirtualFiles => "暂不支持直接贴出此附件，请先保存到本地文件",
            Self::TooLarge => "内容超过当前贴图上限",
            Self::InvalidImage => "无法读取图片",
            Self::InvalidDocument => "无法读取文档，且没有可用文字",
            Self::Platform(message) => message,
        })
    }
}
impl std::error::Error for ReadError {}
