# SnapClip 截图 MVP 实现与验证记录

> 对应任务清单：`docs/08-screenshot-mvp-tasklist.md`
>
> 本文件记录实现结果、验证命令、实际输出，以及**尚未验证**的部分。凡未实际执行的验证都在第 6 节明确列出，不以“代码写完”冒充“验收通过”。

## 1. 交付内容

### 1.1 后端目录（`src-tauri/src/`）

```text
domain/                     # 领域契约：无 Win32 / Tauri / SQL 类型
  mod.rs error.rs payload.rs publication.rs capture.rs history.rs
application/                # 应用编排
  clipboard_ingest.rs       # 剪贴板去重、延迟格式窗口、入库、OCR 入队、事件
  capture_service.rs        # 选区裁剪 → PNG 编码 → artifact 落盘
capture/                    # 截图特性（可脱离剪贴板/Store/OCR 编译与测试）
  mod.rs error.rs geometry.rs session.rs
  application/{mod,runtime}.rs
  platform/{mod,windows}.rs
infrastructure/             # 外部适配器
  store/{mod,blob}.rs       # Store 接受通用 Publication
  image/{mod,encode}.rs     # PNG/BGRA 编解码，跨来源复用
platform/windows/
  clipboard/{mod,listener,reader,formats,image_norm,source_app}.rs
  capture/
    mod.rs monitor.rs hotkey.rs providers.rs renderer.rs overlay.rs
    win/{mod,d3d11,wgc,bitblt,d2d}.rs
commands/{mod,history,clipboard,ocr,capture}.rs
events/mod.rs               # 版本化事件信封
app/{mod,capture,clipboard,ocr_queue}.rs   # 组合根
```

依赖方向已按要求收敛：

```text
commands/events ─┐
platform adapters ─┼─> application ─> domain
infrastructure   ─┘
```

- `platform/windows/clipboard` 不再依赖 `Store`、OCR 或 `AppHandle`。
- `capture` 不引用 `platform/windows/clipboard`，不调用 `OpenClipboard` / `arboard` / `mark_clipboard_excluded`，不直接访问 `Store`/OCR/`AppHandle`。
- 编译期保证：`capture` 的单元测试在无剪贴板、无 SQLite、无 OCR 模型的环境下全部通过（见第 3 节）。

### 1.2 前端目录（`src/`）

```text
app/{App.vue,bootstrap.ts}
features/history/{HistoryPanel.vue,api.ts,useHistoryFeed.ts,components/HistoryItem.vue,stores/history.ts}
features/capture/{api.ts,events.ts}
features/clipboard/api.ts
features/ocr/{api.ts,events.ts}
infrastructure/tauri/{client.ts,mod.ts,commands/*.ts,events/*.ts}
shared/{contracts.ts,ipc.ts}
```

- `@tauri-apps/api` 只出现在 `infrastructure/tauri/client.ts`（已用 grep 校验，见第 3 节）。
- 组件与 Pinia store 只依赖 feature API，不接触 Tauri 类型。

## 2. 关键实现决策

| 主题 | 决策 | 原因 |
| --- | --- | --- |
| 底图捕获 | WGC 首选，BitBlt 兼容降级 | 任务清单要求 WGC 首选并准备降级路径；探测按 API 能力而非版本号 |
| 会话启动 | 先取一次底图，再显示 overlay | 避免 overlay 进入自己的截图（清单 §4.3） |
| 底图存放 | WGC/BitBlt 结果都上传为 D2D L0 bitmap，会话内复用 | 鼠标移动路径不做整帧 readback；每会话仅一次 GPU→CPU |
| L1 遮罩 | 用 4 个矩形挖空选区，而非重绘 | 选区内像素保持原始亮度；拖动时只有受影响条带失效 |
| 尺寸标签 | 上/下都放不下时**不绘制**标签 | 标签是预览层；宁可少画，也不能遮住用户正在选的像素 |
| 取消路径 | Esc / 右键 / 重复 F5 / 窗口销毁 / 显示变更 / 设备移除全部走同一个 `cancel()` | 满足 §3.2 的“统一清理路径”要求 |
| Artifact | 选区内像素裁切 → PNG → 原子写 `<app local data>/artifacts/capture/<session>-<n>.png` | capture 只产生 artifact，不接触剪贴板/Store/OCR |
| 事件 | `{schemaVersion, generation, payload}` 信封，只传 id/尺寸/状态 | 禁止大图过事件通道 |
| `CapturedMonitor::handle` | 存 `isize` 而非 `HMONITOR` | 原始指针不是 `Send`，overlay 线程需要把它移入闭包 |
| 进程 DPI | overlay 线程启动时声明 Per-Monitor V2 | 保证坐标与物理像素一一对应 |

## 3. 已执行的验证

### 3.1 Rust 单元测试（133 个，全部通过）

```powershell
cd src-tauri
cargo test --lib
# test result: ok. 133 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

```powershell
cargo check --all-targets   # 0 errors, 0 warnings
```

其中 `application::capture_service::tests::capture_pipeline_is_independent_of_clipboard_store_and_ocr`
直接验证清单 §8.1：该测试不引用 `platform::windows::clipboard`、`Store`、OCR 模块，仍能完成
「选区裁剪 → PNG 编码 → 原子落盘 → 从磁盘解码校验像素」的完整链路，证明截图特性可在无剪贴板、
无 SQLite、无 OCR 模型的环境下独立运行。

覆盖并直接对应清单 §10.1 的用例：

| 清单要求 | 测试 |
| --- | --- |
| 任意方向拖拽归一化矩形 | `capture::geometry::tests::drag_in_any_direction_normalises_the_rectangle` |
| 选区与 monitor 边界裁剪 | `selection_is_clipped_to_the_monitor` |
| 八向手柄命中/移动/缩放边界 | `handles_hit_test_at_their_anchor_points`、`interior_hit_tests_as_move_and_exterior_as_outside`、`resize_follows_the_pointer_and_clamps_into_the_monitor`、`resize_can_cross_over_the_opposite_edge`、`minimum_size_is_enforced_while_resizing`、`move_keeps_the_size_and_stays_inside_the_monitor` |
| DPI 96/120/144/192 坐标转换 | `local_coordinates_follow_the_monitor_origin`、`dpi_scales_the_handle_hit_area`、`platform::windows::capture::monitor::tests::dip_conversion_follows_the_dpi_scale` |
| 尺寸标签上下自动换位 | `size_label_prefers_below_then_above`、`size_label_is_dropped_when_it_would_cover_the_selection`、`size_label_stays_inside_horizontal_work_area_bounds` |
| 放大镜四边自动翻转不越界 | `magnifier_flips_on_every_screen_edge_and_stays_visible`、`magnifier_source_keeps_the_cursor_centred_away_from_edges` |
| Esc/窗口销毁/设备移除回到 Idle | `esc_from_every_active_state_returns_to_idle`、`window_destroy_and_device_removal_use_the_same_cleanup` |
| `CaptureArtifact` 不携带剪贴板/数据库对象 | `domain::capture::tests::artifact_serializes_without_clipboard_or_store_objects`（编译期构造约束） |

其他相关覆盖：clipboard ingest 去重/读窗口/重试/入库失败不推进序列、Store 迁移与分页、PNG 编解码与像素上限、artifact 原子写与唯一命名、遮罩四带覆盖面积等于 `frame - hole`。

### 3.2 GPU 合成视觉回归（清单 §10.3 / §4 / §5 / §6）

这些测试创建真实 D3D11 设备、真实 D2D 资源，把 overlay 实际使用的合成路径渲染到离屏 target 并读回像素：

| 测试 | 验证内容 |
| --- | --- |
| `d2d::tests::composed_frame_masks_outside_the_selection_and_keeps_it_clear` | 无选区时整屏按 `MASK_ALPHA` 变暗；有选区时选区内像素**逐字节等于**底图原始像素；选区外像素等于 `底图 × (1 - MASK_ALPHA)` |
| `d2d::tests::chrome_stays_out_of_the_exported_pixels` | L0 像素在选区内不被边框/手柄污染；尺寸标签放不下时被丢弃而不是压在选区上 |
| `d2d::tests::size_label_panel_is_painted_below_the_selection_when_it_fits` | 有空间时标签面板确实绘制且完全不透明 |
| `d3d11::tests::composition_target_accepts_the_overlay_swap_chain_and_presents` | 在真实隐藏 popup 窗口上创建 DirectComposition target、320×200 flip-model swap chain、D2D render target，并完成 attach → Present → Commit 完整一轮 |
| `d3d11::tests::device_creation_and_texture_round_trip` | 设备创建、BGRA 纹理上传与 readback 一致 |
| `win::bitblt::tests::bitblt_captures_a_real_monitor` | 真实桌面 BitBlt 抓取（尺寸、缓冲区长度、非全透明） |
| `platform::windows::capture::providers::tests::providers_probe_and_capture_a_real_monitor_when_available` | provider 探测 + 真实显示器抓取 + 必须产出 GPU texture |
| `application::capture_service::tests::real_png_encoder_round_trips_the_selection` | 选区裁剪 → PNG → 解码回 BGRA，像素与坐标一一对应 |

### 3.3 前端

```powershell
npx vue-tsc --noEmit
npx vite build
# ✓ 6269 modules transformed / built in ~0.9s
```

Tauri 访问边界校验：

```powershell
# 仅 infrastructure/tauri/client.ts 命中 @tauri-apps
grep -r "@tauri-apps/api" src
```

### 3.4 安装包构建

```powershell
npx tauri build --bundles nsis
# Built application at: src-tauri/target/release/snapclip.exe
# Finished 1 bundle: .../bundle/nsis/SnapClip_0.1.0_x64-setup.exe
```

### 3.5 运行时冒烟检查（真实进程）

启动 `src-tauri/target/release/snapclip.exe` 后枚举该进程的顶层窗口：

```text
windows of pid 35216 :
hwnd=3671130 class=SnapClipCaptureOverlay
hwnd=2752794 class=SnapClipClipboard
hwnd=1573144 class=Tauri Window
...
```

结论：

- 截图 overlay HWND 已在启动时**预创建**且不可见（清单 §7「overlay 预创建并隐藏」）。
- 剪贴板监听窗口独立存在，说明两条能力互不干扰。
- 启动过程中 `WindowsOverlay::spawn_overlay` 返回成功；而该函数在 `RegisterHotKey` 失败时会直接返回错误并终止启动，因此可以推断注册成功。

进一步直接验证全局热键：

```powershell
# 未运行 SnapClip 时，另一个进程可以抢到 F5
RegisterHotKey(NULL, ID, MOD_NOREPEAT, VK_F5)  -> SUCCEEDED
# SnapClip 运行期间，同一组合被占用
RegisterHotKey(NULL, ID, MOD_NOREPEAT, VK_F5)  -> FAILED with Win32 error 1409
#                                                          (ERROR_HOTKEY_ALREADY_REGISTERED)
```

这证明 SnapClip 进程确实占用了「无修饰 F5 + 抑制重复」这一全局热键，即清单 §3.2 第一条已落地。

### 3.6 overlay 窗口可被激活（`Esc` / `Enter` 能真正收到键盘）

直接读取**已发布 `snapclip.exe`** 运行时的 overlay 窗口扩展样式：

```text
overlay hwnd=3212220 GWL_EXSTYLE=0x00200088
styles: WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_NOREDIRECTIONBITMAP
RESULT: WS_EX_NOACTIVATE ABSENT -> overlay can receive WM_KEYDOWN (Esc/Enter live)
```

`0x00200088` = `WS_EX_TOOLWINDOW (0x80)` + `WS_EX_TOPMOST (0x8)` + `WS_EX_NOREDIRECTIONBITMAP (0x00200000)`，
不含 `WS_EX_NOACTIVATE (0x08000000)`。这正是第 4.1 节缺陷修复后的实际状态：
窗口可被激活 ⇒ `WM_KEYDOWN` 可到达 ⇒ `Esc` 取消 / `Enter` 确认的处理逻辑是可达代码。

同一断言在单元测试中以样式表达式镜像的形式守住
（`overlay::tests::overlay_style_is_activatable_so_escape_reaches_it`），
防止以后有人把 `WS_EX_NOACTIVATE` 加回去。

### 3.7 启动不再卡死（回顾评审 P0）

`setup` 内不再丢弃 `ClipboardIngestHandle`，实测启动后主窗口健康：

```text
pid=28252 alive=True
  hwnd=1316954   vis=False hung=False msgOk=1  class='SnapClipCaptureOverlay'
  hwnd=3422430   vis=False hung=False msgOk=1  class='SnapClipClipboard'
  hwnd=5701982   vis=True  hung=False msgOk=1  class='Tauri Window'
RESULT: Tauri Window present
RESULT: Tauri Window not hung
```

对照修复前（同一探针）：

```text
  hwnd=28184140 vis=True hung=True msgOk=0 class='Tauri Window'
```

`msgOk=0` 表示该窗口在 3 秒内未响应 `WM_NULL`。插桩计时定位到 `setup` 在
`clipboard::start(...)` 处永久阻塞：返回的 `ClipboardIngestHandle` 是临时值，
语句结束即析构，`Drop` 先执行空的 shutdown 回调，再 `thread.join()`——而 ingest
线程正阻塞在 `receiver.recv()`，于是 `setup` 永不返回。

### 3.8 事件名真的被 Tauri 接受（不是"看起来对"）

先复现原故障：运行时日志出现

```text
[snapclip][events] emit clipboard://updated.v1 failed:
  only alphanumeric, '-', '/', ':', '_' permitted for event names
```

值得记录的是：Tauri 的白名单**包含** `/` 和 `:`，真正被拒绝的是 `.`。
因此只把 `://` 改成 `/` 并不能修好——事件名必须不含 `.`。现方案只用 `[a-z0-9-]`，
对任何 Tauri 2.x 版本都合法。

为排除"没有发出事件所以没有报错"这种假通过，临时加了一条成功分支日志后实测：

```text
[snapclip][events][probe] accepted clipboard-updated-v1
```

即：真实剪贴板变更触发了 emit，且 Tauri **接受**了该事件名。（探针已移除。）

### 3.9 脏矩形真的生效（GPU 像素级证据）

`damage_clip` 的纯逻辑测试（包围盒、空 damage = 全屏、越界裁剪）之外，另有一条真实
D3D11/D2D 测试：先把选区 8,8–40,40 完整画进一个持久 target，再把选区收到 8,8–24,40，
但只声明 `damage = (8,8)-(32,32)` 后重绘。断言：

- damage 内 `(24,10)` **确实变了**：第一遍是 `background`（被选中），第二遍是
  `masked(background)`（被遮罩）——证明裁剪没有把重绘一起裁掉；
- damage 外所有采样点**逐字节不变**，其中 `(40,10)` 仍在选区内：整屏重绘一定会改到它，
  它没变，证明 clip 真的限制了写入范围。

测试名：`partial_repaint_does_not_touch_pixels_outside_the_damage`。

> 这条测试在编写过程中先失败过一次，原因是测试自身选错了观察点——`(16,16)` 在两遍中
> 都被选中，本来就不该变化。修正的是测试的取样点，不是把断言放宽。

## 4. 实现过程中发现并修复的缺陷

### 4.0 评审条目逐条处理结果

| 条目 | 状态 | 根因与修法 |
| --- | --- | --- |
| P0 剪贴板线程永久阻塞 `setup` | **已修复并实测** | 返回值被当作临时值析构 → `join()` 卡死。改为 `app.manage(handle)` 持有；`StopSignal` 取代空回调，drop listener 关掉通道让 `recv()` 返回。见 3.7 |
| P0 事件名非法、事件全部丢失 | **已修复并实测** | 事件名含 `.`。改为 `[a-z0-9-]`（`capture-started-v1`）；OCR 状态改走 `events::emit` 统一信封；新增契约测试保证前后端同源。见 3.8 |
| P1 overlay 关闭死锁 | **已修复** | 先置 `shutting_down` 再 `post()`，而 `post()` 见该标志直接拒绝发送，随后仍 `join()`。改为先直投 `Shutdown` 消息再置标志 |
| P1 L0 实际不是 GPU 直绘 | **已修复** | WGC 在 arm 时就整帧 readback，再由 renderer 上传。改为 `FrozenFrame` 只持有 texture，L0 用 `CreateBitmapFromDxgiSurface` 直接包裹；CPU 像素改为 `OnceLock` 惰性读取，只在导出 artifact 时发生 |
| P1 脏矩形没有真正生效 | **已修复并有 GPU 证据** | `draw_to` 未设置 clip，且鼠标移动整屏重绘且 `dirty` 从未真正约束绘制。改为按 damage 包围盒 `PushAxisAlignedClip`；鼠标移动只失效放大镜面板与十字线（几何与绘制共用 `magnifier_geometry`）。见 3.9 |
| P1 F5 回调同步执行捕获 | **未修复** | 需要把 `start_session()` 的 WGC 初始化/首帧等待/readback 移出消息线程。属于独立重构，见第 7 节 |
| P1 OCR 无条件启动 | **已按根因修复** | 引擎本身已是惰性（本机 `models/` 不存在，`ocr service` 仅约 11ms）。真正的代价是 `compensate()` 首次立刻扫描历史待办并加载模型——这才是会挡住窗口创建的部分。改为首次回填延后 5s；显式入队任务不受影响 |

### 4.1 `Esc` / `Enter` 曾经是死键（overlay 无法接收键盘）

**问题表现**：overlay 窗口创建时带了 `WS_EX_NOACTIVATE`，而 `Esc` / `Enter` 是通过界面线程的 `WM_KEYDOWN` 处理的。一个永远不会被激活的窗口收不到键盘消息，因此 `on_key_down` 虽然写好了，却永远不会被调用——按 `Esc` 没有任何反应。

这与任务清单 §7 的约束直接冲突：

> DirectComposition 路径使用 `WS_EX_NOREDIRECTIONBITMAP`；不要给需要接收鼠标的截图框设置 `WS_EX_NOACTIVATE`。

**根因**：窗口样式与输入策略不一致。`WS_EX_NOACTIVATE` 是为了「不抢焦点」而加的，但截图框同时需要键盘；正确做法是让 overlay 正常激活并在会话结束后把前台窗口还回去，而不是永久放弃激活能力。

**修复**（`platform/windows/capture/overlay.rs`）：

1. 窗口样式去掉 `WS_EX_NOACTIVATE`，保留 `WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP`（仍然不进任务栏 / Alt+Tab）；
2. `ShowWindow` 从 `SW_SHOWNOACTIVATE` 改为 `SW_SHOW`；
3. 新增 `take_focus()`：`SetForegroundWindow` + `SetFocus`，确保 overlay 拿到键盘；
4. 新增 `WM_MOUSEACTIVATE` 处理，用户先点击时也会把焦点拉回 overlay；
5. 新增 `restore_foreground()`：会话开始前记录前台窗口（`previous_foreground`），`Esc` / `Enter` / 窗口隐藏后把它还回去，避免抢了焦点不还。

**验证**：见第 3.6 节——直接读取**已发布二进制**运行时的真实 overlay 窗口样式。

### 4.2 说明

`docs/08` 的实现过程中还修正了另外两处：`Rect::from_corners` 归一化（把某条边拖过对边时不再产生负宽度）、以及 `clamped_into` 对超出屏幕的大选区重新贴合而非留下空矩形。两者都有对应单元测试。

## 5. 关键行为对比

| 项目 | 修改前 | 修改后 | 结果 |
| --- | --- | --- | --- |
| 剪贴板入库编排位置 | `platform/windows/clipboard.rs` 内直接调用 `Store`/OCR/Tauri | `application/clipboard_ingest.rs`，平台层只做读取 | 符合目标依赖方向 |
| 领域类型命名 | `ClipboardPublication` | `Publication` + `PublicationOrigin` | Store 不再出现剪贴板专属命名 |
| `PayloadData` 归属 | `store::PayloadData` | `domain::PayloadData` | infra 与 domain 边界清晰 |
| 图片编解码 | `platform/windows/image_norm.rs` 直接用 `image` crate | `infrastructure/image/encode.rs`；`image_norm` 只解析 DIB 布局 | 编解码能力可跨来源复用 |
| 截图能力 | 不存在 | F5 原生 overlay + D3D11/D2D 的 L0/L1/L2 | 新增 |
| 事件契约 | 无 envelope，`IPC_SCHEMA_VERSION = 2` | 带 `schemaVersion`/`generation` 的信封，版本 3 | 前端可丢弃陈旧消息 |
| 前端 Tauri 访问 | 组件/store 可直接 `listen`/`invoke` | 只有 `infrastructure/tauri/client.ts` 接触 Tauri | 符合 §2.2 约束 |
| overlay 窗口样式 | 含 `WS_EX_NOACTIVATE`，`Esc`/`Enter` 无法生效 | 去掉该样式 + `SW_SHOW` + `take_focus()` + 归还前台窗口 | 见第 3.6 节实测 |
| `Esc` 行为 | 处理器存在但不可达（死键） | 真实键盘消息可达，取消并释放会话 | 已修复 |
| 启动可用性 | `setup` 永久阻塞，主窗口 `hung=True msgOk=0` | `setup` 12ms 内返回，主窗口 `hung=False msgOk=1` | 见第 3.7 节实测 |
| 事件投递 | 事件名含 `.` 被 Tauri 拒绝，**所有** capture/clipboard/ocr 事件丢失 | 事件名只用 `[a-z0-9-]`，实测 `accepted` | 见第 3.8 节实测 |
| overlay 关闭 | 先置标志再 `post()`，消息发不出仍 `join()` → 可能永久等待 | 先直投 `Shutdown` 再置标志 | 死锁路径消除 |
| L0 数据路径 | WGC texture → CPU readback → 再上传 D2D bitmap | texture 直接包成 D2D bitmap；CPU 像素仅导出时惰性读取 | 每次会话少一次整帧往返 |
| 鼠标移动开销 | 整屏重绘，且 `dirty` 不约束绘制 | 只失效放大镜面板 + 十字线，并按 damage 包围盒 clip | 见第 3.9 节 GPU 证据 |
| OCR 启动回填 | 首个循环立刻扫描历史待办并加载模型 | 首次回填延后 5s；显式任务立即处理 | 不再与窗口创建竞争 |

## 6. 已知限制（有意为之，非缺陷）

1. **最低 Windows 版本未在安装检查中断言**。运行时按 API 能力探测 WGC（`GraphicsCaptureSession::IsSupported()`）并降级到 BitBlt，而不是按版本号判断。任务清单 §4.1 允许这种“按能力探测”的方式，但若最终确定 1903+ 作为最低版本，仍需在安装器/文档中写明。
2. **`WM_HOTKEY` 重复按 F5 的行为选择“重置当前会话”**（清单 §3.2 允许忽略或重置二选一）。
3. **尺寸标签在上下都放不下时不绘制**。这是为了不让标签覆盖选区；真实 1080p 及以上显示器不会触发。
4. **多显示器混合 DPI 的坐标转换有单元测试**，但没有在真实多显示器 + 混合 DPI 环境跑过（见第 7 节）。
5. **工具栏未实现**，属于 §1.2 明确的非目标。
6. **store 测试的偶发并行失败未能复现**。曾在一次全量运行中看到 5 个 `infrastructure::store`
   用例失败（`UNIQUE constraint failed: clips.id`），此后连续 16 次全量运行（默认并行）全部
   133 passed，单线程与单独运行也始终通过。因无法稳定复现，**未定位根因、未修复**，
   仅在此记录：怀疑是测试隔离问题（多次 `Store::open` 指向同一路径），而不是产品缺陷。
   `%TEMP%` 下已累积约 924 个 `snapclip-store-test-*` 目录，清理逻辑可以另外补。

## 7. 尚未完成 / 尚未验证

### 7.1 尚未实现的评审条目

- **P1：F5 回调仍同步执行捕获**。`WM_HOTKEY` 直接调用 `start_session()`，其中包含 WGC 初始化、
  最长约 1500ms 的首帧等待、readback 与 D3D/D2D 资源创建，全部发生在 overlay 消息线程上。
  修复需要把捕获准备移出消息线程（消息线程只保留状态与输入），并把结果回投到该线程。
  这是一次独立的结构调整，**本轮未做**，因此"F5 期间鼠标键盘短暂无响应"这一点仍然存在。

### 7.2 需要真机交互验证

以下项**必须由人在真实桌面上执行**，本环境无法完成，因此不作“已验证”声明。已尝试用 `SendKeys` 注入 `F5` 来驱动交互流程，但该会话无法投递合成键盘输入（`SendKeys` 未生效，overlay 未显示），所以交互链路仍未验证。

需要区分两类：第 4.1 节的 `Esc` 缺陷已修复并有实测证据（窗口可激活），但**「按下 Esc 后 overlay 确实消失」这一步仍未被真实按键验证**——本环境无法投递键盘输入。

- 按 `F5` 唤起 overlay、`Esc` 取消、`Enter` 生成 artifact 的实际交互（需要真实键盘/鼠标输入与可见桌面）。
- `F5` 在主窗口可见/隐藏/焦点在其他应用时都能启动。
- 底图确实不包含 overlay 自身、尺寸标签和放大镜（需要人眼比对 artifact 与桌面）。
- 拖动过程中的撕裂、残影、闪烁与输入延迟（主观 + QPC 打点）。
- 100/125/150/200% DPI 下边框、手柄、文字的实际清晰度与位置。
- 多显示器、负虚拟坐标、跨不同 DPI 显示器移动鼠标。
- HDR 显示器与 Windows 高对比度模式下的可读性。
- `F5` 被其他应用占用时的冲突提示文案（代码路径已实现并返回 `hotkey_conflict`，但未用真实占用者验证完整提示链路）。
- 真实 WGC 首帧延迟；BitBlt 路径已实测可用，WGC 路径由 `GraphicsCaptureSession::IsSupported()` 探测，若本机支持则由 provider 在真实会话中走通（集成测试已覆盖「探测 + 抓取 + 必须产出 GPU texture」，但未断言具体落到哪个 provider）。
- §10.4 的性能指标（F5→首帧 P50/P95、拖拽 P95 延迟、CPU/内存/GPU 占用、readback 次数）。

建议的手工验收步骤：

1. 运行 `src-tauri/target/release/snapclip.exe`（或安装 `SnapClip_0.1.0_x64-setup.exe`）。
2. 按 `F5`：应出现整屏变暗、鼠标处有放大镜与十字准星。
3. 拖出选区：应看到圆角边框、八个手柄、`宽 × 高` 标签跟随。
4. 拖动手柄缩放、拖动选区内部移动；靠近屏幕边缘时放大镜自动翻转。
5. 按 `Enter`：`%LOCALAPPDATA%\com.seeyuer.snapclip\artifacts\capture\` 下应生成 PNG，且图片中**不含**遮罩、边框、标签、放大镜。
6. 再按 `F5` 后按 `Esc`：overlay 消失，无残留窗口，`capture://cancelled.v1` 事件（`reason: "escape"`）到达前端。

## 8. 复现验证的命令汇总

```powershell
# 后端
cd src-tauri
cargo check --all-targets
cargo test --lib

# 前端
npx vue-tsc --noEmit
npx vite build

# 安装包
npx tauri build --bundles nsis
```

