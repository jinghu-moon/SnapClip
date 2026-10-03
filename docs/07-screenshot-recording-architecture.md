# SnapClip 截图、标注、长截图与录屏架构调研

> 调研范围：Windows 10/11，当前 SnapClip（Tauri + Vue + Rust）以及 `D:\100_Projects\110_Daily\SnapClip-old`。
> 结论先行：WGC/DXGI、窗口生命周期、捕获调度和编码应由 Rust/Win32 后端负责；前端负责工具栏、属性和应用界面，不应成为高频像素管线。

## 现状与老项目结论

当前 SnapClip 的 `src-tauri` 已有 Windows 剪贴板监听、OCR 和历史存储，但没有实际的屏幕捕获实现。`snapclip-old` 已实现一个可用的 SDR MVP，主要证据如下：

| 能力 | 老项目实现/设计 | 可复用结论 |
| --- | --- | --- |
| 捕获 | `crates/snapclip-win32/src/capture.rs` 使用 `BitBlt(SRCCOPY \| CAPTUREBLT)` | 只能作为 SDR 降级路径，不能作为高帧率录屏主路径 |
| 覆盖层 | `composition.rs`、`overlay_render.rs`、`d2d_runtime.rs` 使用 D2D/DirectComposition | 截图框应由原生 Win32 窗口承载 |
| 标注 | `annotation.rs` 保存对象模型，`composition.rs` 将整张 BGRA 图复制后 CPU 光栅化 | 适合静态截图；鼠标移动和录屏不能每帧复制/重绘整张 4K 图 |
| 工具栏 | `gpui_host.rs`/`toolbar.rs` 通过 `PostMessageW(WM_APP+n)` 给原生层发命令 | 当前项目可用 Tauri 工具栏替代，但仍保持异步命令边界 |
| 长截图 | `docs/04-scroll-stitching.md` 已定义位移匹配、`position/max_depth`、重叠覆盖和尺寸上限 | 算法状态机可直接迁移，匹配器应先采用纯 Rust 垂直相关 |
| 录屏 | 代码和文档中没有 Media Foundation、硬件编码器或录屏模块 | 录屏是新能力，不能从现有截图函数简单循环调用 |

## 1. 捕获归属：Rust 调用 WGC/DXGI

是。正确边界是：

```text
Tauri/Vue
  -> 截图/录屏命令（小消息，不传帧）
Rust capture service
  -> Windows.Graphics.Capture（WGC）或 DXGI Desktop Duplication
  -> D3D11 texture / frame metadata
Rust compositor/encoder
  -> DirectComposition 预览或 Media Foundation 编码
```

Rust 通过 `windows`/`windows-sys` 调用 WinRT、D3D11、DXGI 和 Media Foundation。Vue 不应直接访问 WGC，也不应接收每帧 Base64；这会产生 IPC、内存复制和 WebView GC 压力。

### 后端选择

| 场景 | 首选 | 原因 |
| --- | --- | --- |
| 捕获指定窗口、被遮挡窗口、持续预览 | WGC `GraphicsCaptureItem` + `Direct3D11CaptureFramePool` | 面向窗口/显示器，输出 GPU 帧，能处理 DWM 合成 |
| 全屏高帧率录屏 | DXGI `IDXGIOutputDuplication` | `AcquireNextFrame` 提供高频桌面帧和 dirty/move rect 元数据 |
| 静态 SDR 降级 | `BitBlt` | 依赖少，但不保证分层窗口、HDR 和受保护内容 |

如果继续支持 Windows 10 1809，不能无条件调用 WGC 的 `CreateForWindow`/`CreateForMonitor` 互操作入口；这些入口从 Windows 10 1903 起才可用。启动时应按系统版本和运行时能力探测选择 WGC、DXGI、Magnification 或 BitBlt，并把实际 provider 写入诊断日志。

WGC 和 DXGI 不应强行统一成“永远返回 CPU 图片”的接口。建议定义两个层次：

```rust
struct CaptureFrame {
    texture: ID3D11Texture2D,
    timestamp_qpc: i64,
    source_size: PhysicalSize,
    dirty_rects: Option<SmallVec<[Rect; 8]>>, // DXGI 可提供；WGC 通常为 None
    cursor: Option<CursorState>,
}

trait CaptureSource {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<CaptureFrame>, CaptureError>;
}
```

只有保存 PNG、OCR 或无法使用硬件编码器时，才显式执行 GPU 到 CPU 的读回。

## 2. 窗口管理：截图框与工具栏

### 2.1 截图框

截图框属于 Rust/Win32 原生窗口，不属于 Vue 页面。建议每个目标显示器创建一个无边框覆盖层：

- `WS_POPUP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST`；DirectComposition 路径使用 `WS_EX_NOREDIRECTIONBITMAP`。
- 进程声明 PerMonitorV2 DPI 感知；窗口位置、捕获裁剪、标注坐标全部使用物理像素。
- 热键唤起时先捕获底图，再显示覆盖层。静态截图流程在显示覆盖层后不再采集，因此 WGC/DXGI 不会把自己的工具栏和截图框带入这次截图。
- 覆盖层预创建并隐藏，唤起时只更新尺寸、纹理和状态；不要每次截图重新创建 D3D11 device、swap chain 和窗口。
- 覆盖层只处理选择、拖拽、缩放、命中和快捷键；它不执行 PNG 编码、磁盘写入或 OCR。

状态机至少包含：`Idle -> Armed -> Selecting -> Annotating -> Exporting`，录屏另走 `Recording` 状态，不能用截图状态中的同步保存逻辑阻塞录屏。

### 2.2 工具栏

工具栏可以使用 Tauri 的独立无边框窗口，但它只发送命令和显示状态：

```text
Vue toolbar -> bounded command queue -> PostMessage/Win32 host -> native overlay
native overlay -> coalesced state event -> Vue toolbar
```

工具栏窗口使用 `always-on-top`、`skip-taskbar`、`decorations=false` 和 `transparent` 等配置；命令必须有界、可合并，不能在鼠标移动事件中同步调用 Vue 或等待 Rust 返回图像。工具栏失焦后应把键盘焦点还给覆盖层，Esc/Enter 等快捷键才不会丢失。

不建议做一个覆盖整个桌面的 WebView 截图框：透明 WebView 的合成、DPI 换算、焦点和 4K 图像传输都会增加延迟和内存，而且无法自然复用 D2D/DirectComposition 的 GPU 纹理。

### 2.3 原生覆盖层如何保持视觉质量

原生窗口不等于使用老式 GDI 控件。外观应由 Direct2D/DirectWrite 和 DirectComposition 负责，工具栏再使用 Tauri/Vue 的 Fluent 风格组件：

```text
L0  D3D11 截图纹理
L1  一个全屏半透明暗色遮罩（选区区域挖空或直接显示原图）
L2  选区边框、四角手柄、尺寸标签、放大镜（D2D 脏矩形）
L3  Tauri 工具栏/属性面板（小窗口，不参与像素合成）
```

- 选区外使用一次 GPU 半透明填充，不逐像素修改原图；选区边框使用 1–2 个物理像素的高对比强调色，并提供明暗主题和 Windows 高对比度回退。
- 手柄、边框、尺寸标签按 DPI 缩放，手柄命中区域大于视觉区域；拖拽时给旧矩形与新矩形的并集加边距做失效，避免残影和闪烁。
- 线、圆角矩形、箭头和文字使用 Direct2D/DirectWrite 抗锯齿；阴影使用 DirectComposition/DWM 的轻量效果，不在全屏覆盖层使用 `backdrop-filter` 或逐帧模糊。
- 工具栏采用固定的 8px 间距基线、8px 左右圆角、32–36px 控件高度和 Segoe UI/Segoe Fluent Icons；按钮只显示熟悉图标，悬停时提供 tooltip，颜色/线宽使用真正的色板和步进控件。
- 工具栏位置根据选区自动选择下方或上方，并限制在当前显示器工作区；移动/淡入动画控制在约 120–180ms，录制和拖拽期间不做持续动画。
- 主题颜色从系统主题/强调色初始化，工具栏提供可读的 hover、pressed、focus ring 状态；Win10 不支持系统材质时回退到不透明或半透明纯色面板，不能因为材质失败而让窗口变黑。
- 文本编辑使用原生 EDIT/系统 IME 或临时 Tauri 输入窗口，提交后转成 DirectWrite 文本对象；不要让 WebView Canvas 成为标注的最终光栅源。

老项目的 `D2dWindow`、脏矩形和工具栏定位逻辑可复用其交互经验，但应把整图 CPU 合成替换为“底图纹理 + 遮罩 + 矢量覆盖层”。视觉验收除了截图对比，还要检查 100/125/150/200% DPI、浅色/深色主题、HDR、低对比度背景和高对比度模式。

## 3. 标注实现：模型由 Rust 持有，预览分层绘制

推荐采用“Rust 文档模型 + 原生覆盖层绘制 + 前端工具栏”的混合方案：

```text
L0 原始捕获纹理（只读）
L1 已提交标注（D2D 矢量或 GPU overlay texture）
L2 当前拖拽对象（只更新 dirty rect）
L3 选区框、手柄、尺寸提示、工具栏（永不导出）
```

`AnnotationDocument`、撤销栈、几何命中和导出规则放在 Rust；Vue 只编辑颜色、线宽、工具和文字属性。复制/保存时由同一个 `Painter` 重放文档，确保屏幕和导出一致。

老项目目前的 `compose_overlay_image()` 会克隆整张 BGRA 图再 CPU 绘制。这个实现应保留为静态导出的兜底，不应作为录屏帧路径。目标实现是：

1. 底图保持 GPU texture，不因鼠标移动复制整帧。
2. 标注对象按 dirty rectangle 重绘；对象少时可保留矢量重放。
3. 马赛克/遮挡只处理对象覆盖的区域；导出时再对最终 crop 做一次确定性合成。
4. 文本编辑使用原生 EDIT/系统 IME 或专用输入窗口，提交后转为不可变文本对象。

如果短期必须使用 CPU 合成，至少采用 tile cache（例如 256×256）和对象版本号，只重算受影响 tile；禁止每个 `WM_MOUSEMOVE` 分配一张完整 4K `BgraImage`。

## 4. 长截图：先判定位移，再写入重叠区域

长截图不是“固定裁剪后垂直拼接”。核心是可靠估计相邻帧的实际垂直位移。

### 4.1 推荐流程

```text
锁定目标窗口/区域
  -> 发送带重叠的 PageDown 或滚轮输入
  -> 等待画面稳定（连续帧差低于阈值）
  -> 捕获下一帧
  -> 多条横带做 NCC/归一化相关，投票得到 shift
  -> 置信度和边缘一致性检查
  -> 仅追加有效增长，覆盖重叠区
  -> 到底/失败/尺寸上限/用户取消
```

M4 首版建议使用纯 Rust 多带垂直相关：在上一帧底部、中央和顶部取横带，在下一帧限定搜索窗口计算 NCC；三条带的位移必须一致才提交。动态页面导致相关失败时，再评估 ORB/AKAZE；不要一开始引入 OpenCV 作为整个应用依赖。

### 4.2 防止重叠、空白和错误终止

- 使用 `position` 和 `max_depth` 两个计数器，分别表示当前帧位置和历史最深位置；向上滚动时前置画布，向下滚动只在突破 `max_depth` 时增长。
- `shift` 超过帧高的 60%、多带结果不一致或置信度不足时，标记 `rejected`，不能把失败伪装成 `shift=0`。
- 重叠区由新帧覆盖旧帧；新帧只写入 `[position, position + frame_height)`，不填充猜测的白色区域。
- 连续 3 帧边缘相同或无有效增长才判定到达边界；单帧未变化可能只是滚动动画或懒加载尚未完成。
- 每次提交前检查条带边缘、画布高度和像素面积上限；失败时保留已拼出的部分并报告原因，不生成看似完整但含空白的图片。
- 采集和匹配之间使用容量为 1 的 mailbox，始终处理最新帧，避免长截图期间无界堆积 BGRA 图像。

长截图的单元测试应使用程序生成的已知位移帧，覆盖重复帧、上下往返、动态噪声、位移拒绝、重叠覆盖和高度上限。

## 5. 录屏设计：GPU 捕获到硬件编码

### 5.1 不能复用截图保存循环

4K/60 的 BGRA 原始数据量约为：

```text
3840 × 2160 × 4 × 60 ≈ 1.99 GB/s
```

每帧 `Map` 到 CPU、转 PNG、再写磁盘必然造成 CPU、内存带宽和延迟问题。录屏主路径必须保持 D3D11 texture：

```text
WGC/DXGI frame pool
  -> bounded GPU texture ring (3–8 slots)
  -> crop/scale/annotation composite on GPU
  -> NV12 conversion if encoder requires it
  -> Media Foundation H.264/HEVC hardware MFT or Sink Writer
  -> MP4 file
```

捕获、合成和编码至少拆成三个阶段。队列必须有界；编码落后时丢帧并保留原始时间戳，不能阻塞捕获线程、鼠标输入线程或 Vue 工具栏。

### 5.2 鼠标、标注和切换软件的流畅性

- WGC 使用 `Direct3D11CaptureFramePool.CreateFreeThreaded` 或独立捕获线程，DXGI 使用 `AcquireNextFrame` 的超时等待；不要在 UI 线程等待下一帧。
- 捕获帧复制到自有纹理后立即释放 WGC frame；纹理池固定上限，避免帧对象和 GPU 资源无限增长。
- 光标可由 WGC 配置捕获；DXGI 则读取 pointer shape/position，在 GPU 合成阶段叠加。光标叠加不能走 CPU PNG 路径。
- 标注作为独立 overlay texture 按 dirty rect 更新；每个视频帧只做 GPU 合成，不重绘整张 CPU 位图。
- Tauri 工具栏只发送开始/暂停/停止、区域、FPS、编码器等小命令；录制线程不等待前端响应。
- 录制线程和编码线程使用有界 ring buffer、QPC/帧时间戳和丢帧统计。UI 流畅优先于“绝不丢帧”；必须在状态栏显示实际 FPS、编码队列深度和丢帧数。
- 不要把整个进程提升为高优先级。需要时只给捕获/编码线程注册 MMCSS，避免抢占输入和桌面合成线程，并通过基准验证功耗和稳定性。
- 录制前锁定目标源和裁剪矩形；切换窗口时应停止当前 source，等待新 source 首帧，再切换时间线，不能在同一个编码器中混入尺寸变化的纹理。

### 5.3 MP4 与 GIF

- MP4/H.264 是首要目标：Media Foundation Sink Writer 负责封装，优先选择系统可用的硬件 H.264 MFT；HEVC 作为可选编码器，必须检测编码器和系统能力。
- GIF 没有同等的硬件编码路径。默认录制为临时 MP4 或受控帧序列，停止后在后台做调色板量化和 GIF 编码；GIF 应限制为短时、低 FPS、较低分辨率（例如 15–20 FPS、长边 1280），避免录制过程中卡顿。
- GIF 导出失败或耗时较长不能影响录制结果；先保证 MP4/PNG 序列安全落盘，再异步生成 GIF。
- 音频不应与屏幕帧混在一个时钟里临时拼接。后续增加系统声音/麦克风时，使用 WASAPI loopback/输入流和 Media Foundation 时间戳，单独做 A/V 同步验收。

## 6. 当前项目建议的模块边界

```text
src-tauri/src/capture/
  mod.rs                 CaptureSource、CaptureFrame、能力探测
  windows/wgc.rs         GraphicsCaptureItem、FramePool、D3D11 纹理
  windows/duplication.rs IDXGIOutputDuplication、dirty/move rect
  windows/bitblt.rs      SDR 降级和静态截图

src-tauri/src/overlay/
  window.rs              HWND、DPI、z-order、命中和状态机
  render.rs              DirectComposition/D2D/GPU overlay
  document.rs            标注对象、撤销和导出重放

src-tauri/src/stitch/
  driver.rs              滚动输入、稳定等待和终止条件
  matcher.rs             多带 NCC；未来可插拔 ORB
  canvas.rs              position/max_depth、重叠覆盖和上限

src-tauri/src/recording/
  session.rs             录制状态和时间线
  ring.rs                有界 GPU frame ring
  composite.rs           crop/scale/cursor/annotation
  media_foundation.rs    Sink Writer、MFT、MP4
  gif.rs                 停止后的后台 GIF 转码
```

前端只保留 `capture/start`、`capture/finish`、`record/start`、`record/stop`、工具和属性命令；不要为每帧定义 Tauri event。

## 7. 实施顺序与验收指标

1. 先实现 Rust `BitBlt` 静态截图和原生覆盖层，验证物理像素、DPI、z-order、Esc/Enter、复制/保存闭环。
2. 将老项目的 `Document`/`position-max_depth` 逻辑迁移为纯 Rust 单测，不先接入 OpenCV。
3. 接入 WGC 窗口/显示器帧，保留 BitBlt 降级；记录 source、像素格式、GPU/CPU 复制次数。
4. 接入 D3D11 texture ring 和 Media Foundation H.264；完成 MP4 后再做 GIF 后处理。
5. 最后加入 GPU 标注合成、光标策略、音频和编码器自适应。

每个阶段记录：热键到首帧 P50/P95、选择输入延迟、录制实际 FPS/P95 帧间隔、丢帧率、GPU 利用率、CPU 时间、Private Bytes/Working Set、编码队列深度和停止后文件完整性。4K60、双显示器混合 DPI、HDR、窗口切换、浏览器懒加载/动态页面和设备移除必须分别验收。

## 8. 标注质量与可再次编辑

### 8.1 抗锯齿

标注不要先画到低分辨率 WebView Canvas 再放大。Rust 侧保存几何对象，原生渲染器使用 Direct2D/DirectWrite：

- 线、矩形、椭圆、贝塞尔路径和箭头使用 D2D geometry；开启 `D2D1_ANTIALIAS_MODE_PER_PRIMITIVE`，文字使用 DirectWrite 的文本抗锯齿。
- 坐标统一在捕获图像物理像素中，DPI 变换只发生在窗口输入和工具栏布局；不要对已经光栅化的标注反复缩放。
- 细线根据变换后的像素边界做像素对齐；不能无条件给所有线加 `0.5`，否则在不同 DPI/缩放下会出现另一种模糊。
- 屏幕预览和 PNG 导出共用同一 `Painter`/D2D 绘制规则。只有 D2D 无法表达的特殊效果才使用 2x/4x 离屏超采样，并且只在最终导出时缩小一次。
- 实心遮挡的边缘不应抗锯齿到透明背景；马赛克只处理对象内部，避免边缘半透明泄露原图。

老项目 `composition.rs` 目前复制整张 BGRA 后用 CPU 绘制，能保证结果一致但会有锯齿和高分辨率重绘成本。D2D 矢量化应先替换形状、箭头和文字，CPU 路径保留为导出降级和单元测试参考。

### 8.2 样式模型与再次编辑

标注必须是对象，而不是“画完就合并”的像素：

```rust
struct Shape {
    id: ShapeId,
    kind: ShapeKind,
    geometry: Geometry,
    style: Style,
}

struct Style {
    stroke: Color,
    fill: Option<Color>,
    width: f32,
    opacity: f32,
    cap: LineCap,
    join: LineJoin,
    dash: DashPattern,
    font: Option<FontStyle>,
    arrow: Option<ArrowStyle>,
    mosaic_block: Option<u32>,
}
```

同一个截图框内存在多个对象时：

1. 鼠标按下先按 z-order 从上到下做几何命中，命中对象即设置 `selected`；未命中才开始新建选区/对象。
2. 选中对象显示边界框和八个手柄；拖动对象、改变几何或调整样式都只修改该对象的 `id`。
3. 工具栏分开保存“新建对象默认样式”和“当前选中对象样式”。修改颜色、线宽、填充或字号时发出 `UpdateStyle { ids, style }`，不会影响其他对象。
4. 支持 `selected_ids` 多选和组合边界框；单选先落地，多选作为后续能力，不要用复制像素的方式实现。
5. 一次拖拽、一次样式变更、一次删除分别生成一个撤销事务；撤销恢复对象模型后按 dirty rectangle 重绘。

文字对象应保留文本、字体、字号、基线和颜色；双击已存在文字重新进入系统输入控件。马赛克、荧光笔等依赖底图的效果需要保存对象顺序，重放时从原始底图重新计算，不能把当前结果作为新的底图。

## 9. 长截图扩展为横向和双向

不能把 `height`、`top`、`bottom` 写死在拼接器里。将老项目的垂直状态机泛化为：

```rust
enum ScrollAxis { Vertical, Horizontal }

struct StitchPosition {
    axis: ScrollAxis,
    position: i32,
    max_depth: i32,
}
```

- 纵向滚动取横向条带，在 Y 轴搜索位移；横向滚动取纵向条带，在 X 轴搜索位移。
- `position/max_depth` 仍只记录主轴；画布扩张、重叠覆盖、60% 位移拒绝、连续 3 帧无变化终止规则完全复用。
- 横向滚动优先尝试目标窗口的水平滚动条、`Shift+Wheel` 或左右方向键；这些行为在浏览器、自绘控件和远程桌面中并不统一，必须提供“用户手动滚动、按快捷键采样”的半自动模式。
- 横向和纵向都使用多带 NCC 投票；动态内容、固定侧栏和滚动条应从匹配区域裁掉。横向拼接还要排除右侧悬浮按钮和固定广告栏。
- 对角滚动或网页同时发生 X/Y 位移时，首版应拒绝并提示改用单轴模式；不要把两个轴的失败误拼成空白画布。后续可用二维平移估计单独实现。

建议把 `matcher.rs` 接口改为 `estimate_shift(frame_a, frame_b, axis, search_range)`，把 `canvas.rs` 的 `append/prepend` 改为主轴通用操作，并为水平、垂直、上下/左右往返分别建立合成帧单测。

## 10. 录屏库选择

### 10.1 推荐：Windows Media Foundation + Rust Windows bindings

本项目只运行 Windows，首选直接调用 Media Foundation Sink Writer 和系统 H.264/HEVC MFT：

```text
WGC/DXGI D3D11 texture
  -> GPU crop/scale/annotation composite
  -> NV12（编码器要求时）
  -> Media Foundation hardware MFT
  -> IMF* Sink Writer -> MP4
```

优点是系统自带、可使用硬件编码、能通过 D3D11 device manager 共享 GPU 设备，避免 FFmpeg 进程和额外的 CPU/GPU 拷贝。Rust 侧使用 `windows` crate 生成 COM/WinRT bindings，录制服务只在 Windows target 编译。

| 方案 | 结论 | 适用范围 |
| --- | --- | --- |
| Media Foundation Sink Writer/MFT | **主路径** | Windows MP4/H.264/HEVC、硬件编码、低 CPU |
| FFmpeg/libav | 可选后端 | 需要更多编码器、GIF/兼容格式或离线转码；增加 DLL、打包和许可证复杂度 |
| `mpv`/libmpv | 不采用 | 主要是播放器，不是录屏采集和编码管线 |
| `gif`/WIC GIF 编码器 | 后处理 | 录制结束后低 FPS、低分辨率 GIF；不能作为 4K 实时编码器 |

FFmpeg 不应作为录制主路径的第一个实现。若以后必须支持 VP9/AV1、复杂滤镜或跨平台导出，再将 FFmpeg 作为独立可选转换器，而不是让 UI 线程启动 `ffmpeg.exe` 处理每帧。

### 10.2 编码器能力探测

启动录制前枚举系统 MFT/编码器，记录实际编码器、输入像素格式、硬件/软件模式和 profile。硬件编码器不可用时：

1. 先降低 FPS/分辨率或码率；
2. 仍无法满足时回退软件 H.264；
3. UI 明确显示“软件编码/可能丢帧”，不能静默阻塞捕获线程。

## 11. Tauri/WebView2 CPU 与内存控制

WebView2 是多进程浏览器内核；每个 WebView 会带来 browser、renderer、GPU 等进程。Tauri IPC 的 command/event 参数和返回值还要经过 JSON/消息序列化。因此截图和录屏的高频路径必须完全绕过 WebView。

### 11.1 当前代码的直接热点

| 位置 | 问题 | 修复方向 |
| --- | --- | --- |
| `src-tauri/src/lib.rs:45-52` | 每张完整 PNG 读取、BLAKE3、Base64、JSON/IPC、WebView 解码 | 列表只返回缩略图；使用受限 asset/custom protocol URL，完整图仅在详情页加载 |
| `src/App.vue:87-92` | 剪贴板和 OCR 每次事件都清空并重新查询整页 | 事件 50–100 ms 合并；剪贴板增量插入；OCR 按 `clipId` 更新 |
| `src/components/HistoryItem.vue:124` | `payloads` 深度监听造成图片重复请求 | 只监听图片 `contentHash` |
| `src-tauri/src/icon.rs:14-69` | 图标 data URL 无上限缓存 | 有上限 LRU，或返回受限资源 URL |

### 11.2 WebView2/Tauri 设计规则

- 保持一个主 WebView；截图框、录屏帧预览和高频指针层使用原生窗口/DirectComposition，不创建全屏 WebView。
- 多个 Tauri WebView 必须共享同一个 WebView2 environment/user-data folder；能复用窗口就不要反复销毁/创建。
- WebView 隐藏时停止轮询、事件订阅和动画；如果 Tauri 暴露底层 `CoreWebView2`，非活动页面可使用 `MemoryUsageTargetLevel.Low`/`TrySuspendAsync`，恢复时再启用。
- 不要设置 `--disable-gpu`；WebView2 官方建议保留 GPU 合成。避免大面积 `backdrop-filter`、持续 CSS 阴影和频繁布局读写。
- 使用虚拟列表、`content-visibility`/CSS containment、事件 debounce/throttle 和 `requestAnimationFrame` 合并指针更新。当前虚拟列表方向正确，但刷新和深度 watcher 仍需修复。
- 不传输 Base64 大图或视频帧。使用严格 scope 的 Tauri asset/custom protocol，按 content hash 返回缩略图；不要为了方便开放任意本地路径。
- 生产构建关闭 DevTools/HMR，使用 Evergreen WebView2 Runtime；资源和脚本按页面功能懒加载。
- 前端与 Rust 的 IPC 只传小型 command/state；批量发送、合并同类事件，避免每个 OCR 状态或鼠标移动都触发一次 JSON RPC。
- 将 IPC 分为控制面和数据面：`invoke`/event/`Channel<T>` 只传 typed command、领域 ID、状态和进度；缩略图/完整图片通过受限 content-hash asset protocol 流式读取。命名管道只承载 OCR/截图/录屏控制与小型结果，不承载逐帧像素。
- `clipboard.updated` 和 `ocr.updated` 必须携带 `schemaVersion`、`sequence`/`generation`、`clipId`；前端按 ID 增量更新，不能收到通知就清空列表并重新查询。
- 所有高频生产者使用有界队列和 latest-wins 合并；搜索、详情和录制状态使用 generation/取消标记，过期结果在 Rust 侧停止序列化。录制状态摘要限制在 5–10 Hz，鼠标移动和视频帧完全留在原生窗口/录制进程。
- 事件不是可靠队列：窗口隐藏或 WebView 恢复后先取轻量快照，再按序号订阅增量；多窗口使用 `emit_to` 定向发送，避免无关 WebView 被唤醒。

### 11.3 验收方式

分别测量“主窗口静置”“窗口隐藏”“截图框可见”“录屏中”四种状态：

- WebView browser/renderer/GPU 子进程 Private Bytes、Working Set 和 CPU 时间；
- Rust 进程 CPU、GPU dedicated/shared memory；
- IPC 次数、平均 payload 字节数、每秒事件数；
- 图片缩略图加载峰值与完整图详情加载峰值；
- 录屏实际 FPS、丢帧率和编码队列深度。

使用 WebView2 Browser Task Manager、Edge DevTools Performance/Memory、Windows Performance Recorder/Analyzer 和 Process Explorer 采集，不以任务管理器单个总内存数字作为结论。

## 12. 磁盘优先的图片缓存策略

“图片放硬盘、不放内存”方向正确，但当前 BlobStore 已经把原图保存到 `payloads/<prefix>/<blake3>.blob`；真正的问题是读取链路仍然会把完整文件读成 `Vec<u8>`，再做 BLAKE3 校验、Base64 编码和 WebView 解码。因此目标不是完全不使用内存，而是让每一层的驻留都有上限：

```text
原图：内容寻址文件，硬盘为主，按历史保留策略清理
缩略图：Local AppData/thumbnails/<hash>.webp，硬盘为主，列表只加载可见项
解码缓存：有界 LRU，只保留当前选中项和少量可见项
GPU 纹理：当前覆盖层/录屏的 texture ring，有固定槽位
WebView：接收受限资源 URL，不接收大图 Base64/JSON
```

### 12.1 读取与校验

- 列表 DTO 只返回缩略图 URL、尺寸和 hash；完整原图只在详情、复制或保存时读取。
- `read_payload_bytes()` 应从唯一写线程拆出专用只读读取服务；元数据存在性查询使用复用的只读连接，Blob 文件按 content hash 直接打开。
- 新写入时已经校验输入 hash，读取路径不必每次都完整重哈希；改为启动/低频后台审计，或提供显式 `verify=true` 的诊断路径。完整读取仍需限制最大尺寸，防止恶意或损坏文件造成内存峰值。
- 不要为了“零拷贝”默认使用 memory-mapped 文件。映射文件仍可能进入 Working Set，且大图随机访问会造成页错误；普通文件读取配合 Windows 系统文件缓存和有界缓冲更容易测量。
- 不要使用 `FILE_FLAG_NO_BUFFERING` 或主动裁剪进程 Working Set，除非基准证明收益；绕过系统缓存可能增加磁盘延迟和 CPU 复制。

### 12.2 缩略图和淘汰

- 入库后低优先级生成长边 256–320 的缩略图；列表禁止请求原始 PNG。截图文字优先使用无损 WebP/PNG，不能为节省磁盘引入明显 JPEG 伪影。
- 原图由历史保留策略管理，缩略图由磁盘 LRU/总容量上限管理；删除前检查 SQLite 引用，启动时清理孤儿缩略图和临时文件。
- 解码后的内存 LRU 必须以字节数而不是条目数限制，例如初始预算 32–64 MB；超过预算立即淘汰未选中、不可见和最久未使用的对象。
- 录屏和截图的 GPU texture ring 单独限额，不能复用历史图片缓存，也不能让 WebView 缓存整段视频。

### 12.3 这项优化的边界

磁盘驻留降低的是应用主动持有的 Private Bytes，不等于物理内存完全不增加：Windows 文件缓存、WebView 图片解码器和 GPU 纹理仍会占用 Working Set/显存。验收必须同时记录磁盘 I/O、Private Bytes、Working Set、GPU memory、首张缩略图延迟和详情首帧延迟；只有在这些指标的组合结果更好时，才扩大缓存或提高内存预算。

## 13. 常驻进程策略

### 13.1 先区分“按需能力”和“独立进程”

“用户没有启用”首先意味着不创建对应的线程、模型、GPU 资源和窗口；是否再放到独立进程，是第二个决策。独立进程有崩溃隔离和更清晰的资源边界，但会增加启动延迟、IPC、安装包和诊断复杂度，不能把每个模块都机械拆出去。

当前目标的常驻/按需策略如下：

| 组件 | 是否独立进程 | 生命周期 |
| --- | --- | --- |
| 剪贴板监听、托盘、SQLite/BlobStore | 首期与主进程同进程；后续可成为 Agent | 托盘模式常驻，显式退出才结束 |
| Tauri/Vue 主窗口 | 否；WebView2 由 Tauri 管理 | 按需显示；关闭 UI 不应自动启动 OCR/录屏 |
| OCR | **按需 `SnapClipOcr.exe`**（首版也可先做进程内 lazy worker） | 首次 OCR/批处理时启动；空闲 30–60 秒退出 |
| 录屏捕获和编码 | **按需 `SnapClipRecorder.exe`** | 点击录制时启动；停止、设备移除或错误后退出 |
| 原生截图框、工具栏、贴图窗口 | 首期与主 Rust/Agent 同进程；可整体移入 `SnapClipCapture.exe` | 截图会话开始时显示，完成导出/取消后退出或释放 |
| 截图 WGC/DXGI、D2D、DirectComposition | 与截图框保持同一进程 | 截图会话内初始化；不要跨进程传递每帧纹理 |
| 录屏 WGC/DXGI、D3D11、Media Foundation | 与 `SnapClipRecorder.exe` 同进程 | 录制进程内保持 GPU texture 零拷贝；结束后释放 |
| WebView2 browser/renderer/GPU 子进程 | 由 WebView2 自动管理 | 主 UI WebView 存在时可能常驻；不能当作业务 worker 管理 |

当前代码的 `src-tauri/src/lib.rs` 在 `setup` 中无条件调用 `OcrService::start`，与“未启用不启动 OCR”的目标不一致；实现时应改为 `OcrRuntime`，默认只有 `None`/关闭状态，收到首次 OCR 请求后才启动 worker 或子进程。剪贴板监听在 OCR 关闭时传入 `None`，不得为了保留 API 而启动空转 worker。

`SnapClipRecorder.exe` 不应每帧启动 `ffmpeg.exe`。它自己持有 WGC/DXGI、D3D11 texture ring 和 Media Foundation 编码器，主进程只通过命名管道发送开始/停止、区域、FPS 和标注对象更新。GIF/转码是录制结束后的短生命周期任务，不占用录制过程的捕获线程。

### 13.2 为什么 OCR 可以拆，录屏必须特殊处理

| 能力 | 推荐方案 | 原因 |
| --- | --- | --- |
| OCR | 按需独立进程优先；模型较小且需要快速迭代时可先做进程内 lazy worker | OCR 不要求逐帧低延迟；模型/ONNX Runtime/DirectML 可能占用大量 Private Bytes，进程隔离能回收内存并隔离崩溃。输入只传内容寻址文件路径和 hash，结果传小型 JSON，不传整张图片 |
| 录屏 | 按需独立录制进程，捕获和编码必须在同一进程 | WGC/DXGI 返回的 D3D11 texture 要在同一设备上完成合成和硬件编码。若主进程捕获、子进程编码，跨进程共享句柄和同步容易退化为 GPU/CPU 拷贝，直接损害高分辨率高帧率流畅性 |

因此“可选进程”不是把录屏拆成 `capture.exe`、`encode.exe` 两段，而是启动一个拥有完整 GPU 管线的 `SnapClipRecorder.exe`。主进程只传控制消息和矢量标注，不能传每帧像素。

### 13.3 截图进程的选择

静态截图不需要录屏的持续高帧率管线。截图框命中、鼠标拖拽、滚动输入、标注和 WGC/DXGI 设备需要共享 HWND、焦点和 GPU 资源，因此它们应作为一个截图会话单元：

| 部署形态 | 截图进程策略 | 适用条件 |
| --- | --- | --- |
| 首期单 `SnapClip.exe` | 截图会话在主 Rust 进程内按需创建原生窗口和 D3D/D2D 对象 | 启动延迟最低，代码和错误恢复最简单；会话结束后释放捕获对象 |
| `SnapClipAgent.exe` + 按需 UI | 截图会话放在常驻 Agent 内，UI 只发送小型命令 | UI/WebView 可关闭，但托盘、热键和截图必须继续工作 |
| `SnapClipCapture.exe` 按需子进程 | 子进程同时拥有截图框、WGC/DXGI、标注渲染和 PNG/WebP 导出；完成后返回文件路径 | 需要隔离捕获崩溃，或 Agent 必须保持极小；不适合再拆成独立 capture/overlay/encode 进程 |

截图进程只在截图会话期间存在，不应随系统启动常驻。启动时需要把目标显示器/窗口、DPI、裁剪区域和临时文件目录作为小型协议传入；结果通过内容寻址文件和元数据返回。长截图期间进程保持到滚动状态机结束，不能每滚动一步重新启动。

### 13.4 WebView2 子进程不是可忽略的内存

WebView2 使用 browser、renderer、GPU 等多进程模型，实际数量取决于页面 origin、站点隔离和 WebView 实例。它们不是 SnapClip 的业务常驻进程，但会计入用户看到的应用资源占用：

- 保持一个主 WebView，并复用同一个 WebView2 environment 和 user-data folder。
- 截图/录屏进入原生窗口后，主界面只显示轻量状态；不要创建全屏或每个贴图一个 WebView。
- UI 隐藏时停止轮询和动画；能访问底层 `CoreWebView2` 时再评估 `TrySuspendAsync`/低内存目标，否则通过隐藏窗口和释放临时资源降低 CPU/Working Set。
- 不能假设隐藏 WebView 会立即释放 browser 进程；必须按进程组测量。

### 13.5 目标方案：按基准决定是否拆出 Agent

如果单进程在“主界面隐藏、剪贴板监听开启”的 60 秒基线中超过内存门槛，或 WebView 崩溃会连带丢失剪贴板监听，再拆成：

```text
SnapClipAgent.exe  常驻、无 WebView2
  ├─ 托盘/全局热键/剪贴板监听
  ├─ SQLite/BlobStore
  ├─ 原生截图框/贴图（或按需委托给 SnapClipCapture.exe）
  └─ 命名管道或本地 RPC

按需子进程
  ├─ SnapClipOcr.exe       模型、OCR 队列，空闲超时退出
  └─ SnapClipRecorder.exe  WGC/DXGI + D3D11 + Media Foundation，录制结束退出

SnapClip.exe       按需启动的 Tauri/Vue UI
  └─ 历史、搜索、设置、录制状态和标注工具栏
```

Agent 拆分的收益是：UI 关闭后不再保留 WebView2 进程组，剪贴板不会被前端崩溃影响；OCR/录屏仍可独立回收。代价是安装包、升级、单实例、命名管道权限、协议版本和跨进程错误恢复都会增加。Agent 与 UI/子进程只传小型命令/状态和受限文件 URL，不能传输整帧图像。

拆分门槛应由实测决定，而不是预先增加复杂度：

1. 单进程隐藏 UI 的 Private Bytes/Working Set 连续超过目标预算；
2. WebView2 renderer/GPU 在空闲状态仍有持续 CPU 或唤醒；
3. 长时间运行出现 WebView 内存增长而刷新/挂起无法恢复；
4. 需要 UI 频繁重启但剪贴板监听必须连续运行。

在没有满足门槛前，保持单进程更容易验证正确性；满足任一门槛后，优先拆 Agent，而不是继续在 WebView 内堆叠缓存和节流补丁。

### 13.6 不应纳入“常驻进程”的对象

`explorer.exe`、`dwm.exe`、`RuntimeBroker.exe` 和 WebView2 Runtime 的系统协作进程由 Windows 管理，不能由 SnapClip 主动结束。验收时应把它们作为外部基线或子进程组单独统计，不把结束系统进程当作优化方案。

## 官方资料

- [Windows screen capture / WGC](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)
- [Direct3D11CaptureFramePool.CreateFreeThreaded](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool.createfreethreaded)
- [Desktop Duplication API](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api)
- [DXGI `AcquireNextFrame`](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-acquirenextframe)
- [Media Foundation Sink Writer](https://learn.microsoft.com/en-us/windows/win32/medfound/sink-writer)
- [Hardware MFTs](https://learn.microsoft.com/en-us/windows/win32/medfound/hardware-mfts)
- [`MFCreateDXGIDeviceManager`](https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mfcreatedxgidevicemanager)
- [Multimedia Class Scheduler Service](https://learn.microsoft.com/en-us/windows/win32/procthread/multimedia-class-scheduler-service)
- [Direct2D `SetAntialiasMode`](https://learn.microsoft.com/en-us/windows/win32/api/d2d1/nf-d2d1-id2d1rendertarget-setantialiasmode)
- [Direct2D `SetTextAntialiasMode`](https://learn.microsoft.com/en-us/windows/win32/api/d2d1/nf-d2d1-id2d1rendertarget-settextantialiasmode)
- [DirectWrite](https://learn.microsoft.com/en-us/windows/win32/directwrite/introducing-directwrite)
- [WebView2 process model](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/process-model)
- [WebView2 performance best practices](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/performance)
- [Tauri IPC](https://v2.tauri.app/concept/inter-process-communication/)
- [Tauri `convertFileSrc`](https://v2.tauri.app/reference/javascript/api/namespacecore/#convertfilesrc)
