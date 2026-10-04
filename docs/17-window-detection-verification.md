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
