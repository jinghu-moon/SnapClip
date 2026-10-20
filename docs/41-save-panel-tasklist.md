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

**校验 11 条**（字形覆盖由 S0 建立；字段语义和反馈在 S5 实现）：

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

> **`snapclip-ui` 的 `TextField` 与本文无关** ✗：D11 定为原生 `EDIT` ✓ ⇒ 保存面板**不使用**该控件 ✓；其去留见 `docs/47` 与 `docs/42 D11`（**当前无任何调用方** ✗，建议删除或明确标注预留，由上层决定 ✓）。

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
