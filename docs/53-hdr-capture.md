# 53 · HDR 捕获、母版、保存面板与贴图接入方案

初次设计：2026-11-03；更新：2026-11-04。状态：**源像素与母版、WGC/D2D 内部高精度链路、输出能力快照、生产滚动来源合同、HDR PNG16、保存面板策略、F3/F6 共用 GPUI/Yue、SDR/HDR 复制、历史保真源、有界导出与登记失败恢复已分批实现。** F6 旧 GDI 宿主已删除，Pin 保留 FP16/scRGB 原始 Arc；当前证据和边界见 §12～§28。Release 4K/45MP CPU 归档／导出及 20 次原生窗口释放／空闲渲染已验证；workspace 两种模式的 all-targets strict Clippy 已通过。AVIF 的 CPU 格式矩阵、有界 grid/Alpha/ICC 及真实 BandSource／SCPX 行带资格验证通过，包括 4K/45MP 逐行转换、来源变化／损坏拒绝和运行中取消；整帧预算仍失败，grid 仍有 4 MiB payload／5 MiB 文件限制，尚未生产接线。真实 HDR/双屏/DPI/设备恢复、VRAM 及端到端性能仍未验收；**生产 HDR 捕获门禁保持关闭，不代表完整支持 HDR。**

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
| `crates/snapclip-history`及应用history入口 | SQLite 源引用、SDR 行缩略图、真正原图回放/复制及有界导出已接入，见 §17、§19 | 打开／复制的交互预算 256 MiB；超限保留源并明确拒绝；长母版可独立流式导出，不能据此宣称可整图打开 |

`docs/49-avif-bmp-selection.md` 已区分早期研究与当前生产路径。当前 GPUI `SaveFormat` 不含 AVIF，也没有生产 `zenavif` 编码接线；§20 的门禁测试明确拒绝 AVIF，不能用旧研究结论、lock 文件中的包名或其他工具的能力代替生产验证。

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

进度解释：本清单保留产品级验收状态。§12–§22 已交付各项代码、分层测试及部分 Release 实测；不能仅凭无头、合成 DPI 消息或离屏 GPU 数值勾选全部完成。F6 旧宿主的删除与共用资源已在 §16 落地，历史有界导出见 §19，AVIF 明确关闭见 §20，性能与严格检查见 §21～§22。剩余工作按 §22 的真实环境矩阵推进。

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

尚未验证的真实 HDR/双屏/DPI、系统剪贴板接收方行为、设备恢复、RSS/VRAM/20 次替换/空闲重绘仍保持未验收；本节完成时注册失败仍可能留下未注册母版（明确报错并保留源，后续恢复已补于 §23）。生产 HDR 捕获门禁继续关闭。

## 18. 扩展回归暴露的既有问题（2026-11-04）

§17 的两处失败已继续处理，原失败日志保留：

- 设置页不是测试窗口太小，而是生产内容根本没有滚动容器。新增原生 GPUI 纵向滚动区域；测试保持 640×480，先断言底部开关不可见，再发真实滚轮事件、断言可见、点击并核对磁盘设置。不扩大窗口掩盖缺陷。补测 red（缺失滚动区域）见 `hdr-settings-scroll-red.log`，最终 UI integration **10 passed**。
- regression_baseline 的条目已落后源码：D2D 像素用例按 D-14 成为 opt-in，却仍列入普通测试；两条 hotkey 测试已被更完整的表驱动测试取代，清单仍指旧名。保留同一批行为和测试总条目，明确拆为 ordinary 与 GPU_A_CLASS，各自严格检查正确的 ignore 状态；hotkey 映射改为 `the_second_hotkey_is_registered_with_the_same_discipline`（实际断言 F5/F7/MOD_NOREPEAT）及 `each_conflict_message_names_its_own_key`（实际断言 conflict/failed 区分和每键文案），没有改生产热键或弱化现有断言。清单检查最终 **2 passed**。
- 冻结 GPU 用例另在真实 D3D11/D2D 离屏路径实际执行 **1 passed**，验证选区内原像素、选区外遮罩、hover 不染色及整窗 fallback。不把 opt-in 或清单存在当成像素测试已运行。

日志：`target/hdr-regression-{manifest-green,integration-final,gpu-explicit}.log`、`hdr-settings-scroll-{red,green}.log`。GPU 显式复现：

```powershell
cargo test -p snapclip-capture --lib windows::win::d2d::tests::an_element_preview_keeps_its_own_pixels_and_the_window_fallback_does_not --locked --offline --config target/hdr-isolated.toml -- --ignored --exact --nocapture --test-threads=1
cargo test -p snapclip-app --test regression_baseline --test ui --features test-support --locked --offline --config target/hdr-isolated.toml -- --test-threads=1
```

本机隔离配置仅用于剥离 Yue 并行虚拟列表改动，仍非产品默认构建配置。上述修复不代替剩余 HDR 实机和资源性能验收。

## 19. 历史原图的有界导出（2026-11-04）

历史工具栏新增“导出 HDR PNG…”／“导出 SDR PNG…”，使用系统保存对话框，取消后恢复可操作。导出直接从持久源读行带，不调用受 256 MiB 交互预算限制的整图回放，也不把缩略图当作原图。256 MiB 的打开／复制上限保持不变，导出和交互是不同的资源合同。

- `capture/pixel_export.rs::write_capture_source_png` 对 SCPX 核对完整 encoded hash、元数据及每块原始像素 hash，再有界读行；scRGB 生成 Rec.2020/PQ PNG16+cICP，SDR/relative 来源执行明确 SDR 策略。原生 SDR PNG 以 64 KiB scratch 流式复制并校验长度／fingerprint，保留原始编码字节。
- `history/capture.rs::export_source` 先用保存服务解析实际目标，防止去除文件名尾部点／空格后绕过同源保护。禁止覆盖历史保真源；工作线程承担验证和编码，UI 不同步读大文件。
- `save/service.rs::commit_stream` 共用现有名称校验、取消点、flush 与原子 rename。源损坏或编码失败丢弃本次 partial、保留旧目标；创建 partial 失败时不得清理其他写入者的文件。

最后一点修复了真实旧缺陷：旧实现把 `create_new` 放在带无条件错误清理的闭包内，遇到已存在的 partial 会误删它。将该片段恢复为旧实现后，新测试真实失败（`target/hdr-save-partial-red.log`）；恢复修复后保存服务 **15 passed**。历史消费测试 **8 passed**，覆盖 SDR 原编码相等、HDR 位深／PQ／Alpha／尺寸、损坏 SDR/HDR 不替代旧目标、规范化目标同源保护，以及 360 DIP 真实按钮→取消→再次导出→原图查看的组件交互。日志 `target/hdr-history-export-final.log`。

## 20. AVIF 生产证据门禁（2026-11-04）

本次检查的是当前实际构建的 GPUI 应用，并非旧文档或工具中 codec 名称的存在：

| 核查层 | 当前事实 | 产品决定 |
| --- | --- | --- |
| `snapclip-capture::save_format::SaveFormat` | 仅 PNG/JPEG/WebP/BMP；反序列化 `avif` 被拒绝 | 不提供虚假 AVIF 格式选项 |
| `save_panel::Export::encoded_bytes` | 保留 HDR 只有 PNG16；JPEG/WebP/BMP 必须选择 SDR | HDR 不被悄悄量化为八位 |
| App／Pin Cargo manifest | 没有 `zenavif`／`zenravif`／`rav1d-safe` 的生产依赖 | 旧 §49 研究结果不能直接复用为生产通过证据 |
| 当前 Windows App 依赖树 | `cargo tree -p snapclip-app -e features -i ravif` 输出 nothing to print；根 lock 有该包不代表 App 使用 | 不依赖 lock 中未启用包推导能力 |
| HDR AVIF 像素与标签 | 尚无生产 encoder→container→生产 decoder 的高位深、ICC/nclx、Alpha 回读证据 | 门禁关闭；不宣称 HDR AVIF 可用 |
| 许可与性能 | 尚无当前生产候选 feature 图、实际许可证及 4K/45MP 占用证据 | 不给 Yue 默认引入新 codec／许可义务 |

新增生产合同测试 `save_panel::tests::hdr_formats_are_gated_by_the_production_encoder_not_transitive_codec_dependencies`：真实调用当前 Export，PNG16 经当前 Pin 解码仍保留 >1 高光与透明度，所有非 PNG 格式明确拒绝保留 HDR，AVIF 配置明确拒绝，原始像素不改变。此项验证的是门禁，不是 AVIF 功能完成。

后续放行须同时具备：当前生产编码与解码的确定版本和 feature、可分发许可、自行生成的已知亮度／Alpha／元数据冲突小 fixture、真实数值回读、取消／损坏处理和 Release 峰值／耗时。未核实的本地 12 位 AVIF 不当作金标准；不为凑齐格式数量引入未经验证的依赖。

## 21. Release 有界处理、原生生命周期与回归（2026-11-04）

### 21.1 4K 与 45MP 的实际资源测量

`apps/snapclip/tests/hdr_resources.rs` 在独立 Release 进程中调用生产 `HistoryArtifactWriter.with_history` → SCPX/SDR thumbnail/数据库 → `write_capture_source_png` → `commit_stream`。输入由逐行合成器生成，测量前后都不分配完整整图；包含负 scRGB（位于 Rec.2020 色域内）、>1 高光和彩色透明像素。用全局 allocator 记录所有 Rust 堆分配，另每 5ms 采样进程 working set，查询失败直接失败。

| 尺寸／阶段 | 耗时 | Rust 堆增量峰值 | 最大单次分配 | 采样 working set 峰值 | 完整 FP16 整图分配 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 3840×2160 归档＋缩略图＋DB | 413.843ms | 6,996,428 B | 4,177,920 B | 21,278,720 B | 0 |
| 3840×2160 母版验证＋PQ PNG16＋原子提交 | 2440.041ms | 14,117,959 B | 4,177,920 B | 25,243,648 B | 0 |
| 8256×5504 归档＋缩略图＋DB | 2184.606ms | 7,046,092 B | 4,161,024 B | 21,295,104 B | 0 |
| 8256×5504 母版验证＋PQ PNG16＋原子提交 | 11220.333ms | 14,151,353 B | 4,161,024 B | 27,803,648 B | 0 |

4K 原始 FP16 为 66,355,200 B，母版 17,209,055 B、PNG16 5,194,342 B。45MP 原始为 363,528,192 B，母版 91,643,392 B、PNG16 17,209,068 B。测试同时断言原始末行 bits/generation 保留、完整尺寸、16 位深和 cICP `[9,16,0,1]`，删除历史后释放源。两种尺寸的两阶段均受同一 32 MiB Rust 堆增量／16 MiB 单次分配硬断言约束，均通过。

这是单次合成梯度、实际 Release 编码路径的结果，未经冷热缓存和系统负载控制，不表示真实照片吞吐、SDR/HDR 性能对比或完整应用总内存；5ms working set 采样也不等于系统记录的绝对峰值。SQLite/C/驱动占用不属于 Rust allocator 计数，working set 不包含专用 VRAM。

首次色带含不可用 Rec.2020/PQ 表达的负色，导出真实拒绝，失败日志保留于 `hdr-resources-unrepresentable.log`。没有通过裁剪源或放宽颜色校验让测试变绿；另补 `unrepresentable_hdr_export_fails_without_clipping_or_replacing_the_original`，断言保真母版仍能读回原 bits、旧目标不被替换、partial 清理。性能 fixture 改为可表达的负 scRGB 后完成整条导出。

```powershell
cargo test -p snapclip-app --test hdr_resources --release --locked --offline --config target/hdr-isolated.toml hdr_4k_archive_and_export_are_bounded -- --ignored --exact --nocapture --test-threads=1
cargo test -p snapclip-app --test hdr_resources --release --locked --offline --config target/hdr-isolated.toml hdr_45mp_archive_and_export_are_bounded -- --ignored --exact --nocapture --test-threads=1
```

日志：`target/hdr-resources-{4k,45mp}.log`。

### 21.2 20 次真实窗口循环与空闲状态

`screenshot::native_tests::native_twenty_hdr_windows_release_sources_and_do_not_render_while_idle` 实际创建生产 F6 窗口、等待首次 paint 回执、关闭，20 轮均确认 `RenderImage` 与视图释放；undo 期间原始源按设计保留，销毁 manager 后原始 `Arc` 的 Weak 不再能 upgrade。先等待初始化稳定，再记录生产视图的实际 render 调用，空闲 500ms 为 **0**。

64×32 合成图、20 轮首次 paint 回执 P50 **31.124ms**、P95 **33.933ms**，含 10ms 等待采样精度，不是交互帧时间或 4K 首帧承诺。输出快照为 `LinearSdr`／`display_kind=Sdr`、`hdr_supported=Some(true)`、SDR white=80nit、显示峰值报告=400nit、scale=1.5；这只是当时驱动查询值，不代表测光。测试未切换 HDR、未读桌面、未写剪贴板或发送系统鼠标输入。它验证当前 SDR 输出上 HDR 源的窗口生命周期，不能替代 HDR 开启、跨屏或全部 DPI 档位。

```powershell
cargo test -p snapclip-pin --lib --features headless-tests --release --locked --offline --config target/hdr-isolated.toml screenshot::native_tests::native_twenty_hdr_windows_release_sources_and_do_not_render_while_idle -- --ignored --exact --nocapture --test-threads=1
```

日志 `target/hdr-f6-native-lifecycle.log`，**1 passed**；同一 native Application 每个测试独立进程。

### 21.3 扩大回归

- workspace ordinary lib 共 **1225 passed**（App 287、Capture 568、Clipboard 13、History 46、Model 31、Pin 59、UI 221），不重复计算子进程用例；App／Pin headless **310／71 passed**。
- App `ui` **10 passed**、`regression_baseline` **2 passed**；History `capture_history` **6 passed**、`pixel_master` **5 passed**；workspace all-targets check 通过。
- AVIF 生产门禁测试真实调用 Export 的结果与本节统计一起通过，不以文档代替执行；严格 Clippy 初次扩大检查仍暴露 UI 存量诊断，保留 `hdr-final-strict.log`，后续清理另记结果。

证据 `target/hdr-final-{ordinary,headless,ui,history,check,strict}.log`。未测的真实 HDR 开关／双屏／100/125/200% DPI／读屏／设备恢复、剪贴板真实接收方、VRAM、完整捕获到可见以及交互帧时间，继续保持未验收。

## 22. 严格检查收尾与重构后回归（2026-11-04）

### 22.1 改动范围与基线

`1ea5855` 已正常提交、推送；本批核心清理为 `dfe5a1a`，开发工具清理为 `54ea344`。Rust/Cargo 均为 1.98.1，host 为 `x86_64-pc-windows-msvc`。继续使用 §12.3 的冻结 Yue 依赖与 `target/hdr-isolated.toml`，不混入另一项尚在进行的虚拟列表重构。

本批没有改变 HDR 策略或放宽资源预算。改动包括：

- 用同一组 overlay 能力端口贯穿启动和 controller；大 worker 结果与滚动导出状态按所有权装箱，不复制完整像素。
- Dropdown 绘制直接读取自身 selected／hovered／focused，删除可相互矛盾的重复参数；保存参数直接构造既有 `PanelValues`，删除九个位置参数的纯转发函数。保存目录／名称测试保留原断言，并增加真实 `request_for` 的目录、展开名称与格式断言。
- 固定像素分组改用 `as_chunks`，可写分组保持 `iter_mut`；测量用例在 Debug 下仍必须 panic，不能为通过静态检查而把 Release 要求改成静默跳过。
- 审查自动修复涉及的 Win32 参数。`AddFontMemResourceEx` 的字体数量和 `SHBrowseForFolderW` 的结构写回保留真正可写存储，使用 `&raw mut`，不因 Rust 绑定的 const 指针类型而传入共享引用背后的可变存储。
- 编解码工具复用已有 `JpegOptions`，质量／抽样／trellis／Huffman／行带值逐项保留；图标烘焙仍由既有产物一致性测试守住。

测试函数清单与修改前对比没有删除项；没有新增 `allow`、`expect` 或 `ignore`，没有降低 lint 级别。`window_host.rs` 原有 NUL 字节保持，未为获得文本 diff 重写文件编码。

### 22.2 已执行的普通、无头与静态验证

| 验证 | 修改前证据 | 本批结果 |
| --- | --- | --- |
| workspace ordinary lib，串行 | §21.3：1225 passed | **1225 passed**；App 287、Capture 568、Clipboard 13、History 46、Model 31、Pin 59、UI 221；子进程重复项不另计 |
| App／Pin headless lib | §21.3：310／71 passed | **310／71 passed**，另 5／2 ignored |
| App UI／回归基线 integration | §21.3：10／2 passed | **10／2 passed** |
| History capture_history／pixel_master integration | §21.3：6／5 passed | **6／5 passed** |
| Model pixel_contract／Capture source_contract | 原有像素／来源合同和断言保留 | **6／6 passed** |
| App png_params_probe | 原有流式编码／像素回读断言保留 | **2 passed**，另 1 项参数性能测量保持显式 opt-in |
| codec-bench／icon-bake | 原有测试和断言保留 | **28／7 passed**，另 2／0 ignored |
| workspace all-targets check | §21.3 通过，含 gallery 既有 warnings | **通过，0 warnings** |
| workspace all-targets strict Clippy，普通／headless | §21.3 仍失败 | 两种模式均 **通过，0 warnings**；都使用 `--no-deps -- -D warnings` |
| fmt／diff | 原门禁保留 | `cargo fmt --all -- --check`、`git diff --check` **通过** |

主要日志：`target/hdr-clean-{ordinary,headless,ui,history,tools,pixel-contract,source-contract,png-probe,check,strict,strict-headless,fmt,diff}.log`。UI integration 初次链接遇到 D 盘空间不足；只将本轮已结束测试及失败链接的 PDB 移到 C 盘临时目录保留，再执行成功，没有跳过测试或降低 debuginfo。

复现静态检查（去掉 `--features` 即普通模式）：

```powershell
cargo clippy --workspace --all-targets --no-deps --features snapclip-app/test-support,snapclip-pin/headless-tests --locked --offline --config target/hdr-isolated.toml -- -D warnings
```

### 22.3 原生数值与生命周期复测

- D2D HDR 四项、D3D FP16 精确 ROI 一项、HDR→SDR sampler 一项，合计 **6 项 GPU 数值测试通过**，只使用自己的离屏资源。
- Release 的 20 次原生 F6 窗口循环 **1 passed**，无释放失败，稳定后空闲 500ms 的 render 次数仍为 **0**。64×32 合成图首次 paint 回执 P50 **30.996ms**、P95 **40.620ms**；包含轮询精度和调度时间，不能解释为 4K 帧时间、不能据两次单轮测试宣称性能改善或回退。
- 当时输出快照仍为 SDR／LinearSdr、scale=1.5，驱动报告 HDR supported、80nit SDR white／400nit peak。没有切换系统 HDR、读取桌面、写系统剪贴板或发送系统输入，不能把此结果当作真实 HDR 输出验收。

日志：`target/hdr-clean-{d2d,readback,sampler,lifecycle}.log`。原生命令与 §21.2 相同；每个 GPUI Application 继续独立进程运行。

Release 资源测量也重新构建、逐项独立进程执行，各 **1 passed**。输入、预算及命令与 §21.1 相同：

| 路径 | 本轮耗时 | Rust 堆增量峰值 | 最大单次分配 | 采样 working-set peak |
| --- | --- | --- | --- | --- |
| 4K 归档＋缩略图＋DB | 415.887ms | 6,996,428 B | 4,177,920 B | 21,282,816 B |
| 4K 验证母版＋PNG16＋原子提交 | 2464.186ms | 14,117,959 B | 4,177,920 B | 25,440,256 B |
| 45MP 归档＋缩略图＋DB | 2190.801ms | 7,046,092 B | 4,161,024 B | 21,315,584 B |
| 45MP 验证母版＋PNG16＋原子提交 | 11153.244ms | 14,151,353 B | 4,161,024 B | 28,135,424 B |

四条路径的完整图像分配次数均为 **0**，Rust 堆与最大单次分配值同 §21.1；各项耗时与上一轮单次值相差约 1% 以内。它们是同一合成梯度 fixture 的有界 CPU 行带证据，不代表真实照片吞吐、应用总内存、VRAM、4K 首帧或端到端性能。日志：`target/hdr-clean-{4k,45mp}.log`。

### 22.4 剩余产品级验收

以下工作仍未验收；已有内部实现和上述测试不能代替真实环境证据。生产 HDR 捕获门禁继续关闭，AVIF 继续不启用。

| 剩余项 | 必须记录的操作与结果 |
| --- | --- |
| HDR／SDR 和 SDR white 变化 | 使用同一已记录 hash 的 HDR 源，实屏 HDR 关／开、同窗切换、不同白点；核对 UI 白、真实高光、Alpha、源内容不变及失败原因 |
| 双屏／DPI | SDR↔HDR 跨屏，100／125／150／200% 真实缩放；核对原位 F6、预览相机、弹层、鼠标拖动及焦点，不使用合成 DPI 消息代替 |
| 完整采集工作流 | F5 捕获→冻结→标注／圆角→保存／历史→F6，以及滚动正反增长、取消与来源状态变化；记录实际 WGC FP16 与源 generation |
| 设备与驱动恢复 | 真实设备／输出变化和恢复，核对资源重建、来源旧结果拒绝、错误反馈；不以调用一个测试用 reset 等同于设备丢失 |
| 系统接收方与可访问性 | 用户确认真实剪贴板接收应用，分别验证 SDR／HDR 格式优先级；读屏实际播报和焦点操作。此类测试会操作用户环境，不由普通测试自动执行 |
| 端到端与 GPU 预算 | Release 的 4K／滚动长图／45MP／多贴图，冻结到可见与交互 P50／P95、VRAM／进程私有字节、20 次替换后 GPU 残留；CPU 行带测量和小窗生命周期不构成这些指标 |

验收前记录 Windows build、GPU／驱动、显示器／HDR 状态、DPI、文件 hash、构建提交及每条命令结果；缺少硬件条件的格子明确写未测。只有矩阵满足后，才另行决定是否开放生产 HDR 捕获。

## 23. 无需实屏的历史登记失败恢复（2026-11-04）

### 23.1 问题与所有权

§17 的已知缺口是两个持久存储之间没有交接记录：PNG／SCPX 已发布，SQLite 登记失败后只有错误，后续无法发现该母版。SQLite 内部事务已经正确，删除原图或再造一套垃圾回收策略不能解决这个问题。

`snapclip-history::store::capture_recovery` 在既有单数据库写线程中增加有界的待登记记录：

1. 先按原合同校验源文件归属、fingerprint、尺寸／位深／元数据及真实 SDR 缩略图。
2. 在第一次 SQLite 查询前，把 `version/id/source/thumbnail` 写到 `artifacts/pending-captures/<blake3(id)>.json`；临时文件 `create_new`、写完 `sync_all` 后 rename。记录不包含完整源像素，读取上限为 5 MiB，检查文件类型、版本及文件名身份。
3. 登记事务失败或首次查询失败，保留母版和待登记记录并报错；App 的错误包含保留原图的实际路径。
4. 下次 `Store::open` 逐份校验后重试；仍失败或损坏／越界／冲突的记录保留并记录错误，不阻止其他有效记录恢复。相同 id+不可变源幂等，不能替换已存在的其他源或普通 publication。
5. 只有提交成功后清理对应记录；提交后崩溃留下的记录与已存在的相同 source 对账后清理，不新增重复行。显式删除历史前也必须清理匹配记录；清理失败则拒绝删除，避免下次启动把用户删掉的条目恢复出来。

不自动扫描或删除没有有效交接记录的旧母版，不推导新的 LRU／保留天数，也不清理别的写入者的 `.tmp`。该合同覆盖**完成验证且成功写入交接记录后的登记失败**；源发布与交接落盘之间被强制终止、交接目录本身不可写、验证失败或未完成的 `.tmp`，仍只保留源并明确报错，不能宣称跨文件系统／SQLite 的完整断电原子事务。旧开发期的孤立文件也不会擅自导入或删除。

### 23.2 修改前后验证

修改前 History lib／capture_history／pixel_master 为 **46／6／5 passed**。先增加恢复测试，真实得到 **1 passed、4 failed**；继续审查发现 SQLite 首次查询失败仍可能先于记录写入，先补测试再次真实失败，再把交接提前到第一条 SQL 前。

最终 `capture_recovery` integration **9 passed**，覆盖 SDR＋HDR 失败后重开、持续失败不丢记录、提交后崩溃对账、删除不复活、清理失败拒绝删除、相同请求幂等、重复 id 不替换原图、外部／篡改来源、损坏／超限／错名记录、临时文件所有权，以及首次数据库查询失败。原 **46／6／5** 继续全部通过，合计 **66 passed**。App writer **9 passed**（新增 1 项），实际读取错误中指明的母版，确认 metadata／FP16 原始 bits 保留。

```powershell
cargo test -p snapclip-history --lib --tests --locked --offline --config target/hdr-isolated.toml -- --test-threads=1
cargo test -p snapclip-app --lib capture::artifact_writer::tests --locked --offline --config target/hdr-isolated.toml -- --test-threads=1
```

证据 `target/hdr-recovery-{baseline,red,read-red,final-history,app}.log`。没有删除既有测试、放宽断言、新增 allow／ignore 或操作真实显示／剪贴板。

## 24. AVIF 的软件资格验证及确定的数值阻塞（2026-11-04）

**接续状态：本节保留首次失败基线；Identity 解码和新发现的彩色编码／ICC 顺序问题已在 §26 修复并回归。数值门槛通过，整帧资源门槛仍失败，AVIF 未生产接线。**

§20 的“尚无候选证据”已继续推进，范围仅为 CPU 探针，**不是 AVIF 生产功能完成**。详细固定版本、feature、官方包 hash／许可证声明、输入、数值门禁、来源与复现步骤见 [HDR-AVIF.md](../tools/codec-bench/HDR-AVIF.md)。

- 仅 `codec-bench` 的 opt-in `hdr-avif-probe` feature 增加 `zenavif =0.1.6`。App 的实际 normal 依赖图没有候选包，Yue／Pin／Capture 不增加 codec 接线。更新 lock 时保留原包版本，没有夹带 GPUI 或其他已有依赖升级。
- 自行生成 32×16、10 位 Rec.2020/PQ Identity/RGB、straight Alpha 码值阶梯；标签、ICC 回读、编码／解码开始及第三次检查处取消、损坏／截断／像素上限均按探针预期执行。
- **数值失败**：黑色 `[0,0,0,32800]` 被候选解码为 `[0,34721,0,32800]`；最大通道误差约 **541.994/1023**，超出事先设定的 **2/1023**。同一文件通过 FFmpeg 8.1.1 独立解码，RGB 与辅助 Alpha 的误差都为 **0**。
- 已核对固定发布源码：managed 16 位解码把 Identity 平面交给 YUV 转换；矩阵映射对 Identity 落入 BT.601 默认值，缺少 GBR 直读路径。不能因为 HDR 标签正确就判定颜色正确，也不能把屏幕调参当成此问题的修复。
- 探针主命令和独立验证脚本**明确退出 1**，失败证据保留；探针自身两个参考／结构单元测试及 opt-in strict Clippy 通过。没有通过改变阈值、改用八位结果或静默忽略失败获得“通过”。

当前 serializer 发出 ICC 时替换容器 nclx，本轮 ICC fixture 不构成 ICC+nclx 双属性冲突测试；完整 12 位、彩色／Alpha 边界、YUV 矩阵／范围组合、运行中取消延迟及 4K／45MP AVIF 占用仍待验证。它们属于软件工作，不能混入缺少显示器的实机矩阵。先解决或更换这个已确认失败的解码路径，再谈生产接线与 AVIF 大图性能；当前不修改系统 HDR，也不开放 AVIF。

## 25. 本批普通／无头回归与 CPU 资源复测（2026-11-04）

### 25.1 实际执行结果

继续使用 §12.3 的冻结 Yue 快照隔离依赖，不混入并行虚拟列表工作。本批没有修改底座绘制、系统 HDR 设置或 GPU 策略，也没有运行真实窗口／设备恢复／剪贴板接收方测试。

| 验证 | 本批结果 |
| --- | --- |
| workspace ordinary lib | **1226 passed**；App 288、Capture 568、Clipboard 13、History 46、Model 31、Pin 59、UI 221；子进程重复项不另计 |
| App／Pin headless lib | **311／71 passed**，另 5／2 ignored；相对 §22 App 新增 1 项母版登记失败路径回读 |
| App UI／regression_baseline | **10／2 passed**；是无头交互，不是实屏外观证据 |
| History lib／capture_history／capture_recovery／pixel_master | **46／6／9／5 passed**，合计 66 |
| codec-bench／icon-bake 默认工具 | **28／7 passed**，原 2／0 ignored 保留 |
| AVIF 探针单元／显式 feature strict Clippy | **2 passed**／通过；主资格验证仍因真实数值超预算**失败** |
| workspace all-targets check | 通过，0 warnings |
| 普通／headless workspace all-targets strict Clippy | 两组均通过，使用 `--no-deps -- -D warnings` |
| 格式／空白 | `cargo fmt --all -- --check`、`git diff --check` 通过 |

没有删除现有测试或新增 allow／ignore。日志 `target/hdr-recovery-final-{ordinary,headless,ui,history,tools,check,strict,strict-headless,fmt,diff}.log`；AVIF 日志与失败定义见 §24 和工具报告，不能把它与通过的生产回归合并宣称全部绿色。

### 25.2 恢复交接后的有界性能

使用 §21.1 相同的生产 `HistoryArtifactWriter.with_history` → 源母版／SDR thumbnail／DB → 原图验证／PQ PNG16／原子提交；包含本批交接文件写入和同步。两种尺寸分别在独立 Release 进程执行：

| 路径 | 本批耗时 | Rust 堆增量峰值 | 最大单次分配 | 采样 working-set peak |
| --- | ---: | ---: | ---: | ---: |
| 4K 归档＋缩略图＋DB | 422.222ms | 6,996,428 B | 4,177,920 B | 21,340,160 B |
| 4K 验证母版＋PNG16＋原子提交 | 2465.993ms | 14,117,959 B | 4,177,920 B | 24,236,032 B |
| 45MP 归档＋缩略图＋DB | 2206.614ms | 7,046,092 B | 4,161,024 B | 21,331,968 B |
| 45MP 验证母版＋PNG16＋原子提交 | 11026.975ms | 14,151,353 B | 4,161,024 B | 28,266,496 B |

四条路径完整整图分配均为 **0**，两项用例各 **1 passed**；原 32 MiB 堆增量和 16 MiB 最大单次分配断言保持。Rust 堆数值与 §22 相同；四项单次耗时相对 §22 变化约 -1.14%～+1.52%，不足以据此宣称吞吐改善或退化。日志 `target/hdr-recovery-{4k,45mp}.log`，命令同 §21.1。

这是合成行带的 CPU 归档／导出结果，working set 为 5ms 采样，仍不代表应用总内存、VRAM、真实照片吞吐、HDR 显示或端到端首帧。剩余软件工作是 §24 的 AVIF 解码数值／格式与资源资格验证；剩余实机工作仍按 §22.4 的矩阵推进，生产 HDR 捕获门禁继续关闭。

## 26. AVIF 根因修复、完整小图矩阵与大图资源证据（2026-11-04）

### 26.1 修改前后及边界

继续按无需实机的范围执行；没有启动真实产品窗口、切换 HDR／DPI、读取系统剪贴板或提交本地 HDR-IMAGES。修改在消费者开发工具及其隔离 codec 依赖，未触碰并行 Yue 虚拟列表／图片资源测试工作。

| 路径 | 真实失败基线 | 修复后 |
| --- | --- | --- |
| 10 位 Identity 解码 | 原黑色 `[0,0,0,32800]`→`[0,34721,0,32800]`；新增单元测试实际失败 | 8/10/12 位 GBR 直接读取，不进入 BT.601；原灰阶/PQ/Alpha 误差均 0 |
| 16 位 RGB/RGBA 编码 | 4K 回读最大误差 768.011/1023；彩色单元测试首像素 `[0,16400,32800]`→`[32800,0,16400]` | 交给 raw-plane 接口的顺序改为 G/B/R；两个彩色入口和独立 libdav1d 回读所有通道误差均 0 |
| ICC+nclx 双属性 | nclx 在前时 ICC 长度变成 0，nclx 在后时 ICC 保留；实际退出 1 | ICC 优先级不依赖 primary association 顺序，原 612 B ICC 与原始像素逐字节保留；matrix/range 继续取 AV1 位流 |
| 编码额外副本 | RGB16／RGBA16 先构造完整 RGB 数组，RGBA 另构造 Alpha 数组 | 改为有序迭代器；仍有完整输入与 codec 内部帧，不声称流式完成 |

没有改变原 `2/1023`、PQ 亮度 `max(0.02 nit,3%)` 门槛，没有删除旧断言或新增 allow/ignore。原灰阶 fixture/hash 保持不变；新彩色用例不跳过 Alpha=0 的隐藏 RGB，另验证两种编码入口的行 stride、padding 和输入不可变性。源码来自原固定发布包，补丁受仓库版本管理，不修改 Cargo registry。

`vendor/zenavif 0.1.6` 保留 AGPL／商业双许可，`vendor/zenavif-parse 0.6.2` 保留 MPL-2.0。原版本和其他依赖不变，lock 仅将这两个包转为根 patch；原包 SHA-256／本地补丁 hash／实际修改文件均登记在 `tools/codec-bench/hdr-avif-sources.json`，`check-hdr-codec-vendor.py` 校验 80 份文件和补丁，Windows checkout 保留原字节。`Cargo tree -p snapclip-app -e normal` 无这组候选包；MIT Yue、App／Pin 的默认生产依赖和 PNG16 格式门禁均不变。详见 [候选报告](../tools/codec-bench/HDR-AVIF.md)及 [vendor 维护说明](../vendor/SNAPCLIP-PATCHES.md)。

### 26.2 实际执行的数值／格式检查

- `hdr-avif-probe`：**7 passed**，包含 2 项复用 allocator 测试、原 PQ 参考和盒边界、真实失败后修复的灰阶／彩色，以及两个编码入口的 stride／输入所有权测试。
- `verify-hdr-avif.py`：原灰阶、RGB16 彩色、RGBA16 彩色三份独立 libdav1d 回读，RGB／辅助 Alpha 的最大码值误差都为 **0**；原 PQ 亮度误差 0。
- `verify-hdr-avif-matrix.py`：**48 组通过**，8/10/12 位 × Identity/BT.709/BT.601/BT.2020-NCL × full/limited × 有/无 Alpha，37×19 非对齐尺寸；独立 libaom 无损生成，每个像素都比较，统一 **≤2 个原生码值**。YUV 与数学参考、libdav1d+zimg 对照；Identity 原生 GBR 必须与输入完全相等，再显式范围归一化，避免把 zimg 对 RGB 的 full-range 假设当作 limited-range 转换。
- `verify-hdr-avif-metadata.py`：**3 项结构编辑器测试＋2 种双 colr 顺序通过**。真实修改 `meta/ipco/ipma/iloc` 的尺寸、关联和绝对偏移，不以任意子串或只有 ICC 的文件冒充双属性。
- 原有开始／第 3 个 check 的取消、损坏／截断／像素预算拒绝均保留并通过。CLI 用有界 4 MiB worker stack 解决 Windows 主线程 1 MiB 栈下 debug AV1 调用栈溢出，未用崩溃结果冒充颜色失败。
- 显式 feature all-targets strict Clippy `--no-deps -- -D warnings` 通过。锁定 magetypes 的弃用构造改成对应 `_t` API，没有屏蔽告警。

可复现入口 `tools/test-hdr-avif.ps1` 默认完成来源、单元、严格 lint、Release、独立编码像素、双属性与 48 组矩阵；`-Resources` 加两种尺寸测量，`-EnforceStreamingBudget` 执行原资源限额。隔离本轮的调用为：

```powershell
.\tools\test-hdr-avif.ps1 `
  -TargetDir C:/Users/seeyuer/AppData/Local/Temp/snapclip-hdr-avif-target `
  -CargoConfig target/hdr-isolated.toml

# 当前应明确失败，而非通过增加内存预算放行：
.\tools\test-hdr-avif.ps1 `
  -TargetDir C:/Users/seeyuer/AppData/Local/Temp/snapclip-hdr-avif-target `
  -CargoConfig target/hdr-isolated.toml -EnforceStreamingBudget
```

当前机器使用 FFmpeg 8.1.1 的 libaom/libdav1d/zscale。需要这组外部验证工具，缺少能力时明确失败，不退回同一候选库自证。数值日志 `target/hdr-avif-final-suite.log`；每组夹具、SHA-256 和误差记录在 `target/hdr-avif-{fixtures,matrix}/`。修复前证据分别为 `hdr-avif-encoder-red.log`、`hdr-avif-metadata-red.log` 及 §24 的灰阶反例。

### 26.3 Release 大图实际测量及未通过的门槛

两个独立进程生成色块图，每个像素检查 RGBA 回读，不读取用户大图。输入为完整 RGBA16 数组，4K **66,355,200 B**、45MP **363,528,192 B**；堆增量以这份输入常驻后为基线，不能把它排除后称作总内存。进程 peak working set 由父进程每 5ms 查询 OS 当前值／进程峰值，包含输入、分配器保留和取消运行。

| 尺寸／阶段 | 耗时 | Rust 堆增量峰值 | 最大单次分配 |
| --- | ---: | ---: | ---: |
| 3840×2160 编码 | 2373.711ms | 181,482,168 B | 18,837,504 B |
| 3840×2160 解码 | 102.727ms | 217,589,078 B | 66,355,200 B |
| 8256×5504 编码 | 12565.640ms | 912,643,264 B | 95,969,280 B |
| 8256×5504 解码 | 573.228ms | 1,176,053,968 B | 363,528,192 B |

两种尺寸全部像素最大误差 **0**；编码文件 **74,907／418,618 B**。完整进程 peak working set **298,115,072／1,554,968,576 B**。这不是照片压缩率、VRAM、真实截图或端到端应用吞吐。

10ms deadline 下，运行中编码／解码分别在 **67.723／50.770ms**（4K）及 **368.576／277.428ms**（45MP）返回取消，均至少有一次成功 check 后才触发取消。它们证明取消后返回错误，没有证明 10ms 内中断；managed AV1 frame 调用没有 Stop 参数，当前必须等该帧完成后的检查点。每行 GBR 转换的可取消性不能解决已经进入的整帧 AV1 调用。

因此，既有 **32 MiB 额外堆／16 MiB 单次分配**预算在两种尺寸都**失败**。`-EnforceStreamingBudget` 完成两种测量后明确退出 **1**，结果 JSON 中 `fits_streaming_budget=false`，没有放宽预算或把诊断程序退出 0 当成生产资源合格。数值通过和资源失败分开记录；§21／§25 的 PNG16 有界归档／导出预算未变。

### 26.4 相关既有功能回归及接续

冻结 Yue 的依赖配置保持原样，回归日志前缀 `target/hdr-avif-workspace-`：workspace ordinary lib **1226 passed**（App 288、Capture 568、Clipboard 13、History 46、Model 31、Pin 59、UI 221）；App／Pin headless **311／71 passed**；UI／regression baseline **10／2 passed**；History **66**；默认 codec-bench／icon-bake **28／7 passed**。普通和 headless 两组 workspace all-targets strict Clippy、第一方 fmt／diff 均通过，原 ignored 项保留。第三方原包中的 LICENSE／两个 examples 行尾空白及合法 unified-diff 空白 context 均逐项确认并保留原字节，用来源＋补丁校验而非全量清洗；没有增加 whitespace allow。没有把同一子进程或共享测试跨可执行程序重复计入生产测试总数。

**仍未完成的软件工作**：有界 AVIF tile/grid 或另一确定候选的实际高位深／ICC／Alpha 行带接线、资源和运行中取消验收，以及其后的生产导出／预览／原子保存合同。现有 API 的全图输入＋完整输出在空间上随像素数增长，加线程或改变 UI 无法消除这个事实；下一步须先证明可按有限 tile/band 处理，不能用隐藏继续运行的任务或丢弃结果宣称取消。

AVIF、生产 HDR 捕获入口继续关闭。真实 HDR／跨屏／DPI／设备恢复／实际剪贴板接收方仍按 §22.4 验收；本节已经完成的 CPU 格式矩阵与资源诊断不依赖实机，也不能替代那些显示层证据。

## 27. 有界 AVIF grid、Alpha 与构造阶段取消（2026-11-04）

本批只推进 §26 的 CPU 软件工作，不运行真实窗口、GPU、显示设置或系统剪贴板。它交付的是可复现的资格路径，未给 App／Pin 加 AVIF 保存格式，也未给 MIT Yue 引入候选 codec。

### 27.1 修改前基线与根因

- 新建实际的 133×71、2 行×3 列、每块 64×64 的 10 位颜色／Alpha grid。原 parser 把它读成 `(rows=6, columns=1, width=0, height=0)`，新回归真实失败，见 `target/hdr-avif-grid-before.log`。grid 描述是 item payload，不是 ipco 属性；从 ispe 猜网格无法表达边缘裁切。
- 仅修 parser 后，原 `decode_full` 仍把 10 位报告成 8 位；它同时保留全部 Frame 和转换后的 tile，并用 `alpha=None` 拼接。当前全图 API 改为复用逐块解码，返回实际序列／颜色元数据；新增 sink API 不创建完整输出。
- 256×256 tile 虽通过色块，噪声输入却在拒绝前达到 **34,114,090 B** 堆增量，超过原 **32 MiB**。保留失败记录 `target/hdr-avif-grid-256-before.log`，改为 **128×128**。不是抬高内存门槛。
- 45MP 的 2795 块索引曾在 10ms deadline 后才首次检查取消，约 53ms 返回、成功 check 为 0。新增 `ManagedAvifDecoder::new_with_stop`，将 stop 传给 parser 的构建和逐块索引；另外单独测索引完成后的 AV1 tile 取消，防止只取消解析就声称 codec 运行中取消通过。
- serializer 原来扫描整个文件寻找 `BAAD_F00D` 占位字节，会误改相同内容的 ICC／AV1 payload。现在只更新记录下来的 iloc 字段；同时支持主 grid 的 ICC+nclx、合并有序 dimg、检查几何／Alpha 配置及 item ID／32 位长度溢出。

### 27.2 当前资格合同

开发工具 `tools/codec-bench/src/avif_grid.rs` 每次生成一块原始 RGBA16，不先创建全图输入；`decode_grid_tiles(stop, max_tile_pixels, consume)` 每次只交付一块借用缓冲。对应 Alpha 独立解码、保持 coverage，边缘按真实输出裁切。每块在提交 codec 前核对 ispe、AV1 序列尺寸和像素上限；块的布局、位深和颜色定义必须一致。sink 错误或取消立即终止后续交付，调用方在返回成功前不得发布部分产物。

当前限定为 **10 位、Identity/RGB、Rec.2020/PQ、straight Alpha**；小 fixture 同时验证无 Alpha RGB16、ICC 保留和主图双 colr。8/12 位及其他矩阵的 48 组既有验证仍为单图格式矩阵，不升级为完整 grid 格式矩阵声明。

显式上限：128×128 tile、4096 块、64 Mi 像素、64 KiB ICC、**4 MiB 累计压缩 payload／5 MiB 完整压缩容器**。parser 默认仍为 1000 块；仅资格调用通过 `grid_tile_limit(4096)` 配置更细的工作单元。行列编码仍限制 1～255，不把 0 当成 256。压缩数据仍以受限 Vec 收集，超过上限明确失败，没有整帧回退。因此这条路径还不是任意大文件的流式 muxer，不能以其预算通过放行真实大照片或所有敌意元数据。

### 27.3 数值和异常验收

Rust 探针 **17 passed**（原 7 项保留，新增 10 项），覆盖描述读取、每像素只交付一次、RGB16／RGBA16、ICC、Alpha 边界、sink 错误、构造／tile 取消、块数及块像素限额、损坏描述／Alpha／截断、压缩限额拒绝、偏移占位字节出现在 payload 中，以及单图既有回归。

新渐变 fixture 对原图约有 **0.999039/1023** 的最大 RGB 误差；逐块重新封装为单图解码与完整 grid **逐字节相等**，Alpha 相对原图也相等。独立 FFprobe 识别两个 Tile Grid，各 6 条流；FFmpeg/libdav1d 解码全部 12 条流，再按 FFprobe 的画布与偏移合成，结果与候选 **逐字节相等**，Alpha 最大误差 0。两份 fixture 分别覆盖 PQ 和 ICC+nclx，ICC 原字节保留。原 **≤2/1023** 门槛未修改，原 48 组矩阵及 PQ 亮度门槛继续通过。

FFmpeg 的 `0:v:0` 只是一块，不是完整 grid；Alpha 组的 `stream_index` 是组内索引。脚本 `tools/verify-hdr-avif-grid.py` 显式处理两者，检查全部覆盖／无重叠，不将第一块或颜色流中的默认不透明 Alpha 当作全图成功。

### 27.4 Release 资源、取消与噪声拒绝

Windows x86_64 MSVC、Rust 1.98.1，仍用单线程 quality/alpha quality=100、speed=10。各尺寸在独立进程测量；完整原始图像输入／输出分配均为 **0**，每个有效 RGBA 像素都与数学生成源比较，误差 **0**。

| 尺寸／阶段 | 耗时 | Rust 堆增量峰值 | 最大单次分配 |
| --- | ---: | ---: | ---: |
| 3840×2160 编码 | 2338.725ms | 3,028,634 B | 655,360 B |
| 3840×2160 解码 | 91.198ms | 1,863,524 B | 259,072 B |
| 8256×5504 编码 | 12289.727ms | 3,349,996 B | 655,360 B |
| 8256×5504 解码 | 531.419ms | 3,394,253 B | 1,048,576 B |

编码文件 **90,583／494,201 B**；进程 peak working set **15,761,408／17,752,064 B**，包含压缩输入、codec、分配器保留及取消运行。父进程仍每 5ms 查询当前／进程峰值。这些是合成色块，不是照片压缩率、真实截图延迟或 VRAM；历史单次耗时不可直接推断吞吐改善。

10ms deadline 下，4K 的编码／含索引解码／仅 tile 解码在 **10.511／10.216／10.030ms** 返回；45MP 为 **10.527／10.126／10.016ms**。六种运行均先有成功 check，再返回取消；无继续运行的后台任务或丢弃结果。单次时钟测量不构成任意负载下的硬实时保证，AV1 内部解码仍只能在当前小块返回后响应。

高复杂度固定种子噪声使用相同 4K／45MP 几何。两次均只读取 **557,056** 个填充 tile 像素便明确拒绝 payload 超限，没有读取完整图像；堆增量 **13,454,926／13,564,820 B**，最大单分配均 **1,572,864 B**，耗时 **975.067／985.476ms**。这是“有界拒绝”，不是噪声图成功编码。成功和拒绝路径均保持原 **32 MiB 堆增量／16 MiB 单分配**硬门槛。

复现：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/test-hdr-avif.ps1 `
  -TargetDir C:/Users/seeyuer/AppData/Local/Temp/snapclip-hdr-avif-target `
  -CargoConfig target/hdr-isolated.toml `
  -Ffmpeg C:/A_Softwares/MSYS2/ucrt64/bin/ffmpeg.exe -GridResources
```

本轮在 Windows PowerShell **5.1** 实际执行，完整退出 **0**。默认入口也检查独立 grid；`-GridResources` 执行两种成功尺寸与两种噪声拒绝，缺工具／超预算／取消未被实际触发均失败。原 `-EnforceStreamingBudget` 仍测试并拒绝整帧，不改变含义。日志 `target/hdr-avif-grid-full-suite.log`、资源 JSON `target/hdr-avif-grid-resources/summary.json`；三份固定原包、91 份源文件与补丁均逐字节校验。

### 27.5 回归与下一批软件工作

冻结 Yue 的依赖配置不变。workspace ordinary lib **1226 passed**，App／Pin headless **311／71 passed**，UI／基线 **10／2**，History **66**，默认 codec-bench／icon-bake **28／7**；普通／headless 两组 all-targets strict Clippy、AVIF opt-in strict Clippy、第一方 fmt／diff 通过。保留原 ignored 项；合法 patch 的空白 context 与原包字节由来源校验核对，不新增 whitespace allow。此次未运行真实窗口、GPU、显示设置或剪贴板测试。

接续顺序：

1. 将资格工具的像素回调接到现有 `BandSource`／母版行带，明确 RGBA16 与 FP16/scRGB→目标编码的颜色合同；不能为接线重新整帧展开。此项后续已按 §28 完成开发工具接线，生产导出开关仍未开放。
2. 以磁盘后备或真正流写移除累计压缩 Vec 的 4 MiB 限制；sink、临时文件、取消及原子发布共用现有保存合同。不能直接增大上限代替有界设计。
3. 补高复杂度真实输入、敌意元数据的分配预算、更多合法 grid／Alpha 布局和端到端 ICC→scRGB；当前几类明确拒绝不等于通用 AVIF 兼容性完成。
4. 完成生产许可交付与导出／预览／错误／原子保存接线，再谈产品格式放行。真实 HDR／跨屏／DPI／VRAM 继续是独立实机验收。

本批缩小的是尚未完成的软件范围，**没有完成生产 AVIF 或完整 HDR 支持**。

## 28. AVIF 接入真实行带与 SCPX 母版（2026-11-04）

### 28.1 来源、颜色与内存合同

新增开发工具 `tools/codec-bench/src/avif_bands.rs`，直接消费既有 `snapclip_capture::ports::BandSource`，并用同一入口读取 `snapclip_history::pixel_master::PixelMaster`。没有新增产品 crate 或应用 AVIF 依赖。编码核心改为可失败的 tile provider；读取失败、来源变化、颜色超限或取消都会返回错误，不交付部分压缩结果。

- 输入必须是绝对 FP16/scRGB，1.0=80 nit；拒绝把 relative LinearSrgb 或 BGRA8 当作 HDR。
- 固定 source descriptor、generation 和总行数，每次读取前后及完成时核对。按请求尺寸验证紧密像素长度、有限浮点和 Alpha 范围；`PixelMaster` 仍校验原文件 fingerprint 和触及块的 hash。
- Model 新增零输出分配的 `rec2020_pq16_pixels`。PNG16 与 AVIF 共用同一转换：预乘 Alpha 只解除一次，Rec.709/scRGB 转 Rec.2020，再做绝对 PQ 编码，Alpha 始终为线性 coverage。范围外的色彩明确失败，不能裁剪源母版。
- 转换后的字节仅声明 Rec.2020/PQ/Identity/full-range，不附加已经不再描述这些像素的源 ICC。本批没有完成通用外部 ICC→scRGB 解码。
- 源读取单次请求最多 **1 MiB**；缓存最多一条 128 行 PQ16 色带，单分配严格小于 **16 MiB**。每个实际源行只读取一次用于 BandSource 转换；SCPX 底层可能为相邻请求重复解压同一 4 MiB 存储块。大图不整帧展开，不能把小 fixture 的整图比较推广为任意大图路径。
- 仍保留 §27 的 128×128 tile、4096 块、64 Mi 像素、4 MiB payload／5 MiB 容器限制。极宽图若超过色带单分配预算会在读像素前拒绝；没有抬高资源门槛。

取消在源读取前后、逐行颜色转换、tile 编解码及最终返回处检查。既有同步 `BandSource::read_rows`／SCPX 文件校验与单个存储块解压本身不能在内部中断；不能把行间检查声称为任意 I/O 的硬实时取消。

### 28.2 修改前后与独立数值验证

修改前 Model／History lib **31／46 passed**，AVIF 探针 **17 passed**；资格输入仅能按坐标合成 RGBA16，无法报告真实源读取错误。修改后 Model **32 passed**、探针 **27 passed**；原 17 项保留，新增 10 项验证真实行带／母版、边缘 padding、预乘与透明零值、来源 generation／descriptor／高度变化、读失败／错误长度／NaN／非法 Alpha／不可表达色、开始／读后／已有压缩块后的取消、损坏母版及 tile provider 错误。关闭和错误均保留原母版。

133×257 fixture 横跨 2 列×3 行 tile，含 80／1000／10000 nit、可表达的负 scRGB和彩色透明像素。从 BandSource 和 SCPX 读取生成的 AVIF **逐字节相等**，母版元数据／末行 bits／原文件逐字节保持。

`tools/verify-hdr-avif-bands.py` 读取原始 FP16 文件，用独立 PQ 公式与另写的色域矩阵计算参考值。两条路径均由 FFprobe 报告完整网格，再由 libdav1d 解全部 **12** 条颜色／Alpha 流；与候选解码逐字节相等。最大 RGB 误差 **0.433813/1023**，Alpha **0.507324/1023**；原 **≤2/1023** 门槛不变。独立 EOTF／逆矩阵重建后的 scRGB 最大误差 **0.017489**，在原两码值经 EOTF 和色域变换传播的误差包络内，负 scRGB 未被裁掉。这是 PQ10 量化导出，不是 FP16 无损往返。

共享颜色错误现在使用格式无关的 `outside Rec.2020/PQ range`。首次 workspace 回归暴露一条测试仍匹配 `outside HDR PNG`；只同步该文案断言，保留拒绝导出、旧目标逐字节不变、partial 清理和源 bits 断言。首次失败证据保留在 `target/hdr-avif-bands-before-error-wording.log`。

### 28.3 Release 资源与耗时

以下使用现有单线程 quality/alpha quality=100、speed=10；输入逐行生成，不预先分配整幅 FP16。每种尺寸在独立进程运行，仍执行 **32 MiB 额外堆峰值／16 MiB 单次分配**硬门槛。主流程包含真实 `CaptureArtifactStore::write_pixel_master`、`PixelMaster::open` fingerprint 校验、按行读取及 AVIF 编解码。仅小 fixture 独立比较会整图合成。

| 尺寸／阶段 | 耗时 | 额外堆峰值 | 最大单次分配 |
| --- | ---: | ---: | ---: |
| 3840×2160 行带编码 | 5178.072ms | 8,445,028 B | 3,932,160 B |
| 3840×2160 SCPX 写入 | 71.328ms | 4,661,642 B | 4,177,920 B |
| 3840×2160 母版打开并编码 | 5194.427ms | 10,901,130 B | 4,177,920 B |
| 3840×2160 逐块解码 | 111.932ms | 2,260,215 B | 487,134 B |
| 8256×5504 行带编码 | 27586.169ms | 15,518,984 B | 8,454,144 B |
| 8256×5504 SCPX 写入 | 373.917ms | 4,644,741 B | 4,161,024 B |
| 8256×5504 母版打开并编码 | 28577.646ms | 17,395,851 B | 8,454,144 B |
| 8256×5504 逐块解码 | 938.914ms | 5,566,099 B | 2,666,047 B |

两种尺寸分别完整检查 **8,294,400／45,441,024** 像素，RGBA 最大误差 **0.499519/1023**；文件 **487,134／2,666,047 B**。源读取最大请求 **1,044,480／990,720 B**；进程 peak working set **26,935,296／35,844,096 B**，包含准备、压缩数据和分配器保留。原有合成 grid 成功／噪声拒绝路径也重新通过，原堆门槛不变。

10ms deadline 在行带转换已开始后于 **10.273／10.604ms** 返回，两者都有真实源读取与成功 Stop 检查；另有真实 SCPX 读取后取消的确定性回归。它不包括把不可中断的 `PixelMaster::open` 当成可取消操作。耗时为单次 CPU 资格测量；首个 4K 阶段与 workspace 回归部分并行，不用于宣称吞吐改善。45MP 编码约 28 秒，**内存合格不等于交互延迟已合格**；生产延迟仍需剖析和端到端验收。

复现完整入口：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/test-hdr-avif.ps1 `
  -TargetDir C:/Users/seeyuer/AppData/Local/Temp/snapclip-hdr-avif-target `
  -CargoConfig target/hdr-isolated.toml `
  -Ffmpeg C:/A_Softwares/MSYS2/ucrt64/bin/ffmpeg.exe -GridResources -BandResources
```

Windows PowerShell **5.1** 入口退出 **0**，保留原 48 组矩阵、独立 grid/ICC、噪声拒绝并默认执行新的独立 FP16 行带检查；最后补入的 SCPX 读后取消回归另跑探针，最终 **27 passed**、opt-in all-targets strict Clippy 干净。日志 `target/hdr-avif-bands-full-suite.log`、`target/hdr-avif-bands-final-tests.log`，资源 JSON `target/hdr-avif-bands-resources/summary.json`。

### 28.4 回归及剩余软件合同

本批使用同一冻结 Yue 配置；workspace ordinary lib **1227 passed**（Model 多 1 项），App／Pin headless **311／71**，UI／基线 **10／2**，History **66**，默认工具 **28／7**。ordinary／headless all-targets strict Clippy、opt-in strict Clippy、fmt 和第一方 diff 均通过。保留原 ignored 项；未运行真实窗口、GPU、系统剪贴板或显示设置测试。

下一步仍是明确的软件工作：

1. 使用磁盘后备／真正流写替代压缩 Vec，移除 4 MiB 限制而不增加堆门槛；复用临时文件、取消和原子发布合同。
2. 验证高复杂度真实输入、敌意元数据分配、更多合法 grid／Alpha 布局和外部 ICC→scRGB；当前只保证所列合同。
3. 完成许可交付、生产导出／预览／保存错误接线与 CPU 耗时剖析，再讨论放行产品格式。真实 HDR／跨屏／DPI／VRAM 继续独立验收。

本批完成真实来源的开发资格接线，**未开放生产 AVIF 或生产 HDR 捕获**。

## 29. AVIF 共享能力、磁盘后备与显式产品接线（2026-11-04）

### 29.1 当前状态与修改前后

本节接续 §28；此前各节的“仅探针／未接产品／4 MiB”描述保留为对应批次的历史状态，当前以本节为准。App／Pin 已在**默认关闭的 `avif` feature** 下接线，仍是开发功能。生产 HDR 桌面捕获门禁继续关闭，CPU 结果不能替代真实 HDR、跨屏、DPI、设备恢复、VRAM 或完整端到端验收。本批未运行这些实机项目。

| 合同 | 修改前 | 修改后 |
| --- | --- | --- |
| 编码共用 | 开发工具持有 BandSource→tile 实现 | `snapclip-avif` 持有唯一 CPU codec；工具、保存面板、历史导出共用 |
| 模块方向 | BandSource 定义在 Capture，Pin 不能依赖它 | 纯像素 trait 移到 Model；Capture reexport 同一类型，Pin 不依赖 Capture |
| 压缩产物 | Vec 累积，4 MiB 超限拒绝 | 两个自动删除的临时文件保存颜色／Alpha，累计 payload 限 512 MiB，header 后顺序流写 |
| 外部 AVIF | 未接共享高精度预览 | 明确色彩定义的静态 AVIF 解码为 FP16，由原 YueImagePreview 渲染 |
| 预乘输入 | 包装层再次除 Alpha，半透明颜色偏亮 | 底层已返回 straight，包装层不重复解除预乘 |
| 产品保存 | PNG16 为唯一已接 HDR 格式 | 显式 feature 增加 PQ10 AVIF；保留原子提交、旧目标／源母版不变与 partial 清理合同 |
| 许可交付 | 只有候选包声明与 vendor 来源 | 固定 normal/build 闭包清单、原文、源码审计包及 SPDX；缺正文时拒绝作为完整发行包放行 |

`snapclip-avif` 是 Windows CPU 能力 crate，无 GPUI、Capture 或 Pin 依赖。App feature 同时启用 Pin 的同名 feature。MIT Yue 继续只处理明确颜色合同的图片资源，依赖图不引入 AGPL codec，也不新增第二套预览组件。`SaveFormat::Avif` 是可序列化格式值，不能代替运行时／编译时编码能力检查。

### 29.2 编码、保存与取消

固定 **PQ10／Rec.2020／Identity／full-range／straight Alpha**，单线程、quality=100、alpha quality=100、speed=10；这些参数没有为了通过验收而调整。PQ10 为量化导出，不能代替原始 FP16/SCPX 母版。输入仍必须是绝对 scRGB，复用 Model 的颜色转换；SDR 派生结果不能仅改标签导出为 HDR AVIF。

保持 128×128 tile、4096 块、每边最多 255 块、单次源读取 ≤1 MiB、最多 128 行转换缓存及严格小于 16 MiB 的单次转换分配。`GridImage::header` 只接收块长度，检查 item／偏移溢出，不创建完整 payload，也不要求目标可 seek。颜色／Alpha scratch 成功、异常、取消均自动删除；最终输出使用 64 KiB 缓冲写入既有原子提交服务。写入失败、空间不足、体积限制或取消不覆盖旧目标。scratch 与目标 partial 可能同时存在，磁盘峰值约为压缩 payload 的两倍加 header，**堆内存合格不代表磁盘零成本**。

保存面板在保留 HDR 时提供 `.AVIF`，明确提示 PQ10／编码较慢；切到 SDR 回到 PNG 并禁用 AVIF。历史页从已核验的 SCPX 行带导出，校验来源身份、元数据、几何与目标路径，不能覆盖源母版。大图文件导出不先创建完整压缩 Vec；交互预览必须回读完整压缩文件与 FP16 图像，有独立预算，不能套用流式导出的低内存结论。

预览变更或关闭时通过 `CancelOnDrop` 取消实际 codec，不只丢弃结果。开始、行带转换、tile、scratch 复制和返回处均检查停止信号；保留 `Cancelled` 类型并映射为 `Interrupted`。底层单个 AV1 frame、同步文件 fingerprint、块解压及 I/O 内部仍不可立即中断；没有宣称硬实时取消，也没有把使用 `Unstoppable` 的最终保存说成支持用户中途取消。

### 29.3 外部输入与交互预算

- 压缩输入 ≤64 MiB、输出 ≤24MP，parser memory ≤16 MiB；metadata ≤1 MiB、box ≤20000、嵌套 ≤8、items ≤8194、tiles ≤4096。先做 BMFF／ICC 预检，再进入 parser／CMS。
- ICC ≤64 KiB、tags ≤128、TRC 点数 ≤4096、mluc 条目 ≤128；matrix/TRC RGB ICC 由固定 moxcms 0.8.1 转 relative linear Rec.709，保留 `LinearSrgb`。Rec.2020/PQ/full RGB 定义转绝对 `ScRgb`；ICC 优先，不把其相对 SDR 白误作 80 nit。
- 明确拒绝未标色、HLG／未实现 HDR ICC 定义、LUT ICC、动画、gain/depth map、旋转／镜像／裁切、非方像素和不支持的 box／Alpha 布局，不按 SDR 猜测。该范围不构成任意 AVIF 兼容性声明。
- 包装层只做一次正确的 Alpha 处理。新增预乘回归修改前实际得到线性 R≈0.998，正确值应为 ≈0.214；修复后通过，失败日志 `target/hdr-avif-premult-before.log` 保留。
- 预览在 resize／encode **之前**拒绝超过 24MP 的输出，压缩 Vec 的写入也受 64 MiB 约束；两者不能等解码失败后才检查。最终文件保存沿独立的导出几何预算流写。交互路径仍持有完整 FP16 输出，24MP 即约 192 MB，另有压缩数据、codec 和上传资源，不能把约 9 MiB 的编码额外堆解释为整个预览的占用。

### 29.4 独立颜色、真实内容与大产物证据

共享 codec **12 项**测试覆盖 ICC／PQ、Display-P3 矩阵、8/10 位 grid、Alpha、预乘、异常元数据、取消、scratch 清理与 header 字节等价。旧探针 **27 项**保留。独立 libaom／libdav1d 的原 **48 组**8/10/12 位、矩阵、range、Alpha 组合同时通过实际共享解码器，而非仅通过开发工具的另一实现；另有 4 项构造性敌意输入拒绝并计量分配。

`verify-hdr-avif-real.py` 从用户提供的两份 45MP HDR／SDR AVIF 各裁取 160×96 的真实内容，显式恢复工具丢弃的色彩标签，独立无损重编码为 12 位，再比较 FP16；两份通过。它不是原始 ICC 的逐字节往返或整幅照片交互支持。两份原图在未展开完整像素前按 24MP 门槛拒绝，堆增量分别 **7,532,682／8,522,081 B**。用户图像不提交仓库。

保留独立 grid／ICC 与 FP16 BandSource／SCPX 全像素比较：RGB **0.433813/1023**、Alpha **0.507324/1023**、重建 scRGB **0.017489**，满足原两码值经传递函数和矩阵传播的包络，负 scRGB 保留。没有放宽原颜色、32 MiB 编码额外堆或16 MiB 单分配门槛。

固定种子高复杂度噪声现在能成功导出，不再受旧 4 MiB Vec 限制。FFprobe 校验整幅颜色／Alpha grid 的画布和块数；libdav1d 检查第一块原生 10-bit RGBA 最大误差 **2**。此资源用例没有宣称独立回读噪声大图的每个像素；全像素颜色／边缘比较由小 grid、行带及原矩阵承担。验证器曾先转 16-bit 再反算，因两次扩展舍入把原生差 2 报成 **2.013687**；现直接读 `gbrp10le`／`gray10le` 比较，原门槛仍是 2，另断言 full-range 扩展误差 ≤1 个 16-bit LSB。FFmpeg 默认 1000 streams 小于 4K 的 510+510 条 item 流，工具显式传入可信夹具的准确流数，不把工具默认拒绝当成格式损坏。

两种尺寸顺序在独立 Release 进程中测量，不与 cargo 构建／workspace 回归并行：

| 噪声尺寸 | 完整输出 | 总耗时 | 填充源 tile | 编码＋metadata＋scratch I/O | 额外堆峰值 | 最大单分配 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 3840×2160 | 63,762,570 B | 14337.949ms | 50.995ms | 14286.954ms | 9,311,844 B | 1,572,864 B |
| 8256×5504 | 349,436,016 B | 77083.455ms | 275.389ms | 76808.065ms | 9,330,124 B | 1,572,864 B |

两种噪声的第一块原生 RGBA 最大误差均 **2**，full-range 扩展最大差均 **1** 个 16-bit LSB。填充只占约 0.36%，主要时间在其余编码／metadata／I/O 阶段；该计时尚未拆分三者，不能把它当成 AV1 内核 profiling。45MP 高复杂度约 **77 秒**，仍不满足快速交互预期。没有用此前与编译并行的约 94 秒推断本批实现获得了性能改善。

共享行带／SCPX 路径也完成重新测量，源图逐行生成；最大单次分配仍为 **8,454,144 B**，两种尺寸的完整有效像素仍检查到 **0.499519/1023** 最大误差：

| 阶段 | 4K 耗时／额外堆 | 45MP 耗时／额外堆 |
| --- | ---: | ---: |
| 行带编码 | 5176.963ms／7,981,704 B | 27652.639ms／13,624,540 B |
| SCPX 打开并编码 | 5339.791ms／10,491,822 B | 28876.837ms／14,910,556 B |
| 逐块开发解码 | 112.980ms／2,260,215 B | 651.246ms／5,566,099 B |

这条逐块开发解码不持有完整输出，不能作为产品交互 decoder 的总内存。行带运行中取消在 10ms deadline 后分别于 **10.258／11.225ms** 返回，均已有真实读取与成功检查。旧 grid 的 4K／45MP 成功路径、索引／tile 取消与噪声超限拒绝对照继续通过。完整 PowerShell 5.1 入口以 `-GridResources -BandResources -SpoolResources` 实际退出 **0**；日志 `target/hdr-avif-final-suite.log`，机器可读数据 `target/hdr-avif-{spool,bands,grid}-resources/summary.json`。这些 CPU 测量不涉及显示器、GPU、VRAM 或真实截图首帧。

### 29.5 许可、源码和复现

`tools/check-hdr-codec-licenses.py` 固定 **122** 个 normal/build 包的声明、features、原 archive checksum、原许可文本与本地源码 hash。原包未带的正文仅从其固定官方 commit 补齐，来源记录在 `tools/codec-bench/license-texts/sources.json`。原文／archive／patch 均按原字节校验；本地 UTF-8 源码和继承许可使用明确的 LF 规范形式，审计包写出同样字节，避免 Windows CRLF 检出造成假漂移，实际代码变化仍失败。

**尚存发布阻塞**：`simd_helpers 0.1.0` 原包及固定上游源码仅声明 MIT，缺原始许可证正文，不能编造版权或用另一包正文替代。普通清单检查会明确打印 `RELEASE REVIEW REQUIRED`；`--bundle` 会生成源码／原文／补丁／SPDX 2.3 和 `NOT-RELEASE-READY.txt`，并**退出 1**。它是可检查的审计产物，不是完整合格发行包；还需补齐上游原文或重新选择并验证依赖，并完成整个应用的发行义务。未向第三方发送消息或发布内容。

```powershell
# 使用本地已冻结的依赖；正常开发可省略 CargoConfig／指定自己的 TargetDir。
powershell -NoProfile -ExecutionPolicy Bypass -File tools/test-hdr-avif.ps1 `
  -TargetDir C:/Users/seeyuer/AppData/Local/Temp/snapclip-hdr-avif-target `
  -CargoConfig target/hdr-isolated.toml `
  -Ffmpeg C:/A_Softwares/MSYS2/ucrt64/bin/ffmpeg.exe `
  -GridResources -BandResources -SpoolResources

python tools/check-hdr-codec-licenses.py --config target/hdr-isolated.toml
python tools/check-hdr-codec-licenses.py --config target/hdr-isolated.toml --bundle <审计目录>

# 显式开发功能；不启用生产 HDR 桌面捕获门禁。
cargo run -p snapclip-app --features avif
```

PS1 保持 UTF-8 BOM，入口在 Windows PowerShell 5.1 验证。pre-push 增加 feature 依赖方向、vendor 来源和许可清单门禁；不使用 `--no-verify`，不修改 Cargo registry 或升级第三方版本。AVIF 默认关闭、许可阻塞和显示层实机验收分别记录，不能互相替代。

### 29.6 最终相关回归与尚未验收项

仍使用 §12.3 的冻结 Yue 快照隔离并行组件改造，所有构建使用 `--locked --offline`，第三方版本没有升级。预览预算修复后重新跑产品测试及严格 lint，而非沿用修复前结果。

| 最终验证 | 实际结果 |
| --- | --- |
| workspace ordinary lib | **1239 passed**：App 288、共享 codec 12、Capture 568、Clipboard 13、History 46、Model 32、Pin 59、UI 221；子进程重复项不另计 |
| 默认 App／Pin headless | **311／71 passed**，原 5／2 ignored 保留 |
| 显式 AVIF App／Pin ordinary | **290／60 passed**，原 5／0 ignored 保留 |
| 显式 AVIF＋headless App／Pin | **313／72 passed**，原 5／2 ignored 保留 |
| App UI／regression baseline，带 AVIF＋test-support | **10／2 passed**；无头交互，不是实屏外观 |
| History lib／capture_history／capture_recovery／pixel_master | **46／6／9／5 passed**，合计 66 |
| 默认 codec-bench／icon-bake | **28／7 passed**，原 2／0 ignored 保留 |
| 普通／headless／AVIF＋headless＋probe all-targets strict Clippy | 三组均通过，`--no-deps -- -D warnings`，无新增 allow／ignore |
| fmt／第一方 staged 与 unstaged 空白 | 通过；vendor patch 的合法空白 context 保留原字节，另做来源校验 |
| 默认／AVIF 依赖方向及负对照 | 两组通过，App 负对照正确拒绝；默认 App 无 codec，feature Pin 不依赖 Capture |
| 固定 vendor／122 包许可与源码清单 | 通过；LF／CRLF 等价与实际源码变化识别均验证 |
| 审计包缺正文拒绝 | 实际退出 **1**，生成 NOT-RELEASE-READY；逐个核对本地源码 hash 和 SPDX 122 包，不当成完整发行通过 |

日志 `target/hdr-avif-final-{ordinary,headless,product,product-headless,ui,history,tools,strict,strict-headless,strict-product,fmt,diff,vendor,license}.log`，独立真实内容与原图拒绝 `target/hdr-avif-final-{real,product-matrix}.log`。保留修复前失败记录；没有跳过失败用例、降低颜色／内存门槛或把原 ignored 实机测试改成绿色。

当前已交付本节列出的软件能力与回归，仍不能宣称“完整 HDR／AVIF 产品已验收”：**缺失原始许可正文的发行阻塞未解除；45MP 高复杂度编码约 77 秒，低占用不等于低延迟，进一步 AV1／metadata／I/O 热点剖析与优化尚需独立工作；真实 HDR 亮度、跨屏／DPI、VRAM、设备恢复、系统剪贴板接收方及端到端性能仍按 §22.4 实机验收。** 默认关闭的开发 feature 不替代任何一项门禁。
