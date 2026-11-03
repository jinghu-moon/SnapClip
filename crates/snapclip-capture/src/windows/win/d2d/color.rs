//! One color interpretation for a native overlay target. This is not source
//! detection: an HDR caller must supply a verified output SDR-white snapshot.
use super::*;
use snapclip_model::{
    PixelFormat,
    pixels::{AlphaMode, PixelBuffer, PixelDescriptor, PixelEncoding},
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BUFFER_PRECISION_8BPC_UNORM, D2D1_BUFFER_PRECISION_16BPC_FLOAT, D2D1_COLOR_SPACE_SCRGB,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OverlayColor {
    /// None is the unchanged encoded-sRGB SDR path. Some is scRGB UI white.
    sdr_white_nits: Option<f32>,
}
/// Own the COM reference held in the C ABI properties. windows-rs deliberately
/// does not give D2D1_BITMAP_PROPERTIES1 a destructor.
pub(super) struct BitmapProperties(D2D1_BITMAP_PROPERTIES1);
impl BitmapProperties {
    pub(super) fn raw(&self) -> &D2D1_BITMAP_PROPERTIES1 {
        &self.0
    }
}
impl Drop for BitmapProperties {
    fn drop(&mut self) {
        unsafe { std::mem::ManuallyDrop::drop(&mut self.0.colorContext) };
    }
}
impl OverlayColor {
    pub(super) fn for_source(
        descriptor: PixelDescriptor,
        sdr_white_nits: Option<f32>,
    ) -> Result<Self, String> {
        if descriptor == PixelDescriptor::bgra8() {
            return Ok(Self::sdr());
        }
        let hdr = PixelDescriptor::new(
            PixelFormat::Rgba16Float,
            PixelEncoding::ScRgb,
            AlphaMode::Straight,
        )
        .expect("fixed overlay scRGB contract");
        if descriptor != hdr {
            return Err("unsupported native overlay source color contract".into());
        }
        Self::sc_rgb(
            sdr_white_nits.ok_or("HDR overlay requires a verified output SDR-white snapshot")?,
        )
    }
    pub(crate) const fn sdr() -> Self {
        Self {
            sdr_white_nits: None,
        }
    }
    pub(crate) fn sc_rgb(sdr_white_nits: f32) -> Result<Self, String> {
        if !sdr_white_nits.is_finite() || sdr_white_nits <= 0. || sdr_white_nits > 10_000. {
            return Err("overlay SDR white must be finite and in (0,10000] nit".into());
        }
        Ok(Self {
            sdr_white_nits: Some(sdr_white_nits),
        })
    }
    pub(crate) fn is_linear(self) -> bool {
        self.sdr_white_nits.is_some()
    }
    pub(crate) fn format(self) -> PixelFormat {
        if self.is_linear() {
            PixelFormat::Rgba16Float
        } else {
            PixelFormat::Bgra8Unorm
        }
    }
    pub(crate) fn descriptor(self) -> PixelDescriptor {
        if self.is_linear() {
            PixelDescriptor::new(
                PixelFormat::Rgba16Float,
                PixelEncoding::ScRgb,
                AlphaMode::Straight,
            )
            .expect("the native overlay has a fixed scRGB pixel contract")
        } else {
            PixelDescriptor::bgra8()
        }
    }
    /// Only authored SDR UI colors are scaled. Captured scRGB is already absolute.
    pub(crate) fn ui(self, value: D2D1_COLOR_F) -> D2D1_COLOR_F {
        let Some(white) = self.sdr_white_nits else {
            return value;
        };
        D2D1_COLOR_F {
            r: linear(value.r) * white / 80.,
            g: linear(value.g) * white / 80.,
            b: linear(value.b) * white / 80.,
            a: value.a,
        }
    }
    pub(super) fn properties(
        self,
        context: &ID2D1DeviceContext,
        options: windows::Win32::Graphics::Direct2D::D2D1_BITMAP_OPTIONS,
        alpha: windows::Win32::Graphics::Direct2D::Common::D2D1_ALPHA_MODE,
    ) -> Result<BitmapProperties, String> {
        let mut properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: super::super::d3d11::dxgi_format(self.format()),
                alphaMode: alpha,
            },
            dpiX: 96.,
            dpiY: 96.,
            bitmapOptions: options,
            ..Default::default()
        };
        if self.is_linear() {
            properties.colorContext = std::mem::ManuallyDrop::new(Some(
                unsafe { context.CreateColorContext(D2D1_COLOR_SPACE_SCRGB, None) }
                    .map_err(|e| super::super::hresult("CreateColorContext(scRGB)", &e))?,
            ));
        }
        Ok(BitmapProperties(properties))
    }
    pub(crate) fn apply_precision(self, context: &ID2D1DeviceContext) {
        unsafe {
            let mut controls = context.GetRenderingControls();
            controls.bufferPrecision = if self.is_linear() {
                D2D1_BUFFER_PRECISION_16BPC_FLOAT
            } else {
                D2D1_BUFFER_PRECISION_8BPC_UNORM
            };
            context.SetRenderingControls(&controls);
        }
    }
    /// Small SDR thumbnails become explicitly relative UI content in the absolute
    /// overlay. Never use this path to reconstruct the captured HDR source.
    pub(crate) fn thumbnail(self, pixels: &PixelBuffer) -> Result<PixelBuffer, String> {
        if pixels.descriptor() != PixelDescriptor::bgra8() {
            return Err("overlay thumbnail requires straight SDR BGRA8".into());
        }
        if !self.is_linear() {
            return Ok(pixels.clone());
        }
        let [width, height] = pixels.size();
        let mut bytes = Vec::with_capacity(width as usize * height as usize * 8);
        for p in pixels.as_bytes().as_chunks::<4>().0 {
            let converted = self.ui(super::color(
                p[2] as f32 / 255.,
                p[1] as f32 / 255.,
                p[0] as f32 / 255.,
                p[3] as f32 / 255.,
            ));
            for channel in [converted.r, converted.g, converted.b, converted.a] {
                bytes.extend_from_slice(&half::f16::from_f32(channel).to_le_bytes());
            }
        }
        PixelBuffer::from_tight(width, height, self.descriptor(), bytes).map_err(str::to_string)
    }
}
fn linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sdr_thumbnail_is_explicit_ui_content_not_a_reinterpreted_hdr_source() {
        let source = PixelBuffer::from_tight(
            2,
            1,
            PixelDescriptor::bgra8(),
            vec![0, 128, 255, 128, 255, 0, 0, 255],
        )
        .unwrap();
        let mode = OverlayColor::sc_rgb(160.).unwrap();
        let pixels = mode.thumbnail(&source).unwrap();
        assert_eq!(pixels.descriptor(), mode.descriptor());
        let channels: Vec<f32> = pixels
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| half::f16::from_le_bytes(*b).to_f32())
            .collect();
        for (actual, expected) in
            channels
                .iter()
                .zip([2., 0.431721, 0., 128. / 255., 0., 0., 2., 1.])
        {
            assert!((actual - expected).abs() < 0.001);
        }
        assert_eq!(
            OverlayColor::sdr().thumbnail(&source).unwrap().as_bytes(),
            source.as_bytes()
        );
        assert!(
            mode.thumbnail(&pixels).is_err(),
            "an HDR source is not an SDR UI thumbnail"
        );
    }
    #[test]
    fn overlay_white_scales_only_authored_ui_and_validates_the_snapshot() {
        let input = super::super::color(0.5, 1., 0.04045, 0.25);
        let legacy = OverlayColor::sdr().ui(input);
        assert_eq!(
            [legacy.r, legacy.g, legacy.b, legacy.a],
            [0.5, 1., 0.04045, 0.25]
        );
        let hdr = OverlayColor::sc_rgb(160.).unwrap().ui(input);
        assert!((hdr.r - 0.4280823).abs() < 0.000001);
        assert_eq!(hdr.g, 2.);
        assert!((hdr.b - 0.00626161).abs() < 0.000001);
        assert_eq!(hdr.a, 0.25);
        for white in [f32::NAN, f32::INFINITY, 0., -80., 10_001.] {
            assert!(OverlayColor::sc_rgb(white).is_err());
        }
        let hdr = OverlayColor::sc_rgb(160.).unwrap().descriptor();
        assert!(OverlayColor::for_source(hdr, None).is_err());
        assert_eq!(
            OverlayColor::for_source(hdr, Some(160.)).unwrap(),
            OverlayColor::sc_rgb(160.).unwrap()
        );
        assert_eq!(
            OverlayColor::for_source(PixelDescriptor::bgra8(), None).unwrap(),
            OverlayColor::sdr()
        );
        let relative = PixelDescriptor::new(
            PixelFormat::Rgba16Float,
            PixelEncoding::LinearSrgb,
            AlphaMode::Straight,
        )
        .unwrap();
        assert!(OverlayColor::for_source(relative, Some(160.)).is_err());
    }
}
