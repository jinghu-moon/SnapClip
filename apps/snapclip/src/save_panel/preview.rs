//! App-owned image preparation and export revisions; Yue owns the viewport.
use super::{Export, Picture, SavePanel};
use crate::save::assemble;
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
    let (pixels, source) = picture.pixels();
    let expected = (source.width as usize)
        .checked_mul(source.height as usize)
        .and_then(|n| n.checked_mul(4));
    if source.width == 0 || source.height == 0 || expected != Some(pixels.len()) {
        return Err("截图像素长度不正确".into());
    }
    // Borrow the capture. A proxy allocates only its bounded output buffer;
    // native-size preparation copies once, without decode or channel conversion.
    ImagePreviewImage::from_bgra8(source.width, source.height, pixels, "原始截图")
        .map_err(|e| e.to_string())
}

pub(super) fn encode(export: &Export) -> Result<EncodedPreview, String> {
    super::model::validate_preview_size(export.output).map_err(str::to_owned)?;
    let encoded = assemble::encoded_bytes(&export.values(), &export.picture.frame(export.output))
        .map_err(|e| e.message())?;
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
                    .result_label("编码后"),
            )
    }
}
