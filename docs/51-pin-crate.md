# 51 · 独立贴图能力 snapclip-pin

初次拆分：2026-11-02；当前架构更新：2026-11-03。

## 1. 当前范围

`snapclip-pin` 负责准备好的图片、文档和文件集合的展示与生命周期。F3/F6、剪贴板读写、截图来源、数据库均由应用或对应能力提供。本文中的 API 是 F3 演进后的当前接口；原 `PinHome`、`PinPoster`、`PinManager::start` 已删除，不保留兼容包装。

F3 图片、文档、文件与 F6 原位截图均由 GPUI 应用线程展示。F3 使用完整 YueImagePreview；F6 无工具栏画布调用同一 `ImagePreviewImage::paint`，共用像素资源、色彩定义及 HDR/SDR renderer。旧 Win32/GDI 图片线程、DIB、CPU 缩放及轮询通道已删除。公共管理器是 `Entity<PinManager>`，不是可随意跨线程操作的窗口句柄集合。

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
| `snapclip-pin::PinManager` | 准入、统一编号、含准备中窗口的 open 注册表、一次性回执、关闭/隐藏/恢复、撤销和复制意图 |
| `screenshot::ScreenshotView` | F6 原位图片画布、后台准备、鼠标锚定缩放和首次上传/绘制回执 |
| `view::PinView` | GPUI 应用线程拥有 F3 图片/文档/文件窗口，保留 YueImagePreviewState、任务、焦点与菜单 |
| `snapclip-app::clipboard_pin` | 单个 F3 读取 worker、合并/过期、去重、复制写入接线 |
| `snapclip-app::tray` | 已有消息线程注册并释放 F3/F6；显式处理 thread `WM_HOTKEY` |
| `snapclip-app::capture::PinSource` | 一次性取走最近截图，不让 pin 反向依赖 capture |

Windows 是唯一支持平台。平台 HWND/DPI/工作区/打开命令集中于 `platform.rs`；窗口创建、物理 bounds、DPI 策略和系统拖动由维护的 GPUI Windows backend 承担，不跨线程操作 GPUI entity。

## 3. 文件结构

```text
crates/snapclip-pin/
  src/lib.rs                 公开边界、PinSelection、初始化
  src/manager.rs             PinManager、CopyTarget、PinCommand、生命周期
  src/screenshot.rs          F6 无附加控件画布、准备与交互
  src/screenshot_geometry.rs 原位物理几何、整数十分位缩放、滚轮余量
  src/screenshot_tests.rs    opt-in 无头 F6 交互/回执/取消测试
  src/screenshot_native_tests.rs opt-in 独立进程原生窗口探针
  src/view.rs                生产图片/文档/文件视图与交互
  src/table.rs               Kit DataTable 只读委托
  src/image_preview.rs       有界本地图片读取、解码和派生预览
  src/image_preview_contract_tests.rs 生产资源转换与精度回归
  src/platform.rs            窄 Windows 适配
  src/feedback.rs            指针附近反馈、5 秒撤销关闭入口
  src/view_tests.rs           opt-in 无头生产 UI 集成测试
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
// 无常驻轮询；窗口关闭订阅和首次绘制回执驱动生命周期。
```

上例为接线片段，不是独立入口；可编译运行入口见 `examples/pin_preview.rs`。`PinManager::open_selection(selection, &owner, cx, on_created)` 专用于应用认领的截图，保留原屏幕物理坐标、1× 初始比例和圆角。应用把来源 claim 放进 `on_created`：首次上传/绘制成功后才消费；开窗、准备、绘制失败及提前关闭均释放 claim 并保留来源。F3 图片窗口按目标工作区及 DPI 估算初始大小，另留工具条和最小可操作尺寸；图像由组件 FitAll 适配实际视口，原始像素不缩小。

- `open` / `open_selection` 的句柄表示窗口已注册，不等于图片已呈现。同步开窗失败直接返回错误，异步准备/绘制失败通过回执关闭窗口并提示。成功回执不等同于 DWM 最终显示或显示器亮度验收。
- 每张图使用同一编号器，溢出报错，不复用 ID。注册表包含 pending，关闭/隐藏也覆盖尚未确认创建的图片。
- 所有窗口由 `on_window_closed` 清理，即便外部仍持有 view 也能注销。取消先移除注册项，迟到及重复回执不复活窗口、不重复消费来源。未就绪窗口不进入撤销栈。
- `raise / close / close_all / set_visible / undo` 都通过管理器调用。F3 初次显示不抢系统焦点，操作后可激活以选字或控制预览；F6 使用持久 nonactivating popup、置顶和物理 bounds。DPI 变化仅改变 DIP chrome，图片 1× 保持源物理像素；滚轮每档 0.1、范围 0.1–10，复用同一 GPU 资源，无 CPU 重采样。右键菜单使用可越出小 HWND 的系统菜单。
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

F6 迁移的行为、旧测试映射、原生 GPU 读回和边界见 [53 · HDR §16](53-hdr-capture.md#16-f6-共用-gpuiyue-宿主p3-i2026-11-03)。原生探针必须按精确 filter 在独立进程运行，不要一次运行全部 ignored 测试。

## 6. 2026-11-02 拆分的历史基线

以下是旧图片能力独立时的真实记录，不表示当前 API，也不冒充本轮实机验收：

- 原应用中的 58 个相关测试保留，独立 pin 当时为 62 passed / 3 ignored；应用普通 lib 为 261/6，UI 221/0，capture 545/80，history 51/0，model 23/0。
- 当时显式运行过 Win32 生命周期测试，覆盖创建、关闭、销毁、重建宿主；没有验证物理 F6、拖动、滚轮和跨屏 DPI。F3 改动后的桌面验证需重新执行。
- 字体已共享到 `snapclip-ui/assets/harmonyos-sans-sc-subset.ttf`，53,276 字节，SHA-256 `5dd72fd950969e423a98ae32fe6e8b7153d7831eb0c0ecb1cd91cb9316d24aba`；图片徽标继续使用 `snapclip_ui::font`。
- 已知历史集成失败：冻结清单要求一个实际 ignored 的浏览器像素测试默认执行；设置页 `the_unit_suffix_switch_writes_to_disk` 在 640×480 下目标不可见。保留原断言，本轮结果见 docs/52，不据旧日志宣称新版本通过。

拆分时日志保留于本地忽略的 `target/pin-split-*.log`，不是当前版本验证结果。
