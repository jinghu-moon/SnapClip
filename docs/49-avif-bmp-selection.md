# 49 · AVIF 与 BMP 的编码方案选型

> 结论先给：**BMP 自己写**（零依赖、可流式、预览免费）；**AVIF 本轮不做**（编码器好选，**解码器是 C 依赖** ✗，会破坏"所见即所存"的预览原则）。
> 依据与来源见下；凡未验证的，逐条标 `[未验证]`，不当事实用。

## 0. 结论表

| 格式 | 编码器 | 可流式（滚动截图） | 新增依赖 | 许可 | 用在哪 |
| --- | --- | --- | --- | --- | --- |
| **BMP** | **自己写**（约 60–90 行） | **可以** ✓（top-down + 按行写） | **无** ✓ | 自家代码 | **普通 + 滚动** ✓ |
| **AVIF** | **`ravif 0.13`**（已在 `Cargo.lock` ✓，需在 `Cargo.toml` 显式声明） | **不可以** ✗（`encode_rgba()` 整帧） | 编码**无新增下载** ✓ | **BSD-3-Clause** ✓ | **仅普通截图** ✗ |
| AVIF 预览 | **无纯 Rust 解码器** ✗ | — | **C 依赖**（dav1d / AOM）✗ | dav1d 为 BSD-2 ✓ | **这就是 AVIF 被推迟的原因** ✗ |

## 1. 事实核查（每条带来源或标未验证）

### 1.1 `ravif`

- **纯 Rust** ✓，基于 `rav1e` + `avif-serialize`，是 `cavif` 的实现；**API 是单个 `encode_rgba()`** ✓ ⇒ **整帧** ✗ —— [Lib.rs: ravif](https://lib.rs/crates/ravif)；
- **BSD-3-Clause** ✓ —— [ravif LICENSE](https://docs.rs/crate/ravif/0.8.3/source/LICENSE)；
- **本机事实** ✓：`Cargo.lock` 已有 `ravif 0.13.0` 与 `rav1e 0.8.1`，但 `cargo tree -i ravif --offline` **输出为空** ✗ ⇒ **当前没有任何 crate 依赖它** ⇒ 要用必须在 `snapclip-*` 的 `Cargo.toml` 里**显式声明**（与 `miniz_oxide` 那次同一个教训）。

### 1.2 AVIF 解码（决定性）

- **没有可用的宽松许可纯 Rust 解码器** ✓ —— `avif-decode`（BSD-3）走 **AOM（C）** ✗；`image` 的 `avif-native` 走 **dav1d（C）** ✗ —— 来源：[一次同类调研的记录](https://github.com/jysf/crustyimg/commit/3da385214f9dcaf8aa6373f6a132013e6b02406f)（明确写 "no permissive pure-Rust d…"）；
- **Windows 的 WIC 不内置 AVIF** ✗ —— 需要安装商店的 **AV1 Video Extension** —— 来源：[ImageEn 论坛](https://www.imageen.com/ieforum/topic.asp?TOPIC_ID=3641)（"AVIF … 需要 AV1 Video Extension"）；
- ⇒ **结论**：AVIF 的"**编码后解码预览**"要么引入 **C 工具链** ✗，要么**放弃预览** ✗ —— 后者违反 `docs/42` D11 与规格 §139 的"所见即所存"✓。

### 1.3 BMP 的行序

- **`biHeight` 为负 ⇒ top-down**，原点在**左上** ✓；为正 ⇒ bottom-up，原点在左下 ✓ —— 来源：[Apache Commons Imaging 的说明](https://issues.apache.org/jira/browse/IMAGING-162)（原文与 [Chromium 的 BMP 读取器](https://chromium.googlesource.com/chromium/src/+/07955d420101ad98b446a9b1b3d26426627803cf/third_party/WebKit/Source/platform/image-decoders/bmp/BMPImageReader.cpp)一致）。

### 1.4 未验证项（不当事实用）

- `avif-rs` / `zenavif` / `zenravif` / `bmp` / `oxideav-bmp` / `zenbitmaps` 的**存在性、版本、许可、是否纯 Rust** —— **本机 cargo 索引无缓存** ✗，无法离线核实 ⇒ `[未验证]`；若要用，先 `cargo add --dry-run` 或查 crates.io ✓；
- `ravif` 的**实际编码耗时**（不同 speed / 不同内容）—— `[未验证]`，需实机测 ✓；
- **AVIF 尺寸上限**的确切值 —— 见 §3 的保守取值与说明 ✓。

## 2. BMP：自己写行带写入器

**不引入库的理由** ✗：`image` 的编码接口收 `ImageBuffer` ✗ ⇒ **整图物化** ✗（80 882 行 × 1250 × 4 = **404 MB** ✗），违反硬约束 1；而 BMP 的头部是**定长**的 ✓，写完头就能**逐行流式**追加 ✓。

### 2.1 文件结构（全部小端 ✓）

```
[BITMAPFILEHEADER 14 B]
[BITMAPV5HEADER  124 B]
[像素数据：逐行，自顶向下 ✓]
```

**`BITMAPFILEHEADER`（14 B）**

| 字段 | 值 |
| --- | --- |
| `bfType` | `0x4D42`（`'BM'`） |
| `bfSize` | `14 + 124 + 宽 × 高 × 4`（**u32** ⇒ 上限见 2.3） |
| `bfReserved1/2` | 0 |
| `bfOffBits` | `138`（= 14 + 124） |

**`BITMAPV5HEADER`（124 B，只列必须写对的字段）**

| 字段 | 值 | 为什么 |
| --- | --- | --- |
| `bV5Size` | `124` | V5 头 |
| `bV5Width` | 宽 | 有符号 32 位 |
| `bV5Height` | **`-高`** ✓ | **负值 ⇒ top-down** ⇒ 行序 = 我们的流序 ✓ |
| `bV5Planes` | `1` | — |
| `bV5BitCount` | **`32`** ✓ | BGRA ✓ |
| `bV5Compression` | **`BI_RGB (0)`** ✓ | 32bpp + `BI_RGB` 在 Windows 上即 **BGRA** ✓；比 `BI_BITFIELDS` 简单且 WIC 认 ✓ |
| `bV5SizeImage` | `宽 × 高 × 4` | 未压缩时可填 0，但填上更稳 |
| `bV5XPelsPerMeter` / `YPelsPerMeter` | 0（或按 DPI 换算） | 不影响解码 ✓ |
| `bV5ClrUsed` / `ClrImportant` | 0 | 32bpp 无调色板 |
| `bV5CSType` | `LCS_sRGB`（`0x73524742`） | 声明 sRGB ✓ |
| `bV5Intent` | `LCS_GM_IMAGES (4)` | 图像用途 ✓ |
| 其余（`Endpoints` / `Gamma*` / `Profile*` / `Reserved`） | 0 | — |

**像素行**

- **不需要行填充** ✓：32bpp 时每行字节 = `宽 × 4` ✓，**天然 4 字节对齐** ✓（若将来做 24bpp 才需要补到 4 的倍数 ✓）；
- **字节序就是 BGRA** ✓ ⇒ 我们现有的行带（BGRA ✓）**可以整行 memcpy** ✓，零转换 ✓；
- **alpha**：32bpp 有 alpha 通道 ✓，但**很多查看器忽略它** ✗ ⇒ 我们截图不透明 ✓，无影响 ✓（文档里写一句即可 ✓）。

### 2.2 流式写入的形状

```rust
struct BmpRowBandSink { /* impl Write + 已写行数 + 宽高 */ }
impl BmpRowBandSink {
    fn begin(w: &mut impl Write, width: u32, height: u32) -> io::Result<Self>;
    fn write_rows(&mut self, rows: &[u8]) -> io::Result<()>;   // 每行 宽×4 字节，按序追加
    fn finish(self) -> io::Result<()>;                        // 校验行数，缺行即错误
}
```

与 `PngRowBandSink` / `JpgRowBandSink` / `WebpRowBandSink` 的既有形状一致 ✓ ⇒ `write_bands` 里加一支即可 ✓。

### 2.3 上限

- `bfSize` 是 **u32** ✗ ⇒ **单文件 ≤ 4 GiB − 1** ✓ ⇒ 建议**硬上限 2 GiB** ✓（更安全，也避开工具的 32 位路径 ✗）；
- 1250 宽时：2 GiB ⇒ **约 429 000 行** ✓；你的 80 882 行 = **404 MB** ✓（**产品上必须提示文件很大** ✗）；
- 建议 `max_side` 与字节上限**一起判** ✓：`14 + 124 + w × h × 4 ≤ 2 GiB` ✓（这是现有 `FormatLimit` 表达不了的第三条，需要新的错误变体或复用"TooHigh"+文案 ✓）。

## 3. AVIF

### 3.1 编码器：`ravif`

- **推荐 `ravif`** ✓（而不是 `avif-rs` ✗）：**纯 Rust** ✓（`rav1e` 也是纯 Rust ✓）、**已在 lock** ✓、BSD-3 ✓、API 简单 ✓；
- **与"zen 家族一致"这条**：`zenavif`/`zenravif` 若存在且为纯 Rust ✓ 会更一致 ✓，但**本机无法核实** ✗ ⇒ `[未验证]`；若确认存在，再评估替换 ✓（接口同为整帧 ✓，替换不影响架构 ✓）。

### 3.2 只能整帧 ⇒ 只能用于普通截图

`ravif` 的 API 是 `encode_rgba()` ✓ ⇒ 必须拿到**整幅像素** ✗ ⇒ 滚动截图（80 882 行 × 1250 × 4 = 404 MB ✗）**不可用** ✗。

⇒ **接入点**：`scroll_format()`（`crates/snapclip-capture/src/scroll/mod.rs:129` 附近 ✓）里加一支：**AVIF ⇒ 降级 PNG** ✓（与 JPG/WebP 的既有降级一致 ✓）。

### 3.3 上限与质量

| 项 | 建议 | 依据 |
| --- | --- | --- |
| `max_side` | **16 384** ✓（保守） | libavif 默认 item 尺寸上限 16 384×16 384 `[来源: libavif 文档, 未逐条核对]`；AV1 本身可更大但没必要 ✓ |
| 像素预算 | **16 MP** ✓（与 `JPEG_FRAME_BUDGET_PIXELS` 同值，便于复用 `within_*` 形状） | 编码慢 ✗，需要前置拒绝 ✓ |
| 质量 | 复用 `jpeg_quality`（1–100）✓；`ravif` 另有 `speed`（1–10）⇒ **固定 8–10** ✓（快优先） | 用户不感知 speed ✓ |
| 耗时 | **4 MP 约 1–3 s** `[估算·未实测]` ✗ | 必须实机测 ✓（`codec-bench` 加一档 ✓） |

### 3.4 解码：这是 AVIF 的真正阻塞 ✗

- WIC **不内置** AVIF ✗（需商店的 AV1 Video Extension ✓ —— 不能假定用户装了 ✗）；
- **没有纯 Rust 的宽松许可解码器** ✓（见 §1.2）✗ ⇒ 要预览就得引入 **dav1d/AOM（C）** ✗；
- ⇒ **两条路，都得先拍板** ✗：
  - **A. 引入 `dav1d`**（BSD-2 ✓，但需 C 工具链 ✓ + CI ✓）⇒ AVIF **可做** ✓；
  - **B. 不做 AVIF** ✓（或做了但**没有解码预览** ✗ ⇒ 违反"所见即所存" ✗，不建议 ✗）。
- **我的建议：本轮选 B** ✓ —— 理由：BMP 已经给了"无损 + 流式 + 零依赖"的选项 ✓；AVIF 的收益是大图更小 ✗，但它**只能普通截图** ✗ + **要 C 依赖** ✗ ⇒ 收益/成本不成立 ✓（Occam ✓）。

## 4. 格式上限与降级的接入点

`crates/snapclip-capture/src/save_format.rs`：

1. `SaveFormat` 加 `Bmp` ✓（`Avif` 暂不加 ✗）；
2. `max_side()`：`Bmp => 2_147_483_647` ✓（`biWidth`/`biHeight` 是**有符号 32 位 LONG** ✓ ⇒ 这才是头字段的真上限 ✓；**真正的约束是字节上限** ✓ —— 取 32767 会把 4 万行的图误拒 ✗，而字节上限本来就更严 ✓）；
3. `can_encode()`：BMP 还需**字节上限**判据 ✓（`w*h*4 + 138 ≤ 2 GiB` ✓）⇒ 新增 `FormatLimit::TooLarge { bytes, max }` 或在调用方判 ✓（推荐新增变体 ✓，文案进**字体子集白名单** ✗ —— `"BMP"`/`"AVIF"` 是 ASCII ✓ 无需新字形 ✓，但**中文提示**需要 ✓）；
4. `label()`：`"BMP"` ✓（纯 ASCII ✓ ⇒ **字体白名单零改动** ✓）；
5. `scroll_format()`：`Bmp` **允许** ✓（可流式 ✓）；`Avif` 若将来加 ⇒ **降级 PNG** ✓；
6. `SavePreferences`：`format` 已能存枚举 ✓（serde 加一个 `bmp` 值 ✓ 向后兼容 ✓）。

## 5. UI 影响

| 项 | BMP | AVIF（若不选，忽略） |
| --- | --- | --- |
| 格式列表 | 加一项 **"BMP（无损，文件很大）"** ✓ | —— |
| 质量行 | **隐藏** ✓（无损 ✓，与 PNG 同规则 ✓） | 显示 ✓（有损 ✓） |
| 体积提示 | **必须** ✗：404 MB 量级 ⇒ 保存前提示 ✓ | —— |
| 滚动上下文 | **可选** ✓（可流式 ✓）；若超字节上限 ⇒ 禁用并说明原因 ✓ | 禁用 ✓ |

## 6. 测试清单（TDD）

**BMP（先写测试，全部可自动化 ✓）**

1. `a_bmp_header_declares_a_top_down_32bpp_image` ✓ —— 头 138 字节、`biHeight` 为负 ✓、`BI_RGB` ✓、`bfSize` 精确 ✓；
2. `bmp_rows_are_written_in_stream_order` ✓ —— 写两行不同颜色 ⇒ 解码后第 0 行是**先写的**那行 ✓（top-down 的直接证明 ✓）；
3. `bmp_output_matches_a_reference_decoder` ✓ —— 用 WIC 解码 ✓ 与源像素**逐字节相同** ✓（无损 ✓）；
4. `a_bmp_that_would_exceed_the_size_ceiling_is_refused_before_writing` ✓；
5. `a_long_capture_writes_bmp_without_materialising_the_image` ✓ —— 峰值分配断言（复用 `alloc_probe` ✓）；
6. `the_bmp_call_site_agrees_with_the_roll_band_writer` ✓ —— `write_bands` 的 BMP 分支与 `PngRowBandSink` 形状一致 ✓。

**AVIF（若将来做 ✓，先决条件：解码器拍板 ✓）**

7. `avif_is_refused_for_a_scroll_capture` ✓（`scroll_format` ⇒ PNG ✓）；
8. `avif_above_the_frame_budget_is_refused_before_encoding` ✓；
9. `an_avif_round_trip_matches_within_tolerance` ✓（**需要解码器** ✗ ⇒ 与第 3.4 条决定绑定 ✓）。

## 7. 代价

| 项 | 人天 | 说明 |
| --- | ---: | --- |
| BMP 写入器 + 上限 + `save_format` 接入 + UI | **1–1.5** | 头定长 ✓、行序 top-down ✓、无依赖 ✓ |
| BMP 测试（6 条 ✓） | 含在上面 | WIC 解码做参考 ✓ |
| AVIF（编码 + 上限 + UI） | **2–3** | 若做 ✓；`ravif` 已可用 ✓ |
| **AVIF 解码（预览）** | **+1–2 或 §4 的 A/B 决定** ✗ | dav1d（C）✗ —— 这是被推迟的真正原因 ✓ |

## 8. 与硬约束的对照

| 约束 | BMP | AVIF |
| --- | --- | --- |
| 1. 滚动不能物化整图 | **满足** ✓（top-down 按行 ✓） | **不满足** ✗（只能整帧 ✗） |
| 2. 格式上限/降级机制 | 需新增**字节上限**变体 ✓ | 复用 `max_side` + 帧预算 ✓ |
| 3. 质量语义 | 无损 ⇒ 不涉及 ✓ | 复用 `jpeg_quality` ✓ |
| 4. 所见即所存（要能解码） | **满足** ✓（WIC 内置 BMP ✓✓） | **不满足** ✗（无纯 Rust 解码 ✗） |
| 5. 许可 | 自家代码 ✓ | 编码 BSD-3 ✓ / 解码需 C ✗ |
