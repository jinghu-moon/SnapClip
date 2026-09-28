# SnapClip 架构文档 v2

## 1. 文档定位

| 项目 | 结论 |
|---|---|
| 产品名称 | SnapClip |
| 产品定位 | Windows 截图、贴图、OCR 与剪贴板管理二合一效率工具 |
| 目标系统 | Windows 10 1809+、Windows 11 |
| 主技术栈 | Tauri 2 + Rust 2024 + Vue 3/TypeScript + SQLite/FTS5 |
| 原生边界 | 截图、贴图、剪贴板、快捷键、窗口、存储和 IPC 由 Rust/Win32 负责 |
| UI 边界 | Vue 3 负责主界面、历史、搜索、设置、预览和业务交互 |
| 参考项目 | SnapClip-old、ClipVault，以及两者 `docs` 和 `refer` 目录 |
| 项目许可证 | GNU AGPL v3 |
| 文档状态 | v2 目标架构；实现状态以代码和验收记录为准 |

本版本以“Rust 原生核心 + Tauri/Vue UI + 可渐进拆分的后台能力”为主体。SnapClip-old 的原生截图、标注和贴图经验，以及 ClipVault 的 Vue 历史、搜索、富文本、OCR 和存储实现，作为迁移依据而不是架构约束。

## 2. 目标与边界

### 2.1 核心闭环

```text
全局快捷键
  ├─ 截图 → 选区/窗口/滚动 → 标注/OCR/取色/二维码 → 复制/保存/贴图/入库
  └─ 剪贴板呼出 → 搜索历史 → 复制/快速粘贴 → 返回当前应用
```

### 2.2 性能目标

| 指标 | 目标 |
|---|---:|
| 剪贴板后台空闲 CPU | 接近 0%，禁止默认高频轮询 |
| 剪贴板事件到任务入队 | P95 < 20 ms |
| 截图热键到覆盖层首帧 | P50 < 80 ms，P95 < 150 ms |
| 主界面热显示 | P50 < 300 ms |
| 主界面冷启动可交互 | 候选门槛，建立 Windows 基准后确定 |
| 后台代理私有内存 | 未来拆分 ClipAgent 后候选门槛 < 35 MB（基准验证后确认），不加载 WebView2 |
| 4K 普通截图帧预算 | 复用 canonical capture，避免 Save/Copy/OCR 各自复制整帧；峰值内存以基准测量 |
| 万级历史搜索 | P95 < 50 ms |
| 贴图资源 | 建立不同分辨率/并发数下的 GPU、CPU 和窗口资源基线，再确定预算 |

冷启动与热显示、后台代理与含 WebView 的 UI 进程必须分开测量。表中数字是候选验收门槛，必须通过
release 构建、固定数据集和真实 Windows 桌面测试确认，不作为未经测量的承诺。队列容量、贴图数量和 OCR
超时等配置值由压力测试建立，不在架构阶段拍定。

### 2.3 明确不做

- 不把 Electron 作为主 UI 运行时。
- 不把 WebView2 作为全屏截图像素渲染器。
- 不在 `WM_CLIPBOARDUPDATE` 回调中执行 OCR、压缩或数据库写入。
- 不使用无界队列缓存图片帧、OCR 任务或脚本输出。
- v2 不包含云同步、账号系统和远端 OCR。

## 3. 总体架构

### 3.1 进程模型

```text
┌─────────────────────────────────────────────────────┐
│ ClipAgent.exe                                       │
│ 剪贴板监听 / 全局快捷键 / 托盘 / 历史写入 / 快速粘贴 │
└──────────────┬──────────────────────┬───────────────┘
               │ 命名管道/事件         │ 命名管道/事件
┌──────────────▼──────────────┐  ┌────▼────────────────┐
│ MainUI.exe                  │  │ CaptureHost.exe    │
│ Tauri 2 + 一个长期 WebView2 │  │ Win32/D2D/WGC      │
│ Vue：历史/搜索/设置/预览等   │  │ 覆盖层/标注/贴图/OCR │
└─────────────────────────────┘  └─────────────────────┘
                                      │
                              ┌───────▼────────┐
                              │ Worker.exe      │
                              │ OCR/编码/缩略图 │
                              │ 用户脚本        │
                              └────────────────┘
```

上图是可演进的目标进程模型，不代表首期交付结构。首期为一个 Tauri/Rust 进程，图中的
`ClipAgent`、`CaptureHost` 和 `Worker` 是未来可能拆出的进程边界。

### 3.2 渐进部署策略

首期采用单 Tauri 应用进程，剪贴板、历史、截图和 UI 能力在领域模块内隔离；原生覆盖层和贴图使用独立原生窗口，但不因此单独拆进程。只有性能、稳定性或生命周期数据证明有必要时，才评估拆出 `ClipAgent`、`CaptureHost` 或 worker 进程。进程拆分不能改变领域接口，届时只替换传输层。

### 3.3 分层

```text
领域层：ClipItem、ScreenshotDocument、Shape、SearchQuery、HistoryPolicy
应用层：ClipboardService、CaptureSession、PinService、OcrJob、ScriptJob
平台层：Win32、COM/WinRT、D3D11、D2D、UI Automation、WebView2 桥接
基础设施：SQLite/FTS5、内容寻址文件仓库、日志、指标、配置迁移
界面层：Vue 3、自定义 CSS Design Tokens、Reka UI、Tabler Icons、虚拟列表、命令面板
```

领域层不得依赖 HWND、HBITMAP、D2D 对象、Tauri `AppHandle` 或 Vue 组件。

## 4. 技术选型

### 4.1 Rust 原生核心

- 使用 Rust 2024 edition；依赖兼容性通过 Cargo.lock 和 Windows CI 验证。
- 负责 Win32/WinRT、剪贴板、截图、贴图、快捷键、存储和 IPC。
- 使用所有权和 RAII 风格封装资源：`OpenClipboard/CloseClipboard`、`GlobalLock/GlobalUnlock`、COM 引用、D3D/D2D 设备。
- 复杂或高风险平台 API 收敛到 `platform/windows` 模块，稳定后再抽为 crate。

### 4.2 Tauri 2 + Vue 3/TypeScript

- Vue 负责复杂列表、搜索、设置、预览和主题，并优先复用 ClipVault 的 Vue 组件和交互逻辑。
- WebView2 仅在主界面可见或即将显示时预热。
- 大图不走 JSON/base64；首期使用映射文件/应用私有临时文件或 Tauri `Response`/`Channel`，共享缓冲仅作为后续优化路径。
- Vue 组件不得直接读取剪贴板或调用 Win32 API。

### 4.3 Vue 交互与样式

- `@tabler/icons-vue`：统一图标，图标按钮配合 Tooltip 使用。
- Reka UI：Dialog、Popover、Menu、Tooltip 等无样式、可访问交互原语。
- Scoped CSS + CSS Variables：自定义 SnapClip Design Tokens，不引入 Tailwind CSS。
- `@tanstack/vue-virtual`：历史虚拟列表。
- Vue Transition 和 CSS Transition：短时状态动画，尊重系统减少动态效果设置。

## 5. 模块设计

### 5.1 `snapclip-core`

- `PhysicalRect`、`BgraImage`、`RgbaImage`、PNG/DIB 编解码。
- `ClipItem`、`ClipPayloadRef`、内容类型和错误枚举。
- `ScreenshotDocument`、`Shape`、工具状态、撤销/重做。
- 版本化事件和 IPC 数据结构。

来源：SnapClip-old 的 `snapclip-core`、`image.rs`、`snapclip-draw`；ClipVault 的 `types.rs` 和截图数据结构。

### 5.2 `win-platform`

- 隐藏顶层消息窗口、托盘、热键和窗口生命周期。
- `AddClipboardFormatListener`、`WM_CLIPBOARDUPDATE`、剪贴板格式读写。
- WGC、DXGI Desktop Duplication、Magnification、PrintWindow、BitBlt。
- Direct2D/DirectComposition、Layered Window、DWM 属性。
- UI Automation 窗口/元素枚举和滚动容器控制。
- 原生 EDIT 控件和系统 IME。

### 5.3 `clipboard-service`

- 监听事件、序列号去重、代际取消和有界任务队列。
- 文本、图片、HTML/RTF、文件、自定义格式读取。
- BLAKE3 去重、大小限制、隐私模式、应用规则和历史清理。
- 复制、快速粘贴、富文本恢复和系统剪贴板发布回滚。

### 5.4 `capture-service`

- 区域、窗口、元素、全屏和滚动截图。
- 截图覆盖层、标注、同源导出和预览。
- OCR、取色、QR、贴图和截图历史入库。
- 捕获缓存和 GPU/CPU 缓冲池。

### 5.5 `store-service`

- SQLite 初始化、迁移、WAL、FTS5、备份和恢复。
- 元数据与二进制内容分离。
- trigram/LIKE CJK 搜索、DSL 过滤和 KWIC 摘要。
- 内容寻址文件、缩略图、孤儿文件和磁盘配额回收。

### 5.6 `worker-service`

- Windows OCR、RapidOCR、图片编码、缩略图、二维码和用户脚本。
- 有界优先级调度、取消令牌、超时、输出大小限制和 generation 过期任务丢弃。
- 捕获后处理/复制为用户可见高优先级，剪贴板持久化和搜索为常规优先级，OCR/缩略图/QR 为后台优先级；用户脚本隔离执行，不得占满其他队列。
- OCR 模型懒加载，可选低优先级预热；失败时引擎降级。Windows OCR 的 WinRT 初始化、异步调用和 apartment 行为必须在平台适配层验证，不把“必须 MTA”作为未经验证的硬编码前提。
- 缩略图按可见项、邻近预取、后台生成分级；与 worker scheduler 协调预算。高频状态事件按领域 ID 合并为最新状态，搜索及可替换的预览请求采用 generation 取消，避免过期结果覆盖当前视图。

## 6. 截图实现

### 6.1 捕获后端矩阵

```text
Windows 10 1903+:
  窗口/显示器互操作 → WGC CreateForWindow / CreateForMonitor
  全屏高频          → DXGI Desktop Duplication
  其他后端          → Magnification / PrintWindow / BitBlt 按能力与结果降级

Windows 10 1809:
  不调用 1903+ 的 WGC interop 入口；使用经实测可用的 DXGI、Magnification、
  PrintWindow 或 BitBlt 路径，并按窗口/显示器/受保护内容定义能力与限制。

Windows 10 2104+ / Windows 11:
  可按运行时能力评估较新的 Graphics Capture API；不因系统版本推断所有后端必然可用。
```

WGC `CreateForWindow`/`CreateForMonitor` 的最低客户端版本为 Windows 10 1903（Build 18362）。若继续支持
Windows 10 1809，必须实现并测试独立兼容路径；捕获能力在启动时按 API 可用性和运行结果探测，不能只按版本号
判断。最终是否保留 1809 作为最低系统版本由兼容性验收决定。

失败必须返回结构化 `CaptureError`，并记录当前后端、错误码和降级原因。HDR 场景必须查询显示器 HDR 状态和 SDR 白点，预览与导出使用同一色彩转换。

### 6.2 区域截图

1. 热键触发后获取鼠标所在显示器和物理矩形。
2. 复用隐藏覆盖层窗口，避免反复创建 HWND 和 swap chain。
3. 原生覆盖层显示快照、半透明遮罩、选区和工具栏。
4. 捕获和裁剪全程使用物理像素。
5. 复制、保存、贴图、OCR 使用同一份合成图像。

捕获完成后产生一个 canonical capture/artifact，携带尺寸、像素格式及 CPU/GPU 资源或 payload 引用；各消费者共享该产物，
只在格式或生命周期确有要求时派生副本。首期不预设共享内存或固定缓冲数，按 4K/多屏基准测量峰值。

### 6.3 DPI 和多显示器

- 进程声明 Per-Monitor V2 DPI 感知。
- 虚拟桌面坐标允许负值，不通过固定坐标范围判断显示器。
- 鼠标逻辑坐标进入平台层后立即转换为物理像素。
- 每个显示器独立记录原点、尺寸和缩放因子。
- 前端显示尺寸和后端像素尺寸通过显式协议传递。

### 6.4 覆盖层与标注

```text
L0 原始截图
L1 已提交对象缓存
L2 当前拖拽/编辑对象
L3 选区框、手柄、提示和工具栏
```

拖拽时只更新受影响区域；提交和撤销时才全量重放文档。工具不持有宿主窗口指针，只产生 `ToolRequest/Effect`。文本编辑交给原生 EDIT 控件和系统 IME，不自绘组合串。

### 6.5 滚动截图

按目标应用选择 provider：

```text
BrowserProvider          → CDP 全页截图（可用时）
UIAutomationProvider    → ScrollPattern + 区域捕获
GenericStitchProvider   → 滚轮/滚动条 + 图像拼接
```

通用 provider 使用容量为 1 的最新帧槽位、固定区域检测、1D strip SAD/模板匹配和延迟合成。`no_change`、`rejected`、`finished` 分开建模。后续可用相位相关作为粗配准，但必须以真实页面数据验证。

## 7. 贴图实现

贴图必须是独立轻量原生窗口，禁止每个贴图创建 WebView2。

窗口特性：

- `WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`。
- `WS_EX_LAYERED` 或 DirectComposition/D3D11 swap chain。
- 默认置顶但不抢焦点。
- 交互模式与鼠标穿透模式可切换。
- 支持拖动、缩放、透明度、锁定、旋转和快捷键。

资源规则：

- 原图只保留一份 GPU/CPU 显示资源，历史项保存文件引用和缩略图。
- 多个贴图可共享设备级资源；避免每个窗口重复创建 D3D 设备，具体呈现路径由 Windows 实测确定。
- 贴图销毁时显式释放 D3D、GDI、COM 和窗口事件资源。
- 设定并发贴图上限，超过上限时提示或复用最旧窗口。
- 大图可降采样显示，但保存和历史保留原始引用。

## 8. 剪贴板监听与格式管线

### 8.1 事件驱动监听

```text
隐藏顶层窗口
  → AddClipboardFormatListener(hwnd)
  → WM_CLIPBOARDUPDATE
  → 记录 sequence/publication id
  → 有界队列投递后台读取
  → 读取成功后释放剪贴板
  → 编码/OCR/哈希/写库
```

`SetClipboardViewer` 和 `SetWindowsHookEx` 不作为主路径。`GetClipboardSequenceNumber` 仅用于去重和兼容性回退。

监听器使用隐藏窗口注册 `AddClipboardFormatListener`，由系统投递 `WM_CLIPBOARDUPDATE`；退出时必须调用
`RemoveClipboardFormatListener`。消息回调只记录 sequence/publication id 并投递有界任务，不能执行编码、OCR
或数据库写入。是否使用独立消息线程属于实现选择，但不得阻塞 Tauri 主事件循环。

### 8.2 格式快照与解码回退

一次 `WM_CLIPBOARDUPDATE` 应枚举并采集所有受支持且有意义的格式，形成 publication snapshot；格式列表不是
“命中第一个就结束”的跨类型优先级。文本、HTML、RTF、图片、文件各自独立保存；同一语义内部才按解码能力回退：
图片 `CF_DIBV5 → 注册 PNG → CF_DIB`，文本使用 Unicode，HTML/RTF 保留各自表示。

读取时只在必要范围内持有 `OpenClipboard`。文件剪贴板默认保存路径，不自动复制文件实体。延迟渲染格式要设置读取超时和失败回退。格式读取应有总时间/大小预算，发布失败仍按回滚规则处理。

### 8.3 发布与回滚

- 图片：PNG → CF_DIBV5/CF_DIB。
- 富文本：Unicode 文本 → HTML → RTF。
- `SetClipboardData` 成功后句柄所有权转移，立即清空本地句柄变量。
- 任一格式发布失败都执行 `EmptyClipboard`，禁止留下半残发布。
- 重试只针对短暂占用，默认总预算不超过 300 ms。

### 8.4 反向监听去重

SnapClip 自己写入剪贴板后会再次收到更新事件。发布器必须写入内部来源标记或记录最近一次内容哈希，在监听端忽略自身事件，避免 OCR/截图结果重复入库。

## 9. 数据和搜索架构

### 9.1 SQLite

```sql
CREATE TABLE clips (
  id INTEGER PRIMARY KEY,
  primary_type TEXT NOT NULL,
  preview_text TEXT,
  source_app TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  is_pinned INTEGER NOT NULL DEFAULT 0,
  is_favorite INTEGER NOT NULL DEFAULT 0,
  deleted_at INTEGER
);

CREATE TABLE payloads (
  id INTEGER PRIMARY KEY,
  content_hash TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  storage_path TEXT,
  text_content TEXT,
  mime_type TEXT,
  width INTEGER,
  height INTEGER
);

CREATE TABLE clip_payloads (
  clip_id INTEGER NOT NULL REFERENCES clips(id),
  payload_id INTEGER NOT NULL REFERENCES payloads(id),
  role TEXT NOT NULL,
  PRIMARY KEY (clip_id, payload_id, role)
);

CREATE TABLE clip_search (
  clip_id INTEGER PRIMARY KEY REFERENCES clips(id),
  text_content TEXT,
  ocr_text TEXT
);
```

`clips` 表代表一次剪贴板 publication，`payloads` 表按内容哈希去重具体格式，关联表支持一次复制同时持有多种格式。
`clip_search` 是为全文检索维护的轻量投影，可聚合文本和 OCR 内容，不承载二进制载荷。

SQLite 使用 WAL。FTS5 使用 external-content + trigram 索引 `clip_search.text_content` 和 `ocr_text`。
external-content 表必须通过触发器或同一事务保持同步，并提供 rebuild/integrity-check 路径。trigram 全文查询对少于 3 个
Unicode 字符的查询不命中；短查询使用受限 LIKE 子串回退并限制结果量，长查询才走 trigram 索引。
列表默认只取预览字段；搜索结果由后端生成摘要和高亮范围。

### 9.2 内容仓库

```text
%APPDATA%/SnapClip/
  settings.json

%LOCALAPPDATA%/SnapClip/
  data/snapclip.db
  payloads/ab/<blake3>.bin
  thumbnails/<blake3>.webp
  models/
  temp/
  backups/
  logs/
  cache/
  webview2-udf/             # WebView2 独立用户数据目录，不与业务 payload 混放
```

Roaming AppData 仅放体积小、适合漫游的偏好配置；历史数据库和载荷放 Local AppData。通过 Tauri path API 或
Windows Known Folder API 获取目录，不硬编码环境变量。WebView2 UDF 使用应用管理的稳定 Local AppData 目录，
与业务 payload 隔离并纳入升级/异常恢复策略。

WAL 模式允许读写并行，但同一时刻只有一个写事务；所有写操作进入单写者队列，历史/搜索等使用独立只读连接。
队列需定义合并、取消和关闭排空策略，避免多个服务各自争抢写锁。WAL 模式下不能简单复制正在使用的数据库文件作为备份；使用 SQLite Backup API 或 `VACUUM INTO`，并验证
恢复流程、WAL checkpoint 以及数据库和载荷仓库的一致性。载荷 GC 采用事务/标记策略，不能删除仍被记录引用的文件。

写入顺序：写载荷并校验 → 经单写者提交元数据事务及搜索投影 → 低优先级生成缩略图。启动时扫描并清理过期临时文件，低优先级回收孤儿载荷。FTS 更新与所属元数据事务保持原子一致。

### 9.3 查询 DSL

支持：

- 普通关键词：`deploy bugfix`
- 排除词：`-password`
- 标签：`tag:work`
- 来源：`source:Chrome`
- 类型：`type:image`
- 时间：`after:2026-01-01`

中文查询采用 trigram + LIKE 子串语义：至少 3 个字符的查询优先使用 trigram，1~2 个字符或无法利用
索引的模式使用受限 LIKE 扫描。所有路径都必须有最大结果数、超时和取消策略；繁简变体在查询端展开，
避免复制整份 normalized 文本。

## 10. IPC 与事件

### 10.1 UI 命令

- 小参数：Tauri `invoke`。
- 低频生命周期/状态变化：Tauri event，事件名带版本号；高频、有序进度流优先使用 Tauri Channel。
- Tauri event 只承载 JSON 小消息，不能传输大图二进制；大数据通过 `tauri::ipc::Response`、Channel
  分块或应用私有文件/映射文件传递，按场景选择并基准测试。
- 历史缩略图与持久化大图优先返回受控的本地资源引用/payload ID，按需加载缩略图或完整 payload；SharedBuffer 仅评估于明确受益的瞬时高吞吐场景。
- 图片消息优先传 payload id、宽高、格式和所有权/释放语义，不传 HWND/D2D 对象。

### 10.2 进程间协议

- 控制命令：命名管道。
- 小事件：MessagePack 或紧凑 JSON。
- 大图：内存映射、共享句柄或应用私有临时文件。
- 管道 ACL 限制为当前用户 SID。
- 每个请求具有 request id、超时、取消和错误码。
- DTO 包含 `schema_version`、`request_id`/`trace_id` 和稳定错误码；事件消费者需容忍重复、延迟和未知版本，
  并可在丢失/重连后主动查询权威状态。

### 10.3 推荐事件

```text
clip://changed.v1
capture://started.v1
capture://progress.v1
capture://completed.v1
ocr://status.v1
pin://created.v1
pin://closed.v1
```

事件只传领域 ID、变更类型、协议版本和必要状态，不传 HWND、HBITMAP、ID2D1Bitmap、完整 HTML、OCR 全文
或前端组件实例。事件是异步通知，不作为高吞吐、有序任务流的唯一通道；前端订阅必须在视图销毁时解除。

## 11. UI/UX 架构

### 11.1 窗口

| 窗口 | 技术 | 生命周期 |
|---|---|---|
| MainUI 主窗口 | 一个长期 Tauri/WebView2 | 历史、搜索、设置、预览和命令面板以路由/面板呈现；按快捷键显示/隐藏，可保活 |
| 原生辅助窗口 | Win32/D2D 等 | 只用于确有原生输入、像素或生命周期要求的覆盖层/对话界面，不默认再建 WebView |
| 截图覆盖层 | 原生 Win32/D2D | 预创建隐藏、按需显示 |
| 贴图窗口 | 原生 Layered Window | 独立、置顶、不抢焦点 |

首期默认一个 MainUI WebView。设置、搜索和命令面板属于同一 UI 中的路由、弹层或面板，不因视觉上独立就创建
额外 WebView。WebView2 的资源与进程按环境/UDF 管理；确有独立 WebView 需求时须说明隔离收益，并测量额外进程、内存和启动成本。

### 11.2 主界面布局

```text
左侧：类型、标签、收藏、回收站、来源筛选
中间：剪贴板历史虚拟列表
右侧：内容预览、格式、OCR、复制/贴图/导出操作
```

高频操作使用图标按钮和快捷键；不把可用图标按钮改造成过长文本按钮。动画默认 120~180ms，尊重系统减少动态效果设置。
历史列表固定视觉密度，初始行高目标从 64-88 px 范围基准；只显示类型、缩略图、1-3 行预览、时间和来源。
不同格式的完整内容放在右侧预览区按需读取，避免动态行高造成滚动定位和虚拟化抖动。

### 11.3 主题

- SnapClip 自定义 CSS tokens 作为基础，使用 CSS Variables 集中管理颜色、间距、字号、圆角和状态色。
- 浅色、深色、跟随系统三态。
- Windows 11 使用 Mica/Acrylic；低版本回退为纯色半透明。
- 图标使用 `@tabler/icons-vue`；无样式交互原语使用 Reka UI。
- 历史列表使用 `@tanstack/vue-virtual`，大图只加载可见项缩略图。
- 不让截图覆盖层依赖 WebView CSS 才能工作。

## 12. 安全与资源管理

- 所有剪贴板内容默认只存本地，日志不输出正文和图片。
- 本项目及用户拥有的 SnapClip-old 均采用 GNU AGPL v3 体系；SnapClip-old 属于本项目所有者的既有项目，当前内部复用不要求将其项目版权/许可说明另行搬运到本仓库。SnapClip 自身仍按 GNU AGPL v3 管理；未来分发或提供网络交互服务时，按许可证履行相应义务。第三方代码、依赖、模型与素材仍需分别审查。
- 隐私模式支持暂停采集、应用黑名单和富文本原始数据不落盘。
- 脚本在低权限子进程运行，限制超时、输出、工作目录和环境变量。
- OCR 输入路径必须限制在应用数据目录或用户明确选择的路径。
- 剪贴板、GDI、COM、D3D、文件和线程资源必须成对释放。
- 大图任务必须有尺寸上限和取消路径。
- 使用 ETW、WPR/WPA、Process Explorer 监测线程唤醒、句柄、堆和 Private Bytes。
- 性能记录分别统计 Rust 主进程与 WebView2 进程组的 CPU/内存，并测量 GPU 内存、句柄、线程、唤醒次数和 SQLite WAL 大小；覆盖空闲、常规浏览/复制和 4K 截图/多贴图峰值场景。

## 13. SnapClip-old/ClipVault 复用与差距

| 目标能力 | 可复用来源 | v2 补齐事项 |
|---|---|---|
| 原生覆盖层和标注 | SnapClip-old `snapclip-win32`、`snapclip-draw` | WGC/DXGI、设备丢失、多 DPI 验收 |
| 原生贴图 | SnapClip-old `pin.rs`、ClipVault `pin/layered.rs` | 统一缩放/穿透/上限和历史引用 |
| 剪贴板读写 | ClipVault `clipboard`、SnapClip-old PNG/DIB 发布 | 改为事件监听、完整格式回滚 |
| 历史与搜索 | ClipVault `store`、FTS5、虚拟列表 | 统一数据模型和内容仓库 |
| OCR | ClipVault Windows OCR + RapidOCR | Worker 化、模型生命周期和取消 |
| 大图传输 | ClipVault `screenshot/shared_buffer.rs` | 首期文件/映射文件或 Tauri Response/Channel 回退；SharedBuffer 需先解决所有权和释放协议 |
| 滚动截图 | SnapClip-old 设计、ClipVault/Snow Shot 研究 | 先 1D 匹配，再按实测升级 |
| UI | ClipVault Vue 组件和设计令牌 | 复用 Vue 结构，按 SnapClip tokens 和 Tabler Icons 统一视觉 |

## 14. 分阶段迁移计划

### M0：统一契约

- 定义 `ClipItem`、`PayloadRef`、`BgraImage`、事件和错误类型。
- 将 SnapClip-old 图像/标注模型与 ClipVault 存储模型适配。
- 固定协议版本和枚举追加规则。

### M1：历史数据底座

- 实现 SQLite schema/migration、WAL/FTS5、单写者队列、BlobStore、payload 引用和历史查询 API。
- 定义一条 `Clipboard Publication` 可关联多个格式化 `Payload` 的模型，并支持事务一致性、备份/恢复和孤儿载荷回收。

### M2：剪贴板与历史最小闭环

- 实现事件驱动剪贴板监听、文本和图片读取、序列号/自身写入去重。
- 将剪贴板格式快照写入 M1 的 SQLite/BlobStore，完成分页历史查询和复制回写。
- 验证内容模型、隐私策略、存储恢复和基本搜索闭环。

### M3：主 UI 和历史

- Vue 3 主界面、搜索、虚拟列表、设置和预览；优先复用 ClipVault Vue 组件。
- 以分页/游标查询和轻量可见项元数据驱动 UI，不把全量历史载入 Pinia。
- 主窗口隐藏/显示与应用退出生命周期明确；首期单进程退出后不承诺后台 Agent 继续运行。

### M4：截图和贴图闭环

- 复用 SnapClip-old 原生覆盖层、标注、复制、保存和贴图能力，并按 AGPL-3.0 履行来源和发布要求。
- 预创建覆盖层；完成显示/导出同源。
- 完成单显示器 SDR 起步验收，再覆盖 100/125/150/200% DPI、多显示器和设备恢复。
- 增加 HTML/RTF/PNG/DIBV5/文件剪贴板格式、发布回滚和兼容性测试。

### M5：OCR 和截图增强

- OCR worker、截图 OCR 入库、颜色和 QR。
- 按负载基准选择 Tauri Response/Channel 或文件/映射方式；SharedBuffer 仅在所有权/释放协议验证后评估。
- UI Automation 窗口/元素吸附。

### M6：滚动截图和进程拆分

- 手动滚动 + strip 匹配 + 撤销。
- 固定区域检测、自动停止和相位相关候选实现。
- 完成资源和生命周期测量后，再评估是否将监听/历史拆为 ClipAgent，或将捕获能力拆为 CaptureHost。

## 15. 验收计划

### 功能

- 文本、图片、HTML、RTF、文件剪贴板历史完整保存和恢复。
- 区域、窗口、滚动截图；标注、OCR、取色、二维码、贴图闭环。
- 全局快捷键、托盘、快速粘贴、隐私模式和回收站。

### 稳定性

- 剪贴板被占用、延迟渲染、连续快速复制和自身写入不重复。
- Explorer 重启、窗口销毁、系统关机、设备移除和 OCR 失败可恢复。
- 多显示器负坐标、HDR、混合 DPI、UAC 安全桌面行为明确。

### 性能

- 60 秒后台空闲 CPU、唤醒、句柄和内存基线。
- 4K/多屏截图峰值、贴图并发、滚动截图 20,000 px。
- 万级和五万级历史搜索 P50/P95。
- 中文 1/2 字 trigram 回退（如“图”“截图”“AI”“UI”）及含大量 OCR 文本的五万条历史查询。
- OCR 冷启动、预热和回收。

## 16. 架构决策记录

| 编号 | 决策 |
|---|---|
| ADR-001 | Rust 原生核心负责所有 Windows 系统能力 |
| ADR-002 | Vue/Tauri 只负责应用界面，不负责截图像素闭环 |
| ADR-003 | 剪贴板监听以 `AddClipboardFormatListener` 为主 |
| ADR-004 | 贴图优先原生 Layered Window，不创建贴图 WebView |
| ADR-005 | 元数据进 SQLite，二进制进内容寻址文件仓库 |
| ADR-006 | 所有高频后台任务使用有界队列、取消和代际去重 |
| ADR-007 | 屏幕预览、复制、保存和贴图共享同一合成结果 |
| ADR-008 | 滚动截图按 provider 分层，算法渐进升级 |
| ADR-009 | Tauri event 仅传小型 JSON；大数据使用 Response/Channel 或文件/映射传输，并保留回退 |
| ADR-010 | 本项目采用 GNU AGPL v3；SnapClip-old 为同一所有者的既有项目，内部复用无需另行搬运其项目版权/许可说明；第三方代码、依赖和素材另行审查 |
| ADR-011 | 前端使用 Vue 3 + 自定义 CSS tokens + Tabler Icons，不强制引入 Fluent UI 或 Tailwind |
| ADR-012 | 首期一个长期 MainUI WebView；设置/搜索/命令面板复用路由与面板，原生覆盖层和贴图除外 |
| ADR-013 | 一次剪贴板 publication 与多种 payload 分离建模；数据库写入由单写者队列串行化 |
| ADR-014 | 保留 1809 时提供非 WGC-interop 兼容捕获路径；1903+ 才调用 WGC CreateForWindow/Monitor |
| ADR-015 | 截图产物采用 canonical capture，复制/保存/OCR/贴图尽量共享结果，避免重复全尺寸帧 |
| ADR-016 | Worker 按交互优先级调度，具备有界容量、取消/代际失效；缩略图分级调度，事件合并、查询代际取消 |
| ADR-017 | 历史缩略图和大载荷默认使用受控本地资源引用/payload ID；SharedBuffer 仅按需验证 |
| ADR-018 | 历史 UI 使用分页/游标和可见项轻量状态，固定视觉密度，不在前端构建全量历史镜像 |

## 17. 未决问题

1. WGC、DXGI、Magnification 和 BitBlt 的实际兼容矩阵。
2. SharedBuffer 的最终生命周期协议及是否改用映射文件。
3. OCR 模型随包分发还是按需下载，模型常驻内存上限是多少。
4. 是否支持 Win+V 替换，以及注册表/UAC/回滚策略。
5. 是否引入 `rustfft` 或 OpenCV，必须由真实滚动页面 benchmark 决定。
6. 达到什么性能和生命周期指标后拆分 ClipAgent/CaptureHost/worker。

## 附录：建议目录结构

```text
SnapClip/
  src/                    # Vue 主界面
  src-tauri/
    src/
      app/                 # 运行时组装、状态和生命周期
      commands/            # Tauri command 适配层
      events/              # 版本化事件
      domain/              # 平台无关模型和规则
      application/         # clipboard、capture、history、search 用例
      infrastructure/      # SQLite、BlobStore、缩略图、配置与诊断
      workers/             # 有界优先级后台任务
      platform/windows/    # Win32、WGC、D2D 等平台实现
  crates/
    # 在领域边界稳定后按需抽取，不要求首期建立 workspace
    snapclip-core/
    snapclip-platform-windows/
    snapclip-clipboard/
    snapclip-capture/
    snapclip-store/
    snapclip-worker/
    snapclip-ipc/
  models/ocr/
  docs/
  THIRD_PARTY_NOTES.md
```
