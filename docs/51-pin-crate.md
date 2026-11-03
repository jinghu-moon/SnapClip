# 51 · 独立贴图能力 snapclip-pin

初次拆分：2026-11-02；当前架构更新：2026-11-03。

## 1. 当前范围

`snapclip-pin` 负责准备好的图片、文档和文件集合的展示与生命周期。F3/F6、剪贴板读写、截图来源、数据库均由应用或对应能力提供。本文中的 API 是 F3 演进后的当前接口；原 `PinHome`、`PinPoster`、`PinManager::start` 已删除，不保留兼容包装。

F3 图片、文档、文件通过 GPUI/Yue 展示；F3 图片及文件/文档的完整图片视图使用 YueImagePreview。F6 原位截图保留独立 Win32 分层窗口路径。公共管理器是 GPUI 应用线程上的 `Entity<PinManager>`，不是可随意跨线程操作的窗口句柄集合。

## 2. 依赖与所有权

```text
snapclip-app
  ├── snapclip-clipboard ──→ snapclip-model
  ├── snapclip-history ────→ snapclip-clipboard / snapclip-model
  ├── snapclip-capture ────→ snapclip-model / snapclip-ui
  └── snapclip-pin ────────→ snapclip-model / snapclip-ui
                         └→ gpui-kit / yue-component
```

pin 不依赖 clipboard、capture、history、OCR 或数据库。它接受不可变 `Arc<PinSnapshot>`，通过 `PinCommand::Copy` 请求应用复制。应用在后台写系统剪贴板，再用 `report_copy` 将真实结果反馈给原窗口。

| 所有者 | 职责 |
| --- | --- |
| `snapclip-model::pin` | `PinSnapshot`、Image/Document/Files、文档模式、表格、文件元数据；无 UI |
| `snapclip-clipboard` | 同锁读取、用途过滤、格式解码、分类、写入和自写标记；无 UI/数据库 |
| `snapclip-pin::PinManager` | 准入、统一编号、pending/open 注册表、关闭/隐藏/恢复、撤销和复制意图 |
| 私有 `host::NativeImages` | F6 原位截图的 Win32 线程，拥有 HWND、DIB、缩放与消息泵 |
| `view::PinView` | GPUI 应用线程拥有 F3 图片/文档/文件窗口，保留 YueImagePreviewState、任务、焦点与菜单 |
| `snapclip-app::clipboard_pin` | 单个 F3 读取 worker、合并/过期、去重、复制写入接线 |
| `snapclip-app::tray` | 已有消息线程注册并释放 F3/F6；显式处理 thread `WM_HOTKEY` |
| `snapclip-app::capture::PinSource` | 一次性取走最近截图，不让 pin 反向依赖 capture |

Windows 是唯一支持平台。平台 HWND/DPI/工作区/打开命令集中于 `platform.rs`；只有图片宿主在线程内处理自己的 HWND，不跨线程操作 GPUI entity。

## 3. 文件结构

```text
crates/snapclip-pin/
  src/lib.rs                 公开边界、PinSelection、初始化
  src/manager.rs             PinManager、CopyTarget、PinCommand、生命周期
  src/host.rs                私有原生图片宿主、几何/重采样测试
  src/source.rs              像素格式与图像源
  src/view.rs                生产图片/文档/文件视图与交互
  src/table.rs               Kit DataTable 只读委托
  src/image_preview.rs       有界本地图片读取、解码和派生预览
  src/platform.rs            窄 Windows 适配
  src/feedback.rs            指针附近反馈、5 秒撤销关闭入口
  src/view_tests.rs           opt-in 无头生产 UI 集成测试
  src/windows_host_tests.rs   opt-in 原生 Windows 生命周期测试
  examples/pin_preview.rs     8 类样本的生产窗口预览
```

模块保持私有，从 crate 根使用公开类型。完整内容模型不由调用方直接修改字段。

## 4. 当前调用方式

初始化顺序：`gpui_kit::init` → `yue_component::init` → `snapclip_pin::init`。应用保留一个管理器 entity：

```rust,ignore
let (manager, commands) = snapclip_pin::PinManager::new()?;
let owner = cx.new(|_| manager);
let snapshot = std::sync::Arc::new(snapclip_pin::PinSnapshot::new(
    snapclip_pin::PinContent::Document(snapclip_pin::PinDocument::new(
        "对照文字".into(), snapclip_pin::DocumentMode::Text,
    )),
));
let handle = snapclip_pin::PinManager::open(
    snapshot, snapclip_pin::PinPlacement::Pointer(pointer), &owner, cx,
)?;
// 异步消费 commands 中的 Copy { id, target }，系统写入后 report_copy(id, result, cx)。
// 应用循环调用 owner.update(cx, |manager, cx| manager.poll(cx))。
```

上例为接线片段，不是独立入口；可编译运行入口见 `examples/pin_preview.rs`。`PinManager::open_selection(selection, &owner, cx)` 专用于应用取走的截图，保留原屏幕物理坐标、1× 初始比例和圆角。F3 图片窗口按目标工作区及 DPI 估算初始大小，另留工具条和最小可操作尺寸；图像由组件 FitAll 适配实际视口，原始像素不缩小。

- `open` 对 F6 原生图片返回句柄只表示请求受理，通过 `poll` 得到 Created/Failed；F3 的 GPUI 开窗失败直接返回错误。
- 每张图使用同一编号器，溢出报错，不复用 ID。注册表包含 pending，关闭/隐藏也覆盖尚未确认创建的图片。
- 原生事件用不丢完成消息的通道；GPUI 窗口由 `on_window_closed` 清理，即便外部仍持有 view 也能注销。
- `raise / close / close_all / set_visible / undo` 都通过管理器调用。GPUI 窗口初次显示不抢系统焦点，操作后可激活以选字或控制预览；F6 原位截图保留原生不激活行为。
- 原始和派生内容预算暂定 256 MiB，最多 32 张（含 pending）；撤销最多保留最近 8 张并计入字节预算。它们是保护策略，不是经过压力测量的容量保证。
- 关闭后提供 5 秒撤销按钮；`Ctrl+Shift+T` 与托盘也可撤销。关闭全部会取消 F3 读请求，迟到结果不再打开窗口。

## 5. 验证入口

```powershell
cargo test -p snapclip-pin --lib --locked
cargo test -p snapclip-pin --features headless-tests --lib --locked
cargo test -p snapclip-clipboard --test classification --locked
cargo clippy -p snapclip-pin -p snapclip-clipboard --all-targets --no-deps --locked -- -D warnings
pwsh -NoProfile -File tools/check-dependency-direction.ps1
cargo run -p snapclip-pin --example pin_preview -- table
```

预览可选 `text / rich / table / code / link / path / files / image`；`$env:SNAPCLIP_PIN_DARK='1'` 启用深色，删除该环境变量恢复浅色。预览复制只模拟完成反馈，不写剪贴板；打开链接/文件仍执行显式系统命令。

真实剪贴板与 Win32 宿主测试均显式 ignored 并在理由中说明副作用；不要默认执行全部 `--ignored`，其中还有测量和出图任务。无头 UI 测试不能证明真实桌面的热键、渲染、DPI 或读屏播报。当前实现/测试结果与尚未验收事项集中在 [52 · 实现与验证记录](52-clipboard-pin-design.md#11-实现与验证记录2026-11-03)。

## 6. 2026-11-02 拆分的历史基线

以下是旧图片能力独立时的真实记录，不表示当前 API，也不冒充本轮实机验收：

- 原应用中的 58 个相关测试保留，独立 pin 当时为 62 passed / 3 ignored；应用普通 lib 为 261/6，UI 221/0，capture 545/80，history 51/0，model 23/0。
- 当时显式运行过 Win32 生命周期测试，覆盖创建、关闭、销毁、重建宿主；没有验证物理 F6、拖动、滚轮和跨屏 DPI。F3 改动后的桌面验证需重新执行。
- 字体已共享到 `snapclip-ui/assets/harmonyos-sans-sc-subset.ttf`，53,276 字节，SHA-256 `5dd72fd950969e423a98ae32fe6e8b7153d7831eb0c0ecb1cd91cb9316d24aba`；图片徽标继续使用 `snapclip_ui::font`。
- 已知历史集成失败：冻结清单要求一个实际 ignored 的浏览器像素测试默认执行；设置页 `the_unit_suffix_switch_writes_to_disk` 在 640×480 下目标不可见。保留原断言，本轮结果见 docs/52，不据旧日志宣称新版本通过。

原日志保留于本地忽略的 `target/pin-split-*.log`。本轮没有提交或推送。
