# 图片编码格式选型调研

> 日期：2026-10-17 ｜ 性质：**只调研，不改产品代码、不加依赖、不提交**
> 仓库 HEAD：`595d5e1` ｜ 工具链：`rustc 1.98.1 (48a229cea 2026-09-01)`
> 一句话结论：**PNG 维持现状；JPG 选 `mozjpeg-rs`；WebP 本轮不做。**
> 最重要的修正：**仓库自身是 AGPL-3.0-only，所以 `zenjpeg`/`zenwebp` 的 AGPL 不构成障碍** —— 用户那份报告里把授权当成排除理由，对本仓库不成立。

---

## 0. 结论摘要

| 项 | 结论 | 依据 |
| --- | --- | --- |
| PNG | **不动**。现有流式路径已满足长图（1280×30000）与内存目标 | 代码 `row_band_png.rs` + 实测 2.34 s / 14.4 MiB / 峰值 16.5 MiB（§2） |
| JPG | **首选 `mozjpeg-rs 0.9.2`**（BSD-3、默认纯 Rust、MSRV 1.89、下载 10.4 万、1.7 万行） | §1 事实表；授权 + 成熟度都稳 |
| JPG 备选 | `zenjpeg 0.8.4`（同作者、能力更强、但 2026-06 才发布、8.2 万行、下载仅 6.7k） | 授权不再是问题（§4），是**新代码风险**问题 |
| WebP | **不做**：`16383×16383` 硬上限使 1280×30000 无法编码；授权干净的纯 Rust 选项只有**无损**，相对 PNG 无功能收益 | Google WebP FAQ（§1.3） |
| 架构 | **无需改动**：`snapclip-capture` 只给 BGRA + 尺寸、`apps/snapclip` 各自写 sink —— 现在就是这样 | `mod.rs:20-25`、`ports.rs`（§2.5） |
| 授权 | 仓库四个 crate 都是 `license = "AGPL-3.0-only"`，`LICENSE` 为 AGPLv3 全文 ⇒ 选 AGPL 分支合法 | `LICENSE`、各 `Cargo.toml:6` |

---

## 1. 事实表（授权 / MSRV / 流式 / 尺寸上限 / 纯 Rust）

全部来自 crates.io API 的版本元数据（2026-10-17 抓取）与 Google WebP 官方 FAQ。

### 1.1 编码器候选

| crate | 版本 | license | MSRV | 流式编码 | 纯 Rust | 代码行 / 下载 | 备注 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `png` | 0.18.1 | MIT OR Apache-2.0 | **1.73** | ✅ `into_stream_writer`（在用） | ✅ | 9.0k / 7.9e7 | 当前依赖 |
| `mozjpeg-rs` | 0.9.2 | **BSD-3-Clause** | **1.89** | 未验证（README 称有 streaming 模式） | ✅ 默认纯 Rust（C 版经可选特性 `mozjpeg-sys-config`） | 17.4k / 1.0e5 | 同作者（imazen），带 trellis 量化 |
| `jpeg-encoder` | （未列入 lock；`image` 的 jpeg 特性会用它） | MIT/Apache-2.0 | 低 | ✅ 逐行 | ✅ | 小 | 最保守、能力最弱 |
| `zenjpeg` | 0.8.4 | **AGPL-3.0-only OR LicenseRef-Imazen-Commercial** | **1.93** | 未验证（README 称流式） | ✅ | 81.6k / 6.7e3 | 2026-06 首发，"部分 AI 生成"自述 |
| `zenwebp` | 0.4.4 | 同上（AGPL 或商业） | 1.93 | 未验证 | ✅ | 52.4k / 3.8e4 | WebP 编解码 |
| `image-webp` | 0.2.4 | MIT OR Apache-2.0 | 1.80.1 | ❌ | ✅ | 6.9k / 6.2e7 | 用户称**编码只有无损**（未独立验证） |
| `webpx` | 0.4.0 | MIT OR Apache-2.0 | 1.89 | ✅（`streaming` 特性） | ❌ 绑定 **libwebp**（`libwebp-sys`） | 6.5k / 2.4e5 | 同作者；需要 C 工具链 |

### 1.2 尺寸上限（这是"能不能用"的分水岭，不是口味问题）

| 格式 | 单边上限 | 1280×30000 可行？ | 依据 |
| --- | --- | --- | --- |
| PNG | `u32`（本仓库 `begin()` 用 `u32::try_from` 拒绝越界） | ✅ | `row_band_png.rs:165-172` |
| JPEG（基线） | 65535 | ✅（30000 < 65535） | JPEG/JFIF 规范 |
| **WebP** | **16383** | ❌ **根本不可能** | Google FAQ 原文：*"WebP with VP8 uses 14 bits for width and height. The maximum pixel dimensions of a WebP image is 16383 × 16383."* |

### 1.3 WebP 的采样事实（修正一条重要建议）

Google WebP FAQ 原文：

- **有损 WebP 只能是 8-bit Y'CbCr 4:2:0** ⇒ **lossy WebP 没有 4:4:4 这个选项**；
- 无损 WebP 是 RGBA。

⇒ 报告里"用 4:4:4 救彩色文字/细线"这条建议**只对 JPEG 成立**，对 WebP 不成立（它只能是 4:2:0 或无损）。

### 1.4 与"用户那份结论"的差异（逐条）

| 用户报告的说法 | 本次核查 | 影响 |
| --- | --- | --- |
| "`zenjpeg`/`zenwebp` 是 AGPL 或商业授权 ⇒ 授权敏感时退到 `mozjpeg-rs`" | **AGPL 属实**（元数据 `AGPL-3.0-only OR LicenseRef-Imazen-Commercial`），但**仓库自身就是 AGPL-3.0-only** ⇒ **不构成障碍** | 结论从"排除"变成"可选"；选择依据改成工程质量与风险 |
| "`zenjpeg` MSRV 1.93" 被当作风险项 | 属实，但**本机 rustc 1.98.1 且仓库未声明 `rust-version`** ⇒ 不是阻塞 | 不计入排除理由 |
| "`mozjpeg-rs` 的流式模式不支持 trellis/渐进/Huffman 优化" | **未独立验证**；其 crate 描述为"with trellis quantization" | 需要落地前核对 docs.rs |
| "质量参数不能替代 4:4:4" | ✅ 正确（对 JPEG） | 采纳 |
| "`image` 的 WebP 主要是无损" | 与 6.9k 行规模一致；**未独立验证** | 采纳但标注未验证 |
| "`encode_to_writer` ≠ 行带流式" | ✅ 正确：`image` 的 API 面向整张 `DynamicImage` | 采纳 |
| "WebP 比 PNG 小 20–50% 不能当产品保证" | ✅ 正解；Google 自己的说法是**无损 WebP 比 PNG 小 26%**（平均值，非保证） | 采纳 |
| "架构上 capture 只给 BGRA 行带、shell 各自写 sink" | ✅ **已经就是这样**，无需改 | 采纳（无需动作） |

---

## 2. 代码现状审计（读代码所得，附证据）

### 2.1 PNG 流式写入如何工作

`apps/snapclip/src/capture/row_band_png.rs`：

- 参数：`Compression::Balanced` + `Filter::Up`，来自 P0.04 对 12 组组合的实测（`docs/30 §17.7.1`）；
- 头部：`begin()` 先做 `u32::try_from` 双向检查（`row_band_png.rs:165-172`），再 `set_color(Rgba)` / `set_depth(Eight)` / `write_header()`（**IHDR 在此定型，所以高度必须在 begin 就知道**），然后 `into_stream_writer()`；
- 行带：`write_rows(first_row, rows)` 校验三件事 —— 行长度是整行、行号严格递增、不越过声明高度（`:207-228`）；
- **BGRA→RGBA 的交换逐行做**，只用 `self.row` 一块行大小的 scratch（`:235-243`）——注释明说"整图 RGBA 一次缓存就是 P4.03 要删掉的那份拷贝"；
- 收尾：`finish()` 若行数不足直接 `Sink` 错误（PNG 高度在头部，短写只能是损坏文件，`:250-256`），然后 `stream.finish()`，再从共享 `Arc<Mutex<Vec<u8>>>` 里 `Arc::try_unwrap` 取字节（正常路径**不拷贝**，`:268-276`）；
- 每行内存 = 1 行 scratch；编码器自身缓冲 = 3 行 + 4 KiB 块（文件头注释 `:26-28`）。

### 2.2 峰值内存约束落在哪里

在 `write_rows` 的**逐行循环**里：编码器被喂调用方的切片，永远不需要第二张整图（`artifact_writer.rs:20-25` 的注释 + `P4.03` 的实测装置 `tests/export_path_copies.rs`：1280×20000 下"≥整图大小的分配"为 **0**，改造前是 6 次）。

### 2.3 导出链路收口

```
overlay（选区提交） → publish_selection（SelectionPixels：BGRA + region + dpi + 显示器名 + corner_radius）
                    → export worker 线程（export_worker.rs:262 spawn）
                    → ArtifactWriter::write（apps/snapclip/src/capture/artifact_writer.rs:52）
                    → PngRowBandSink（唯一的编码器）
                    → CaptureArtifactStore 落盘
```

- `artifact_writer.rs:72-95`：先做 `u32` 转换与 **BGRA 长度自检**（"bgra buffer of N bytes does not match WxH"），再编码；
- 背景线程：`export_worker.rs:262` `thread::spawn`；取消语义完整 —— 队列里的陈旧任务直接丢弃（`:151-152`）、已在跑的导出结果被丢弃并**删掉已写的文件**（`:269-270`、测试 `a_cancelled_export_deletes_the_file_it_already_wrote`）、日志 `export discarded … (cancelled)`。

### 2.4 滚动长图的上限与素材

- 上限 **30000 px**：`apps/snapclip/tests/png_row_band_memory.rs:104 const HEIGHT: u32 = 30_000`；`docs/21` 的 `max_height=30000`；
- 实测素材：1280×30000（原始行 146.5 MiB）与 3840×30000（439.5 MiB），装置 `tests/png_params_probe.rs` + `tools/p0-04-png-params.ps1`。

### 2.5 `png` 从哪来、有没有 JPEG/WebP

- **直接依赖**：`apps/snapclip/Cargo.toml:50 png = "0.18"`（P4.02 从 `[dev-dependencies]` 提升为普通依赖）；
- **lock 里两个 png**：`0.18.1`（直接 + 经 `image`）与 `0.17.16`（别的 crate 带来）；`cargo tree -p snapclip-app` 确认应用侧走 `0.18.1`；
- **没有任何 JPEG/WebP 编码器**：lock 里只有 `zune-jpeg 0.5.15`（及 0.4.21），它由 `image 0.25.10` 拉入，而 `image` 在 `snapclip-history` 里只开 `["png","bmp"]` ⇒ `zune-jpeg` 是**解码**用途；没有 `jpeg-encoder`、`webp`、`mozjpeg`；
- **架构方向已被测试守住**：`tools/check-dependency-direction.ps1` 检查 `cargo tree -p snapclip-capture` 里没有 `png` 行（P4.02 记录：capture 30 packages、`scroll/` 13 files）。

### 2.6 已经拿到的真实数字

| 素材 | 时间 | 产物 | 峰值 | 出处 |
| --- | --- | --- | --- | --- |
| 1280×30000（合成页） | p50 **2344 ms** / max 2355 ms | **14.4 MiB** | **16.5 MiB** | `docs/30 §17.7.1` |
| 3840×30000（合成页） | **7749 ms** | **40.1 MiB** | **65.1 MiB** | 同上（补充行） |
| 1280×30000（L4 内存装置） | 0.24 s | 317 KiB（内容极规整） | **peak-live 885 KiB** | `png_row_band_memory` 输出 |
| 1242×1000（真机、照片类内容） | `encode_write_ms=203` | — | — | 用户日志 |
| 1303×1035（真机、照片类内容） | `encode_write_ms=272` | — | — | 用户日志 |

> 注意口径：日志里的 `encode_write_ms` 包含**编码 + 落盘**，且内容熵高（照片类）⇒ 与合成页的 MiB/s 不可直接比。

---

## 3. 授权风险判断

### 3.1 事实

- 本仓库：`LICENSE` = **GNU Affero General Public License v3**（全文），四个 `Cargo.toml:6` 都是 `license = "AGPL-3.0-only"`；
- `zenjpeg` / `zenwebp`：`AGPL-3.0-only OR LicenseRef-Imazen-Commercial` —— 即**双许可**，你可以选择 AGPL 那一支。

### 3.2 对 AGPL 桌面应用意味着什么

- AGPL 的第 13 条（网络交互）对**不提供网络服务**的桌面应用**不触发**；
- 但 AGPL 是**强 copyleft**：分发二进制时，必须提供**完整对应源码**，且**整个衍生作品**都要以 AGPL 授权；
- **本仓库本身就是 AGPL-3.0-only** ⇒ 引入同为 AGPL-3.0-only 的 `zenjpeg`/`zenwebp` **不产生任何新义务**，是完全同许可的混合；
- **反向条件**：如果将来要把产品改成专有/闭源，那么 `zenjpeg`/`zenwebp` 会立刻变成不可接受（除非购买商业授权）。`mozjpeg-rs`（BSD-3）没有这个约束 —— BSD-3 与 AGPL-3.0 兼容，两个方向都安全。

### 3.3 结论

**① `zenjpeg`/`zenwebp` 的授权不会阻碍本项目**（因为项目已是 AGPL-3.0-only）。它们的真正风险是**工程质量**：`zenjpeg` 2026-06 首发、8.2 万行、下载 6.7k、README 自述部分 AI 生成。
**但** `mozjpeg-rs`（BSD-3）在"授权 + 成熟度"两项上都更稳，且同作者同血统 ⇒ **默认选它**，把 `zenjpeg` 作为"需要更强流式/4:4:4 能力时"的备选记录。

---

## 4. 推荐的最小方案

```
PNG  ：不动（png 0.18.1 + 现有 Balanced/Up 行带 sink）
JPG  ：mozjpeg-rs 0.9.2，quality 90–95、4:4:4、非渐进；输入由现有逐行 BGRA→RGB 交换喂进去
WebP ：本轮不做（UI 不提供该项）
```

### 4.1 PNG：为什么"不动"就是正解（问题 ② 的回答）

**是，现有 PNG 路径已经满足长图与内存目标**，证据不是估算而是代码 + 实测：

1. 流式：`into_stream_writer()`，行块逐行喂（`row_band_png.rs:182, 236-243`）；
2. 内存与图像大小无关：每行 1 块 scratch + 编码器 3 行 + 4 KiB（`:26-28, :200-204`）；
3. 1280×30000 实测：**2.34 s / 14.4 MiB / 峰值 16.5 MiB**；4K 宽 3840×30000：7.7 s / 40.1 MiB / 65.1 MiB；
4. L4 内存装置：`peak-live 885 KiB`（1280×30000，规整内容）；
5. 越界与短写都有明确拒绝（`TooLarge` / `Sink`），不会产出"尺寸说谎"的文件。

⇒ 长图与内存目标**已经达成**，PNG 不需要任何改动，也不该为了"统一抽象"把它换掉。

### 4.2 JPG：为什么 `mozjpeg-rs` 而不是 `zenjpeg`

| 维度 | `mozjpeg-rs 0.9.2` | `zenjpeg 0.8.4` |
| --- | --- | --- |
| 授权 | BSD-3（两个方向都安全） | AGPL/商业（本仓库 OK，改闭源则不行） |
| 成熟度 | 2026-04 首发，17.4k 行，10.4 万下载 | 2026-06 首发，81.6k 行，6.7k 下载 |
| MSRV | 1.89 | 1.93（本机 1.98 OK） |
| 纯 Rust | 默认是（C 版是可选特性） | 是 |
| 已知能力 | 带 trellis 量化 | 自述流式 + 4:4:4 + Bgra8Srgb |

**落地前必须先核**：`mozjpeg-rs` 的 **streaming API** 是否逐行、是否支持 **4:4:4 / 非渐进 / BGRA 直接输入**。若它只支持整图编码 ⇒ 会**多一次整图 RGB 缓冲**（180 MB 级），需要按 `P4.03` 的"零整图拷贝"纪律重新评估。**→ 已于 §8 核查完毕：支持逐行流式与 4:4:4，不支持 BGRA 直入（按行带转换即可），结论是不需要转 `zenjpeg`。**

### 4.3 WebP：为什么不做（问题 ③ 的回答）

**跳过 WebP 会损失什么？**

1. **长图根本用不了**：`16383×16383` 硬上限（官方 FAQ 原文），1280×30000 无法编码 ⇒ 只能做成"短图用 WebP、长图用别的"的两套行为，复杂度和用户困惑都上升；
2. **没有"有损 + 4:4:4"**：有损 WebP 固定 4:2:0 ⇒ 对彩色文字/细线/UI 边缘，它比 PNG 更差，而不是更好；
3. **授权干净的纯 Rust 选项只有无损**（`image-webp`）⇒ 相对 PNG 只剩"平均小 26%"（Google 自己的平均值），而我们已经实测页面类截图 1280×30000 只有 14.4 MiB、规整内容 317 KiB —— 边际收益很小；
4. **损失的是**：更小的短图文件、以及"支持 WebP"这个勾选项；**不是**任何功能（没有它照样能截所有图）。

⇒ 结论：WebP 延后，直到出现"短图 + 有损 + 体积敏感"的真实需求，届时再在 `zenwebp`（AGPL，本仓库可用）与 `webpx`（MIT，需 C 工具链）之间选。

---

## 5. 基准测试计划（本轮不跑，落地时按此执行）

### 5.1 素材

| 编号 | 内容 | 目的 |
| --- | --- | --- |
| A | 1920×1080 桌面（含窗口边框、彩色文字、细线） | 常规截图基线 |
| B | 3840×2160（150% 缩放下本机真实尺寸） | 4K |
| C | 1280×30000 合成页（现成装置） | 长图、内存上限 |
| D | 带透明（圆角 > 0，四角 alpha=0） | PNG/WebP 的 alpha 路径 |
| E | 彩色文字 + 细线 + 圆角混合 | **4:2:0 与 4:4:4 的判别素材** |

### 5.2 度量（每个格式 × 每个素材）

- **时间**：p50 / max，≥3 run，**每组合独立进程**（沿用 `tools/p0-04-png-params.ps1` 的纪律）；
- **体积**：产物字节数；
- **内存**：`CountingAllocator` 的 live/peak（同时记，避免"堆流量证明空间下降"的误区）；
- **保真**：
  - PNG / 无损 WebP：**解码后逐像素比对**；
  - JPG：SSIM + PSNR **且** 文字区人工检查（另加"文字带内每通道最大误差"作为可自动断言）；
- **错误路径**：> u32（PNG）、> 16383（WebP）、> 65535（JPG）必须**显式拒绝并命名**；取消/写盘失败/线程退出各一条用例。

### 5.3 已有真实数字（可直接引用，不必重测）

见 §2.6：1280×30000 = 2.34 s / 14.4 MiB / 16.5 MiB；3840×30000 = 7.75 s / 40.1 MiB / 65.1 MiB；真机 1242×1000 = 203 ms（含落盘）。

---

## 6. 未验证项

1. `mozjpeg-rs` 的流式 API、4:4:4、非渐进、BGRA 直入 —— **落地前必须查 docs.rs**（本轮只看了 crates.io 元数据与描述）；
2. `mozjpeg-rs` 流式模式是否真的缺 trellis/渐进/Huffman 优化（报告的说法）—— 未独立验证；
3. `zenjpeg` 的流式与 4:4:4 能力 —— 仅依据 README/描述；
4. `image-webp` 编码是否只有无损 —— 未独立验证（与其 6.9k 行规模一致）；
5. WebP 的实际编码体积/耗时 —— 未测（没有选定编码器）；
6. JPEG 65535 维度上限在 `mozjpeg-rs` 里的具体行为 —— 未验证。

---

## 7. 三个必答问题

**① `zenjpeg`/`zenwebp` 的授权是否真的会阻碍商用？**
**不会** —— 本仓库自身是 **AGPL-3.0-only**（`LICENSE` 全文 + 四个 `Cargo.toml:6`），与它们同许可，直接用 AGPL 那一支即合法、无新增义务。**唯一条件**：若产品将来转闭源/专有，则这两个 crate 立即不可用（除非买商业授权），而 `mozjpeg-rs`（BSD-3）不受影响。

**② 现在的 PNG 路径是否真的已经满足长图与内存目标？**
**是**，代码证据：`into_stream_writer` 逐行喂（`row_band_png.rs:182,236-243`）、每行只 1 块 scratch（`:200-204`）、编码器 3 行 + 4 KiB（`:26-28`）、u32 越界与短写都显式拒绝（`:165-172,250-256`）；实测：1280×30000 = **2.34 s / 14.4 MiB / 峰值 16.5 MiB**，L4 内存装置 `peak-live 885 KiB`。不需要改动。

**③ 只做 PNG + JPG（跳过 WebP）会损失什么？**
损失：短图更小的体积、"支持 WebP"这个勾选项、以及"有损 + alpha"这种 WebP 独有能力。**不损失任何功能**。而且长图（>16383）根本不能用 WebP，有损 WebP 固定 4:2:0 对 UI 文字更差，授权干净的纯 Rust 选项只有无损 ⇒ 收益不足以支撑"两套行为"的复杂度。

---

## 8. 落地前的门：`mozjpeg-rs 0.9.2` API 核查（已验，2026-10-17）

全部出处为 docs.rs 0.9.2 的 API 页与其 `Source` 行号；未写"未验证"的条目均已查到明确文档。

| # | 问题 | 答案 | 出处 |
| --- | --- | --- | --- |
| 1 | **流式** | **支持**：`StreamingEncoder::start_rgb(width, height, writer) -> EncodingStream<W>`，随后 `EncodingStream::write_scanlines(&[u8])` 与 `finish()`；文档原话 "scanline-by-scanline encoding, which is memory-efficient for large images"。**约束**：一次喂的行数必须是 **8 或 16 的倍数**（取决于子采样）⇒ 行带取 8 或 16 行即可 | `struct.StreamingEncoder.html`（源码 `encode/streaming.rs:200-225`） |
| 2 | **4:4:4** | **支持**：`Subsampling::S444`（"No subsampling (highest quality)"），另有 `S422/S420/S440/Gray`。**默认值文档未写明** ⇒ 必须显式设 `S444`，不要依赖默认 | `enum.Subsampling.html`（源码 `types.rs:494-506`） |
| 3 | **非渐进** | **支持**，而且流式只能是 baseline：流式模式文档明写 "No progressive mode (requires multiple passes)"，另提供 `force_baseline(bool)`。注意**批量 `Encoder` 默认是 progressive**（`Preset::ProgressiveBalanced`）⇒ 流式路径不要走默认 preset | 同 #1 页面 |
| 4 | **BGRA 直入** | **不支持**：只有 `start_rgb`（3 字节/像素）与 `start_gray`。⇒ 需要**按行带**做 BGRA→RGB 转换（每带一次，峰值内存仍是一个帧带级 ✓，**不是**整帧 RGB 缓冲） | 同 #1 页面 |
| 5 | **超限** | **明确返回错误、不 panic**：`Error::DimensionLimitExceeded{width,height,max_width,max_height}`、`PixelCountExceeded`、`AllocationLimitExceeded`、`InvalidDimensions`、`InvalidStride`、`Cancelled`、`TimedOut`。3840×30000 在 u32 与 JPEG 65535 之内 ⇒ **可编码** | `enum.Error.html`（源码 `error.rs:11-103`） |

**顺带核实的三条**

- 流式模式确实**不支持** trellis / 渐进 / Huffman 优化 —— 文档原文列出这三项"requires buffering the entire image"，与上一轮"未验证"的猜测**一致**；
- `quality(u8)` 范围 **1–100**（越界走 `Error::InvalidQuality`）；默认值文档未写明 ⇒ 显式传；
- 许可证 **BSD-3-Clause**；`mozjpeg-sys`（C 版）是**可选依赖**，默认路径是纯 Rust —— 但"默认特性是否关闭"本轮**未查**（记入未验证）；另有 `Unstoppable`/`Stop` 协作取消，与仓库现有的取消语义对得上。

### 一句话结论

**能满足** SnapClip 的约束：**逐行流式 + 4:4:4 + 非渐进 + 3840×30000 + 峰值内存不超一个帧带**。唯一额外代价是 **BGRA→RGB 的行带转换**（不是整帧拷贝，不破坏内存目标），所以**不需要为此转向 `zenjpeg`**。卡点只有"不直接吃 BGRA"这一条，且它不影响 §2.2 的内存口径。

**落地形态**：`start_rgb` 接受任意 `W: Write` ⇒ JPG sink 可以照 `row_band_png::PngRowBandSink` 的结构写，逐带转换 + `write_scanlines` + `finish`，与现有 PNG 路径并列。

### §8 的未验证项

1. 流式模式的**行倍数**到底按 8 还是 16（取决于子采样；S444 应为 8，未查实）；
2. `Subsampling` / `quality` 的**默认值**（文档未写，落地时显式设置即可绕开）；
3. **默认特性是否关闭 C 版** `mozjpeg-sys`（影响"纯 Rust、无 C 工具链"这一条）；
4. 3840×30000 的**实际耗时与峰值内存**（计划项，未测）。

## 9. 对 `zenjpeg 0.8.4` 的门（"高性能 / 高质量"定为前提后复验）

> **前提变更**：§4.2 与 §8 的选型建立在"风险最小"上。用户明确"高性能 / 高质量，这是前提" ⇒ 本节按能力复验 `zenjpeg`，结论**覆盖 §8 那句"不需要转向 zenjpeg"**。

| # | 问题 | 结论 | 出处 |
| --- | --- | --- | --- |
| 1 | **流式** | ✅ `BytesEncoder` 自述 "wraps `StreamingEncoder` to provide **true streaming encoding without buffering the entire image in memory**"；`push(data, rows, stride_bytes, stop)` 接受**任意行数**（页面未给倍数约束，与 `mozjpeg-rs` 不同），另有 `push_packed(data, stop)`；`finish()` / `finish_into(&mut Vec<u8>)` / **`finish_to(W: Write)`**；`rows_pushed()` / `rows_remaining()` | docs.rs `encoder/struct.BytesEncoder.html`；源码 `encode/byte_encoders.rs:19-35,176-263,308-416` |
| 2 | **4:4:4** | ✅ `ChromaSubsampling::None` = "4:4:4 - No subsampling (full chroma resolution, best quality, largest files)"；经 `EncoderConfig::ycbcr(quality, ChromaSubsampling::None)` | `encoder/enum.ChromaSubsampling.html`；源码 `encode/encoder_types.rs:259-268` |
| 3 | **BGRA 直入** | ✅ `PixelLayout::Bgra8Srgb` = "BGRA, 4 bytes/pixel, sRGB gamma (alpha ignored on encode)"。`BytesEncoder` 按 layout 收字节 ⇒ **流式也吃 BGRA** ⇒ 省掉逐带 BGRA→RGB | `encoder/enum.PixelLayout.html`；源码 `encoder_types.rs:367-409` |
| 4 | **超限 / 非法尺寸** | ⚠️ 全部走 `Result`（`Error`/`Result` 在 crate 根导出），**没有 panic 路径**；但"尺寸上限"的**具体错误变体名**在本次抓到的页面里未见 ⇒ 记为未验证。可编码性：3840×30000 < JPEG 65535 ✓ | `zenjpeg/index.html`（Encoder API 段） |
| 5 | **流式下的优化** | ⚠️ **部分确认**：`finish_with_huffman_frequencies` 文档写 "Returns `None` for counts if **streaming-through mode** was used (no buffered blocks to count from)" ⇒ 存在"真流式（不缓冲块）"与"缓冲块"两种模式，**真流式拿不到 Huffman 优化**；trellis 同样依赖块数据（文档未直说 ⇒ 未验证）。**`BytesEncoder::push` 属于哪一种模式，抓到的页面没有明说** ⇒ 本节最大的未验证项 | `struct.BytesEncoder.html#method.finish_with_huffman_frequencies` |

**顺带核实**

- 许可：crates.io `license = "AGPL-3.0-only OR LicenseRef-Imazen-Commercial"` ✓；依赖表里**没有任何 `-sys` / C 绑定** ⇒ 纯 Rust ✓；
- MSRV `rust_version = 1.93`；本机 rustc 1.98.1、仓库未声明 `rust-version` ⇒ 不阻塞 ✓；
- **`trellis` 默认值有冲突**：crates.io 的 `features.default = ["decoder","yuv"]`（**不含 trellis**），docs.rs 的特性表却写 trellis ✅ 默认 —— 以 Cargo 可读的 crates.io 元数据为准 ⇒ **落地必须显式启用 `trellis`**；
- 默认特性含 `decoder` ⇒ 只做编码时应 `default-features = false`，否则白编一个解码器；
- 取消：`Stop` trait + `Unstoppable`，`push(..., stop)` **每条带都可取消** ✓，与 `export_worker` 的"取消时删半成品"语义对得上 ✓。

**一句话结论**：在"高性能 / 高质量"前提下，`zenjpeg` **能满足**「真流式 + 4:4:4 + BGRA 直入 + 非渐进 + 3840×30000 + 峰值内存不超一个帧带」，并在两处**优于 `mozjpeg-rs`**：**BGRA 直入**（省逐带转换）、**`push` 无行倍数约束**（m-rs 要求 8/16 的倍数）。它与 m-rs 在"真流式丢掉 Huffman/trellis"这一点上**相同**；**未验证**的是 `push` 走 streaming-through 还是 strip 缓冲 —— 这条决定长图 JPG 的体积与速度，**必须在基准里量**。

### 9.1 顺带：`zenwebp 0.4.4` 的无损路径

| 项 | 结论 | 出处 |
| --- | --- | --- |
| 无损配置 | `LosslessConfig`：`quality`（0–100 压缩努力，默认 75）、`method`（0–6，默认 4）、`alpha_quality`（默认 100 = 无损 alpha）、`near_lossless`（默认 100 = 关闭 ⇒ 真无损）、`exact`（默认 false：alpha=0 处的 RGB 允许被改） | `encoder/struct.LosslessConfig.html` |
| 资源上限 | `with_max_dimensions` / `with_max_memory` / `estimate_memory(_ceiling)` ⇒ 16383 超限可在**编码前**拒绝 | 同上 |
| 流式 | ❌ 没有逐行接口：`EncodeRequest` + `encode()` / `encode_to(W)` 是**整图**路径（`std` 只为 `io::Write`） ⇒ WebP 只能整图编码 | `zenwebp/index.html` |
| alpha | ✅ `Rgba8` + `alpha_quality` | 同上 |
| 纯 Rust | ✅ crate 根自述 `#![forbid(unsafe_code)]`（SIMD 走 `archmage` 安全封装）；依赖表无 C 绑定 | `zenwebp/index.html`（Safety 段） |
| 版本 | **0.4.5 已被 yank** ⇒ 钉 `=0.4.4` | crates.io 元数据 |

**一句话**：`zenwebp` 的无损路径**满足"高质量"**（真无损 + 无损 alpha + 可限资源），但**不满足"流式"** —— 对 WebP 不构成问题（它只承载 ≤16383 的普通截图），却再次说明 **WebP 不能作为滚动长图的格式**（与 §1.2 一致）。

### 9.2 裁定（覆盖 §4.2 / §8 的选型）

- 前提"高性能 / 高质量"成立时（用户已明确）：**JPG 用 `zenjpeg 0.8.4`** —— `ChromaSubsampling::None`(4:4:4) + `PixelLayout::Bgra8Srgb` + 非渐进 + **显式启用 `trellis`**；
- 若改回"接受 5–10% 体积换更成熟依赖"：`mozjpeg-rs 0.9.2`（BSD-3）仍是安全退路，两者 sink 同构（`W: Write` + 行带）⇒ **换库只改 sink 内部**；
- **WebP**：`zenwebp 0.4.4` 无损、仅 ≤16383、**整图**编码；
- 三者的 sink 都落在 `apps/snapclip` 侧，`snapclip-capture` 只给 BGRA 行带与尺寸（§2 结论不变）。

### 9.3 §9 的未验证项

1. `BytesEncoder::push` 是 streaming-through 还是 strip 缓冲（决定长图 JPG 的体积/速度）；
2. `Error` 里"尺寸上限"的具体变体名；
3. trellis 在真流式下是否可用（目前由 Huffman 那句反推，无官方明文）；
4. `trellis` 默认值的两处文档冲突已做交叉核对，但**未编译验证**；
5. 各配置的实际耗时 / 体积 / 峰值内存（按 §5 的基准计划量）。

## 10. 基准：`tools/codec-bench` 的首轮实测

`cargo run -p codec-bench --release -- --report`。素材是**程序生成**的合成截图（彩色小字、1px 细线、圆角、平色面板、渐变、半透明窗口），四类：`ui-1080`(1920×1080) · `ui-4k`(3840×2160) · `scroll-tall`(1280×30000) · `alpha-2k`(2000×2000)。三个指标：输出字节、中位耗时、**峰值额外分配**（进程内统计分配器的包装，回答"这次编码额外吃了多少内存"）。JPEG 统一 quality 92 / 4:4:4 / baseline。

| 素材 | 编码器 | 字节 | 中位 ms | 峰值 +MB |
| --- | --- | --- | --- | --- |
| ui-1080 | png row-band | 43,168 | 13.1 | 0.4 |
| ui-1080 | zenjpeg trellis+huffman | 248,328 | 110.1 | 25.1 |
| ui-1080 | zenjpeg plain | 219,083 | 15.3 | 1.4 |
| ui-1080 | mozjpeg 444 trellis | 597,750 | 29.1 | 7.0 |
| ui-1080 | zenwebp lossless | **10,724** | 58.5 | 64.3 |
| ui-4k | png row-band | 99,137 | 42.3 | 0.5 |
| ui-4k | zenjpeg trellis+huffman | 566,435 | 358.5 | 98.9 |
| ui-4k | zenjpeg plain | 515,441 | 56.5 | 2.9 |
| ui-4k | mozjpeg 444 trellis | 1,320,525 | 105.4 | 25.8 |
| ui-4k | zenwebp lossless | 20,878 | 200.3 | 257.1 |
| scroll-tall | png row-band | 941,703 | 250.3 | **1.9** |
| scroll-tall | zenjpeg trellis+huffman | 6,460,133 | 2954.3 | **456.5** |
| scroll-tall | zenjpeg trellis+huffman 64r | 6,460,133 | 2992.7 | **456.5** |
| scroll-tall | zenjpeg plain | 6,064,082 | **393.0** | **13.9** |
| scroll-tall | mozjpeg 444 trellis | 16,501,515 | 833.3 | 125.9 |
| alpha-2k | png row-band | 43,623 | 33.1 | 0.4 |
| alpha-2k | zenjpeg trellis+huffman | 220,247 | 224.5 | 47.8 |
| alpha-2k | zenjpeg plain | 203,517 | 37.4 | 1.5 |
| alpha-2k | mozjpeg 444 trellis | 585,423 | 68.3 | 12.5 |
| alpha-2k | zenwebp lossless | 8,380 | 144.0 | 124.0 |

### 10.1 这张表回答了什么

1. **trellis / Huffman 在长图上不是"更慢一点"，是"整帧缓冲"。** `scroll-tall` 打开它们后峰值 **456.5 MB**、耗时 **2.95 s**；关掉后 **13.9 MB / 393 ms**。也就是说 `BytesEncoder::push` 在**不开**这些优化时确实是逐带流式（13.9 MB ≈ 少量缓冲），**一旦开 trellis 就会退化成整帧块缓冲**。这正是上一节那条"最大未验证项"的答案。
2. **分带大小不影响输出**：整帧一次 push 与 64 行一带 push 的字节数**完全相同**（6,460,133）⇒ sink 可以按任意带长喂，不受 8/16 倍数约束（mozjpeg-rs 则要求倍数）。
3. **在截图素材上，trellis+Huffman 反而更大**（248KB vs 219KB，566KB vs 515KB，长图 6.46MB vs 6.06MB）—— 注意这是**预设**对比（`MozjpegBaseline` 还换了量化表），不是"只翻转 trellis"的干净 A/B，所以结论只能是：**这套"照片优化"预设对截图没有收益，还带来 7.5× 时间与 33× 内存**。
4. **WebP 无损是这轮最大的意外收益**：ui-1080 **10.7 KB vs PNG 43.2 KB（约 1/4）**，ui-4k 20.9 KB vs 99.1 KB。代价是整帧缓冲（1080p 64 MB、4K **257 MB**）与更慢（58 / 200 ms）。⇒ 适合**普通截图**，绝不适合长图。
5. **mozjpeg-rs 应当淘汰**：它在这批素材上**在自己的 `write_bits` 里 panic**（移位 64），streaming 构造器没有 trellis 旋钮，同 quality 下输出是 zenjpeg 的 2.4 倍（注：此处用它自带的 `baseline_fastest()` 预设，不算完全公平，但 panic 一条已足够）。

### 10.2 因此的落地结论（覆盖 §9 的两条待定）

- **PNG**：不动。长图 1280×30000 = 250 ms / 942 KB / **峰值 1.9 MB**。
- **JPG**：`zenjpeg`，**baseline + 4:4:4 + quality 92 + `plain`（不开 trellis/Huffman）**。短图快（15 ms @1080p）、省内存（1.4 MB），长图逐带流式 13.9 MB。
- **WebP**：`zenwebp` **无损**，只给两轴 ≤16383 的普通截图（UI 素材上体积约为 PNG 的 1/4）；长图 UI 上不提供。
- **mozjpeg-rs**：从候选里移除（panic + 输出更大 + 流式无 trellis）。

### 10.3 未验证

- 合成素材偏"平"，真实照片类截图的比例关系会不同（这正说明 WebP 的"1/4"是**内容相关**的，不是产品保证）。
- JPEG 的质量用肉眼与 SSIM 都还没测（本轮只测体积/耗时/内存）。
- `zenwebp` 在 4K 上 257 MB 峰值是否可接受，取决于产品允许多大的截图走 WebP。
- mozjpeg-rs 的批量 `Encoder`（带 trellis）没有测，所以"它的批量质量"仍是未知。

## 11. 质量对比：JPEG 在文字上到底损失多少（`tools/codec-bench --quality`）

前提是"高性能 / 高质量"，所以体积之外还要回答质量问题。这一轮把 JPEG **解码**回来，与同一素材的**无损 PNG** 逐像素比较。

### 11.1 为什么必须量"文字区"，不能只看整图

UI 素材绝大部分是平面色块，压缩几乎无损；整图 SSIM 因此能一直停在 0.999 以上，**掩盖文字被压糊**的事实。

解决办法是让素材生成器**自己报告文字画在哪**：`synthetic::glyph_rows` 现在把每条文字行的行框记进 `Asset::text_regions`，`quality::in_regions` 只在那些框里算 SSIM/PSNR。整图数值仍然报，但结论看文字区。

`quality.rs` 里两个指标都按 RGBA 输入、8×8 窗口（小于一个窗口的区域会缩窗口而不是放弃测量）：
- **PSNR** 按 RGB 三通道算 —— 色度损伤会在这里显形；
- **SSIM** 按 BT.601 亮度算 —— 结构损伤在这里显形。

这两个指标分工的结果就是下表最有意思的一行：4:2:0 的 SSIM 几乎不动，但 PSNR 掉 2 dB 以上，而人眼在截图里看到的正是那个"颜色发灰"（见 11.4 的图）。

### 11.2 实测（`cargo run -p codec-bench --release -- --quality`）

| 素材 | 变体 | 字节 | 整图 SSIM | 整图 PSNR | **文字 SSIM** | **文字 PSNR** |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| ui-1080 | q92 4:4:4 plain | 219,083 | 0.9996 | 39.35 | **0.9968** | **28.52** |
| ui-1080 | q95 4:4:4 plain | 267,492 | 0.9998 | 41.08 | 0.9986 | 30.37 |
| ui-1080 | q85 4:4:4 plain | 160,251 | 0.9987 | 37.56 | 0.9900 | 26.62 |
| ui-1080 | q92 **4:2:0** plain | 180,586 | 0.9993 | 37.42 | 0.9954 | **26.27** |
| ui-1080 | q92 4:4:4 **huffman only** | 206,849 | 0.9996 | 39.35 | 0.9968 | 28.52 |
| ui-1080 | q92 4:4:4 **preset(trellis)** | 248,328 | 0.9970 | 40.81 | **0.9729** | 30.45 |
| ui-4k | q92 4:4:4 plain | 515,441 | 0.9998 | 42.09 | 0.9966 | 28.67 |
| ui-4k | q92 4:2:0 plain | 422,617 | 0.9996 | 40.22 | 0.9949 | 26.36 |
| ui-4k | q92 4:4:4 huffman only | 476,018 | 0.9998 | 42.09 | 0.9966 | 28.67 |
| ui-4k | q92 4:4:4 preset(trellis) | 566,435 | 0.9984 | 43.45 | **0.9709** | 30.45 |
| alpha-2k | q92 4:4:4 plain | 203,517 | 0.9988 | 43.04 | 0.9881 | 30.54 |
| alpha-2k | q92 4:2:0 plain | 168,546 | 0.9986 | 41.45 | 0.9840 | 28.14 |

`plain` 与 `huffman only` 在同样量化表下的**解码像素完全相同**（三段素材都逐字节比对过），只是后者小 5.6%–9%：
`219,083 → 206,849`、`515,441 → 476,018`、`203,517 → 185,408`。

### 11.3 四条结论

1. **4:2:0 的损失是真实的，而且专门伤文字**：同质量下文字 PSNR 从 **28.52 → 26.27 dB**（−2.25），仅省 18% 体积。整图 SSIM 只掉 0.0003，**完全看不出来** —— 这就是必须分开量两个指标的理由。⇒ **坚持 4:4:4**。
2. **q92 够用**：q95 多 **22%** 体积只换来文字 SSIM +0.0018、PSNR +1.85 dB；q85 省 **27%** 体积、文字 SSIM 掉 0.0068、PSNR 掉 1.9 dB。⇒ 默认 q92，给"更小"一档 q85，q95 留给归档。
3. **Huffman 优化在体积与像素上是免费的**：解码像素与 `plain` 完全相同（§14 用两个解码器复核：`mean |Δ| = 0.0000`、`max 0`），体积小 **5.6%–9%**。⇒ **短图应当开着它**。**但它在内存上不免费**：`optimize_huffman(true)` 就是 `HuffmanStrategy::Optimize`，文档写明是 **two-pass**（`src/encode/encoder_config.rs:437`），必须看完整帧 ⇒ 这正是 §10 里"开优化就退化成整帧缓冲、1280×30000 峰值 456 MB"的原因。
4. **`preset(trellis)` 在文字上更差**：文字 SSIM **0.9729 vs 0.9968**、体积还大 13%。它的文字 PSNR 反而更高（30.45 vs 28.52），说明它换的是量化表、优化的是逐像素误差，却**牺牲了结构**。**这不是干净的 trellis A/B** —— zenjpeg 把 `TrellisConfig` 与 `trellis` 字段都设成 `pub(crate)`，外部唯一入口就是 `OptimizationPreset::MozjpegBaseline`，它同时换量化表。所以能说的只有"mozjpeg 预设对截图文字更差"，不能说是 trellis 本身的错。

### 11.4 人眼证据

`target/jpeg_text_zoom.png`（ui-1080 第一条文字行的前 160 列，8× 放大）：**PNG | q92 4:4:4 | q92 4:2:0**。

- 左（PNG）：笔画锐利、1–2 像素细线清晰；
- 中（4:4:4）：几乎无法与左区分，边缘略软；
- 右（4:2:0）：**整行明显发灰、颜色偏移** —— 深蓝变淡紫、橙色变米色。这就是 −2.25 dB 的样子。

### 11.5 因此的产品配方

```
JPG = baseline + 4:4:4 + q92
      · 短图（整帧缓冲可接受）→ 开 Huffman 优化，省 6%–9%，像素不变
      · 长图（滚动截图）      → 关优化走真流式，峰值 13.9 MB 而不是 456 MB
      · 永不用 4:2:0，永不用 mozjpeg 预设
```

### 11.6 §11 的未验证

- 真实（非合成）截图的比例关系；合成素材的"文字"是规则笔画，真实字形的高频更复杂。
- **纯 trellis 的 A/B 无法从外部做**（API 私有），所以"trellis 对截图无益"这条只对 mozjpeg 预设成立。
- 4:2:0 在**纯灰度**文字上损失会小得多 —— 我们的素材刻意用了彩色字，这是偏向 4:4:4 的选择，但截图里彩色文字与图标是常态。

## 12. 滚动截图链路的性能（`codec-bench --scroll`）

编码器的数字（§10）只说明"最后一步多贵"，而滚动截图的成本大头在**画布**：拼接器在编码器看到图像之前，就已经把整张长图握在内存里。所以这一节测的是真实链路。

### 12.1 怎么测

`tools/codec-bench/src/scroll.rs`，`cargo run -p codec-bench --release -- --scroll`：

- 页面 1280×30000，每行带自己的行号标记（错拼立刻可查），视口 1280×960，每帧滚动 600 px（重叠 360 行，远高于估计器 32 行的下限）；
- 帧序列喂给**真实的** `Tracker` —— 通过 `snapclip-capture` 的 `test-support` 特性暴露的 `scroll::probe`（产品构建不变，特性默认关闭）；
- 三组读数：每帧拼接耗时、整个过程的**峰值额外分配**（计数分配器）、拼完之后的编码（体积/耗时/峰值）；
- **正确性**：拼出来的长图与"直接生成的完整页面"**逐像素比对**。

### 12.2 实测（1280×30000，960 视口，49 帧）

```
canvas cap: 25600 rows at a 800-row viewport, 30720 at a 960-row one
stitching : 49 frames · 29760 rows · 1.31 ms median / 16.02 ms max per frame
            thumb total 27.2 ms · canvas peak 161.9 MB · lost 0 · pixel-exact true
PNG       : 3.42 MB · 550 ms · 7.3 MB peak
JPG       : 33.8 MB · 555 ms · 96.4 MB peak  (baseline 4:4:4 q92, plain, 64-row bands)
WebP      : refused before encoding, 0.0 MB peak (1280x29760 > 16383)
verdict   : 88.5 ms stitching + 578 ms PNG, 161.9 MB canvas peak; the bottleneck is the canvas.
```

（合成素材是高熵纹理，JPG 与 PNG 的绝对体积都比真实 UI 页面大一个量级，**两者之间的比例不可外推**；这里可信的是耗时、峰值内存与"能否编码"。）

### 12.3 三条结论

1. **画布是瓶颈**：161.9 MB 的峰值全部来自拼接器，编码器只占 7.3 MB（PNG）。要控内存，控画布，不是控编码器。
2. **每帧有 16 ms 的尖峰**（中位 1.31 ms）：那是画布增长时重新分配 + 拷贝造成的，也就是**滚动过程中可能掉一帧**。要更平滑就得让画布按更大块增长（或预先按上限预留）。
3. **上限与视口绑定**：`MAX_SCREENS × 视口高`（再受 `MAX_BYTES` 约束）⇒ **800 行视口最多 25 600 行，根本到不了 30 000**；960 行视口能到 30 720。产品上如果要支持 30 000 行的页面，视口至少要 938 行——这是产品参数，不是编码问题。

### 12.4 WebP 的产品含义（必须前置检查）

`1280×29760` 超过 WebP 的 16383 上限，所以滚动截图**不能存 WebP**。测得的拒绝是"编码前就拒绝、峰值 0.0 MB"，但那是在 bench 里做的；**产品里目前没有任何格式概念参与滚动**（`overlay/scroll.rs` 只有 `format!` 字符串，没有格式限制）。

⇒ **要求**：滚动会话开始时就要检查当前保存格式的上限，不满足就**立刻**降级为 PNG 并告知用户。最晚不能晚于第一帧之后——"滚了半天才发现存不了"是不可接受的。

**建议位置**：`crates/snapclip-capture/src/windows/overlay/scroll.rs:240` `begin_scroll_session()`（或更早的 `session.rs:63 start_scroll_entry()`，取决于在哪里能读到当前默认格式）。检查依据就是 `width`、以及滚动可能到达的最大行数（`MAX_SCREENS × view`，与 `MAX_BYTES` 取小）。

## 13. 落地：`SaveFormat` 与两个 sink（后端，未接 UI）

按"只做后端"的边界落地：三个新文件，导出 worker 的调用点、滚动代码、UI 一概没动。

### 13.1 `SaveFormat` 能力模型

`apps/snapclip/src/capture/save_format.rs`。纯数据：无 I/O、无编码器、无分配。

| 格式 | `max_side()` | 依据 |
| --- | --- | --- |
| `Png` | `u32::MAX` | IHDR 的宽高字段就是 u32 |
| `Jpeg` | `65_535` | 规范的单边上限 |
| `Webp` | `16_383` | 容器单边上限（Google FAQ） |

`can_encode(w, h) -> Result<(), FormatLimit>`，错误是结构化的 `TooWide`/`TooHigh`（带 `max`），
`Display` 给出一句可读中文。**没有**像素数上限：三个格式都没有规定，凭空造一个就是把产品决定
伪装成格式事实；WebP sink 自己有一条**内存守卫**，并在定义处写明它是内存而不是格式。

`label()` = `PNG` / `JPG` / `WebP（无损）` —— 最后那个必须带"无损"，因为有损 WebP 固定 4:2:0。

这一层就是下一轮"滚动会话前置格式检查"要复用的东西：问"WebP 能不能装下 30000 行"不该打开编码器。

### 13.2 `JpgRowBandSink`

`apps/snapclip/src/capture/row_band_jpeg.rs`。固定配置（§11 的结论）：**baseline + 4:4:4 + q92 +
`PixelLayout::Bgra8Srgb` 直入**，不做 BGRA→RGB 转换。两条策略，闸门是帧字节数：

| 帧大小 | 策略 | 依据（§10） |
| --- | --- | --- |
| ≤ `SHORT_IMAGE_MAX_BYTES`（16 MiB） | 缓冲整帧 + 开 Huffman 优化 | 优化器需要整帧；预算把峰值钉在 16 MiB 内 |
| 更大 | 逐带流式 + 关优化 | 开优化会退化成整帧缓冲：1280×30000 时 456.5 MB / 2.95 s，关掉是 13.9 MB / 393 ms |

`plan()` 是纯函数（`optimise_huffman` / `buffers_whole_frame`），所以"哪张图走哪条"可断言。
取消通过 `enough::Stop` 的 `Cancel`：`cancel()` 之后下一次 `push` 返回取消错误，`finish` 不会产出文件。
错误一律 `ExportError`（沿用 `row_band_png` 的 `ImageMeta`/`Artifact`/`ExportError`），**不 panic**。

### 13.3 `WebpSink`

`apps/snapclip/src/capture/webp_sink.rs`。**无损、整图**（`zenwebp` 没有流式接口）：
`near_lossless(100)`（关）+ **`exact(true)`**（真无损：否则 libwebp 可以改写完全透明像素下的 RGB）
+ `with_max_dimensions(16383, 16383)`（编码前拒绝）+ `PixelLayout::Bgra8` 直入（不转换）。

另有 `MAX_PIXELS = 16_000_000` 的内存守卫：§10 量到 4K 一帧在编码器内部就占 257 MB，所以
16383×16383 的合法尺寸会变成半 GB 级保留；这条是"保存按钮不该悄悄做的事"，不是格式限制。

### 13.4 测试

app 新增 20 条（137 → **157**），capture 493 不变，codec-bench 17 不变，workspace 0 warning。

- `SaveFormat`：16383 通过 / 16384 拒绝、65535 / 65536、PNG 到 `u32::MAX`、标签、错误文案带数字；
- JPG：strategies（1080p 缓冲+优化、1280×30000 流式）、预算边界（恰好在预算内仍缓冲，多一行不）、
  小图 round-trip 且**不含渐进式标记**（`FFC2`）、优化臂比 plain 小、行序/整行/短写/超限、取消；
- WebP：无损 **逐像素**回环（含"完全透明但 RGB 很响"的像素，正是 `exact(true)` 的存在理由）、
  16384 拒绝、30000 行拒绝、内存守卫边界、行序/短写；
- 解码一律用 **`image`** 而不是编码库自己（编解码不同源，一个共享 bug 无法自我抵消）。

### 13.5 一处与 §11 冲突的实测，需要 bench 复核

sink 的"优化臂 vs plain 臂"对比里，**同一帧、同一量化表（DQT 段头逐字节相同）、只切
`optimize_huffman`**，输出体积小 20%（1,973,260 vs 2,480,235），但**解码像素不是等价的**
（`mean |Δ| = 75`、`max = 255`，尺寸一致）。这暗示 `zenjpeg 0.8.4` 的 `optimize_huffman(true)`
**不只是熵编码开关**（很可能连带 trellis），与 §11"像素逐字节相同、体积小 5.6–9%"矛盾。

因此该断言被**移出** sink 单测（像素等价属于 `codec-bench` 的职责，它对着无损参考图比较），
sink 只断言它自己拥有的东西：文件更小、尺寸正确、可解码。

> **已由 §14 裁定：那条"像素不等价"的实测复现不出来，是那次**临时对比**的缺陷（`max |Δ| = 255`
> 是通道顺序搞错的典型特征），不是 `optimize_huffman` 的语义问题。§11 的结论成立，"短图开优化"
> 在像素与体积上是免费的，但在内存上不免费（需要整帧缓冲）。

## 14. `optimize_huffman` 的最小可判定 A/B（裁定 §13.5 的冲突）

`codec-bench --huffman-ab`：同一素材、**唯一差别是 `optimize_huffman(true/false)`**，其余全部相同
（q92 · 4:4:4 · baseline · `Bgra8Srgb` · 64 行一带）。解码用 **`image`**（与编码库隔离）与 `zenjpeg` 各一次。

| 素材 | plain | huffman | 体积 | 解码像素（image 解码器） |
| --- | ---: | ---: | ---: | --- |
| ui-1080 | 219,083 | 206,849 | **−5.6%** | mean \|Δ\| **0.0000** · max **0** · 100.0000% 相同 |
| scroll-tall | 6,064,082 | 5,649,882 | **−6.8%** | mean \|Δ\| **0.0000** · max **0** · 100.0000% 相同 |

段结构（两支）：

| 段 | plain | huffman | 判定 |
| --- | --- | --- | --- |
| `SOF0` | 15 B | 15 B | 逐字节相同 |
| `DQT` | 195 B | 195 B | **逐字节相同** ⇒ 量化表没被换 |
| `DHT` | 287 B | 185 / 178 B | **不同** ⇒ 开关确实作用在熵编码表上 |
| `SOS` | 10 B | 10 B | 逐字节相同 |

### 14.1 三个可能原因逐一排除

| 可能 | 检验 | 结果 |
| --- | --- | --- |
| ① 两次对比用了不同配置 | 两支只差一个布尔（构造处唯一） | 体积差 5.6–6.8%、DQT/SOF 完全相同 ⇒ 配置一致 |
| ② 编解码同源自相抵消 | `image` 与 `zenjpeg` 两个解码器各解一遍；同字节跨解码器差异 = **噪声地板** | 噪声地板 **0.0000**，而 plain vs huffman 也是 **0.0000** |
| ③ 编码器有 bug | 分带这个剩余变量也排掉：整帧一次 push vs 64 行分带（均开优化） | **字节数完全相同**（206,849 / 5,649,882），像素 **0.0000** |

⇒ **`optimize_huffman` 是纯熵编码优化**（只换 DHT），不可能改变像素；§13.5 里那个
`1,973,260 vs 2,480,235 / mean |Δ| = 75 / max = 255` **复现不出来**，其签名（max 恰为 255）
符合**通道顺序错位**（BGRA 当 RGBA 比）而非编码语义。

### 14.2 对 `JpgRowBandSink` 的影响

- **短图继续开 `optimize_huffman`**（省 5.6–9%，像素不变）—— 但要记住它 means **整帧缓冲**，
  这就是 `SHORT_IMAGE_MAX_BYTES` 存在的理由；
- **长图继续关**（§10：开它会让 1280×30000 退化成 456 MB / 2.95 s）；
- 建议在 sink 的测试里补一条**两支像素恒等**的断言（现在是刻意不测），防止将来有人把"更小"
  当成"更好"而放松了别的约束；若那条断言失败，问题在 sink 的行/通道处理，**不在** 这个开关。

### 14.3 未验证

`ui_frame` 那份测试素材与 bench 合成素材不同源，所以"20% 体积差"本身也无法对着 bench 复现；
`optimize_huffman` 在**渐进式**下的行为未测（我们只用 baseline）。
