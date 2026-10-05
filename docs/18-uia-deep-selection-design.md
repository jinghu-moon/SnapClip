# SnapClip 截图子控件深选（v2）设计方案

> 文档状态：设计定稿（待实现）
>
> 上游：`docs/14-screenshot-window-detection-design.md` §11「v2（预留）：子控件深选」
>
> 参考源码：`refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/{uia.rs,uia/cache.rs,msaa.rs}`、
> `refer/snow-apps/snow_shot/src/presentation/selector/screenshotselectorcoordinator.cpp`
>
> 适用平台：Windows 10/11 x64，Per-Monitor-V2 DPI awareness

## 1. 范围与不变量

v2 在 v1 整窗吸附之上增加**子控件深选**：光标停稳后，把目标从「顶层窗口外框」细化到「客户区」或
「最深的可交互 UI 元素」。以下不变量是硬约束，实现与测试都必须能证明它们：

1. **v1 路径不被触碰**：`capture/window_detection/**` 与 detection worker 不出现 COM/UIA/MSAA 类型；
   深选失败、超预算、被取消时，hover/预览**回落到 v1 的整窗目标**，而不是无结果。
2. **对象不跨线程**：所有 UIA/MSAA/COM 对象只在 refinement 线程创建、使用、释放；跨线程只传
   可序列化数据（矩形、`StopReason`、epoch/request id）。
3. **不缓存无界 UI 树**：按需展开一层 sibling，缓存已获得的批次；失败/超时节点记录停止原因，
   后续查询可继续而不重复获取同一批次。
4. **有界**：每次查询有总预算、单次 provider 调用预算、取消检查、最大深度/节点数/返回矩形数；
   预算耗尽时发布**已验证的部分路径**并带 `stop_reason`，绝不无限等待。
5. **提交语义不变**：确认仍然只在检测 worker 校验通过后 `snap_to(rect)`；v2 只改变“目标矩形从哪来”。

## 2. 触发与调度（对齐 snow_shot coordinator）

参考实现的调度语义（`screenshotselectorcoordinator.cpp`）逐条复刻：

| 参考行为 | v2 规则 |
| --- | --- |
| `QTimer` 单次精确计时，`remaining = 80 - (now - targetChangedAt)` | 光标停稳 **80 ms**（`REFINEMENT_DWELL_MS`）后才提交 refinement；目标变化重新计时 |
| `if (!ready \|\| hitTestInFlight \|\| hasPendingHitTestPoint \|\| refinementSubmitted)` 守卫 | **单飞**：同一时刻最多一个 refinement 在飞；有未消费的最新点时不提交 |
| `invalidateRefinement()` + `cancelRefinement()` | 光标移动、按下、拖拽、快照 epoch 变化、显示器变化、会话结束 → 使在途请求失效 |
| 「在完整缓存路径内移动不得触碰 refinement worker」 | 命中已缓存的深选路径时**不触发** refinement，直接复用 |
| `emit refinementReady(rects, displayId, replacePath)` | 结果发布携带 `replace_path` 语义：新路径要么替换、要么仅扩展 |

## 3. 预算、降级与隔离

对齐 `snow-ui-selector/src/query.rs`：

| 档位 | 总预算 | 单次 provider 调用上限 | 发布间隔 | 超时重试 |
| --- | --- | --- | --- | --- |
| 前台（v1 整窗命中） | 168 ms | 168 ms | 无 | 否 |
| refinement（v2 深选） | **1500 ms** | **500 ms** | **32 ms** | 是 |

`StopReason`（沿用参考的枚举语义）：`Complete` / `BudgetExhausted` / `TraversalLimit` /
`ProviderTimeout` / `ProviderFailure` / `Cancelled` / `Unsupported`。

降级规则：

- 任何非 `Complete` 的结果都**先发布已验证的部分路径**，并由 overlay 决定显示层级；
- 完全没有可用路径 → 回落到 v1 整窗目标；
- provider 无响应 → 临时 quarantine 到下一次快照刷新（不在本次会话内反复重试）；
- 结构容器（`Pane`/`Group` 与父节点同边界）**允许回溯到重叠 sibling**，不得用 control-type
  黑名单粗暴删除，否则 Chromium 等应用的真实内容会被结构分支吞掉。

## 4. 数据契约

```rust
enum TargetKind {
    TopLevelWindowFrame,   // v1
    ClientArea,            // v2：窗口客户区
    UiElement,             // v2：最深的可交互元素
}

struct DeepTarget {
    window: WindowIdentity,       // 恒为 v1 快照里的窗口身份
    kind: TargetKind,
    screen_bounds: Rect,          // 虚拟桌面物理像素（与 v1 同一坐标系）
    path: Vec<Rect>,              // 自顶向下；path[0] == 窗口外框，末项 == screen_bounds
    stop_reason: StopReason,
}

enum StopReason { Complete, BudgetExhausted, TraversalLimit, ProviderTimeout,
                  ProviderFailure, Cancelled, Unsupported }
```

`DeepTarget` 不携带显示器本地坐标，也不携带任何 COM 句柄。

## 5. 线程模型

```text
overlay 线程       : 80ms 停稳计时、命中缓存路径、绘制层级、单击确认（提交矩形）
detection worker   : v1（EnumWindows/DWM/validate）—— v2 不新增任何调用
refinement worker  : 独立 COM apartment；UIA（主）/MSAA（回退）；预算、取消、quarantine
结果传递           : refinement -> overlay：PostMessage + epoch/request id 校验
```

refinement worker 的邮箱是**容量 1 的最新点**模型（与 detection worker 同构），因此
快速移动不会堆积请求。

## 6. 明确不做

- 不把 UIA/MSAA 放进 v1 热路径或 `WM_MOUSEMOVE` 同步路径；
- 不在 overlay 线程创建 COM 对象；
- 不为 v2 引入空间索引（v1 基准已证明线性扫描足够；v2 的瓶颈在 provider 调用，不在几何）；
- 不缓存完整无界 UI 树，不为「更聪明」而预取整棵子树。

## 7. 分期与验收

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| v2-P0 | 契约（`TargetKind` 扩展、`DeepTarget`、`StopReason`）与**纯净调度状态机**（80ms 停稳、单飞、最新点合并、epoch 失效、缓存路径复用） | 纯单测覆盖全部分支；不需要 COM |
| v2-P1 | refinement worker + 容量 1 邮箱 + 取消/超时；用假 provider 打通线程语义 | 线程/取消/超时/陈旧丢弃单测 |
| v2-P2 | UIA provider：COM apartment、按需展开、批次缓存、预算与 `stop_reason`、quarantine | 真机 UIA 探针（记事本/浏览器/资源管理器） |
| v2-P3 | MSAA 回退 provider | MSAA 探针 + 回退顺序 |
| v2-P4 | overlay 接入：hover 显示层级、单击确认提交最深目标、Esc/移动取消 | 真机交互回归 + v1 回归（确认整窗仍可用） |
| v2-P5 | 性能与验收：refinement 延迟分布、quarantine 命中率、与 v1 的 CPU 对比 | 数据 + 报告 |

## 8. 测试计划要点

- 调度：停稳 80ms 才提交；连续移动只保留最新点；单飞期间的新点排队而非并发提交；
  命中缓存路径不触发 refinement；epoch 变化丢弃在途结果。
- 预算：总预算耗尽发布部分路径 + `BudgetExhausted`；单次调用超时 → `ProviderTimeout` 并 quarantine。
- 遍历：深度/节点/矩形上限触发 `TraversalLimit`；结构容器回溯不产生重复矩形。
- 隔离：`capture/window_detection/**` 编译期不含 COM/UIA 引用（可用 `rg` 静态检查断言）；
  v2 失败时 hover/预览仍显示 v1 整窗目标。

## 9. 实现进度

### v2-P0（已提交 `bc52233`）

`TargetKind` 增加 `ClientArea`/`UiElement`（`is_refined()`），新增
`capture/window_detection/deep.rs`：`StopReason`、`DeepTarget`、`RefinementScheduler`、
`QueryControl`、`DeepSelectionProvider`、`UnsupportedDeepSelection`。
调度规则全部有单测：80 ms 停稳、单飞、最新点合并、epoch 失效、完整缓存路径复用、
部分路径不复用、失败释放单飞槽。

### v2-P1（本次提交）

`platform/windows/capture/refinement_worker.rs`：独立线程 + 容量 1 最新点邮箱 +
共享 request gate + 协作式取消（`QueryControl::is_cancelled`），并接入 overlay：
hover 变化喂调度器 → 80 ms 一次性定时器 → 提交 → 结果回投 → 有路径则替换预览矩形，
无路径则保持 v1 整窗帧。

**P1 中发现并修复的两个根因缺陷**（都由实机诊断日志暴露，不是测试问题）：

| 缺陷 | 现象 | 根因 | 修复 |
| --- | --- | --- | --- |
| refinement 线程立即退出 | 提交的 job 永远不执行，日志里既无 worker 行也无结果行 | 用「gate 没有 latest」判定 shutdown，而线程启动时本来就没有请求 → 启动即 `return` | 增加显式 `shutdown: AtomicBool`；`retire()` 只做取消，不再兼职关闭 |
| dwell 定时器反复触发 | 一次停稳后每 80 ms 复投一次 tick（日志刷屏、空转唤醒消息循环） | `SetTimer` 是**周期性**定时器，而 120 ms/80 ms dwell 的注释与设计都是 **one-shot**，却只在少数路径 `KillTimer` | 到期即 `KillTimer`（`on_dwell` / `on_refinement_tick`），新 hover 再重新 arm |

修复后的实机证据（`.tmp-v2-probe.ps1`，4K/DPI144/单屏 WGC）：

```text
[win-detect] refinement submit hwnd=9963044 point=(2850,1050) epoch=1
[win-detect] refinement hwnd=9963044 point=(2850,1050) elapsed_us=0 reason=Unsupported  # worker 线程执行
[win-detect] refinement empty reason=Unsupported                                        # 单飞槽被释放
[win-detect] hover hwnd=9963044 z=1 bounds=(992,307)->(2912,1522)                      # v1 帧不变
[win-detect] auto-snap preview hwnd=9963044 epoch=1 local=(992,307)->(2912,1522)
```

恰好一次提交、一次 worker 执行、一次结果消费，无 tick 刷屏、无错误 —— v2 管线已真实运行，
且在 provider 缺失时**严格降级到 v1**。

### v2-P2 的测试对象

按产品建议，UIA provider 的初始验收对象定为 **Windows 资源管理器**（`CabinetWClass`）：
它暴露层次丰富的 UIA 树（导航窗格 `SysTreeView32`、文件列表 `DirectUIHWND`、命令栏、
地址栏），能同时验证「结构性容器回溯」与「最深可交互元素」两条规则。
当前探针用的 `FindWindowW("CabinetWClass")` 未能取到窗口句柄（返回 0），
P2 改为**枚举顶层窗口 + 过滤 `explorer.exe` 进程**并等待窗口出现，失败才回退到屏幕中央。

---

## 10. 预览矩形变化动画（对齐 snow_shot）

### 10.1 问题

深选开启后，截图框优先贴合光标下**最近的一个控件**。鼠标横穿一个窗口时会连续经过不同的控件，
目标矩形随之忽大忽小；如果直接把目标矩形画出来，高亮框会以鼠标事件频率跳动，观感上是抖动而非选择。

### 10.2 参考实现（真实源码）

`refer/snow-apps/snow_shot/include/snow_shot/presentation/screenshotsmartselectiontransition.h`
与 `src/presentation/core/screenshotsmartselectiontransition.cpp` 定义了
`ScreenshotSmartSelectionTransition`：

| 参考行为 | 规格 |
| --- | --- |
| 时长 | `kDurationMs = 101` |
| 缓动 | `kEasingCurve = QEasingCurve::OutQuad` |
| 目标变化 | `stop()` → `setStartValue(m_displayedSelection)` → `setEndValue(新目标)` → `start()`：从**当前显示中的矩形**出发，因此打断重定向是平滑的 |
| 首次出现 | `!m_hasPresentedSmartSelection` → `presentDirectly()`，**不播动画**（避免从“无”放大） |
| 目标未变 | `selection == m_targetSelection` → 返回 `false`，不触发重绘 |
| 无智能框选 | `smartFraming == false` 或选区无效/为空 → `presentDirectly()` |
| 关闭动画 | `setEnabled(false)` 时若动画在跑 → 立即 `presentDirectly(m_targetSelection)` |

### 10.3 SnapClip 的落地规格

| 项目 | 决定 |
| --- | --- |
| 动画对象 | **预览/悬停矩形**（`preview_bounds` / `hover_bounds`，显示器本地像素）。已确认选区不参与动画：选区是用户（或确认动作）明确设定的几何，不做平滑 |
| 时长/缓动 | 与参考一致：101 ms、OutQuad（`f(t) = 1 - (1-t)²`） |
| 时钟 | overlay 既有的 15 ms 合并渲染 tick；动画未结束就保持 dirty，结束后停止请求重绘（不引入额外定时器） |
| 起点 | 当前**显示中**的矩形；无显示中矩形时直接呈现（首次出现不播动画） |
| 打断 | 新的目标矩形到达时直接以当前显示值为起点重新计时 |
| 清空 | 预览消失（离开吸附半径、按下、确认、会话结束）→ **直接清除**，不做缩小动画 |
| 关闭条件 | 会话结束 / 预览清空 / epoch 变化 → 停止动画并丢弃状态 |

### 10.4 纯函数与测试

`capture/window_detection/transition.rs::RectTransition`：保存 `from`/`to`/`started_at`/`duration`，
`value_at(now)` 返回插值矩形，`is_running(now)` 判断是否还在动画中；
不持有定时器、不依赖 Win32，因此可在单测中按任意时间点取值。

必须覆盖：`t=0` 等于起点、`t=duration` 等于终点、OutQuad 在中点的取值、打断后以“显示中值”为起点、
首次出现直接呈现（`from == None`）、`now` 早于起点与晚于终点的边界、以及“目标未变不产生变化”。

### 10.5 实现与实机证据

实现：`capture/window_detection/transition.rs`（纯取值函数 + 7 项单测，含打断、首次直接呈现、
同值不动画）+ overlay 接线（`preview_rect`/`preview_target`/`RectTransition`，由既有 15 ms 合并
渲染 tick 推进；动画未结束就保持 dirty）。**动画只影响绘制**：确认提交用的仍是 gesture 里的
真实目标矩形，因此缓动不可能改变提交结果。

实机证据（`.tmp-anim-probe.ps1`，两个尺寸差异明显的夹具窗口，4K/DPI144）：

```text
# 小窗 → 大窗：目标 (2636,1050)->(3664,1819)
preview anim=(2131,826)->(2711,1258) target=(2636,1050)->(3664,1819)
preview anim=(2287,895)->(3004,1431) target=(2636,1050)->(3664,1819)
preview anim=(2419,954)->(3254,1578) target=(2636,1050)->(3664,1819)
preview anim=(2519,998)->(3443,1689) target=(2636,1050)->(3664,1819)
preview anim=(2591,1030)->(3579,1769) target=(2636,1050)->(3664,1819)
preview anim=(2629,1047)->(3651,1811) target=(2636,1050)->(3664,1819)   # 收敛

# 大窗 → 小窗：注意第一帧从 (2456,970) 起，而不是上一条的终点——
# 说明新目标是从“当前显示中的矩形”接管，正是参考实现的打断语义
preview anim=(2456,970)->(3324,1619) target=(1961,750)->(2389,1069)
preview anim=(2294,898)->(3018,1439) target=(1961,750)->(2389,1069)
preview anim=(2163,840)->(2771,1294) target=(1961,750)->(2389,1069)
preview anim=(2068,798)->(2591,1188) target=(1961,750)->(2389,1069)
preview anim=(2001,768)->(2465,1114) target=(1961,750)->(2389,1069)
preview anim=(1966,752)->(2399,1075) target=(1961,750)->(2389,1069)
```

两个方向都是单调收敛的 ~6 帧缓动（≈101 ms，15 ms 合并 tick），无跳变；
v1 的 `auto-snap preview` 目标行保持不变，错误 0。

---

## 11. v2-P2 进展（进行中）

| 已完成 | 内容 |
| --- | --- |
| 依赖 | `Cargo.toml` 的 `windows` features 增加 `Win32_UI_Accessibility`（`IUIAutomation` 所在地） |
| 纯遍历策略 | `capture/window_detection/uia.rs`：`WalkNode`/`WalkBudget`/`WalkOutcome`、`deepest_child_at`（点内最小矩形胜出）、`is_descendable`（越界/离屏/退化子节点拒绝）、`is_structural_container`（同边界容器继续下钻）、路径去重与 `MAX_PATH_LEN` 上限；7 项单测 |

策略层的三条关键规则都有测试固定：

1. **点内最小矩形胜出** —— 控件优先于包住它的容器；
2. **结构性容器必须下钻**（与父节点同边界的 `Pane`/`Group` 不作为最终答案），这是 Chromium 系应用内容不被结构分支吞掉的前提；
3. **离屏/退化/父框外子节点一律拒绝**，预算耗尽或路径超长 → `TraversalLimit`。

### v2-P2 provider（本次提交）

`platform/windows/capture/uia_provider.rs`：

- **COM 在 refinement 线程上惰性初始化**：provider 由 worker 通过 **factory** 构造
  （`ProviderFactory`），而不是把值搬过线程——`IUIAutomation` 不是 `Send`，且设计要求
  apartment 对象在其使用线程上创建；
- `ElementFromHandle(窗口)` 起手（**不**回退到 `ElementFromPoint`：查询是「关于这个窗口」的，
  用点命中会回答到另一个窗口的几何，属于凭空捏造），失败即 quarantine 并返回 `Unsupported`；
- `RawViewWalker` 按 §3 的策略自顶向下展开（点内最小矩形胜出、同边界容器继续下钻、
  越界/离屏/退化子节点拒绝），受 `WalkBudget`（4096 节点 / 24 层 / 24 段路径）与
  `QueryControl::is_cancelled` 约束；
- 结果映射为 `DeepTarget`：`path[0]` = 窗口外框，`kind` 在真正下钻后为 `UiElement`，
  否则保持 `TopLevelWindowFrame`（于是 overlay 按 v1 方式渲染）。

真机证据（`.tmp-uia-probe.ps1`，4K/DPI144；资源管理器窗口句柄未能从 shell 进程的
`MainWindowHandle` 取到，故先落在屏幕中央的最大化真实窗口上）：

```text
refinement submit hwnd=5967024 point=(1257,173) epoch=1
refinement hwnd=5967024 elapsed_us=22118 reason=Complete          # 首次查询 22 ms（含 COM 初始化）
refinement published hwnd=5967024 bounds=(0,47)->(3840,2088) depth=4 reason=Complete
```

`depth=4` 说明路径是「窗口外框 → … → 元素」四层；发布矩形 `(0,47)` 比窗口帧 `(0,0)` 内缩 47px，
即解析到了窗口内的内容区而不是回退整窗；`reason=Complete`。第二次停稳**没有**再触发查询，
说明「完整缓存路径内移动不得触碰 refinement worker」在生产路径上生效。

### 待完成

| 项目 | 说明 |
| --- | --- |
| 资源管理器专项验收 | 探针未能从 `explorer.exe` 的 `MainWindowHandle` 取到 shell 窗口句柄（返回 0 或桌面窗口）；需要改为 `EnumWindows` + 类名 `CabinetWClass` 枚举，再分别验证导航窗格 / 文件列表 / 命令栏三个停稳点的 `path` 深度与 `stop_reason` |
| 批次缓存 | 当前每次查询重新下钻；§3 要求的「按需展开一层 sibling + 批次缓存」尚未实现，`release()` 目前只清 quarantine |
| v2-P3 MSAA 回退 | 未开始 |
| v2-P4 overlay 层级路径渲染 | 预览矩形已跟随深选结果，但 `path` 的逐层描边未画 |
| v2-P5 性能与验收 | UIA 单次查询 22 ms（首次，含 COM 初始化）已测得；延迟分布与 quarantine 命中率待实测 |

### 实测反馈与根因：窗口内控件「很难触发」

现场报告：应用窗口之间切换时吸附灵敏，但**在资源管理器内部很难触发**控件级吸附。

排查的第一层是**可见性**：overlay 里所有 `refinement …` 行（以及 `hover`/`auto-snap preview`）
都是 verbose 门控的，默认运行看不到任何 `[win-detect]` 输出（日志里只剩 `render session=` 刷屏），
于是「没触发」与「触发了但看不到」无法区分。本次修掉：

| 问题 | 修复 |
| --- | --- |
| `render session=` 每帧打印，几千行淹没日志 | 改为 verbose 门控；会话结束的汇总行承载计数 |
| 无法判断精化是否发生 | 指标新增 `refinement_submitted` / `refinement_published` / `refinement_empty` / `refinement_elapsed_us(last/max)` 并进入每会话汇总行；`RefinementResult` 带回 provider 耗时 |

第二层才是**真正的性能根因**（深调研参考实现后确认）。对比 `snow-ui-selector/src/windows/uia.rs`：

| 参考实现 | 当前实现 | 影响 |
| --- | --- | --- |
| `refresh()` 里 `build_uia_window_cache()` 建**窗口缓存 + 窗口级空间索引**，`release_cache()` 释放 | 无 | — |
| `query()` 先用空间索引在**进程内**命中窗口，再交给 `window.tree.query(...)`（**每窗口一棵 `WindowTree` 缓存**，跨查询复用） | 每次停稳都从窗口根节点**重新下钻** | 同一窗口内反复停稳要重复走完整棵树 |
| 用 `CacheRequest` + `GetCachedChildren`，读 `CachedBoundingRectangle`/`CachedControlType`/`CachedIsOffscreen`（**一次批量取回**） | 每个节点 4 次 `Current*` 跨进程调用 | DirectUI 大树上单次查询变成数千次跨进程调用 |
| `QueryClock` 预算 + `progress` 回调**增量发布**（`QueryControl::refinement` 发布间隔 32 ms） | 只在遍历结束发布 | 慢查询期间用户看不到任何反馈 |

结论：资源管理器的 DirectUI 树让「每次从根重走 + 每节点 4 次跨进程调用」的查询慢到用户早已移动鼠标，
而任何窗口/hover 变化都会 `invalidate_in_flight` 取消在途查询 —— 于是**永远没有结果发布**，
表现就是「窗口内控件很难触发」。

**下一步（v2-P2 收尾）**：按参考实现改造 provider —— ①`CacheRequest` 批量取属性（`Cached*`）；
②每窗口 `WindowTree` 缓存（按快照 epoch 失效）；③遍历中按 32 ms 间隔增量发布已验证的最深元素；
④保留 `WalkBudget` 与取消检查。完成后用资源管理器导航窗格 / 文件列表 / 命令栏三点验收。

### ① 已完成：批量 CacheRequest（本次提交）

`uia_provider.rs` 现在在 refinement 线程上惰性构建一个
`IUIAutomationCacheRequest`（`AddProperty` × 3：`BoundingRectangle` / `ControlType` / `IsOffscreen`，
`SetTreeScope(TreeScope_Children)`），遍历改为：

```text
每层：BuildUpdatedCache(request) ×1  →  GetCachedChildren() ×1  →  逐子节点读 Cached*（进程内）
```

即每层 2 次跨进程调用，替代原先「每节点 4 次 `Current*` + 每个兄弟一次 `GetNextSiblingElement`」。

实测效果（`.tmp-uia-probe.ps1`，同一台机器、同类窗口）：

| | 改造前 | 改造后 |
| --- | --- | --- |
| 路径深度 | 3 | **5** |
| 发布矩形 | `(1336,511)->(2910,1482)`（1574×971，粗区域） | **`(1390,1253)->(1744,1282)`（354×29，控件级）** |
| 查询耗时 | 20–41 ms | 20–66 ms |

结论：单看耗时变化不大（受被测窗口自身的 provider 响应速度限制），但**同样的时间里探到了更深的层级**
并给出了真正的控件级矩形——这正是「每层 2 次调用」换来的检索深度。`refinement_submitted=2 /
published=2 / empty=0`，无错误。

### ②③ 待完成

| 项目 | 说明 |
| --- | --- |
| 每窗口树缓存 | 同一窗口内在不同控件间移动仍需重新下钻；需要按 `WindowTree` 缓存（按快照 epoch 失效） |
| 增量发布（32 ms） | 目前只在遍历结束发布；慢 provider 期间用户看不到反馈 |
| 资源管理器夹具 | 需要 `EnumWindows` + `CabinetWClass` 枚举，才能做导航窗格 / 文件列表 / 命令栏三点验收 |

### ④ 已完成：触发源改为「光标」而非「窗口」

现场数据（产品自测 `npm run tauri dev`，默认日志的会话汇总行）：

```text
window_hit_test_us n=945        hover_target_switch_count=6
refinement_submitted=6  refinement_published=6  refinement_empty=0
refinement_elapsed_us last=10772  max=57977
```

`submitted` 与 `published` 1:1、单次 10–58 ms —— 精化**每次提交都成功**，既没被取消，
provider 也不慢。但 `submitted` 恰好等于**窗口级** `hover_target_switch_count`。

根因：`update_hover` 在「窗口目标未变」时提前返回，把喂调度器的代码一起跳过了，
于是**触发条件是窗口变化，而不是光标位置**。在一个资源管理器窗口内部换控件时窗口没变，
就再也不会查询 —— 深选停在第一次命中的控件上，表现即「窗口之间灵敏、窗口内控件很难触发」。

修复：新增 `drive_refinement(screen, target)`，在**每次光标更新**时都喂调度器，早于
「窗口未变」的重绘短路；由 `RefinementScheduler` 自己决定是否需要 worker（点落在已发布
的完整路径内仍走缓存、不触发查询）。这样：

- 停稳在某个控件上 → 查询并发布控件级矩形；
- 在同一窗口内移到另一个控件并停稳 → 重新查询（新点落在已发布路径之外）；
- 在同一控件内部微动 → 不触发查询，命中缓存。

**产品侧复验（`npm run tauri dev`，默认日志的会话汇总行）——修复生效**：

```text
window_hit_test_us n=1790   hover_target_switch_count=5
refinement_submitted=26  refinement_published=22  refinement_empty=0
refinement_elapsed_us last=43398  max=61331
```

修复前 `submitted == hover_target_switch_count`（=6，即每个窗口只查一次）；修复后
`submitted=26` 对 `hover_target_switch_count=5`，**精化次数约为窗口变化次数的 5 倍**，
说明它确实在跟随控件而不是窗口；`published=22/26` 的 4 次差额是光标先移动、查询在完成前被
取代（`invalidate_in_flight`），`empty=0` 表示没有「不支持/失败」结果。单次 43 ms、峰值 61 ms，
均在 1500 ms 预算内。

**这 4/26 的取代率正是 ② 树缓存要解决的问题**：同一窗口内换控件时，已展开的层级本可复用，
而现在每次都要从窗口根重新下钻，慢 provider 上就会「查一半被取代」。

### ⑤ 已完成：每窗口元素树缓存

`uia_provider.rs` 增加按 `(hwnd, 父节点矩形)` 索引的**已展开子节点表**：

```rust
children: HashMap<(isize, i32, i32, i32, i32), Vec<(IUIAutomationElement, WalkNode)>>
cache_epoch: Option<SnapshotEpoch>
```

- 命中即跳过 `BuildUpdatedCache` + `GetCachedChildren`，直接用已读几何做「点内最小矩形」裁决；
- 表按快照 epoch 失效：`resolve` 入口 `sync_cache_epoch(job.epoch)`，`release()` 整表清空；
- 键用「父节点矩形」而非 COM 指针：同一窗口内同矩形的节点对命中测试等价，正好对上策略层
  「同边界容器」的处理方式。

单测 `expanded_levels_are_reused_within_a_generation_and_dropped_across_them`（真机夹具窗口）：
同一代内二次查询复用已展开层级、`cached_epoch` 保持 1；把 epoch 改成 2 后表被重建、旧代层级
不会存活。`cargo test --lib` **326 passed / 0 failed**，`cargo check --all-targets` 0 warnings。

**尚未测到收益**：探针的两个停稳点分别落在**不同窗口**（depth 5 与 depth 1），不构成缓存复用场景，
因此耗时与改造前同级（20 / 66 ms）。要量化 ② 的收益必须用**同一个深树窗口内的两个控件**——
也就是待补的资源管理器夹具（导航窗格 / 文件列表 / 命令栏）。

### ⑥ 已完成：资源管理器夹具 + ② 的实测收益

夹具改用 `Shell.Application.Windows()` 取真正的 `CabinetWClass` 句柄（`explorer.exe` 的
`MainWindowHandle` 要么是 0、要么是桌面窗口，此前一直取不到），随后在**同一个窗口内**依次停稳于
导航窗格 / 文件列表 / 命令栏（`.tmp-explorer-probe.ps1`）：

```text
explorer hwnd=9963044 title='Windows' rect=(459,230)-(1751,1046)

rest 1  point=(921,896)        elapsed 17.6 ms  depth=2  bounds=(700,549)->(2616,1558)   文件列表区
rest 2  point=(1755,431)       elapsed 15.8 ms  depth=4  bounds=(1099,424)->(2087,460)   命令栏 988×36（控件级）
rest 3  point=(1260,801)       无 submit —— 该点落在已发布路径内，命中缓存，不触发 worker
summary refinement_submitted=3  refinement_published=3  refinement_empty=0
        refinement_elapsed_us last=16124  max=19300
```

结论：

1. **② 的收益实测到了**：同一窗口内的后续查询从改造前的 43–65 ms 降到 **16–19 ms**（上界 19.3 ms），
   因为已展开的上层被复用、只补最深一层；
2. **深选真的到了控件级**：命令栏解析为 `988×36` 的条状控件（depth 4），而不是整块内容区；
3. **缓存路径复用规则在生产路径生效**：第三个停稳点落在已发布矩形内，调度器直接命中缓存，
   连一次 worker 调用都没有。

### ④ 增量发布：**暂缓，且理由是有据可依**

设计里预留的 32 ms 增量发布，前提是「遍历耗时超过发布间隔」。实测三次查询为
**19.1 / 17.6 / 15.8 ms**，均已低于 32 ms 间隔——此时加入增量发布会**不产生任何可见收益**，
却引入回调、中间结果与额外的陈旧校验路径。按项目规则「性能优化必须有明确依据，禁止为了臆测
性能而增加不必要的复杂抽象」，**本轮不实现**。

触发条件（满足其一再实现）：某个真实应用的查询稳定超过 32 ms；或 `refinement_published`
明显低于 `refinement_submitted` 且原因是「查询太久被取代」而非「点落在缓存外」。

### 剩余

| 项目 | 状态 |
| --- | --- |
| v2-P3 MSAA 回退 provider | 未开始（UIA 覆盖不到的老控件/自绘控件才需要） |
| v2-P4 `path` 逐层描边 | 未开始（预览矩形已跟随深选结果，缺的是层级可视化） |
| v2-P5 性能与 quarantine 命中率 | 部分（单次查询 15–19 ms、缓存收益、`empty=0` 已测得；quarantine 命中率与延迟分布待补） |

---

## 12. v2-P3 MSAA 回退：调研结论与设计（**实现未开始**）

### 12.1 为什么需要它

资源管理器的文件列表区目前只解析到 depth 2（一大块内容区），命令栏能到 depth 4。UIA 覆盖不到的
老式/自绘控件需要 MSAA（`IAccessible`）作为第二来源。调研 `snow-ui-selector/src/windows/msaa.rs`
后确认：**它不是「调一个 API 取矩形」那么简单**，参考实现为它配了一整套防挂死机制。

### 12.2 参考实现要点（真实源码）

| 机制 | 参考代码 | 规格 |
| --- | --- | --- |
| 挂死窗口预检 | `IsHungAppWindow(hwnd)` | 命中即 `mark_unresponsive` 并跳过，**不发起 MSAA 调用** |
| 失败隔离 | `msaa_quarantined` 标志 + `mark_unresponsive()` | 隔离持续到下一次快照刷新（快照重建时重置） |
| 超时执行 | `self.worker.hit_test(hwnd, point, bounds, MSAA_REQUEST_TIMEOUT)`，`MSAA_REQUEST_TIMEOUT = 168 ms` | MSAA 调用跑在**可超时的独立 worker** 上；三态语义见下 |
| 三态区分 | `Option<Result<Result<Vec<RECT>>>>` | `None` = 准入失败（worker 忙）→ **只重试、不隔离**；`Some(Err)` = 超时 → 隔离；`Some(Ok(Err))` = provider 报错 → 空结果 |
| 无 MSAA 兜底路径 | `fallback_hit_path()` → `window::visible_child_window_rects(hwnd, bounds)` | 用 `EnumChildWindows` 收集可见子窗口矩形（与窗口求交、去重、剔除与窗口等大的），**按窗口缓存**（`get_or_insert_with`），再取所有包含该点的矩形 + 窗口外框 |
| 路径合并 | `merge_hit_paths(msaa, fallback, bounds, point)` | 选 seed：MSAA 首矩形**包含**兜底首矩形时取兜底（更具体），否则取 MSAA，都没有则取窗口外框；再把全部候选矩形按面积排序、去重，逐个「包含当前 path 末项」时追加，最后补上窗口外框 |
| 入口 | `AccessibleObjectFromWindow(hwnd, OBJID_WINDOW, IID_IAccessible, &mut raw)` | 取**窗口的** accessible 对象（不是 `AccessibleObjectFromPoint`），再做命中/遍历 |

### 12.3 SnapClip 的落地契约（设计）

新增 `platform/windows/capture/msaa_provider.rs`，实现既有的 `DeepSelectionProvider`：

1. **预检**：`IsHungAppWindow(job.window.hwnd)` → 直接 `Empty(Unsupported)` 并隔离该窗口；
2. **超时执行**：MSAA 查询提交给一个**独立的可超时 worker**（168 ms），超时 → `Empty(ProviderTimeout)`
   并隔离；准入失败 → `Empty(ProviderTimeout)` 但**不隔离**（语义与参考一致，避免把「忙」误判成「坏」）；
3. **兜底路径**：用 `EnumChildWindows` 收集可见子窗口矩形（按窗口缓存、快照 epoch 失效），
   与 MSAA 结果按 §12.2 的 seed + 包含链合并；
4. **映射**：合并后的 path 直接转 `DeepTarget`（`path[0] = ` 窗口外框，`kind = UiElement` 当 path 长度 > 1）；
5. **隔离**：`quarantined: HashSet<isize>`，`release()`（快照换代）清空——与 UIA provider 同构。

**回退顺序**：`FallbackDeepSelection` 组合两个 provider —— 先 UIA，`Empty(Unsupported)` 时再问 MSAA；
两者都失败则保持 v1 整窗帧。这样 v1 路径与叠加动画都不受影响。

### 12.4 为什么本轮**不实现**

这不是「再包一层 API」的规模：它需要
①可超时的独立执行体（含准入控制，超时后无法杀死阻塞中的 COM 调用，只能放弃该线程）；
②`IsHungAppWindow` 预检；
③`EnumChildWindows` 兜底路径及其按窗口缓存；
④seed + 包含链的路径合并；
⑤隔离语义与三态区分。
按项目规则「代码改完不是完成，验证通过才是完成」，我不在剩余预算里开写一个无法完整验证的
COM/超时/线程层——那正是最容易被「看起来能跑」掩盖问题的部分。

**实现顺序（下一步）**：先做 ③ 兜底路径（纯 `EnumChildWindows` + 缓存 + 合并，**不需要 COM**，
可完整单测，且立刻能让文件列表区拿到子窗口级矩形）→ 再做 ①②④⑤ 的 MSAA 本体与超时隔离。
这样即使 MSAA 部分延后，③ 也能独立提升现有 UIA 结果的精度。

### 12.5 ③ 已完成：兜底路径 + 合并（零 COM）

| 位置 | 内容 |
| --- | --- |
| `win/window.rs::visible_child_rects(parent, parent_bounds)` | `EnumChildWindows` 收集可见子窗口矩形；与父框求交、剔除退化与「与父框等大」、最小优先排序去重 |
| `window_detection/uia.rs::{fallback_hit_path, merge_hit_paths, push_if_useful}` | 兜底命中路径构造 + 合并策略（更具体的 seed 胜出、按面积升序做包含链、去重、窗口外框收尾），全部纯函数 |
| `uia_provider.rs` | UIA 下钻结果与兜底路径合并后发布；子窗口矩形按 `(hwnd, epoch)` 缓存，随 `sync_cache_epoch` / `release()` 失效 |

测试：`uia.rs` 新增 4 项合并策略单测（更具体 seed、包含链、去重与嵌套序、不插入不含 tail 的兄弟），
`win/window.rs` 新增真机单测 `visible_child_rects_reports_child_windows_inside_the_parent`
（自建父子 HWND 夹具，断言子矩形被报告、被裁剪、且不含与父框等大的项）。
`cargo test --lib` **331 passed / 0 failed**，`cargo check --all-targets` 0 warnings。

**实机（资源管理器三点探针）**：文件列表停稳点的路径深度由 **2 提升到 3**，命令栏仍为 depth 4。

**必须说明的局限**：发布矩形本身仍是整块内容区——因为资源管理器**文件项是 DirectUI，没有子 HWND**，
`EnumChildWindows` 只能贡献内容面板这一层。也就是说 ③ 的收益体现在「经典子窗口控件」类应用上，
Explorer 文件项的更细粒度必须靠 P3 本体（MSAA / UIA 条目级）。这一点在上一节的实测里已经体现，
不夸大。

### 12.6 二轮调研：解决思路（来自 `uia/cache.rs` 与 `screenshotselectorworkflow.cpp`）

针对「DirectUI 文件项只解析到大块内容区」，二次深调研找到三条**当前实现确实缺少**的机制。

#### ① 结构性回溯（主因）

`snow-ui-selector/src/windows/uia/cache.rs::WindowTree::query`：

```rust
let Some(child) = children.hit_before(point, usize::MAX) else {
    // UIA sibling order is not a stacking guarantee. A redundant structural leaf may cover
    // the content branch (for example in Chromium windows).
    // Only backtrack through equal-bounds structural nodes; actual controls and distinct
    // container frames retain their existing precedence.
    if let Some(alternative) = self.structural_alternative(current, point) {
        current = alternative;
        continue;
    }
    reason = StopReason::Complete;
    break;
};
```

**某条分支走到死路（没有子节点包含该点）时不能结束**：UIA 的兄弟顺序不是叠放保证，
一个与父节点**等边界**的冗余 `Pane`/`Group` 可能挡在真正承载内容的分支前面；只回溯**等边界的
结构性节点**，真实控件与不同边界的容器保持原优先级。

这正是 SnapClip 当前「资源管理器文件列表只解析到整块内容区」的成因：我们按「点内最小矩形」选中
那个冗余 Pane，下钻后发现其子节点都不包含该点，就在 `resolve` 里 `break` 收工，**从不回到它的
兄弟分支**。

**落地方案**：把 `resolve` 的线性下钻改成**带显式栈的 DFS**——每层记录已尝试的分支；死路时在
同一层寻找「等边界结构性且未访问」的兄弟继续；仅当所有分支都无果才停止并保留当前最深结果。
策略层先加纯函数 `structural_alternative(parent_bounds, tried, candidates)` 并用单测锁定
「只回溯等边界结构性节点」，provider 再按栈消费。

#### ② 精化结果「只在更深时应用」

`snow_shot/src/presentation/selector/screenshotselectorworkflow.cpp::handleRefinement` 用
`replacePath` 区分两种语义：`true` → 整体替换（目标变了）；`false` → 走
`applyCanvasRefinementPath(...)`，而该函数在 `refined.size() <= m_hitRects.size()` 时返回 false。

SnapClip 目前是「新结果一律覆盖」，会出现**精化后又被更粗的结果顶回去**（深度回退）。

**落地方案**：`DeepTarget` 增加 `replace_path: bool`（或发布时带上「本次是否换目标」），
overlay 侧：换目标 → 替换；同目标 → 仅当 `path.len()` 更大时应用。这条同样是纯逻辑，可单测。

#### ③ 增量发布

同一个 `query` 内：`control.publication_interval` 到期**且路径变化**时才 `progress(path)`，
配合 `QueryClock` 的 deadline 逐层检查预算、`MAX_STEPS` 封顶。

SnapClip 已实测单次 15.8–19.1 ms（低于 32 ms 间隔）故此前暂缓；**①落地后深度上升、耗时也会上升**，
届时启用就有数据依据（触发条件仍见 §11 的约定）。

#### 实施顺序（修订）

1. **① 结构性回溯**（策略纯函数 + 单测 → provider DFS）——直接针对文件项/Chromium 类窗口；
2. **② 更深才应用**（`replace_path` 语义 + 单测）——防止深度回退；
3. ③ 增量发布（待 ① 落地后的实测数据）；
4. 之后才是 MSAA 本体（超时执行 + 隔离三态），它是 ① 之后的兜底来源而非唯一希望。

### 12.7 ① 尝试过：带栈的 DFS + 结构性回溯（**已回退**）

> **结论先行：这次改动在真机上造成了回归，已回退。** 回退理由与数据见本节末尾。

重构内容（`uia_provider.rs`）：

- 线性下钻改为**显式栈 DFS**：每层保留「所有包含该点且可下钻的子节点，按面积升序」，
  逐个尝试；某分支走到死路时回到同层试下一个候选。
- **回溯判据**由纯函数 `may_try_sibling_branch(dead, parent_bounds)` 决定：只有
  **与父节点等边界的结构性节点**才可以让位给兄弟分支；真实控件与不同边界的容器保持原优先级，
  因此回溯不可能把用户没有指向的东西提升为目标。
- `path` 镜像当前已提交的分支、`best_path` 记住最深已提交路径——放弃死分支不会丢掉已有的好答案；
  节点/深度预算与取消检查仍逐层生效。

新增单测 `only_an_equal_bounds_structural_branch_may_backtrack`（等边界容器可回溯 / 真实控件不可 /
不同边界容器不可 / 离屏不可）。`cargo test --lib` **332 passed / 0 failed**，
`cargo check --all-targets` 0 warnings。

**实机结果（必须如实记录）：资源管理器文件列表停稳点没有变化**——仍为 `depth=3`、
`bounds=(698,345)->(2618,1560)`（整块内容区）。也就是说：**这条机制本身按参考实现落地了，
但并没有解释本机 Explorer 文件项的粗粒度**。下一步需要先取证再改，而不是继续加机制。

#### 产品自测复验 → 发现回归 → 回退

产品在 `npm run tauri dev` 中实测后反馈「本次基本无法识别控件」，会话汇总行给出了判据：

| 指标 | DFS 之前（好） | DFS 之后（差） |
| --- | --- | --- |
| `refinement_submitted` | 26 | **8** |
| `hover_target_switch_count` | 5 | **13** |
| `refinement_published` | 22 | 8 |

光标驱动修复后的正确形态是「`submitted` 明显大于窗口切换次数」；DFS 之后 `submitted` 反而
**少于**窗口切换次数，说明**发布出来的目标矩形变粗了**：调度器的「点落在已发布完整路径内 →
命中缓存、不重查」规则因此抑制了绝大多数后续查询，控件不再跟随光标——正是产品描述的
「基本无法识别控件」。

按「可以破坏旧实现，但不能无意破坏现有功能」与「不得伪完成」，处理方式是**回退到已被验证的
线性下钻**，而不是继续在回归版本上叠补丁：

- provider 恢复为线性下钻（`containing_children(...).into_iter().next()`，即「点内最小矩形」）；
- 策略层保留 `may_try_sibling_branch` 与其单测（纯函数，不影响行为），作为该规则的文档与将来
  重新尝试的基础；
- 回退理由写入代码注释，避免以后有人「再顺手加一次」。

**重新尝试的前置条件**：先加**每次查询的取证日志**（每层「子节点数 / 空矩形数 / 越界数 /
包含该点数」+ 发布时的 `depth` 与矩形），用 Explorer 三点探针跑一遍，确认回溯究竟改变了什么、
以及是否真的能让文件项变细。**没有这份数据就不再加回溯**——本轮的教训正是「按参考实现落地」
不等于「在本机有效」。

#### 取证结论（2026-10-05）：① **不需要**，但发现并修复了「同边界节点环路」

取证日志里每层的 `containing` 就是「该层含该点的候选数」，`>1` 意味着线性下钻跳过了备选分支。
用资源管理器多点探针复跑后的判决：

| 观测 | 数据 | 结论 |
| --- | --- | --- |
| 跳过备选是否导致浅层结果 | 命令栏那次 `#3 containing=2`，选中的最小分支最终到 **`82×27` 的单个按钮**（depth 6）；导航窗格、两个文件项同样到达合理深度 | **没有**出现「兄弟分支遮挡」的实际损失 |
| 是否存在真问题 | 导航窗格查询在**同边界的两个节点之间来回**（`A raw=1 → B raw=9 → A …`），一直跑到深度预算 24 才停，发布路径 depth=13、含成对重复层级 | **有**：自环防护（孩子≠自己）挡不住 `A→B→A` 的二环 |

**处理**：不加结构性回溯（依据：取证显示不需要），改为按**每查询已访问节点集合**防环——
每个节点最多进入一次，走查规模由树本身而非预算决定。修复后同一查询 **24 层 → 5 层**，
发布路径 **depth 13 → 3**，而**发布矩形完全不变** `(703,549)->(1036,1520)`（333×971）；
路径不再带重复层级，P4 的逐层描边也不会把同一个框描两遍。

`may_try_sibling_branch` 与它的单测**保留**（作为该规则的文档），但**不接线**；
若将来遇到真实的「兄弟分支遮挡」实例（判据：某层 `containing>1` 且选中分支提前死路、
而发布深度明显浅于备选可及的层级），再按 §12.6 的方案接线。

---

## 13. 产品缺陷报告（2026-10-05）：目标不跟随光标 + A→B 动画异常

### 13.1 原则（产品确认）

1. **目标由鼠标当前位置决定**——不依赖「是否刚进入区域」、不依赖上一次结果；
2. 嵌套区域**取最内层**可捕获区域；
3. 相邻区域 A→B 的切换**不得出现错误中间目标**；
4. **不得用动画掩盖目标识别问题**：先保证检测 `A → B`，再对 `A → B` 做平滑。

### 13.2 问题 1（父级 → 子级不切换）——根因已定位并修复

**根因**：`RefinementScheduler::on_cursor_moved` 里有一条抑制规则——
「点落在**已发布路径**内 → 命中缓存、不重查」。已发布的「大盒子」矩形覆盖其内部所有点，
因此鼠标在大盒子内部移动到小盒子时**根本不触发查询**，目标停在大盒子；
「先离开再进入」之所以有时有效，是因为它绕过了这条抑制。

**为什么参考实现的同名规则在这里不成立**：参考的 `WindowTree` 缓存**整棵已展开的树**，
新点可以在缓存内自己走一遍；而 SnapClip 的层级缓存只在 provider 内部，调度器一旦抑制就没有
任何人再走查。**规则与实现不匹配**，因此这条抑制是错误优化。

**修复**：**每个光标位置都各自重新查询**（`on_cursor_moved` 始终 arm dwell）。
代价由 provider 的层级缓存承担——实测同一窗口内重复查询 2.3–6.7 ms，远低于 80 ms 防抖与
1500 ms 预算。测试 `moving_inside_the_published_path_still_re_queries` 固定这条语义
（原测试断言「路径内移动不触发查询」，属**预期行为的正确变化**，已按新语义改写）。

### 13.3 问题 2（A → B 出现「先放大再收缩」）——诊断与取证要求

两种可能来源，必须先用数据区分，不做猜测式修复：

| 可能来源 | 判别方法 |
| --- | --- |
| **真实中间目标**：光标在 A、B 之间的空隙停留 ≥80 ms，空隙属于容纳 A/B 的容器 C，查询如实发布 C（大）→ 动画放大 → 移到 B 后收缩 | verbose 日志里会出现三条 `refinement published`：A 的矩形 → C 的矩形 → B 的矩形，且 `point` 落在 A/B 之间 |
| **动画本身**：只有一个目标变化 A→B，但插值过程产生更大矩形 | 日志里只有 A → B 两条发布；此时问题在 `RectTransition`（理论上 A 与 B 逐边插值不会超出两者包络，需实测确认） |

**修复方向（待数据）**：若是前者，则该中间目标**在语义上并非错误**（空隙确实属于 C），
需要的是交互策略而非检测修补——例如「容器级目标需连续两个 dwell 稳定才升级为预览」或用
container 深度阈值过滤；若是后者，则修动画的插值。**无论哪种，都不许用增大插值/延长动画来掩盖**。

### 12.8 真正的根因：兜底合并语义错误（已修复）

回退 DFS 后产品复测**仍然一样差**，于是判据指向了 ③（子窗口兜底合并）而不是 DFS。定位结果：

**`merge_hit_paths` 把 fallback 的首个矩形当作 seed**。而 UIA 的下钻结果 `outcome.path` 是
**外框在前**的（`path[0]` = 窗口外框），于是 `contains_rect(primary.first(), fallback.first())`
恒为真 → seed 取 fallback（更粗的子窗口矩形）→ 合并路径的最深项变成那个粗矩形；再叠加
provider 用 `path.last()` 作为发布矩形 → **每次发布都是整窗/大面板**。后果连锁：
调度器的「点落在已发布路径内 → 命中缓存、不重查」把后续查询全部抑制 → 控件不再跟随光标、
`refinement_submitted` 反而少于窗口切换次数。

**修复（根因，不是补丁）**：重写合并规则为**只允许向下细化，绝不向上变粗**——

```text
1. 取 primary 路径里可用（非空、含点）的项，保持其外框在前的既有顺序；为空则用窗口外框；
2. 把 fallback 的矩形按面积升序，**仅当严格位于当前 tail 之内时**追加到路径末尾。
```

这样 fallback 只能把结果做得更细（例如把「文件列表面板」细化为子窗口控件），永远无法把
UIA 已经解析出的细控件替换成大矩形；发布矩形 `= path.last()` 也随之恢复为「最具体项」。

**实机复验（资源管理器同一窗口内四点停稳）**：

| 停稳点 | 耗时 | depth | 发布矩形 | 含义 |
| --- | --- | --- | --- | --- |
| (1591,594) | 29.7 ms | 3 | `(1042,549)->(2616,1520)` | 文件列表区 |
| (921,896) | 6.7 ms | 3 | `(703,549)->(1036,1520)` | **导航窗格 333×971** |
| (1851,1019) | 2.3 ms | 3 | `(1042,549)->(2616,1520)` | 文件列表区（**层级缓存命中 → 2.3 ms**） |
| (1755,431) | 8.1 ms | 4 | `(1099,424)->(2087,460)` | **命令栏 988×36** |

`refinement_submitted=4 / published=4 / empty=0`，且 `hover_target_switch_count=1` —— 同窗口内
精化次数大于窗口切换次数，正是「跟随控件」的正确特征；三个区域给出三个**互不相同**的矩形，
不再退化为整窗。

**仍未解决**：资源管理器**文件项**（DirectUI 虚拟化列表项）仍只到「文件列表面板」一级。
这是 ① 结构性回溯（已回退，待取证）与 P3 本体要解决的范围，与本次合并语义缺陷是两件事。

**产品复验（`npm run tauri dev`，默认日志汇总行）——修复确认**：

| 指标 | 回归前（好） | 回归中（差） | 修复后 |
| --- | --- | --- | --- |
| `refinement_submitted` | 26 | 8 | **23** |
| `hover_target_switch_count` | 5 | 12–13 | **6** |
| `refinement_published` | 22 | 8 | **22** |
| `refinement_elapsed_us` | last 43 / max 61 ms | last 3.9 / max 22.8 ms | **last 3.5 / max 22.6 ms** |
| `window_hit_test_us` | n=1790 | n≈1200 | n=1963（max 123 µs） |

`submitted ≫ 窗口切换次数` 的「跟随控件」特征恢复，`published/submitted = 22/23`（差额是光标先
移动、查询在完成前被取代），`empty = 0`、错误 0；峰值耗时同时优于回归前的 61 ms。

### 12.9 文件项粗粒度的根因：缓存键用了矩形（已修复）

### 12.10 P4 已完成：`path` 逐层描边

`path_bounds` 从 `DeepTarget::path` 推出（**去掉最深一项**，它由 hover/preview 矩形负责，
避免重复描边），只在「发布路径属于当前光标下窗口」时生成——否则会在无关像素上画框；
`draw_window_hints` 先描祖先层（细线、只描边不填充），再画强调矩形，因此层次关系可见而
目标仍然是最后那一层。与 hover/preview 一样，路径提示**永不进入产物像素**（
`OverlayRenderer::render_export` 统一剥离三个字段）。

GPU 视觉回归（`window_snap_hints_are_painted_but_never_exported` 扩展）：断言祖先层的边缘像素
≠ 纯遮罩像素（确实描了边），且导出同一矩形仍是逐像素原始冻结帧。

取证日志（`refinement level #N node=…`）**决定保留**：它在两轮排查里都是唯一能分辨
「同边界链前进」与「原地打转」的证据；保持 verbose 门控，默认不输出。

### 12.11 P3 进行中：可超时执行体已完成

`platform/windows/capture/timed_call.rs`（本次提交）实现了参考实现里 MSAA 那套防挂死机制的
**执行体部分**——它不依赖 COM，因此可以完整单测：

```rust
pub enum TimedOutcome<T> { Completed(T), TimedOut, Busy }
pub struct TimedCallRunner { /* max_in_flight + 原子计数 */ }
```

| 语义 | 实现与理由 |
| --- | --- |
| 按时返回 | `Completed(T)`；槽位在闭包返回时释放 |
| 超过 deadline | `TimedOut`；**被放弃的调用仍占着槽位直到它真正返回** —— 这正是把「挂死的 provider」限制在有限线程内的机制（参考实现同构：超时后无法杀死阻塞中的 COM 调用，只能放弃线程） |
| 达到容量 | `Busy`；**只重试、绝不隔离** —— 「忙」与「这个窗口有问题」是两件事（参考实现注释明确区分） |
| 线程创建失败 | 释放槽位并返回 `Busy`（不因自身失败而隔离窗口） |

单测 3 项：按时返回并释放槽位；超时后槽位仍被占用、第二次请求得到 `Busy`、被放弃的调用结束后
容量恢复；容量 2 时第三个请求仍被拒绝。`cargo test --lib` **335 passed / 0 failed**。

**尚未完成**：MSAA 的 COM 查询本体（`AccessibleObjectFromWindow(OBJID_WINDOW)` →
`accHitTest` / `accLocation`）。它需要 `VARIANT`（`Win32_System_Variant` feature + VT_I4 联合体
初始化），而 `accLocation`/`accHitTest` 都要求 `VARIANT` 参数——这部分单独一步做，避免在
预算末尾留下编译不过的半成品。接入方式已定：provider 先 `IsHungAppWindow` 预检 → 用
`TimedCallRunner` 包裹查询 → `TimedOut` 隔离该窗口、`Busy` 仅重试、报错返回空；
与 UIA 组成 `FallbackDeepSelection`（先 UIA，`Unsupported` 再问 MSAA）。

### 12.12 P3 已完成：MSAA 查询本体 + 回退组合（本次提交）

`platform/windows/capture/msaa_provider.rs`：

| 环节 | 实现 |
| --- | --- |
| 依赖 | `windows` 0.61 features 增加 `Win32_System_Variant` **与** `Win32_System_Ole` —— `VARIANT` 及其实体在 windows-rs 里由 **Com + Ole 双 feature** 共同门控，只加 Variant 仍然不可见 |
| 预检 | `IsHungAppWindow(hwnd)` 命中即隔离，**不发起** MSAA 调用 |
| 超时 | 查询交给 `TimedCallRunner`（168 ms，与参考一致）；`TimedOut` → 隔离该窗口；`Busy` → **只重试不隔离**；provider 报错 → 空结果 |
| 查询 | `AccessibleObjectFromWindow(hwnd, OBJID_WINDOW, IID_IAccessible)` → `accHitTest(point)` →（`VT_I4` 子 id 或 `VT_DISPATCH` 子对象）→ `accLocation` 取屏幕矩形 |
| 线程 | 闭包在独立线程执行，`HWND` 以整数传递（裸指针不是 `Send`），COM 在线程内 `CoInitializeEx(APARTMENTTHREADED)` |
| 生命周期 | 隔离持续到快照换代（`release()`），与 UIA provider 同构 |
| 组合 | `FallbackDeepSelection`：先 UIA，仅在 UIA 返回 `Unsupported` 时问 MSAA；两者皆空 → overlay 保持 v1 整窗帧 |

指标：`refinement_msaa_attempts / timeouts / busy / failures` 已加入 `WindowDetectionMetrics`
（暴露「隔离是否必要」与「是否只是忙」的区分）。

**实机（资源管理器三点 + 两个文件项）**：

```text
published (703,549)->(1036,1520)   depth=3  导航窗格
published (1063,985)->(1960,1022)  depth=4  文件项 A  897×37
published (1063,1200)->(1960,1237) depth=4  文件项 B  897×37
published (1099,424)->(2087,460)   depth=4  命令栏   988×36
refinement_submitted=5  published=5  empty=0
```

不同文件项给出不同矩形，说明深选确实跟随到条目级；`cargo test --lib` **339 passed / 0 failed**
（新增 MSAA 单测 4 项），`cargo check --all-targets` 0 warnings。

**已知小缺口**：`refinement_msaa_*` 计数已记录但**尚未出现在会话汇总行**（格式串漏加），
下次一并补上——本轮不以「看似完整」掩盖它。

### 12.13 P5 已完成：汇总行补齐 + 延迟分布 + quarantine 统计

- **汇总行补齐**：`refinement_msaa_attempts/timeouts/busy/failures` 已进入会话汇总；断言
  「汇总行包含全部指标名」的单测同步扩展（上一轮漏项被测试兜住）。
- **延迟分布**：新增 5 档直方图（`<16ms / <32ms / <64ms / <256ms / >=256ms`）。分档依据是
  该功能的可感阈值：32 ms 以下相对 80 ms 防抖不可感知，32–64 ms 在快速掠过多个控件时开始可感，
  最高档才是 quarantine 真正发挥作用的地方。`record_refinement_published` 自动入桶。
- **quarantine 统计**：`refinement_quarantine_added`（被隔离的窗口数）与
  `refinement_quarantine_hit`（**直接从隔离回答、完全没碰 provider** 的查询数）——后者就是这套
  机制省下的调用量；UIA 与 MSAA 两个 provider 的隔离点都已接入。

**实机复验（资源管理器，重新测试）**：

```text
published (703,549)->(1036,1520)   depth=13  导航窗格 333×971
published (1063,985)->(2076,1022)  depth=4   文件项 A  1013×37
published (1063,1200)->(2076,1237) depth=4   文件项 B  1013×37
published (1751,427)->(1833,454)   depth=6   命令栏上的**单个按钮** 82×27
refinement_submitted=5  published=5  empty=0
refinement_msaa_attempts/timeouts/busy/failures = 0        （UIA 已足够，未触发回退）
refinement_latency_buckets <32ms=2 <64ms=3                  （全部 16–64 ms）
refinement_quarantine_added/hit = 0
errors = 0
```

命令栏由上一轮的整条 `988×36` 细化到 **`82×27` 的单个按钮**（depth 6），两个文件项各自给出
自己的行矩形——深选已经落到控件/条目级。`cargo test --lib` 339 passed / 0 failed，
`cargo check --all-targets` 0 warnings。

**取证过程的一个教训**：第一次取证日志里的停稳点落在 `hwnd=5967024`（全屏最大化窗口），
**不是资源管理器**——`Shell.Application.Windows()` 每个标签页返回一项，而 Windows 11 上它们
**共用同一个 HWND**，所以「取最后一个窗口」并不等于「刚打开的那个」。探针已改为按
`LocationName/LocationURL` 匹配并打印全部候选。

在**真正的**资源管理器窗口（`hwnd=9963044`）上，取证日志（`refinement level #N node=… raw/empty/
containing`）给出：

```text
#1 node=0x…cfb80 (698,345)->(2618,1560) raw=8  empty=1  containing=5   窗口层
#2 node=0x…cf790 (700,549)->(2616,1558) raw=1  containing=1            内容面板
#3 node=0x…04eb0 (700,549)->(2616,1558) raw=3  containing=1            ← 同边界、不同节点，孩子 1→3
#4 node=0x…05600 (703,549)->(1036,1520) raw=1  containing=1
   published (703,549)->(1036,1520) depth=3                             导航窗格 333×971

#4 node=0x…deb50 (1042,549)->(2616,1520) raw=1  containing=1
#5 node=0x…a1230 (1042,549)->(2616,1520) raw=24 containing=1           ← 24 个子节点 = 文件项
#6 node=0x…22c40 (1063,985)->(1960,1022) raw=4  containing=0
   published (1063,985)->(1960,1022) depth=4                            **文件行级 897×37**
```

**根因**：`children` 展开缓存原先以**父节点矩形**为键。UIA 在资源管理器里给出一条
**同边界的节点链**（内容面板 → 单子容器 → 真正的列表容器），矩形键让这些**不同节点共用同一条缓存
条目**，于是「选中的孩子」永远是同一批 → 下钻原地打转直到深度预算耗尽，发布矩形停在面板一级。
参考实现用 `cache_index`（节点索引）而非矩形做键，正是为了区分同边界节点。

**修复**：缓存键改为**元素身份（COM 指针）**，并加**自环防护**（绝不走进当前节点自身）。
修复后同一边界链被逐层正确展开：`raw` 从 1 → 3 → **24**，最终发布 **`(1063,985)->(1960,1022)`
= 897×37 的文件行**。`cargo test --lib` 332 passed / 0 failed，`cargo check --all-targets` 0 warnings。

**遗留**：取证日志目前是 verbose 门控的调试输出（`refinement level #N node=…`），在文件项问题
收尾后应决定是保留为长期诊断还是移除。① 结构性回溯仍处于「已回退、需重新取证」状态——
本轮数据说明它未必必要（同边界链已能正常下钻），除非遇到「兄弟分支遮挡」的实例。

**产品复验（`npm run tauri dev`，默认日志汇总行）——确认文件项级吸附生效**：

```text
window_hit_test_us n=6634      hover_target_switch_count=6
refinement_submitted=36  refinement_published=34  refinement_empty=0
refinement_elapsed_us last=24549  max=59174
```

同窗口内 `submitted(36) ≫ 窗口切换(6)`，说明精化在**文件项之间**持续跟随光标；
34/36 发布成功（2 次是光标先走、查询在完成前被取代），`empty=0`、错误 0。
峰值 59 ms（比面板级的 22 ms 高）符合「多下钻两层到文件项」的预期，且仍远低于 1500 ms 预算。

**下一个取证假设（待验证）**：DirectUI 的虚拟化条目在未实现/未滚入视口时会把
`CurrentBoundingRectangle` 报成**空矩形**，而 `cached_node()` 目前对空矩形直接返回 `None`，
于是这些条目连同它们**可能报出真实矩形的子孙**一起被跳过。验证方式：加一个 verbose 统计
（每层「子节点数 / 空矩形数 / 越界数 / 包含该点数」），跑一次 Explorer 三点探针即可判断。
只有在数据确认是空矩形导致之后，才按「空矩形节点允许下钻、路径沿用父矩形」来修（同样是纯策略改动）。

### 12.15 预览回退策略：等待答案期间不回落整窗（已修复，对应缺陷报告场景 2/3/4）

**现象**：从控件移动到它的**父级空白区**时，预览矩形先变成**整窗**（"最大的盒子"），片刻后再收缩
到父级容器（场景 2）；A→B 之间也会出现同样的中间态（场景 3）。

**根因（预览回退条件写错了对象）**：`preview_for_cursor` 只有两种取值——"命中的深层矩形"或
**v1 整窗帧**。判断"是否该等答案"用的是 `refinement_hold`，而它 **只在等待序列的第一次移动时设置**
（`if self.refinement_hold.is_none()`），并且在**第一次答案到达时就被清空**——包括
`NeedsConfirmation` 那种**尚未发布**的暂存答案。于是只要满足任一条件：

* 光标在本窗口内已经移动超过 320 ms 才停下；
* 或者答案已到但降级还在等第二次停稳（§13.3）；

…下一次重新计算预览（hover 复验 `BoundsChanged` 也会触发）就会落进整窗分支，把**整窗帧**画出来，
答案到达后再收缩。整窗帧不是中性的填充物：它本身就是一个"更大的目标"。

**修复（策略提纯为纯函数 + 等待锚定到"当前问题"）**：

```rust
pub fn preview_bounds(
    deep: Option<&DeepTarget>, window: WindowIdentity, point: Point,
    window_bounds: Rect, waiting: bool,
) -> Option<Rect>
```

| 状态 | 预览 |
| --- | --- |
| 已验证矩形仍覆盖光标 | 该矩形 |
| 该窗口有已验证矩形、但不再覆盖光标，且**答案在途** | **保留**该矩形（等待中不回退整窗） |
| 该窗口**尚无可信矩形**，且答案在途 | **不画**（`None`）——第一次看到的就是光标下的控件，不是整窗 |
| 无在途答案（provider 不可用 / 已超时 / 已放弃） | v1 整窗帧（诚实的下限） |

`refinement_pending: Option<Instant>` 取代 `refinement_hold`，语义是"**当前位置**的答案正在路上"：
**arm 即计时（含暂存降级重新 arm 的第二次停稳）**，答案落地、失败、窗口消失或会话结束即清零；
上限沿用 320 ms（`REFINEMENT_PREVIEW_WAIT_MS`）。因此在 320 ms 内永远保留可信矩形、绝不闪整窗，
超过上限才降级到 v1——"有界降级"的保证没有被削弱，只是把窗口期的取值从"整窗"换成了
"上一次已验证的矩形"或"不画"。

**测试（纯函数，`deep.rs`）**：等待中保留上一次矩形（即使它已不覆盖光标）；等待结束回落整窗；
首次 hover 在途时不画；别的窗口的路径不得泄漏；覆盖光标的目标即使不在等待中也继续显示。

`cargo test --lib` 352 passed / 0 failed，`cargo check --all-targets` 0 warnings。

### 12.16 单飞槽的预算与看门狗（已修复）

**背景**：`REFINEMENT_BUDGET_MS = 1500` / `REFINEMENT_CALL_LIMIT_MS = 500` 从 v2-P0 起就写在
契约里（docs/18 §3），但**没有任何代码读它**——provider 只检查节点/深度预算与协作式取消。
一个卡在单次 COM 调用里的 provider 会永久占住单飞槽：现象与 12.14 的 id 漂移**完全一样**
（`refinement_submitted=1 refinement_published=0`，此后不再增长），但成因不同。

**修复（两层，各自可用纯单测验证）**：

1. **协作式总预算**：`QueryControl::budget_exhausted()`（`started + budget`），UIA 走查在**每层之间**
   检查 → 超时即发布部分路径并标 `StopReason::BudgetExhausted`。这是 provider 能做到的那一半。
2. **外层看门狗**：`RefinementScheduler::on_in_flight_timeout(now)`，上限
   `REFINEMENT_INFLIGHT_TIMEOUT_MS = 1500 + 500`。超过即释放单飞槽、`retire()` 掉 worker 的 gate、
   记 `refinement_inflight_timeouts`，并让预览回落到 v1 帧。`now` 由调用方传入，
   规则不依赖 sleep 就能测。卡死的 COM 调用无法被杀，只能**放弃**（与 MSAA 的 `TimedCallRunner`
   同一取舍：放弃线程，不阻塞调用方）。

看门狗挂在 overlay 的周期性 hover tick 上（`poll_refinement_timeout`），因此不依赖光标继续移动：
静止的光标也能等到降级。

**测试**：预算内的查询不动它；超预算返回被放弃的 request id 且槽位可复用（下一次 dwell 能发新号）；
被放弃的查询即使随后返回也不能再发布；`QueryControl` 新建时预算未耗尽。

`cargo test --lib` 352 passed / 0 failed，`cargo check --all-targets` 0 warnings。

### 12.14 跨会话根因：refinement 有两个 request id 空间（已修复）

**现象（产品复验）**：同一次进程里，**第一次 F5 深选正常**（`submitted=36 published=34`），
**第二次及以后完全失效**——只吸附整窗（"默认吸附软件窗口"）：

```text
session 1: refinement_submitted=40 refinement_published=35 refinement_latency_buckets <16ms=3 <32ms=1 <64ms=31
session 2: refinement_submitted=1  refinement_published=0  refinement_empty=0
           refinement_elapsed_us last=0 max=0   ← 一次都没有送达
```

`submitted=1` 且此后不再增长，说明**单飞槽被永久占住**：调度器在等一个永远不会被接受的答案。

**根因（模块边界的 id 所有权错误，不是 provider 问题）**：refinement 有**两个** `RequestGate`，
各自从 1 开始发号：

| 位置 | 发号者 | 用途 |
| --- | --- | --- |
| `RefinementScheduler::on_dwell_due` | 调度器 | 记 `in_flight`，判定结果是否仍是当前问题 |
| `RefinementWorker::request` | worker | 判定在途查询是否已被取消 |

两者**只是碰巧同步**：只要每个 `on_dwell_due` 都恰好对应一次 `request()`，计数器就一致。而
`begin_window_detection()` 每次会话都会 `refine.reset()`——调度器的计数器回到 0，worker 的计数器
继续累加。于是第二次 F5 起，worker 回投的 id 与调度器 `in_flight` 恰好差「上一会话的查询条数」，
`on_result()` 判为陈旧 → **静默丢弃**，`on_failure()` 同样不匹配 → 单飞槽永不释放 → 本会话只提交
1 次查询。日志里 `published=0 / empty=0 / elapsed=0` 正是「结果被拒」与「结果从未产生」两种情况的
共同表现，仅凭汇总行无法区分——这也是它拖了一轮才被定位的原因。

**修复（单一 id authority）**：id 属于**拥有问题的一方**，即调度器。worker 不再发号，改为
`RequestGate::adopt(job.request)` 接收调用方（调度器）给的 id；`request()` 的签名从
`(epoch, window, point, bounds)` 变成 `(job: RefinementJob, window_bounds)`，overlay 直接把调度器的
job 转交。两端从此共用一个 id 空间，任何一侧 `reset()/retire()` 都不会再产生漂移。

同时把「worker 一旦开始执行某个 job 就**必定且仅回投一次**」写成模块不变量：被取代的 job 回投
`Empty(Cancelled)`（带自己的 id，由调度器按陈旧丢弃），而不是静默消失——静默消失正是单飞槽"没有
任何一方能释放它"的另一条路径。

**回归测试（先红后绿）**：

- `refinement_worker::tests::a_second_session_still_accepts_the_worker_result`：在**同一个 worker**
  上跑两个会话（`scheduler.reset()` + `worker.retire()` 之后重新提交），断言第二个会话的结果仍然
  是当前问题。旧实现下该断言失败（红），修复后通过（绿）。
- `model::tests::an_adopted_id_becomes_the_newest_request_and_keeps_issue_monotonic`：adopt 之后
  `latest` 即该 id，且后续 `issue()` 不会重发已用过的号。
- `refinement_worker::tests::a_superseded_query_is_cancelled_cooperatively`：按新不变量更新为
  「被取代的 job 回投 `Empty(Cancelled)`，最新 job 回投目标」。

**实机（真实 UIA 栈）端到端回归**：
`uia_provider::tests::the_real_pipeline_still_answers_in_the_second_capture_session` 用测试自建的
真实窗口（`FixtureWindow`）把 **调度器 → refinement worker → 真实 UIA provider** 串起来，
在**同一个 worker** 上跑两次会话（中间 `scheduler.reset()` + `worker.retire()`，与两次 F5 同构），
断言第二次会话的答案仍被接受。**故意把 worker 改回自建 id 空间后该测试失败（10.2 s 超时），
修复版通过（0.27 s）**——这是"第二次 F5 失效"在真实 accessibility 栈上的红→绿证据。

`cargo test --lib` 353 passed / 0 failed，`cargo check --all-targets` 0 warnings。

### 12.17 实机复验（产品确认，2026-10-05）

三轮修复（`5ca3b11` id 空间、`2e7d85c` 预览回退策略 + 预算/看门狗、`c1e118c` 真实栈端到端回归）
在 4K/DPI144/单屏 WGC、Windows 资源管理器上复验通过。

**① 连续 9 次 F5（每次都在窗口内停稳）——"第二次起失效"不再出现**：

```text
session 1  refinement_submitted=1  refinement_published=1  last=26 ms
session 2  refinement_submitted=1  refinement_published=1  last=19 ms
session 3  refinement_submitted=1  refinement_published=1  last=26 ms
session 4  refinement_submitted=1  refinement_published=1  last=20 ms
session 5  refinement_submitted=1  refinement_published=1  last=24 ms
session 6  refinement_submitted=0  refinement_published=0              （F5 太快，没到 80 ms 停稳）
session 7  refinement_submitted=0  refinement_published=0
session 8  refinement_submitted=1  refinement_published=0              （提交瞬间被下一次 F5 打断）
session 9  refinement_submitted=2  refinement_published=2  last=20 ms  ← 紧接着就恢复正常
```

session 8/9 是这次修复最直接的证据：**会话在提交后立刻被 F5 打断，留下一个在途查询，下一次会话
照样能提交并发布**。修复前从第二次会话起 `refinement_published` 恒为 0。

**② 连续移动（真实手感）**：

```text
session 10  mouse_move_coalesced_count=4305  hover_target_switch_count=1
            refinement_submitted=43  refinement_published=38  refinement_empty=0
            refinement_latency_buckets <32ms=2 <64ms=36      （全部 16–64 ms）
            refinement_inflight_timeouts=0   errors=0
```

同一窗口内 `submitted(43) ≫ 窗口切换(1)`：精化在**控件之间**持续跟随光标；38/43 发布成功，
5 次是查询完成前光标已移开（协作式取消），无一次失败、无一次超时。会话汇总行新增的
`refinement_inflight_timeouts` 全程为 0，说明 §12.16 的看门狗在本轮负载下从未触发，
`REFINEMENT_BUDGET_MS` 也从未被真正用尽。

产品侧判定：四个验收场景（父级→子级 / 子级→父级 / A→B / 连续移动）与"多次 F5"全部通过，
预览不再出现整窗中间态。

## 14. 浏览器（网页）元素捕获：调研与实测（2026-10-05，**未实现**）

### 14.1 结论先行

网页元素**无法**从进程外读 DOM，也**不需要**读 DOM：Chromium（Edge/Chrome）、Firefox 把网页
发布成**操作系统无障碍树**（Windows 上是 UIA），每个有角色的 DOM 元素就是一个 UIA 节点，
**自带屏幕矩形**。所以"浏览器里更简单"这个直觉**对了一半**：树更规整（按钮、链接、输入框、
文本都是明确的 control type + 真实矩形），但入口和 Win32 程序**完全相同**——同一条 UIA 走查。

snow-shot 里**没有任何**浏览器专用代码（无 CDP、无扩展、无 DOM 注入）：整个 `snow-ui-selector`
对浏览器和普通程序走同一个 `UiaBackend`。它对 Chromium 的特殊处理只有一条注释和一条很窄的规则
（§14.3），却足以决定"能不能进到网页元素"。

### 14.2 实测：网页的盒子确实在，但我们现在的走查进不去

探针：`uia_provider::tests::browser_element_probe`（`#[ignore]`，自带临时 profile 启动本机
Chromium，**不触碰用户浏览器状态**）。

```text
cargo test --lib browser_element_probe -- --ignored --nocapture
```

页面是 3 列网格（button / div / a）+ 嵌套 span + input。**Chromium 暴露的层级**（实测）：

```text
#0 Window            子节点=2，两个 Pane 边界几乎相同，都包含光标
   ├─ Pane(127,60)-(1392,1033) name=""                     ← 死叶子：子节点=0
   └─ Pane(127,60)-(1394,1033) name="SnapClip UIA probe - Google Chrome"
        └─ Pane(同边界) ×4 层同边界 Pane                      ← 需要一路穿过
             └─ Document(127,147)-(1393,1033) "SnapClip UIA probe"
                  └─ Button(151,171)-(371,291)  "Button One"     ← 网页元素，真实矩形
                     Text(469,222)-(525,244)    "Div Two"
                     Hyperlink(623,171)-(843,295) "Link Three"
                     Text(181,372)-(271,393)    "Nested Span"
                     Edit(151,470)-(479,517)    ""
```

**现在的实现（实测）**：五个探针点（按钮格、div 格、链接格、嵌套 span、页面空白）**全部**发布
`(189,90)-(1392,1033)` = 1203×943 ≈ 整个窗口，`depth=3`，`reason=Complete`——一次都没进到
Document，更没进到网页元素。

**根因（下钻偏好，不是 provider 能力问题）**：窗口根下两个 Pane 是**重叠兄弟**：

| | 边界 | 面积 | 子节点 | 谁选中 |
| --- | --- | --- | --- | --- |
| 无名 Pane | `(127,60)-(1392,1033)` | 1 265×973（**2 px 更窄 → 更小**） | **0** | **我们选它** |
| 标题 Pane | `(127,60)-(1394,1033)` | 1 267×973 | 有（内容分支） | 参考实现选它 |

我们的 `deepest_child_at` 是"**最小者胜**"，于是选中那个**更小但已经没有子节点的死叶子**，
走查到此结束。参考实现用的是另一条规则：`hit_before(point, usize::MAX)` 取**索引最大**（视觉上
最靠上）的包含子节点，因此直接落到标题 Pane（内容分支）；当它也不含光标时才用
`structural_alternative` 回溯到**更早的兄弟**。

### 14.3 参考实现里与浏览器直接相关的四点（真实源码）

| 位置 | 做法 | 为什么对浏览器重要 |
| --- | --- | --- |
| `uia.rs` `NativeProvider::new` | `request.SetTreeFilter(&automation.ControlViewCondition()?)` | 注释原文：Control view **去掉"只参与布局"的 pane**，"can obscure the content hit path"。Chromium 的布局层节点极多 |
| `uia/cache.rs` `query` | `children.hit_before(point, usize::MAX)` —— **索引最大**的包含子节点优先 | 决定进"内容分支"而不是死叶子；UIA 兄弟顺序不是层叠保证，但对 Chromium 恰好有效 |
| `uia/cache.rs` `structural_alternative` | 无包含子节点时，**只在**「当前节点是 structural(`Pane`/`Group`) **且与父节点同边界**」时，向**更早的兄弟**回溯；否则立即 `Complete` | 注释原文点名 **Chromium**："A redundant structural leaf may cover the content branch (for example in Chromium windows)"。我们 12.7 试过并回退的是**更宽**的版本（对所有重叠兄弟做栈式 DFS），语义不同 |
| `uia.rs` `NativeProvider::set_timeouts` | `IUIAutomation2::SetConnectionTimeout/SetTransactionTimeout`，每次调用设为 `min(剩余预算, call_limit)`；超时 → `ProviderTimeout`，`retry_timeout` 时**每个节点重试一次** | 这是 UIA **客户端自己的**超时：卡死的 provider 会被 UIA 直接中止调用。我之前说"单次调用无法设限"是不准确的——参考实现正是这么做的（我们用 `CUIAutomation` 而非 `CUIAutomation8`，所以拿不到这两个接口） |

另外参考实现有两处我们需要对照的语义：

- `StopReason::AccessibilityPending`（`query.rs`）：**树尚未就绪**是一种独立结果，不是"窗口没有树"。
  实测正好撞上：第一个探针点（窗口刚出现）读到的第 7 层只有一个 `Pane(0,0)-(0,0)` 占位节点，
  几毫秒后的其余四个点才读到真正的 Button/Hyperlink/Edit。也就是说 **Chromium 的无障碍树是
  异步物化的**，第一次查询可能落在占位态；把它当成最终答案就会一直停在整窗。
- `StopReason::DecodingPending` + `publication_interval=32 ms`：同一批子节点分多次解码、期间
  **增量发布**路径（§3 的"发布间隔"档位）。

### 14.4 对 SnapClip 的落点（尚未实现，待确认）

1. **下钻偏好**：`deepest_child_at` 从"最小者胜"改为"**最靠上的包含者优先**"，并在其死胡同时
   按参考实现回溯到**下一个候选兄弟**（仅限包含光标的候选，数量天然很少），保留"最小者胜"作为
   **同层级平局**的次序。这会改变 Win32 用例的既有行为，必须有 Explorer/示例程序的回归证据。
2. **占位态**：空矩形/占位层级视为"树未就绪"，返回既有的 `StopReason` 语义（新增
   `AccessibilityPending` 或复用）**而不是**发布整窗；调度器侧它等价于"这次没答案"，下一个
   dwell 会重新查询（我们的调度器本来就每次位置都重查）。
3. **ControlView 过滤 + UIA 级超时**：换 `CUIAutomation8`/`IUIAutomation2` 拿
   `SetConnectionTimeout/SetTransactionTimeout`（顺带把 12.16 的"单次调用无法设限"补成真正的
   调用级上限），并评估 ControlView 过滤对 Explorer 结果的影响（可能同时改善/改变已验收行为，
   必须前后对比）。
4. **回归夹具**：保留 `browser_element_probe` 作为人工探针；若要进自动化，则用参考项目那套做法
   （`examples/support/native_fixture.rs` 风格：在测试里建**真实窗口并让 UIA 看见**），但浏览器
   夹具依赖本机安装的浏览器，只能 `#[ignore]`。
