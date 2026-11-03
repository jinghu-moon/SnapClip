# 53 · HDR 捕获、母版、保存面板与贴图接入方案

初次设计：2026-11-03；更新：2026-11-04。状态：**源像素与母版、WGC/D2D 内部高精度链路、输出能力快照、生产滚动来源合同、HDR PNG16、保存面板策略、F3/F6 共用 GPUI/Yue、SDR/HDR 复制及历史保真源已分批实现。** F6 旧 GDI 宿主已删除，Pin 保留 FP16/scRGB 原始 Arc；当前证据和边界见 §12～§17。AVIF 生产证据、资源性能与真实 HDR/双屏/DPI/设备恢复仍未验收，App/Capture 全仓 strict Clippy 尚未通过；**生产 HDR 捕获门禁保持关闭，不代表完整支持 HDR。**

通用像素、色彩、GPUI／Windows输出、依赖选择及官方资料统一见相邻Yue仓库的 [27 · Windows HDR图片与显示管线](../../GPUI-Yue-Component/docs/27-hdr-pipeline.md)。本文只定义SnapClip的业务和迁移，不复制第二套HDR框架。

## 1. 必须一起完成的范围

不是把保存预览改为FP16就完成支持。以下链路必须同时有明确合同：

```text
WGC / 原始文件 / 剪贴板编码内容
  → 带格式与色彩定义的冻结帧或原始内容
  → 裁切 / 圆角 / 滚动行带 / 原始母版
  ├→ HDR显示资源 → YueImagePreview → 保存面板 / F3 / F6
  ├→ SDR分析派生图 → OCR / 匹配 / 历史缩略图
  ├→ HDR文件 / 明确的SDR文件
  └→ 明确的SDR兼容复制 / HDR复制
```

HDR屏且系统HDR开启时优先直接显示；内容超出目标HDR能力时仅做HDR→HDR显示适配，不满足HDR输出时显示SDR派生结果，均保留原始HDR。原位F6、截图覆盖层、滚动预览也在范围内；不能让它们继续显示已经截断的BGRA8再宣称应用支持HDR。

本轮不替换Tauri前端、不新增图片引擎crate、不维持失效的旧API兼容层。既有普通截图、选区、圆角、OCR、滚动、保存、历史、贴图和剪贴板仍需通过回归。

## 2. 当前源码与改造落点

| 现有文件／模块 | 当前断点 | 改造 |
| --- | --- | --- |
| `crates/snapclip-model/src/capture.rs`、`pixels.rs` | 已区分 BGRA8／FP16、编码、Alpha 与来源 generation；载荷已支持压缩像素母版 | 后续捕获和消费必须传播真实定义，不能由显示输出重新解释源 |
| `crates/snapclip-model/src/artifact.rs`、`pin.rs` | 像素／编码图的合同传播到业务 | 原尺寸、精度、色彩、来源revision完整传递 |
| `crates/snapclip-capture/src/windows/win/wgc.rs` | 单帧、session及重建共用 BGRA8／FP16 格式合同；生产仍显式请求 SDR | 接真实源能力快照，实机验证 FP16 捕获及变化重建 |
| `windows/win/d3d11.rs` | typed ROI、上传、目标与显式 swap-chain 颜色空间已接通 | 生产捕获／显示策略使用真实来源定义，保持 context 线程所有权 |
| `windows/win/d2d.rs`、`windows/renderer.rs` | 内部 FP16 背景／模糊／标注／UI 白已验证；生产无 HDR 白点快照时明确拒绝 | 接真实输出白点；继续迁移 HDR 倒计时、取色派生与生产滚动 |
| `windows/providers.rs` | 冻结帧已有 typed 缓冲、四／八字节裁切和独立 generation；真实 WGC 来源仍 SDR | 同步迁移生产捕获格式，继续保持冻结后关闭 pool |
| `ports.rs`、`artifact.rs` | `BandSource::source()`／`PixelSliceSource` 已携带定义，FP16 圆角已实现 | 下游使用选区内容定义；读回前后校验来源代次，不能混合来源 |
| `scroll/`、`windows/scroll_worker.rs` | 原始字节存储已可按 FP16 行宽落盘；匹配／预览／生产 session 仍为八位 | 拼接几何用分析图，保存的行带保留原始精度并传播色彩 generation |
| `apps/snapclip/src/capture/`、`save/bands.rs` | writer 已将高精度来源写为 `.scpx`；普通 SDR 保留流式 PNG；旧消费者显式拒绝高精度 | 继续完成 HDR 编码、明确 SDR 派生及保存工作流 |
| `apps/snapclip/src/save_panel/` | 原图/结果/对比已保留高精度，同一 revision；HDR PNG16 与明确 SDR 策略 | 原图归档与导出分离，真实 HDR/DPI 验收 |
| `crates/snapclip-pin/src/image_preview.rs`、`view.rs` | F3 已保留 FP16 原始 Arc，普通复制显式生成 SDR | 真实输入/显示验收，原始复制不走显示代理 |
| `crates/snapclip-pin/src/screenshot.rs` | F6 已使用共用 Yue paint；旧 `host.rs`/`source.rs` 删除 | 保持原位/无激活/DPI 合同，见 §16 |
| `crates/snapclip-clipboard/src/writer.rs`、`windows/reader.rs` | 输出四字节像素／PNG，输入PNG优先后DIB | 明确复制策略，优先解析原编码PNG色彩，损坏时才用可说明的备用格式 |
| `crates/snapclip-history`及应用history入口 | SQLite 源引用、SDR 行缩略图、真正原图回放/复制已接入，见 §17 | 交互预算 256 MiB；超限保留源并明确拒绝；长母版的历史导出入口仍需补齐 |

现有 `docs/49-avif-bmp-selection.md` 文末记录AVIF编码／解码已实现，不能再说“整个项目没有AVIF”。但当前GPUI应用的 `capture/convert.rs` 及Yue公共encoded入口不能据此被认定已有HDR AVIF；P0须分别核对Tauri、工具和实际生产保存路径。

## 3. 捕获、覆盖层和滚动截图

### 3.1 捕获策略

WGC使用统一的源能力快照：已确认SDR且来源范围稳定时可用现有八位；HDR、跨HDR输出或无法可靠确定的来源选择FP16。选择记录到冻结帧，不能在导出时重新读取系统状态来解释旧像素。捕获API的格式参数和Windows scRGB能力见Yue方案[W1][W2]，FP16全链路是据此确定的项目合同。

GPU帧携带尺寸、format、scRGB亮度语义和source generation；frame pool创建、重建、冻结纹理、CopyResource及readback格式必须匹配。不能只修改 `DirectXPixelFormat`，让后续 `width * 4`继续工作。

HDR切换、跨显示器或捕获尺寸／格式变化触发一次有序重建。普通冻结截图保留取得时的定义；滚动session遇到格式／色彩generation改变，应暂停并重建一致的母版或提示重试，不能把不同白尺度／编码的行带直接拼在一起。

BitBlt等只能提供SDR的fallback，返回来源能力限制。HDR捕获失败时不得把fallback静默称为HDR：让用户明确选择SDR结果或重试；保护／黑屏内容和HDR失败是不同原因。

### 3.2 Direct2D覆盖层

保留现有native截图覆盖层及D3D11 device，不为了显示冻结图另建GPUI覆盖层。将目标／中间效果升级为支持的FP16／scRGB，按微软Direct2D颜色管理合同指定bitmap色彩上下文及效果精度，而非仅换DXGI格式。

冻结背景保持绝对scRGB；选区外压暗是线性合成的视觉效果，不污染母版。标注／边框／文字及工具UI按SDR white显示。放大镜取原始高精度像素再显示；取色若仍输出HEX，需明确为SDR映射结果，不把>1截断后声称原色。倒计时模糊、滚动缩略图及CPU fallback同样检查。

选区裁切、圆角与透明Alpha在高精度工作空间完成，PNG16编码前才量化。D3D immediate context仍由既有线程拥有，任何新转换不能跨线程无锁使用该context。

### 3.3 母版不能只是HDR PNG

**FP16 scRGB→PNG16/PQ有量化及可表达色域限制，不能取代精确保留的原始捕获。** 删除“所有来源的唯一母版都是八位PNG”这一模型约束。

- 文件来源保留原编码文件／稳定引用和色彩定义。
- 捕获来源保留scRGB16F像素母版；优先扩展现有行带／压缩tile存储的格式描述，复用磁盘后备和生命周期，不新增通用媒体容器框架。
- 普通截图与滚动截图使用同一像素描述，按需以行带读取；compressed母版做字节无损往返测试。PNG是可重建的导出产物，不能被历史层当成唯一真源。
- 母版格式版本显式变更；项目开发期不加旧布局兼容读取器。迁移说明明确哪些旧开发缓存需要重新生成，实施不得自动清空用户目录。
- 如果现有容器扩展实测不适合普通截图，先记录具体峰值／I/O问题再选择现成无损浮点容器；不能靠改为PQ PNG掩盖失真。

滚动匹配／OCR允许使用小型SDR／灰度分析图，但估计结果只决定原始行带的拼接几何。压缩、裁切、接缝和导出母版保留8字节像素，禁止从分析图反向构造HDR原图。tone-map参数在同一分析session固定，不能每帧自适应亮度导致匹配不稳定。

## 4. 保存面板、历史与复制

### 4.1 保存面板

继续使用YueInput、Select、Segmented和ImagePreview，不新增自绘HDR控制器。宿主保留输入实体、图片任务、色彩策略和结果revision，render只读状态。

- HDR内容初始选择“保留HDR”及有证明的编码格式；只验证通过的HDR PNG16先开放。
- 普通JPEG／WebP／BMP提供“转换为SDR”策略后才可保存。该策略是可见参数，不能选择格式后背地丢HDR。
- 原图／结果／对比使用真实编码后解码结果；若结果为SDR，显示“SDR结果”。缩放／平移不因切换策略重建。
- 大图原文件可以直接保留；改尺寸／圆角／格式则从原始母版处理，不能从显示代理导出。
- “自动缩小”、目标体积、质量和Alpha背景继续使用真实格式能力；HDR PNG没有有损quality旋钮，不复用SDR字节估算当精确值。
- 输出不支持时显示原因及SDR预览，不禁止保留原始HDR文件。

历史存母版引用、色彩描述和编码产物，缩略图作为SDR派生图独立生成。恢复／再次保存走原始母版；删除历史时按既有资源所有权释放关联文件，不把仍由贴图／导出持有的源删掉。

### 4.2 剪贴板

现有默认复制明确为**SDR兼容复制**，CF_DIBV5和PNG来自同一已映射图像。不要继续沿用“可选PNG一定与像素缓冲相同”这一未校验假设，测试应核对两种载荷的含义。

增加独立“复制HDR图片”：使用带有效色彩定义的PNG16，说明目标应用必须支持HDR PNG。是否附带SDR DIB必须根据接收方实际优先级验证后决定；不能保证浏览器／Office／其他图片软件都会选择HDR载荷。

F3读取已复制HDR PNG时保留编码精度；仅有DIB时按其真实能力处理。读取到损坏HDR PNG而退回DIB，应显示SDR来源／回退原因，不能仍贴HDR标签。剪贴板变化、取消与来源revision继续原子检查。系统测试会改用户剪贴板，必须保留opt-in／ignored，不在普通验收里自动执行。

## 5. F6迁移：一套图片显示、保留原位合同

决策：F3与F6图片统一到已有GPUI／Yue图片展示。删除完成迁移后的GDI像素绘制和私有缩放缓存，不再维护独立的native HDR贴图renderer。F3的文档／文件展示不受影响。

F6与F3的业务差异作为**创建意图**而不是第二套图片模型：

| F6既有合同 | 迁移后要求 |
| --- | --- |
| 最近截图的一次性消费 | 仍由应用处理，失败时不能错误消费或重复贴出 |
| 原屏物理坐标、1× | 支持负坐标、跨DPI后仍按原始像素定义，不能改为默认居中fit |
| 置顶、tool window、不抢焦点 | 窗口创建／显示都无激活；不能只在创建后调用一次`SetWindowPos`来补救 |
| 点击拖动、滚轮0.1～10倍、鼠标锚定 | 保留现有输入语义，不能强制复用F3普通文档的手势 |
| 双击关闭、倍数徽标、圆角 | 保留，支持键盘／可访问入口时不引入无意义焦点抢占 |
| 生命周期、报告／事件、关闭恢复 | 统一进现有PinManager，关闭、撤销及源资源释放有测试 |

先为core Windows窗口补充必要的窄平台能力：无激活创建／显示、topmost、物理位置与DPI处理；由同一个GPUI窗口生命周期管理，保留 `Root`及现有组件基础。若当前GPUI窗口选项已能表达，直接使用，不重建同名包装。不得在两个窗口线程间同时操作HWND和renderer。

先把上述合同写成回归，再迁移；若无激活／DPI无法被真实窗口验证，不能仅以F3无头测试通过宣称F6迁移完成。旧宿主只在替代路径验收完成后删除，不为兼容长期保留两条绘制路线。

## 6. 样本、预算与依赖

本地样本见 [HDR-IMAGES说明](../HDR-IMAGES/README.md)和[manifest](../HDR-IMAGES/manifest.json)。原图已精确忽略，不把约400MiB的PNG或对话日志提交进仓库。清单hash只识别用户提供文件，不证明上游commit或像素色彩正确性。

- HDR PNG：RGB16，cICP `[9,16,0,1]`及Rec.2020 PQ ICC。
- SDR PNG：RGB16、sRGB ICC。16位不是HDR判据。
- 两张AVIF：12位YUV444，均同时含ICC与nclx；nclx primaries／transfer为unspecified，不能只读ffprobe就丢ICC。
- 四图均45.44MP；单份RGBA16F约346.69MiB。两张PNG超过现有64MiB文件预算，四图超过当前Pin的2400万像素限制。

调整为编码文件、解码峰值、显示代理、原始母版、并发源和总存活预算分别控制。大图用流式来源和有界HDR代理；不为了测试样本取消所有限制。默认Pin预算改变需与多贴图总量一起测，不能仅把24MP改成64MP。

保留现有静态捕获完成冻结后关闭WGC session／frame pool的行为，不因HDR让整个pool活到贴图关闭。母版持久化后，按后续实际需要释放捕获GPU帧和临时CPU像素；贴图只持有原始来源引用及显示资源。滚动session只保留有界帧与磁盘后备，多个窗口按真实swapchain和atlas容量记账。复用既有device不代表必须永久保留所有纹理。

CI新增可提交的小型fixture：已知亮度阶梯、透明HDR边、带ICC的PQ／sRGB、元数据冲突／损坏、可支持时的12位AVIF。裁小后必须检查位深和标签未丢；未核实再分发许可前，不从用户大图裁片提交。数值fixture优先自行生成，保留生成脚本和预期值。

Rust依赖使用Yue方案的决策：现有windows／D3D11、image／png、moxcms和half；不自动引入gainforge／pic-scale／新codec。SnapClip现有AVIF实现和许可证按实际包核对，不能把AGPL依赖变成MIT Yue默认依赖。纯数学CPU与HLSL共享定义和golden vector，不为共享少量数学而让capture依赖GPUI视图。

## 7. 执行批次与验收

阶段依赖遵循Yue方案P0～P5，应用部分按下表推进，每批都要先有基线及可复现失败证据。

| 批次 | 新增测试必须证明 | 必须保持的既有能力 |
| --- | --- | --- |
| A 模型／行带／母版 | 带padding的FP16读回、数据长度、防溢出；compressed母版逐bit往返；源与代理不同 | SDR行带、长图流式、裁切、圆角及历史读取 |
| B WGC／覆盖层 | 三处pool路径统一；>1／负值不丢；D2D文字／遮罩／放大镜不二次缩放 | 普通截图fallback、选区／倒计时、线程所有权 |
| C 滚动 | 分析图可复现、FP16行带保留、generation变化不混拼、超长图有界处理 | 滚动匹配／接缝／磁盘后备及取消 |
| D 保存面板 | 真正HDR编码结果重新解码后保高光；SDR导出显式；取消／过期结果不覆盖 | 文件名／路径／格式、体积、Alpha、保存后动作、焦点与快捷键 |
| E F3／F6 | F3 HDR PNG、F6原位无激活／DPI、切屏、缩放、关闭／撤销、回收 | 文本／文件／表格贴图、原图复制、最近截图一次性消费 |
| F 剪贴板／历史 | PNG16元数据与数值、默认SDR载荷一致、重放走原始母版 | 自复制标记、去重、损坏回退、文件引用及其他剪贴板类型 |

命令在实施前后都运行并留日志；下列为验证入口，实际执行证据见 §8，不以命令列表代替通过记录：

```powershell
cargo test -p snapclip-app -p snapclip-pin --features snapclip-app/test-support,snapclip-pin/headless-tests --lib --locked --offline
cargo test -p snapclip-model -p snapclip-capture -p snapclip-clipboard -p snapclip-history --lib --locked --offline
cargo clippy -p snapclip-pin --features headless-tests --all-targets --no-deps --locked --offline -- -D warnings
cargo check -p snapclip-app -p snapclip-pin --all-targets --locked --offline
cargo fmt --all -- --check
```

捕获／母版和格式改造后，再执行相关集成测试、Release编码测试及完整workspace测试／Clippy。先记录当前已知失败，不把“原来就失败”当作本次不用回归，也不为了让全量变绿删除／弱化断言。真实系统副作用测试单独列命令和授权执行记录，不把ignored计入通过。

实机验收：HDR开／关和SDR白变化、HDR／SDR双屏、100／125／150／200%DPI、截图→覆盖层→保存面板→HDR文件→F3／F6→历史恢复。对照原内容高光、颜色与SDR UI，记录设备及设置；普通SDR保存／贴图也必须实测。现有Computer Use通道此前不可用，不能把本方案或旧预览exe当作本轮验证。

性能在Release下分别测4K截图、滚动长图、45MP文件和多贴图：首帧、冻结到可见、读回／上传、转换／编码、P50／P95交互、峰值内存／GPU容量、取消与20次替换后的释放。WGC池、冻结帧、覆盖层swapchain和GPUI窗口swapchain分开记账；仅算单张8字节像素不足以估计总占用。

完成条件是端到端证据，不是某个 enum 新增成功。用户授权后，先前检查点已正常提交并推送（本仓库 `3b405c7`，Yue `4846e89`）；接续的 P3-B／P3-C 改动仍在工作区。没有切换系统 HDR 或清理开发缓存；WGC 格式合同／D2D 内部链路见 §10，接着推进生产能力快照、滚动与编码，不提前开放 HDR 捕获入口。

## 8. 第一批实施记录（P3-A，2026-11-03）

### 8.1 实际变化

1. `snapclip-model/src/pixels.rs`：新增无 GPUI/Win32/half 依赖的像素合同。`PixelDescriptor` 把布局、编码、Alpha 分开，拒绝不合法组合，反序列化同样校验；`PixelBuffer` 检查尺寸、长度、stride、有限 FP16 和合法 Alpha。HDR 负分量、超白与负零逐 bit 保留，紧凑拥有缓冲直接移动。`PixelFormat` 增加真实八字节 `Rgba16Float`。
2. `windows/win/d3d11.rs`：ROI 读回由一条 typed 路径实现，检查真实 texture format、边界、溢出、row pitch，按四／八字节生成 `PixelBuffer`。`Map` 后即使复制失败也执行 `Unmap`。既有 `read_back_bgra`/`read_back_region_bgra` 是明确的 SDR 字节入口；遇到 FP16 返回错误，不截断、不解释为四字节、不自动 tone-map。
3. `scroll/backed.rs`：既有原始 tile/ring 存储改为显式布局行宽，复用磁盘文件和释放逻辑；追加／前插／读范围在修改前检查非法长度与溢出。FP16 测试检查多次 seal、反向增长、跨 tile 读取、预算和 Drop 删除临时文件。还修正了初始行数超过后续增长 cap 时缩短首帧 ring 的问题，行为与内存 oracle 一致：保留整幅首帧并拒绝继续增长。当前是**原始字节 tile，不是压缩格式**，没有新建媒体容器或缓存。

这些是源到存储的准备合同。生产冻结帧/滚动 session 尚未传播完整 descriptor/generation，tile 尚非可重启恢复的耐久母版；WGC 三处 frame pool 继续八位，因此**不把本批称为 HDR 截图功能完成**。

### 8.2 验证

修改前：`target/hdr-p3a-baseline.log` 保留 model/capture/clipboard/history 的现有回归。新测试先证明旧接口不能满足 FP16 合同（`hdr-p3a-{pixels,tiles,readback}-before.log`），实现后分别转绿：

```powershell
cargo test -p snapclip-model --test pixel_contract --locked --offline
cargo test -p snapclip-capture --lib fp16_tiles --locked --offline
cargo test -p snapclip-capture --lib typed_copy --locked --offline
# opt-in 实际 GPU，无窗口/捕获/剪贴板/系统设置副作用
cargo test -p snapclip-capture --lib fp16_region_readback --locked --offline -- --ignored --nocapture
```

5 项像素合同、1 项原始行带、1 项初始行数边界、1 项 CPU pitch、1 项实际 D3D11 FP16 ROI 测试。GPU 测试单独 opt-in，不计入普通 ignored 通过数。读取结果的每一位均与原数据一致（包括负零），并验证 SDR API 拒绝 FP16。没有执行真实 WGC 截图或 HDR 设置切换。

完整关联回归与 Clippy 日志为 `target/hdr-p3a-current.log`、`hdr-current-app-pin.log`、`hdr-p3a-clippy.log`；格式检查为 `hdr-p3a-fmt.log`。Yue 的 GPU、隐藏 HWND、数值及大图准备结果统一见 [实施证据](../../GPUI-Yue-Component/docs/performance/2026-11-03-hdr-resources.md)。

| 回归 | 修改前 | 本批结果 |
| --- | --- | --- |
| Capture lib | 545 通过 / 80 ignored | 548 通过 / 81 ignored，最终日志 `hdr-p3a-capture-final.log` |
| Model lib | 26 通过 | 26 通过，另 5 项新增像素 integration test 通过 |
| Clipboard lib | 11 通过 / 2 ignored | 相同 |
| History lib | 43 通过 | 相同 |
| App / Pin | 285 / 71 通过，另 5 / 4 ignored | 285 / 72 通过，另 5 / 4 ignored |
| 格式、app/pin all-targets 编译 | 现有入口 | 通过 |
| Model/Pin all-targets Clippy `-D warnings` | 现有入口 | 通过，`hdr-p3a-model-pin-clippy.log` |
| Capture/History 扩大 Clippy 检查 | 本批实施前未跑 lint 基线，不能称此前全绿 | **未通过**，Capture lib/test 49/79 项、History lib/test 2/3 项；包含重叠，不能直接相加 |

扩大 Clippy 的错误位于原有几何注释、`annotation`、`ports`、原有滚动测试、D2D/Win32 代码及 History；如 `d3d11.rs` benchmark 的 `worst(&mut Vec<...>)` 在 HEAD 已存在。没有用 `allow` 或删断言掩盖这些问题，也没有扩张本批去自动重写其余未提交代码。因此本批新增功能测试通过，但不能称整个 SnapClip lint 门禁已通过。

额外运行的既有真实 GPU/D2D 回归：`device_creation_and_texture_round_trip`、`composed_frame_masks_outside_the_selection_and_keeps_it_clear`、`export_crop_matches_preview_and_bakes_annotations`，均以明确筛选的 `--ignored --test-threads=1` 执行，各 1 项通过；没有运行会真实捕获屏幕或修改剪贴板的其他 ignored 测试。

## 9. 第二批实施记录（P3-B，2026-11-03）

### 9.1 来源、选区与发布合同

- 新增 `PixelSource { descriptor, generation }`，字段私有，构造／读取与 serde 合同显式。generation 属于一次采集的源定义，不能拿导出 worker 的取消计数或显示输出 revision 代替。实际冻结帧分配非零代次；旧 SDR 行带消费者目前仍用明确的 SDR 定义和代次 0，生产滚动的颜色变化检测尚未接入。
- `CapturedFrame`、`SelectionSpec`、`CaptureMetadata`、`CaptureArtifact` 的 `pixel_format` 改为 `source`；`SelectionPixels.bgra` 改为经过校验的 `PixelBuffer`。`PixelSliceSource` 改为 typed `read_pixels`，`BandSource::source()` 为必需方法，没有伪装成 HDR 的默认实现。调用方已一并迁移，不保留旧字段或旧读取适配层。
- 冻结帧 CPU 缓存保留 descriptor，按四／八字节裁切；GPU 走 P3-A 的 typed ROI。选区读回在读取前后核对来源代次，返回的 descriptor 必须与原帧完全一致。母版行读取同样在读前后校验，来源变化时拒绝发布。
- 圆角沿用原 coverage 算法。FP16 straight RGB 不变，仅乘 alpha；premultiplied RGB 与 alpha 同乘 coverage；Opaque 加遮罩后变为 Straight。`SelectionPixels::source()` 描述选区成品的真实 Alpha，保留采集代次；`frame.source` 仍描述原始冻结帧。自审发现的 Opaque 圆角发布错误已先复现失败、再修复，不用修改原帧标签掩盖派生变化。
- `sdr_bgra()` 是旧消费者的显式能力边界：只接收 SDR BGRA8，拒绝关联 Alpha 和高精度源；不在边界偷偷 tone-map。保存面板、F6、旧 PNG8 行带和 D2D 入口仍受此限制。
- `HistoryArtifactWriter` 对 ordinary straight BGRA8 保持既有流式 PNG；其他真实定义写像素母版。`CapturePayload::PixelMaster` 定位母版引用，`artifact_path()` 统一取消清理，`png_path()` 对母版返回 `None`。当前只接上应用 writer 和存储端口，未接历史 DB／重放，更未开启真实 HDR WGC 入口。

### 9.2 压缩像素母版 v1

实现位于既有 `snapclip-history` 的 `pixel_master.rs`，扩展 `CaptureArtifactStore`，不新增引擎 crate。它是捕获存储格式，不是通用图片编码器；HDR PNG 仍是后续独立导出产物。

| 项目 | v1 合同 |
| --- | --- |
| 定位 | `.scpx`；MIME `application/x-snapclip-pixels`；`SCPX` magic 和显式版本 1 |
| 定义 | JSON `CaptureMetadata`，包含 descriptor／generation；独立 BLAKE3 header 校验 |
| 像素 | 独立 zlib 行 tile，存原始字节长度对应的压缩块、原字节 BLAKE3；逐 bit 保留 FP16、负零、负 RGB 和超白 |
| 发布 | `create_new` 临时文件 → 按 tile 写 → `sync_all` → 关闭句柄 → rename；失败只清本次成功创建的临时文件 |
| 身份 | `ArtifactRef` 带编码长度、尺寸、完整编码文件 BLAKE3；打开时用 64 KiB scratch 顺序验证全文件，再校验 header 和 tile 索引 |
| 读回 | 只解压触及的 tile；检查解压精确长度、zlib 完整消耗、原字节 hash、FP16 有限值和 Alpha，再组合请求的行 |
| 预算 | 单 raw tile ≤4 MiB；单 raw row ≤4 MiB；单次输出 ≤64 MiB；header ≤64 KiB；tile 数 ≤262,144 |
| 开发缓存 | 不兼容版本直接拒绝，需显式重新生成；不猜布局、不自动删除用户目录 |

4 MiB 是**单块原始像素**预算，不是整个调用的峰值内存。写入还持有压缩块及编码器工作区；读取会同时持有请求输出、压缩块与一个解压 tile。`PixelMaster::open` 不整图解码，但会顺序读取全部编码文件，不能称为纯索引复杂度或零 I/O。持久化后的精确像素与显示代理分开，不用 PQ 量化图冒充无损母版。

新增直接依赖 `flate2 = "1"`、`half = "2.7"` 均使用已有 lock 中的包，offline 构建，不新增运行时 DLL。

### 9.3 验证与真实边界

本批基线为已推送的 `3b405c7`。缺口日志 `hdr-p3b-contract-before.log`／`hdr-p3b-master-before.log` 先证明接口不支持所需合同；`hdr-p3b-rounded-before.log` 记录真实圆角发布失败，`hdr-p3b-rounded-after.log` 转绿。

| 验证 | 修改前 | 本批最终结果 |
| --- | --- | --- |
| 普通 workspace lib，串行 | App 265、Capture 550、Clipboard 11、History 43、Model 26、Pin 71、UI 221 | 268／552／11／43／26／71／221 通过；各包 ignored 仍为 5／81／2／0／0／4／0 |
| App／Pin 无头 lib | 285／72 通过 | 288／72 通过，另 5／4 ignored |
| Model 像素 integration | 5 项 | 6 项通过，增加来源 serde 合同 |
| Capture 来源 integration | 无 typed 选区合同 | 6 项通过，覆盖 FP16、来源变化、返回定义不符、行带与三种 Alpha 圆角 |
| History 母版 integration | 无压缩母版接口 | Debug／Release 各 5 项通过，覆盖跨 tile、逐 bit、损坏、版本、失败不发布和 SDR／相对线性定义 |
| 实际 GPU／D2D 精确筛选 | P3-A 已有入口 | FP16 ROI、纹理往返、选区遮罩、带标注裁切各 1 项通过；未执行其他 ignored |
| SDR 导出分配 Release probe | 既有 3 项 opt-in | 3 项通过，无第二份全图分配、有界单 band 和真正 banded 来源 |
| workspace all-targets check／fmt check | 既有入口 | 均通过；UI gallery 示例仍有 9 项 must-use warning |
| Model／History all-targets 严格 Clippy | History 原有 3 项失败 | 均通过；修复对应原有 chunks、single-match 和无用转换 |
| Capture all-targets 严格 Clippy | P3-A 已记录 lib/test 49／79 项 | 仍失败，49／79；未新增 allow 或弱化警告 |
| 扩大 App／Pin 严格 Clippy | 本批前未跑 App 严格 lint 基线 | App lib/test 41／47 项失败，包含原有保存窗口／转码／历史等问题；不能称全仓 lint 已通过 |

普通 workspace lib 共 1,192 项通过（子进程嵌套输出不重复计数）。新增 integration、headless 和 opt-in 分开记录，不能将 ignored 算进通过数。最终日志使用 `target/hdr-p3b-{contracts,workspace,headless,all-targets,fmt}-final.log`；严格 lint、Release 和 GPU 分别见同前缀对应日志。

测试数量基线、依赖方向、desktop opt-in 标记、过时引用、ledger／符号／PowerShell BOM 门禁均通过；测试基线脚本自身的 4 项合成日志回归也通过。未通过的严格 lint 仍按上表单列，不绕过或改变门禁。

复现本批核心验证：

```powershell
cargo test -p snapclip-model --test pixel_contract -p snapclip-capture --test source_contract -p snapclip-history --test pixel_master --locked --offline -- --test-threads=1
cargo test --workspace --lib --locked --offline -- --test-threads=1
cargo test -p snapclip-app -p snapclip-pin --features snapclip-app/test-support,snapclip-pin/headless-tests --lib --locked --offline -- --test-threads=1
cargo test -p snapclip-history --test pixel_master --release --locked --offline -- --test-threads=1 --nocapture
cargo test -p snapclip-app --test export_path_copies --release --locked --offline -- --ignored --test-threads=1
```

Release 单次 1024×1200 合成 FP16 母版：原像素 9,830,400 B，编码 1,032,769 B，最大 raw tile 4,194,304 B；写入含 fixture 生成／验证约 21.786 ms，打开含全文件身份验证约 0.437 ms。数据高度可压缩，未隔离缓存与系统负载，不能推广为真实截图吞吐；未测 RSS／VRAM，也不替代 45MP 门禁。

### 9.4 后续顺序

接续的 WGC 三处 pool 格式合同、D3D 上传／目标和 D2D 内部颜色处理已完成到 §10 所列边界；尚需真实源能力与输出白点快照，再接生产滚动源／分析派生图、颜色 generation 变化拒绝混拼及 HDR PNG。保存面板、F3／F6、剪贴板与历史 DB 属于后续消费批次。真实 HDR 显示、跨屏、DPI、RSS／VRAM 与端到端性能仍未验收。

本批保留现有 SDR 用户入口；typed FP16 端口测试和母版落盘通过，**不表示普通用户现在可以截图得到 HDR**。

## 10. 第三批实施记录（P3-C，2026-11-03）

### 10.1 WGC 格式合同与原生目标

- `capture_monitor`、`WgcSession::open_monitor` 显式接收来源 descriptor；单帧创建、session 创建和 resize 重建共用一个格式映射。支持 Straight BGRA8/sRGB 与 RGBA16F/scRGB，拒绝相对 LinearSrgb 或不符的 Alpha 标签，避免把 FP16 存储直接等同于 HDR。
- session 保留不可变 descriptor，`WgcFrame` 将它交给读回方；实际纹理的格式、内容大小必须校验。resize 时丢弃旧 pool 帧，再等待新 pool 帧，受原等待期限约束。
- provider 将已请求的定义传给冻结帧，不在构造元数据时再硬编码；当前普通捕获与滚动调用方仍明确请求 SDR。这里只完成格式传播，**未执行实际 WGC HDR 捕获**，也没有替代尚缺的显示器／源能力策略。
- D3D typed 上传共用尺寸、pitch 和字节数检查；目标按真实 BGRA8／FP16 创建。composition swap chain 显式检查并设置 G22/P709 或 G10/P709；不支持所需颜色空间时返回错误。
- 保持既有 D3D11 device 与 immediate-context 单线程所有权，没有另建渲染器或引入新引擎 crate。

### 10.2 D2D 的一次颜色解释

新增私有 `d2d/color.rs`，集中管理目标格式、scRGB bitmap context、效果中间缓冲精度和 UI 白点；调用方需要提供已确认的输出 SDR-white 快照，不猜测 80 nit。当前生产 `Win32Renderer` 没有该快照，因此仍拒绝 HDR 来源。

冻结 scRGB 背景直接绘制，保留负值与 >1 高亮，**不乘 UI 白**。只有 authored SDR UI／标注色做 sRGB EOTF，再乘 `sdr_white_nits / 80` 一次，Alpha 保持独立。桌面 L0 沿用原捕获的不透明显示约定（D2D Ignore Alpha），不是用于透明文件的通用图片预览器；透明 UI 的混合另有数值测试。

以下路径一起迁移：

- source bitmap、swap target、blur cache、带标注 export target 使用对应格式；HDR 效果缓冲指定 16bpc float。
- 静态画刷、动态 `SetColor`、标注／高亮、渐变 stop 与小型 SDR 缩略图按同一 UI 白解释；HDR 渐变使用 scRGB／FP16。小缩略图是显式 UI 派生内容，不用于重建 HDR 母版。
- 14 个图标的着色目标使用 FP16；缓存颜色键保存四通道完整 bit，不再用折叠 hash。重建／白点变化时清理目标、画刷、图标和缩略图；失败的图标准备不发布部分缓存。
- bitmap 属性中的 `ManuallyDrop` COM 颜色上下文在成功／失败路径均释放本地引用；资源创建失败不留下可被误判为完整的 target。
- `render_export` 返回 typed `PixelBuffer`，实际调用方核对尺寸及来源定义；错误后也恢复上下文 target。既有 SDR 裁切逐像素断言保留。

放大镜有一项先失败再修复的真实缺陷：只跳过八位中心填色仍会留下十字辅助色带的染色，测试取到 R=0.1776123，而源值是 0.25。现在中心格在辅助层之后直接重绘原始 FP16 像素，不使用滞后的八位样本；HDR HEX 色值暂显示 `--`，不伪装为原始 HDR 取色。

### 10.3 验证与边界

先失败证据：`target/hdr-p3c-before.log` 与 `hdr-p3c-wgc-before.log` 分别记录缺失 typed renderer／格式 API；`hdr-p3c-loupe-before.log` 记录上述实际像素偏差。基线 `hdr-p3c-baseline.log` 的单包 Capture 为 550 passed／81 ignored；workspace 会由 feature unification 额外启用原有两项 `test-support` 测试，不能把这个差异当作测试丢失。

| 验证 | 实际结果与限制 |
| --- | --- |
| 普通 workspace lib，串行 | App 268、Capture 556、Clipboard 11、History 43、Model 26、Pin 71、UI 221，共 **1,196** 项通过；各包 ignored 为 5／84／2／0／0／4／0，分开不计入通过数 |
| App／Pin headless lib | 288／72 通过，另 5／4 ignored；没有新增桌面副作用 |
| 像素／来源／母版 integration | Model 6、Capture 6、History 5，17 项通过 |
| 新增普通测试 | 4 项：WGC 格式、纹理合同、UI 白／生产门禁和 SDR 缩略图解释 |
| 新增 HDR GPU opt-in | Debug／Release 各 **3** 项通过：CPU／GPU 背景、遮罩／模糊、FP16 标注导出；14 图标、Alpha、80／160／240 nit 白点切换并回到 SDR；放大镜中心原色及拒绝 FP16 进入八位 sampler |
| 既有 SDR D2D opt-in | 最终筛选 **28** 项通过；另外一次较宽筛选共 31 项，包含字体／性能 probe 和文本清单 codegen，不能作为 31 项像素断言或性能达标证据 |
| D3D opt-in 精确筛选 | 6 项通过：纹理往返、全帧读回、FP16 ROI、无绑定窗口的 swap-chain present、上传纹理异步采样、context 所有权；未运行 WGC 截屏或可见 HWND 测试 |
| all-targets check／fmt check／diff check | 通过；`component_gallery` 原有 9 项 must-use warning 保留，本批没有新增 rustc warning |
| Capture strict Clippy `--no-deps` | **仍失败**：lib 41／lib test 71 项，后者包含前者；本批前为 49／79，清理了触及画刷的 8 项 unit binding，没有增加 allow |
| 含 workspace 依赖的 Capture strict Clippy | 在 UI 原有 16 项 lint 处失败；不声称整个 workspace lint 通过，App 上批的 41／47 项失败也尚未清理 |
| 仓库门禁 | 测试数量基线及其 4 项合成回归、依赖方向、desktop opt-in、过时引用、ledger／符号／PowerShell BOM 均通过 |

主要日志：`target/hdr-p3c-{workspace-final,headless,contracts,gpu-final,gpu-release-final,sdr-d2d-final,d3d,check-final,fmt-final,capture-clippy,clippy}.log`。GPU 验证只创建离屏资源／未绑定可见窗口的 swap chain，没有切换系统 HDR、实际截屏、修改用户剪贴板或清缓存。它证明这台 GPU 上的内部数值，不证明显示器观感、DWM 终端输出或 WGC 抓到的真实 HDR 帧。

复现核心命令：

```powershell
cargo test --workspace --lib --locked --offline -- --test-threads=1
cargo test -p snapclip-capture --lib hdr_ --locked --offline -- --ignored --test-threads=1
cargo test -p snapclip-capture --release --lib hdr_ --locked --offline -- --ignored --test-threads=1
cargo test -p snapclip-capture --lib windows::win::d2d::tests:: --locked --offline -- --ignored --test-threads=1 --skip the_cost_of --skip font_cost_probe --skip write_drawn_text_for_the_font_subset --skip the_overlay_thread_never_blocks
cargo test -p snapclip-capture --lib windows::win::d3d11::tests:: --locked --offline -- --ignored --test-threads=1 --skip composition_target --skip the_cost_of --skip the_latency_ --skip async_sample_from_a_wgc
cargo clippy -p snapclip-capture --all-targets --no-deps --locked --offline -- -D warnings
```

**仍未完成**：真实源能力／输出白快照及变化策略、实际 WGC HDR 帧验收、生产滚动的原始行带与固定分析派生图、HDR 倒计时 composition 宿主、明确映射的 HEX sampler、HDR PNG、保存面板／F3／F6／剪贴板／历史 DB 消费。旧 `UpdateLayeredWindow` 倒计时遇到 HDR 明确报错，生产滚动读回也只接受 SDR，均不静默截断。

下一步先接源能力与输出白快照，并让生产滚动保持同一来源 generation；再推进编码和消费。真实 HDR／跨屏／DPI、45MP、RSS／VRAM、20 次替换资源残留与端到端性能门禁仍保留。本批没有得出性能提升或低占用的结论，也没有开启用户 HDR 捕获入口；P3-B／P3-C 改动尚未提交。


## 11. 后续完整执行清单（2026-11-03）

已按用户要求提交并推送上一检查点：SnapClip `a2932db`，Yue `7e9b74d`。以下是接续实施清单，勾选必须有对应验收证据；不得把底座完成等同于产品完成。

- [ ] 源能力和输出白快照、状态变化与明确 fallback；保留生产入口门禁至所有消费者就绪。
- [ ] 生产滚动：固定 SDR 分析、FP16 原始行带、来源变化检测、反向增长及有界落盘。
- [ ] HDR composition 倒计时、明确为 SDR 派生的 HEX 取色。
- [ ] HDR PNG16/Rec.2020/PQ+cICP、明确 SDR 导出、真实编码回读及有界处理。
- [ ] 保存面板 HDR 策略和格式能力、原图/结果/对比同一相机和 revision。
- [ ] F3/F6 共用 GPUI/Yue 图片窗口、保留原位/无激活/DPI/缩放合同并删除被替代的 native F6 实现。
- [ ] SDR/HDR 剪贴板独立策略、载荷一致性和 PNG 优先保精度读取。
- [ ] 历史保真源引用、SDR 缩略图与共享资源生命周期。
- [ ] HDR AVIF 按真实生产编解码/许可/色彩证据门禁。
- [ ] Release 资源性能、旧功能回归、严格 Clippy 与真实窗口/HDR/DPI 验收。

进度解释：§12–§16 已交付代码与分层测试；本清单保留产品级验收状态，不能仅凭无头、合成 DPI 消息或离屏 GPU 数值勾选全部完成。F6 旧宿主的删除与共用资源已在 §16 落地，真实热键/跨屏/HDR 肉眼矩阵仍待验收。

### 11.1 纯数学共享边界

Yue 的 `yue-color` 仅承载已有的传递函数、基色矩阵及 BT.2390 数学，无 GPUI、图像、编解码器、资源或平台依赖。目的是让原生捕获和 GPUI 显示引用同一个实现，而非复制数值算法或让 Capture 反向依赖 GPUI。ICC 管理仍留在 Yue 图片资源层，采集/文件/生命周期仍在各自既有模块；没有新增媒体引擎。

Model 新增 `half` 表达 FP16、`yue-color` 承载纯数学。依赖门禁只额外允许这两个包和 `half` 的 `cfg-if`／`zerocopy`／`zerocopy-derive`，没有放开任意传递依赖；UI、Windows、图片编解码器和能力 crate 仍被禁止。原 §23 模块拆分记录中的“仅标准库 + serde”是当时基线，本节取代其当前依赖约束。

## 12. 第四批实施记录（P3-D，2026-11-03）

### 12.1 捕获、滚动与导出

- `windows/color_output.rs` 从 Windows 读取真实输出能力、SDR-white 及拓扑快照，区分 SDR、HDR、advanced color 与未知状态。`capture_preserving_color()` 使用有门禁的 FP16 路径；只有确认 SDR 才可使用明确的 SDR fallback。普通 `capture()` 仍维持 SDR，未在消费者尚未齐备时开启生产 HDR。
- 滚动匹配保持固定 SDR 分析图，输出行带保留 FP16 原始像素与来源 generation；来源定义／输出状态改变时拒绝混拼。内存及磁盘后备沿用已有滚动存储，不复制成第二个媒体引擎。
- `pixel_export.rs` 显式区分 `PreserveHdr` 与 SDR 派生。HDR PNG 为 16-bit Rec.2020/PQ，附 cICP `[9,16,0,1]`；按行处理。不可表示的负／超域 RGB 或超过 10,000 nit 的源明确拒绝，不能用静默裁剪把有损导出称为无损母版。SCPX 原始像素母版仍保留完整 FP16。
- 保存面板持有共享原像素，原图／结果交给 Yue；缩放在线性光、关联 Alpha 下计算，HDR／SDR 和格式能力由显式导出策略控制。

### 12.2 贴图与剪贴板

- `PinImage` 改为持有 `PixelSource` 与 `Arc<PixelBuffer>`；截图到贴图共享同一分配，保留编码、Alpha 与 generation。屏幕位置独立存于 `PinSelection`，不再用 RGBA8 数组同时承担位置和色彩合同。
- F3 注入 Yue 的颜色管理解码器，PNG 优先于 DIB；PQ PNG／高精度 SDR 继续保留 FP16。PNG 解码失败、改用 DIB 时显示明确的 SDR 副本提示。framework-free 默认解码器拒绝自身不能正确管理的高精度／颜色元数据，而非静默降为八位。
- Yue `ImagePreviewPixels::into_bytes` 将拥有的解码缓冲搬移到 Model，不复制整图；借入资源仍会建立独立所有权。传递函数、矩阵和 BT.2390 共用已提交的零依赖 `yue-color`。
- 普通复制显式派生 SDR，从同一 RGBA 生成 sRGB PNG 和 DIB；HDR 图片菜单另设“复制 HDR 图片（PNG）”，只发布高精度 PNG。获取系统剪贴板锁之前校验完整编码流、尺寸预算、位深和 cICP。F3 窗口使用 `AutomaticHdr` 输出策略。
- **F6 仍使用旧 native GDI 宿主，明确拒绝 HDR**。尚未完成原位、无激活、物理像素缩放、跨屏 DPI、圆角和生命周期的 GPUI 迁移；当前不能称 F3／F6 已统一。失败时捕获槽位的保留语义也必须在后续迁移中修复。

### 12.3 关键行为与验证

| 行为 | 修改前 | 本批实现与证据 |
| --- | --- | --- |
| 捕获到贴图 | 转成 RGBA8 并复制 | 共享原 PixelBuffer；测试 `Arc::ptr_eq`、负坐标、generation 91 与 Alpha |
| PQ PNG 贴图 | 八位解码丢失高亮 | 同一 Yue 解码器保留 1,000 nit、FP16 负值／超白和 16-bit SDR |
| 普通图片复制 | PNG／DIB 生成路径可能分离 | 同一 SDR RGBA 生成两种载荷；纯测试比较真实 DIB serializer 输出 |
| HDR 复制 | 无独立策略 | 高精度 PNG 单格式命令；测试完整流、截断、SDR 拒绝和与普通命令分离 |
| 保存面板重试 | 注入失败未通知父 view，测试无法看到按钮 | 先复现失败，再通知并测试重试恢复 Ready；不依赖已移除按钮仍存在 |
| Model 依赖门禁 | 只允许 serde，拒绝新数值依赖 | 精确增加纯数学 allowlist；正常图通过，`-Package snapclip-app` 负对照仍拒绝 UI／能力依赖 |

本批 Yue 检查点已推送为 `2ccc07d`。验证期间 Yue 工作树有独立、未提交且持续变化的虚拟列表重构；为避免混入该任务，SnapClip HDR 回归使用 `target/hdr-vendor-baseline` 中的 Yue `7e9b74d` 快照，叠加 `2ccc07d` 的 `into_bytes`，并使用已提交的 `yue-color`。Core／Base／Windows 后端均固定到此快照。`target/hdr-isolated.toml` 及快照仅为本机验证设施，不提交，也不是产品默认依赖配置。**下表不能解释为当前另一个任务的工作树已通过完整构建。**

| 验证 | 最终结果 |
| --- | --- |
| 普通 workspace lib，串行、隔离依赖 | App 273、Capture 561、Clipboard 13、History 43、Model 31、Pin 73、UI 221，共 **1,215** 项通过；另 5／85／2／0／0／4／0 ignored，不计入通过数 |
| App／Pin headless lib、隔离依赖 | **293／75** 项通过，另 5／4 ignored |
| 图片资源纯 lib | Yue image_preview **48** 项通过，包含 owned 缓冲搬移／borrowed 所有权 |
| Model／Clipboard lib 与 integration | **75** 项通过，另 2 ignored；分类 integration 25 项 |
| Model／Clipboard／Pin all-targets strict Clippy | 带 Pin headless feature、`--no-deps -D warnings` 通过；不等于全仓 lint 通过 |
| workspace all-targets check、fmt check | 隔离依赖构建通过；UI gallery 既有 9 项 must-use warning 保留；fmt 通过 |

主要日志为 `target/hdr-p3d-{workspace-final,app-pin-isolated-final,model-clipboard-pin-clippy,check-final,fmt-final,contracts-final,dependency-before,dependency-after,dependency-negative}.log`。重试先失败／后通过见 `hdr-p3d-retry-{before,after}.log`；原 Pin 调用方缺失 typed API 的失败证据见 `hdr-p3d-pin-isolated-before.log`。子进程额外输出的测试结果不重复计数。

核心复现命令（隔离配置仅在有上述本机快照时可用）：

```powershell
cargo test --workspace --lib --locked --offline --config target/hdr-isolated.toml -- --test-threads=1
cargo test -p snapclip-app -p snapclip-pin --features snapclip-app/test-support,snapclip-pin/headless-tests --lib --locked --offline --config target/hdr-isolated.toml -- --test-threads=1
cargo test -p snapclip-model --test pixel_contract -p snapclip-capture --test source_contract -p snapclip-history --test pixel_master -p snapclip-clipboard --test classification --locked --offline --config target/hdr-isolated.toml -- --test-threads=1
cargo clippy -p snapclip-model -p snapclip-clipboard -p snapclip-pin --all-targets --features snapclip-pin/headless-tests --no-deps --locked --offline --config target/hdr-isolated.toml -- -D warnings
```

本批没有改系统 HDR 设置、实际截屏或写系统剪贴板；headless 证明实体／命令／资源生命周期，不能证明 DWM 最终显示、无激活窗口或实际剪贴板格式优先级。尚需 F6 宿主迁移、倒计时 composition、SDR HEX、历史 DB 保真引用／回放、AVIF 生产证据、Release 资源性能，以及真实 HDR／双屏／DPI 验收。§11 清单保持未勾选表示这些是产品级验收项；本节只记录已完成的代码及测试层次。

## 13. F6 迁移前置（P3-E，2026-11-03）

第四批已提交并推送为 `c461313`。本批先解决截图源的创建交接，再验证 GPUI 无激活窗口合同；尚未替换 F6 的 native 图像宿主。

原先读取选区就清空槽位并结束捕获，窗口同步拒绝或异步创建失败后无法重试。修改前回归 `hdr-p3e-source-before.log` 实际失败。现在 `SelectionProvider` 只提供共享源，`PinSource::claim` 保留一次创建资格；只有窗口 `Created` 后执行 `SelectionClaim::commit`。失败／取消释放资格而不消费源；同源不能重复创建，新选区可独立创建，旧回执不能消费新选区或取消新捕获会话。捕获线程 post 失败也保留原槽位。像素通过同一 Arc 传递。

Yue core 增加 Windows `WindowOptions::is_activatable`，将持久无激活与初始 `focus` 分开。在 native 创建前设置 NOACTIVATE／TOOLWINDOW，初次与后续显示不抢焦点，鼠标消息返回 MA_NOACTIVATE，显式 activation 不进入设置焦点或模拟 Alt 的路径。详见 Yue `docs/performance/2026-11-03-pin-window-contracts.md`。短暂的真实 GPUI popup probe 已通过；它不是 F6 的完整 UI 验收。

| 验证 | 实际结果 |
| --- | --- |
| 普通 workspace lib，串行、隔离依赖 | App 275、Capture 564、Clipboard 13、History 43、Model 31、Pin 73、UI 221，共 **1,220** 通过；子进程重复结果不另计 |
| App／Pin headless lib | **295／76** 通过，另 5／4 ignored |
| Pin all-targets strict Clippy | 带 headless feature，`--no-deps -D warnings` 通过；未宣称全仓 lint 通过 |
| GPUI Windows 普通回归 | **51** 通过／3 ignored |
| GPUI 无激活真实窗口 opt-in | **1** 通过；初始隐藏／显示、延后显示、实际 HWND 样式／消息及前台 HWND 不变；普通窗口作为负对照 |
| 新测试脚本普通模式 | `tools/test-gpui-window.ps1 -CorePath ...`，1 通过／1 ignored |

应用日志为 `target/hdr-p3e-{source-before,source-after,handoff-headless,workspace-final,pin-clippy}.log`。依赖隔离沿用 §12.3 快照，再叠加本批 core 激活选项与 Windows 后端；没有纳入并行虚拟列表改造，不能据此称默认依赖工作树已完整通过。没有切换系统 HDR、实际截屏或写剪贴板。真实 F6 的原位、DPI、拖动、0.1–10 倍鼠标锚定缩放、圆角、关闭／撤销与资源释放仍须迁移后逐项验收。

## 14. HDR 异步取色（P3-G，2026-11-03）

### 14.1 显示和数值分别负责什么

- 放大镜中心格继续直接绘制原 FP16 scRGB 像素，保留负通道和高亮；不使用八位派生值覆盖它。
- 数值栏和旁边的小色块使用明确的 SDR 派生值，HDR 时显示 `SDR #RRGGBB`、`SDR rgb(...)` 或 `SDR hsl(...)`。S 切换格式，C 复制纯数值，UI 标签不进入剪贴板载荷。
- 采样固定使用 **1000 nit 来源峰值策略、100 nit SDR 目标**及 Model/Yue 共用的 BT.2390 数学。1000 nit 是本功能的固定映射参数，**不是**检测出的显示器／内容峰值；不随鼠标或每个 tile 改变。它不承诺与按整图内容峰值映射的导出像素完全一致。
- 每次只异步回读 32×32 像素：SDR 是 4 KiB，FP16 是 8 KiB；不读整屏、不修改原始纹理。三块 staging 满时拒绝覆盖；格式变化重建 staging，编码／alpha／策略重绑使旧完成失效，拒绝非法重绑时保留原请求。

### 14.2 同时修复的调度问题

原控制器只有一个 `sample_slot` 和一个 pending origin，却允许快速移动到另一 tile 时继续提交，旧编号被覆盖后无人领取。新增纯测试先复现该失败（`hdr-p3g-scheduler-red.log`），再修改为一个在途请求。完成严格使用提交时的 origin，再检查当前鼠标是否需要补采；节流期间鼠标停止也会重试。命中缓存时不保持刷新，表单／标注／倒计时中隐藏放大镜时不启动新采样。错误仍保留最后有效数值，不自行安排无界错误重试。

### 14.3 已执行验证和边界

| 验证 | 实际结果 |
| --- | --- |
| 普通 workspace lib、串行、隔离依赖 | App 275、Capture 568、Clipboard 13、History 43、Model 31、Pin 73、UI 221，共 **1224** 通过；子进程重复项不另计 |
| 纯 ColorSampler | **16** 通过，包括新增 pending 不覆盖、缓存数据／四边界、原有节流／格式／stale／重置 |
| 新离屏 FP16 GPU sampler | Debug／Release 各 **1** 通过；straight／premultiplied、负通道、12.5 高亮、固定策略重绑、SDR 重绑、满槽／部分 tile 拒绝、完成只消费一次、源逐字节不变 |
| 原有 SDR 上传纹理异步采样 | **1** 通过 |
| D2D HDR 三项 | **3** 通过；中心格仍为 HDR 原色，数值色块实际填为经输出白映射的 SDR UI 色；原有 UI／alpha／白变化及 FP16 标注导出回归通过 |
| 格式与差异检查 | `cargo fmt -p snapclip-capture -- --check`、`git diff --check` 通过 |
| Capture all-targets strict Clippy | **未通过**，报告原有 `geometry`、`annotation`、滚动、Win32、测试辅助函数等问题（lib test 74 项），未添加 `allow` 或删除断言 |

日志：`target/hdr-p3g-{sampler-before,scheduler-before,scheduler-red,sampler-pure,gpu,gpu-release,d2d-ui,workspace,capture-final,capture-clippy}.log`。依赖快照继续隔离并行虚拟列表改造；默认 `cargo fmt --all` 在其缺失的 showcase `virtual_list/tests.rs` 处失败，不能称默认工作树全量格式检查通过。

测试使用离屏纹理，不截图、不写系统剪贴板、不切换 HDR 设置。实际 HDR 屏幕、放大镜各 DPI 下的文字可读性和真实复制操作仍需产品验收；§11 的倒计时＋取色组合项保持未勾选，直到 composition 倒计时也有对应证据。

## 15. HDR composition 倒计时（P3-H，2026-11-03）

### 15.1 输入宿主和颜色表面

P3-G 已提交推送为 `de39fa9`。本批保留倒计时既有的独立 layered 输入窗口、鼠标穿透、无激活、topmost、工具窗口、截图排除及圆角洞口，在其全透明 DIB **上方**挂载独立 DirectComposition 树。树持有与冻结来源一致的 FP16 scRGB swap chain；模糊和遮罩复用原 D2D 资源／输出白，不读回 HDR 像素、不经过八位内容、不重新解释来源峰值。

这一点纠正了旧文档中过宽的“DirectComposition 不能 layered”：微软 [Basic concepts](https://learn.microsoft.com/en-us/windows/win32/directcomp/basic-concepts) 的 Window composition 描述了 layered bitmap 和独立 composition tree 并存；[IDCompositionDevice::CreateTargetForHwnd](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositiondevice-createtargetforhwnd) 也明确允许顶层或 layered window。视觉树不负责输入命中，本项目的输入合同仍由 layered HWND 样式与窗口区域实现。未改主浮层的既有输入机制。

SDR 倒计时保留原 BGRA8 内容路径，与 HDR 共用一次模糊＋遮罩绘制函数。HDR composition 创建／尺寸检查失败时，明确记录错误并使用既有的纯 UI 色遮罩；**不**把 HDR 冻帧偷偷转成 PNG8／DIB。圆角洞口移动只更新 window region，不重新上传颜色表面；隐藏时先隐藏窗口，再清除 composition 和 swap-chain 引用。再次显示为 SDR 时不会叠上旧 HDR 图层。监视器尺寸变化会重建 DIB；释放前先销毁选择了 DIB 的 DC，避免对仍被选中的 bitmap 调用无效删除。

这里仍保留输入宿主的透明 DIB（4 bytes/pixel），HDR flip swap chain 另有两块 FP16 缓冲；没有宣称零额外内存。VRAM／RSS 的实际峰值和重复替换后回收，继续属于 §11 的性能验收项。

### 15.2 修改前后证据

新增测试还查到原有 `ensure_blurred_frame` 在首次生成模糊帧后未恢复原 D2D target。先记录真实失败（`hdr-p3h-target-restore-before.log`），再在结束绘制后立即恢复旧 target，包括 EndDraw 失败分支；倒计时绘制也恢复入口 target。没有改为弱化地址断言或删除旧行为测试。

| 验证 | 实际结果 |
| --- | --- |
| 新离屏 HDR 倒计时 | Debug／Release 各 **1** 通过；FP16 格式／尺寸、负通道和 >1 高亮、线性遮罩数值、来源逐字节不变、主 target 恢复、SDR 目标拒绝 |
| D2D HDR 全模块 | **4** 项实际执行通过，包括上一批的原色中心格、UI 输出白、alpha 和标注导出 |
| 短暂原生窗口合同 probe | **1** 通过；实际 layered／transparent／noactivate／topmost／toolwindow、`WindowFromPoint` 目标不变、前台 HWND 不变、截图排除标志、圆角区域、隐藏清除引用、创建失败后纯色回退、不同尺寸重建 |
| 原有 SDR 倒计时模糊 GPU 测试 | **1** 通过；模糊局部对比度与压暗阈值保留 |
| 普通 workspace lib、串行、隔离依赖 | **1224** 通过；Capture 568 通过／88 ignored，测试总数未减少；子进程重复项不另计 |
| 格式／差异 | `cargo fmt -p snapclip-capture -- --check`、`git diff --check` 通过 |
| strict Clippy | 仍未通过（lib 42、lib test 74 项）；按“诊断＋文件”对照 P3-G，新增与消失的诊断均为 0，没有扩大 allow 或删除测试 |

日志为 `target/hdr-p3h-{countdown-before,target-restore-before,d2d-gpu,countdown-release,window-contract,sdr-blur,workspace,capture-clippy}.log`。仍使用前述隔离依赖，不把并行虚拟列表工作树当作已经通过。窗口 probe 只创建短暂、无激活的测试内容，没有截取桌面、注入输入、写剪贴板或改变 HDR 设置。

这些证据证明内部颜色链路和原生输入宿主合同；未宣称已经验证 DWM 在真实 HDR 屏上的最终亮度、圆角外观、跨屏／各 DPI、完整倒计时触发截图或资源峰值。因此 §11 的产品级项暂不勾选；剩余 F6 宿主替换、历史母版引用与回放、AVIF 生产门禁、性能与实机矩阵继续执行，普通捕获入口的 preserving 门禁保持关闭。

## 16. F6 共用 GPUI/Yue 宿主（P3-I，2026-11-03）

### 16.1 所有权与实际变化

- F3 `PinView` 与 F6 `ScreenshotView` 都由应用线程的 `PinManager` 管理。F6 直接调用既有 Yue `ImagePreviewImage::paint`，使用同一来源 Arc、generation、Alpha 和颜色定义；HDR 不再被旧宿主拒绝。没有新增媒体引擎、全局图像缓存或 CPU 缩放器。
- 删除 `host.rs`、`source.rs`、`windows_host_tests.rs`：GDI DIB、专用消息线程、重采样滤镜、通道和 50ms 常驻轮询均不再参与生产。示例也删除轮询，精确删除不再使用的 parking_lot 和 Windows features。
- F6 使用无标题 `WS_POPUP`、topmost、持久 `NOACTIVATE`，物理 bounds 包含负坐标；图片以源物理像素为 1×，DPI 仅影响 DIP chrome。滚轮整数十分位、范围 0.1–10，带余量及鼠标锚定。窗口移动调用 backend 的 deferred 系统拖动。圆角 shader 与 native hit region 配合；Root 显式透明，避免半透明像素被 Kit 默认底色填实。
- open map 也拥有准备中窗口。成功上传/绘制后只回执一次；空 HWND 或解码完成都不消费截图来源。失败/取消释放 claim，不进撤销；重复/迟到回执无效。hidden 准备不提交，显示后再确认。此回执不是 DWM/显示器完成信号。
- 右键系统菜单可越出小尺寸、无激活 HWND，支持普通 SDR 复制、HDR PNG 复制、文字备选及关闭。普通 F3 选字/文件/工具栏路径保持原有语义。右键与真实鼠标拖动仍需人工验收。

### 16.2 旧行为与测试迁移

| 原实现/检查 | 当前生产路径与覆盖 | 验收边界 |
| --- | --- | --- |
| 独立线程和跨线程 request/report | 应用线程 Entity、统一编号/溢出拒绝、窗口关闭订阅、准备回执 | 线程宿主的 stopped/channel/Send+Sync 测试随被删除 API 退役，不保留空壳 |
| 原位/负坐标/滚轮/缩放/边框几何 | 14 项仍适用的几何测试迁移；增加极端 delta、NaN、完整十分位往返、边界反向及 DPI 物理几何 | 旧 `i32::MIN.abs()` 先真实 panic；新算法 O(1)，不按数千万 notch 循环 |
| ImageSource RGBA/BGRA/尺寸/不透明化 | 14 项真实生产 PinImage→Yue 资源测试，覆盖行序、stride、8/16bit、关联/零 alpha、LinearSrgb/ScRgb、代理边缘、同 Arc | 旧强制 alpha=255 是错误合同，改为明确保留/解除关联；不再测已删除的 u32 DIB |
| GDI resample/packed blend/GDI 字体/滤镜出图/旧性能 | 生产使用共用 native GPU renderer 与 Yue 文本；F6 原生读回检查 alpha/透明圆角，资源回归检查精度 | 旧 GDI 算法单测和 ignored 性能出图工具退役；不能据 GPU probe 宣称滤镜视觉/字体质量已人工验收 |
| 创建/失败/取消/重复/迟到回执 | 首次 paint 消费、失败和取消不消费；claim Drop 恰好一次；double click、hide/raise、undo | 原 manager receipt 组合测试已补成真实 GPUI 窗口回归 |
| native 宿主创建/销毁/重建 | 新精确 opt-in probe 创建生产 F6、发 own-HWND 滚轮、检查样式/前台/FP16/alpha/资源释放 | 不使用 SendInput，不读取桌面/写剪贴板/移动光标/改变系统 HDR |

测试总数发生变化是被替代 GDI 线程/算法/API 的退役，不是修改 baseline 门禁。修改前 Pin headless 76 passed / 4 ignored；当前 ordinary 59 passed，headless 71 passed / 1 ignored，native 另显式执行 1 passed。原有 58-test Pin 下限不降，新 ordinary 实际 59，门禁通过。

### 16.3 已执行验证

| 验证 | 结果 |
| --- | --- |
| workspace ordinary lib，串行、隔离依赖 | App 275、Capture 568、Clipboard 13、History 43、Model 31、Pin 59、UI 221，共 **1,210 passed**；ignored 分别 5/88/2/0/0/0/0，不计入通过 |
| App / Pin headless lib | **295 / 71 passed**，另 5 / 1 ignored |
| Pin all-targets strict Clippy | headless feature、`--no-deps -D warnings` 通过，包含 preview example；不是全仓 strict lint 结论 |
| 原生生产 F6（Debug / Release） | 精确 filter 各 **1 passed**；原位(16,16)、NOACTIVATE/TOOLWINDOW/TOPMOST、无 LAYERED/APPWINDOW、前台不变；FP16 bits/源 Arc 保留，GPU corner alpha≈0/center≈0.5，滚轮1.1且资源复用，关闭后 Weak 释放 |
| framework 原生窗口 | 两个独立进程各 **1 passed**：持续无激活及 synthetic DPI 保物理尺寸；禁用 preserve 分支的负对照先失败 |
| workspace all-targets check、scoped fmt、diff check、test-count gate | 均通过；gallery 既有 9 项 must-use warnings 保留 |

回归主要日志：`target/hdr-f6-{baseline,admission-red,wheel-red,ordinary-final,headless-final,native-final,workspace-final,app-headless-final,pin-clippy,check-final}.log`。框架窗口日志位于 Yue `target/hdr-f6-window-*.log`。继续使用 §12.3 的本机隔离依赖快照（叠加本轮窗口 API），不宣称另一任务的虚拟列表工作树已通过。

原生命令必须精确选择单个测试，并在独立进程执行：

```powershell
cargo test -p snapclip-pin --lib --features headless-tests screenshot::native_tests::native_hdr_surface_keeps_placement_alpha_focus_and_releases_its_render_image --locked --offline --config target/hdr-isolated.toml -- --ignored --exact --nocapture --test-threads=1
# Yue 仓执行；CorePath 仅在使用本机隔离快照时需要：
pwsh -NoProfile -File tools/test-gpui-window.ps1 -Native -CorePath D:/100_Projects/110_Daily/SnapClip/target/hdr-vendor-baseline/vendor/gpui-pre
```

**仍未验收**：F5/F6 全链路真实热键、实际鼠标拖动/菜单、DWM 闪烁、HDR 开/关和高光观感、不同显示器/真实 DPI、屏幕 45MP 和完整设备恢复；原生小窗与无头测试不能代替这些证据。Release、RSS/VRAM、20 次替换、空闲重绘仍单独记录，未完成前不开放 production HDR 捕获门禁。

## 17. 历史保真源、SDR 行缩略图与回放（P3-J，2026-11-04）

### 17.1 所有权与生产接线

SQLite v5 为 clips 保存真实 origin，capture_sources 与历史行在同一 transaction 中存储 CaptureSource（完整采集元数据和 ArtifactRef）。旧数据默认 clipboard；不存在的采集 DPI 明确为 0、monitor 为 None，不用当前窗口状态推测旧来源。

CaptureArtifactStore 校验 owned 目录、尺寸/编码、完整文件 hash 和母版 metadata。列表中的 image payload 仅是规范化的 PNG8 缩略图；它不能作为完整预览或复制来源。两条历史引用同一母版时，删除第一条仍保留文件，删除最后一条才回收；替换/篡改的文件不擅自删除。已经打开的窗口独立拥有 Arc 像素，不受历史删除影响。

缩略图随源 bands 生成，128×128 的线性光/关联 Alpha 累加器不会持有第二张大图。计算整幅峰值后，使用已有 Model/Yue BT.2390 映射为 SDR；隐藏透明色不污染边缘。列表读取/解码放在后台，最多两个在途任务；删除不提前释放任务名额，迟到结果不能复活已删除的行。capture 缩略图明确排除 OCR，避免误识别低分辨率派生内容。

保存面板归档未修改的原图，与输出尺寸/格式独立；同一 Picture 的 clone 共享一次成功归档，失败允许重试。文件保存成功而归档失败时显示“图片已保存，但历史原图未归档”，不丢失保存路径或谎称文件保存失败。归档成功后通过现有事件总线刷新列表。生产面板窗口选择 AutomaticHdr。

历史页新增“查看原图…”、“复制 SDR”和 HDR 来源的“复制 HDR PNG”。Space 查看、Enter 明确复制 SDR；读取/转换/复制在后台，操作中禁用重复启动。真正回放重新验证母版、按行读 FP16，PNG 源重新核对 hash；丢失/损坏/超预算不能回退缩略图。360 DIP 工具栏可折行。交互读取上限为 256 MiB；45MP FP16 超限会明确拒绝，原始保真源仍保留。历史的长母版有界导出 UI 尚未补齐，不以此宣称 45MP 交互已完成。

### 17.2 一项真实的旧精度损失

新增 2048×1 PNG16 用例在旧 normalize_png 上实际失败：读回变为 1920×1 PNG8（`target/hdr-history-png-red.log`）。现在 PNG 入库保留原始 bytes、尺寸、位深和 metadata，仍受 24MP/64MiB 预算约束；DIB 的既有 SDR 下采样策略未改。带 cICP/ICC 或 16bit 的预览走共享色彩 decoder，不能把 PQ 编码值直接当普通 RGBA8。真实生产 PQ PNG16→SDR 预览测试检查 Alpha 和映射像素，原始 bytes 不变。普通 PNG8 长图仍按顶部窗口流式解码，其内存合同没有换成完整解码。

### 17.3 已执行验证与未通过项

| 验证 | 实际结果 |
| --- | --- |
| workspace ordinary lib，串行、隔离依赖 | App 280、Capture 568、Clipboard 13、History 46、Model 31、Pin 59、UI 221，共 **1218 passed**；子进程重复项不另计 |
| App / Pin headless lib | **303 / 71 passed**，另 5 / 1 ignored |
| History 普通 lib + integration | **46 + 6 capture_history + 5 pixel_master passed**；重开 DB、原始 bits/generation、共享删除、外部/篡改拒绝、重复 ID、OCR 隔离 |
| 新 App 原图回放/入口 | 5 项通过；真实组件点击打开 Pin，360 DIP 按钮 bounds 在视口内、缺源不退缩略图、归档失败可重试 |
| 保存面板 UI | **22 passed**，包含缩放导出保留原始尺寸、归档失败仍返回已保存文件及原有布局/输入/IME/预览回归 |
| Model / History all-targets strict Clippy | `--no-deps -D warnings` 通过 |
| workspace all-targets check | 通过；gallery 既有 9 项 must-use warnings 保留 |
| 扩展 UI integration | 9 passed / 1 failed：既有设置页 `label-unit-suffix` 超出 640×480 可视范围，尚未把此项称为通过 |
| regression_baseline integration | 1 passed / 1 failed：A_CLASS 把已 ignore 的离屏 D2D GPU 测试列为普通测试；保留失败证据，后续修正层次与实际执行门禁 |
| App all-targets Clippy | 仍有存量诊断；未加 allow、未声称全仓 strict clean |

日志为 `target/hdr-history-{red,png-red,db-final,new-tests,replay-tests,save-ui,workspace,headless,check,clippy-final,ui-final,integration}.log` 和 `hdr-history-app-clippy.jsonl`。依赖隔离沿用 §12.3 本机快照，未混入 Yue 并行虚拟列表重构；快照/临时脚本/测试产物不提交。F6 Release probe 的新增成功证据为 `target/hdr-f6-native-release.log`。

尚未验证的真实 HDR/双屏/DPI、系统剪贴板接收方行为、设备恢复、RSS/VRAM/20 次替换/空闲重绘仍保持未验收；注册失败可能留下未注册母版（明确报错并保留源，暂不自动扫除）。生产 HDR 捕获门禁继续关闭。
