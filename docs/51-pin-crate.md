# 51 · 贴图能力独立为 snapclip-pin

日期：2026-11-02

## 1. 范围与目标

将应用内已有的图片贴图能力迁入 `crates/snapclip-pin`，让窗口、像素呈现、交互与资源生命周期有独立的编译和测试边界。

本轮完成现有截图贴图的拆分，保留 F6 与捕获完成后的自动贴图接线。F3 剪贴板入口、文本贴图、文件贴图和剪贴板 crate 拆分不属于本轮实现；后续由应用协调来源，将准备好的内容交给贴图能力。

## 2. 依赖与职责

```text
snapclip-app
  ├── snapclip-capture ──→ snapclip-model / snapclip-ui
  ├── snapclip-history ──→ snapclip-model
  └── snapclip-pin ──────→ snapclip-model / snapclip-ui
```

`snapclip-pin` 不依赖 `snapclip-capture`、`snapclip-history`、`snapclip-app`、GPUI、Base 或 Yue，也不读取系统剪贴板、不回读屏幕、不编码文件。

| 所有者 | 职责 |
| --- | --- |
| `snapclip-pin` | 专用 Win32 消息线程、F6 注册、贴图窗口、置顶且不激活、拖动、双击关闭、滚轮缩放、DPI、边框与圆角、倍数徽标、事件和清理 |
| `snapclip-ui::font` | 一份共享字体子集、族名与回退顺序、进程内注册、倍数徽标字重和字号 |
| `snapclip-capture` | 截图与最近确认选区槽位；不调用贴图能力 |
| `snapclip-app::capture::PinSource` | 将截图槽位接成 `PinHome`，保留一次性取走、屏幕坐标、长图排除与圆角语义 |
| `snapclip-app` | 启动管理器、连接捕获完成回调、定期排空贴图事件 |

Windows 是唯一支持的平台，crate 采用 `#![cfg(windows)]`，不提供无效的跨平台实现。

## 3. 文件与公开接口

```text
crates/snapclip-pin/
  Cargo.toml
  src/
    lib.rs              公开边界、PinHome、PinSelection
    source.rs           ImageSource、像素格式校验和转换
    host.rs             PinManager 与私有 Win32 宿主实现
  tests/
    windows_host.rs     显式运行的真实桌面生命周期测试
```

只从 crate 根导出 `PinManager`、`PinHome`、`PinSelection`、`ImageSource`、`PinRequest`、`PinHandle`、`PinPoster`、`PinEvent`、`PinReport`；`host`、`source` 是私有实现模块。

删除了应用原 `pin/` 目录，没有转发模块或兼容别名。未被声明、实际未编译的旧 `target.rs` 一并移除，它描述的是已废弃的“光标所指窗口贴图”，不符合现有 F6 语义。

### 3.1 宿主与直接创建

```rust
use std::sync::Arc;
use snapclip_pin::{ImageSource, PinHome, PinManager, PinRequest, PinSelection};

struct Source;
impl PinHome for Source {
    fn take_selection(&self) -> Option<PinSelection> {
        None
    }
}

fn example() -> anyhow::Result<()> {
    let pins = PinManager::start(Arc::new(Source))?;
    let image = ImageSource::from_bgra(vec![0xFF4488AA; 32 * 32], 32, 32)?;
    let handle = pins.create_pin(PinRequest::new(image, (100, 100)).radius(4))?;
    // 应用在事件循环中定期调用 try_recv_event() 或 drain_events()。
    pins.close_pin(handle)?;
    Ok(())
}
```

此例同样放在 crate rustdoc 中，已运行编译型文档测试。

- 一个进程使用一个活跃管理器。`clone()` 共享宿主，不另起线程。
- 不再保留原 `PinManager → PinWindowHost` 的重复包装，`PinManager` 直接持有宿主。
- 位置使用虚拟桌面物理像素，指整个窗口左上角；负坐标合法。
- 圆角使用原图物理像素，默认 0，负值归零，缩放时随原图比例变化。
- BGRA 截图入口强制不透明；RGBA 入口保留原有透明度语义。
- 请求不暴露 ID 字段，由宿主统一分配。`PinHandle::id()` 用于匹配事件。
- `create_pin()` 返回成功只代表请求已投递，实际窗口结果由 `Created` / `Failed` 确认。
- `close_pin()` 投递关闭命令，已关闭或不存在的同宿主句柄会被忽略。

### 3.2 最近截图入口

`PinHome::take_selection()` 在宿主线程调用，必须及时返回，不应同步等待 UI 线程。`None` 表示没有内容，不分配编号、不产生失败事件；取走后槽位为空。

`PinSelection::new(region, pixels).radius(radius)` 保留截图的 BGRA 数据、虚拟桌面位置与拍摄时的圆角。宿主消费时校验尺寸和字节长度；非法选区产生 `Failed`。

F6 和 `PinPoster::post()` 共用取图处理逻辑。后者投递内部命令后唤醒宿主，应用无需知道 Win32 消息号。

### 3.3 生命周期与线程

- HWND、DIB、GDI 绘制与窗口状态仅由宿主线程持有。移除了与此约束相矛盾的 `unsafe impl Send for LayeredBuffer`。
- 最后一个 `PinManager` 释放时，发送退出命令、销毁所有窗口、释放热键与窗口类，并等待线程退出。
- `PinPoster` 只保存弱引用，不维持宿主存活；宿主释放后返回错误，不再凭旧线程 ID 向可能被系统复用的线程投递。
- 事件接收器使用短时互斥锁，管理器和投递句柄可跨线程使用；没有增加新的后台运行时。
- F6 被其他应用占用时明确记录错误，直接创建与 `PinPoster` 仍可用，保持既有降级策略。

### 3.4 事件与编号

| 事件 | 含义 |
| --- | --- |
| `Created { id }` | 窗口已创建并提交内容 |
| `Failed { id }` | 已请求，但未能创建窗口 |
| `Closed { id }` | 已创建的窗口被关闭 |

应用必须轮询事件。现有通道容量保持为 64；满时记录日志并丢弃该事件，不阻塞窗口消息线程。`drain_events()` 汇总三类事件，`try_recv_event()` 可按 ID 单独消费；它们共享同一个消费通道。

原来 F6 与调用方各有一套从 1 起的编号器。现在共用同一个原子编号器，取号在溢出前报错，绝不回绕复用。没有内容的 F6 不取号；已提交后失败的请求可以留下编号空缺。

另修正了 F6 创建失败时误上报 `Closed` 的路径，现在正确上报 `Failed`，并有失败回归断言。

## 4. 共享字体迁移

原先 `pin/host.rs` 直接依赖 `snapclip_capture::overlay_font`，导致仅移动文件仍无法独立。共享资源现在归入已有的 `snapclip-ui`，没有复制字体、也没有额外创建字体 crate：

```text
crates/snapclip-ui/src/font.rs
crates/snapclip-ui/assets/harmonyos-sans-sc-subset.ttf
```

截图覆盖层、贴图和原生保存控件的引用已同步更新。`subfont/build_subset.py` 的安装目标也更新到新路径，避免下次生成仍写旧位置。

移动前后字体字节完全一致：53,276 字节，SHA-256 为 `5dd72fd950969e423a98ae32fe6e8b7153d7831eb0c0ecb1cd91cb9316d24aba`。字体族、14 DIP / 700 的徽标度量、回退族序和渲染逻辑保持一致。

## 5. 验证与前后对比

迁移前先执行应用、贴图筛选、UI、capture 测试。迁移后检查了原 `mod.rs` / `host.rs` / `source.rs` 的测试函数集合：原有 **58 个测试全部保留**，新增 **7 个**。

| 验证 | 迁移前 | 迁移后 |
| --- | --- | --- |
| 应用普通 lib | 316 通过 / 9 ignored | 261 通过 / 6 ignored；另有 pin 62 通过 / 3 ignored |
| 应用 test-support lib | 334 通过 / 9 ignored（上轮记录） | 279 通过 / 6 ignored；另有 pin 62 通过 / 3 ignored |
| 贴图自身 | 应用内筛选 55 通过 / 3 ignored | 独立 crate 62 通过 / 3 ignored |
| UI lib | 221 通过 | 221 通过 |
| capture 单包 lib | 545 通过 / 80 ignored | 545 通过 / 80 ignored |

原有 3 个 ignored 贴图用例仍是测量 / 出对比图，不把它们改成假通过。默认用例不创建真实窗口；新增真实 Windows 冒烟测试同样明确标记为 opt-in。

新增回归覆盖：多线程共享编号、编号耗尽不回绕、无图 F6 不取号、非法 F6 请求报失败且使用同一编号序列、弱句柄不维持宿主存活且关闭后报错、跨线程类型约束、负屏幕坐标与圆角规范化。原编号与空选区用例继续保留并改为测试实际分配器和消费路径。

本轮已显式运行真实 Win32 冒烟测试并通过：直接创建与最近截图创建使用不同 ID、非法来源报 `Failed`、窗口带 `WS_EX_NOACTIVATE`、关闭单张、重复关闭、管理器克隆维持窗口、最后释放销毁窗口、关闭后投递失败、重建宿主。

**这不等于完整人工交互验收。** 首次实跑时 F6 注册返回 1409（已被占用），重跑时初次及重建宿主均成功注册 F6。真实窗口测试通过 `PinPoster` 调用共用处理路径，未验证物理 F6 按键投递。实机鼠标拖动、滚轮、双击、跨屏 DPI 与屏幕视觉对比仍需人工回归；本轮保留的纯函数测试覆盖其几何与像素算法。

### 可重复命令

```powershell
cargo test -p snapclip-pin --locked
cargo test -p snapclip-app --lib --locked
cargo test -p snapclip-app --lib --features test-support --locked
cargo test -p snapclip-ui -p snapclip-capture --lib --locked
cargo test --workspace --lib --locked -- --test-threads=1
cargo test --workspace --features snapclip-app/test-support --locked --no-fail-fast -- --test-threads=1
cargo clippy -p snapclip-pin --all-targets --no-deps --locked -- -D warnings
cargo fmt -p snapclip-pin -- --check
cargo check --workspace --all-targets --locked
cargo build -p snapclip-app --locked
pwsh -NoProfile -File tools/check-dependency-direction.ps1
pwsh -NoProfile -File tools/check-test-baseline.ps1 -Log target/pin-split-workspace-tests.log
```

真实桌面测试（需交互式 Windows 会话，会短暂创建贴图窗口）：

```powershell
cargo test -p snapclip-pin --test windows_host --locked -- --ignored --test-threads=1 --nocapture
```

`--no-deps` 严格 Clippy 的结论仅覆盖新能力 crate；已有 `snapclip-ui` 等依赖仍有本轮范围之外的诊断，不将它表述为全工作区 Clippy 干净。像素内核保留已有显式参数形式，仅对四个内核函数使用带理由的参数个数 lint 例外，没有全局压制告警。

依赖方向门禁已加入 `snapclip-pin`，并双向禁止 capture/history 与 pin 互相依赖。测试计数门禁将原应用基线拆成应用 261/6 与 pin 55/3，总和仍为迁移前的 316/9，确保移动测试不导致计数门禁失效。

### 最终验收状态与已发现的仓库问题

- 普通工作区 lib：1,265 通过，89 ignored，0 失败。应用普通 lib 为 261/6；开启 test-support 的应用 lib 为 279/6。
- 除应用外的 workspace 全套测试（含 integration / doctest）通过。`snapclip-pin` 文档测试 1 项通过，真实 Win32 冒烟 1 项已显式运行并通过。
- 新 crate 的格式检查、严格 Clippy（`--no-deps`）、依赖方向、测试数基线、桌面测试标记、注释路径和实体映射门禁通过。
- `cargo check --workspace --all-targets --locked` 通过；已有 `snapclip-ui/examples/component_gallery.rs` 的 9 个 unused-result 告警仍存在，未将它描述为无告警。
- 默认应用已重建，预览副本已同步。两者均为 77,322,752 字节，SHA-256 同为 `6dd8b4c30bb7f2ce4d4225d1d40c20293b7c0f60831b70f59bba76dfd40e84bb`，构建时间为 2026-11-02 21:41:19。

完整应用集成测试尚未全绿，已保留失败断言：

1. `regression_baseline::the_regression_suite_names_the_behaviours_it_freezes`：冻结清单把 `an_element_preview_keeps_its_own_pixels_and_the_window_fallback_does_not` 列为必须默认运行，但它实际上标记了 GPU / D2D 环境依赖而 ignored。清单文件与该 ignore 标记在 HEAD 中已经如此；本轮未修改清单，也未删除或放宽断言。
2. `ui::the_unit_suffix_switch_writes_to_disk`：在 640×480 的测试窗口里，点击 `label-unit-suffix` 时节点不可见。开启 test-support 的 UI 套件为 9 通过、1 失败，单独重跑该项同样失败。测试与设置页没有本轮改动；本轮迁移前未运行这条 UI 集成测试，因此不虚构其迁移前的运行结果。

未启用 `snapclip-app/test-support` 的全量命令还会因未注册历史页面测试节点而多出 2 个 UI 失败；该页面源码及 UI 测试说明均要求启用该 feature。保留两种日志，不把缺少测试 feature 的运行当成完整 UI 验收。

本轮没有提交、推送，也没有修改上述失败用例来让总结果变绿。完整日志位于本地忽略目录 `target/pin-split-all-tests.log` 和 `target/pin-split-all-tests-with-ui.log`。
