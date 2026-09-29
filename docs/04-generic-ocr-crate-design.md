# 通用 Rust OCR crate 设计方案

## 1. 文档信息

| 项目 | 内容 |
|---|---|
| 文档状态 | 设计基线，待进入 P0 实现 |
| 目标 | 建设可被 SnapClip 及其他 Rust 项目复用的本地 OCR crate |
| 首发模型 | PP-OCRv6 multi medium |
| 首发运行时 | ONNX Runtime，通过 `ort` crate 接入 |
| 首发平台 | Windows、Linux；Windows 优先验证 CPU/DirectML |
| 首发输入 | PNG/JPEG 字节、RGB/RGBA 内存图像、文件路径 |
| 首发输出 | 文本、置信度、行框、检测框、阶段耗时 |

本方案把 OCR 算法、模型加载和推理运行时从 SnapClip 中抽离。SnapClip 只负责剪贴板来源、任务队列、数据库和 UI；通用 crate 不依赖 Tauri、SQLite、剪贴板或任何具体应用。

## 2. 结论摘要

首个完整验证组合固定为：

```text
PP-OCRv6_det_medium.onnx
PP-OCRv6_rec_medium.onnx
ppocrv6_dict.txt
```

首版关闭方向分类：

```text
det = enabled
cls = disabled
rec = enabled
```

原因是 PP-OCRv6 注册表提供检测和识别模型，没有对应的 v6 分类模型。第一版不混入 PP-OCRv4 分类模型，避免跨版本组合和额外推理开销。

`medium` 是一致性和准确率基线，不代表最终默认性能配置。使用同一套 pipeline 另外注册 `small` 和 `tiny`，完成基准后由应用选择默认档位。

## 3. 外部资料结论

### 3.1 PaddleOCR

PaddleOCR 官方仓库当前说明：

- PP-OCRv6 使用单一统一模型覆盖中文、英文、日文和其他拉丁文字语言；
- 提供 tiny、small、medium 三档模型；
- medium 约 34.5M 参数，small 约 7.7M，tiny 约 1.5M；
- PP-OCRv6 相对 PP-OCRv5 有检测和识别精度提升；
- PP-OCR 系列支持通过 ONNX Runtime 等后端部署；
- PaddleOCR 项目代码采用 Apache-2.0。

来源：

- <https://github.com/PaddlePaddle/PaddleOCR>
- <https://www.paddleocr.ai/latest/en/version3.x/pipeline_usage/OCR.html>
- <https://paddlepaddle.github.io/PaddleOCR/latest/en/version3.x/inference_deployment/others/obtaining_onnx_models.html>

### 3.2 RapidOCR

RapidOCR 官方仓库说明：

- 其核心目标是将 PaddleOCR 模型转换为 ONNX，便于离线和跨平台部署；
- 默认使用检测、分类、识别三阶段 pipeline，但分类阶段可以按配置关闭；
- RapidOCR 代码采用 Apache-2.0；
- 模型权重源自 PaddleOCR，模型的版权和再分发信息需要单独记录。

来源：

- <https://github.com/RapidAI/RapidOCR>
- <https://github.com/RapidAI/RapidOCR/blob/main/MODEL_LICENSES.md>

### 3.3 ONNX Runtime 与 Rust `ort`

ONNX Runtime 官方项目定位为跨平台推理运行时，支持图优化和硬件加速。`ort` 是 Rust 接口层，支持加载 ONNX 模型并配置 execution provider。

来源：

- <https://github.com/microsoft/onnxruntime>
- <https://github.com/pykeio/ort>
- <https://ort.pyke.io/>
- <https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html>

本地参考项目 `refer/paddle-ocr-rs-main` 已验证以下组合：`ort` Session、DB 检测、CTC 识别、纯 Rust 图像预处理、CPU provider 和 DirectML provider 配置。新 crate 可以复用其算法思路，但不直接暴露其 CLI、在线下载和应用级配置。

## 4. 目标与非目标

### 4.1 目标

1. 提供独立、可测试、可嵌入的 Rust OCR API。
2. 通过模型包支持 PP-OCRv6 medium/small/tiny，而不复制 pipeline。
3. 支持离线模型、模型校验和可重复部署。
4. 支持 CPU，并为 DirectML、CUDA 等 provider 保留扩展点。
5. 返回结构化文本和几何信息，供搜索、标注和布局 UI 使用。
6. 允许单个常驻 engine 多次 warm inference，避免每张图片重新加载模型。

### 4.2 首版不做

- Tauri、Vue、SQLite、剪贴板监听；
- 云端 OCR；
- Python sidecar 或外部 OCR 进程；
- 文档表格/公式/VLM 解析；
- 在线模型下载作为核心运行路径；
- 自动训练或模型微调；
- 强制启用方向分类。

## 5. 模型策略

### 5.1 模型包而不是单一模型

OCR pipeline 至少由检测模型、识别模型和字符字典组成。crate 不接受“只给一个 det 模型就能完成 OCR”的假设。

```text
OcrModelBundle
  ├── detector
  ├── recognizer
  ├── dictionary
  ├── optional classifier
  └── manifest
```

首发模型清单：

| profile | detector | recognizer | 用途 |
|---|---|---|---|
| `ppocrv6-multi-medium` | `PP-OCRv6_det_medium` | `PP-OCRv6_rec_medium` | 准确率/一致性基线 |
| `ppocrv6-multi-small` | `PP-OCRv6_det_small` | `PP-OCRv6_rec_small` | 日常桌面默认候选 |
| `ppocrv6-multi-tiny` | `PP-OCRv6_det_tiny` | `PP-OCRv6_rec_tiny` | 低配置和高吞吐候选 |

这三个 profile 共享完全相同的 pipeline 和输出类型。模型差异只通过 manifest 和模型文件表达。

### 5.2 模型 manifest

```rust
pub struct ModelManifest {
    pub id: String,
    pub family: String,
    pub version: String,
    pub languages: Vec<String>,
    pub detector: ModelArtifact,
    pub recognizer: ModelArtifact,
    pub dictionary: ModelArtifact,
    pub classifier: Option<ModelArtifact>,
}

pub struct ModelArtifact {
    pub file_name: String,
    pub sha256: String,
    pub source_url: Option<String>,
}
```

`source_url` 只用于模型清单和开发工具，不在 OCR 主线程中访问网络。

PP-OCRv6 medium 的参考 SHA-256：

```text
det: 92078b7355007ccfffcd4c8cd441a3afd4538904d06881b29a155e1e679907c2
rec: eef444829dbbe18d7fea59a3f6eb75647518d2b3a9568d27c92e42940204894b
```

### 5.3 模型文件分发

核心 crate 不内置大模型二进制，也不默认在线下载。应用可以选择：

1. 安装包附带模型目录；
2. 首次安装器下载并校验模型；
3. 用户通过设置选择模型目录；
4. 测试使用仓库外部 fixture 目录。

模型下载器应是独立的 `model-fetch` 工具或可选 feature，不能使 `OcrEngine::recognize` 隐式触网。

## 6. Crate 分层

建议使用 workspace，但保持核心 crate 可独立发布：

```text
crates/
  ocr-core/       # 输入、输出、错误、模型包、公共 trait
  ocr-ppocr/      # PP-OCRv6 det/rec pipeline
  ocr-runtime/    # ort Session、provider、线程和模型校验
  ocr-models/     # 可选 manifest、下载与校验工具
  ocr-cli/        # 可选命令行，不进入核心依赖
```

如果初期不希望拆成多个发布 crate，可以先使用一个 `local-ocr` crate，内部仍按上述模块边界组织。不要把 Tauri 适配器放进核心 crate。

依赖方向：

```text
ocr-core
   ↑
ocr-runtime ── ort
   ↑
ocr-ppocr ── ndarray/image/imageproc/rayon
   ↑
SnapClip adapter / other applications
```

`ocr-core` 不依赖 `ort`，这样其他运行时或未来的纯 Rust backend 可以复用公共类型。

## 7. 公共 API

### 7.1 输入

```rust
pub enum OcrInput<'a> {
    Encoded(&'a [u8]),
    Rgb {
        width: u32,
        height: u32,
        data: &'a [u8],
    },
    Rgba {
        width: u32,
        height: u32,
        data: &'a [u8],
    },
    File(&'a Path),
}
```

应用可以在进入 crate 前自行完成 PNG 规范化和像素上限检查；crate 仍必须对尺寸、通道数、整数溢出和解码错误做二次校验。

### 7.2 Engine

```rust
pub trait OcrEngine {
    fn model_id(&self) -> &str;
    fn provider(&self) -> ProviderInfo;
    fn recognize(&mut self, input: OcrInput<'_>) -> Result<OcrOutput, OcrError>;
}
```

`recognize` 使用 `&mut self`，原因是 ONNX Runtime Session 的执行接口可能需要可变访问。应用若需要并发，应创建多个 engine 实例或显式使用受控锁，而不是在 crate 内隐藏无界并发。

### 7.3 输出

```rust
pub struct OcrOutput {
    pub text: String,
    pub lines: Vec<OcrLine>,
    pub image_size: ImageSize,
    pub timings: OcrTimings,
    pub model_id: String,
    pub provider: ProviderInfo,
}

pub struct OcrLine {
    pub text: String,
    pub score: f32,
    pub polygon: Polygon,
    pub words: Option<Vec<OcrWord>>,
}

pub struct OcrWord {
    pub text: String,
    pub score: f32,
    pub polygon: Polygon,
}
```

`text` 是按阅读顺序拼接的便捷字段；结构化消费者应使用 `lines` 和 `polygon`，避免重新解析纯文本。

### 7.4 Provider

```rust
pub enum ProviderPreference {
    Cpu,
    DirectMl { device_id: usize },
    Cuda { device_id: usize },
}

pub struct ProviderInfo {
    pub requested: ProviderPreference,
    pub resolved: ResolvedProvider,
    pub fallback_to_cpu: bool,
}
```

provider 回退必须在初始化时明确记录。不能仅凭 provider 构造成功就宣称所有计算已在 GPU 执行，必须配合基准和运行日志验证。

## 8. Pipeline 设计

```text
decode input
  -> EXIF/orientation normalization
  -> pixel/side limit
  -> detector preprocess
  -> detector inference
  -> DB postprocess / polygons
  -> crop text regions
  -> optional angle classification
  -> recognizer batch preprocess
  -> recognizer inference
  -> CTC decode
  -> score filtering
  -> map polygons to original image
  -> OcrOutput
```

### 8.1 预处理约束

- 输入长边限制由配置控制，默认不超过 1920；
- 解码前检查声明的宽高和像素总数；
- 拒绝 0 尺寸、整数乘法溢出和超过上限的图片；
- 统一内部颜色顺序，建议使用 RGB 或 BGR 之一，不允许阶段间隐式切换；
- 检测阶段和识别阶段分别维护 scratch buffer，避免每行文本重复分配；
- 识别 batch 按文本框宽高比排序，减少 padding 浪费。

### 8.2 Session 生命周期

- engine 创建时加载 detector、recognizer，必要时加载 classifier；
- Session 常驻，不在每张图片上重新创建；
- 单个 engine 默认串行执行；
- 多并发由应用创建多个 engine，并显式控制内存预算；
- 启动阶段可以选择 lazy load，但首次识别延迟必须可观测。

## 9. 运行时和 feature 设计

建议 Cargo feature：

```toml
[features]
default = ["ort-runtime"]
ort-runtime = ["dep:ort"]
directml = ["ort-runtime", "ort/directml"]
cuda = ["ort-runtime", "ort/cuda"]
download-models = ["dep:reqwest", "dep:sha2"]
cli = ["dep:clap"]
```

默认构建只启用 CPU，避免所有用户都携带 GPU provider。Windows 发布包单独验证 DirectML DLL、ORT DLL 和 Tauri bundle 的复制规则。

`ort` 版本必须锁定并在 Windows CI 中构建验证。参考项目使用 `2.0.0-rc.10`，新 crate 不应无条件跟随浮动版本；升级 RC 或正式版必须重新运行模型契约测试和性能基准。

## 10. 错误和取消

```rust
pub enum OcrError {
    InvalidInput(String),
    Decode(String),
    ModelNotFound(PathBuf),
    ModelHashMismatch { expected: String, actual: String },
    ModelContract(String),
    ProviderUnavailable(String),
    Inference(String),
    Cancelled,
}
```

取消设计分两层：

1. pipeline 在阶段边界检查 cancellation token；
2. provider/runtime 在支持时调用 ORT 的取消机制。

不能承诺任意模型推理都能立即中断。应用关闭时应停止接收任务、发出取消、等待 worker，并将未完成任务标记为可重试。

## 11. SnapClip 适配方式

SnapClip 只实现适配器：

```text
SnapClip OcrEngine
  -> ocr-ppocr::PpOcrEngine
  -> OcrOutput
  -> text / layout / engine name
  -> store.update_clip_ocr
```

适配器负责：

- 将 `Arc<[u8]>` 转换为 `OcrInput::Encoded`；
- 将 `OcrOutput.text` 写入 `ocr_text`；
- 将 provider、model id 写入 `ocr_engine`；
- 将 polygon/line 结果序列化到 `ocr_layout`；
- 失败时映射为 SnapClip 自己的错误码。

通用 crate 不知道 `clip_id`、`ocr_status`、SQLite 或 Tauri event。

## 12. 测试方案

### 12.1 单元测试

- 输入通道和 stride 校验；
- PNG/JPEG 解码和 EXIF 方向；
- 超大图片在完整解码前拒绝；
- 模型 manifest 解析和 SHA-256 校验；
- detector/recognizer Session 输入输出 rank/type 契约；
- CTC 字典解码、重复字符和 blank 处理；
- polygon 坐标反变换；
- provider 不可用时 CPU fallback；
- 取消状态在每个阶段边界生效。

### 12.2 集成测试

固定一组不包含隐私内容的中英文截图：

- 中文短句；
- 英文和数字混排；
- 多行 UI 文本；
- 小字号和高 DPI 截图；
- 旋转文本（预期第一版不保证）。

验收指标：

| 指标 | 要求 |
|---|---|
| 完整 pipeline | det + rec 均能加载并输出文本 |
| warm inference | 连续运行不重新加载模型 |
| 结果稳定性 | 同图重复识别文本和 polygon 一致 |
| 错误可诊断 | 模型、字典、provider、解码错误可区分 |
| 线程安全 | 单 engine 串行无数据竞争，多实例行为明确 |
| 部署 | 离线环境可启动和识别 |

### 12.3 性能基准

必须分别记录：

- 模型加载时间；
- 首次推理耗时；
- warm P50/P95；
- detector、recognizer、后处理分段耗时；
- 常驻内存和峰值内存；
- CPU provider 与 DirectML provider 对比；
- medium、small、tiny 对比。

不在没有基准数据的情况下声称 DirectML 更快或 medium 更准确。

## 13. 许可证和供应链

需要维护 `THIRD_PARTY_NOTES.md`，至少记录：

- `ocr-core` 自身许可证；
- `ort`、ONNX Runtime、image、ndarray 等 crate 许可证；
- RapidOCR 参考代码的 Apache-2.0 声明；
- PaddleOCR 代码和模型来源；
- 每个模型文件的版本、URL、SHA-256 和许可证说明；
- Windows ORT/DirectML DLL 的分发说明。

代码许可证和模型许可证必须分开判断，不能因为 PaddleOCR/RapidOCR 代码是 Apache-2.0，就自动推断所有模型文件都无需单独记录。

## 14. 实施阶段

### P0：模型包和 Session 契约

- 固定 PP-OCRv6 medium 三件套；
- 建立 manifest 和 SHA-256 校验；
- 加载 detector/recognizer Session；
- 验证输入输出 tensor contract；
- 完成最小 fixture smoke test。

### P1：完整 pipeline

- 移植/实现 detector preprocess 和 DB postprocess；
- 实现文本区域裁剪；
- 实现 recognizer batch preprocess 和 CTC decode；
- 输出 `OcrOutput`。

### P2：性能和 provider

- 常驻 Session；
- scratch buffer 复用；
- medium/small/tiny profile；
- CPU 基准；
- DirectML 实机验证和 fallback。

### P3：应用适配

- SnapClip adapter；
- Windows OCR fallback；
- OCR layout 入库；
- 搜索和 UI 展示；
- 关闭、取消、失败重试回归。

### P4：发布和复用

- 独立 crate 文档和示例；
- Linux/Windows CI；
- 模型分发工具；
- 许可证清单；
- `ocr-cli` 示例程序。

## 15. 最终决策

通用 crate 的第一条可执行基线是：

```text
Rust pipeline
  + ort / ONNX Runtime
  + PP-OCRv6 det_medium
  + PP-OCRv6 rec_medium
  + ppocrv6_dict.txt
  + CPU provider
  + cls disabled
```

`small` 和 `tiny` 作为同构 profile 预留并进行基准，不增加第二套算法实现。模型文件、应用队列、数据库和 UI 分离，确保该 crate 可以脱离 SnapClip 被其他项目复用。

## 16. 参考资料

- <https://github.com/PaddlePaddle/PaddleOCR>
- <https://github.com/RapidAI/RapidOCR>
- <https://github.com/pykeio/ort>
- <https://github.com/microsoft/onnxruntime>
- <https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html>
- `refer/paddle-ocr-rs-main/`（本地 Rust 实现参考）
