# 36 滚动截图拼接算法

> 这份文档讲**算法**：怎么从一串视口帧里量出位移、把帧放到长长的画布上、并保证不重叠、不留缝。
> 存储与性能（流式画布、tile 后备、四类预算）在 `docs/35-scroll-canvas-streaming-plan.md`。
> 两者共享一条不变量：`band ≤ view`（`stitch.rs:146-148`）既是 `docs/35` 里"可改写窗口 = 一个视口"的根据（`backed.rs:3-22`），也是本文"一行的内容只有一个来源"的根据。

---

## 0 一页总结

| 保证 | 由什么机制给出 | 代码位置 |
| --- | --- | --- |
| 位移可测 | 8 个竖条 × 每行一个哈希；只有**在本条内唯一**的哈希算锚点，投票给 `dy = y_ref - y_cur` | `estimate.rs:20,38-50,88-118` |
| 测不出就不猜 | 三态判定：`Shift` / `Still` / `Unknown`；票数低于地板或领先不够 2 倍 ⇒ `Unknown` | `estimate.rs:143-165` |
| **不重复** | ① `growth` 只算**净增行**；② 边缘那条 `band` 是**覆盖**不是追加；③ `Contained` 一行都不写 | `stitch.rs:117,120,131,142` |
| **缓冲区无空隙** | 画布是一段连续缓冲，`append/prepend` 正好加 `growth` 行 | `stitch.rs:117-138` |
| **内容无空隙** | 由 `dy` 的准确性保证；测不出就丢弃该帧（宁可缺一段，不可错位） | `mod.rs:241-246` |
| 固定表头/底栏只出现一次 | 增长时重写最新帧的那 `band` 行，把"上一帧留下的那份"换成真实内容 | `stitch.rs:9-14,120,131` |
| 上限到顶会停 | 行数上限 = `min(MAX_BYTES/行字节, MAX_SCREENS × 视口高)` | `mod.rs:26,32,90,110` |
| 写盘失败不说谎 | `Status::Storage` 与 `Status::Full` 分开 | `mod.rs:216-225` |

---

## 1 数据流

```
帧（BGRA, width × view × 4）
  → FrameSig::of          每 (条, 行) 一个 64-bit 哈希      estimate.rs:38-50
  → estimate(reference, current)                            estimate.rs:76-166
  → Estimate::{Shift{dy} | Still | Unknown}
  → Stitcher::place(frame, dy)                              stitch.rs:110-148
      Appended{growth,band} / Prepended{growth,band} / Contained / Full
  → （流式改造后）离开可改写窗口的行封口成 tile 落临时文件   backed.rs:349-380
```

调度者是 `Tracker::push`（`mod.rs:202-252`）：它拿 `Estimate` 决定是否调用 `place`，并维护 `Status` 与缩略图刷新范围。

---

## 2 位移估计（`estimate.rs`）

### 2.1 帧签名

- 每帧切成 **8 个竖条**（`STRIPS = 8`，`estimate.rs:20`），每条内**每一行**算一个哈希（`estimate.rs:38-50`），哈希函数是 64-bit multiply-xor（`177-190`）。
- **为什么要分条**：固定侧栏（导航条、滚动条）在每一帧都占同样的列，若整行哈希，它会把内容的位移淹掉；按列切条后，有内容的那些条各投各的票（模块注释 `estimate.rs:13-14`；测试 `a_fixed_sidebar_does_not_spoil_the_strips_beside_it`，`288`）。
- 成本 O(W·H)/帧（`15`），有基准测试 `the_cost_of_one_estimate_at_the_field_selection_size`（`441`）。

### 2.2 锚点与投票

1. 在 reference 的**同一竖条**里，只保留**只出现一次**的哈希（重复的标记为 `-1` 并跳过），得到"锚点行号"表（`estimate.rs:88-96,111-113`）；
2. 在 current 的同一竖条里，**重复出现**的哈希整条丢弃（`97-106`）——两侧都要求唯一，否则无法定位；
3. 每个锚点投一票：`dy = y_ref - y_cur`（`115`）；`dy` 的绝对值必须 ≤ `max_shift`（`116`）才计入；
4. 没匹配上的 current 行计入 `unmatched`（`107-110`）——它们是"新露出来的内容"。

**搜索范围**：`max_shift = h - MIN_OVERLAP`，`MIN_OVERLAP = 32`（`estimate.rs:23,79`）⇒ 两帧**至少共享 32 行**，位移才可测。

### 2.3 三态判定

| 结果 | 条件 | 位置 |
| --- | --- | --- |
| `Shift { dy, votes, second }` | `best ≥ floor` **且** `best ≥ 2 × second` | `estimate.rs:144-150` |
| `Still { votes, unmatched }` | `still > 0` **且** `still ≥ 2 × best` **且** `still ≥ unmatched` | `estimate.rs:154-159` |
| `Unknown { best, votes, second, unmatched }` | 其余 | `estimate.rs:160-165` |

其中：
- `floor = max(MIN_VOTES, anchors / 50)`（`MIN_VOTES = 8`，`estimate.rs:26,143`）—— 锚点越多要求越高；
- `second` 是"与最佳位移相差 ≥ 3 行"的次佳票数（`|dy - best| > 2`，`estimate.rs:136-140`）⇒ 允许亚像素抖动造成的相邻行票，但不允许两个**不同答案**势均力敌；
- `Still` 必须额外与 `unmatched` 比较，理由写在 `151-153`：**固定表头/底栏每次都投 0 票**，所以"0 票最多"不等于"没滚动"；当旁边的内容全是新的，那是跳转或换页，不能把它当成新参考。测试 `a_jump_under_a_fixed_header_is_unknown_not_still`（`364`）钉的就是这条。

### 2.4 语义

`dy > 0` 表示内容**上移**（页面向下滚）：current 的第 `y` 行显示的是 reference 第 `y + dy` 行（`estimate.rs:61-63`）。

---

## 3 放置（`stitch.rs:110-148`）

```rust
let cand  = pos + dy;        // 这一帧的内容应落在画布的哪一行
let known = store.rows();    // 画布当前有多少行
```

| 分支 | 条件 | 动作 | 位置 |
| --- | --- | --- | --- |
| `Appended { growth, band }` | `cand + view > known` | `growth = cand + view - known`；在末尾追加 `growth` 行，并把帧的**最后 `band` 行**写到画布最后 `band` 行；`pos = rows - view` | `stitch.rs:116-128` |
| `Prepended { growth, band }` | `cand < 0` | `growth = -cand`；在头部插入 `growth` 行，并把帧的**最前 `band` 行**写到画布最前 `band` 行；`pos = 0`、`top -= growth` | `stitch.rs:129-139` |
| `Contained` | 其余 | 只更新 `pos = cand`，**不写任何像素** | `stitch.rs:140-143` |
| `Full` | store 拒绝（撞行数上限或写盘失败） | 什么都不变，由调用方转成 `Status::Full` / `Status::Storage` | `stitch.rs:32-33`、`mod.rs:216-225` |

- `pos` = 视口顶端对应的画布行（`stitch.rs:5-6,72-75`）；
- `top` = 画布第 0 行的**内容坐标**，每次 prepend 减 `growth`（`stitch.rs:43-44,135`）。

---

## 4 不重叠

四道独立的保险，任何一道单独失效都不至于让一行出现两次：

1. **`growth` 只算净增行**（`stitch.rs:117`）
   `growth = (cand + view) - known` 是"这一帧的底边超出画布多少行"；帧内与画布重叠的部分**根本不会被追加**，所以同一内容不会被写第二遍。
2. **`band` 是覆盖，不是追加**（`stitch.rs:120,131`）
   追加/前插之后，接着**改写**画布边缘那 `band` 行；改写用的是最新帧同一位置的行（`&frame[(view - band) * row..]` / `&frame[..band * row]`）。这是替换，不是叠加。
3. **`Contained` 一行不写**（`stitch.rs:142`）
   测试 `contained_moves_match_the_oracle_and_write_nothing`（`624`）和 `a_move_inside_known_rows_writes_nothing`（`655`）钉住这一点。
4. **`Full` 不动画布**（`stitch.rs:32-33`；store 侧 `a_buffer_that_cannot_grow_is_left_untouched`，`store.rs:338`）。

端的验证：`a_scroll_down_reproduces_the_page_with_one_header_and_one_footer`（`stitch.rs:285`）用**不整齐的偏移序列**（`0,3,40,41,97,…,530`）拼出的长图与源页面**逐字节相等**——若任何一行重复或错位，这条就会红。

---

## 5 无间隙：必须把两种"连续"分开

### 5.1 缓冲区连续（结构性保证 ✓）

画布是一段连续缓冲（`docs/35` 改造后是"前环 + tile 文件 + 后环"，但对外仍是按行号连续的一段）；`append(growth)` 把行紧接在已有行之后，`prepend(growth)` 紧接在之前（`stitch.rs:117-138`）⇒ **行号上不可能出现空洞**。

### 5.2 内容连续（依赖 `dy` 的准确性 ✗）

缓冲区连续 ≠ 内容连续：`dy` 错了，缓冲区依然连续，但内容是**错位或重复**的。代码对此的立场是**宁可缺、不可错**：

- 估计不可靠 ⇒ 该帧**不放置**，`Status::Lost`，**参考帧保留**以便滚回来恢复（`mod.rs:241-246`；`Status::Lost` 的注释 `mod.rs:40-43`；测试 `a_jump_the_estimator_cannot_measure_is_skipped_and_the_session_recovers`，`stitch.rs:311`）；
- 因此"无间隙"的准确表述是：**在被接受的帧之间无间隙**；被丢弃的帧意味着画布上可能出现一段内容缺口（而不是错位）。

### 5.3 行数上限

`row_limit(width, view, MAX_BYTES)`（`mod.rs:90,110`）取 `min(MAX_BYTES / 行字节, MAX_SCREENS × view)`（`MAX_SCREENS = 32`，`MAX_BYTES = 512 MiB`，`mod.rs:26,32`）。到顶后 `place` 返回 `Full`，画布停止增长（测试 `the_canvas_stops_growing_at_its_cap`，`stitch.rs:341`；预算侧 `a_sequence_that_hits_the_row_cap_matches_the_oracle`，`630`）。

---

## 6 `band` 为什么存在

模块注释（`stitch.rs:9-14`）说得很清楚：**固定表头/底栏在每一帧都出现在增长边缘的同一位置**。若只按 `growth` 追加：

- 向下滚时，底部的固定条会被**反复追加** ✗；
- 向上滚时，顶部的固定表头会被反复追加 ✗。

所以增长时**额外重写**边缘一条带：

```rust
band = (view / 2).max(growth + view / 4).min(view)     // stitch.rs:146-148
```

- 下界 `view/2`：至少要能覆盖"固定条 + 一帧的位移"；
- 上界 `view`：不能超过一帧的高度（再大也没有像素来源）；
- `growth + view/4`：位移越大，需要的带越宽；
- **对面那条边从不写** ⇒ 那一侧的固定条只会出现一次 ✓。

这解释了为什么 `band ≤ view`：它不是调参，而是**像素来源的上限**（一帧只有 `view` 行）。`docs/35` 把它直接变成了"可改写窗口 = 一个视口"（`backed.rs:3-22,349-380`）。

---

## 7 失败模式（诚实列全）

| 情形 | 系统的行为 | 位置 / 测试 |
| --- | --- | --- |
| 位移估计不唯一（重复内容：空白行、相同列表行） | `Unknown` ⇒ 丢帧、`Status::Lost`、参考帧保留 | `estimate.rs:88-118`；`324` |
| 单帧跳变超过 `h - MIN_OVERLAP` | `Unknown`（无足够重叠） | `349` |
| 跳变发生在固定表头之下 | `Unknown` 而**不是** `Still`（0 票不代表没动） | `364` |
| 缩放变化（内容被 DPR 改变） | `Unknown`，不当作位移 | `400` |
| 换成了另一个页面 | `Unknown` | `429` |
| 固定表头 / 底栏 | 由 `band` 处理，只出现一次 | `285` |
| 闪烁光标 + 动画 | 仍能测出位移（唯一哈希足够多） | `301` |
| **原处变化**（加载中的图、打字、动画） | **不写像素**：`Contained` 不写（`stitch.rs:142`）、`Still` 只更新参考帧（`mod.rs:232-239`）⇒ 画布里保留的是较早的那份 | 这是有意取舍：否则每帧都要重写整个窗口 |
| 行数到顶 | `Status::Full`，画布停止 | `341`；`630` |
| 临时文件写失败 | `Status::Storage`（与"到顶"分开） | `mod.rs:216-225`；测试 `a_failed_seal_is_reported_as_storage_not_as_the_row_cap`（`mod.rs:495`） |

---

## 8 与流式改造（`docs/35`）的关系

1. **可改写窗口 = 一个视口**：因为一次放置只覆盖增长边缘的 `band ≤ view` 行、且是**纯覆盖**（源是新帧，从不读旧像素），所以离任一边缘超过一个视口的行**永不再变** ⇒ 可以立即封口落盘（`backed.rs:3-22`；`seal_ring` 在 `349-380`；"一块只在其所有行离开可改写范围后才封口"）。
2. **输出顺序必须可变**：向上滚动是 `Prepended`（`stitch.rs:129-139`），等于把新行插到已写内容**前面**；而 PNG 是单条 zlib 流、只能追加 ⇒ 中间层用可重排的 tile 序列（`docs/35 §3.1`）。
3. 一句话：**"一行的内容只有一个来源"**（§4）既是拼接正确性的根据，也是"封口后不可变、所以必须可重排"的根据。

---

## 9 未验证 / 已知弱点

1. **真实网页的匹配成功率未测**：合成页是"每行都不同"的高熵纹理（`estimate.rs:196-205` 的 `page()`），真实页面大量纯色、空白与重复行 ⇒ 锚点更少 ⇒ 更容易落到 `Unknown`（丢帧）✗；
2. **大面积持续变化**（视频、轮播）未测：测试只覆盖"闪烁光标 + 动画不遮挡位移"（`301`）✗；
3. **亚像素滚动**（内容移动不足 1 行）未测：行哈希对不齐 ⇒ 大概率 `Unknown` ⇒ 丢帧 ✗（代码里没有专门处理）；
4. **原处变化不会被修正**（§7 表），这是设计取舍而非缺陷，但没有测试钉住"有意不写"这一语义 ✗；
5. `estimate` 的 `Still`/`Unknown` 阈值（`anchors/50`、`2×`、`≥3 行`）都是经验值，**没有在真实页面上标定过** ✗。

---

## 10 代码与注释/文档不一致之处（本文写作时发现）

| 位置 | 问题 |
| --- | --- |
| `stitch.rs:4-5` | 注释写 "Today the only store is one contiguous BGRA buffer"，**已过时**：`Tracker` 现在用的是 `Stitcher<BackedCanvas>`（`mod.rs:21,134,161`），后端是"前环 + tile 文件 + 后环"（`backed.rs:201-236`） |
| `estimate.rs:3` | 注释指向 `docs/33`（滚动会话设计），本文（`docs/36`）是算法的独立说明 |

其余经核对与代码一致：`band` 公式（`146-148` 与注释 `14` 一致）、三态判定与注释一致、行数上限与 `Status` 语义一致。
