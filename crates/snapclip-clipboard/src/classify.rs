use crate::{
    MAX_BYTES, MAX_FILES, MAX_TEXT_CHARS, PreparedSnapshot, ReadError, Snapshot, document, image,
    rtf,
};
use snapclip_model::pin::{DocumentMode, FileKind, PinContent, PinDocument, PinFile};
use std::{path::Path, sync::Arc};

pub fn prepare_files(paths: &[String]) -> Result<Vec<PinFile>, ReadError> {
    if paths.is_empty() {
        return Err(ReadError::Empty);
    }
    if paths.len() > MAX_FILES {
        return Err(ReadError::TooLarge);
    }
    Ok(paths
        .iter()
        .map(|path| {
            let file = Path::new(path);
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| path.clone());
            let extension = file
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            let kind = match extension.as_str() {
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" => FileKind::Image,
                "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" => {
                    FileKind::Document
                }
                "mp3" | "wav" | "flac" | "m4a" | "ogg" => FileKind::Audio,
                "mp4" | "mkv" | "avi" | "mov" | "webm" => FileKind::Video,
                "zip" | "7z" | "rar" | "tar" | "gz" => FileKind::Archive,
                _ => FileKind::Other,
            };
            if path.starts_with("\\\\") {
                return PinFile::new(path.clone(), name, kind)
                    .unavailable("网络位置尚未访问".into());
            }
            match std::fs::metadata(file) {
                Ok(meta) if meta.is_dir() => PinFile::new(path.clone(), name, FileKind::Folder),
                Ok(meta) => PinFile::new(path.clone(), name, kind).with_size(meta.len()),
                Err(error) => {
                    PinFile::new(path.clone(), name, kind).unavailable(match error.kind() {
                        std::io::ErrorKind::NotFound => "文件已移动或删除".into(),
                        std::io::ErrorKind::PermissionDenied => "没有访问权限".into(),
                        _ => "文件暂时不可用".into(),
                    })
                }
            }
        })
        .collect())
}

pub fn prepare(snapshot: Snapshot) -> Result<PreparedSnapshot, ReadError> {
    prepare_with_image_decoder(snapshot, image::decode)
}

/// Prepare representations using the host's color-managed decoder. The clipboard
/// crate owns bounded I/O, ordering and classification, without depending on a UI
/// renderer. The decoder must preserve the source color definition and precision.
pub fn prepare_with_image_decoder(
    snapshot: Snapshot,
    mut decode: impl FnMut(
        &[u8],
        crate::ImageEncoding,
    ) -> Result<snapclip_model::pin::PinImage, ReadError>,
) -> Result<PreparedSnapshot, ReadError> {
    if snapshot.byte_len() > MAX_BYTES {
        return Err(ReadError::TooLarge);
    }
    if snapshot.files.is_empty() && snapshot.virtual_files {
        return Err(ReadError::VirtualFiles);
    }
    let mut image = None;
    let mut image_error = ReadError::InvalidImage;
    let mut png_failed = false;
    let mut notice = None;
    // File references remain files: their incidental bitmap representation is not a preview request.
    if snapshot.files.is_empty() {
        let candidates = snapshot
            .images
            .iter()
            .filter(|(f, _)| *f == crate::ImageEncoding::Png)
            .chain(
                snapshot
                    .images
                    .iter()
                    .filter(|(f, _)| *f != crate::ImageEncoding::Png),
            );
        for (format, bytes) in candidates {
            match decode(bytes, *format) {
                Ok(decoded) => {
                    if png_failed && *format == crate::ImageEncoding::Dib && !decoded.is_hdr() {
                        notice = Some("PNG 表示无法读取，当前显示 DIB 的 SDR 副本".into());
                    }
                    image = Some(decoded);
                    break;
                }
                Err(error) => {
                    if *format == crate::ImageEncoding::Png {
                        png_failed = true;
                    }
                    if error == ReadError::TooLarge {
                        image_error = ReadError::TooLarge;
                    }
                }
            }
        }
    }
    let content = if !snapshot.files.is_empty() {
        PinContent::Files(Arc::new(prepare_files(&snapshot.files)?))
    } else {
        let rich = snapshot
            .html
            .as_deref()
            .and_then(|bytes| document::parse_html(bytes).ok());
        let rtf = if rich.is_none() {
            snapshot.rtf.as_deref().and_then(rtf::rtf_spans)
        } else {
            None
        };
        if let Some(rich) = rich.filter(|rich| {
            rich.rich
                || (snapshot
                    .text
                    .as_deref()
                    .and_then(document::http_url)
                    .is_none())
        }) {
            let mode = if rich.table.is_some() {
                DocumentMode::Table
            } else {
                DocumentMode::Rich
            };
            let mut doc = PinDocument::new(
                snapshot
                    .text
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(rich.text),
                mode,
            )
            .with_html(rich.safe);
            if let Some(table) = rich.table {
                doc = doc.with_table(table);
            }
            PinContent::Document(doc)
        } else if let Some(spans) = rtf {
            let mut safe = String::from("<div>");
            let mut text = String::new();
            for span in spans {
                text.push_str(span.text());
                let mut inline = document::escape(span.text()).replace('\n', "<br>");
                for (enabled, tag) in [
                    (span.is_bold(), "b"),
                    (span.is_italic(), "i"),
                    (span.is_underlined(), "u"),
                ] {
                    if enabled {
                        inline = format!("<{tag}>{inline}</{tag}>");
                    }
                }
                safe.push_str(&inline);
            }
            safe.push_str("</div>");
            PinContent::Document(
                PinDocument::new(
                    snapshot
                        .text
                        .clone()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(text),
                    DocumentMode::Rich,
                )
                .with_html(safe),
            )
        } else if let Some(image) = &image {
            PinContent::Image(image.clone())
        } else if let Some(text) = snapshot.text.as_ref().filter(|text| !text.is_empty()) {
            if text.chars().count() > MAX_TEXT_CHARS {
                return Err(ReadError::TooLarge);
            }
            let table = document::tsv(text);
            let mode = if document::http_url(text).is_some() {
                DocumentMode::Link
            } else if document::is_absolute_path(text) {
                DocumentMode::Path
            } else if text.trim_start().starts_with("```") && text.trim_end().ends_with("```")
                || (matches!(text.trim().as_bytes().first(), Some(b'{' | b'['))
                    && serde_json::from_str::<serde_json::Value>(text).is_ok())
            {
                DocumentMode::Code
            } else if table.is_some() {
                DocumentMode::Table
            } else {
                DocumentMode::Text
            };
            let mut doc = PinDocument::new(text.clone(), mode);
            if let Some(table) = table {
                doc = doc.with_table(table);
            }
            if snapshot.html.is_some() || snapshot.rtf.is_some() {
                doc = doc.with_notice("格式未完整保留，已显示文字");
            }
            PinContent::Document(doc)
        } else if snapshot.oversized_image && snapshot.images.is_empty() {
            return Err(ReadError::TooLarge);
        } else if !snapshot.images.is_empty() {
            return Err(image_error);
        } else if snapshot.has_formats {
            return Err(ReadError::Unsupported);
        } else {
            return Err(ReadError::Empty);
        }
    };
    if let PinContent::Document(document) = &content
        && document.text().chars().count() > MAX_TEXT_CHARS
    {
        return Err(ReadError::TooLarge);
    }
    let mut hash = blake3::Hasher::new();
    for (tag, bytes) in [
        (0, snapshot.text.as_ref().map(|s| s.as_bytes())),
        (1, snapshot.html.as_deref()),
        (2, snapshot.rtf.as_deref()),
    ] {
        if let Some(bytes) = bytes {
            hash.update(&[tag]);
            hash.update(&(bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
    }
    for path in &snapshot.files {
        hash.update(&[3]);
        hash.update(&(path.len() as u64).to_le_bytes());
        hash.update(path.as_bytes());
    }
    for (format, bytes) in &snapshot.images {
        hash.update(&[4, *format as u8]);
        hash.update(&(bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let fingerprint = *hash.finalize().as_bytes();
    Ok(PreparedSnapshot {
        snapshot: Arc::new(snapshot),
        content,
        image,
        fingerprint,
        notice,
    })
}
