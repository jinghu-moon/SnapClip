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

**落地前必须先核**（本轮未验证）：`mozjpeg-rs` 的 **streaming API** 是否逐行、是否支持 **4:4:4 / 非渐进 / BGRA 直接输入**。若它只支持整图编码 ⇒ 对 1280×30000 也不致命（图像本来就在内存里，见 §2.2 的口径），但会**多一次整图 RGB 缓冲**（180 MB 级），需要按 `P4.03` 的"零整图拷贝"纪律重新评估。

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
