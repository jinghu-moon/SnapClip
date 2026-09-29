# SnapClip 图片 OCR 实施方案

## 1. 文档信息

| 项目 | 内容 |
|---|---|
| 文档编号 | 03 |
| 文档版本 | **v0.2**（二审修订） |
| 文档状态 | 架构方向通过；补齐本文契约后可进入 S1 |
| 目标平台 | Windows 10 1809+、Windows 11 |
| 覆盖范围 | 剪贴板图片自动 OCR、入库检索、可扩展引擎/来源 |
| 当前实现范围 | MVP：剪贴板图片 + Windows OCR；截图/RapidOCR 仅预留 |
| 关联文档 | `docs/01-snapclip-architecture-v2.md`、`docs/02-snapclip-technology-selection.md`、`AGENTS.md` |
| 项目许可证 | GNU AGPL v3 |

**修订摘要（相对 v0.1）**

| 二审阻塞 | 本版处理 |
|---|---|
| 缺入队/领取原子 API | 新增 `enqueue_ocr` / `claim_ocr_job` / `finish_ocr_job`，`attempt` 由 DB 原子递增 |
| 规范化边界矛盾 | DIB→PNG **仅在** `platform/windows`；Store 只收规范化 payload |
| 超时无取消 | `IAsyncInfo::Cancel` + 取消上下文 + worker Drop/join |
| IPC 未闭环 | `IPC_SCHEMA_VERSION=2`、`history_page` 读 OCR 字段、commands/事件契约 |
| 短查询不搜 OCR | LIKE 同时匹配 `text_content` 与 `ocr_text` |
| 其他 | FTS 触发器收窄、空文本=done、枚举化状态、多图入库约束、失败语义澄清 |

---

## 2. 开发期原则（前提）

项目处于**开发期、未正式发布**，因此：

1. **鼓励重构与破坏性改动**，从根源解决问题。
2. **不考虑兼容性**：不留兼容 shim、不做双写、不维护旧契约长期别名。
3. **必须以前后测试兜底**：每次破坏性/重构改动，升级后「新功能 + 老功能」均须通过。
4. 允许直接修改公共类型、IPC DTO、SQLite schema；以 `cargo test` 与前端 `typecheck` 绿灯为完成标准。
5. 开发环境旧库可 drop 重建；迁移仍需可测（v2→v3 单测），不为生产平滑升级预留双轨。

---

## 3. 目标与边界

### 3.1 目标

| 优先级 | 目标 |
|---|---|
| P0 | 剪贴板图片自动识别文字并入库可搜（含短查询） |
| P0 | 不阻塞采集；崩溃后任务可恢复；无永久卡死状态 |
| P0 | 规范化边界清晰：平台层转 PNG，Store 不感知 DIB |
| P0 | 状态机含 queued/running 且入队/领取/写回均为 DB 原子操作 |
| P1 | OCR 状态对 IPC/UI 可见；backfill / retry 有明确 command |
| P2 | 多引擎、多来源、布局结果预留 |

### 3.2 非目标（本期）

- 内置截图 / 选区 UI
- RapidOCR / 云 OCR 实现（仅预留 trait）
- OCR 布局标注 UI
- 多图 payload 的独立结果表（见 §5.3）
- Settings 语言包选择（**降为后续**；本期固定「用户系统语言包」）
- 多 worker 并发

### 3.3 与截图功能的关系

无需内置截图：外部截图工具写入系统剪贴板后，已有捕获链路得到图片 payload。未来截图经 `OcrSource::Screenshot` 入队即可复用。

---

## 4. 总体架构

```text
platform/windows/clipboard.rs
    │ read CF_PNG / CF_DIB / CF_DIBV5
    │ ──► normalize_image_to_png()   【唯一 DIB 知识点】
    │ hash / size / dimensions 由规范化后的 PNG 计算
    │ 构造 PayloadData
    ▼
Store::save_publication  （只接受规范化 payload，校验 hash/size 一致）
    │  事务内：blob 落盘、clip、clip_search
    │  若含 image：enqueue_ocr(clip_id, content_hash)  ──原子：none→queued, attempt+1
    │  enqueue 失败/队列满：release_queued（queued→none）+ 计数
    ▼
OcrQueue（内存，try_send，去重仅为优化）
    ▼
OcrWorker ×1（专用线程）
    │ claim_ocr_job → (attempt)   ──原子：queued→running
    │ read_payload_bytes(content_hash, image)
    │ WindowsOcrEngine.recognize(input, cancel_token)
    │   WinRT 初始化 / OcrEngine / RecognizeAsync / Cancel 均在此线程
    ▼
finish_ocr_job(clip_id, attempt, result)  ──CAS：running+attempt 写 done/failed/skipped
    ▼
clip_search.ocr_text + 状态 → FTS → 搜索
```

**边界铁律**

1. **DIB 只存在于 `platform/windows`**；Store/Blob/OCR 只见 PNG 字节。  
2. **Job 以 `content_hash` 为准**，禁止外部 `payload_id` / `storage_path`。  
3. **状态迁移全部经 Store 单写者原子 API**；内存队列不是一致性来源。  
4. **attempt 由 DB 生成**，调用方不得自选。

---

## 5. 数据模型

### 5.1 枚举（Rust 侧权威）

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcrStatus { None, Queued, Running, Done, Failed, Skipped }

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcrErrorCode {
    LanguageUnavailable,
    DecodeFailed,
    Timeout,
    Cancelled,
    EngineFailed,
}
```

DB 存字符串仅为序列化形式；**读写均经受控转换**，禁止任意字符串入库。

### 5.2 状态机

```text
none ──enqueue_ocr──► queued ──claim_ocr_job──► running
                         │                        │
                         │                    ┌───┴────┬──────────┐
                         ▼                    ▼        ▼          ▼
              release_queued /              done    failed    skipped
              reset_stale                   (空文本也=done)
                         │                    ▲        │
                         ▼                    └─retry──┘
                        none
```

| 迁移 | API | 语义 |
|---|---|---|
| none → queued | `enqueue_ocr` | 原子递增 `ocr_attempt`，写入 `content_hash` 槽位（见 5.3） |
| queued → running | `claim_ocr_job` | 校验 hash 匹配，返回 **DB 生成的 attempt** |
| running → done/failed/skipped | `finish_ocr_job` | **CAS**：`WHERE status='running' AND attempt=?` |
| queued → none | `release_queued_on_queue_full` / `reset_stale_ocr_jobs` | 入队失败补偿、启动恢复 |
| running → none | `reset_stale_ocr_jobs` | 启动恢复（进程崩溃残留） |
| failed → queued | `retry_ocr_failed` → `enqueue_ocr` | attempt 再次由 DB 递增 |

**成功但文本为空**：`done` + `ocr_text = ''`，**不得**记 `failed`。

`skipped` 仅当 `language_unavailable`（引擎/语言包不可用）。其余错误一律 `failed` + `OcrErrorCode`。

### 5.3 Schema v3

```sql
ALTER TABLE clip_search ADD COLUMN ocr_status TEXT NOT NULL DEFAULT 'none';
ALTER TABLE clip_search ADD COLUMN ocr_engine TEXT;
ALTER TABLE clip_search ADD COLUMN ocr_attempt INTEGER NOT NULL DEFAULT 0;
ALTER TABLE clip_search ADD COLUMN ocr_content_hash TEXT;   -- Job 绑定的 image hash
ALTER TABLE clip_search ADD COLUMN ocr_error_code TEXT;
ALTER TABLE clip_search ADD COLUMN ocr_updated_at INTEGER;
CREATE INDEX clip_search_ocr_status ON clip_search(ocr_status);
-- 预留：ocr_layout TEXT（行/词 bbox JSON）
```

**FTS 触发器收窄**（避免状态字段更新重写索引）：

```sql
DROP TRIGGER clip_search_au;
CREATE TRIGGER clip_search_au
AFTER UPDATE OF text_content, ocr_text ON clip_search BEGIN
  INSERT INTO clip_search_fts(clip_search_fts, rowid, text_content, ocr_text)
  VALUES ('delete', old.rowid, old.text_content, old.ocr_text);
  INSERT INTO clip_search_fts(rowid, text_content, ocr_text)
  VALUES (new.rowid, new.text_content, new.ocr_text);
END;
```

**多图约束（入库时强制，不只文档约定）**

- `insert_publication`：若同 clip 出现 **多于 1 个** `PayloadKind::Image`，  
  - **策略（本版拍板）**：只选定 **一个 canonical image**（取 payload 列表中第一个 Image）绑定 `ocr_content_hash`；其余 image 仅存 blob、不参与 OCR。  
  - 在测试中断言该行为；不静默写错 clip。  
- 未来多图：`clip_ocr_results(clip_id, payload_id, engine)`，不在本期。

### 5.4 IPC 契约

`IPC_SCHEMA_VERSION: 1 → 2`（破坏性，前端同步改）。

```ts
// contracts.ts
export const IPC_SCHEMA_VERSION = 2;

export type OcrStatus = "none" | "queued" | "running" | "done" | "failed" | "skipped";
export type OcrErrorCode =
  | "language_unavailable"
  | "decode_failed"
  | "timeout"
  | "cancelled"
  | "engine_failed";

export interface ClipSummary {
  // ...既有字段
  ocrStatus: OcrStatus;          // 非 null，与 DB NOT NULL 一致
  ocrEngine: string | null;
  ocrUpdatedAt: number | null;
  ocrErrorCode: OcrErrorCode | null;
}
```

**查询链路**：`history_page` SQL **必须** join `clip_search` 读取上述字段（不得只改 DTO）。

**命令契约（Tauri）**

| Command | 入参 | 出参 | 说明 |
|---|---|---|---|
| `ocr_backfill` | `{ limit?: number }` | `{ enqueued, skipped, failed }` | 扫描 `status='none'` 且含 image 的 clip |
| `ocr_retry_failed` | `{ limit?: number }` | `{ enqueued }` | 扫描 `status='failed'` 且可重试 |
| `ocr_get_status` | `{ clipId }` | `{ status, engine, updatedAt, errorCode }` | 单条查询（UI 可选） |

**事件（选用事件而非强制轮询）**

- `ocr://status.v1`：payload `{ clipId, status, engine?, errorCode?, updatedAt }`  
- worker 在 `finish_ocr_job` 成功后 emit；UI 可增量刷新列表行。  
- 无订阅方时安全丢弃（不阻塞 worker）。

---

## 6. 核心 API（完整契约）

### 6.1 OCR 契约

```rust
pub enum OcrSource { ClipboardImage, Screenshot, FileImage }

pub struct OcrJob {
    pub clip_id: String,
    pub content_hash: String,  // 规范化后 PNG 的 blake3
    pub source: OcrSource,
}

pub enum OcrInput {
    Png(Arc<[u8]>),
}

pub struct OcrText {
    pub text: String,              // 可为 ""
    pub layout: Option<OcrLayout>, // MVP: None；预留行/词 bbox
    pub engine: &'static str,
}

pub struct OcrLayout { pub lines: Vec<OcrLine> } // 预留
pub struct OcrLine { pub text: String, pub rect: (f32, f32, f32, f32) }

pub enum OcrError {
    LanguageUnavailable,
    Decode,
    Timeout,
    Cancelled,
    Engine(String), // 实现侧不入 DB，DB 仅存 OcrErrorCode
}

/// 取消上下文：adapter 必须响应
pub struct OcrCancel(Arc<AtomicBool>);
impl OcrCancel {
    pub fn cancel(&self);
    pub fn is_cancelled(&self) -> bool;
}

pub trait OcrEngine: Send + Sync {
    fn name(&self) -> &'static str;
    fn is_available(&self) -> bool;
    /// 必须在超时/取消时尽快返回 Err(Timeout|Cancelled)，
    /// 并尝试取消底层异步（WinRT: IAsyncInfo::Cancel）。
    fn recognize(&self, input: &OcrInput, cancel: &OcrCancel)
        -> Result<OcrText, OcrError>;
}
```

### 6.2 Store 原子 API（单写者）

写请求枚举在现有 `WriterRequest` 上扩展（**不**在 OCR worker 开独立写连接）：

```rust
enum WriterRequest {
    SavePublication { .. },
    EnqueueOcr { clip_id, content_hash, response },          // none→queued
    ClaimOcrJob { clip_id, content_hash, response },         // queued→running → attempt
    FinishOcrJob { clip_id, attempt, outcome, response },    // CAS running→终态
    ReleaseQueued { clip_id, attempt, response },            // queued→none
    ResetStaleOcrJobs { response },                          // queued|running→none
    ListOcrCandidates { filter, limit, response },
    ReadPayloadBytes { content_hash, kind, response },
}
```

| API | 签名/语义 |
|---|---|
| `enqueue_ocr(clip_id, content_hash)` | 仅当 `ocr_status='none'`：`attempt=attempt+1`，`ocr_content_hash=?`，`status='queued'`。已 queued/running → `QueueDecision::AlreadyPending`；failed 由 retry 路径调用。返回 `enqueued { attempt_hint }` / `already_pending` / `not_image` / `not_found` |
| `claim_ocr_job(clip_id, content_hash)` | `UPDATE ... SET status='running' WHERE status='queued' AND ocr_content_hash=?`；成功返回 **该行当前 `ocr_attempt`（DB 产生）**；hash 不匹配/状态不对 → `None` |
| `finish_ocr_job(clip_id, attempt, outcome)` | `UPDATE ... SET status=?, ocr_text=?, ocr_engine=?, ocr_error_code=?, ocr_updated_at=? WHERE clip_id=? AND status='running' AND ocr_attempt=?`；0 行 → 过期 attempt，调用方丢弃结果 |
| `release_queued_on_queue_full(clip_id, attempt)` | 队列满时 `queued→none`（条件含 attempt，防误释放新任务） |
| `reset_stale_ocr_jobs()` | 启动：`queued|running → none`，清空 `ocr_content_hash` 可选 |
| `list_ocr_candidates(filter, limit)` | `filter ∈ {None, Failed}`；**INNER JOIN** 确保存在 image payload；返回 `(clip_id, content_hash)` |
| `read_payload_bytes(content_hash, kind)` | BlobStore 安全读；禁止路径拼接 API 外泄 |

**attempt 规则**：只在 `enqueue_ocr` 内 `ocr_attempt = ocr_attempt + 1`。内存中的 `OcrJob` **不带** attempt；领取后由 claim 返回并作为完成时 CAS 条件。

### 6.3 图片规范化（平台层）

```text
clipboard.rs:
  CF_PNG      → bytes（已是 PNG）
  CF_DIBV5    → decode_dibv5() → RGBA → encode_png()
  CF_DIB      → decode_dib()   → RGBA → encode_png()
  失败        → 不进入 image payload（见下）
  成功        → png_bytes
               → hash=blake3(png_bytes), size, mime=image/png, dimensions
               → PayloadData
               → Store::save_publication
```

**失败语义（澄清 v0.1 矛盾）**

| 情况 | 行为 |
|---|---|
| DIB 解码失败 | **不落 raw DIB**；该次剪贴板若无其它格式则丢弃 image；**不入 OCR**（无 hash）。日志：`decode_failed` 计数（脱敏） |
| PNG 本身损坏 / OCR 解码失败 | 图已按 PNG 入库；`finish(Decode)` → `failed` + `decode_failed`；`retry` 可再试同一 PNG |

→ **OCR 入口永远是 PNG**；`ocr_retry_failed` 不需要认识 DIB。

**Store 职责**：校验 `PayloadRef` 与 bytes 的 hash/size 一致后存 blob；**不**做格式转换。

### 6.4 取消、超时与 Worker 生命周期

| 项 | 约定 |
|---|---|
| 超时 | worker 设 watchdog（默认 10s，可配）；到期 `cancel.cancel()`，再有限等待 |
| WinRT 取消 | adapter 持有 `IAsyncInfo`，超时/取消时调用 **`IAsyncInfo::Cancel`**；等待侧可中断 |
| 结果 | 已 Cancel → `OcrError::Cancelled` → `failed`+`cancelled`；超时未归 → `failed`+`timeout` |
| Worker 停止 | `Drop`：停收队列 → 对当前任务 `cancel` → `join`；进程退出无线程残留 |
| 线程模型 | WinRT/COM 初始化、`OcrEngine` 创建、`RecognizeAsync` **全部**在该专用线程 |
| 不做 | 不把 OCR 生命周期绑到 Tauri 主 async runtime |

### 6.5 队列与反压

- `enqueue_ocr` **先**写 DB（queued），再 `try_send` 内存队列。  
- `try_send` 满：`release_queued_on_queue_full` → 保持 `none` + 计数。  
- **运行期补偿（本版明确）**：  
  1. worker 每完成 N 个任务或每 T 秒，执行一次**有限** `list_ocr_candidates(None, K)` 回填入队；  
  2. 定时（如 5min）轻量 backfill；  
  3. 手动 `ocr_backfill`。  
  内存去重集合仅减少重复入队，**正确性以 DB CAS 为准**。

---

## 7. 搜索（含短查询）

现有 fallback 仅 `text_content LIKE`，**必须改为**：

```sql
AND (
  ?2 = 0 AND (text_content LIKE ?4 ESCAPE '\' OR ocr_text LIKE ?4 ESCAPE '\')
)
```

- FTS 路径（trigram ≥3 字）已覆盖 `ocr_text`，保持不变。  
- **测试**：OCR 文本的 **单字、双字** 中文查询能命中图片 clip。

---

## 8. 实施阶段（二审顺序）

| 阶段 | 内容 | 完成标准 |
|---|---|---|
| **S1** | schema v3、枚举、`enqueue/claim/finish/release/reset` 原子 API、队列去重语义、`IPC_SCHEMA_VERSION=2` + `ClipSummary` 字段、`history_page` 读 OCR 列 | 状态机单测 + CAS/attempt 单测；typecheck |
| **S2** | DIB→PNG 移入 `platform/windows`；hash/dimensions 在规范化后计算；Store 只收规范化 payload；多图 canonical 约束入库 | DIB-only 测试；hash 校验仍通过 |
| **S3** | WindowsOcrEngine + 取消/超时 + worker Drop/join | 取消路径测试（或可控假引擎）；无泄漏 |
| **S4** | save→enqueue→claim→finish；启动 `reset_stale`；队列满 `release`；完成 emit `ocr://status.v1` | 崩溃恢复测试；过期 attempt 不覆盖 |
| **S5** | `ocr_backfill` / `ocr_retry_failed` / `ocr_get_status`；短查询 LIKE OCR；FTS 触发器收窄 | 短查询测试；retry 测试 |
| **S6** | 回归全集：DIB-only、空文本=done、状态-only 更新不重写 FTS、队列满、脱敏、老功能 | 全绿 |

---

## 9. 测试计划

| 类型 | 用例 |
|---|---|
| Migration | v2→v3；触发器 `AFTER UPDATE OF text_content, ocr_text` |
| 原子状态 | enqueue 仅 none；claim 仅 queued+hash；finish CAS；release/reset |
| attempt | 原子递增；旧 attempt finish 被拒 |
| 输入 | **仅 DIB** 规范化后 OCR 成功；DIB 解码失败不落 raw |
| 去重 | 相同图二次复制 → 同 content_hash，不写错 clip |
| 多图 | 两个 image payload → 仅 canonical 进 OCR |
| 空结果 | 识别成功空串 → `done` + `''` |
| 取消/超时 | cancel → `cancelled`；超时 → `timeout`；worker Drop 可 join |
| 恢复 | `queued/running` 残留 → `reset_stale` → 可搜 |
| 搜索 | FTS 命中 + **1～2 字** OCR LIKE 命中 |
| 状态-only 更新 | 改 status 不触发 FTS 重建（可用计数/日志断言） |
| 反压 | 队列满 → `release` → 后续补偿能入队 |
| 脱敏 | 日志无正文/图片/OCR 全文 |
| 回归 | 既有 store/clipboard/domain 测试全过 |

---

## 10. 验收标准（MVP）

1. 复制/截图入剪贴板（含 **DIB-only**）→ 图片以 PNG 入库。  
2. 搜索图中中英文（含单字/双字）→ 命中。  
3. 采集不卡顿；OCR 异步单 worker。  
4. 无语言包 → `skipped` + `language_unavailable`。  
5. 崩溃/重启后无永久 `queued/running`。  
6. 旧 attempt 不覆盖新结果。  
7. 队列满不丢采集，可补偿跑完。  
8. IPC v2 字段可查；`ocr://status.v1` 可选推送。  
9. 全量测试（新 + 老）通过。  

---

## 11. 扩展点

| 扩展 | 接入 | MVP 改动 |
|---|---|---|
| RapidOCR | 实现 `OcrEngine` | 否 |
| 内置截图 | `OcrSource::Screenshot` | 否 |
| 文件图片 | `OcrSource::FileImage` | 否 |
| 布局/点字 | `OcrLayout` + UI | 仅 UI |
| Settings 语言 | Engine 构造参数 | 后续（本期不做） |
| 多图 | `clip_ocr_results` | 否 |
| 多 worker | 放开并发 + 仍用 claim CAS | 配置化 |
| 敏感应用跳过 | enqueue 前 filter | 配置化 |

---

## 12. 默认参数（可配，需实测）

| 参数 | 默认 | 说明 |
|---|---|---|
| worker 并发 | 1 | |
| 任务超时 | 10s | + cancel |
| 长边缩放 | 1920 | 入库后/识别前均可，**须可配** |
| 最大解码像素 | ~4K 级 | 防 OOM |
| 队列容量 | 小 + release 保持 none | |
| 补偿周期 | 完成 N 条 / T 秒有限重扫 + 5min 轻量 backfill | |
| 主引擎 | Windows OCR | 语言=系统用户语言包 |

---

## 13. 改动清单

| 路径 | 改动 |
|---|---|
| `src-tauri/src/platform/windows/clipboard.rs` | DIB→PNG 规范化；规范化后算 hash/尺寸 |
| `src-tauri/src/platform/windows/image_norm.rs`（新） | 唯一 DIB/PNG 转换实现 |
| `src-tauri/src/ocr/*`（新） | 契约、队列、Windows 引擎（含 Cancel）、Manager、worker |
| `src-tauri/src/store/mod.rs` | v3 迁移、原子 OCR API、短查询 LIKE ocr_text、FTS 触发器收窄、canonical image、history 读 OCR 字段 |
| `src-tauri/src/domain.rs` | `IPC_SCHEMA_VERSION=2`；`OcrStatus`/`OcrErrorCode`；`ClipSummary` 扩展 |
| `src-tauri/src/lib.rs` | 启动 reset_stale；commands：backfill/retry/status；emit 事件 |
| `src/shared/contracts.ts` | v2 DTO |
| 前端 | 可选状态展示（S5） |

---

## 14. 风险与对策

| 风险 | 对策 |
|---|---|
| WinRT package identity | worker 内封装；失败 `skipped`/`engine_failed` |
| 取消不彻底 | `IAsyncInfo::Cancel` + 有限等待 + Drop join |
| 状态并发 | DB 原子 enqueue/claim/finish；attempt 递增 |
| DIB 变种 | 仅平台层转换；失败不落 raw |
| 短查询漏搜 | LIKE ocr_text + 测试 |
| FTS 风暴 | 触发器只监听文本列 |
| 队列满 | release + 运行期补偿 + 手动 backfill |
| 日志泄露 | 受控 ErrorCode + 脱敏测试 |

---

## 15. 结论

- 二审 5 项阻塞已纳入契约：**原子 enqueue/claim/finish、平台层规范化、可取消超时、IPC v2 闭环、短查询 OCR**。  
- 实现顺序：**S1 → S6**；S1 完成状态 API 与 IPC 版本后方可进入平台层改造。  
- 开发期允许破坏性修改，每阶段以测试绿灯收口。  

**本版为 S1 开工基线。**
