# 33 滚动截图

本文是滚动截图的唯一设计文档。旧方案（`docs/19、24、25、27、29、30、31、32`）已移入 `docs/archive/`，仅作历史参考。
旧方案的失败原因在于它的核心前提：先等"整帧字节级静止"，再估计位移。真机上的在线页面（光标闪烁、动画、插入符）永远不会静止，所以旧实现拼接步数为 0。

## 1. 不可再还原的事实

| # | 事实 | 证据 |
|---|---|---|
| F1 | 覆盖层在选区上挖空（`SetWindowRgn`）并对其余部分返回 `HTTRANSPARENT` 时，滚轮会直接到达页面 | 两轮真机日志 |
| F2 | WGC 显示器捕获看不到覆盖层（`WDA_EXCLUDEFROMCAPTURE`） | 第 2 轮日志，`wgc-monitor` 裁剪正确 |
| F3 | 在线页面相邻两帧几乎从不字节相同 | 第 2 轮日志 `steps=0` |
| F4 | 滚动 = 选区内"滚动子区域"按整数物理像素纵向平移；固定的头部、底部、侧栏、滚动条不动 | 定义 |
| F5 | 重复内容（空白行、相同行）仅凭内容无法定位，只能拒绝，不能猜 | 第 1 轮日志 |
| F6 | 产物 = 各视口在内容坐标系中的并集，是一张图 | 定义 |
| F7 | 一格滚轮滚多少像素由目标程序决定，覆盖层无法预知 | 第 1 轮日志（20 px 行高假设在 144 DPI 下过冲） |

## 2. 由事实推出的约束

- **F3**：每一帧都要估计，不以静止为前提。
- **F4**：按"列条带 × 行"投票。固定侧栏只污染它所在的条带，固定头部和底部只会投 dy=0。
- **F5**：重复出现的行签名不参与投票，没有唯一答案时返回 `Unknown`。参考帧保持不变，因此往回滚可以恢复。
- **F4 + F5**：固定行永远投 0，所以"0 票最多"不足以判为静止。必须同时满足"不动的行 ≥ 新出现的行"，否则说明页面跳过了重叠区或内容被整体替换，也判 `Unknown`。这一条由测试 `a_jump_under_a_fixed_header_is_unknown_not_still` 锁定。
- **F7**：自动模式只发"一格滚轮"，位移全部由估计器测量，代码里没有行高常量。

## 3. 设计

```
crates/snapclip-capture/src/scroll/
  estimate.rs   FrameSig::of + estimate(ref, cur) -> Shift{dy} | Still | Unknown      纯函数
  stitch.rs     Stitcher：连续 BGRA 画布，pos/top，Append/Prepend/Contained            纯数据
  thumb.rs      缩略图：按块求均值缩到面板宽度，增量更新                                纯函数
  layout.rs     面板（标题、缩略图槽、状态、按钮）的几何与文字                                          纯函数
  mod.rs        Tracker：估计 → 拼接 → 缩略图，每帧调用一次
crates/snapclip-capture/src/windows/
  scroll_worker.rs   捕获线程：WGC 显示器会话 → 只回读选区 → Tracker → 发布 Report → 唤醒覆盖层；
                     自动滚动与滚轮转发共用同一个 post_wheel
  overlay/scroll.rs  覆盖层侧：启动/结束会话、显示 Report、按钮、导出
```

### 3.1 设计名与落地符号

| 设计名 | 落地符号 | 位置 |
|---|---|---|
| `FrameSignature` | `FrameSig` | `crates/snapclip-capture/src/scroll/estimate.rs` |
| `Estimate` | `Estimate` | `crates/snapclip-capture/src/scroll/estimate.rs` |
| `Stitcher` | `Stitcher` | `crates/snapclip-capture/src/scroll/stitch.rs` |
| `BandPlacement` | `Placement` | `crates/snapclip-capture/src/scroll/stitch.rs` |
| `Thumbnail` | `Thumb` | `crates/snapclip-capture/src/scroll/thumb.rs` |
| `Tracker` | `Tracker` | `crates/snapclip-capture/src/scroll/mod.rs` |
| `CaptureLoop` | `capture` | `crates/snapclip-capture/src/windows/scroll_worker.rs` |
| `OverlayController` | `OverlayController` | `crates/snapclip-capture/src/windows/overlay.rs` |

设计名与类型名不是一一对应的地方：`FrameSignature` 与 `Thumbnail` 是设计名，代码里取短名 `FrameSig` / `Thumb`；`BandPlacement` 是"这一帧该往画布的哪一段写"，代码里叫 `Placement`（`Displacement` 这个名字已被 V2 用过又删掉，不再复用）；`CaptureLoop` 不是类型，它是捕获线程的执行体函数 `capture`。

<!-- entity-map:start -->
| 设计名 | 落地符号 | 位置 |
|---|---|---|
| `FrameSignature` | `FrameSig` | `crates/snapclip-capture/src/scroll/estimate.rs` |
| `Estimate` | `Estimate` | `crates/snapclip-capture/src/scroll/estimate.rs` |
| `Stitcher` | `Stitcher` | `crates/snapclip-capture/src/scroll/stitch.rs` |
| `BandPlacement` | `Placement` | `crates/snapclip-capture/src/scroll/stitch.rs` |
| `Thumbnail` | `Thumb` | `crates/snapclip-capture/src/scroll/thumb.rs` |
| `Tracker` | `Tracker` | `crates/snapclip-capture/src/scroll/mod.rs` |
| `CaptureLoop` | `capture` | `crates/snapclip-capture/src/windows/scroll_worker.rs` |
| `OverlayController` | `OverlayController` | `crates/snapclip-capture/src/windows/overlay.rs` |
<!-- entity-map:end -->

### 3.2 估计（`estimate.rs`，O(W·H)）

1. 帧在列方向切成 `STRIPS = 8` 个条带，每个（条带，行）算一个 64 位哈希。
2. 参考帧中同一条带内只出现一次的哈希作为锚点。当前帧里同样只出现一次、且命中锚点的行，为 `dy = y_ref − y_cur` 投一票。
3. 判定规则：
   - **`Shift(dy)`**：票数最高的非零 dy，票数 ≥ `max(8, 锚点数/50)`，且 ≥ 2 × 次优（次优须与它相距 2 以上）。
   - **`Still`**：dy=0 的票数 ≥ 2 × 最优非零票数，且 ≥ `unmatched`（当前帧中唯一、但参考帧没有的行数）。
   - **`Unknown`**：其他所有情况。
4. `|dy| ≤ H − MIN_OVERLAP(32)`。因此选区高度至少为 `MIN_VIEW = 64` 行，过矮时 `begin_scroll` 会拒绝，并保留原来的截图会话。

### 3.2 拼接（`stitch.rs`，采用 snow_shot 的 viewport + band 模型）

- 首帧整帧写入画布。
- 每个 `Shift(dy)` 令 `cand = pos + dy`：
  - 超出底部时为 **Append**：画布增长 `growth` 行，并用当前帧底部的 `band = min(H, max(H/2, growth + H/4))` 行重写画布底部。底部的固定栏因此只留下最后一份，头部只出现一次。
  - 超出顶部时为 **Prepend**：与 Append 镜像。
  - 否则为 **Contained**：只移动 `pos`，不写像素。
- `Still` 时把参考帧换成当前帧（跟随动画）。`Unknown` 时参考帧不变，状态记为 `Lost`。
- 上限：画布同时受**行数**与**字节**两条限制，取更紧的一条。行数上限是 `MAX_SCREENS = 32 × H`；字节上限是 `MAX_BYTES = 512 MiB`，换算成行数为 `MAX_BYTES / (W × 4)`（`row_limit`）。只按行数限制不是资源策略：`32 × H` 在 1241×821 的选区下约 130 MB，但在 3840×2160 下是 1.06 GB，而扩容的一瞬间新旧两块分配同时存在。常见的 1200×1024 选区里 `32 × H` 仍然是更紧的一条（字节侧允许 131072 行，行数侧只给 38400 行），所以字节预算只在极宽或极高的选区上生效。达到上限后停止增长，状态记为 `Full`，自动滚动同时停止。增长按容量倍增摊销（`grow`：目标容量取 `max(所需, min(2×当前容量, 上限))`，再 `try_reserve_exact`），整次会话只搬移画布 O(log n) 次；内存不足时同样返回 `Full`、画布保持原样，而不是让 `Vec::resize` 终止进程。注意 `grow` 的 `limit` 只约束**倍增的选择**，真正拦住越界的是 `place` 在增长之前做的 `rows + growth > max_rows` 检查。向上增长用 `copy_within` 原地平移已有行。（第 4 轮日志：按需精确预留导致每次追加都整块复制，追加耗时从 68 ms 涨到 123 ms；改为摊销后 release 下稳定在约 0.25 ms/次。）

### 3.3 线程与消息

- 捕获线程每帧调用 `next_frame(20 ms)`，回读选区，再调用 `Tracker::push`。只有画布或状态发生变化时，才更新 `Report{preview, status, auto, failure}`，并 `PostThreadMessage(SCROLL_READY_MESSAGE)`。
- 覆盖层线程收到消息后执行 `take_report`，然后依次：上传缩略图 → 重算布局 → 必要时重设窗口区域 → 重绘。
- **自动滚动**：先发送一格 `WM_MOUSEWHEEL(−120)`，**再等页面把它显示出来**（`observe` / `OBSERVE_TIMEOUT = 1000 ms`），确认之后就发下一格。一格滚轮是请求而不是结果：慢页面、远程会话和懒重绘可能要一秒以上才重绘，按固定 `200 ms` 计时会把它们误判成"已到底"。目标窗口这样确定：
  1. 按 Z 序找到选区中心下第一个不属于本进程的可见窗口；
  2. 再用 `ChildWindowFromPointEx` 下钻到最深的子窗口。

  连续 `10` 格都没有测到位移，就判定已到底，自动关闭。判定的依据是**窗口内有没有新帧**，不是墙钟：
  - 仍在重绘的页面会顺延窗口（"连续一秒没有变化"才算一次未响应），慢页面因此有机会画完它已经开始的那一次滚动；
  - 日志会区分两种"没动"：`the page repainted without moving`（有新帧但没位移）与 `no new frame arrived`（根本没有新帧）；
  - 任意一次成功位移都把计数清零。
  `PostMessageW` 返回 `ERROR_ACCESS_DENIED` 时，说明目标以更高完整性级别运行（以管理员身份运行），UIPI 会丢弃低完整性进程投递的输入（`docs/archive/30` §24.6.2 实测）。这时立即关闭自动，状态栏显示「窗口拒绝自动，请手动滚」，不再把 10 次被拒误判为"已到底"。用户自己的滚轮不受影响，再次点「自动」会清除该状态。
- **手动滚动**：滚轮直接落到页面上（F1）。覆盖层若自己收到 `WM_MOUSEWHEEL`，用同一个 `post_wheel` 转发。

### 3.4 覆盖层与交互

| 操作 | 行为 |
|---|---|
| F7 | 打开截图框，并标记"Enter 进入滚动" |
| 左键框选 → Enter | `begin_scroll`。截图会话交回 Idle，覆盖层窗口区域缩为：面板及其阴影 ∪ 选区边缘内侧的边框带（`frame_band`，即边框线宽向上取整）。边框与普通截图是同一个绘制例程 `draw_selection_frame`，只是不画手柄和尺寸标签（`FrameMarks::SCROLLING`）。边框以内是空洞，页面持续绘制并直接接收滚轮 |
| 滚轮 / 「自动」 | 手动或自动滚动。右侧面板中缩略图底部对齐、随内容增长，当前视口以高亮框标出 |
| F9 / Enter / 「完成」 | `finish` 交出长图，走普通截图的 `SelectionPixels` 导出路径。导出线程依次写入：**历史 → 桌面 `SnapClip-{id}.png` → 剪贴板**。剪贴板由应用侧 `clipboard.rs` 直接写入：一份 `CF_DIBV5`（自下而上的 BGRA 行，从任务里已有的像素一次写进剪贴板内存，正高度以兼容 Word），加上刚写好的 PNG 文件原样作为 `PNG` 格式，不再经 arboard 二次编码 PNG（2148×42924 在 release 下从约 1.35 s 降到 154 ms）。不回读解码 PNG，因此不受解码器对不可信输入的 24 MP 上限约束；覆盖层线程只负责发布结果 |
| Esc / 「取消」 | 丢弃，不写任何文件。滚动期间 Esc 注册为临时全局热键（`CANCEL_HOTKEY`），焦点在页面上时同样生效 |

面板宽 160 DIP，与选区相距 8 DIP，优先放在右侧，放不下时翻到左侧。样式取自 `docs/design/overlay-ui-demo.html` 的 `drawScrolling`：圆角卡片带阴影；标题行左侧「长截图」，右侧 `宽×行数`；缩略图槽底部对齐显示尾部，被截断时顶部渐隐，当前视口用强调色描边；状态行；底部一行三个按钮 `F9 完成` / `A 自动` / `Esc 取消`（键名取自当前键位绑定）。

## 4. 验证

### 4.1 自动化测试

运行命令：`cargo test -p snapclip-capture --lib -- --test-threads=1`

- **估计器**：
  - 上下平移都能精确测出；
  - 固定头部和底部、固定侧栏不影响结果；
  - 插入符和动画不影响结果；
  - 有变化但未滚动的页面判为 Still；
  - 重复内容、跳过重叠区、固定头部下的跳跃、不同页面，都判为 Unknown；
  - 页面缩放（Ctrl+滚轮，0.8/1.25/1.5/2×，含每行纯色的条纹页）判为 Unknown，不会把两种比例拼进同一张图。
- **拼接**：
  - 带头部和底部的 4000 行页面经非均匀偏移序列拼接后，与真值**逐字节相等**；
  - 往回滚不改写已有内容；
  - Prepend 正确；
  - 跳跃后能恢复；
  - 达到上限后停止增长；
  - 字节预算成为更紧的一条时，行数上限由它决定（`the_byte_budget_can_be_the_cap_that_stops_the_image`：64 MiB / 1024 宽 → 16384 行，长到 16300 行后再追加即 `Full`，且重复追加不会长出一行）；
  - 内存申请失败时画布保持原样。
- **Tracker**：随机在线滚动后与真值逐字节相等；Lost 后往回滚能恢复；Full 状态正确；预览跟随视口。
- **缩略图与布局**：
  - 缩略图每个像素等于对应块的均值，增量更新时只重算 band；
  - 面板位于右侧且底部对齐、随内容增长、空间不足时翻到左侧、随 DPI 缩放；
  - 三个按钮依序铺满底部一行，命中测试正确；阴影不触及选区。
- **边框**：普通截图与滚动截图的边框都画在选区边缘内侧（GPU 像素测试）；滚动时选区内部与边框、面板以外全透明，没有手柄。
- **覆盖层**：窗口区域不覆盖选区内部（边框带以内），边框带四周封闭。
- **自动滚动**：只有 `ERROR_ACCESS_DENIED` 被视为拒绝；拒绝后自动关闭并进入 Refused，重新开启后清除；目标窗口变化时记录；目标的类名取自真实窗口。**等待闭环**：一帧显示位移即结束当前 notch；未到期无位移时仍在等待；到期区分"重绘但没位移"与"没有新帧"；重绘会顺延窗口（慢页面不会被旧期限判死）；连续 `AUTO_IDLE_NOTCHES` 次未响应才停止，任何一次位移都清零（`a_notch_ends_the_instant_a_frame_shows_it`、`a_notch_that_times_out_reports_what_the_page_did`、`a_page_that_keeps_repainting_gets_to_finish_the_scroll_it_started`、`auto_gives_up_only_after_a_run_of_idle_notches`、`the_closed_loop_sets_the_cadence_from_the_page`——最后一条用虚拟时间走一遍闭环，把节奏交给页面：一帧应答的页面仍是 `AUTO_INTERVAL` 一击，一个窗口才应答的页面按窗口计，每 5 s 才出一帧的页面十击后放弃）。
- **导出**：长图（1250×25826）的剪贴板拿到的就是任务自己的像素缓冲；桌面目录不可用时剪贴板照常；历史写入失败时不做任何复制。
- **性能证据**（`--ignored`）：
  - `the_cost_of_one_estimate_at_the_field_selection_size`：1241×821 下单次估计的耗时（实测 991 µs）；
  - `the_overlay_thread_never_blocks_longer_than_eight_ms`：一次缩略图上传加整帧合成的耗时，上限 8 ms；
  - `the_cost_of_one_frame_at_the_field_size`：2148×1344 一帧的 CPU 成本，按签名 / 估计 / 拼接 / 缩略图 / 预览拆开；
  - `the_cost_of_one_region_readback_at_the_field_selection_size`：2148×1344 区域从 3840×2160 帧回读的耗时，按 staging 创建 / 复用纹理的阻塞 Map / 主机行拷贝 / `DO_NOT_WAIT` 轮询拆开。

  实测（release，本机一次运行，样本 30–99）：

  | 阶段 | p50 | p95 | 说明 |
  | --- | --- | --- | --- |
  | 区域回读（全帧→区域） | 2545 µs | 3145 µs | 11.5 MB，`create=32 µs`、阻塞 Map+拷贝 1325 µs、主机行拷贝 1666 µs |
  | 签名 `FrameSig::of` | 1221 µs | 1417 µs | 8 条带 × 1344 行，O(W·H)，已接近内存带宽 |
  | 估计 `estimate` | 324 µs | 420 µs | 每帧重建 2 个 `HashMap` |
  | 拼接 `place` | < 100 µs | | 0.83 MB 一次 memcpy，`Instant` 分辨不出 |
  | 缩略图 + 预览 | 19 µs | 29 µs | |
  | 一帧 CPU 合计 `push` | 2383 µs | 4368 µs | 峰值 38 ms 是画布倍增重分配，全会话仅 O(log n) 次 |

  上表是合成探针（同一块空闲 GPU，只跑捕获线程）。**真机不是这个数**：一次自动滚动会话 870 帧、选区 1577×858、长图 1577×7664，收尾聚合行读数为

  | 阶段 | 真机 mean | 说明 |
  | --- | --- | --- |
  | 区域回读 `read_region` | 1311 µs | 占每帧 10%，与合成探针同量级 |
  | 其余（`tracker.push`：签名 + 估计 + 拼接 + 缩略图） | 11631 µs | 占每帧 90% |
  | 一帧合计 `us` | 12942 µs | 合成探针同链路约 3.3 ms，真机慢约 4× |

  从上表看，真正贵的不是回读而是 `push` 那 11.6 ms，所以**在把 `push` 拆成签名 / 估计 / 拼接 / 缩略图之前，不要因为"回读很贵"去改回读**：合成探针里这三段拆开后合计只有约 1.7 ms，与真机的 11.6 ms 差一个数量级，说明缺口不在已测量的算法里。逐帧拆分由 `SNAPCLIP_SCROLL_VERBOSE` 打开，收尾聚合行（`stopped frames=… read_mean_us=… push_mean_us=…`）默认打印，见 §4.3。

  **回读池化（多缓冲异步回读）已否决，有探针。** `the_latency_a_staging_pool_would_hide_at_the_field_selection_size` 把一帧跑完整（队列里排一次拷贝 → 花 11.6 ms 处理上一帧 → 取回这一帧），同一块 11.5 MB 区域、`--release`：

  | 变体 | 回读等待 p50 | 一帧 p50 | 一帧 max |
  | --- | --- | --- | --- |
  | 现状（每帧新建 staging + 阻塞 Map） | 1196 µs | 1473 µs | 3815 µs |
  | 双槽池 + `DO_NOT_WAIT` 轮询 | 1717 µs | 2357 µs | 2939 µs |
  | 池 + 无 CPU 工作（拷贝自身的落地时间） | 1851 µs | 1851 µs | 2366 µs |

  也就是说：拷贝要 1.85 ms 才落地，但阻塞 `Map` 在真实工作负载里只等 1.2 ms——**GPU 已经在管线里跑在 CPU 前面了**，池化没有东西可藏，反而因为 50 µs 一档的轮询多花约 0.9 ms/帧。同一份代码里 `DO_NOT_WAIT` 单次轮询（2331 µs，4 次睡眠）也慢于阻塞 Map。结论：**不要为异步回读增加复杂度**；回读真正的大头是 11.5 MB 的主机行拷贝（约 1.6 ms），那与缓冲数量无关。

### 4.2 真机测试步骤

1. 在 Chrome 长页面上：F7 → 框选 → Enter → 手动滚动。确认右侧缩略图底部对齐并随滚动增长。
2. 按 F9，检查三处输出：桌面 `SnapClip-*.png`、剪贴板粘贴、历史条目。
3. 再来一轮，点「自动」，确认滚到页底后自动停止，状态显示"滚动页面"。
4. 再来一轮，按 Esc，确认不产生任何文件。

### 4.3 日志字段（`[snapclip][scroll]`）

| 行 | 含义 |
|---|---|
| `session=… selection=… aim=(x,y) dpi=…` | 会话开始；`aim` 是选区中心的屏幕坐标，也是自动滚动投递滚轮的位置 |
| `started monitor_frame=(w,h) region=… options=…` | 捕获线程打开了显示器会话；`region` 是显示器内的裁剪框 |
| `first frame WxH` | 第一帧到达，选区的物理像素尺寸 |
| `frame=N Shift { dy, votes, second } placement=… rows=… status=… quiet_since_last=… us=…` | 一次被接受的位移。`votes / second` 是最优与次优的票数；`placement` 为 `Appended/Prepended{growth, band}` 或 `Contained`；`rows` 是长图当前高度；`us` 是本帧回读 + 估计 + 拼接的耗时 |
| `frame=N Unknown { best, votes, second, unmatched } …` | 被拒绝的一帧。`unmatched` 大说明发生跳跃或内容被替换；`votes` 与 `second` 接近说明内容重复 |
| `auto=true/false`、`auto ended: N notches moved nothing within 1000 ms (…)` | 自动滚动的开关与结束原因。括号里区分两种"没动"：`the page repainted without moving`（有新帧但没有位移）与 `no new frame arrived`（窗口内根本没有新帧）；被目标拒绝滚轮则单独一行说明 |
| `auto target hwnd=0x… class=… pid=…` | 自动滚动的滚轮实际投递到的窗口；开始时和目标变化时各打印一次 |
| `auto notch failed: …` | 一格滚轮没有投递出去（选区下没有窗口，或窗口已失效），下一格会重新寻找目标 |
| `the long image cannot grow by N bytes past M` | 内存不足，长图停止增长（状态为 `Full`） |
| `desktop copy path=…`、`clipboard copy done ms=…` | F9 后桌面副本与剪贴板写入的结果 |
| `finished reason=f9/enter/button image=WxH` | 结束并导出 |
| `stopped frames=… read_mean_us=… push_mean_us=… read_us=… push_us=…` | 会话收尾的聚合读数,微秒。`read_mean_us`/`push_mean_us` 是 `read_region` 与 `tracker.push` 的均值；后面两个 `read_us`/`push_us` 是相应样本集的 `mean/p50/p95/max`（`push_us` 为 `us − read_region`，即"不在回读里的那一部分"，因此它把 `tracker.push` 与回读之后的一切都算进去） |
| `frame=… read_us=… push_us=… total_us=… bytes=…` | 逐帧拆分，只在设置环境变量 `SNAPCLIP_SCROLL_VERBOSE` 时打印 |
| `failed: …`、`the capture thread ended: …` | 捕获线程出错；面板显示"截图失败，见日志" |

`Still` 帧，以及达到上限后的重复 `Shift`（`placement=Some(Full)`，只有第一条打印），都不逐帧打印，只在下一条打印行里以 `quiet_since_last` 计数。达到上限后画布不再变化，捕获线程也不再刷新预览、唤醒覆盖层。

## 5. 已知风险

- 精确哈希可能被亚像素文本渲染破坏，表现为日志里出现大量 `Unknown` 且 `votes` 偏低。到时按日志证据加入容差，现在不预先引入。
- 有的程序不响应 `PostMessage` 投递的 `WM_MOUSEWHEEL`（snow_shot 用的也是同一方案），表现为 `auto ended: 10 notches moved nothing within 1000 ms (…)`。这只影响自动模式，手动滚动不受影响。对照 `auto target` 行的 `class` 可以判断滚轮投到了哪个窗口。
- 以管理员身份运行的目标窗口会拒绝投递的滚轮（UIPI），自动模式会立即停止并在状态栏说明；手动滚动不受影响。
