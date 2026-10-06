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

> **2026-10-05 更新（S1/S2/S2b 已提交，真机通过）**：app 里"确认成整窗"的根因已定位并修复，
> 见 §5.4。S3（provider 命中测试）不再是浏览器可用的前提，降为增强项。

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
7. **要求跨域帧自报就绪**：夹具页 `file://`、跨域帧来自本机 HTTP 服务，父页面读不到那一帧，所以
   子帧自己 `postMessage` 报告（`cross-ready` / `cross-button`），探针 10 s 内拿不到就**直接失败**
   （不是 skip）：没有它，一个还没物化的帧和"进不去跨域帧"在测量上完全一样（§5.20 那次订正）。
   服务端按路径记录请求（`/cross.html`、子帧的 `/cross-ping`），用来区分"帧没要页面 / 脚本没跑 /
   消息没回来"。

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

### 5.4 根因：同边界结构包装 Pane 排在内容分支之后（S2b）

S1 的默认日志证明是**查询层**问题（`deep=… bounds=(63,159)->(3834,2082) depth=2`，
`refinement_superseded=0 refinement_downgrades_staged=0`）。对用户的最大化 Edge
（4K，frame `(0,0)-(3840,2088)`）逐层 dump：

- 根：两个同边界 Pane，第一个 `children=0`，第二个是内容分支 → "最后列出者优先"选对；
- 第 3 层：两个与父节点**同边界**的 Pane，**第一个**才是内容分支（下接 `Pane(61,156)…` 页面），
  **最后一个**是 `children=0` 的死包装 → "最后列出者优先"选中死包装，走查在 depth=2 结束。

所以只按面积、只按 provider 顺序都不够，必须有 snow-ui-selector 的 `structural_alternative`：
死胡同时，若当前节点是**与父节点同边界的 `Pane`/`Group`**，回到父层尝试更早的、含光标的
sibling，条件成立就继续上溯；真实控件与有独立边框的容器不回溯（不会把细结果变粗）。
实现：`uia.rs::is_structural_wrapper` + `WalkBudget::leave_children`，`uia_provider.rs`
的候选栈深度优先走查，回溯时打 `refinement backtrack` 日志。夹具窗口没有这个排列，所以
S2 在探针里 21/23、在真机里整窗——**夹具门禁覆盖不到窗口级树形差异，真机日志必不可少**。

| 项目 | S2 | S2b |
| --- | --- | --- |
| 用户 Edge (1625,776) | 整窗 depth=2 | `(1233,762)->(1689,797)` depth=10 |
| 浏览器夹具 | 21/23 | 21/23（nested-outer/inner 仍为文字块问题，S4） |
| Explorer 网格（同一窗口当日复测） | 9/25 | 11/25（0 次回溯，差异来自窗口内容；早先 12/25） |
| app 真机 Edge | 确认成整窗 | `snap confirmed selection=(1877,599)->(1982,661)` depth=10 |
| app 真机 Explorer | depth=3 内容区 | 文件项 `(1986,950)->(2229,979)` depth=5 |

遗留：Explorer 网格中部分点仍停在 depth=3 内容区 `(1527,466)->(3101,1437)`，与回溯无关，单独排查。（已在 §5.5 查清）

### 5.5 "偶发停在父级盒子"：三个独立根因

用户报告：鼠标停在控件上偶发不吸附或一直吸附父级，需要移出再移回。逐层 dump 后确认是三个
互不相关的问题，各自独立提交、独立验证。

1. **缓存键地址复用（浏览器）**。`children` 表以元素 COM 指针为键，但条目不持有父元素；根元素
   每次由 `ElementFromHandle` 新取、下钻后即释放，其地址会被新读出的子元素复用，于是该子元素被
   `visited` 当作"已走过"跳过，或拿到别的节点的子列表。表现为同一点连续两次全新查询得到控件 /
   页面 / 整窗三种结果。修复：`ExpandedLevel` 持有父元素。最大化 Edge 96 点网格两次查询不一致：
   34–58 → 0。**曾误判为 Chromium UIA 本身不确定——关掉缓存差异不减，是因为 `visited` 同样以地址判重。**
2. **后台标签页（Explorer）**。每个标签页是一个同尺寸、`WS_VISIBLE` 的 `ShellTabWindowClass`
   子窗口，只靠 z-order 区分；UIA 不标 offscreen，且把后台标签排在后面，"最后列出者优先"走进
   用户看不见的标签，其行布局不同，所以"有的行能吸附、有的行只到内容区"。修复：自带窗口的候选
   需 `ChildWindowFromPointEx(CWP_SKIPINVISIBLE)` 确认是该点实际显示的子窗口
   （`win32::is_shown_child_at`）。九标签窗口 120 点：6 点由内容区变为文件行。
3. **无矩形分组节点（Explorer 导航窗格）**。"桌面"等分组节点报告 `(0,0)-(0,0)`，其下的树项有真实
   矩形；走查把它当叶子，停在整个导航窗格。修复：本层没有任何带矩形的候选包含光标时，才按
   provider 顺序（最后列出者先）透视无矩形子节点，计入节点预算。同一窗口：13 点由整个窗格变为具体条目。

| 门禁 | 修复前 | 修复后 |
| --- | --- | --- |
| `cargo test --lib` | 353 | 357 通过（新增补查 3 项、叠放子窗口夹具 1 项） |
| `browser_element_probe` | 21/23 | 21/23 |
| `explorer_rule_probe` | 12/25 | 12/25（固定 25 点不落在修复的区域） |
| app 真机 | 场景 1 偶发失败 | 用户确认 Edge 与 Explorer 正常 |

仍会落到内容区的点（列右侧空白、末项下方）在 UIA 树里确实没有更小的包含元素，按"最内层可捕获
区域"定义内容区即为正确答案。

### 5.6 收尾四项（2026-10-06）

**A. 带边框容器认领自己的文字块**（`4d94077`）

`nested-outer`/`nested-inner`（`role=group` 的 span）过去期望 192×72、实际拿到内部的
`Text(70×20)`。这条规则的三种形式实测差别很大：

| 形式 | 浏览器夹具 | Explorer（确定性夹具 25 点） |
| --- | --- | --- |
| 不认领（基线） | 21/23 | 12/25 |
| 交互控件 + 带边框容器都认领 | 23/23 | 6/25（**当时是内容混淆，见 C**） |
| 只有交互控件认领 | 21/23（group 那条仍失败） | 6/25（同上） |
| **只有带边框 `Pane`/`Group` 认领（采用）** | **23/23** | **12/25（开关各测 2 次，稳定）** |

为什么不能更宽：Explorer 把整个虚拟化列表暴露成**一个 `DataItem`**，它的 `Text` 子节点细得多；
让交互控件认领文字会把答案变成整个列表。单测 `a_text_run_inside_an_element_is_not_a_target`
覆盖两侧（框架认领；按钮/链接/ListItem/DataItem/文档/窗口不认领）。

**B. 空层不缓存**（`4d94077`）

Chromium 的树**惰性物化**：同一节点先答 `raw=0`（或占位 `Pane(0,0)-(0,0)`），片刻后才列出子节点。
旧代码 `entry().or_insert_with()` 把空层写进表里，于是**整个 snapshot epoch** 都在回答粗框——这是
"偶发停在父级/整窗"的第四类独立成因（前三类见 §5.5）。现在空层只用于当前查询、不写入表，下次
查询重读；`an_empty_uia_level_is_never_remembered` 在真实窗口上断言"表里不存在空层"。

与 `docs/18` 的关系：**没有新增 `StopReason` 变体**（也不需要）——恢复由"空层重读 + 下一次
dwell"完成，探针侧由"等树就绪、否则 skip"完成。

**C. 门禁确定性**（`e8ebf87`）

`explorer_rule_probe` 过去量的是**用户自己的** Explorer 窗口：同一份代码在不同日期/文件夹上给出
9/25、11/25、12/25。一次规则对比因此把 12 看成 6，几乎把我引向错误的回退（"这条规则让 Explorer
变粗"）。现在探针**自己打开**一个临时目录（24 个固定文件）并只测那个窗口 → 连续两次 12/25。

新增纪律：**改任何走查策略前，先用同一构建连测两次确认门禁本身可重复**。

**D. 延迟数据**（`e8ebf87`）

| 探针 | 查询数 | p50 | p95 | max |
| --- | --- | --- | --- | --- |
| 浏览器夹具（Chromium 页面） | 70 | **13.3 ms** | 26.4 ms | 30.0 ms |
| Explorer（确定性夹具，25 点） | 25 | **51.2 ms** | 176.6 ms | 221.0 ms |

都远在 `REFINEMENT_BUDGET_MS = 1500 ms` 之内；**Explorer 是最贵的场景**（p95 176 ms 超过 80 ms
dwell，但预览等待上限是 320 ms，所以不会闪整窗）。成本项是"每个自带窗口的候选 3 次 user32 调用
（`is_shown_child_at`）+ 回溯 + 无矩形透视"；当前没有 profiling 依据，不建议先优化。

### 5.7 精度补足：走查停得偏高时采用 provider 的命中框（2026-10-06）

**问题（用户原话）**："能不能更精确？识别鼠标指针所在的最内层可捕获区域——盒子里没有盒子，就是最
内层。"

**客观基准**：`IUIAutomation::ElementFromPoint`（参考实现里的 `ControlFromPoint`）按构造返回**该点
最内层元素**，正是产品要的那个定义。所以把它当尺子：每个采样点同时问

- 我们的走查答案（`refinement published`），
- provider 自己的命中框（`[probe] precision:` 行的 `provider_hit_available` / `provider_hit_is_finer_on`）。

"provider 命中框**更小、且仍含光标**" = 走查停在比最内层更粗的一层。归属校验照 §5.3：命中框必须能用
`CompareElements` 沿父链回到该窗口根元素（浏览器 43/43 点都能校验成功）。

**实测（同一构建，4K 3840×2160 @DPI144）**：

| 采样点 | provider 命中可用 | provider 比我们更细（补足前） | 补足后仍更细 |
| --- | --- | --- | --- |
| 浏览器夹具（43 点） | 43/43 | **4** | **3** |
| Explorer 确定性夹具（25 点） | 25/25 | **0** | **0** |

补足前那 4 个点：

1. `nested-outer` / `nested-mid` / `nested-inner`：provider 答内部的 `Text(70×20)`，我们答
   `192×72` 的容器——**有意不采用**，这是 §5.6 A 的产品决策（盒子 vs 盒子里那条字），不是精度缺口。
2. `input-disabled`：provider 答 `284×40` 的 `Pane(50033)`（Chromium 给这个只读输入框画了一层无名
   Pane），我们答 `298×101` 的容器——**真的精度缺口**，且**系统的命中测试也说那层 Pane**
   （`system hit test: Pane(1295,148)-(1579,188) ""`）。补足后日志出现
   `refinement adopted provider box=(1295,148)->(1579,188) type=50033 (walk was coarser)`，该点不再
   计入 `provider_hit_is_finer_on`。注意这一行仍不满足夹具"等于页面自报 240×56"的期望（`optional`，
   从不参与断言）：我们现在的答案是"OS 认为该点最内层的那个盒子"，而夹具注释记的是**页面**几何，
   两者在 Chromium 这层无名 Pane 上本来就不一致。

**规则**（`should_adopt_provider_box`，纯函数，单测
`the_providers_finer_box_is_adopted_unless_it_is_a_text_run`）：

```text
采用 provider 命中框 ⟺ 命中框含光标 ∧ 控制类型 ≠ Text ∧ 面积严格小于走查答案
```

- **只可能更细**：等于或更大的框一律不采用——走查答案已经表达了层叠顺序与回溯规则，原始命中框
  对这些一无所知。
- 代价：每次查询多一次 `ElementFromPoint` + 上行 `CompareElements`。补足后的延迟：浏览器
  n=70 p50 **16.8** / p95 26.3 / max 27.3 ms（补足前 13.3 / 26.4 / 30.0）；Explorer n=25 p50
  **58.6** / p95 67.3 / max 159 ms（补足前 51.2 / 176.6 / 221.0）。都在 1500 ms 预算内。

**遮挡问题（`ElementFromPoint` 没有 Z 序概念）**：overlay 全屏置顶时会自己答自己。两层处理：

只有**一个**杠杆有效：overlay 自己的窗口过程在 `WM_NCHITTEST` 里返回 `HTTRANSPARENT`。
实现见 `HitTestPassThrough`（`win/window.rs`）：refinement 线程在**那一次** `ElementFromPoint`
调用前后取/放一个 guard，overlay 线程的窗口过程读它——跨线程的只有这一个原子布尔。

为什么是它、而不是样式：把替身做到和真 overlay 一样（自注册窗口类 + `HTCLIENT`）之后，各杠杆的
实测答案（`browser_element_probe` 的 `[overlay]` 阶段，本分支可复跑）：

| 杠杆 | `ElementFromPoint` 的答案 |
| --- | --- |
| 什么都不做（overlay 压在上面） | **overlay 自己** |
| `WS_EX_TRANSPARENT` | overlay 自己——**无效**（本项目曾据此实现过一版，等于没做） |
| `WS_EX_LAYERED \| WS_EX_TRANSPARENT` | 页面元素（但 DirectComposition 窗口不能开 LAYERED） |
| `WM_NCHITTEST → HTTRANSPARENT` | **页面元素（采用）** |
| 在窗口区域上挖一个洞 | overlay 自己——无效 |

两个细节是必须的，不是修饰：

- **guard 只包那一次 `ElementFromPoint`**，不是整段查询。`HTTRANSPARENT` 对真实鼠标输入同样生效，
  所以暴露窗口越短越好：整段查询实测 20–50 ms，足够吞掉"停下鼠标后紧接着的那一次点击"。
- **按住鼠标键时否决穿透**（`GetAsyncKeyState` 读物理按键，不看本线程处理到哪条消息）：查询完全可以在
  框选拖拽停顿的瞬间发起，此时一次按下/抬起绝不能漏给下层窗口。

即使这些都失效，规则本身也挡住最坏情况：overlay 的框永远比走查答案**更大**，过不了"严格更小"的门槛，
只是这一次少了补足，不会发布错误的框。

> **订正**：本节曾根据一个**用系统 `STATIC` 类**做的替身窗口断言"我们的置顶全屏窗口根本不被命中测试
> 看见"。那个结论是错的——UIA 会跳过系统类的替身，却会老老实实返回自注册类的窗口（真 overlay 就是
> 这一类）。实测见 §5.8 与 §6 第 9 行。这个错误的代价是一整轮诊断：探针与实机据此不一致。

**门禁（补足后，本机连续一次）**：

| 门禁 | 结果 |
| --- | --- |
| `cargo check --all-targets` | **0 warnings** |
| `cargo test --lib` | **361 passed / 0 failed / 2 ignored** |
| `browser_element_probe` | 断言 **23/23**，`provider_hit_available=43`、`provider_hit_is_finer_on=3`（补足前 4） |
| `explorer_rule_probe` | `control_level_points=12/25`、`median_area_pct=65.8`、`provider_hit_is_finer_on=0`——**与补足前逐位一致** |

### 5.8 实机现象：遮罩答了每一次命中测试（2026-10-06，已定位并修复）

**报告**：某个 SPA 页面上，`div.group/side-pane-shell-host … flex-1 has-[[data-side-pane-shell-transition]]:overflow-x-clip`
这个盒子**内部的元素识别不到**。

**决定性证据**（`91e6b08` 加的强制行，用户下一次运行就拿到了）：

```text
refinement_precision_unavailable=14
last precision unavailable answer "SnapClipCaptureOverlay" 3840x2160 is not a descendant of hwnd=34734272
```

**14 次查询、14 次都由我们自己的 overlay 回答**（`class="SnapClipCaptureOverlay"`、整屏
`3840x2160`）。也就是说：精度补足在实机里一次都没生效过；而探针里因为**没有 overlay**，同一份代码
一切正常——这正是 §5.2/§8 一直没定位的"实机与探针不一致"。

**为什么之前会误判**：先前的替身窗口建在系统 `STATIC` 类上，UIA 会**跳过**它（哪怕它是置顶窗口、
是前台窗口、还是 `WindowFromPoint` 的答案）；而 capture overlay 用的是**自己注册的窗口类**。把替身
改成自注册类 + `HTCLIENT` 之后，故障逐字复现（§5.7 的杠杆表就是在那之后测的）。教训：
**替身必须复制被测对象的类，不只是它的样式**——样式相同、类不同，UIA 的行为完全不一样。

**修复**：`fc9abb3`。overlay 的窗口过程在 `WM_NCHITTEST` 里按 `HitTestPassThrough` 返回
`HTTRANSPARENT`，refinement 线程只在一次 `ElementFromPoint` 期间置位（并在按住鼠标键时否决）——
细节与理由见 §5.7。原先那套 `WS_EX_TRANSPARENT` 状态机连同它的三条复位路径一起删掉了。

**剩下的一半（仍需实机确认）**：遮罩不再挡住命中测试之后，用户那页会落到两种结果之一——

1. `adopted …`：走查虽然停在整窗（`depth=4, Complete`），provider 的更细盒子成了答案 → 现象消失。
2. `not-finer provider=… walk=…`：provider 的盒子没比整窗更细。**这一类的第一种成因不是"树里没有
   更细的节点"，而是"它的盒子是布局盒"——见 §5.9**；只有排除了那一类，剩下的才是"那个点确实没有更细
   的可捕获节点"（Tailwind 这类 SPA 大量无 role 无文本的 `<div>` 会被无障碍树剪掉；夹具里
   `checkbox`/`radio`/`para`/`table-cell-1`/`code-box` 五个 `optional` 行就是这个形状：走查与
   `ElementFromPoint` 都答整页 `1784x1125`）。若是后者，UIA 路线到此为止——要按 DOM 盒子捕获就必须
   换数据源（用户自己 `docs/20` 里设计的那条路）。

两种都会在**每次会话强制输出**的 `last precision …` 行里明确写出来，不再需要 verbose。

**这一轮之前的日志（保留作对照）**：

```text
refinement_submitted=47 refinement_published=44 refinement_empty=0 refinement_downgrades_staged=3
last deep target hwnd=34734272 kind=UiElement bounds=(63,159)->(3834,2082) depth=4 reason=Complete
```

走查**走了 4 层、以整窗边界结束**（`reason=Complete`），`(63,159)->(3834,2082)` 是窗口框而不是页面
web area——正是 §5.4 记录的"同边界结构包装 Pane 排在内容分支之后"形态；在这种形态下本该由精度补足
救回来，而它当时一次也没能运行（`unavailable=14`）。

**为什么这份旧日志读不出原因（已修）**：补足的判定过去只在 `SNAPCLIP_WIN_DETECT_VERBOSE` 下打印，
于是"跑了但什么都没做"和"根本没跑"在日志里一模一样。`91e6b08` 起改为**每次会话强制**输出
（与 `last deep target` 同级），也就是本节开头那两行：

```text
[snapclip][win-detect] last precision adopted|not-finer|unavailable provider=WxH at (x,y) type=N class="…" walk=WxH at (x,y)
… refinement_precision_adopted=N refinement_precision_not_finer=N refinement_precision_unavailable=N
```

`class` 是 Chromium 给出的 **DOM class**（夹具实测 `class="fixture cap"`），所以这一行会直接点名
provider 认为"最内层"的那个盒子是哪一个。

补充：同一次运行若带上 `SNAPCLIP_WIN_DETECT_VERBOSE=1`，`refinement level #N node=… parent=… raw=…
empty=… offscreen=… containing=…` 会逐层说明走查是在哪一层、因为什么（子节点数为 0 / 没有子节点含光标）
停下来的。

### 5.9 可见部分：布局盒，以及"更细"的判据（2026-10-06）

遮罩修好之后（`fc9abb3`）用户的第二次运行给出了**下一层**的证据：

```text
refinement_precision_adopted=3  refinement_precision_not_finer=26  refinement_precision_unavailable=0
last precision not-finer provider=1153x22623 at (1567,-9870) type=50026
                class="min-h-8 text-message relative flex w-full flex-col items-end gap-2 text-start
                       break-words whitespace-normal outline-none keyboard-focused:focus-ring
                       [.text-message+&]:mt-1"  walk=3771x1923 at (63,159)
```

**几何是对的，判据是错的。** 那个 `class` 是**一条消息气泡**，22623 px 高、起点在屏幕上方——它就是
光标底下的元素（一条很长的回答），只是 Chromium 报的是它的**布局盒**（含滚出视口的部分）。我们的规则
拿"未裁切的面积"去比整窗（26M > 7.25M），于是把一个**比整窗更具体**的盒子判成"更粗"，退回了整窗。

**修法（`fa76cc6`）**：候选框先按**祖先链 ∩ 窗口**裁成"可见部分"，再比较、再发布。

- 对出现在视口内的普通元素，裁切是恒等操作：**零额外开销**（逐个祖先读矩形是跨进程调用，实测在
  Explorer 的深链上会让 p95 涨 100 ms）。所以只有"原始框超出窗口"时才走那条路。
- 裁切只在**结果仍含光标**时生效：Explorer 的虚拟化条目会报空/过时矩形（docs/18 §12.7），
  一个和命中测试矛盾的祖先不允许把答案裁没（第一版没有这个守卫，实测
  `provider_hit_available=24/25`，命中测试白白变成"不可用"）。
- 走查自己发布的每一层同样按上一级裁切（同一个形状也会从走查侧漏出来）。

**门禁**：夹具新增 `deep-scroll-box`/`deep-item` —— 一个 20000 px 高的元素放在 100 px 高、已滚到
5000 px 的滚动视口里，Chromium 实测报 `358x20000 at (73,-3983)`（与用户那页同形），修复后发布
`358x100`（视口内的可见部分）：

```text
[probe] assert deep-item  expect=inside_self  expected=358x20000 @(73,-3983)  published=358x100 @(73,1017)  OK
```

同时探针把三条不变式都变成**断言**（不只是打印）：更细必被采用、每个采样点的命中测试必须可用、
**任何发布的框都必须在窗口内**（截图只能包含屏幕上的内容）。

> **订正**：`91e6b08` 的提交信息声称探针已经"断言"了前两条，实际当时只打印、没有断言（这个提交
> 补上了，并新增第三条）。以代码为准。

### 5.10 无障碍树的天花板，以及其它通道（2026-10-06，调研 + 实测）

**问题（用户）**："除了 Chromium 的无障碍树，还有更好的方案吗？"

**先把天花板钉死：Blink 在把树交给平台之前就把"不感兴趣"的节点丢掉了。**

- Blink 自己的文档（`third_party/blink/renderer/modules/accessibility/readme.md`）："An 'ignored'
  accessibility object is one that **will not be exposed in platform accessibility APIs** to
  assistive technologies."——常见的被忽略原因里明确列着 **"Uninteresting Content"：没有额外 ARIA
  信息的 `<span>`/`<div>` 这类布局包装**。
- Chromium 的 UIA 文档（`docs/accessibility/browser/uiautomation.md`）讲的是 `IsControlElement` /
  `IsContentElement` 决定的 **control view / content view / raw view**，其中 **raw view 是 control view
  的超集**——所以"换 raw view 就能多看到东西"曾经是最有希望的一条路。
- 于是 `browser_element_probe` 新增 `[sources]` 阶段，用夹具里专门加的一个"裸 div"（`plain-div`：无
  role、无 label、无自身文本，只有内部一个带文本的 span）实测三条通道：

```text
[sources] plain-div point=(698,1096)          # 点在裸 div 内部、远离那个 span
[sources]   control hit : Document(48,107)-(1832,1232)
[sources]   raw view    : 158 nodes, 154 with a rectangle, 0 raw-only, 8 contain the point, 45 ms
[sources]   raw smallest: Pane(48,107)-(1832,1232) class="Chrome_WidgetWin_1"
[sources]   text range  : enclosing=Text(469,1047)-(501,1066)   # 229 px 外的那条文本
```

**结论**：

1. **raw view 一点忙也帮不上**：Chromium 的 raw view 里 `raw-only` 节点数是 **0**（158 个节点全都在
   control view 里），这个裸 div 在两棵树里都不存在。因为它不是"`IsControlElement=false`"，而是
   **根本没进平台树**。
2. **`TextPattern.RangeFromPoint` 不是命中测试**：光标下没有文本时它返回**最近的**文本跑条
   （上面那条离光标 229 px），所以它不能用来回答"这是哪个盒子"；而且粒度是文本，产品本来就决定不选
   文本。
3. **MSAA 与 UIA 是同一棵树**：`AXPlatformNodeWin::accHitTest` 只是用 Blink 的 `HitTestSync` 拿到
   *无障碍节点*（z-index/overflow 更准），节点集合与 UIA 相同；`BrowserAccessibility::accLocation`
   同样返回**未裁切**矩形（`GetUnclippedScreenBoundsRect`）。

**那么还剩哪些通道**（按"能不能拿到真正的 DOM 盒子"排序）：

| 通道 | 能拿到什么 | 代价 / 风险 | 结论 |
| --- | --- | --- | --- |
| **CDP**（`DOM.getNodeForLocation` + `DOM.getBoxModel`） | 真正的 DOM 命中测试 + content/padding/border/margin 四组 quad；`DOMSnapshot.captureSnapshot{includeDOMRects:true}` 还能一次拿到整页盒表 | 需要浏览器带 `--remote-debugging-port` 启动；**Chrome 136 起默认 profile 上该开关被拒**（必须配 `--user-data-dir`，企业策略 `RemoteDebuggingAllowed` 可放行）；自己拉起的 profile **没有用户的登录态** | 精度最高；适合"应用自己托管/自己启动浏览器"的场景 |
| **伙伴扩展**（content script：`document.elementFromPoint` + `getBoundingClientRect`，经 native messaging 回传） | 同样的 DOM 盒子，且跑在**用户自己的浏览器会话**里 | 要用户装扩展 + 注册 native host；`chrome.debugger` 版本还要 `"debugger"` 权限并会弹"正在调试此浏览器"警告条；MV3 service worker | 唯一能覆盖"用户已登录页面"的路 |
| **自己托管 WebView2** | 完整 DOM/CDP | 只覆盖 SnapClip 自己开的窗口，用户不用它浏览 | 只对产品自己的窗口有意义 |
| **UIA / MSAA（现状）** | Chromium 认为"有趣"的节点 + 其祖先；几何未裁切（本仓库已修成取可见部分） | 零安装、零权限、跨浏览器 | **继续作为默认**：本轮之后它已经能用（遮罩穿透 + 可见部分 + 精度补足） |
| **视觉分割（像素/OCR）** | 视觉上的框（边框、色块、文本行） | 与 DOM 无关，纯启发式；已有 OCR 管线可复用 | 只作兜底/辅助，不做主路径 |

**给下一轮的建议**：`plain-div` 这类"裸 div"是 UIA 的**硬边界**，不值得再在无障碍树上投资。要真正跨过
它只有两条路——**应用自己启动浏览器（CDP）**或**伙伴扩展**。两者都改变产品形态（登录态 / 安装步骤），
所以这是产品决策，不是实现细节：先决定"要不要为 DOM 精度引入一条需要用户配合的通道"，再动手。

> **本节结论已被 §5.11 取代**：那句话只对"UIA 这一条通道"成立。**MSAA 的 `accHitTest` 能拿到 UIA 拿不到
> 的节点**（Chromium 侧走的是渲染进程真正的命中测试），而且不需要 CDP、扩展或任何用户配合。
> 先看 §5.11。

### 5.11 PixPin 为什么做得到：MSAA，而不是 UIA（2026-10-06，实测）

**背景**：用户指出 PixPin 在同一页面上能做到元素吸附。安装目录 `C:\A_Softwares\PixPin` 的取证结果：

- 进程只加载 `UIAutomationCore.dll`、**`OLEACC.dll`**、`UiSpy.dll`、`UiRegionDetector.dll`、
  `PixWindowNotify.dll`、Qt、以及 `PixVision.dll`（OpenCV 4.13 全量构建）——**没有** WebView2、
  没有浏览器扩展、没有任何 CDP/DOM 通道的痕迹。
- `UiSpy.dll` 的符号与字符串：`CarpUIElement{UIA,HWND}`、`WinGetWinRectByPointUIA`、
  `DirectGetRect`、`AccessibleObjectFromWindow`、`WindowFromPoint`、`ChildWindowFromPoint`、
  `IsHungAppWindow`，以及字面量 **`chrome.exe`** 和
  **`Failed to get IAccessible for Chrome window:`**；它的日志里刷屏的是
  `[UiSpy::DirectGetRect] accLocation failed or returned invalid rect` 与
  `get_CurrentBoundingRectangle failed ... use WindowFromPoint rect instead`。
  → **它就是"UIA 取矩形 / MSAA 取矩形 / WindowFromPoint 兜底"三级阶梯，并且给 Chrome 单独走了
  `IAccessible` 分支。**
- `UiRegionDetector.dll` 的导入表与 UiSpy 相同（**不导入** PixVision/OpenCV），所以区域检测器同样是
  UIA/MSAA 通道，视觉库只服务于 PixPin 的其它功能（OCR、自动马赛克等）。

**为什么 MSAA 能看到 UIA 看不到的盒子**（源码依据）：

| 通道 | Chromium 侧实现 | 结果是 |
| --- | --- | --- |
| UIA `ElementFromPoint` | 由平台树（Blink 交给平台的节点）按矩形比较回答；`IsControlElement/IsContentElement` 决定 control/content/raw view，而 raw view 仍是它的超集 | 只能看到"有趣的"节点；裸 `<div>`/`<span>` 被 `AXObject::ComputeIsIgnored` 判为 ignored，**根本不进平台树** |
| MSAA `accHitTest` | `BrowserAccessibilityWin::accHitTest` → `CachingAsyncHitTest` → **渲染进程真正的命中测试**（`HitTestSync`） | 返回 DOM 命中节点对应的**"ignored but included in tree"** 无障碍对象：**带几何、能被 MSAA 看到** |

**实测（夹具 8 个点，页面自报的盒子做基准）**：

```text
[sources] plain-div    control hit=Document         msaa window=300x96  at (448,1027)  ← (448,1027) 300x96 ✓
[sources] checkbox     control hit=Document         msaa window=168x56  at (72,211)    ← (72,211)  168x56  ✓
[sources] radio        control hit=Document         msaa window=168x56  at (256,211)   ← (256,211) 168x56  ✓
[sources] para         control hit=Document         msaa window=392x88  at (480,315)   ← (480,315) 392x88  ✓
[sources] code-box     control hit=Document         msaa window=300x120 at (864,907)   ← (864,907) 300x120 ✓
[sources] table-cell-1 control hit=Document         msaa window=165x35  at (1187,344)  ← (1187,344) 165x35 ✓
```

8/8 逐像素命中，其中 `plain-div` 是**任何无障碍树里都不存在的布局容器**。代价：`accHitTest` 只要
**1 ms**（两次调用即到底）。

**两个把它接到产品里的关键细节（都已实测）**：

1. **不要用全局命中测试**。`AccessibleObjectFromPoint` 会被截图遮罩挡住，而且——
   **`WM_NCHITTEST → HTTRANSPARENT` 对 MSAA 无效**（它只让 UIA 穿透）：

   ```text
   [overlay] overlay +HTTRANSPARENT:  UIA=Button(页面)  |  MSAA=role=0xa 遮罩 3840x2160   ← 只有 UIA 过了
   [overlay] overlay +LAYERED|TRANS:  UIA=Button(页面)  |  MSAA=role=0x29 "Button One" 74x16
   ```

   只有 `WS_EX_LAYERED|TRANSPARENT` 能让 MSAA 穿透，而 DirectComposition 窗口不能用。
   正确做法是 PixPin 的做法：**拿目标窗口自己的 `IAccessible`**
   （`AccessibleObjectFromWindow(hwnd, OBJID_CLIENT)`）再调 `accHitTest`——没有全局命中测试，
   遮罩自然不参与（探针里 `msaa window` 那一行就是这么来的，1 ms、`depth=1`）。
2. `accLocation` 返回**未裁切**矩形（夹具里那个 20000 px 高的元素回 `358x20000`），
   所以 §5.9 的"可见部分"规则仍要在 MSAA 结果上再跑一遍。

**实施计划（下一步，尚未动手）**：在 refinement worker 里加一个 MSAA 命中源
（`OBJID_CLIENT` → `accHitTest` → `accLocation`，含 `OBJID_WINDOW` 兜底与 Chrome 分支），
按"严格更细才采纳、跳过裸文本角色（`ROLE_SYSTEM_TEXT/STATICTEXT`）、按祖先链裁可见部分"接入现有精度
补足，并把现成的 6 个 `optional` 夹具行（`plain-div`/`checkbox`/`radio`/`para`/`code-box`/
`table-cell-1`）**从"打印"改成"断言 `self`"** 作为门禁。Explorer 12/25 与延迟需要同时回归。

**订正（2026-10-06，`aa6acdc`）：隔离必须落在命中测试本身，而不是它的调用方。**

实机日志里出现了 `refinement_msaa_attempts=21 refinement_msaa_timeouts=13
refinement_quarantine_added=1`：隔离名单只加了一个窗口，超时却有 13 次——**隔离没起作用**。
根因是位置放错了：隔离检查写在 `MsaaHit::resolve()` 里，而"这个窗口是不是已经被隔离"要到
`hit()`（真正调用 `AccessibleObjectFromWindow` + `accHitTest` 的那一层）才算数。于是每次精度补足
都会**再问一次**那个已经卡死过的窗口，`TimedCallRunner` 每次都等到 168 ms 截止才放弃——一小时里
13 次超时、每次 168 ms 的机会成本，全部花在同一个已知卡死的窗口上（日志里 `refinement_elapsed_us
last=217950 max=246968` 也是同一条路径）。

修法：`MsaaHitFailure::Quarantined` 成为 `hit()` 自己的失败态，在一个查询内**一次**命中就短路，
`resolve()` 把它映射成 `Empty(Unsupported)`（与"这个窗口答不了"同义）。单测
`an_already_quarantined_window_is_answered_without_touching_the_provider` 现在同时断言 `hit()`
本身返回 `Quarantined`，即"不碰 provider"。**验收判据**：同一个窗口在一场会话里最多超时一次，
`refinement_msaa_attempts` 与 `refinement_msaa_timeouts` 不再同步增长。

### 5.12 浏览器扩展路线：`refer/smart-screenshot-main` 的实现（2026-10-06，源码调研）

用户提供的第二个参照物是 Chrome 扩展"精准截图"（MV3，`host_permissions: <all_urls>` +
全站 content script）。它的"智能识别页面元素边界"核心只有三行（`content/content.js`）：

```js
// handleInspectorMouseMove (content.js:4209)
this.eventBlocker.style.setProperty('pointer-events', 'none', 'important');   // ① 临时让遮罩对命中测试透明
const element = document.elementFromPoint(e.clientX, e.clientY);              // ② 浏览器自己的 DOM 命中测试
this.eventBlocker.style.setProperty('pointer-events', 'all', 'important');    // ③ 立刻恢复
if (element) { this.updateHighlight(element); this.currentElement = element; }

// updateHighlight (content.js:4233)
const rect = element.getBoundingClientRect();                                  // ④ 框就是元素的 border box
this.highlightElement.style.top = `${rect.top + scrollY}px`;                   //    + 滚动量换算成绝对坐标
```

**它为什么天生精确**：`document.elementFromPoint` 就是浏览器的命中测试，返回**最内层的 DOM 元素**
——包括 UIA 看不到的裸 `<div>`（§5.11 里我们用 MSAA 换来的那个能力，在页面内是一行 JS）。
没有树遍历、没有回溯、没有"最内层"猜谜：一次调用就是答案。这从产品侧再次印证了 §5.10/§5.11 的结论：
**精度差的是"在不在页面内"，不是算法。**

**交互/工程细节（值得借鉴的部分）**：

| 事项 | 它的做法 | 对我们的意义 |
| --- | --- | --- |
| 遮罩与命中测试 | 全屏 `eventBlocker`（`z-index:9998`, `pointer-events:all`）拦截页面交互；**只在查询那一瞬间**改为 `none`，随后立刻恢复 | 与我们的 `HitTestPassThrough`（只包一次 `ElementFromPoint`）是同一个纪律；它也证明了"查询瞬间穿透"不会打断页面状态 |
| 高亮与尺寸 | 高亮 div（0.2s ease、2px 边框 + 半透明填充）+ 尺寸标签 `W × H`，靠顶部时标签翻到下方 | 纯 UX，可直接抄 |
| 连续模式 | `isInspectMode` 跨多次截图保持；`Enter` 确认、`Esc` 退出、滚动时按 `currentElement` 重新定位 | 我们已有类似状态（hover/preview），可对齐交互 |
| 磁性吸附 | `getElementsNearPoint`：`querySelectorAll('*')` 扫全部元素，过滤 `display:none/visibility:hidden/opacity:0` 与 <10px 的，收集 left/right/centerX 与 top/bottom/centerY，**每轴只留最近 3 条**，阈值 8px、强度 0.5，边缘缓存 200ms | 只有"在页面内"才做得到（原生工具枚举不了页面元素）；可抄的是"可见性与最小尺寸过滤 + 每轴取最近 3 条"这个降噪策略 |
| 长截图滚动容器 | `findScrollableContainer`：用 `document.elementsFromPoint`（**复数**，返回整条元素链）从中心点向上找第一个 `overflow-y: auto/scroll/overlay` 且 `scrollHeight > clientHeight + 10` 的元素 | 我们做滚动捕获时需要同样的"内层滚动容器"判定（见 docs/19） |
| 截图本身 | `chrome.tabs.captureVisibleTab`（**只有可见视口**）+ 后台按 `dpr` 裁剪；超视口靠滚动拼图 | 这是扩展路线的硬约束：要么只能截视口，要么自己做拼图（它的 `.specstory` 里 "截图保存区域偏差"、"长截图滚动问题" 两份记录合计 10 万字符，全是坐标/滚动踩坑） |

**它的局限（源码里查不到处理）**：整个仓库没有任何 `shadowRoot` / `contentWindow` / `composedPath`，
也就是说 `elementFromPoint` 只会给出 `<iframe>` 或 shadow **宿主**本身，进不去里面（这是**扩展**路线的局限；
反过来看我们的 MSAA 通道，shadow DOM 与同源/跨域 iframe 内部都能进去，见 §5.20——所以"扩展才能进 iframe"
这个判断只对扩展自己成立）。另外它必须装扩展、要 `<all_urls>` 权限，浏览器商店审核/用户信任都是成本。

**对 SnapClip 的结论**：扩展路线能拿到的"元素边界"，**MSAA 已经能在不装任何东西的前提下拿到**
（§5.11：8/8 逐像素命中、1 ms）。两者的差别在于**扩展能拿到 DOM 语义**（
`getBoundingClientRect` 之外的属性、跨 iframe、整页盒表、`elementsFromPoint` 的完整链），而 MSAA 只能
拿到"盒子 + 角色 + 名字"。所以：**先做 MSAA（零安装、零权限、覆盖所有 Chromium 浏览器）；
只有当产品真的需要 DOM 语义时，才考虑扩展**——那时 §5.12 的这些实现细节（遮罩瞬间穿透、尺寸标签、
连续模式、可见性过滤）可以直接复用。

### 5.13 CDP 路线的两份现成实现（`refer/cdp-html-shot-main`、`refer/webshot-master`）

两份都是 Rust + CDP，都能"按 CSS 选择器截元素"，但技术层次不同：前者是**手写 CDP 客户端**（只依赖
`tokio-tungstenite` + `serde_json`，传输层 `src/transport.rs` 只有 8 KB），后者是**CLI 外壳**，把
定位与截图交给 `headless_chrome` crate。对 SnapClip 有价值的是"完整管道长什么样、坑在哪"。

**A. `cdp-html-shot`：手写 CDP 的完整配方**

启动（`src/browser.rs`）：

- 端口：用 `TcpListener::bind` 探测一个空闲端口；参数含 `--remote-debugging-port={port}`、
  **`--user-data-dir=<临时目录>`**（正是 Chrome 136 起要求的"必须配非默认 profile"）、
  `--headless=new`、`--no-first-run`、`--hide-scrollbars`、`--window-size=1200,1600`、
  `--disable-features=…` 与一组 GPU 开关。
- **发现 ws 地址靠 stderr 正则**：`listening on (.*/devtools/browser/.*)\s*$`（`wait_for_ws`）——
  比解析 `DevToolsActivePort` 文件省事，且是 Chromium 的稳定输出；后台线程持续 drain stderr 以免管道满。
- 找浏览器：显式安装路径 → `CHROME` 环境变量 → **注册表 `App Paths\chrome.exe` / `msedge.exe`**
  （`winreg`，HKLM）；再用 `--version` 解析大版本，拼一个普通桌面 UA 来"不暴露 headless"。

元素链路（`src/tab.rs` + `src/element.rs`，共 6 次调用）：

```text
DOM.getDocument                          → root.nodeId
DOM.querySelector(root, selector)        → nodeId（**0 = 无匹配**，必须显式判错）
DOM.describeNode(nodeId, depth:100)      → backendNodeId（跨节点引用稳定）
DOM.getBoxModel(backendNodeId)           → model.border = 四角坐标（取 border，不是 content）
  clip = { x: border[0], y: border[1],
           width:  border[2]-border[0],
           height: border[5]-border[1], scale: 1.0 }
Page.captureScreenshot{ format, clip, fromSurface: true, captureBeyondViewport: full_page }
轮次：Page.enable + Page.loadEventFired（set_content/goto）或每 100ms 轮询 wait_for_selector
细节：截图前 Target.activateTarget；透明背景用 Emulation.setDefaultBackgroundColorOverride
      （只对 PNG；用完发一次 {} 复位）；DPR 用 Emulation.setDeviceMetricsOverride
      { width, height, deviceScaleFactor, mobile, screenOrientation }
```

**B. `webshot`：CLI 外壳 + 拼图**

- 元素截图 = `tab.find_element(selector)` + `element.capture_screenshot()`；`headless_chrome` 的实现
  （上游 `src/browser/tab/element/mod.rs`）是 **先 `scroll_into_view()` 再用元素盒子 clip 截图**——
  即"元素在视口外"这件事，由滚动解决。
- 视口/DPR 同样走 `Emulation.setDeviceMetricsOverride`；默认 1280×800、`retina=false`、
  `timeout=30s`、`max_height=30000`、`scroll_delay=100ms`。
- 全页与"超高元素"：`ScrollMode::{Viewport, FullPage, FullElement}`——**自己滚动 + `image` crate 拼接**
  （FullElement 先 `Runtime.evaluate` 取 `{x, y, width: scrollWidth, height: scrollHeight}`，再逐屏截拼）；
  批量任务用 `buffer_unordered(parallel)`；PDF 走 `PrintToPdfOptions`；截图前可执行 JS、可等元素。

**对 SnapClip 的五条结论**：

1. **管道成本很低且已有成熟做法**：启动、发现 ws、attach、选择器→盒子、截图、DPR、透明背景，
   手写也就 8 KB 传输层 + 40 KB 主逻辑（`cdp-html-shot` 全量），不需要引入 CDP 封装库。
2. **两份都不做"光标所在元素"**：它们只接受调用者给的 CSS 选择器。要按光标吸附仍必须用
   `DOM.getNodeForLocation(x, y)`（§5.10 已确认 CDP 提供），这两份补的是**管道**，不是命中测试。
3. **坐标事实（接入时必须记住）**：`DOM.getBoxModel` 的四角是**页面文档坐标系**的 CSS px；
   `Page.captureScreenshot.clip` 用同一坐标系，输出像素 = clip × `scale`（或 × deviceScaleFactor）。
   我们的 overlay 用**物理屏幕 px**，换算链是：浏览器窗口原点 + 页面原点相对 client 的偏移
   （本机实测 87 px）+ 页面缩放 + DPR。§5.4 的坑（把 client 当作页面原点）在这里一模一样。
4. **不要抄"滚动 + 拼图"**：CDP 有 `captureBeyondViewport: true` + 文档坐标 clip，元素高于视口时
   一次就能截；webshot 的拼图正是另一份参考项目里 10 万字符坐标 bug 的来源。
5. **产品形态**：这两份都是"**自己拉起 headless 浏览器（全新临时 profile）**"，从不 attach 用户正在用
   的浏览器——与 §5.11 的结论一致（Chrome 136 起默认 profile 禁用远程调试）。所以 SnapClip 若走 CDP，
   定位应是"在 SnapClip 启动的浏览器里做精准截图"，而不是"吸附你正在浏览的页面"。

### 5.15 "最内层"的准确定义（2026-10-06，用户提出的边界问题）

用户提出的场景：`A ⊃ B ⊃ C` 三层嵌套，光标在 **C 内** → 取 C 没问题；那光标在 **C 外、B 内** 时，
"最内层"该怎么定义？

**定义**：目标是**光标所在处命中的、DOM 层级最深的那个元素**。"盒子里没有盒子"只是它在叶节点时的
**特例**，不能当定义用——否则 B\C 这种区域无法回答。

| 光标位置 | 被该点包围的元素 | 层级最深者 | 目标 |
| --- | --- | --- | --- |
| C 内 | A、B、C | C | **C** |
| C 外、B 内 | A、B | B | **B** |
| B 外、A 内 | A | A | **A** |

**为什么不能靠"矩形包含 + 面积最小"自己算**：同一层级里可能有更小的**兄弟**盒子也覆盖该点（面积最小会
选错）；`position`/`transform`/`overflow` 会让"布局盒包含"与"视觉上在该点"不一致；还有
`pointer-events: none`、被完全裁掉的元素、零尺寸包装层。这些正是浏览器命中测试（堆叠顺序 + 裁剪 +
变换）在做的事，所以**目标必须来自命中测试，而不是几何猜测**。

**门禁**：夹具新增两条边界点，把上表变成断言（`nested-outer` 的第二个采样点落在 A\B → 期望 A；
`nested-mid` 的第二个采样点落在 B\C → 期望 B），当前实现 **27/27 全过**：

```text
[probe] assert nested-outer  expect=self  expected=320x200 @(72,458)   published=320x200 @(72,459)   OK  name=Outer Shell
[probe] assert nested-mid    expect=self  expected=256x136 @(104,490)  published=256x136 @(104,491)  OK  name=Mid Shell
[probe] assert nested-inner  expect=self  expected=192x72  @(136,522)  published=192x72  @(136,523)  OK  name=Deep Span
```

**产品规则（把"精确吸附到每一个元素盒子"变成可实现的东西）**：

1. **目标 = 命中测试的最深元素**；**发布的盒子 = 它的可见部分**（祖先链 ∩ 窗口，§5.9 已实现）。
2. **可见部分为空时向上退**：被 `overflow:hidden` 完全裁掉、或可见交集为空的节点不能发布；当前实现
   在走查里跳过这类节点，等价于自动上退到最近一个可见祖先。命中测试侧要用同一条规则。
3. **需要"上移一层"的交互**：只按定义走，"指着 C 却想截 A"就无解。DevTools 的做法是 **↑/↓ 沿祖先链
   移动选中层**（`elementFromPoint` 只给最深者，父层靠遍历）。要做到"每一个元素盒子"，建议对齐这个
   交互（滚轮或 ↑/↓ 切换层级），否则用户只能靠"把光标挪到空白处"来选择父盒子。
4. **三种命中测试的"口味"不同，需要选一种**：
   · 页面内 `document.elementFromPoint`：遵守 `pointer-events`（§5.12 的扩展用它）；
   · CDP `DOM.getNodeForLocation(..., ignorePointerEventsNone)`：DevTools 的 Inspect 用它，**可以越过**
     `pointer-events: none` 的装饰层；
   · MSAA `accHitTest`（§5.11）：走渲染进程真正的命中测试，**完全不看 `pointer-events`**——对"抓元素
     盒子"反而更合适。
5. **"不可见的包装层"要不要跳过，是产品政策**：Tailwind 类页面里大量无绘制内容的布局 `<div>` 会成为
   命中者（§5.11 实测的 `plain-div` 就是），选它"正确但可能没用"。可选过滤是"可见部分有实际绘制
   面积/尺寸 ≥ N px 才作为终点"（`smart-screenshot` 的磁性吸附就是这么过滤的：`display`/`visibility`/
   `opacity` + 最小 10px）。**这条需要你定**：默认选中最深元素（忠实），还是跳过无绘制包装（好用）。

### 5.16 已实现：MSAA 命中源接进精度补足（2026-10-06）

产品决策：**先忠实**（最深元素胜出，不加"无绘制包装层"过滤），以后再加开关。实现如下。

**1. MSAA 命中源**（`platform/windows/capture/msaa_provider.rs`）

- `MsaaDeepSelectionProvider::hit(hwnd, point, window_bounds)`：先 `OBJID_CLIENT`、失败再 `OBJID_WINDOW`
  （PixPin 的 `chrome.exe` 分支同理：Chromium 的 **client** 对象才是走渲染进程命中测试的那个）；
  `accHitTest` **循环到 `CHILDID_SELF`**（上限 8 次，oleacc 的 `AccessibleObjectFromPoint` 就是这个循环，
  只是我们不能用它——它会先做全局命中测试，被我们的遮罩答掉，而 `HTTRANSPARENT` 对 MSAA 无效）；
  `accLocation` 取矩形；`get_accRole`/`get_accName` 取角色与名字。
- **可见部分**：`accLocation` 是未裁切的（夹具里 20000px 高的元素回 `358x20000`），所以照 §5.9 的规矩沿
  `accParent()` 链逐级求交（上限 16 层），只在"结果仍含光标"时才收缩；再与窗口求交。
- **仍然全程 deadline 保护**：走既有的 `TimedCallRunner`（168ms）+ 每代 quarantine，MSAA 卡死不会拖住查询。
- 命中框不含光标时返回失败（而不是发布一个没盖住光标的框）。

**2. 判定与记录**（`refinement_worker.rs` 的组合层 + `uia_provider.rs` 报告自己的决定）

- UIA provider 不再自己记 `record_precision`，而是把"这次命中测试的判定 + 事实"存下来，由组合层
  `take_hit_decision()` 取走——**每次查询只记一条判定**，否则两个传输会让会话汇总里的计数器翻倍、无法解读。
- 组合层的 `adopt_msaa_hit`：把 MSAA 的**可见部分**用与 UIA 完全相同的规则判定
  （`should_adopt_msaa_box`：非空、含光标、严格更小、且角色不是 `ROLE_SYSTEM_TEXT/STATICTEXT`），
  采纳则追加进 `path` 并把 kind 提升为 `UiElement`。
- 强制取证行现在一行说清三方：`walk=… uia=[…] msaa=[role=… name=… depth=… 原始盒/可见盒 ADOPTED]`。
- 计数器语义保持"每次查询一条"：`adopted` = 至少一个传输细化了走查答案；`not-finer` = 有传输回答但没细化；
  `unavailable` = 两个传输都没给出可用框。

**3. 门禁变化（同一台机器，4K@DPI144）**

| 门禁 | 之前 | 现在 |
| --- | --- | --- |
| 浏览器夹具断言 | 27/27（6 行因 UIA 看不见而 `optional`） | **33/33**，6 行全部改成 `expect: self` 并**通过**（`plain-div`/`checkbox`/`radio`/`para`/`code-box`/`table-cell-1`） |
| 浏览器探针重试 | `slow_fixtures=30` | **0**（MSAA 一次就给出答案，不再需要等 UIA 树物化） |
| 浏览器延迟 | n=72 p50 17.5 / p95 29.0 / max 34.7 ms | n=48 p50 30.3 / p95 49.8 / max 61.3 ms（+一次 MSAA 调用，约 13ms） |
| Explorer | 12/25、中位 65.8%、25/25 命中可用、p50 58.9 / p95 69.2 / max 151.9 ms | **12/25、65.8% 不变**，25/25 可用，p50 50.4 / p95 56.9 / max 68.3 ms |
| 单元测试 | 361 passed | 361 passed（新增 MSAA 规则断言并入既有测试） |

**4. 还没做的（按 §5.15 的清单）**

- **祖先链上移（↑/↓ 或滚轮）**：**已实现，见 §5.17**（普通滚轮 + ↑/↓；默认仍取最深）。
- **无绘制包装层的开关**：用户已定"先忠实、以后再加开关"，所以本轮没有加任何可见性/尺寸过滤。

### 5.17 层级上移（ancestor walk）设计（2026-10-06，先设计后动手）

**要解决的问题**：§5.15 定义的"最内层"是忠实策略——最深元素一定胜出。于是"指着 C 却想截 A"无解。
DevTools 的 Inspect 用"悬停取最深 + ↑/↓ 沿祖先链上移"解决，这也是"吸附到每一个元素盒子"的最后一环。

**1. 数据契约：`DeepTarget.path` 的语义收紧为"发布盒子的真实包含链"**

现在 `path: Vec<Rect>` 是"走查接受的层级"，frame → … → 最深。问题在采纳命中框之后：`path` 的尾部是
**UIA 走查**的层级，而被采纳的框来自 UIA/MSAA 命中测试，它**不一定是那些层级的后代**——于是
`path` 不再是包含链，"上移一层"会跳到一个不含当前框的盒子。

设计：**任何能发布盒子的来源都必须产出"该盒子的真实祖先链"**，`path` 的语义由"走查层级"收紧为
"frame → … → 发布盒子的包含链"，并作为**不变式**用探针断言（每层包含下一层，最后一项 =
`screen_bounds`）。具体到三个来源：

| 来源 | 祖先链怎么来 |
| --- | --- |
| UIA 走查（现状） | 已经是包含链（`stack` 逐级入栈）——只需断言守住 |
| UIA 命中采纳 | 只保留 `path` 中**包含该框**的前缀，再 append 命中框（`push_box_keeping_containment`） |
| MSAA 命中采纳 | 用 `accParent()` 链（§5.16 已经在走它算可见部分）反向收集：frame → … → 框 |

第 2、3 条都要做"前缀裁剪"：把不包含新框的尾部层级丢掉，而不是直接 append（那正是现在会产生
非包含链的地方）。

**2. 交互：普通滚轮 + ↑/↓，只在"还指着这个盒子"的时候有效**

查过现有占用：普通滚轮是 no-op（`Z`+滚轮才是放大镜），↑/↓ 未被占用——两个都可用。

```text
层级索引 index 默认 = path.len()-1（最深，即现状，零行为变化）
滚轮上 / ↑  → index -= 1（更外层，最多到 frame）
滚轮下 / ↓  → index += 1（更内层）
预览框、初始选区、以及点击后的调整，都读 path[index]
```

**重置规则**（防止索引跨目标泄漏）：`(请求 id, index)` 成对保存；当调度器为**新的位置**发布目标时
index 回到最深。光标移开当前盒子也重置。改变层级**不触发新查询**——链已经在手里。

**生效范围**：首次点击之前的悬停预览阶段（元素选择发生在这里），以及紧随其后的调整阶段——只要光标还
停在同一个目标上。开始拖拽创建自定义选区后不再参与（那时用户在画框，不在选元素）。

**可视化**：沿用已有的 `deep_path_local` 描边；额外把 `path[index]` 作为强调框，让用户看清"现在选的是
哪一层"。

**3. 实现顺序（每条独立提交 + 门禁）——已全部实现**

1. **契约 + 不变式**：`path` 收紧为包含链（UIA 采纳路径做前缀裁剪；MSAA 收集 `accParent` 链并裁剪），
   在两个探针里加"path 必须是包含链且末项 = 发布框"的断言。**这一步改数据语义，不改交互**——默认
   `index = 最深`，所以产品行为不变，可以先独立验证。
2. **纯逻辑**：`LevelChain { levels, index }`（`deeper()/shallower()/current()/reset_to_deepest()`）
   放进库代码并单元测试（边界：单层、到顶、到底、越界）。
3. **接线**：overlay 保存 `(请求 id, LevelChain)`，把滚轮/↑/↓ 接到它上面，预览与初始选区读 `current()`。
4. **回归**：浏览器 33/33、Explorer 12/25、延迟不明显退化；单元测试含新边界用例。
5. **文档**：本节标注"已实现"，补实测数字与交互说明。

**已实现（2026-10-06，`3679b64` 第 1 步 + 本次第 2、3 步）**

| 步骤 | 落地 |
| --- | --- |
| 1 契约 + 不变式 | `path` 收紧为包含链：采纳命中框时裁掉不含它的层级（`push_box_keeping_containment`），MSAA 祖先先按窗口裁剪再反转、frame 补在最外层；两个探针对**每个采样点**断言"每层包含下一层且末项 = 发布框" |
| 2 纯逻辑 | `LevelChain { levels, index }`：`new` 默认最深、`shallower/deeper` 到头返回 `false`、`reset`、`current(path)` 按 path 长度**钳制**（陈旧链不会发布链外的框）；6 个单测覆盖单层/到顶/到底/钳制/空路径 |
| 3 overlay 接线 | 状态 `deep_levels`；**普通滚轮 + ↑/↓**（`Z`+滚轮仍是放大镜）；新答案到达时重置为最深、光标离开所选层级时交还给指针；`preview_bounds` 增加 `level` 参数（选中层级覆盖发布框，但不会让**不属于该窗口**的目标复活）；`deep_path_local` 只画选中层之上的轮廓；确认行加 `level=n/N` |

**实测（同机 4K@DPI144）**：浏览器 **33/33** 断言、48/48 命中可用、0 条非包含链，延迟 n=48 p50 31.0 /
p95 49.2 / max 51.6 ms；Explorer **12/25**、中位 65.8%、25/25 可用、0 条非包含链，p50 63.1 / p95 70.2 /
max 162.7 ms；`cargo test --lib` **368 passed**（新增 6 个）、`cargo check --all-targets` 0 warning。

**默认零变化**：`LevelChain::new` 选最深，所以不按键时预览、初始选区、确认结果与本次改动前完全一致——
层级只在用户主动滚轮/按方向键时改变。

**4. 明确不做（本轮）**

- 不做"按可见性/尺寸跳过包装层"的开关（用户已定"先忠实、以后再加"）。
- 不做跨 iframe / shadow DOM 的层级（那需要 §5.13 的 CDP 或 §5.12 的扩展通道）。
- 不改 `Z`+滚轮 的放大镜行为；不做层级动画（层级切换是离散选择，不需要补间）。

### 5.18 "纯文本能不能捕获"：文字跑条 vs 装它的盒子（2026-10-06，实测）

用户给了一段真实 DOM：CodeMirror 6 只读代码块（`div.cm-editor > div.cm-scroller > pre.cm-content >
code > span`），**所有文字在那一个 `<span>` 里**，问"这个 div 里面的纯文本就不能捕获了吗"。

夹具里加了同构的 `code-block`（`pre#code-pane` + `code` + 含多行文字的 `span#code-run`），实测两条通道在
这个点上的答案：

```text
[sources] code-run (文字内部) point=(815,1070)
  control hit : Text(777,1034)-(854,1109)                        ← UIA：文字跑条 77x75
  msaa hit    : role=0x29 name="项目 A ↓ 打开资源管理器…" 77x75   ← MSAA：STATICTEXT

[probe] assert code-block  expected=320x96 @(768,1026)  published=318x94 @(769,1028)  OK
```

**结论：能捕获——捕到的是"装着这段文字的盒子"（这里就是代码块 318x94），而不是文字跑条本身。** 文字跑条
（UIA `Text` 50020 / MSAA `ROLE_SYSTEM_TEXT(0x2a)`、`STATICTEXT(0x29)`）被现有规则明确排除，这是
§5.6 A / §5.16 定下的产品决策，而且有据：夹具里 `nested-*` 三行如果在文字跑条上采纳，答案会变成盒子里
那条字（70x20），而产品要的是那个盒子（192x72）。

**这条规则要不要开开关，由产品定**（与"跳过无绘制包装层"是两个独立的开关）：

- **忠实（现状）**：文字跑条不选，选它所属的盒子 → "指着一行字"得到代码块/段落。
- **可选**：允许文字跑条作为目标 → CodeMirror 这类**每行一个 `span`** 的编辑器里能精确吸附到**单独一行**
  （点"复制路径"那一行就得到那一行）；代价是 `nested-*` 的期望要跟着改成"取文字"。

落点在 `should_adopt_provider_box` / `should_adopt_msaa_box` 里排除文本类型的那两个条件——开关加上去是
一行判断，不动流程。

### 5.19 采纳文字跑条（产品已定：要这个行为）（2026-10-06）

**决定**：把"文字跑条"也允许作为目标——在 CodeMirror 这类**每行一个 `span`** 的编辑器里，指着一行字就
得到**那一行的盒子**；接受 `nested-*` 那几行期望从"盒子"变成"盒子里那条字"的代价。

**为什么现在开是安全的（这是关键）**：§5.6 A 当年排除文字跑条，是因为那时**没有别的办法回到容器**——一旦
取了文字，用户就没法拿到那个盒子。现在 §5.17 的**层级上移**已经存在：指着文字得到那一行，**滚轮往上一格**
就回到它所在的盒子。也就是说这条规则从"单向取舍"变成了"默认更细、可一步退回"，两边的能力都不丢。

**规则改动（最小面）**：

- `should_adopt_provider_box` / `should_adopt_msaa_box` 增加 `adopt_text_runs: bool`：为真时不再因
  `TEXT_CONTROL_TYPE` / `ROLE_SYSTEM_TEXT|STATICTEXT` 拒绝，其余条件（非空、含光标、**严格更小**）不变。
- **走查自己的规则不动**：带边框的 `Pane`/`Group` 仍然"认领"自己的文字块（`is_text_run_inside_element`），
  因为走查在树里拿到的是容器；**更细的那一层由命中测试给出**（UIA `ElementFromPoint` / MSAA `accHitTest`
  都能直接答出文字跑条，§5.18 已实测）。
- 策略值从 overlay → `RefinementWorker` → 组合 provider → 两个 provider 逐层传下去；本轮用常量
  `DEFAULT_ADOPT_TEXT_RUNS = true`，将来接设置时它就是 `capture/deep_select_text_runs` 的默认值。
- **"跳过无绘制包装层"仍是独立开关**，本轮不动（用户已定"先忠实"）。

**门禁要跟着改（这正是这次的行为变化）**：

| 行 | 之前 | 现在 |
| --- | --- | --- |
| `nested-outer` / `nested-mid` / `nested-inner` | 期望**盒子**（320x200 / 256x136 / 192x72） | 期望**盒子里面**（`inside_self`）+ 新断言 `finer_than_self`：答案必须严格小于该盒子 |
| `code-block` | 期望**代码块**（320x96） | 同上：答案是块里那一段文字，必须严格小于块 |
| 精度计数器 `provider_hit_is_finer_on` | 3（三条文字跑条被**有意放过**） | **0**：更细的命中一律采纳，"更细却没采纳"的断言不再有例外 |

`finer_than_self` 是新加的夹具断言（`published ⊆ 自身盒子` 且**面积严格更小**）——只判 `inside_self` 太弱，
它会同时接受"盒子本身"和"盒子里那条字"，而这次要验证的恰恰是**真的取到了更细的那一层**。

**已实现（2026-10-06，最终规则 + 一个被门禁挖出来的底层事实）**

第一版按"最细者通吃"实现后，门禁立刻指出代价比预期大：**按钮也变成它的标签**（`btn-plain` 期望
168x56，拿到 74x16）。查下去发现两件事：

1. **UIA 的 `ElementFromPoint` 是"近似命中"，不稳定**。同一个点连续三次查询，它分别答出**文字跑条
   70x20 → 外层盒子 320x200 → 中层盒子 256x136**（探针现在把每条查询的判定都打出来了：
   `[probe] decision nested-mid: … uia=[not-finer provider=320x200 …]`）。这正是 Chromium 那条
   "approximate hit test" 的注释所指：浏览器侧用缓存树按矩形近似，而 MSAA 的 `accHitTest` 走渲染进程
   **真实命中**。所以"UIA 具体就听 UIA"是错的前提——**MSAA 永远要问**（UIA 的候选仍然先试，但只可能
   用来细化，永远不会因为它的不稳定而发布一个更粗的框）。
2. **按钮的标签需要一个产品分界**：文字跑条只在"**父不是交互控件**"时才算独立目标
   （`is_interactive_control_role`：Link / PushButton / CheckButton / RadioButton / ComboBox /
   DropList / MenuItem / ListItem / PageTab / Slider / SpinButton）。父角色来自我们本来就在走的
   `accParent` 链（`MsaaHitBox::parent_role`，多一次 `get_accRole`）。

于是最终行为：**按钮是按钮（168x56），编辑器里的一行字是那一行（70x20 / 77x75）**——
夹具里两边都是断言。

| 门禁 | 结果 |
| --- | --- |
| 浏览器夹具 | **35/35** 断言通过：`nested-*` 三条发布 **70x20**（文字跑条，`finer_than_self`），
`btn-plain`/`btn-disabled`/`role-button` 仍发布 **168x56**（按钮），`code-block` 发布 **77x75**（块内那段文字） |
| 精度计数器 | `provider_hit_is_finer_on=0`——"更细必被采纳"的断言**再无任何例外**（文字跑条不再是豁免项） |
| 延迟 | 浏览器 n=49 p50 29.4 / p95 42.7 / max 66.6 ms；Explorer p50 64.4 / p95 106.2 / max 174.2 ms
（MSAA 每次都问 + 一次 `accParent` 取父角色，仍在 1500 ms 预算内） |
| Explorer | `control_level_points=12/25`、中位 65.8% **不变** |
| 其它 | `cargo test --lib` 369 passed、`cargo check --all-targets` 0 warning |

唯一还没做的开关仍是"跳过无绘制包装层"（用户已定"先忠实、以后再加"）。

### 5.20 浏览器元素的可达边界（2026-10-06，三条边界升为断言）

用户问"是不是没有直接拿网页 DOM 的方案"，于是把三类边界从 `optional`（只打印）升成**正式断言**，
顺带发现了一个**夹具自身的 bug**，它让一条结论一度是错的。

| 边界 | 结果 | 实测 |
| --- | --- | --- |
| Shadow DOM 内部 | **可达** ✓ | `shadow-button` 发布 `192x56`（shadow root 里的按钮本身） |
| 同源 iframe 内部 | **可达** ✓ | `iframe-button` 发布 `160x48`；系统命中测试同样答 `Button(1105,500)-(1265,548) "Iframe Button"` |
| **跨域 iframe 内部** | **可达 ✓（但要在那一帧的子树物化之后）** —— 本条 2026-10-06 已订正，见下 | 夹具页是 `file://`，另一帧来自本机 `http://127.0.0.1:<port>`（探针自己起的单页服务）→ 真跨域。**带上"子帧已就绪"的哨兵并等它之后**，4/4 次运行都答到 frame **内部那个按钮**（`160x48`，MSAA `role=0x29 "Cross Button"`）；不带哨兵、页面刚发布几何就测的那一批里，4 次有 3 次只答到 frame 自身（`358x94`，`role=0xf` = 该 frame 的文档节点） |
| `display:none` / `aria-hidden` | 正确拒绝 ✓ | 三行 `expect: "none"` 断言"答案必须比它更粗" |
| 没有盒子的节点（未渲染的虚拟化项） | 拿不到（**它本来就没有盒子**，CDP 也只能告诉你这个事实） | — |

**同一个坑踩了两次——两次都是"没有哨兵"**：

1. **同源那次**：`builders` 里的构造函数跑在一个**游离的 holder 元素（`div`）**上，真正的 `<iframe>` 之后
   才创建；我在 builder 里写 `el.srcdoc = '…'`，而 `div` 没有 `srcdoc` 这个 IDL 属性——只是加了个 JS 字段，
   **srcdoc 从未成为 HTML 属性**，iframe 一直是空白页。于是"指着 iframe 只拿到宿主元素"看起来成立，
   我还据此给出了"iframe 进不去"的结论。修法是把 `srcdoc` 挪到 `after`（那里拿到的才是真实元素）。
2. **跨域那次（2026-10-06 订正）**：父页面读不到跨域帧，于是我把"指向里面的按钮只答到 frame 自身"当成
   结论记进了这份文档——**但那一刻帧的子树还没物化**（Chromium 的跨进程无障碍树是惰性的，§5.20 上一节
   与 §6 的坑 3 是同一件事）。没有"子帧说自己已经就绪"的哨兵，这两种状态在探针里长得一模一样。
   现在的做法：子帧测量自己并 `postMessage` 报告（`cross-ready` = 内部 body 的 HTML 长度，`cross-button`
   = 它自己量到的按钮盒子），**探针拿不到这份报告就拒绝测量并直接失败**（10 s 上限），另外子帧还会
   请求一次 `/cross-ping`——服务端按路径记录请求，用来区分"帧没要页面 / 页面被返回了但脚本没跑 /
   脚本跑了但消息没回来"三种情况（这三种情况在日志里分别是：没有 `/cross.html`、没有 `/cross-ping`、
   两者都有但没有 `cross-ready`）。

教训与 §5.12 的一条同源：**夹具是产品结论的地基，哨兵要能区分"没命中"与"里面本来就没东西 / 还没长出来"。**

**订正后的答法**：对 DevTools Elements 面板里那些 HTML 元素——只要它在光标下有**盒子**并且已经物化，
我们现在基本都能拿到：裸 `<div>` ✓、文字行 ✓、Shadow DOM 内部 ✓、同源 iframe 内部 ✓、
**跨域 iframe 内部 ✓（等它物化；第一次查询可能只答到 frame 节点，dwell 的重复查询会接着细化）**、
CSS transform 旋转盒 ✓、canvas/svg/表格/表单控件 ✓。真正剩下的只有"**没有布局盒的节点**"
（`display:none`、未渲染的虚拟化项）。也就是说：日常吸附根本不需要 CDP；CDP/扩展的价值主要在
"整页盒表、DOM 属性、以及我们自带浏览器的精准截图"这些**独立功能**上（§8 待办 2）。

**跨域夹具怎么搭的**：探针在 `127.0.0.1` 上起一个极简 HTTP 服务（固定布局：`margin:0` + 一个
`left:40 top:40 160x48` 的按钮 + 几行报告脚本），把端口写进夹具页的查询串
（`…?truth=1&cross=127.0.0.1:<port>`），页面用它做 iframe 的 `src`。父页面读不到这个 frame，所以
**盒子由子帧自己量、自己报**（`postMessage` → `crossReport` → `truth['cross-button']`，父页面只补上
用 `getComputedStyle` 量出来的边框宽度，不再假设 1 px）。

两条断言行，写法与理由：

| 行 | 点位 | 期望 | 为什么这样写 |
| --- | --- | --- | --- |
| `cross-frame` | frame 内、**不在按钮上** | `self`（frame 自己的盒子 `360x96`） | 无论子树物化与否，这一点的答案都只可能是 frame 节点或它的文档根，两者都是 frame 的盒子——所以这条是**稳定**的，而且能抓住"答成别的东西 / 答成整页" |
| `cross-button` | 按钮上 | `within:cross-frame`（必须落在 frame 盒**之内**） | 这一点的答案在实测里是内部按钮 `160x48`（4/4），但跨进程树合并的粒度不该由我们钉死；`within:` 这条规则表达的是真正的不变量：**不能掉到 frame 外面去，更不能退化成整页**。实测的盒子仍然照打在 assert 行上（`published=`），所以粒度变了在日志里看得见 |

### 5.21 智能吸附的 UI / 动画规格（2026-10-06，已实现）

用户问"对智能吸附的 UI、动画有什么建议"。有一条**底线**先立住：**动画不许用来掩盖检测误差**。
矩形永远只是"最后一个被确认的答案"的缓动插值，而**确认（Enter / 点击）用的是真实目标**
（`GestureState::snap_preview()`），动画与提交是两条互不相干的值：所以缓动不可能改变被截下的像素，
也不会让一个还没确认的答案看起来像已经确认。

**1. 插值本身是纯函数，不是计时器**

`capture/window_detection/transition.rs`：

| 项 | 值 |
| --- | --- |
| 时长 | `PREVIEW_TRANSITION_MS = 101`（对齐参照选择器的 OutQuad） |
| 曲线 | `out_quad(t) = 1-(1-t)²`（快起、稳落） |
| 几何 | `lerp_rect(from, to, amount)`：四条边各自独立插值（位置与尺寸同时过渡） |
| 状态 | `RectTransition { settled / start(from, to, now) / present(rect, now) / value_at(now) / is_running(now) / target() }` |

它**不拥有 timer、不拥有窗口**：由 overlay 既有的 15 ms 合并渲染 tick（`on_render_tick` →
`advance_preview_animation`）驱动，动画和其他所有重绘共用同一个时钟。好处是可测试：任意时间点求值，
6 条单测覆盖两端点、50 %、超时、打断、`present()` 取消。

**2. 四个时机的表现**

| 时机 | 表现 |
| --- | --- |
| 会话的第一个预览 | **直接呈现**（`present()`）：不从"空的框"里长出来，没有出处的东西不该有入场动画 |
| 同链层级切换（滚轮 / ↑↓） | 从**当前屏幕上那个矩形**缓动到新层级；再滚一次从当前显示值接着走（`start(displayed, to)`），所以连滚看起来是一条连续的轨迹，而不是跳回上一格 |
| 跨目标位移（鼠标移到另一个元素） | 与上一条同一条代码路径，行为相同 |
| 等待答案（dwell 已确认、refinement 未回） | 不显示"半成品"：预览停在 v1 整窗框（或上一个答案）直到答案到达，再走同一段 101 ms |
| 消失（取消 / 换窗口） | **直接消失**，从不向 0 收缩 |

**3. 标签（preview label）——这一层才是"看得懂的反馈"**

标签文本由 overlay 的纯函数 `preview_label(rect, is_window, levels, degraded)` 生成，渲染层只负责排版与绘制
（复用选择框尺寸标签的 panel、字体、内边距；放不下就整体丢弃——**宁可没有标签，也不盖住正在选的像素**）：

```text
341×55 px  元素          最深层的答案（每次预览的起始状态）
689×55 px  容器 8/9      用户走过层级：说清"是什么" + "还剩几层"
3840×2088 px  窗口        整窗兜底 / 一路走到了 path[0]
341×55 px  元素?         这个位置什么都没有答上来（降级）
3840×2088 px  窗口 1/7    走到最外层时的完整形态
```

**为什么是这三个词**（`窗口` / `容器` / `元素`）：这三个词不需要任何解释，而用户真正在问的问题就是它们——
"它吸的是那个东西本身，还是那个东西外面的壳？" `容器` 这三个字直接把"你往上走了一层、它变宽是应该的"
说完了，`8/9` 只是补充"这条链还剩几层没走"。而**光有分数是读不出来的**：`3/7` 在用户眼里可能是缩放、
页码、色阶，任何一个都需要额外的解释成本。

**`?` 而不是 `~`**：`~` 在这个位置是密码。降级的意思恰恰是"这不算一个确信的答案"，`?` 在任何语境里都读作
"不确定"，不需要教。降级标记贴在词尾（`元素?` / `容器 8/9?`），不单独占一段。

**什么时候不显示计数**：最深层。那是 refinement 给出的答案本身，也是每次预览的起始状态；给它标 `9/9` 是冗余，
而且会让"计数出现 = 你动过"这条唯一的线索失效（确认行里它同样叫 `deepest` 而不是 `9/9`）。

**编号方向固定为 1..N**：`1` 永远是窗口框（`path[0]`），`N` 永远是答案本身。不采用"往上第几层"这种相对说法——
相对值会随用户所在的位置变化，同一个数字在两步之间含义不同。

**4. 一次性提示（one-shot hint）：不可见的交互等于不存在**

同一个提示槽（`ArmedHint { until, text }`）承担**两句话**，因为它们是两个不同的教学任务：

| 时机 | 文案 | 教什么 |
| --- | --- | --- |
| 会话开始（F5 之后立即） | `滚轮 / ↑↓ 换吸附层级` | **发现**：这个功能存在 |
| 本会话**第一次真正换层成功**时 | `吸附层级 8/9（1=窗口）· 滚轮 / ↑↓ 切换` | **解释**：那两个数字是什么 |

两句话都 2600 ms、锚在光标处，按下鼠标即撤销。

- **为什么开头那条要保留**：否则是死循环——用户不知道滚轮有用，就永远不会"第一次换层"，第二句话也就永远不会
  出现。发现必须在第一次使用**之前**，解释必须在数字出现**的同一刻**，这是两件事，所以两句话。
- **为什么换层那句要动态生成**（`level_hint(level, total)`）：它出现的那一屏上，用户刚把计数从"没有"变成 `8/9`，
  句子里的数字必须就是此刻标签上的数字。存的也是**文本**不是模板（`ArmedHint.text`）：提示一旦武装，内容不再
  随状态变化，否则用户停止走动后计数器还在跳，反而像 bug。
- **`1=窗口` 是这句话的重点**：光说"层级"没用，没人能猜出哪一端是窗口。这句话说完之后，用户下一次看到 `容器`
  就知道自己在往哪走。
- **每会话只教一次**（`hint_taught`），**但这句还挂在屏幕上的那段时间里，继续换层会让它跟着走**
  （`should_teach(taught, showing) = !taught || showing`）：一句话说自己 `8/9`、而标签已经写着 `3/7`，
  比没有这句话更糟。提示消失之后不再复活——那时候标签自己就能读（`容器 8/9`）。
  这条是 ③ 的交互原型（`prototypes/chain-rings-demo.html`）第一次跑起来时暴露的：原实现把数字冻在了
  第一次换层那一刻，而滚轮连滚三格就会和标签对不上。
- 提示挂在墙上时钟上，而静止的光标本身不产生重绘，所以 `on_hover_tick`（250 ms）在提示还在时保持一次重绘，
  让它准时消失（`hint_text()` 一旦为 `None` 就自然停）。

**5. 整窗 vs 元素的视觉区分**

| 答案 | 画法 |
| --- | --- |
| 元素（refinement 的答案，或走到 chain 中间某层） | accent 洗色 + **双倍**描边（"这是预览"） |
| 整窗（v1 兜底，或用户一路走到 `path[0]`） | **中性 hover 洗色 + 单倍细边** |

后者的意义：`deep_target.kind == TopLevelWindowFrame` 是"我们没能钻到窗口以下"，它不该看起来像一个确信的元素
预览。柱状之外的层级（`deep_path_local`）仍然只画细轮廓，不洗色。

**6. 导出不含 UI**

`OverlayRenderer::render_export` 在**渲染器这一层**把 `hover_bounds` / `preview_bounds` / `path_bounds` /
`preview_label` / `preview_is_window` / `hint` 全部清零（调用方忘了也安全），overlay 的导出路径同时也清零。
标注可以进产物，交互提示永远不进。

**7. 刻意不做的**

- 不给遮罩（mask）做淡入淡出：遮罩是"当前画面之外的全部"，它是一个**事实**，不是一个可以渐变的观点。
- 不在确认时做动画：确认要的就是"立刻"。
- 不用动画"填"等待：答案没到就不动（停在已知的矩形），动了反而是在说谎。

**8. 门禁与单测**

- 纯函数单测：`preview_label` 3 条（最深叫 `元素`、不计数 / 走层级叫 `容器` 并计 `n/N`、走到最外层叫 `窗口 1/N` /
  `窗口` 与 `?` 的组合），`level_hint` + 确认行的编号一致性 1 条，`transition` 6 条。
  `cargo test --lib` 373 passed。
- 两个实机探针必须同时过（这一步只碰 paint 层，但没有例外）：`browser_element_probe`
  `asserted=41 passed=41 provider_hit_available=52 finer=0`、`explorer_rule_probe`
  `12/25 median_area_pct=65.8 available=25/25 finer=0`。

### 5.22 整条链轮廓（③）设计草案（2026-10-06，尚未实现）

**要解决的问题**：走动时"我现在在哪一层"读不出来。现状是静止时已经画了"选中层之外的祖先"（`deep_path_local`
的 `take(selected)`，最深层默认全画），所以 ③ 的**增量只在走动中**：用户往上走到 `容器 3/7` 之后，
里面那 4 层（包括原来的答案）完全看不见，只能靠标签上的数字相信"里面还有东西"。

**视觉编码**（最外 → 最内单调递进，方向靠深浅读出来）：

| 环 | 线宽 | 颜色 |
| --- | --- | --- |
| 祖先（选中层之外） | 1 px | 边框色 × 45%（= 现状） |
| **选中层** | 2× + accent 洗色 | 不变（"你在这"只有一个最强信号） |
| **内层（选中层之内）** | 1 px | 边框色 **× 45%（与外侧同浓度；③ 新增）** |

**内层为什么不能更淡（2026-10-06 实测，v2 原型量像素）**：内层环画在选中层的洗色**之上**，而两者是
**同一个色相**（accent 叠 accent），所以"更淡"很快就读不出来。同一个环在不同浓度下与背景的 RGB 差之和：

| 内层浓度 | 环像素 | 背景（洗色后的深色代码区） | Δ |
| --- | --- | --- | --- |
| 28%（原定） | `rgb(39,94,169)` | `rgb(24,54,100)` | **124** |
| 45% | `rgb(48,110,198)` | 同上 | 176 |
| 60% | `rgb(52,119,217)` | 同上 | 210 |
| 外侧环（对照，45% 压在深色底上） | `rgb(44,99,179)` | `rgb(0,0,0)` | **322** |

28% 只有外侧环对比度的 38%——这就是"内层环明明在画、却看不见"的数字。所以：**内外同浓度（45%），
靠"在选中层里还是外"这个位置区分，不靠浓度区分**；想调低就调低（参数栏里那个旋钮留着）。

**画序**：外侧环画在选中层洗色**之下**（它们是外面那圈上下文），**内层环必须画在洗色之上**——它们本来就在
选中层内部，画在下面会被那层洗色盖掉。产品的 `draw_window_hints` 现在是"路径轮廓 → hover 洗色 → preview
洗色 → 标签"一条直线，实现 ③ 时必须把内层环拆到 preview 洗色之后。

**层与选中层重合时合并（不画）**：某一环与选中层的四边最小间距 < 2 px 时，它其实就是选中层那条边——
画出来只会把选中层 2× 边框加粗成 3 px，看起来像描边糊了。真机上这种 1 px 内缩的包装层**很常见**
（v2 夹具里 `cm-scroller` 贴在 `code-block-viewer` 内侧 1 px、`text-message` 在它外侧 1 px），所以这条
规则在真机上一定会被用到，而不是实验室情形。判定后该层在图层栈里标「合并」，并写清差了几 px。

**调色板（2026-10-06 定，用户提出"主题色 + 捕获色分开"，数值经实测校准）**

一句话原则：**蓝色说"这是一条链上的层"，绿色说"这是你要的东西"**。三色分工：

| 角色 | 颜色 | 用在哪 | 实测对比度（遮罩之上）|
| --- | --- | --- | --- |
| 品牌蓝 | **#1f75db**（= 产品现有 `border_brush`，rgb(31,117,219)） | 未遮罩内容上的 chrome（确认选区、把手）、hover 幽灵、遮罩的品牌染（≤10%） | 未遮罩内容上本来就够 |
| 环蓝 | **#4a9bff**（同色相浅一档） | 画在**遮罩之上**的链环（外侧 / 内层） | 外侧 **3.07 / 3.49 / 3.85 : 1**（1/9、3/9、4/9 三层实测）；内层 2.1–2.2:1（压在选中层洗色上） |
| 捕获绿 | **#1bb15f**（rgb(27,177,95)） | **将被截下的那一层**：边框 + 洗色（18%）；整窗兜底仍用中性灰 | 边框 **3.16:1**；浅底 7.5:1；被蓝染的底上仍有 3.15:1 |

两条实测结论（都是这次量出来的，不是审美判断）：

1. **遮罩不要用主题色去染**。30% 的蓝染把整屏底色推向环的色相，蓝环掉到 1.2–1.8:1（连绿色边框也掉到
   3.15）。中性黑负责压暗、主题色最多 **≤10%** 只是品牌感。顺序也要对：**先压黑，再叠主题色**（反过来等于
   在冰蓝上再刷一层灰，颜色变脏）。
2. **产品那个 #1f75db 在深色内容上太暗**：满浓度对比度也只有 2.02:1，再叠遮罩就没法看。所以环线用同色相
   浅一档的 #4a9bff——这不是引入第二个品牌色，而是同一条蓝色刻度上的两档（600 / 400）。若坚持单色，只能把
   环的浓度提到 80–100%（实测 #1f75db@100% 在浅底 4.63:1、深底 2.02:1），代价是链和选中层抢注意力。

**内层环为什么读起来弱一点**：它压在选中层的**绿色洗色**上，等于"彩色线压彩色底"，实测 2.1–2.2:1。
这是可接受的——它本来就是上下文，不是要你确认的东西；想更清楚就把内层浓度从 60% 提到 75%。

**绿色什么时候出现（用户问"大多数情况应该是蓝色吧"）**——按状态列清楚，因为颜色本身就是状态信号：

| 状态（`RenderView` 里的矩形） | 颜色 | 出现频率 |
| --- | --- | --- |
| `preview_bounds` = **即将被截下来的那一层**（refinement 的答案，或你走上去的容器） | **捕获绿** | dwell 之后（100–300 ms）就一直有，直到确认 |
| `hover_bounds` = 光标下的**窗口框** | 品牌蓝（`border_brush`，现状不变） | 打开遮罩就有 |
| 链环（外侧 / 内层）③ 新增 | 环蓝 | 有元素答案时才有链 |
| 确认之后：选区边框 + 八个把手 | 品牌蓝（现状不变） | 确认后（这是"已截下的结果"，不再是"将要截的"） |
| 整窗兜底（只答到窗口） | **中性灰**，标签写「窗口」 | 检测不到元素时 |

**所以蓝多绿少是设计，不是失衡**：蓝是背景语言（上下文 + 结果），绿是**状态标记，全屏最多一个**。
每帧大致是"≤7 条蓝环 + 1 条蓝的窗口边框 + 最多 1 个绿盒子"。

**绿的频率 = 检测到元素的频率**，用已有数据估：

* Chromium 夹具 52 个采样点全部有元素答案（`provider_hit_available=52`）→ 浏览器里绿色几乎常驻；
* Explorer 网格 25 点里 12 点落到控件级盒子（`control_level_points=12/25`），另外 13 点只到窗口框
  → 那类界面上绿色大约一半时间出现，另一半是中性灰的"整窗兜底"。

**正因为两者频率相当，"整窗兜底"更不能用绿色**：它和"找到了元素"是两种结果，而且**会改变你截到的东西**。
如果全用蓝（或全用绿），用户就必须读标签才知道自己将要拿到哪一个；分成绿/灰之后，"有没有绿"本身
就把这件事说完了（标签里的「元素」/「窗口」和降级标记是冗余的第二通道，也照顾色觉异常）。

**产品侧一条重合规则**：`hover_bounds`（光标下的窗口框）与 `path[0]`（链的最外圈）是**同一个矩形**，
产品现在会画两次蓝边（hover 边框 + 环 0）。实现时要么让环跳过与 `hover_bounds` 相等的路径项，
要么在画 hover 边框时不画环 0——总之同一个矩形只描一次。

**滚轮连滚时"哪个是我的"（用户提出：连滚会出现很多边框，才需要用绿色区分）**

这正是绿色的岗位，规则可以写成一个不变量：**有元素答案时，绿恒 1 个；蓝 ≤7 个；灰 0–1 个**。
每个蓝框都是"再滚一格就会变成绿"的候选——颜色标记**当前**，位置与圈数标记**还有哪些候选**。

不靠"给不同层级不同颜色"来区分：那会把画面变成彩色框森林，破坏"只有一个最强信号"这条底线。
方向信息已经由**位置**给出（在绿框里还是外）加上浓度阶梯（离选中层越远越淡）。

连滚实测（v2 的「⟳ 连续滚 6 格」按钮，每 140 ms 一格，逐格采边框像素）：

| 采样 | 选中层 | 边框像素 | 画出的盒子 |
| --- | --- | --- | --- |
| 1 | 9/9 | `rgb(27,177,95)` 绿 | 捕获 1 + 环 4（外 4 · 内 0）· 塌缩 4 |
| 3 | 7/9 | 绿 | 捕获 1 + 环 5（外 3 · 内 2）· 塌缩 2 · 合并 1 |
| 6 | 4/9 | 绿 | 捕获 1 + 环 5（外 2 · 内 3）· 塌缩 3 |
| 8 | 3/9 | 绿 | 捕获 1 + 环 5（外 2 · 内 3）· 塌缩 3 |

即：**"我的那一个"唯一（捕获恒 1），而环的集合每格都在重算**（塌缩 / 合并 / 内外比都在变）——
所以"很多边框同时在"是常态，而可读性靠三件事冗余保证：**绿** + **2× 描边 + 洗色** + **贴着它的尺寸标签**。

一个诚实的边界：缓动进行中的 101 ms 里，绿框处在两层之间（它是插值矩形，不等于任何一层的目标框），
所以瞄着"目标框的边"去采像素会采到背景。眼睛跟的是一个连续移动的对象，没有问题；如果想要绿色
**瞬时**贴到新层，就得取消这段缓动（代价是回到"矩形跳一下"）。建议保留缓动。

**产品侧要改的画笔**（`win/d2d.rs::recreate_resources`）：`border_brush` 不动；`preview_fill_brush`
改成捕获绿 18%；新增一个满浓度的捕获绿画笔给 snap 预览的边框（现在预览用的是 `border_brush`）；
新增一个**可变色**的环画笔（≤7 次 `SetColor`/帧，和标注笔画一样复用同一支笔，不做分配）；
`mask_brush` 保持中性黑（`MASK_ALPHA` 0.45），品牌染若要做，另外叠一层 ≤10% 的主题色。

**该画哪些环：锚点 + 几何塌缩**（用规则而不是调参来控制"9 层会不会炸"）：

1. 锚点必画：`path[0]`（窗口框）、选中层外侧那个、**选中层**、选中层内侧那个、以及（可选）最深那个原答案；
2. 其余环从外到内扫一遍，只有当它与"上一条已画的环"的**四条边距离都 ≥ 阈值**（初值 8 px）时才画；
   9 层里那些挤在 1–2 px 内的包装盒本来就在画双线，自动被吃掉；
3. **与选中层重合的锚点例外地不画**（间距 < 2 px 即"就是那条边"，见上一节）；
4. 环数**强制 ≤ 7**：超过时丢非锚点环（先丢间距最小的，再丢离选中层最远的）。锚点最多 5 条
   （窗口 / 选中层±1 / 选中层 / 最深答案），所以这个上限永远可达，不必靠丢锚点来满足。

**性能**：真正的代价是 **present 次数**（每次 present 重画整个表面，实测 `renderer_ready ~3 ms` @4K），
不是矩形条数。所以：静态几何、不参加 101 ms 过渡、不渐隐 → **零额外满屏帧**、新增 0 次跨线程调用、
新增 1 个低透明度画笔。若以后真觉得吵要"停下就淡出"，代价约 6 个满屏帧/次走动，并且内层线会与外层缓动矩形
不同步（那才是"几何在撒谎"）。落地时同时给 overlay 加一个 present 计数（次数 + last/max µs），
让"多画 6 条线"变成数字而不是口头保证。

**改动面**：`deep_path_local() -> Vec<Rect>` 换成带角色的环集合（`Outer | Selected | Inner`），
规则在这里实现（可纯函数单测：给定 path + index 断言画哪些环），绘制层只按角色选笔刷。

**验证**：① 规则纯函数单测（含"12 层路径 → 画出的环 ≤ 7，被吃掉的都是边距 < 阈值的"）；
② 离屏渲染 + 像素断言（选中层边框坐标、内层线坐标、粗细/亮度关系），把"看得清"变成能红的东西；
③ 真机目视（③ 是 paint 层，两个实机探针盖不到）。

**交互原型**：`prototypes/chain-rings-demo.html`（1280×720 的"显示器"，9 层真机形态 + 4 层 + 2 层三种场景；
滚轮 / ↑↓ 换层，101 ms OutQuad 缓动；可切「当前实现」与「③a」直接对照；参数可调；侧栏给出每一层"画 / 跳过"
与原因）。打开方式：直接双击，或 `npx serve prototypes` 后访问（`file://` 也可以）。
**v2 原型（`prototypes/chain-rings-demo-v2.html`，用户提供）**在同一页里把状态也做成了开关：
`模拟整窗兜底`（只截断"链"，页面照旧 —— 对应真机 `kind == TopLevelWindowFrame`、`path.len() == 1`）、
`加降级标记 ?`（对应 `PrecisionOutcome::Unavailable`）、`⟳ 连续滚 6 格`（每 140 ms 一格，用来看
"很多候选盒子同时在场"时绿色跟不跟得住），以及右栏的真实 present 计数。

**v2 原型**（`prototypes/chain-rings-demo-v2.html`，用户提供 + 修一处 bug）：把"口头保证"换成**本页真实渲染循环的计数**
（脏标记驱动，稳定期真的不重绘）。它给了 §5.22 几个必须固化的决定，以及第一组实测数字。

实测（9 层形态，`resetMetrics()` 后计数；数字来自 demo 自己的 present 计数器）：

| 场景 | present（满屏重绘） | 其中"环动画"帧 | 结论 |
| --- | --- | --- | --- |
| ③a 静止 | 2 | 0 | 静止期 0 重绘（环是静态几何） |
| ③a 走 3 层 | 16 | **0** | 帧数来自选中层的 101 ms 缓动**本身**，环不额外产生帧 |
| ③a 之后 3 s | 17 | 0 | 回到安静（多的那一帧是提示到点消失） |
| ③b 走 3 层后淡出 | 18 → **26** | 7 | 整链淡出 = **≈8 个额外满屏帧**，之后完全安静 |
| 「环参与渐隐 180 ms」+ 连走 6 层 | 29 | **12** | 反面演示：环动画的帧数 ≈ 走路本身的帧数（16→29），且与 101 ms 的选中层节奏对不齐 |
| 「当前实现」连走 6 层 | 17 | 0 | 对照：帧数相当，差别在**画得多且乱**（不塌缩、不封顶），不在帧数 |

**据此固化的决定**：

1. **上限 7 是强制的，锚点永不丢**：超过 7 条时丢**非锚点**（先丢间距最小的，再丢离选中层最远的）。
   不变量：锚点最多 5 条（窗口 / 选中层上一个 / 选中层 / 选中层下一个 / 最深答案），所以"≤7"永远可达，
   不需要为了满足上限破坏"窗口框在最外、答案在最内"这两个参照。12 层压力场景实测：`画 7 · 塌缩 0 · 丢弃 5`，
   留下的是 `{0, 3, 4, 5(选中), 6, 7, 11}`——**锚点 + 离选中层最近的填充**。
2. **③b 的"活动"包含移动光标**（不只是换层）：否则用户停下滚轮、把鼠标移向别处看清时，链正在淡出——
   而光标移动正是"我还在这层看"的信号。产品侧的对应物是 `on_hover_tick` 的保活语义。
3. **③b 淡出按 1/8 量化**（1200 ms 空闲后，240 ms 内 8 步）：把额外满屏帧**封顶在 8 个**，实测 7 帧。
4. **环不参加动画这条继续保留**，现在有数字：环渐隐 180 ms 在 6 次走动里多花 12 帧，而且新环淡入 / 旧环淡出
   与选中层的 101 ms 缓动错拍（就是"几何在撒谎"那幅画面）。demo 把它留成显式开关叫**反面演示**——
   这个做法值得沿用：把反例做成可勾选的开关，比在文档里争论便宜。
5. **产品侧要加的计量**：present 次数 + "环动画帧" + 绘制耗时 last/max（demo 的右栏就是这个形状）。

**demo 说明不了的事**（别把它的 µs 当产品数字）：它的 `draw()` 是 1280×720 的 JS canvas，而产品的 present 是
3840×2160 的 D2D 整面重绘（实测 `renderer_stage elapsed_ms=3` 量级）。**能迁移的是帧数**，不是微秒。
另外它的层是合成的，说明不了"链是 provider 验证过的路径"这件事（跳过的无绘制包装层不在链上，见 §5.15）。

**v2 的一个 bug（已修）**：`const q = (s) => document.getElementById(s)` 但所有调用点都传 `"#id"`
（`q("#mPresent")`），于是**永远返回 null**；`init()` 里第一处 `renderStack()` 就抛 `Cannot set properties of
null`，把同一条链上的 `bindEvents()` / `fitStage()` / `requestAnimationFrame(frame)` 一起带走——页面既不画
画布也不响应滚轮键盘。修法是让 `q()` 接受两种写法（`s.replace(/^#/, "")`）。

**状态**：草案 + v2 原型。等真机看完原型再决定是否实现，以及 ③b（淡出）要不要做（现在有 8 帧的价码）。

### 5.23 内嵌 UI 字体：文案是白名单（2026-10-06，已修 + 已设门禁）

overlay 的 DirectWrite 文本（尺寸标签、放大镜信息条、层级提示）用的是**内嵌的 HarmonyOS Sans SC 子集**
（`src-tauri/fonts/harmonyos-sans-sc-subset.ttf`，`include_bytes!` 进二进制）：8.5 MB 的系统字体裁到
app 自己画的那几十个字形，换来"任何机器都不需要装字体"。

**这个子集是硬白名单**：缺字形不会报错、不会打日志、不会抛异常——DirectWrite 会**逐字形回退**到系统字体，
于是字符串一半是 HarmonyOS、一半是雅黑（回退被拒时才是豆腐块）。只有眼睛看得出来。

它已经**同时漂移了三处**（都由 §5.21 的新文案暴露）：

| 问题 | 证据 |
| --- | --- |
| `include_bytes!` 读的那份是**旧的** | `src-tauri/fonts/…ttf` 7044 B / 57 码位 / 10 个汉字，而脚本写出的 `subfont/…ttf` 是 7992 B / 59 / 13——脚本里的安装步骤是**注释掉的** `Copy-Item`，"重做字体"实际上是个两步手工仪式 |
| 两份都**不含新文案的字形** | 元素 容器 窗口 吸附层级 换 （） · ? ↑ ↓ 一个都没有；连 `滚轮缩放` 在安装份里也缺 |
| 字形表是 `.ps1` 里手写的 `U+xxxx` | 也正因为如此对编码脆弱：PowerShell 5.1 用 ANSI 读无 BOM 的 `.ps1`（旧脚本注释里记着这件事） |

**现在的流程**（`subfont/subset.ps1` 是唯一入口；**字体里只有真正画出来的字**）：

1. **清单在 Rust 里**，而且只有一份：`win/d2d.rs::overlay_drawn_strings()`。凡是文案有生产者的，它
   **调用生产者**而不是抄一份文本（`preview_label` 的四种状态、`level_hint`、`LEVEL_HINT`、改成 `const`
   的 `INFO_HINTS`、尺寸标签/缩放刻度/色值格式的取值形态）。`subset.ps1` 先跑
   `cargo test --lib write_drawn_text_for_the_font_subset -- --ignored`，把这个清单写成
   `subfont/drawn-text.txt`（生成物、已 gitignore，18 条字符串 / 31 个非 ASCII 字符）。
2. 必需集合 = **这份清单里的字符** ∪ **可打印 ASCII（U+0020–U+007E）整段**。后者不能靠清单：面板画的是
   **计算出来的**文本（色值、坐标、尺寸、百分比），任何"举例式"清单都必然差一个字形——第一版就是这样
   被 `cargo test` 抓到的：缺 `3`（`hsl(359,100%,100%)`）。用户输入的标注文字**故意不在集合里**：它可以
   是任何字符，本来就该回退到系统字体。
3. `subfont/build_subset.py`：读清单 → **拒绝过期清单**（`drawn-text.txt` 比 Rust 源码旧就直接拒绝构建，
   防止拿到一份陈旧清单生成缺字形的字体）→ **守卫**：凡调用 DWrite 文本 API 的文件（今天只有 `d2d.rs`），
   其中任何未列入清单的非 ASCII 字面量都要报错并指出文件:行:字符 → fontTools 裁表 → **校验产物覆盖
   全部必需码位** → 安装到 `include_bytes!` 真正读的路径。
4. **安全网**（清单看不见的那一类：运行时拼出来的文本、`include_str!` 资源、将来的本地化）在 Rust 里：
   `the_embedded_subset_covers_the_strings_the_overlay_draws` 直接从 `INFO_EMBEDDED_FONT` 的 cmap 读
   覆盖率并要求清单里的每个字符都在；另一个测试检查这个 cmap 读取器本身（不虚构覆盖范围：报出的码位数
   不得超过 `maxp` 的字形数）。

**两次自证**（2026-10-06 实测）：

* 守卫抓到过我**自己**写的一个字：`#[ignore = "codegen … — …"]` 里的破折号 —— 它在绘制文件里、却不属于
  任何被画的字符串，于是构建直接失败并点名 `d2d.rs:2745 '—' (U+2014)`。规则很硬但很好遵守：**绘制文件里
  不要写不被画的非 ASCII 字面量**。
* 在 `LEVEL_HINT` 里加四个新字（试/用/龘/字）但不重建字体 → `cargo test` 红并指出是哪个字、哪条字符串；
  **只跑一次** `subset.ps1` → 测试转绿。

产物：两份 `harmonyos-sans-sc-subset.ttf` 均为 **14.3 KB / 126 码位**（ASCII 95 + 31 个真正被画出来的非 ASCII）。
对照：最早提交的 7.0 KB / 57 码位缺了已经画在屏幕上的字；中途"扫全 crate"的版本 16.6 KB / 139，多出来的
14 个是**从没画过**的字（store 测试里的 `不你图在好存截搜索`、日志里的 `§ — … →`），按"只收用到的"要求已经去掉。
顺手删掉了 `subfont/*.ttf.br`：crate 里没有任何东西读压缩块，它是个没有消费者的提交产物。

**体积 vs 性能（2026-10-06 实测，`cargo test --lib font_cost_probe -- --ignored --nocapture`）**：

14 KB 是**二进制体积**项，不是运行期项。运行期只有两处和字体有关，且都与"文件里有多少字形"无关——
只与"这一次真正被 shaping 的字形"有关：

| 项 | 实测 |
| --- | --- |
| 一次性注册（`AddFontMemResourceEx`，每进程一次；字节已在 `.rdata`，无磁盘 I/O） | **331 µs** |
| 每次绘制前的文本排版（`CreateTextLayout` + `GetMetrics`，尺寸标签/预览标签每帧都做） | 内嵌子集 **12.4 µs**（600 次）、Microsoft YaHei UI（**约 10 MB** 系统字体）11.4 µs、Segoe UI 11.4 µs |
| 与之相比，overlay 每帧的真实成本 | 一次整面 present，日志里 `stage=renderer_ready elapsed_ms=3`，节奏是 15 ms 的合并 tick |

也就是说：把字体从 10 MB 换成 14 KB，排版成本没变（差 1 µs，噪声级）；字形栅格化由 DWrite 的 glyph-run
缓存承担，也只针对真正画出来的那十来个字形。体积去向：`glyf` 12.6 KB / 14.6 KB；可打印 ASCII 95 个字形
8.4 KB，31 个被画的非 ASCII 字形（CJK、`×`、`（）`、`·`、`↑↓`）再加 5.9 KB（约 193 B/字形）。

想更小只有一个方向：把 ASCII 从 95 个裁到**实际用到的那 38 个**（十进数字、十六进制 A-F、格式串里的
`rgb hsl px S C P Z` 等）→ 9.9 KB，省 4.4 KB。代价正是第一版翻过的那一类：算出来的文本一旦出现没列举到的
字母/数字就回退到系统字体（当时缺的是 `3`）。所以除非二进制体积成为真实约束，建议保留整段 ASCII。
另外不能靠压缩：`AddFontMemResourceEx` 需要原始 TTF（这也是那个没人读的 `.ttf.br` 被删掉的原因）。

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
| 9 | 自建全屏 topmost 弹窗"从不被 `ElementFromPoint` 返回" | **订正**：那只在替身用系统 `STATIC` 类时成立——UIA 会跳过系统类的替身窗口；换成 overlay 那样的**自注册类**之后，UIA 每次都返回它（实测见 §5.7/§5.8）。这个误判直接导致精度补足在实机里一次都没跑过 |
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

**状态更新（2026-10-06）：上面 1–4、6 条已有结论，待办只剩两条**

| 旧条目 | 现状 |
| --- | --- |
| 1 app 与探针的差异 | **根因已定位并修复**：截图遮罩回答了每一次命中测试（用户日志 `last precision unavailable answer "SnapClipCaptureOverlay" 14/14`）。遮罩现在对**一次**命中测试返回 `HTTRANSPARENT`（`fc9abb3`），发布也改成"只看可见部分"（`fa76cc6`）；两个探针都改走产品链路（`3679b64`） |
| 2 Explorer 命中路径不可用 | **已解决**：UIA 的命中在 Chromium 上是**近似且不稳定**的（§5.19 实测同一点三次给出三个元素），MSAA 的 `accHitTest` 才是渲染进程真实命中，现已接入（`e2a7e05`）。Explorer 门禁 12/25 + 25/25 命中可用 |
| 3 文字块语义 | **已定并实现**：文字跑条可以作目标，但**控件的标签不算**（§5.19，`4c05249`） |
| 4 层级缓存新鲜度 | 已做"空批次不缓存"（`4d94077`） |
| 6 MSAA 迭代 `accHitTest` | **已实现**（`e2a7e05` + §5.16） |

**待办 1（最高优先）：设置通道 + 两个行为开关**

- 现状：两个开关都还没有用户可改的通道。
  - `capture/deep_select_text_runs` —— 代码里是 `DEFAULT_ADOPT_TEXT_RUNS = true`
    （`capture/window_detection/mod.rs`），已经按 overlay → `RefinementWorker` → 组合 provider →
    UIA/MSAA 两个 provider 贯通，所以"接设置"只需要换掉这一个常量的来源（§5.19）；
  - `capture/deep_select_visible_wrappers` —— **尚未实现**，即"跳过无绘制包装层"（§5.15 第 5 条）：
    可见部分没有绘制面积、或尺寸小于 N px 的包装层不作为终点。落点是
    `should_adopt_provider_box` / `should_adopt_msaa_box` 的判定。
- 需要的最小管道：①设置存储（读写 + 默认值 + 合法性校验）；②设置页两个开关；③**热更新到 overlay**
  （会话中改也生效，不必重启）——overlay 已有 `OverlayCommand` 通道，加一条"策略变更"即可，
  也可以让 worker 读一个共享原子量。
- 验收：改开关后无需重启即生效；文字开关**两种状态都有夹具断言**（开：`nested-*` 取到文字跑条、
  按钮仍是按钮；关：回到取盒子）；包装层开关需要**新增**"无绘制包装层"夹具行（开=跳过、关=现状）；
  Explorer 12/25 与延迟不退化；两个默认值同时写进文档与设置页文案。

**待办 2（低优先）**：Firefox 未验证（若它不暴露元素级矩形，验收写明"整窗降级即合格"）；
CDP / 扩展通道按 §5.13 / §5.12 作为**独立功能**再做（跨域 iframe 已从"CDP 才能做"里划掉：
MSAA 通道在 §5.20 订正后 4/4 能进到帧内部，CDP 现在只剩"整页盒表 + DOM 属性"这些语义能力）。

---

## 9. 建议的实施顺序（每条独立提交、独立验证）

> 本表是**当时的计划**；实际落点见 §5.x 与 §8 的状态更新（例如 S3 的"命中途径"最终由 §5.11 的
> MSAA 源取代，S4 的粒度策略落在 §5.19）。

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
cargo test --lib                       # 基线 fbcfd8d：354 passed / 0 failed / 1 ignored；当前 373 passed / 2 ignored
cargo check --all-targets              # 0 warnings

# 需要浏览器与（可选的）资源管理器窗口；都是 #[ignore] 的人工探针
cargo test --lib browser_element_probe   -- --ignored --nocapture   # 期望 asserted=41 passed=41、cross-ready=Some(…)、available=52、finer=0
cargo test --lib explorer_rule_probe     -- --ignored --nocapture   # 期望 control_level_points=12/25、median_area_pct=65.8、available=25/25
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
