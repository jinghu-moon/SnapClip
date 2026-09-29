# 通用 Rust OCR Crate 设计评审

> **评审对象**：`docs/04-generic-ocr-crate-design.md`  
> **场景约束**：Windows 10/11 桌面 · 屏幕截图 OCR · 性能优先  
> **交叉资料**：PaddleOCR / PP-OCRv6、RapidOCR、ort、ONNX Runtime DML、PowerToys Text Extractor、DXGI/WGC、本地 `refer/paddle-ocr-rs-main`

---

## 1. 结论摘要

| 项 | 判断 |
|---|---|
| **一句话** | 作为「可复用 PP-OCR 引擎库」合格；作为「Windows 截图 OCR 产品管线」不完整，须补截图输入层与屏幕预处理 |
| **是否适合 Windows 截图 OCR** | **方向正确，需定向瘦身**（算法层适合，缺屏幕特有环节） |
| **是否继续投入** | 适合；与 SnapClip 可插拔引擎路线一致 |
| **当前成熟度** | 设计基线 / P0 前 |
| **最大风险** | 无端到端延迟预算 + 无屏幕文本预处理 + 测试缺截图专项 |
| **最优先改进** | 输入元数据（BGRA/DPI/stride）、默认档改 small 候选、黄金集 CER + warm P95 门禁 |

**批准 P0 的三条硬约束：**

1. 公共 API 增加屏幕输入元数据（`bgra` / `dpi_scale` / `row_stride`）  
2. 默认档倾向 **small**（medium=一致性基线、tiny=吞吐档），须本机基准确认  
3. P0 验收必须含**截图黄金集 CER** 与 **warm P95**

---

## 2. 资料依据

| 来源 | 用途 | 关键结论 |
|---|---|---|
| `docs/04-generic-ocr-crate-design.md` | 评审对象 | PP-OCRv6 medium 首发；ort；core/ppocr/runtime 分层；P0–P4 |
| PaddleOCR / PP-OCRv6 文档 | 模型核验 | tiny 1.5M / small 7.7M / medium 34.5M；multi 统一多语言；相对 v5 检测 +4.6%、识别 +5.1%（与文档一致） |
| RapidOCR + MODEL_LICENSES | 部署与许可 | det/cls/rec 可关 cls；模型版权与 Apache-2.0 代码分开——文档 §13 正确 |
| pykeio/ort · ORT DML EP | 运行时 | Session 常驻、EP 配置、DirectML；RC 需锁定（文档已提 2.0.0-rc.10） |
| PowerToys Text Extractor | 产品参考 | 快捷键 + 区域截图 + 本地 OCR → 剪贴板；亚秒级反馈是桌面标配 |
| DXGI Duplication / WGC | 截图路径 | WGC 窗口级、兼容好；Duplication 延迟更低、实现复杂；BitBlt 适合小区域 |
| `refer/paddle-ocr-rs-main/` | 本地算法参考 | det/rec/cls/pipeline/runtime/vision 模块可借鉴；CLI/下载不进核心 crate |
| SnapClip `docs/03` + OCR 实现 | 落地约束 | 已有状态机/CAS/Windows OCR worker；新 crate 应可替换现有引擎 |

**资料不足**：PP-OCRv6 在「屏幕 UI 小字 / ClearType」的公开专项指标很少；文档 SHA-256 与 medium 延迟须本机复测。  
**推断**：medium 34.5M 在 CPU 全图检测上难以满足「快捷键后几百毫秒」，必须实测 small/tiny 作默认。

---

## 3. 设计文档核心理解

| 维度 | 定位 |
|---|---|
| 项目目标 | 从 SnapClip 抽离的**本地可复用 OCR crate**（算法 + 模型加载 + 推理），不依赖 Tauri/SQLite/剪贴板 |
| 本质 | **PP-OCR ONNX pipeline 封装库**，不是截图工具，也不是多引擎聚合层 |
| 核心模块 | `ocr-core` / `ocr-ppocr` / `ocr-runtime` / `ocr-models` / 可选 `ocr-cli` |
| 关键抽象 | `OcrInput` / `OcrEngine` / `OcrOutput` / `OcrLine` / `ModelManifest` / `ProviderPreference` / `OcrError` |
| 首发范围 | PP-OCRv6 det+rec medium/small/tiny，cls 关闭；CPU 优先，DirectML 预留 |
| 与截图匹配度 | **中等**：有 RGB/RGBA 缓冲，缺 DPI、BGRA、stride、GPU 纹理等屏幕细节 |

**判断**：文档定义为「引擎层」合理；产品要做屏幕 OCR，必须在适配层或新增 `ocr-screen` 补截图与屏幕预处理。

---

## 4. 逐维度评估

### 4.1 架构设计

| 项 | 现状 | 风险 | 建议 | 截图适用 |
|---|---|---|---|---|
| 模块分层 | core/ppocr/runtime 边界清晰 | 低 | 保持；增加可选 `ocr-screen` | 适合 |
| 跨平台抽象 | Windows+Linux，无移动端负担 | 中 | Windows 路径一等公民；允许 BGRA/COM 输入 | 适合 |
| Windows 深度优化 | 仅 DirectMl preference | **高** | `#[cfg(windows)]` 快速路径：WIC、BGRA 零拷贝 | 需补 |
| 可测试性 | fixture/mock 友好；`&mut self` 串行 | 低 | 阶段纯函数单测 | 适合 |
| unsafe 边界 | 未显式约定 | 中 | unsafe 仅限 runtime/screen；对外安全 API；COM RAII | 需约定 |
| 同步/异步 | 仅同步 `recognize` | 中 | 核心同步 + 应用层 worker（SnapClip 已有） | 可接受 |
| 错误模型 | 解码/模型/provider/取消 | 低 | 补 `Timeout`、`UnsupportedInput`；映射 SnapClip error_code | 基本适合 |

**建议目录（Windows 优先）：**

```text
crates/
  ocr-core/        # 类型、错误、manifest、安全公共 API
  ocr-ppocr/       # det/rec pipeline（纯计算）
  ocr-runtime/     # ort Session、EP、取消；DirectML
  ocr-screen/      # 可选：BGRA/WIC/DPI 预处理（Windows 一等）
  ocr-models/      # manifest / 校验 / 可选 fetch
SnapClip adapter   # 队列/DB/UI
```

### 4.2 Windows 屏幕截图场景

| 能力 | 文档 | 缺口 | 建议 |
|---|---|---|---|
| PNG/JPEG/RGB/RGBA | 有 | 缺 stride 文档 | 明确 `row_stride`、通道顺序 |
| 剪贴板 BGRA/DIB | 无 | 零拷贝路径无 | `ImageSource::Bgra` + 长度校验 |
| 区域/窗口/显示器截图 | 外置（合理） | 产品延迟关键 | 应用层 WGC/Duplication，crate 只收像素 |
| DPI / Per-Monitor | 无 | 高 DPI 小字退化 | 输入携带 `dpi_scale` |
| 深色/反色/ClearType | 无 | 漏检/低分 | 反色、对比度、小字增强 |
| 受保护内容黑屏 | 无 | 无效输入 | 应用层检测纯黑帧 |

**结论**：当前是「通用 ONNX OCR 库」，不是「屏幕 OCR 管线」。快捷键区域识别的最大风险是 medium 全图 det 延迟 + 无屏幕预处理。

### 4.3 性能与识别速度

**已正确规划**：Session 常驻、warm、分段耗时、长边 1920、scratch buffer、rec 按宽高比 batch、禁止无基准吹 DirectML。

**缺失**：

- 端到端延迟预算（区域截图建议目标 P95 &lt; 300–500ms，实测定档）
- 冷启动/加载与首帧拆分门禁
- 区域小图快速路径
- 连续/批量截图吞吐与背压
- 内存峰值（medium + ORT + 大图解码）

| 场景 | 期望 | 建议 |
|---|---|---|
| 区域 500×200 | &lt;300ms 理想 | small/tiny + 预热 |
| 剪贴板 1080p | &lt;1s | small/medium + 后台 worker |
| 4K 全屏 | 1–2s | downscale + 分块 det |
| 连续截图 | 不阻塞 UI | 队列 + 取消 + 去重 |

**推断**：tiny@2048 CPU ONNX 约 317ms（公开数据），medium 明显更慢。桌面默认应倾向 **small**。

### 4.4 通用性

「通用」= 截图场景内输入/引擎扩展，不是无限跨平台。

- **必要**：文件/字节/RGB(A)、BGRA/stride/DPI、三档 profile  
- **可插件化不首做**：Tesseract/Windows OCR/candle  
- **禁止**：在线下载为运行时主路径、表格/公式/VLM 进核心（同意文档）

### 4.5 识别精度

| 类型 | 难度 | 对策 |
|---|---|---|
| 清晰 UI/菜单 | 低 | 默认 pipeline |
| 中英混排/标点 | 中 | multi 有利；空白/标点后处理 |
| 代码/终端 | 中高 | 符号词典、code mode、慎合并行 |
| 小字号/高 DPI | **高** | DPI-aware 上采样、小字增强 |
| 深色/反色 | **高** | 自适应反色 + 对比度 |
| 表格/多列/长网页 | 中高 | 阅读顺序、保留 polygon |

**建议屏幕预处理：**

```text
BGRA/PNG
  → 灰度可选 / 浅字深底检测
  → 自适应反色或对比度拉伸
  → 小字上采样
  → det → crop(pad) → rec
  → CTC 后处理 → 阅读顺序排序
```

### 4.6 生态兼容

| 目标 | 适配 | 说明 |
|---|---|---|
| Tauri/SnapClip | 高 | §11 适配器正确，可替换现有 `OcrEngine` |
| CLI/后台/取词 | 高 | 无 UI 依赖 |
| feature 裁剪 | 中 | CPU 默认；dml/cuda/download/cli 已规划 |
| 离线模型 | 高 | manifest + SHA256 亮点 |

### 4.7 测试与识别能力

**已有**：输入校验、EXIF、超大图、manifest 哈希、tensor 契约、CTC、polygon、CPU fallback、取消；集成 fixture、warm 稳定；基准分段与三档对比。

**必须补强：**

| 层 | 内容 | 时机 |
|---|---|---|
| 精度集 | UI/代码/终端/深色/小字/中英/表格 ≥40 黄金样本 | P1 前 |
| 指标 | CER/WER/行召回/IoU/小字/数字/URL | P1 |
| 鲁棒 | JPEG 二次压缩、低对比、反色、阴影、图标混排 | P1–P2 |
| 边界 | 空图/损坏/超大/无文本/模型缺失/取消/超时 | P0–P1 |
| 性能门禁 | warm P95、峰值内存、加载回归 | P2 |
| CI | win-latest + small/tiny smoke；DML 本地 | P2 |

**门禁**：单元/契约进 PR CI；medium 不进 PR；精度集 + P95 作 release 门禁。

---

## 5. Windows 原生优化

### 截图捕获（应用层）

| API | 延迟 | 适用 | 注意 |
|---|---|---|---|
| BitBlt/GDI | 中 | 小区域、兼容 | CPU 拷贝 |
| DXGI Duplication | 低 | 全屏/高刷 | 复杂，需 GPU |
| WGC | 低 | 窗口/区域 | 现代，受保护内容仍可能无效 |
| PrintWindow | 高 | 特例 | 兼容差 |

**产品建议**：快捷键区域 → WGC 或 BitBlt 小矩形；连拍/全屏 → Duplication。**禁止落盘再读**，BGRA 直接进 `OcrInput`。

### 像素与推理

- WIC 解码优先，失败回退 `image`  
- BGRA→RGB：`chunks_exact` + 缓冲复用；预处理 rayon  
- Session/tensor 常驻复用；CPU EP 调线程；DML opt-in  
- unsafe 集中在 runtime/screen；COM RAII（可参考 SnapClip `ComApartment`）

---

## 6. 瓶颈分析（按严重度）

| # | 瓶颈 | 级别 | 影响 |
|---|---|---|---|
| 1 | 无截图→识别端到端延迟预算 | P0 | 无法验收快捷键体验 |
| 2 | medium 全图 det 默认路径 | P0 | CPU 延迟/内存超标风险 |
| 3 | 缺屏幕预处理 | P0 | 深色 UI、高 DPI 精度不稳 |
| 4 | 输入缺 DPI/BGRA/stride | P1 | 无法零拷贝与 DPI-aware |
| 5 | 测试未覆盖截图多样性 | P1 | 回归靠肉眼 |
| 6 | 取消仅阶段边界 | P1 | 退出等待 |
| 7 | DirectML 未验证 | P2 | GPU 收益未知 |
| 8 | ort RC 漂移 | P2 | 构建/ABI（文档已要求锁定） |

---

## 7. 优化建议

### 短期（P0）

1. `OcrInput` 增加 BGRA / dpi_scale / row_stride  
2. 默认 profile：small 候选、medium 基线、tiny 吞吐  
3. `OcrTimings` 强制输出并进日志/metrics  
4. 20+ 屏幕黄金样本 + CER 门禁  
5. warm P95 criterion 基准脚本  
6. SnapClip adapter 对接现有 worker（取消/错误码）

### 中期（P1）

- 反色/对比度/小字增强；深色模式评测  
- DPI 100/150/200%；多显示器；剪贴板 BGRA  
- ORT 超时/取消完善；Windows CI + 模型校验制品  

### 长期（P2）

- DirectML 实测与 fallback；自动档位  
- 区域快速模式；表格阅读顺序；热词  
- `ocr-cli`、THIRD_PARTY_NOTES 发布流程  

### 对文档的最小改动

1. §7.1 `OcrInput` 扩展屏幕元数据；§8.1 增加屏幕预处理可选步骤  
2. §12 集成测试改为「截图黄金集 + CER/P95」  
3. §14 P0 增加「SnapClip 对接 + 默认 small」决策  
4. §15 补充：medium=基线，small=默认候选，tiny=吞吐（须实测）  

---

## 8. 测试与评测方案（摘要）

```text
单元 (跨平台 CI)     校验、CTC、polygon、manifest、预处理核
契约 (mock)          I/O shape、provider fallback、cancel
集成 (模型 fixture)  golden 截图、warm 稳定、离线启动
基准 (nightly)       size × profile × provider 矩阵
专项 (真实桌面)      DPI、深色/浅色、多显示器、剪贴板 BGRA
```

指标：CER、WER、行 Precision/Recall、IoU、小字召回、数字/URL/路径准确率、warm P50/P95、峰值内存、加载时间。

---

## 9. 参考方案

**值得借鉴**

- PowerToys Text Extractor：快捷键 → 小区域 → 本地 OCR → 剪贴板  
- RapidOCR / Umi-OCR：cls 可关、离线模型包、桌面打包  
- ORT EP 纪律：初始化解析 provider 并记录 fallback  
- 本地 paddle-ocr-rs 模块切分  

**不建议照搬**

- Python Paddle 全量依赖  
- 在线下载作运行时默认  
- 移动端/WGPU 大而全抽象  
- 每图新建 Session / 临时文件 OCR  

---

## 10. 优先级路线图

### P0 — 截图主路径可交付

- [ ] `OcrInput` 屏幕元数据  
- [ ] 默认 small / medium 基线决策（基准确认）  
- [ ] 阶段耗时强制输出  
- [ ] 截图黄金集 + CER  
- [ ] warm P95 基准  
- [ ] SnapClip adapter  

### P1 — 屏幕精度与稳定

- [ ] 屏幕预处理（反色/小字/对比度）  
- [ ] 深色模式专项  
- [ ] DPI / 多显示器 / 剪贴板 BGRA  
- [ ] 取消与超时  
- [ ] Windows CI  

### P2 — 高性能与生态

- [ ] DirectML 实测  
- [ ] 自动档位 / 区域快速模式  
- [ ] 表格顺序、热词  
- [ ] ocr-cli + 许可证清单发布  

---

## 11. 最终意见

**批准进入 P0，附加三条硬约束：**

1. **屏幕输入元数据进入公共 API**  
2. **默认档以 small 为候选、medium 仅基线**（须本机基准）  
3. **P0 验收含截图黄金集 CER 与 warm P95**

在此约束下，该 crate 可作为 SnapClip 及后续取词/翻译/搜索工具的可靠底座。

---

*报告依据：`docs/04-generic-ocr-crate-design.md` + 公开资料交叉验证 + 本地 `refer/paddle-ocr-rs-main`。未证实处已标注「推断 / 资料不足」。*
