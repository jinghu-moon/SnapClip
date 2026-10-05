# SnapClip 浏览器元素智能吸附：调研、设计、实测与交接

> 用途：把"在浏览器里吸附网页元素盒子"这件事的**真实调研结论、可复现实验、失败记录和
> 未解问题**交接给下一个 agent。本文只写**有证据**的内容：每条结论都标注了来源（源码位置 /
> 实测数字 / 复现命令）。没有验证过的推断一律标注"未验证"。
>
> 相关文档：`docs/14`（窗口快照与命中）、`docs/18`（UIA 深选，含 §13 产品缺陷报告与 §14 浏览器
> 调研）、`docs/20`（浏览器元素吸附设计稿，本文 §11 给出针对它的评审意见）。
>
> 仓库状态（写作时）：`main` = `fbcfd8d`（= 标签 `baseline-2026-10-05-pre-browser-capture`）；
> 本轮尝试完整保留在分支 **`browser-capture-attempt`**（`53c5acd`），可 `git cherry-pick` 取回。

---

## 0. 结论摘要（先看这一节）

1. **只从顶层窗口根逐层下钻，到不了浏览器网页元素。** 实测：同一套 23 个浏览器断言用例，
   从窗口根下钻只有 **3/23** 通过；app 里表现为"确认成整个浏览器窗口"
   （日志：`snap confirmed hwnd=133230 selection=(63,159)->(3834,2082)`，而同一会话
   `refinement_published=8`）。
2. **两份能用的参考实现都不是"从根下钻"**：
   - `snow-ui-selector`（UIA）：仍然从窗口根走，但**候选顺序**取 provider 列表里**最后一个**
     含光标的子节点（`uia/cache.rs`: `children.hit_before(point, usize::MAX)`）；把我们的
     "面积最小者胜"换成这条，浏览器夹具立刻从 **3/23 → 21/23**，并且 Explorer 的控件级点位
     从 **3/25 → 12/25**（同一窗口、同一 25 点）。
   - `ScreenSnap-master`（Python + `uiautomation`）：**先问 provider**——
     `ControlFromPoint`（= `IUIAutomation::ElementFromPoint`），再只穿过"容器类型"下钻，
     最后沿父链上行成 `窗口 → … → 目标`。照它实现后浏览器夹具 **22/22**（69 次查询由命中路径
     服务），Explorer 无回归（12/25）。
3. **但 app 里浏览器仍然失败**（两版都确认成整窗）。命中路径在探针里通、在 app 里不通，
   原因尚未定位；缺的是**默认日志里的定位信息**（见 §8 第 1 条，这是下一步最该做的事）。
4. **两次"看起来能跑"的提交都被回退了**，教训是：**不要打包多个改动**，每条改动都必须同时过
   "浏览器夹具 + Explorer 点位网格"两侧门禁（§10）。
5. `ControlViewCondition` 树过滤（参考项目里有）**实测无效**：加与不加，Chromium 窗口根下
   都是 `raw=2 containing=2`，分数不变（3/23）。

---

## 1. 目标与不变量（沿用 docs/14、docs/18，不要重开一套）

- 窗口仍由 v1 `WindowSnapshot` 命中；UIA 只在 `refinement_worker` 的独立 COM 线程按需展开。
- 鼠标路径（`WM_MOUSEMOVE`）只做快照命中、距离计算、状态更新、合并重绘；**不得**同步调用
  UIA/MSAA/DWM/跨进程子窗口枚举。
- 80 ms 停稳才提交 refinement；单飞；邮箱容量 1 只保留最新点；结果必须校验
  `session_id + snapshot_epoch + request_id + WindowIdentity`。
- fallback 只能**细化**、不能变粗（`docs/18` 62baf87 的结论）。
- 深选失败不得清掉 v1 整窗目标；已确认选区不再被 hover/预预览改写。
- 目标矩形统一为**虚拟桌面物理像素**；UIA `BoundingRectangle` 就是物理像素，**不得再乘 DPI**。
- 预览矩形变化沿用 `RectTransition` 101 ms OutQuad；已确认选区不动画。

---

## 2. 参考实现调研（三份源码，含关键行号）

### 2.1 `snow-ui-selector`（Rust，Windows UIA）——"从根下钻"的正确写法

源码：`refer/snow-apps/snow-crates/crates/snow-ui-selector/src/windows/{uia.rs, uia/cache.rs, query.rs}`

| 位置 | 做法 | 为什么重要 |
| --- | --- | --- |
| `uia.rs` `NativeProvider::new` | `CUIAutomation8` + `IUIAutomation2` | 拿到 `SetConnectionTimeout`/`SetTransactionTimeout`：**每次调用**设成 `min(剩余预算, call_limit)`，这是它真正给单次 UIA 调用设上限的方式（我们此前以为 COM 调用无法设限） |
| 同上 | cache request：`TreeScope(Element \| Children)` + `SetTreeFilter(ControlViewCondition())` + `AutomationElementMode_Full` + 属性 `BoundingRectangle/IsOffscreen/ControlType` | `Element` **必须**包含（见 §6 的实测坑）；ControlView 过滤在本轮 Chromium 用例里**无效** |
| `uia/cache.rs::query` | `children.hit_before(point, usize::MAX)` → 取**索引最大**（最上层）的含光标子节点 | **这就是浏览器能用的关键**：Chromium 窗口根两个重叠 Pane 里，"更小"的那个是死叶子 |
| `uia/cache.rs::structural_alternative` | 无含光标子节点时，**仅当**当前节点是 `Pane/Group` 且与父同边界，才向**更早的兄弟**回溯；否则立即 `Complete` | 源码注释点名 Chromium："A redundant structural leaf may cover the content branch (for example in Chromium windows)" |
| `uia/cache.rs::expand` | 逐层 `BuildUpdatedCache`；离屏跳过；子矩形与窗口求交；`MAX_STEPS=80`、`MAX_RECTS=100`、`LINEAR_CHILD_LIMIT=16`（超过用 R-tree）；失败区分 `ProviderTimeout`/`ProviderFailure`，`retry_timeout` 时**每节点重试一次**，中途可 `DecodingPending` 增量发布 | 预算/取消/重试的完整形态 |
| `query.rs` | `QueryControl`：前台 `budget=call_limit=168 ms`、refinement `budget=1500 ms`、`call_limit=500 ms`、`publication_interval=32 ms`；`StopReason` 含 **`AccessibilityPending`**（树未就绪是独立结果）与 `DecodingPending` | "树还没建好"要当独立语义，别当"这个窗口没有树" |

**它自己就有浏览器夹具**：`examples/support/browser_fixture.html` = **250 个 `<fieldset>` × 4 个
`<button>`**，`examples/hit_bench.rs` 支持 `--points X Y …`，打印**整条 path** 与 p50/p95/p99。
注意它用的是**最朴素的标记**（原生 button/fieldset），不像 self-made 的 flex+ARIA 用例那样
依赖 Chromium 是否把某个 div 暴露成节点。

### 2.2 `snow_shot`（Qt/C++ 协调器）——调度与产品开关

源码：`refer/snow-apps/snow_shot/src/presentation/selector/screenshotselectorcoordinator.cpp`

- `QTimer` 单次精确计时、`remaining = 80 - (now - targetChangedAt)`；守卫
  `if (!ready || hitTestInFlight || hasPendingHitTestPoint || refinementSubmitted) return;`
  → **单飞 + 有未消费最新点时不提交**。
- `invalidateRefinement()` + `cancelRefinement()`：光标移动、按下、epoch 变化、会话结束即失效。
- **产品开关**：`screenshot/window_element_api`，schema 默认 **`"uia"`**，可选 `msaa`；
  也支持环境变量 `SNOW_SHOT_SELECTOR_BACKEND` / `SNOW_SHOT_UI_SELECTOR_BACKEND`
  （`screenshotselectorserviceclient.cpp` `configuredSelectorBackend()`）。
  → 即"某个 app 的树难缠"在参考产品里是**用户可切换的一个维度**。
- ⚠️ 它的一条调度规则**不能照搬**：`cancelRefinement()` 注释"Moving through complete cached
  paths must not touch the refinement worker"。我们照做时产生了
  `docs/18 §13`（父级→子级不切换）的缺陷，提交 `af5c666` 已改成**每个光标位置都重新查询**。

### 2.3 `ScreenSnap-master`（Python + `uiautomation`）——"先问 provider"

源码：`refer/ScreenSnap-master/core/window_uia.py`（单文件 535 行，全部逻辑在此）

完整流程（每步都有源码依据）：

1. Win32 先取光标下的**外部顶层窗口**（`top_window_at`）→ 作为目标窗口与上行终点。
2. **临时把自家遮罩设成命中穿透**：`set_click_through(hwnd)` 给窗口加 `WS_EX_TRANSPARENT`，
   `try/finally` 还原。原因写在注释里：**UIA 命中测试没有 Z 序概念**，不这么做只会命中自家遮罩。
3. `automation.ControlFromPoint(x, y)` → **`IUIAutomation::ElementFromPoint`**。
4. 校验：矩形必须覆盖该点；命中不能是自家遮罩（`own_control` 沿父链看 `NativeWindowHandle`
   的**进程号**）。
5. `deepest_at(hit)`：**只穿过容器类型**下钻。`CONTAINER_TYPES` 共 27 种
   （Pane/Group/List/ListItem/DataItem/Tree/TreeItem/Tab/TabItem/Table/DataGrid/ToolBar/Menu/
   MenuBar/ComboBox/Header/HeaderItem/SplitButton/Spinner/Slider/Calendar/Custom/Window/
   Document…），评分 `min(面积, 是否容器, -深度)`，并列时优先"有名字"的；
   `VISIT_BUDGET=60` 节点、`DESCEND_LIMIT=24` 层。注释：*"ControlFromPoint 对虚拟化列表往往
   只给到列表容器"* —— 这就是为什么第 5 步必须存在（资源管理器要它才能到文件项）。
6. `climb(control, handle, max_depth)`：沿父链上行收矩形，跳过无效/`<4px`、矩形去重、
   到目标窗口句柄为止、**不含桌面根**，最后 `reverse()` → `窗口 → … → 目标`。
7. 命中失败（例如打在自己遮罩上）→ `ControlFromHandle(窗口)` + 同一下钻兜底。
8. 熔断：单次查询 >0.4 s → UIA 暂停 1 s（它自己的"挂死 provider"对策）。

### 2.4 其它参考（仅作背景）

- `refer/shot-refer/Crisp-main/*`、`meazure-master/*`：v1 时代窗口/子窗口命中的交互参考，与浏览器
  元素无关。
- `refer/snow-apps/snow-ui-selector/src/windows/msaa.rs`：MSAA 路径不是"一次 hit test"，而是
  **反复 `accHitTest`**：每层问同一个对象"光标下面是哪个子节点"，`ChildId` 则取子对象继续，
  `SelfObject` 则停；`MSAA_MAX_HIT_PATH_STEPS=64`、`RECTS=100`、独立 worker + 168 ms 超时 +
  准入上限 2。**我们现有的 `msaa_provider` 只做了单次 hit test**（潜在改进点，未验证收益）。

---

## 3. SnapClip 现状（基线 `fbcfd8d`）

| 文件 | 职责 |
| --- | --- |
| `src-tauri/src/capture/window_detection/deep.rs` | `RefinementScheduler`（80 ms dwell、单飞、最新点、epoch/request 失效、缓存路径）、`DeepTarget`/`StopReason`/`QueryControl`（1500 ms 总预算 / 500 ms 单调用上限）、`REFINEMENT_INFLIGHT_TIMEOUT_MS` 看门狗、`preview_bounds`（预览回退策略） |
| `src-tauri/src/platform/windows/capture/uia_provider.rs` | UIA provider：`ElementFromHandle` → 按层批量展开 → 同边界结构容器处理 → 子窗口兜底 → 合并 |
| `src-tauri/src/platform/windows/capture/msaa_provider.rs` | `TimedCallRunner`（168 ms）+ 单次 `accHitTest` |
| `src-tauri/src/platform/windows/capture/refinement_worker.rs` | 独立 COM 线程、容量 1 邮箱、协作式取消、共享 request gate |
| `src-tauri/src/platform/windows/capture/overlay.rs` | dwell 定时器、预览绘制、命中穿透、`refinement_pending` 等待策略 |
| `src-tauri/src/capture/diagnostics.rs` | 会话汇总行（`refinement_*` 指标，含 `refinement_inflight_timeouts`） |

已验收（真机）：多次 F5 均能深选、预览不再回落整窗、Explorer 文件项级吸附、四项场景
（父级→子级 / 子级→父级 / A→B / 连续移动）。**浏览器元素级是唯一未通过的一项。**

---

## 4. 实验台（本轮的真正产出，务必保留）

### 4.1 浏览器夹具：页面自报盒子

`src-tauri/tests/fixtures/browser-element-demo.html`（在 `browser-capture-attempt` 分支上）

- **26 个绝对定位用例**：按钮/禁用按钮/链接/ARIA 按钮/图标链接/输入框/只读输入/复选/单选/
  switch/下拉/滑块/文本域/图片/标题/段落/列表+条目/表格+单元格/三层嵌套/覆盖层叠放/
  滚动容器+条目/iframe/canvas/内联 SVG/shadow DOM/details/contenteditable/`display:none`/
  `aria-hidden`/旋转盒/图片链接/进度条/空壳容器。
- 页面脚本把**自己量到的 `getBoundingClientRect()`** 写进**窗口标题**
  （`?truth=1` → `SNAPCLIP_TRUTH:{...}`）。探针用 `GetWindowTextW` 读回。
  → 几何只有一个事实来源，**不要手算 CSS 盒模型**（笔者错了三次：`<h2>/<p>/<ul>` 的默认
  margin 会让绝对定位元素偏 14–17 px；列表项/表格格高度由字体决定）。
- 期望语义：`self`（自己的盒子）/`<other-id>`（另一个夹具的盒子）/`none`（不应出现在树里）/
  `inside_self`（答案必须落在该元素内）/`covers_self`（元素未暴露，允许更粗但必须局部）；
  `optional:true` 只打印不断言。

### 4.2 浏览器探针 `browser_element_probe`（`#[ignore]`）

```powershell
cd src-tauri
cargo test --lib browser_element_probe -- --ignored --nocapture
```

它做的事（每一步都是踩坑后定下来的）：

1. **测试进程先 `set_per_monitor_v2_awareness()`**：否则 user32 会把 `GetClientRect` 虚拟化成
   逻辑像素，而 DWM 的 frame 是物理像素，两者差 1.5 倍，所有点位全错。
2. 启动自己的 Chromium：临时 `--user-data-dir`（**每次删除**，否则会"恢复上次会话"气泡并忽略
   `--window-size`）、`--force-device-scale-factor=1`（1 CSS px = 1 物理 px）、`--hide-scrollbars`、
   `--window-size` 要大于页面尺寸（外框包含标签栏/工具栏）。
3. **把窗口置顶**（`SetWindowPos(HWND_TOPMOST, …)`）：Chromium 的**无障碍树是惰性构建**的，
   窗口被遮挡或不在前台时会停止构建。
4. 等树就绪：找"视口节点"（与 client 同宽、贴底、顶边低于 client 顶边——实测 Chromium 的
   client 包含标签栏与工具栏，页面从这里往下 87 px 开始），并要求它内部至少 8 个节点。
   **10 s 内始终没就绪 → skip（不是 fail）**，否则环境问题会被误报成产品缺陷。
5. 逐用例：点位 = 视口原点 + 该用例自报盒子（+可选的 `probe` 偏移）；调用**产品走查**；
   打印 `expected / published / stop_reason / depth`；失败时再打印 raw 链与
   **系统自己的 `ElementFromPoint`** 结果。
6. 允许**有界重试**（Chromium 的树是惰性物化的，首次查询可能读到占位），并把
   `slow_fixtures` 计数打出来当观测数据。

### 4.3 Explorer 回归探针 `explorer_rule_probe`（`#[ignore]`）

```powershell
cargo test --lib explorer_rule_probe -- --ignored --nocapture
```

找到/打开一个 `CabinetWClass` 窗口 → 置顶 → 在窗口内取 **25 个点位**（5×5 网格）→ 对每个点打印
矩形与深度，并给出两个汇总指标：`median_area_pct`（中位面积占窗口比例）与
`control_level_points`（面积 < 20% 窗口的点数）。**改任何走查策略都要跑它**，用数字而不是肉眼判断
"有没有变粗"。

### 4.4 穿透探针 `overlay_hit_through_probe`（`#[ignore]`）

在**另一个线程**创建全屏 topmost 弹窗（模拟 overlay），对比"跨线程改 `WS_EX_TRANSPARENT`"与
"拥有线程改"时 `ElementFromPoint` 的答案。实测结论见 §6 最后两行。

---

## 5. 实测数据

### 5.1 候选顺序 / 树过滤（同一台机器，4K 3840×2160 @DPI144）

| 组 | 改动 | 浏览器夹具 | Explorer（25 点） |
| --- | --- | --- | --- |
| A | 基线：候选按**面积升序**（最小者胜） | **3/23** | **3/25** |
| B | A + `SetTreeFilter(ControlViewCondition())` | **3/23**（无变化） | 无变化 |
| C | A 改成 **provider 顺序反转**（最后列出者优先） | **21/23** | **12/25** |
| C+ | C + 文字块规则 + 环处理 + 每查询读树（**打包，已回退**） | 23/23 | 未测（打包提交后被回退） |
| D | C + **provider 命中测试**（ScreenSnap 机制，已回退） | 21/23 → 22/22※ | 12/25（命中路径在 Explorer 不可用，走兜底） |

※ 22/22 是把两个"树里根本不存在该盒子"的夹具改成 `inside_self`/`optional` 之后的数字。

**A 组失败原因（实测链）**：Chromium 窗口根 `raw=2 containing=2` 是两个重叠 Pane，其中**更小**的
那个 `children=0`（死叶子）；"最小者胜"于是钻进死叶子，`depth=3` 就结束 → 发布整窗/整页。

### 5.2 app 实机（两版都未通过）

| 版本 | 日志证据 | 结果 |
| --- | --- | --- |
| 候选顺序版（`71ce1aa`） | `refinement_submitted=9 refinement_published=8`，`snap confirmed hwnd=133230 selection=(63,159)->(3834,2082)` | 确认成整窗 |
| 命中测试版（`5744c91`+`53c5acd`） | 两次会话都是 `submitted=8 published=8` / `submitted=1 published=1`，确认同为 `(63,159)->(3834,2082)` | 确认成整窗 |

两版都**Explorer 正常**。app 与探针的差异尚未定位（见 §8）。

### 5.3 命中测试路径在探针里的有效性

`browser_element_probe` 在 D 组的有效运行：**69 次查询由 `ElementFromPoint` 命中路径服务**，
断言 22/22；`refinement hit hwnd=… box=… depth=…` 是成功标志行。

---

## 6. 实测踩坑清单（每条都花了时间，务必先读）

| # | 现象 | 结论 / 处理 |
| --- | --- | --- |
| 1 | 探针点位整体偏移 ~1.5 倍 | 测试进程不是 DPI 感知的：user32 虚拟化 `GetClientRect`，DWM 给物理像素。**先设 per-monitor-v2** |
| 2 | 五个探针点全落在 Chrome 的"重新加载"按钮上 | **client 原点 ≠ 页面原点**：Chromium 的 client 含标签栏+工具栏（本机页面原点比 client 顶边低 87 px）。页面原点要从无障碍树里读（页面节点矩形），或用页面自报 |
| 3 | 每次查询都 `raw=0` / 读到 `Pane(0,0)-(0,0)` | Chromium **无障碍树惰性物化**；窗口被遮挡/非前台时**停止构建**。置顶 + 等就绪；"没就绪"要 skip 不要 fail |
| 4 | 命中元素拿不到矩形（`no usable target`） | cache request 的 `TreeScope` 必须含 **`Element`**（`Element \| Children`）。只写 `Children` 时，被 `BuildUpdatedCache` 的元素**自身**没有缓存属性 |
| 5 | 归属校验 69 次命中 0 次通过 | Chromium **只在外层节点**暴露 `NativeWindowHandle`。改用 `CompareElements` 沿父链比对窗口根元素；句柄存在时按参考做法"相同即接受/不同即拒绝" |
| 6 | 链接/group 被选成"文字那一条" | Chromium 把 `<a>`/`role=group` 的**文字作为 `Text` 子节点**暴露（实测 `Hyperlink(168×56)` + `Text(56×20)`）。产品规则待定（§8 第 3 条） |
| 7 | 同一坐标连续两次查询结果不同（一次整页、一次元素） | 跨查询的 per-epoch 层级缓存是**陈旧快照**：空批次不能缓存；批次解释不了当前点时应重读；或每次查询重读（实测 2–7 ms/查询 vs 1500 ms 预算） |
| 8 | `EnumChildWindows` 拿不到 DOM 盒子 | 浏览器只暴露一个巨大的 render-host 子窗口；**不要**把它当元素 |
| 9 | 自建全屏 topmost 弹窗从不被 `ElementFromPoint` 返回 | 三种样式状态下都返回桌面元素（`hwnd=0x0`）。即"无子窗口的 DComp 弹窗"可能本来就不参与 UIA 命中——穿透只是保险 |
| 10 | Chrome 标题带 `" - Google Chrome"` | 从窗口标题读 JSON 时要按标记定位并截到匹配的 `}`，不能假设整串是 payload |
| 11 | 复用 profile 导致忽略 `--window-size` 并弹"恢复页面" | 每次用**全新/删除后的** temp profile；否则窗口小于页面，点位全落空 |
| 12 | 探针自身命令 | 构建输出被占用会 `LNK1104`（上一次测试进程没退）；跑探针前确保没有残留进程 |

---

## 7. 失败记录（为什么现在 `main` 是干的）

1. **尝试 1**（`7ecda71`）：把四件事**打包**提交——候选顺序 + 文字块规则 + 环只否决分支 +
   每查询读树。浏览器探针明显变好，但用户实测 **Explorer 回归**，按要求回退（`3967007`）。
2. **尝试 2**（`71ce1aa`）：只改**候选顺序**（provider 顺序反转）+ 删掉三个早已无调用点的
   死代码（`deepest_child_at`/`is_structural_container`/`may_try_sibling_branch`）及其测试。
   用户实测 **Explorer 正常**，浏览器仍整窗。测得 C 组数字（浏览器 21/23、Explorer 12/25）。
3. **尝试 3**（`5744c91` + `53c5acd`）：实现 ScreenSnap 机制 + 把穿透改到拥有窗口的线程 +
   加失败分步日志。探针 22/22、Explorer 无回归，但 **app 浏览器仍整窗**，随后按要求回退
   （`94d568f` 两个 revert，最后 `reset --hard` 回 `fbcfd8d`）。

**根因教训**：两次回归都发生在"打包提交 + 只用肉眼看"上。正确做法见 §10。

---

## 8. 未解问题（按优先级，交接重点）

1. **app 与探针的差异（最高优先）**：命中路径在探针通、在 app 不通。需要**默认日志**
   （不需要环境变量）就能分辨，建议在会话汇总里加两项：
   - `refinement_hit_paths=N`（命中路径成功次数，`WindowDetectionMetrics` 加计数器即可）；
   - 每个会话末尾强制打印一行"最后一次深选目标"：`hwnd/bounds/depth`（overlay 在
     `release_session` 里已有 `deep_target`，直接打）。

   这次尝试停在这一步（当时正在改 `diagnostics.rs`）。拿到这两项后，一次普通测试即可定位：
   命中路径计数为 0 → 修命中；计数 >0 但 bounds 是整窗 → 修上行/合并或 overlay 采用条件。
2. **Explorer 的命中路径不可用**（探针日志 `refinement hit unavailable`）：打印命中元素的
   `type/rect/hwnd/wanted` 与父链，判断是"命中了别的窗口（前台/置顶问题）"还是"控制视图里
   上溯不到窗口根"。
3. **文字块语义**：树里只有 `Text` 子节点时，发布文字块（ScreenSnap 的做法）还是发布它所属的
   元素框？这直接决定 `docs/20 §5.1` 的粒度规则；需要夹具覆盖 + Explorer 回归。
4. **层级缓存新鲜度**：至少要实现"空批次不缓存"，再决定是否取消跨查询缓存。
5. **Firefox（未验证）**：如果它不暴露元素级矩形，验收应写明"整窗降级即合格"。
6. **MSAA 路径**：参考实现是**迭代 `accHitTest`**，我们只有单次 hit test；可作为"UIA 树缺失"
   时的兜底增强（未测收益）。
7. **CDP/扩展通道**：只能覆盖我们托管的 WebView2 或用户显式安装的扩展；
   对"用户正在用的 Chrome/Edge"没有帮助，别把它当成浏览器元素吸附的主路径。

---

## 9. 建议的实施顺序（每条独立提交、独立验证）

| 步骤 | 内容 | 门禁 |
| --- | --- | --- |
| S1 | **只加观测**：`refinement_hit_paths` 计数 + 会话末"最后深选目标"行（不改行为） | `cargo test --lib` + 用户跑一次普通测试，把汇总行发回 |
| S2 | **候选顺序** = provider 顺序反转（`71ce1aa` 的内容） | 浏览器夹具 ≥21/23 **且** Explorer `control_level_points` ≥12/25 |
| S3 | **provider 命中测试**（`ElementFromPoint` + 归属校验 + 只穿容器下钻 + 上行成路径），保留根下钻兜底 | 浏览器夹具 22/22（命中行 >0）**且** Explorer 不降（命中不可用时应回落到与 S2 相同的 12/25） |
| S4 | **粒度策略**：交互控件优先、最小尺寸阈值、文字块规则 | 夹具覆盖 + Explorer 回归；必要时再加设置开关 |
| S5 | 稳定性与失效：滚动/导航/DOM 重排后失效、连续稳定才替换预览、确认前重新验证 | 手工场景（滚动、SPA、缩放、多显示器）+ 单测 |
| S6 | （可选）MSAA 迭代 `accHitTest`、WebView2/扩展 CDP 通道 | 见 §8 第 6/7 条 |

**硬性纪律**（本轮两次翻车的直接原因）：

1. **一次只改一条**，绝不打包；
2. 每条改动**必须同时**跑浏览器夹具与 Explorer 点位网格，数字不劣化才提交；
3. 涉及交互的改动，一定要让用户跑一次真机，并把**默认汇总行**作为证据；
4. 回退粒度按"单条改动"而不是"整个功能"。

---

## 10. 复现命令与产物清单

```powershell
cd D:\100_Projects\110_Daily\SnapClip\src-tauri
cargo test --lib                       # 基线 fbcfd8d：354 passed / 0 failed / 1 ignored
cargo check --all-targets              # 0 warnings

# 需要浏览器与（可选的）资源管理器窗口；都是 #[ignore] 的人工探针
cargo test --lib browser_element_probe   -- --ignored --nocapture   # 期望 asserted=22 passed=22（D 组）
cargo test --lib explorer_rule_probe     -- --ignored --nocapture   # 期望 control_level_points=12/25（C 组起）
cargo test --lib overlay_hit_through_probe -- --ignored --nocapture # 穿透/命中测试的环境实验
```

关键产物（都在分支 `browser-capture-attempt` = `53c5acd`）：

| 产物 | 位置 |
| --- | --- |
| 浏览器夹具（26 用例 + 自报盒子） | `src-tauri/tests/fixtures/browser-element-demo.html` |
| 三个探针 | `src-tauri/src/platform/windows/capture/uia_provider.rs` 的 `#[cfg(test)] mod tests` |
| 候选顺序修复 | 提交 `71ce1aa` |
| ScreenSnap 机制实现 | 提交 `5744c91` |
| 拥有线程穿透 + 失败分步日志 | 提交 `53c5acd` |
| 调研记录 | 基线版 `docs/18 §14`（第一轮浏览器调研 + 当时的探针）；`docs/18 §14.5/§14.6/§14.7`（候选顺序 A/B/C、ScreenSnap 机制、失败分步日志）**只在 `browser-capture-attempt` 分支上**，本文是它们的整理版 |
| 设计稿评审 | 本文 §11（针对 `docs/20`） |

回退基准：标签 `baseline-2026-10-05-pre-browser-capture` → `fbcfd8d`
（`git reset --hard` 到它即可，注意需要 `--force-with-lease` 推远端）。

---

## 11. 对 `docs/20-browser-element-snapping-design.md` 的评审意见

总评：**工程约束写得好（线程/不变量/降级/CDP 授权/测试矩阵都值得照做），但有两处承重决策照做
会在浏览器上仍然失败**；另有若干实测事实缺失，建议补进正文。

### 必须改

1. **§3.1 规则 1「不用 `ElementFromPoint` 重新选择窗口」**：这条堵死了唯一实测能到达浏览器元素的
   路径（根下钻 3/23；命中测试 22/22）。归属风险真实，但**可以证明**：`CompareElements` 比对窗口根
   （有效）；只用 `NativeWindowHandle`（无效，Chromium 只在外层暴露）。建议改成
   "命中测试作为候选来源 + 强制归属证明 + 失败回落"，并写明 Chromium 为何不能只靠根下钻。
2. **§5.1 候选排序**："具体控件优先 → 面积更小优先"在 Chromium 窗口根上会选中**没有子节点的
   那个 Pane**（两者都是容器、无法用"具体控件优先"区分）。必须加入"能继续下钻的分支优先 /
   沿用 provider 顺序（最后一个优先）"，并附实测数字。

### 建议补进正文

3. §2.2 把"缓存完整路径时不重新查询"列为"应吸收"——它与 `docs/18 §13`（父级→子级不切换的根因，
   已由 `af5c666` 删除）**直接冲突**，应改成"每个光标位置都要重新查询，靠 provider 展开缓存保性能"。
4. §7 的缓存规则（"按元素身份键控 + epoch 清空"）不够：**空批次不能缓存**，且批次解释不了当前
   点时应重读（实测同点两次查询结果不同）。
5. §8 步骤 1"立即显示整窗 hover/预览"与 `docs/18 §13.5`（不闪整窗）冲突，应对齐后者。
6. §5.1 需要明确"树里只有文字块时发布什么"（§8 第 3 条），并给出最小尺寸阈值的同时给出
   与 Explorer 的回归判据（否则阈值会把文件项滤掉）。
7. 把 §6 的踩坑清单（坐标、惰性树、`TreeScope`、句柄暴露、文字块）以"实测事实"形式写进 §5.2。
8. §10 建议把**自动夹具探针**列为强制项（本轮唯一真正有效的门禁），而不是只列人工验收条目。
9. Firefox：把"元素级吸附"标为待验证；若其 UIA 不暴露元素矩形，验收应写"整窗降级即合格"。
10. §3.3/B4 的 CDP 通道要写清成本与范围：需要 native messaging host + 扩展，只覆盖
    WebView2 或装了扩展的用户浏览器，**不解决"用户正在用的 Chrome/Edge"**。

---

## 12. 坐标与契约速查（避免重复踩坑）

- `Rect` 半开区间：`contains` 为 `[left, right) × [top, bottom)`；`intersect` 贴边不算相交。
- `MonitorLayout::to_local/to_screen` 只做平移；`window_rect_to_local` 再按本显示器裁剪；
  另一台显示器的窗口裁完必为空（单调平移 + 裁剪）。
- `DeepTarget { window, kind, screen_bounds, path, stop_reason }`：`path[0]` 是窗口外框、
  最后一项等于 `screen_bounds`；`kind` 只有在真的下钻到窗口之下时才是 `UiElement`。
- `StopReason`：`Complete / BudgetExhausted / TraversalLimit / ProviderTimeout / ProviderFailure /
  Cancelled / Unsupported`；参考实现多一个 `AccessibilityPending`（树未就绪）——我们目前把它
  当 `Complete` 处理，**这是缺失语义**。
- 预览回退策略（`docs/18 §13.5`）：答案覆盖光标 → 显示；同窗口有已验证矩形且在途 → 保留它；
  同窗口无可信矩形且在途 → 不画；等待结束 → 才回落整窗帧。
