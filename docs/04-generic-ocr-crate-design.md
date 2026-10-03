# 通用 Rust OCR crate 设计方案

## 1. 文档信息

| 项目 | 内容 |
|---|---|
| 文档状态 | rapid-ocr-rs 核心代码已实现；正式发布前验证与发布工程待执行 |
| 目标 | 建设可被多个 Rust 项目复用的本地 OCR crate；SnapClip 适配后置 |
| 首发模型 | PP-OCRv6 multi medium |
| 首发运行时 | ONNX Runtime，通过 `ort` crate 接入 |
| 首发平台 | 核心 API 跨平台；Windows 优先验证 CPU/DirectML；Linux 首发只承诺纯 CPU CI |
| 首发输入 | 编码图片、带 stride 的 RGB/RGBA/BGRA/Gray 像素视图、文件路径、可选 ROI |
| 首发输出 | 文本、置信度、行框、检测框、阶段耗时 |

本方案把 OCR 算法、模型加载和推理运行时从 SnapClip 中抽离。SnapClip 只负责剪贴板来源、任务队列、数据库和 UI；通用 crate 不依赖 Tauri、SQLite、剪贴板或任何具体应用。

### 1.1 二次评审修订

两份审核报告和官方资料交叉核对后，本版作以下调整：

1. 保留“通用 PP-OCR pipeline”定位，但把 Windows 屏幕输入作为一等适配场景；核心 crate 仍不依赖 Windows API。
2. 公共输入增加 `PixelView`、stride、通道顺序、bottom-up、DPI hint 和 ROI，避免截图必须先编码 PNG 再解码。
3. `medium` 仍是端到端准确率/契约基线；`small` 是桌面默认候选，`tiny` 是吞吐候选，最终档位由同一评测集和硬件基准决定。
4. 长边 1920 不再作为不可变算法规则。缩放、放大、分块和 ROI 策略由 `PreprocessPolicy` 控制，并以屏幕黄金集验证。
5. DirectML 从“性能路线”改为“可选实验路线”。CPU 是首发基线；只有在固定硬件、固定模型和固定数据集上优于 CPU 才允许应用默认启用。
6. 服务线程、latest-wins、队列和 Windows OCR fallback 属于应用/平台层，不塞入通用核心 crate。

报告中的推断性性能数字、第三方单机反馈和未核实的 Windows OCR 产品结论不作为本设计的事实依据；它们只能形成待验证实验项。

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

本地参考项目的 PP-OCRv6 registry 提供检测和识别模型，未提供可直接配套的 v6 分类 artifact。官方高层 API 是否在某些发行版中使用文本行方向模型，需要按具体模型包核验；因此第一版不混入 PP-OCRv4 分类模型，避免跨版本组合和额外推理开销。方向分类接口保留，但只有 manifest 明确提供兼容 classifier 时才启用。

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

官方公开指标是通用场景/模型基准，不是 Windows UI、ClearType、高 DPI 截图的保证；本项目必须自行建立屏幕黄金集。

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

官方资料只能证明 ONNX Runtime/`ort` 提供 execution provider 和硬件加速接口，不能证明某个 OCR 模型在某个桌面 GPU 上一定更快。DirectML、CUDA、OpenVINO 等 provider 的启用必须与模型 shape、驱动、DLL 和硬件一起验证。

本地参考项目 `refer/paddle-ocr-rs-main` 已验证以下组合：`ort` Session、DB 检测、CTC 识别、纯 Rust 图像预处理、CPU provider 和 DirectML provider 配置。新 crate 可以复用其算法思路，但不直接暴露其 CLI、在线下载和应用级配置。

### 3.4 对审核报告的取舍

| 建议 | 决策 | 原因 |
|---|---|---|
| 增加 BGRA/stride/DPI/ROI | 采纳 | 这是输入契约缺口，不依赖性能猜测即可验证 |
| medium 直接作为桌面默认 | 不采纳 | 官方模型规模信息不等于屏幕延迟；先做 medium 基线、small 候选 |
| 1920 长边固定限制 | 不采纳 | 高 DPI 小字可能被过度缩小；改为可配置策略、ROI 和分块 |
| DirectML 默认加速 | 不采纳 | provider 可用不等于端到端更快；固定 CPU 基线并实测 |
| 把服务线程/latest-wins 放进核心 | 不采纳 | 属于应用调度，不应污染可复用同步 API |
| 加 `ocr-screen` Windows 适配层 | 采纳 | 集中处理 BGRA/WIC/DPI/unsafe，保持核心跨平台 |
| P0 承诺字符级 words | 不采纳 | 当前 pipeline 没有经过验证的 CTC 字符坐标算法 |
| Windows OCR 提前作为产品基线 | 部分采纳 | 在 SnapClip P1 做对照基准，但不成为通用 crate 依赖 |

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
| `ppocrv6-multi-medium` | `PP-OCRv6_det_medium` | `PP-OCRv6_rec_medium` + 该 profile 声明的字典 | 准确率/契约基线 |
| `ppocrv6-multi-small` | `PP-OCRv6_det_small` | `PP-OCRv6_rec_small` + 该 profile 声明的字典 | 日常桌面默认候选 |
| `ppocrv6-multi-tiny` | `PP-OCRv6_det_tiny` | `PP-OCRv6_rec_tiny` + 该 profile 声明的字典 | 低配置和高吞吐候选 |

这三个 profile 共享完全相同的 pipeline 和输出类型。模型差异只通过 manifest 和模型文件表达；不能假设 tiny、small、medium 永远共用同一个字典。

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

运行时加载器应同时支持 `Path` 和已校验的 `Bytes` 来源；`Bytes` 适合安装包资源、内嵌测试 fixture 或应用自有缓存，但不改变 manifest 的哈希校验要求。

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
  ocr-screen/     # 可选 Windows BGRA/WIC/DPI 适配，不进入核心 pipeline
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

`ocr-screen` 只接收/产生安全的 `PixelView`，集中处理 WIC、DIB、BGRA、预乘 alpha、DPI 元数据和 Windows `unsafe`。DXGI/WGC/BitBlt 的捕获生命周期仍属于应用或平台层；核心 crate 不持有 COM、纹理或映射指针。

核心 API 保持同步；工作线程、取消、超时、latest-wins、队列背压和引擎 fallback 由 SnapClip 或其他应用的 service layer 实现。需要阶段性 UI 反馈的应用可以在 service layer 中调用检测/识别阶段 API，但不为此把应用调度器塞进 `ocr-core`。

## 7. 公共 API

### 7.1 输入

```rust
pub enum PixelFormat {
    Bgra8,
    Rgba8,
    Rgb8,
    Gray8,
}

pub struct PixelView<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,
    pub bottom_up: bool,
    pub data: &'a [u8],
}
```

`PixelView` 构造或进入 pipeline 时必须验证：每行字节数不小于 `width * bytes_per_pixel`、`stride * height` 不溢出且不超过 `data.len()`，格式与 alpha 语义明确。`bottom_up` 只描述内存行顺序，不改变输出坐标系。

```rust
pub enum OcrInput<'a> {
    Encoded(&'a [u8]),
    Pixels(PixelView<'a>),
    File(&'a Path),
}
```

请求级元数据和 ROI 单独表达，避免把 Windows/DPI 语义硬编码到像素格式：

```rust
pub struct OcrRequest<'a> {
    pub input: OcrInput<'a>,
    pub roi: Option<RectU32>,
    pub scale_hint: Option<f32>,
    pub preprocess: PreprocessPolicy,
}
```

ROI 坐标以输入图像像素为单位；输出 polygon 也以输入图像像素为单位，并由 pipeline 负责加回 ROI 原点。`scale_hint` 仅是 DPI/显示缩放提示，不是未经测量的强制放大倍数。

应用可以在进入 crate 前完成 PNG 规范化，但不应为了适配 crate 强制走 PNG 往返；crate 仍必须对尺寸、通道数、stride、整数溢出和解码错误做二次校验。

```rust
pub struct PreprocessPolicy {
    pub max_decode_pixels: u64,
    pub max_side: Option<u32>,
    pub min_text_scale: Option<f32>,
    pub tile: Option<TilePolicy>,
    pub enhance: EnhancementPolicy,
}
```

默认策略只提供安全上限和保守缩放，不预先启用反色、对比度拉伸或锐化。深色主题、低对比度、ClearType、小字号等处理必须先经过黄金集 CER/召回评测，再作为可选策略启用。

### 7.2 Engine

```rust
pub trait OcrEngine {
    fn model_id(&self) -> &str;
    fn provider(&self) -> ProviderInfo;
    fn recognize(&mut self, request: OcrRequest<'_>) -> Result<OcrOutput, OcrError>;
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
    pub words: Option<Vec<OcrWord>>, // 仅在模型/配置支持字符级对齐时提供
}

pub struct OcrWord {
    pub text: String,
    pub score: f32,
    pub polygon: Polygon,
}
```

`text` 是按阅读顺序拼接的便捷字段；结构化消费者应使用 `lines` 和 `polygon`，避免重新解析纯文本。P0 不承诺字符级 `words`：CTC 输出本身不等于字符几何框，只有实现并验证时间步对齐后才填充该字段。

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

### 7.5 阶段 API

整体 `recognize` 是首发稳定 API。实现可额外暴露以下阶段接口，供标注工具或渐进式 UI 使用：

```rust
fn detect(&mut self, request: DetectionRequest<'_>) -> Result<DetectionOutput, OcrError>;
fn recognize_lines(
    &mut self,
    request: LineRecognitionRequest<'_>,
) -> Result<Vec<OcrLine>, OcrError>;
```

阶段 API 必须复用同一模型、预处理和坐标契约，不能形成第二套算法路径；若阶段结果没有明确消费者，则不实现。

## 8. Pipeline 设计

```text
decode input
  -> EXIF/orientation normalization
  -> pixel/side limit and optional ROI/tile plan
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

- 输入长边和像素上限由 `PreprocessPolicy` 控制，不把 1920 固化为所有屏幕的规则；
- 解码前检查声明的宽高和像素总数；
- 拒绝 0 尺寸、整数乘法溢出和超过上限的图片；
- 统一内部颜色顺序，建议使用 RGB 或 BGR 之一，不允许阶段间隐式切换；
- 检测阶段和识别阶段分别维护 scratch buffer，避免每行文本重复分配；
- 对 BGRA/RGBA 输入，评估将通道重排、缩放、归一化和 CHW 写入融合到一次遍历；只有基准证明预处理占主要耗时后才引入 SIMD 或平台特化路径；
- 识别 batch 按宽高比分桶，而不是只排序，减少 padding 和动态 shape 碎片；
- 大图可按 tile 规划检测，tile 必须有重叠区并在原图坐标中做 polygon 去重；小 ROI 可走放大/快速路径；具体阈值由评测集扫描确定。

屏幕输入的推荐策略不是无条件缩小：4K 全屏可能需要分块，小选区可能需要适度放大，DPI hint 只参与策略选择。HDR 色调映射、受保护窗口黑屏判断和截图捕获 API 属于 `ocr-screen`/应用层，不放入 PP-OCR 核心。

### 8.2 Session 生命周期

- engine 创建时加载 detector、recognizer，必要时加载 classifier；
- Session 常驻，不在每张图片上重新创建；
- 单个 engine 默认串行执行；
- 多并发由应用创建多个 engine，并显式控制内存预算；
- 启动阶段可以选择 lazy load，但首次识别延迟必须可观测。
- 可选空闲卸载必须由应用或服务层控制，并以实测内存收益和重载代价决定；核心 engine 不自行创建后台线程。

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

默认构建只启用 CPU，避免所有用户都携带 GPU provider。CUDA 不属于 Windows 首发验收范围；DirectML 仅作为 opt-in 实验 provider。Windows 发布包单独验证 DirectML DLL、ORT DLL 和 Tauri bundle 的复制规则。

`ort` 版本必须锁定并在 Windows CI 中构建验证。参考项目使用 `2.0.0-rc.10`；官方 `ort` 文档目前展示的 API reference 已到 `2.0.0-rc.13`，但不能仅凭版本号升级，必须在目标 Windows toolchain 上重新运行模型契约、DLL 加载、准确率和性能基准。

Windows 发布自检必须确认实际加载的 ORT DLL 版本和路径，不能假设系统目录中的同名 DLL 与构建版本一致；发布包应将匹配的 DLL 放在应用可控的加载目录，并在 smoke test 中验证。

DirectML 的 provider 可用不等于 OCR 更快。检测输入尺寸和识别行宽具有动态性，provider 选择、顺序执行/内存配置、shape bucket 和会话创建成本都必须实测。首发默认 CPU；应用只有在固定硬件和屏幕数据集上的 warm P95、峰值内存与准确率均满足门槛时才启用 DirectML。

## 10. 错误和取消

```rust
pub enum OcrError {
    InvalidInput(String),
    UnsupportedInput(String),
    Decode(String),
    ModelNotFound(PathBuf),
    ModelHashMismatch { expected: String, actual: String },
    ModelContract(String),
    ProviderUnavailable(String),
    Inference(String),
    Timeout,
    Cancelled,
}
```

取消设计分两层：

1. pipeline 在阶段边界检查 cancellation token；
2. provider/runtime 在支持时调用 ORT 的取消机制。

不能承诺任意模型推理都能立即中断。应用关闭时应停止接收任务、发出取消、等待 worker，并将未完成任务标记为可重试。

受保护窗口黑屏、HDR 色调映射失败和捕获句柄错误属于 `ocr-screen`/捕获层错误，不伪装成“无文本”；核心 crate 只报告它实际观察到的输入为空、无文本或解码失败。

## 11. SnapClip 适配方式（发布后）

rapid-ocr-rs 正式发布后，SnapClip 只实现适配器；本阶段不实施以下调用链：

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
- `PixelView` 的 BGRA/RGBA/RGB/Gray、bottom-up、padding stride 和 ROI 坐标映射；
- PNG/JPEG 解码和 EXIF 方向；
- 超大图片在完整解码前拒绝；
- 模型 manifest 解析和 SHA-256 校验；
- detector/recognizer Session 输入输出 rank/type 契约；
- CTC 字典解码、重复字符和 blank 处理；
- polygon 坐标反变换；
- provider 不可用时 CPU fallback；
- 取消状态在每个阶段边界生效。
- 属性测试覆盖 stride、尺寸乘法、ROI 往返映射；纯 Rust 像素核可增加 fuzz 测试。

### 12.2 集成测试

固定一组不包含隐私内容的屏幕黄金集和通用图片：

- 中文短句；
- 英文和数字混排；
- 多行 UI 文本；
- 小字号和高 DPI 截图；
- 深色/浅色主题、低对比度、ClearType；
- IDE/终端/网页/聊天/表格等真实场景；
- 旋转文本（第一版不保证，仅记录能力边界）。

验收指标：

| 指标 | 要求 |
|---|---|
| 完整 pipeline | det + rec 均能加载并输出文本 |
| warm inference | 连续运行不重新加载模型 |
| 结果稳定性 | 同图重复识别文本和 polygon 一致 |
| 错误可诊断 | 模型、字典、provider、解码错误可区分 |
| 线程安全 | 单 engine 串行无数据竞争，多实例行为明确 |
| 部署 | 离线环境可启动和识别 |

精度指标至少包含：字符错误率（CER）、行检测召回/精度、polygon IoU，以及数字、URL、路径等关键字段的整串准确率。屏幕黄金集建议由 DirectWrite 合成样本加少量人工脱敏截图组成；样本和标注不进入运行时 crate。

### 12.3 性能基准

必须分别记录：

- 模型加载时间；
- 首次推理耗时；
- warm P50/P95；
- detector、recognizer、后处理分段耗时；
- 常驻内存和峰值内存；
- CPU provider 与 DirectML provider 对比；
- medium、small、tiny 对比。
- 端到端 warm P50/P95、ROI 尺寸分档、峰值内存和空闲内存；
- CPU 与 DirectML 的同机对比，检测和识别阶段分别记录。

P0 通过门槛必须包含屏幕黄金集 CER/检测召回和 warm P95；CI 性能只做相对回归，发布基准在固定硬件上执行。不在没有基准数据的情况下声称 DirectML 更快或 medium 更准确。

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

### P0：输入、模型包和 CPU Session 契约

- 固定 PP-OCRv6 medium 三件套；
- 实现 `PixelView`、stride/ROI 校验和 `OcrRequest`；
- 建立 manifest 和 SHA-256 校验；
- 加载 detector/recognizer Session；
- 验证输入输出 tensor contract；
- 完成 tiny/small fixture smoke test 和 medium 契约测试；
- 建立屏幕黄金集、CER/检测召回计算和 CPU warm P50/P95 harness；
- 以 medium 建立准确率基线，以 small 建立桌面默认候选。

### P1：完整 pipeline

- 移植/实现 detector preprocess 和 DB postprocess；
- 实现文本区域裁剪；
- 实现 recognizer batch preprocess 和 CTC decode；
- 输出 `OcrOutput`。
- 实现融合预处理评估、识别宽度分桶和 ROI 快速路径；
- `ocr-screen` 完成 Windows BGRA/WIC/DPI 适配，但不把捕获 API 放入核心。
- SnapClip 同期保留/测量现有 Windows OCR 作为零模型体积基线；它属于 `ocr-win`/应用层，不作为本 crate 的依赖。

### P2：性能和 provider 实验

- 常驻 Session；
- scratch buffer 复用；
- medium/small/tiny profile；
- CPU 基准；
- DirectML 实机验证和 fallback；
- 仅在同机数据证明收益时启用 shape bucket、provider 特化或 GPU 预处理；
- 空闲卸载/预热由应用 service layer 评估。

### P3：应用适配（暂缓）

- 本阶段不进入 `rapid-ocr-rs` 首次发布范围；
- rapid-ocr-rs 正式发布后，再由各应用实现独立适配器；
- SnapClip 的数据库、队列、Windows OCR fallback、搜索和 UI 不属于本轮验收；
- 适配前必须基于已发布 crate API 增加应用级前后回归测试。

### P4：发布和复用

- 独立 crate 文档和示例；
- Linux/Windows CI；
- 模型分发工具；
- 许可证清单；
- `ocr-cli` 示例程序。

## 15. 最终决策

### 15.1 当前实现状态（2026-09-30）

- `crates/rapid-ocr-rs` 已从参考项目独立为内部可复用 crate，统一到 `ort 2.0.0-rc.13` 实际 API 和 `ndarray 0.17`；默认 CPU，DirectML 仅 Windows 可选，CUDA/CANN 为显式 feature。
- `PixelView::to_bgr` 已覆盖 BGRA/RGBA/RGB/Gray、stride、bottom-up、ROI；编码图片/文件在读取尺寸后执行像素上限检查。
- `RapidOcrEngine` 已实现通用 `OcrEngine` trait，输出文本、行 polygon、阶段耗时和 provider 回退信息；本轮只验证 crate 公共 API，不把任何应用适配视为发布条件。
- SnapClip 适配、数据库字段、应用队列、Windows OCR fallback、搜索和 UI 均不属于本轮 rapid-ocr-rs 发布验收；现有应用改动保持独立，待 crate 正式发布后重新评估。
- 当前没有随仓库提交模型权重。模型 manifest、权重校验和离线 smoke test 由 crate 使用方或发布包单独管理。
- 尚未声称 PP-OCRv6 的真实准确率、DirectML 性能或语言包覆盖；屏幕黄金集、真机基准和离线打包验收仍是发布前任务。

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

这只是准确率和模型契约基线，不是桌面默认档。`small` 是首选默认候选，`tiny` 是吞吐候选；必须经过同一屏幕黄金集和固定硬件基准后才能拍板。模型文件、应用队列、数据库和 UI 分离，确保该 crate 可以脱离 SnapClip 被其他项目复用。只有完成发布验收并发布 crate 后，才开始 SnapClip 适配。

P0 完成的判定不是“模型能跑”，而是同时满足：

1. `Encoded` 与 `PixelView(BGRA/RGBA/RGB + stride + ROI)` 都能离线识别；
2. medium 契约和 small/tiny 冒烟测试通过；
3. 屏幕黄金集有 CER、检测召回和关键字段准确率基线；
4. CPU warm P95、峰值内存和取消/错误路径可观测；
5. DirectML 未经同机基准证明前不作为默认 provider。

## 16. 参考资料

- <https://github.com/PaddlePaddle/PaddleOCR>
- <https://github.com/RapidAI/RapidOCR>
- <https://github.com/pykeio/ort>
- <https://github.com/microsoft/onnxruntime>
- <https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html>
- `refer/paddle-ocr-rs-main/`（本地 Rust 实现参考）
