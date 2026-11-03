# 53 · HDR 捕获、母版、保存面板与贴图接入方案

日期：2026-11-03。状态：**Yue 高精度资源／Windows 输出和本项目第一批源像素／行带底座已实现并通过关联功能测试；WGC、D2D、耐久母版发布和消费迁移仍未完成，扩大 Clippy 检查也仍有失败，不代表 SnapClip 已支持 HDR。** Pin 已移除假色彩标签接口，为 16 位 SDR 明确生成 SDR 派生像素；仍是 RGBA8 模型，对 HDR 返回明确错误，原图不被截断或覆盖。第一批实际修改与证据见 §8，不能在后端尚为八位时仅打开 UI 中的 HDR 选项。

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
| `crates/snapclip-model/src/capture.rs` | `PixelFormat`只有BGRA8；`CapturePayload`仅PNG路径 | 像素布局与色彩定义分离；载荷能定位真实母版及编码原文件 |
| `crates/snapclip-model/src/artifact.rs`、`pin.rs` | 像素／编码图的合同传播到业务 | 原尺寸、精度、色彩、来源revision完整传递 |
| `crates/snapclip-capture/src/windows/win/wgc.rs` | 单帧、session及重建三处都固定BGRA8 | 一处共同的捕获格式策略，FP16全链路验证 |
| `windows/win/d3d11.rs` | 第一批已实现 typed ROI readback；上传／目标仍为八位 | 继续让冻结帧／上传／目标传播真实format及源色彩定义 |
| `windows/win/d2d.rs`、`windows/renderer.rs` | 覆盖层、模糊、滚动预览采用八位路径 | FP16／scRGB目标与D2D颜色处理；UI白、遮罩、放大镜同步 |
| `windows/providers.rs` | `FrozenFrame`与`read_bgra`等接口固化八位 | 返回带像素描述的帧／行带，不把FP16字节放进BGRA接口 |
| `ports.rs`、`artifact.rs` | `BandSource::read_rows`明确为tightly packed BGRA，圆角等按四通道u8 | 行带描述和操作按格式实现，删除含混命名及隐式降精度 |
| `scroll/`、`windows/scroll_worker.rs` | 原始字节存储已可按 FP16 行宽落盘；匹配／预览／生产 session 仍为八位 | 拼接几何用分析图，保存的行带保留原始精度并传播色彩 generation |
| `apps/snapclip/src/capture/`、`save/bands.rs` | PNG母版及转码当前以八位行为为基础 | 原始母版与导出分离，流式HDR编码及SDR转换 |
| `apps/snapclip/src/save_panel/` | 图片资源与编码结果仍是SDR入口 | 同一revision、色彩策略、编码后真实预览和明确格式限制 |
| `crates/snapclip-pin/src/source.rs`、`image_preview.rs`、`view.rs` | F3已走Yue；来源仍有8位限制 | 消费高精度资源，原始复制不走显示代理 |
| `crates/snapclip-pin/src/host.rs` | F6为DIB／`UpdateLayeredWindow`原生八位宿主 | 迁移到GPUI／Yue图片窗口，见§5 |
| `crates/snapclip-clipboard/src/writer.rs`、`windows/reader.rs` | 输出四字节像素／PNG，输入PNG优先后DIB | 明确复制策略，优先解析原编码PNG色彩，损坏时才用可说明的备用格式 |
| `crates/snapclip-history`及应用history入口 | 历史文件和缩略图可能混用 | 原始母版保留色彩，缩略图为可重建SDR派生内容 |

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

完成条件是端到端证据，不是某个enum新增成功。当前没有切换系统 HDR、清理开发缓存或提交推送；后续从冻结帧色彩定义和母版发布继续，不提前开放 HDR 捕获入口。

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
