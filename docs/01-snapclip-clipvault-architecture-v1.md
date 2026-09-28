# SnapClip + ClipVault 二合一效率工具架构文档 v1

## 1. 文档信息

| 项目 | 内容 |
|---|---|
| 文档版本 | v1.0 |
| 文档状态 | 架构基线，供后续实现与评审使用 |
| 目标平台 | Windows 10 1809+、Windows 11 |
| 产品形态 | 截图、标注、OCR、贴图、剪贴板历史一体化工具 |
| 参考工程 | `SnapClip`、`clipvault` 及其 `docs`/`refer` 目录 |
| 当前结论 | 采用 Rust 原生能力 + Tauri/Vue 应用界面 + 独立轻量后台代理 |

本文是融合现有两个项目后的目标架构，不等同于任一现有仓库的当前实现。凡标记为“现状”的内容，表示已经在源码中找到；凡标记为“目标”的内容，必须经过后续实现和验收。

## 2. 架构目标

### 2.1 产品目标

构建一款 Windows 专属效率工具，形成以下闭环：

```text
全局快捷键
   ├─ 截图 → 选区/窗口/滚动截图 → 标注/OCR/取色/二维码 → 复制/保存/贴图/入库
   └─ 剪贴板呼出 → 搜索历史 → 复制或快速粘贴 → 继续工作
```

### 2.2 非功能目标

| 指标 | v1 目标 | 测量方式 |
|---|---:|---|
| 后台空闲 CPU | 接近 0%，禁止固定高频轮询 | 60 秒 ETW/WPA/Process Explorer |
| 剪贴板事件响应 | P95 < 100 ms（不含图片编码/OCR） | QPC 分段计时 |
| 截图热键到覆盖层首帧 | P50 < 80 ms，P95 < 150 ms | QPC：热键、捕获、首帧 Present |
| 主界面首次显示 | P50 < 300 ms | 冷启动与热显示分别测量 |
| 常驻内存 | 后台代理 < 35 MB；主 UI 关闭时不加载 WebView | Private Bytes |
| 截图内存峰值 | 4K 普通截图尽量控制在 2~3 张 BGRA 帧以内 | 4K/多屏压力测试 |
| 搜索延迟 | 万级历史记录 P95 < 50 ms | `perf_probe` + 真实数据集 |
| 大图历史 | 二进制不直接堆积在列表查询中 | SQLite/文件存储统计 |

性能目标是验收门槛，不是未经测量的承诺。WebView2、OCR 模型和多屏 HDR 场景应单独记录资源开销。

## 3. 现有工程研读结论

### 3.1 SnapClip 可复用能力

- Rust workspace 已按 `core`、`draw`、`overlay`、`win32` 分层。
- `BgraImage`、物理像素矩形、PNG/DIB 编码形成了清晰的图像契约。
- 原生 Win32 覆盖层、DirectComposition/D2D 呈现、原生 EDIT + 系统 IME 已有实现。
- 标注文档模型、工具状态机、对象快照式撤销和马赛克/序号等 CPU 光栅路径已存在。
- 原生贴图窗口、托盘、F5 热键、复制/保存/贴图闭环已存在。

### 3.2 SnapClip 当前边界

- 捕获主路径仍是 `BitBlt(SRCCOPY | CAPTUREBLT)`，不是完整 WGC/DXGI/HDR 方案。
- 剪贴板历史、系统剪贴板监听、OCR、滚动截图尚未形成产品闭环。
- GPUI 工具栏与原生覆盖层的状态双向同步、设备移除注入、多 DPI 桌面验收仍不完整。
- 标注当前主要是 CPU 合成后上传 D2D bitmap，不等于完整 D2D 矢量画布。

### 3.3 ClipVault 可复用能力

- Tauri 2 + Vue 3 + Rust + SQLite/FTS5 的完整应用骨架。
- 文本、图片、文件路径、HTML/RTF 的历史数据模型与导入导出。
- BLAKE3 去重、回收站、收藏/置顶、标签、过滤 DSL、虚拟列表和多窗口 UI。
- Windows OCR + RapidOCR 双引擎、OCR 结果回写和搜索集成。
- SharedBuffer 像素传输、ScreenshotCache、RTree/UI Automation 窗口吸附。
- Win32 Layered Window 贴图，避免为每个贴图创建独立 WebView2。

### 3.4 ClipVault 当前风险

- 源码中的剪贴板采集线程仍可见 120~900 ms 自适应轮询路径；必须迁移到事件驱动监听，轮询仅作为兼容回退。
- SharedBuffer 封装包含人为延长借用生命周期的 `unsafe transmute`，必须收敛到同步生命周期或拥有化缓冲协议。
- 截图 OCR 的端到端测试和性能基准在任务文档中仍未完成。
- 部分文档是设计稿或历史版本，不能直接当作已实现行为。

## 4. 总体架构

### 4.1 进程划分

目标采用三进程模型。低频功能可先在一个 Tauri 进程内实现，但接口必须按下述边界设计。

```text
┌─────────────────────────────────────────────────────────┐
│ ClipAgent.exe                                            │
│ 剪贴板监听 / 全局快捷键 / 托盘 / 历史写入 / 快速粘贴      │
│ 事件驱动，用户 Session 内常驻，关闭主 UI 仍运行           │
└───────────────┬───────────────────────┬─────────────────┘
                │ 命名管道/事件          │ 命名管道/事件
┌───────────────▼───────────────┐  ┌────▼─────────────────┐
│ MainUI.exe                    │  │ CaptureHost.exe     │
│ Tauri 2 + Vue + WebView2      │  │ Win32/D2D/WGC       │
│ 历史、搜索、设置、预览、脚本   │  │ 覆盖层、标注、贴图、OCR │
└──────────────────────────────┘  └──────────────────────┘
```

v1 实施允许将 `ClipAgent` 和 `MainUI` 暂时合并为一个 Tauri 进程，原因是 ClipVault 已具备该形态。无论是否拆进程，剪贴板监听线程、OCR、图片编码和脚本执行都不能阻塞 UI 消息循环。

### 4.2 分层原则

```text
领域层（平台无关）
  ClipRecord / ScreenshotDocument / Annotation / SearchQuery / HistoryPolicy

应用层
  CaptureSession / ClipboardSession / OcrJob / PinSession / HistoryService

平台层（Windows 唯一入口）
  Win32 HWND/WndProc / WGC-DXGI-BitBlt / Clipboard API / UIAutomation / D2D

界面层
  Tauri/Vue 主界面、设置、预览、截图辅助 UI

基础设施层
  SQLite/FTS5、文件内容仓库、命名管道、日志、指标、配置迁移
```

原则：平台 API 只在 Windows 适配层出现；领域模型不依赖 HWND、HBITMAP、WebView 或 Tauri 类型；大图通过句柄、文件引用或共享缓冲传输，不通过 JSON 复制。

## 5. 模块职责

### 5.1 `clip-core`

统一领域模型和错误类型：

- `ClipItem`、`ClipPayloadRef`、`ContentType`。
- `PhysicalRect`、`BgraImage`、`RgbaImage` 与坐标转换。
- `ScreenshotDocument`、`Shape`、`ToolKind`、撤销/重做模型。
- `CaptureError`、`ClipboardError`、`OcrError` 等可诊断错误枚举。
- 版本化序列化协议，枚举值只允许追加，不允许重排。

### 5.2 `win-platform`

Windows 原生能力唯一入口：

- 窗口类注册、消息循环、托盘、全局热键。
- `AddClipboardFormatListener`、`WM_CLIPBOARDUPDATE`。
- 剪贴板格式读取/发布：Unicode 文本、PNG、CF_DIBV5、CF_DIB、HTML、RTF、CF_HDROP。
- WGC、DXGI Desktop Duplication、Magnification、BitBlt 降级链。
- UI Automation 窗口/元素枚举与边界吸附。
- Direct2D/DirectComposition、Layered Window、DWM 属性。
- 原生 EDIT 控件和系统 IME。

### 5.3 `clipboard-service`

- 接收剪贴板更新事件。
- 生成 `publication_id`，有界队列背压和代际去重。
- 在不持有 `OpenClipboard` 的情况下完成编码、OCR、数据库写入。
- 内容哈希、敏感应用规则、大小限制和保留策略。
- 历史查询、复制、快速粘贴、导入导出命令。

### 5.4 `capture-service`

- 区域、全屏、窗口、元素截图。
- 滚动截图会话和拼接状态机。
- 标注预览、同源导出、OCR/QR/拾色。
- 贴图窗口生命周期与最大窗口数限制。
- 维护截图缓存，结束会话后显式清理大缓冲。

### 5.5 `store-service`

- SQLite 初始化、迁移、WAL、FTS5、备份和恢复。
- 元数据和二进制内容分离。
- 内容寻址存储与孤儿文件 GC。
- 查询 DSL、CJK/trigram 子串搜索、KWIC 摘要。

### 5.6 `ui-app`

- Vue 3 + TypeScript + Vite。
- 主窗口、设置窗口、片段编辑、预览、截图辅助面板。
- 虚拟列表、键盘导航、主题和设计令牌。
- 只通过命令/事件调用后端，不直接处理 Win32 句柄和大图编码。

### 5.7 `ocr-worker` 与 `script-worker`

- OCR 模型加载、识别和布局结果生成。
- 用户脚本在低权限、超时和输出大小限制下执行。
- 使用有界线程池；任务取消后不得继续持有图片或数据库连接。

## 6. 截图架构

### 6.1 捕获后端优先级

```text
窗口/持续帧捕获 → Windows.Graphics.Capture
全屏高频抓帧   → DXGI Desktop Duplication
需要分层窗口   → Magnification API
后台窗口兜底   → PrintWindow(PW_RENDERFULLCONTENT)
兼容性兜底     → BitBlt + GetDIBits
```

运行时失败必须记录具体原因并缓存能力状态，不能静默改变语义。HDR 显示器需要查询 HDR 状态与 SDR 白点，预览和导出必须使用同一色彩映射路径。

### 6.2 坐标契约

- 捕获、裁剪、拼接和导出统一使用物理像素。
- UI 布局使用逻辑像素，转换函数集中管理。
- 进程声明 Per-Monitor V2 DPI 感知。
- 多显示器允许负坐标和不同缩放比例，不使用硬编码坐标区间猜测显示器。
- 跨显示器选区必须记录每个显示器的原点和 DPI。

### 6.3 覆盖层与标注

覆盖层采用原生窗口，建议采用以下分层：

```text
L0 原始截图
L1 已提交标注缓存
L2 当前拖拽对象
L3 选区框、手柄、提示和工具栏
```

拖拽热路径只重绘 L0+L1+L2+L3；提交、撤销和重做才触发完整对象重放。屏幕显示和复制/保存/贴图必须共用同一个领域合成结果，避免所见与导出不一致。

工具新增流程：实现 `Tool` → 末尾追加稳定枚举 → 注册工厂/命令 → 绑定快捷键 → 添加图标和测试。

### 6.4 滚动截图

采用会话状态机，不把滚动截图实现成一次性命令：

```text
Init → CaptureFrame → WaitStable → Match → AppendStrip
                  ↑                 │
                  └── Undo/Retry ◄──┘
                         │
                 Stop/NoChange/MaxSize
```

第一阶段使用固定区域 1D strip SAD/模板匹配；第二阶段再加入相位相关。实现要求：

- 抓帧与匹配使用容量为 1 的最新帧槽位，禁止无界队列。
- 检测并剔除固定 header/footer/滚动条区域。
- 将 `no_change`、`rejected`、`finished` 分开建模。
- 结果以片段列表延迟合成，提供撤销最后一段。
- 默认最大长图高度，超过上限必须提示并停止。
- WGC → PrintWindow → BitBlt 为显式降级链。

## 7. 剪贴板架构

### 7.1 监听

目标路径：

1. 创建隐藏顶层窗口（不使用依赖广播的 message-only 窗口承载关键系统通知）。
2. 调用 `AddClipboardFormatListener`。
3. `WM_CLIPBOARDUPDATE` 中只记录序列号并投递有界任务。
4. 后台读取时有限重试 `OpenClipboard`，总耗时受预算约束。
5. 退出时注销监听、销毁窗口和停止任务。

轮询 `GetClipboardSequenceNumber` 仅作为系统异常或兼容性回退，默认关闭。

### 7.2 读取优先级

```text
CF_DIBV5 → 注册格式 PNG → CF_DIB → CF_UNICODETEXT
         → HTML Format → Rich Text Format → CF_HDROP
```

实际读取应按格式存在性和内容语义判断：带 RTF 的富文本不能误判成普通图片；文件剪贴板默认保存路径，是否复制文件本体由设置决定。

### 7.3 发布与回滚

图片发布顺序：PNG → CF_DIBV5/CF_DIB；文本发布顺序：Unicode 文本 → HTML → RTF。任何中途失败都必须 `EmptyClipboard`，成功转移所有权的 `HGLOBAL` 句柄立即置空，防止双重释放。

### 7.4 内容模型

```rust
enum ClipPayload {
    Text { text: String, html: Option<Vec<u8>>, rtf: Option<Vec<u8>> },
    Image { width: u32, height: u32, object: PayloadRef },
    Files { paths: Vec<PathBuf> },
}
```

二进制内容写入内容寻址文件仓库，SQLite 只保存引用、预览、哈希和尺寸。重复内容只保留一份载荷。

## 8. 数据存储设计

### 8.1 SQLite 元数据

核心表建议：

```sql
CREATE TABLE clips (
  id INTEGER PRIMARY KEY,
  content_type TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  text_content TEXT,
  html_content BLOB,
  rtf_content BLOB,
  payload_path TEXT,
  preview_text TEXT,
  source_app TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  is_pinned INTEGER NOT NULL DEFAULT 0,
  is_favorite INTEGER NOT NULL DEFAULT 0,
  deleted_at INTEGER,
  ocr_text TEXT,
  ocr_layout TEXT
);
```

FTS5 使用 external-content + trigram，覆盖 `text_content` 和 `ocr_text`。列表查询只返回预览字段；搜索命中后由后端生成 KWIC 摘要，禁止把完整大字段无条件传到前端。

### 8.2 文件仓库

```text
app-data/
  clipvault.db
  payloads/ab/<hash>.bin
  thumbnails/<hash>.webp
  temp/
  backups/
```

写入顺序为“先写并校验载荷，再提交元数据事务”。启动时清理过期临时文件，后台低优先级回收孤儿载荷。

### 8.3 保留与隐私

- 支持按天数、条目数、磁盘配额清理；`0` 明确表示不限制，不能解释为立即清空。
- 收藏/置顶条目默认不自动清理。
- 隐私模式可暂停采集，按应用黑名单跳过记录，富文本原始字节可选择不保存。
- 日志禁止输出剪贴板正文、图片内容和 OCR 全文。

## 9. IPC 与窗口通信

### 9.1 UI 与后端

- 小消息使用 Tauri command/event。
- 大图优先 SharedBuffer；失败回退到应用私有临时文件或映射文件。
- 图片传输消息只包含 `id`、尺寸、像素格式和释放语义，不传 HWND/D2D 对象。
- 前端事件统一使用版本化事件名，例如 `clip://changed.v1`、`capture://progress.v1`。

### 9.2 进程间

- 命名管道传控制命令和小型事件。
- 大图使用临时文件、内存映射或共享句柄。
- 管道 ACL 限制为当前用户 SID。
- 每个请求具有 request id、超时和取消标志。

### 9.3 生命周期

```text
启动 → 单实例互斥 → 初始化配置/数据库 → 创建隐藏消息窗口
     → 注册剪贴板监听/快捷键/托盘 → 主 UI 按需初始化

退出 → 停止接收新任务 → 取消 OCR/编码/脚本 → 刷新数据库
     → 注销监听和快捷键 → 销毁覆盖层/贴图 → 退出进程
```

Windows 关机、Explorer 重启和异常窗口销毁必须有独立清理路径。

## 10. 关键性能与资源策略

- 禁止剪贴板固定频率轮询作为主路径。
- 禁止无界图片帧队列、无界 OCR 队列和无界脚本输出。
- `OpenClipboard` 持有时间只覆盖复制数据，不覆盖编码、OCR、数据库写入。
- 4K BGRA 帧约 31.6 MB，捕获缓冲、预览缓冲和导出缓冲需明确所有权。
- 历史列表必须虚拟化，图片只加载可见项缩略图。
- OCR 模型懒加载；可选低优先级预热，模型失败时回退另一引擎。
- 贴图优先使用 Layered Window，设置并发窗口上限。
- WebView2 主窗口关闭后不应继续刷新或保持大图引用。
- 所有后台线程在退出时可取消、可 join 或由明确的进程生命周期托管。

## 11. 安全与许可

- 剪贴板内容属于敏感数据，默认本地存储，不上传云端。
- 用户脚本运行在受限子进程，限制工作目录、环境变量、执行时长和输出大小。
- Win+V 替换、注册表修改、管理员启动等高风险能力默认关闭，并提供回滚。
- OCR 和图片文件路径必须做边界校验，禁止路径穿越。
- 参考代码必须按文件/目录许可证核对：MIT/Apache 可按许可证要求复用；GPL 代码只借鉴行为和架构，闭源产品不得直接复制其实现。
- 第三方图标、模型、DLL 和许可证记录进入 `THIRD_PARTY_NOTES.md`。

## 12. 测试策略

### 12.1 单元测试

- 几何、裁剪、坐标/DPI 转换。
- PNG/DIB/HTML/RTF 编解码和行序。
- BLAKE3 去重、保留策略、查询 DSL、CJK/KWIC。
- 标注对象重放、撤销/重做、马赛克边界。
- 滚动拼接位移、拒绝帧、到底判定和撤销片段。

### 12.2 Windows 集成测试

- `WM_CLIPBOARDUPDATE` 高频复制与自身写入去重。
- 剪贴板占用重试、发布失败回滚和进程异常退出。
- 多显示器负坐标、100/125/150/200% DPI、HDR。
- 覆盖层、工具栏、贴图 z-order、鼠标穿透、Explorer 重启。
- 中文/日文 IME、emoji、字体回退。
- WGC/DXGI/BitBlt 降级和设备移除恢复。

### 12.3 性能测试

- 60 秒后台空闲 CPU/唤醒/句柄增长。
- 4K、多屏截图峰值内存。
- 5 万条历史搜索 P50/P95。
- OCR 首次加载、已预热和模型回收。
- 3 个贴图窗口、长截图 20,000 px 的内存上限。

## 13. 分阶段落地

### M0：统一契约

- 抽取统一 `ClipItem`、图像格式、错误类型和事件协议。
- 保留 ClipVault 数据库兼容迁移。
- 将 SnapClip 原生截图模块与 ClipVault UI 通过适配层连接。

### M1：截图闭环

- 原生覆盖层、选区、标注、复制/保存/贴图。
- 预创建或复用覆盖层窗口。
- 完成多 DPI、设备丢失和同源导出验收。

### M2：剪贴板事件驱动

- `AddClipboardFormatListener` 替换轮询主路径。
- 完善 PNG/DIBV5/HTML/RTF/文件读取与发布回滚。
- 接入 ClipVault SQLite、FTS5、虚拟列表和富文本粘贴。

### M3：OCR 与截图增强

- OCR 双引擎统一调度，截图 OCR 结果进入历史。
- SharedBuffer 生命周期收敛，提供 PNG/文件回退。
- 取色、QR、窗口/元素吸附和截图历史。

### M4：滚动截图

- 先实现手动滚动 + strip 匹配 + 撤销。
- 再加入固定区域检测、相位相关和自动停止。
- 通过浏览器、Office、聊天窗口和动态页面验收。

### M5：后台拆分与打磨

- 将剪贴板监听/历史写入拆为 `ClipAgent.exe`。
- 主 UI 按需启动，CaptureHost 独立管理原生窗口。
- 完成启动、唤醒率、内存和异常恢复基线。

## 14. v1 架构决策记录

| 编号 | 决策 |
|---|---|
| ADR-001 | Windows 原生截图和贴图不依赖 WebView 像素渲染 |
| ADR-002 | 剪贴板监听以 `AddClipboardFormatListener` 为主，轮询仅回退 |
| ADR-003 | UI 使用 Tauri 2 + Vue 3；后台和像素处理使用 Rust/Win32 |
| ADR-004 | 元数据进 SQLite，图片/二进制进内容寻址文件仓库 |
| ADR-005 | 屏幕预览与复制/保存/贴图共享同一合成结果 |
| ADR-006 | 所有高频任务采用有界队列、取消和代际去重 |
| ADR-007 | 贴图窗口优先 Layered Window，不为每个窗口创建 WebView |
| ADR-008 | 滚动截图先采用 1D 匹配，复杂算法由实测驱动渐进升级 |
| ADR-009 | 领域层不依赖 Windows 句柄、Tauri 或前端框架 |
| ADR-010 | 参考项目只按许可证边界复用，GPL 代码不直接并入闭源实现 |

## 15. 当前未决问题

1. 最终交付形态采用单进程 Tauri，还是拆分 `ClipAgent/CaptureHost/MainUI`。
2. WGC、DXGI 和 BitBlt 的实际设备兼容矩阵及 HDR 色彩验收结果。
3. SharedBuffer 是否保留为主路径，还是统一采用映射文件/共享句柄以降低 `unsafe`。
4. OCR 模型是否随安装包分发，以及模型预热对后台内存的影响。
5. 是否支持 `Win+V` 替换；如支持，需要单独的权限、注册表和回滚设计。
6. 长截图 Phase 2 是否引入 `rustfft`，必须以真实页面数据和性能结果决定。

---

## 附录 A：现有代码到目标模块的映射

| 目标模块 | SnapClip 来源 | ClipVault 来源 |
|---|---|---|
| 图像/几何契约 | `crates/snapclip-core` | `src-tauri/src/screenshot` |
| 标注模型 | `crates/snapclip-draw`、`annotation.rs` | `src/components/ScreenOcr/AnnotationLayer.vue` |
| Win32 捕获 | `crates/snapclip-win32/src/capture.rs` | `src-tauri/src/screenshot/capture.rs` |
| 覆盖层 | `windows_app.rs`、`overlay_render.rs` | `src/screen-ocr` + `screenshot/mod.rs` |
| 剪贴板写入 | `crates/snapclip-win32/src/clipboard.rs` | `src-tauri/src/clipboard/copy.rs`、`win_richtext.rs` |
| 剪贴板采集 | 待实现 | `src-tauri/src/clipboard/capture.rs` |
| 历史/搜索 | 待实现 | `src-tauri/src/store`、`useClipStore.ts` |
| OCR | 待实现 | `src-tauri/src/ocr` |
| 贴图 | `crates/snapclip-win32/src/pin.rs` | `src-tauri/src/pin/layered.rs` |
| 滚动截图 | 设计阶段 | `docs/scroll-screenshot-design.md`、Snow Shot 参考实现 |

## 附录 B：必须避免的反模式

- 用 100~200 ms 定时器轮询剪贴板作为默认实现。
- 在 `WM_CLIPBOARDUPDATE`、WndProc 或 UI 事件回调中直接做 OCR、PNG 编码或数据库事务。
- 将完整 4K RGBA 图像通过 JSON/base64 在前后端反复复制。
- 为每个贴图窗口创建一个 WebView2。
- 用无界 channel 缓存滚动截图帧或 OCR 任务。
- 将窗口逻辑坐标直接当作物理截图坐标。
- 把 CF_DIB/PNG/HTML/RTF 的部分发布当作成功，不做回滚。
- 让领域模型持有 HWND、WebView、D2D 资源或 UI 组件指针。
- 以设计文档中的“已完成”标记代替真实构建、桌面验收和性能测量。
