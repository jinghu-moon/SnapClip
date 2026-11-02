# SnapClip Windows IME 集成调研

> 调研日期：2026-10-19  
> 适用范围：保存面板的自绘 DirectWrite TextField 以及现有截图覆盖层  
> 结论性质：工程调研资料；产品决策仍以 `docs/42-save-panel-decisions.md` 为准

## 1. 结论先行

项目中已经有 **IME 基础代码**，但它不是通用文本输入实现。目前代码只服务于截图覆盖层中的数字设置面板：

- 保存并恢复覆盖层窗口的 `HIMC`；
- 面板打开时通过 `ImmAssociateContext(hwnd, null)` 暂时摘除 IME；
- 通过 `WM_CHAR` 接收 ASCII 数字和全角数字；
- 面板关闭时恢复原来的 IME context。

这套逻辑的目标是让数字快捷键不被中文输入法拦截，不能用于保存面板的文件名、路径和重命名输入框。自绘文本框仍需要处理预编辑、提交、取消、组合光标和候选窗定位。

目前没有发现一个可以无改造地嵌入 SnapClip 单 HWND、单 D2D 绘制流程的 Rust IME/TextField 库。

推荐方案是：

1. 保持保存面板为自绘 DirectWrite TextField；
2. 复用 IMM32 的消息和 API 语义，实现一个很薄的 `ImeController`；
3. 把 IME 消息适配与纯文本编辑状态分离；
4. 用 DirectWrite 的文本布局计算 caret，再定位系统组合窗口和候选窗口；
5. 只有在需要快速验证时，才把 Quel'Thalas 作为独立 child HWND 的备选实现。

## 2. 项目现状

### 2.1 已有依赖

`crates/snapclip-capture/Cargo.toml` 已启用：

- `windows` 的 `Win32_UI_Input_Ime`；
- `windows-sys` 的 `Win32_UI_Input_Ime`。

因此不需要为基础 IMM32 API 额外引入系统绑定依赖。

### 2.2 已有实现位置

| 文件 | 现有职责 | 能否作为通用文本框 IME |
| --- | --- | --- |
| `crates/snapclip-capture/src/windows/overlay.rs` | 保存覆盖层原有 `HIMC` | 不能，只有窗口级保存状态 |
| `crates/snapclip-capture/src/windows/overlay/window_restore.rs` | `ImmAssociateContext` 摘除与恢复 | 不能，这是禁用 IME 的操作 |
| `crates/snapclip-capture/src/windows/overlay/input.rs` | `WM_CHAR` 转数字面板按键 | 不能，没有 composition 状态 |
| `crates/snapclip-capture/src/region.rs` | ASCII/全角数字归一化 | 不能，只是数字规则 |

覆盖层在数字面板打开期间摘除 IME，是因为现代 TSF 输入法可能忽略 `ImmSetOpenStatus(FALSE)`。这项行为应保留给“不接受文本组合输入”的数字面板，不能套用到保存面板 TextField。

### 2.3 当前缺失能力

代码中目前没有完整处理以下消息或数据：

- `WM_IME_STARTCOMPOSITION`；
- `WM_IME_COMPOSITION`；
- `WM_IME_ENDCOMPOSITION`；
- `GCS_COMPSTR`、`GCS_COMPATTR`、`GCS_CURSORPOS`；
- `GCS_RESULTSTR`；
- composition 取消、焦点切换时的清理；
- `ImmSetCompositionWindow` 和 `ImmSetCandidateWindow`；
- 预编辑字符串与正式文本的分离渲染。

## 3. Windows IME 的最小事实模型

Windows 输入法不是“按键变字符”的单一流程，而是一个短生命周期的编辑事务：

```text
普通键盘消息
    -> IME 开始组合
    -> 预编辑文本不断更新
    -> 用户选字或确认
    -> 结果文本提交
    -> 组合结束
```

保存面板需要同时维护两份文本：

```text
committed_text       已提交到字段的正式文本
composition_text     尚未提交的预编辑文本
composition_cursor   预编辑内部的 UTF-16 光标位置
selection            正式文本中的 UTF-16 选区
composition_attrs    转换中、已转换、目标字符等属性
```

关键规则：

1. `GCS_COMPSTR` 只更新临时预编辑，不直接写入正式文本；
2. `GCS_RESULTSTR` 才替换选区并提交正式文本；
3. `WM_CHAR` 不能与 `GCS_RESULTSTR` 对同一输入重复入账；
4. `WM_IME_COMPOSITION` 没有任何有效 `GCS_*` 标志时，应清除预编辑状态；
5. `WM_IME_ENDCOMPOSITION` 必须结束组合，即使没有最终字符串；
6. DirectWrite 和 IMM32 都以 UTF-16 code unit 定位，Rust 文本编辑状态必须提供 UTF-8/UTF-16 边界转换。

## 4. 可直接使用的库评估

### 4.1 Quel'Thalas：最接近“直接可用的控件”

项目地址：[github.com/wuweiran/quelthalas](https://github.com/wuweiran/quelthalas)  
Crate：[crates.io/crates/quelthalas](https://crates.io/crates/quelthalas)  
文档：[docs.rs/quelthalas](https://docs.rs/quelthalas/latest/quelthalas/)  
许可：MIT  
调研版本：0.5.0

它提供使用 Direct2D/DirectWrite 绘制的 Win32 控件，其中包括 `input` 和 `textarea`。输入控件代码已经涉及：

- 文本缓冲区；
- 光标、选区、复制、粘贴和删除；
- `WM_IME`/IMM32 组合输入；
- `ImmSetCompositionWindow`；
- DirectWrite 文本布局和 caret 定位；
- DPI 缩放。

但它的关键架构限制是：控件运行在真实的 child HWND 上。SnapClip 当前保存面板计划是一个单窗口自绘层，因此使用 Quel'Thalas 会引入：

- child HWND 的创建、销毁和层级管理；
- D2D target 与 child HWND 的边界同步；
- 焦点、鼠标命中和 UIA 的跨层协调；
- 与 demo 的视觉 token、尺寸和动画重新对齐。

**结论**：可以作为独立原型或 fallback，用来快速验证 IME 行为；不能把它当成现有自绘 TextField 的无缝依赖。

### 4.2 winit：可复用 IME 适配代码，但不是控件

源码：[winit-win32/src/ime.rs](https://github.com/rust-windowing/winit/blob/f5e20bd7/winit-win32/src/ime.rs)

可参考：

- `ImmGetCompositionStringW` 的 UTF-16 读取；
- `GCS_COMPSTR`、`GCS_RESULTSTR`、`GCS_CURSORPOS`；
- `GCS_COMPATTR` 到目标转换区间的解析；
- `ImmSetCompositionWindow`；
- `ImmSetCandidateWindow`；
- DPI 下的候选窗定位。

它不提供文本编辑器状态、DirectWrite 绘制、选区和撤销，所以应当视为平台适配参考。

### 4.3 slate-platform：更完整的 IMM32 消息分层参考

源码：[slate-platform/win/ime.rs](https://docs.rs/crate/slate-platform/latest/source/src/win/ime.rs) 及 [message_loop/ime.rs](https://docs.rs/crate/slate-platform/latest/source/src/win/message_loop/ime.rs)

可参考：

- `ImmGetContext`/`ImmReleaseContext` 的 RAII 封装；
- 组合字符串和属性读取；
- UTF-16 光标到 UTF-8 字节偏移的转换；
- `WM_IME_STARTCOMPOSITION`、`WM_IME_COMPOSITION`、`WM_IME_ENDCOMPOSITION` 的消息分层；
- 将 IME 事件转换为上层编辑事件。

它的实现属于自身窗口框架，不是可独立安装后直接挂到 SnapClip 的控件。

### 4.4 awita：有 IME 数据模型，但不是现成保存面板控件

源码：[docs.rs/awita/src/awita/ime.rs](https://docs.rs/awita/latest/src/awita/ime.rs.html)

它提供 composition 属性、候选列表和 IMM32 上下文的封装，适合借鉴数据结构和 API 分层。它仍然要求调用方拥有自己的窗口消息循环、文本编辑状态和绘制层。

### 4.5 ib-ime：只适合开关输入法

Crate：[crates.io/crates/ib-ime](https://crates.io/crates/ib-ime)  
文档：[docs.rs/ib-ime](https://docs.rs/ib-ime/latest/ib_ime/)

它主要提供：

- 设置 IME 开关；
- 设置输入法转换模式；
- 为窗口或编辑控件挂接默认关闭 IME 的 hook。

它不能读取 composition、管理选区或绘制预编辑文本，因此不能解决保存面板 TextField 的需求。

### 4.6 imekit：不是应用文本框的 IME 适配器

Crate：[crates.io/crates/imekit](https://crates.io/crates/imekit)  
仓库：[github.com/SergioRibera/imekit](https://github.com/SergioRibera/imekit)

它的定位是跨平台输入法协议/输入法引擎集成，包含 TSF、Wayland、X11 和 IMK 方向的能力。它不是一个可以接收 SnapClip `HWND` 消息并直接驱动 DirectWrite TextField 的小型适配层，当前不适合作为保存面板依赖。

### 4.7 Bevy IME Text Field：功能完整，但框架绑定过重

项目：[bevy_bc_ime_text_field](https://github.com/SheepBC/bevy_bc_ime_text_field)  
Crate：[crates.io/crates/bevy_bc_ime_text_field](https://crates.io/crates/bevy_bc_ime_text_field)

它提供中文、日文、韩文输入、选区、撤销、剪贴板等功能，但绑定 Bevy ECS 和 Bevy UI/2D 渲染，不能直接用于 SnapClip。

## 5. 官方资料

- [WM_IME_COMPOSITION](https://learn.microsoft.com/en-us/windows/win32/intl/wm-ime-composition)：消息语义和 `lParam` 标志。
- [Processing the WM_IME_COMPOSITION Message](https://learn.microsoft.com/en-us/windows/win32/intl/processing-the-wm-ime-composition-message)：读取 `GCS_RESULTSTR` 的官方示例。
- [ImmGetCompositionStringW](https://learn.microsoft.com/en-us/windows/win32/api/imm/nf-imm-immgetcompositionstringw)：读取 UTF-16 composition 数据。
- [ImmSetCompositionWindow](https://learn.microsoft.com/en-us/windows/win32/api/immdev/nf-immdev-immsetcompositionwindow)：设置预编辑窗口位置。
- [IME Messages](https://learn.microsoft.com/en-us/windows/win32/intl/ime-messages)：IME 消息分发和默认窗口处理规则。
- [Using an Input Method Editor in a Game](https://learn.microsoft.com/en-us/windows/win32/dxtecharts/using-an-input-method-editor-in-a-game)：自绘场景下处理预编辑和候选窗口的完整背景说明。

## 6. SnapClip 的目标设计

### 6.1 分层

```text
Win32 window procedure
    -> ImeController              IMM32 读取与坐标定位
    -> TextEditState               纯 Rust 文本、选区、组合状态
    -> DirectWrite TextLayout      排版、hit-test、caret 坐标
    -> D2D renderer                字段、选区、预编辑文本和光标
```

`ImeController` 不应知道文件名校验、路径校验或保存业务。`TextEditState` 不应调用 Win32。这样可以在没有 IME 的测试环境中验证绝大多数编辑规则。

### 6.2 最小接口方向

```text
TextEditState
  insert_committed(text)
  begin_composition()
  update_composition(text, cursor_utf16, attrs)
  commit_composition(text)
  cancel_composition()
  move_caret(...)
  replace_selection(...)

ImeController
  on_start(hwnd)
  on_composition(hwnd, lparam)
  on_end(hwnd)
  update_caret_rect(hwnd, rect_client_px)
```

DirectWrite 每次布局更新后，使用 `HitTestTextPosition` 得到 caret 的 client 坐标，并更新：

- `COMPOSITIONFORM`：组合字符串位置；
- `CANDIDATEFORM`：候选列表位置和避让矩形。

候选窗坐标必须使用窗口 client 坐标，并乘以当前窗口 DPI；窗口移动、缩放、文本滚动和 caret 移动后都要重新定位。

### 6.3 现有 IME 禁用逻辑的边界

| 场景 | IME context |
| --- | --- |
| 截图覆盖层普通快捷键 | 可按现有策略禁用/摘除 |
| 数字设置面板 | 可摘除，防止数字被 IME 拦截 |
| 保存面板文件名 | 必须保留 |
| 保存面板路径 | 必须保留 |
| Toast 重命名输入框 | 必须保留 |
| TextField 失去焦点或面板关闭 | 结束或取消 composition，并清理候选状态 |

## 7. 方案比较

| 方案 | IME 成熟度 | 视觉还原 | 集成成本 | 架构风险 | 结论 |
| --- | --- | --- | --- | --- | --- |
| Quel'Thalas child HWND | 高于从零实现 | 需重新适配 | 中 | 引入子窗口体系 | 原型/fallback |
| 自绘 DirectWrite + IMM32 适配 | 需要实现 | 最高 | 中 | 可控 | **推荐** |
| imekit | 不确定且偏 TSF/协议 | 不解决绘制 | 高 | 过度引入 | 不采用 |
| ib-ime | 仅开关控制 | 不解决绘制 | 低 | 能力不足 | 不采用 |
| Bevy 控件 | 功能完整 | 框架不匹配 | 极高 | 引入 Bevy | 不采用 |
| 完整 TSF 自研 | 理论上最完整 | 最高 | 极高 | 实现面过大 | 暂不采用 |

## 8. 测试与验收

### 8.1 纯状态测试

在 `TextEditState` 层测试：

- 普通 ASCII、中文和 emoji 插入；
- UTF-8 与 UTF-16 光标边界；
- 选区替换；
- `GCS_COMPSTR` 多次更新只改变预编辑；
- `GCS_RESULTSTR` 只提交一次；
- 空 composition；
- Esc/焦点切换取消 composition；
- 组合提交不产生重复字符；
- 删除、撤销、重做和剪贴板操作不破坏 composition。

### 8.2 Windows 手工验收

至少使用：

- 微软拼音；
- 日文 Microsoft IME；
- 韩文 Microsoft IME。

检查：

- 预编辑文本显示在 caret 位置；
- 候选窗跟随 caret；
- 输入法选字后只提交一次；
- Esc 取消不留下半成品；
- 点击另一个字段不会把旧组合提交到新字段；
- 窗口移动、DPI 125%/150%、多屏时候选窗位置正确；
- 保存面板关闭后 IME context 和焦点恢复；
- 数字设置面板仍不会被输入法劫持。

### 8.3 Quel'Thalas 原型验收

如果先评估 Quel'Thalas，必须先验证：

1. child HWND 是否能在截图浮层上正确保持 z-order；
2. 面板关闭和重新打开时焦点是否稳定；
3. demo 的 920 × 569 DIP 布局是否能达到要求；
4. 文件名模板、错误描边、选区和 IME 预编辑是否能使用同一套视觉 token；
5. UIA 树是否符合保存面板的控件顺序。

## 9. 最终建议

不要因为“有现成 IME crate”就引入一个能力边界不匹配的框架。对 SnapClip 来说，真正不可替代的是保存面板的单窗口层级、D2D 渲染和 demo 的视觉一致性；IMM32 只是一个有限的平台输入适配接口。

因此建议：

1. 以 `TextEditState` 为先，建立纯状态测试；
2. 从 winit/slate-platform/awita 提取 IMM32 的必要实现思路；
3. 在 SnapClip 内实现小型 `ImeController`，不引入 imekit、ib-ime 或 Bevy；
4. 用 Quel'Thalas 做独立 child HWND 原型，只有在自绘实现无法按期满足 IME 验收时才考虑采用；
5. 不实现完整 TSF，除非真实测试证明目标输入法在 IMM32 路径下无法满足需求。

