# SnapClip 截图全流程、控件功能与 UI 重构执行任务清单

> 文档状态：开发期目标设计与执行清单
>
> 适用平台：Windows 10/11 x64，Per-Monitor-V2 DPI awareness
>
> 目标：在不依赖 Tauri WebView 像素管线、不耦合剪贴板/OCR/历史存储的前提下，重构 SnapClip 的截图全流程、原生截图框、放大镜、选区控件、标注工具栏、长截图和导出闭环。
>
> 重要原则：本项目尚未正式发布，允许破坏性重构。任务完成标准是根因解决、功能正确、性能可测、回归通过，而不是“代码已经修改”。

文档关系：`08-screenshot-mvp-tasklist.md` 保留 MVP 的原生截图闭环验收；`10-screenshot-pipeline-self-audit.md` 保留性能审计和指标要求；本文件扩展并统一全流程、控件、UI、标注、长截图和后续录屏的执行顺序。三者发生冲突时，以本文件的后续重构设计和实际测试结果为准，不能用旧 MVP 结构限制根因重构。

---

## 0. 使用规则

### 0.1 勾选规则

- [ ] 未经过测试的设计只能写为“计划”，不能写成“已完成”。
- [ ] 每个性能优化必须记录修改前后指标。
- [ ] 每个跨模块改动必须先补状态/契约/单元测试，再改 Win32 或 GPU 实现。
- [ ] 不得通过删除测试、放宽断言、关闭功能或只测试热路径来制造通过。
- [ ] 参考项目只吸收设计和算法经验，不复制代码、目录和许可证不明的实现。

### 0.2 参考资料已研读范围

已阅读 `refer/shot-refer/项目介绍.md`，并按与 SnapClip 的相关度复核以下源码：

- Starshot：`Features/Screenshot/RegionCaptureWindow.xaml.cs`、`ScreenCaptureHelper.cs`
- PowerToys Color Picker：`ZoomView.xaml.cs`、`MouseInfoProvider.cs`、`ZoomWindowHelper.cs`
- ShareX：`ScreenColorPickerWindow.axaml.cs`、`MagnifierPixelGrid.cs`
- MagnifyShit-cpp：D3D11 renderer、shader、Eyedropper
- Meazure：`Magnifier.cpp`、`Magnifier.h`
- Crisp：`Annotation.*`、`Overlay*`、`Stitch*`、`Geometry`、相关测试
- MinimalColorPicker、Free-Color-Picker、Windows Magnification API 示例

### 0.3 采用与拒绝的经验

| 来源 | 采用 | 明确拒绝 |
| --- | --- | --- |
| Starshot | WGC 冻结帧、15x15 最近邻、整数像素对齐、约 16ms 渲染节奏、多 DPI | 不复制 WinUI 生命周期和整窗状态 |
| PowerToys | 会话级像素缓存、按刷新率采样、复用 1x1/小缓冲、亮度自适应网格 | 不使用 GDI `CopyFromScreen` 作为 SnapClip 主路径 |
| ShareX | 放大镜信息布局、颜色预览、Hex/坐标、15x15 UI 组织 | 不使用 DIB/BitBlt 作为高频 GPU 路径 |
| MagnifyShit-cpp | D3D11 最近邻 shader、像素网格、staging 小区域采样 | 不照搬立即 `Map` 导致 GPU/CPU 同步停顿的实现 |
| Meazure | 1/2/3/4/6/8/16/32 倍率、颜色格式、低频更新、可暂停放大镜 | 不使用 `GetPixel`/`StretchBlt` 实时采样 |
| Crisp | 可再次调整选区、对象化标注、窗口选择、纵横长截图、纯逻辑测试 | 不复制其 CPU 整图合成作为鼠标/录屏路径 |
| MinimalColorPicker | 原生 Win32 轻量交互、直接索引小像素块 | 不常驻整屏 CPU DIB（4K 约 32 MiB） |
| Magnification API | 坐标变换和窗口过滤的理解 | 不作为冻结帧和自定义网格核心 |

---

## 1. 现状基线与必须解决的问题

### 1.1 当前已存在的正确边界

- [x] 截图框使用 Rust/Win32 原生 HWND，不使用 Tauri WebView 全屏截图框。
- [x] WGC 优先，BitBlt 作为兼容降级。
- [x] 截图前冻结底图，防止 overlay 捕获自身。
- [x] D3D11 texture 直接绑定 D2D 作为 L0，正常预览不整帧回读。
- [x] 截图模块不直接依赖剪贴板监听、OCR 或 SQLite。
- [x] 会话结束释放旧选区和冻结帧，避免下一次 F5 显示上一次状态。
- [x] Tauri 只接收低频生命周期/结果事件，不接收鼠标移动和像素帧。

### 1.2 当前确定的结构性问题

#### P0：捕获阻塞 overlay 消息线程

当前 `WM_HOTKEY -> OverlayController::start_session()` 同步执行：显示器查询、D3D/WGC 初始化、WGC 首帧等待、renderer 准备和首次绘制。

影响：

- F5 后到 overlay 可见前，Esc、鼠标和窗口消息响应不稳定。
- WGC 当前首帧轮询最长可达 1500ms。
- 设备创建或驱动异常会阻塞输入线程。

目标：捕获准备移至独立 worker；overlay 线程只维护 HWND、输入和状态机。

#### P0：每个 `WM_MOUSEMOVE` 立即渲染和 Present

当前 `on_mouse_move` 直接调用 `request_redraw`，渲染路径使用 `Present(1)`。

目标：鼠标事件只更新最新状态；用 `WM_APP_RENDER` 或刷新 tick 合并事件后提交一次 Present。

#### P0：damage 被合并为大包围盒

当前放大镜之外还加入贯穿全屏的水平/垂直十字线，最后统一求包围盒，导致小范围鼠标移动可能退化成接近整屏重绘。

目标：静态层、选区层、指针层分开；十字线默认限制为局部准星。若必须全屏十字线，使用独立动态 visual 或多个独立 damage，不再求单一大包围盒。

#### P1：确认截图整屏 GPU 回读

当前确认时创建整屏 staging texture、`CopyResource`、Map 全屏，再裁剪选区。

目标：`FrozenFrame::read_region(Rect)` 使用 `CopySubresourceRegion` 直接读取选区大小的 staging texture；BitBlt 已有 CPU buffer 时直接裁剪。

#### P1：WGC 首帧使用 20ms 轮询

目标：`CreateFreeThreaded + FrameArrived` 或等价条件变量等待，保留可配置超时和 fallback。

#### P1：renderer 生命周期没有按空闲策略设计

当前会话结束会释放 renderer。这样能清除旧像素，但重复 F5 需要重建 D2D/DirectComposition/swap chain。

目标：会话资源立即释放，D3D/D2D/device/swap chain 外壳按连续使用和空闲时间复用/回收，并记录 Working Set 与首帧收益。

---

## 2. 产品级完整流程

### 2.1 主流程

```text
应用启动
  -> 注册 F5
  -> 预创建并隐藏 overlay HWND
  -> 初始化低成本 capture runtime
  -> 可选后台预热 D3D device

F5
  -> 记录前台窗口、鼠标物理坐标和 monitor
  -> generation + 1
  -> capture worker 获取冻结 texture
  -> overlay 保持可响应，允许取消准备过程
  -> frame ready 后创建/复用 renderer
  -> 先在隐藏/屏外状态完成首帧合成
  -> 显示 overlay，进入 Selecting

鼠标交互
  -> 创建选区 / 移动 / 八向缩放
  -> 更新局部静态层和指针层
  -> 放大镜显示冻结帧局部内容

Enter
  -> 校验非空选区
  -> 只回读选区
  -> 导出 worker 编码 PNG
  -> 原子落盘
  -> 可选发布剪贴板、历史和 Tauri 结果事件
  -> 释放会话资源并还原前台窗口

Esc / 右键 / 失败 / 设备移除 / 显示器变化
  -> generation + 1
  -> 取消 worker 结果
  -> 隐藏 overlay
  -> 释放当前会话 texture、采样缓存、标注临时状态
  -> 恢复原前台窗口
```

### 2.2 状态机

Rust 领域状态必须保持纯逻辑、可单元测试；Win32 消息只驱动状态变化。

```text
Idle
  -> Preparing        F5
  -> Armed            冻结帧和 renderer 就绪
  -> Selecting        overlay 已显示，可创建选区
  -> Selected         左键拖拽完成，选区有效
  -> Adjusting        移动/缩放已有选区
  -> Annotating       已进入标注阶段（后续版本）
  -> Exporting        Enter 后异步裁剪/编码
  -> Idle             完成、取消或失败
```

状态规则：

- [x] `Preparing` 阶段 Esc 必须可取消；不能等 WGC 超时后才响应。
- [x] 每个会话有 `generation`；旧 worker 返回结果必须丢弃。
- [ ] F5 重复触发不得把旧 texture、selection、magnifier 或 label 带入新会话。
- [ ] `Exporting` 期间 overlay 不再接收新的确认；失败回到 `Selected` 或结束会话，策略需明确。
- [x] 设备移除、窗口销毁、显示器变化和用户取消共用清理路径。

### 2.3 快捷键和鼠标行为

| 输入 | Selecting | Selected/Adjusting | Annotating | 说明 |
| --- | --- | --- | --- | --- |
| F5 | 重新开始当前会话 | 取消旧会话后启动新会话 | 由产品策略决定，默认结束旧会话后启动 | 不堆积请求 |
| Esc | 取消 | 取消 | 关闭工具栏/取消当前工具，连续 Esc 再退出 | 全部可回到 Idle |
| Enter | 无操作 | 确认导出 | 完成当前标注/进入导出 | 不同步执行编码 |
| 右键 | 取消 | 取消 | 取消当前工具或退出编辑 | 与 Esc 共用清理 |
| 左键拖拽 | 新建选区 | 移动/缩放选区 | 绘制标注 | 命中区域按物理像素计算 |
| Shift | 保持中心/约束比例，后续实现 | 同左 | 约束标注方向/比例 | 必须有单测 |
| Ctrl | 精细移动/吸附策略，后续实现 | 同左 | 复制/对齐策略，后续实现 | 不在 MVP 偷加复杂行为 |
| 方向键 | 后续：移动选区 1px | 后续：移动选区 1px | 移动选中对象 | 每次更新合并渲染 |
| Space | 后续：选中当前显示器 | 同左 | 无 | 参考 Crisp |

### 2.4 选区几何规则

- [ ] 所有截图坐标使用物理像素，`right/bottom` 为 exclusive。
- [ ] 鼠标 client 坐标通过 monitor origin 转为 monitor-local physical pixels。
- [ ] 拖动任意方向都归一化为 `Rect`。
- [ ] 选区始终裁剪到当前 monitor frame。
- [ ] 最小可导出尺寸默认为 1x1；产品 UI 可设置更大的误触阈值，但必须区分“点击”和“截图”。
- [ ] 八向手柄视觉尺寸按 DPI 放大，命中区域大于视觉区域。
- [ ] 选区移动保持尺寸，超出边界时钳制。
- [ ] 选区缩放可跨过相对边缘，最终仍归一化。
- [ ] 选区重置必须清空旧 selection、drag、resize mode、label、magnifier cache。

---

## 3. 捕获后端和线程重构

### 3.1 推荐线程模型

```text
Tauri 主线程
  -> 低频命令：start/cancel/confirm/toolbar

Overlay UI thread
  -> F5 注册、HWND、WM_*、鼠标状态、渲染 tick
  -> 不等待 WGC、不做 PNG、不写 SQLite、不调用剪贴板

Capture worker
  -> WGC/DXGI provider、frame pool、首帧等待
  -> 有界 mailbox（容量 1）返回 PreparedFrame

Export worker
  -> selection readback、PNG/WIC 编码、原子落盘

Application service
  -> artifact -> 可选 publication/history/clipboard/OCR
```

### 3.2 捕获 provider 策略

#### WGC：静态截图首选

- [x] 复用 D3D11 device，不在每次 F5 创建 device。
- [x] 活跃会话创建 monitor `GraphicsCaptureItem`、frame pool 和 session。
- [x] 使用 `CreateFreeThreaded`，通过 `FrameArrived` 或条件变量等待首帧。
- [x] `SetIsCursorCaptureEnabled(false)`，由 overlay 自己绘制鼠标/准星，避免鼠标被重复捕获。
- [x] `SetIsBorderRequired(false)`；失败时只记录能力，不把截图整体判定为失败。
- [x] 首帧超时后 fallback 到 BitBlt，并记录 provider/failure reason。
- [x] 会话结束立即关闭 WGC session/frame pool，释放 frame 引用。

#### DXGI Desktop Duplication：录屏和增量捕获预留

- [ ] 不把录屏 provider 塞进静态 `FrozenFrame`。
- [ ] 录屏时保存 dirty rect、move rect、pointer shape、QPC timestamp。
- [ ] 使用有界 GPU frame ring；编码落后时丢帧，不阻塞输入和捕获。

#### BitBlt：兼容降级

- [ ] 只用于 WGC 不可用、远程桌面或受限环境。
- [x] 已有 CPU DIB 时直接裁剪选区；禁止上传 GPU 后再整屏 readback。
- [ ] 明确 HDR、分层窗口、受保护内容的限制并写入诊断信息。

### 3.3 Worker 请求协议

定义内部协议，不通过 Tauri 传输像素：

```rust
StartRequest {
    generation: u64,
    monitor: MonitorId,
    cursor_screen: Point,
    requested_at_qpc: i64,
}

PreparedFrame {
    generation: u64,
    provider: ProviderKind,
    texture: GpuTexture,
    size: PhysicalSize,
    dpi: u32,
    captured_at_qpc: i64,
}
```

任务：

- [x] `generation` 不匹配时丢弃 frame 并释放 GPU 引用。
- [x] channel 容量固定为 1，新请求覆盖旧请求。
- [x] Cancel 使用原子标志或轻量消息，不等待 worker join。
- [x] 记录 `monitor_ready/provider_ready/frame_ready/renderer_ready/visible` 时间戳。

---

## 4. 原生截图框和窗口管理

### 4.1 HWND 样式

- [ ] `WS_POPUP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP`。
- [ ] 不使用 `WS_EX_NOACTIVATE`，因为 Esc/Enter 必须进入 overlay。
- [ ] `WM_MOUSEACTIVATE` 激活 overlay 并设置焦点。
- [ ] 记录会话前 `GetForegroundWindow`，结束时恢复前台窗口。
- [ ] 隐藏/屏外状态不允许显示旧 swap-chain 内容。
- [ ] overlay 不进入任务栏和 Alt+Tab。
- [ ] DPI context 在进程启动前设置，窗口响应 `WM_DPICHANGED`。

### 4.2 窗口生命周期

- [ ] 启动时预创建一个隐藏 overlay HWND 和 F5 热键。
- [ ] 可连续截图时复用 HWND；会话状态、selection、frame bitmap 必须每次重置。
- [ ] renderer/swap-chain 采用“会话内创建、短时间复用、空闲回收”策略。
- [ ] 30~60 秒空闲后释放 4K swap-chain 的阈值作为可配置实验参数。
- [ ] `WM_DISPLAYCHANGE`、`WM_DPICHANGED`、`WM_DEVICECHANGE` 统一触发重新准备或取消。
- [ ] overlay 销毁时注销 F5、停止 worker、释放 composition target 和 graphics device 引用。

### 4.3 首帧防闪烁

显示顺序必须是：

1. 捕获 worker 返回新冻结帧。
2. 设置新 renderer frame 和清空动态层。
3. 隐藏状态完成一次完整绘制和 Present/Commit。
4. 确认 compositor 已提交后再显示 overlay。
5. 新会话不允许继承上次 selection、label、magnifier 或 annotation texture。

验收：连续执行“框选 -> Esc -> F5”至少 20 次，不得出现上次选区一闪而过。

---

## 5. GPU 渲染架构

### 5.1 DirectComposition visual 分层

目标树：

```text
Root Visual
  ├─ Static Visual
  │    └─ L0 frozen texture
  ├─ Selection Visual
  │    └─ L1 mask + border + handles + size label
  └─ Pointer Visual
       └─ magnifier + pixel marker + local crosshair + color info
```

实现约束：DirectComposition visual 的 content 必须是可组合的 swap chain、composition
surface 或 D2D render target；不能把任意 `ID3D11Texture2D` 直接当作 visual content。
冻结 D3D11 texture 作为 L0 的 GPU source，由 D2D 绘制到静态 surface/swap chain，之后
由 visual tree 组合。指针层优先使用小尺寸透明 surface，避免为放大镜额外创建第二个
全屏 BGRA swap chain。

任务：

- [x] L0 在会话内只绑定一次冻结 texture。
- [x] L1 无选区时填充全屏遮罩；有选区时用四个 band 挖空。
- [ ] Selection Visual 只在选区、DPI、主题或标签变化时更新。
- [ ] Pointer Visual 只在鼠标位置、采样颜色或放大倍率变化时更新。
- [ ] 静态层和动态层不共享需要整层重绘的 D2D target。
- [x] 若暂时保持单 target，必须实现多个 damage clip，不得合并成全屏包围盒。

### 5.2 遮罩和选区框

- [ ] 选区外使用单色半透明暗色遮罩，默认 alpha 约 0.45，允许主题配置。
- [ ] 选区内保留 L0 原始像素，不通过 CPU 重新合成。
- [ ] 边框使用 1~2 个物理像素高对比颜色，支持明暗背景。
- [ ] 圆角边框仅用于视觉层，不改变导出矩形几何；若产品最终采用直角，删除无效圆角参数。
- [ ] 手柄改为更易识别的实心圆，视觉直径按 DPI 缩放，命中半径更大。
- [ ] 手柄颜色、描边、悬停和拖拽状态可配置。
- [ ] 高对比度模式下使用系统高对比色，不能依赖半透明阴影。

### 5.3 尺寸标签

默认格式：

```text
12,1334 123×200 px
```

其中：

- `12,1334` 为选区左上角物理屏幕坐标；
- `123×200` 为物理像素宽高；
- 标签默认放在选区顶部左侧；
- 如果顶部空间不足，翻转到选区底部；
- 如果上下都遮挡选区或超出工作区，则不绘制，不能压住选区内容。

实现任务：

- [ ] 文本由 DirectWrite 绘制，不使用 WebView 文本。
- [ ] 文字和背景面板按 DPI 缩放，宽度按数字位数动态计算。
- [ ] 标签 panel 不得写入 artifact。
- [ ] 坐标选择 screen/local 坐标的规则写入 DTO 和单测。
- [ ] 负坐标、多显示器、4K/150% DPI 必须有黄金测试。

---

## 6. 放大镜重构任务

### 6.1 产品规格

基于 `docs/Temp/放大镜设计.md` 和参考项目，初版固定如下：

- [x] 采样源：冻结帧，不读取实时桌面，不读取 overlay 自身。
- [x] 可视源窗口：默认 21x21 物理像素；兼容配置 15x15。
- [x] GPU 缓存 tile：32x32 BGRA8，源窗口与 tile 使用同一坐标体系。
- [x] 默认显示倍率：8x；可选倍率 `1,2,3,4,6,8,16,32`。
- [x] 缩放算法：最近邻；一个源像素对应一个清晰放大单元。
- [ ] 网格：倍率 >= 4x 显示，整数像素对齐；亮/暗背景自适应黑白线。
  (已实现 zoom>=4 门控和整数对齐；亮度自适应需 Phase 7 视觉完善)
- [x] 中心标记：实心圆或中心像素高亮框，不再使用容易误解为采样像素的黄色小点。
- [x] 颜色预览块：约 20~28 physical px，来自中心源像素。
- [x] 文本：`#RRGGBB`、可选 RGB、物理坐标。
- [x] 面板边界自动翻转和钳制，不能越过当前 monitor work area。

### 6.2 放大镜空间布局

放大镜不是一个独立实时屏幕窗口，而是 pointer visual 内的 GPU panel：

```text
┌──────────────────────────────┐
│  21x21 nearest-neighbor view │
│  grid + center marker        │
├──────────────┬───────────────┤
│ color swatch │ #RRGGBB       │
│              │ x,y           │
└──────────────┴───────────────┘
```

任务：

- [x] 面板不覆盖鼠标热点；默认右下，靠近右/下边缘时翻转到左/上。
- [x] 面板源像素区域和颜色采样区域必须由同一个 `MagnifierSample` 计算。
- [x] 面板位置变化只 dirty 旧 panel 与新 panel 的并集。
- [x] 面板文本颜色根据背景亮度选择，不能固定白色导致不可读。
- [x] 不绘制额外黄色点；中心采样位置只用一个清晰的圆/框表达。
- [x] 颜色块和 Hex 无变化时不触发文本重绘。

### 6.3 GPU 放大绘制

```text
Frozen D3D11 texture
  -> source rect（整数物理像素）
  -> D2D DrawBitmap / shader，NearestNeighbor
  -> pixel grid
  -> center marker
  -> color swatch / text
```

任务：

- [x] D2D `DrawBitmap` 指定 `D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR`。
- [x] source rect 在 frame 边缘时平移而不是缩小，保持固定源窗口尺寸。
- [x] grid 线落在整数 cell boundary，避免半像素模糊。
- [x] 中心 marker 最后绘制；不能改变颜色采样值或导出内容。
- [ ] 如未来改用 shader，必须保留 D2D 路径作为视觉回归基线。

### 6.4 异步颜色采样

颜色块可以 GPU 直接从 1x1 纹理绘制；Hex 读取使用三缓冲 staging：

```text
cursor moved
  -> tile cache hit: 直接使用上次 Map 结果
  -> miss: CopySubresourceRegion 32x32
  -> submit query/fence
  -> 下一 render tick 非阻塞检查
  -> DO_NOT_WAIT 成功才 Map
  -> 读取中心像素并更新 #RRGGBB
```

任务：

- [x] 三个 32x32 BGRA staging slot，总 CPU/GPU staging 数据约 12KB。
- [x] `WM_MOUSEMOVE` 不执行阻塞 Map。
- [x] GPU 未完成时保留上次颜色，不阻塞鼠标和 Present。
- [x] 采样请求最多 60Hz，或不超过显示刷新率。
- [x] 相同 tile 不重复提交 CopySubresourceRegion。
- [x] 采样失败只标记 stale，不清空可用的上一次 Hex。
- [x] 采样不经过 Tauri，不创建常驻进程或临时线程。

### 6.5 放大镜测试

- [x] 15x15/21x21 源窗口在屏幕四边均保持固定尺寸。
- [x] 1x/4x/8x/16x/32x 显示倍率的源像素对应关系正确。
- [ ] 每个源像素放大后颜色逐像素一致。（Phase 7 视觉回归）
- [x] grid >= 4x 才出现，线条不覆盖中心颜色块。
- [ ] 中心 marker 在黑、白、彩色和透明 alpha 背景上均可见。（Phase 7 视觉回归）
- [x] 光标快速移动 10 秒，采样队列不会增长，CPU 不持续分配大 buffer。
- [x] Hex 延迟和 stale 状态可观测，不得让 UI 卡顿。

---

## 7. 选区工具和工具栏 UI

### 7.1 工具栏窗口边界

工具栏可以使用独立 Tauri 无边框窗口，但不能成为截图像素管线：

```text
Vue toolbar
  -> low-frequency command
  -> Rust capture application
  -> PostMessage/overlay state
```

禁止：

- [ ] WebView 接收每个鼠标移动。
- [ ] WebView Canvas 绘制冻结帧或标注最终图。
- [ ] 工具栏等待 PNG/texture 返回后再允许拖拽。
- [ ] 每个 UI 控件调用一次 Tauri IPC 形成高频循环。

### 7.2 工具栏阶段

#### Selecting 工具栏

- [ ] 截图模式按钮：区域、窗口、全屏、显示器（后续能力分级）。
- [ ] 尺寸标签：只读显示坐标和尺寸，不覆盖选区内容。
- [ ] 放大镜开关。
- [ ] 放大倍率菜单或步进按钮。
- [ ] 网格开关（倍率过低时禁用或提示不可用）。
- [ ] 取消按钮：图标按钮，tooltip“取消截图”。
- [ ] 确认按钮：图标按钮，tooltip“确认截图”。

#### Annotating 工具栏

- [ ] 选择工具：选中已有标注、移动、缩放、删除。
- [ ] 箭头、直线、矩形、椭圆、画笔、高亮、文字、序号。
- [ ] 模糊/马赛克：只作用于标注对象的局部区域。
- [ ] 颜色 swatch：打开颜色选择器，不使用长文本按钮。
- [ ] 线宽 stepper：1/2/3/4/6/8 physical px。
- [ ] 填充开关：checkbox/toggle。
- [ ] 撤销、重做：熟悉的图标按钮。
- [ ] 复制、保存、关闭：图标按钮并提供 tooltip。

#### 长截图工具栏

- [ ] 方向 segmented control：垂直 / 水平。
- [ ] 开始滚动、暂停、停止。
- [ ] 重叠策略：自动 / 手动高级设置。
- [ ] 当前帧、已拼高度/宽度、置信度、失败原因。
- [ ] 到底、匹配失败、尺寸超限必须显示明确状态。

### 7.3 UI 视觉规则

- [ ] 工具栏使用 8px 间距基线，控件高度 32~36px，卡片圆角不超过 8px。
- [ ] 工具按钮优先使用图标；陌生图标必须有 tooltip。
- [ ] 浅色、深色、高对比度主题均有可读 focus/hover/pressed 状态。
- [ ] 不使用 WebView 大渐变、装饰性光球或高频模糊。
- [ ] 选区工具栏自动选择上方/下方位置，限制在当前工作区。
- [ ] 窄屏/高 DPI 下文字不得溢出按钮或遮挡选区。
- [ ] 操作期间不做持续动画；只有打开、关闭、倍率改变可使用 120~200ms 动画。

---

## 8. 标注对象模型与可再次编辑

### 8.1 数据模型

标注必须保存为对象，不能立即烧录进底图：

```rust
AnnotationDocument {
    base: FrozenImageRef,
    items: Vec<AnnotationItem>,
    selected_id: Option<AnnotationId>,
    undo: Vec<DocumentSnapshot>,
    redo: Vec<DocumentSnapshot>,
}

AnnotationItem {
    id: AnnotationId,
    kind: AnnotationKind,
    geometry: Geometry,
    style: AnnotationStyle,
    z_index: u32,
}
```

`AnnotationStyle` 至少包含：

- stroke color、fill color、opacity；
- stroke width（物理像素）；
- line cap/join；
- font family/size/weight；
- blur/mosaic strength；
- arrow head style；
- text alignment。

### 8.2 编辑规则

- [ ] 新建对象只在 pointer-up 或文字提交时进入 undo history。
- [ ] 拖动中的每个 mouse move 不创建 undo snapshot。
- [ ] 选中对象显示控制点/边界框，但控制点不写入导出。
- [ ] 修改颜色、线宽、字体只修改当前对象样式。
- [ ] 删除、复制、置顶、置底均可撤销。
- [ ] 选区裁剪/旋转/缩放属于 image operation；执行前先保存可重放状态。
- [ ] 对象坐标使用截图内容物理像素坐标，不使用窗口 DIP 坐标。
- [ ] 导出和预览重放同一份文档，防止屏幕和 PNG 样式不一致。

### 8.3 抗锯齿策略

- [ ] 线、矩形、椭圆、贝塞尔路径使用 Direct2D geometry。
- [ ] 文字使用 DirectWrite，不使用低分辨率 WebView Canvas。
- [ ] 开启 per-primitive antialias；像素精确边框单独使用整数对齐。
- [ ] 不对已经栅格化的标注重复缩放。
- [ ] 模糊/马赛克只处理对象 bounds 内部，边缘不能泄露原图。
- [ ] 如必须超采样，限制在最终导出或局部 tile，禁止每个鼠标移动超采样整图。
- [ ] D2D 预览与导出使用同一 geometry/style 代码路径。

### 8.4 标注渲染层

```text
L0 frozen image
L1 committed annotations
L2 current editing object
L3 selection handles / toolbar / magnifier
```

- [ ] L1 按 tile 或 dirty bounds 更新。
- [ ] L2 只绘制当前对象及其旧/新 bounds 并集。
- [ ] 工具栏和 handles 永远不进入 artifact。
- [ ] 录屏未来使用 L1/L2 GPU overlay 合成，不复制整张 CPU BGRA。

---

## 9. 长截图：垂直与水平统一设计

### 9.1 采集流程

```text
锁定目标窗口/区域
  -> 发送滚动输入
  -> 等待连续帧稳定
  -> 捕获下一冻结帧
  -> 多带相关匹配 shift
  -> 置信度通过才提交
  -> 重叠区域覆盖合并
  -> 边界/停止条件检查
```

### 9.2 方向抽象

```rust
enum StitchAxis { Vertical, Horizontal }
```

垂直和水平必须共享：

- overlap 搜索核心；
- 置信度评分；
- 动态内容拒绝；
- position/max_depth；
- 画布上限和停止状态。

只替换坐标投影、滚动输入和追加方向，禁止复制两套几乎相同实现。

### 9.3 防重叠、空白和错误终止

- [ ] 上一帧底部/中部/顶部取多条带，下一帧限定搜索窗口。
- [ ] 多条带 shift 必须一致，否则拒绝当前帧。
- [ ] shift 超过帧高/宽 60% 时拒绝，不能伪装成有效位移。
- [ ] 新帧只追加有效增长区，重叠区由新帧覆盖旧帧。
- [ ] 连续 3 帧无增长或边缘一致才判断到底/到边。
- [ ] 粘性 header/footer 不参与匹配；header/footer 只按规则保留一次。
- [ ] 失败时保留已拼出的内容并报告停止原因，不生成看似完整的空白图。
- [ ] 使用 `position` 和 `max_depth` 防止来回滚动重复扩张画布。
- [ ] mailbox 容量为 1，只保留最新帧，避免 BGRA 图像无限堆积。
- [ ] 同时支持垂直网页、横向表格、时间线和图片画廊。

### 9.4 长截图测试

- [ ] 已知垂直位移的合成帧恢复精确 shift。
- [ ] 已知水平位移的合成帧恢复精确 shift。
- [ ] 交替方向、负方向和小位移可拒绝或正确处理。
- [ ] 动态广告/时间戳噪声不产生错误拼接。
- [ ] 粘性 header/footer 不重复。
- [ ] 匹配失败、达到最大尺寸、用户取消均有确定状态。
- [ ] 输出没有重叠、空白、重复条带和越界像素。

---

## 10. 导出、剪贴板、历史和 OCR 集成

### 10.1 截图模块只输出 artifact

截图模块输出：

```rust
CaptureArtifact {
    session_id,
    path,
    width,
    height,
    dpi,
    provider,
    captured_at,
}
```

截图模块不得直接：

- [ ] `OpenClipboard`/`SetClipboardData`；
- [ ] 写 SQLite；
- [ ] 启动 OCR；
- [ ] 发起 Tauri 高频事件；
- [ ] 依赖 `ClipboardMonitor`。

### 10.2 Export worker

- [x] `Enter` 只做选区 readback（唯一触碰 D3D11 即时上下文的步骤，须留在 overlay 线程），再把回读得到的选区像素交给 export worker；worker 收到的是像素而非 frame 引用（device 单线程，不能跨线程并发）。
- [x] GPU 路径 `CopySubresourceRegion` 只读 selection。
- [x] 逐行处理 RowPitch，生成紧凑 BGRA buffer。
- [x] PNG/WIC 编码不在 overlay 线程执行。
- [x] 采用临时文件 + flush + atomic rename。
- [x] 输出路径、尺寸、provider、耗时写入诊断日志。
- [x] 失败时不留下半成品文件（原子落盘 + 取消时删除已写文件）。

### 10.3 应用层集成顺序

```text
CaptureArtifact
  -> 可选保存 history/blob
  -> 可选复制到 Windows clipboard
  -> 可选 enqueue OCR
  -> 发送 capture-completed-v1
```

- [ ] 剪贴板策略由 `features/clipboard` 决定。
- [ ] 历史列表只请求缩略图或按需加载完整图片。
- [ ] OCR 默认不阻塞截图完成。
- [ ] Tauri event 只发送 id、路径、尺寸、状态和错误码，不发送 Base64 图像。

---

## 11. 前端目录和控件重构

### 11.1 前端目标目录

```text
src/
  app/
    App.vue
    bootstrap.ts
  features/
    capture/
      api.ts
      events.ts
      components/
        CaptureStatus.vue
        CaptureToolbar.vue
        CaptureSettings.vue
    annotation/
      api.ts
      stores/document.ts
      components/AnnotationToolbar.vue
      components/StylePopover.vue
    stitch/
      api.ts
      components/StitchToolbar.vue
      stores/stitch.ts
    history/
      api.ts
      components/
      stores/
    clipboard/
      api.ts
    ocr/
      api.ts
  infrastructure/
    tauri/
      client.ts
      commands/
      events/
  shared/
    contracts.ts
    ipc.ts
  styles/
    tokens.css
    app.css
```

### 11.2 后端目标目录

```text
src-tauri/src/
  capture/
    session.rs
    geometry.rs
    hotkey.rs
    application/
      runtime.rs
      capture_service.rs
    platform/windows/
      overlay.rs
      capture_worker.rs
      providers.rs
      renderer.rs
      sampler.rs
      wgc.rs
      dxgi.rs
      bitblt.rs
      d3d11.rs
      d2d.rs
      composition.rs
  annotation/
    document.rs
    geometry.rs
    style.rs
    painter.rs
  stitch/
    driver.rs
    matcher.rs
    canvas.rs
    model.rs
  infrastructure/
    image/
    store/
  application/
    clipboard_ingest.rs
    capture_service.rs
    annotation_service.rs
    stitch_service.rs
```

迁移要求：

- [ ] 先保持 `capture` 与 `clipboard` 依赖方向独立，再添加标注和长截图。
- [ ] 任何 `@tauri-apps/api` 只能出现在 `infrastructure/tauri/client.ts`。
- [ ] 原生 overlay 不导入 Vue、Tauri AppHandle、Store 或 OCR。
- [ ] 前端 feature API 不暴露 Win32 类型和 GPU texture。
- [ ] DTO 使用版本号，字段只传可序列化元数据。

---

## 12. 任务分阶段执行清单

### Phase 0：基线和契约冻结

- [x] 记录当前 `cargo test --lib`、`cargo check --all-targets`、`npm run typecheck`、`npm run build`。
- [x] 记录当前 F5 到 overlay visible 的 P50/P95。
- [x] 记录鼠标移动 10 秒 CPU、Private Bytes、GPU memory。
- [x] 记录确认 300x200、1920x1080、3840x2160 的 readback 字节数。
- [x] 增加 `generation`、stage timestamp、provider、readback bytes、dirty area 日志。
- [x] 冻结 capture event DTO 和错误码，不冻结内部 Win32 API。

以上各项的结果与证据见 `docs/13-screenshot-refactor-verification.md` Phase 0 章节。

### Phase 1：捕获与 overlay 解耦

- [x] 实现 `CaptureWorker` 和容量 1 mailbox。
- [x] `WM_HOTKEY` 只提交 `StartRequest`。
- [x] overlay 在 Preparing 状态继续泵消息。
- [x] Esc 可取消 worker，旧 generation 结果丢弃。
- [x] WGC 改为 FrameArrived/条件等待。
- [x] 添加 WGC timeout、BitBlt fallback 和设备移除测试。

> 真机 F5 回归额外发现并修复根因缺陷：`overlay_thread` 消息泵把 `PostThreadMessageW`
> 线程消息（`FRAME_READY_MESSAGE`/`WM_OVERLAY_COMMAND`/Shutdown）交给 `DispatchMessageW`
> 后被静默丢弃（`MSG.hwnd == NULL` 不进窗口过程），导致 overlay 永不停在 `Preparing`。
> 已改为对线程消息直接 `controller.handle(...)`。证据见 docs/13 P1.4/P1.5。

### Phase 2：渲染节奏和分层

- [x] 增加 `WM_APP_RENDER`/刷新 tick。
- [x] `WM_MOUSEMOVE` 只更新 latest state。
- [x] 一个 tick 最多一次 Present/Commit。
- [x] 创建 Static/Selection/Pointer 三层 visual 或等效 surface。
- [x] 移除全屏十字线导致的单包围盒退化。
- [x] 增加"静态层像素不变、指针层局部变化"的 GPU 回归测试。

> 实现等效路径：单 target + 多个独立 damage clip（docs/11 §5.1 line 343 允许）。局部准星
> 替代全屏十字线，hover 包围盒不再覆盖整帧；WM_TIMER 15ms tick 合并鼠标事件。

### Phase 3：选区 readback 和导出

- [ ] 实现 `GraphicsDevice::read_region`。
- [ ] 实现 `FrozenFrame::read_region`。
- [ ] BitBlt 路径直接 CPU crop。
- [ ] 将 PNG 编码移至 export worker。
- [ ] 增加选区大小与 readback bytes 的断言。
- [ ] 加入异常 RowPitch、负显示器 origin、右下边界测试。

### Phase 4：放大镜和取色

- [ ] 按 21x21/32x32 tile 统一采样几何。
- [ ] 调整 panel 布局、中心实心圆、颜色 swatch、Hex 和坐标。
- [ ] 实现 15x15/21x21、倍率、网格和边缘翻转。
- [ ] 实现三缓冲 staging + non-blocking Map。
- [ ] 增加采样 stale、队列长度和颜色更新时间日志。
- [ ] 完成黑/白/彩色背景视觉回归。

### Phase 5：工具栏与标注

- [ ] 完成 Selecting toolbar。
- [ ] 建立 `AnnotationDocument`、对象 ID、样式和 undo/redo。
- [ ] 先支持矩形、箭头、文字，再增加画笔、高亮、模糊、马赛克。
- [ ] 标注预览使用 D2D geometry；导出重放同一 document。
- [ ] 实现选中、移动、缩放、样式再次编辑和删除。
- [ ] 验收多个标注互不破坏，撤销只回退一个用户动作。

### Phase 6：长截图

- [ ] 实现方向抽象和垂直 matcher。
- [ ] 增加水平 matcher/scroll driver。
- [ ] 实现稳定等待、多带相关、重叠覆盖、sticky header/footer。
- [ ] 增加动态内容、匹配失败、尺寸上限、用户取消测试。
- [ ] UI 显示方向、进度、置信度、停止原因。

### Phase 7：应用集成和回归

- [ ] artifact 接入 history/blob。
- [ ] 可选复制到剪贴板。
- [ ] 可选 OCR 入队，不能阻塞导出。
- [ ] 前端只订阅低频 capture/stitch/annotation 状态。
- [ ] 完成安装包、热键冲突、窗口销毁、设备移除、多显示器人工验收。

---

## 13. 测试与验收矩阵

### 13.1 纯逻辑测试

- [ ] 状态机每个状态的合法/非法转移。
- [ ] generation 取消和旧结果丢弃。
- [ ] 任意方向拖拽、八向缩放、移动钳制。
- [ ] 96/120/144/192 DPI 几何。
- [ ] 负坐标和多显示器 origin。
- [ ] 尺寸标签上下翻转和溢出丢弃。
- [ ] 放大镜源区域和 panel 翻转。
- [ ] 长截图垂直/水平 shift、重叠、边界。
- [ ] 标注对象 bounds、样式、ScaleTo、undo/redo。

### 13.2 GPU/D3D 回归

- [ ] WGC texture 可直接作为 L0 D2D source。
- [ ] 选区内像素逐字节等于冻结帧。
- [ ] 选区外遮罩 alpha 正确。
- [ ] label/handle/magnifier 不进入 artifact。
- [ ] pointer layer 变化不修改 static layer。
- [ ] `read_region` 只返回选区并正确处理 RowPitch。
- [ ] staging ring 在 GPU 未完成时不阻塞。

### 13.3 Windows 人工验收

- [ ] F5 -> Preparing -> overlay visible。
- [ ] Preparing 阶段立即 Esc 可退出。
- [ ] Selecting 阶段 Esc/右键可退出。
- [ ] Enter 导出正确尺寸和像素。
- [ ] 连续 20 次 F5/取消不闪现旧选区。
- [ ] 主窗口、overlay、剪贴板窗口互不冻结。
- [ ] 多显示器、负坐标、混合 DPI。
- [ ] WGC 不可用时 BitBlt fallback。
- [ ] HDR/高对比度/深浅主题。
- [ ] 设备移除和显示器变更恢复。

### 13.4 性能验收

使用 WPA/WPR、PresentMon 和进程计数器记录：

```text
F5 -> monitor ready/provider ready/frame ready/renderer ready/visible
WM_MOUSEMOVE -> Present P50/P95
10 秒快速移动 CPU 平均/P95
确认不同选区的 readback bytes
确认峰值 Private Bytes/Working Set/GPU Dedicated/Shared
空闲 30 秒资源占用
连续 F5 的暖机收益
```

验收底线：

- [ ] overlay 消息线程不执行 WGC 首帧等待。
- [ ] 鼠标事件不与 Present 一对一绑定。
- [ ] 小选区不触发整屏 staging/readback。
- [ ] 采样队列和 frame queue 有界。
- [ ] 空闲资源回收策略有真实指标依据。
- [ ] 性能优化不能使截图像素、DPI、Esc/Enter 或历史集成回归。

---

## 14. 完成判定

只有同时满足以下条件，才可将本次重构标记为完成：

1. F5、Esc、Enter、选区移动/缩放、PNG 导出和旧剪贴板/历史功能回归通过。
2. 捕获、渲染、取色、导出线程边界符合本文件设计。
3. 放大镜中心像素、Hex、网格和颜色块视觉/逻辑一致。
4. 标注对象可再次选择、移动、调整样式、撤销和重做。
5. 长截图垂直和水平模式都能拒绝低置信度拼接，不制造空白或重复区域。
6. 小选区 readback 不再整屏复制。
7. 鼠标快速移动不造成无界队列、同步 Map 或 Tauri 高频 IPC。
8. 前后性能指标和 Windows 实测日志已保存到验证文档。
9. 没有遗留旧的全屏 CPU 合成、无效兼容层、重复实现或已知无效代码。

最终目标不是“把参考项目拼在一起”，而是形成一条清晰链路：

```text
Win32 输入
  -> WGC/DXGI 冻结 GPU texture
  -> DirectComposition 静态/选区/指针层
  -> 对象化交互和标注
  -> 选区级 GPU readback
  -> 异步编码与应用层发布
```

---

## 15. 官方 API 依据

执行具体 Win32/D3D11 改造时，以以下官方文档的 API 契约为准：

- Windows Graphics Capture：<https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture>
- `Direct3D11CaptureFramePool.CreateFreeThreaded`：<https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool.createfreethreaded>
- Desktop Duplication API：<https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api>
- `ID3D11DeviceContext::CopySubresourceRegion`：<https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11devicecontext-copysubresourceregion>
- `ID3D11DeviceContext::Map`：<https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11devicecontext-map>
- `IDXGISwapChain1::Present1`：<https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgiswapchain1-present1>
- DXGI presentation improvements：<https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/dxgi-1-2-presentation-improvements>
- DirectComposition visual tree：<https://learn.microsoft.com/en-us/windows/win32/directcomp/directcomposition-portal>
- Windows Magnification API（仅作对照）：<https://learn.microsoft.com/en-us/windows/win32/winauto/magapi/magapi-intro>
