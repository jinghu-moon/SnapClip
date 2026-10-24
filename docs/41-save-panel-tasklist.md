# 保存面板：分阶段 TDD 任务清单

> **本清单不决定产品项。** 所有产品决策都引用 `docs/42-save-panel-decisions.md` 的编号（D1–D11）。
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
| D6 | 保存后面板保持打开 `[已定]` ✓ | S3 底栏区域；S5 保存操作；S8 Toast 位置 |
| D7 | Toast 位置 `[已定]` ✓（面板内底栏上方 / 已关则右下角） | S8 |
| D8 | 质量 92/75/50，其中 75、50 `[待测]` | S2、S6 只承诺 92 |
| D9 | 单一文件型 `EncodedArtifact` 服务预览、复制、拖出与保存 | S1/S2 共用编码结果；保存不重编码 |
| D10 | PDF 页尺寸 / WebP 回退 `[已定]` ✓（A4 纵向边距 0、按页宽缩放 + 按页高分页、1px=1pt 仅单页可容纳时、WebP 源走 PNG/Flate） | S7（已解闸 ✓） |
| D11 | 文本输入使用**原生 `EDIT`** + 独立普通窗口 + DWM backdrop，外壳自绘 | S5 做窗口、外壳、原生字段与校验；S9 验 UIA 由系统提供 |

## 0.4 UI 保真要求（用户明示，常驻）

> 保存面板的视觉与交互必须**尽可能与 `docs/design/save-panel-demo-v3.html` 一致**，**扣除 docs/42 D13 记录的三条不可复现项**（圆角 12→系统≈8 dip、投影改用 DWM、模糊不可调）。
> 任何"与 demo 不一致"的取舍都必须**写明理由**（例如原生 `EDIT` 的方角由外框遮住），不得默默偏离。

验收分两档：

| 档 | 内容 | 方式 |
| --- | --- | --- |
| **可自动化** | 颜色、圆角、间距、字号、状态样式（default/hover/focused/error/warning） | **离屏 D2D 渲染 → 位图比对**（装置已有 `render_view` / `pixel_at` / `assert_pixel_close`） |
| **只能人判** | 整体观感、动效节奏、文案语气 | **人工验收**，对着 demo **并排**看 |

- **S1 是纯服务（无 UI）**，本要求自 **S2/S3** 起生效；但 S1 里定稿的**文案**（11 条校验）属于 UI 的一部分，且必须进字体子集白名单。
- 位图比对断言的是"与 demo 的**已定稿状态**一致"，因此 demo 每次改版都要同步更新期望位图——否则断言会保护一个过时的设计。

---
## 0.1 现状锚点（全部查证过）

| 物件 | 位置 | 状态 |
| --- | --- | --- |
| `SaveFormat{Png,Jpeg,Webp}` / `max_side` / `can_encode` / `label` | `crates/snapclip-capture/src/save_format.rs:32/117/129/145` | `[已有]` |
| `JPEG_FRAME_BUDGET_PIXELS = 16_000_000` / `within_jpeg_frame_budget` | 同文件 `:78` / `:84` | `[已有]` |
| `SavePreferences{format, jpeg_quality}` / `quality()` / 默认 92 | 同文件 `:236-239` / `:253` / `:260` | `[已有]`，**无 UI 可达** |
| `convert(master, format, quality, impl Write)` / `convert_to_file` | `apps/snapclip/src/capture/convert.rs:36` / `:135` | `[已有]`，**无生产调用者** |
| 既有转换测试 7 条 | 同文件 `:302`–`:394` | `[已有]`，可扩展 |
| JPG 行带 sink；**SOF2 断言** | `apps/snapclip/src/capture/row_band_jpeg.rs:282-288` | `[已有]`（D2 的依据） |
| `ExportSource{Whole,Bands}` / `materialise()` | `crates/snapclip-capture/src/windows/export_worker.rs:74-86` / `:143` | `[已有]`，但 crate-private；保存源需由 capture crate 提供 opaque 所有权 API |
| 导出线程协议：容量 1 mailbox + generation + 过时丢弃 + `WM_APP+19` | 同文件 `:13-16` / `:224-236` / `:30-31` | `[已有]`，**复用它，不新建线程池** |
| `ScrollBands`（实现 `BandSource`） | `crates/snapclip-capture/src/scroll/mod.rs:1035` | `[已有]`，`pub(crate)`；app 不得直接持有 |
| `Destination{Copy,Pin}` | `crates/snapclip-capture/src/region.rs:355` | `[缺]` `Save` |
| 确认分派 `confirm_action` | `crates/snapclip-capture/src/windows/overlay/session.rs:1177` | `[需改]` |
| 滚动"桌面副本"写死 `SnapClip-{session_id}.png` | `crates/snapclip-capture/src/windows/overlay/scroll.rs:567`（`copy_into`） | `[需改]` |
| 滚动前置格式检查 | `crates/snapclip-capture/src/scroll/mod.rs:129-147`、`overlay/scroll.rs:280` | `[已有]` |
| 字体子集词汇表 / 生成器 / **覆盖门禁测试** | `subfont/drawn-text.txt`、`subfont/build_subset.py`、`subfont/subset.ps1`、`crates/snapclip-capture/src/windows/win/d2d/tests.rs:1451` | `[已有]`，**新文案必须先进词汇表** |
| 现有窗口创建（单一 `CreateWindowExW`） | `crates/snapclip-capture/src/windows/overlay.rs:42` | `[已有]`；保存面板需新增 D2D 自绘输入控件，不创建 EDIT 子窗口 |

## 0.2 平台前提（本清单的唯一技术底座）

**本项目是 Win32 + D2D 的桌面应用，不是 GPUI 应用。** 所有涉及 UI 的阶段（S0、S3–S9）按下列事实编写，**不得**出现"复用 GPUI 的异步任务/组件"之类的前提：

- **线程模型**：面板在**自己的窗口线程**上（**独立普通窗口** ✓，不与截图浮层共享线程/渲染目标 ✗ —— 截图框此时已消失 ✓），有独立的渲染目标与 DWM backdrop ✓；
- **异步**：复用导出线程的**容量 1 mailbox + 单调 generation + 过时丢弃 + `WM_APP` 通知**（`export_worker.rs:13-16/224-236/30-31`），不新建线程池；
- **绘制**：D2D（现有 `windows/win/d2d/*`）；文本**走字体子集**；
- **文本输入（D11）**：字段用**原生 `EDIT` 子控件** ✓（独立普通窗口 ✓）⇒ 组合输入/候选窗/插入点/选区/拖选/撤销/复制粘贴/键盘导航/**UIA** 全部由系统提供 ✓；外壳（圆角外框、悬停/聚焦描边）由 D2D 画 ✓，`EDIT` 内缩 1–2 dip ✓。**禁止**再实现自绘 TextField / IMM32 / TSF ✗；
- **模态焦点**：面板打开时用 `EnableWindow(parent, false)` 或在宿主侧维护焦点栈，`Esc` / `Tab` 由面板的消息循环处理；子控件之间用 `IsDialogMessage` 或显式 `Tab` 处理。

## 0.2.1 现有自绘控件基线（必须复用）

项目已经有一套运行中的自定义截图面板，不能把保存面板当成从零开始的第二套 D2D 控件系统。当前实现的职责边界如下：

| 现有能力 | 代码位置 | 对保存面板的处理 |
| --- | --- | --- |
| D2D/DWrite 文本绘制、测量、等宽数字、灰度抗锯齿 | `crates/snapclip-capture/src/windows/win/d2d.rs`：`draw_text_in`、`measure_text_width`、`info_text_format_mut` | 抽为共享绘制服务；保存面板不得再写一套文本测量和基线算法 |
| 面板 token 与画刷：`fill`、`line`、`accent`、`muted`、`danger`，圆角填充/描边 | 同文件：`fill_round`、`stroke_round`、`panel_*_brush` | 作为共享 `PanelTheme` / `PanelPainter`；颜色和圆角以规格 token 为准 |
| 分段控件：轨道、选中块、悬停、焦点环、滑动过渡 | 同文件：`draw_segmented` | 保存面板的格式、质量、尺寸模式、保存后动作优先复用 |
| 图标缓存与着色绘制 | 同文件：`draw_glyph`、`panel_icon.rs` | 复用图标缓存和 DPI 缩放；保存面板只增加缺失图标 |
| 布局矩形与命中测试共用同一份数据 | `geometry.rs`：`RegionPanelLayout`、`RegionPanelTarget::target_at` | 保存面板采用同样的“layout 是命中测试唯一来源”规则 |
| 焦点顺序、编辑缓冲、键盘分派、`PanelChange` | `region.rs`：`PanelSlot::ORDER`、`RegionPanel`、`PanelKey`、`PanelChange`；`overlay/input.rs` 负责分派 | 复用状态机模式和测试方式；不复用截图业务字段 |
| 视图快照与渲染分离 | `overlay/render_submit.rs`：`region_panel_view` → `RegionPanelView` → `draw_region_panel` | 保存面板也采用“状态计算 → 不可变 View → 绘制”的调用链 |

**抽取边界**：

1. 抽取已有通用控件与绘制能力：`PanelTheme`、`TextLayout`/测量、`SegmentedControl`、`IconButton`、圆角/焦点/悬停状态、布局与命中测试工具；TextField 编辑核心在 S5 新增，但必须依赖这些能力。
2. 保留领域状态：截图面板的 `RegionPanel`、选区联动、单位/延时/圆角和 `PanelChange::Region` 不进入保存面板；保存面板另建自己的状态机。
3. 现有 `RegionPanel` 的输入入口只接受数字（`panel_key_from_char`）✗，且它是**浮层内自绘**的 ✓；保存面板按 D11 使用**原生 `EDIT`** ✓，两者不共用输入实现 ✓（D11 改变后这条不再是“要自研 TextField”的理由 ✓）。
4. 共享模块的归属应与依赖方向一致。若保存面板继续位于 `snapclip-app`，通用控件应移入独立的 `snapclip-ui` Windows/D2D crate；不得让 app 访问 `snapclip-capture` 的 `pub(crate)` 绘制实现，也不得让 capture 依赖 app。只有在两者合并到同一 UI crate 后，才保留模块内 `pub(crate)`。

**抽取验收**：先把现有自定义截图面板迁移到共享控件并保持现有 `geometry.rs` / `region.rs` / D2D 回归测试全绿，再让保存面板消费同一控件。迁移前后应比较分段器、数字字段插入点、焦点环、悬停态、DPI 缩放和命中测试；不能只验证保存面板而放弃截图面板回归。

## 0.3 S0 必须就位的文案（逐字，来自规格）

**唯一来源是代码，不是这张表** ✓：`crates/snapclip-ui/src/copy.rs`（`ValidationCopy` / `ConflictLink`）。
本表只是索引；改文案改代码，并让 `every_message_appears_verbatim_in_the_spec` 继续绿 ——
那条测试**直接读规格文件**断言逐字存在，所以文档、代码、规格三者不会再各自漂移 ✗。

**校验 11 条**（以规格 §8.2 + §8.3 为准 ✓；`docs/41` 早先的代码块把 11 条与修复动作拍平成 13 行，
照它计数会数错 ✗ —— 计数以 `ValidationCopy::ALL` 的 `[_; 11]` 为硬约束 ✓）：

| # | 来源 | 级别 | 文案（逐字） |
| --- | --- | --- | --- |
| 1 | §8.2 | error | 路径为空 |
| 2 | §8.2 | error | 请输入完整路径，如 C:\Users\… |
| 3 | §8.2 | error | 路径含非法字符 |
| 4 | §8.2 | error | 没有写入权限，请换一个目录 |
| 5 | §8.2 | error | {盘}: 仅剩 x，保存需约 y |
| 6 | §8.2 | warn | 目录不存在，保存时创建 |
| 7 | §8.3 | error | 文件名不能为空 |
| 8 | §8.3 | error | 含非法字符 {去重后的字符列表} |
| 9 | §8.3 | error | “{名}”是系统保留名 |
| 10 | §8.3 | error | 完整路径超过 260 字符 |
| 11 | §8.3 | warn | 末尾的点或空格会被系统去掉 |

**修复动作与冲突链接**（**不是**校验条目 ✓，不计入 11 ✓）：

| 来源 | 文本 | 用途 |
| --- | --- | --- |
| `ValidationCopy::DirectoryMissing.repair()` | 立即创建 | warn 的修复动作 |
| `ValidationCopy::StemIllegalChars.repair()` | 替换为 _ | warn 的修复动作 |
| `ValidationCopy::StemTrailingDotOrSpace.repair()` | 去掉 | warn 的修复动作 |
| `ConflictLink::Overwrite` | 改为覆盖 | 底栏链接 |
| `ConflictLink::Rename` | 改为重命名 | 底栏链接 |

**字形**：以上全部已进字体子集 ✓（`subfont/subset.ps1` 重建后 **280 codepoints / 42.5 KB**，从 225 / 32 KB
涨上来 ✓）—— 由 `the_embedded_subset_covers_the_strings_the_overlay_draws` 守住 ✓；未进子集时它**先红** ✓，
红的原文是 `no glyph for '路' (U+8DEF), drawn by "路径为空"` ✓。

**冲突与 Toast**：Toast 四操作在 S8 实现时逐字从规格 §7.6 摘录 ✓。

> **`snapclip-ui` 的 `TextField` 与本文无关** ✗：D11 定为原生 `EDIT` ✓ ⇒ 保存面板**不使用**该控件 ✓；其去留见 `docs/47` 与 `docs/42 D11`（**当前无任何调用方** ✗，建议删除或明确标注预留，由上层决定 ✓）。

> **变异测试必须强制重建** ✗：用文件拷贝还原被变异的源文件时 **mtime 被保留** ✓ ⇒ cargo 认为无需重编
> ✓ ⇒ "还原后复跑"实际跑的是**变异后的二进制** ✓（会产生**假红** ✗，本清单记录过一次）。
> 做法：还原后 `(Get-Item <file>).LastWriteTime = Get-Date` ✓（或 `git apply` 反向补丁 ✓），并**确认 cargo
> 真的重编了** ✓（看到 `Compiling` 行 ✓）。

> **纪律**：S0 同时负责共享控件内核、既有截图面板迁移回归和新文案字形门禁；产品字段语义仍由 S5/S6 实现。截图基线与迁移测试必须真实执行，不能以代码抽取完成代替验收。

---

## 1. 阶段总表

| 阶段 | 内容 | 人天 | 出口 |
| --- | --- | ---: | --- |
| **S0** | **已有自绘控件封装 + 现有截图面板迁移 + 文案基线** | **0.5–1**（迁移已完成 ✓，见 `docs/47`） | 面板外壳、文本绘制、图标、分段器、按钮、布局命中等共享能力就位；现有截图面板迁移后回归绿；字体覆盖门禁绿 |
| S1 | 保存与 artifact 提交（`PendingCapture`，无 UI） | 3–4 | 20 条服务测试绿；PNG 字节相同 |
| S2 | 编码服务（字节数 + 可解码字节） | 2–3 | 体积与实际文件一致；代际取消 |
| S3 | **整体面板框架（独立普通窗口 + D2D 外壳）** | **4–5** | 920 dip 设计视口、标题栏/底栏、两栏网格、模态焦点与共享视觉 token 就位 |
| **S4** | **预览：字节数 + 解码画面 + 对比滑块** | **5–7** | 解码画面与产物逐像素一致 |
| S5 | **右侧操作栏基础表单（原生字段）** | **3–5** | 路径/命名/格式/质量/校验与原生 `EDIT` 输入可用 |
| S6 | **右侧操作设置：尺寸、限制体积与高级项** | **4–5** | 尺寸联动正确、产物不越体积上限、高级选项生效 |
| S7 | 长截图分割 + PDF | **已解闸** ✓（D10 已定），估时待细化 ✗ | —— |
| S8 | Toast / 撤销 / 重命名 / 保存后动作 | 4–5 | 撤销与回滚在失败注入下正确 |
| S9 | 键盘 / UIA / 动效 / 高对比 | 4–6 | 人工脚本 + 可自动化部分单测 |
| S10 | 接进捕获流程（`Destination::Save`） | 2–3 | 确认后不写文件；滚动走同一命名规则 |

**总人天（重算，D11 = 原生 `EDIT` + 独立普通窗口）**：S0 **0.5–1**（迁移已完成 ✓）+ S1 3–4 + S2 2–3 + S3 **4–5** + S4 **5–7**（WIC / `zenwebp` 解码，零新依赖 ✓）+ S5 **3–5** + S6 **4–5** + S8 4–5 + S9 4–6 + S10 2–3 ⇒ **约 31.5–44 人天（不含 S7）** ✓。

---

## S0. 已有自绘控件封装、现有面板迁移与文案基线

**目标**：先把现有自定义截图面板已经验证过的绘制与交互能力封装成共享层，再让原面板迁移到共享层。S0 不把缺失的 TextField 假装成“已有能力”；它只建立后续 TextField、Select、Toast 等控件必须遵守的绘制、布局、命中和状态契约。

### S0 当前实施状态

截至本轮，S0 已完成第一批可复用能力并已接入截图面板：

| 能力 | 当前实现 | 状态 |
| --- | --- | --- |
| 共享 crate 与依赖方向 | `crates/snapclip-ui`；capture/app 依赖它，UI 不依赖业务层 | 已完成 |
| D2D 原语 | `PanelPainter` 统一圆角填充、描边、线条、文本绘制与 DirectWrite 测量 | 已完成 |
| 分段器 | `SegmentState` + `PanelPainter::segmented`；RegionPanel 已消费共享实现 | 已完成 |
| 按钮 | `panel/button.rs`：`Button` / `ButtonState`，含命中与 `paint` 绘制入口 | 已完成 |
| Tag 胶囊 / 下拉 / 单选框 | `panel/tag.rs`、`panel/dropdown.rs`、`panel/radio.rs`：几何、状态、命中和 D2D 绘制入口 | 已完成（截图面板的尺寸/位置/单位已使用 Tag；保存面板后续复用） |
| 主题 token | `PanelTheme`；RegionPanel 画刷初始化使用同一组 token | 已完成 |
| 图标资产与图标按钮 | `snapclip-ui::panel::icons` 提供 `Glyph`/PNG 契约，`panel/icon_button.rs` 提供几何与命中；`PanelIcons` 仍由 capture 持有设备相关 WIC/D2D 缓存 | 共享契约已完成；设备缓存按生命周期留在 renderer |
| 通用 PanelLayout | 仍由 `RegionPanelLayout` 持有领域布局；尚未抽出跨面板布局接口 | 未完成 |
| 迁移前后视觉快照对比 | 现有 D2D 测试和几何/命中回归已运行；GPU 真实渲染测试仍按现有 L4 规则 `ignored` | 部分完成 |

因此本轮不能把整个 S0 标记为完成；保存面板在 S3–S6 之前仍需等待跨面板布局契约和视觉基线比较收口。图标不再需要在保存面板复制一套资产或缓存接口。

**基础能力**：

- `PanelTheme` / 画刷缓存：surface、fill、line、line-soft、accent、muted、danger、warning，以及 DPI 下的圆角、边框和焦点环；
- 文本绘制服务：DirectWrite 字体回退、字号/字重、文本测量、基线、等宽数字和灰度抗锯齿；
- 已有通用视觉控件：`SegmentState`、`IconButton`、`Button`、`Tag`、`Dropdown`、`RadioGroup`、圆角容器和状态画刷；图标资产契约在 `snapclip-ui`，WIC/D2D 设备缓存留在 renderer；`TextField` 的通用交互实现留给后续阶段，但必须消费同一套 token 和 layout 约定；
- 布局/命中测试约定：绘制和命中测试共用不可变 layout；
- `PanelView` 快照约定：状态机先计算不可变视图，再由绘制器消费，渲染器不反推业务状态。

现有自定义截图面板的数字字段仍是领域输入逻辑，不得把它误称为通用 TextField；保存面板的任意文本 TextField 在 S5 实现，但必须消费 S0 的文本绘制、token、状态和命中测试约定。不得复制文本测量、焦点环和分段器绘制逻辑。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `the_existing_region_panel_is_green_before_extraction` | 现有 `region::`、`geometry::`、D2D 视觉回归作为迁移前基线 | ——（基线） |
| 2 | `shared_controls_have_one_layout_and_hit_test_source` | 分段器、按钮、菜单和图标的绘制矩形与命中矩形来自同一 layout | 控件内核不存在 |
| 3 | `shared_paint_primitives_match_the_existing_region_panel` | 文本、圆角、图标、分段器和按钮在共享层的输出与现有面板一致 | 共享绘制层不存在 |
| 4 | `the_region_panel_uses_the_shared_controls_without_visual_drift` | 迁移前后固定 DPI/状态截图的几何、颜色、字重和命中行为一致 | 现有面板尚未迁移 |
| 5 | `the_embedded_subset_draws_every_new_save_panel_character` | `windows/win/d2d/tests.rs:1451` 覆盖门禁对新词表全绿 | 词表未加 |
| 6 | `the_existing_conversion_suite_still_passes` | `convert.rs` 的 7 条测试全绿 | ——（基线） |

**实现顺序**：先固定现有截图面板的测试/截图基线；从 `d2d.rs` 抽取 `draw_text_in`、文本测量/格式、圆角填充/描边、分段器、图标着色和画刷/token；整理 layout 即命中测试唯一来源；将现有 RegionPanel 的视觉控件迁移到共享层并删除重复实现；最后补字体子集文案并运行门禁。当前已完成文本、测量、圆角、线条、分段器、主题、基础按钮命中、图标资产契约和 `IconButton` 几何迁移；跨面板布局契约和迁移前后固定状态的 GPU 快照比较仍待完成。文本输入按 D11 由**原生 `EDIT`** 承担 ✓，其插入点/选区/撤销/剪贴板/UIA 均由系统提供 ✓；S0 不涉及文本输入 ✓。

**实现清单**：优先新增独立的 `snapclip-ui` Windows/D2D crate（只有依赖图证明不需新 crate 时才选择同 UI crate 内模块）；截图面板和保存面板共同依赖它。不得让 `snapclip-app` 访问 `snapclip-capture` 的 `pub(crate)` 绘制实现。

**验收命令**

```bash
powershell -NoProfile -ExecutionPolicy Bypass -File subfont/subset.ps1
cargo test -p snapclip-capture --lib region:: -- --test-threads=1
cargo test -p snapclip-capture --lib geometry:: -- --test-threads=1
cargo test -p snapclip-capture --lib windows::win::d2d -- --test-threads=1
```

**阶段出口**：已有视觉控件只有一套实现；现有截图面板完成迁移且迁移前后回归通过；S3–S6 只能组合共享视觉控件。TextField 尚未完成不视为 S0 失败，但 S5 不得绕过共享绘制层自行实现。

**风险与回退**：抽取可能改变字形度量或命中热区 ⇒ 以迁移前后截图、DPI 矩阵和现有单测验收；不通过保留第二套旧控件来“回退”。

**估时**：**0.5–1 人天** ✓ —— 控件封装与截图面板迁移**已经完成** ✓（`crates/snapclip-ui` ✓，`docs/47` 审查为真抽取 ✓，capture 534 测试与迁移前一致 ✓）；剩余：文案白名单 ✓ + `snapclip-ui` 的契约补充（笔刷与 hit-test 收进控件 ✓，见 `docs/47` P1/P2 ✓）。

---

## S1. 编码产物与保存提交（`PendingCapture`，无 UI）

> **三条我凭记忆写错、已被规格纠正的事实** ✗（下一个人别再照旧版做 ✓）：
> 1. **序号不补零** ✓ —— 规格示例是 `7` ✓，不是 `007` ✗；
> 2. **模板变量是** `[日期][时间][序号][分辨率][软件名]` ✓ —— 不是 `[名][时间][序号][宽][高]` ✗；
> 3. **末尾的点/空格是 `warn` + 修复（去掉）** ✓ —— 不是拒绝 ✗。
>
> 另：文案的**唯一来源**是 `crates/snapclip-ui/src/copy.rs` ✓（不是本清单、也不是 app 里的副本 ✗）；
> 本清单 §0.3 只是索引 ✓。


**目标**：从 capture crate 的 `PendingCapture` 编码出唯一的文件型 `EncodedArtifact`，再将该 artifact 的字节提交到目标路径。预览、复制、拖出和保存共用该产物（D9）；保存操作不再次编码。输入源和临时产物分别由 `PendingCapture` 与 `EncodedArtifact` 单一持有。

**接口（编码参数与文件命名分离，路径变化不使编码缓存失效）**

```rust
pub fn encode(source: &mut PendingCapture, request: &EncodeRequest) -> Result<EncodedArtifact, SaveError>;
pub fn save(artifact: &EncodedArtifact, request: &SaveRequest) -> Result<SaveOutcome, SaveError>;

pub struct EncodeRequest {
    pub output_size: Size,
    pub format: SaveFormat,
    pub quality: u8,
    pub advanced: AdvancedOptions,
}

pub struct SaveRequest {
    pub directory: PathBuf,
    pub template: Template,
    pub sequence: u32,
    pub conflict: ConflictPolicy,    // 默认 AutoRename
}

pub struct SaveOutcome { pub path: PathBuf, pub bytes: u64, pub format: SaveFormat }
```

- `PendingCapture` 是 capture crate 的 opaque 类型；内部独占 `SelectionPixels`、滚动 tile lease 或现有 artifact。app crate 不引用 crate-private 的 `ScrollBands`，也不重复保存 lease。
- 编码器直接写临时 artifact 文件，不把完整编码结果强制装进 `Vec<u8>`。`EncodedArtifact` 独占清理该文件；其 key 包含源身份、尺寸、格式、质量和高级项。
- PNG/JPEG 使用有界行带写入；WebP 只在现有尺寸与像素预算内接受，超限必须在读取首行前拒绝。S1 覆盖三种格式的 whole source；Bands 对 PNG/JPEG 覆盖行带路径，WebP 按格式预算验证接受/拒绝行为。
- 保存将 artifact 复制到目标目录的 `.partial`，flush 后提交；保留 artifact 供面板再次保存。失败时清理目标 `.partial`，不得破坏缓存 artifact。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `a_png_request_copies_the_master_byte_for_byte` | `EncodedArtifact` 提交后的目标与 artifact 字节相同 | 服务不存在 |
| 2 | `a_banded_png_request_matches_the_whole_png_byte_for_byte` | 同一编码器配置下 `Bands` 与 `Whole` 的 PNG 字节相同（且 Bands 不物化整图） | 同上 |
| 3 | `a_jpeg_request_produces_a_readable_jpeg_of_the_source_size` | Whole 与预算内 Bands 均可解码，尺寸 = 源尺寸 | 同上 |
| 4 | `a_webp_request_produces_a_lossless_webp` | 预算内 Whole 可解码且逐像素等于源（D1）；超预算在读取前拒绝 | 同上 |
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
| 20 | `cancelling_midway_leaves_no_file_and_no_partial` | 取消编码或提交 ⇒ 无目标文件、无 `.partial`；artifact owner 清理临时文件 | 同上 |

**实现清单**

| 文件 | 内容 |
| --- | --- |
| `crates/snapclip-capture/src/save_source.rs`（新） | opaque `PendingCapture` 与受控行带访问；封装 `ScrollBands`，由 capture crate 唯一持有和清理 |
| `apps/snapclip/src/save/mod.rs`（新） | `EncodeRequest` / `SaveRequest` / `SaveOutcome` / `SaveError` / `EncodedArtifact` 门面 |
| `apps/snapclip/src/save/source.rs`（新） | capture crate opaque source 到编码器的适配；不暴露 `ScrollBands` |
| `apps/snapclip/src/save/template.rs`（新） | `Template::parse` / `expand` |
| `apps/snapclip/src/save/validate.rs`（新） | 11 条校验（纯函数，不产出文件） |
| `apps/snapclip/src/save/conflict.rs`（新） | `resolve(dir, stem, ext, policy)`；不区分大小写 |
| `apps/snapclip/src/save/service.rs`（新） | 校验 → 格式能力检查 → `EncodedArtifact` 临时文件编码 → artifact 字节原子提交；保存不重编码 |
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

**目标**：一次编码产出文件型 `EncodedArtifact`，其文件大小是实测体积、内容供 S4 解码；S1 保存直接提交同一个 artifact。带防抖、代际取消和有界缓存。

**测试先行清单**

| # | 测试名 | 断言 | 先红原因 |
| --- | --- | --- | --- |
| 1 | `the_encoded_artifact_size_equals_its_file_length` | 报告体积等于 artifact 文件长度 | 服务不存在 |
| 2 | `a_cached_request_reuses_the_same_artifact` | 同一 key 第二次不编码，artifact 内容相同 | 同上 |
| 3 | `changing_an_advanced_option_invalidates_the_cache` | 键含格式/质量/高级项/源摘要 | 同上 |
| 4 | `a_newer_request_supersedes_the_older_one` | 旧代次结果被丢弃并清理临时 artifact（复用 export worker 协议） | 同上 |
| 5 | `saving_a_preview_artifact_does_not_encode_again` | 保存目标逐字节等于预览所用 artifact；编码调用数不增加 | 同上 |
| 6 | `the_artifact_decodes_to_the_expected_pixels` | PNG/WebP 无损逐像素一致；JPG 只断言可解码、尺寸正确 | 同上 |
| 7 | `the_artifact_cache_obeys_its_byte_budget` | 活跃预览可保留；缓存按字节上限驱逐且删除临时文件 | 上限与实现不存在 |

**实现清单**：`save/encode.rs`（编码器写入临时文件，产出 `EncodedArtifact`）、`save/cache.rs`（key + 按字节限额的 artifact LRU）、`save/generation.rs`（复用导出线程的 generation 语义，不自建线程池）。完整编码字节不得默认驻留内存；S2 必须用真实尺寸语料记录峰值内存并据此定缓存上限。

**验收命令**：`cargo test -p snapclip-app --lib save::`

**风险与回退**：大图编码会占 CPU ⇒ 必须在后台线程（导出线程），面板只收结果。

**估时**：2–3 人天。

---

## S3. 整体面板框架（Win32 + D2D 外壳）

**目标**：先搭完整面板的整体结构，再进入具体区域。建立 920 dip 设计视口、无边框模态窗口、标题栏、左预览/右操作两栏网格、底栏与独立顶层窗口；S3 只呈现结构占位，不提前实现预览内容或右侧表单行为。整体尺寸、间距、圆角、色彩和字体 token 依据规格 §2 与 §11。

**平台实现**：面板是**独立普通（非分层）顶层窗口** ✓ —— 打开时截图框**已消失**（D11 ✓），因此不需要浮层层级、不存在与选区的 z-order 或焦点跨栈协调 ✗。背景用 DWM backdrop（毛玻璃 ✓，普通窗口才能做 ✓），外壳由 D2D 绘制 ✓。关闭路径（Esc/关闭按钮/取消 ✓）按 **D12** 处理，并在此阶段确立 `PendingCapture` 唯一所有权 ✓。左栏预览区与右栏操作区仅使用边界明确的布局槽位，后续 S4 填充左栏，S5–S6 填充右栏。

**测试先行清单**

| # | 测试名 | 断言 | 层次 | 先红原因 |
| --- | --- | --- | --- | --- |
| 1 | `the_panel_layout_matches_the_demo_frame` | 固定 920 dip 视口下标题栏、两栏、间距、底栏边界符合 §2 和 §11.4 | 布局单测 + 截图 | 面板不存在 |
| 2 | `the_preview_and_action_slots_keep_their_declared_geometry` | 左 556、右 320、栏间距 12 dip；小屏按 §15 适配，不挤压控件 | 布局单测 | 两栏布局不存在 |
| 3 | `the_modal_frame_restores_parent_state_on_every_close_path` | 取消、Esc、关闭按钮和异常退出均恢复父窗口状态并释放 `PendingCapture` | 真实窗口测试 | 生命周期未接 |
| 4 | `the_panel_shell_renders_with_shared_tokens` | surface、line、字体和圆角 token 有固定环境截图基线 | 截图基准 + 实机检查 | D2D 外壳未实现 |

**实现清单**：`window.rs`（窗口类、消息循环、模态焦点与 `WM_APP`）、`layout.rs`（仅整体框架与两栏几何）、`draw.rs`（面板外壳、标题栏、底栏、占位区域）、`pending.rs`（capture crate opaque handle 的唯一所有权）。前置条件是 S0 的共享控件抽取和现有截图面板迁移回归已通过。

**验收命令**

```bash
cargo test -p snapclip-app --lib ui::save_panel::layout
cargo test -p snapclip-app --features test-support --lib ui::save_panel::window
```

**阶段验收**：先在完整面板视口检查整体框架与区域比例；此时只验布局与外观基线，不以占位区域冒充预览或操作功能。窗口缩小时遵守规格 §15，不通过整体缩放字体或控件来挤入视口。

**估时**：**4–5 人天** ✓ —— 普通窗口与 D2D 外壳 ✓、四象限布局 ✓、关闭路径与 `PendingCapture` 所有权 ✓；不含浮层协调 ✗（那部分随 D11 消失 ✓）。

---

## S4. 预览：字节数 + 解码画面 + 对比滑块（D5：**不可选**）

**目标**：左栏显示**真实产物**：①编码后字节数 ②**编码后再解码**的画面 ③对比滑块（有损格式）；含缩放/网格/尺寸标签/背景。

**前置：解码（已无选型问题 ✓）** —— **PNG/JPG 走 WIC** ✓（`IWICImagingFactory` 已在用 ✓，零新依赖 ✓）；**WebP 走 `zenwebp`** ✓（已是依赖 ✓，`zenwebp::oneshot::decode_rgba` ✓）。⇒ **不新增任何解码 crate** ✓（`image` 仍否 ✗）。解码器只用于**预览与剪贴板表示**，不进入编码/保存路径。大图解码按可见区域或有界 tile 供给，缓存按字节计量并驱逐；S2/S4 用支持的最大输入确定并验证上限。

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

**验收命令**：

```bash
cargo test -p snapclip-app --lib ui::save_panel::preview
cargo test -p snapclip-app --lib save::decode
```

**风险与回退**：不再是"新依赖"风险 ✓，而是**解码一致性** ✓：WIC 对异常 JPEG（CMYK/渐进）与 `zenwebp` 对特殊 WebP 的行为需在 S4 实测 ✓；若某格式解不出 ⇒ **只影响该格式的"解码画面"** ✓，字节数与保存不受影响 ✓，但**不能默默降级为"只有字节数"** ✗（D5 ✓）。

**估时**：**5–7 人天** ✓（含原 S4b；原 7–9 ✗ —— 差额来自解码零新依赖 ✓ 与面板已是普通窗口 ✓）。

---

## S5. 右侧操作栏基础表单（原生字段）

**目标**：在 S3 的右栏槽位内实现路径/命名/格式/质量/冲突与保存底栏。输入字段使用**原生 `EDIT`** ✓（D11 ✓），外壳与按钮由 D2D/`snapclip-ui` 绘制 ✓；状态机、校验、持久化由右栏统一驱动 ✓。**数据源由 `PendingCapture` 持有** ✓，右栏不复制源像素 ✓。

**平台实现（D11）**：面板是**独立普通窗口** ✓（截图框已消失 ✓），输入字段是**真实 `WS_CHILD` `EDIT`** ✓ ⇒ IME/候选窗/撤销/剪贴板/拖选/UIA 由系统提供 ✓。**不自绘 TextField、不接 IMM32、不接 TSF** ✗。`Tab` 在真实控件间传递焦点 ✓；`EDIT` 通过 `WM_CTLCOLOR*` 与父窗口的 D2D 外壳对齐颜色与字体 ✓；`EDIT` **内缩 1–2 dip**，圆角由外框负责 ✓。

**测试先行清单**

| # | 测试名 | 断言 | 层次 | 先红原因 |
| --- | --- | --- | --- | --- |
| 1 | `the_form_reports_each_validation_rule_without_saving` | 11 条规则各一非法状态 ⇒ 状态行逐字等于 §0.3 | 单测（纯状态机） | 表单不存在 |
| 2 | `the_save_button_is_disabled_while_an_error_stands` | error ⇒ 禁用；仅 warn ⇒ 可用 | 单测 | 同上 |
| 3 | `the_warning_offers_the_repair_it_names` | “立即创建”可点且真的创建目录 | 单测 + 临时目录 | 同上 |
| 4 | `jpeg_is_disabled_for_a_scroll_source_above_the_budget` | 超 16 MP 的 `Bands` 源 ⇒ JPG 禁用并给出原因 | 单测 | 上限未接 UI |
| 5 | `the_sequence_previews_the_next_number` | 显示下一个值；成功后自增 | 单测 | 未接 |
| 6 | `escape_cancels_and_enter_saves` | 键盘分派表 | 单测（分派）+ 真实窗口 | 未接 |
| 7 | `the_quality_row_is_absent_for_png_and_webp` | D1：WebP 也无质量档 | 单测 | 未接 |
| 8 | `the_native_edit_fields_receive_focus_in_tab_order` | `Tab` 在真实 `EDIT` 之间循环、不越出面板 | 真实窗口 | 控件不存在 |
| 9 | `a_native_edit_accepts_composition_and_commits_once` | 真实 IME 组合/提交/取消；字符**只出现一次** ✗ | 真实窗口（需交互桌面） | 控件不存在 |
| 10 | `the_edit_inset_keeps_the_rounded_frame_visible` | 位图断言：外框圆角完整、`EDIT` 背景与框同色、内缩 1–2 dip | 位图（`render_view`） | 外框未接 |
| 11 | `cancelling_the_panel_follows_d12` | 默认退出截图；配置为回退时截图框恢复且 `PendingCapture` 未释放 | 真实窗口 + 单测 | D12 未接 |
| 12 | `the_basic_action_rows_follow_the_demo_order` | 行序与 §2、§5 一致；为 S6 保留规格定义的区域 | 布局单测 + 截图 | 右栏不存在 |

> **层次纪律**：1–7、12 为纯单测 ✓；8/9/11 需**真实窗口** ✓（无桌面 CI 用 `#[ignore = "needs an interactive desktop"]` 登记 ✓，发布前门禁必须执行 ✓）；10 用现有位图装置 ✓。**不写任何 IMM32 / 组合渲染 / 候选窗测试** ✗ —— 那些行为属于系统 ✓，只验“能输入中文且只提交一次” ✓。

**实现清单**：`mod.rs`（右栏状态机、派生量与校验）、`snapclip-ui`（外壳、按钮、分段器、`Tag`）、`layout.rs`（右栏行组 + `EDIT` 的放置矩形）、`draw.rs`（标签、外框、状态行）、`native_edit.rs`（`CreateWindowExW("EDIT")` + 字体/`WM_CTLCOLOR*` + 内缩 ✓）、`preferences.rs`（模板/序号/路径/格式/质量/保存后动作）。**不再需要** `ime.rs`（IMM32）✗ 与共享 `TextField` ✗。

**阶段验收**：S3 面板框架与 S4 预览就位后，确认右栏密度/分组/底栏对齐，再确认字段行为；`EDIT` 的视觉与外壳一致性由第 10 条位图断言收口 ✓。右栏不得靠增大面板或改左栏几何来掩盖布局问题 ✓。

**估时**：**3–5 人天** ✓（原 13–22 ✗ —— 差额全部来自“不再自研文本输入” ✓）。

## S6. 右侧操作设置：尺寸、限制体积与高级项

### S6.1 尺寸缩放 + 限制体积

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

### S6.2 高级选项（按 D1–D4 裁剪）

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

**只能人工**（写明并只承认人工结论）：13 条动效的观感、三套高对比主题下 ≥4.5:1 的对比度、UIA 树的语义是否可理解（用 Inspect.exe）。所有区域实现完成后，以规格 §11.4 为准，在同 Windows/DPI/字体环境做 demo 与原生面板截图对比，覆盖面板布局、颜色、字体、控件状态和层级；记录差异并修正可见偏差。抗锯齿逐像素差异可接受，几何偏移、比例、色值、字重或基线明显不符则不通过。

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
| **M1 能保存** | S0 + S1 | **3.5–5** | 共享控件内核已迁移回归；保存服务 + 20 条测试；`convert()` 有生产调用者 |
| **M2 面板框架就位** | + S2 + S3 | **9.5–13** | 完整窗口框架、标题栏、左右区域、底栏与模态生命周期正确 |
| **M3 所见即所存** | + S4 | **14.5–20** | 左侧预览显示**真实解码结果**（D5），对比滑块可用 |
| **M4 操作可用** | + S5 + S6 | **21.5–30** | 右侧基础表单、尺寸/体积上限与高级项可用 |
| **M5 核心流程可交付** | + S8 + S9 + S10 | **31.5–44**（不含 S7） | 不包含长截图分割与 PDF；不得称为完整保存面板 |
| **长截图输出完整** | S7 | 待估 | D10 已定、可以实施；完成测试拆解与估时后纳入整体排期 |

> **阶段顺序**：S0 先统一并迁移基础控件；M2 验整体框架，M3 验左侧预览，M4 验右侧操作，随后 S9 验完整面板。后续 UI 阶段不得绕过 S0 另造控件。

> **D5 的纪律**：M2 **不是**"预览已完成"的里程碑 —— 它只有面板框架与字节服务。**M3 才算"所见即所存"**，任何文档/汇报不得把 M2 说成预览已完成 ✗。

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
| 质量 75 / 50 的文字与渐变损失（D8） | `codec-bench --quality` 三类内容 | S6 的档位搜索与质量行 |
| 4:2:0 在真实截图上的代价 | 同上 + `docs/34 §11` 方法 | S6 的默认值说明 |
| PDF 页尺寸 / WebP 回退（D10） | **已定** ✓ | S7（解闸 ✓，估时待细化 ✗） |
| 解码依赖选型（S4） | **已定 ✓**：PNG/JPG 走 WIC ✓、WebP 走 `zenwebp` ✓ ⇒ 零新依赖 | —— |
| 重采样依赖选型（S6） | 同上 | S6 的缩放部分 |
| 文本输入实现（D11） | **已定：原生 `EDIT` + 独立普通窗口 + DWM backdrop** | S5、S9 |

### 视觉断言的纪律（三次事故之后立的规矩）✗

**每一条视觉断言必须同时说出"哪个位置"和"什么颜色"。** ✗

禁止"某处存在某个像素满足…"这类写法 ✓ —— 它在三处连续失效：

| 断言 | 变异 | 结果 |
| --- | --- | --- |
| `a_focused_field_draws_its_border_and_ring` | 删掉内环 | **仍然通过** ✗ |
| `an_idle_field_has_no_visible_border` | 边框改成可见 | **仍然通过** ✗ |
| `the_card_border_is_drawn_over_the_checkerboard` | 描边换成棋盘格画刷 | **仍然通过** ✗ |

做法：

- 用 `assert_px(&canvas, x, y, colour, tol, what)` ✓ 与 `assert_not_any(&canvas, x, y, &[colours], tol, what)` ✓
  （`crates/snapclip-ui/src/panel/frame.rs` 的测试模块 ✓），**位置 + 颜色**缺一不可 ✓；
- `--line` 这类**半透明描边**要按"叠在什么之上"算出**混合色**再断言 ✓（`line_over(fill)` ✓）—— 只排除底下的填充是不够的 ✓；
- 每条视觉断言都要用**变异**证明它会咬 ✓：**换颜色** ✓ / **换绘制顺序** ✓ / **删元素** ✓。
  变异还原后必须 `touch` 文件并**看到 `Compiling`** ✓，否则跑的是变异后的二进制 ✓（`Copy-Item` 保留 mtime 的坑 ✓）。

### 批次 A 记录：`Ctrl+S` 的两条设计约束（实测得出）

- **隐藏截图框不得直接调 `hide_overlay`** ✗ —— 必须走**会话退出序列**（`ExitStep::Hide`，`windows/overlay/window_restore.rs:149` 附近）✓，否则会跳过恢复窗口 region、清理遮罩等步骤 ✓。`Ctrl+S` 的处理器**只置 `pending_save` 并重绘** ✓，隐藏交给退出序列 ✓。
- **读回的 BGRA 目前没有留存** ✗ —— `SelectionPixels` 是**导出时现场构造**的（`windows/overlay/session.rs` 的导出路径 ✓）⇒ 面板要预览，必须在**导出路径之前**把读回结果接出来 ✓（批次 B ✓）。

另记：`keymap.rs` 的 `ALL` 是**定长数组** `[Action; 21]` ✗ ⇒ 新增动作**必须同时改长度** ✓（这是上一次 `E0308` 的根因 ✓）。

### 变异检查清单（三次假绿之后立的规矩）

1. **改一处"一定生效"的地方** ✗ —— 正则没匹配 = 变异没发生 = "绿"没有意义 ✓；
2. **用测试名过滤** ✓，不要自己写正则 ✗；
3. **不要过滤输出** ✓ —— 被截断就重跑 ✓；**看不到红就不算有证据** ✓；
4. 还原后必须**看到 `Compiling`** ✓（`Copy-Item` 保 mtime ⇒ 不重编 ⇒ 跑的是变异后的二进制 ✗）**且 SHA256 与原件一致** ✓；
5. 若变异后**仍然绿** ✗ ⇒ **停下报告** ✓ —— 那是"断言咬不到"的真问题 ✓，比拿到红更重要 ✓。

### 已知缺口（**分诊后的唯一待办视图** ✓）

上面各批末尾的「新增已知缺口」是**当时的记录** ✓，保留作历史 ✗；**本节的 A / B / C 才是待办** ✓ —— 每条带落点 ✓，完成后**在本节划掉** ✓（并在原批的记录里也划掉 ✓）。这样它不会变成一个**只增不减**的清单 ✗。

#### A. 还没做（计划内，有落点）

| # | 缺口 | 落点 | 归属批次 |
| --- | --- | --- | --- |
| ~~A1~~ | ~~两个尺寸格**起始为空** ✗~~ ✓ **已显示真实宽高**（`ShellOptions.capture_size` ✓，`SelectionPixels.region` 那条链路 ✓；没有捕获时**显示空** ✗ 而不编造 ✓） | — | 已完成 ✓ |
| ~~A4~~ | ~~**`.link-hub` 与两个 ghost 按钮未画** ✗~~ ✓ hub **已画**（`link` 图标 ✓）；**两个 ghost 按钮仍未画** ✗（`更改` ✓ / 文件夹图标 ✓）⇒ 见 A9 | 同上 ✓ | 交互批 ✓ |
| A5 | **`保存后` 的两个 chip 未画** ✗（高 24 ✓、行内 gap 12 ✓、chip 间 gap 8 ✓ 均已量 ✓） | 同上 ✓ | 交互批 ✓ |
| ~~A6~~ | ~~**尺寸行还有两个模式钮（`数值`/`百分比`，`:585`–`:586` ✓）与一个重置钮（`:589` ✓）** ✗~~ ✓ **已画出**（`.size-mode-btn` 的几何来自 `:226`/`:227` ✓，重置钮用 `refresh` 图标 ✓，宽度**真的测量了文字** ✓）⇒ 仍**只画不接** ✗ | — | 已完成绘制 ✓；点击属交互批 ✗ |
| ~~A7~~ | ~~**尺寸格的 `宽`/`高` 与单位 `px` 未画** ✗~~ ✓ **已画出**（三段几何同源 ✓：`size_cell_label_rect` / `size_cell_input_rect` / `size_cell_unit_rect` ✓，第二格吸收余数 ✓） | — | 已完成 ✓ |
| ~~A8~~ | ~~**数值字段的等宽字体未用** ✗~~ ✓ **已用**：`assets/font/FiraCode-Regular.ttf`（283 KB）取子集到 **3.3 KB**（`subfont/build_number_font.py` ✓，16 个字符 ✓）⇒ 内嵌在 `snapclip-ui`（`panel/fonts.rs` ✓）⇒ 尺寸格的 `EDIT`（`WM_SETFONT` + `ES_RIGHT` ✓）与 `px` 单位（`number_format` ✓）都用它；门禁 `the_embedded_number_subset_covers_the_numbers_the_panel_shows` ✓ | — | 已完成 ✓ |
| A9 | **`.link-hub` 之外的两个 ghost 按钮未画** ✗（路径行的 `更改` ✓ 与文件夹图标 ✓，demo `:545`/`:546` ✓） | 宽度需要测量文本 ✓（同 B1 ✓） | 交互批 ✓ |
| ~~A10~~ | ~~**格式下拉未画** ✗~~ ✓ **已画并可点**：按钮（`--fmt-w: 80` ✓ `:49`，等宽字 ✓ 箭头 ✓）在名字行右端，菜单（`156` ✓ `:180`）右对齐它、三项 `.PNG`/`.JPG`/`.WebP` ✓ `:717`，当前项打勾 ✓ `:184`，展开时描边变蓝 + 箭头**翻转** ✓ `:175`/`:178`（翻转在像素上做 ✓，见 `icons::flipped_180` ✓）；选中项**送进保存** ✓（`format_of_index` ✓ `save/window.rs` ✓）。缺口见 A15/A16 | — | 已完成 ✓ |
| ~~A11~~ | ~~**`[序号]` 固定为 1** ✗~~ ✓ **已做**：`Sequence`（`save/assemble.rs` ✓）由 shell 的 `Arc<Mutex<Sequence>>` **跨面板共享** ✓（面板每次捕获一个 ✓），只在**保存成功且名字含 `[序号]`** 时 +1 ✓（见文末「序号」一节 ✓） | — | 已完成 ✓ |
| ~~A11b~~ | ~~**名字模板不跨面板记住** ✗~~ ✓ 规格 `:201` 的另一半（"模板与序号随输入实时记住" ✗）⇒ **模板**那半是新缺口 ✓：现在每次开面板都是默认模板 ✓ | 需要 `EDIT` 的 `EN_CHANGE` ✓ 把文本写回共享状态 ✓ | 偏好批 ✓ |
| ~~A12~~ | ~~**保存失败只记日志** ✗~~ ✓ **已做**：底部的**状态行** ✓（`.status` ✓ `:275`/`:675`，`--fs-status: 11px` ✓ `:37`）显示 `status_of` 的结果 ✓ —— 校验错误用 `copy` 的句子 ✓（有 `detail` 用 `detail` ✓）、级别决定颜色 ✓（error `--danger` ✓ `:276`，warn `--warning` ✓ `:327`）、**取消与成功都不显示** ✓；**失败时面板不再关闭** ✓（关掉就把原因带走了 ✗） | — | 已完成 ✓ |
| A13 | **悬停态未做** ✗（`.close:hover` ✓ `:97` ✓、`.btn:hover` ✓、`.fmt-btn:hover` ✓ `:174`） | 需要鼠标进入/离开消息 ✓ | 交互批 ✓ |
| A15 | **格式菜单每项右侧的预估体积未画** ✗（`.fmt-item .val` ✓ `:186`，`:203` 要求"当前尺寸与质量下的预估体积"✓）—— 要先有体积模型 ✓ | `fmt_item_rect` 已给出每行矩形 ✓ | 预估批 ✓ |
| A16 | **切换格式的连带变化未做** ✗（`:204`：质量行 ✓ 高级选项 ✓ JPG 填充行 ✓ 与重新估算 ✓）—— 后两者**还没画** ✓ | — | 高级项批 ✓ |
| A17 | **行头 `.field-msg` 与修复胶囊未做** ✗（`:335`–`:341` ✓，`repair()` 已在 `copy.rs` 里备好 ✓）—— 失败目前只在**底部一行** ✓，指出**哪个字段**要等它 | 字段级提示需要一个"哪个字段出错"的来源 ✓ | 错误提示批其二 ✓ |
| A18 | **字段边框变红未做** ✗（`.text-input.is-error { border-color: danger }` ✓ `:333`）—— `FieldState::Error` 的规则**早就写好** ✓，只差把失败接到具体字段上 ✓ | 同上 ✓（先要有"哪个字段"✓） | 与 A17 同批 ✓ |
| ~~A19~~ | ~~**绘制入口的实参已经 13 个** ✗~~ ✓ **已做**：收成 `PaintInput` ✓（字段有名 ✓ —— `quality` ✓ `format` ✓ `format_open` ✓ `fill_surface` ✓ `footer_widths` ✓ `status` ✓ `preview` ✓ `fields` ✓ `dpi` ✓）；`PanelRenderer::paint_with_preview` 只剩 `(hdc, area, input)` ✓；**只用于保留旧签名的三个壳子已删** ✗（`paint_to_dc` ✓ `paint_to_dc_with_state` ✓ `PanelRenderer::paint` ✓），**位图目标那条路径也删了** ✗（`paint_to_target` ✓ `paint_to_target_with_preview` ✓ —— 全仓**无任何调用者** ✓，其文档注释还写着"测试在用"✗，而测试走的是内存 DC ✓） | — | 已完成 ✓（**同名类型的值互换仍拦不住** ✗ ⇒ 见 A23 ✓） |
| A20 | **三种失败的状态文案是开发者语** ✗ —— `Capability` ✓ `UnsupportedFormat` ✓ `Io` ✓ 走的是 `SaveError::message()` ✓，那是 `Debug`/英文短句 ✓（ascii ✓ 所以能画 ✓），不是给用户看的措辞 ✓ | 规格 §8.2 没有这三条的句子 ✓ ⇒ 要先定文案 ✓（新增句子会牵动字形子集 ✓） | 文案批 ✓ |
| A21 | **`detail` 可能引用用户输入** ✗ —— 例如 `illegal_chars("a*b")` ✓ 与 `reserved_name(名字)` ✓ ⇒ 用户打的字**不一定在子集里** ✗ ⇒ 会**回退系统字体** ✓（能显示 ✓，字形不是我们的 ✓） | 子集装不下任意输入 ✓；要么限定显示 ✓、要么接受回退 ✓ —— **先记录** ✓ | 文案批（与 A20 一起决定）✓ |
| A22 | **两个黄色** ✗ —— `field_colours` 的 `WARNING` 是 `[255,191,0]` ✗，而 demo 的 `--warning` 是 `rgb(240,179,90)` ✓（`:23` ✓，`.status.is-warning` 与冲突链接都用它 ✓）；demo **没有**字段警告边框的规则 ✗ | 字段警告边框的规则**不在设计稿里** ✓ ⇒ 那个黄是**推断出来的** ✓，而本批的状态行用的是**设计稿的黄** ✓（`FramePalette::warning` ✓） | 画字段警告态时统一 ✓ |
| A23 | **同类型字段的「值」仍可互换** ✗ —— `PaintInput` 的 `quality` 与 `format` 都是 `usize` ✓，`fill_surface` 与 `format_open` 都是 `bool` ✓ ⇒ 把 `quality: s.format` 写在 `format` 的位置上**编译照样通过** ✗（实测 ✓：互换两个 `usize` 的值 ⇒ `Finished` ✓） | 结构体拦的是**位置**错 ✗，不是**类型**错 ✗；要拦后者得给每个量一个 **newtype**（`Quality(usize)` ✓ `Format(usize)` ✓）⇒ 值**根本不能**互相赋值 ✓ | 若要根治：类型化批（`Quality` ✓ `Format` ✓ `OptionalWidths` ✓）✓；不划算就写明"这里靠命名 + 评审" ✓ |

> **A10 的两条行为决定**（不是遗漏 ✓）：① 菜单打开时，**菜单外的一次点击只关闭菜单、不做别的** ✓ —— 菜单里的第一下属于菜单 ✓（与常见菜单一致 ✓）；② 菜单的**每项体积**与**悬停态**见 A15/A13 ✓。
>
> **A10 的一条工程改动** ✓：`.fmt-btn` / `.fmt-item` 用 `--num` 等宽字 ✓（`:173`/`:182`）⇒ `.PNG`/`.JPG`/`.WebP` 的字母要进**数字子集** ✗ ⇒ 允许集从 `fonts.rs` 与 `build_number_font.py` **两处**收进 `subfont/number-chars.txt` **一处** ✓（两个副本就是两条规则 ✗），子集 3.3 KB → **4.3 KB** ✓，23 个字符 ✓。

> **A8 的一条边界** ✗：`SIZE_UNIT_DIP = 18` 仍是**预留值** ✗（demo 无此数 ✓，而 `measure_text_width` **已可用** ✓）⇒ 改成实测会动几何与测试 ✓ ⇒ 留作单独一批 ✓。

#### B. 做不了 / 有意不做（写明为什么与何时能做）

| # | 缺口 | 为什么现在不做 | 什么时候能做 |
| --- | --- | --- | --- |
| B1 | **`保存后` 两个 chip 的宽度未预留** ✗ | chip 宽 = **文字宽 + 2×10**（`:192` ✓），而**测量文本需要设备上下文** ✗，`field.rs` 里没有 ✓ ⇒ **不编造宽度** ✓ | 交互批（届时已有绘制目标 ✓） |
| B2 | **`.est-size` 的预估值未画** ✗（demo 是 `227.31 KB` ✓ `:668` ✓） | **动态数据** ✗ —— 与"尺寸格不编造数字"同一条规矩 ✓ | 编码产物可用之后 ✓ |
| B3 | **`.enc-state` / `.enc-bar` 未画** ✗（`:349` ✓ `:351` ✓） | 它们是**编码进度** ✓，而**编码尚未发生** ✓ ⇒ 不编造进度 ✓ | 与 B2 同批 ✓ |

#### C. 未来会变成 bug（优先解决 ✗）

（空 ✓ —— C1 已解决 ✓，见下方「已划掉」✓）

#### 已划掉（曾在本节，现已完成 ✓）

- ~~**三行 label 文字未画**~~ ✓ 已画（字形子集批 ✓，一次重建 ✓ 280 ⇒ 285 ✓）；
- ~~**分段器三个词未画**~~ ✓ 已画（同批 ✓）；
- ~~**demo 有一行（`保存后`）未建模**~~ ✓ 已建模并画出 ✓；
- ~~**`FrameMetrics::row_height()/row_pitch()/group_pitch()` 名字偏窄**~~ ✓ 已改名为 `text_row_height()` / `text_row_pitch()` / `text_group_pitch()` ✓，并加了一条断言把"两类行不同高"钉在明处 ✓（见本节末 ✓）；
- ~~**A2：路径行与尺寸格未接焦点**~~ ✓ 已接 ✓ —— 焦点不再是"一个状态 + 若干副本"✗，而是**每个字段各有一份**（`FieldStates` ✓）；`WM_COMMAND` 按控件**查表**取字段身份 ✓；`FORM_FIELD_TAB_STOPS` 2 ⇒ **4** ✓；
- ~~**C1：`WM_CTLCOLOREDIT` 用单一的 `s.field_state`**~~ ✓ 已解决 ✓ —— 同一个查表 ✓ ⇒ **加一个字段不必改消息处理** ✗ ✓。

#### 交互批其一：焦点与每字段状态（本批）

**形状** ✗：`FieldId`（字段身份 ✓，`ALL` 是**唯一**字段清单 ✓，`rect()` 给出它的槽位 ✓）与 `FieldStates`（每字段一份状态 ✓）都在 `field.rs` ✓；窗口只留**一张控件表**（`controls: [HWND; FieldId::ALL.len()]` ✓），`WM_CTLCOLOREDIT` 与 `WM_COMMAND` 都从 `field_of(hwnd)` 起手 ✓ ⇒ **两处都没有逐字段分支** ✗。

**焦点通知是唯一写者** ✓：`EN_SETFOCUS` / `EN_KILLFOCUS` 各自**点名它是哪个字段** ✓ ⇒ 不需要"清掉上一个"的逻辑 ✓，也没有"当前聚焦字段"这份额外状态 ✓。

**一条被测试逼出来的契约** ✗✓：控件表**初始全是空句柄** ✓，而 `field_of` 若只按相等匹配 ✓ ⇒ **空句柄会命中第一个字段** ✗（测试当场红 ✓）⇒ 修法是**空句柄不算字段** ✓ —— 这与"读单一状态"是**同一个错误的下一层** ✗✓：把"谁都不是"的输入当成"第一个"✓。

**证据** ✓：`each_field_draws_its_own_frame_state` ✓（位图 ✓ 指名位置与颜色 ✓：路径聚焦时**路径有 accent ✓ 且文件名无边框 ✓**，反向再来一遍 ✓）、`each_field_answers_from_its_own_state` ✓、`the_drawing_loop_sees_every_field_with_its_own_state` ✓、`the_form_declares_one_tab_stop_per_field` ✓（4 ✓）、`the_control_table_answers_with_one_field_per_handle` ✓（app 侧 ✓ 唯一性 ✓ 空句柄 ✓）。

**变异三条** ✓（按本节末的清单 ✓）：把绘制循环改成读**单一**字段状态 ✗ ⇒ 位图断言红 ✓（`the focused name row's own border must be the accent` ✓）；把 `state_of` 改成恒返回 `path` ✗ ⇒ **四条**红 ✓；把控件表索引恒为 0 ✗ ⇒ 唯一性断言红 ✓（`left: 1 / right: 4` ✓）。

### 质量行记录：组间距与分段器（实测得出）

**组间距此前缺失** ✗ —— `field_rect` 原来用**一个**行距 `index × pitch` ✓，而 demo 自己的注释（`:163`）写着 `组间 16 / 组内 12：位置·文件名 | 尺寸·质量 | 预估` ✓ ⇒ **组边界要多 4 dip** ✓（`.form-row.grp { margin-top: 4px }` ✓）。缺它的后果是**尺寸、质量、预估三行各上移 4 dip** ✓ —— 而这种偏差**单看一行是看不出来的** ✗，它要等行与行之间的关系被断言时才暴露 ✓。

做法 ✓：行高**逐行相加**，而不是 `index × pitch` ✓（`FORM_ROWS` 走一遍 ✓）—— 因为**行高并不相同** ✗：文本字段是 `32`（`:167` ✓），而质量行的内容是 `.quality-line`，其 `min-height` 是 `--row` = **28**（`:306` ✓、`:47` ✓）⇒ 用一个统一 pitch 会把质量行**下面**的行放错位置 ✓。`Row::group()`（0,0,1,1 ✓）决定哪个边界加那 4 ✓，`Row::content_height()` 决定每行的内容高 ✓。

**分段器几何** ✓（全部来自 demo ✓）：轨道 **108×28** ✓、圆角 **7** ✓、内缩 **3** ✓；选中块宽 **34** ✓、圆角 **4** ✓、`translateX(index × 34)` ✓（`:263`–`:265` ✓）。三者自洽 ✓：`3 × 34 + 2 × 3 = 108` ✓ —— 已**断言**而不是默认 ✓（选项宽写错会在轨道末端留下空隙或溢出 ✓，那种事**截图看得见、颜色断言看不见** ✗）。

**新增已知缺口** ✗：

- ~~**分段器的三个词（`高`/`中`/`低`）未画**~~ ✓ **已画**（字形子集批 ✓）；
- **分段器不可点** ✗ —— `quality` 目前是 `Frame` 上的一个索引 ✓（调用方恒传 0 ✓），点击、设置与写回属**交互批** ✓；`FORM_FIELD_TAB_STOPS` 仍是 **2** ✓，因为分段器还不是原生子窗口 ✓。

### 字形子集批（本批）：一次重建，四处文字

**做法** ✓：文字**由生产者产出**，不在门禁里抄 ✗ —— `Row::label()`（`field.rs` ✓）与 `QUALITY_OPTION_WORDS` ✓ 由 `form_drawn_strings()` ✓ 汇总 ✓，而**字形门禁**（`overlay_drawn_strings()` ✓，`d2d/tests.rs:1459` ✓）**调用同一个函数** ✓ ⇒ 加一个 label 却忘了重建子集 ⇒ **门禁红** ✓，这正是我们要的约束 ✓。

**红→绿证据** ✓：加完文字、未重建时 ✓ `the_embedded_subset_covers_the_strings_the_overlay_draws` **FAILED** ✓；跑 `subfont/subset.ps1` ✓（先由 `write_drawn_text_for_the_font_subset` 重写 `drawn-text.txt` ✓，再由 `build_subset.py` 建字体 ✓）⇒ 输出 `required: 285 codepoints` ✓、`all required ones covered` ✓ ⇒ 门禁**绿** ✓（子集 43.4 KB ✓）。

**字号与颜色** ✓（全部来自 demo ✓）：label 用 `--fs-row: 11px`（`:35` ✓）weight 500（`:166` ✓）`rgba(232,236,242,.74)`（`:166` ✓，**比 `--muted` 略强** ✓，所以 palette 里多了一个 `label` ✓）；分段器的词用 `--fs-chip: 11px`（`:36` ✓）`--muted`（`:266` ✓）、选中为 `--text`（`:267` ✓）⇒ 两个 11px 的**格式**而不是一个 ✓，因为 `.seg .opt` 是 `font: inherit` ✓，字重是正文的、不是 label 的 ✓。

**两条变异都红了** ✓（按上面的检查清单 ✓）：
- label 画成**表面色** ✗ ⇒ `the_row_labels_are_drawn_above_their_rows` **FAILED** ✓（`Path's label ("保存路径") must put ink in its own row, at Rect { left: 584, top: 48, right: 904, bottom: 70 }` ✓）；
- 选中词与未选中词的画刷**对调** ✗ ⇒ `the_chosen_quality_word_is_brighter_than_the_unchosen_ones` **FAILED** ✓（`chosen=185 unchosen=242` ✓）。

**一处连带修正** ✓：`the_quality_segment_marks_the_chosen_option_with_the_accent` 原来采样选项的**中心** ✗ —— 而那里正是**词**所在 ✓ ⇒ 画上文字后它红了 ✓，而这是**正确**的红 ✓（不是误报 ✓）⇒ 采样点移到**选项的角上**（`left+4, top+4` ✓，仍在块内 ✓、避开居中的词 ✓）✓ —— 断言**没有被削弱** ✗：它仍然指名位置与颜色 ✓，并写明了为什么不在中心 ✓。

**两条新的已知缺口** ✗：

- ~~**demo 有一行我们尚未建模**~~ ✓ **已建模并画出**（本批 ✓，见下）。
- **尺寸格的 `宽`/`高` 与单位 `px` 未画** ✗ —— 它们是 `.cell-label`（13px ✓ `:237` ✓）与 `.dim-unit` ✓，位于**每个格子之前** ✓，而 `size_cell_rect` 把**整个格子**给了输入框 ✗ ⇒ 要画它们得先改**尺寸行的横向切分** ✓（让出 label 的宽度 ✓）⇒ 与"尺寸格接数字"同批做更省 ✓。

### `保存后` 行（本批）：一行"只有 header、没有字段"

**demo 实读** ✓：`.form-row`（**不是 `.grp`** ✗，`:569` ✓）⇒ 它与 `位置`/`文件名` **同组** ✓；行内是 `.row-header.after-row { justify-content: flex-start; gap: 12px }`（`:302` ✓），label `保存后`（`:571` ✓）之后是两个 `role="switch"` 的 chip ✓（`复制路径` `:573` ✓、`打开文件夹` `:574` ✓，**默认关** ✓）；`.after-toggles { gap: 8px }`（`:303` ✓）、`.chip { height: 24px; padding: 0 10px; border-radius: 999px }`（`:192` ✓）。

**结构上的新东西** ✗：这是**第一行没有字段** ✓ —— 它的 `.row-header` **就是整行** ✓ ⇒ 因此**没有** `.form-row { gap: 6px }`（`:164` ✓）✓。原公式 `label + gap + content` 会**多算 6 dip** ✗ ⇒ 引入 `Row::has_content()` ✓（`Saved` 为 `false` ✓），累加时**只在有内容时才加 gap 与 content_height** ✓；`content_height()` 对无内容行返回 **0** ✓ 并在文档里写明"**先问 `has_content`**" ✓。

**顺带修正一处耦合** ✓：`label_rect` 原来是"**从内容矩形往上抬** `gap + label_height`" ✗ —— 对无内容行**没有意义** ✗ ⇒ 改为抽出 `row_label_top()` ✓（累加的唯一出处 ✓），`label_rect` 与 `field_rect` **都从它派生** ✓ ⇒ "往上抬"这种隐式耦合消失 ✓，且对既有行**结果不变** ✓（87 绿 ✓）。

**门禁没有红，如实记录** ✗：`保存后` 三个字（`保` `存` `后`）**已在子集里** ✓（`保存路径` 已覆盖 ✓）⇒ 门禁**直接绿** ✓，**不需要重建** ✓。

**两条变异都真红** ✓（备份逐字节比对 ✓）：
- 把 `Row::Saved` 从 `FORM_ROWS` 去掉 ✗ ⇒ **9 条测试红** ✓（含 `the_after_save_row_sits_between_the_name_row_and_the_size_row` ✓、尺寸与质量的几何 ✓）⇒ 说明这一行**真的接进了布局** ✓，不是画了个孤儿 ✓；
- label 画成**表面色** ✗ ⇒ `the_row_labels_are_drawn_above_their_rows` **FAILED** ✓（`Path's label ("保存路径") must put ink in its own row` ✓）。

**两条期望按"预期正确变化"更新** ✓（不是为让错误实现通过 ✗）：`the_size_row_opens_a_group_and_sits_sixteen_below_the_name_row` ✓ 与 `the_size_row_sits_below_the_name_row_and_splits_into_two_cells` ✓ 原来量的是"**尺寸行比文件名行低 16**" ✗ —— 现在尺寸行的上一行是 `保存后` ✓ ⇒ 改为**从 `保存后` 行顶量**：它自己的 label 行 22 + 行间 12 + 组间 4 ✓（内容行再加 22 + 6 ✓）✓。

**新增已知缺口** ✗：

- **`保存后` 的两个 chip 未画、宽度也未预留** ✗ —— 与 `.link-hub` 同理 ✓：**画了不接就是假控件** ✗。**宽度未预留是有意的** ✓：chip 宽 = **文字宽 + 2×10**（`:192` ✓），而文字宽**要测量文本** ✗，`field.rs` 里没有设备上下文 ✗ ⇒ **不编造宽度** ✓（与尺寸格"不编造数字"同一条规矩 ✓）⇒ 等交互批（届时已有绘制目标 ✓）再定 ✓。已知的量 ✓：chip 高 **24** ✓、行内 gap **12** ✓、chip 间 gap **8** ✓。
- **尺寸行还有两个模式钮与一个重置钮** ✗（本批实读 demo 时发现 ✓）：`数值` / `百分比`（`.size-mode-btn`，`:585`–`:586` ✓，默认 `数值` ✓）与一个 `.action` 重置钮（`:589` ✓，`aria-label="重置为原始大小"` ✓）⇒ 它们是**控件** ✓，同样留到交互批 ✓，且**它们的宽度会影响尺寸行的横向切分** ✗（`size_cell_rect` 现在把整行给了两个格子 ✓）⇒ 与"尺寸格接数字"同批处理更省 ✓。

### 预估行（本批）：表单的最后一行，也是第三组

**demo 实读** ✓：`.form-row.grp`（`:666` ✓）⇒ 它**开新组** ✓（与 `:163` 的注释 `… | 预估` 一致 ✓）；`.row-header { height: 22px }`（`:165` ✓）里是 `.row-label` **`预估大小`**（`:667` ✓）；内容是 `.est-size { font-size: 12px; height: var(--row) }`（`:269` ✓）⇒ 高 **28** ✓ —— 与分段器**同一个 `--row`** ✓。

**结构上的新东西** ✗：`Row::Estimate`（index 5 ✓）+ `FORM_ROWS` 扩到 **6** ✓ ⇒ **label 自动画出来** ✓ —— `frame.rs` 的 label 循环**遍历 `FORM_ROWS`** ✓，所以本批**没有改 `frame.rs`** ✗（它不在改动列表里 ✓）。这是前几批"把字符串与几何交给生产者"的回报 ✓。

**顺手修的一处命名与设计不符** ✗：`QUALITY_SEG_HEIGHT_DIP` 改名为 **`ROW_DIP`** ✓ —— 因为 demo 的 `--row: 28px`（`:47` ✓）被**两个**用户共享 ✓：`.seg { height: var(--row) }`（`:263` ✓）与 `.est-size { height: var(--row) }`（`:269` ✓）⇒ 以**设计自己的变量**命名 ✓，而不是以它**第一个用户**命名 ✗（后者会让第二个用户复制一个常量 ✗）。

**字形门禁红→绿** ✓：`预估大小` 的 `预` `估` `大` **不在**子集里 ✓ ⇒ 门禁**先红** ✓（`the_embedded_subset_covers_the_strings_the_overlay_draws` FAILED ✓）⇒ 重建一次 ✓ ⇒ **285 → 288 codepoints** ✓（`小` 本就在 ✓）⇒ 绿 ✓。

**顺带印证门禁为什么必需** ✗✓：门禁红的同时 **`ui` 87 全绿** ✓ —— 位图断言**仍然通过** ✓，因为 DirectWrite 对缺失字形**回退到系统字体** ✗ ⇒ **有墨、但字形不是我们的** ✗。那种事**只有门禁看得见** ✓。

**三条变异都真红** ✓（备份逐字节比对 ✓ + 确认看到 `Compiling` ✓）：

- 把 `Estimate` 从 `FORM_ROWS` 去掉 ✗ ⇒ **编译错**（`[Row; 6]` 只剩 5 个 ✓）—— 这与早前 `Action::ALL` 的定长数组是**同一类** ✓：**加变体必须同时改长度** ✓ ⇒ 换一种**行为变异** ✓：把 `Estimate` 换成**重复的 `Quality`**（数组仍 6 个 ✓）⇒ `the_estimate_row_is_last_and_opens_the_third_group` **FAILED** ✓（`the estimate is the last row of the form (:666)` ✓）；
- label 画成**表面色** ✗ ⇒ `the_row_labels_are_drawn_above_their_rows` **FAILED** ✓（点名 `Path` ✓ —— 它按顺序查，先撞上第一行 ✓）；
- **把预估行的 label 改成空串** ✗ ⇒ 同一条测试 **FAILED** ✓，信息**专指这一行** ✓：`Estimate's label ("") must put ink in its own row, at Rect { left: 584, top: 374, right: 904, bottom: 396 }` ✓ ⇒ 这是"**它真的被画了**"的最强证据 ✓。

**本批的验证** ✓：`ui` **88/0** ✓ · `app` **208/0/9** ✓ · `capture` **544/0/80** ✓ · `check --workspace --all-targets` Finished ✓。

**新增已知缺口** ✗：

- **`.enc-state` 与 `.enc-bar` 未画** ✗（`:349` ✓ `:351` ✓）—— 它们是**编码进度指示** ✓，需要**我们没有的状态** ✗（编码尚未发生 ✓）⇒ 不画 ✓，不编造进度 ✓；
- **`.est-size` 的预估值未画** ✗（demo 显示 `227.31 KB` ✓ `:668` ✓）—— **动态数据** ✗，与尺寸格"不编造数字"同一条规矩 ✓；
- ~~**`FrameMetrics::row_height()/row_pitch()/group_pitch()` 的名字现在偏窄**~~ ✓ **已改名**（见「改名与分诊」批 ✓）：`text_row_height()` / `text_row_pitch()` / `text_group_pitch()` ✓，并加了一条断言把"两类行不同高"钉在明处 ✓。
### 改名与分诊（本批）：把"名字撒谎"修掉，把缺口清单变成待办

**② 三个指标改名** ✗ —— `row_height()` / `row_pitch()` / `group_pitch()` 的名字读起来像"**任何一行**的高度" ✗，而它们只描述**带文本字段的那一行**（22 + 6 + 32 = 60 ✓），表单里早已有 **28** 高的行 ✓（`--row` ✓，分段器 `:263` ✓ 与预估 `:269` ✓）⇒ 与 `…has_no_border_when_idle` ✗ 那次是**同一个坑** ✓。

改名 ✓：`text_row_height()` ✓ / `text_row_pitch()` ✓ / `text_group_pitch()` ✓ —— 名字**说出它适用于哪类行** ✓。牵动 **10 处** ✓（`frame.rs` 的定义 ✓ + `:850` ✓ + 三条测试 ✓；`field.rs:432/462/463/514/530` ✓）✓。

**新断言** ✓：`a_text_row_and_a_row_of_design_height_are_not_interchangeable` ✓ —— 同时钉住 **60** ✓、**`ROW_DIP == 28`** ✓、以及**两者不等** ✓（把"两类行不同高"写在明处 ✓，避免以后有人再拿一个名字覆盖两类 ✗）。

**变异** ✓（`ROW_DIP: 28 → 32` ✗）：`assertion left == right failed: --row, the design's own content height for a segment or estimate row (:47) / left: 32 / right: 28` ✓；还原后 `sha identical: True` ✓、**89 passed / 0 failed** ✓。

**① 缺口分诊** ✓：把散在各批末尾的「新增已知缺口」收进**一个**待办视图 ✓（A 还没做 8 条 ✓ / B 做不了或有意不做 3 条 ✓ / C 未来会变成 bug 1 条 ✓），每条带**落点**与**归属批次** ✓，已完成的**划掉** ✓ —— 目的：它**不能变成只增不减的清单** ✗。

### 交互批其二（本批）：分段器可点（A3 ✓ 已划掉）

**接的是现成件** ✓：`segmented.rs` 的 `SegmentState` / `target_at` / `pointer_event` ✓ —— **没有写第二套命中测试** ✗。命中矩形与绘制矩形都来自 `field.rs` 的 `quality_option_rect` ✓（`quality_segment_state` 一处构造 ✓）⇒ 点击落在**看得见的那一档**上 ✓。

**单一写者** ✓：选中的档位**只存在 `SegmentState.selected` 里** ✓（由 `pointer_event` 写 ✓），窗口侧只提供 `chosen_quality()` 读取 ✓（`field.rs` 的 `chosen_quality` ✓）—— 没有"索引 + 状态"两份答案 ✓。

**索引如何到达绘制** ✓：`paint_to_dc_with_preview` / `paint_to_target_with_preview` / `PanelRenderer::paint_with_preview` 各加一个 `quality: usize` ✓，旧的 `_with_state` / `paint_to_target` 委托 **0** ✓ ⇒ 既有测试**一行未改** ✓。

**窗口侧** ✓：`WindowState` 加 `segment` ✓ + `segment_scale` ✓；`WM_LBUTTONDOWN` / `WM_LBUTTONUP` 取 `lParam` 的**客户区像素** ✓（分段器几何本来就是像素 ✓ —— `rects(scale)` 已换算 ✓ ⇒ **没有二次换算** ✗），选中变化才 `InvalidateRect` ✓。

**测试** ✓（`ui` **97/0** ✓，`app` **211/0/9** ✓）：`the_hit_rectangles_are_the_rectangles_that_are_drawn` ✓（四 DPI ✓ 每档中心命中自己 ✓）· `clicking_the_second_option_selects_it` ✓ · `a_press_released_elsewhere_chooses_nothing` ✓ · `every_option_can_be_chosen_by_clicking_it` ✓ · `clicking_an_option_moves_the_block_the_painter_draws` ✓（**点击 → 索引 → 位图** 一条链 ✓）· `the_window_is_the_only_writer_of_the_quality_choice` ✓ · `a_scale_change_rebuilds_the_geometry_and_keeps_the_choice` ✓。

**两条变异都真红** ✓（备份逐字节比对 ✓ + 见到 `Compiling` ✓）：命中矩形取自**轨道** ✗ ⇒ 四条红 ✓（`option 0's hit rectangle is the one it is drawn at (:265, :266)` ✓）；画家**忽略索引**、恒画第 0 档 ✗ ⇒ `clicking_an_option_moves_the_block_the_painter_draws` 红 ✓（`expected Px { b: 219, g: 117, r: 31 } saw Px { b: 46, g: 41, r: 41 }` ✓）。

**一处自己的失手** ✗：第一次写测试时让"点击**已选中**的档位"也期望 `Some(index)` ✓，而契约是"**报告新选择，而不是重复的选择**" ✓ ⇒ 是**测试错了** ✓（不是代码 ✓），改成从**另一档**起手 ✓。另：还原脚本第一次**解析失败** ✗ ⇒ 整段**没执行** ✓ ⇒ 靠"检查文件当前状态"发现 ✓（**脚本没跑 ≠ 跑失败** ✗，这种要按"变异没发生"处理 ✓）。

### A6 / A7 完成：尺寸行的零件（本批）

`宽`/`高` 标签、数字框、`px` 单位、两个模式钮（`数值`/`百分比`）与重置钮（`refresh` 图标）都已画出；
上一批的"只有几何没有绘制"已补齐 —— 五条死代码警告**靠真的用上消失**，不是靠 `#[allow]`。

- **尺寸格是两个矩形，不是两个**：控制窗口取 `size_cell_input_rect`，框取 `size_cell_rect`（包裹框），
  由 `FieldId::rect` 与 `FieldId::frame_rect` 分开给出；行的字段两者相同。
- **单位在包裹框内、标签在包裹框外** —— 两条都由断言钉住（`:237`、`:244`）。
- 字形子集 288 → **292** codepoints（`宽` `高` `数` 等）。

**两条已知偏离**（都不是本批引入，但到此才看得见）：
1. `.cell-label` 在 demo 里是 **13px**（`:237`），而面板用的是 **11px 的 label 格式** —— 字号未建独立格式；
2. `SIZE_UNIT_DIP = 18` 是**预留值**（demo 无此数，文字宽要测量）—— 现在 `measure_text_width` 已存在，
   可以改为实测，但那会动几何与测试，留作单独一批。

### 底部接线（本批）：一次测量，绘制与命中共用

**形状** ✗（上一批定、本批实现）：`Frame` 多一个 `footer_widths: Option<[i32; 3]>`，窗口在**建窗时**测一次
（那时有 DC）存进 `WindowState`，**画时**传给 `Frame`、**点时**用同一份 ⇒ "**命中矩形 = 绘制矩形**" ✓。
`Frame` 里保留了"调用方没给就自己量"的回退（有注释 ✓），而**窗口总是传入** ✓ ⇒ 窗口的点击与绘制必然一致 ✓。

**三个点击** ✓：`保存` ⇒ `save_panel(&panel_values(…), w, h, bgra)` ⇒ 成功记日志（路径/字节/格式）+ **走现成的 `close`** ✓
（**不**自己 `PostMessage` 到可能被复用的句柄 ✗）；`取消` ⇒ 同一条 `WM_CLOSE` ✓；`复制` ⇒ **只画不接** ✓（剪贴板另说 ✓）。
`WM_DPICHANGED` 里**重测一次** ✓（宽度是像素 ✓，换 DPI 就失效 ✓）。

**两条取值按规格** ✓：`[日期]` = `YYYY-MM-DD`、`[时间]` = `HHmm`（规格 `:194`/`:195` ✓，**无冒号**因为文件名不能有冒号 ✓），
用 `GetLocalTime` ✓（**本地**而非 UTC ✓ —— 文件按昨天的日子命名是个意外 ✓）⇒ 为此给 app 的 `windows-sys` 加了
`Win32_System_SystemInformation` ✓（有注释说明为什么 ✓）。

**一条结构性决定** ✓：`panel_values(directory, name, date, time)` 抽成**自由函数** ✓ —— 目录与文件名**都是字符串** ✗ ⇒
互换在屏幕上**看起来完全合理** ✓ ⇒ 必须**不用窗口就能断言** ✓（与 `initial_text` 同一招 ✓）。

**证据** ✓（`ui` **120/0** ✓、`app` **221/0/9** ✓、`capture` **545/0/80** ✓、`check` Finished ✓ **无死代码** ✓、`[pre-push] OK` ✓）：
`a_footer_click_is_answered_where_the_button_is_drawn` ✓（ui ✓ 三个按钮 × 四个 DPI ✓：四角与中心都命中 ✓，界外三点都不命中 ✓）、
`the_save_takes_the_directory_and_the_name_in_that_order` ✓、`the_local_date_and_time_are_filename_shaped` ✓（长度 ✓ 分隔符 ✓ 全是数字或 `-` ✓）。

**两条变异都真红** ✓（都**先断言替换命中 1 处** ✓，还原**逐字节一致** ✓）：
- 让 `footer_button_contains` **自己算一份矩形** ✗ ⇒ `a_footer_click_is_answered_where_the_button_is_drawn` **FAILED** ✓；
- 互换 `panel_values` 的**两个实参** ✗ ⇒ `the_save_takes_the_directory_and_the_name_in_that_order` **FAILED** ✓（`left: "SnapClip_[日期]" / right: "C:\shots"` ✓）。

**本批修的三处编译错** ✗（上一批死在中间态 ✓）：`paint_with_preview` 少第 8 个实参 ✓；`GetDpiForWindow` 缺 `unsafe` ✓（它在自由函数里，不在 WndProc 的 unsafe 块内 ✓）；三处**测试**用的 `WindowState` 字面量缺 `footer_widths` ✓。

---

#### 序号（A11 ✓ 已完成）

**规格** ✓（`:196` ✓ `:201` ✓）：`[序号]` 是"下一个序号" ✓；**模板含 `[序号]` 时，每次保存成功后 +1 并记住** ✓；撤销保存时回退 ✓。

**形状** ✓：`Sequence`（`apps/snapclip/src/save/assemble.rs` ✓）是一个**只有一个数**的类型 ✓，规则写成**一个纯函数** ✓：

```rust
pub fn after<T>(self, result: &Result<T, SaveError>, template: &str) -> Self {
    match result {
        Ok(_) if template::uses_sequence(template) => Self(self.0.saturating_add(1)),
        _ => self,
    }
}
```

⇒ 三条规则压在**一个 match** 里 ✓：成功才动 ✓、名字要了才动 ✓、取消与失败**都不动** ✓；"撤销保存时回退"因此**不需要 undo 步骤** ✓ —— 计数器**根本没动** ✓。

**谁持有它** ✓：`lib.rs` 在装 `save_opener` 时建**一个** `Arc<Mutex<Sequence>>` ✓ ⇒ 每次开面板把它 clone 进 `ShellOptions.sequence` ✓ ⇒ `WindowState` 存着 ✓ ⇒ 保存时读 ✓、保存后写回 ✓。面板**自己不留副本** ✗（一份数只有一个写者 ✓，与"控件表只有一个写者"同一原则 ✓）。

**证据** ✓（`app` **224/0/9** ✓，+3 ✓）：
`a_successful_save_advances_the_sequence_when_the_name_asks_for_it` ✓、`a_template_without_the_sequence_does_not_consume_a_number` ✓、`neither_cancelling_nor_failing_uses_up_a_number` ✓、`the_save_takes_the_directory_and_the_name_in_that_order` ✓（现在也断言序号从外面进来 ✓）。

**两条变异都真红** ✓（都先确认**替换命中 1 处** ✓，还原**逐字节一致** ✓）：
- 让失败也推进 ✗ ⇒ `neither_cancelling_nor_failing_uses_up_a_number` **FAILED** ✓（`a cancelled save wrote no file: left: 3 right: 2` ✓）；
- 去掉"名字要了才推进"这个条件 ✗ ⇒ `a_template_without_the_sequence_does_not_consume_a_number` **FAILED** ✓（`the name never asked for a number: left: 2 right: 1` ✓）。

**仍缺的一半** ✗：规格同一句里的"**模板也要记住**" ✗ —— 现在每次开面板都是默认模板 ✓ ⇒ 新缺口 **A11b** ✓，需要 `EDIT` 的 `EN_CHANGE` 把文本写回共享状态 ✓。

---

#### 底部状态行（A12 ✓ 已完成）

**问题** ✓：保存失败**只记日志** ✗ ⇒ 用户**看不见** ✗。而比"看不见"更糟的是：点「保存」后**无条件关面板** ✗（`PostMessageW(WM_CLOSE)` 在 `save_from_panel` 之后 ☑）⇒ **失败也把原因一起带走** ✗。

**demo 的两处错误位** ✓：`.field-msg`（**行头** ✓ `:335`–`:341` ✓，带修复胶囊 ✓）与 `.status.is-error`（**红色状态行** ✓ `:276` ✓）；本批**只做后者** ✓（A17/A18 记前者 ✓）。

**要画的** ✓：`.status` 是 footer 的**第一个子元素** ✓（`:675` ✓），`flex: 1` ✓ ⇒ 占三个按钮**剩下的那条** ✓；`--fs-status: 11px` ✓（`:37` ✓）；级别决定颜色 ✓（`is-error` ⇒ `--danger` ✓ `:276`、`is-warning` ⇒ `--warning` ✓ `:327`）。

**几何** ✓：`footer_status_rect`（`field.rs` ✓）= 从 `strip.left + FOOTER_PAD_X_DIP` ✓ 到 `copy.left - FOOTER_GAP_DIP` ✓，**band 与按钮相同** ✓（`.foot { align-items: center }` ✓ `:274` ⇒ 单行文字落在按钮的中线上 ✓）；宽度为负时**退化成空的** ✓ 而不是"右边缘在左边缘左边"✗。

**文案与级别** ✓：`status_of(&Result<SaveOutcome, SaveError>)`（`save/window.rs` ✓，**自由纯函数** ✓ —— 同一路数的第三个：`initial_text` ✓ `panel_values` ✓ 现在这个 ✓）。三条规则可见 ✓：
- **成功 ⇒ `None`** ✓（面板要关 ✓，没有行可读 ✓）；
- **取消 ⇒ `None`** ✓（`SaveError::Cancelled` 自己的注释写着"用户改主意了，不是错误"✓ —— 画成红色是**对事实的撒谎** ✗）；
- 其余 ⇒ 有话说 ✓，级别来自**消息自己的规则** ✓（`ValidationCopy::level()` ✓）⇒ "不拦保存的警告"与"拦住保存的错误"**不会同色**✓；`detail` 有就用它 ✓（模板带 `{…}` 比没用还糟 ✗）。

**行为改动** ✓：失败时**面板不关** ✓ + 重绘 ✓；成功才关 ✓。「每次点保存先把旧句子清掉」✓（否则会在一件即将成功的事上方挂着上次的失败 ✗）。

**证据** ✓（`ui` **132/0** ✓（+4）、`app` **230/0/9** ✓（+5）、`capture` 545/0/80 ✓、`check` Finished ✓ **无死代码** ✓）：
- `the_footer_status_takes_the_space_the_buttons_leave` ✓（四 DPI ✓：左=条带内边距 ✓、右=copy 左缘减一个 gap ✓、band 与按钮同 ✓）；
- `a_footer_status_that_would_be_negative_is_empty_rather_than_backwards` ✓；
- `the_footer_status_is_drawn_in_its_levels_colour` ✓（**2× 上采样** ✓：同一句 `文件名不能为空` 在 error 级 = `--danger` ✓、warn 级 = `--warning` ✓ —— **指名位置 + 颜色** ✓）；
- `a_panel_without_a_message_leaves_the_status_space_clean` ✓（**框内每个像素都是表面色** ✓ —— 这条让上一条有意义 ✗：若那条线**永远有墨**✓，颜色断言会在"装饰"✗ 而不是"回答"✓ 上通过 ✓）；
- app 侧五条 ✓：成功不说 ✓ / 取消不说 ✓ / 校验句与级别 ✓ / `detail` 胜出 ✓ / 没有键的失败也说 ✓。

**两条变异都真红** ✓（都**先断言替换命中 1 处** ✓，还原**逐字节一致** ✓）：
- 让取消也说 ✗ ⇒ `a_cancelled_save_says_nothing` **FAILED** ✓（`left: Some((Error, "cancelled")) / right: None` ✓）；
- 让警告用错误色 ✗ ⇒ 位图那条 **FAILED** ✓（`must be drawn in --warning: the closest pixel … is Px{b:118,g:117,r:241}` ✓ = 抗锯齿后的 `--danger` ✓）。

**一条被测试抓出来的真不一致** ✗✓：`field_colours` 的 `WARNING` 是 `[255,191,0]` ✗，而 demo 的 `--warning` 是 `rgb(240,179,90)` ✓ —— **两个黄** ✓。本条线用**设计稿的黄** ✓（`FramePalette::warning` ✓ + `warning_text` 笔刷 ✓），字段边框那个**不动** ✗（不在本批范围 ✓）⇒ 记入 **A22** ✓。

**一条工程账** ✗：绘制入口的**位置参数已到 13 个** ✓ ⇒ 记入 **A19** ✓（下一个给绘制加参数的批次先把它们收成结构体 ✓ —— 位置参数错一个**不会编译不过** ✓，同类型可以互换 ✗）。

### 收参数（本批）：`PaintInput`，以及它拦得住与拦不住的东西

**做了什么** ✓：`paint_to_dc_with_preview` 的 13 个位置参数、`paint_with_preview` 的 11 个 ✓ ⇒ 收成一个 `PaintInput` ✓（9 个命名字段 ✓ + `PaintInput::idle(dpi)` ✓ 给"其余都不显示"的调用者 ✓）。`PanelRenderer::paint_with_preview` 现在是 `(hdc, area, &input)` ✓。

**顺手删掉的** ✗（都只用于保留旧签名 ✓）：`paint_to_dc` ✓ `paint_to_dc_with_state` ✓ `PanelRenderer::paint` ✓ —— 以及**位图目标那条路径** ✓（`paint_to_target` ✓ `paint_to_target_with_preview` ✓）：全仓 `grep` **无任何调用者** ✗，而它的文档注释还写着"测试在用"✗（测试走的是内存 DC ✓）⇒ 留着就是"无用实现" ✓。

**一条行为上的收紧** ✗（**没有**调用者受影响 ✓）：目标那条路径原来把 `fields` **写死成 IDLE** ✗，现在从 `input.fields` 取 ✓ —— 两条路径的规则合成一条 ✓。

**它拦得住的** ✓ —— 变异：**漏掉一个字段** ✗ ⇒
```
error[E0063]: missing field `format_open` in initializer of `PaintInput<'_>`
```
⇒ 位置上**漏一个会静默移位** ✗ 的那类错，现在**编译不过** ✓。

**它拦不住的** ✗✓ —— 变异：把两个 `usize` 字段的**值**互换 ✗ ⇒ **`Finished`** ✓（编译通过 ✓）
⇒ 结构体拦的是**位置** ✗，不是**类型** ✗；而要拦类型得给每个量一个 **newtype** ✓ ⇒ 记入 **A23** ✓，本批**不做** ✗（它不是 A19 的要求 ✓，且会散到所有调用点 ✓）。

**没改测试** ✓：`ui` **132/0** ✓、`app` **230/0/9** ✓ —— 与改前**条数完全一致** ✓（只有**调用写法**从位置参数变成结构体字面量 ✓，**没有一条断言或期望被动过** ✓）。