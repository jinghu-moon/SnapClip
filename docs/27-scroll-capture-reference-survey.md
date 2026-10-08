# 27 · 滚动截图参考项目与技术调研（证据附录）

本文是 `docs/25-pixpin-benchmark-and-scroll-capture-review.md` 的**证据附录**。
本文是**只读**调研 `refer/` 下滚动截图/长截图相关项目的产物：全部代码证据均来自实际读取的文件内容，行号可复核，无法验证处明确标注「未验证」；外部结论均附可访问链接。
本文**未修改 `refer/` 下任何文件**。

调研对象：`refer/` 下与滚动截图/长截图相关项目；评审目标：`docs/19-scroll-capture-design.md`。

---

## 0. 证据规则

- **确证** = 我直接读到源码/官方文档原文。
- **强推断** = 由确证事实推得，中间只有一步推理。
- **弱推断** = 社区/二手信息，或缺少直接原文。
- **未验证** = 找不到证据。
- 子代理在本会话不可用（depth 限制），全部工作由我逐文件完成。

---

## 1. 结论摘要（12 条）

| # | 结论 | 强度 |
|---|---|---|
| 1 | Crisp 的拼接核心是「轴向共用搜索核 + band 起始 = 区间 1/3 + 每候选 early-exit + 像素差预算阈值 + 失败即 0」，四者缺一不可；单看阈值或单看 1/3 都不足以复现其行为。 | 确证 |
| 2 | Crisp 的「先规划全部 accepted shift 再分配输出」是 `PlanShifts()` 模板函数（`StitchInternal.h:44-71`），三个停止条件（尺寸不符 / `shift<=0` / 超 `kMaxImageSide`）统一由它裁决，竖/横两个轴共用。 | 确证 |
| 3 | **docs/19 §2.4 称 Crisp `TestStitch.cpp` 含「重复纹理」回归模型 —— 该测试不存在。** 该文件 17 个用例中最接近的只有 `MakeFlat`（纯色）。docs/19 此处属事实错误。 | 确证 |
| 4 | ShareX 的滚动方式只有 4 种输入注入（MouseWheel / DownArrow / PageDown / ScrollMessage），**完全没有 UIAutomation / ScrollPattern / DOM 路径**；且只有单向向下。 | 确证 |
| 5 | ShareX 每个 step 都 `CombineImages` 新建整张 `SKBitmap`（`:356-367`），完成后预览还走 PNG 编码→解码回环（`:239-246`）。这是 docs/19 拒绝「逐帧整图」的正确依据。 | 确证 |
| 6 | snow_shot 的位移估计不是 SAD profile，而是 **ORB 二值特征 + 候选位移聚类 + tile 权重内点评分 + 复合置信度**（`estimator.rs`），并带「低分辨率失败后自动全分辨率重试」（`:1062-1081`）。 | 确证 |
| 7 | snow_shot 用**时序学习的 tile 三分类**（fixed/scrolling/dynamic，EMA）给候选加权，权重钳在 0.1–2.0（`region.rs:779-788`）：**降权而非剔除**，所以「全部带都固定」不会硬失败。 | 确证 |
| 8 | snow_shot 对**匹配失败不停止采集**：`NoMotion/Indeterminate → StitchBranch::NoMovement`，画布不增长、`previous_raw` 前移、继续下一帧（`stitcher.rs:356-390`）。另有 `Contained` 与 `Prepend`（真双向）。docs/19 §7.2「搜索无结果即停止」会被一帧动画误杀。 | 确证 |
| 9 | docs/19 §6.2 排除 `PostMessage` 与两份反证冲突：snow_shot 生产代码正是 `PostMessageW` + 子 HWND 下沉（`scrollinput.cpp:52-54`）；Microsoft Learn 原文写 `post-message` "**bypasses UIPI (works across integrity levels)**"，而 `send-input` 被 UIPI 拦。 | 确证 + 未解疑点 |
| 10 | 「右侧采集期实时预览」只有 snow_shot 与 PixPin 提供；ShareX 只在完成后预览整图，Crisp 没有。docs/19 §5.6 判断正确。 | 确证 |
| 11 | 「不把最接近当成功」并不统一：ShareX 有 `bestGuess → PartiallySuccessful`（黄灯），官网明确红灯=前两帧无法拼接即自动停止；PixPin 则把「反向自动裁剪」做成交互能力。 | 确证 |
| 12 | 尺寸上限有外部量化数据：Crisp `kMaxImageSide = 32767`；PixPin 文档写超长模式「最大 200 万像素长度」，并警告**高度接近 100 万像素可能无法导出、约 75 万像素起普通看图软件可能打不开**。**限定**：30,000 px 作为**默认上限**在可打开性上是保守安全的，但作为**架构上限**低于 PixPin 的实测量级（50 万 px）；正确做法是架构支持到 50 万量级、默认上限由产品与实测决定。 | 确证（PixPin/Crisp）+ 强推断（阈值适配） |

> **结论 #12 的分歧记录（供 docs/25 双向呈现）**
>
> 本报告原始判断为「docs/19 §8.4 的 30,000 px / 150 MP 量级合理」，依据是 PixPin 官方文档给出的**可打开性红线**（约 75 万 px 起普通看图软件可能打不开、接近 100 万 px 可能无法导出）。该判断针对的是「**默认保护阈值**在用户端是否安全」，不是「架构不能被钉死在 30,000 px」。
>
> 评审方（docs/25）的对标证据是：**PixPin 实际产物达 `1058 × 502,649 px`**（官方配图的 Windows 文件属性对话框中确认），且把「超长截图模式」（>29,000 px 触发）作为正式卖点；因此「30,000 px 作为默认硬上限」会让 SnapClip 在竞品的主打能力上明显落后——30,000 px 在 PixPin 里只是「平常长度」，不是「超长」。
>
> 评审方立场（我认同并已并入上文限定语）：不是照抄 50 万，而是**架构必须能支撑到 50 万 px 量级**（tile 化 + 流式写盘），而**默认上限必须是可注入的策略值、由数据冻结**，不能在纯内存画布阶段就被 30,000 钉死（参见 `docs/24` S1.8）。
>
> **两方并不矛盾的地方（重要）**：PixPin 文档的 2,000,000 px 上限与「75 万 / 100 万」两条警告阈值，**完全容纳** `502,649 px` 这一实测产物——50 万 px 位于警告线以下的安全区。因此分歧点只有一个：**默认阈值 vs 架构上限**，而非「PixPin 到底能出多长」。我在此把原判断保留为「默认上限的保守性论证」，并接受「架构上限不得低于 50 万 px 量级」这一补充约束。

---

## 2. A1 · Crisp（C++/Qt/Win32，含滚动截图）

### 2.1 能力清单

| 能力 | 实现 | 证据 |
|---|---|---|
| 滚轮注入 | `SendInput` + `MOUSEEVENTF_WHEEL`（垂直）/ `MOUSEEVENTF_HWHEEL`（水平） | `ScrollCapture.cpp:43-60` |
| 等待期间泵消息 | 自研 `SleepPumping` | `ScrollCapture.cpp:24-41` |
| 光标停放/还原 | RAII `CursorParking` | `ScrollCapture.cpp:90-110` |
| 轴自动探测 | Auto：先竖后横，取「动了的那个」 | `ScrollCapture.cpp:146-161` |
| 拼接 | 轴向共用核 + 竖/横两套装配 | `Stitch.cpp:51-117`、`StitchVertical.cpp`、`StitchHorizontal.cpp` |
| sticky header/footer | 逐行严格相等 + 高度 1/3 上限 | `StitchVertical.cpp:28-61` |
| 失败语义 | 阈值不过 → 返回 0 → 停止 | `Stitch.cpp:113-115`、`StitchInternal.h:57-59` |
| 测试 | 17 个纯合成用例 | `tests/TestStitch.cpp` |

### 2.2 滚轮注入（问题 1）

`ScrollCapture.cpp:43-60`：
```cpp
input.type = INPUT_MOUSE;
if (direction == ScrollDirection::Horizontal) {
    input.mi.dwFlags = MOUSEEVENTF_HWHEEL;
    input.mi.mouseData = static_cast<DWORD>(WHEEL_DELTA * notches);
} else {
    input.mi.dwFlags = MOUSEEVENTF_WHEEL;
    input.mi.mouseData = static_cast<DWORD>(-WHEEL_DELTA * notches);
}
::SendInput(1, &input, sizeof(input));
```
- `mouseData = ±WHEEL_DELTA × notches`（±120×n）；水平正 = 右；明确拒绝 Shift+滚轮替代（`:47-50`）。
- **不发 `PostMessage`**：`ScrollCapture.h:13-16` 注释「浏览器和 Electron 窗口会忽略 `WM_MOUSEWHEEL`」。

步长/等待（`ScrollCapture.h:48,52,57,60`）：`maxFrames = 30`、`notchesPerStep = 3`、`settleMs = 260`（固定）、`direction = Auto`。

消息泵（`ScrollCapture.cpp:24-41`）：`GetTickCount64` 截止时间 + `PeekMessage/TranslateMessage/DispatchMessage` + `MsgWaitForMultipleObjects(..., QS_ALLINPUT)`；用 64 位 tick 避免回绕（注释 `:25-26`）。

光标停放（`:90-110,127,132`）：滚轮发给**光标下**窗口，故必须把光标移到选区中心并在析构时还原；移入后额外 `SleepPumping(120)` 等 hover 高亮稳定。每帧 `CaptureRect(region, frame, false)`——**不画光标**（`:64-65`）。

### 2.3 拼接算法（问题 2）

```cpp
// Stitch.cpp:52-117（节选）
const int bandStart = begin + length / 3;                       // :75
const int overlap  = (std::max)(1, (std::min)(minOverlap, end - bandStart));  // :76
const int maxShift = end - bandStart - overlap;                 // :81
for (int shift = 1; shift <= maxShift; ++shift) {                // :91
    const uint64_t score = BandDifference(previous, bandStart + shift, next,
                                          bandStart, overlap, bestScore, difference);  // :94-96
    if (score < bestScore) { bestScore = score; best = shift; }
}
const uint64_t budget = (uint64_t)breadth * (uint64_t)overlap * 12u;  // :111-112
if (bestScore > budget) return 0;                                // :113-115
```
1. **搜索空间**：`shift ∈ [1, maxShift]`，`maxShift = end - bandStart - overlap`；200 行帧 + overlap=40 → 最大 94 px（`TestStitch.cpp:54-57`）。
2. **band 起始**：区间的 **1/3 处**。理由（`Stitch.cpp:61-74` 注释）：顶部可能有标题栏/工具栏/吸顶 header，从顶端取带只能匹配 shift=0，而 shift=0 被排除 → 什么都抓不到。`FindVerticalShift(header, footer)` 再把已知 header/footer 排除，两层叠加保证 band 不落入固定区。
3. **度量**：`RowDifference` = 逐像素三通道绝对差之和（alpha 忽略）（`Stitch.cpp:16-25,127-145`）。未归一化 L1。
4. **early-exit**：`BandDifference` 超过 `giveUpAt`（当前 best）立即返回（`:32-44`）——branch-and-bound。注释说省 10 倍（`:27-31`）。
5. **阈值**：`budget = 帧宽 × overlap × 12`，即**每像素平均 12 单位**，为 JPEG 噪声/抗锯齿留余量（`:103-110`）。
6. **最大边长**：`kMaxImageSide = 32767`（`Capture.h:23`），`Image::Create` 拒绝（`Capture.cpp:119-121`），`PlanShifts` 提前停止（`StitchInternal.h:63-65`）。
7. **尺寸**：`SameSize()` 宽高都需相等（`Stitch.cpp:119-123,176-178`）。

### 2.4 sticky header/footer（问题 3）

```cpp
// StitchVertical.cpp:42-60（节选）
const int limit = height / 3;
int sticky = 0;
while (sticky < limit) {
    const int row = fromBottom ? height - 1 - sticky : sticky;
    bool same = true;
    for (size_t i = 1; i < frames.size() && same; ++i)
        same = RowDifference(first, row, frames[i], row) == 0;
    if (!same) break;
    ++sticky;
}
```
- **严格逐像素相等**（`== 0`），非相似度；**与 first 比较并遍历所有帧**，任一帧不同即终止 run（`:24-27`）；**上限帧高 1/3**，与 band 起始同源（`:42-46`）。
- 「只复制一次」的两条路径：**header 靠结构证明**（`:119-132` 注释：最大 shift ≤ (contentEnd−header)×2/3，`contentEnd - shift` 永远在 header 之下，中间帧不可能复制到 header）；**footer 从最后使用的帧一次性追加**：
```cpp
const Image& last = frames[used - 1];
for (int row = contentEnd; row < height; ++row)
    CopyRow(out, writtenTo + row - contentEnd, last, row);   // :148-151
```
- 总长不变 = `height + Σshift`；中间帧只复制新增的底部 `shift` 行（`:133-141`）。

### 2.5 先规划、后分配（问题 4）

```cpp
// StitchInternal.h:44-71（节选）
size_t used = 1;
for (size_t i = 1; i < frames.size(); ++i) {
    if (!frames[i].Valid() || frames[i].Width() != width ||
        frames[i].Height() != height) break;
    const int shift = find(frames[i - 1], frames[i]);
    if (shift <= 0) break;                    // :57-59 ← 失败即停止
    if (total + shift > kMaxImageSide) break; // :63-65 ← 边长预算
    shifts.push_back(shift); total += shift; ++used;
}
```
调用点 `StitchVertical.cpp:106-111`、`StitchHorizontal.cpp:44-49`；`used` 经 `stopped` 出参供 `AppActions.cpp:382-384` 决定是否提示「部分」。

### 2.6 失败/拒绝语义（问题 5）

- **停止，不取最接近**：`Stitch.cpp:113-115` → 0 → `PlanShifts` `break`。理由（`Stitch.h:92-95`）：「错误位置上的整条比缺一条更糟——前者静默产出错误图像」。
- **但停止后仍交付部分**：`AppActions.cpp:365-377` 只在 `used <= 1` 报错；`used < frames.size()` 仅写日志后照常交付（`:382-388`）。即 Crisp 的「部分成功」是**静默的**——这正是 docs/19 要求显式 Partial 的合理动机。

### 2.7 `TestStitch.cpp` 测试模型清单（问题 6）

| 用例 | 行 | 断言方式 |
|---|---|---|
| 已知位移可还原 | 53 | `{1,5,40,94}` → 等于；`{95,150,199}` → 0 |
| 顶部固定 header | 74 | 前 90 行覆写相同内容，仍还原 45 |
| 相同帧不产生位移 | 103 | 0 |
| 无关帧不编造 | 113 | 纹理 vs 纯色 → 0 |
| 尺寸不一致 | 124 | 宽/高不同 → 0 |
| 5 帧拼长图 | 136 | 高度 = `h+shift*4`，**逐行** `RowDifference==0` |
| 失败时停止并上报 | 169 | `used==2`，高度只 +40 |
| 单帧 = 自身 | 193 | — |
| 空列表失败 | 205 | — |
| 行差自身 0 / 越界 UINT64_MAX | 211 | 越界不返回 0 |
| 水平已知位移 | 273 | 同垂直镜像 |
| 水平无关帧 / 轴不混 | 292 | 竖向纹理在水平不算位移 |
| 4 帧拼宽图 | 309 | 逐列 `ColumnDifference==0` |
| sticky 检测与 1/3 上限 | 349 | 30 行→30；一帧改 1px→9；1 帧→0；150 行→截到 100 |
| **高 footer 使搜索失效** | 378 | 3 参数→0；带 footer 5 参数→80 |
| footer 只出现一次且在末尾 | 394 | 并对比 `trimStickyFooter=false` 旧行为 |
| header 只复制一次 | 439 | header 段 = frames[0]，其后逐行 = 整页 |

**关键更正**：**没有重复纹理用例**。docs/19 §2.4 列「重复纹理」属误记。

### 2.8 UI/交互（问题 7）

- 区域选择**复用普通 overlay**（`AppActions.cpp:295-302`，`showActionBar=false`）。
- **没有可点击穿透边框**：选完即销毁 overlay，`::Sleep(450)` 等桌面重绘（`:308-319`，注释说 150ms 不够会拍到暗化残留）。
- 进度提示是 toast（`:321-325` + `Toast.cpp:368` 的 `WS_EX_LAYERED|TOOLWINDOW|TOPMOST|NOACTIVATE`）。
- **无实时预览、无暂停/取消**：UI 线程阻塞循环，只有 `BusyScope m_busy` 防重入（`:292`）。
- **完成后无预览**，直接 `DeliverCapture`。

### 2.9 不应照搬

| 项 | 证据 | 原因 |
|---|---|---|
| GDI/BitBlt 全屏 CPU 管线 | `Capture.cpp:46-50,79-80` `BitBlt(..., SRCCOPY \| CAPTUREBLT)` + `:91-100` CPU 补 alpha | 每帧全屏 blit + O(W×H) 遍历；无 HDR；不能只抓窗口 |
| 全部帧驻留内存 | `ScrollCapture.cpp:115` `std::vector<Image> frames`，30 帧 32bpp DIB | 30×1080p ≈ 250 MB |
| 固定阈值 | `Stitch.cpp:112` 硬编码 `12u` | 未随内容/压缩噪声自适应 |
| 固定 settleMs/notches | `ScrollCapture.h:52,57` | 不区分应用与平滑滚动时长 |
| 失败即整体停止 | `StitchInternal.h:57-59` | 一帧动画毁掉 100 步会话 |
| UI 线程阻塞、无取消 | `AppActions.cpp:292,349-350` | 十秒级不可中断 |
| 部分成功静默交付 | `:382-384` 只写日志 | 用户不知道图是否完整 |

---

## 3. A2 · ShareX（C#，ShareX.ScreenCaptureLib）

### 3.1 能力清单

文件：`ScrollingCaptureManager.cs`(387)、`ScrollingCaptureOptions.cs`(39)、`ScrollingCaptureService.cs`(53)、`Presentation/ScrollingCapture/ScrollingCaptureWindow.axaml.cs`(398)、`ScrollingCaptureRegionWindow.axaml.cs`(154)、`Enums.cs`(244-257)。

### 3.2 截图/滚动方法（问题 1、2）—— 只有输入注入，无 UIA

```csharp
public enum ScrollMethod { MouseWheel, DownArrow, PageDown, ScrollMessage }   // Enums.cs:251-257
```
```csharp
// ScrollingCaptureManager.cs:130-150
case ScrollMethod.MouseWheel: InputHelpers.SendMouseWheel(-120 * Options.ScrollAmount); break;
case ScrollMethod.DownArrow:  for (...) InputHelpers.SendKeyPress(VirtualKeyCode.DOWN); break;
case ScrollMethod.PageDown:   InputHelpers.SendKeyPress(VirtualKeyCode.NEXT); break;
case ScrollMethod.ScrollMessage:
    for (...) NativeMethods.SendMessage(selectedWindow.Handle, VSCROLL, SB_LINEDOWN, 0); break;
```
- `SendMouseWheel` → `InputManager` + `MOUSEEVENTF_WHEEL`（`InputHelpers.cs:112-117`、`InputManager.cs:202`）。
- **没有失败回退链**：`ScrollMethod` 是用户手选裸 switch，无探测/验证。
- **完全没有 UIAutomation**：全目录 grep `UIAutomation|AutomationElement|ScrollPattern|FlaUI|IUIAutomation` → **0 命中**（确证）。
- **没有横向**（无 `HWHEEL`）；**不是双向**：`AutoScrollTop` 只是开始前 `SendKeyPress(HOME)` + `SendMessage(WM_VSCROLL, SB_TOP)` 拉到顶（`:108-114`）。

### 3.3 配置项（问题 3）

```csharp
// ScrollingCaptureOptions.cs:30-37（全部）
StartDelay=300; AutoScrollTop=false; ScrollDelay=300;
ScrollMethod=MouseWheel; ScrollAmount=2;
AutoIgnoreBottomEdge=true; AutoUpload=false; ShowRegion=true;
```
- **没有** `MaximumScrollCount`（全仓库 grep 无命中）→ 无限流页面只能靠用户 stop 或内存耗尽。
- **没有** `AutoIgnoreLeftEdge/RightEdge`。
- 消费点：`ScrollDelay` `:106,113,185-190`（`ScrollDelay - elapsed` 补偿式节流）；`AutoIgnoreBottomEdge` `:287`；`ShowRegion` `:96-100`。

### 3.4 动态底部排除（问题 4）

```csharp
// :284-304（节选）
int ignoreBottomOffsetMax = currentImage.Height / 3;
int ignoreBottomOffset = Math.Max(50, currentImage.Height / 10);
if (Options.AutoIgnoreBottomEdge) {
    for (int i = 0; i <= ignoreBottomOffsetMax; i++) {
        if (CompareRows(resultScan0Last - i*stride, currentImageScan0Last - i*stride, compareLength) != 0) {
            ignoreBottomOffset += i; break; } }
    ignoreBottomOffset = Math.Max(ignoreBottomOffset, bestIgnoreBottomOffset);  // 跨帧记忆
}
```
左右也有固定排除 `ignoreSideOffset = clamp(max(50,W/20), .., W/3)`（`:271-272`），但**不自适应**。

### 3.5 拼接与状态机（问题 5、6）

```csharp
// :267-329（节选）
int matchLimit = currentImage.Height / 2;
for (int currentImageY = H-1; currentImageY >= 0 && matchCount < matchLimit; currentImageY--) {
    int currentMatchCount = 0;
    for (int y = 0; currentImageY-y >= 0 && currentMatchCount < matchLimit; y++) {
        if (CompareRows(resultScan0 + ((rectBottom-y)*stride),
                        currentImageScan0 + ((currentImageY-y)*stride), compareLength) == 0)
            currentMatchCount++; else break; }
    if (currentMatchCount > matchCount) { matchCount = currentMatchCount; matchIndex = currentImageY; }
}
```
- `CompareRows` = 整行字节**完全相等**（`SequenceEqual`，`:255-256`）——二值判据，非 SAD，无归一化、无 margin。
- `bestGuess`：
```csharp
// :333-341
if (matchCount == 0 && bestMatchCount > 0) {
    matchCount = bestMatchCount; matchIndex = bestMatchIndex;
    ignoreBottomOffset = bestIgnoreBottomOffset; bestGuess = true; }
// :369-376
if (bestGuess) status = ScrollingCaptureStatus.PartiallySuccessful;
else if (status != ScrollingCaptureStatus.PartiallySuccessful) status = ScrollingCaptureStatus.Successful;
// :382-384
status = ScrollingCaptureStatus.Failed; return null;
```
→ **本步完全失败时回退历史最佳 offset 继续拼**，降级为黄灯。
- 枚举 `Failed / PartiallySuccessful / Successful`（`Enums.cs:244-249`）。
- 结束：`CompareLastTwoImages()`（整图逐像素 `SequenceEqual`，`SkiaPixelBuffer.cs:106-113`）为真即 break（`:125-128`）。`IsScrollReachedBottom`（`GetScrollInfo` 比较 `nMax == nTrackPos + nPage - 1`，`:226-238`）**在 `StartCapture` 中从未被调用**（死代码，确证）。
- 官网语义：Green=成功；Yellow=部分成功（"used the best available match"）；**Red=前两张图无法拼接，滚动截图被自动停止**。

### 3.6 整图物化（问题 6）

```csharp
// :356-367
Bitmap newResult = SkiaImageHelpers.CreateBitmap(result.Width, result.Height - ignoreBottomOffset + matchHeight);
g.DrawImage(result, ...);  g.DrawImage(currentImage, ...);
```
每一步新建整张 `SKBitmap` 并两次 DrawImage（O(N) 分配 + O(N) 拷贝），叠加 `Task.Run` 在同一 loop（`:250-253`）。**没有任何输出尺寸上限**。

### 3.7 UI/交互

**区域边框（可点击穿透）** —— 最有价值的一段：
```csharp
// ScrollingCaptureRegionWindow.axaml.cs:41-42,92-102
private const int BorderPixels = 1;  private const int RegionDiff = 4;
info.ExStyle |= WindowStyles.WS_EX_TRANSPARENT | WindowStyles.WS_EX_TOOLWINDOW;
// :104-133
IntPtr frameRegion = CreateRectRgn(0, 0, _frameWidth, _frameHeight);
IntPtr apertureRegion = CreateRectRgn(BorderPixels, BorderPixels, _frameWidth-BorderPixels, _frameHeight-BorderPixels);
CombineRgn(frameRegion, frameRegion, apertureRegion, RegionDiff);   // frame − aperture = 1px 边框
SetWindowRgn(handle, frameRegion, true);
```
DPI：`Width = _frameWidth / _windowScaling`，`_windowScaling` 取自 `Screens.ScreenFromPoint(Position)?.Scaling`（`:80-90`）；`ApplyNativeFrameRegion` 派发两次（`Opened` + `DispatcherPriority.Loaded`，`:77`）。

**暂停/取消**：按钮开始/停止二态；窗口 `Activated` 或 `Closing` 时自动 `StopCapture()`（`:103-120`）——「用户切回来 = 停」。

**完成后预览**：`bitmap.Save(stream, Png)` → `new AvaloniaBitmap(stream)`，即 **PNG 编解码回环**（`:239-246`）；拖拽平移改 `PreviewScrollViewer.Offset`（`:351-397`）。**无采集期实时预览**。

### 3.8 可吸收 / 不可照搬

**可吸收**：1) `SetWindowRgn(全框−空洞) + WS_EX_TRANSPARENT + TOOLWINDOW` 三件套；2) `bestGuess` 降级 + 三色状态；3) `ignoreBottomOffset = max(基线, max(50,H/10)) + 首个不等行` 跨帧记忆；4) 完成后可平移查看器；5) `Activated → StopCapture()`；6) 滚动方法枚举作为降级候选清单。

**不可照搬**：逐帧整图重建；PNG 编解码回环；`SequenceEqual` 二值比较；无 margin/无上限；无 UIA/无回退/无横向/非双向；`WM_VSCROLL/SB_LINEDOWN` 直发窗口。

---

## 4. A3 · snow_shot（C++/Qt + Rust 核心）

三个参考项目里工程成熟度最高：位移估计是独立 Rust crate（`estimator.rs` 78 KB、`orb.rs` 67 KB、`tiled_canvas.rs` 50 KB），Qt 侧只做 UI/编排。

### 4.1 缩略图与 tile（问题 1、2、3）

```cpp
// screenshotscrollingthumbnailwidget.cpp:23-24
constexpr int kThumbnailExtent = 128;     // 交叉轴固定 128 px
constexpr int kPreviewTileSpan  = 256;    // 沿轴 tile span
```
- 交叉轴**必须精确等于 128** 才接受（`:283,372,413`）。
- tile 模型（`.h:81-86`）：`struct PreviewTile { QImage image; int firstSpan; int spanCount; qint64 firstPosition; };` + `std::deque<PreviewTile>`。append 填满 256 再开新 tile（`:384-408`）；prepend 从 `firstSpan` 往前填（`:411-449`）——**同一结构同时支持双向**。
- `discardPreviewBack/Front(span)` 按 span 弹出/缩短 tile（`:309-339`）；`compactActiveTile()` 在切换方向时归一化半满 tile（`:349-368`）。
- 渲染只遍历可见 tile：`drawPreviewTiles` 用 `std::lower_bound` 按 `firstPosition + (visibleStart-targetStart)/tileScale` 定位（`:451-469`）。

### 4.2 overlap 替换行修正缩放取整漂移（问题 3）

```cpp
// screenshotscrollingpipeline.cpp:370-382
// Refresh the splice overlap and absorb all scale rounding into this
// small edge patch so the retained preview tiles never drift in height.
const int overlapSourceRows = std::max(0, deltaRows - std::max(0, addedRows));
int replacedRows = 0;
if (overlapSourceRows > 0) {
    replacedRows = std::clamp(qRound((qreal)overlapSourceRows * scale), 1, m_emittedPreviewHeight);
}
const int patchHeight = targetHeight - (m_emittedPreviewHeight - replacedRows);
if (patchHeight <= 0) return {};
return {targetHeight, patchHeight, replacedRows, false, true};
```
`scale = 128 / outputCrossExtent`（`:355-356`）。**把全部缩放取整误差集中到重叠区那一小块边缘 patch 上替换掉**，旧 tile 高度永不漂移。Qt 侧按 `replacedPreviewRows` 先 discard 再 append/prepend（`thumbnailwidget.cpp:186-214`）。

### 4.3 局部脏区绘制（问题 4）

```cpp
// thumbnailwidget.cpp:805-817
if (m_hoverPreviewRect != preview) {
    const QRectF dirty = m_hoverPreviewRect.united(preview).adjusted(-1.0, -1.0, 1.0, 1.0);
    m_hoverPreviewRect = preview;
    update(dirty.toAlignedRect());
}
```
**只有 hover 框走脏区**；其余路径是整 widget `update()`（`:87,116,271,634,885,929`）。→ 「每个 patch 只使新增像素失效」在 snow_shot **没有直接对应实现**。

### 4.4 hover 请求单飞 / epoch / revision（问题 5）

```cpp
// scrollinghoverpreview.h:93-121（节选）
if (!m_paused || !m_ready || m_pending || m_desired.isEmpty() || m_desired == m_completed) return;
const quint64 epoch = m_epoch, contentRevision = m_contentRevision, serial = ++m_requestSerial;
m_pending = true;
m_context.request(rect, [receiver, rect, epoch, contentRevision, serial](QImage image) {
    if (!receiver || serial != receiver->m_requestSerial) return;
    receiver->m_pending = false;
    if (receiver->m_paused && receiver->m_ready && epoch == receiver->m_epoch &&
        contentRevision == receiver->m_contentRevision && rect == receiver->m_desired) {
        receiver->m_completed = rect;
        image.isNull() ? receiver->m_context.clear() : receiver->m_context.present(image, receiver->m_cropping);
    }
    receiver->dispatch();
});
```
**单飞 + 四重校验（serial/epoch/contentRevision/rect）**；且**取预览前必须先暂停采帧**（`reconcile()` 的 `m_context.pause(...)`，`:64-91`），pause 一路传导到 controller 的 `hoverPaused`（`controller.cpp:642`）。会话销毁时 `reset()` 递增 serial 与 revision，在途回调永不再 present（`:47-57`）。

### 4.5 捕获 worker 与 UI 解耦（问题 6）

```cpp
// screenshotscrollingnativesource.cpp:114-139（节选）
config.target_fps = 30; config.min_fps = 1; config.buffer_depth = 3;
config.max_consecutive_errors = 30; config.capture_retry_count = 1;
config.wgc_update_mode = SNOW_CAPTURE_WGC_UPDATE_MODE_COMPLETE_ONLY;
// Auto tries DXGI first, then WGC and GDI on eligible capture failures.
config.capture_backend = SNOW_CAPTURE_BACKEND_AUTO;
config.adaptive_fps = 1; config.include_cursor = 0;
config.exclusions.windows = excludedWindowIds.constData();
```
- 后端 Auto = **DXGI → WGC → GDI**（`SCROLLING_DIAGNOSTICS.md:36`：0 Auto/1 DXGI/2 WGC/3 GDI）；帧带 `is_duplicate`（`:50`）。
- 信箱解耦 `LatestBridgeMailbox<OwnedScrollFrame, quint64>`（`pipeline.cpp:98-99`），容量不足 drop 并计数（`:593-616`）。
- `pause/resume` 递增 `controlRevision`，worker 校验失配即丢弃（`:864-935`）；resume 重建 source。
- **多源暂停汇聚**（`controller.cpp:635-672`）：`paused = exportPaused || movement.active() || hoverPaused` → 单 `capturePaused` + `pauseTransition` 世代号；暂停时先 `autoScroller.setPaused(true)`，恢复走 `singleShot(0)`。

### 4.6 cadence 控制（问题 7）—— docs/19 §6.6 的原型

```cpp
// adaptivescrollingcapturecadence.h:9-20
int minimumFps = 1; int maximumFps = 30; int initialFps = 30;
double capacityHeadroom = 1.25; double ewmaSampleWeight = 0.25;
int recoverySamples = 4; std::uint32_t pressureQueueDepth = 2;
// :55-64 队列压力 → 立即降速
m_targetFps = std::max((double)minimumFps, std::floor(std::min(m_targetFps * 0.75, sustainableFps())));
// :148-168 恢复需连续健康样本，一次只 +1 fps
++m_recoverySampleCount;
if (m_recoverySampleCount < m_config.recoverySamples) return;
m_recoverySampleCount = 0;
m_targetFps = std::min(sustainable, m_targetFps + 1.0);
```
EWMA `ewma = ewma*(1-w) + latest*w`（`:143-145`），**同时保留 latest 与 EWMA 取 max** 作为阶段成本（`:79-80,96-98`）以防尖峰被平滑。压力信号接在 `pipeline.cpp:511`（另一处 `:518` 强制降到最低档）。有独立单测 `tests/scrolling_capture_cadence_tests.cpp`。

### 4.7 滚轮注入

```cpp
// src/platform/windows/scrollinput.cpp:16-55（节选）
constexpr LONG_PTR passThroughStyles = WS_EX_LAYERED | WS_EX_TRANSPARENT;
if (processId != GetCurrentProcessId() && IsWindowVisible(window) && IsWindowEnabled(window) &&
    (styles & passThroughStyles) != passThroughStyles &&
    GetWindowRect(window, &bounds) && PtInRect(&bounds, screenPoint)) { target = window; break; }
...
const HWND child = ChildWindowFromPointEx(target, clientPoint, CWP_SKIPINVISIBLE|CWP_SKIPDISABLED|CWP_SKIPTRANSPARENT);
...
if (!PostMessageW(target, horizontal ? WM_MOUSEHWHEEL : WM_MOUSEWHEEL,
                  MAKEWPARAM(0, (WORD)delta), MAKELPARAM(center.x(), center.y())))
    return {PostFailed, GetLastError()};
```
1. **`PostMessage` 而非 `SendInput`，完全不移动用户光标**。
2. 从 z 序顶找第一个他人窗口，跳过自己进程与「LAYERED|TRANSPARENT 全占」的窗口（注释解释为何两个样式都要具备）。
3. **`ScreenToClient` + `ChildWindowFromPointEx` 下沉到子 HWND**（Chromium 的 render widget host 就是子窗口）。这与 Crisp「浏览器忽略 PostMessage」**直接矛盾**。
4. 失败分类完整（`SCROLLING_DIAGNOSTICS.md:65-72`）：0 Posted（不代表应用处理）/1 InvalidRequest/2 TargetNotFound/3 CoordinateFailure/4 PostFailed(access denied)/5 Unsupported —— **可直接作为 docs/19 `InputRejection` 的枚举蓝本**。
5. 增量固定 ±120（`scrollingstepinput.h:6-16`）。

**Microsoft Learn 原文**（我抓取了全文）：`post-message` "is HWND-targeted and **bypasses UIPI (works across integrity levels)**"，限制是「无法触发 `WH_KEYBOARD_LL`」「`GetAsyncKeyState` 看不到修饰键」「WinUI3/UWP 无窗口控件收不到」；`send-input` "goes to whatever window is foreground and **is blocked by UIPI**"，且需要 unlocked interactive desktop。

### 4.8 排除自身窗口

```cpp
// windowchrome.cpp:241,255-256
const DWORD affinity = excluded ? WDA_EXCLUDEFROMCAPTURE : WDA_NONE;
return SetWindowDisplayAffinity(hwnd, affinity) != 0;
```
独立佐证：PowerToys `WindowCaptureExclusionHelper.cs:23-27` 用同一 API（带 `OSVersion >= 10.0.19041` 判断 + 每会话只告警一次）。

input hole：
```cpp
// screenshotoverlaywindow.cpp:887-910
if (hole.isEmpty() || (m_screenshotRenderer && m_screenshotRenderer->hasScrollingResultPreview()))
    visibleRegion = {};
else
    visibleRegion = QRegion(rect()).subtracted(QRegion(hole));
if (m_windowMaskInitialized && visibleRegion == m_appliedWindowMask) return;   // 幂等
...
visibleRegion.isEmpty() ? clearMask() : setMask(visibleRegion);
```
诊断会记 `full_hole`、`mask_empty`、`native_hole_contains_center`、DPR、`input_target`（`SCROLLING_DIAGNOSTICS.md:19-20`）——现成的验收断言清单。

### 4.9 Rust 核心：位移估计（对 docs/19 §7 最有价值）

```rust
// estimator.rs:12-20
const LOWE_RATIO: f32 = 0.8;  const MAX_HAMMING_DISTANCE: f32 = 64.0;
const MAX_CROSS_AXIS_DELTA: i32 = 4;  const INLIER_TOLERANCE: i32 = 2;
const MAX_CANDIDATES: usize = 8;  const MAX_FEATURES_PER_TILE: usize = 8;
const MIN_INLIER_MATCHES: u32 = 8;  const MIN_INLIER_TILES: u32 = 4;
const MIN_RESIDUAL_GAIN: f32 = 0.15;
// types.rs:178-188
tile_size: 32, max_features: 2_500, max_motion_ratio: 0.6,
min_confidence: 0.65, temporal_learning_rate: 0.2,
```
流程（`estimator.rs:993-1083`）：几何校验 → **可见内部像素全等则直接 `IdenticalInterior`+`NoMotion`**（`:1021-1027`）→ 三张灰度 + 两张相似度图 → ORB 证据 → `evaluate()` → **降采样失败自动全分辨率重跑**：
```rust
// :1062-1081
let mut evaluated = self.evaluate(&images, evidence);
if self.sampling.reduced() && evaluated.needs_full_resolution() {
    let pyramid = self.full_pyramid_plan.get_or_init(|| PyramidPlan::new(w, h));
    let evidence = pure_rust_feature_evidence(..., pyramid, None);
    evaluated = self.evaluate(&images, evidence);
}
```
`needs_full_resolution()`（`:85-98`）：无补偿图 / margin ≤ 0 / 首选候选的 `precise_alignment_error` 不 < 1.0 px。

打分与接受：
```rust
// :1128-1130, 1148
let maximum_shift = (primary_extent as f32 * max_motion_ratio).floor() ...;
let candidates = candidate_offsets(&observations, self.axis, maximum_shift);
// :1179-1191 排序 & tie-break
scored.sort_by(|l, r| r.diagnostics.score.total_cmp(&l.diagnostics.score)
    .then_with(|| l.diagnostics.offset.abs().cmp(&r.diagnostics.offset.abs()))
    .then_with(|| l.diagnostics.offset.cmp(&r.diagnostics.offset)));
// :1233-1241 用 INLIER_TOLERANCE 之外的次优算 margin
let second_score = scored.iter().skip(1)
    .find(|c| (c.diagnostics.offset - scored[0].diagnostics.offset).abs() > INLIER_TOLERANCE)
    .map(|c| c.diagnostics.score).unwrap_or(0.0);
let margin = ((best_score - second_score)/best_score).clamp(0.0,1.0);
// :1248-1252 复合置信度
let confidence = (0.40 * best.weighted_inlier_share + 0.25 * best.spatial_coverage
                + 0.20 * best.residual_gain + 0.15 * margin).clamp(0.0, 1.0);
// :1269-1272 四个并列门限
let accepted = best.raw_inliers >= MIN_INLIER_MATCHES
            && best.inlier_tiles >= MIN_INLIER_TILES
            && best.residual_gain >= MIN_RESIDUAL_GAIN
            && confidence >= self.options.min_confidence;
// :1285-1288 scene cut
let scene_cut = direct_similarity < 0.5
             && scored.iter().all(|c| c.diagnostics.alignment_error > 0.6);
```
四个要点：候选**先聚类再打分**（天然处理多峰）；**tie-break 偏向更小的 |offset|**；**复合置信度 + 硬门限并列**（`MIN_INLIER_TILES` 防内点挤在一处重复纹理块）；**scene cut 与"没滚动"分离**。

时序区域模型（`region.rs:746-861`）：
```rust
struct RegionState { fixed: f32, scrolling: f32, dynamic: f32, observations: u16 }   // 默认各 1/3
fn weight_at_tile(&self, index: usize) -> f32 {                      // :779-788
    let learned = 1.0 + 1.5*(state.scrolling - 1.0/3.0)
                    - (state.fixed - 1.0/3.0) - (state.dynamic - 1.0/3.0);
    let influence = (state.observations as f32 / 3.0).clamp(0.0, 1.0);
    (1.0 + influence * (learned - 1.0)).clamp(0.1, 2.0)
}
// :811-820 update 的两个 skip
if direct.texture_at(index).max(compensated.texture_at(index)) < 0.05 { continue; }  // 无纹理不学
if direct_similarity * compensated_similarity >= 0.5 { continue; }                   // 歧义不学
let fixed     = direct_similarity * (1.0 - compensated_similarity);
let scrolling = compensated_similarity * (1.0 - direct_similarity);
let dynamic   = (1.0 - direct_similarity) * (1.0 - compensated_similarity);
```
**「歧义 tile 不学习」是防重复纹理污染区域模型的关键。**

缝合决策（`stitcher.rs`）——与 Crisp/docs/19 差异最大：
```rust
// :318-338 完全重复帧 → Skip，不改变 previous_raw_index
// :356-390 未接受位移 → NoMovement，previous_raw 前移，继续
let accepted_offset = match estimate.outcome {
    MotionOutcome::Motion { offset } if offset != 0 && offset.unsigned_abs() as f32 <= maximum_shift => Some(offset),
    MotionOutcome::Motion { .. } | MotionOutcome::NoMotion | MotionOutcome::Indeterminate => None,
};
let Some(offset) = accepted_offset else {
    self.previous_raw = incoming; self.previous_raw_index = index;
    ... branch: StitchBranch::NoMovement ... return Ok(()); };
// :404-476 三个分支
StitchBranch::Append  => { canvas.truncate_end(canvas.extent()-overlap); canvas.append_axis(&previous_raw, vp-band, vp); }
StitchBranch::Prepend => { canvas.truncate_start(band-growth); canvas.prepend_axis(&previous_raw, 0, band); }
StitchBranch::Contained => { self.reference_mode = ReferenceMode::CanvasWindow; }   // 不增长
```
- 枚举（`types.rs:36-42`）：`Append/Prepend/Contained/Skip/NoMovement`；参照系二态（`:44-49`）`Synthetic | CanvasWindow`。
- 每步**先 truncate 再 append**（`:411-417`），重叠区只由新帧像素决定；`synthesize_append/prepend` 让参照系随滚动增长（`:419-439`）——**防长会话 overlap 稀释**。
- 拒绝原因枚举（`snow_stitch_images.h:62-69`）：`NONE / INSUFFICIENT_OVERLAP / LOW_INFORMATION / AMBIGUOUS / CONFLICTING_REFERENCES / FIXED_CONTENT_DOMINATED / VERIFICATION_FAILED`；帧事件（`:46-54`）：`INITIAL / EXTENDED_TOP / EXTENDED_BOTTOM / COVERED / DUPLICATE / UNMATCHED / EXTENDED_LEFT / EXTENDED_RIGHT`。

### 4.10 预览内存上限

唯一上界是 UI 尺寸量 `m_maximumPreviewExtent = 640`（`.h:127`，屏幕像素）与 128 px 交叉轴；`previewLogicalBytesForTesting()/previewAllocatedBytesForTesting()`（`:512-522`）**只测量不设限**；grep `previewBudget|maxPreviewBytes` 无命中。
→ **docs/19 §5.6 规则 2 的"预览内存预算 + `PreviewDegraded`"在 snow_shot 中没有参考实现，属自创要求（未验证）。**

### 4.11 可吸收 / 不可照搬

**可吸收（按价值）**：1) 复合置信度 + 四个并列门限 + |offset| 小者优先；2) 不因单帧失败终止（`NoMovement` 前移继续）；3) `Contained` + 参照系二态；4) 时序 tile 三分类加权（含两个 skip）；5) 降采样失败→全分辨率重跑整个评估；6) prepend + truncate 真双向 canvas；7) overlap 替换行吸收缩放取整；8) 多源暂停汇聚 + `pauseTransition`；9) hover 单飞 + 四重校验 + **先暂停写者再读**；10) cadence 参数几乎可照抄；11) 连续帧源 + DXGI→WGC→GDI + duplicate 标志 + exclusion；12) mask 挖 hole 幂等 + 空 hole 退化；13) 诊断事件契约；14) `WDA_EXCLUDEFROMCAPTURE`；15) tile 化预览 + `compactActiveTile`。

**不可照搬**：Qt/QImage 依赖；`setMaximumPreviewExtent=640` 当内存预算；全分辨率 ORB 作 v1 常驻（docs/19 的取舍合理，但要承认是"以准确率换成本"并保留两级重试接口）；OpenCV 后端分支；改动 128 px 交叉轴。

---

## 5. A4 · 其他参考项目

**5.1 `ScreenSnap-master`（Python/PySide）**：`sticker/sticker_item.py:69-70` `FramelessWindowHint|WindowStaysOnTopHint|Tool` + `WA_TranslucentBackground`；`:695` `setWindowOpacity`；`:702` `WindowTransparentForInput`；`:747` `WindowStaysOnTopHint`。**是 Qt 实现，不含任何 `WS_EX_*` Win32 样式**，与任务预期不符；**无滚动截图**。可吸收价值低。docs/19 §5.5 的 F6 贴图契约应去 Crisp `PinWindow.cpp:321-343` 找 Win32 证据。

**5.2 PowerToys**：ColorPicker `MouseInfoProvider.cs:43-48` 采样周期 = `1000/刷新率`，`:106-118` 每 tick `Graphics.CopyFromScreen` 抓 1×1 像素再 `GetPixel`，**无去抖**；放大镜不是 Magnification API，而是整屏 `CanvasBitmap` + 像素网格（`ZoomWindowHelper.cs:31,182,273,98`、`ZoomView.xaml.cs:32-33,180,198-200`）；`WindowCaptureExclusionHelper.cs:16-27` `WDA_EXCLUDEFROMCAPTURE` + 版本判断 + 只告警一次。MeasureTool `OverlayUI.cpp:44-46,64-65`：`WS_EX_NOREDIRECTIONBITMAP|WS_EX_TOOLWINDOW[|WS_EX_TOPMOST]`，注释说明为排除 Win+Tab 预览。

**5.3 `Starshot-main`**：Windows 原生 HDR 截图（WGC + scRGB + AVIF/JXL/PNGv3），**无滚动截图**。README 对 WGC vs BitBlt vs DXGI 的对比可作 §2.5 旁证。无新东西。

**5.4 `ShareX.ImageEditor`**：标注编辑器；grep `Merge|CombineImages|Stitch|Align` 仅命中 `TextHorizontalAlignment` 与 `EmbroideryImageEffect.StitchSize`（假阳性）。**无任何可复用拼接工具。**

**5.5 `ElegantClipboard-main`**：Electron 剪贴板管理器，无滚动截图，与本题无关。

**5.6 `smart-screenshot-main`（Chrome 扩展）**：`background.js` 用 `chrome.tabs.captureVisibleTab`（`:449,787`）+ `window.scrollTo`/容器 `scrollTop`；**回读实际滚动位置**（`:720-738` 返回 `actualScrollX/Y`，拼接用实际值 `:819`）；DPR 全程参与（`dpr` `:538`，画布 `:876-877`，坐标 `:921-936`）。**没有 sticky/fixed 处理（grep 0 命中）**，绝对坐标摆放导致吸顶元素会在长图里重复 N 次。→ DOM 路径可拿到精确 scrollTop 所以不需要图像匹配；但它反证了「已知精确位移也不能解决 sticky 重复」。

**5.7 `webshot-master`（Rust）**：`browser.rs:453-525` `num_screenshots = effective_height.div_ceil(viewport_height)`，`window.scrollTo(0, i*viewport_height)`，sleep `scroll_delay`（默认 100 ms），`:659-704` 按固定视口高逐块摆放（**无重叠匹配**）。`:537-651` 也支持滚动容器（`FullElement`）与 `max_height` 裁剪。→ 「无匹配拼接」对照实现。

**5.8 `cdp-html-shot-main`（Rust）**：
```rust
// src/tab.rs:205-209
let mut params = json!({ "format": ..., "fromSurface": true,
                         "captureBeyondViewport": opts.full_page });
```
`src/element.rs:61-114` 用 `DOM.getBoxModel` 读 `model.border` 四角 → clip → `Page.captureScreenshot` + `captureBeyondViewport`；`src/tab.rs:66` 另有 `Emulation.setDeviceMetricsOverride`。**这是 docs/19 §9.1 `NativeFullPageStrategy` 与元素级截图的现成技术证据。**（懒加载/sticky/headed 可靠性未验证。）

---

## 6. B · 外部技术路线与竞品

### 6.1 Windows 捕获路线取舍

| 路线 | 关键性质 | 滚动截图里的取舍 | 来源 |
|---|---|---|---|
| **GDI BitBlt** | 抓屏幕上可见内容；硬件加速/HDR 有缺陷；CPU 全屏拷贝 | MS 工程师明确推荐：滚动截图 "**a GDI-based approach is often the most stable and straightforward choice**, while WGC is better suited to real-time capture scenarios" | [MS Q&A](https://learn.microsoft.com/en-us/answers/questions/5722908/best-practice-for-long-scrolling-screenshot-in-win) |
| **PrintWindow / PW_RENDERFULLCONTENT** | 可抓被遮挡窗口，但 Chromium 常黑屏 | 专利 CN108681428A 用「**所有像素是否全为 FFFFFF**」判定失败并退回屏幕 BitBlt | [CN108681428A](https://patents.google.com/patent/CN108681428A/en) |
| **WGC** | 硬件加速、低 CPU、支持 HDR、可按窗口；Win32 互操作成本高（`IGraphicsCaptureItemInterop`+D3D11+长生命周期 `DispatcherQueueController`），任一不符即 `InvalidCastException` | MS 答复：WGC「can be more complex than what's typically required」，仅适合实时/高频 | 同上 |
| **DXGI Desktop Duplication** | 输出/显示器级 | snow_shot 把它当首选、WGC 次之、GDI 兜底（`SCROLLING_DIAGNOSTICS.md:36`）；显示器级抓不到被遮挡窗口内容 | [DuplicateOutput](https://learn.microsoft.com/zh-cn/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutput1-duplicateoutput) |
| **UIA ScrollPattern** | `Scroll`/`SetScrollPercent` 是跨应用标准滚动接口 | 提供精确可验证滚动，但 Chromium/Electron/游戏不实现 | [SetScrollPercent](https://learn.microsoft.com/zh-CN/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationscrollpattern-setscrollpercent)、[Scroll Pattern](https://learn.microsoft.com/en-za/windows/win32/WinAuto/uiauto-implementingscroll) |
| **CDP 全页截图** | `captureBeyondViewport` + `clip` | 一次拿全页、无 sticky 重复，但仅 Chromium 系且需调试通道/扩展 | [参数文档](https://pub.dev/documentation/puppeteer/2.25.1/protocol_page/PageApi/captureScreenshot.html)、`cdp-html-shot-main/src/tab.rs:205-209` |

**输入注入两条路线**：

| | SendInput | PostMessage(WM_MOUSEWHEEL) |
|---|---|---|
| UIPI | **被拦**（官方：`send-input` "is blocked by UIPI when injecting from an elevated process into a lower-integrity (AppContainer/AppX) target"） | **绕过**（官方：`post-message` "is HWND-targeted and **bypasses UIPI (works across integrity levels)**"） |
| 前台要求 | 需 unlocked interactive desktop + 目标在前台 | 不依赖前台，直投 HWND/子 HWND |
| 已知局限 | 干扰用户环境 | 应用可不处理；`GetAsyncKeyState` 看不到修饰键；**WinUI3/UWP 无窗口控件收不到** |
| 参考实现 | Crisp `ScrollCapture.cpp:43-60`、ShareX `InputManager` | snow_shot `scrollinput.cpp:9-61`（下沉子 HWND） |

来源：[winapp-cli UI Automation（Microsoft Learn，我抓取全文）](https://learn.microsoft.com/en-us/windows/apps/dev-tools/winapp-cli/ui-automation)

### 6.2 行业方案快照：专利 CN108681428A

唯一找到的、把滚动截图算法写进权利要求的公开资料：
1. **硬件加速探测**（S101-S103）：取窗口图，若**所有像素 = FFFFFF** 判定为硬件加速 → 走屏幕 BitBlt。
2. **缓存带**（S402/S701）：取图「向上/下 **1/M**」，实施例 `M = 2`。
3. **位移搜索**（S7037-S7039）：逐行比较，**相同行数 > 缓存带行数的 1/4** 即认定有效。
4. **抗误配**（S703）：带**沿垂直方向均分为 X 个矩形区域**（实施例 X=9）逐区域比较，「以避免带滚动条的图或未滚动的动态图导致偏移获取失败」——**多带 consensus 的专利化表述**。
5. **等待**（S603）：滚动后暂停 **200 ms**（权利要求写「K > 100 的自然数」）。
6. **头部保留**（S500）：位移基准取「起始图高度的 **1/4**」，让头部固定图像被包含。

→ `band 起点 = 帧高的 1/4~1/3`、`阈值以带行数比例表述`、`多区域共识`、`100~200 ms 固定等待` 四条在 2018 年已成型，与 Crisp 互相印证。**docs/19 §7.2 的判断正确，且现在有两份独立证据支持「起点必须在中上部而非顶部」。**

### 6.3 竞品能力对照表

✅=厂商文档确证；⚠️=源码/社区确证；❓=未验证。

| 工具 | 滚动截图 | 方向 | 双向 | 采集期实时预览 | 元素级/自动区域 | 失败表现 | 尺寸上限 | 证据 |
|---|---|---|---|---|---|---|---|---|
| **PixPin** | ✅ 含超长模式 | ✅ 纵向 + **横向可切换** | ✅ 反向滑动触发自动裁剪（VIP） | ✅ **开始状态右侧预览 + 缩略图 + 位置指示（绿=匹配成功）** | 手动框选 | 文档列 9 类失败原因 | 文档写 3.2 起「最大 200 万像素长度」，并警告近 100 万像素可能无法导出、约 75 万像素起看图软件打不开（**实测产物见 §1 脚注：`1058 × 502,649 px`**） | [PixPin 文档](https://pixpin.cn/docs/capture/long-capture) ✅ |
| **ShareX** | ✅ | 仅纵向 | ❌ | ❌ 仅完成后（PNG 编解码回环 + 拖拽平移） | ❌ 无 UIA | ✅ **绿/黄/红三态**；红=前两图无法拼接→**自动停止**；黄=best guess | ❌ 无 | [官方文档](https://getsharex.com/docs/scrolling-screenshot) ✅ + `ScrollingCaptureManager.cs:333-384` ⚠️ |
| **Snagit** | ✅（原 Panoramic Capture） | 仅纵向 | ❌ 官方建议「一次只朝一个方向，不要之字形」 | ✅ 采集时可见滚动 | ❌ | 第三方工具/不支持的浏览器/驱动/安全软件都可能失败；**视差网站容易抓歪** | 未验证 | [TechSmith 203731338](https://support.techsmith.com/hc/en-us/articles/203731338-Unable-to-Complete-Scrolling-Capture-in-Snagit) ✅ |
| **Snipaste** | ❌ 未见支持 | — | — | — | — | — | — | 官方 feedback **Issue #3269「请支持滚动截屏功能」**（功能请求）→ 强推断不支持 |
| **Windows 截图工具** | ❌ | — | — | — | — | — | — | MS 官方答复原文：「This is by design. **You will not be able to scroll to capture** a webpage/window that continues below the screen...」（2011）；2024–2026 现状未验证 → [MS Q&A](https://learn.microsoft.com/en-za/answers/questions/2491900/using-snipping-tool-to-capture-a-webpage-that-cont) ✅/❓ |
| **FastStone Capture** | ✅ 有"Scrolling Window" | ❓ | ❓ | ❓ | ❓ | ❓ | ❓ | 官方帮助为 PDF，**未抓取** ❓ |
| **Greenshot** | ❌ 未见 | — | — | — | — | — | — | ❓ |
| **Edge / Firefox 全页截图** | ✅ 浏览器内部（非滚动拼接） | 整页 | n/a | 编辑器内可见 | Edge 有"捕获区域" | n/a | 浏览器限制 | docs/19 已列链接；**本次未抓取** ❓ |
| **CDP 全页截图** | ✅ `captureBeyondViewport` | 整页/元素 clip | n/a | n/a | ✅ 元素级 clip | 错误码/超时 | CDP/浏览器限制 | [参数文档](https://pub.dev/documentation/puppeteer/2.25.1/protocol_page/PageApi/captureScreenshot.html) ✅ + `element.rs:61-114` ⚠️ |
| **Crisp** | ✅ | 纵向 + 横向（Auto 先竖后横） | ❌ | ❌（仅进度 toast） | ❌ | 阈值不过即停；UI 无「部分成功」提示 | `kMaxImageSide = 32767` | `ScrollCapture.cpp`、`Stitch*.cpp` ⚠️ |
| **snow_shot** | ✅ | 纵向 + 横向 | ✅ **真双向**（`Prepend`） | ✅ patch 预览 + 视口高亮 + 裁剪手柄 + hover 放大 | ❌ | 7 类 `UnmatchedReason` + 8 类 frame event；不因单帧失败终止 | canvas/`int` 限制 | `snow-stitch-images/src/*.rs` ⚠️ |

**产品结论**：① 横向是差异化能力（PixPin/snow_shot 有）；② **双向几乎无人做好**（Snagit 明说别之字形，ShareX 单向，只有 snow_shot 原语真支持，PixPin 用"反向→自动裁剪"绕法）；③ **采集期实时预览是强差异化**；④ **失败表现两极**（ShareX 三色坦白，Snagit 只给排查清单）；⑤ 超大图必须给上限与提示。

### 6.4 位移估计方法对比

| 方法 | 原理 | 重复纹理鲁棒性 | 光照/抖动 | 复杂度 | 1080p 量级 | 实现难度 | 子像素 |
|---|---|---|---|---|---|---|---|
| **行/列 profile SAD/L1** | 逐行差累加找最小 | **差**：周期内容多个 offset 得分接近，argmin 会系统性选错（Crisp 的弱点：无 margin 判据） | 好 | `O(W·S)`，early-exit 后退化为 `O(W·k)` | 微秒~毫秒 | 极低 | 需插值 |
| **NCC/ZNCC** | 归一化互相关 | **中**：压低平坦区虚高得分，但周期仍多峰 | **最好** | `O(W·H·S)` | 毫秒~数十毫秒 | 低 | 需拟合 |
| **相位相关（FFT）** | `F1·F2*/|F1·F2*|` 逆变换峰位=位移 | **差且危险**：会产生**多个等高峰**且峰高不可直接比较；Optik 2015 有专文 *Robust rigid registration by scanning multiple phase correlation peaks* | 中（需加窗/带内归一化/梯度预处理） | `O(N log N)`，与位移范围无关 | 数十毫秒 | 中 | ✅ 天生 |
| **ORB/AKAZE + 内点投票** | 二值描述子 + 汉明 + Lowe 比值 + 内点聚类 | **最好（需空间分布门限）**：snow_shot 用 `MIN_INLIER_TILES=4` 强制内点分散，正是为拒绝"单块内假一致" | 中 | `O(K²)`（K ≤ 2500） | 数十~上百毫秒（Rust+AVX2） | **高** | ❌ 需额外精修 |

**结论**：profile SAD 作粗搜索合适但**必须补"候选不唯一"判据**（margin + 空间分布）；相位相关**不建议作主路径**（同样的多峰歧义 + FFT 成本 + 峰高不可比）；ORB 的价值在**重复纹理下的唯一性**（靠内点空间分布），docs/19 把它放第 4 位是工程可接受的分期，但**应把 `MIN_INLIER_TILES` 的思想提前到 v1**（如"至少 N 个不同 y 带各自支持同一 delta"）；**先快后准两级策略**性价比最高。

---

## 7. 对 `docs/19` 的具体建议

### 7.1 已被证明可行（保持）

| docs/19 条目 | 证据 |
|---|---|
| §5.1/§5.6 overlay 排除自身窗口（`WDA_EXCLUDEFROMCAPTURE`） | snow_shot `windowchrome.cpp:241-256` + PowerToys `WindowCaptureExclusionHelper.cs:23-27` |
| §5.6 128 px / 256 px tile / replace-append-prepend / overlap 替换行 / 局部失效 / hover 单飞 | `thumbnailwidget.cpp:23-24,186-214,309-449,805-817`、`scrollinghoverpreview.h:93-121` 逐项对应 |
| §6.3 自适应 settled | Crisp 固定 `settleMs=260` 是反面样本 |
| §6.6 cadence | `adaptivescrollingcapturecadence.h:9-20,55-64,148-168` + `pipeline.cpp:511,518`，**参数几乎可照抄** |
| §6.7 终点二次确认 | snow_shot 连续 NoMovement + 探针；Crisp 单次不匹配即停是反面 |
| §7.2 先规划后分配 + band early-exit | `StitchInternal.h:44-71`、`Stitch.cpp:32-44` |
| §7.2 不在最顶端取带 | `Stitch.cpp:61-75`（1/3）+ 专利 `S500`（1/4）+ `S703`（9 区域共识）——**三方独立印证** |
| §7.2 margin/多带/拒绝语义 | Crisp 缺此 → 必须补；snow_shot 给了完整范本（`estimator.rs:1233-1272`） |
| §8.2 sticky 首/末帧各写一次；footer 截断搜索区；「所有帧稳定」才算 run | `StitchVertical.cpp:42-60,97-102,148-151`、`Stitch.cpp:174-186` |
| §8.2 新帧覆盖 overlap + 质量保护 | `stitcher.rs:411-417` + `Contained` 分支 |
| §10.3 事件契约 | `SCROLLING_DIAGNOSTICS.md` 的 backend/status 码与 `input_state` 字段 |

### 7.2 缺少证据支撑（降级为"自行验证"或改设计）

1. **§5.6 规则 2 的预览内存预算 + `PreviewDegraded`**：snow_shot **没有**字节预算（640 是屏幕像素）；属自创要求，且"两端摘要"没有先例。
2. **§5.6 规则 4「每个 patch 只使并集失效」**：snow_shot 只有 hover 用 `update(dirty)`，其余整 widget `update`。**无参考先例**，收益需自测。
3. **§6.5 闭环步长数值（0.30–0.40 / 0.20 / 0.12 / 4 次健康才 +1 notch）**：三个参考项目都没有闭环步长（Crisp 固定 3 齿、ShareX 固定 2×120、snow_shot 固定 ±120 调 cadence）。**完全自创**，必须用速度-质量矩阵自证。
4. **§7.4 全局重锚定**：三个参考项目都没做（snow_shot 靠 `synthetic_reference` 增长间接抑制漂移，`stitcher.rs:419-439`，**是另一种解法**）。应并列讨论「合成参照系增长」这条已证明可行的替代路线。
5. **§7.3 `band_score = texture + edge + ... - penalty` 的加分式线性加权**：snow_shot 是**乘法/概率式**（tile 权重）+「歧义不学习」排除。建议改为乘法形式或给出标定实验。
6. **§5.6 规则 2「预览可旋转为横向布局」**：snow_shot 是**几何换轴**，不是 UI 旋转。建议改述为「按轴重排」。
7. **§9.1 `NativeFullPageStrategy` 的权限/协议探测**：CDP 可做（`cdp-html-shot-main`），但**如何对用户已有浏览器窗口建立 CDP 连接（需 `--remote-debugging-port` 或扩展）未验证**；应标为"需要用户侧前置条件"。
8. **§8.4 的默认上限与架构上限未分离**：见 §1 结论 #12 的限定语——30,000 px 可作**默认策略值**，但架构（tile 化 + 流式写盘）必须支撑到 PixPin 实测量级（50 万 px，`docs/25` 证据），不得在纯内存画布阶段被钉死（参见 `docs/24` S1.8）。

### 7.3 遗漏了参考项目已踩过的坑（必须补）

1. **【最高优先】一帧匹配失败不应终止整个会话。** docs/19 §7.2 的"搜索无结果即在该帧之前停止"在 100 步会话里会被一次 loading 动画毁掉。证据：`stitcher.rs:367-390` 记 `NoMovement`、前移 `previous_raw` 并继续；Crisp 的"停止"适合批处理不适合交互式长会话。**建议**：拆开「本帧不可用（跳过继续）」与「滚动已结束（Confirmed）」，容忍 N 次（建议 3–5）连续拒绝；§6.7 与 §7.2 的冲突需统一。
2. **缺少 `Contained`（新帧完全落在已有画布内）分支。** 触发场景：小步长抖动、用户回滚、平滑滚动中间帧。docs/19 隐含处理了"不增长"，但**没处理"此时不能再用上一原始帧当参照"**（会反复接受同一位移）。建议显式定义 `Contained` + 参照系二态。
3. **缺少「合成参照系」概念。** snow_shot 每步 `synthesize_append/prepend` 更新参照（`:419-439`），使 overlap 不随长会话稀释；docs/19 只保留 2–4 个微型摘要，长会话下 overlap 会被固定区侵蚀。
4. **sticky 应从"掩码/裁剪"升级为"加权"**：`weight_at_tile` 降权而非剔除（0.1–2.0 钳制，观察 <3 次保持中性，`region.rs:779-788`）。docs/19「全部带动态 → Uncertain」是硬失败，容易误停。
5. **`MIN_INLIER_TILES` 类空间分散度门限缺失**：snow_shot 的 `MIN_INLIER_MATCHES=8 + MIN_INLIER_TILES=4 + MIN_RESIDUAL_GAIN=0.15` 三元组可直接借鉴为 v1 的"至少 K 个不同 y 带各自支持同一 delta"。
6. **「|offset| 小者优先」tie-break 缺失**（`estimator.rs:1184-1186`）：重复纹理下最省事且有效的启发式。
7. **输入注入策略被过度收窄（§6.2）**：应实现两种传输 + 用 snow_shot 的 5 类 status 上报结果 + 以「是否观察到位移」选择与降级 + 在 docs 记录实测差异。
8. **「用户抢回焦点即停止」缺失**：ShareX `Activated/Closing → StopCapture()`（`ScrollingCaptureWindow.axaml.cs:103-120`）。
9. **超大图用户可见提示缺失**：PixPin 明确警告 100 万/75 万像素两条门槛；docs/19 只有技术阈值。**补充**：`docs/25` 已确认 PixPin 实际产物达 `1058 × 502,649 px`，故提示文案应表述为"接近/超过某个量级后普通看图软件可能打不开"，而不是在 30,000 px 处就暗示超限。
10. **场景突变（scene cut）与"内容变了"的区分缺失**：`estimator.rs:1285-1291` + `scene_cut_streak`（`:972-991`）；建议区分"加载中/内容重排"（等待重试）与"画面真换了"（停止并标 Partial）。
11. **诊断字段可再补一批**：`input_state` 的 overlay 矩形、DPR、mask 状态、`full_hole`、`thumbnail_visible`、`native_hole_contains_center`、`input_target`（`SCROLLING_DIAGNOSTICS.md:19-20`）——正是滚动截图最常见的静默失败点。

### 7.4 需更正的事实性错误

- **§2.4 表格「Crisp `TestStitch.cpp` … 重复纹理 …」不存在**（实际清单见 §2.7；`MakeFlat` 是纯色不是重复纹理）。
- **§2.4「ShareX … 选择窗口后显示可点击穿透的区域边框」措辞需精确化**：ShareX 选的是**区域**（`GetRectangleRegionAsync`），滚动注入用窗口句柄；边框窗口是**独立创建的 1 px 环**，不是"选区本身的边框"。
- **§2.4「ShareX 滚动方法/步长/延迟可配置」应补"没有回退链"**：`ScrollMethod` 是用户手选裸 switch（`ScrollingCaptureManager.cs:130-150`），无任何自动探测/失败切换。

---

## 8. 参考链接

**本地只读**：`refer/shot-refer/Crisp-main/src/{ScrollCapture.cpp,ScrollCapture.h,Stitch.cpp,Stitch.h,StitchInternal.h,StitchVertical.cpp,StitchHorizontal.cpp,Capture.cpp,Capture.h,AppActions.cpp,Toast.cpp}`、`tests/TestStitch.cpp`；`refer/shot-refer/ShareX-develop/ShareX.ScreenCaptureLib/{ScrollingCaptureManager.cs,ScrollingCaptureOptions.cs,ScrollingCaptureService.cs,Enums.cs,Presentation/ScrollingCapture/*}`、`ShareX.HelpersLib/{Input/InputHelpers.cs,SkiaPixelBuffer.cs}`；`refer/snow-apps/snow_shot/src/presentation/capture/*`、`src/presentation/overlay/screenshotoverlaywindow.cpp`、`src/platform/windows/{scrollinput.cpp,windowchrome.cpp}`、`SCROLLING_DIAGNOSTICS.md`、`refer/snow-apps/snow-crates/crates/snow-stitch-images/src/{estimator,region,stitcher,types}.rs`、`snow-stitch-images-c/include/snow_stitch_images.h`；`refer/ScreenSnap-master/sticker/sticker_item.py`；`refer/shot-refer/PowerToys-main/src/modules/{colorPicker,MeasureTool}/**`；`refer/shot-refer/Starshot-main/README.md`；`refer/smart-screenshot-main/background/background.js`；`refer/webshot-master/src/browser.rs`；`refer/cdp-html-shot-main/src/{tab,element}.rs`

**外部**
- [Best practice for long scrolling screenshot in Win32/WPF? WGC vs GDI — Microsoft Q&A](https://learn.microsoft.com/en-us/answers/questions/5722908/best-practice-for-long-scrolling-screenshot-in-win)
- [UI Automation（winapp-cli）— post-message vs send-input / UIPI 原文](https://learn.microsoft.com/en-us/windows/apps/dev-tools/winapp-cli/ui-automation)
- [SetWindowDisplayAffinity](https://learn.microsoft.com/it-it/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity)
- [IDXGIOutput1::DuplicateOutput](https://learn.microsoft.com/zh-cn/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutput1-duplicateoutput)
- [IUIAutomationScrollPattern::SetScrollPercent](https://learn.microsoft.com/zh-CN/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationscrollpattern-setscrollpercent) ｜ [Scroll Control Pattern](https://learn.microsoft.com/en-za/windows/win32/WinAuto/uiauto-implementingscroll)
- [Chrome DevTools Protocol — Page domain](https://chromedevtools.github.io/devtools-protocol/#/Page)
- [captureScreenshot 参数（captureBeyondViewport 原文）](https://pub.dev/documentation/puppeteer/2.25.1/protocol_page/PageApi/captureScreenshot.html)
- [ShareX — Scrolling screenshot（绿/黄/红语义与限制）](https://getsharex.com/docs/scrolling-screenshot) ｜ [#1275](https://github.com/ShareX/ShareX/issues/1275) ｜ [#7531](https://github.com/ShareX/ShareX/issues/7531) ｜ [#7916](https://github.com/ShareX/ShareX/issues/7916)
- [PixPin — 长截图（横向、预览、自动裁剪、200 万像素上限、失败原因清单）](https://pixpin.cn/docs/capture/long-capture)
- [Snipaste feedback #3269 — 请支持滚动截屏功能](https://github.com/Snipaste/feedback/issues/3269)
- [TechSmith — Unable to Complete Scrolling Capture in Snagit](https://support.techsmith.com/hc/en-us/articles/203731338-Unable-to-Complete-Scrolling-Capture-in-Snagit)
- [Microsoft Q&A — Snipping Tool 无法抓取屏幕以下内容](https://learn.microsoft.com/en-za/answers/questions/2491900/using-snipping-tool-to-capture-a-webpage-that-cont)
- [专利 CN108681428A — 一种 Windows 系统下的滚动截图方法](https://patents.google.com/patent/CN108681428A/en)
- [FastStone Capture 帮助（PDF，未验证）](https://documentation.help/back.FSCaptureHelp/documentation.pdf) ｜ [Greenshot 帮助（未验证）](https://getgreenshot.org/archive/help/zh-cn/)
- [Optik 2015 — Robust rigid registration by scanning multiple phase correlation peaks（未取得可访问全文）](https://www.sciencedirect.com/science/article/abs/pii/S0030402615013373)
- [StackOverflow — phase correlation 2D (Stitch 2D in ImageJ)](https://stackoverflow.com/questions/9324463/phase-correlation-2d-stitch-2d-in-imagej-for-image-stitching)
- [SeleniumBase #3858 — Reliable Full-Page Screenshot in Headed Chrome](https://github.com/seleniumbase/SeleniumBase/discussions/3858)

**本仓库内相关文档**
- `docs/19-scroll-capture-design.md`（评审目标）
- `docs/24-scroll-capture-tasklist.md`（S1.8 纯内存画布阶段的相关约束）
- `docs/25-pixpin-benchmark-and-scroll-capture-review.md`（本文所服务的评审报告）
- `docs/26-pixpin-static-analysis.md`（PixPin 静态取证）

---

## 9. 未解疑点

1. **`PostMessage(WM_MOUSEWHEEL)` 在 Chromium/Electron 上到底行不行。** Crisp 注释说不行，snow_shot 生产代码正是这么做并下沉到子 HWND。可能差异在"是否做了子窗口下沉"或浏览器版本。**这是 §6.2 决策关键，必须实测**（Chrome / Edge / Electron（VS Code、Discord）/ WinUI3 各一次，两种传输对比）。
2. **UIPI 的方向性。** 官方文档说 `post-message` bypasses UIPI，而 `send-input` 的反例方向是 elevated → lower-integrity。经典 UIPI 是低完整性不能给高完整性窗口发消息。**PostMessage 到提权窗口是否真的可行未验证**（snow_shot status 4 明确把 access denied 列为可能）——这直接决定「管理员运行的记事本/浏览器」能否被抓。
3. **`captureBeyondViewport` 对懒加载与 sticky 的实际行为。** 官方只有一句话；是否需要先滚动触发懒加载、`position: fixed` 是否重复绘制、headed 模式是否可靠，均**未验证**。
4. **WGC 在 overlay 可见时抓窗口是否会把自己画进去。** snow_shot 用 `WDA_EXCLUDEFROMCAPTURE` + exclusion 列表，说明**窗口级捕获也需显式排除**。**`SetWindowDisplayAffinity` 对 WGC 窗口捕获与显示器捕获是否都生效未验证**（两份实现的注释都暗示两者行为可能不同）。
5. **FastStone Capture 与 Greenshot 的滚动截图能力与限制**：未抓取官方文档（PDF），表格相关格标 ❓。
6. **2024–2026 年 Windows 11 截图工具是否新增滚动截图**：只找到 2011 年"设计如此"的答复，现状未验证。
7. **`AutoIgnoreBottomEdge` 的 `bestIgnoreBottomOffset` 跨帧记忆是否会累积错误**：它被 `Math.Max(...)` 单向放大（`:301`）且只在上界 `H/3` 截断。**动态底边高度变化时是否长期高估（丢真实内容）未验证**；若是，docs/19 不应照搬"取 max"。
8. **snow_shot 的 preview 是否有隐式内存上限**：只找到屏幕像素上限与测量函数；可能在 `LatestBridgeMailbox` 或 controller 的 `cachedSnapshot*`（`controller.cpp:749-753`）里有未发现的预算。需继续追 `ScrollingSnapshotRequest`/`cachedSnapshot` 生命周期。
9. **Crisp 水平 auto 探测在真实应用上的成功率与副作用**：`ScrollCapture.cpp:146-161` 只给一次机会。`HWHEEL` 被忽略的应用里这次尝试是否会产生副作用（误触横向滚动条）未验证；docs/19 §6.4 做探测时应参考这一风险。
10. **`refine_motion_offset` 的精修细节未读完**（`estimator.rs:826-876` 只看了签名与调用点）；docs/19 §7.2 第 4 条（候选 ±4..8 px 内全分辨率精修）是否与它等价，未验证。

---

*所有行号基于实际读取的文件内容；`refer/` 未做任何修改。本文为 `docs/25` 的证据附录。*

---

## 附：与 `docs/25` 的对应关系

- 结论 #3（Crisp `TestStitch.cpp` 无重复纹理用例）与 §7.4 的三条事实性更正（重复纹理用例不存在 / ShareX 区域边框措辞需精确化 / ShareX 无滚动方法回退链）→ `docs/25` **R11**。
- 结论 #8（单帧匹配失败不应终止会话；证据 `stitcher.rs:356-390` 的 `NoMovement` 继续前移）→ `docs/25` **R13**「单帧失败不得终止会话」。
- 结论 #9（`PostMessage` 被过度排除；证据 `scrollinput.cpp:52-54` + Microsoft Learn 的 UIPI 原文）→ `docs/25` **R15**「输入注入不应只留 SendInput」。
- §4.9（snow_shot 的复合置信度、四个并列门限、`|offset|` 小者优先、`MIN_INLIER_TILES` 空间分散度）→ `docs/25` **R17**「吸收 snow_shot 的判据结构」。
- 结论 #12 及其限定语（30,000 px 只能作**默认策略值**；架构必须支撑到 PixPin 实测量级 50 万 px）→ `docs/25` 的尺寸上限条目，并与 `docs/24` S1.8 的纯内存画布阶段约束对照。
- §7.3 其余「已踩过的坑」（`Contained` 分支与参照系二态、合成参照系增长、tile 加权替代硬掩码、抢回焦点即停、scene cut 与"未滚动"区分、输入侧诊断字段）→ `docs/25` 对应建议条目（编号以 `docs/25` 为准）。
- §7.2 的「缺少证据支撑」清单（预览内存预算与 `PreviewDegraded`、patch 并集失效、闭环步长数值、全局重锚定、加分式 `band_score`、横向"旋转"措辞、CDP 前置条件）→ `docs/25` 的「未验证 / 需自行基准」条目。
- §6.4 的方法学结论（profile SAD 必须补 margin 与空间分布判据；相位相关不宜作主路径）→ `docs/25` 的 matcher 选型依据。
- §9 的 10 条未解疑点 → `docs/25` 的遗留风险清单；其中第 1、2 条（`PostMessage` 在 Chromium/Electron 的可行性、UIPI 的方向性）应作为 v1 落地前的**实测闸门**。
- 「尺寸上限」上的两方观点（本报告的默认阈值保守性论证 vs 评审方的 50 万 px 实测量级）均已在 §1 结论 #12 的脚注中完整保留，供 `docs/25` 双向呈现。
