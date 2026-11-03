# HDR AVIF 候选的 CPU 资格验证

2026-11-04。**当前结论：已修复 Identity/GBR 解码、16 位编码平面顺序及 ICC/nclx 顺序依赖，CPU 像素和格式矩阵通过；整帧候选仍超出既有流式内存预算，不接入生产。** 应用仍仅提供已验证的 HDR PNG16；MIT Yue 不新增这些编解码依赖。颜色正确、资源合格、真实显示验收是三个独立条件。

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

本轮将 `zenavif 0.1.6` 和 `zenavif-parse 0.6.2` 的已校验发布源码及原许可证存入 `vendor/`，通过消费者 workspace patch 使用，**未修改 Cargo registry 缓存**。版本和所有其他依赖保持不变；lock 仅移除这两个包原来的 registry source/checksum，官方原包 hash 仍保存在清单。补丁范围和许可见 [维护说明](../../vendor/SNAPCLIP-PATCHES.md)。`python tools/check-hdr-codec-vendor.py` 对原包、80 份源文件及两份本地补丁逐字节校验，拒绝未记录的新增／缺失／修改文件。

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

剩余软件工作明确为：先证明有界 tile/grid 或其他可取消的确定实现符合现有预算，再补生产 AVIF 编解码／ICC／原子保存合同；测试不能用隐藏任务或丢弃结果冒充运行中取消。当前 AVIF 生产开关继续关闭，PNG16 和 Yue 的 MIT 依赖边界保持原样。
