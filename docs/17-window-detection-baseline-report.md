# 窗口自动吸附 Phase 0：基线确认、契约冻结与诊断埋点

本文件是 `docs/14-screenshot-window-detection-design.md` 落地的 Phase 0 交付物：
修改前的真实行为基线、参考源码审计结论、契约冻结内容与质量门禁证据。
后续阶段（Phase 1~7）的基线与对比均以本文件为准。

- 基线提交：`581268c`（`main`，工作区仅有未跟踪的 `docs/16-*.md`）
- 测量环境：Windows 11 IoT Enterprise LTSC build 26100 / AMD64 / MS-Terminator Z790-A
- 显示环境：单显示器 3840×2160 @ (0,0)，工作区 3840×2088，DPI 144（Per-Monitor V2 实测值）
- 测量构建：`cargo build` debug（`src-tauri/target/debug/snapclip.exe`）

---

## 1. 现状盘点（全部来自代码与实测，非推测）

### 1.1 捕获链路与线程 / HWND 所有权

| 对象 | 创建线程 | 使用线程 | 释放时机 |
| --- | --- | --- | --- |
| overlay HWND | 专用 overlay 线程（`overlay_thread`） | overlay 线程（消息泵 + `OverlayController`） | 消息循环退出后 `DestroyWindow` + `UnregisterClassW` |
| overlay 消息泵 | overlay 线程 `GetMessageW` | 同线程 | `PostQuitMessage` |
| D3D11 device | capture worker 线程 | worker（捕获/回读）与 overlay（renderer 共享同一 device） | worker `shutdown()` / `invalidate_providers()` |
| frozen texture | capture worker | overlay renderer（只读） | `release_session()` 丢弃 renderer/frozen |
| D2D / DirectComposition 资源 | overlay 线程（`Win32Renderer::new`） | overlay 线程 | `release_session()` 置 `renderer = None` |
| capture worker | overlay 线程创建、自带线程 | worker 线程 | `OverlayCommand::Shutdown` |
| export worker | 同上 | export 线程 | 同上 |
| 颜色采样 staging（3 slot） | overlay 线程（GPU 缓冲） | overlay 线程 | `release_session() → sampler.reset()` |

- 线程消息（`FRAME_READY_MESSAGE` / `EXPORT_READY_MESSAGE` / `WM_OVERLAY_COMMAND`）由
  `overlay_thread` 在 `message.hwnd.is_null()` 分支手工分派——这是 docs/13 记录过的
  根因修复，窗口消息才走 `DispatchMessageW`。
- HWND 样式实测：`WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP`，
  窗口样式 `WS_POPUP`；**不加 `WS_EX_NOACTIVATE`**，因为 overlay 必须能激活才能收到
  `WM_KEYDOWN`。已有单测 `overlay_style_is_activatable_so_escape_reaches_it` 固定该事实。

### 1.2 坐标系与 DPI

- overlay 与 session 全程工作在**显示器本地物理像素**：`show_overlay` 把窗口放到
  `layout.bounds`，鼠标 `lParam` 客户端坐标即本地物理像素。
- Win32 窗口矩形是**虚拟桌面物理像素**；转换入口是 `MonitorLayout::to_local` /
  `local_bounds()`（`capture/geometry.rs`）。本阶段冻结的候选矩形一律保留屏幕坐标。
- `monitor::set_per_monitor_v2_awareness()` 在 overlay 线程**创建窗口之前**调用，
  降级链为 V2 → 检查现有上下文是否 ≥ Per-Monitor → 否则报错；创建 overlay 之后
  再声明 awareness 会让首帧几何错位。
- `Rect::contains` 已是**半开区间**（`x >= left && x < right`），Phase 0 契约直接复用，
  不新增第二套边界语义。

### 1.3 输入状态机与选区生命周期（根因所在）

```text
overlay.rs:1180 on_left_down
  └─ overlay.rs:1192  session.pointer_pressed(point)        ← 按下即改选区
       └─ session.rs:194 pointer_pressed
            ├─ Create | Outside → selection = Rect::new(x, y, x, y)   ← 零尺寸选区
            └─ 立即 mode/drag = Some(...)  → 进入 Selecting
overlay.rs:1208 on_left_up → session.pointer_released()
```

这就是 docs/14 §4 指出的冲突：**按下瞬间就产生零尺寸选区并进入拖拽状态**。
“停稳才产生吸附预览”无法叠加在它上面——任何按下都会先毁掉预览。属于输入状态机的
设计错误，不做分支补丁，Phase 3 用 `PointerGesture` 重建。

现状没有：窗口枚举、DWM 调用、窗口快照、显示器缓存、检测 worker、exclusions 集合。
`SetWindowDisplayAffinity` / `WDA_EXCLUDEFROMCAPTURE` 在整个 `src-tauri` **不存在**，
当前的“overlay 不进入截图”靠的是**先冻结帧再显示 overlay**（`session.rs` 的
`CapturedFrame` 文档注释），不是 affinity。三层排除目前只有第 0 层。

### 1.4 依赖缺口

`src-tauri/Cargo.toml` 的 `windows` 0.61 features 缺 `Win32_Graphics_Dwm`
（`DwmGetWindowAttribute` / `DWMWA_*` 所在），Phase 1 补加。

---

## 2. 修改前实测基线

探针：`.tmp-p0-probe.ps1`（debug exe + `keybd_event` 注入 F5/Esc + `SetCursorPos`
随机移动，解析应用自身 stderr）；原始日志 `.tmp-p0-probe.log`。

### 2.1 静态门禁（修改前 → 本阶段修改后）

| 命令 | 修改前 | 本阶段修改后 |
| --- | --- | --- |
| `cargo test --lib` | 200 passed / 0 failed | **219 passed / 0 failed**（+19：契约 13、诊断 6） |
| `cargo check --all-targets` | exit 0，0 warnings | exit 0，**0 warnings** |
| `cargo clippy --lib --tests`（新增文件） | — | 新文件 **0 告警**（其余为既有 overlay 指针 cast 等） |
| `npm run typecheck` | exit 0 | exit 0 |
| `npm run build` | exit 0 | exit 0（vite 158 ms） |

### 2.2 F5 → overlay visible（n=5，单屏 4K/DPI144/WGC）

| 阶段 | 冷启动（第 1 次） | warm（第 2~5 次） |
| --- | --- | --- |
| monitor 查询 | 0 ms | 0–2 ms |
| worker 队列 | 135 ms | 0–2 ms |
| provider 捕获（WGC 首帧） | 31 ms | 25–30 ms |
| renderer 准备 | 4 ms | 2–4 ms |
| 显示到 `visible` | 17 ms | 8–12 ms |
| **合计 F5 → visible** | **≈ 188 ms** | **≈ 37–45 ms** |

### 2.3 快速移动鼠标期间的资源占用（5 会话 × 约 900 ms 连续随机移动）

| 指标 | 实测 |
| --- | --- |
| Present（`render session=` 行） | 266 次 / 5 会话 ≈ 53 次/会话（≈ 59 Hz，与 15 ms 合并 tick 一致） |
| 进程 CPU 总计 | 609 ms / 约 10.5 s ≈ 单核 **5.8 %** |
| Private Bytes | **56.0 MB** |
| Working Set | **71.1 MB** |
| 会话释放 | 5 次 `release session=`，与 5 次 F5 一一对应 |
| 错误 | 0（无 `failed` / `error` / panic 行） |

### 2.4 窗口检测指标的本阶段取值

`window_snapshot_refresh_us` / `window_hit_test_us` / `window_nearest_target_us` /
`window_validate_us` / `candidate_count` 在修改前**不存在**（功能未实现），
基线记为“0 候选、不适用”。Phase 2 建立第一份真实测量；50/100/200 候选的
P50/P95 在 Phase 6 用真实桌面数据补齐。此处不编造数字。

---

## 3. 参考项目对照表（全部来自真实源码）

### 3.1 直接吸收为架构原则

| 来源 | 结论 |
| --- | --- |
| `snow-ui-selector/src/windows/window.rs` | 两阶段过滤：`EnumWindows` 回调只做廉价检查（可见 / 非最小化 / `WS_EX_LAYERED+WS_EX_TRANSPARENT`），DWM cloaked 与 frame bounds 在回调返回后批量读取 |
| 同上 | `DWMWA_EXTENDED_FRAME_BOUNDS` 成功且非空才采用，否则退 `GetWindowRect`，退后仍做空矩形检查 |
| 同上 | `is_click_through_layered_window` 只判 `LAYERED && TRANSPARENT`；`WS_EX_TRANSPARENT` 单独**不**排除（注释明确说明它只影响绘制顺序） |
| `snow-ui-selector/src/windows/spatial.rs` | `SMALL_WINDOW_LINEAR_SCAN_THRESHOLD = 16`：小窗口集线性扫描优于空间索引；索引必须有 `release_cache()`；空间命中后仍要 `contains_point`（半开）复核并按 `z_order` 最小值裁决 |
| `snow-ui-selector/src/windows/geometry.rs` | `MonitorCache` 由一次 `EnumDisplayMonitors` 建立，窗口裁剪只做进程内矩形相交，不再对每个候选调 `MonitorFromRect` |
| `snow-ui-selector/src/windows/mod.rs` | DPI awareness 降级链 V2 → Per-Monitor → System，用 `Once` 只执行一次 |
| `snow_shot/.../screenshotintelligentselectionmodel.cpp` | `shouldStartManualDrag` 用位移平方与系统拖拽阈值比较；`beginPress` 只记录按下点与按下前选区，**不修改选区** |
| `Crisp-main/src/WindowPick.cpp` | overlay 全屏时 `WindowFromPoint` 必然返回 overlay，正确做法是按 Z 序 `EnumWindows` 并显式跳过 overlay |
| 同上 | Shell 类名精确黑名单（`Progman` / `WorkerW` / `Shell_TrayWnd` / `Shell_SecondaryTrayWnd` / `NotifyIconOverflowWindow` / `TopLevelWindowForOverflowXamlIsland` / `Windows.UI.Core.CoreWindow` / `XamlExplorerHostIslandWindow`） |
| 同上 | `DWMWA_CLOAKED` 必须与 `IsWindowVisible`/`IsIconic` 一起判断：商店应用关闭后只被 cloaked，`IsWindowVisible` 仍返回真 |

### 3.2 仅作测试 / 交互参考

- `Crisp-main/tests/TestWindowPick.cpp`：测试自建窗口、**泵消息 + `Sleep(30)` 等 DWM 合成后**
  再读 frame bounds、只断言“在内或容差内”与“非空”，不写死坐标（DPI 会使逻辑坐标落到别处）。
  Phase 1 的 Windows 夹具照此实现。
- `Crisp-main/src/OverlayInternal.h`：`kGrabThreshold = 4` 与注释“**按下不改选区，拖拽才改**”；
  以及会话级 `settled` / `allowHover` 分层——与 docs/14 “`Settled` 是会话状态而非指针手势”一致。
- `snow_shot/.../screenshotoverlayinputhandler.cpp`：`pressActive` 未超阈值时不做任何选区修改，
  超阈值才 `beginSelectionDrag`。其“释放时提交智能选区”的语义**不采用**
  （SnapClip v1 改为停稳预览 + Enter 确认）。

### 3.3 明确不采用

| 来源 | 不采用的部分 | 原因 |
| --- | --- | --- |
| `Crisp WindowPick.cpp` | `IsAltTabCandidate` 的“排除 `WS_EX_TOOLWINDOW` + 排除无标题窗口”更严规则 | 会把浏览器弹出面板、开发工具浮窗等真实可见窗口排除掉；docs/14 §5.3 明确不排除 tool window / no-activate |
| `snow-ui-selector/window.rs` | `EnumChildWindows` 子矩形收集 | 跨进程子窗口枚举不能进入 v1 热路径；服务 v2 深选 |
| `snow-ui-selector/{uia,msaa}.rs`、`uia/cache.rs` | UIA/MSAA 命中路径与 COM 缓存 | v1 只做顶层窗口外框；v2 才引入，且必须独立 worker/apartment |
| `snow-ui-selector/geometry.rs` | 跨显示器取并集包围框 | SnapClip v1 只吸附**当前显示器可见部分**，取交集而非并集 |
| `meazure WindowTool.cpp` | `WindowFromPoint` + `ChildWindowFromPointEx` + `EnumChildWindows` 找最深子窗口，`GetWindowRect` 作边界 | overlay 存在时先命中 overlay；每次鼠标移动跨进程遍历子窗口；`GetWindowRect` 含不可见 resize border |
| 任何参考 | 把 GDI / 同步 `WindowFromPoint` / 同步 `EnumChildWindows` / UIA 全树遍历放入 v1 热路径 | 会在 overlay 消息循环内做跨进程同步调用；`DwmGetWindowAttribute` 同理，必须留在检测 worker |

---

## 4. 根因、影响范围与目标架构

- **根因**：输入层用“按下即改选区”的隐式状态代替显式手势状态（`overlay.rs:1192` →
  `session.rs:194/207`），且完全缺少窗口快照层。两者都属于状态机/模块边界问题，
  必须在根因层重建，不在外层加判断。
- **影响范围**：`overlay.rs` 输入处理与重绘调度、`session.rs` 选区语义、
  `geometry.rs` 坐标转换、`Cargo.toml` features，以及新增的检测 worker 与快照模块。
- **目标架构**（docs/14 §5.2 原文职责边界）：

```text
Win32 overlay/message thread  -> 读取输入、合并最新鼠标位置、维护 PointerGesture 和 WindowSnapshot
Window detection worker      -> EnumWindows、DWM cloaked/frame bounds、refresh、revalidate、validate
GPU/render path              -> 冻结 D3D11 texture、遮罩、hover/selection/preview、局部重绘
Application/session          -> confirmed selection、Settled/edit state、Enter/toolbar confirmation
Tauri/frontend               -> 只接收低频状态、目标 id、尺寸、错误码和完成事件
```

---

## 5. 本阶段落地的契约与埋点（代码）

### 5.1 契约（`src-tauri/src/capture/window_detection/`）

| 契约 | 位置 | 冻结内容 |
| --- | --- | --- |
| `WindowIdentity` | `model.rs` | `hwnd + process_id + class_name_hash`；`matches` 三者全等才视为同一窗口（HWND 重用检测） |
| `WindowCandidate` | `model.rs` | `identity + screen_bounds`（虚拟桌面物理像素）`+ client_bounds`（v1 恒 `None`）`+ z_order + snapshot_epoch` |
| `WindowTarget` / `TargetKind` | `model.rs` | v1 只有 `TopLevelWindowFrame`；`WindowTarget` **不携带**显示器本地坐标 |
| `WindowSnapshot` | `model.rs` | `epoch + candidates`（显示器缓存与可选索引在 Phase 1/2 加入同一结构）；`find` 按 identity；`release()` 释放候选与 epoch |
| `HoverValidity` | `model.rs` | `Valid / BoundsChanged{epoch, identity, new_bounds} / Invalid`；`applies_to` 要求 epoch 与 identity 同时匹配，否则 no-op |
| epoch 规则 | `EpochCounter` | 从 1 开始严格递增；`0` 保留为“尚无快照”，永不与真实快照相等；会话结束 `reset()` |
| request id 规则 | `RequestGate` | 只保留最新请求；`accepts(id)` 为假即丢弃陈旧结果（对应 `stale result dropped` 指标） |
| `PointerGesture` | `gesture.rs` | `None / AutoSnapPreview / PendingPointer / ManualDrag / MoveSelection / ResizeSelection`；**按下不改选区**；`Settled` 明确不属于该枚举 |
| 拖拽阈值 | `should_start_manual_drag` | 位移平方 ≥ 阈值平方；`i64` 防溢出；阈值 ≤ 0 视为立即拖拽 |
| 常量 | `mod.rs` | dwell 120 ms、snap radius 24 px、hover 重验证 250 ms |

### 5.2 诊断埋点（`src-tauri/src/capture/diagnostics.rs`）

`WindowDetectionMetrics` 是 overlay 线程与检测 worker 共享的原子计数器句柄，覆盖
docs/14 §10.2 全部指标：`window_snapshot_refresh_us` / `window_snapshot_release_us` /
`window_hit_test_us` / `window_nearest_target_us` / `window_validate_us` /
`candidate_count` / `hover_target_switch_count` / `stale_target_count` /
`hover_revalidate_stale_dropped_count` / `window_worker_queue_depth` /
`window_worker_max_queue_depth` / `window_worker_stale_result_dropped_count` /
`mouse_move_coalesced_count`。

- 输出前缀 `[snapclip][win-detect]`；`is_verbose()` 默认 `false`，避免洪泛；
  `summary_line()` 给出一行完整读数供会话结束与验收探针使用。
- 队列深度是**有界 gauge**（enqueue 减 dequeue，饱和不减到负数），并保留高水位。

### 5.3 文件清单

| 文件 | 变更 |
| --- | --- |
| `src-tauri/src/capture/window_detection/mod.rs` | 新增：模块职责、线程边界、v1 范围、冻结常量 |
| `src-tauri/src/capture/window_detection/model.rs` | 新增：身份 / 候选 / 目标 / 快照 / hover 判决 / epoch / request id 契约 |
| `src-tauri/src/capture/window_detection/gesture.rs` | 新增：`PointerGesture` 契约、与 `Settled` 的边界、拖拽阈值纯函数 |
| `src-tauri/src/capture/diagnostics.rs` | 新增：§10.2 指标计数器与单行报告 |
| `src-tauri/src/capture/mod.rs` | 修改：导出 `diagnostics`、`window_detection` |

后续阶段将新增 `window_detection/{snapshot,hit_test,provider,spatial}.rs`、
`capture/monitor_cache.rs`、`platform/windows/capture/win/window.rs`，并修改
`Cargo.toml`（`Win32_Graphics_Dwm`）、`capture/geometry.rs`、`capture/session.rs`、
`platform/windows/capture/overlay.rs`、`win/d2d.rs`。

**明确不修改**：`refer/**`（只读参考，且已被 `.gitignore` 忽略）、`crates/**`、
`dist/**`、`comparison-*` 与任何本任务无关的用户改动。

---

## 6. 工作区状态与改动归属

- `git status --short` 在本阶段开始时只有 `?? docs/16-screenshot-window-detection-agent-prompt.md`
  （用户提供的任务提示词，未跟踪）——本阶段不提交它。
- 分支 `main`，远程 `origin` = `https://github.com/jinghu-moon/SnapClip.git`；
  `git ls-remote --heads origin` 成功，push 通道可用。
- 根目录 `.tmp-*`（探针脚本与日志）与 `refer/` 均被 `.gitignore` 覆盖，不进入提交。
- 本阶段未执行任何破坏性 git 命令；未改写历史；未强制 push。

---

## 7. 未执行项与风险

| 项目 | 状态 | 原因 / 替代 |
| --- | --- | --- |
| 多显示器 / 混合 DPI / 负坐标实测 | 未执行 | 本机单显示器；负坐标与跨屏裁剪由 Phase 1 纯函数单测 + Phase 7 人工清单覆盖 |
| 窗口检测指标 P50/P95（50/100/200 候选） | 不适用 | 功能尚未存在；Phase 2 建立首测，Phase 6 补齐分位数 |
| WPA / PresentMon 逐帧采集 | 未执行 | 环境无 PresentMon；以 Present 计数 + 进程 CPU/内存 + 阶段日志替代，Phase 6 再评估 |
| affinity 三层排除的第 1 层 | 未实现 | 当前靠“先冻结后显示”；Phase 5 接入 `SetWindowDisplayAffinity` 与 excluded 集合 |
| HDR / 高对比度 | 未执行 | 环境不具备 |
