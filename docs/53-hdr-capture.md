# 53 · HDR 捕获、母版、保存面板与贴图接入方案

日期：2026-11-03。状态：**Yue 高精度资源／Windows 输出、本项目源像素／行带底座、冻结帧来源代次和压缩像素母版发布已实现；WGC 格式合同与 D2D 内部 FP16 链路已接通并有离屏 GPU 数值验证。生产捕获策略、滚动与消费迁移仍未完成，App／Capture 严格 Clippy 仍有失败，不代表 SnapClip 已支持 HDR。** Pin 已移除假色彩标签接口，为 16 位 SDR 明确生成 SDR 派生像素；仍是 RGBA8 模型，对 HDR 返回明确错误，原图不被截断或覆盖。分批修改与证据见 §8～§10，不能在后续消费链路未齐时仅打开 UI 中的 HDR 选项。

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
| `apps/snapclip/src/save_panel/` | 图片资源与编码结果仍是SDR入口 | 同一revision、色彩策略、编码后真实预览和明确格式限制 |
| `crates/snapclip-pin/src/source.rs`、`image_preview.rs`、`view.rs` | F3已走Yue；来源仍有8位限制 | 消费高精度资源，原始复制不走显示代理 |
| `crates/snapclip-pin/src/host.rs` | F6为DIB／`UpdateLayeredWindow`原生八位宿主 | 迁移到GPUI／Yue图片窗口，见§5 |
| `crates/snapclip-clipboard/src/writer.rs`、`windows/reader.rs` | 输出四字节像素／PNG，输入PNG优先后DIB | 明确复制策略，优先解析原编码PNG色彩，损坏时才用可说明的备用格式 |
| `crates/snapclip-history`及应用history入口 | 存储层已有压缩母版及有界行读取；历史 DB／重放／缩略图尚未接入 | 原始母版保留色彩，缩略图为可重建SDR派生内容 |

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
