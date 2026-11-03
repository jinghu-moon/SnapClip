# HDR 测试样本

核验日期：2026-11-03。用户从 `aaron-rust/hdr-sample-images` 的
`different_file_formats_in_sdr_and_hdr` 目录下载并提供以下文件。
上游仓库：https://github.com/aaron-rust/hdr-sample-images 。

本目录保留用户提供的原始字节。文件大小、SHA-256、PNG 色彩标签及 AVIF
探测结果见 [manifest.json](manifest.json)，编码来源说明见
[Encoding Commands.txt](Encoding%20Commands.txt)。本次没有核对上游提交的文件哈希；
本地摘要用于后续回归确认样本未被转换或修改。提供的文件不包含上游 LICENSE，
入库派生资源前应一并保留上游的许可声明。

## 实际文件

| 文件 | 尺寸 | 像素编码与色彩元数据 | 文件大小 |
| --- | --- | --- | ---: |
| `1A_PNG_HDR.png` | 8256×5504 | RGB 16 位；PNG cICP `[9,16,0,1]`；ICC 名称 `Rec. 2020 PQ` | 205,736,718 B（196.21 MiB） |
| `1B_PNG_SDR.png` | 8256×5504 | RGB 16 位；ICC 名称 `sRGB IEC61966-2.1` | 221,975,080 B（211.69 MiB） |
| `2A_AVIF_HDR.avif` | 8256×5504 | AV1 12 位 YUV 4:4:4；容器 ICC 描述 `Rec. 2020 PQ` | 6,729,702 B（6.42 MiB） |
| `2B_AVIF_SDR.avif` | 8256×5504 | AV1 12 位 YUV 4:4:4；容器 ICC 描述 `sRGB IEC61966-2.1` | 7,722,561 B（7.36 MiB） |

每张图 45,441,024 像素；仅 RGBA8 栅格就需要 181,764,096 字节，RGBA16F
栅格需要 363,528,192 字节。这不是进程峰值：解码器、原图、预览代理、旧结果和
GPU atlas 可能额外占用内存。文件压缩后较小不代表解码成本较小。

### AVIF 色彩判定注意

这两个 AVIF 的 `colr` 同时包含 `prof`（ICC）和 `nclx`。后者均为
`primaries=2, transfer=2, matrix=6, full_range=true`，不能只看它区分两张图。
本机 ffprobe 只报告了 `smpte170m` 矩阵，没有报告 primaries／transfer；
进一步检查 `meta/iprp/ipco/colr` 的 ICC 描述，才能看到上述 HDR／SDR 信息。
不能因 ffprobe 未输出 PQ 就断言样本没有 HDR，也不能仅凭文件名判断。

## 与当前实现的基线差异

- Yue 静态解码默认限制编码文件 64 MiB，两张原始 PNG 均超限；组件像素上限
  6400 万并不意味着它会接受超限文件。
- Pin 本地图片还有 2400 万像素限制，四张原图均超过此限制。
- Yue 当前静态编码入口允许 PNG／JPEG／WebP／BMP，尚未允许 AVIF。
- 当前解码最终进入 `into_rgba8()`，GPUI 图片纹理是 8 位 BGRA；
  `Unmanaged` 只表示未转换色彩，不能把此路径视为已完成 HDR 解码／显示。
- 预览纹理的单边 8192／64 MiB 限制与编码文件限制属于不同预算。
  不应以整体抬高默认限制作为 HDR 支持的修复。

以上限制核对自 Yue `image_preview/resource.rs` 和 Pin `src/image_preview.rs`，
属于当前代码合同检查；本批没有执行生产加载路径、改变限制或完成 HDR 渲染验证。

## 首批验证使用方式

1. 以 PNG HDR／SDR 为主对照，AVIF 为第二种容器的交叉验证。
2. 在正确识别色彩元数据的 HDR 查看器中展示原文件，再测试 SnapClip 的
   HDR 屏幕捕获。记录显示器、Windows HDR 开关、SDR 内容亮度、查看器及版本。
3. 明确区分捕获保留 HDR、转换成 SDR、预览显示三条合同；RGBA／BGRA
   通道交换本身不承担 PQ 解码、色域转换或色调映射。
4. 为自动化建立较小派生样本前，先定义保持位深、PQ／色域标签的转换方法并核验。
   另用数值已知的灰阶／高光阶梯测试数学正确性。作者的 SDR 导出是同场景对照，
   不作为任意色调映射算法都必须逐字节相同的唯一答案。

原始 PNG／AVIF 只作本地大图验收，已单独忽略；README、manifest 和编码说明
可提交。本批只核对文件、头部／容器及探测元数据，没有完成像素正确性或实窗验收。

## 重核对

在 SnapClip 根目录用 PowerShell 运行以下命令，按 manifest 逐项检查大小和摘要：

```powershell
$manifest = Get-Content HDR-IMAGES/manifest.json -Raw | ConvertFrom-Json
foreach ($entry in $manifest.files) {
    $path = Join-Path HDR-IMAGES $entry.file
    if ((Get-Item -LiteralPath $path).Length -ne $entry.bytes) {
        throw "Size mismatch: $($entry.file)"
    }
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $entry.sha256) {
        throw "SHA-256 mismatch: $($entry.file)"
    }
}
```

AVIF 辅助探测（此命令不完整报告容器 ICC）：

```powershell
ffprobe -v error -show_entries stream=codec_name,width,height,pix_fmt,color_range,color_space,color_transfer,color_primaries -of json HDR-IMAGES/2A_AVIF_HDR.avif
```
