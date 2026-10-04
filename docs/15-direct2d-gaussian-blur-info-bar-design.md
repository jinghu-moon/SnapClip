# Direct2D 原生真高斯模糊信息栏

> 已按 windows-rs 0.61.3 实际绑定形态修正，代码片段可直接照抄编译。
> 与旧版差异集中在：PROPVARIANT helper 删除、`Effects::` 子模块路径删除、
> `SetInput` 无 `Result`、`DrawImage` 用 `Option<*const _>`、`SetValue` 是
> `&[u8]` 字节切片而不是 `&PROPVARIANT`、Mode A/B 二选一改为拍板 Mode A、
> ColorMatrix / Noise 推迟到 v2、字段归属明确到 `OverlayRenderer`。

## 一、 核心架构原则

1. **真高斯 + 局部求值**：`CLSID_D2D1GaussianBlur` 输入整帧 `frame_bitmap`，通过
   `ID2D1DeviceContext::DrawImage` 的 `imageRectangle` 只把信息栏那块从 effect
   输出里取出并绘制，D2D 会按 destination ∩ clip 计算 effect 求值范围，不做整帧
   RT。注意：layer + effect 仍会各自分配一张中间 surface（layer 自身 + effect 输出），
   开销有界但非严格为零。
2. **实例缓存与零分配**：`ID2D1Effect` 和 `ID2D1SolidColorBrush` 都在
   `OverlayRenderer::recreate_resources` 里一次性创建（跟 `info_bg_brush` 等
   现有 brush 同一生命周期），draw 帧只做 `SetInput` + `SetValue` + `DrawImage`。
3. **圆角复用**：模糊与 Tint 都绘制在现有 `PushLayer(geometricMask = 圆角矩形)`
   裁剪层内，天然获得完美圆角。
4. **低 Alpha 着色**：Tint 层 Alpha 从旧 `info_bg_brush` 的 0.88 降到 **0.48**，
   让高斯纹理透出。

---

## 二、 windows-rs 0.61.3 关键 API 事实（先读这段再动代码）

下面这些是**必须与旧文档不同**的地方，全部经 crates 源码核对：

| 主题 | windows-rs 0.61.3 实际形态 | 常见错误写法 |
| :- | :- | :- |
| `Effects` 命名空间 | **不存在**。`ID2D1Effect` / `ID2D1Image` / `CLSID_D2D1GaussianBlur` / `D2D1_GAUSSIANBLUR_PROP_*` / `D2D1_GAUSSIANBLUR_OPTIMIZATION_*` / `D2D1_PROPERTY_TYPE_*` / `D2D1_INTERPOLATION_MODE_*` 位于 `windows::Win32::Graphics::Direct2D` 顶层；`D2D1_BORDER_MODE_*` / `D2D1_COMPOSITE_MODE_*` / `D2D_RECT_F` 位于 `windows::Win32::Graphics::Direct2D::Common`（尽管名字看着像 Direct2D 根） | ~~`Direct2D::Effects::CLSID_D2D1GaussianBlur`~~ 或 ~~`Direct2D::D2D1_BORDER_MODE_HARD`~~ |
| Cargo feature | 只需已有的 `Win32_Graphics_Direct2D` 和 `Win32_Graphics_Direct2D_Common`；**不存在** `Win32_Graphics_Direct2D_Effects` 这个 feature | ~~加 `Win32_Graphics_Direct2D_Effects`~~ |
| `ID2D1Properties::SetValue` | `pub unsafe fn SetValue(&self, index: u32, r#type: D2D1_PROPERTY_TYPE, data: &[u8]) -> Result<()>` | ~~`(u32, &PROPVARIANT)`~~ |
| `ID2D1Effect::SetInput` | `pub unsafe fn SetInput<P1>(&self, index: u32, input: P1, invalidate: bool)` **返回 `()`，无 `Result`** | ~~`blur.SetInput(0, &img, true)?;`~~ |
| `ID2D1DeviceContext::DrawImage` | `DrawImage<P0: Param<ID2D1Image>>(&self, image, targetoffset: Option<*const Vector2>, imagerectangle: Option<*const Common::D2D_RECT_F>, interpolationmode, compositemode)` **返回 `()`，不是 `Result<()>`**（windows-rs 把 C++ HRESULT 丢掉了）；目标偏移是 `Vector2` 不是 `D2D1_POINT_2F`，两个 rect/point 是 `Option<*const _>` | ~~`self.d2d.DrawImage(...).map_err(...)?;`~~ |
| `ID2D1Effect::CreateEffect` 输入图像 cast | `P1: Param<ID2D1Image>` 已支持从 `&ID2D1Bitmap1` / `&ID2D1Effect` 上转型；显式 `.cast::<ID2D1Image>()?` 是 belt-and-braces，可选 | — |
| `PROPVARIANT` / `InitPropVariantFromFloat` | 存在于 `Win32::System::Com::StructuredStorage`（另一 feature），但**本方案用不到**，因为 SetValue 是 `&[u8]` | ~~propvar_f32 / propvar_u32 helper~~ |
| Enum → u32 传值 | windows-rs 里 `D2D1_BORDER_MODE` / `D2D1_GAUSSIANBLUR_OPTIMIZATION` 是 `pub struct X(pub i32)`，写值时 `v.0 as u32` 再 `to_ne_bytes()` | — |
| GaussianBlur 属性常量名 | `D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION` / `D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION` / `D2D1_GAUSSIANBLUR_PROP_BORDER_MODE`（**属性名无 `_MODE` 后缀**，与 C++ `D2D1_GAUSSIANBLUR_PROPERTY_PROP_OPTIMIZATION` 一致；enum 值才是 `D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED` 等） | ~~`D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION_MODE`~~ |

`D2D1_PROPERTY_TYPE_UNKNOWN` 让 D2D 依据 effect schema 自解释原始字节，无需按类型
手工挑 `FLOAT`/`UINT32`。

---

## 三、 落地代码

### 1. Effect 属性写入的小 helper

不封装 PROPVARIANT，只做 "把 f32/u32 转成 D2D 需要的 `&[u8]`"：

```rust
use windows::Win32::Graphics::Direct2D::{
    ID2D1Effect, D2D1_PROPERTY_TYPE_UNKNOWN,
};

trait BlurProperty {
    fn set_f32(&self, index: u32, value: f32) -> windows::core::Result<()>;
    fn set_u32(&self, index: u32, value: u32) -> windows::core::Result<()>;
}

impl BlurProperty for ID2D1Effect {
    #[inline]
    fn set_f32(&self, index: u32, value: f32) -> windows::core::Result<()> {
        let bytes = value.to_ne_bytes();
        unsafe { self.SetValue(index, D2D1_PROPERTY_TYPE_UNKNOWN, &bytes) }
    }
    #[inline]
    fn set_u32(&self, index: u32, value: u32) -> windows::core::Result<()> {
        let bytes = value.to_ne_bytes();
        unsafe { self.SetValue(index, D2D1_PROPERTY_TYPE_UNKNOWN, &bytes) }
    }
}
```

### 2. Effect / Tint brush 缓存（挂在 `OverlayRenderer`，不是 `ChromeResources`）

字段归属：跟 `info_bg_brush` / `border_brush` 完全一致的时机与生命周期。
`recreate_resources` 阶段建；`draw_layers` 里 clone 到 `ChromeResources`；`draw_magnifier`
只用 clone 出来的引用。

```rust
// 沿用现有 Direct2D 顶层命名空间（无 Effects 子模块）；
// BORDER_MODE / COMPOSITE_MODE / D2D_RECT_F 在 Common 子模块。
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_BORDER_MODE_HARD, D2D1_COMPOSITE_MODE_SOURCE_OVER,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1GaussianBlur,
    D2D1_GAUSSIANBLUR_PROP_BORDER_MODE,
    D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION,
    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED,
    ID2D1Effect, ID2D1Image,
};

// OverlayRenderer 结构体新增两个字段（与 info_bg_brush 等现有 brush 并列）：
//     blur_effect: Option<ID2D1Effect>,
//     info_tint_brush: Option<ID2D1SolidColorBrush>,

// OverlayRenderer::new 的初值：
//     blur_effect: None,
//     info_tint_brush: None,

// recreate_resources 中一次性创建（跟其他 brush 同段）：
unsafe {
    let blur: ID2D1Effect = self.d2d.CreateEffect(&CLSID_D2D1GaussianBlur)?;

    // 静态属性只在这里设一次，不每帧改（windows-rs 里属性名无 `_MODE` 后缀）
    blur.set_u32(
        D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
        D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED.0 as u32,
    )?;
    blur.set_u32(
        D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
        D2D1_BORDER_MODE_HARD.0 as u32, // HARD 避免读到帧外透明像素产生暗边
    )?;

    // Tint：#121214 @ 0.48（旧 info_bg_brush 是 0.88，压得太死看不见模糊纹理）
    let tint_color = D2D1_COLOR_F { r: 0.07, g: 0.07, b: 0.08, a: 0.48 };
    // windows-rs 0.61 里 ID2D1DeviceContext::CreateSolidColorBrush 是 2 参数：
    //   (&self, color: *const D2D1_COLOR_F, brushproperties: Option<*const D2D1_BRUSH_PROPERTIES>)
    // 与 d2d.rs 现有 `create_brush` helper 调用形态一致。
    let tint = self.d2d.CreateSolidColorBrush(&tint_color, None)?;

    self.blur_effect = Some(blur);
    self.info_tint_brush = Some(tint);
}
```

> 上面这段建议直接调 `self.create_brush(&tint_color)?`（`d2d.rs:1507` 已有）
> 保持一致；示例里内联只是为了让参数一眼可见。

### 3. draw_layers 中把新资源 clone 进 `ChromeResources`

`ChromeResources` 结构体新增两字段（都是 `Option<..>` 或已 unwrap 的接口值，跟
`frame_bitmap` 同风格）：

```rust
struct ChromeResources {
    // ... 现有字段
    blur_effect: Option<ID2D1Effect>,
    info_tint_brush: Option<ID2D1SolidColorBrush>,
}
```

在 `draw_layers` 组装 `ChromeResources` 的地方（`frame_bitmap` 那一段旁边）加：

```rust
blur_effect: self.blur_effect.clone(),
info_tint_brush: self.info_tint_brush.clone(),
```

### 4. `draw_magnifier` 中替换信息栏背景填充

现位置：`d2d.rs` 中 `self.d2d.FillRectangle(&to_d2d(info), &bg);`
（当前 `bg = self.info_bg_brush`，位于 `PushLayer` 内、info panel 起始处）。

**替换为**（保持在外层 PushLayer 的圆角 mask 之内）：

```rust
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_INTERPOLATION_MODE_LINEAR,
};

if let (Some(fb), Some(blur)) = (
    resources.frame_bitmap.as_ref(),
    resources.blur_effect.as_ref(),
) {
    // 1. 每次抽帧都要重绑输入（frame_bitmap 每 session 换一次）
    //    SetInput 返回 `()`，不是 Result —— 不要加 `?`
    let fb_img: ID2D1Image = fb.cast()?;
    blur.SetInput(0, &fb_img, true);

    // 2. 动态更新模糊半径（详见 §四 拍板公式）
    let std_dev = 12.0 * (metrics.dpi.max(96) as f32 / 96.0);
    blur.set_f32(
        D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
        std_dev,
    )?;

    // 3. DrawImage 只在 info 矩形范围内求值
    //    targetoffset 是 Option<*const Vector2>，imagerectangle 是 Option<*const D2D1_RECT_F>，
    //    必须显式 `as *const _` 转型；vector2() 是 d2d.rs 里已有的构造 helper。
    let dest = vector2(info.left as f32, info.top as f32);
    let src  = to_d2d(info);

    let blur_img: ID2D1Image = blur.cast()?;
    self.d2d.DrawImage(
        &blur_img,
        Some(&dest as *const windows_numerics::Vector2),
        Some(&src as *const D2D_RECT_F),
        D2D1_INTERPOLATION_MODE_LINEAR,
        D2D1_COMPOSITE_MODE_SOURCE_OVER,
    ); // ← DrawImage 返回 ()，不要加 .map_err(...)?

    // 4. 上层再叠 0.48 Tint（替换原来的 FillRectangle(bg)）
    let tint = self.require_brush(&resources.info_tint_brush, "info tint")?;
    self.d2d.FillRectangle(&to_d2d(info), &tint);
} else {
    // Effect 不可用时退回旧行为，避免整块信息栏消失
    self.d2d.FillRectangle(&to_d2d(info), &bg);
}
```

**旧代码里的 `bg`（`info_bg_brush`）** 保留即可（走 else 分支的 fallback），
不必删；但把 brush 的 alpha 从 0.88 调到 0.68，让 fallback 也不至于过黑。

---

## 四、 DPI 与坐标系（拍板 Mode A，不再"二选一"）

- **DPI 与坐标（Mode A）**：SnapClip 当前 `OverlayRenderer` 不调 `SetDpi`，
  target 由 `CreateBitmapFromDxgiSurface` 建立默认 96 DPI，因此**实际是 Mode A**：
  - D2D context DPI = 96 → 1 DIP = 1 物理像素，所有传入坐标都用物理像素。
  - 现有 `metrics.dpi.max(96) as f32 / 96.0` 就是 `scale`，与代码里其它 DIP 常量
    （`INFO_PADDING_V_DIP` 等）的乘 scale 模式一致。
  - `std_dev = 12.0 * scale`：12 是"96 DPI 下期望的模糊半径"，`* scale` 换算到
    实际物理像素。高 DPI 屏上模糊带会等比变宽，视觉上模糊强度不变。
  - 别把 `std_dev` 当"逻辑半径"再乘 scale 两次——只走上面这个式子。

**Mode B 不采纳**（屏幕 DPI 交给 D2D 自己换算，会与现有 `to_d2d(rect)`
物理像素约定冲突）。

---

## 五、 视觉与质感的可选项（v1 不做，先跑主链路）

以下两段是"锦上添花"，不解决"能不能跑"的问题。**v1 只做 §三 的 blur + tint，
先真机目视**；如仍有亮度断层或 banding 再引入。

1. **ColorMatrix 亮度对齐**：L1 遮罩把桌面压暗，直接 blur 原帧（L0）会让信息栏
   像"发光的窗户"。可串一个 `CLSID_D2D1ColorMatrix` 到 blur 之后，将 RGB 乘 0.6
   对齐 L1。触发时机：目视发现面板明显比周围亮。
2. **噪点纹理**：128×128 灰度噪点 bitmap，`D2D1_COMPOSITE_MODE_MULTIPLY` 或
   极低 alpha 的 `SOURCE_OVER` 平铺在 tint 之上，破渐变带。触发时机：真机上
   看到明显 color banding。

---

## 六、 性能与降级

- **GPU**：`DrawImage` + 源矩形把 effect 求值范围限制在 info 矩形 + kernel 半径
  邻域（约 300×80 + 边缘若干像素），不是整帧 fragment shader。
- **显存**：不引入独立全帧 RT；layer 中间 surface + effect 输出 surface 是
  有界小尺寸，不是"零额外"。
- **降级 (v1 不实现)**：D2D GaussianBlur 从 D2D 1.1（Win7 SP1）起就有，SnapClip
  已经假定 DXGI flip-model + DirectComposition（Win8.1+），**当前无实际触发场景**。
  §三.4 的 else 分支保底"退回旧 `FillRectangle(bg)`"即可，不需要额外实现
  "降采样 + `HIGH_QUALITY_CUBIC` 上采样"这类近似方案。

---

## 七、 最终落地 Checklist

- [ ] **Cargo.toml**：**不需要加新 feature**。`Win32_Graphics_Direct2D` + `Win32_Graphics_Direct2D_Common` 已覆盖本方案所有符号；`Win32_System_Com` 保留给别处但本方案不用。
- [ ] **OverlayRenderer 结构**：新增 `blur_effect: Option<ID2D1Effect>`、`info_tint_brush: Option<ID2D1SolidColorBrush>`；`new()` 里初始化为 `None`；`recreate_resources()` 里创建并 `Some(..)`。
- [ ] **ChromeResources 结构**：同步加两个字段（`Option<ID2D1Effect>` / `Option<ID2D1SolidColorBrush>`），`draw_layers` 里 clone 传入。
- [ ] **Helper**：`trait BlurProperty` 或两个内联 `SetValue` 调用（`&[..to_ne_bytes()]` + `D2D1_PROPERTY_TYPE_UNKNOWN`），**不引入 PROPVARIANT / InitPropVariantFrom* helper**。
- [ ] **绘制替换**：在 `d2d.rs` 中原 `FillRectangle(&to_d2d(info), &bg)` 那一行（PushLayer 内、info panel 起始）替换为 §三.4 的 `SetInput + SetValue + DrawImage + FillRectangle(tint)` 块，带 `else` 分支 fallback。
- [ ] **`SetInput` 无 `?`**：`blur.SetInput(0, &fb_img, true);`（返回 `()`，不是 Result）。
- [ ] **`DrawImage` 指针参数 + 无 Result**：`Some(&dest as *const Vector2)` 和 `Some(&src as *const D2D1_RECT_F)`；`D2D1_RECT_F` 完整路径 `windows::Win32::Graphics::Direct2D::Common::D2D1_RECT_F`；windows-rs 0.61 中该方法返回 `()`，**不要** 写 `.map_err(...)?`。
- [ ] **`info_bg_brush` alpha 从 0.88 降到 0.68**（作为 fallback 也不至于过黑）。
- [ ] **真机验证**：100% / 125% / 150% DPI 下分别截图，检查
  - 模糊边缘是否自然、圆角是否被 PushLayer 完美裁掉；
  - 信息栏内文字对比度是否够（必要时微调 tint alpha，不要调 std_dev）；
  - 桌面变暗时信息栏是否仍像"发光窗户"（若是 → §五.1 引入 ColorMatrix）。
