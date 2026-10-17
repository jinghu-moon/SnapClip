//! 倒计时期间压暗选区之外的那一层，单独开一个 **layered** 窗口。
//!
//! 为什么必须单独一个窗口：主浮层是 DirectComposition 窗口，**不能**是 layered；而
//! "能看见 + 鼠标穿过去到别的应用"只有 `WS_EX_LAYERED | WS_EX_TRANSPARENT` 这一条路
//! （`win/window.rs` 的对照表：单独的 `WS_EX_TRANSPARENT` 不起作用，`HTTRANSPARENT`
//! 只覆盖同一线程）。
//!
//! 这个文件的每个细节都是差分实验量出来的（docs/23 §14.55.9.8）：
//!
//! * 普通样式位**必须**在创建时带上 `WS_VISIBLE`（贴图窗口踩过同一个坑）；
//! * `SetWindowRgn` 的矩形是**客户区坐标**——上一版把屏幕坐标直接递进去，区域整个落在
//!   窗口外面，窗口被裁成空，于是"读数全绿但屏幕上什么都没有"；
//! * `WDA_EXCLUDEFROMCAPTURE` 会让它从**任何截图**里消失（延时贴图因此不会把遮罩截进去），
//!   所以验证"它真的显示了"时不能靠截图，除非临时把这一位摘掉。
//!
//! 内容：优先用渲染器给的**「冻帧 + 模糊 + 压暗」**一帧（`d2d::DimSource`，与正常截图态的
//! `mask_blur` 观感一致）；拿不到时退回**单层纯色**（把遮罩那两层合成等价值），绝不阻断倒计时。
//! 纯色那条的合成：`MASK_RGB` 在 `MASK_ALPHA`，再一层主题色在 `MASK_TINT_ALPHA`；像素同样是
//! premultiplied，交给 `UpdateLayeredWindow(ULW_ALPHA)`。内容与窗口**同尺寸**时逐行精确拷贝；
//! 尺寸对不上就退回纯色 —— 不做 GDI 缩放：`StretchBlt(SRCCOPY)` 会把 32bpp `BI_RGB` DIB 的
//! alpha 清零、`AlphaBlend` 在这种 DIB 上静默失败，两者都会让整块窗口变透明（docs/23 §14.55.9.11）。

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{GetLastError, HWND, POINT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    SetWindowRgn, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowDisplayAffinity, GetWindowLongPtrW,
    GetWindowRect, IsWindow, IsWindowVisible, LoadCursorW, RegisterClassW, SetWindowDisplayAffinity,
    SetWindowPos, ShowWindow, UpdateLayeredWindow, HWND_TOPMOST, IDC_ARROW, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WDA_EXCLUDEFROMCAPTURE, WNDCLASSW,
    WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
};

use crate::geometry::Rect;
use crate::ring_contrast::{ACCENT_RGB, MASK_ALPHA, MASK_RGB, MASK_TINT_ALPHA};
use crate::windows::win::d2d::DimSource;
use crate::windows::win::window::{region_from_pieces, WindowRegion};

/// 窗口类名。只在第一次显示时注册。
const CLASS_NAME: &str = "SnapClipCountdownDim";

/// 交给 `UpdateLayeredWindow` 的那一层的像素（premultiplied BGRA）。
///
/// 单层等效值来自遮罩自己的两层叠加 —— 不另调一套观感：
/// 先 `MASK_RGB` 以 `MASK_ALPHA`，再 `ACCENT_RGB` 以 `MASK_TINT_ALPHA`。
pub(super) fn dim_pixel() -> [u8; 4] {
    let base_a = MASK_ALPHA;
    let tint_a = MASK_TINT_ALPHA;
    let out_a = tint_a + base_a * (1.0 - tint_a);
    if out_a <= f32::EPSILON {
        return [0, 0, 0, 0];
    }
    let channel = |top: u8, bottom: u8| -> u8 {
        let straight = (top as f32 * tint_a + bottom as f32 * base_a * (1.0 - tint_a)) / out_a;
        straight.round().clamp(0.0, 255.0) as u8
    };
    let r = channel(ACCENT_RGB.r, MASK_RGB.r);
    let g = channel(ACCENT_RGB.g, MASK_RGB.g);
    let b = channel(ACCENT_RGB.b, MASK_RGB.b);
    let a = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    let premul = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
    [premul(b), premul(g), premul(r), a]
}

/// 选区之外那圈的**客户区**矩形：外框是整个窗口，洞是选区（减去窗口原点）。
pub(super) fn dim_hole(monitor: Rect, selection: Rect) -> Option<Rect> {
    let width = monitor.width();
    let height = monitor.height();
    if width <= 0 || height <= 0 || selection.is_empty() {
        return None;
    }
    let hole = Rect::new(
        selection.left - monitor.left,
        selection.top - monitor.top,
        selection.right - monitor.left,
        selection.bottom - monitor.top,
    );
    Some(hole)
}

/// 倒计时遮罩层。懒创建；`hide` 之后保留窗口，下次显示复用。
#[derive(Default)]
pub(super) struct DimLayer {
    hwnd: HWND,
    memory_dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u32,
    width: i32,
    height: i32,
    /// 当前内容是否来自模糊渲染（诊断用：`true` = 模糊帧，`false` = 纯色兜底）。
    content_is_blur: bool,
}

// 窗口与 GDI 对象都由本结构独占持有，且只在浮层线程上使用。
unsafe impl Send for DimLayer {}

impl DimLayer {
    /// 显示（或更新）遮罩：铺满 `monitor`，选区不出现在区域里。
    pub(super) fn show(
        &mut self,
        monitor: Rect,
        selection: Rect,
        content: Option<&DimSource>,
    ) -> Result<(), String> {
        if monitor.width() <= 0 || monitor.height() <= 0 {
            return Err(format!(
                "遮罩层尺寸非法 {}x{}",
                monitor.width(),
                monitor.height()
            ));
        }
        self.ensure_window(monitor)?;
        self.paint(content);
        self.submit(Some((monitor.left, monitor.top)))?;
        self.apply_shape(monitor, selection)?;
        self.exclude_from_capture();
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            // 两个窗口都是 topmost：主浮层必须压在遮罩层上面，否则选框环的外半边会被压暗。
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        Ok(())
    }

    /// 拖动改框时只换"洞"：不重填 DIB、不重新提交层内容。
    pub(super) fn update(&mut self, monitor: Rect, selection: Rect) -> Result<(), String> {
        if self.hwnd.is_null() {
            return Ok(());
        }
        self.apply_shape(monitor, selection)
    }

    pub(super) fn hide(&mut self) {
        if !self.hwnd.is_null() {
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }

    /// 仅供测试：临时摘掉/恢复"排除出截图"，这样截图-based 观测才能看见它。
    #[cfg(test)]
    pub(super) fn set_capture_exclusion(&self, excluded: bool) {
        if self.hwnd.is_null() {
            return;
        }
        let affinity = if excluded {
            WDA_EXCLUDEFROMCAPTURE
        } else {
            windows_sys::Win32::UI::WindowsAndMessaging::WDA_NONE
        };
        unsafe { SetWindowDisplayAffinity(self.hwnd, affinity) };
    }

    /// 仅供测试：窗口句柄。
    #[cfg(test)]
    pub(super) fn hwnd_for_test(&self) -> HWND {
        self.hwnd
    }

    /// 永久保留的一行诊断：这条功能只有这里能观测（layered 窗口截不到图）。
    pub(super) fn diagnose(&self) -> String {
        if self.hwnd.is_null() {
            return "hwnd=null".to_owned();
        }
        let mut rect: windows_sys::Win32::Foundation::RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(self.hwnd, &mut rect) };
        let ex_style = unsafe { GetWindowLongPtrW(self.hwnd, -20) as u32 };
        let mut affinity = 0u32;
        let affinity_ok = unsafe { GetWindowDisplayAffinity(self.hwnd, &mut affinity) } != 0;
        let first = if self.bits.is_null() {
            [0u8; 4]
        } else {
            let pixel = unsafe { *self.bits };
            pixel.to_le_bytes()
        };
        let content = if self.content_is_blur { "blur" } else { "flat" };
        format!(
            "hwnd={:?} content={} is_window={} visible={} rect=({},{},{},{}) dib=({}, {}) first_px={:?} affinity=0x{:x}(ok={}) style_complete={}",
            self.hwnd,
            content,
            unsafe { IsWindow(self.hwnd) } != 0,
            unsafe { IsWindowVisible(self.hwnd) } != 0,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            self.width,
            self.height,
            first,
            affinity,
            affinity_ok,
            ex_style & ex_style_required() == ex_style_required(),
        )
    }

    fn ensure_window(&mut self, monitor: Rect) -> Result<(), String> {
        if !self.hwnd.is_null() {
            return Ok(());
        }
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        if instance.is_null() {
            return Err("GetModuleHandleW 失败".to_owned());
        }
        let name = wide(CLASS_NAME);
        let mut class: WNDCLASSW = unsafe { std::mem::zeroed() };
        class.lpfnWndProc = Some(wndproc);
        class.hInstance = instance;
        class.lpszClassName = name.as_ptr();
        class.hCursor = unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) };
        class.hbrBackground = unsafe {
            windows_sys::Win32::Graphics::Gdi::GetStockObject(
                windows_sys::Win32::Graphics::Gdi::BLACK_BRUSH,
            ) as _
        };
        unsafe { RegisterClassW(&class) };

        let hwnd = unsafe {
            CreateWindowExW(
                ex_style_required(),
                name.as_ptr(),
                std::ptr::null(),
                // `WS_VISIBLE` 必须在创建时就带上（贴图窗口踩过的坑，docs/23 §14.55.9.8）。
                WS_POPUP | WS_VISIBLE,
                monitor.left,
                monitor.top,
                monitor.width(),
                monitor.height(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null_mut(),
            )
        };
        if hwnd.is_null() {
            return Err(format!("CreateWindowExW 失败，错误码 {}", unsafe {
                GetLastError()
            }));
        }

        let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: monitor.width(),
            biHeight: -monitor.height(),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let screen = unsafe { GetDC(std::ptr::null_mut()) };
        let memory_dc = unsafe { CreateCompatibleDC(screen) };
        unsafe { ReleaseDC(std::ptr::null_mut(), screen) };
        if memory_dc.is_null() {
            unsafe { DestroyWindow(hwnd) };
            return Err("CreateCompatibleDC 失败".to_owned());
        }
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = unsafe {
            CreateDIBSection(
                memory_dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            )
        };
        if bitmap.is_null() || bits.is_null() {
            unsafe {
                DeleteDC(memory_dc);
                DestroyWindow(hwnd);
            }
            return Err("CreateDIBSection 失败".to_owned());
        }
        unsafe { SelectObject(memory_dc, bitmap as _) };

        self.hwnd = hwnd;
        self.memory_dc = memory_dc;
        self.bitmap = bitmap;
        self.bits = bits.cast::<u32>();
        self.width = monitor.width();
        self.height = monitor.height();
        Ok(())
    }

    /// 铺内容：同尺寸精确拷贝；没内容或尺寸对不上就退回纯色。
    fn paint(&mut self, content: Option<&DimSource>) {
        match content {
            Some(source)
                if source.width as i32 == self.width && source.height as i32 == self.height =>
            {
                self.copy_source(source);
                self.content_is_blur = true;
            }
            Some(_) | None => {
                // 尺寸对不上（例如显示器刚换过）就退回纯色：宁可少一层模糊，
                // 也不要用 GDI 缩放把 alpha 弄丢（见模块文档）。
                self.fill();
                self.content_is_blur = false;
            }
        }
    }

    /// 内容与窗口同分辨率：逐行搬进 DIB（BGRA 逐字节一致，premultiplied 原样保留）。
    fn copy_source(&mut self, source: &DimSource) {
        if self.bits.is_null() {
            return;
        }
        let row_bytes = (self.width as usize).saturating_mul(4);
        let target = self.bits.cast::<u8>();
        for row in 0..self.height as usize {
            let from = row.saturating_mul(row_bytes);
            let Some(slice) = source.pixels.get(from..from + row_bytes) else {
                return;
            };
            unsafe {
                std::ptr::copy_nonoverlapping(slice.as_ptr(), target.add(from), row_bytes)
            };
        }
    }

    fn fill(&mut self) {
        if self.bits.is_null() {
            return;
        }
        let [b, g, r, a] = dim_pixel();
        let pixel = (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
        let count = (self.width as usize).saturating_mul(self.height as usize);
        for index in 0..count {
            unsafe { *self.bits.add(index) = pixel };
        }
    }

    fn submit(&self, position: Option<(i32, i32)>) -> Result<(), String> {
        let size = SIZE {
            cx: self.width,
            cy: self.height,
        };
        let src = POINT { x: 0, y: 0 };
        let destination = position.map(|(x, y)| POINT { x, y });
        let destination = match &destination {
            Some(point) => point,
            None => std::ptr::null(),
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let ok = unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                std::ptr::null_mut(),
                destination,
                &size,
                self.memory_dc,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            )
        };
        if ok == 0 {
            return Err(format!("UpdateLayeredWindow 失败，错误码 {}", unsafe {
                GetLastError()
            }));
        }
        Ok(())
    }

    fn apply_shape(&mut self, monitor: Rect, selection: Rect) -> Result<(), String> {
        let outer = Rect::new(0, 0, monitor.width(), monitor.height());
        let pieces = match dim_hole(monitor, selection) {
            Some(hole) => vec![WindowRegion::Ring {
                outer,
                inner: hole,
            }],
            // 没有选区（或退化）时整块铺满：宁可多压暗，也不要留一个"什么都不显示"的空洞。
            None => vec![WindowRegion::Rect(outer)],
        };
        let region = region_from_pieces(&pieces).ok_or_else(|| "区域构造失败".to_owned())?;
        let ok = unsafe { SetWindowRgn(self.hwnd, region, 1) };
        if ok == 0 {
            // 失败时 `SetWindowRgn` 不接管区域，必须自己释放。
            unsafe { DeleteObject(region as _) };
            return Err(format!("SetWindowRgn 失败，错误码 {}", unsafe {
                GetLastError()
            }));
        }
        Ok(())
    }

    fn exclude_from_capture(&self) {
        let ok = unsafe { SetWindowDisplayAffinity(self.hwnd, WDA_EXCLUDEFROMCAPTURE) };
        if ok == 0 {
            eprintln!(
                "[snapclip][capture] countdown dim: 排除出截图失败，错误码 {}",
                unsafe { GetLastError() }
            );
        }
    }
}

impl Drop for DimLayer {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.is_null() {
                DestroyWindow(self.hwnd);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap as _);
            }
            if !self.memory_dc.is_null() {
                DeleteDC(self.memory_dc);
            }
        }
        self.hwnd = std::ptr::null_mut();
        self.bitmap = std::ptr::null_mut();
        self.memory_dc = std::ptr::null_mut();
        self.content_is_blur = false;
    }
}

/// 这层窗口要求的 ex-style：前两位是跨进程穿透、其余不影响输入。
fn ex_style_required() -> u32 {
    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wndproc(
    window: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dim_pixel_is_the_single_layer_equivalent_of_the_overlay_mask() {
        let [b, g, r, a] = dim_pixel();
        assert_eq!(a, 129, "α 应当约等于合成的 0.505");
        // premultiplied：每个通道都不能超过 α。
        assert!(b <= a && g <= a && r <= a, "{b} {g} {r} <= {a}");
        // 主题色是蓝的，合成后蓝分量最大。
        assert!(b > r, "遮罩应当偏主题色: b={b} r={r}");
    }

    #[test]
    fn the_hole_is_in_client_coordinates() {
        let monitor = Rect::new(100, 50, 1100, 850);
        let selection = Rect::new(300, 150, 700, 550);
        let hole = dim_hole(monitor, selection).expect("应当有洞");
        // 客户区坐标：窗口原点变成 (0,0)。
        assert_eq!((hole.left, hole.top), (200, 100));
        assert_eq!((hole.right, hole.bottom), (600, 500));
        // 退化选区不给洞。
        assert!(dim_hole(monitor, Rect::new(10, 10, 10, 10)).is_none());
    }

    #[test]
    fn the_required_style_bits_are_the_click_through_layered_set() {
        let ex = ex_style_required();
        assert_ne!(ex & WS_EX_LAYERED, 0);
        assert_ne!(ex & WS_EX_TRANSPARENT, 0);
        assert_ne!(ex & WS_EX_TOPMOST, 0);
        assert_ne!(ex & WS_EX_TOOLWINDOW, 0);
        assert_ne!(ex & WS_EX_NOACTIVATE, 0);
    }

}
