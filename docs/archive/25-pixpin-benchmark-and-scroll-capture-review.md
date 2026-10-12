# SnapClip 对标 PixPin 调研 · 文档 19/24 评审与实施建议

> 状态：评审稿（待用户裁决）
> 评审对象：`docs/19-scroll-capture-design.md`（修订稿 v5）、`docs/24-scroll-capture-tasklist.md`
> 对标对象：`C:\A_Softwares\PixPin`（PixPin **3.5.5.1**，Qt **5.15.13** / C++，重点模块为纯 WGC 捕获内核 + 自研 `PixStitching` 拼接引擎）
> 仓库基线：HEAD `bd56705`（main），`docs/19` 已修改未提交，`docs/24` 未跟踪
> 证据约定：**确证** = 有命令输出 / 字节级原文 / 反汇编 / 代码 `文件:行`；**强推断** = 有多条独立证据指向同一结论但缺一环；**弱推断/待验证** = 只有单一间接证据
> **证据附录**（本文是结论，附录是证据，引用时请指到附录）：[docs/26 二进制静态取证](26-pixpin-static-analysis.md) · [docs/27 参考项目调研](27-scroll-capture-reference-survey.md) · [docs/28 运行期数据与日志](28-pixpin-runtime-data-and-logs.md)（清单见附录 C）
> **开源同类型对标**：[docs/29 Snow Shot / snow-apps 滚动截图与拼接引擎深度研读](29-snow-shot-scroll-capture-study.md) —— PixPin 是闭源二进制对标（产品与上限的实测边界），Snow Shot 是开源完整实现对标（可读、可复用的实现与测试方法论）。**本文 §2 的 R1/R3/R12/R16/R17 在 docs/29 中都有参考实现的印证与修订，两文应合读。**

---

## 0. 方法与边界

- 本文只做**调研与评审**，不修改任何生产代码、不改动 `docs/19` 与 `docs/24`（两者作为被评审对象保持原样）。
- `C:\A_Softwares\PixPin` 全程**只读**：未运行 PixPin.exe、未写入、未改名。调研期间发现 **PixPin 正在本机运行**（PID 14928，启动于 2026/10/1 16:59:48），因此取其当日日志前先做只读快照（见 §1.4）。
- 事实来源分四类，正文逐条标注：
  1. **本机静态取证**：PE 头/导入导出表/反汇编/字符串/Qt `.qm` 翻译表；
  2. **本机运行期取证**：配置文件、`LocalStorage.data`、SQLite、`.his`/`.meta`、当日日志（含两次真实长截图会话）；
  3. **本机联网核实**：PixPin 官方文档、更新日志、FAQ、官方配图（下载后以图像方式实读）；
  4. **仓库与参考项目核实**：`文件:行` + 实读代码片段；`refer/` 下 Crisp / ShareX / snow_shot / PowerToys 等。
- 未能核实的一律写"未核实"，不做补齐式推断；**调研过程中被推翻的自己的结论也保留并标注**（例如"tile 化画布""PostMessage 是兜底"两处已在 §1.2 ⑥/⑦ 更正）。

---

## 1. 对标结论：PixPin 实际能力全景

### 1.1 技术栈与模块（安装目录实测，确证）

| 层 | 事实 |
|---|---|
| UI | **Qt5** 全家桶（`Qt5Core/Gui/Widgets/Qml/Network/OpenGL/PrintSupport/Sql/Svg/WinExtras/Xml/Multimedia/MultimediaWidgets`）+ `qwindows.dll` 平台插件；存在 `qxtglobalshortcut.dll`（第三方全局快捷键库） |
| 主程序 | `PixPin.exe` 23.7 MB；`PixPinAuxiliary.exe` 749 KB = **提权代理 + 崩溃重启器 + 独立更新器**（见 §1.2 ⑦），**不参与捕获** |
| 截图/捕获 | `PixWin32CaptureCore.dll` 141 KB、`PixWinCapture.dll` 76 KB、`PixScreenManager.dll` 89 KB、`PixWindowNotify.dll` 39 KB |
| UI 检测 | `UiRegionDetector.dll` 91 KB、`UiSpy.dll` 85 KB（配合 UIA；官方 FAQ 要求 Chrome/Edge 加 `--force-renderer-accessibility`） |
| 图像 | `PixVision.dll` 5.8 MB、`d3dcompiler_47.dll` 4.8 MB、`Qt5OpenGL.dll` |
| 键鼠/钩子 | `PixKeyMouse.dll` 75 KB |
| 录制/编解码 | `PixMovie.dll` 575 KB、`PixAVCodec.dll` 8.0 MB、`mediservice` 插件（`wmfengine`/`dsengine`） |
| OCR/AI | `PixOCR.dll` 4.8 MB、`PixOCR2.dll` 5.0 MB、`PixModelRunner.dll` 94 KB、`onnxruntime.dll` 14.4 MB、`PixFormulaRec.dll` 3.1 MB、`PixLatex2MathML.dll` 1.3 MB |
| 模型 | `model\`：5 个 hash `.bin`（9.9/16.6/4.7/21.2/10.8 MB）、`detect.caffemodel`+`detect.prototxt`（OpenCV DNN 文本检测形态）、`sr.caffemodel`+`sr.prototxt`（超分形态）、`paragraph_recognition.onnx` 3.2 MB；另有独立 `OcrModel\` |
| 导出格式 | `plugins\imageformats\`：**qwebp**、qjpeg、qtiff、qico、qsvg、qgif、qtga、qwbmp、qicns |
| 数据/持久化 | `plugins\sqldrivers\qsqlite.dll`、`Data\PinWindowd.sqlite`（贴图库）、`History\_ScreenshotRecord\*.his` |
| 网络/加密 | `libssl-1_1-x64.dll` / `libcrypto-1_1-x64.dll`（OpenSSL 1.1）、`PixNetwork.dll`、`PixAuth.dll`、`PixDownload.dll`、`PixStat.dll` |
| 崩溃上报 | **Sentry + Crashpad**（`crashpad\`、`__sentry-event`、`crashpad_handler.exe`） |
| Shell 集成 | `PixPinContextMenu\PixPinContextMenuExt.dll` + `.msix` |

**版本与依赖（二进制确证，详见证据附录 `docs/26`）**：
- 安装版本 **3.5.5.1**（`ProductVersion`；注意 `FileVersion` 键不存在）；`CompanyName = Shenzhen Shendu Tujing Technology`，代码签名同主体。
- **Qt 5.15.13**；**ONNX Runtime 1.23.2**；**OpenCV 4.13.0 静态链接**（磁盘与导入表都没有 `opencv_*.dll`，靠二进制内源码路径 `...\.xmake\cache\packages\2604\o\opencv\4.13.0\source\modules\...` 确证；构建系统为 xmake）。
- **OCR 不是 PaddleOCR**：`PaddleOCR`/`paddle`/`dbnet`/`crnn`/`ch_ppocr` 全部未命中；确证的是 `detect.caffemodel`+`detect.prototxt`（文本检测）、`sr.caffemodel`+`sr.prototxt`（超分）、`paragraph_recognition.onnx`（producer `pytorch 2.11.0+cpu`）+ OpenCV DNN。
- **`model\*.bin`（5 个）是私有加密容器**：魔数 `7F 79 59 26 2F 36 5B` 等，同族前 7 字节一致，熵 ≈7.5 bit/byte，格式未解 → **SnapClip 不可能复用这些模型**。
- **Manifest：`PerMonitorV2` + `asInvoker` + `uiAccess=false`** → PixPin 默认**不提权**；`GetDpiForMonitor`/`SetProcessDpiAwarenessContext` 未命中，是纯 Manifest 路线。FAQ 里的「以管理员身份运行」是用户可选。
- 录屏：`PixMovie`（Media Foundation 多轨）+ `PixAVCodec`（**静态 FFmpeg**，含 Intel MFX/oneVPL 与 OpenH264）。
- 翻译**硬编码百度**（`api.fanyi.baidu.com`），**无图床**（imgur/sm.ms 未命中）；崩溃上报 Crashpad+Sentry（DSN `…@bugreport.pixpin.cn/1`），遥测独立在 `PixStat.dll`，后端 `api.pixpin.cn`。
- 升级：Inno Setup（`unins000.exe/.dat`）+ `upgradeZIP.bat` 离线 ZIP 热替换（建 `unzip_temp` → 解压 → `xcopy /y /e /i *` 覆盖）。

### 1.2 长截图（对标核心）——完整还原

PixPin 的长截图**不是**"自动滚轮 + 拼接"这一条路径，而是一个**五件套**。以下全部来自 `.qm` 翻译表里的**英文源串**（确证）与官方文档/配图（确证）：

**① 会话控件与交互**
- 控件类名：`LongShotWidget`（主控件）、`LongShotDirCtrl`（方向控件）、`LongShotSaveProgressDialog`（超长保存进度对话框）。
- 选区框 **红=正在捕获 / 蓝=等待开始**；蓝态可自由调整选区；红态只能沿截图方向移动选区。
- 工具栏（官方配图实读）：拖动把手（Move Button）、**实时拼接像素尺寸**、方向切换（默认竖向，可配置）、开始/停止、关闭、**贴图**、**保存**、**复制**。
- 右侧**实时缩略图预览**，其中的**绿色视口框**表示"当前屏幕在长图中的位置"。
- 反复出现的官方劝告（即官方承认的弱点）：内容要复杂静态；**不要把滚动条框进选区**；**滚动要平缓，不要太快**；标题栏占比要小；鼠标悬停变色会导致识别失败（飞书多维表格、微信笔记）。

**② 两条驱动路径都存在**
- 官方长截图文档的主描述是**用户手动滚动**（"use the mouse wheel or control the scroll bar"）。
- 但同时存在 **`Auto Scroll Screenshot`（自动滚动截图）**，且它以故障形态出现在 FAQ（Illustrator 的"滚轮缩放"把自动滚动变成缩放）→ 自动滚动是**注入滚轮**实现的。
- 支撑"手动滚动也能被感知"的机制是**低层鼠标钩子**：用户可见配置项 **`Hook Mouse Wheel`（鼠标滚轮钩子）**、**`Mouse Move Hook`（鼠标移动钩子）**，对应模块 `PixKeyMouse.dll`。
- 还有可绑定快捷键的动作 **`Simulate scroll wheel down/up`（模拟滚轮下滚/上滚）**；配合 3.5.5 新增配置「**响应快捷键**：启用后长截图过程中无法使用鼠标中键滚轮滚动」，可推断长截图期间 PixPin 会占用中键/键盘来驱动滚动。

**③ 上限是分层的，且对用户可见**
- **超长截图模式阈值 = 29,000 px**（**确证**，双重证据）：`.qm` 英文源串原文 + `PixPin.exe` `.text` 中 **9 处** `cmp eax,0x7148 / jle|jg`（`29000 = 0x7148`）；命中点在 `0x1402103e4` 等处，超阈值即置"超长模式"标志位（`or ebx,0x20`）。多个入口（截图/保存/复制/OCR）各自把关。
- 超长模式的三条限制（**确证**，文案 + 代码双证）：**仅支持保存**（`pin` 被禁用；`saveSuperLongImageToFile] jpg is not supported for current logical length` 显式拒绝 JPG）；**PNG 轴长 ≤ 200 万 px**（`.qm` 文案，代码侧 `2000000` 命中 7 处但**未能关联**到长图逻辑，未确证）；**JPG 轴长上限 —— 文案写 `65000`，代码实际是 `65500`（`0xffdc`）**：`VA=0x140c55de7` `mov ecx,0xffdc / cmp [r14],ecx / jg fail`，失败置错误码 `0x2a`。（65500 是 libjpeg 16 位 DCT 系数的经典上限，说明常量可信、文案是近似值。）
- **另一条独立的限制 = 最大拼接范围**（**确证**）：弹窗原文"当前截图已达到最大拼接范围，无法继续拼接…如果当前预览窗口中的绿框未显示，说明当前截图区域已超出最大拼接尺寸…请往回滚动一小段距离，并以绿框重新出现时的截图位置为准"。
- 官方配图实测产物：**`1058 × 502649` px，位深 32**（Windows 文件属性对话框），另有 `1058 x 502020` 的工具栏读数。
  - **强推断（数值证据强、常量未定位）**：拼接上限来自 **32bpp 的 32 位字节数**，即 `INT32_MAX / 4 = 536,870,911 px ≈ 512 MP`。实测 `1058 × 502,649 = 531,802,642 px` = 该值的 **99.06%**，与"分配 4 字节/像素的 `int` 长度缓冲"这一经典写法吻合。
  - **未确证的原因（诚实声明）**：在 `PixPin.exe` 里 `0x7fffffff` 出现 1515 次、`0x20000000` 出现 6030 次，噪声淹没信号；绝对指针搜索、RIP-relative disp32 求解、348 万行全反汇编 grep **全部 0 命中**。该扫描器已用已知引用（`0x140cf3f88` → `VA 0x140210544`，与 objdump 一致）自校验通过，所以"未命中"是真结论而非工具问题。`PixLongImageTile::mMaxHeight` / `TILE_MAX_HEIGHT` 的数值同样未解。
  - **对 SnapClip 的含义**：**不要去反推 PixPin 的精确值**，而是自己定义一个显式常量（如 `MAX_LONG_IMAGE_PIXELS`）并把它做成**可注入的策略值**（见 R5）；量级参考 500 MP。

**④ 失败与超限的产品化恢复路径（docs/19 完全没有）**
- `Match Failed`（匹配失败）：*"Image matching failed. Possible reasons: 1. The scrolling speed is too fast. Try returning to the last successfully matched frame and continue the screenshot."* → 引导用户**回到上次成功匹配的帧继续**。
- 超出最大拼接范围 → 引导用户**回滚到绿框重新出现的位置**。
- 滚过头 → **`Long Screenshot Auto Crop`（长截图自动裁剪）**：*"During long screenshots, PixPin automatically crops extra content when the scrolling direction changes"* / *"simply scroll back to automatically detect and crop excess parts. No post-editing needed, perfect in one take."*
  - 该文案所在翻译 context 是 **`PixTutorialVipPage`**，且另有 `自动裁剪VIP功能，点击升级` → **自动裁剪是付费会员功能**（确证）。

**⑤ 商业定位：长截图是免费核心卖点**
- 官方会员功能清单（Global Mouse、智能擦除、录制键鼠、录制后剪辑、翻译、多语言 OCR、表格识别导出 Excel、LaTeX 公式、云配置同步…）**不含长截图**；超长模式也不在会员列表。
- 只有"自动裁剪"这一增强项是会员功能。→ **长截图 = 免费获取用户的核心武器**，不是增值项。

**⑥ 长截图的内部实现（二进制确证）——对 `docs/19` 的独立验证**

这一节是本次调研**最有价值**的发现：PixPin 的长图实现与 `docs/19` 提出的架构**高度同构**，可以当作 `docs/19` 核心架构判断的独立验证。

| PixPin 的实体 | 证据 | 与 `docs/19` 的对应 | 结论 |
|---|---|---|---|
| `PixStitching` 模块，源文件 `PixStitching\src\DetectDisplacement.cpp` | 内嵌源码路径 + 符号 `matchImageFast`、`DetectDisplacement::clearOffsetMaskByScore`、`PixStitching::tryAddImage`、`PixStitching::tryAddImage] Failed to build legacy half-frame contact input, fallback to full frame.` | `docs/19` §7 的 `ShiftMatcher` / 多带 consensus / 拒绝语义 | **自研引擎**（不是 `phaseCorrelate`；`phaseCorrelate`/`SAD`/`NCC` 字面量全未命中）。做法是**半帧重叠区匹配 + 偏移候选掩码 + 按 score 筛选**，与 §7.2「profile SAD 粗搜 + 多带 consensus + 候选检查」属同一族，但**它的"半帧"策略值得吸收**（见 §1.5 可吸收点） |
| `PixLongImage`（`contactImage` / `removeHead` / `removeTail` / `toImage`） | 符号确证 | `docs/19` §8.1 的 **union 画布** | **确证 union 模型**：`removeHead`/`removeTail` 正是"可裁掉两端"的 union 语义 |
| `PixLongImageTile`（`putImage`，含 `mMaxHeight`） | 符号确证（**数值未解**） | `docs/19` §8.3 的 **`ScrollTile`** | **注意：这是编码侧的 tile，不是画布侧的 tile。** 运行时日志（§1.9）显示**画布本身是一整块 `QImage`（`Format_RGB32`，`bytesPerLine = 宽 × 4`）**，`PixLongImageTile` 服务于编码器输入的组织（分块喂给 libpng/libjpeg）。**不要据此说"PixPin 也是 tile 化画布"** —— 见本节末尾的更正 |
| `PixLongImageFileEncodeThread`（`encodePng` / `encodeJpeg` / `prepareTileImages`） | 符号确证 | `docs/19` §8.3 的 **`ScrollSink` + 流式 PNG** | **确证流式编码**：内嵌 **libpng 1.6.39 + libjpeg**，**逐 scanline 流式编码**，**绕开 Qt 的整图编码器**。→ 与 `docs/24` §S4.2 想做的事是同一个方案（行带 + 流式），连"不用框架自带的整图编码器"这个决定都一样 |
| OpenCV 4.13.0 **静态链接** | 源码路径确证；`PixVision.dll`/`PixOCR*.dll` 导入表无 `opencv_*.dll` | `docs/19` §7.2「OpenCV 不是 v1 常驻依赖，且不得进入 `snapclip-capture` 的默认依赖图」 | **一致**。PixPin 把 OpenCV 静态链进图像/OCR 模块；`docs/19` 的"不进 capture 默认依赖图"与之一致 |

**结论（已在运行时证据下修正）**：
- `docs/19` 的**流式编码**方向被竞品独立验证（PixPin 也是 libpng 逐 scanline 写 + 自带编码线程）。
- 但**"tile 化画布"没有被 PixPin 验证**：PixPin 的画布是**一整块连续 `QImage`（RGB32，`W × H × 4` 字节）**，因此它的可拼长度**天然被 32 位字节数限制在 ~512 MP / W×4 可表示的范围内**（§1.2 ③ 的强推断由此获得机理上的解释，而不再只是数值巧合）。
- → 对 SnapClip 的含义**不变，但理由要改**：`docs/19` 的 tile 化画布**不是"照抄竞品"，而是"绕开竞品撞到的那堵墙"**。这仍然支持 §8.3/§8.4 的设计，而且比"竞品也这么做"更有说服力——**它是相对 PixPin 的一个结构性优势，应当在文档里明确写成优点**，而不是含糊地当成"必要复杂度"。

**⑦ 捕获内核（二进制确证）：纯 WGC，且"零 GDI"**

- `PixWin32CaptureCore.dll` 的导入表**完全没有 `USER32.dll` / `GDI32.dll` / `dwmapi.dll`**；只有 `d3d11.dll`（`D3D11CreateDevice`、**`CreateDirect3D11DeviceFromDXGIDevice`**）、`api-ms-win-core-winrt-l1-1-0.dll`（`RoGetActivationFactory`）、`api-ms-win-core-com-l1-1-0.dll`。
- 字符串确证：`Windows.Graphics.Capture.GraphicsCaptureItem` / `.Direct3D11CaptureFramePool` / `.GraphicsCaptureSession`、C++/WinRT `2.0.240405.15`、WIL `1.0.240803.1`、**`robmikh.common\capture.desktop.interop.h`**（即 `IGraphicsCaptureItemInterop` / `CreateForWindow` / `CreateForMonitor` 的封装头）、源码工程 `Win32CaptureSample\PixWin32CaptureCore.cpp`。
- **会话配置**（这一条 `docs/19` 完全没提，见 R12）：
  - `[PixWinCapture::ApplyCaptureSessionOptions] IGraphicsCaptureSession3 is unavailable; capture border remains enabled`
  - `[PixWinCapture::ApplyCaptureSessionOptions] IGraphicsCaptureSession2 is unavailable; cursor capture remains enabled`
  → PixPin 用 **`IGraphicsCaptureSession3::IsBorderRequired(false)`** 关掉 WGC 的黄色边框，用 **`IGraphicsCaptureSession2::IsCursorCaptureEnabled(false)`** 关掉光标烧进帧。
- `BitBlt`/`PrintWindow` **只出现在 `PixPin.exe`**，用途是贴图窗口的 ROI 刷新（`PinWindowRoiMap::captureRoiImage - PrintWindow failed`）；`PixScreenManager` 的 GDI 用于抓光标位图。**`IDXGIOutputDuplication` 未命中**（不用桌面复制）；`dxgi.dll` 只导入 `CreateDXGIFactory1` 用于枚举。
- **`ScrollPattern` / `TextPattern` 未命中** → PixPin 的长截图**不依赖 UIA 滚动模式**，与 `docs/19` 的"图像拼接为主、UIA 留待 v2"一致。
- **仿真滚轮（反汇编确证，纠正了"兜底"这一误解）**：`PixSystemUtils.dll` 导出**两个互不降级的函数**——
  - `?SimulateMouseScroll@@YAX_N0H@Z`（Ordinal 26）= **纯 `SendInput`**：`bool#1` 轴向（`true` → `MOUSEEVENTF_WHEEL 0x800` 垂直 / `false` → `MOUSEEVENTF_HWHEEL 0x1000` 水平），`bool#2` 方向（`false` 时 `neg`），`int` = 增量；函数内无任何 `PostMessage`。**这就是自动滚动的主路径。**
  - `?SimulateMouseWheel@@YAXVQPoint@@H@Z`（Ordinal 27）= **`WindowFromPoint` → `ScreenToClient` → `PostMessageW(WM_MOUSEWHEEL 0x20A)`**，`wParam = (delta × 120) << 16`；**`hwnd == NULL` 时只写日志、不注入**；函数内无任何 `SendInput`。**这是对指定窗口定点注入的路径。**
  → 与 `docs/19` §6.2「主路径用 `SendInput`」**一致**（有对标依据）；但 `docs/19` 对 `PostMessage` 的**排除**缺少依据（见 R15）——PixPin 是**两条并列路径**，不是兜底关系。
- **`PixPinAuxiliary.exe` 的职责（确证：反汇编 + 导入表 + 用法串）**：它是**"提权代理 + 崩溃重启器 + 独立更新器"三合一控制台工具**（`WINDOWS_CUI`，单入口 `RestartProcessWmain`），**不参与捕获、不是常驻进程、不做注入**。
  - 提权：`SHELL32.dll` 的**唯一**导入就是 `ShellExecuteExW`，配合字符串 `runas`、`Starting process in elevated mode`；**不是 `CreateProcessW`**（后者根本不支持提权）。用法串 `RestartProcess Elevated <exepath>`；主进程侧对应 `PixProgramManage::restartToAdmin/setRunAsAdmin/isAdministrator`，manifest 保持 `asInvoker`。
  - 更新：`Upgrade <TargetRootPath> <UpgradeFilePath> <ExeName>`，是 `upgradeZIP.bat` 逻辑的 C++ 内化版（`unzip_temp` / `xcopy` 流程的日志串逐字对应），带 `Global\PixPinUpdater` 互斥与"等主程序退出再改文件"的进程快照逻辑（`CreateToolhelp32Snapshot` + `WaitForSingleObject`）。
  - 崩溃重启：`CrashRestart <mode> <exepath> <pid> <statepath> <logpath>`，带 **10 分钟限流**（`Restart skipped because the 10-minute limit was reached`）。
  - 捕获相关**未命中清单**：`Windows.Graphics.Capture` / `GraphicsCaptureItem` / `Direct3D11CaptureFramePool` / `d3d11` / `dxgi` / `PrintWindow` / `SetWindowDisplayAffinity` / `SetWindowsHookEx` / `SendInput` **全部未命中**；`Capture` 只命中 CRT 的 `RtlCaptureContext`；`BitBlt` 的命中是它**自身分层圆角窗口的双缓冲绘制**（配 `CreateCompatibleDC` + `CreateCompatibleBitmap`，另有 `Creating hidden UpgradeDialog window` 佐证）。
  → **对 `docs/19` §3.5 / §6.2 的直接含义（重要）**：**PixPin 在 UIPI/高权限窗口场景下不存在第二条捕获通道**——采集只有 WGC 一条路，没有"换通道"的退路，只能降级提示或让用户手动操作。所以 `docs/19` 把"输入失败时的手动模式"当兜底**是对标一致的**；但不要指望"换一条捕获通道"能解决 UIPI。
- **滚轮钩子宿主 = `PixKeyMouse.dll`**（确证）：导入 `SetWindowsHookExA`、`UnhookWindowsHookEx`、`SendInput`、`GetAsyncKeyState`、`GetKeyState`、`WindowFromPoint`、`GetWindowThreadProcessId`、`GetAncestor`、`SetCursorPos`；导出 `PixKeyMouseHook::OnMouseWheelEvent(PixWheelEvent)`、`PixGloabalMouse::OnMouseWheelEvent`；并导入 `PixWindowNotify.dll`（`activeProcessPath` / `refreshActiveWindow`）用于**判定滚轮的目标进程**。→ 这是"用户手动往回滚，PixPin 也能感知并自动裁剪"的实现基础。
- **脚本 API 暴露了长截图会话状态机**（`PixAuth.dll` / 内置 QJSEngine）：`longshot.startStop()`、`longshot.toggleAutoScroll()`、`longshot.cropStart()`、`longshot.cropEnd()`、`longshot.edit()`；配置示例 `"script":"pixpin.screenShotAndEdit()"`。→ 反证长截图会话有**明确的 start/stop、自动滚动开关、裁剪区间**三组能力。
- **截图工具栏是按位标志组**：`ScreenShot.ActBarFlag.*` = `0x100`(基础) / `0x200`(OCR/长图) / `0x300`(编辑/打印)，例如 `LongShot=256`、`OcrTable=512`、`LatexRecognition=514`、`WinRoi=515`、`ImageEdit=769`。
- **长截图自动裁剪是 VIP**：`PixAuth.dll` 内含 `LongScreenshotAutoCrop`、`Auto Stitch`（与 §1.2 ④ 的产品层证据互相印证）。

### 1.3 PixPin 的关键实现取向（对 SnapClip 的设计有直接影响）

| 结论 | 强度 | 证据 |
|---|---|---|
| 长截图选区是**屏幕区域**，且可在会话中沿轴移动（Move Button）以便"目标已滚到底但内容还没截完"时继续 | 确证 | 官方文档 item 3 与使用提示；FAQ「Adobe Illustrator…拖动截图框左侧的移动按钮继续截取」 |
| 手动驱动是一等公民，自动驱动是增强 | 确证 | 官方长截图文档主描述 + FAQ 故障条目 + `Auto Scroll Screenshot` 只是 `LongShotWidget` 内的一个模式 |
| 支持**双向/回滚**，并能**自动裁剪多余部分** | 确证 | `Auto crop when scrolling direction changes`、`滚动截图时只需往回动…一次成型` |
| 存在**两套捕获后端并可用户切换** | 确证（产品层）/ 待二进制定名 | 配置「截图模式：自动（性能模式）/ 兼容模式」；FAQ「Windows 更新后截图延迟、贴图透明区显示马赛克 → 切兼容模式」 |
| 元素检测走 **UIA** | 确证 | FAQ 要求 `--force-renderer-accessibility` / `chrome://accessibility` 勾选 Native accessibility API support |
| 高权限窗口需**以管理员运行**（UIPI） | 确证 | FAQ「在某些软件下快捷键不生效…勾选『以管理员身份运行』」 |
| 历史记录是**可再编辑文档**，不是最终 PNG | 确证 | `.his` 实测：QDataStream 容器，key 序列 `sect/rect/SelectMode/RoundRadius/mark/BgLayer/screens/rect/pixmap`，内嵌 3840×2160 全屏底图 PNG + 48×48 缩略图；100/100 文件一致 |
| 长截图可导出 PDF（含页边距/分页，会员） | 确证 | 3.2.3.1 更新说明 |
| 有 **JavaScript 脚本 API**（`pixpin.getSpRect(PixConst.SpRectScreenUnderMouse)`、`pixpin.directScreenShot(rect, ShotAction.QuickSave)`） | 确证 | 官方 FAQ 脚本示例 + 「配置-脚本」页 |
| 超大贴图 + 开机恢复曾导致**蓝屏** | 确证 | 官方 FAQ 给出安全模式处置步骤 |
| OCR 有 **8,000 px 宽度上限** | 确证 | `.qm`：`当前图像宽度超过 8000 像素，无法识别。` |

### 1.4 运行期证据（最高价值的一手材料）

调研期间发现 **PixPin 3.5.5.1 正在本机运行**（`Get-Process PixPin` → PID 14928，启动于 2026/10/1 16:59:48），因此 `pixpin.log` **不是空文件**，而是 392,999 字节且仍在写入的**当前日志**——其中包含 **2026-10-08 当天两次真实长截图会话**的记录。全部只读快照到 `%TEMP%`，未触碰安装目录。

**① 两次真实长截图的逐字日志（原文未改）**
```
[2026-10-08 09:03:48.451] [3.5.5.1] [info] Qt: [LongShotWidget::closeLongShot] Closing this=0x299e09be800 superLong=false hasResult=true timerActive=true shotRect=466,325 1505x901 pixStitching=0x299df8312d0
[2026-10-08 09:03:48.455] [3.5.5.1] [info] Qt: [LongShotWidget::getExportPixmap] Export after stopping timer this=0x299e09be800 image=size=1505x3241 format=4 bytesPerLine=6020 bytes=19510820 dpr=1 logicalLength=3241 shotRect=466,325 1505x901 pixStitching=0x299df8312d0
[2026-10-08 09:03:48.455] [3.5.5.1] [info] Qt: [LongShotWidget::~LongShotWidget] Destroying widgets this=0x299e09be800 imagePreview=0x299dee39e10 actBar=0x299db219cc0 maskOverlay=0x299dc6d8c80 pixStitching=0x299df8312d0 cropFloatPanel=0x299df1a5820
[2026-10-08 09:46:52.263] [3.5.5.1] [info] Qt: [LongShotWidget::actionBarInit] Crop float panel attached this=0x299e05381f0 actBar=0x299e85a0f30 class=ActionsBar ... cropButton=0x299dbfd9160 class=PixIconButton ... cropFloatPanel=0x299dedbac10 class=CropFloatPanel ... dir=0 autoCropEnabled=false
[2026-10-08 09:47:06.756] [3.5.5.1] [info] Qt: [LongShotWidget::getExportPixmap] Export after stopping timer this=0x299e05381f0 image=size=1178x8966 format=4 bytesPerLine=4712 bytes=42247792 dpr=1 logicalLength=8966 shotRect=1107,665 1178x686 pixStitching=0x299dfacf810
```

**② 从这段日志可以直接确证的事实**

| 事实 | 证据 | 意义 |
|---|---|---|
| 长图成品是**一整块连续位图**，不是分段 | `image=size=1505x3241 format=4 bytesPerLine=6020 bytes=19510820`；`1505×4 = 6020`、`6020×3241 = 19510820` **精确自洽**；第二例 `1178×4=4712`、`4712×8966=42247792` 同样自洽 | 推翻"tile 化画布"的对标假设（见 §1.2 ⑥ 的更正）；也解释了上限来自 `W×H×4` |
| **无 Alpha**：`format=4` = `QImage::Format_RGB32` | 同上 | 长图是 RGB；`bytesPerLine = 宽×4` |
| **`logicalLength` = 拼接长度 = 高度** | `logicalLength=3241` 与 `image=size=...x3241` 一致 | `logicalLength` 是 PixPin 自己的命名，对应 `docs/19` 的 `captured_length_px` |
| **每步位移是可变的（存在可变 overlap）** | `logicalLength / shotRect.height` = `3241/901 = 3.60`、`8966/686 = 13.07`，**均明显非整数** | 反证"固定视口高逐块摆放"（如 `webshot-master` 那样）；PixPin 走的是**真实位移匹配**，与 `docs/19` §7 的路线一致 |
| 组件构成 | `imagePreview`（缩略预览）、`maskOverlay`（**绿框指示**）、`actBar`（`ActionsBar`）、`cropButton`（`PixIconButton`）、`cropFloatPanel`（`CropFloatPanel`）、`pixStitching`（拼接器）、`dir`（方向，`0`=纵向）、`superLong`、`autoCropEnabled` | **与官方文档的 14 项界面元素一一对应**，可交叉验证 |
| `superLong=false`（3241 < 29000）、`hasResult=true` | 第一例 | 印证 §1.2 ③ 的 29,000 px 阈值语义 |
| 两次会话的 `autoCropEnabled=false` | 第二例 | 该用户未启用（VIP）自动裁剪 |
| 内存量级 | `1505×3241×4 ≈ 19.5 MB`；`1178×8966×4 ≈ 42.2 MB`（日志 `bytes` 字段直接给出） | 与 `W×H×4` 完全一致 |

**③ 一个对 SnapClip 直接有用的负面结论：算法路径零埋点**
跨三个日志（30,706 物理行 / 23,135 有效记录行）统计关键词命中：
- `LongShot` 13、`Stitch` 5（**全部只是字段名 `pixStitching=`**）、`logicalLength` 2、`superLong` 1、`autoCrop` 3、`Export` 4；
- **`scroll` / `stitch`（小写）/ `merge` / `match` / `wheel` / `hover` / `WGC` / `GraphicsCapture` / `BitBlt` / `PrintWindow` / `Dwm` / `history` / `license` / `DPI` / `webp` / `onnx` / `mosaic` / `formula` / `translate` / `clipboard` / `memory` / `cache` 全部 0 命中。**
→ 拼接算法内部、推理细节、许可校验**没有任何埋点**。这既意味着我们无法从日志反推它的算法，也提示 SnapClip：**`docs/19` §10.2 的必需指标必须真正实现**，否则产品上线后同样无法定位问题（`docs/24` §11.2 的"位移证据/停止原因/readback"四个必填项正是为此）。

**④ 已发现的错误与降级链（对 SnapClip 是现成的"要避开的坑"清单）**
- `error` 共 17 条（3.5.5.1 期间 0 条）：**16×** `TrigerGraphicEditManage::TrigerGraphicEditManage scene is null`（图形编辑场景空指针，5 个时间簇）；**1×** `".../2025-07-06_09-17-32-0.meta" Not exist!`（数据一致性缺陷）。
- `warning` Top：**`UiSpy::DirectGetRect accLocation failed` 301×**（占全部 warning 的 26%，UIA 元素定位失败）；屏幕分辨率变更 131+64+…（多屏拓扑频繁变化）；**`Synthetic Ctrl+C event detected and filtered` 20×**（PixPin 会注入 Ctrl+C 复制并自我过滤）；`PixTrack network error ... stat2.pixpin.cn/api/track Bad Gateway` 14×；**`Failed to create PixScreenGXDI: "initializeCapture: -2147024809"` 8×** —— `-2147024809` = `E_INVALIDARG`，并**回退到 `PixScreenQt`**。
  → 这是"两套捕获后端 + 失败降级"的**直接运行时证据**，与配置项「截图模式：自动（性能）/兼容」互相印证。**PixPin 的"性能后端"高频失败（8/16 次错误都是它），是它自己承认的弱点。**
- 正常流程被记为 warning（QSS 加载 82×2 等），噪音淹没真问题 → SnapClip 的日志分级要避免这一点。

**⑤ 性能埋点（`RobinLog`，可直接作为 SnapClip 的阶段化指标蓝本）**
```
Spend Time: 188 ms  before ScreenShot: 9 ms  ScreenShot: 40 ms  ScreenShotWidgetInit: 7 ms
  UiSpyInit: 17 ms  ShortCutTipsInit: 0 ms  QrCodeDetectInit: 0 ms
  SetupScreenShotWindow: 0 ms  ShowScreenShotWindow: 63 ms
```
四次实测总量 **87 / 99 / 168 / 188 ms**；4K 全屏抓取 **36–47 ms**；还包含 `QrCodeDetectInit`（二维码识别）。→ 与 `docs/24` §1.2 要求的"一次普通截图 F5 → 导出的 `present_us` / `over16ms`"口径**不同但互补**：PixPin 量的是**分阶段毫秒**，SnapClip 量的是**帧时长直方图**。建议两者都要。

**⑥ 数据格式与持久化（对 SnapClip 的 history/贴图设计有参考价值）**

| 对象 | 结论 | 对 SnapClip 的启示 |
|---|---|---|
| `LocalStorage.data`（30,954 B） | **Qt QSettings `IniFormat`**（不是 SQLite/JSON），两种转义混用（QString 用 `\xHHHH` 四位、QByteArray 用不定长 `\xHH`） | 本地会话态与配置分离；`HistoryShotRectDatas` **字节级 100/100 精确解码** |
| `HistoryShotRectDatas` | **最近 100 次截图选区**（`u32 count` + 100×(`u32 len` + 裸 `QMap<QString,QVariant>`)，每条形如 `rect=QRectF(1169,573,1657,1129)` / `SelectMode="rect"` / `RoundRadius=0.0`，全局物理像素） | **低成本高收益**：SnapClip 记最近 N 次选区（`docs/19` 完全没提），可用 `R`/`Shift+R` 复用 |
| `Config\PixPinConfig.json` | `{键: {t: Unix 秒[, v: 值][, d: 删除时间]}}`（未改动项只落 `t`）；键后缀 `#s.win` 是平台作用域 | 带"最后修改时间 + 软删除"的配置模型，便于云同步与冲突合并 |
| 热键与工具栏 | 动作是**脚本字符串**（F1=`pixpin.screenShotAndEdit()`、F3=`pixpin.pinFromClipBoard()`）；工具栏是**整数位掩码**（`LongShot=256`、`OcrTable=512`、`LatexRecognition=514`、`WinRoi=515`、`ImageEdit=769`） | 与 `docs/19` §10.3 的 `AppEvent` 词表是不同抽象层；位掩码方式值得借鉴（可配置工具栏顺序/显隐） |
| `Data\PinWindowd.sqlite`（106,496 B = 26×4096） | `PinItem` 100 行（`type` Image 98 / Text 2；`pinOnScreen` 全 0；`"group"` 全 `default`；`createTime` 2026-05-14→2026-10-05）+ `PinImageItem` 98 行（`Width/Height`、`OcrResult` 95/98 非空、`WinTitle` 78 个不同、`ProcessName` 22 个不同）+ `sqlite_sequence`。恢复靠 `pinOnScreen`，软删除靠 `closeTime` | 贴图/历史是**四层持久化**（配置 / 会话态 / SQLite / 每实体 sidecar）；`ProcessName`+`WinTitle` 被留存用于溯源 |
| `Data\*.meta`（100 个） | **不是标注图层，而是贴图窗口状态文档**：`QMap<QString,QVariant>`，`QVariant = u32 typeId + u8 保留位(0x00) + payload`（实测 `1=Bool, 6=Double, 10=String, 11=StringList, 19=QRect, 20=QRectF, 26=QPointF, 48=内嵌 UTF-8 JSON, 80=QTransform`）。顶层键：`WinStaysOnTop`/`WinOpacity`/`WinKeepRatio`/`Transform`/`SrcGeometry`/`PinWindowDataRelateFiles`/`CenterPos` 各 100，`OcrTextJson` 98、`ImageDevicePixelRatio` 98、`OcrTextSelectable` 97 | 与 §2 的 `.his` 合起来看：**PixPin 的持久化是"可再编辑状态文档"而非"成品图"** |
| **`OcrTextJson`** | UTF-8 JSON：`{"blocks":[{"box":[[x,y]×4],"textLines":[{"box":[[x,y]×4],"centerPos":[x,y],"text":"...","textType":0|1}]}],"lang":"zh-cn"}` | **带任意四边形定位的 OCR 版面模型**（支持倾斜/旋转文本）+ 行中心 + 类型 + 语种。这是本次对 SnapClip **价值最高的单一格式发现**——`docs/03`/`docs/04` 的 OCR 输出应考虑采用同类版面模型，而不是纯字符串 |
| `Data\*.png` | 99 个，最大 `3840×2088`（整屏）；98 个 `colorType=2`(RGB)，**仅 1 个 `colorType=6`(RGBA)** = `2026-09-25_17-12-55-0.png 1368×1121`（自由/折线选区的透明区，对应 3.5 新特性）；另有 1 个孤儿 PNG、2 个孤儿 `.meta` | 长截图不在 `Data\`；也说明"非矩形选区 → 透明区"需要显式设计（PixPin 3.5 的做法） |
| `model\` / `OcrModel\` | `OcrModel\` **空**；`model\` 确证 `detect.prototxt`+`detect.caffemodel`（OpenCV DNN 文本检测）、`sr.prototxt`+`sr.caffemodel`（超分）、`paragraph_recognition.onnx`（producer `pytorch 2.11.0+cpu`）；5 个 MD5 命名 `.bin` **熵 7.506–7.566、XOR 0x77 后仍不可读 → 加密/混淆模型**（流密码/AES 类，内容寻址命名）。`PixOCR2.dll` 含 `MNN` 75 次 → **新 OCR 走阿里 MNN**；`PixOCR.dll` 含 ONNX 14 + opencv 113 | **这些模型 SnapClip 无法复用**（加密）；但"MNN + ONNX 双运行时"的取向说明 PixPin 在 OCR 上是多后端 |
| 崩溃上报 | **Sentry Native 0.15.2 + Crashpad**（`crashpad_wer.dll`）；`__sentry-event` 是 **MessagePack**；`release=PixPin@3.5.5.1`、`environment=production`、`os=Windows 10.0.26100`；`last_crash=2026-01-14T13:06:31Z`；`metadata`=`DAPC` v1、`settings.dat`=`sdPC` v1 | SnapClip 若要接崩溃上报，这是现成配方 |
| `PixPinAuxiliary.exe` 职责 | **按需启动的"重启/升级中介"**（单入口 `RestartProcessWmain`），**不是常驻、不做注入、不做提权代理**；升级模式证据 `starting upgrade process: ".../Temp/PixPinAuxiliary.exe" args: ("Upgrade", "C:/A_Softwares/PixPin", ".../UpgradeFiles/3.2.3.1.exe", "PixPin.exe")`；常驻的是 `crashpad_handler`（三份副本） | 回答了"非提权主进程如何自更新"——**用独立中介进程** |

**⑦ 本机环境（用于解释为什么只有单屏数据）**
GPU `NVIDIA GeForce RTX 4070 Ti SUPER`，主显示器 `3840×2160`（可用 `3840×2088`），**`PixelRatio: 1`**（无 DPI 缩放），Windows 11 24H2（`10.0.26100`）。→ **本机取不到 125%/150% DPI 与多屏混合的数据**，`docs/19` §11.4 的 DPI 维度仍需自测（与 R10 的关切一致）。

### 1.5 关于"取不到长截图产物"的诚实更正

- 我此前的声明"本机取不到长截图产物样本"**只对了一半**：
  - `History\_ScreenshotRecord\*.his`（100 个）全量扫描：每个文件恰好 2 个 PNG，最大一律 `3840×2160`（+48×48 缩略图）；`Data\*.png`（99 个）最大 `3840×2088` → **确实没有长截图成品可供像素级分析**，**overlap 条带无法直接实测**。
  - **但运行期日志提供了两次真实长截图的完整元数据**（§1.4），足以确证产物模型、`logicalLength`、可变 overlap 的存在、无 Alpha、组件构成与内存量级。
- 因此：**产物形态**已由官方配图（§1.2 ③）+ 运行期日志（§1.4）双重确证；**拼接像素质量**（overlap 比例、有无重复行、sticky 处理效果）**仍无实测样本**。
- 可复用的检测方法（待拿到真实长图即可执行）：对 PNG 逐行取指纹，再在候选 overlap 区间做匹配率反解，即可测出真实 overlap 与是否存在重复行带（方法与反解公式见证据附录 `docs/28` §5.4）。

---

## 2. `docs/19` 评审

### 2.1 立得住的部分（不建议改动）

1. **总体架构判断正确**：活动帧源 + 有界容量 1 流水线 + 实际位移匹配 + union 画布 + 磁盘 tile 输出。PixPin 的 50 万 px 实测**恰好证明这套架构是必要的**（2.1 GB 的 32bpp 连续缓冲不可能在长图路径上反复物化）。
2. **§4.2 的"immediate context 单写者"是正确性论证**，不是性能臆测；§2.2 用实测把前提钉死，符合 AGENTS.md §2。
3. **§8.1 的二维覆盖断言**（沿轴单段连续 + 垂直轴每个有效坐标覆盖完整）与 PixPin 的"绿框"判据是**同一个不变量**，是正确的。
4. **§7.4 的全局重锚定 + keyframe 内存契约（≤64 KB、只留 1 个）** 与 AGENTS.md §4 的"反对无依据的复杂度"一致。
5. **§6.7 的终点二次确认**、**§7.2 的拒绝语义**（"不得把最接近当成功"）与 docs/24 §0.1 的"不许把不确定变成成功"是同一纪律，正确。
6. **§10.3 的事件契约可直接落地**（已核实：`AppEvent` 是枚举、`CaptureEvent` 是结构体、派生 `PartialEq, Eq`、有 `size_of::<AppEvent>() <= 256` 测试、有 `generation()`/`with_generation()`）。
7. **§8.4 删除 journal 崩溃重放**、**§12 的阶段划分**、**§11.2 先建夹具再写算法** 都合规。

### 2.2 P0 级问题（建议在开工前裁决）

---

#### **R1 · 全文没有对标 PixPin，导致 5 处产品级决策缺少基准**

- **证据**：`docs/19` §2.3 的参考实现只有 `Crisp-main`、`ShareX-develop`、`snow_shot`；全文检索无 "PixPin"。而项目定位是"对标 pixpin"。
- **具体落差**：

| 维度 | docs/19 | PixPin（实测/确证） | 影响 |
|---|---|---|---|
| 轴向长度默认上限 | `30,000 px`（§8.4） | 超长模式阈值 **29,000 px**，运行时拼接上限 ≈ **50 万 px** | docs/19 的"超长"起点恰好等于竞品的"平常"起点 |
| 总像素默认上限 | `150 MP` | ≈ **512 MP**（`INT32_MAX/4`） | 差 3.4× |
| v1 滚动方向 | **只向前**（§1.2、§6.2） | 双向；且"滚回去"是**官方指定的失败恢复动作** | 竞品的核心恢复路径在 SnapClip 里不存在 |
| 滚过头的处理 | 无对应能力（§7.4 的重锚定只改逻辑坐标，不改写已提交 tile） | **长截图自动裁剪**（会员功能）：往回滚即自动识别并裁掉多余部分 | 缺一个用户可感知的"善后"能力 |
| 目标滚到底但内容没截完 | 无 | **Move Button**：拖动选区到剩余内容继续截 | 长页面/长列表的常见场景无法完成 |
| 匹配失败 | 停止 + Partial（§3.4） | **"回到上次成功匹配的帧继续"** + 绿/红视口框 | 竞品把失败变成可恢复状态，SnapClip 把它变成终态 |
| 上限对用户可见性 | 技术性出口（`ResourceLimit` / Partial / 裁剪） | **弹窗 + "不再提示" + 明确的回滚指引** | 用户体感差异极大 |

**关于上限取值，本次调研内部有两种意见，我不掩盖它：**
- **我的立场**：`30,000 px` 作为**架构上限**会让 SnapClip 在竞品的主打能力上落后——在 PixPin 里 30,000 px 只是"平常长度"（超长模式的门槛就是 29,000），它的实测产物是 50 万 px 量级。正确做法是**架构支撑到 50 万 px 量级、默认上限做成可注入的策略值**。
- **参考项目调研的立场**（`docs/27` 结论 12）：`30,000 px / 150 MP` 作为**默认上限**在"可打开性"上是保守安全的——Crisp 的硬上限是 `kMaxImageSide = 32767`；PixPin 自己也警告**接近 100 万 px 高度可能无法导出、约 75 万 px 起普通看图软件可能打不开**。
- **综合结论（两者可以同时成立，建议照此落笔）**：
  1. **架构上限**：必须是 50 万 px 量级（tile 化 + 流式写盘，见 R3）。这是相对 PixPin 的结构性优势——**PixPin 因为画布是单块 `W×H×4` 的 `QImage`（§1.4），天然被 32 位字节数卡在 ~512 MP**；SnapClip 的 tile 画布没有这堵墙。**这条应当在 `docs/19` 里明确写成优点**，而不是当成需要辩护的复杂度。
  2. **默认上限（产品策略）**：由 S5 数据 + 用户可见提示共同决定。`30,000 px` 可以保留为"**不提示、不警告**"的安全区，之上按 PixPin 的做法**分级提示**（"你的看图软件可能打不开"），而不是硬性拒绝。
  3. **绝不接受的做法**：在 `S1.8` 的纯内存画布阶段就把 `30,000` 写成 `ResourceLimit` 的边界测试值（见 R5）。

- **建议动作**：
  1. `docs/19` 新增一节「§x 对标基准（PixPin）」，把上表作为**设计约束的输入**固化下来，并把本文 §1 的证据作为附录引用；
  2. `docs/24` 新增 **S5.0「对标基线」任务**（见 R7），冻结一组真实场景集并记录 PixPin 的实际结果；
  3. 明确裁决下列三项**产品范围**（不是技术细节）：
     - **是否支持双向/回滚**：建议至少支持"回滚到已提交覆盖范围内的任意位置"（PixPin 的失败恢复依赖它）；
     - **上限取值**：建议把 v1 目标定为**与 PixPin 同量级**（高度 ≥ 50 万 px、总像素 ≥ 500 MP），或明确写出"v1 有意低于 PixPin，理由是 X"；
     - **Move Button 是否进 v1**：这是 PixPin 解决"目标已到底"的唯一手段，且成本不高（移动选区 = 改变裁剪矩形，模型上不新增概念）。

---

#### **R2 · §4.2 的"GPU 线程单写者"代价过高，且排除替代方案时没有给依据**

- **前提正确**：D3D11 的 `ID3D11DeviceContext`（immediate）确实只允许单线程使用；docs/19 §2.2 的实测也成立（D2D 绘制与 Present 都不需要它）。
- **问题 1 — 事实前提漏了一项**：§2.2/§4.2 说"今天真正使用 context 的只有两个 overlay 线程侧调用点"。**实为 3 处**（仓库核实）：
  - `crates/snapclip-capture/src/windows/providers.rs:110` `read_back_bgra`（全额回读，`FrozenFrame::pixels`）
  - `crates/snapclip-capture/src/windows/providers.rs:171` `read_back_region_bgra`（区域回读）
  - `crates/snapclip-capture/src/windows/win/d2d.rs:667` `read_back_region_bgra`（**带标注导出 `render_export`**）
  - 外加 `crates/snapclip-capture/src/windows/renderer.rs:134-135` 的 `AsyncSampleBuffer::new(device.device(), device.context())`（放大镜）
  → `docs/24` §1.3 与 §S2.2 已按 3 处写，**docs/24 比 docs/19 更准**，docs/19 需同步。
- **问题 2 — 代价被转嫁到已发布的普通截图路径**：为满足"单写者"，必须把**放大镜取色**（`AsyncSampleBuffer`，overlay 每帧轮询的 32×32 取色）搬到 GPU 线程，docs/19 自己承认"可能多一帧延迟"，再额外建一条 P95 门禁。放大镜对长截图**没有任何收益**，纯粹是回归风险。
- **问题 3 — 排除替代方案无依据**：§4.2 写"不引入 deferred context，也不引入第二个 device"，**没有给出任何数据或理由**。AGENTS.md §4 明确要求"性能优化必须有明确依据…不得为了臆测性能而增加不必要的复杂抽象"。这里反向也成立：**为了架构纯度而增加已发布功能的回归风险，同样需要依据。**
- **代价传导**（仓库核实）：`docs/23:75` 把"改变截图 overlay 的线程模型"排除在批准范围外 → `docs/24` §0.4 因此必须为 S2 单独重新取得授权，S2.1–S2.4 变成"整份清单里风险最高"且**无新功能、纯重构**的四个高风险任务。
- **建议动作**：
  1. **不要动摇不变量本身，只把它的表述改准**：把"**进程只能有一个 context**"改写为"**每个 `ID3D11DeviceContext` 有唯一所属线程；debug 构建下跨线程调用即 panic**"。§4.2 的可测性主张（S2.4 的 debug 断言）完全保留，而且更准确。
  2. **为滚动会话给一个独立的 context**（第二 device，或同一 device 的 deferred context），代价是显存与跨 device 共享需 staging —— 这正是需要**用一次 spike 实测**的东西（显存、readback 吞吐、放大镜 P95、导出像素一致）。
  3. **重排 S2**：先做 S2.4（线程断言，纯新增、零回归），再做一次 spike 决定"搬放大镜"还是"独立 context"。若选后者，**S2.0 的授权范围显著缩小，S2.2/S2.3 从"高风险重构"降级为"新增路径"**。
  4. 无论选哪条，**`d2d.rs:667`（带标注导出）这条必须纳入迁移清单**——docs/19 目前漏了它。

---

#### **R3 · §8.3 的 tile 端口自相矛盾，"唯一编码器"没有落脚点**

- **矛盾（同一节内三条互斥描述）**：
  1. 端口签名收 **tile**：`fn write_tile(&mut self, tile: &ScrollTile)`；
  2. "capture 拥有 session 级临时 tile 目录，**写盘与 LRU 都在 capture 内**"；
  3. "导出按画布 `y` 升序读行带…**所需 tile 已被 LRU 换出就从会话临时目录读回**" → **行带组装在 capture**。
  → 若行带组装在 capture，端口就该收**行带**（`write_rows`），而不是 tile；若端口收 tile，则 capture 的临时目录与 LRU 就是第二份副本。
- **量级证明这条不能含糊**：按 §8.4 的规模，`1058 × 500,000` 画布 = 约 `3 × 977 ≈ 2,931` 个 `512×512` tile ≈ **3 GB BGRA**。把 tile 逐个送过端口不可接受。
- **"唯一编码器"无处安放**：§8.3 同时要求"PNG 编码器只有一个实现（输入为行带）"与"capture 不 import `snapclip-history`"。
  - 放 `snapclip-model` → 编解码塞进领域值 crate（其依赖图现在只有 8 个包，且有白名单门禁 `tools/check-dependency-direction.ps1:85-93` 只允许 serde 家族）；
  - 放 `snapclip-history` → capture 到不了；
  - 现有 `snapclip-history/src/image.rs:59` 的 `encode_png` 走 `image::DynamicImage::write_to`，要求整图在内存，**与 §8.4 直接冲突**。
- **建议动作（根因解法）**：
  1. 端口改成**行带 sink**：`begin(meta) -> ScrollSink { write_rows(row_index, rgba_rows), finish(self) -> ArtifactRef, abort(self) }`；
  2. **tile store 与 LRU 完全留在 `snapclip-capture`**，行带组装由 capture 完成，行缓冲 = `画布宽 × 4`，与高度无关；
  3. 唯一编码器由**壳层（`snapclip-history`）实现并通过端口注入**（capture 只流出行带），因此"只有一个实现"与"capture 不依赖 history"同时成立——不需要新建 crate；
  4. `S4.1` 冻结 `ScrollTile` / `ScrollExportMeta` 之前先把这一条定下来，否则会冻结一个错误形状。
- **补充（本轮二进制取证的重大结论，可直接替换"512×512 是我拍的"这个状态）**：PixPin 的 tile 形状与预算是**可推导的公式**，不是任意值——
  ```asm
  ; PixLongImage::contactImage   VA 0x1403f2930-0x1403f373e
  1403f2a93:  call QWORD PTR [rip+0x8dd57f]        ; -> Qt5Gui!QImage::bytesPerLine()
  1403f2a9b:  mov  eax,DWORD PTR [rip+0x117321f]   ; -> [0x141565cc0] = 0x08000000
  1403f2aa2:  idiv ecx                             ; 0x08000000 / bytesPerLine
  1403f2aa6:  mov  QWORD PTR [r15+0x40],rax        ; this->mTileMaxHeight = ...
  ```
  → **`TILE_MAX_HEIGHT = 0x08000000 / QImage::bytesPerLine()`**，即**每 tile 的字节预算固定 128 MiB，高度按画布宽度反算**。该常量在 `.data`（`VA 0x141565cc0`）**全 `.text` 只有 1 处引用、0 个写点**（已用语句级 grep 验证是真只读常量）。
  **两个可直接采纳的结论**：
  1. **tile 宽度恒等于画布宽度（整宽行带），不是正方形块。** 依据：全部 tile API 参数**只有高度轴一维**（`startIndex`/`putSize`/`addedHeight`/`headInsertSize`/`tileMaxHeight`/`imageHeight`/`copiedHeight`/`expectedHeight`/`currentEndIndex`），**无任何 x/column/width 参数**；横向长图靠把**整条 tile 旋转 90°**（`prepareTileImages] Failed to rotate tile image for horizontal output.`）。
     → **`docs/19` §8.3 的"tile 尺寸默认 512×512"是错的形状**：正方形 tile 与同一节要求的"按 y 升序读行带、行缓冲 = 画布宽 × 4"**互相冲突**（后者本来就要求整宽）。**建议改为"tile = 整宽行带，高度由字节预算反算"**，与 R3 的行带 sink 方案一致。
  2. **用字节预算而不是像素尺寸**：`tile_max_height = LONG_IMAGE_TILE_BYTE_BUDGET / bytes_per_line`。对标取值 **128 MiB**（PixPin 的实测值）；若想更保守，取其 1/4–1/2（32/64 MiB）再反算。各分辨率下的对照（可直接引用）：

     | 画布宽 | bytesPerLine | PixPin 的 `TILE_MAX_HEIGHT` | 50 万 px 高图需要 |
     |---:|---:|---:|---:|
     | 1058 | 4,232 | **31,714** | 16 tiles |
     | 1280 | 5,120 | **26,214** | 20 |
     | 1920 | 7,680 | **17,476** | 29 |
     | 2560 | 10,240 | **13,107** | 39 |
     | 3840 | 15,360 | **8,738** | 58 |
  3. **顺带印证 §1.2 ③ 的强推断**：官方产物 `1058 × 502,649` 在该公式下恰好需要 `ceil(502649 / 31714) = 16` 个 tile，而 `16 × 128 MiB = 2^31` 字节——**机制对上了**（PixPin 的尺寸管理整体就是"字节预算 ÷ `bytesPerLine`"的风格）。但**总上限的那条判据本身仍未定位**，所以仍标注为**强推断（数值 + 机制强、判据未定位）**，不可写成确证。
- **对 R5/D2 的连带影响**：默认上限的取值从此有对标依据——**tile 预算 128 MiB（可保守取 32/64 MiB）**，而总上限仍应由 SnapClip 自己的实测数据冻结。

---

#### **R4 · `docs/24` 在驱动模型未定之前冻结 `ScrollStopReason`（排序风险）**

- **证据**：`docs/24` §S0.2 要求把 `ScrollStopReason` 冻结进 `snapclip-model`，并注明"字段一旦冻结就不要在后续任务里反复改"；但该枚举含大量**驱动相关**取值：`InputRejected`、`HorizontalUnsupported`、`UncertainAfterRetries`、`DriftBeyondBudget`、`EndConfirmedByUiAExtent`。而驱动模型（自动优先 vs 手动优先，是否双向）要到 S3 才定，且 **R1 建议重新裁决**。
- **建议动作**：
  - `S0.2` 只冻结**驱动无关**部分：`ScrollAxis` / `ScrollState` / `ScrollOutcome` / `ScrollProgress` 的字段形状 / `AppEvent::Scroll` 变体 + `generation()`/`with_generation()` 分支 + `size_of` 测试；
  - `ScrollStopReason` 的驱动相关取值延到 `S3.1`，与 `DriverCommand`/`DriverEvent` 一起冻结；
  - 若 R1 裁决"双向/回滚进 v1"，`ScrollStopReason` 需要新增 `UserScrolledBack` / `ReanchorAfterBackward` 之类取值——早冻结就是早返工。

---

#### **R5 · §8.4 的默认上限若在 S1.8 被钉死，会锁死产品上限**

- **证据**：`docs/24` §S1.8「上限与资源预算（**纯内存画布版**）」要求"每条上限都有'刚好不触发 / 刚好触发'的边界测试"；§S1.10 据此冻结 tag `scroll-s1`。若此时把 `30,000 px` 写进边界测试，后续提升到 PixPin 量级就必须同时改**核心画布语义与其边界测试**——这正是 AGENTS.md 反对的"在错误的数据结构上继续堆功能"。
- **建议动作**：
  1. 上限必须是**注入的策略值**（`ScrollBudget { max_axis_px, max_total_px, max_temp_bytes, max_export_time }`），由会话配置传入；
  2. `S1.8` 只测"策略被正确执行"（用**测试专用的小值**，如 4,000 px），并断言"上限变化不改变画布语义"；
  3. **生产默认值留到 S4/S5**，用 §R7 的对标基线与实测内存数据冻结；
  4. 在 `docs/19` §8.4 把"30,000 px / 150 MP"明确标注为**占位值、非设计目标**，并写明 PixPin 的实测参照。

---

### 2.3 P1 级问题（建议在对应阶段开工前修正）

#### **R6 · 500k px 量级下，"canvas 落盘留在 scroll driver 线程"这个唯一开放项应提前关闭**
- **证据**：`docs/19` §4.3 让**同一个 scroll driver 线程**承担 SendInput + 稳定等待 + CPU 匹配 + 画布提交编排；§13.1 唯一开放项是"canvas 落盘是否拆线程"，当前按"先不拆、有 profiling 证据再拆"落笔。
- **问题**：该判断建立在 30k px 的隐含假设上。到 `1058 × 500,000`（≈2,931 tile、导出期峰值 I/O 数 GB）时，同一线程既要驱动滚轮又要服务导出，几乎必然成为瓶颈。
- **建议**：把 §13.1 从"开放项"改为"**依赖实测的 go/no-go**"，并在 S5.3 采样里显式包含"会话结束后的落盘/导出阶段占用 driver 线程的时间"，作为拆线程的判据。

#### **R7 · `docs/24` 缺"对标基线"任务**
- **证据**：§S5.1 的矩阵是"wheel step × 动画 × settle × 视口 × 轴向 × 内容"，§S5.2 是内部质量指标；**没有一条把 PixPin 的成功/失败结果作为判据**。而 §0.1 要求每个任务给 before/after 证据、项目定位是"对标 pixpin"。
- **建议**：新增 **S5.0「对标基线（PixPin）」**：
  - 冻结一组**真实场景集**（浏览器长页 / 微信与飞书聊天 / 飞书多维表格 / 代码编辑器 / Windows 资源管理器 / 宽表格 / Electron 应用 / WinUI-UWP 应用 / RDP 会话 / 管理员权限窗口 / 负坐标多屏 / 125% 与 150% DPI），**建议 ≥20 个用例并归档为带哈希的清单**；
  - 对每个用例记录 PixPin 的：成功/失败、失败时的表现、耗时、产物尺寸与格式、是否触发超长模式、是否触发最大拼接范围；
  - 把它作为 S5.1/S5.2 的对照表；S5.4 冻结默认参数时必须能指回这张表。
- **理由**：没有这张表，"只依据数据冻结默认值"（§S5.4）就没有对标含义；有了它，"SnapClip 在 X 个场景里成功率不低于 PixPin"才是可判定的验收。

#### **R8 · v1 驱动主次（自动优先 vs 手动优先）应作为显式裁决，而不是默认**
- **证据**：`docs/19` §6.2 以 `SendInput` 为 v1 主路径，§3.5 把 `ManualPanoramaDriver` 降为"输入失败时的手动模式"（v1 只建接口，见 §12 Phase S3）。PixPin 的实证是**两条路径都有**，且官方文档以**用户手动滚动**为默认描述，自动滚动是后来的增强。
- **两种路线的取舍（供裁决）**：

| | 自动优先（现设计） | 手动优先 |
|---|---|---|
| 重叠率可控性 | 好（闭环步长） | 差（用户可能滚太快，PixPin 官方劝告佐证） |
| UIPI / 管理员窗口 / RDP / VM | 需降级或失败 | 天然可用 |
| 焦点/光标/前台管理 | 必须（§5.3） | 不需要注入，不需要恢复光标 |
| overlay 输入隔离 + controller HWND + 会话级热键 | 必须（§5.1/§5.2，S3.6 高风险） | 大幅缩减为"overlay 输入穿透 + 不抢焦点" |
| 双向/回滚恢复 | 需要 driver 支持反向注入 | 用户自己就是反向输入源 |
| 用户可预测性 | 低（自己动） | 高（自己动） |

- **建议**：
  1. **不要删掉任何一条**，而是把"overlay 输入模型"提前成 **S0 阶段的一次真实集成 spike**。**重要前置事实（本次实测，确证）**：SnapClip **已经**解决了"命中测试穿透"这个问题，而且有实测结论表——
     - `crates/snapclip-capture/src/windows/win/window.rs:62-68` 记录了在同一形状（topmost/全屏/工具窗/无重定向位图）替身上的实测结果：

       | 手段 | UIA 的答案 |
       |---|---|
       | 什么都不做（overlay 挡着） | overlay |
       | `WS_EX_TRANSPARENT` | overlay —— **不起作用** |
       | `WS_EX_LAYERED \| WS_EX_TRANSPARENT` | 页面（但 DirectComposition 窗口不能分层） |
       | **`WM_NCHITTEST` 返回 `HTTRANSPARENT`** | **页面 —— 我们采用的手段** |
       | 在窗口 region 上开洞 | overlay —— 不起作用 |

     - 实现：`windows/overlay/window_host.rs:157-161`（`WM_NCHITTEST => HTTRANSPARENT / HTCLIENT`），跨线程只传一个 `AtomicBool`（`win/window.rs:75-76` `HitTestPassThrough`）。
     - `win/window.rs:47-48` 另有 `is_click_through_layered`，并写明"**只有 `WS_EX_LAYERED` 与 `WS_EX_TRANSPARENT` 同时存在才真正穿透**"。
     → 所以 **B3 的未知项不是"能不能穿透"，而是"穿透的作用域"**。`win/window.rs:71-74` 已明确记录现有设计的暴露面："flag 置位期间真实的鼠标点击也会落到下层窗口——所以守卫只包住那一次可访问性调用（个位数毫秒），而不是整轮查询（实测 20–50 ms，长到足以吞掉用户在光标停下后立刻做的点击）"。
     → **滚动会话需要的是"秒到分钟级"的穿透**，这与现有"毫秒级守卫"是**不同的作用域语义**。可行的做法是把当前的**全局 AtomicBool**升级为**区域感知的命中测试策略**（`WM_NCHITTEST` 拿得到坐标：选区内部返回 `HTTRANSPARENT`、工具栏/把手/边框返回 `HTCLIENT`），这样**不需要 `SetWindowsHookEx`** 也能完成基本的手动滚动。
  2. **不需要鼠标钩子也能做手动滚动**：手动模式下 overlay 无需知道滚轮增量，只需让目标收到滚轮、然后从帧里观察结果。PixPin 之所以有 `Hook Mouse Wheel`，是因为它还要用滚轮增量驱动自动滚动与自动裁剪。→ **SnapClip 的 v1 可以比 PixPin 更简单**；钩子只在实现"按键模拟滚轮"（PixPin 的 `Simulate scroll wheel up/down`）时才需要。
  3. **必须显式处理的隐藏前提：焦点与 hover-scroll**。`windows/overlay/window_host.rs:528-531` 明确写着"**No `WS_EX_NOACTIVATE`**：overlay 必须可激活，否则…"，`hotkey.rs:24-27` 也说明 Esc/Enter 只从 `WM_KEYDOWN` 读（前提是 overlay 拥有会话焦点）。
     → 即 **今天 overlay 在会话期间持有焦点**。而"用户在目标窗口上滚动"要送达目标，只有两条路：
     - ① 系统设置「当我悬停在非活动窗口上时滚动它」开启（`SPI_GETMOUSEWHEELROUTING`，Win10 起默认开启）→ 目标按**悬停**收到滚轮，overlay 保持焦点，`WM_KEYDOWN` 仍可用；
     - ② 该设置关闭 → 必须把焦点交还给目标，overlay 收不到 `WM_KEYDOWN`，此时**只能靠会话级全局热键**。
     → **这正好解释了 PixPin 为什么同时有全局快捷键库（`qxtglobalshortcut.dll`）和鼠标钩子（`PixKeyMouse.dll`）。**
     → 建议：spike 必须**同时测量这两种系统设置下的行为**，并据此决定 v1 是否需要会话级热键；把 `SPI_GETMOUSEWHEELROUTING` 的探测写进 `ScrollSession` 的能力快照。
  4. 该 spike 的结论**决定 v1 是否需要整个 controller HWND + 会话级 `RegisterHotKey` 体系**（即 S3.6 是否要保留原规模）。
  5. 明确裁决 `docs/24` §S3.6 已自认的矛盾（"affinity 失败时 `Paused` 显示 controller" vs "monitor 级捕获要求 controller 不入镜"）；若走手动优先，S3.6 可整体缩水。

#### **R9 · F6 贴图的范围与 PixPin 不一致，需要显式裁决**
- **证据**：`docs/19` §1.2 / §5.5 明确禁止在滚动结束前贴图（"滚动结束前不能 pin 正在变化的 tile/canvas"）。而 PixPin 的长截图工具栏**自带贴图 / 保存 / 复制按钮**，即会话内即可对"当前已拼接内容"操作。
- **建议**：这是**产品决策**而非技术约束。若决定对齐 PixPin，做法很简单：把当前已提交 coverage **快照**成一个带 `Partial` 标记的 artifact（tile store 已存在，代价低），与 §5.5"Partial 必须保留元数据"一致。
- **附带**：PixPin 的"启动时恢复大尺寸贴图 + 开机自启"曾导致蓝屏（官方 FAQ 给出安全模式处置）。`docs/19` 应**显式声明 v1 不做开机恢复贴图**，把这条当作已知风险规避，而不是留给实现者自行决定。

#### **R10 · §7.2"只接受整数像素位移"没有处理 DPI 缩放造成的系统性子像素误差**
- **证据**：§7.2 第 4 步"最终只接受整数像素位移"；§7.4 把 1 px 当随机噪声，预算 ±2 px/100 步；§11.4 把 100/125/150% DPI 列进矩阵但**没有对应判据**。
- **问题**：125%/150% 缩放下，一次滚轮的物理像素位移常为分数（如 37.5 px）。**同方向取整是系统性偏置**，会线性累积，比随机噪声更早触发 `DriftBeyondBudget`。
- **建议**：S5.1 矩阵增加一项测量——"每步位移的小数部分分布"；若确认存在系统偏置，则 union 模型需要亚像素重采样，或按 DPI 分档调参。这条应在 S1.6（union 画布）冻结坐标语义**之前**确认。

#### **R12 · `docs/19` 完全没有提 WGC 的"边框"与"光标捕获"两个会话开关（对标实证的缺口）**
- **证据（二进制确证）**：PixPin 的捕获内核在 `PixWinCapture::ApplyCaptureSessionOptions` 里做了两件事，并各自带了降级日志：
  - `IGraphicsCaptureSession3 is unavailable; capture border remains enabled` → 正常路径会调 **`IGraphicsCaptureSession3::IsBorderRequired(false)`** 关掉 WGC 的黄色捕获边框；
  - `IGraphicsCaptureSession2 is unavailable; cursor capture remains enabled` → 正常路径会调 **`IGraphicsCaptureSession2::IsCursorCaptureEnabled(false)`** 让光标不进帧。
- **为什么 `docs/19` 必须处理**：
  1. **边框**：`docs/19` 用**窗口级** WGC（`CreateForWindow`）取帧。WGC 的黄色边框是叠加在**被捕获窗口**上的，会**进入捕获帧**。长图要拼 500 次，边框会污染每一帧的固定位置，并在 union 画布上留下一条无法解释的色带。`docs/19` §11.3/§11.5 的"无空白/无重复"门禁**抓不到这种污染**（它不是空白，是错误像素）。
  2. **光标**：`docs/19` §7.2 第 5 步把"光标"列为**由 `ValidMask` 排除**的动态内容。但根因解法是**根本不捕获光标**（`IsCursorCaptureEnabled(false)`），而不是捕获后再用掩码排除——掩码排除会额外引入"光标恰好压住有效纹理"的退化情形（§7.3 的 `valid_pixels < minimum` → `Uncertain`）。
  3. §2.2 的能力探测里，**没有任何一项是"`IGraphicsCaptureSession2/3` 是否可用"**；而这两个接口需要 **Windows 10 2004+ / 11**，与 `WDA_EXCLUDEFROMCAPTURE` 的要求同代。`docs/19` §5.1 已为 `WDA_EXCLUDEFROMCAPTURE` 设了降级层，却忘了给同样需要版本探测的会话选项设降级层。
- **建议动作**：
  - `docs/19` §4.1 的能力探测清单增加 `IGraphicsCaptureSession2`/`IGraphicsCaptureSession3` 的可用性探测，并记录到 provider 诊断（与 `wgc-window` / `wgc-monitor` 同处）；
  - §7.2 第 5 步把"光标"从"由 `ValidMask` 排除的动态内容"改为"**在会话选项层禁用光标捕获**；仅在接口不可用时才退回掩码排除"；
  - §11.3 增加一条**像素级断言**："长图产物中不存在 WGC 边框色带"（边框固定位置可精确断言）。

### 2.4 P1 级问题（参考项目调研新增，证据见 `docs/27`）

> 本节的证据来自 `refer/` 逐文件只读调研（Crisp / ShareX / snow_shot / PowerToys 等）与外部技术路线核实，全部附 `文件:行`，详见证据附录 **[docs/27-scroll-capture-reference-survey.md](27-scroll-capture-reference-survey.md)**。

#### **R13 · 【最高优先】§7.2 的"搜索无结果即在该帧之前停止"必须改：一帧动画会毁掉整场会话**
- **冲突**：`docs/19` §7.2 写"任一帧尺寸不符、`delta <= 0`、搜索无结果或超过边长预算，**都在该帧之前停止**"；而 §6.7 又要求"连续两次 `NoMovement` + 稳定相似度 + 画布边缘证据同时成立"才判 EOF。**同一份文档里两处语义冲突**（一处是"一帧即停"，一处是"连续判定"）。
- **为什么必须改（两条独立证据）**：
  1. **snow_shot**：`stitcher.rs:356-390` 对 `NoMotion`/`Indeterminate` **不终止**——记 `StitchBranch::NoMovement`、画布不增长、`previous_raw` 前移、**继续处理下一帧**；只有连续无位移才判 EOF。`types.rs:36-42` 的 `StitchBranch` 枚举里 `NoMovement` 与 `Skip` 都是**正常分支**，不是终态。
  2. **PixPin**：`Match Failed` 的产品文案是 *"Image matching failed… Try returning to the last successfully matched frame and **continue the screenshot**"*（§1.2 ④）——竞品把"这一帧匹配不上"当成**可恢复状态**，而不是会话终态。
- **Crisp 的相反做法只适用于批处理**：`StitchInternal.h:57-59` `if (shift <= 0) break;` 是"一次性拿到 30 帧然后拼"的批处理语义；放到 100–500 步的交互式长会话里，一次 loading 动画/一次滚动条闪动就交付 Partial。
- **建议动作**：
  1. §7.2 的规划阶段改为"**跳过并继续**"：本帧 `Rejected`/`Uncertain` 时**不提交、不扩展画布、前移参照帧**，并累计连续拒绝计数；
  2. 增加容忍度 `max_consecutive_rejections`（建议 3–5，由 S5 数据冻结），**超过才停止**并标 `Partial`，`ScrollStopReason` 用 `AlignmentRejected`；
  3. 把 §6.7 的"连续两次 `NoMovement`"与"连续 N 次拒绝"统一成一条显式的**容忍度策略**，消除 §6.7 与 §7.2 的语义冲突；
  4. 对**尺寸不符**与**目标失效**保留"立即停止"（那是 `Ended(...)` 语义，不是匹配失败）——即把"帧不可用"与"目标没了"分开。

#### **R14 · 缺 `Contained` 分支与"参照系二态"，会在小步长/回滚时反复接受同一位移**
- **证据**：snow_shot 有 `StitchBranch::Contained`（`types.rs:36-42`）——**新帧完全落在既有画布内**（小步长抖动、用户手动回滚一点、平滑滚动动画的中间帧）时画布不增长，并**把参照系从 `Synthetic` 切到 `CanvasWindow`**（`stitcher.rs:472-474`）。
- **`docs/19` 的缺口**：§8.1 的"只为 `new_union - old_union` 分配新范围"**隐含**处理了"不增长"，但**没有处理"此时不能再用上一原始帧当参照"**——继续拿上一帧比，会反复接受同一个位移，或者把 union 当成有新增而误判。
- **建议动作**：
  1. 在 §8.1 显式定义 `Contained` 状态（`new_union == old_union`）；
  2. 引入参照系二态（`previous_raw` / `canvas_window`），`Contained` 时切换；
  3. 加一条测试：**连续 3 次"零新增"输入不得产生任何画布增长，也不得改变逻辑原点**。

#### **R15 · §6.2 把 `PostMessage(WM_MOUSEWHEEL)` 一句话排除，缺少依据且与官方文档冲突**
- **`docs/19` 原文**："主路径不使用 `PostMessage(WM_MOUSEWHEEL)`：浏览器、Electron、自绘控件和嵌套容器处理不一致。"
- **先给一个对 `docs/19` 有利的结论**：**"主路径用 `SendInput`"这个选择本身与 PixPin 一致**（确证）。PixPin 的自动滚动函数是**纯 `SendInput`**：
  ```
  PixSystemUtils.dll  导出 ?SimulateMouseScroll@@YAX_N0H@Z  (Ordinal 26, RVA 0x102A0)
    bool#1 = 轴向: true -> MOUSEEVENTF_WHEEL (0x800) 垂直
                  false -> MOUSEEVENTF_HWHEEL (0x1000) 水平
    bool#2 = 方向: true 原值 / false -> neg（反向）
    int    = 滚轮增量;  INPUT.type=0(INPUT_MOUSE), cbSize=0x28
    函数内**没有任何 PostMessage**
  ```
  → `docs/19` §6.2 的主路径判断**有对标依据**，这一点不必改。
- **真正缺依据的是"排除"这条动作**。PixPin 另有一个**并列（不是兜底）**的定点注入函数：
  ```
  PixSystemUtils.dll  导出 ?SimulateMouseWheel@@YAXVQPoint@@H@Z  (Ordinal 27, RVA 0x10300)
    hwnd = WindowFromPoint(pt)
    hwnd == NULL -> **只写 QMessageLogger::warning，不注入任何事件**
    hwnd != NULL -> ScreenToClient(hwnd,&pt)
                    wParam = (delta * 120) << 16        ; 0x78 = WHEEL_DELTA
                    lParam = MAKELPARAM(clientX, clientY)
                    PostMessageW(hwnd, 0x20A /*WM_MOUSEWHEEL*/, ...)
    函数内**没有任何 SendInput**
  ```
  → 即 PixPin 的证据是"**两条路径并存、各司其职**"：`SendInput` 用于自动滚动（不需坐标），`PostMessage` 用于对**指定窗口**发一次滚轮（需坐标）。**它不是 fallback 关系**——把 `docs/19` 的"排除 PostMessage"改成"实现两条并列路径"才与对标一致。
- **另外两条独立证据说明 `PostMessage` 不该被排除**：
  1. **Microsoft Learn 原文**（winapp-cli UI Automation）：`post-message` "is HWND-targeted and **bypasses UIPI (works across integrity levels)**"；而 `send-input` "goes to whatever window is foreground and **is blocked by UIPI**"。→ 在 UIPI 场景（`docs/19` §3.5 专门为它准备了手动模式）`SendInput` **必然失败**，`PostMessage` 是唯一可能成功的那条。
  2. **snow_shot 的生产实现用的是 `PostMessage`**：`scrollinput.cpp:52-54` `PostMessageW(target, WM_MOUSEWHEEL, ...)`，且**先 `ScreenToClient` 再 `ChildWindowFromPointEx` 逐层下沉到子 HWND**（Chromium 的 render widget host 正是子窗口）。这恰好解释了 Crisp 的相反结论（`ScrollCapture.h:13-16` 说"浏览器会忽略"）：**Crisp 没有做子窗口下沉**。
- **`PostMessage` 的真实局限**（必须写进文档）：应用可以不处理；`GetAsyncKeyState` 类实现看不到修饰键；**WinUI3/UWP 的无窗口控件收不到**；需要一个可用的 HWND。PixPin 在 `WindowFromPoint` 返回 NULL 时**直接放弃**——这是它的一个可改进点（SnapClip 可以退回 `SendInput`）。
- **建议动作**：
  1. `WheelDriver` **实现两条并列路径**：`SendInput`（自动滚动主路径，与 PixPin 一致）+ `PostMessage` 到选区中心下的子 HWND（定点注入）；
  2. 用"**发送后是否观察到位移**"选择与降级，并把实际使用的那条记进诊断。**注意：这属于 SnapClip 相对 PixPin 的增强**（PixPin 的两条路径不互相降级），实现时要在代码注释里写明这是有意超出对标行为；
  3. `InputRejection` 的枚举**直接采用 snow_shot 的 5 类 status 作蓝本**（`SCROLLING_DIAGNOSTICS.md:65-72`）：`Posted`（**不代表应用处理了**）/ `InvalidRequest` / `TargetNotFound` / `CoordinateFailure` / `PostFailed`（access denied）/ `Unsupported`——比 `docs/19` 现有的 `InputRejection` 更能区分失败原因；
  4. 在 `docs/19` §11.3 增加一条**必须实测**的对照：Chrome / Edge / Electron（VS Code、Discord）/ WinUI3 各测一次，两种传输对比（`docs/27` §9 未解疑点 1、2 已列为待验证项）。

#### **R16 · §7.3 的 `band_score` 加分式线性加权没有先例，且 sticky 用"硬掩码/硬失败"容易误停**
- **证据**：snow_shot 用**时序学习的 tile 三分类**（`fixed` / `scrolling` / `dynamic`）+ **乘法式权重**（`region.rs:779-788`）：
  ```rust
  let learned = 1.0 + 1.5*(state.scrolling - 1.0/3.0) - (state.fixed - 1.0/3.0) - (state.dynamic - 1.0/3.0);
  let influence = (state.observations as f32 / 3.0).clamp(0.0, 1.0);
  (1.0 + influence * (learned - 1.0)).clamp(0.1, 2.0)
  ```
  两个 **skip** 是关键：无纹理 tile 不学（`texture < 0.05`）、**歧义 tile 不学**（`direct_similarity * compensated_similarity >= 0.5`，即"既相似又不相似"→ 重复纹理/周期性内容**直接拒绝污染模型**）。
- **`docs/19` 的缺口**：
  1. §7.3 的 `band_score = texture + edge + temporal_stability - dynamic_penalty - sticky_penalty - scrollbar_penalty` 是**加分式**，各权重没有标定方法，也没有参考实现；
  2. §8.2 / §7.5 的"所有带动态时不得伪造成功" + "有效区过小 → `Uncertain`" 是**硬失败**，在"整个界面都是固定 chrome + 一小块内容"这种很常见的目标上会误停。
- **建议动作**：
  1. 把选带改为"**tile 权重（乘法）× 候选得分**"，并把 `sticky`/`scrollbar` 从"扣分项"改为"降权项"（**降权而非剔除**）；
  2. 采用**中性先验 + 观察次数影响**（默认 1/3 三分、`influence = observations/3`，观察不足 3 次保持中性）；
  3. 采纳"**歧义不学习**"这条排除条件（`direct*compensated >= 0.5` 不更新模型）——它是防重复纹理污染区域模型的关键；
  4. 只有当降权后**仍然**没有可用的有效区时才 `Uncertain`（保留为最后手段）。

#### **R17 · 吸收 snow_shot 的"候选唯一性"判据结构（这是重复纹理下最缺的一环）**
- **证据**（`estimator.rs`）：复合置信度 + 四个**并列硬门限** + `|offset|` 小者优先的 tie-break：
  ```rust
  let confidence = (0.40*weighted_inlier_share + 0.25*spatial_coverage
                  + 0.20*residual_gain + 0.15*margin).clamp(0.0, 1.0);
  let accepted = raw_inliers >= MIN_INLIER_MATCHES   // 8
              && inlier_tiles >= MIN_INLIER_TILES    // 4  ← 空间分散度
              && residual_gain >= MIN_RESIDUAL_GAIN  // 0.15
              && confidence >= min_confidence;       // 0.65
  // 排序：得分降序 → |offset| 升序 → offset 升序
  ```
  以及 `scene_cut = direct_similarity < 0.5 && all(candidates.alignment_error > 0.6)`（`estimator.rs:1285-1291`，含 `scene_cut_streak` 连击）。
- **为什么对 `docs/19` 重要**：§7.2 的拒绝清单里有"best/second-best margin 不足或重复纹理导致候选不唯一"，但**没有给出量化门限**。`MIN_INLIER_TILES` 的思想——**要求支持同一 delta 的证据在空间上分散**——正是重复纹理下唯一有效的防线（重复纹理会在多处同时出现，只有空间分布能区分"自洽的一组位移"与"单块内的假一致"）。
- **建议动作（v1 就能做，不需要引入 ORB）**：
  1. 在 profile 多带实现里加硬门限：**至少 K 个不同 y 带各自独立支持同一 delta**（K 由 S5 数据冻结，snow_shot 的 `MIN_INLIER_TILES=4` 是参考量级）；
  2. 加"**相对零位移的残差增益**"门限（`MIN_RESIDUAL_GAIN=0.15`）——防止在"没滚动"时也接受一个微小位移；
  3. 加 **`|offset|` 小者优先**的 tie-break（重复纹理下最省事且有效的启发式）;
  4. 引入 **scene cut** 概念，区分"加载中/内容重排"（应等待重试）与"画面真的换了"（应停止并标 `Partial`）——`docs/19` 目前只有 `Uncertain` 一个含混出口。

### 2.5 P2 级问题（可随任务顺带修正）

#### **R11 · 若干处提法不可实现或缺少限频规则**
1. **§6.3 第 1 步"等待至少一个新的 WGC frame"在静止窗口上会永远等不到**：窗口级 WGC item 在内容未变化时可能不产生新帧（这正是 §4.1 `FramePoll::Idle` 存在的原因）；而 §6.3 把 timeout 只当 watchdog，第 5 步又规定 timeout 后"降低步长、延长等待并重试" → 小步长下会进入"每次都 timeout 再降速"的慢循环。**建议**明确为"等待帧**或** QPC 超时"，超时且无新帧视为**静止候选**进入二次确认。
2. **§4.4"按跳跃步数上限放大搜索窗"不可实现**：丢帧后"错过的滚轮齿数"与"每齿对应多少像素"都未知。**建议**改写为 `上限 = 自上次 accepted 帧以来已注入齿数 × 由历史 accepted 步测得的每齿像素 × 安全系数`；窗口超过视口即 `Uncertain`。另外**首步的 `expected_delta` 应优先用系统设置推算**（`SystemParametersInfo(SPI_GETWHEELSCROLLLINES)` / `SPI_GETWHEELSCROLLCHARS`），而不是盲搜。
3. **§3.3 缺"uncertain commit"的转移规则**：§8.2 允许"保留旧 tile、标记该 commit 为 uncertain 并等待下一稳定帧"，但转移表没有对应行，也没说下一帧到达后是重试同一 `request_id` 还是新开一步。
4. **§10.3 只定义"低频"没有定义限频**：§6.1 每步会产生 `InputInjected / WaitingForFrame / Settled / Committed` 多个事件，若都映射为 `ScrollProgress`，一个 500 步会话就是数千个事件。**建议**补"状态变化必发 + 运行中每 N 步或每 T ms 合并发一次"，并加一条事件预算测试。
5. **§8.3 的 `read_region` 命名错误**：`win/d3d11.rs` 里**没有** `read_region`。正确的是 `read_back_bgra`（`d3d11.rs:254`）、`read_back_region_bgra`（`d3d11.rs:315`）、`FrozenFrame::read_region`（`providers.rs:135`）。`docs/24` §S2.1/§S2.2/§S2.5 也沿用了这个错名。
6. **§8.3"`CaptureArtifactStore::write` 边写边算 blake3"是事实错误**（已独立复核）：`artifact_store.rs:47` 先 `write_atomically(&path, &output.bytes)` 整篇落盘，`:56` 才对内存 `output.bytes` 求 `blake3::hash`；`write_atomically`（`:79`）是纯写入器，不接 hasher。模块自己的注释 `artifact_store.rs:4` 写 "computes the `blake3` fingerprint **while writing**"，与实现不符——`docs/19` §8.3 与 `docs/24` §S4.2 都抄了这句错话。**"写的过程中算 blake3"是 S4.2 的新增能力，不是既有能力**。
   - 补充有利事实：仓库里**已经有流式哈希的现成范式**——`crates/snapclip-history/src/blob_store.rs:151` `let mut hasher = blake3::Hasher::new();`（剪贴板 blob 的流式指纹）。S4.2 可直接照抄这一模式，不必新造。
   - 同时复核了 `crates/snapclip-history/src/image.rs:60-73`：`encode_png` → `bgra_to_rgba(image.bytes())`（第 1 份）→ `encode_rgba_png` → `rgba.to_vec()`（第 2 份）→ `image::RgbaImage::from_raw` + `DynamicImage::write_to` 写入 `Vec<u8>`（第 3 份）。**"≥2 份整图同时驻留"属实，实际是 3 份**。
7. **§4.3 的线程预算表不完整**：只列 overlay / GPU 线程 / scroll driver / export worker 四行，结论"整体只增加 1 个常驻线程"。实测仓库已有**常驻 5 个**（+ `detection_worker` `snapclip-window-detection`、`refinement_worker` `snapclip-refinement`），另有每次 MSAA 调用新建的 `snapclip-msaa-call` 短命线程。应表述为"在既有 5 个之上只加 1 个"。
8. **§2.4 说 Crisp 的 `TestStitch.cpp` 含"重复纹理"回归模型 —— 该用例不存在**（参考项目调研确证）。`tests/TestStitch.cpp` 的 17 个用例是：已知位移（含越界返回 0）、顶部固定 header、相同帧无位移、无关帧不编造、尺寸不一致、5 帧拼长图（逐行相等）、失败时停止并上报、单帧、空列表、行差自身为 0/越界为 `UINT64_MAX`、水平已知位移、水平无关帧与轴不混、4 帧拼宽图、sticky 检测与 1/3 上限、**高 footer 使搜索失效**、footer 只出现一次且在末尾、header 只复制一次。最接近的 `MakeFlat` 是**纯色**（信息不足），不是重复纹理。
   → **这不是文字小节问题**：`docs/19` §7.2 把"重复纹理"列为 Crisp 已验证的回归模型，会让实现者以为这条已有参考防线；而实际上**重复纹理歧义恰恰是 Crisp 设计中最薄弱的一环**（它只有 argmin + 全局阈值，没有 best/second-best margin 判据）。这也正是 R17 要补的东西。**建议把该行改为"已知位移、固定页眉页脚、无关帧、尺寸不一致、水平/垂直、高 footer 使搜索失效"，并明确标注"重复纹理 Crisp 未覆盖"。**
9. **§2.4"ShareX 选择窗口后显示可点击穿透的区域边框"措辞需精确化**：ShareX 选的是**区域**（`GetRectangleRegionAsync`），滚动注入用的是**窗口句柄**（`selectedWindow.Handle`）；那个可穿透的边框窗口是**独立创建的 1 像素环**（`SetWindowRgn(全框 − 空洞)` + `WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW`，`ScrollingCaptureRegionWindow.axaml.cs:92-133`），**不是"选区本身的边框"**。现有表述会让实现者以为边框要跟着选区走。
10. **§2.4"ShareX 滚动方法/步长/延迟可配置"应补上"没有回退链"**：`ScrollMethod` 是用户手选的裸 `switch`（`ScrollingCaptureManager.cs:130-150`），代码里**没有任何自动探测或失败切换**；且它**完全没有 UIAutomation**（全目录 grep `UIAutomation|ScrollPattern|FloatingElement` 等 0 命中）、**没有横向**（无 `HWHEEL`）、**不是双向**（`AutoScrollTop` 只是开始前发 HOME 拉到顶），还有一段**死代码** `IsScrollReachedBottom`（定义了但 `StartCapture` 从未调用）。

---

## 3. `docs/24` 评审

### 3.1 事实断言核实（逐条）

`docs/24` §S2 的"背景事实"共 26 条，**20 条完全属实**、4 条需修正、2 条无法核实。清单如下（只说有问题的；其余属实不再赘述）：

| # | 位置 | 断言 | 结论 | 正确说法（带代码证据） |
|---|---|---|---|---|
| 1 | §0.4 | 范围边界含「OpenCV 作为 `snapclip-capture` 的默认依赖」 | **事实错误** | `crates/snapclip-capture/Cargo.toml` 全文无 opencv；全仓库唯一命中 `.comparison-old/Cargo.toml:21`（旧 Tauri 工程快照）。**应删除该条** |
| 2 | §S2.1 | "今天没有任何跨线程协议可用" | **不符** | 已有 **4 套**"容量 1 信箱 + `Condvar` + `PostThreadMessageW`"：`capture_worker.rs:35`(`WM_APP+18`)、`export_worker.rs:31`(`+19`)、`detection_worker.rs:35`(`+43`)、`refinement_worker.rs:295`(`+44`)。正确说法："没有针对 **GPU context 访问**的协议，但协议模式已有 4 处实例可照抄" |
| 3 | §S2.1/§S2.2/§S2.5 | 把 `read_region` 当作 `overlay/session.rs` 或 `d3d11.rs` 里的名字 | **命名错误** | `session.rs:354` 是**注释**；`read_region` 定义在 `providers.rs:135`；d3d11 侧是 `read_back_region_bgra`（`:315`）与 `read_back_bgra`（`:254`） |
| 4 | §0.5 | 不干净清单 | **漏列自身** | 实际 `git status --porcelain` = ` M docs/19-scroll-capture-design.md` / `?? docs/24-scroll-capture-tasklist.md` / `?? prototypes/demo.html` / `?? scripts/` |
| 5 | §S2.5 | "既有 `read_back_bgra`/`read_region` 的语义不许顺带改" | **需澄清** | 与 AGENTS.md"根因优先"不矛盾（防顺手改坏）；但若 S2 发现**全额回读**（`providers.rs:109-111`）本身是根因，按 AGENTS.md §2 应直接改，不该因这句话保留 |

**被 `docs/24` 引用但已过期的 `docs/23` 状态（S0.1 要对齐的对象）**：

| 位置 | 现状 | 事实 |
|---|---|---|
| `docs/23:270` T6.x | 仍标 `[ ]` | **P6 已落地**（`d7b5708 refactor(P6): the Tauri shell and the Vue front-end are gone`）→ docs/23 错 |
| `docs/23:236-239` T1.6.2/4/5 | 标"**顺延（D3）**" | **已完成**：`overlay/input.rs`(`80bd318`)、`overlay/render_submit.rs`(`bae01d3`)、`overlay/window_restore.rs`(`d257fda`)、`overlay/session.rs`(`8b1839b`)、`overlay/hover.rs`(`ed19d15`) → 文字属实、状态过期 |
| `docs/23:32` G2 门禁 | `cargo test -p snapclip` | 该 package 名不存在（`apps/snapclip/Cargo.toml:2 name = "snapclip-app"`）→ docs/24 §0.2 的纠正正确 |
| `docs/23:241` T1.8 | frame/mask/text 三个 pass"顺延（D3）" | **仍真的顺延**（`win/d2d.rs` 1589 行，`win/d2d/` 下只有 `helpers.rs`/`magnifier_pass.rs`/`tests.rs`） |

**代码里的失效架构注释（AGENTS.md §7/§8 相关，建议随 S2 清理）**：

| 位置 | 内容 | 为什么错 |
|---|---|---|
| `crates/snapclip-capture/src/windows/mod.rs:13` | "through `crate::application::capture_service`" | `crate::application` 不存在（现为 `artifact.rs` + `ports.rs`） |
| `crates/snapclip-capture/src/window_detection/mod.rs:5` | "`platform::windows::capture::win::window` performs the enumeration" | 该转发层已删除 |
| `crates/snapclip-capture/src/window_detection/mod.rs:33` | "Only `TargetKind::TopLevelWindowFrame` exists" | `ClientArea`/`UiElement` 已在 `uia_provider.rs:905-907`、`msaa_provider.rs:202-204` 产出并被 `overlay/hover.rs:139` 消费 |
| `crates/snapclip-capture/src/windows/win/d3d11.rs:250-253` | "the only GPU → CPU transfer on the capture path… happens once" | readback 已改惰性（`providers.rs:91-122`），另有 `read_back_region_bgra` 与 `render_export` |
| `apps/snapclip/Cargo.toml:9-11` | "the Tauri host (src-tauri) already owns the package name `snapclip`" | Tauri 宿主已在 P6 删除 |
| `crates/snapclip-capture/src/windows/mod.rs:7` | "`renderer` — overlay HWND, message loop, input and session lifecycle" | 实际在 `windows/overlay.rs` + `overlay/*` |

### 3.2 清单的结构性问题

1. **`README.md` 与运行时基线矛盾（本次实测，确证）** —— `README.md` 仍写"Tauri 2、Rust 2024、Vue 3、TypeScript、SQLite"，并给出 `npm ci` / `npm run dev` / `npm run typecheck` / `npm run build` / `npm run tauri dev` 五条命令；实测 `package.json` 只有 `ocr:serve` 与 `audit:tokens` 两个 script，**五条命令全部不存在**，`src-tauri/` 目录也不存在。而 `docs/19` §2.1 定义的运行时基线是"workspace + `snapclip-capture` + GPUI 壳"。
   → **建议并入 S0.1**（零成本、高收益）：修正 README，并给 `docs/01` / `docs/02` 打上"Tauri 时代、已被 docs/19/22 取代"的状态标注。否则新实现者会按 README 设置环境并直接失败。
2. **S0.2 的冻结顺序问题** → 见 R4。
3. **S1.8 的上限固化问题** → 见 R5。
4. **S4.1 冻结的类型形状有问题** → 见 R3。
5. **缺对标基线任务** → 见 R7。
6. **S3.6 的自我矛盾**（清单已诚实发现）→ 见 R8。
7. **S4.6"结果进历史"没有覆盖"可再编辑"维度** —— PixPin 的 `.his` 实测是**全屏底图 + 选区 rect + 矢量标注**的可再编辑文档。`docs/24` §S4.6 只要求"滚动结果作为 artifact 进 history，走与普通截图相同的落盘与剪贴板规则"。
   → **架构级建议（需用户裁决，会明显扩大 S4 范围）**：把 history 的存储模型定义为"**规范化文档（底图引用 + 标注树 + 选区/coverage）**，PNG 只是派生视图"。这样滚动长图的"底图"天然就是 tile store，标注只存矢量，`S4.2` 的流式编码器成为**导出**路径而非存储路径——与 R3 是同一个根因，可一并解决。若不做，则应显式写明"SnapClip 的 history 只存最终 PNG，不支持再编辑"，作为已知的产品差异。

---

## 4. 参考项目与外部调研的关键结论

> 完整证据（含全部 `文件:行`、代码片段、竞品表、参考链接、未解疑点）见证据附录 **[docs/27-scroll-capture-reference-survey.md](27-scroll-capture-reference-survey.md)**（776 行）。本节只列影响裁决的部分。

### 4.1 三个参考项目对 `docs/19` 的独立验证（保持现有设计）

| `docs/19` 的判断 | 独立验证 |
|---|---|
| §5.6 右侧采集期实时预览是差异化能力（128 px 交叉轴 / 256 px 轴向 tile / replace-append-prepend / overlap 替换行吸收缩放取整 / hover 单飞 + revision） | **snow_shot 逐项对应**：`screenshotscrollingthumbnailwidget.cpp:23-24`（`kThumbnailExtent = 128`、`kPreviewTileSpan = 256`）、`:186-214,309-449`、`scrollinghoverpreview.h:93-121`；`screenshotscrollingpipeline.cpp:370-382` 的 overlap 替换行注释原文即"absorb all scale rounding into this small edge patch so the retained preview tiles never drift in height" |
| §5.1/§5.6 overlay 必须排除出捕获（`WDA_EXCLUDEFROMCAPTURE`） | **两个独立实现**：snow_shot `windowchrome.cpp:241-256`、PowerToys `WindowCaptureExclusionHelper.cs:23-27`（后者还带 `OSVersion >= 10.0.19041` 判断 + 每会话只告警一次） |
| §6.3 不把固定 `Sleep` 当 settled 的唯一依据 | **反面样本**：Crisp `ScrollCapture.h:57` 硬编码 `settleMs = 260`，无 settled 判定 |
| §6.6 cadence（EWMA 成本、队列压力降速、连续健康样本才恢复） | **snow_shot 的参数几乎可照抄**：`adaptivescrollingcapturecadence.h:9-20`（`min 1 / max 30 / initial 30 / headroom 1.25 / ewma 0.25 / recoverySamples 4 / pressureQueueDepth 2`）、`:55-64`（压力 → 立即 ×0.75）、`:148-168`（恢复每 4 个健康样本只 +1 fps）；另有一个值得吸收的细节：**同时保留 latest 与 EWMA 取 max** 作为阶段成本，防止尖峰被平滑掉（`:79-80,96-98`） |
| §7.2 "先规划后分配" + band early-exit | Crisp `StitchInternal.h:44-71`（`PlanShifts` 模板，竖/横共用，三个停止条件统一裁决）、`Stitch.cpp:32-44`（`BandDifference` 超当前 best 立即返回，branch-and-bound） |
| §7.2 "不在最顶端取带" | **三方独立印证**：Crisp `Stitch.cpp:61-75`（区间 **1/3** 处）、专利 **CN108681428A** `S500`（起始图高度的 **1/4**）+ `S703`（带沿轴均分为 **9** 个矩形区域逐区比较，即"多带 consensus"的专利化表述）、snow_shot 的 tile 权重 |
| §6.7 终点二次确认（`NoMovement` 不等于 EOF） | snow_shot 用"连续 `NoMovement` + 探针"；Crisp 的"单次不匹配即停"是反面样本 |
| §8.2 sticky 只从首帧写 leading、只从最终稳定帧写 trailing；"所有已采样帧都稳定"才算 run | Crisp `StitchVertical.cpp:42-60`（**严格逐像素相等** + 遍历所有帧 + 上限 1/3）、`:148-151`（footer 只从最后使用的帧追加一次）、`Stitch.cpp:174-186`（footer 上方截断搜索区） |
| §10.3 事件契约 | snow_shot 的 `SCROLLING_DIAGNOSTICS.md` 提供现成的字段清单（见 R17 第 3 条的 status 码与 `input_state` 字段） |

### 4.2 三处必须改的（已展开为 R13–R17）

见 §2.4。另外三条独立的事实更正已并入 R11 第 8–10 项（Crisp 无"重复纹理"用例、ShareX 边框的真实构造、ShareX 无回退链/无 UIA/无横向/非双向且有死代码）。

### 4.3 外部技术路线的两个反直觉结论

1. **`PostMessage` 不是"下策"**：Microsoft Learn 原文写明 `post-message` "bypasses UIPI (works across integrity levels)"，而 `send-input` "is blocked by UIPI"；snow_shot 的生产代码正是 `PostMessageW` + `ChildWindowFromPointEx` 下沉到子 HWND。→ 直接改写了 R15。
2. **WGC 不是滚动截图的必要条件，但 PixPin 的实证与它有共同结论**：Microsoft Q&A 里 MS 工程师明确推荐"滚动截图用 **GDI 更稳**，WGC 更适合实时高频捕获"。而 PixPin 的捕获内核是**纯 WGC、零 GDI**（§1.2 ⑦），且它用 `IGraphicsCaptureSession3::IsBorderRequired(false)` / `IGraphicsCaptureSession2` 把边框与光标问题从根上解决。→ `docs/19` 的"窗口级 WGC 为主"**对标基准正确**（PixPin 就是这么做的），但要补上会话选项（R12）。

### 4.4 竞品能力对照（浓缩版，完整表见 `docs/27` §6.3）

| 工具 | 滚动截图 | 横向 | 双向/回滚 | 采集期实时预览 | 失败表现 | 尺寸上限 |
|---|---|---|---|---|---|---|
| **PixPin** | ✅ 含超长模式 | ✅ 可切换 | ✅（反向 → 自动裁剪，VIP） | ✅ 右侧预览 + 绿框位置指示 | 文档列 9 类失败原因 + "回到上次成功帧继续" | 200 万 px（PNG 轴长）/ 实测 50 万 px |
| **ShareX** | ✅ | ❌ | ❌（`AutoScrollTop` 只是先 HOME） | ❌ 仅完成后（PNG 编解码回环 + 拖拽平移） | **绿/黄/红三态**，红=自动停止；黄=best guess | ❌ 无任何上限 |
| **Snagit** | ✅ | ❌ | ❌（官方建议"一次只朝一个方向，不要之字形"） | ✅ 采集时可见滚动 | 只给排查清单；视差网站容易抓歪 | 未验证 |
| **Snipaste** | ❌（官方 feedback #3269 仍是功能请求） | — | — | — | — | — |
| **Crisp** | ✅ | ✅（Auto 先竖后横） | ❌ | ❌（仅进度 toast） | 阈值不过即停，UI 无"部分成功"提示 | `kMaxImageSide = 32767` |
| **snow_shot** | ✅ | ✅ | ✅ **真双向**（`Prepend`） | ✅ patch 预览 + 视口高亮 + hover 放大 | 7 类拒绝原因 + 8 类帧事件，不因单帧失败终止 | canvas / `int` 限制 |
| **CDP 全页截图** | ✅ `captureBeyondViewport` | 整页/元素 clip | n/a | n/a | 错误码/超时 | 浏览器限制 |

**读出的产品结论**：① 横向是差异化能力；② **双向/回滚几乎无人做好**——只有 snow_shot 的 canvas 原语真支持，PixPin 用"反方向 → 自动裁剪"的产品化绕法，ShareX/Snagit 明确不支持；③ **采集期实时预览是强差异化**（只有 PixPin 与 snow_shot）；④ 超大图必须给上限与**用户可见**提示。

### 4.5 位移估计方法在这个场景下的取舍

| 方法 | 重复纹理鲁棒性 | 成本 | 结论 |
|---|---|---|---|
| 行/列 profile SAD / L1 | **差**（周期内容多个 offset 得分接近，argmin 会系统性选错；Crisp 只有 argmin + 全局阈值，**没有 margin 判据**） | 微秒~毫秒 | **适合做粗搜索，但必须补"候选不唯一"判据**（R17） |
| NCC / ZNCC | 中（压低平坦区虚高得分，周期仍多峰） | 毫秒~数十毫秒 | 困难样本的精修手段（`docs/19` §7.2 第 2 档，正确） |
| 相位相关（FFT） | **差且危险**（多个等高峰且峰高不可直接比较） | 数十毫秒 | **不建议作为主路径**（同样的多峰歧义 + FFT 成本 + 峰高不可比） |
| ORB / AKAZE + 内点投票 | **最好**（前提是加空间分布门限） | 数十~上百毫秒，实现量大 | `docs/19` 排到最后作为可插拔选项是**合理的工程取舍**，但要承认这是"以准确率换实现成本"，并把 `MIN_INLIER_TILES` 的**思想**提前到 v1（R17） |

**性价比最高的组合**：低分辨率 profile 粗搜 → 只在候选 ±4~8 px 内做全分辨率精修（`docs/19` §7.2 第 4 步，正确）→ 困难样本才升级到 NCC / ORB；并且**降采样评估失败时要重跑整个评估**（snow_shot `estimator.rs:1062-1081`：无补偿图 / margin ≤ 0 / `precise_alignment_error` 不 < 1.0 px 三个触发条件），比 `docs/19` 现写的"困难样本改用 1/2 降采样"更进一层。

## 5. 与 AGENTS.md 的一致性评估

`docs/19` + `docs/24` 整体**高度契合** AGENTS.md：破坏性重构被明确鼓励（S4.2 删旧编码器、S3.9 不留整图预览、S3.10 不留旧 pin 路径）、before/after 证据纪律严格、"勾选框 ≠ 完成"、不许把 `Uncertain` 变成 `Accepted`。以下是少数风险点：

| # | AGENTS.md 规则 | 相关写法 | 判定 |
|---|---|---|---|
| 1 | §4「不得为臆测性能增加不必要的复杂抽象、缓存、并发机制」 | `docs/24` §S2.1 第 2 条要求"必达车道（容量 ≥1 有界队列）+ **每个在途请求独立响应槽/完成令牌** + 最大并发数"，同条自己又写"默认 1…若 profiling 需要 2，再以数据调整" | **偏高**。并发度锁 1 时，队列 + 逐请求令牌登记可简化为"GPU 线程串行 + 单响应槽 + `request_id` 校验"。建议把"独立响应槽"降级为"数据证明需要并发时才引入" |
| 2 | §4 同上 | `docs/19` §4.2 排除"deferred context / 第二个 device"**未给依据**，而代价是把回归风险转嫁给已发布的放大镜 | **偏高**（见 R2） |
| 3 | §8「没有保留已知无效代码」 | 仓库现存 6+ 处失效架构注释（§3.1 表）与 2 处过期任务清单状态 | **风险**：`docs/24` §S0.1 只要求对齐 `docs/23` 状态，未要求清理代码内失效注释 |
| 4 | §6「数字必须由命令在当前 HEAD 现场产生」 | `docs/19` §0.1 与 `docs/24` §1.1 把 `475/9/0` 写成"当前已知结果" | **可接受**，因为 §S0.1 已把"复跑并填成正式基线"列为第一个任务。**但只要跳过 S0.1 直接引用这些数字，就构成"不真实验证"** |
| 5 | §1「不增加 Adapter / 兼容层」 | §S4.2"不保留旧函数做兼容""没有『受限物化』这条退路"；§S3.9"不保留整图预览兼容实现" | **无冲突，正面案例** |
| 6 | §7「禁止伪完成」 | §2 进度表全 `[ ]`、§12 无记录、"勾选框 ≠ 完成"、滚动特有四个必填项 | **无冲突，写法正确** |
| 7 | §2「根因优先」 | `docs/19` §4.2 是正确性论证；§8.3 要求重构编码器根因（而不是给滚动加旁路） | **一致**。唯一提醒：§2.2 的前提数字错（2 处 → 3 处），裁决**范围**需重算 |

---

## 6. 建议清单（按优先级与决策归属）

### 6.1 需要**用户裁决**的产品/范围决策（建议先答这些）

| 编号 | 问题 | 建议默认答案 |
|---|---|---|
| **D1** | v1 是否支持**双向/回滚**（对应 PixPin 的失败恢复与自动裁剪）？ | **至少支持"回滚到已提交 coverage 范围内"**；不做自动裁剪（那是会员增强）。注意：三个参考项目里**只有 snow_shot 的原语真支持双向**（`StitchBranch::Prepend`），ShareX/Snagit 明确不支持 → 这是差异化机会，也是 R14 的直接动因 |
| **D2** | v1 的**上限**取多少？（分三层答，不要只给一个数） | ① **架构上限**：高度 ≥ 50 万 px、总像素 ≥ 500 MP（tile 画布没有 PixPin 那堵 `W×H×4` 的墙，见 §1.4）；② **不提示安全区**：`30,000 px` 以内不打扰用户；③ **分级提示**：接近"普通看图软件打不开"的量级时按 PixPin 的做法给**用户可见提示**，而不是硬性拒绝 |
| **D3** | **Move Button**（目标到底后拖动选区到剩余内容继续截）是否进 v1？ | **进 v1**。它解决 PixPin 官方文档明写的高频场景，代价只是改变裁剪矩形 |
| **D4** | v1 驱动**主次**：自动优先还是手动优先？ | 先做**一次 overlay 输入模型 spike**（S0）——已知 `HTTRANSPARENT` 是可用杠杆（`win/window.rs:62-68` 实测表），且 overlay 今天**故意不用** `WS_EX_NOACTIVATE`（`window_host.rs:528-531`）；spike 前不改 S3 的结构 |
| **D5** | §4.2 的 GPU context 方案：搬放大镜 vs 滚动独立 context？ | 先落地"**每 context 单线程 + debug 断言**"（零回归），再做 spike 用数据选；不预先排除第二 device |
| **D6** | history 是否演进为**可再编辑文档模型**（对齐 PixPin 的 `.his`）？ | **v1 不做**，但要在文档里显式写明"只存最终 PNG、不支持再编辑"这一产品差异。**附带两个低成本高收益项**：采纳 PixPin 的"记住最近 N 次选区"（`HistoryShotRectDatas` 实测为最近 100 条，§1.4 ⑥）与"OCR 结果存四边形版面 JSON"（`OcrTextJson`，§1.4 ⑥） |
| **D7** | 输入注入**传输策略**：只留 `SendInput`，还是两条都实现？（R15） | **两条都实现**，用"发送后是否观察到位移"选择与降级；`InputRejection` 采用 snow_shot 的 5 类 status。`SendInput` 在 UIPI 下**必然失败**（官方文档原文），只留它等于放弃管理员窗口场景 |
| **D8** | 匹配失败的**容忍度**：一帧即停还是连续 N 次？（R13） | **连续 N 次**（建议 3–5，由 S5 冻结）。Crisp 的"一帧即停"是批处理语义，不适合交互式长会话；snow_shot 与 PixPin 都把它当可恢复状态 |

### 6.2 建议修订 `docs/19`（不需要用户裁决的部分）

| 编号 | 动作 |
|---|---|
| A1 | 新增「对标基准（PixPin）」一节，固化 §1.2/§1.3 的事实（含 29,000 px 阈值、200 万/JPG 65,000 上限、512 MP 强推断、绿框语义、Move Button、Match Failed 恢复、Auto Crop） |
| A2 | §2.2/§4.2：把"2 处 context 使用者"改为 **3 处 + 放大镜**，并补 `d2d.rs:667`（`render_export`） |
| A3 | §2.2：修正 `read_region` 的归属（`providers.rs:135`）与 d3d11 侧名称（`read_back_bgra` / `read_back_region_bgra`） |
| A4 | §4.2：把不变量改写为"**每个 context 有唯一所属线程**，debug 下跨线程调用 panic"；删除未给依据的"不引入 deferred context / 第二 device"，改为"由 spike 数据决定" |
| A5 | §8.3：端口改为**行带 sink**（`write_rows`），tile store/LRU 完全留在 capture，唯一编码器由壳层实现并注入；修正"边写边算 blake3"为"**S4.2 新增增量哈希**" |
| A6 | §8.4：把 30,000 px / 150 MP 标注为**占位值**，上限改为**注入的策略值**；补"上限对用户可见（弹窗 + 不再提示 + 回滚指引）"的产品要求 |
| A7 | §6.3 第 1 步改为"等待帧**或** QPC 超时"；超时无新帧按**静止候选**处理 |
| A8 | §4.4 的丢帧搜索窗改为可实现的公式；补 `SPI_GETWHEELSCROLLLINES` / `SPI_GETWHEELSCROLLCHARS` 作为首步 `expected_delta` 的依据 |
| A9 | §3.3 补"uncertain commit"的转移行与 `request_id` 规则 |
| A10 | §10.3 补事件**限频规则**（状态变化必发 + 每 N 步/每 T ms 合并）与事件预算测试 |
| A11 | §4.3 线程预算表补齐既有 5 个常驻线程，表述改为"在既有 5 个之上 +1" |
| A12 | §5.5 显式声明"**v1 不做开机恢复贴图**"（规避 PixPin 的蓝屏类风险） |
| A13 | §7.2 补"DPI 缩放下的小数位移"这一误差源与对应判据（与 §11.4 的 DPI 维度对齐） |
| A14 | §13.1 的"canvas 是否拆线程"改为"依赖实测的 go/no-go"，并把测量项写进 §10.2 |
| A15 | **§7.2 改为"跳过并继续"**：本帧 `Rejected`/`Uncertain` 不提交、不扩展画布、前移参照帧，累计到 `max_consecutive_rejections`（建议 3–5）才停止并标 `Partial`；与 §6.7 的连续判定合并成一条**容忍度策略**（见 R13） |
| A16 | **§8.1 补 `Contained` 分支 + 参照系二态**（`previous_raw` / `canvas_window`），并加"连续零新增不得改变画布与逻辑原点"的测试（见 R14） |
| A17 | **§6.2 改为"两条并列注入路径"**（`SendInput` 主路径 + `PostMessage` 到子 HWND 定点），并采纳 snow_shot 的 5 类注入 status；写明"两条互相降级"是 SnapClip 超出 PixPin 的**有意增强**（见 R15） |
| A18 | **§7.3 的选带改为乘法式权重**（tile 权重 × 候选得分），`sticky`/`scrollbar` 由"扣分"改为"降权"；采纳"歧义不学习"（`direct × compensated >= 0.5` 不更新模型）与"观察不足保持中性先验"（见 R16） |
| A19 | **§7.2 补量化门限**：至少 K 个不同带/空间位置独立支持同一 delta（对标 `MIN_INLIER_TILES = 4` 的量级）、相对零位移的残差增益门限（对标 `MIN_RESIDUAL_GAIN = 0.15`）、`|offset|` 小者优先的 tie-break；并引入 **scene cut** 以区分"内容重排（等待）"与"画面换了（停止）"（见 R17） |
| A20 | **§8.3 的 tile 形状改为"整宽行带 + 字节预算反算高度"**（对标 `TILE_MAX_HEIGHT = 128 MiB / bytesPerLine`），删除"tile 尺寸默认 512×512"（见 R3 补充） |
| A21 | **§2.4 更正 Crisp 的测试清单**（无"重复纹理"用例）并对齐 ShareX 的边框构造与"无回退链/无 UIA/无横向/非双向"事实（见 R11 第 8–10 项） |
| A22 | **§4.1 能力探测补 `IGraphicsCaptureSession2/3` 可用性**；§7.2 第 5 步把"光标由 `ValidMask` 排除"改为"**会话选项层禁用光标捕获**，接口不可用才退回掩码"；§11.3 增加"长图中不存在 WGC 边框色带"的像素断言（见 R12） |

### 6.3 建议修订 `docs/24`

| 编号 | 动作 |
|---|---|
| B1 | **S0.1 扩容**：除对齐 `docs/23` 外，一并修正 `README.md`、标注 `docs/01`/`docs/02` 已过时、清理 §3.1 表里的失效架构注释 |
| B2 | **S0.2 缩小**：只冻结驱动无关词表；`ScrollStopReason` 延到 S3.1（见 R4） |
| B3 | **新增 S0.7「overlay 输入模型 spike」**：验证 `WS_EX_NOACTIVATE` / `WS_EX_TRANSPARENT` / 分层 / 命中测试 / 低层鼠标钩子 + 全局快捷键的组合，产出决定 v1 是否需要 controller HWND + 会话级热键的结论 |
| B4 | **新增 S2.0.5「GPU context 隔离方案 spike」**：搬放大镜 vs 滚动独立 context，测显存、readback 吞吐、放大镜 P95、导出像素一致 |
| B5 | **S2.2 必查清单补 `d2d.rs:667`**（`render_export`）——docs/24 已包含，但 docs/19 漏了，两份要对齐 |
| B6 | **S1.8 改为策略注入 + 测试专用小值**（见 R5） |
| B7 | **S4.1 冻结前先解决端口形状**（见 R3） |
| B8 | **新增 S5.0「对标基线（PixPin）」**（见 R7） |
| B9 | **S3.6 按 B3 的结论重写**（可能整体缩水） |
| B10 | **S5.2 增补判据**：`Accepted` 的证据链之外，增加"PixPin 能成功而 SnapClip 失败"的场景必须**逐条给出根因分类** |
| B11 | **§0.5 的不干净清单补上 `docs/24` 自身**；并按 AGENTS.md 要求由用户确认归属后再动手 |
| B12 | **S1.1/S1.3/S1.9 增加"容忍度策略"**：拒绝计数状态机 + `max_consecutive_rejections`（建议 3–5），并消除 S1.9 与 S1.3 之间"一帧即停 vs 连续判定"的语义冲突（见 R13） |
| B13 | **S1.6 增加 `Contained` 与参照系二态**，并加"连续零新增不改变画布与逻辑原点"的测试（见 R14） |
| B14 | **S3.2 增加 `PostMessage` 定点注入路径 + 5 类注入 status**，并在验收里加入"Chrome / Edge / Electron / WinUI3 两种传输对照"的实际测量（见 R15） |
| B15 | **S1.4 的选带改为乘法式权重 + "歧义不学习" + 中性先验**，并加一条"样本不足时保持中性"的断言（见 R16） |
| B16 | **S1.2/S1.3 增加三项量化门限**（空间分散度、残差增益、`|offset|` tie-break）与 **scene cut** 状态（见 R17） |
| B17 | **S4.1 的 tile 类型改为"整宽行带"**，尺寸由 `LONG_IMAGE_TILE_BYTE_BUDGET / bytes_per_line` 反算（对标 128 MiB，保守可取 32/64 MiB），并在 S5.3 记录实际 bytes/tile（见 R3 补充） |
| B18 | **S5.3 增加"单帧拒绝次数分布"与"scene cut 命中次数"两项指标**——它们是 R13/R17 的调参依据，而 PixPin 恰恰**没有这些埋点**（§1.4 ③），SnapClip 必须自己做 |

### 6.4 建议的落地顺序（不改阶段划分，只调整内部次序）

```
S0.1 基线 + 文档一致性修复（B1）
S0.2 冻结驱动无关词表（B2）
S0.7 overlay 输入模型 spike（B3）      ← 新增，决定 S3.6 规模
S0.3–S0.5 夹具（合成帧 / 可控窗口 / 时间与故障注入）
S0.8 注入传输对照实测（B14）           ← 新增，Chrome/Edge/Electron/WinUI3 × {SendInput, PostMessage}
S1.x 纯拼接核心（S1.8 用策略注入值；S1.1/S1.3 带容忍度状态机；S1.6 带 Contained 与参照系二态；
                  S1.4 用乘法权重；S1.2/S1.3 带三项量化门限与 scene cut）   ← 见 R13–R17
S2.0 授权 + S2.0.5 context 方案 spike（B4）  ← 决定 S2.2/S2.3 的实际工作量
S2.x 帧源与 GPU 服务（S2.6 带 IGraphicsCaptureSession2/3 探测与降级）
S3.x 驱动与交互（S3.2 两条注入路径 + 5 类 status；S3.6 按 B3 结论）
S4.x 画布落盘与导出（S4.1 先定端口形状与整宽行带 tile）
S5.0 对标基线（B8）→ S5.1–S5.4 矩阵与参数冻结（S5.3 加拒绝次数分布与 scene cut 命中）
```

**这份顺序的意图**：把**花几十行代码就能避免的坑**（文档一致性、注入对照实测、容忍度、`Contained`、量化门限）全部提到 S1 阶段之内或之前；把**必须靠实测数据才能定的东西**（上限默认值、context 方案、overlay 输入模型、步长与阈值）留到 S5。这与 AGENTS.md"根因优先"和"性能优化必须有依据"一致：**能靠推理确定的立刻做，不能靠推理确定的绝不拍。**

---

## 7. 风险与未解项

### 7.1 已识别的主要风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| 上限取值（30k vs 500k）定错 | 要么产品在核心卖点上直接输给 PixPin，要么后期返工核心画布语义 | R1/R5 + D2 三层取值：架构上限 50 万 px / 不提示安全区 30k / 分级用户提示 |
| S2 的前置重构（搬放大镜）引入普通截图回归 | 已发布功能退化、门禁变红、需要额外授权 | R2：改为"每 context 单线程断言" + 独立 context 备选 |
| 端口形状与 tile 形状冻结错误（tile vs 行带、正方形 vs 整宽） | S4 整体返工 | R3 + A20/B17：S4.1 前先裁决，tile 用"整宽行带 + 字节预算反算高度" |
| **一帧匹配失败即终止会话**（R13） | 100 步会话被一次动画/懒加载毁掉，交付 Partial | A15/B12：容忍度策略，连续 3–5 次才停 |
| overlay 输入模型未验证就按"隐藏 overlay + controller + 热键"实施 | S3.6 高风险任务可能整块白做 | R8/A（B3）：S0 的 spike 先行；已知 `HTTRANSPARENT` 是可用杠杆、overlay 今天故意不用 `WS_EX_NOACTIVATE` |
| **只留 `SendInput` 一条注入路径**（R15） | UIPI 场景必然失败且无退路；而 PixPin 在此**也没有第二条捕获通道** | A17/B14：两条并列路径 + 5 类 status + 四种目标的实测对照 |
| 选带/接受判据缺少"候选不唯一"量化门限（R16/R17） | 重复纹理页面误接受错误位移（最严重的正确性风险） | A18/A19/B15/B16：乘法权重 + 歧义不学习 + 空间分散度/残差增益门限 + scene cut |
| `docs/23` 与仓库状态不一致被带进实现 | 实现者按过期清单工作 | B1：S0.1 一次对齐 |
| 未测基线被当作基线引用 | 违反 AGENTS.md §6，后续 before/after 失去意义 | S0.1 必须真跑；本文所有"未核实"项已显式标注 |
| **参考项目证据只存在于会话中**（若只写进聊天而不落盘） | 后续实现者无法复核行号 | 已落盘：`docs/26`（PixPin 静态取证）、`docs/27`（参考项目调研）、`docs/28`（运行期数据与日志） |

### 7.2 未解疑项（需要进一步证据）

**本次调研已解决、从疑点清单移除的：**
- ✅ **PixPin 的捕获后端**：**纯 WGC、零 GDI**（`PixWin32CaptureCore.dll` 不导入 `user32`/`gdi32`/`dwmapi`，基于微软 `Win32CaptureSample` + `robmikh.common`）→ `docs/19`"窗口级 WGC 为主"的对标基准**成立**。
- ✅ **`PixPinAuxiliary.exe` 的职责**：提权代理 + 崩溃重启器 + 独立更新器，**不参与捕获**。
- ✅ **tile 尺寸**：`TILE_MAX_HEIGHT = 128 MiB / bytesPerLine`，整宽行带（见 R3 补充）。
- ✅ **仿真滚轮的两条路径**：两个互不降级的函数（`SendInput` / `PostMessageW`）。
- ✅ **`Data\*.meta` 与 `PinWindowd.sqlite` 的结构**：见 §1.4 ⑥。
- ✅ **日志关键词与错误分类**：见 §1.4 ③④（结论是"算法路径零埋点"，这本身就是要给 SnapClip 的教训）。

**仍然未解、且明确标注的：**
1. **PixPin 的"最大拼接范围"总上限判据未定位**。tile 级机制已确证（128 MiB/`bytesPerLine`），且 `16 tiles × 128 MiB = 2^31` 与实测 99.06% 吻合，因此**强推断（数值 + 机制强、判据未定位）**。已用 6 种静态手段尝试（绝对指针、RIP-relative disp32、348 万行全反汇编 grep 等）**全部 0 命中**，且扫描器已用已知引用自校验通过 → "未命中"是真结论。**不要再投入**（唯一可行的是动态附加调试器，超出本次只读边界）。→ 结论：**SnapClip 不应反推 PixPin 的精确值，而应定义自己的 `MAX_LONG_IMAGE_PIXELS` 并强制 tile 化。**
2. **`PostMessage(WM_MOUSEWHEEL)` 在 Chromium/Electron 上到底行不行**：Crisp 注释说不行（未做子窗口下沉），snow_shot 生产代码用 `ChildWindowFromPointEx` 下沉且可用；PixPin 也用它做定点注入。**必须在真实目标上实测**（Chrome / Edge / Electron / WinUI3 各一次，两种传输对比）。
3. **UIPI 的方向性**：官方文档说 `post-message` bypasses UIPI，而 `send-input` 被 UIPI 拦；但"`PostMessage` 到**提权窗口**是否真的可行"未验证（snow_shot 把 access denied 列为可能）。→ 直接决定"以管理员运行的浏览器/记事本"能否被抓。
4. **`captureBeyondViewport` 对懒加载与 `position: fixed` 的实际行为**未验证（官方只有一句描述；headed 模式可靠性存疑）。→ 影响 `docs/19` §9.1 `NativeFullPageStrategy` 的可行性判断。
5. **`SetWindowDisplayAffinity` 对 WGC 窗口捕获与显示器捕获是否都生效**未验证（两份参考实现的注释都暗示行为可能不同）。→ 影响 R12 与 §5.6 的降级层设计。
6. **本机环境是单屏、`PixelRatio: 1`**（§1.4 ⑦）→ **取不到 125%/150% DPI 与多屏混合的任何数据**，R10 的"系统性子像素误差"仍需自测。
7. **PixPin 长图的 overlap 比例与是否含重复行带**无实测样本（只有 `logicalLength / shotRect.height` = 3.60 / 13.07 这两个非整数比可推断"存在可变 overlap"）。检测方法与反解公式见 `docs/28` §5.4。
8. **`cargo test` 真实数字未复现**：`475 passed / 9 ignored / 0 failed` 与分包数字自洽（app 56+3、capture 345+6、history 51、model 23），但本次未实际运行；`--all-targets`、壳 UI 测试、三个 `#[ignore]` 探针均未跑。
9. **`docs/24` §S4.2 关于 `image 0.25.10`（`CompressionType::Fast` + `FilterType::Adaptive`）与裸 `png::Encoder`（`FilterType::Sub`）的差异推断**未展开 crate 源码核对。另有一处文档未指明的连带影响：走 `png::Encoder::stream_writer()` 需把 `png = "0.18"` 加为**直接依赖**，`snapclip-history` 的包数（现 47）会变，门禁记录需同步预期；`capture` 仍 30 包不受影响。
10. **`Cargo.lock` 里的 `image-webp`**（`3401/3414/5745/5762`）来自哪条子图未定位（`cargo tree -p snapclip-history` 不含它）。"仓库没有 WebP 编码路径"在**我们自己的代码**成立；建议把 `docs/19` §8.3 的措辞限定为"workspace 代码无 WebP 编码路径，`snapclip-history` 的 `image` 未启用 webp feature"。
11. **PixPin 的 `model\*.bin` 私有加密容器格式未解**（熵 7.5、XOR 0x77 后仍不可读；新增线索是 `PixPinAuxiliary.exe` 里的完整 bcrypt 栈 = PBKDF2 + 哈希 + 对称加密，但那是**升级包**的密钥派生，未必与模型同源）。**已决定收口**，因为对 SnapClip 的结论不受影响：**这些模型无法复用**。

---

## 附录 A：可复核命令

```powershell
# --- 文档与仓库 ---
git -C D:\100_Projects\110_Daily\SnapClip status --porcelain
git -C D:\100_Projects\110_Daily\SnapClip tag
cargo tree -p snapclip-capture -e normal   # 30 包，无 tauri/wry/gpui-kit
cargo tree -p snapclip-history -e normal   # 47 包，image v0.25.10（无 image-webp）
cargo tree -p snapclip-model   -e normal   # 8 包
cargo check -p snapclip-app                # 0 error, 1 warning: unused variable: `content_label`

# --- 附件文档哈希核对 ---
Get-FileHash docs\19-scroll-capture-design.md -Algorithm SHA256   # 962ca786…（76845 B）
Get-FileHash docs\24-scroll-capture-tasklist.md -Algorithm SHA256 # 3ed24a11…（84897 B）

# --- PixPin 安装目录（只读） ---
Get-ChildItem -Recurse -Force C:\A_Softwares\PixPin | Select-Object FullName,Length

# --- .his 全量扫描（内嵌 PNG 与尺寸） ---
python $env:TEMP\his_probe.py

# --- Qt 翻译表（英文源串 + 中文译文，UTF-16BE） ---
python -c "d=open(r'C:\A_Softwares\PixPin\language\zh-cn.qm','rb').read(); print(d[:16].hex())"
python $env:TEMP\qm3.py    # → $env:TEMP\qm-long.txt / qm-nums.txt
python $env:TEMP\qm6.py    # → $env:TEMP\qm-en-strings.txt

# --- 官方配图（长截图 UI / 上限弹窗 / 实测尺寸） ---
Invoke-WebRequest https://pixpin.com/docs/assets/max-height-reached.dbea7960.png -OutFile $env:TEMP\pixpin-assets\max-height-reached.png
```

## 附录 B：结论强度总表

| 结论 | 强度 | 依据 |
|---|---|---|
| PixPin 长截图超长模式阈值 = 29,000 px；PNG 轴长上限 200 万 px、JPG 65,000 px；超长模式仅支持保存 | **确证** | `.qm` 英文源串 + 中文译文逐字（`language\zh-cn.qm`） |
| PixPin 存在"最大拼接范围"限制，并以"绿框是否显示"作为覆盖完整性判据，引导用户回滚 | **确证** | `.qm` 串 + 官方配图与文档 item 11 |
| PixPin 有自动裁剪（回滚即裁掉多余部分），且是**会员功能** | **确证** | `.qm` 串 + `自动裁剪VIP功能` + context `PixTutorialVipPage` |
| PixPin 同时有手动滚动与自动滚动；用**低层鼠标钩子**感知手动滚动 | **确证**（钩子机制）/ **强**（自动滚动用注入） | `Hook Mouse Wheel` / `Mouse Move Hook` / `PixKeyMouse.dll` / FAQ Illustrator 条目 |
| PixPin 支持双向/回滚 | **确证** | 上述自动裁剪串 + `Match Failed` 恢复指引 |
| PixPin 实测产物可达 1058 × 502,649 px（位深 32） | **确证** | 官方配图内的 Windows 文件属性对话框 |
| 拼接上限 ≈ 512 MP（`INT32_MAX/4`） | **强推断** | 实测 531,802,642 = 536,870,911 的 99.06% |
| PixPin 历史记录是可再编辑文档（全屏底图 + 选区 + 矢量标注） | **确证** | 100 个 `.his` 全量扫描 + 单文件 hex 结构；另见 §1.4 ⑥（`Data\*.meta` 是**贴图窗口状态文档**，与 `.his` 是两套东西） |
| PixPin 有两套捕获后端（性能/兼容模式） | **确证** | 官方 FAQ 与配置项；**运行期证据**：`Failed to create PixScreenGXDI: "initializeCapture: -2147024809"`（`E_INVALIDARG`）8 次并回退 `PixScreenQt`（§1.4 ④） |
| PixPin 元素检测走 UIA | **确证** | 官方 FAQ 的 `--force-renderer-accessibility` 指引；日志 `UiSpy::DirectGetRect accLocation failed` 301 次 |
| PixPin 捕获后端 = **纯 WGC、零 GDI** | **确证** | `PixWin32CaptureCore.dll` 不导入 `user32`/`gdi32`/`dwmapi`；内嵌 C++/WinRT 2.0.240405.15 + WIL + `robmikh.common\capture.desktop.interop.h` + `Win32CaptureSample` 源路径（见 §1.2 ⑦） |
| PixPin 用 **`IGraphicsCaptureSession3::IsBorderRequired(false)`** 关 WGC 黄框、用 **`IGraphicsCaptureSession2`** 关光标捕获 | **确证** | 两条降级日志串逐字（§1.2 ⑦） |
| PixPin 的 tile 高度 = **128 MiB ÷ `QImage::bytesPerLine()`**，tile 宽度 = 整画布宽度 | **确证** | `PixLongImage::contactImage` 反汇编 `idiv` + `.data` 常量 `0x08000000`（1 处引用、0 写点）；tile API 只有高度轴参数（§R3 补充） |
| PixPin 的**画布本身是一整块 `Format_RGB32` 连续位图**（无 Alpha、`bytesPerLine = 宽×4`、`logicalLength` = 高度） | **确证** | 运行期日志两例数值三重自洽（§1.4 ②） |
| 拼接总上限 ≈ 512 MP（`INT32_MAX/4`） | **强推断（数值 + 机制强、判据未定位）** | 实测 `531,802,642 = 536,870,911 × 99.06%`；`16 tiles × 128 MiB = 2^31` 机制吻合；6 种静态手段 0 命中（扫描器已自校验） |
| PixPin 的滚轮注入是**两条并列路径**（`SendInput` 自动滚动 / `PostMessage` 定点），**不是兜底** | **确证** | `PixSystemUtils.dll` 两个导出的完整反汇编（§1.2 ⑦） |
| PixPin 在 UIPI/高权限窗口下**没有第二条捕获通道** | **确证** | `PixPinAuxiliary.exe` 的捕获相关导入全部未命中（§1.2 ⑦） |
| PixPin 的拼接算法是**自研**（半帧重叠匹配 + 偏移候选掩码 + score 筛选），不是 `phaseCorrelate` | **确证** | `PixStitching\src\DetectDisplacement.cpp` 源路径 + `matchImageFast` / `clearOffsetMaskByScore` 符号 |
| PixPin 的**算法路径零埋点**（`scroll`/`stitch`/`match`/`wheel` 等 0 命中） | **确证** | 30,706 物理行日志统计（§1.4 ③） |
| `docs/19`/`docs/24` 的事实错误与过时描述 | **确证** | §3.1 共 5 条（docs/24 侧）+ §2.5 共 10 条（docs/19 侧，含 Crisp 测试清单、`blake3`、线程预算表等），每条附代码 `文件:行` |
| `docs/24` §S2 背景事实 26 条中 20 条属实 | **确证** | 逐条核实（§3.1） |
| snow_shot 的 ORB 复合置信度、不因单帧失败终止、`Contained`/参照系二态、tile 三分类加权 | **确证** | `refer/snow-apps/snow-crates/crates/snow-stitch-images/src/{estimator,region,stitcher,types}.rs` 逐行（`docs/27`） |
| `PostMessage(WM_MOUSEWHEEL)` 能绕过 UIPI | **确证（官方文档原文）** | Microsoft Learn winapp-cli UI Automation（`docs/27` §6.1）；**但在真实浏览器上的效果仍未实测**（§7.2 第 2 项） |
| Crisp 的 `TestStitch.cpp` **没有**"重复纹理"用例 | **确证** | 17 个用例逐条（`docs/27` §2.7） |

---

## 附录 C：证据附录清单

| 文件 | 内容 | 规模 |
|---|---|---|
| **[docs/26-pixpin-static-analysis.md](26-pixpin-static-analysis.md)** | PixPin 二进制/DLL 静态取证：模块与依赖、导入导出表、字符串与源码路径、tile 预算公式、`PixPinAuxiliary.exe`、滚轮注入反汇编、E1–E15 原始证据附录 | 1,752 行 / 130 KB |
| **[docs/27-scroll-capture-reference-survey.md](27-scroll-capture-reference-survey.md)** | `refer/` 参考项目调研（Crisp / ShareX / snow_shot / PowerToys 等）+ 外部技术路线与竞品对照 + 位移估计方法对比 + 未解疑点 | 776 行 / 67 KB |
| **[docs/28-pixpin-runtime-data-and-logs.md](28-pixpin-runtime-data-and-logs.md)** | PixPin 运行期数据：配置全文与逐字段、`LocalStorage.data` 解码、`HistoryShotRectDatas` 字节级还原、`PinWindowd.sqlite` schema、`Data\*.meta` 与 `OcrTextJson`、`model\` 模型、崩溃上报、日志关键词与错误分类、20 个原始证据附录 A–T | ~130 KB |

> 本文（`docs/25`）是**结论与裁决**；三份附录是**证据**。引用本文任何一条结论时，请指到附录里的对应证据位置。
