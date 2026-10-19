# 46 自定义截图面板的入口：从尺寸标签里解耦，并加一个快捷键入口

> 状态：**方案待实施**（只写文档，未改代码）。
> 前序：`docs/45-overlay-label-visibility.md` 记录了因果链与"单向门"的定性；
> 本文不改写那条链，只解决"入口"。

## 1. 问题

`docs/45` 已确认的单向门：

```
三项标签（尺寸 / 位置 / 坐标）全关
  ⇒ Label::text() 返回空串（crates/snapclip-capture/src/units.rs:189）
  ⇒ size_label_bounds 返回 None（crates/snapclip-capture/src/windows/win/d2d.rs:2552-2554）
  ⇒ label_badge = None（crates/snapclip-capture/src/windows/overlay/render_submit.rs:406）
  ⇒ 只有徽标能开面板（crates/snapclip-capture/src/windows/overlay/input.rs:270-274）
  ⇒ 面板打不开（geometry.rs:995-1019：去向 · 延迟 · 三项开关 · 图片卡 · 圆角 · 比例 · 开始 · 取消）
  ⇒ 而"把三项开关回来"的 UI 就在这个面板里
```

这是**锁定功能**，不是外观问题 ✗：用户关掉三个读数后，去向、延迟、圆角、比例、开始、取消全部不可达，只能离开截图去设置窗口恢复。

**根因不在"标签能不能空"** ✗，而在**入口挂在了标签的布局产物上** ✗：徽标的"存在"是 `text().is_empty()` 的推论（`d2d.rs:2552` 的注释明说"没有徽标可画，也就没有徽标菜单"，依据 `docs/23 §14.54`——这是**有意**的旧决定 ✓），而徽标同时是唯一的门 ✗。**标签可以隐藏，门不能。**

## 2. 三种方案

| 方案 | 做法 | 代价 | 结论 |
| --- | --- | --- | --- |
| **A 空浮层** | 文本为空也画一个空圆角块 | 无含义 + 挡住选区左上角 | ✗ 否决 |
| **B 最小图标徽标** | 文本为空时画一个小图标，悬停亮、点击开面板 | **改画面** ⇒ 要新设计稿；图标无含义 | ✗ 本次不采纳 |
| **C 快捷键入口**（推荐） | 新增 `Ctrl+Enter` 打开面板；**保留"有文字时点击徽标"** | 可发现性依赖快捷键指南 | ✓ 采纳 |

**推荐：C 为主**，并把一条工程修正写死：

> **入口的判定与命中一律不得依赖文本布局** ✗。
> 具体为：`toggle_region_panel` 的可达性只由"上下文 + 是否有选区"决定，**不得**再经过 `label_badge` / `label_bounds` / `text()`；
> 文本为空、或**文本测量失败**时（`d2d.rs` 里 `measure_text_*` 的 `Err` 分支），快捷键路径必须仍然可用 ✓。

**不画图标** ✗：既然有快捷键，"无含义的图标"是多余的；B 留给将来想要"可见锚点"时再做。

## 3. 快捷键

### 3.1 建议

| 项 | 值 |
| --- | --- |
| 组合 | **`Ctrl+Enter`** |
| Action | **`OpenRegionPanel`**（新增，放进 `keymap.rs` 的 `Action` 枚举，`crates/snapclip-capture/src/keymap.rs:28`） |
| 上下文 | **`Context::Pick`**（`keymap.rs:19-24`）；`Context::Scroll` 不绑定 |
| 生效条件 | 处于 `Context::Pick` **且已有选区**（`Selecting`/`Selected`）；无选区时**不动作**（不弹面板、不提示） |

### 3.2 冲突核查（已查，逐条给位置）

| 组合 | 现状 | 结论 |
| --- | --- | --- |
| **`Ctrl+Enter`** | `Pick` 上下文里 Ctrl 组合只有 `Ctrl+ArrowUp`（`keymap.rs:145`）与 `Ctrl+ArrowDown`（`keymap.rs:154`）；`Enter` 单独绑定 `Confirm`（`keymap.rs:262`） | **空闲** ✓，可绑定 |
| `Ctrl+S` | 色值格式是**裸 `S`**（`keymap.rs` 绑定表 `["S"]` → `Action::ColorFormat`；测试 `keymap.rs:1012` `pick(Key::Char('S'))`），**不是 `Ctrl+S`** | **`Ctrl+S` 其实空闲** ✗ —— 更正一条流传的说法；它被排除的理由改为"**`Ctrl+S` 在本产品语义上就是『保存』**，而保存面板正是要被这个快捷键打开的东西 ✗"，避免将来语义打架 |
| `Ctrl+Space` | 与输入法切换冲突 | ✗ 排除（IME 用户会撞） |
| `Tab` | 焦点导航 | ✗ 排除 |
| `Alt+Enter` | 属性 | ✗ 排除 |
| `Enter` | 已是 `Confirm`（`keymap.rs:262`），滚动上下文是 `ScrollFinish`（`keymap.rs:291`） | **不动** ✗ |

`keymap.rs` 只有通过校验才能构造（`keymap.rs:11`："`Keymap` is only ever built through validation"），并且"同一按键出现在两个上下文不算冲突"（`keymap.rs:1162`）⇒ 新绑定落在 `Pick` 上下文，天然通过校验 ✓。

### 3.3 面板已开时的行为

`input.rs:399-402`：面板打开时按键**先交给面板**（`on_region_panel_key`）✓ ⇒ 需要显式决定 `Ctrl+Enter`：

- **推荐：幂等**——面板已开时 `Ctrl+Enter` **什么都不做**（不关闭）✓；
- 理由：`Esc` 已经是"关面板 / 取消"的主语义（`input.rs:487` `Action::Cancel`）；让 `Ctrl+Enter` 既可开又可关，会与 `Esc` 形成两个相反语义的同义键 ✗。

## 4. 交互矩阵

| 状态 \ 按键 | `Ctrl+Enter` | `Enter` | `Esc` |
| --- | --- | --- | --- |
| **无选区**（`Pick`，尚未拖出） | 不动作 | 现有语义不变（`Confirm` 的守卫照样拒绝） | 取消/退出（现状） |
| **有选区，面板关闭** | **打开面板** ✓（新） | 确认截图（现状 `Confirm`） | 取消（现状） |
| **有选区，面板打开** | 不动作（幂等） | 由面板消费（`input.rs:399`，不再触发确认） | 关面板（现状） |
| **滚动截图**（`Scroll`） | 不绑定 ⇒ 不动作 | `ScrollFinish`（现状） | `ScrollCancel`（现状） |

## 5. 测试清单（TDD，先红后绿）

先红的原因都是"功能不存在" ✓，不是"测试写错" ✗。

**入口解耦**
1. `ctrl_enter_opens_the_region_panel_with_a_selection` —— 有选区、面板关闭 ⇒ 面板打开；
2. `ctrl_enter_does_nothing_without_a_selection` —— 无选区 ⇒ 面板仍关闭、状态不变；
3. `ctrl_enter_is_idempotent_while_the_panel_is_open` —— 面板已开 ⇒ 仍是打开的（不关闭、不重置选择）；
4. `ctrl_enter_does_not_apply_in_the_scroll_context` —— `Context::Scroll` 下不触发；
5. `every_label_combination_still_opens_the_panel_with_ctrl_enter` —— 三项开关的 **8 种组合**逐一断言：`Ctrl+Enter` 都能打开（这是"不再依赖文本布局"的主测试 ✓）；
6. `a_failed_text_measurement_still_leaves_ctrl_enter_working` —— 注入文本测量失败（`d2d.rs` 的 `measure_text_*` 返回 `Err`）⇒ 快捷键仍能开面板；
7. `the_badge_still_opens_the_panel_when_it_has_text` —— 有文字时，点击徽标的行为**保持不变**（防回归）；
8. `the_shortcut_is_registered_in_the_pick_context_only` —— `Action::OpenRegionPanel` 的绑定只在 `Pick`；`Scroll` 下 `action_for` 不含它。

**指南与文案**
9. `the_guide_lists_open_region_panel_with_ctrl_enter` —— 快捷键指南里出现该条目（文本由 `Action` 的 guide 生成，见 `keymap.rs:318` 与 `:875`）；
10. `every_guide_character_is_in_the_font_subset` —— 新增文案字符必须进字体子集白名单（现有覆盖门禁测试：`d2d/tests.rs` 的 "embedded subset has to render every character"）。

## 6. 影响面

| 面 | 影响 |
| --- | --- |
| `keymap.rs` | 新增 `Action::OpenRegionPanel` + 一条默认绑定 + guide 文案 + 两条测试（上下文/冲突） |
| `windows/overlay/input.rs` | 新增 case：`Action::OpenRegionPanel =>` 若面板关闭且无面板则 `toggle_region_panel()`；把 `toggle_region_panel` 的**可达性**与 `label_badge` 解耦（判定改为"有选区 + 面板未开"） |
| 命中测试 | **不改**：点击徽标的路径仍走 `label_badge.contains(point)`（有文字时才有徽标）✓ |
| **字体子集** | 新文案字符要进 `subfont/drawn-text.txt` 并重建子集，否则渲染成豆腐 ✗（覆盖门禁测试会红，这正是我们要的） |
| **设计稿** | **需要**：快捷键指南多一条 ⇒ `docs/design/overlay-ui-demo.html` 要加这一行 ✓（画面主体不变，只是指南多一项） |
| 设置窗口 | 可选：快捷键在设置里是否展示/可改，跟随现有 keymap 的展示机制 ✓（未查，标 `[待定]`） |
| `docs/45` | **结论要更正**：`docs/45` 推荐的是 **B（最小图标徽标）** ✗；本文改为 **C（快捷键，不画图标）** ✓ —— 提交本文时一并改 `docs/45` 的推荐段 |

## 7. 不做的事

- ✗ 不画图标徽标（B 留待将来）；
- ✗ 不改尺寸标签的绘制与文本布局（`d2d.rs` 的 `size_label_bounds` 保持"文本为空 ⇒ 无徽标"，这是旧决定，`docs/23 §14.54`）；
- ✗ 不改面板布局、不改去向/延迟/圆角/比例；
- ✗ 不改 `Enter` / `Esc` / `F9` 的现有语义；
- ✗ 不新增鼠标手势（拖拽、双击等）。

## 8. 未验证

- 快捷键指南的**具体渲染位置**（浮层底栏？）未查 —— 这决定 `docs/design/overlay-ui-demo.html` 要改哪一块（`[待定]`）；
- 设置窗口是否已有"快捷键"展示区未查（`[待定]`）；
- `Ctrl+Enter` 在**滚动截图会话中**是否被别处（非 keymap）处理未查（`[待定]`）；
- 文本测量失败注入的具体钩子位置未查（`d2d.rs` 的 `measure_text_*`，需实现时确认）。
