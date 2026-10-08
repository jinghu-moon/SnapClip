# SnapClip 滚动截图 V2 · TDD 实施任务清单

> **上游设计权威**：`docs/30-scroll-capture-design-v2.md`（下称 **V2**）。本文与 V2 冲突时**以 V2 为准**。
> **本文性质**：V2 §35 实施计划（P0–P6）的 **TDD 细化** + **提交/推送/回滚协议**（V2 没有这部分，本文新增；它的约束是对 V2 §33.6「测试先于删除」的执行层补充）。
> **文档版本**：v1 · 编写日期 2026-10-08 · 协议版本 **C1–C11**（本文 §3）
> **状态词表**：`[ ]` 未开始 · `[~]` 进行中 · `[x]` 完成 · `[!]` 阻塞 · `[-]` 取消
> **提交粒度**：一个任务 = 一个提交（`RED → GREEN → REFACTOR` 三阶段**落在同一个提交里**）

---

## 0. 文档元信息

### 0.1 本文与其它文档的关系（五条声明）

| # | 声明 |
|---|---|
| 1 | **与 V2（`docs/30`）的关系**：本文是 V2 §35 的 TDD 细化。**任何冲突以 V2 为准**；本文只增加"怎么做、怎么验、怎么提交"三件事，不改变 V2 的任何决策。 |
| 2 | **与 `docs/24`（旧滚动截图任务清单 S0–S5）的关系**：本文**取代** `docs/24` 的阶段切分。`docs/24` 降级为历史参考；其 §S0.2 的 16 变体 `ScrollStopReason` 词表已被 V2 §20.4 收敛为 11 个（D-8），不得再从 `docs/24` 取词表。 |
| 3 | **与 `docs/23`（模块化重构任务清单）的关系**：本文**不修改** `docs/23`。P6 阶段的清理任务与 `docs/23` 的遗留项（T1.8 三个 pass 顺延、`docs/23:270` 的 T6.x 状态过期）有交叉，已在本文 §12 逐条标注。 |
| 4 | **`docs/19`（V1 设计）保持不变**：本文**不覆盖**、不修改 `docs/19`。对 V1 的批评只出现在 V2 §4 与本文的引用里。 |
| 5 | **本文新增的内容**：提交/推送/回滚协议（C1–C11）、pre-push 钩子、任务级/阶段级门禁分层、并行策略与合并点、测试矩阵 → 任务的映射表、调研任务（`RES-*`）。 |

### 0.2 编写前的必做检查（逐项结果，2026-10-08 实测）

| # | 检查项 | 结果 |
|---|---|---|
| 1 | `docs/30-scroll-capture-design-v2.md` 存在且已完整阅读 | ✅ 存在，**432,959 字节 / 4,009 行**；本次读取了 §2/§15/§16/§17/§19/§20/§21/§22/§23/§24/§25/§27/§28/§29/§30/§31/§33/§35/§36 |
| 2 | `docs/19-scroll-capture-design.md` 存在（不覆盖） | ✅ 存在，76,845 字节；**本文与 P0–P6 全程不写它** |
| 3 | 当前代码基线已确认 | ✅ **已实测**，见 §0.3（**477 passed / 10 ignored / 0 failed**，并用精确增量解释了 `docs/19` 的 475/9/0） |
| 4 | `docs/23` / `docs/24` 的修正状态已确认 | ✅ 见 V2 §36.4 台账；`docs/24:1` 的 §0.4「OpenCV 是 capture 默认依赖」是**事实错误**（V2 §4.4） |
| 5 | `refer/` 目录存在 | ✅ 存在，14 个子目录；关键项：`refer/snow-apps/snow-crates`（Apache-2.0）、`refer/snow-apps/snow_shot`（GPL-3.0-or-later）、`refer/shot-refer/{Crisp-main,ShareX-develop,PowerToys-main,Starshot-main}`、`refer/webshot-master`、`refer/cdp-html-shot-main` |
| 6 | 参考项目许可已确认 | ✅ 见 V2 §0.3：`snow-crates/` = **Apache-2.0**（可并入 AGPL-3.0 工程）；`snow_shot/` = **GPL-3.0-or-later**（**不改编代码**，只做行为对照）。**这一条决定了 P1.20 必须自写 ORB**（V2 §36.1 D-1）。 |
| 7 | 目标文件 `docs/31-scroll-capture-tdd-tasklist.md` 是否已存在 | ✅ **不存在**（`Test-Path` 为 `NOT EXIST`）→ 本文为**新建**，不覆盖任何文件 |
| 8 | `.githooks/` 目录是否已存在 | ❌ **不存在** → §4.3 已作为 **P0.8** 的交付物（创建 + 安装 + 验证生效） |
| 9 | `git config core.hooksPath` 当前值 | ❌ **未设置**（`(unset)`）→ 规则 C4 的"推送级门禁"今天**没有机械保证**，P0.8 负责建立 |
| 10 | 外部调研工具可用 | ✅ 本会话可用（`web_search` / `web_fetch`）；V2 §6.4 的 19 个主题已有第一轮结果 |
| 11 | 参考项目关键源码可访问 | ✅ 只读可访问；`snow-stitch-images` / `snow-capture` 的行数级清单见 `docs/29-snow-shot-scroll-capture-study.md` |

### 0.3 本机实测基线（**P0.1 的产物，本文编写时已跑**）

**命令**：`cargo test --workspace --lib`（Debug，2026-10-08，本机 = Windows 11 24H2 / RTX 4070 Ti SUPER / 单屏 2560×1440 / `PixelRatio = 1`）

| crate | passed | failed | ignored | 耗时 |
|---|---|---|---|---|
| `snapclip-app` | 56 | 0 | 3 | 1.00 s |
| `snapclip-capture` | **347** | 0 | **7** | 10.58 s |
| `snapclip-history` | 51 | 0 | 0 | 0.21 s |
| `snapclip-model` | 23 | 0 | 0 | 0.00 s |
| **合计** | **477** | **0** | **10** | — |

**`cargo check --workspace --all-targets`**：**0 error**，**1 warning** = `unused variable: content_label`（`apps/snapclip/src/history/view.rs`）。

**`docs/19` 的「475 passed / 9 ignored / 0 failed」被证实，且差值的 3 个测试可逐一定位**：

| 项 | V1 文档数字 | 今天实测 | 差值来源 |
|---|---|---|---|
| passed | 475 | 477 | **+2** = `crates/snapclip-capture/src/windows/scroll_probe.rs` 的两个**非 ignored** 自证测试（`the_shift_estimator_recovers_a_known_synthetic_shift`、`the_shift_estimator_is_not_fooled_by_line_structure`），提交 `c15e614` |
| ignored | 9 | 10 | **+1** = 同文件的 `inject_probe`（`#[ignore]`，需真实桌面），提交 `c15e614` |
| failed | 0 | 0 | — |
| `snapclip-capture` 单独 | 345 / 6 | 347 / 7 | 同上（+2 / +1） |

**结论**：基线可信；**+3 全部来自 P0.6 的测量装置**，不是回归。**这一条必须写进 P1 的 A 类回归测试基线**（§2.6 的"改动前：记录基线数字"）。

**同批已确证的环境事实**（P0–P6 全程当作前提，不再重测）：

| 事实 | 值 | 出处 |
|---|---|---|
| 滚动截图代码量 | **0 行**（全仓无 `Scroll`/`ScrollAxis`/`ScrollTarget`/`TileStore`/`ScrollSink`） | V2 §5.1 |
| `snapclip-capture` 包数 | **30**（无 tauri/wry/gpui-kit） | V2 §5 |
| `snapclip-history` 包数 | **47**（`image 0.25.10`，无 `image-webp`） | V2 §5 |
| `snapclip-model` 包数 | **8**（依赖白名单只允许 serde 家族） | V2 §5 |
| 依赖方向门禁 | `tools/check-dependency-direction.ps1`（`$SHELL_ONLY` 在 `:35`，capture 表在 `:37`） | V2 §5 |
| 已占用消息 id | `WM_APP + 1 / 2 / 17 / 18 / 19 / 43 / 44` | V2 §21.4 |
| 已确证可用的探针 | `crates/snapclip-capture/src/windows/scroll_probe.rs`（`#[cfg(test)]`）；`bitblt::capture_rect(rect)` | 本会话 P0.6 |

### 0.4 本文的方法论声明（贯穿全部任务）

**第一性原理**：每个任务的"第一性原理追溯"一栏必须填出 V2 §2.1 的 `F?` 编号。**填不出这一栏的任务是历史遗留，应删除或重新定义**（V2 §3.9 的 Occam 检查的机械执行方式：这一栏就是判据）。

**奥卡姆剃刀**：本清单**不为"看起来完整"增加任务**。因此：

* 每个任务只列**它自己**的 RED/GREEN/REFACTOR；公共模板（提交信息、门禁命令、分层定义）在 §2/§3/§4 定义一次，**不在 60 多个任务里重复 60 多遍**（重复本身违反 Occam）。
* 能由一个测试覆盖的，不拆成两个任务；能由一次 `edit` 完成且不需要独立验证的，并入相邻任务。

**破坏性改动**：V2 §33 的 D-1…D-15 / R-1…R-7 每一条都必须在 `当前抽象导致 X → 根本原因是 Y → 因此删除/重构 Z` 三段式**已经写好**的前提下执行（V2 §33.1/§33.2 的表格就是三段式，本文直接引用其编号，**不重抄**）。**执行顺序由 §2.6 强制：先测后删。**

### 0.5 编号与文件规则

| 项 | 规则 |
|---|---|
| 任务编号 | `P{阶段}.{序号}`（如 `P1.07`）；调研任务 `RES-{n}` |
| 批次 | `[P0-A]`…`[P6-A]`；P1 细分为 `[P1-A]`（类型与漏斗）/`[P1-B]`（门限与先验）/`[P1-C]`（画布与导出）/`[P1-D]`（ORB 与 `E-ACC-1`） |
| 阶段标签 | `scroll-p0` … `scroll-p6`（轻量标签，规则 C3） |
| 新增文件 | 一律先建 `#[cfg(test)]` 测试骨架再建实现（§2.1） |
| 文档引用 | 一律写 `docs/30 §x.y` 或 `docs/30:<行号>`，**不允许只写"见设计文档"** |

### 0.6 本文对 V2 的补充（**四处 deviation，前三处已按用户授权回填 `docs/30`**）

> **回填状态**：前三处均已写入 `docs/30-scroll-capture-design-v2.md`（**未覆盖原文，只做定点增补**）——
> DEV-1 → `§28.2` 新增 test-only 文件块（`scroll/testkit.rs`）与"10 生产 + 1 test-only"口径、`§33.3` 文件清单由 9 补为 11（**顺带修正了原文漏列 `orb.rs` 的内部矛盾**）；
> DEV-2 → `§35 P4` 的 `P4.1` 拆为 `P4.1a`（trait，capture）/ `P4.1b`（实现，shell）并附理由；
> DEV-3 → `§33.1` 的 `D-10` 拆为 `D-10a`（硬失败）/ `D-10b`（静默降级）、`§24.3` 修正 `wgc.rs:107` 的表述并说明"两种相反失败模式的共同落点"、`§24.8` 的探测用例扩为两条；
> **DEV-4** → `§21.3` 新增"第 1 步的实测结果（`P0.02` / `T-THREAD-1`）"整块（2026-10-08 执行时新增，见下）。
> 下表保留**原始登记内容**（作为"当时看到了什么"的记录）。

| # | 补充 | 为什么必须补 | 回填位置 |
|---|---|---|---|
| **DEV-1** | 新增 **test-only 文件** `crates/snapclip-capture/src/scroll/testkit.rs`（以 `#[cfg(test)] mod testkit;` 注册） | V2 §35 的 **P1.1** 要求"合成夹具生成器（多尺度结构 + 自证）"，而 V2 §28.2 只枚举了 `scroll/` 的 **10 个**文件、**没有给它位置**。把生成器塞进 `mod.rs`（自述"只有约 40 行"）会破坏该文件的定位；塞进 `displacement.rs` 会让"夹具"与"被测物"同文件（夹具自证的价值就是**独立于被测实现**）。**因此新增一个 test-only 文件是本清单对 V2 的最小补充** | `docs/30 §28.2` 的新增清单（10 → 11 个文件），并在 `§28.3` 的"不做"表里保留"不建 `scroll/tests/` 目录"（一个 test-only 文件 ≠ 一个测试目录） |
| **DEV-2** | V2 §35 的 **P4.1** 一行写"`RowBandSink`/`RowBandWriter` + `PngRowBandSink` 落在 shell 组合根"，但 V2 §27.3/§33.3 把这两个 **trait** 定为 capture 侧新增（`png` 不得进 capture） | 一行把"端口"与"实现"混在一起。若不拆，执行者会把 trait 放进 shell → 滚动模块无法在自己的 crate 里被测试（破坏 §4.1 的 L1/L2 门禁），或会把 `png` 加进 capture（破坏 §28.4 的目标） | `docs/30 §35 P4.1` 拆为两句："trait 在 capture（`canvas.rs` 或 `bands.rs`）、`PngRowBandSink` 实现落 shell 组合根" |
| **DEV-3** | V2 §33.1 的 **D-10** 把 `crates/snapclip-capture/src/windows/win/wgc.rs:107` 与 `:110` **都**写成"吞错误"，与代码不符 | 逐行核实：`:106-108` 的 `SetIsCursorCaptureEnabled(false).map_err(...)?` **用 `?` 传播** → 它的失败模式是"**`IGraphicsCaptureSession2` 不可用 ⇒ 整体捕获失败**"；`:110-112` 的 `SetIsBorderRequired(false)` 才是"**只 `eprintln`、静默降级**"。**两种相反的失败模式**（一个太严、一个太松）都必须由 `P2.04` 的能力探测统一处理，否则执行者会按 V2 的措辞只修一处 | `docs/30 §33.1 D-10` 改为两个独立条目：D-10a"`SetIsCursorCaptureEnabled` 的**硬失败**改为探测 + 记录 + 退回掩码排除"、D-10b"`SetIsBorderRequired` 的**静默降级**改为产出 `CaptureOptionUnavailable` 诊断"；`§24.3` 的 `CaptureCapabilities` 表同步说明它同时解决这两种失败模式 |
| **DEV-4**（执行时新增） | V2 §21.3 把 `T-THREAD-1` 的判据写成"**若 `cargo test --workspace --lib` 今天就 panic**"，并预期它能回答这个问题 | 2026-10-08 实测：**全量 lib 测试不会 panic**（每个测试都在同一线程创建设备并使用它）→ **该门禁对这条不变式不敏感**；但生产路径**真的会 panic**（设备在 capture worker 创建，overlay 线程放大镜取色时 `submit`），已用 `the_production_hand_off_trips_the_context_guard` 固定为可执行证据。若照原文把"全绿"当作通过，`P0.02` 会被误判为完成，`P2.02` 的回读路径会带着一个**假的**不变式进实现 | `docs/30 §21.3` 新增"第 1 步的实测结果（`P0.02` / `T-THREAD-1`）"整块（四条事实 + 调用点清单修正 + 问题域扩大 + 发布期如何保证）；§6 `P0.02` 的状态行改 `[!]`；§3.4/§14.2 的 `[!]` 表与 R-2 同步 |

---

## 1. 总览

### 1.1 阶段总表

| 阶段 | 名称 | 交付物 | 任务数 | 测试层级 | 批次 | 标签 |
|---|---|---|---|---|---|---|
| **P0** | 前置实验与基线 | 实验数据 + 门禁设施 | 9 | 混合 | `[P0-A]` | `scroll-p0` |
| **P1** | 纯逻辑核心 | `scroll/` 11 + `windows/` 2 文件 | 24 | **L1（全部）** | `[P1-A..D]` | `scroll-p1` |
| **P2** | 平台帧源 | 窗口级 WGC 多帧 | 8 | L2 + L3 | `[P2-A]` | `scroll-p2` |
| **P3** | 注入与闭环 | 驱动器 + 会话 | 9 | L1/L2 + L3 | `[P3-A]` | `scroll-p3` |
| **P4** | 导出与内存 | 行带端口 + 流式导出 | 7 | L1/L2 + L4 | `[P4-A]` | `scroll-p4` |
| **P5** | 预览 UI | `PreviewStream` + 覆盖层面板 | 6 | L1/L2 + L3 | `[P5-A]` | `scroll-p5` |
| **P6** | 清理与收口 | 删除/重构/回填/门禁 | 9 | 全量回归 | `[P6-A]` | `scroll-p6` |
| | **P0–P6 小计** | | **72** | | | |
| **RES** | 调研（跨阶段） | 调研笔记 + 影响记录 | 9 | — | 跟随所属阶段 | — |
| | **合计** | | **81** | | | |

### 1.2 任务统计（可机检口径）

| 统计项 | 值 | 说明 |
|---|---|---|
| 总任务数 | **81** | 工程任务 **72**（P0–P6）+ 调研任务 **9**（`RES-1`…`RES-9`） |
| 复杂度点数 | **167** | S=1 / M=2 / L=4 / XL=8，**不折算人日**（没有可靠依据，AGENTS.md 第 6 条禁止臆测） |
| 测试用例行（矩阵） | **78** | V2 §30 的 7 组矩阵逐行（30.1=12 / 30.2=10 / 30.3=16 / 30.4=12 / 30.5=9 / 30.6=9 / 30.7=10） |
| A 类（回归冻结，新增用例测旧行为） | **20** | 其中矩阵内 5 行 + 矩阵外约 15 条既有回归测试（`P6.01` 的清单） |
| B / C / D（矩阵内，互斥口径） | **41 / 9 / 23** | 见 §13.8 的逐行统计与与 V2 §30 末"约 55 / 约 30"的对账 |
| **失败场景占比** | **23 / 41 = 56.1%** | 要求 ≥ 50%（用户 §12）**已满足，且是逐行数出来的，不是声称的** |
| 层级分布（78 行） | L1 = 32 · L2 = 16 · L3 = 24 · L4 = 2 · L1+L2 = 1 · L1+L3 = 1 · CI = 2 | 与 V2 §30 一致 |
| 可并行任务 | **41** | 构成 **14** 个并行组 `G1`…`G14`，见 §1.4 与 §15.5 |
| 涉及 V2 章节 | §2/§3/§11–§31/§33/§35/§36 | 逐任务"上游依据"栏 |

### 1.3 阶段依赖图（含并行标注与合并点）

```
P0.01 基线 ✅ ──────────────────────────────────────────────┐
P0.02 T-THREAD-1 ──[!]──┐                                   │
P0.03 E-PERF-1 ══╗      │                                   │
P0.04 E-PERF-2 ══╣ G1   │                                   │
P0.05 E-CAP-1  ══╣      │                                   │
P0.09 E-INJECT-1 补齐 ═╝│                                   │
P0.06 E-INJECT-1 最小版 ✅                                  │
P0.07 路由/行数读取 ══╗ G2                                  │
P0.08 .githooks 前置 ═╝                                    ▼
P1.01 testkit ─┬─ P1.02 Axis ═╗ G3 ─ P1.03 Observation ─ P1.04 Displacement
               │               ╝
               ├─ P1.05 第 1 层 ═╗
               ├─ P1.06 第 2 层 ═╣ G4（`Candidate` 先冻结）── P1.07 第 3 层
               ├─ P1.08 门一 ═══╗
               ├─ P1.09 门二 ═══╣ G5（`Displacement` 先冻结）
               ├─ P1.10 门三 ═══╣ ── P1.12 scene_cut ── P1.13 Scratch/ablation
               ├─ P1.11 门四 ═══╝ ── P1.14 P1 先验 ── P1.15 手动模式
               │                  ── P1.16 三分类
               ├─ P1.17 画布 ═╗ G6 ── P1.18 旧像素优先 ── P1.19 双向/Contained
               ├─ P1.20 BandStore ═╗ G7 ── P1.22 撤销
               ├─ P1.21 上限三层 ═╝
               ├─ P1.23 自写 ORB ═╗ G8
               └─ P1.24 E-ACC-1 ═╝ ← P1 的最后一件事（`[!]` 门禁）      ▼
P2.01 CreateForWindow + pool 复用 ─┬─ P2.02 WgcWindow/read_region ═╗
                                   ├─ P2.03 FrameSource ═══════════╣ G9
                                   ├─ P2.04 CaptureCapabilities ══╣
                                   └─ P2.05 拓扑三档 ══════════════╝
P2.06 后端选择 ── P2.07 结束语义 ── P2.08 无桌面可运行                  ▼
P3.01 actuator 两路径 ═╗ G10 ── P3.03 自检/切换
P3.02 choose() ════════╝
P3.04 闭环/等待稳定 ═╗
P3.05 ĝ / E-CTRL-1 ══╣ G11 ── P3.07 停/取消 + 延迟 ═╗
P3.08 命令端口 ══════╣                              ╣ ── P3.09 装配/交接
P3.04–P3.06 的会话 ══╝                              ╝                ▼
P4.01 trait ═╗
P4.02 PngRowBandSink ═╣ G12 ── P4.04 u32 越界 ── P4.05 严格递增/Abort
P4.03 消除 4 份拷贝 ═╣                                                │
P4.06 换出清理 ══════╝ ── P4.07 E-MEM-1 ─────────────────────────────┤
                                                                     ▼
P5.01 PreviewStream ═╗
P5.02 窗口化缩略 ════╣ G13 ── P5.05 渲染回读 ── P5.06 不阻塞/延迟
P5.03 视口框三态 ════╣
P5.04 撤销/拖动 ═════╝                                               ▼
P6.01 A 类回归冻结 ── P6.02 B/D 补齐 ── P6.03 删除 D ── P6.04 R/改名
P6.05 静默跳过 ═╗ G14
P6.06 失效注释 ═╣ ── P6.08 回填/命名 ── P6.09 收口
P6.07 门禁扫描 ═╝
```

**读图规则**：`═╗ ═╣ ═╝` 表示同一并行组（接口先冻结，实现互不引用）；`──` 表示数据或状态依赖；`[!]` 表示该任务不通过就必须走侧分支（§14.2）；`✅` 表示已完成。

**关键路径**（最长链，决定交付时间）：`P0.01 → P0.02 → P1.01 → P1.04 → P1.06 → P1.08 → P1.14 → P1.16 → P1.17 → P1.20 → P1.24 → P2.01 → P2.03 → P3.01 → P3.04 → P3.07 → P4.01 → P4.03 → P4.07 → P5.01 → P5.05 → P6.01 → P6.02 → P6.04 → P6.09`（**25 个任务**）。**这条链上的任何任务都不允许被跳过或与相邻任务合并提交。**

### 1.4 并行组与合并点

**这是本文档唯一的一份并行组清单**（§15.5 只引用它）。**规则**：并行组的成员**必须**先冻结接口（表中"并行前提"列）；**禁止并行的情况**（用户 §8.4）：有数据依赖、有状态依赖、改同一函数、属破坏性改动、受阻塞（规则 C11）——**这五条优先于下表**。

| 组 | 成员 | 并行前提（先冻结的接口） | 合并点（必须跑全部门禁） |
|---|---|---|---|
| **G1** | `P0.03` ∥ `P0.04` ∥ `P0.05` | 无共享代码（三个实验互不调用） | `[P0-A]` 推送前 |
| **G2** | `P0.07` ∥ `P0.09` | `Probe` 结构（`P3.02` 的输入） | `P3.02` 开写前 |
| **G3** | `P1.02` ∥ `P1.03` | `Axis` 的三个 const fn | `P1.04` 开写前 |
| **G4** | `P1.05` ∥ `P1.06` ∥ `P1.07` | `enum Candidate` | `P1.08` 开写前 |
| **G5** | `P1.08` ∥ `P1.09` ∥ `P1.10` ∥ `P1.11` | `Displacement` 结构 | `P1.12` 开写前 |
| **G6** | `P1.17` ∥ `P1.19` | `CoverageMap` 形状 | `P1.20` 开写前 |
| **G7** | `P1.20` ∥ `P1.21` | `BandStore` 预算接口 | `P1.22` 开写前 |
| **G8** | `P1.23` ∥ `P1.24` | `SecondOpinion` 的布尔接口 | `P1.23` 完成后跑 `E-ACC-1` 全量 |
| **G9** | `P2.02` ∥ `P2.03` ∥ `P2.04` ∥ `P2.05` | `Poll` / `CaptureCapabilities` | `P2.06` 开写前 |
| **G10** | `P3.01` ∥ `P3.02` | `InjectRequest`/`InjectOutcome` | `P3.03` 开写前 |
| **G11** | `P3.04` ∥ `P3.05` ∥ `P3.08` ∥ `P3.09` | `ScrollSession` 字段集 | `P3.07` 开写前 |
| **G12** | `P4.01` ∥ `P4.02` ∥ `P4.03` ∥ `P4.06` | `RowBandSink` 的三个签名 | `P4.04` 开写前 |
| **G13** | `P5.01` ∥ `P5.02` ∥ `P5.03` ∥ `P5.04` | `PreviewUpdate` 形状 | `P5.05` 开写前 |
| **G14** | `P6.05` ∥ `P6.06` ∥ `P6.07` | 脚本/注释互不重叠 | `P6.08` 开写前 |

### 1.5 提交与推送计划概览

| 阶段 | 预计提交数 | 推送批次 | 标签 |
|---|---|---|---|
| P0 | 9 | `[P0-A]`（每 3 任务推一次 → 3 次推送） | `scroll-p0` |
| P1 | 24 | `[P1-A]`(6) `[P1-B]`(6) `[P1-C]`(6) `[P1-D]`(6) | `scroll-p1` |
| P2 | 8 | `[P2-A]`（3 次推送） | `scroll-p2` |
| P3 | 9 | `[P3-A]` `[P3-B]`（3 次推送） | `scroll-p3` |
| P4 | 7 | `[P4-A]`（3 次推送） | `scroll-p4` |
| P5 | 6 | `[P5-A]`（2 次推送） | `scroll-p5` |
| P6 | 9 | `[P6-A]` `[P6-B]` `[P6-C]`（3 次推送） | `scroll-p6` |
| **合计** | **72 个任务提交 + 7 个阶段收口提交 = 79** | **约 20 次推送** | 7 个标签 |

**提交信息的唯一模板**在 §3.5；**回滚协议**在 §3.6；**规则 C1–C11** 在 §3.2。**每个任务一个提交**（`C2`）是本文档的硬约束，不允许把两个任务压进一个提交来"省事"。

---

## 2. TDD 纪律（本文档的核心方法论）

### 2.1 每个实现任务的循环：三阶段，**一个提交**

```
┌──────────────────────────────────────────────────────────────────────┐
│ 步骤 1 · RED（红灯）                                                  │
│   先写测试。测试必须失败。必须写明：                                   │
│     测试文件 + 测试函数名 + 断言内容 + **预期失败原因**                │
│   合法形式：(a) 断言失败；(b) 编译失败（被测符号尚不存在）             │
├──────────────────────────────────────────────────────────────────────┤
│ 步骤 2 · GREEN（绿灯）                                                │
│   写最小实现让测试通过。必须写明：实现文件 + 函数签名 + 核心逻辑        │
├──────────────────────────────────────────────────────────────────────┤
│ 步骤 3 · REFACTOR（重构）                                             │
│   在测试保护下重构。必须写明：重构目标 + 不变量断言 + 回归测试          │
└──────────────────────────────────────────────────────────────────────┘
```

**关键纪律**：三阶段**作为一个提交**落地，**不是三次提交**。

* **规则 C5**：禁止把 `[RED]`“仅测试、未实现”的状态推送到 `main`。RED 的失败证据**写在提交信息正文里**，而不是作为一个红色提交存在。
* **远程历史必须始终全绿**：`git bisect` 才能用（规则 C9）。
* 上一条纪律的**代价**必须承认：本仓库不是 rustfmt-clean，**禁止对整 crate 跑 `cargo fmt`**（2026-10-08 曾因它重排 52 个无关文件、6324 行改动）。改动大时只格式化**自己新建的文件**，且逐文件确认 diff 只含自己的改动。

### 2.2 测试分层（与 V2 §29.2 对齐）

| 层级 | 含义 | 运行环境 | CI 可运行 | 门禁归属 |
|---|---|---|---|---|
| **L1** | 纯逻辑（`scroll/` 内部，无桌面/无 GPU/无 Win32） | 任意 | ✅ | 任务级 |
| **L2** | 集成（`scroll/` + **mock 端口**） | 无桌面 | ✅ | 任务级 |
| **L3** | 真实桌面（真实 WGC / 注入 / Chrome） | 真实交互桌面 | ❌ `#[ignore]` | **阶段级** |
| **L4** | 独立进程性能（`Release` + 稳定机器） | 独立进程 | ❌ `#[ignore]` | **阶段级** |

**L1/L2 的存在条件**：`scroll/` 下除 `mod.rs` 的 re-export 外**不得引用 `crate::windows`**（V2 §28.4）。这条不是口号——**P6.04 的 8 行门禁**是它的机械保证。

### 2.3 测试先于删除（V2 §33.6 的硬约束，**禁止颠倒**）

```
1. 先写 A 类回归测试（冻结当前行为：普通截图/区域/窗口/快捷键/Cancel/既有 UI）
2. 再写 B/D 类滚动测试（含 E-ACC-1 合成扫描）
3. 然后执行 D-1…D-15 / R-1…R-7 / 新增 / 移动
4. 最后跑全部门禁 + E-* 实验回填 §23.3
```

**为什么第 1 步必须在删除之前**：AGENTS.md 第 7/8 条要求"**相关已有功能正常**"。第 1 步是把这个要求变成**可判定**的唯一办法。**任何一步的顺序颠倒都会把"破坏性重构"变成"不可验证的重写"**（V2 §33.6）。

### 2.4 测试命名与组织

| 规则 | 内容 |
|---|---|
| 命名表达**行为** | `rejects_a_shift_that_exceeds_the_viewport`，不写 `test_estimate_2` |
| 失败用例命名含失败类型 | 必须能一眼看出是 failure path（如 `..._exceeding_viewport`、`..._returning_the_boundary_value`） |
| `#[ignore]` 必须注明原因与恢复条件 | `#[ignore = "requires a real interactive desktop; see docs/31 §8.7"]` |
| **禁止静默跳过** | 真实桌面用例只有两种合法形态：**(a)** `#[ignore]`；**(b)** 显式环境断言（`assert!` 失败即红）。**不允许 `if !desktop_available() { return; }`**（D-14）。**"存在但不可见"与"不存在"等价。** |
| 组织 | 沿用仓库惯例：**内联 `#[cfg(test)] mod tests`**，不建 `scroll/tests/` 目录（V2 §28.3） |

### 2.5 绿色提交要求（规则 C6 / C7）

```
[C6] 每个提交必须满足（三条全过才算绿）：
     cargo check --workspace --all-targets   → 0 error
     cargo test  --workspace --lib           → 全绿
     pwsh tools/check-dependency-direction.ps1 → clean

[C7] 禁止提交：
     - 编译不过的中间态（哪怕只是 [RED] 写了一半）
     - 测试红着但"我知道后面会绿"
     - 注释掉测试以让 CI 通过
     - 把多个不相关任务塞进一个提交
```

### 2.6 破坏性改动的测试保障（开发期特殊要求）

**这类任务在本文里的标记**：任务的"破坏性"一栏填 `D-x` / `R-x`（引用 V2 §33 的编号）。

| 字段 | 内容（**每一条都必须真实填写，不允许"不适用"占位**） |
|---|---|
| 改动前三段式 | 直接引用 V2 §33.1 / §33.2 对应行的三列（**本文不重抄**） |
| 改动前测试（A 类） | 哪些 A 类用例冻结了当前行为 + 该行为今天是否真的被杀掉（**若 A 类全绿说明没测到，必须补**） |
| 改动内容 | 具体删除/重构/移动什么（文件 + 行区间） |
| 改动后测试（B/D 类） | 哪些用例验证新行为 |
| 回归验证 | 改动后 A 类**全部通过**，且 §0.3 的四个 crate 数字**只增不减**（新增测试只能让 passed 增加；ignored 变化必须能逐条解释） |

**基线漂移的记账规则**：任何一次提交后，`passed` 比 §0.3 少，或 `ignored` 增加而**说不出是哪 1 个用例**，都视为回归。**这条规则今天已经用过一次**（477/10/0 的 +3 被逐条定位到 `scroll_probe.rs`）。

### 2.7 第一性原理检查清单（每个任务落地前自查）

```
□ 这个任务解决的基本事实是什么？→ 填出了 V2 §2.1 的 F?
□ 如果今天从零开始、只知道这个事实，我们是否仍然会做这个任务？
□ 它是否因为"以前就是这么做的"而存在？（若是 → 删除或重新定义）
□ 它里面的每个数字是否有推导链？无链条的必须标"启动值，待校准"
□ 它是否引入不必要的抽象？（Occam：为什么现有实体无法承担？）
```

---

## 3. 提交与推送协议（**本文新增**）

### 3.1 核心原则：提交与推送解耦

| 动作 | 频率 | 作用 | 成本 |
|---|---|---|---|
| **本地提交** | 每个任务完成 | 原子变更、支持回滚、支持 `bisect` | 接近零 |
| **分支内中间推送** | 每 3–5 个任务或一个逻辑分组完成 | 备份 + 早期反馈 | CI 一次 |
| **推送到 `main`** | **每个阶段完成** | 阶段边界、可发布点、打标签 | CI 一次 + 语义重量 |

**一句话**：**提交是本地安全网，推送是对外承诺。**（用户 §16 的原文）

### 3.2 规则清单 C1–C11

```
[C1]  每个任务 [P?-??] 完成时：本地提交，标题格式 [P?-??] <标题>。
[C2]  每完成 3 个任务，或完成一个逻辑分组（如 [P1-A]），推送到 origin。
[C3]  每个阶段结束时：推送 + 打标签 scroll-p<n>。
[C4]  推送前必须通过任务级门禁（§4.1）。未通过禁止推送。
[C5]  禁止把 [RED] 阶段的"仅测试、未实现"状态推送到 main。
        [RED] 证据只作为提交信息的文字记录。
[C6]  每个提交必须满足：cargo check --workspace --all-targets → 0 error
                          cargo test  --workspace --lib           → 全绿
                          依赖方向门禁                              → clean
[C7]  禁止提交：编译不过的中间态 / 测试红着 / 注释掉测试 / 多个不相关任务混在一个提交。
[C8]  阶段结束序列：
        1. 跑阶段级门禁（含 L3 / L4 —— 手工触发）
        2. 把 E-* 实验产出回填 docs/30 对应位置
        3. 本地提交一个 [stage] 收口提交（只改文档，不改代码）
        4. git tag -a scroll-p<n> -m "P<n> <名称>"
        5. git push origin main --tags
[C9]  任何任务 [P?-??] 必须能被单独回滚：
        git revert <hash>   →  不破坏其它任务的测试
        git bisect          →  每个提交都是绿色的
[C10] 回滚不删除历史：
        git revert <hash>            ← 首选，保留历史
        git revert <hash1>..<hash2>  ← 多个连续任务一起回滚
        禁止 git reset --hard / git push --force（除非明确标记为紧急且告知）
[C11] 阻塞任务的处理：
        - 阻塞原因 = OQ / E-*：任务不启动，只在文档里留 [ ]。
        - 做到一半才被阻塞：把已完成的 [RED] 测试放到分支
            blocked/P?-??-<slug>
          提交到该分支并推送到 origin，不合并到 main。
        - 阻塞解除后：从 main 拉 rebase，恢复 [GREEN] 阶段，完成后正常提交。
```

### 3.3 阶段收口提交的内容（**只改文档，不改代码**）

1. 更新本文中本阶段所有任务的 `[x]` 状态；
2. 记录本阶段**实际产出**与 V2 §23.3 的对比（**逐指标**，含"未取得"）；
3. 记录本阶段引入的**偏差**（若与 V2 有偏离，写清偏离点、依据、是否需回填 V2）；
4. 记录推送批次与标签信息（`git log --oneline <last-tag>..HEAD` 的输出摘要）。

### 3.4 阻塞任务的侧分支规范

侧分支命名：`blocked/P?-??-<slug>`（例：`blocked/P3-02-webview2-inject`）。

**本文标注的 `[!]` 阻塞任务**（受 OQ / E-* / 设备阻塞，**代码不允许直接进 `main`**）：

| 任务 | 阻塞原因 | 解除条件 |
|---|---|---|
| `P0.02`（`T-THREAD-1` 的断言） | 断言会在**合法的生产交接路径**上 panic（放大镜取色：`overlay/session.rs:565` → `renderer.rs:311`），而库测试全绿看不到它 | `docs/30 §21.3` 第 ② 步定下 deferred context / 显式交接后并入并重跑两个用例 |
| `P0.05`（`E-CAP-1` 的 WebView2 一格） | OQ-2 / 需要真实 WebView2 宿主 | 装一个 WebView2 宿主或记为"未取得" |
| `P1.22` 的 125%/150% DPI 扫描行 | **OQ-4**：本机 `PixelRatio = 1`，取不到 | 接一台可改缩放的真实显示器 |
| `P3.02` 的 UIPI 组 | **OQ-3**：官方两页互相矛盾 | 以管理员身份跑一次 `E-INJECT-1` |
| `P3.09` 的"非前台 `SendInput`"分支判据 | **OQ-5**：`SPI_GETMOUSEWHEELROUTING` 是用户可改设置 | 在两种设置下各跑一次 |

**不在侧分支上、但允许"先按推导实现"的任务**：`P4.02`（`E-PERF-2` 未定时允许先选 `Compression::Fast` + `Filter::Sub` 并标"启动值，待校准"）。

### 3.5 提交信息模板（**唯一模板，任务里不再重复正文**）

```text
[P?-??] <任务标题>

RED 证据：
  <测试命令>
  → <失败原因：编译失败（符号不存在）/ 断言失败（期望 X，实际 Y）>

GREEN 验证：
  <测试命令>
  → <通过结果：N passed / 0 failed / M ignored>

REFACTOR（如有）：
  <重构内容摘要；若无写"无（GREEN 已是最小形态）">

DoD：
  cargo check --workspace --all-targets  → 0 error
  cargo test  --workspace --lib           → 全绿
  依赖门禁                                → clean

来源：docs/30 §<章节号>
第一性原理：docs/30 §2.1 <F?>
依赖：[P?-??] 已完成
```

### 3.6 回滚演练（**每个阶段至少做一次**）

| 步骤 | 命令 | 判据 |
|---|---|---|
| 1 | 挑本阶段一个**中间**任务提交，`git revert <hash>` | 门禁三条全过（A 类不回归） |
| 2 | `git bisect start <坏> <好>` 定位一个**故意注入**的失败 | `bisect` 能收敛到唯一提交（验证规则 C9 成立） |
| 3 | `git revert <hash1>..<hash2>` 回滚一个分组 | 分组前后的测试状态与提交信息记录一致 |
| 4 | 把演练结果写进本阶段收口提交 | 三个判据都有记录 |

**为什么必须演练**：规则 C9/C10 只有在**真的用过一次**之前都是愿望。回滚演练是"提交纪律合规性"从声称变成事实的唯一方式。

---

## 4. 门禁体系

### 4.1 任务级门禁（**每次推送前**，规则 C4）

```bash
# 基线编译（含全部 target：test/bench/example）
cargo check --workspace --all-targets

# 全量单测（Debug，不需要桌面）
cargo test --workspace --lib

# 依赖方向（含 P6.04 新增的 scroll/ 平台纯度扫描）
pwsh tools/check-dependency-direction.ps1
```

### 4.2 阶段级门禁（**打标签前手工跑**，规则 C8 第 1 步）

```bash
# 真实桌面用例（L3）。--test-threads=1 是必需的，不是偏好：
# 这些用例会抢前台窗口（scroll_probe.rs:404 的 bring_to_front 断言），并行跑会互相打断。
# 2026-10-08 实测：并行 → 6 passed / 1 FAILED（inject_probe，前台断言失败）；串行 → 全绿（29.18s）。
cargo test -p snapclip-capture --lib -- --ignored --test-threads=1

# 性能用例（L4：Release + 稳定机器 + 每场景独立进程）
cargo test -p snapclip-capture --release --lib -- --ignored perf

# UI 回读用例（shell 侧）
cargo test -p snapclip-app --features test-support --test ui

# 依赖门禁（含第二遍扫描）
pwsh tools/check-dependency-direction.ps1
```

### 4.3 Pre-push 钩子（**全文**；P0.08 负责创建与安装）

`.githooks/pre-push`：

```sh
#!/bin/sh
# SnapClip: push-level gate. See docs/31-scroll-capture-tdd-tasklist.md §4.1/§4.3
set -e
echo "[pre-push] cargo check --workspace --all-targets"
cargo check --workspace --all-targets
echo "[pre-push] cargo test --workspace --lib"
cargo test --workspace --lib
echo "[pre-push] dependency direction"
powershell -NoProfile -File tools/check-dependency-direction.ps1
echo "[pre-push] OK"
```

**安装**（Windows 上钩子文件必须能被 `sh` 执行；Git for Windows 自带 `sh`）：

```bash
git config core.hooksPath .githooks
```

**验证生效**（P0.08 的退出条件，**必须真的做一次**）：故意让一个测试失败，`git push` 必须**被拒绝**；恢复后 `git push` 通过。**只配置不验证等于没有配置**（与 D-14 的教训同源：不可见 == 不存在）。

### 4.4 门禁分层总结

| 层级 | 触发时机 | 包含内容 | 环境 | 阻塞什么 |
|---|---|---|---|---|
| **提交级** | 每次 `git commit` 前（人工） | `cargo check` + `cargo test --lib` + 依赖门禁 | 本地 | 提交 |
| **推送级** | 每次 `git push` 前（钩子自动） | 同上 | 本地 | 推送 |
| **阶段级** | 打标签前（手工） | 全部 + L3/L4 + UI 回读 | 本地/真实桌面 | 标签 |
| **全量回归** | 阶段收口 | 全部 + `E-*` 回填 | 本地 | 进入下一阶段 |

### 4.5 为什么 L3/L4 **不**进任务级门禁

| 理由 | 说明 |
|---|---|
| L3 需要真实交互桌面 | CI/无桌面环境必然失败 → 若放进提交级，就等于**强迫**把 L3 写成静默跳过（正是 D-14 的成因） |
| L4 需要 `Release` + 稳定机器 + 独立进程 | 单次耗时与噪音都不适合每个提交 |
| 分层的**代价**必须补上 | 所以 **P6.02 必须写"非 `#[ignore]` 的真实桌面用例数 == 0"的统计脚本**——否则"L3 不在提交级"会退化为"L3 不存在" |

---

## 5. 调研任务与产出

### 5.1 外部网页调研（`RES-1`…`RES-5`）

**调研纪律**（用户 §14）：每次调研必须回答 **我们不知道什么 → 需要研究什么 → 发现了什么 → 这个发现是否适用于 SnapClip → 是否改变设计**。**禁止为了"看起来研究很多"堆链接。**

| 任务 | 问题（我们不知道什么） | 调研对象（优先级：官方文档 > 官方源码 > 高质量实现 > 论文） | 产出 | 影响 | 前置 |
|---|---|---|---|---|---|
| `RES-1` | `WM_MOUSEWHEEL` 的 `lParam` 到底是屏幕坐标还是客户区坐标？多显示器下 `LOWORD/HIWORD` 为何不能用？ | MS Learn `mouseinput`/`wm-mousewheel`；`windows-rs` 的 `WM_MOUSEWHEEL` 文档 | `docs/Temp/research-wm-wheel-coords.md` | **P3.02**（两条路径共用 `make_lparam`）；**已有一轮结果**（V2 §6.4 A1/A2），本任务只补齐"多显示器"一格 | 无 |
| `RES-2` | Chromium 忽略跨进程投递的滚轮吗？`GetMessageTime()` 的横向误判如何触发？ | Chromium 源码（`hwnd_message_handler.cc` 等，只读 web 上的官方仓库文件） | 同上格式 | **P3.02** 的子窗口下沉与时间戳策略；**本机已实测"不忽略"**（V2 §24.6.1），本任务只需把"为什么"落到源码行 | 无 |
| `RES-3` | PNG 编码器的轴长/像素上限与流式能力（`png` crate 的 `Limits`）？ | `png` crate 官方 docs/源码；libpng 的限制 | `docs/Temp/research-png-limits.md` | **P4.02**（选参数）与 **P1.22**（导出后解码比对的 `Limits{bytes}` 收紧规则） | 无 |
| `RES-4` | 图像配准的原始方法（ZNCC、相位相关、ORB）的**失效条件**是什么？ | 原始论文 + 官方实现（OpenCV `matchTemplate`/`phaseCorrelate`/`ORB` 文档） | `docs/Temp/research-matching-failure-modes.md` | **P1.04/P1.06–P1.09/P1.20**；V2 §15.5 已否决"相位相关优先"，本任务只为**四门与 P1 的门限**提供外部依据 | 无 |
| `RES-5` | 125%/150% 缩放下 Chromium 的滚动偏移语义（是否物理整数）？ | Chromium 官方渲染/滚动文档、`devicePixelRatio` 语义、W3C CSSOM View | `docs/Temp/research-fractional-scroll.md` | **P1.22** 的 DPI 扫描行、**OQ-4** 的收敛；若结论是"可能非整数"，则触发 V2 §16.1 门一的重新评估 | 无 |

### 5.2 参考项目本地调研（`RES-6`…`RES-9`）

**要求**（用户 §16 / V2 §0.1）：**不能只看 README**；必须回答 **"项目 X 为什么采用 Y？SnapClip 是否具有相同约束？"**；**禁止照搬**。

| 任务 | 目标 | 必须读到的位置 | 产出 | 影响 |
|---|---|---|---|---|
| `RES-6` | 参考实现的**参数来源**：为什么 `MIN_INLIER_TILES = 4`、`MIN_RESIDUAL_GAIN = 0.15`、`LOWE_RATIO = 0.8`、`tile_size = 32`？ | `refer/snow-apps/snow-crates/crates/snow-stitch-images/src/estimator.rs:12-20`、`:632`、`:1248-1252`；`region.rs:779-788` | 参数来源表（值 / 出处行 / 是否有推导 / SnapClip 是否同约束） | **P1.06–P1.12** 的启动值；**若查不出推导**则 SnapClip 标"启动值，待校准"并在 `E-ACC-1` 里扫 |
| `RES-7` | 参考实现的**测试夹具为什么是程序生成的合成图**（而不是录制帧）？它的盲区是什么？ | `refer/snow-apps/snow_shot/tests/scrolling_image_replay*.{h,cpp}`；`snow-stitch-images/examples/scroll_4_benchmark.rs:235-268` | 夹具方法论笔记 + "它的盲区清单" | **P1.21** 的夹具设计（V2 §29.3 已采纳米尺度结构 + 逐行相等判据）；**并解释为什么 V2 的判据比"基线图片比对"更强** |
| `RES-8` | 参考实现的**注入实现**为何先 `ScreenToClient` 再 `ChildWindowFromPointEx` 下沉？ | `refer/snow-apps/snow_shot/.../scrollinput.cpp:52-54`（及 `SCROLLING_DIAGNOSTICS.md:61-72` 的 5 类 status） | 注入路径对照笔记 | **P3.02/P3.03**；**本机实测已确认有效**（V2 §24.6.1），本任务补"为什么有效" |
| `RES-9` | 参考实现的 **tile/画布**实现为何是"位图 tile + 编码 tile"两套？它的上限在哪？ | `snow-stitch-images/src/tiled_canvas.rs:7-18`（`CANVAS_TILE_SPAN=256`、`MAX_SPARE_TILES=2`、`tile_capacity_limit`）；`snow-stitch-images-c` 的导出线程 | 画布存储对照笔记 | **P1.14/P1.15**；**不照搬**：V2 的 `BandStore` 是**整宽条带 + 磁盘换出**（参考实现无换出、无预算），差异必须写进笔记 |

### 5.3 调研产出格式（**唯一格式**）

```markdown
# <主题>
## 1. 我们不知道什么
## 2. 调研对象与来源分级   （官方文档 / 官方源码 / 高质量实现 / 论文 / 社区）
## 3. 发现了什么            （每条附出处：URL 或 repo:path:line）
## 4. 是否适用于 SnapClip   （约束对比：参考项目的约束 == 我们的约束？）
## 5. 是否改变设计          （改变了 → 指向具体任务号；没改变 → 写"不改变，理由是…"）
## 6. 未取得的资料          （如实写"未取得"，不编造）
```

**存放位置**：`docs/Temp/`（**被 `.gitignore:38` 忽略**，不入库）。**只有"影响设计"的结论才回填 `docs/30`**（V2 §32 的研究笔记表）。

### 5.4 调研结果对任务设计的影响（登记表，**随调研逐步填写**）

| 调研 | 结论 | 是否改变设计 | 影响的 V2 章节 | 影响的任务 |
|---|---|---|---|---|
| `RES-1` | （待填） | | §24.6 | P3.02 |
| `RES-2` | 本机实测：两条传输都驱动 Chromium（800 px/8 notch） | **不改变**（两条并列路径保留） | §24.6/§24.6.1 | P3.01/P3.02 形状不变 |
| `RES-3` | （待填） | | §17.7 | P4.02 |
| `RES-4` | （待填） | | §15.3/§16 | P1.04/P1.06–P1.09 |
| `RES-5` | 待取得（需 125%/150% 显示器） | （可能改变门一） | §16.1 | P1.22 的 DPI 行 |
| `RES-6` | （待填） | | §16.3/§16.4/§16.7 | P1.06–P1.12 |
| `RES-7` | （待填） | | §29.3 | P1.21 |
| `RES-8` | （待填） | | §24.6 | P3.02 |
| `RES-9` | （待填） | | §17.5 | P1.14/P1.15 |

### 5.5 未调研项（**如实列出**，用户 §14 要求）

| 未调研项 | 原因 | 影响 | 何时补 |
|---|---|---|---|
| macOS/Linux 的滚动截图行为 | 本项目目标平台只有 Windows（V2 §28.3） | 无 | 不补 |
| 浏览器扩展/CDP 全页截图路径 | **已明确非目标**（V2 §8 N1） | 无 | 不补 |
| 深度学习配准/超分 | **已明确非目标**（V2 §8 N4） | 无 | 不补 |
| 多显示器混合 DPI 的实测 | 本机单屏 `PixelRatio = 1`（OQ-4） | P1.22 的一行被阻塞 | 有设备时 |
| WebView2 宿主的注入行为 | 本机无 WebView2 宿主可测 | P0.05 一格、P3.01 一组 | 有宿主时 |
| `model\*.bin` 之类的竞品私有格式 | 与滚动截图无关 | 无 | 不补 |

---

## 6. P0 前置实验与基线

**本阶段的性质**：**没有一行产品代码**。它把 V2 §23.3 里所有标"待测"的格子变成数字，把"基线可信"从传说变成实测，把"推送级门禁"从愿望变成钩子。**V2 §35 的总纪律是"实验先于数字"——P0 就是这句话本身。**

**P0 的执行顺序**（V2 §35 的裁决）：`P0.06` ✅ 已完成 → **`P0.05` → `P0.07` → `P0.09` → `P0.01` ✅/`P0.02` → `P0.03`/`P0.04` → `P0.08`**。
理由：`P0.06` 是这批实验里**唯一可能推翻现有方案**的一个（若 `Chrome + PostMessageW` 失败，§24.6 与 P3 的接口形状都要重评）。它已经跑完且结果为"两条路径都成立"，所以后续任务的接口形状**不变**。`P0.01`/`P0.02` 随时可插入。

**本阶段所有任务都是 `[P0-A]` 批次**；阶段结束时打标签 `scroll-p0`（规则 C3/C8）。

### P0.01 真实基线：把"475/9/0 的传说"变成实测

| 字段 | 内容 |
|---|---|
| 上游依据 | V2 §35 P0.1；V2 §0.2；`docs/19` 的"475 passed / 9 ignored / 0 failed" |
| 第一性原理 | **F-07**（内存/资源的上界必须可量化）——把一个被反复引用却无人复现的数字变成**可复现的事实**，是后面一切"没有回归"声明的前提 |
| 前置 | 无 |
| 可并行 | 是（与 P0.03–P0.07 全并行；它是唯一不改代码的实验） |
| 推送批次 | `[P0-A]` |
| 测试层级 / 分类 | L1 命令级 / A（基线） |
| **RED** | 新写 `scripts/record-baseline.ps1`（**新增脚本**，`scripts/` 今日已存在且未入库）先输出"unmeasured"：`cargo test --workspace --lib -- --list \| Measure-Object` 与 §0.3 的记录**不一致**时脚本报红。**预期失败原因**：仓库里**没有任何基线产物**（`Select-String -Pattern '475 passed' -Path docs` 只命中 `docs/19`/`docs/24` 的正文，无产物文件）→ 脚本的比对目标不存在 |
| **GREEN** | 跑三条命令并把四个 crate 的 `passed/failed/ignored` 写入 `docs/31 §0.3` + `docs/Temp/baseline-<date>.json` |
| **REFACTOR** | 把与 `docs/19` 的差值**逐条定位到具体测试**（本次已定位到 `scroll_probe.rs` 的 2+1 个）；把该定位规则写进 §2.6 的"基线漂移记账规则" |
| 退出条件 | ① 四个 crate 的数字都有记录；② 与 `docs/19` 的任何差值都能**指名到测试**；③ `cargo check --workspace --all-targets` 的 warning 数与其位置有记录 |
| 提交信息标题 | `[P0-01] the baseline is measured, and every delta has a name` |
| 复杂度 / 阻塞 | S / 无 |
| **状态** | **[x] 已完成**（2026-10-08）：477 passed / 10 ignored / 0 failed（app 56+3、capture 347+7、history 51、model 23）；`+3` 全部来自 `scroll_probe.rs`（提交 `c15e614`）；`cargo check` = 0 error / 1 warning（`unused variable: content_label`，`apps/snapclip/src/history/view.rs`） |

### P0.02 `T-THREAD-1`：给 `context()` 加所有者线程断言，看今天会不会 panic

| 字段 | 内容 |
|---|---|
| 上游依据 | V2 §21.3 第 ① 步；V2 §33.2 **R-6**；V2 §34.1 G10 |
| 第一性原理 | **F-01**（GPU 资源的访问必须串行化）+ **推论 2.11**：现在的不变式"GPU 线程是 immediate context 的唯一使用者"**与代码不符**（4 个调用点、唯一互斥是一个布尔位）→ **假的不变式比没有更危险** |
| 前置 | 无 |
| 可并行 | 是（与 P0.01/P0.03–P0.07 并行） |
| 推送批次 | `[P0-A]` |
| 测试层级 / 分类 | L1（负面用例，必须 panic）/ D |
| **RED** | 在 `crates/snapclip-capture/src/windows/win/d3d11.rs` 的 `#[cfg(test)] mod tests` 加 `using_the_immediate_context_from_a_second_thread_panics()`：`std::thread::spawn` 里调 `device.context()`（或等价访问器）并断言 `should_panic`。**预期失败原因**：今天**没有**所有者断言 → 不会 panic → 用例红 |
| **GREEN** | 在 context 访问器里加 `assert_eq!(std::thread::current().id(), self.context_owner, "ID3D11DeviceContext used from a non-owner thread")`；保留 `graphics_released` 布尔位**原样不动**（V2 §33.5 明令） |
| **REFACTOR** | 用 `#[cfg(debug_assertions)]` 与否的取舍写成一行注释（依据：断言成本 vs 发布期风险）；**不做**任何线程迁移（那是 P0.02 之后按 §21.3 第 ② 步决定的） |
| 退出条件 | ① 负面用例通过（今天会 panic）；② `cargo test --workspace --lib` 全绿 → **回答 `T-THREAD-1`**：今天是否已在跨线程使用 context；③ 结论写进 `docs/30 §21.3` 与本文 §14 |
| 提交信息标题 | `[P0-02] the context owner is asserted instead of assumed` |
| 复杂度 / 阻塞 | S / 无（**若全量测试今天 panic**，则升级为 `[!]` 并触发 §21.3 第 ② 步的 deferred context 评估） |
| **风险** | 若断言在发布期为真但仍被触发，会变成**崩溃**而不是降级。缓解：断言只在 `debug_assertions` 下生效，发布期保留 `graphics_released` 语义，并把"发布期如何保证"作为 `P0.02` 的第二个交付物写进 `docs/30 §21.3` |
| **状态** | **[!] 阻塞**（2026-10-08）：断言与两个用例已实现、已实测（RED → GREEN → 全量 `478 passed / 10 ignored / 0 failed`），但**不进 `main`**；代码在侧分支 `blocked/P0-02-context-owner`（提交 `d1098cb`，已推 origin）。结论已回填 `docs/30 §21.3`。 |

**`P0.02` 的实测结论与两处偏离（2026-10-08）**

1. **RED**：`using_the_immediate_context_from_a_second_thread_panics` 在加断言前是红的 —— `cargo test -p snapclip-capture --lib using_the_immediate_context_from_a_second_thread_panics -- --nocapture` → `panicked at crates\snapclip-capture\src\windows\win\d3d11.rs:853:9: a second thread used the immediate context without panicking: the guard is not wired to the accessor`（0 passed; 1 failed; 354 filtered out）。**GREEN**：同一命令 → 1 passed，子线程 panic 文案 `GraphicsDevice::context ran on a thread that does not own the immediate context; one ID3D11DeviceContext has exactly one user thread (docs/30 §21.3)`；全量 `cargo test --workspace --lib` → **478 passed / 10 ignored / 0 failed**（capture 348+7，`+1` 即本用例）。
2. **`T-THREAD-1` 的答案是"两句话"**：**库测试回答不了**（每个测试都在同一线程创建设备并使用它，所以全绿）；**但生产路径真的会 panic** —— 设备在 capture worker 线程创建（`capture_worker.rs:373` → `providers.rs:292-297`），经 `FrozenFrame::device()`（`providers.rs:67-75`）交到 overlay 线程（`overlay/window_restore.rs:22-27`），overlay 线程在放大镜取色时 `submit`（`overlay/session.rs:565` → `renderer.rs:311`）。这条链已用第二个（`#[ignore]` 的）用例 `the_production_hand_off_trips_the_context_guard` 固定为可执行证据：设备在 A 线程创建、B 线程 `submit` 必然 panic。
3. **偏离 1（触发条款）**：本节写的升级条件是"**若全量测试今天 panic**"。实测**没有**触发这一条（全量测试全绿），触发的是它的**实质**（"假的不变式"这个根因）。因此按 §2.7 的第一性原理自查升级为 `[!]`，并把"库测试全绿 ≠ 不变式成立"这一修正写进 `docs/30 §21.3`。
4. **偏离 2（侧分支名）**：本节 §14.2 原定侧分支名是 `spike/deferred-context`，实际按 §3.4 的命名规范建的是 `blocked/P0-02-context-owner`。两者不冲突、各有用途：**`blocked/P0-02-context-owner` 装被阻塞的断言代码**（本次），**`spike/deferred-context` 留给第 ② 步的实验**（尚未开始）。
5. **发布期如何保证（本任务要求的第二个交付物）**：断言只在 `debug_assertions` 下存在；发布期仍然只有 `overlay.rs:507 graphics_released` 这个布尔位。结论已写入 `docs/30 §21.3`：**第 ② 步要做的是"消除跨线程使用"，而不是"让布尔位更可靠"**。

### P0.03 `E-PERF-1`：位移估计四层组合的 P50/P95/Max

| 字段 | 内容 |
|---|---|
| 上游依据 | V2 §23.1 `E-PERF-1`；V2 §23.3 的 Stitch Latency / CPU 两行 |
| 第一性原理 | **F-08**（**我们没有任何本项目的匹配耗时数据**）→ 没有它，任何"要不要优化"的决定都是直觉（AGENTS.md 第 6 条禁止） |
| 前置 | **P1.02/P1.03/P1.04/P1.05**（需要被测的 `estimate()`；若尚未完成，本任务先只测"第 1 层雏形"并把结论标为"不完整"） |
| 可并行 | 是（组 **G1**；接口是 `fn estimate(...)` 的签名） |
| 推送批次 | `[P0-A]` |
| 测试层级 / 分类 | L4（独立进程，`Release`）/ E |
| **实验目的** | 在目标硬件上一次位移估计到底多少毫秒；四层组合各占多少 |
| **实验装置** | 新写 `crates/snapclip-capture/src/scroll/perf_probe.rs`（`#[cfg(test)]`）+ `docs/Temp/perf1-<date>.json`；**用 P1.01 的 testkit 合成序列**（已知真值）；`--release`；**每场景独立进程** |
| **测量方式** | 四个组合：①仅第 1 层 ②第 1+2 层 ③第 1+2+3 层 ④+ORB。三档视口（1080p / 1440p / 4K）× 每档 1000 步；记录 P50/P95/Max + 每层耗时占比；同时读 `CountingAllocator` 的 live/peak |
| **产物格式** | JSON Lines（`{"combo":..,"viewport":..,"p50_us":..,"p95_us":..,"max_us":..,"layers":{...}}`）+ 一段人读结论 |
| **RED** | 先写**装置自证**用例 `the_probe_reports_zero_for_an_empty_sequence()` + `the_probe_reproduces_a_known_synthetic_step_count()`（合成序列长度已知 → 步数必须相等）。**预期失败原因**：装置未实现 → 编译失败 |
| **GREEN** | 实现测量装置并跑出四组 × 三档数据 |
| **REFACTOR** | 结论回填 `docs/30 §23.3` 的 Stitch Latency 与 CPU 两行；若某层占比 >60%，把"是否需要第 4 层/预降采样"作为结论写进 `docs/30 §36.2（OQ-8）` |
| 退出条件 | ① 12 组（4 组合 × 3 视口）都有 P50/P95/Max；② `docs/30 §23.3` 的 `待测` 被替换或明确标注"未取得+原因"；③ 记录 lockfile 哈希与二进制哈希（§23.5 的纪律） |
| 提交信息标题 | `[P0-03] the matching cost is measured before anyone optimizes it` |
| 复杂度 / 阻塞 | L / 无 |
| **对后续阶段的影响** | Stitch Latency 的目标/阈值最终值；**§15.4 是否需要第四层**；`P1.03–P1.05` 的实现方向（若第 2 层是瓶颈，优先优化降采样而非 ZNCC 公式） |

### P0.04 `E-PERF-2`：PNG 流式 12 组参数

| 字段 | 内容 |
|---|---|
| 上游依据 | V2 §23.1 `E-PERF-2`；V2 §17.7（流式导出必须显式设置压缩/滤波参数） |
| 第一性原理 | **F-12**（PNG 的 `height` 必须在写 IHDR 前确定 → 只能流式）+ **F-08**（无数据）→ 参数必须测出来 |
| 前置 | **P4.01/P4.02**（需要 trait 与 shell 侧实现）；若未完成，先在 `docs/Temp/` 用一次性脚本对同一张合成图做等价测量并标明"非端口路径" |
| 可并行 | 是（组 **G1**） |
| 推送批次 | `[P0-A]` |
| 测试层级 / 分类 | L4 / E |
| **实验目的** | `Compression::{Fast,Balanced,High} × Filter::{NoFilter,Sub,Up,Adaptive}` = 12 组，哪组最快且体积可接受 |
| **实验装置** | 30,000 px 高合成图（P1.01 的 testkit）；`docs/Temp/perf2-<date>.json`；独立进程 |
| **测量方式** | 每组的编码耗时（MB/s）、输出体积、峰值内存（`CountingAllocator`）；**并验证产物能被解码回读**（含"必须显式提高 `png::Limits{bytes}`"这一条，见 §13.5） |
| **RED** | `the_encoder_streams_rows_without_materializing_the_image()`：断言编码期间 `peak - live` 不超过"一份条带"的量级。**预期失败原因**：装置未实现 |
| **GREEN** | 12 组各跑一遍，产出选定参数 |
| **REFACTOR** | 把选定参数写进 V2 §17.7 与本文 `P4.02`；若最优组需要增大 `Limits{bytes}`，把该值一并记录 |
| 退出条件 | ① 12 组数据齐全；② 选定的一组有**两条**依据（速度 + 体积）；③ 回读验证通过（**这一条同时验证了 F-10 的 `Limits` 陷阱**） |
| 提交信息标题 | `[P0-04] the png parameters are chosen from twelve measurements` |
| 复杂度 / 阻塞 | M / 无 |
| **对后续阶段的影响** | `P4.02` 的默认参数；`docs/30 §17.7` 的参数表；导出时间的量级 |

### P0.05 `E-CAP-1`（最小版）：窗口级 WGC 对五类目标各取 10 帧

| 字段 | 内容 |
|---|---|
| 上游依据 | V2 §35 P0.5；V2 §24.2；V2 §24.8 验证表 |
| 第一性原理 | **F-09/F-10**（窗口级捕获是"覆盖层不必隐藏"与"遮挡下仍取目标内容"的**唯一**机制）+ **F-20**（窗口级捕获天然不含其它顶层窗口） |
| 前置 | 无（可与 P2.02 并行做，P2.02 是它的产品化形态） |
| 可并行 | 是（组 **G1**） |
| 推送批次 | `[P0-A]` |
| 测试层级 / 分类 | L3（`#[ignore]`，真实桌面）/ B + D |
| **实验目的** | `CreateForWindow` + `CreateFreeThreaded(..., bufferCount = 3)` 对 **Chrome / Edge / Electron（若可取得）/ WebView2（若可取得）/ 记事本** 是否都可用；**pool 是否能在会话内复用** |
| **实验装置** | 扩 `crates/snapclip-capture/src/windows/scroll_probe.rs`（**同一装置，不新建文件**）新增 `capture_probe` 臂；每类目标 10 帧，记录黑帧比例与首帧耗时 |
| **测量方式** | 每帧：`FrameArrived` 到像素可读的耗时、是否非黑（用方差阈值而不是"看起来有内容"）、pool `Recreate` 次数 |
| **RED** | 装置自证：`the_capture_probe_detects_a_black_frame()`（对全黑合成帧必须判黑）+ `an_unknown_window_handle_reports_unavailable()`（对无效句柄必须返回"不可用"而不是 panic）。**预期失败原因**：装置未实现 |
| **GREEN** | 五类目标各 10 帧；若某类不可用，**记录不可用的具体错误码**（`E_INVALIDARG`/`CreateForWindow` 失败等），并把"未取得"写清楚 |
| **REFACTOR** | 结论回填 `docs/30 §24.2` 与 §24.8；**若发现"传子窗口 HWND 不可用"或"最小化时不可用"**，这两条今天是**官方依据缺口**（V2 §6.4 B9），必须写成 `docs/30 §36.2` 的开放项并影响 `P2.01`（`ScrollTarget` 的句柄必须是顶层窗口） |
| 退出条件 | ① 五类目标各 10 帧的结果表；② pool `Recreate` 次数（除尺寸变化外应为 0）；③ 不可用的类**有错误码**，不是"失败了" |
| 提交信息标题 | `[P0-05] the window-level capture path is measured on five targets` |
| 复杂度 / 阻塞 | L / **[!]** WebView2 那一格受设备阻塞（§3.4）→ 侧分支 `blocked/P0-05-webview2-capture` |
| **对后续阶段的影响** | `P2.01`（`ScrollTarget` 句柄语义）、`P2.02`、`P2.03`（pool 复用是否可行）、`P2.04`（能力探测要探什么） |

### P0.06 `E-INJECT-1`（最小版）：两条传输 × Chrome ✅

| 字段 | 内容 |
|---|---|
| 上游依据 | V2 §24.6、§24.6.1（实测记录）、§24.6.2（仍开放的部分） |
| 第一性原理 | **F-14**（Chromium 是否忽略跨进程投递的滚轮，**只有源码旁证、没有官方依据**） |
| 前置 | 无 |
| 可并行 | 是（组 **G1**） |
| 推送批次 | `[P0-A]` |
| 测试层级 / 分类 | L3（`#[ignore]`）/ B |
| 退出条件 | 4 组是否都产生位移；**装置自证通过** |
| 提交信息标题 | `[P0-06] the injection decision is measured instead of assumed` |
| 复杂度 / 阻塞 | M / 无 |
| **状态** | **[x] 已完成**（2026-10-08，提交 `c15e614`）。结论：**两条传输都驱动 Chromium**（`SendInput` 800 px、`PostMessageW` 800 px，各 8 notch 逐 notch 累加）；`lParam` 坐标空间在本机两种写法都得 800 px（**几何巧合，不得据此认为两空间可互换**；实现采用 MSDN 的**屏幕坐标**）；`EDIT` 控件类对注入滚轮与 `WM_VSCROLL` **都不响应** → 夹具必须自建窗口（否则会得到"注入无效"的假结论）；**一次连发 8 notch 在第一次运行里是"测量不到"而不是"没滚动"** → 产品必须**逐步**估计位移，且 1D 行指纹**绝不能作为判据**（V2 §15.2/§16.10）。装置 = `crates/snapclip-capture/src/windows/scroll_probe.rs`（`#[cfg(test)]`），命令 = `cargo test -p snapclip-capture --lib inject_probe -- --ignored --nocapture` |

### P0.09 `E-INJECT-1`（补齐）：五类目标 × 两条传输 + UIPI + 路由设置 + 小窗口坐标空间

| 字段 | 内容 |
|---|---|
| 上游依据 | V2 §24.6.2（**仍开放的四项**）；V2 §25.5；V2 §30.5 的浏览器行 |
| 第一性原理 | **F-14 + F-15**（UIPI 单向、`PostMessage` 绕过 UIPI）→ 这两条决定了 P3 的 `choose()` 判据是否成立 |
| 前置 | **P0.06** ✅ |
| 可并行 | 是（组 **G1**，但必须与 P0.06 串行，因为共用装置） |
| 推送批次 | `[P0-A]` |
| 测试层级 / 分类 | L3 / B + D |
| **实验目的** | ① Edge / Electron / WebView2 / WinUI3 各 1 组（两条传输）；② **提权目标**（管理员记事本）两条路径各 1 组；③ `SPI_GETMOUSEWHEELROUTING = MOUSE_POS` 时非前台目标的 `SendInput` 是否生效；④ **小窗口**（客户区小于目标落点）下"客户端坐标 vs 屏幕坐标"是否仍等价 |
| **实验装置** | 复用 `scroll_probe.rs` 的 `Transport`/`run_arm_stepwise`/WheelFixture；**WinUI3 若不可得就写"未取得"**（不编造） |
| **测量方式** | 每臂逐 notch 累加位移（**不得连发**，P0.06 的教训）；记录 `InjectStatus` + 到达计数 + `GetMessageTime` 是否被同时间戳影响 |
| **RED** | `the_probe_distinguishes_rejected_from_unreached()`：构造"消息入队但目标不消费"的窗口（已有 WheelFixture 的计数机制）→ 断言装置能区分这两种状态。**预期失败原因**：装置未实现该臂 |
| **GREEN** | 跑完全部臂并记录 |
| **REFACTOR** | 结论回填 `docs/30 §24.6.2`（逐项标"已答/未取得"）；**若小窗口下两种坐标空间不等价**，把 `make_lparam` 的实现要求（必须屏幕坐标）写成 `P3.02` 的硬约束 |
| 退出条件 | ① 五类目标（含 Chrome）都有两条传输的结果；② UIPI 目标的两条路径结果明确；③ 路由设置两种取值各一次；④ 小窗口一格有结论 |
| 提交信息标题 | `[P0-09] the injection matrix is closed on five targets and both integrity levels` |
| 复杂度 / 阻塞 | L / **[!]** 部分受设备阻塞（WebView2 宿主、混合 DPI）→ 侧分支 |
| **对后续阶段的影响** | `P3.01`/`P3.02`（传输实现）、`P3.03`（`choose()` 的四组判据）、`docs/30 §24.5` 的 UIPI 矩阵、`docs/30 §36.2` 的 OQ-2/OQ-3/OQ-5 收口 |

### P0.07 `SPI_GETMOUSEWHEELROUTING` / `SPI_GETWHEELSCROLLLINES` 读取

| 字段 | 内容 |
|---|---|
| 上游依据 | V2 §11.4 的探测表；V2 §12.3；V2 §24.6；V2 §36.2 OQ-5 |
| 第一性原理 | **F-15**（路由设置是**用户可改的系统设置**）→ 不能把它当设计前提，只能当**运行期读数** |
| 前置 | 无 |
| 可并行 | 是（组 **G1**） |
| 推送批次 | `[P0-A]` |
| 测试层级 / 分类 | L1（纯读取 + 默认值映射）/ B |
| **RED** | `system_parameter_reads_return_documented_defaults()`：断言 `SPI_GETWHEELSCROLLLINES` 的返回值 ∈ {0, WHEEL_PAGESCROLL, 1..} 且 `0` 的语义是"不滚"；`SPI_GETMOUSEWHEELROUTING` 返回 {0,1,2}。**预期失败原因**：读取函数未实现 |
| **GREEN** | 在 `windows/scroll_actuator.rs`（或 `windows/` 下的探测模块）实现读取，并把这个值**作为运行期输入**传给 `choose()`，**不在编译期固化** |
| **REFACTOR** | 把"本机实测值"写进 `docs/30 §11.4` 的探测表（**注明这是本机值，不是设计前提**） |
| 退出条件 | ① 两个 `SystemParametersInfoW` 读取可用；② `WHEELSCROLLLINES = 0` 的"不滚"分支有测试（这是**必须被处理**的用户设置）；③ 本机值已记录 |
| 提交信息标题 | `[P0-07] the wheel routing is read at runtime, not assumed` |
| 复杂度 / 阻塞 | S / 无 |
| **对后续阶段的影响** | `P3.03` 的 `choose()` 判据（V2 §24.6 的"非前台 → 必须 `PostMessageW`"是否过严，见 OQ-5） |

### P0.08 推送级门禁的前置：`.githooks/pre-push` 的创建、安装与**验证生效**

| 字段 | 内容 |
|---|---|
| 上游依据 | 本文 §4.3；用户 §9.3；规则 C4/C6 |
| 第一性原理 | 纪律**不可见 == 不存在**（与 D-14 的教训同源）→ "推送即可信"必须是机械事实 |
| 前置 | 无（但必须在第一次推送**之前**完成，否则前几个批次的门禁只能靠人） |
| 可并行 | 是（与 P0.03–P0.07 并行） |
| 推送批次 | `[P0-A]` |
| 测试层级 / 分类 | L1（脚本级）/ B |
| **RED** | 证据：`git config --get core.hooksPath` **unset**、`.githooks/` **不存在**（本文 §0.2 第 8/9 项已实测）→ 写用例/脚本 `scripts/verify-hooks.ps1` 断言"钩子存在且已挂载"。**预期失败原因**：断言失败（今天是 unset） |
| **GREEN** | 创建 `.githooks/pre-push`（§4.3 全文）+ `git config core.hooksPath .githooks`；**然后故意让一个测试失败并 `git push`**，确认推送**被拒绝** |
| **REFACTOR** | 恢复测试，确认 `git push` 通过；把"验证生效"的两步写进 `docs/31 §4.3`（**必须真的做过**） |
| 退出条件 | ① 钩子文件存在且可执行；② `core.hooksPath = .githooks`；③ **一次被拒绝的推送有一次通过的推送**作为证据（写入提交信息） |
| 提交信息标题 | `[P0-08] the push gate is a hook, not a habit` |
| 复杂度 / 阻塞 | S / 无 |
| **风险** | Windows 上钩子脚本的换行符（CRLF）会让 `sh` 报 `bad interpreter`。缓解：钩子文件**必须以 LF 结尾**，并在 `verify-hooks.ps1` 里断言其不含 `\r\n` |

**P0 退出条件（阶段级）**：① 七个实验（P0.01–P0.07、P0.09）都有产物，**未取得的项目逐条列明原因**；② `docs/30 §23.3` 中标"待测"的格子被替换为数字或"未取得"；③ `scroll-p0` 标签已打且 `git push origin main --tags` 成功；④ §3.6 的回滚演练做过一次。

---

## 7. P1 纯逻辑核心（**可完全在 CI 无桌面运行**）

**本阶段的性质**：位移估计、画布、条带存储全部是**纯逻辑**，运行在 `crates/snapclip-capture/src/scroll/` 内，**不接触 Win32、不接触 GPU、不接触窗口**。§15 的 `tools/check-dependency-direction.ps1` 第二遍扫描（§28.4）会**机械保证**这一点。

**写法说明**：P0 的每个任务用长表格（因为实验任务还需要用户 §6.3 的五项：目的/装置/测量方式/产物格式/对后续影响）；**P1–P6 的实现任务用紧凑单行头部 + 五行正文**，字段一个不少，但不再重复解释同一套 TDD 纪律。每个任务的提交信息正文都按 §3.5 的**唯一模板**写，因此任务里只给**标题行**。

**P1 的推送批次**（对应 V2 §35 的逻辑分组）：`[P1-A]` 类型与三层漏斗 · `[P1-B]` 四门与先验 · `[P1-C]` 画布与条带 · `[P1-D]` ORB 与门禁。**每完成 3 个任务或一组即推送**（规则 C2）。

### P1.01 合成夹具生成器（testkit）与其自证

**上游**：V2 §29.3 ｜ **第一性原理**：**F-03/F-04**（"位移未知"是问题的本质；只有**已知真值**的输入才能判定估计器的对错）｜ **前置**：无 ｜ **可并行**：无（它是 P1 其他一切的前置，V2 §11.2"夹具先于算法"）｜ **批次**：`[P1-A]` ｜ **层级/分类**：L1 / B ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`crates/snapclip-capture/src/scroll/testkit.rs`（**DEV-1**：V2 §28.2 的十文件清单没有给夹具位置，本文新增此 test-only 文件并回填 V2 §28.2）内联 `#[cfg(test)]`，写 `the_fixture_reports_the_script_it_was_built_from()`：生成"多尺度结构长图 + 一串视口裁剪"后，断言它能**倒推出每一步的真值位移 `d_true[k]` 与视口区间**。**预期失败原因**：生成器未实现 → 编译失败
- **GREEN**：实现 `TestImage`（**确定性**：无时间、无随机种子之外的熵）与 `ScrollScript`：结构层至少含 **① 大块纯色 ② 单方向条纹（周期 `P`，可参数化）③ 二维棋盘 ④ 平滑渐变 ⑤ 随机噪声块 ⑥ 高频"文字状"块 ⑦ 1px 细线**；脚本层支持 `d` 的正负、重复帧、跳帧、动态区域占比、噪声 `σ`；对外只暴露 `fn take(&mut self, k: usize) -> Observation` 与 `fn truth(&self, k: usize) -> i32`
- **REFACTOR**：**夹具自证**（V2 §29.3 的硬要求）：写 `the_simplest_estimator_recovers_every_scripted_step()` —— 先用**最简单的逐行指纹直通**（不做 ZNCC、不做门限）恢复每一个脚本化位移并断言与真值一致。**这一步是夹具自身可信的唯一证据**；若它失败，说明夹具或坐标约定有问题，**不得继续 P1 的其它任务**
- **退出条件**：① 自证用例通过；② 夹具不依赖 `apps/snapclip`、不依赖真实桌面；③ `cargo test -p snapclip-capture --lib testkit` 在**干净 checkout** 上通过
- **提交标题**：`[P1-01] the fixture proves itself before the algorithm exists`

### P1.02 `Axis` 抽象与 `T-AXIS-1`

**上游**：V2 §13.1、§17.8 ｜ **第一性原理**：**F-02/F-06**（"垂直"与"水平"是**同一个问题的两次实例化**，不是两个问题）→ 写两套算法会产生两套必须同步修正的缺陷 ｜ **前置**：P1.01 ｜ **可并行**：与 P1.03（组 **G3**；接口是 `Axis` 的 `pub(crate)` 形状）｜ **批次**：`[P1-A]` ｜ **层级/分类**：L1 / B ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`crates/snapclip-capture/src/scroll/observation.rs` 内联测试 `the_axis_mapping_is_exhaustive()`：断言 `primary_delta(dx,dy)`、`cross_delta(dx,dy)`、`primary_extent(w,h)` 在**四种组合**下与手工表一致。**预期失败原因**：三个 `const fn` 未定义
- **GREEN**：`pub(crate) enum Axis { Vertical, Horizontal }` + 三个 `const fn`（**唯一允许的轴分叉点**，V2 §17.8）
- **REFACTOR**：**`T-AXIS-1`**：对同一脚本，用垂直轴与"转置后的输入"各跑一次，断言结果**逐行相等**；这条测试同时锁死"`cross` 一栏垂直取 `dx`、水平取 `dy`"这个最容易写反的地方
- **退出条件**：① `T-AXIS-1` 通过；② 全文件内 `Axis::Vertical` 的分支数 ≤ 1（可用 `grep` 计数）
- **提交标题**：`[P1-02] one axis abstraction, one place to get it wrong`

### P1.03 `Observation` 类型与"只读视图"

**上游**：V2 §2.2、§11.1、§27.3 ｜ **第一性原理**：**F-01**（观测是"某一时刻的视口局部图像"）+ **F-05**（帧可能无效、可能重复）｜ **前置**：P1.01 ｜ **可并行**：与 P1.02（G3）｜ **批次**：`[P1-A]` ｜ **层级/分类**：L1 / B ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`observation.rs` 的 `an_observation_keeps_its_geometry_and_ignores_extra_bytes()`：断言 `Observation` 携带 `pixels/region/qpc/size/axis`，且构造时**拒绝** `pixels.len() != w*h*4`。**预期失败原因**：类型未定义
- **GREEN**：实现 `Observation` + `ObservationView<'_>`（`estimate()` 只接受视图，**不接受 `Observation` 所有权**，这样"估计器不能修改观测"是类型层面的）
- **REFACTOR**：把"内部严格 packed、无 stride"的约定写成断言（`row_stride == w*4`），因为 ZNCC 的像素索引依赖它
- **退出条件**：① 类型与视图可用；② 越界/错长构造有负面用例
- **提交标题**：`[P1-03] an observation is read-only by type`

### P1.04 `Displacement` 与 `status` 三态

**上游**：V2 §16.10 ｜ **第一性原理**：**F-03**（位移必须用**证据强弱**分级，而不是"算出来了就是算出来了"）｜ **前置**：P1.03 ｜ **可并行**：与 P1.05（组 **G2**）｜ **批次**：`[P1-A]` ｜ **层级/分类**：L1 / B ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`displacement.rs` 的 `a_displacement_cannot_carry_a_value_in_the_none_state()`：断言 `status` 与 `d` 的组合是**穷举无冗余**的（`None` 不得携带 `d`）。**预期失败原因**：类型未定义
- **GREEN**：`pub(crate) struct Displacement { d: i32, confidence: f32, evidence: Evidence, status: Status }`、`pub(crate) enum Status { Confirmed, Uncertain, None }`；**`Displacement` 是 `pub(crate)`**（V2 §27.1：否则 `confidence` 会变成兼容性承诺，而 §16.11 标它可校准）
- **REFACTOR**：把 §16.10 的 `status → 行为` 映射写成 `fn effect(&self) -> StepEffect`（`Commit` / `Continue` / `Skip`），使"不提交画布 + 继续会话"成为**一处**代码
- **退出条件**：① 三态与行为映射有穷举测试；② 无 `Option`/裸 `i32` 的两义表达
- **提交标题**：`[P1-04] unknown is a first-class answer`

### P1.05 第 1 层：1D 行摘要候选产生器

**上游**：V2 §15.4 ①、§15.6 ｜ **第一性原理**：**推论 2.1**（投影降维会带来孔径问题 → 它**只能产生候选，永远不能作判据**）｜ **前置**：P1.01 ｜ **可并行**：与 P1.06（组 **G2**）｜ **批次**：`[P1-A]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`displacement.rs` 的 `one_d_candidates_include_the_true_shift_on_a_periodic_image()` 与 `one_d_candidates_are_not_treated_as_a_verdict()`：前者断言真值位移**在前 K=8 个候选内**（允许含错），后者断言该层的输出类型**不是** `Displacement`（类型层面禁止越权）。**预期失败原因**：函数未实现
- **GREEN**：`fn candidates_1d(prev: &ObservationView, next: &ObservationView, axis: Axis, window: i32) -> ArrayVec<Candidate, 8>`：每行 64-bit 折叠摘要 → 在 `W_search = max(4, ceil(0.3 · n · ĝ))`（手动模式退化为 `max(8, ceil(0.15 · H_match))`）内做整数位移的 SAD 排名 → **前 K 个极值**
- **REFACTOR**：把"**载波伪峰**"的实测教训写进注释与测试（`scroll_probe.rs` 的实跑证据：文本行高 19px 会在非整数倍行高位移处给出 `corr≈0.6–0.8` 的伪峰）→ 对应一个用例 `a_periodic_line_carrier_does_not_decide_the_step()`
- **退出条件**：① `docs/30 §30.3` 的"周期纹理"两行在**只开第 1 层**时**绝不**给出 `Confirmed`；② 候选数 ≤ 8 且**确定性**（同输入同输出）
- **提交标题**：`[P1-05] the cheap layer may propose but never decide`

### P1.06 第 2 层：1/4 降采样二维条带 ZNCC

**上游**：V2 §15.4 ②、§15.6（灰度公式、降采样倍数）｜ **第一性原理**：**F-02**（位移的正确判据必须来自**二维结构**，因为周期只在一维上成立）｜ **前置**：P1.01、P1.05 ｜ **可并行**：无（它消费 P1.05 的候选）｜ **批次**：`[P1-A]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`the_score_is_invariant_to_brightness_and_contrast()`（整体亮度偏移与对比度缩放不改变排名 → ZNCC 的定义性质）+ `uniformly_flat_input_scores_zero_instead_of_nan()`（**F-02 的失败模式**：`0/0 → NaN` 必须变 0，绝不能参与比较）。**预期失败原因**：未实现
- **GREEN**：灰度 `Y = (77R + 150G + 29B) >> 8`；**4× 面积平均**降采样；`H_match = max(16, H_viewport / 2)`；`W_search` 同 P1.05；对每个候选算条带 ZNCC；用**三点差分**估曲率做排序；`score = 0.60·zncc2d + 0.25·gain + 0.15·coverage`（权重取自参考实现 `estimator.rs:640`，V2 §16.7）
- **REFACTOR**：把灰度与降采样放进 `Scratch`（**P1.13** 的中间缓冲），使**每步只降采样一次**（不是每个候选一次）；用 `E-PERF-1` 的层占比数据确认这一步值得
- **退出条件**：① 亮度/对比度不变性用例通过；② 纯色与纯渐变输入返回 0 而非 NaN（**D 类**）；③ 候选 ≤ 8 且时间可测
- **提交标题**：`[P1-06] the second layer is the first one allowed to argue`

### P1.07 第 3 层：全分辨率 ±1 整数精修

**上游**：V2 §15.4 ③、§16.11 ｜ **第一性原理**：**F-04**（观测是**整数像素**的栅格 → 亚像素重采样没有事实依据，N3）｜ **前置**：P1.06 ｜ **可并行**：无 ｜ **批次**：`[P1-A]` ｜ **层级/分类**：L1 / B ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`refinement_only_moves_the_winner_by_one_pixel()` 与 `the_final_value_is_an_integer()`：断言精修邻域是 `{-1,0,+1}`（**不含亚像素**），且**唯一进入最终 `d` 的数值来自这一层**。**预期失败原因**：未实现
- **GREEN**：在全分辨率上对 `argmax(score)` 的 ±1 邻域重算 ZNCC，取最大者为最终 `d`
- **REFACTOR**：把"三层各自的输出类型"固化（候选集 → 打分候选集 → **单个整数**），使越权不可表达
- **退出条件**：① 精修用例通过；② `d` 的整数性有断言；③ §30.3 的"整数位移 1..40"全绿
- **提交标题**：`[P1-07] the last layer is integer because the evidence is integer`

### P1.08 门一：几何硬约束 `|d| ≤ viewport_extent`（闭区间）

**上游**：V2 §16.2、§16.9 ｜ **第一性原理**：**F-06**（如果位移不小于视口，则两帧**没有重叠**，位移在事实层面不可判定 → 这不是"可验证性门限"而是**正确性约束**）｜ **前置**：P1.07 ｜ **可并行**：与 P1.09（组 **G4**；接口是"门的签名"先冻结）｜ **批次**：`[P1-B]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`the_gate_accepts_exactly_viewport_extent_and_rejects_one_more()`（**闭区间**的两侧各一个用例）+ **边界值禁令** `the_gate_never_returns_half_the_dimension()`（`±N/2`/`±M/2` 直接 `status = None`，F-02 的共同落点）。**预期失败原因**：门未实现
- **GREEN**：实现门一，返回 `GateOutcome::{Pass, Reject(reason)}`；**与"可验证性约束"分开表达**（V2 §16.1：V1 与 `docs/25` R17 把两者混谈过）
- **REFACTOR**：把 `ρ_min = 0.35`（区间 0.30–0.40）作为**独立**的可校准常量暴露，不与门一混淆
- **退出条件**：① 两个边界用例一正一反；② §30.3 的"位移 == 视口 / 位移 > 视口"两行通过；③ `±N/2` 返回 `None`
- **提交标题**：`[P1-08] overlap is a fact, not a preference`

### P1.09 门二：残差增益 `gain ≥ 0.15`（唯一必须通过的硬门）

**上游**：V2 §16.3、§16.7 ｜ **第一性原理**：**F-03**（"找到一个峰"不等于"这个峰比不移动更好" → 必须与**零位移假设**做对照，参考实现 `MIN_RESIDUAL_GAIN = 0.15`）｜ **前置**：P1.07 ｜ **可并行**：与 P1.08（G4）｜ **批次**：`[P1-B]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`a_peak_that_is_no_better_than_standing_still_is_rejected()`（构造"零位移与最优候选几乎同分"的输入 → 必须 `Uncertain`）+ `when_the_winner_is_zero_the_gain_is_undefined_and_the_fingerprint_path_runs()`（`d_best == 0` 时 `gain` 无定义 → 改走行指纹，**不得除零**）。**预期失败原因**：未实现
- **GREEN**：`gain = 1 − RMSE(d_best)/RMSE(d_0)`，`MIN_RESIDUAL_GAIN = 0.15`；`d_best == 0` 走行指纹分支
- **REFACTOR**：把 `RMSE` 的定义（哪一块区域、是否含边界带）写成一个函数并被两处共用（避免"分母用 A 区域、分子用 B 区域"的隐性错误）
- **退出条件**：① 两条用例通过；② §30.3 的"逐行完全相等"行**不进估计器**（在 **P1.18** 的 `Skip` 路径上命中）
- **提交标题**：`[P1-09] a peak must beat standing still`

### P1.10 门三：空间独立支持 `supporter_tiles ≥ 4`

**上游**：V2 §16.4 ｜ **第一性原理**：**F-02**（周期纹理可以在**一个**空间位置成立而在另一个位置不成立 → 支持必须来自**互相独立**的位置）｜ **前置**：P1.09 ｜ **可并行**：与 P1.11（组 **G4**）｜ **批次**：`[P1-B]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`four_tiles_that_touch_each_other_count_as_one()`（tile = 32px，**独立需 `|i−j| ≥ 2`** → 相邻 tile 的支持不得累加）+ `a_single_patch_supporter_is_rejected()`。**预期失败原因**：未实现
- **GREEN**：tile 化支持统计 + `MIN_TILES = 4`；**用"内点占比 ≥ 0.5 的带数"替代参考实现的 `MIN_INLIER_MATCHES = 8`**（V2 §16.4 的口径统一）
- **REFACTOR**：把 tile 网格与 §18.2 的 `region.rs` 风格**时间模型**用的网格**分开命名**（参考实现的 `region.rs:24-30` 是 32px、`tiled_canvas.rs` 是 256px，两者毫无关系 —— 命名混用会让下一个读者以为它们相关）
- **退出条件**：① 两条用例通过；② §30.3 的"低纹理"行（tile 跳过更新）通过
- **提交标题**：`[P1-10] support must be independent, not adjacent`

### P1.11 门四：候选不唯一 `margin ≥ 0.15`

**上游**：V2 §16.5、§16.9 ｜ **第一性原理**：**F-03**（"最好的候选"与"第二好的候选"太接近时，选择本身没有信息量 → V1 缺的就是这条量化门限）｜ **前置**：P1.10 ｜ **可并行**：与 P1.09（G4）｜ **批次**：`[P1-B]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`equal_scoring_modes_report_uncertain()`（合成二维周期棋盘 → 峰族检测必须报 `Uncertain`）+ `a_two_pixel_margin_is_not_enough()`。**预期失败原因**：未实现
- **GREEN**：`margin = (score(best) − score(second)) / score(best)`，`MIN_MARGIN = 0.15`；`|offset|` 小者优先作为 tie-break
- **REFACTOR**：把 §30.3 的"边界值 `±N/2`"与"二维周期"两条 D 类用例挂到这里，并确认它们**不依赖门一**
- **退出条件**：① 两条用例通过；② §30.3 的"二维周期（棋盘）"行报 `Uncertain`
- **提交标题**：`[P1-11] an ambiguous winner is not a winner`

### P1.12 `scene_cut` 检测与 `streak` 行为

**上游**：V2 §16.8 ｜ **第一性原理**：**推论 2.5 + F-10**（"内容重排 → 等待"与"画面换了 → 应当停"是**两种不同事实**，必须能区分；且**单帧失败永不终止会话**）｜ **前置**：P1.11 ｜ **可并行**：无 ｜ **批次**：`[P1-B]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`a_single_rearranged_frame_stays_uncertain_and_the_session_continues()`（`streak = 1–2` → `Uncertain` + 继续）+ `three_consecutive_scene_cuts_decay_the_model_but_do_not_stop()`（`streak ≥ 3` → `decay_toward_neutral(0.05)` + `reset()` + 非阻塞提示 + **仍然继续**）。**预期失败原因**：未实现
- **GREEN**：`scene_cut := (zncc2d(0) < 0.50) && (∀i: alignment_error(d_i) > 0.60)`；`streak` 计数与复位规则
- **REFACTOR**：**明确写死一条**：`scene_cut` **不是** `StopReason`（与 §20.4 的"`MatchFailed` 不是 `StopReason`"同源）；用类型（`SceneCut` 只出现在 `Evidence` 里）而不是注释来保证
- **退出条件**：① 两条用例通过；② §30.3 的"scene cut ×1–2 / ×≥3"两行通过；③ `StopReason` 的变体数 == 11
- **提交标题**：`[P1-12] a changed page is not a failed session`

### P1.13 `Scratch` 中间缓冲与四门 ablation（"若关闭某门不改变结果，该门就是冗余的"）

**上游**：V2 §22.5、§16.12 ｜ **第一性原理**：**F-07**（内存必须可界定）+ **AGENTS.md 第 6 条**（没有依据的抽象/优化必须能证明自己的价值）｜ **前置**：P1.08–P1.12 ｜ **可并行**：无 ｜ **批次**：`[P1-B]` ｜ **层级/分类**：L1 / B + E ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`closing_any_gate_changes_the_error_rate()`：对四门做**顺序关闭**的 ablation，断言每关一门"错误且被判为 `Confirmed`"的比率**上升**。**预期失败原因**：四门未全部实现 → 无法构造 ablation 矩阵
- **GREEN**：实现 `Scratch`（灰度图 / 1/4 降采样 / 前缀和 / 梯度图，**由 driver 线程独占，不做跨线程池化**）与 ablation 运行器；产出四行结果
- **REFACTOR**：**按 ablation 结果删除冗余门**（这是 AGENTS.md 第 6 条的直接执行）；把结论写进 `docs/30 §16.12`
- **退出条件**：① ablation 四行数据在案；② **每一行都能回答"关掉它之后错在哪"**；③ 冗余门被删除或给出保留依据
- **提交标题**：`[P1-13] every gate earns its place or gets deleted`

### P1.14 机制 P1：注入量先验（`E[d_k] = n_k · ĝ`）

**上游**：V2 §16.6（**V1 与参考实现都没有这条**）｜ **第一性原理**：**F-10 的推论**（我们自己决定了注入几格 `n`，并且维护着"每格多少像素"的估计 `ĝ` → **这是唯一能对抗周期歧义的独立证据源**）｜ **前置**：P1.13 ｜ **可并行**：与 P1.15（组 **G5**）｜ **批次**：`[P1-B]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`the_prior_is_soft_and_cannot_reject_on_its_own()`（区间外只降权 ×0.9，**不得丢弃**）+ `a_wrong_prior_converges_instead_of_locking()`（`ĝ` 初值错 2×，**100 步内收敛回真值**）。**预期失败原因**：先验未实现
- **GREEN**：`E[d_k] = n_k·ĝ`、区间 `[n·ĝ(1−κ), n·ĝ(1+κ)]`（`κ = 0.5`）；四条使用规则（只加权不判决 / `n·ĝ < 4px` 时**关闭** / 区间外只降权 / `ĝ` 只在 `Confirmed` 步更新，`ĝ ← 0.7ĝ + 0.3(d_k/n_k)`）
- **REFACTOR**：把"**自锁**"作为一等风险写进注释与用例名 —— 先验的失败模式是"把自己的错误变成下一帧的前提"（这正是 V2 §17.4 删掉 `Synthetic` 参照系的同一个理由）
- **退出条件**：① 自锁用例在 100 步内收敛（**这是 §30.3 的"P1 自锁"行**）；② §30.3 的"周期纹理 `P == |d|` 有 P1 时 `Confirmed` 且正确"通过；③ 先验**关闭**时的行为等价于"没有先验"，可用一个开关证明
- **提交标题**：`[P1-14] the amount we injected is evidence, not an assumption`

### P1.15 手动模式：`n = 0` 的闭环退化与峰族检测

**上游**：V2 §13.4、§16.6（手动模式的那一段）｜ **第一性原理**：**F-01**（手动模式下"用户滚了多少"**不可知**，但"内容动了多少"仍可观测 → 不能因此失去判定能力）｜ **前置**：P1.14 ｜ **可并行**：与 P1.14（G5）｜ **批次**：`[P1-B]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`manual_mode_without_a_prior_reports_uncertain_on_a_periodic_page()`（多个等间距峰 > `0.85·score(best)` ⇒ `Uncertain`）+ `manual_mode_never_guesses_from_the_scrollbar()`（**类型层面**证明它拿不到滚动条/UIA/滚轮钩子的信息）。**预期失败原因**：未实现
- **GREEN**：手动模式退化为"峰族检测 + `Uncertain`"；`n = 0` 时 P1 关闭
- **REFACTOR**：**明确写死"不试图从滚动条/滚轮钩子/UIA 猜用户滚了多少"**（这是 V2 的刻意选择：那些信号要么不可靠、要么需要额外权限；参考实现用低层鼠标钩子，V2 不走那条路）
- **退出条件**：① 两条用例通过；② §30.2 的手动序列在 `E-ACC-1` 夹具下**不产生错误确定**（与 P1.24 联合验证）
- **提交标题**：`[P1-15] manual scrolling keeps the verdicts but loses the prior`

### P1.16 动态内容：tile 三分类 + **乘法**权重 + "歧义不学习"

**上游**：V2 §18.2、§16.7；参考实现 `region.rs:779-788` 原文 ｜ **第一性原理**：**F-05**（页面是动态的，"哪些区域在跟着滚、哪些没动、哪些在动"是**区域级**的持久属性，逐像素 mask 是错层次）｜ **前置**：P1.13 ｜ **可并行**：与 P1.14（G5）｜ **批次**：`[P1-B]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`a_fixed_header_becomes_a_low_weight_region_without_being_excluded()`（**降权而非排除**：排除会让"标题栏占一半面积的页面"出现永远无法覆盖的横带）+ `ambiguous_tiles_are_not_learned_from()`（`direct × compensated ≥ 0.5` 时**跳过更新**）。**预期失败原因**：未实现
- **GREEN**：`RegionModel`：三分类似然 `fixed = direct(1−comp)`、`scrolling = comp(1−direct)`、`dynamic = (1−direct)(1−comp)`，`texture < 0.05` 跳过；权重 `let learned = 1.0 + 1.5*(scrolling − 1/3) − (fixed − 1/3) − (dynamic − 1/3); let influence = (observations/3).clamp(0,1); (1.0 + influence*(learned − 1.0)).clamp(0.1, 2.0)`（**乘法式**，前 3 次观测线性 ramp）；`decay_toward_neutral(0.05)`
- **REFACTOR**：把 `texture`/`direct`/`compensated` 的定义与 P1.06 的 ZNCC **共用同一套函数**（避免两套相似度定义漂移）；`E-DYN-1`（稀疏光流定位动态内容）作为**可选第四层**：**判据是"是否改善错误确定率"，不改善就删除**（`OQ-12`）
- **退出条件**：① 两条用例通过；② §30.5 的"CSS 动画元素（降权，`Confirmed` ≥ 95%）"行所需的度量可产出（真机验证在 P5/P6）
- **提交标题**：`[P1-16] dynamic regions are down-weighted, never excluded`

### P1.17 `RecoveredImage`、`CoverageMap` 与八条不变量

**上游**：V2 §17.1、§17.2、§20.3 ｜ **第一性原理**：**F-07**（覆盖必须是**二维完整**的，否则会产出"看起来正常但缺一条横带"的长图）｜ **前置**：P1.04 ｜ **可并行**：与 P1.20（组 **G6**；`BandStore` 的接口先冻结）｜ **批次**：`[P1-C]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`assert_invariants_fires_on_each_of_the_eight_violations()`：**逐条构造**违反（cross_len 变化 / span_start ≠ 0 / span_end ≠ primary_len / 二维覆盖缺口 / `committed + discarded ≠ step` / 条带重叠 / `∑resident > budget` / context 所有者线程不符）并断言 `assert_invariants()` **必须 panic**。**预期失败原因**：类型与断言未实现
- **GREEN**：`RecoveredImage{ axis, primary_len, cross_len, coverage: CoverageMap, bands: BandStore }`、`CoverageMap{ span_start, span_end, stale_steps }`（**没有"画布位图"这个东西**）
- **REFACTOR**：把"**不写任何无法断言的不变量**"作为规则写进注释（V1 的 `CaptureState::Adjusting` 就是不可断言不变量的产物，D-2）
- **退出条件**：① 八条负面用例全部 panic；② §30.4 的"覆盖不变量"行通过；③ `CaptureState`/`ScrollSession` 的字段与八条不变量**一一对应**（可 `grep` 核对）
- **提交标题**：`[P1-17] eight invariants, eight negative tests`

### P1.18 `band_height` 与"旧像素优先"提交（逐字节）

**上游**：V2 §17.2、§17.3（**V2 与参考实现的唯一刻意分歧**）｜ **第一性原理**：**F-04**（重叠区在两帧里**不是同一个东西** —— 后一帧的重叠部分是**被运动模糊/重绘过的观测**，不是更可信的真相）｜ **前置**：P1.17 ｜ **可并行**：无 ｜ **批次**：`[P1-C]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`only_the_new_rows_are_written()`（两步重叠后，重叠区**逐字节**等于第 1 步的值）+ `band_height_never_drops_below_a_quarter_of_the_extent()`。**预期失败原因**：未实现
- **GREEN**：`band(extent, shift) = max(extent/2, extent/4 + shift)`（**每步动态**）；只写 `[旧 span_end, 新 span_end)`；`Skip` 路径（逐行完全相等 → `Confirmed, d == 0`，**不进估计器**）
- **REFACTOR**：把这条分歧的三条理由写进代码注释（① 重叠区不是同一个东西 ② 行覆盖是漂移写进画布的通路 ③ 写入可审计）；**代价**（"四重确认下的 ±1px 错位被固化"）也写清楚 —— 判据是"罕见且局部 vs 常见且累积"
- **退出条件**：① 逐字节用例通过；② §30.4 的"旧像素优先"与"`band_height` 推导"两行通过；③ `Skip` 路径不进估计器（可用计数器证明）
- **提交标题**：`[P1-18] the older pixels win, on purpose`

### P1.19 双向扩展与 `Contained`

**上游**：V2 §17.1、§17.4；参考实现 `state.rs:19-84` ｜ **第一性原理**：**F-02**（`next_pos = current_pos + signed_delta`：负位移是**合法**事实，不是错误）+ **N3**（不引入 `Synthetic` 参照系 → 参照系**永远是已确认的画布内容**）｜ **前置**：P1.18 ｜ **可并行**：无 ｜ **批次**：`[P1-C]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`scrolling_up_produces_a_prepend_not_a_duplicate()` + `a_small_rollback_that_is_fully_covered_is_contained()`（**回滚完全落在已覆盖区** → 识别为 `Contained`，**不写重复内容**）+ `one_hundred_confirmed_steps_leave_zero_drift()`（最终画布与真值**逐行相等**）。**预期失败原因**：未实现
- **GREEN**：`ViewportState::transition`（`candidate = position − offset`；`< 0` → `Prepend`；`> max_position` → `Append`；否则 `Contained`）；**参照系永远取画布内容**（保留 `previous_raw` 与 `motion_reference` 的**双帧分离**：前者用于读回位移，后者用于参与估计）
- **REFACTOR**：把"**取消 `Synthetic`**"的四条理由写进 ADR-4 的引用注释（含"`Synthetic` 是**用自己的结论做自己的前提**"）
- **退出条件**：① 三条用例通过（③ 是 §30.4 的"参照系无漂移"行）；② §30.4 的"双向扩展""`Contained`"两行通过；③ `grep -c Synthetic src/scroll` == 0
- **提交标题**：`[P1-19] the reference is always the canvas, never our own guess`

### P1.20 `BandStore`：有界 LRU + 磁盘换出 + 校验

**上游**：V2 §17.5、§22.3 ｜ **第一性原理**：**F-07 + G3**（"内存上界与图像长度无关"是 V2 相对 PixPin 单块画布的**结构性优势**，必须由存储层而不是调用方纪律来保证）｜ **前置**：P1.17 ｜ **可并行**：与 P1.17（G6）｜ **批次**：`[P1-C]` ｜ **层级/分类**：L1 / L2 / B + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`a_one_band_budget_still_produces_a_correct_canvas()`（预算注入成 1 个条带 → 反复换出/换回后**逐字节**正确）+ `the_reference_band_and_the_last_two_confirmed_bands_are_never_evicted()` + `a_corrupted_spill_file_is_detected()`（`fnv` 校验失败 → `ErrorCode::CorruptBand`、会话失败但**不清空画布**、可导出 `Partial`）。**预期失败原因**：未实现
- **GREEN**：`BandStore{ resident: LruMap<BandIndex, Arc<Band>>, spilled: BTreeMap<BandIndex, SpillRef>, budget }`、`Band{ rows, first_row, fnv }`、**整宽条带**、默认预算 `视口像素 × 4 × 8`；裸 BGRA 换出到 `docs/Temp/` 同级的会话临时目录
- **REFACTOR**：预算耗尽三步（先换出预览 → 按 LRU 换出画布但**保护参照与最近 2 个** → 仍不足 `MemoryLimit` + `Partial`）落成一处函数；`SpillRef` 的 `Drop` 清理留给 P4.06（与导出路径同一处治理）
- **退出条件**：① 三条用例通过；② §30.4 的"条带换出"行通过；③ **`MemoryBudget{total, resident_canvas, resident_preview}` 的三个字段都有读取点**（不是死配置 —— 参考实现有三个死配置字段，见 S3，**V2 不接受继承**）
- **提交标题**：`[P1-20] memory is bounded by the store, not by discipline`

### P1.21 上限三层 + 导出预算 + `Partial`

**上游**：V2 §17.6（**三层、无硬失败上限**）｜ **第一性原理**：**F-07 的推论 2.6**（"上限"有两种完全不同的东西：**架构上限**与**用户提示阈值**；V1 把 30,000 px 当上限，比竞品实测值小 17 倍，是**取值错误**而不是实现错误）｜ **前置**：P1.20 ｜ **可并行**：无 ｜ **批次**：`[P1-C]` ｜ **层级/分类**：L1 + L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`the_architecture_limit_is_injectable_and_trims_to_a_valid_partial()`（把 `MAX_LONG_IMAGE_PIXELS` 注入成 1/10 → 产出**能被解码回读**的合法 PNG，且是 `Partial`）+ `the_warn_length_is_a_pure_ui_parameter()`（`LONG_IMAGE_WARN_LENGTH` 改变**不改变**任何像素结果）。**预期失败原因**：未实现
- **GREEN**：三层 = `MAX_LONG_IMAGE_PIXELS`（默认 `u32::MAX / 2`，**可注入**）+ `LONG_IMAGE_WARN_LENGTH`（默认 **29,000 px，刻意与 PixPin 对齐**）+ 导出预算；超限 = **裁成连续前缀**并产出可用 `Partial`
- **REFACTOR**：三条不可协商性质写进注释与测试：① **没有任何一层是"失败"** ② **上限必须可注入**（PixPin 的精确总上限至今未定位，**不得反推竞品**）③ 提示阈值对齐是**纯 UI 参数**
- **退出条件**：① 两条用例通过；② §30.4 的"上限三层"行通过；③ `u32` 越界在 `begin` **之前**被拒绝（D-12 的正面用例在这里落地，实现点在 P4.05）
- **提交标题**：`[P1-21] three layers, none of them a wall`

### P1.22 撤销（`undo_last`）

**上游**：V2 §19.6（**PixPin / Snow Shot / ShareX / Snagit 都不提供滚动中撤销** → 这是 V2 的净增量）｜ **第一性原理**：**"旧像素优先"的直接推论**（既然写入是"只追加新行"且**可审计**，那么"撤销一步"就只是回退一个 `span_end` —— 这个能力是**免费的**，不需要额外日志）｜ **前置**：P1.18 ｜ **可并行**：与 P1.21（组 **G7**）｜ **批次**：`[P1-C]` ｜ **层级/分类**：L1 / B ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`undo_returns_to_the_previous_step_and_deletes_later_bands()`（提交 10 步后撤销 → 回到第 9 步状态，`primary_len` 回退，后续条带被删）+ `undo_does_not_touch_the_learned_ĝ()`（撤销**不更新** `ĝ`）。**预期失败原因**：未实现
- **GREEN**：`ScrollSession.undo: Vec<u64>`（每步记录 `span_end`）+ `fn undo_last(&mut self) -> bool`；粒度 = **一步**，UI 只暴露"撤销上一步"（可连按）
- **REFACTOR**：把"**撤销不更新 `ĝ`**"的理由写成一行（`ĝ` 是**物理**属性，撤销是**用户意图**；让用户意图回写物理模型会污染 P1 先验）
- **退出条件**：① 两条用例通过；② §30.4 的"撤销一步"行通过；③ 连续撤销到第 0 步后状态与初始一致（可用 `assert_invariants` 证明）
- **提交标题**：`[P1-22] undo is free because writes are append-only`

### P1.23 自写 ORB（第二意见，永不产生 `d`）

**上游**：V2 §15.4 ④、§16.11、§36.1 **D-1**（**重写，不改编参考实现**：`snow-crates/.../orb.rs` 是 Apache-2.0，可以并入，但 **D-1 明确选择重写**）｜ **第一性原理**：**N7/F-09**（生态里没有可用的 Rust 匹配器/ratio test → 不新增 crate 依赖）+ **推论 2.1**（第二意见的作用是**投票**，不是给出更好的数字）｜ **前置**：P1.06 ｜ **可并行**：与 P1.14–P1.16（组 **G8**）｜ **批次**：`[P1-D]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：XL（约 300–400 行）｜ **阻塞**：无

- **RED**：`ratio_test_and_mutual_nearest_neighbour_are_both_enforced()`（构造"第二近邻几乎一样近"的描述子集 → 必须被 `ratio = 0.8` 拒绝）+ `a_single_frame_with_no_match_yields_no_votes()`（**失败必须降级为"没有投票"而不是错误**）。**预期失败原因**：未实现
- **GREEN**：`scroll/orb.rs`（`#[cfg(test)]` 与主路径同文件）：FAST-9/16 + Harris + 方向 + rBRIEF 256 位 + 每 tile 上限 + 几何递减配额；`BFMatcher`（**Hamming，量化**）+ `ratio` + **互为最近邻** cross-check；输出**布尔投票**：`Agree(d_best)` / `Disagree` / `NoEvidence`
- **REFACTOR**：**触发条件**必须写死（`margin < 0.25` **或** best 落 P1 区间外 **或** 同步 `uncertain ≥ 3`），**平均每步 < 0.2 次**（V2 §15.6）；**不得**成为主路径（`E-PERF-1` 的层占比会证明它太贵）
- **退出条件**：① 两条用例通过；② `cargo tree -p snapclip-capture -e normal` 的包数**不变**（N7 的机械检查）；③ `E-ACC-1` 覆盖"ORB 投票与主候选是否一致"这一维度；④ 触发率可测且 < 0.2 次/步
- **提交标题**：`[P1-23] a second opinion that can only say yes, no, or I don't know`

### P1.24 `E-ACC-1`：合成扫描落成**门禁**（错误确定率必须为 0）

**上游**：V2 §29.3、§30.3、G1 ｜ **第一性原理**：**G1**（唯一零容忍的指标 = "**错误且被判为 `Confirmed`**"的比率）—— 它是"正确性绝对优先于覆盖率"的**唯一可机检形式**｜ **前置**：P1.01–P1.23 ｜ **可并行**：无（本阶段的最后一个任务）｜ **批次**：`[P1-D]` ｜ **层级/分类**：L1 / B + D + E ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`the_gate_reports_a_nonzero_error_rate_before_the_gates_are_all_on()`：故意只开第 1 层跑全扫描 → 断言"错误确定率 > 0"。**预期失败原因**：全扫描装置未实现（这个 RED 同时证明**扫描真的有辨别力**，不是"恒绿的空门禁"）
- **GREEN**：扫描维度全开：`|d| ∈ {1..40, 100, 500} × 符号 × P/|d| ∈ {0.5, 1, 2, 4} × 动态占比 {0, 10, 30, 60}% × σ ∈ {0, 2, 5, 10}`；判据 = 与**生成器真值逐行相等**（不需要基线图片、不需要 `insta`）；**唯一零容忍项 = 错误确定率为 0**（允许 `Uncertain`/`None`/`scene_cut`）
- **REFACTOR**：把扫描拆成"**冒烟子集（CI 每次跑，秒级）**"与"**全量（`#[ignore]` + `Release`，分钟级）**"两层，但**门禁用冒烟子集**（否则 CI 会被人为跳过）；剩余维度记入 `docs/Temp/`
- **退出条件**：① 错误确定率 == **0**；② 冒烟子集在**干净 checkout** 上 `cargo test -p snapclip-capture --lib` 通过；③ 覆盖率（`Confirmed` 占比）作为**被记录但不被优化**的指标（G1：不许为了好看而放宽门限）
- **提交标题**：`[P1-24] zero wrong-and-confident, and the gate proves it can fail`

**P1 退出条件（阶段级）**：① `tools/check-dependency-direction.ps1` 的第二遍扫描（`scroll/` 内不得出现 `crate::windows|crate::sampler|winapi|windows_sys|windows::`）通过；② `E-ACC-1` 冒烟子集在 CI 通过、错误确定率为 0；③ `E-PERF-1` 的结论已回填（`Stitch Latency` 与 CPU 两行不再是"待测"）；④ `scroll-p1` 标签已打。

---

## 8. P2 平台帧源（窗口级 WGC 多帧）

**本阶段的性质**：把"一次取一张图"的 `FrozenFrame` 变成"**一条流**"（R-3）。**改动集中在既有的 `windows/win/wgc.rs`（约 +150 行，不拆文件）与 `windows/providers.rs`**，新文件只有 `windows/scroll_source.rs`。**回归风险为零**：窗口级是**新增路径**，`attempt_order` 的 `[Wgc, BitBlt]` 完全不受影响（V2 §24.2）。

**已核实的当前代码事实（本阶段的前提下，逐行）**：`crates/snapclip-capture/src/windows/win/wgc.rs:262-268` **只有** `create_item_for_monitor`（`interop.CreateForMonitor(monitor)`），**全文件没有 `CreateForWindow`**；`:106-108` `SetIsCursorCaptureEnabled(false)` 用 `?` **传播**错误；`:110-112` `SetIsBorderRequired(false)` **吞掉**错误（只 `eprintln`，注释写着 "report but never fail on it"）；`:88-101` `FrameArrived` 不可用时降级为轮询；`:103-105` 每次 `CreateCaptureSession`；`:117` 取一帧。→ **两种相反的失败模式（"接口不可用即整体失败"与"静默降级"）都存在**，这正是 P2.04 要同时处理的（见 §0.6 **DEV-3**）。

### P2.01 `wgc.rs` 增 `CreateForWindow` + 会话内复用 pool/session

**上游**：V2 §24.2、D-9 ｜ **第一性原理**：**F-09/F-10**（窗口级捕获是 C5"覆盖层不必隐藏"、遮挡下仍取目标内容的**唯一**机制）+ **F-08**（每帧重建的 1.5s 首帧窗口无法支撑连续多帧）｜ **前置**：P0.05 ｜ **可并行**：与 P2.02（组 **G9**；接口 `ProviderKind`/`FrozenFrame` 形状先冻结）｜ **批次**：`[P2-A]` ｜ **层级/分类**：L2 + L3 / B + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`a_window_capture_item_can_be_created_for_a_top_level_window()`（mock/真实两类目标各一次；对**无效句柄**必须返回 `InvalidTarget` 而不是 panic）+ `the_pool_is_not_recreated_between_frames()`（100 步 → `Recreate` 计数 == 0，除尺寸变化）。**预期失败原因**：`create_item_for_window` 不存在
- **GREEN**：新增 `fn create_item_for_window(hwnd: isize)`（`IGraphicsCaptureItemInterop::CreateForWindow`，与既有 `CreateForMonitor` **并存**）；把 item/pool/session 提升为**会话级对象**（`WgcSession`），`CreateFreeThreaded(device, format, bufferCount = **3**, size)`（今天是 2；参考实现是 3）；尺寸变化只 `pool.Recreate(...)`；**不启用 `SetDirtyRegionMode`**（V2 §24.2）
- **REFACTOR**：`next_frame`/`FrameArrived` 的既有降级逻辑保持不动（`:88-101` 的"事件不可用→轮询"是**好**的降级：它不改变结果质量）；把会话对象的所有权写清楚（谁 drop 谁停捕获）
- **退出条件**：① 两个用例通过；② `E-CAP-1` 的五类目标用**这条新路径**复跑一遍；③ `attempt_order` 相关的既有用例（A 类）全绿
- **提交标题**：`[P2-01] a window capture item can live for a whole session`

### P2.02 `ProviderKind::WgcWindow` 与 `providers.rs` 的多次 `read_region`

**上游**：V2 §11.3、§28.2 ｜ **第一性原理**：**F-01**（每步只需要**新露出的那一块**，不需要整屏）+ **F-07**（`read_region` 的惰性是既有能力，长图必须用它）｜ **前置**：P2.01 ｜ **可并行**：与 P2.01（G9）｜ **批次**：`[P2-A]` ｜ **层级/分类**：L2 / B ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`a_scroll_source_reads_a_region_instead_of_the_whole_frame()`（断言一帧只在需要时回读、且**每次回读的矩形都落在该帧内**）+ `reading_the_same_region_twice_is_an_error_or_a_cache_hit_but_never_a_second_gpu_transfer()`（§11.3"每步一次回读"的机械检查）。**预期失败原因**：`ProviderKind::WgcWindow` 不存在
- **GREEN**：`providers.rs` 增 `ProviderKind::WgcWindow`，走 `read_region`（既有 `providers.rs:135` 的定义 / `d3d11.rs:254` `read_back_bgra` / `:315` `read_back_region_bgra`）
- **REFACTOR**：**移除"一次全额回读"在滚动路径上的可达性**（滚动路径只允许 `read_region`）；把"每步一次回读"做成**计数器断言**而不是注释
- **退出条件**：① 两个用例通过；② §30.1 的"每步仅一次回读（100 步计数==100）"行通过
- **提交标题**：`[P2-02] the scroll path reads regions, never whole frames`

### P2.03 `FrameSource` 实现（`windows/scroll_source.rs`）

**上游**：V2 §11.1、§27.3、§9.2（`scroll_source.rs` 只负责"窗口级 WGC 多帧 + 初始几何/分辨率/monitor rect，**不扫描窗口**"）｜ **第一性原理**：**R-3**（`OnceLock` 在类型上就禁止第二帧 → 只能换类型，不能打补丁）｜ **前置**：P2.02 ｜ **可并行**：与 P2.04（组 **G9**）｜ **批次**：`[P2-A]` ｜ **层级/分类**：L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`next_returns_idle_instead_of_blocking_forever_when_no_frame_arrives()`（超时必须返回 `Poll::Idle`）+ `a_closed_window_ends_the_stream_with_a_reason()`（`Poll::Ended(EndReason)`，**不是错误**）。**预期失败原因**：未实现
- **GREEN**：`pub(crate) trait FrameSource { fn next(&mut self, timeout: Duration) -> Result<Poll, FrameError>; }`、`pub(crate) enum Poll { Frame(Observation), Idle, Ended(EndReason) }`；`ScrollSourceRuntime` 产出初始几何/分辨率/monitor rect
- **REFACTOR**：把"目标选择"从帧源里**排除**（`ScrollTarget` 来自 `App`/既有 `WindowTargetProvider`，V2 §9.2）—— **帧源不得自己扫描窗口**
- **退出条件**：① 两个用例通过；② `grep -c "window_detection\|EnumWindows" src/windows/scroll_source.rs` == 0
- **提交标题**：`[P2-03] a frame source is a stream, not a snapshot`

### P2.04 能力探测 `CaptureCapabilities` 与"选项失败必须可见"

**上游**：V2 §24.3、§11.4、D-10、**DEV-3** ｜ **第一性原理**：**G12**（失败必须可见）+ **F-09**（`IGraphicsCaptureSession2/3` 的可用性**随系统版本变化** → 必须运行期探测，不能写死）｜ **前置**：P2.01 ｜ **可并行**：与 P2.03（G9）｜ **批次**：`[P2-A]` ｜ **层级/分类**：L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`an_unavailable_capture_option_is_recorded_and_asserted_not_swallowed()`（注入"`IsBorderRequired` 抛异常"→ 必须产出 `CaptureOptionUnavailable` **且**断言"长图里不存在边框色带"）+ `an_unavailable_cursor_option_does_not_fail_the_capture()`（**DEV-3**：今天 `:107` 用 `?` 让整体失败；新行为必须是"探测 + 记录 + 退回掩码排除光标"）。**预期失败原因**：`CaptureCapabilities` 不存在
- **GREEN**：`CaptureCapabilities{ border_control: Option<bool>, cursor_control: Option<bool>, dirty_regions, backend, window_target }`；先探接口（`IGraphicsCaptureSession2/3` 的 `QI`）再调用；两条选项的失败都走诊断而不是 `?` 或 `eprintln`
- **REFACTOR**：**站在 PixPin 一侧**（它把降级记进日志）而不是参考实现一侧（`let _ = session.SetIsBorderRequired(false)`）；把"`eprintln` 不是诊断"写成规则
- **退出条件**：① 两个用例通过；② §30.1 的"选项不可用"行通过；③ 每次降级都有 `Diagnostic`（G12）
- **提交标题**：`[P2-04] an unavailable option is a recorded fact`

### P2.05 显示拓扑三档处置

**上游**：V2 §24.4、R-7 ｜ **第一性原理**：**F-06 + G12**（"全部取消"把不必要的破坏当成保守：目标无关的显示器变化**不该**终止用户已经滚了 30 秒的会话）｜ **前置**：P2.03 ｜ **可并行**：与 P2.06（组 **G9**）｜ **批次**：`[P2-A]` ｜ **层级/分类**：L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：**五行**用例（逐行对 §24.4）：目标显示器 DPI 变化 → 停止 + `TargetLost` + `Partial`；**其它**显示器变化 → **继续**；目标尺寸变化 → 停止 + `Partial`；跨显示器移动（尺寸与 DPI 不变）→ **继续**；最小化 → 停止 + `Partial`。**预期失败原因**：三档逻辑未实现
- **GREEN**：把 `WM_DISPLAYCHANGE`/`WM_DPICHANGED` 的既有"全取消"（今天的行为）替换为**三档判定**；`TargetLost` 的语义固定为"目标不能再作为滚动目标"（**这是它是 11 个 `StopReason` 变体之一的原因**）
- **REFACTOR**：把"目标相关与否"的判定写成**一个函数**（输入：目标 rect/DPI/可见性 + 变化事件 → 输出：`Continue` / `Stop(reason)`），五行用例全部走它
- **退出条件**：① 五行用例通过；② §30.1 的对应五行通过；③ 既有的"显示器变化→取消 renderer"行为在**普通截图**路径上不变（A 类回归）
- **提交标题**：`[P2-05] only the changes that matter to the target stop the session`

### P2.06 后端选择与降级（窗口级优先，显示器级回退）

**上游**：V2 §11.5、§24.2 ｜ **第一性原理**：**F-09 + F-20**（窗口级天然不含覆盖层与遮挡者；显示器级会取到遮挡者 → 窗口级**永远是首选**）｜ **前置**：P2.04 ｜ **可并行**：与 P2.05（G9）｜ **批次**：`[P2-A]` ｜ **层级/分类**：L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`the_window_backend_is_preferred_and_the_monitor_backend_is_the_fallback()`（断言顺序，**不是"先试 A 再退 B"的实现细节**：顺序是策略，不是重试）+ `a_fallback_records_a_diagnostic()`。**预期失败原因**：选择逻辑未实现
- **GREEN**：滚动路径的后端顺序 = `[WgcWindow, WgcMonitor]`（显示器级回退时才会涉及 WDA，见 §24.5）；每次回退产出 `CaptureBackendFallback`（`ScrollDiagnosticCode` 之一）
- **REFACTOR**：**不改 `attempt_order`**（普通截图的后端选择是 §33.5 的保护项）；滚动路径**自带**选择逻辑（这是"不侵入既有路径"的具体做法）
- **退出条件**：① 两个用例通过；② §30.7 的"捕获路径不变（`attempt_order` 行为与改动前一致）"行通过
- **提交标题**：`[P2-06] the window backend is the policy, not a retry`

### P2.07 目标丢失与结束语义

**上游**：V2 §11.1、§20.4、§24.4 ｜ **第一性原理**：**G12 + 推论 2.5**（`Closed` / 最小化 / 尺寸变化是**三种不同事实**，必须映射到**不同**的 `EndReason`/`StopReason`，否则用户无法知道发生了什么）｜ **前置**：P2.03 ｜ **可并行**：无 ｜ **批次**：`[P2-A]` ｜ **层级/分类**：L2 / D ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`closed_minimised_and_resized_are_three_distinct_endings()`（三个用例各断言一个不同的 reason）。**预期失败原因**：映射未实现
- **GREEN**：`Closed → EndReason::TargetLost`、最小化 → `EndReason::TargetLost`、尺寸变化 → `EndReason::TargetLost`，但**都携带 `Partial`** 与**各自的 `detail`**（区分靠 `detail`，不靠新增变体）
- **REFACTOR**：核对 `StopReason` 的 11 个变体里**没有**任何一个是为这三者单独新增的（保持 `docs/30 §20.4` 的词表）
- **退出条件**：① 三个用例通过；② `StopReason` 变体数 == 11
- **提交标题**：`[P2-07] three endings, one reason, three details`

### P2.08 帧源的**无桌面可运行性**与 `T-THREAD-1` 的边界落实

**上游**：V2 §29.2、§21.3、G9 ｜ **第一性原理**：**G9**（核心可测且不依赖真实桌面）+ **F-01**（`ID3D11DeviceContext` 的所有者线程不变式必须**可断言**）｜ **前置**：P0.02、P2.02 ｜ **可并行**：无 ｜ **批次**：`[P2-A]` ｜ **层级/分类**：L2 / A + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`the_scroll_source_compiles_and_its_logic_tests_run_without_a_desktop()`（用 mock `FrameSource` 跑一遍 `ScrollLoop` 的**决策**逻辑，不需要 GPU）+ `no_real_desktop_test_silently_skips_in_this_module()`（D-14 的规则：只允许 `#[ignore]` 或显式环境断言）。**预期失败原因**：mock 帧源不存在
- **GREEN**：`FrameSource` 的测试替身（`MockFrameSource`，按脚本产出 `Poll`）+ 在 P2 的所有平台代码里**不使用** `desktop_available()` 式静默返回
- **REFACTOR**：把 P0.02 的 context 断言与"谁在哪个线程回读"的关系写成 §21.3 第 ③ 步的**前置结论**（若 P0.02 显示今天就 panic，本任务必须先落 deferred context，见 `E-THREAD-1`）
- **退出条件**：① mock 帧源可驱动 `ScrollLoop`；② `grep -c "return; }" `（静默跳过式）== 0（该模块内）；③ `T-THREAD-1` 的结论已落实到本模块
- **提交标题**：`[P2-08] the frame source is testable without a desktop`

**P2 退出条件（阶段级）**：① `E-CAP-1` 五类目标用新路径复跑并**逐类记录**；② `E-CAP-1` 扩一条（`WDA_EXCLUDEFROMCAPTURE` —— `OQ-2` 的收口），**若未取得设备就如实标"未取得"**；③ 100 步 `Recreate == 0`；④ `scroll-p2` 标签已打。

---

## 9. P3 注入与闭环

**本阶段的性质**：**唯一新增线程** `snapclip-scroll-driver` 落在这里（V2 §21.1）。三条理由（注入+等待稳定语义阻塞 / 匹配与画布写入不得抢 `RENDER_TICK_MS = 15` 的整面重绘预算 / context 所有权不变式要求"谁用 context 谁就是所有者线程"，而滚动管线**不用** context）—— **不是**为了并发性能（V2 §21.1 的措辞，AGENTS.md 第 6 条）。

### P3.01 `ScrollActuator`：两条并列路径 + 子窗口下沉

**上游**：V2 §14.3、§24.6、C6；**已实测**（§24.6.1）｜ **第一性原理**：**F-14/F-15**（`PostMessageW` 绕过 UIPI；`SendInput` 受 UIPI 限制且作用于**前台**）→ 两条路径解决**不同**的问题，不是主备 ｜ **前置**：P0.09 ｜ **可并行**：与 P3.02（组 **G10**；`InjectRequest`/`InjectOutcome` 先冻结）｜ **批次**：`[P3-A]` ｜ **层级/分类**：L2 + L3 / B + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`post_message_sinks_to_the_deepest_child_window()`（**子窗口下沉**：`ScreenToClient` + **`ChildWindowFromPointEx` 逐层**；Chromium 的 `Chrome_RenderWidgetHostHWND` 是子窗口，**不下沉就会失败**）+ `the_lparam_uses_screen_coordinates()`（`scroll_probe.rs` 的实测：两种坐标空间在本机都得 800px 是**几何巧合**；实现必须按 MSDN 用**屏幕坐标**）+ `post_failed_and_target_not_found_are_distinct_statuses()`。**预期失败原因**：未实现
- **GREEN**：`windows/scroll_actuator.rs`：`SendInput`（`MOUSEEVENTF_WHEEL`/`HWHEEL` + `mouseData = ±120·n`）与 `PostMessageW(WM_MOUSEWHEEL/0x020E, wParam = delta<<16, lParam = MAKELPARAM(screen_x, screen_y))`；**共用 Snow Shot 的 5 类状态** `InjectStatus{ Posted, InvalidRequest, TargetNotFound, CoordinateFailure, PostFailed, Unsupported }`
- **REFACTOR**：`Posted` 必须**显式标注"未证明被处理"**（V2 §24.6；`PostMessageW` 返回 `TRUE` 只说明入队）—— 这条是 P3.03 自检存在的原因
- **退出条件**：① 三个用例通过；② `E-INJECT-1` 的 8 组（`P0.09` 的产物）都能用**这条产品路径**复现；③ 不引入新 crate 依赖
- **提交标题**：`[P3-01] two transports for two different problems, not one fallback`

### P3.02 条件选择 `choose()`

**上游**：V2 §24.6（**条件选择，不是"先试 A 再退 B"**）、`OQ-5` ｜ **第一性原理**：**正确性**（`SendInput` 会打到**前台**窗口 → 非前台目标下用 `SendInput` 是**错误**，不是"效果差"）→ 必须在**事前**判定，不能靠重试 ｜ **前置**：P3.01、P0.07 ｜ **可并行**：与 P3.01（G10）｜ **批次**：`[P3-A]` ｜ **层级/分类**：L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：`OQ-5`（路由设置）+ `OQ-3`（UIPI）—— **不阻塞实现**（判定表按 `P0.07`/`P0.09` 的读数写，读数缺失时走保守分支）

- **RED**：**四组**用例（前台/非前台 × 提权/非提权）断言选择结果与 §24.6 的表一致 + `a_non_foreground_target_never_uses_send_input()`。**预期失败原因**：`choose()` 未实现
- **GREEN**：`fn choose(target: &ScrollTarget, probe: &Probe) -> Transport`：提权且自身未提权 → `PostMessageW`；**非前台 → `PostMessageW`**；否则 `SendInput`；输入含 `SPI_GETMOUSEWHEELROUTING`（P0.07）
- **REFACTOR**：**把"不是 fallback"写成测试**（构造"前台普通窗口"时**不得**出现 `PostMessageW` 作为第一选择）
- **退出条件**：① 四组用例通过；② §30.2 的"条件选择"行通过；③ 判定表**只有一处**（`grep` 可核对）
- **提交标题**：`[P3-02] the transport is decided, not retried`

### P3.03 内容位移自检 + 路径切换 + `ActuatorFailed`

**上游**：V2 §24.7（**"注入是否生效"的自检 = 用内容位移验证**）｜ **第一性原理**：**G12 + F-03**（`Posted` 不是效果；"我说了"与"它动了"是两件事）｜ **前置**：P3.02、P1.14 ｜ **可并行**：无 ｜ **批次**：`[P3-A]` ｜ **层级/分类**：L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`three_posted_steps_without_a_confirmed_step_switch_the_path()`（连续 3 步 `Posted` 但 `d` 未 `Confirmed` ⇒ **切换注入路径** + `InjectPathSwitched`）+ `both_paths_failing_three_times_ends_the_session()`（两条路径各连续 3 步无效 ⇒ `ActuatorFailed` + `Partial`）。**预期失败原因**：未实现
- **GREEN**：自检器（连续计数 + 切换 + 上限）落在 `loop_control.rs`；切换时**重新跑 `choose()`** 并记录
- **REFACTOR**：把"3"这个数字做成命名常量并写清依据（`Posted` 是**弱**证据 → 判定"没生效"需要多次以避免误切换；该值是**启动值，待校准**，`E-CTRL-1` 会调整）
- **退出条件**：① 两个用例通过；② §30.2 的"注入失败后切路径""两条路径都失败"两行通过
- **提交标题**：`[P3-03] posted is not proof, the pixels are`

### P3.04 `ScrollLoop` 闭环与"等待稳定"

**上游**：V2 §13.2、§13.3、§11.1 ｜ **第一性原理**：**F-10（闭环）**：我们自己决定注入量 `n` → 于是"注入了多少"是**已知量**，这既产生 P1 先验，也产生"等待稳定"的可判定终点 ｜ **前置**：P3.01 ｜ **可并行**：与 P3.05（组 **G11**）｜ **批次**：`[P3-A]` ｜ **层级/分类**：L2 / B + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`the_loop_waits_until_two_consecutive_frames_agree_before_estimating()`（frame-driven settled：连续两帧一致才算静止）+ `smooth_scrolling_is_waited_out_instead_of_being_estimated()`（内容持续移动 3 帧 → **等到静止**才估计；**不得**在滚动中就下结论）。**预期失败原因**：未实现
- **GREEN**：`crates/snapclip-capture/src/scroll/loop_control.rs`：注入 → 等待稳定（帧驱动，不用固定 `sleep`）→ 取第二帧 → `estimate()` → `commit`/`continue`
- **REFACTOR**：**闭环不引入状态机**（V2 §20.1）：`ScrollSession{ ..., step, committed, discarded, streak, last_step_at, stop, undo }` + **纯函数** `fn phase(&ScrollSession) -> Phase`（`stop.is_some()` → `Stopped`；`bands.is_empty()` → `Preparing`；否则 `Running`）
- **退出条件**：① 两个用例通过；② §30.2 的"平滑滚动等待"行通过；③ `grep -c "enum Phase"` == 1 且**没有** `enum ScrollState`
- **提交标题**：`[P3-04] settle first, estimate second`

### P3.05 `ĝ` 更新与 `E-CTRL-1` 收敛曲线

**上游**：V2 §13.5、§16.6、§23.1 `E-CTRL-1` ｜ **第一性原理**：**F-10 的推论**（`ĝ` 是我们对"一格滚轮等于多少像素"的估计；它必须**缓慢且只在确证步上**更新，否则会把噪声变成前提）｜ **前置**：P3.04、P1.14 ｜ **可并行**：与 P3.04（G11）｜ **批次**：`[P3-A]` ｜ **层级/分类**：L1 + L2 / B + E ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`ĝ_converges_within_six_steps_to_within_twenty_percent()`（**六步内 ±20%**，`E-CTRL-1` 的判据）+ `ĝ_is_not_updated_on_uncertain_steps()`。**预期失败原因**：未实现
- **GREEN**：`ĝ ← 0.7·ĝ + 0.3·(d_k / n_k)`，只在 `Confirmed` 步更新；初值来自 `SPI_GETWHEELSCROLLLINES` 与行高的**量级估计**（明确标注"启动值"）
- **REFACTOR**：把 `E-CTRL-1` 的曲线画法（每步 `ĝ` vs 真值）落成一个可复跑的产物（`docs/Temp/`）
- **退出条件**：① 两个用例通过；② `E-CTRL-1` 有曲线与数字；③ `ĝ` 的初值与收敛速度记入 `docs/30 §13.5`
- **提交标题**：`[P3-05] the control loop closes on its own measurements`

### P3.06 手动模式（`n = 0`）

**上游**：V2 §13.4、§16.6、§19.5 ｜ **第一性原理**：**F-01**（用户手动滚动时我们**不知道** `n`，但知道"内容动了多少"→ 判定能力**不能**因此下降，只是失去先验）｜ **前置**：P3.04、P1.15 ｜ **可并行**：无 ｜ **批次**：`[P3-A]` ｜ **层级/分类**：L1 + L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`a_manual_session_tracks_the_non_zero_steps_and_reports_uncertain_on_periodic_pages()`。**预期失败原因**：未实现
- **GREEN**：`n = 0` ⇒ P1 关闭、走 P1.15 的峰族检测；"拖动预览框"也进入手动模式（`set_follow(false)`，§19.5）
- **REFACTOR**：把"手动模式**不是**另一种会话"写成注释与测试（同一个 `ScrollLoop`，只有 `n` 与先验不同）
- **退出条件**：① 用例通过；② §30.6 的"拖动后停止跟随"行通过；③ 手动序列在 `E-ACC-1` 夹具下**不产生错误确定**
- **提交标题**：`[P3-06] manual scrolling is the same loop with n = 0`

### P3.07 停止/取消与延迟测量

**上游**：V2 §20.5、§23.3（`Cancel latency`）、§21.4 ｜ **第一性原理**：**G8**（用户必须能**停**、能**取消**，且两者语义不同）+ **诚实性**（注入**不可中断** ⇒ 取消延迟有一个**物理下界** = 一次注入 + 一次稳定性等待，必须被**测量**而不是被声称）｜ **前置**：P3.04 ｜ **可并行**：无 ｜ **批次**：`[P3-A]` ｜ **层级/分类**：L1 + L2 / B + E ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`stop_commits_the_export_and_cancel_discards_it()`（两个用例，断言 `StopReason` 不同且产物处理不同）+ `cancel_latency_has_a_measured_max()`（**Max ≤ 500 ms 阈值**，`P50 ≤ 60`）。**预期失败原因**：未实现
- **GREEN**：沿用既有"推进 generation"的取消协议（`mailbox` 的 generation 丢弃语义是 §33.5 的保护项）；`Enter`/`Esc` 与普通截图一致（§20.5）；`WM_APP + 45` 新消息 + **冲突断言**（已占用 `+1/+2/+17/+18/+19/+43/+44`）
- **REFACTOR**：把"取消延迟的下界"写成**测量表**里的固定注释（否则下一个人会以为可以优化到 0）
- **退出条件**：① 两个用例通过；② §30.2 的"取消延迟""停止延迟"两行通过；③ 消息 id 冲突断言存在
- **提交标题**：`[P3-07] stop and cancel are different promises about the same pixels`

### P3.08 命令端口（`ScrollController`）与 `PreviewStream` 的**非对称**设计

**上游**：V2 §27.2（**命令不能被丢，状态必须可丢**）｜ **第一性原理**：**F-01 + G12**（`Stop`/`Cancel`/`Undo` 丢了 → 用户的意图消失；预览帧丢了 → 下一帧就覆盖，无损失）｜ **前置**：P3.07 ｜ **可并行**：与 P3.09（组 **G11**）｜ **批次**：`[P3-B]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`commands_are_sticky_and_cannot_be_lost_under_load()`（灌入 10^5 次预览更新 + 一次 `Stop`，断言 `Stop` **必然生效**）+ `preview_updates_may_be_dropped_but_the_count_is_visible()`（`dropped` 计数器）。**预期失败原因**：端口未实现
- **GREEN**：`ScrollController`：`stop/cancel/undo/set_follow/shutdown` —— 用**粘性 `AtomicBool`/`AtomicU32`** 而不是队列槽位；`PreviewStream`：`publish/take/dropped` —— 容量 1 **最新覆盖**
- **REFACTOR**：把这处**刻意的非对称**写成 ADR 引用（V2 §27.2："这是一处刻意的非对称设计"）
- **退出条件**：① 两个用例通过；② `grep -c "AtomicBool\|AtomicU32"` 在命令侧 ≥ 3；③ 端口方向与 §27.3 的表一致
- **提交标题**：`[P3-08] commands are sticky, previews are droppable`

### P3.09 `ScrollSession` 装配、与 `CaptureSession` 的干净交接、teardown

**上游**：V2 §20.6、R-5、§9.2 ｜ **第一性原理**：**R-5**（"选区"与"滚动"是**两个生命周期**；混在一起会让 `CaptureSession` 的 6 态穷举测试失效）｜ **前置**：P3.04、P2.03 ｜ **可并行**：与 P3.08（G11）｜ **批次**：`[P3-B]` ｜ **层级/分类**：L2 / A + B ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`the_capture_session_ends_before_the_scroll_session_starts()`（断言**干净交接**：普通截图选区确定 → `CaptureSession` 结束 → 把冻结帧 + 选区几何 + DPI 交给 `ScrollSession`）+ `a_scroll_session_tears_down_without_leaking_bands_or_threads()`（teardown 后线程退出、临时文件删除）。**预期失败原因**：装配未实现
- **GREEN**：`scroll/session.rs`（聚合 + teardown）；`ScrollSession` 是**独立对象**，**`CaptureSession` 不变**（§33.5 保护项）
- **REFACTOR**：把"滚动**不是** `CaptureSession` 的扩展"写成 §20.6 的引用注释；`CaptureState` 的 6 个活跃状态保持原样（`Adjusting` 的删除在 P6.03）
- **退出条件**：① 两个用例通过；② 既有 `CaptureSession` 的穷举测试**全绿**（A 类）；③ teardown 无残留
- **提交标题**：`[P3-09] the scroll session starts where the capture session ends`

**P3 退出条件（阶段级）**：① `E-CTRL-1` 与 `E-INJECT-1` ⑧ 组都有产物；② Cancel latency 的 Max 实测值在案（**这一条是本阶段唯一不可跳过的性能门槛**）；③ 普通截图回归（A 类）全绿；④ `scroll-p3` 标签已打。

---

## 10. P4 导出与内存

**本阶段的性质**：导出**从"把一张完整图像交给编码器"变成"把行交给编码器"**（R-4/D-11）。它同时是**内存**阶段的正面证据：`E-MEM-1` 的三档长度必须给出**同一个峰值**（G3）。

**DEV-2（必须记住）**：V2 §35 的 `P4.1` 一行把**端口**（`RowBandSink`/`RowBandWriter`，属 `snapclip-capture`）与**实现**（`PngRowBandSink`，属 shell 组合根）混在一起。本文拆成 `P4.01`（端口，capture 内）与 `P4.02`（实现，shell 内）—— **`png` 因此不会成为 `snapclip-capture` 的直接依赖**（N7 与依赖门禁都保持干净）。

### P4.01 `RowBandSink` / `RowBandWriter` 端口（capture 侧，纯 trait）

**上游**：V2 §17.7、§27.3、R-4、**F-12** ｜ **第一性原理**：**F-12**（PNG 的 `height` 必须在写 IHDR **之前**确定 → 只能用**签名**让违约不可编译；这不是编码器的偏好，是格式的事实）｜ **前置**：P1.21 ｜ **可并行**：与 P4.03（组 **G12**；`ImageMeta` 先冻结）｜ **批次**：`[P4-A]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`the_height_must_be_known_before_the_first_row()`（**类型层面**：`begin(&ImageMeta)` 拿不到 height 就写不出第一行）+ `writing_rows_out_of_order_is_rejected()` + `finish_after_abort_still_produces_a_decodable_artifact()`。**预期失败原因**：trait 未定义
- **GREEN**：`fn begin(&mut self, meta: &ImageMeta) -> Result<Box<dyn RowBandWriter>, ExportError>`；`fn write_rows(&mut self, first_row: u64, rows: &[u8]) -> Result<(), ExportError>`；`fn finish(self: Box<Self>, outcome: Option<AbortReason>) -> Result<Artifact, ExportError>`；`ImageMeta` 含 `width`/`height`/`length`/`axis`/`dpr`；**`u64` 用于行号**
- **REFACTOR**：把"**严格递增**"作为 V2 的实现要求（允许乱序的接口 + 拒绝乱序的实现 = 一处显式的取舍，必须写在注释里）
- **退出条件**：① 三个用例（含两个 D 类）通过；② `cargo tree -p snapclip-capture -e normal` **不含 `png`**（机械检查）
- **提交标题**：`[P4-01] the height is a precondition, so the API says so`

### P4.02 `PngRowBandSink` 落在 shell 组合根

**上游**：V2 §17.7、§27.5、P0.04 ｜ **第一性原理**：**依赖方向**（编码器是**输出格式**的选择，不是捕获的事实 → 它属于 shell）｜ **前置**：P4.01 ｜ **可并行**：与 P4.03（G12）｜ **批次**：`[P4-A]` ｜ **层级/分类**：L2 + L4 / B + E ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`the_sink_streams_without_materializing_the_image()`（编码期间 `peak − live` 不超过"一份条带"量级）+ `the_artifact_decodes_back_to_the_expected_pixels()`。**预期失败原因**：实现不存在
- **GREEN**：`apps/snapclip/src/capture/` 内实现 `PngRowBandSink`，用 `png::Encoder::stream_writer()`；**显式设置压缩/滤波参数**（取 `P0.04` 选定的一组）；解码验证时**显式提高 `png::Limits{bytes}`**（F-10 的陷阱：默认限制会让 >64 MiB 的产物"假失败"）
- **REFACTOR**：把 `png` 出现在**哪个 crate** 写成注释（shell 可以，capture 不可以），并让 §28.4 的门禁**同时覆盖**这一条
- **退出条件**：① 两个用例通过；② `tools/check-dependency-direction.ps1` 干净；③ `P0.04` 的选定参数生效（可用一个参数断言）
- **提交标题**：`[P4-02] the encoder belongs to the shell, the rows belong to the capture`

### P4.03 消除导出路径的 4 份完整像素拷贝（D-11）

**上游**：V2 §22.1、§22.4、D-11 ｜ **第一性原理**：**F-07**（`1920×300000` 需 ≈ **8.6 GiB**，**必然失败**）→ 这不是"优化"，是"能不能跑" ｜ **前置**：P4.01 ｜ **可并行**：与 P4.02（G12）｜ **批次**：`[P4-A]` ｜ **层级/分类**：L2 / A + D + E ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`the_export_path_holds_at_most_one_row_band_at_a_time()`（用 `CountingAllocator` 断言峰值）——**预期失败原因**：今天有 4 份拷贝（`apps/snapclip/src/capture/artifact_writer.rs:44` → `image.rs:153` → `image.rs:66` → 编码器内部）
- **GREEN**：删除中间整图；**只保留两处不可避免的整块拷贝**（GPU→CPU 每步一次、画布↔磁盘 LRU 未命中时）
- **REFACTOR**：把"拷贝数"做成**可断言的数字**（今天 4 → 目标 ≤ 2）而不是"感觉少了"
- **退出条件**：① 峰值断言通过；② §30.7 的"导出拷贝数 ≤ 2 份"行通过；③ 普通截图的导出路径（A 类）**行为不变**（这里是 `ArtifactWriter` 的既有实现，改动必须由 A 类兜底）
- **提交标题**：`[P4-03] export stops copying the image four times`

### P4.04 `u32` 越界必须在 `begin` 之前拒绝（D-12）

**上游**：V2 §22.1、§26.1、D-12 ｜ **第一性原理**：**G12 + F-12**（静默截断会产出**尺寸错误的图**，比报错更糟；`image.rs:26-35` 已经用 `u64` 做对了，**这里没有对齐**）｜ **前置**：P4.01 ｜ **可并行**：无 ｜ **批次**：`[P4-A]` ｜ **层级/分类**：L2 / D ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`an_oversized_dimension_is_rejected_before_the_first_byte()`（构造 > `u32::MAX` 的尺寸 → **拒绝**；`apps/snapclip/src/capture/artifact_writer.rs:42-43` 今天是 `as u32` 截断）。**预期失败原因**：截断仍存在
- **GREEN**：尺寸域统一为 `u64`；超限返回 `ExportError`（不是 panic、不是截断）
- **REFACTOR**：把"**截断与拒绝是两种不同的失败**"写进 §26.1 的引用注释
- **退出条件**：① 用例通过；② §30.7 的"`u32` 越界"行通过；③ `grep -c "as u32" apps/snapclip/src/capture/` == 0（该文件内）
- **提交标题**：`[P4-04] an oversized image is refused, never truncated`

### P4.05 严格递增校验、`AbortReason` 与"取消/超限仍产出合法 PNG"

**上游**：V2 §17.7、§17.6、§26.2 ｜ **第一性原理**：**G4 + G12**（超限不是失败 → 必须能把"我产出了前缀"这件事**表达出来**且**产物可解码**）｜ **前置**：P4.04 ｜ **可并行**：与 P4.06（组 **G12**）｜ **批次**：`[P4-A]` ｜ **层级/分类**：L1 + L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`finish_with_abort_writes_a_complete_iend_and_the_file_decodes()`（三种 `AbortReason`：取消 / `MemoryLimit` / `ExportBudget`）+ `a_skipped_row_range_is_an_error()`. **预期失败原因**：未实现
- **GREEN**：`AbortReason` 三值 + `finish(Some(reason))` 仍写 IEND；行号严格递增（跳过或回退返回 `ExportError`）
- **REFACTOR**：**不新建诊断通道**（V2 §26.4）：只增加 `ExportTrimmed`/`ArtifactDiscarded` 两个 `ScrollDiagnosticCode`（§26.3 的 13 个之一）
- **退出条件**：① 两个用例通过；② §30.4 的"流式导出（乱序返回错误）"与"上限三层（`Partial` 是合法 PNG）"两行通过；③ 诊断码总数 == 13
- **提交标题**：`[P4-05] a partial result is still a real file`

### P4.06 `BandStore` 换出文件的 `Drop` 清理与临时文件治理

**上游**：V2 §22.7、§22.3、D-13 ｜ **第一性原理**：**G3 + 无残留**（今天 `artifact_store` **自承无清理** → V2 不接受继承；换出文件是"用磁盘换内存"这一策略的**代价**，代价必须被计账和回收）｜ **前置**：P1.20 ｜ **可并行**：与 P4.05（G12）｜ **批次**：`[P4-A]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`a_cancelled_session_leaves_no_spill_files()`（取消、panic 之外的所有路径）+ `a_spill_file_is_removed_when_its_band_is_evicted_back_to_memory()`。**预期失败原因**：`Drop` 未实现
- **GREEN**：`SpillRef` 的 `Drop` 删除文件；会话级临时目录在 teardown 时清空；**同时记录换出文件大小**（`E-MEM-1` 要求单独记这一项）
- **REFACTOR**：把"换出文件必须在 `Drop` 里删"作为一条**可 `grep` 的规则**（`impl Drop for SpillRef`）
- **退出条件**：① 两个用例通过；② §30.7 的"换出文件已删除"行通过；③ `docs/Temp/` 与会话临时目录在测试后都为空
- **提交标题**：`[P4-06] memory swapped to disk is still memory`

### P4.07 `E-MEM-1`：三档长度的独立进程内存测量

**上游**：V2 §23.1 `E-MEM-1`、§22.6、G3、§30.7 ｜ **第一性原理**：**G3**（内存上界与图像长度无关）—— 这是 V2 相对 PixPin **单块画布**（`W×H×4`）的结构性差异，**必须被测量**而不是被声称 ｜ **前置**：P4.03、P4.06 ｜ **可并行**：无 ｜ **批次**：`[P4-A]` ｜ **层级/分类**：L4 / E + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`the_probe_reports_live_peak_and_allocated_separately()`（装置自证：三项指标都必须非零且可区分）+ `the_three_lengths_run_in_separate_processes()`（结构性检查：不是同一个进程跑三次）。**预期失败原因**：装置未实现
- **GREEN**：10,000 / 30,000 / 100,000 px 三档，各独立进程（`Release`），记录 `live`/`peak`/`allocated` + **换出文件大小**（单独一列）；`#[cfg(feature = "stage-timing")]` 的 `CountingAllocator` **只在本 crate 内**
- **REFACTOR**：把方法论纪律写进产物（**"堆流量不能证明空间下降（存储搬到 OS 映射时）"** —— 这是参考项目 `benchmark-support/README.md` 的教训）→ 因此**必须同时**记 `allocated` 与 `peak`，且"OS 映射的临时文件"必须单独记
- **退出条件**：① 三档的 `peak` 差异 **≤ 10%**；② 换出文件大小有数字；③ `docs/30 §22.6` 的目标列被替换为实数或标"未取得"
- **提交标题**：`[P4-07] the memory ceiling does not grow with the image`

**P4 退出条件（阶段级）**：① `E-MEM-1` 三档数据在案且 `peak` 差异 ≤10%；② 拷贝数 ≤2；③ 取消/超限路径都产出**可解码**的 PNG；④ `scroll-p4` 标签已打。

---

## 11. P5 预览 UI

**本阶段的性质**：**"为什么需要预览"不是审美问题**——V2 §19.1 给了实证：§5 确证今天存在历史缩略图**死分支**且 `load_thumbnail` 读**全量**字节 ⇒ 30 万像素高的截图让历史列表**单行加载约 2 GiB**（D-13）。预览是**长图能否被用户使用**的前提。

**所有权已裁决**（§0.6 不涉及；见 `b45`）：**预览面板由覆盖层线程用 D2D 绘制，位于 `snapclip-capture` 内**；滚动语境下的"UI 线程"= **覆盖层线程**；GPUI 只在 shell（历史/设置），**不在捕获路径上**（门禁保证）。

### P5.01 `PreviewStream` 生产者与 10 Hz 上限

**上游**：V2 §19.3、§27.2、P3.08 ｜ **第一性原理**：**F-08（预览不能拖慢采集）**：预览的更新频率必须**受控**，否则它会成为最贵的那个消费者 ｜ **前置**：P3.08、P1.20 ｜ **可并行**：与 P5.02（组 **G13**）｜ **批次**：`[P5-A]` ｜ **层级/分类**：L1 + L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`updates_are_capped_at_ten_hertz()`（100 步 → 更新次数 ≤ 10 Hz × 时长）+ `a_locked_mailbox_drops_instead_of_blocking_the_driver()`（`try_lock` 失败 → 只累加 `dropped`，**绝不阻塞生产者**）+ `the_update_carries_no_pixels()`（**只带"哪段可读"**）。**预期失败原因**：未实现
- **GREEN**：`PreviewStream{ mailbox: Mutex<Option<PreviewUpdate>>, wake: Condvar, dropped: AtomicU64 }`、`PreviewUpdate::{Bands{first_row,rows,scale}, Span{primary_len}, Viewport{band,status}, Ended{reason}}`
- **REFACTOR**：把 10 Hz 标注为**启动值，待校准**（`E-PERF-4` 会在 5/10/20/30 Hz 之间取舍）
- **退出条件**：① 三个用例通过；② §30.6 的"更新频率"行通过；③ 消费者只 `try_recv` + 取只读句柄（可用类型/API 核对）
- **提交标题**：`[P5-01] the preview is told what is readable, not handed pixels`

### P5.02 窗口化缩略（与画布**共享** `BandStore` 预算）

**上游**：V2 §19.2、ADR-9、D-5 ｜ **第一性原理**：**G3 + Occam**（缩略条带 = `BandStore` 的**另一种条目**，不是"另一种数据" → 因此不新增预算、不新增策略）｜ **前置**：P1.20 ｜ **可并行**：与 P5.01（G13）｜ **批次**：`[P5-A]` ｜ **层级/分类**：L1 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`a_hundred_thousand_pixel_canvas_never_materialises_a_whole_thumbnail()`（断言总分配的缩略内存与 `primary_len` **无关**）+ `evicting_preview_bands_never_evicts_the_reference_band()`。**预期失败原因**：未实现
- **GREEN**：缩略条带作为 `BandStore` 条目（`scale` 字段），共享 `MemoryBudget{total, resident_canvas, resident_preview}` 与 LRU；**只生成可见窗口**
- **REFACTOR**：把 `D-5` 的理由写成注释（"预览不是另一种数据，是同一种数据的不同尺度"）；**`grep -c PreviewPatch` == 0**
- **退出条件**：① 两个用例通过；② §30.6 的"预览窗口化（不生成整图缩略）"行通过；③ 预算字段全部有读取点
- **提交标题**：`[P5-02] the thumbnail is a band, not a second copy`

### P5.03 视口框三态与"八问可答"（overlay D2D 绘制）

**上游**：V2 §19.1、§19.4、§19.7 ｜ **第一性原理**：**G8**（用户的八个问题都必须有答案）+ **F-05**（`Confirmed`/`Uncertain`/`Ended` 是**三种不同事实**，视觉上必须可区分）｜ **前置**：P5.01 ｜ **可并行**：与 P5.04（组 **G13**）｜ **批次**：`[P5-A]` ｜ **层级/分类**：L2 / B + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`the_eight_questions_have_concrete_answers_after_ten_steps()`（八项逐项断言有确定值）+ `uncertain_steps_are_drawn_dashed_with_a_counter_not_in_red()`（**不用红绿**：PixPin 的红色指向"回滚"动作，而 V2 的失败态**不需要用户做任何事**）+ `the_viewport_box_never_gets_thinner_than_four_dip()`。**预期失败原因**：未实现
- **GREEN**：覆盖层 D2D 绘制（`crates/snapclip-capture/src/windows/` 内）；三态外观 = `Confirmed` 实线 / 其余**虚线 + 累计未采用计数** / `Ended` 固定；框高下限 **4 DIP**
- **REFACTOR**：**相对 PixPin 的三处改动**写进注释（① 停止与取消分开 ② 失败态不画成错误而写"本步未被采用，会话继续" ③ 增加"累计未采用步数"）
- **退出条件**：① 三个用例通过；② §30.6 的"八问可答""视口框三态""视口框下限"三行通过；③ `grep -c "gpui" crates/snapclip-capture/src/**/*.rs` == 0
- **提交标题**：`[P5-03] three states the user can tell apart without a legend`

### P5.04 撤销 / 拖动 / 回到最新

**上游**：V2 §19.5、§19.6、P1.22 ｜ **第一性原理**：**G8 + P1.22 的推论**（撤销是"旧像素优先"带来的**免费**能力 → 不暴露它是浪费；拖动是"用户想看看刚才那段"的最小手段）｜ **前置**：P1.22、P5.03 ｜ **可并行**：与 P5.03（G13）｜ **批次**：`[P5-A]` ｜ **层级/分类**：L1 + L2 / B ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`dragging_the_preview_leaves_follow_mode_and_returns_on_command()` + `undo_can_be_pressed_repeatedly_and_never_underflows()` + `zooming_has_exactly_two_levels()`（**不做连续缩放**）。**预期失败原因**：交互未实现
- **GREEN**：拖动 → `set_follow(false)` + "回到最新"可用；撤销可连按；缩放两档
- **REFACTOR**：**明确不做**：方向下拉 / 缩略窗任意缩放 / 自动裁剪独立 UI / 在预览里画选区标注（§19.7）
- **退出条件**：① 三个用例通过；② §30.6 的"拖动后停止跟随"行通过；③ 撤销与 P1.22 的 `undo_last()` 是**同一处**逻辑
- **提交标题**：`[P5-04] undo exists because the writes are append-only`

### P5.05 真实渲染回读断言（面板不覆盖操作区）

**上游**：V2 §19.7、§30.6 最后一行 ｜ **第一性原理**：**"布局正确"必须可机检**（`d2d/tests.rs` 已有同型用例 → 沿用同一形态，不发明新机制）｜ **前置**：P5.03 ｜ **可并行**：无 ｜ **批次**：`[P5-A]` ｜ **层级/分类**：L3（`#[ignore]`，真实渲染回读）/ B ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`the_preview_panel_does_not_cover_the_toolbar_hit_regions()`（回读像素 + 与工具的命中矩形求交）。**预期失败原因**：用例未实现
- **GREEN**：新增一个与 `crates/snapclip-capture/src/windows/win/d2d/tests.rs` **同型**的用例（覆盖层 + 预览面板一起渲染后回读）
- **REFACTOR**：该用例必须是 `#[ignore]`（真实渲染）**或**显式环境断言，**不得静默跳过**（D-14 规则）
- **退出条件**：① 用例从"红"到"绿"（有证据）；② 它不出现在 `apps/snapclip/tests/ui.rs` 内（滚动会话**不进**那个文件，§29.6）
- **提交标题**：`[P5-05] the panel proves it does not cover the controls`

### P5.06 "预览不阻塞采集"与主线程同步工作预算（`E-PERF-4`）

**上游**：V2 §23.1 `E-PERF-4`、§23.3、§21.2 ｜ **第一性原理**：**F-08**（预览是**消费者**，它的慢不能变成采集的慢）+ **量化**（"跟手"必须是数字：`Scroll response` / `Preview update latency` / `Stop-Cancel latency`）｜ **前置**：P5.01、P3.07 ｜ **可并行**：无 ｜ **批次**：`[P5-A]` ｜ **层级/分类**：L2 + L4 / B + E ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`a_slow_consumer_does_not_slow_the_capture_loop()`（mock 让 UI 消费极慢 → 采集步数**不受影响**，且 `dropped > 0`）+ `the_overlay_thread_never_blocks_longer_than_eight_ms()`。**预期失败原因**：未实现/未测量
- **GREEN**：`E-PERF-4`（预览成本与 5/10/20/30 Hz 的取舍）+ 三项延迟的 P50/P95/Max 实测（`Scroll response` 阈值 P95 ≤ 80 ms 来自 `RENDER_TICK_MS = 15` 的推导）
- **REFACTOR**：把"覆盖层线程最大同步工作 ≤ 8 ms"（阈值）与"≤ 4 ms"（目标）**分开记录**（V2 §23.3 的两列）
- **退出条件**：① 两个用例通过；② 三项延迟有数字；③ `docs/30 §23.3` 的 `待测` 格子被替换或标"未取得"
- **提交标题**：`[P5-06] a slow preview cannot slow the capture`

**P5 退出条件（阶段级）**：① 100,000 px 画布**不生成整图缩略**；② 八问全部可答（真机会话一次）；③ 三项延迟有数字；④ `scroll-p5` 标签已打。

---

## 12. P6 清理与收口

**本阶段的性质**：**测试先于删除**（V2 §33.6 的四步顺序，**禁止颠倒**）。P6 的删除/重构**一条都不允许先做** —— 这正是"破坏性重构"与"不可验证的重写"的分界线。

### P6.01 A 类回归测试：冻结当前行为

**上游**：V2 §33.6 第 1 步、§30.7 ｜ **第一性原理**：**AGENTS.md 第 7/8 条**（"相关已有功能正常 + 验证通过"是验收标准）→ 把它变成**可判定**的唯一办法是先把当前行为**冻成测试** ｜ **前置**：P5 完成 ｜ **可并行**：无 ｜ **批次**：`[P6-A]` ｜ **层级/分类**：L1 + L3 / A ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`the_regression_suite_names_the_behaviours_it_freezes()`（元测试：断言 A 类清单里的每一条都有对应测试名）。**预期失败原因**：A 类清单尚未落成测试
- **GREEN**：覆盖 §30.7 后半与既有测试：普通截图全流程（F5）· 区域截图 · 窗口截图 · 浏览器窗口 · 快捷键 · Cancel · 既有 UI（`apps/snapclip/tests/ui.rs`）· `hit_test.rs:435` 的 p95 断言 · 9 个既有 `#[ignore]`（**保留**）
- **REFACTOR**：把"**基线不回归**"写成**数字断言**（本文 §0.3 的 477/10/0 与 `+3` 的逐条定位），而不是"跑一遍看看"
- **退出条件**：① A 类清单的每条都有测试；② 基线数字与 §0.3 一致；③ **这一步之前不得开始 P6.03/P6.04 的任何删除**
- **提交标题**：`[P6-01] the current behaviour is frozen before anything is deleted`

### P6.02 B/D 类滚动测试补齐（含 `E-ACC-1` 的行级覆盖）

**上游**：V2 §33.6 第 2 步、§30.1–§30.6 ｜ **第一性原理**：**同步自 P6.01**（破坏性改动没有兜底 = 不可验证的重写）｜ **前置**：P6.01 ｜ **可并行**：无 ｜ **批次**：`[P6-A]` ｜ **层级/分类**：L1–L4 / B + D ｜ **复杂度**：L ｜ **阻塞**：无

- **RED**：`every_matrix_row_maps_to_at_least_one_test()`（元测试：逐行核对 §13 的映射表，缺一行就红）。**预期失败原因**：尚有未落成测试的行
- **GREEN**：把 §13 映射表里标"待补"的行全部落成测试；**失败场景（D 类 23 行）不得少于成功场景（B 类 41 行）的 50%**（实际 **56.1%**，见 **§13.8**）
- **REFACTOR**：把"**测试命名表达行为而不是函数名**"（§2.4）落实为一次全量重命名审查
- **退出条件**：① 映射表每行都有测试；② D:B ≥ 50%；③ 所有真实桌面用例都是 `#[ignore]` 或显式环境断言
- **提交标题**：`[P6-02] every matrix row has a test, and the failures outnumber the successes`

### P6.03 执行删除清单 D-1…D-15

**上游**：V2 §33.1、§33.6 第 3 步 ｜ **第一性原理**：**AGENTS.md 第 1/2 条**（开发期允许破坏性改动，但**每一段删除都必须填出三段式**）｜ **前置**：P6.02 ｜ **可并行**：不可与 P6.04 并行（两者会改同一批文件）｜ **批次**：`[P6-B]` ｜ **层级/分类**：L1 + L2 / A + D ｜ **复杂度**：L ｜ **阻塞**：无

- **做法**：**逐条**执行，每条一个提交（或每 2–3 条一个提交，按相关性），**每条提交信息必须引用 V2 §33.1 对应行的三段式**（当前抽象导致 X → 根本原因 Y → 因此删除 Z）。**不写三段式的不删**。
- **RED（每条）**：删除前，先确认"这条删除**不是**为了好看" —— 对 **D-2** 而言，RED = `every_capture_state_variant_is_reachable()`（今天 `Adjusting` 不可达 → 红）
- **GREEN（每条）**：删除/改写；**D-14 与 D-15 分别在 P6.05/P6.06 单独做**（它们不是"删代码"而是"改测试形态"与"改注释"）
- **退出条件**：① D-1…D-13 全部完成；② 每次删除后 `cargo test --workspace --lib` 全绿（**规则 C6**）；③ 提交信息里能 `grep` 到三段式
- **提交标题**：`[P6-03] the deletions that could explain themselves`

### P6.04 执行重构清单 R-1…R-7 与移动清单（`top_level_provider` 改名）

**上游**：V2 §33.2、§33.4、§33.5 ｜ **第一性原理**：**AGENTS.md 第 2/5 条**（根因优先、正确性优先）；R-6 的前提是"**假的不变式比没有更危险**" ｜ **前置**：P6.03 ｜ **可并行**：无 ｜ **批次**：`[P6-B]` ｜ **层级/分类**：L1 + L2 / A + B ｜ **复杂度**：L ｜ **阻塞**：无

- **RED（R-6 为例）**：`the_context_owner_assertion_is_the_invariant_we_actually_hold()`（P0.02 的用例升级为**不变式**；若 P0.02 显示今天就 panic，则本任务必须先落 deferred context，见 `E-THREAD-1`）
- **GREEN**：R-1…R-7 逐条（其中 R-1/R-2/R-3/R-4/R-5 的主体已在 P1–P4 实现，本任务做的是**收口**：删除旧路径、删除死代码、确认只有一个实现）；改名 `crates/snapclip-capture/src/windows/window_detection.rs` → `windows/top_level_provider.rs`（解决与根 `window_detection/` 的**同名双模块**）
- **REFACTOR**：**核对 §33.5 的保护清单**逐项未被触碰（五端口 / 四条线程 / `graphics_released` / mailbox generation 语义 / 门禁既有规则 / `ui.rs` / `hit_test` p95 / 9 个 `#[ignore]`）
- **退出条件**：① R-1…R-7 全部完成且有测试；② 保护清单逐项 `git diff` 核对无改动；③ 改名后所有引用更新（`cargo check` 干净）
- **提交标题**：`[P6-04] the refactors that replaced an assumption with a fact`

### P6.05 D-14：修复 3 处静默跳过 + 统计脚本

**上游**：V2 §33.1 D-14、§29.2、ADR-16 ｜ **第一性原理**：**"存在但不可见"与"不存在"等价**（静默 `return` 让门禁变成假的）｜ **前置**：P6.02 ｜ **可并行**：与 P6.06（组 **G14**）｜ **批次**：`[P6-B]` ｜ **层级/分类**：L1（脚本）/ A + D ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`scripts/count-unignored-desktop-tests.ps1` 先报出 **3 处**（`windows/win/bitblt.rs:128-145`、`windows/providers.rs:684-747`、`windows/window_detection.rs:121-325` 的 `desktop_available()` 式静默返回）。**预期失败原因**：脚本未实现
- **GREEN**：三处改成**两种合法形态之一**：`#[ignore]`（+ 忽略原因与恢复条件）**或**显式环境断言（失败时给出环境原因）；脚本输出"非 `#[ignore]` 的真实桌面用例数 == 0"
- **REFACTOR**：把脚本挂进 §4.3 的 pre-push 钩子（**门禁化**，否则它会再次退化）
- **退出条件**：① 脚本输出 == 0；② 三处都有"为什么忽略/为什么断言"的文字；③ 钩子包含该脚本
- **提交标题**：`[P6-05] a skipped test is no longer indistinguishable from a passing one`

### P6.06 D-15：6 处失效架构注释

**上游**：V2 §33.1 D-15、§28.2 ｜ **第一性原理**：**注释是负资产**（重构只改代码不改注释 → 新读者按注释找到**不存在的模块**）｜ **前置**：P6.04（改名必须先完成）｜ **可并行**：与 P6.05（G14）｜ **批次**：`[P6-B]` ｜ **层级/分类**：L1 / A ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：`no_comment_references_a_module_that_does_not_exist()`（脚本：对每条注释里出现的 `crate::...` 路径做存在性检查）。**预期失败原因**：今天 6 处失败
- **GREEN**：逐条改写（`windows/mod.rs:13` 的 `crate::application::capture_service`、`window_detection/mod.rs:5` 的已删除转发层、`window_detection/mod.rs:33` 的"只有 `TargetKind::TopLevelWindowFrame`"（**已是假的**：`ClientArea`/`UiElement` 已在 `uia_provider.rs:905-907`、`msaa_provider.rs:202-204` 产出并被 `overlay/hover.rs:139` 消费）、`win/d3d11.rs:250-253` 的"唯一 GPU→CPU 传输、只发生一次"（**已是假的**：回读已改惰性 `providers.rs:91-122`）、`apps/snapclip/Cargo.toml:9-11`、`windows/mod.rs:7`、以及 `ports.rs:19,41,55`（"Tauri command threads"/"The Vue toolbar" —— **Tauri 与 Vue 已在本仓库删除**））
- **退出条件**：① 脚本输出 0 处失效引用；② 每条改写后的注释都指向**真实存在**的模块/职责
- **提交标题**：`[P6-06] the comments stop pointing at modules that no longer exist`

### P6.07 门禁第二遍扫描（§28.4）：`scroll/` 必须保持平台无关

**上游**：V2 §28.4、G9、ADR-10 ｜ **第一性原理**：**G9**（核心可测且不依赖真实桌面）—— 唯一能**机械保证**它的是门禁，而不是纪律 ｜ **前置**：P6.04 ｜ **可并行**：与 P6.05/P6.06（G14）｜ **批次**：`[P6-B]` ｜ **层级/分类**：CI（脚本）/ A ｜ **复杂度**：S ｜ **阻塞**：无

- **RED**：把 §28.4 的扫描片段加进 `tools/check-dependency-direction.ps1` 后，**先故意在 `scroll/` 里加一行 `use crate::windows::...`** → 门禁必须**失败**（证明扫描有效）。**预期失败原因**：扫描尚未加入 → 门禁通过（假绿）
- **GREEN**：保留扫描、删除那行试探代码 → 门禁通过
- **REFACTOR**：把"**门禁必须被证明能失败**"写成规则（一个从不失败的门禁等于没有门禁 —— 与 `E-ACC-1` 的"故意先开一层让错误率 > 0"是同一个道理）
- **退出条件**：① 一次**被拒绝**的证据 + 一次通过；② 钩子里包含该门禁；③ `cargo tree -p snapclip-capture -e normal` 的包数与其历史一致（**无新增依赖**，N7）
- **提交标题**：`[P6-07] the platform-free rule is enforced, and the gate is proven to fail`

### P6.08 `E-PERF-1..4` 回填 §23.3 + 九实体命名一致性检查

**上游**：V2 §23.3、§2.2、§35 P6.5/P6.6 ｜ **第一性原理**：**G7**（性能目标必须可测量可复现）+ **Occam**（实体名是**接口**的一部分；两个名字会各自长逻辑，D-6）｜ **前置**：P4.07、P5.06 ｜ **可并行**：无 ｜ **批次**：`[P6-B]` ｜ **层级/分类**：L1（脚本）+ L4 / A + E ｜ **复杂度**：M ｜ **阻塞**：无

- **RED**：`no_performance_cell_still_says_pending()`（断言 `docs/30 §23.3` 不再含"待测"）+ `no_legacy_entity_name_survives()`（`grep` 旧名：`ScrollFrame`/`Alignment`/`DriverCommand`/`DriverEvent`/`PreviewPatch`/`PreviewState`/`ScrollTile`/`ScrollExportMeta`/`ScrollArtifactWriter`/`Adjusting`）。**预期失败原因**：还有残留
- **GREEN**：把 `E-PERF-1..4`/`E-MEM-1`/`E-CTRL-1`/`E-CAP-1`/`E-INJECT-1`/`E-ACC-1`/`E-DYN-1`/`E-THREAD-1` 的产物填回 `docs/30` 对应位置；**未取得的项目如实写"未取得 + 原因"**（不编造，用户 §44-9/§44-10）
- **REFACTOR**：把九实体（`Observation`/`Axis`/`Displacement`/`RecoveredImage`/`BandStore`/`FrameSource`/`ScrollActuator`/`ScrollLoop`/`PreviewStream`/`ScrollSession`）的口径做成**一次 `grep` 就能核对**的清单
- **退出条件**：① §23.3 无"待测"（或每处"未取得"都有原因）；② 旧名残留 == 0；③ 每个数字都能指回 `E-*` 的产物
- **提交标题**：`[P6-08] the numbers come back, and the names are one`

### P6.09 阶段收口：验收矩阵逐条核对 + 标签

**上游**：V2 §34.1–§34.6、§35；本文 §4.2 阶段级门禁 ｜ **第一性原理**：**用户 §42**（验收标准必须能回答六组问题）+ **§41**（破坏性改动必须说明"为什么能从根源上解决问题"）｜ **前置**：P6.01–P6.08 ｜ **可并行**：无 ｜ **批次**：`[P6-C]`（收口提交**只改文档，不改代码**，规则 C8）｜ **层级/分类**：阶段级（含 L3/L4）/ A ｜ **复杂度**：M ｜ **阻塞**：无

- **做法**：跑**阶段级门禁**（§4.2：`cargo test -p snapclip-capture --lib -- --ignored`、`cargo test --release --lib -- --ignored perf`、`cargo test -p snapclip-app --features test-support --test ui`）；逐条核对 V2 §34 的六组验收；回填 `docs/30 §34.6` 的最后一问（"**如果今天从零开始，只知道基本事实，为什么仍然会得到这个架构？**"—— 若答不出，说明仍有历史包袱，**必须写出来**）
- **退出条件**：① 六组验收逐条有"满足/未满足 + 证据"；② `docs/30 §34.6` 有答案；③ `git tag -a scroll-p6` + `git push origin main --tags`；④ 本文所有任务的 `[x]` 状态更新完毕
- **提交标题**：`[P6-09] scroll capture v2 is accepted against its own criteria`

**P6 退出条件（阶段级）**：① 没有任何非 `#[ignore]` 的真实桌面用例；② 依赖方向门禁干净（含 §28.4 的新扫描）；③ 性能回填完成；④ 命名一致；⑤ `scroll-p6` 标签已打且已推送。

---

## 13. 测试矩阵映射（V2 §30 的每一行 → 本文的任务）

**这张表的作用**：V2 §30 是一份**需求**；本文是**排期**。**只有每一行都能指到一个任务（且那个任务真的会写这条测试），V2 §30 才不是愿望清单。** `P6.02` 会用一个元测试（`every_matrix_row_maps_to_at_least_one_test()`）守住这张表。

**列的含义**：`场景` 取 V2 §30 的原文措辞（不重写）；`任务` 是**主责**任务（可并列）；`分类` 用用户 §31 的 A/B/C/D（A 原有功能 · B 新功能 · C 边界 · D 失败）；`层级` 用本文 §2.2 的 L1–L4（`CI` = 钩子/脚本级）。

### 13.1 Capture（V2 §30.1，12 行）

| # | 场景 | 任务 | 分类 | 层级 |
|---|---|---|---|---|
| 1 | 窗口级 WGC 捕获 Chrome | `P2.01` + `P0.05` | B | L3 |
| 2 | 窗口级 WGC 捕获 Edge/Electron/WebView2/记事本 | `P2.01` + `P0.09` | B | L3 |
| 3 | 会话内 pool 复用（100 步 `Recreate == 0`） | `P2.01` | B | L2 |
| 4 | 选项不可用（mock `IsBorderRequired` 失败） | `P2.04`（**DEV-3**） | D | L2 |
| 5 | 目标被完全遮挡（仍取目标内容） | `P2.01` + `P2.06` | B | L3 |
| 6 | 目标最小化 | `P2.07` | D | L2 |
| 7 | 目标尺寸变化 | `P2.05` + `P2.07` | D | L2 |
| 8 | 目标跨显示器移动（尺寸与 DPI 不变） | `P2.05` | B | L3 |
| 9 | 非目标显示器拓扑变化（会话继续） | `P2.05` | B | L2 |
| 10 | 目标显示器 DPI 变化 | `P2.05` | D | L2 |
| 11 | 设备丢失（mock） | `P2.06` + `P3.04` | D | L2 |
| 12 | 每步仅一次回读（100 步计数 == 100） | `P2.02` | B | L1+L2 |

### 13.2 Scroll（V2 §30.2，10 行）

| # | 场景 | 任务 | 分类 | 层级 |
|---|---|---|---|---|
| 1 | `SendInput` 生效 | `P3.01` | B | L3 |
| 2 | `PostMessageW` 生效 | `P3.01` | B | L3 |
| 3 | Chromium 子窗口下沉 | `P3.01` | B | L3 |
| 4 | UIPI 目标（提权记事本） | `P3.02` + `P0.09` | D | L3 |
| 5 | 条件选择（前台/非前台 × 提权/非提权 = 4 组） | `P3.02` | B | L2 |
| 6 | 注入失败后切路径（连续 3 次 `Posted` 但 `d == 0`） | `P3.03` | D | L2 |
| 7 | 两条路径都失败 | `P3.03` | D | L2 |
| 8 | 平滑滚动等待（内容持续移动 3 帧） | `P3.04` | B | L2 |
| 9 | 取消延迟（Max ≤ 500 ms） | `P3.07` | B | L4 |
| 10 | 停止延迟 | `P3.07` | B | L4 |

### 13.3 Matching / Offset（V2 §30.3，16 行）

| # | 场景 | 任务 | 分类 | 层级 |
|---|---|---|---|---|
| 1 | 整数位移（正/负，`|d| ∈ 1..40`） | `P1.05`/`P1.06`/`P1.07` + `P1.24` | B | L1 |
| 2 | 大位移（`|d| = 100, 500`） | `P1.06` + `P1.24` | B | L1 |
| 3 | 位移 == 视口（边界，接受） | `P1.08` | B | L1 |
| 4 | 位移 > 视口（拒绝，门一） | `P1.08` | D | L1 |
| 5 | 纯色帧 | `P1.06` + `P1.12` | D | L1 |
| 6 | 周期纹理（`P == |d|`） | `P1.14` | D | L1 |
| 7 | 周期纹理（`P ≠ |d|`） | `P1.14` | D | L1 |
| 8 | 二维周期（棋盘，峰族） | `P1.14` + `P1.11` | D | L1 |
| 9 | 低纹理（渐变，tile 跳过） | `P1.16` | D | L1 |
| 10 | 逐行完全相等（`Skip`，不进估计器） | `P1.18` | B | L1 |
| 11 | 边界值 `±N/2`（拒绝，`status = None`） | `P1.07` | D | L1 |
| 12 | 门限校准（ROC 扫描） | `P1.13` + `P0.03` | B | L1 |
| 13 | 逐门 ablation | `P1.13` | B | L1 |
| 14 | P1 自锁（`ĝ` 初值错 2×，100 步） | `P1.14` | D | L1 |
| 15 | `scene_cut` ×1–2（继续） | `P1.12` | D | L1 |
| 16 | `scene_cut` ×≥3（衰减 + 继续） | `P1.12` | D | L1 |

### 13.4 Stitching（V2 §30.4，12 行）

| # | 场景 | 任务 | 分类 | 层级 |
|---|---|---|---|---|
| 1 | 覆盖不变量（二维完整） | `P1.17` | B | L1 |
| 2 | 旧像素优先（逐字节） | `P1.18` | B | L1 |
| 3 | 参照系无漂移（100 步逐行相等） | `P1.19` | B | L1 |
| 4 | `band_height` 推导（`overlap ≥ extent/4`） | `P1.18` | B | L1 |
| 5 | 双向扩展（`Append` + `Prepend`） | `P1.19` | B | L1 |
| 6 | `Contained`（本帧完全落在已覆盖区） | `P1.19` | B | L1 |
| 7 | 撤销一步 | `P1.22` | B | L1 |
| 8 | 条带换出（把预算注入成 1 个条带） | `P1.20` | B | L1 |
| 9 | 上限三层（`MAX_LONG_IMAGE_PIXELS` 注入 1/10 → 合法 PNG） | `P1.21` + `P4.05` | B | L1+L2 |
| 10 | 流式导出（乱序返回错误） | `P4.01` + `P4.05` | B | L2 |
| 11 | 水平轴 | `P1.02` + `P1.24` + `P0.03` | B | L1 |
| 12 | 轴映射（`T-AXIS-1`，`(dx,dy)` 全组合） | `P1.02` | B | L1 |

### 13.5 Browser（V2 §30.5，9 行）→ 全部归 **C 类（边界）**

| # | 场景 | 任务 | 分类 | 层级 |
|---|---|---|---|---|
| 1 | 30,000 px 合成本地页面（与 DOM 高度×DPR 逐行相等） | `P1.24` + `P2.01` + `P3.01` | C | L3 |
| 2 | `position: fixed` 头部只出现一次 | `P1.16` + `P1.24` | C | L3 |
| 3 | CSS 动画元素（降权，`Confirmed` ≥ 95%） | `P1.16` | C | L3 |
| 4 | `<video>` 播放中 | `P1.16` | C | L3 |
| 5 | 懒加载图片序列 | `P1.16` + `P1.21` | C | L3 |
| 6 | 无限滚动 | `P1.21` | C | L3 |
| 7 | 中途切窗口再切回 | `P2.05` | C | L3 |
| 8 | 页面缩放（Ctrl+滚轮改变 `devicePixelRatio`） | `P1.08` | C | L3 |
| 9 | 滚动条在选区内 | `P1.17` | C | L3 |

### 13.6 UI（V2 §30.6，9 行）

| # | 场景 | 任务 | 分类 | 层级 |
|---|---|---|---|---|
| 1 | 八问全部可答 | `P5.03` | B | L2 |
| 2 | 视口框三态 | `P5.03` | B | L2 |
| 3 | 视口框下限（100,000 px 画布，框高 ≥ 4 DIP） | `P5.03` | B | L2 |
| 4 | 预览窗口化（不生成整图缩略） | `P5.02` | B | L1 |
| 5 | 拖动后停止跟随 | `P5.04` | B | L2 |
| 6 | 预览不阻塞采集 | `P5.06` | B | L2 |
| 7 | 更新频率（≤ 10 Hz） | `P5.01` | B | L1 |
| 8 | 主线程（覆盖层线程）同步工作 ≤ 8 ms | `P5.06` | B | L4 |
| 9 | 布局不遮挡操作（真实渲染回读） | `P5.05` | B | L3 |

### 13.7 Memory / Regression（V2 §30.7，10 行）

| # | 场景 | 任务 | 分类 | 层级 |
|---|---|---|---|---|
| 1 | 三档长度内存（10k/30k/100k，`peak` 差异 ≤ 10%） | `P4.07` | D | L4 |
| 2 | 换出文件已删除 | `P4.06` | D | L1 |
| 3 | 导出拷贝数（≤ 2 份） | `P4.03` | D | L2 |
| 4 | `u32` 越界（拒绝不截断） | `P4.04` | D | L2 |
| 5 | 普通截图回归（F5 全流程） | `P6.01` | A | L3 |
| 6 | 普通截图延迟不受滚动影响（P95 变化 ≤ 10%） | `P6.01` + `P4.07` | A | L4 |
| 7 | 捕获路径不变（`attempt_order` 行为一致） | `P6.01` + `P2.06` | A | L1 |
| 8 | 依赖门禁（含 §28.4 的 `scroll/` 纯度扫描） | `P6.07` | A | CI |
| 9 | 无可达状态负债（删 `Adjusting` 之后） | `P6.03` | D | L1 |
| 10 | 无静默跳过（脚本统计 == 0） | `P6.05` | A | CI |

### 13.8 分类与层级合计（**逐行统计，与 V2 的近似值对照**）

| 项 | 本文逐行统计 | V2 §30 末尾的近似 | 差异原因 |
|---|---|---|---|
| **B（新功能）** | **41** | "约 55" | V2 把 §30.5 的 9 行浏览器用例算进了"新功能"；本文按用户 §31 的口径把它们归 **C（边界）**，并把 §30.7 的 5 行归 A/D |
| **C（边界）** | **9** | 9 | 一致 |
| **D（失败）** | **23** | "约 30" | V2 把 §30.3 的几行"边界"与 §30.7 的回归行算进了错误场景；本文严格按"**输入是异常/失败，还是正常但难**"区分 |
| **A（原有功能，矩阵内）** | **5** | — | 另有约 15 条**矩阵外**的既有回归测试（`P6.01` 的清单）→ A 合计 ≈ 20 |
| **D : B** | **23 / 41 = 56.1%** | "接近 2:3" | **仍 ≥ 50%**：用户 §31"失败场景必须和成功场景同等重要"这条要求**被数字执行** |
| 层级 | L1 = 32 · L2 = 16 · L3 = 24 · L4 = 2 · L1+L2 = 1 · L1+L3 = 1 · CI = 2 | 同 | 与 V2 §30 一致 |

**这一节的自我检查**：**每一行都有主责任务**（`P6.02` 的元测试会守住它）；**每一行的层级都不高于它的依赖**（例如 30.3 的全部行都是 L1，因为它们只需要合成夹具，不需要 GPU/桌面）。

### 13.9 三个硬要求（写测试时的**前置知识**，漏了会造成假红/假绿）

1. **导出后解码比对的测试，在产物 > 64 MiB 时必须显式提高 `png::Limits{bytes}`**，否则会**假失败**（F-10）。受影响：`P1.21`、`P4.02`、`P4.05`、`P1.24` 的大长度档。
2. **内存测试必须同时记 `allocated` 与 `peak`，并单独记换出文件大小**（参考项目 `benchmark-support/README.md` 的教训："堆流量不能证明空间下降（存储搬到 OS 映射时）"）。受影响：`P4.07`、`P5.02`、`P1.20`。
3. **性能断言只能建立在 `E-PERF-*` 的产物上**（AGENTS.md 第 6 条：性能优化必须有依据）。在 `P0.03`/`P0.04`/`E-PERF-3` 完成前，**任何 `⏳` 格子不得填数字**（用户 §44-9/§44-10）。

### 13.10 与既有测试的关系（**不重写、不删除、只补齐**）

| 既有资产 | 处置 | 依据 |
|---|---|---|
| 9 个既有 `#[ignore]` | **保留**（加注"需要真实交互桌面"与恢复条件） | §30.7 第 5 行；V2 §29.6 |
| 3 处静默跳过（D-14） | **改成两种合法形态之一**（`#[ignore]` 或显式环境断言），由 `P6.05` + 脚本守住 | V2 §29.2（"存在但不可见"与"不存在"等价） |
| `crates/snapclip-capture/src/windows/hit_test.rs:435` 的 `p95 < 0.1 ms` | **保留不动**（"跟手"的已有证据） | V2 §23.3 的六子项 |
| `apps/snapclip/tests/ui.rs` | **保留**；滚动会话**不进**这个文件（它是 shell 的 GPUI 测试，滚动预览在覆盖层线程） | V2 §29.6 + `b45` 的所有权裁决 |
| `crates/snapclip-capture/src/windows/win/d2d/tests.rs` | **同型新增一条**（覆盖层 + 预览面板一起渲染后回读）= `P5.05` | V2 §30.6 第 9 行 |

---

## 14. 风险、开放问题与阻塞处置

### 14.1 开放问题 → 任务（逐条，**没有一条会被"跳过"**）

| OQ | 问题（V2 §36.2 原文口径） | 状态 | 裁定任务 | 若结论不利 |
|---|---|---|---|---|
| **OQ-1** | `PostMessageW(WM_MOUSEWHEEL)` 能否驱动 Chromium？ | ✅ **本机已答：能**（`E-INJECT-1` 最小版 800 px / 8 notch，§24.6.1） | `P0.06` ✅ | — |
| **OQ-2** | `WDA_EXCLUDEFROMCAPTURE` 对 WGC 是否生效？ | 未定 | `P0.05`（扩一条） | 只影响**显示器级回退路径**的措辞，不影响窗口级主路径 |
| **OQ-3** | `SendInput` 在 UIPI 场景下的方向性 | 未定（官方两页矛盾） | `P0.09` | 若两条路径都不通 → UIPI 目标在 v1 只能**提示用户以管理员运行**（与 PixPin 一致） |
| **OQ-4** | 125%/150%/175% 下"整数物理像素位移"是否成立？ | **本机取不到**（`PixelRatio: 1`） | `P0.04` 记"未取得"；`P1.08`/`P1.24` 保留"累计小数余量"的**接口空间**（不加实现） | 若位移非整数 → 门一与"整宽行带"模型需重评（**这是 V2 里最可能被推翻的一条**） |
| **OQ-5** | `SPI_GETMOUSEWHEELROUTING = MOUSE_POS(2)` 时非前台窗口能否收到 `SendInput`？ | 本机实测 `2`，但**是用户可改的设置** | `P0.07` 读取 + `P0.09` 两种设置各跑一组 | §24.6 的"非前台 → 必须 `PostMessageW`"可能过严（只会**更保守**，不会更危险） |
| **OQ-6** | 水平轴到底慢多少？ | 无任何公开数据 | `E-PERF-3`（`P0.03` 的兄弟实验） | 若慢到不可接受 → 评估"列优先临时转置"（`spike/transposed-canvas`），**先不加实现** |
| **OQ-8** | 三层漏斗会不会太慢？ | 未测 | `P0.03` `E-PERF-1` | 加第 4 层（多尺度）→ 侧分支 `spike/matcher-layer4` |
| **OQ-9** | `MAX_DECODE_PIXELS = 24 MP` 是否放宽？ | 未定 | `P4.07` 的产物 + 一次评审 | 若长图必须可读回 → **新增**"分块解码"工作（本文不排，因为它是新范围） |
| **OQ-10** | 长图是否进历史库？ | **产品决策**，未定 | `P6.09` 收口时向所有者提问 | 不进历史 ⇒ 长图"导出即结束"（本文的默认口径） |
| **OQ-11** | 29,000 px 提示阈值是否合适？ | 纯 UI 参数 | `P5.02` + 一次 29,000 px 可读性评审 | 随时可改，**不阻塞任何任务** |
| **OQ-12** | `E-DYN-1`（稀疏光流）要不要保留？ | 未测 | `E-DYN-1`（归 `P1.16` 的扩展） | **不改善错误确定率就删除**（AGENTS.md 第 6 条） |
| **OQ-13** | DXGI `GetFrameMoveRects` 是否作为显示器级回退的位移来源？ | **不阻塞** | 记录在案；仅当 P2 之后仍需"显示器级高质量"才评估 | 不做（v1 主路径走 WGC，用不到） |

**OQ-7 与 OQ-14 已由 `docs/30 §36.1` 的 D-1/D-2 裁决收口**（ORB 自写；`docs/23`/`docs/24` 修正），**不再是开放问题**。

### 14.2 `[!]` 阻塞任务与侧分支（与 §3.4 的表是同一集合的两半）

**规则（本文 §3.4 的落地）**：`[!]` 任务**不通过则不得进入依赖它的阶段**。**不允许**"先把后面做完、回头再修" —— 那正是 V1 的失败方式（`docs/19` 的 §7.2 与 §6.7 自相矛盾到 v5 才被发现）。

| `[!]` | 触发条件 | 侧分支名 | 侧分支内容 | 影响面 |
|---|---|---|---|---|
| `P0.02` | `T-THREAD-1`：**库测试全绿但生产路径会 panic**（2026-10-08 实测，见 §6 `P0.02` 的结论块）——触发的是触发条款的实质而非字面（字面是"全量测试 panic"） | **`blocked/P0-02-context-owner`**（装被阻塞的断言代码，✅ 已建并推送）+ `spike/deferred-context`（留给第 ② 步实验，未开始） | 断言 + 两个用例；第 ② 步按 V2 §21.3 **优先用 deferred context**（"不改任何人线程"的方案），把"移线程"作为最后手段 | `P2.02` 的回读路径、`P2.01` 的放大镜采样 |
| `P0.03` | `E-PERF-1` 的三层漏斗 P95 超过 §23.3 的阈值 | `spike/matcher-layer4` | 加第 4 层（多尺度 / 预降采样）或把 ORB 提到主路径（与 `P1.23` 互换主次） | `P1.05`–`P1.13` |
| `P0.05` | 窗口级 WGC 在 Electron/WebView2 上不可用（或 `CreateForWindow` 不接受子窗口） | `spike/monitor-fallback` | 显示器级为主 + WDA/覆盖层隐藏 + "遮挡下取到遮挡者"的用户提示 | `P2.01`、C5、§24.2 |
| `P1.24` | `E-ACC-1` 打完四门后"错误确定率"仍 > 0 | `spike/orb-primary` | ORB 从"第二意见"提为主候选（`P1.23` 的角色反转），保留四门为**验证**层 | `P1.05`–`P1.16`、§15.4 |

**两处表的口径**：§3.4 列出的是"**因环境/设备/官方依据不足**而阻塞"的四项（`P0.05`、`P1.22` 的 DPI 行、`P3.02` 的 UIPI 组、`P3.09` 的非前台判据），本表列出的是"**因实验结论**而阻塞"的四项；两表并集即本阶段的全部 `[!]`，`P0.02` 由本次执行新增进 §3.4 的表。

### 14.3 风险表

| # | 风险 | 概率 | 影响 | 缓解 | 责任任务 |
|---|---|---|---|---|---|
| R-1 | **上限取值定错**（V2 用 `u32::MAX/2` 且**没有硬失败上限**） | 中 | 高（用户拿到一个"以为会一直拼"的会话） | `P1.21` 的三层 + `P4.05` 的"`Partial` 仍合法" + UI 可见计数器 | `P1.21`/`P5.03` |
| R-2 | **改 `context` 使用者引入普通截图回归** | 中 | 高（A 类全绿是硬门槛） | **先加断言、再考虑 deferred context、最后才搬线程**（`P0.02` → `spike/deferred-context`）；`P6.01` 先冻结行为。**2026-10-08 实测后收紧**：断言已证明有效，而且**今天就会在生产路径 panic**（放大镜取色），因此断言不进 `main`，代码在 `blocked/P0-02-context-owner`；**"库测试全绿"不再被当作不变式成立的证据**（见 §6 `P0.02` 的状态行） | `P0.02`/`P2.02`/`P6.01` |
| R-3 | **端口/tile 形状冻结错误**（像 V1 §8.3 那样自相矛盾） | 低 | 高 | `P1.21`/`P4.01` 的签名由**类型**约束（height 先于行） | `P1.21`/`P4.01` |
| R-4 | **一帧匹配失败即终止**（V1 §7.2 的错误） | 低 | 高 | `P1.12`/`P3.04` 的 `Uncertain → 继续`；`P6.02` 守住 §30.3 第 15/16 行 | `P1.12`/`P3.04` |
| R-5 | **overlay 输入模型未验证**（覆盖层是否抢焦点/是否被捕获） | 中 | 中 | `P3.01`+`P5.03` 的实测；已知杠杆：`HTTRANSPARENT`、**覆盖层今天故意不用 `WS_EX_NOACTIVATE`** | `P3.01`/`P5.03` |
| R-6 | **只留一条注入路径** | 低 | 中 | `P3.01` 两条**并列**；`OQ-5`/`OQ-3` 收口 | `P3.01`/`P3.02` |
| R-7 | **缺"候选不唯一"的量化门限** | 中 | 高 | `P1.11` 的 `margin ≥ 0.15` + `P1.14` 的峰族检测 | `P1.11`/`P1.14` |
| R-8 | **`docs/23` 与仓库状态不一致**（历史文档过期） | 已发生 | 中 | `docs/30 §36.4` 的修正台账已完成；本文 §13.10 不再依赖那份状态 | — |
| R-9 | **未测基线被当基线引用** | 已修正 | 中 | `P0.01` 已实测 **477 / 10 / 0**（§0.3）并定位 `+3` 的来源 | `P0.01` ✅ |
| R-10 | **参考项目证据只存在于会话中** | 已修正 | 中 | 已落盘 `docs/26`–`docs/29`；本文 `RES-1`–`RES-9` 把"外网调研"也变成产物 | `RES-*` |
| R-11 | **性能数字被编造**（用户 §44-9/§44-10） | 中 | 高 | 本文的 `⏳` 标记 + §13.9 第 3 条 + `P6.08` 的"无 `待测` 残留"断言 | `P6.08` |
| R-12 | **破坏性改动缺少兜底**（用户 §2 的硬要求） | 中 | 高 | `P6.01` → `P6.02` → 删除/重构的**严格顺序**；`C6` 规则（每段改动后全量 lib 测试） | `P6.01`/`P6.02` |
| R-13 | **`.githooks` 从未启用**（今天 `core.hooksPath` unset） | 已确认 | 中 | `P0.08` 强制创建 + 安装 + **验证生效**（故意让一次提交失败） | `P0.08` |
| R-14 | **整 crate `cargo fmt` 造成无关重排** | 已发生 | 中 | 本文 §2.1：**禁止整 crate fmt**，只格式化自己改动的区域；`C11` 规则 | 全局 |
| R-15 | **任务清单本身变成愿望清单** | 中 | 中 | §13 的逐行映射 + `P6.02` 的元测试"缺一行就红" | `P6.02` |
| R-16 | **L3 门禁并行跑会假红**（7 个真实桌面用例抢前台，`scroll_probe.rs:404` 的前台断言先失败） | 已发生（2026-10-08） | 中（会把串行才能过的门禁误判为代码问题） | §4.2 的命令固定加 `--test-threads=1`；后续新增 L3 用例时**不要**用"抢前台"作为前置，改用 `PostMessageW` 或把自己的窗口设为前台后立即测量 | `P0.05`/`P0.09`/所有 L3 任务 |

### 14.4 失败处置与回滚

| 情形 | 处置 |
|---|---|
| `[!]` 任务结论不利 | 走 §14.2 的侧分支；**在侧分支上继续按 TDD 走**，不修改主干任务的判据 |
| 某任务卡住 > 预计复杂度 | **拆任务**（在 §7–§12 里插入 `<阶段>.<NN>a`），**不降低 TDD 要求** |
| 阶段门禁失败 | 回滚到该阶段**最后一个绿色提交**（`P0.08` 会先给出可回滚的标签点），修好再前进 |
| 已验证的基线被污染（例如误跑了整 crate fmt） | 按 §3.6 的四步回滚演练处置；**禁止**用 `git add -A` 掩盖无关改动（`C9`） |
| 用户中途改变产品口径（例如 OQ-10 决定"长图进历史"） | 只新增任务，**不改已完成任务的判据**；受影响的 `docs/30` 章节由 `P6.09` 统一回填 |

---

## 15. 附录

### 15.1 文件清单

**新建（13 个）**

| 文件 | 任务 | 内容 |
|---|---|---|
| `crates/snapclip-capture/src/scroll/mod.rs` | `P1.03` | 模块门面（约 40 行，只做导出与文档） |
| `crates/snapclip-capture/src/scroll/observation.rs` | `P1.03` | `Observation`、只读视图、几何 |
| `crates/snapclip-capture/src/scroll/displacement.rs` | `P1.04`–`P1.14` | `Displacement`、四门、三层漏斗、P1 先验、三分类 |
| `crates/snapclip-capture/src/scroll/orb.rs` | `P1.23` | 自写 ORB（约 300–400 行，第二意见） |
| `crates/snapclip-capture/src/scroll/canvas.rs` | `P1.17`–`P1.19` | `RecoveredImage`、`CoverageMap`、八不变量 |
| `crates/snapclip-capture/src/scroll/bands.rs` | `P1.20`/`P1.21`/`P1.22` | `BandStore`、换出、上限、撤销 |
| `crates/snapclip-capture/src/scroll/target.rs` | `P2.07` | `ScrollTarget`（含滚动能力与当前位置） |
| `crates/snapclip-capture/src/scroll/loop_control.rs` | `P3.03`–`P3.06` | 闭环、`ĝ`、自检、手动模式 |
| `crates/snapclip-capture/src/scroll/preview.rs` | `P5.01`/`P5.02` | `PreviewStream` 生产者、窗口化缩略 |
| `crates/snapclip-capture/src/scroll/session.rs` | `P3.09` | `ScrollSession` 装配与 teardown |
| `crates/snapclip-capture/src/scroll/testkit.rs` | `P1.01` | **（DEV-1）** test-only 合成夹具生成器与自证 |
| `crates/snapclip-capture/src/windows/scroll_source.rs` | `P2.03` | 窗口级 WGC 多帧 + 初始几何/分辨率/monitor rect |
| `crates/snapclip-capture/src/windows/scroll_actuator.rs` | `P3.01` | 两条注入路径 + 子窗口下沉 |

> **注**：上表 13 行中 `scroll/` 占 11 个文件。**V2 §28.2 已按 DEV-1 回填为"10 个生产文件 + 1 个 test-only 文件（`testkit.rs`）"**，`§33.3` 的文件清单同步由 9 补为 11（原清单漏列了 `orb.rs`）。`P6.08` 只做**校验**，不再需要新增内容。

**修改（不新增文件，全部在既有文件内）**

| 文件 | 任务 | 改动 |
|---|---|---|
| `crates/snapclip-capture/src/windows/win/wgc.rs` | `P2.01`/`P2.04` | `CreateForWindow`、会话级 pool/session/bufferCount=3、`CaptureCapabilities`（**不拆文件**） |
| `crates/snapclip-capture/src/windows/providers.rs` | `P2.02` | `ProviderKind::WgcWindow`、多次 `read_region`、每步一次回读的计数断言 |
| `crates/snapclip-capture/src/session.rs` | `P6.03` | 删 `CaptureState::Adjusting` |
| `crates/snapclip-capture/src/windows/mod.rs` | `P6.04`/`P6.06` | 改名 `window_detection` → `top_level_provider`；删失效注释 |
| `crates/snapclip-capture/src/windows/win/bitblt.rs`、`providers.rs`、`window_detection.rs` | `P6.05` | 3 处静默跳过改成两种合法形态（**注意 `window_detection.rs` 在 `P6.04` 改名后为新路径**） |
| `apps/snapclip/src/capture/artifact_writer.rs` | `P4.03`/`P4.04` | 删 4 份拷贝中的 2 份中间整图、`u32` 越界改为拒绝 |
| `apps/snapclip/src/capture/`（新增 `row_band_png.rs` 之类的实现文件） | `P4.02` | `PngRowBandSink`（**这是 shell 侧唯一的新文件**） |
| `crates/snapclip-capture/src/ports.rs` | `P6.06` | `:19,41,55` 的失效注释（Tauri/Vue） |
| `crates/snapclip-capture/src/windows/win/d2d/tests.rs` | `P5.05` | 同型新增"覆盖层 + 预览面板"回读用例 |
| `tools/check-dependency-direction.ps1` | `P6.07` | 第二遍扫描：`scroll/` 平台纯度 |
| `.githooks/pre-push` | `P0.08` | 新建并通过 `core.hooksPath` 生效 |
| `docs/30-scroll-capture-design-v2.md` | `P6.08`/`P6.09` | 回填性能数字与 OQ 结论（**只改数据与结论，不改设计**） |

**删除**：**没有整文件删除**（D-1…D-15 全部是删代码段/类型/注释，不是删文件）。**`window_detection.rs` 是改名不是删除**。

### 15.2 依赖变更

**结论：无新增依赖**（N7 + AGENTS.md 第 6 条）。

| crate | 变更 | 验证方式 |
|---|---|---|
| `snapclip-capture` | **无**（不新增 `png`、不引入 `imageproc`/`opencv`/`phaseCorrelation`） | `cargo tree -p snapclip-capture -e normal` 包数与 `docs/30 §0.2` 的 30 包一致 |
| `snapclip-core` | 无 | — |
| `snapclip-history` | 无（`png` 走 shell，不进 history） | — |
| `apps/snapclip`（shell） | **无新增 crate**；`png` **已是** shell 的既有依赖，`PngRowBandSink` 复用它 | `git diff --stat apps/snapclip/Cargo.toml` 在 `P4.02` 期间**应为空** |
| workspace | 无 | `cargo check --workspace --all-targets` |

**唯一可能的例外**：`P0.04` 若发现 V2 §17.7 的"必须显式设置压缩/滤波参数"在既有 `image` 版本上不可表达，则需要把 `png` 提升为 shell 的直接依赖 —— **这属于 shell，不违反 N7**，但必须在 `P0.04` 的产物里**写明并说明为什么**。

### 15.3 消息 ID 与常量分配

| 项 | 值 | 任务 | 说明 |
|---|---|---|---|
| `WM_APP + 45` | 滚动会话消息 | `P3.07` | 已占用：`+1`/`+2`/`+17`/`+18`/`+19`/`+43`/`+44`；**必须有冲突断言** |
| `RENDER_TICK_MS` | `15`（既有） | — | `Scroll Response` 阈值 P95 ≤ 80 ms 的推导来源 |
| `INJECT_NOTCH` | `WHEEL_DELTA = 120` | `P3.01` | `mouseData`/`wParam` 的换算基数 |
| `SETTLE_FRAMES` | `2` | `P3.04` | frame-driven settled：连续两帧一致 |
| `PATH_SWITCH_AFTER` | `3` | `P3.03` | 连续 3 步 `Posted` 无效则切换路径（**启动值，待校准**） |
| `PREVIEW_HZ` | `10` | `P5.01` | **启动值，待 `E-PERF-4` 校准** |
| `MIN_RESIDUAL_GAIN` | `0.15` | `P1.09` | 门二（唯一必须通过的硬门） |
| `MIN_TILES` | `4` | `P1.10` | 门三（32 px tile，独立需 `|i−j| ≥ 2`） |
| `MIN_MARGIN` | `0.15` | `P1.11` | 门四 |
| `PRIOR_KAPPA` | `0.5` | `P1.14` | P1 先验区间 `[n·ĝ(1−κ), n·ĝ(1+κ)]` |
| `prior_off_below_px` | `4` | `P1.14` | `n·ĝ < 4 px` 时关闭先验 |
| `ĝ_alpha` | `0.3` | `P3.05` | `ĝ ← 0.7·ĝ + 0.3·(d_k/n_k)` |
| `LONG_IMAGE_WARN_LENGTH` | `29_000` | `P1.21` | **刻意对齐 PixPin**；纯 UI 参数（`OQ-11`） |
| `MAX_LONG_IMAGE_PIXELS` | `u32::MAX / 2` | `P1.21` | **必须可注入**（`OQ-8`/`OQ-11` 的收口方式） |
| `SCENE_CUT_STREAK` | `3` | `P1.12` | ≥3 才衰减模型，**仍继续会话** |

### 15.4 标签与推送计划

| 标签 | 时机 | 内容 |
|---|---|---|
| `scroll-p0` | `P0` 全部完成 | 基线、`T-THREAD-1`、四个实验的最小版、钩子生效 |
| `scroll-p1` | `P1` 全部完成 | 纯逻辑核心 + `E-ACC-1` 门禁 |
| `scroll-p2` | `P2` 全部完成 | 窗口级 WGC 流 + 能力探测 |
| `scroll-p3` | `P3` 全部完成 | 注入 + 闭环 + 取消延迟实测 |
| `scroll-p4` | `P4` 全部完成 | 行带导出 + `E-MEM-1` 三档 |
| `scroll-p5` | `P5` 全部完成 | 预览 + 三项延迟 |
| `scroll-p6` | `P6` 全部完成 | 清理、门禁、回填、验收 |

**推送节奏**：**每个任务一个提交**（`C2`），**每个阶段一次 push**（`C3`）。`P0.08` 的钩子负责在 push 前跑任务级门禁；`P6.09` 负责打最后一个标签并 push `--tags`。

### 15.5 并行依赖图

**唯一来源**：依赖图与关键路径见 **§1.3**；并行组 `G1`…`G14` 与其接口冻结前提见 **§1.4**。**本附录不重复它们** —— 两份依赖图一旦不同步，就会让人按错的那份排期（这正是 V1 文档"同一件事写两遍"的失败方式，见 V2 §4.4）。

### 15.6 调研产出索引（`RES-1`…`RES-9` 的落点）

| 编号 | 主题 | 产出落点 | 影响的任务 |
|---|---|---|---|
| `RES-1` | `WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL`/`WM_POINTERWHEEL` 的坐标空间与注入语义 | `docs/Temp/research/RES-1.md` → `docs/30 §24.6.1` 的补注 | `P3.01`/`P3.02` |
| `RES-2` | UIPI 的单向性、`ChangeWindowMessageFilterEx`、提权目标 | `docs/Temp/research/RES-2.md` | `P3.02`、`OQ-3` |
| `RES-3` | WGC 会话接口 1–7 的版本点（`IsCursorCaptureEnabled`/`IsBorderRequired`/`IncludeSecondaryWindows`/`DirtyRegionMode`） | `docs/Temp/research/RES-3.md` | `P2.04` |
| `RES-4` | DPI 感知（manifest vs API、`PER_MONITOR_AWARE_V2`）与 `GetDpiForMonitor` 的来源头文件 | `docs/Temp/research/RES-4.md` | `P2.05`、`OQ-4` |
| `RES-5` | D2D/DirectWrite 在有界覆盖层上的绘制预算（10 Hz 预览的成本上界） | `docs/Temp/research/RES-5.md` | `P5.06` |
| `RES-6` | 参考实现 `snow-stitch-images`：四门、`MIN_INLIER_TILES`、乘法权重、`Contained`、参照系二态 | `docs/29`（已存在）+ `docs/Temp/research/RES-6.md` 的**只读**摘录 | `P1.09`–`P1.11`、`P1.16`、`P1.19` |
| `RES-7` | 参考实现 `snow-capture`：后端优先级、降级白名单、3 slot 读回、帧池溢出语义 | `docs/29` + `RES-7.md` | `P2.02`、`P2.06` |
| `RES-8` | 参考实现 `snow_shot` C++ 侧：`LatestBridgeMailbox`、自适应节拍、预览 6 态、暂停三段屏障 | `docs/29` + `RES-8.md` | `P3.04`、`P5.01`、`P3.07` |
| `RES-9` | PixPin 运行期证据：单块 `Format_RGB32` 画布、`logicalLength`、tile 128 MiB 预算、29000 阈值 | `docs/28`（已存在） | `P1.20`、`P1.21`、`OQ-11` |

**规则**：`RES-*` 的产物是**可引用的文件**（`docs/Temp/` 下，属 gitignore），正式结论回填 `docs/30`。**不允许**只有会话记录（R-10 的教训）。

### 15.7 与 `docs/30 §35` 的对应表（V2 的粗排 → 本文的细排）

| V2 §35 | V2 一句话 | 本文任务 |
|---|---|---|
| `P1.1` | 合成夹具生成器 | `P1.01` |
| `P1.2` | `Observation`/`Axis`/`Displacement` + `estimate()` 三层漏斗 | `P1.02`–`P1.07` |
| `P1.3` | 四门 + P1 先验 + 三分类权重 | `P1.08`–`P1.16` |
| `P1.4` | `RecoveredImage` + `BandStore` + 不变量 | `P1.17`–`P1.22` |
| `P1.5` | 上限三层 + 导出预算 + 撤销 | `P1.21`/`P1.22` |
| `P1.6` | `E-ACC-1` 落成门禁 | `P1.24` |
| `P1.7` | 自写 ORB | `P1.23` |
| `P2.1` | `wgc.rs` 增 `CreateForWindow` + pool 复用 | `P2.01` |
| `P2.2` | `FrameSource` + 每步一次回读 | `P2.02`/`P2.03` |
| `P2.3` | 能力探测 + 选项失败可见 | `P2.04` |
| `P2.4` | 显示拓扑三档 | `P2.05` |
| `P3.1`–`P3.5` | 注入两路径 / `choose()` / 闭环 / 手动 / 停取消 | `P3.01`–`P3.07` |
| `P4.1a` + `P4.1b` | `RowBandSink`（端口）/ `PngRowBandSink`（实现） | `P4.01` + `P4.02`（**DEV-2 拆分，已回填 V2 `§35 P4`**） |
| `P4.2` | 消除 4 拷贝 + 拒绝越界 | `P4.03`/`P4.04` |
| `P4.3` | 换出文件 `Drop` 清理 | `P4.06` |
| `P5.1` | `PreviewStream` + 窗口化缩略 | `P5.01`/`P5.02` |
| `P5.2` | 视口框三态 + 八问 | `P5.03` |
| `P5.3` | 撤销/拖动/回到最新 | `P5.04` |
| `P5.4` | 渲染回读断言 | `P5.05` |
| `P6.1` | 执行 D/R 与改名 | `P6.03`/`P6.04` |
| `P6.2` | 修复 3 处静默跳过 | `P6.05` |
| `P6.3` | 6 处失效注释 | `P6.06` |
| `P6.4` | 门禁第二遍扫描 | `P6.07` |
| `P6.5` | `E-PERF-*` 回填 §23.3 | `P6.08` |
| `P6.6` | 命名一致性 | `P6.08` |

**本文相对 V2 §35 的净增**：`P0` 从 8 → 9（`P0.09`，`E-INJECT-1` 补齐）；`P1` 从 7 → 24（**V2 §35 的 P1 是"一行为一个工作包"，本文拆成可独立提交的粒度** —— 这是 TDD 的硬要求：RED 必须小到能一次写对）；`P2` 4 → 8；`P3` 5 → 9；`P4` 3 → 7；`P5` 4 → 6；`P6` 6 → 9。**净增的全部理由都是"一个提交只做一件事"**（`C2`），而不是增加范围。






