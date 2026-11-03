//! Convert rich clipboard formats to a bounded resource-free native text representation.
use crate::ReadError;
use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use snapclip_model::pin::PinTable;

const MAX_RICH_BYTES: usize = 2 * 1024 * 1024;
const MAX_NODES: usize = 30_000;

pub struct ParsedHtml {
    pub(crate) safe: String,
    pub(crate) text: String,
    pub(crate) table: Option<PinTable>,
    pub(crate) rich: bool,
}

/// CF_HTML offsets are UTF-8 byte offsets into the entire envelope, never character offsets.
pub fn html_fragment(bytes: &[u8]) -> Result<&str, ReadError> {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    if bytes.len() > MAX_RICH_BYTES {
        return Err(ReadError::TooLarge);
    }
    if bytes.starts_with(b"Version:") {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]);
        let offset = |key: &str| {
            head.lines()
                .find_map(|line| line.trim().strip_prefix(key))
                .and_then(|n| n.trim().parse::<usize>().ok())
        };
        let start = offset("StartFragment:").ok_or(ReadError::InvalidDocument)?;
        let end = offset("EndFragment:").ok_or(ReadError::InvalidDocument)?;
        let fragment = bytes.get(start..end).ok_or(ReadError::InvalidDocument)?;
        return std::str::from_utf8(fragment).map_err(|_| ReadError::InvalidDocument);
    }
    std::str::from_utf8(bytes).map_err(|_| ReadError::InvalidDocument)
}

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Default)]
struct Build {
    safe: String,
    text: String,
    outside_table: String,
    tables: Vec<PinTable>,
    rich: bool,
    nodes: usize,
    cells: usize,
}

impl Build {
    fn walk(&mut self, node: &Handle, depth: usize, in_table: bool) -> Result<(), ReadError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES || depth > 128 {
            return Err(ReadError::TooLarge);
        }
        match &node.data {
            NodeData::Text { contents } => {
                let text = contents.borrow();
                self.safe.push_str(&escape(&text));
                self.text.push_str(&text);
                if !in_table {
                    self.outside_table.push_str(&text);
                }
            }
            NodeData::Element { name, attrs, .. } => {
                let tag = name.local.as_ref();
                if matches!(tag, "td" | "th") {
                    self.cells += 1;
                    if self.cells > PinTable::MAX_CELLS {
                        return Err(ReadError::TooLarge);
                    }
                }
                if tag == "tr" && node.children.borrow().iter().filter(|child|matches!(&child.data,NodeData::Element{name,..} if matches!(name.local.as_ref(),"td"|"th"))).count() > PinTable::MAX_COLUMNS {
                    return Err(ReadError::TooLarge);
                }
                if matches!(
                    tag,
                    "script"
                        | "style"
                        | "iframe"
                        | "object"
                        | "embed"
                        | "svg"
                        | "math"
                        | "head"
                        | "form"
                        | "input"
                        | "button"
                        | "textarea"
                        | "video"
                        | "audio"
                        | "img"
                        | "link"
                        | "meta"
                ) {
                    return Ok(());
                }
                let allowed = matches!(
                    tag,
                    "p" | "div"
                        | "br"
                        | "b"
                        | "strong"
                        | "i"
                        | "em"
                        | "u"
                        | "s"
                        | "ul"
                        | "ol"
                        | "li"
                        | "pre"
                        | "code"
                        | "blockquote"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "a"
                        | "table"
                        | "thead"
                        | "tbody"
                        | "tfoot"
                        | "tr"
                        | "td"
                        | "th"
                );
                if allowed {
                    self.safe.push('<');
                    self.safe.push_str(tag);
                    if tag == "a"
                        && let Some(href) = attrs
                            .borrow()
                            .iter()
                            .find(|a| a.name.local.as_ref() == "href")
                        && let Some(url) = http_url(&href.value)
                    {
                        self.safe.push_str(" href=\"");
                        self.safe.push_str(&escape(&url));
                        self.safe.push('"');
                    }
                    self.safe.push('>');
                    self.rich |= matches!(
                        tag,
                        "b" | "strong"
                            | "i"
                            | "em"
                            | "u"
                            | "s"
                            | "ul"
                            | "ol"
                            | "li"
                            | "pre"
                            | "code"
                            | "blockquote"
                            | "h1"
                            | "h2"
                            | "h3"
                            | "h4"
                            | "table"
                    );
                }
                if tag == "table"
                    && !in_table
                    && let Some(table) = table_from(node)
                {
                    self.tables.push(table);
                }
                for child in node.children.borrow().iter() {
                    self.walk(child, depth + 1, in_table || tag == "table")?;
                }
                if allowed && tag != "br" {
                    self.safe.push_str("</");
                    self.safe.push_str(tag);
                    self.safe.push('>');
                }
                if matches!(
                    tag,
                    "p" | "div"
                        | "br"
                        | "li"
                        | "pre"
                        | "blockquote"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "tr"
                ) {
                    self.text.push('\n');
                    if !in_table {
                        self.outside_table.push('\n');
                    }
                }
                if matches!(tag, "td" | "th") {
                    self.text.push('\t');
                }
            }
            _ => {
                for child in node.children.borrow().iter() {
                    self.walk(child, depth + 1, in_table)?;
                }
            }
        }
        Ok(())
    }
}

fn text_of(node: &Handle, out: &mut String) {
    if let NodeData::Text { contents } = &node.data {
        out.push_str(&contents.borrow());
    }
    if let NodeData::Element { name, .. } = &node.data
        && name.local.as_ref() == "br"
    {
        out.push('\n');
    }
    for child in node.children.borrow().iter() {
        text_of(child, out);
    }
}
fn table_from(node: &Handle) -> Option<PinTable> {
    fn nested_table(node: &Handle) -> bool {
        node.children.borrow().iter().any(|child| {
            matches!(&child.data, NodeData::Element {name,..} if name.local.as_ref() == "table")
                || nested_table(child)
        })
    }
    if nested_table(node) {
        return None;
    }
    fn rows(node: &Handle, output: &mut Vec<Vec<String>>, header: &mut bool) -> Option<()> {
        if let NodeData::Element { name, .. } = &node.data
            && name.local.as_ref() == "tr"
        {
            let mut row = vec![];
            let mut all_headers = true;
            for cell in node.children.borrow().iter() {
                if let NodeData::Element { name, attrs, .. } = &cell.data
                    && matches!(name.local.as_ref(), "td" | "th")
                {
                    // Spans cannot be represented by a rectangular projection: rich view keeps them.
                    if attrs.borrow().iter().any(|a| {
                        matches!(a.name.local.as_ref(), "rowspan" | "colspan")
                            && a.value.as_ref() != "1"
                    }) {
                        return None;
                    }
                    all_headers &= name.local.as_ref() == "th";
                    let mut text = String::new();
                    text_of(cell, &mut text);
                    row.push(text);
                }
            }
            if !row.is_empty() {
                if output.is_empty() {
                    *header = all_headers;
                }
                output.push(row);
            }
            return Some(());
        }
        for child in node.children.borrow().iter() {
            rows(child, output, header)?;
        }
        Some(())
    }
    let mut output = vec![];
    let mut header = false;
    rows(node, &mut output, &mut header)?;
    PinTable::new(output, header)
}

pub fn parse_html(bytes: &[u8]) -> Result<ParsedHtml, ReadError> {
    let fragment = html_fragment(bytes)?;
    let dom = parse_document(RcDom::default(), Default::default()).one(fragment);
    let mut pending = vec![(dom.document.clone(), 0)];
    let mut count = 0;
    while let Some((node, depth)) = pending.pop() {
        count += 1;
        if count > MAX_NODES || depth > 128 {
            return Err(ReadError::TooLarge);
        }
        for child in node.children.borrow().iter() {
            pending.push((child.clone(), depth + 1));
        }
    }
    let mut build = Build::default();
    build.walk(&dom.document, 0, false)?;
    let text = build.text.trim().to_string();
    if text.is_empty() {
        return Err(ReadError::InvalidDocument);
    }
    if text.chars().count() > crate::MAX_TEXT_CHARS {
        return Err(ReadError::TooLarge);
    }
    let table = if build.tables.len() == 1 && build.outside_table.trim().is_empty() {
        build.tables.pop()
    } else {
        None
    };
    Ok(ParsedHtml {
        safe: build.safe,
        text,
        table,
        rich: build.rich,
    })
}

pub fn http_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.chars().any(char::is_whitespace) {
        return None;
    }
    let url = url::Url::parse(text).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then(|| text.to_string())
}
pub fn is_absolute_path(text: &str) -> bool {
    let text = text.trim();
    let bytes = text.as_bytes();
    !text.contains(['\r', '\n', '\0'])
        && ((bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
            || (text.starts_with("\\\\")
                && text[2..].split('\\').filter(|s| !s.is_empty()).count() >= 2))
}

pub fn tsv(text: &str) -> Option<PinTable> {
    let rows: Vec<Vec<String>> = text
        .trim_end_matches(['\r', '\n'])
        .split('\n')
        .map(|line| {
            line.trim_end_matches('\r')
                .split('\t')
                .map(str::to_string)
                .collect()
        })
        .collect();
    if rows.len() < 2
        || rows.first()?.len() < 2
        || rows
            .iter()
            .all(|row| row.first().is_none_or(String::is_empty))
    {
        return None;
    }
    PinTable::new(rows, false)
}
