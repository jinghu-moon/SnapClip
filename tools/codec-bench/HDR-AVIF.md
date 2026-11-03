# HDR AVIF 候选的 CPU 资格验证

2026-11-04。**当前结论：候选的 10 位 Identity/RGB 解码数值门禁失败，不能据此接入生产。** 这不是显示器验收失败，也不等于所有 AVIF／YUV 路径都不可用。应用仍仅提供已验证的 HDR PNG16；MIT Yue 不新增这些编解码依赖。

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

## 实际结果

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

普通构建不需要探针。单独执行：

```powershell
cargo test -p codec-bench --bin hdr-avif-probe --features hdr-avif-probe --locked --offline
cargo build -p codec-bench --bin hdr-avif-probe --features hdr-avif-probe --release --locked --offline
python tools/verify-hdr-avif.py --probe target/release/hdr-avif-probe.exe
```

最后一条当前应退出 **1**，保留候选失败及独立解码证据；不能改预算或把失败转换为成功。需要本机 FFmpeg，可用 `--ffmpeg` 指定路径，`--output` 指定生成目录。没有该程序会明确失败。

本轮为避开并行 Yue 重构，Cargo 命令另加 `--config target/hdr-isolated.toml`；D 盘空间不足，探针构建另加 `--target-dir C:/Users/seeyuer/AppData/Local/Temp/snapclip-hdr-avif-target`，这不是产品默认配置。实际日志 `target/hdr-avif-{qualify-final,independent,unit-final,strict-final,feature-tree,production-tree}.log`；两个探针单元测试和显式 feature 的 strict Clippy 通过。

数值失败意味着当前候选尚不具备放行条件，暂不拿 4K/45MP AVIF 吞吐冒充成功验收。后续需要修复或更换明确的解码实现，再补 12 位、彩色／Alpha 边界、ICC+nclx 双属性冲突与范围／矩阵组合、运行中取消和 Release 峰值／耗时；这些仍是软件任务，不能归入“缺 HDR 屏才无法完成”。
