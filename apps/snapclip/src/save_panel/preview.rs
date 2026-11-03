//! App-owned image preparation and export revisions; Yue owns the viewport.
use super::{Export, Picture, SavePanel};
use gpui_kit::*;

use yue_component::{
    ImagePreviewDocument, ImagePreviewImage, ImagePreviewLimits, ImagePreviewLoad, YueImagePreview,
    YueImagePreviewState,
};

/// Application-owned load results; the retained Yue state owns the camera and gestures.
pub(super) struct Preview {
    pub original: ImagePreviewLoad,
    pub encoded: ImagePreviewLoad,
    pub state: Entity<YueImagePreviewState>,
}
impl Preview {
    pub fn new(window: &mut Window, cx: &mut Context<SavePanel>) -> Self {
        Self {
            original: ImagePreviewLoad::Loading,
            encoded: ImagePreviewLoad::Empty,
            state: cx.new(|cx| YueImagePreviewState::new("save-image-preview", window, cx)),
        }
    }
}

pub(super) struct EncodedPreview {
    pub bytes: u64,
    pub encoded: ImagePreviewImage,
}

/// The capture is already decoded BGRA. Its first display must not depend on
/// export compression, format support, debounce, or the current output draft.
pub(super) fn original(picture: &Picture) -> Result<ImagePreviewImage, String> {
    let source = picture.original_pixels();
    let [width, height] = source.size();
    let descriptor = source.descriptor();
    if descriptor.format() == snapclip_model::PixelFormat::Bgra8Unorm {
        return ImagePreviewImage::from_bgra8(
            width,
            height,
            picture.sdr_pixels().as_bytes(),
            "原始截图",
        )
        .map_err(|e| e.to_string());
    }
    let bytes = if descriptor.alpha() == snapclip_model::pixels::AlphaMode::Premultiplied {
        std::borrow::Cow::Owned(
            snapclip_model::pixel_color::linear_rgba(source)
                .flatten()
                .flat_map(|v| half::f16::from_f64(v).to_le_bytes())
                .collect::<Vec<u8>>(),
        )
    } else {
        std::borrow::Cow::Borrowed(source.as_bytes())
    };
    let encoding = if picture.is_hdr() {
        yue_component::ImagePreviewColorSpace::ScRgb
    } else {
        yue_component::ImagePreviewColorSpace::LinearSrgb
    };
    yue_component::ImagePreviewPixels::from_rgba16f(
        width,
        height,
        source.row_stride(),
        bytes,
        encoding,
    )
    .and_then(|pixels| pixels.into_preview(&ImagePreviewLimits::default(), "原始截图"))
    .map_err(|e| e.to_string())
}

pub(super) fn encode(export: &Export) -> Result<EncodedPreview, String> {
    super::model::validate_preview_size(export.output).map_err(str::to_owned)?;
    let encoded = export.encoded_bytes().map_err(|e| e.message())?;
    // Native detail within the same budget as the source; oversized output is
    // explicitly a proxy. Byte counts still measure the exact export file.
    let encoded_image =
        ImagePreviewImage::from_encoded(&encoded, &ImagePreviewLimits::default(), "编码后的图片")
            .map_err(|e| format!("无法解码预览：{e}"))?;
    Ok(EncodedPreview {
        bytes: encoded.len() as u64,
        encoded: encoded_image,
    })
}

impl SavePanel {
    pub(super) fn publish_preview(&self, window: &mut Window, cx: &mut Context<Self>) {
        let document = ImagePreviewDocument::new("save-capture", self.revision)
            .with_source(self.preview.original.clone())
            .with_result(self.preview.encoded.clone())
            .full_frame(true);
        self.preview.state.update(cx, |state, cx| {
            state
                .set_document(document, window, cx)
                .expect("monotonic save preview revision");
        });
    }

    pub(super) fn viewport(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = yue_component::current_theme(cx);
        div()
            .id("save-preview")
            .test_support()
            .flex_1()
            .w_full()
            .min_h(rems(12.))
            .min_w_0()
            .child(
                YueImagePreview::new(&self.preview.state)
                    .theme(theme)
                    .result_label(if self.picture.is_hdr() && !self.preserve_hdr {
                        "SDR 结果"
                    } else {
                        "编码后"
                    }),
            )
    }
}
