# SnapClip 滚动截图执行任务清单（docs/24）

> **⚠ 与 `docs/30` 的关系（2026-10-08 起，用户裁决）**：**`docs/30-scroll-capture-design-v2.md` 是本项目的滚动截图实施基准**；本文是 V1 时代的执行清单，保留作为**任务级细节来源**（阶段切分、门禁写法、执行记录模板、逐条"必须保持/验收/回退"）。**两者冲突时以 `docs/30` 为准**，具体对应关系见 `docs/30` §33（删除/重构/新增/移动清单）与 §35（P0–P6 实施计划）。本文已被明确作废的设计主张集中在：§7.2"搜索无结果即停止"、§4.2"GPU 单写者"、§8.3 tile 端口形状、§S3.6 的 overlay 必隐藏；逐条处置见 `docs/30` §36.4 的修正台账。
>
> 文档状态：开发期执行清单（可逐步勾选，可直接交给 agent 执行）
>
> 依据：`docs/19-scroll-capture-design.md`（修订稿 v4，**唯一设计基线**）。任务写法、门禁矩阵、执行记录模板沿用 `docs/23-snapclip-modular-refactor-tasklist.md` 的 §0 / §12 / §13；与实现或实测冲突时**以实测为准**，并把冲突写回 `docs/19`。
>
> 目标：按本文顺序执行完 S0–S5 后，得到——窗口级 WGC 活动帧源、真实滚轮驱动、可验证的位移匹配与 union 画布、tile 化导出、截图框右侧的增量滚动预览，以及 F6 原生贴图；并且**每一步都有前后测试证明"新功能可用、老功能不退化"**。
>
> 重要前提（本项目既定）：**尚未正式发布，允许破坏性重构**。不考虑向后兼容，不做兼容层、不做适配器堆积；但**不允许无意破坏当前功能**，每个任务都要跑门禁。
>
> 前置：docs/23 的 P1–P4 已落地、P6 已删除 Tauri/Vue。仓库现状符合。~~但 `docs/23 §2` 的 T6.x 仍标 `[ ]`，且 T1.6/T1.8 有"顺延（D3）"的剩余拆分 —— **S0.1 要对齐一次**~~ **（2026-10-08 已对齐：`docs/23` 的 T6.x 已按实测改为"部分 [x]"并把残留项（`rg tauri` 仍命中 37 文件 / 62 行、README 与 `.vscode` 仍写 Tauri）记录在案；T1.6 整组已完成，只有 T1.8 的 frame/mask/text 三个 pass 确实顺延；`docs/23 §0.2` 的 G2 包名与依赖方向门禁阴性对照也已修正。S0.1 因此不再需要"对齐两个清单"这一步，只需复跑并填基线。）**

---

## 0. 使用规则

### 0.1 勾选与完成规则

- [ ] 只有"门禁命令全部通过 + 数字与基线一致（或按预期变化并写明原因）"才允许勾选。
- [ ] 勾选框只表示"我准备做/我已做完动作"；**完成状态必须用 §11 的状态词表 + §12 的执行记录**，不能仅凭勾选框声称完成。
- [ ] **提交粒度与推送的关系**：每个任务 = 一个提交，该提交**同时包含**这个任务的 §12 执行记录条目（记录不单独留成未提交改动，否则会和 §0.5 的"开工前工作区必须干净"互相打架）；推送至少每阶段一次，推荐每任务一次。阶段收尾任务 = 一个提交 + 一个 tag。
- [ ] 不得通过删除测试、放宽容差、跳过 `#[ignore]` 探针、关闭功能来制造通过。
- [ ] 任务里的"必须保持"是回归清单：任何一条被破坏，即使测试是绿的，也视为失败。
- [ ] **每个任务都要写"前后"两组证据**：一组证明新功能对（"验收"），一组证明被它碰到的既有功能没退化（"必须保持"）。**只跑一遍全量测试不算回归证明**：凡改动共享代码路径的任务（S2 全部、S3.6、S4.2），都要给出针对**那个**功能的 before/after 数值或像素对照，并把两组数字写进提交消息。
- [ ] **开发期方针（AGENTS.md §1/§2/§4）**：本项目未正式发布，**允许并鼓励破坏性改动与重构**，以根因解法取代妥协；不考虑向后兼容，为此**不新增** Adapter / 兼容层 / 迁移逻辑 / 重复实现。当遇到"老实现还能跑"与"根因解法要改它"冲突时，选后者，并补上回归证据。若根因解法必须改到本文档之外的设计，先改设计文档（docs/19），不要在代码里留临时分支。
- [ ] **任务条目的模板**：编号任务与阶段收尾任务用同一套字段（前置 / 规模 / 背景事实 / 动作 / 必须保持 / 验收 / 注意 / 回退 / 风险）。阶段收尾任务的"规模""背景事实"可以写 `—`，但"必须保持""验收""风险"**必填**——阶段收尾最容易变成"跑了一遍门禁就说完成"。
- [ ] **S6 的条目在开工前必须展开**：§9 现在只有判据与范围，不是可执行任务。每个 S6.x 开工前要按 §11.2 模板写成完整条目（前置/动作/必须保持/验收/回退/风险），并重新确认 §0.4 的扩权。
- [ ] 滚动截图特有的两条纪律：
  - **不许用"看起来拼对了"当验收**：每个 `Accepted` 必须能回溯到 candidate margin、band consensus、valid area、residual、overlap 证据（docs/19 §11.5）。
  - **不许把不确定变成成功**：`Uncertain` 就是 `Uncertain`；降低阈值让它变成 `Accepted` 属于制造假绿。

### 0.2 门禁矩阵（当前处于 docs/23 的 **G2** 阶段）

**每个任务都跑（通用门禁）：**

| 门禁 | 命令 | 当前已知结果（2026-10-08 实测） |
| --- | --- | --- |
| 静态检查 | `cargo check --workspace --all-targets` | 0 error；**1 条既有 warning** `unused variable: content_label` @ `apps/snapclip/src/history/view.rs:769`（与本清单无关，不修；新出现的 warning 必须清零） |
| 单元测试 | `cargo test --workspace --lib` | **475 passed / 9 ignored / 0 failed**（app 56+3、capture 345+6、history 51、model 23） |
| 依赖方向 | `powershell -NoProfile -ExecutionPolicy Bypass -File tools/check-dependency-direction.ps1` | `dependency direction is clean`（capture 30 包 / history 47 包 / model 8 包） |

**受影响时才跑，阶段收尾必跑：**

| 门禁 | 命令 |
| --- | --- |
| 全量测试 | `cargo test --workspace --all-targets` |
| 壳 UI 测试 | `cargo test -p snapclip-app --features test-support --test ui` |
| 窗口检测探针（**必须串行，之间停 3 秒**） | `cargo test -p snapclip-capture --lib browser_element_probe -- --ignored --nocapture; Start-Sleep -Seconds 3`<br>`cargo test -p snapclip-capture --lib explorer_rule_probe -- --ignored --nocapture; Start-Sleep -Seconds 3`<br>`cargo test -p snapclip-capture --lib ring_contrast_probe -- --ignored --nocapture` |

> 探针位置：`browser_element_probe` / `explorer_rule_probe` 在 `crates/snapclip-capture/src/windows/uia_provider/tests/probes.rs`，`ring_contrast_probe` 在 `crates/snapclip-capture/src/ring_contrast.rs`。它们与滚动的重叠点是"我们改了 capture 的线程与设备使用"，所以 **S2 之后每次都要跑**。

**S2 专属门禁（前置重构的回归证明，见 §5）：**

| 门禁 | 内容 |
| --- | --- |
| 放大镜取色延迟 | 迁移到 GPU 线程后 P95 不劣于现状（记录 before/after 两组数字） |
| 导出像素一致 | 同一选区迁移前后的 **BGRA 像素**逐字节一致（这是像素，不是 PNG 文件字节——PNG 字节的判据见 S4.2） |

**docs/23 §0.2 的两处过期写法，本清单已纠正：**

- `cargo test -p snapclip` —— 包里没有名为 `snapclip` 的 package（P6 已删除 Tauri 宿主，它曾占用该名）。壳是 `snapclip-app`（目录仍是 `apps/snapclip`）。
- 全量门禁在本机可能因"`snapclip-app.exe` 正在运行"而失败，见 §0.3。

### 0.3 环境纪律

- **跑 `--all-targets` 或壳 UI 测试前，必须先关掉正在运行的 `snapclip-app.exe`。** 2026-10-08 实测：一个自 10-07 23:50 起运行的实例占着 `target\debug\snapclip-app.exe`，两条命令都直接 `error: failed to remove file`。这是环境问题，**不是回归**；关掉进程后重跑，红了才算回归。
- 探针需要真实窗口（浏览器由探针自己拉起；Explorer 需要本机有一个可见的资源管理器窗口）。
- 红了**先排除环境**（窗口不存在、被遮挡、机器高负载），复跑 2 次；复跑后仍稳定红才当回归。假红与真回归都要写进提交消息。
- 滚动的人工验收会**真的驱动鼠标**（`SendInput`）。跑之前存好手头的东西，别在有未保存编辑的窗口上测。

### 0.4 需要用户明确批准的破坏性改动

`docs/23 §0.5` 明确把"**改变截图 overlay 的线程模型**"列在**批准范围之外**。而 docs/19 v4 的 §4.2 裁决要求把 immediate context 的使用者整体迁到 GPU 线程 —— 这**正是**线程模型变更。因此：

- **S2.0（即 S2.1 之前）必须重新取得用户批准**（这是本次唯一一处需要扩权的改动）。
- 未获批准前，S0、S1、S4 的全部任务与 S3 的纯 CPU/输入部分仍可推进；S2 只能停在"设计 + 夹具"。
- 另外明确批准范围（沿用 docs/19 v4）：新增 `AppEvent::Scroll` 变体、新增滚动词汇表到 `snapclip-model`、新增 `ScrollSink`/`ScrollArtifactWriter` 端口、新增 `snapclip-capture` 内的滚动模块。
- 下面这一组**不是"兼容性约束"，而是本次的范围边界**（避免顺手改动与滚动无关的契约）：`CaptureEvent` 的结构、`CaptureEventSink`/`CaptureRuntime`/`OverlayPlatform` 的既有语义、EventBus 的丢弃规则。按 §0.1 的开发期方针，**若滚动截图确实需要动它们，动就是允许的**——前提是先证明它是根因解法、同步更新 docs/19 与本清单、并补齐 before/after 回归证据。**不允许**的做法是加一层封装把老接口原样留在旁边。

> **2026-10-08 事实更正（原第 4 条"OpenCV 作为 `snapclip-capture` 的默认依赖"）**：**`snapclip-capture` 从未依赖 OpenCV。** `crates/snapclip-capture/Cargo.toml` 全文没有 `opencv`；全仓唯一命中在 `.comparison-old/Cargo.toml:21`（旧对照工程）。原文把它当既有契约写进范围边界，会让后来者以为"capture 里已经有 OpenCV，用它是免费的"，而事实相反——`docs/19:691` 正是以"要引入 OpenCV"为由把 ORB 降级为最后手段。**该条已删除**：它不是需要保护的范围边界。相关裁决见 `docs/30` §4.7、§15.5 与 §36.1 D-1（ORB 自写、不引入任何新 crate 依赖）。

### 0.5 提交、tag 与回退

- 规则完全沿用 `docs/23 §0.4` 与 `§0.6`（提交粒度、回退点必须可达、`revert` 优先、`reset --hard` 需批准并 `--force-with-lease`、误删用 `reflog`）。
- 本清单的 tag：`scroll-s0`、`scroll-s1`、`scroll-s2`、`scroll-s3`、`scroll-s4`、`scroll-s5`；高风险阶段（S2、S3）用分支 `scroll/s2-framesource`、`scroll/s3-driver`。
- **开工前工作区必须干净**（`git status --porcelain` 为空）。2026-10-08 现状**不干净**，完整清单是：`docs/19-scroll-capture-design.md` 已修改（用户侧的基线更新，非本清单产生）；**本清单自身 `docs/24-scroll-capture-tasklist.md`** 以及 `docs/25`–`docs/30`（本轮评审与 V2 设计）全部未跟踪；另有 `prototypes/demo.html` 与 `scripts/` 未跟踪。**原文漏列了 `docs/24` 自身**——"要求工作区干净"的文档自己就是未跟踪文件，按 docs/23 §0.6，**先报告、由用户确认归属**，不要自动 `stash`/`clean`/提交。`docs/Temp/` 被 `.gitignore` 忽略，不进该清单。

> **归属已确认（2026-10-08，裁决见 `docs/30` §36.1 D-3）**：`docs/24`–`docs/30` 与 `docs/19` 的既有修改一并提交推送，作为滚动截图实施的基准；`prototypes/demo.html` 与 `scripts/` 与本清单无关，**保持未跟踪**。

### 0.6 并行执行候选

仍按 docs/23 §0.7 的规则（独立 worktree、不同时改同一个模块入口、各自跑完整门禁）。本清单内互不重叠的候选：

- **S0.3 / S0.4 / S0.5**（合成帧、可控窗口、故障注入三套夹具）可并行——它们各自独立文件。
- **S1.2 / S1.3** 在 S1.1 冻结 `MatchView`/`Alignment` 之后可并行。
- 其余默认串行；S2 与 S3 不并行（都要动 capture 的线程与设备）。

---

## 1. 基线（动手前记录；每个阶段对照）

> 规则同 docs/23 §1.1：**数字必须由命令在当前 HEAD 现场产生**，不手写、不从旧文档抄。
> 下表是 2026-10-08 的实测值，**S0.1 的职责就是复跑并把它填成正式基线**（含下面标"待测"的两行）。

### 1.1 正确性基线

| 门禁 | 生成命令 | 2026-10-08 实测 |
| --- | --- | --- |
| 静态检查 | `cargo check --workspace --all-targets` | 0 error / 1 warning（`content_label` @ `view.rs:769`，既有） |
| 单元测试（分包） | `cargo test --workspace --lib` | app **56/3**、capture **345/6**、history **51/0**、model **23/0** → 合计 **475 passed / 9 ignored / 0 failed** |
| 依赖方向 | `tools/check-dependency-direction.ps1` | clean（capture 30 / history 47 / model 8） |
| 全量测试 | `cargo test --workspace --all-targets` | **待测**（被运行中的 `snapclip-app.exe` 阻塞，见 §0.3） |
| 壳 UI 测试 | `cargo test -p snapclip-app --features test-support --test ui` | **待测**（同上） |
| 窗口检测探针 ×3 | 见 §0.2 | **待测**（需要真实浏览器 / 可见 Explorer 窗口） |

### 1.2 与滚动相关的资源基线（S5 的性能对比要用）

S5 之前必须先有一份"滚动功能之前"的底数，否则 S5 的"没有不可接受退化"无从判定。至少记录：

| 指标 | 怎么测 | 现在 |
| --- | --- | --- |
| 进程 / 线程数 | 见 docs/23 §1.2 的取数命令 | **待测**（S2 之后线程数会 +1：scroll driver） |
| 常驻内存 WS / Private | 同上 | **待测** |
| 空闲 CPU | 同上 | **待测** |
| 一次普通截图 F5 → 导出的 `present_us` / `over16ms` | 真机日志（docs/23 §1.1 的口径） | **待测** |
| 放大镜取色延迟 P95 | S2.3 的门禁自建测量（用 S0.5 的可注入时间源） | **待测**（S2.3 的 before 值） |

> 纪律：**"未测"就写"未测"**，不要留空、不要拿预期收益当数字（docs/23 §1.2 已经踩过一次"把噪声当收益"）。

### 1.3 规模基线

| 项目 | 现在 | 说明 |
| --- | --- | --- |
| `snapclip-capture` 正常依赖包数 | 30 | S2 新增工作不应改变它（除 `windows`/`windows-sys` 既有依赖外不引入新包） |
| 滚动相关代码 | 0 行 | 全新模块 `crates/snapclip-capture/src/windows/scroll/` |
| 既有 GPU 回读调用点 | 2 处（另有 1 条带标注的 D2D 导出链路） | `FrozenFrame::read_region`（**定义 `windows/providers.rs:135`**，调用点 `providers.rs:110` 的全额回读与 `providers.rs:171` 的区域回读）、`AsyncSampleBuffer`（构造于 `windows/renderer.rs:134-135`）、`render_export`（`windows/win/d2d.rs:667`）（S2.2 / S2.3 迁移对象） |
| 既有跨线程信箱协议 | **4 套** | `capture_worker.rs:35`（`WM_APP+18`）、`export_worker.rs:31`（`+19`）、`detection_worker.rs:35`（`+43`）、`refinement_worker.rs:295`（`+44`）——S2.1 的模板，**不是从零发明** |

取数命令：

```powershell
cargo metadata --no-deps --format-version 1 | ConvertFrom-Json |
  Select-Object -ExpandProperty packages | Select-Object name, manifest_path
```

---

## 2. 任务总览（进度表）

| 编号 | 任务 | 依赖 | 规模 | 风险 | 状态 |
| --- | --- | --- | --- | --- | --- |
| S0.1 | 复跑并记录基线（含对齐 docs/23 状态） | — | — | 低 | [ ] |
| S0.2 | 冻结滚动接缝类型进 `snapclip-model`（`AppEvent::Scroll`） | S0.1 | ~3 文件 | 中 | [ ] |
| S0.3 | 合成帧夹具（已知位移 / 固定边缘 / 重复纹理） | S0.1 | ~2 文件 | 低 | [ ] |
| S0.4 | 可控滚动窗口夹具 | S0.1 | ~2 文件 | 中 | [ ] |
| S0.5 | 确定性时间与故障注入夹具 | S0.3 | ~2 文件 | 中 | [ ] |
| S0.6 | 阶段验收 + tag `scroll-s0` | S0.2–S0.5 | — | 低 | [ ] |
| S1.1 | 轴抽象 + `MatchView`/`Alignment`/`MatchConstraints` | S0.6 | ~3 文件 | 中 | [ ] |
| S1.2 | profile descriptor + 1D SAD 粗搜索 | S1.1 | ~2 文件 | 中 | [ ] |
| S1.3 | 多带 consensus + margin/valid area/residual 判据 | S1.2 | ~2 文件 | 中 | [ ] |
| S1.4 | 动态选带与短期动态 mask | S1.3 | ~2 文件 | 中 | [ ] |
| S1.5 | sticky leading/trailing 置信区间 | S1.3 | ~2 文件 | 中 | [ ] |
| S1.6 | union 画布 + coverage + 拒绝/零位移语义 | S1.1 | ~3 文件 | 高 | [ ] |
| S1.7 | 全局重锚定（keyframe 内存契约 + 邻接判定） | S1.6 | ~2 文件 | 高 | [ ] |
| S1.8 | 上限与资源预算（纯内存画布版） | S1.6 | ~1 文件 | 低 | [ ] |
| S1.9 | End Confirmation 状态机 | S1.7 | ~1 文件 | 中 | [ ] |
| S1.10 | 阶段验收 + tag `scroll-s1` | S1.1–S1.9 | — | 低 | [ ] |
| S2.0 | **取得用户批准**（线程模型变更，§0.4） | S1.10 | — | — | [ ] |
| S2.1 | GPU 线程请求/响应协议（信箱 / ID / 取消 / 唤醒 / 超时） | S2.0 | ~2 文件 | 中 | [ ] |
| S2.2 | 两条导出链路迁移（无标注直读 + 带标注 `render_export`） | S2.1 | ~4 文件 | 高 | [ ] |
| S2.3 | 放大镜采样迁移（latest-request + `cursor_revision`） | S2.1 | ~3 文件 | 高 | [ ] |
| S2.4 | context 单写者断言 + 测试 | S2.3 | ~2 文件 | 中 | [ ] |
| S2.5 | 泛化区域异步回读（尺寸/槽数可配） | S2.4 | ~2 文件 | 中 | [ ] |
| S2.6 | `ActiveFrameSource`（`CreateForWindow` + 降级条件 + 帧所有权 + 取消唤醒） | S2.5 | ~3 文件 | 高 | [ ] |
| S2.7 | 容量 1 信箱 + 丢帧跨多位移语义 | S2.6 | ~2 文件 | 中 | [ ] |
| S2.8 | 尺寸变化 / 设备移除 / 窗口关闭 / provider 诊断 | S2.7 | ~3 文件 | 中 | [ ] |
| S2.9 | 阶段验收 + tag `scroll-s2` | S2.1–S2.8 | — | 低 | [ ] |
| S3.1 | `DriverCommand`/`DriverEvent` + 关联信息 | S2.9 | ~2 文件 | 低 | [ ] |
| S3.2 | WheelDriver 竖向（SendInput + 焦点/光标恢复） | S3.1 | ~3 文件 | 中 | [ ] |
| S3.3 | frame-driven settled | S3.2 | ~2 文件 | 中 | [ ] |
| S3.4 | 闭环步长（overlap_ratio + conservative mode） | S3.3 | ~2 文件 | 中 | [ ] |
| S3.5 | 横向能力探测 + HWHEEL | S3.4 | ~2 文件 | 中 | [ ] |
| S3.6 | overlay 输入隔离 + controller HWND（热键/排除/降级） | S3.2 | ~3 文件 | 高 | [ ] |
| S3.7 | 低频状态事件（`AppEvent::Scroll` 上报） | S3.2 | ~2 文件 | 低 | [ ] |
| S3.8 | v1 只向前 + `ManualPanoramaDriver` 接口 | S3.4 | ~2 文件 | 低 | [ ] |
| S3.9 | 右侧滚动预览（增量 patch / 视口框 / stale 丢弃） | S3.3, S3.7 | ~4 文件 | 高 | [ ] |
| S3.10 | F6 贴图热键与原生贴图窗口 | S3.7 | ~4 文件 | 高 | [ ] |
| S3.11 | 阶段验收 + tag `scroll-s3` | S3.1–S3.10 | — | 中 | [ ] |
| S4.1 | `ScrollTile`/`ScrollExportMeta`/`ScrollFormat` | S1.8 | ~2 文件 | 低 | [ ] |
| S4.2 | 导出端口 + **流式 PNG**（重构编码器为行带输入，不物化整图） | S4.1 | **按调用图核定** | 中 | [ ] |
| S4.3 | tile 化画布 + 有界 LRU + 临时目录 | S4.2 | ~3 文件 | 中 | [ ] |
| S4.4 | 部分结果与"没有画布"的三种出口（`abort` / Partial / 清理） | S4.3 | ~2 文件 | 中 | [ ] |
| S4.5 | 导出预算 + 原子替换 | S4.4 | ~2 文件 | 中 | [ ] |
| S4.6 | 端到端：结果进历史 + 剪贴板规则 | S4.5 | ~2 文件 | 中 | [ ] |
| S4.7 | F6 artifact 读取与普通/Partial 贴图回归 | S4.5, S3.10 | ~3 文件 | 中 | [ ] |
| S4.8 | 阶段验收 + tag `scroll-s4` | S4.1–S4.7 | — | 低 | [ ] |
| S5.1 | v1 验收矩阵（向前） | S4.8, S3.11 | — | 中 | [ ] |
| S5.2 | 质量门禁（blank/gap/重复/false acceptance/漂移） | S5.1 | — | 中 | [ ] |
| S5.3 | 性能采样并对 §1.2 | S5.1 | — | 中 | [ ] |
| S5.4 | 冻结默认参数（只依据数据） | S5.3 | — | 低 | [ ] |
| S5.5 | 人工验收八条 | S5.4 | — | 低 | [ ] |
| S5.6 | 阶段验收 + tag `scroll-s5` | S5.1–S5.5 | — | 低 | [ ] |
| S6.x | v2 扩展（PageKey / UIA / BrowserAdapter / 双向） | **触发式** | — | 高 | [ ] |

---

## 3. S0：基线与夹具

> 这一阶段不产出任何用户可见功能，但它决定后面每一步能不能被验证。**夹具没建好就写匹配算法，等于没有验收。**

### 夹具支持层的公共契约（S0.3–S0.5 共同遵守）

夹具会被单元测试、`tests/` 集成测试和 `#[ignore]` 探针三种目标复用，位置与生命周期必须先定死，否则后面每个任务都会各写一套：

- **位置与可见性**
  - 纯 CPU 的（合成帧生成器）：`crates/snapclip-capture/src/fixtures.rs`，用 `#[cfg(any(test, feature = "test-support"))]`。
  - 需要真实窗口/线程的（可控滚动窗口、故障注入）：`crates/snapclip-capture/src/fixtures/`，同样挂在 `test-support` feature 下——这样 `--lib` 探针和 `tests/` 集成测试都能用同一份实现。
  - 门禁不受影响：这些模块只在 test / feature 下编译，**不得进入正常依赖图**；S0.6 要复跑依赖方向门禁确认这一点。
- **可控窗口的生命周期**（三个任务都不能省）
  - `start(config) -> Result<Self>`：创建窗口并**等到就绪**（ready handshake），不能 `CreateWindow` 一返回就当可用。
  - `offset() -> (i32, i32)`：读当前滚动位置，供断言；`set_offset()` 用于构造初态。
  - `shutdown()`：投递关闭消息、join 窗口线程、幂等；**`Drop` 必须调用它**，测试 panic 时也不能泄漏窗口与线程。
  - 窗口线程由夹具独占；测试只通过上述 API 交互，不直接碰 HWND 与消息循环。
  - 用它的测试一律 `#[ignore]`，按 §0.2 串行纪律跑。
- **故障注入的可用性**：`Injector::{drop_frames, resize, dpi_change, device_lost, target_close, disk_fail}` 每次注入都要留下一条可断言的"已触发"记录——避免"注入了但没生效"这种假绿。

### S0.1 复跑并记录基线

- 前置：—
- 规模：不改生产代码
- 背景事实（2026-10-08 实测）：`--lib` 合计 475/9/0；`check` 有 1 条既有 warning；依赖门禁 clean；**`--all-targets` 与壳 UI 测试被一个运行中的 `snapclip-app.exe` 阻塞**（§0.3）。
- 动作：
  1. 先向用户确认可以关闭正在运行的 `snapclip-app.exe`（它是用户的实例，不要自行杀进程）。
  2. 关掉后跑 §0.2 的**全部**门禁，把 §1.1 里标"待测"的两行填实。
  3. 记录 §1.2 的滚动相关资源基线（`before` 栏），S5 要用。
  4. 对齐 `docs/23 §2` 的状态表与仓库现状：T6.x 是否已在 commit `d7b5708` 之后实际完成？T1.6/T1.8 的"顺延（D3）"剩余拆分是否影响 capture 的模块布局？**两条清单不能互相矛盾。**
  5. 发现不一致时，改错的那一份（docs/23 的状态 or docs/19 的事实描述），不要两份都留着。
- 必须保持：不改任何生产代码；既有测试数字不变。
- 验收：§1.1 / §1.2 里没有"待测"残留；docs/23 的状态与仓库一致，或有明确记录说明谁对谁错。
- 注意：探针必须串行且间隔 3 秒；Explorer 探针要求本机有一个**可见**的资源管理器窗口。
- 回退：无需回退（只改文档）。
- 风险：低。

### S0.2 冻结滚动接缝类型（`snapclip-model`）

- 前置：S0.1
- 规模：~3 文件（`crates/snapclip-model/src/events.rs`、可能的 `scroll.rs`、`lib.rs` 导出）
- 背景事实：`CaptureEvent` 是**结构体**、`AppEvent` 才是枚举；`AppEvent` 派生 `PartialEq, Eq`，且 `events.rs` 有一条 `size_of::<AppEvent>() <= 256` 的测试。
- 动作：
  1. 在 `snapclip-model` 定义 `ScrollAxis` / `ScrollState` / `ScrollOutcome` / `ScrollStopReason` / `ScrollProgress`（字段与取值按 docs/19 §4.6 与 §10.3）。
  2. `AppEvent` 增加第四个分支 `Scroll(ScrollProgress)`；**补 `generation()` 与 `with_generation()` 的 `Scroll` 分支**——漏了它，新分支的 generation 恒为 0，"丢弃更旧事件"对它失效。
  3. 置信度用 `u16` 定点（`last_confidence_bp`，0..=10000）。不要为了 `f32` 放弃 `Eq`。
  4. 壳（`apps/snapclip`）至少要能编译：给 `AppEvent::Scroll` 一个处理分支（先按"记录/忽略"处理，UI 呈现留给 S3.7）。
  5. 新增测试：`Scroll` 事件参与重盖与丢弃；`size_of::<AppEvent>() <= 256` 仍成立。
- 必须保持：`CaptureEvent` 的形状与语义不变；既有三类事件行为不变；`Eq` 派生不变。
- 验收：滚动词汇表在 `snapclip-model`（capture 依赖 model 而不是反向）；`cargo test --workspace --lib` 绿；新测试在测试列表里可见。
- 注意：`ScrollStopReason` 会被 `snapclip-capture` 的 matcher、driver、canvas 多处引用，字段一旦冻结就不要在后续任务里反复改（要改就在这个任务里改完）。
- 回退：`git revert`。
- 风险：中（跨 crate 公共类型，改动面会扩散）。

### S0.3 合成帧夹具

- 前置：S0.1
- 规模：~2 文件（`crates/snapclip-capture` 内的测试支持模块）
- 动作：写纯函数帧生成器，覆盖 docs/19 §11.1 要求的图案：可逆行/列纹理、固定页眉页脚、重复纹理区、全空白区、局部动态噪声、**已知整数位移（含 ±1 px）**。
- 必须保持：不进生产编译路径（`#[cfg(test)]` 或 test-support feature）。
- 验收：生成器自身有测试钉住——"生成 → 反解 offset"必须等于输入；同一 seed 两次生成逐字节一致。
- 回退：`git revert`。
- 风险：低。

### S0.4 可控滚动窗口夹具

- 前置：S0.1
- 规模：~2 文件
- 动作：一个自绘 Win32 测试窗口，可编程设置滚动位置、步长、动画曲线与懒加载延迟，并对外暴露"当前 offset"供断言。它是 S3/S5 的唯一真实输入闭环。**API 与生命周期按本节开头的"夹具支持层的公共契约"实现**（`start` 带就绪握持、`offset`/`set_offset`、`shutdown` 幂等且由 `Drop` 调用）。
- 必须保持：仅测试夹具，挂在 `test-support` feature 下，不进正常依赖图；`#[ignore]` 探针式运行（要真实窗口）。
- 验收：用手动 `SendInput` 滚轮能驱动它滚动，且读到的 offset 变化与滚轮量一致；在无窗口环境里它被 `#[ignore]` 排除，不会让 CI 红；连续 `start`/`shutdown` 20 次后窗口与线程数回落（这一条直接防"夹具自己泄漏"）。
- 回退：`git revert`。
- 风险：中（Win32 窗口 + 输入注入的测试夹具本身可能不稳）。

### S0.5 确定性时间与故障注入

- 前置：S0.3
- 规模：~2 文件
- 动作：
  1. 可注入的时间源（QPC / `SystemRelativeTime` 替身），让 settled 判定与丢帧判定能确定复现。
  2. 故障注入按公共契约的 `Injector` 提供：`drop_frames` / `resize` / `dpi_change` / `device_lost` / `target_close` / `disk_fail`，每次注入留一条"已触发"记录。
- 必须保持：注入只影响测试路径；生产代码仍用真实时钟。
- 验收：每条故障路径至少一条测试到达预期终态（`Ended(WindowChanged|DpiChanged|DeviceLost)` / `DriftBeyondBudget` / `ResourceLimit`），而不是卡在某处不返回；断言里要读那条"已触发"记录，证明注入真的生效。
- 回退：`git revert`。
- 风险：中。

### S0.6 阶段验收 + tag `scroll-s0`

- 前置：S0.2–S0.5
- 规模：—
- 背景事实：—
- 动作：跑完整 §0.2 门禁 + 真机普通截图一次（确认没被夹具污染），打 tag `scroll-s0`，并做一次回退演练（`git switch --detach scroll-s0` → 跑门禁 → 切回）。
- 必须保持：夹具只在 `test-support` / `#[cfg(test)]` 下编译——**依赖方向门禁的数字必须与 S0.1 一致**（capture 仍 30 包）；普通截图行为不变。
- 验收：tag 处可编译可测；§1.1 基线齐全；夹具的公共契约（位置/生命周期/注入记录）已在代码里落地而不是只写在文档里。
- 风险：低。

---

## 4. S1：纯拼接核心（不接 WGC、不接 SendInput）

> 这一阶段结束时的标准是：**给它一串合成帧，它能给出正确的位移、正确的画布、正确的拒绝**。全部是纯函数 + 纯 CPU，一条真实输入都不发。这也是唯一能"逐像素断言"的阶段，不要跳过。

### S1.1 轴抽象与核心值类型

- 前置：S0.6
- 规模：~3 文件（新模块 `crates/snapclip-capture/src/windows/scroll/`）
- 动作：定义 `ShiftMatcher` trait、`MatchView`、`MatchConstraints`、`Alignment`、`AlignmentStatus`，以及垂直/水平共用的轴访问器（行 ↔ 列只差一个投影，不要写两套算法）。
- 参考实现约束：吸收 Crisp 的“先规划 accepted shift 再分配画布”、band early-exit、sticky 区域缩小搜索范围和最大边长保护；实现不得把固定 40/80 px、固定像素差阈值或 GDI 位图生命周期照搬进生产路径。
- 必须保持：`Alignment.confidence`/`residual` 是 capture 内部类型，可以用 `f32`（不跨事件边界）；半开区间 `[min, max)` 语义在垂直与水平上一致。
- 验收：正负 delta、半开区间、轴访问器有单元测试；同一算法在两个轴上的行为对称（同一合成图旋转 90° 后结果一致）。
- 验收：另加 Crisp `TestStitch` 等价夹具：known shift、零位移、无关帧、尺寸不符、重复纹理、sticky header/footer、高 footer、水平/垂直；无关/歧义输入必须拒绝而不是给出“最佳猜测”。
- 回退：`git revert`。
- 风险：中（这是后面所有算法的公共形状）。

### S1.2 profile descriptor 与一维粗搜索

- 前置：S1.1
- 规模：~2 文件
- 动作：每行/列生成 compact descriptor（mean luma、variance、edge energy、少量 bins，**不得只用平均亮度**）；在 `expected_delta ± search_margin` 内做一维 profile SAD。
- 必须保持：搜索窗只在首次、步长大变、丢帧或异常时扩大——不要每步全窗扫描。
- 验收：合成纹理上"已知 offset → 求解 == 输入"；纯色/零纹理输入返回 `Uncertain` 而不是随便给一个数。
- 回退：`git revert`。
- 风险：中。

### S1.3 多带 consensus 与接受判据

- 前置：S1.2
- 规模：~2 文件
- 动作：多带投票；候选必须同时满足 band agreement、best/second-best margin、valid area、residual、temporal consistency。`delta ± 4..8` 内做一次全分辨率窄条精修，最终只接受整数像素。
- 必须保持：任何一条不满足就返回 `Rejected` / `Uncertain`，**不得**把最佳候选当成成功（docs/19 §7.2 的拒绝清单逐条要有测试）。
- 验收：重复纹理、动态噪声、尺寸不一致三类输入都必须被拒；正常纹理必须被接受且能给出可回溯的 margin / band / valid area / residual 证据（这些证据要能在测试里断言到）。
- 回退：`git revert`。
- 风险：中。

### S1.4 动态选带与短期动态 mask

- 前置：S1.3
- 规模：~2 文件
- 动作：按 `texture + edge + temporal_stability - dynamic_penalty - sticky_penalty - scrollbar_penalty` 选 2–3 个带；每会话只保留最近 2–4 个**微型摘要**（不是 MatchView、不是 BGRA）。
- 必须保持：mask 过大导致 `valid_pixels < minimum` 时必须 `Uncertain`，**不允许通过降低阈值强行接受**；不得累积历史帧。
- 验收：动态区域（模拟视频/光标/loading）被 mask 掉后匹配仍稳定；一个专门的反向测试钉住"阈值没被偷偷放宽"。
- 回退：`git revert`。
- 风险：中。

### S1.5 sticky leading/trailing

- 前置：S1.3
- 规模：~2 文件
- 动作：用相似度 / edge similarity / motion ratio / 连续 run length 估计 `StickyRegion { start_px, end_px, confidence }`；高置信度只从首帧写 leading、只从最终 `EndConfirmed` 帧写 trailing；中低置信度只加入 mask，不裁剪真实内容。
- 必须保持：横向的 left/right 检测不稳定时保守保留，不强行删除。
- 验收：固定页眉/页脚只出现一次；置信度不足时**像素不被裁掉**（这是关键回归点）。
- 回退：`git revert`。
- 风险：中。

### S1.6 union 画布与覆盖语义

- 前置：S1.1
- 规模：~3 文件
- 动作：signed 坐标的 union 画布 + coverage bitmap；只为 `new_union - old_union` 新增；overlap 允许新帧覆盖旧帧但逻辑坐标只出现一次；`NoMovement` 不扩展、`Rejected`/`Uncertain` 不改变画布。
- 必须保持：`blank_pixels = 0`、无 gap、无重复逻辑范围；检测到 gap 时停止并报 Partial，**不用白色/透明填充**。
- 验收：向下、向上、往返三种序列下画布都不膨胀；overlap 覆盖有质量保护（新 overlap 质量显著下降时保留旧 tile 并标 uncertain）。
- 回退：`git revert`。
- 风险：高（这是正确性的核心不变量）。

### S1.7 全局重锚定

- 前置：S1.6
- 规模：~2 文件
- 动作：每 N 步（默认 20）或累计 `|delta|` 超阈值做一次全局对齐；keyframe = 低分辨率 profile + 窄带摘要（默认 1/8、≤64 KB、每会话只留 1 个）。
- 必须保持：修正**只改当前帧的绝对位置估计**，不改写已提交像素；应用前必须验证"修正后与原 union 相邻或相交、新增条带 ≥1 px、overlap 不低于硬下限"；否则不应用，直接 `DriftBeyondBudget`。
- 验收：注入 ±1 px 误差的合成序列，100 步后总误差 ≤2 px，且不产生 gap / 重复逻辑坐标；内存占用有上限断言（keyframe ≤64 KB、只留 1 个）。
- 回退：`git revert`。
- 风险：高（唯一会"移动坐标系"的机制）。

### S1.8 上限与资源预算

- 前置：S1.6
- 规模：~1 文件
- 动作：轴向长度、总像素、tile 数、临时目录字节、导出时长五条上限；本阶段先做纯内存版，超限返回 `ResourceLimit`。
- 必须保持：上限是**保护阈值**，不是目标——不要为了让大图通过而抬高它。
- 验收：每条上限都有"刚好不触发 / 刚好触发"的边界测试。
- 回退：`git revert`。
- 风险：低。

### S1.9 End Confirmation 状态机

- 前置：S1.7
- 规模：~1 文件
- 动作：实现 `Moving → NoMovement → Probe → WaitLoad → ProbeAgain → EndConfirmed / EndUncertain`；`NoMovement` 只是"这一步没看到位移"，不是 EOF。
- 必须保持：`EndUncertain` 保留 Partial；加载中不得提前判 EOF；UIA extent（未来）只能作为额外证据。
- 验收：加载延迟夹具下不提前结束；真 EOF 下能确认；两次 NoMovement 的判据可复现。
- 回退：`git revert`。
- 风险：中。

### S1.10 阶段验收 + tag `scroll-s1`

- 前置：S1.1–S1.9
- 动作：跑完整门禁 + 回退演练 + tag `scroll-s1`。做一张"合成序列 → 期望输出"的表，作为后面真实场景的对照。
- 验收：docs/19 §11.1 的单元测试清单逐条有对应测试，且每一条都能指出测试名。
- 风险：低。

---

## 5. S2：帧源与 GPU 线程

> 这一阶段动的是**既有截图代码**（不是新增模块），也是整份清单里风险最高的地方。S2.1–S2.4 是 docs/19 §4.2 的**前置重构**：把 immediate context 变成单写者。

### S2.0 取得用户批准

- 前置：S1.10
- 动作：向用户确认 §0.4 里的扩权项（"改变截图 overlay 的线程模型"）。未获批准就停在这里，**不要**先改代码再说。
- 验收：批准记录写进本任务条目与提交消息。
- 风险：—（这是流程任务）。

### S2.1 GPU 线程请求/响应协议

- 前置：S2.0
- 规模：~2 文件（新模块，例如 `windows/gpu_service/`）
- 背景事实：~~今天没有任何跨线程协议可用~~ **这句话是错的（2026-10-08 更正）**——仓库里已经有**四套结构相同的跨线程"容量 1 信箱 + `Condvar` + `PostThreadMessageW`"协议**可作模板：`crates/snapclip-capture/src/windows/capture_worker.rs:35`（`WM_APP+18`）、`windows/export_worker.rs:31`（`WM_APP+19`）、`windows/detection_worker.rs:35`（`WM_APP+43`）、`windows/refinement_worker.rs:295`（`WM_APP+44`）。**真正缺的不是"协议"，而是"给 GPU context 用的那一条"**：今天所有使用方都在 overlay 线程**同步**调用 `GraphicsDevice::context()`（`windows/win/d3d11.rs:103` 的注释即写 "same thread usage constraint as D2D"），所以"迁移"要解决的是**谁拥有 context**，不是"从零发明一套信箱"。**已确证的回读调用点归属（修正原文的错误归因）**：`read_region` 是 `FrozenFrame` 的方法，**定义在 `crates/snapclip-capture/src/windows/providers.rs:135`**；`crates/snapclip-capture/src/windows/overlay/session.rs:354-355` **只是一段注释**（说明"区域回读是唯一触碰单线程 immediate context 的一步，所以留在这里同步做"），不是定义处；GPU 侧的两个原语是 `read_back_bgra`（`windows/win/d3d11.rs:254`）与 `read_back_region_bgra`（`windows/win/d3d11.rs:315`）；放大镜在 `windows/renderer.rs:135` 构造时就持有 `AsyncSampleBuffer`。
- 动作：定义 S2.2（导出回读）、S2.3（放大镜采样）、S2.5（MatchView 回读）三处共用的协议：
  1. 请求/结果类型（建议形状，允许改名但不允许省字段）。三类请求都要有——**三类使用方共用一套协议，缺一类就写不下去**：
     ```
     enum GpuRequest {
         ReadRegion    { session_id, generation, request_id, frame: Arc<GpuFrame>, rect: PhysicalRect },
         // 滚动匹配用：同一张帧的低分辨率视图，downsample 由调用方给（默认 4）
         ReadMatchView { session_id, generation, request_id, frame: Arc<GpuFrame>,
                         rect: PhysicalRect, downsample: u8 },
         SampleCursor  { session_id, generation, request_id, cursor_revision: u64,
                         frame: Arc<GpuFrame>, point: Point },
     }
     enum GpuResult {
         RegionRead    { session_id, generation, request_id, pixels: SelectionPixels },
         MatchView     { session_id, generation, request_id, view: MatchView },
         CursorSample  { session_id, generation, request_id, cursor_revision: u64, tile: CursorTile },
         Failed        { session_id, generation, request_id, reason: GpuRequestFailure },
     }
     enum GpuRequestFailure { FrameGone, MapStalled, ReadbackTooSlow, DeviceLost, Cancelled }
     ```
  2. **两条有界车道，不是一条队列**。两类请求的语义相反，塞进同一个容量 1 的槽里会互相覆盖：
     - **必达车道**（`ReadRegion` / `ReadMatchView`）：容量 ≥1 的**有界**队列。每条请求要么被处理、要么回 `Failed`，**不得静默丢弃**（丢一条，导出就会永远等下去）。
     - **最新覆盖车道**（`SampleCursor`）：容量 1，新请求直接替换 pending 的旧请求（鼠标快速移动时不排队、不积压）。
     - **响应槽不能共用一条容量 1 的信箱**：`ReadRegion` 与 `ReadMatchView` 可能先后到达，后者会把前者的结果覆盖掉，而必达请求丢了就永远等。规则：
       - **必达请求有最大并发数**（默认 1：导出与 MatchView 本来就不需要并行回读；若 profiling 需要 2，再以数据调整），且**每个在途请求拥有独立的响应槽/完成令牌**（请求发出时登记，结果到达时按 `request_id` 落到对应槽并唤醒等待者）；
       - 超过最大并发数的必达请求**在提交时就被拒绝**（回 `Failed`），而不是进队后丢结果；
       - `SampleCursor` 的结果仍走 latest-only 槽（丢掉旧的采样结果是对的）。
     - 测试必须覆盖："两个必达结果先后到达时都能被各自的等待者看到"，而不只是"容量有界"。
  3. **每条结果都携带 `session_id + generation + request_id`**（三者齐全）。缺一个，消费方就无法执行 docs/19 §3.1 的四重校验，"丢弃 stale 结果"也就无从谈起。
  4. **取消与失效**：会话携带 `AtomicBool` cancel；`session_id` / `generation` / `request_id` 任一过期就丢弃；取消时**不 join** GPU 线程，pending 请求作废并回 `Failed(Cancelled)`，让等待方解除等待。
  5. **唤醒方式**：沿用项目既有做法——GPU 线程用 `PostThreadMessageW` 发 `WM_APP+n`（对照 `capture_worker.rs` 的 `FRAME_READY_MESSAGE`、`export_worker.rs` 的 `EXPORT_READY_MESSAGE`），overlay 收到后取结果。**不允许** overlay 同步阻塞等待。
  6. **超时**：调用方只声明愿意等多久，到期由各自语义降级——导出 `Failed(ReadbackTooSlow)`、放大镜保留上一颜色、MatchView 该步转重采。
- 必须保持：协议只传**类型与消息**。`Arc<GpuFrame>` **复用既有 `GpuFrame`**（`win/d3d11.rs` 的 device texture + 尺寸，`FrozenFrame` 已是这种组合），不新建平行句柄类型；`GpuFrame` 是资源句柄（可移动），"对它做 context 操作"只能在 GPU 线程；**协议里绝不出现 `ID3D11DeviceContext`**——它属于 GPU 线程自己的结构，进不了任何请求/结果。这三条要写进类型注释，别只靠约定。
- 验收：协议层单元测试覆盖五种情况——最新覆盖（连发 3 个 `SampleCursor` 只执行最后一个）、必达车道丢请求即 `Failed`、`generation` 失效丢弃、取消解除等待、`ReadRegion` 与 `ReadMatchView` 的结果不会互相覆盖；**这些测试不需要真实 GPU**（用假 GPU 线程跑）。
- 注意：不引入无界 channel；协议里唯一的大块是 `ReadRegion` 的像素，且它是**所有权转移**而不是拷贝。
- 回退：`git revert`。
- 风险：中（定义的形状后面三处都要用，改起来会扩散）。

### S2.2 两条导出链路迁移到 GPU 线程

- 前置：S2.1
- 规模：~4 文件（`windows/overlay/session.rs`、`windows/providers.rs`、`windows/export_worker.rs`、`windows/capture_worker.rs`）
- 背景事实：`confirm()`（`overlay/session.rs:341` 起）有**两条**导出链路，只改一条会留下半迁移状态：
  - 无标注：`service.prepare_selection(&frozen.frame, selection, &FrozenFramePixels::new(frozen))`，直接区域读回；
  - 有标注：把标注文档经 renderer 的 `render_export` 重放到离屏目标，再裁到选区（`session.rs:361` 起的注释写明"与预览逐像素一致"是它的目的）。
  两条都在 overlay 线程同步完成。`export_worker.rs:43` 的 `ExportJob` 只接收**已经读回**的 `SelectionPixels`，worker 本身已是 GPU-free —— 所以迁移的只是"回读"这一步。
- 动作：
  1. 无标注链路：`read_region` 改为 S2.1 的 `ReadRegion` 请求。
  2. 有标注链路：D2D 重放仍留在 overlay 线程（D2D 不需要 immediate context），但它渲染到的**离屏目标**要按下面的交接契约移交给 GPU 线程回读。
     今天的代码是同一线程里 `create_render_target_texture` → `draw_to` → `read_back_region_bgra` 一气呵成（`win/d2d.rs:625` 起），跨线程之后这段时序必须写死：
     - 顺序：创建目标　→　`draw_to`（内部 `BeginDraw` / `draw_layers` / `EndDraw`）　→　**`EndDraw` 返回 `S_OK`**　→　才允许交接；
     - 交接 = 所有权转移：目标纹素装进 `Arc<GpuFrame>` 交给 GPU 线程；**交接之后 overlay 不再触碰它**（今天的 `SetTarget` 复位要在交接前完成）；
     - GPU 线程拿到 `Arc` 之后才允许 `CopySubresourceRegion` / `Map`；
     - `EndDraw` 失败 → 该次导出失败并走 S4.4 的失败出口，**不交接**半个渲染结果。
     - D2D 的命令提交**不在** `ID3D11DeviceContext` 上，所以"`EndDraw` 之后能否直接 Copy"必须由像素测试证明，不能靠推断；若实测出现未完成渲染，允许的补救是在 **GPU 线程侧**加显式同步（`Flush` 或 event query），**不允许**把回读搬回 overlay 线程。
  3. 明确 `SelectionPixels` 的生成方：迁移后由 **GPU 线程**产生，经结果信箱交给 overlay，再原样装进 `ExportJob` 移交 export worker。该类型必须 `Send`。
  4. **确认流程的异步状态**（S2.1 禁止 overlay 同步等待，这条不能省，否则只能靠实现者自行发明分支）：
     - `Enter` → 进入 `CaptureState::Exporting`，并在导出子状态里多一个 `ExportPending`（已提交回读请求、等结果）；
     - `ExportPending` 期间再次收到 `Enter`：**忽略**，不产生第二个 `request_id`；
     - `ExportPending` 期间 Esc / `WM_DESTROY` / 设备丢失：取消 pending 请求，该会话**不产出** artifact；export worker 的 stale-generation 规则照旧兜底；
     - 结果回来（或 `Failed`）：成功则原样装进 `ExportJob` 交给 export worker（保持今天"确认后不阻塞消息泵"的行为）；失败经既有事件路径上报原因。overlay 的关闭/恢复沿用现有规则，本任务不新增分支。
  5. 更新 `export_worker.rs` 与 `overlay/session.rs` 里"回读必须留在 overlay 线程"的注释——过期的架构注释比没有注释更坏。
- 必须保持：两条链路各自的导出像素**与迁移前一致**（判据见下条），含带标注场景；"确认后不阻塞消息泵"不变；Esc 仍可取消；失败仍可见；取消路径不泄漏、不 join。
- 验收：无标注与有标注各一组前后对照，两组数字都写进提交消息；**对照判据是解码后的 BGRA + 尺寸 + 透明度 + 元数据**（不是 PNG 文件字节——换编码器后字节必然可能变，见 S4.2）；`cargo test --workspace --lib` + 三个探针全绿；**另跑一次带标注的真机截图**（D2D 行为单测覆盖不到），以及一次"`ExportPending` 期间按 Esc"验证不产出 artifact。
- 注意：两条链路的裁剪语义（选区 ∩ 帧）必须一致，否则尺寸断言会时好时坏。
- 回退：`git revert`。
- 风险：高（已发布的导出路径，且有两条）。

### S2.3 放大镜采样迁移（latest-request）

- 前置：S2.1
- 规模：~3 文件（`windows/renderer.rs`、`win/d3d11.rs`、overlay 取色调用点）
- 背景事实：`renderer.rs:135` 在构造 `Win32Renderer` 时就建好 `AsyncSampleBuffer`（3 槽、32×32、`DO_NOT_WAIT`），overlay 每帧轮询它；迁移后会变成跨线程往返，**必须**定义清楚否则会显示过期颜色。
- 动作：走 S2.1 的 `SampleCursor`：
  1. 请求侧容量 1 且"最新覆盖"——鼠标快速移动时新请求替换 pending 的旧请求，不排队、不积压。
  2. 每个请求带 `cursor_revision`（或等价单调号）；结果回来时**校验 revision，不匹配就丢弃**，绝不把过期颜色画到当前光标位置。
  3. 会话结束时清掉 pending 请求并停止推送（对应 S2.1 的 `Failed(Cancelled)`）。
- 必须保持：取色值与迁移前**同点同色**；取色延迟 P95 不劣于现状（这是本任务的门禁，不是"希望"）；`DO_NOT_WAIT` 的非阻塞性质不变。
- 验收：before/after 两组延迟数字；一条"快速移动光标"的测试用注入的 revision 序列证明不会显示过期颜色。
- 注意：若 P95 显著退化，解法是 GPU 线程**主动推送**采样结果（仍然只有一个写者），**不是**把 context 交回 overlay。
- 回退：`git revert`。
- 风险：高（已发布交互功能，且有帧率耦合）。

### S2.4 context 单写者断言

- 前置：S2.3
- 规模：~2 文件
- 动作：`GraphicsDevice::context()` 记录并校验调用线程；debug 构建下非 GPU 线程调用直接 panic。加一条测试覆盖该断言。
- 必须保持：release 构建不带这个开销（或开销可忽略并写明）。
- 验收：故意在 overlay 线程调一次 `context()`，debug 下必须 panic；去掉后全绿。这条断言是"§4.2 裁决可执行"的唯一证据。
- 回退：`git revert`。
- 风险：中。

### S2.5 泛化区域异步回读

- 前置：S2.4
- 规模：~2 文件（`win/d3d11.rs`）
- 背景事实：现有 `AsyncSampleBuffer` 是 32×32 放大镜专用（`SAMPLE_TILE = 32`、固定 3 槽、只接 32×32 tile）；`read_back_bgra`/`read_region` 是同步的、每次新建 staging。
- 动作：新增一个尺寸/槽数可配的区域异步回读（复用 `DO_NOT_WAIT` 的模式而非那个对象），供 MatchView 与窄条回读使用；槽数由基准决定，不预置。
- 必须保持：`Map` 失败/仍在使用/device removed 都要变成显式诊断，不得无限等待 GPU；**既有 `read_back_bgra` / `read_region` 的语义不许顺带改**（S2.2 只是把调用方搬到 GPU 线程，不是重写它们的裁剪与错误行为）。
- 验收：readback bytes / frame 与耗时被记录；槽数选择有基准数据支撑（而不是文档里武断写死）。
- 回退：`git revert`。
- 风险：中。

### S2.6 `ActiveFrameSource`

- 前置：S2.5
- 规模：~3 文件（`win/wgc.rs` + 新 scroll 模块）
- 背景事实：`win/wgc.rs` 目前只有 `create_item_for_monitor` → `CreateForMonitor`（`wgc.rs:267`）一条路，`capture_monitor` 取一帧即释放 pool；窗口级互操作入口与它同源（`factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()`），但**没有窗口分支**。
- 动作：按 docs/19 §4.1 实现，并把下面四项写成可验收契约（这是本任务的重点，不是"顺手实现"）：
  1. **帧类型与所有权**：`CapturedFrame` 携带 `GpuFrame`（`ID3D11Texture2D` + 尺寸 + `Arc<GraphicsDevice>`）。`GpuFrame` 是**资源句柄，可跨线程移动**；但"对它执行 context 操作（copy/map）"只能在 GPU 线程。frame pool、session 纹素、staging **都归 GPU 线程**——`ActiveFrameSource` 实例本身住在 GPU 线程，只有帧句柄离开。
  2. **降级条件**（写实，不留模糊）：仅当①该系统不支持窗口级互操作（< Win10 1903）、②对该 HWND 建 item 失败（最小化 / 被 DWM 排除 / 窗口已销毁）、③运行时拒绝窗口级捕获 三者之一成立，才降级到 `CreateForMonitor`。**降级前必须确认 overlay 与 controller 已被 `WDA_EXCLUDEFROMCAPTURE` 排除或已隐藏**；做不到就 `TargetUnsupported` 结束，而不是带着自己的窗口入镜继续拼。降级要在诊断里记下 provider（`wgc-window` / `wgc-monitor`）。
     **跨屏目标不是降级理由**：窗口级捕获返回整窗内容，按 docs/14 裁到当前显示器可见区就能得到与显示器捕获相同的像素（且不被遮挡物污染）。docs/19 §4.1 原先把"跨屏裁剪到显示器"也列成降级场景，与本条冲突——**以本条为准**，docs/19 已同步删掉那句话。
  3. **取消如何唤醒**：帧等待用 `WaitForSingleObjectEx` 同时等"帧到达事件"与"取消事件"（manual-reset）；`cancel()` 置位取消事件后 `next_frame` 立刻返回，不依赖 timeout 到期。
  4. 返回值形状：`FramePoll { Frame | Idle | Ended(reason) }`；`start`/`next_frame`/`cancel`/`stop`（`stop` 幂等、Drop 同路径）。
- 必须保持：`Idle` 不得被当成 EOF；`Ended` 由尺寸变化/目标失效/设备丢失产生。
- 验收：① 目标被其它窗口遮挡时窗口级捕获**仍**返回该窗口内容（这是选窗口级的理由，需要真实窗口测试）；② 降级路径下 overlay 与 controller 都不出现在帧里（像素断言）；③ `cancel()` 之后 `next_frame` 在 100 ms 内返回，而不是等满 timeout。
- 回退：`git revert`。
- 风险：高（WGC 窗口级互操作 + 生命周期 + 降级安全性）。

### S2.7 容量 1 信箱与丢帧语义

- 前置：S2.6
- 规模：~2 文件
- 动作：容量 1 最新帧覆盖；帧生命周期状态可诊断；**丢帧检测**（计数增加或 QPC 跳变超阈值）后扩大搜索窗，仍不唯一则 `Uncertain`。
- 必须保持：`Dropped` 不得当 `NoMovement`；`NoNewFrame` 不得当"滚到边界"；丢帧后**不得**继续按普通相邻帧拼接。
- 验收：用 §S0.5 的丢帧注入跑一遍，断言走的是"扩大搜索窗 → 必要时 Uncertain"这条路径。
- 回退：`git revert`。
- 风险：中。

### S2.8 尺寸变化 / 设备移除 / 窗口关闭 / provider 诊断

- 前置：S2.7
- 规模：~3 文件
- 动作：`ContentSize` 变化 → 释放旧 pool + `Ended(WindowChanged|DpiChanged)`（v1 不重建继续拼）；设备移除、目标窗口关闭、重复启动、取消五条路径都走统一 Drop。
- 必须保持：设备只在显示器/设备变化时替换，且替换与 renderer 重建同一时刻；任何路径不泄漏 frame pool / staging / 纹理。
- 验收：五条路径各有测试或探针；连续 20 次开始/取消后线程数、句柄数回落（对齐 docs/19 §11.6 第 7 条）。
- 回退：`git revert`。
- 风险：中。

### S2.9 阶段验收 + tag `scroll-s2`

- 前置：S2.1–S2.8
- 规模：—
- 背景事实：—
- 动作：完整门禁 + 三个探针 + 真机普通截图链路 + 回退演练 + tag `scroll-s2`。
- 必须保持：**普通截图（含带标注）与放大镜行为逐项与 S0.1 基线一致**——这一阶段没有新功能，只有重构，所以"没变"就是成功；S2 专属门禁（放大镜延迟、导出像素一致）两组数字齐备。
- 验收：完整门禁 + 三个探针 + 带标注真机截图各一次；tag 处可编译可测（回退演练）。
- 风险：低。

---

## 6. S3：驱动、稳定等待与交互

> 从这一阶段起会**真的驱动鼠标**。人工测试前先存好手头的工作。

### S3.1 `DriverCommand` / `DriverEvent`

- 前置：S2.8
- 规模：~2 文件
- 动作：按 docs/19 §6.1 定义命令-事件模型；**每个事件都携带 `session_id + generation + request_id`**；包含 `Committed` / `Cancelled` / `Failed { reason }`。
- 必须保持：消费方按 docs/19 §3.1 的四重校验丢弃 stale 事件——否则"丢弃 stale"只是口号。
- 验收：stale 事件（旧 generation / 旧 request_id）被丢弃且有测试；`Committed` 由画布提交产生而不是驱动器自己声称。
- 回退：`git revert`。
- 风险：低。

### S3.2 WheelDriver 竖向

- 前置：S3.1
- 规模：~3 文件
- 动作：`SendInput` + `MOUSEEVENTF_WHEEL`；每步从 1 notch 起；注入前移光标到 region 中心，结束时恢复光标/前台/焦点；目标校验用顶层 ancestor + PID + class hash，并同时验证 foreground。
- 必须保持：不走 `PostMessage(WM_MOUSEWHEEL)`；UIPI/输入失败立即 `InputRejected` 并进入 Partial/Failed，不要求用户无依据地提权。
- 验收：真实窗口（S0.4 夹具 + 一个浏览器）上滚轮产生位移；目标不是前台时不发送；结束后前台窗口与光标恢复。
- 回退：`git revert`。
- 风险：中。

### S3.3 frame-driven settled

- 前置：S3.2
- 规模：~2 文件
- 动作：等新帧 → MatchView → QPC/motion energy/粗位移一致性判断 Moving/AlmostStable/Stable；`stable_samples` 在有证据的 1–3 之间取值；timeout 只作 watchdog。
- 必须保持：settled 是帧证据事件，不是 `Sleep` 到期；动态视频不能单独决定 settled。
- 验收：用 S0.5 的确定性时间源，settled 判定可复现；平滑动画与瞬时跳变两类目标都能正确判稳。
- 回退：`git revert`。
- 风险：中。

### S3.4 闭环步长

- 前置：S3.3
- 规模：~2 文件
- 动作：`overlap_ratio` 控制（目标 0.30–0.40、安全下限 0.20、硬下限 0.12）；`NoMovement` 不参与；步长变化产生新 `request_id`；连续 3 步频繁增减进入 conservative mode。
- 必须保持：任何 `abs(delta) >= viewport_axis`、overlap 不足、尺寸变化都要拒绝。
- 验收：合成夹具上目标区间收敛；频繁抖动时进入 conservative mode（有断言）。
- 回退：`git revert`。
- 风险：中。

### S3.5 横向能力探测与 HWHEEL

- 前置：S3.4
- 规模：~2 文件
- 动作：横向会话前用小步 `HWHEEL` 探测是否真产生位移，结果写入 `AxisCapabilities`；失败 → `HorizontalUnsupported` 并提示，不伪造成功。
- 必须保持：v1 不使用 Shift+垂直滚轮冒充水平轴；PageKey/Shift 回退不混进 v1。
- 验收：在一个不支持横向滚动的目标上得到明确提示（而不是"拼出一张错的图"）；在支持的目标上横向拼接正确。
- 回退：`git revert`。
- 风险：中。

### S3.6 overlay 输入隔离与 controller HWND

- 前置：S3.2
- 规模：~3 文件（`windows/overlay/*` + `windows/hotkey.rs`）
- 动作：
  1. **按 provider 决定 overlay 与 controller 的可见性（2026-10-08 更正：原文"进入 `Scrolling` 前隐藏 overlay"与第 4 条自相矛盾，现统一）**：
     - provider 为 **`wgc-window`（窗口级捕获，本阶段的目标形态）**：**overlay 与 controller 都可以保持可见**——窗口级捕获在物理上不含其它顶层窗口，因此它们不会入镜。**不需要 WDA，也不需要隐藏。**
     - provider 为 **`wgc-monitor` / BitBlt（显示器级回退路径）**：overlay 与 controller **都必须隐藏**，或退而求其次用 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`；面板控件在那条路径上只能全部走热键。
     - 两条路径共用的部分：创建 controller（`SW_SHOWNOACTIVATE`，绝不 `SetForegroundWindow`），它的消息 post 到 overlay 线程处理。
  2. `exclude_overlay_from_capture`（`SetWindowDisplayAffinity`）**只在显示器级回退路径上调用**，并把它当成"优化 + 兜底"，不是正确性依赖（`docs/30` C5 / §24.2：窗口级路径天然不需要 WDA；MS Learn 对 WDA 与 WGC 的关系零处提及，见 `docs/30` OQ-2）。
  3. **会话级热键**（把 docs/19 §5.1 的细节写进任务，别只写"降级路径可用"）：
     - 由 overlay 线程 `RegisterHotKey`，使用**独立 ID**（与 F5 的 `CAPTURE_HOTKEY_ID = 0x5343` 分开，例如 0x5344/0x5345/0x5346）与 `MOD_NOREPEAT`；
     - 注册失败要按错误码分支：`1409`（`ERROR_HOTKEY_ALREADY_REGISTERED`）与其它失败必须可区分——沿用 `HotkeyError::{Conflict, Failed(u32)}` 的既有形状；
     - 失败降级：controller 改为可获取焦点——`SW_SHOW` + 记录原前台窗口 → 取前台；会话结束时恢复原前台窗口与焦点；
     - 退出路径（完成 / 取消 / 异常 / 目标关闭 / 设备丢失 / 窗口销毁）**每一条**都要 `UnregisterHotKey`，并有断言证明注销过。
  4. **controller 与显示器级捕获的冲突（原文自认矛盾，现已由第 1/2 条挑定一条）**——"affinity 失败时 `Paused` 显示 controller"与"monitor 级捕获要求 controller 不入镜"不能同时留着。**唯一不变量是：`controller` 只要可见，就必须确认它不在被捕获的帧里。**
     - provider 为 **`wgc-window`**：可见（窗口级捕获不含它，与 affinity 是否生效无关）；
     - provider 为 **`wgc-monitor`**：`Scrolling` 期间 controller 必须隐藏；`Paused` 时**只有当 frame source 已 stop**（暂停期间确实不采帧）才允许显示，且 Resume 之前必须先隐藏它、再重新开采；
     - 若实现上"暂停"只是不发滚轮但仍在采帧，则 controller 在整个会话中保持隐藏，控制全部走热键。
     - 这条规则要写进代码注释：它属于"看不见的像素污染"，从画面上很难发现。**它是本清单里唯一一处"两条并列规则互相矛盾"的记录，按 §0.1"先定根因再决定改哪里"处理完毕**；`docs/30` §24.4 与 C5 的处置与此一致。
- 必须保持：任何路径都要注销热键、销毁 controller、恢复 overlay 命中模式与前台窗口；不得只加 `WS_EX_TRANSPARENT` 就宣称穿透；**普通截图的 overlay 交互（悬停、滚轮、确认、Esc）与放大镜行为逐项不变**——这是本任务最容易误伤的地方，要有 before/after 证据。
- 验收（五条必须真实执行）：① 目标窗口收到滚轮并滚动；② controller 与 overlay 都不收到滚轮；③ 隐藏 overlay 时 Esc/Enter/暂停热键可用，热键被占用（1409）时降级路径走得通；④ 显示器级降级路径下 overlay **与 controller** 都不出现在帧里（像素断言）；⑤ `Paused` 显示 controller 的那条路径下，帧里仍然没有 controller。
- 回退：`git revert`。
- 风险：高（跨进程输入 + 焦点 + 热键三件事同时动）。

### S3.7 低频状态事件

- 前置：S3.2
- 规模：~2 文件
- 动作：把会话状态经 `AppEvent::Scroll(ScrollProgress)` 上报；generation 由壳的 EventBus 盖戳；壳侧（必要时）显示进度。
- 必须保持：不传纹理/完整帧/画布像素；不逐帧洪泛。
- 验收：一次完整会话的事件序列与状态转移表（docs/19 §3.3）一致；stale 事件被丢弃。
- 回退：`git revert`。
- 风险：低。

### S3.8 v1 只向前 + `ManualPanoramaDriver`

- 前置：S3.4
- 规模：~2 文件
- 动作：确认 v1 driver 只向前；建立 `ManualPanoramaDriver` 接口（被动采帧 + 复用同一 matcher/canvas），不与自动 driver 混写状态。
- 必须保持：手动模式不把"收到帧"当成"滚动成功"；两者共用同一套 `Alignment`/End Confirmation/画布。
- 验收：手动模式下用户自己滚，画布正确；自动与手动不共享可变会话状态。
- 回退：`git revert`。
- 风险：低。

### S3.9 右侧滚动预览（增量 patch / 视口框 / stale 丢弃）

- 前置：S3.3、S3.7；先读 docs/19 §5.6 与 `refer/snow-apps/snow_shot` 的 `ScreenshotScrollingThumbnailWidget`、`ScreenshotScrollingPipeline`、`ScrollingHoverPreview`，并对照 `refer/shot-refer/Crisp-main`/ShareX 的失败诊断。
- 规模：~4 文件；以 capture overlay 的预览状态、patch 生成/传输和原生窗口绘制为主，不把预览放进 GPUI 壳。
- 背景事实：参考实现固定交叉轴 128 px、轴向 256 px tile，首帧 replace、后续 append/prepend；overlap 只替换边缘 patch 以吸收缩放取整误差，hover 请求单飞且按 epoch/content revision 丢弃旧结果。
- 参考边界：Crisp 的 `Stitch*.cpp` 只提供 CPU 拼接与 sticky 边缘测试；ShareX 的 `ScrollingCaptureWindow` 仅在完成后加载整张结果并支持平移，二者都没有采集中实时右侧缩略图。因此不能把“完成后整图预览”当作本任务的实现，也不能物化长图。
- 动作：定义 `PreviewState`/`PreviewPatch`；stitch commit 只生成 patch；在选区右侧创建不抢焦点的预览窗口，空间不足按左侧/上下翻转并夹回工作区；绘制底图暗化层、已提交内容、当前 viewport 高亮框和 Partial/Uncertain 状态；实现有界 tile 与 latest-only patch 通道。
- 动作：预览 patch 的逻辑坐标必须来自 matcher/canvas 的 accepted delta，不能从 ShareX 式的最终整图重新推导；`replaced_extent` 必须记录 overlap 重采样替换量，供拼图和预览像素测试共同校验。
- 必须保持：预览不读回/保存完整 canvas，不阻塞 overlay、滚轮、matcher；预览窗口不进入捕获帧；session/generation/revision 失配不更新；取消、Esc、下一次 F5 清空全部 tile 和 HWND；普通截图 overlay 与放大镜行为不变。
- 验收：合成序列逐 patch 对照最终缩略图像素与逻辑坐标；首帧 replace、连续 append、overlap 替换、横向布局各有测试；Crisp 风格的 known-shift/sticky header/footer/重复纹理/无关帧/尺寸不符用例逐项通过；ShareX 风格的“只能 best guess”用例必须标 Partial/Uncertain；快速连续提交只显示最新 revision；预览只使局部脏区重绘；真实窗口滚动期间右侧预览随每次 commit 更新；首 patch 延迟、P95、patch bytes、峰值 tile bytes 有 before/after 数字。
- 回退：`git revert`；不保留“整图预览”兼容实现。
- 风险：高（overlay 布局、捕获排除、异步 stale 结果）。

### S3.10 F6 贴图热键与原生贴图窗口

- 前置：S3.7；普通截图 artifact 端口可用，滚动 artifact 允许完整或带明确 Partial 标志。
- 规模：~4 文件；先核对 docs/01 的原生 Layered Window 原则、当前 hotkey 注册表和 `refer/shot-refer/Crisp-main` 的 `PinWindow`/`PinStore`，以及 `refer/snow-apps/snow_shot` 的 pinned window 生命周期；不得复制 Qt/GDI 实现。
- 背景事实：F6 不是新的 capture provider；贴图应复用 canonical artifact，窗口是轻量原生置顶窗口；每窗口不建 WebView/GPUI 窗口，不重复创建 D3D device。
- 动作：注册独立 `PIN_CAPTURE_HOTKEY_ID`（与 F5、滚动会话热键不同，冲突错误可诊断）；WM_HOTKEY 只投递 `PinArtifact { artifact_id, generation }`；读取 artifact 后创建不激活、置顶、可拖动、可关闭且不进 Alt+Tab 的原生窗口；限制贴图数量，按实测冻结；失败/取消/device lost 全路径释放资源。
- 必须保持：F6 无可贴产物时不创建空 HWND；滚动进行中不抢会话；贴图不进入后续 WGC/显示器帧；F5、Esc、Enter、暂停与 F6 热键注销/冲突处理互不影响；普通截图复制、保存、历史行为不变。
- 验收：普通截图、滚动完整、滚动 Partial 各成功贴图；贴图解码像素与 artifact 逐像素一致；F6→可见延迟、每窗口资源、20 次创建/关闭后 HWND/Private Bytes 回落有数字；读失败/超尺寸/窗口创建失败不留下空窗口；与现有热键注册冲突测试分别覆盖 1409 与其它 Win32 错误。
- 回退：`git revert`；删除无效旧 pin 路径，不新增兼容层。
- 风险：高（原生窗口、共享 GPU 资源、热键生命周期）。

### S3.11 阶段验收 + tag `scroll-s3`

- 前置：S3.1–S3.10
- 规模：—
- 背景事实：—
- 动作：完整门禁 + 三个探针 + 真机走一遍"进入滚动 → 暂停 → 继续 → Esc/Enter"、右侧预览更新和 F6 贴图 + 回退演练 + tag `scroll-s3`。
- 必须保持：普通截图与放大镜仍与 S0.1 基线一致；预览/贴图窗口不入捕获；所有热键在任何退出路径都已注销。
- 验收：三种终态（Completed / Partial / Cancelled）都能真实到达；预览无旧 revision 闪回；普通/滚动 artifact 均可 F6 贴图；资源回落。
- 风险：中。

---

## 7. S4：画布落盘与导出

> 两条已经实测的约束决定这一阶段的形状：`snapclip-history::CaptureArtifactStore::write` **只接受整幅 PNG 字节**（没有 tile / journal 能力），仓库里**没有任何 WebP 编码路径**。所以 v1 只导 PNG，长图经端口流式交给壳层。

### S4.1 tile 数据结构

- 前置：S1.8
- 规模：~2 文件
- 动作：定义 `ScrollTile`（画布逻辑坐标、紧凑 BGRA、crc32）、`ScrollExportMeta`（含 `tile_count`、`partial`）、`ScrollFormat`。
- 必须保持：tile 之间在逻辑坐标上**不重叠**（覆盖由 coverage bitmap 保证）；重叠只发生在"新帧覆盖旧内容"的提交层。
- 验收：坐标语义有测试（同一逻辑像素只落在一个 tile 上）；checksum 覆盖 `bgra` 全字节、不覆盖 padding。
- 回退：`git revert`。
- 风险：低。

### S4.2 导出端口与流式 PNG 落盘路径

- 前置：S4.1
- 规模：**按调用图核定，不要预设文件数**（上一版写 ~4 文件明显低估）。至少会动到：`snapclip-history` 的编码器与写入入口、`snapclip-model` 里 `CaptureOutput` 的形态、`apps/snapclip` 的 artifact 适配。实施前先 `rg` 出**所有**普通截图的 artifact 调用方与相关测试，列成本任务的必查清单。
- 背景事实（它决定了本任务只有一种可行写法）：`snapclip-history/src/image.rs:59` 的 `encode_png(&Bgra8Image) -> Vec<u8>` 走 `image::DynamicImage::write_to`，要求**整张图已在内存**；它内部还要 `bgra_to_rgba` 再复制一份。`artifact_store.rs:41` 的 `write(&CaptureOutput)` 同样只接受完整字节。也就是说**现有 PNG 路径 = ≥2 份整图 + PNG 字节同时在内存**，与 docs/19 §8"不得 materialize 长图"直接冲突。
  **这两条是"改造前事实"，本任务完成后即失效**：后续任务不得再据"store 只收整幅字节 / 编码器只吃整图"来做设计（docs/19 §8.3 已同步标注）。把这句话留在任务里，是为了说明"为什么要改"，不是为了让后来者继续依赖它。
- 动作（根因解法，只有一条路，不做双实现）：
  1. **唯一调用链**（写死，否则会同时留下两套入口）：
     ```
     像素行 → encode_png_rows(rows, sink) → 临时文件 → 原子 rename → ArtifactRef
     ```
     - 编码器只有一个实现，输入是**行带**。正确的流式 API 是 `Encoder::stream_writer()` → `StreamWriter`（它实现了 `Write`）+ 逐行 `write_all(row)` + `finish()`。
     - **依赖版本要写死**：用 `png = "0.18"`。`snapclip-history` 依赖的 `image 0.25.10` 要求 `png = "0.18.0"`，直接依赖 0.18 才会复用同一份；锁文件里另有 `png 0.17.16`，那条子图是 `tiny-skia → resvg → gpui-component`，与本路径无关——**别拿 0.17 的源码当依据**（上一版就是这么写的，已纠正）。已核 0.18.1 有 `stream_writer`、`impl Write for StreamWriter` 与 `finish`，收尾行为与 0.17 一致。
     - **不要**用 `write_image_data` 逐行喂：该 API 的语义是"写入**整张**图像数据"（一次调用对应一帧），逐行调用会失败或产出错图。
     - 行缓冲是 **RGBA**（`png::ColorType` 没有 BGRA 变体），所以每行要做一次 BGRA→RGBA 转换，用的是同一个行缓冲，不额外驻留整图。
     - 普通截图（像素已在内存）与滚动（tile 在盘上）都走这一个原语：前者是"一次喂入全部行带"的退化情形。
     - `CaptureArtifactStore` 随之改成"接收 sink / 写入流"的形态（blake3 边写边算），不再要求先把整张 PNG 攒进 `CaptureOutput.bytes`；`ArtifactWriter` 端口的语义不变（"已确认的选区 → artifact"），内部换成这条链。**不保留旧函数做兼容**，普通截图的调用方随签名一起改。
  2. **tile → 扫描行的组装契约**（tile 的到达顺序与扫描行顺序**不是一回事**，必须写清）：
     - tile 以画布逻辑坐标 `(x, y)` 为键；导出时按 `y` 升序读行带，**不依赖提交顺序**（提交沿滚动轴单调，导出按 y 扫描）。
     - 画布宽度大于 tile 宽度时，一条扫描行横跨多个 tile：从覆盖该行的每个 tile 取对应行切片，拼进同一行缓冲。
     - 行缓冲大小 = 画布宽 × 4 字节（RGBA），**与画布高度无关**——这是"不物化整图"的关键。
     - 行带所需的 tile 若已被 LRU 换出，从会话临时目录读回；读不到（文件缺失 / checksum 不符）→ 走 S4.4 的失败出口，**不产半张图**。
  3. 峰值内存由"行缓冲 + 编码器缓冲 + 有界 tile cache + 回读窗口"限定；具体数字在 S4.5 用实测冻结，不在本任务里拍。
  4. `snapclip-history` 的写入入口接收 tile/行带而不是整幅 `CaptureOutput`，沿用同一套命名规则、blake3 边写边算与原子 rename；这条边界写进模块注释。
- 必须保持：普通截图导出**解码后**一致——BGRA 像素、尺寸、alpha、元数据（DPI/色彩信息）逐项相同。**不要把"PNG 文件字节一致"当门禁**，而且这次不靠推断：已核实的差异来源是——`image 0.25.10` 的 `PngEncoder::new` 用 `CompressionType::Fast` + `FilterType::Adaptive`（`src/codecs/png.rs` 的默认值），裸 `png::Encoder` 的默认是 `FilterType::Sub` 且非自适应、压缩档位也不同，所以字节会变。**但结论仍以实测为准**：本任务要跑一次对照（同一份像素，旧路径与新路径各编码一次，同时比较**解码像素**与**文件字节**），把实际结果记进提交消息，不要照抄这里的推断。若确实要求字节一致，必须先把 filter/compression/chunk 显式冻结成与旧路径相同再验证。其余保持项：`finish` 只做"齐备 + 校验 + 落盘"，不做重采样；`abort` 幂等且不留临时文件；缺块时 `finish` 必须报错；capture 不 import `snapclip-history`。
- 验收：依赖门禁仍 clean；checksum 不匹配时 `finish` 报错而不是产出半张图；**一张远超内存预算的长图能导出成功**且导出期间 Private Bytes 被记录（S4.5 用它冻结预算）；普通截图重构前后的**解码像素**对照；一条"多 tile 拼一行"的组装测试（画布宽 > tile 宽）。
- 注意：若流式 PNG 被证明不可行，正确做法是**回到 docs/19 §8 改设计**（带证据与实测），而不是保留"流式"措辞却物化整图，也不是加一层封装让老编码器继续工作。**没有"受限物化"这条退路。**
- 回退：`git revert`。
- 风险：中（新增编码路径，但隔离在 history 与壳，不碰 capture）。

### S4.3 tile 化画布与有界 LRU

- 前置：S4.2
- 规模：~3 文件
- 动作：画布改为 tile 化 + 有界 LRU + 会话级临时目录；启动时清理过期目录；磁盘不足立即停止并保留可读 Partial。
- 必须保持：150 MP 只是保护阈值，**不得** materialize 成连续 BGRA（约 600 MB）；内存只保留有界 LRU。
- 验收：**预算内**的大图能拼完；**超过保护预算**时按有界停止——已有 coverage 就返回 Partial，没有就返回 `ResourceLimit`，而不是突破上限去"拼完"。临时目录字节有上限且有测试；内存峰值有断言。
- 回退：`git revert`。
- 风险：中。

### S4.4 部分结果与"没有画布"的语义

- 前置：S4.3
- 规模：~2 文件
- 背景事实：**不是每次停止都有画布。** 首帧失败、首帧前取消、目标无效这几种情况下一个 tile 都没有，此时"生成可打开的 Partial"在物理上不可能。原稿"任何提前停止都产出 Partial"是一条无法满足的验收。
- 动作：把三种出口分开写清：
  1. `abort()`：**尚未产生任何 coverage** 时使用（首帧失败、首帧前取消、目标无效）。不产出 artifact，只上报 Cancelled / Failed 与原因。
  2. `finish(partial = true)`：**仅当 coverage > 0**（至少一个已提交 tile）时使用。返回的 artifact 有效，但调用方必须把"这是部分结果"一并上报，不得悄悄当完整图。
  3. 导出失败后的清理：编码/写盘失败时删掉临时文件与半成品，保留可读的 Partial（若有 coverage）与诊断；`abort` 幂等。
  4. **导出矩形必须二维完整覆盖**——这才是"禁止空洞"能成立的条件，因为**矩形 PNG 无法表达稀疏 coverage**。只查沿滚动轴的连续性是**不够**的：纵向范围连通，仍可能某些 y 只覆盖了部分宽度。
     - 断言拆成两半，**两条都要检查**：
       - 沿滚动轴：coverage 是**单段连续区间**（没有内部空洞）；
       - 垂直于滚动轴：**每个有效 y 覆盖完整目标宽度**（竖向滚动）／**每个有效 x 覆盖完整目标高度**（横向滚动）。
     - 理由：union 模型只会把与现有 union 相邻、且宽度等于视口的条带追加进来（v1 只向前），所以二维完整覆盖本来就是不变量。把它从"假设"变成断言。
     - 检测到不满足（内部空洞，或某行/列覆盖不全）→ 属于 bug 或"未按预期在 gap 前停止"：**不产出 PNG**，回报证据（缺失的行/列区间、最后一个成功 commit）并按失败出口处理。
     - 测试要**构造**一个"纵向连续、但某行只覆盖部分宽度"的 coverage，证明断言会拒绝它——只测纵向连通的用例覆盖不到这个洞。
     - v1 **不采用**的两条替代路线，写清以免实现者自行发明：①"导出最大完整矩形 + 记录裁剪原点"会**静默丢掉**已成功拼接的内容；②"多区域结果"超出 v1 的结果契约。
- 必须保持：`EndUncertain` / `AlignmentRejected` / `DriftBeyondBudget` / `ResourceLimit` / `UserCancelled` 这些终态都要落到上面几条之一；没有任何路径在 gap 处用白色/透明填充；coverage 连续性是被断言检查的，不是被默认假设的。
- 验收：每种停止原因都有测试到达预期出口（有连续 coverage → Partial；无 coverage → 无 artifact + 明确原因）；专门断言"无 coverage 时不产生文件"；另有一条"人为构造非连续 coverage → 不产出 PNG 且回报空洞区间"的测试。
- 回退：`git revert`。
- 风险：中。

### S4.5 导出预算与原子替换

- 前置：S4.4
- 规模：~2 文件
- 动作：定义 `max_export_pixels` / `max_export_dimension` / `max_export_bytes` / `max_export_time` 四条预算，并把**超预算时的结果契约**定死（上一版写的"分段导出 / 导出局部 / 提示裁剪"三选一，没有结果契约等于没写）：
  1. v1 超预算只做一件事：按 coverage 裁成一个**连续的** PNG（不重采样），并在 metadata 里记录 `original_canvas_size` 与 `crop_origin`。
  2. 裁剪规则必须可预测：v1 一律保留**画布起点一侧的连续前缀**（滚动是向前的，用户最关心开头）。不得做居中或"智能"裁剪——那会让同一输入产出的文件无法解释。
  3. 不裁剪（即连裁剪后的结果都超预算）→ 返回 `ResourceLimit`，不产出文件。
  4. 分段导出 / 多文件结果 / 导出中间局部，v1 **不实现**：它们没有结果契约，做出来没人能说清"这几个文件是什么"。
  5. 剪贴板规则：裁剪后仍超过剪贴板安全尺寸 → **不写剪贴板**，只进历史并把原因上报。
  6. 写临时文件 → flush → atomic rename。
- 必须保持：失败保留部分结果与诊断，不污染剪贴板/历史；裁剪只按上面的规则去掉尾部未覆盖/越预算的部分，**不为了"好看"改动保留区的内容**。
- 验收：四条预算各有边界测试；磁盘写失败（S0.5 注入）不会留下半个文件；超预算产出的那张图，其 metadata 里的 `crop_origin` / `original_canvas_size` 可被读回并校验。
- 回退：`git revert`。
- 风险：中。

### S4.6 端到端：结果进历史

- 前置：S4.5
- 规模：~2 文件
- 动作：滚动结果作为 artifact 进 history，走与普通截图相同的落盘与剪贴板规则（含"SnapClip 自己的写入不进历史"那条）。
- 必须保持：**普通截图的用户可见行为与解码像素不变**（产物照旧是 PNG、进 history、可打开、尺寸/DPI 一致）；history 侧不需要为滚动新增**第二套存储**。注意这与"保留旧 API"是两件事——旧的整图 API 与调用链按 S4.2 一起删掉，不要为了"不破坏语义"把它留下来。
- 验收：真机一次滚动截图 → history 里出现该条目、可打开、尺寸正确；部分结果带明确标注。
- 回退：`git revert`。
- 风险：中。

### S4.7 F6 artifact 读取与普通/Partial 贴图回归

- 前置：S4.5、S3.10；S4.6 已证明 artifact 能进入 history。
- 规模：~3 文件；只连接既有 artifact 读取端口、贴图窗口和结果元数据，不增加第二套 PNG 编码/存储。
- 背景事实：F6 接收 artifact 引用而不是像素；滚动 Partial 仍是可打开的 PNG，但必须携带 Partial、coverage、`original_canvas_size`/`crop_origin` 等元数据。
- 动作：实现普通截图、滚动完整、滚动 Partial 三类 artifact 的读取与 pin；共享有限 GPU/CPU 资源；窗口创建失败、读取失败、尺寸超限和 device lost 统一清理。
- 必须保持：普通截图历史/复制/保存/解码像素不变；贴图不进入后续捕获帧；不存在 artifact 时不创建空窗口；旧整图 API 不恢复。
- 验收：三类 artifact 贴图像素逐项等于解码文件；Partial 标志可读；20 次 F6 创建/关闭后资源回落；错误注入没有半初始化 HWND/临时文件；before/after 记录 F6 延迟、峰值内存和句柄。
- 回退：`git revert`。
- 风险：中。

### S4.8 阶段验收 + tag `scroll-s4`

- 前置：S4.1–S4.7
- 规模：—
- 背景事实：—
- 动作：完整门禁 + 普通/滚动完整/Partial 各一次 F6 贴图 + 回退演练 + tag `scroll-s4`。
- 必须保持：普通截图的**用户可见行为与解码像素不变**；旧整图编码 API 已经消失；贴图不进入后续捕获结果。
- 验收：真机完成一次完整滚动截图并存进历史并贴图；另有一次**带 coverage 的提前停止**存进历史、标为 Partial 并贴图；F6 资源回落证据齐全。
- 风险：低。

---

## 8. S5：矩阵、性能与收口

### S5.1 v1 验收矩阵（只跑向前）

- 前置：S4.8、S3.11
- 动作：按 docs/19 §11.4 的 v1 矩阵跑（wheel step × 动画 × settle × 视口 × 轴向 × 内容类型），每组合 ≥10 次，并加权限 / DPI（100/125/150%）/ 负坐标 / 跨屏 / 鼠标与触摸板 / RDP 维度。**往返属于未来双向矩阵，不进 v1 门禁。**
- 必须保持：参数扫掠（找默认值）与正确性门禁（判通过/失败）**分开跑**——前者抽样，后者固定用例。
- 验收：结果表归档；每条失败都有归类（误接受 / 误拒绝 / 提前终止 / 超时）。
- 风险：中（耗时长）。

### S5.2 质量门禁

- 前置：S5.1
- 动作：`blank_pixels = 0`、unexpected transparent gaps = 0、illegal duplicate logical range = 0；**以 false acceptance 为主导指标**（不追求低 false rejection）；100 步累计漂移 ≤2 px。
- 必须保持：重复纹理、低纹理、动态页面允许返回 `Uncertain`——**不得**为了降低 false rejection 而放宽接受条件。
- 验收：每个 `Accepted` 都能回溯到 margin / band consensus / valid area / residual / overlap 证据。
- 风险：中。

### S5.3 性能采样并对 §1.2

- 前置：S5.1
- 动作：记录 readback bytes/frame 与 /session、每步 capture/match/canvas/export 耗时、预览 patch 延迟/bytes/tile 峰值、F6→HWND 延迟、贴图 CPU/GPU/句柄/Private Bytes、CPU、内存、GPU、磁盘写入；与 §1.2 的 `before` 栏对比。
- 必须保持：**"没有不可接受的退化"是判据，不要求数字变好**。数字没变或变差时如实写并给原因。
- 验收：`before` / `after` 两栏齐全，每条差异有解释；未测的写"未测"。
- 风险：中。

### S5.4 冻结默认参数

- 前置：S5.3
- 动作：只依据 S5.1/S5.3 的数据冻结 notches 上限、overlap 目标区间、settle 参数、`stable_samples`、tile 尺寸、readback 槽数；把"为什么是这个值"写进 docs/19。
- 必须保持：没有数据支撑的参数不许冻结（AGENTS.md：性能优化必须有依据）。
- 验收：每个默认值都能指到一组测量。
- 风险：低。

### S5.5 人工验收八条

- 前置：S5.4
- 动作：逐条走 docs/19 §11.6 的八条（浏览器长页面 / 宽表格横向 / 快慢与边界 / 动态与懒加载 / 暂停调整继续 / 右侧预览随 commit 更新且取消不残留 / F6 贴图普通与 Partial / 多显示器负坐标混合 DPI 与 20 次循环资源回落）。
- 必须保持：人工验收也要留证据（截图或日志），不写"试过了没问题"。
- 验收：八条各有可复核的记录。
- 风险：低。

### S5.6 阶段验收 + tag `scroll-s5`

- 前置：S5.1–S5.5
- 规模：—
- 背景事实：—
- 动作：完整门禁 + 真机全链路 + 回退演练 + tag `scroll-s5`；更新 docs/19 的状态（哪些是实测、哪些仍是设计）。
- 必须保持：S5 只冻结参数与阈值，**不新增功能**；每一项默认值都能指回一组测量。
- 验收：docs/19 与 docs/24 的状态一致；没有"已知 TODO 却声称完成"的项目。
- 风险：低。

---

## 9. S6：v2 扩展（**触发式，不满足判据就不执行**）

判据：S5.6 完成，且真实矩阵证明通用路径在目标场景上确实不够用（例如某个高频目标必须靠 PageDown 或 UIA 才能滚动）。

- S6.1 PageKeyDriver（PageDown/PageUp/方向键，仍由图像 delta 确认）
- S6.2 UiaScrollDriver（仅对已验证窗口调 ScrollPattern/SetScrollPercent，有界超时 + quarantine，不遍历 UIA 全树）
- S6.3 `BrowserAdapter` / `NativeFullPageStrategy`（独立模块 + 能力探测 + 失败回退，不得污染 docs/14 的窗口检测热路径）
- S6.4 双向滚动 / 元素级目标（`TargetKind::ClientArea` / `UiElement`，需先由 docs/18 / docs/20 定义元素身份）

每个 S6 任务都要按 §0.4 重新确认扩权，并重跑 S5 的矩阵子集。

**这些条目现在只是判据与范围，不是可执行任务。** 每个 S6.x 开工前必须按 §11.2 的模板展开成完整条目（前置 / 规模 / 动作 / 必须保持 / 验收 / 回退 / 风险），否则不允许开工——照标题开工是最容易产生"做完了但没法验收"的地方。

---

## 10. 完成判据（Definition of Done）

### 10.1 单个任务

- [ ] `git status --porcelain` 干净开始，任务 = 一个提交，消息含任务号与门禁数字。
- [ ] §0.2 的通用门禁全过；受影响的门禁（全量 / UI / 探针）也跑过并留数字。
- [ ] 任务条目里的"必须保持"逐条核对过。
- [ ] 回退点明确（上一个 tag 或本任务前一个提交）。
- [ ] §11 的状态词已更新（**勾选框不是完成状态**）。

### 10.2 单个阶段

- [ ] 阶段内全部任务勾选。
- [ ] 阶段 tag 已推送，且**回退演练**通过（tag 处可编译、可测）。
- [ ] 该阶段相关的资源/性能列已更新（S2 之后尤其重要：普通截图与放大镜是回归重点）。
- [ ] 真机走查一遍与本阶段有关的人工路径。

### 10.3 滚动截图目标（全部完成时可用工具验证）

- [ ] `cargo tree`：`snapclip-capture` 不含 `tauri`/`wry`/`gpui-kit`，也不含 `snapclip-history`、`snapclip-recognize`（由 `tools/check-dependency-direction.ps1` 守住）。
- [ ] immediate context 只有一个使用者：debug 断言在测试里被覆盖，`AsyncSampleBuffer` 与 `read_region` 都不在 overlay 线程。
- [ ] 质量门禁可证：`blank_pixels = 0`、无透明 gap、无重复逻辑范围；每个 `Accepted` 可回溯到证据。
- [ ] 漂移可证：100 步合成序列总误差 ≤2 px，且重锚定不产生 gap / 重复。
- [ ] 部分结果诚实：**有 coverage** 的提前停止产出可打开的 Partial；**没有 coverage**（首帧失败 / 首帧前取消 / 目标无效）时只报原因、不产出文件。**没有任何路径**用白色/透明填充洞。
- [ ] 导出不物化整图：长图导出期间峰值内存由行带窗口 + 编码器缓冲 + 有界 tile cache 限定（S4.5 的实测数字为证）。
- [ ] 右侧预览只消费有界 `PreviewPatch`：首帧 replace、后续 append/prepend、overlap 替换、viewport 高亮、stale revision 丢弃和会话清理均有证据。
- [ ] F6 只贴已有 canonical artifact：普通/完整滚动/Partial 三类像素一致，贴图窗口不抢焦点、不进入捕获，错误路径不留空 HWND，资源循环后回落。
- [ ] v1 只输出 PNG；文档中不再出现"PNG/WebP"这种与实现不符的表述。
- [ ] docs/19 与本文的状态同步更新，并把"哪些结论是实测、哪些仍是设计"写清。

---

## 11. 状态词表与执行记录模板

### 11.1 状态词表

**完全沿用 `docs/23 §13.1` 的七个状态**（未开始 / 进行中 / 门禁失败 / 已验证 / 已提交 / 已推送 / 已回退），不另造词。规则同那里：**勾选框 ≠ 完成**；声明完成必须同时具备"已验证 + 已推送 + 本节记录填全"。

### 11.2 执行记录模板（每个任务复制一份）

```
任务编号：S2.1
状态：未开始 / 进行中 / 门禁失败 / 已验证 / 已提交 / 已推送 / 已回退
分支：scroll/s2-framesource（或 <默认分支>）
前置提交/tag：scroll-s1（sha: …）
修改范围：<文件清单 + 大概行数>
修改前测试：cargo test --workspace --lib → 475 passed / 9 ignored
修改后测试：<同上格式，写出真实数字>
受影响门禁：<全量 / UI / 探针 / 依赖方向 —— 跑了哪些、结果如何>
性能指标：<只写与本任务相关的；没测就写"未测"，不要留空>
人工验证：<真机步骤 + 观察到的日志/现象>
失败与根因：<红了什么；根因属于实现/接口/数据结构/边界/流程/抽象 哪一类>
提交 SHA：<sha>
推送/tag：<origin/<分支> 已推送 / tag scroll-s2 已推送>
回退对象：<上一个 tag 或本任务前一个提交的 sha>
```

**滚动截图特有的四个必填项**（缺任何一项就不算填全）：

```
位移证据：<本次 Accepted 的 margin / band deltas / valid area / residual / overlap，或"本任务不涉及">
停止原因：<ScrollStopReason 的实际取值，或"本任务不涉及">
readback：<bytes/frame 与 bytes/session；S2 之后必填>
是否部分结果：<本次导出是完整还是 Partial；Partial 时必须写出触发它的原因>
```

填写纪律（与 docs/23 §13.2 一致）：数字必须来自实际命令输出；探针要写明第几次跑；"没做/没测"就明确写"未完成"。

---

## 12. 执行记录（实际填写）

> 逐任务按 §11.2 模板追加。状态词只用 §11.1 的七个。

（尚无记录。S0.1 完成后从这一节开始追加。）
