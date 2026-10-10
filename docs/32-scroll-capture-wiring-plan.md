# SnapClip 滚动截图 V2 · 装配根接线方案与 TDD 任务书（P7）

> **上游设计权威**：`docs/30-scroll-capture-design-v2.md`（下称 V2）。本文与 V2 冲突时以 V2 为准；
> V2 未覆盖的装配细节以本文为准，并按 §0.1 的关系表回填 V2。
>
> **本文的性质**：**方案 + TDD 任务书，不含任何代码改动**。本文回答一个问题——
> "V2 的滚动截图已经按 `§34` 六组判据验收完毕（标签 `scroll-p6` = `dacb99a`，2026-10-10），
> 但产品里按什么键都进不去；把它接到产品上需要哪些任务、每个任务的 RED 是什么、做完怎么证明。"
>
> **文档版本**：v1 · 2026-10-10 · 协议版本沿用 `docs/31` 的 C1–C11（本文不重定义，见 §0.1）。
>
> **状态词表**（与 `docs/31 §0` 相同）：`[ ]` 待办 ｜ `[~]` 进行 ｜ `[x]` 完成 ｜ `[!]` 阻塞 ｜ `[-]` 取消。
>
> **提交粒度**：一个任务 = 一个提交，RED→GREEN→REFACTOR 三阶段落在**同一个提交**里（`docs/31 §2.1`、C1）。
> 本文的每个任务块给出 RED/GREEN/REFACTOR 与**预期失败原因**，不给出实现代码。

---

## 0. 文档元信息

### 0.1 本文与其它文档的关系（五条声明）

| # | 声明 |
|---|---|
| 1 | **协议不重定义**：提交/推送协议（C1–C11）、提交信息模板、门禁命令、测试分层、回滚演练全部沿用 `docs/31 §2`–`§4`。本文只引用条款号，不复制正文——同一套规则写两遍就会漂移，而漂移本身正是 `docs/31 P6.06`（D-15）建了门禁要挡的东西。 |
| 2 | **V2 不动**：本文只做**定点增补**（新章节挂到 V2 已有的 `§27`/`§28`/`§35`/`§36` 之下），不覆盖 V2 的原文。增补清单见 §12.2。 |
| 3 | **`docs/31` 不动**：`docs/31` 是 P0–P6 的台账，已收口并打标签。本文是**第二本任务书**（P7），与 `docs/31 §1.1` 的阶段表并列。两者共用同一套协议与门禁，但各自的 `[x]`/`[!]` 只记自己那一段。 |
| 4 | **第一性原理栏必填**：每个任务的元信息行里 `第一性原理` 必须填 `docs/30 §2.1` 的 `F?` 编号或 `G?`/`N?` 目标编号。填不出来的任务当场删除（`docs/31 §0.4`）。 |
| 5 | **引用一律带行号**：本文对代码的每一处断言都写成 `路径:行号`。只写"见某模块"的句子不允许进入本文——它是 `D-15` 要清理的形态。 |

### 0.2 本文为什么存在（一句话）

V2 的滚动截图是一条**已经实现、已经测量、已经验收，但没有任何生产调用者**的路径。
`crates/snapclip-capture/src/session.rs:365` 的 `begin_scroll()` 与
`crates/snapclip-capture/src/windows/overlay/render_submit.rs:29` 的 `watch_scroll_preview(...)`
各自带着 `#[allow(dead_code)]`，它们欠的就是**装配根**这一笔账。
用户裁决（m21615）：**先出方案与任务书，不写代码**；**滚动截图的全局热键是 F7**。

### 0.3 编写前的必做检查（逐项结果，2026-10-10 实测）

| # | 检查项 | 结果 |
|---|---|---|
| 1 | F7 是否已被占用 | ✅ 空闲。全仓 `F7`/`0x76` 唯一命中是 `crates/snapclip-capture/src/windows/win/d2d/tests.rs:50` 的颜色注释 |
| 2 | `WM_APP + N` 已占用哪些偏移 | +1、+2（`apps/snapclip/src/tray.rs:43-44`）、+17、+18、+19、+43、+44、**+45**（`crates/snapclip-capture/src/windows/overlay.rs:104` 的 `SCROLL_READY_MESSAGE`，已有冲突断言 `overlay/tests.rs:57-77`）⇒ 新消息 id 只能取 ≥ +46 且必须进同一张断言表 |
| 3 | 全局热键基础设施能否承载第二个键 | ❌ **不能**。`crates/snapclip-capture/src/windows/hotkey.rs` 全文 114 行只有一个 `CAPTURE_HOTKEY_ID: i32 = 0x5343`（`:18`），`register_capture_hotkey(window)`（`:57-68`）与 `unregister_capture_hotkey(window)`（`:71-75`）把 id/modifiers/key 全部硬编码，`HotkeyError::message()`（`:38-51`）的两条文案写死 `"F5"` ⇒ F7 需要一个**键表**，不是一次复制粘贴 |
| 4 | 捕获 worker 能否交付"窗口"的冻结帧 | ❌ **不能**。`crates/snapclip-capture/src/windows/providers.rs:500` 的 `ProviderKind::WgcWindow => Err(CaptureError::ProviderUnavailable(...))`；`:601-602` 只在**候选排序**里把 `WgcWindow` 排在首位；`:1214-1217` 的注释逐字说明它"存在是为了让滚动路径给自己的帧源一个名字" ⇒ 冻结帧仍然只来自**显示器**，窗口级帧源是滚动驱动自己开的 |
| 5 | 面板按钮的点击命中区是否存在 | ❌ 不存在。`crates/snapclip-capture/src/scroll/panel.rs:114` 有 `pub(crate) buttons: [Rect; 4]`，但 `panel.rs` 没有任何 `hit_test`/`button_at`，全仓唯一读者是 `crates/snapclip-capture/src/windows/win/d2d.rs:1448`（**仅绘制**） |
| 6 | 覆盖层是否已有"滚动会话"的输入分支 | ❌ 没有。`crates/snapclip-capture/src/windows/overlay/input.rs:93` 的 `on_left_down` 在非 `Selecting`/`Selected` 时直接 `return`；`:125` 的 `on_left_up` 同形 ⇒ 面板点击今天会落到"空选择"分支 |
| 7 | `PreviewStream` 是否支持消费者→生产者的请求 | ❌ 不支持。`refresh_window`（`crates/snapclip-capture/src/scroll/preview.rs:448`）的第一个参数是 `&mut RecoveredImage`，而画布归驱动线程 ⇒ 覆盖层拿不到；`PreviewUpdate` 是单向的（V2 `:3620-3630` 已登记） |
| 8 | 普通截图的产物今天交给谁 | ❌ **没有人**。`apps/snapclip/src/adapters.rs:89` 在 `on_completed` 里发布的 `CaptureEvent { artifact: None }`；`copy_image` 全仓唯一调用点是 `apps/snapclip/src/history/view.rs:426`；`save_publication` 的唯一生产调用者是剪贴板 ingest（`crates/snapclip-history/src/ingest.rs:224-230`） |
| 9 | 从 shell 能否驱动滚动会话 | ❌ 不能。`scroll::{session,loop_control,preview,panel,ports}` 全是 `pub(crate)`，`apps/snapclip` 只能看到 `scroll::export`、`Axis`、`ScrollDiagnosticCode`（`crates/snapclip-capture/src/scroll/mod.rs:56,64,89`） ⇒ 装配根只能落在 capture crate 内（见 ADR-18） |
| 10 | `scroll/` 的平台纯净门禁今天扫什么 | `tools/check-dependency-direction.ps1:109` 的 `Select-String -Pattern "crate::windows\|crate::sampler\|winapi\|windows_sys\|windows::"`，无代码剥离 ⇒ **注释里的这几个字面量也算**（`scroll/` 下 17 个文件，2026-10-10 实测） |
| 11 | 帧源的三个零件是否已经互相认识 | ❌ **不认识**。`WgcFrameSource::new(backend: Box<dyn FrameBackend>, axis)`（`crates/snapclip-capture/src/windows/scroll_source.rs:236`）与 `WgcFrameBackend::open(device: Arc<GraphicsDevice>, handle: isize)`（`:351`）的桥接代码**只存在于探针**（`windows/scroll_probe.rs:4487-4488`，而 `windows/mod.rs:38-39` 把它门在 `#[cfg(test)]` 里）；`ScrollFrame`（`windows/providers.rs:656`）既不实现 `FrameBackend`，也没有任何转成 `Observation` 的代码；`ScrollSourceRuntime::open`（`scroll_source.rs:445`）**零调用者（连测试都没有）** |
| 12 | Win32 侧有没有 `impl ScrollActuator` | ❌ 没有。`windows/scroll_actuator.rs` 里有 `choose`（`:179`）、`InjectRequest`（`:207`）、`InjectionTarget`（`:229`）、`inject`（`:292`）、`Win32Injection`（`:332`），但**没有实现端口**；真实实现只有测试/探针三处：`scroll/session.rs:1115`（`SilentActuator`）、`scroll/loop_control.rs:2009`（`LinkedActuator`）、`windows/scroll_probe.rs:4393`（`Win32ScrollActuator`）⇒ 适配层（`WheelRouting`/`TargetProbe`/`Choice` 的会话内取值 + `InjectOutcome` 的构造）尚不存在 |
| 13 | `ScrollOutcome`/`ScrollConfig`/`ScrollCommand` 是否存在 | ❌ **三个都零定义**。V2 `§27.1`/`§27.2` 只承诺了名字；`ScrollOutcome`/`ScrollConfig` 只出现在 `crates/snapclip-capture/src/scroll/session.rs:190,192` 的 doc 里，`ScrollCommand` 只出现在**测试断言的字符串字面量**里（`session.rs:1040`/`:1058` 的 `production.contains("Mutex<Option<ScrollCommand>>")`）⇒ ADR-20 的"容量 1 覆盖式"必须按 `SetFollow` 的既有形状**新造**，不能引用一个不存在的枚举 |
| 14 | 驱动主循环叫什么 | `ScrollDriver::run`（`crates/snapclip-capture/src/scroll/loop_control.rs:773`），**不存在** `driver_main`；它在 `:779` 按值返回 `ScrollSession`，由 `ScrollRuntime::teardown`（`scroll/session.rs:737`）交回 ⇒ "谁读 `disposal()`"就是装配根的问题（今天零消费者） |

### 0.4 编号与文件规则

| 规则 | 内容 |
|---|---|
| 任务号 | `P7.01` … `P7.13`（阶段 7 = 装配根接线） |
| 批次 | `[P7-A]` 入口与目标解析 ｜ `[P7-B]` 交接与驱动 ｜ `[P7-C]` 面板与交互 ｜ `[P7-D]` 产物与诊断 ｜ `[P7-E]` 收口 |
| 侧分支 | `blocked/P7-??-<slug>`（沿用 `docs/31 §3.4`） |
| 阶段标签 | `scroll-p7` |
| 新增文件 | 必须先建 `#[cfg(test)]` 骨架（`docs/31 §0.5`） |
| 消息 id | 新值只允许取 `WM_APP + 46` 及以后，且必须进 `overlay/tests.rs:57-77` 的同一张断言表（`assert_ne!` 逐项 + `let taken = [...]` + `assert_eq!(offset, ..)`） |

### 0.5 已实测的本机事实（本次接线会依赖它们）

| 事实 | 值 | 出处 |
|---|---|---|
| 滚轮路由 | `SPI_GETMOUSEWHEELROUTING = 2 (MOUSE_POS)`；`SPI_GETWHEELSCROLLLINES = 3` | V2 `§24.6.1`、`docs/31` 的 `E-INJECT-1` |
| `SendInput` 打到哪里 | **光标所在窗口**，不是前台窗口（非前台目标实测 400 px、拿到前台的自己消费 0 px） | V2 `§24.6.1` |
| 小窗口下的坐标空间 | `PostMessageW` 用 **screen** 坐标 4 格 = 400 px，用 **client** 坐标两次都是 0 px ⇒ **必须屏幕坐标** | `E-INJECT-1` 矩阵 |
| 窗口级 WGC 的交付尺寸 | **等于 DWM 可见边界**（`DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`），1188×894；`GetClientRect`(1184×892) 与 `GetWindowRect`(1200×900) **都不是** | `docs/31` 的 `P3.10`（run 2/run 3/run 4） |
| 取消延迟的量级 | Max 44 ms / P50 41 ms（`mid-flight` 0 ms、`parked` 44 ms） | `docs/31` 的 `P3.10` |
| 本机 DPI | 3840×2160 @ 150%（`GetDpiForSystem = 144`） | V2 `§24.1.1` |

---

## 1. 现状：为什么今天按 F7 什么都不会发生

### 1.1 一张图

```
                    今天                                        接线后（本文 §3）
用户按 F5 ──► overlay 线程注册的唯一热键 ──► start_session()
                                                    │
                                          capture worker 冻结显示器
                                                    │
                                          overlay 画选区 / 悬停 / 标注
                                                    │
                                        Enter ──► confirm ──► ExportJob ──► export worker
                                                    │                              │
                                               on_export_ready ◄────────────────────┘
                                                    │
                                        sink.on_completed(artifact)   ← apps/.../adapters.rs:89 发布 artifact: None
                                                    │
                                        （进程内再没有任何消费者：不写剪贴板、不入历史）

用户按 F7 ──► 没有任何人监听这个键
```

```
                    接线后（目标形状）
用户按 F7 ──► overlay 线程的第二个热键 ──► start_scroll_entry()
                                                    │
                          光标下的窗口（复用既有 hover 快照）⇒ ScrollTarget
                                                    │
                            DWM 可见边界 ⇒ ScrollPlan{cross, extent, crop, axis}
                                                    │
                     capture worker 冻结显示器（只为选区 UI 与 DPI，不作为原点像素）
                                                    │
                    Enter/单击 ──► CaptureSession::begin_scroll() ⇒ ScrollHandoff（会话结束）
                                                    │
                        ScrollRuntime::start(plan, ‖ 驱动线程自建 GraphicsDevice + WGC 源, Win32ScrollActuator)
                                                    │
                    驱动：注入 ──► 等稳定 ──► estimate ──► 画布写带 ──► 预览端口（单向）
                                                    │  ▲
                                     SCROLL_READY_MESSAGE │  │ ScrollController::{watch_rows, set_follow, undo, stop, cancel}
                                                    ▼  │
                    overlay 线程：watch_scroll_preview(端口) ⇒ 面板模型 ⇒ 渲染 ⇒ 点击命中区
                                                    │
                          停止 ──► 行带流式 PNG（既有 RowBandWriter）──► ArtifactWriter 端口
                                                    │
                                   剪贴板（超长图除外）+ 历史 save_publication
```

### 1.2 九条缝的逐条证据

| # | 缝 | 今天的形状（`file:line`） | 为什么它今天不生效 |
|---|---|---|---|
| 1 | **入口** | `crates/snapclip-capture/src/windows/hotkey.rs:18`（唯一 id）、`:57-68`（注册硬编码 F5）、`window_host.rs:61-67`（只有一个 `if`） | 没有第二个键的槽位；`WM_HOTKEY` 的其它 id 被 `Some(0)` 吞掉，连"未知键"都看不出 |
| 2 | **目标解析** | `crates/snapclip-capture/src/windows/overlay/hover.rs:405-455`（`update_hover` 只写 `hover_target`，且**只在 `CaptureState::Selecting`** 里工作） | 悬停结果只用于画预览与深度选择，没有任何"选区 → 窗口"的裁决函数；V2 `§28.2` 计划里的 `scroll/target.rs` **从未成为文件** |
| 3 | **几何** | `crates/snapclip-capture/src/windows/scroll_probe.rs:4304`（`visible_geometry`，探针私有） | 生产代码里没有任何地方用 DWM 可见边界；用 `GetClientRect`/`GetWindowRect` 建 plan 会撞画布不变量（`scroll/canvas.rs:1154`） |
| 4 | **交接** | `crates/snapclip-capture/src/session.rs:365`（`begin_scroll`，**零生产调用者**，只有 `:913`/`:940-949` 的测试） | 没有人在 `confirm` 与 `begin_scroll` 之间做选择：滚动会话今天只能由测试构造 |
| 5 | **驱动装配** | `crates/snapclip-capture/src/scroll/session.rs:664`（`ScrollRuntime`）、`:689`（`start(plan, make_source, actuator)`，工厂闭包在驱动线程内调用） | 唯一的构造点在 `windows/scroll_probe.rs` 的 `#[ignore]` 探针里。**这一缝里还藏着两个不存在的适配层**（详见 `§1.5`）：帧源的三角桥接（`WgcFrameBackend::open` → `WgcFrameSource::new`，今天只在 `scroll_probe.rs:4487-4488`）与执行器的端口实现（`scroll_actuator.rs` 里有 `choose`/`inject`/`Win32Injection`，但**没有 `impl ScrollActuator`**） |
| 6 | **覆盖层消费侧** | `crates/snapclip-capture/src/windows/overlay/render_submit.rs:29`（`watch_scroll_preview`，`#[allow(dead_code)]`，**零调用者**）；`:45-66` 的 `on_scroll_ready` 已经接在 `window_host.rs:53` 上 | 端口这一半已经通了（消息 id、排空、失效重绘都在），**缺的是"谁把端口交给面板"** |
| 7 | **面板交互** | `crates/snapclip-capture/src/scroll/panel.rs:114`（四个矩形）、`windows/win/d2d.rs:1448`（唯一读者，只画） | 没有命中函数、没有"矩形 → 命令"的类型；`scroll/` 内不能命名平台类型，所以这个映射必须是一个**纯几何 + 纯命令**的 API |
| 8 | **预览窗口通道** | `crates/snapclip-capture/src/scroll/preview.rs:448`（`refresh_window(canvas: &mut RecoveredImage, …)`）、`scroll/latency_probe.rs:33`（"`refresh_window` has no production caller today"） | 窗口由**消费者**选（V2 `§19.2` 固定高度窗 + `§19.5` 回到最新/手动拖动），而 `PreviewUpdate` 是单向的 ⇒ 需要一条消费者→生产者的请求通道 |
| 9 | **产物出口** | `apps/snapclip/src/adapters.rs:89`（`artifact: None`）、`apps/snapclip/src/clipboard.rs:27`（`copy_image` 只被历史界面调用）、`crates/snapclip-history/src/ingest.rs:224-230`（`save_publication` 只被 ingest 调用） | 普通截图今天**也没有**出口。滚动截图要么先补上共同出口，要么自己造一条（本文选前者，ADR-22） |

### 1.3 今天唯一能看到滚动截图的四条库级命令

均需**已解锁的桌面**与 `--ignored --nocapture --test-threads=1`：

| 命令 | 看什么 |
|---|---|
| `cargo test -p snapclip-capture --lib inject_matrix_probe -- --ignored --nocapture --test-threads=1` | 五类目标的注入矩阵（V2 `§24.6.2`） |
| `cargo test -p snapclip-capture --lib capture_probe -- --ignored --nocapture --test-threads=1` | 窗口级 WGC 的六臂捕获（`E-CAP-1`） |
| `cargo test -p snapclip-capture --lib cancel_latency_probe -- --ignored --nocapture --test-threads=1` | 真机 `Cancel latency`（44 ms / 0 ms） |
| `cargo test -p snapclip-capture --lib stop_latency_probe -- --ignored --nocapture --test-threads=1` | 真机 `Stop latency` 的**驱动侧端点**（V2 `§23.3.5`） |

### 1.4 三件被高估的"看起来已经完成"

| 看起来 | 实际 | 后果 |
|---|---|---|
| "面板画得出来 ⇒ UI 做完了" | 四个按钮**画出来但不可点**（V2 `:3710`、`:3801`、`:3844` 三处逐字写明"点击命中区属装配根 `P6`"） | 用户按不到"停止" |
| "`PreviewStream` 已实现 ⇒ 预览通了" | 生产者一半（驱动 publish）+ 消费者一半（`watch_scroll_preview`）都在，**中间没人接** | 面板永远不出现 |
| "`§34.4 UI 满足`" | V2 `:6691` 的判据同时写了"**有一处未接线**"（指 `refresh_window`），但没有覆盖"按钮不可点""端口无人交付"两处 ⇒ 这条缝比那句话更大 | 验收结论本身需要一条 `[!]`（本文 §12.2 的回填） |

### 1.5 实现侧的缺口清单（本轮只读侦察实测；每一行都必须落到 `§9` 的某个任务上）

`§1.2` 是按**用户可见的缝**切的，下表按**实现侧的零件**切。两表的并集才是本次接线的全部工作量；没有第三张表。

| # | 缺口 | 今天的形状（`file:line`） | 归属 |
|---|---|---|---|
| A1 | `ScrollRuntime` 零生产调用者 | `crates/snapclip-capture/src/scroll/session.rs:664`；构造者只有 `:1159`/`:1209`/`:1229`/`:1255`（全在 `#[cfg(test)] mod tests`）与 `windows/scroll_probe.rs:4797`/`:5044`（`windows/mod.rs:38-39` 把探针门在 `#[cfg(test)]` 里）；源文件自述 `session.rs:35-37` | **P7.05** |
| A2 | 帧源的三角桥接只活在探针里 | `WgcFrameBackend::open(device: std::sync::Arc<GraphicsDevice>, handle: isize) -> Result<Self, FrameError>`（`windows/scroll_source.rs:351`）+ `WgcFrameSource::new(backend: Box<dyn FrameBackend>, axis: Axis) -> Self`（`:236`，尺寸由 `FrameBackend::size`（`:213`）自报）；把两者接起来的代码只在 `windows/scroll_probe.rs:4487-4488`，且探针外面套了一层 `DeferredWgcSource`（`:4447`）把 `Result` 变成合法帧源 | **P7.05**（与 P7.03 的 `windows/win/frame.rs` 共用同一个几何来源） |
| A3 | `providers.rs` 的 `ScrollFrame` 与 `FrameBackend` 没有类型关系 | `pub struct ScrollFrame`（`windows/providers.rs:656`，字段 `delivered: Option<wgc::WgcFrame>` 带 `#[allow(dead_code)]`、`transfer`、`reads`、`read_bytes`），它**不实现** `FrameBackend`，也没有任何转成 `Observation` 的代码；`windows/scroll_source.rs:1227-1237` 的用例只钉住"这个文件不碰 D3D11 context"，没钉住它被谁用 | **P7.05 必须裁决去向**：要么让它成为 `FrameBackend` 的实现（那 `WgcFrameBackend` 就重复了），要么在那里的 doc 里写清它是显示器侧的单次回读包装、与滚动路径无关。**不允许两套帧路径并存而不写裁决** |
| A4 | `ScrollSourceRuntime` 零调用者（连测试都没有） | `windows/scroll_source.rs:437`/`:445`（`open(handle, content)`），`handle`/`content`/`frame`/`monitor`/`viewport` 五个 getter 只在 `:466-482` 定义 | **P7.05**：要么成为端口交付的载体，要么删除；留一个"看起来是入口、实际没人调"的类型正是 `§1.4` 那类错觉 |
| A5 | Win32 侧没有 `impl ScrollActuator` | `windows/scroll_actuator.rs` 有 `choose(&TargetProbe) -> Choice`（`:179`）、`InjectRequest`（`:207`）、`InjectionTarget`（`:229`）、`inject(&dyn InjectionTarget, &InjectRequest) -> InjectOutcome`（`:292`）、`Win32Injection`（`:332`），**没有端口实现**；端口定义在 `scroll/ports.rs:196`（`type Path` / `path()` / `switch(from)` / `inject(notches) -> InjectOutcome`）。真实实现只有测试/探针：`scroll/session.rs:1115`（`SilentActuator`）、`scroll/loop_control.rs:2009`（`LinkedActuator`）、`windows/scroll_probe.rs:4393`（`Win32ScrollActuator`） | **P7.05**，且**必须放在 `windows/` 侧**（端口实现要命名 `InjectPath`/`Aim`/`target`/`screen`，`scroll/` 的平台纯净门禁不允许这些名字出现在那边） |
| A6 | `ScrollCommand` 零定义 | 只在 `scroll/session.rs:1040`/`:1058` 的**测试断言字符串**里（`production.contains("Mutex<Option<ScrollCommand>>")`）；V2 `§27.2` 承诺的 `begin`/`step`/`stop`/`cancel`/`undo` 里只有 `stop`/`cancel`/`undo` 真实存在（`ScrollController` 的粘性位，`session.rs:459`/`:482`/`:502`） | **P7.08**（按 `set_follow` 的容量 1 形状新造，ADR-20） |
| A7 | `ScrollOutcome` / `ScrollConfig` 零定义 | 只出现在 `scroll/session.rs:190`/`:192` 的 doc 里 | **P7.12** |
| A8 | `refresh_window` 零生产调用者 | `scroll/preview.rs:448`；调用者只有 `scroll/latency_probe.rs:286`/`:291`（探针）与 `preview.rs:869`（测试）；源文件自述 `preview.rs:83-84`、`latency_probe.rs:33` | **P7.08** |
| A9 | `ScrollPanel::on_update` 的 `Bands` 分支是空实现 | `scroll/panel.rs:459`（理由在 `:456-458`：像素位置不属八问，消费方是 overlay） | **P7.08**（像素带由覆盖层贴，不改 `PanelActions`） |
| A10 | `ScrollSession::disposal()` 零消费者 | `scroll/session.rs:369`（`Option<Disposal<'_>>`，`Disposal` 在 `:193`） | **P7.09**（停止/取消之后由装配根读它决定产物去向） |
| A11 | 导出缝只服务普通截图路径 | `apps/snapclip/src/capture/artifact_writer.rs:102-111` 写死 `length = height`、`axis: Axis::Vertical`、`dpr: 1`，注释逐字 "Not the scroll path: there is no canvas that could have been capped"；`RowBandSink`/`RowBandWriter` 的实现（`apps/snapclip/src/capture/row_band_png.rs:83`/`:125`/`:158`/`:169`）与调用点（`artifact_writer.rs:114-126`）都只在普通截图上 | **P7.09 + P7.10**（ADR-21/22） |
| A12 | 面板命中区不存在 | 全仓无 `fn hit_test`/`fn command_at`（`scroll/panel.rs` 内 `layout.buttons[i]` 的引用全在 `:896-:937` 的测试里）；`scroll/panel.rs:270-273` 与 `:387`/`:392`/`:398` 的 doc 明说路由归覆盖层 | **P7.06** |

---

## 2. 目标与非目标

### 2.1 目标（G13–G20；与 V2 `§7` 的 G1–G12 同表，编号续）

| # | 目标 | 判据（可证伪） |
|---|---|---|
| **G13** | **一次按键可达**：任意窗口上按 F7 即可开始一次窗口级滚动截图，不需要先按 F5 | L3：真机上 F7 → 面板在 ≤ 500 ms 内可见（`SCROLL_READY_MESSAGE` 到达且 `scroll_panel.is_some()`） |
| **G14** | **与普通截图同构的承诺**：`Enter` = 停止并产出（可能 `Partial`）、`Esc` = 取消且不留文件 | L1：两条路径的 `StopReason` 不同且 `yields_partial()` 分别为真/假（`scroll/session.rs:56-80,88`） |
| **G15** | **面板可点、可拖、可回到最新**：四个按钮与视口框都接受鼠标 | L1：命中函数对四个按钮互不重叠、每个点恰好命中一个、面板外返回 `None`；L2：每个 `PanelAction` 恰好映射到一个控制器调用 |
| **G16** | **产物与普通截图同一出口**：产出一个真实的 PNG 文件，出现在历史列表里，能行预览 | L3：真产物进历史；`row_preview_png` 对它成功（`apps/snapclip/src/history/preview.rs:70`） |
| **G17** | **失败可解释**：13 个 `ScrollDiagnosticCode` 至少有一条到面板的路径，停止原因可回查 | L1：诊断码 → 面板文案穷举；L2：驱动发布一个码之后 `trouble_text` 非空 |
| **G18** | **有界内存**：会话期间峰值不随图像高度增长（沿用 `P4.07` 的结论） | L4：`E-MEM-1` 的 0.16% 极差在接了真实消费者之后仍然成立 |
| **G19** | **普通截图零回归**：F5 路径的行为、状态机、事件与今天逐字相同 | 全量 A 类（20 项）通过；`crates/snapclip-capture/src/session.rs:550` 的 `esc_from_every_active_state_returns_to_idle` 不动 |
| **G20** | **首次可达的测量**：`Preview 更新延迟` 与 `Stop latency` 的**产品端点**在接线后必须给出真机数字 | L3/L4：两条指标各有一次真机测量（V2 `§23.2` 的端点定义） |

### 2.2 非目标（N13–N20；与 V2 `§8` 的 N1–N12 同表，编号续）

| # | 非目标 | 理由 |
|---|---|---|
| **N13** | 不做"选区滚动"（在任意矩形上滚动，而不绑定某个窗口） | 捕获是窗口级的（V2 `§24.1`）；任意矩形的滚动需要屏幕级拼接与另一套遮挡语义，收益不明确 |
| **N14** | 不做滚动会话中的**标注** | 标注属于普通截图（`artifact.rs` 的注释路径）；滚动产物是长图，"在这一帧上画一笔"没有稳定参照 |
| **N15** | 不做滚动会话中的**缩放面板以外**的交互（旋转、裁剪后重排） | 与 V2 `§19.5` 的能力表一致 |
| **N16** | **超长图不写剪贴板** | `apps/snapclip/src/clipboard.rs:27` 的 `copy_image(width, height, rgba)` 需要**整块 RGBA**；1058×502649 是 2028 MiB，物理上不可行。裁决：超过阈值只进历史，面板与托盘给一句说明（见 ADR-23） |
| **N17** | 不新增线程 | V2 `§9.4`：滚动只新增 `snapclip-scroll-driver` 一个线程，已经存在 |
| **N18** | 不改 `CaptureSession` 的 6 状态穷举语义 | V2 `§20.6`；`session.rs:550` 的测试是保护项 |
| **N19** | 不引入第二个 `GraphicsDevice` 之外的设备共享机制 | `OQ-26` 的三条出口里选"驱动自建设备"（ADR-19） |
| **N20** | 不为 shell 打开 `scroll/` 的内部类型 | 装配根留在 capture crate 内（ADR-18）；shell 只看到 `ScrollOutcome`/`ScrollConfig`/`ScrollDiagnosticCode`（V2 `§27.1`）与既有的 `ArtifactWriter`/`CaptureEventSink` 端口 |

---

## 3. 端到端流程

### 3.1 F7 的时序（接线后的目标形状）

```
用户                 overlay 线程                      capture worker         scroll driver          export worker
 │                        │                                │                      │                      │
 ├─ F7 ─────────────────► │                                │                      │                      │
 │                  注册表命中 SCROLL_HOTKEY_ID            │                      │                      │
 │                  start_scroll_entry()                   │                      │                      │
 │                        ├─ 光标下的窗口（既有快照）⇒ ScrollTarget
 │                        ├─ DWM 可见边界 ⇒ cross/extent                       │
 │                        ├─ worker.start(StartRequest) ──►│ 冻结显示器           │
 │                        │◄── FRAME_READY_MESSAGE ────────┤                      │
 │                        ├─ 画选区/悬停（既有 UI）        │                      │
 │                        ├─ ScrollPlan{target, crop, axis, viewport}
 │                        ├─ session.begin_scroll() ⇒ ScrollHandoff（CaptureSession 结束）
 │                        ├─ ScrollRuntime::start(plan, ‖→ 新建 GraphicsDevice + WgcFrameSource,
 │                        │                               Win32ScrollActuator) ──►│（线程启动）
 │                        ├─ watch_scroll_preview(port) ⇒ 面板模型               │
 │                        │                                │   每步：注入→等稳定→estimate→写带
 │                        │◄── SCROLL_READY_MESSAGE ────────┴──────────────────────┤
 │                        ├─ on_scroll_ready() ⇒ 折叠进面板 ⇒ 重绘                │
 ├─ 单击 [停止] ─────────►│ controller.stop() ────────────►│ 收尾 ──► Disposal::Export(&canvas)
 │                        │                                │     行带流式 PNG ────►│ 编码+落盘
 │                        │◄── EXPORT_READY_MESSAGE ────────┴──────────────────────┤
 │                        ├─ sink.on_completed(artifact) ⇒ 剪贴板（短图）+ 历史
 ├─ Esc ────────────────►│ controller.cancel() ⇒ 无文件、无历史条目
```

### 3.2 谁在哪个线程做什么（本次接线的增量）

| 线程 | 已有的职责 | 本次新增的职责 |
|---|---|---|
| `snapclip-capture-overlay` | 消息泵、D2D 立即上下文、选区/标注、`confirm` 的唯一回读 | 第二个热键的分派、目标解析、`begin_scroll` 的调用、`ScrollRuntime` 的**创建与持有**、面板命中区、`ScrollController` 的调用、导出的 `on_export_ready` |
| `snapclip-capture-worker` | 持有 `GraphicsDevice`、冻结显示器 | **不变**（滚动路径不使用它的冻结帧作原点，见 ADR-19） |
| `snapclip-scroll-driver` | 不存在，本次第一次有生产用户 | 自建 `GraphicsDevice` + `WgcFrameSource`（工厂闭包内）、注入、等稳定、估计、写画布、发布预览、消费窗口请求 |
| `snapclip-export-worker` | 编码+落盘普通截图 | 新增一种**行带**导出任务（见 ADR-21） |
| shell 主线程 | GPUI 界面、历史列表 | **不变**（只多了一条"滚动产物完成"的事件） |

### 3.3 一次滚动截图经过的五个所有权阶段（沿用 V2 `§9.3`，本次只需把第 1 段讲清楚）

| 段 | 所有者 | 本次接线的裁决 |
|---|---|---|
| ① 捕获段（GPU） | **滚动驱动自己的 `GraphicsDevice`** | 驱动在工厂闭包内 `create()`，立即上下文的所有者是驱动线程（V2 `§21.3.1` 的"第一个使用者认领"）。**不共用 overlay 的设备**（ADR-19） |
| ② 证据段（`Observation`） | `ScrollLoop`，每步替换 | 不变 |
| ③ 画布段（`BandStore`） | `ScrollSession`（驱动线程可变借用） | 不变；`refresh_window` 的 `&mut` 正是"画布归驱动"的直接后果 |
| ④ 预览段（`PreviewStream`） | `Arc` 共享：驱动写、overlay 读 | 新增**反向**的窗口请求（容量 1 覆盖式，ADR-20） |
| ⑤ 产物段（行带 PNG） | export worker | 新增行带任务；**产物先落盘再通知**，与普通截图同形 |

### 3.4 停止与取消：两条路径，两个承诺

| 动作 | 键/鼠标 | `ScrollController` | `StopReason` | `yields_partial()` | 产物 |
|---|---|---|---|---|---|
| 停止 | `Enter` / `[停止]` | `stop()` | `UserStopped` | `true` | 有文件（可能 `Partial`） |
| 取消 | `Esc` / `[取消]` | `cancel()` | `UserCancelled` | `false` | **无文件、无历史条目** |
| 目标消失 | 窗口关闭/最小化 | 驱动侧 `EndReason::TargetLost` | `TargetLost` | `true` | 有文件（`Partial`） |
| 内部错误 | 设备丢失等 | 驱动侧 | `InternalError` | `false` | 无文件 |
| 撤销 | `[撤销]` | `undo()` | — | — | 只回退一步（不更新 `ĝ`，V2 `§19.6`） |

---

## 4. 九条缝的接线裁决

### 4.1 缝 1：入口（F7）

**今天的形状**：`hotkey.rs:18/21/22` 三个常量、`:57-68`/`:71-75` 两个硬编码函数、`:38-51` 两条写死 "F5" 的错误文案；`window_host.rs:61-67` 一个 `if`。

**接线形状**：把"一个热键"提升为**一张键表**，表项 = `Hotkey { id, modifiers, virtual_key, label }`：

```rust
// crates/snapclip-capture/src/windows/hotkey.rs（目标形状，非本次实现）
pub const CAPTURE_HOTKEY: Hotkey = Hotkey { id: 0x5343, modifiers: MOD_NOREPEAT, virtual_key: VK_F5,  label: "F5" };
pub const SCROLL_HOTKEY:  Hotkey = Hotkey { id: 0x5344, modifiers: MOD_NOREPEAT, virtual_key: VK_F7,  label: "F7" };
pub const HOTKEYS: [Hotkey; 2] = [CAPTURE_HOTKEY, SCROLL_HOTKEY];

pub fn register(window: HWND) -> Result<(), HotkeyError>;   // 逐项注册，失败项带上自己的 label
pub fn unregister(window: HWND);                             // 逐项注销
pub fn from_id(id: i32) -> Option<Hotkey>;                   // WM_HOTKEY 的分派依据
```

**落点**：`crates/snapclip-capture/src/windows/hotkey.rs`（改）+ `windows/overlay/window_host.rs`（`:61` 的 `if` 改成 `match hotkey::from_id(wparam as i32)`，未知 id 必须**仍然**返回 `Some(0)`——今天的行为，避免把热键消息漏给 `DefWindowProcW`）。

**约束**：① `CAPTURE_HOTKEY_ID = 0x5343` 必须不变（它是既有的钉住值，`hotkey.rs:80` 的断言）；② 新 id 不能与 `0x5343` 相同，也不能被别处使用；③ 错误文案必须带**哪一个键**（"F7 is already registered …"）——今天的写死文案是这条缝的实质；④ 注册仍在 overlay 线程（`window_host.rs:573`），因为 `WM_HOTKEY` 只能投递给注册线程的队列。

### 4.2 缝 2：目标解析（选区 → 窗口）

**今天的形状**：`hover.rs:405-455` 只把 `hover_target` 存下来给绘制与深度选择用，且**只在 `CaptureState::Selecting`**。

**接线形状**：新增一个**纯函数**做裁决（V2 `§28.2` 计划里的 `scroll/target.rs` 终于要有内容，但它必须是 platform-free 的）：

```rust
// crates/snapclip-capture/src/scroll/target.rs（目标形状）
pub struct ScrollTarget { window: u64, bounds: Rect, crop: Rect }   // u64 = 窗口身份（非 HWND 类型）
pub enum TargetChoice { Accepted(ScrollTarget), NoWindow, TooSmall { .. }, Ambiguous { .. } }
pub fn choose_target(selection: Rect, candidates: &[WindowCandidate], min_extent: u32) -> TargetChoice;
```

`WindowCandidate` 由 `windows/overlay/hover.rs` 从既有快照（`snapshot.hit_test` / `top_level_provider`）填出来 —— **平台类型不过河**：`scroll/target.rs` 只认识 `Rect` 与一个不透明的 `u64` 身份，`unsafe`/`HWND` 的转换留在 `windows/` 一侧。

**裁决规则（写进测试）**：① 选区与候选窗口的交集面积最大者胜；② 交集面积相同 ⇒ 取 z-order 更靠前者；③ 交集为零 ⇒ `NoWindow`；④ 候选窗口的主轴范围 < 视口下限（V2 `§16.2.1` 推出的 `≥ 448 px`） ⇒ `TooSmall`；⑤ 两个候选的交集面积差 < 5% ⇒ `Ambiguous`（拒绝猜测，让用户改用鼠标点选窗口）。

### 4.3 缝 3：几何（DWM 可见边界进入生产）

**今天的形状**：`visible_geometry` 只存在于 `windows/scroll_probe.rs:4304`。

**接线形状**：把该助手提升为生产 API（`crates/snapclip-capture/src/windows/win/frame.rs` 新建，或并入 `windows/win/window.rs` 的既有窗口查询家族），签名 `pub(crate) fn visible_geometry(window: isize) -> Option<(u32, u32)>`（可再加一个返回 `Rect` 的变体）。

**理由（不是风格）**：`docs/31 P3.10` 实测三次才定下来——`GetClientRect` 1184×892、`GetWindowRect` 1200×900、`DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` 1188×894，而 WGC 交付的正是 **1188×894**。用另外两个矩形建 plan 会让驱动在第一帧撞 `scroll/canvas.rs:1154` 的画布不变量（`the frame's cross axis is not the canvas's`）。

**约束**：`windows` crate 的 `Win32_Graphics_Dwm` feature 已启用（`crates/snapclip-capture/Cargo.toml:35`）；探针里那份**保留**（它是 L3 证据的来源），新生产函数与它互不引用，落一条 L1 断言"两者对同一矩形给出同一对数字"。

### 4.4 缝 4：交接（`begin_scroll` 的调用者）

**今天的形状**：`session.rs:365` 零生产调用者。`ScrollHandoff` 携带 `{ frame: CapturedFrame, selection: Rect, dpi: u32 }`（`:62`）。

**接线形状**（ADR-19 的落地）：`ScrollHandoff` 的载荷改为**几何与身份**，不再携带 GPU 帧：

```rust
// crates/snapclip-capture/src/session.rs（目标形状）
pub struct ScrollHandoff { target: ScrollTarget, crop: Rect, dpi: u32 }
// 仍由 CaptureSession::begin_scroll() 产出：它结束普通截图会话（frame.take()），
// 并把"用户选在哪儿、那一刻的 DPI"交给滚动会话；原点像素由驱动的第一帧提供。
```

调用点：`crates/snapclip-capture/src/windows/overlay/session.rs` 里新增 `fn begin_scroll_session(&mut self)`（**与 `confirm` 并列**，不走 `confirm`——后者会构造 `ExportJob`），流程 = `session.begin_scroll()` → `ScrollPlan` 构造 → `ScrollRuntime::start(...)` → `watch_scroll_preview(...)`。

**为什么改类型而不是留 `frame`**：`AGENTS.md` 禁止死代码。窗口级 WGC 的**第一帧就是原点**（同一窗口、同一遮挡语义），而冻结帧是**显示器**的（含遮挡）⇒ 用它当原点会在"窗口被遮挡"时留下一道接缝。改类型比留一个永远不读的字段诚实（V2 是 pre-release，`AGENTS.md` 明确不考虑向后兼容）。

### 4.5 缝 5：驱动装配

**今天的形状**：唯一构造点在 `windows/scroll_probe.rs`（`ScrollRuntime::start` 在 `:4797`），而且探针里那一段**不能直接搬**——它由三块只存在于探针里的代码拼成：

| 块 | 探针里的形状 | 生产里缺什么 |
|---|---|---|
| 帧源的**延迟打开** | `struct DeferredWgcSource { opened: Result<WgcFrameSource, FrameError>, viewport: Rect, frames: Arc<AtomicU32> }`（`windows/scroll_probe.rs:4447`）+ `fn open_wgc_source(handle: isize, axis: Axis, viewport: Rect, state: Arc<AtomicU32>, frames: Arc<AtomicU32>) -> DeferredWgcSource`（`:4475`，急切建 `d3d11::GraphicsDevice::create()` → `WgcFrameBackend::open` → `WgcFrameSource::new`） | `ScrollRuntime::start` 的工厂闭包返回 `F: FrameSource`，**不是 `Result`**（`scroll/session.rs:689`）；`WgcFrameBackend::open` 会失败，而 `FrameSource::next` 的失败通道是 `Poll::Ended`/`FrameError`（`scroll/ports.rs:94-99`）⇒ 失败必须**搬进帧源自己**。这个"打开失败也仍然是一个合法帧源"的包装是生产代码，必须落在 `windows/scroll_source.rs`（探针版本不能成为生产依赖） |
| 帧源的**三角桥接** | `WgcFrameBackend::open(Arc::new(device), handle)` + `WgcFrameSource::new(backend, axis)` 两行（`:4487-4488`） | `WgcFrameSource::new` 收 `Box<dyn FrameBackend>` 与 `Axis`、返回 `Self`（`windows/scroll_source.rs:236`），尺寸由后端自己报（`FrameBackend::size` `:213`）⇒ 桥接本身很短，但**它必须与 `P7.03` 的 DWM 可见几何对上**，否则第一帧就撞画布不变量（`scroll/canvas.rs:1154`，`docs/31 P3.10` 的实测） |
| 执行器的**端口实现** | `struct Win32ScrollActuator`（`windows/scroll_probe.rs:4393`），字段 `injected_at`（注入后盖时间戳） | `windows/scroll_actuator.rs` 里**没有 `impl ScrollActuator`**（`§1.5` A5）。生产版要自己持有 `TargetProbe` 的四项取值（`target_is_elevated`/`self_is_elevated`/`target_is_foreground`/`routing`，`windows/scroll_actuator.rs:131`）、在会话开始算一次 `Choice`（`choose` `:179`），并让 `path()`/`switch(from)` 回答 `InjectPath`（端口要求 `Path: Copy + PartialEq + Debug`，`scroll/ports.rs:199`） |

**接线形状**：

```rust
// crates/snapclip-capture/src/windows/overlay/session.rs（新增，与 confirm 并列）
let plan = ScrollPlan::new(axis, cross_len, viewport_extent, budget)   // 见下方约束 ①
    .with_wheel(lines_per_notch, line_height_px);                      // 取自 SPI_GETWHEELSCROLLLINES × 行高
let runtime = ScrollRuntime::start(
    plan,
    move || OpeningFrameSource::open(window, axis),   // 工厂闭包：在 snapclip-scroll-driver 线程内建
    WindowWheelActuator::new(window, screen, probe),  // 执行器是值（无设备、无公寓）
);
```

其中 `OpeningFrameSource`（`windows/scroll_source.rs` 新增）与 `WindowWheelActuator`（`windows/scroll_actuator.rs` 新增）就是上表后两行的生产版；**两者的名字与位置都写进 `P7.05` 的 RED 断言**，避免又变成"看起来接好了"。

**约束**：① `ScrollPlan::new`（`scroll/session.rs:597`）**不做任何校验**（`§1.5` 未列出但同源：`with_wheel` 只 `.max(1)` 钳位）⇒ 装配根自己是唯一能把 `viewport_extent ≥ 448 px`（V2 `§16.2.1` 的 `MIN_TILES = 4` 推论）、`cross_len` 与 DWM 几何一致、`budget` 来自真实视口尺寸三者对上的人；② 工厂闭包**必须**在驱动线程内建设备（V2 `§21.3.1` 的"第一个使用者认领"+ `scroll/session.rs:679` 的注释）；③ **任何线程都不声明公寓**（V2 `§21.5`），WGC 激活依赖 combase 的隐式 MTA；④ 执行器可以跨线程递交（`SendInput`/`PostMessageW` + 整数），帧源**不能**（`ID3D11Device` 与 `HWND` 都不是 `Send`）——这正是 `start` 收工厂而不是收帧源的原因（`docs/31 P3.10` 的编译器证据：`error[E0277]: `*mut c_void` cannot be sent between threads safely`）；⑤ `providers.rs` 的 `ScrollFrame` 与 `ScrollSourceRuntime` 必须在本次裁决去向（`§1.5` A3/A4），**不允许出现第三条同样"看起来像入口"的帧路径**。

### 4.6 缝 6：覆盖层消费侧

**今天的形状**：`render_submit.rs:29` 的 `watch_scroll_preview` 有 `#[allow(dead_code)]`，`on_scroll_ready` 已经接好。

**接线形状**：在 4.5 的同一处调用 `watch_scroll_preview(preview.clone(), cross_len, extent)`，并撤掉 `#[allow(dead_code)]`。参数的两个 u64 来自 plan（不是从更新里推断——`render_submit.rs:18-27` 的 doc 已经把理由写清楚：视口 extent 只在交接时说一次）。

**约束**：① 交接必须发生在 `show_overlay` **之前**或紧接着，避免面板已画而端口未接（否则第一次 `SCROLL_READY_MESSAGE` 落在 `scroll_preview == None` 上，`on_scroll_ready` 直接返回 `false`）；② `graphics_released` 的语义不变（滚动会话的产物导出**不**经过它：它是普通截图"冻结纹理交给导出"的标志，`scroll/session.rs` 的画布是 CPU 字节）。

### 4.7 缝 7：面板命中区

**今天的形状**：`scroll/panel.rs:114` 四个矩形 + `d2d.rs:1448` 只画。

**接线形状**：在 `scroll/panel.rs` 内新增**纯几何 + 纯命令**的 API（`scroll/` 不能命名平台类型，门禁扫文本含注释）：

```rust
// crates/snapclip-capture/src/scroll/panel.rs（目标形状）
pub enum PanelAction { ReturnToLatest, Undo, Stop, Cancel, DragViewport { .. } }
pub enum PanelHit { Button(PanelAction), Viewport, Outside }
impl PanelLayout {
    pub fn hit_test(&self, point: Point) -> PanelHit;   // point 是 DIP
    pub fn actions(&self, state: &ViewState) -> [Option<PanelAction>; 4];  // 与绘制同源：画灰的不可点
}
```

`windows/overlay/input.rs` 的 `on_left_down`（`:93`）在**滚动会话活跃**时先问 `hit_test`：命中按钮 ⇒ 交给映射表（`PanelAction` → `ScrollController::*`，见 4.8 的测试载体）；命中视图框 ⇒ 进入手动拖动（`set_follow(false)`）；`Outside` ⇒ 落在选区语义上（今天的行为）。

**约束**：① `actions()` 必须与 `state.appearance()`/`doing()` 同源——"画成灰的按钮不可点"要有 L1 断言（V2 `§19.4.1` 的 `UNADOPTED_RGB` 就是这条视觉约定）；② 坐标是 DIP，DIP→px 的换算仍在绘制处（`d2d.rs` 的 `scale = dpi / 96`），**不搬进 `scroll/`**；③ `d2d/tests.rs:1700` 的 `the_preview_panel_does_not_cover_the_toolbar_hit_regions` 里的"toolbar hit regions"其实是选区握把，**不要**把它当成已有的按钮命中测试。

### 4.8 缝 8：预览窗口的请求通道

**今天的形状**：`refresh_window(canvas: &mut RecoveredImage, scale, first_row, rows)` 只有探针调用；`PreviewUpdate` 单向。

**接线形状**（ADR-20）：在 `ScrollController` 上加一条**容量 1、覆盖式**的请求。**注意：这个枚举今天不存在**——`ScrollCommand` 在全仓只出现在 `crates/snapclip-capture/src/scroll/session.rs:1040`/`:1058` 的**测试断言字符串**里（`production.contains("Mutex<Option<ScrollCommand>>")`），V2 `§27.2` 承诺的 `ScrollCommand{begin, step, stop, cancel, undo}` 里真正落地过的只有 `ScrollController` 的三个粘性位（`stop()` `:459`、`cancel()` `:482`、`undo()` `:502`）与容量 1 的 `set_follow`（`:508`，`follow: Mutex<Option<bool>>`）。所以本次不是"给已有枚举加一个变体"，而是**按 `set_follow` 的既有形状新造一个请求通道**：

```rust
// crates/snapclip-capture/src/scroll/session.rs（目标形状，全部是新代码）
pub(crate) struct WindowRequest { pub first_row: u64, pub rows: u64 }   // rows = 0 表示"不需要窗口"
// ScrollController 上新增：window: Mutex<Option<WindowRequest>>
pub(crate) fn request_window(&self, request: WindowRequest);            // 覆盖式写入
pub(crate) fn take_window_request(&self) -> Option<WindowRequest>;      // 驱动侧取走
```

`set_follow` 的先例给了两条现成的判据：**`Mutex<Option<T>>` 就是"容量 1、覆盖式"**，而 `stop`/`cancel`/`undo`/`shutdown` 是"粘性位、不可丢失"（`session.rs:409-415` 的 doc 表逐字写了这三分法）。请求属于前者，所以它**不**参与 `compare_exchange` 的那套。

驱动在主循环的**可中断点**读一次该请求（与读取消同一个节奏），命中时对**自己的**画布调用 `refresh_window(...)`，把结果经端口发布（`PreviewUpdate` 不新增变体——窗口内容仍然是一批条带，V2 `§19.3` 的"一次唤醒看全"继续成立）。

**约束**：① 请求必须**覆盖式**：拖动时每帧都发一个新窗口，排队只会让预览落后于手指；② `refresh_window` **不得**调 `relieve`（`P5.02` 的裁决：`insert` 不换出、`relieve` 每步一次）；③ 消费者**必须**能表达"不需要窗口"（跟随模式 ⇒ 不发请求，或发 `rows = 0`），否则驱动会为不可见的窗口做盒式滤波。

### 4.9 缝 9：产物出口（停止 → 文件 → 剪贴板/历史）

**今天的形状**：`Disposal::Export(&RecoveredImage)`（`scroll/session.rs:369`）之后没有任何消费者；普通截图的产物也只到 `adapters.rs:89` 就断了。

**接线形状**：三段，顺序固定（**先落盘，再通知**，与普通截图同形）：

1. **行带导出**（capture crate 内，驱动→导出线程）：`ImageMeta { width: cross, height: primary_len, length: primary_len, axis, dpr }` 交给既有的 `scroll/export.rs` 端口（`RowBandSink`/`RowBandWriter`，`:166-189`），由 shell 侧已有的 `PngRowBandSink`（`apps/snapclip/src/capture/row_band_png.rs:83`）实现。逐带遍历用 `RecoveredImage` 的只读行访问（**不**把画布整体物化——那是 `F7` 的禁令）。
2. **落盘**：经既有 `ArtifactWriter` 端口（`crates/snapclip-capture/src/ports.rs`，实现 = `apps/snapclip/src/capture/artifact_writer.rs:53`）。但该端口的签名只接受 `SelectionPixels`（整块 BGRA）⇒ **新增一个行带口子**（ADR-21）。
3. **出口**：产物落到 `CaptureArtifactStore` 后，照普通截图的路径发布完成事件；剪贴板对**短图**复制、**长图**（N16）跳过。

**约束**：① 产物的高度必须在**第一行写出前**已知（`RowBandWriter::begin(meta)` 的契约，`apps/snapclip/src/capture/row_band_png.rs:129-136` 会做 `u32::try_from` 拒绝超限）——滚动会话停止时 `primary_len` 是已知的，但 `Partial` 时要按**实际行数**而不是承诺高度建 meta（否则 PNG 头里的高度与写入行数不符，`finish` 会报 `ExportError::Sink`）；② 取消路径**不落盘**（与普通截图一致：`export_worker.rs:179-209` 的 `is_stale` 分支会 `delete_quietly`）；③ 超长产物的 `copy_image` 失败必须**不**影响产物的有效性（N16）。

---

## 5. 线程、公寓与 D3D11 所有权（`OQ-26` 的裁决）

### 5.1 裁决

**滚动驱动自建 `GraphicsDevice`，与 overlay 的设备不共用**（`OQ-26` 出口 ①）。

理由：① R-6 的不变式（V2 `§21.3.1`）是"每个 `ID3D11DeviceContext` 恰好一个使用者线程，由第一个使用者认领"；② `windows/scroll_source.rs:372-376` 的回读跑在 `snapclip-scroll-driver` 上，而 overlay 线程已经认领了它自己的设备的上下文 ⇒ 共用会让守卫在**第一个滚动步** panic（`crates/snapclip-capture/src/windows/win/d3d11.rs:129`）；③ `CreateDeferredContext` 已知覆盖不了 `Map`/`Unmap`（V2 `§21.3.1`），所以出口 ③ 不成立；④ 出口 ②（把回读搬回 owner 线程）会把每步回读塞进 overlay 线程，与 `§21.2` 的禁令（覆盖层线程禁止"阻塞等待捕获/回读"）直接冲突。

代价（必须写进文档，不许装作没有）：第二份 D3D11 设备与它的小规模资源占用；驱动线程退出时设备随之释放。

### 5.2 公寓

**本 crate 任何线程都不声明公寓**（V2 `§21.5`）：驱动线程的第一次 WGC 激活建立**隐式 MTA**；不得调用 `RoInitialize`/`CoInitializeEx`。钉住用例是既有的 `crates/snapclip-capture/src/windows/win/wgc.rs::winrt_activation_keeps_the_implicit_apartment`（父+子进程两段），本次**不新增**形状，只把驱动线程纳入同一纪律（L3 上跑一次滚动会话即为证据）。

### 5.3 跨线程边界清单（本次新增的每一条都要有测试）

| 边界 | 传递的东西 | 方向 | 容量/语义 | 测试载体 |
|---|---|---|---|---|
| F7 热键 → overlay | `WM_HOTKEY` | 系统 → overlay | 队列 | L3（真按键） |
| overlay → 驱动 | `ScrollPlan`（值） | 一次 | 不可变 | L1（构造后不可改） |
| overlay → 驱动 | `WindowRequest`（`first_row`/`rows`） | 覆盖式 | 容量 1 | L2（只保留最新） |
| 驱动 → overlay | `SCROLL_READY_MESSAGE` | 唤醒 | 可合并 | 既有 `overlay/tests.rs:57-77` + L3 |
| 驱动 → overlay | `PreviewUpdate`（端口内） | 有界、可丢 | 既有 | L2（排空语义） |
| overlay → 驱动 | `stop`/`cancel`/`undo`/`set_follow` | 粘性位/覆盖 | 既有 | L1（`P3.10` 的发布顺序用例） |
| 驱动 → 导出线程 | 行带导出任务 | 一次 | 覆盖式邮箱 | L2（generation 过期丢弃） |
| 导出线程 → overlay | `EXPORT_READY_MESSAGE` | 唤醒 | 既有 | L2/L3 |

---

## 6. ADR（续 V2 `§31` 的 ADR-1…ADR-17）

### ADR-18：装配根留在 `snapclip-capture` 内，不为 shell 打开 `scroll/` 内部类型

**背景**：`scroll::{session,loop_control,preview,panel,ports}` 全是 `pub(crate)`；shell 只能看到 `scroll::export`、`Axis`、`ScrollDiagnosticCode`（`crates/snapclip-capture/src/scroll/mod.rs:56,64,89`）。

**决策**：滚动会话的生命周期由 **overlay 线程**（capture crate 内）持有与驱动；对 shell 只暴露 V2 `§27.1` 的公开边界（`ScrollOutcome`/`ScrollConfig`/`ScrollDiagnosticCode`）与**既有端口**（`ArtifactWriter`、`CaptureEventSink`、`ClipboardWriter`）。

**被否决的备选**：把 `ScrollController` 开放给 shell，由 GPUI 侧当装配根。否决理由：① 滚动需要 overlay 的 HWND 与消息泵（`WM_MOUSEWHEEL` 的 `z_held` 分支、`WM_HOTKEY`、命中区都在 overlay 线程）；② 会让 shell 依赖 `scroll/` 的内部类型，等于把 V2 `§28.4` 的分层反过来；③ `docs/30` `§21.2` 明确"GPUI（shell 主线程）不在滚动会话里"。

### ADR-19：滚动会话的原点像素来自驱动自己的第一帧，`ScrollHandoff` 只携带几何与身份

**背景**：`ScrollHandoff` 今天携带 `CapturedFrame`（显示器冻结帧）。窗口级 WGC 是滚动路径的捕获方式（`OQ-26` 的裁决），而冻结帧属于**另一个设备**（`snapclip-capture-worker` 的），驱动读它需要一个跨设备的回读，正是 R-6 禁止的形态。

**决策**：`ScrollHandoff { target, crop, dpi }`；驱动的第一帧就是原点（同一窗口、同一遮挡语义）。

**代价与理由**：如果保留冻结帧作原点，窗口被遮挡时"原点带"含遮挡而后续带不含 ⇒ 产物里有一条**只在被遮挡时出现**的接缝。改类型比留一个不读的字段诚实（`AGENTS.md` 禁死代码；pre-release 不考虑兼容）。

**与 V2 的关系**：`§20.6` 的"冻结帧是会话的原点（§17.2）"这条**意图**保留（原点仍是用户在上面画选区的那个窗口内容），改变的是**来源**：从"显示器的冻结帧"变成"该窗口的第一帧"。回填见 §12.2。

### ADR-20：窗口请求走 `ScrollController` 的容量 1 覆盖式命令，端口保持单向

**背景**：`refresh_window` 需要 `&mut RecoveredImage`，画布归驱动 ⇒ 消费者不能直接调。

**决策**：在 `ScrollController` 上新增一条**容量 1、覆盖式**的窗口请求（`Mutex<Option<WindowRequest>>` + `request_window`/`take_window_request`，与既有 `set_follow` 同形；**不是**给某个已有枚举加变体——`ScrollCommand` 今天零定义，见 `§4.8`），驱动在每个可中断点读一次；`PreviewUpdate` **不新增变体**。

**被否决的备选**：① 让 `PreviewStream` 双向（`preview` 模块的端口会同时是两个方向的队列，`P3.04` 才刚把"一个端口一个方向"作为设计讲清楚）；② 给覆盖层一个"画布只读句柄"（V2 `§19.3` 约束 3 已断定不可行：`&mut` 借用 + 两个所有者）；③ 让覆盖层自己重算缩略（它没有画布）。

### ADR-21：行带产物新增一个 `ArtifactWriter` 口子，而不是把长图物化成 `SelectionPixels`

**背景**：`ArtifactWriter::write(&self, session_id, prepared: &SelectionPixels, dpi, monitor)` 要求整块 BGRA。1058×502649 是 2028 MiB。

**决策**：在 `crates/snapclip-capture/src/ports.rs` 的 `ArtifactWriter` 旁新增行带口子（形如 `fn write_rows(&self, session_id: &str, meta: &ImageMeta, rows: &mut dyn RowBandSource, dpi: u32) -> CaptureResult<CaptureArtifact>`），实现仍在 shell 侧（复用 `PngRowBandSink`）。

**被否决的备选**：① 让滚动会话构造一个假的 `SelectionPixels`（会先物化整图，直接违反 F7）；② 在 capture crate 里编码 PNG（会把 `png` 放进 capture 的依赖图，`apps/snapclip/src/capture/mod.rs:17` 的模块注释明确说编码器留在 shell 是为了这个）；③ 让驱动把产物写到磁盘（`§21.2` 禁止驱动线程做磁盘 I/O）。

### ADR-22：滚动产物与普通截图共用出口，并先修普通截图那条断链

**背景**：`apps/snapclip/src/adapters.rs:89` 的 `on_completed` 发布 `artifact: None`，注释 `:93-99` 把原因写成"等 T6 把装配根搬到 shell，那时由 writer 发布带 `ArtifactRef` 的完成事件"。

**决策**：本次把那条断链接上（写者发布真实 `ArtifactRef`），滚动与普通截图共用它。

**理由**：滚动截图如果自己造一条出口，就会有两套"产物已完成"的语义；而断链的存在意味着**普通截图今天也没有出口**——这是接线任务绕不开的共同前置（本文 `P7.10`），不是"顺手多做的重构"。

### ADR-23：超长产物不写剪贴板（`N16` 的落地）

**背景**：`apps/snapclip/src/clipboard.rs:27` 的 `copy_image(width, height, rgba)` 需要整块 RGBA；`crates/snapclip-history/src/image.rs:19` 的 `MAX_DECODE_PIXELS = 24_000_000` 已经说明这个仓库对"整块解码"有硬上限。

**决策**：产物像素数 > 阈值（建议取 `MAX_DECODE_PIXELS` 同一个数，**可注入**、**留一条测试**）时跳过剪贴板，只在历史里出现，并给用户一句说明。

**被否决的备选**：① 复制一个缩略图（把"我复制的不是我看到的东西"变成默认行为，不可接受）；② 放宽容忍度去尝试整块解码（会 OOM，`N16` 的理由）；③ 悄悄不复制（静默失败正是 `docs/31 P6.05` 建立门禁要挡的形态）。

### ADR-24：`Stop latency` 的产品端点以"导出任务已提交"为准，并保留驱动侧端点作为对照

**背景**：V2 `§23.2` 把产品端点定义为"停止 → 导出任务已提交给 export-worker"，而 `P3.10` 的真机探针只能量到"驱动线程结束、会话交回"（`§23.3.5` 已登记这个区分）。

**决策**：接线后两条都要量：产品端点（`P7.13` 的真机演练，`Instant` 差值到 `export_worker.submit` 返回）与驱动侧端点（既有探针，保持可比）。两个数字都写进 V2 `§23.3`，并注明端点。

### ADR-25：`scroll/target.rs` 以"平台无关的窗口身份"为输入

**背景**：V2 `§28.2` 的计划里有 `target.rs`，实际从未存在；而 `scroll/` 的门禁禁止出现 `crate::windows`/`winapi`/`windows_sys`/`windows::`（`tools/check-dependency-direction.ps1:109`，**含注释**）。

**决策**：`ScrollTarget { window: u64, bounds: Rect, crop: Rect }` —— 身份是**不透明整数**，`HWND` 的转换只发生在 `windows/` 一侧。命中裁决（§4.2 的五条规则）是纯函数，可 L1 穷举。

---

## 7. 新增测试矩阵（V2 `§30` 的续行，编号从 79 开始）

| # | 场景 | 任务 | 分类 | 层级 |
|---|---|---|---|---|
| 79 | F7 注册为第二个热键且与 F5 互不干扰 | `P7.01` | B | L1 |
| 80 | 两个热键都被别的应用占用时的错误文案各带自己的键名 | `P7.01` | D | L1 |
| 81 | 未知 `WM_HOTKEY` id 被吞掉且不让 `DefWindowProcW` 处理 | `P7.01` | B | L1 |
| 82 | 选区落在单一窗口内 ⇒ 选中它 | `P7.02` | B | L1 |
| 83 | 选区跨越两窗口 ⇒ 交集大者胜；面积差 < 5% ⇒ `Ambiguous` | `P7.02` | D | L1 |
| 84 | 选区与所有候选无交集 ⇒ `NoWindow` | `P7.02` | D | L1 |
| 85 | 候选主轴 < 448 px ⇒ `TooSmall`（`§16.2.1` 的下限） | `P7.02` | D | L1 |
| 86 | DWM 可见边界 == WGC 交付尺寸（真窗口） | `P7.03` | A | L3 |
| 87 | 探针里的 `visible_geometry` 与生产函数对同一窗口给出同一对数字 | `P7.03` | B | L1 |
| 88 | `begin_scroll` 之后 `session.frame()` 为 `None`（所有权交接） | `P7.04` | B | L1 |
| 89 | 三种非法交接状态都返回 `InvalidState` | `P7.04` | D | L1 |
| 90 | `ScrollRuntime::start` 在驱动线程内调用工厂（设备归属） | `P7.05` | B | L2 |
| 91 | 打不开的窗口仍然是一个合法帧源（`next()` 给 `Ended`/`Err`，**不** panic、**不**永远 `Idle`） | `P7.05` | B | L2 |
| 92 | 执行器把会话开始时 `choose` 的结果当作 `path()`；`inject(3)` 只触发一次注入 | `P7.05` | B | L1 |
| 93 | 会话开始后第一次 `SCROLL_READY_MESSAGE` 能被排空 | `P7.05` | B | L3 |
| 94 | 四个按钮矩形互不重叠 | `P7.06` | B | L1 |
| 95 | 每个面板内的点恰好命中一个目标；面板外 `Outside` | `P7.06` | B | L1 |
| 96 | 画成灰的按钮不可点（`actions()` 与 `appearance()` 同源） | `P7.06` | D | L1 |
| 97 | 每个 `PanelAction` 恰好映射到一个控制器调用（机械核对） | `P7.06` | B | L2 |
| 98 | 滚动会话中的左键按下不再落到"空选择"分支 | `P7.07` | B | L1 |
| 99 | 拖动视口框 ⇒ `set_follow(false)` 且进入手动模式 | `P7.07` | B | L1 |
| 100 | 窗口请求只保留最新（覆盖式） | `P7.08` | B | L2 |
| 101 | 跟随模式下不发窗口请求 | `P7.08` | B | L2 |
| 102 | `rows == 0` 表示"不需要窗口"，且 `take_window_request()` 会给出它 | `P7.08` | B | L2 |
| 103 | 驱动只在收到请求时调 `refresh_window`（且不调 `relieve`） | `P7.08` | B | L2 |
| 104 | 3 行画布导出成可解码 PNG（行带契约） | `P7.09` | B | L1 |
| 105 | `Partial` 产物的 PNG 头高度 == 实际写入行数 | `P7.09` | D | L1 |
| 106 | 取消后目录里没有产物（`is_stale` 删除） | `P7.09` | D | L2 |
| 107 | 超长产物跳过剪贴板但历史里存在 | `P7.10` | D | L2 |
| 108 | 短图产物同时进剪贴板与历史 | `P7.10` | B | L2 |
| 109 | 13 个诊断码 → 面板文案穷举 | `P7.11` | B | L1 |
| 110 | 驱动发布诊断码后面板 `trouble_text` 非空 | `P7.11` | B | L2 |
| 111 | `yields_partial()` 的五种停止原因逐一核对 | `P7.12` | B | L1 |
| 112 | 初始化失败（窗口不可捕获）⇒ `ScrollOutcome` 的对应态 | `P7.12` | D | L2 |
| 113 | 注入失败后会话以 `InternalError` 结束且无文件 | `P7.12` | D | L2 |
| 114 | F7 全流程真机：面板出现 → 滚动 → 停止 → 产物高度正确 | `P7.13` | A | L3 |
| 115 | 普通截图（F5）全流程逐字不变 | `P7.13` | A | L3 |
| 116 | `Preview 更新延迟` 的产品端点首测 | `P7.13` | E | L3 |
| 117 | `Stop latency` 的产品端点首测 | `P7.13` | E | L3 |

**分类合计（新增，2026-10-10 逐行重数）**：编号 `79`–`117` 共 **39 行**；A 3（86/114/115）｜B 23｜C 0｜D 11｜E 2；其中失败场景 D 占 **28.2%**（11/39）。（首版把这一行写成"A 3｜B 21｜D 12｜E 2 ⇒ 40 行"，与表格实际行数不符——按表格重数后的数字以本行为准。）

---

## 8. 风险与开放问题

### 8.1 风险（续 `docs/31 §14.3` 的 R-1…R-30）

| # | 风险 | 概率 | 影响 | 缓解 | 责任任务 |
|---|---|---|---|---|---|
| R-31 | **F7 与别的应用冲突**（`RegisterHotKey` 失败） | 中 | 高（整个功能进不去） | `HotkeyError::Conflict` 必须把**键名**带给用户；不给静默降级；L1 钉住文案含 `F7` | `P7.01` |
| R-32 | **目标窗口解析错**（选到桌面、选到被遮挡的窗口、选到 shell 自己的窗口） | 中 | 中（滚动无效果） | `choose_target` 的五条规则 + `Ambiguous` 拒绝猜测；L3 在 Chrome/记事本/Edge 上各跑一次 | `P7.02` |
| R-33 | **驱动自建设备带来的第二次 D3D 初始化成本**（首帧延迟） | 中 | 中（G13 的 500 ms 判据） | `P7.05` 的 L3 测首帧延迟；若越界则把设备创建提前到目标解析之后、交接之前（仍在驱动线程内，只是更早） | `P7.05` |
| R-34 | **面板命中区与绘制漂移**（画的位置和点的位置不同源） | 中 | 中 | `actions()` 与绘制共用 `PanelLayout`；L1 断言"画成灰的不可点" | `P7.06` |
| R-35 | **手动拖动窗口请求与自动跟随打架** | 中 | 中 | `set_follow(false)` 与 `WindowRequest` 同源（`follow` 是数据不是模式）；L2 断言跟随模式下不发请求 | `P7.07`/`P7.08` |
| R-36 | **超长产物的历史侧链路**：`AppEvent` 没有滚动变体，`save_publication` 的参数形状要现凑 | 高 | 中 | `P7.10` 先做一次"产物进历史并渲染行预览"的 L3；参数形状以 `PayloadData`/`PayloadRef.image_dimensions` 为准（`store.rs:426-434`） | `P7.10` |
| R-37 | **`Preview 更新延迟` 的阈值**（P50 ≤ 40 ms / P95 ≤ 100 ms）在真实消费者上首次可达，可能超 | 中 | 中 | `P7.13` 首测；若超，按 V2 `§23.3` 的纪律**改目标必须给依据**，不许调阈值了事 | `P7.13` |

**继承的开放风险**：R-21（公寓/并行崩溃，仍然开放，`spike/apartment-mta`）、R-30（墙钟用例与机器状态绑定）、`docs/31 §14.2` 的 `P0.09`/`P3.06` 两条 `[!]`。

### 8.2 开放问题（续 V2 `§36.2` 的 OQ-1…OQ-26）

| # | 问题 | 为什么现在不能定 | 如何定 | 阻塞什么 |
|---|---|---|---|---|
| **OQ-27** | F7 的入口形态：**直接开始**（光标下窗口即目标，crop = 内容区）还是**先确认**（进入滚动专用选区态）？ | 需要用户裁决；两者都满足 V2 `§20.6` 的"选区已确定后交接" | `P7.02` 的 `choose_target` 对两种形态是同一个函数，差别只在 `P7.07` 的交互分支有/无；先按"直接开始 + 允许在面板出现后拖动视口校正"实现 | `P7.02`、`P7.07` |
| **OQ-28** | `ScrollOutcome`/`ScrollConfig` 的具体字段（V2 `§27.1` 只给了名字与 5 类状态） | 需要"谁会读它"的答案：面板？托盘？历史备注？ | `P7.12` 落地时按"面板 + 历史备注"两个消费者最小化字段 | `P7.12` |
| **OQ-29** | 剪贴板阈值取 `MAX_DECODE_PIXELS`（24 MP）还是另一个数 | 24 MP 是**历史侧解码**的上限，不是"剪贴板放得下"的上限 | `P7.10` 做一次实测（4K 视口的滚动产物典型像素数）再定；值必须可注入 | `P7.10` |
| **OQ-30** | `Preview 更新延迟` 的端点是否要加时间戳 | V2 `§23.2` 说用 `PreviewUpdate.tick` 做对比，而 `PreviewUpdate` **没有任何时间戳**（V2 `:4756` 已登记） | `P7.13` 若要从旁证升级为实测，需要给端口加时间戳——那是改 `scroll/preview.rs`（一个新任务），不是接线的一部分 | `P7.13` |
| **OQ-31** | 首帧延迟的目标值（G13 的 500 ms 是本文提的，V2 没有这条） | 没有实测基线 | `P7.05` 的 L3 量一次，再决定目标值写在 V2 `§23.3` 还是删掉 G13 的具体数字 | `P7.05` |

---

## 9. TDD 任务书（P7.01 – P7.13）

> 任务块的字段与 `docs/31 §6` 同形：元信息九栏 → RED/GREEN/REFACTOR → 退出条件 → 提交标题。
> **本文不写实现代码**：GREEN 栏描述的是"实现哪几件事、放在哪个文件"，不是代码。
> 每个任务的 RED 必须先失败（断言失败或**编译失败**两种合法形态，`docs/31 §2.1`）。

---

### P7.01 第二个全局热键：F7

**上游**：V2 `§21.4`、`§24.6` ｜ **第一性原理**：**G13**（一次按键可达）+ **F-01** ｜ **前置**：`P6.09`（收口）｜ **可并行**：`P7.02`、`P7.03` ｜ **批次**：`[P7-A]` ｜ **层级/分类**：L1 + L3 / B + D ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`the_second_hotkey_is_registered_with_the_same_discipline()`（断言键表有两个项、id 互不相同、`CAPTURE_HOTKEY` 仍是 `0x5343`、`SCROLL_HOTKEY.virtual_key == VK_F7`、`from_id(0x5344) == Some(SCROLL_HOTKEY)`）+ `each_conflict_message_names_its_own_key()`（断言两条文案分别含 `"F5"` 与 `"F7"`）+ `an_unknown_hotkey_id_is_swallowed()`。**预期失败原因**：编译失败（`Hotkey`、`SCROLL_HOTKEY`、`from_id` 都不存在）。
- **GREEN**：把 `crates/snapclip-capture/src/windows/hotkey.rs` 的三常量 + 两函数改成键表 + 逐项注册/注销 + `from_id`；`windows/overlay/window_host.rs:61` 改成 `match`（未知 id 仍 `Some(0)`）；新增 `fn start_scroll_entry(&mut self)` 的**空实现**（它只打印一行，任务本体留给 `P7.04`）。
- **REFACTOR**：把 `overlay/tests.rs:57-77` 的冲突断言表结构复制到热键 id（`assert_ne!` 逐项 + 偏移表），让"加第三个热键时忘了改表"变成编译/测试失败。
- **退出条件**：① 三条 L1 用例通过；② L3：真机上 F7 与 F5 各自触发一次（`#[ignore]`，日志证据）；③ 未知 id 的行为有测试；④ `hotkey.rs` 里不再出现写死的 `"F5"` 文案（除 `CAPTURE_HOTKEY.label`）。
- **提交标题**：`[P7-01] the second hotkey is a row in a table, not a copy of the first`

**状态**：`[x] 完成（2026-10-10）`

**执行记录（2026-10-10）**

- **RED（实测，日志 `docs/Temp/p701-red.txt`，exit=101）**：`cmd /c "cargo test -p snapclip-capture --lib hotkey -- --test-threads=1"` → **10 previous errors**，逐条是 `E0432`（`super::{CAPTURE_HOTKEY, HOTKEYS, Hotkey, HotkeyKind, SCROLL_HOTKEY, SCROLL_HOTKEY_ID, from_id}` 全部 unresolved）、`E0425`（`register`/`unregister` 在 `super` 里找不到）、`E0559`（`HotkeyError::Conflict`/`Failed` 没有 `label`/`code` 字段）。**失败原因是编译失败（符号不存在）**，与任务书的预期一致。
- **GREEN（日志 `docs/Temp/p701-green.txt`、`docs/Temp/p701-check.txt`）**：`cargo check --workspace --all-targets` → **exit 0**（唯一警告是既有的 `unused variable: content_label`，见 `docs/31 §0.3`）；`cargo test -p snapclip-capture --lib hotkey -- --test-threads=1` → **6 passed / 0 failed / 1 ignored**；`pwsh tools/check-dependency-direction.ps1` → `checked crates/snapclip-capture/src/scroll: 17 files scanned for platform references` + `dependency direction is clean`。
- **L3（日志 `docs/Temp/p701-l3.txt`，exit=0）**：`cargo test -p snapclip-capture --lib both_hotkeys_register_and_release_on_a_real_thread -- --ignored --test-threads=1` → **1 passed**。这是 `R-31` 的第一份真机证据：**本机 F5 与 F7 都还没有被别的应用占用**（`RegisterHotKey` 两次都成功，释放之后再次注册仍成功 ⇒ 失败不是"注册了两个但忘了释放"）。
- **落地形状**（`crates/snapclip-capture/src/windows/hotkey.rs`）：`Hotkey { id, modifiers, virtual_key, label, kind }` + `HotkeyKind::{Capture, Scroll}` + `CAPTURE_HOTKEY`/`SCROLL_HOTKEY`/`HOTKEYS` + `from_id(id)` + `register(window)`/`unregister(window)`；`CAPTURE_HOTKEY_ID` 仍是 `0x5343`，新 `SCROLL_HOTKEY_ID = 0x5344`（与 `docs/24:644` 的建议值一致）。`HotkeyError` 改成**自带 `label`** 的两个变体（`Conflict { label }` / `Failed { label, code }`）——**这是对 `docs/24:645`「沿用 `HotkeyError::{Conflict, Failed(u32)}` 既有形状」的有意偏离**：形状保留了（两个变体 + `is_conflict()`，`error.rs` 的两个 `CaptureError` 映射一字未改），但键名随错误走；否则 `register` 遍历键表时无法回答"是哪一个键失败了"，而"哪一个是可行动的"。`F5_MODIFIERS`/`F5_VIRTUAL_KEY` 被 `CAPTURE_HOTKEY` 取代并删除（全仓无其它读者）。
- **落点改动**：`crates/snapclip-capture/src/windows/overlay/window_host.rs` —— `:61` 的 `WM_HOTKEY` 分支由 `if id == CAPTURE_HOTKEY_ID` 改成 `:66` 的 `from_id` + `kind` 匹配（`:70` 的 `Scroll` 臂调 `start_scroll_entry`；**未知 id 仍然返回 `Some(0)`**，不落到 `DefWindowProcW`）、`:580` `hotkey::register(window)`、`:613`/`:646` `hotkey::unregister(window)`；`crates/snapclip-capture/src/windows/overlay/session.rs:66` 新增 `pub(super) fn start_scroll_entry(&mut self)`（**只打印一行**，会话本体在 `P7.04`/`P7.05`）。
- **一处对任务书措辞的偏离（须记住）**：`§9 P7.01` 退出条件 ② 的原文是"真机上 F5 与 F7 各自**触发一次**"。**合成一次全局按键被有意否决**：`SendInput` 会把 F5/F7 发给本桌面上的**每一个**应用（包括正在运行测试的 harness 自己），这笔代价不该由一个单元测试替用户承担。因此本任务覆盖"真实注册 + 真实释放 + 再次注册"（OS 那一半）与 `from_id`/`kind`（分派那一半），**物理按键留给 `P7.13` 的矩阵行 114 端到端覆盖**。按 `docs/31 §2.4` 的纪律，这一条写在这里，而不是让用例静默跳过。
- **DoD（实测，日志 `docs/Temp/p701-workspace.txt`）**：`cargo check --workspace --all-targets` → **0 error**；`cargo test --workspace --lib -- --test-threads=1` → **67 + 536 + 51 + 23 = 677 passed / 0 failed / 69 ignored（exit 0）**；`pwsh tools/check-dependency-direction.ps1` → clean。与 `docs/31 §0.3` 的基线（477 passed / 10 ignored）相比**只增不减**；`snapclip-capture` 从 `P6.08` 记录的 533 passed / 65 ignored 变成 **536 / 66**（净 +4 = 本任务新增 6 个 L1 + 1 个 `#[ignore]` L3，删掉 2 个被取代的旧用例；`tools/check-test-baseline.ps1` 不会因此报警）。
- **未取得**：① 物理 F7 按下的端到端（归 `P7.13`）；② `WM_HOTKEY` 在真实窗口过程里的臂覆盖（今天由 `from_id` 的 L1 断言 + 泵里三行的形状保证，`P7.04` 接上会话之后由 L3 端到端覆盖）。

---

### P7.02 目标解析：选区 → 窗口

**上游**：V2 `§24.1`、`§16.2.1`、`§28.2`（计划里的 `target.rs`）｜ **第一性原理**：**F1**（必须有确定的目标窗口）+ **C7a** ｜ **前置**：`P7.01` ｜ **可并行**：`P7.03` ｜ **批次**：`[P7-A]` ｜ **层级/分类**：L1 + L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：`OQ-27` 的形态裁决（不阻塞实现，只阻塞交互分支）

- **RED**：`the_containing_window_wins_by_intersection_area()`（两条重叠候选，交集大者胜）+ `a_near_tie_is_ambiguous_rather_than_guessed()`（面积差 < 5% ⇒ `Ambiguous`）+ `no_overlap_is_no_window()` + `a_candidate_below_the_viewport_floor_is_too_small()`（`< 448 px`）+ `z_order_breaks_an_exact_tie()`。**预期失败原因**：编译失败（`scroll/target.rs` 不存在；`ScrollTarget`/`TargetChoice`/`choose_target` 都不存在）。
- **GREEN**：新建 `crates/snapclip-capture/src/scroll/target.rs`（**platform-free**：只有 `Rect` 与不透明 `u64` 身份）+ `scroll/mod.rs` 声明；`windows/overlay/hover.rs` 侧新增"从既有快照填候选"的适配（`HWND` → `u64`）。
- **REFACTOR**：把五条裁决规则写成模块 doc 的编号清单，并让每条规则指向它的测试名（一一对应，多一条规则没测试就红）。
- **退出条件**：① 五条 L1 用例通过；② `tools/check-dependency-direction.ps1` 干净（`scroll/` 文件数 +1）；③ L2：在一个真快照上（夹具构造的两窗口）走一次完整裁决；④ 模块 doc 里没有 `crate::windows` 等字样（含注释）。
- **提交标题**：`[P7-02] the target window is chosen by a rule that refuses to guess`

---

### P7.03 几何：DWM 可见边界进入生产

**上游**：V2 `§24.1`、`docs/31` 的 `P3.10` 实测 ｜ **第一性原理**：**F1** + **F7** ｜ **前置**：`P7.02` ｜ **可并行**：`P7.01` ｜ **批次**：`[P7-A]` ｜ **层级/分类**：L1 + L3 / A + B ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`the_probe_and_the_production_helper_agree_on_one_window()`（对同一 `isize` 句柄，两处给出同一对数字）+ `a_frame_rect_that_is_neither_the_client_nor_the_window_rect()`（L3：断言 `DwmGetWindowAttribute` 的结果既不等于 `GetClientRect` 也不等于 `GetWindowRect`——**这条是"我们知道差在哪"的证据**）。**预期失败原因**：编译失败（生产函数不存在）。
- **GREEN**：新建 `crates/snapclip-capture/src/windows/win/frame.rs`（或并入 `window.rs`），把探针里那份 `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` 助手提升为 `pub(crate)`；`scroll_probe.rs` 改为调用它（保留探针自己的打印）。
- **REFACTOR**：给该函数加 doc，逐字写清三种矩形的实测数字与"用错会让驱动在第一帧撞 `canvas.rs` 的不变量"。
- **退出条件**：① 两条用例通过；② 探针的 L3 复跑仍全绿（`capture_probe`）；③ 生产代码里 `GetClientRect`/`GetWindowRect` 不再被用于构造 plan（grep 证据）。
- **提交标题**：`[P7-03] the only rectangle that matches capture is the one DWM reports`

---

### P7.04 交接：`begin_scroll` 的第一个生产调用者

**上游**：V2 `§20.6`、`§17.2` ｜ **第一性原理**：**F1** + **Occam**（一个会话一个所有者）｜ **前置**：`P7.02`、`P7.03` ｜ **可并行**：无（与 `P7.05` 同一条链）｜ **批次**：`[P7-B]` ｜ **层级/分类**：L1 + L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`a_scroll_handoff_carries_geometry_and_identity_only()`（编译期断言：`ScrollHandoff` 不再有 `frame()`；有 `target()`/`crop()`/`dpi()`）+ `begin_scroll_ends_the_capture_session()`（交接后 `session.frame()` 为 `None`、状态回到可复用态）+ `three_illegal_hand_offs_are_invalid_state()`。**预期失败原因**：断言失败（旧的三访问器存在 ⇒ `frame()` 仍可调用）/ 编译失败（新的访问器不存在）。
- **GREEN**：改 `crates/snapclip-capture/src/session.rs` 的 `ScrollHandoff` 与 `begin_scroll`（载荷 = `ScrollTarget` + `crop` + `dpi`）；在 `windows/overlay/session.rs` 新增 `fn begin_scroll_session(&mut self)`，流程 = `session.begin_scroll()` → `ScrollPlan` 构造（用 `P7.03` 的几何）→ `P7.05` 的装配。
- **REFACTOR**：把"为什么不再携带 GPU 帧"写成 `ScrollHandoff` 的 doc（引用 ADR-19），并删掉 `session.rs` 里因此变成死代码的访问器。
- **退出条件**：① 三条用例通过；② `cargo check --workspace --all-targets` 干净（旧签名没有残留调用者）；③ `docs/31 §0.3` 的四个 crate 测试数**只增不减**（基线门禁）。
- **提交标题**：`[P7-04] the capture session ends where the scroll session begins`

---

### P7.05 驱动装配与端口交付

**上游**：V2 `§9.4`、`§21.2`、`§21.5`、`§19.3`、`§27.2` ｜ **第一性原理**：**G1**（正确性来自闭环）+ **F3** ｜ **前置**：`P7.04` ｜ **可并行**：`P7.06` ｜ **批次**：`[P7-B]` ｜ **层级/分类**：L2 + L3 / B + E ｜ **复杂度**：L ｜ **阻塞**：无

本任务**含两个尚不存在的适配层**（`§1.5` A2/A5），它们是"装配"这个词在这一阶段的实际内容：

| 新类型 | 位置 | 契约 |
|---|---|---|
| `OpeningFrameSource` | `crates/snapclip-capture/src/windows/scroll_source.rs` | 实现 `FrameSource`（`scroll/ports.rs:94`）；内部持 `Result<WgcFrameSource, FrameError>` 与打开时用的 `(window, axis)`；**打开失败仍是一个合法帧源**（`next` 返回 `Poll::Ended(EndReason::CaptureFailed)`/`FrameError`），因为工厂闭包不能返回 `Result`（`scroll/session.rs:689`）。探针里的 `DeferredWgcSource`（`windows/scroll_probe.rs:4447`）证明了这个形状可行，但**不能**成为生产依赖 |
| `WindowWheelActuator` | `crates/snapclip-capture/src/windows/scroll_actuator.rs` | `impl ScrollActuator`（`scroll/ports.rs:196`）：`type Path = InjectPath`（端口要求 `Copy + PartialEq + Debug`，`:199`）；会话开始时用 `choose(&TargetProbe)`（`:179`）算一次 `Choice`；`inject(notches)` 走既有 `inject(&Win32Injection, &InjectRequest)`（`:292`）并把结果翻成 `InjectOutcome`；`switch(from)` 回答"下一个传输是谁"（今天是 `SendInput` ↔ `PostMessageW`，`windowing` 见 `window_host.rs` 的看门狗用法）。**必须放在 `windows/` 侧**：它要命名 `InjectPath`/`Aim`/`screen`/`target`，`scroll/` 的平台纯净门禁不允许这些名字出现在那边 |

- **RED**：`the_factory_runs_on_the_driver_thread()`（L2：工厂里记录线程 id，断言 == 驱动线程；用假帧源）+ `the_session_starts_and_tears_down_once()`（L2：`start` → 一次 `teardown` 返回 `Some(session)`，再调返回 `None`）+ `an_unreachable_window_still_yields_a_frame_source()`（L2：`OpeningFrameSource` 用不可达句柄构造，断言 `next()` 给出 `Ended`/`Err` 而**不是** panic、也不是永远 `Idle`）+ `the_actuator_reports_the_choice_it_was_given()`（L1：`WindowWheelActuator` 的 `path()` == 构造时 `choose` 的结果；`switch` 的往返；用一个假 `InjectionTarget` 断言 `inject(3)` 只调用一次 `send_wheel`/`post_wheel`）+ `the_overlay_receives_the_first_publish()`（L3：真窗口，`SCROLL_READY_MESSAGE` 到达且面板模型非空）+ `the_first_frame_latency_is_measured()`（L3/E：记录目标解析 → 首帧的耗时，**只报不断言**，见 `OQ-31`）。**预期失败原因**：编译失败（`start_scroll_entry` 仍是空实现；`OpeningFrameSource`/`WindowWheelActuator` 都不存在）。
- **GREEN**：在 `windows/scroll_source.rs` 与 `windows/scroll_actuator.rs` 落上表的两个类型；在 `overlay/session.rs` 实现装配（`ScrollPlan::new` 的四个参数由 `P7.02`/`P7.03` 的几何与 `SPI_GETWHEELSCROLLLINES × 行高` 给出，工厂闭包内 `GraphicsDevice::create()` → `WgcFrameBackend::open` → `WgcFrameSource::new`，执行器 = `WindowWheelActuator`）；调用 `watch_scroll_preview(preview, cross, extent)` 并撤掉 `crates/snapclip-capture/src/windows/overlay/render_submit.rs:28` 的 `#[allow(dead_code)]`；把 `ScrollRuntime` 存进 `OverlayController`（并在 `cancel`/`Shutdown` 路径上 `teardown`）；**同时裁决** `providers.rs:656` 的 `ScrollFrame` 与 `scroll_source.rs:437` 的 `ScrollSourceRuntime` 的去向（`§1.5` A3/A4）。
- **REFACTOR**：把"设备属于驱动线程、公寓不声明、执行器可以跨线程而帧源不能"三条写成装配函数的 doc；删掉 `overlay.rs` 里因此不再需要的 `Option` 包装。
- **退出条件**：① 六条用例通过；② 依赖门禁干净（`apps/` 不出现 `scroll/` 内部类型——ADR-18）；③ L3 真机上一次会话能开、能停、能收尾，且**没有** context 守卫 panic；④ 首帧延迟有一次真机数字（不论多少）；⑤ `ScrollFrame` 与 `ScrollSourceRuntime` 各有一句结论（用或删），无第三个"看起来像入口"的帧类型。
- **提交标题**：`[P7-05] the driver owns its device and the overlay owns the session`

---

### P7.06 面板命中区与命令映射

**上游**：V2 `§19.4.1`、`§19.5.1`、`§27.2` ｜ **第一性原理**：**G15** + **Occam**（画与点在同一个模型上）｜ **前置**：`P7.05` ｜ **可并行**：`P7.07` ｜ **批次**：`[P7-C]` ｜ **层级/分类**：L1 + L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`the_four_buttons_do_not_overlap()` + `every_point_inside_the_panel_hits_exactly_one_target()` + `a_greyed_button_is_not_clickable()`（`actions()` 与 `appearance()` 同源）+ `each_action_maps_to_exactly_one_controller_call()`（L2：机械核对映射表，`PanelAction` 每个变体恰好出现一次）+ `a_point_outside_the_panel_hits_nothing()`。**预期失败原因**：编译失败（`PanelAction`/`PanelHit`/`hit_test`/`actions` 不存在）。
- **GREEN**：在 `crates/snapclip-capture/src/scroll/panel.rs` 加纯几何命中与动作枚举（**不含**任何平台类型）；在 `windows/overlay/` 侧新增映射表（`PanelAction` → `ScrollController::{set_follow, undo, stop, cancel}`）并在 `on_left_down`（`input.rs:93`）滚动会话活跃时先问命中。
- **REFACTOR**：把四个按钮的语义（`RETURN_TEXT`/`UNDO_TEXT`/`STOP_TEXT`/`CANCEL_TEXT`）与动作枚举放在同一处，让"新增一个按钮忘了加动作"变成编译失败。
- **退出条件**：① 五条用例通过；② `scroll/` 门禁干净（含注释）；③ `d2d.rs:1448` 仍只读 `buttons`（绘制侧不改）；④ L3：真机上点四个按钮各一次，行为与 §3.4 的表一致。
- **提交标题**：`[P7-06] the buttons that are drawn are the buttons that can be clicked`

---

### P7.07 滚动会话的鼠标语义

**上游**：V2 `§19.5`（拖动、回到最新）、`§13.4`（手动模式 = `n = 0`）｜ **第一性原理**：**G15** + **F-01** ｜ **前置**：`P7.06` ｜ **可并行**：`P7.08` ｜ **批次**：`[P7-C]` ｜ **层级/分类**：L1 + L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`a_press_during_a_scroll_session_does_not_start_a_selection()`（今天会落到"空选择"分支）+ `dragging_the_viewport_stops_following()`（`follow() == false` 且 `manual() == true`）+ `a_press_outside_the_panel_keeps_the_session_alive()` + `the_wheel_over_the_overlay_does_not_scroll_the_panel()`（放大镜分支在滚动会话里必须让位）。**预期失败原因**：断言失败（今天 `on_left_down` 在非选择态直接返回；`follow` 从不为假）。
- **GREEN**：`windows/overlay/input.rs` 的三个入口（`:93` `on_left_down`、`:125` `on_left_up`、`:11` `on_mouse_move`）加滚动会话分支；拖动视口框时发 `WindowRequest`（`P7.08` 的 `request_window`）并 `set_follow(false)`；`window_host.rs:101` 的 `WM_MOUSEWHEEL` 在滚动会话活跃时不再走层深选择。
- **REFACTOR**：把"滚动会话里的三个鼠标入口与普通截图不同"写成 `input.rs` 的分派注释，并让每个分支指向它的测试名。
- **退出条件**：① 四条用例通过；② 普通截图的鼠标语义零回归（A 类：`d2d/tests.rs` 与 `overlay` 的既有用例全绿）；③ L3：真机上拖动视口框后面板显示"回到最新"可用。
- **提交标题**：`[P7-07] inside a scroll session the mouse means the panel, not a selection`

---

### P7.08 预览窗口的请求通道

**上游**：V2 `§19.2`、`§19.3`、`§27.2` ｜ **第一性原理**：**G15** + **F8**（预览是派生物，不是真值）｜ **前置**：`P7.05` ｜ **可并行**：`P7.07` ｜ **批次**：`[P7-C]` ｜ **层级/分类**：L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`a_window_request_keeps_only_the_latest()`（覆盖式：发两次，只读到最后一次）+ `following_publishes_no_window_request()` + `rows_zero_means_no_window()`（`take_window_request()` 表达"不需要"）+ `the_driver_refreshes_only_the_requested_window()`（假画布：断言 `refresh_window` 被调一次、参数正确）+ `refreshing_does_not_relieve_the_store()`（`P5.02` 的裁决）。**预期失败原因**：编译失败（`WindowRequest`/`request_window`/`take_window_request` 不存在——`ScrollCommand` 在全仓只是一个**测试断言字符串**，见 `§4.8` 与 `§1.5` A6）。
- **GREEN**：在 `crates/snapclip-capture/src/scroll/session.rs` 加 `WindowRequest` 与 `ScrollController::{request_window, take_window_request}`（`Mutex<Option<WindowRequest>>`，与 `set_follow` 同形）；`scroll/loop_control.rs` 的驱动主循环在可中断点读取并调用 `refresh_window`（`preview.rs:448`），结果经既有端口发布。
- **REFACTOR**：把"端口一个方向 + 命令一个方向"写成 `scroll/ports.rs` 与 `session.rs` 的对照 doc；`scroll/latency_probe.rs:33` 的"no production caller today"改成指向 `P7.08`。
- **退出条件**：① 四条用例通过；② `PreviewUpdate` **没有**新增变体（`preview.rs` 的端口形状不变）；③ L3：真机上拖动一次视口框，面板内容跟随（肉眼 + 帧计数证据）。
- **提交标题**：`[P7-08] the window the consumer wants is a request, not a second channel`

---

### P7.09 停止 → 行带导出

**上游**：V2 `§17.7`、`§20.5`、`§23.3.5` ｜ **第一性原理**：**F7**（有界内存）+ **G18** ｜ **前置**：`P7.05` ｜ **可并行**：`P7.10`、`P7.11` ｜ **批次**：`[P7-D]` ｜ **层级/分类**：L1 + L2 + L3 / B + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`a_three_row_canvas_becomes_a_decodable_png()`（行带契约）+ `a_partial_result_declares_the_rows_it_wrote()`（PNG 头高度 == 实际行数）+ `a_cancelled_export_deletes_its_artifact()`（复用 `export_worker` 的 generation 语义）+ `the_band_walk_never_materialises_the_canvas()`（L2：峰值分配断言，复用 `E-MEM-1` 的方法）。**预期失败原因**：编译失败（行带出口不存在）。
- **GREEN**：`crates/snapclip-capture/src/ports.rs` 新增行带口子（ADR-21），实现留在 `apps/snapclip/src/capture/`（复用 `PngRowBandSink`）；`overlay/session.rs` 的新函数把 `Disposal::Export(&RecoveredImage)` 接到 export worker（新任务形状），产物经既有 `ArtifactWriter` 落盘。
- **REFACTOR**：把"先落盘再通知"与"取消不落盘"两条写成 doc，并让它们各自指向测试。
- **退出条件**：① 四条用例通过；② L4：一次真实的长产物导出（≥ 100,000 px）峰值内存不随高度增长；③ `E-MEM-1` 的 0.16% 结论在接了真实消费者之后仍成立；④ 取消路径的目录里没有产物。
- **提交标题**：`[P7-09] stopping writes bands, not a bitmap`

---

### P7.10 产物出口：剪贴板与历史

**上游**：V2 `§19.2.2`（`OQ-25`）、`§30.7` ｜ **第一性原理**：**G16** + **G17** ｜ **前置**：`P7.09` ｜ **可并行**：`P7.11` ｜ **批次**：`[P7-D]` ｜ **层级/分类**：L2 + L3 / B + D ｜ **复杂度**：L ｜ **阻塞**：`OQ-29`（阈值）

- **RED**：`a_long_artifact_skips_the_clipboard_and_says_so()`（N16/ADR-23）+ `a_short_artifact_reaches_both_sinks()` + `the_history_entry_carries_its_dimensions()`（`PayloadRef.image_dimensions`）+ `the_row_preview_of_a_long_artifact_renders()`（L3，`apps/snapclip/src/history/preview.rs:70`）。**预期失败原因**：断言失败（今天 `adapters.rs:89` 发布 `artifact: None`；没有任何滚动产物进历史）。
- **GREEN**：修 `apps/snapclip/src/adapters.rs:75-125` 的完成事件（带真实 `ArtifactRef`，即 ADR-22 的断链）；把滚动产物接进历史（`save_publication`，用 `PayloadData`/`PayloadRef`）；按阈值决定是否 `copy_image`。
- **REFACTOR**：把"阈值的来源与可注入性"写成常量 doc（OQ-29 的落点）；把"普通截图与滚动截图共用出口"写成一句可 grep 的断言（两条路径调用同一个发布函数）。
- **退出条件**：① 四条用例通过；② 普通截图也走同一条出口（A 类不回归）；③ L3：一条 ≥ 20,000 px 的真实产物出现在历史列表且行预览可渲染；④ 超长产物跳过剪贴板时**有可见说明**（不是静默）。
- **提交标题**：`[P7-10] a finished capture reaches the clipboard and the history, or says why not`

---

### P7.11 诊断与停止原因

**上游**：V2 `§34.4`、`§27.1`、`§23.3` ｜ **第一性原理**：**G17**（失败可解释）+ **诚实性** ｜ **前置**：`P7.05` ｜ **可并行**：`P7.09`、`P7.10` ｜ **批次**：`[P7-D]` ｜ **层级/分类**：L1 + L2 / B ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`every_diagnostic_code_has_a_panel_line()`（13 个码穷举，`scroll/panel.rs:596` 的 `trouble_text` 已经在位 ⇒ 断言的是"驱动能把它送进去"这条路径）+ `the_session_publishes_its_diagnostic()`（L2：假源发布一个码 ⇒ 面板 `trouble_text` 非空）+ `a_stopped_session_records_its_reason()`（产物旁能查到 `StopReason`）。**预期失败原因**：断言失败（`note_diagnostic`（`panel.rs:470`）今天**没有调用者**；停止原因没有任何去向）。
- **GREEN**：让驱动把诊断经既有端口送出（`PreviewUpdate` 已有 `note_diagnostic` 的输入端？若没有则在 `P7.08` 的同一处扩一条，**不新增第二通道**）；把 `StopReason` 与 `ScrollDiagnosticCode` 接到历史条目的备注字段。
- **REFACTOR**：把 13 个码与面板文案的对应表放进一处（今天文案在 `panel.rs:596`，码在 `session.rs:105`），让"码改了文案没改"变成 L1 失败。
- **退出条件**：① 三条用例通过；② 诊断路径只有一条（grep 证据）；③ `ScrollDiagnosticCode` 的 13 个变体没有新增（新增是破坏性改动，V2 `§27.1`）。
- **提交标题**：`[P7-11] the panel has something to say when the loop cannot`

---

### P7.12 失败与降级

**上游**：V2 `§24.6`、`§27.1`、`§36.2`（`OQ-22`）｜ **第一性原理**：**G17** + **Occam** ｜ **前置**：`P7.09`、`P7.10` ｜ **可并行**：无 ｜ **批次**：`[P7-D]` ｜ **层级/分类**：L1 + L2 / D ｜ **复杂度**：M ｜ **阻塞**：`OQ-28`（`ScrollOutcome` 字段）

- **RED**：`five_stop_reasons_map_to_the_documented_outcomes()` + `a_failed_first_capture_never_starts_a_driver()` + `an_injection_failure_ends_the_session_with_no_file()` + `an_unrecoverable_overshoot_is_reported_rather_than_hidden()`（`OQ-22` 的越冲：今天会永远 `committed() == 0`）。**预期失败原因**：编译失败（`ScrollOutcome` 不存在）。
- **GREEN**：在 capture crate 内落地 `ScrollOutcome`/`ScrollConfig`（V2 `§27.1`，字段按 OQ-28 最小化）；把"目标不可捕获 / 注入失败 / 越冲 / 设备丢失 / `BandError`"五类接到它；`OQ-22` 的越冲在 `P7.12` 只做**上报**（不改 `RHO_MIN`，不引入多尺度——那是 `OQ-20`/`OQ-22` 的出口任务）。
- **REFACTOR**：把"每条失败路径的产物承诺"（有/无文件）写成一张表，并让 §3.4 的表与它逐行对应（一张表两处写就必须同源）。
- **退出条件**：① 四条用例通过；② §3.4 的五行与实现逐行一致（含 `yields_partial()`）；③ `RHO_MIN`/`MIN_TILES`/`MIN_MARGIN` 的标定值**一个都没动**（grep 证据）。
- **提交标题**：`[P7-12] every way this can fail has a name and a promise`

---

### P7.13 阶段门禁、真机演练与收口

**上游**：V2 `§34`、`§23.3`、`§30` ｜ **第一性原理**：**G19** + **G20** ｜ **前置**：`P7.01`–`P7.12` ｜ **可并行**：无 ｜ **批次**：`[P7-E]` ｜ **层级/分类**：L3 + L4 ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`the_f7_flow_produces_a_real_file()`（L3：真机上 F7 → 面板 → 滚动 N 步 → 停止 → 产物高度 == 承诺值）+ `the_f5_flow_is_unchanged()`（A 类逐字回归）+ `preview_latency_has_a_measured_number()`（`OQ-30`：有数字才断言）+ `stop_latency_has_a_product_endpoint()`（ADR-24）。**预期失败原因**：断言失败（今天四条都不存在）。
- **GREEN**：完成阶段级脚本（四条真机用例 + 两条测量），把数字回填 V2 `§23.3`（含"未取得 + 原因"），按 `docs/31 §3.3` 写**只改文档**的收口提交，打标签 `scroll-p7`。
- **REFACTOR**：把"接线后新出现的两条指标"加进 `§23.3` 的指标定义表；把 `docs/31 P6.09` 的 `§34.4` 判定（"满足，有一处未接线"）更正为"接线完成，见 `docs/32`"。
- **退出条件**：① 四条 L3/L4 用例通过；② `docs/31 §4.2` 的四条阶段门禁全绿（`--test-threads=1`）；③ `E-MEM-1`/`E-ACC-1` 复跑结论不变（滚动产物接入消费者不改变匹配正确性）；④ 标签 `scroll-p7` 已打；⑤ 回滚演练（`docs/31 §3.6`）至少一次。
- **提交标题**：`[P7-13] scroll capture enters the product through the door it already had`

---

## 10. 阶段退出条件与收口

**P7 退出条件（阶段级）**：

① **F7 在生产里可用**（真机演练有产物，且产物进历史）—— 判据：`P7.13` 的 L3 用例；
② **G13–G20 逐条有证据**（含"未取得 + 原因"）—— 判据：本文 §2.1 的判据列；
③ **普通截图零回归**（A 类 20 项 + `session.rs:550` 的状态机用例）—— 判据：阶段门禁；
④ **`OQ-26`/`OQ-27`/`OQ-29`/`OQ-31` 全部有结论或明确的出口**；
⑤ 标签 `scroll-p7` 指向收口提交。

**收口提交的内容**（沿用 `docs/31 §3.3` 四项）：任务状态 / 逐指标对比（含未取得）/ 偏差 / 推送批次与标签。

---

## 11. 第一性原理与 Occam 复核

| V2 的事实/目标 | 本次接线如何保持它 |
|---|---|
| **F1**（必须有确定的目标窗口） | `P7.02` 的裁决函数把"选区 → 窗口"变成**可测规则**，`Ambiguous` 拒绝猜测 |
| **F3**（位移只能从图像测） | 接线不改变估计器：`estimate` 与四门一个都不动（`P7.12` 明确禁止动标定值） |
| **F7**（输出必须在有界内存下成为一个矩形） | `P7.09` 的行带导出是唯一的产物路径；`P7.09` 的第四条第 4 条断言"不得物化整图" |
| **F8**（预览是派生物） | `P7.08` 的选择：窗口请求是**命令**、端口仍单向；预览不成为真值 |
| **G8**（用户必须能停、能取消） | `P7.06`/`P7.07` 让按钮可点、`P7.09` 让停止有产物、取消无产物 |
| **Occam**（不新增没有消费者的机器） | 本次**不新增**：线程（`N17`）、`PreviewUpdate` 变体（`P7.08` 退出条件 ②）、`scroll/` 内部类型的公开（ADR-18）、第二套产物出口（ADR-22） |
| **`AGENTS.md`**（根因优先 / 禁兼容层 / 禁死代码 / 禁伪完成） | ADR-19（改类型而不是留不读的字段）、ADR-22（修根因：断链）、`P7.01`/`P7.05` 撤掉两处 `#[allow(dead_code)]`、§8.2 的每条 OQ 都有出口 |

---

## 12. 回填清单（执行后必须写回）

### 12.1 `docs/30`（V2）的定点增补

| 位置 | 增补 |
|---|---|
| `§19.3` 之后 | 消费者→生产者的窗口请求（ADR-20），并把 `:3620-3630` 的"尚未接线"改成"已接线，见 `P7.08`" |
| `§19.4.1` / `§19.5.1` | 四个按钮的命中区落地（`P7.06`）；`:3710`/`:3744`/`:3801`/`:3844` 四处"属装配根 `P6`"改成"由 `P7.06` 落地" |
| `§20.6` | `ScrollHandoff` 的载荷变化（ADR-19） |
| `§21.3.1` | `OQ-26` 的裁决：驱动自建设备（ADR-19） |
| `§23.2` / `§23.3` | 两条首次可达的指标（`P7.13`）+ `Stop latency` 的两个端点（ADR-24） |
| `§27.1` | `ScrollOutcome`/`ScrollConfig` 的真实字段（`P7.12`），并记下"这两个名字在整个 `P0`–`P6` 期间零定义"这一事实 |
| `§27.2` | `ScrollCommand` 的真实形状（`P7.08`）：`scroll/session.rs:1040`/`:1058` 的测试断言字符串证明它曾被当作已存在的类型；落地后要改成 `WindowRequest` + `ScrollController::{request_window, take_window_request}` 的容量 1 覆盖式，并把 `begin`/`step` 明确为**不存在**（`stop`/`cancel`/`undo` 是粘性位） |
| `§24.2` / `§28.2` | `ScrollFrame`（`crates/snapclip-capture/src/windows/providers.rs:656`）与 `ScrollSourceRuntime`（`crates/snapclip-capture/src/windows/scroll_source.rs:437`）的去向（`P7.05` 的裁决结果）；`§28.2` 的目录树要补上 `OpeningFrameSource`/`WindowWheelActuator` 两个真实类型 |
| `§28.2` | `scroll/` 的真实文件清单（`target.rs` 落地） |
| `§34.4` | "满足，有一处未接线" → "接线完成"；并补记"端口无人交付、按钮不可点"这两条当时漏掉的缝 |
| `§35` | 新增 `### P7` 一节（本文 §9 的粗排） |
| `§36.2` | `OQ-27`…`OQ-31` |

### 12.2 `docs/31`（P0–P6 台账）的定点增补

| 位置 | 增补 |
|---|---|
| `§14.1` | `OQ-26` 的出口改成"由 `P7.05` 落地，见 `docs/32`" |
| `§15.4` | 标签表加 `scroll-p7` |
| `§1.1` / `§1.5` | 在表末追加一行 P7（**不改** P0–P6 的计划数字，沿用"只记录实际形态"的惯例） |

### 12.3 本文自身

任务执行完毕后，每个任务块补 `**状态**：[x] 完成（日期）` 与 `**执行记录（日期）**`（`docs/31 §6` 的格式），并把"预期失败原因"替换成**实测的** RED 证据（日志路径），未取得的条目如实写"未取得 + 原因"。
