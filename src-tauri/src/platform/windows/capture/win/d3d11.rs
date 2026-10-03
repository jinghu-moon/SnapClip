//! D3D11 device, DirectComposition swap chain and texture helpers.
//!
//! The overlay owns exactly one device for the lifetime of the process (unless the
//! device is removed) and reuses it across capture sessions. Only the swap chain is
//! recreated when the overlay moves to a monitor with different dimensions.

use ::windows::Win32::Foundation::HWND;
use ::windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use ::windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1CreateDevice, ID2D1Bitmap1,
    ID2D1Device, ID2D1DeviceContext,
};
use ::windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use ::windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use ::windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use ::windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};use ::windows::Win32::Graphics::Dxgi::{
    DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
};
use ::windows::core::{Interface, Result as WinResult};

/// A BGRA frame together with the texture that holds it.
#[derive(Debug)]
pub struct GpuFrame {
    pub texture: ID3D11Texture2D,
    /// Texture dimensions, kept so callers can cross-check the monitor geometry.
    #[cfg_attr(not(test), allow(dead_code))]
    pub width: u32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub height: u32,
}

/// One D3D11 device plus the D2D device derived from it.
///
/// The device is intentionally not tied to a window: the same device backs the WGC
/// frame pool, the composition swap chain and the D2D device context.
#[derive(Clone)]
pub struct GraphicsDevice {
    d3d: ID3D11Device,
    context: ID3D11DeviceContext,
    d2d: ID2D1Device,
    dxgi_factory: IDXGIFactory2,
}

impl GraphicsDevice {
    pub fn create() -> Result<Self, String> {
        let mut d3d: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                Some(&mut context),
            )
        }
        .map_err(|error| super::hresult("D3D11CreateDevice", &error))?;
        let d3d = d3d.ok_or_else(|| "D3D11CreateDevice returned no device".to_string())?;
        let context = context.ok_or_else(|| "D3D11CreateDevice returned no context".to_string())?;

        let dxgi_device: IDXGIDevice = d3d
            .cast()
            .map_err(|error| super::hresult("IDXGIDevice::cast", &error))?;
        let adapter = unsafe { dxgi_device.GetAdapter() }
            .map_err(|error| super::hresult("IDXGIDevice::GetAdapter", &error))?;
        let dxgi_factory: IDXGIFactory2 = unsafe { adapter.GetParent() }
            .map_err(|error| super::hresult("IDXGIAdapter::GetParent", &error))?;
        let d2d = unsafe { D2D1CreateDevice(&dxgi_device, None) }
            .map_err(|error| super::hresult("D2D1CreateDevice", &error))?;

        Ok(Self {
            d3d,
            context,
            d2d,
            dxgi_factory,
        })
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.d3d
    }


    /// A fresh D2D device context. The overlay keeps one per session.
    pub fn create_d2d_context(&self) -> Result<ID2D1DeviceContext, String> {
        unsafe { self.d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) }
            .map_err(|error| super::hresult("ID2D1Device::CreateDeviceContext", &error))
    }

    /// Create a flip-model composition swap chain sized to the monitor.
    pub fn create_composition_swap_chain(
        &self,
        width: u32,
        height: u32,
    ) -> Result<IDXGISwapChain1, String> {
        let description = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        unsafe {
            self.dxgi_factory
                .CreateSwapChainForComposition(&self.d3d, &description, None)
        }
        .map_err(|error| super::hresult("CreateSwapChainForComposition", &error))
    }

    /// Wrap a swap chain back buffer as a D2D render target.
    pub fn create_target_bitmap(
        &self,
        d2d_context: &ID2D1DeviceContext,
        swap_chain: &IDXGISwapChain1,
    ) -> Result<ID2D1Bitmap1, String> {
        let surface: IDXGISurface = unsafe { swap_chain.GetBuffer(0) }
            .map_err(|error| super::hresult("IDXGISwapChain::GetBuffer", &error))?;
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            ..Default::default()
        };
        unsafe { d2d_context.CreateBitmapFromDxgiSurface(&surface, Some(&properties)) }
            .map_err(|error| super::hresult("CreateBitmapFromDxgiSurface", &error))
    }

    /// Create a shader-resource BGRA texture and upload `bgra` into it.
    pub fn create_bgra_texture(
        &self,
        width: u32,
        height: u32,
        bgra: &[u8],
    ) -> Result<GpuFrame, String> {
        let expected = width as usize * height as usize * 4;
        if bgra.len() != expected {
            return Err(format!(
                "bgra upload of {} bytes does not match {width}x{height}",
                bgra.len()
            ));
        }
        let description = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: bgra.as_ptr() as *const _,
            SysMemPitch: width * 4,
            SysMemSlicePitch: 0,
        };
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            self.d3d
                .CreateTexture2D(&description, Some(&initial), Some(&mut texture))
        }
        .map_err(|error| super::hresult("CreateTexture2D", &error))?;
        Ok(GpuFrame {
            texture: texture.ok_or_else(|| "CreateTexture2D returned no texture".to_string())?,
            width,
            height,
        })
    }

    /// Create a texture that Direct2D can use as a render target.
    ///
    /// Used by the offscreen composition harness: the real overlay renders into a
    /// composition swap chain, which D2D already accepts as a target.
    #[cfg(test)]
    pub fn create_render_target_texture(
        &self,
        width: u32,
        height: u32,
    ) -> Result<GpuFrame, String> {
        let description = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (::windows::Win32::Graphics::Direct3D11::D3D11_BIND_RENDER_TARGET.0
                | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe { self.d3d.CreateTexture2D(&description, None, Some(&mut texture)) }
            .map_err(|error| super::hresult("CreateTexture2D(render target)", &error))?;
        Ok(GpuFrame {
            texture: texture.ok_or_else(|| "CreateTexture2D returned no texture".to_string())?,
            width,
            height,
        })
    }

    /// Copy `texture` into a CPU-readable staging texture and return its pixels.
    ///
    /// This is the only GPU → CPU transfer on the capture path and it happens once,
    /// when a session is armed.
    pub fn read_back_bgra(&self, texture: &ID3D11Texture2D) -> Result<Vec<u8>, String> {
        let mut description = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut description) };
        let staging = D3D11_TEXTURE2D_DESC {
            Width: description.Width,
            Height: description.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: description.Format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging_texture: Option<ID3D11Texture2D> = None;
        unsafe { self.d3d.CreateTexture2D(&staging, None, Some(&mut staging_texture)) }
            .map_err(|error| super::hresult("CreateTexture2D(staging)", &error))?;
        let staging_texture = staging_texture
            .ok_or_else(|| "staging CreateTexture2D returned nothing".to_string())?;

        unsafe {
            self.context.CopyResource(&staging_texture, texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging_texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|error| super::hresult("ID3D11DeviceContext::Map", &error))?;

            let row_bytes = description.Width as usize * 4;
            let mut pixels = Vec::with_capacity(row_bytes * description.Height as usize);
            for row in 0..description.Height as usize {
                let source = (mapped.pData as *const u8).add(row * mapped.RowPitch as usize);
                pixels.extend_from_slice(std::slice::from_raw_parts(source, row_bytes));
            }
            self.context.Unmap(&staging_texture, 0);
            Ok(pixels)
        }
    }

    /// Present the current back buffer.
    pub fn present(&self, swap_chain: &IDXGISwapChain1) -> Result<(), String> {
        unsafe { swap_chain.Present(1, DXGI_PRESENT(0)) }
            .ok()
            .map_err(|error| super::hresult("IDXGISwapChain::Present", &error))
    }

    /// Detect `DXGI_ERROR_DEVICE_REMOVED` / `DEVICE_RESET` / `DEVICE_HUNG`.
    pub fn is_device_lost(message: &str) -> bool {
        const DEVICE_REMOVED: i32 = 0x887A_0005u32 as i32;
        const DEVICE_HUNG: i32 = 0x887A_0006u32 as i32;
        const DEVICE_RESET: i32 = 0x887A_0007u32 as i32;
        message
            .rsplit('#')
            .next()
            .and_then(|tail| tail.strip_prefix("code="))
            .and_then(|value| value.parse::<i32>().ok())
            .is_some_and(|code| {
                code == DEVICE_REMOVED || code == DEVICE_HUNG || code == DEVICE_RESET
            })
    }

    /// Build the DirectComposition target that hosts the swap chain on an HWND.
    pub fn create_composition_target(&self, window: HWND) -> Result<CompositionTarget, String> {
        let dxgi_device: IDXGIDevice = self
            .d3d
            .cast()
            .map_err(|error| super::hresult("IDXGIDevice::cast", &error))?;
        let device: IDCompositionDevice = unsafe { DCompositionCreateDevice(&dxgi_device) }
            .map_err(|error| super::hresult("DCompositionCreateDevice", &error))?;
        let target: IDCompositionTarget = unsafe { device.CreateTargetForHwnd(window, true) }
            .map_err(|error| super::hresult("CreateTargetForHwnd", &error))?;
        let visual: IDCompositionVisual = unsafe { device.CreateVisual() }
            .map_err(|error| super::hresult("IDCompositionDevice::CreateVisual", &error))?;
        unsafe {
            target
                .SetRoot(&visual)
                .map_err(|error| super::hresult("IDCompositionTarget::SetRoot", &error))?;
        }
        Ok(CompositionTarget {
            device,
            target,
            visual,
        })
    }
}

/// DirectComposition objects that bind one swap chain to one HWND.
pub struct CompositionTarget {
    device: IDCompositionDevice,
    target: IDCompositionTarget,
    visual: IDCompositionVisual,
}

impl CompositionTarget {
    /// Point the composition visual at a (new) swap chain.
    pub fn attach(&self, swap_chain: &IDXGISwapChain1) -> Result<(), String> {
        unsafe {
            self.visual
                .SetContent(swap_chain)
                .map_err(|error| super::hresult("IDCompositionVisual::SetContent", &error))?;
            self.device
                .Commit()
                .map_err(|error| super::hresult("IDCompositionDevice::Commit", &error))?;
        }
        Ok(())
    }

    pub fn commit(&self) -> Result<(), String> {
        unsafe { self.device.Commit() }
            .map_err(|error| super::hresult("IDCompositionDevice::Commit", &error))
    }
}

impl Drop for CompositionTarget {
    fn drop(&mut self) {
        // Release the visual before the target and the device.
        let _ = unsafe { self.target.SetRoot(None) };
        let _ = unsafe { self.device.Commit() };
    }
}

/// Create a D2D bitmap over an existing texture.
///
/// `options` decides whether the bitmap can be drawn into
/// (`D2D1_BITMAP_OPTIONS_TARGET`, for tests and readback harnesses) or only sampled
/// (`D2D1_BITMAP_OPTIONS_CANNOT_DRAW`, for the magnifier's zoom source).
#[cfg_attr(not(test), allow(dead_code))]
/// Wrap an existing GPU texture as a D2D bitmap.
///
/// `alpha_mode` is explicit because the two callers differ: the composition back buffer
/// is premultiplied, while the captured desktop is opaque and must be blended with
/// alpha ignored.
pub fn create_bitmap_from_texture(
    d2d_context: &ID2D1DeviceContext,
    texture: &ID3D11Texture2D,
    options: D2D1_BITMAP_OPTIONS,
    alpha_mode: D2D1_ALPHA_MODE,
) -> WinResult<ID2D1Bitmap1> {
    let surface: IDXGISurface = texture.cast()?;
    let properties = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: alpha_mode,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: options,
        ..Default::default()
    };
    unsafe { d2d_context.CreateBitmapFromDxgiSurface(&surface, Some(&properties)) }
}

#[cfg(test)]
mod tests {
    use super::GraphicsDevice;
    use ::windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use ::windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, UnregisterClassW,
        WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_POPUP,
    };

    /// Create a hidden popup window shaped exactly like the capture overlay.
    ///
    /// Returns `None` when the process has no window station (headless test run).
    fn hidden_overlay_window() -> Option<(HWND, ::windows::core::HSTRING)> {
        let class_name = ::windows::core::HSTRING::from("SnapClipCompositionTest");
        let title = ::windows::core::HSTRING::from("SnapClip");
        unsafe extern "system" fn window_proc(
            window: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }

        let instance = unsafe { GetModuleHandleW(None) }.ok()?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: ::windows::core::PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return None;
        }
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
                ::windows::core::PCWSTR(class_name.as_ptr()),
                ::windows::core::PCWSTR(title.as_ptr()),
                WS_POPUP,
                0,
                0,
                320,
                200,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
        .ok()?;
        Some((window, class_name))
    }

    /// The overlay's real presentation path: a DirectComposition target on a popup
    /// window, a monitor-sized flip-model swap chain and a drawn frame.
    #[test]
    fn composition_target_accepts_the_overlay_swap_chain_and_presents() {
        let Some((window, class_name)) = hidden_overlay_window() else {
            eprintln!("no window station in this session; skipping the composition check");
            return;
        };
        let Ok(device) = GraphicsDevice::create() else {
            unsafe {
                let _ = DestroyWindow(window);
                let _ = UnregisterClassW(
                    ::windows::core::PCWSTR(class_name.as_ptr()),
                    Some(GetModuleHandleW(None).unwrap().into()),
                );
            }
            return;
        };

        let composition = device.create_composition_target(window).unwrap();
        let swap_chain = device.create_composition_swap_chain(320, 200).unwrap();
        composition.attach(&swap_chain).unwrap();

        let context = device.create_d2d_context().unwrap();
        let target = device.create_target_bitmap(&context, &swap_chain).unwrap();
        let pixel_size = unsafe { target.GetPixelSize() };
        assert_eq!((pixel_size.width, pixel_size.height), (320, 200));

        // One present cycle: this is what the overlay does on every repaint.
        device.present(&swap_chain).unwrap();
        composition.commit().unwrap();

        let instance = unsafe { GetModuleHandleW(None) }.unwrap();
        unsafe {
            let _ = DestroyWindow(window);
            let _ = 
                UnregisterClassW(::windows::core::PCWSTR(class_name.as_ptr()), Some(instance.into()));
        }
    }

    #[test]
    fn device_lost_codes_are_recognised_from_tagged_messages() {
        assert!(GraphicsDevice::is_device_lost("Present failed #code=-2005270523"));
        assert!(GraphicsDevice::is_device_lost("Map failed #code=-2005270521"));
        assert!(!GraphicsDevice::is_device_lost("Present failed #code=0"));
        assert!(!GraphicsDevice::is_device_lost("no code here"));
    }

    #[test]
    fn device_creation_and_texture_round_trip() {
        // Skip silently on machines without a D3D11 device (headless CI).
        let Ok(device) = GraphicsDevice::create() else {
            return;
        };
        let pixels: Vec<u8> = (0..(4 * 3 * 4)).map(|index| index as u8).collect();
        let frame = device.create_bgra_texture(4, 3, &pixels).unwrap();
        assert_eq!((frame.width, frame.height), (4, 3));
        let read_back = device.read_back_bgra(&frame.texture).unwrap();
        assert_eq!(read_back, pixels);

        let wrong = device.create_bgra_texture(4, 3, &[0u8; 8]).unwrap_err();
        assert!(wrong.contains("does not match"));
    }

    #[test]
    fn swap_chain_can_be_created_and_presented() {
        let Ok(device) = GraphicsDevice::create() else {
            return;
        };
        let swap_chain = device.create_composition_swap_chain(64, 48).unwrap();
        let context = device.create_d2d_context().unwrap();
        let target = device.create_target_bitmap(&context, &swap_chain).unwrap();
        let pixel_size = unsafe { target.GetPixelSize() };
        assert_eq!((pixel_size.width, pixel_size.height), (64, 48));
        // Present on an unattached swap chain is allowed and must not error.
        device.present(&swap_chain).unwrap();
    }
}











