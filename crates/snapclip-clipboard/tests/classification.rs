use snapclip_clipboard::{ImageEncoding, MAX_TEXT_CHARS, ReadError, Snapshot, document, prepare};
use snapclip_model::pin::{DocumentMode, PinContent};

#[test]
fn injected_decoder_prefers_png_and_reports_explicit_sdr_fallback() {
    use snapclip_clipboard::prepare_with_image_decoder;
    use snapclip_model::pin::PinImage;
    let snapshot = Snapshot::new(1)
        .with_image(ImageEncoding::Dib, vec![1])
        .with_image(ImageEncoding::Png, vec![2]);
    let mut calls = Vec::new();
    let prepared = prepare_with_image_decoder(snapshot, |_, encoding| {
        calls.push(encoding);
        if encoding == ImageEncoding::Png {
            return Err(ReadError::InvalidImage);
        }
        Ok(PinImage::new(1, 1, vec![20, 40, 60, 128]).unwrap())
    })
    .unwrap();
    assert_eq!(calls, [ImageEncoding::Png, ImageEncoding::Dib]);
    let pin = prepared.into_pin();
    assert!(pin.notice().unwrap().contains("SDR"));
    assert!(matches!(pin.content(), PinContent::Image(image) if !image.is_hdr()));

    let prepared = prepare_with_image_decoder(
        Snapshot::new(2)
            .with_image(ImageEncoding::Png, vec![1])
            .with_files(vec!["C:\\image.png".into()]),
        |_, _| panic!("file references must not invoke the image decoder"),
    )
    .unwrap();
    assert!(matches!(prepared.content(), PinContent::Files(_)));
}

fn mode(snapshot: Snapshot) -> DocumentMode {
    match prepare(snapshot).unwrap().content() {
        PinContent::Document(doc) => doc.mode(),
        other => panic!("expected document, got {other:?}"),
    }
}
fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([20, 40, 60, 128]));
    let mut out = std::io::Cursor::new(vec![]);
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
fn files_win_over_all_other_representations() {
    let snapshot = Snapshot::new(1)
        .with_text("file")
        .with_html(b"<b>text</b>".to_vec())
        .with_image(ImageEncoding::Png, png(1, 1))
        .with_files(vec![r"C:\missing\photo.png".into()]);
    assert!(
        matches!(prepare(snapshot).unwrap().content(),PinContent::Files(files)if files.len()==1)
    );
}
#[test]
fn empty_unicode_flavor_does_not_erase_rich_document_text() {
    let p = prepare(
        Snapshot::new(1)
            .with_text("")
            .with_html("<b>中文</b>".as_bytes().to_vec()),
    )
    .unwrap();
    assert!(matches!(p.content(), PinContent::Document(d) if d.text() == "中文"));
}
#[test]
fn spanning_or_nested_tables_never_become_a_partial_rectangular_projection() {
    for html in [
        "<table><tr><td colspan='2'>wide</td></tr><tr><td>A</td><td>B</td></tr></table>",
        "<table><tr><td><table><tr><td>inner</td></tr></table></td></tr></table>",
    ] {
        assert_eq!(
            mode(Snapshot::new(1).with_html(html.as_bytes().to_vec())),
            DocumentMode::Rich
        );
    }
}
#[test]
fn virtual_attachments_are_not_presented_as_their_filename_text() {
    assert!(matches!(
        prepare(
            Snapshot::new(1)
                .with_virtual_files(true)
                .with_text("attachment.pdf")
        ),
        Err(ReadError::VirtualFiles)
    ));
}
#[test]
fn word_paragraphs_win_over_bitmap_and_preserve_text() {
    let text = "中文 😀\nSecond paragraph";
    let s = Snapshot::new(1)
        .with_image(ImageEncoding::Png, png(1, 1))
        .with_text(text)
        .with_html(b"<p>Chinese</p><p><b>Second paragraph</b></p>".to_vec());
    let result = prepare(s).unwrap();
    assert!(
        matches!(result.content(),PinContent::Document(d)if d.mode()==DocumentMode::Rich&&d.text()==text)
    );
}
#[test]
fn unstyled_html_body_is_a_document_not_a_bitmap() {
    assert_eq!(
        mode(
            Snapshot::new(1)
                .with_text("A paragraph")
                .with_html(b"<p>A paragraph</p>".to_vec())
                .with_image(ImageEncoding::Png, png(1, 1))
        ),
        DocumentMode::Rich
    );
}
#[test]
fn excel_table_wins_over_bitmap_and_keeps_header_semantics() {
    let html = b"<table><tr><th>Name</th><th>Value</th></tr><tr><td>A</td><td></td></tr></table>";
    let p = prepare(
        Snapshot::new(1)
            .with_html(html.to_vec())
            .with_text("Name\tValue\nA\t")
            .with_image(ImageEncoding::Png, png(1, 1)),
    )
    .unwrap();
    assert!(
        matches!(p.content(),PinContent::Document(d)if d.mode()==DocumentMode::Table&&d.table().unwrap().has_header()&&d.table().unwrap().rows()[1][1].is_empty())
    );
}
#[test]
fn mixed_document_is_not_reduced_to_its_only_table() {
    let s = Snapshot::new(1)
        .with_html(b"<p>Intro</p><table><tr><td>A</td></tr></table><p>Outro</p>".to_vec());
    assert_eq!(mode(s), DocumentMode::Rich);
}
#[test]
fn path_text_never_becomes_a_file_reference() {
    assert_eq!(
        mode(Snapshot::new(1).with_text(r"D:\work\file.txt")),
        DocumentMode::Path
    );
    assert_eq!(
        mode(Snapshot::new(1).with_text(r"\\server\share\folder")),
        DocumentMode::Path
    );
}
#[test]
fn full_url_detection_is_conservative_and_keeps_original() {
    for text in [" https://example.com/a?q=1 \n", "https://example.com/中文"] {
        let p = prepare(Snapshot::new(1).with_text(text)).unwrap();
        assert!(
            matches!(p.content(),PinContent::Document(d)if d.mode()==DocumentMode::Link&&d.text()==text)
        );
    }
    assert_eq!(
        mode(Snapshot::new(1).with_text("see https://example.com")),
        DocumentMode::Text
    );
    assert!(document::http_url("javascript:alert(1)").is_none());
    assert!(document::http_url("https://").is_none());
}
#[test]
fn simple_link_html_does_not_hide_link_mode() {
    assert_eq!(
        mode(
            Snapshot::new(1)
                .with_text("https://example.com")
                .with_html(b"<a href=\"https://example.com\">https://example.com</a>".to_vec())
        ),
        DocumentMode::Link
    );
}
#[test]
fn image_wrapper_and_alt_are_not_a_document() {
    let p = prepare(
        Snapshot::new(1)
            .with_text("https://example.com/image.png")
            .with_html(
                b"<div><img src=\"https://remote/p.png\" alt=\"not body text\"></div>".to_vec(),
            )
            .with_image(ImageEncoding::Png, png(1, 1)),
    )
    .unwrap();
    assert!(matches!(p.content(), PinContent::Image(_)));
}
#[test]
fn original_image_resolution_and_transparency_survive() {
    let p = prepare(Snapshot::new(1).with_image(ImageEncoding::Png, png(2400, 2))).unwrap();
    assert!(
        matches!(p.content(),PinContent::Image(i)if i.width()==2400&&i.height()==2&&i.sdr_rgba().unwrap()[3]==128)
    );
}
#[test]
fn corrupt_preferred_image_can_use_valid_alternative() {
    let p = prepare(
        Snapshot::new(1)
            .with_image(ImageEncoding::Png, vec![0; 20])
            .with_image(ImageEncoding::Png, png(2, 2)),
    )
    .unwrap();
    assert!(matches!(p.content(),PinContent::Image(i)if i.width()==2));
}
#[test]
fn malformed_rich_content_falls_back_without_losing_text() {
    let p = prepare(
        Snapshot::new(1)
            .with_text(" \tChinese 中文\n")
            .with_html(b"Version:1\nStartFragment:-1\nEndFragment:99999\n".to_vec()),
    )
    .unwrap();
    assert!(
        matches!(p.content(),PinContent::Document(d)if d.notice().is_some()&&d.text()==" \tChinese 中文\n")
    );
}
#[test]
fn nonempty_whitespace_is_content_but_empty_is_not() {
    assert_eq!(
        mode(Snapshot::new(1).with_text(" \t\r\n")),
        DocumentMode::Text
    );
    assert!(matches!(
        prepare(Snapshot::new(1).with_text("")),
        Err(ReadError::Empty)
    ));
}
#[test]
fn only_explicit_code_structures_default_to_code() {
    for text in ["{\"x\":1}", "[1,2]", "```rust\nlet x=1;\n```"] {
        assert_eq!(mode(Snapshot::new(1).with_text(text)), DocumentMode::Code);
    }
    assert_eq!(
        mode(Snapshot::new(1).with_text("let x = 1;")),
        DocumentMode::Text
    );
}
#[test]
fn tsv_is_rectangular_and_does_not_invent_headers() {
    let p = prepare(Snapshot::new(1).with_text("\tvalue\nname\t")).unwrap();
    assert!(
        matches!(p.content(),PinContent::Document(d)if d.mode()==DocumentMode::Table&&!d.table().unwrap().has_header()&&d.table().unwrap().rows()[0][0].is_empty())
    );
    assert!(document::tsv("a\tb\nc").is_none());
    assert!(document::tsv("\tindent\n\tmore indent").is_none());
    assert!(document::tsv("a,b\nc,d").is_none());
}
#[test]
fn sanitizer_cannot_load_or_execute_external_content() {
    let html=document::parse_html(br#"<p onclick="bad()"><strong>Hello</strong><img src="https://remote/image"><script>alert(1)</script><a href="javascript:bad()">bad</a><a href="https://example.com">good</a></p>"#).unwrap();
    let _ = html; /* safe source is inspected through the public prepared content seam */
    let p=prepare(Snapshot::new(1).with_html(br#"<b>Hello</b><img src="https://remote"><script>bad()</script><a href="javascript:bad()">link</a>"#.to_vec())).unwrap();
    if let PinContent::Document(d) = p.content() {
        let html = d.html().unwrap();
        assert!(!html.contains("src="));
        assert!(!html.contains("script"));
        assert!(!html.contains("javascript:"));
        assert!(!html.contains("bad()"));
    } else {
        panic!("document expected");
    }
}
#[test]
fn cf_html_uses_valid_utf8_byte_offsets() {
    let fragment = "<b>中文😀</b>";
    let header = format!(
        "Version:1.0\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n",
        0, 0
    );
    let envelope = format!(
        "Version:1.0\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n{fragment}",
        header.len(),
        header.len() + fragment.len()
    );
    assert_eq!(
        document::html_fragment(envelope.as_bytes()).unwrap(),
        fragment
    );
    for bad in [
        b"Version:1\nStartFragment:-1\nEndFragment:2".as_slice(),
        b"Version:1\nStartFragment:9999\nEndFragment:99999",
    ] {
        assert_eq!(
            document::html_fragment(bad),
            Err(ReadError::InvalidDocument)
        );
    }
}
#[test]
fn text_and_html_depth_budgets_reject_instead_of_truncating() {
    assert!(matches!(
        prepare(Snapshot::new(1).with_text("字".repeat(MAX_TEXT_CHARS + 1))),
        Err(ReadError::TooLarge)
    ));
    let nested = format!("{}text{}", "<div>".repeat(150), "</div>".repeat(150));
    assert!(matches!(
        document::parse_html(nested.as_bytes()),
        Err(ReadError::TooLarge)
    ));
}

#[test]
fn table_budgets_preserve_the_original_text_fallback() {
    let wide = std::iter::repeat_n("x", 257).collect::<Vec<_>>().join("\t");
    let text = format!("{wide}\n{wide}");
    let pin = prepare(Snapshot::new(1).with_text(&text)).unwrap();
    assert!(
        matches!(pin.content(),PinContent::Document(doc) if doc.mode()==DocumentMode::Text && doc.text()==text && doc.table().is_none())
    );
    let html = format!("<table><tr>{}</tr></table>", "<td>x</td>".repeat(257));
    assert!(matches!(
        document::parse_html(html.as_bytes()),
        Err(ReadError::TooLarge)
    ));
    let html = format!(
        "<table>{}</table>",
        (0..65)
            .map(|_| format!("<tr>{}</tr>", "<td></td>".repeat(256)))
            .collect::<String>()
    );
    assert!(matches!(
        document::parse_html(html.as_bytes()),
        Err(ReadError::TooLarge)
    ));
}
#[test]
fn raw_rtf_bytes_are_retained_for_copy() {
    let bytes = br"{\rtf1\ansi\ansicpg936 \'c4\'e3\'ba\'c3}".to_vec();
    let p = prepare(Snapshot::new(1).with_rtf(bytes.clone()))
        .unwrap()
        .into_pin();
    assert_eq!(&*p.sources()[0].1, bytes.as_slice());
    assert!(matches!(p.content(),PinContent::Document(d)if d.text()=="你好"));
}
#[test]
fn missing_files_remain_visible_and_unc_is_not_probed() {
    let files = snapclip_clipboard::prepare_files(&[
        r"C:\snapclip-nonexistent-clip.txt".into(),
        r"\\offline-server\share\file".into(),
    ])
    .unwrap();
    assert!(files[0].unavailable_reason().is_some());
    assert_eq!(files[1].unavailable_reason(), Some("网络位置尚未访问"));
}
