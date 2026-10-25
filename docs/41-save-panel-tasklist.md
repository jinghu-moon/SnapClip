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

> 保存面板的视觉与交互必须**尽可能与 `docs/design/save-panel-demo-v3.html` 一致**，**扣除 docs/42 D13 记录的四条不可复现项**（圆角 12→系统≈8 dip、投影改用 DWM、模糊随材质一起放弃、**不用 DWM 材质 ⇒ 面板为不透明纯色**）。
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
| A5 | ~~**`保存后` 的两个 chip 未画** ✗~~ ✓ **已画**（`复制路径` ✓ `打开文件夹` ✓；高 24 ✓ 行内 gap 12 ✓ chip 间 8 ✓ 按 `:192` / `:302` / `:303` ✓，宽度与 label 宽度都**真的测量** ✓）；**点击未接** ✗ | 同上 ✓ | 交互批 ✓ |
| ~~A24~~ | ~~**路径框没有默认值** ✗ ⇒ 面板一打开就是红框~~ ✓ **已修**：`save/dirs.rs::desktop()` 从 shell 取**真实的桌面** ✓（`SHGetKnownFolderPath` ✓，不是抄 demo 那句 `C:\Users\阿伟\Desktop` ✗ `:544`）；`ShellOptions.directory` ✓ ⇒ `initial_text(Path)` ✓；开发入口也走同一条 ✓ | — | 已完成 ✓ |
| ~~A6~~ | ~~**尺寸行还有两个模式钮（`数值`/`百分比`，`:585`–`:586` ✓）与一个重置钮（`:589` ✓）** ✗~~ ✓ **已画出**（`.size-mode-btn` 的几何来自 `:226`/`:227` ✓，重置钮用 `refresh` 图标 ✓，宽度**真的测量了文字** ✓）⇒ 仍**只画不接** ✗ | — | 已完成绘制 ✓；点击属交互批 ✗ |
| ~~A7~~ | ~~**尺寸格的 `宽`/`高` 与单位 `px` 未画** ✗~~ ✓ **已画出**（三段几何同源 ✓：`size_cell_label_rect` / `size_cell_input_rect` / `size_cell_unit_rect` ✓，第二格吸收余数 ✓） | — | 已完成 ✓ |
| ~~A8~~ | ~~**数值字段的等宽字体未用** ✗~~ ✓ **已用**：`assets/font/FiraCode-Regular.ttf`（283 KB）取子集到 **3.3 KB**（`subfont/build_number_font.py` ✓，16 个字符 ✓）⇒ 内嵌在 `snapclip-ui`（`panel/fonts.rs` ✓）⇒ 尺寸格的 `EDIT`（`WM_SETFONT` + `ES_RIGHT` ✓）与 `px` 单位（`number_format` ✓）都用它；门禁 `the_embedded_number_subset_covers_the_numbers_the_panel_shows` ✓ | — | 已完成 ✓ |
| ~~A9~~ | ~~**`.link-hub` 之外的两个 ghost 按钮未画** ✗~~ ✓ **已画**（`更改` ✓ 与文件夹图标 ✓，`:545`/`:546` ✓）；**点击未接** ✗ ⇒ 见 A31 | 宽度**真的测量** ✓（`measure_path_change` ✓，控制与绘制同源 ✓） | 交互批 ✓ |
| ~~A10~~ | ~~**格式下拉未画** ✗~~ ✓ **已画并可点**：按钮（`--fmt-w: 80` ✓ `:49`，等宽字 ✓ 箭头 ✓）在名字行右端，菜单（`156` ✓ `:180`）右对齐它、三项 `.PNG`/`.JPG`/`.WebP` ✓ `:717`，当前项打勾 ✓ `:184`，展开时描边变蓝 + 箭头**翻转** ✓ `:175`/`:178`（翻转在像素上做 ✓，见 `icons::flipped_180` ✓）；选中项**送进保存** ✓（`format_of_index` ✓ `save/window.rs` ✓）。缺口见 A15/A16 | — | 已完成 ✓ |
| ~~A11~~ | ~~**`[序号]` 固定为 1** ✗~~ ✓ **已做**：`Sequence`（`save/assemble.rs` ✓）由 shell 的 `Arc<Mutex<Sequence>>` **跨面板共享** ✓（面板每次捕获一个 ✓），只在**保存成功且名字含 `[序号]`** 时 +1 ✓（见文末「序号」一节 ✓） | — | 已完成 ✓ |
| ~~A11b~~ | ~~**名字模板不跨面板记住** ✗~~ ✓ **已完成**：模板与序号收进同一个 `Remembered` ✓（`save/assemble.rs` ✓），由 shell 的 `Arc<Mutex<Remembered>>` 跨面板共享 ✓；名字框的初值来自它 ✓（默认 `SnapClip_[日期]` ✓ = 规格 `:68` = demo `:556` ✓）；`EN_CHANGE` 只写**名字框** ✓ 的文本 ✓（见文末「模板持久化」一节 ✓） | — | 已完成 ✓ |
| ~~A12~~ | ~~**保存失败只记日志** ✗~~ ✓ **已做**：底部的**状态行** ✓（`.status` ✓ `:275`/`:675`，`--fs-status: 11px` ✓ `:37`）显示 `status_of` 的结果 ✓ —— 校验错误用 `copy` 的句子 ✓（有 `detail` 用 `detail` ✓）、级别决定颜色 ✓（error `--danger` ✓ `:276`，warn `--warning` ✓ `:327`）、**取消与成功都不显示** ✓；**失败时面板不再关闭** ✓（关掉就把原因带走了 ✗） | — | 已完成 ✓ |
| A13 | **悬停态未做** ✗（`.close:hover` ✓ `:97` ✓、`.btn:hover` ✓、`.fmt-btn:hover` ✓ `:174`） | 需要鼠标进入/离开消息 ✓ | 交互批 ✓ |
| A15 | **格式菜单每项右侧的预估体积未画** ✗（`.fmt-item .val` ✓ `:186`，`:203` 要求"当前尺寸与质量下的预估体积"✓）—— 要先有体积模型 ✓ | `fmt_item_rect` 已给出每行矩形 ✓ | 预估批 ✓ |
| A16 | **切换格式的连带变化未做** ✗（`:204`：质量行 ✓ 高级选项 ✓ JPG 填充行 ✓ 与重新估算 ✓）—— 后两者**还没画** ✓ | — | 高级项批 ✓ |
| A17 | **行头 `.field-msg` 与修复胶囊未做** ✗（`:335`–`:341` ✓，`repair()` 已在 `copy.rs` 里备好 ✓）—— 失败目前只在**底部一行** ✓，指出**哪个字段**要等它 | 字段级提示需要一个"哪个字段出错"的来源 ✓ | 错误提示批其二 ✓ |
| ~~A18~~ | ~~**字段边框变红未做** ✗~~ ✓ **已做**：`copy::ValidationCopy` 的**键**经 `panel::problem_of` 映射成 `(FieldId, FieldState)` ✓（`Stem*` ⇒ 文件名 ✓、`Path*`/`DirectoryMissing`/`NoWritePermission`/`NotEnoughSpace`/`PathTooLong` ⇒ 路径 ✓；`level()` 决定 Error/Warning ✓）⇒ 保存失败时只有**相关字段**变红 ✓，其余不动 ✓（位图断言 ✓）；**焦点不再能抹掉它** ✗ —— `FieldStates::set_focus` 遇到消息态就返回 ✓（democratic：`:333` 在 `:169` 之后 ✓ ⇒ 消息赢 ✓） | 已解决 ✓ | 已完成 ✓ |
| ~~A19~~ | ~~**绘制入口的实参已经 13 个** ✗~~ ✓ **已做**：收成 `PaintInput` ✓（字段有名 ✓ —— `quality` ✓ `format` ✓ `format_open` ✓ `fill_surface` ✓ `footer_widths` ✓ `status` ✓ `preview` ✓ `fields` ✓ `dpi` ✓）；`PanelRenderer::paint_with_preview` 只剩 `(hdc, area, input)` ✓；**只用于保留旧签名的三个壳子已删** ✗（`paint_to_dc` ✓ `paint_to_dc_with_state` ✓ `PanelRenderer::paint` ✓），**位图目标那条路径也删了** ✗（`paint_to_target` ✓ `paint_to_target_with_preview` ✓ —— 全仓**无任何调用者** ✓，其文档注释还写着"测试在用"✗，而测试走的是内存 DC ✓） | — | 已完成 ✓（**同名类型的值互换仍拦不住** ✗ ⇒ 见 A23 ✓） |
| A20 | **三种失败的状态文案是开发者语** ✗ —— `Capability` ✓ `UnsupportedFormat` ✓ `Io` ✓ 走的是 `SaveError::message()` ✓，那是 `Debug`/英文短句 ✓（ascii ✓ 所以能画 ✓），不是给用户看的措辞 ✓ | 规格 §8.2 没有这三条的句子 ✓ ⇒ 要先定文案 ✓（新增句子会牵动字形子集 ✓） | 文案批 ✓ |
| A21 | **`detail` 可能引用用户输入** ✗ —— 例如 `illegal_chars("a*b")` ✓ 与 `reserved_name(名字)` ✓ ⇒ 用户打的字**不一定在子集里** ✗ ⇒ 会**回退系统字体** ✓（能显示 ✓，字形不是我们的 ✓） | 子集装不下任意输入 ✓；要么限定显示 ✓、要么接受回退 ✓ —— **先记录** ✓ | 文案批（与 A20 一起决定）✓ |
| ~~A22~~ | ~~**两个黄色** ✗~~ ✓ **已做** —— **并纠正了本行的两处错误** ✗✓：① demo **有**字段警告边框的规则 ✓（`.text-input.is-warn { border-color: var(--warning) }` `:334` ✓，`inp.classList.toggle('is-warn', …)` `:1425` ✓，尺寸格还有 `.cell.is-warning` `:241`/`:243` ✓）—— 前一版说"没有"✗ 是**错的** ✓ ② 那个黄不是"推断"的 ✓ ⇒ 现在 **`panel::WARNING_RGBA` 是唯一来源** ✓（`field_colours` ✓ 与 `FramePalette::warning` ✓ 都读它 ✓）⇒ 边框 (`:334`) ✓ 状态行 (`:327`) ✓ 冲突链接 (`:328`) ✓ **同色** ✓ | 已解决 ✓ | 已完成 ✓ |
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

**没改测试** ✓：`ui` **132/0** ✓、`app` **230/0/9** ✓ —— 与改前**条数完全一致** ✓（只有**调用写法**从位置参数变成结构体成员 ✓，**没有一条断言或期望被动过** ✓）。

---

## A18 + A22：字段边框说出"哪里错了"，两个黄收成一个

**A18 的形状** ✓：`copy::ValidationCopy`（**键** ✓）⇒ `panel::problem_of(copy) -> Option<(FieldId, FieldState)>` ✓ —— **纯函数** ✓、**不活在消息处理里** ✗：

- `Stem*`（空 ✓ 非法字符 ✓ 保留名 ✓ 尾点/尾空格）⇒ **文件名**字段 ✓；
- `Path*` ✓ `DirectoryMissing` ✓ `NoWritePermission` ✓ `NotEnoughSpace` ✓ `PathTooLong` ⇒ **路径**字段 ✓；
- 级别由 `copy.level()` 决定 ✓：`Warn` ⇒ `FieldState::Warning` ✓（`DirectoryMissing` ✓ `StemTrailingDotOrSpace` ✓）、其余 ⇒ `Error` ✓。

于是保存失败时：**状态行说发生了什么** ✓、**边框说发生在哪个字段** ✓ ⇒ 两处信息**不重复也不矛盾** ✓。

**一条被测试逼出来的规则** ✗✓：焦点通知原来会把 `Error`/`Warning` **覆盖成 `Focused`/`Idle`** ✗ ⇒ 用户一点进出错的字段 ✓，红边框就**消失** ✓ —— 而那正是他要改它的时候 ✗。修法不是加一个"当前聚焦字段"的第二份状态 ✗（那会引入两个写者 ✗），而是让**焦点在一个有消息的字段上不生效** ✓：

```rust
pub fn set_focus(&mut self, id: FieldId, focused: bool) {
    if self.state_of(id).is_message() { return; }
    self.set(id, if focused { FieldState::Focused } else { FieldState::Idle });
}
```

⇒ 与 CSS 一致 ✓：`:333` 的 `.is-error` 在 `:169` 的 `:focus` **之后** ✓ ⇒ 消息赢 ✓ —— 所以这不是"我们定的"，是**设计稿定的** ✓。

**清空** ✓：每次保存尝试开始 `clear_messages()` ✓（与清状态行同一处 ✓）⇒ 旧句子不会活过一个新尝试 ✓。

**A22 的两处更正** ✗✓（本批推翻了自己上一版的两条说法）：

1. demo **有**字段警告边框 ✓（`:334` ✓，`:1425` 还把 `is-warn` 挂在输入上 ✓，尺寸格另有 `.cell.is-warning` ✓ `:241`/`:243`）⇒ 上一版写"demo 没有"✗ 是**错的** ✓；
2. 那个黄**不是**推断的 ✓ ⇒ 现在 `panel::WARNING_RGBA = [240,179,90,255]` ✓ 是**唯一字面量** ✓：`field_colours(Warning)` ✓ 与 `FramePalette::warning` ✓ 都读它 ✓ ⇒ 边框 ✓ 状态行 ✓ 冲突链接 ✓ **三处同色** ✓。

**变异（三条，全红 ✓，还原逐字节一致 ✓）**

| 变异 | 红的信息 |
| --- | --- |
| 警告黄改回 `255,191,0` ✗ | `warning border is --warning (:334, :23)` ✓ |
| 焦点覆盖消息（删掉 `is_message` 守卫）✗ | `focus must not overwrite a message about the value (:333 over :169)` ✓ |
| `Level::Warn` 当成 `Error` ✗ | `DirectoryMissing keeps its level` ✓ |

⚠️ **第三条第一次没跑成** ✗：替换锚点在我自己的测试里**也命中了一处** ✓ ⇒ 命中数 2 ✓ ⇒ 守卫（"必须正好 1 处"）**跳过**了 ✓ —— 这正是 `docs/41` 变异清单第 1 条要防的"变异没发生" ✓；换成带上下文的锚点才拿到红 ✓。

**门禁** ✓：`ui` **136/0** ✓（132 ⇒ +4）· `app` **230/0/9** ✓ · `capture` **545/0/80** ✓ · `icon-bake` ✓ · `check --workspace --all-targets` Finished ✓ **无 `never used`** ✓。

---

## 模板持久化（A11b ✓）

规格 `:201` 的一句话把两件事放在一起：**"模板与序号随输入实时记住"** ✓ —— 序号那半在「序号」一节 ✓，这里是模板那半 ✓。

### 形状：一个 `Remembered`，两个写者各管一半 ✓

```rust
pub struct Remembered { pub template: String, pub sequence: Sequence }
```

- **为什么收进同一个结构体** ✓：两者的**存活期与所有者相同** ✓（都比一个面板长 ✓、都由 shell 持有 ✓、都出自规格的**同一句** ✓）⇒ 一个 `Arc<Mutex<Remembered>>` 而不是两个句柄 ✓；
- **写者仍然分开** ✗：模板由 `EN_CHANGE` 写 ✓（跟着输入 ✓），序号只在**保存成功且名字含 `[序号]`** 时移动 ✓（`Sequence::after` ✓）⇒ 保存那一步**只写回 `sequence`** ✓，**不碰模板** ✓（有一条测试专钉这一点 ✓ —— 否则保存会把手打字的内容退回到"保存开始时"的版本 ✗）。

### 默认值是规格的，不是别处的 ✓

`DEFAULT_TEMPLATE = "SnapClip_[日期]"` ✓ —— 规格 `:68` ✓ 与 demo `:556` ✓ **一致** ✓。

⚠️ **这里修了一处不一致** ✗：名字框原先是 `捕获` ✗，而规格与 demo 都是 `SnapClip_[日期]` ✓ ⇒ 现在由 `DEFAULT_TEMPLATE` 给出 ✓。

### 哪条通知算"改过" ✓

```rust
fn note_edit(remembered, id, is_change, edited) -> bool
    if !is_change || id != FieldId::Name { return false }
    remembered.remember_template(edited)
```

- **`EDIT` 置文本也会触发 `EN_CHANGE`** ✗ ⇒ 面板装自己的初值时也会走到这里 ✓ ⇒ 规则里"**文本没变就不写**"✓ 让这一趟成为**无操作** ✓（有测试 ✓）；
- **路径框是另一个 `EDIT`** ✗ 而且**文本也是字符串** ✓ ⇒ 只按"某个文本框变了"匹配就会把**目录**塞进下一张面板的名字框里 ✗ ⇒ 规则**点名 `FieldId::Name`** ✓（有测试 ✓）。

### 证据

| 测试 | 钉住什么 |
| --- | --- |
| `a_first_run_starts_with_the_specs_template_and_the_specs_number` ✓ | 默认值是 `SnapClip_[日期]` ✓ 与序号 1 ✓ |
| `an_edited_template_is_taken_and_an_unchanged_one_is_not` ✓ | 改了就写 ✓、没变就不写 ✓、**清空也算用户的决定** ✓（合法性归保存判断 ✓） |
| `moving_the_number_leaves_the_template_alone` ✓ | 保存**只动序号** ✓，不碰模板 ✓ |
| `only_a_change_in_the_name_box_writes_the_template` ✓ | **路径框不算** ✓、**焦点不算** ✓ |
| `the_next_panel_opens_with_the_template_the_last_one_was_left_with` ✓ | 跨面板 ✓（并断言两次**不同** ✓ —— 否则"都打开成默认"也能通过 ✗） |
| `installing_the_initial_text_is_not_a_change` ✓ | 装初值那一趟是**无操作** ✓ |
| `the_name_row_starts_with_the_template_it_was_given` ✓ | 记忆的模板**优先于**默认 ✓ |

**两条变异都真红** ✓（各自**先断言替换命中 1 处** ✓，还原后**逐字节一致** ✓）：

| 变异 | 红的信息 |
| --- | --- |
| `note_edit` 不看字段 id ✗ | `路径那一格 must not become the template` ✓（`only_a_change_in_the_name_box_writes_the_template`） |
| `remember_template` 永不写回 ✗ | `the next panel opens with the remembered template` ✓ |

### 一条诚实边界 ✗

`note_edit` 与 `remember_template` 是**规则** ✓，都被断言了 ✓；而"**消息处理里确实调用了它们**"✗ **没有测试覆盖** ✗ —— 它需要一个真窗口 ✓（桌面测试 ✓）。这一条只能靠**编译**（`note_edit` 是唯一入口 ✓）与**人眼**（打一个字、关掉、再开一次 ✓）确认 ✓。

⇒ 真机验证 ✓：开面板 ✓ → 把名字改成 `我的_[序号]` ✓ → 点「取消」关掉 ✓ → 再按一次 `Ctrl+S` ✓ ⇒ 名字框应是 **`我的_[序号]`** ✓（而不是 `SnapClip_[日期]` ✗）。

---

## 面板与设计稿的差距：第一批（A24 + A5）

用户指出"保存面板与原型还有很大差距" ✗，并把它归到两件事：**空的地方没画** ✗ 与 **选型差异**（普通窗口 + 原生 `EDIT` ⇒ DWM 圆角 8 ✓ 无自绘模糊 ✓ 无自定阴影 ✗，见 D13 ✓）。

### 做了什么

| 项 | 内容 |
| --- | --- |
| **A24 默认目录** ✓ | 新增 `apps/snapclip/src/save/dirs.rs` ✓：`desktop()` 用 `SHGetKnownFolderPath(FOLDERID_Desktop)` 取**真实桌面** ✓，并用 `CoTaskMemFree` 释放 ✓（为此给 app 的 `windows-sys` 加 `Win32_System_Com` ✓，注释写明理由 ✓）。`ShellOptions.directory` ✓ ⇒ `initial_text(FieldId::Path, …)` ✓；**开发入口也走同一条** ✓。**不再抄 demo 的 `C:\Users\阿伟\Desktop`** ✗（`:544` 是一个人的桌面 ✓，不是默认值 ✓） |
| **A5 保存后两个 chip** ✓ | `field.rs`：`AFTER_CHIP_*` 常量（高 24 ✓ `:192`、padding 10 ✓、label→chip 12 ✓ `:302`、chip 间 8 ✓ `:303`）+ `AFTER_CHIP_WORDS` ✓ + `after_chip_width` ✓ + `after_chip_rect` ✓；`frame.rs` 在**标签循环之后**画两个胶囊 ✓（`--line-soft` 描边 ✓ `--muted` 文字 ✓ 未选中 ⇒ **不填底** ✓）；`form_drawn_strings()` **收进这两个词** ✓ ⇒ 字形门禁会替我们盯着 ✓ |
| **`Glyph` 新变体** ✓ | `Folder` ✓ `FolderOpen` ✓ `Braces` ✓（三个图标早已烘好 ✓，`ICON_COUNT` 9 → 12 ✓）—— 为 A9（路径行的两个 ghost 按钮）与魔法棒准备 ✓，**本批尚未画它们** ✗ |

### 证据

- **字形门禁先红** ✓：`no glyph for '夹' (U+5939), drawn by "打开文件夹"` ⇒ 重建子集 ⇒ **292 → 293 codepoints** ⇒ 绿 ✓（`subfont/subset.ps1` ✓）。
- `ui` **137/0** ✓（新增 `the_after_save_switches_sit_on_the_label_line_after_the_label` ✓ 四 DPI ✓ 断言：首个 chip 距 label 恰好一个 gap ✓、两 chip 相距恰好一个 gap ✓、高 24 ✓、在 22 dip 标签行上**居中（±1 px）** ✓）
- `app` **238/0/9** ✓（新增 `the_path_row_starts_where_the_shell_says_and_nowhere_else` ✓ 与 `dirs` 的 `the_desktop_is_absolute_or_empty_and_never_ends_with_a_separator` ✓）

### 两条诚实边界 ✗

1. **chip 的像素没有断言** ✗ —— 它们的 x 取决于 `保存后` 这个词的**测量宽度** ✓，而测试装置手里没有设备 ✓；几何由上面那条测试守着 ✓，"看得见"只能人判 ✓。**不假装有** ✗。
2. **chip 点击未接** ✗（A5 只剩这一半 ✓），并且它们**看起来像开关** ✓ ⇒ 属"画了不接"✓，按规矩记在这里 ✓。

### 还没画的（按"看起来像坏了"✗ 排序）

| # | 缺口 | demo |
| --- | --- | --- |
| ~~A9~~ | ~~路径行的 `更改` ✓ 与**打开文件夹**两个 ghost 按钮~~ —— **已接** ✓（文件夹选择器 ✓ 打开资源管理器 ✓，见本批） | `:545` / `:546` ✓ |
| ~~A25~~ | ~~路径行的三个**快捷 chip**~~ —— **已接** ✓（点了就改目录 ✓，取系统的真实文件夹 ✓） | `:548` ✓ / `:1008` ✓ |
| ~~A26~~ | ~~名字行里的**魔法棒**~~ —— **已接** ✓（菜单 + 在光标处插入 ✓，见本批） | `:557` ✓ |
| A27 | 质量行的 `.adv-btn`（图标**未备** ✗ —— 那是 sliders 图形 ✓，`assets/icon/` 里没有 ✗ ⇒ **要么下载要么不画** ✗） | `:630` ✓ |
| A28 | 预估大小的**数值** ✗（`227.31 KB` 是动态值 ✓ ⇒ 要先有体积模型 ✓ ⇒ 现在**留空** ✓，**不编造** ✗） | `:668` ✓ |
| A29 | 尺寸行的 `.size-presets`（100/75/50/25 ✓）与 `.warn-hint` ✗ | `:614` / `:621` ✓ |
| A30 | 质量行的 `限制体积` 开关 ✓、`PNG 无损` 提示 ✓、`不超过 … KB` ✓、`.fill-row` 透明填充 ✗ | `:631` / `:638` / `:645` / `:655` ✓ |
| A31 | 预估行的 `.enc-state` / `.enc-bar`（编码进度 ✓） | `:667` ✓ |
| A32 | 变量菜单每项右侧的**当前值** ✗（`:221` 的 `.val`）—— 要实时值 + 字形 | `:221` ✓ |
| A33 | `更改` 对话框的**初始目录**是系统默认，不是当前路径 ✗ | — |

⇒ 观感差距的**八成**在这些"空位"✓；**选型那部分**（圆角 8 vs 12 ✓ 无自绘模糊 ✓ 无自定阴影 ✗）改不了 ✓，见 D13 ✓。

## 面板与设计稿的差距：第二批（A9 + A25 + 两条恒真断言）

| 项 | 结果 |
| --- | --- |
| **A9 路径行两钮** ✓ | `更改`（`.btn-ghost` ✓ `:189`，宽 = 词 + 12×2 ✓ 由 `measure_path_change` 测一次 ✓）与**方形文件夹钮**（`32` ✓ `:205`，图标 16 ✓ `:207`）✓；**输入框让出它们**（`path_field_rect` ✓，`EDIT` 因此变窄 ✓ 不会跑到按钮下面 ✓） |
| **A25 三个快捷 chip** ✓ | `桌面` ✓ `图片` ✓ `下载` ✓（`:739`–`:741` ✓）；`.chip` 的规格与 after 行**共用**（高 24 ✓ padding 10 ✓ 圆角=半高 ✓ `--line-soft` 边 ✓ `--muted` 字 ✓）；每个带 `folder` 图标 12 ✓ 与 4 的间隙 ✓（`:192`、`:195`、`:1008`） |
| **行高 62** ✗ | 路径行的内容**不再是 32**：输入框 + `.form-row { gap: 6px }` + chip 24 ⇒ `Row::Path::content_height` = 62 ⇒ **下面所有行下移 30** ✓；两条按旧间距钉住的测试按**预期正确变化**改 ✓（并说明理由 ✓） |
| **两条恒真断言** ✗✓ | `the_close_glyph_…` 曾用**同一个像素与自己**比较（容差 0）⇒ 永远不会红 ✓；`the_variable_button_…` 只问"框里有非底色墨" ⇒ 任何图标都过 ✓。现在前者断言**绝对的混合色**（`--muted` 叠 surface ✓）+ 四角仍是表面 ✓；后者断言**绝对色** + 字形**居中且不触边** ✓ |

### 一条被恒真断言抓出来的**真 bug** ✗✓

`tint_rgba_to_premultiplied_bgra` 把**源 alpha** 乘进 RGB ✓，却把 **`colour[3]` 丢了** ✗ ⇒ 每个图标都按**满不透明**画 ✓：`--muted`（62%）显示成 100% ✗。改成 `src[3] × colour[3]` ✓ 后，关闭钮与魔法棒才真正是设计里的灰 ✓。**恒真断言存在的意义**：它让这个 bug 藏了整整一批 ✓；一改成绝对色 ✓ 立刻暴露 ✓（`expected 152,155,161 / saw 232,236,242` ✓）。

### 形状断言的**限度**（如实记 ✗）

"换一个图标必红"这条**没做到** ✗：变异"把 `Braces` 画成 `Folder`"**仍然绿** ✓ —— 在 32 像素上，兜底形状（居中、不触边、有色）对这两个图标都成立 ✓。**能做到的是**：色不对必红 ✓（改颜色 ✓、丢 alpha ✓）、尺寸/位置错必红 ✓（改 `GHOST_ICON_ONLY_DIP` ✓）、块状填充必红 ✓。**做不到的是**：分辨同尺寸的另一个图标 ✗ —— 那需要与图标自身的像素做**逐点**对照 ✓，而 96→32 的下采样会让单个像素不可靠 ✓ ⇒ **不做，记在这里** ✓。

### 新增缺口

| # | 缺口 | 归属 |
| --- | --- | --- |
| **A31** | 路径行两钮与三个 chip **只画不接** ✗（点击未接 ✓；`图片`/`下载` 两条路径**也还没取** ✗ —— 没有消费者就不加 ✓ 与"不留无用实现"一致 ✓） | 交互批 ✓ |

## 让已有控件真的生效（尺寸 / 格式 / 质量）

用户的原话：「不能只实现 UI，功能也要实现」。先核实了四条，再动手：

| 控件 | 核实到的旧状态 | 现在 |
| --- | --- | --- |
| **尺寸格** | `save_from_panel` 把 `s.preview` 的选中尺寸直接交给 `save_panel` ✗ ⇒ 改动两个格子**对输出没有任何影响** ✗ | **生效** ✓：保存时读两个 `EDIT`，按 `output_size` 得到输出尺寸，`scale::scaled` 在编码前重采样一次 |
| **格式菜单** | 选 `.JPG` / `.WebP` 会走到 `bands::save_bands` 的 `UnsupportedFormat` ✗ ⇒ **保存直接失败** | **生效** ✓：非 PNG 走「PNG 母版 → `transcode::to_bytes` → `commit_bytes`」，仍是同一处原子提交 |
| **质量分段器** | `PanelValues` 里**没有** `quality` ✗ ⇒ 选档**不传**给编码器 ✗ | **生效** ✓：`quality_of_tier`（`docs/42` D8 的 **92 / 75 / 50**）经 `PanelValues.quality` 进转码器；只有 JPEG 花它（PNG 是拷贝、本项目 WebP 无损 ✓） |
| **数值 / 百分比 模式** | `SIZE_MODE_CHOSEN` 是**常量** ✗ ⇒ 没有消费者 ✗ | **仍未接** ✗（见下） |
| **重置钮** | 未接 ✗ | **仍未接** ✗（见下） |

新增模块与抉择：

- `apps/snapclip/src/save/scale.rs` ✓ —— `parse_size` / `output_size` / `scaled`。**缩小用面积平均、放大用双线性**：不这样，双线性缩小会丢像素（文字闪），面积平均放大会退化成最近邻（台阶）。两遍分离，中间只有一行输出的宽度。
- `apps/snapclip/src/save/transcode.rs` ✓ —— `to_bytes(png, format, quality)`：母版落到临时文件（转码器从路径读）、转码进内存、**无论成败都删掉母版** ✓。
- `save_panel` 收 `Frame { bgra, source, output }` ✓ —— 两个 `Size` 同类型 ⇒ **用具名字段而不是位置参数** ✗（与 `initial_text` / `panel_values` / `status_of` 同一类教训）。

证据：`253 passed` ✓（`app`，含 **8 条缩放** ✓ + **4 条转码** ✓ + **2 条集成** ✓）；`ui 142/0` ✓；`check --workspace` Finished ✓；**死代码 0** ✓。变异四例都真红：

```
输出尺寸写回选区   ⇒ the_panels_size_is_the_size_of_the_file FAILED（"the file is the size the panel asked for"）
格式恒走 PNG 路径  ⇒ the_chosen_format_is_the_file_that_lands FAILED
档位恒为第一档     ⇒ the_quality_tiers_are_the_recorded_numbers FAILED
质量恒 92          ⇒ the_quality_reaches_the_encoder FAILED
```

仍未接的两件（**不是遗漏** ✗）：`数值` / `百分比` 与**重置钮**都需要点击接线，而模式切换还要把所选档位穿过 `PaintInput`/`Frame` 才能在画面上体现 ✓ ⇒ 与「质量行」「高级钮」同属**交互批** ✓。**模式对保存语义的影响**（`数值` 是像素、`百分比` 是相对原图的百分比）也一并留到那批 ✓，因为只做一半会出现"切了模式但数字含义没变"✗ —— 那比不做更容易误读。

### 点击接线：路径行三处、魔法棒、底部复制（本批）

**做了什么** ✓

| 处 | 现在点了会怎样 |
| --- | --- |
| 三个快捷 chip ✓ | 把**系统真实的**桌面/图片/下载写进路径框 ✓（`dirs.rs` 的 `desktop`/`pictures`/`downloads` ✓）—— 不是 demo 的示例路径 ✗ |
| `更改` ✓ | 打开**文件夹选择器** ✓（`SHBrowseForFolderW` ✓ —— 见下面的取舍 ✓）；取消 ⇒ **什么都不改** ✓ |
| 方形文件夹钮 ✓ | 用 `ShellExecuteW` **打开资源管理器** ✓；路径不是目录 ⇒ **记日志、不动窗口** ✓ |
| 魔法棒 ✓ | 弹**变量菜单** ✓（五项、逐字来自规格 `:192`–`:198` ✓）；点一项 ⇒ **插到光标处** ✓（有选区则替换 ✓）⇒ 光标落在变量后 ✓ + **焦点留在输入框** ✓ |
| 底部「复制」✓ | 真的写剪贴板 ✓，尺寸走**与保存同一条规则** ✓（`scale::output_size` + `scale::scaled` ✓） |

**三个必须记下的取舍** ✗

1. **`SHBrowseForFolderW` 而不是 `IFileOpenDialog`** ✗ —— 前者是旧对话框 ✓，但 app 通过 `windows-sys` 取 Windows ✓，后者要**手写 COM 接口定义** ✓ 且要 `CoInitialize` ✓ ⇒ 为一个按钮加那些是**多出来的实体** ✓（奥卡姆 ✓）。代价 ✗：对话框是老样式 ✓。
2. **菜单会盖住原生输入框** ✗ —— 父窗口的画**永远在子窗口下面** ✓ ⇒ 菜单打开时，**被它压住的那几个 `EDIT` 会被隐藏** ✓，关掉再显示 ✓。判断"压住哪些"是**纯函数** `fields_under` ✓（带测试 ✓），不是两份手写判断 ✗。
3. **变量菜单的"当前值"列没画** ✗（demo `:221` 的 `.val` ✓）—— 那要 `[日期]`/`[序号]` 的**实时值** ✓ ⇒ 会牵动字形子集与状态 ✓ ⇒ 单列一批 ✓。

**证据** ✓

- 纯逻辑：`insert_variable`（光标 / 选区 / 越界 / **UTF-16 单位** ✓）、`rect_contains` 半开区间 ✓、`fields_under` 覆盖判定 ✓、菜单几何四种 DPI ✓、每档命中自己 ✓、路径两钮互不顶替 ✓；
- 复制：`RecordingCopy` 记下 `(w, h, 字节数)` ⇒ 断言**一次、尺寸对、字节全** ✓；
- 快捷键：`shortcut_directory(3)` ⇒ `None` ✓（不是"第四个别名"✗）。

**仍缺** ✗：变量菜单每项的**当前值** ✓（新缺口 A32）；`更改` 对话框的**默认目录**取的是系统默认而不是当前路径 ✓（A33）。

### 键盘与两条误导：`Enter` 保存、PNG 不显示质量档、四个尺寸预设（本批）

三件都不是"画东西"✗ —— 两件是**行为**✓，一件是**去掉误导**✗。

**① `Enter` 保存（`docs/design/save-panel-demo-v3.html:686` 的 `title="保存 (Enter)"`）**

核实结论 ✗✓：**改之前完全收不到** ✗。消息循环只有 `GetMessageW` + `TranslateMessage` + `DispatchMessageW` ✓，没有 `IsDialogMessageW` ⇒

- 单行 `EDIT` **自己吃掉** `Enter` ✓（还会响一声 ✓）⇒ 父窗口根本收不到 `WM_KEYDOWN` ✗；
- 于是 `Esc` 也只有在**面板自己持有焦点**时才有用 ✗（在输入框里按 `Esc` 是没反应的 ✗）。

修法 ✓：消息循环里先问 `IsDialogMessageW(hwnd, &message)` ✓（返回非零 ⇒ `continue` ✓），面板补 `WS_EX_CONTROLPARENT` ✓（**不补它，`IsDialogMessage` 不会下沉到子控件** ✗）。于是：

- `Enter` ⇒ `WM_COMMAND(IDOK=1)` ✓、`Esc` ⇒ `IDCANCEL=2` ✓ ⇒ 由**纯函数** `shortcut_command` ✓ 映射成 `Shortcut::{Save, Cancel}` ✓；
- 顺带 `Tab` 从此能在四个字段间走 ✓（`WS_TABSTOP` 早就有 ✓，缺的就是这一环 ✓）；
- `IsDialogMessage` 会给**焦点控件先拒绝的机会** ✓ ⇒ 将来到多行编辑框时，换行仍然是换行 ✓。

保存的两条入口（`Enter` 与底部的按钮）现在共用 `attempt_save` ✓ ⇒ **不会有一天两处对"失败后关不关面板"意见不同** ✗✓。

**② PNG 不显示质量分段器，改显示「无损」提示（`:638`、`:639`、`:1096`、`:1097`）**

demo 有两条**行为**规则而我们只画了控件 ✗：`:639` 给分段器 `display:none` ✓、`:1097` 只在**非 PNG** 时显示 ✓；`:638` 的提示文字只在 PNG 时出现 ✓（`:1096`）。

- 唯一的判定是 `quality_segment_shown(format)` ✓（`FMT_PNG_INDEX = 0` ✓，`.PNG` 在 `FMT_OPTIONS` 的第 0 位 ✓）；**画**与**命中**都问它 ✓ ⇒ 看不见的控件**点不动** ✗✓（否则点在空白上会改一个屏幕上看不到的设置 ✗）；
- 提示色 `.hint-text { color: rgba(232,236,242,.56) }`（`:262`）⇒ 我们的笔刷不透明 ✓ ⇒ `quality_hint_over_surface` 合成为 `(145,142,142)` ✓（有断言 ✓）。

**③ 四个尺寸预设（`:614`–`:619`、`:258`、`:259`）**

`100% / 75% / 50% / 25%` ✓，点了**真的改尺寸** ✓ —— 走 `size_preset_size` ✓（四舍五入 ✓、**最小 1 像素** ✓），写进**两个尺寸格的原生控件** ✓ ⇒ 保存读的仍是那两个控件 ✓（**一个写者** ✓，而不是"预设另存一份数字"✗）。

结构性后果 ✗：尺寸行的内容变成 `32 + 6 + 2 + 24 = 64` ✓（`.values` + `.form-row` 的 gap + `.size-presets { margin-top: 2px }` + `.chip { height: 24px }`）⇒ **下面所有行再下移 32** ✓。两条按旧间距钉住的测试按**预期正确变化**改 ✓：

- `the_size_row_is_its_cells_and_a_line_of_four_shortcuts` ✓（新，四种 DPI ✓，内容高 64 ✓ 是**字面量**而不是关系式 ✓）；
- `the_quality_row_stays_inside_its_group` ✓ —— 从"相差一个行距"改成"**内容的底 + 12 = 下一行的标签顶**"✓：行距是随行高变的 ✗，而"组内只隔 12"才是设计说的那句 ✓（`:162`）。

**证据** ✓

```
ui   161/0  ✓（142 ⇒ +19：几何 5 + 位图 2 + 语义 1 …）
app  256/0/9 ✓（+1：shortcut_command）
capture 545/0/80 ✓   icon-bake 7/0 ✓
check --workspace --all-targets  Finished ✓  死代码 0 ✓
两个 gate：desktop-tests 0 unmarked ✓  ledger ✓  stale comments 0 ✓  dep-direction clean ✓  baseline OK ✓
```

**三条变异都真红 ✓，还原逐字节一致 ✓**（各先断言 `grep -Fxc` 命中**正好 1 处** ✓）：

| 变异 | 红的信息 |
| --- | --- |
| 分段器对所有格式都显示 ✗ | `the_quality_control_is_hidden_for_png_and_the_hint_takes_its_place` FAILED |
| 预设忽略百分比 ✗ | `a_shortcut_scales_the_capture_to_its_percentage` FAILED（"half is half"） |
| 预设字号 11 → 0 ✗ | `the_size_row_draws_its_four_shortcuts` FAILED |

⚠️ 第二轮演练里**两条第一次没跑成** ✗：`perl -0pi -e "s/…/…/"` 的锚点里含 `/` ⇒ 替换**没发生** ✓ ⇒ 那条"测试仍然绿"**不算证据** ✓；而 `grep -Fc` 对**多行锚点**是按行计数的 ✗ ⇒ 守卫读到 3838 处 ⇒ **拒绝执行** ✓（正是它该做的 ✓）。改成单行锚点 + `s{}{}` 分隔符才拿到红 ✓。

**字形子集** ✗✓：新文字（`PNG 格式是无损压缩，不支持质量调整`）⇒ 门禁**先红** ✓（`no glyph for '无' (U+65E0)` ✓）⇒ 重建 ⇒ **297 → 303 codepoints** ✓ ⇒ 绿 ✓。四个预设处于**数字字面**（`.size-presets .chip { font-family: var(--num) }`，`:259`）⇒ 只用数字与 `%` ✓ ⇒ `number-chars.txt` 已含 ✓ ⇒ 无需重建数字子集 ✓。

**仍缺** ✗：`数值`/`百分比` 模式与**重置钮**（下一批 ✓）；尺寸行的 `warn-hint`（`:621`，超过原图时出现 ✓，A29 的另一半 ✓）；`.adv-btn` 与体积上限（A30 ✓）。

## 尺寸单位与重置、以及 JPG 的透明填充（这一批）

**数值 / 百分比**（`:585`, `:586`）：模式不改变尺寸，只改变那两个数字怎么写下来（`:1072`, `:1073`）—— 所以切换用**旧的单位读、新的单位写**，尺寸本身不动。`scale::SizeMode` + `output_size(mode, …)` + `display_text(mode, …)`，三个纯函数，百分比按 `:1165` 的 `round(original × percent / 100)` 取整，小到 0 的份额留 1 px。

**重置**（`:1213`）：两个数字回到原图尺寸，**模式不变** —— demo 就是这样，所以在百分比下按重置得到的是 `100`。

**证据**：`a_percentage_size_reaches_the_file` 走完整保存并读 PNG 的 IHDR；变异「百分比当像素」使三条测试红，变异「写入时丢掉面板尺寸」使两条文件级测试红，都还原逐字节一致。命中与选中态也有断言（两个模式钮各自只答自己的矩形、重置不与模式钮重叠、选中态跟随模式移动 —— 其中位图那条的比较对象是**写死的 `SIZE_MODE_WIDTHS`**，不是现场测量的同一个数，避免自己比自己）。

**JPG 透明填充**（`:655`–`:660`, `S. fill` `:724`）：`scale::over_fill` 按**直通 alpha**（`artifact.rs` 的 "Straight alpha"）合成 `rgb × a + fill × (1 − a)`，只在 JPG 时施加，因为 PNG 与本项目的 WebP 保留 alpha。默认白（`WHITE_FILL`）。

**新增缺口**：

- ~~**A34 填充色的两个色块没画、没接** ✗~~ ⇒ **已做** ✓（见文末「JPG 的填充行」）。
- ~~**A35 填充的文件级断言缺** ✗~~ ⇒ **已做** ✓：`image` 0.25（含 `jpeg`）**本来就是 app 的 dev-dependency** ✓，注释写明用途就是跨解码器 ✓ ⇒ **零新依赖** ✓。
- **A36 尺寸模式与重置的消息处理没有被测试** ✗ —— 规则（`output_size` / `display_text` / 两个命中函数）都被断言，但「点击真的到达它们」要真窗口。

## JPG 的填充行（这一批）

**为什么只在 JPG** ✗✓ —— 这一条是**从 demo 的 JS 里读实**的 ✓，不是猜的 ✓：`:1102` 是 `$('fill-row').classList.toggle('is-on', S.format === 1)` ✓ ⇒ 只有 `.JPG` ✓。理由也成立 ✓：PNG 有 alpha ✓，本项目的 WebP 无损且保留 alpha ✓ ⇒ 只有 JPEG 必须替透明像素选一个颜色 ✓。`fill_row_shown(format)` 是**唯一判定** ✓，`paint` 与命中**都问它** ✓ ⇒ 看不见的行点不动 ✓。

**几何**（全部带行号 ✓）：`.fill-row { min-height: 22px }`（`:315`）· `.form-row { gap: 6px }`（`:164`）· `.bg-opt { 16×16; border-radius: 4 }`（`:150`, `:58`）· `.bg-switch { gap: 8px }`（`:149`）· `.bg-opt.is-on { box-shadow: 0 0 0 1.5px surface, 0 0 0 3px accent }`（`:153`）· `.limit-label`（`:309`）· `透明区域填充`（`:656`）。

**布局后果** ✗：填充行在**质量行内部** ✓ ⇒ 它一出现 ✓，**预估行下移 28**（22 + 6）✗ ⇒ 而它**由格式决定** ✓ ⇒ 预估行的位置成了**格式的函数** ✗。这里**没有**把标志穿过十六个调用点 ✗，而是加了一个 `estimate_label_rect(.., fill_row)` ✓ —— 因为**只有这一行**在质量行下面 ✓；理由写在函数注释里 ✓。

**画法** ✗：没有白/黑画刷 ✗ ⇒ 三个色块用**位图**画 ✓，圆角**烘焙进 alpha** ✓（`icons::rounded_square_bgra` ✓）；第三块是 demo 的**色轮** ✓（`:317`）⇒ `icons::hue_wheel_bgra` ✓，角度从**正上方顺时针** ✓（与 CSS `conic-gradient` 同向 ✓），自绘 96×96 后由 `draw_bitmap_at` 缩小 ✓（平滑来自降采样 ✓，与图标同法 ✓）。选了自定义色之后，第三块**显示那个颜色** ✓（否则关掉取色器就看不见自己选了什么 ✗）。

**接** ✓：白/黑两个直接置位 ✓；第三个开**系统取色器** ✓（`ChooseColorW` + `CC_RGBINIT|CC_FULLOPEN` ✓ —— demo 那里是 `<input type="color">` ✓），**取消则不变** ✓（与文件夹选择器同一条规则 ✓）。`COLORREF` 是 `0x00BBGGRR` ✓，缓冲是直通的 `[b,g,r]` ✓ ⇒ 转换只在一处 ✓。新增的 feature 只有 `Win32_UI_Controls_Dialogs` ✓（不是新 crate ✓）。

**A35 的文件级证据** ✗✓：16×16 的图，正中 4×4 **完全透明** ✓ ⇒ 存 JPG（填充黑）⇒ 用 **`image` 解码**（与编码器 `zenjpeg` **不同** ✓）⇒ **那一点读回来是黑的** ✓；**对照**：同一张图填充白 ⇒ 那一点**是白的** ✓（否则"编码器把一切都压暗了"也能让第一条通过 ✗）；再存 PNG ⇒ 同一点 **alpha 仍是 0** ✓。

**变异**（各先断言替换命中 1 处 ✓，还原后 SHA256 逐字节一致 ✓）✗：

| 变异 | 红的信息 |
| --- | --- |
| 填充恒为白（忽略面板选择）✗ | `the transparent pixel became the black fill, but the file reads Rgba([252,255,234,255])` ✓ |
| 填充对**所有格式**都施加 ✗ | `PNG keeps the selection's transparency`（left 255 / right 0）✓ |
| 填充行对**所有格式**都显示 ✗ | 纯逻辑那条 + 位图那条（`no swatch is drawn at (1696,874), saw Px{b:255,…}`）✓ |
| 白/黑两块**互换** ✗ | `the first swatch is the white fill: … saw Px{b:0,…}` ✓ |

**两条测试我一开始写错了，都是"自己比自己"的近亲** ✗✓：

1. `the_custom_swatch_is_a_wheel_rather_than_one_colour` 里把 `top` 放进**与 `top` 自己比较**的循环里 ✗ ⇒ 这条断言**永远为假不了** ✓；改成**两两比较** ✓。这正是这一轮反复遇到的那一类 ✗，这次是我自己写出来的 ✓。
2. 色块居中那条要求"上下间隙**相等**"✗ —— 两次独立取整在奇数余数上会差 1 px ✓ ⇒ 改成"**相差 ≤ 1**" ✓（不是弱化 ✓，是整数网格上唯一能成立的说法 ✓）。

**诚实边界** ✗：

- **A37** ✗：色块与取色器的**点击消息处理没有被测试** ✓ —— 规则（`fill_row_shown` / `fill_swatch_at` / `fill_swatch_selected`）与绘制都有断言 ✓，但"点击真的到达 `choose_fill`"要真窗口 ✓。
- 填充行**没有**把限制体积、`不超过…KB`、`自动缩小尺寸`（`:645`–`:653`）一起做 ✓ —— 那属于 A30 ✓。

## B 步（体积上限 · 预估体积 · 自动缩小）—— 做了一半，接线未做

**已完成并验证：**

- `save/estimate.rs` —— `format_bytes`（`227.31 KB` 二进制单位、两位小数）、`shrink_step`（面积平方根 × 0.97 余量，宁可多一步也不越限）、`fit_under`（**每次都用真实编码**，首试原尺寸）、`best_tier`（从最高档往下试，全不满足回报最低档的字节数）、`preset_of`。9 条纯逻辑测试。
- `assemble::weigh` —— 与 `save_panel` **同一条编码链**（同 resample、同 JPG 填充、同 band 编码器、同转码器），只是母版落在内存里。`what_the_panel_weighs_is_what_the_save_writes` 用 PNG/JPG/WebP 三种格式断言"面板说的字节数 = 真写出来的文件长度"——这是这一批唯一的硬证据。
- UI —— `FieldId::Limit`（第五个字段，`FORM_FIELD_TAB_STOPS` 4 → 5）、`limit_switch_rect`/`limit_track_rect`/`limit_knob_rect`、`limit_label_rect`/`limit_input_rect`/`limit_unit_rect`、`limit_note_rect`/`auto_shrink_rect`、`estimate_value_rect`/`encode_state_rect`、`quality_line`（三选一的唯一判定）、`quality_extra_height`；`frame.rs` 画开关（轨道 + 旋钮 + 词）、限制组（`不超过 [64×28] KB`）、说明行、`自动缩小尺寸`、预估值、`编码中 / ✓ 实测`。两条几何测试用**绝对数字**钉住（`:293`/`:297`/`:298`/`:300`/`:307`/`:310`/`:311`）。

**一起修掉的旧缺陷：** `Row::Quality` 的内容高只算了 28，而填充行画在它下面 28 ⇒ 会压到估算行。现在改成"说明行的空间**恒定预留** 24（8 + 16）、填充行接在说明行下面"，估算行由 `quality_extra_height` 位移。**偏离 demo**：demo 的说明行与填充行是按需显示（会跳动），我们恒定预留（不跳）。

**没做（下一批，接线）：** `PaintInput.limit` 目前传 `None` ⇒ 开关/限制组/说明/预估**都还没出现在窗口上**。差的正是"接上"这一半：`WindowState` 的限制状态、`FieldId::Limit` 的原生 `EDIT`（含开关时的显隐）、开关与 `自动缩小尺寸` 的点击、`LimitView` 的宽度测量、以及"预估在打开面板/换格式/换质量时实测一次"（**尺寸编辑不刷新**，见 A38）。

**新缺口：** **A38** 预估值在尺寸编辑时不刷新（需要防抖后台实测；同步实测会卡在每次按键上）；**A39** `limit` 开关的**状态持久化**（规格 `:232` 说"开关状态持久"，`Remembered` 还没有它）；**A40** `自动缩小尺寸` 之后的"宽高滚动并闪烁"（`:235`）没有动效。

## 接线：体积上限与预估值（本批）

**做了什么：**

- `WindowState` 新增 `limit_on` / `limit_words` / `estimate` / `over_limit`；`initial_text(FieldId::Limit)` 给设计稿的 `500`（`:647`）。
- **一处装配**：`with_panel_request(hwnd, f)` 读控件、算输出尺寸、拼 `PanelValues` 与 `Frame`，把三者交给 `f`。**估算与保存都走它**，所以"面板说的"和"落盘的"不可能描述两个请求。
- **开关**：点击 ⇒ `toggle_limit` ⇒ 清掉旧测量、重测、`apply_menu_occlusion` 决定 `FieldId::Limit` 的显隐。**显隐与命中同一个函数**（`apply_menu_occlusion` 是唯一决定控件在不在屏幕上的地方），所以关着的限制框既看不见也点不到。
- **上限真的生效**：`save_with_limit(limit, values, frame)` —— 有上限时用 `estimate::fit_under`（**每次真实编码**）找能装下的尺寸，然后在**那个尺寸**写文件；一个都装不下 ⇒ 不写文件，只记日志（面板保持打开）。
- **上限解析**：`limit_kilobytes(text)` —— 空 / 非数字 / 负数 / 小于设计下限（`:236` 的 `下限 10`）**一律取 10**：读不懂的数字不能等于"不可能"。
- **说明行**：`limit_note(estimate, over, png)` 三种形态（`:234`），数字来自 `format_bytes`，词是 `field.rs` 的常量。
- **`自动缩小尺寸`**：`shrink_to_fit` 用同一个 `fit_under`，把结果**写回两个尺寸格**（一个写者），再重测。
- **测量时机**：开关、换格式、换质量档。**重绘里绝不称重**（`weigh` 是一次真实编码，4K 图秒级）。
- **`WM_DPICHANGED`** 重测 `limit_words`：宽度是像素，换显示器就过期。

**证据：**

- `a_limit_the_picture_exceeds_writes_a_file_under_it` —— 900×700 **噪声**（噪声不压缩，所以无上限的文件必然超限）⇒ 先断言**无上限的文件 > 200 KB**（否则"它本来就小"也能过）⇒ 再有上限保存 ⇒ **落盘文件 ≤ 200 KB 且 > 1 KB**。**这是"上限真的生效"唯一说得过去的证据。**
- 变异：忽略上限 ⇒ 该条红（`the file is 2171317 bytes, over the 204800 byte limit`）✓；说明行恒说"未超" ⇒ `the_note_says_what_the_limit_did` 红（`"当前 640.00 KB，未超出上限"`）✓。都先断言锚点命中 1 处，还原后 SHA256 逐字节一致。
- 字形门禁**先红**（`no glyph for '体' (U+4F53), drawn by "限制体积"`）⇒ 重建 ⇒ **308 → 317 codepoints** ⇒ 绿。**`✓`（U+2713）源字体没有** ⇒ `ENCODE_STATE_DONE` 只能是 `实测`（找不到的字形不能硬画）。

**顺手修掉的旧缺陷：** `frame.rs` 里魔法棒的 `#[test]` 与填充行的 `#[test]` 被写成同一个函数上**两个属性**（`duplicated attribute` 警告）⇒ 那条测试被注册了两遍（175 = 174 + 1 重复）。把魔法棒的文档与属性移回它自己的函数，测试名逐文件比对 120 条全部一致 ⇒ **没丢测试，只是重复没了**。

**新缺口：**

- **A41** 限制打开时的**自动选档**（`:233`「自动选满足上限的最高质量档，都不满足则用最低档」）**未做**：本批只在保存/缩小尺寸时让文件达标，档位仍由用户选。
- **A42** 预估行的 `编码中`（`:243`）**画不出来**：编码是同步的，没有消息泵在编码期间重绘 ⇒ 只能显示完成态 `实测`。要做真的进度就得把编码挪到后台线程。
- **A43** 一个都装不下时**没有句子**：设计稿没有这条文案，本批只记日志、不写文件（`docs/41` A20 是同一类）。
- **A44** `:236` 的 `↑/↓ ±50、Shift ±200` **未做**；`A39`（开关状态持久）仍未做。
---

### 冲突链接（改为覆盖 / 改为重命名）与文件夹对话框的起点

**目标**：名字已被占用时，底栏出现一条链接；点它改变的是"这次保存要不要覆盖"。文件夹对话框从路径框当前的位置开始，而不是从系统默认。

**一处逻辑**（service）：新增 	arget_of(request)（按请求的 conflict 解析）与 plain_target(request)（按覆盖解析，即"如果没有被占用会用哪个名字"）。两者都走 prepare + 
esolve，所以裁剪尾点、补扩展名、_2 编号只有一份。面板**不自己拼路径** ✗。

**可见性**：plain_target(...).exists() 为真才显示链接（:1149）；名字一变自由，策略**自动回到重命名**（:1138），因为覆盖只有在存在文件时才是一个有意义的选择。

**文案是动作**：copy::ConflictLink 的两个词是"改为覆盖"/"改为重命名"，即点击**将要做的事**（:1150），不是当前策略的名字。策略 → 链接的映射是自由函数 conflict_link(policy)，带测试。

**几何**：链接是 .foot 里 .status 与 .copy 之间的 lex: none（:328）；.link-btn 没有内边距（:158），所以宽度就是词宽。measure_conflict_link 测**两个词**（[i32; 2]），用 ConflictLink::index() 取当前那个 ⇒ 画的和点的同一组数字；ooter_status_rect 多收一个 link_width: Option<i32>，句子让出"链接 + 一个间距"（.status { flex: 1 } :275）。

**时机**：开面板时问一次；名字/目录 EN_CHANGE 后问一次；换格式后问一次（扩展名变了 ⇒ 问的是另一个文件）。**重绘里不问**：答案来自文件系统，重绘不是它改变的证据。

**文件夹对话框的起点**（:1133）：BROWSEINFOW 没有"从这里开始"的成员，起点在 BFFM_INITIALIZED 之后经 BFFM_SETSELECTIONW 送进去，字符串由 lParam 带着；rowse_initial_path(current) 是纯函数（空/全空白 ⇒ None，否则 NUL 结尾的 UTF-16），带测试。

**证据（文件级）**：_taken_name_is_replaced_or_kept_beside_by_the_panels_choice —— 先写一个内容为 last night 的 shot.png；重命名保存 ⇒ 路径**不是** shot.png、原文件**字节未变**、新文件在盘上；覆盖保存 ⇒ 路径**就是** shot.png、字节**被替换**。	he_plain_target_is_the_name_the_save_starts_from 钉住 plain_target 原地不动而 	arget_of 前进到 shot_2.png。

**变异（三条，全红，还原后 SHA256 逐字节一致，都见 Compiling snapclip-app）**：

- 面板的选择被忽略（panel_values 写死 AutoRename）⇒ 两条红：	he_panels_conflict_choice_reaches_the_values（left: AutoRename / right: Overwrite）与 _taken_name_is_replaced_or_kept_beside_by_the_panels_choice（ssertion failed: overwriting keeps the name）。
- 链接显示**当前策略**而不是动作 ⇒ 	he_conflict_link_offers_the_policy_that_is_not_in_force 红（left: Rename / right: Overwrite）。
- plain_target 跟着策略走 ⇒ 	he_plain_target_is_the_name_the_save_starts_from 红（left: …\shot_2.png / right: …\shot.png）。

**新缺口：**

- **A45** 设计稿在名字被占用时还有两句状态句（:1142 ⚠ 同名文件已存在，将保存为 <name>、:1143 ⚠ 将覆盖已有文件 <name>）**未做**：它们与"保存成功/失败"共用底栏那一条状态行，需要先定优先级（本轮只做了链接本身）。
- **A46** 更改 用的仍是 SHBrowseForFolderW（旧式树对话框）：不是一个按钮值得手写 COM 接口的程度，代价是观感，不是行为。

## 收尾三件：C2 超分辨率提示 · C3 变量值列 · C5 高级选项只记录

### C2 尺寸行的超分辨率提示 ✓

- 位置：`Row::Size` 内容区的**第三行**，四个预设 chip 之下（`.warn-hint`，`:621`–`:623`），逐字取自
  `:623` 的 `超过原始分辨率，放大将损失清晰度`，图标是 `:622` 的 info-circle，颜色 `--warning`（`:254`）。
- 判定：`field::warn_hint_shown(output, source)` = **任一维比原图大**（`:1069` 的 `isWarnW || isWarnH`）。
  `paint` 只读 `PaintInput.warn_hint`，窗口用 `scale::output_size` 的结果算它 —— 与保存读的是同一个尺寸。
- **行高 10 dip，不是图标的 12**：10 是那句话自己的字号（`:254`）。用 12 会让表单比正文带高 2 dip
  （实测 510 对 508）。图标因此上下各溢出 1 dip，而那条行不裁剪任何东西。
- **恒定预留**（与质量行的说明行、填充行同一条偏离）：demo 里它在流内、显示时把下面的行推下去；
  我们的窗口高度是行高之和算出来的，让一行来了又走意味着窗口改高度或内容溢出。
- 顺带修掉一个**早就存在**的缺陷 ✗✓：`size_cell_rect` 的 `bottom` 取的是 `row.bottom`，
  于是尺寸格的包装框跟着整行内容长 —— 预设 chip 一直在**尺寸格的 `--fill` 底色上**，
  只是没人往那行下面再放东西。现在包装框按 `metrics.field_height` 收成格子本身。
- 测试：几何用**绝对数字**（6 / 28 / 12 / 4，四档 DPI）；位图在 2× 下断言图标与句子都是 `--warning`；
  另有一条"没有可警告的东西时那条行是干净的"，否则"永远画着"也能让颜色断言通过。
- 字形子集 **317 → 323**（`原`、`始`、`辨`、`率`、`损`、`晰`）。

### C3 变量菜单的值列 ✓

- `PaintInput.variable_values: Option<[&str; 5]>`，由窗口在绘制时装配，取自 `local_date_and_time`、
  `Remembered.sequence.next()`、`scale::output_size`（**输出**尺寸）与 `APP_NAME` —— `[软件名]` 与
  `PanelValues.app` 现在是同一个常量，不再各写一遍。
- 画法：右对齐（宽度靠 `measure_text_width` 反推左缘，因为共享的 11 dip 数字格式是左对齐的，
  改它的对齐会把预设 chip 的词一起挪），颜色 `--muted`（`:221`）。
- 测试：`--muted` 是**上限**（值区里不许有比它更亮的像素，且必须真的有墨）。第一版写成"找最接近
  `--muted` 的像素"，把颜色换成 `--text` 仍然绿 ✗ —— 抗锯齿的边缘总有像素落在容差里。

### C5 高级选项对话框 ⇒ **只记录，不画** ✗

`docs/design/save-panel-demo-v3.html:1492`–`:1505`。**不画**的理由是里面 ≥3 项要做新机制，
画一个点不动的对话框比不画更糟（它承诺了一件没做的事）：

| 项 | 设计 | 现在做不了的原因 |
| --- | --- | --- |
| PNG `压缩等级`（最快/平衡/最小） | `:1494` | banded PNG 写器没有暴露 DEFLATE 的参数 |
| PNG `色板量化`（256 色，有损） | `:1495` | 需要调色板量化 + 索引色编码，整条新路径 |
| JPG `色度采样`（4:4:4 / 4:2:0） | `:1498` | `zenjpeg` 是否暴露未知，要查 |
| JPG `渐进式` | `:1499` | 同上 |
| WebP `无损压缩` 开关 | `:1501` | 本项目 WebP 恒为无损，开关要么恒开要么需要非无损编码 |
| `移除元数据` | 三格式都有 | 我们本来就不写元数据 ⇒ 现在是 no-op |

落点：`service::SaveRequest` 增一个 `Advanced` 结构（三格式各自的可选项用枚举），
`bands::encode_png` 与 `transcode::to_bytes` 各取所需；对话框本身用已有的 `.dialog` 几何
（`docs/41` A27 的 `sliders` 图标还没下载）。

### 新缺口

- **A47** 面板高度仍是 `header + body_top + preview + footer = 564` **固定值**，而 demo 的面板是
  **内容驱动**的。正文带现在是 508 dip（表单最后一行正好落在带上），再加一行就会溢出 ⇒
  需要让高度取 `max(预览带, 表单底)`，并把 footer 的位置一起搬过去。
- **A48** 提示的图标（12 dip）比它所在的行（10 dip）高 1 dip 上下各一 —— 不裁剪，但两行同时出现时
  图标与上一行 chip 的间距会比设计稿小 1 dip。

## 诊断：格式菜单关掉后"只是变透明"（本次修复）

用户实测：格式菜单打开后，再点触发按钮或点菜单外，菜单没有消失，只是变透明。

**根因（两层，都在结构上，不是某一行写错）**

1. 面板用 DWM 材质（`DWMWA_SYSTEMBACKDROP_TYPE` = `DWMSBT_TRANSIENTWINDOW`）且 `WM_ERASEBKGND` 返回 1、`fill_surface = false` ⇒ **本体刻意不填**，未绘制处透出材质。而绘制入口**从不 Clear**（`frame.rs`、`d2d.rs` 都没有 `Clear`）⇒ 某帧没画到的像素 = **上一帧留下的**。
2. 菜单关闭只清标志 + `InvalidateRect`，没有任何东西把菜单矩形重画成不透明 ⇒ 旧菜单的半透明像素留在目标里，而目标**带 alpha** ⇒ DWM 把它与桌面合成 ⇒「菜单还在，只是透明」。

**修法**

- `PaintTarget::clear()`（四个目标共用基类 `ID2D1RenderTarget::Clear(None)`）+ `PanelPainter::clear()`；`PaintInput::clear_first` 决定是否执行。**窗口传 `true`**（目标跨帧保留像素）；**离屏画布传 `false`**（`idle()` 的默认）——画布在同一个测试里创建、绘制、读取，而它预先填的表面色正是**装置对 DWM 材质的模拟**，先清掉会毁掉这个模拟（这一条被 8 条表面色断言当场抓到）。
- `close_menus(hwnd, why)` 成为**关闭菜单的唯一入口**：清两个标志 + `apply_menu_occlusion` + `InvalidateRect`。此前有两条路径只清标志不恢复被隐藏的输入框（选格式、点菜单外）⇒ 那些框会一直隐身。
- 新增 `WM_ACTIVATE`：菜单是窗内自绘的弹层，点面板**外面**拿不到鼠标消息，失活才是关闭时机。
- `panel::hidden_fields(Option<Rect>, &[Rect])`：把"无菜单 ⇒ 不隐藏任何框"变成**规则的属性**，可断言（`a_closed_menu_hides_nothing`）。
- 仪表：`apply_menu_occlusion` 每次转换打一行 `[save-panel] occlusion format_open=… var_open=… menu=… hidden=… shown=…`，`close_menus` 打关闭原因。

**证据**：`erasing_the_target_leaves_the_unpainted_body_transparent` 两侧都断言（不清 ⇒ a=255，清 ⇒ a=0）；变异 M1（`None` 也返回全部索引）红在"with no menu open every box is shown"；变异 M2（去掉清底）红在"with an erase nothing the frame did not draw is left"；两次还原 SHA256 逐字节一致。

**未覆盖**：真实窗口里"点击 → 不再残留"只能人眼确认；`close_menus` 与 `WM_ACTIVATE` 的消息处理没有窗口级测试（需要真窗口）。
## 名字框的 `EDIT` 压在格式按钮底下（P9-70）

真机现象：格式下拉**永远打不开**；用户日志里 `format_open=true` 一次都没出现，只有魔法棒的 `var_open=true`。

根因是**同一个坑第二次**：`FieldId::Name` 的控制矩形只让出了魔法棒的 34 dip（`:212`），而**没有让出格式按钮的 80 + gap 8**（`:561`）。原生子窗口画在父窗口之上，所以那个 `EDIT` 不但盖住按钮，还**吃掉落在按钮上的点击** —— `WM_LBUTTONDOWN` 收不到，菜单自然打不开。

修法：一条推导链，不新增第二处算术。

```
field_rect(Row::Name)            // .file-row 的整行
  → name_field_rect              // 减 80 + gap 8 = .name-wrap
    → name_text_rect             // 再减 34 = EDIT
    → name_wand_rect             // .name-wrap 右端内缩 4，宽 24
```

`FieldId::frame_rect(Name)` 同时改为 `name_field_rect`：画的框是 `.name-wrap`，`EDIT` 比它再少一条魔法棒带。

**顺带抓到的第二条缺陷**：`the_variable_button_is_drawn_inside_the_name_field` 断言"加号中间必须有墨"，它**一直在给错误的东西作证** —— 旧的图标位置 1752..1800 落在格式按钮 1648..1808 里，它读到的墨是**格式按钮自己的字**。改成"两道笔画 + 中间必须有空隙"后，它才真的在说 braces。

**第三条**：`a_token_menu_without_values_leaves_its_right_side_clean` 要求值带**恰好等于**表面色，而 `--surface` 是 `rgba(23,23,28,.92)`（`:25`）—— 92% 是设计稿的透明度，底下那行 chip 的词本来就会透出 8%。改成"不比表面亮 24 以上"，值词是 `--muted`，远超这个上限。

新断言 `no_text_box_reaches_under_a_control_the_frame_draws`：五个字段 × 五个自绘控件（格式钮 / 魔法棒 / 更改 / 文件夹 / 尺寸链） × 四种 DPI，用 `rects_intersect` —— 与遮挡用的是同一个谓词，所以"控件不许压在框下"和"菜单压住的框要隐藏"不可能对"相切"有两种看法。变异（把控制矩形改回从行带派生）后三条测试红，报的正是诊断：

```
Name's text box { left: 584, right: 870 } reaches under the format button { left: 824, ... } at 1x
```

## 头部：移除系统自带的关闭钮，加最小化钮

**现象**（用户实测）：面板右上角有一个**系统画的小 ×**，点了没反应 ✗。

**根因**：创建窗口时样式是 `WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU` ✗。`WS_CAPTION` **必须保留** —— 它是 DWM 画圆角、阴影和材质的依据（`WM_NCCALCSIZE if wparam != 0 => 0` 那条注释写的就是这个）；而 `WS_SYSMENU` 让 DWM 在客户区上再画一个标题栏关闭框 ✗，同一个 `WM_NCCALCSIZE` 把非客户区压成 0 ⇒ **画得出、点不到** ✗。看得见却按不动的按钮比没有更糟，所以去掉这一位。

**形状**：新增 `PANEL_STYLE` 常量（`WS_OVERLAPPED | WS_CAPTION`），创建处不再写字面量；`WS_MINIMIZEBOX` / `WS_MAXIMIZEBOX` / `WS_THICKFRAME` 在测试里断言为**零**，这样将来谁要加，必须说出来。

**代价（如实记）**：没有系统菜单 ⇒ `Alt+F4` 不再关窗；面板自己有关闭钮、`Esc` 和「取消」，所以不走那条路。

**最小化钮**：`rects.minimize` 与 `rects.close` **同尺寸**（26 dip，`:96`），间距用 `HEADER_BUTTON_GAP_DIP`，它**读的是** `field.rs` 已有的 `SIZE_MODE_GAP_DIP`（`.row-header-actions { gap: 8px }`，`:224`）—— 不是又写一个 8。设计稿的头部只有一个按钮（`:474`），所以没有可抄的头部间距，这是设计稿自己"一行两个小按钮"的那条 token。

图标用已烘好的 `minus.png`（643 B，早已入库），新增 `Glyph::Minus`（`ICON_COUNT` 13 → 14）；绘制与关闭钮同一套 `muted_rgba`；命中测试在按钮矩形上，不在 14 dip 的图标上（与关闭钮同一分工）。行为是 `ShowWindow(SW_MINIMIZE)`。

**未完成的验证**：`cargo test -p snapclip-ui` 200/0（含三条新断言：几何四档 DPI、命中半开区间、位图绝对色）；`icon-bake` 7/0、`capture` 545/0/80 都在加图标之后绿。但 **`snapclip-app` 此刻编译不过** —— 并行的"自绘数字框"批次正在中途（`NumericFields` / `numbers` / `focused_number` 未就绪），所以本批在 `window.rs` 的四处改动（`PANEL_STYLE`、创建处、最小化点击、样式断言）**还没有经过编译验证**，必须在那一批落地后重跑。

另一个并行痕迹：`frame.rs` 的值列测试里有一行 `let muted = fill_rgb(152, 157, 161);` 重复了两次（2853 与 2858），第一行未使用 ⇒ `lib test` 一条 `unused variable` 警告。那是并行批次的半成品，我没有动。

### 自绘数字框的断言（P9-72）

上一提交把三个原生 `EDIT` 换成了自绘（`text_field.rs`），这一批补的是**能咬的断言**：

- `field::a_selection_is_the_accent_mixed_into_the_field_fill` —— 选中色 = `--accent` 按 35% 混进字段填充，**写死字面量**（`[37,67,106,255]` 与两端各一条）。不重算，否则会和错的百分比一起对。
- `frame::a_drawn_number_carries_its_selection_and_its_caret` —— **两侧**：有焦点 ⇒ 该颜色（由上面那条钉住的算式给出）在格子里出现；**失焦 ⇒ 一个都没有**。另外把选区收成光标后，`--accent-text` 的竖线必须出现。
- `frame::a_drawn_number_in_warning_takes_the_warning_colour` —— 警告态的数字用 `--warning` 画，走 `field_colours` 的 `text` 而不是画家的第二套判断（`:241`）。
- `window::the_three_numbers_have_no_controls` —— 三个数字字段的句柄都是空：没有子窗口，也就没有东西能压在自绘控件上吃掉点击（名字框那次就栽在这）。
- `window::the_size_cells_read_and_write_their_models` · `focusing_a_number_selects_it_and_losing_the_keyboard_drops_it` · `only_one_drawn_number_has_the_focus` —— 模型是这三个数的**唯一**所在；聚焦即全选、失焦即收选区、同一时刻只有一个有焦点。

**两条变异都真红**（各自先断言替换命中 1 处、还原后 `sha256` 逐字节相同、都见 `Compiling`）：

```
M1 输入时不吃选区（start,end 都取光标）
   ⇒ gaining_the_focus_selects_everything_so_typing_replaces FAILED
      left: "13419"  right: "9"        （"the first keystroke replaces the number"）

M2 选区带用字段填充色画
   ⇒ a_drawn_number_carries_its_selection_and_its_caret FAILED
      "a selected number paints its highlight in the accent mixed into the field's fill"
```

**一处踩坑记下来**：`PaintInput::idle(dpi)` 的参数是 **DPI**，写 `idle(1.0)` 会让整帧按 0.5 缩放画，于是按 scale 1.0 算出来的采样矩形**整块是表面色**——第一次的位图断言就是这么“什么也没测到”的。

## 路径行改成只读（`P9-75`）

**为什么改**：`.text-input` 是给“能输入的东西”的形状。路径不能输入——整行的唯一动作是打开文件夹选择器，和它右边的 `更改` 一样——所以那个原生 `EDIT` 带来三样它用不上的东西：插入符、**系统选中色**、以及和自己圆角框对不上的底色。

**形状**：
- `FieldId::Path` 不再创建控件（`controls[Path]` 为空），值改由 `WindowState.directory` 承载，**一个写者**（`set_directory`，chips 与选择器都走它），四个读者（保存、`choose_folder` 的起点、文件夹钮、冲突重算）都读它；
- 绘制在 `frame.rs`：`--fs-input` 12 dip、`--text`、`.text-input` 的 `padding: 0 10px`（`path_text_rect`）；
- **过长时中间省略**（新 `panel/elide.rs`）：保住盘符与**尽可能多的尾部目录**，因为路径的两头才是信息（去哪个卷、进哪个文件夹）。`D:\100_Projects\110_Daily\SnapClip\assets\icon` 在 112 px 下得到 `D:\…\assets\icon`；更窄时 `D:\…\icon`；连最后一级都装不下才退到 `…SnapC`——**保留尾部片段的开头**，因为文件夹靠开头辨认，扩展名才靠结尾；
- 整行可点：`rect_contains(path_field_rect, point)` 与 `更改` 走同一个 `choose_folder`；
- 光标自然是箭头：去掉子窗口后客户区用窗口类的 `IDC_ARROW`，没有地方再设工字光标。

**顺带纠正的一处“两种数法”**：`FORM_FIELD_TAB_STOPS` 写的是 **5**，而三个数字框早在 `P9-71` 就自绘了——那个数从那时起就在说谎。现在它是 **1**，测试改成 `FieldId::ALL.iter().filter(is_focusable).count()`：**哪个字段能拿键盘**与**有几个 tab 停靠**从此是同一个答案；`is_focusable()` 同时是“要不要建控件”的判据（`!id.is_focusable() → continue`），所以一个字段想变成控件，必须同时承认自己可聚焦。

**新断言抓到的真缺陷**：`the_directory_row_draws_its_value_and_never_a_focus_ring` 第一版**红**——路径文字一个像素都没有。原因不在新代码：`FrameBrushes::field_brush` 只在**三个边框色**里查表，而规则要的是字段的 `text`（纯白），查不到返回 `None`，调用方把 `None` 当成“这条规则什么也不想画”，于是**静默不画**。修法是把规则能产生的**每个**颜色都建一支笔（`field_brushes()` 遍历四个状态 × border/text/inset 去重），而不是给这次的颜色打补丁——否则下一个新颜色还会静默消失。

**变异（两条都真红；先断言锚点命中 1 处；还原后 SHA256 逐字节一致）**：
```
M1 取后缀的顺序反过来（丢盘符留尾巴）
   ⇒ the_drive_and_the_last_segment_stay FAILED
      left: "…icon"   right: "D:\\…\\assets\\icon"

M2 让 Path 也算可聚焦
   ⇒ the_form_declares_one_tab_stop_per_focusable_field FAILED
      assertion `left == right` failed: every focusable field is a tab stop
      left: 2   right: 1
```
第一次的变异写法**没编译过** ⇒ 按清单第 1 条**不算证据**，换成“改循环方向”才拿到红。

**没覆盖的**：
- “点击整行打开选择器”与“真的没有控件”只在有窗口时成立，测试装置没有窗口 ⇒ 靠编译（`!is_focusable()` 是唯一入口）与人眼；
- 路径是**运行时数据**，字形子集只覆盖**字面量**：目录名里出现子集外的字会回退系统字体（与 A21 同一类）；
- `hover` 显示完整路径与只读行的悬停态：未做，记 **A49 / A13**。

---

## 头部：最小化要真的能最小化

用户报：**点最小化，窗口跑到屏幕左下角**，而不是最小化到任务栏。

**两个位叠在一起**（`apps/snapclip/src/save/window.rs`）：

| 位 | 改前 | 后果 |
| --- | --- | --- |
| `WS_EX_TOOLWINDOW` | **在** `PANEL_EX_STYLE` 里 | 工具窗口**不进任务栏**、不进 Alt+Tab ⇒ 最小化**没有目的地** ⇒ 折到桌面角落 |
| `WS_MINIMIZEBOX` | **不在** `PANEL_STYLE` 里 | 窗口**不可最小化** ⇒ 任务栏也没有可恢复的对象 |

⇒ 两件都要改：去掉 `WS_EX_TOOLWINDOW`（获得任务栏按钮 + Alt+Tab），加上 `WS_MINIMIZEBOX`（窗口可最小化）。

**`WS_MINIMIZEBOX` 不会画出东西**：它描述的是**非客户区**的标题栏按钮，而 `WM_NCCALCSIZE` 把非客户区**全部**认作客户区 ⇒ 没有地方画。上一件那个小 × 是 **`WS_SYSMENU`** 带来的，与这一位无关，所以 `WS_SYSMENU` 仍然不在。

**`WS_CAPTION` 不动**：DWM 的圆角、阴影、材质靠它。

**没有波及别人**：`PANEL_STYLE` / `PANEL_EX_STYLE` 只在 `window.rs` 的创建处（`:671` / `:674`）使用，浮层用自己那套样式。owner 参数是 `null_mut()` ⇒ **无主窗口** ⇒ 去掉工具窗口位**确实**会得到任务栏按钮（有主窗口则不会）。

**代价**：面板会**出现在任务栏与 Alt+Tab** 里。对"能最小化再叫回来"的保存对话框这是用户要的；对"永远浮在最上面的一次性面板"就不再成立。

**变异（两条都真红；先断言锚点命中 1 处；还原后与 HEAD 的差异只有本节的改动）**：
```
M1 去掉 WS_MINIMIZEBOX
   ⇒ the_panel_style_leaves_the_caption_buttons_to_the_header FAILED
      assertion `left != right` failed: a window without this bit cannot be minimised
      left: 0   right: 0

M2 把 WS_EX_TOOLWINDOW 加回去
   ⇒ the_panel_asks_to_be_above_the_windows_it_replaces FAILED
      assertion `left == right` failed: the panel needs a taskbar entry
      left: 128   right: 0
```
两条断言**都是反过来改的**：原来断言"有工具窗口位"与"没有最小化位"，现在断言相反的结论。这是**预期正确变化**，不是为了让实现通过——旧结论来自"头部自己画按钮，所以不要系统位"，而那条推理漏了"最小化需要任务栏"。

**没覆盖的**：
- "`WS_MINIMIZEBOX` 不画东西"是**读文档 + 推理**（非客户区被 `WM_NCCALCSIZE` 认满），没有窗口可截图 ⇒ 只真人可见；
- 任务栏按钮与 Alt+Tab 里的实际表现：要真机看。

## 一批：字段背景的笔刷（每状态一支）

用户报告：有些字段内部有一块比周围亮的底（"贴片"）。

根因：`WM_CTLCOLOREDIT` 里 `SetBkColor` 用的是 `field_colours(state)` 的 `fill`（每个状态不同），
而返回的笔刷 `s.fill_brush` 是**按 Idle 建的一支**。于是"告诉系统的背景色"与"实际刷上去的颜色"
不是同一个来源 —— 聚焦/警告态的字段被告诉一个色、刷上另一个色。

修法：`fill_brushes: [HBRUSH; FieldState::ALL.len()]`，用 `std::array::from_fn` 遍历
`FieldState::ALL` 建笔；`WM_CTLCOLOREDIT` 用 `fill_brushes[state.index()]`。
笔与 `SetBkColor` 现在来自**同一次** `field_colours` 调用、同一个索引，不可能分家。

`FieldState` 因此新增 `ALL` 与 `index()`（照 `ConflictLink::index` 的先例）。
断言 `every_state_is_listed_once_and_indexed_by_its_position` 钉住"编号是 ALL 里的位置、且两两不重复"；
变异（把 `Warning => 3` 改成 `=> 0`）真红：`left: 0 right: 3`。

## 未做（下一批）

- ① "无损压缩"后面的浅色块**未定位**：`QualityLine::Hint` 分支只 `draw_text`，
  没有 fill；`Limit` 字段也**没有原生控件**（只有 `Name` 有）。⇒ 需要真机像素或截图才能定位，未猜。
- ② `Name` 的右键菜单（复用 `dropdown.rs`）未做；字号字重未核对完。
---

# 名称框的右键菜单与字号（本批）

## 做了什么

`Name` 是面板上**最后一个**原生控件，于是它的两处"系统味"都落在它身上：右键弹出的是**亮色的系统菜单**，
字号字重也没跟设计稿对齐。

1. **右键菜单自绘**（`WM_CONTEXTMENU`）：只在**鼠标落在 `Name` 上**时接管（`wParam` 是子窗口句柄，
   经 `field_of` 判定），其它字段直接 `DefWindowProcW`。菜单**锚在点击处**，下方放不下就翻到上方
   （与格式菜单 `:902` 同一条规则），宽高沿用格式菜单自己的 token（`:180`、`:182`）与 8 px 文字内缩
   （`dropdown::d2d::inset_text`）——**没有新数字**。
2. **命令走消息而不是改文本**：`WM_CUT` / `WM_COPY` / `WM_PASTE` / `EM_SETSEL(0,-1)`。
   控件自己维护撤销栈与剪贴板，从外面重写文本会把两者都弄坏。
3. **遮挡接的是同一套**：`apply_menu_occlusion` 现在**先问右键菜单**（它是唯一锚在点击处而不是控件旁的
   菜单，且同一时刻只有一个菜单打开）；`close_menus` 一并清 `context`，`Esc` 与"点菜单外"都走它。
   这是同一个 bug 的**第三次**预防（前两次：格式菜单、魔法棒菜单）。
4. **字号字重**：`FW_SEMIBOLD`（600）→ `FW_NORMAL`。设计稿 `.text-input`（`:167`）**不设字重**，
   只给 `font-size: var(--fs-input)`（`:39`，12 px）；600 从来没有依据，且让值比标签还粗。

## 一处**仍未对齐**的偏离（如实记）

`.file-row .text-input`（`:172`）把**文件名框**的字体设成 `var(--num)`（等宽数字族）。
我们没有跟：面板的 `--num` 是**只含 23 个字符的子集**（`number-chars.txt`），文件名要输中文，
用它会大面积回退系统字体。⇒ 文件名仍用界面字（`FAMILY`，含中文子集），
**代价**是数字看起来不是等宽的；这是"能输中文"换来的，不是遗漏。

## 证据

```
ui 216/0 ✓（+5：四条几何/规则 + 一条"菜单盖住名字框且只盖它" + 一条字号）
app 285/0/9 ✓   capture 547/0/80 ✓   icon-bake 7/0 ✓
check --workspace --all-targets ✓ 无 never used ✓
cargo test --workspace --lib -- --test-threads=1 全绿 ✓（hook 那条）
```

两条变异**都真红**（各自先断言锚点命中 1 处、还原后与基线逐字节一致）：

```
M1 所有命令恒可用      ⇒ only_the_commands_that_can_act_are_enabled FAILED（left: [true,true,true,true]）
M2 菜单永不向上翻      ⇒ the_context_menu_opens_upward_... FAILED（left: 200）
```

⚠️ 第一次跑变异时**两条都"绿"**：`-- --exact` 配**裸测试名**⇒ **一个测试都没跑** ✗。
去掉 `--exact`、并确认 panic 里有测试全名之后才拿到上面两条红。这正是"变异没发生 ≠ 断言弱"那一课。

## 未覆盖（要人眼 / 后续）

- `WM_CONTEXTMENU`、`WM_PASTE`、`EM_GETSEL` 的**真实消息链**没有窗口级测试（需真窗口）。
- **粘贴恒可用**：不查询剪贴板（那要新增 `Win32_System_DataExchange` 特性，为一个菜单项不值）；
  剪贴板为空时粘贴是一次无操作。剪切/复制按 `EM_GETSEL` 判定，无选中时**变暗且不响应**。
- 菜单**无悬停高亮**、**无键盘导航**（`Dropdown` 的 `key()` 没接）⇒ 与格式菜单同一批缺口（A13）。

---

## 右键菜单被系统菜单顶掉：子类化只放行一条消息

**现象**：在文件名框右键，弹的是系统 `EDIT` 的菜单（撤销/剪切/复制/粘贴/从右到左阅读顺序/汉字重选），
不是面板画的那个。

**根因**：父窗口确实收到了 `WM_CONTEXTMENU` 的分支，而且分支本身是对的（`field_of(wparam)` 判定
`FieldId::Name`、`ScreenToClient`、测量宽度、遮挡、重绘都在）。问题在**消息根本到不了父窗口**：
右键落在 `EDIT` 上时，`WM_CONTEXTMENU` 发给**子窗口**，`EDIT` 自己处理并弹系统菜单。
父窗口那条分支因此**从未被执行过一次**——和 `Esc`/`Enter` 那次一样，是"处理函数在、链路不通"。

**修法**：只给文件名框挂 `SetWindowSubclass`，进程里第一个子类化：

```
WM_CONTEXTMENU 且 lparam != -1  ⇒ 转发给父窗口（wparam 原样，即子窗口句柄；lparam 原样，是屏幕点）
其它一切消息                     ⇒ DefSubclassProc（打字/光标/选区/剪贴板/撤销栈都在它自己的过程里）
WM_NCDESTROY                    ⇒ RemoveWindowSubclass（句柄+id 用同一个常量，不可能分家）
```

`lparam == -1` 是键盘的上下文菜单键：面板没有"光标在哪"的几何来放置菜单，那条**留给控件自己**，
记为缺口（`Shift+F10` 仍是系统菜单）。

放行的判定抽成 `forwards_context_menu(message, lparam)`：一条纯函数，于是"拿走什么"与"放行什么"
可以不用窗口就断言。测试 `the_box_gives_up_only_the_context_menu_it_can_place` 断言**恰好一条**：
`WM_CHAR`/`WM_KEYDOWN`/`WM_SETFOCUS`/`WM_LBUTTONDOWN`/`WM_NCDESTROY` 都必须到达控件自己的过程——
一个多吞消息的子类不会让别的测试红，它会**悄悄让这个框不再是输入框**。

**变异**（都先断言锚点命中 1 处、`Compiling snapclip-app` 可见、还原后逐字节一致）：

```
M1 `lparam != -1`（拿走所有消息）        ⇒ message 0x0102 has to reach the box's own procedure
M2 `message == WM_CONTEXTMENU`（连键盘键也拿走）⇒ the keyboard's context key has no caret position …
```

M1 第一版写成 `true`，结果红在 M2 那条断言上 —— **没隔离**"只拿一条"这件事，换成"只看 lparam"才拿到
指向 `WM_CHAR` 的红。

## 顺带修掉一个偶发：测试在数全进程共享的临时目录

`no_master_is_left_in_the_temporary_folder` 数 `std::env::temp_dir()` 里 `snapclip-master-*` 的**数量差**。
并行跑时另一次转码也在同一目录里放母版 ⇒ 前后的差被别人的文件扰动 ⇒ **串行绿、并行红**。
证据：单独跑 3 次全绿，整套并行红，且 TEMP 里真的留下 1 个母版。这与"串行红/并行绿"是同一族的反面。

修法是给 `to_bytes` 一个**可注入的目录**：`to_bytes_in(..., folder)`（公共 `to_bytes` 用 `temp_dir()`），
测试用自己建的私有目录 ⇒ 数到的只可能是这次调用放的。测试改名
`no_master_is_left_in_the_folder_it_was_parked_in`，因为它的新断言说的是另一件事。

## 未覆盖

- 子类化真的收到消息、转发后菜单真的弹出、命令真的剪贴/粘贴 —— 都要**真窗口 + 人眼**。
- 键盘上下文菜单键（`Shift+F10`）仍走系统菜单。
- `examples/component_gallery.rs` 的 4 条 `unused BOOL` 是既有 warning（文件未改）。
- 顺带清掉了 `window.rs` 里 4 处 `unnecessary unsafe`（这个版本的 `windows-sys` 里
  `SetWindowSubclass`/`ScreenToClient`/`GetClientRect`/`InvalidateRect` 都是安全函数）。

## 右键菜单：宽度、快捷键（用户实测两处）

### ① 窄到竖排 —— 两处内缩叠加，量宽时少算了一处

```
量宽  measure_context_menu:  widest + 2×CONTEXT_TEXT_PAD(8)                ✗ 只算了一处
绘制  paint_context:         项矩形 = 菜单 − 2×FMT_MENU_PAD(5)             ✗ 第二处
                            文字   = 项矩形 − 2×CONTEXT_TEXT_PAD(8)
⇒ 文字可用宽 = widest − 10 ✗ ⇒ 差一丁点 ⇒ DirectWrite 默认换行 ⇒ 两字一行 ⇒ 竖排 ✓
```

而且**全仓没有一处 `SetWordWrapping`**：默认是"允许换行"，所以"差一点点"不会溢出成
可见的错误，而是**折成两行**——这正是一个断言不会描述成"错"的错。

**修法（两处，都是根因）**
- `field::context_menu_width(word, shortcut, scale)`：**唯一**的宽度公式，把两处内缩都算进来
  （`2×px(FMT_MENU_PAD_DIP + CONTEXT_TEXT_PAD_DIP)`）。`measure_context_menu` 改成**按行**量
  （词 + 间距 + 键，取最宽那行），不再用"最宽词拼最宽键"（那描述的是不存在的行）。
- `frame::no_wrap`：菜单行格式（`var_label_format`、`format_label_format`）设
  `DWRITE_WORD_WRAPPING_NO_WRAP`。规则挂在格式上，因为**格式才是决定者**，一处答案服务所有行。

### ② 快捷键 —— 菜单行是"词在左、键在右"

`CONTEXT_SHORTCUTS = ["Ctrl+X", "Ctrl+C", "Ctrl+V", "Ctrl+A"]`，与 `CONTEXT_ITEMS` 同序；
布局照 demo 的菜单行 token：`.var-item { justify-content: space-between; gap: 12px }`（`:219`）⇒
`CONTEXT_ITEM_GAP_DIP = 12`，键**右对齐**、色 `--muted`（`.var-item .val`，`:221`），字号与菜单
文字同一族（界面字，不是数字子集——那是只含 23 字符的等宽子集）。词矩形在键左侧再减一个间距，
所以即使调用方给的宽度比量出来的窄，词与键也不会挤在一起。

`form_drawn_strings()` 收进 `CONTEXT_SHORTCUTS`（先改生产者，门禁才看得见）；**门禁没有红**：
`C t r l + X V A` 都已在既有子集里（327 codepoints），无需重建。

### 证据

- `a_menu_row_is_wide_enough_for_its_word_and_its_key`：**绝对数字**
  `context_menu_width(40, 30, 1.0) == 40 + 12 + 30 + 2*(5+8)`，以及 2× 档
  `== 40 + 24 + 30 + 2*26`（间距与两处内缩随 scale 走；**文字宽已是像素**，不再乘）。
  用绝对数是因为"用同一组常量算出的宽度再去和这组常量比"抓不住一个错常量，而**漏掉一处内缩**
  正是这个 bug 的形状。
- `the_menu_rows_carry_the_keys_the_box_answers_to`：四条键逐字 + 与命令同长 + 每条是
  `Ctrl+` 加一个字母。
- **两条变异都真红**（各自先断言锚点命中恰好 1 处、还原后 SHA256 一致、`touch` 后见
  `Compiling`）：
  - M1 宽度忘掉菜单自己的 padding ⇒ 绝对数字那条 FAILED；
  - M2 词与键的间距改成 0 ⇒ 同一条 FAILED。

### 仍未覆盖（如实）

- **"不换行"与"键右对齐"没有位图断言**：两者都在渲染层，需要设备上下文；位图断言要等
  一个能画菜单的测试装置（现有几何测试只测矩形，不画）。
- `measure_context_menu` 自身没有断言（需要 DWrite 工厂）；它的正确性由"宽度公式 + 按行量"
  两处纯逻辑覆盖，**不是**由它的返回值覆盖。
- 快捷键**不参与命中**：整行都是热区（`context_item_rect` 未改）。
## 原生控件的内缩，与两个"保存后"开关要跨面板记住

两件都不加功能，都是把已批准的改动落实。

### ① 直角探出圆角弧：内缩 1.5 → 2（**新增一个常量，而不是改旧的那个**）

`EDIT_INSET_DIP = 1.5` 同时被**两处**使用：原生控件的矩形，与**聚焦环**的位置（`frame.rs:743`）。
直接把它改成 2 会让环整体外移半个 dip —— 那是**另一处可见变化**，与本次缺陷无关。

所以拆成两个常量：`EDIT_INSET_DIP = 1.5`（聚焦环，纯绘制）与
`EDIT_CONTROL_INSET_DIP = 2.0`（原生控件）。依据：六 dip 圆角（`--r-input`，`:54`）的弧留下
`6·(1−1/√2) ≈ 1.76` dip 的弦，1.5 不够，控件的直角**真的探出弧外**；2 dip 在支持的每档缩放都够，
代价是横向半个 dip 的位移。

**那条绝对数字断言在 1× 下咬不到这次改动**：`1.5.round() == 2`，两个常量在一倍缩放下落在同一处。
所以补了一档 **1.5×**（`1.5×1.5 = 2.25 → 2`，`2×1.5 = 3`）——这是让"内缩够不够"可断言的唯一办法。

### ② 两个"保存后"开关要跨面板记住

它们是开关（`:573`/`:574`），描述的是**下一次保存**，所以是设置。面板每次捕获开一个，开关只在面板里
的话下一次就又是关的，第一次保存之后会**悄悄什么都不做**。

`Remembered` 增加 `after: [bool; 2]` 与 `remember_after`（与 `remember_template` 同一形状：没变就不写，
避免无谓唤醒 shell）。面板开窗时从它读初值，点击时把**整对**交回去 —— 一个写者。

### 变异（两条都真红，都先断言锚点命中恰好 1 处、见 `Compiling`、还原后逐字一致）

- M1 内缩改回 1.5 ⇒ `a_native_control_is_inset_and_centred_at_its_text_height` FAILED
  （`left: 152 / right: 153`，正是 2.25→2 与 3 的差）；
- M2 两个 bool 共用一个槽 ⇒ `the_two_after_save_switches_do_not_share_a_slot` FAILED
  （`left: [false, false] / right: [false, true]`）。

### 仍未覆盖（如实）

- "面板开窗时真的从 `Remembered.after` 读初值"没有窗口级测试（要真窗口）；规则本身有断言，
  但**调用**只靠编译与类型约束。
- 内缩是否真的消掉屏幕上那对直角，只能人眼：`EDIT` 由 GDI 画，位图断言看不到它。

## 原生控件的填充色与"打开文件夹"选中文件（`P9-83`）

### ① "还有一块原生背景" —— 颜色早就同源，剩下的成因是几何

核实结论：`field_colours(..).fill` 已经由 `fill_over_surface` 返回 **alpha 255**（把 `--fill` 的
8% 白在 surface 上合成好，再交给只能画不透明色的 GDI）⇒ 框与控件**同一个值**，颜色不是那块差异
的来源。真正让它看起来像"贴片"的是**几何**：控件的直角矩形躺在圆角框里，所以 `EDIT` 的矩形必须
整个落在圆角弧以内 —— 那条在 `P9-82` 已从 1.5 dip 提到 2 dip（`1.76 = 6·(1−1/√2)`）。

本批补的是**断言**，不是颜色：

- `the_control_is_filled_with_the_colour_the_frame_fills_its_box` —— 四个状态逐一断言
  `fill[3] == 255`，并断言 `bgr(fill)` 与 `fill` 的三个通道一致（`bgr` 是控件被告知的那个数）。
  它咬住的是"谁把半透明的值交给只画不透明色的控件"。
- **仪表**：`WM_CTLCOLOREDIT` 每次打印
  `[save-panel] fill child=… field=… state=… rgba=…`。它回答的是"控件到底有没有被问过"——
  没被问过的控件，它的填充不是我们给的，那就不是颜色问题。

**变异**：把 `fill_over_surface` 末位从 `255` 改成 `surface[3]`（=235）⇒
`assertion left == right failed: Idle: a native control cannot paint a translucent fill…` ✓

⚠️ 记一次自己的错：第一次变异写成 `255surface[3]]` ⇒ **语法错误** ⇒ 按清单第 1 条
"变异没发生 ≠ 断言弱"，那次"没红"**不算证据**；改成整段替换才拿到红。

### ② `打开文件夹` 要**选中**那个文件 —— 原写法只在没有空格时生效

```
std::process::Command::new("explorer").arg(format!("/select,{}", path))
```

- `Command::arg` 会给**含空格的参数**加引号 ⇒ 命令行成了 `explorer "/select,C:\My Files\a.png"`；
- 而 `explorer` 的 `/select,` 是**前缀**，不能落在引号里 ⇒ 它当成普通路径 ⇒ **打开目录、什么都没
  选中**；路径没有空格时看起来正常 —— 一个"有时能用"的 bug。

改成 `SHParseDisplayName`（目录）+ `ILCreateFromPathW`（文件）+ `SHOpenFolderAndSelectItems`
⇒ 没有引号这一层。决定抽成纯函数 `reveal_plan(path) -> Reveal`：文件在 ⇒ `Select(file)`；文件不在
（被移走/改名）⇒ `Open(所在目录)` —— 没有可指的东西时，打开目录是老实话。

测试用真临时文件断言两态，并断言"删掉之后打开的是目录"（这条是"不崩"的那一半）。

**变异**：`reveal_plan` 里 `if path.is_file()` 改成 `if true` ⇒
`left: Select(…\shot.png) / right: Open(…\snapclip-reveal-plan)` ✓

### 仍未覆盖

- `SHOpenFolderAndSelectItems` 的**真实效果**（资源管理器里真的高亮那个文件）没有窗口级测试，
  只能人眼；`reveal_plan` 只保证"选的是文件、不是目录"。
- `WM_CTLCOLOREDIT` 的仪表**没有测试**，它存在的意义就是让人眼一次读出来。

## 揭示的公寓，与字段框的两个色

用户报两件：名字框里"还有一块原生背景"、`打开文件夹` 不选中文件。第二件的根因在**公寓**上：

`SHOpenFolderAndSelectItems` 要求调用线程有 COM 公寓，而本文件的对话框路径刻意不需要公寓，
那行注释写的是"不需要 `CoInitialize`"，所以整个窗口从未初始化过 COM。没有公寓时这个函数
**返回 S_OK 而什么也不做** —— 目录打开了、文件没选中，日志里 `hresult=0x0` 看起来完全正常。
现在在 `Reveal::Select` 分支前后配对 `CoInitializeEx(COINIT_APARTMENTTHREADED)` / `CoUninitialize`，
且只有成功才配对（`S_FALSE` 表示线程本来就有公寓，不增加引用计数），`RPC_E_CHANGED_MODE` 会打进日志
而不是被吞掉 —— 那是另一种问题，需要另一种修法。日志同时带上每一步：公寓 HRESULT、`SHParseDisplayName`
的 HRESULT、两个 PIDL 是否为空、最终 HRESULT。

**第一件被量了一遍，而结论与"背景色不同"相反**：把用户截图（1380×846，客户区 920×564，1.5 倍）按像素读出来，
名字框内部在 x=1200 的竖扫上 y=267..315 **全是** `(41,41,46)`，与 `WM_CTLCOLOREDIT` 日志里告诉控件的
`[41,41,46,255]` **完全一致**。唯一异常是一条 `(72,73,81)` 的横带：y=290 横扫显示它从 x≈879 铺到 x≈1170，
正好在魔法棒让出的那 34 dip 之前结束 —— 也就是**控件的整条宽度**。`(41,41,46)` 再叠约 15% 白正是
`(72,73,81)`，所以这条带子在说"某个不透明的填充被画了两次"，而不是"颜色取错了"。

因此这一批只加仪表、不改颜色：`frame.rs` 的字段循环对 `FieldId::Name` 打印框自身的
`rect` 与 `rgba`。一次真机运行会把两个数并排放在控制台里 —— 若框是 `(41,41,46)` 而屏幕上是
`(72,73,81)`，那就说明这层底色不是这两个来源之一，下一轮该量的是**谁在这块区域又填了一次**。

### 仍未覆盖

- 两张仪表都只是打点，没有断言；它们存在的意义就是一次运行读出来。
- 那条 `(72,73,81)` 横带的来源**尚未定位**；本轮只把"两个候选值"并排摆出来。
### 三处入口都收起原生控件的选区

**问题**：打开面板后名字框里横着一条比框更亮的带子。量出来它在 `x≈1170` 结束，而框到 `x=1224`
—— 也就是说它**比框窄**，窄掉的那一截正好是魔法棒让出的 34 dip。所以它不是"控件的背景"，
而是**被选中文字的高亮**（`(72,73,81)` 正是系统给非活动窗口的选中色）。

**为什么修过一次又回来了**：`ShowWindow` 之后那一段早就收过初始选区，注释也写明了理由。但
"窗口管理器在激活控件时把内容全选"这件事**不止发生在开窗那一次** —— 它还会发生在控件拿到焦点、
以及窗口重新被激活的时候。只堵一个入口，另外两个入口照样能把高亮带回来。

**修法**：抽出 `collapse_native_selection(control)`，三处入口都调它 ——
开窗后（`ShowWindow` 之后）、`EN_SETFOCUS`（`set_focus` 之后、日志之前）、`WM_ACTIVATE` 且激活。
收起的目标位置由纯函数 `collapse_position(start, end)` 决定：**有选区收到 `end`**，**已经是光标就不动**
（幂等）。收到的位置刻意**不是 0** —— 用户点进框中部时光标就停在那儿，收到 0 会把光标弹到最前面，
那是没人要求过的可见变化。这也是为什么不写成 `EM_SETSEL(0, 0)`。

**证据**：新增纯逻辑测试四处断言（整段选中、局部选中、已折叠、空字段）。变异把 `Some(end)` 改成
`Some(0)` ⇒ 该测试红：

```
assertion `left == right` failed: a whole-field selection keeps the caret at the end, where typing left it
  left: Some(0)   right: Some(12)
```

**如实记下的一条**：把 `EN_SETFOCUS` 那一处调用删掉 ⇒ **整个 app 套件 292 条仍全绿** ——
也就是"三处都调了"**没有测试覆盖**，规则有断言、入口只靠编译与日志。要真守住这三处，
需要窗口级测试（真窗口）；目前只能靠一次真机读三行日志。

### 仍未覆盖

- 三处入口的调用没有测试（见上）；`WM_ACTIVATE` 的激活分支同样只在真机上才走到。
- 顺带解掉一个旧疑问：`capture` 的条数在 545/547 之间漂，是因为 `-p snapclip-capture` 不带
  feature、`--workspace` 会把 feature 统一起来；两套配置的条数本来就不同。

## 控件背景的另一半：视觉样式，与"shell 还不知道文件存在"

两件都是"**代码看着对、结果不对**"的一类，和前两次（`Esc`/`Enter` 消息不到、右键菜单被子窗口吃掉）同族。

### ① 控件背景 `rgb(73,73,78)` 的来源

四条假设被依次排除，靠的是**量**而不是读代码：

| 假设 | 排除方式 |
| --- | --- |
| 颜色算错 | 日志里框与控件**被告知的是同一个** `[41,41,46,255]`，刷子句柄非零 |
| 框叠了两次 | `frame.rs` 对 `Name` **只填一次**；四个状态同一个 `fill` |
| 是选区高亮 | `EM_GETSEL` 在开窗与聚焦后都是 `start==end` |
| 是我们选的全选 | 全仓 `EM_SETSEL(0,-1)` 只在右键菜单的"全选"项 |

⇒ 用户的双色实验（刷子=红、`SetBkColor`=绿）判读：**只有文字那条变红** ⇒ 再叠约 15% 白正好是 73 ⇒
**整块背景是 Windows 视觉样式（`EDIT` 主题部件）画的**，`WM_CTLCOLOREDIT` 返回的刷子**只影响字形背景**。

修法：建控件时 `SetWindowTheme(edit, L"", L"")` —— **空串**才是"不用主题"，`NULL` 是"按系统设置"（相反）。
随后在 `WM_CTLCOLOREDIT` 里显式 `SetBkMode(child, OPAQUE)`，让字形背景用同一个 `SetBkColor`（与刷子同索引、同值）。

### ② 揭示不选中：shell 的目录列表早于文件

用户观察是关键：**资源管理器先出现、文件随后才出现在列表里**。⇒ `SHOpenFolderAndSelectItems` 对一个
**它还不知道的名字**作答：返回 `S_OK`、什么也不选。日志 `select=0x0` 因此看起来完全正常。

修法：保存成功后**先** `SHChangeNotify(SHCNE_CREATE, SHCNF_PATHW, path, null)`，**再** reveal。
顺序写成**数据**（`AFTER_SAVE_STEPS`）而不是行序，所以交换是行为变化而不是看不见的重排：

```rust
pub const AFTER_SAVE_STEPS: [AfterSaveStep; 3] = [CopyPath, TellShell, Reveal];
```

断言 `the_shell_is_told_about_the_file_before_it_is_asked_to_select_it` 钉住这个顺序；
变异（交换后两项）⇒ `left: [CopyPath, Reveal, TellShell] / right: [CopyPath, TellShell, Reveal]` ✓。

### 诊断代码的去留

两个实验开关（`SNAPCLIP_DEBUG_EDIT_FILL`、`SNAPCLIP_DEBUG_REVEAL`）**已删除**；
`reveal: apartment=… parsed=… select=…` 三行**保留**，因为它们让"返回成功却什么也没做"不再无声。

### 仍未覆盖

- `SetWindowTheme` 的效果**没有纯逻辑断言**（需要真窗口），只能人眼：名字框整块应是 `#29292E`。
- "通知先于揭示"有断言，但**真实选中**仍要人眼；含空格的目录要试一次（旧写法的破绽在那里）。

## 名字框那一块颜色，与"揭示选了却没有选"

两件事在同一轮里被证伪了两次，最后都是"引擎在骗我们"而不是"我们算错了"。

### 一、控件背景：主题在画，刷子在喂文字

四条假设依次被排除：半透明合成不同源（`fill_over_surface` 早已合成成不透明 alpha 255）、刷子取错色（日志显示 brushed 非空且值等于框）、框叠了两次（`frame.rs` 对 `Name` 只填一次）、开窗全选（`CreateWindowExW` 的 `lpWindowName` 建初值，`EM_SETSEL(0,-1)` 只在右键菜单里）。

红/绿双色实验才把机制分开：**刷子只影响文字那一条**，整块背景由**视觉样式**画。

修法：`create_field_edit` 里 `SetWindowTheme(edit, L"", L"")`（**空串**才是"不用主题"，`NULL` 是"按系统设置"），并在 `WM_CTLCOLOREDIT` 对传进来的 HDC 调 `SetBkMode(OPAQUE)`；另加 `WM_ERASEBKGND` 子类分支，用面板的 `fill_brushes` 自己 `FillRect` 并 `return 1`——**不再依赖主题，也不依赖控件是否听话**。

`subclass_action(message, lparam)` 是唯一判定（`ForwardContextMenu` / `EraseBackground` / `PassThrough`），测试走 `WM_CHAR`/`WM_KEYDOWN`/`WM_SETFOCUS`/`WM_LBUTTONDOWN`/`WM_PAINT`/`WM_NCDESTROY` 六条放行、两条拦截；变异（把 `EraseBackground` 分支关掉）真红：`left: PassThrough right: EraseBackground`。

### 二、揭示：`S_OK` 但没选中，于是回退分支永远不执行

`SHOpenFolderAndSelectItems` 在本机返回 `S_OK` 而不选中，`revealed = Some(0)`，因此"失败才回退"的 `None` 分支**从未执行**——而 `explorer_select_parameters`（`/select,"<path>"`，引号在路径外、开关在内）早就正确并被测过，只是没被用上。

修法：shell 调用之后**总是**也走 `explorer.exe`（它复用刚打开的文件夹窗口，是选中而不是第二个窗口）；日志改成"shell 报告选中"与"shell 没有选中"两句，随后一句"请 explorer 选中"。

### 仍未覆盖

- `WM_ERASEBKGND` 的实际效果与 `SetWindowTheme` 一样，只能人眼：整块应是 `#29292E`。
- "总是也问 explorer"没有纯逻辑断言（顺序与调用在窗口过程里），只有 `explorer_select_parameters` 的引号有断言。

## 读数本身错了两处：`screen:` 探针与 `/select,`

### 一、`screen:` 读到的是窗口出现之前的桌面

第一次的读数 `outside=0xfffbfa top_edge=0xfffbfa middle=0xeeeae9` 近乎纯白，而面板是 `(41,41,46)`，所以那条读数**不是面板**。

- **时机** ✗：探针跟在 `shown` 那一行后面，而 `ShowWindow` 只是**排队**一次绘制 ⇒ 探针跑在首帧之前 ⇒ 读到桌面上原来的东西。修法：先 `RedrawWindow(RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN)`（**要 `RDW_ALLCHILDREN`**，否则子窗口 `EDIT` 还没画），再读。
- **坐标** ✗→**不成立**：本来怀疑要 `ClientToScreen`，但 `GetWindowRect` 返回的就是**屏幕坐标**，而 `GetDC(NULL)` 的 `GetPixel` 也吃屏幕坐标 ⇒ **两处都对**。这条假设在代码里被否证，写进注释。

读数现在多打一个 `box=(l,t,r,b)`，让"读的是哪一块"和数值一起可核。三个点同列：`outside`=框外 `left-2`、`top_edge`=上缘 `+2`、`middle`=文字行。

### 二、`/select,` 在已打开的窗口里不重新选中

用户两次测试都是"该文件夹已经打开"，而 `explorer /select,` 在这种情况**不改变选中**却仍返回成功（`ShellExecuteW` 的 `result=42 > 32`）⇒ 看起来一切都对，屏幕上什么都没发生。

修法：参数改成 `/e,/select,"<path>"` —— `/e,` 要求**新开一个窗口**，新窗口里的选中才会生效；代价是文件夹已开时会多一个窗口。断言两条：`explorer_select_opens_a_window_so_its_selection_is_honoured`（必须以 `/e,` 开头且仍含 `/select,`）+ 既有的引号位置测试（示例同步改成 `/e,/select,…`）。日志补 `args=`，让引号与开关在日志里看得见。

### 仍未覆盖

- `RedrawWindow` 之后 `EDIT` 是否真的重画完，只能由用户读 `screen:` 三值：三者相同 ⇒ 屏幕上没有带子（截图/色彩管理的产物）；只有 `middle` 不同 ⇒ 带子在文字行；`top_edge` 也不同 ⇒ 带子是整个控件。
- `/e,` 的实际效果只能人眼：新窗口里那个文件应被选中。


## 实验：把 DWM 材质关掉（`SNAPCLIP_NO_BACKDROP`）

### 为什么

同一块屏幕上有两个读数：程序 `GetPixel` 读到 `(43,43,48)`（即 `(41,41,46)` + 抗锯齿），而用户 `PrintScreen → 画图` 取色读到 `(72,73,81)`，约 `+32`。两者不可能同时描述同一层像素，所以中间必有一层合成。材质**只作用于顶层窗口**，而原生 `EDIT` 的盒子是 GDI 不透明填充、**不被材质染**——这正好产生"框被提亮、控件没有"的差值。

### 怎么做的

`apply_backdrop` 里只跳过**材质与帧延伸**两项，其余（暗色模式、圆角、描边）**照旧**——单变量实验，否则变化里就分不清哪一项造成的。

```rust
if std::env::var_os("SNAPCLIP_NO_BACKDROP").is_some() {
    eprintln!("[save-panel] backdrop disabled by env: the frame will paint its own surface");
    return false;
}
```

⇒ `apply_backdrop` 的返回值语义就是"有材质在显示"，所以返回 `false` **同时**让 `fill_surface = !s.backdrop` 变成 `true`，窗口自己填底色。**不需要第二处改动**——如果另写一个"填充开关"，两者就会有一天不一致。

日志两侧都说话：`backdrop applied type=DWMSBT_TRANSIENTWINDOW` / `backdrop disabled by env`。

### 判读

用户跑 `$env:SNAPCLIP_NO_BACKDROP="1"` 后 `PrintScreen → 画图` 取色：

- 名字框**一片均匀** ⇒ 材质就是成因 ⇒ 修法是让控件与框**都不受材质影响**（框不吃材质，或让控件区域由父窗口画）；
- **带子还在** ⇒ 不是材质 ⇒ 回到"子窗口/父窗口合成差异"另查。

### 不可测的部分

材质与合成器的行为没有纯逻辑断言可写，只能靠这一对人眼读数。这条实验的全部价值在于**它只改一个变量**。

---

## 采纳 B：永久不用 DWM 材质（D13 第四条偏离）

### 用户实测的结论

`SNAPCLIP_NO_BACKDROP=1` ⇒ 名字框**一片均匀** ✓ ⇒ **材质是那条亮带的成因** ✓✓

机理：材质是 DWM 在**顶层窗口内容**上合成时加的一层（实测约 +32/通道）✓；原生 `EDIT` 是**独立子窗口** ✓ ⇒ 它自己的不透明填充**不进材质的染色范围** ✗ ⇒ **GDI 子窗口不可能被材质染色** ⇒ 二者**不可调和** ✓。

⇒ 而面板**必须**保留一个原生控件（文件名要输中文 ✓ IME 不是自己该写的东西 ✓）⇒ 所以**放弃材质** ✓。

### 落地

| 件 | 改动 |
| --- | --- |
| `SNAPCLIP_NO_BACKDROP` | **删除** ✓（实验结束 ✓） |
| `apply_backdrop` → **`apply_dwm_chrome`** | 不再设材质 ✓；**保留**暗色模式 ✓ 圆角 ✓ 描边 ✓；返回 `()` ✗（它不再决定任何事 ✓） |
| `DWMWA_SYSTEMBACKDROP_TYPE` | 改成**显式请求 `DWMSBT_NONE`** ✓（`BACKDROP_TYPE = 1`）⇒ 将来的系统默认值不能把材质悄悄带回来 ✓ |
| `DwmExtendFrameIntoClientArea` | **删除** ✓ —— 它只服务"让材质透进客户区" ✓。**圆角不依赖它** ✗✓：那条配方是 `WM_NCCALCSIZE` 返回 0 + `DWMWA_WINDOW_CORNER_PREFERENCE` ✓，帧延伸是另一件事 ✓ |
| `WindowState.backdrop` | **删除字段** ✓（永远为 `false` ⇒ 留着就是无用实现 ✗） |
| `WM_ERASEBKGND` | 恒定返回 `0` ✓（不再有"有材质时返回 1"的分支 ✓） |
| `fill_surface` | 恒为 `true` ✓，注释指向 D13 第四条 ✓ |
| `WM_PAINT` 的兜底填充 | `if !painted` ✓（去掉 `&& !backdrop` ✓） |

### 证据

- **新断言** `the_panel_asks_for_no_material` ✓：`BACKDROP_TYPE == 1` ✓ —— 用**绝对数字** ✗✓（与常量比就是和自己比 ✓）。
- **变异真红** ✓：`BACKDROP_TYPE` 改成 `2` ⇒
  ```
  assertion `left == right` failed: DWMSBT_NONE is one and the acrylic that shows the seam is two
    left: 2   right: 1
  ```
  还原后 **SHA256 逐字节一致** ✓（`b6685a57…` ✓）、**见 `Compiling snapclip-app`** ✓。
- **真窗口读回** ✓：`the_panel_window_opens_and_closes` 里断言 `matches!(backdrop, None | Some(BACKDROP_TYPE))` ✗✓ ⇒ 有该属性的系统必须报告"无材质" ✓，没有该属性的系统报告 `None` ✓；报告 `Some(2)` 就说明接缝回来了 ✓。
- 门禁 ✓：`app 295/0/9` ✓、`ui` ✓、`check --workspace --all-targets` **0 warning / 无 `never used`** ✓、串行 workspace 全绿 ✓。

### 代价（写清 ✓）

**面板失去毛玻璃** ✓ —— 但那 8% 的透明在"没有背景可透"的对话框里本来也没有意义 ✓；换来的是**名字框与周围完全一致** ✓，而那是用户连续三轮指出的问题 ✓。

## GPUI 探针：左侧预览

`SNAPCLIP_GPUI_PROBE=1` 的窗口现在左边是预览卡、右边是六行表单，与原型同布局。

| 项 | 做法 | 出处 |
| --- | --- | --- |
| 卡片矩形 | `(16, 48, 572, 508)`，**边**不是尺寸 | padding 16；header 44 + body 4；卡 556×460（`DEMO` 80–85） |
| 棋盘格 | `16` dip 方块、两色，`div` 拼（无平铺背景可用） | `:111`；`frame.rs` 94/422/423 |
| 适配 | **调用面板自己的 `fit_preview`**，不重写规则 | `frame.rs:5429` |
| 图片 | 仓库里没有照片，用**已跟踪**图标 `assets/icon/close.png`（96×96） | —— |

两件事值得记：

1. **`fit_preview` 拒绝放大**（`scale.min(1.0)`，`frame.rs:5438`）⇒ 96 的图在 556×460 的卡里**按原尺寸居中**，不是铺满。断言用**字面值**写了这条规则的两个例子（`(246,230,342,326)` 与小图、`(64,48,524,508)` 与大图）。
2. **断言抓到了我自己的错**：`PROBE_CARD` 初版按 `(left, top, w, h)` 写，而 `fit_preview` 要 `(left, top, right, bottom)` ⇒ 卡宽算成 540 ⇒ 两个 fit 测试同时红，报的差正是 16 与 24。**矩形与尺寸的混淆只有断言能抓**。

`fit_preview` 原本没有从 `panel` 再导出，为探针**新增一行 re-export**（纯添加，行为不变）。不这样做就得在探针里重写一遍适配规则，那会变成第二套几何。

**未做**：预览区只显示一张固定图标，没有接捕获、没有底栏的复制、没有编码器（预估仍是原型样例）。

### 预览卡的圆角：框架的裁切是矩形，不是圆角

`.preview-frame` 在原型里是**同一条规则**里既圆角又裁切（`:111`，`--r-card: 10px`，`:53`）。探针照此写成 `rounded(PROBE_CARD_RADIUS)` + `overflow_hidden()`，圆角半径取 10 并加了断言（变异成 8 会红，报的是 `:53`/`:111`）。

**但 GPUI 的 `overflow_hidden` 只产出矩形裁切**：`gpui-fast-0.1.10/src/style.rs:639` 的 `overflow_mask` 只算 `min`/`max` 两个角点，**不读圆角半径**。所以：

- **超出部分**（棋盘格按 `ceil` 铺满会多出最多一格）现在被裁掉了 ✓；
- **四个角上的方块仍会盖住圆弧** ✗ —— 这是框架限制，改颜色、改半径都消不掉。

两条出路，代价不同，需要决定：
1. **省掉落在角上的方块**（只画圆心在内侧的那些）⇒ 角上露出卡片自己的圆角底色 ✓ 观感接近 ✓，代价是"棋盘格在角上不连续"这一处近似；
2. 等框架支持圆角裁切，或自己在 `paint` 层做 mask ⇒ 代价大。

记录为一条**待决**，不在探针里擅自选择。
