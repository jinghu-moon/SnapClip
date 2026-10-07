# SnapClip 模块化与插件化重构设计

> 状态：审核修订版（开发期架构方案）
>
> 目标：在 Windows-only 前提下，将截图、剪贴板、GPUI 界面和可选智能能力拆成稳定边界，降低耦合、常驻资源和后续扩展成本。
>
> 执行清单见 `docs/23-snapclip-modular-refactor-tasklist.md`（逐任务的动作、可复制的验收命令、git 纪律与回退流程都在那里）。本文只谈"为什么这样分层"；两者冲突时以本文的设计意图 + 实测结果为准，并把结论写回本文。

## 1. 结论摘要

项目保留四个核心能力边界，但不把插件 host 和 GPUI 壳过早固化为五、六个“大而全” crate。推荐的目标是：

```text
稳定模型与协议
  snapclip-model

核心能力 crate
  snapclip-capture
  snapclip-history       # 剪贴板、历史、artifact
  snapclip-recognize     # OCR 等识别能力；按需启用

GPUI 应用壳
  apps/snapclip

可选的插件进程边界
  仅在出现第二种真实实现或测量证明需要隔离后增加
```

`snapclip-model`、`snapclip-capture`、`snapclip-history` 和 `snapclip-recognize` 是核心能力边界；`apps/snapclip` 是 GPUI 组合根，不是把所有界面文件再拆成全局 `views/`、`components/` 和 `adapters/`。这样可以同时满足：

- 核心功能依赖稳定、编译边界清晰；
- OCR、翻译、公式识别、表格识别等能力可选启用；
- 未启用的识别能力不加载模型、不创建 worker、不常驻进程；
- 当资源或稳定性证据成立时，识别实现可以独立进程运行；
- GPUI 只负责交互和展示，不承载识别算法。

当前不创建通用 `plugin-api`/`plugin-host` 协议层。现有 `OcrEngine`、`OcrCancel` 和 `ocr-rapid` feature 已经提供了识别能力的最小扩展形状；先补齐惰性启动、超时、取消、缓存和失败熔断。满足以下任一条件后，再引入独立 host：出现第二个资源模型明显不同的真实实现，或 profiling/崩溃数据证明必须进程隔离。

## 2. 当前结构问题

当前 `src-tauri/src` 已有 `domain/application/capture/platform` 的雏形，但仍存在以下问题：

1. `capture/application` 是 `CaptureEventSink` + `CaptureRuntime` 端口，负责让截图领域不接触 Win32；它不是冗余层，必须保留并在拆 crate 时迁移到 `snapclip-capture` 的公开服务边界。
2. `capture/platform` 才是纯转发层（约 11 行），只需删除并把 Windows-only 编译边界上移到 `snapclip-capture`。
3. `app` 同时承担组合根、Tauri 生命周期、截图启动、剪贴板启动和事件适配。
4. `commands` 的应用服务边界只守住了一半：`commands/capture.rs` **已经**通过 `CaptureRuntime`（端口）调用，**绕过的是 `commands/history.rs` 与 `commands/ocr.rs`**（直接 `State<'_, Store>`）。它的真正含义是 `ClipboardService`/`HistoryService` 目前**不存在**——那是 P2 的交付物，不是"commands 都不守规矩"。
5. `events` 的 Tauri 依赖集中在适配层；`domain`、`capture`、`application` 当前没有 `tauri::`。问题是缺少物理 crate 边界和事件出口 trait，而不是核心服务无法测试。
6. `ocr` 已有 `OcrEngine`、`OcrCancel` 和默认关闭的 `ocr-rapid` feature；真正缺口是 setup 中无条件启动 `OcrService`，以及事件出口仍绑定 `AppHandle`。
7. `infrastructure/store` 的 artifact 布局、命名、清理/LRU 所有权没有收敛；应由 history 的 `ArtifactStore` 统一拥有，capture 只交付字节和元数据。
8. 大文件拆分必须按收益排序：`overlay.rs` 是首要单点（约 4423 行、测试约 7%）；`d2d.rs` 约 3805 行、测试约 38%；`uia_provider.rs` 约 3338 行，但生产代码约 916 行、测试约 72%，优先搬运测试和隔离 COM 查询，不应与 overlay 同等优先级整体重写。

这些问题不是通过增加更多 `mod.rs` 转发可以解决的，必须以 crate 边界重新定义所有权。

## 3. 目标工作区结构

```text
SnapClip/
├── Cargo.toml                         # workspace，统一版本和 profiles
├── crates/
│   ├── snapclip-model/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── ids.rs                 # ClipId、ArtifactId、RecognitionTaskId、SessionId
│   │       ├── geometry.rs            # 与 UI/Win32 无关的坐标和值对象
│   │       ├── artifact.rs            # 磁盘 artifact 引用、媒体元数据
│   │       ├── events.rs              # 稳定的低频事件摘要
│   │       ├── error.rs
│   │       └── recognition.rs         # 能力名、任务/结果摘要
│   │
│   ├── snapclip-capture/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── service.rs             # CaptureService，对外唯一业务入口
│   │       ├── session.rs             # 截图会话和状态机
│   │       ├── annotation.rs          # 标注文档和命令
│   │       ├── geometry.rs
│   │       ├── window_detection/
│   │       │   ├── mod.rs
│   │       │   ├── model.rs
│   │       │   ├── snapshot.rs
│   │       │   ├── hit_test.rs
│   │       │   ├── gesture.rs
│   │       │   ├── top_level.rs       # 原生窗口 provider
│   │       │   ├── browser.rs         # UIA/MSAA/浏览器元素 provider
│   │       │   └── validation.rs
│   │       └── windows/
│   │           ├── mod.rs
│   │           ├── overlay/
│   │           │   ├── mod.rs
│   │           │   ├── window.rs       # HWND、消息循环、焦点、生命周期
│   │           │   ├── input.rs       # 鼠标、键盘、PointerGesture
│   │           │   ├── state.rs
│   │           │   └── damage.rs
│   │           ├── render/
│   │           │   ├── mod.rs
│   │           │   ├── d3d11.rs
│   │           │   ├── d2d.rs
│   │           │   ├── composition.rs
│   │           │   ├── overlay_pass.rs
│   │           │   └── readback.rs
│   │           ├── providers/
│   │           │   ├── mod.rs
│   │           │   ├── wgc.rs
│   │           │   ├── bitblt.rs
│   │           │   └── monitor.rs
│   │           ├── accessibility/
│   │           │   ├── mod.rs
│   │           │   ├── uia.rs
│   │           │   ├── msaa.rs
│   │           │   └── timed_call.rs
│   │           └── workers.rs          # detection/refinement/export/capture worker
│   │
│   ├── snapclip-history/               # 剪贴板 + 历史 + artifact 能力
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── service.rs             # ClipboardService、HistoryService
│   │       ├── listener.rs            # Windows clipboard listener
│   │       ├── reader.rs              # 延迟读取格式
│   │       ├── formats.rs
│   │       ├── normalize.rs
│   │       ├── source_app.rs
│   │       ├── history.rs             # 历史记录 repository
│   │       ├── artifact_store.rs      # 唯一的布局/命名/清理/LRU 所有者
│   │       └── windows.rs              # user32/clipboard API 适配
│   │
│   ├── snapclip-recognize/             # 先作为能力 crate，不是通用插件 host
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── engine.rs               # OcrEngine trait / OcrCancel
│   │       ├── manager.rs              # 引擎选择，默认 Windows OCR
│   │       ├── worker.rs               # lazy start、队列、超时、取消、熔断
│   │       ├── cache.rs                # 输入指纹和模型版本缓存
│   │       ├── win_ocr.rs
│   │       └── rapid.rs                # ocr-rapid feature，默认关闭
│
└── apps/
    └── snapclip/
        ├── Cargo.toml
        └── src/
            ├── main.rs                 # GPUI application entry
            ├── shell.rs                # 组合窗口和能力 crate
            ├── app_state.rs            # 仅壳级状态；领域状态归能力 crate
            ├── root.rs                 # 每个窗口第一层 Root
            ├── history/                 # 历史能力的 GPUI model/view/commands
            │   ├── model.rs
            │   ├── history_view.rs
            │   └── commands.rs
            ├── settings/                # 设置能力；稳定后再独立 crate
            ├── tray.rs                 # Win32 托盘，向壳发送低频事件
            └── adapters.rs             # capture/history/recognize 的窄适配
```

GPUI 代码按能力组织：历史列表的 model、commands、workflow 和 view 应保持在 history 能力附近；设置同理。不得建立全局 `views/`、`components/`、`models/`、`commands/` 分类目录。只有拥有独立状态、生命周期和稳定公开接缝的能力才升级为独立 crate；不要为每个页面或 helper 建 crate。

GPUI 壳必须只通过 `gpui-kit` 使用 GPUI 能力：创建组件视图前先调用一次 `gpui_kit::init(cx)`，每个窗口第一层使用 `Root`。截图 overlay 是 `snapclip-capture` 的原生 HWND 和消息循环线程，不能出现 `gpui_kit`、`Root` 或 GPUI 类型；其 F5 热键仍注册在 overlay 线程。

若迁移期同时保留 Tauri，对照壳必须是独立的 `apps/snapclip-tauri`，不能与 GPUI 壳共用一个会把 `gpui-kit` 拖入 Tauri 构建的 `ui` crate。两者只能共享能力 crate。

## 4. 依赖方向

```text
snapclip-model
    ↑       ↑       ↑             ↑
capture  history recognize
    ↑       ↑          ↑             ↑
    └───────┴──────────┴─────────────┘
                    apps/snapclip
```

更精确的规则如下：

| 模块 | 可以依赖 | 禁止依赖 |
|---|---|---|
| `snapclip-model` | 标准库、稳定序列化库 | Win32、D3D11、GPUI、SQLite、Tauri、具体引擎 |
| `snapclip-capture` | `model`、Windows SDK | `history`、`recognize`、GPUI、Tauri |
| `snapclip-history` | `model`、Windows SDK、SQLite/图片编码 | `capture`、GPUI、Tauri、具体识别引擎 |
| `snapclip-recognize` | `model`、引擎依赖 | GPUI、截图 HWND、Tauri、history 内部模块 |
| `apps/snapclip` | `model`、`capture`、`history`、`recognize`、`gpui-kit` | 直接调用 capture 的 Win32 内部模块 |

截图完成后，`snapclip-capture` 将图像字节和元数据交给应用组合根；`snapclip-history::ArtifactStore` 是布局、命名、原子写入、清理和 LRU 的唯一所有者，返回 `ArtifactRef` 后再创建历史记录或发布事件。capture 不写历史数据库、不写剪贴板；history 不读取 capture 的 HWND 或 GPU 资源。

三条接口决策（写死，避免 P2/P3 拆到一半卡住）：

1. **`ArtifactRef` 自带解析所需的一切**：`{ absolute_path, mime, ImageDimensions, byte_len, content_fingerprint }`。内容是**写入 artifact 时**由 `ArtifactStore` 算好的（`blake3`），所以 `snapclip-recognize` 既不依赖 history，也不需要为了缓存键把文件再读一遍。布局仍然只在 history 里决定；ref 是"已经解析好的句柄"。
2. **能力之间不互相调用**：`snapclip-history` 只发布 `ArtifactRef` 和低频事件，**由 `apps/snapclip` 的适配层**把它提交给 `snapclip-recognize`（"识别任务提交"没有第二个落点，也不需要）。这样 §4 的"history 不依赖具体识别引擎"才是一句可执行的规则，而不是愿望。
3. **值对象只有一份**：`Rect`/`Point`/`ImageDimensions` 归 `snapclip-model`；`snapclip-capture::geometry` 只放**派生布局**（`window_rect_to_local`、标签/放大镜摆放、尺寸标签定位）。两个 crate 各有一个 `geometry.rs` 名字可以保留，但类型不能各定义一份。

## 5. 识别能力与未来插件边界

### 5.1 当前能力模型

第一阶段定义四种能力：

```rust
pub enum RecognitionCapability {
    Ocr,
    Translation,
    FormulaRecognition,
    TableRecognition,
}
```

输入统一使用磁盘 artifact 引用，不在事件中传输大图片：

```rust
pub struct RecognitionInput {
    pub artifact: ArtifactRef,
    pub mime: String,
    pub dimensions: ImageDimensions,
    pub language_hint: Option<String>,
    pub selection: Option<Rect>,
}
```

输出按能力返回结构化结果：

- OCR：纯文本、行/词坐标、置信度；
- 翻译：源文本、目标语言、段落结果；
- 公式：LaTeX、纯文本、置信度；
- 表格：列/行、单元格文本、Markdown/CSV 导出。

识别引擎不得直接修改历史数据库、剪贴板或截图会话，只返回结果，由应用服务决定是否保存和展示。第一阶段使用 `OcrEngine` 这类 Rust trait，不建立跨进程协议。

### 5.2 生命周期和常驻策略

识别能力默认按需启动：

1. 用户未启用：不加载模型、不创建识别 worker 或进程。
2. 用户首次调用：创建任务，检查磁盘缓存，再启动对应 worker。
3. 连续任务：复用已经启动的 worker，避免反复加载模型。
4. 空闲超时：默认 60 秒释放模型和 worker 资源；超时应可配置。
5. 关闭应用：先取消任务，再停止 worker，最后释放进程句柄。

常驻进程建议：

```text
GPUI 主进程             必须常驻
截图 overlay/捕获线程   截图功能启用时常驻；不单独拆进程
剪贴板 listener          用户启用剪贴板历史时常驻
OCR/翻译/公式/表格       默认不常驻，按需启动
```

截图和剪贴板是产品主功能，不能把每次截图都变成进程启动；插件是可选能力，才适合按需进程化。

### 5.3 何时升级为独立插件 host

满足以下任一条件，才增加 `snapclip-plugin-api`/`snapclip-plugin-host`：

1. 出现第二个真实实现，且其资源模型与现有 OCR 不同（例如需要常驻数百 MB 的 ONNX/Paddle 进程）；
2. profiling、崩溃率或第三方代码风险证明必须进行进程隔离。

届时 host 才负责 registry、调度、named pipe、协议版本、崩溃重启和权限边界。第一阶段不设计通用 DLL 加载器，也不为尚未存在的第三方插件预埋 manifest 协议。

### 5.4 缓存和资源预算

当前识别缓存键至少包含：

```text
engine_id + engine_version + input_blake3 + options + model_version
```

缓存值写入磁盘，元数据进入数据库；UI 只保留缩略图和结果摘要。识别 worker 负责：

- 单插件最大并发数，默认 1；
- 全局任务队列上限；
- 单任务超时和取消；
- 模型内存预算和空闲回收；
- 结果大小上限；
- 失败熔断，避免识别异常导致主界面重试风暴。

## 6. 核心能力职责

### `snapclip-model`

只放跨 crate 的稳定值对象、artifact 引用、低频事件摘要和识别结果摘要。不得把现有 `Store`、`AppHandle`、Win32 handle、GPU 资源或 GPUI `Entity` 搬进来。它必须有清晰的领域名字，不能成为新的基础设施垃圾桶。

### `snapclip-capture`

拥有截图领域的完整生命周期：WGC/BitBlt、D3D11/D2D、overlay、选区状态机、窗口和浏览器元素吸附、标注模型、滚动截图。Windows-only 代码直接放在 crate 内的 `windows/`：**删除 `capture/platform` 这个 11 行转发层，并把 `platform/windows/capture/*` 上移合并进 `snapclip-capture/src/windows/`**（它是真正的实现，不是重复层——不要把两件事写成一句）。

### `snapclip-history`

拥有剪贴板监听、格式读取、规范化、来源程序、历史记录、artifact 存储和剪贴板写回。它不拥有 OCR 引擎；识别任务通过窄服务接口提交。

### `snapclip-recognize`

拥有 OCR/未来识别能力的 trait、worker、取消、超时、缓存和资源回收。默认 Windows OCR 可按需启动，`ocr-rapid` feature 默认关闭。它只接受 `ArtifactRef`，不读取截图 HWND 或剪贴板内部状态。

### `apps/snapclip`

GPUI 应用壳负责窗口组合、用户操作、设置和 Win32 托盘；能力 crate 自己拥有其 model、commands、workflow 和 view。壳只调用公开 service API，不能读取 `overlay.rs` 或 D3D11 资源。截图 overlay 仍是 `snapclip-capture` 的原生 HWND。

## 7. 当前文件迁移映射

### 7.1 直接迁移

| 当前路径 | 新位置 |
|---|---|
| `domain/*` | `snapclip-model/src/`，按 ids/artifact/events/error/recognition 拆分 |
| `capture/annotation.rs` | `snapclip-capture/src/annotation.rs` |
| `capture/session.rs` | `snapclip-capture/src/session.rs` |
| `capture/geometry.rs` | `snapclip-capture/src/geometry.rs`，纯值对象部分；平台转换留 Windows |
| `capture/window_detection/*` | `snapclip-capture/src/window_detection/*` |
| `capture/application/runtime.rs` | `snapclip-capture/src/service.rs` 和 `windows/workers.rs` |
| `capture/application/mod.rs` | 保留其端口语义，迁移为 `snapclip-capture/src/ports.rs`；不得删除抽象 |
| `capture/platform/*` | 删除纯转发层；Windows-only 边界上移到 `snapclip-capture` |
| `platform/windows/capture/*` | 合并到 `snapclip-capture/src/windows/` |
| `platform/windows/clipboard/*` | `snapclip-history/src/` |
| `application/clipboard_ingest.rs` | `snapclip-history/src/service.rs`、`reader.rs`、`history.rs` |
| `infrastructure/store/*` | clip/history/artifact repository 迁移到 `snapclip-history`；连接和 migration 按职责拆分 |
| `ocr/*` | `snapclip-recognize/src/`；保留 `OcrEngine`/`OcrCancel`，补齐惰性生命周期 |
| `app/capture.rs` | UI adapter 或组合根，不再属于核心截图 crate |
| `app/clipboard.rs` | UI adapter 或组合根，不再拥有剪贴板业务逻辑 |
| `commands/*` | 删除 Tauri command；改为 UI action 调用 service API |
| `events/*` | 低频事件摘要进 `snapclip-model`；Tauri/GPUI channel 适配留各自壳 |
| `icon.rs` | **注意：它是"按 exe 路径提取来源程序图标"的缓存（供历史行显示），不是托盘**。图标能力随 history 行迁移到 `apps/snapclip/src/history/`；托盘在 `apps/snapclip/src/tray.rs`，是**新建**能力（当前仓库没有任何托盘实现，`Cargo.toml` 只有 `tauri-plugin-opener`） |

### 7.2 必须拆分的大文件

- `overlay.rs`：第一优先级，拆为 HWND/消息循环、输入状态、会话绑定、渲染提交、窗口恢复；先搬生产职责，再搬测试。
- `d2d.rs`：第二优先级，拆为设备资源、底图 pass、遮罩/选框 pass、放大镜 pass、文本布局。
- `uia_provider.rs`：第三优先级，生产代码约 916 行、测试约 72%；优先把测试移到 provider/fixture，再按 COM 初始化、UIA 查询、缓存和身份校验拆分。
- `store/mod.rs`：拆为连接生命周期、clip repository、artifact repository、recognition result repository、迁移；artifact repository 归 history 所有。
- `application/clipboard_ingest.rs`：拆为事件接收、去重窗口、格式读取、publication、recognize enqueue。

拆分时允许破坏旧内部 API，但每阶段必须有编译和回归门禁；`capture/application` 的端口不能因“路径变短”而删除。纯 `capture/platform` 转发层可在 P0.5 直接删除，不保留长期兼容层。

## 8. 事件和调用模型

核心服务使用强类型、低频事件，不使用 Tauri event name 字符串作为领域协议：

```rust
pub enum AppEvent {
    Capture(CaptureEvent),
    Clipboard(ClipboardEvent),
    Recognition(RecognitionEvent),
}
```

`AppEvent` 只包含 id、状态、尺寸、`ArtifactRef` 和错误码，不包含鼠标移动、每帧画面、放大镜像素或大块 PNG。capture/recognize 的细节事件留在各自 crate；只有需要跨能力观察的摘要才进入 `snapclip-model`，避免每次 capture 演进都修改共享 crate。

Tauri 适配器实现 `AppEventSink` 并调用 `AppHandle`；GPUI 适配器使用 channel。它们是壳的适配代码，不能反向污染 `capture`、`history` 或 `recognize`。

GPUI 异步规则：I/O、数据库、识别和解码使用 `cx.spawn`/`background_spawn`；完成回调只能通过 `Entity::update` 修改实体。每个异步结果必须携带 revision/epoch/request id，过期结果直接丢弃。复用现有 `SnapshotEpoch`、`RequestId`、`RequestGate` 和 `refinement.retire()` 的原则，不在 GPUI 适配层重新发明竞态控制。

截图高频路径必须保持：

```text
F5 -> Win32 hotkey -> capture worker -> native overlay -> GPU export
```

不能改成：

```text
F5 -> GPUI event -> JSON/IPC -> overlay -> JSON/IPC -> GPUI
```

## 9. 渐进式迁移步骤

每个阶段结束都必须提交并推送；阶段之间允许并行，但合并前必须通过工作区全量构建和回归测试。

### P0：补齐资源基线和 workspace 骨架

- 使用已有基线作为正确性起点：tag `smart-snapping-v1-2026-10-07`、`cargo test --lib` 403、`cargo check --all-targets` 0 warning、浏览器探针 41/41、Explorer 探针 12/25 且 available 25/25、真机会话 `over16ms=0`、字体子集门禁通过。
- 只补缺失的启动资源数据：进程数、空闲 CPU、常驻内存/GPU 内存、冷启动、包体积；另测“修改 model crate 一行”的增量编译时间。
- **这些资源数字必须在 P0.5 之前测**，并且 before/after 两个数都要留档：OCR 惰性化（P0.5）唯一能拿出来的收益就是这两个数之间的差。
- 建立 Cargo workspace，不改变运行时行为。
- 添加 `snapclip-model` 空实现和类型测试；不添加尚无真实 owner 的 plugin host。

验收：现有 Tauri 构建、Rust 测试、自动夹具探针和截图人工链路不变；记录基线原始输出。

### P0.5：低风险边界收敛

- 删除 `capture/platform` 11 行纯转发层，保留 `capture/application` 的端口语义。
- 仿照 `CaptureEventSink`，把 OCR 事件出口从 `AppHandle` 换成框架无关 trait。
- 将 `OcrService::start` 改为首次任务时惰性启动；未启用 OCR 不创建 worker、不加载模型。

验收：截图 403/当前全量测试、OCR 重试/取消测试、启动资源对比；确认领域 crate 中 `tauri::` 仍为 0。

### P1：抽离截图 crate，先拆 overlay

- 先迁移 `capture/application` 端口和 `overlay.rs` 的生产职责，再迁移其余 Windows provider。
- 将 `capture` 和 Windows 捕获实现迁入 `snapclip-capture`，保持原生 overlay 独立线程。
- 保持 F5、Esc、Enter、窗口吸附、DPI、多显示器和导出测试。

验收：`cargo test --lib`、`cargo check --all-targets`、浏览器/Explorer 自动探针、截图回归和 GPU 像素测试通过；任何探针退化不得提交。

### P2：抽离 history，先收敛 artifact 所有权

- 迁移 listener、reader、formats、normalize、source app、Store。
- 先把布局、命名、原子写入、清理和 LRU 收敛到 `snapclip-history::ArtifactStore`。
- 定义 `ClipboardService`、`HistoryService` 和 artifact API；capture 只交付字节和元数据。

验收：文本、HTML、图片、去重、历史分页、磁盘缓存和复制回写均通过。

### P3：recognize 能力的惰性化

- 将现有 OCR worker 迁入 `snapclip-recognize`，保留 `OcrEngine`/`OcrCancel`。
- 增加按需启动、取消、超时、磁盘缓存和失败状态。
- 用启动资源和任务延迟证明收益；不要先实现 named pipe/manifest/通用 host。

验收：OCR 结果、重试、历史关联和启动资源基线通过。

### P4：GPUI 壳（独立立项）

- 新建独立 `apps/snapclip`，先实现主窗口、历史能力、搜索和复制。
- 启动时先调用 `gpui_kit::init(cx)`，每个窗口第一层使用 `Root`。
- 通过 channel 接收 capture/history/recognize 低频事件；使用 `cx.spawn`/`background_spawn` 和带 revision 的 `Entity::update`。
- 截图 overlay 不使用 GPUI；Tauri 对照壳若保留，必须是独立 app。

验收：GPUI 主窗口可用，历史列表/设置交互测试通过，截图 overlay 仍保持独立原生窗口，关键性能指标不退化。

### P5：按证据增加进程插件边界

- 只有满足第二种真实实现或隔离证据后，才增加 `plugin-api`/`plugin-host`。
- 轻量实现继续使用进程内 worker，重型模型使用 named pipe 外部 worker。
- 增加缓存命中、空闲回收、崩溃重启和权限测试。

验收：未启用识别能力零额外常驻 worker/模型；启用后任务可取消、可重试、结果可持久化。

### P6：删除 Tauri 和旧目录

- GPUI 完成托盘、隐藏/显示、焦点、退出、DPI、多显示器回归。
- 删除 `commands`、Tauri `events`、Vue adapter 和旧 `src-tauri` 组合根。
- 执行全量依赖检查，确认不存在 Tauri/WebView 残留。

## 10. 必须建立的测试与门禁

### 单元测试

- model ID、artifact、事件和识别结果序列化；
- recognize worker 的排队、取消、超时、并发上限；
- cache key 稳定性和版本失效；
- capture 状态机、窗口吸附和坐标转换；
- clipboard 去重、格式规范化和数据库 repository。

### 集成测试

- F5 到 overlay 的原生链路；
- 截图完成后 artifact 发布和历史入库；
- 识别能力未启用时进程/线程/模型不启动；
- OCR/未来识别 worker 的崩溃、超时和取消；
- GPUI 事件更新不阻塞截图线程。

### 性能门禁

- 不允许 UI/插件事件进入鼠标移动和 GPU 绘制路径；
- 不允许通过事件传递大图片；
- 截图 overlay 出现延迟、输入延迟和 GPU readback 指标与 P0 基线对比；
- 分别记录 GPUI 主进程、截图线程、剪贴板 listener、识别 worker 的 CPU/内存；
- 识别能力未启用时额外常驻 worker/模型目标为零，不得加载模型。

### 10.1 依赖方向与接缝门禁

- 使用 `cargo tree`/脚本断言 `snapclip-capture`、`snapclip-history`、`snapclip-model` 不依赖 `tauri`、`wry` 或 `gpui-kit`。
- 断言 `snapclip-capture` 不依赖 `snapclip-history`，高频截图路径不依赖识别能力。
- 断言 overlay 线程和 capture crate 的依赖图中不存在 `gpui-kit`。
- 跨 crate 坐标契约测试固定“显示器本地物理像素 ↔ 虚拟桌面物理像素”，覆盖负坐标和混合 DPI。
- **公共接缝不得暴露 pub 字段**：这些 crate 拆出来之后，`CaptureService`/`HistoryService`/`OcrEngine` 就是公共 API。画笔层与状态机的内部类型（`RenderView`、`OverlayFrameState`、`RenderMetrics`、`RingOptions`、`ChainRingView`…）**留在 crate 内、不 re-export**；确实要跨接缝的数据类型用 builder + reader 方法（GPUI Kit 规范 "No `pub` fields across the seam"）。拆 crate 之前先决定每个类型是内部还是接缝，别让内部结构变成合同。

### 10.2 GPUI 壳的具体契约

- 启动：建任何组件视图前调用一次 `gpui_kit::init(cx)`；每个窗口第一层是 `Root`；应用只依赖 `gpui-kit`（GPUI 通过 `use gpui_kit::*`）。
- 状态归属按能力：`Entity<HistoryState>`、`Entity<SettingsState>` 各自拥有其 model/commands/view；**禁止一个 entity 装全部应用状态**，也禁止把领域状态放进壳。
- 历史列表：长列表必须虚拟化（`List`/`VirtualList`）；**`ElementId` 用 clip id，不能用索引**（可重排内容用 index 是规范点名的失败模式）。
- 异步：`cx.spawn`/`background_spawn` 只做 I/O 与计算，`Entity::update` 才改状态；每个结果带 revision/epoch/request id，过期即丢弃。
- 托盘：`gpui-kit` 没有托盘组件，托盘继续是 Win32 实现（`apps/snapclip/src/tray.rs`），只向壳发送低频事件。
- 测试分层：纯函数 → `#[gpui_kit::test]`（entity/事件/订阅）→ `VisualTestContext`（焦点/键盘/指针/布局）→ 真实窗口按**无障碍树**断言（role/label/value/enabled/focus）。最后一层就是现有的截图探针模式（UIA/MSAA），历史页与设置页照此补。

### 10.3 现有基线与新增基线

正确性基线直接复用 `smart-snapping-v1-2026-10-07`：`cargo test --lib` 403、`cargo check --all-targets` 0 warning、浏览器夹具 41/41、Explorer 12/25 且 available 25/25、真机会话 `over16ms=0`、字体子集门禁通过。重构阶段每个阶段都必须复跑这些门禁。

P0 只新增启动资源测量：进程数、空闲 CPU、常驻内存/GPU 内存、冷启动、包体积，以及修改 `snapclip-model` 一行的增量编译时间。探针不能因为跨 crate 失效；依赖 Tauri 才能运行的探针视为拆分未完成。

**探针是环境相关的，不是 CI 里的无声门禁**：它们需要真实的浏览器/资源管理器窗口存在且可见。红了要先排除环境（窗口不存在、被别的窗口遮挡、机器高负载），复跑确认后才当回归；反过来，也不能把复跑后仍然稳定的红当成噪声放过去。上一轮实测过一次假红（Explorer `available=10/25`、几何逐位不变，复跑回到 25/25）。

## 11. 最终建议

“四个核心能力边界 + 可证据触发的插件进程层”是当前最合适的方向：

```text
model        只放稳定合同和小型值对象
capture      截图及其 Windows GPU/overlay 实现
history      剪贴板、历史和 artifact 存储
recognize    OCR 等识别能力，先按需进程内运行
apps/snapclip GPUI 主界面和组合根
future host  只有第二个真实实现或隔离证据成立后再增加
```

GPUI 迁移只替换 UI 壳；截图和剪贴板核心仍是 Rust crate。识别能力先通过已有 trait 按需运行，重型能力只有在资源或稳定性证据成立后才独立进程；所有大数据以磁盘 artifact 传递。这样能降低常驻 CPU/内存，又不会为了臆测的插件需求把高频截图路径引入额外 IPC 和序列化延迟。
