# SnapClip 技术选型方案

## 1. 文档信息

| 项目 | 内容 |
|---|---|
| 文档编号 | 02 |
| 文档版本 | v0.1 |
| 文档状态 | 前后端选型基线，具体实现分阶段落地 |
| 目标平台 | Windows 10 1809+、Windows 11 |
| 产品形态 | 截图、标注、OCR、贴图、剪贴板历史一体化工具 |
| 当前前端形态 | Tauri 2 + Vue 3 + TypeScript |
| 当前实现范围 | 定义前后端技术、职责边界和实施顺序 |
| 项目许可证 | GNU AGPL v3 |

本文根据 `docs/01-snapclip-clipvault-architecture-v1.md` 和
`docs/01-snapclip-architecture-v2.md` 编写，并参考 `clipvault` 与 `SnapClip-old`
的 Cargo 配置。本文记录技术选型基线；具体实现和性能参数仍需通过后续阶段验证。

## 2. 选型结论

### 2.1 核心技术栈

| 领域 | 选型 | 决策 |
|---|---|---|
| 桌面容器 | Tauri 2 | 使用 WebView 承载主界面，原生能力由 Rust 提供 |
| 前端框架 | Vue 3 | 便于复用 ClipVault 现有 Vue 组件和交互代码 |
| 语言 | TypeScript | 为组件、状态、IPC DTO 和事件提供静态类型 |
| 构建工具 | Vite | 使用 Tauri 官方 Vue 模板和现有构建链 |
| 图标 | `@tabler/icons-vue` | 统一图标风格，按钮优先使用图标和 Tooltip |
| 样式 | Scoped CSS + CSS Variables | 建立 SnapClip 自有视觉系统，不引入 Tailwind CSS |
| 交互原语 | Reka UI | 提供无样式、可访问的 Dialog、Popover、Menu、Tooltip 等原语 |
| 全局状态 | Pinia | 仅管理 UI 状态、当前页轻量元数据/ID、选中项、查询条件和任务状态；不充当历史数据仓库 |
| 通用组合式工具 | VueUse | 提供键盘、监听、节流、防抖和媒体查询等 composable |
| 历史虚拟化 | `@tanstack/vue-virtual` | 支持万级以上剪贴板历史的可见区域渲染 |
| 运行时校验 | Zod | 校验 Tauri 命令结果和版本化事件载荷 |
| HTML 安全渲染 | DOMPurify | 渲染剪贴板 HTML/富文本前执行清洗 |

### 2.2 明确不采用

- 不使用 Fluent UI React。其视觉风格不是本项目的目标风格。
- 不使用 Tailwind CSS。项目使用语义化 CSS 类和集中式设计令牌。
- 不引入大型全量组件库作为默认视觉系统，例如 Element Plus、Naive UI、Ant Design Vue。
- 不在前端直接实现 Win32、剪贴板监听、截图像素处理、OCR 或 SQLite。
- 不为每个贴图窗口创建 WebView；贴图属于后端原生窗口职责。
- 不在第一阶段引入远程数据请求缓存库。Tauri 本地命令使用 Pinia 和 composable 即可。
- 本项目整体采用 GNU AGPL v3；第三方依赖、模型和素材仍须分别满足其许可证及归属要求。

## 3. 前端职责边界

### 3.1 前端负责

- 单个 MainUI WebView 中主历史、设置、搜索/命令面板的路由或面板呈现。
- 历史列表虚拟化、搜索输入、筛选、排序、选中和键盘导航。
- 文本、图片、HTML/RTF 预览及缩略图展示。
- 收藏、置顶、删除、恢复、复制、贴图等用户操作入口。
- 截图流程的辅助状态、进度、结果预览和错误反馈。
- 主题、密度、面板尺寸和其他非核心 UI 偏好。
- 调用 Tauri 命令、订阅版本化事件，并将后端错误转换为用户可理解的状态。

### 3.2 前端不负责

- 直接读取系统剪贴板或调用 Win32 API。
- 直接编码 PNG/DIB、执行 OCR、生成缩略图或操作 SQLite。
- 在 WebView 中实现完整的屏幕覆盖层、标注像素合成和贴图窗口。
- 将原始大图、原始 HTML 或完整 OCR 文本无条件放入历史列表状态。
- 在日志中输出剪贴板正文、图片内容、文件路径或 OCR 全文。

前端只能依赖领域数据、缩略图引用、任务状态和结构化错误，不依赖 HWND、HBITMAP、D2D
对象或其他平台句柄。

## 4. 目录结构

```text
src/
  app/
    router.ts
    shell/                   # 主窗口组合与全局布局
    commands.ts              # 命令注册和快捷键映射
  features/
    history/
      components/
      composables/
      store.ts
      types.ts
    search/                  # 查询状态与搜索交互
    preview/                 # 单个活动预览会话
    capture/                 # 截图流程辅助界面
    settings/                # 设置和隐私策略
    command-palette/
  shared/
    ui/                      # 无业务语义的基础组件
    icons/
    utils/
  infrastructure/
    tauri/                   # invoke、event 和 DTO 适配器
  styles/
    tokens.css               # 颜色、间距、字号、圆角和状态色
    global.css               # 全局基础样式
  App.vue
  main.ts
```

组件不得直接调用 `invoke` 或监听 Tauri 事件。所有后端通信必须经过
`src/infrastructure/tauri`，再由 feature 消费。功能内的组件、store、composable 和类型尽量放在所属 feature。

## 5. 视觉与样式方案

### 5.1 设计令牌

设计令牌集中写入 `src/styles/tokens.css`，至少包含：

- 背景、表面、边框、正文、次要正文和状态色。
- 4、8、12、16、20、24 等基础间距。
- 小型工具栏、列表行、按钮和输入框的稳定高度。
- 小圆角、面板阴影和焦点环。
- 浅色、深色、跟随系统三种主题状态。

页面区域使用普通布局，不将整个页面包装成嵌套卡片。卡片只用于预览项、对话框和确实
需要边界的重复内容。

### 5.2 交互规范

- 图标按钮优先使用 Tabler 图标，并为不熟悉的图标提供 Tooltip。
- 文本按钮只用于明确的用户命令，例如“复制”“保存”“恢复”。
- 键盘操作与鼠标操作必须具备等价路径。
- 所有异步操作都要提供加载中、成功、失败和可重试状态。
- 尊重系统的减少动态效果设置；动画默认使用短时 CSS Transition。
- 使用 CSS Grid/Flexbox 和稳定尺寸约束，避免列表和工具栏因内容变化而跳动。
- 历史列表保持稳定视觉密度：先以 64-88 px 行高范围做可用性验证，显示类型、缩略图、1-3 行预览、时间和来源；完整内容在预览区按需加载，避免任意动态行高。
- 历史列表保持稳定视觉密度：先以 64-88 px 行高范围做可用性验证，显示类型、缩略图、1-3 行预览、时间和来源；完整内容在预览区按需加载，避免任意动态行高。

## 6. 状态与数据流

### 6.1 Pinia 状态分类

| Store | 职责 |
|---|---|
| `historyStore` | 当前分页/游标、可见 item IDs 与轻量元数据、选中 ID 和删除状态；不缓存全量历史 |
| `searchStore` | 查询文本、过滤条件、排序和搜索状态 |
| `previewStore` | 单个当前预览会话的 payload ID、格式和加载状态；预览数据按需加载，不长期保存大对象 |
| `settingsStore` | 主题、密度、快捷键和隐私 UI 偏好 |
| `taskStore` | 截图、OCR、导出等任务进度和错误 |
| `windowStore` | 面板尺寸、窗口显示状态和跨窗口同步状态 |

Pinia 只保存 UI 状态、当前页 ID/轻量 metadata、当前选中 ID 和任务状态。原始图片、完整 HTML、OCR 大文本、二进制和全量搜索结果不作为常驻状态；列表按分页查询，只持有可见项所需字段、缩略图引用和按需加载所需的 payload id。

搜索和可替换的预览请求带 generation/cancellation 标识；旧请求完成时不得覆盖较新的筛选或选择结果。高频进度/状态事件可按 task/domain ID 合并，避免将事件频率直接变成响应式更新频率。

### 6.2 请求和事件

前端通信分为两类：

```text
invoke 命令  → 用户主动请求、查询、复制、保存和设置变更
Tauri event  → 历史变化、截图进度、OCR 状态和贴图生命周期
```

Tauri event 是异步、单向且仅支持 JSON payload 的通知机制，不传二进制大图；高频有序进度流使用
Channel，命令响应的大块二进制按需要使用 `Response` 或私有文件/映射文件。大图传输需定义所有权、释放、
超时回收和失败回退。事件携带协议版本和领域 ID；消费者处理重复/过期事件，并通过查询恢复权威状态。

事件名称必须带协议版本，例如：

```text
clip://changed.v1
capture://started.v1
capture://progress.v1
capture://completed.v1
ocr://status.v1
pin://created.v1
pin://closed.v1
```

事件只传领域数据，不传前端组件实例、窗口句柄或原生图形对象。

## 7. 路由、窗口与平台 API

- 默认只运行一个长期 MainUI WebView：使用 `vue-router` 管理历史、收藏、回收站、设置、搜索和命令面板视图。
- 原生截图覆盖层、贴图窗口由 Windows 原生窗口实现；不为设置、命令面板或每个辅助功能默认创建独立 WebView。确需独立 UI 窗口时先核算 WebView2 进程组和生命周期成本。
- `@tauri-apps/api/window` 只通过 `infrastructure/tauri` 封装使用。
- `@tauri-apps/plugin-dialog` 仅用于用户主动选择文件或导出目录。
- `@tauri-apps/plugin-opener` 仅用于打开用户明确选择的文件或外部链接。
- `@tauri-apps/plugin-store` 只适合保存主题、面板尺寸等轻量 UI 偏好；业务历史和隐私配置
  仍由后端统一管理。

## 8. IPC 类型与安全

初期可以手写版本化 DTO，并使用 Zod 在前端边界校验。Rust 命令数量稳定后，评估使用
`specta`/`tauri-specta` 生成 TypeScript 类型和调用封装，减少前后端字段漂移。

前端必须遵守以下规则：

- 不信任后端返回的 HTML、文件名、路径和 OCR 内容。
- HTML 预览统一经过 DOMPurify 清洗。
- 不拼接未经校验的文件路径和 URL。
- 不把敏感数据写入浏览器控制台、埋点或错误上报。
- CSP、Tauri capability 和命令权限由后端配置；前端不绕过权限边界。

## 9. 测试与质量门禁

### 9.1 测试工具

- Vitest：纯函数、查询解析、状态转换和 composable。
- Vue Test Utils：列表、预览、筛选、对话框和表单组件。
- Playwright：搜索、键盘导航、复制、收藏、删除和窗口尺寸场景。
- Playwright Screenshot：浅色/深色主题、窄窗口、不同缩放比例的视觉回归。
- Histoire：独立开发和检查基础 UI 组件，可在组件数量达到稳定规模后引入。

### 9.2 必测交互

- 空历史、无搜索结果、加载中、失败和重试。
- 连续搜索、防抖和快速切换筛选条件。
- 虚拟列表滚动、键盘上下选择和焦点恢复。
- 图片缩略图加载失败和大图按需加载。
- 系统深色模式、窗口缩放和高 DPI 下的布局稳定性。
- 后端事件乱序、重复事件和窗口重新打开后的状态恢复。

## 10. 分阶段引入

两份设计文档使用同一条依赖顺序：契约先行，后端历史底座先于剪贴板接入，前端历史 UI 作为后端查询的消费层，截图能力随后接入。

### M0：统一契约与工程基线

- 固定 Rust 2024、Vue 3/TypeScript、版本化 DTO、错误码和事件命名。
- 建立 `infrastructure/tauri`、`app`、按需的 `features` 与 `shared/ui` 目录。
- 完成构建、类型检查、Rust 测试和许可证/依赖记录基线。

### M1：历史数据底座

- 实现 SQLite schema/migration、WAL/FTS5、单写者队列、BLAKE3 BlobStore 和 payload 引用；跨语义类型可共享字节文件但保留独立 payload 元数据。
- 提供分页/游标历史查询 command；读连接与写连接职责分离。
- 验证多格式 publication、事务一致性、BlobStore 原子写入、FTS 同步和启动时孤儿载荷回收；备份/恢复协议作为后续存储验收项。

### M2：剪贴板与历史最小闭环

- 接入 `AddClipboardFormatListener`/`WM_CLIPBOARDUPDATE`、格式快照读取、序列号/自身写入去重。
- 将文本、图片、HTML/RTF 和文件路径写入 M1 的 SQLite/BlobStore，完成复制回写和隐私规则。

### M3：主 UI 历史消费层

- 在单 MainUI WebView 中接入分页查询 DTO、虚拟列表、搜索和筛选；只将可见页轻量状态保存在 Pinia。
- 完成文本、图片、HTML/RTF 和文件路径的预览状态，以及收藏、置顶、删除、恢复反馈。

### M4：截图与贴图闭环

- 接入 SnapClip-old 的原生覆盖层、标注、复制、保存和贴图能力，完成原生窗口闭环。
- 按 Windows 版本捕获能力矩阵验证区域/窗口/全屏、DPI、多显示器、设备恢复和降级路径。

### M5：OCR 和截图增强

- 接入可取消 OCR worker、截图 OCR 入库、取色、二维码和窗口/元素吸附。
- 按负载基准选择 Response/Channel 或文件/映射传输，SharedBuffer 仅按需评估。

### M6：滚动截图、质量与进程演进

- 实现滚动截图 provider、撤销和拼接算法的渐进升级。
- 增加 Playwright/视觉回归、性能基线和资源诊断；根据数据决定是否拆分 ClipAgent/CaptureHost/Worker。

## 11. 当前决策与待定事项

### 已确定

1. 前端使用 Vue 3 + TypeScript。
2. 图标使用 `@tabler/icons-vue`。
3. 样式使用 Scoped CSS + CSS Variables，不使用 Tailwind。
4. 使用 Pinia、VueUse、Reka UI 和 TanStack Virtual。
5. 前端通过 Tauri 命令和版本化事件与后端通信。
6. 大图、剪贴板监听、截图、OCR 和数据库不在 Vue 层实现。

### 已确定但待实现

1. 后端采用 Rust 2024 edition；Tauri 2 作为桌面宿主。
2. 初期保持单 Tauri 应用，按领域模块组织；边界稳定后再拆 workspace crates。
3. 采用平台无关领域模型和 Windows 平台适配层，禁止 Win32 句柄进入领域层。
4. 剪贴板监听以 `AddClipboardFormatListener`/`WM_CLIPBOARDUPDATE` 为主，轮询仅作回退。
5. SQLite 使用 `rusqlite` bundled，启用 WAL/FTS5；大图和二进制进入内容寻址文件仓库。
6. 内容去重使用 BLAKE3；元数据与载荷分离，列表查询只返回预览字段。
7. Windows API 按职责使用 `windows-sys` 和 `windows`，严格控制 feature，避免重复封装。
8. 截图、标注和贴图使用原生 Windows 能力；WebView 只负责主界面和辅助面板。
9. 长耗时任务使用有界、可取消的 worker 队列，不阻塞 Tauri/UI 消息处理。
10. IPC 使用 `serde`/`serde_json` 的版本化 DTO 和结构化错误，稳定后评估 `specta`/`tauri-specta`。
11. OCR、二维码、滚动截图和多进程拆分按阶段接入，不在初期一次性引入全部依赖。

### 后续需要验证

1. WGC、DXGI、BitBlt、PrintWindow 等捕获后端的兼容矩阵和降级顺序。
2. Windows OCR 与 RapidOCR 的模型分发、内存占用和取消行为。
3. SharedBuffer、映射文件或临时文件作为大图 IPC 的最终方案。
4. 单进程方案在性能和生命周期测试后的 ClipAgent/CaptureHost/MainUI 拆分时机。
5. 备份、恢复、孤儿载荷回收和数据库迁移失败时的恢复策略。
6. 固定 Rust toolchain 与 `rust-version`（MSRV），在 Windows CI 验证 Rust 2024、Tauri 和 Windows crate 组合。
7. 队列容量、丢弃/合并规则和延迟预算，由负载测试决定，不预设未经验证的固定数值。

## 12. 后端技术选型

### 12.1 参考工程结论

`clipvault/src-tauri/Cargo.toml` 已验证 Tauri 2、`rusqlite`、BLAKE3、图像处理、
剪贴板和 Windows API 的组合，适合作为能力参考。`SnapClip-old` 的 workspace
按 `snapclip-core`、`snapclip-draw`、`snapclip-overlay`、`snapclip-win32` 分层，
适合作为截图领域边界参考；复用代码须履行 AGPL 和来源标注要求，依赖也应按当前用途逐项选取，不能整份 manifest 照搬。

### 12.2 核心后端选型

| 领域 | 推荐方案 | 决策与边界 |
|---|---|---|
| 桌面宿主 | Tauri 2 + Rust | Vue 通过 command/event 调用，系统能力全部在 Rust |
| Rust edition | Rust 2024 | 作为新项目基线；依赖兼容性由 CI 和锁文件验证 |
| 工程组织 | 初期单 Tauri crate，按领域目录分层；稳定后抽 workspace crates | 控制早期复杂度，保留 `snapclip-core` 等后续拆分边界 |
| 领域模型 | 自有平台无关 core 模块 | 包含 ClipItem、PayloadRef、BgraImage、ScreenshotDocument、错误和版本化 DTO |
| Windows API | `windows-sys` + `windows` 按职责使用 | 低层消息/剪贴板优先 `windows-sys`；WinRT、COM、WGC、D2D 按需启用 `windows` |
| 剪贴板监听 | `AddClipboardFormatListener` + `WM_CLIPBOARDUPDATE` | 事件驱动为主；`GetClipboardSequenceNumber` 仅作去重和回退 |
| 剪贴板格式 | 原生读取/发布 Unicode、PNG、DIBV5、DIB、HTML、RTF、CF_HDROP | `arboard` 可辅助简单读写，但不承担完整格式管线 |
| 数据库 | `rusqlite` + `bundled` + WAL/FTS5 | 集中封装迁移和查询，禁止 command 直接拼 SQL |
| 数据位置 | Roaming AppData 仅轻量偏好；Local AppData 存 DB、payload、缩略图、模型/缓存/日志 | WebView2 UDF 单独位于应用管理的 Local AppData 路径，与业务载荷隔离 |
| 二进制存储 | 内容寻址文件仓库 + SQLite 元数据 | 图片、HTML/RTF 原始字节和其他大对象不直接堆在列表查询中 |
| 内容去重 | `blake3` | 哈希作为载荷引用，重复内容只保留一份 |
| 图像处理 | `image`，关闭默认 features，按需启用 PNG/WebP/BMP/JPEG | 通用编解码与截图像素契约分离 |
| 截图与标注 | 原生 Windows 捕获、覆盖层和标注模块 | 复用 SnapClip-old 的领域分层思想，不让 WebView 处理像素闭环 |
| OCR 与二维码 | 统一 worker 接口，分阶段接入 Windows OCR、RapidOCR 和 QR 解码 | 模型和较重依赖延后到截图闭环后引入 |
| 并发任务 | 有界优先级 worker scheduler | 捕获后处理/用户动作优先；持久化/搜索常规；OCR/缩略图/QR 后台；支持取消、generation 过期、超时和独立脚本限流 |
| 数据库并发 | 单写者队列 + 独立读连接 | WAL 支持读写并行但仅允许一个并发 writer；集中事务、FTS 同步、checkpoint 和备份策略 |
| 序列化与错误 | `serde`/`serde_json` + 版本化 DTO + `thiserror` 领域错误 | 稳定后评估 `specta`/`tauri-specta` 自动生成 TS 类型 |
| 日志诊断 | `tracing` 体系 | 记录阶段、耗时、trace id 和错误码，不记录剪贴板正文或 OCR 全文 |
| 平台插件 | Tauri global-shortcut、dialog、opener、autostart 按需启用 | 自启动默认关闭；特殊 Windows 行为通过平台适配层补充 |

### 12.3 推荐后端目录

```text
src-tauri/
  src/
    app/             # 运行时组装、状态和生命周期
    commands/        # Tauri command 适配层
    events/           # 版本化事件发布
    domain/           # 平台无关模型、规则和错误
    application/      # 用例编排：clipboard、capture、history、search
    infrastructure/   # SQLite、BlobStore、缩略图、配置、日志
    workers/          # 有界优先级调度和后台任务
    platform/
      windows/       # Win32、WGC、D2D、OCR 等适配
```

目录表达依赖职责，不要求 M0 就创建全部空目录；先按当前功能落地，避免架构目录本身变成维护负担。

当模块边界和依赖稳定后，按需拆出：

```text
crates/
  snapclip-core/
  snapclip-store/
  snapclip-clipboard/
  snapclip-capture/
  snapclip-platform-windows/
  snapclip-worker/
```

第一阶段不拆分 `ClipAgent`、`CaptureHost`、`MainUI` 三个进程；多进程是性能和生命周期
验证后的演进选项。

### 12.4 后端实施顺序

1. 定义领域类型、publication/payload 模型、错误类型、版本化 DTO 和 Tauri command/event 适配层。
2. 实现 SQLite schema/migration、WAL/FTS5、单写者队列、BlobStore 和基础历史查询；备份恢复策略在数据生命周期验收阶段完善。
3. 实现剪贴板事件监听、全格式快照读取、去重、隐私规则和发布回滚。
4. 建立单 MainUI WebView 的历史、搜索和设置界面，接入分页查询及虚拟列表。
5. 接入 SnapClip-old 的截图、标注、复制、保存和贴图能力，完成原生窗口闭环及兼容矩阵验证。
6. 接入 OCR、二维码、窗口/元素吸附和滚动截图；之后再以基准数据调整调度、传输方式和进程边界。

### 12.5 许可与复用约束

- SnapClip 与用户拥有的 `SnapClip-old` 均采用 GNU AGPL v3 体系；SnapClip-old 是本项目所有者的既有项目，当前内部复用不是来源不明的第三方代码，也不要求将 SnapClip-old 的项目版权/许可说明另行搬运到本仓库。
- SnapClip 自身保留根目录 `LICENSE`，未来分发衍生作品或提供网络交互服务时，按 AGPL-3.0 履行对应源代码和其他许可证义务。本文不构成法律意见。
- 第三方依赖、模型、图标、DLL 和素材仍需逐项审查；项目所有权不会让第三方内容自动兼容。
- 仅借鉴思想、算法目标和独立接口不同于复制具体实现；对衍生作品边界有疑问时先做许可审查或清洁室重写。
- ClipVault 的依赖清单包含阶段性和可选能力，不整体复制；每个 crate 必须有明确的当前用途。
- 第三方 crate、OCR 模型、图标、DLL 和参考代码的许可证记录应维护在 `THIRD_PARTY_NOTES.md`。

## 13. 审核意见与官方资料核验

### 13.1 采纳的建议

- 首期单 Tauri 进程、内部模块化；进程拆分须由崩溃隔离、内存、启动或设备恢复数据触发。
- 首期一个长期 MainUI WebView；设置、搜索和命令面板放在同一 WebView 内路由/面板呈现；Pinia 限于 UI 状态、分页可见 ID/轻量元数据、选中 ID 和任务状态。
- 以一次 Clipboard Publication 关联多种 Payload 表达多格式复制；payload 内容按哈希去重，UI 历史仅持分页元数据和缩略图引用。
- Roaming AppData 只存适合漫游的小配置；数据库、大载荷、缩略图和缓存使用 Local AppData，WebView2 UDF 作为独立数据目录管理。
- 若保留 Windows 10 1809，提供不依赖 1903 WGC interop 的兼容捕获路线；在 1903+ 才调用 `CreateForWindow`/`CreateForMonitor`，运行时仍需能力检测。
- 固定 Rust toolchain/MSRV，提交应用的 `Cargo.lock` 并在 Windows CI 验证 Edition 2024 与关键依赖。
- `windows-sys` 和 `windows` 按 API 类型明确边界；同一平台能力避免双重封装。
- 剪贴板通知回调只做轻量记录和有界入队；编码、OCR、数据库写入在后台处理。
- 后台工作采用有界优先级调度，缩略图按可见项/预取/后台分级；高频事件合并，搜索请求用 generation/cancellation 防止过期结果覆盖新状态。
- SQLite 写操作集中在单写者队列，读连接分离；WAL checkpoint、FTS 同步、备份恢复与 payload 生命周期纳入存储设计。
- 截图生成共享的 canonical artifact，复制、保存、OCR 和贴图复用，减少全尺寸缓冲复制；具体峰值通过基准建立。
- trigram 全文查询短于 3 个 Unicode 字符时不匹配；短查询使用受限 LIKE 回退，长查询使用 trigram，并基准测索引大小及写入开销。
- 中文搜索基准覆盖 1/2 字查询（如“图”“截图”“AI”“UI”）及 5 万条含大量 OCR 文本的数据集；必要时再评估轻量 bigram 索引，不提前引入独立搜索引擎。
- 明确 FTS external-content 同步、数据库备份/恢复、payload GC 和事件幂等/状态恢复策略。
- OCR 放在可取消 worker，但按实际 WinRT API 线程模型初始化，不固化未经证实的“MTA 必需”假设。
- SharedBuffer 不作首期默认路径；Response、Channel、文件/映射按数据大小和流式需求选择，定义释放协议和回退。
- 性能门槛拆分冷启动/热显示以及后台进程/UI 进程测量；所有具体容量、时延和贴图上限由基准验证。

### 13.2 辩证保留的建议

- 报告提出的队列容量、贴图数量、OCR 超时/释放时间和启动门槛没有本项目实测支撑，因此只要求有界、可取消、可测，不预设具体数值。
- “映射文件优先”不是所有数据的通用最优路径：小响应可使用 command Response，有序流使用 Channel，大载荷可用私有文件/映射；选型按场景 benchmark。
- 组件清单、任务事件全家桶、过细目录分层不是首期必需，按功能和维护成本逐步引入。
- 报告 1 与更新前的报告 2 高度重合；本轮更新后的报告 2 含新增建议，须按新增内容重新评估。报告 3 也单独核对；相同意见不因出现在多份报告中而重复累计为证据。

### 13.3 官方资料

- Rust 2024 Edition Guide：<https://doc.rust-lang.org/stable/edition-guide/rust-2024/>（Rust 1.85 稳定支持 Edition 2024）。
- Rust 2024 Cargo resolver：<https://doc.rust-lang.org/edition-guide/rust-2024/cargo-resolver.html>（edition 2024 默认 resolver 3，并按 `rust-version` 选择兼容依赖）。
- Cargo lockfile：<https://doc.rust-lang.org/stable/cargo/guide/cargo-toml-vs-cargo-lock.html>（lockfile 记录解析后的精确依赖版本，应用项目应纳入版本控制）。
- SQLite FTS5 trigram：<https://sqlite.org/fts5.html#the_trigram_tokenizer>（少于 3 个 Unicode 字符的全文查询不匹配；无可用字面片段的 LIKE/GLOB 会线性扫描）。
- Windows clipboard listener：<https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-addclipboardformatlistener>、<https://learn.microsoft.com/en-us/windows/win32/dataxchg/using-the-clipboard>。
- Windows Graphics Capture interop：<https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nf-windows-graphics-capture-interop-igraphicscaptureiteminterop-createforwindow>、<https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nf-windows-graphics-capture-interop-igraphicscaptureiteminterop-createformonitor>（Win32 interop 最低支持 Windows 10 1903）。
- `GraphicsCaptureItem.TryCreateFromWindowId`：<https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscaptureitem.trycreatefromwindowid>（Windows 10 version 2104 引入；需另外核实应用能力声明要求）。
- WebView2 process model / performance / UDF：<https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/process-model>、<https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/performance>、<https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/user-data-folder>（不同 UDF 对应独立进程组；避免冗余实例/目录并管理 UDF 生命周期）。
- Windows Known Folders：<https://learn.microsoft.com/en-us/windows/win32/shell/knownfolderid>（RoamingAppData 与 LocalAppData 的系统目录定义）。
- SQLite WAL：<https://www.sqlite.org/wal.html>（读写可并行，但单个 WAL 同一时刻只有一个 writer）。
- Windows OCR：<https://learn.microsoft.com/en-us/uwp/api/windows.media.ocr.ocrengine>（异步 `RecognizeAsync`；API 元数据标注 threading model 为 Both，不支持笼统断言必须 MTA）。
- Tauri commands/Response/Channel/events：<https://v2.tauri.app/develop/calling-rust/>、<https://v2.tauri.app/concept/inter-process-communication/>（Events 仅 JSON 单向通知；大响应可用 Response，流式数据推荐 Channel）。
- GNU AGPL v3：<https://www.gnu.org/licenses/agpl-3.0.html>（对应源代码、修改版本、目标代码分发和网络交互义务以许可证正文为准）。
