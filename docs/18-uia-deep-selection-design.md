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
