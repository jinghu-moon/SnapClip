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
- [ ] 勾选框只表示"我准备做/我已做完动作"；**完成状态必须用 §13 的状态词表 + 执行记录**，不能仅凭勾选框声称完成。
- [ ] 每个任务独立提交、独立推送；提交消息里带上门禁数字（现有习惯）。
- [ ] 不得通过删除测试、放宽断言、跳过 `#[ignore]` 探针、关闭功能来制造通过。
- [ ] 任务里的"必须保持"条目是回归清单：任何一条被破坏，即使测试是绿的，也视为失败。
- [ ] 发现问题先定根因（实现 / 接口 / 数据结构 / 模块边界 / 调用流程 / 抽象），再决定改哪里；不要用临时分支绕过。

### 0.2 阶段门禁矩阵（**P1 之后 `src-tauri` 不再是全部门禁的所在地**）

各阶段用的命令不同，必须按阶段选。**每个任务跑该阶段"受影响"的快门禁；每个阶段结束跑完整门禁。**

| 阶段 | 门禁组 | 命令（在仓库根执行，除非另注） |
| --- | --- | --- |
| G0 · 旧 Tauri 阶段（P0–P0.5，以及 P1 迁移期间） | 完整 | `cargo test --lib --manifest-path src-tauri/Cargo.toml`<br>`cargo check --all-targets --manifest-path src-tauri/Cargo.toml`（**T0.3 之前没有 workspace 根**，此时 `--workspace` 会直接报 "could not find Cargo.toml"；T0.3 之后改用 `cargo check --workspace --all-targets`）<br>探针（见下，同样带 `--manifest-path`） |
| G1 · workspace / capture / history / recognize 阶段（P1 之后） | 完整 | `cargo test --workspace --all-targets`<br>`cargo check --workspace --all-targets`<br>探针改为按包跑：`cargo test -p snapclip-capture --lib browser_element_probe -- --ignored --nocapture`（Explorer 同理） |
| G2 · GPUI 阶段（P4 之后） | 完整 | `cargo test --workspace --all-targets`<br>`cargo test -p snapclip`（壳的 `#[gpui_kit::test]` / `VisualTestContext`）<br>G1 的探针仍然要跑 |

探针命令（**必须串行**，之间停 3 秒，避免互相抢焦点）：

```powershell
# G0 阶段
cargo test --lib browser_element_probe -- --ignored --nocapture; Start-Sleep -Seconds 3
cargo test --lib explorer_rule_probe   -- --ignored --nocapture
cargo test --lib ring_contrast_probe   -- --ignored --nocapture   # A4 底色表，对照 docs/21 §5.26
# G1/G2 阶段：把 `--lib` 换成 `-p snapclip-capture --lib`（探针随 capture crate 迁移）
```

真机链路（每个阶段至少一次）：现有启动方式 → F5 → 悬停/滚轮/确认或 Esc → 看会话汇总的 `present_us` / `over16ms`。

改了 UI 文案或新增中文字串时追加：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File subfont/subset.ps1
cargo test -p snapclip-capture --lib the_embedded_subset_covers_the_strings_the_overlay_draws
```

### 0.3 环境纪律（探针）

- 探针需要真实窗口（浏览器由探针自己拉起；Explorer 需要本机有一个可见的资源管理器窗口）。
- 红了**先排除环境**（窗口不存在、被遮挡、机器高负载），复跑 2 次；复跑后仍稳定红才当回归。
- 已实测过一次假红：Explorer `provider_hit_available=10/25` 而几何逐位不变，复跑回到 25/25。**不许把这类假红当噪声忽略，也不许把它当回归阻塞**：复跑结果写进提交消息。
- 探针**不得**因为"跨 crate 了"而失效；哪个探针需要 Tauri 才能跑，说明拆分没做完。

### 0.4 提交、tag 与回退

- **任务完成**：本地一个提交（消息含任务号与门禁数字）。推送节奏 = **至少每阶段一次，推荐每任务都推**——远端就是备份。
- **阶段完成**：合并（高风险阶段用 `--no-ff`）→ 复跑完整门禁 → annotated tag（`refactor-p0`、`refactor-p05`、`refactor-p1`…）→ 推送 tag。
- **回退默认用 `git revert <sha>`**。`git reset --hard` **不是常规回退手段**：只在用户明确批准、并且已经先建 `backup/*` 分支或 tag 的情况下使用（见 §0.6）。
- 最终回退基准：`smart-snapping-v1-2026-10-07`（V1 功能封版，已推远端）。

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
- 开工前先探明环境，**不要假设分支名和远端名**：
  ```powershell
  git branch --show-current; git remote -v; git status --porcelain; git log --oneline -3
  ```
  下文用 `<默认分支>` / `<远端>` 指代实际值。
- 任务开始前工作区必须**干净**（`git status --porcelain` 为空）。**有未知改动时停止并报告**——不自动 `git stash`、不自动提交、不丢弃：`prototypes/` 下有用户手稿，动了就是事故。确认归属后由用户决定怎么处置。
- **禁止把红的树留在 main 上**。门禁没过就不要提交；确实需要中途保存进度时，提交为显式的 `wip(T1.6): …（门禁红，未完成）` 并留在**分支**上，不要推到 main。

**分支与 tag：回退点必须存在且可达**

- 高风险阶段开分支：`git switch -c refactor/p1-capture`；阶段验收通过后 `git switch main && git merge --no-ff refactor/p1-capture`（历史里留下一个阶段合并点，回退粒度 = 一个 merge）。分支期间 **main 始终保持可用**。
- 每个阶段结束打 **annotated tag** 并推送：`refactor-p0`、`refactor-p05`、`refactor-p1`、`refactor-p2`、`refactor-p3`、`refactor-p4`、`refactor-p6`。`git tag -a refactor-p1 -m "…" && git push origin refactor-p1`。
  **tag 不推远端等于没打**：本地仓库损坏时远端就是唯一回退点。
- 任务条目里的"回退"统一指向**具体对象**（上一个提交 sha 或上一个 tag 名），不要写"回退一下"。

**场景 → 命令（照抄）**

| 场景 | 命令 | 说明 |
| --- | --- | --- |
| 任务做到一半发现方向不对，还没提交 | 先 `git branch backup/T1.6-wip` 再 `git switch <默认分支>`；需要留进度才 `git stash push -m "T1.6 wip"` | 先建可找回的分支（stash 只是补充）；`git stash list` 能找回 |
| 想在动手前留一份保险 | `git branch backup/pre-T1.6` | 比 reflog 好找；确认成功后 `git branch -D` |
| 已提交但发现坏了，历史已推送 | `git revert <sha>` | **优先用 revert**，保留可追溯性 |
| 已提交但坏得彻底，且**用户明确批准**回退 | 先 `git branch backup/pre-reset`，再 `git reset --hard refactor-p1` + `git push --force-with-lease <远端> <默认分支>` | 只有用户批准才允许；**必须 `--force-with-lease`**，不要裸 `--force` |
| 误删/误 reset，找不到提交 | `git reflog`（默认保留 90 天）→ `git branch rescue/<sha> <sha>` | reflog 是最后一道保险，但它不是备份 |
| 需要把 tag 指到新提交 | `git tag -d <tag> && git tag -a <tag> -m …` + `git push --force-with-lease <远端> <tag>` | 本项目做过一次（V1 tag 挪到收尾提交）；先确认没有别人依赖它 |
| 只想看某个文件的旧版本 | `git show refactor-p1:src-tauri/src/.../overlay.rs` | 不用切换工作区 |

**阶段收尾必做一次"回退演练"**（这是本节存在的意义）：

```powershell
git switch --detach refactor-p1
cargo test --workspace --all-targets        # 回退点必须自身可编译、可测（G0 阶段用 src-tauri 的那条）
git switch <默认分支>
```

回退点编译不过 = 这个阶段的 tag 是假的，必须修好再往前走。

**禁止事项**

- 不用 `git checkout -- <file>`、`git clean -fd` 抹掉不确定归属的改动（本项目 `prototypes/` 下有用户手稿）。
- 不在没有备份分支/tag 的情况下执行 `git reset --hard`。
- 不为了让门禁变绿而改历史（例如把失败提交 reset 掉再重写）：`revert` 优先，历史留痕。

### 0.7 并行执行规则（多人/多 agent 时）

文档允许并行，但并行只在满足下面全部条件时才允许：

- 每个并行任务使用**独立 worktree**（`git worktree add ../.zcf/SnapClip/<任务号> -b refactor/<任务号>`），不共用同一个工作区。
- **不同时修改同一个模块入口**：`mod.rs`、`Cargo.toml`、`snapclip-model` 里的公共类型、`lib.rs` 的导出表，同一时间只能有一个任务在改。
- 每个 worktree 在合并前各自跑**完整**门禁（按 §0.2 的阶段矩阵）。
- 主分支只接收"已验证 + 已跑完整门禁"的合并提交；合并后主分支再跑一次完整门禁。

首批可并行的候选（互不重叠）：**T1.7**（UIA 测试搬家）与 **T1.8**（D2D 拆分）——它们都在 P1 之后、且不碰同一入口。除此之外默认串行。

---

## 1. 基线（动手前记录；每个阶段对照）

### 1.1 正确性基线（**当前 HEAD 实测，命令生成**）

> 规则：本表的数字必须由下面的命令在**当前 HEAD** 现场产生，不手写、不从 tag 或旧文档抄。
> **历史参考**：V1 封版 tag `smart-snapping-v1-2026-10-07` 当时是 `398 passed / 5 ignored`；当前 HEAD 是 `403 / 6`（ring_contrast 那 5 个测试 + 1 个探针是 tag 之后加的）。两者不要混用。
> **已知陷阱**：`docs/21 §10` 里有一行 `373 passed / 2 ignored`，那是 V1 之前的旧基线，**已经过期**；引用它会导致 T0.1 一开始就误判"基线不对"。

| 门禁 | 生成命令 | 当前 HEAD 实测（2026-10-07） | P0 · T0.1 复跑（2026-10-07，HEAD 0bd9191） |
| --- | --- | --- | --- |
| 单元测试 | `cargo test --lib --manifest-path src-tauri/Cargo.toml` | **403 passed / 0 failed / 6 ignored** | [x] `403 passed / 0 failed / 6 ignored`（一致） |
| 静态检查 | `cargo check --workspace --all-targets` | 0 warnings | [x] 0 warnings（一致） |
| 浏览器探针 | `cargo test --lib browser_element_probe -- --ignored --nocapture` | `asserted=41 passed=41 failed=0` / `available=52` / `finer=0` | [x] `asserted=41 passed=41 failed=0` / `available=52` / `finer=0`（一致） |
| Explorer 探针 | `cargo test --lib explorer_rule_probe -- --ignored --nocapture` | `control_level_points=12/25` / `median_area_pct=65.8` / `available=25/25` / `finer=0` | [x] `control_level_points=12/25` / `median_area_pct=65.8` / `available=25/25` / `finer=0`（一致，本机有可见 Explorer 窗口） |
| A4 底色表 | `cargo test --lib ring_contrast_probe -- --ignored --nocapture` | 与 docs/21 §5.26 表格一致 | [x] passed，底色表与 docs/21 §5.26 一致 |
| 真机会话汇总 | 真机 F5 → 看日志 | `over16ms=0`；`present_us` 量级 500–2000 µs | [ ] **待人工**：agent 无法在你的机器上按 F5 观测 overlay，需你本地跑一次并把 `present_us`/`over16ms` 反馈回来 |
| 字体子集门禁 | `cargo test --lib the_embedded_subset_covers_the_strings_the_overlay_draws` | 通过（子集 20.3 KB） | [x] 通过（随 403 一起跑绿） |
| 事件契约 | `cargo test --lib` 里的 `ALL_EVENT_NAMES` 契约测试 | 通过（与 `src/shared/contracts.ts` 同步） | [x] 通过（随 403 一起跑绿） |

> 迁移到 G1 之后，探针与字体门禁按 §0.2 换成 `-p snapclip-capture` 形式，**数字预期不变**；变了就是回归。

### 1.2 资源基线（P0 新测，before / after 两栏）

| 指标 | 怎么测 | before（T0.2 实测，2026-10-07，HEAD 0bd9191，debug 构建） | after（P0.5 之后） |
| --- | --- | --- | --- |
| 进程数 / 线程数 | 任务管理器（详细信息），或下面的 PowerShell（表格里不能直接写竖线，命令放在表下方） | **1 进程 / 18 线程**（`snapclip.exe`，debug） | |
| WebView2 附加进程 | 同一时刻的 `msedgewebview2` 进程数，**减去应用关闭时的对照值** | **+6 进程**（运行 21 / 空闲 15） | |
| 常驻内存（应用自身） | 上面的 PowerShell 的 `WorkingSet64` / `PrivateMemorySize64` | **37.7 MB WS / 6.8 MB 私有** | |
| 常驻内存（含 WebView2） | 同上的 WebView2 差值 | **+342.2 MB WS**（运行 843.3 / 空闲 501.1） | |
| GPU 内存 | 任务管理器"GPU 内存"列 | **未测**（需按 pid 归因的 GPU 计数器，本轮未采集） | |
| 空闲 CPU（30 s 平均） | 应用启动静置后用 `TotalProcessorTime` 差值 ÷ 30 s ÷ 逻辑核数（20 核） | **~0.00 %**（30 s 内无可见增长，分辨率不足，只作量级参考） | |
| 冷启动到可交互 | 启动日志 `setup begin` → `clipboard pipeline ready` 的 elapsed_ms | **68 ms**（`capture overlay ready` 58 ms；debug 构建） | |
| 包体积 | 产物目录大小 + 主 exe 大小 | **exe 19.34 MB**（debug）；`dist/` 131.5 KB / 5 文件 | |
| 改一行 → 增量检查 | T0.3 之前没有共享 crate：改 `src-tauri/src/lib.rs`（只动 mtime）后 `cargo check --manifest-path src-tauri/Cargo.toml` 计时。T0.3 之后改为"改 `snapclip-model` 一行后 `cargo check --workspace`" | **1.36 s**（只重编 `snapclip` 一个 crate） | |

> **测量口径（可复现）**：`Start-Process src-tauri\target\debug\snapclip.exe -RedirectStandardOutput/-RedirectStandardError` + `-WindowStyle Hidden`，静置 10 s 后取进程快照，再采样 30 s CPU，最后 `Stop-Process` 并等 8 s 取对照。**`webview page_load Finished` 不能当"可交互"**：直接跑 debug exe 时前端走 `devUrl`（`http://localhost:1420`），没有 dev server 会反复重载（日志里会出现 500/1478/6488/36500 ms 多条 "Finished"），所以冷启动口径改用 `clipboard pipeline ready`（那之后 F5 已经可用）。

进程/线程/内存的取数命令（在仓库根跑）：

```powershell
Get-Process | Where-Object { $_.ProcessName -like '*snapclip*' } | Select-Object Name, Id, @{n='Threads';e={$_.Threads.Count}}, WorkingSet64
```

> **必须 before 先测**：P0.5 的 OCR 惰性化唯一能量化的收益就是这张表的两栏之差；但**收益是实测结果，不是预期结论**（见 T0.5.3/T0.5.4）。

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

生成命令（**数字由命令产生，不手写**；下表是 2026-10-07 的实测值，若与现场不符以命令输出为准）：

```powershell
# 单文件行数
Get-Content src-tauri/src/<路径> | Measure-Object -Line
# 目录总行数 + 文件数
$f = Get-ChildItem src-tauri/src/<目录> -File; ($f | ForEach-Object { Get-Content $_.FullName } | Measure-Object -Line).Lines; $f.Count
```

对报告里出现过的三个数字做一次校正（都以本命令为准）：`infrastructure/store/mod.rs` = **1595** 行、`application/clipboard_ingest.rs` = **694** 行、`platform/windows/clipboard/` = **6** 个文件（含 `mod.rs`）。

---

## 2. 任务总览（进度表）

| 编号 | 任务 | 依赖 | 规模 | 风险 | 状态 |
| --- | --- | --- | --- | --- | --- |
| T0.1 | 复跑并记录正确性基线 | — | — | 低 | [x]（自动门禁全绿；真机 F5 待人工） |
| T0.2 | 记录资源基线（before） | T0.1 | — | 低 | [ ] |
| T0.3 | 建 workspace 骨架（+ 修 `.gitignore`） | T0.1 | 4 文件 + lock | 中（Tauri 构建） | [x]（自动门禁全绿；`npm run tauri dev` + F5 待人工） |
| T0.4 | 建 `snapclip-model` 骨架（+ 搬 `Rect`/`Point`/`ImageDimensions`） | T0.3 | 10 文件 | 低 | [x]（自动门禁全绿） |
| T0.5.1 | 删除 `capture/platform` 转发层（+ 顺带清掉它续命的死代码） | T0.4 | 2 文件 + 1 处死代码链 | 低 | [x]（自动门禁全绿） |
| T0.5.2 | OCR 事件出口改框架无关 trait | T0.5.1 | 2 文件 | 中（IPC 契约） | [ ] |
| T0.5.3 | OCR 惰性启动 | T0.5.2 | 2 文件 | 中 | [ ] |
| T0.5.4 | 记录资源基线（after）并对比 | T0.5.3 | — | 低 | [ ] |
| T1.1 | `snapclip-capture` 骨架与依赖 | T0.5.4 | 2 文件 | 低 | [ ] |
| T1.2 | 定义 capture 公共接缝（端口 + 无 pub 字段审查） | T1.1 | 3 文件 | 中 | [ ] |
| T1.3 | 值对象迁入 `snapclip-model` | T1.2 | ~10 文件 | 中 | [ ] |
| T1.4 | 迁移平台无关 capture（20 文件） | T1.3 | 10 686 行 | 中 | [ ] |
| T1.5 | 迁移 Windows 实现（20 文件） | T1.4 | 18 256 行 | 高 | [ ] |
| T1.6.1 | 拆出 `overlay/window.rs` | T1.5 | — | 高 | [ ] |
| T1.6.2 | 拆出 `overlay/input.rs` | T1.6.1 | — | 高 | [ ] |
| T1.6.3 | 拆出 `overlay/state.rs` | T1.6.2 | — | 高 | [ ] |
| T1.6.4 | 拆出 `overlay/render_submit.rs` | T1.6.3 | — | 高 | [ ] |
| T1.6.5 | 拆出 `overlay/window_restore.rs` + 组收尾 | T1.6.4 | 4 423 行（整组） | 高 | [ ] |
| T1.7 | `uia_provider.rs` 测试搬家 | T1.5 | 3 338 行（测试 72%） | 中 | [ ] |
| T1.8 | 拆 `d2d.rs`（4 pass + 文本） | T1.5 | 3 805 行 | 中 | [ ] |
| T1.9 | 依赖方向与接缝门禁落地 | T1.5 | 1 脚本 | 低 | [ ] |
| T1.10 | 阶段验收 + tag `refactor-p1` | T1.6.1–T1.9 | — | 低 | [ ] |
| T2.1 | `ArtifactRef`/`CaptureOutput` 在 `snapclip-model` 定死（**不建 crate**） | T1.10 | 2 文件 | 低 | [ ] |
| T2.2 | `snapclip-history` 骨架 | T2.1 | 2 文件 | 低 | [ ] |
| T2.3 | 两个独立存储：`CaptureArtifactStore` + `ClipboardBlobStore` | T2.2 | 2 文件 | 中 | [ ] |
| T2.4 | 切换 capture 导出链（搬 PNG 编码，改 `finish_artifact`） | T2.3 | ~4 文件 | 中 | [ ] |
| T2.5 | 拆 `store/mod.rs`（连接/仓库/迁移） | T2.4 | 1 595 行 | 高 | [ ] |
| T2.6 | 迁移剪贴板 Windows 适配（6 文件） | T2.5 | 1 210 行 | 中 | [ ] |
| T2.7 | 迁移 `clipboard_ingest`（去重/格式/publication） | T2.6 | 694 行 | 中 | [ ] |
| T2.8 | `ClipboardService`/`HistoryService` 公共 API | T2.7 | 3 文件 | 中 | [ ] |
| T2.9 | `commands/history.rs`、`commands/ocr.rs` 改走服务 | T2.8 | 2 文件 | 低 | [ ] |
| T2.10 | 阶段验收 + tag `refactor-p2` | T2.9 | — | 低 | [ ] |
| T3.0 | recognize 接缝设计（`ArtifactReader`/`RecognitionJobStore`/`RecognitionEventSink`） | T2.10 | 1 文件 | 中 | [ ] |
| T3.1 | `snapclip-recognize` 骨架（迁 `ocr/`） | T3.0 | 829 行 | 低 | [ ] |
| T3.2 | 惰性 + 取消 + 超时 + 缓存 + 熔断 | T3.1 | ~4 文件 | 中 | [ ] |
| T3.3 | 壳接线（history 只发 `ArtifactRef`，结果由壳写回） | T3.2 | 2 文件 | 中 | [ ] |
| T3.4 | 资源对比 + 阶段验收 + tag `refactor-p3` | T3.3 | — | 低 | [ ] |
| T4.1 + T4.1.1 | 开工前研读 + **前端功能迁移矩阵** | — | 1 表格 | 低 | [ ] |
| T4.2 | `apps/snapclip` 骨架（init/Root/单窗口） | T4.1, T3.4 | ~5 文件 | 中 | [ ] |
| T4.3 | history 能力（Entity + 虚拟列表 + `ElementId`=clip id） | T4.2 | ~4 文件 | 高 | [ ] |
| T4.4 | settings 能力（**新增**，不是迁移） | T4.3 | ~3 文件 | 中 | [ ] |
| T4.4.1 | 两个窗口检测开关接成真实设置通道 | T4.4 | 3 文件 | 中 | [ ] |
| T4.5 | 事件桥（`snapclip-model::AppEvent` + channel + 丢弃过期） | T4.3 | 2 文件 | 高 | [ ] |
| T4.6 | 托盘（Win32，**新建**） | T4.2 | 1 文件 | 中 | [ ] |
| T4.7 | 测试三层 + 无障碍树断言（**先 spike**） | T4.3–T4.6 | ~4 文件 | 中 | [ ] |
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
- **开工前先修一个会静默毁掉整个重构的坑（T0.2 实测发现）**：`.gitignore` 里有一条 `crates/`，把整个 `crates/` 目录都忽略了。而 `crates/rapid-ocr-rs` 是**带自己 `.git` 的外部仓库**（`src-tauri/Cargo.toml` 用 `path = "../crates/rapid-ocr-rs"` 依赖它）。后果：T0.4/T1.1 之后新建的 `crates/snapclip-model`、`crates/snapclip-capture`… 全部**不会被 git 跟踪**——本地编译一切正常、提交里什么都没有、别人克隆下来直接崩。必须在 T0.3 一起改掉：
  - `.gitignore`：`crates/` → `/crates/rapid-ocr-rs/`（只忽略外部仓库，自己的 crate 要跟踪）。
  - 生成后立刻验证：`git check-ignore -v crates/snapclip-model/Cargo.toml` **必须无输出**（有输出就是还被忽略）。
- 动作：
  1. 根目录新增虚拟 workspace `Cargo.toml`：`[workspace]`、`resolver = "2"`、**`members = ["src-tauri"]`（逐个显式列，每建一个 crate 加一行）**、**`exclude = ["crates/rapid-ocr-rs"]`**；同时把 `src-tauri/Cargo.toml` 的 `[profile.release]` 整体搬到根上（成员里的 profile 会被 cargo 忽略并告警，语义不变）。
     - **不要用 `crates/*` 通配**：那会把外部仓库 `crates/rapid-ocr-rs` 卷进我们的 workspace（它有自己的 `.git`/`Cargo.toml`/`Cargo.lock`，会报 "believes it's in a workspace when it's not" 或锁文件冲突）。
     - **也不要改成 `crates/snapclip-*` 之类"范围更窄的 glob"**——这是 T0.3 实测踩到的坑：cargo 的成员 glob 只有在**匹配到 ≥1 个目录**时才展开；一个都匹配不到时它按字面路径处理，直接失败：
       `error: failed to load manifest for workspace member ...\crates/snapclip-* / failed to read ...\crates\snapclip-*\Cargo.toml (os error 123)`。
       第一个 `snapclip-*` crate 落地之前（T0.4 之前），任何 `crates/snapclip-*` 写法都会让 workspace 起不来。`apps/*` 同理——P4 建出 `apps/snapclip` 之前不要写。
  2. 让现有 `src-tauri/Cargo.toml` 继承 workspace（保持 `[package]` 与 Tauri 配置不变）。
  3. `crates/rapid-ocr-rs` 是 `optional = true` 的路径依赖，workspace 化后要确认 `cargo check --workspace --all-targets`（不带 `--features ocr-rapid`）与 `cargo check --workspace --all-targets --features ocr-rapid`（在 `src-tauri` 内）都仍然解析成功。
  4. 确认 Cargo.lock 位置变化后，`cargo test --lib --manifest-path src-tauri/Cargo.toml` 与 `cargo check --workspace --all-targets` 都通过。
  5. `git status --porcelain` 必须能看到根 `Cargo.toml`/`Cargo.lock`（如果看不到，说明第 0 步没做对）。
  6. 真机跑一次 `npm run tauri dev`（或现有启动方式）+ F5，确认 Tauri 构建与 overlay 不受影响。
- **实测结果（2026-10-07，本任务已完成）**：
  - `.gitignore`：`crates/` → `/crates/rapid-ocr-rs/`；`git check-ignore -v crates/snapclip-model` **无输出**（exit 1），`crates/rapid-ocr-rs` 仍被忽略（`.gitignore:55`）。
  - 根 `Cargo.toml`：`members = ["src-tauri"]`、`exclude = ["crates/rapid-ocr-rs"]`、`resolver = "2"`；`[profile.release]`（codegen-units/lto/opt-level/panic/strip）从 `src-tauri` 搬到根上，源码不变。
  - `src-tauri/Cargo.lock`（7164 行，680 包）删除，改由根 `Cargo.lock`（690 包）统一管理；**两个 lock 都包含 `rapid-ocr-rs`/`ort` 子树**，可选路径依赖没有被 workspace 化丢掉。
  - 冷构建 `cargo check --workspace --all-targets` = **1m35s**（根 `target/` 首次全量），**0 warning**；重编译 `snapclip` 一个 crate = **1.19s**（改一行后跑整条门禁 1.32s，与 T0.3 之前的 1.36s 同量级，**迭代速度没有退化**）。
  - G0 门禁：`cargo test --lib --manifest-path src-tauri/Cargo.toml` → **403 passed / 0 failed / 6 ignored**；三个探针复跑一致（浏览器 41/41·52·finer=0，Explorer 12/25·65.8·25/25，A4 passed）。
  - `cargo tree -p snapclip --features ocr-rapid` 解析成功且含 `rapid-ocr-rs v0.7.0 (path)` → 可选 feature 的解析没被破坏（**未做** `--features ocr-rapid` 的实际编译：那要拉 ONNX Runtime，代价大且与本步目标无关）。
  - 真机代理验证（自动可跑的部分）：`cargo build` 后在 `target/debug/snapclip.exe` 启动 10 s，日志依次出现 `capture overlay ready elapsed_ms=72`、`clipboard pipeline ready elapsed_ms=83`、`webview page_load Finished`，退出后无残留进程。
  - **仍需人工**：`npm run tauri dev` + 按 F5 的交互验证（agent 无法在你的屏幕按 F5）。
- 必须保持：Tauri 构建可用；`tauri.conf.json`、前端构建脚本不变。
- 验收：§0.2 的 G0 门禁全过；`git check-ignore` 对 `crates/snapclip-*` 无输出；真机 F5 一次成功。
- 回退：删除根 `Cargo.toml`，恢复 `.gitignore` 与 lock 位置。
- 风险：中（Tauri CLI 与 workspace 的交互是唯一不确定点，用真机构建兜住；其次是外部 crate 的 workspace 归属）。

### T0.4 建 `snapclip-model` 骨架

- 前置：T0.3
- 动作：
  1. `crates/snapclip-model/`：`Cargo.toml`（仅标准库 + serde）+ `src/lib.rs`。
  2. 建 `ids.rs`、`geometry.rs`、`artifact.rs`、`events.rs`、`error.rs`、`recognition.rs` 六个空模块，并在 `lib.rs` 里 `pub use`。
  3. 先只把**值对象**搬进来：`Rect`/`Point`/`ImageDimensions`（从 `capture/geometry.rs` 复制定义，原位置改为 `pub use snapclip_model::…` 的**过渡 re-export**，P1 结束前删除）。
  4. 为值对象写单测（边界、负坐标、包含关系），保证与旧行为逐位一致。
- **实测结果（2026-10-07，本任务已完成）**：
  - `crates/snapclip-model/`：`Cargo.toml` 运行期**只依赖 `serde`**（`serde_json` 只在 `[dev-dependencies]`，用于钉住 `ImageDimensions` 的线上 JSON 格式）；`lib.rs` 导出 `geometry` 及 `Point`/`Rect`/`ImageDimensions`；`ids.rs`/`artifact.rs`/`events.rs`/`error.rs`/`recognition.rs` 是带职责说明的空模块（各自写明由哪个 T 编号填充）。
  - **唯一实现已在新 crate**：`Point`/`Rect`/`ImageDimensions` 的定义只存在于 `crates/snapclip-model/src/geometry.rs`；`src-tauri/src/capture/geometry.rs`（1590 → 1421 行）与 `src-tauri/src/domain/payload.rs` 各留一处 `pub use` 转发，并写明"T1.10 前删除"。
  - 单测 **11 passed / 0 failed**：3 条从旧 `geometry.rs` **逐字搬来**的回归（拖拽归一化、挖洞四块、无洞退化），外加负坐标、包含关系边界、`inflate`/`translate`、`clamped_into`（含超宽矩形钉边）、`intersect`/`union`、线上 JSON 格式。
  - **搬移途中发现一个真实语义（必须记住，否则会被"顺手修"）**：`Rect::width()/height()` 用的是 `saturating_sub`，它**只防 i32 溢出、不钳到 0**——倒置矩形（`right < left`）的宽度是**负数**，唯一拦住它的是 `is_empty()`（`width <= 0`）。更坑的是 `area()` 对倒置矩形会得到**正数**（两个负数相乘）。行为**原样保留**（T0.4 只搬不改），但把它写成了显式测试 `width_is_signed_and_is_empty_is_the_guard_against_inverted_rects`：将来谁想"顺手把 width 钳到 0"，测试会先红。
  - 门禁：`cargo test -p snapclip-model` 11/11；`cargo test --lib --manifest-path src-tauri/Cargo.toml` **403 passed / 0 failed / 6 ignored**（与搬移前一致）；`cargo check --workspace --all-targets` 0 warning；浏览器探针复跑 41/41·available=52·finer=0。
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
- **实测结果（2026-10-07，本任务已完成）**：
  - 删除前先验证引用为 0：`rg -n "capture::platform" src-tauri/src` 无匹配（组合根直接写 `platform::windows::capture::overlay::WindowsOverlay`）。
  - 删除 `src-tauri/src/capture/platform/{mod.rs,windows.rs}` 与 `capture/mod.rs` 的 `#[cfg(windows)] pub mod platform;`；`UnsupportedOverlay` 占位实现、它的 `#[cfg(not(windows))] use CaptureError` 与 `#[cfg(not(windows))] pub fn start()` 一并删除，`rg UnsupportedOverlay` 已清零。
  - **删除暴露出一处真正的死代码（根因：它靠转发层"续命"）**：删掉 `capture::platform::windows` 这个 pub 转发后，`WindowsOverlay::window_state()` 立刻变成 `never used` —— 它唯一的引用面就是那条转发路径（`src-tauri/tests/` 是空的，docs/prototypes 里也没有引用）。顺着删掉整条快照链：`window_state()` 方法、`OverlayWindowState` 结构体、`OverlayShared.window` 字段与其两处赋值、只被它使用的 `GWL_EXSTYLE` 常量、测试模块里那个只为它存在的 `GetWindowLongPtrW` extern 声明（那个"样式必须可激活"的测试本身不调用它，只在自己的注释里提了一句）。
  - 门禁：`cargo test --lib --manifest-path src-tauri/Cargo.toml` **403 passed / 0 failed / 6 ignored**；`cargo check --workspace --all-targets` **0 warning**；真机代理：`cargo build` 后启动 `target/debug/snapclip.exe`，`overlay excluded from capture` + `overlay ready hwnd=0x1510cfc thread=59840` + `capture overlay ready elapsed_ms=57` 正常。
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
- 验收：§0.2 门禁全过；启动日志中"未触发 OCR 的会话"没有 `ocr worker started`；触发一次 OCR 后出现一次；T0.2 的资源表 `after` 栏填写。
- 注意：**不要预设"线程数与常驻内存必然下降"**。`OcrManager::new()` 是否在构造时就加载语言/模型，需要实测确认（`rg -n "fn new" src-tauri/src/ocr/manager.rs` 看它做了什么）。惰性化只保证"不触发 OCR 就不启动 worker"；资源数字如实记录，差异写清解释。
- 回退：`git revert`。
- 风险：中（低风险改动，但要小心"首次任务"路径的竞态：两个任务同时首次入队只允许启动一次 worker）。

### T0.5.4 资源 after 与阶段收尾

- 前置：T0.5.3
- 动作：
  1. 按 §1.2 填 `after` 栏，与 before 并列写进提交消息。
  2. 打 tag `refactor-p05`。
- 必须保持：§1.1 全部门禁仍绿。
- 验收：资源表两栏齐全，且**每一条差异都有解释**。数字没变化或变差时，如实写"无显著变化/变差"并给出原因，不要为了让表格好看而改口径。
- 风险：低。

---

## 5. P1：抽离 `snapclip-capture`

> 本阶段是整次重构最大的单点。纪律：**一次只做一件事，每做完一个文件/一个职责就跑一次 §0.2 门禁**。
>
> **迁移接线策略（先读这一节，否则会在 T1.4 卡住）**：旧 crate 的代码不是"搬过去就完了"——`src-tauri` 必须始终可编译。规则是：
>
> 1. **唯一实现只在新 crate**：新 crate 里的实现是唯一版本，不接受"两边各一份"；
> 2. **旧模块变薄转发**：`src-tauri/src/...` 下的旧文件在迁移期只保留 `pub use` 转发（每搬一个文件，旧文件立刻变成一行 `pub use`），阶段末整目录删除；
> 3. **每一步都可编译**：每搬一个文件/一个责任就跑一次该阶段门禁（G0/G1 见 §0.2）；
> 4. **不新增兼容层**：转发只允许存在于迁移进行中的那一阶段，**T1.10 结束时必须为零**。
>
> 两条必须在 P1 就定好的交付协议（否则 P2 会推翻 P1）：
>
> - **`infrastructure/image`（PNG 编码）**：P1 期间留在 `src-tauri`（`PngArtifactEncoder` 属于组合根），capture 只通过已有的 `ArtifactEncoder`/`ArtifactDir` **端口**使用它，端口签名不变；**P2 的 T2.4 才把编码与写盘一起搬到 `snapclip-history`**，capture 那时改成交付字节 + 元数据。
> - **`CaptureService::finish_artifact`（导出落盘）**：P1 不动它的签名与调用链（overlay/export worker 继续用 `ArtifactDir`）；P2 的 T2.4 一次性切到 `CaptureOutput { bytes, metadata }` + `CaptureArtifactStore`。**两个阶段各改一次不行**——只允许在 T2.4 改一次。

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

> **任务粒度规则**：T1.6 是**任务组**，由 T1.6.1–T1.6.5 五个**子任务**组成。**每个子任务 = 一个提交 + 一次该阶段门禁**（子任务之间不允许把两个职责合并进同一个提交，也不允许"先全拆完再跑门禁"）。§0.4 的"每个任务一个提交"在这里展开为"每个子任务一个提交"。
>
> 每个子任务的公共约束：
>
> - 前置：T1.5（子任务之间串行，按编号顺序做）。
> - 顺序规则：**先搬生产代码，再搬该职责的测试**；只搬移与可见性收敛，**不改行为、不改断言**（要改断言说明搬错了）。
> - 回退：`git revert` 对应子任务的提交。
> - 风险：高。这个文件同时持 HWND、会话、输入、渲染与吸附状态，**一次只动一块**。

#### T1.6.1 拆出 `overlay/window.rs`

- 动作：窗口类注册与 WNDCLASS、HWND/窗口创建与销毁、窗口消息循环、焦点获取与生命周期（含 `WM_*` 分发骨架）。
- 必须保持：overlay 仍是**独立原生 HWND + 独立线程**；F5 热键仍注册在 overlay 线程；窗口类名不变（`SnapClipCaptureOverlay`，它是精度探测与捕获排除逻辑的依赖）。
- 验收：§0.2 门禁全过 + 真机 F5 起 overlay / Esc 取消一次。

#### T1.6.2 拆出 `overlay/input.rs`

- 动作：鼠标与键盘事件路由、`WheelAccumulator`、`PointerGesture`、光标形状管理。
- 必须保持：滚轮与 ↑↓ 切层语义、按点切换目标的节流/合并行为、光标形状反馈（`mouse_move_coalesced_count` 量级不恶化）。
- 验收：§0.2 门禁全过 + 真机滚轮向上/向下各换层一次。

#### T1.6.3 拆出 `overlay/state.rs`

- 动作：会话绑定与预览状态机、层级游走与提示/动画状态（`WalkColour`、`ChainVisibility`、`RingAppear`、`ArmedHint`）。
- 必须保持：**A2 挖洞**、**A3 绿跟滚轮**、动画与 `chain_fade_frames`/`walk_frames` 的现有语义；层级计数/徽标数值与 `level=` 日志一致。
- 验收：§0.2 门禁全过 + 真机"滚动时绿框出现、停止后回蓝"一次。

#### T1.6.4 拆出 `overlay/render_submit.rs`

- 动作：渲染提交、damage 回合与合并 tick、`present_us`/`over16ms` 计量。
- 必须保持：**截图高频路径零 IPC**（F5 → overlay 不经过壳）；`over16ms=0`、`present_us` 与基线同量级；导出 PNG 不带任何 overlay UI。
- 验收：§0.2 门禁全过 + 真机会话汇总里 `over16ms=0`。

#### T1.6.5 拆出 `overlay/window_restore.rs` + 子任务组收尾

- 动作：窗口恢复/取消/异常清理路径（取消、失败、设备丢失、资源释放），然后做一次**子任务组收尾**：`overlay.rs` 只留组合入口（`mod.rs` 级别的装配），确认无残留死代码。
- 必须保持：取消/失败路径不泄漏 GPU 与 HWND 资源（`session graphics released` 仍出现一次）。
- 验收：§0.2 门禁全过 + **真机全链路**（悬停 → 滚轮换层 → 确认 → PNG 导出）+ 一次 Esc 取消；汇总 `over16ms=0`。
- 风险：高（这一步是整组唯一的"整体验收"，前四个子任务各自的真机验证不能替代它）。

### T1.7 `uia_provider.rs` 测试搬家（3 338 行，测试 72%）

- 前置：T1.5
- 动作：
  1. 先把 `#[cfg(test)] mod tests`（2 416 行）按主题拆成 crate **内部**的子模块 `windows/accessibility/tests/{mod,browser,explorer,sources,timeout}.rs`，用 `mod` 嵌套（必要时 `#[path]`）挂回 `uia_provider`。
  2. 再把生产代码（约 916 行）按 COM 初始化、UIA 查询、树缓存、元素身份校验、超时调用拆文件。
- **不要搬去 `tests/` 集成测试目录**：现有测试大量依赖 provider 的**私有函数**与测试辅助类型（夹具、构造器），搬到 `tests/` 会强迫把私有项改成 `pub`，等于把内部实现泄进公共 API（违反 T1.2 与 §10.1 的接缝门禁）。确实需要跨文件共享的辅助类型，放进 `#[cfg(test)] mod tests_support`。
- 必须保持：**两个实机探针的可运行性与期望值**（浏览器 41/41、Explorer 12/25·65.8）；探针的启动方式（自带临时 profile 起浏览器）不变。
- 验收：§0.2 门禁全过（探针仍从同一命令跑 `--ignored --nocapture`）；`rg -n "pub fn" src/windows/accessibility` 命中数**不比搬之前多**（多了说明为了搬测试泄了私有项）。
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
- 动作：跑完整 §0.2 门禁 + 真机全链路 + 删掉所有过渡 re-export（T0.4/T1.3 留下的转发模块），打 tag `refactor-p1`。
- 验收（四条，逐条给证据；**不要用 `rg snapclip_model` 当"壳还能编译"的证明**——壳引用 `snapclip_capture`/`snapclip_model` 本来就应该存在，这条既能命中也能落空，证明不了任何事）：
  1. **旧路径引用为零**：`rg -n "crate::capture::" src-tauri/src` 与 `rg -n "platform::windows::capture" src-tauri/src` 都不再命中**实现**（只剩壳对 `snapclip_capture` 的调用）。
  2. **新 crate 自测通过**：`cargo test -p snapclip-capture`（G1 门禁，见 §0.2）。
  3. **壳仍可构建并可用**：`cargo check --workspace --all-targets` 通过 + 真机 F5 → 截图 → Esc 一次。
  4. **依赖方向门禁绿**：T1.9 的检查脚本通过。
- 说明：迁移期允许转发模块存在，**T1.10 结束时必须为零**；被禁的是"引用旧路径实现"与"留下转发模块"，不是"引用新 crate"。
- 风险：低。

---

## 6. P2：抽离 `snapclip-history`

> 关键前提：**artifact 的写盘权先收敛成一条**，再拆 crate。现在 `CaptureService` 通过 `ArtifactDir` 写（截图 PNG），`store/blob.rs` 也在动磁盘（剪贴板内容的 content-addressed blob），两边都写是最难查的一类 bug。
>
> **两套存储不是同一种东西，不许合并**：
>
> | 现有 | 内容 | 语义 |
> | --- | --- | --- |
> | `application/capture_service::ArtifactDir` | 截图 PNG | 目录约定 + 命名 |
> | `infrastructure/store/blob.rs` | 剪贴板内容 blob | `blake3` 内容寻址（`{hash[..2]}/{hash}.blob`）+ 读取校验 + 去重/GC |
>
> 合并会破坏剪贴板去重与 blob GC：blob 的哈希身份是它存在的理由，PNG 文件名是人看的东西。两者可以都属于 `snapclip-history`，但**必须各自独立目录、独立测试**。
>
> **顺序不能颠倒**：类型先定（T2.1）→ crate 再建（T2.2）→ 存储再实现（T2.3）→ 最后才切 capture 的导出调用链（T2.4）。把"在 `snapclip-history` 里定义存储"写在"创建 `snapclip-history`"之前是**不可执行**的，照做会在 T2.1 就卡住。

### T2.1 `snapclip-model` 侧类型定死（**只定义类型，不建 crate**）

- 前置：T1.10
- 动作：
  1. 在 `snapclip-model` 定义 `ArtifactRef { absolute_path, mime, dimensions, byte_len, content_fingerprint }`（`content_fingerprint` = `blake3`，**写入时算好**，读取方不再重算，见 docs/22 §4）。
  2. 定义 `CaptureOutput { bytes, metadata }`：截图产出的**交付形态**。
  3. 此时**不改任何调用方**：`CaptureService::finish_artifact`、`ArtifactDir`、`ArtifactEncoder` 保持原样（签名与调用链在 P1 期间已被冻结，见 §5 顶部的交付协议）。
- 必须保持：只新增类型，不改行为；`cargo test --lib` 仍是基线数字。
- 验收：§0.2 门禁全过（G1，因为 P1 之后代码已在 `snapclip-capture`）。
- 回退：`git revert`。
- 风险：低。

### T2.2 `snapclip-history` 骨架

- 前置：T2.1
- 动作：新增 `crates/snapclip-history/`（依赖 `snapclip-model` + SQLite + 图像编码 + Windows SDK），先只放空模块与 crate 文档注释里的"接缝清单"。
- 必须保持：`src-tauri` 仍可编译（新 crate 暂时没人引用）。
- 验收：`cargo check --workspace --all-targets` 0 warning。
- 回退：删除 crate。
- 风险：低。

### T2.3 两个独立存储：`CaptureArtifactStore` + `ClipboardBlobStore`

- 前置：T2.2
- 动作：
  1. `capture_artifact.rs`：从 `ArtifactDir` 迁目录约定、命名、原子写、清理/LRU；接口收 `CaptureOutput`，返回 `ArtifactRef`（指纹写入时算出）。这是截图 artifact 的**唯一所有者**。
  2. `clipboard_blob.rs`：从 `store/blob.rs` 原样迁入 content-addressed blob（`{hash[..2]}/{hash}.blob`、写入后校验、读取再校验、去重）。这是剪贴板 blob 的**唯一所有者**。
  3. 两者各自独立目录、独立测试文件；**不要抽出共同基类/共同 trait 再实现**——它们的共同点只有"都用文件系统"，抽象化会把两套语义耦死。
- 必须保持：`blob.rs` 的既有测试（`content_is_deduplicated_and_verified_on_read` 等）逐条通过；截图导出的像素与文件名契约（`docs/11 §8.2`）。
- 验收：§0.2 门禁全过 + 两个存储各自的单测通过（含"写入后立即校验字节数/指纹"）。
- 回退：`git revert`。
- 风险：中。

### T2.4 切换 capture 导出链（**P1 冻结的那个签名在这里改一次**）

- 前置：T2.3
- 动作：
  1. 把 `PngArtifactEncoder` 与 `infrastructure/image` 的 PNG 编码一起搬到 `snapclip-history`。
  2. `CaptureService::finish_artifact`（或等价出口）改为交付 `CaptureOutput { bytes, metadata }`；落盘改由壳调用 `CaptureArtifactStore`，返回 `ArtifactRef`。
  3. 删除 `capture` 侧的 `ArtifactDir`/`ArtifactEncoder` 端口与其测试替身；确认 `cargo tree -p snapclip-capture -e normal` 里没有 `snapclip-history`（方向必须是壳调用 history，不是 capture 调用 history）。
- 必须保持：overlay/export worker 的调用链仍然只经过**一个**落盘出口；导出 PNG 与历史可见性不变；`docs/21 §5.26` 的 ring 调色板仍只有一处定义（搬动时别复制）。
- 验收：§0.2 门禁全过 + 真机截图 → 导出 → 历史可见 → 磁盘文件可打开；`ArtifactRef.content_fingerprint` 与实际文件 `blake3` 一致。
- 回退：`git revert`。
- 风险：中（数据面切换；护栏是"导出后立即校验字节数与指纹"的测试）。

### T2.5 拆 `store/mod.rs`（1 595 行）

- 前置：T2.4
- 动作：拆为 `db/connection.rs`、`db/migration.rs`、`clip_repository.rs`、`artifact_repository.rs`、`recognition_repository.rs`；`Store` 保留为组合门面或直接消失（由调用方持有仓库）。
- 必须保持：迁移顺序与 schema 版本；现有 `migration_upgrades_existing_v1_database` 等测试逐条通过；分页/去重语义不变。
- 验收：§0.2 门禁全过；用一份已有数据库文件跑一次真实读取（复制一份到测试临时目录，不要销毁用户数据）。
- 回退：`git revert`。
- 风险：高（`migration_upgrades_existing_v1_database` 是这条路径的护栏，先读它再动）。

### T2.6 迁移剪贴板 Windows 适配（6 文件 / 1 210 行）

- 前置：T2.5
- 动作：迁 `platform/windows/clipboard/{formats,image_norm,listener,reader,source_app,mod}.rs` 到 `snapclip-history/src/windows/`。
- 必须保持：文本/HTML/图片格式读取、延迟渲染格式、来源程序识别行为不变。
- 验收：§0.2 门禁全过 + 真机复制文本/图片各一次，历史里正确入库。
- 风险：中。

### T2.7 迁 `application/clipboard_ingest.rs`

- 前置：T2.6
- 动作：
  1. 迁为 `snapclip-history/src/{service,reader,history}.rs`；按 docs/22 §7.2 拆"事件接收 / 去重窗口 / 格式读取 / publication / 识别入队"。
  2. **保留 `OcrQueue` 端口**（现在 `application/clipboard_ingest::OcrQueue` 由 `app/ocr_queue.rs` 实现，这是已经存在的正确模式）：端口留 history，实现由壳绑定到 `snapclip-recognize`。
- 必须保持：去重窗口时长、publication 事件字段、`clipboard-updated-v1` 契约不变。
- 验收：§0.2 门禁全过 + 真机连续复制去重行为与之前一致。
- 风险：中。

### T2.8 `ClipboardService` / `HistoryService` 公共 API

- 前置：T2.7
- 动作：定义接缝（查询分页、按 id 取详情、复制回写、删除、订阅低频事件）；公共类型用 builder + reader，**不暴露 pub 字段**；内部 repository 类型不 re-export。
- 必须保持：`commands/*` 里现有行为逐条对齐（改命令前先把旧行为列成清单）。
- 验收：§0.2 门禁全过。
- 风险：中。

### T2.9 `commands/history.rs`、`commands/ocr.rs` 改走服务

- 前置：T2.8
- 动作：把直接 `State<'_, Store>` 改成调用 `HistoryService`/`ClipboardService`（识别相关的入队/重试/状态查询改走 recognize 的服务或 `OcrQueue` 端口）。`commands/capture.rs` 已经走 `CaptureRuntime`，不动。
- 必须保持：前端命令名/返回 JSON 结构不变（前端契约）。
- 验收：§0.2 门禁全过 + 前端历史页/OCR 操作各点一次无报错。
- 回退：`git revert`。
- 风险：低。

### T2.10 阶段验收 + tag

- 前置：T2.9
- 动作：完整 §0.2 门禁 + 真机"复制 → 历史 → 复制回写 → 删除"全链路 + 回退演练（§0.6）+ tag `refactor-p2`。
- 风险：低。

---

## 7. P3：抽离 `snapclip-recognize`

### T3.0 接缝设计（**先定接口，再搬代码**）

- 前置：T2.10
- 背景事实（已核实的当前实现，正是它让"只搬文件"行不通）：
  - `ocr/engine.rs`：`OcrInput::Png(Arc<[u8]>)` —— 输入是**内存字节**，不是文件引用。
  - `ocr/worker.rs`：`OcrService` 直接持有 `Store`（`tauri::AppHandle` 也一并持有），worker 线程**直接读写 OCR 数据库**（`list_ocr_candidates`/`enqueue_ocr`/`claim_ocr_job`/finish）。
  - `OcrQueue` 端口已存在且模式正确（定义在 `application/clipboard_ingest.rs`，由 `app/ocr_queue.rs` 实现）——新设计沿用它，不要另起一套。
- 动作：定义三个 trait（放进 `snapclip-recognize`，由壳/history 实现）：

  | trait | 职责 | 谁实现 |
  | --- | --- | --- |
  | `ArtifactReader` | 按 `ArtifactRef` 交出字节（或内存映射） | 壳 / history（`CaptureArtifactStore`） |
  | `RecognitionJobStore` | 领取/认领任务、写回结果与状态 | history（`recognition_repository`） |
  | `RecognitionEventSink` | 状态事件出口（P0.5 已产出的 `OcrEventSink` 是它的基础版，事件名仍 `ocr-status-v1`） | 壳（Tauri 适配器 / GPUI 适配器） |

- 目标调用链（**方向单向，`recognize` 不依赖 `history`，也不依赖 `Store`**）：

  ```
  壳 / history 创建任务（clip_id + ArtifactRef）
    → recognize worker 经 ArtifactReader 取字节，跑引擎
    → recognize 返回 RecognitionResult
    → 壳 / history 经 RecognitionJobStore 持久化结果
  ```

- 必须保持：现有 OCR 结果的字段与语义、`ocr-status-v1` 事件契约。
- 验收：接口定义提交里只有 trait + 类型，**没有搬运实现**；`cargo check --workspace --all-targets` 0 warning。
- 回退：`git revert`。
- 风险：中（接口定错，T3.1 会返工；所以这一步单独提交、单独评审）。

### T3.1 crate 骨架（迁 `ocr/` 829 行 / 6 文件）

- 前置：T3.0
- 动作：新增 `crates/snapclip-recognize`，迁 `engine.rs`（`OcrEngine`/`OcrCancel`，**保留契约**）、`manager.rs`、`worker.rs`、`win_ocr.rs`、`rapid.rs`（`ocr-rapid` feature 默认关）、`mod.rs`；接缝输入改为 `ArtifactRef`（经 `ArtifactReader` 取字节）；`OcrService` 不再持有 `Store` 与 `tauri::AppHandle`，改为按 T3.0 的三个 trait 注入（内部仍可用 `Arc<[u8]>` 处理已读入的字节，那只是实现细节，不再跨越接缝）。
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
- 动作：接线严格按 T3.0 的链路，**持久化发生在壳/history 一侧**：
  1. `snapclip-history` 在入库时只发布 `ArtifactRef` 与低频事件（不发内存字节、不发数据库句柄）。
  2. 由 `app`（或未来的 GPUI 壳适配层）实现 `OcrQueue` + `RecognitionJobStore` + `RecognitionEventSink`，把任务提交给 `snapclip-recognize`，拿到 `RecognitionResult` 后**自己**调用 history 写回。
  3. `snapclip-recognize` 不依赖 `snapclip-history`、不依赖 `Store`（T3.0 已把这两条依赖删掉，这里只是把实现接上）。
- 必须保持：`ocr-status-v1` 事件契约；历史里 OCR 文本/状态字段不变。
- 验收：§0.2 门禁全过 + 真机截图含文字 → 历史里出现文本；`cargo tree -p snapclip-recognize -e normal` 中不含 `snapclip-history`、`rusqlite`、`tauri`。
- 风险：中。

### T3.4 资源对比 + 阶段验收 + tag

- 前置：T3.3
- 动作：重测 §1.2 并**如实记录**，然后回退演练 + tag `refactor-p3`。
- 验收：**不出现不可接受的退化**（拆 crate 本身不保证降低运行时资源，所以不写"必须更好"）；任何"更好"的结论都必须有 §1.2 的数字支撑，没有数字就只写"无显著变化"。
- 风险：低。

---

## 8. P4：GPUI 壳（**独立立项**）

> 纪律：这一阶段与 P1–P3 分开提交、分开验收。换壳会让"历史面板、来源图标、复制回写"这类真实功能出现回退（其中历史面板当前甚至不可达，见 T4.1.1），**不能和 crate 拆分混在一批**，否则回归无法归因。
>
> 本阶段的范围（按 T4.1.1 的矩阵与决策 D1）：历史 + 剪贴板动作 + 托盘 + 设置（新增）+ 事件桥。**标注 UI 本轮不做**（D1）；OCR 只保留行内状态，不建独立页面。

### T4.1 开工前研读（不许跳）

- 动作：读 `gpui-kit` 的 SKILL 与 **Coding Guides**（分层、`RenderOnce` vs `Entity<T>`、状态归属、`ElementId`、事件/焦点、异步、公共 API、测试分层），设计可见界面时读 **Design Guides**；查组件用 `https://gpui-kit.com/llms.txt` + `component/{name}.md`。
- 必须记住的四条硬约束：**GPUI 只通过 `gpui-kit` 使用**（不要把 `gpui` 直接加进 `Cargo.toml`）；`gpui_kit::init(cx)` 在建组件视图前调用一次；每个窗口第一层是 `Root`；**绝不凭记忆写 API**（先查签名）。
- 验收：本阶段要用的组件（`List`/`VirtualList`、`Input`、`Button`、`WindowExt` 覆盖层等）逐个查过文档，并在提交消息里列出确认过的文档路径。

#### T4.1.1 前端功能迁移矩阵（**做完这张表才有资格动 P6**）

先把现状盘出来（2026-10-07 实测；执行前用右边两列的命令复核一遍）：

| 现有前端功能 | 位置 | 当前真实状态 | 复核命令 |
| --- | --- | --- | --- |
| 历史面板（列表/搜索/分页/复制回写/删除） | `src/features/history/*` + `HistoryPanel.vue`，由 `App.vue` 装配 | 代码完整，但主窗口 `visible: false`，且**没有任何显示入口**（无托盘、无第二个热键）→ 运行时不可达 | `rg -n "visible" src-tauri/tauri.conf.json`；`rg -n "RegisterHotKey" src-tauri/src` |
| 剪贴板动作（复制回写、来源程序图标） | `src/features/clipboard/api.ts`、`commands/clipboard.rs`、`icon.rs` | 可用；历史面板依赖它渲染每行的来源图标（`icon.rs` 是**图标提取缓存**，不是托盘） | `rg -n "pub fn" src-tauri/src/icon.rs` |
| OCR 状态与事件 | `src/features/ocr/{api,events}.ts`、`commands/ocr.rs` | 命令与 `ocr-status-v1` 事件可用；**没有独立 OCR 页面**，UI 只是行内状态 | `rg -n "ocr-status-v1" src-tauri/src src` |
| 截图命令与事件 | `src/features/capture/*`、`commands/capture.rs` | 可用（F5 → overlay → 导出/剪贴板） | 真机 F5 |
| 标注工具条 | `src/features/annotation/AnnotationToolbar.vue` + `capture/annotation.rs`(1 123 行) + `capture_annotation` 命令 | Rust 侧文档模型与命令都在；Vue 工具条**没有被 `App.vue` 装配**（只有组件文件与 API） | `rg -n "AnnotationToolbar" src` |
| 托盘 | 无 | **不存在**（`Cargo.toml` 无 tray 插件；除 F5 外没有其它全局热键） | `rg -n "tauri-plugin" src-tauri/Cargo.toml` |
| 设置 | 无 | **不存在**（全仓库无 settings 页面、无设置存储） | `rg -ni "settings" src-tauri/src src` |

据此填出迁移矩阵（**这是本任务的交付物，写进提交消息**）：

| 现有功能 | GPUI 目标模块 | 是否保留行为 | 验证方式 |
| --- | --- | --- | --- |
| 历史面板 | `apps/snapclip/src/history/` | 保留；**并补上"可达性"**（托盘/热键/窗口显示，现状没有） | `#[gpui_kit::test]` + 真机 |
| 剪贴板动作与来源图标 | 随 history 能力（`clipboard` 子模块） | 保留 | UI test + 真机 |
| OCR 行内状态 | 随 history 行渲染（typed event） | 保留（不新增 OCR 页面） | typed event 单测 + 真机 |
| 截图命令/事件 | capture crate + overlay（**不进 GPUI 高频路径**） | 保留 | 真机 F5 |
| 标注工具条 | **本轮不做 GPUI 界面**（见下方决策 D1） | **本轮不保留 UI 行为**（Rust 侧模型保留） | P6 前只核对"模型未丢"，UI 留待 V2 |
| 托盘 | `apps/snapclip/src/tray.rs` | **新增能力**（不是迁移） | 真机 |
| 设置 | `apps/snapclip/src/settings/` | **新增能力**（不是迁移） | 热更新测试 |

**硬规则**：这张矩阵没有填完（每行都有目标模块与验证方式）之前，**不允许删除 Vue/Tauri**（P6 前置）。

**决策 D1（2026-10-07，用户决定）：标注功能本轮暂不做。**

这是本轮唯一的功能性让步，必须写清后果，不许含糊：

- **不做**：P4 不为标注建任何 GPUI 界面（不写 `apps/snapclip/src/annotation/`），也不把 `AnnotationToolbar.vue` 搬到 GPUI。
- **保留**：`snapclip-capture` 侧的标注文档模型（`annotation.rs`，1 123 行纯状态，无 Win32/GPU）与 overlay 内已有的标注接线**原样保留**，随 T1.4 迁移，**不要**为了"统一风格"删掉或加 `#[allow(dead_code)]`。
- **已知损失**：P4 之后标注**没有 UI 生产者**——没有界面能发出 `AnnotationCommand`。这是被接受的取舍，不是 bug，也不要在 P6 之前为了"保住它"临时加兼容层或保留 Vue 壳。
- **P6 的处置**：删除 Vue/Tauri 时，`src/features/annotation/*`、`src/infrastructure/tauri/commands/annotation.ts` 与 `commands/capture.rs::capture_annotation` 一并删除；`snapclip-capture` 侧不动。
- **重新接线的时机**：留到 V2 独立立项（界面归属届时再定：画进 overlay / 放进 GPUI 壳）。V1 的隔离靠 `snapclip-capture` 保留模型，不靠留着旧壳。

- 验收：矩阵的每一行都有目标模块与验证方式；决策 D1 已记录在本文（不另开文件）；矩阵内容随提交消息一起提交。

### T4.2 `apps/snapclip` 骨架

- 前置：T4.1、T3.4
- 动作：`gpui_kit::application().with_assets(...).run(|cx| { gpui_kit::init(cx); … })` + `open_window` + `Root`；窗口标题/尺寸/DPI 行为对齐现有 Tauri 主窗口；**不接**截图 overlay（仍是 capture crate 的原生 HWND）。
- 依赖边界（说清以免误解）：**GPUI 只能通过 `gpui-kit` 使用**（不要把 `gpui` 直接写进 `Cargo.toml`）；但**应用本身仍然依赖能力 crate**——`snapclip-model`、`snapclip-capture`、`snapclip-history`、`snapclip-recognize` 都是壳的正常依赖。"只依赖 gpui-kit"说的是 UI 层，不是整个 app。
- 验收：能打开一个空壳窗口；§0.2 门禁不受影响；截图 overlay 仍可独立 F5 起来。
- 风险：中。

### T4.3 history 能力（GPUI 侧）

- 前置：T4.2
- 动作：`apps/snapclip/src/history/{model,history_view,commands}.rs`；`Entity<HistoryState>` 持有列表/搜索/选中；列表用 `List`/`VirtualList` 虚拟化；**`ElementId` 用 clip id**；缩略图懒加载。
- 必须保持：与现有前端一致的行为（分页、去重展示、复制回写、删除）。
- 验收：`#[gpui_kit::test]` + `VisualTestContext` 覆盖：加载、搜索过滤、键盘上下移动、回车复制、删除确认。
- 风险：高（第一个真正的 GPUI 功能）。

### T4.4 settings 能力（**新增能力，不是迁移**）

- 前置：T4.3
- 事实校正：当前仓库**没有任何设置功能**——没有设置页、没有设置存储、没有配置读写通道（`rg -ni "settings" src-tauri/src src` 只命中一条文档注释）。所以 T4.4 是**从零建通道**，不要写成"先落现有设置项"。
- 动作：
  1. `apps/snapclip/src/settings/`：定义设置模型（serde + 默认值）、持久化位置（与现有 store 的数据目录同族，不要新开一处）、读写与校验。
  2. 只接**当前真实存在**的行为开关；新增开关必须先有消费方，不造空开关。
- 验收：改设置 → 立即生效（热更新到 overlay / recognize），重启后保持。
- 风险：中（设置是"所有模块都要读的东西"，模型定错会牵动多个 crate：设置类型应放在 `snapclip-model` 或独立的 `snapclip-settings`，**不要**让每个 crate 各自定义一份）。

#### T4.4.1 把两个窗口检测开关接成真实设置通道（**独立子任务，不要和 T4.4 混做**）

- 前置：T4.4
- 背景事实（实测）：`capture/window_detection/mod.rs` 里只有 `DEFAULT_ADOPT_TEXT_RUNS: bool = true` 这一个真实的布尔开关；`deep_select_text_runs` / `deep_select_visible_wrappers` 这两个名字**只存在于文档注释**里，代码里没有对应常量。所以这不是"给现有常量加通道"，而是"把注释里承诺的开关真正建出来"。
- 动作：
  1. 在 `window_detection` 里把文字跑条行为与"跳过无绘制包装层"策略显式化成可注入的配置（默认值保持现状：`ADOPT_TEXT_RUNS = true`，包装层跳过行为不变），替换硬编码常量。
  2. 把这两个开关 + 动画参数接进 T4.4 的设置通道，并热更新到 overlay（overlay 读的是快照，不是每次绘制都查设置）。
  3. 补测试：默认值下行为与今天逐位一致（这是"忠实于现状"的护栏，见 docs/21 §5.19）。
- 必须保持：默认行为完全不变（文字跑条仍会被捕为目标；`nested-*` 相关探针期望值不变）。
- 验收：§0.2 门禁全过 + 探针 `browser_element_probe` 仍 41/41；改开关 → overlay 行为立即变化 → 重启后保持。
- 风险：中。

### T4.5 事件桥

- 前置：T4.3
- 事实校正：**GPUI 不能复用现有 `EventEnvelope`**——它定义在 Tauri 的 `events` 模块里并且依赖 `tauri::Emitter`（`src-tauri/src/events/mod.rs`）。让 GPUI 壳去依赖 Tauri 等于换壳失败。
- 动作：
  1. 在 `snapclip-model` 定义框架无关的 `AppEvent`（只带：id、状态、尺寸、`ArtifactRef`、错误码、generation/revision），**它是共享事件的唯一形态**。
  2. `apps/snapclip/src/adapters.rs`：Cargo 侧走 typed channel；**Tauri 侧适配器**才把 `AppEvent` 序列化成 `EventEnvelope`（保持 `schemaVersion` 字段与 IPC 事件名不变，前端契约不动）。
  3. 用 `cx.spawn`/`background_spawn` 做 I/O，**只在 `Entity::update` 里改状态**；丢弃过期事件沿用现有纪律（`generation`/revision 语义），复用 `SnapshotEpoch`/`RequestId`/`RequestGate` 这类已有机制，**不要发明第二套 revision 体系**。
- 必须保持：高频路径零 IPC（F5 → overlay 不经过 GPUI）。
- 验收：事件到达顺序/丢弃语义有单测；`cargo tree -p snapclip -e normal` 中不含 `tauri`/`wry`；真机 F5 → 历史自动刷新。
- 风险：高。

### T4.6 托盘（Win32）

- 前置：T4.2
- 事实校正：**当前没有托盘**（`Cargo.toml` 只有 `tauri-plugin-opener`；`src-tauri/src` 里 "tray" 只命中窗口类名 `Shell_TrayWnd`）。`icon.rs` 是"按 exe 路径提取来源程序图标"的缓存（供历史行显示），**不是托盘实现**，不能"沿用"。
- 动作：`apps/snapclip/src/tray.rs` **新建** Win32 托盘（`Shell_NotifyIcon` + 消息窗口，或 gpui-kit 若已提供等价组件——先用 `https://gpui-kit.com/llms.txt` 确认）；菜单项（显示/隐藏/退出/截图）向壳发低频事件。**显示窗口这一条同时补上 T4.1.1 里记录的"现状不可达"缺口**。
- 必须保持：截图高频路径不经过托盘消息循环。
- 验收：托盘菜单全部可用；退出干净（无残留进程/线程）；窗口可显示/隐藏（当前 `visible: false` 且无入口，必须变成可达）。
- 风险：中。

### T4.7 测试三层

- 前置：T4.3–T4.6
- 动作：纯函数 → `#[gpui_kit::test]` → `VisualTestContext`（焦点/键盘/指针/布局）→ 真实窗口按**无障碍树**断言（role/label/value/enabled/focus）。
- **无障碍断言先做 spike**：在把它写成硬门禁之前，先用一个最小用例验证当前 gpui-kit 版本确实能读到窗口的无障碍树（查文档 + 跑通一次）。验证不通过时：门禁降级为"`VisualTestContext` + 真机人工走查"，并把结论写进 docs/22 §10.2，**不要**因为做不到就把这条悄悄删掉。
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

- 前置：
  1. T4.9（GPUI 壳完成托盘、隐藏/显示、焦点、退出、DPI、多显示器回归）。
  2. **T4.1.1 的前端功能迁移矩阵每行都已填完**（有目标模块、有验证方式）。标注工具条的归属已由 T4.1.1 的**决策 D1** 定为"本轮暂不做"（模型保留在 `snapclip-capture`，UI 不迁移），因此它**不再阻塞 P6**。
- 能力搬迁前置：`icon.rs`（**来源程序图标提取，不是托盘**）与 `commands/clipboard.rs` 的复制回写，必须先作为能力落到 GPUI 壳（T4.3 的来源图标列依赖它），再删旧实现。
- 删除前的最后核对（对应决策 D1）：`snapclip-capture` 里的标注模型**已随 crate 保留**，确认这一点后，标注相关的 Vue/命令代码与其它前端代码一起删除。
- 清单：
  1. 删 `src-tauri/src/commands/*`、Tauri `events` 适配、`app/`（组合根）与 `icon.rs` 的旧实现（都在上面的能力搬迁完成之后）。
  2. 删前端（Vue adapter、`src/shared/contracts.ts` 与 package.json 的 Tauri 相关脚本）与 `src-tauri/` 目录本身；**`src/features/annotation/*` 与 `AnnotationToolbar.vue` 一并删除**（决策 D1：本轮不做标注 UI，`snapclip-capture` 侧模型保留）。
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
| 4 | 写盘权分裂（截图 PNG 与剪贴板 blob 两套） | T2.3：各自收敛成**一个**所有者（`CaptureArtifactStore` / `ClipboardBlobStore`），**不许合并成一套**，否则破坏 blob 去重与 GC |
| 5 | 共享 crate 变胖 | `snapclip-model` 只放稳定值对象/事件摘要；内部类型不 re-export（T1.2 + §10.1 门禁） |
| 6 | 测试搬迁窗口期 | 搬测试只改路径不改断言；这期间不加新功能 |
| 7 | 增量编译时间恶化 | §1.2 记录"改 model 一行"的耗时；明显恶化再决定拆 crate 粒度 |
| 8 | `overlay.rs` 是唯一真单点 | T1.6：一次只搬一块 + 每块跑门禁 |
| 9 | 用户手稿（`prototypes/`）与无关改动 | §0.6：先确认归属，不顺手提交/丢弃 |
| 10 | `reset --hard` 丢工作 | §0.6：先建 `backup/*` 分支或 tag；`reflog` 只是保险不是备份 |
| 11 | **迁移中途加兼容层**（"先两边各留一份，回头再删"） | §5 顶部的迁移接线策略：唯一实现在新 crate，旧模块只做 `pub use` 转发，T1.10 结束时转发必须为零；这是本项目最容易被违反的一条 |
| 12 | **把"文档里写过"当成"代码里有"** | 例：`deep_select_text_runs` 只存在于注释、托盘从未实现、设置通道从未存在。动手前先用 `rg` 核实，写进 §13 记录 |
| 13 | 无障碍树断言做不到却被当硬门禁 | T4.7：先 spike；做不到就降级并写回 docs/22 §10.2，不要静默删门禁 |
| 14 | **标注功能被静默丢弃**（决策 D1 的已接受让步） | T4.1.1 的 D1 写清了"不迁移 UI、保留 Rust 模型"；P6 删前端前必须核对 `snapclip-capture` 侧 `annotation.rs` 仍在，并**明确写进 P6 的提交消息** |

---

## 12. 完成判据（Definition of Done）

### 12.1 单个任务

- [ ] `git status --porcelain` 干净开始，任务 = 一个提交，消息含任务号与门禁数字。
- [ ] §0.2 门禁全过（含两个实机探针），数字与基线一致或变化有解释。
- [ ] 任务条目里的"必须保持"逐条核对过。
- [ ] 回退点明确（上一个 tag 或本任务前一个提交）。
- [ ] §13 的状态词已更新（**勾选框不是完成状态**；见 §0.1）。

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

---

## 13. 执行记录模板与状态词表

### 13.1 状态词表（**唯一允许用来声明"完成"的说法**）

| 状态 | 含义 | 允许宣称的事 |
| --- | --- | --- |
| 未开始 | 还没动 | 无 |
| 进行中 | 改了代码，门禁没跑完 | "在做"，**不能说完成** |
| 门禁失败 | 跑了门禁，红了 | 只能说"卡在哪条门禁、红在哪一行" |
| 已验证 | 门禁全过，还没提交 | "本地验证通过" |
| 已提交 | 本地提交完成 | "提交好了"（**还没备份**） |
| 已推送 | 提交到了远端 / tag 也推了 | "有回退点了" |
| 已回退 | 用 `git revert`（或经批准的回退）撤销 | "已恢复到 <sha / tag>" |

规则：**勾选框 ≠ 完成**。§2 的 `[ ]` 只表示"动作做了没有"；声明完成必须同时具备"已验证 + 已推送 + §13 记录填全"。

### 13.2 执行记录模板（每个任务复制一份）

```
任务编号：T1.6.3
状态：未开始 / 进行中 / 门禁失败 / 已验证 / 已提交 / 已推送 / 已回退
分支：refactor/p1-capture（或 <默认分支>）
前置提交/tag：refactor-p05（sha: …）
修改范围：<文件清单 + 大概行数>
修改前测试：cargo test --workspace --all-targets → 403 passed / 6 ignored
修改后测试：<同上格式，写出真实数字>
性能指标：<只写与本任务相关的；没测就写"未测"，不要留空>
人工验证：<真机步骤 + 观察到的日志/现象>
失败与根因：<红了什么、根因属于实现/接口/数据结构/边界/流程/抽象 哪一类>
提交 SHA：<sha>
推送/tag：<origin/<分支> 已推送 / tag refactor-p1 已推送>
回退对象：<上一个 tag 或本任务前一个提交的 sha>
```

填写纪律：

- **数字必须来自实际命令输出**，不手写、不从旧文档抄（§1.1 已经踩过一次：`docs/21 §10` 的 `373/2` 是过期基线）。
- 探针结果要写明是"第几次跑"（§0.3 的假红复跑规则）。
- 出现"没做/没测"的部分，**明确写"未完成"**，不要用含糊表述掩盖（本项目禁止伪完成）。

---

## 14. 执行记录（实际填写）

> 逐任务按 §13.2 模板追加。状态词只用 §13.1 里的七个。

### §14.1 T0.1 复跑并记录正确性基线

```
任务编号：T0.1
状态：已验证（自动门禁全绿；真机会话汇总待人工）
分支：main（HEAD 0bd9191）
前置提交/tag：0bd9191（docs: record decision D1）
修改范围：只改 docs/23 §1.1/§0.2/§2；无代码改动
修改前测试：—
修改后测试：cargo test --lib --manifest-path src-tauri/Cargo.toml
             → 403 passed / 0 failed / 6 ignored
             cargo check --all-targets --manifest-path src-tauri/Cargo.toml
             → 0 warnings（0.39 s，缓存命中）
             browser_element_probe → asserted=41 passed=41 failed=0
                                     available=52 / finer=0
                                     p50=31.9 p95=52.2 max=61.2 ms
             explorer_rule_probe   → control_level_points=12/25
                                     median_area_pct=65.8
                                     available=25/25 / finer=0
                                     p50=69.1 p95=93.1 max=116.8 ms
             ring_contrast_probe   → passed（底色表与 docs/21 §5.26 一致）
             the_embedded_subset_covers_the_strings_the_overlay_draws → passed
性能指标：本任务不涉及
人工验证：真机会话汇总（F5 悬停/滚轮/确认，看 present_us / over16ms）**未跑**：需要真人操作鼠标，留到 T0.5 或 P1 阶段补
失败与根因：无失败。踩到一个文档问题：T0.3 之前没有 workspace 根，
             `cargo check --workspace --all-targets` 会直接报 "could not find Cargo.toml"；
             已在 §0.2 与 §1.1 里改成 `--manifest-path src-tauri/Cargo.toml`
提交 SHA：待提交（本次改动与 T0.2 记录同一个提交）
推送/tag：待推送
回退对象：0bd9191
```

> **过程说明**：§1.1 的表格由另一个并行会话填写过一遍（同一工作区），本次按 §0.1 的要求**独立复跑**了一遍全部命令，数字逐项一致（403 / 41·52 / 12·65.8·25 / A4 / 字体 / 事件契约）。两个会话的结论一致；冲突的处置与教训见 §14.4。

### §14.2 T0.2 记录资源基线（before）

```
任务编号：T0.2
状态：已验证（数字见 §1.2；GPU 内存一项未测）
分支：main（HEAD 0bd9191）
前置提交/tag：0bd9191
修改范围：只改 docs/23 §1.2；无代码改动（唯一一次"改文件"是 `lib.rs` 的 mtime 触碰，
          触碰后 `git status --porcelain` 仍为空，已确认内容未变）
修改前测试：—
修改后测试：同上（T0.1 的 403 仍绿；`cargo check` 0 warning）
性能指标：见 §1.2 的 before 栏（1 进程 / 18 线程、37.7 MB WS、
          WebView2 +6 进程 / +342.2 MB、空闲 CPU ~0.00%、
          冷启动到 clipboard pipeline ready 68 ms、exe 19.34 MB、
          改一行 → cargo check 1.36 s）
人工验证：无需；测量脚本与口径写在 §1.2 下方
失败与根因：GPU 内存未测（需要按 pid 归因的 GPU 计数器，本轮未采集）——
            **明确记为未完成**，不是"已测为 0"。
            另外发现：直接跑 debug exe 时前端走 devUrl，没有 dev server 会反复重载，
            所以 `webview page_load Finished` 不能作为"可交互"口径（已改口径）
提交 SHA：待提交（与 T0.1 同一个提交）
推送/tag：待推送
回退对象：0bd9191
```

### §14.3 T0.2 期间发现的阻塞项（必须在 T0.3 一起修）

| # | 发现 | 证据 | 后果 | 处理 |
| --- | --- | --- | --- | --- |
| B1 | `.gitignore` 的 `crates/` 忽略整个目录，而 `crates/rapid-ocr-rs` 是外部仓库、新的 `crates/snapclip-*` 将要放进去 | `git check-ignore -v crates/snapclip-model` → `.gitignore:53:crates/`（该目录此时还不存在，就已经被规则命中）；`crates/rapid-ocr-rs/.git` 存在；`git status --ignored` 显示 `!! crates/` | 重构新建的 4 个能力 crate **无法被提交**：本地全绿、远端什么都没有，属于典型的伪完成 | 已写进 T0.3 的动作第 0 步（`.gitignore` 改成 `/crates/rapid-ocr-rs/`，并用 `git check-ignore` 验证） |
| B2 | 并行会话在同一工作区改同一份文档（同一分支 `main`，两次提交相隔 19 s） | §1.1/§2 出现非本会话的改动；`91c9a1b`(17:35:55) 与本会话 `adaa80f`(17:36:14) 交替出现；另一会话的完整过程留在 `refer/review-report1.txt` | 两个 agent 同时改 `docs/23`，后面还要同时改根 `Cargo.toml`/`src-tauri/Cargo.toml`，互相覆盖风险高；实际已造成 §14 出现两个重复章节 | **已裁决（2026-10-07，用户）**：本会话为唯一执行者；重复章节在 T0.3 之前清理（见 §14.4） |

### §14.4 并行冲突的处置记录（B2 的收尾）

| 时间（2026-10-07） | 事件 |
| --- | --- |
| 17:35:55 | 会话 B 提交 `91c9a1b`（T0.1 基线），顺带把会话 A 当时未提交的 §0.2/§1.2 改动一起提交 |
| 17:36:14 | 会话 A 提交 `adaa80f`（T0.2 资源基线 + T0.1/T0.2 执行记录 + B1/B2） |
| 17:36:28 | 会话 A 提交 `7dff649`（B1 证据行号修正） |
| — | 两个会话各自独立发现冲突并停下报告；此时 `docs/23` 里出现了**两个 `## 14`**（双方各写了一份执行记录） |
| 用户裁决 | **本会话为唯一执行者**，另一个会话停止；重复章节由本会话清理 |

清理动作（本会话执行）：删除后一份 `## 14. 执行记录（按 §13.2 模板，逐任务追加）`（内容与本会话 §14.1 记录重复；其中唯一独有的信息是会话 B 的口径说明，已并入本记录表），保留单一 §14。

**教训（写进 §0.7 的执行面）**：这次的代价不是代码，是"同一份文档被两个执行者各写一遍"。所以 §0.7 的"独立 worktree"不是可选项——如果再有并行需求，先 `git worktree add`，再动手。

### §14.5 T0.3 建 workspace 骨架

```
任务编号：T0.3
状态：已验证 + 待提交（自动门禁全绿；`npm run tauri dev` + F5 属人工，未做）
分支：main
前置提交/tag：223cd7d（§14 去重）；回退基准 smart-snapping-v1-2026-10-07
修改范围：.gitignore、新增根 Cargo.toml、src-tauri/Cargo.toml（移除 [profile.release]）、
          删除 src-tauri/Cargo.lock、新增根 Cargo.lock、docs/23（T0.3 章节 + §2 + §14.5）
修改前测试：403 passed / 0 failed / 6 ignored（HEAD 223cd7d 的 G0）
修改后测试：403 passed / 0 failed / 6 ignored（workspace 化之后，命令同上）
            cargo check --workspace --all-targets → 0 warning（冷构建 1m35s）
            浏览器探针 41/41·available=52·finer=0（p50=28.2 p95=33.9 max=44.8）
            Explorer 探针 12/25·65.8·available=25/25·finer=0
            A4 ring_contrast → passed
性能指标：改一行后 `cargo check --workspace --all-targets` = 1.32 s（T0.3 之前 1.36 s，
          同量级 → 迭代速度无退化）；冷构建 1m35s（一次性）
人工验证：未做（`npm run tauri dev` + F5）。已做其自动化代理：
          `cargo build` → 启动 target/debug/snapclip.exe 10 s →
          `capture overlay ready`=72ms / `clipboard pipeline ready`=83ms / webview Finished → 无残留进程
失败与根因：一次失败，属于**工具语义误用**，不是设计问题——
          `members = ["src-tauri", "crates/snapclip-*", "apps/*"]` 直接让 cargo 报
          "failed to read ...\crates\snapclip-*\Cargo.toml (os error 123)"：
          cargo 的成员 glob 在匹配到 0 个目录时按字面路径处理。
          根因解决：成员逐个显式列（`members = ["src-tauri"]`），每建一个 crate 加一行。
提交 SHA：待提交
推送/tag：待推送
回退对象：223cd7d
```

补充取证（可复核）：

- `git check-ignore -v crates/snapclip-model` → 无输出（B1 已修）；`git check-ignore -v crates/rapid-ocr-rs` → `.gitignore:55:/crates/rapid-ocr-rs/`。
- 两个 lock 的包数：旧 `src-tauri/Cargo.lock` 680 包、新根 `Cargo.lock` 690 包，**两者都含 `rapid-ocr-rs`/`ort`/`imageproc`**。
- `cargo tree -p snapclip --features ocr-rapid` 含 `rapid-ocr-rs v0.7.0 (path)`，无解析错误；**未做** `--features ocr-rapid` 的实际编译（会拉 ONNX Runtime，代价与本步无关），记为未完成。

### §14.6 T0.4 建 `snapclip-model` 骨架

```
任务编号：T0.4
状态：已验证（自动门禁全绿）
分支：main
前置提交/tag：51cca9e（T0.3）；回退基准 smart-snapping-v1-2026-10-07
修改范围：新增 crates/snapclip-model/{Cargo.toml, src/lib.rs, src/geometry.rs 及 5 个空模块}；
          src-tauri/src/capture/geometry.rs（移除 Point/Rect 定义 → pub use）；
          src-tauri/src/domain/payload.rs（移除 ImageDimensions 定义 → pub use）；
          src-tauri/Cargo.toml（+snapclip-model 路径依赖）；根 Cargo.toml（members +1）；docs/23
修改前测试：403 passed / 0 failed / 6 ignored（HEAD 51cca9e）
修改后测试：cargo test -p snapclip-model → 11 passed / 0 failed
            cargo test --lib --manifest-path src-tauri/Cargo.toml → 403 passed / 0 failed / 6 ignored
            cargo check --workspace --all-targets → 0 warning
            浏览器探针 → asserted=41 passed=41 failed=0 / available=52 / finer=0
性能指标：不涉及（纯类型搬移）
人工验证：不涉及
失败与根因：1 次失败，属于**我的测试断言写错**，不是实现错误——
            我按 `saturating_sub` 的字面印象断言"宽度不会为负"，实测倒置矩形 width=-300。
            根因：`saturating_sub` 只防溢出，不钳 0；而 `area()` 对倒置矩形是**正数**。
            处置：不改实现（T0.4 只搬不改），改断言并把它写成显式的行为钉死测试。
            这是"测试红了要判断是实现错还是预期错"的一个正例。
提交 SHA：待提交
推送/tag：待推送
回退对象：51cca9e
```

### §14.7 T0.5.1 删除 `capture/platform` 转发层

```
任务编号：T0.5.1
状态：已验证（自动门禁全绿；真机 F5 仍待人工）
分支：main
前置提交/tag：533f307（T0.4）；回退基准 smart-snapping-v1-2026-10-07
修改范围：删除 src-tauri/src/capture/platform/{mod.rs,windows.rs}；
          src-tauri/src/capture/mod.rs（去掉 pub mod platform）；
          src-tauri/src/capture/application/runtime.rs（去掉非 Windows 占位）；
          src-tauri/src/platform/windows/capture/overlay.rs（删除 window_state 死代码链）；
          docs/23
修改前测试：403 passed / 0 failed / 6 ignored；cargo check 0 warning（HEAD 533f307）
修改后测试：403 passed / 0 failed / 6 ignored；cargo check --workspace --all-targets 0 warning
性能指标：不涉及
人工验证：真机 F5 未做；自动化代理已做——启动 target/debug/snapclip.exe，
          `overlay excluded from capture` / `overlay ready hwnd=0x1510cfc thread=59840` /
          `capture overlay ready elapsed_ms=57` 全部正常
失败与根因：2 次门禁红，都是**删除副作用**，不是设计错：
          (1) `window_state()` 报 never used —— 根因是它此前只被刚删掉的 pub 转发路径"续命"；
          (2) 删字段后又冒出 `GetWindowLongPtrW` never used —— 同一根因的下一层。
          处置：顺着依赖链把死代码整条删掉（方法 / 结构体 / 字段 / 常量 / extern 声明），
          而不是把 pub 转发加回来"消警告"（那是隐藏问题）。
          **需要用户确认的判断**：我删的是"没有任何消费者"的诊断快照；
          如果真机验收清单里将来要用它（比如断言 GWL_EXSTYLE），
          应该在需要时连同**真实消费者**一起重新引入，而不是留着空壳。
提交 SHA：待提交
推送/tag：待推送
回退对象：533f307
```
