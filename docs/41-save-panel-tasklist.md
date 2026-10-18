# 保存面板：分阶段 TDD 任务清单

> **本清单不决定产品项。** 所有产品决策都引用 `docs/42-save-panel-decisions.md` 的编号（D1–D10）。
> 若某条任务需要规格未定的产品决定，本清单只能写「阻塞于 D*」，**不得自行决定**。
> 规格仍是产品意图的权威来源；`docs/42` 是"哪些地方规格被覆盖、被延后"的正式记录。

---

## 0. 决策引用表（只引用，不重开）

| 编号 | 结论 | 对任务的影响 |
| --- | --- | --- |
| D1 | WebP 只做无损 | S6 无"无损压缩"开关；S2 只算无损体积 |
| D2 | 删除 JPG 渐进式 | S6 无此项 |
| D3 | 删除"移除元数据" | S6 无此项 |
| D4 | JPG 默认 4:4:4，4:2:0 作高级项 | S6 默认值；S2 体积按 4:4:4 |
| D5 | **解码预览必须实现** | S4 同时含字节数与解码画面；**无"可选"分支** |
| D6 | 保存后面板保持打开 `[已定]` ✓ | S3 底栏策略；S8 Toast 位置 |
| D7 | Toast 位置 `[已定]` ✓（面板内底栏上方 / 已关则右下角） | S8 |
| D8 | 质量 92/75/50，其中 75、50 `[待测]` | S2、S5 只承诺 92 |
| D9 | 单一数据源 `SaveSource` | S1 接口、S3 持有、S10 接线 |
| D10 | PDF 页尺寸 / WebP 回退 `[已定]` ✓（A4 纵向边距 0、按页宽缩放 + 按页高分页、1px=1pt 仅单页可容纳时、WebP 源走 PNG/Flate） | S7（已解闸 ✓） |

## 0.1 现状锚点（全部查证过）

| 物件 | 位置 | 状态 |
| --- | --- | --- |
| `SaveFormat{Png,Jpeg,Webp}` / `max_side` / `can_encode` / `label` | `crates/snapclip-capture/src/save_format.rs:32/117/129/145` | `[已有]` |
| `JPEG_FRAME_BUDGET_PIXELS = 16_000_000` / `within_jpeg_frame_budget` | 同文件 `:78` / `:84` | `[已有]` |
| `SavePreferences{format, jpeg_quality}` / `quality()` / 默认 92 | 同文件 `:236-239` / `:253` / `:260` | `[已有]`，**无 UI 可达** |
| `convert(master, format, quality, impl Write)` / `convert_to_file` | `apps/snapclip/src/capture/convert.rs:36` / `:135` | `[已有]`，**无生产调用者** |
| 既有转换测试 7 条 | 同文件 `:302`–`:394` | `[已有]`，可扩展 |
| JPG 行带 sink；**SOF2 断言** | `apps/snapclip/src/capture/row_band_jpeg.rs:282-288` | `[已有]`（D2 的依据） |
| `ExportSource{Whole,Bands}` / `materialise()` | `crates/snapclip-capture/src/windows/export_worker.rs:74-86` / `:143` | `[已有]`，`SaveSource` 与之同构 |
| 导出线程协议：容量 1 mailbox + generation + 过时丢弃 + `WM_APP+19` | 同文件 `:13-16` / `:224-236` / `:30-31` | `[已有]`，**复用它，不新建线程池** |
| `ScrollBands`（实现 `BandSource`） | `crates/snapclip-capture/src/scroll/mod.rs:1035` | `[已有]` |
| `Destination{Copy,Pin}` | `crates/snapclip-capture/src/region.rs:355` | `[缺]` `Save` |
| 确认分派 `confirm_action` | `crates/snapclip-capture/src/windows/overlay/session.rs:1177` | `[需改]` |
| 滚动"桌面副本"写死 `SnapClip-{session_id}.png` | `crates/snapclip-capture/src/windows/overlay/scroll.rs:567`（`copy_into`） | `[需改]` |
| 滚动前置格式检查 | `crates/snapclip-capture/src/scroll/mod.rs:129-147`、`overlay/scroll.rs:280` | `[已有]` |
| 字体子集词汇表 / 生成器 / **覆盖门禁测试** | `subfont/drawn-text.txt`、`subfont/build_subset.py`、`subfont/subset.ps1`、`crates/snapclip-capture/src/windows/win/d2d/tests.rs:1451` | `[已有]`，**新文案必须先进词汇表** |
| 现有窗口创建（单一 `CreateWindowExW`，无 EDIT 子控件） | `crates/snapclip-capture/src/windows/overlay.rs:42` | `[缺]` 原生 EDIT 宿主 |

## 0.2 平台前提（本清单的唯一技术底座）

**本项目是 Win32 + D2D 的桌面应用，不是 GPUI 应用。** 所有涉及 UI 的阶段（S3–S9）按下列事实编写，**不得**出现"复用 GPUI 的异步任务/组件"之类的前提：

- **线程模型**：面板在**自己的窗口线程**上（或现有 overlay 窗口线程上作为**模态层**），有独立的渲染目标；
- **异步**：复用导出线程的**容量 1 mailbox + 单调 generation + 过时丢弃 + `WM_APP` 通知**（`export_worker.rs:13-16/224-236/30-31`），不新建线程池；
- **绘制**：D2D（现有 `windows/win/d2d/*`）；文本**走字体子集**；
- **文本输入**：用**原生 `EDIT` 子窗口**宿主（位置/尺寸由布局驱动，样式去边框后由 D2D 画外壳）。**不自绘 DirectWrite 文本光标** ✗ —— IME、选区、撤销、复制粘贴、UI Automation 都由系统提供，自绘要重做这五样且极易出错；
- **模态焦点**：面板打开时用 `EnableWindow(parent, false)` 或在宿主侧维护焦点栈，`Esc` / `Tab` 由面板的消息循环处理；子控件之间用 `IsDialogMessage` 或显式 `Tab` 处理。

## 0.3 S0 必须就位的文案（逐字，来自规格）

**校验 11 条**（语义实现在 S3；S0 只保证字形能渲染）：

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

**冲突与 Toast**：`改为覆盖`、`改为重命名`；Toast 四操作在 S8 实现时逐字从规格 §7.6 摘录。

> **纪律**：S0 只负责"字形就位"，不负责语义。S0 的验收是**门禁测试转绿**，不是肉眼检查。

---

## 1. 阶段总表

| 阶段 | 内容 | 人天 | 出口 |
| --- | --- | ---: | --- |
| S0 | 文案字形 + 基线确认 | 1 | 覆盖门禁测试绿 |
| S1 | 保存服务（`SaveSource`，无 UI） | 3–4 | 18 条服务测试绿；PNG 字节相同 |
| S2 | 编码服务（字节数 + 可解码字节） | 2–3 | 体积与实际文件一致；代际取消 |
| S3 | 模态表单骨架（Win32 窗口 + 原生 EDIT） | 6–9 | 11 条校验逐条可见；真实窗口测试绿 |
| **S4** | **预览：字节数 + 解码画面 + 对比滑块** | 7–9 | 解码画面与产物逐像素一致 |
| S5 | 尺寸缩放 + 限制体积 | 4–5 | 结果 ≤ 上限（无容差） |
| S6 | 高级选项（按 D1–D4 裁剪） | 3–4 | 每项生效且可恢复默认 |
| S7 | 长截图分割 + PDF | **已解闸** ✓（D10 已定），估时待细化 ✗ | —— |
| S8 | Toast / 撤销 / 重命名 / 保存后动作 | 4–5 | 撤销与回滚在失败注入下正确 |
| S9 | 键盘 / UIA / 动效 / 高对比 | 4–6 | 人工脚本 + 可自动化部分单测 |
| S10 | 接进捕获流程（`Destination::Save`） | 2–3 | 确认后不写文件；滚动走同一命名规则 |

**总人天（不含 S7）**：**36–49**。S7 已因 D10 定稿而**解除实施闸门** ✓，但**估时仍待细化** ✗（切分逻辑与 PDF 排版可共用，具体人天在 S7 展开测试清单后再给，**不给承诺值**）。

---

## S0. 前置：文案字形 + 基线确认

**目标**：11 条校验文案与冲突/Toast 文案全部能被字体子集渲染；现有保存/转换测试全绿作为基线。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `the_embedded_subset_draws_every_new_save_panel_character` | `windows/win/d2d/tests.rs:1451` 的覆盖门禁对新词表全绿 | 词表未加 |
| 2 | `the_existing_conversion_suite_still_passes` | `convert.rs` 的 7 条测试全绿（基线） | —— （用于确认没破坏） |

**实现清单**：`subfont/drawn-text.txt` 增补文案字符 → 跑 `subfont/subset.ps1` → 门禁绿。

**验收命令**

```bash
powershell -NoProfile -ExecutionPolicy Bypass -File subfont/subset.ps1
cargo test -p snapclip-capture --lib windows::win::d2d
```

**风险与回退**：字体重建可能改变现有字形度量 ⇒ 以门禁 + 现有 UI 测试为准；若红，分批加入。

**估时**：1 人天。

---

## S1. 保存服务（`SaveSource`，无 UI）

**目标**：给定 `SaveSource` + `SaveRequest`，把像素写成目标文件并返回最终路径；全程校验、失败语义明确、原子提交。**`SaveSource` 承载数据源，服务本身不持有像素的所有权**。

**接口（登记在此，实施时可微调但需回改本表）**

```rust
pub fn save(source: &mut SaveSource, request: &SaveRequest) -> Result<SaveOutcome, SaveError>;

pub struct SaveRequest {
    pub directory: PathBuf,
    pub template: Template,
    pub sequence: u32,
    pub format: SaveFormat,
    pub quality: u8,
    pub advanced: AdvancedOptions,   // S6 才填满；S1 先只有默认值
    pub conflict: ConflictPolicy,    // 默认 AutoRename
}

pub struct SaveOutcome { pub path: PathBuf, pub bytes: u64, pub format: SaveFormat }
```

- `SaveSource::Whole` / `Written`：读像素（`Written` 直接读文件）；
- `SaveSource::Bands`：**按行回放**喂行带 sink ⇒ **不物化整图**（D9 的硬约束）；
- 编码结果落 `.partial` ⇒ rename；**`.partial` 不得残留**。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `a_png_request_copies_the_master_byte_for_byte` | 目标文件与 `Written` 母版字节相同 | 服务不存在 |
| 2 | `a_banded_png_request_matches_the_whole_png_byte_for_byte` | `Bands` 与 `Whole` 产出的 PNG **字节相同**（D9：不物化也不改像素） | 同上 |
| 3 | `a_jpeg_request_produces_a_readable_jpeg_of_the_source_size` | 解码宽高 = 源宽高 | 同上 |
| 4 | `a_webp_request_produces_a_lossless_webp` | 解码后逐像素等于源（D1） | 同上 |
| 5 | `a_second_save_of_the_same_name_becomes_underscore_two` | `名_2.PNG`；第一个仍在 | 同上 |
| 6 | `overwriting_a_name_replaces_it_and_no_partial_survives` | 只剩一个文件；无 `.partial` | 同上 |
| 7 | `the_template_expands_every_variable_the_spec_defines` | 5 个变量各一个用例 | 变量解析不存在 |
| 8 | `an_unknown_variable_is_an_error_not_literal_text` | `Err(InvalidTemplate)`，不产出 | 同上 |
| 9 | `an_empty_or_relative_directory_is_refused` | `PathEmpty` / `PathNotAbsolute` | 同上 |
| 10 | `a_missing_directory_is_created_on_save` | 保存成功且目录被创建 | 同上 |
| 11 | `a_read_only_directory_reports_no_permission` | `Err(NoPermission)`，无文件无 `.partial` | 同上 |
| 12 | `a_failed_write_is_reported_and_leaves_nothing` | 注入写失败 ⇒ `Err(NotEnoughSpace{..})`，无残件 | 同上 |
| 13 | `a_name_with_an_illegal_character_reports_the_character` | `Err(IllegalName(char))` 带该字符 | 同上 |
| 14 | `a_reserved_windows_name_is_refused` | `CON` / `NUL` / `COM1` ⇒ `ReservedName` | 同上 |
| 15 | `a_name_ending_in_a_dot_or_space_is_refused` | `TrailingDotOrSpace` | 同上 |
| 16 | `a_path_longer_than_260_is_refused` | `PathTooLong` | 同上 |
| 17 | `a_jpeg_request_beyond_the_budget_is_refused_before_any_row_is_read` | 超 16 MP ⇒ `FormatTooLarge`，且**未回放任何行**（用计数断言） | 同上 |
| 18 | `a_webp_request_beyond_16383_refuses` | 宽或高 > 16383 ⇒ `FormatTooLarge` | 同上 |
| 19 | `the_sequence_advances_only_after_a_successful_save` | 成功 +1；失败不变 | 同上 |
| 20 | `cancelling_midway_leaves_no_file_and_no_partial` | 取消源回放 ⇒ 无文件、无 `.partial` | 同上 |

**实现清单**

| 文件 | 内容 |
| --- | --- |
| `apps/snapclip/src/save/mod.rs`（新） | `SaveRequest` / `SaveOutcome` / `SaveError` / `SaveSource` 门面 |
| `apps/snapclip/src/save/source.rs`（新） | `SaveSource` 三支 + 行回放适配（复用 `BandSource`） |
| `apps/snapclip/src/save/template.rs`（新） | `Template::parse` / `expand` |
| `apps/snapclip/src/save/validate.rs`（新） | 11 条校验（纯函数，不产出文件） |
| `apps/snapclip/src/save/conflict.rs`（新） | `resolve(dir, stem, ext, policy)`；不区分大小写 |
| `apps/snapclip/src/save/service.rs`（新） | 校验 → 冲突 → 编码（`Written`/`Whole` 走 `convert_to_file`；`Bands` 走 `write_bands`）→ 返回 |
| `crates/snapclip-capture/src/save_format.rs` | 上限判据复用 `can_encode` / `within_jpeg_frame_budget` |

**验收命令**

```bash
cargo test -p snapclip-app --lib save::
cargo run -p codec-bench --release -- --assert
git hook run pre-push
```

**风险与回退**：磁盘满不可真造 ⇒ 注入失败 `Write`；`[序号]` 持久化不在 S1（只接收参数）。

**估时**：3–4 人天。

---

## S2. 编码服务（字节数 + 可解码字节）

**目标**：一次编码同时产出 ①编码后字节数 ②**可解码的字节**（S4 的解码预览要用，D5）；带防抖、代际取消、缓存。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `a_lower_jpeg_quality_never_increases_the_byte_count` | q92、q75、q50 的字节数**非递增**（**不断言严格单调** ✗：平坦区域可能相等） | 服务不存在 |
| 2 | `a_cached_request_encodes_nothing` | 同一 key 第二次 ⇒ `encodes == 0` | 同上 |
| 3 | `changing_an_advanced_option_invalidates_the_cache` | 键含格式/质量/高级项/源摘要 | 同上 |
| 4 | `a_newer_request_supersedes_the_older_one` | 旧代次结果被丢弃（复用 export worker 协议） | 同上 |
| 5 | `the_reported_size_equals_the_file_that_saving_writes` | 预估字节数 == S1 实际文件大小 | 同上 |
| 6 | `the_produced_bytes_decode_to_the_source_pixels` | PNG/WebP 解码后逐像素等于源；JPG 允许有损但尺寸一致 | 同上 |

**实现清单**：`save/encode.rs`（复用 `convert()` 的 `impl Write` 到内存 sink ⇒ 同时得到字节数与字节）、`save/cache.rs`（key + LRU）、`save/generation.rs`（复用导出线程的 generation 语义，不自建线程池）。

**验收命令**：`cargo test -p snapclip-app --lib save::encode save::cache`

**风险与回退**：大图编码会占 CPU ⇒ 必须在后台线程（导出线程），面板只收结果。

**估时**：2–3 人天。

---

## S3. 模态表单骨架（Win32 + 原生 EDIT）

**目标**：920 宽的模态面板：路径 / 模板 / 格式 / 质量 / 冲突 / 底栏；11 条校验逐条可见；键盘可达；**数据源由 `PendingCapture` 持有**。

**平台实现（按 §0.2）**：独立窗口线程或宿主模态层；D2D 绘制外壳；文本字段 = **原生 `EDIT` 子窗口**（去边框、字号由字体子集渲染、`EM_*` 消息处理 IME/选区/撤销）；模态焦点用 `EnableWindow` + 焦点栈；`Tab`/`Esc`/`Enter` 由面板消息循环处理。

**测试先行清单**

| # | 测试名 | 断言 | 层次 | 先红原因 |
| --- | --- | --- | --- | --- |
| 1 | `the_form_reports_each_validation_rule_without_saving` | 11 条规则各一个非法状态 ⇒ 状态行逐字等于 §0.3 | 单测（纯状态机） | 表单不存在 |
| 2 | `the_save_button_is_disabled_while_an_error_stands` | error ⇒ 禁用；只有 warn ⇒ 可用 | 单测 | 同上 |
| 3 | `the_warning_offers_the_repair_it_names` | "立即创建"可点且真的创建目录 | 单测 + 临时目录 | 同上 |
| 4 | `jpeg_is_disabled_for_a_scroll_source_above_the_budget` | 超 16 MP 的 `Bands` 源 ⇒ JPG 项禁用并给出原因 | 单测 | 上限未接 UI |
| 5 | `the_sequence_previews_the_next_number` | 显示下一个值；成功后自增 | 单测 | 未接 |
| 6 | `escape_cancels_and_enter_saves` | 键盘分派表 | 单测（分派）+ 真实窗口（见下） | 未接 |
| 7 | `the_quality_row_is_absent_for_png_and_webp` | D1：WebP 也无质量档 | 单测 | 未接 |
| 8 | `a_pending_capture_is_dropped_when_the_panel_closes` | 关闭 ⇒ `PendingCapture` 释放；`Bands` 的 tile 文件被删 | **真实窗口测试** | 生命周期未接 |
| 9 | `the_edit_control_accepts_and_reports_ime_composition` | 原生 EDIT 收到 `WM_IME_*` 后文本正确 | **真实窗口测试**（可能需要注入 IME 消息） | 无 EDIT 宿主 |
| 10 | `focus_stays_inside_the_modal_panel` | `Tab` 循环不出面板；父窗口被禁用 | **真实窗口测试** | 未接 |

> **测试层次纪律**：1–7 是纯状态机单测（不需窗口）；8–10 必须**真开窗口**（`CreateWindowExW` + `SendMessage` 驱动），**不得**降级为人工验收。窗口测试在 CI 里可能因桌面会话受限而跳过 —— 跳过必须写成 `#[ignore = "needs an interactive desktop"]` 并登记，**不许静默 skip**。

**实现清单**

| 文件 | 内容 |
| --- | --- |
| `apps/snapclip/src/ui/save_panel/mod.rs`（新） | 状态机（字段、派生量、校验状态） |
| `apps/snapclip/src/ui/save_panel/window.rs`（新） | 窗口类注册、消息循环、模态焦点、`WM_APP` 处理 |
| `apps/snapclip/src/ui/save_panel/edit_host.rs`（新） | 原生 `EDIT` 子控件宿主（创建/移动/取值/IME/选中） |
| `apps/snapclip/src/ui/save_panel/layout.rs`（新） | 布局（规格 §11.3 token） |
| `apps/snapclip/src/ui/save_panel/draw.rs`（新） | D2D 外壳与状态行（文本走字体子集） |
| `apps/snapclip/src/save/pending.rs`（新） | `PendingCapture`（持有 `SaveSource` + Drop 清理） |
| `crates/snapclip-capture/src/preferences.rs` | 持久化：模板/序号/路径/格式/质量/限制体积/保存后动作 |

**验收命令**

```bash
cargo test -p snapclip-app --lib ui::save_panel
cargo test -p snapclip-app --features test-support --lib ui::save_panel::window
cargo build --release
```

**风险与回退**：原生 EDIT 的样式与 D2D 外壳的贴合是主要风险 ⇒ 先接受"EDIT 外观略朴素（去边框 + 底色一致）"，不追求像素级一致；如 EDIT 与 D2D 混排不可接受，再评估自绘（但那要接受 IME/UIA 的代价，需重新决策）。

**估时**：6–9 人天。

---

## S4. 预览：字节数 + 解码画面 + 对比滑块（D5：**不可选**）

**目标**：左栏显示**真实产物**：①编码后字节数 ②**编码后再解码**的画面 ③对比滑块（有损格式）；含缩放/网格/尺寸标签/背景。

**前置**：JPG/WebP **解码器**。选型要在本阶段开头完成并登记（候选：小而专的解码 crate；`image` 因体积被否）。解码器只用于**预览**，不进入保存路径。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `the_decoded_preview_matches_the_encoded_bytes` | 预览用的像素 == 对编码字节解码的结果（逐像素） | 预览不存在 |
| 2 | `a_lossy_preview_differs_from_the_source_where_the_encoder_lost` | JPG q50 下与源有差异；PNG 下无差异 | 同上 |
| 3 | `the_comparison_slider_reveals_encoded_on_the_right` | 滑块右半取解码结果、左半取源 | 同上 |
| 4 | `zoom_clamps_between_five_and_eight_hundred_percent` | 5%/800% 边界 | 同上 |
| 5 | `the_pixel_grid_appears_only_at_or_above_four_hundred_percent` | 399 无、400 有 | 同上 |
| 6 | `the_size_label_reads_output_and_original` | 文案与规格一致 | 同上 |
| 7 | `the_preview_background_cycles_all_four_options` | 四选一循环 | 同上 |
| 8 | `a_parameter_change_keeps_the_old_preview_until_the_new_one_is_ready` | 旧结果保留、新结果就绪后替换（防闪烁，规格 §3） | 同上 |

**实现清单**：`ui/save_panel/preview.rs`（变换、脏区、网格、背景）、`save/decode.rs`（解码器封装）、`save/compare.rs`（滑块合成）。

**验收命令**：`cargo test -p snapclip-app --lib ui::save_panel::preview save::decode`

**风险与回退**：解码依赖的编译时间/许可是唯一硬风险 ⇒ 若不可接受，**必须回到 D5 重新决策**（不是默默降级 ✗）。

**估时**：7–9 人天（含原 S4b）。

---

## S5. 尺寸缩放 + 限制体积

**目标**：数值/百分比/锁比/预设；"限制体积"用质量搜索。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `a_locked_ratio_keeps_the_aspect_when_one_side_changes` | 取整规则一致 | 不存在 |
| 2 | `an_out_of_range_size_is_clamped_with_the_spec_message` | 文案一致 | 同上 |
| 3 | `the_written_file_never_exceeds_the_limit` | **产物字节 ≤ 上限**（**无 1.03 容差** ✗；3% 只允许出现在搜索/估算里） | 同上 |
| 4 | `the_search_lands_on_the_highest_quality_tier_that_fits` | 选中的是满足上限的最高档 | 同上 |
| 5 | `when_no_tier_fits_the_lowest_is_used_and_the_limit_is_reported_unmet` | 状态行给出"仍超出"与"自动缩小尺寸" | 同上 |
| 6 | `a_manual_quality_choice_leaves_automatic_mode` | 手动选档 ⇒ 退出自动 | 同上 |
| 7 | `the_search_takes_at_most_seven_encodes` | 编码次数上界 | 同上 |

**实现清单**：`save/resize.rs`（重采样依赖需先评估并登记）、`save/budget.rs`（搜索 + 取消）。

**估时**：4–5 人天。

---

## S6. 高级选项（按 D1–D4 裁剪）

**内容**：PNG 压缩等级（已有，接出去）；PNG 色板量化（规格 §6.2，需评估）；JPG 填充色；JPG 色度采样（**默认 4:4:4**，4:2:0 高级项并标注"文字更糊"）。**不做**：WebP 无损开关（D1）、渐进式（D2）、移除元数据（D3）。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `the_png_compression_level_changes_bytes_but_not_pixels` | 三档字节不同、解码像素相同 | 未接 |
| 2 | `the_jpeg_fill_colour_appears_where_transparency_was` | 透明区域变填充色 | 未接 |
| 3 | `the_default_subsampling_is_444` | 默认产物的色度采样是 4:4:4（D4） | 默认未改 |
| 4 | `switching_to_420_shrinks_the_file_and_costs_text_quality` | 字节更少；并记录文字 PSNR 代价（引用 `docs/34 §11`） | 未接 |
| 5 | `a_non_default_option_shows_a_dot_and_can_be_reset` | 蓝点 + 恢复默认 | 未接 |

**估时**：3–4 人天。

---

## S7. 长截图分割 + PDF

**状态**：D10 **已定** ✓ ⇒ **解除实施闸门** ✓。估时**仍待细化** ✗（不给承诺值 ✓）。

**已定的规则（D10）**：

- **页尺寸 A4 纵向、边距 0** ✓；图像**按页宽缩放**（保持纵横比 ✓）、**按页高分页** ✓；
- **1 px = 1 pt 只在单页可容纳时成立** ✓ —— PDF 单边上限 **14 400 pt**（≈200 英寸 ✓），80 882 px 在 1:1 下远超上限 ✗ ⇒ 必须缩放 + 分页；
- **WebP 源 ⇒ PNG/Flate** ✓（PDF 不支持 WebP ✗）；**JPG 源 ⇒ 嵌 JPEG** ✓；**PNG 源 ⇒ Flate** ✓；
- **分页与分片共用同一套切分逻辑** ✓（都按"页高/片高"切 ✓）。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `a_four_way_split_yields_four_files_with_continuous_rows` | 四片首尾相接，拼起来与未分割像素等价 | 分割功能不存在 |
| 2 | `the_split_scales_the_image_to_the_page_width` | 图片宽度映射到 A4 页宽（保持纵横比），页内不裁切 | 排版不存在 |
| 3 | `a_long_image_spans_several_pages_by_page_height` | 页数 = ⌈缩放后高 / 页高⌉，逐页内容连续 | 同上 |
| 4 | `a_webp_source_is_embedded_as_flate` | 源为 WebP 时页内流是 Flate（无损），不是 WebP | 同上 |
| 5 | `one_slice_failing_leaves_no_partial_group` | 任一切片失败 ⇒ 整组不提交、已写的回滚删除 | 同上 |
| 6 | `a_group_conflict_is_decided_for_the_whole_group` | 多文件冲突整组一致（不能只看第一个文件名） | 同上 |
| 7 | `a_pdf_write_failure_rolls_back_every_slice` | PDF 写入失败 ⇒ 切片与 `.partial` 全部清理 | 同上 |

**其余必须覆盖的失败路径**：`.partial` 清理 ✓、覆盖模式下旧文件可恢复 ✓、整组冲突检测 ✓。

---

## S8. Toast / 撤销 / 重命名 / 保存后动作

**目标**：Toast（位置按 D7）、撤销（删除文件 + 序号回退 + 回收站恢复）、重命名、保存后动作（复制路径 / 打开文件夹）。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `undo_deletes_the_file_and_rolls_the_sequence_back` | 文件消失；序号回退 | 不存在 |
| 2 | `undo_restores_the_replaced_file_from_the_recycle_bin` | 覆盖后撤销 ⇒ 原文件可读 | 同上 |
| 3 | `a_recycle_bin_failure_reports_and_keeps_the_new_file` | 回收站 API 失败 ⇒ 明确错误，不静默吞掉 | 同上 |
| 4 | `a_restore_failure_reports_and_does_not_delete_the_new_file` | 恢复失败 ⇒ 新文件保留，状态可见 | 同上 |
| 5 | `an_exit_before_undo_finishes_leaves_a_consistent_state` | 撤销是"先标记后执行"还是"原子"必须有定义，测试钉住 | 同上 |
| 6 | `rename_replaces_illegal_characters` | 与保存规则一致 | 同上 |
| 7 | `the_post_save_actions_run_independently` | 复制路径与打开文件夹互不影响 | 同上 |
| 8 | `the_toast_sits_above_the_footer_and_never_covers_the_size_label` | 位置（D7） | 同上 |

**估时**：4–5 人天。

---

## S9. 键盘 / UIA / 动效 / 高对比

**可自动化**（单测钉住）：键盘分派表、动效时长与曲线常量、"减少动画"开关的选择逻辑。

**真实窗口测试**（不得只靠人工）：`Tab` 顺序、`Esc`、`Enter`、`Ctrl+C`、焦点不出面板、错误用 `LiveRegion` 属性。

**只能人工**（写明并只承认人工结论）：13 条动效的观感、三套高对比主题下 ≥4.5:1 的对比度、UIA 树的语义是否可理解（用 Inspect.exe）。

**估时**：4–6 人天。

---

## S10. 接进捕获流程（`Destination::Save`）

**目标**：`Save` 成为第三段去向；确认后**打开面板但不写文件**；滚动路径的副本改走同一规则；数据源由 `PendingCapture` 交给面板。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `a_save_destination_opens_the_panel_and_writes_nothing_yet` | 确认后目录无新增文件 | 无 `Save` |
| 2 | `cancelling_the_panel_leaves_no_file_and_no_tile` | 无文件；滚动 tile 已删 | 生命周期未接 |
| 3 | `saving_a_whole_capture_writes_the_master_byte_for_byte` | 普通截图 PNG 字节相同 | 未接 |
| 4 | `saving_a_banded_capture_never_materialises_the_whole_image` | `Bands` 路径的全图物化次数为 0（用 S1 的计数断言） | 未接 |
| 5 | `the_scroll_copy_uses_the_same_naming_rule` | 文件名不再是 `SnapClip-capture-<id>` 硬编码 | `scroll.rs:567` 未改 |
| 6 | `a_scroll_saved_as_png_is_pixel_identical_to_the_old_artifact` | 逐像素不变 | 同上 |

**实现清单**

| 文件 | 内容 |
| --- | --- |
| `crates/snapclip-capture/src/region.rs:355` | `Destination` 加 `Save` |
| `crates/snapclip-capture/src/windows/overlay/session.rs:1177` | `confirm_action` 加 `Save` 分支：构造 `PendingCapture`，不写盘 |
| `crates/snapclip-capture/src/windows/overlay/scroll.rs:567` | 硬编码 `copy_into` 改为走 S1 命名规则（开发阶段不考虑兼容 ⇒ 直接替换，**并在提交信息里写明用户可见变化**） |
| `apps/snapclip/src/ui/save_panel/*` | 接收 `PendingCapture` |

**验收命令**

```bash
cargo test -p snapclip-capture --lib
cargo test -p snapclip-app --features test-support --lib
cargo run -p codec-bench --release -- --assert
```

**估时**：2–3 人天。

---

## 2. 里程碑

| 里程碑 | 包含 | 人天 | 说明 |
| --- | --- | ---: | --- |
| **M1 能保存** | S0 + S1 | 4–5 | 保存服务 + 20 条测试；`convert()` 有生产调用者 |
| **M2 面板可用** | + S2 + S3 | 12–17 | 能选格式、看体积、键盘可用、11 条校验可见 |
| **M3 所见即所存** | + S4 | 19–26 | 预览显示**真实解码结果**（D5），对比滑块可用 |
| **M4 能控** | + S5 + S6 | 26–35 | 尺寸、体积上限、高级项 |
| **M5 完整** | + S8 + S9 + S10 | 36–49（不含 S7） | **S7 已解闸** ✓，其估时待细化 ✗ |

> **D5 的纪律**：M2 **不是**"预览已完成"的里程碑 —— 它只有字节数。**M3 才算"所见即所存"**，任何文档/汇报不得把 M2 说成预览已完成 ✗。

---

## 3. 每个阶段的固定收尾（不可省略）

1. 该阶段全部新测试**先红再绿**（红的原因必须是"功能不存在"）；
2. `cargo test -p snapclip-capture --lib`（默认 + `--features test-support`）；
3. `cargo test -p snapclip-app --features test-support --lib`；
4. `cargo run -p codec-bench --release -- --assert`；
5. `cargo check --workspace --lib --all-targets`（0 warning）；
6. `git hook run pre-push`；
7. 一次提交（逐文件 `git add`，**不用** `git add -A`）；
8. 文档同步：勾掉本阶段 + 若产生新决策，写进 `docs/42` 并回填编号。

---

## 4. 待测与待定（阻塞实施）

| 项 | 手段 | 阻塞谁 |
| --- | --- | --- |
| 质量 75 / 50 的文字与渐变损失（D8） | `codec-bench --quality` 三类内容 | S5 的档位搜索、S6 质量行 |
| 4:2:0 在真实截图上的代价 | 同上 + `docs/34 §11` 方法 | S6 的默认值说明 |
| PDF 页尺寸 / WebP 回退（D10） | **已定** ✓ | S7（解闸 ✓，估时待细化 ✗） |
| 解码依赖选型（S4） | 体积/许可/编译时间对比 | S4 |
| 重采样依赖选型（S5） | 同上 | S5 的缩放部分 |
| 保存后是否关窗（D6）/ Toast 位置（D7） | 用户拍板 | S3 底栏、S8 位置 |
