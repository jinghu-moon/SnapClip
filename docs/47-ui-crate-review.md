# 47 · `crates/snapclip-ui/` 审查：是抽取，但把 S5/S6 的活提前做了

审查对象：`crates/snapclip-ui/`（19 个文件 ≈ 2 400 行 + `examples/component_gallery.rs` 804 行）。
纪律依据：`docs/41` 的 S0（"**不能以代码抽取完成代替验收**"）、`docs/43` 的 8 类可复用能力盘点。

## 结论摘要

- **抽取是真的** ✓ —— 面板控件的绘制已经搬进 `snapclip-ui`，capture 侧只剩 **chrome / 放大镜 / 指南** 自己的图元；**迁移没有放宽任何断言** ✓（`d2d/tests.rs` 被删的 4 行全是路径改名）。
- **封装是"样式 + 几何"，不是"交互组件"** ⚠️ —— `PanelPainter` 给图元与分段器，**命中测试仍留在调用方**；`PanelPainter` 在 capture 侧有 **12 处构造**、笔刷由调用方逐个提供。
- **S5/S6 的活被提前做了** ✗ —— `text_field.rs` 是**真的编辑模型**（光标/选区/删除/UTF-8 边界测试），但**没有 IME / UIA / 剪贴板 / 撤销 / 鼠标拖选**；`dropdown/slider/checkbox/radio/toggle` 同样属 S6 范围。按 `docs/41`，S0 只应交付**契约**。

## 逐条清单

| # | 问 | 结论 | 证据 |
| --- | --- | --- | --- |
| A1 | capture 里旧绘制还在吗 | **已迁走** ✓（残余 5 处是 chrome） | `guide_pass.rs:351`、`magnifier_pass.rs:223/296/391`、`d2d.rs:1559` 是放大镜/指南/底色，不是面板控件 |
| A2 | capture 怎么用 ui | **真调用** ✓（14 处） | `d2d.rs:74-76`（`Glyph/PanelTheme/Segment/Tag/PanelPainter`）、`d2d.rs` 内 **12 处** `PanelPainter { … }` 构造、`geometry.rs:960`（`Button/Dropdown/IconButton`） |
| A2b | 几何是否同源 | **是** ✓（ui 再导出） | `geometry.rs:959/1115/1191/1194` 再导出 `segment_slide`、`SEGMENT_PAD_DIP`、`segment_radius`、`segment_outer_radius` |
| A3 | 断言被放宽/删除吗 | **没有** ✓ | `git diff` 中 `d2d/tests.rs` 的 4 行删除全是 `crate::icons::` → `snapclip_ui::panel::` 路径改名 |
| A3b | 迁移后测试 | **绿** ✓ | `snapclip-capture` **534 / 0 / 80**（与迁移前一致）、`snapclip-ui` **40 / 0** |
| B4 | API 是否中性 | **是** ✓ | `text_field.rs:1` 用 `snapclip_model::geometry::{Point, Rect}`；选区/滚动/区域面板布局仍在 capture（`geometry.rs:965` 起 `RegionPanelLayout`） |
| B5 | 契约是否完整（paint/hit/layout/state） | **不完整** ✗ | `ui/src/panel/d2d.rs` 只有图元（`fill_round` / `stroke_round` / `line` / `draw_text*` / `measure_*` / `segmented`）；**没有 `hit_test`**，命中仍在 capture（`geometry.rs` 的 `target_at` 读同一份矩形） |
| B6 | 状态与样式是否统一 | **是** ✓ | `ui/src/panel/interaction.rs`（86 行）、`theme.rs`（26 行） |
| B7 | 可测试性 | **是** ✓ | ui 40 tests；`text_field.rs:268` `editing_preserves_utf8_boundaries_and_selection` |
| C8 | 有没有提前做 S5/S6 | **有** ✗ | `text_field.rs`（270 行，光标/选区/删除 ✓，**IME/UIA 缺失** ✗）、`dropdown.rs` 232、`slider.rs` 179、`checkbox.rs` 213、`radio.rs` 196、`toggle.rs` 161 |
| C9 | TextField 是否远超 S0 | **是** ✗ | 有编辑模型与测试，但**无 IME**（grep 无 `WM_IME`/`composition`）、**无 UIA**、无剪贴板/撤销/鼠标拖选 |
| D10 | 依赖方向 | **干净** ✓ | `snapclip-ui` 只依赖 `snapclip-model` + `windows-numerics` + `windows`；capture 依赖 ui（`crates/snapclip-capture/Cargo.toml`）；无环、无反向依赖 |
| D11 | 真跑 | **绿** ✓ | `cargo test -p snapclip-ui` 40 passed；`cargo test -p snapclip-capture --lib` 534 passed；`cargo check --workspace --all-targets` 0 warning |
| D12 | `component_gallery` 是什么 | **验收装置** ✓ | 804 行，逐控件独立绘制；属"能在无截图流程下看控件"的有用装置，不是产品页面 |

## 必须修的问题（按严重度）

1. **P0 · 把"提前做的控件"与 S0 的契约区分开** ✗
   `snapclip-ui` 里已有 `text_field`/`dropdown`/`slider`/`checkbox`/`radio`/`toggle` 的**实现**，但 **IME / UIA / 剪贴板 / 撤销** 全缺 ✗。若 `docs/41` 的 S0 交付物写成"已有自绘控件封装"，读者会误以为 S5 的输入控件已经完成 ✗ —— 这正是 S0 那句"不能以代码抽取完成代替验收"防的事 ✓。**修法**：在 `docs/41` S0 的交付物里明确标注：`snapclip-ui` 只交付**契约 + 纯模型 + 视觉样式**，**输入控件的 IME/UIA 属 S5**。

2. **P1 · `PanelPainter` 让调用方逐个提供笔刷** ✗
   capture 侧 **12 处** `PanelPainter { … }` 构造，每处自带一组笔刷 ⇒ 新增控件要在调用方再插一遍笔刷 ✗。**修法**：把"控件 → 笔刷"的映射收进 `snapclip-ui`（一个 `PanelBrushes` 或内建主题），调用方只传渲染目标。

3. **P2 · 命中测试没有随控件进 ui** ✗
   现在靠"同一份矩形"保证画与点一致 ✓（`geometry.rs` 注释已写明 ✓），但**没有封装** ⇒ 每个新控件都要在调用方再写一份命中 ✓。**修法**：把 `hit_test` 与 `paint` 一起放进控件定义（同一处几何 ✓）。

## 不算问题的地方

- capture 侧残余的 **5 处 `FillRoundedRectangle`** ✓ —— 都在 chrome / 放大镜 / 指南，是**背景与浮窗**，不是面板控件 ✓；
- **`examples/component_gallery.rs` 804 行** ✓ —— 是**验收装置**（每控件可独立绘制），保留 ✓；
- **几何再导出**（`geometry.rs:959/1115/1191/1194`）✓ —— 保留"几何/绘制/命中三者同源"的既有性质 ✓，**没有引入布局引擎** ✓。

## 三句话

1. **是真抽取** ✓ —— 面板控件已迁入 `snapclip-ui`，capture 只剩 chrome 的图元；**迁移没有放宽任何断言**（删掉的 4 行全是路径改名）。
2. **封装部分达标** ✓ —— 几何与样式同源、依赖方向干净、40 + 534 全绿；**最差的一处是 `PanelPainter` 的笔刷外泄**（capture 侧 12 处构造，新增控件要给调用方再加笔刷）。
3. **提前做了 S5/S6 的活** ✗ —— `text_field` 是有模型的真实现但**缺 IME/UIA/剪贴板/撤销**，`dropdown/slider/checkbox/radio/toggle` 属 S6 范围；必须在 `docs/41` 里把"S0 = 契约 + 纯模型"与"S5 = 输入控件"划清，否则会被当成已完成。

## 未验证

- 未在真实窗口里跑 `component_gallery`（按指示不必真跑 GUI）；控件的**实际像素**是否与 demo 一致未比对；
- `text_field` 的 `caret_visible(elapsed_ms)` 是否与面板动画时钟同源未核对。

> **后续（不改本文结论）**：`snapclip-ui/src/panel/text_field.rs` 已在 `P8-03` **删除** ✓（零调用方 ✓）；保存面板改用**原生 `EDIT`**（`docs/42` D11 ✓）。本文对提前做到 S6 的判断作为历史证据保留 ✓。
