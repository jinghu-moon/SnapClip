# SnapClip 图片 OCR 实施方案

## 1. 文档信息

| 项目 | 内容 |
|---|---|
| 文档编号 | 03 |
| 文档版本 | v0.1 |
| 文档状态 | 已评审修订，待实现（S1–S6） |
| 目标平台 | Windows 10 1809+、Windows 11 |
| 覆盖范围 | 剪贴板图片自动 OCR、入库检索、可扩展引擎/来源 |
| 当前实现范围 | MVP：剪贴板图片 + Windows OCR；截图/RapidOCR 仅预留 |
| 关联文档 | `docs/01-snapclip-architecture-v2.md`、`docs/02-snapclip-technology-selection.md` |
| 项目许可证 | GNU AGPL v3 |

本文根据评审意见（DIB 输入、payload 去重、队列恢复、并发覆盖）修订，并遵循项目开发期原则。实现以本方案为准。

---

## 2. 开发期原则（前提）

项目处于**开发期、未正式发布**，因此：

1. **鼓励重构与破坏性改动**，从根源解决问题。
2. **不考虑兼容性**：不留兼容 shim、不做双写、不维护旧契约长期别名。
3. **必须以前后测试兜底**：每次破坏性/重构改动，升级后「新功能 + 老功能」均须通过。
4. 允许直接修改公共类型、IPC DTO、SQLite schema；以 `cargo test` 与前端 `typecheck` 绿灯为完成标准。
5. 开发环境旧库可 drop 重建；迁移脚本仍需可测（v1/v2 → v3 单测），但不为「生产平滑升级」预留双轨。

---

## 3. 目标与边界

### 3.1 目标

| 优先级 | 目标 |
|---|---|
| P0 | 剪贴板中的图片自动识别文字，写入库并可被搜索 |
| P0 | 不阻塞剪贴板采集；进程异常后任务可恢复 |
| P0 | 输入兼容 PNG 与 DIB/DIBV5（含「仅有 DIB」场景） |
| P1 | OCR 状态对 IPC/UI 可见（status / engine / error_code） |
| P1 | 历史图片可 backfill / 失败可 retry |
| P2 | 为多引擎、多来源、布局结果预留扩展点 |

### 3.2 非目标（本期）

- 内置截图 / 选区 UI / 截图快捷键
- RapidOCR / PaddleOCR / 云 OCR 接入（仅预留接口）
- OCR 布局标注、图内点选复制
- 多图 payload 的 OCR 结果表（见 §5.3 约束）
- 无约束多 worker 并发

### 3.3 与截图功能的关系

SnapClip **无需内置截图**即可交付 OCR：外部截图工具（PixPin、微信等）写入系统剪贴板后，已有捕获链路会得到图片 payload。内置截图为独立后续功能，通过 `OcrSource::Screenshot` 入队即可复用同一管线。

---

## 4. 总体架构

```text
clipboard worker（已有）
    │ save_publication（图片规范化为 PNG）
    │ try_send(OcrJob)  ──队列满──► 保持 ocr_status=none，计数，backfill 补
    ▼
OcrQueue（内存，非阻塞）
    ▼
OcrWorker ×1（专用线程）
    │  WinRT/COM 初始化、OcrEngine 创建、RecognizeAsync 均在此线程
    ▼
OcrManager ──► OcrEngine trait
                  ├─ WindowsOcrEngine（MVP）
                  ├─ RapidOcrEngine（后续）
                  └─ 其他引擎
    ▼
Store::update_clip_ocr（CAS，单写者队列）
    ▼
clip_search.ocr_text + 状态字段
    ▼
现有 FTS5 external-content 触发器 → 搜索可命中
```

**原则**

- 业务只依赖 `OcrEngine` / `OcrJob` 契约，不绑定 Windows API。
- OCR 读 blob 一律经 **Store/BlobStore**，禁止外部传入 `storage_path` 或未解析的 `payload_id`。
- 全部写操作进入现有 **单写者** 队列；OCR worker 不得自开写连接。
- 入队 `try_send` 失败不得反压剪贴板保存路径。

---

## 5. 数据模型

### 5.1 状态机

```text
none ──enqueue/backfill──► queued ──领取(attempt=n)──► running
                                                          │
                    ┌─────────────────────────────────────┼──────────────────┐
                    ▼                                     ▼                  ▼
                  done                                  failed            skipped
              （CAS 写回）                    （decode/timeout/engine）  （语言包/引擎不可用）
                    ▲                                     │
                    └────────────── retry ◄───────────────┘

queued/running ──启动恢复 reset_stale──► none（再扫描入队）
```

| 状态 | 含义 |
|---|---|
| `none` | 未识别（含队列满放弃、启动恢复后） |
| `queued` | 已入队 |
| `running` | worker 处理中（含 attempt） |
| `done` | 成功，`ocr_text` 已写入 |
| `failed` | 可重试错误；记录 `ocr_error_code` |
| `skipped` | **仅**表示语言包/引擎不可用，不是普通识别错误 |

`ocr_error_code` 受控枚举（禁止自由文本/正文）：

`language_unavailable` | `decode_failed` | `timeout` | `cancelled` | `engine_failed`

### 5.2 Schema v3

```sql
ALTER TABLE clip_search ADD COLUMN ocr_status TEXT NOT NULL DEFAULT 'none';
ALTER TABLE clip_search ADD COLUMN ocr_engine TEXT;
ALTER TABLE clip_search ADD COLUMN ocr_attempt INTEGER NOT NULL DEFAULT 0;
ALTER TABLE clip_search ADD COLUMN ocr_error_code TEXT;
ALTER TABLE clip_search ADD COLUMN ocr_updated_at INTEGER;
CREATE INDEX clip_search_ocr_status ON clip_search(ocr_status);
-- 预留（MVP 不写）：ocr_layout TEXT  — 行/词 bbox JSON
```

保留现有 `ocr_text` 与 FTS5 external-content 触发器；更新 `ocr_text` 即进入检索。

### 5.3 clip 与 payload 关系

- `clip_search` 为 **clip 级一行**；Job 为 **payload 级** 问题。
- **MVP 约束**：每个 clip **至多 1 个** image payload 参与 OCR（与当前剪贴板捕获「一次 publication 至多一张图」一致）。
- 未来多图：新建 `clip_ocr_results(clip_id, payload_id, engine, ...)`，不在此阶段过度设计。
- 入库仍按 `(content_hash, kind)` 去重；**禁止**把输入侧 `payload_id` 当作 canonical ID。

### 5.4 IPC 契约（`ClipSummary` 扩展）

```ts
ocrStatus: "none" | "queued" | "running" | "done" | "failed" | "skipped" | null;
ocrEngine: string | null;
ocrUpdatedAt: number | null;
ocrErrorCode:
  | "language_unavailable"
  | "decode_failed"
  | "timeout"
  | "cancelled"
  | "engine_failed"
  | null;
```

破坏性直接替换 `ClipSummary` / `contracts.ts`（开发期不留兼容默认值）。

---

## 6. 核心接口

### 6.1 OCR 契约

```rust
pub enum OcrSource {
    ClipboardImage,
    Screenshot,   // 预留
    FileImage,    // 预留
}

pub struct OcrJob {
    pub clip_id: String,
    pub content_hash: String,  // 不是 payload_id
    pub source: OcrSource,
    pub attempt: u32,
}

pub enum OcrInput {
    /// 仅由 Store 解析出的规范化 PNG 字节
    ImageBytes { bytes: Arc<[u8]> },
}

pub struct OcrText {
    pub text: String,
    pub layout: Option<OcrLayout>, // MVP 恒为 None
    pub engine: &'static str,
}

pub trait OcrEngine: Send + Sync {
    fn name(&self) -> &'static str;
    fn is_available(&self) -> bool; // 语言包 / 运行时是否可用
    fn recognize(&self, input: &OcrInput) -> Result<OcrText, OcrError>;
}
```

### 6.2 Store 单写者 API

| API | 职责 |
|---|---|
| `save_publication(...)` | 保持采集语义；图片 **规范化为 PNG** 再落 blob |
| `update_clip_ocr(clip_id, attempt, status, text, engine, error_code)` | **CAS** 写回：`WHERE clip_id=? AND ocr_status='running' AND ocr_attempt=?` |
| `list_ocr_candidates(limit)` | 扫描 `ocr_status IN ('none')`（backfill / 启动后入队） |
| `reset_stale_ocr_jobs()` | 启动时：`queued/running → none` |
| `read_payload_bytes(content_hash, kind)` | 经 BlobStore 安全读取，防路径穿越与 TOCTOU |

所有 API 走现有 writer 队列，与剪贴板写入串行一致。

### 6.3 队列语义

- `enqueue` 使用 `try_send`；容量满 → 记结构化计数，**保持 `none`**，由 backfill 补跑。
- MVP **并发固定 1** 个专用 worker 线程；禁止无约束 `1..N`。
- `save_publication` 成功且含 image payload 后异步入队，失败不影响采集结果。

---

## 7. 图片输入规范化（P0）

```text
CF_PNG     → 原样入库（已是 PNG）
CF_DIBV5 / CF_DIB
           → 内存解码为 RGBA → 编码为 PNG → 入库
OCR        → 只消费 PNG 字节，不接触 raw DIB
```

| 规则 | 说明 |
|---|---|
| 规范化时机 | **采集/入库前**一次完成 |
| 解码失败 | 仍尽量保存原图（或跳过 OCR）；`ocr_status=failed` + `decode_failed` |
| 尺寸判断 | `image_dimensions` 可能为空；**先解码再判断**像素数 |
| 缩放阈值 | 长边默认 1920（**初始配置，非已验证结论**），Settings 可配 |
| 资源上限 | 可配置：最大解码像素、内存、单任务超时 |

**验收必须包含：剪贴板仅有 CF_DIB、无 PNG 的场景。**

---

## 8. Worker 与 WinRT

| 约定 | 说明 |
|---|---|
| 线程模型 | 单一专用 OCR worker；WinRT/COM 初始化、`OcrEngine` 创建、`RecognizeAsync` 等待均在该线程 |
| 不做 | 不把 OCR 生命周期绑到 Tauri 主 async runtime |
| crate 边界 | OCR/Imaging 用 `windows` crate；剪贴板/进程沿用 `windows-sys`，职责不混 |
| 超时 | 单任务超时（默认可配，如 10s）→ `failed` + `timeout` + CAS |
| 语言 | 默认 `TryCreateFromUserProfileLanguages`；Settings 可指定；不可用 → `skipped` + `language_unavailable` |
| 日志脱敏 | **禁止**剪贴板正文、图片内容、OCR 全文；仅 clip_id、hash 前缀、status、error_code、耗时 |
| 空闲资源 | 可选：空闲释放 WinRT/图像缓冲（参照 clipvault 思路） |

MTA/COM 假设不得写死，以 `docs/02` 为准，在 worker 内封装并实测。

---

## 9. 实施阶段（S1–S6）

| 阶段 | 内容 | 完成标准 |
|---|---|---|
| **S1** | schema v3、Store OCR API、状态机、v2→v3 migration 测试 | 单测覆盖状态迁移与 CAS |
| **S2** | PNG/DIB 规范化入库；`content_hash` 安全读 Blob | 「仅 DIB」可规范化；无外部 path 注入 |
| **S3** | 单 worker + `WindowsOcrEngine`（WinRT/语言/超时） | 真图中英文识别成功；超时/缺语言包分支正确 |
| **S4** | `save_publication` → `try_send`；启动恢复；attempt CAS | 崩溃后不永久卡 `queued`；旧 attempt 不覆盖新结果 |
| **S5** | `ocr_backfill` / `ocr_retry_failed`；`ClipSummary` 状态字段；可选 UI | 历史可补识；UI 可见状态 |
| **S6** | 集成与回归：DIB-only、FTS 命中、恢复、反压、脱敏 | 测试清单全绿 |

每阶段允许破坏性修改；每阶段结束跑全量 `cargo test` + `npm run typecheck`。

---

## 10. 测试计划

| 类型 | 用例 |
|---|---|
| Migration | v2 → v3；开发库可重建后断言目标 schema |
| 状态机 | none→queued→running→done；failed/skipped；CAS 拒绝过期 attempt |
| 输入 | **仅 DIB** 可 OCR；损坏图 → `decode_failed` |
| 去重 | 相同图片二次复制：canonical payload，不写错 clip |
| 恢复 | `queued` 后杀进程 → `reset_stale` → 可再跑并可搜 |
| 搜索 | 更新 `ocr_text` 后 FTS trigram 命中 |
| 反压 | 队列满时剪贴板仍落库，状态保持 `none` |
| 脱敏 | 日志无正文/图片/OCR 全文 |
| 回归 | 原有 14 项 store/clipboard/domain 测试保持通过 |

---

## 11. 验收标准（MVP）

1. 任意截图/复制图片进入剪贴板（含 DIB-only）→ 图片入库。  
2. 搜索图中中英文关键字 → 命中对应记录。  
3. 剪贴板监听不卡顿；OCR 异步。  
4. 无语言包 → `skipped` + `language_unavailable`，应用不崩。  
5. 崩溃/重启后任务不永久停在 `queued/running`。  
6. 旧 attempt 结果不会覆盖新 attempt。  
7. 全量测试（新 + 老）通过。  

---

## 12. 扩展点（明确预留）

| 扩展 | 接入方式 | MVP 是否改动 |
|---|---|---|
| RapidOCR / Paddle | 新 `OcrEngine` 实现 + Settings 切换 | 否（加模块） |
| 内置截图 | `OcrJob.source = Screenshot` 入队 | 否 |
| 文件图片 | `OcrSource::FileImage` | 否 |
| 布局 / 点字复制 | 填 `ocr_layout` JSON + UI | 仅 UI |
| 多语言 | Engine lang 参数 + Settings | 小改 Windows 引擎 |
| 换引擎重跑 | `ocr_backfill --engine` | 否 |
| 敏感应用跳过 | enqueue 前 filter | 配置化 |
| 性能调参 | worker 数、缩放、超时、像素上限 | 配置化 |
| 多图 payload | `clip_ocr_results` 表 | 否（表结构已预留语义） |

---

## 13. 默认参数（均可配置，需实测）

| 参数 | 初始默认 | 说明 |
|---|---|---|
| worker 并发 | 1 | 基准后再调 |
| 长边缩放 | 1920 | 非最终结论 |
| 任务超时 | 10s | 防卡死 |
| 最大解码像素 | 约 4K 级 | 防 OOM |
| 队列容量 | 小容量 + 丢弃保持 `none` | 不阻塞采集 |
| 主引擎 | Windows OCR | 后续可插拔 |

---

## 14. 风险与对策

| 风险 | 对策 |
|---|---|
| WinRT 桌面 package identity 限制 | 在 worker 内封装；对照 clipvault 可行路径实测；失败记 `skipped`/`engine_failed` |
| 中文语言包未装 | `skipped` + 文档/设置提示安装 OCR 语言包 |
| DIB 变种 | 入库前统一 PNG；覆盖 CF_DIB/CF_DIBV5 测试 |
| 大图内存峰值 | 先解码再限像素/缩放；可配上限 |
| 队列/崩溃导致状态悬挂 | `reset_stale_ocr_jobs` + backfill |
| 并发覆盖 | `ocr_attempt` + CAS |
| 日志泄露 | 受控 error_code + 脱敏约定 + 测试 |

---

## 15. 与现有模块的改动清单

| 路径 | 改动 |
|---|---|
| `src-tauri/src/ocr/*` | 新增：契约、队列、Windows 引擎、Manager |
| `src-tauri/src/store/mod.rs` | migrate v3、`update_clip_ocr`、`list_ocr_candidates`、`reset_stale_ocr_jobs`、`read_payload_bytes`、图片 PNG 规范化 |
| `src-tauri/src/domain.rs` | `ClipSummary` 增加 `ocr*` 字段 |
| `src-tauri/src/platform/windows/clipboard.rs` | 图片 DIB→PNG 规范化后入 payload |
| `src-tauri/src/lib.rs` | 启动恢复、worker 启动、backfill/retry 命令 |
| `src-tauri/Cargo.toml` | `windows` crate 及 Imaging/Ocr features |
| `src/shared/contracts.ts` | `ocrStatus` / `ocrEngine` / `ocrUpdatedAt` / `ocrErrorCode` |
| 前端 HistoryItem / 详情 | 可选展示 OCR 状态（S5） |

---

## 16. 结论

- 路线：剪贴板图片 → 异步单 worker → 可插拔引擎 → CAS 写回 → FTS 检索。  
- 评审阻塞项已纳入：**PNG 规范化、content_hash 安全读、启动恢复、attempt CAS、状态机含 running、IPC 可见、单写者、反压与脱敏**。  
- 开发期允许破坏性重构，但 **S1–S6 每步以测试绿灯收口**。  

实现顺序：**S1 → S6**。
