# SnapClip 滚动截图设计方案

> 状态：评审修订稿（v2），待实现前基线验证
> 适用：Windows 10/11，Tauri + Rust + Win32 + WGC/D3D11
> 依赖：docs/14-screenshot-window-detection-design.md 的窗口身份、DPI、排除集和生命周期约束。

## 1. 目标与边界

### 1.1 目标

1. 支持已确认截图区域内的竖向滚轮截图和横向滚轮截图。
2. 支持不同滚轮步长、平滑动画、懒加载和动态区域，使用实际位移对齐而不是固定步长拼接。
3. 交界处不重复、不重叠、不出现猜测的白色或透明空白；无法可靠对齐时停止并报告部分结果。
4. 捕获、匹配、画布和导出不阻塞 overlay 消息循环、鼠标输入或 Tauri WebView。
5. 为后续 PageDown、UIA ScrollPattern 和浏览器适配器保留驱动接口。
6. 通过合成帧、Windows 集成和真实窗口速度矩阵确定默认参数。

### 1.2 非目标

- v1 不遍历 UIA/MSAA 子树；docs/14 已将深选留给 v2。
- v1 不通过 Tauri event、Base64 或 WebView Canvas 传输帧。
- 一次会话只锁定一个轴，不同时滚动横向和纵向。
- 滚动截图不复用视频/GIF 编码器；完成后才导出 PNG/WebP。
- 不将 Crisp 的 GDI/BitBlt CPU 管线直接带入 SnapClip。
- 不把浏览器的“全页截图”宣传能力误写成 SnapClip 可直接调用的通用 Windows API；浏览器路径必须是单独适配器，并经过目标浏览器能力探测。

## 2. 调研结论

### 2.1 SnapClip 当前基线

- docs/14 已冻结 WindowIdentity、WindowSnapshot、Per-Monitor V2、虚拟桌面物理坐标、显式 excluded_hwnds/excluded_process_ids 及 overlay/worker 边界。
- docs/07 规定 WGC/DXGI 输出 D3D11 texture，只有算法或导出需要时才读回。
- 普通截图使用冻结帧；滚动截图必须使用短生命周期的活动帧源，不能复用单张冻结纹理。
- 当前 `src-tauri/src/platform/windows/capture/win/wgc.rs` 仍以单次冻结帧为主要调用形态；滚动实现必须新增独立的 session-owned active source，不得在普通 `FrozenFrame` 上循环调用 readback 冒充活动捕获。
- 当前 `win/d3d11.rs` 已有异步 readback slot（现状为 3 槽）和 `D3D11_MAP_FLAG_DO_NOT_WAIT` 路径；滚动实现应优先复用该 wrapper，并以基准决定 2/3 槽，不得在文档中武断改成每步新建 staging texture。
- SnapClip-old/docs/04-scroll-stitching.md 已有容量 1 最新帧信箱、position/max_depth、重叠覆盖、rejected 与零位移分离、静态边缘检测和硬上限原则。

### 2.2 Crisp 可吸收项

已研读 refer/shot-refer/Crisp-main 的 ScrollCapture、Stitch、TestStitch：

- 使用 SendInput 产生真实 MOUSEEVENTF_WHEEL/HWHEEL，而不是只投递 WM_MOUSEWHEEL；光标临时停在区域中心，结束后恢复。
- 采集与拼接分离；等待滚动动画时泵送消息，避免界面假死。
- 位移搜索抽象为一维行/列算法，支持垂直和水平；搜索带避开固定页眉，页脚不参与匹配。
- 只有可信新增条带写入画布；找不到位移即停止，不能把“最佳候选”强行当成功。
- 测试覆盖已知位移、固定页眉、重复帧、无关帧、尺寸不一致、水平和垂直拼接。

不能照搬：Crisp 的 GDI BitBlt、固定 settleMs、固定 notches 和把全部帧留在内存的实现不适合 WGC/D3D11 和低占用目标。

### 2.3 Snow Shot 可吸收项

已研读 snow_shot 的 ScrollingCapture 状态、overlay 暂停/恢复路径、capture worker 和 AdaptiveScrollingCaptureCadence：

- 滚动模式拥有独立的渲染/交互状态，可暂停、调整选区、继续，不应阻塞普通 Settled 状态。
- cadence 根据 capture/stitch EWMA、队列深度和丢帧数调节目标频率；压力立即降速，连续健康样本才恢复。
- cadence 只负责吞吐控制，不负责判断拼接正确性；对齐仍必须由图像证据确认。

### 2.4 评审建议逐项裁决

| 建议 | 决定 | 吸收方式或不采用原因 |
|---|---|---|
| 修正 `desired_delta` 与 `delta > 60%` 的冲突 | 吸收 | 控制量改为 `overlap_ratio`，不再用互相冲突的 delta 阈值 |
| 1D profile + multi-band + fine verification | 吸收 | 作为 v1 默认 matcher；NCC/SAD 退为 profile 距离和困难样本 fallback |
| 动态选择匹配带、动态 mask、peak margin | 吸收 | 以短期低分辨率时域统计选带；候选须满足带一致性、margin、残差和有效面积 |
| sticky 使用相似度和置信区间 | 吸收但保守 | 高置信度才裁剪；中/低置信度只用于 mask，不删除真实内容 |
| 横向 sticky leading/trailing | 吸收 | Vertical 映射为 top/bottom，Horizontal 映射为 left/right |
| `NoMovement` 二次 probe | 吸收 | NoMovement 不等于 EOF；进入 Probe/WaitLoad/ProbeAgain |
| overlap 低于阈值自动减小步长 | 吸收 | 目标 30%-40%，安全下限 20%，硬下限 12%；连续健康样本才加步长 |
| 手动滚动兜底 | 吸收为 P1 | `ManualPanoramaDriver` 只负责被动采帧和拼接，不伪造自动输入成功 |
| 浏览器 NativeFullPage | 吸收为未来策略 | Edge/Firefox 的产品能力不等于可供第三方任意调用的 API；需 BrowserAdapter/CDP/扩展能力探测，失败回退通用路径 |
| 新帧无条件覆盖 overlap | 部分吸收 | 默认新稳定帧覆盖；若质量评分显著下降，保留旧 tile 并标记 uncertain |
| 150 MP 直接 materialize 为 BGRA | 不采用 | 150 MP 仅为保护上限，必须使用 tile、LRU 和流式导出 |
| 固定 30 FPS | 不采用 | 只作为基准上限；滚动按有价值状态变化和队列压力调节 |

### 2.5 参考源码证据

| 项目 | 文件 | 结论 |
|---|---|---|
| Crisp | `refer/shot-refer/Crisp-main/src/ScrollCapture.cpp` | `SendInput`、光标停放、等待期间泵送消息、方向探测 |
| Crisp | `refer/shot-refer/Crisp-main/src/Stitch.cpp` | 轴向共享搜索、early-exit、低质量候选拒绝 |
| Crisp | `refer/shot-refer/Crisp-main/src/StitchVertical.cpp` | sticky header/footer、只写新增条带、失败停止 |
| Crisp | `refer/shot-refer/Crisp-main/src/StitchHorizontal.cpp` | 横向对称拼接核心 |
| Crisp | `refer/shot-refer/Crisp-main/tests/TestStitch.cpp` | 已知位移、重复帧、无关帧、固定边缘、水平回归 |
| Snow Shot | `refer/snow-apps/snow_shot/src/presentation/capture/adaptivescrollingcapturecadence.h` | EWMA 成本、压力降速、健康样本恢复 |
| Snow Shot | `refer/snow-apps/snow_shot/src/presentation/overlay/screenshotoverlayinputhandler.cpp` | scrolling mode 暂停/恢复和选区调整 |
| Snow Shot | `refer/snow-apps/snow_shot/src/presentation/capture/screenshotcaptureworker.cpp` | 捕获 worker 与 UI 解耦 |

参考项目目录只读；实现只能吸收算法原则和测试思想，不能直接复制其 GDI/Qt/C++ 管线。

### 2.6 两份评审报告的完整裁决摘要

下表逐项记录 `docs/Temp/review-report1.md` 和 `docs/Temp/review-report2.md` 的建议，避免实现阶段只选择性引用结论：

| 评审建议 | 最终裁决 |
|---|---|
| WGC/D3D11 主路径，GDI 仅降级 | 吸收；普通 Windows v1 使用 WGC，DXGI 只做显示器级能力降级，BitBlt 不进入滚动主路径 |
| GPU crop/downsample/luma/edge，小范围异步 readback | 吸收；复用现有 D3D11 readback wrapper，基准决定 2/3 槽，Map stall/bytes 纳入门禁 |
| Frame mailbox=1、记录丢帧和状态 | 吸收；latest-frame 覆盖，但 request/session/generation 不可乱序，Dropped 必须诊断 |
| Frame-driven settle，固定 sleep 仅 watchdog | 吸收；QPC、motion energy、粗位移一致性主导 settled |
| overlay 隐藏或真实验证穿透 | 吸收为 P0；不能只设置 WS_EX_TRANSPARENT/HTTRANSPARENT |
| Tauri 仅收低频状态 | 吸收；不传帧、不传 Base64、不发逐帧高频 JSON |
| signed union / position-max_depth | 吸收；core 双向，v1 WheelDriver 向前，避免过早增加 driver 复杂度 |
| overlap 新帧覆盖旧帧 | 吸收但增加质量保护；新帧质量显著下降时保留旧 tile 并标 uncertain |
| 横向 sticky 左/右边缘 | 吸收；统一为 leading/trailing，置信度不足只 mask 不裁剪 |
| NoMovement 二次 probe | 吸收；区分 EOF、输入未生效、无新帧、加载延迟 |
| 导出尺寸和剪贴板保护 | 吸收；增加 pixels/dimension/bytes/time 预算，超大图不直接进剪贴板 |
| profile matcher、动态选带、动态 mask、peak margin | 吸收；作为 v1 核心质量机制 |
| NCC/SAD 保留为困难样本 fallback | 吸收；不每步同时运行全部算法 |
| OpenCV/ORB/AKAZE | 不作为 v1 依赖；真实矩阵证明必要后再做可插拔后端 |
| SendInput + UIPI 诊断 | 吸收；不因为失败就要求用户提权，提示原因并提供手动模式 |
| Manual Panorama | 吸收为 P1；复用同一 matcher/canvas，不与自动 driver 混写状态 |
| HWHEEL 失败回退 Shift/PageKey | 延后；v1 只记录失败，后续 driver 按应用 profile 增加，不混入首版基线 |
| 光标/hover 区域 mask | 吸收；光标静止，动态 mask 排除 tooltip/hover/光标区域 |
| RAII session resource group、全链路有界队列、临时目录清理 | 吸收；统一 Drop/取消/设备丢失/窗口关闭路径 |
| 浏览器 NativeFullPage | 吸收为 BrowserAdapter 未来策略；不能假设任意第三方窗口可调用 Edge/Firefox 内部能力 |
| Screenpresso/竞品 GPU 路线 | 仅作为行业趋势佐证，不作为 SnapClip API 或性能保证 |
| “固定 30 FPS” | 不采用；30 仅 benchmark 起点，按 useful state transitions 和压力控制 |
| Snagit 自动+手动双模式 | 吸收产品原则；自动 WheelDriver + ManualPanoramaDriver |

关键源码证据：

| 项目 | 文件 | 吸收结论 |
|---|---|---|
| Crisp | refer/shot-refer/Crisp-main/src/ScrollCapture.cpp | SendInput、光标停放、等待期间消息泵送、滚动方向探测 |
| Crisp | refer/shot-refer/Crisp-main/src/Stitch.cpp | 轴向共享位移搜索、早停差异计算、拒绝低质量候选 |
| Crisp | refer/shot-refer/Crisp-main/src/StitchVertical.cpp | sticky header/footer、只追加新增条带、终止语义 |
| Crisp | refer/shot-refer/Crisp-main/src/StitchHorizontal.cpp | 横向拼接的对称轴实现 |
| Crisp | refer/shot-refer/Crisp-main/tests/TestStitch.cpp | 已知位移、重复帧、无关帧、页眉页脚和水平测试 |
| Snow Shot | refer/snow-apps/snow_shot/src/presentation/capture/adaptivescrollingcapturecadence.h | EWMA 成本、队列压力降速、健康样本恢复 |
| Snow Shot | refer/snow-apps/snow_shot/src/presentation/overlay/screenshotoverlayinputhandler.cpp | 滚动模式暂停/恢复与选区调整边界 |
| Snow Shot | refer/snow-apps/snow_shot/src/presentation/capture/screenshotcaptureworker.cpp | 捕获 worker 与 UI 解耦 |

### 2.7 Windows 官方约束

- SendInput 将输入串行插入系统输入流，受 UIPI 完整性级别限制；失败记录 GetLastError，并恢复 foreground、focus、cursor。
- WGC Direct3D11CaptureFramePool 适合窗口/显示器 GPU 帧；每帧消费后立即释放。
- `CreateFreeThreaded`/后台 frame handoff 可避免把重处理放入 UI dispatcher；frame 的 `ContentSize` 和 `SystemRelativeTime` 必须保存到 `ScrollFrame`，用 QPC/时间戳判断新帧和动画阶段。
- DXGI Output Duplication 是显示器级降级源；WGC 不可用时才用于桌面区域，不能替代窗口级 WGC。
- WGC 的窗口/显示器互操作入口按系统版本和运行时能力探测；不可用时明确记录 provider，不把 DXGI/BitBlt 伪装成窗口级 WGC。
- DwmFlush 仅允许用于捕获前的合成同步，不得放入鼠标移动、匹配或滚动等待热路径。
- D3D11 staging resource 的 `Map` 可能因 GPU/CPU 同步产生 pipeline stall；必须用异步 copy、有限 staging slot 和小范围读回，不能在每帧 Map 完整 BGRA。

官方资料：
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput
- https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/screen-capture
- https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nn-dxgi1_2-idxgioutputduplication
- https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmflush
- https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11devicecontext-map

竞品/产品资料（用于能力和交互对照，不作为 API 依赖）：

- https://getsharex.com/docs/scrolling-screenshot.html
- https://github.com/ShareX/sharex.github.io/blob/master/changelog.md
- https://www.microsoft.com/en-us/edge/features/screenshot
- https://support.mozilla.org/en-US/kb/take-screenshots-firefox
- https://www.screenpresso.com/releases/screenpresso-2-2-8/
- https://www.techsmith.com/snagit/

## 3. 会话与交互

滚动截图从普通截图的 Settled 选区进入：

~~~
Settled
  -> ScrollArmed
  -> CapturingFirstFrame
  -> Scrolling
  -> Paused
  -> Completed / Partial / Cancelled / Failed
~~~

### 3.1 会话契约

ScrollSession 保存：

- session_id、generation；
- WindowIdentity、snapshot_epoch、monitor/DPI、screen_region；
- ScrollAxis（Vertical 或 Horizontal）；
- source provider、driver、matcher、canvas、request_id；
- 原始 cursor、foreground/focus 和 exclusion 快照。

开始后锁定目标窗口和区域。窗口关闭、HWND 重用、DPI/显示器变化或 exclusion 变化时停止当前会话，不把新几何混入旧画布。所有异步结果检查：

~~~
session_id + generation + source_identity + request_id
~~~

结束、取消和失败都立即释放旧帧、matcher、canvas、tile cache、frame pool、staging texture 和临时文件。下一次 F5 不得读取任何旧滚动状态。

### 3.2 用户操作

- 工具栏明确提供“竖向滚动截图”和“横向滚动截图”。
- 光标停在选区中心，发送真实滚轮；结束后恢复光标和前台窗口。
- 用户可以暂停、继续、取消；暂停后可移动/缩放选区，继续时从新首帧开始。
- Enter 完成已拼出的结果；Esc 取消并清理。
- 对齐失败默认保留已拼部分并显示停止帧、原因和是否可导出，不生成伪完整图。

v1 的 `WheelDriver` 只实现从当前首帧向前滚动（垂直向下或水平向右）。核心画布仍使用 signed coordinate，后续 `PageUp`、向左/向上和手动全景可以复用，不让 v1 driver 同时承担双向复杂度。

### 3.3 滚动期间的 overlay 输入隔离

这是滚轮截图的必要前置条件，不是视觉优化：全屏 overlay 如果继续命中鼠标，SendInput 会被 overlay 消费，目标窗口不会滚动。

- 进入 Scrolling 前，overlay 进入 `ScrollInputPassthrough`。首选方案是临时隐藏全屏交互 overlay，仅保留独立的暂停/取消控制 HWND；若需要继续绘制进度，必须用真实窗口集成测试证明 `WM_NCHITTEST -> HTTRANSPARENT` 在跨线程/跨进程目标上可穿透，不能把 `WS_EX_TRANSPARENT` 或 HTTRANSPARENT 当作无条件保证。
- 不允许只把 `WS_EX_TRANSPARENT` 加到 overlay 后宣称输入已穿透；该样式与鼠标命中、分层窗口和线程归属有关，必须以 `WindowFromPoint`、目标滚动位置和回归测试共同验收。
- driver 将光标移动到目标 region 中心；`WindowFromPoint` 可能返回子控件，必须通过顶层 ancestor/PID/class hash 解析后与锁定的 WindowIdentity 比较，并同时验证 foreground；不匹配时不发送滚轮。
- WGC 窗口捕获可以在 overlay 可见时继续工作；DXGI/BitBlt 显示器降级必须在捕获前隐藏或通过 affinity/exclusion 排除 overlay，完成后再恢复。
- 每一步滚轮、稳定帧采集和状态更新完成后，overlay 不得重新抢焦点；只允许 Esc、暂停和完成命令走控制窗口。
- 任何异常路径都恢复 overlay 命中模式、前台窗口、焦点和光标；不能留下“鼠标点击穿透”或全屏窗口无法操作的状态。

### 3.4 自动输入失败时的手动模式

v1 主路径是 `SendInput`，但必须保留 `ManualPanoramaDriver` 接口：用户自行滚动，SnapClip 只监听稳定帧、估计位移和提交画布。适用于 UIPI/管理员窗口、远程桌面、虚拟机、特殊控件或安全软件拦截输入的场景。

手动模式不能把“收到帧”当成“滚动成功”；仍使用相同 `Alignment`、End Confirmation、CanvasStore 和资源上限。输入失败提示应区分 `InputRejected` 与 `NoMovement`，不得要求用户无依据地以管理员运行。

## 4. 线程和资源架构

~~~
Overlay/message thread
  -> 处理 Esc/暂停/完成，显示低频状态，不等待捕获或匹配

Scroll driver worker
  -> focus/SendInput、稳定等待、步长闭环、边界判定

Capture worker
  -> WGC/D3D11 frame pool、选区裁剪、QPC 时间戳

Matcher worker
  -> 低分辨率多带相关、置信度、实际位移

Canvas worker
  -> union 画布、重叠覆盖、tile/journal

Export worker
  -> 顺序读 tile、PNG/WebP、原子落盘
~~~

捕获策略位于 driver 之上，避免把浏览器特殊能力混入通用路径：

~~~text
CaptureStrategy
  -> NativeFullPageStrategy（未来浏览器/应用适配器，能力探测后才使用）
  -> ScrollAndStitchStrategy（v1 通用窗口）
  -> ManualPanoramaStrategy（输入失败兜底）
~~~

- 所有队列有界；帧信箱容量为 1，最新帧覆盖旧帧，禁止无界 Vec/Channel。
- overlay 只保存会话摘要，不保存完整长图、不调用 WGC/DWM/AcquireNextFrame。
- 复用现有 D3D11 device；沿用当前 3 槽异步 readback 基线，2/3 槽由 benchmark 决定；被拒帧立即释放。任何 `Map` 失败、仍在使用或 device removed 都必须成为显式诊断，不得同步等待 GPU 无限完成。
- staging copy 使用 `D3D11_MAP_FLAG_DO_NOT_WAIT` 时若返回仍在使用，丢弃该次 MatchView 并记录 `readback_not_ready`，由 mailbox 最新帧继续推进；不得在 overlay 或 capture callback 中忙等 GPU。
- 画布使用 256x256 或 512x512 tile，内存只保留有界 LRU；大图写入临时目录，journal + checksum 后原子 rename。
- 默认以 512x512 作为基准，256x256 作为低内存配置；1024x1024 只有基准证明更优时才启用。
- 每个滚动会话拥有独立资源组，Drop 路径必须可在取消、设备移除、窗口销毁和重复启动时执行。

建议类型：

~~~
enum ScrollAxis { Vertical, Horizontal }

struct Alignment {
    axis: ScrollAxis,
    signed_delta_px: i32,
    overlap_px: u32,
    confidence: f32,
    residual: f32,
    status: Accepted | NoMovement | Rejected | Uncertain,
}

struct ScrollFrame {
    session_id: ScrollSessionId,
    source: WindowIdentity,
    axis: ScrollAxis,
    qpc: i64,
    source_size: PhysicalSize,
    crop: PhysicalRect,
    texture: D3D11Texture,
    match_view: MatchView,
    request_id: u64,
}
~~~

ScrollFrame 不能跨线程携带没有明确 apartment/device 约束的裸 COM 指针；使用项目已有 D3D11 wrapper 或明确的 Arc 所有权模型。

帧生命周期必须可诊断，至少区分：

~~~text
Requested -> InFlight -> Dropped -> Arrived -> MatchViewReady -> Accepted/Rejected
~~~

`Dropped` 不得被当成 `NoMovement`；`NoNewFrame` 不得被当成滚动到边界。

停止原因统一为：

~~~rust
enum ScrollStopReason {
    EndReachedByNoMovement,
    EndReachedByUiAExtent,
    InputRejected,
    NoNewFrame,
    AlignmentRejected,
    UncertainAfterRetries,
    WindowChanged,
    DpiChanged,
    MonitorChanged,
    DeviceLost,
    ResourceLimit,
    UserCancelled,
}
~~~

## 5. 滚轮驱动和稳定等待

### 5.1 v1 WheelDriver

- 垂直使用 SendInput + MOUSEEVENTF_WHEEL，水平使用 SendInput + MOUSEEVENTF_HWHEEL；不以 Shift+垂直滚轮替代水平轴。
- 每一步从 1 notch 开始，设置上限，不发送巨大滚轮量。
- SendInput、焦点恢复和滚动等待在 driver worker；输入失败/UIPI 拒绝/目标窗口关闭立即进入 Partial/Failed。
- 主路径不使用 PostMessage(WM_MOUSEWHEEL)：浏览器、Electron、自绘控件和嵌套容器处理不一致。
- driver 不能仅凭输入 API 成功判断页面滚动成功，必须等待帧证据。

### 5.2 自适应 settled

固定 Sleep(260ms) 不能覆盖所有应用。它只能作为 watchdog，不能作为 settled 的唯一依据。每个滚轮动作：

1. 等待至少一个新的 WGC frame/QPC 时间戳；
2. 生成低分辨率 MatchView；
3. 以 QPC 单调、motion energy、粗位移一致性判断 Moving/AlmostStable/Stable；`stable_samples=2` 只是基线，策略可根据刷新率、动态 mask 和置信度选择 1 至 3 个有证据样本，不能无条件等待固定时间或固定帧数；
4. 达到 timeout 仍不稳定则降低步长、延长等待并重试；
5. 超过重试上限返回 Uncertain，不提交中间帧。

初始参数仅作为基线：min_settle=50ms、stable_samples=2、settle_timeout=500ms；最终值由速度矩阵决定。动态视频不能单独决定 settled，多带匹配需要稳定区域投票。settled 是帧证据事件，不是 `Sleep` 到期事件；在证据充分时应提前继续，在证据不足时必须等待或拒绝。

### 5.3 闭环步长

~~~
overlap_px = viewport_axis - abs(delta)
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
~~~

`NoMovement` 不参与 overlap 控制。改变步长产生新的 request_id，重试候选帧只能有一个提交机会。连续 3 步内步长频繁增减时进入 conservative mode：降低采样节奏、增加稳定样本和 timeout。任何 `abs(delta) >= viewport_axis`、overlap 不足或尺寸变化均拒绝。

### 5.4 Snow cadence 的使用

记录 capture、match、canvas commit 的 latest/EWMA：

- 队列深度达到 2 或出现丢帧时立即降低采样频率/步长；
- 连续四个健康样本后每次只恢复一个等级；
- max_fps 30 只作为 benchmark 起点，matcher 实际处理 useful state transitions，最终值由真实成本决定；
- cadence 不能把低置信度帧变成成功，也不能为了速度跳过必要的稳定帧。

### 5.5 终点二次确认

`NoMovement` 只表示当前 step 未观察到有效位移，不直接表示 EOF：

~~~text
Moving -> NoMovement -> Probe -> WaitLoad -> ProbeAgain -> EndConfirmed
~~~

默认执行一次小步长 probe；若连续两次 NoMovement、稳定相似度和画布边缘证据同时成立，才报告 `EndReachedByNoMovement`。若仍有 loading evidence，允许短暂等待；超时报告 `EndUncertain` 并保留 Partial。UIA extent（未来）只能作为额外证据，不能替代帧证据。

## 6. 位移匹配

### 6.1 统一轴接口

~~~
trait ShiftMatcher {
    fn estimate(&self, previous: &MatchView, next: &MatchView,
                axis: ScrollAxis, constraints: MatchConstraints) -> Alignment;
}
~~~

垂直按行方向搜索横向带，水平按列方向搜索纵向带；实现一个轴抽象，不维护两套复制算法。

### 6.2 v1 三级 ProfileMatcher

为满足低 CPU/内存，v1 不强制 OpenCV。默认 matcher 是 `1D Profile + MultiBandConsensus + FineVerification`：

1. GPU 将选区缩小到 1/4（困难样本 fallback 1/2），转 luma/R8/edge；只读回小型 profile 和 band descriptor。
2. 每行/列生成 compact descriptor：mean luma、variance、edge energy 和少量横/纵 bins；不得只使用平均亮度。
3. 在 `expected_delta +/- search_margin` 内做一维 profile SAD 粗搜索；首次、步长大变或异常时才扩大搜索窗。NCC/edge 作为困难或歧义样本 fallback，不是每步都全跑。
4. 通过动态 band 选择后做多带 consensus；候选必须检查 best/second-best margin、band agreement、valid area、residual 和 temporal consistency。
5. 仅在候选 `delta +/- 4..8` physical px 内回读原始窄 overlap strip，做 full-resolution normalized SAD/edge 精修，最终只接受整数像素位移。
6. 页眉、页脚、滚动条、边框、光标和 overlay 由 ValidMask 排除。profile 预处理可在 GPU 上完成；若当前 D3D11 renderer 尚无 compute/shader 路径，允许只对低分辨率 MatchView 做有界 CPU fallback，但必须以 readback bytes 和耗时基准决定是否保留。

质量降级顺序：

1. Profile SAD + 多带 consensus；
2. 困难/歧义样本启用 NCC + edge 或扩大 overlap；
3. 降低 notches、重新采集 settled 帧；
4. 真实样本证明仍失败后，才增加可插拔 ORB/AKAZE；OpenCV 不是 v1 常驻依赖。

以下均返回 Rejected/Uncertain：

- 多带位移不一致；
- best/second-best 候选 margin 不足或重复纹理导致候选不唯一；
- `abs(delta) >= viewport_axis`，或 `overlap_ratio < 0.12`；
- 残差、有效像素或尺寸不合格；
- source identity、generation、request_id 不匹配；
- 不能区分 NoMovement 和错误匹配。

### 6.3 动态选带和短期动态掩码

不要永久固定“上中下”三条带。每个 session 只保留最近 2 至 4 个**微型 profile/统计摘要**（不是完整 MatchView 或 BGRA 帧），按小块/行/列计算短期变化量、纹理复杂度和边缘密度：

~~~text
band_score = texture + edge + temporal_stability
             - dynamic_penalty - sticky_penalty - scrollbar_penalty
~~~

选取得分高且时域稳定的 2 至 3 个带；视频、动画、光标、tooltip、loading 和聊天更新区域形成动态 mask。mask 过大导致 `valid_pixels < minimum` 时必须 `Uncertain`，不能通过降低阈值强行接受。动态 mask 只保存低分辨率短期统计，不能积累历史帧。

### 6.4 动态内容和固定边缘

匹配带不能固定取最顶部。统计相似度、edge similarity、motion ratio 和连续 run length，上限不超过视口三分之一：

- sticky header 只从首帧写一次；
- sticky footer 只从最后稳定帧写一次；
- 所有带动态时不得伪造成功；
- 连续 3 个稳定帧无新增或边缘未变化才判定边界，单帧不变可能是加载动画。

## 7. 画布拼接不变量

### 7.1 双向 union 模型

单轴位置使用相对首帧的有符号坐标：

~~~
next_pos = current_pos + signed_delta
old_union = [min_pos, max_pos + viewport_axis)
new_union = old_union union [next_pos, next_pos + viewport_axis)

只为 new_union - old_union 分配新范围；
old_union 与 next frame 的交集只覆盖，不增加输出长度。
~~~

等价实现可以使用旧文档的 position/max_depth。必须满足：

- 首帧完整写入；
- 后续只新增 union 外条带；
- overlap 允许新帧覆盖旧帧，但逻辑坐标只出现一次；
- NoMovement 不扩展画布，Rejected/Uncertain 不改变画布；
- 向上滚动扩展前缀时移动逻辑原点，不重复追加历史内容；
- 缺失帧、delta 超过视口或检测到 gap 时停止，不用白色/透明像素填洞。

### 7.2 Sticky 区域与一致性检查

固定边缘抽象为 `sticky_leading` / `sticky_trailing`：Vertical 对应 top/bottom，Horizontal 对应 left/right。使用相似度、edge similarity、motion ratio 和连续 run length 估计 `StickyRegion { start_px, end_px, confidence }`，不再要求严格逐像素相等。

- 高置信度：只从首帧写入 leading，只从最终 EndConfirmed 帧写入 trailing；
- 中/低置信度：不主动裁剪真实内容，只把区域加入 matcher mask；
- 横向固定左列/右列检测不稳定时保守保留，不强行删除。

每次提交前检查：

- overlap 像素误差低于阈值；
- 新增范围与旧 union 相邻或相交，不允许 gap；
- 输出 tile 的覆盖 bitmap 无未覆盖像素；
- 画布范围、tile 数量和像素总量未越界。

新帧覆盖重叠区是有意设计：滚动后懒加载完成的内容可以修正旧像素；导出只读取每个逻辑坐标一次。但覆盖必须经过质量保护：若新 overlap 的边缘密度/局部方差/有效像素质量显著低于旧 tile，或残差异常，则保留旧 tile、标记该 commit 为 uncertain 并等待下一稳定帧；不得用占位图无条件覆盖清晰内容。

### 7.3 限制和磁盘安全

- 同时限制轴向长度、总像素、tile 数、临时目录字节和导出时长。初始值可参考旧文档 30,000 px / 150 MP，但必须用 SnapClip 基准重新确定。
- 150 MP 仅是保护阈值，不能 materialize 为连续 BGRA（约 600 MB）；CanvasStore 必须 tile 化、流式写盘，内存只保留有界 LRU。
- 导出还必须检查 `max_export_pixels`、`max_export_dimension`、`max_export_bytes` 和 `max_export_time`；超过编码器安全尺寸时禁用 WebP、分段导出、导出局部或提示裁剪，不把超大图直接写入剪贴板。
- 每次 tile commit 写 session、frame index、canvas range、checksum journal；崩溃恢复只重放完整 journal。
- 每个会话使用独立临时目录，启动时清理过期目录；磁盘不足立即停止并保留可读的 Partial 结果。
- 导出写临时文件，flush 后 atomic rename；失败保留部分结果和诊断，不污染剪贴板/历史。
- 滚动结束显式 drop LRU、frame pool、staging 和 matcher。

## 8. 自动滚动扩展（v2）

滚轮与自动滚动共享接口：

~~~
trait ScrollDriver {
    fn begin(&mut self, target: &WindowTarget, axis: ScrollAxis) -> Result<()>;
    fn step(&mut self, request: ScrollStep) -> Result<ScrollRequestId>;
    fn pause(&mut self);
    fn cancel(&mut self);
    fn end_reached(&self) -> Option<bool>;
}
~~~

实现顺序：

1. WheelDriver：真实 SendInput，支持两轴和速度闭环。
2. PageKeyDriver：PageDown/PageUp/方向键，仍由图像 delta 确认。
3. UiaScrollDriver：仅对已验证窗口直接调用 ScrollPattern/SetScrollPercent，有界超时和 quarantine；不遍历 UIA 全树。
4. 浏览器/特定应用适配器：独立模块，不污染窗口检测和普通截图路径。

driver 成功不等于内容移动；终止仍由 Alignment、重复帧计数和（可用时）UIA extent 决定。

### 8.1 浏览器/应用专用策略

浏览器的 full-page screenshot 是产品能力或 DevTools/扩展能力，不是 SnapClip 可以对任意浏览器窗口直接调用的 Windows API。后续定义 `CaptureStrategy`：

~~~text
NativeFullPageStrategy  -> 能力探测成功且权限/协议可用时使用
ScrollAndStitchStrategy -> v1 通用 WGC 路径
ManualPanoramaStrategy  -> 自动输入不可用时兜底
~~~

`BrowserAdapter` 必须独立处理 Chromium/Edge/Firefox 的协议或扩展通信，明确超时、权限和失败回退；不能让浏览器专用依赖进入普通窗口截图或 docs/14 的窗口检测热路径。

## 9. 性能、日志和前后端边界

### 9.1 热路径红线

- overlay、Tauri command、WebView 不同步等待 WGC、DXGI、DWM 或 SendInput。
- 不把完整帧 JSON/Base64 化，不保存所有历史帧。
- 不为每个滚轮事件创建线程、device、frame pool 或 staging texture。
- 不使用无界 channel、无界临时文件或无界 PNG 压缩队列。

### 9.2 诊断指标

每个 session 低频聚合记录：

- 首帧、每步稳定、capture/match/canvas/export P50/P95；
- requested notches、实际 delta、overlap%、confidence、residual、rejected/uncertain/no-movement；
- matcher 的 candidate_count、best/second-best score、peak margin、band deltas、valid pixels、动态 mask 面积；
- 信箱峰值/丢帧、tile cache 命中率、临时目录字节；
- GPU preprocess、readback bytes/frame、readback bytes/session、pixels examined/accepted、tile write MB/s；
- CPU、Private Bytes、Working Set、GPU dedicated/shared memory；
- 输出长度、总像素和交界验证失败位置；
- 每次停止必须记录 `ScrollStopReason`：`EndReachedByNoMovement`、`EndReachedByUiAExtent`、`InputRejected`、`NoNewFrame`、`AlignmentRejected`、`UncertainAfterRetries`、`WindowChanged`、`DpiChanged`、`MonitorChanged`、`DeviceLost`、`ResourceLimit`、`UserCancelled`。

默认不逐帧洪泛；diagnostics 开启才记录每步摘要。

Tauri/Vue 只接收低频状态小消息，例如：

~~~ts
type ScrollProgress = {
  sessionId: string;
  state: 'armed' | 'capturing_first_frame' | 'waiting_settle' | 'matching' |
    'scrolling' | 'paused' | 'partial' | 'completed' | 'cancelled' | 'failed';
  axis: 'vertical' | 'horizontal';
  capturedLengthPx: number;
  viewportLengthPx: number;
  stepIndex: number;
  lastDeltaPx?: number;
  lastConfidence?: number;
  stopReason?: string;
};
~~~

前端不接收纹理、完整帧、画布像素或高频逐帧 JSON；预览只使用完成/部分结果后的本地缩略图路径。

## 10. 测试和验收

### 10.1 Rust 纯单元测试

- 垂直/水平轴访问器、正负 delta 和半开区间；
- 已知位移多带匹配、动态带投票、固定页眉/页脚和滚动条 mask；
- profile descriptor、best/second-best margin、重复纹理歧义和动态 mask 选带；
- NoMovement、无关帧、尺寸变化、低置信度、多带不一致、超过硬下限和 overlap 不足拒绝；
- position/max_depth 或 min/max 在向下、向上、往返时不膨胀；
- overlap 覆盖、union 外只新增一次、无 gap、逻辑坐标无双写；
- session/generation/request stale 结果丢弃；
- tile cache、journal、checksum、上限、取消释放；
- cadence 压力降速、健康恢复、步长闭环。

测试帧用程序生成可逆行/列纹理、固定边缘、局部动态噪声和已知 offset，逐像素比较期望输出。

### 10.2 Windows 集成测试

- WGC 首帧/后续帧尺寸、DPI、QPC 单调，设备移除和窗口关闭释放 frame pool；
- 可滚动测试窗口验证竖/横滚轮、平滑动画、不同 wheel delta、固定页眉/页脚和边界；
- Win32/WPF/WinUI/Electron/Chromium/自绘窗口的真实输入链路；管理员/UIPI、RDP/VM、触摸板平滑滚动和低纹理/重复纹理页面；
- SendInput 成功/失败/UIPI 时恢复 foreground、focus、cursor；
- overlay、工具栏和主窗口不进入捕获，检测排除与捕获排除独立断言；
- tile 输出、checksum、PNG 原子落盘，中途取消/磁盘错误不产生伪完整文件。

### 10.3 速度-质量矩阵

在静态文档、浏览器懒加载页、动态聊天/视频页，分别测试竖/横轴：

| 变量 | 级别 |
|---|---|
| wheel step | 1、2、3、6 notches |
| 动画 | 关闭、系统平滑、应用自定义 |
| settle timeout | 50、100、180、260、500 ms |
| 视口 | 800x600、1920x1080、4K 选区 |
| 方向 | 竖、横、往返 |

每个组合至少 10 次，并增加权限（普通/管理员）、输入设备（鼠标/触摸板）、RDP/VM、导出大小和资源回落维度。记录重复行/列、空白像素、错位率、误接受/拒绝、提前终止、每步延迟、总吞吐、CPU、内存、GPU、readback bytes 和磁盘。默认参数只能从数据确定；无法可靠对齐时必须降速、重试或返回部分结果。

### 10.4 质量门禁

- `blank_pixels = 0`、unexpected transparent gaps = 0、illegal duplicate logical range = 0；任何 gap 都只能停止并报告 Partial。
- 重点指标是错误位移被 `Accepted` 的比例（false acceptance），而不是单纯追求低 false rejection；重复纹理、低纹理和动态页面允许返回 `Uncertain`。
- 对每次 `Accepted`，测试必须能回溯到 candidate margin、band consensus、valid area、fine residual 和 overlap 验证证据。
- 默认参数、质量阈值和 tile/readback 预算只有在矩阵数据和性能采样归档后才能冻结。

### 10.5 人工验收

1. 浏览器长页面竖向截图：页眉一次、无重复段和白带。
2. 宽表格/画廊横向截图：列顺序和交界正确。
3. 快速/慢速、平滑滚动、滚到边界、手动回滚：画布不膨胀；v1 自动 WheelDriver 仍只向前滚动。
4. 动态聊天、懒加载和固定输入框：稳定等待及 sticky footer 正确。
5. 暂停、调整、继续、Esc、重复开始：无旧帧闪现、输入卡住或资源泄漏。
6. 多显示器、负坐标、混合 DPI、WGC 降级路径。
7. 20 次循环后线程、窗口、GPU resource、Private Bytes 和临时目录回落。

## 11. 实施阶段

### Phase S0：基线和夹具

阅读 docs/14、docs/07、docs/06、旧项目长截图文档、Crisp/Snow 源码；建立合成帧、可滚动测试窗口、像素级基线；冻结 ScrollAxis、ScrollSession、Alignment、TileStore 和 driver trait；运行截图/窗口检测/GPU/前端基线。

### Phase S1：纯拼接核心

实现轴抽象、1D ProfileMatcher、多带 consensus、动态 mask、sticky 置信区间、End Confirmation、双向画布、拒绝状态、硬上限和纯单元测试；不接真实滚轮/WGC。

### Phase S2：WGC/D3D11 活动帧源

复用现有 device 和当前 WGC/D3D11 wrapper，建立 session-owned 短生命周期 frame pool、crop/readback、容量 1 信箱；验证 `ContentSize`/`SystemRelativeTime`、设备移除、窗口关闭和取消。不得把现有一次性 FrozenFrame API 改成隐式循环接口。

### Phase S3：WheelDriver

接入 SendInput、焦点/光标恢复、frame-driven settled、overlap 控制步长、End Confirmation、低频状态事件；先竖向，再用同一轴抽象接横向。v1 只向前滚动，同时建立 ManualPanoramaDriver 接口。

### Phase S4：TileStore 与导出

实现 journal/checksum、LRU、原子 PNG/WebP、部分结果和内存回落测试。

### Phase S5：自动滚动和性能收口

接入 PageKey/UIA driver 的有界超时和 fallback，评估 BrowserAdapter/NativeFullPage；完成速度-质量矩阵及 WPR/WPA/PresentMon 或等价采样；只根据数据调整阈值，不预置无证据的 OpenCV、R-tree 或额外常驻进程。

每阶段均需：修改前基线、修改后新旧回归、真实测试证据、独立提交；与 docs/14 冲突时先记录证据并修订设计，不在实现中默默偏离。

## 12. 结论

SnapClip 滚动截图应采用“活动 GPU 帧源 + 有界容量 1 流水线 + 实际位移匹配 + union 画布 + 磁盘 tile 输出”，而不是固定高度截图后直接拼接。

推荐落地顺序：

1. 纯 Rust 轴向多带匹配和双向画布逐像素测试；
2. WGC/D3D11 活动帧源和容量 1 信箱；
3. SendInput 竖/横滚轮、稳定等待和闭环步长；
4. 交界验证、拒绝不可信帧、tile 和原子导出；
5. 用速度-质量数据确定默认参数，再扩展 PageDown/UIA 自动滚动。

该边界满足低延迟、低占用和可验证性：前端只接收低频状态，overlay 不被长截图阻塞，拼接错误不会被白色填充掩盖，未来更换滚动驱动或 matcher 不会破坏 docs/14 的窗口检测核心。
