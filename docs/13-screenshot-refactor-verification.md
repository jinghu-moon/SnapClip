# 截图全流程重构验证记录（Phase 0 ~ 7）

本文件记录 `docs/11-screenshot-fullflow-ui-refactor-tasklist.md` 各阶段的基线指标、
契约冻结内容、可观测性输出与验证证据。每个阶段完成后追加对应章节。

- 基线提交（重构前 MVP 快照）：`179a40b`
- 测量硬件/系统：Windows 11 24H2，单显示器 3840×2160 @ DPI 144，WGC 可用
- 测量构建：`cargo build` debug profile（`snapclip.exe`），前端 `npm run build` 产物

---

## Phase 0：基线、契约冻结与可观测性

### P0.1 静态基线（重构前，commit `179a40b`）

| 命令 | 结果 |
| --- | --- |
| `cargo test --lib` | 133 passed / 0 failed |
| `cargo check --all-targets` | exit 0，0 warnings |
| `npm run typecheck`（vue-tsc） | exit 0 |
| `npm run build`（vite） | exit 0，182 ms |

Phase 0 修改后复验：`cargo test --lib` 134 passed（新增 1 个 readback 基线测试）、
`cargo check --all-targets` 0 warnings、typecheck 通过。

### P0.2 契约冻结

冻结对象为 capture 事件 DTO、状态机值集与错误码；内部 Win32 API 不冻结。

**状态机值集**（`domain::CaptureState`，serde `snake_case`，与
`src/shared/contracts.ts` 一一对应，docs/11 §2.2）：

```text
idle → preparing → armed → selecting → selected → adjusting → annotating → exporting → idle
```

`finishing` 为当前 MVP 的产出态，Phase 3 用 `exporting` 取代后删除。
`preparing/adjusting/annotating/exporting` 随各自阶段接入转移，Phase 0 仅冻结值集。

**事件信封**（`events::EventEnvelope`，`IPC_SCHEMA_VERSION = 3`）：

```json
{ "schemaVersion": 3, "generation": <u64 单调递增>, "payload": { ... } }
```

**事件 DTO**（事件名仅 `[a-z0-9-]`，payload 字段 camelCase）：

| 事件 | 字段 |
| --- | --- |
| `capture-started-v1` | `sessionId, monitorLeft, monitorTop, monitorWidth, monitorHeight, dpi, provider` |
| `capture-state-v1` | `sessionId, state, dpi|null` |
| `capture-completed-v1` | `sessionId, artifactRef, width, height` |
| `capture-cancelled-v1` | `sessionId, reason` |
| `capture-failed-v1` | `sessionId|null, errorCode, provider, message` |

**错误码**（`CaptureErrorCode.as_str()` → IPC `ErrorCode`，共 13 个）：

| 字符串码 | IPC 映射 | 字符串码 | IPC 映射 |
| --- | --- | --- | --- |
| `unsupported` | unsupported | `device_removed` | internal |
| `hotkey_unavailable` | internal | `window_failed` | internal |
| `hotkey_conflict` | conflict | `render_failed` | internal |
| `monitor_unavailable` | invalid_argument | `encode_failed` | internal |
| `provider_unavailable` | unsupported | `invalid_state` | invalid_argument |
| `capture_failed` | internal | `cancelled` | cancelled |
| `internal` | internal | | |

原生失败附带 `#code=<HRESULT/Win32>` 后缀，可经 `CaptureError::native_code()` 提取。

### P0.3 可观测性插桩

统一前缀 `[snapclip][capture]`（stderr），关键字段含 session id（内含会话
generation：`capture-<unix_ms>-<counter>`）：

| 日志 | 内容 |
| --- | --- |
| `monitor bounds=… lookup_ms=` | 显示器查询与耗时 |
| `providers initialized wgc_supported= elapsed_ms=` | provider 探测 |
| `provider attempt= / success= elapsed_ms=` | 单次捕获尝试与结果 |
| `frame frozen provider= size= elapsed_ms=` | 冻结帧（stage：frame ready） |
| `renderer prepared elapsed_ms=` / `overlay shown … elapsed_ms=` | stage：renderer ready / visible |
| `readback provider= frame=WxH bytes= elapsed_ms=` | GPU→CPU 整屏回读量（Phase 3 判据） |
| `render session= rects= damaged_px= bbox= frame_px=` | 每次 Present 的脏区统计（Phase 2 判据） |
| `[snapclip][bench] readback frame=… selection=… readback_bytes=…` | 单元测试内产生的 readback 基线（`readback_always_copies_the_whole_frame`） |

### P0.4 性能基线（实测）

自动化手段：debug 应用实例 + `keybd_event`/`mouse_event` 系统级注入 F5/Esc/移动，
解析应用自身 stage 日志；CPU/内存取 `Process.TotalProcessorTime`、
`PrivateMemorySize64`、PDH `\GPU Process Memory(pid_*)\Dedicated Usage`。

**F5 → overlay visible**（n=11，含 1 次冷启动）：

| 指标 | 冷启动 | warm（后 10 次） |
| --- | --- | --- |
| 总延迟 | 230 ms | **P50 = 63 ms，P95 = 92 ms**，min 58 ms |
| provider 捕获（累计至 frame frozen） | 209 ms | P50 ≈ 47 ms |
| renderer prepared（累计） | 213 ms | P50 ≈ 50 ms |

**确认导出的 readback 字节数**（当前 API 无选区参数，任意选区均付整屏）：

| 选区 | 选区字节 | 实际 readback（1080p 帧） | 实际 readback（4K 帧） |
| --- | --- | --- | --- |
| 300×200 | 240,000 B | 8,294,400 B（3 ms） | 33,177,600 B（9 ms） |
| 1920×1080 | 8,294,400 B | 8,294,400 B | 33,177,600 B |
| 3840×2160 | 33,177,600 B | —（超出帧） | 33,177,600 B |

**端到端 confirm 实测**（本机 4K，注入拖拽 + Enter，应用自身日志）：

```text
selection 970×647（选区 2,510,660 B）
→ readback provider=wgc frame=3840x2160 bytes=33,177,600 elapsed_ms=8
→ artifact written size=970x647 elapsed_ms=177（readback+裁剪+PNG 编码+落盘，全部在 overlay 消息线程）
```

**鼠标移动 ~10 s（4K overlay 可见）**——修复 P0.5 缺陷后：

| 指标 | 修复前 | 修复后 |
| --- | --- | --- |
| CPU（单核百分比均值，两次采样） | 27.6% / 7.1% | 4.4% / 1.4% |
| render(Present) 次数 / 9.3 s | 1608 / 1678（含 ~180/s 空闲风暴） | ≈194（仍与已处理 move 1:1，Phase 2 目标） |
| Private Bytes | 62 MB | 59 MB |
| Working Set | 79 MB | 75 MB |
| GPU Dedicated（窗口内） | 552–584 MB | 170–202 MB |
| 空闲（会话结束） | CPU ≈ 0%，日志静默 | 同左 |

### P0.5 Phase 0 发现并修复的根因缺陷：WM_PAINT 永挂起

- **现象**：overlay 可见期间无任何输入时，仍以 ~180 次/秒整屏重绘并 Present
  （空闲 3 s ≈ 540 次全帧 render；日志 10 s 会话刷出 3600+ 行）。
- **根因**：`overlay.rs` 的 `WM_PAINT` 处理直接 `render()` 后返回，从未调用
  `BeginPaint/EndPaint`（或 `ValidateRect`），GDI 更新区域永不清除 → 消息队列
  立即重投 `WM_PAINT`，形成风暴。像素实际来自 DirectComposition visual，HDC 无用。
- **修复**：`WM_PAINT` 分支包以 `BeginPaint/EndPaint`。
- **验证**：修复后同法测得"可见空闲 3 s renders = 7"（含真实鼠标扰动）；风暴消失；
  上表 CPU/内存/GPU 指标为修复后数值。134 个单元测试全部通过。
- **对比**：修复前基线数据（本文件上方表格"修复前"列）保留了缺陷现场证据。

### P0.6 未执行项与原因

| 项目 | 状态 | 原因 / 替代 |
| --- | --- | --- |
| WPA / PresentMon 逐帧指标 | 未执行 | 环境无 PresentMon；以 stage 日志 + PDH 采样替代量级判断，Phase 7 前如需再引入 |
| 多显示器、混合 DPI 实测 | 未执行 | 本机仅单显示器；列入 Phase 7 人工验收清单（docs/11 §13.3） |
| HDR / 高对比度主题 | 未执行 | 环境不具备 |
| 3840×2160 本机整屏 readback 实测 | 已执行 | 单元基线（合成纹理）+ 端到端 confirm（注入拖拽/Enter，见 P0.4）均实测 33,177,600 B |
| 连续 20 次 F5 无旧选区闪现（人工目检） | 未执行 | 需人工观察；自动化已覆盖 11 次 F5/Esc 循环无异常日志 |

---
