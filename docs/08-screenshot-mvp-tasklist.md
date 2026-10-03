# SnapClip 截图 MVP 任务清单

> 目标：在 Windows 平台实现第一个可验收的原生截图闭环。
>
> MVP 范围：`F5` 唤起截图、`Esc` 取消；D3D11 截图纹理（L0）；全屏半透明暗色遮罩（L1）；选区边框、圆角、手柄、尺寸标签和放大镜（L2）。
>
> 设计约束：参考 `SnapClip-old` 的窗口、输入和 D2D 经验，但不复制其代码、模块组织或 CPU 整图合成实现。截图能力与剪贴板监听/写入保持独立模块边界。

## 1. 目标与非目标

### 1.1 MVP 必须完成

- [ ] Windows 桌面上按 `F5` 后进入截图选择状态。
- [ ] 在按键所在鼠标的显示器上创建/显示原生 Win32 覆盖层。
- [ ] 在显示覆盖层前获取当前显示器快照，避免把覆盖层自身捕获进底图。
- [ ] 底图以 D3D11 texture 保存和显示，不以 WebView Canvas 作为像素管线。
- [ ] 选区外绘制一层全屏半透明暗色遮罩，选区内保持底图清晰。
- [ ] 鼠标左键拖出选区，支持移动和八向缩放。
- [ ] 选区边框支持圆角；手柄在高 DPI 下仍有足够命中区域。
- [ ] 显示物理像素尺寸标签（宽 × 高），随选区更新。
- [ ] 显示跟随鼠标的局部放大镜和十字准星。
- [ ] 任意非空截图状态按 `Esc` 取消并隐藏窗口，释放当前会话资源。
- [ ] 捕获、选择和渲染代码不直接依赖 `ClipboardMonitor`、`arboard`、SQLite 或 OCR。
- [ ] 建立可重复的 Windows 手工验收和自动化单元测试基线。

### 1.2 明确不属于本次 MVP

- [ ] 不实现滚动截图、横向拼图、窗口/元素吸附。
- [ ] 不实现录屏、GIF、音频或硬件视频编码。
- [ ] 不实现完整标注工具（矩形、箭头、文字、马赛克等）；L2 只包含选区视觉层。
- [ ] 不实现 Tauri 全屏透明截图框。
- [ ] 不在截图线程中执行 OCR、PNG 编码、数据库写入或剪贴板写入。
- [ ] 不为截图进程引入跨进程像素传输；首期在主 Rust 进程内按需创建截图会话。

### 1.3 截图 MVP 的前置条件：解除剪贴板耦合

当前 `src-tauri/src/platform/windows/clipboard.rs` 不只是剪贴板监听器，还同时负责：

- `WM_CLIPBOARDUPDATE` 消息和 sequence 去重；
- Windows 格式读取（HTML/RTF/PNG/DIB/CF_BITMAP）；
- 图片规范化和限制；
- 来源进程解析；
- `Store::save_publication` 入库；
- OCR 入队；
- Tauri 更新事件发布。

这会让截图功能只能复制剪贴板的调用链，最终形成“截图必须依赖剪贴板”的错误边界。MVP 必须先完成以下重构，再接入 F5 和 overlay：

- [ ] 将 `ClipboardPublication` 重命名为通用的 `Publication`，从领域模型中删除剪贴板专属命名。
- [ ] 将 `PayloadData`、`PayloadRef` 和 `Publication` 放入通用 domain/storage contract；Store 不再接受 `ClipboardPublication`。
- [ ] 将剪贴板监听、格式读取、来源进程解析、入库编排拆为独立模块。
- [ ] 将 `image_norm.rs` 和 `source_app.rs` 放入 `platform/windows/clipboard/`；它们不属于截图平台层。
- [ ] 将 `Store::save_publication` 的调用从 Win32 clipboard adapter 移到 `application/clipboard_ingest.rs`。
- [ ] 将 OCR 入队和 Tauri 事件发布从 clipboard reader 中移出，由 application service 在入库成功后执行。
- [ ] 创建不依赖剪贴板的 `CaptureArtifact`；截图模块只产生 artifact，不直接调用 Store、OCR 或 `OpenClipboard`。
- [ ] 将 `copy_payload` 保留在 clipboard command adapter；capture command 只负责截图会话和 artifact 生命周期。

目标依赖方向：

```text
platform/windows/clipboard ─┐
platform/windows/capture   ─┼─> application services ─> domain/store
Tauri commands             ─┘
```

平台适配器只能产生快照、帧或 artifact；应用层负责持久化、OCR、剪贴板发布和事件通知。禁止 `platform/windows/clipboard` 直接依赖 OCR、Store 编排和 Tauri `AppHandle`。

## 2. 目标架构

### 2.1 模块边界

截图模块负责“获取底图 → 交互选区 → 产生捕获结果”，不负责剪贴板策略和历史入库。

```text
Tauri/Vue 主界面
  └─ 设置、历史、状态；不接收每个鼠标移动或图像帧

capture application service
  ├─ F5/ESC 生命周期
  ├─ CaptureSession 状态机
  └─ CaptureArtifact 结果发布

capture platform/windows
  ├─ RegisterHotKey / WM_HOTKEY
  ├─ Win32 overlay HWND 与输入
  ├─ WGC 或 DXGI Desktop Duplication
  ├─ D3D11 device/texture/swap chain
  └─ Direct2D/DirectWrite 绘制 L1/L2

clipboard service（现有模块，独立）
  ├─ WM_CLIPBOARDUPDATE
  ├─ 延迟格式读取和去重
  └─ 剪贴板发布/复制策略

application integration（后续接入）
  ├─ 将 CaptureArtifact 交给 ClipboardService
  ├─ 保存 BlobStore/History
  └─ 发送小型 Tauri 状态事件
```

剪贴板和截图共享的最小接口只有通用 payload/publication/artifact contract。两者不能共享监听器、窗口消息循环、OCR 队列或“复制到剪贴板”副作用。

### 2.2 前后端建议目录

MVP 不要求立即拆成 workspace crate，但前后端目录都必须表达职责。截图框是原生窗口，因此前端只保留命令、状态和后续工具栏，不创建全屏 `CaptureOverlay.vue`。

```text
src/
  app/
    App.vue                    # 应用壳、路由/面板组装，不直接 invoke
    bootstrap.ts               # Pinia、Tauri 事件和生命周期组装
  features/
    history/
      components/
        HistoryItem.vue
      stores/
        history.ts
      api.ts                   # 历史用例，不暴露 Tauri 细节
    capture/
      api.ts                   # start/cancel/confirm 等低频命令
      events.ts                # capture://*.v1 事件订阅
      components/              # 后续 Tauri 工具栏/状态面板；MVP 可为空
    clipboard/
      api.ts                   # copy_payload 等用户动作
    ocr/
      api.ts
      events.ts
  infrastructure/tauri/
    client.ts                  # invoke、listen 的唯一底层封装
    commands/
      history.ts
      capture.ts
      clipboard.ts
      ocr.ts
    events/
      clipboard.ts
      capture.ts
      ocr.ts
  shared/
    contracts.ts               # 与 Rust DTO 对齐的版本化契约
    ipc.ts                     # IPC_SCHEMA_VERSION、事件 envelope
  styles/
    tokens.css
    app.css

src-tauri/src/
  app/
    mod.rs                     # AppState、运行时组装和生命周期
  commands/
    mod.rs
    history.rs                 # Tauri command 适配，不写业务 SQL
    capture.rs                 # capture/start、cancel、confirm
    clipboard.rs               # copy_payload 等剪贴板命令
    ocr.rs
  events/
    mod.rs                     # 版本化事件 envelope 和定向发布
  domain/
    mod.rs
    payload.rs                 # PayloadRef、PayloadData、PayloadSource
    publication.rs             # Publication、PublicationOrigin
    capture.rs                 # CaptureArtifact、CaptureState
    error.rs
  application/
    capture_service.rs         # F5/ESC、CaptureSession、artifact 编排
    clipboard_ingest.rs        # 剪贴板快照入库、OCR 入队、事件触发
    history_service.rs
  infrastructure/
    store/
      mod.rs
      blob.rs
    image/
      encode.rs                # 真正跨来源复用的编解码能力
    config.rs
  platform/windows/
    capture/
      hotkey.rs
      overlay.rs
      d3d11.rs
      wgc.rs
      dxgi.rs
      d2d.rs
    clipboard/
      listener.rs
      reader.rs
      formats.rs
      image_norm.rs
      source_app.rs
```

### 2.3 当前文件迁移表

| 当前文件 | 目标位置 | 迁移原则 |
| --- | --- | --- |
| `src/App.vue` | `src/app/App.vue` | 只做应用壳和 feature 组装；历史列表下沉到 `features/history` |
| `src/components/HistoryItem.vue` | `src/features/history/components/HistoryItem.vue` | 组件不直接调用 `invoke` |
| `src/stores/history.ts` | `src/features/history/stores/history.ts` | store 依赖 feature API，不依赖 Tauri 类型 |
| `src/infrastructure/tauri/history.ts` | `src/infrastructure/tauri/commands/history.ts` | 只封装 `invoke` 和 DTO，不包含业务状态 |
| `src/infrastructure/tauri/icons.ts` | `src/infrastructure/tauri/commands/icons.ts` | 统一 Tauri command 入口 |
| `src/shared/contracts.ts` | 保留并拆分为 `shared/contracts.ts`/`shared/ipc.ts` | 增加 capture DTO、事件 envelope 和版本号 |
| `src-tauri/src/lib.rs` | `src-tauri/src/app/mod.rs` + `commands/mod.rs` | `lib.rs` 只保留 crate 入口和模块注册 |
| `src-tauri/src/platform/windows/clipboard.rs` | `platform/windows/clipboard/{listener,reader,formats}.rs` | Win32 读取与格式解析，不做入库/OCR/Tauri 编排 |
| `src-tauri/src/platform/windows/image_norm.rs` | `platform/windows/clipboard/image_norm.rs` | 这是剪贴板 DIB/CF_BITMAP 规范化，不是通用截图渲染 |
| `src-tauri/src/platform/windows/source_app.rs` | `platform/windows/clipboard/source_app.rs` | 只解析剪贴板 owner/foreground 来源 |
| `src-tauri/src/store/mod.rs` | `infrastructure/store/mod.rs` | Store 接受通用 `Publication`，不出现 `ClipboardPublication` |

前端依赖方向必须保持：

```text
Vue components
  -> feature api/store
  -> infrastructure/tauri commands/events
  -> Tauri IPC
```

组件和 Pinia store 不得直接导入 `@tauri-apps/api/core` 或 `listen`；只有 `infrastructure/tauri` 可以接触 Tauri。这样截图原生窗口未来替换实现时，前端只需要保持 `capture` DTO 和事件契约不变。

后端依赖方向必须保持：

```text
Tauri commands/events -> application -> domain
platform adapters     -> application/domain contracts
infrastructure/store  -> domain
```

`capture` 与 `clipboard` 可以共享 `Payload`/`Publication` 契约，但不能共享监听器、窗口消息循环、OCR 队列或复制副作用。

### 2.4 后端目录明细

MVP 不要求立即拆成 workspace crate，但后端代码目录必须表达职责：

```text
src-tauri/src/
  capture/
    mod.rs              # 对外 API、CaptureSession、CaptureArtifact
    error.rs            # CaptureError 和结构化错误码
    session.rs          # Idle/Armed/Selecting/Selected/Finishing
    geometry.rs         # 选区归一化、DPI、手柄和放大镜几何
    hotkey.rs           # F5 注册、WM_HOTKEY、注销和冲突错误
    overlay.rs          # HWND 创建、消息循环、输入和窗口生命周期
    render.rs           # L0/L1/L2 渲染调度和脏矩形
    windows/
      mod.rs
      d3d11.rs          # device、texture、swap chain、设备丢失
      wgc.rs            # Windows.Graphics.Capture provider
      dxgi.rs           # Desktop Duplication provider/降级
      d2d.rs            # Direct2D/DirectWrite 资源和绘制
  platform/windows/
    clipboard.rs        # 保持剪贴板专有实现，不被 capture 反向依赖
```

`capture` 可以依赖 Windows 平台适配层，但不能依赖 `platform/windows/clipboard`。剪贴板服务也不能通过全局状态反向调用截图内部对象。

目录调整不是单纯移动文件：每次移动都必须同时删除旧模块对 Store、OCR、Tauri 和另一平台能力的直接调用，避免形成新目录下的旧耦合。

### 2.5 进程与窗口决策

- 首期不创建常驻 `SnapClipCapture.exe`；截图会话在主 Rust 进程内按需启动。
- 截图框必须是 Rust/Win32 原生顶层窗口，不是 Tauri WebView 窗口。
- 未来若需要崩溃隔离，可整体移入按需 `SnapClipCapture.exe`；截图框、捕获、D2D 和导出不能拆成互相传输像素的多个进程。
- 工具栏不属于本 MVP。后续工具栏可使用小型 Tauri 无边框窗口，通过小型命令驱动原生覆盖层。

## 3. 状态机与用户流程

### 3.1 状态定义

```text
Idle
  -- F5 --> Armed
Armed
  -- 底图和 overlay 就绪 --> Selecting
  -- Esc --> Idle
Selecting
  -- 左键按下并移动 --> Selecting（更新 draft rect）
  -- 左键释放且非空 --> Selected
  -- Esc --> Idle
Selected
  -- 拖动选区/手柄 --> Selected（更新 rect）
  -- Enter/确认动作 --> Finishing
  -- Esc --> Idle
Finishing
  -- 产生 CaptureArtifact --> Idle
  -- 错误 --> Idle（报告 CaptureError）
```

MVP 必须在产品层确定确认动作。建议默认使用 `Enter` 产生结果，鼠标释放只完成选区而不自动结束；这样尺寸标签、放大镜和后续标注仍有稳定交互空间。

### 3.2 F5/ESC 行为

- [ ] 使用 Windows `RegisterHotKey` 注册无修饰 `VK_F5`，并处理 `MOD_NOREPEAT`。
- [ ] 注册失败返回结构化错误并记录冲突信息；不能静默假设热键可用。
- [ ] 热键线程只投递 `StartCapture`，不在 `WM_HOTKEY` 回调中创建纹理或编码图片。
- [ ] `F5` 在 `Idle` 启动新会话；在已有会话中不重复创建窗口，按产品决定是忽略还是重置当前会话。
- [ ] `Esc` 在 `Armed`、`Selecting`、`Selected`、`Finishing` 的可取消阶段终止会话。
- [ ] 取消路径撤销鼠标捕获、隐藏工具窗口、清空临时 texture 引用、恢复光标并回到 `Idle`。
- [ ] 应用退出、显示器移除、D3D 设备移除和窗口销毁都必须经过同一取消/清理路径。

### 3.3 结果契约

截图模块只产生领域结果，不直接操作剪贴板：

```rust
struct CaptureArtifact {
    session_id: String,
    width: u32,
    height: u32,
    dpi: u32,
    pixel_format: PixelFormat,
    // MVP 可为一次性 GPU/CPU 资源句柄或临时文件引用。
    payload: CapturePayload,
}
```

后续由应用层选择：

```text
CaptureArtifact
  -> ClipboardService（复制到 Windows 剪贴板）
  -> BlobStore/History（保存历史）
  -> OCR service（用户启用时异步处理）
```

这些消费者共享同一份 canonical capture；不得让截图模块为了复制、保存和 OCR 各自重新捕获或重复读取整张图。

## 4. L0：D3D11 截图纹理

### 4.1 Provider 选择

- [ ] 首选 Windows Graphics Capture（WGC）显示器捕获，输出 D3D11 texture。
- [ ] 准备 DXGI Desktop Duplication provider 作为全屏/兼容降级路径。
- [ ] 启动时探测系统版本、WGC interop 和 D3D11 能力；不只按 Windows 版本号判断。
- [ ] 如果 MVP 将最低版本限定为 Windows 10 1903+，在文档和安装检查中明确写出；若保留 1809，必须验收 DXGI/BitBlt 降级。
- [ ] provider 统一返回 `CaptureFrame`：texture、尺寸、时间戳、像素格式、provider 名称和结构化错误。

### 4.2 D3D11 资源生命周期

- [ ] 截图服务启动时创建一个 D3D11 device/context；会话之间复用轻量设备。
- [ ] 创建可绑定为 shader resource/render target 的 BGRA texture 和双缓冲 swap chain。
- [ ] 捕获底图后立即释放 WGC/DXGI frame 对象，但保持自有 texture 引用。
- [ ] 禁止每个 `WM_MOUSEMOVE` 创建 device、swap chain 或完整 CPU BGRA 副本。
- [ ] 设备移除时停止当前会话，记录 HRESULT 和 provider，并允许下一次 F5 重建设备。
- [ ] 只有确认/导出/OCR 需要时才执行 GPU → CPU readback。

### 4.3 捕获时序

```text
F5
  -> 确定鼠标所在 monitor 和物理矩形
  -> 取得一次底图 texture
  -> 显示 overlay HWND
  -> Selecting/Selected 期间只显示自有 texture，不继续捕获桌面
```

这样可以避免截图框、尺寸标签和后续工具栏被写入静态截图。若后续扩展录屏，录屏将采用独立 Recording 状态，不能复用这里的“显示后停止采集”逻辑。

## 5. L1：全屏半透明暗色遮罩

- [ ] 使用一个全屏 GPU quad/几何层填充半透明暗色，不逐像素修改底图。
- [ ] 选区区域使用四个矩形或 stencil/scissor 挖空，保持底图原始亮度。
- [ ] 遮罩颜色、alpha 和高对比度模式可配置；默认值必须通过浅色/深色背景人工验收。
- [ ] 遮罩不参与最终 `CaptureArtifact` 导出，只属于交互预览层。
- [ ] 遮罩和底图使用相同物理像素坐标，避免 DPI 缩放产生一像素错位。
- [ ] 拖拽时只使旧选区和新选区并集失效；不能每次鼠标移动刷新整个显示器。

建议的渲染层顺序：

```text
L0  D3D11 底图 texture
L1  全屏半透明遮罩 + 选区挖空
L2  边框、手柄、尺寸标签、放大镜
```

## 6. L2：选区、边框、手柄、尺寸标签、放大镜

### 6.1 选区几何

- [ ] 将鼠标屏幕坐标立即转换为当前 overlay client 的物理像素坐标。
- [ ] 拖拽起点和当前点生成归一化矩形，始终保证 `left <= right`、`top <= bottom`。
- [ ] 选区限制在当前显示器底图范围；允许从任意方向拖出。
- [ ] 支持 `Shift` 固定比例或对称扩展的行为必须在 MVP 任务中明确；若暂不实现，至少保证不崩溃且坐标稳定。
- [ ] 采用八个边/角手柄，视觉尺寸和命中尺寸独立，命中区域随 DPI 放大。
- [ ] 使用包围盒快速命中，后续标注对象增加后再引入空间索引。

### 6.2 边框和圆角

- [ ] 使用 D2D rounded rectangle 绘制圆角边框，不使用 GDI `FrameRect` 作为最终路径。
- [ ] 边框宽度按物理像素计算，至少在 100%、125%、150%、200% DPI 下保持清晰。
- [ ] 细线根据变换后的像素边界对齐；不要无条件添加 `0.5` 偏移。
- [ ] 选区边框、手柄和尺寸标签都属于预览层，不污染导出结果。

### 6.3 尺寸标签

- [ ] 使用 DirectWrite 绘制 `宽 × 高`，字体采用 Segoe UI/Segoe UI Variable。
- [ ] 标签跟随选区，优先放在选区下方；空间不足时放到上方，再限制在显示器工作区。
- [ ] 标签背景为不透明或高对比度半透明面板，避免直接叠在复杂底图上不可读。
- [ ] 标签尺寸和文字布局不影响选区几何，也不能导致窗口尺寸变化。

### 6.4 放大镜

- [ ] 放大镜使用底图 texture 的局部采样，不从 CPU 重新读取整张屏幕。
- [ ] 默认显示鼠标周围固定大小区域，倍率和面板大小保持稳定，建议首版固定为 4x 和 120×120 物理像素，再按实测调整。
- [ ] 添加十字准星和像素坐标/颜色信息的扩展点；MVP 至少实现十字准星。
- [ ] 放大镜靠近屏幕边缘时自动翻转到可见区域，不能遮挡鼠标热点或选区关键手柄。
- [ ] 放大镜只更新局部脏矩形，不能因为鼠标移动触发整屏 CPU 合成。

## 7. Overlay HWND 与 DPI

- [ ] 窗口使用 `WS_POPUP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST`。
- [ ] DirectComposition 路径使用 `WS_EX_NOREDIRECTIONBITMAP`；不要给需要接收鼠标的截图框设置 `WS_EX_NOACTIVATE`。
- [ ] 进程声明 Per-Monitor V2 DPI awareness。
- [ ] 使用真实 monitor 工作区和虚拟桌面坐标，支持负坐标和多显示器混合 DPI。
- [ ] overlay 预创建并隐藏，唤起时只更新位置、尺寸、纹理和状态。
- [ ] 维护窗口坐标、底图坐标、选区坐标三者的显式转换函数，并为转换写单元测试。
- [ ] 处理 `WM_DPICHANGED`、`WM_DISPLAYCHANGE`、`WM_DEVICECHANGE` 和窗口销毁消息。
- [ ] 不在 `WM_PAINT` 中做数据库、PNG 编码、OCR 或同步文件 I/O。

## 8. 与剪贴板代码的隔离清单

### 8.1 Capture 禁止依赖

- [ ] `capture` 不导入 `platform/windows/clipboard`。
- [ ] `capture` 不调用 `OpenClipboard`、`SetClipboardData`、`arboard` 或 `mark_clipboard_excluded`。
- [ ] `capture` 不直接访问 `Store`、`BlobStore`、OCR engine 或 `AppHandle`；如需通知，经过 application service 的 trait/事件适配。
- [ ] `capture` 的单元测试可以在没有剪贴板、SQLite 和 OCR 模型的环境运行。

### 8.2 Clipboard 允许依赖

- [ ] 剪贴板服务可以消费 `CaptureArtifact` 的文件引用/像素资源。
- [ ] 剪贴板服务负责 Windows clipboard ownership、格式发布、排除自身事件和重试。
- [ ] 剪贴板服务失败不能破坏已完成的截图会话；结果必须先成为可恢复的 artifact。
- [ ] 截图完成事件只传 session ID、artifact 引用、尺寸和状态，不传 Base64 大图。

## 9. Tauri 边界

### 9.1 MVP 命令与事件

F5 和鼠标交互由原生捕获服务处理，Tauri 不参与高频输入。可以预留以下低频协议：

```text
capture://started.v1
capture://state.v1       { sessionId, state, monitor, dpi }
capture://completed.v1   { sessionId, artifactRef, width, height }
capture://cancelled.v1   { sessionId, reason }
capture://failed.v1      { sessionId, errorCode, provider }
```

- [ ] 事件只传 typed 小消息，携带 `schemaVersion`、`sessionId` 和 `generation`。
- [ ] 不为 `WM_MOUSEMOVE`、放大镜刷新或每次绘制定义 Tauri event。
- [ ] 主 WebView 隐藏时，截图仍能通过原生热键和窗口完成；不要依赖 Vue 页面存活。
- [ ] 后续 Tauri 工具栏只发送 tool/style/confirm/cancel 命令，命令队列必须有界。

### 9.2 Artifact 传输

- [ ] MVP 优先返回受限临时文件/内容寻址引用，不通过 JSON/Base64 返回完整图片。
- [ ] 文件引用只能解析到应用 Local AppData 或明确的 artifact 目录，禁止开放任意本地路径。
- [ ] 记录 artifact 生命周期：创建、确认、复制/保存消费者完成、失败清理和超时回收。
- [ ] 大图后续评估 Tauri `Response`、`Channel` 或映射文件；共享内存不是 MVP 默认方案。

## 10. 测试与验收

### 10.1 纯 Rust 单元测试

- [ ] 任意方向拖拽都能得到正确归一化矩形。
- [ ] 选区与 monitor 边界相交时正确裁剪。
- [ ] 八向手柄命中、移动和缩放边界正确。
- [ ] DPI 96/120/144/192 下逻辑坐标到物理坐标转换正确。
- [ ] 尺寸标签在上下边界自动换位并保持可见。
- [ ] 放大镜在四个屏幕边缘自动翻转且不越界。
- [ ] `Esc`、窗口销毁、设备移除都能让状态机回到 `Idle`。
- [ ] `CaptureArtifact` 不携带剪贴板或数据库对象。

### 10.2 Windows 集成测试

- [ ] `F5` 从主窗口可见、隐藏和焦点位于其他应用时都能启动截图。
- [ ] `Esc` 在无选区、拖拽中和已选中状态都能退出。
- [ ] 截图底图不包含 overlay、尺寸标签、放大镜或其他 SnapClip UI。
- [ ] 选区拖动无明显撕裂、残影、闪烁和输入延迟。
- [ ] 100/125/150/200% DPI 下边框、手柄和文字清晰且位置正确。
- [ ] 多显示器、负虚拟坐标、不同 DPI 显示器切换正确。
- [ ] 浅色背景、深色背景、HDR 显示器和 Windows 高对比度模式可读。
- [ ] D3D 设备移除、显示器热插拔和目标窗口关闭后能恢复下一次 F5。
- [ ] 剪贴板关闭/不可用时截图核心仍可完成并产生 artifact。

### 10.3 视觉回归

- [ ] 固定底图集验证 L0/L1/L2 合成结果，记录遮罩 alpha、边框宽度和圆角半径。
- [ ] 验证放大镜倍率、采样区域和十字准星位置。
- [ ] 验证尺寸标签文本、背景对比度和显示器边界定位。
- [ ] 视觉差异测试只比较预览层；遮罩、标签和放大镜不得出现在最终导出图。

### 10.4 性能验收

记录以下指标，不能只凭主观流畅度通过：

| 指标 | MVP 目标/记录方式 |
| --- | --- |
| F5 到 overlay 首帧 | QPC 记录 P50/P95；沿用架构目标 P50 < 80ms、P95 < 150ms，若硬件差异超出需记录原因 |
| 鼠标输入到 Present | 记录拖拽期间 P95 延迟和丢帧/残影 |
| CPU | Idle、Selecting、放大镜移动三种状态分别记录 |
| 内存 | Rust Private Bytes、Working Set、GPU dedicated/shared memory |
| GPU/CPU 复制 | 记录 readback 次数；拖拽期间应为 0 次整帧 readback |
| 资源生命周期 | HWND、D3D device、texture、D2D factory 创建/释放次数 |

## 11. 实施顺序

### M0：边界和基线

- [ ] 记录当前主 crate 编译基线；不要把现有 OCR API 迁移错误归因于截图改动。
- [ ] 将 `ClipboardPublication`、`PayloadData` 和 `PayloadRef` 重新归类为通用领域/存储契约，并更新 Store 测试夹具。
- [ ] 拆分 clipboard listener、format reader、image normalization、source resolver 和 ingest service。
- [ ] 移除 `platform/windows/clipboard` 对 Store、OCR 和 Tauri 事件编排的直接依赖。
- [ ] 将前端 `invoke`/`listen` 集中到 `src/infrastructure/tauri/`，组件和 Pinia store 不再直接访问 Tauri API。
- [ ] 将 `App.vue`、历史组件和 history store 按 feature 归位，保持历史功能行为不变。
- [ ] 增加版本化 capture command/event DTO；MVP 前端只接收状态和 artifact 引用，不接收图像帧。
- [ ] 新建 `capture` 模块骨架和错误类型，确认它可以在没有 clipboard 模块的情况下编译/测试。
- [ ] 建立 CaptureSession 状态机、CaptureArtifact 契约和纯 Rust 几何测试。
- [ ] 确认最低 Windows 版本、WGC/DXGI provider 策略和 F5 冲突行为。
- [ ] 在重构后重新运行剪贴板、Store 和 OCR 相关测试，确认已有剪贴板功能没有行为退化。

### M1：热键和原生窗口

- [ ] 实现 F5 注册/注销和 `WM_HOTKEY` 投递。
- [ ] 实现预创建、显示、隐藏和销毁 overlay HWND。
- [ ] 完成 Per-Monitor V2、多显示器和物理坐标转换。
- [ ] 实现 Esc 和窗口销毁的统一清理路径。

### M2：D3D11/WGC 或 DXGI 底图

- [ ] 创建 D3D11 device、context、BGRA texture 和 swap chain。
- [ ] 接入首选 capture provider，捕获当前 monitor 的一次底图。
- [ ] 将底图显示到 overlay，确认没有 CPU 每帧复制。
- [ ] 添加设备移除和 provider 失败诊断。

### M3：L1 遮罩和 L2 选区

- [ ] 绘制全屏半透明遮罩和选区挖空。
- [ ] 实现鼠标拖拽、移动、八向缩放和边界限制。
- [ ] 使用 D2D 绘制圆角边框和手柄。
- [ ] 使用 DirectWrite 绘制尺寸标签并实现自动换位。
- [ ] 加入局部放大镜、十字准星和边界翻转。
- [ ] 实现脏矩形更新，验证拖拽期间无整图 readback。

### M4：结果和隔离验收

- [ ] Enter 生成 CaptureArtifact，Esc 丢弃 artifact。
- [ ] 通过 application service 将 artifact 接入保存/剪贴板适配器；capture 模块不反向依赖剪贴板。
- [ ] 发布版本化 capture 事件；事件不传输完整图片。
- [ ] 完成视觉、DPI、多显示器、设备移除和性能验收。
- [ ] 将验证结果和已知限制写入变更记录，再决定是否进入标注和滚动截图。

## 12. 参考资料与复用边界

### 12.1 老项目只复用的内容

- 原生 overlay HWND 的生命周期和消息路由经验。
- D3D11 BGRA + DirectComposition swap chain 的设备组织方式。
- D2D 绘制和脏矩形更新的交互经验。
- 选区几何、八向手柄、放大镜定位和 DPI 测试思路。

### 12.2 不直接复制的内容

- 老项目的整图 CPU `BgraImage` 合成路径。
- 老项目的业务状态、剪贴板发布、历史入库和 OCR 耦合。
- 老项目的工具栏框架和命令编号；SnapClip 使用版本化 typed command。
- 老项目未经本项目基准验证的缓存、线程和进程假设。

### 12.3 官方资料

- [Windows Graphics Capture](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)
- [Desktop Duplication API](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api)
- [IDXGIOutputDuplication::AcquireNextFrame](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-acquirenextframe)
- [RegisterHotKey](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey)
- [Per-Monitor DPI awareness](https://learn.microsoft.com/en-us/windows/win32/hidpi/setting-the-default-dpi-awareness-for-a-process)
- [Direct2D antialiasing](https://learn.microsoft.com/en-us/windows/win32/direct2d/guide-to-antialiasing)
- [DirectWrite](https://learn.microsoft.com/en-us/windows/win32/directwrite/introducing-directwrite)
- [DirectComposition](https://learn.microsoft.com/en-us/windows/win32/directcomp/directcomposition-portal)
- [Tauri inter-process communication](https://v2.tauri.app/concept/inter-process-communication/)

## 13. 完成定义

本 MVP 只有同时满足以下条件才算完成：

1. F5、Esc 和状态机行为在目标 Windows 版本上可重复验证。
2. L0/L1/L2 均由原生 Rust/Win32/D3D11/D2D 路径实现，未使用全屏 WebView。
3. 截图框、捕获和剪贴板代码职责分离，capture 模块可独立测试。
4. 截图底图不包含 SnapClip 覆盖层，最终 artifact 不包含遮罩、尺寸标签和放大镜。
5. DPI、多显示器、设备移除、取消和窗口销毁没有已知资源泄漏或状态卡死。
6. 性能指标、视觉回归和 Windows 手工验收结果已记录；未以“代码编译通过”替代功能验收。
