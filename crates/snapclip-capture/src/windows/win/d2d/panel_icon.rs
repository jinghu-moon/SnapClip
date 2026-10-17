//! The panel's glyphs on a Direct2D device: WIC-decode the baked PNGs, tint them, blit them.
//!
//! The picture comes from `assets/icon/*.png`, which the `icon-bake` tool rasterised from the SVGs
//! with resvg (docs/23 §14.54) — the overlay never parses SVG itself, because Direct2D's own SVG
//! renderer painted nothing on this machine and redrawing the shapes by hand was visibly different
//! from the design.
//!
//! ## Why the tint is pre-rendered instead of applied while drawing
//!
//! Tinting is a `CLSID_D2D1ColorMatrix` effect, and drawing an effect needs a target: that means
//! `BeginDraw`/`EndDraw`, which cannot nest inside the frame's own draw pass. So the tinted bitmaps
//! are produced once, in [`PanelIcons::prepare`], which the renderer calls from `recreate_resources`
//! — outside any frame. Drawing then costs one `DrawBitmap` per glyph, and the colours a glyph can
//! be drawn in come from the panel's own brushes, which exist by then.
//!
//! ## Why `DrawBitmap` and not a transform
//!
//! Placement and scaling are the destination rectangle's job, so nothing here depends on the render
//! target's transform — the one part of this renderer that has never been exercised. The only thing
//! a destination rectangle cannot do is the chevron's vertical flip, which the affine-transform
//! effect performs *inside* the offscreen render, where it needs no target state at all.

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_OVER,
    D2D1_PIXEL_FORMAT, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D12DAffineTransform, CLSID_D2D1ColorMatrix,
    D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_COLORMATRIX_PROP_COLOR_MATRIX,
    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_PROPERTY_TYPE_UNKNOWN, ID2D1Bitmap1,
    ID2D1DeviceContext, ID2D1Effect, ID2D1Image,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory,
    IWICPalette, WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, IStream,
};
use windows::core::Interface;

use super::helpers::to_d2d;
use crate::geometry::Rect;
use crate::icons::{Glyph, ICON_BAKED_PX};

/// A tinted bitmap's identity: the glyph, the colour it was tinted in, and whether it is flipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tinted {
    glyph: Glyph,
    colour: u32,
    flipped: bool,
}

/// The panel's glyph bitmaps, ready to blit.
pub(crate) struct PanelIcons {
    factory: Option<IWICImagingFactory>,
    /// The decoded white-on-transparent sources, one per glyph, decoded on first use.
    sources: Vec<(Glyph, ID2D1Bitmap1)>,
    /// The colour matrix, reused: its input and its value are set per tint render.
    tint: Option<ID2D1Effect>,
    /// The vertical flip for the open dropdown's chevron, likewise reused.
    flip: Option<ID2D1Effect>,
    /// What [`PanelIcons::prepare`] produced.
    tinted: Vec<(Tinted, ID2D1Bitmap1)>,
}

impl PanelIcons {
    pub(crate) fn new() -> Self {
        Self {
            factory: None,
            sources: Vec::new(),
            tint: None,
            flip: None,
            tinted: Vec::new(),
        }
    }

    /// Drop everything that belongs to the device; the next `prepare` rebuilds it.
    pub(crate) fn reset(&mut self) {
        self.sources.clear();
        self.tint = None;
        self.flip = None;
        self.tinted.clear();
    }

    /// Render every glyph in every colour the panel draws it in.
    ///
    /// Called from `recreate_resources`, i.e. outside a frame: the tint renders need their own
    /// `BeginDraw`/`EndDraw`, and D2D forbids nesting those inside the frame's draw pass.
    pub(crate) fn prepare(
        &mut self,
        context: &ID2D1DeviceContext,
        colours: &[D2D1_COLOR_F],
    ) -> Result<(), String> {
        if !self.tinted.is_empty() {
            // 幂等：调用方在每帧之前问一次，只有第一次真的做（`reset` 之后——比如设备重建——会重做）。
            return Ok(());
        }
        for colour in colours {
            for glyph in Glyph::ALL {
                // The chevron is the one glyph drawn both ways: the dropdown's arrow points up while
                // the menu is open.
                let flips: &[bool] = if glyph == Glyph::Chevron {
                    &[false, true]
                } else {
                    &[false]
                };
                for flipped in flips {
                    let bitmap = self.tint_one(context, glyph, *colour, *flipped)?;
                    self.tinted.push((
                        Tinted {
                            glyph,
                            colour: colour_key(*colour),
                            flipped: *flipped,
                        },
                        bitmap,
                    ));
                }
            }
        }
        Ok(())
    }

    /// Blit one glyph into `rect` in `colour`.
    ///
    /// A colour that was never prepared is a programming error rather than a drawing failure: the
    /// palette comes from the panel's own brushes, so name the pair that is missing instead of
    /// drawing nothing.
    pub(crate) fn draw(
        &mut self,
        context: &ID2D1DeviceContext,
        glyph: Glyph,
        rect: Rect,
        colour: D2D1_COLOR_F,
        flipped: bool,
    ) -> Result<(), String> {
        let wanted = Tinted {
            glyph,
            colour: colour_key(colour),
            flipped,
        };
        let bitmap = self
            .tinted
            .iter()
            .find(|(candidate, _)| *candidate == wanted)
            .map(|(_, bitmap)| bitmap)
            .ok_or_else(|| {
                format!(
                    "{} was not prepared for colour {colour:?} (flipped={flipped})",
                    glyph.name()
                )
            })?;
        let destination = to_d2d(rect);
        // The inherited `ID2D1RenderTarget::DrawBitmap` is the one windows-rs resolves here and it
        // returns nothing: D2D reports drawing errors at `EndDraw`.
        unsafe {
            context.DrawBitmap(
                bitmap,
                Some(&destination),
                1.0,
                D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                None,
                None,
            );
        }
        Ok(())
    }

    /// Render one glyph in one colour (optionally flipped) into a bitmap of the baked size.
    fn tint_one(
        &mut self,
        context: &ID2D1DeviceContext,
        glyph: Glyph,
        colour: D2D1_COLOR_F,
        flipped: bool,
    ) -> Result<ID2D1Bitmap1, String> {
        let source = self.source(context, glyph)?;
        let tint = self.tint_effect(context, &source, colour)?;
        // An effect is usable as an image, but windows-rs needs the cast spelled out.
        let tinted: ID2D1Image = tint
            .cast()
            .map_err(|error| format!("colour matrix as an image: {error}"))?;
        let image: ID2D1Image = if flipped {
            let flip = self.flip_effect(context)?;
            unsafe { flip.SetInput(0, &tinted, true) };
            flip.cast()
                .map_err(|error| format!("flip effect as an image: {error}"))?
        } else {
            tinted
        };

        let target = offscreen(context)?;
        // Render into the offscreen, then put the frame's target back whatever happens.
        let previous =
            unsafe { context.GetTarget() }.map_err(|error| format!("GetTarget: {error}"))?;
        unsafe {
            context.SetTarget(&target);
            context.BeginDraw();
            let clear = D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            };
            // `Clear` and `DrawImage` go through their inherited void forms; `EndDraw` is what
            // reports whether the offscreen actually drew.
            context.Clear(Some(&clear));
            context.DrawImage(
                &image,
                None,
                None,
                D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            let ended = context.EndDraw(None, None);
            context.SetTarget(&previous);
            ended.map_err(|error| format!("tinting {} (EndDraw): {error}", glyph.name()))?;
        }
        Ok(target)
    }

    /// The decoded source for one glyph, decoding it on first use.
    fn source(
        &mut self,
        context: &ID2D1DeviceContext,
        glyph: Glyph,
    ) -> Result<ID2D1Bitmap1, String> {
        if let Some((_, bitmap)) = self.sources.iter().find(|(cached, _)| *cached == glyph) {
            return Ok(bitmap.clone());
        }
        let bitmap = self.decode(context, glyph)?;
        self.sources.push((glyph, bitmap.clone()));
        Ok(bitmap)
    }

    /// WIC: PNG bytes → a Direct2D bitmap in the layout the colour matrix expects (premultiplied).
    fn decode(
        &mut self,
        context: &ID2D1DeviceContext,
        glyph: Glyph,
    ) -> Result<ID2D1Bitmap1, String> {
        let name = glyph.name();
        let factory = self.factory()?;
        unsafe {
            let stream = factory
                .CreateStream()
                .map_err(|error| format!("CreateStream({name}): {error}"))?;
            stream
                .InitializeFromMemory(glyph.png())
                .map_err(|error| format!("InitializeFromMemory({name}): {error}"))?;
            // `IWICStream` is an `IStream`, which is what the decoder takes.
            let stream: IStream = stream
                .cast()
                .map_err(|error| format!("IWICStream as IStream ({name}): {error}"))?;
            let decoder = factory
                .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
                .map_err(|error| format!("CreateDecoderFromStream({name}): {error}"))?;
            let frame: IWICBitmapSource = decoder
                .GetFrame(0)
                .and_then(|frame| frame.cast())
                .map_err(|error| format!("GetFrame({name}): {error}"))?;
            let converter = factory
                .CreateFormatConverter()
                .map_err(|error| format!("CreateFormatConverter({name}): {error}"))?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None::<&IWICPalette>,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(|error| format!("Initialize(converter, {name}): {error}"))?;
            context
                .CreateBitmapFromWicBitmap(&converter, None)
                .map_err(|error| format!("CreateBitmapFromWicBitmap({name}): {error}"))
        }
    }

    /// The WIC factory, created once per device.
    fn factory(&mut self) -> Result<IWICImagingFactory, String> {
        if let Some(factory) = self.factory.as_ref() {
            return Ok(factory.clone());
        }
        // WIC is a COM component, so the thread needs an apartment. MTA matches the rest of this
        // crate; a thread that is already initialised answers RPC_E_CHANGED_MODE, which is fine.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let factory: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
                .map_err(|error| format!("CoCreateInstance(WICImagingFactory): {error}"))?;
        self.factory = Some(factory.clone());
        Ok(factory)
    }

    /// The colour matrix, pointed at `source` and scaled to `colour`.
    fn tint_effect(
        &mut self,
        context: &ID2D1DeviceContext,
        source: &ID2D1Bitmap1,
        colour: D2D1_COLOR_F,
    ) -> Result<ID2D1Effect, String> {
        if self.tint.is_none() {
            self.tint = Some(
                unsafe { context.CreateEffect(&CLSID_D2D1ColorMatrix) }
                    .map_err(|error| format!("CreateEffect(ColorMatrix): {error}"))?,
            );
        }
        let tint = self.tint.as_ref().expect("just created");
        // The icons are white on transparent and the source bitmap is premultiplied, so "tint" is a
        // per-channel scale, with the colour's own alpha on the colour rows and on the alpha row:
        // out = (a·r·α, a·g·α, a·b·α, a·α) for an input pixel (a, a, a, a).
        let scale = |value: f32| value * colour.a;
        // `D2D1_MATRIX_5X4_F` is five rows of four: four channel mixes and a bias row.
        let matrix: [f32; 20] = [
            scale(colour.r), 0.0, 0.0, 0.0, //
            0.0, scale(colour.g), 0.0, 0.0, //
            0.0, 0.0, scale(colour.b), 0.0, //
            0.0, 0.0, 0.0, colour.a, //
            0.0, 0.0, 0.0, 0.0,
        ];
        unsafe {
            tint.SetInput(0, source, true);
            tint.SetValue(
                D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                as_bytes(&matrix),
            )
            .map_err(|error| format!("SetValue(ColorMatrix): {error}"))?;
        }
        Ok(tint.clone())
    }

    /// The vertical flip, used for the open dropdown's chevron.
    fn flip_effect(&mut self, context: &ID2D1DeviceContext) -> Result<ID2D1Effect, String> {
        if self.flip.is_none() {
            let flip = unsafe { context.CreateEffect(&CLSID_D2D12DAffineTransform) }
                .map_err(|error| format!("CreateEffect(AffineTransform): {error}"))?;
            // Mirror inside the baked square: `translate(0, size) · scale(1, -1)`.
            let matrix = windows_numerics::Matrix3x2::translation(0.0, ICON_BAKED_PX as f32)
                * windows_numerics::Matrix3x2::scale(1.0, -1.0);
            unsafe {
                flip.SetValue(
                    D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                    D2D1_PROPERTY_TYPE_UNKNOWN,
                    as_bytes(&matrix),
                )
                .map_err(|error| format!("SetValue(TransformMatrix): {error}"))?;
            }
            self.flip = Some(flip);
        }
        Ok(self.flip.as_ref().expect("just created").clone())
    }
}

/// A premultiplied BGRA render target of the baked size, for one tint render.
fn offscreen(context: &ID2D1DeviceContext) -> Result<ID2D1Bitmap1, String> {
    let properties = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
        ..Default::default()
    };
    unsafe {
        context
            .CreateBitmap(
                D2D_SIZE_U {
                    width: ICON_BAKED_PX,
                    height: ICON_BAKED_PX,
                },
                None,
                0,
                &properties,
            )
            .map_err(|error| format!("CreateBitmap(offscreen): {error}"))
    }
}

/// The tint cache key for a colour: the four channels' bits.
fn colour_key(colour: D2D1_COLOR_F) -> u32 {
    let mut key = 0u32;
    for value in [colour.r, colour.g, colour.b, colour.a] {
        key = key.rotate_left(8) ^ value.to_bits();
    }
    key
}

/// A value's bytes, which is how `ID2D1Properties::SetValue` takes them in windows-rs.
fn as_bytes<T: Copy>(value: &T) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
    }
}
