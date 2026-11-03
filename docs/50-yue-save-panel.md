# 50 · 用 Yue 重写 GPUI 保存面板

日期：2026-11-02。范围：普通选区的 GPUI 保存面板与独立开发预览。

后续HDR资源、保留HDR／转换为SDR和编码结果预览的待实施改造见 [53 · HDR捕获与消费接入](53-hdr-capture.md)，不将本文既有SDR验收视作HDR验证。

依据 `docs/design/save-panel-demo-v3.html` 保留任务、信息层级与左右布局，按 GPUI 控件尺寸、滚动、焦点和真实编码能力作调整。此次删除旧 `gpui_probe.rs`，将 `gpui_save.rs` 的有效业务适配与回归测试迁入 `save_panel/source.rs`；没有同名转发模块或旧探针 API。

## 1. 先判断组件缺口

| 原型区域 | 本次实现 | 是否需要先新增通用组件 |
| --- | --- | --- |
| 路径、文件名、尺寸、体积输入 | YueInput + 宿主校验和尺寸模型 | 否 |
| 保存、取消、复制、目录命令 | YueButton，Tabler 图标 | 否 |
| 命名变量菜单 | YueDropdownMenu | 否 |
| 文件格式 | YueSelect，稳定格式 key | 否 |
| 预览模式、单位、JPEG 质量 | YueSegmented | 否 |
| 目录快捷入口、比例预设、保存后动作 | YueTag 的 action/checkable 形态 | 否 |
| 体积开关 | YueSwitch | 否 |
| JPEG 填充色 | YueSwatchGroup + YueColorPickerState | 否 |
| 编码说明浮层 | Base Popover 负责开关、定位、Escape 和焦点，宿主提供 Yue 表面样式 | 不为一个说明浮层新增通用 Popover |
| 图片、缩放平移、原图/编码后比较 | YueImagePreview + 保留的 YueImagePreviewState | 已接入现有组件；宿主只负责准备图像及导出 |
| 富通知、内联重命名、OLE 拖出、PDF/长图分片 | 未纳入普通选区面板重写 | 不用原型的模拟按钮替代真实业务 |

2026-11-03 已接入 Yue 仓库 `docs/25-image-preview.md` 对应的现有实现，删除原私有视口的相机、手势、网格和分隔线绘制逻辑。

## 2. 原型到桌面的调整

- 默认客户区 1000 × 720 DIP，可调整窗口，最小 800 × 520；不照搬 HTML 的固定绝对坐标。
- 左侧图片得到剩余空间，右侧表单约 21.5 rem。右侧独立滚动，底部操作区始终保留在滚动区外。
- 使用 Yue 的尺寸、语义颜色、动效和字体配置。本消费项目关闭内置字体 feature，使用 Yue 的系统字体策略。
- 原图、对比和编码后三种模式共享一个视角。滚轮围绕指针缩放，拖动平移；对比模式可拖动分隔线；提供适应、100%和像素网格。
- JPEG 使用现有编码器的 92 / 75 / 50 三档；PNG、WebP 为无损，明确显示“不支持质量调整”。高级入口只解释真实编码方式，没有伪参数。
- 超过体积上限时须先“自动缩小”，尺寸字段同步后再保存；不在用户看不到的情况下导出另一组尺寸。
- 保存成功后保留面板，显示文件名、大小和“查看文件”；失败保留输入与可重试状态。
- Enter / Ctrl+Enter 保存，Escape 关闭最上层浮层或空闲面板，Ctrl+Shift+C 复制图片。保存中的关闭与重复操作被拒绝。
- 仅此面板的字段上下文将 Tab / Shift+Tab 定义为焦点前进/后退。YueInput/Base Input 的通用 Tab 缩进合同未修改。
- 输入法有预编辑文本时不响应保存/关闭；无头测试覆盖组合区存在时 Enter 不落盘，不能代替真实 Windows IME 测试。

## 3. 模块和状态

```text
apps/snapclip/src/save_panel/
  mod.rs       SavePanel、订阅、窗口生命周期、后台任务与不可变导出快照
  directories.rs  路径预设、校验去重、原子持久化和应用级记录
  model.rs     源尺寸、输出像素、单位、当前锁定比例与校验
  view.rs      Yue 控件组合、表单滚动、状态和底栏
  preview.rs   内存图像准备、版本化结果提交及 YueImagePreview 组合
  source.rs    原保存适配与编码/落盘回归测试
  tests.rs     新面板行为与布局测试
```

输出像素是唯一尺寸真值。单位切换只改变呈现；比例预设与重置使用真实截图尺寸；解锁后重新锁定采用当前比例。非法草稿保留在输入框中，不能悄悄导出源尺寸或上一次有效尺寸。

打开面板时，独立后台任务立即从已有 BGRA 准备原图，不等待导出防抖、编码或解码。尺寸、格式、质量或填充变化后，180 ms 防抖再在另一条后台任务链执行真实编码；单调 revision 拒绝旧结果。路径与文件名的编辑不会重新编码；非法尺寸或取消导出任务不会取消原图。视图 render 不编码、不写盘、不测量文件体积。

预览字节与保存字节使用同一缩放、填充、PNG master、转码实现。`assemble::encoded_bytes` 提供预览内容，`weigh` 读取其长度；正式 PNG 保存仍保留现有按带写盘路径。保存继续走现有验证、模板展开、同名策略和 `.partial` 原子提交。

这里是**同一编码规则，不是复用同一个编码产物**：预览与正式保存仍各编码一次。`docs/42` §3 中规划的 PendingCapture / 文件型 EncodedArtifact / 长图有界 tile 解码尚未由本次实现，不能将此面板称为该长图架构已完成。

宿主持有一份 `Arc<Picture>`，任务克隆资源引用。原图通过Yue公共BGRA入口准备，编码结果通过公共静态解码入口准备；二者采用相同的8192单边／64MiB栅格双预算。预算内保留完整分辨率，超预算从借用源生成有界代理，不先复制整张原图；实际atlas开销、宿主原图及旧结果另计。已移除4096阈值与固定2048结果纹理；当前合同详见§11.1，保存算法不变。

当前普通选区编辑器使用整帧编码/解码，明确拒绝超过 6400 万像素或单边 32768 px 的输出草稿，避免无界分配；格式自身的更严限制同样生效。这是面板预算，不是 PNG 等格式的能力上限。长图的独立流式导出路径没有改动，也没有被转换成整图送入此面板。

捕获侧到 UI 的打开请求通道容量为 1。新截图先关闭旧的空闲 GPUI 面板再创建；已经在提交保存的窗口保持存活，完成序号记忆和保存后动作。关闭过的窗口句柄不会被重新当作活动面板使用。

## 4. 消费依赖

`apps/snapclip/Cargo.toml` 通过相邻目录 path 引入 Yue，继续保留 GPUI Kit 的 styled 层供历史/设置页面使用。整个图只有一份 GPUI、Base 和 Kit。

消费工作区根 `Cargo.toml` 必须包含：

```toml
[patch.crates-io]
gpui-base = { path = "../GPUI-Yue-Component/vendor/gpui-base" }
gpui-pre = { path = "../GPUI-Yue-Component/vendor/gpui-pre" }
gpui-pre-windows = { path = "../GPUI-Yue-Component/vendor/gpui-pre-windows" }
```

依赖库工作区中的 patch 不会自动传播给消费方。未修改 Cargo registry。

本轮实际接入发现：之前 Base 的 `on_open_change` 被改成返回 bool，官方 Kit 的 Select/Combobox 通知回调无法编译。现拆清两个合同：

- `on_open_change`：无返回值通知，接受请求状态，保持官方 Kit 的调用合同。
- `on_open_request`：返回实际 open 状态，供 Yue 拒绝请求和约束焦点转移。

初始化顺序是 Kit init → Yue init → 创建窗口 → 一个 Root。Base 全局主题仍只有一份，不能把同一应用的窗口伪装成互不相关的全局主题。

## 5. 如何运行

普通启动的选区保存入口直接使用 Yue 面板，不再需要 `SNAPCLIP_GPUI_PANEL`。旧 `SNAPCLIP_GPUI_PROBE` 已删除。

独立预览（PowerShell，在 SnapClip 根）：

```powershell
$env:SNAPCLIP_YUE_PREVIEW = '1'
cargo run -p snapclip-app --locked
```

默认图片是开发测试图；可指定真实文件作为预览源：

```powershell
$env:SNAPCLIP_YUE_IMAGE = 'D:\images\sample.png'
cargo run -p snapclip-app --locked
```

测试后回到普通启动：

```powershell
Remove-Item Env:SNAPCLIP_YUE_PREVIEW -ErrorAction SilentlyContinue
Remove-Item Env:SNAPCLIP_YUE_IMAGE -ErrorAction SilentlyContinue
cargo dev
```

历史 Win32/D2D 面板及其专用开发入口仍存在；本轮删除的是旧 GPUI 探针，而非捕获、编码、剪贴板和整个 Win32 实现。

## 6. 验证与边界

改造前真实基线：`cargo test -p snapclip-app --lib --locked` 为 330 通过、9 ignored；原设置/历史 UI 目标为 9 通过、1 失败。失败项为 `the_unit_suffix_switch_writes_to_disk`，`label-unit-suffix` 不可见。

旧探针中固定尺寸、文案常量与棋盘格细分实现测试随旧实现删除；原有效编码/保存测试迁入 source.rs。旧 Win32 替换槽辅助函数已无调用方，与其专属测试一起删除，新增实际 GPUI 窗口替换测试。没有为了凑通过数保留无使用者的兼容壳。

新测试覆盖实际 Yue 控件的尺寸输入/联动、预设与单位、比例重锁、格式选择与质量区域、弹层 Escape、防父级误提交、表单 Tab、预编辑 Enter、防重复保存、真实落盘尺寸、I/O 失败重试、快速参数更新、三个窗口尺寸下的双栏/底栏关系，以及三种格式实际编码/透明填充。它们不证明屏幕像素与原型相同。

运行命令：

```powershell
cargo test -p snapclip-app --lib --locked
cargo test -p snapclip-app --lib --features test-support --locked
cargo test -p snapclip-app --lib --features test-support save_panel --locked
cargo test -p snapclip-app --features test-support --test ui --locked
cargo build -p snapclip-app --locked
```

本轮最终结果：

| 验证项 | 结果 |
| --- | --- |
| SnapClip 普通 lib 测试 | 316 通过，9 ignored，0 失败 |
| SnapClip lib + `test-support` | 334 通过，9 ignored，0 失败（含下述颜色、菜单布局、首图加载、路径预设回归） |
| 新保存面板（含真实控件的无头交互） | 40 通过 |
| 原设置/历史 UI 集成目标 | 9 通过，1 失败；与改造前相同，仍为 `the_unit_suffix_switch_writes_to_disk` 的 `label-unit-suffix` 不可见 |
| 开发可执行文件 | `cargo build -p snapclip-app --locked` 通过；默认 `target/debug/snapclip-app.exe` 与 `snapclip-app-preview.exe` 均已更新为本轮链接产物，SHA-256 一致 |
| Base Select / Combobox 回归 | 分别 15 / 2 通过 |
| Yue 组件回归 | 组件完整 lib 无头测试 321 通过，Tag 集成测试 17 通过；其中新增 Select 右边缘定位回归 |
| 新面板及本轮 Base/Select 文件 `rustfmt --check` | 通过（Rust 2024） |
| 两仓库本轮改动 `git diff --check` | 通过 |
| 严格 Clippy | 全量未通过：现有 history/ui 等依赖与 app 其它模块仍有诊断；`--no-deps --all-targets --features test-support` 的 JSON 输出中，新 `save_panel` 模块诊断为 0。此结果不等于全仓 Clippy 通过 |

普通 lib 用例总数变化来自旧探针和无调用方替换辅助函数的实现专属测试被删除，以及有效业务测试迁移、新行为测试加入；不是对失败断言作忽略或弱化。原设置页失败不会被记为新增面板测试通过。

关键行为对比：

| 项目 | 修改前 | 修改后 |
| --- | --- | --- |
| 普通选区保存入口 | 默认 Win32，旧 GPUI 面板需环境变量启用 | 直接打开 Yue 保存面板 |
| 布局 | 旧 GPUI 固定尺寸、不可调整窗口 | 可调整双栏，表单独立滚动，底栏固定；三种窗口尺寸已作无头几何检查 |
| 尺寸编辑 | 旧探针包含示例尺寸假设 | 以真实截图为基准；单位不改变像素真值，非法草稿阻止导出 |
| 预览 | 旧探针绘制路径 | 原图立即独立准备；编码后预览在后台防抖、真实编码并解码，迟到结果丢弃；三种模式共用视角 |
| 编码与落盘 | 现有保存服务 | 继续复用并保留原子提交；三种格式、透明填充、失败后重试均有回归 |
| 焦点 | Base 输入框的 Tab 缩进 | 只在此面板内定义 Tab 遍历，不改变组件库合同 |

本轮未提交或推送。

实机验证未完成：Computer Use 的 Windows native pipe 不可用，重试、重置并重新初始化后仍失败。不得据无头结果宣称已通过 Windows DPI、字体渲染、真实鼠标拖拽、读屏或 IME 候选窗口验收。

## 7. 实机截图发现的颜色转换错误

用户截图中出现整片青色背景。根因在宿主的颜色适配：`YueTheme::page()`、`surface()`、`text_primary()` 等语义便捷方法返回 `0xRRGGBB`，旧面板却传给了 GPUI 的 `rgba()`，后者要求 `0xRRGGBBAA`。例如白色 `0xFFFFFF` 会被当成 `0x00FFFFFF`，得到青色；其它颜色同时发生通道和透明度错位。

已逐处修正 `save_panel/view.rs` 与 `preview.rs`：语义 RGB 值使用 `rgb()`；错误、警告直接使用 token 的 `.rgba()` 解析结果。图片填充色等原本就是八位 RGBA 的值保留正确格式。

新增 `theme_colors_reach_the_painted_panel_and_preview_without_channel_shifts`，在修改前真实复现青色绘制；修改后在浅色、深色主题中核对实际提交的面板背景、预览底色、边框和 YueInput 底色。无头文字系统不栅格化普通字形，此测试不声称验证了文字像素或实际屏幕对比度。保存面板 26 项、完整 feature lib 320 项均通过（另 9 ignored），开发可执行文件已重建；运行中的旧进程需要重新启动。

## 8. 文件格式菜单的默认左对齐

格式菜单没有主动设置居中，但 YueSelect 的普通选项行始终保留左侧图标空位，使不含图标的短格式名在窄面板内显得居中。组件现按当前结果集决定是否保留图标列：纯文本列表直接从正常左内边距开始，混排图标时所有普通选项仍保持统一文字列；右侧选中标记保留。选择框与弹层同时明确采用左对齐。

组件新增失败后修复的几何回归，覆盖三尺寸、普通/虚拟列表和有/无图标；SnapClip 新增实际格式菜单的左侧坐标及键盘选择回归。最新结果见第 6 节，Yue `--all-targets --features headless-tests --no-deps` 严格 Clippy 通过。

## 9. 原图晚于面板出现

旧流程把首图发布放在导出流程末尾：打开窗口 → 等待 180 ms → 完整尺寸编码 → 解码编码结果 → 生成编码后纹理 → 准备原图 → 一起更新。内存中已有 BGRA 的原图因此被与它无关的压缩与解码阻塞；超过 2048 px 的截图还要先执行一次 CPU 面积重采样。

现将两项工作拆开：原图加载不防抖，只依赖不可变截图；编码后预览与文件大小继续使用现有防抖和 revision。普通截图及 4K 截图不再为首图作 CPU 缩略，保持原始 BGRA/alpha，绘制时按视口缩放。没有新增缓存、编码器、依赖或保存质量分支。

同机 debug 构建的临时分阶段测量如下，临时计时代码已删除：

| 输入 | 修改前原图数据准备 | 修改后原图数据准备 |
| --- | --- | --- |
| 真实 1700 × 1373 截图，PNG 232890 字节 | 1119–1137 ms，且此前还要等 180 ms 防抖 | 约 1.3 ms；编码独立继续，PNG 字节数相同 |
| 合成 3840 × 2160 BGRA 截图 | 约 1049–1056 ms，仅计算缩略处理 | 约 4 ms，保留 3840 × 2160 原像素 |

这是从已有 BGRA 到 `RenderImage` 可发布的 **CPU 阶段**计时，第一行修改前包含串行编码/解码等待，第二行隔离了原图缩略热点；两行不是同一套工作负载。计时不包括窗口创建、GPU 上传或屏幕呈现，不能据此声称首帧在 1–4 ms 内显示。该批采用的4096／2048策略已由§11.1公共双预算替代；大图仍有后台准备成本，未承诺所有尺寸都立即就绪。

新增 5 项永久回归：①原图不等待 180 ms，且非法尺寸不会替换已加载原图；②在导出启动前取消它仍能得到原图；③BGRA/alpha 与大图纹理边界；④4K 原像素直通；⑤非法像素缓冲拒绝。首图时序及 4K 路径用例均先观察到失败，再实施修复。现有 PNG/JPEG/WebP 预览与落盘字节/尺寸/透明填充测试继续通过。具体命令与测试总数见第 6 节；实机边界仍按该节说明。

本轮链接完成后，默认 exe 被运行中的旧进程锁定，Cargo 最后复制步骤返回访问拒绝；没有强制结束用户进程。可退出当前 SnapClip 后运行 `target/debug/snapclip-app-preview.exe` 验证此次修复。该文件来自 `target/debug/deps/snapclip_app.exe`，复制后已核对 SHA-256 一致；并非上轮旧构建。此次仍未完成真实屏幕首帧测量。

## 10. 截图中的弹层、焦点环、Tag 与路径预设

依据 2026-11-02 20:06 / 20:08 的实机截图修正以下内容：

1. 格式菜单靠右时被向左推。原因是 Select 把扩散阴影也当成必须完整装入窗口的内容；现只为面板表面和 margin 作碰撞避让，保留原有上下翻转、进出动画和阴影绘制，允许边缘阴影裁切。同时将 Anchor 和宽度测量移到完整 border box，避免 MatchTrigger 比字段窄两个边框宽度。组件新增右边缘几何回归，先失败后修复。
2. 蓝色焦点环右侧被表单滚动 clip 裁掉，字段本身没有越出窗口。滚动所有者仍为 `save-form`，内层内容按 `--input-focus-ring-width` + `--input-focus-ring-offset` 留白。无头测试检查实际绘制的四边及每一边的 content mask；GPUI 的圆角边框会拆成多个绘制片段，不能只拿第一个片段判断整环。测试在没有留白时真实复现右侧裁切。
3. YueTag 的 `self_start` 用来在列布局里保持内容宽度，却会覆盖水平行的 `items_center`。保存后的 Tag 现在组成独立内容行，再由外层行与“保存后”标签共同居中；路径预设也用自然高度槽位与“＋”按钮居中。没有给 Tag 施加随意 top 偏移，也没有改变通用 Tag 的默认列布局合同。
4. 此面板的所有 Tag 均明确使用 `TagShape::Pill`，包括路径、覆盖同名、保存后动作、尺寸预设和预览开关。新增实际圆角几何检查。
5. 路径预设末尾增加带无障碍名称和 tooltip 的“＋”。先通过路径字段或“更改”选定目录，再点击“＋”把当前目录加入预设；新 Tag 以目录名显示，长名字省略，完整路径作为无障碍描述。点击预设会切换保存路径。不存在的目录、文件和相对路径拒绝添加；内置路径与自定义路径按 Windows 解析后的路径去重。路径行可换行。

自定义路径存入 `<app data>/save-panel-paths.json`，与设置页的文件分开，避免它们互相覆盖。目录验证、路径解析和写盘在后台执行，临时文件写完后原子替换；写盘成功后才发布新 Tag。应用级记录串行化新增请求，视图订阅记录变化；保存任务不会因为发起面板关闭而取消，失败保留现有预设并反馈错误。测试使用独立目录或内存记录，不写真实用户预设。

本节新增 8 项 SnapClip 回归（含关闭面板期间保存、读回持久化、重复/非法路径和写盘失败）、1 项 Yue Select 回归；最新计数见第 6 节。Computer Use 仍报告 native pipe 不可用，真实窗口截图验收尚未完成；无头几何、交互和绘制检查不等同于实机验收。


## 11. 接入现有 YueImagePreview（2026-11-03）

保存面板现在持有一个 `Entity<YueImagePreviewState>`，由现有 `YueImagePreview` 提供原图/对比/编码后三种模式、缩放、100%、适应窗口/宽度、拖动平移、对比分隔线、背景选择、像素网格及作用域快捷键。原 `PreviewMode`、zoom/pan/split、手写 checker canvas 和重复工具条已删除。

消费根已补齐 `gpui-pre-windows` 本地patch并更新Cargo.lock；三个补丁分别为Base、GPUI core、Windows后端。仅core与Base不足以提供组件工具条的真实局部磨砂和像素网格的最近邻采样，必须使用与Yue工作区相同的Windows后端。依赖树已核对指向相邻Yue仓库，不修改Cargo registry。

应用仍负责独立原图准备、180ms 防抖后的真实编码、导出字节计数、保存与复制；`RenderImage` 使用共享 Arc 提交。每次原图/结果状态改变，在事件或异步完成回调中提交同一文档 ID 及单调 revision，不在 render 中重建状态或启动任务。旧编码任务不能覆盖新版本。刷新期间组件可显示带“上一版”标记的旧结果；非法尺寸草稿会明确清除结果，保留原图并阻止保存。源图与结果的失败通过组件错误态和重试事件接回应用任务。

对比只用于同一完整画面且宽高比对齐的两张图；解除比例锁并改变宽高比后，对比不可用，原图和编码后仍可单独查看。不会把不同宽高比的图强行拉伸。双击图像改为适应窗口。预览快捷键只在图像焦点内工作，输入框中的 `1` 等字符继续作为输入。

原图和编码结果现统一走Yue公共资源准备，移除私有通道转换和固定2048结果纹理。组件的100%按方向归一化后的**原始尺寸**映射物理屏幕像素，代理纹理不改变倍率语义；页脚分别标明两侧的可用纹理和细节边界。实际导出尺寸和字节不随预览纹理缩小，4K原图快速路径与PNG/JPEG/WebP落盘回归继续保留。

### 行为与验证

| 项目 | 接入前 | 接入后 |
| --- | --- | --- |
| 相机和手势 | 面板私有逻辑 | YueImagePreviewState统一拥有 |
| 原图首次准备 | 不等待编码 | 保留，并以组件Loading/Ready/Failed呈现 |
| 对比 | 私有canvas直接覆盖 | 组件检查完整画面和宽高比，真实分隔线交互 |
| 失败重试 | 面板状态文字 | 组件错误态/重试按钮，宿主执行真实任务 |
| 100% | 私有逻辑像素比例 | 按原始尺寸的物理像素比例；代理显式报告细节边界 |
| 保存与复制 | 应用真实编码/写盘/系统复制 | 保留业务所有权，无组件内伪操作 |

接入前保存面板相关测试40项通过；先补组件挂载断言并实际观察旧代码失败，日志为 `target/save-image-preview-before.log`。随后用真实组件UI测试替代已删除的私有相机算式测试，并新增错误重试及综合交互回归，当前相关测试41项通过。

最终消费方回归：

```powershell
cargo test -p snapclip-app -p snapclip-pin --features snapclip-app/test-support,snapclip-pin/headless-tests --lib --locked
cargo clippy -p snapclip-pin --features headless-tests --all-targets --no-deps --locked -- -D warnings
```

`target/image-preview-final-tests.log`：app **283 passed / 5 ignored**，pin **70 passed / 4 ignored**；pin严格Clippy通过。Yue图片预览专项33项通过（`target/snapclip-image-preview-regression.log`，位于Yue仓库）。应用包的全量严格Clippy仍被旧捕获、历史记录和原生保存窗口中的既有告警阻断，见 `target/image-preview-clippy.log`，不据此声称整个应用lint干净。完整workspace上轮已记录的两个集成失败见docs/52 §11.4，本次未改其断言。

没有取得本轮真实窗口截图或实机交互验收证据；无头的交互、几何和绘制检查不能代替Windows DPI、字体、显卡与读屏检查。开发预览入口仍使用§5的环境变量。

### 11.1 公共资源与采样升级（2026-11-03）

- 原图借用Picture的连续BGRA，通过`ImagePreviewImage::from_bgra8`后台准备；不等待180ms导出防抖或编码结果。编码结果使用实际导出的字节和`from_encoded`，体积继续统计同一文件。
- 两侧默认纹理限制为单边8192、栅格64MiB；超出时明确使用代理。原始像素、原始尺寸和保存／复制对象保持不变。默认静态解码还限制64MiB编码、6400万像素、32768原始单边、512MiB解码分配；限制并不等于进程或显存峰值。
- 同文档代理升级完整图时保留手动倍率和中心。Compare任意一侧为代理时禁用最近邻和原始像素网格；完整图可选平滑／像素显示，静止100%与整数倍率对齐物理像素。保持现有版本取消／过期保护。
- Windows精确采样与上传生命周期依赖相邻Yue仓库的core和Windows补丁；不修改Cargo registry，不另建图片缓存。EXIF方向在解码时应用，未转换ICC／非sRGB色彩明确标记，未提供完整色彩管理或超长图分块细节。
- 新回归验证长图代理保留原始尺寸，以及3072px编码结果保留完整分辨率且纹理像素逐字节来自实际导出。最终命令、统计和真实UI边界见Yue的`docs/25-image-preview.md` §17.9；CPU测量见其`docs/performance/2026-11-03-image-preview-resources.md`。

本批最终`target/image-preview-resources-final-tests.log`：app **285 passed / 5 ignored**、pin **71 passed / 4 ignored**；pin all-targets严格Clippy与两包all-targets check通过，见`image-preview-resources-pin-clippy.log`及`image-preview-resources-check.log`。应用及pin_preview示例均重新构建，通过`-C extra-filename=-image-resources`生成本批链接产物，日志为`image-preview-resources-app-build.log`和`image-preview-resources-pin-build.log`。未启动实窗，未提交或推送。
