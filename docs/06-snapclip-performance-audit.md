我研读了当前代码、OCR 基准和官方资料。结论是：当前最主要的问题不是线程数量，而是重复读取、重复哈希、重复刷新和过早持有资源。

当前仓库实际已经实现的是“Windows 剪贴板监听 + OCR + 历史管理”；真正的截图捕获代码尚未进入 `src-tauri`，截图部分目前主要停留在架构文档。

仓库中已有的基准报告可以作为初始基线，但它们不一定来自当前可编译的工作树。当前工作树中的 `src-tauri/src/ocr/rapid.rs` 仍处于 API 迁移状态，主 crate 暂时无法编译；因此以下数字用于定位热点，不应直接作为当前版本的验收结果。重新测量时应同时记录 commit、模型文件/manifest、OCR 配置、线程数和实际 execution provider。

现有基准结果（来源：`OCR-test-image/bench-cpu-t16-r3.json`、`bench-side1280-t16-r3.json`、`bench-directml-r3.json`；文件中的 `avg`/`p50` 指标不能替代当前工作树的重新验收）：

- CPU medium：端到端约 `1000 ms`，det 约 `580 ms`，rec 约 `330 ms`，PNG 解码约 `84 ms`。
- `max_side_len=1280`：端到端约 `666 ms`。
- DirectML：约 `490 ms`，det 降到约 `65 ms`，rec 仍约 `350 ms`；这说明 det 阶段明显受益，而 rec 仍是主要瓶颈。仅凭这些汇总耗时不能判断 rec 的实际 execution provider，需要按 Session 记录 provider 或使用 ORT profiling 验证。

**最高优先级热点**

| 优先级 | 代码位置                                                     | 根因                                                         | 建议                                                         |
| ------ | ------------------------------------------------------------ | ------------------------------------------------------------ | ------------------------------------------------------------ |
| P0     | src-tauri/src/platform/windows/clipboard.rs:279、src-tauri/src/platform/windows/clipboard.rs:399 | 外层最多 5 次完整读取，内层又最多 5 次 `OpenClipboard`；最坏 25 次打开尝试，成功后还可能重复解码、缩放、编码 | 保留现有 sequence 去重，并补充“只保留最新事件”的队列合并；使用定时重试；只在 clipboard busy/延迟格式未就绪时重试 |
| P0     | src-tauri/src/ocr/rapid.rs:89、src-tauri/src/ocr/manager.rs:28 | 当 `manifest.json` 存在时，`is_available()` 每个 OCR 任务都会读取 manifest 并 SHA-256 校验整个 det/rec/dict 文件；worker 和 manager 一次任务会调用两遍 | 启动时只验证一次，使用 `OnceLock<Result<ModelStatus>>` 缓存结果；运行期间只检查缓存状态 |
| P0     | src/App.vue:87、src/App.vue:90                               | 每次剪贴板更新和每次 OCR 状态更新都清空列表、重新查询、重新挂载行 | 剪贴板事件只增量插入一条；OCR 事件只更新对应 `clipId`；事件做 50～100 ms 合并 |
| P0     | src-tauri/src/lib.rs:45、src/components/HistoryItem.vue:101  | 前端为每个图片请求完整 PNG，后端读取、校验 BLAKE3、Base64 编码；Base64 还增加约 33% 内存 | 入库时生成小尺寸缩略图，只给列表返回缩略图；完整图片仅在选中或打开详情时读取 |
| P1     | src-tauri/src/store/blob.rs:30、src-tauri/src/store/blob.rs:60、src-tauri/src/store/blob.rs:138 | 同一图片至少被多次 BLAKE3：采集、Store 校验、BlobStore 写入、写入后再次校验 | 让 BlobStore 接收已经计算好的 hash；新文件写入后不立即完整重哈希；保留启动时或后台低频完整审计 |
| P1     | src-tauri/src/ocr/rapid.rs:54                                | 当前应用固定使用 medium，且 `max_side_len=2048`              | 增加 `tiny/small/medium` profile；将 small 作为默认候选、medium 作为高精度候选，必须通过精度集和 P95 基准确认；对大图采用 1280～1600 的自适应上限 |
| P1     | src-tauri/src/ocr/win_ocr.rs:90                              | 每次识别都重新创建 Windows `OcrEngine`                       | 使用 `OnceLock<OcrEngine>` 缓存语言引擎，只保留解码和识别调用 |

剪贴板路径应重构成：

```
WM_CLIPBOARDUPDATE
  -> 记录最新 sequence number
  -> 短定时器等待 delayed rendering
  -> OpenClipboard
  -> 只复制原始格式数据
  -> CloseClipboard
  -> 后台线程做 DIB/PNG 解码、缩放、编码、哈希、保存
```

现在 `read_open_clipboard()` 在 clipboard 仍打开时执行 `dib_to_png()` 和 PNG 编码。Microsoft 文档明确要求尽快复制 `GetClipboardData` 返回的数据，并且不要长时间持有或锁定 clipboard 数据：

- [Using the Clipboard](https://learn.microsoft.com/en-us/windows/win32/dataxchg/using-the-clipboard)
- [GetClipboardSequenceNumber](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getclipboardsequencenumber)
- [GetClipboardData](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getclipboarddata)

**OCR 线程和内存**

`crates/rapid-ocr-rs` 当前 `RuntimeConfig::default()` 为：

- `auto_tune_threads = true`
- `enable_cpu_mem_arena = true`
- 每个 ONNX Session 按各自配置创建线程池和 CPU arena；是否真的产生重复线程/arena，需要结合实际构造的 det/cls/rec Session 及进程内存测量确认

建议提供三种运行档：

```
省电：intra=4，inter=1，关闭线程 spinning
平衡：intra=8，inter=1
性能：intra=物理核心数，inter=1
```

ONNX Runtime 官方说明默认线程数接近物理核心数，线程池 spinning 会增加 CPU 和功耗；应针对桌面常驻软件测试并关闭：

- [Thread management](https://onnxruntime.ai/docs/performance/tune-performance/threading.html)

可评估以下配置：

```
session.intra_op.allow_spinning = 0
session.inter_op.allow_spinning = 0
```

`enable_cpu_mem_arena=true` 有利于重复推理速度，但 arena 的内存通常不会主动归还系统。内存受限模式可以关闭 arena，或为 det/rec 共享 allocator。不要直接假设关闭一定更好，需测量 working set 和 P95：

- [ONNX Runtime memory consumption](https://onnxruntime.ai/docs/performance/tune-performance/memory.html)
- [ONNX Runtime C API memory arena](https://onnxruntime.ai/docs/get-started/with-c.html)

当前 SnapClip 的 `Cargo.toml` 只启用了 `ort-runtime`，没有启用 `directml-provider`。因此 DirectML 基准不能直接代表当前应用。启用 DirectML 时要注意：

- det 通常收益明显，rec 可能仍在 CPU；
- DirectML Session 不支持并行执行；
- 同一个 DirectML Session 不应并发调用 `Run`；
- 当前单 OCR worker 的串行结构是合适的；
- 需要记录实际 provider，避免“请求 DirectML、实际 CPU fallback”。

参考：[DirectML Execution Provider](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)。

**前端内存问题**

src/components/HistoryItem.vue 当前有两个明显问题：

1. `sourceExePath` 变化时同时重新加载图片，图标变化不应触发图片请求。
2. `payloads` 深度监听会在 OCR 刷新替换对象后重新加载图片。
3. 后端没有真正使用 `thumbnail` 字段，列表直接获取完整图片。
4. Base64 data URL 在峰值期间可能同时存在于 Rust 字符串、IPC 序列化缓冲、WebView 字符串和解码后的 GPU/DOM 图片内存中；需要用 Private Bytes、Working Set 和图片尺寸实测确认。

建议：

- `sourceExePath` watcher 只刷新图标；
- 图片只监听 `image.contentHash`；
- 增加 `thumbnail` blob 或自定义 Tauri asset protocol；缓存分层、内存 LRU 和磁盘淘汰策略见 `docs/07-screenshot-recording-architecture.md` §12；
- 列表缩略图限制长边 256～320；
- 完整图只加载当前选中项；
- 图标缓存改成有上限的 LRU，当前 src-tauri/src/icon.rs 的 `HashMap` 无上限。

**SQLite 和文件存储**

当前已经使用 WAL 和 `synchronous=NORMAL`，方向是正确的。SQLite 官方说明 WAL 能让读写并行，但 WAL 文件过大时读性能会下降，应定期 checkpoint：

- [SQLite WAL](https://www.sqlite.org/wal.html)

可以进一步改进：

- 为历史查询线程复用只读连接，避免每次 `history_page` 都重新打开 SQLite；
- 对高频 OCR 状态读取复用只读连接；
- `read_payload_bytes()` 当前通过 `WriterRequest::ReadPayloadBytes` 排队到唯一数据库写线程，并在该线程执行一次元数据查询和完整 BLAKE3 校验；图片预览和 OCR 读取会与写入、OCR 状态更新争用同一队列。应拆出专用只读连接/读取服务，按 content hash 直接读取 blob，并把一致性校验移到低频审计或可配置的首次读取路径；
- 使用 `EXPLAIN QUERY PLAN` 验证 `clip_payloads` 查询是否出现全表扫描或临时排序；只有确认现有主键前缀不足时，才增加按 `clip_id`、`role` 的辅助索引；
- 统计 WAL 文件大小并定期 checkpoint；
- 不要让每个图片预览都经过数据库查询、文件读取和完整 BLAKE3 校验。

**截图路径**

当前仓库没有实际的 WGC/DXGI 截图实现。实现截图时建议：

- 小区域或窗口：`Windows.Graphics.Capture`；
- 连续全屏或高频捕获：DXGI Desktop Duplication；
- 只把最终区域复制成 BGRA CPU buffer；
- 不写临时 PNG 再读回；
- 给 OCR 输入增加 `BGRA + stride + DPI` 元数据；
- 大图使用 dirty region 或区域裁剪；
- 复用同一个 canonical capture，保存、预览、OCR 不要各自复制整帧。

官方资料：

- [Windows screen capture](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)
- [Desktop Duplication API](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api)

建议的实施顺序是：

1. 缓存 OCR 模型验证结果。
2. 修复剪贴板事件合并和 clipboard 锁持有时间。
3. 增加缩略图，停止列表加载完整 PNG。
4. 将前端刷新改为增量更新。
5. 消除 BlobStore 的重复哈希。
6. 增加 small/tiny 和 CPU 线程 profile，并用截图黄金集验证识别质量。
7. 再做 DirectML、WIC、WGC/DXGI 的硬件路径评估。

验证时应记录：

```
剪贴板事件 -> 保存完成
保存完成 -> OCR 完成
OpenClipboard 持有时长
每个图片的复制/解码/编码/哈希次数
CPU 平均值和 P95
Private Bytes / Working Set / Commit
GPU Dedicated / Shared Memory
```

每项优化都应至少比较优化前后的 P50/P95 延迟、Private Bytes 峰值、Working Set 峰值和 CPU 时间。模型 profile 切换还必须同时比较 CER/WER、行召回率和数字/URL 等屏幕文本样本，不能只根据端到端耗时选择默认模型。

本次只读验证结果：

- `cargo test --manifest-path crates/rapid-ocr-rs/Cargo.toml --lib`：252 个测试通过。
- `npm run typecheck`：通过。
- SnapClip 主 crate 当前无法编译：工作树中的未提交 `src-tauri/src/ocr/rapid.rs` 仍引用已移除的 `GenericOcrInput`、`GenericOcrOutput` 和旧版 `OcrRequest` 字段。未修改这些用户现有改动。
