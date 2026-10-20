# 48 · 自绘 TextField 的 IME 难点与 S5 测试清单

> 状态：**设计记录，未实现** ✗。配套：`docs/42` D11（自绘 DirectWrite TextField）、`docs/41` S5、`docs/47`（`snapclip-ui` 审查）、`docs/46 §8.1`（测试宿主空白）。
> 本文只写 IME。光标/选区/撤销/粘贴等编辑模型能力见 `crates/snapclip-ui/src/panel/text_field.rs`。

---

## 0. 结论摘要

1. **今天的截图面板根本走不到"组合态"** ✗ —— 全仓 `WM_IME_*` **零处**；字符只有一条路进来：`WM_CHAR` → `on_char` → `panel_key_from_char` → `on_region_panel_key`（`windows/overlay/input.rs:1110-1119`）。它把**字符映射成面板按键**，面板里**没有文本缓冲** ✗。
2. **面板打开时会主动摘掉 IME context** ✗ —— `input.rs:951-956` 里**只要面板存在就摘**（`if self.region_panel.is_some() { self.detach_ime_for_panel() }`，摘除本身在 `window_restore.rs:111-116`）。原因写在 `:108-112`：`ImmSetOpenStatus(himc,0)` 被**现代 TSF 输入法（微软拼音等）忽略，数字仍被吃掉**（`docs/23 §14.55`）。这条是**既有经验**，也是 S5 最大的冲突点：文件名要中文，而面板现在刻意没有 IME。
3. **`snapclip-ui` 没有开 IME feature** ✗ —— `crates/snapclip-ui/Cargo.toml:24` 只有 `Win32_UI_Input_KeyboardAndMouse`；而 `crates/snapclip-capture/Cargo.toml:64` 与 `:80` **都开了** `Win32_UI_Input_Ime` ✓ ⇒ "门开了，但控件所在的 crate 进不来" ✗。
4. 因此 S5 的第一步不是"写组合渲染"✗，而是三件前置（§6）：**加 feature** ✓、**决定 `detach_ime_for_panel` 的去留** ✓、**建键鼠/输入测试宿主** ✓。

---

## 1. 现状核实（`[事实]` 全部带位置）

| # | 事实 | 位置 | 含义 |
| --- | --- | --- | --- |
| 1 | capture crate 已开 `Win32_UI_Input_Ime`（两处） | `snapclip-capture/Cargo.toml:64`、`:80` | IMM32 可用 ✓ |
| 2 | **ui crate 未开** IME feature | `snapclip-ui/Cargo.toml:24` | TextField 放进 ui 后**调不到** IMM32 ✗ |
| 3 | 全仓 `WM_IME` **零处** | `grep` 结果 | 组合态完全未实现 ✗ |
| 4 | 窗口消息处理器只处理 24 条 `WM_*`，**无任何 `WM_IME_*`** | `windows/overlay/window_host.rs` 的 `handle()` | 入口位置明确 ✓ |
| 5 | 字符入口 = `on_char`（`WM_CHAR`）→ `PanelKey` | `input.rs:1110-1119` | 有"字符"，没有"文本" ✗ |
| 6 | 字符只能走一条路，**两条都收会"落两次"** | `input.rs:42` 注释；`window_host.rs:87-89` | 项目已付过"双输入"的学费 ✓ |
| 7 | 面板打开时**摘 IME context**、关闭时还回，**按面板寿命而非字段焦点** | `input.rs:951-956`（判据）/ `window_restore.rs:111-116`（摘）/ `:120-123`（还） | 与"文件名要中文"直接冲突 ✗ |
| 8 | `ImmSetOpenStatus` 被 TSF 输入法忽略、数字被吃 | `window_restore.rs:108-112`（引 `docs/23 §14.55`） | **既有真机结论** ✓ |
| 9 | overlay 窗口在**独立线程**创建 | `overlay.rs:252-254` `thread::Builder::new().spawn(…)` | `ImmGetContext` 必须在该线程调用 ✓ |
| 10 | overlay **没有键鼠测试宿主** | `docs/46 §8.1`（`docs/46:126-128`） | IME 测试的前置阻塞 ✗ |
| 11 | `TextField` 现有 API：`new / hit_test / focus_at / blur / insert / key / selected_range / caret_visible / paint` | `snapclip-ui/src/panel/text_field.rs:27`…`:179`（共 289 行） | 编辑模型有 ✓；**无** IME/剪贴板/撤销/拖选/UIA ✗ |
| 12 | 面板里**没有文件名/模板输入框** | `geometry.rs` 里 `grep TextField\|filename\|模板` 无命中 | S5 的 UI 面从零开始 ✓ |

---

## 2. 六个子系统

### 2.1 消息协议：系统给的不是字符串，是**带结构的组合态**

`WM_IME_COMPOSITION` 的 `lParam` 是位组合（`GCS_*`）：

| 位 | 内容 | 你要做的 |
| --- | --- | --- |
| `GCS_COMPSTR` | 正在组合、尚未确认的串 | 内联画进文本行 ✓ |
| `GCS_COMPATTR` | 每字符属性（可转换/已转换/候选焦点） | 画下划线与高亮分段 ✓ |
| `GCS_COMPCLAUSE` | 子句边界 | 子句级高亮 ✓ |
| `GCS_CURSORPOS` | 组合串内部光标 | 组合期光标在此 ✓ |
| `GCS_RESULTSTR` | 已确认文本 | 插入编辑模型 ✓ |

配套消息：`WM_IME_STARTCOMPOSITION` / `_COMPOSITION` / `_ENDCOMPOSITION` / `_SETCONTEXT` / `_REQUEST` / `_CHAR`；取串用 `ImmGetCompositionStringW`，并**必须在 `ImmGetContext`/`ImmReleaseContext` 之间**。

**本项目的落点**：`window_host.rs` 的 `handle()`（现状 24 条消息，`[事实] #4`）新增这六条；`input.rs` 新增一个**组合态**字段（与"面板按键"并列，不要塞进 `on_char` ✗）。

### 2.2 组合串渲染：清 `ISC_SHOWUICOMPOSITIONWINDOW`

收到 `WM_IME_SETCONTEXT` 时把 `ISC_SHOWUICOMPOSITIONWINDOW` **清掉** ✓，否则系统会用它自己的小窗口画组合串 ✗ —— 与你的排版并存，用户看到**两处文字** ✗。

清了就要自己画：组合串、下划线（按 `COMPATTR`）、子句高亮（按 `COMPCLAUSE`）、组合内光标（`CURSORPOS`）。**位图可测** ✓（复用 `d2d/tests.rs:1974 render_view` / `:28 pixel_at`）。

### 2.3 候选窗定位：你不画它，但**必须给屏幕坐标**

现代 IME 自己画候选列表 ✓，位置由你给：回 `WM_IME_REQUEST` 的 `IMR_QUERYCHARPOSITION`（新）或 `IMR_CANDIDATEPOS` / `IMR_COMPOSITIONWINDOW`（旧）。

要返回的是**插入点的屏幕坐标矩形** ✓ —— 而你在面板里算出来的是 **dip / 客户区**，必须过 DPI + 窗口位置（外层还有 `WM_DPICHANGED`，`window_host.rs` 已处理 ✓）。

**典型症状**：候选窗永远贴在屏幕左上角；或滚动/缩放后位置漂移。

### 2.4 字体度量：`LOGFONT` ↔ `IDWriteTextFormat`，再加上**内嵌子集**

- `ImmSetCompositionFont` 要的是 `LOGFONT` ✗ 不是 DWrite 的 `IDWriteTextFormat` ⇒ 必须映射字体族/字号/字重 ✓；
- 本项目字体内嵌**子集**（225 codepoints，`subfont/` + `assets/harmonyos-sans-sc-subset.ttf`）⇒ 组合串里会出现**子集外的字** ✗ ⇒ 自绘就豆腐 ✗；
- 所以要么**准备输入期字体**（系统字体，仅组合态使用），要么扩子集（不可能穷尽）✓。

`docs/47` 已确认 `text_field.rs` 不带字体资产 ✓，这条是**新增依赖** ✗。

### 2.5 编辑模型互操作：组合期**让出按键**

组合期间 `Backspace`/`Delete`/方向键/`Enter` 的语义**属于 IME**；你的 TextField 必须让出，并且：

- **一次组合 = 一个撤销单元** ✓（`text_field.rs` 目前**没有撤销** ✗，S5 补）；
- 组合期不改选区 ✗；
- `WM_IME_CHAR` 与 `WM_CHAR` **只取一条** ✓ —— `input.rs:42` 的注释说明项目已经知道"两条都收会落两次"，此处**是同一个坑的第二个入口** ✓。

### 2.6 TSF vs IMM32：项目已经有真机结论

`window_restore.rs:108-112` 的原话是这次最有用的前提：

> `ImmSetOpenStatus(himc, 0)` 只把 IME 切到"英文模式"，是 IMM 时代的兼容垫片：现代 TSF 输入法（微软拼音等）经常直接忽略它，数字仍被输入法吃掉（`docs/23 §14.55`）。

⇒ **同一批 TSF 输入法**在组合态也走兼容层 ✓：结果串能拿到（`WM_CHAR`），但**组合态细节**（候选窗、UILess、主题化）可能缺失 ✗。IMM32 能用于 v1 ✓，但要在**真机 TSF 输入法**上验收 ✓（§5 的人工项）。

---

## 3. 本项目额外的四条

1. **独立线程**（`overlay.rs:252-254`）：`ImmGetContext` / `ImmAssociateContext` **必须在该线程**调用 ✓；跨线程用会拿到空 `HIMC` ✗。
2. **面板打开时摘 context，且按面板寿命而非字段焦点**（`input.rs:951-956`）：今天的做法对"数字/模板"是对的 ✓（防止输入法吃数字），对"文件名要中文"是**错的** ✗ ⇒ 必须改成**按焦点字段**启用/摘除（见 §6 的第 2 项）。
3. **只有字符、没有文本**（`on_char` → `PanelKey`，`input.rs:1114-1117`）：`TextField` 已有 `insert/key/selected_range`，但**没有接进面板** ✗ ⇒ S5 要先把面板的"按键"路径扩成"文本字段"路径，组合态才有落点。
4. **分层窗口 + 光标自绘 + 模态层**：浮层用 `UpdateLayeredWindow`（`docs/35` 记录过的分层路径）⇒ 没有原生控件句柄 ✓；面板是模态层，**候选窗是独立顶层窗口** ⇒ 要验证点到候选窗不会丢掉模态焦点 ✓；光标由 `WM_SETCURSOR` + `SetCursor` 自管（`window_host.rs` 已处理 ✓）。

---

## 4. 十个症状与测法

| # | 症状 | 根因 | 怎么测 |
| --- | --- | --- | --- |
| 1 | 候选窗贴在屏幕角落 | 没回 `IMR_QUERYCHARPOSITION`，或回了客户区坐标 | 合成 `WM_IME_REQUEST`，断言返回矩形 = 插入点屏幕坐标 |
| 2 | **每个字输入两次** | `WM_IME_CHAR` 与 `WM_CHAR` 都处理 | 合成两次消息，断言只插入 1 个字符 |
| 3 | 组合串与系统小框**同时出现** | 没清 `ISC_SHOWUICOMPOSITIONWINDOW` | 断言 `WM_IME_SETCONTEXT` 后 lParam 该位为 0 |
| 4 | 撤销把一个汉字拆成两次 | 组合没合成一个事务 | 组合 → 撤销 → 断言回到组合前状态 |
| 5 | 组合里删字后 IME 错乱 | 组合期自己处理了 `Backspace` | 组合期发 `VK_BACK`，断言字段文本不变、动作转给 IME |
| 6 | 组合串用系统字体、与正文不符 | 没 `ImmSetCompositionFont` | 断言映射出的 `LOGFONT` 与 DWrite 面/字号一致 |
| 7 | 组合态光标不显示 | 只画了普通光标 | 位图断言：组合态光标落在 `CURSORPOS` |
| 8 | 子句高亮缺失 | 没读 `COMPCLAUSE` | 位图断言：两子句之间颜色分段 |
| 9 | 输入法取消后字段残留半截 | 没处理 `WM_IME_ENDCOMPOSITION` | 合成 START → END（无 RESULT），断言字段为空 |
| 10 | 读屏读不到未确认文本 | UIA Text Pattern 未实现 | 断言 `ITextProvider` 报告组合态区间 |

---

## 5. S5 测试清单（TDD；名字可直接引用）

> 命名规则：`<行为>_<条件>`，断言行为而非实现。
> 自动化列：**✓** = 合成消息/位图可自动；**真** = 需交互桌面（`#[ignore = "needs an interactive desktop"]`）；**人** = 只能人工验收。

### 5.1 协议层（合成消息，✓）

| 测试名 | 断言 | 先红原因 |
| --- | --- | --- |
| `chars_arrive_exactly_once_from_ime_and_keyboard` | 同时投 `WM_IME_CHAR` 与 `WM_CHAR`，字段只多 1 个字符 | 今天两条都会落（`input.rs:42` 的同族坑） |
| `composition_text_is_drawn_inline_and_not_in_a_system_window` | `WM_IME_SETCONTEXT` 后该位被清，且组合串出现在字段插入点 | 无任何 `WM_IME_*` 处理 |
| `a_composition_ends_cleanly_when_the_ime_cancels` | START → END 无 RESULT ⇒ 字段不变 | 同上 |
| `composition_is_a_single_undo_unit` | 组合 + 确认 ⇒ 一次撤销回到组合前 | 撤销尚不存在，且组合未建模 |
| `backspace_during_composition_is_left_to_the_ime` | 组合期 `VK_BACK` 不改字段 | 无组合态分支 |
| `the_candidate_window_receives_the_caret_in_screen_coordinates` | `WM_IME_REQUEST` 返回的矩形 = 插入点屏幕坐标（含 DPI/窗口偏移） | 无处理 |
| `the_composition_font_is_mapped_from_the_dwrite_face` | 映射出的 `LOGFONT` 与 DWrite 面/字号一致 | 无映射 |

### 5.2 与既有行为的冲突（✓，且是**本项目独有**）

| 测试名 | 断言 |
| --- | --- |
| `the_ime_context_follows_the_field_not_the_panel` | 焦点在文本字段 ⇒ **不摘**；焦点在数字/模板字段 ⇒ **仍摘** ✓（回归 `input.rs:951-956` 的寿命耦合 ✗） |
| `detaching_the_ime_context_is_reversible_on_field_change` | 在两类字段间来回切，`HIMC` 摘/还次数正确、不泄漏 ✓ |
| `a_chinese_filename_can_be_typed_through_the_composition` | 合成组合串 ⇒ 字段得到中文（覆盖 §0 的冲突点） |

### 5.3 视觉（位图断言，✓）

| 测试名 | 断言 |
| --- | --- |
| `a_composing_clause_is_underlined` | 组合态下划线像素出现在基线下方 |
| `two_clauses_are_drawn_with_different_highlight` | `COMPCLAUSE` 两段颜色不同 |
| `the_composition_caret_follows_the_ime_cursor_position` | 光标像素落在 `CURSORPOS` |

### 5.4 可访问性（真）

| 测试名 | 断言 | 自动化 |
| --- | --- | --- |
| `the_text_pattern_reports_an_unconfirmed_composition` | UIA `ITextProvider` 报告组合区间 | **真** |

### 5.5 只能人工（人）

| 项 | 步骤 | 判据 |
| --- | --- | --- |
| 真机候选窗位置 | 微软拼音 + 一个第三方 TSF 输入法，分别在面板底部/右侧输入 | 候选窗出现在插入点旁，不遮挡、不贴角 |
| UILess / Win11 "IME 兼容性" | 关闭兼容性选项后重复上一行 | 行为一致 |
| 组合态字体观感 | 截屏对比系统输入框与自绘字段 | 字号/基线不跳 |

> **纪律**：人工项只承认人工结论 ✓；不把"合成消息通过"当成"输入法可用" ✗。

---

## 6. 前置条件（阻塞项，按顺序）

1. **`snapclip-ui` 开 `Win32_UI_Input_Ime`** ✗（`Cargo.toml:24`）—— 否则 TextField 里写不出 `ImmGetCompositionStringW`。
2. **把摘除从"面板寿命"改成"字段焦点"** ✗ —— 今天是 `input.rs:951-956` 按面板存在与否决定；推荐改为**文本字段有焦点才开** ✓、数字/模板字段**仍摘** ✓，保留"防吃数字"的既有收益 ✓。这是 S5 的**第一个设计决定**，晚做会让后面所有测试都建在错的前提上。
3. **键鼠/输入测试宿主** ✗（`docs/46 §8.1`）—— 最小形状：真实 `hwnd` + `ImmAssociateContext` + 合成 `WM_KEYDOWN`/`WM_CHAR`/`WM_IME_*` + 断言模型与位图。**没有它，§5 的协议层全部只能靠纯函数**。
4. **面板侧有文本字段** ✗ —— 今天面板只有 `PanelKey`（`input.rs:1114-1117`）；`text_field.rs` 的能力还没接进去。

---

## 7. 成本与顺序

| 步 | 内容 | 量级 |
| --- | --- | --- |
| 0 | 前置 1–4（feature / 摘除策略 / 宿主 / 接字段） | **小到中** ✓ |
| 1 | 协议层（§5.1 七条 + §5.2 三条） | **中** |
| 2 | 视觉（§5.3 三条，位图） | 小 ✓ |
| 3 | UIA Text Pattern（§5.4） | 小到中 |
| 4 | （可选）TSF `ITextStoreACP` | **另立一项** ✗ |

顺序理由：**先定摘除策略**再写协议（否则 §5.2 的测试会白写 ✗）；**先协议再视觉**（组合串没有来源，下划线无从谈起 ✓）；UIA 放后（它只暴露已有状态 ✓）。

---

## 8. 与原生 `EDIT` 的对照

| | 原生 `EDIT` | 自绘 TextField |
| --- | --- | --- |
| IME（组合/候选/UIA） | **免费** ✓ | 七件事全做 ✗ |
| 视觉一致（D11） | 差 ✗（无法与 demo 完全一致） | **完全一致** ✓ |
| 与 D2D 外壳混排 | 需子窗口宿主 + 焦点管理 ✗ | 天然 ✓ |
| 与"摘 IME"策略 | 同样要处理 ✗ | 同样要处理 ✗ |

**折中（若将来想省钱）**：文件名/路径用**原生 `EDIT` 子窗口** ✓、数值/模板用自绘 ✓ —— 但这会把"两种输入外观"放进同一个面板 ✗，与 D11 的视觉目标冲突 ✗。**不推荐** ✗，仅记录。

---

## 9. 未验证

- 真机 `HIMC` 在面板窗口上的摘/还行为（代码读到，未在真机观察）；
- 微软拼音在当前 Windows 版本下对本浮层的候选窗定位表现（需 §5.5 人工）；
- `snapclip-ui` 加上 IME feature 后是否有 `windows` crate 版本/特性冲突（未编译验证）；
- 组合态位图断言的容差（`assert_pixel_close` 阈值，`d2d/tests.rs:64`）是否够用。
