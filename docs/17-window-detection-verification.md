# 窗口自动吸附（docs/14）验证记录：Phase 0 ~ Phase 7

本文件是 `docs/14-screenshot-window-detection-design.md` 落地的逐阶段验证记录，
与 `docs/13-screenshot-refactor-verification.md` 同构：每个阶段完成后追加一节，
记录基线、契约冻结、实机证据、性能数据与未覆盖项。

Phase 0 是修改前的真实行为基线、参考源码审计结论、契约冻结内容与质量门禁证据；
后续阶段（Phase 1~7）的对比均以 Phase 0 数据为基准。

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

---

## Phase 1：DPI、几何与 Windows 窗口检测基础设施

### 1.1 结论

建立了窗口检测的坐标、过滤与 Win32/DWM 读取基础层。本阶段**不含**快照组装与命中
算法（Phase 2），也**不改变** overlay 的输入行为：overlay 仍按原逻辑运行，
新增能力由测试与后续阶段消费。

### 1.2 落地内容

| 能力 | 位置 | 说明 |
| --- | --- | --- |
| DPI 声明 | `platform/windows/capture/monitor.rs` | 按文档降级链 Per-Monitor V2 → Per-Monitor → System；已声明时读取实际 thread context，不降级；返回值用于日志 |
| DPI 启动顺序 | `app/mod.rs` | 在 `tauri::Builder` 之前声明，早于任何窗口创建与光标读取 |
| screen→local 转换 | `capture/geometry.rs::window_rect_to_local` | 裁剪到当前显示器并减去显示器原点；纯函数；不产生负尺寸 |
| 显示器矩形缓存 | `capture/monitor_cache.rs` | 刷新周期内的显示器物理矩形集合；`intersects_any` 用半开区间做进程内相交，替代逐窗口 `MonitorFromRect`；`release()` 显式释放 |
| Win32 FFI | `platform/windows/capture/win/window.rs` | `IsWindow`/`IsWindowVisible`/`IsIconic`/扩展样式/PID/类名/DWM cloaked/DWM frame bounds（失败或空才退 `GetWindowRect`，退后再查空） |
| 两阶段过滤 | 同上 + `capture/window_detection/snapshot.rs` | `EnumWindows` 回调只做廉价检查并顺带读类名；DWM 查询由 `read_dwm_batch` 在回调**之后**批量执行 |
| Shell 黑名单 | `capture/window_detection/provider.rs` | 8 个类名的**精确**匹配（不区分 ASCII 大小写，与 Win32 自身语义一致），不做前缀/样式判断 |
| 排除集合 | 同上 `Exclusions` | `excluded_hwnds` + `excluded_process_ids`，带 `epoch`；集合实际变化才递增 epoch |
| 过滤策略 | `capture/window_detection/snapshot.rs` | `passes_cheap_policy`（排除集合 + shell 面）、`passes_dwm_policy`（cloaked / 无边界 / 全屏外）、`classify`、`candidates_from_classified` |
| Cargo feature | `Cargo.toml` | 补加 `Win32_Graphics_Dwm` |

**不排除** `WS_EX_TOOLWINDOW` / `WS_EX_NOACTIVATE`；只有
`WS_EX_LAYERED && WS_EX_TRANSPARENT` 同时成立才视为点击穿透。

### 1.3 静态与单元验证

| 项目 | 命令 | 结果 |
| --- | --- | --- |
| 单元测试 | `cargo test --lib` | **252 passed / 0 failed**（Phase 0 基线 219 → +33） |
| 编译零告警 | `cargo check --all-targets` | exit 0，**0 warnings** |
| Clippy（本阶段文件） | `cargo clippy --lib --tests` | 本阶段新增/修改文件 **0 告警** |
| 前端 | `npm run typecheck` / `npm run build` | exit 0（未改前端，确认无退化） |

新增测试要点：

| 测试 | 断言 |
| --- | --- |
| `geometry::window_rect_conversion_subtracts_the_monitor_origin` | 负虚拟桌面原点（副屏在左上）转换正确 |
| `geometry::window_rect_conversion_clips_a_window_that_spans_monitors` | 跨屏窗口只保留当前显示器可见带 |
| `geometry::window_rect_conversion_rejects_rects_off_this_monitor` | 完全在屏外/仅贴边（半开）返回空 |
| `geometry::window_rect_conversion_normalises_degenerate_input` | 退化/倒置矩形不产生负尺寸 |
| `monitor_cache::*`（6 项） | 空缓存、退化矩形剔除、负坐标、半开边界、跨屏可见、`release` 后不再命中 |
| `window::click_through_requires_both_layered_and_transparent` | 单独 `LAYERED`、单独 `TRANSPARENT` 不判穿透；组合与手写画布样式（`0x0a08_00a8`）判穿透 |
| `window::a_visible_tool_window_is_enumerated_and_a_hidden_one_is_not` | 真机：可见 tool window 进入候选；`SW_HIDE` 后不再进入，句柄仍有效 |
| `window::a_minimised_window_is_not_a_candidate` | 真机：`SW_MINIMIZE` 后 `IsIconic`/不可见，且不在候选内 |
| `window::a_click_through_layered_window_is_not_a_candidate` | 真机：点击穿透 overlay 被剔除，其下方普通窗口仍可选 |
| `window::frame_bounds_include_the_title_bar_and_exclude_the_invisible_border` | 真机：DWM 外框包含标题栏（在客户区之上）、不含不可见 resize border（对 `GetWindowRect` 的容差断言） |
| `window::window_attributes_are_stable_for_a_live_window` | 类名 `Static`、PID 为本进程、可见、非最小化、非 cloaked |
| `window::dwm_batch_returns_one_read_per_handle` | 批量 DWM 每个句柄一条结果；无效句柄无边界 |
| `snapshot::pass_one_*` / `pass_two_*` / `classify_applies_both_passes_and_pairs_identities` | 两阶段过滤矩阵：自家窗口、shell 面、tool/no-activate、cloaked、无边界、屏外、退化矩形 |
| `snapshot::class_name_hash_is_stable_and_class_sensitive` | 身份哈希稳定且类名敏感 |
| `provider::*`（5 项） | 排除集合命中/epoch 语义、shell 面精确匹配、provider trait 可用纯实现替身 |
| `monitor::a_second_declaration_never_downgrades_an_aware_process` | 重复声明不降级已有的 (V2/PM/System) 上下文 |

**真机夹具方法**：测试自建窗口（`CreateWindowExW` + `STATIC`），`SW_SHOWNA` 避免抢焦点，
泵消息 + 短 `Sleep` 等 DWM 提交后再读边界；坐标断言全部相对 `GetWindowInfo`/`GetWindowRect`
而非写死像素（对齐 `Crisp tests/TestWindowPick.cpp` 的做法）。

**一个必须记录的坑**：几何断言要求进程已声明 DPI 感知。未声明时 `GetWindowInfo`/
`GetWindowRect` 返回**虚拟化**矩形而 `DWMWA_EXTENDED_FRAME_BOUNDS` 始终是物理像素，
两者不可比较（实测：帧 368×259 @ (191,180) vs 客户区 244×141 @ (128,151)，比值正是
150% 缩放）。测试因此在夹具建立前声明一次 Per-Monitor V2；这是应用启动路径本身的要求，
不是测试特例。

### 1.4 实机回归（debug exe + `keybd_event` 注入，4K/DPI144/单屏 WGC）

| 指标 | Phase 0 基线 | Phase 1 复测 |
| --- | --- | --- |
| 启动 DPI 日志 | 无 | `[snapclip][startup] dpi awareness=per-monitor-v2` |
| F5 → visible（warm） | 37–45 ms | 14–17 ms（`prepare_elapsed_ms`），worker 队列 0–33 ms |
| 会话取消 | 5/5 干净 | 2/2 干净（`reason=escape`，`release session` 一一对应） |
| Private Bytes / Working Set | 56.0 MB / 71.1 MB | **56.8 MB / 71.3 MB**（无退化） |
| Present（会话内） | ≈53 次/会话 | 52 次/会话（104 / 2） |
| 错误行 | 0 | 0 |

### 1.5 未执行项与风险

| 项目 | 状态 | 原因 / 替代 |
| --- | --- | --- |
| 普通/最大化/无边框窗口的**吸附**结果 | 不适用 | 吸附尚未实现；本阶段验证的是过滤与边界读取（普通/无边框/最小化/隐藏/穿透已覆盖） |
| cloaked 真机夹具 | 未覆盖真机 | 合成 cloaked 窗口需要 UWP/虚拟桌面宿主；策略函数由纯单测覆盖，cloaked 的真机读取路径由 Phase 7 人工验收 |
| 多显示器 / 混合 DPI 真机 | 未执行 | 本机单显示器；负坐标、跨屏裁剪、显示器缓存的纯单测已覆盖 |
| `win::window` 的 FFI 面在非 test 构建下暂标 `dead_code` | 临时 | provider 编排在 Phase 2 接入；接入后移除该 allow（见文件头注释） |

---

## Phase 2：WindowSnapshot、命中算法与生命周期

### 2.1 结论

快照成为 overlay 可以安全读取的唯一窗口数据源：命中与最近距离查询是纯缓存读，
不触发 `EnumWindows`/DWM；快照携带 epoch、按 Z 序的候选与本次刷新周期的显示器缓存，
失效时整体替换并显式释放。**未引入空间索引**——见 2.4 的基准数据。

### 2.2 落地内容

| 能力 | 位置 | 说明 |
| --- | --- | --- |
| 快照查询 | `capture/window_detection/hit_test.rs` | `WindowSnapshot::hit_test`（半开区间、Z 序最前）、`nearest_target`（半径内最近、距离相同先取**包含**该点的候选再按 Z 序）、`rect_distance_squared` |
| 边界写回 | 同上 `apply_candidate_update` | `BoundsChanged` 仅在 `epoch` 与 `identity` 同时匹配时写回候选矩形（修复“hover 跳回旧位置”的根因）；`Valid`/`Invalid` 不改数据 |
| 快照生命周期 | `capture/window_detection/model.rs` | `epoch + candidates + monitors`；`release()` 同时释放候选与显示器缓存；`is_current`/`find` 按身份查询 |
| 代际隔离 | 同上 + `hit_test.rs` | 用一个 `is_live` 判据同时要求候选 `snapshot_epoch == snapshot.epoch` 且矩形可用：混入上一代的候选对象在任何查询里都不可见 |
| Windows provider | `platform/windows/capture/window_detection.rs` | `TopLevelWindowProvider`：`refresh`（显示器缓存 → 廉价枚举 → 批量 DWM → 策略过滤 → 递增 epoch → 组装快照）、`validate`、`revalidate_hover` |
| 身份校验 | 同上 `read_target` | 可见 / 非最小化 / 非 cloaked / PID 与类名哈希匹配（HWND 重用检测）/ 边界可读 |

**距离语义**：几何距离取**闭包**（跨边对称：边左右各 5 px 都是 5），包含判定取**半开**
（共享边只属于一个窗口）。两者在 `nearest_target` 汇合：距离相同先取真正包含该点的
候选，再按 Z 序。没有这条规则时，正好落在共享边上的点会是“两个窗口距离都是 0”，
只能由 Z 序静默决定用户意图。

### 2.3 静态与单元验证

| 项目 | 命令 | 结果 |
| --- | --- | --- |
| 单元测试 | `cargo test --lib` | **275 passed / 0 failed**（Phase 1 基线 252 → +23） |
| 编译零告警 | `cargo check --all-targets` | exit 0，**0 warnings** |
| Clippy（本阶段文件） | `cargo clippy --lib --tests` | 本阶段新增/修改文件 **0 告警** |

新增测试要点：

| 测试 | 断言 |
| --- | --- |
| `distance_is_zero_inside_a_rectangle_and_positive_outside` | 边界为 0；越界 1 px = 1；对角 = 3²+4²；左右对称 |
| `a_shared_edge_is_zero_distance_from_both_but_belongs_to_one` | 共享边距离都为 0，但 `hit_test` 与 `nearest_target` 都选被包含的那个 |
| `hit_test_picks_the_frontmost_overlapping_window` | 三窗重叠取 Z 序最前 |
| `hit_test_uses_half_open_edges` | 左/上边命中、右/下边归邻窗、越过最后窗口为空 |
| `nearest_target_respects_the_snap_radius` | 恰好 24 px 命中、25 px 不命中 |
| `nearest_target_breaks_ties_by_z_order` | 等距时取 Z 序更小者 |
| `a_point_in_two_overlapping_windows_belongs_to_the_frontmost` | 双覆盖点距离均为 0，按 Z 序裁决 |
| `degenerate_rectangles_are_never_hit_or_snapped_to` | 零宽/倒置矩形既不命中也不参与吸附 |
| `a_released_snapshot_answers_nothing` | `release()` 后两个查询都不返回目标 |
| `candidates_from_another_generation_are_invisible` | 混入旧代候选时不返回旧对象 |
| `bounds_changed_is_written_back_into_the_snapshot` | 写回后新矩形立即可命中、旧矩形立即失效、重复写回是 no-op |
| `stale_or_mismatched_updates_are_ignored` | 错 epoch / HWND 重用 / 未知窗口 / 矩形未变 都不改写快照 |
| `top_level_window_provider::refresh_builds_a_labelled_snapshot_with_ordered_candidates` | 真机：候选非空、Z 序严格递增、全部属于当前 epoch、全部落在显示器可见范围 |
| 同上 `each_refresh_advances_the_epoch_and_drops_the_previous_snapshot` | 真机：epoch 严格递增，旧 target 对新快照即 stale |
| 同上 `exclusions_remove_a_window_from_the_snapshot` / `our_own_process_can_be_excluded_entirely` | 真机：句柄排除与进程排除都生效 |
| 同上 `hover_revalidation_reports_bounds_changes_and_stale_targets` | 真机：未移动 → `Valid`；矩形被改 → `BoundsChanged{epoch, identity, new_bounds}`；未知句柄 → `Invalid` |
| 同上 `reset_clears_the_epoch_and_the_cached_topology` | `reset()` 后显示器缓存为空、下个会话 epoch 从 1 重新开始 |

### 2.4 线性扫描基准证据（docs/14 §10.2 预算：P95 < 0.1 ms）

`hit_test_and_nearest_target_stay_inside_the_latency_budget` 在 50 / 100 / 200 候选下各采样 2000 次，
候选为「每窗 300×200、按 z 递增铺开」的真实量级：

| 候选数 | hit_test P50 | hit_test P95 | nearest_target P50 | nearest_target P95 |
| --- | --- | --- | --- | --- |
| 50 | 600 ns | 700 ns | 900 ns | 1000 ns |
| 100 | 1200 ns | 1300 ns | 1700 ns | 1800 ns |
| 200 | 2300 ns | 2700 ns | 3500 ns | 4000 ns |

即 200 候选下 P95 仍比预算低 **约 25~37 倍**（debug 构建）。结论：v1 使用 `Vec` 线性扫描，
**不预置 R-tree**；只有将来真实桌面数据显示热点时才按 docs/14 §5.4 的规则引入空间索引，
并保留半开区间二次校验与 Z 序裁决。

### 2.5 质量门禁逐条对应

| 门禁 | 证据 |
| --- | --- |
| `WM_MOUSEMOVE` 只使用快照，不触发 EnumWindows/DWM | `hit_test`/`nearest_target` 全部是 `&self` 纯读，模块内无任何 FFI 引用（`hit_test.rs` 只 `use` geometry/monitor_cache/model） |
| 重叠窗口始终选择最顶层 | `hit_test_picks_the_frontmost_overlapping_window`、`a_point_in_two_overlapping_windows_belongs_to_the_frontmost` |
| 远离候选不产生目标 | `nearest_target_respects_the_snap_radius`、`degenerate_rectangles_are_never_hit_or_snapped_to` |
| 快照失效后旧对象无法返回有效目标 | `a_released_snapshot_answers_nothing`、`candidates_from_another_generation_are_invisible`、`stale_or_mismatched_updates_are_ignored` |
| ≤16 候选线性扫描有基准证据，未证明前不预置 R-tree | 2.4 表格；`WindowSnapshot` 结构内**没有**索引字段，并附文档说明为什么没有 |

### 2.6 未执行项与风险

| 项目 | 状态 | 原因 / 替代 |
| --- | --- | --- |
| overlay 真正在 `WM_MOUSEMOVE` 中调用 `hit_test` | 未接线 | 属于 Phase 3（输入状态机）与 Phase 5（渲染）的职责；本阶段先冻结可安全调用的接口与数据 |
| provider 在非 test 构建下暂标 `dead_code` | 临时 | 检测 worker（Phase 4）接手其所有权；接线后移除该 allow |
| 真实桌面 200 候选场景 | 未构造 | 本机常驻顶层窗口数量远小于 200；以合成候选集做量级基准，Phase 6 在真实桌面补测 |

---

## Phase 3：PointerGesture 与停稳自动吸附预览

### 3.1 结论

“按下即改选区”的隐式状态已被显式手势状态机替换：**按下不再修改选区**，拖动阈值以
系统拖拽距离为准，光标停稳 120 ms 后对缓存快照求最近窗口产生预览，`Enter` 经检测 worker
校验后才提交吸附。overlay 消息线程在整个过程中不执行 `EnumWindows`/DWM。

### 3.2 落地内容

| 能力 | 位置 | 说明 |
| --- | --- | --- |
| 手势状态机 | `capture/window_detection/gesture.rs::GestureState` | `press` / `move_cursor` / `release` / `apply_dwell` / `clear_preview` / `reset`；纯逻辑，无 Win32、无 GPU |
| 手势结果 | 同上 `PressOutcome` / `MoveOutcome` / `ReleaseOutcome` | 明确告诉 overlay 该做什么，overlay 只做转发与渲染 |
| 会话 API | `capture/session.rs` | 删除“按下即改选区”：`press` 只做命中判定；新增 `begin_drag(press_point, mode, creating)` 与 `snap_to(rect)`（裁剪 + 最小尺寸校验） |
| 检测 worker | `platform/windows/capture/detection_worker.rs` | 容量 1 的**最新请求**邮箱；`Refresh`/`Confirm` 任务；`PostThreadMessageW(DETECTION_READY_MESSAGE)` 回投；结果带 request id；`shutdown` 幂等 |
| 停稳计时器 | `overlay.rs`（`DWELL_TIMER_ID`） | 120 ms 一次性计时器；到期只做 generation 校验 + 缓存 `nearest_target` + 一次坐标转换 |
| hover | `overlay.rs::update_hover` | 每次鼠标移动后查缓存快照；相同 hwnd 且相同矩形不重绘 |
| 预览 | `overlay.rs::on_dwell` | 产生/替换/清除 `AutoSnapPreview`；按下即被 `PendingPointer` 取代（等于“超阈值取消预览”） |
| 确认 | `overlay.rs::confirm_snap_preview` / `apply_confirmation` | `Enter` 投递 `Confirm{target}`；worker 回投后校验 identity 才 `snap_to`；失败则保留原选区、清预览、刷新快照 |
| 自身排除 | `OverlayController::new` | 注册 overlay HWND + 本进程 PID（docs/14 §7 第 2 层）；防止全屏 overlay 命中自己 |
| 诊断开关 | `capture/diagnostics.rs::VERBOSE_ENV` | `SNAPCLIP_WIN_DETECT_VERBOSE=1` 打开逐操作日志，默认关闭 |

### 3.3 关键行为对比（修改前 / 修改后，实测）

| 项目 | 修改前 | 修改后 | 预期 |
| --- | --- | --- | --- |
| 在空白处按下 | 立刻产生零尺寸选区并进入 Selecting | 只记录 `PendingPointer`，选区不变 | 符合 docs/14 §4.2 |
| 按下后轻微抖动并释放 | 释放即提交（或留下 1 px 选区） | 保持 Click，不提交、不改选区 | 抖动不误判 |
| 光标停稳 | 无任何吸附行为 | 120 ms 后产生最近窗口预览 | 停稳才预览 |
| 鼠标释放 | 无吸附语义 | **不确认**吸附 | 只能 Enter/工具栏确认 |
| 按下前已有预览 | — | 按下即清除预览，超过阈值进入 ManualDrag | 拖拽让位 |
| 命中手柄/选区内部 | 同（立即拖拽） | 同（立即拖拽，无阈值） | 不退化 |

### 3.4 静态与单元验证

| 项目 | 命令 | 结果 |
| --- | --- | --- |
| 单元测试 | `cargo test --lib` | **290 passed / 0 failed**（Phase 2 基线 275 → +15） |
| 编译零告警 | `cargo check --all-targets` | exit 0，**0 warnings** |
| Clippy | `cargo clippy --lib --tests` | 本阶段新增文件 0 告警；overlay.rs 剩余告警均为既有 annotation/指针 cast 项（位置与 Phase 2 相同） |
| 前端 | `npm run typecheck` / `npm run build` | exit 0 |

新增测试要点：

| 测试 | 断言 |
| --- | --- |
| `a_plain_press_never_creates_or_moves_a_selection` | 空白处按下 → `PendingPointer`，释放 → Click，选区不被触碰 |
| `a_press_on_a_handle_or_inside_starts_the_edit_immediately` | 命中手柄/内部 → 立即 Resize/Move |
| `a_tiny_jitter_stays_pending_and_a_real_move_starts_the_drag` | 2 px 抖动保持 Pending；跨 4 px 阈值转 ManualDrag 且锚点为按下点 |
| `releasing_the_button_never_confirms_an_automatic_snap` | 有预览时按下 → 预览清除 → 释放为 Click（不确认） |
| `a_stale_dwell_result_never_produces_a_preview` | 计时器 generation 与当前不符时**不产生**预览 |
| `a_held_button_blocks_the_dwell_preview` | 按住按钮时 dwell 不产生预览 |
| `dwell_replaces_a_preview_only_when_the_rectangle_changes` | 同矩形不重绘、换候选替换、离开半径清除 |
| `the_preview_carries_the_selection_it_must_be_able_to_restore` | 预览携带确认前选区 |
| `reset_drops_the_preview_and_the_pending_press` | 会话结束清理彻底，且 generation 前进 |
| `session::a_press_alone_never_creates_a_zero_size_selection` | 生产侧 API 同样保证按下不产生选区 |
| `session::pressing_outside_keeps_the_selection_until_the_drag_starts` | 点击不删除已确认选区；只有开始拖拽才替换 |
| `session::snap_to_*`（2 项） | 采用可用矩形、越界裁剪、空/退化/过小/全屏外拒绝且不改状态 |
| `detection_worker::*`（4 项） | 请求 id 回传、邮箱只留最新、确认返回有效性、shutdown 幂等且不阻塞 |

### 3.5 实机端到端证据（`.tmp-p3-probe.ps1`，4K/DPI144/单屏 WGC）

探针自建一个 420×300 的 WinForms 夹具窗口（属 PowerShell 进程，因此对检测可见），
注入 F5 → 停稳 → 点击 → 再停稳 → Enter → Esc，并开启 `SNAPCLIP_WIN_DETECT_VERBOSE=1`。
应用自身日志（节选，两次会话）：

```text
[win-detect] snapshot epoch=1 candidates=4
[win-detect] hover hwnd=12583054 z=0 bounds=(2111,960)->(2719,1399)
[win-detect] auto-snap preview hwnd=12583054 epoch=1 local=(2111,960)->(2719,1399)
[win-detect] key down vk=0x1B state=Selecting          # Esc 取消
[capture] cancel session=…-1 reason=escape active=true
[win-detect] snapshot epoch=2 candidates=4             # 第二次 F5：新 epoch、无旧快照
[win-detect] auto-snap preview hwnd=12583054 epoch=2 local=(2111,960)->(2719,1399)
[win-detect] key down vk=0x0D state=Selecting          # Enter
[win-detect] confirm requested hwnd=12583054 epoch=2 confirmation=3
[win-detect] snap confirmed hwnd=12583054 selection=(2111,960)->(2719,1399)
[win-detect] key down vk=0x1B state=Selected           # 已确认后 Esc
```

点击路径的日志（第二步）证明按下不再产生选区：

```text
[capture] pointer down session=…-1 point=(2550,1275), hit=Create, outcome=Pending
[capture] pointer up   session=…-1 selection=(0,0)->(0,0) state=Selecting
```

会话指标（每次会话结束输出一行）：

| 指标 | 会话 1 | 会话 2 |
| --- | --- | --- |
| `window_snapshot_refresh_us` | 760 / 792 µs（worker） | 1009 / 598 µs（worker） |
| `window_hit_test_us` | last 1, max 3 µs, n=7 | last 2, max 6 µs, n=4 |
| `window_nearest_target_us` | last 5, max 5 µs, n=4 | last 2, max 6 µs, n=2 |
| `window_validate_us` | — | **last 28 µs, n=1**（Enter 校验） |
| `candidate_count` | 4 | 4 |
| `window_worker_queue_depth` / max | 0 / 1 | 0 / 1 |
| `window_worker_stale_result_dropped_count` | 0 | 0 |
| 错误行 | 0 | 0 |

夹具窗口位于逻辑 (1400,640) 尺寸 420×300，被检测到的边界为
`(2111,960)->(2719,1399)`（物理 630×439，等于 1.5×DPI 缩放后的值），确认后选区与之逐像素一致。

### 3.6 质量门禁逐条对应

| 门禁 | 证据 |
| --- | --- |
| 按下不会创建零尺寸选区 | 单测两处 + 实机日志 `outcome=Pending` / `selection=(0,0)->(0,0)` |
| 轻微抖动不会误进入手动拖拽 | `a_tiny_jitter_stays_pending_and_a_real_move_starts_the_drag` |
| 光标停稳会产生最近窗口预览 | 实机 `auto-snap preview …`；`window_nearest_target_us` 有采样 |
| 鼠标释放不确认吸附 | `releasing_the_button_never_confirms_an_automatic_snap`；`WM_LBUTTONUP` 分支只在 `CommitDrag` 时提交 |
| Enter/工具栏确认入口清晰且不在鼠标线程同步验证 | 实机 `confirm requested` → worker（`window_validate_us`）→ `snap confirmed`；`on_key_down` 与鼠标路径互不调用 |
| 手动框选、移动、缩放不被自动吸附破坏 | `a_press_on_a_handle_or_inside_starts_the_edit_immediately`、`session::dragging_inside_moves_the_existing_selection`（既有测试保持通过） |

### 3.7 未执行项与风险

| 项目 | 状态 | 原因 / 替代 |
| --- | --- | --- |
| 预览/悬停高亮的**视觉**呈现 | 未实现 | 属于 Phase 5（D2D 绘制层）；本阶段以状态机 + 日志证明预览状态正确 |
| hover 定期重验证（250 ms 定时器） | 未实现 | 属于 Phase 4；provider 的 `revalidate_hover` 已实现并有真机测试，缺少的是定时触发与回投处理 |
| 窗口移动/关闭后的 `BoundsChanged` 在线写回 | 未实现 | 同上：`apply_candidate_update` 已单测覆盖，缺定时投递 |
| 真实「点击后 120 ms 内再移动」的时序抖动 | 未专门构造 | 以 generation 单测覆盖；实机日志未见陈旧预览 |
| 工具栏（Vue）确认入口 | 未接线 | 工具栏走 `OverlayCommand::Confirm`；预览确认目前只有 Enter 路径，Phase 5 统一 |
| `mouse_move_coalesced_count` 语义 | 部分 | 目前统计“重绘 tick 已经挂起时到达的移动”（被合并进同一次 Present）；`WM_MOUSEMOVE` 本身仍逐条处理，Phase 6 若测得热点再改成 PeekMessage 级合并 |

---

## Phase 4：检测 worker、hover 重验证与过期处理

### 4.1 结论

hover 时效性路径打通：overlay 每 250 ms 只投递当前 hover 的 `{hwnd, identity, epoch}`，
worker 执行单窗口重验证并回投 `Valid / BoundsChanged / Invalid`，overlay 校验
epoch + identity 后处理。`BoundsChanged` **先写回快照再重算 hover/预览**，
实机验证了“窗口移动后 hover/预览跟随新边界、不跳回旧位置”。

### 4.2 落地内容

| 能力 | 位置 | 说明 |
| --- | --- | --- |
| 重验证任务 | `detection_worker.rs::Job::Revalidate` | 与 `Refresh`/`Confirm` 共用同一个容量 1 的最新请求邮箱；`revalidate_hover` 在 worker 线程执行 |
| hover 定时器 | `overlay.rs`（`HOVER_TIMER_ID`, 250 ms） | 会话可见时启动、会话结束停止；tick 只做 `hover_target` 判空 + 单飞检查 + 投递 |
| 单飞控制 | `hover_request: Option<RequestId>` | 同一时刻只允许一个重验证在途，快速移动不会堆积 |
| 结果处理 | `overlay.rs::on_detection_ready` → `apply_hover_validity` | `Valid` 无动作；`BoundsChanged` → `apply_candidate_update` 写回快照 → 重算 hover → 重算预览；`Invalid` → 丢弃 hover/预览 → 刷新快照 |
| 陈旧结果丢弃 | 三处 request id 校验 | refresh / confirm / revalidate 各自比对当前请求；不匹配即计数丢弃 |
| 会话清理 | `begin_window_detection` / `release_session` | 三个 request 与两个定时器统一复位；worker 由 `DetectionWorker::Drop` join（`shutdown` 幂等） |

**为什么 `Invalid` 不立即用旧快照重命中**：旧快照里仍然列着那个已经消失的窗口，用它重命中
会把刚判定失效的目标又取回来。实现是清 hover/预览 + 请求刷新，等新快照落地后再由
`on_detection_ready → update_hover` 重命中一次——语义与 docs/14 §5.4 的“刷新快照后重命中一次”
一致，只是把重命中放在了快照真正更新之后。

### 4.3 静态与单元验证

| 项目 | 命令 | 结果 |
| --- | --- | --- |
| 单元测试 | `cargo test --lib` | **291 passed / 0 failed**（Phase 3 基线 290 → +1：worker 重验证分类） |
| 编译零告警 | `cargo check --all-targets` | exit 0，**0 warnings** |
| 前端 | `npm run typecheck` / `npm run build` | exit 0 |

Phase 2/4 已覆盖的相关单测（此处一并计入门禁证据）：
`hit_test::bounds_changed_is_written_back_into_the_snapshot`、
`hit_test::stale_or_mismatched_updates_are_ignored`、
`window_detection::hover_revalidation_reports_bounds_changes_and_stale_targets`（真机）、
`detection_worker::a_revalidate_request_classifies_the_hovered_window`、
`detection_worker::the_mailbox_keeps_only_the_newest_request`。

### 4.4 实机端到端证据（`.tmp-p4-probe.ps1`）

夹具窗口初始逻辑 (1400,640)（物理 `(2111,960)-(2719,1399)`），光标停在其内部；
随后把夹具移动到逻辑 (1300,580)（物理 `(1961,870)-(2569,1309)`，光标仍在窗口内），
最后 `Hide()`。应用自身日志：

```text
[win-detect] snapshot epoch=1 candidates=4
[win-detect] hover hwnd=18416204 z=0 bounds=(2111,960)->(2719,1399)
[win-detect] auto-snap preview hwnd=18416204 epoch=1 local=(2111,960)->(2719,1399)
[win-detect] hover bounds changed hwnd=18416204 new=(1961,870)->(2569,1309)   # worker 重读外框并写回
[win-detect] hover hwnd=18416204 z=0 bounds=(1961,870)->(2569,1309)           # 与写回数据同源，未跳回旧矩形
[win-detect] auto-snap preview hwnd=18416204 epoch=1 local=(1961,870)->(2569,1309)
[win-detect] hover invalid hwnd=18416204                                      # 窗口被隐藏
[win-detect] snapshot epoch=2 candidates=3                                    # 刷新后死窗口消失
[win-detect] hover hwnd=5967024 z=0 bounds=(0,0)->(3840,2088)                 # 以当前光标重命中下层窗口
[win-detect] auto-snap preview hwnd=5967024 epoch=2 local=(0,0)->(3840,2088)
```

会话指标：`window_validate_us` n=16、last 56 µs、max 219 µs（250 ms 周期、单飞）；
`window_hit_test_us` n=28、max 1 µs；`window_nearest_target_us` n=7、max 2 µs；
`window_worker_queue_depth=0`、`window_worker_max_queue_depth=1`、
`window_worker_stale_result_dropped_count=0`、`hover_revalidate_stale_dropped_count=0`、
`stale_target_count=1`（对应隐藏窗口）、错误行 0。

### 4.5 质量门禁逐条对应

| 门禁 | 证据 |
| --- | --- |
| 窗口移动后 hover/预览跟随新边界，不跳回旧位置 | 4.4 日志第 4~6 行（`hover bounds changed` → `hover` 新矩形 → 预览新矩形） |
| 窗口关闭或 HWND 重用不会误吸旧矩形 | `hover invalid` + `apply_candidate_update` 的 identity 校验单测 + `read_target` 的 PID/类名哈希比对 |
| overlay 消息循环不执行同步 DWM | hover tick 只投递；`window_validate_us` 由 worker 记录；`overlay.rs` 中 DWM 只出现在 provider 模块 |
| 快速移动和连续刷新时陈旧结果被丢弃 | 三个 request id 校验 + `hover_revalidate_stale_dropped_count` 指标；`hit_test::stale_or_mismatched_updates_are_ignored` |
| 关闭会话不会等待无限期 worker 或泄漏线程/资源 | `DetectionWorker::shutdown` 幂等 + `Drop` join；`release_session` 复位三个请求与两个定时器；实机 2 次会话无残留线程/句柄告警 |

### 4.6 未执行项与风险

| 项目 | 状态 | 原因 / 替代 |
| --- | --- | --- |
| provider 级 bounded timeout / quarantine | 未实现 | v1 不引入 UIA/MSAA；`DwmGetWindowAttribute` 本身无超时参数，实测单窗口重验证 max 219 µs。若 Phase 6 实测出现长尾，再按 docs/14 §5.5 引入隔离 |
| HWND 真机重用（close→open 同句柄） | 未构造 | 需要精确控制句柄回收；以 `WindowIdentity` 三元组单测 + `read_target` 的 PID/类名校验覆盖，Phase 7 人工复核 |
| 显示器拓扑变化时的 hover 失效 | 未覆盖 | 显示器变化走 `WM_DISPLAYCHANGE` → 取消会话（既有路径）；新会话重新建快照 |
