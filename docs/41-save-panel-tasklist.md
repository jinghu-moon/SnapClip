# 保存面板：分阶段 TDD 任务清单

> 依据：权威规格 `docs/design/SnapClip 保存面板 · 功能与 UI 规格.md`（628 行，以下简称**规格**）、
> 论证与差距 `docs/40-save-panel-plan.md`（405 行）、后端现状（下述 `文件:行` 均实际查证）、
> 工程规则 `AGENTS.md`（根因优先 / 验证通过才算完成 / 测试必须真能红）。
>
> **本文件是执行清单**：每一阶段都是「测试先行 → 实现 → 验收命令全绿 → 提交」的闭环。
> 估计人天含写测试与真跑验证，不含规格 §15 的待决项。

---

## 0. 已定决策（不再重开）

| # | 决策 | 依据 |
| --- | --- | --- |
| ① | **JPG 默认 4:4:4**；4:2:0 仅作高级项，标注「体积更小、文字更糊」 | `docs/34 §11`：文字 PSNR 28.52 → 26.27 dB，而整图 SSIM 仅 −0.0003（整图指标会掩盖文字损失）。规格 §6.2 的 4:2:0 默认是原型沿袭 |
| ② | **不做有损 WebP**；删掉「无损压缩」开关与 WebP 质量档 | 后端只有无损（`save_format.rs:142-149`）；有损仅 4:2:0，对 UI 文字不利（`docs/34 §9.1`） |
| ③ | **质量只承诺 92**；75 / 50 必须先补测 | 92 有实测（`docs/34 §11`），75/50 无任何实测。补测手段：`codec-bench --quality`（`main.rs:133`），档位矩阵见 `main.rs:239-241` |
| ④ | **删除 JPG 渐进式** | 行带 sink 只写 baseline，且 `row_band_jpeg.rs:282-288` 有「progressive markers must not appear」断言；渐进式要整图多扫描 ⇒ 与「长图流式、不物化整图」根本冲突 |
| ⑤ | **删除「移除元数据」** | 三个 sink 本来不写 EXIF/ICC，该开关无事可做 |
| ⑥ | 预览优先**只显示编码后字节数**，不显示解码画面 | 零新依赖；解码画面（需 JPG/WebP 解码器）作为可选阶段 S4b |

**辅助决策**（`docs/40 §5.2` 的九条，本清单默认执行）：维持 `zenjpeg` / `zenwebp` / `windows-sys`；质量存**数值**而非档位；保存后关闭面板、Toast 移到独立小窗；面板固定最大高度、内部滚动。

---

## 0.1 现状锚点（全部查证过）

| 物件 | 位置 | 状态 |
| --- | --- | --- |
| `SaveFormat{Png,Jpeg,Webp}` | `crates/snapclip-capture/src/save_format.rs:32` | `[已有]` |
| `max_side()` / `can_encode()` / `label()` | 同文件 `:117` / `:129` / `:145` | `[已有]` |
| `JPEG_FRAME_BUDGET_PIXELS = 16_000_000` | 同文件 `:78`、`:84` | `[已有]` |
| `SavePreferences{format, jpeg_quality}` | 同文件 `:236-239`；`quality()` `:253`；`DEFAULT_JPEG_QUALITY = 92` `:260`；1–100 `:261-262` | `[已有]`，但**没有任何 UI 能改** |
| `convert(master, format, quality, impl Write) -> Result<Conversion, ExportError>` | `apps/snapclip/src/capture/convert.rs:36` | `[已有]`，**无生产调用者** |
| `convert_to_file(master, out, format, quality)`（`.partial` + rename） | 同文件 `:135` | `[已有]`，同上 |
| 既有转换测试 7 条 | 同文件 `:302` PNG 字节拷贝、`:315` JPG 可读、`:329` 质量生效、`:347` 帧预算拒绝、`:367` WebP 拒绝、`:379` 损坏母版、`:394` 失败不留文件 | `[已有]`，可直接扩展 |
| JPG 行带 sink（baseline + 强制 Huffman） | `apps/snapclip/src/capture/row_band_jpeg.rs`（SOF2 断言 `:282-288`） | `[已有]` |
| `Destination{Copy,Pin}` | `crates/snapclip-capture/src/region.rs:355` | `[缺]` `Save` |
| 确认分派 | `crates/snapclip-capture/src/windows/overlay/session.rs:1177` | `[需改]` |
| 滚动路径写死 PNG + session id 的桌面副本 | `crates/snapclip-capture/src/windows/overlay/scroll.rs:567` | `[需改]` |
| 滚动前置格式检查 | `crates/snapclip-capture/src/scroll/mod.rs:129-147`、`overlay/scroll.rs:280` | `[已有]` |
| **字体子集**（文案硬约束） | 词汇表 `subfont/drawn-text.txt`；生成器 `subfont/build_subset.py`、`subfont/subset.ps1`；覆盖门禁测试 `crates/snapclip-capture/src/windows/win/d2d/tests.rs:1451`；词汇导出 codegen `tests.rs:1509-1515`（`#[ignore]`，经 `subset.ps1` 运行） | `[已有]`，**新文案必须先进词汇表** |

---

## 1. 阶段总表

| 阶段 | 目标 | 出口测试 | 依赖 | 人天 |
| --- | --- | --- | --- | --- |
| **S0** | 文案字形就位 + 现状基线 | 字形覆盖门禁绿 | — | 1 |
| **S1** | 保存服务（无 UI） | 3 格式 × 2 冲突 × 11 校验 + 失败注入 | S0 | 3–4 |
| **S2** | 编码服务（字节数，无解码画面） | 体积随质量单调 + 缓存命中零编码 | S1 | 2–3 |
| **S3** | 模态表单骨架（无预览） | 11 条校验逐条可见 + 键盘 | S1 | 5–7 |
| **S4** | 预览（字节数态）+ 对比滑块骨架 | 缩放/网格/标签/背景 | S2, S3 | 4–5 |
| **S4b** |（可选）解码画面预览 | 三格式解码后可显示 | S4 + 解码依赖 | 3–4 |
| **S5** | 尺寸缩放 + 限制体积（二分） | 上限真被满足（留 3%） | S2 | 4–5 |
| **S6** | 高级选项（PNG 等级 / JPG 填充 + 4:2:0） | 每项真实影响字节 | S2 | 3–4 |
| **S7** | 长截图分割 + PDF | 阈值/张数/页数逐条 | S5 | 4–6 |
| **S8** | Toast / 撤销 / 重命名 / 保存后动作 | 撤销删除 + 序号回退 | S3 | 3–4 |
| **S9** | 键盘全链路 / UIA / 动效 / 高对比 | **人工验收脚本**（见 §S9） | S3–S8 | 4–6 |
| **S10** | 接进捕获流程（`Destination::Save`） | 分派单测 + `--scroll` 像素不变 | S1, S3 | 2–3 |

**合计 ≈ 36–52 人天。**
**最小可用纵切（能保存、能选格式、能看体积、能拖出）= S0 + S1 + S2 + S3 + S10 ≈ 13–18 人天**，
其中「能看体积」用 S2 的字节数态即可，不含 S4 的画面预览。

---

## S0. 前置：文案字形 + 基线确认

**目标**：规格里所有**新文案**能被渲染（否则是豆腐 ✗），且现状测试基线被记录。

**测试先行清单**

| 测试名 | 断言 | 先红原因 |
| --- | --- | --- |
| `every_new_save_panel_phrase_has_a_glyph` | 把 §0.2 列出的全部新文案字符集并入 `subfont/drawn-text.txt` 后，`d2d/tests.rs:1451` 的覆盖门禁通过 | 词表缺字 ⇒ 门禁红（这是真实红，不是测试写错） |

**实现清单**

1. 从规格逐字摘出文案，与现有 `subfont/drawn-text.txt` 求**字符差集**；
2. 差集非空 ⇒ 追加进词表 ⇒ 跑 `subfont/subset.ps1`（它调用 `build_subset.py`，并会用 `tests.rs:1515` 的 `#[ignore]` codegen 反写词汇）；
3. 记录基线：`cargo test -p snapclip-capture --lib`、`cargo test -p snapclip-app --features test-support --lib`、`cargo run -p codec-bench --release -- --assert` 的当前读数。

**验收命令**

```bash
powershell -NoProfile -ExecutionPolicy Bypass -File subfont/subset.ps1
cargo test -p snapclip-capture --lib
```

**风险与回退**：字体重建若改变现有字形度量（可能影响面板排版），以「覆盖门禁 + 现有 UI 测试」为准；一旦红，回退词表增量、分批加入。

**估时**：1 人天。

### §0.2 S0 必须就位的文案（逐字，来自规格）

**校验 11 条**（`docs/40 §1.8` 已从规格抄录）：

```
路径为空
请输入完整路径，如 C:\Users\…
路径含非法字符
没有写入权限，请换一个目录
{盘}: 仅剩 x，保存需约 y
目录不存在，保存时创建
立即创建
文件名不能为空
含非法字符 {去重后的字符列表}
替换为 _
"{名}"是系统保留名
完整路径超过 260 字符
末尾的点或空格会被系统去掉
去掉
```

**底栏与冲突**：`改为覆盖`、`改为重命名`、
**Toast 四操作**（规格 §7.6，逐字见规格原文，S8 实现时再摘）。

> **纪律**：S0 只负责「字形就位」，不负责语义。文案的**语义实现**在 S3（校验）与 S8（Toast）。

---

## S1. 保存服务（无 UI）

**目标**：给定「目录 + 模板 + 序号 + 格式 + 质量 + 冲突策略」，把 PNG 母版写出成目标文件并返回最终路径，全程有校验、有失败语义、原子提交。

**测试先行清单**（全部放在 `apps/snapclip/src/save/service.rs` 的 `#[cfg(test)]`；命名按行为）

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `a_png_request_copies_the_master_byte_for_byte` | 目标文件与母版**字节相同**；`Outcome.bytes` = 母版长度 | 服务不存在 |
| 2 | `a_jpeg_request_produces_a_readable_jpeg_of_the_master_size` | 用 `zenjpeg` 解码出宽高 = 母版宽高；`format == Jpeg` | 同上 |
| 3 | `a_webp_request_produces_a_lossless_webp` | 解码后**逐像素**等于母版 | 同上 |
| 4 | `a_second_save_of_the_same_name_becomes_underscore_two` | 第二次保存产生 `名_2.PNG`，第一次的文件仍在 | 同上 |
| 5 | `overwriting_a_name_replaces_it_and_no_partial_survives` | 覆盖后只剩一个文件；同目录**无 `.partial`** | 同上 |
| 6 | `the_template_expands_every_variable_the_spec_defines` | `SnapClip_[日期]` → 形如 `SnapClip_2026-10-18.PNG`；`[时间]`=HHmm、`[序号]`=7、`[分辨率]`=`1440x900`、`[软件名]`=`SnapClip` | 变量解析不存在 |
| 7 | `an_unknown_variable_is_an_error_not_literal_text` | 模板含 `[不存在]` ⇒ `Err(InvalidTemplate)`，不产出文件 | 同上 |
| 8 | `a_relative_or_empty_directory_is_refused` | `Err(PathNotAbsolute)` / `Err(PathEmpty)` | 同上 |
| 9 | `a_missing_directory_is_created_on_save` | 目录不存在 ⇒ 保存成功且目录被创建（规格「目录不存在，保存时创建」） | 同上 |
| 10 | `a_read_only_directory_reports_no_permission` | `Err(NoPermission)`，且**不留文件、不留 `.partial`** | 同上 |
| 11 | `a_full_disk_is_reported_as_not_enough_space` | 注入写失败 ⇒ `Err(NotEnoughSpace{needed, available})`，不留残件 | 同上 |
| 12 | `a_name_with_an_illegal_character_is_refused_with_the_character` | `Err(IllegalName(char))`，错误里带**那个字符** | 同上 |
| 13 | `a_reserved_windows_name_is_refused` | `CON` / `NUL` / `COM1` ⇒ `Err(ReservedName)` | 同上 |
| 14 | `a_name_ending_in_a_dot_or_space_is_refused` | `Err(TrailingDotOrSpace)` | 同上 |
| 15 | `a_path_longer_than_260_is_refused` | `Err(PathTooLong)` | 同上 |
| 16 | `a_scroll_master_above_the_jpeg_budget_is_refused_before_writing` | 超 16 MP 的母版 + JPG ⇒ `Err(FormatTooLarge)`，且**母版未被解码**（用「母版文件不可读也能立刻失败」的次序断言） | 同上 |
| 17 | `a_webp_request_beyond_16383_refuses` | 宽或高 > 16383 ⇒ `Err(FormatTooLarge)` | 同上 |
| 18 | `the_sequence_advances_only_after_a_successful_save` | 成功 ⇒ 序号 +1；失败 ⇒ 序号不变 | 同上 |

**实现清单**

| 文件 | 内容 | 为什么这样切 |
| --- | --- | --- |
| `apps/snapclip/src/save/mod.rs`（新） | 模块出口，`SaveRequest` / `SaveOutcome` / `SaveError` | 与 `capture` 平级：保存是**独立能力**，不该塞进 capture |
| `apps/snapclip/src/save/template.rs`（新） | `Template::parse` / `expand(&self, ctx: &TemplateContext) -> Result<String, SaveError>` | 纯函数，可穷举测试；变量集合由规格 §5.2 固定为 5 个 |
| `apps/snapclip/src/save/validate.rs`（新） | `validate_path` / `validate_name`（对应规格 §8 的 11 条） | 与模板解耦：可以单元测试**不产出文件** |
| `apps/snapclip/src/save/conflict.rs`（新） | `resolve(dir, stem, ext, policy) -> PathBuf`；`ConflictPolicy{AutoRename, Overwrite}` | 不区分大小写；`_2`、`_3`…取第一个不存在 |
| `apps/snapclip/src/save/service.rs`（新） | `save(master: &Path, request: &SaveRequest) -> Result<SaveOutcome, SaveError>`：校验 → 解析冲突 → `convert_to_file` → 返回路径与字节数 | **唯一**调用 `convert_to_file` 的地方 ⇒ `convert()` 从此有生产调用者 |
| `apps/snapclip/src/save/service.rs` | 复用 `convert_to_file`（`convert.rs:135`）的 `.partial` + rename；**不要**重新发明原子写 | 已有实现已验证（`convert.rs:394`） |
| `crates/snapclip-capture/src/save_format.rs` | 若 `SaveError` 需要「格式装不下」，复用 `can_encode` / `within_jpeg_frame_budget`（`:129` / `:84`） | 上限判据只有一处 |

**接口形状（草案，实施时可微调但要在此登记）**

```rust
pub struct SaveRequest {
    pub directory: PathBuf,
    pub template: Template,        // 已 parse，默认 "SnapClip_[日期]"
    pub sequence: u32,             // [序号] 的当前值
    pub format: SaveFormat,
    pub quality: u8,               // 1–100，仅 JPG 有效
    pub conflict: ConflictPolicy,  // 默认 AutoRename
}

pub struct SaveOutcome {
    pub path: PathBuf,             // 最终路径
    pub bytes: u64,                // 成品大小
    pub format: SaveFormat,
}

pub fn save(master: &Path, request: &SaveRequest) -> Result<SaveOutcome, SaveError>;
```

**验收命令**

```bash
cargo test -p snapclip-app --lib save::
cargo test -p snapclip-app --features test-support --lib
cargo run -p codec-bench --release -- --assert      # 证明没碰坏编码路径
git hook run pre-push
```

**风险与回退**

-「磁盘满」在当前机器上不可真造 ⇒ 用**注入**（把写接口抽成 `trait Writer` 或在测试里传一个会失败的 `Write` 实现）；若难以注入，退回「只读目录」覆盖同一错误分支，并在文档里标注 `[未验证]`。
- `[序号]` 的持久化在 S1 **不做**（只接收参数）⇒ 持久化属于 S3/S8。
- 若模板变量解析与规格的「光标处插入」交互冲突，以**保存服务的语义**为准（交互是 S3 的事）。

**估时**：3–4 人天。

---

## S2. 编码服务（字节数，不解码画面）

**目标**：一次编码同时给出「编码后字节数」，供格式菜单与预估大小显示；带防抖、代际取消与缓存。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `a_smaller_jpeg_quality_yields_fewer_bytes` | q92 > q75 > q50 的字节数**严格递减**（同一母版） | 服务不存在 |
| 2 | `a_cached_request_encodes_nothing` | 同一 (母版摘要, 格式, 质量, 高级项) 第二次请求 ⇒ `encodes == 0` | 同上 |
| 3 | `changing_an_advanced_option_invalidates_the_cache` | 改了 PNG 等级 ⇒ 重新编码（规格 §12.4 的键必须含高级项） | 同上 |
| 4 | `a_newer_request_cancels_the_older_one` | 代际取消：旧请求不再回填 | 同上 |
| 5 | `the_sizes_reported_match_the_file_actually_written` | 预估字节数 == S1 写出的文件大小 | 同上 |

**实现清单**

| 文件 | 内容 |
| --- | --- |
| `apps/snapclip/src/save/encode.rs`（新） | `encode_size(master, format, quality, advanced) -> Result<u64, ExportError>`（复用 `convert()` 的 `impl Write` 到一个计数 sink ⇒ **不落盘**） |
| `apps/snapclip/src/save/cache.rs`（新） | `CacheKey{ master_digest, format, quality, advanced }` + LRU |
| `apps/snapclip/src/save/debounce.rs`（新） | 防抖 + 代际号；**不用新线程池**，复用 GPUI 的异步任务 |

**为什么这样切**：`convert()` 已经接受 `impl Write`（`convert.rs:36`）⇒ 传一个 `CountingWriter` 就能"只编码不写盘"，**零新依赖**。这也正是 `docs/40` 更正过的结论。

**验收命令**

```bash
cargo test -p snapclip-app --lib save::encode save::cache
```

**风险与回退**：4K 母版编码耗时会阻塞 UI 线程 ⇒ 必须走后台任务；若 GPUI 任务模型不适配，退化为「只在打开格式菜单时编码一次 + 手动刷新」。

**估时**：2–3 人天。

---

## S3. 模态表单骨架（无预览）

**目标**：920 宽模态窗，含路径 / 模板 / 格式 / 质量 / 冲突 / 底栏；11 条校验**逐条可见**；键盘可达。

**测试先行清单**（能单测的入单测；不能的写人工步骤）

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `the_form_reports_each_validation_rule_without_saving` | 对 11 条规则各构造一个非法状态 ⇒ 状态行文案逐字等于规格 | 表单不存在 |
| 2 | `the_save_button_is_disabled_while_an_error_stands` | 有 error ⇒ 按钮禁用；只有 warn ⇒ 可用 | 同上 |
| 3 | `the_warning_offers_the_repair_it_names` | 「目录不存在，保存时创建」的「立即创建」可点且真的创建 | 同上 |
| 4 | `the_format_menu_refuses_jpeg_for_a_scroll_master` | 滚动母版（宽高已知且超 16 MP）⇒ JPG 项禁用并给出原因 | 上限未接到 UI |
| 5 | `the_sequence_previews_the_next_number` | `[序号]` 显示下一个值；保存成功后自增 | 持久化未接 |
| 6 | `escape_cancels_and_enter_saves` | 键盘分派（Esc → 关闭；Enter → 触发保存；Ctrl+C → 复制） | 未接 |
| 7 | `the_quality_row_is_absent_for_png` | PNG ⇒ 质量行不渲染（规格 §5.5） | 未接 |

**实现清单**

| 文件 | 内容 |
| --- | --- |
| `apps/snapclip/src/ui/save_panel.rs`（新） | `SavePanel` 实体：字段、派生量、校验状态 |
| `apps/snapclip/src/ui/save_panel/view.rs`（新） | 布局树（左占位/右表单/底栏），尺寸与间距取规格 §11.3 的 token |
| `apps/snapclip/src/ui/save_panel/text_field.rs`（新） | 自绘 TextField（规格 §15 #17 待决：自绘 vs EDIT）——**S3 先自绘**，与现有 d2d 文本绘制一致 |
| `crates/snapclip-capture/src/preferences.rs` | 扩展持久化字段：模板、序号、路径、格式、质量、限制体积、快捷路径、保存后动作（与 `SavePreferences` 合并或并列，**实施时在此登记**） |

**验收命令**

```bash
cargo test -p snapclip-app --lib ui::save_panel
cargo build --release
# 人工：打开面板，逐条制造 11 种非法状态，比对文案
```

**风险与回退**：自绘 TextField 是最大不确定项（输入法、选区、拖选）⇒ 若 3 天内做不出可接受的体验，退回 Win32 `EDIT` 子窗口（规格 §15 #17 正是这个待决项）。

**估时**：5–7 人天。

---

## S4. 预览（字节数态）

**目标**：左栏显示**实际产物**的缩略展示（或仅尺寸与体积信息），加缩放 / 网格 / 标签 / 背景；**不做解码画面**。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `zoom_clamps_between_five_and_eight_hundred_percent` | 5% / 800% 边界与超越 | 不存在 |
| 2 | `the_pixel_grid_appears_only_at_or_above_four_hundred_percent` | 399% 无网格、400% 有网格（规格 §4.5） | 同上 |
| 3 | `the_size_label_reads_output_and_original` | 文案格式与规格一致（如 `1440×900 · 原始 1920×1080`） | 同上 |
| 4 | `the_preview_background_cycles_all_four_options` | 四选一循环（规格 §4.6） | 同上 |

**实现清单**：`ui/save_panel/preview.rs`（变换矩阵、脏区、网格绘制）；背景四态；尺寸标签。

**验收命令**：`cargo test -p snapclip-app --lib ui::save_panel::preview` + 人工看缩放锚点。

**风险与回退**：无解码画面时，"对比滑块"没有意义 ⇒ 滑块与有损呈现**推到 S4b**（这与 `docs/40` 的最小纵切一致）。

**估时**：4–5 人天。

---

## S4b（可选）. 解码画面预览

**前置**：选一个 JPG/WebP 解码器（`image` 过大；候选 `zune-jpeg` / `webp` 等），**先量依赖体积与许可**。
**测试先行**：`a_decoded_preview_matches_the_encoded_file`（解码后与 `convert` 产物逐像素相同）。
**风险**：新增依赖与编译时间 ⇒ 若不可接受，保持只有字节数态，并在规格里标注该功能未实现。
**估时**：3–4 人天。

---

## S5. 尺寸缩放 + 限制体积

**目标**：数值/百分比/锁比/预设；「限制体积」用**连续质量二分**（规格 §12.3 建议）。
**测试先行**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `a_locked_ratio_keeps_the_aspect_when_one_side_changes` | 改宽 ⇒ 高按比例、取整规则一致 | 不存在 |
| 2 | `an_out_of_range_size_is_clamped_with_a_message` | 越界提示文案 | 同上 |
| 3 | `the_size_limit_is_met_within_three_percent` | 二分后文件 ≤ 上限 × 1.03 | 同上 |
| 4 | `binary_search_takes_at_most_seven_encodes` | 编码调用次数 ≤ 7 | 同上 |
| 5 | `a_manual_quality_choice_leaves_automatic_mode` | 手动选档 ⇒ 退出自动（规格 §5.5） | 同上 |

**实现清单**：`save/resize.rs`（重采样依赖需先评估；候选 `fast_image_resize`）、`save/budget.rs`（二分 + 取消）。
**风险**：重采样依赖是新面 ⇒ 先做「不缩放也能二分」的路径，缩放单列子阶段。
**估时**：4–5 人天。

---

## S6. 高级选项

**内容**（按已定决策裁剪后）：PNG 压缩等级（**已有**，接出去即可）；JPG 填充色 + 色度采样（4:4:4 默认、4:2:0 高级项并标注）；WebP **无**无损开关（因为只有无损）。
**测试先行**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `the_png_compression_level_changes_the_bytes_but_not_the_pixels` | 三档字节不同、解码像素相同 | 未接 |
| 2 | `the_jpeg_fill_colour_appears_where_transparency_was` | 透明区域变填充色 | 未接 |
| 3 | `switching_to_420_shrinks_the_file` | 4:2:0 字节更少（并记录文字 PSNR 代价，引用 `docs/34 §11`） | 未接 |
| 4 | `a_non_default_option_shows_a_dot_and_can_be_reset` | 蓝点 + 恢复默认 | 未接 |

**估时**：3–4 人天。

---

## S7. 长截图分割 + PDF

**前置**：规格 §9 的阈值（高度 ≥ 3000 且 ≥ 2.5× 视口）。**PDF 依赖**需选型（`pdf-writer` 候选）。
**测试先行**：`the_split_threshold_is_exactly_3000_and_25x`、`a_four_way_split_yields_four_files_with_continuous_rows`（拼起来等于未分割的像素）、`each_pdf_page_holds_one_slice`。
**风险**：多文件命名的**整组冲突检测**（规格 §7.3 建议）比单文件复杂 ⇒ 先做单文件命名复用，整组检测单列。
**估时**：4–6 人天。

---

## S8. Toast / 撤销 / 重命名 / 保存后动作

**测试先行**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `undo_deletes_the_file_and_rolls_the_sequence_back` | 文件消失；`[序号]` 回退 | 不存在 |
| 2 | `overwriting_puts_the_old_file_in_the_recycle_bin` | 撤销能恢复原文件（回收站 API） | 同上 |
| 3 | `rename_replaces_illegal_characters` | 与保存规则的替换一致 | 同上 |
| 4 | `the_post_save_actions_run_independently` | 「复制路径」与「打开文件夹」互不影响（规格 §3.1） | 同上 |

**估时**：3–4 人天。

---

## S9. 键盘 / UIA / 动效 / 高对比

**这一阶段大部分无法自动化** ⇒ 写**人工验收脚本**（不假装有单测 ✗）：

1. **纯键盘**：打开面板 → 逐字段 Tab → 修一个非法路径 → Enter 保存 → Esc 关闭，全程不碰鼠标；
2. **UIA**：用 Inspect.exe 确认每个控件有 `Name` / `ControlType` / 状态；错误用 `LiveRegion` 播报；
3. **动效**：规格 §11.1 的 13 条逐条看；系统「减少动画」打开后**无动效**；
4. **高对比度**：三套主题下文字与边框对比度 ≥ 4.5:1。

可自动化的部分（键盘分派表、动效时长常量）用单测钉住；其余**只承认人工结论**。

**估时**：4–6 人天。

---

## S10. 接进捕获流程

**目标**：`Destination::Save` 成为第三段；确认后打开面板；滚动路径的副本改走同一规则。

**测试先行**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `a_save_destination_opens_the_panel_and_writes_nothing_yet` | 确认后**未**写文件，面板打开 | `Destination` 无 `Save` |
| 2 | `a_cancel_in_the_panel_leaves_no_file` | 取消 ⇒ 目录无新增文件 | 同上 |
| 3 | `the_scroll_copy_uses_the_same_naming_rule` | 滚动产物文件名不再是 `SnapClip-capture-<id>` 硬编码 | `scroll.rs:567` 未改 |
| 4 | `the_scroll_artifact_is_unchanged_when_saved_as_png` | `--scroll` 产物**逐像素**不变 | 同上 |

**实现清单**

| 文件 | 内容 |
| --- | --- |
| `crates/snapclip-capture/src/region.rs:355` | `Destination` 加 `Save` |
| `crates/snapclip-capture/src/windows/overlay/session.rs:1177`（`confirm_action`） | 加 `Save` 分支：不写文件，交面板 |
| `crates/snapclip-capture/src/windows/overlay/scroll.rs:567` | 硬编码 PNG + session id 改为走 S1 的命名规则（**保留**原桌面副本作为兼容？✗ 开发阶段不考虑兼容 ⇒ 直接替换，但要确认用户可见行为变化并记录） |

**验收命令**

```bash
cargo test -p snapclip-capture --lib
cargo run -p codec-bench --release -- --assert
# 人工：F7 → 框选 → 选"保存" → 面板 → 保存 → 检查文件与 Toast
```

**风险与回退**：滚动路径的替换会**改变用户可见文件名** ⇒ 必须在提交信息与文档记明；若担心，先只加 `Save` 去向、滚动路径的替换单独一个提交。

**估时**：2–3 人天。

---

## 2. 里程碑

| 里程碑 | 包含 | 人天 | 交付物 |
| --- | --- | --- | --- |
| **M1 能保存** | S0 + S1 | 4–5 | 保存服务 + 18 条测试；`convert()` 有了生产调用者 |
| **M2 能选**（最小可用纵切） | + S2 + S3 + S10 | 13–18 | 面板可保存、可选格式、可看体积、可键盘操作 |
| **M3 能看** | + S4 | 17–23 | 预览（字节数态 + 缩放 + 网格） |
| **M4 能控** | + S5 + S6 | 24–32 | 尺寸、体积上限、高级项 |
| **M5 完整规格** | + S7 + S8 + S9 (+S4b) | 36–52 | 长图/PDF、Toast/撤销、无障碍与动效 |

---

## 3. 每个阶段的固定收尾（不可省略）

1. 该阶段的**全部**新测试先红再绿（红的原因必须是「功能不存在」）；
2. `cargo test -p snapclip-capture --lib`（默认 + `--features test-support`）；
3. `cargo test -p snapclip-app --features test-support --lib`；
4. `cargo run -p codec-bench --release -- --assert`（三条门禁）；
5. `cargo check --workspace --lib --all-targets`（0 warning）；
6. `git hook run pre-push`；
7. 一次提交（逐文件 `git add`，**不用** `git add -A`）；
8. 文档同步：本清单勾掉该阶段 + `docs/40` 的相关差距标为已闭。

---

## 4. 待测项（阻塞决策 ③）

| 项 | 手段 | 现状 |
| --- | --- | --- |
| 质量 75 / 50 的文字与渐变损失 | `codec-bench --quality`（已有矩阵 `main.rs:239`） | `[待测]` |
| 4:2:0 在真实截图上的代价 | 同上 + `docs/34 §11` 的方法 | `[已有部分]` |
| 重采样依赖（S5）、解码依赖（S4b）、PDF 依赖（S7） | 体积/许可/编译时间对比 | `[待拍板]` |
