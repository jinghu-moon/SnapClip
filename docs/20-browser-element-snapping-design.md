# SnapClip 浏览器内部元素盒子智能吸附设计

> 文档状态：实现设计稿（基于 `docs/14` 窗口快照、`docs/18` UIA 深选和
> `refer/snow-apps/snow_shot` 源码调研）
>
> 目标：在已有顶层窗口外框吸附的基础上，为 Chrome、Edge、Firefox 等浏览器的网页内容提供
> 稳定、低延迟、可降级的元素盒子吸附。浏览器元素不是 HWND 子窗口，不能依赖
> `EnumChildWindows` 得到 DOM 盒子。

## 1. 结论

1. **通用主路径是 UI Automation（UIA）深选**。窗口仍由 `docs/14` 的 `WindowSnapshot` 命中，
   UIA 只在 `refinement_worker` 的独立 COM 线程中按需展开该窗口的无障碍树。不能在
   `WM_MOUSEMOVE`、overlay 线程或 v1 detection worker 同步调用 UIA。
2. **浏览器元素不是“最深 DOM 节点”**。默认选择光标所在的最小、可见、非退化的
   `InteractiveControl` 或 `VisualRegion`；纯布局容器只有在没有更具体候选时才作为回退。
   这避免文字 `span`、图标、伪元素导致选框过细、抖动或不可用。
3. **UIA 失败时保持 v1 整窗吸附**，必要时按顺序尝试 MSAA，再尝试真实 child HWND 链。
   fallback 只能细化，不能把已有的细元素替换成更粗的矩形；所有结果必须绑定窗口身份、
   snapshot epoch、request id。
4. **CDP/浏览器扩展是可选的高精度专用通道，不是通用后门**。CDP 能读取 DOM/CSS/layout
   盒子，但任意正在运行的 Chrome/Edge 默认没有可用调试端口。该通道只用于 SnapClip
   自己托管的 WebView2、用户明确启用扩展，或用户明确授权的远程调试实例。
5. **所有最终矩形统一为虚拟桌面物理像素**。UIA 的 `BoundingRectangle` 官方定义就是物理屏幕坐标；
   CDP 返回 CSS px，必须叠加页面缩放、device scale factor、浏览器 viewport 偏移、窗口屏幕坐标
   和当前滚动偏移后再发布。
6. **预览可以异步变深，确认必须重新验证**。鼠标停稳后约 80 ms 发起单飞请求；结果迟到、导航、
   重排、窗口移动或 epoch 改变时丢弃。单击/Enter 确认前重新验证元素仍属于相同窗口和页面版本。

## 2. 现有实现与参考项目结论

### 2.1 SnapClip 当前基线

- `src-tauri/src/capture/window_detection/model.rs` 已有 `WindowIdentity`、`SnapshotEpoch`、
  `WindowTarget` 和 `TargetKind`。
- `src-tauri/src/capture/window_detection/deep.rs` 已有 80 ms dwell、单飞、容量 1 最新点、
  epoch/request-id 丢弃、取消、预算和 `DeepTarget` 契约。
- `src-tauri/src/platform/windows/capture/refinement_worker.rs` 已隔离 COM worker。
- `uia_provider.rs` 已按 `ElementFromHandle`、批量属性缓存、按层展开、同边界结构容器处理、
  epoch 缓存和 quarantine 实现；`msaa_provider.rs` 提供有时限的 `IAccessible` fallback。
- 因此本任务首先是**浏览器语义和边界补齐**，不应另建一条同步命中链路或复制 worker。

### 2.2 snow-shot / snow-ui-selector 可吸收的实现

调研源码：

- `refer/snow-apps/snow_shot/src/presentation/selector/screenshotselectorcoordinator.cpp`
- `refer/snow-apps/snow_shot/src/presentation/selector/screenshotselectorserviceclient.cpp`
- `refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/uia.rs`
- `refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/uia/cache.rs`
- `refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/msaa.rs`

应吸收：

- 窗口刷新、普通命中、深选 refinement 三层隔离；普通命中不访问 UIA。
- 80 ms 停稳、单飞 refinement、移动即取消、只保留最新点；**每个新的光标位置都重新发起
  refinement**，性能由 provider 的已展开批次缓存保证，不能由“命中已发布路径”抑制查询。
- `ElementFromHandleBuildCache`/`ElementFromPoint` 配合 `TreeScope_Element | TreeScope_Children`
  一次批量获取命中元素和一层属性，避免每个节点逐项跨进程调用；只写 `Children` 会使命中元素
  自身没有缓存矩形。
- 子节点批次缓存按元素身份而非矩形键控；同边界的 Chromium/Explorer 容器可能是不同节点。
- UIA sibling 顺序不是视觉 Z 序；命中候选按几何过滤，结构性 `Pane/Group` 允许回溯，不能靠
  control type 黑名单粗暴删除。
- 缓存、quarantine、epoch 和服务释放都有明确生命周期；关闭会话或快照变化必须释放旧路径。
- snow-shot 的 `canRefine`、`epoch/requestId/generation` 协议和 phased performance tests 可作为
  SnapClip 的调度与验收模板。

不能照搬：

- snow-shot 的 macOS Accessibility、Qt/C ABI 和平台权限模型不属于 SnapClip Windows 路径。
- snow-shot 的窗口空间索引不解决浏览器树遍历瓶颈；SnapClip 仍以 UIA provider budget 为主。
- 参考项目的“最深可访问节点”不能直接作为产品选择规则，必须经过粒度策略过滤。

## 3. Windows 浏览器可用的三层后端

### 3.1 UIA：默认、无注入、跨浏览器

UIA 的 `IUIAutomation::ElementFromHandle` 可用顶层 HWND 获取根元素；对 Chromium/Edge 等网页
内容，`IUIAutomation::ElementFromPoint` 也必须作为**元素候选入口**保留。浏览器的网页树经常
从根开始呈现两个重叠 `Pane`，其中面积较小的节点可能是没有 children 的死叶子；只从根按面积
向下走会停在错误 Pane。`ElementFromPoint` 先给出 provider 当前认为位于光标下的元素，再沿父链
回溯到已命中的窗口根，才能同时得到浏览器实测可用性和窗口归属安全性。

浏览器把网页的可访问树投影为 UIA 节点：按钮、链接、输入框、图片、列表项以及部分结构容器
都有矩形和 control type。

使用规则：

1. v1 快照命中的 `WindowIdentity` 仍是窗口归属的唯一来源。`ElementFromPoint` 只能提供候选，
   **不能用来发现或替换目标窗口**。
2. 对候选执行归属证明：用 UIA parent walker/BuildCache 沿父链回溯，使用 `CompareElements`
   与 `ElementFromHandle(snapshot.hwnd)` 比对；同时校验 PID、窗口 identity 和矩形包含关系。
   不能只依赖网页节点的 `NativeWindowHandle`，Chromium 通常只在最外层节点暴露该属性。
   无法证明归属时丢弃候选并回落整窗。
3. 根元素和每层 children 都读取 `BoundingRectangle`、`IsOffscreen`、`ControlType`、
   `IsControlElement`/`IsContentElement`（若 provider 支持）。
4. `CacheRequest` 的 tree scope 必须包含 `Element | Children`；先消费 point-hit 候选，再按
   provider 顺序和可继续展开性走查包含光标的分支。最多 `MAX_DEPTH`、`MAX_NODES`、`MAX_PATH_LEN`，并遵守
   `REFINEMENT_BUDGET_MS`/`REFINEMENT_CALL_LIMIT_MS`。
5. 同边界 `Pane`/`Group`/未知结构节点继续下钻；存在具体控件时不把结构节点作为最终目标。
6. UIA 返回的矩形仍需与窗口屏幕矩形、当前捕获显示器矩形求交，空矩形和完全离屏矩形丢弃。

官方依据：

- Microsoft `ElementFromHandle`：<https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomation-elementfromhandle>
- Microsoft UIA 元素获取：<https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-obtainingelements>
- `UIA_BoundingRectanglePropertyId` 是物理屏幕坐标，但不保证可点击区域，且可包含被遮挡部分：
  <https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-automation-element-propids>
- Edge 网页 ARIA/无障碍树到 UIA 映射：<https://learn.microsoft.com/en-us/microsoft-edge/accessibility/build/aria-and-ui-automation>

### 3.2 MSAA：兼容性后备

当 UIA 不支持、树为空或 provider 超时时，refinement worker 可使用
`AccessibleObjectFromWindow(hwnd, OBJID_WINDOW, IAccessible)`，再执行有界 `accHitTest`/`accLocation`。
MSAA 调用可能阻塞，必须继续使用现有 `TimedCallRunner`、超时 quarantine 和独立线程。

`AccessibleObjectFromPoint` 不能作为窗口发现或 overlay 热路径入口；但在 refinement worker 中，
它可以作为已命中窗口内的 MSAA 候选来源。结果必须沿 accessible parent/identity 证明属于快照窗口，
并通过 HWND/PID/class/epoch 验证；证明失败就回落整窗。MSAA 的 `accHitTest` 从窗口根开始是同样
的候选路径。

官方依据：<https://learn.microsoft.com/en-us/windows/win32/api/oleacc/nf-oleacc-accessibleobjectfrompoint>

### 3.3 BrowserAdapter：可选的 DOM/CSS 高精度后端

BrowserAdapter 不改变 UIA 默认路径，只在满足授权条件时启用：

- SnapClip 自己托管的 WebView2：使用 WebView2 的 DevTools Protocol 能力；
- 用户安装并启用 SnapClip 浏览器扩展：扩展通过 `runtime.connectNative`/本地 adapter 传递 DOM 命中；
- 用户显式提供远程调试端口且 SnapClip 能验证浏览器 PID、窗口、tab 和 session。

CDP 查询建议：`DOM.getNodeForLocation` → `DOM.getBoxModel`，必要时使用 `Page.getLayoutMetrics`、
`Runtime` 或 `Accessibility` domain。结果必须携带：browser PID、tab/target id、CDP session id、
document/navigation id、node/backendNode id、CSS box、device scale factor、scroll offset。

禁止：扫描端口猜测任意浏览器、把网页 URL 当身份、把 CDP 节点 id 跨导航复用、把 WebView2 的
CDP session 当成外部 Edge/Chrome 的通用能力。

官方依据：

- Microsoft Edge DevTools Protocol：<https://learn.microsoft.com/en-us/microsoft-edge/devtools-protocol/>
- WebView2 `ICoreWebView2_13`：<https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2_13>
- Chrome UIA 支持说明：<https://developer.chrome.com/blog/windows-uia-support>
- Chromium UIA 设计：<https://chromium.googlesource.com/chromium/src.git/+/refs/heads/main/docs/accessibility/browser/uiautomation.md>

## 4. 浏览器元素目标契约

当前 `DeepTarget` 需要扩展为可区分“UIA 元素”和“浏览器 DOM 元素”的纯数据结果。COM 对象、
CDP socket、扩展连接都不得跨 worker。

```rust
enum ElementBackend {
    Uia,
    Msaa,
    BrowserAdapter,
}

enum ElementGranularity {
    SemanticElement,    // 页面语义节点：button/link/input/image/list item
    InteractiveControl, // 可操作控件，默认优先
    VisualRegion,       // 可见内容区域，作为较粗 fallback
}

struct BrowserElementTarget {
    window: WindowIdentity,
    snapshot_epoch: SnapshotEpoch,
    request_id: RequestId,
    backend: ElementBackend,
    granularity: ElementGranularity,
    screen_bounds: Rect,      // 虚拟桌面物理像素，已裁剪
    visible_bounds: Rect,     // 当前显示器/捕获区域交集
    path: Vec<Rect>,          // 窗口 -> viewport/container -> element
    identity: ElementIdentity,
    confidence: u8,
    stop_reason: StopReason,
}
```

`ElementIdentity`：

- UIA/MSAA：runtime id（若有）、provider PID、窗口 identity、control type、稳定路径签名；
- BrowserAdapter：browser PID、tab target/session、document/navigation id、backendNode id。

身份只是确认时的验证线索，不是永久句柄。矩形、页面导航、窗口 bounds、显示器 DPI 任何一项
发生变化，都使旧目标进入 `Stale`。

## 5. 候选选择与浏览器语义

### 5.1 三种粒度

默认策略为 `InteractiveControl`，设置中可切换：

| 粒度 | 目标 | 适用 |
| --- | --- | --- |
| `SemanticElement` | button、link、input、image、heading、list item 等语义节点 | 便于复制网页控件 |
| `InteractiveControl` | 可点击、可输入、可滚动、可拖拽的控件 | v1 浏览器元素吸附默认 |
| `VisualRegion` | 页面中可见文本/图片/卡片/容器区域 | 没有交互模式或 UIA 树不完整时 |

不要默认选最深 leaf。文本 span、SVG path、装饰 icon、伪元素的矩形可能小于可操作区域；
对同一光标点，候选排序为（这是 Chromium 能否走到网页内容分支的硬规则）：

1. 位于目标窗口且 `visible && !offscreen && !empty`；
2. **能继续展开且存在可解释 children 的分支优先于已知死叶子**；
3. 同一层多个包含候选时优先 provider 列表中更靠后的候选（Chromium 实测为上层/内容分支），
   但保留同边界结构节点向更早 sibling 回溯的规则；
4. 交互性/语义等级符合当前粒度；
5. 包含光标且矩形与父节点有合理包含关系；
6. 具体控件优先于纯布局容器；
7. 面积更小只作为**同一可展开/语义等级内的次级排序**，并设置最小宽高/面积阈值，避免
   不可用微小目标；
8. 同等级仍相同时用路径稳定签名、几何位置、provider 顺序作确定性裁决。

### 5.2 结构容器与 Chromium 特殊情况

Chromium/Edge 可能暴露多个相同矩形的 `Pane`/`Group`，其中一个分支只是结构包装，另一个分支
才包含真实网页内容。实现必须保留可回溯分支：

- 同边界结构节点继续展开；
- 当前分支无具体候选时回到父节点，尝试同边界 sibling；
- 真实控件或不同边界容器不因“结构”标签被跳过；
- 不把完整 UIA 树预取到内存，只缓存已展开的 batch。

浏览器实测约束必须写进实现和探针：

| 现象 | 规则 |
| --- | --- |
| 根下两个重叠 Pane，较小者 `children=0` | 不能按面积先选；先选可继续展开/内容分支，再按 provider 顺序裁决 |
| `TreeScope_Children` 只返回子级属性 | cache request 必须是 `Element | Children`，否则命中元素自身无矩形 |
| 网页节点通常没有 `NativeWindowHandle` | 用 `CompareElements` + parent chain + PID/窗口 identity 归属证明 |
| `<a>`/`group` 下暴露 `Text`，部分 `div` 不进树 | 文字块只有在 `VisualRegion` 或无更具体候选时发布；默认不把装饰文字当 InteractiveControl |
| 浏览器树惰性物化，首读可能是 `Pane(0,0)` 或 `raw=0` | 返回 `AccessibilityPending`，不把占位态当最终失败；下一次 dwell 重试 |
| 同 epoch 缓存可能与当前点不解释 | 空批次不缓存；缓存批次没有包含候选时重新读取一次；仍无候选才结束本次查询 |
| 无子窗口的 DComp overlay 不会被 `ElementFromPoint` 返回 | 不把 overlay 命中当作通用排除依据，仍使用显式 excluded HWND/PID 和 epoch 验证 |

`ElementFromPoint` 的候选命中和根节点下钻是互补路径：浏览器优先 point-hit，传统 Win32/无障碍
树完整的窗口可使用 root-first；两者都必须回到同一个已验证的 v1 窗口身份。

### 5.3 浏览器状态导致的失效

以下事件清除 BrowserAdapter 目标并让 UIA 重新 refinement：导航、刷新、SPA 路由变化、DOM 大范围
重排、tab 切换、页面缩放、DevTools 打开/关闭、浏览器窗口移动或跨屏、滚动位置改变。

UIA 的 `IsOffscreen=true` 表示元素已滚出或折叠，不等同于被别的窗口遮挡；遮挡判断仍依赖窗口快照
和捕获画面，不能仅用 `IsOffscreen` 推断完全不可见。

## 6. 坐标、DPI 与滚动换算

### 6.1 UIA 坐标

UIA `BoundingRectangle` 直接视为虚拟桌面物理像素，但必须验证：

```text
element_screen = UIA_BoundingRectangle
element_visible = intersect(element_screen,
                             window_candidate.screen_bounds,
                             capture_monitor.screen_bounds)
```

不得把 UIA 矩形再次乘 DPI。overlay 绘制前才调用既有 `window_rect_to_local` 转成本显示器局部坐标。

### 6.2 CDP 坐标

CDP 的 CSS px 不是屏幕物理像素。转换必须记录一次一致的 `LayoutSnapshot`：

```text
screen_px = browser_viewport_screen_origin_px
           + (css_box - visual_viewport_offset_css)
             * device_scale_factor
```

其中 `browser_viewport_screen_origin_px` 由浏览器内容区/窗口外框在同一 epoch 重新测量；不能用
固定标题栏高度。页面缩放、OS Per-Monitor-V2 DPI、`deviceScaleFactor`、visual viewport、
滚动 offset 和 iframe 坐标都必须来自同一 snapshot。

跨 iframe 时沿 frame chain 累加 offset；无法验证 frame/tab/session 时放弃 CDP 结果，回落 UIA。
CSS box 如果跨屏，按当前捕获显示器交集裁剪；最终只发布非空矩形。

### 6.3 滚动与动态页面

滚动改变元素的 screen rect，不必把旧 DOM node 当成稳定目标。CDP 使用同一 document id 重新读
box model；UIA 使用当前缓存 epoch 的元素重新读取 bounds。连续几次观察的 identity+rect 都一致后
才替换预览，避免滚动/动画造成抖动。

## 7. 调度、缓存与线程边界

```text
overlay thread
  ├─ 处理鼠标、120 ms 整窗 dwell、80 ms 元素 refinement dwell
  ├─ 只读 WindowSnapshot/已验证路径
  └─ 绘制 preview，不执行 COM、DWM、CDP、IPC

detection worker
  └─ EnumWindows/DWM/窗口 identity 验证（docs/14）

refinement worker (COM apartment)
  ├─ UIA 主路径
  ├─ MSAA fallback
  ├─ 可选 BrowserAdapter bridge
  └─ 预算、取消、缓存、quarantine
```

硬约束：

- 同一 session 同时最多一个 refinement；邮箱容量 1，只保留最新点。
- 结果必须检查 `session_id + snapshot_epoch + request_id + WindowIdentity`，任何不符直接丢弃。
- UIA child batch 缓存按元素身份键控，epoch 变化清空；不缓存完整无界树。
- 每个新的光标位置都重新执行 refinement；调度器不得因为点仍在已发布矩形内而跳过查询。
  缓存只减少 provider 调用，不改变“当前位置决定目标”的产品语义。
- 空批次不写入长期缓存；缓存批次无法解释当前点时最多重新读取一次，防止动态页面复用已失效结构。
- UIA provider 失败的窗口 quarantine 到下一次 snapshot refresh；不要每次移动重复撞同一个挂死 provider。
- BrowserAdapter 连接/事件只在其 worker 侧，overlay 只接收序列化 `BrowserElementTarget`。
- 目标矩形连续稳定后才能显示；目标变更沿用 `RectTransition` 的 101 ms OutQuad，已确认选区不动画。

## 8. 预览与确认流程

1. v1 窗口快照命中后立即显示整窗 hover/预览，保证 UI 不等待浏览器树。
2. 光标停稳 80 ms，向 refinement worker 提交当前窗口 identity、点、epoch 和 request id。
3. worker 先尝试 UIA；仅在 UIA 结果为空/不支持/超时才进入 MSAA；BrowserAdapter 由策略显式选择，
   不在 UIA 失败后自动扫描未知调试端口。
4. 返回路径后做候选粒度过滤、坐标裁剪和稳定性滞后；通过后替换预览，失败保持整窗。
5. 鼠标移动、拖拽、Esc、会话结束或快照变化会取消并使旧结果失效。
6. 左键单击/Enter 确认前重新验证窗口 identity、元素 identity、epoch、矩形和页面版本；失败时
   刷新一次并重命中，仍失败则保持确认前选区，不凭空进入拖拽。

## 9. 分期实施

| 阶段 | 工作 | 验收 |
| --- | --- | --- |
| B0 | 固化 `BrowserElementTarget`、`ElementIdentity`、粒度策略和坐标纯函数 | Rust 单测覆盖裁剪、排序、最小尺寸、epoch/request 丢弃 |
| B0.5 | 浏览器元素可达性门禁：`ElementFromPoint` 候选、归属证明、`Element | Children` cache、每点重查 | 自动 Chromium 夹具必须逐点比较页面自报盒子、expected、published；Explorer 网格点必须有记录，未通过不得进入 B1 |
| B1 | 将现有 UIA provider 补成浏览器粒度策略；保留结构容器回溯、可展开分支优先与 batch cache | Chrome/Edge 真机探针；不影响整窗吸附 |
| B2 | UIA 稳定性滞后、滚动/导航失效、确认前重新验证 | 页面滚动、SPA、缩放、跨 DPI、多显示器回归 |
| B3 | MSAA/HWND fallback 完善与 quarantine 指标 | UIA 禁用/超时/空树仍可整窗吸附；无 overlay 卡顿 |
| B4 | BrowserAdapter trait；先接 SnapClip WebView2 或显式扩展，不接任意浏览器端口 | tab/session/document 身份校验、CSS→物理像素测试 |
| B5 | 性能与可靠性 | P50/P95 refinement 延迟、overlay 输入延迟、worker CPU、缓存命中率和 stale 丢弃率有基线 |

## 10. 测试矩阵

### 10.1 纯单元测试

- UIA/DOM 矩形与窗口/显示器交集、负虚拟桌面坐标、跨屏裁剪；
- CSS px、DPR、浏览器 viewport origin、visual viewport offset、滚动 offset 换算；
- `SemanticElement`/`InteractiveControl`/`VisualRegion` 候选排序和最小尺寸阈值；
- `ElementFromPoint` 候选的 parent-chain/`CompareElements` 归属证明；`Element | Children` cache scope；
- 可展开分支优先、provider 顺序优先于面积、死叶子回溯；空批次不缓存和“缓存无法解释点则重读一次”；
- 同边界结构节点回溯、重复矩形去重、fallback 只能细化；
- 稳定性滞后、RectTransition、epoch/request/session 丢弃；
- 导航/document id、tab/session、窗口 identity 不匹配时拒绝确认。

### 10.2 Windows 集成验收

- Chrome、Edge：按钮、链接、输入框、图片、文本卡片、滚动容器、iframe；这些是 v1 浏览器
  元素吸附的强验收对象。
- Firefox：先验证是否暴露足够的元素级矩形；若 provider 只提供窗口/粗粒度树，**整窗降级是合格结果**，
  不得为了通过验收而引入同步注入或无限遍历。
- Chromium 同边界 Pane/Group 不遮蔽真实网页内容；
- 浏览器缩放 100/125/150%、Windows DPI 100/150%、负坐标显示器；
- 页面滚动、动态列表、SPA 路由、刷新、DevTools、窗口移动/关闭后重新吸附；
- UIA 禁用或超时、MSAA 命中、无障碍树为空时整窗降级；
- **自动夹具门禁**：测试进程必须 Per-Monitor-V2；页面通过页面脚本/标题或扩展自报元素盒子，
  harness 记录 `point / expected / backend / published / stop_reason / elapsed_us`，逐点断言误差、
  目标身份和降级原因。不得以肉眼观察截图框代替该门禁。
- BrowserAdapter 仅在明确授权的 WebView2/扩展/调试实例生效，任意普通浏览器不误连接。

### 10.3 性能基线

记录：整窗命中延迟、80 ms 后到元素预览的 P50/P95、overlay `WM_MOUSEMOVE` 处理耗时、
refinement worker CPU、每窗口缓存节点数、provider 调用数、quarantine 命中数、stale 丢弃数。

目标不是先写死阈值，而是用 Chrome/Edge/Firefox 的固定页面建立基线；任何优化必须以 profiling
或基准证明为依据。严禁用增大预算、同步查询或预取整棵 DOM/UIA 树换取“命中率”。

## 11. 明确不做

- 不在 overlay/`WM_MOUSEMOVE` 同步访问 UIA、MSAA、CDP 或浏览器 IPC；
- 不因为光标仍在已发布路径内而抑制 refinement；当前位置变化必须重新查询；
- 不把 `EnumChildWindows` 当成浏览器 DOM 检测；
- 不把 CDP 当成任意 Chrome/Edge 的默认可用接口；
- 不根据 URL、标题或进程名单独确认 tab/元素身份；
- 不默认选择最深 DOM leaf，不缓存无界 UIA/DOM 树；
- 不让浏览器深选失败清除 v1 整窗目标；
- 不把 UIA 的“可见”误解为没有被其他窗口遮挡；
- 不为了兼容旧实现保留第二套同步 selector。

## 12. 参考资料

- SnapClip：`docs/14-screenshot-window-detection-design.md`
- SnapClip：`docs/18-uia-deep-selection-design.md`
- snow-shot：`refer/snow-apps/snow_shot/src/presentation/selector/screenshotselectorcoordinator.cpp`
- snow-ui-selector：`refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/uia.rs`
- snow-ui-selector：`refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/uia/cache.rs`
- Microsoft UI Automation 总览：<https://learn.microsoft.com/en-us/windows/win32/winauto/entry-uiauto-win32>
- Microsoft UIA Tree Overview：<https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-treeoverview>
- Microsoft UIA Control Types：<https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-controltypesoverview>
- Microsoft Edge ARIA/UIA：<https://learn.microsoft.com/en-us/microsoft-edge/accessibility/build/aria-and-ui-automation>
- Chrome UIA：<https://developer.chrome.com/blog/windows-uia-support>
- Chromium UIA：<https://chromium.googlesource.com/chromium/src.git/+/refs/heads/main/docs/accessibility/browser/uiautomation.md>
- Edge DevTools Protocol：<https://learn.microsoft.com/en-us/microsoft-edge/devtools-protocol/>
- WebView2 DevTools Protocol：<https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2_13>
