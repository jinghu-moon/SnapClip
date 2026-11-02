# 保存面板的控件层：先盘点已有的自绘控件

> 结论先说：**截图面板已经有一套自绘控件的雏形** ✓ —— 圆角面板底、图标、尺寸徽标、文本绘制、分段器、按钮行（含键位提示）、视图模型分层，以及一套**离屏渲染 + 像素断言**的测试装置。
> 保存面板真正缺的是**输入类控件**（TextField / 数字步进 / 下拉 / Toast / 拖拽区）和**通用焦点模型** ✗。
> 因此 D11 的"自绘 DirectWrite TextField"应从"**先把已有控件抽成层**"开始，而不是从零写 ✗。

## 0. S0 实施落点

第一批共享控件已经落在 `crates/snapclip-ui`，截图面板正在真实消费它，而不是保留一套平行实现：

| 能力 | 实现 | 当前使用者 |
| --- | --- | --- |
| 主题 token | `panel::PanelTheme` | `snapclip-capture` 的 RegionPanel 画刷初始化 |
| D2D 原语与 DirectWrite 测量 | `panel::d2d::PanelPainter` | RegionPanel、尺寸徽标、方向提示、注释文本 |
| 分段器 | `SegmentState` + `PanelPainter::segmented` | RegionPanel 的单位/去向/延时分段器 |
| 按钮 | `panel/button.rs`：`Button` / `ButtonState`，含命中和 D2D `paint` | RegionPanel 的动作、比例、关闭、开始/取消 |
| Tag 胶囊 | `panel/tag.rs`：`Tag` / `TagState`，只绘制选中/悬停/聚焦胶囊，不绘制 thumb | 截图面板的尺寸/位置/单位；保存面板后续复用 |
| 下拉菜单 | `panel/dropdown.rs`：按钮、菜单、选项和 `DropdownTarget`，含 D2D `paint` | 格式、质量等选项 |
| 单选框 | `panel/radio.rs`：`RadioGroup` / `RadioOption`，含 D2D `paint` | 互斥模式选择 |
| 图标资产 / 图标按钮 | `panel/icons.rs` 的 `Glyph`/PNG 资产契约、`panel/icon_button.rs` 的矩形与命中；WIC/D2D `PanelIcons` 缓存仍在 `snapclip-capture::windows::win::d2d::panel_icon`，因为它绑定设备生命周期 | 截图面板消费共享 glyph；保存面板复用同一资产契约，不复制资产或缓存 |

每个组件现在一个文件；组件只持有矩形和状态，业务动作由面板状态机持有。尺寸、位置、单位是可选显示标签，因此使用 `Tag` 胶囊，而不是带 thumb 的二值 `Toggle`。图标资产契约已经共享，设备相关缓存有意留在 renderer；`RegionPanelLayout` 仍是截图领域布局，跨面板布局契约是 S0 尚未收口的独立事项，不应在保存面板中复制截图布局。

## 1. 盘点：已有的自绘元素

| 元素 | 位置 | 交互 | 状态样式 | 命中测试 | 动画 | UIA |
| --- | --- | --- | --- | --- | --- | --- |
| 面板底（圆角 + 阴影） | `windows/win/d2d.rs:1860` `draw_panel_backdrop` | — | 单一 | — | 无 | 无 |
| 图标（工具/按钮图形） | `windows/win/d2d/panel_icon.rs:131` `draw` | — | 由调用方传色 | — | 无 | 无 |
| 字形/符号绘制 | `windows/win/d2d.rs:2461` `draw_glyph` | — | 同上 | — | 无 | 无 |
| 尺寸徽标 | `windows/win/d2d.rs:2491` `draw_size_label` | — | 单一 | — | 有（让位动画，见 `tests.rs:1213`） | 无 |
| 文本绘制（静态） | `windows/win/d2d.rs:2816` `draw_text_in`、`:3044` `draw_annotation_text` | — | 单一 | — | 无 | 无 |
| **分段器**（延时四档） | `windows/win/d2d.rs:2416` `draw_segmented`；模型 `region.rs:264` `Delay::ALL` | 点击换档 | 选中/未选中 | 有 | 有（插值，`tests.rs`） | 无 |
| **按钮行 + 键位提示**（滚动面板） | `scroll/layout.rs:34` `Button`；`:47` `BUTTONS`；`windows/overlay/scroll.rs:152-191` 组装 | 点击 / 快捷键 | 标签随状态变（`label(auto)`） | **有，且与绘制同源**（见下） | 无 | 无 |
| 滚动预览井 | `windows/win/d2d.rs:2732` `draw_scroll_well`、`:2648` `draw_scroll_panel` | — | 单一 | — | 无 | 无 |
| 图片卡片 / 区域面板 | `d2d.rs:1907` `draw_image_card`、`:2020` `draw_region_panel` | 见 `RegionPanel` | 多态（字段/比例/延时） | 有 | 有（滑入，`tests.rs:2176`） | 无 |
| 提示条 | `d2d.rs:1678` `draw_hint_at` | — | 单一 | — | 无 | 无 |
| 倒计时/遮罩 | `windows/overlay/dim_layer.rs` | — | 数字渐变 | — | 有 | 无 |
| **区域字段模型**（宽/高/X/Y） | `region.rs:16` `RegionField`；`:81` `get`、`:91` `set` | **只能拖拽/键推，不能输入** ✗ | — | 有 | 无 | 无 |
| **落点选择**（复制/贴图） | `region.rs:351-361`；**注释明写 `Save` 还没有** | 点击 | 选中 | 有 | 无 | 无 |
| 焦点/键盘 | `keymap.rs:21-47`（无"输入数字/打字"动作 ✗） | 会话级 | — | — | — | — |

两条值得单独说：

- **几何、绘制、命中三者同源** ✓：`scroll/layout.rs` 的模块注释写着"Pure arithmetic so the painter, the hit test and the window region agree by construction" ✓ —— 这正是控件层最该保留的性质 ✓。
- **测试装置已经是位图级** ✓：`windows/win/d2d/tests.rs:1974` `render_view()` 把视图渲到一个离屏位图并返回 `Vec<u8>` ✓，配 `:28` `pixel_at`、`:64` `assert_pixel_close`、`:285` `chrome_stays_out_of_the_exported_pixels`、`:1074` `size_label_panel_is_painted_at_the_selection_top_left` ✓。→ **"控件六态位图对比"不需要新建基础设施** ✓。

## 2. 分类

### 可直接复用 ✓

| 项 | 依据 |
| --- | --- |
| 圆角面板底 + 阴影 | `draw_panel_backdrop:1860` |
| 图标/字形 | `panel_icon::draw:131`、`draw_glyph:2461` |
| 尺寸徽标 + 让位动画 | `draw_size_label:2491`、`tests.rs:1213` |
| 静态文本绘制 | `draw_text_in:2816` |
| 分段器（含选中态与插值） | `draw_segmented:2416`、`region.rs:264` |
| 按钮"标签 + 动作 + 键位提示"三合一 | `scroll/layout.rs:34-60` |
| 视图模型分层（view struct → painter） | `d2d.rs:175` `RegionPanelView`、`:238` `RenderView` |
| 离屏渲染 + 像素断言测试 | `d2d/tests.rs:1974` 等 |

### 可适配 ✓（逻辑在，但被具体面板绑住）

| 项 | 现状 | 要抽出什么 |
| --- | --- | --- |
| `Button` / `BUTTONS` | 滚动面板仍有 2 个领域按钮；通用矩形命中已抽为 `snapclip_ui::panel::Button` | 后续按保存面板需要补 `ButtonSpec { label, action, enabled, tooltip }`，不复制命中逻辑 |
| `draw_segmented` | 已由 `PanelPainter::segmented` 接管，领域只负责组装 `SegmentState` | 保存面板直接提供状态与文案 |
| 面板布局 | `scroll/layout.rs` 只服务滚动面板 ✗ | 抽出 `PanelLayout`（宽/内边距/行高/徽标位）供保存面板复用 |
| 状态样式 | 每个绘制函数里写死色值 ✗ | 一个 `StateStyle { fill, border, text } × {default, hover, pressed, focused, disabled, error, warning}` |
| 焦点处理 | 会话级（`keymap::Action` 全局派发）✗ | 控件树级焦点 + `Tab` 顺序 |
| 命中 | 各面板自己算 ✓ 但没统一入口 ✗ | `Widget::hit_test(&self, pt) -> Option<WidgetId>` |

### 缺失 ✗

| 项 | 说明 |
| --- | --- |
| **TextField** | 光标、选区、拖选、双击选词、撤销、复制粘贴、键盘导航、IME 组合串、UI Automation —— **一样都没有** ✗ |
| **数字步进/数值输入** | `RegionField` 只有 `set()` ✗，没有"正在编辑"的状态 |
| **下拉/选择器** | 只有分段器（选项 ≤4 时够用 ✓，格式/质量等下拉不够 ✗） |
| **Toast** | 现有只有静态状态行 ✗，没有"出现→停留→淡出"的生命周期 |
| **拖拽区（拖出文件）** | 无 ✗ |
| **复选框/开关** | 无 ✗（若保存面板需要"同时复制"等开关） |
| **我们自己的 UIA 服务端** | 现有 UIA 代码是**窗口检测的客户端** ✗，不暴露本产品 UI |

## 3. 控件层设计

### 3.1 三层，一层一职责

```
Widget           layout(bounds) + hit_test(pt) -> Option<Id> + paint(ctx, state)
Interactive      + hover/pressed/focused/disabled + keyboard nav + focus ring
TextInput        + caret / selection / IME composition / clipboard / undo / UIA
```

- **不做通用 UI 框架** ✗：这是一层"面板内控件"，不是布局引擎 ✓；布局仍走已有的"纯算术 + 由调用方给定 bounds" ✓（`scroll/layout.rs` 的模式 ✓）。
- **`StateStyle` 是唯一色板来源** ✓：`{default, hover, pressed, focused, disabled, error, warning}` × `{fill, border, text}` ✓ —— 保存面板的输入框在 error/warning 两态有明确要求（`docs/42` D11 ✓），必须有统一来源 ✓。
- **命中与绘制同源** ✓：控件自己算 bounds ✓，painter 只画 ✓，hit test 只查 ✓ ⇒ 三处不会漂移 ✓（这是现有 `scroll/layout.rs` 已经证明过的性质 ✓）。

### 3.2 与现有代码的关系

| 文件 | 变化 |
| --- | --- |
| `windows/win/d2d.rs` | **只被消费** ✓：绘制函数的签名从"面板专用"收敛到"控件通用" ✓ |
| `scroll/layout.rs` | 成为**第一个使用者** ✓；其中通用部分上移 ✓ |
| `windows/overlay/*` | 改造为"持有控件树 + 转发消息" ✓，不再自己算按钮矩形 ✓ |
| `keymap.rs` | 增加 `Action::TypeChar / Backspace / Tab / ShiftTab` 等**输入动作** ✗（现在完全没有） |

### 3.3 D11 的修正

原 D11 写的是"**自绘 DirectWrite TextField**" ✓ —— 方向对 ✓，但**起点错** ✗。

**修正为三个步骤** ✓：

1. **抽出控件层**（复用 §2 的"可直接复用"与"可适配" ✓）—— 让 `Button`、`Segmented`、`Text`、`Badge` 成为可组合控件 ✓；
2. **补 `TextField`**：按 `docs/42` D11 的八项行为实现 ✓（插入点/选区/拖选/双击选词/撤销/复制粘贴/键盘导航/IME ✓）+ `IMM32` 组合串与候选窗定位 ✓ + UIA 暴露 ✓；
3. **补 `Stepper` / `Select` / `Toast` / 拖拽区** ✓。

**新增代码量级**（粗估 ✓，供 S5 重排用 ✓）：

| 项 | 人天 |
| --- | ---: |
| 控件层抽出（含 `StateStyle`、焦点、命中统一 ✓） | **3–5** |
| `TextField`（八项行为 + IMM32 + UIA ✓） | **6–10** ✗ |
| `Stepper` / `Select` / `Toast` / 拖拽区 | **3–5** |
| 控件六态位图测试（复用现有装置 ✓） | **1–2** |

## 4. 验收（可自动化 ✓）

| 层 | 方法 |
| --- | --- |
| **视觉（六态）** | 离屏渲染 → `Vec<u8>` → 与期望位图比对 ✓ —— **沿用 `d2d/tests.rs` 的 `render_view` + `assert_pixel_close`** ✓（default / hover / focused / error / warning / IME 组合 ✓） |
| **交互** | 合成消息驱动 ✓：`WM_MOUSEMOVE` / `WM_LBUTTONDOWN|UP` / `WM_KEYDOWN` / `WM_CHAR` / `WM_IME_COMPOSITION` ✓ |
| **焦点与导航** | 断言 `Tab`/`Shift+Tab` 的焦点顺序 ✓、焦点环绘制 ✓、模态内焦点不逃逸 ✓ |
| **UIA** | 断言控件暴露了正确的 `ControlType` / `Name` / `IsEnabled` / `Value` ✓（可用现有 UIA 客户端代码反向验证 ✓） |
| **无法自动化的** | 动画观感 / 高对比视觉 ✓ —— 人工 ✓，且必须写明"只承认人工结论" ✓ |

## 5. 对 `docs/41` 的影响

| 阶段 | 原估 | 现在 | 说明 |
| --- | ---: | ---: | --- |
| S5（表单与 TextField） | 6–9 | **13–22** ✗ | 控件层 3–5 + TextField 6–10 + 其余 3–5 + 测试 1–2 ✓ |
| S9（动效/UIA 验收） | 4–6 | **2–3** ✓ | 位图对比装置已有 ✓，UIA 可复用客户端 ✓ |
| **合计** | 10–15 | **15–25** | ✗ 净增 5–10 人天 |

**诚实的结论** ✗：自绘 TextField **比原生 `EDIT` 贵得多** ✓ —— 原生控件免费提供 IME/选区/撤销/粘贴/UIA ✓，自绘要一样一样实现 ✓。控件层能降低**后续控件**的边际成本 ✓，但**不能降低 TextField 本身**的成本 ✗。这笔钱买的是：**与 demo 的视觉一致** ✓ + **一整套可复用控件**（后续面板都受益 ✓）+ **位图级回归测试** ✓。

## 6. 一句话回答

> **现有自绘控件足以支撑保存面板的"外壳与按钮"** ✓（面板底、图标、文本、徽标、分段器、按钮行、键位提示、离屏位图测试都在 ✓）；
> **不足以支撑"输入"** ✗（TextField、数字步进、下拉、Toast、拖拽区、通用焦点、我们的 UIA 服务端全缺）✓。
> 所以 D11 的正确起点是**先把已有控件抽成层**（3–5 人天 ✓），再补 TextField（6–10 人天 ✗）。

## 7. Rust 生态参考：D2D + DirectWrite + WIC

### 7.1 最接近目标的组件库：Quel'Thalas

[Quel'Thalas](https://github.com/wuweiran/quelthalas) 是目前检索到最贴近"Rust + Win32 + D2D + DirectWrite + WIC + 自绘控件"这一组合的项目。其 [crates.io 页面](https://crates.io/crates/quelthalas) 标示版本 0.5.0、MIT、Windows only；项目 README 列出了 Button、Input、Textarea、ComboBox、Slider、SpinButton、Switch、TabList、TreeView、Tooltip 等组件。它使用 `windows` crate，而非提供一套跨平台渲染抽象。

从 [docs.rs 的库入口源码](https://docs.rs/quelthalas/latest/src/quelthalas/lib.rs.html) 可核实，它集中持有 `ID2D1Factory1`、`IDWriteFactory` 与 `IWICImagingFactory`，并持有主题 token、动画服务和描边样式；控件源码（例如 [Input](https://docs.rs/quelthalas/latest/src/quelthalas/component/input.rs.html)）直接使用 D2D/DWrite 做绘制与文本布局，并自行处理 Win32 输入、焦点、选区、剪贴板和 IME。它的设计是**在原生 child HWND 上重绘经典控件**，而不是把所有控件画进一个窗口的 retained widget tree。

因此，Quel'Thalas 是**最值得做源码级对照的实现**，但现阶段不能据 README 就认定为可直接接入的依赖：

- 宿主模型不同：SnapClip 现有面板在一个窗口的 D2D 绘制/命中流程里组合视图；Quel'Thalas 的控件拥有独立 HWND、窗口过程和生命周期。
- 输入实现有研究价值：可对照字符缓冲、selection/caret 几何、鼠标捕获、键盘编辑、剪贴板、IME 候选窗定位和失效重绘的职责划分；这些行为仍须按 SnapClip 的需求和测试重新验收。
- 控件覆盖广不等于质量已被本项目验证：截至本次检索，crates.io 显示 0 dependents、约百次下载量级。该指标只说明采用面较小，不能单独证明实现好坏；应先固定 crate 版本并做最小编译、焦点/IME、DPI、多显示器和销毁重建验证。
- 核查 UI Automation、键盘可访问性、DPI 变化、D2D device-loss 重建等能力时，应逐项查源码和测试；本次 README 与抽查的 Input 源码不足以证明这些完整性。

### 7.2 其他可比较项目

| 项目 | 与目标栈的关系 | 对 SnapClip 的参考价值 | 不宜直接照搬之处 |
| --- | --- | --- | --- |
| [Kyute](https://github.com/ennis/kyute) | Windows-only GUI；Skia 绘制，DirectWrite 做文字布局；不是 D2D + WIC 组件库 | 保留式 widget 组合、布局、状态失效、TextEdit 作为整体 UI 的思路 | 渲染器与 SnapClip 不同；仓库规模/维护状态需单独核实，不是底层 D2D 控件实现 |
| [nsh-platform-windows](https://docs.rs/nsh-platform-windows/latest/nsh_platform_windows/) | Win32 surface + D2D 绘制 retained view tree，DirectWrite 在引擎线程做文本布局，flip-model swap chain 呈现 | 视图树、提交/绘制分离、统一渲染线程和提交消息的边界 | 该项目含 web-view/JS 场景，线程模型和 SnapClip 的轻量面板不同；资料没有证明它是完整通用控件库 |
| [Native Windows GUI](https://docs.rs/native-windows-gui/latest/native_windows_gui/) | Win32 控件工具包，提供 Direct2D canvas/backend，但不是 D2D/DWrite/WIC 一体的 retained 自绘控件层 | Win32 消息分发、控件封装、事件处理和小型 Windows 工具应用的边界 | 文档标记项目不再维护；大量控件使用系统原生控件，视觉一致性与单画布布局不匹配 |
| [wind-ui-rust](https://github.com/huanfeng/wind-ui-rust) | tiny-skia + 原生 DirectWrite/Core Text；跨平台；非 D2D/WIC | 平台无关 widget/layout/event 与平台文本引擎分离；retained tree、dirty rendering、截图测试 | 与项目选定的 D2D 绘制后端不同，作为架构参照即可 |
| [raxis](https://github.com/emmachase/raxis) 的 `Image` widget | 使用 `windows` crate 的 WIC decoder/format converter，把图片转为 32bppPBGRA 后创建 `ID2D1Bitmap` 并缓存 | WIC 图片加载、intrinsic size、fit 模式、D2D bitmap 缓存和 device context 的代码组织 | 这是单个控件实现，不是稳定的通用组件发行版；仍需补 device-loss、异步取消和线程约束测试 |
| [Core UI](https://github.com/ghboke/core-ui) | C/C++ Direct2D/Direct3D UI 库，通过 C ABI 可供 Rust 调用；不是 Rust 原生库，也未确认 WIC 是其组件资产管线 | 可观察成熟度更高的控件清单、主题 token、声明式布局、调试注入和 golden-image 测试设计 | 引入外部 DLL/C ABI、C++ 构建链与另一套运行时/视觉体系；不是本项目封装自有控件的直接答案 |
| [directwrite-rs](https://github.com/Connicpu/directwrite-rs) | 较早的 DirectWrite 安全抽象，目标是与 Direct2D 配合做文字绘制 | 可比较 Rust 对 COM 接口、factory、text format/layout 的薄封装方式 | 不是控件库，项目较早且不覆盖 WIC、焦点、输入或主题系统 |
| [`windows` crate](https://crates.io/crates/windows) | Rust Windows API bindings，覆盖 D2D、DirectWrite、Imaging/WIC | 项目当前使用的系统 API 边界；绑定提供 API，不提供控件行为或 UI 架构 | 不能把绑定层当作组件库；缓存、布局、命中、焦点、文本编辑仍由应用实现 |

### 7.3 官方资料推导出的边界

Microsoft 将三者定义为可组合但职责不同的基础设施：[Direct2D interoperability](https://learn.microsoft.com/en-us/windows/win32/direct2d/interoperability-overview) 说明 D2D 通过 `DrawText` / `DrawTextLayout` / `DrawGlyphRun` 与 DirectWrite 集成，并通过 `CreateBitmapFromWicBitmap` 等接口接入 WIC；[Direct2D API overview](https://learn.microsoft.com/en-us/windows/win32/direct2d/the-direct2d-api) 说明 factory、render target、brush/bitmap 的资源关系及设备丢失边界；[WIC-to-D2D 示例](https://learn.microsoft.com/en-us/windows/win32/wic/-wic-bitmapsources-howto-drawusingd2d) 展示图像解码、转换为 32bppPBGRA、创建 D2D bitmap、绘制的流水线。

对 SnapClip 的工程含义：D2D/DirectWrite/WIC 是绘制、文字和图像的原生服务，不会自动给出控件模型。控件抽取层应拥有稳定的绘制/文本/图像能力接口，但不需要再包一层通用图形框架；D2D device-dependent 资源应由 renderer/device 生命周期统一创建、缓存和重建，不能让每个控件重复创建 factory 或在每帧解码图片。WIC 可作为图片输入/格式转换通路，输出统一的预乘 alpha 像素后再进入 D2D。

### 7.4 对 S0 的落地建议

1. **仍以抽取 SnapClip 已有能力为 S0 主线**：`PanelPainter`、`PanelTheme`、`SegmentState` 和基础 `Button` 命中已经落地并接入 RegionPanel；剩余工作是图标位图缓存、跨面板布局契约和迁移前后固定状态的像素比较。保存面板先消费已落地能力，再补缺口。
2. **把 Quel'Thalas 固定版本作为实现参照，不先引入为运行时依赖**：优先阅读 Input、Textarea、Button、ComboBox、Theme 与 scroll helper。只记录可迁移的算法和 Win32 消息处理，避免复制它的独立 HWND 宿主架构。
3. **为保存面板新增的 TextField 单独定义文本编辑核心与 Win32 adapter**：编辑状态（UTF-16/selection/undo）和由 DWrite layout 产生的测量、caret、命中几何保持同源；IMM32、剪贴板、窗口消息属于 adapter。它应消费 S0 的 renderer/layout/theme 契约，不把输入编辑逻辑塞进 D2D painter。
4. **设接入门槛而非按项目名选型**：只有当候选库能在现有窗口/渲染设备模型下复用，且构建、视觉基线、DPI、焦点、IME、资源重建、UIA 与维护状态均通过项目测试，才考虑直接依赖。否则将其作为行为参考，SnapClip 自己维护更小的控件集合。

本次检索结论：**存在与指定 API 栈吻合的 Rust 自绘组件库，首选研究 Quel'Thalas；但它不是已经证明适配 SnapClip 的直接依赖。** 对本项目更确定的收益来自复用其控件行为设计，同时沿 SnapClip 现有单窗口/同源布局与命中机制抽出最小控件层。
