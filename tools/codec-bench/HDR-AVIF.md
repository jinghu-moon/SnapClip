# HDR AVIF 候选的 CPU 资格验证

2026-11-04。**当前结论：CPU 像素和 48 组格式矩阵通过；有界 grid／Alpha／ICC 及真实 BandSource／SCPX 行带资格路径均已独立核对。整帧候选仍超预算；grid 的压缩文件上限与尚未完成的生产合同见文末和消费者方案 §27～§28，不接入生产。** 应用仍仅提供已验证的 HDR PNG16；MIT Yue 不新增这些编解码依赖。颜色正确、资源合格、真实显示验收是三个独立条件。

## 固定的候选与来源

只有 `codec-bench/hdr-avif-probe` 显式 feature 引入候选，默认工具和产品不启用。`Cargo.lock` 固定实际解析图，`zenavif` 关闭默认 feature、仅打开 `encode`；没有启用 `unsafe-asm`、`encode-asm`、`encode-threading` 或 `encode-imazen`。同一测试配置用单线程编解码，避免将线程数差异混入结果。

| 发布包 | 测试版本 | 包清单中的 SPDX 表达式 |
| --- | --- | --- |
| zenavif | 0.1.6 | AGPL-3.0-only OR LicenseRef-Imazen-Commercial |
| zenravif | 0.1.3 | AGPL-3.0-only OR LicenseRef-Imazen-Commercial |
| zenrav1e | 0.1.4 | AGPL-3.0-only OR LicenseRef-Imazen-Commercial |
| rav1d-safe | 0.5.7 | AGPL-3.0-only OR LicenseRef-Imazen-Commercial |
| zenavif-parse | 0.6.2 | MPL-2.0 |
| zenavif-serialize | 0.1.4 | BSD-3-Clause |

已实际请求 crates.io 的版本 API，将本地 `.crate` 的 SHA-256 与官方校验和逐项比较，并确认检查的源码与该压缩包内容相等；证据记录在 [hdr-avif-sources.json](hdr-avif-sources.json)。这里只记录声明和来源核对，不把它当成发行包已完成许可交付的证明。升级候选时必须重新验证，不能从上游 main 分支 README 推断已发布版本的 API。

本轮将 `zenavif 0.1.6`、`zenavif-parse 0.6.2` 和 `zenavif-serialize 0.1.4` 的已校验发布源码及原许可证存入 `vendor/`，通过消费者 workspace patch 使用，**未修改 Cargo registry 缓存**。版本和其他包保持不变；lock 移除这三个包原来的 registry source/checksum，工具新增两条可选直接依赖，官方原包 hash 仍保存在清单。补丁范围和许可见 [维护说明](../../vendor/SNAPCLIP-PATCHES.md)。`python tools/check-hdr-codec-vendor.py` 对原包、91 份源文件及三份本地补丁逐字节校验，拒绝未记录的新增／缺失／修改文件。

官方固定版本源码：[zenavif encoder](https://docs.rs/crate/zenavif/0.1.6/source/src/encoder.rs)、[managed decoder](https://docs.rs/crate/zenavif/0.1.6/source/src/decoder_managed.rs)、[channel scaling](https://docs.rs/crate/zenavif/0.1.6/source/src/convert.rs)、[container serializer](https://docs.rs/crate/zenavif-serialize/0.1.4/source/src/lib.rs)。

## 合成输入与门禁

[hdr_avif_probe.rs](src/bin/hdr_avif_probe.rs) 自行生成 32×16 灰阶，RGB 是 10 位码值 0、128、256、384、512、640、768、896，各占 4 列；Alpha 固定为 512/1023。输入用 `(code << 6) | (code >> 4)` 扩展到 u16，编码实际为 **10 位 AV1**，不是 16 位 AV1。Rec.2020/PQ、Identity/RGB、full range、straight alpha、quality/alpha quality=100、speed=10。

不读取或提交用户的 HDR-IMAGES，不依赖截图、GPU、HDR 开关、网络解码服务或系统剪贴板。生成物只写入显式指定的 `target` 目录。

检查：

- 高位深解码保持 RGBA16 布局和尺寸；来源标签为 9/16/0/full，Alpha 不被当成 PQ 通道。
- 所有通道回读误差不超过 2/1023；独立 ST 2084 EOTF 的亮度误差不超过 `max(0.02 nit, 3%)`。参考公式另有 0、1000、10000 nit 单元测试。
- 编码不改变输入；编码／解码分别在开始及第 3 次取消检查处停止。这里只证明这些取消点，不声称覆盖整帧解码中的实时中断延迟。
- 损坏数据、截断数据及 511 像素预算下的 512 像素输入明确拒绝。
- 通过 moxcms 生成 sRGB ICC 并回读。候选 serializer 在提供 ICC 后**替换**容器 nclx；探针检查实际发出的 ICC，不能把这份 fixture 当成同时存在 ICC+nclx 的冲突优先级测试。PQ 位流标签和 ICC 都能从 `ImageInfo` 观察到，如何解释冲突仍需产品合同。
- fixture 的 BMFF 检查只走其 `meta/iprp/ipco/colr` 结构并严格检查边界，不是产品 AVIF parser，也不搜索任意字节子串冒充元数据。

## 修复前的基线

Rust/Cargo 1.98.1、Windows x86_64 MSVC、Release：

| 检查 | 结果 |
| --- | --- |
| HDR／Alpha／尺寸标签 | 通过 |
| 开始与第 3 个检查处的取消 | 编码／解码均通过 |
| 损坏／截断／像素预算 | 均拒绝，通过 |
| ICC 编码与真实回读 | 通过；容器只有 ICC，未伪称双 colr 通过 |
| PQ 数值回读 | **失败**；最大通道误差约 541.994/1023，最大灰阶 R 亮度误差约 9009.888 nit |
| 独立 FFmpeg 8.1.1 | 同一编码文件的 RGB、独立辅助 Alpha stream 的最大码值误差均为 **0** |

最小反例：源 `[0, 0, 0, 32800]`，候选解码为 `[0, 34721, 0, 32800]`；FFmpeg 的 RGB 为 `[0, 0, 0]`，辅助 Alpha 为 `32800`。独立解码结果把问题收敛到候选解码路径，而非编码 fixture。固定版本 `convert_16bit` 将 Identity 矩阵交给 YUV 转换；`to_yuv_matrix` 对未列出的 Identity 使用 BT.601 默认值，缺少 GBR 平面直读路径。

生成的 `pq10-rgb-alpha.avif` 为 589 B，SHA-256 为 `13b49bc710b904d4eb70de12c98bc63e4bc834a7149ff55b784a0d764b91f345`；期望 RGBA64LE 为 4096 B，SHA-256 为 `8bdb32262110db5562b65574971a34c1dd4db3934bdcc20aded8517ab4534add`。FFmpeg 输出色彩 stream 时不自动合入此辅助 Alpha，验证脚本明确分开读取两个 stream，不能用默认不透明 Alpha 得出透明度正确的结论。

## 复现

普通构建不需要探针。数值、格式和严格检查可统一执行：

```powershell
.\tools\test-hdr-avif.ps1
# 独立进程测量 4K 和 45MP；不把测量成功等同于资源预算通过。
.\tools\test-hdr-avif.ps1 -Resources
# 执行原有 32 MiB 堆增量／16 MiB 单次分配门禁：当前必须失败。
.\tools\test-hdr-avif.ps1 -EnforceStreamingBudget
# 验证有界 grid、独立颜色／Alpha／ICC、4K/45MP 成功及噪声超限拒绝。
.\tools\test-hdr-avif.ps1 -GridResources
```

第一条通过；最后一条在完成所有颜色检查和两种资源测量后退出 **1**，明确记录资源超预算。需要支持 `libaom-av1`、`libdav1d` 和 `zscale` 的 FFmpeg；本轮使用 8.1.1，可用 `-Ffmpeg` 指定路径，没有所需能力会失败。源校验默认只读本地 `.crate`；缺原包时显式运行 `python tools/check-hdr-codec-vendor.py --download`，仅下载固定版本并比较原先记录的官方 SHA-256。

本轮为避开并行 Yue 重构，使用 `-CargoConfig target/hdr-isolated.toml`；D 盘空间不足，使用 `-TargetDir C:/Users/seeyuer/AppData/Local/Temp/snapclip-hdr-avif-target`，这不是产品默认配置。最终汇总 `target/hdr-avif-final-suite.log`；数值夹具、每组结果和 SHA-256 在 `target/hdr-avif-{fixtures,matrix}/`，独立进程测量在 `target/hdr-avif-resources/`。

## 已修复的三个根因

1. **解码 Identity 平面**：8/10/12 位的 G、B、R 按各自 stride 读取，直接生成 RGB/RGBA；有限范围按三个同样的 RGB 分量归一化，不使用 YUV 色差中心。高位深随后仍由原路径扩为 u16，Alpha 单独作为 coverage 附加。8 位 strip 入口对 Identity 使用此正确路径。每行检查取消；只有一份交错输出分配。
2. **编码 16 位彩色**：`encode_rgb16`／`encode_rgba16` 原来把 `[R,G,B]` 交给要求 `[G,B,R]` 的 raw-plane 接口。灰阶三个分量相同，掩盖了错误；4K 彩色回读和新增单元测试实际失败，首像素 `[0,16400,32800]` 被轮换为 `[32800,0,16400]`。现在传正确顺序，并直接传迭代器，去掉不必要的全图 RGB／Alpha 中间数组。既有全图输入与 codec 内部帧仍存在，不能声称已变为流式编码。
3. **ICC/nclx 属性顺序**：primary item 同时关联两者时，解析器原来取第一个颜色属性，nclx 在前就丢失 ICC。现在 ICC 对颜色管理的优先级不依赖排列顺序，AV1 位流的 matrix/range 继续控制原始样本解码。两种顺序均逐字节保留 ICC 和所有原始像素；此检查不等于生产侧已完成 ICC→scRGB 的 AVIF 接线。

没有改误差阈值、删除旧断言或用显示器参数掩盖数值错误。vendor 中原有的 40 处弃用 SIMD 构造改成锁定 `magetypes` 提供的对应 `_t` API，没有增加 lint allow；其余上游源字节保持不变。

## 修复后的 CPU 验证

| 检查 | 结果及范围 |
| --- | --- |
| 原 10 位 PQ 灰阶＋Alpha | 候选与独立 libdav1d 最大码值误差均 0；原 2/1023 和 `max(0.02 nit,3%)` 不变 |
| 独立回读两个彩色编码入口 | RGB16／RGBA16 均检查所有通道；透明、第一档 Alpha、半透明、不透明，不跳过 Alpha=0 的隐藏 RGB |
| 8/10/12 位格式矩阵 | **48 组通过**：Identity／BT.709／BT.601／BT.2020-NCL × full/limited × 有/无 Alpha；37×19 非对齐尺寸，所有像素误差 ≤2 个原生码值 |
| 12 位夹具来源 | 独立 libaom 无损编码，不把 u16 输入或用户的未验证大图称作 12 位金标准 |
| 独立矩阵参考 | YUV 用明确定义的 Kr/Kb 数学参考和 libdav1d＋zimg；Identity 读取独立解码的原生 GBR 并显式归一化，原生平面要求与输入完全相等 |
| 夹具范围有效性 | raw 输入和输出均明确 CICP/range；避免 FFmpeg 的 gbrp 包装强制 full-range，也避免默认元数据导致生成时发生隐式重映射 |
| 双 `colr` 冲突 | 两种真实 primary 关联顺序均通过；BMFF 长度／association／绝对 iloc 结构重建，3 项编辑器测试通过 |
| 编码 stride／输入所有权 | 不读取每行 padding，不修改原输入；RGB16、RGBA16 都有实际编码→解码断言 |
| 取消／损坏／尺寸预算 | 原有开始及第 3 次 check、损坏／截断／像素预算断言保留；大图追加运行中取消测量 |
| Rust 单元测试 | 7 项通过，其中包含 2 项复用 allocator 测试 |
| opt-in all-targets strict Clippy | `--no-deps -- -D warnings` 通过 |

`verify-hdr-avif-matrix.py` 中的 2/native-code 门槛对所有 48 组一致。有限范围 Identity 的独立解码平面验证与显式范围归一化写在脚本中，不假称 zimg 会替它扩展 RGB。`verify-hdr-avif.py` 分开读取色彩与辅助 Alpha stream；原 fixture 的灰阶 hash 不变，彩色错误无法再被灰阶抵消。

## 资源和取消边界

4K 和 45MP 各在独立 Release 进程运行，输入是合成的 Rec.2020/PQ、straight Alpha 色块，所有像素回读误差均 0；**不是照片吞吐或生产流式验收**。计数 allocator 报各阶段相对输入常驻基线的堆峰值和最大单次分配；父进程每 5ms 查询 working set／操作系统进程 peak，包括输入、分配器保留和取消运行。详细数字以 SnapClip `docs/53-hdr-capture.md` §26 为准。

初次修复后的 45MP 测量：编码约 12.35s，解码约 548ms，进程 peak working set 约 1.55GB。即使去掉 wrapper 的中间数组，codec 内部与完整输出仍需大帧。因此原 **32 MiB 额外堆／16 MiB 单次分配**门槛明确失败，不改阈值，不将资源阻塞归类成缺 HDR 显示器。

运行中取消在 10ms deadline 后触发；返回错误并不意味着 10ms 内响应。初测 45MP 编码／解码约在 358／271ms 返回，解码在当前 AV1 frame 结束后的检查点才能退出；managed raw decoder 没有传入 `Stop` 的接口。每行 GBR 转换中的取消不能反向中断已经进入的整帧 AV1 调用。CLI 的 debug AV1 栈帧还会超过 Windows 主线程默认栈，现用有界 4 MiB worker stack，不能将曾经的栈溢出当作颜色测试结果。

以上是保留的整帧失败基线。接续 grid 验证见下；生产 AVIF 开关继续关闭，PNG16 和 Yue 的 MIT 依赖边界保持原样。

## 有界 grid 接续（2026-11-04）

`avif_grid.rs` 使用 **128×128** 原始 tile 和借用式逐块 sink，不创建完整原始输入／输出。parser 读取实际 grid item 描述和对应 Alpha；managed decoder 在提交 codec 前核对块尺寸／位深，裁切边缘，并返回实际 10 位／4:4:4 信息。`new_with_stop` 把取消传到网格构建；索引和已开始的 tile 编解码分别实测。默认 parser 仍为 1000 块，只有资格调用显式设置 4096 块。

压缩数据仍有 **4 MiB payload／5 MiB 文件**上限，ICC 64 KiB；超过就明确失败。编码器和 decoder 只在这个资格合同内声明有界。本节完成时尚未接到真实母版行带，接续见下；生产原子保存仍未接线，也不是任意大 AVIF 文件流式方案。

修改前的 133×71、2×3 fixture 实际被猜成 6×1，修 parser 后又暴露 8 位元数据和 Alpha 丢失；这些路径均有回归。旧 serializer 的全文件占位符扫描改为只补已记录的 iloc，避免误改 ICC／压缩 payload。第三个 vendor 包也按固定原包＋最小补丁管理。

结果：Rust **17 passed**，原 48 组格式矩阵通过。新 PQ 与 ICC+nclx 两个 grid fixture 均经 FFprobe 取得实际布局、FFmpeg/libdav1d 独立解码全部 **12** 条颜色／Alpha 流并合成，结果与候选逐字节相等。相对原始渐变的 RGB 最大误差 **0.999039/1023**，Alpha **0**，保留原 ≤2/1023 门槛。第一条视频流仅是一块，脚本没有将它误认为全图。

| 成功路径 | 编码堆峰值 | 解码堆峰值 | 进程 peak working set |
| --- | ---: | ---: | ---: |
| 3840×2160 | 3,028,634 B | 1,863,524 B | 15,761,408 B |
| 8256×5504 | 3,349,996 B | 3,394,253 B | 17,752,064 B |

两图全像素误差均 0，最大单次分配分别不超过 655,360／1,048,576 B。10ms deadline 的编码／含索引解码／仅 tile 解码实际返回分别为 **10.511／10.216／10.030ms** 和 **10.527／10.126／10.016ms**；都有成功 check 后再取消，不代表硬实时保证。

初次 256×256 方案在噪声拒绝前分配到 **34,114,090 B**，原门槛真实失败，因而缩小工作单元。128×128 固定噪声在读取 557,056 像素后达到压缩上限并拒绝，4K／45MP 的堆峰值 **13,454,926／13,564,820 B**，最大单分配均 **1,572,864 B**。这是有界拒绝，不是噪声编码成功；没有增大 32 MiB／16 MiB 门槛。

`tools/test-hdr-avif.ps1 -GridResources` 在 Windows PowerShell 5.1 完整退出 **0**；指定 FFmpeg 路径时自动取同目录 FFprobe，也可显式 `-Ffprobe`。日志 `target/hdr-avif-grid-full-suite.log`；独立布局验证 `tools/verify-hdr-avif-grid.py`；资源 JSON `target/hdr-avif-grid-resources/summary.json`。完整基线、命令、既有 workspace 回归和后续工作统一见 [消费者方案 §27](../../docs/53-hdr-capture.md#27-有界-avif-gridalpha-与构造阶段取消2026-11-04)。

## 真实行带与 SCPX 接续（2026-11-04）

`avif_bands.rs` 接入现有 `BandSource`／`PixelMaster`，共用 Model 的零输出分配 `rec2020_pq16_pixels` 与 PNG16 颜色转换。只接受绝对 FP16/scRGB：预乘只解除一次，转换为 Rec.2020/PQ，Alpha 保持线性。转换后只声明 nclx，不把源 ICC 误贴到新的色域／传递函数上。来源 generation、descriptor、行数在读取前后检查；错长、读失败、NaN、Alpha／色彩超限与取消均返回错误，不返回部分 AVIF。

源请求最多 1 MiB，缓存一条 128 行转换色带，单次分配严格小于 16 MiB；极宽图明确拒绝。原 4 MiB payload／5 MiB 容器限额仍存在，未引入产品依赖。读取前后与逐行转换可取消，SCPX 同步文件 fingerprint／块解压内部还不能中断。

最终探针 **27 passed**（原 17 保留），Model **32 passed**。新增 133×257、2×3 颜色／Alpha grid 从两类来源生成相同字节；独立 FFprobe/libdav1d 解码每份全部 12 条流，与候选逐字节一致。独立源 FP16→PQ 比较 RGB 最大 **0.433813/1023**、Alpha **0.507324/1023**；EOTF＋逆矩阵重建 scRGB 最大误差 **0.017489**，满足原两码值传播的误差包络，负 scRGB 保留。不能把 PQ10 量化文件当成 FP16 无损母版。

| 路径 | 4K 堆峰值 | 45MP 堆峰值 |
| --- | ---: | ---: |
| 行带转换并编码 | 8,445,028 B | 15,518,984 B |
| SCPX 写入 | 4,661,642 B | 4,644,741 B |
| SCPX 打开、校验并编码 | 10,901,130 B | 17,395,851 B |
| 逐块解码 | 2,260,215 B | 5,566,099 B |

原 32 MiB／16 MiB 门槛通过；最大单分配 **8,454,144 B**。完整检查两图全部 RGBA，最大误差 **0.499519/1023**。45MP 编码约 **28 秒**，尚不能宣称产品延迟达标。工作集、取消边界与完整耗时见消费者方案 §28，不从堆计数推断 VRAM 或界面性能。

`tools/test-hdr-avif.ps1 -GridResources -BandResources` 已在 PowerShell 5.1 完整退出 0；最终 SCPX 取消回归另行执行，27 项和 opt-in strict Clippy 均通过。完整日志 `target/hdr-avif-bands-full-suite.log`，资源 `target/hdr-avif-bands-resources/summary.json`，独立检查 `tools/verify-hdr-avif-bands.py`。仍需磁盘后备压缩数据、外部输入／ICC 验证、许可交付及生产保存接线，详见[消费者方案 §28](../../docs/53-hdr-capture.md#28-avif-接入真实行带与-scpx-母版2026-11-04)。

## 共享产品 codec 与磁盘后备（2026-11-04）

接续结果见[消费者方案 §29](../../docs/53-hdr-capture.md#29-avif-共享能力磁盘后备与显式产品接线2026-11-04)。此前章节保留修改前的整帧失败和 4 MiB 有界拒绝，不表示当前共享编码器仍有同一限制。

- `snapclip-avif` 提供唯一的有界 BandSource／SCPX 编码与明确标色 AVIF→FP16 解码。BandSource 移到 Model，Capture 仅 reexport，Pin 不因此依赖 Capture。MIT Yue 保持 codec 无关，渲染继续用原图片预览组件。
- 两个自动清理的临时文件代替 payload Vec，累计压缩 payload 限 **512 MiB**；serializer 根据长度流写准确 header，随后顺序复制颜色与 Alpha。source、sink、磁盘失败和取消不交付成功结果，产品沿既有原子提交发布。scratch＋partial 的磁盘峰值可接近压缩量两倍，编码堆门槛仍为 **32 MiB／16 MiB**。
- App／Pin 的 `avif` feature 默认关闭。显式开启才提供 HDR PQ10 保存、历史 SCPX 导出与贴图解码；没有开放生产 HDR 捕获。保存不收集全文件 Vec，交互预览仍需完整压缩数据和 FP16 图像，单独限制 **64 MiB／24MP**，不能以流式编码数字声称预览总内存只有数 MiB。
- 外部颜色／Alpha 资格使用实际共享 decoder：原 **48 组**矩阵、4 项有界敌意拒绝、ICC matrix/TRC、PQ／SDR 和 8/10 位 grid。用户两份 45MP 原图按 24MP 上限明确拒绝，另以显式标色的 160×96 真实内容 12 位裁片独立回读，HDR／SDR 均通过。没有声称原图的完整 ICC／整帧交互支持。
- 真实预乘反例修改前线性 R≈0.998，正确应≈0.214。原因是底层已返回 straight，包装层又除 Alpha；共享解码器取消第二次操作并保留回归。
- 大噪声资源测试验证整幅 FFprobe grid 几何与第一块原生 10-bit RGBA，**不冒充全图逐像素独立验证**。原生误差门槛仍为 2；转 16 位再反算曾因扩展舍入得到 2.013687，现分别校验原生误差和 ≤1 个 16-bit LSB 的扩展误差。`-max_streams` 设置为可信夹具准确的颜色＋Alpha 条数，避免 FFmpeg 默认 1000 streams 截断 4K／45MP 网格。

复现全部 CPU 资格：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/test-hdr-avif.ps1 `
  -TargetDir C:/Users/seeyuer/AppData/Local/Temp/snapclip-hdr-avif-target `
  -CargoConfig target/hdr-isolated.toml `
  -Ffmpeg C:/A_Softwares/MSYS2/ucrt64/bin/ffmpeg.exe `
  -GridResources -BandResources -SpoolResources
```

资源输出 `target/hdr-avif-spool-resources/summary.json` 分开记录填充时间、编码＋metadata＋scratch I/O 时间、原生码误差和扩展误差。CPU 时间仍很高，不把内存合格等同于交互延迟合格；详细数字和最终回归统一记录在消费者方案 §29。

固定源码／许可门禁 `tools/check-hdr-codec-licenses.py` 校验 **122** 个 normal/build 包。原文／archive／patch 使用精确 hash，本地 UTF-8 源码使用明示的 LF 规范形式，审计包写同样字节。上游原包缺失的正文仅从固定官方 commit 取回，保留来源与 hash，未修改 registry。

`simd_helpers 0.1.0` 的固定原包及上游树仍缺原始 MIT 正文。因此清单会打印 `RELEASE REVIEW REQUIRED`；`--bundle <目录>` 生成原 archive、本地源、补丁、原文、SPDX 2.3 及 `NOT-RELEASE-READY.txt` 后**退出 1**。这是可审计但不完整的发行材料，不能宣称许可交付已全部解决，也不能据此公开发布完整产品。
