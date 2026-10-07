# SnapClip 模块化与插件化重构执行任务清单（docs/23）

> 文档状态：开发期执行清单（可逐步勾选，可直接交给 agent 执行）
>
> 依据：`docs/22-snapclip-modular-plugin-architecture.md`（审核修订版）。与实现或实测冲突时，**以实测为准**，并把冲突写回 `docs/22`。
>
> 目标：按本文顺序执行完所有任务后，得到——4 个能力 crate + 1 个 GPUI 壳、依赖方向由工具守住、截图高频路径零 IPC、识别能力按需启动、Tauri 与旧目录删除，并且**每一步都有前后测试证明"新功能可用、老功能不退化"**。
>
> 重要前提（本项目既定）：**尚未正式发布，允许破坏性重构**。不考虑向后兼容，不做兼容层、不做适配器堆积；但**不允许无意破坏当前功能**，每个任务都要跑门禁。

---

## 0. 使用规则

### 0.1 勾选与完成规则

- [ ] 只有"门禁命令全部通过 + 数字与基线一致（或按预期变化并写明原因）"才允许勾选。
- [ ] 每个任务独立提交、独立推送；提交消息里带上门禁数字（现有习惯）。
- [ ] 不得通过删除测试、放宽断言、跳过 `#[ignore]` 探针、关闭功能来制造通过。
- [ ] 任务里的"必须保持"条目是回归清单：任何一条被破坏，即使测试是绿的，也视为失败。
- [ ] 发现问题先定根因（实现 / 接口 / 数据结构 / 模块边界 / 调用流程 / 抽象），再决定改哪里；不要用临时分支绕过。

### 0.2 每一步的通用门禁（复制执行）

在 `D:\100_Projects\110_Daily\SnapClip\src-tauri` 下：

```powershell
cargo test --lib
cargo check --all-targets
cargo test --lib ring_contrast_probe -- --ignored --nocapture      # A4 底色表，对照 docs/21 §5.26
# 探针必须串行，之间停 3 秒，避免互相抢焦点
cargo test --lib browser_element_probe -- --ignored --nocapture; Start-Sleep -Seconds 3
cargo test --lib explorer_rule_probe -- --ignored --nocapture
```

然后再做一次**真机链路**：`npm run tauri dev`（或现有启动方式）→ F5 → 悬停/滚轮/确认或 Esc → 看会话汇总里 `present_us` 与 `over16ms`。

改了 UI 文案或新增中文字串时追加：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File subfont/subset.ps1
cargo test --lib the_embedded_subset_covers_the_strings_the_overlay_draws
```

### 0.3 环境纪律（探针）

- 探针需要真实窗口（浏览器由探针自己拉起；Explorer 需要本机有一个可见的资源管理器窗口）。
- 红了**先排除环境**（窗口不存在、被遮挡、机器高负载），复跑 2 次；复跑后仍稳定红才当回归。
- 已实测过一次假红：Explorer `provider_hit_available=10/25` 而几何逐位不变，复跑回到 25/25。**不许把这类假红当噪声忽略，也不许把它当回归阻塞**：复跑结果写进提交消息。
- 探针**不得**因为"跨 crate 了"而失效；哪个探针需要 Tauri 才能跑，说明拆分没做完。

### 0.4 提交、tag 与回退

- 每个阶段结束打一个 tag（`refactor-p0`、`refactor-p05`、`refactor-p1`…），作为回退点。
- 任何一步失败的回退：`git reset --hard <上一个阶段 tag>`（只动本次工作区，不 push 破坏远端历史）。
- 最终回退基准：`smart-snapping-v1-2026-10-07`（V1 功能封版）。

### 0.5 允许的破坏性改动清单（本次明确批准）

- 删除 `src-tauri/src/capture/platform/*`（11 行转发层）与 `capture/mod.rs` 里的 `pub mod platform;`。
- 删除非 Windows 编译分支（本项目 Windows-only）：`capture/application/runtime.rs` 的 `#[cfg(not(windows))] UnsupportedOverlay` 及其引用。
- 移动/重命名模块与公共 API（`snapclip-capture`/`snapclip-history`/`snapclip-recognize`/`snapclip-model` 的接缝）。
- 删除 `commands/*`、Tauri `events` 适配、Vue adapter 与旧 `src-tauri` 组合根（P6）。
- **不在**批准范围内：删除 `capture/application` 的端口语义（`CaptureEventSink`/`CaptureRuntime`/`OverlayPlatform`）、改变截图 overlay 的线程模型、改变 IPC 事件名与载荷（前端契约）。

---

### 0.6 git 纪律与恢复（**每一个任务都适用**）

大重构最大的风险不是写错代码，是"改坏了又回不去"。本节是硬性要求。

**提交粒度与前置条件**

- 每个任务 = **一个提交**，提交消息第一行写清任务号（例如 `refactor(T1.6): split overlay.rs into input/state/render`），正文带上门禁数字。
- 任务开始前工作区必须**干净**：`git status --porcelain` 为空。存在别人的手稿（例如 `prototypes/` 下的实验文件）时**先确认归属**，不要顺手提交或丢弃；不相关改动先单独提交或 `git stash push -m "…"`。
- **禁止把红的树留在 main 上**。门禁没过就不要提交；确实需要中途保存进度时，提交为显式的 `wip(T1.6): …（门禁红，未完成）` 并留在**分支**上，不要推到 main。

**分支与 tag：回退点必须存在且可达**

- 高风险阶段开分支：`git switch -c refactor/p1-capture`；阶段验收通过后 `git switch main && git merge --no-ff refactor/p1-capture`（历史里留下一个阶段合并点，回退粒度 = 一个 merge）。分支期间 **main 始终保持可用**。
- 每个阶段结束打 **annotated tag** 并推送：`refactor-p0`、`refactor-p05`、`refactor-p1`、`refactor-p2`、`refactor-p3`、`refactor-p4`、`refactor-p6`。`git tag -a refactor-p1 -m "…" && git push origin refactor-p1`。
  **tag 不推远端等于没打**：本地仓库损坏时远端就是唯一回退点。
- 任务条目里的"回退"统一指向**具体对象**（上一个提交 sha 或上一个 tag 名），不要写"回退一下"。

**场景 → 命令（照抄）**

| 场景 | 命令 | 说明 |
| --- | --- | --- |
| 任务做到一半发现方向不对，还没提交 | `git stash push -m "T1.6 wip"` 然后 `git switch main`（或 `git reset --hard refactor-p1`） | 先保命再重做；`git stash list` 能找回 |
| 想在动手前留一份保险 | `git branch backup/pre-T1.6` | 比 reflog 好找；确认成功后 `git branch -D` |
| 已提交但发现坏了，历史已推送 | `git revert <sha>` | **优先用 revert**，保留可追溯性 |
| 已提交但坏得彻底，且确认没人基于它工作 | `git reset --hard refactor-p1` + `git push --force-with-lease` | 单人开发可用；**必须 `--force-with-lease`**，不要裸 `--force` |
| 误删/误 reset，找不到提交 | `git reflog`（默认保留 90 天）→ `git branch rescue/<sha> <sha>` | reflog 是最后一道保险，但它不是备份 |
| 需要把 tag 指到新提交 | `git tag -d <tag> && git tag -a <tag> -m …` + `git push --force-with-lease origin <tag>` | 本项目做过一次（V1 tag 挪到收尾提交） |
| 只想看某个文件的旧版本 | `git show refactor-p1:src-tauri/src/.../overlay.rs` | 不用切换工作区 |

**阶段收尾必做一次"回退演练"**（这是本节存在的意义）：

```powershell
git switch --detach refactor-p1
cd src-tauri; cargo test --lib; cargo check --all-targets   # 回退点必须自身可编译、可测
cd ..; git switch main
```

回退点编译不过 = 这个阶段的 tag 是假的，必须修好再往前走。

**禁止事项**

- 不用 `git checkout -- <file>`、`git clean -fd` 抹掉不确定归属的改动（本项目 `prototypes/` 下有用户手稿）。
- 不在没有备份分支/tag 的情况下执行 `git reset --hard`。
- 不为了让门禁变绿而改历史（例如把失败提交 reset 掉再重写）：`revert` 优先，历史留痕。

---

## 1. 基线（动手前记录；每个阶段对照）

### 1.1 正确性基线（已有，复跑确认即可）

| 门禁 | 期望值（V1 封版实测） | 本阶段结果 |
| --- | --- | --- |
| `cargo test --lib` | 403 passed / 0 failed / 6 ignored | [ ] |
| `cargo check --all-targets` | 0 warnings | [ ] |
| `browser_element_probe` | `asserted=41 passed=41 failed=0` / `available=52` / `finer=0` | [ ] |
| `explorer_rule_probe` | `control_level_points=12/25` / `median_area_pct=65.8` / `available=25/25` / `finer=0` | [ ] |
| A4 底色表（`ring_contrast_probe`） | 与 docs/21 §5.26 表格一致 | [ ] |
| 真机会话汇总 | `over16ms=0`；`present_us` 量级 500–2000 µs | [ ] |
| 字体子集门禁 | 通过（子集 20.3 KB） | [ ] |
| 事件契约（`events::ALL_EVENT_NAMES` 与 `src/shared/contracts.ts` 同步） | 通过 | [ ] |

### 1.2 资源基线（P0 新测，before / after 两栏）

| 指标 | 怎么测 | before | after（P0.5 之后） |
| --- | --- | --- | --- |
| 进程数 / 线程数 | 任务管理器（详细信息），或 `Get-Process | Where-Object { $_.ProcessName -like '*snapclip*' } | Select-Object Name,Id,Threads.Count,WorkingSet64` | | |
| 常驻内存 / GPU 内存 | 任务管理器对应列 | | |
| 空闲 CPU（30 s 平均） | 任务管理器，静止不操作 | | |
| 冷启动到可交互 | 启动日志 `setup begin` → `webview page_load Finished` 的 elapsed_ms | | |
| 包体积 | 产物目录大小 + 主 exe 大小 | | |
| 改一行共享 crate 的增量编译 | 改 `snapclip-model` 里一行注释后 `cargo check --workspace` 计时 | | |

> **必须 before 先测**：P0.5 的 OCR 惰性化唯一能量化的收益就是这张表的两栏之差。

### 1.3 规模基线（要搬的代码量，来自实测）

| 模块 | 行数 / 文件数 | 去向 |
| --- | --- | --- |
| `capture/`（含 window_detection、application） | 10 686 / 20 | `snapclip-capture`（值对象部分去 `snapclip-model`） |
| `platform/windows/capture/`（含 `win/`） | 18 256 / 20 | `snapclip-capture/src/windows/` |
| `domain/` | 458 / 6 | `snapclip-model` |
| `platform/windows/clipboard/` | 1 210 / 6 | `snapclip-history` |
| `application/`（capture_service、clipboard_ingest） | 1 179 / 3 | 端口留 capture / `clipboard_ingest` 去 history |
| `infrastructure/store/` | 1 785 / 2 | `snapclip-history`（拆 repository） |
| `ocr/` | 829 / 6 | `snapclip-recognize` |
| `events/`、`commands/`、`app/` | 920 / 10 | 事件摘要去 `snapclip-model`；其余留壳（P6 删） |

---

## 2. 任务总览（进度表）

| 编号 | 任务 | 依赖 | 规模 | 风险 | 状态 |
| --- | --- | --- | --- | --- | --- |
| T0.1 | 复跑并记录正确性基线 | — | — | 低 | [ ] |
| T0.2 | 记录资源基线（before） | T0.1 | — | 低 | [ ] |
| T0.3 | 建 workspace 骨架 | T0.1 | 2 文件 | 中（Tauri 构建） | [ ] |
| T0.4 | 建 `snapclip-model` 骨架 | T0.3 | 3 文件 | 低 | [ ] |
| T0.5.1 | 删除 `capture/platform` 转发层 | T0.4 | 11 行 | 低 | [ ] |
| T0.5.2 | OCR 事件出口改框架无关 trait | T0.5.1 | 2 文件 | 中（IPC 契约） | [ ] |
| T0.5.3 | OCR 惰性启动 | T0.5.2 | 2 文件 | 中 | [ ] |
| T0.5.4 | 记录资源基线（after）并对比 | T0.5.3 | — | 低 | [ ] |
| T1.1 | `snapclip-capture` 骨架与依赖 | T0.5.4 | 2 文件 | 低 | [ ] |
| T1.2 | 定义 capture 公共接缝（端口 + 无 pub 字段审查） | T1.1 | 3 文件 | 中 | [ ] |
| T1.3 | 值对象迁入 `snapclip-model` | T1.2 | ~10 文件 | 中 | [ ] |
| T1.4 | 迁移平台无关 capture（20 文件） | T1.3 | 10 686 行 | 中 | [ ] |
| T1.5 | 迁移 Windows 实现（20 文件） | T1.4 | 18 256 行 | 高 | [ ] |
| T1.6 | 拆 `overlay.rs`（5 职责） | T1.5 | 4 423 行 | 高 | [ ] |
| T1.7 | `uia_provider.rs` 测试搬家 | T1.5 | 3 338 行（测试 72%） | 中 | [ ] |
| T1.8 | 拆 `d2d.rs`（4 pass + 文本） | T1.5 | 3 805 行 | 中 | [ ] |
| T1.9 | 依赖方向与接缝门禁落地 | T1.5 | 1 脚本 | 低 | [ ] |
| T1.10 | 阶段验收 + tag `refactor-p1` | T1.6–T1.9 | — | 低 | [ ] |
| T2.1 | `ArtifactStore`/`ArtifactRef` 定死 | T1.10 | 2 文件 | 中 | [ ] |
| T2.2 | `snapclip-history` 骨架 | T2.1 | 2 文件 | 低 | [ ] |
| T2.3 | 拆 `store/mod.rs`（连接/仓库/迁移） | T2.2 | 1 785 行 | 高 | [ ] |
| T2.4 | 迁移剪贴板 Windows 适配（6 文件） | T2.3 | 1 210 行 | 中 | [ ] |
| T2.5 | 迁移 `clipboard_ingest`（去重/格式/publication） | T2.4 | 694 行 | 中 | [ ] |
| T2.6 | `ClipboardService`/`HistoryService` 公共 API | T2.5 | 3 文件 | 中 | [ ] |
| T2.7 | `commands/history.rs`、`commands/ocr.rs` 改走服务 | T2.6 | 2 文件 | 低 | [ ] |
| T2.8 | 阶段验收 + tag `refactor-p2` | T2.7 | — | 低 | [ ] |
| T3.1 | `snapclip-recognize` 骨架（迁 `ocr/`） | T2.8 | 829 行 | 低 | [ ] |
| T3.2 | 惰性 + 取消 + 超时 + 缓存 + 熔断 | T3.1 | ~4 文件 | 中 | [ ] |
| T3.3 | 壳接线（history 只发 `ArtifactRef`） | T3.2 | 2 文件 | 中 | [ ] |
| T3.4 | 资源对比 + 阶段验收 + tag `refactor-p3` | T3.3 | — | 低 | [ ] |
| T4.1 | 开工前研读 GPUI 规范与组件文档 | — | — | 低 | [ ] |
| T4.2 | `apps/snapclip` 骨架（init/Root/单窗口） | T4.1, T3.4 | ~5 文件 | 中 | [ ] |
| T4.3 | history 能力（Entity + 虚拟列表 + `ElementId`=clip id） | T4.2 | ~4 文件 | 高 | [ ] |
| T4.4 | settings 能力 | T4.3 | ~3 文件 | 中 | [ ] |
| T4.5 | 事件桥（channel + `cx.spawn` + 丢弃过期） | T4.3 | 2 文件 | 高 | [ ] |
| T4.6 | 托盘（Win32） | T4.2 | 1 文件 | 中 | [ ] |
| T4.7 | 测试三层 + 无障碍树断言 | T4.3–T4.6 | ~4 文件 | 中 | [ ] |
| T4.8 | 性能对比（对 §1.2 基线） | T4.7 | — | 中 | [ ] |
| T4.9 | 阶段验收 + tag `refactor-p4` | T4.8 | — | 低 | [ ] |
| T5.x | 进程插件边界（**触发式**，见 §9） | 判据成立 | — | 高 | [ ] |
| T6.x | 删除 Tauri 与旧目录 | T4.9 | — | 中 | [ ] |

---

## 3. P0：基线 + workspace 骨架

### T0.1 复跑并记录正确性基线

- 前置：无（当前 `main` 已绿）
- 动作：
  1. 按 §0.2 跑全部门禁，把实际数字填进 §1.1 表格。
  2. 数字与期望不一致时**停下来**：先排除环境（§0.3），仍不一致就说明起跑线不对，先修再继续。
- 必须保持：无（只读操作）
- 验收：§1.1 表格全部勾选，数字与期望一致。
- 回退：无
- 风险：低。这一步的价值是把"起跑线"写成可对照的数字，后面每一步都跟它比。

### T0.2 记录资源基线（before）

- 前置：T0.1
- 动作：
  1. 按 §1.2 表格逐项测量，填 `before` 栏（命令与口径写在同一张表里）。
  2. `before` 必须在启动应用、**不触发任何 OCR** 的状态下测。
- 必须保持：无
- 验收：§1.2 的 `before` 栏填满；数值写进提交消息。
- 回退：无
- 风险：低。**注意顺序**：before 必须在 T0.5.3 之前完成。

### T0.3 建 workspace 骨架

- 前置：T0.1
- 动作：
  1. 根目录新增虚拟 workspace `Cargo.toml`：`[workspace] members = ["src-tauri", "crates/*", "apps/*"]`、`resolver = "2"`，需要时补 `[workspace.package]`/`[profile.release]`。
  2. 让现有 `src-tauri/Cargo.toml` 继承 workspace（保持 `[package]` 与 Tauri 配置不变）。
  3. 确认 Cargo.lock 位置变化后，`cargo test --lib --manifest-path src-tauri/Cargo.toml` 与 `cargo check --workspace --all-targets` 都通过。
  4. 真机跑一次 `npm run tauri dev`（或现有启动方式）+ F5，确认 Tauri 构建与 overlay 不受影响。
- 必须保持：Tauri 构建可用；`tauri.conf.json`、前端构建脚本不变。
- 验收：§0.2 的门禁全过；真机 F5 一次成功。
- 回退：删除根 `Cargo.toml`，恢复 lock 位置。
- 风险：中（Tauri CLI 与 workspace 的交互是唯一不确定点，用真机构建兜住）。

### T0.4 建 `snapclip-model` 骨架

- 前置：T0.3
- 动作：
  1. `crates/snapclip-model/`：`Cargo.toml`（仅标准库 + serde）+ `src/lib.rs`。
  2. 建 `ids.rs`、`geometry.rs`、`artifact.rs`、`events.rs`、`error.rs`、`recognition.rs` 六个空模块，并在 `lib.rs` 里 `pub use`。
  3. 先只把**值对象**搬进来：`Rect`/`Point`/`ImageDimensions`（从 `capture/geometry.rs` 复制定义，原位置改为 `pub use snapclip_model::…` 的**过渡 re-export**，P1 结束前删除）。
  4. 为值对象写单测（边界、负坐标、包含关系），保证与旧行为逐位一致。
- 必须保持：`capture::geometry::Rect` 的语义与测试不变（过渡期靠 re-export 保证）。
- 验收：`cargo test -p snapclip-model` 通过；`src-tauri` 的 403 测试仍全过。
- 回退：删除 crate + 撤销 re-export。
- 风险：低。**过渡 re-export 必须在 T1.10 之前删干净**，不留兼容层。

---

## 4. P0.5：低风险边界收敛

### T0.5.1 删除 `capture/platform` 转发层

- 前置：T0.4
- 规模：2 文件 / 11 行
- 动作：
  1. `rg -n "capture::platform" src-tauri/src` 列出引用方（预期：**0 处**——目前 `app/capture.rs` 已直接引用 `platform::windows::capture::overlay::WindowsOverlay`）。
  2. 删除 `src-tauri/src/capture/platform/{mod.rs,windows.rs}`，并删掉 `capture/mod.rs` 中的 `pub mod platform;` 与其模块文档里的对应句子。
  3. 删除 `capture/application/runtime.rs` 里 `#[cfg(not(windows))]` 的 `UnsupportedOverlay` 分支与 `#[cfg(not(windows))] pub fn start()`；确认 `rg -n "UnsupportedOverlay"` 为空。
- 必须保持：`CaptureEventSink`、`CaptureRuntime`、`OverlayPlatform` 三个端口的语义与调用方式不变。
- 验收：§0.2 门禁全过（`cargo test --lib` 仍 403）。
- 回退：`git revert`。
- 风险：低。

### T0.5.2 OCR 事件出口改框架无关 trait

- 前置：T0.5.1
- 规模：约 2 文件（+ 壳里的适配器）
- 背景事实：`src/ocr/worker.rs` 持有 `tauri::AppHandle` 并调用 `events::emit`（`ocr-status-v1`）；`events/mod.rs` 是版本化信封（`schemaVersion` + `generation`）且**事件名与 `src/shared/contracts.ts` 必须同步**（有契约测试 `ALL_EVENT_NAMES`）。
- 动作：
  1. 新增 `src/ocr/events.rs`：`pub trait OcrEventSink: Send + Sync + 'static`，方法签名照 `CaptureEventSink`（`on_status(&self, clip_id: &str, status: &str, engine: &str, code: Option<&str>)`）。
  2. `ocr/worker.rs`：把 `app: tauri::AppHandle` 换成 `sink: Arc<dyn OcrEventSink>`，`emit_status` 改为调 sink；**事件名、载荷字段与语义完全不变**。
  3. 壳（`app/mod.rs`）新增 `TauriOcrEventSink`，把原先的 `events::emit(...)` 逻辑搬进去实现 trait。
- 必须保持：IPC 事件名 `ocr-status-v1`、载荷字段、`schemaVersion`/`generation` 语义不变；`src/shared/contracts.ts` 无需改动（若改了，说明契约被破坏，改回来）。
- 验收：§0.2 门禁全过；真机做一次"含文字的截图 → 历史里能看到 OCR 文本"，前端控制台无未知事件报错。
- 回退：`git revert`。
- 风险：中（唯一风险是事件契约被无意改动，用"前端不报错 + 契约测试"兜住）。

### T0.5.3 OCR 惰性启动

- 前置：T0.5.2
- 规模：约 2 文件
- 背景事实：`app/mod.rs` 的 setup 里 `OcrManager::new` + `OcrService::start(store, app.handle(), engine)` 是**无条件**执行；启动日志因此每次都有 `[snapclip][startup] ocr worker started elapsed_ms=…`。
- 动作：
  1. 先读 `src/ocr/win_ocr.rs`、`manager.rs`：确认构造是否已经加载语言/模型；若构造即加载，把初始化推迟到首次 `recognize`。
  2. `OcrService::start(...)` 改成 `OcrService::new(...)`（不起线程/进程），内部加 `ensure_started()`；`try_enqueue` 首次调用时启动。
  3. 启动日志：未触发 OCR 时**不再**打印 `ocr worker started`；首次任务启动时打印一次，并带上 `trigger=first-task`。
- 必须保持：OCR 结果的正确性、重试、取消、历史关联不变；队列上限/去重行为不变。
- 验收：§0.2 门禁全过；启动日志中"未触发 OCR 的会话"没有 `ocr worker started`；触发一次 OCR 后出现一次；T0.2 的资源表 `after` 栏填写并对比（线程数与常驻内存应下降）。
- 回退：`git revert`。
- 风险：中（低风险改动，但要小心"首次任务"路径的竞态：两个任务同时首次入队只允许启动一次 worker）。

### T0.5.4 资源 after 与阶段收尾

- 前置：T0.5.3
- 动作：
  1. 按 §1.2 填 `after` 栏，与 before 并列写进提交消息。
  2. 打 tag `refactor-p05`。
- 必须保持：§1.1 全部门禁仍绿。
- 验收：资源表两栏齐全；before/after 的差异能解释（线程数减少、常驻内存减少）。
- 风险：低。

---

## 5. P1：抽离 `snapclip-capture`

> 本阶段是整次重构最大的单点。纪律：**一次只做一件事，每做完一个文件/一个职责就跑一次 §0.2 门禁**。

### T1.1 `snapclip-capture` 骨架与依赖

- 前置：T0.5.4
- 动作：
  1. 新增 `crates/snapclip-capture/`：`Cargo.toml` 依赖 `snapclip-model` + Windows SDK（`windows`/`windows-sys` 与现有 `src-tauri` 一致）+ `serde`/图像编码等按需。
  2. `lib.rs` 先只 `pub mod` 空模块，`cargo check -p snapclip-capture` 通过。
  3. 在 `snapclip-capture` 里加一条 `#![cfg(windows)]`（或等价）声明它是 Windows-only crate。
- 必须保持：`src-tauri` 仍可编译（两个 crate 暂时并存）。
- 验收：`cargo check --workspace --all-targets` 0 warning。
- 回退：删除 crate。
- 风险：低。

### T1.2 定义 capture 公共接缝

- 前置：T1.1
- 动作：
  1. 把 `capture/application/{mod,runtime}.rs` 的端口语义原样迁为 `crates/snapclip-capture/src/ports.rs`（`CaptureEventSink`、`OverlayPlatform`）与 `service.rs`（`CaptureRuntime` + 公开方法）。
  2. 逐个审查**要跨 crate 暴露**的类型：`RenderView`、`OverlayFrameState`、`RenderMetrics`、`ChainRingView`、`RingOptions`、`AbandonedHint` 等**画笔/状态机内部类型不得 re-export**；确实要暴露的（如 `CaptureState`、`CaptureArtifact`、`CaptureError`）改为 builder + reader 方法（GPUI 规范：no `pub` fields across the seam）。
  3. 写下"接缝清单"（哪些类型/方法是公共 API）进 crate 文档注释。
- 必须保持：端口语义不变；`app/capture.rs` 与 `commands/capture.rs` 的调用方式不变。
- 验收：§0.2 门禁全过。
- 回退：`git revert`。
- 风险：中（决定哪些类型是公共 API，宁可保守：先不暴露，需要时再开）。

### T1.3 值对象迁入 `snapclip-model`

- 前置：T1.2
- 动作：
  1. 把 `Rect`/`Point`/`ImageDimensions` 的**唯一实现**移进 `snapclip-model`（T0.4 的过渡 re-export 替换为真正的迁移）。
  2. `capture/geometry.rs` 只保留**派生布局**：`window_rect_to_local`、标签/放大镜摆放、尺寸标签定位等；其单测随函数一起留下。
  3. 迁 `domain/` 的 id 类型（`ClipId`/`ArtifactId`/`SessionId`/`RecognitionTaskId`）与 `CaptureState`、`CaptureArtifact`、`Publication` 等到 `snapclip-model` 对应模块。
- 必须保持：所有坐标语义与现有测试逐位一致；`docs/21 §12` 的坐标契约（显示器本地物理像素 ↔ 虚拟桌面物理像素）测试必须原样通过。
- 验收：§0.2 门禁全过；**坐标契约测试**与负坐标/混合 DPI 用例全绿。
- 回退：`git revert`。
- 风险：中（这一步决定后面所有 crate 的类型归属，做慢一点）。

### T1.4 迁移平台无关 capture（20 文件 / 10 686 行）

- 前置：T1.3
- 动作：按清单逐个迁（每迁一个跑一次门禁）：
  `capture/{annotation,diagnostics,error,geometry,monitor_cache,ring_contrast,sampler,session,mod}.rs`、
  `capture/window_detection/{deep,gesture,hit_test,model,provider,snapshot,transition,uia,mod}.rs`、
  `capture/application/{mod,runtime}.rs`（已迁为 ports/service）。
- 必须保持：`ring_contrast` 的调色板仍是**唯一一处**定义（paint 层 import 它，见 docs/21 §5.26）；`window_detection` 的全部单测（deep 47% 是测试）随之搬走。
- 验收：§0.2 门禁全过；`rg -n "crate::capture::" src-tauri/src` 的残留逐条清零。
- 回退：按文件 `git revert`。
- 风险：中（纯搬移，风险主要来自 `use` 路径与测试模块）。

### T1.5 迁移 Windows 实现（20 文件 / 18 256 行）

- 前置：T1.4
- 动作：迁 `platform/windows/capture/` 全部文件到 `crates/snapclip-capture/src/windows/`：
  `capture_worker.rs`、`detection_worker.rs`、`export_worker.rs`、`hotkey.rs`、`monitor.rs`、`msaa_provider.rs`、`overlay.rs`、`providers.rs`、`refinement_worker.rs`、`renderer.rs`、`timed_call.rs`、`uia_provider.rs`、`window_detection.rs`、`mod.rs`，以及 `win/{bitblt,d2d,d3d11,wgc,window,mod}.rs`。
- 必须保持：overlay 仍是独立原生 HWND + 自己的消息循环线程；F5 热键仍注册在 overlay 线程；`present_us`/`over16ms` 不退化。
- 验收：§0.2 门禁全过 + 真机 F5/Esc/Enter + 窗口吸附 + 导出 PNG 正常。
- 回退：按文件 `git revert`。
- 风险：高（文件多，先搬 provider/worker 这类低耦合的，最后搬 overlay）。

### T1.6 拆 `overlay.rs`（第一优先，4 423 行）

- 前置：T1.5
- 动作（按职责拆，**先搬生产代码，再搬测试**；每拆一个文件跑一次门禁）：
  1. `overlay/window.rs`：窗口类注册、HWND、消息循环、焦点、生命周期。
  2. `overlay/input.rs`：鼠标/键盘/`WheelAccumulator`/`PointerGesture` 绑定。
  3. `overlay/state.rs`：会话绑定、预览/层级/提示/动画状态机（`WalkColour`、`ChainVisibility`、`RingAppear`、`AbandonedHint`）。
  4. `overlay/render_submit.rs`：渲染提交、damage/coalescing tick、present 计量。
  5. `overlay/window_restore.rs`：窗口恢复/取消/清理路径。
- 必须保持：滚轮/↑↓ 切层、链环与徽标、A2 挖洞、A3 绿跟滚轮、动画（docs/21 §5.24.10）、导出路径不带 UI。
- 验收：§0.2 门禁全过 + 真机一条完整链路（悬停 → 滚轮换层 → 确认 → PNG 导出）+ 汇总 `over16ms=0`。
- 回退：`git revert`（建议每个子文件一个提交）。
- 风险：高。**不要一次性搬完再测**：这个文件同时持 HWND、会话、输入、渲染和吸附，一次只动一块。

### T1.7 `uia_provider.rs` 测试搬家（3 338 行，测试 72%）

- 前置：T1.5
- 动作：
  1. 先把 `#[cfg(test)] mod tests`（2 416 行）按主题拆到 `windows/accessibility/tests/`（或 crate 的 `tests/`）下：浏览器夹具探针、Explorer 探针、sources 对照、超时/隔离用例。
  2. 再把生产代码（约 916 行）按 COM 初始化、UIA 查询、树缓存、元素身份校验、超时调用拆文件。
- 必须保持：**两个实机探针的可运行性与期望值**（浏览器 41/41、Explorer 12/25·65.8）；探针的启动方式（自带临时 profile 起浏览器）不变。
- 验收：§0.2 门禁全过（探针仍从同一命令跑）。
- 回退：按提交回退。
- 风险：中（搬测试最容易"顺手改断言"，禁止；只改路径与模块结构）。

### T1.8 拆 `d2d.rs`（3 805 行，测试 38%）

- 前置：T1.5
- 动作：拆为 `render/{device,frame_pass,mask_selection_pass,magnifier_pass,text}.rs`（按 docs/22 §7.2）；`render/d2d.rs` 保留设备资源与提交入口。
- 必须保持：调色板/遮罩 alpha 从 `ring_contrast` import（不复制）；GPU 像素回归测试（环对比度、标签像素签名、A2 洞、A3 颜色）逐条通过。
- 验收：§0.2 门禁全过（含 GPU 像素测试）。
- 回退：按提交回退。
- 风险：中。

### T1.9 依赖方向与接缝门禁落地

- 前置：T1.5
- 动作：
  1. 加一个可执行检查（脚本或测试）：`cargo tree -p snapclip-capture -e normal | Select-String -Pattern "tauri|wry|gpui"` 必须为空；`snapclip-capture` 不得依赖 `snapclip-history`。
  2. 把该检查写进 §0.2 通用门禁（成为每步都要跑的门禁）。
- 必须保持：现有 403 测试全绿。
- 验收：故意造一次违规（临时给 capture 加 tauri 依赖）→ 检查必须红；撤销后绿。
- 回退：删除脚本。
- 风险：低。

### T1.10 阶段验收 + tag

- 前置：T1.6–T1.9
- 动作：跑完整 §0.2 门禁 + 真机全链路 + 删掉所有过渡 re-export（T0.4/T1.3 留下的），打 tag `refactor-p1`。
- 验收：`src-tauri` 里 `capture/`、`platform/windows/capture/` 目录已空或只剩壳的引用；`rg -n "snapclip_model" src-tauri` 说明壳仍能编译。
- 风险：低。

---

## 6. P2：抽离 `snapclip-history`

> 关键前提：**artifact 的写盘权先收敛成一条**，再拆 crate。现在 `CaptureService` 通过 `ArtifactDir` 写、`store/blob.rs` 也在动磁盘，两边都写是最难查的一类 bug。

### T2.1 `ArtifactStore` / `ArtifactRef` 定死

- 前置：T1.10
- 动作：
  1. 在 `snapclip-model` 定义 `ArtifactRef { absolute_path, mime, dimensions, byte_len, content_fingerprint }`（指纹 = `blake3`，**写入时算好**，见 docs/22 §4）。
  2. 在 `snapclip-history` 定义 `ArtifactStore`：布局/命名/原子写/清理/LRU 的**唯一所有者**；`blob.rs` 的原子写与 `ArtifactDir` 的目录约定合并进来。
  3. 让截图导出改为"交付字节 + 元数据"，由壳调用 `ArtifactStore` 落盘；`ArtifactStore` 返回 `ArtifactRef`。
- 必须保持：导出 PNG 的像素与文件名行为（`docs/11 §8.2` 的导出契约）；历史里已有 artifact 仍能被读取（迁移期允许一次性重写索引，但要在提交消息里说明）。
- 验收：§0.2 门禁全过 + 真机截图 → 导出 → 历史可见 → 磁盘文件可打开。
- 回退：`git revert`。
- 风险：中（写盘路径切换是数据面风险，先加一条"导出后立即校验字节数/指纹"的测试）。

### T2.2 `snapclip-history` 骨架

- 前置：T2.1
- 动作：新增 crate（依赖 `snapclip-model` + SQLite/图像编码 + Windows SDK），先放空模块与 `ArtifactStore`。
- 验收：`cargo check --workspace --all-targets` 0 warning。
- 风险：低。

### T2.3 拆 `store/mod.rs`（1 595 行）

- 前置：T2.2
- 动作：拆为 `db/connection.rs`、`db/migration.rs`、`clip_repository.rs`、`artifact_repository.rs`、`recognition_repository.rs`；`Store` 保留为组合门面或直接消失（由调用方持有仓库）。
- 必须保持：迁移顺序与 schema 版本；现有 `migration_upgrades_existing_v1_database` 等测试逐条通过；分页/去重语义不变。
- 验收：§0.2 门禁全过；用一份已有数据库文件跑一次真实读取（复制一份到测试临时目录，不要销毁用户数据）。
- 回退：`git revert`。
- 风险：高（`migration_upgrades_existing_v1_database` 是这条路径的护栏，先读它再动）。

### T2.4 迁移剪贴板 Windows 适配（6 文件 / 1 210 行）

- 前置：T2.3
- 动作：迁 `platform/windows/clipboard/{formats,image_norm,listener,reader,source_app,mod}.rs` 到 `snapclip-history/src/windows/`。
- 必须保持：文本/HTML/图片格式读取、延迟渲染格式、来源程序识别行为不变。
- 验收：§0.2 门禁全过 + 真机复制文本/图片各一次，历史里正确入库。
- 风险：中。

### T2.5 迁 `application/clipboard_ingest.rs`

- 前置：T2.4
- 动作：
  1. 迁为 `snapclip-history/src/{service,reader,history}.rs`；按 docs/22 §7.2 拆"事件接收 / 去重窗口 / 格式读取 / publication / 识别入队"。
  2. **保留 `OcrQueue` 端口**（现在 `application/clipboard_ingest::OcrQueue` 由 `app/ocr_queue.rs` 实现，这是已经存在的正确模式）：端口留 history，实现由壳绑定到 `snapclip-recognize`。
- 必须保持：去重窗口时长、publication 事件字段、`clipboard-updated-v1` 契约不变。
- 验收：§0.2 门禁全过 + 真机连续复制去重行为与之前一致。
- 风险：中。

### T2.6 `ClipboardService` / `HistoryService` 公共 API

- 前置：T2.5
- 动作：定义接缝（查询分页、按 id 取详情、复制回写、删除、订阅低频事件）；公共类型用 builder + reader，**不暴露 pub 字段**；内部 repository 类型不 re-export。
- 必须保持：`commands/*` 里现有行为逐条对齐（改命令前先把旧行为列成清单）。
- 验收：§0.2 门禁全过。
- 风险：中。

### T2.7 `commands/history.rs`、`commands/ocr.rs` 改走服务

- 前置：T2.6
- 动作：把直接 `State<'_, Store>` 改成调用 `HistoryService`/`ClipboardService`（识别相关的入队/重试/状态查询改走 recognize 的服务或 `OcrQueue` 端口）。`commands/capture.rs` 已经走 `CaptureRuntime`，不动。
- 必须保持：前端命令名/返回 JSON 结构不变（前端契约）。
- 验收：§0.2 门禁全过 + 前端历史页/OCR 操作各点一次无报错。
- 回退：`git revert`。
- 风险：低。

### T2.8 阶段验收 + tag

- 前置：T2.7
- 动作：完整 §0.2 门禁 + 真机"复制 → 历史 → 复制回写 → 删除"全链路 + 回退演练（§0.6）+ tag `refactor-p2`。
- 风险：低。

---

## 7. P3：抽离 `snapclip-recognize`

### T3.1 crate 骨架（迁 `ocr/` 829 行 / 6 文件）

- 前置：T2.8
- 动作：新增 `crates/snapclip-recognize`，迁 `engine.rs`（`OcrEngine`/`OcrCancel`，**保留契约**）、`manager.rs`、`worker.rs`、`win_ocr.rs`、`rapid.rs`（`ocr-rapid` feature 默认关）、`mod.rs`；输入类型改为接受 `ArtifactRef`（不再接受 `Arc<[u8]>`，或同时保留内存入口给内部使用，但**接缝**用 `ArtifactRef`）。
- 必须保持：引擎选择策略（rapid 可用则用，否则 Windows OCR）、`OcrError` 变体语义、取消语义。
- 验收：§0.2 门禁全过。
- 风险：低。

### T3.2 生命周期：惰性 / 取消 / 超时 / 缓存 / 熔断

- 前置：T3.1
- 动作：
  1. 惰性（P0.5 已做基础版）：保持"首个任务才启动"，并把它做成幂等（并发首个任务只启动一次 worker）。
  2. 超时：`OcrError::Timeout` 现在是 `#[allow(dead_code)]`，接上真实超时与 `OcrCancel`。
  3. 缓存：键 = `engine_id + engine_version + input_blake3 + options + model_version`，值写磁盘、元数据进 `recognition_repository`；`input_blake3` 直接取 `ArtifactRef.content_fingerprint`（不重读文件）。
  4. 熔断：连续失败 N 次后进入冷却并上报状态（避免 UI 重试风暴）；空闲 60 s 释放 worker/模型。
- 必须保持：OCR 结果、重试、历史关联；未启用时零 worker。
- 验收：§0.2 门禁全过 + 单测覆盖"排队/取消/超时/并发上限/缓存命中/熔断"五类；真机 OCR 一次成功、一次取消。
- 风险：中。

### T3.3 壳接线（history 只发 `ArtifactRef`）

- 前置：T3.2
- 动作：`snapclip-history` 只发布 `ArtifactRef` 与低频事件；由 `app`（或未来的 GPUI 壳适配层）实现 `OcrQueue` 并提交给 `snapclip-recognize`。识别结果写库经 `recognize → history` 的 service 调用（方向单向，不允许 recognize 依赖 history）。
- 必须保持：`ocr-status-v1` 事件契约；历史里 OCR 文本/状态字段不变。
- 验收：§0.2 门禁全过 + 真机截图含文字 → 历史里出现文本。
- 风险：中。

### T3.4 资源对比 + 阶段验收 + tag

- 前置：T3.3
- 动作：重测 §1.2（此时 `after` 应稳定好于 `before`）+ 回退演练 + tag `refactor-p3`。
- 风险：低。

---

## 8. P4：GPUI 壳（**独立立项**）

> 纪律：这一阶段与 P1–P3 分开提交、分开验收。换壳会让"设置页/历史/标注工具条"出现真实功能回退，**不能和 crate 拆分混在一批**，否则回归无法归因。

### T4.1 开工前研读（不许跳）

- 动作：读 `gpui-kit` 的 SKILL 与 **Coding Guides**（分层、`RenderOnce` vs `Entity<T>`、状态归属、`ElementId`、事件/焦点、异步、公共 API、测试分层），设计可见界面时读 **Design Guides**；查组件用 `https://gpui-kit.com/llms.txt` + `component/{name}.md`。
- 必须记住的四条硬约束：应用**只依赖 `gpui-kit`**；`gpui_kit::init(cx)` 在建组件视图前调用一次；每个窗口第一层是 `Root`；**绝不凭记忆写 API**（先查签名）。
- 验收：把本阶段要用的组件（`List`/`VirtualList`、`Input`、`Button`、`Settings`、`WindowExt` 覆盖层）逐个查过文档并在提交消息里列出确认过的路径。

### T4.2 `apps/snapclip` 骨架

- 前置：T4.1、T3.4
- 动作：`gpui_kit::application().with_assets(...).run(|cx| { gpui_kit::init(cx); … })` + `open_window` + `Root`；窗口标题/尺寸/DPI 行为对齐现有 Tauri 主窗口；**不接**截图 overlay（仍是 capture crate 的原生 HWND）。
- 验收：能打开一个空壳窗口；§0.2 门禁不受影响；截图 overlay 仍可独立 F5 起来。
- 风险：中。

### T4.3 history 能力（GPUI 侧）

- 前置：T4.2
- 动作：`apps/snapclip/src/history/{model,history_view,commands}.rs`；`Entity<HistoryState>` 持有列表/搜索/选中；列表用 `List`/`VirtualList` 虚拟化；**`ElementId` 用 clip id**；缩略图懒加载。
- 必须保持：与现有前端一致的行为（分页、去重展示、复制回写、删除）。
- 验收：`#[gpui_kit::test]` + `VisualTestContext` 覆盖：加载、搜索过滤、键盘上下移动、回车复制、删除确认。
- 风险：高（第一个真正的 GPUI 功能）。

### T4.4 settings 能力

- 前置：T4.3
- 动作：`apps/snapclip/src/settings/`；先落现有设置项（如剪贴板历史开关、OCR 开关），并把 docs/21 §8 待办里的 `deep_select_text_runs`/`deep_select_visible_wrappers` 及动画参数一起接成真实设置通道（这一步同时消灭那条挂了两轮的待办）。
- 验收：改设置 → 立即生效（热更新到 overlay / recognize），重启后保持。
- 风险：中。

### T4.5 事件桥

- 前置：T4.3
- 动作：`apps/snapclip/src/adapters.rs` 用 channel 接 capture/history/recognize 的低频事件；用 `cx.spawn`/`background_spawn` 做 I/O，**只在 `Entity::update` 里改状态**；复用现有 `EventEnvelope` 的 `schemaVersion` + `generation` 语义丢弃过期事件（不要发明第二套 revision）。
- 必须保持：高频路径零 IPC（F5 → overlay 不经过 GPUI）。
- 验收：事件到达顺序/丢弃语义有单测；真机 F5 → 历史自动刷新。
- 风险：高。

### T4.6 托盘（Win32）

- 前置：T4.2
- 动作：`apps/snapclip/src/tray.rs`，沿用现有 `icon.rs` 的 Win32 实现；菜单项（显示/隐藏/退出/截图）向壳发低频事件。
- 验收：托盘菜单全部可用；退出干净（无残留进程/线程）。
- 风险：中。

### T4.7 测试三层

- 前置：T4.3–T4.6
- 动作：纯函数 → `#[gpui_kit::test]` → `VisualTestContext`（焦点/键盘/指针/布局）→ 真实窗口按**无障碍树**断言（role/label/value/enabled/focus）。
- 必须保持：截图 overlay 的 UIA/MSAA 探针仍是同一套门禁。
- 验收：至少覆盖"历史列表键盘操作 + 设置热更新 + 托盘退出"三条端到端。
- 风险：中。

### T4.8 性能对比

- 前置：T4.7
- 动作：对比 §1.2 基线：overlay 出现延迟、输入延迟、截图延迟、`present_us`/`over16ms`、空闲 CPU/内存；确认高频路径没有引入 IPC/序列化。
- 验收：无不可接受的退化（有退化必须给出原因与补救，否则不进入 T4.9）。
- 风险：中。

### T4.9 阶段验收 + tag

- 前置：T4.8
- 动作：回退演练 + tag `refactor-p4`；此时 **Tauri 壳仍在**（`apps/snapclip-tauri` 或原 `src-tauri`），两个壳只共享能力 crate。
- 风险：低。

---

## 9. P5：进程插件边界（**触发式，不满足判据就不执行**）

判据（docs/22 §5.3）：① 出现第二个真实实现且资源模型明显不同（例如常驻数百 MB 的 ONNX 进程）；或 ② profiling/崩溃数据证明必须进程隔离。**两个都不满足就停在这里**——继续按需进程内运行。

满足后按序做：

1. `snapclip-plugin-api`：能力枚举、输入（`ArtifactRef`）、输出摘要、协议版本、最大消息大小、超时与取消。
2. `snapclip-plugin-host`：registry（manifest/启用状态/版本检查）、scheduler（队列/并发/优先级/取消）、lifecycle（lazy start / idle stop / 熔断）、cache、named pipe 传输、`builtins/`。
3. 外部进程 worker：崩溃重启次数上限、权限边界、大图仍走 artifact 路径。
4. 验收：未启用时零额外 worker/模型；启用后可取消、可重试、结果可持久化；host 崩溃不影响主进程。

---

## 10. P6：删除 Tauri 与旧目录

- 前置：T4.9（GPUI 壳完成托盘、隐藏/显示、焦点、退出、DPI、多显示器回归）
- 清单：
  1. 删 `src-tauri/src/commands/*`、Tauri `events` 适配、`app/`（组合根）与 `icon.rs` 的旧实现。
  2. 删前端（Vue adapter、`src/shared/contracts.ts` 与 package.json 的 Tauri 相关脚本）与 `src-tauri/` 目录本身。
  3. 从 workspace 移除旧成员，跑 `cargo tree -i tauri`、`cargo tree -i wry` 确认为空；全仓库 `rg -n "tauri"` 只应命中文档。
  4. 全量依赖检查 + `cargo check --workspace --all-targets` + §0.2 门禁 + 真机全链路（截图 → 历史 → OCR → 设置 → 托盘退出）。
- 验收：以上全部通过；打 tag `refactor-p6`（= 重构完成点）。
- 回退：`git reset --hard refactor-p4`（保留 GPUI 壳与能力 crate）。
- 风险：中（删除面大，但此时所有功能已在 GPUI 壳上跑通）。

---

## 11. 风险与已知坑

| # | 坑 | 处理 |
| --- | --- | --- |
| 1 | 探针假红（环境相关） | §0.3：串行 + 复跑 2 次；假红与真红都要写进提交消息 |
| 2 | 字体子集门禁（改 UI 文案/新图标） | 跑 `subfont/subset.ps1`；新增中文字串必须先加进 `overlay_drawn_strings()` |
| 3 | 双壳冲突（Tauri 与 GPUI 共用一个 ui crate） | 两个独立 app，只共享能力 crate（docs/22 §3） |
| 4 | artifact 写盘权分裂 | T2.1 先收敛成 `ArtifactStore` 一处 |
| 5 | 共享 crate 变胖 | `snapclip-model` 只放稳定值对象/事件摘要；内部类型不 re-export（T1.2 + §10.1 门禁） |
| 6 | 测试搬迁窗口期 | 搬测试只改路径不改断言；这期间不加新功能 |
| 7 | 增量编译时间恶化 | §1.2 记录"改 model 一行"的耗时；明显恶化再决定拆 crate 粒度 |
| 8 | `overlay.rs` 是唯一真单点 | T1.6：一次只搬一块 + 每块跑门禁 |
| 9 | 用户手稿（`prototypes/`）与无关改动 | §0.6：先确认归属，不顺手提交/丢弃 |
| 10 | `reset --hard` 丢工作 | §0.6：先建 `backup/*` 分支或 tag；`reflog` 只是保险不是备份 |

---

## 12. 完成判据（Definition of Done）

### 12.1 单个任务

- [ ] `git status --porcelain` 干净开始，任务 = 一个提交，消息含任务号与门禁数字。
- [ ] §0.2 门禁全过（含两个实机探针），数字与基线一致或变化有解释。
- [ ] 任务条目里的"必须保持"逐条核对过。
- [ ] 回退点明确（上一个 tag 或本任务前一个提交）。

### 12.2 单个阶段

- [ ] 阶段内全部任务勾选。
- [ ] 阶段 tag 已推送，且**回退演练**通过（tag 处 `cargo test --lib` + `cargo check --all-targets` 绿）。
- [ ] 真机全链路（截图 → 历史 → OCR → 设置）人工走查一遍。
- [ ] 资源/性能指标与本阶段相关的那几列已更新。

### 12.3 重构目标（全部完成时可用工具验证）

- [ ] `cargo tree`：`snapclip-model`/`snapclip-capture`/`snapclip-history` 不含 `tauri`/`wry`/`gpui-kit`；`snapclip-capture` 不依赖 `snapclip-history`。
- [ ] 截图高频路径零 IPC：F5 → overlay 不经过壳；`over16ms=0` 与基线同量级。
- [ ] 识别能力未启用时零额外 worker/模型（资源表可证）。
- [ ] `cargo tree -i tauri` 与 `cargo tree -i wry` 为空，旧目录删除。
- [ ] 门禁全绿：单元测试（≥ 现 403 + 新增）、两个实机探针、字体子集门禁、事件契约测试。
- [ ] `docs/21`/`docs/22`/本文状态更新，并把"哪些结论是实测、哪些仍是设计"写清。
