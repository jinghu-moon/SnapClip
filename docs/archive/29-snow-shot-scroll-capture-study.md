# Snow Shot / snow-apps 滚动截图与拼接引擎深度研读

- **标的**：`refer/snow-apps`（Snow Shot，Windows + macOS 截图工具），重点 `refer/snow-apps/snow_shot` 与 `refer/snow-apps/snow-crates/crates/snow-stitch-images`。
- **对标位置**：与 `docs/25-pixpin-benchmark-and-scroll-capture-review.md`（PixPin 对标）并列。PixPin 是**闭源二进制**对标，Snow Shot 是**开源同类型软件**对标，两者互补：前者给出产品与上限的实测边界，后者给出可读、可复用的完整实现。
- **文档地址**：本文件为 SnapClip 仓库内文档，`refer/` 被 `.gitignore` 忽略，故所有 `refer/` 证据只能在本地复核。
- **日期**：2026-10-08。

---

## 0. 方法与边界

### 0.1 事实来源与取证强度

| 等级 | 含义 | 本次来源 |
|---|---|---|
| **确证（本人精读原文）** | 直接读取源码/头文件原文得到，行号可复核 | `snow_shot/SCROLLING_DIAGNOSTICS.md`、`snow_shot/src/platform/windows/scrollinput.cpp`、`snow_shot/include/snow_shot/presentation/screenshotscrollingtypes.h`、`screenshotscrollingpipeline.h`、`screenshotscrollingsnapshot.h`、`screenshotscrollingcapturecontroller.h`、`screenshotimagerowsource.h`、`screenshotexportscrollingsource.cpp`、`adaptivescrollingcapturecadence.h`、`screenshotscrollingautoscroller.h`、`scrollingstepinput.h`、`scrollinghoverpreview.h`、`scrollingselectionmovement.h`、`scrollingsnapshotrequest.h`、`snow-crates/crates/snow-stitch-images/src/types.rs`、`state.rs`、`region.rs:700-829`、`estimator.rs:1-120`、`tiled_canvas.rs:1-200`、`snow-crates/crates/snow-stitch-images-c/src/lib.rs:288-451`、`include/snow_stitch_images.h`、根 `CMakeLists.txt` 的 scrolling 条目、`settingscatalog.cpp` 的滚动条目 |
| **确证（子代理取证，附 `文件:行号`）** | 由 4 个并行只读子代理完成，笔记落盘 `docs/Temp/snow-*-deep.md`，本文件引用时标注来源笔记 | Rust 引擎（`snow-stitch-images-deep.md`，994 行）、C++ 管线（`snow-shot-cpp-scrolling-deep.md`，994 行）、捕获层（`snow-capture-wgc-deep.md`，2052 行）、测试/基准（`snow-scrolling-tests-deep.md`，418 行） |
| **未验证** | 本报告未运行 `ctest`、未构建 snow-apps、未运行任何基准 | 所有性能数字均为**代码中的门槛/参数**，不是实测结果；凡涉性能结论均降级为"设计意图" |

### 0.2 边界

- 只读。`refer/snow-apps` 下未修改任何文件。
- 未构建。snow-apps 需要 VS 2026 + CMake 4.2 + MSVC 14.51 + Rust 1.97.1 + 仓库托管 vcpkg/Qt，本机不具备；且其 `AGENTS.md` 明令禁止跑全量 `ctest`、要求基准必须用 `windows-msvc-performance` Release preset。**因此本报告不含任何 snow-apps 实测数据。**
- 本报告不复制 `refer/` 的 GPL 代码。见 §1.2 的许可边界。

### 0.3 与 `docs/19` 的关系

`docs/19-scroll-capture-design.md:120` 已把 `refer/snow-apps/snow_shot` 列为参考实现之一，`docs/19:132-136` 的"可吸收的参考结论"表里已有 5 条 snow_shot 条目（cadence / overlay-worker / thumbnail widget / pipeline preview patch / hover preview），`docs/19:514` 也再次引用。

**本次研读的首要任务是核对这 5 条是否属实**（结论：5 条全部属实，见 §8.1），**第二任务是把 `docs/19` 没吸收、但实现里真实存在的机制挖出来**（见 §8.2 起）。

---

## 1. 仓库与许可全景

### 1.1 结构

```
refer/snow-apps/
├── AGENTS.md, CMakeLists.txt, CMakePresets.json, vcpkg.json, rust-toolchain.toml
├── README.md, LICENSE.md, docs/, docs-macos-build.md
├── scripts/, cmake/, test-support/, homebrew/, licenses/
├── .cargo/, .cursor/, .github/
├── snow_shot/            # 主应用（C++/Qt6），含 src/ include/ tests/ i18n/ rust/ packaging/
├── snow_image/           # C++/Qt 库
├── snow_image_viewer/    # C++/Qt 应用
├── ant_design_qt/        # C++/Qt 控件库
├── snow_draw_engine_qt/  # C++/Qt 绘制引擎
├── snow_rust_ffi/        # 把 snow-crates 打成 CMake 可消费的静态库
└── snow-crates/          # 29 成员 Rust workspace（crates/*）
```

`snow_shot/` 内部（滚动截图相关目录）：

```
snow_shot/
├── SCROLLING_DIAGNOSTICS.md          # 滚动截图诊断事件契约（77 行）
├── CMakeLists.txt                    # scrolling 目标接线（大量条目）
├── include/snow_shot/
│   ├── platform/windows/scrollinput.h
│   └── presentation/
│       ├── screenshotscrollingcapturecontroller.h
│       ├── screenshotscrollingtypes.h
│       ├── screenshotscrollingsnapshot.h
│       ├── screenshotscrollingthumbnailwidget.h
│       └── screenshotimagerowsource.h          # ← 行带端口
├── src/
│   ├── platform/windows/scrollinput.cpp
│   ├── image/snowimageqtcodec.cpp              # static_cast_rowSource/readRows 生产者
│   └── presentation/capture/
│       ├── adaptivescrollingcapturecadence.h
│       ├── latestbridgemailbox.h
│       ├── screenshotscrollingautoscroller.h
│       ├── screenshotscrollingcapturecontroller.cpp
│       ├── screenshotscrollingnativesource.cpp
│       ├── screenshotscrollingpipeline.cpp
│       ├── screenshotscrollingthumbnailwidget.cpp
│       ├── scrollinghoverpreview.h
│       ├── scrollingselectionmovement.h
│       ├── scrollingsnapshotrequest.h
│       ├── scrollingstepinput.h
│       └── windowinputtransparency.h / windowcaptureexclusion.h
└── tests/                            # 15 个滚动相关测试/基准文件
```

`snow-crates/crates/` 中与滚动截图直接相关的 5 个 crate：

| crate | 许可 | 角色 |
|---|---|---|
| `snow-memory` | Apache-2.0 | 大光栅的显式 OS 页所有权（`RasterBuffer` / `RasterArray<T>`） |
| `snow-capture` | Apache-2.0 | 桌面捕获（DXGI Duplication / WGC / GDI + 自动降级 + 流协议） |
| `snow-capture-c` | Apache-2.0 | `snow_capture.h` C ABI（18 KB） |
| `snow-stitch-images` | Apache-2.0 | **有状态滚动拼接引擎**（纯 CPU、无 OS 依赖） |
| `snow-stitch-images-c` | Apache-2.0 | `snow_stitch_images.h` C ABI（234 行） |

### 1.2 许可边界（**对 SnapClip 的法律关键点**）

`refer/snow-apps/LICENSE.md` 是**逐目录的多许可**：

| 目录 | SPDX |
|---|---|
| `snow_shot/`、`snow_image/`、`snow_image_viewer/` | **GPL-3.0-or-later** |
| `ant_design_qt/`、`snow-crates/`、`snow_draw_engine_qt/`、`snow_rust_ffi/` | **Apache-2.0** |
| 根级编排文件（根构建文件、`.github/`、`docs/`、`scripts/`、共享 `cmake/`） | GPL-3.0-or-later |

Copyright (C) 2025-2026 mg-chao。

SnapClip 自身是 **AGPL-3.0**（根 `LICENSE`）。要点：

1. **算法与设计思想**（本报告的主体）不受版权保护，可自由吸收——这也是 `docs/19:120` 的既定立场（"只吸收算法原则、贴图生命周期和测试思想，不复制其 GDI/Qt/C++/Avalonia 管线"）。
2. **`snow-crates/` 是 Apache-2.0**，理论上可以真的复用代码（保留 NOTICE/许可与归属）。`snow-stitch-images/Cargo.toml` 的 `description` 自述 **"Clean-room Rust implementation workspace for stateful vertical screenshot stitching"**。
3. C++ 应用层（`snow_shot/`）是 GPL-3.0-or-later；GPLv3 §13 允许与 AGPLv3 组合，但一旦把 GPL 代码并入 SnapClip，SnapClip 的 AGPL 义务会**向下传染到该目录**，且与 SnapClip 的"尚未发布、可破坏性改造"阶段无益。**建议：只读参考，不并入**。
4. **本报告的默认立场**：所有 snow 结论都是"设计证据"，SnapClip 按自身架构重写。凡真正要抄的，仅限 §8 中标注为"语义级、与拼接内核无耦合"的机制。

---

## 2. 总架构：五层，且拼接内核与平台完全解耦

Snow Shot 的滚动截图不是一个模块，而是**跨语言、跨 crate 的六段流水线**：

```
┌─ C++/Qt (GPL-3.0) ─────────────────────────────────────────────────────────┐
│  ScreenshotScrollingCaptureController   会话状态机、注入契约、交互          │
│    ├── ScreenshotScrollingPipeline      线程编排、preview patch、快照       │
│    ├── ScrollingHoverPreview / ThumbnailWidget / SelectionMovement          │
│    ├── ScreenshotScrollingAutoScroller  自动滚动 QTimer                     │
│    └── platform/windows/scrollinput.cpp PostMessageW(WM_MOUSEWHEEL)         │
└────────────────────────┬───────────────────────────────────────────────────┘
                         │ C ABI: snow_stitch_images.h (234 行)
┌────────────────────────▼───────────────────────────────────────────────────┐
│  snow-stitch-images-c              会话/快照/导出/PNG 的 FFI 包装           │
├─────────────────────────────────────────────────────────────────────────────┤
│  snow-stitch-images (Apache-2.0)   纯 CPU 有状态拼接引擎                    │
│    estimator(ORB) → state(union) → compositor → tiled_canvas → snapshot     │
├─────────────────────────────────────────────────────────────────────────────┤
│  snow-memory (Apache-2.0)          RasterBuffer: OS 页/大页所有权           │
└─────────────────────────────────────────────────────────────────────────────┘
                         ▲ 同时
┌────────────────────────┴───────────────────────────────────────────────────┐
│  snow-capture (Apache-2.0)   DXGI Duplication / WGC / GDI + 自动降级        │
│    → snow-capture-c → C ABI                                                │
└─────────────────────────────────────────────────────────────────────────────┘
```

**这一分层本身就是对 `docs/19` 的最强支持**：`docs/19` §1 的主张（活动帧源 + 位移匹配 + union 画布）在 Snow Shot 里被一个成熟实现逐条印证，而且 Snow Shot 把"拼接"做成了**不依赖任何平台 API 的纯 Rust crate**，`snow-capture/src` 里 grep `scroll|stitch|long_capture` 只命中注释与测试辅助函数——**捕获层完全不知道滚动截图的存在，拼接层完全不知道 Windows 存在**。

`snow_shot/SCROLLING_DIAGNOSTICS.md` 是这条链路的可观测性契约（24 个 `scrolling.*` 事件 + 1 个 perf counter），下文按层展开。

---

## 3. `snow-stitch-images`：有状态拼接引擎（本次研读的核心）

### 3.1 公开面与 crate 结构

```
snow-crates/crates/snow-stitch-images/src/
├── lib.rs            28   模块声明 + 公开面
├── types.rs         195   ← 默认选项、轴抽象、错误类型
├── error.rs          64
├── decisions.rs      30   ← StitchProgressState / StitchDecision（可序列化审计记录）
├── frame.rs         713   Frame / Geometry / PixelFormat / from_strided / from_row_ranges
├── state.rs         142   ViewportState：union 坐标模型（append/prepend/contained）
├── compositor.rs    627   band_height + append/prepend_and_repaint + synthesize_*
├── tiled_canvas.rs 1286   256 px tile + 页后备 RasterBuffer + 快照租约
├── sampling.rs      271   PyramidPlan（降采样采样计划）
├── region.rs       1106   32 px 估计 tile 网格 + 三分类时序模型 + 相似度图
├── orb.rs          1739   纯 Rust ORB（detect / FAST-9_16 / Harris / brief / compute）
├── estimator.rs    2036   位移估计主流水线（候选投票 → 内点 → 置信度）
├── stitcher.rs      931   每帧状态机 + 参照系切换 + 不变量断言
├── perf.rs          119   16 阶段 thread_local 计时（编译期禁止跨线程）
└── estimator_optimization_tests.rs 373  内联优化/等价性测试
```

公开模块：`compositor` `decisions` `error` `estimator` `frame` `state` `stitcher` `tiled_canvas` `types` `perf`；私有模块：`orb` `region` `sampling`。**`orb.rs` 与 `region.rs` 是私有的**——外界无法替换特征提取或区域模型，只能通过 `MotionEstimatorOptions` 调参。

依赖极薄：`snow-memory`(path)、`anyhow`、`image`、`rayon`、`serde`、`serde_json`、`thiserror`；dev-deps `proptest 1.11.0`、`tempfile`。**没有 OpenCV、没有 GPU、没有 OS API。**`[[bin]]` 是 `snow-stitch-images`（`required-features=["cli"]`）。

### 3.2 轴抽象：3 个 `const fn` 覆盖两个方向

`types.rs:11-32`：

```rust
primary_delta(dx, dy)   // Vertical → dy, Horizontal → dx
cross_delta(dx, dy)     // Vertical → dx, Horizontal → dx?? → 实为 Horizontal → dy
primary_extent(w, h)    // Vertical → h, Horizontal → w
```

全套算法按"主轴"写一遍，只有**像素搬运**在 `compositor.rs` / `tiled_canvas.rs` 里分叉（垂直可整块 `memcpy`，水平必须逐行）。这是一个值得 SnapClip 直接借鉴的组织方式：**`ScrollAxis` 不是一个贯穿所有类型的泛型参数，而是一组把二维问题降成一维的投影函数。**

### 3.3 坐标系与内存模型

| 层 | 表示 | 说明 |
|---|---|---|
| 单帧 | `Frame` = `RasterBuffer`（page-backed）+ `Geometry{width,height,pixel_format}` | **内部严格 packed，无 stride**；`from_strided(w,h,fmt,row_stride,bytes)` 是唯一带 stride 的入口，逐行丢 padding |
| 画布 | `TiledCanvas`：`VecDeque<Arc<CanvasTile>>` + 主轴 `start`/`end` | tile 主轴跨度 **256 px**，横轴恒为整宽 |
| 会话 | `ViewportState{ position, max_position, viewport_height, canvas_height }` | `canvas_height = viewport_height + max_position` |

**唯一的帧坐标↔画布坐标转换点是 `Stitcher::comparison_reference()`（`stitcher.rs:289-306`）**，它等于 `canvas.materialize_axis(state.position, position+viewport_extent)`。把"参照坐标"收口到一个函数，是这套实现里最值得抄的一条架构纪律。

### 3.4 `ViewportState`：union 模型（`state.rs:19-84`）

```rust
let candidate = position.checked_sub(offset);       // offset<0 == 向下滚 == Append
if candidate < 0            → Prepend(growth = -candidate, max_position += growth, position = 0)
else if candidate > max_pos → Append(position = max_position = candidate, growth = candidate - max_position)
else                        → Contained(growth = 0)
```

- `offset` 为正 = 用户回滚 = `Prepend`；`offset` 为负 = 向未探索方向滚 = `Append`。
- `Contained` 是**独立分支**：`growth == 0`，画布完全不变。
- `StitchBranch = { Append, Prepend, Contained, Skip, NoMovement }`。

**这与 `docs/19` §8.1 的 `next_pos = current_pos + signed_delta` 是同一个模型**，但 `docs/19` 缺 `Contained`（见 §8.4）。

### 3.5 每帧流水线（`stitcher.rs:308-499`）

```
validate_incoming      geometry（含 pixel_format）必须与首帧完全一致，否则 ViewportMismatch
   ↓
exact_duplicate?       previous_raw.visible_pixels_equal(incoming)
                       → Skip + exact_duplicate=true，不调 estimator、不推进 previous_raw_index
   ↓
reference 准备          仅 reference_mode == CanvasWindow 时 materialize 画布窗口
   ↓
estimator(reference, previous_raw, incoming) → MotionEstimate{offset, confidence, stage, diagnostics}
   ↓
硬门限                 offset != 0 且 |offset| <= viewport_extent * max_motion_ratio(0.6)   （闭区间）
   ↓
state.transition(offset) → Append | Prepend | Contained
   ↓
compositor 写入 / 不动画布
   ↓
assert_invariants()    canvas 主轴 extent == canvas_height；横轴未变；
                       0 <= position <= max_position；synthetic_reference.geometry 未变
```

**失败容忍（与 `docs/19` §7.2 直接冲突）**：引擎里**没有任何"单帧失败就终止会话"的机制**。错误只来自几何不匹配与 `checked` 算术；低置信度一律降级为 `NoMovement` 事件继续；`NoMovement` 时 `previous_raw = incoming` 且 `previous_raw_index = index`（测试 `non_skip_advances_previous_raw_even_without_motion` 固化）。

**scene cut 不进状态机**：`SceneCut` 只影响 estimator 内部 streak 与 `TemporalRegionModel`（连续 3 次 → `decay_toward_neutral(0.05)` + `reset()` + 返回 `stage=SceneCut`），对外只是一个 `Unmatched` 事件。

### 3.6 位移估计：ORB + tile 加权直方图投票（`estimator.rs`）

**没有 RANSAC、没有 SAD、没有 phaseCorrelate。** 候选位移来自**投票**：

`candidate_offsets`（`estimator.rs:466-514`）：
- 每个匹配对给出 `primary_delta`，`|delta| < 2` 归零（原地假设）；
- 票权 = `(1.0 - distance/64.0).clamp(0.05, 1.0)`；
- 在 `±INLIER_TOLERANCE(2)` 滑窗里累加 support；
- 排序键：support ↓、`|value|` ↑、value ↑；
- **`selected[0]` 永远是 0**（强制把"原地假设"作为第一名候选）；
- 最多保留 `MAX_CANDIDATES = 8`。

常量表（`estimator.rs:12-20`，全部第一手）：

| 常量 | 值 | 用途 |
|---|---|---|
| `LOWE_RATIO` | 0.8 | 最近邻/次近邻比检验 |
| `MAX_HAMMING_DISTANCE` | 64.0 | 描述子距离硬上限 |
| `MAX_CROSS_AXIS_DELTA` | 4 | 丢掉横轴漂移过大的匹配（隐含"滚动是纯主轴运动"） |
| `INLIER_TOLERANCE` | 2 | 投票滑窗半宽 = 内点判据 |
| `MAX_CANDIDATES` | 8 | 候选位移上限 |
| `MAX_FEATURES_PER_TILE` | 8 | 每 32 px tile 的特征配额（空间均匀性） |
| `MIN_INLIER_MATCHES` | 8 | 接受下限（内点数） |
| `MIN_INLIER_TILES` | 4 | 接受下限（**独立空间位置数**） |
| `MIN_RESIDUAL_GAIN` | 0.15 | 相对零位移的残差增益 |

**打分与置信度（两条不同的线性组合）**：

```
score      = 0.60*weighted_inlier_share + 0.25*residual_gain + 0.15*spatial_coverage   (estimator.rs:640)
confidence = 0.40*weighted_inlier_share + 0.25*spatial_coverage + 0.20*residual_gain + 0.15*margin   (:1248-1252)

accepted = raw_inliers >= 8 && inlier_tiles >= 4 && residual_gain >= 0.15 && confidence >= min_confidence(0.65)
```

> **`MIN_INLIER_TILES = 4` 与 `MIN_RESIDUAL_GAIN = 0.15` 正是 `docs/25` R17 要求 `docs/19` §7.2 补上的两个量化门限，Snow Shot 里它们是常量。**

**匹配质量保障**：
- 描述子距离用 AVX2 nibble-popcount / popcnt / 便携三档按 CPU 分派（`nearest_two`）；
- `passes_ratio` = `distance <= 64 && distance < 0.8 * second`；
- **互为最近邻 cross-check**（`mutual_observations_rust`，`rayon::join` 双向匹配），再丢 `|cross_delta| > 4` 与落在 tile 网格外的点。

**early-exit 阶梯**（`estimate`，`estimator.rs:993-1083`）：

1. geometry 不等 → `InvalidFrame`；
2. `w < 5 || h < 5` → `NoMotion @ InputTooSmall`，confidence 1.0；
3. `visible_interior_pixels_equal` → `NoMotion @ IdenticalInterior`，confidence 1.0（**零成本重复帧检测**）；
4. 同时算 `direct`（`previous_raw` vs `incoming`）与 `zero_alignment`（`motion_reference` vs `incoming`）；
5. **唯一降级路径**：`sampling.reduced() && evaluated.needs_full_resolution()` → 用 `OnceLock` 懒建的全分辨率计划 + `sampling = None` **重跑一次**。

`needs_full_resolution() = compensated.is_none() || margin <= 0.0 || 首选候选的 precise_alignment_error 不 < 1.0`（`estimator.rs:85-98`）。**这就是"降采样优先、只在困难样本上回退全分辨率"的具体实现**，与 `docs/19` §7.2 "GPU 线程回读降采样 1/4 MatchView，困难样本 1/2" 同构，但 Snow Shot 的降级粒度是**内联重跑**而不是换一条数据通路。

### 3.7 ORB：1,739 行纯 Rust，无 OpenCV

`orb.rs` 逐条照抄 OpenCV ORB 语义（注释与常量均标明来源）：

| 常量 | 值 |
|---|---|
| `SCALE_FACTOR` | 1.2 |
| `LEVELS` | 8 |
| `EDGE_THRESHOLD` | 31 |
| `PATCH_SIZE` | 31 |
| `FAST_THRESHOLD` | 20 |
| `HARRIS_K` | 0.04 |
| `HARRIS_BLOCK_SIZE` | 7 |
| `BLUR_KERNEL` | OpenCV 7-tap（用 `f32::from_bits` 精确复刻） |
| `ORB_PATTERN_BASE64` | 行内 base64 的 256 点采样模式 |

其它复刻细节：`cv_round` 用 round-half-to-even；`reflect_101` = `BORDER_REFLECT_101`；`feature_quota` 按 1/1.2 几何递减；`detect` 每层 `fast_9_16` → `filter_border` → `retain_best(quota*2)` → `harris_response` → `retain_best(quota)` → `orient` → 坐标乘 `scale_for_level`；`compute` 只对用到的 octave 做 blur。

> **这是本次研读里对 `docs/19` 最有价值的一条证据。** `docs/19:691` 把 ORB/AKAZE 放到质量降级的**最后一步**，并加了硬约束"OpenCV 不是 v1 常驻依赖，且不得进入 `snapclip-capture` 的默认依赖图"；`docs/24` §0.4 甚至把"OpenCV 作为 `snapclip-capture` 默认依赖"当成既存事实（该陈述本身是事实错误，见 `docs/25` §3）。
>
> **Snow Shot 用一个 1,739 行、零 OpenCV 依赖的纯 Rust 文件证明：ORB 可以在不引入 OpenCV 的前提下实现。** 这同时消解了 `docs/19` 的依赖顾虑、纠正了 `docs/24` 的事实错误，并给出一个可直接对照的实现规模估计。
>
> 更重要的是**它反过来支持把 ORB 提到 v1 主路径**：目前可读的、最完整的开源滚动拼接实现对这个问题选的正是 ORB + 空间加权投票，而不是 profile-SAD。见 §8.3。

### 3.8 特征的空间均衡：`MAX_FEATURES_PER_TILE = 8`

`detect_balanced_rust`（`estimator.rs:359-425`）：

```
SamplingPlan::apply → orb::detect(candidate_limit = max_features * 2)
  → 每点 score = response.max(0.001) * regions.weight_at_tile(tile)
  → 排序 score ↓ / x / y / octave / response / angle
  → 每 32px tile 最多 8 个
  → 总数封顶 max_features (2500)
  → compute 描述子，坐标经 sampling.source_coordinates 映射回源分辨率
```

**这是"三分类权重影响特征选择"的入口**：动态/固定区域的特征被打低分，滚动区域的特征被抬高。`docs/19` §7.3 的动态 mask 是在**匹配之后**排除像素；Snow Shot 是在**提取之前**给特征降权——后者更省算力，且不会因为"排除后可用像素太少"而直接 `Uncertain`。

### 3.9 三分类时序模型与乘法式权重（`region.rs`）

- 估计器用**独立的 32 px tile 网格**（`TileLayout`，`DOWNSAMPLE = 4`），与 TiledCanvas 的 256 px tile **毫无关系**。
- 相似度：SSIM `c1 = 6.5025`、`c2 = 58.5225`；`value = 0.7*ssim + 0.3*(1 - gradient_error/64).clamp(0,1)`；tile 有效下限 `expected.div_ceil(2).max(4)`。
- `SimilarityMap::between` 之后**必做 3×3 中值滤波**；`mean()` 用 `weight = 0.1 + 0.9*texture` 做纹理加权（= `direct_similarity`）。
- **权重公式原文（`region.rs:779-788`）**：

```rust
let learned = 1.0 + 1.5 * (state.scrolling - 1.0/3.0)
                - (state.fixed     - 1.0/3.0)
                - (state.dynamic   - 1.0/3.0);
let influence = (state.observations as f32 / 3.0).clamp(0.0, 1.0);
(1.0 + influence * (learned - 1.0)).clamp(0.1, 2.0)
```

→ **乘法式**：滚动区域最高 2.0×，固定/动态区域最低 0.1×；前 3 次观测线性 ramp，观察不足时保持 1/3 中性先验。

- 状态更新（`region.rs:797-833`）：`texture < 0.05` 跳过；**`let ambiguous = direct * compensated; if ambiguous >= 0.5 { continue; }`（`:814-816`）——"两边都像就不表态"**；三分类似然 `fixed = direct*(1-comp)`、`scrolling = comp*(1-direct)`、`dynamic = (1-direct)*(1-comp)`，归一化后指数滑动，`observations += 1`。
- `decay_toward_neutral(rate)`（estimator 用 0.05）、`reset()`、`summary()`（`observations < 3` → neutral）。

> **对照 `docs/19` §7.3**：`docs/19` 用的是**加法式线性加权**
> `band_score = texture + edge + temporal_stability - dynamic_penalty - sticky_penalty - scrollbar_penalty`，
> 而 Snow Shot 用**乘法式权重**（`docs/25` R16 已提出这一点）。乘法式的好处是"任何一项为 0 就整体归零"，不会被其它正项补偿；且中性先验 1/3 + 观察不足时 ramp 的设计避免了冷启动偏差。
>
> 另外 `docs/19` §7.3 要求保持"每 session 只留最近 2–4 个微型 profile/统计摘要"，Snow Shot 的对应物是**每 32 px tile 一个 `RegionState{scrolling, fixed, dynamic, observations}`**——规模是 `ceil(w/32) × ceil(h/32)` 而不是 2–4 个，但它是**固定尺寸的小状态**（每 tile 4 个 f32），不随滚动长度增长。

### 3.10 `compositor.rs`：`band_height` 与"整行覆盖"

```rust
band_height(H, shift) = (H/2).max(H/4 + shift)      // compositor.rs:8-27
```

`append_and_repaint`（`:42-77`）：`overlap = band - growth`、`old_end = H - overlap`、`incoming_start = H - band`，
行范围 `[(old, 0..old_end), (incoming, incoming_start..H)]`。

范例（`docs/Temp` 笔记里的具体例子）：`old = [10,11,12,13]`、`incoming = [100,101,102,103]`、`growth = 2`
→ `band = max(2, 1+2) = 3`、`overlap = 1`、`old_end = 3`、`incoming_start = 1`
→ `[10,11,12] ++ [101,102,103] = [10,11,12,101,102,103]`。

`prepend_and_repaint`（`:147-162`）同理 → `[100,101,11,12,13]`。

**纯行覆盖：没有 blending、没有 feather、没有质量保护，alpha 逐字节照抄。** 参照帧合成：

- `synthesize_append_reference`（`:214-244`）= `[(reference, shift..shift+keep), (incoming, H-band..H)]`；
- `synthesize_prepend_reference`（`:299-331`）= `[(incoming, 0..band), (reference, band-shift..band-shift+keep)]`；
- `*_in_place` 变体用 `copy_within` / `extend_from_slice` / `resize`，**stitcher 只在 `axis == Vertical && reference_mode == Synthetic` 时用 in_place**；
- 水平轴走 `from_column_ranges`（`:464-500`），逐行切片，**没有就地快速路径**。

### 3.11 `tiled_canvas.rs`：256 px tile + 租约 + 有界空闲池

常量（`:7-18`）：

| 常量 | 值 |
|---|---|
| `CANVAS_TILE_SPAN` | 256（主轴） |
| `CANVAS_TILE_ROWS` | 256 |
| `MAX_SPARE_TILES` | 2 |
| `MAX_SPARE_TILE_BYTES` | 8 MiB |
| `tile_capacity_limit(len)` | `len + len/8 + 64`（注释原文 "Match the raster allocator's bounded growth headroom plus its SIMD tail."） |

**没有预分配网格**：`VecDeque<Arc<CanvasTile>>`，tile 数 = `ceil(extent / 256)`。

两条设计原文（值得直接引用进 `docs/19`）：

- `:10-11` —— *"Active scrolling can replace the trailing band repeatedly. Keep only a small owner-local working set, never tiles leased by immutable snapshots."*
- `:158-159` —— *"A snapshot or cloned canvas owns its own lease. Those pixels must stay immutable; they are released by the final lease instead of recycled."*

机制：
- `take_tile_pixels`（`:131-143`）先从 spare 复用，否则 `RasterBuffer::zeroed`；
- `recycle_tile`（`:145-163`）受 `MAX_SPARE_TILES` 与 `MAX_SPARE_TILE_BYTES` **双重限制**，且必须 `Arc::try_unwrap` 成功（有租约就拒绝回收）；
- `slice_tile_in_place`（`:289-343`）**无租约时原地压缩、有租约时写时复制**；
- `Clone for TiledCanvas`（`:47-58`）只克隆 tiles，spare 置空；
- `snapshot_axis`（`:444-461`）+ `retained_range`（`:533-568`）**零拷贝**（只 Arc::clone 命中的 tile 区间）；
- `render_scaled`（`:765-830`）**最近邻且恒输出 Rgba8**，无插值。

### 3.12 参照系二态：`Synthetic` ↔ `CanvasWindow`

- `Contained` 分支**只做一件事**：`self.reference_mode = ReferenceMode::CanvasWindow;`（`stitcher.rs:472-474`）。
- 任何 `Append`/`Prepend` 之后：`reference_mode = Synthetic`（`:438` / `:469`）。
- **`NoMovement` / `Indeterminate` 不改变 `reference_mode`。**
- **双帧分离**：`previous_raw`（用于算位移）与 `motion_reference`（参与估计）是两个不同的东西。`comparison_reference()` 是唯一转换点。

> **`docs/25` R14 提出 `docs/19` 缺 `Contained` 分支与参照系二态——这里就是参考实现的完整形态。** 值得注意的是 Snow Shot 的"重锚定"机制**只有这一条**：没有任何周期性 keyframe、没有累计漂移阈值、没有 `DriftBeyondBudget`。这一点对 `docs/19` §7.4 是**反向证据**（见 §8.5）。

### 3.13 `perf.rs`：16 阶段 thread_local 计时

`Stage = { FrameFreeze, DuplicateCheck, ReferencePreparation, Grayscale, SimilarityMaps, FeatureExtraction, DescriptorMatching, CandidateScoring, Refinement, RegionUpdate, CanvasComposition, ReferenceSynthesis, PreviewScaling, PushTotal, Initialization, Reserved }`；`Snapshot{ elapsed_ns: [u64;16], calls: [u64;16] }`；thread_local；**`Scope` 含 `PhantomData<Rc<()>>` 在编译期禁止跨线程**；未启用 feature 时是零大小空 struct。

doc 原文：*"Inclusive wall-clock scopes on the calling thread. Parallel work is timed around its join, so worker CPU times are not mistaken for elapsed latency."*

> 这条"**并行工作只在 join 两侧计时**"的纪律，可以直接写进 `docs/19` §10.2 的指标定义。

### 3.14 上限：引擎无上限，**上限完全由 FFI 施加**（第一手确证）

`snow-stitch-images-c/src/lib.rs`：

```rust
const DEFAULT_MAX_OUTPUT_HEIGHT: u32  = 2_160 * 32;          // = 69,120 px          (:20)
const DEFAULT_MAX_OUTPUT_PIXELS: u64  = 3_840 * 2_160 * 32;  // = 265,420,800 px     (:67)
const DEFAULT_MIN_OVERLAP_ROWS: u32   = 48;                  // 死配置 (:68)
const DEFAULT_MIN_OVERLAP_RATIO: f32  = 0.15;                // 死配置 (:69)
const DEFAULT_ACCEPTED_HISTORY_CAPACITY: usize = 4;          // 死配置 (:70)
```

`struct StitchLimits { max_output_height: u32, max_output_pixels: u64 }`；`struct StitchConfig { axis, limits, min_overlap_rows, min_overlap_ratio, accepted_history_capacity }`。

配置校验（`:325-332`）：
```rust
min_overlap_rows == 0
  || !(0.0..=0.5).contains(&min_overlap_ratio)
  || !(1..=8).contains(&accepted_history_capacity)   // → 拒绝
```

**超限行为（`:421-432`）**：

```rust
let pixels = u64::from(width) * u64::from(height);
let output_extent = self.config.axis.primary_extent(width, height);
if output_extent > max_output_height || pixels > max_output_pixels {
    return Err(StitchError::InvalidFrame {
        message: format!("stitched output {}x{} exceeds configured limits", width, height),
    });
}
```

> **这是一条对 `docs/19` §8.4 极其重要的证据**：Snow Shot 的超限是**硬错误**——`InvalidFrame` 沿 FFI 变成本帧失败，C++ 层 `ScrollWorker::process` 里任何 push 失败都 `result.fatalError = true`（`pipeline.cpp`），而 `fatalError` 一次即终局（`capturecontroller.cpp` 的 grep 确证）。也就是说：
> **Snow Shot 撞到上限时不是"优雅截断并保留部分结果"，而是整场会话失败。**
> `docs/19` §8.4 的"部分结果保持可导出"是**超出参考实现的行为**，属于 SnapClip 的差异化设计，应在文档里明确标注为"参考实现未做到"。

### 3.15 `decisions.rs`：可序列化审计记录

```rust
StitchProgressState { viewport_position, max_viewport_position, canvas_height, processed_count, accepted_count }
StitchDecision {
    input_index, previous_raw_index, exact_duplicate, reference_mode,
    motion, confidence, accepted_offset, branch,
    before, after, growth,
    canvas_band_height, synthetic_reference_band_height,
    motion_diagnostics,
}
```

**这是 `docs/19` §10.2/§10.3 想要但没有具体形状的东西**：一个纯 `serde` 可序列化的决策记录，包含"帧序号 / 上帧序号 / 是否精确重复 / 参照系 / 位移 / 置信度 / 分支 / 前后坐标 / 生长量 / 两条 band 高度"。

### 3.16 `snow-stitch-images` 的弱点（子代理 A 读码得出，本报告认同）

| # | 弱点 | 证据 |
|---|---|---|
| **S1** | **接缝零 blending、零质量保护**，纯行覆盖，alpha 逐字节照抄 | `compositor.rs:42-77`、`:147-162` |
| **S2** | **`Synthetic` 参照系有累积漂移** —— 用当前帧位移估计结果合成参照帧，误差写回参照再影响下一帧；唯一复位是 `Contained → CanvasWindow`；**连续 Append 期间没有任何中间校正** | `stitcher.rs:438/469/472` |
| **S3** | **FFI 三个配置字段是死配置**：`min_overlap_rows` / `min_overlap_ratio` / `accepted_history_capacity` 只被赋值（`:316-318`）、区间校验（`:330-332`）、双向转换（`:1065-1067`），全文无其它读取点 | `snow-stitch-images-c/src/lib.rs` |
| **S4** | **`MotionStage::NoMatches` 不可达** —— `estimator.rs:1136-1138` 先判 `EmptyDescriptors` → FFI 的 `INSUFFICIENT_OVERLAP` 上层**永远收不到** | `estimator.rs:1136-1138`、`c/src/lib.rs:566-580` |
| **S5** | `MatchMetrics.reference_count` 硬编码为 1 | `c/src/lib.rs:610` |
| **S6** | `retain_best` 用 `select_nth_unstable_by` 取第 count 大 response 当阈值再 retain，**并列时实际数量会超过配额** | `orb.rs:680-697` |
| **S7** | **水平轴明显更慢** —— 只有垂直轴能整块 `memcpy`，水平必须逐行 | `compositor.rs:464-500`、`tiled_canvas.rs:654-763` |
| **S8** | `render_scaled` **最近邻 + 恒 Rgba8**，缩 3–30 倍时严重 aliasing | `tiled_canvas.rs:765-830` |
| **S9** | `SimilarityMap::between` 每次都做 median filter，且 `evaluate` 里**每个候选都新建一张**（最多 8 张） | `region.rs:670-704`、`estimator.rs` |
| **S10** | TiledCanvas 无段/稀疏元数据，固定底栏重复内容在**画布层无去重** | `tiled_canvas.rs` |
| **S11** | **两个 crate 的 `.rs`/`.h` 全文 `TODO\|FIXME\|HACK\|XXX\|todo!\|unimplemented!` 零命中** → 遗留问题只能靠读代码挖 | grep |

---

## 4. `snow-memory`：大光栅的显式 OS 页所有权

`refer/snow-apps/snow-crates/crates/snow-memory/`（`Cargo.toml` 348 B、`src/lib.rs` 27,131 B、`src/array.rs` 5,313 B、`src/pages.rs` 9,320 B、`README.md` 2,167 B）。描述：**"Explicit OS-page ownership for large CPU raster buffers"**。

`README.md` 原文要点：

> `RasterBuffer` 拥有已初始化的 CPU 光栅字节；**≥ 1 MiB 的缓冲用整块 OS 区域（`VirtualAlloc` / Mach VM / Linux 匿名 `mmap`），drop 时按原始分配容量释放；更小的用 `Vec`。Windows 在可用时保留特权大页分配，失败则退回普通 OS 页。**

```rust
pub const MIN_PAGE_BUFFER_BYTES: usize = 1024 * 1024;   // src/lib.rs:14
```

API：`new` / `try_with_capacity`(`io::Result`) / `with_capacity` / `try_zeroed` / `zeroed` / `len` / `is_empty` / `capacity` / `is_page_backed` / `try_reserve` / `resize` / `resize_for_overwrite` / `truncate` / `clear` / `extend_from_slice` / `into_vec` / `as_slice` / `as_mut_slice`；`impl Deref/DerefMut/AsRef<[u8]>/From<Vec<u8>>/From<&[u8]>/Clone/Eq/Serialize`。

README 原文：

> *"Keep the owner throughout a raster pipeline. Slice consumers borrow it directly; `Arc<RasterBuffer>` supports sharing and mapped copy-on-write via `Arc::make_mut`. `clear`/`truncate` retain capacity for active pools."*
> *"Fallible constructors return allocation errors instead of silently falling back to the heap."*

`RasterArray<T>` 对定长数值光栅工作区（模糊中间层、像素索引网格）套用同一所有权策略，其元素 trait 是 sealed。

**对 SnapClip 的意义**：`docs/19` §8.4 在讲"150 MP 不能 materialize 为连续 BGRA（约 600 MB）"，`docs/19:876` 明确要求"CanvasStore 必须 tile 化、流式写盘，内存只保留有界 LRU"。Snow Shot 的答案有两层：**tile 化**（`tiled_canvas.rs`）**之上**再加一层**页后备分配**（`snow-memory`），让每个 tile 的分配/释放绕过堆分配器、避免碎片与 `Vec` 的容量抖动，并在 Windows 上尝试大页。**SnapClip 若要 tile 化画布，这一层是配套的，且与 `docs/19` 的"可维护性/简洁性低于正确性与性能"的优先级一致。**

---

## 5. `snow_shot` C++/Qt 上层管线

### 5.1 执行体拓扑：5 个

| 执行体 | 位置 | 职责 |
|---|---|---|
| GUI 线程 | — | 交互、缩略图、hover、导出触发 |
| `snow-shot-scrolling-capture`（Qt QThread） | `pipeline.cpp:703` | Producer：拥有 capture source、节拍、背压 |
| （Producer 内部再起 1 个 `std::thread`） | `pipeline.cpp:483` | `consume()`：把捕获帧推给 stitch |
| `snow-shot-scrolling-stitch`（Qt QThread） | `pipeline.cpp:704` | Worker：调 `snow_stitch_session_push_owned`、产出 preview/快照 |
| Rust capture worker | `snow-capture` 内部 | 平台捕获、转换、读回 |

> 注意：`docs/19` §4.3 的线程预算表是 **4 个**（overlay 消息线程 / GPU 线程 / scroll driver / export worker），而参考实现实际是 **5 个**（多出 Producer 内部那个 `std::thread`）。多出来的原因见 §8.7。

### 5.2 跨线程协议：`LatestBridgeMailbox`（容量 2，无 Condvar）

`latestbridgemailbox.h:27-89`：mutex 保护，`latest-win` 覆盖式合帧。
- `publish` 在已占用时 `m_latest.emplace()` 覆盖并返回 `false`；
- `hasPendingCapacity()`（`:84-89`）被 `pipeline.cpp:593` 当**背压信号**用：Producer 发现没有 pending 容量就降速；
- `receive(100)` 超时 **100 ms**（`pipeline.cpp:526`）是链路里**唯一的轮询/停止延迟来源**。

**这是一个明确的缺陷**（子代理 B 确证）：

- `pipeline.h:44` 的接口注释写 *"`stop()` must wake a blocked receive()"*；
- 实际实现里 `snow_capture_stream_stop` 只 `stop_flag.store(true, Ordering::Release)`（`snow-crates/crates/snow-capture/src/streaming.rs:350-352`）；
- 队列只在 `close()` 时 notify（`snow-crates/crates/snow-core/src/stream_queue.rs:132/150`），而 `close` 发生在消费者 join 之后（`pipeline.cpp:488-498`）。

→ **`stop()` 不能唤醒阻塞的 `receive`，停止延迟被 `receive(100)` 的 100 ms 超时兜底。** 这是"注释承诺 > 实现"的典型，SnapClip 若照抄语义必须在停止路径上显式唤醒（`docs/19` §4.1 要求 `cancel()` 幂等且"取消不 join 线程"，正好覆盖这一点）。

### 5.3 自适应采样节拍（`adaptivescrollingcapturecadence.h`，179 行，已全文读）

```cpp
struct AdaptiveScrollingCaptureCadenceConfig {
    int    minimumFps        = 1;
    int    maximumFps        = 30;
    int    initialFps        = 30;
    double capacityHeadroom  = 1.25;
    double ewmaSampleWeight  = 0.25;
    int    recoverySamples   = 4;
    std::uint32_t pressureQueueDepth = 2;
};
enum class LimitingStage { Warmup, Capture, Stitch };
using Clock = std::chrono::steady_clock;
```

- `recordCapture(Duration)` / `recordStitch(Duration)` → `updateCost`（latest + EWMA，权重 0.25）→ `updateTarget()`。
- `sustainableFps() = clamp(floor(1000.0 / (stageCostMs * capacityHeadroom)), minimumFps, maximumFps)`，其中 `stageCostMs = max(captureEWMA, captureLatest, stitchEWMA, stitchLatest)`。
- **下降瞬时、恢复迟缓**：`sustainable < target` 直接 `target = sustainable; recoverySampleCount = 0`；恢复必须连续 `recoverySamples = 4` 个健康样本才 `target = min(sustainable, target + 1.0)`。
- `recordStreamPressure(queueDepth, droppedFrames)`：无丢帧且 `queueDepth < pressureQueueDepth(2)` 直接返回；否则 `target = max(minimumFps, floor(min(target * 0.75, sustainableFps())))`。
- `limitingStage()` 是比较 capture 与 stitch 的 `max(ewma, latest)`。
- `setMaximumFps(int)` 把 `maximumFps` clamp 到 `[minimumFps, kAbsoluteMaximumFps = 30]`。

闭环位置在 `pipeline.cpp:503-520`：

```cpp
recordStitch(...) → recordCapture(stats.captureLatencyNs)
  → recordStreamPressure(bufferedFrames, droppedFrames)
  → m_source->setTargetFps(lround(m_cadence.fps()));
```

原生流参数（`screenshotscrollingnativesource.cpp:121-131`）：`target_fps=30 / min_fps=1 / buffer_depth=3 / max_consecutive_errors=30 / capture_retry_count=1 / adaptive_fps=1`。

> **这就是 `docs/19` §6.6 "cadence 控制"的完整参考实现**，也是 `docs/19:132` 那条"Snow Shot cadence | EWMA 成本、队列压力降速、连续健康样本才恢复"的原文来源。**它验证的是"自适应帧率"，而不是 `docs/19` §6.5 的"闭环步长"。**

### 5.4 自动滚动：固定 120 单位、固定间隔的**开环**注入

`ScreenshotScrollingAutoScroller`（`screenshotscrollingautoscroller.h`，94 行，已全文读）：

```cpp
using ScrollStep = std::function<void(const QRect&, const QPoint&)>;   // (selection, wheelDelta)
m_timer.setTimerType(Qt::PreciseTimer);
m_timer.setInterval(kScreenshotScrollingAutoScrollIntervalDefault = 200);
// on timeout:
if (m_timer.isActive())
    m_scrollStep(m_selection, m_mode == Horizontal ? QPoint(120, 0) : QPoint(0, -120));
```

`setIntervalMs` clamp 到 `[128, 1000]`。

`scrollingstepinput.h`（17 行，已全文读）：

```cpp
inline std::optional<QPoint> scrollingStepDelta(const QString& direction) {
    "up"    → QPoint(0,  120);
    "down"  → QPoint(0, -120);
    "left"  → QPoint(-120, 0);
    "right" → QPoint( 120, 0);
    else    → std::nullopt;
}
```

> **注入量恒为一个 `WHEEL_DELTA`（120），间隔恒定 200 ms，没有任何基于实测位移的反馈。**
> `docs/19` §6.2 的 `WheelDriver`（v1 只向前）与本实现在这一点上一致；但 `docs/19` §6.5 的"闭环步长"在参考实现里**不存在**。见 §8.6。

### 5.5 滚动目标定位与注入传输（`platform/windows/scrollinput.cpp`）

这是 `docs/25` R15 引用的那份实现，本次重新核对确认其形态：

1. `WindowFromPoint` 拿目标窗口；
2. `ScreenToClient` 把物理屏坐标转成客户区坐标；
3. **`ChildWindowFromPointEx` 逐层下沉到子 HWND**（Chromium 的 render widget host 就是子窗口）；
4. `PostMessageW(target, WM_MOUSEWHEEL, MAKEWPARAM(0, delta*WHEEL_DELTA), MAKELPARAM(pt.x, pt.y))`。

`SCROLLING_DIAGNOSTICS.md:61-66` 的 `wheel_dispatch.status` 五态：

| status | 含义 |
|---|---|
| 0 | `PostMessageW` 成功 —— **注意：只表示投递成功，不表示目标应用处理了** |
| 1 | 空 selection 或 delta 为 0 |
| 2 | 没有合格的候选目标窗口 |
| 3 | 坐标转换失败 |
| 4 | `PostMessageW` 失败（如 access denied） |
| 5 | 平台不支持 |

**为什么 `PostMessage` 可行而 `SendInput` 会被 UIPI 挡**：`docs/25` R15 已用 Microsoft Learn `winapp-cli` 原文确证——`post-message` *"is HWND-targeted and bypasses UIPI"*，`send-input` *"goes to whatever window is foreground and is blocked by UIPI"*。snow_shot 是这一结论的**生产实现证据**。

**与 PixPin 的对照（`docs/25` §1.2 ⑦）**：PixPin 是**两条并列路径**（`SimulateMouseScroll` 纯 `SendInput` 用于自动滚动；`SimulateMouseWheel` 纯 `PostMessage` 用于定点发一次滚轮），互不降级；Snow Shot 只走 `PostMessage` 一条。**两者都没有"SendInput 失败退 PostMessage"的降级链——这条对 SnapClip 是超出两个竞品的增强。**

**Snow Shot 相对 PixPin 的一个改进**：PixPin 的 `PostMessage` 路径在 `WindowFromPoint` 返回 NULL 时**直接放弃**（只打日志）；Snow Shot 有显式的 status 枚举可诊断。

### 5.6 预览：6 态 `StitchChange` + `replacedPreviewRows`

Worker 产出 6 种 `ScreenshotScrollingStitchChange`，直接驱动 `thumbnailwidget.cpp:186-272`：

| 事件 | 动作 |
|---|---|
| `Initial` / `Replaced` | `replacePreview` |
| `Appended*` | `discardPreviewBack(replacedPreviewRows)` + `appendPreview` |
| `Prepended*` | `discardPreviewFront(...)` + `prependPreview` |

`replacedPreviewRows` 的注释原文（`pipeline.cpp:343-383`）：

> *"Refresh the splice overlap and absorb all scale rounding into this small edge patch so the retained preview tiles never drift in height."*

`emitPreview`（`:344-383`）里：

```cpp
const bool append  = event == EXTENDED_BOTTOM || event == EXTENDED_RIGHT;
const bool prepend = event == EXTENDED_TOP    || event == EXTENDED_LEFT;
overlapSourceRows  = max(0, deltaRows - max(0, addedRows));
```

> **这正是 `docs/19` §5.6 的 `PreviewPatch { replace, append, prepend }` 的参考实现**，但参考实现是 **6 态**（Append/Prepend 各分上下/左右 = 4 态 + Initial/Replaced），且解决了 `docs/19` 没提的一个真问题：**缩放取整漂移**——把重叠行的重绘集中到边缘小 patch 上吸收，使已保留的预览 tile 永不发生高度漂移。

### 5.7 缩略图控件：128×256 瓦片 + 二分可见区

`thumbnailwidget.cpp:23-30` / `:451-491`：

- 交叉轴固定 **128 px**，主轴瓦片跨度 **256 px**，最大预览主轴 **640 px**；
- 用 `std::lower_bound` 定位可见瓦片区间后**只重绘局部脏区**；
- 手柄命中半径 9 / 线宽 2 / 厚 4 / 宽 30，颜色 `#faad14`；
- trim 自动滚屏边距 18、步长 20；
- 高亮带高度 `min(一个捕获视口轴长, 当前总长)`；
- 遮罩色：压暗 `(0,0,0,132)`、trim 遮罩 `(0,0,0,178)`、hover 框 `(22,119,255,64)`。

> `docs/19:134` 那条"固定 128 px 交叉轴缩略图；沿滚动轴以 256 px tile 增量追加/替换；视口高亮、裁剪边界和 hover 映射只更新局部脏区"**属实**。
>
> **注意这里有两套不同的 tile 尺寸，不要混淆**：
> - **画布 tile**：256 × 256（`tiled_canvas.rs`），横轴整宽；
> - **预览 tile**：交叉轴 128、主轴 256（`thumbnailwidget.cpp`）；
> - **估计器 tile**：32 × 32（`region.rs`）。
> `docs/19` 的 512×512 是**第四套**。见 §8.8。

### 5.8 Hover 预览：单飞 + 四重过期校验 + 暂停采集

`scrollinghoverpreview.h`（138 行，已全文读）：

```cpp
struct Context {
    std::function<void(std::function<void()>)>                             pause;
    std::function<bool(const QRect&, std::function<void(QImage)>)>         request;
    std::function<void(const QImage&, bool)>                               present;
    std::function<void()>                                                  clear;
    std::function<void()>                                                  resume;
};
```

API：`setEnabled(bool enabled, bool hoverEnabled = true)` / `setHoverRect(const QRect&, bool cropping = false)` / `contentChanged()` / `reset()` / `paused()`。

**四重过期校验**（回调里）：

```cpp
if (!receiver || serial != receiver->m_requestSerial) return;
if (!(receiver->m_paused && receiver->m_ready
      && epoch == receiver->m_epoch
      && contentRevision == receiver->m_contentRevision
      && rect == receiver->m_desired)) return;
```

- **先 pause 采集、等 ack（`m_ready = true`）再发 request**（`reconcile()`）；
- `m_pending` **单飞**；
- `contentChanged()` 只 `++m_contentRevision` 并清 `m_completed`。

类注释原文：

> *"Owns the temporary pause and a bounded, latest-position image request queue. Callbacks run on the owner's GUI thread; source shutdown and extraction remain asynchronous."*

> **这是 `docs/25` R6（预览与 revision）的参考实现，也是 `docs/19:136` 那条的原文来源。** 关键设计：
> ① **`epoch` 与 `contentRevision` 是两个不同的版本号**（会话代次 vs 内容修订）——`docs/19` §5.6 只提到"stale revision"，应补"两个版本号维度"；
> ② **hover 读取前必须"先暂停采集并等 ack"**，否则 `materialize` 会与正在写入的 tile 竞争。这条 `docs/19` 没写。

### 5.9 选区移动：只沿捕获轴

`scrollingselectionmovement.h`（50 行，已全文读）：

```cpp
bool begin(ScreenshotScrollingRecognitionMode requestedAxis,
           ScreenshotScrollingRecognitionMode captureAxis,
           QRect selection, QPoint pointer)
{
    if (m_active || requestedAxis != captureAxis || selection.isEmpty()) return false;
    ...
}

// update(QPoint pointer, QRect bounds)：只沿捕获轴移动
// Horizontal: result.moveLeft(clamp(result.x() + delta.x(), bounds.x(),
//                                     max(bounds.x(), bounds.x() + bounds.width() - result.width())));
// Vertical:   result.moveTop (clamp(result.y() + delta.y(), ...));
```

注释原文：*"All coordinates are physical canvas pixels; the desktop may span multiple monitors."*

> **与 PixPin 的"捕获态只能沿截图方向移动"（`docs/25` §1.1 界面元素 ③）完全同构。** `docs/19` §3.4 的用户操作要求里应明确这一约束（`docs/25` D3 的 Move Button 问题即源于此）。

### 5.10 输入隔离：三层，且**不隐藏 overlay**

这是本报告里**与 `docs/19` §5.1 分歧最大的一处**。

**第一层：输入穿透只翻 `WS_EX_TRANSPARENT` 一位**（`windowchrome.cpp:259-292`）

- 要求窗口已带 `WS_EX_LAYERED`；
- 返回旧值（可恢复）；
- 开启时顺手 `ReleaseCapture()`；
- 调用点 `overlaywindow.cpp:565-574`，注释原文：*"do not hide/show the overlay or ask Qt to rebuild its window flags"*。

**第二层：视觉/输入空洞 = Qt `setMask(QRegion(rect()) - QRegion(hole))`** → `SetWindowRgn`（`overlaywindow.cpp:887-911`）

- **幂等比较**，空区域必须 `clearMask()`；
- `m_captureFrameMargins = QMargins(0, 1, 0, 0)`（`:117`）。

**第三层：捕获排除 = `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`**（`windowchrome.cpp:241-257`）

- Windows 下 `captureWindowId` 恒为 `nullopt`（`:533-537`）；
- 排除对象 = overlay + scrollingThumbnailWindow + toolbar（`capturecontroller.cpp:353-371`）；
- **可被设置项跳过**：`captureUiInScrollingScreenshot()`（`screenshot/capture_ui_in_scrolling_screenshot`，见 §5.12）或 `--e2e-allow-overlay-capture`。

**现场自检探针**（`overlaywindow.cpp:327-368`）：每次启动查询 `GetWindowRgn` / `PtInRegion` / `GetWindowDisplayAffinity` / `GetCapture()` / **`SystemParametersInfoW(SPI_GETMOUSEWHEELROUTING)`**，另有 `full_hole` / `mask_empty` 字段。

> **`SPI_GETMOUSEWHEELROUTING` 这条是重要发现**：它是 `PostMessageW(WM_MOUSEWHEEL)` 能生效的**系统前提**（Windows 10 1703 引入的 "Scroll inactive windows when I hover over them" 路由设置）。SnapClip 若走注入路线，应在能力探测里读这个值——`docs/19` §4.1 的能力探测清单里没有。

> **对 `docs/19` §5.1 的结论**：`docs/19` 的方案是"**隐藏 overlay + 独立 controller HWND + `WDA_EXCLUDEFROMCAPTURE` 降级层**"。Snow Shot 的方案是"**overlay 保持可见与可交互，用 `WS_EX_TRANSPARENT` 一位做输入穿透 + `setMask` 挖洞 + `WDA_EXCLUDEFROMCAPTURE` 排除**"，并且**把是否排除做成用户设置**。
>
> 两者解决同一个问题（"用户要能操作、但 overlay 不能入镜"），但**隐藏 overlay 会连带丢掉 PixPin 的实时尺寸读数、方向下拉、贴图/保存/复制按钮（`docs/25` §1.1 元素 ②–⑨）**。Snow Shot 的可见 overlay 方案保留了这些。**这是 `docs/19` §5.1 应当重新评估的一处。**

### 5.11 暂停/恢复：三段屏障 ack

`setExportPaused(true)` → `paused = exportPaused || movement.active() || hoverPaused` → `pipeline->pause(generation, ack)`（`capturecontroller.cpp:564-572` / `:635-672`）。

`pause` 是**三段屏障**（`pipeline.cpp:864-902`）：

1. producer 停源并 join；
2. stitch 线程屏障；
3. 回到 GUI 线程才 `mailbox->reset()` 并 `acknowledged()`。

**ack 的语义 = 源已停 + 所有已提交结果已投递到 GUI。** 两个版本号分工（`pipeline.cpp:850-863/889/1010-1012`）：

- `generation` —— 管会话/轴；
- `controlRevision` —— 管同一会话内的 pause/resume/reset。

### 5.12 快照、导出与所有权移交

- `requestSnapshot(top, bottom)` = worker finalize 后取裁剪成品（`pipeline.cpp:968-986` / `:250-277`）。**交付时不校验 `generation`，是隐患。**
- `requestViewportPreview(start, end)` = 不 finalize 的逐行拷贝，越界 `padded.fill(Qt::black)` 补黑；`generation` / `controlRevision` 变化则回 `null`（`:988-1018` / `:279-335`）。
- **所有权移交靠"清指针不清标志"**（`scrollingsnapshotrequest.h`，53 行，已全文读）：

```cpp
begin(Completion) → 返回 [accepted = m_pending, completion](snapshot) {
                        if (std::exchange(*accepted, false)) completion(...);
                    }
cancel()  → *m_pending = false; m_pending.reset();
detach()  → m_pending.reset();          // ← 不置 false
```

注释原文：

> *"Capture owns cancellation until export detaches the accepted request. The completion then owns its lifetime, independently of capture stop/restart. Requests and completions run on the capture controller's thread."*

→ 已接受的 completion **能活过 capture 的 stop/restart**。导出侧调用顺序是 `screenshotcontroller.cpp:2921`：**先 `detach` 再 `stopScrollingCapture(false)`**。

- 导出池 `Priority::Foreground` + 可选 Smooth 缩放（`:6362-6390`），被拒时**立刻** `setExportPaused(false)`。
- `requestTrimmedSnapshot` 有 `(generation, trim.top, trim.bottom)` 缓存 + `QTimer::singleShot(0)` 异步交付（`:486-526`）。

### 5.13 其余数值

| 项 | 值 | 位置 |
|---|---|---|
| stitch 帧池容量 | 6 | `pipeline.cpp:469` |
| 首帧预览看门狗 | 5000 ms（**只记日志，不终止**） | `screenshotscrollingcapturecontroller.cpp:140` |
| 诊断首报/周期 | 首个 5 s → 之后每 30 s | `screenshotscrollingdiagnostics.h:39-44` |
| 合成事件退避 | `min(1000, 50 << min(attempt, 5))` ms | `screenshotscrollingcapturecontroller.cpp:599-603` |

### 5.14 两个缺失

- **C++ 控制器层没有任何尺寸上限、没有任何"到达终点"的自动判定。** 全文件 grep `limit` / `maximum` / `max_canvas` / `boundary` / `exhaust` **零命中**；`end` 只命中 `endSelectionMove` / `setTrimRange(start, end)` / `end > extent`。**用户自己决定何时停。**
- **`fatalError` 一次即终局**（grep 确证 `:178-179, :216-218, :231, :239-241, :433, :442, :545-548, :582-583`）：只在 stitch 失败与 `Kind::Error` 时置位。
- **整个 `snow_shot/src` 无滚动截图相关 `TODO`/`FIXME`/`HACK`**（唯 7 处命中全是 `XXXXXX` 临时文件名模板）。

---

## 6. `snow-capture`：捕获层（Snow Shot 与 `docs/19` 差异最大的一层）

### 6.1 两条后端优先级——**窗口目标也优先 WGC**

| 目标 | 优先级 | 位置 |
|---|---|---|
| 显示器 | `[DxgiDuplication, WindowsGraphicsCapture, Gdi]` | `snow-capture/src/backend.rs:103-108` |
| **窗口** | **`[WindowsGraphicsCapture, DxgiDuplication, Gdi]`** | `snow-capture/src/platform/windows/mod.rs:68-72` |

判据（`mod.rs:74-83`）：`if policy_is_explicit { policy.normalized_priority() } else { DEFAULT_WINDOW_AUTO_PRIORITY.to_vec() }`。

> **`docs/19` §4.1 的首选是 `CreateForWindow(hwnd)`、显示器仅作降级——与参考实现一致。**

### 6.2 降级机制：单次捕获调用内部的即时重试

- `mod.rs:260-308` 对 `attempt_order()` **逐个即时重试**；
- 失败者 `discard_candidate`，下次 `ensure_candidate` 重建（`mod.rs:166-186`）；
- `preferred` 只在成功后改写（`mod.rs:211-216`）；
- Snapshot 另有 `SNAPSHOT_ACQUISITION_BUDGET = 750 ms`（`mod.rs:67`）。

**可降级错误白名单是显式的**（`mod.rs:637-647`）：

```rust
matches!(error, BackendUnavailable | UnsupportedFormat | AccessLost
                | Timeout | WorkerDead | Platform)   // → 继续下一个后端
// InvalidTarget | InvalidConfig | BufferOverflow | Canceled | PermissionDenied  → 直接整体失败
```

全部失败 → `all_backends_failed(target, errors)` = `BackendUnavailable("all screenshot backends failed for {target}: …")`。

> 这张白名单值得**逐条抄进 `docs/19` §4.1**：`docs/19` 只说"仅 ① <Win10 1903 ② 建 item 失败 ③ 运行时拒绝才降级"，粒度不够，且没说哪些错误**不该**降级（例如权限拒绝直接失败比反复重试更诚实）。

### 6.3 能力探测是假的

`src/capabilities.rs:37-59` 非 macOS 分支只有：

```rust
let supported = cfg!(windows);
hdr_capture: false,        // 写死
window_enumeration: false, // 写死
```

**不探测** `IGraphicsCaptureSession2/3`、**不探测** `IsBorderRequired(false)` / `IsCursorCaptureEnabled(false)` 可用性、**不探测** WDA。

> `docs/25` R12 要求 `docs/19` §4.1 补 `IGraphicsCaptureSession2/3` 可用性探测。**参考实现没做这件事，所以 R12 是 SnapClip 的净增量，不是"照抄参考实现"。** 这一点必须在 `docs/19` 里写清楚，避免把"参考实现没做"误当成"不需要做"。

### 6.4 WGC 会话细节

| 项 | 值 | 位置 |
|---|---|---|
| 唯一的准入检查 | `GraphicsCaptureSession::IsSupported()` | `wgc.rs:151-164` |
| 捕获项创建 | 只有 `CreateForMonitor` / `CreateForWindow` | `wgc.rs:133-140` / `:142-149` |
| **没有** `CreateFromVisual` | 无自绘 HWND 路径 | — |
| 帧池 | `Direct3D11CaptureFramePool::CreateFreeThreaded(device, fmt, WGC_FRAME_POOL_BUFFERS, pool_size)` | `wgc.rs:624-631` |
| `WGC_FRAME_POOL_BUFFERS` | **3** | `wgc.rs:60-68` |
| 像素格式 | HDR 转换开 → `R16G16B16A16Float`；否则 `B8G8R8A8UIntNormalized` | `wgc.rs:619-623` |
| **会话选项** | `let _ = session.SetIsCursorCaptureEnabled(false);`<br>`let _ = session.SetIsBorderRequired(false);` **错误被丢弃** | `wgc.rs:636-637` |
| 脏区域协商 | 先 `SetDirtyRegionMode(ReportOnly)`，成功记 `dirty_regions_supported`；`BaselineForOrdered` 时升级 `ReportAndRender` | `wgc.rs:638-645`、`:1002-1029` |
| 熔断 | `WGC_ORDERED_FAULT_LIMIT = 3`；`UnsupportedContract` 立即 `disabled = true` | `wgc.rs:486-534` |
| 帧通知 | `crossbeam_channel::bounded(1)` + `FrameArrived` 回调 | `wgc.rs:647-662` |
| 传输队列 | 容量 32 | `wgc.rs:648` |
| 超时 | `WGC_FRAME_TIMEOUT = 250 ms`；`SNAPSHOT_FRESH_WAIT = 2 ms`；`CONTINUOUS_FRESH_WAIT = 1 ms`；worker 启动 10 s / 快照 250 ms | `wgc.rs:60-68` |
| 尺寸变化 | **只 `frame_pool.Recreate(...)`，不重建会话** | `wgc.rs:1065-1092` |

> **参考实现也调了 `SetIsBorderRequired(false)` 与 `SetIsCursorCaptureEnabled(false)`（`docs/25` R12 的两条），但把错误直接丢弃、也不做能力探测。** `docs/19` §7.2 第 5 步"光标由 `ValidMask` 排除"应改为"会话选项层禁用光标捕获，接口不可用才退回掩码"——参考实现正是这个顺序，只是它没写降级分支。

### 6.5 空脏矩形语义（正确性关键点）

`wgc/update.rs:68-107 classify()`：

```rust
dirty_region_count == 0
  → 仅当 (timestamp_hns != 0 && timestamp_hns == last_timestamp_hns) 才判 Duplicate
  → 否则判 Resynchronize          // "空脏矩形 + 新时间戳" 视为不一致
```

> **这条判据是"不把丢帧误判成静止"的关键**。`docs/19` §4.4 只说"`Dropped` 不得当 `NoMovement`"，参考实现给出了具体做法：**同时比较时间戳**。

### 6.6 滚动时只按露出的条带应用脏矩形会出错

`wgc/update.rs:309-320` 逐脏矩形 `CopySubresourceRegion`。测试 `update.rs:697 bottom_strip_alone_cannot_reconstruct_a_scroll` 断言：

```rust
assert_eq!(&malformed[..(height - rows) * width],
           &patterned_frame(width, height)[..(height - rows) * width]);
```

> **这是一个直接可以照搬的测试思路**：证明"只补条带"会留下错误，从而说明"滚动必须整体重建或正确处理 move rect"。

### 6.7 ★ 最有价值的单点发现：DXGI `GetFrameMoveRects` 位块搬移重建

`duplication.rs:849-891`：

```rust
duplication.GetFrameMoveRects(...)   // → extract_move_rects
```

应用：

```rust
duplication.rs:585-642 apply_move_rects_to_frame(frame, move_rects, dst_origin_x, dst_origin_y)
// 全部 checked_add 校验，越界即 BufferOverflow
```

启用判据 `duplication.rs:1896-1909 should_try_screenshot_move_reconstruct(...)`；使用点 `duplication.rs:2550-2552` 与 `:3883-3884`。

> **这是整个 snow-apps 仓库里与"滚动截图"语义最贴近的一处 API 用法。** DXGI Desktop Duplication 的 `GetFrameMoveRects` 返回的是**操作系统告诉我们"桌面内容移动了多少像素"**。对滚动截图而言，这把"**估计位移**"变成了"**读位移**"。
>
> 限制：它只在 **DXGI Duplication（显示器目标）** 上可用，**WGC（窗口目标）没有对应接口**。所以它对 `docs/19` 的"窗口级 WGC 为主"路线不能直接套用，但有两个实际用途：
> 1. 当捕获目标退化为显示器时（跨屏窗口、WGC 不可用），可以启用 move rect 重建，把这一档的位移精度提到接近无损；
> 2. 作为 `docs/19` §11.2 **夹具/验证的独立参照量**：在显示器级夹具上同时采集"注入的位移"与"系统报告的 move rect"，用它校验 `ShiftMatcher` 的输出。
>
> **注意参考实现本身并不知道这是滚动截图**——它只是把 move rect 当作通用的"屏幕内容移动"优化，`snow-capture` 里 grep `scroll|stitch` 只命中注释与测试辅助函数。**这条机制是 SnapClip 可以主动利用、而参考实现没有利用的。**

### 6.8 DXGI 窗口捕获的硬限制

`duplication.rs:4374-4378`：

```rust
if !rect_within_rect(win_rect, &self.current_monitor_rect) {
    return Err(CaptureError::BackendUnavailable(
        "DXGI window capture requires the window to fit within one monitor".into()));
}
```

→ **跨显示器窗口必然落到 WGC。**

另：`IsWindow` / `IsIconic` 失败 → `InvalidTarget`，**不降级**。

### 6.9 读回：3 slot + 三元组寻址 + 懒提交

`wgc/readback.rs`：

| 常量 | 值 |
|---|---|
| `READBACK_SLOT_COUNT` | 3 |
| `QUERY_WAIT_TIMEOUT` | 250 ms |
| `QUERY_SPIN_POLLS` | 16 |

- `ReadbackPipeline { slots, next_slot, target }`，用 **`(epoch, generation, target)` 三元组精确匹配**（测试 `readback.rs:853 delivered_generation_requires_exact_target_and_parent`）；
- **按需接线**：`wgc.rs:1094-1108 prefetch_current()` 里 `if self.readback.target().is_none() { return Ok(()); }`；
- **最省 CPU 路径**（`readback.rs:330-396`）：`target_unchanged = metadata.ordered_delta && history_matches_parent && metadata.dirty_rects.is_empty()` → `is_duplicate = true`，**零转换**；
- 否则 `map_dirty`（`readback.rs:460-515`）：`if converted == 0 { self.map_complete(...) }` 整体回退。

`region_pipeline.rs`：自适应自旋 + `ID3D11Query` 同步 + 3 slot 环（`REGION_SPIN_* = 4/2/64/4`），`query_signaled(..., strict)` 里 **WGC 额外要求 `data != 0`**。

**DXGI dirty-copy 三档量化阈值**（`duplication.rs:135-142`）：`max_rects = 192 / gpu = 64 / low_latency = 8`，面积阈值 `70% / 45% / 18%`。

### 6.10 脏矩形越界是"拒绝"不是"夹紧"

`wgc/update.rs:397-416 exact_rect` + `:418-453 extract_exact_dirty_rects`：

- `regions.Size() > MAX_ORDERED_DIRTY_REGIONS (65_536)` → `DirtyRegionFailure::Invalid`；
- 按 `DIRTY_REGION_BATCH = 64` 分批 `GetMany`。

### 6.11 帧池溢出：两个相位处理不同

- `wgc/transport.rs:16-72 BoundedFrameQueue::push`：满时 `pop_front()` 丢最旧并置 `overflowed`；
- `DrainPolicy::CompleteLatest`：静默只留最新（`wgc.rs:925-934 coalesce_complete_snapshot`）；
- `Ordered`：overflow 当**契约破坏**触发 `resynchronize()`（`wgc.rs:829-843`）。

### 6.12 `content_generation` 与 `is_duplicate` 是独立的两个信号

`frame.rs:64-66` 注释原文：

> *"Stream-local image generation, unchanged for duplicates. Unlike a duplicate flag, this survives dropped or intentionally superseded frames."*

实现（`streaming.rs:820-825`）：

```rust
if *generation == 0 || !frame.metadata.is_duplicate {
    *generation = generation.saturating_add(1);
}
```

帧回收：`frame.rs:521-535 FrameRecycleSender::recycle` 用 `let _ = self.tx.try_send(frame);`（**满即丢弃、绝不阻塞生产者**）；`CapturedFrame` 在 `impl Drop` 里 `Arc::try_unwrap`，最后一个引用时才把 `Frame` 还给回收池。`frame.rs:504/510 ensure_rgba_capacity` 重置时同时作废 `content_generation` 与 `is_duplicate`。

> **`content_generation` 这个概念值得直接引进 `docs/19`**：`docs/19` §4.4 的帧生命周期是 `Requested → InFlight → Dropped → Arrived → MatchViewReady → Accepted/Rejected`，但**"被丢弃/被顶替"只影响生命周期，不影响"内容是第几代"**。参考实现把这两件事分开，使 `Dropped` 之后的帧仍能与"上一张真正收到的帧"正确配对——这正是 `docs/19` §4.4 想解决但只用生命周期表达的问题。

### 6.13 其它

- `src/streaming.rs`：有界队列 + 双通道 + 自适应帧率；`stop` 的唤醒缺陷见 §5.2。
- `platform/windows/display_change.rs:277-284`：真实 `WM_DISPLAYCHANGE` 监听，含坑注释 `:251 // Message-only windows do not receive WM_DISPLAYCHANGE broadcasts.`
- `platform/windows/mod.rs:32-64`：用 `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` 定位窗口像素；注释说明 `GetWindowRect` 会含不可见 resize 边框且受调用方 DPI 虚拟化影响。
- **`snow-capture` 里没有滚动拼接算法**：全 crate grep `scroll|stitch|long_capture` 只命中 `capture_session.rs:1072` 的文档注释与 `wgc/update.rs:492/508/655/676/697` 的测试辅助函数。`src/` 内 `TODO|FIXME|HACK|XXX` 零命中。
- `SetWindowDisplayAffinity` / `WDA_EXCLUDEFROMCAPTURE` 在 `src/` **零命中**，只出现在 `tests/wgc_scrolling_e2e.rs:26-27` 与 `:564-566`（→ WDA 排除是 C++ 层做的，见 §5.10）。
- **`tests/wgc_scrolling_e2e.rs`（602 行，5 个测试全部 `#[ignore]`，需真实交互桌面）**：阈值 `SCROLL_ROWS = 24 / BAND_HEIGHT = 8 / PIXEL_TOLERANCE = 3 / REQUIRED_SHIFT_MATCH = 0.97 / MAX_STALE_MATCH = 0.35`；夹具用真实 `ScrollWindowEx(hwnd, 0, -SCROLL_ROWS, None, None, None, Some(&mut update), SW_INVALIDATE)` 同步驱动；通过条件 = 位移匹配 ≥ 0.97 **且** 同位置残留 ≤ 0.35 **且**（`OrderedIncremental` 时）确实观察到非空 `dirty_rects`。
  > **这是一条可直接抄的门禁形状**：既要求"新内容匹配"，又要求"旧内容不残留"，两个方向的像素断言都要满足。

---

## 7. 测试、基准与诊断方法论

### 7.1 测试清单

| 文件 | 行数 | label |
|---|---|---|
| `tests/scrolling_image_replay.h` / `.cpp` | 68 / 357 | 夹具 |
| `tests/scrolling_image_replay_tests.cpp` | 933 | unit |
| `tests/scrolling_hover_geometry_tests.cpp` | 408 | unit（offscreen） |
| `tests/scrolling_hover_preview_tests.cpp` | 245 | unit |
| `tests/scrolling_auto_scroll_tests.cpp` | 229 | unit（**未显式设 label**） |
| `tests/scrolling_capture_cadence_tests.cpp` | 137 | unit（**未显式设 label**） |
| `tests/scrolling_capture_exclusion_tests.cpp` | 172 | unit（offscreen） |
| `tests/scrolling_selection_movement_tests.cpp` | 45 | unit（**未显式设 label**） |
| `tests/scrolling_thumbnail_uia_e2e_test.cpp` | 1204 | `e2e;uia;interactive` + windows |
| `tests/macos_scrolling_input_tests.mm` | 133 | `-platform cocoa` |

基准 4 个，**均无 `add_test`（只能按 target 手动跑）**：`scrolling_image_performance_benchmark.cpp` 311 / `scrolling_preview_benchmark.cpp` 143 / `scrolling_hover_preview_benchmark.cpp` 321 / `scrolling_result_async_benchmark.cpp` 266。

Rust 侧：`snow-stitch-images/examples/scroll_4_benchmark.rs` 607（唯一帧序列夹具）/ `snow-capture-c/examples/scroll_region_benchmark.rs` 499 / `snow-stitch-images/src/estimator_optimization_tests.rs` 373 / `snow-capture/src/capture_region_memory_benchmark.rs` 158 / `memory_{paths 165, snapshot 97, reference 181}_benchmark.rs` / `benchmark-support/memory.rs` 167 + `README.md` 145。

### 7.2 夹具方法论：4 条互不相干路径，**没有一条是"录制真实帧序列 → 回放 → 基线比对"**

| 路径 | 输入 | 说明 |
|---|---|---|
| **A replay（主路径）** | **程序生成的合成图** | `scrolling_image_replay.cpp:130-131`：`event.frame.image = m_state.image.copy(0, trace->offset, m_state.image.width(), m_state.viewportHeight);` —— **一张静态大图按整数 offset 裁窗口当"当前帧"** |
| **B 单张长图 PNG** | `snow_shot/test-imgs/scrollscreenshot-test.png` | CMake 宏 `SNOW_SCROLLING_DEFAULT_IMAGE` —— **该文件不在 checkout 里** |
| **C PNG 帧序列目录** | 任意目录 | `scroll_4_benchmark.rs:235-268`：只认 `.png`、**按文件名字典序排序**、≥2 帧 |
| **D 脚本化真实窗口** | UIA e2e 自绘分层窗口 | 纹理是文档绝对坐标的函数（`:208-226`，`y + state.offset`），10 次滚动 × 96px |

**A 的图案生成器**（`scrolling_image_replay_tests.cpp:177-191`）：640×640 RGBA8888 白底 + xorshift32 5×5 噪声块每 7px（种子 1234567）—— **无周期重复纹理**。

**A 的调度数学**（`scrolling_image_replay.cpp:163-184`）：`offsetAt` / `motionTimeForOffset` / `nextCaptureTime`，帧数 = `movements + 1`；源队列上限 3，溢出丢最旧并标 `"source_dropped"`（`:201-212`）。

**断言方式以逐像素为主**：
- `snapshotMatchesSrgbPixels` = `colorSpace() == sRGB && snapshot == expected`（`scrolling_image_replay_tests.cpp:34-37`，**无容差、无哈希**）；
- UIA 层用"变化比例"（源 ≥ 5%、缩略图 ≥ 1%，`:1051-1144`）与"特征色像素计数"（把手 `#faad14` ≥ 80 px，`:817-865`）。

**亮点：夹具自证** —— `verifyScrollingSourceOffsets()`（`:697-751`）**先让真实拼接器恢复脚本化 96 px 位移并断言 `added_rows == kScrollDistance`，再测 UI**。

> **这是本次研读里第二高价值的可抄点（仅次于 ORB）。** 它把 `docs/24` §S0.3 的夹具从"生成 → 反解"升级为"**生成 → 真实拼接器反解 → 断言等于已知位移**"——即**夹具自己先证明夹具是自洽的**，然后再拿它测别的层。这直接对应 `docs/24` §11.2 要求的"位移证据"。

**测试"最强的一面"**：
- **调度数学**：分数帧率不漂移（`scrolling_image_replay_tests.cpp:59-65`）；越界不增长 / `maximumSteps` 截断（`:50-69`）；4 组非法参数必抛（`:70-97`）；
- **生命周期竞态**：pause-在飞（`:306-371`）、teardown 后两方向各 256 请求（`:532-600`）、detach 后 cancel 不撤单（`:492-530`）；
- **诊断即断言**：JSONL 断言 `received == 3 && accepted == 1 && invalid == 1 && duplicate == 1`（`:740-839`）。

**生命周期竞态的三条可直接映射到 `docs/19` §4.1 的 `cancel()` 契约**："取消不 join 线程"、"detach 后已接受的完成回调仍然生效"、"teardown 后继续来请求必须安全拒绝"。

### 7.3 用例覆盖矩阵（★有 ☆部分/间接，空 = 无）

| 场景 | replay 单测 | hover/几何 | cadence | UIA e2e | Rust |
|---|---|---|---|---|---|
| 已知整数位移（正） | ★ | ☆ | | ★ | ★ |
| **方向变化（中途反转）** | | | | | |
| 回滚 / 双向 | ☆ | ☆ | | | ☆ |
| **重复纹理** | | | | | ☆ |
| 无移动（重复帧） | ★ | | ★ | | ☆ |
| 内容重排 / scene cut | | | | | ☆ |
| 大图上限 | | | | | ☆ |
| DPI | ☆ | ★ | | ☆ | |
| 多显示器负坐标 | | | | | |
| 帧率上限 / 降速 | ★ | | ★ | | |
| 丢帧后仍正确 | ☆ | | ☆ | | |
| 竞态 / 生命周期 | ★ | ★ | | ☆ | |
| 横向滚动 | ★ | ★ | ★ | | ★ |
| **真实录制帧序列回归** | | | | | |

**明确空白（对 `docs/24` §11.2 的直接价值）**：

1. **方向变化（中途反向）零用例** —— 而 `SCROLLING_DIAGNOSTICS.md:8-10` 声称"方向变化会推进 operation"；
2. **C++ 侧重复纹理零用例**（replay 图无周期纹理）；
3. **懒加载 / 动态内容 / 动画中间帧零夹具**；
4. **混合 DPI、跨屏负坐标的拼接正确性零用例**（仅 `scrolling_selection_movement_tests.cpp:25-41` 的逻辑 clamp）；
5. **大图 / 内存上限的"拒绝路径"零用例**（只有丢弃与截断）；
6. **丢帧后画布 gap / 重复一致性无端到端断言**（只断言 `disposition` 与计数）；
7. **真实录制帧序列回归无基线**；
8. macOS 只有鼠标透传、**无滚动拼接 e2e**；Linux 无；
9. **无"100 步累计漂移 ≤ N px"**（`docs/19` §11.5 要求 ≤ 2 px）；
10. **无 false acceptance 比例统计**（只有 `Uncertain` 标签）。

pipeline 端到端只测单向 `EXTENDED_BOTTOM`。

### 7.4 基准门槛（全部硬编码，无基线文件）

| 基准 | 规模 | 硬门槛 |
|---|---|---|
| `scrolling_image_performance_benchmark.cpp` | viewport 1600 / step 25 / fps 30 / max-steps -1，默认图 =（缺失的）test-imgs PNG | `:104-105` `require(build == "Release")`；`:242-259` `outputSize == expected && preview == QSize(128, ceil(h*128/w)) && coverage_complete` |
| `scrolling_preview_benchmark.cpp` | 128×256 tile、10000 updates | `:142` `checksumMatches && storageBounded && improvementPercent >= 80.0`；`:124-125` `allocatedBytes <= logicalBytes + tileBytes` |
| `scrolling_result_async_benchmark.cpp` | 3840×2160、warmups 2、rounds 9 | `:261-263` `schedulingP95 < 5ms && blockingReduction >= 90% && eventLoopGap < 50ms && completionChange <= 5%` |
| `scrolling_hover_preview_benchmark.cpp` | 源 1920×12000 / 12000×1080、round -3..30 | `:277-280` `requests == 2 && maximumInFlight == 1 && outputBytes <= 640*640*4` |
| `scroll_4_benchmark.rs` | `--samples` 默认 1；输入目录 ≥ 2 PNG | 无阈值；仅 `decisions.len() == frames - 1`（`:389-395`）+ Release（`:227-231`） |
| `scroll_region_benchmark.rs` | 默认 800×600、warmups 30、samples 240 | 无阈值；帧宽高/stride 匹配（`:421-430`）、区域覆盖（`:282-284`） |
| `estimator_optimization_tests.rs` | 7 组裁剪 × reverse；round 0..10 | 仅配对 `assert_eq`；**整宽裁剪误差必须恰为 0**（`:206-208`） |

运行方式：C++ 一律 `windows-msvc-performance`(Release)；`snow_shot/scripts/run-scrolling-perf.ps1` 串起 Rust + cargo + cmake 三步（`:28, :38-43, :53-68, :77, :82-88, :90-110`），实机示例双 4K `-RegionWidth 7680 -RegionHeight 2160 -LiveWarmups 10 -LiveSamples 60`（`:118-120`）。

> **滚动系基准不使用基线文件**（仓库唯一被消费的基线是 `screenshot_history_performance.json`，`snow_shot/CMakeLists.txt:7414-7421`）。
>
> **"门禁 = 退出码"** 的形状值得抄：`scrolling_preview_benchmark.cpp:124-142` 把 `checksumMatches && storageBounded && improvementPercent >= 80.0` 三个条件合成一个退出码。

### 7.5 内存基准方法论（`snow-crates/benchmark-support/`）

- `#[global_allocator] CountingAllocator` 维护 `LIVE / PEAK / ALLOCATED / ALLOCATIONS`（`memory.rs:14-60`）；
- `phase()` 输出 JSON Lines，含 `rss_bytes`（Windows 下为 `null`）/ `heap_live` / `peak` / `allocated` / `allocations`（`:102-111`）；
- `measure()` = 3 次 warmup → 重置 PEAK → 31 次采样（`:129-155`）；
- `checksum()` 每 4096 字节 + 末字节折叠（`:159-167`）。

各基准：

| 基准 | 规模 | 断言 |
|---|---|---|
| `memory_paths_benchmark.rs` | 竖直 canvas 3840×21600 ≈ 331 MB，6 条路径（stitch-repaint / retained / export / materialize / horizontal / orb） | ORB 路径 `\|offset + 64\| <= 2` 且 keypoints/matches > 0（`:149-159`） |
| `memory_snapshot_benchmark.rs` | 额外 `snapshot-retained` 相 | **把"持有"与"物化"分开计量**（`:74-93`） |
| `memory_reference_benchmark.rs` | 4K = 31.6 MiB 比较视口 | `branch == Contained`、`reference_mode == CanvasWindow`、**最终整图逐像素相等**（`:162-163`） |
| `capture_region_memory_benchmark.rs` | mock backend，头注释 *"no native GPU capture is performed"* | 输出与源**不共享指针**（必须真 detach，`:140`）、每行 `covered_width*4` 等于源、其余必须全 0（`:144-152`） |

**`benchmark-support/README.md` 是最佳方法论，直接可抄**：

- 成对修订 `5aff3a15` vs `8c90efb6`（**勿把 `865805a0` 当完整收益**）；
- 记录 lockfile 与二进制哈希；
- 每场景独立进程；
- macOS 固定 `MACOSX_DEPLOYMENT_TARGET=15.0` + `RAYON_NUM_THREADS=4`；
- 警告 *"堆流量不能证明空间下降（存储搬到 OS 映射时）"*（`:68-70`）；
- `logical_bytes` 不含容量余量 / 临时输出（`:60-62`）；
- 声明不覆盖 native 捕获 / ONNX / 硬编 / FFmpeg / overlay（`:78-82`）。

> **`memory_reference_benchmark.rs` 那三条断言（`Contained` + `CanvasWindow` + 整图逐像素相等）就是 `docs/19` §7.4 "重锚定不产生 gap/重复"的验收形状。** 参考实现有一个可运行的门禁，`docs/19` 只有文字要求。

### 7.6 `scrolling.*` 诊断事件契约（24 个事件 + 1 个 perf counter）

| 事件 | 位置 |
|---|---|
| `start_rejected` | `screenshotscrollingcapturecontroller.cpp:149, :165` |
| `started` | `:212` |
| `preparing` | `:215` |
| `prepared` | `:231` |
| `input_state` | `:133` |
| `input_target` | `:122` |
| `source_initializing` | `screenshotscrollingpipeline.cpp:471` |
| `native_create` | `screenshotscrollingnativesource.cpp:135` |
| `source_ready` | `pipeline.cpp:481` |
| `backend_selected` | `nativesource.cpp:44` |
| `first_frame` | `pipeline.cpp:568` |
| `frame_rejected` | `pipeline.cpp:599` |
| `stitch_result` | `pipeline.cpp:784` |
| `first_preview` | `controller.cpp:455` |
| `preview_timeout` | `:104` |
| `failed` | `:394` |
| `capture_progress` | `pipeline.cpp:527` |
| `capture_summary` | `pipeline.cpp:562` |
| `auto_scroll` | `controller.cpp:819` |
| `wheel_dispatch` | `:692` |
| `mode_changed` | `:264` |
| `export_pause` | `:569` |
| `stopped` | `:300` |
| `window_exclusion` | `:737` |
| perf counter `scrolling.snapshot_cache_hit` | `controller.cpp:514` |

埋点基建 `screenshotscrollingdiagnostics.h`：`logScrollingEvent(event, generation, fields, level)` 自动填 `fields["operation"]`（`:10-15`），channel `snow_shot.scrolling`；计数器 `received / accepted / timeouts / duplicates / invalid / mailboxDropped / poolUnavailable / droppedEvents`；首报 5 s → 之后每 30 s（`:29, :39-44`）；`fields()` 输出 `duration_ms` 等 8 项（`:46-58`）。

**文档反复标注"这条证据不证明什么"**：

- `backend_selected` **不能**证明之前的 backend 为何被跳过；
- 几何只记元数据，不记像素与窗口标题（隐私边界，`:23-27`）；
- `wheel_dispatch.status == 0` 只表示投递成功，**不证明目标应用处理了**。

> **这是 `docs/19` §10.3 `AppEvent::Scroll(ScrollProgress)` 与 `docs/24` §11.2 执行记录模板的直接参考形态**：事件表 + 计数器 + **"不证明什么"列** + 隐私边界声明。特别是最后一条——`docs/24` §11.2 要求必填"位移证据"，而参考实现明确区分了"注入成功"与"目标处理了"，这正是 `docs/25` R15 的 5 类 status 的意义。

### 7.7 仓库级缺陷（"引用即失败"）

1. **`snow_shot/test-imgs/scrollscreenshot-test.png` 不存在**：
   - `-Filter test-imgs|scrollscreenshot*` 零命中；
   - `git ls-files | Select-String test-imgs` 零命中；
   - `git status` 干净；`.gitignore` 只忽略 `**/artifacts/`；
   - 后果：`scrolling_image_performance_benchmark.cpp` 默认 `--image` 是死路径；`estimator_optimization_tests.rs:170-171 / :280-281` 两个 `#[ignore]` 基准会在 `Frame::decode(...).unwrap()` **panic**。
2. **`run-scrolling-perf.ps1:38-43` 调 `cargo run --example scrolling_perf`，而该 example 不存在**（`snow-stitch-images` 全 crate 搜 `scrolling_perf` 零命中；examples 只有 `memory_paths` / `memory_reference` / `memory_snapshot` / `scroll_4` / `snapshot_export` / `stitch_test_frames` / `stitch_test_imgs`）→ **脚本第一步即失败，整条 perf 链路在干净 checkout 不可运行**。
3. cadence / selection-movement / auto-scroll 三个 test 未显式设 LABELS（`snow_shot/CMakeLists.txt:3592-3620`，落默认 unit）；thumbnail-drag / hover-pipeline 复用同一 binary 的子集开关（`scrolling_image_replay_tests.cpp:889-933`），**在 CTest 列表上不可见**。
4. `SCROLLING_DIAGNOSTICS.md` 中"方向变化推进 operation"、"`capture_progress` 限流"、"`backend_selected` 状态表"**均无测试覆盖**。

> **教训（对 SnapClip 的验收纪律）**：Snow Shot 的滚动截图**没有任何可运行的端到端门禁**——真机 e2e 全部 `#[ignore]`，基准默认输入缺失，perf 脚本引用了不存在的 example。**`docs/24` 的验收要求（真跑基线、写执行记录、夹具先于算法）恰好补上这个洞，不应因为"参考实现也没做"而放松。**

---

## 8. 对 `docs/19` 的逐节对照与建议

### 8.1 `docs/19:132-136` 已有 5 条 snow_shot 引用——**逐条核对全部属实**

| `docs/19` 行 | 原文要点 | 核对结果 |
|---|---|---|
| `:132` | "Snow Shot cadence \| EWMA 成本、队列压力降速、连续健康样本才恢复" | **属实**。`adaptivescrollingcadence.h`：`ewmaSampleWeight = 0.25`、下降瞬时、`recoverySamples = 4` 才恢复、`recordStreamPressure` 里 `target * 0.75` 折扣（§5.3） |
| `:133` | "Snow Shot overlay/worker \| 滚动模式独立交互状态、暂停/恢复、捕获 worker 与 UI 解耦" | **属实**。4 个 Qt/原生执行体 + `pause` 三段屏障 ack（§5.1、§5.11） |
| `:134` | "`ScreenshotScrollingThumbnailWidget` \| 固定 128 px 交叉轴缩略图；沿滚动轴以 256 px tile 增量追加/替换；视口高亮、裁剪边界和 hover 映射只更新局部脏区" | **属实**。`thumbnailwidget.cpp:23-30`（128 / 256 / max 640）+ `:451-491`（`std::lower_bound` 可见区局部重绘）（§5.7） |
| `:135` | "`ScreenshotScrollingPipeline` \| 每次 stitch commit 只生成 preview patch；首帧 replace，后续按 append/prepend 更新，并用重叠行修正缩放取整漂移" | **属实**。6 态 `StitchChange` + `replacedPreviewRows`，注释原文 *"absorb all scale rounding into this small edge patch so the retained preview tiles never drift in height"*（§5.6） |
| `:136` | "`ScrollingHoverPreview` \| hover 请求单飞、带 epoch/content revision，旧回调丢弃；必要时短暂停止采帧后再读取 viewport" | **属实**。`m_pending` 单飞 + 四重过期校验（serial / paused&&ready / epoch+contentRevision / rect）+ 先 pause 等 ack 再 request（§5.8） |
| `:514` | 再次点名 128 px / 256 px / replace-append-prepend / overlap 替换行 / 局部绘制 / hover revision | **属实**（同上） |

> **结论：`docs/19` 现有的 5 条引用措辞准确、无夸大，且都抓住了机制要点。** 问题不在"抄错了"，而在**抄得太少**——下面 5 条是实现里真实存在、且直接关系到 `docs/19` 正确性的机制，`docs/19` 完全没有吸收。

### 8.2 `docs/19` 未吸收、但参考实现里真实存在的机制（按重要度排序）

| # | 机制 | 位置 | 为什么 `docs/19` 需要它 |
|---|---|---|---|
| **N1** | **纯 Rust ORB（1,739 行，零 OpenCV）** | `orb.rs` | 直接消解 `docs/19:691` 的依赖禁令，并把 ORB 从"最后手段"变成"可行主路径"（§8.3） |
| **N2** | **`Contained` 分支独立成态** | `state.rs:19-84` | `docs/19` §8.1 的 union 模型缺这一支（§8.4） |
| **N3** | **双帧分离：`previous_raw` vs `motion_reference`** | `stitcher.rs:289-306` | `docs/19` §7.2 只讲"上一帧"，没区分"用于算位移的帧"与"用作匹配参照的帧"（§8.4） |
| **N4** | **参照系二态 `Synthetic` ↔ `CanvasWindow`** | `stitcher.rs:472-474` | `docs/19` §7.4 的重锚定是另一套机制，缺这第一道（§8.5） |
| **N5** | **乘法式 tile 三分类权重 + 中性先验 1/3 + 观察 ramp** | `region.rs:779-788` | `docs/19` §7.3 是加法式线性加权（§8.3 之后） |
| **N6** | **`MIN_INLIER_TILES = 4` / `MIN_RESIDUAL_GAIN = 0.15` 量化门限** | `estimator.rs:12-20` | `docs/25` R17 要求的正是这两个量（§8.3） |
| **N7** | **`content_generation` 与 `is_duplicate` 是两个独立信号** | `frame.rs:64-66`、`streaming.rs:820-825` | `docs/19` §4.4 只用生命周期表达"丢帧"，不足以表达"内容是第几代"（§8.7） |
| **N8** | **`band_height(H, shift) = (H/2).max(H/4 + shift)`** | `compositor.rs:8-27` | `docs/19` §8.2 讲了 sticky/overlap 但没给行带高度的公式（§8.8） |
| **N9** | **tile 租约 + 有界空闲池（`MAX_SPARE_TILES=2`、`MAX_SPARE_TILE_BYTES=8 MiB`、`tile_capacity_limit = len + len/8 + 64`）** | `tiled_canvas.rs:7-18` | `docs/19` §8.3/§8.4 要求"有界 LRU"，但没给有界口径（§8.9） |
| **N10** | **快照零拷贝租约："快照不依赖会话生命周期"** | `snow_stitch_images.h` | `docs/19` §5.6 的预览与 §8.3 的导出都需要这一条所有权语义（§8.9） |
| **N11** | **夹具自证：先让真实拼接器反解已知位移** | `scrolling_thumbnail_uia_e2e_test.cpp:697-751` | `docs/24` §S0.3 的夹具应升级到这一形态（§8.13） |
| **N12** | **DXGI `GetFrameMoveRects` 位块搬移重建** | `duplication.rs:849-891` / `:585-642` | `docs/19` §7.2 可以多一条"显示器档读位移"的路径（§6.7） |
| **N13** | **`perf.rs` 的"并行只在 join 两侧计时"** | `perf.rs` | `docs/19` §10.2 的指标定义纪律（§8.14） |
| **N14** | **`StitchDecision` 可序列化审计记录** | `decisions.rs` | `docs/24` §11.2 的执行记录模板（§8.14） |
| **N15** | **`SPI_GETMOUSEWHEELROUTING` 能力探测** | `overlaywindow.cpp:327-368` | `PostMessage(WM_MOUSEWHEEL)` 的系统前提（§8.11） |
| **N16** | **引擎无上限、上限完全由 FFI 施加的层次划分** | `snow-stitch-images-c/src/lib.rs:20, :67` | `docs/19` §8.4 的上限取值应做成可注入策略值（§8.15） |

### 8.3 方向性问题：ORB 应从"最后手段"提到 v1 主路径

**`docs/19` §7.2 的现状**（原文要点）：

- "先实现小尺寸 CPU MatchView 基线，不预设 GPU 预处理"；
- 5 步结构：① 每行/列 compact descriptor（mean luma、variance、edge energy、少量 bins）② `expected_delta ± search_margin` 一维 profile SAD 粗搜 ③ 多带 consensus ④ **仅在候选 `delta ± 4..8` 物理 px 内**回读原始窄 overlap strip 做全分辨率精修 ⑤ `ValidMask` 排除页眉/页脚/滚动条/边框/光标/tooltip；
- 质量降级顺序：① Profile SAD + 多带 consensus ② NCC + edge ③ 降低 notches ④ **"真实样本证明仍失败后，才增加可插拔 ORB/AKAZE。OpenCV 不是 v1 常驻依赖，且不得进入 `snapclip-capture` 的默认依赖图"**。

**参考实现的现状**：`orb.rs`（1,739 行）+ `estimator.rs`（2,036 行），**ORB 是唯一主路径，没有 SAD、没有 NCC、没有 profile**。

**为什么这条分歧值得改**：

1. **依赖顾虑已被消解**。`docs/19` 把 ORB 放最后的**唯一硬理由**是不想引入 OpenCV。`orb.rs` 用 1,739 行纯 Rust 复刻了 OpenCV ORB 的全部常量与语义（`SCALE_FACTOR 1.2`、`LEVELS 8`、`FAST_THRESHOLD 20`、`HARRIS_K 0.04`、`PATCH_SIZE 31`、`EDGE_THRESHOLD 31`、OpenCV 7-tap blur 用 `f32::from_bits`、256 点 pattern 用行内 base64）。**这个理由消失了。**
2. **`docs/24` §0.4 的"OpenCV 作为 `snapclip-capture` 默认依赖"本身就是事实错误**（`docs/25` §3 已证实：capture 的 `Cargo.toml` 全文无 opencv，全仓唯一命中 `.comparison-old/Cargo.toml:21`）。**一个基于错误前提的禁令，其结论不应继续约束架构。**
3. **profile-SAD 有一个 `docs/19` 自己承认的失败模式，ORB 没有**：`docs/19` §7.2 返回 `Rejected/Uncertain` 的第 2 条就是"best-second margin 不足**或重复纹理**导致候选不唯一"。profile-SAD 是**一维**的——沿主轴丢掉了横轴的全部结构信息，遇到周期性条纹（表格、日志、代码、聊天记录）时天然产生多个等强候选。ORB 用**二维特征点 + 空间分布**判据（`MIN_INLIER_TILES = 4` 要求至少 4 个**独立空间位置**支持同一位移），对周期纹理的抵抗力是结构性更强的。
4. **参考实现给出了完整的量化门限**：`MIN_INLIER_MATCHES = 8`、`MIN_INLIER_TILES = 4`、`MIN_RESIDUAL_GAIN = 0.15`、`min_confidence = 0.65`、`MAX_CROSS_AXIS_DELTA = 4`。这些正是 `docs/25` R17 要求补的量。

**建议（对 `docs/19` §7.2 的具体改法）**：

- **把 ORB 与 profile-SAD 并列为主路径**，而不是"第 4 步降级"。`ShiftMatcher` trait（`docs/19` §7.1）已经为此设计好了接口，这是 `docs/19` 的一个真实优点——保留它。
- 具体的两级策略：**先用极廉价的整帧相等/近似相等做 `IdenticalInterior` 早退**（参考实现 `estimator.rs:993-1083` 的第 2、3 步：`w<5||h<5`、`visible_interior_pixels_equal`，置信度直接 1.0），**然后用 ORB 做唯一的位移估计**。profile-SAD 若要做，只作为"ORB 不可用时的降级"（对称地反转 `docs/19` 的降级顺序）。
- 若担心 ORB 的算力：参考实现的降采样策略是 `sampling.rs` 的 `PyramidPlan` + `DOWNSAMPLE = 4`，并在 `needs_full_resolution()` 时**内联重跑一次全分辨率**。这是"降采样优先、困难样本回退"的正确粒度——**比 `docs/19` §7.2 的"回读 1/4 降采样 MatchView、困难样本 1/2"更简单，因为不需要第二条数据通路**。
- **不要照抄 `orb.rs`**。它是 Apache-2.0（法律上可抄），但 `docs/19:120` 的既定立场是"只吸收算法原则与测试思想，不复制其管线"。建议：在 `snapclip-capture` 内部自写一个 ORB 子集，**用参考实现的常量表作为正确性锚点**（同参数应给出接近的 keypoint 分布）。
- **空间均衡必须一起做**：`MAX_FEATURES_PER_TILE = 8` + `score = response.max(0.001) * regions.weight_at_tile(tile)`（`estimator.rs:359-425`）。否则滚动条、固定页眉这类高对比度区域会吃掉全部特征预算——这正是 `docs/19` §7.3 的动态 mask 想解决的问题，但参考实现是在**提取之前降权**而不是**匹配之后排除**，代价更低，也不会因为排除后有效像素不足而被迫 `Uncertain`。

**同时保留 `docs/19` 的一处优点**：`docs/19` §7.3 要求"`valid_pixels < minimum` 必须 `Uncertain`"。参考实现没有这个安全阀（它有 `LowConfidence` 但门限是内点数与 tile 数，在**动态内容占比极高时会给出低置信度而不是显式拒绝**）。**这是 `docs/19` 应当保留的净增量。**

### 8.4 `docs/19` §8.1 的 union 模型缺 `Contained`，且缺双帧分离

**`docs/19` §8.1 现状**：`next_pos = current_pos + signed_delta`，要求 coverage 二维完整覆盖。

**参考实现（`state.rs:19-84`）**：

```rust
candidate = position.checked_sub(offset)
  candidate < 0            → Prepend(growth = -candidate, max_position += growth, position = 0)
  candidate > max_position → Append(position = max_position = candidate, growth = candidate - max_position)
  else                     → Contained(growth = 0)
```

**要补的三件事**：

1. **`Contained` 是独立分支**。用户滚回已探索过的区域内时，`growth == 0`、画布完全不变，但**必须把 `max_position` / `position` 更新为新的观察值**（参考实现不更新 `max_position`，因为 `candidate <= max_position`）。`docs/19` 的 `next_pos` 公式能算出位置，但没有"这次不生长"这一态的显式表达——而这一态**决定了预览 patch 是 `replace` 还是 `append`/`prepend`**（§8.10）。
2. **`offset` 为负 = 向下滚 = `Append`**。符号约定必须写死在文档里，否则"向前/向后"与"上/下"会互相污染。参考实现用 `position.checked_sub(offset)`，即**正的 offset 表示回滚**。
3. **双帧分离**。`docs/19` §7.2 只说"上一帧"，但参考实现区分：
   - `previous_raw` —— **紧接着收到的上一张真实帧**，用于计算位移；
   - `motion_reference` —— **参与匹配的参照帧**，可能是合成的（`Synthetic`）或从画布物化的（`CanvasWindow`）；
   - `NoMovement` 时 `previous_raw = incoming` 但 `motion_reference` 不变（`stitcher.rs` 的 `non_skip_advances_previous_raw_even_without_motion` 测试固化）。
   **唯一转换点 `comparison_reference()`（`stitcher.rs:289-306`）**——把帧坐标↔画布坐标的换算收口到一个函数，这条纪律应直接写进 `docs/19`。

### 8.5 `docs/19` §7.4 的全局重锚定：参考实现**没有**，属于无背书设计

**`docs/19` §7.4 现状**：每 N 步（默认 20）或累计 `|delta|` 超阈值做全局重锚定；keyframe = 低分辨率 profile + 必要窄带摘要（默认 1/8 行/列），单会话只留 1 个，**上限默认 ≤ 64 KB**；重锚定只改逻辑坐标原点不改写已提交 tile；若全局对齐与链路预测差异 > 2 px → `DriftBeyondBudget` 停止并保留 Partial；验收"100 步滚动后总误差 ≤ 2 px"。

**参考实现的现状**：**唯一的重锚定机制是 `Contained → reference_mode = CanvasWindow`**（`stitcher.rs:472-474`）。没有周期性 keyframe、没有累计漂移阈值、没有 `DriftBeyondBudget`、没有 keyframe 尺寸预算。

**同时，参考实现有一个 `docs/19` 没有的问题**（子代理 A 的 S2）：**`Synthetic` 参照系有累积漂移** —— `synthesize_append_reference` 用**当前帧的位移估计结果**拼参照帧，误差被写回参照帧，再影响下一帧的估计。连续 Append 期间（没有 `Contained` 打断）**没有任何中间校正**。

**结论与建议**：

- `docs/19` §7.4 是**比参考实现更完备**的设计，这一点应在文档里明确标注为**净增量 + 无参考实现**；
- **但先采纳参考实现的第一道重锚定**：`Contained → CanvasWindow`。它零成本（只是切一个 enum）、且 App/用户回滚时必然触发；
- §7.4 的周期性 keyframe 与 `DriftBeyondBudget` 应**降级为 S1.x 的待验证设计**，验证方式就是 §7.4 自己写的验收"100 步滚动后总误差 ≤ 2 px"——而这恰好是参考实现**零用例**的地方（§7.3 留白第 9 条）；
- **`docs/19` §13.1 唯一的开放项"canvas 落盘是否拆线程"应扩写**：现在还应加上"§7.4 的重锚定机制是否需要（以及 keyframe ≤ 64 KB 的依据）"。

### 8.6 §6.3 自适应 settled 与 §6.5 闭环步长：参考实现**两条都没有**

**事实**：

- `screenshotscrollingpipeline.cpp` 全文 grep `settle` / `stability` / `consecutive` / `identical` / `unchanged` / `idle`（除 `:947 bool ScreenshotScrollingPipeline::idle() const`）**零命中** → **pipeline 里不存在"等画面稳定"逻辑**；
- 重复帧靠 `ScrollingSourceEvent::Kind::Timeout`（`:552 ++m_diagnostics.timeouts`）与 `source.duplicate`（`:573` / `:595` / `:608-615`）处理；
- 自动滚动是**固定 120 单位 + 固定 200 ms 间隔的开环注入**（§5.4），**没有任何基于实测位移的反馈**；
- C++ 控制器层**没有任何尺寸上限、没有任何"到达终点"的自动判定**（§5.14）。

**结论与建议**：

| `docs/19` 节 | 参考实现有无 | 建议 |
|---|---|---|
| §6.3 自适应 settled（等画面稳定再采帧） | **无** | **改为"自适应采样节拍（cadence）"**——这是有完整参考实现的机制（§5.3），且 `docs/19:132` 已经引用了它。settled 等待降级为可选：若 S1.x 夹具证明"匹配失败的主因是动画中间帧"，再引入。**不要为它预先设计状态机。** |
| §6.5 闭环步长 | **无**（固定 120，开环） | **降级为 S1.x 的实验项**。参考实现用"自适应 cadence + duplicate 检测 + 低置信度降级为 `NoMovement`"覆盖了同样的需求（避免滚动过快导致匹配失败）。开环 + 自适应帧率是一个**更简单、且有部署验证**的方案。 |
| §6.7 终点二次确认（`EndConfirmed`/`EndUncertain`） | **无**（用户自己决定何时停） | **保留但标注"无参考实现"**。`docs/19` §7.5 的"连续 3 个稳定帧无新增或边缘未变化才判定边界，单帧不变可能是加载动画"是对的（参考实现也没有终点判定，所以无法反证），但**这是 SnapClip 的净增量**。 |

> **这条建议与 `AGENTS.md` 的"性能优化必须有依据"同源**：`docs/19` 的 §6.3/§6.5 是**没有参考实现、也没有实测支撑**的两处复杂度。把它们降级为"待验证"比继续按它们设计状态机更符合 `AGENTS.md` 的"禁止为臆测性能增加不必要的抽象"。

### 8.7 §4.4 帧信箱容量 1 → 建议 2–3，且必须区分"代次"

**`docs/19` §4.4**："帧信箱容量为 1，最新帧覆盖旧帧"；帧生命周期 `Requested → InFlight → Dropped → Arrived → MatchViewReady → Accepted/Rejected`；`Dropped` 不得当 `NoMovement`；丢帧时下一帧可能跨界 → 必须扩大搜索窗。

**参考实现的对应物（三层缓冲）**：

| 层 | 容量 | 语义 |
|---|---|---|
| `LatestBridgeMailbox` | **2**（mutex，无 Condvar，latest-win 覆盖） | 跨线程（capture producer → stitch worker） |
| `hasPendingCapacity()` | — | 被 `pipeline.cpp:593` 当**显式背压信号** |
| WGC 帧池 | **3**（`WGC_FRAME_POOL_BUFFERS`） | GPU 侧 |
| Rust 连续流队列 | `buffer_depth = 3` | 源侧 |
| BoundedFrameQueue | 满时丢最旧并置 `overflowed` | 传输侧 |

**为什么容量 1 有问题**：`docs/19` §4.5 同时说"staging 用 `DO_NOT_WAIT`，返回仍在使用则丢弃该次 MatchView 并记 `readback_not_ready`"。**容量 1 + 独立丢帧 = 消费者必须永远就绪，否则频繁空转**。参考实现用"容量 2 的 latest-win mailbox + `hasPendingCapacity()` 背压 + 上游自适应降帧"解决了同一问题，且**没有让生产者阻塞**（`recycle` 用 `try_send`，满即丢弃）。

**另外必须引进 `content_generation`**（`frame.rs:64-66` 注释原文：*"Stream-local image generation, unchanged for duplicates. Unlike a duplicate flag, this survives dropped or intentionally superseded frames."*）：

```rust
if *generation == 0 || !frame.metadata.is_duplicate {
    *generation = generation.saturating_add(1);
}
```

> `docs/19` §4.4 用**生命周期**（`Dropped`/`Arrived`）表达"这一帧是否被顶替"，参考实现把**"内容是第几代"**做成独立信号，使 `Dropped` 之后的帧仍能与"上一张真正收到的帧"正确配对。这正是 `docs/19` §4.4 想解决但表达不足的地方。

**§4.3 线程预算表也要改**：`docs/19` 说"整体只增加 1 个常驻线程"（4 个执行体）。参考实现实际是 **5 个**（GUI / capture producer QThread / producer 内部的 1 个 `std::thread` / stitch QThread / Rust capture worker）。如果 SnapClip 也要在"捕获生产者"内部再分一层（`consume()`），预算表要如实写 5 个，而不是把内部 `std::thread` 藏起来。

### 8.8 tile 尺寸与 `band_height`

**`docs/19` §8.3 现状**：`ScrollTile { bgra: Vec<u8>, checksum: u32 }`、默认 **512×512**、`ScrollExportMeta`、`ScrollSink` / `ScrollArtifactWriter`。

**参考实现有 `tile` 的地方共三处，尺寸完全不同**：

| 用途 | 尺寸 | 位置 |
|---|---|---|
| 画布 tile | **主轴 256，横轴恒整宽** | `tiled_canvas.rs:7-18`（`CANVAS_TILE_SPAN = 256`、`CANVAS_TILE_ROWS = 256`） |
| 预览 tile | **交叉轴 128，主轴 256** | `thumbnailwidget.cpp:23-30` |
| 估计 tile | **32 × 32** | `region.rs:24-30` |

`docs/19` 的 512×512 是**第四套**，且是**唯一一个把横轴也切分的**。

> **`docs/25` R3 建议改为"整宽行带 tile"，参考实现完全印证了这一点**：`snow_stitch_images.h` 的全部 tile API 参数只有高度轴一维，无 x/column/width；横向输出靠整条 tile 旋转 90°；`prepareTileImages` 按 scanline 搬运并按 `bytesPerLine` 校验。
>
> **建议**：`docs/19` §8.3 的 `ScrollTile` 改为**整宽行带**，主轴跨度取 256（与参考实现一致）或 512（与 PixPin 的 128 MiB 字节预算折算出的 4,232–31,714 行同数量级），**关键是横轴不切分**。同时明确区分三套 tile 的尺寸，避免"512"被误用到预览或估计层。

**另外补上 `band_height` 公式**（`docs/19` §8.2 讲了 sticky 与 overlap，但没给行带高度）：

```rust
band_height(H, shift) = (H/2).max(H/4 + shift)
```

即：行带高度 = 视口高度的一半，或"四分之一视口 + 实测位移"取大者。**这条公式同时决定了写入多少行与合成参照帧取哪些行**，是 `docs/19` §8.2 与 §8.3 之间缺失的接口量。

### 8.9 `docs/19` §8.3 的 tile 端口：参考实现证实"行带端口"是对的

（`docs/25` R3 已提出这一点；本次给出参考实现的完整形状作证据。）

**C ABI 的帧输出（`snow_stitch_images.h`）**：

```c
typedef struct {
    SnowStitchFrameEvent   event;             // 8 态，见下
    SnowStitchUnmatchedReason unmatched_reason;
    int32_t  matched_reference_offset_y;
    uint8_t  has_matched_reference_offset_y;
    uint8_t  reserved[3];
    SnowStitchMatchMetrics metrics;
    uint32_t added_rows, output_width, output_height;
    uint32_t delta_top, delta_rows;           // ← "Full edge band rewritten by this frame, including splice overlap."
} SnowStitchFrameOutcome;
```

**8 态事件**：`INITIAL=0, EXTENDED_TOP=1, EXTENDED_BOTTOM=2, COVERED=3, DUPLICATE=4, UNMATCHED=5, EXTENDED_LEFT=6, EXTENDED_RIGHT=7`。

**7 类未匹配原因**：`NONE, INSUFFICIENT_OVERLAP, LOW_INFORMATION, AMBIGUOUS, CONFLICTING_REFERENCES, FIXED_CONTENT_DOMINATED, VERIFICATION_FAILED`。

**5 项匹配度量**：`score, second_score, content_coverage, fixed_coverage, inlier_ratio` + `feature_support, reference_count`。

**快照族（零拷贝）**：

```
snow_stitch_session_snapshot[_axis]
snow_stitch_session_snapshot_slice_rows[_axis]
snow_stitch_snapshot_copy_rows(snapshot, top, rows, destination_stride, destination, destination_len)
snow_stitch_snapshot_materialize
snow_stitch_snapshot_render_scaled
```

注释原文：*"Snapshots retain shared immutable canvas tiles and do not depend on session lifetime. Creating or slicing a snapshot does not copy canvas pixels; materialization does."*

**导出族（异步 + 可取消 + 有进度 + 原子提交）**：

```
snow_stitch_snapshot_export_png(snapshot, const SnowStitchPngExportConfig*) → SnowStitchExportTask*
export_task_cancel / poll / wait / error_message / destroy
SnowStitchExportStage     { PREPARING, ENCODING, COMMITTING }
SnowStitchExportStatus    { RUNNING, COMPLETE, FAILED, CANCELED }
SnowStitchExportProgress  { stage, rows_written, total_rows, percent }
SnowStitchPngCompression  { FAST, BALANCED, BEST }
```

实现（`snow-stitch-images-c/src/lib.rs:746-846`）：独立线程（name `snow-stitch-png-export`，spawn 前 `snow_core::qos::apply_current_thread()`）、`sync_channel(16)` 报阶段、**每 64 行一个 strip**、temp 文件 + `flush` + `sync_all` + `persist`/`persist_noclobber` **原子提交**、循环中检查取消位、`Drop` 先 cancel 再 join（`:731-738`）。注释原文：*"Cancels and joins a still-running background export before returning."*

**有界帧池即背压**：

- `snow_stitch_frame_pool_create(width, height, capacity)`；
- `snow_stitch_frame_pool_acquire(pool)` —— *"Returns NULL while all bounded slots are owned by the pipeline."*；
- `snow_stitch_frame_pool_acquire_for_overwrite(pool)` —— *"Same bounded acquisition, with initialized but unspecified pixels. The caller must overwrite every packed RGBA byte before submitting the frame."*；
- `snow_stitch_session_push_owned(session, SnowStitchFrameBuffer** inout_frame, SnowStitchFrameOutcome* out)` —— *"Always consumes *inout_frame and sets the caller's slot to NULL."*

**错误约定**：`uint8_t` 返回 0 = 失败 / 1 = 成功；`const char* snow_stitch_last_error_message(void)`（**线程局部**）。

> **实现的是 `docs/25` R3 的建议形态**：端口收**行带**（`delta_top` + `delta_rows` + `added_rows`），tile 完全留在引擎内部（`tiled_canvas.rs`），**唯一编码器只有一条路**（`snow_stitch_snapshot_export_png`）。`docs/19` §8.3 的"端口收 tile"与"行带组装在 capture"与"唯一 PNG 编码器"三者之间的矛盾，参考实现用"**引擎内部 tile、跨端口只出行带、编码器只在引擎侧**"一句化解。
>
> **`docs/19:1103` 的 S4 要求（"tile 化画布、有界 LRU、checksum、`ScrollTile`/`ScrollExportMeta`、`ScrollSink`/`ScrollArtifactWriter` 端口与壳层实现"）应据此重写**：tile 与 checksum 是**引擎内部**的实现细节，**不跨端口**；端口只有行带与快照/导出句柄。

### 8.10 §5.6 预览：3 态 → 6 态，并补两个版本号

**`docs/19` §5.6**：`PreviewPatch` replace/append/prepend + viewport 高亮 + stale revision 丢弃。

**参考实现**：

- **6 态**（Initial / Replaced / AppendedTop / AppendedBottom / PrependedTop / PrependedBottom）—— 即 append/prepend 各分两端；
- **`replacedPreviewRows`** 把重叠行重绘与**缩放取整漂移**集中到一个边缘 patch（注释原文见 §5.6）；
- **两个版本号**：`epoch`（会话代次）与 `contentRevision`（内容修订）—— `docs/19` 只说"stale revision"，应补"两个维度"；
- **hover 读取前必须"先 pause 采集并等 ack"**（`m_ready = true` 才发 request）—— `docs/19` 没写，但这是避免 `materialize` 与正在写入的 tile 竞争的必要条件。

**建议**：`docs/19` §5.6 的 `PreviewPatch` 改为与 6 态 `StitchChange` 对齐，并明确"重叠行 + 缩放取整漂移必须在一个边缘 patch 内吸收"。

### 8.11 §5.1 overlay 输入模型：`docs/19` 的"隐藏 overlay"是另一条路，建议重新评估

**`docs/19` §5.1 现状**：v1 采用"隐藏 overlay + 独立 controller HWND"，`WDA_EXCLUDEFROMCAPTURE` 作降级层。

**参考实现（§5.10）**：**overlay 不隐藏、保持可见可交互**，三层处理：

1. **输入穿透**：只翻 `WS_EX_TRANSPARENT` 一位（要求已 `WS_EX_LAYERED`，返回旧值，开启时顺手 `ReleaseCapture()`），调用点注释明确写 *"do not hide/show the overlay or ask Qt to rebuild its window flags"*；
2. **视觉/输入空洞**：Qt `setMask(QRegion(rect()) - QRegion(hole))` → `SetWindowRgn`（幂等比较，空区域 `clearMask()`）；
3. **捕获排除**：`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`，**可被用户设置跳过**（`screenshot/capture_ui_in_scrolling_screenshot`）。

**建议**：

- **`docs/19` §5.1 应把"可见 overlay + 输入穿透 + 挖洞 + 捕获排除"列为方案 A、"隐藏 overlay + controller HWND"列为方案 B**，并在 `docs/24` §S0.7 的 overlay spike 里**用同一夹具同时测两条**；
- **理由**：隐藏 overlay 会连带丢掉 PixPin 的实时尺寸读数、方向下拉、贴图/保存/复制按钮（`docs/25` §1.1 界面元素 ②–⑨）；Snow Shot 的可见 overlay 方案保留了这些，并用 `setMask` 挖洞让**选区内部完全穿透**。这是产品能力的实质差异，不只是实现风格。
- **无论选哪条，都要加 `SPI_GETMOUSEWHEELROUTING` 探测**（`overlaywindow.cpp:327-368`）：它是 `PostMessage(WM_MOUSEWHEEL)` 生效的系统前提，`docs/19` §4.1 的能力探测清单里没有。
- `docs/24` §S3.6 的自认矛盾（"affinity 失败时 `Paused` 显示 controller" vs "monitor 级捕获要求 controller 不入镜"）在参考实现里有现成答案：**`captureUiInScrollingScreenshot()` 是一个用户设置**——想录进 UI 的用户自己打开，此时 controller/工具栏会入镜。**矛盾不是要消除的，而是要变成一个开关。**

### 8.12 §7.2 第 5 步：光标应在会话选项层禁用（R12 已被参考实现印证）

`docs/19` §7.2 第 5 步把"光标由 `ValidMask` 排除"。`docs/25` R12 已建议改为"会话选项层禁用光标捕获，接口不可用才退回掩码"。

参考实现 `wgc.rs:636-637`：

```rust
let _ = session.SetIsCursorCaptureEnabled(false);
let _ = session.SetIsBorderRequired(false);
```

**参考实现正是这个顺序**——先试会话选项，**错误被丢弃**（即"不可用就不管"，但**没有退回 `ValidMask` 掩码**）。

**注意**：参考实现 `capabilities.rs:37-59` **不探测** `IGraphicsCaptureSession2/3` 可用性（`hdr_capture: false` / `window_enumeration: false` 是写死的）。**所以 R12 的"能力探测"部分仍是 SnapClip 的净增量**，只有"优先用会话选项"这一半被参考实现印证。**这一点必须写清楚**，避免把"参考实现没做"误当成"不需要做"。

同时 `docs/19` §11.3 应加"长图中不存在 WGC 边框色带"的像素断言 —— 参考实现的所有像素断言里**没有这一条**（它的 e2e 断言只有位移匹配 ≥0.97 与残留 ≤0.35），所以这条同样是净增量。

### 8.13 `docs/24` §11.2 测试与验收：12 条可直接借鉴的形态

| # | SnapClip 的目标 | 参考实现的形态 | 位置 |
|---|---|---|---|
| 1 | **夹具自证** | 先让**真实拼接器**反解已知位移并断言 `added_rows == kScrollDistance`，再测 UI | `scrolling_thumbnail_uia_e2e_test.cpp:697-751` |
| 2 | 图案生成器（补重复纹理维度） | 纹理 = 文档坐标的函数 + 周期细线；扩成周期重复块即补上参考项目缺的重复纹理 | 同文件 `:208-226` |
| 3 | 可控窗口 | 分层窗口 + DIB，`scrollDown()` 只改 offset 重绘 | 同文件 `:306-355` |
| 4 | **输入注入 settle 纪律** | 按下 25 ms、拖动 50 ms + 6×25 ms；`SendInput` 绝对坐标归一化，注释写明原因 *"Windows can coalesce the messages and leave the app in its intelligent-selection state"* | 同文件 `:571-587, :610-653` |
| 5 | 故障注入证据闭环 | 排查树 + `wheel_dispatch` 五态 status 表 | `SCROLLING_DIAGNOSTICS.md:51-58, :61-66` |
| 6 | 调度/边界数学 | 分数帧率不漂移；越界不增长 / `maximumSteps` 截断；4 组非法参数必抛 | `scrolling_image_replay_tests.cpp:50-69` |
| 7 | 重锚定/等价性 | 旧实现做 oracle + 分派边界 side±1 | `estimator_optimization_tests.rs:61-125` |
| 8 | **正/负位移不重录制** | `reverse` 交换 reference/incoming + 同图 top / top+75 两裁剪 | 同文件 `:197-202` |
| 9 | 帧序列夹具约定 | 只认 `.png` + 帧序表（`frames.csv` 字段表 `:436-438`）—— **但必须补帧号零填充 + manifest（帧 → 已知位移）** | `scroll_4_benchmark.rs:235-268` |
| 10 | 像素断言三招 | 整图相等 / 变化比例（屏蔽 alpha）/ 特征色计数 | `scrolling_thumbnail_uia_e2e_test.cpp:753-815, :817-865` |
| 11 | 门禁 = 退出码 | 三条件合一：`checksumMatches && storageBounded && improvementPercent >= 80.0` | `scrolling_preview_benchmark.cpp:124-142` |
| 12 | 资源回落口径 | `memory.rs` 四相 + `memory_snapshot_benchmark.rs` 第五相（`snapshot-retained`）+ `README.md` 的指标定义与告警 | `benchmark-support/memory.rs`、`benchmark-support/README.md:49-70` |

**反面教材（不要抄）**：

- 单张静态图当唯一夹具（`scrolling_image_replay.cpp:130-131`）；
- **字典序当帧序**（`scroll_4_benchmark.rs:260`）；
- 基准默认输入指向库外文件（§7.7 缺陷 1）；
- 只记录 `achieved_capture_fps` 而不设下限；
- 真机 e2e 全部 `#[ignore]`（§7.7 缺陷 4）。

### 8.14 §10.2/§10.3 指标与事件：可抄的三块形状

| 块 | 参考实现 | 建议 |
|---|---|---|
| **阶段计时** | `perf.rs` 16 阶段 thread_local，**`Scope` 用 `PhantomData<Rc<()>>` 编译期禁止跨线程**；doc 原文 *"Parallel work is timed around its join, so worker CPU times are not mistaken for elapsed latency."* | 写进 `docs/19` §10.2 的指标定义：**并行工作只在 join 两侧计时** |
| **事件契约** | `screenshotscrollingdiagnostics.h`：24 事件 + 计数器（`received/accepted/timeouts/duplicates/invalid/mailboxDropped/poolUnavailable/droppedEvents`）+ 首报 5 s→之后 30 s + **每个事件标注"不证明什么"** + 隐私边界（几何只记元数据，不记像素与窗口标题） | 写进 `docs/19` §10.3 与 `docs/24` §11.2：事件表必须有**"不证明什么"列**与**隐私边界声明** |
| **决策审计** | `decisions.rs` 的 `StitchDecision`（`input_index, previous_raw_index, exact_duplicate, reference_mode, motion, confidence, accepted_offset, branch, before, after, growth, canvas_band_height, synthetic_reference_band_height, motion_diagnostics`）纯 `serde` 可序列化 | 直接作为 `docs/24` §11.2 执行记录模板的字段表 |

> 特别是 `wheel_dispatch.status == 0` 的注释：**"只表示投递成功，不表示目标应用处理了"**。`docs/24` §11.2 要求必填"位移证据"，这条正是"注入成功 ≠ 滚动成功"的准确表达。

### 8.15 上限取值：三方对照

| | **PixPin 3.5.5.1**（闭源，`docs/25`/`docs/26`/`docs/28`） | **Snow Shot**（开源，本文件） | **SnapClip `docs/19`** |
|---|---|---|---|
| 主轴高度上限 | 提示阈值 **29,000 px**；实测产物 **502,649 px** | **69,120 px** = `2160 × 32`（FFI 默认） | **30,000 px** |
| 总像素上限 | **强推断 ≈ 512 MP**（`INT32_MAX / 4`；实测产物占 99.06%） | **265,420,800 px** = `3840 × 2160 × 32` | **150 MP** |
| 上限施加点 | 未定位（判据未反查出） | **引擎无上限，完全由 FFI 施加** | 未定 |
| 超限行为 | **警告对话框 + 提示回滚一小段，会话继续** | **硬错误 `InvalidFrame` → `fatalError` → 会话终止** | **部分结果保持可导出** |
| 画布 | **单块 `Format_RGB32` 连续位图** | tile（主轴 256，横轴整宽）tile | tile 512×512 |
| 拼接引擎 | 自研半帧匹配 + 偏移候选掩码（`DetectDisplacement.cpp`） | 自研 ORB + tile 加权投票 | 规划：profile SAD 为主 |
| 编码 | 内嵌 libpng 1.6.39 + libjpeg，逐 scanline | `png` crate 流式 + 原子 `persist` | 行带 sink + `png = "0.18"` |
| 内存 | 单块 `W × H × 4` | 页后备 `RasterBuffer` + tile 租约 | 未定（普通 `Vec`） |

**结论**：

1. **SnapClip 的 30,000 px 在高度上比 Snow Shot 还小 2.3 倍，比 PixPin 的实测产物小 16.8 倍。** `docs/25` R1/R5+D2 的"上限三层取值"问题在这里得到**第二个独立参照**：Snow Shot 的 69,120 恰好是"4K 高度 × 32"，即**"32 屏"**的量级直觉；PixPin 是 **232 屏**。
2. **上限必须是可注入的策略值，且引擎层与产品层要分层**（参考实现：`DEFAULT_MAX_OUTPUT_*` 在 FFI，引擎本身无上限）。`docs/19` §8.4 应照此拆成"引擎接收 `ScrollLimits{max_output_height, max_output_pixels}`"与"应用层决定这两个值"。
3. **"超限时保留部分结果"是 SnapClip 的净增量**——Snow Shot 是硬错误直接终止，PixPin 是警告后让用户回滚继续。**三种行为各不相同，`docs/19` 的选择（部分结果可导出）是最稳妥的**，应在文档里明确写成"参考实现未做到这一点"。
4. **参考实现的三个"死配置"（`min_overlap_rows=48` / `min_overlap_ratio=0.15` / `accepted_history_capacity=4`）是一个警告**：配置项加了但没接线，比不加更糟（`docs/19` §8.4 若要暴露上限参数，必须同时有读取点与测试）。

### 8.16 内存层：建议补 `snow-memory` 那一层

`docs/19:876` 明确要求"150 MP 仅是保护阈值，不能 materialize 为连续 BGRA（约 600 MB）；CanvasStore 必须 tile 化、流式写盘，内存只保留有界 LRU"。

参考实现是**两层**：

1. **tile 化画布**（`tiled_canvas.rs`，§3.11）：主轴 256、横轴整宽；`MAX_SPARE_TILES = 2`、`MAX_SPARE_TILE_BYTES = 8 MiB`、`tile_capacity_limit = len + len/8 + 64`；`Arc` 租约 + 有租约时写时复制；
2. **页后备分配**（`snow-memory`，§4）：`MIN_PAGE_BUFFER_BYTES = 1 MiB`，≥1 MiB 走 `VirtualAlloc` / Mach VM / 匿名 `mmap`，Windows 上尝试大页；`Arc<RasterBuffer>` + `Arc::make_mut` 做 COW；`clear`/`truncate` 保留 capacity 供池复用；`try_*` 构造函数返回分配错误而不是静默退回堆。

**建议**：`docs/19` §8.4 的"有界 LRU"应细化为上面两层，并把 `MAX_SPARE_TILE_BYTES` 这类"空闲池字节上限"写进文档。这些是有界内存的真实口径——**`docs/19` 现在只有"有界 LRU"四个字**。

### 8.17 `docs/19` §13.1 唯一开放项应扩写

`docs/19` §13.1 现在唯一开放项是"canvas 落盘是否拆线程"。本次研读后，至少还应加入：

- §7.4 的周期性重锚定是否需要（keyframe ≤ 64 KB 的依据是什么）；
- §6.3 自适应 settled 与 §6.5 闭环步长是否需要（参考实现都没有）；
- §7.2 的 matcher 主路径选择（profile-SAD vs ORB，见 §8.3）；
- §5.1 的 overlay 输入模型二选一（可见+穿透 / 隐藏+controller，见 §8.11）。

---

## 9. 对 `docs/24` 的具体修订建议

（与 `docs/25` §3 的 `docs/24` 评审互补，此处只列 snow_shot 引出的增量。）

| # | `docs/24` 位置 | 现状 | 建议 |
|---|---|---|---|
| **M1** | §0.4 范围边界 | 把"OpenCV 作为 `snapclip-capture` 默认依赖"当既存事实 | **删除**。这既是事实错误（`docs/25` §3），又被参考实现反证：`orb.rs` 1,739 行纯 Rust ORB 无 OpenCV（§8.3） |
| **M2** | §S1.x（纯拼接核心） | 未提 `Contained` 与双帧分离 | 补：`ViewportState` 三分支（Append/Prepend/**Contained**）；`previous_raw` vs `motion_reference` 双帧；`comparison_reference()` 单点转换（§8.4） |
| **M3** | §S1.x | 未提行带高度 | 补：`band_height(H, shift) = (H/2).max(H/4 + shift)`（§8.8） |
| **M4** | §S1.x 的 matcher | 按 `docs/19` §7.2 的 profile-SAD 主路径 | 重新评估为 **ORB 主路径**（§8.3）；补四项量化门限 `MIN_INLIER_MATCHES=8` / `MIN_INLIER_TILES=4` / `MIN_RESIDUAL_GAIN=0.15` / `min_confidence=0.65`，以及 `MAX_CROSS_AXIS_DELTA=4` |
| **M5** | §S1.x 的 tile 权重 | 按 `docs/19` §7.3 的加法式 `band_score` | 改为**乘法式**：`learned = 1 + 1.5*(scrolling - 1/3) - (fixed - 1/3) - (dynamic - 1/3)`，`influence = (observations/3).clamp(0,1)`，`clamp(0.1, 2.0)`；歧义不学习（`direct * compensated >= 0.5` 时跳过）；观察不足保持中性 1/3（§3.9） |
| **M6** | §S0.2 冻结词表 | 冻结 `ScrollStopReason` 等 | 补：`Contained` / `Append` / `Prepend` / `Skip` / `NoMovement` 五态分支名；7 类未匹配原因；`reference_mode` 二态名（§3.1、§3.4） |
| **M7** | §S2.x 前置线程模型重构 | 按 `docs/19` §4.3 的 4 执行体 | 如实写 **5 执行体**（GUI / capture producer / producer 内部 `std::thread` / stitch worker / 原生捕获 worker）；帧信箱容量从 **1 改为 2–3 + latest-win + 显式背压信号**；补 `content_generation` 与 `is_duplicate` 的独立语义（§8.7） |
| **M8** | §S3.x 输入注入 | 未定传输路径 | 补 `wheel_dispatch` 式 **status 枚举**（≥5 态），且 0 态必须写"只表示投递成功，不表示目标处理了"；补 `SPI_GETMOUSEWHEELROUTING` 能力探测（§5.5、§8.11） |
| **M9** | §S3.6 overlay 输入隔离与 controller HWND | 自认内部矛盾 | 改为**两条方案并列**：A 可见 overlay + `WS_EX_TRANSPARENT` 穿透 + `setMask` 挖洞 + `WDA_EXCLUDEFROMCAPTURE`（可被用户设置跳过）；B 隐藏 overlay + controller HWND。**矛盾变成开关**（§8.11） |
| **M10** | §S4.1 端口形状 | 按 `docs/19` §8.3 的 `ScrollTile` 跨端口 | 改为：**tile 与 checksum 是引擎内部细节，不跨端口**；端口只有**行带**（`delta_top` + `delta_rows` + `added_rows`）+ **快照句柄**（零拷贝切片、脱离会话生命周期）+ **导出任务句柄**（异步 + 可取消 + 阶段进度 + 原子提交）（§8.9） |
| **M11** | §S4.2 PNG 编码器 | 提议 `png = "0.18"`，未核差异 | 版本选择被独立证实：`snow-crates` workspace 已用 **`png = "0.18.1"`**（§1.1）。**参考实现同样是用 `png` crate 流式编码 + 原子 `persist`**，与 `docs/24` §S4.2 的方向一致。仍需核 `docs/25` §7.2 第 ⑨ 条的 `image` vs 裸 `png` 差异 |
| **M12** | §S0.3 夹具 | "生成 → 反解" | 升级为 **"生成 → 真实拼接器反解 → 断言等于已知位移"**（夹具自证）；并补**周期重复纹理**图案（§8.13 #1/#2） |
| **M13** | §S0.4 可控窗口 | 未定 | 补输入注入 settle 纪律（按下 25 ms、拖动 50 ms + 6×25 ms；`SendInput` 绝对坐标归一化）（§8.13 #4） |
| **M14** | §S0.7 overlay spike | 只测一条方案 | 用同一夹具**同时测 A/B 两条 overlay 方案**（§8.11） |
| **M15** | §S1.x 新增门禁 | 无 | 补"3 步重锚定不变量"：`Contained → reference_mode = CanvasWindow`、最终整图逐像素相等（参考 `memory_reference_benchmark.rs:162-163`）；并补**显示器档 move-rect 参照**（§6.7）——参考实现未利用这条机制，是 SnapClip 可主动用的独立验证量 |
| **M16** | §S5.x 参数冻结 | 按 `docs/19` 的 30,000 px / 150 MP | 补第二个独立参照：Snow Shot 的 **69,120 px / 265,420,800 px**，且**上限分层**（引擎无上限、FFI 施加、应用定值）（§8.15） |
| **M17** | §11.2 执行记录模板 | 四个必填项（位移证据/停止原因/readback/是否部分结果） | 补**决策审计字段表**（`StitchDecision` 的 14 个字段）与**"不证明什么"列**、**隐私边界声明**（§8.14） |
| **M18** | §11.2 覆盖率 | 未列空白 | 补参考实现的 **10 项明确空白**（方向反转、重复纹理、懒加载、混合 DPI/跨屏负坐标、上限拒绝路径、丢帧 gap 一致性、真实录制回归、100 步漂移 ≤2 px、false acceptance 比例）（§7.3）；`docs/24` 已经要求的项目**不应因"参考实现也没做"而放松**（§7.7） |

---

## 10. 风险、未解项与自评

### 10.1 本报告的风险声明

1. **未构建、未运行、未实测。** snow-apps 需要 VS 2026 + CMake 4.2 + MSVC 14.51 + Rust 1.97.1 + 仓库托管 vcpkg/Qt，本机不具备。**本报告不含任何 snow-apps 实测数据**；所有性能数字都是代码中的门槛/参数，不是结果。
2. **`refer/snow-apps` 的滚动截图链路在干净 checkout 上不可运行**（§7.7 缺陷 1 与 2）。这意味着**参考实现的所有性能声明都无法在我这里被复核**，也无法成为 `docs/24` §S5.0 的"对标基线"。
3. **`docs/Temp/snow-*-deep.md` 四份笔记是本报告的一手证据**，均为子代理只读取证；凡本报告引用的 `文件:行号` 都可在本地复核。凡由本人第一手读过的部分在 §0.1 已分级标注。

### 10.2 未解项

| # | 未解项 | 影响 | 建议 |
|---|---|---|---|
| **U1** | **Snow Shot 的 ORB 匹配质量无实测基线**（`test-imgs/scrollscreenshot-test.png` 缺失、无滚动基线文件、无 false acceptance 统计） | 无法回答"ORB 在 SnapClip 的目标场景（浏览器长页面 / 代码 / 表格 / 聊天记录）上的接受率与误接受率" | 这是 §8.3 建议的最大不确定性。**建议在 `docs/24` §S1.x 里加一个专门的 spike**：用 `docs/24` §S0.3 的夹具（含周期重复纹理）对 **profile-SAD vs ORB** 做污染对比，用数据决定主路径，而不是靠推理 |
| **U2** | **Snow Shot 的 `Synthetic` 参照系累积漂移量级未知**（子代理 A 的 S2） | `docs/19` §7.4 的 `DriftBeyondBudget` 阈值是否合理、`Contained → CanvasWindow` 是否足够 | 参考实现零用例（无"100 步漂移"测试）。**`docs/24` §S1.x 应补这条测试**，它同时也是 §7.4 的验收 |
| **U3** | **水平轴性能未知**（子代理 A 的 S7：只有垂直轴能整块 `memcpy`） | `docs/19` 的双向支持里，横向的实际成本没有参照 | 参考实现的水平基准（`scroll_region_benchmark.rs`）**无阈值**，等于没有基线 |
| **U4** | **WGC 脏区域的 `OrderedIncremental` 在滚动时是否真的可用** | 影响 `docs/19` §4.2 的"GPU 线程回读"能否只回读条带 | `tests/wgc_scrolling_e2e.rs` 5 个测试全 `#[ignore]`，需真实交互桌面；且 `update.rs:697` 的测试**证明只补条带会出错** → 需要 `GetFrameMoveRects` 或整体重建。**这条是 `docs/19` §4.2/§4.5 的实质风险** |
| **U5** | **许可传染的最终判断** | 若 SnapClip 真要复用 `snow-crates` 的 Apache-2.0 代码 | 建议先做**一次纯设计吸收**（本报告即此定位），真正复用留到有明确收益时再单独评估 NOTICE/归属成本 |
| **U6** | **Snow Shot 的 macOS 滚动截图实现路径** | `docs/19` 是 Windows-only，不影响 | `macos_scrolling_input_tests.mm` 133 行只有鼠标透传，**macOS 无滚动拼接 e2e**；`snow-crates` 有 `snow-macos`/`snow-media` 但未纳入本次范围 |

### 10.3 自评：本次研读对 SnapClip 的三条最重要结论

1. **`docs/19` 的大方向被一个成熟开源实现逐条印证**（活动帧源 + 位移匹配 + union 画布 + tile 化 + 行带导出 + 预览 patch + 悬停 revision + 自适应 cadence）。`docs/19` 原有 5 条 snow_shot 引用**全部准确**。**不需要推翻设计。**
2. **需要改的是三处"主次关系"**：
   - **ORB 从最后手段提到主路径**（依赖顾虑已被 `orb.rs` 消解）；
   - **`Contained`/双帧分离/参照系二态从缺失补进来**（`docs/19` §8.1 与 §7.2）；
   - **tile 与编码器从跨端口收回到引擎内部，端口只出行带**（`docs/25` R3 已被参考实现完全印证）。
3. **`docs/19` 有三处是参考实现没有的净增量，应保留并明确标注**：
   - **超限时保留部分结果**（Snow Shot 是硬错误终止、PixPin 是警告后回滚继续）；
   - **周期性全局重锚定 + `DriftBeyondBudget`**（参考实现只有 `Contained → CanvasWindow`）；
   - **`valid_pixels < minimum` 必须 `Uncertain`** 的安全阀；
   - 以及 `docs/25` R12 的两条（`IGraphicsCaptureSession2/3` 能力探测、WGC 边框色带的像素断言）。

---

## 附录 A 关键数值总表

| 类别 | 值 | 位置 |
|---|---|---|
| 画布 tile 主轴跨度 | 256 | `tiled_canvas.rs:7-18` |
| 画布空闲池 | `MAX_SPARE_TILES = 2`、`MAX_SPARE_TILE_BYTES = 8 MiB` | 同上 |
| tile 容量余量 | `len + len/8 + 64` | 同上 |
| 预览 tile | 交叉轴 128、主轴 256、最大预览主轴 640 | `thumbnailwidget.cpp:23-30` |
| 估计 tile | 32 × 32 | `region.rs:24-30` |
| 估计降采样 | `DOWNSAMPLE = 4` | `region.rs:21` |
| 特征预算 | `max_features = 2500`、`MAX_FEATURES_PER_TILE = 8` | `types.rs:170-188`、`estimator.rs:12-20` |
| 匹配门限 | `LOWE_RATIO = 0.8`、`MAX_HAMMING_DISTANCE = 64.0` | `estimator.rs:12-20` |
| 接受门限 | `MIN_INLIER_MATCHES = 8`、`MIN_INLIER_TILES = 4`、`MIN_RESIDUAL_GAIN = 0.15`、`min_confidence = 0.65` | 同上 |
| 位移上限 | `max_motion_ratio = 0.6`（× 视口主轴长度） | `types.rs:170-188` |
| 横轴漂移容忍 | `MAX_CROSS_AXIS_DELTA = 4` | `estimator.rs:12-20` |
| 内点容忍 | `INLIER_TOLERANCE = 2` | 同上 |
| 候选数上限 | `MAX_CANDIDATES = 8` | 同上 |
| 时序学习率 | `temporal_learning_rate = 0.2`；衰减 0.05；歧义阈值 `direct*compensated >= 0.5` | `types.rs:170-188`、`region.rs:814-816` |
| 区域权重 | `clamp(0.1, 2.0)`；`influence = (observations/3).clamp(0,1)` | `region.rs:779-788` |
| 行带高度 | `(H/2).max(H/4 + shift)` | `compositor.rs:8-27` |
| 上限（FFI 默认） | `max_output_height = 69,120`；`max_output_pixels = 265,420,800` | `snow-stitch-images-c/src/lib.rs:20, :67` |
| 死配置 | `min_overlap_rows = 48`；`min_overlap_ratio = 0.15`；`accepted_history_capacity = 4` | 同上 `:68-70` |
| 帧信箱容量 | 2（latest-win，无 Condvar） | `latestbridgemailbox.h:27-89` |
| 帧信箱超时 | `receive(100)` = 100 ms | `pipeline.cpp:526` |
| WGC 帧池 | 3 | `wgc.rs:60-68` |
| WGC 传输队列 | 32 | `wgc.rs:648` |
| WGC 熔断 | `WGC_ORDERED_FAULT_LIMIT = 3` | `wgc.rs:486-534` |
| WGC 超时 | 帧 250 ms / 快照新鲜 2 ms / 连续新鲜 1 ms / worker 启动 10 s | `wgc.rs:60-68` |
| 快照预算 | `SNAPSHOT_ACQUISITION_BUDGET = 750 ms` | `mod.rs:67` |
| 读回 | `READBACK_SLOT_COUNT = 3`、`QUERY_WAIT_TIMEOUT = 250 ms`、`QUERY_SPIN_POLLS = 16` | `readback.rs:20-23` |
| DXGI dirty 阈值 | `max_rects 192/64/8`、面积 `70%/45%/18%` | `duplication.rs:135-142` |
| 脏矩形上限 | `MAX_ORDERED_DIRTY_REGIONS = 65,536`、`DIRTY_REGION_BATCH = 64` | `wgc/update.rs:418-453` |
| 节拍 | `minimumFps 1` / `maximumFps 30` / `initialFps 30` / headroom 1.25 / EWMA 0.25 / `recoverySamples 4` / `pressureQueueDepth 2`；压力折扣 0.75 | `adaptivescrollingcapturecadence.h` |
| 原生流 | `target_fps 30 / min_fps 1 / buffer_depth 3 / max_consecutive_errors 30 / capture_retry_count 1 / adaptive_fps 1` | `screenshotscrollingnativesource.cpp:121-131` |
| 自动滚动 | 固定 `120`（一个 `WHEEL_DELTA`）；间隔默认 200 ms，clamp `[128, 1000]` | `screenshotscrollingautoscroller.h` |
| stitch 帧池 | 6 | `pipeline.cpp:469` |
| 首帧预览看门狗 | 5000 ms（只记日志） | `screenshotscrollingcapturecontroller.cpp:140` |
| 诊断节奏 | 首报 5 s → 之后每 30 s | `screenshotscrollingdiagnostics.h:39-44` |
| 合成事件退避 | `min(1000, 50 << min(attempt, 5))` ms | `screenshotscrollingcapturecontroller.cpp:599-603` |
| PNG 导出 | strip = 64 行；`sync_channel(16)` | `snow-stitch-images-c/src/lib.rs:746-846` |
| e2e 阈值 | `SCROLL_ROWS 24 / BAND_HEIGHT 8 / PIXEL_TOLERANCE 3 / REQUIRED_SHIFT_MATCH 0.97 / MAX_STALE_MATCH 0.35` | `snow-capture/tests/wgc_scrolling_e2e.rs:32-43` |
| 内存 | `MIN_PAGE_BUFFER_BYTES = 1 MiB` | `snow-memory/src/lib.rs:14` |
| 内存基准 | warmup 3 → 采样 31；checksum 每 4096 字节 | `benchmark-support/memory.rs:129-167` |
| ORB | `SCALE_FACTOR 1.2 / LEVELS 8 / EDGE_THRESHOLD 31 / PATCH_SIZE 31 / FAST_THRESHOLD 20 / HARRIS_K 0.04 / HARRIS_BLOCK_SIZE 7` | `orb.rs:29-47` |

---

## 附录 B 结论强度总表

| # | 结论 | 强度 | 依据 |
|---|---|---|---|
| B1 | Snow Shot 的滚动截图是"活动帧源 + 位移匹配 + union 画布 + tile + 行带导出" | **确证** | 本人精读 `stitcher.rs` / `state.rs` / `tiled_canvas.rs` / C ABI 头 |
| B2 | 拼接引擎是**纯 CPU、零平台依赖、零 OpenCV** 的 Rust crate | **确证** | `snow-stitch-images/Cargo.toml` + `orb.rs` 1,739 行 |
| B3 | ORB 是**唯一主路径**，无 SAD / NCC / phaseCorrelate | **确证** | `estimator.rs` 全文 |
| B4 | 参考实现**没有 settled 等待、没有闭环步长、没有终点判定、没有尺寸上限** | **确证** | 本人 grep `settle/stability/consecutive/identical/unchanged/idle`、`limit/maximum/max_canvas/boundary/exhaust` 均零命中 |
| B5 | `Contained` 是独立分支；参照系二态 `Synthetic`/`CanvasWindow` | **确证** | `state.rs:19-84`、`stitcher.rs:472-474` |
| B6 | 双帧分离（`previous_raw` vs `motion_reference`） | **确证** | `stitcher.rs:289-306` + `non_skip_advances_previous_raw_even_without_motion` 测试 |
| B7 | 超限是**硬错误**，会话终止 | **确证** | `snow-stitch-images-c/src/lib.rs:421-432` + `pipeline.cpp` fatalError + `capturecontroller.cpp` grep |
| B8 | 引擎无上限、上限完全由 FFI 施加 | **确证** | 同上 |
| B9 | 三个配置字段是死配置 | **确证** | 全文无其它读取点（子代理 A 逐处核对，本人复核区间校验与转换点） |
| B10 | `MotionStage::NoMatches` 不可达 → FFI 的 `INSUFFICIENT_OVERLAP` 上层收不到 | **确证** | `estimator.rs:1136-1138` 先判 `EmptyDescriptors` |
| B11 | 输入穿透只翻 `WS_EX_TRANSPARENT` 一位，**不隐藏 overlay** | **确证** | `windowchrome.cpp:259-292`、`overlaywindow.cpp:565-574` 注释原文 |
| B12 | 三层输入隔离（透明位 / `setMask` 挖洞 / `WDA_EXCLUDEFROMCAPTURE`），且排除可被用户设置跳过 | **确证** | `windowchrome.cpp:241-257`、`overlaywindow.cpp:887-911`、`settingscatalog.cpp:1324-1332` |
| B13 | 注入传输是 `PostMessageW(WM_MOUSEWHEEL)` + `ChildWindowFromPointEx` 下沉 | **确证** | `scrollinput.cpp` |
| B14 | 自动滚动是固定 120 + 固定间隔的**开环**注入 | **确证** | `screenshotscrollingautoscroller.h`、`scrollingstepinput.h` |
| B15 | 节拍是 EWMA + 队列压力 + 连续 4 样本恢复的自适应帧率 | **确证** | `adaptivescrollingcapturecadence.h` 全文 |
| B16 | 预览是 6 态 + `replacedPreviewRows` 吸收缩放取整漂移 | **确证** | `thumbnailwidget.cpp:186-272`、`pipeline.cpp:343-383` |
| B17 | hover 单飞 + 四重过期校验 + 先 pause 等 ack | **确证** | `scrollinghoverpreview.h` 全文 |
| B18 | DXGI `GetFrameMoveRects` 被用于位块搬移重建，且**捕获层不知道滚动截图的存在** | **确证** | `duplication.rs:849-891` / `:585-642`；grep 结果 |
| B19 | 窗口目标的后端优先级是 **[WGC, DXGI, GDI]**（WGC 优先） | **确证** | `platform/windows/mod.rs:68-72` |
| B20 | 能力探测是假的（`hdr_capture`/`window_enumeration` 写死，不探测 Session2/3、不探测 WDA） | **确证** | `capabilities.rs:37-59` |
| B21 | 会话选项禁光标/边框**被调用**，但错误被丢弃、无降级分支 | **确证** | `wgc.rs:636-637` |
| B22 | `content_generation` 与 `is_duplicate` 是两个独立信号 | **确证** | `frame.rs:64-66` 注释原文 + `streaming.rs:820-825` |
| B23 | 快照零拷贝租约、脱离会话生命周期 | **确证** | `snow_stitch_images.h` 注释原文 |
| B24 | 导出一致：异步任务 + 可取消 + 阶段进度 + 原子 `persist` | **确证** | `snow-stitch-images-c/src/lib.rs:746-846` |
| B25 | `snow-memory` 的 ≥1 MiB 页后备 + `Arc` COW + `try_*` 返回分配错误 | **确证** | `snow-memory/README.md` + `src/lib.rs:14` |
| B26 | 夹具自证（真实拼接器反解已知位移） | **确证** | `scrolling_thumbnail_uia_e2e_test.cpp:697-751` |
| B27 | 仓库级缺陷 4 条（缺失 PNG、不存在的 example、未设 label、诊断无覆盖） | **确证** | 子代理 D 的 grep + `git ls-files` |
| B28 | **`docs/19:132-136` 的 5 条 snow_shot 引用全部属实** | **确证** | 逐条源码核对（§8.1） |
| B29 | 线程拓扑是 **5 个执行体**（含 producer 内部 `std::thread`） | **确证** | `pipeline.cpp:483/703/704` |
| B30 | `stop()` 不能唤醒阻塞的 `receive`（注释与实现不符） | **子代理确证** | `pipeline.h:44` vs `streaming.rs:350-352` vs `stream_queue.rs:132/150` vs `pipeline.cpp:488-498`；**本人未逐行复核** |
| B31 | 水平轴明显更慢（无就地快速路径） | **子代理确证** | `compositor.rs:464-500`、`tiled_canvas.rs:654-763`；**本人未实测** |
| B32 | `Synthetic` 参照系有累积漂移 | **子代理确证** | `stitcher.rs` 的合成路径 + `Contained` 唯一复位；**本人未实测漂移量级** |
| B33 | 滚轮注入 **bypasses UIPI** | **确证（外部）** | Microsoft Learn `winapp-cli` 原文（`docs/25` R15）+ snow_shot 生产实现 |
| B34 | Snow Shot 的 ORB 匹配质量 | **未验证** | 无基线、无 false acceptance 统计、夹具图缺失 |
| B35 | Snow Shot 的滚动截图在干净 checkout 上可运行 | **证伪** | §7.7 缺陷 1 与 2 |

---

## 附录 C 证据附录清单

| 文件 | 内容 | 规模 |
|---|---|---|
| `docs/Temp/snow-stitch-images-deep.md` | Rust 拼接引擎逐模块取证：轴抽象、Frame/坐标系、默认选项、位移估计流水线常量、候选投票、ORB 常量与复刻细节、region 三分类权重、compositor band 公式、tiled_canvas 常量与租约、stitcher 状态机、perf 16 阶段、导出实现、snow 侧弱点 S1–S11 | 994 行 |
| `docs/Temp/snow-shot-cpp-scrolling-deep.md` | C++/Qt 上层管线：5 执行体、`LatestBridgeMailbox`、节拍闭环、两个代次号、pause 三段屏障 ack、preview patch 与 `replacedPreviewRows`、缩略图控件数值、三层输入隔离、现场自检探针、快照/导出/所有权移交（`detach`）、其余数值 | 994 行 / 79.6 KB |
| `docs/Temp/snow-capture-wgc-deep.md` | 捕获层：后端优先级与降级白名单、`capabilities` 假探测、WGC 会话参数、空脏矩形语义、条带重建反例、**DXGI `GetFrameMoveRects`**、DXGI 窗口单屏限制、读回 3 slot 与懒提交、脏矩形越界拒绝、帧池溢出两相位、`content_generation`、`wgc_scrolling_e2e` 阈值 | 2,052 行 / 136 KB |
| `docs/Temp/snow-scrolling-tests-deep.md` | 测试/基准/诊断：全套件清单、四种夹具方法论、覆盖矩阵、基准硬门槛表、内存基准基架与 `README` 方法论、`scrolling.*` 24 事件 + 1 counter、`wheel_dispatch` 五态、仓库级缺陷 4 条、12 条可借鉴形态 | 418 行 / 55 KB |

**本文件**（`docs/29-snow-shot-scroll-capture-study.md`）与 `docs/25`（PixPin 对标）、`docs/26`（PixPin 静态取证）、`docs/27`（参考实现调研）、`docs/28`（PixPin 运行期数据与日志）共同构成本轮滚动截图设计的证据基础。
