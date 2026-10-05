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
