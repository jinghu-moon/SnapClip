# SnapClip 截图窗口边界自动识别设计方案

> 文档状态：实现定稿（已按三轮审核报告及自动吸附交互复核，v1 落地 + v2 预留）
>
> 适用平台：Windows 10/11 x64，Per-Monitor-V2 DPI awareness
>
> 目标：截图进入选区阶段时，自动识别屏幕上各个窗口的边界（位置 + 宽高），支持"光标停稳后自动吸附最近的顶层窗口外框"，同时不破坏现有手动拖拽框选。
>
> 重要原则：本项目尚未正式发布，允许破坏性重构。方案以根因解决、正确性、性能与合理架构为准，不为兼容旧实现引入过渡层。完成标准是根因解决 + 功能正确 + 相关已有功能正常 + 验证通过。

文档关系：`07-screenshot-recording-architecture.md` 定义捕获归属与 WGC/DXGI 边界；`11-screenshot-fullflow-ui-refactor-tasklist.md` 统一截图全流程、选区控件与 UI 重构顺序。本文件聚焦"窗口边界自动识别与吸附"这一子能力，是 `11` 中选区交互的增量设计；三者冲突时，窗口识别相关以本文件为准。

---

## 1. 结论先行

1. **窗口发现采用快照模型，不做每帧枚举**：会话开始时 `EnumWindows` 建立 `WindowSnapshot`（含窗口身份、DWM 外框、Z 序）；`WM_MOUSEMOVE` 只对快照做缓存命中/最近距离查询。只有在会话新建、显示器变化、目标验证失败、排除集合变化、显式失效等时才刷新快照。**禁止在每次鼠标移动中执行 `EnumWindows + DwmGetWindowAttribute`。**
1b. **hover 时效性与自动吸附有明确兜底**：定期（非鼠标移动路径）只对当前 hover HWND 做轻量重验证，边界变化或验证失败才刷新快照并重新命中；光标停稳超过防抖时间后，在吸附半径内选择最近窗口并更新吸附预览（见 §4.1、§5.5）。重验证的 Win32/DWM 调用运行在**独立窗口检测 worker 线程**，overlay 定时器只投递、结果回投后只校验与更新，同步 DWM 调用不得阻塞 overlay 消息循环。
2. **停稳吸附与拖拽是两阶段状态机**：鼠标移动停止后才允许产生 `AutoSnapPreview`；按下只记录指针手势，位移超过阈值才转 `ManualDrag`。自动吸附不依赖点击释放，`Enter` 或工具栏确认当前预览。这与现有 `session.pointer_pressed()` 按下即改选区的行为冲突，必须先重构输入状态再实现吸附。
3. **窗口目标必须有身份和版本**：`DetectedWindow` 只有 hwnd+bounds 不够安全（HWND 可能被关闭后重用）。快照条目携带 `WindowIdentity`（hwnd + process_id + class_name_hash）与 `snapshot_epoch`，应用自动吸附预览或确认前重新验证，验证失败刷新快照后重命中一次；重命中仍失败则**保持原选区不变、清除自动吸附预览**。
4. **过滤规则修正**：不武断排除 `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`；真正的点击穿透是 `WS_EX_LAYERED && WS_EX_TRANSPARENT`（单独 `WS_EX_TRANSPARENT` 不等于穿透）。SnapClip 自身窗口通过显式 `excluded_hwnds / excluded_process_ids` 集合排除。
5. **v1 只做 `TopLevelWindowFrame`（顶层窗口外框）**：`DWMWA_EXTENDED_FRAME_BOUNDS` 是包含标题栏的窗口外框，**不是**客户区、不是网页内容区、不是"精确内容边界"。`ClientArea` 与 `UiElement`（UIA/MSAA 子元素深选）留给 v2，且 v2 与 v1 整窗吸附严格隔离。
6. **v1 不必引入 R-tree**：按 Z 序排序的 `Vec<WindowCandidate>` + 线性扫描足够；是否升级空间索引由基准测试数据决定（参考 snow_shot：≤16 窗口线性扫描，更多才上 R-tree）。
7. **`WDA_EXCLUDEFROMCAPTURE` 只负责捕获内容排除**，不负责 `EnumWindows` 过滤，不替代 excluded HWND/PID 集合。自家排除采用三层保护。
8. **hover 重验证跨线程执行**：`DwmGetWindowAttribute` 是同步调用，与 overlay 同线程必然暂时阻塞消息循环。因此定时器只向检测 worker 投递当前 hover 的 HWND/identity/epoch，worker 回投 `Valid / BoundsChanged / Invalid`，overlay 线程校验 epoch+HWND 后更新状态并重绘（§5.5）。
9. **`BoundsChanged` 必须写回快照**：只改 hover 高亮不改 `WindowSnapshot` 的话，下一次 `hit_test` 仍返回旧矩形，hover 会跳回旧位置。通过 `apply_candidate_update`（按 epoch+identity 校验）同步更新快照内该候选的矩形（§5.5）。

---

## 2. 参考实现调研结论

调研对象：`refer/shot-refer/{Crisp, ShareX, PowerToys}` 与 `refer/snow-apps/snow_shot`。

| 项目 | 检测机制 | 返回粒度 | 复杂度 | 对 SnapClip 的适用性 |
| --- | --- | --- | --- | --- |
| Crisp（C++） | `EnumWindows` Z 序 + `PtInRect`，DWM 扩展边界 | 单个整窗矩形 | 低 | 整窗命中与过滤条件可参考 |
| ShareX（C#） | `EnumWindows` + `GetWindowRectangle`，可选 `EnumChildWindows` 深选 | 整窗 + 子控件矩形列表 | 中 | 过滤条件与子窗口去重值得参考 |
| PowerToys（C++） | `DWMWA_EXTENDED_FRAME_BOUNDS` 为主，多处窗口工具复用 | 单个整窗矩形 | 低 | 验证扩展边界 API 的正确性 |
| snow_shot（Qt/Rust+C++） | 窗口快照 + 空间索引命中；UIA 仅用于深选 | 整窗矩形 / 嵌套命中路径 | 中（v1 部分）/ 高（v2 部分） | **v1/v2 双蓝本**，其真实分层见下 |

### 2.1 snow_shot 的真实实现（修订版结论，此前文档描述有误）

**窗口发现不是每帧枚举**（`snow-ui-selector/src/windows/window.rs`、`spatial.rs`）：

- `EnumWindows` 枚举顶层窗口，只做**廉价过滤**：不可见、最小化、点击穿透；
- 使用 `DWMWA_EXTENDED_FRAME_BOUNDS` 取边界；
- 结果保存为**窗口快照**：`HWND + 窗口边界 + Z 序 + 缓存索引`；
- 后续命中测试**只查快照，不重新枚举窗口**；
- 命中测试按窗口数量选择策略：≤16 个窗口线性扫描，更多用 R-tree 空间索引；多候选命中时选 Z 序最靠前者。

**UIA 只用于深选，不参与普通整窗吸附**（`snow-ui-selector/src/windows/uia.rs`、`snow-ui-selector-c/src/lib.rs`）：

- 整窗模式直接返回窗口边界，**不访问 UIA 树**；
- 子元素模式才走 UIA：独立 COM apartment，缓存请求读取 bounding rect / offscreen / control type，查询带超时与取消；
- 双 worker 调度：前台 worker 负责刷新窗口快照和普通命中；refinement worker 负责 UIA 深度查询；
- 队列只保留最新鼠标点；旧查询用 `epoch / request_id / generation` 丢弃；
- refinement 在鼠标静止约 **80ms** 后才启动。

**停稳吸附与拖拽是明确的两阶段状态机**（拖拽阈值参考 `screenshotoverlayinputhandler.cpp`、`screenshotintelligentselectionmodel.cpp`；吸附触发按 SnapClip 需求调整）：

```
鼠标停稳 -> AutoSnapPreview（不立即确认）
按下并移动超过阈值 -> ManualDrag
Enter/工具栏确认 -> 提交当前吸附预览
```

`shouldStartManualDrag(...)` 仍用位移平方和对比系统拖拽阈值。**关键点：停稳才产生吸附预览，按下时不会立即修改正式选区。**

### 2.2 轻量路线共同的"铁律"

- **精确外框 = `DWMWA_EXTENDED_FRAME_BOUNDS`，不是 `GetWindowRect`**：后者在 Win10/11 含约 7–8px 不可见 resize 边框，会大一圈。首选扩展框架边界，失败/为空再退 `GetWindowRect`。
- **命中"用户看到的窗口" = Z 序（最顶先行）扫描，第一个包含光标的即答案**。比 `WindowFromPoint` 更可控（后者会返回子控件、受 layered 窗口干扰）。
- **过滤质量决定吸附体验**，过滤规则见 §5.3。

> 结论：snow_shot 证明"快照命中 + 悬停预览 + 拖拽转手动 + 显式排除集合"这套架构成熟可参考；SnapClip 将吸附触发改为光标停稳后的最近窗口预览，不采用单击触发。其 UIA 命中路径服务对应本需求暂缓的"子控件深选"，v1 不引入，且必须保持 UIA 与 v1 整窗吸附隔离。

---

## 3. SnapClip 现状与集成点

已核对现有代码，关键事实：

- **枚举范式已存在**：`src-tauri/src/platform/windows/capture/monitor.rs` 已用 `EnumDisplayMonitors` + `LPARAM` 收集器回调，窗口快照构建直接照抄该范式。
- **坐标模型**：overlay 与 session 全程工作在**显示器本地物理像素**（`overlay.rs` `Point::new(client.x, client.y)`）；Win32 窗口矩形是**虚拟桌面物理像素**，须经 `MonitorLayout::to_local` / `local_bounds()` 转换。进程已是 Per-Monitor V2，扩展边界与 back buffer 同为物理像素，**无需再缩放**。
- **输入现状与冲突**：`overlay.rs:1158` 处 `session.pointer_pressed(point)` 在按下瞬间即修改 selection（`Create/Outside` 分支即建零尺寸选区）。**这与窗口吸附的两阶段状态机直接冲突，必须先按 §4 重构输入，再实现吸附**。属于设计错误而非局部实现错误，不做分支补丁。
- **依赖缺口**：`src-tauri/Cargo.toml` 的 `windows` 0.61 features **缺 `Win32_Graphics_Dwm`**（`DwmGetWindowAttribute`/`DWMWA_*` 所在地），必须补加。`Win32_UI_WindowsAndMessaging`、`Win32_Graphics_Gdi`、`Win32_Foundation`、`Win32_System_Threading` 已具备。

---

## 4. 交互设计与输入状态机

### 4.1 产品语义（已确认）

- **悬停**：鼠标移动时高亮光标下"用户看到的"窗口的**顶层窗口外框**（强调描边 + 半透明填充），仅预览，不改变已有选择。
- **停稳自动吸附**：鼠标移动停止并持续达到防抖时间（初始 120ms，依据手感测试调整）后，在吸附半径内按点到窗口矩形的最短距离选择最近者，显示吸附预览并将选区预览贴合到该窗口在当前显示器的可见部分；不自动确认。点位于多个重叠窗口内时距离均为零，按 Z 序选择最上层窗口。
- **确认**：按 `Enter` 或工具栏确认命令时，验证当前吸附目标并提交选区；目标失效时刷新快照并重命中一次，仍失败则不改变原选区。
- **拖拽**（按下后位移² ≥ 阈值）：立即取消自动吸附预览，维持现有自由橡皮筋框选。
- **跨屏窗口**：当前截图会话只捕获当前显示器，因此跨屏窗口**只吸附当前显示器可见部分**，不承诺整窗。若未来产品要求"整窗吸附"，须改为虚拟桌面捕获，而不是继续使用单显示器 frozen frame（见 §6.2）。
- **深选子控件**：本期不做，接口预留（`WindowTargetProvider::hit_test` 未来可扩展为返回命中路径的 v2 provider）。

### 4.2 PointerGesture 状态机

引入显式手势状态，替换"按下即改选区"的隐式状态：

```rust
enum PointerGesture {
    None,
    AutoSnapPreview {
        target: WindowTarget,
        preview_selection: Rect,
        selection_before_preview: Rect,
    },
    PendingPointer {
        press_point: Point,
        selection_before_press: Rect,
    },
    ManualDrag {
        press_point: Point,
        mode: ResizeMode,
    },
    MoveSelection,
    ResizeSelection,
}
```

处理顺序：

```text
PointerDown:
  1. 命中已有选区手柄   -> ResizeSelection
  2. 命中已有选区内部   -> MoveSelection
  3. 否则              -> 记录 PendingPointer（不改变选区）

PointerMove:
  1. 无按键 + 光标移动       -> 更新 hover，重置 dwell timer
  2. 无按键 + 停稳达到 dwell -> 在 snap radius 内计算最近窗口并进入 AutoSnapPreview
  3. PendingPointer + 超阈值 -> 取消 AutoSnapPreview，转 ManualDrag
  4. ManualDrag              -> 更新自由框选

PointerUp:
  1. PendingPointer -> 结束指针等待，不提交窗口吸附；吸附只能由 Enter/工具栏确认
  2. ManualDrag   -> 提交自由选区
  3. Move/Resize  -> 提交选区编辑

Confirm (`Enter` / toolbar):
  1. AutoSnapPreview -> 验证目标并提交预览选区
  2. 目标失效        -> 刷新快照并重命中一次；仍失败则恢复确认前选区

AutoSnapPreview:
  1. 预览几何只用于绘制，不覆盖已确认 selection
  2. 光标移动到另一候选并重新停稳 -> 替换为新的最近窗口预览
  3. 离开所有候选的 snap radius -> 清除预览，恢复 hover/已有选区
```

拖拽阈值沿用 snow_shot 的判别：`should_start_manual_drag(press_pt, cur_pt, d) = dx² + dy² ≥ d²`，`d` 取系统拖拽阈值（`SystemParametersInfo(SM_CXDRAG)` 折算像素），避免抖动误判。

**必须如此设计的根因**：若在按下时先创建零尺寸选区，会引入点击时先产生零尺寸选区、窗口吸附状态与手动拖拽状态互相混淆等一类缺陷，这类缺陷无法在现有结构上局部修复。

---

## 5. 窗口快照与命中模型

### 5.1 数据结构（含身份与版本）

```rust
struct WindowIdentity {
    hwnd: isize,
    process_id: u32,
    class_name_hash: u64,      // GetClassNameW 的哈希，用于 HWND 重用检测
}

struct WindowCandidate {
    identity: WindowIdentity,
    screen_bounds: Rect,        // 虚拟桌面物理像素（DWM 扩展框架边界）
    client_bounds: Option<Rect>,// v1 可缺省；v2/客户区语义时启用
    z_order: u32,               // EnumWindows 访问序，0 = 最顶
    snapshot_epoch: u64,        // 所属快照版本
}

struct WindowTarget {
    candidate: WindowCandidate,   // 含 screen_bounds（虚拟桌面物理坐标）
    target_kind: TargetKind,      // v1 只有 TopLevelWindowFrame
}
// 注意：WindowTarget 不携带显示器本地坐标。screen → local 的裁剪转换
// 由 overlay 侧调用 geometry.rs::window_rect_to_local 完成（见 §5.2、§6.2）。

enum TargetKind {
    TopLevelWindowFrame,  // v1：DWM 顶层窗口外框（含标题栏，不含不可见 resize border）
    // ClientArea,       // v2+：客户区
    // UiElement,        // v2+：UIA/MSAA 子元素
}

struct WindowSnapshot {
    epoch: u64,
    candidates: Vec<WindowCandidate>, // 按 z_order 升序（最顶在前）
}
```

### 5.2 核心接口与模块划分

```text
capture/window_detection/
  model.rs       // WindowIdentity, WindowCandidate, WindowTarget, WindowSnapshot, TargetKind
  provider.rs    // WindowTargetProvider trait
  snapshot.rs    // EnumWindows + DWM 边界 + 过滤，构建快照
  hit_test.rs    // 快照命中、Z 序裁决、半开区间
  gesture.rs     // PointerGesture: AutoSnapPreview / PendingPointer / ManualDrag / Move / Resize

platform/windows/capture/win/window.rs
                 // 只负责 Win32 枚举、DWM 边界、窗口属性读取（纯 FFI 封装）

capture/geometry.rs
                 // screen/local 坐标、裁剪、跨显示器转换（平台无关纯函数）

platform/windows/capture/overlay.rs
                 // 输入消息、PointerGesture 状态机、GPU 重绘调度

capture/session.rs
                 // 最终选区和截图会话状态（snap_to 入口）
```

```rust
/// Provider 只在虚拟桌面屏幕坐标系工作，不感知显示器布局。
trait WindowTargetProvider {
    fn refresh(&mut self, exclusions: &Exclusions) -> Result<WindowSnapshot>;
    fn hit_test(&self, point: ScreenPoint) -> Option<WindowTarget>;  // 点在窗口内时按 Z 序命中
    fn nearest_target(&self, point: ScreenPoint, snap_radius: u32) -> Option<WindowTarget>;
    // 纯缓存命中：按点到矩形的距离选择最近候选；距离相同按 Z 序裁决
    fn validate(&self, target: &WindowTarget) -> bool;               // 自动预览/Enter 确认前的单次校验
    fn revalidate_hover(&mut self, hover: &WindowTarget) -> HoverValidity;
    // ^ hover 重验证路径：包含 Win32/DWM 调用，只允许在检测 worker 线程执行（§5.5）

    fn apply_candidate_update(&mut self, result: &HoverValidity); // 纯状态更新，无 Win32 调用；
    // epoch 与候选 identity 匹配才把新矩形写回当前快照，否则 no-op（陈旧结果由 §5.5 回投校验拦截）
}

enum HoverValidity {
    Valid,
    BoundsChanged { epoch: u64, identity: WindowIdentity, new_bounds: Rect },
    Invalid,
}
```

线程模型（对齐 snow_shot 的"前台 worker 刷新快照"经验，v1 缩小为单检测 worker）：

```text
overlay 线程   hit_test / apply_candidate_update / snap_to 前的 validate：纯缓存或单窗口廉价检查
检测 worker    refresh / revalidate_hover：包含 EnumWindows/DWM 调用，不碰 overlay 消息循环
结果传递       worker → overlay：PostMessage/通道回投，overlay 校验 epoch+HWND 后应用
```

坐标分工：`hit_test`/`WindowTarget` 全程**虚拟桌面物理坐标**；`WindowTarget → local_visible_bounds` 的转换发生在外层，由 `geometry.rs` 纯函数完成，Provider 不接收也不持有 `MonitorLayout`。

v1 只实现 `TopLevelWindowProvider`。v2 再实现 `UiAutomationTargetProvider` / `MsaaTargetProvider`，且 **v2 provider 不得进入 v1 整窗吸附路径**（hover/自动吸附命中整窗时绝不触碰 UIA）。

### 5.3 快照构建与过滤规则

快照构建采用**两阶段过滤**（对齐 snow_shot 实际实现，避免把 DWM 调用放进枚举回调）：

**阶段一：`EnumWindows` 回调内的廉价检查**（仅用户态、无跨进程/合成器开销的 API）：

| 排除项 | API |
| --- | --- |
| 空句柄 / 非窗口 | `IsWindow` |
| 不可见 | `IsWindowVisible` |
| 最小化 | `IsIconic` |
| 真正的点击穿透 | `GetWindowLongPtrW(GWL_EXSTYLE)`：`WS_EX_LAYERED && WS_EX_TRANSPARENT`（**单独 `WS_EX_TRANSPARENT` 不等于鼠标穿透，不得单独排除**） |
| SnapClip 自家窗口 | `excluded_hwnds` / `excluded_process_ids` 显式集合（见 §7） |
| Shell 表面（桌面/任务栏/Island） | 类名黑名单：`Progman`、`WorkerW`、`Shell_TrayWnd`、`Shell_SecondaryTrayWnd`、`Windows.UI.Core.CoreWindow`、`XamlExplorerHostIslandWindow`（精确黑名单，不按样式位一刀切） |

**阶段二：通过阶段一的候选再调用 DWM 检查**（回调内不执行，收集 hwnd 列表后统一处理）：

| 排除项 | API |
| --- | --- |
| Cloaked（UWP/虚拟桌面/Snap Assist 隐藏） | `DwmGetWindowAttribute(DWMWA_CLOAKED)` |
| 窗口边界获取 | `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`，失败/空退 `GetWindowRect` |
| 空矩形 | 上一步结果为空 |

**明确不再排除**（相对初版文档的修正）：`WS_EX_TOOLWINDOW`、`WS_EX_NOACTIVATE` 不再作为过滤条件。这两个样式位不能推断"不可截图"——浏览器弹出面板、开发工具窗口、应用浮动工具栏等都是带其一甚至两者、但真实可见且应可截图的窗口。对齐 snow_shot 的实际过滤集：不可见 / 最小化 / cloaked / `WS_EX_LAYERED + WS_EX_TRANSPARENT` / 显式排除集合。

快照刷新时机（**只有以下情况**）：

```text
- 新截图会话建立
- 显示器布局变化（WM_DISPLAYCHANGE / MonitorLayout 变更）
- 目标窗口验证失败（§5.4）
- hover 轻量重验证发现目标边界变化或失效（§5.5）
- 工具栏 / overlay 等 HWND 集合发生变化（excluded 集合变更即快照失效）
- 显式 invalidate
```

### 5.4 命中与失效处理

`hit_test.rs`：

- 对 `candidates` 按 Z 序线性扫描，点在 `screen_bounds` 内即命中（v1 线性扫描；候选数显著增大且基准测试证明有热点时才升级空间索引）；
- 边界采用**半开区间**（`left ≤ x < right`，`top ≤ y < bottom`），避免相邻窗口在共享边界处二义；
- 多候选命中时取 `z_order` 最小者。

**过期/关闭/移动/HWND 重用**：自动吸附预览产生或确认提交前必须重新验证目标：

```text
IsWindow(hwnd)
IsWindowVisible(hwnd)
!IsIconic(hwnd)
!cloaked
process_id / class_name_hash 仍匹配   // 防 HWND 重用后指向另一个窗口
当前窗口边界仍有效（重读 frame_bounds 与快照比对）
```

验证失败时：**刷新快照并重新命中一次**，不能直接使用旧矩形吸附。重命中仍无目标时，清除自动吸附预览并恢复确认前选区，等待下一次停稳或按键操作：

```text
自动吸附目标验证失败
  -> 刷新快照并重命中一次
  -> 仍失败：恢复确认前选区，清除 AutoSnapPreview，恢复 hover 预览
  -> 下一次光标停稳才重新产生吸附预览
```

### 5.5 hover 与自动吸附预览时效性（窗口移动的去过期）

`WM_MOUSEMOVE` 只查快照的直接后果是：截图过程中窗口被移动/缩放时，`hover_target` 和 `AutoSnapPreview` 可能显示旧矩形。去过期策略如下，两条红线：**不得放入每次鼠标移动的同步路径；不得在 overlay 消息循环线程执行 DWM 同步调用**（`DwmGetWindowAttribute` 是同步调用，若与 overlay 同线程，回调本身就会暂时阻塞消息循环，"不阻塞"的承诺无法成立）：

**线程模型**：

```text
定时器（初始 250ms，随 §10.2 指标调参，运行在 overlay 线程）
  └─ 只投递 {hwnd, identity, epoch} 到窗口检测 worker（队列合并，只保留最新 hover）
       └─ worker 执行 revalidate_hover：单窗口重读 frame_bounds + cloaked/可见性
            └─ PostMessage/通道 将 HoverValidity 回投 overlay 线程
                 └─ overlay 线程校验回投的 epoch+HWND 与当前 hover 一致后应用；不一致直接丢弃（陈旧结果）
```

**结果处理**：

- `Valid`：无事发生；
  - `BoundsChanged`：**必须同时更新 `WindowSnapshot` 与 `hover_target`**：overlay 线程先校验 epoch+HWND，再调 `apply_candidate_update` 把新矩形写回当前快照中该 candidate，然后更新 hover/自动吸附预览并 `invalidate()` 重绘。只改 hover 不改快照是缺陷：下一次鼠标移动 `hit_test`/`nearest_target` 仍返回旧矩形，hover 会跳回旧位置；
- `Invalid`（关闭/cloaked/身份失配）：刷新快照 → 以当前鼠标位置重命中一次 → 更新或清除 hover；
- epoch 不匹配（期间发生过 refresh/invalidate）：结果作废，等下一轮定时器；无 hover 目标时定时器跳过投递。

**确认路径的例外**：`on_confirm` 的 `validate` 仍保留在 overlay 线程同步执行——它是一次 Enter/工具栏确认触发的单窗口检查（若干廉价 API + 一次 DWM 读），不是周期性路径；其耗时由 `window_validate_us` 埋点监控，若实测超出 §10.2 预算再迁入 worker，并在结果回投后提交吸附结果。

---

## 6. 几何与坐标

### 6.1 screen/local 转换

```rust
/// 把虚拟桌面物理窗口矩形裁剪到某显示器并转本地坐标；跨屏窗口按当前显示器可见部分裁剪。
pub fn window_rect_to_local(win: Rect, monitor: &MonitorLayout) -> Rect {
    let c = win.intersect(monitor.bounds);
    Rect::new(c.left - monitor.bounds.left, c.top - monitor.bounds.top,
              c.right - monitor.bounds.left, c.bottom - monitor.bounds.top)
}
```

纯函数，无 Win32 依赖，直接单元测试。须覆盖负虚拟桌面坐标（主显示器左侧存在副屏时 `monitor.bounds.left < 0`）。

### 6.2 跨显示器语义

坐标分层：

```text
Provider / WindowTarget      → 只有 screen_bounds（虚拟桌面整窗外框，原始 DWM 矩形）
geometry.rs 转换后 overlay 持有 → local_visible_bounds（当前显示器可见部分，本地坐标）
target_kind                  → TopLevelWindowFrame
```

`hit_test` 不接收显示器布局、不计算本地坐标（§5.2）；`window_rect_to_local(target.candidate.screen_bounds, monitor)` 在 overlay 侧调用，overlay 是唯一知道当前捕获显示器的一方。避免把"整窗矩形"和"当前显示器可见部分"混为一谈。当前会话基于单显示器 frozen frame，吸附结果 = `local_visible_bounds`；产品语义在 UI/文档中统一表述为"当前显示器可见部分"，不宣称"整窗"。

---

## 7. SnapClip 自身排除（三层保护）

snow_shot 的做法（`windowchrome.cpp`）经核对：`RtlGetVersion` 检查 Win10 2004+ → `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` → 失败保留降级路径 → **另外仍维护显式排除窗口集合**。据此修正初版文档"overlay 设置 affinity 后天然避免窗口命中"的错误表述：

> `WDA_EXCLUDEFROMCAPTURE` 只负责**捕获内容排除**，不负责 `EnumWindows` 过滤，也不替代 excluded HWND/PID。

SnapClip 采用三层保护：

1. **捕获内容层**：overlay/工具栏/颜色面板等自家 HWND 创建后设置一次 `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`（`RtlGetVersion` 探测 Win10 2004+，失败降级）；
2. **命中过滤层**：快照过滤与吸附验证使用显式集合：
   ```rust
   struct Exclusions {
       excluded_hwnds: HashSet<isize>,       // 截图 overlay、工具栏、颜色面板、Tauri 主窗口、后续录屏控制窗口
       excluded_process_ids: HashSet<u32>,   // 自家进程兜底（新窗口创建竞态）
   }
   ```
   excluded 集合变化 → 快照失效 → 下次命中前刷新；
3. **捕获降级层**：affinity 不可用（旧系统/失败）时，保留"捕获前隐藏自家窗口或移往屏外"的降级路径。

`DwmFlush()` 只用于**捕获前的合成同步**（确保 affinity/隐藏变更已生效），不用于每帧 hover。

---

## 8. 交互接入（改动后）

前提：§4 的 `PointerGesture` 状态机重构已落地。

- `overlay.rs on_mouse_move`（`PointerGesture::None` 或 `PendingPointer`）：`screen_pt = monitor.to_screen(cursor)` → `provider.hit_test(screen_pt)`（**纯缓存命中，不调用 EnumWindows/DWM**）→ `window_rect_to_local(target.candidate.screen_bounds, monitor)` 得 `local_visible_bounds` → 存 `self.hover_target`（同 hwnd 且同矩形则跳过重算与重绘）→ 重置停稳计时器。光标停稳达到防抖时间后，在吸附半径内选择最近候选并进入 `AutoSnapPreview`，预览选区只更新画面、不提交会话状态。本地转换只发生在这里，Provider 不感知显示器。
- `overlay.rs on_left_down`：按 §4.2 判定 Resize/Move/PendingPointer；命中 `AutoSnapPreview` 时记录确认前选区，**不直接调用 `session.pointer_pressed()` 改选区**。
- `overlay.rs on_mouse_move`（`PendingPointer`/`AutoSnapPreview`/`ManualDrag`）：按下后位移² ≥ 阈值时清除 `AutoSnapPreview`，转 `ManualDrag`，再调用现有 `session.pointer_moved` 更新橡皮筋。
- `overlay.rs on_left_up`：`PendingPointer` 只结束指针等待，不提交吸附；`AutoSnapPreview` 不因鼠标释放而确认；`ManualDrag` → 现有 `pointer_released`。
- `overlay.rs on_confirm`（Enter/工具栏）：`AutoSnapPreview` → `provider.validate(target)`，失败刷新快照重命中一次 → 成功则 `session.snap_to(local_visible_bounds)`；仍失败则恢复确认前选区并清除预览（见 §5.4）。
- `session.rs snap_to(rect)`：非空且 ≥ 最小尺寸则设 selection + 置 `Selected`，否则忽略。
- `d2d.rs`：在现有 crosshair 之后、info panel 之前画 `hover_target` 高亮（复用 band/label brush，无需新配色）。
- 鼠标事件合并：处理循环只消费**最新点**，堆积的中间点丢弃（`mouse_move_coalesced_count` 指标可观测）。
- 定时器（~250ms，overlay 线程）：只投递当前 hover 的 HWND/identity/epoch 给窗口检测 worker；worker 回投 `Valid/BoundsChanged/Invalid`，overlay 线程校验 epoch+HWND 后：`BoundsChanged` → `apply_candidate_update` 更新快照 + hover 高亮并重绘；`Invalid` → 刷新快照重命中（§5.5）。验证不落在鼠标移动路径，也不在 overlay 消息循环线程执行 DWM 调用。

---

## 9. 边界与坑

| 场景 | 处理 |
| --- | --- |
| DPI | 进程 Per-Monitor V2，扩展边界即物理像素，只减显示器原点，不缩放 |
| 最大化窗口 | Win10/11 的 `DWMWA_EXTENDED_FRAME_BOUNDS` 已正确，无需 `MaximizedWindowFix`（仅 <Win10 才需，本项目不支持） |
| 跨屏窗口 | 只吸附当前显示器可见部分（§6.2）；`intersect(monitor.bounds)` 裁剪 |
| Cloaked/UWP | `DWMWA_CLOAKED` 过滤 |
| 单独 `WS_EX_TRANSPARENT` | **不过滤**（可能是真实可见可截图窗口）；只滤 `WS_EX_LAYERED && WS_EX_TRANSPARENT` |
| `WS_EX_TOOLWINDOW` / `WS_EX_NOACTIVATE` | **不过滤**（修正初版武断规则） |
| HWND 重用 | 吸附前验证 PID/class hash/边界（§5.4） |
| 快照过期 | 目标验证失败 → 刷新快照重命中一次；excluded 集合/显示器变化 → 快照失效 |
| 窗口移动致 hover 过期 | 检测 worker 轻量重验证当前 hover HWND，`BoundsChanged` 经 `apply_candidate_update` 同步更新快照与 hover，`Invalid` 则刷新快照重命中（§5.5） |
| DWM 同步调用阻塞消息循环 | hover 重验证在检测 worker 线程执行；overlay 线程只接收校验后的结果；点击路径的单次 `validate` 由 `window_validate_us` 监控（§5.5） |
| BoundsChanged 后 hover 跳回旧位置 | 新矩形必须写回快照，`hit_test` 与 hover 共用同一数据源；回投结果按 epoch+HWND 校验，陈旧丢弃（§5.5） |
| 吸附最终失败 | PointerUp 后不进入 ManualDrag：保持原选区、清除 PendingClick，等待下一次按下（§5.4） |
| overlay 命中自己 | 三层保护：affinity + excluded 集合 + 捕获降级（§7） |
| 空/退化矩形 | `is_empty` + 最小尺寸过滤 |
| 负虚拟桌面坐标 | `window_rect_to_local` 纯函数单测覆盖 |

---

## 10. 性能设计与指标

### 10.1 必须遵守的规则

- 新会话只刷新一次窗口快照；
- `WM_MOUSEMOVE` 只做快照缓存命中，**不同步执行 `EnumWindows`，不在鼠标移动时调用 DWM 查询，不调用 UIA/MSAA**；
- hover 重验证的 `EnumWindows`/DWM 调用只在检测 worker 线程执行，overlay 消息循环线程不做周期性同步 DWM 调用；
- 鼠标事件合并，只处理最新点；
- 相同 HWND、相同矩形时不重绘；
- `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` 在 overlay HWND 创建后设置一次；
- `DwmFlush()` 仅用于捕获前合成同步。

### 10.2 指标记录

埋点：

```text
window_snapshot_refresh_us
window_hit_test_us
window_validate_us
candidate_count
hover_target_switch_count
stale_target_count
hover_revalidate_stale_dropped_count
mouse_move_coalesced_count
```

初始目标（**基线优先**：在 50 / 100 / 200 窗口的真实桌面测量后再定硬阈值）：

```text
缓存命中 P95 < 0.1 ms
快照刷新 P95 < 10 ms
鼠标移动处理不阻塞 overlay 消息循环
点击路径单次 validate P95 < 1 ms（超出则按 §5.5 迁入 worker 异步提交）
hover 重验证不占用 overlay 消息循环线程时间片（worker 执行，§5.5）
```

R-tree 等空间索引不在 v1 预置；仅当基准测试显示线性扫描成为热点时才引入（snow_shot 的经验分界：>16 候选）。

---

## 11. 分期路线图

- **v1（本方案）**：窗口快照 + 缓存命中 + `PointerGesture` 状态机重构 + 悬停高亮 + 单击吸附 `TopLevelWindowFrame` + 拖拽转手动 + 三层自身排除 + 目标验证与快照失效。依赖仅新增 `Win32_Graphics_Dwm`。
- **v2（预留）**：子控件深选。引入 UIA/MSAA 命中路径服务，采用 snow_shot 的异步 coordinator 蓝本（前台 worker 刷新快照与普通命中；refinement worker 独立 COM apartment 做 UIA；单飞 + 最新点合并 + `epoch/generation/request_id` 去陈旧 + 鼠标静止 80ms 防抖 + 超时/取消 + `stopReason` 降级）。`WindowTargetProvider` 扩展 `UiAutomationTargetProvider` / `MsaaTargetProvider`，新增 `ClientArea` / `UiElement` 目标类型。**v2 不改动 v1 整窗吸附路径，两者严格隔离。**

---

## 12. 验证计划

### 12.1 单元测试

- `geometry.rs`：`window_rect_to_local` 原点偏移、显示器内、跨屏裁剪、退化矩形、**负虚拟桌面坐标**；
- `hit_test.rs`：重叠窗口按 Z 序命中；半开矩形边界（点在右/下边缘外 1px 不命中、左/上边缘命中）；
- `snapshot.rs` 过滤：`WS_EX_LAYERED + WS_EX_TRANSPARENT` 被排除；**单独 `WS_EX_TRANSPARENT` 不被排除**；cloaked / iconic / 不可见被排除；excluded hwnd/pid 被排除；
- 目标失效：HWND 关闭后 `validate` 返回 false；PID/class 变化视为不同窗口，不吸附旧矩形；
- `gesture.rs`：PendingClick 不改变选区；位移超阈值转 ManualDrag；PendingClick 抬起提交吸附；PendingClick 吸附最终失败时选区不变且手势归 None（不产生 ManualDrag）；Move/Resize 优先级高于 PendingClick；
- hover 去过期（§5.5）：模拟目标边界变化 → `BoundsChanged` 经 `apply_candidate_update` 只更新该 candidate、不触发全量刷新，且后续 `hit_test` 返回新矩形（回归"hover 跳回旧位置"缺陷）；epoch/identity 不匹配的更新为 no-op；模拟窗口关闭 → `Invalid` → 刷新重命中；无 hover 时定时器跳过投递；worker 回投结果 epoch/HWND 与当前 hover 失配时被丢弃（`hover_revalidate_stale_dropped_count` +1）。

### 12.2 Windows 集成测试（真机探针）

覆盖窗口类型：普通窗口、最大化窗口、无边框窗口、浏览器、资源管理器、layered overlay、点击穿透 overlay、多显示器混合 DPI；行为场景：

- 窗口移动后再次吸附（命中新位置，验证 §5.5 重验证与快照刷新路径，hover 矩形跟随不长期过期，且不跳回旧位置）；
- 拖动窗口过程中 overlay 消息循环无卡顿（hover 重验证在 worker 线程，60fps 重绘无可见阻塞）；
- 窗口关闭后再次吸附（validate 失败 → 刷新 → 不误吸旧矩形）；
- overlay 不进入截图结果（验证三层排除）；
- 与系统 `Win+Shift+S` 窗口吸附边界逐像素比对。

### 12.3 交互与既有功能回归

- 悬停高亮、单击吸附、拖拽自由框选、resize、move 四类互不破坏；特别验证：按下瞬间不再产生零尺寸选区；
- 手动框选、放大镜、取色、标注、导出闭环不受影响；
- 性能指标不劣于基线（§10.2）。

### 12.4 构建/静态检查

`cargo check` 零告警；`cargo test` 全绿；关键路径有 §10.2 埋点数据输出可供复核。

---

## 13. 影响文件清单

| 文件 | 变更 |
| --- | --- |
| `src-tauri/Cargo.toml` | `windows` features 增加 `Win32_Graphics_Dwm` |
| `src-tauri/src/capture/window_detection/model.rs` | 新增：`WindowIdentity` / `WindowCandidate` / `WindowTarget` / `WindowSnapshot` / `TargetKind` |
| `src-tauri/src/capture/window_detection/provider.rs` | 新增：`WindowTargetProvider` trait（含 `revalidate_hover`/`apply_candidate_update`）与 `Exclusions`、`HoverValidity`；接口只工作在虚拟桌面屏幕坐标系 |
| `src-tauri/src/capture/window_detection/snapshot.rs` | 新增：快照构建（EnumWindows + DWM + 过滤），刷新时机与 epoch 管理 |
| `src-tauri/src/capture/window_detection/hit_test.rs` | 新增：Z 序线性扫描、半开区间命中、多候选裁决 |
| `src-tauri/src/capture/window_detection/gesture.rs` | 新增：`PointerGesture` 状态机 |
| `src-tauri/src/platform/windows/capture/win/window.rs` | 新增：Win32 枚举 / DWM 边界 / 窗口属性读取（纯 FFI 封装） |
| `src-tauri/src/capture/geometry.rs` | 新增 `window_rect_to_local` 纯函数 + 单测 |
| `src-tauri/src/platform/windows/capture/overlay.rs` | 输入改为 `PointerGesture` 状态机（含移除按下即改选区的路径）、`hover_target`、事件合并、重绘抑制、三层自身排除、hover 重验证投递与结果回投处理（worker 线程 + epoch/HWND 校验） |
| `src-tauri/src/capture/session.rs` | 新增 `snap_to(rect)`；`pointer_pressed` 语义并入手势状态机 |
| `src-tauri/src/platform/windows/capture/win/d2d.rs` | 绘制 hover 高亮描边 + 半透明填充 |
| 性能埋点 | §10.2 指标接入现有日志/统计通道 |
