# Direct2D 原生真高斯模糊信息栏

## 一、 核心架构原则
1. **真高斯与零额外离屏**：使用 `CLSID_D2D1GaussianBlur`。将整帧 `frame_bitmap` 作为输入，利用 `DrawImage` 的 source rect 仅提取信息栏区域，GPU 自动处理边缘采样，杜绝黑边（Halo）且无额外显存开销。
2. **实例缓存与零分配**：`ID2D1Effect` 与 `ID2D1SolidColorBrush` 在 `recreate_resources` 中一次性创建，绘制帧仅执行 `SetInput`、`SetValue` 和 `DrawImage`。
3. **圆角复用**：模糊与着色均绘制于现有的圆角 `PushLayer` 裁剪层内，天然获得完美圆角。
4. **低 Alpha 着色**：Tint 层 Alpha 必须降至 `0.45 ~ 0.55`，确保高斯模糊的纹理透出，避免被纯色盖没。

---

## 二、 关键技术修正与避坑 (windows-rs 0.61+)

在落地前，必须确认以下 API 细节，这是编译通过且运行正确的关键：

1. **接口转换 (`cast`)**：`SetInput` 和 `DrawImage` 接收的是 `ID2D1Image`。`ID2D1Bitmap1` 和 `ID2D1Effect` 必须显式调用 `.cast::<ID2D1Image>()?`。
2. **属性名修正**：优化模式的属性名是 `D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION_MODE`（非 `..._OPTIMIZATION`）。
3. **PROPVARIANT 构造**：优先使用 Win32 系统函数 `InitPropVariantFromFloat` / `InitPropVariantFromUInt32`，比手动构造 Union 更安全且符合 COM 规范。
4. **FillRectangle 参数**：第二个参数必须是 `&ID2D1Brush`（或 `&ID2D1SolidColorBrush`），而非颜色结构体。

---

## 三、 核心代码实现

### 1. PROPVARIANT 系统级 Helper
```rust
use windows::Win32::System::Com::{PROPVARIANT, InitPropVariantFromFloat, InitPropVariantFromUInt32};
use windows::core::Result;

#[inline]
fn propvar_f32(v: f32) -> Result<PROPVARIANT> {
    let mut pv = PROPVARIANT::default();
    unsafe { InitPropVariantFromFloat(v, &mut pv) }?;
    Ok(pv)
}

#[inline]
fn propvar_u32(v: u32) -> Result<PROPVARIANT> {
    let mut pv = PROPVARIANT::default();
    unsafe { InitPropVariantFromUInt32(v, &mut pv) }?;
    Ok(pv)
}
```

### 2. 资源缓存与初始化 (`recreate_resources`)
```rust
use windows::Win32::Graphics::Direct2D::{
    ID2D1Effect, ID2D1SolidColorBrush, Effects::{
        CLSID_D2D1GaussianBlur, D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION_MODE,
        D2D1_GAUSSIANBLUR_PROP_BORDER_MODE, D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED,
        D2D1_BORDER_MODE_HARD
    }
};

// 在 Renderer 结构体中增加：
// blur_effect: Option<ID2D1Effect>,
// tint_brush: Option<ID2D1SolidColorBrush>,

// 在 recreate_resources 中：
let blur = self.d2d.CreateEffect(&CLSID_D2D1GaussianBlur)?;

// 预设静态属性
blur.SetValue(
    D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION_MODE.0 as u32,
    &propvar_u32(D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED.0 as u32)?,
)?;
blur.SetValue(
    D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
    &propvar_u32(D2D1_BORDER_MODE_HARD.0 as u32)?, // HARD 避免边缘采样到透明黑边
)?;
resources.blur_effect = Some(blur);

// 缓存 Tint 笔刷
let tint_color = D2D1_COLOR_F { r: 0.07, g: 0.07, b: 0.08, a: 0.48 }; // #121214, Alpha 0.48
resources.tint_brush = Some(self.d2d.CreateSolidColorBrush(&tint_color, None)?);
```

### 3. 核心绘制逻辑 (`draw_magnifier`)
```rust
use windows::Win32::Graphics::Direct2D::{
    ID2D1Image, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER,
    Effects::D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION
};

if let (Some(fb), Some(blur), Some(tint_brush)) = (
    resources.frame_bitmap.as_ref(),
    resources.blur_effect.as_ref(),
    resources.tint_brush.as_ref(),
) {
    // 1. 绑定整帧输入 (cast 为 ID2D1Image)
    let fb_img: ID2D1Image = fb.cast()?;
    blur.SetInput(0, &fb_img, true)?;

    // 2. 动态更新模糊半径 (假设模式A：D2D上下文DPI=96，坐标为物理像素)
    let std_dev = 12.0 * scale; // 12 DIP 基础半径
    blur.SetValue(
        D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
        &propvar_f32(std_dev)?,
    )?;

    // 3. 局部绘制：只渲染信息栏区域
    let dest = D2D1_POINT_2F { x: info.left as f32, y: info.top as f32 };
    let src = to_d2d_rect(info);
    
    let blur_img: ID2D1Image = blur.cast()?;
    self.d2d.DrawImage(
        &blur_img,
        Some(&dest),
        Some(&src),
        D2D1_INTERPOLATION_MODE_LINEAR,
        D2D1_COMPOSITE_MODE_SOURCE_OVER,
    )?;

    // 4. 叠加半透明深色 Tint
    self.d2d.FillRectangle(&to_d2d_rect(info), tint_brush)?;
}
```

---

## 四、 视觉与质感调优 (The Secret Sauce)

为了达到 Windows 11 原生控件（如亚克力/云母）的顶级质感，建议在基础高斯模糊之上叠加以下两层处理：

1. **L1 遮罩光影一致性**：
   * **问题**：如果 L1 层压暗了桌面，直接 Blur 原图（L0）会导致信息栏像一个“发光的窗户”。
   * **解法**：除了降低 Tint 的 Alpha（0.48），若追求极致物理正确，可在 `GaussianBlur` 后串联一个 `CLSID_D2D1ColorMatrix` 效果，将 RGB 通道乘以 0.6，使其亮度与 L1 遮罩完美融合。
2. **噪点纹理 (Noise)**：
   * **问题**：纯色高斯模糊在渐变区域容易出现色彩断层（Banding）。
   * **解法**：在资源初始化时生成一张 128x128 的灰度噪点 `ID2D1Bitmap`。在 `FillRectangle` 之后，使用 `D2D1_COMPOSITE_MODE_MULTIPLY` 或极低 Alpha 的 `SOURCE_OVER` 将噪点图平铺在信息栏区域。这能瞬间打破数字感，赋予材质真实的“磨砂”颗粒感。

---

## 五、 DPI 与坐标系规范 (必须二选一)

| 模式              | D2D Context DPI | 坐标单位       | `info` 矩形处理 | `std_dev` 计算     | 适用场景                   |
| :---------------- | :-------------- | :------------- | :-------------- | :----------------- | :------------------------- |
| **模式 A (推荐)** | 96.0 (默认)     | 物理像素       | 直接使用        | `blur_dip * scale` | 现有代码延续，逻辑最简单   |
| **模式 B**        | 屏幕实际 DPI    | DIP (逻辑像素) | 需除以 `scale`  | `blur_dip`         | 严格遵循 D2D 官方 DPI 规范 |

*注：切勿混用，否则模糊半径和绘制位置会发生严重偏移。*

---

## 六、 性能、占用与降级

* **GPU 占用**：由于 `DrawImage` 限制了输出区域，GPU 仅对信息栏（约 300×80）执行 Fragment Shader，边缘像素仅作为只读采样源，开销极低。
* **内存占用**：仅增加一个 Effect 实例和一个 Brush 实例，无额外离屏 RenderTarget。
* **降级方案 (Fallback)**：若目标环境（如极老旧的虚拟机）不支持 D2D Effects，可退回“降采样到 1/4 尺寸 $\rightarrow$ 使用 `HIGH_QUALITY_CUBIC` 插值放大”的近似模糊方案。

---

## 七、 最终落地 Checklist

- [ ] **Cargo.toml**：确认包含 `Win32_Graphics_Direct2D`、`Win32_Graphics_Direct2D_Effects`、`Win32_System_Com`。
- [ ] **结构体更新**：在 Renderer 中增加 `blur_effect` 和 `tint_brush` 字段。
- [ ] **Helper 实现**：引入 `propvar_f32` 和 `propvar_u32`。
- [ ] **资源初始化**：在 `recreate_resources` 中完成 Effect 静态属性配置和 Brush 创建。
- [ ] **绘制替换**：在 `draw_magnifier` 中移除旧的 `FillRectangle(bg)`，替换为 `DrawImage` + `FillRectangle(tint)`。
- [ ] **真机验证**：在不同 DPI 缩放（100%, 125%, 150%）下检查模糊边缘是否自然、圆角是否完美裁剪。