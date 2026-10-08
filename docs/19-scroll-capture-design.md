# SnapClip 滚动截图设计方案

> 状态：修订稿 v5（2026-10-08），实现前基线
> 唯一基线：**本文件**。`docs/Temp/19-scroll-capture-design-v2.md` 是未跟踪草稿（`docs/Temp/` 被 `.gitignore` 忽略），自本版起被取代。
> 适用：Windows 10/11 x64，Rust workspace + Win32 + WGC/D3D11 + GPUI 壳
> 依赖：docs/14（窗口身份、DPI、排除集、生命周期）、docs/07（GPU 帧与按需回读）、docs/22（模块边界与依赖方向门禁）、docs/23（任务粒度与验收写法）

## 0. 本版修订依据

本版合并三轮评审：

1. 对本方案本身的评审（算法、交互、竞品对照、Windows 约束）；
2. docs/23 评审中与本方案相关的门禁与验收口径；
3. 第三轮评审：线程所有权、控制窗口输入、跨 crate 类型、导出接口的可实现性。

修订原则按 `AGENTS.md`：根因优先、最小 Diff 不是目标、性能优化必须有依据、"代码改完"不等于"完成"。

相对 v2 的主要变化：

| 类别 | 变化 |
|---|---|
| 事实修正 | 运行时基线改为 workspace + `snapclip-capture` + GPUI 壳；删除 Tauri/Vue/`src-tauri` 表述 |
| 契约补齐 | 新增 `ScrollTarget`、`ActiveFrameSource`、`DriverCommand`/`DriverEvent`、controller HWND 生命周期 |
| 设计裁决 | 裁决 GPU 上下文归属（单线程串行）；裁决 v1 目标粒度（顶层窗口） |
| 边界收敛 | TileStore/导出改走端口，capture 不依赖 `snapclip-history` |
| 做减法 | 删除 journal 崩溃重放；诊断指标与测试矩阵按 v1 能力精简 |
| 一致性 | 补全状态转移表；`EndConfirmed`/`EndUncertain` 进入正式词表 |
| v4 · 线程所有权 | GPU 线程（现 capture worker）成为 immediate context 的唯一使用者；overlay 失去 context |
| v4 · 输入来源 | Esc/Enter/暂停改为会话级 `RegisterHotKey`，不再依赖 overlay 焦点 |
| v4 · 捕获排除 | controller HWND 与 overlay 一样走 `WDA_EXCLUDEFROMCAPTURE`，并给出降级层 |
| v4 · 类型契约 | 事件改为 `AppEvent::Scroll(ScrollProgress)`；词表归 `snapclip-model`；confidence 用定点 |
| v4 · 导出接口 | `ScrollTile`/`ScrollExportMeta`/`ScrollSink` 定义到可实现；v1 只导 PNG |
| v4 · 命名统一 | 复用 docs/14 已有的 `TargetKind`；删除自造的 `ElementIdentity` 占位 |
| v5 · 预览与贴图 | 吸收 `snow_shot` 的增量缩略图模型；新增 F6 原生贴图、生命周期与回归门禁 |

### 0.1 修改前基线（2026-10-08 实测）

```
cargo check --workspace --all-targets
cargo test  --workspace --lib
```

- `cargo test --workspace --lib`：475 passed / 9 ignored / 0 failed。
- `cargo check`：1 条警告 —— `unused variable: content_label`，位于 `apps/snapclip/src/history/view.rs:769`。与本方案无关，本方案不修复。
- 本方案落地后的回归必须与上述数字对比，并单独列出滚动相关新增用例。

---

## 1. 目标与边界

### 1.1 目标

1. 支持已确认截图区域内的竖向滚轮截图和横向滚轮截图。
2. 支持不同滚轮步长、平滑动画、懒加载和动态区域，使用实际位移对齐而不是固定步长拼接。
3. 交界处不重复、不重叠、不出现猜测的白色或透明空白；无法可靠对齐时停止并报告部分结果。
4. 捕获、匹配、画布和导出不阻塞 overlay 消息循环、鼠标输入和壳层 UI。
5. 为后续 PageDown、UIA ScrollPattern 和浏览器适配器保留驱动接口。
6. 通过合成帧、Windows 集成和真实窗口速度矩阵确定默认参数。
7. 滚动会话在截图框右侧提供有界的增量滚动预览；预览只显示已提交内容和当前视口，不把完整长图传给壳层。
8. F6 将当前已确认的截图产物贴到屏幕上；贴图复用同一份 canonical artifact，不重新捕获、不建立第二套编码链路。

### 1.2 非目标

- v1 不遍历 UIA/MSAA 子树；docs/14 已将深选留给 v2。
- v1 不做元素级滚动：只支持顶层窗口目标。元素级目标必须显式返回"不支持"，不得静默退化为整窗滚动（见 §3.2）。
- v1 不通过壳层事件、Base64 或 WebView Canvas 传输帧。历史 Tauri/Vue 实现已删除（P6）；此条现在只是边界声明，不再是迁移约束。
- 一次会话只锁定一个轴，不同时滚动横向和纵向。
- v1 `WheelDriver` 只向前滚动（垂直向下 / 水平向右），不做双向。
- 滚动截图不复用视频/GIF 编码器；完成后才导出。v1 **只输出 PNG**：当前仓库没有任何 WebP 编码路径，`CaptureArtifactStore` 也只写 PNG（见 §8.3），WebP 等编码器落地后再评估。
- 不将 Crisp 的 GDI/BitBlt CPU 管线直接带入 SnapClip。
- 不把浏览器的"全页截图"宣传能力误写成 SnapClip 可直接调用的通用 Windows API；浏览器路径必须是单独适配器，并经过目标浏览器能力探测。
- F6 贴图不是 v1 的第二种截图源：没有已写入的 `ArtifactRef`（普通截图或滚动 `Ended` 的完整/Partial 产物）时，F6 必须明确忽略并记录原因；`Selected` 或仍在 `Exporting` 的像素不能直接贴图，不得偷偷启动新的捕获会话。

---

## 2. 运行时基线

本节是 v2 缺失的部分。实现者必须先对齐这里的事实，再读后续章节。

### 2.1 工作区结构

```
crates/snapclip-model     领域值与低频事件（AppEvent、CaptureArtifact、CaptureState）
crates/snapclip-capture   截图能力：WGC/BitBlt、D3D11/D2D、overlay、选区状态机、窗口检测
crates/snapclip-history   历史库、artifact 落盘、blob
apps/snapclip             GPUI 壳（事件总线、托盘、剪贴板、history 视图）
```

- 捕获实现位于 `crates/snapclip-capture`，Windows 专用代码在其 `src/windows/` 下；`platform/windows/capture/*` 这层转发目录已随 docs/22 的 P1 删除。
- WGC provider：`crates/snapclip-capture/src/windows/win/wgc.rs`
- D3D11 wrapper：`crates/snapclip-capture/src/windows/win/d3d11.rs`
- 低频事件：`snapclip_model::AppEvent`（`Capture` / `Clipboard` / `Recognition`），由壳层 typed channel 分发。
- docs/22 已把"滚动截图"划归 `snapclip-capture` 的职责范围（docs/22 §6）。

### 2.2 当前捕获实现的实际形态

| 事实 | 证据 |
|---|---|
| WGC 是显示器级捕获，`CreateForMonitor`，没有窗口级路径 | `win/wgc.rs` `create_item_for_monitor` / `CreateForMonitor` |
| 每次会话只取一帧后立刻释放 pool | `win/wgc.rs` `capture_monitor`，`FIRST_FRAME_TIMEOUT = 1500ms` |
| FramePool 由 `CreateFreeThreaded` 创建，缓冲数为 2 | `win/wgc.rs` |
| `FrozenFrame` 的语义是"一次会话的一张冻结帧"，CPU 像素惰性读回 | `windows/providers.rs` |
| 帧回读是同步的：每次调用新建 staging texture 并阻塞 `Map(D3D11_MAP_READ)` | `win/d3d11.rs` `read_back_bgra`、区域版 `read_region`（`CopySubresourceRegion`） |
| 唯一的异步回读是放大镜专用：3 槽、32×32 BGRA、`DO_NOT_WAIT` | `win/d3d11.rs` `AsyncSampleBuffer`（`SAMPLE_TILE = 32`、`SLOT_COUNT = 3`） |
| immediate context 是单线程的，`GraphicsDevice::context()` 的注释即写 "same thread usage constraint as D2D" | `win/d3d11.rs:103` |
| 今天真正使用 context 的只有两个 overlay 线程侧调用点：放大镜 `AsyncSampleBuffer` 与 `FrozenFrame::read_region` | `windows/renderer.rs:135`、`windows/providers.rs` |
| WGC 捕获与 capture worker 不碰 immediate context（没有 `CopyResource` / `Map` / `context()` 调用） | `win/wgc.rs`、`windows/capture_worker.rs` |
| D2D 绘制只经 `device.create_d2d_context()`，不需要 immediate context | `win/d2d.rs:434` |
| overlay 已用 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` 把自己排除出捕获 | `windows/overlay/window_host.rs` `exclude_overlay_from_capture` |
| capture worker 拥有 D3D11 device，跨会话复用；device 只在显示器/设备变化时重建 | `windows/capture_worker.rs` 模块注释 |
| 依赖门禁：`cargo tree -p snapclip-capture` 不得出现 `tauri`/`wry`/`gpui-kit` | `tools/check-dependency-direction.ps1`，docs/23 T1.9 |

必须纠正的两处 v2 误读：

1. v2 说"已有异步 readback slot（现状 3 槽），滚动实现应优先复用该 wrapper，2/3 槽由基准决定"。这条不成立：`AsyncSampleBuffer` 是 32×32 放大镜取色专用（固定尺寸、固定 3 槽、只接 32×32 tile），而真正的帧回读是同步的、每次新建 staging。滚动截图需要的是新增一个泛化的区域异步回读（可配尺寸与槽数，复用 `DO_NOT_WAIT` 的模式而非该对象）。这是 S2 的主要工作量，不是零成本复用。
2. v2 说"WGC 窗口捕获可以在 overlay 可见时继续工作"。当前 provider 是显示器级，拍的是合成后的桌面，overlay 可见就必然入镜。见 §5。

### 2.3 参考项目与失效引用

- 参考实现只读：`refer/shot-refer/Crisp-main`、`refer/shot-refer/ShareX-develop`、`refer/snow-apps/snow_shot`（`refer/` 被 `.gitignore` 忽略，不入库）。只吸收算法原则、贴图生命周期和测试思想，不复制其 GDI/Qt/C++/Avalonia 管线。
- `SnapClip-old/docs/04-scroll-stitching.md` 已不存在（旧目录整体移除）。其原则（容量 1 帧信箱、position/max_depth、rejected 与零位移分离、静态边缘检测、硬上限）已并入本文件 §4.4 / §7 / §8，不再引用外部路径。
- v2 §2.5 与 §2.6 的两张"源码证据"表逐行重复，§2.4 与 §2.6 的两张裁决表高度重叠；本版各合并为一张。
- v2 声明"逐项记录 `docs/Temp/review-report1.md` 和 `review-report2.md` 的建议"，但 `review-report1.md` 评审的是 docs/23 模块化重构，全文没有滚动截图内容；只有 `review-report2.md` 评审本方案。该归属错误已删除。

### 2.4 可吸收的参考结论

| 来源 | 可吸收项 | 不采用 / 不照搬 |
|---|---|---|
| Crisp `ScrollCapture.cpp` | 真实 `SendInput` 滚轮；光标停放；等待期间泵送消息；方向探测 | 固定 settleMs、固定 notches |
| Crisp `Stitch*.cpp` | 先规划所有 accepted shift 再分配输出；共享轴向搜索核；从滚动区域三分之一开始；band early-exit；像素差预算；尺寸/最大边长保护；sticky header/footer 先检测并只复制一次 | GDI/BitBlt CPU 管线、全部帧留内存、固定阈值；匹配失败必须停止，不把“最接近”当成功 |
| Crisp `TestStitch.cpp` | 已知位移、固定页眉/页脚、重复纹理、无关帧、尺寸不一致、水平/垂直和高 footer 使搜索失效等回归模型 | 不把测试中的固定 40/80 px、颜色纹理或单一阈值直接变成生产默认值 |
| Snow Shot cadence | EWMA 成本、队列压力降速、连续健康样本才恢复 | cadence 不参与正确性判断 |
| Snow Shot overlay/worker | 滚动模式独立交互状态、暂停/恢复、捕获 worker 与 UI 解耦 | — |
| Snow Shot `ScreenshotScrollingThumbnailWidget` | 固定 128 px 交叉轴缩略图；沿滚动轴以 256 px tile 增量追加/替换；视口高亮、裁剪边界和 hover 映射只更新局部脏区 | 不复制 Qt widget、QImage 或整套 UI；Rust 只吸收数据模型和更新语义 |
| Snow Shot `ScreenshotScrollingPipeline` | 每次 stitch commit 只生成 preview patch；首帧 replace，后续按 append/prepend 更新，并用重叠行修正缩放取整漂移 | 不在每个鼠标移动或原始帧上重建完整预览 |
| Snow Shot `ScrollingHoverPreview` | hover 请求单飞、带 epoch/content revision，旧回调丢弃；必要时短暂停止采帧后再读取 viewport | v1 可先只实现视口框；若实现 hover 放大，必须沿用单飞与过期结果丢弃 |
| ShareX `ScrollingCaptureManager`/`ScrollingCaptureWindow` | 选择窗口后显示可点击穿透的区域边框；滚动方法/步长/延迟可配置；底部动态排除；“best guess”只能输出 `PartiallySuccessful`；完成后提供可拖拽平移的结果预览 | 逐帧整图 `Bitmap`、固定 delay 和异步重建整图不进入 SnapClip 热路径；完成后预览不替代实时右侧 patch 预览 |

### 2.5 Windows 官方约束

- SendInput 将输入串行插入系统输入流，受 UIPI 完整性级别限制；失败记录 `GetLastError`，并恢复 foreground、focus、cursor。
- WGC `Direct3D11CaptureFramePool` 适合窗口/显示器 GPU 帧；每帧消费后立即释放。
- `CreateFreeThreaded` 可避免把重处理放进 UI dispatcher；帧的 `ContentSize` 与 `SystemRelativeTime` 必须保存到 `ScrollFrame`。
- DXGI Output Duplication 是显示器级降级源；WGC 不可用时才用于桌面区域，不能替代窗口级 WGC。
- `CreateForWindow`/`CreateForMonitor` 互操作入口从 Windows 10 1903 起可用；启动时按能力探测，并记录实际 provider。
- DwmFlush 仅用于捕获前合成同步，不得进入鼠标移动、匹配或滚动等待热路径。
- D3D11 staging 的 `Map` 可能因 GPU/CPU 同步产生 pipeline stall；必须用异步 copy、有限 staging slot 和小范围读回。

官方资料：

- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput
- https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/screen-capture
- https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nn-dxgi1_2-idxgioutputduplication
- https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmflush
- https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3ddevicecontext-map

竞品 / 产品资料（能力与交互对照，不作为 API 依赖）：

- https://getsharex.com/docs/scrolling-screenshot.html
- https://www.microsoft.com/en-us/edge/features/screenshot
- https://support.mozilla.org/en-US/kb/take-screenshots-firefox
- https://www.screenpresso.com/releases/screenpresso-2-2-8/
- https://www.techsmith.com/snagit/

---

## 3. 会话、滚动目标与交互

### 3.1 会话契约

滚动会话从普通截图的已确认选区（代码中的 `CaptureState::Selected`，旧稿称 Settled）进入。

`ScrollSession` 保存：

- `session_id`、`generation`；
- `ScrollTarget`（见 §3.2）；
- `monitor`/DPI、`screen_region`（显示器本地物理像素）；
- `ScrollAxis`（Vertical / Horizontal）；
- source provider、driver、matcher、canvas、`request_id`；
- 原始 cursor、foreground/focus 和 exclusion 快照。

开始后锁定目标与区域。所有异步结果校验：

```
session_id + generation + source_identity + request_id
```

窗口关闭、HWND 重用、DPI/显示器变化或 exclusion 变化时停止当前会话，不把新几何混入旧画布。结束、取消和失败都立即释放旧帧、matcher、canvas、tile cache、frame pool、staging texture 和临时文件。下一次 F5 不得读取任何旧滚动状态。

单显示器裁剪（沿用 docs/14）：当前截图会话只捕获当前显示器，跨屏目标只滚动/拼接当前显示器可见部分。`screen_region` 必须按此裁剪，不承诺整窗。

坐标统一：capture / match / canvas / 导出全程使用物理像素，不得重复乘 DPI。DPI 只用于 overlay 的逻辑布局。

### 3.2 ScrollTarget：滚动目标契约

v2 只保存 `WindowIdentity + screen_region`，不足以支撑"每步重新验证目标"。改为：

```
struct ScrollTarget {
    identity: WindowIdentity,      // hwnd + process_id + class_name_hash
    kind: TargetKind,              // 复用 window_detection::model 的既有类型
    screen_bounds: Rect,           // 虚拟桌面物理像素
    visible_clip: Rect,            // 裁剪到当前显示器后的可见区
    snapshot_epoch: u64,           // docs/14 的快照版本
    axis_capabilities: AxisCapabilities,
}

struct AxisCapabilities {
    vertical: AxisCapability,
    horizontal: AxisCapability,
}

enum AxisCapability { Native, ShiftFallback, Unsupported, Unknown }
```

`TargetKind` 不是新类型：它是 `crates/snapclip-capture/src/window_detection/model.rs` 的既有枚举（`TopLevelWindowFrame` / `ClientArea` / `UiElement`），docs/14 已定稿同名。本方案不另造一套，也不引入 `ElementIdentity` 占位字段 —— 元素身份等 docs/18 / docs/20 真正落地时再定义，现在预留只会造出两套类型。

规则：

- 每一步滚动前重新验证 HWND、PID/class hash、`screen_bounds`、`snapshot_epoch`；任一不符即 `WindowChanged`，停止并把已有结果标记为 Partial。
- v1 只接受 `TargetKind::TopLevelWindowFrame`。`ClientArea` 与 `UiElement` 一并返回 `TargetUnsupported` 并提示原因，不得静默退化为整窗滚动。
- `axis_capabilities` 由能力探测写实，不假设存在（见 §6.4）。

### 3.3 状态机与转移表

顶层 `CaptureState`（`snapclip_model::capture`）保持不变：

```
Idle / Preparing / Armed / Selecting / Selected / Adjusting / Annotating / Exporting
```

滚动不新增顶层状态，而是从 `Selected` 进入一个二级状态机 `ScrollState`，由事件上报：

```
ScrollArmed
  -> CapturingFirstFrame
  -> Scrolling            // 内部逐步：Injecting -> WaitingFrame -> Matching -> Committing
  -> Paused
  -> Ended                // Completed / Partial / Cancelled / Failed
```

转移表：

| 当前 | 事件 | 下一状态 | 结果 |
|---|---|---|---|
| ScrollArmed | 首帧就绪并稳定 | Scrolling | 首帧整幅写入画布 |
| Scrolling | `Accepted` 位移 | Scrolling | 只新增 union 外条带 |
| Scrolling | 用户暂停 | Paused | 保留画布，暂停注入 |
| Paused | 继续 | CapturingFirstFrame | 旧首帧作废，重新起拼 |
| Scrolling | `EndConfirmed` | Ended | Completed |
| Scrolling | `EndUncertain` | Ended | Partial |
| Scrolling | 目标变更 / 设备丢失 / 超限 | Ended | Failed 或 Partial（按已有内容） |
| Scrolling | Esc / 用户取消 | Ended | Cancelled |

`EndConfirmed` 与 `EndUncertain` 都是正式状态；`EndUncertain` 同时是 `ScrollStopReason` 成员（见 §4.6），v2 中"用了 `EndUncertain` 却没定义"的不一致已消除。

### 3.4 用户操作

- 工具栏明确提供"竖向滚动截图"和"横向滚动截图"。
- 光标停在选区中心，发送真实滚轮；结束后恢复光标和前台窗口。
- 用户可以暂停、继续、取消；暂停后可移动/缩放选区，继续时从新首帧开始。
- Enter 完成已拼出的结果；Esc 取消并清理。
- 对齐失败默认保留已拼部分并显示停止帧、原因和是否可导出，不生成伪完整图。

### 3.5 输入失败时的手动模式

v1 主路径是 `SendInput`，但保留 `ManualPanoramaDriver`：用户自行滚动，SnapClip 只监听稳定帧、估计位移并提交画布。适用于 UIPI 管理员窗口、远程桌面、虚拟机、特殊控件或安全软件拦截输入的场景。

手动模式不能把"收到帧"当成"滚动成功"；仍使用相同 `Alignment`、End Confirmation、CanvasStore 和资源上限。输入失败提示必须区分 `InputRejected` 与 `NoMovement`，不得要求用户无依据地以管理员运行。

---

## 4. 帧源、线程与资源

### 4.1 ActiveFrameSource：活动帧源契约

滚动不能用一次性 `capture_monitor()` 循环冒充活动捕获，也不能把现有 `FrozenFrame` API 改成隐式循环接口。新增独立、会话持有的接口：

```
enum FramePoll {
    Frame(CapturedFrame),
    Idle,                   // timeout 内没有新帧（不是 EOF）
    Ended(ScrollStopReason) // 目标消失 / 设备丢失 / 尺寸变化
}

trait ActiveFrameSource {
    fn start(&mut self, target: &ScrollTarget, region: Rect) -> Result<(), CaptureError>;
    /// 最多阻塞 `timeout`，可被 `cancel()` 提前打断；绝不忙等。
    fn next_frame(&mut self, timeout: Duration) -> Result<FramePoll, CaptureError>;
    fn cancel(&mut self);
    /// 幂等，可重复调用；由 GPU 线程调用，Drop 也走同一条路径。
    fn stop(&mut self);
}
```

必须明确的语义：

- 捕获项选择：顶层窗口用 `CreateForWindow(hwnd)`；仅当 ① 系统不支持窗口级互操作（< Win10 1903）、② 对该 HWND 建 item 失败（最小化 / 被 DWM 排除 / 已销毁）、③ 运行时拒绝窗口级捕获 时，才降级到 `CreateForMonitor`。**跨屏目标不是降级理由**：窗口级捕获返回整窗内容，按 §3.1 裁到当前显示器可见区即可。不得把显示器捕获裁一块当成窗口捕获 —— 目标被遮挡或移动时会拼出错误内容。
- 互操作实现：`IGraphicsCaptureItemInterop::CreateForWindow(hwnd)`。`win/wgc.rs` 已经有同一条入口（`factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()`），现在只调了 `CreateForMonitor`，S2 加的是窗口分支与能力探测。
- 返回值语义：`Idle` 只表示"这段时间没有新帧"，**不得**当成 EOF（EOF 一律由 §6.7 的二次确认判定）；`Ended` 由尺寸变化 / 目标失效 / 设备丢失产生，直接映射到 §4.6 的停止原因。
- 取消：`cancel()` 幂等，置位后 `next_frame` 必须在下一次唤醒时返回（`AtomicBool` 或 cancellation token），取消不 join 线程。
- 停止：`stop()` 幂等，释放 frame pool 与 session 纹理；`Drop` 走同一路径，重复调用无副作用。
- 帧的 GPU 所有权：`CapturedFrame` 携带 `GpuFrame`（device 纹理 + 尺寸）。它是"只能在 GPU 线程上做 context 操作"的资源，跨线程只传所有权句柄，不传裸 context。
- FramePool 所有权：pool 与 session 同生命周期，由 GPU 线程独占创建与销毁；frame 消费后立即释放，纹理复制到 session 自有纹理。
- 回调线程：`CreateFreeThreaded` + 事件等待，禁止在 UI dispatcher 上等待新帧。
- 尺寸变化：`ContentSize` 变化（窗口 resize、DPI 变化）→ 释放旧 pool，返回 `Ended(WindowChanged | DpiChanged)`；v1 不重建 pool 继续拼，也不把两种尺寸混入同一画布。
- 时间戳：保存 `SystemRelativeTime` 与 QPC，用于"是否有新帧"和动画阶段判断。
- 遮挡语义：窗口级捕获在窗口被遮挡时仍返回该窗口内容（这是选它的理由）；若实际 provider 退化为显示器级，overlay 与其它窗口会入镜，必须走 §5 的隐藏路径，并在诊断里记录 provider。

### 4.2 GPU 上下文归属（v4 裁决）

v3 的写法自相矛盾：一边说 capture worker 独占 immediate context，一边把 `read_region` 留给 overlay —— 而这两处用的是同一个 context。§2.2 的实测把前提钉死了：

- D2D 绘制只经 `device.create_d2d_context()`，Present 也只经 swap chain，**都不需要 immediate context**；
- 今天真正用 context 的只有 overlay 线程的两处：放大镜 `AsyncSampleBuffer` 与 `FrozenFrame::read_region`；
- WGC 取帧与 capture worker 完全不碰 context。

裁决：**GPU 线程（即今天的 capture worker，本方案起称 GPU 线程）是 immediate context 的唯一使用者；overlay 线程不再做任何 context 操作。**

- GPU 线程串行执行：`ActiveFrameSource` 取帧 → crop →（可选）降采样/预处理 → 有界区域回读。它还负责按需服务其它模块：
  - 滚动匹配需要的 MatchView 与窄条回读；
  - 确认导出需要的 `read_region`（原在 overlay 线程）；
  - 放大镜 32×32 取色采样（原在 overlay 线程）。
- overlay 线程保留 D2D 绘制、DirectComposition / Present、窗口与消息处理 —— 这些只需要 `device()`。
- matcher / canvas / tile 是纯 CPU，只消费已回读的小数据。
- 不引入 deferred context，也不引入第二个 device。

前置重构（S2 的第一件事，本身不属于滚动功能）：

1. `FrozenFrame::read_region` 的一次性回读改为向 GPU 线程提交请求、取回 `Vec<u8>`；
2. `AsyncSampleBuffer` 取色改为 GPU 线程服务：overlay 提交采样点，GPU 线程回读并把结果推回。

已知代价与门禁：放大镜取色从"overlay 本地轮询一个槽"变成一次跨线程往返，可能多一帧延迟。S2 必须保留一条可测量的门禁（放大镜取色延迟 P95 与现状对比）；若确实退化，解法是让 GPU 线程**主动推送**采样结果（仍然只有一个写者），而不是把 context 交回 overlay。

可测性：`GraphicsDevice::context()` 记录并校验调用线程，debug 构建下非 GPU 线程调用直接 panic，让这条不变量成为可执行的断言而不是文档约定。

### 4.3 线程预算

禁止每个 session 创建新线程。固定预算：

| 线程 | 归属 | 职责 | 是否新增 |
|---|---|---|---|
| overlay 消息线程 | 既有 | 消息泵、controller HWND、D2D 绘制与 Present、注册会话级热键（Esc/Enter/暂停/取消） | 否 |
| GPU 线程（现 capture worker） | 既有，扩展 | immediate context 的唯一使用者：`ActiveFrameSource`、crop、预处理、区域回读、`read_region` 服务、放大镜采样 | 否 |
| scroll driver | 新增 1 个常驻 | SendInput、稳定等待、CPU 匹配、画布提交编排 | 是 |
| export worker | 既有 | 编码、流式导出、原子落盘 | 否 |

整体只增加 1 个常驻线程，队列全部有界。只有 GPU 线程可以调用 `GraphicsDevice::context()`（§4.2）。若 canvas 落盘成为瓶颈，只有在 profiling 证据下才再拆线程。

### 4.4 容量 1 信箱与丢帧语义

- 帧信箱容量为 1，最新帧覆盖旧帧；禁止无界 Vec/Channel、无界临时文件、无界编码队列。
- 帧生命周期必须可诊断：`Requested -> InFlight -> Dropped -> Arrived -> MatchViewReady -> Accepted/Rejected`。
- `Dropped` 不得当作 `NoMovement`；`NoNewFrame` 不得当作滚动到边界。
- 跨多位移的丢失帧：若检测到丢帧（计数增加或 QPC 跳变超阈值），下一帧可能跨越多个滚轮位移。此时必须扩大搜索窗（`expected_delta` 容差按跳跃步数上限放大）；仍不唯一则返回 `Uncertain`，重采集稳定帧或降低步长，不得继续按普通相邻帧拼接。

### 4.5 资源生命周期

- 复用 GPU 线程已有的 device；滚动 session 只是 device 的一个消费者。
- device 只在显示器/设备变化时替换，替换与 renderer 重建是同一时刻；滚动 session 必须在此时释放自己的 frame pool 并结束。renderer 重建同样发生在 overlay 线程，但它只重建 D2D/合成目标，不重建 context 使用者。
- 每个滚动会话拥有独立资源组，Drop 路径必须能在取消、设备移除、窗口销毁和重复启动时执行。
- staging 使用 `DO_NOT_WAIT` 时若返回仍在使用，丢弃该次 MatchView 并记录 `readback_not_ready`，由信箱最新帧继续推进；不得在 GPU 线程的回调里忙等 GPU，也不得把等待转嫁到 overlay 线程。

### 4.6 建议类型

```
enum ScrollAxis { Vertical, Horizontal }

struct Alignment {
    axis: ScrollAxis,
    signed_delta_px: i32,
    overlap_px: u32,
    confidence: f32,
    residual: f32,
    status: AlignmentStatus,   // Accepted | NoMovement | Rejected | Uncertain
}

struct ScrollFrame {
    session_id: ScrollSessionId,
    source: WindowIdentity,
    axis: ScrollAxis,
    qpc: i64,
    source_size: PhysicalSize,
    crop: PhysicalRect,
    texture: GpuFrame,          // 项目已有 D3D11 wrapper，明确的 Arc 所有权
    match_view: MatchView,
    request_id: u64,
}
```

`Alignment` 是 capture 内部的算法结果、不跨事件边界，所以这里可以用 `f32`；只有进入 `AppEvent` 的 `ScrollProgress` 才受 `Eq` 约束（§10.3）。

`ScrollFrame` 不能跨线程携带没有明确 apartment/device 约束的裸 COM 指针；使用项目已有的 `GpuFrame`/`FrozenFrame` 所有权模型。

停止原因统一为：

```
enum ScrollStopReason {
    EndConfirmed,               // 帧证据 + 二次 probe 成立
    EndConfirmedByUiAExtent,    // UIA extent 作为额外证据（未来 driver）
    EndUncertain,               // 仍有加载迹象但超时
    InputRejected,
    NoNewFrame,
    AlignmentRejected,
    DriftBeyondBudget,          // 全局重锚定与链路预测差异超预算
    UncertainAfterRetries,
    TargetUnsupported,          // v1：元素级目标等
    HorizontalUnsupported,      // 目标不支持原生横向滚动
    WindowChanged,
    DpiChanged,
    MonitorChanged,
    DeviceLost,
    ResourceLimit,
    UserCancelled,
}
```

---

## 5. Overlay 输入隔离与控制窗口

这是滚轮截图的前置条件，不是视觉优化：全屏 overlay 如果继续命中鼠标，SendInput 会被 overlay 消费，目标窗口不会滚动。同时注意 §2.2 的事实：当前 provider 是显示器级捕获，overlay 可见就会入镜。

### 5.1 v1 采用方案：隐藏 overlay + 独立 controller HWND

```
ScrollInputPassthrough:
  1. 隐藏全屏交互 overlay（不再命中鼠标，也不再入镜）
  2. 创建独立的 controller HWND：小尺寸、置顶、不抢焦点
  3. 滚轮注入由 scroll driver 完成，controller 不接收滚轮
  4. 对 controller 施加 SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)
```

键盘来源（v3 漏掉的关键一条）：overlay 隐藏、目标窗口持有焦点之后，controller 收不到 `WM_KEYDOWN`，Esc / Enter / 暂停**没有输入来源**。所以 v1 不能一边说"controller 不抢焦点"、一边指望它处理快捷键。采用会话级热键：

- 进入 `Scrolling` 时由 overlay 线程 `RegisterHotKey` 注册 Esc / Enter / 暂停键，会话用独立 ID（与 F5 的 `CAPTURE_HOTKEY_ID` 分开）；`WM_HOTKEY` 会 post 给注册它的线程，隐藏窗口照样收得到。
- 全局截图入口另注册 F6（`PIN_CAPTURE_HOTKEY_ID` 与 F5、会话级热键 ID 不同）。F6 是“将当前可贴图的截图产物新建为置顶原生窗口”的命令，不在滚动采集期间抢占滚轮/会话热键；滚动 `Ended` 并产生完整或 Partial artifact 后可用。尚无可贴产物或正处于未完成的选择/导出状态时忽略并记录状态，不创建空窗口。
- 退出会话的**任何**路径都要 `UnregisterHotKey`。
- 注册失败（被其它程序占用，Win32 错误 1409）时的降级：controller 变为可获取焦点，`SW_SHOW` + 取前台；会话结束时恢复原前台窗口与焦点。

`hotkey.rs` 里"Esc / Enter 只从 `WM_KEYDOWN` 读"的选择在滚动模式下不适用：那条注释的前提是 overlay 拥有会话，而滚动时 overlay 不拥有焦点。

捕获排除：controller 与 overlay 一样调用 `exclude_overlay_from_capture`（`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`）。该 API 需要 Windows 10 2004+；普通截图路径可以不在乎它失败（帧在 overlay 显示前就已冻结），滚动路径**不行**，因为帧是持续采的。因此 controller 需要一层降级：affinity 调用失败时，controller 在 `Scrolling` 期间保持隐藏、只在 `Paused` 显示（暂停时不采帧），控制全部走热键。

备选方案（v2 再评估）：保留 overlay，用经过真实集成测试验证的跨进程穿透。`WS_EX_TRANSPARENT` 或 `HTTRANSPARENT` 都不是无条件保证，不得只加样式就宣称穿透。

### 5.2 controller HWND 生命周期（v1 必须完整）

| 阶段 | 契约 |
|---|---|
| 创建 | 进入 `Scrolling` 前创建；消息 post 到 overlay 线程处理，不自建消息泵 |
| 捕获排除 | `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`；失败则退化为"`Scrolling` 期间隐藏 controller"（§5.1） |
| 热键 | 由 overlay 线程注册会话级 Esc / Enter / 暂停热键，退出时注销 |
| 焦点 | 默认 `ShowWindow(SW_SHOWNOACTIVATE)`，绝不 `SetForegroundWindow`；仅在热键注册失败时才取焦点，并记录原前台窗口 |
| 暂停 / 继续 | 热键或 controller 按钮 → 发命令给 driver → 状态经事件上报 |
| 完成 | Enter → 结束会话，进入导出 |
| 取消 | Esc → 取消会话，保留 Partial 结果 |
| 销毁 | 任何路径（含异常、目标窗口关闭、设备丢失）都必须注销热键、销毁 controller、恢复 overlay 命中模式与前台窗口 |

### 5.3 焦点与光标契约

- driver 在注入前把光标移到目标 region 中心；注入与等待都在 driver 线程。
- `WindowFromPoint` 可能返回子控件，必须用顶层 ancestor / PID / class hash 解析后与 `ScrollTarget.identity` 比较，并同时验证 foreground；不匹配时不发送滚轮。
- 会话结束后恢复原始 cursor 位置、foreground 窗口和焦点。
- 每一步滚轮、稳定帧采集和状态更新完成后，overlay 不得重新抢焦点。

### 5.4 验收测试（必须真实执行）

1. 目标窗口确实收到滚轮并滚动（位置变化可观测）。
2. controller 与 overlay 都不收到滚轮。
3. overlay 隐藏时 Esc / Enter / 暂停热键可用；热键被占用时的降级路径也走得通。
4. 取消 / 异常后前台窗口、焦点、光标全部恢复，且热键已注销。
5. 显示器级降级路径下，overlay **与 controller** 都不出现在捕获结果里（像素级断言）。

### 5.5 F6 贴图契约

- F6 在热键线程只投递 `PinArtifact { artifact_id, generation }` 命令；不在 `WM_HOTKEY` 中编码、读完整长图或直接创建/绘制窗口。
- 可贴对象是最近一次已完成普通截图或滚动截图（包括明确标记的 Partial）的 canonical artifact。滚动结束前不能 pin 正在变化的 tile/canvas；Partial 必须在贴图和后续历史中保持 Partial 元数据。
- `snapclip-capture` 负责呈现窗口及输入交互；artifact 查找与像素读取经既有应用端口完成，避免 capture 依赖 history。贴图窗口使用原生轻量窗口，不为每个贴图创建 GPUI 窗口/WebView；D3D device 等共享资源按现有 capture GPU 所有权复用，不为每个窗口新建 device/context。
- 成功后窗口置顶、不激活、不进入 Alt+Tab；屏幕坐标使用物理像素，跨显示器/DPI 改变时重新映射或有界夹回有效工作区。窗口关闭、显示器移除、进程退出时释放纹理、artifact lease 与 HWND；多贴图采用有界数量，容量依据实测冻结，不在设计阶段拍固定值。
- F6 不要求正在显示主窗口；重复 F6 对同一 artifact 的行为明确为创建独立贴图还是聚焦已有贴图，实施前在 S3.10 冻结并写测试，禁止隐式重复大图解码。
- 错误路径（artifact 失效/读取失败/尺寸超预算/device lost/窗口创建失败）不得留下空窗口或半初始化资源，返回可诊断错误并允许再次 F6。

验收：普通截图与滚动完整/Partial artifact 均能 F6 贴图；确认贴图像素与 artifact 解码像素逐像素一致；窗口置顶不抢焦点、可拖动/关闭且不进入截图帧；连续创建/关闭后 HWND、纹理、句柄与 Private Bytes 回落；F6 与 F5、会话 Esc/Enter/暂停 ID 不冲突，所有退出路径注销相应 hotkey。

### 5.6 右侧滚动预览契约

滚动预览是 overlay 会话的一部分，位置固定在选区右侧，空间不足时按“左侧 → 选区上方/下方”的顺序翻转并夹回当前显示器工作区；横向滚动时预览可旋转为横向布局，但仍保持“贴近选区、可见、不遮住选区”的约束。预览窗口使用 `SW_SHOWNOACTIVATE`/工具窗口样式，不抢目标焦点，并调用 `WDA_EXCLUDEFROMCAPTURE`；显示器级捕获下若排除失败，预览必须隐藏而不是冒险入帧。

预览模型只接受 stitch commit 产生的 `PreviewPatch`，不接受原始帧：

```
PreviewState {
    session_id, generation,
    axis, source_size, viewport_size,
    extent_px,                 // 已提交内容沿滚动轴的长度
    patches: bounded tile list, // 固定交叉轴缩略图，按逻辑位置有序
    visible_range,              // 当前捕获视口在 source 坐标中的区间
    committed_range,            // 可导出的 coverage 区间
    trim_range, status,
}
PreviewPatch { logical_start, logical_extent, rgba_thumbnail, replaced_extent }
```

规则：

1. 首个 commit 发送一个 `replace` patch；后续只发送新增边缘 patch（向前 v1 为 append，未来双向允许 prepend），若 matcher 覆盖 overlap，则在 patch 边缘替换等比例的旧缩略像素，吸收缩放取整误差。patch 到达顺序不得改变逻辑坐标顺序。
2. 缩略图交叉轴固定为 128 个**物理像素**（默认值只有在 S5 性能采样后冻结）；沿轴分成有界 tile，默认 tile span 256。预览内存只保存缩略图，不保存完整 BGRA 画布；超出预览预算时保留最新可见区和两端摘要并报告 `PreviewDegraded`，不得影响主画布正确性。
3. 预览底图使用低对比度暗化层，已提交/最新 patch 以正常亮度显示，当前视口以半透明高亮框标出；不得通过白色/透明像素填补尚未提交区域。状态为 `Partial`/`Uncertain` 时在预览边缘显示状态标识，但不伪装为完整结果。
4. 每个 patch 更新只使旧 patch 与新 patch 的并集失效；不能因一个滚动步重绘整块 overlay。首帧目标是一次局部更新，后续更新应与新增缩略像素量近似线性。
5. 预览更新经 capture→overlay 的有界 latest-only 通道；同一会话最多一个生成中的 patch。`session_id + generation + preview_revision` 不匹配的结果丢弃；会话结束先停止发布，再清空 patch/tile/窗口，下一次 F5 不得闪现上次预览。
6. 若实现 hover 预览或裁剪拖拽，读取的是当前 `visible_range` 的小范围 viewport preview；请求必须单飞，带 `content_revision`，旧回调丢弃，不能在鼠标移动路径同步读回整张长图。v1 至少支持视口高亮和滚动条/边界显示。

参考 `refer/snow-apps/snow_shot` 的 `ScreenshotScrollingThumbnailWidget`、`ScreenshotScrollingPipeline`：它用 128 px 交叉轴、256 px tile、replace/append/prepend patch、overlap 替换行、局部绘制和 hover revision；参考 `refer/shot-refer/Crisp-main`/ShareX 的拼接和失败诊断只用于算法测试，不复制 Qt/GDI/整图内存路径。

明确区分参考实现的预览能力：`refer/shot-refer/ShareX-develop` 的 `ScrollingCaptureWindow` 只在采集完成后把整张 `Bitmap` 放入可滚动、可拖拽平移的结果查看器；`Crisp-main` 没有滚动缩略图窗口。它们不能证明“采集期间右侧预览”已经解决，也不能成为 SnapClip 将整张长图物化的理由。SnapClip 的右侧预览必须继续遵守本节的 patch/tile/有界内存契约。

---

## 6. 滚轮驱动与稳定等待

### 6.1 DriverCommand / DriverEvent

v2 的 `step()` 只返回 request id，外部只能猜状态。改为命令-事件模型：

```
enum DriverCommand {
    Begin  { session_id: ScrollSessionId, generation: u64, target: ScrollTarget, axis: ScrollAxis },
    Step   { request_id: u64, notches: i32 },
    Pause,
    Resume,
    Cancel,
}

struct DriverEvent {
    session_id: ScrollSessionId,
    generation: u64,
    request_id: Option<u64>,   // 与某一步无关的事件（Begin/Cancel）为 None
    kind: DriverEventKind,
}

enum DriverEventKind {
    InputInjected   { notches: i32 },
    InputRejected   { reason: InputRejection },
    WaitingForFrame { waited_ms: u32 },
    Settled         { qpc: i64 },
    Committed       { signed_delta_px: i32, overlap_px: u32 },
    NoMovement,
    EndConfirmed,
    EndUncertain,
    Cancelled,
    Failed          { reason: ScrollStopReason },
}

enum InputRejection { UipiBlocked, TargetLost, ForegroundMismatch, SendInputFailed, UnsupportedAxis }

trait ScrollDriver {
    fn submit(&mut self, command: DriverCommand) -> Result<(), CaptureError>;
    fn poll(&mut self, timeout: Duration) -> Result<Option<DriverEvent>, CaptureError>;
}
```

每个事件都携带 `session_id + generation + request_id`，消费方按 §3.1 的四重校验丢弃 stale 结果 —— 否则"stale 结果丢弃"只是 §3.1 的一句空话。`Committed` 由画布提交产生（不是驱动器的判断），`Failed` 携带 §4.6 的停止原因。

driver 成功不等于内容移动；终止仍由 Alignment、重复帧计数和（可用时）UIA extent 决定。

### 6.2 v1 WheelDriver

- 垂直用 `SendInput` + `MOUSEEVENTF_WHEEL`，水平用 `SendInput` + `MOUSEEVENTF_HWHEEL`；不以 Shift+垂直滚轮替代水平轴。
- 每一步从 1 notch 开始，设上限，不发送巨大滚轮量。
- 主路径不使用 `PostMessage(WM_MOUSEWHEEL)`：浏览器、Electron、自绘控件和嵌套容器处理不一致。
- 输入失败 / UIPI 拒绝 / 目标窗口关闭 → `InputRejected` → 进入 Partial/Failed。

### 6.3 自适应 settled

固定 sleep 只能作为 watchdog，不能作为 settled 的唯一依据。每个滚轮动作：

1. 等待至少一个新的 WGC frame（`SystemRelativeTime` / QPC 前进）；
2. 生成低分辨率 MatchView；
3. 以 QPC 单调、motion energy、粗位移一致性判断 Moving / AlmostStable / Stable；
4. `stable_samples = 2` 只是基线，按刷新率、动态 mask 和置信度在 1–3 个有证据样本间取值；
5. 达到 timeout 仍不稳定 → 降低步长、延长等待并重试；
6. 超过重试上限 → `Uncertain`，不提交中间帧。

初始参数仅作基线：`min_settle = 50ms`、`stable_samples = 2`、`settle_timeout = 500ms`；最终值由速度矩阵决定。settled 是帧证据事件，不是 `Sleep` 到期事件。

### 6.4 横向能力探测

不支持原生横向滚动的应用很常见（`HWHEEL` 常被忽略）。因此：

- 进入横向会话前，用一两个小步长的 `HWHEEL` 探测是否真的产生位移；
- 探测结果写入 `AxisCapabilities.horizontal`；
- 探测失败 → `HorizontalUnsupported`，明确提示"该目标不支持横向滚动"并提供手动模式，不伪造成功；
- 垂直探测同理，失败 → `UnsupportedAxis`。

Shift+滚轮 / PageKey 回退按目标应用能力单独加，不混入 v1 基线。

### 6.5 闭环步长

```
overlap_px    = viewport_axis - abs(delta)
overlap_ratio = overlap_px / viewport_axis

目标区间：0.30 .. 0.40
安全下限：0.20
硬下限：0.12

overlap_ratio < 0.12
  -> Rejected/Overshoot，notches--，用新 request_id 重试

0.20 <= overlap_ratio < 0.30 或 0.40 < overlap_ratio <= 0.45
  -> 保持步长，继续观察

overlap_ratio > 0.45 且连续 4 次健康、confidence 足够
  -> notches++，不超过配置上限

overlap_ratio 在目标区间
  -> 保持步长
```

`NoMovement` 不参与 overlap 控制。改变步长产生新的 `request_id`，重试候选帧只能有一个提交机会。连续 3 步内步长频繁增减 → conservative mode：降低采样节奏、增加稳定样本和 timeout。任何 `abs(delta) >= viewport_axis`、overlap 不足或尺寸变化均拒绝。

### 6.6 cadence 控制

记录 capture、match、canvas commit 的 latest / EWMA：

- 队列深度达到 2 或出现丢帧时立即降低采样频率/步长；
- 连续四个健康样本后每次只恢复一个等级；
- max_fps 30 只作 benchmark 起点；
- cadence 不能把低置信度帧变成成功，也不能为了速度跳过必要的稳定帧。

### 6.7 终点二次确认

`NoMovement` 只表示当前 step 未观察到有效位移，不直接表示 EOF：

```
Moving -> NoMovement -> Probe -> WaitLoad -> ProbeAgain -> EndConfirmed
                                                       \-> EndUncertain
```

- 默认执行一次小步长 probe；
- 连续两次 NoMovement + 稳定相似度 + 画布边缘证据同时成立 → `EndConfirmed`；
- 仍有 loading 迹象 → 短暂等待；超时 → `EndUncertain` 并保留 Partial；
- UIA extent（未来）只能作为额外证据，不能替代帧证据。

---

## 7. 位移匹配

### 7.1 统一轴接口

```
trait ShiftMatcher {
    fn estimate(&self, previous: &MatchView, next: &MatchView,
                axis: ScrollAxis, constraints: MatchConstraints) -> Alignment;
}
```

垂直按行方向搜索横向带，水平按列方向搜索纵向带；实现一个轴抽象，不维护两套复制算法。

### 7.2 v1 matcher：CPU 优先

先实现小尺寸 CPU MatchView 基线，不预设 GPU 预处理。当前 D3D11 renderer 没有现成的 compute/shader 通路，v2 直接假定 GPU downsample/luma/edge 属于没有依据的预设。

落地顺序：

1. GPU 线程（现 capture worker）回读已降采样的小尺寸 MatchView（默认 1/4，困难样本 1/2）与窄 overlap strip；
2. CPU 上做 1D profile SAD 粗搜索 + 多带 consensus + 全分辨率精修；
3. 只有在 readback bytes 与 CPU profiling 证明 GPU 预处理确实值得时，才增加 shader 路径。

matcher 结构：

1. 每行/列生成 compact descriptor：mean luma、variance、edge energy 和少量 bins；不得只用平均亮度。
2. 在 `expected_delta ± search_margin` 内做一维 profile SAD 粗搜索；首次、步长大变、丢帧或异常时才扩大搜索窗。
3. 动态 band 选择后做多带 consensus；候选必须检查 best/second-best margin、band agreement、valid area、residual、temporal consistency。
4. 仅在候选 `delta ± 4..8` physical px 内回读原始窄 overlap strip，做全分辨率 normalized SAD/edge 精修，最终只接受整数像素位移。
5. 页眉、页脚、滚动条、边框、光标、tooltip 由 ValidMask 排除。

实现必须保留 Crisp `Stitch*.cpp` 已验证的两个结构性约束：

- **先规划、后分配**：先对相邻帧求出全部可接受的轴向 delta，并累计 `total_extent`，再创建/扩展输出画布或提交 tile；不要在每次尝试中反复重分配整幅图。任一帧尺寸不符、delta <= 0（v1 向前）、搜索无结果或超过边长预算，都在该帧之前停止，已提交部分保持可导出。
- **搜索成本有界**：候选 delta 按 `expected_delta +/- search_margin` 搜索；每个候选在 band 差异累计超过当前 best 或像素差预算时 early-exit。首次/步长大变/丢帧/异常才扩大窗口，正常步不回到全轴暴力扫描。

Crisp 的默认“从内容区三分之一开始取 band”不能直接作为固定阈值，但其原因必须保留：顶部/底部 sticky 内容不能单独决定 delta。实现先用 `StickyRegion` 缩小有效搜索区，再在该区间的稳定带上做多带 consensus；有效区过小或所有带都动态时返回 `Uncertain`。

质量降级顺序：

1. Profile SAD + 多带 consensus；
2. 困难/歧义样本启用 NCC + edge 或扩大 overlap；
3. 降低 notches、重新采集 settled 帧；
4. 真实样本证明仍失败后，才增加可插拔 ORB/AKAZE。OpenCV 不是 v1 常驻依赖，且不得进入 `snapclip-capture` 的默认依赖图（门禁见 §2.2）。

以下均返回 Rejected/Uncertain：

- 多带位移不一致；
- best/second-best margin 不足或重复纹理导致候选不唯一；
- `abs(delta) >= viewport_axis`，或 `overlap_ratio < 0.12`；
- 残差、有效像素或尺寸不合格；
- source identity、generation、request_id 不匹配；
- 无法区分 NoMovement 和错误匹配。

### 7.3 动态选带与短期动态掩码

不永久固定"上中下"三条带。每个 session 只保留最近 2–4 个微型 profile/统计摘要（不是完整 MatchView 或 BGRA 帧），按小块/行/列计算短期变化量、纹理复杂度和边缘密度：

```
band_score = texture + edge + temporal_stability
             - dynamic_penalty - sticky_penalty - scrollbar_penalty
```

选取得分高且时域稳定的 2–3 个带；视频、动画、光标、tooltip、loading 和聊天更新区域形成动态 mask。mask 过大导致 `valid_pixels < minimum` 时必须 `Uncertain`，不能通过降低阈值强行接受。动态 mask 只保存低分辨率短期统计，不积累历史帧。

### 7.4 累计漂移与全局重锚定

逐帧 delta 会累积 1 像素级误差，v2 未处理，必须补：

- 每 N 步（默认 20）或累计 `|delta|` 超过阈值时做一次全局重锚定：把当前帧直接与最近 keyframe（首帧或上一个锚点）在扩大搜索窗下对齐；
- 重锚定只调整后续逻辑坐标原点，不改写已提交 tile 的像素；
- 若全局对齐与链路预测差异超过预算（默认 2 px）→ `DriftBeyondBudget`：停止并保留 Partial，或按配置回退到 keyframe 重新拼接；
- 验收：100 步滚动后总误差 ≤ 2 px（合成纹理可精确断言）。

keyframe 的内存契约（必须与 §7.3"只保留微型摘要"一致，否则重锚定会悄悄推翻内存上限）：

- keyframe 是**低分辨率 profile + 必要窄带摘要**（默认 1/8 行/列 profile、边缘能量与有效掩码），不是完整 BGRA 帧；单会话只保留 1 个（对齐后按策略替换，不累积历史），上限默认 ≤ 64 KB；
- 重锚定只用 keyframe profile 做粗对齐，再按 §7.2 第 4 步在窄 overlap strip 上做一次全分辨率精修，取得整数位移。

修正如何不制造 gap 或重复：

- 修正只改变**当前帧**相对首帧的绝对位置估计，不改写已提交 tile 的像素；
- 应用修正前必须验证：把当前帧放到修正后的位置，结果与原 union 相邻或相交，新增条带宽度 ≥ 1 px，且 overlap 不低于硬下限；
- 若修正后的位置与原 union 之间会留下未覆盖区，或需要重写已提交像素 → 不应用修正，直接 `DriftBeyondBudget` 停止并保留 Partial。

验收：100 步滚动后总误差 ≤ 2 px；对注入 ±1 px 误差的合成序列，重锚定要把总误差拉回预算内，且不产生 gap 或重复逻辑坐标。

### 7.5 动态内容和固定边缘

匹配带不能固定取最顶部。统计相似度、edge similarity、motion ratio 和连续 run length，上限不超过视口三分之一：

- sticky header 只从首帧写一次；
- sticky footer 只从最后稳定帧写一次；
- 所有带动态时不得伪造成功；
- 连续 3 个稳定帧无新增或边缘未变化才判定边界，单帧不变可能是加载动画。

---

## 8. 画布、Tile 与导出

### 8.1 双向 union 模型

单轴位置使用相对首帧的有符号坐标：

```
next_pos  = current_pos + signed_delta
old_union = [min_pos, max_pos + viewport_axis)
new_union = old_union union [next_pos, next_pos + viewport_axis)

只为 new_union - old_union 分配新范围；
old_union 与 next frame 的交集只覆盖，不增加输出长度。
```

必须满足：

- 首帧完整写入；
- 后续只新增 union 外条带；
- overlap 允许新帧覆盖旧帧，但逻辑坐标只出现一次；
- NoMovement 不扩展画布，Rejected/Uncertain 不改变画布；
- 向上滚动扩展前缀时移动逻辑原点，不重复追加历史内容；
- 缺失帧、delta 超过视口或检测到 gap 时停止，不用白色/透明像素填洞；
- 重锚定的原点修正不改变"逻辑坐标只出现一次"这条不变量。
- coverage 必须**二维完整覆盖**导出矩形：沿滚动轴是单段连续区间（无内部空洞），**且垂直于滚动轴的每个有效坐标都覆盖完整的目标宽度/高度**。这是"禁止空洞"能成立的前提——**矩形 PNG 无法表达稀疏 coverage**，只查一维连续并不够（纵向连通但某行缺角，同样拼不出无空白的矩形）。`finish` 之前把这两条都当断言检查；一旦不满足（bug 或未在 gap 前按预期停止），就不产出文件并回报缺失的行/列区间（见 §8.3 的出口规则）。

v1 `WheelDriver` 只向前，但核心画布保留 signed 坐标，后续双向与手动全景复用，不让 v1 driver 承担双向复杂度。

### 8.2 Sticky 区域与一致性检查

固定边缘抽象为 `sticky_leading` / `sticky_trailing`：Vertical 对应 top/bottom，Horizontal 对应 left/right。用相似度、edge similarity、motion ratio 和连续 run length 估计 `StickyRegion { start_px, end_px, confidence }`，不要求严格逐像素相等。

- 高置信度：只从首帧写 leading，只从最终 `EndConfirmed` 帧写 trailing；
- 中/低置信度：不主动裁剪真实内容，只把区域加入 matcher mask；
- 横向固定左列/右列检测不稳定时保守保留，不强行删除。

参考实现的边界规则：

- Crisp 对 sticky footer 的搜索区在 footer 上方截断，避免大 footer 把真实 overlap 推入固定区而误拒绝；导出时 footer 只从最终稳定帧追加一次。SnapClip 只在 `confidence` 达到门限时采用该裁剪，否则保留像素、仅加入 matcher mask。
- Crisp 的 sticky header/footer 检测要求“所有已采样帧都稳定”，任一帧该行变化就结束连续 run；不得用单帧相等推断固定边缘。
- ShareX 的 `AutoIgnoreBottomEdge` 是经验型动态排除，且 `bestGuess` 只改变结果状态为 `PartiallySuccessful`。SnapClip 若只能依靠 fallback/上一成功匹配继续，必须产生 `Uncertain`/`Partial` 事件，绝不能把猜测当 `Accepted`。

每次提交前检查：

- overlap 像素误差低于阈值；
- 新增范围与旧 union 相邻或相交，不允许 gap；
- 输出 tile 覆盖 bitmap 无未覆盖像素；
- 画布范围、tile 数量和像素总量未越界。

新帧覆盖重叠区是有意设计：滚动后懒加载完成的内容可以修正旧像素。但必须经过质量保护：若新 overlap 的边缘密度/局部方差/有效像素显著低于旧 tile，或残差异常，则保留旧 tile、标记该 commit 为 uncertain 并等待下一稳定帧。

### 8.3 TileStore 所有权与导出端口

两条约束事实（都已实测）：

- `snapclip-history::CaptureArtifactStore::write` 只接受整幅 PNG 字节（`CaptureOutput` → `<session>-<seq>.png`，边写边算 blake3），没有 tile / journal 能力 —— **这是"改造前事实"，docs/24 S4.2 会把它改掉**（写入入口改为接收写入流）。改完之后，任何后续设计都不得再依据这句话（这条保留在这里是为了说明"为什么要改"）。
- 仓库里没有任何 WebP 编码路径（这一条仍然成立）。

结论：v1 **只输出 PNG**，WebP 延后；长图经端口流式交给壳层，capture 不依赖 `snapclip-history`。

两层端口、一条实现：`ArtifactWriter`（普通截图）与 `ScrollSink`（滚动）只是**上层语义端口**——前者接收"已确认的选区"，后者接收 tile/行带。两者最终都汇入同一个行带编码器与同一个写入实现（同一套命名规则、边写边算的 blake3、原子 rename），**不是两套存储，也不是两套编码器**。

所有权划分：

- capture 拥有 session 级临时 tile 目录，写盘与 LRU 都在 capture 内；
- 最终产物经端口流式交给壳层实现，capture 只声明需求。

v3 只写了 trait 名、没有定义被引用的类型，等于不可实现。补齐：

```
/// 一个逻辑 tile。坐标是画布逻辑坐标（物理像素，原点 = union 的 (min_x, min_y)）。
struct ScrollTile {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    /// 紧凑 BGRA，行距 = width * 4（写盘前已去掉 pitch）。
    bgra: Vec<u8>,
    /// 对 bgra 全字节的 crc32；写入端与读取端都要校验。
    checksum: u32,
}

struct ScrollExportMeta {
    session_id: String,
    axis: ScrollAxis,
    /// 画布逻辑尺寸 = union 的宽高。
    width: u32,
    height: u32,
    /// v1 固定 Png。
    format: ScrollFormat,
    /// 预期 tile 总数，finish 时据此校验无缺块。
    tile_count: u32,
    /// 是否为提前停止的部分结果。
    partial: bool,
}

enum ScrollFormat { Png }

pub trait ScrollArtifactWriter: Send + Sync + 'static {
    fn begin(&self, meta: &ScrollExportMeta) -> Result<Box<dyn ScrollSink>, CaptureError>;
}

pub trait ScrollSink: Send {
    fn write_tile(&mut self, tile: &ScrollTile) -> Result<(), CaptureError>;
    /// 校验 tile 齐备后落盘，返回 artifact 引用。`partial == true` 时返回的引用同样有效，
    /// 由调用方把"这是部分结果"一并上报，不能悄悄当成完整图。
    fn finish(self: Box<Self>) -> Result<ArtifactRef, CaptureError>;
    /// 丢弃已接收的 tile，绝不留下文件。
    fn abort(self: Box<Self>);
}
```

语义约定：

- tile 尺寸默认 512×512，末列/末行可以更小；**tile 之间在逻辑坐标上不重叠**，覆盖由 coverage bitmap 保证（§8.2 的检查项）。重叠只发生在"新帧覆盖旧内容"的提交层，不在 tile 层。
- checksum 覆盖 `bgra` 全字节，不覆盖 padding；`finish` 必须逐 tile 校验。
- `finish` 只做"齐备性 + 校验 + 落盘"，不做重采样；部分结果按 `partial` 标记返回。
- `abort` 幂等，且不残留临时文件。

与既有端口的关系：

- 壳层实现落 PNG、进 history、写剪贴板；capture 不 import `snapclip-history`。
- **PNG 编码器只有一个实现，且输入是行带**（流式）：`snapclip-history` 现有的"整图 `Bgra8Image` → PNG"路径要重构成"行带输入"的同一份代码——普通截图是"一次喂入全部行带"的退化情形，滚动是"分多次喂入"。现路径同时驻留 ≥2 份整图（BGRA + RGBA）再加 PNG 字节，与 §8.4"不得 materialize 长图"直接冲突，所以这里改的是根因，不是给滚动加一条旁路。按开发期方针（AGENTS.md §1），**不留第二套编码器、不留旧函数做兼容**，调用方随签名一起改。
- `ArtifactWriter` 端口仍然是"整幅选区 → artifact"的入口（普通截图走它）；`ScrollSink` 是同一套落盘规则下的**流式入口**（同样的命名规则、边写边算的 blake3、原子 rename），二者共用上面那一个编码器实现，不是两套存储。
- **唯一调用链**：`像素行 → encode_png_rows(rows, sink) → 临时文件 → 原子 rename → ArtifactRef`。编码器用 `png::Encoder::stream_writer()` 拿 `StreamWriter`（实现 `Write`）逐行 `write_all`，最后 `finish()`；**不是**逐行调 `Writer::write_image_data`（那个 API 一次写整张图）。行缓冲是 RGBA（`png::ColorType` 没有 BGRA 变体），每行做一次 BGRA→RGBA 转换，不额外驻留整图。`CaptureArtifactStore` 因此也要从"先攒整张 PNG 字节"改成**接收写入流**（写的过程中算 blake3），不再要求 `CaptureOutput.bytes`。
- tile 的**到达顺序与扫描行顺序无关**：导出按画布 `y` 升序读行带，一条扫描行横跨多个 tile 时逐 tile 取行切片拼进同一行缓冲；行缓冲大小 = 画布宽 × 4，与高度无关。所需的 tile 已被 LRU 换出就从会话临时目录读回，读不到即失败，不产半张图。

### 8.4 限制与磁盘安全

- 同时限制轴向长度、总像素、tile 数、临时目录字节和导出时长。初始值可参考 30,000 px / 150 MP，但必须用 SnapClip 基准重新确定。
- 150 MP 仅是保护阈值，不能 materialize 为连续 BGRA（约 600 MB）；CanvasStore 必须 tile 化、流式写盘，内存只保留有界 LRU。
- 导出还必须检查 `max_export_pixels`、`max_export_dimension`、`max_export_bytes`、`max_export_time`。**超预算时的结果契约**（只有一种，不留三选一）：按 coverage 裁成一个**连续的** PNG（不重采样），一律保留画布起点一侧的连续前缀，并在 metadata 记录 `original_canvas_size` 与 `crop_origin`；连裁剪后都超预算则返回 `ResourceLimit`，不产出文件。分段导出 / 多文件结果 / 中间局部导出 v1 不实现。裁剪后仍超过剪贴板安全尺寸 → 不写剪贴板，只进历史并把原因上报。
- 每个会话使用独立临时目录，启动时清理过期目录；磁盘不足立即停止并保留可读的 Partial 结果。
- 导出写临时文件，flush 后 atomic rename；失败保留部分结果和诊断，不污染剪贴板/历史。
- 滚动结束显式 drop LRU、frame pool、staging 和 matcher。
- v2 的 journal + 崩溃重放已删除：产品没有"重启后继续上次滚动"的入口，该机制缺少需求依据（AGENTS.md：禁止为臆测增加不必要的复杂抽象）。保留 checksum 与原子替换即满足"不产生伪完整文件"。

---

## 9. 自动滚动扩展（v2）

滚轮与自动滚动共享 `ScrollDriver` 接口（§6.1 的命令-事件模型）。实现顺序：

1. WheelDriver：真实 SendInput，支持两轴和速度闭环。
2. PageKeyDriver：PageDown / PageUp / 方向键，仍由图像 delta 确认。
3. UiaScrollDriver：仅对已验证窗口直接调用 ScrollPattern / SetScrollPercent，有界超时和 quarantine；不遍历 UIA 全树。
4. 浏览器 / 特定应用适配器：独立模块，不污染窗口检测和普通截图路径。

driver 成功不等于内容移动；终止仍由 Alignment、重复帧计数和（可用时）UIA extent 决定。

### 9.1 浏览器 / 应用专用策略

浏览器的 full-page screenshot 是产品能力或 DevTools / 扩展能力，不是 SnapClip 可以对任意浏览器窗口直接调用的 Windows API。定义 `CaptureStrategy`：

```
NativeFullPageStrategy    -> 能力探测成功且权限/协议可用时使用
ScrollAndStitchStrategy   -> v1 通用窗口路径
ManualPanoramaStrategy    -> 自动输入不可用时兜底
```

`BrowserAdapter` 必须独立处理 Chromium/Edge/Firefox 的协议或扩展通信，明确超时、权限和失败回退；不能让浏览器专用依赖进入普通窗口截图或 docs/14 的窗口检测热路径。

---

## 10. 性能、日志与边界

### 10.1 热路径红线

- overlay 消息线程、壳层 UI 不同步等待 WGC、DXGI、DWM 或 SendInput。
- 不把完整帧 JSON / Base64 化，不保存所有历史帧。
- 不为每个滚轮事件创建线程、device、frame pool 或 staging texture。
- 不使用无界 channel、无界临时文件或无界 PNG 压缩队列。
- GPU 操作按 §4.2 串行在 GPU 线程，不在 overlay 线程做匹配，也不在 overlay 线程调用 `context()`。
- 预览更新只处理 `PreviewPatch`，不在滚动步内 materialize 完整 canvas；预览 tile、patch 队列和 hover viewport 请求均有界。
- F6 贴图热路径只传 `artifact_id`/轻量元数据；完整像素只在贴图窗口准备阶段按有界资源读取一次，不复制给壳层，不触发新的 capture。

### 10.2 诊断指标

v1 只要求"能用来定阈值"的那一组，其余标为可选，避免在指标上先行投入：

必需：

- 每步：requested notches、实际 delta、overlap%、confidence、residual、status（accepted/rejected/uncertain/no-movement）；
- matcher：candidate_count、best/second-best score、peak margin、band deltas、valid pixels、动态 mask 面积；
- 信箱：峰值 / 丢帧数；重锚定次数与 `DriftBeyondBudget` 命中；
- readback：bytes/frame、bytes/session、`readback_not_ready` 次数；
- 输出：长度、总像素、交界验证失败位置；
- 预览：首个 patch 延迟、patch bytes、patch 更新 P50/P95、preview tile 数/峰值字节、丢弃 stale patch 数、视口框更新延迟；
- 贴图：F6→HWND 可见延迟、贴图窗口数、每窗口 GPU/CPU 字节、创建/销毁后的句柄与 Private Bytes；
- 每次停止必须记录 `ScrollStopReason`（§4.6）。

可选（profiling 时再开）：capture/match/canvas/export 的 P50/P95、tile cache 命中率、临时目录字节、GPU dedicated/shared memory、CPU 与 Working Set。

默认不逐帧洪泛；diagnostics 开启才记录每步摘要。

### 10.3 事件契约

v3 这里的形状写错了：`snapclip_model::CaptureEvent` 是**结构体**（`session_id` / `state` / `artifact` / `error_code` / `generation`），枚举是 `AppEvent`。滚动进度作为 `AppEvent` 的第四个分支，而不是塞进 `CaptureEvent`：

```
// crates/snapclip-model/src/events.rs
pub enum AppEvent {
    Capture(CaptureEvent),
    Clipboard(ClipboardEvent),
    Recognition(RecognitionEvent),
    Scroll(ScrollProgress),        // 新增
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollProgress {
    pub session_id: String,
    pub state: ScrollState,
    pub outcome: Option<ScrollOutcome>,
    pub axis: ScrollAxis,
    pub captured_length_px: u32,
    pub viewport_length_px: u32,
    pub step_index: u32,
    pub last_delta_px: Option<i32>,
    /// 定点置信度：0..=10000 表示 0.0000..=1.0000。
    /// 用整数而不是 f32，是因为 `AppEvent` 派生 `Eq`（见下）。
    pub last_confidence_bp: Option<u16>,
    pub stop_reason: Option<ScrollStopReason>,
    pub generation: u64,
}

// 词表全部归 snapclip-model：capture 已经依赖 model，反向依赖会成环。
pub enum ScrollAxis { Vertical, Horizontal }
pub enum ScrollState { Armed, CapturingFirstFrame, Scrolling, Paused, Ended }
pub enum ScrollOutcome { Completed, Partial, Cancelled, Failed }
pub enum ScrollStopReason { /* §4.6 的完整列表 */ }
```

三条必须遵守的既有约束（都是当前代码里已经存在的硬约束，不是风格问题）：

- `AppEvent` 派生 `PartialEq, Eq`：所以置信度用 `u16` 定点。若将来非要 `f32`，必须在文档里明确说明为什么放弃 `Eq`，不能默默改掉派生。
- `AppEvent::generation()` 与 `with_generation()` 必须补 `Scroll` 分支；漏了的话新分支的 generation 恒为 0，`EventBus` 的"丢弃更旧事件"对它完全失效。
- `events.rs` 有一条 `size_of::<AppEvent>() <= 256` 的测试。`ScrollProgress` 必须保持轻量（无像素、无 Vec）：这条测试是设计意图，不是需要放宽的断言。

壳层不接收纹理、完整帧、画布像素或高频逐帧 JSON。滚动预览属于 capture overlay：只接收有界的 `PreviewPatch` 与视口/状态元数据；GPUI 壳只接收低频 `ScrollProgress`。完成/部分结果的完整 artifact 仍经 `ArtifactWriter` 进入历史，供 F6 贴图按需读取。

---

## 11. 测试与验收

### 11.1 Rust 纯单元测试

- 垂直/水平轴访问器、正负 delta 和半开区间；
- 已知位移多带匹配、动态带投票、固定页眉/页脚和滚动条 mask；
- profile descriptor、best/second-best margin、重复纹理歧义和动态 mask 选带；
- NoMovement、无关帧、尺寸变化、低置信度、多带不一致、超过硬下限和 overlap 不足拒绝；
- union 在向下、向上、往返时不膨胀；overlap 覆盖、union 外只新增一次、无 gap、逻辑坐标无双写；
- 重锚定：原点修正后逻辑坐标仍唯一，100 步总误差 ≤ 2 px；
- session / generation / request stale 结果丢弃；
- tile cache、上限、取消释放；
- cadence 压力降速、健康恢复、步长闭环。
- 事件契约：`AppEvent::Scroll` 参与 generation 重盖与"丢弃更旧"，且 `size_of::<AppEvent>() <= 256` 仍成立；
- 重锚定邻接：注入 ±1 px 误差的序列修正后不产生 gap、不产生重复逻辑坐标；
- tile：checksum 不匹配时 `finish` 必须报错；缺块时 `finish` 必须报错而不是产出半张图；`abort` 不留文件；
- GPU 上下文单写者：非 GPU 线程调用 `GraphicsDevice::context()` 在 debug 构建下 panic（§4.2 的断言本身要有测试覆盖）。

测试帧用程序生成可逆行/列纹理、固定边缘、局部动态噪声和已知 offset，逐像素比较期望输出。

### 11.2 自动夹具（v1 必建）

滚动截图必须能在无人值守下重复验证，因此先建夹具再写算法：

- 可控滚动窗口：一个自绘测试窗口，可编程控制滚动位置、步长、动画曲线与懒加载延迟；
- 确定性帧序列：已知位移的合成纹理（可逆行/列图案、固定页眉页脚、重复纹理区、全空白区）；
- 冻结 QPC/时间戳注入：让 settled 与丢帧判定可确定复现；
- 故障注入：丢帧、窗口 resize、DPI 变化、设备移除、目标窗口关闭、磁盘写失败。

### 11.3 Windows 集成测试

- WGC 首帧/后续帧尺寸、DPI、QPC 单调，设备移除和窗口关闭释放 frame pool；
- 可滚动测试窗口验证竖/横滚轮、平滑动画、不同 wheel delta、固定页眉/页脚和边界；
- Win32 / WPF / WinUI / Electron / Chromium / 自绘窗口的真实输入链路；管理员/UIPI、RDP/VM、触摸板平滑滚动和低纹理/重复纹理页面；
- SendInput 成功 / 失败 / UIPI 时恢复 foreground、focus、cursor；
- controller HWND 的四条验收（§5.4）；overlay、工具栏和主窗口不进入捕获，检测排除与捕获排除独立断言；
- tile 输出、checksum、PNG 原子落盘，中途取消 / 磁盘错误不产生伪完整文件。
- `PreviewPatch` 首帧 replace、后续 append/prepend、overlap 替换和视口映射；预览只更新脏区且无 stale patch 闪回；结束后窗口与 tile 全部清理。
- F6：普通截图与滚动完整/Partial artifact 像素逐项一致；贴图置顶但不抢焦点、不进入后续捕获；artifact 读取失败不留空 HWND；重复创建/关闭资源回落。
- 会话热键：overlay 隐藏时 Esc / Enter / 暂停可用；热键被占用时降级路径生效；退出后热键已注销。
- overlay 与 controller 都不出现在显示器级捕获的帧里（像素断言）。
- 放大镜取色迁移到 GPU 线程后，延迟 P95 不劣于现状（§4.2 的门禁）。

### 11.4 速度-质量矩阵

矩阵按 v1 能力拆分，避免把未来能力混进门禁：

v1 验收矩阵（向前滚动）：

| 变量 | 级别 |
|---|---|
| wheel step | 1、2、3、6 notches |
| 动画 | 关闭、系统平滑、应用自定义 |
| settle timeout | 50、100、180、260、500 ms |
| 视口 | 800x600、1920x1080、4K 选区 |
| 轴向 | 竖、横 |
| 内容 | 静态文档、浏览器懒加载页、动态聊天/视频页 |

未来双向矩阵（v2 引入向后滚动后再补）：往返、向上、PageUp。

每个组合至少 10 次，并增加权限（普通 / 管理员）、DPI（100% / 125% / 150%）、负虚拟桌面坐标与跨屏目标、输入设备（鼠标 / 触摸板）、RDP/VM 维度。记录重复行/列、空白像素、错位率、误接受/拒绝、提前终止、每步延迟、总吞吐、CPU、内存、GPU、readback bytes 和磁盘。

参数扫掠（找默认值）与正确性门禁（判通过/失败）分开执行：前者可以只跑抽样组合，后者只跑固定用例。默认参数只能从数据确定；无法可靠对齐时必须降速、重试或返回部分结果。

### 11.5 质量门禁

- `blank_pixels = 0`、unexpected transparent gaps = 0、illegal duplicate logical range = 0；任何 gap 都只能停止并报告 Partial。
- 重点指标是错误位移被 `Accepted` 的比例（false acceptance），而不是单纯追求低 false rejection；重复纹理、低纹理和动态页面允许返回 `Uncertain`。
- 对每次 `Accepted`，测试必须能回溯到 candidate margin、band consensus、valid area、fine residual 和 overlap 验证证据。
- 100 步累计漂移 ≤ 2 px。
- 默认参数、质量阈值和 tile/readback 预算只有在矩阵数据和性能采样归档后才能冻结。

### 11.6 人工验收

1. 浏览器长页面竖向截图：页眉一次、无重复段和白带。
2. 宽表格 / 画廊横向截图：列顺序和交界正确；不支持横向滚动的目标给出明确提示。
3. 快速 / 慢速、平滑滚动、滚到边界、手动回滚：画布不膨胀；v1 自动 WheelDriver 仍只向前滚动。
4. 动态聊天、懒加载和固定输入框：稳定等待及 sticky footer 正确。
5. 暂停、调整、继续、Esc、重复开始：无旧帧闪现、输入卡住或资源泄漏；右侧预览随每次 commit 增量更新，取消/下一次 F5 不残留旧预览。
6. 多显示器、负坐标、混合 DPI、WGC 降级路径。
7. 20 次滚动循环后线程、窗口、GPU resource、Private Bytes 和临时目录回落。
8. 20 次 F6 创建/关闭循环后贴图 HWND、纹理、句柄、GPU resource 和 Private Bytes 回落；普通、完整滚动、Partial 三类 artifact 各验证一次。

---

## 12. 实施阶段

每阶段均需：修改前基线、修改后新旧回归、真实测试证据、独立提交；与 docs/14 冲突时先记录证据并修订设计，不在实现中默默偏离。

阶段门禁命令（workspace 化之后）：

```
cargo check --workspace --all-targets
cargo test  --workspace --lib
cargo test  -p snapclip-app --features test-support --test ui
```

### Phase S0：基线和夹具

阅读 docs/14、docs/07、docs/22、docs/23 与参考项目；建立 §11.2 的合成帧、可控滚动窗口和故障注入夹具；冻结 `ScrollAxis`、`ScrollTarget`、`ScrollSession`、`Alignment`、`ScrollStopReason`、`ScrollSink` 与 driver 命令-事件类型；跑一遍 §0.1 基线。

### Phase S1：纯拼接核心

实现轴抽象、CPU 1D ProfileMatcher、多带 consensus、动态 mask、sticky 置信区间、End Confirmation、union 画布、重锚定、拒绝状态、硬上限和纯单元测试；不接真实滚轮 / WGC。

### Phase S2：WGC/D3D11 活动帧源

S2 的第一件事是 §4.2 的前置重构：把 `read_region` 与放大镜 `AsyncSampleBuffer` 从 overlay 线程搬到 GPU 线程，并加上"非 GPU 线程调用 `context()` 即 panic"的 debug 断言；这一步要有自己的回归门禁（放大镜取色延迟、导出选区像素一致）。之后的滚动部分：实现 `ActiveFrameSource`（`CreateForWindow` 为主、`CreateForMonitor` 降级）、session 级 frame pool、新增泛化区域异步回读、容量 1 信箱；验证 `ContentSize` / `SystemRelativeTime`、丢帧语义、设备移除、窗口关闭和取消。不得把一次性 `FrozenFrame` API 改成隐式循环接口。

### Phase S3：WheelDriver、overlay 控制窗口、右侧预览与 F6 贴图

接入 SendInput、焦点/光标恢复、controller HWND 生命周期（含热键注册与捕获排除）、frame-driven settled、overlap 控制步长、横向能力探测、End Confirmation、低频状态事件；实现 §5.6 的有界增量右侧预览和 stale 丢弃；注册 F6 并实现 §5.5 的原生贴图窗口生命周期。先竖向，再用同一轴抽象接横向。v1 只向前滚动，同时建立 `ManualPanoramaDriver` 接口。

### Phase S4：TileStore、导出与贴图 artifact 回归

实现 tile 化画布、有界 LRU、checksum、`ScrollTile` / `ScrollExportMeta`、`ScrollSink` / `ScrollArtifactWriter` 端口与壳层实现、原子 PNG、部分结果、磁盘上限和内存回落测试；接入普通/完整滚动/Partial artifact 的 F6 读取回归，不增加第二套编码或存储。

### Phase S5：自动滚动、预览/贴图性能和收口

接入 PageKey / UIA driver 的有界超时和 fallback，评估 BrowserAdapter / NativeFullPage；完成 §11.4 v1 验收矩阵及 WPR / WPA / PresentMon 或等价采样，包含 preview patch 与 F6→HWND 指标；只根据数据调整阈值和决定是否引入 GPU 预处理或 OpenCV 后端。

---

## 13. 结论

SnapClip 滚动截图应采用"活动 GPU 帧源 + 有界容量 1 流水线 + 实际位移匹配 + union 画布 + 磁盘 tile 输出"，而不是固定高度截图后直接拼接。

推荐落地顺序：

1. 纯 Rust 轴向多带匹配、重锚定和双向画布逐像素测试；
2. `ActiveFrameSource`（窗口级 WGC 为主）与泛化区域异步回读；
3. SendInput 竖 / 横滚轮、controller HWND、稳定等待和闭环步长；
4. 交界验证、拒绝不可信帧、tile 和原子导出（经端口，capture 不依赖 history）；
5. 用速度-质量数据确定默认参数，再扩展 PageDown / UIA 自动滚动与浏览器适配器。

该边界满足低延迟、低占用和可验证性：壳层只接收低频状态，overlay 不被长截图阻塞，拼接错误不会被白色填充掩盖，未来更换滚动驱动或 matcher 不会破坏 docs/14 的窗口检测核心。

### 13.1 开放项

只剩一项，留待 profiling 决定：

- canvas 落盘是留在 scroll driver 线程，还是拆成独立线程：本版按"先不拆，有 profiling 证据再拆"落笔。

以下三项已在 v4 裁决，不再作为开放项：

- GPU 上下文单写者归 GPU 线程（现 capture worker），overlay 不再做任何 context 操作（§4.2）；
- v1 只支持 `TargetKind::TopLevelWindowFrame`，其余显式返回 `TargetUnsupported`（§3.2）；
- 删除 journal 崩溃重放，保留 checksum + 原子替换（§8.4）。

若产品日后要求浏览器元素级滚动，需要先由 docs/18 / docs/20 定义元素身份，再走 §3.2 的 `TargetKind::UiElement` 分支 —— 那是 v2 的扩展点，不在 v1 的占位里。
