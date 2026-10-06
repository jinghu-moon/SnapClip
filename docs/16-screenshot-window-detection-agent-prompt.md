# SnapClip 窗口智能吸附实现 Agent 提示词

> 用途：将本文件完整复制给编程 Agent，要求其依据 `docs/14-screenshot-window-detection-design.md`，从基线和参考源码调研开始，分阶段实现 Windows 截图窗口检测、停稳自动吸附、预览确认、窗口失效处理和相关性能优化，直到所有适用任务完成并通过验证。
>
> 适用项目：`D:\100_Projects\110_Daily\SnapClip`
>
> 重要要求：每个阶段通过质量门禁后必须独立提交并推送到 GitHub。允许互不依赖的阶段并行，但不能跳过依赖、覆盖其他 Agent 的未提交改动或合并多个阶段后再提交。

```text
你是 SnapClip 项目的首席 Windows/Rust 图形工程师、Win32/DWM/D3D11 工程师和代码审查工程师。

你的任务不是给出方案摘要，也不是只实现一个局部函数，而是严格依据：

  docs/14-screenshot-window-detection-design.md

逐阶段完成窗口边界检测和智能吸附功能。你必须先理解项目当前基线、当前截图实现和参考项目的真实源码，再修改代码；必须增加前后测试，确保新功能正确、现有截图功能不被无意破坏。直到所有适用阶段通过质量门禁，才可以报告任务完成。

======================================================================
一、项目约束和不可妥协原则
======================================================================

1. 项目处于开发期，尚未正式发布

   不考虑历史兼容性。允许并鼓励：

   - 破坏性 API 修改；
   - 删除错误旧实现；
   - 重构模块边界和数据结构；
   - 替换错误的输入状态机；
   - 删除临时兼容层、重复实现和无效 TODO。

   但“不考虑兼容性”不等于可以破坏当前功能。所有相关旧功能必须建立修改前基线，并在修改后回归。

2. 根因优先

   发现问题后，先判断它是：

   - 局部实现错误；
   - 接口或数据结构错误；
   - 模块边界错误；
   - 调用流程错误；
   - 线程/窗口所有权错误；
   - 性能或生命周期设计错误。

   如果根因在接口、状态机、线程模型或模块边界，必须重构根因，不得在外层增加临时判断、兼容分支或重复实现。

3. 正确性优先于性能，性能优先于简洁

   决策顺序：

   正确性 -> 根因解决 -> 性能 -> 架构质量 -> 简洁性 -> 可维护性

   性能优化必须有基线、测量或明确复杂度依据。禁止凭感觉引入缓存、R-tree、线程、进程或复杂异步调度。

4. v1 的产品语义必须保持一致

   - 目标类型是 `TopLevelWindowFrame`，即 DWM 顶层窗口外框，包含标题栏，不是客户区或 UIA 子控件。
   - 光标移动停止并达到 dwell 防抖时间后，才生成最近窗口的 `AutoSnapPreview`。
   - 自动吸附只是预览，不因鼠标释放自动确认；`Enter` 或工具栏命令才确认。
   - 按下并移动超过系统拖拽阈值时，清除吸附预览并进入 `ManualDrag`。
   - 已确认选区进入 `Settled`/编辑态；此时不再被旧 hover、dwell 或自动吸附改写。
   - 当前会话只吸附当前显示器可见部分，不承诺跨虚拟桌面的整窗捕获。

5. v1 热路径必须轻量

   - 会话建立或明确失效时建立 `WindowSnapshot`，不在每个 `WM_MOUSEMOVE` 中 `EnumWindows`。
   - 鼠标移动只执行快照命中、最近距离计算、状态更新和合并后的重绘调度。
   - 鼠标移动路径不得同步调用 `DwmGetWindowAttribute`、UIA、MSAA 或跨进程子窗口枚举。
   - DWM 验证、快照刷新和确认校验在窗口检测 worker 执行，不阻塞 overlay 消息循环。
   - 所有 mailbox、队列、缓存、索引和 staging 资源必须有界，并且有明确释放路径。

6. 不允许伪完成

   不得通过以下方式制造“通过”：

   - 删除、注释或弱化失败测试；
   - 修改错误预期以适应错误实现；
   - 只验证新增路径，不验证相关旧路径；
   - 把未执行的人工测试写成通过；
   - 把已有 TODO、无效旧实现或重复路径留在主流程后声称完成；
   - 用永久 fallback 掩盖 API/线程/资源问题；
   - 只改调用方而不修复错误抽象。

======================================================================
二、必须先阅读的内容：先理解基线和参考源码
======================================================================

在任何代码修改前，必须按以下顺序阅读并输出“基线确认报告”。

1. 工程规则和主设计

   - 根目录 `AGENTS.md`；
   - `docs/14-screenshot-window-detection-design.md` 全文；
   - `docs/08-screenshot-mvp-tasklist.md`；
   - `docs/09-screenshot-mvp-verification.md`；
   - `docs/10-screenshot-pipeline-self-audit.md`；
   - `docs/11-screenshot-fullflow-ui-refactor-tasklist.md`；
   - `docs/12-screenshot-refactor-agent-prompt.md`；
   - `docs/13-screenshot-refactor-verification.md`。

2. 当前实现和测试

   至少阅读并梳理：

   - `src-tauri/src/capture/`；
   - `src-tauri/src/platform/windows/capture/`；
   - overlay、session、monitor、renderer、D2D、D3D11/WGC/DXGI 相关代码；
   - 输入消息、F5、Esc、Enter、鼠标捕获和窗口销毁路径；
   - 现有单元测试、GPU 视觉回归、Windows 探针、构建脚本和 Cargo features。

   必须确认：

   - 当前 overlay HWND 的创建线程、样式、焦点和消息泵；
   - 当前截图纹理、冻结帧和坐标系；
   - 当前鼠标按下/移动/释放如何修改 selection；
   - 当前是否已有按下即创建零尺寸选区的错误路径；
   - 当前窗口销毁、Esc、设备移除和重复 F5 的清理顺序；
   - 当前 DWM/D3D11/DirectComposition 调用分别在哪个线程；
   - 当前测试基线、已知失败项和工作区未提交修改。

3. 参考项目说明和真实源码

   先阅读：

   - `refer/shot-refer/项目介绍.md`；

   然后只读深度研读与本任务直接相关的源码：

   - `refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/window.rs`；
   - `refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/spatial.rs`；
   - `refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/geometry.rs`；
   - `refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/uia.rs`；
   - `refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/uia/cache.rs`；
   - `refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/msaa.rs`；
   - `refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/mod.rs`；
   - `refer/snow-apps/snow_shot/src/presentation/overlay/screenshotoverlayinputhandler.cpp`；
   - `refer/snow-apps/snow_shot/src/presentation/core/screenshotintelligentselectionmodel.cpp`；
   - `refer/snow-apps/snow_shot/src/presentation/capture/screenshotcapturecoordinator.cpp`；
   - `refer/snow-apps/snow_shot/src/presentation/capture/screenshotcaptureworker.cpp`；
   - `refer/snow-apps/snow_shot/src/platform/windows/windowchrome.cpp`；
   - `refer/shot-refer/Crisp-main/src/WindowPick.cpp`；
   - `refer/shot-refer/Crisp-main/src/OverlayInput.cpp`；
   - `refer/shot-refer/Crisp-main/src/OverlayAdjust.cpp`；
   - `refer/shot-refer/Crisp-main/src/OverlayPaint.cpp`；
   - `refer/shot-refer/Crisp-main/tests/TestWindowPick.cpp`；
   - `refer/shot-refer/meazure-master/src/meazure/tools/WindowTool.cpp`。

   必须分别写清：

   - 哪些参考可直接吸收为架构原则；
   - 哪些只适合作为测试/交互参考；
   - 哪些实现不能照搬到 SnapClip；
   - 为什么 GDI、同步 `WindowFromPoint`、同步 `EnumChildWindows`、UIA 全树遍历不能进入 v1 热路径。

   参考项目目录只读，不得修改、重命名、删除或直接复制代码到 SnapClip。

4. 基线确认报告

   阅读完成后，先输出一份简短但有证据的报告，至少包含：

   - 当前截图捕获链路和线程/窗口所有权；
   - 当前坐标系、DPI awareness 和多显示器处理；
   - 当前鼠标状态机和选区生命周期；
   - 当前性能/资源指标和测试结果；
   - 参考项目的可吸收结论及不采用项；
   - 本任务的根因、影响范围和目标架构；
   - 拟修改、新增和明确不修改的文件；
   - 工作区已有改动及其归属。

   没有完成上述确认，不得修改核心 overlay、窗口检测或渲染代码。

======================================================================
三、Git、并行和工作区规则
======================================================================

1. 保护用户已有改动

   - 开始前执行 `git status --short`、`git branch --show-current`、`git remote -v`。
   - 不得使用 `git reset --hard`、`git checkout --`、`git clean -fd` 或其他破坏性命令。
   - 不得覆盖、删除或混入与本任务无关的用户改动。
   - 每个阶段提交前检查 staged diff，只提交本阶段负责的文件。

2. 每阶段提交并推送

   每个阶段的质量门禁全部通过后，立即执行：

   - 查看 `git diff --check`；
   - 查看 `git status` 和 staged diff；
   - 创建独立 commit，commit message 必须包含阶段编号，例如 `feat(capture): implement window snapshot phase 2`；
   - 确认当前分支和远程仓库正确；
   - `git push` 推送本阶段 commit；
   - 阶段报告记录 commit id、分支、远程和 push 输出摘要。

   不得把多个阶段攒到一次提交。不得强制 push、改写远程历史或提交参考目录、密钥、临时日志、构建产物。

   push 失败时：保留本地 commit，记录完整原因；可继续执行不依赖 push 的本地工作，但不能谎称已推送。若需要用户处理认证或远程分支问题，只请求最小必要操作。

3. 允许并行，但必须遵守依赖

   - Phase 0 必须串行完成。
   - Phase 1 完成 DPI/Win32 适配和数据契约后，Phase 2 的快照模型与 Phase 3 的纯几何/命中测试可在不修改同一核心文件的前提下并行。
   - 输入状态机和 overlay 接入必须等待快照接口、坐标契约和会话状态契约冻结。
   - worker 生命周期、排除集合和验证路径必须等待快照 epoch/identity 结构稳定。
   - 性能收口和最终回归必须等待全部实现阶段合并。

   每个并行任务必须提前声明：输入接口、输出接口、负责文件、测试命令和提交边界。不能通过并行绕过质量门禁。

   并行任务的 Git 协作规则：

   - 不允许多个 Agent 同时在同一个工作分支上修改、提交或推送；每个并行阶段使用独立分支（例如 `agent/phase-2`、`agent/phase-3`）或独立 worktree；
   - 每个阶段仍必须在自己的分支上完成质量门禁、创建阶段 commit 并推送该分支；阶段报告记录分支和 commit；
   - 依赖阶段开始前，由协调 Agent 按依赖顺序合并已推送的阶段分支，并在合并后重新运行受影响的测试；合并提交不得替代阶段 commit；
   - 合并冲突必须由协调 Agent 基于接口和测试解决，禁止直接覆盖另一阶段改动；解决后要在集成分支重新执行质量门禁；
   - 如果远程仓库策略不允许创建分支，必须退回串行执行，不得让多个 Agent 争抢同一分支。

======================================================================
四、目标架构和契约
======================================================================

最终实现必须符合以下职责边界：

```text
Win32 overlay/message thread
  -> 读取输入、合并最新鼠标位置、维护 PointerGesture 和 WindowSnapshot

Window detection worker
  -> EnumWindows、DWM cloaked/frame bounds、refresh、revalidate、validate

GPU/render path
  -> 冻结 D3D11 texture、遮罩、hover/selection/preview、局部重绘

Application/session
  -> confirmed selection、Settled/edit state、Enter/toolbar confirmation

Tauri/frontend
  -> 只接收低频状态、目标 id、尺寸、错误码和完成事件
```

必须落地以下数据和规则：

```text
WindowIdentity:
  hwnd + process_id + class_name_hash

WindowCandidate:
  identity + screen_bounds + z_order + snapshot_epoch

WindowSnapshot:
  epoch + candidates + MonitorCache + optional spatial index

PointerGesture:
  None
  AutoSnapPreview
  PendingPointer
  ManualDrag
  MoveSelection
  ResizeSelection
```

必须保证：

- `WindowTarget` 只使用虚拟桌面物理坐标，不携带 overlay 本地坐标；
- `geometry.rs` 负责 screen -> local 裁剪转换；
- `WindowSnapshot::hit_test` 和 `nearest_target` 只读缓存；
- 点在矩形内距离为 0，重叠候选按 Z 序选择；矩形外按欧氏距离选择，超出 snap radius 不吸附；
- 采用半开矩形 `[left, right) x [top, bottom)`；
- `WS_EX_TOOLWINDOW` 和 `WS_EX_NOACTIVATE` 不得被武断排除；
- 只排除 `WS_EX_LAYERED && WS_EX_TRANSPARENT` 的真正点击穿透窗口；
- Shell 类名使用精确黑名单；自家 HWND/PID 使用显式 exclusions；
- `DWMWA_EXTENDED_FRAME_BOUNDS` 失败/为空后才能回退 `GetWindowRect`；
- `BoundsChanged` 必须写回 `WindowSnapshot`，不能只改 hover；
- 所有异步回投都检查 `epoch + identity + request_id/confirmation_id`；
- 快照、MonitorCache 和可选空间索引在失效/会话结束时释放；
- UIA/MSAA 仅作为 v2 深选，不能进入 v1 顶层窗口吸附热路径。

======================================================================
五、执行循环和通用质量门禁
======================================================================

对每个阶段、每个任务严格执行：

### A. 修改前建立基线

运行与当前改动相关的单测、集成测试、构建和 Windows 探针。至少记录：

- `cargo test --lib`、`cargo check --all-targets`；
- 前端 typecheck/build；
- F5 到 overlay visible 的阶段耗时；
- snapshot refresh、hit_test、nearest_target、validate 的耗时；
- 50/100/200 个候选窗口的候选数、P50/P95；
- 鼠标快速移动期间 overlay 消息响应和 Present 次数；
- CPU、Private Bytes、Working Set、GPU memory；
- 当前已知失败和无法执行项目。

无法执行的 Windows 真机验证必须写明原因，不得假称通过。

### B. 分析调用链和所有权

编辑前确认每个 HWND、D3D11 resource、DWM 查询、worker、timer、channel 的创建线程、使用线程、所有者、释放顺序和取消语义。特别检查旧 session、旧 snapshot、旧 hover、旧 dwell timer 是否可能泄漏到下一次 F5。

### C. 实施根因修复

优先使用当前项目已有模块和 Windows 原生 API；不为隐藏错误增加 adapter 或兼容层。若实现发现 `docs/14` 与代码事实冲突，先停在当前阶段，写出冲突、证据、推荐修订和影响，再同步修订设计文档并单独提交设计变更，不能默默偏离文档。

### D. 立即补测试

每完成一个逻辑单元，立即补充最接近的单测、Windows 集成测试、GPU 视觉测试或运行时探针。测试应覆盖正常、边界、异常、取消、陈旧结果和资源释放。

### E. 运行验证

按“最具体 -> 全量”运行：

1. 修改模块单元测试；
2. 相关 Windows 集成测试；
3. `cargo test --lib`；
4. `cargo check --all-targets`；
5. `npm run typecheck` 或仓库实际 typecheck 命令；
6. `npm run build`；
7. 必要的 `npx tauri build --bundles nsis`；
8. 真机 F5/鼠标/Esc/Enter/DPI/多显示器探针；
9. 必要时使用 WPR/WPA、PresentMon 和进程计数器。

### F. 阶段报告和提交

阶段报告必须写：完成任务、根因、修改文件、测试命令和结果、性能前后数据、未完成项、风险、commit id、分支和 push 结果。只有门禁全部通过，才能进入下一阶段。

======================================================================
六、分阶段执行清单
======================================================================

-----------------------------------------------------------------------
Phase 0：基线、参考源码审计和契约冻结
-----------------------------------------------------------------------

目标：先知道系统真实行为，冻结实现边界，不在错误假设上编码。

任务：

- 完成“必须先阅读的内容”全部阅读；
- 运行并保存修改前测试、构建、运行时和性能基线；
- 输出参考项目对照表，明确 snow_shot、Crisp、Meazure 的可吸收项和不采用项；
- 盘点当前 HWND、线程、坐标、DPI、snapshot、selection、capture worker 和 renderer 所有权；
- 定义 `WindowIdentity`、`WindowCandidate`、`WindowTarget`、`WindowSnapshot`、`TargetKind`、`HoverValidity`、epoch/request ID 规则；
- 定义 `PointerGesture` 与 `Settled` 的职责边界；
- 定义窗口检测 worker 的请求/结果消息和取消语义；
- 增加基础诊断埋点：snapshot refresh、hit test、nearest target、validate、candidate count、stale result dropped、queue depth；
- 不在本阶段实现自动吸附行为，除非是为契约和测试建立必要的纯函数。

质量门禁：

- 基线可重复或所有差异有解释；
- 参考项目结论来自真实源码，不来自猜测；
- 契约能够覆盖窗口身份、坐标、陈旧结果和资源生命周期；
- 工作区已有改动已分类，不会混入本阶段 commit。

交付：通过后提交并推送 `Phase 0`，记录基线和 commit/push 证据。

-----------------------------------------------------------------------
Phase 1：DPI、几何和 Windows 窗口检测基础设施
-----------------------------------------------------------------------

目标：建立正确的 Windows 坐标、过滤和 DWM 边界读取基础。

任务：

- 在创建 overlay、读取鼠标和枚举窗口前初始化 Per-Monitor V2；按文档顺序实现降级；
- 建立 `monitor_cache.rs` 或等价模块，缓存本次刷新周期的显示器物理矩形；
- 实现 screen/local 转换、跨屏裁剪、负虚拟桌面坐标和半开矩形工具；
- 实现 Win32 FFI 封装：`IsWindow`、`IsWindowVisible`、`IsIconic`、扩展样式、PID、类名 hash、DWM cloaked、DWM frame bounds；
- 采用两阶段过滤：EnumWindows 回调只做廉价检查，回调结束后批量执行 DWM 查询；
- 实现精确 Shell 黑名单和 exclusions；不排除 tool window/no-activate；
- 实现 DWM frame bounds 失败/空时回退 `GetWindowRect`；
- 增加真实窗口夹具，泵送消息并等待合成后测试边界。

质量门禁：

- 普通、最大化、无边框、cloaked、最小化、不可见、layered 点击穿透和单独 transparent 的过滤结果正确；
- 标题栏包含在顶层 frame，隐形 resize border 不被错误扩张；
- 负坐标和混合 DPI 坐标测试通过；
- 没有在 EnumWindows 回调中同步执行 DWM；
- 所有 Windows 资源和错误路径可清理。

交付：通过后提交并推送 `Phase 1`。

-----------------------------------------------------------------------
Phase 2：WindowSnapshot、命中算法和生命周期
-----------------------------------------------------------------------

目标：把检测结果变为可在 overlay 热路径安全使用的快照。

任务：

- 实现快照构建、epoch 递增、candidate Z 序和身份保存；
- 实现 `WindowSnapshot::hit_test`；
- 实现 `nearest_target(point, snap_radius)`：矩形内距离为 0，矩形外按欧氏距离，距离相同按 Z 序；
- 实现 `apply_candidate_update`，按 epoch + identity 更新单个候选；
- 默认使用 Vec 线性扫描；只有基准证明热点时才实现可选 R-tree；
- 如果启用 R-tree，保留 cache index/Z 序，空间命中后执行二次半开区间检查和 Z 序裁决；
- 实现 snapshot、MonitorCache、索引的 replace/release，禁止旧 epoch 继续命中；
- 增加快照缓存命中、最近距离、重叠 Z 序、边界、空矩形、索引释放和 stale epoch 测试。

质量门禁：

- `WM_MOUSEMOVE` 可以只使用快照，不触发 EnumWindows/DWM；
- 重叠窗口始终选择最顶层；
- 远离候选不产生目标；
- 快照失效后旧对象无法返回有效目标；
- ≤16 候选线性扫描有基准证据，未证明前不预置 R-tree。

交付：通过后提交并推送 `Phase 2`。

-----------------------------------------------------------------------
Phase 3：PointerGesture 和停稳自动吸附预览
-----------------------------------------------------------------------

目标：替换按下即改选区的错误流程，实现“停稳预览、拖拽让位、显式确认”。

任务：

- 重构 overlay 输入处理为显式 `PointerGesture`；
- PointerDown 命中手柄 -> Resize，命中选区内部 -> Move，否则只记录 PendingPointer；
- PointerMove 无按键时更新 hover 并重置 dwell generation；
- dwell 初始 120ms，到期只调用快照 `nearest_target`；
- 有目标时创建/替换 `AutoSnapPreview`，预览不得覆盖已确认 selection；
- PendingPointer 位移平方达到系统拖拽阈值时清除预览并进入 ManualDrag；
- PointerUp 不确认 AutoSnapPreview；ManualDrag/Move/Resize 按既有选区语义结束；
- 离开 snap radius 时清除预览；移动到另一候选后重新 dwell；
- 保证旧 dwell generation、快照 epoch 和新鼠标位置不匹配时不产生预览；
- 增加输入状态机纯单测和 overlay 真机交互日志。

质量门禁：

- 按下不会创建零尺寸选区；
- 轻微抖动不会误进入手动拖拽；
- 光标停稳会产生最近窗口预览，但不会自动确认；
- 鼠标释放不确认吸附；
- Enter/工具栏确认入口清晰且不会在鼠标线程同步验证；
- 手动框选、移动和缩放不被自动吸附破坏。

交付：通过后提交并推送 `Phase 3`。

-----------------------------------------------------------------------
Phase 4：检测 worker、目标验证和过期处理
-----------------------------------------------------------------------

目标：处理窗口移动、关闭、cloaked、HWND 重用、陈旧结果和确认时序。

任务：

- 建立有界检测 worker mailbox；hover 重验证只保留最新请求；
- 定时器初始约 250ms，只投递当前 hover 的 HWND/identity/epoch；
- worker 执行单窗口 revalidate：可见性、最小化、cloaked、PID/class hash、frame bounds；
- 回传 `Valid`、`BoundsChanged`、`Invalid`；overlay 校验 epoch + identity 后处理；
- `BoundsChanged` 必须写回 snapshot，再更新 hover/preview；
- `Invalid` 刷新快照并以当前鼠标重命中一次；仍失败时清除预览并恢复确认前选区；
- Enter/toolbar 将 `{target, epoch, confirmation_id}` 投递 worker，成功回投后才 `snap_to`；
- 新请求先取消/使旧请求失效；陈旧结果不可覆盖新预览；
- 会话关闭、Esc、重复 F5、显示器变化和 worker shutdown 统一清理；
- 对无响应 provider 保留 bounded timeout/quarantine 原则，但 v1 不引入 UIA/MSAA。

质量门禁：

- 窗口移动后 hover/预览跟随新边界，不跳回旧位置；
- 窗口关闭或 HWND 重用不会误吸旧矩形；
- overlay 消息循环不执行同步 DWM；
- 快速移动和连续刷新时陈旧结果被丢弃；
- 关闭会话不会等待无限期 worker 或泄漏线程/资源。

交付：通过后提交并推送 `Phase 4`。

-----------------------------------------------------------------------
Phase 5：Settled 编辑态、overlay 渲染和排除集成
-----------------------------------------------------------------------

目标：让预览、确认后的编辑态和捕获排除边界清晰，消除旧选区/旧 hover 干扰。

任务：

- 确认吸附或手动框选后进入 Settled/edit state；
- Settled 状态关闭 window hover、dwell 和全屏十字线，只保留选区、手柄和工具栏；
- 选区仍支持 Move/Resize，不能被旧 preview 或新鼠标移动覆盖；
- 新会话创建前释放旧 selection preview、hover、dwell generation、snapshot 和 GPU 资源；
- 自家 overlay、工具栏、颜色面板、Tauri 主窗口等加入显式 excluded HWND/PID；
- capture native session 的 backend/exclusion 列表变化时在 worker 中重建 session；
- affinity 只用于捕获排除，不能代替 detection exclusions；
- `DwmFlush` 只在捕获前隐藏/affinity 状态同步时使用，不放入 hover 热路径；
- 更新 D2D/GPU hover、selection、preview 的绘制层，避免预览和 Settled 状态混画；
- 增加旧选区闪现、重复 F5、Settled 编辑和 overlay 不进入截图的回归测试。

质量门禁：

- 第一次 Esc 后再次 F5 不出现旧选区闪现；
- 已确认选区不会被 hover 或自动吸附偷偷改写；
- overlay、工具栏、颜色面板不会进入截图或窗口吸附候选；
- Settled 状态可移动/缩放，Esc 仍可取消当前会话；
- 捕获层 exclusions 和检测层 exclusions 均有独立测试。

交付：通过后提交并推送 `Phase 5`。

-----------------------------------------------------------------------
Phase 6：性能基线、诊断和系统回归
-----------------------------------------------------------------------

目标：证明实现满足性能和资源约束，而不是只凭主观感觉。

任务：

- 记录并比较修改前后 snapshot refresh、hit_test、nearest_target、validate、worker queue、陈旧结果丢弃；
- 在 50/100/200 个候选窗口下建立线性扫描基线；只有测得热点才考虑索引；
- 测量鼠标快速移动 10 秒的 CPU、overlay 消息延迟、Present 次数和内存；
- 测量 F5 到 overlay visible 的各阶段耗时；
- 检查 Private Bytes、Working Set、GPU Dedicated/Shared Memory 和快照释放后的回落；
- 检查 DWM 调用是否只发生在 worker；
- 增加 diagnostics 开关，默认不洪泛日志；
- 使用 WPR/WPA、PresentMon 或等价工具完成 Windows 真机数据采集；
- 修复由真实数据证明的瓶颈，不为未测量问题预先复杂化。

质量门禁：

- 缓存命中/最近距离查询满足文档初始预算或有真实解释；
- 鼠标移动不阻塞 overlay 消息循环；
- snapshot/index 释放后内存不持续增长；
- 所有性能结论有命令、环境、数据和采样条件；
- 未覆盖风险被明确记录。

交付：通过后提交并推送 `Phase 6`。

-----------------------------------------------------------------------
Phase 7：完整功能、构建和最终验收
-----------------------------------------------------------------------

目标：完成文档 §12 全部适用测试并收口实现。

任务：

- 运行 Rust 单测、Windows 集成测试、GPU 回归、前端检查、Tauri 构建和 NSIS 构建；
- 真机验证普通/最大化/无边框窗口、浏览器、资源管理器、layered overlay、点击穿透窗口、多显示器、负坐标、混合 DPI、高对比度；
- 验证 F5、Esc、Enter、手动拖拽、停稳吸附、预览替换、窗口移动/关闭、重复取消/确认循环；
- 验证截图导出、放大镜、取色、标注和剪贴板等相关旧功能；
- 检查没有残留无效旧接口、重复实现、永久兼容层或已知 TODO；
- 将实现证据补充到验证文档，必要时更新 `docs/14` 的实际完成状态；
- 整理最终文件清单、性能对比和未覆盖风险。

质量门禁：

- 所有适用任务有测试或真实验收证据；
- 新功能和相关旧功能均正常；
- 代码、测试、文档和指标相互一致；
- 最终 commit 只包含本功能相关改动，并已推送 GitHub。

交付：通过后提交并推送 `Phase 7`，然后输出最终报告。

======================================================================
七、必须覆盖的测试场景
======================================================================

纯单元测试：

- 半开矩形边界；
- 重叠窗口按 Z 序命中；
- 点内距离为 0；
- 矩形外最近距离和 snap radius；
- 负虚拟桌面坐标和跨显示器裁剪；
- 空/退化矩形；
- identity/PID/class hash 失配；
- HWND 关闭、cloaked、最小化、不可见；
- layered + transparent 与单独 transparent；
- dwell generation/epoch 失效；
- PendingPointer -> ManualDrag；
- PointerUp 不确认吸附；
- Enter/toolbar 确认；
- 确认失败恢复旧选区；
- BoundsChanged 写回 snapshot；
- stale request/result 丢弃；
- snapshot/index/MonitorCache release；
- Settled 状态禁止 hover/preview 改写。

Windows 集成测试：

- 创建测试窗口并等待 DWM 合成后读取 frame bounds；
- 普通、最大化、无边框窗口；
- tool window/no-activate 窗口仍可按策略吸附；
- layered 点击穿透 overlay 被排除；
- overlay 自身不会命中；
- 窗口移动、缩放、关闭、HWND 重用；
- DPI 初始化和显示器缩放/拓扑变化；
- 多显示器负坐标；
- worker DWM 调用不阻塞 overlay；
- 重复 F5/Esc/Enter 和窗口销毁。

人工验收：

1. 启动应用，确认主窗口和 overlay 不 hung。
2. 按 F5，确认冻结帧和窗口检测快照建立。
3. 鼠标停在窗口内部，等待 dwell，确认出现吸附预览。
4. 移到另一窗口，确认预览替换；移到空白处，确认预览清除。
5. 按下后轻微抖动并释放，确认不自动确认吸附。
6. 按下并超过拖拽阈值移动，确认进入自由框选。
7. 按 Enter，确认目标验证后提交正确窗口边界。
8. 窗口移动/关闭后再次停稳，确认不会吸附旧边界。
9. 确认后移动/缩放选区，确认 hover/preview 不再干扰。
10. Esc 取消，再次 F5，确认没有旧选区或旧 hover 闪现。
11. 在多显示器、负坐标和混合 DPI 环境重复以上流程。
12. 重复至少 20 次确认/取消循环，观察线程、窗口和内存是否泄漏。

======================================================================
八、阻塞处理
======================================================================

遇到失败时：

1. 保存完整日志、复现步骤和失败现场；
2. 判断代码、环境、驱动、权限、DPI、窗口状态、API 能力还是测试假设问题；
3. 至少尝试三条合理的本地排查路径；
4. 不得跳过测试、关闭功能或增加永久 fallback 来掩盖问题；
5. 真正需要用户手工操作时，只请求最小必要信息，并提供明确步骤；
6. 阶段报告写清阻塞原因、已尝试路径、当前影响、下一步和未验收风险；
7. 同一外部阻塞连续三次无法推进且没有可行本地替代时，才可以标记 blocked；不能因为工作量大、测试慢或设计复杂就提前停止。

======================================================================
九、最终报告格式
======================================================================

全部阶段结束时，必须输出：

### 完成结论

- 是否完成 `docs/14` 的全部适用要求；
- 是否存在未完成或 blocked 项；
- 相对基线的关键行为变化。

### 主要架构变化

- 窗口快照、DWM 边界和过滤；
- MonitorCache、坐标和 DPI；
- PointerGesture、AutoSnapPreview、Settled；
- worker、取消、epoch、request ID；
- exclusions 和捕获生命周期；
- 性能和资源释放。

### 阶段提交和推送

列出每个阶段的：

- 阶段编号；
- commit id；
- 分支；
- push 结果；
- 若失败，记录真实原因。

### 验证证据

- 执行过的测试和命令；
- Windows 真机步骤及结果；
- 关键日志和 GPU/窗口回归证据；
- 性能和内存前后数据。

### 未覆盖风险

只列真实风险。不要用“后续优化”掩盖尚未完成的硬性任务。

### 文件清单

列出新增、修改、删除的关键文件及职责变化。

只有在“根因解决 + 新功能正确 + 相关旧功能回归 + 性能有数据 + 每阶段已提交并推送 + 所有适用质量门禁通过”同时满足时，才可以写“任务完成”。
```
