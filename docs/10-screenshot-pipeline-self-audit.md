# SnapClip 截图全链路自审与重构方案

> 审计日期：2026-10-03
>
> 范围：F5 热键、显示器捕获、冻结帧、Win32 覆盖层、D3D11/D2D 绘制、选区交互、放大镜、确认导出和资源释放。
>
> 结论：当前实现是一个通过功能回归的 GPU-first MVP，但不是延迟最低、CPU/内存最低或整体性能最优的最终方案。存在三个确定的结构性热点：捕获阻塞 overlay 消息线程、鼠标移动触发同步 Present 且脏矩形被合并成大包围盒、确认时整屏 GPU 回读。

## 1. 结论先行

### 1.1 当前方案的定位

当前方案已经做对了以下方向：

- 截图框是 Rust/Win32 原生窗口，不是 Tauri WebView 全屏画布。
- WGC 优先、BitBlt 降级；WGC 帧直接绑定 D2D，正常预览路径不发生 GPU 到 CPU 的整帧回读。
- 截图前冻结底图，截图框不会被捕获进自己的底图。
- Tauri 只接收生命周期/结果事件，不传输每个鼠标移动和图像帧。
- 会话结束会释放冻结纹理、选区和 renderer，已解决上一次选区残留。
- 截图 artifact 不依赖剪贴板、OCR 或 SQLite。

这些是正确的 MVP 基线，但不能推出“性能最好”。

### 1.2 必须承认的限制

“最好”必须绑定目标。低空闲占用、F5 后首帧延迟、鼠标移动延迟和确认导出延迟之间存在取舍：

| 策略 | 空闲占用 | F5 首帧 | 适用 |
| --- | ---: | ---: | --- |
| 完全按需创建 D3D/WGC | 最低 | 首次较慢 | 默认、省电 |
| 常驻 D3D，按需 WGC | 中等 | 较低 | 推荐默认 |
| 常驻每显示器捕获帧池 | 最高 | 最低 | 明确选择低延迟模式 |

SnapClip 应采用“默认低空闲占用 + 可观测的首次预热 + 不阻塞输入”的中间策略，而不是为了单一 P50 指标永久常驻每个捕获源。

## 2. 当前实现基线

### 2.1 调用链

目前 F5 的完整调用链位于同一个 overlay 线程：

```text
WM_HOTKEY
  -> OverlayController::start_session()
  -> monitor::captured_monitor_at_cursor()
  -> CaptureProviders::new()        // 首次创建 D3D11/D2D/DXGI
  -> CaptureProviders::capture()
     -> WGC CreateFreeThreaded
     -> StartCapture
     -> TryGetNextFrame + 20ms sleep
  -> Win32Renderer::new/resize/set_frame
  -> D2D 绘制
  -> ShowWindow
```

依据：

- `src-tauri/src/platform/windows/capture/overlay.rs:346` 的 `start_session` 在处理 `WM_HOTKEY` 的线程内同步执行全部步骤。
- `src-tauri/src/platform/windows/capture/overlay.rs:922` 直接从 `WM_HOTKEY` 调用 `start_session`。
- `src-tauri/src/platform/windows/capture/win/wgc.rs:116` 使用轮询 `TryGetNextFrame`，间隔为 `20ms`，超时为 `1500ms`。
- `src-tauri/src/platform/windows/capture/providers.rs:183` 首次创建 `GraphicsDevice`，并把它保留在 provider 中。

### 2.2 已确认的性能热点

#### P0：F5 捕获准备阻塞窗口消息循环

WGC 初始化、首帧等待、D3D/D2D 资源准备都在窗口线程。结果是：

- F5 后到 overlay 可见前，鼠标和键盘消息无法正常处理。
- 首帧等待最坏可达 1500ms。
- 设备初始化失败或驱动异常时，整个 overlay 输入循环被拖住。

这不是局部优化问题，而是线程边界错误。需要把捕获准备移到 worker，并通过有界消息回投 overlay。

#### P0：鼠标移动路径同步 Present

`src-tauri/src/platform/windows/capture/overlay.rs:698` 的 `on_mouse_move` 每次移动都会调用 `request_redraw`；`src-tauri/src/platform/windows/capture/renderer.rs:154` 随后调用 D2D 绘制和 `Present(1)`。

这会把高频 `WM_MOUSEMOVE` 变成同步渲染/等待垂直同步。鼠标消息越密集，输入线程越容易被渲染阻塞。正确做法是：输入只更新最新状态并标记 dirty，渲染按显示刷新节奏合并执行。

#### P0：damage 列表最终几乎总是整屏

`src-tauri/src/platform/windows/capture/renderer.rs:118` 的 `cursor_damage` 同时加入：

- 放大镜区域；
- 横跨整个 frame 的水平十字线；
- 横跨整个 frame 的垂直十字线。

`src-tauri/src/platform/windows/capture/win/d2d.rs:852` 再把所有矩形合并成一个包围盒。水平线和垂直线的联合包围盒接近整个屏幕，因此鼠标移动时 `PushAxisAlignedClip` 仍可能覆盖整个 frame，L0 和 L1 也会被重绘。

已有测试证明 clip 能限制写入，但没有证明“鼠标移动的实际绘制面积很小”。这是正确性测试通过、性能目标仍未达到的典型情况。

#### P1：确认时整屏 GPU 到 CPU 回读

`src-tauri/src/platform/windows/capture/providers.rs:79` 的 `FrozenFrame::pixels` 调用 `read_back_bgra`；`src-tauri/src/platform/windows/capture/win/d3d11.rs:246` 创建整屏 staging texture、`CopyResource`、Map 整张图，再由 `FrozenFramePixels` 裁剪。

3840x2160 BGRA8 的原始数据约为 31.6 MiB。确认一个 300x200 的选区仍然会复制整张 4K 纹理，产生 staging、CPU Vec 和 PNG 输入的额外峰值。这是确定的带宽、延迟和内存浪费。

#### P1：WGC 首帧采用轮询而不是 FrameArrived

`CreateFreeThreaded` 已经避免绑定 UI dispatcher，但当前代码没有使用 `FrameArrived` 事件，而是以 20ms 睡眠轮询。它引入不必要的首帧等待和线程唤醒抖动。应使用事件/条件变量等待，并保留超时作为故障保护。

#### P1：每次会话销毁并重建 renderer

`release_session` 会丢弃 `Win32Renderer`。这能清除旧选区，但也会重建 D2D context、composition target、swap chain 和 target bitmap。更合理的边界是：

- 会话结束立即释放冻结帧和动态资源；
- renderer/device 外壳按空闲策略保留或延迟释放；
- 下一次会话第一次绘制必须做显式全量初始化，禁止复用旧像素。

保留 renderer 会降低重复 F5 延迟，但会保留 4K swap chain 内存。因此必须由基准决定空闲回收时间，而不是无条件常驻。

### 2.3 当前实现中不应继续增加的内容

- 不要把每个鼠标事件发送给 Tauri/Vue。
- 不要在每次鼠标移动时读取颜色或整帧 Map。
- 不要为了“统一接口”把 WGC/DXGI 强行转换成整张 CPU 图片。
- 不要在 overlay 线程同步执行 PNG 编码、数据库写入、剪贴板写入或 OCR。
- 不要用 FFmpeg/MPV 作为静态截图的像素管线；它们属于后续录屏/转码边界。

## 3. 参考项目研读结论

已先阅读 `refer/shot-refer/项目介绍.md`，再按与 SnapClip 的相关度研读源码。

### 3.1 Starshot：最接近的冻结帧参考

位置：`refer/shot-refer/Starshot-main/src/Starshot/Features/Screenshot/`。

- 使用 WGC/Win2D/D3D11 方向和冻结帧区域选择，技术路线最接近 SnapClip。
- 放大镜采用固定源像素窗口、最近邻和像素网格，证明当前 GPU 放大方向正确。
- 它是 UI 框架驱动的实现，不能直接证明 SnapClip 当前单线程同步渲染是最优。
- 应借鉴其源像素坐标、整数对齐和边缘翻转，不应照搬 WinUI/Win2D 生命周期。

### 3.2 PowerToys Color Picker：采样节流和对象复用

位置：`refer/shot-refer/PowerToys-main/src/modules/colorPicker/ColorPickerUI/Mouse/MouseInfoProvider.cs`、`Views/ZoomView.xaml.cs`。

- 复用 1x1 Bitmap/Graphics，避免每次取色创建 GDI 对象。
- 使用 DispatcherQueueTimer 按刷新节奏采样，而不是每条鼠标消息采样。
- 高倍率才显示网格，并对亮暗背景调整网格颜色。

SnapClip 的对应方案是 GPU 纹理内最近邻采样；颜色 Hex 可通过低频、非阻塞 1x1 staging 采样，不能把 PowerToys 的 GDI `CopyFromScreen` 放进 D3D11 主路径。

### 3.3 MagnifyShit-cpp：D3D11 放大镜，但不能照搬同步回读

位置：`refer/shot-refer/MagnifyShit-cpp-main/src/`。

- 展示了 D3D11 最近邻 shader、像素网格和 staging 1x1 采样。
- 如果复制后立即 Map staging，会造成 CPU 等 GPU 的同步停顿。
- SnapClip 应使用 staging ring + `D3D11_MAP_FLAG_DO_NOT_WAIT`，读不到时保留上次颜色，而不是阻塞鼠标路径。

### 3.4 Crisp：交互参考，不是捕获性能参考

位置：`refer/shot-refer/Crisp-main/src/`。

- 选区可继续移动、缩放和调整，交互模型值得参考。
- 其 GDI/CPU BitBlt 路径适合 SDR 兼容，不适合作为 4K 高频 GPU 主路径。
- 标注对象和编辑器方向可参考，但 SnapClip 不能每次操作复制整张 BGRA 图。

### 3.5 ShareX、Meazure、MinimalColorPicker

- ShareX：15x15 放大镜、颜色预览、Hex/坐标布局成熟；捕获是 DIB/BitBlt，不能直接替换 WGC 纹理。
- Meazure：按约 70ms 更新放大镜，说明“采样/绘制节流”比响应每条鼠标消息更重要；GDI 实现不能作为 SnapClip 的最终 GPU 方案。
- MinimalColorPicker：全屏 DIB 后内存索引像素很快，但 4K 会常驻约 32 MiB CPU 缓存，不符合“尽量不复制到 CPU”的主路径目标。

### 3.6 Windows Magnification API

官方样例在 `refer/shot-refer/Magnification/`。它适合实时桌面放大，不适合 SnapClip 的冻结纹理、选区挖空和自定义像素网格，因此不应替代 D3D11/D2D overlay。

## 4. 目标架构：低延迟且低空闲占用

### 4.1 线程和所有权

```text
Tauri/WebView 主线程
  -> 低频 start/cancel/confirm 命令

Overlay UI 线程（Win32 message loop）
  -> HWND、输入、状态机、最新鼠标位置
  -> 不等待 WGC，不编码 PNG，不写数据库

Capture worker
  -> WGC 或 DXGI 初始化
  -> FrameArrived/AcquireNextFrame
  -> 产出 PreparedFrame(texture + metadata)

Render tick（可与 overlay 线程同线程，但不能在 WM_MOUSEMOVE 直接 Present）
  -> 合并输入状态
  -> 更新动态 GPU surface
  -> 按刷新节奏 Present/Commit

Export worker
  -> 接收选区大小的 BGRA buffer
  -> PNG/WIC 编码和原子落盘
```

实现要求：

1. `WM_HOTKEY` 只生成带 generation 的 `StartRequest` 并唤醒 capture worker。
2. worker 返回前，overlay UI 线程继续泵消息；结果过期时丢弃，不得把上一会话的 frame 投递到新会话。
3. `Esc`/关闭/显示器变更会增加 generation，并让 worker 丢弃未完成结果。overlay 显示后 `Esc` 走普通 `WM_KEYDOWN`；准备阶段若产品要求可取消，应增加专门的取消通道/轻量键盘监听，不能阻塞窗口线程等待捕获。
4. channel 必须有界，容量为 1；新请求覆盖旧请求，禁止积压多个 4K texture。

### 4.2 捕获 provider 选择

#### 静态截图

- Windows 10/11 首选 WGC monitor capture，输出 D3D11 texture。
- `CreateFreeThreaded` 配合 `FrameArrived` 事件等待首帧；保留约 500ms 可配置超时，超时才 fallback。
- 不要每个 F5 重建 D3D device。device 可在首次使用时创建并缓存；WGC frame pool/session 只在活跃会话期间创建，结束后释放。
- BitBlt 作为 SDR/远程桌面/不支持 WGC 的降级。BitBlt 路径已经有 CPU buffer，应直接按选区裁剪，不再上传后又整屏读回。

#### 录屏预留

- 全屏/高帧率录屏另建 DXGI Desktop Duplication provider，利用 `AcquireNextFrame` 的 dirty/move rect 和 pointer metadata。
- 不要把录屏需求反向塞入静态截图的 `FrozenFrame` 接口。

### 4.3 渲染分层

目标不是“把所有东西放进一个 D2D target 再做包围盒 clip”，而是按变化频率分层：

```text
Visual A（静态）
  L0 冻结帧

Visual B（选区层）
  L1 遮罩、边框、手柄、尺寸标签
  仅选区/拖拽时更新

Visual C（指针层）
  放大镜、中心像素、颜色信息、局部十字准星
  只更新小矩形
```

推荐把十字准星限制在放大镜附近。若产品必须保留贯穿全屏的十字线，应把水平/垂直线放进独立动态 visual，或者逐个 damage rectangle 绘制，禁止再把不连续区域合并成一个全屏包围盒。

鼠标路径改成：

```text
WM_MOUSEMOVE -> 更新 cursor/latest state -> dirty=true -> 若无 render pending 则 post WM_APP_RENDER
WM_APP_RENDER/刷新 tick -> 读取最新 state -> 一次绘制 -> Present/Commit -> 清 pending
```

`Present(1)` 只允许出现在 render tick。这样输入延迟不再等于“每条鼠标消息一次垂直同步”。

### 4.4 放大镜和颜色采样

- 源窗口默认 15x15 或 21x21 物理像素；整数坐标，最近邻采样。
- GPU 直接从冻结纹理绘制放大区域和网格，不做 CPU crop。
- 网格在倍率足够高时显示；中心像素使用实心圆/清晰边框，避免把网格误认为采样像素。
- Hex/颜色块更新频率限制为显示刷新率或更低。
- 颜色读取使用 3 个 1x1 staging texture ring：`CopySubresourceRegion` 后用 `Map(..., D3D11_MAP_FLAG_DO_NOT_WAIT)`；返回 `WAS_STILL_DRAWING` 时保留上一次值。
- 不允许每次 `WM_MOUSEMOVE` 创建 Bitmap、Map 整图或发送 Tauri 事件。

### 4.5 确认导出：只读回选区

将当前接口：

```text
FrozenFrame::pixels() -> full-frame readback
```

改成：

```text
FrozenFrame::read_region(Rect) -> selection-sized BGRA
```

GPU 路径：

1. 创建或复用与选区尺寸相同的 staging texture。
2. 使用 `ID3D11DeviceContext::CopySubresourceRegion`，源 box 为选区。
3. Map staging，逐行处理 `RowPitch`，复制到紧凑的选区 buffer。
4. 将紧凑 buffer 交给 Export worker 做 PNG/WIC 编码。

边界：

- 选区必须先在物理像素坐标中裁剪到 frame bounds。
- 不允许把 overlay 的边框、手柄或标签写入 artifact。
- 多次确认/撤销只重读当前选区，不缓存整张 CPU frame。
- BitBlt fallback 直接从其已有 CPU buffer 裁剪；不要再 GPU readback。

### 4.6 资源生命周期

会话结束时立即释放：

- WGC session/frame pool；
- 冻结 frame texture；
- 选区和动态 surface。

可按策略保留：

- D3D11 device；
- D2D device；
- 隐藏 overlay HWND；
- renderer 外壳和 swap chain。

推荐增加 30~60 秒空闲回收计时器：连续截图时复用 renderer，长时间空闲时释放 4K swap chain，兼顾重复 F5 延迟和长期内存占用。这个阈值必须由基准调整，不能凭感觉固定。

## 5. 官方 Windows API 依据

以下是本方案依赖的官方契约，已通过网页资料核对：

- Windows Graphics Capture：<https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture>
- `Direct3D11CaptureFramePool.CreateFreeThreaded`：<https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool.createfreethreaded>
- Desktop Duplication API：<https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api>
- `CopySubresourceRegion`：<https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11devicecontext-copysubresourceregion>
- `ID3D11DeviceContext::Map` / `D3D11_MAP_FLAG_DO_NOT_WAIT`：<https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11devicecontext-map>
- DXGI `Present1` dirty rectangles：<https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgiswapchain1-present1>
- DXGI presentation improvements：<https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/dxgi-1-2-presentation-improvements>
- DirectComposition visual tree：<https://learn.microsoft.com/en-us/windows/win32/directcomp/directcomposition-portal>

关键解释：

- `CreateFreeThreaded` 的 frame-arrived 回调不要求 UI dispatcher，适合把捕获从窗口消息线程移走。
- Desktop Duplication 的 dirty/move rect 适合录屏帧增量处理；静态截图不需要为了一个 frame 长驻 duplication。
- `CopySubresourceRegion` 能直接复制源纹理的矩形子区域，是消除整屏回读的官方路径。
- `D3D11_MAP_FLAG_DO_NOT_WAIT` 明确允许在 GPU 尚未完成时立即返回，适合颜色预览等非关键数据。
- `Present1` 的 dirty rect 只能作为合成优化，不能替代应用层正确保存未损坏的 back buffer；应用层仍需按层和区域设计。
- DirectComposition 的 visual tree 适合把静态底图、选区层和指针层分开提交。

## 6. 修改优先级

### P0：先改根因

1. 将 `start_session` 拆成 `StartRequest -> capture worker -> PreparedFrame`，overlay 消息线程不再同步等待 WGC。
2. 将鼠标移动改为 coalesced render tick；禁止 `WM_MOUSEMOVE -> Present(1)`。
3. 去掉“全屏水平线 + 全屏垂直线 + 单包围盒”的 damage 设计；至少把指针层独立出来。
4. 将 `FrozenFramePixels` 改成区域 GPU readback；补充 4K 大屏/小选区的字节数和峰值内存测试。

### P1：随后优化

5. WGC 改用 FrameArrived 等待，不再 20ms 轮询。
6. BitBlt fallback 保留 CPU buffer，确认时直接裁剪。
7. renderer 外壳按空闲计时复用/回收，记录复用收益和 Working Set。
8. 放大镜颜色信息使用 1x1 staging ring + `DO_NOT_WAIT`，不阻塞渲染。

### P2：产品能力扩展

9. 标注使用 Rust 对象模型 + D2D 矢量层；导出时重放同一模型。
10. 长截图独立使用稳定等待、多带位移匹配、重叠覆盖和边界终止状态机。
11. 录屏独立使用 DXGI/WGC frame ring + Media Foundation 硬件编码；GIF 停止后异步转码。

## 7. 前后测试与性能验收

### 7.1 已运行基线

```text
cargo test --lib       133 passed, 0 failed
cargo check --all-targets   passed, 0 warnings
npm run typecheck      passed
```

这些结果只证明当前功能和类型检查通过，不证明延迟或占用达标。

### 7.2 必须新增的自动化测试

- capture worker 取消、generation 过期结果丢弃、重复 F5 不串帧。
- WGC FrameArrived 超时和 provider fallback。
- `read_region` 对左上、右下、负显示器坐标、奇数 RowPitch 的裁剪正确性。
- 4K frame 截取小区域时，GPU readback 字节数等于选区 staging 尺寸，而不是整帧。
- render coalescing：N 个鼠标事件在一个 tick 内只产生一次 Present。
- damage 不连续区域不会扩展成整屏；指针层更新不改变静态层像素。
- overlay 取消/设备移除/显示器变化后，GPU 资源和 session generation 均归零。

### 7.3 Windows 实测指标

使用 Windows Performance Recorder/Analyzer、PresentMon 和进程计数器记录，至少比较重构前后：

```text
F5 -> monitor ready
F5 -> provider ready
F5 -> frame ready
F5 -> renderer ready
F5 -> overlay visible
WM_MOUSEMOVE -> 实际 Present 延迟 P50/P95
鼠标持续移动 10 秒的 CPU 平均/P95
确认 300x200、1920x1080、3840x2160 选区的 readback 字节数
确认时 Private Bytes / Working Set / GPU Dedicated / Shared 峰值
空闲 30 秒和重复 F5 的资源占用
```

建议验收目标（不是未经测量的承诺）：

- 热键消息线程不出现超过 16ms 的同步捕获工作。
- 鼠标移动渲染最多按显示刷新率提交，输入事件不触发一对一 Present。
- 小选区确认不产生整屏 staging/readback。
- 首次 F5 和暖机后 F5 分开报告，不能用平均值掩盖首次初始化。
- 4K/144% DPI、双显示器、负坐标、WGC 不可用、设备移除分别验收。

## 8. 最终判断

当前方案适合作为“已能工作的原生截图 MVP”，不适合作为“已经达到最佳性能的正式截图架构”。不应继续在现有同步调用链上堆缓存或局部判断；应按以下顺序重构：

```text
异步捕获准备
  -> 刷新节奏合并渲染
  -> 静态/选区/指针分层
  -> 选区级 GPU readback
  -> 可观测的 device/renderer 空闲回收
```

完成这五步并用真实 Windows ETW/PresentMon 数据对比后，才能回答“延迟最低、占用最低”是否成立。当前测试的 133 个单元/视觉回归用例必须保留，并在每一步重构后追加对应的前后行为和资源指标，不能以单元测试通过替代性能验收。
