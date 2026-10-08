# PixPin v3.5.5.1 二进制与依赖深度静态调研（供 SnapClip 对标）

- **分析对象**：`C:\A_Softwares\PixPin`（只读参考，未修改/删除/重命名任何文件，未执行 `PixPin.exe`）
- **安装版本**：`ProductVersion = 3.5.5.1`（PE VERSION 资源；`FileVersion` 字段缺失）
- **构建时间戳**：`2026-09-07T10:32:20Z`（`PixPin.exe` COFF TimeDateStamp）
- **分析产物**：全部写入 `%TEMP%\pixpin_re\`
- **方法**：纯静态。PE 头/导出表/导入表用自写 Python 解析器（`%TEMP%\pixpin_re\pe.py`）+ `llvm-readobj`/`objdump`/`strings` 交叉验证；字符串用 `strings -a -n 5` 与 `strings -a -el -n 5`；常量用原始字节扫描 + 反汇编确认。

> **证据强度标注约定**：**确证** = 直接读取到文件/符号/导入/指令；**强推断** = 多个独立确证证据指向同一结论；**弱推断** = 单一间接证据。

---

## 1. 结论摘要

| # | 结论 | 强度 |
|---|---|---|
| 1 | 截图核心 `PixWin32CaptureCore.dll` **100% 使用 Windows.Graphics.Capture (WGC)**，且**完全不导入 user32/gdi32/dwmapi** —— 不存在 BitBlt/PrintWindow 回退。 | **确证** |
| 2 | WGC 实现基于 **C++/WinRT 2.0.240405.15** + **WIL** + **robmikh.common 0.0.23-beta**（`capture.desktop.interop.h` 提供 `IGraphicsCaptureItemInterop` 封装），源码来自微软官方 WGC 示例工程 **`Win32CaptureSample`**（单文件 `PixWin32CaptureCore.cpp`）。 | **确证** |
| 3 | 帧路径为 **D3D11 纹理 → shader（`POSITION`/`TEXCOORD`，`D3DCompile`）→ staging texture → `Map` → `memcpy` 到 `ImageBuffer`**；`PixWinCapture.dll` 编译期嵌入 HLSL 做颜色/几何变换。 | **确证** |
| 4 | **BitBlt/PrintWindow 仅存在于 `PixPin.exe`**，用途是"贴图窗口 ROI 实时刷新"（字符串 `PinWindowRoiMap::captureRoiImage - PrintWindow failed`），**不是主截图路径**。 | **确证** |
| 5 | **滚动/长截图确实存在且是自研引擎**：模块 `PixStitching`（`src/DetectDisplacement.cpp`）+ `PixLongImage`（分块拼接）+ `PixLongImageFileEncodeThread`（自研 PNG/JPEG 流式编码，内嵌 **libpng 1.6.39**）。 | **确证** |
| 6 | 位移检测用 **OpenCV**（`matchImageFast`，断言 `CV_8UC1`/`cmpMask`）+ **offset mask + score 候选筛选**（`clearOffsetMaskByScore`）。OpenCV **4.13.0 静态链接**（`xmake` 构建），磁盘上**无任何 `opencv_*.dll`**。 | **确证** |
| 7 | **超长模式阈值 = 29000 px**：`cmp eax, 0x7148` / `jg`（5 处），与文案 "over 29000 pixels" 完全对应。 | **确证** |
| 8 | **JPEG 边长上限 = 65500**：`mov ecx,0xffdc` + `cmp` + 失败置错误码 `0x2a`，与文案 "JPG supports up to 65000 pixels" 对应。 | **确证** |
| 9 | "最大拼接范围"上限的**具体常量未能确证**。父代理提出的 `INT_MAX/4 = 536870911 px` 假说**无法用字节扫描证实**（`0x7fffffff` 在 `PixPin.exe` 中出现 1515 次，`0x20000000` 出现 6030 次，噪声完全淹没信号）。**未命中，不强断。** | **未解** |
| 10 | 全局输入钩子宿主是 **`PixKeyMouse.dll`**（导入 `SetWindowsHookExA`/`UnhookWindowsHookEx`/`SendInput`/`GetAsyncKeyState`），配合 `PixWindowNotify.dll` 判定目标进程 —— 这解释了"用户在目标窗口滚轮时被感知"的机制。 | **确证** |
| 11 | 全局热键由第三方 **libqxt (`qxtglobalshortcut.dll`)** 实现，底层 `RegisterHotKey`。 | **确证** |
| 12 | **UA 自动化**：`UiRegionDetector.dll` / `UiSpy.dll` 使用 **IUIAutomation** + **MSAA (`OLEACC.dll` → `AccessibleObjectFromWindow`)** 双通道做窗口区域探测；`PixSystemUtils.dll` 用 UIA/MSAA 抓取文件管理器选中项。 | **确证** |
| 13 | **网络能力**：`PixPin.exe` 硬编码 **百度翻译 API**（`api.fanyi.baidu.com`）；自建服务 `api.pixpin.cn`；崩溃上报为 **Crashpad + Sentry**（DSN `…@bugreport.pixpin.cn/1`）。**无 imgur / sm.ms 图床。** | **确证** |
| 14 | **OCR/公式**：ONNX Runtime **1.23.2** 驱动 `PixModelRunner`；公式识别 = 自研 Caffe 模型 + **libxslt**（`MML2OMML.XSL`，可直出 Word OMML）+ LaTeX 渲染引擎。 | **强推断** |
| 15 | 下载的模型以**私有加密容器**存储（`model\*.bin`，魔数 `7F 79 59 26` / `7F 74 71 …`，熵 ≈7.5 bit/byte）。**格式未解。** | **未解** |

---

## 2. 模块与依赖清单表

### 2.1 主程序 PE 头

命令：`python pe.py hdr PixPin.exe PixPinAuxiliary.exe PixPinContextMenu\PixPinContextMenuExt.dll`

| 项目 | `PixPin.exe` | `PixPinAuxiliary.exe` | `PixPinContextMenuExt.dll` |
|---|---|---|---|
| 大小 | 23,662,904 B (22.57 MB) | 749,880 B | 64,312 B |
| Machine | `0x8664` AMD64 | `0x8664` AMD64 | `0x8664` AMD64 |
| Characteristics | `0x0022` EXECUTABLE_IMAGE + LARGE_ADDRESS_AWARE | 同左 | `0x2022` + DLL |
| Optional Magic | PE32+ (`0x20b`) | PE32+ | PE32+ |
| **LinkerVersion** | **14.44** (VS 2022 17.10+) | 14.44 | 14.44 |
| **Subsystem** | 2 WINDOWS_GUI | **3 WINDOWS_CUI（控制台！）** | 2 WINDOWS_GUI |
| DllCharacteristics | `0x8160` HIGH_ENTROPY_VA, DYNAMIC_BASE, NX_COMPAT, TERMINAL_SERVER_AWARE | `0x8160` | `0x0160` |
| **TimeDateStamp** | `0x6a9e92b4` = **2026-09-07T10:32:20Z** | `0x6a9e9162` = 2026-09-07T10:26:42Z | `0x6a9e92a6` = 2026-09-07T10:32:06Z |
| ImageBase | `0x140000000` | `0x140000000` | **`0x180000000`** |
| SizeOfImage | `0x173a000` | `0xb9000` | `0x10000` |
| Sections | `.text .rdata .data .pdata` **`.qtmetad`** `.rsrc .reloc` | `.text .rdata .data .pdata` **`.fptable`** `.reloc` | `.text .rdata .data .pdata .reloc` |
| 资源 | GRPICON×1, ICON×12, MANIFEST×1, VERSION×1 | **无资源目录** | **无资源目录** |
| Export 表 | 存在（rva `0x14fc620`, 140 B） | 无 | `DllCanUnloadNow`, `DllGetActivationFactory`, `DllGetClassObject` |
| 数字签名 | Sectigo Public Code Signing CA R36 → **Shenzhen Shendu Tujing Technology Co., Ltd.**，DigiCert 时间戳 | — | — |

**`.qtmetad` 段**是 Qt5 的 meta-object 专用段，**确证 Qt 构建链**。

**PDB 路径（`Debug` 目录，CodeView RSDS）**：

| 模块 | PDB 路径 | GUID/Age |
|---|---|---|
| `PixPin.exe` | `D:\Code\Private\PixPinProject\build\windows\x64\release\PixPin.pdb` | `ded140d5225e164dadc811d10944d5d8` / 1 |
| `PixWin32CaptureCore.dll` | `D:\Code\Private\PixPinProject\PixWinCapture\x64\Release\PixWin32CaptureCore.pdb` | `e98f1f319e121147bcb24c087eda8765` / 1 |
| `PixWinCapture.dll` | `D:\Code\Private\PixPinProject\build\windows\x64\release\PixWinCapture.pdb` | `8892f6ec46e14d4cafbb7458e0999b24` / 1 |
| `PixScreenManager.dll` | `…\release\PixScreenManager.pdb` | `1e163e277c3a1942bf982482f8f81ff9` / 1 |
| `UiRegionDetector.dll` | `…\release\UiRegionDetector.pdb` | `077a58a86278e44ba2a34ea398d52f4d` / 1 |
| `UiSpy.dll` | `…\release\UiSpy.pdb` | `7e82f72bf3e2f445bba5143745c148eb` / 1 |
| `PixVision.dll` | `…\release\PixVision.pdb` | `c30d1a57b6848a4b94ffca36821fda58` / 1 |

> **架构含义**：`PixWinCapture\Win32CaptureSample\x64\Release\` 是一个**独立 MSBuild/VS 工程**（含 `Generated Files\winrt\*.h`），其余模块走**另一套构建树**（`build\windows\x64\release\`，qmake/xmake 风格）。即 **WGC 核心是一个被单独隔离编译的原生子工程**，与 Qt 世界解耦 —— 这正是 SnapClip 应该采用的边界。

**内嵌 Manifest**（`PixPin.exe`，696 B，UTF-8 BOM）：
```xml
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges>
    <requestedExecutionLevel level="asInvoker" uiAccess="false"></requestedExecutionLevel>
  </requestedPrivileges></security></trustInfo>
  <asmv3:application><asmv3:windowsSettings>
    <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
    <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
  </asmv3:windowsSettings></asmv3:application>
</assembly>
```
→ **确证**：`asInvoker`（不请求管理员）、`uiAccess="false"`、**PerMonitorV2 DPI 感知**。

**`PixWin32CaptureCore.dll` Manifest**（784 B）：`asInvoker` + Common-Controls 6.0 + `supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"`（Win10），**无 DPI 段**（不建窗口，不需要）。

### 2.2 `Pix*.dll` 清单（31 个，全部含导出表）

全部 31 个 `Pix*.dll` **均导出 C++ 修饰名**（MSVC 名字修饰，`?name@@…`），即它们是 **DLL 化的 C++ 类**，不是 C ABI 插件。数量为 `NumberOfNames`。

| DLL | 大小 | 导出数 | 关键导出（节选，逐字） |
|---|---|---|---|
| `PixWin32CaptureCore.dll` | 141,112 | 17 | `?prepareCapture@PixWin32CaptureCoreStatic@@QEAA_NXZ`、`?captureToBuffer@…`、`?createEmptyBuffer@…`、`?getExpectedImageSize@…`、`?hasCaptureChanged@…`、`?updateRect@…`、`?releaseBuffer@…`、`?releaseCapture@…`（类 `PixWin32CaptureCore` / `PixWin32CaptureCoreStatic`，结构体 `ImageBuffer`/`Params`/`Rect`） |
| `PixWinCapture.dll` | 76,088 | 31 | **`?enumDxgiInfo@PixScreenInfoManager@@AEAA_NXZ`**、**`?enumDisplayConfig@PixScreenInfoManager@@AEAA_NXZ`**、`?captureToQImage@…`、`?captureToBuffer@…`、`?prepareCapture@…`、`?onPrepareTimeout@…`、`?restartPrepareTimer@…`、`?updateRegion@…`、`?refreshScreenInfo@…`、`?getScreensDesc@…` |
| `PixScreenManager.dll` | 88,888 | 56 | 类 `PixScreen`、`PixScreenManager`、**`PixScreenStreamer`**（`?captureTo@`、`?hasFrameChanged@`、`?updateArea@`）、`PixCursorPixmap`（`?GetCursor@`、`?hotSpot@`、`?isInvertingCursor@`、`?drawTo@`）、`?grabScreen@`、`?screenAt@`、`?wholeRect@` |
| `PixVision.dll` | 5,758,776 | 33 | `?DecodeQRCode@@YA?AUQRCodeResult@@AEAVQImage@@@Z`、`?CodeTypeName@`、`?IsIndustrialCode@`、`?BlurImage@@YAXAEAVQImage@@H@Z`、`?ResizeImage@`、`?SplitImage@`、`?ImageColorTransform@`、`ColorMatrix`（`?grayscale@`/`?brightness@`/`?contrast@`/`?saturation@`/`?sepia@`/`?invert@`/`?hue@`/`?rgbScale@`/`?colorOverlay@`/`?addictiveColor@`） |
| `PixWindowNotify.dll` | 39,224 | 21 | `?instance@PixWindowNotify@@SAPEAV1@XZ`、`?activeProcessChanged@`、`?activeProcessId@`、`?activeProcessName@`、`?activeProcessPath@`、`?activeWindowChanged@`、`?activeWindowId@`、`?refreshActiveWindow@`、**`?setEnhanceUIDetection@`**、`?setEnhancePinTopmost@`、`?sigWindowStateChanged@` |
| `UiRegionDetector.dll` | 90,936 | 40 | `?DirectGetRect@UiRegionDetector@@SA?AVQRect@@VQPoint@@_N@Z`、`?GetWinRectByPoint@`、`?GetChildWinRect@`、`?GetParantWinRect@`、`?GetWinProcessName@`、`?GetWinTitle@`、**`?WinGetWinRectByPointUIA@…PEAUWinRect@@HH@Z`**、`?SnapshotAllWinRect@`、`?checkWinRectAlive@`、`?setIsOnlyWindow@`、`UiRegionDetectorThreadWarp`（`?run@`、`?sigNewRectDetected@`） |
| `UiSpy.dll` | 84,792 | 40 | 与 `UiRegionDetector` **几乎逐一对应**（旧版/简化版；`sigNewRectDetected(QRect)` vs `UiRegionDetector` 的 `sigNewRectDetected(QRect,QPoint,qulonglong,qint64,QRect,QRect)`） |
| `PixMovie.dll` | 574,776 | 386 | `?recordingError@PixMultiTrackRecorder@@QEAAXAEBVQString@@@Z`（多轨录制器） |
| `PixAVCodec.dll` | 8,008,504 | 105 | `?error@pixGifEncoder@@QEBAAEBVQString@@XZ` |
| `PixOCR.dll` | 4,755,256 | 101 | `PixOcr::PixOcrError`（`?detectionFailed@`、`?recognitionFailed@`、`?layoutFailed@`、`?networkError@`、`?timeoutError@`）、`?getError@PixOcrObject@PixOcr@@` |
| `PixOCR2.dll` | 4,999,480 | 219 | 同上 + 额外 `onError` 重载（v2，多一个 `_N` bool 构造参数 → 新旧两代 OCR 并存） |
| `PixModelRunner.dll` | 94,008 | **6** | `?getInstance@PixModelRunner@@SAPEAV1@XZ`、**`?inference@PixModelRunner@@QEAA?AUResult@1@AEBUParams@1@@Z`**、**`?setSessionCacheCount@PixModelRunner@@SAXH@Z`**（→ ONNX Runtime `InferenceSession` 缓存池） |
| `PixFormulaRec.dll` | 3,126,584 | 161 | `?onRecognizeError@PixFormulaRecDialog@`、`PixFormulaRecInterface` |
| `PixLatex2MathML.dll` | 1,258,808 | 256 | `Latex2MathML`、`MathML2OMML`；**导出 libxslt 符号**（`xsltGenericError`、`xsltSetTransformErrorFunc`…）→ libxslt 静态链接 |
| `PixKeyMouse.dll` | 74,552 | 64 | `?OnMouseWheelEvent@PixKeyMouseHook@@UEAA_NUPixWheelEvent@@@Z`、`?OnMouseWheelEvent@PixGloabalMouse@@EEAA_NUPixWheelEvent@@@Z`（注意原始拼写 `Gloabal`）、`PixWheelEvent` |
| `PixWidget.dll` / `PixWidget2.dll` | 991,032 / 399,160 | **1126 / 797** | `AttachScrollWhenHover`、`ScrollBar@PixWidget2`、`ComboBoxWheelSwitcher`、`PixComboBox` |
| `PixActionsBar.dll` | 107,320 | 109 | `ActionsBar`、`MoveControl`、`ActionsGapLine`、`?addButton@`、`?setVisible@`、`?sigWheelEvent@` |
| `PixStyle.dll` | 224,056 | 110 | — |
| `PixSystemUtils.dll` | 150,328 | 35 | **`?SimulateMouseScroll@@YAX_N0H@Z`**、**`?SimulateMouseWheel@@YAXVQPoint@@H@Z`**、`?SetWindowNotRecord@@YAXPEAVQWidget@@_N@Z`、`?GetWindowIsNotRecord@@YA_N_K@Z`、`?HasHDRMonitor@@YA_NXZ`、`?IsInGameMode@@YA_NXZ`、`?IsWindowsTopmost@`、`?GetScreenPixelColor@`、`?getExplorerSelectedFiles@`、`?RegisterUrlScheme@` |
| `PixUtils.dll` | 2,535,224 | 201 | `?QFPngSave@`、`?QFPngLoadPixmap@`、`?sendErrorLogToBugReport@`、`sendExceptionToBugReport` |
| `PixConfiguration.dll` | 339,768 | 251 | — |
| `PixAuth.dll` | 3,766,072 | 282 | `PixAuthException`、`PixSecureStorageStrategy2`（`mapSystemStoreError`、`SecureStorageError`）、`PixFeatureManager`、`VipFeatureDialog`、`Subscription` |
| `PixNetwork.dll` | 105,272 | 69 | `?imageToBase64@PixNetwork@@SA?AVQString@@AEBVQImage@@@Z`、`?imageToBase64Url@`、`?isRetryableNetworkError@`、`?scheduleTransientRetry@`、`?sslErrors@` |
| `PixDownload.dll` | 81,720 | 72 | `?getError@PixDownloadItem@@` |
| `PixNotification.dll` | 144,696 | 126 | — |
| `PixLottie.dll` | 372,024 | 34 | `PixLottie`、`PixLottieWidget`（`?loadLottieFile@`、`?setFrameRate@`、`?totalFrame@`、`?paint@`） |
| `PixColorPalette.dll` | 92,472 | 31 | `PixColorPalette`（`?addColor@`、`?setWaitingColor@`、`?mainThemeColor@`） |
| `PixStat.dll` | 44,344 | 12 | `PixTrack`（`?trackEvent@`、`?postTrackData@`、`?getProfileID@`）→ 遥测 |
| `PixProgramManage.dll` | 61,240 | 17 | `?setAutoRun@`、`?setRunAsAdmin@`、`?restartToAdmin@`、`?removeTaskIfExist@`、`?initPreheatProgram@` |
| `PixWebCallback.dll` | 92,928 | 17 | `PixWebCallbackDialog`（`?onUrlSchemeCall@`、`?getCallbackUrl@`、`?supportWebView@`）→ OAuth 回跳 |
| `PixPinIcon.dll` | 507,192 | 60 | 图标字体 |
| `PixPinTutorial.dll` | 4,269,368 | 165 | 教程页（含 VIP 页） |
| `PixWindowNotify.dll` | 39,224 | 21 | 见上 |

**非 `Pix*` 但关键**：`UiRegionDetector.dll`(40 导出)、`UiSpy.dll`(40)、`qxtglobalshortcut.dll`、`QCrashpad.dll`、`PandaConfiguration.dll`、`RobinLog.dll`、`SalmonActions.dll`、`QAppStat.dll`、`QIconfont.dll`。

### 2.3 第三方技术栈

#### Qt —— **5.15.13**（确证）

命令（原生）：`(Get-Item Qt5Core.dll).VersionInfo`
```
Qt5Core.dll   FileVer=5.15.13.0  ProdVer=5.15.13.0
              Company='The Qt Company Ltd.'  Prod='Qt5'
              Descr='C++ Application Development Framework'
```
PE VERSION 资源交叉验证（`parse_ver`）：`FileVersion = 5.15.13.0`、`LegalCopyright = Copyright (C) 2022 The Qt Company Ltd.`

**Qt 模块清单**（安装目录内）：`Qt5Core`、`Qt5Gui`、`Qt5Widgets`、`Qt5Network`、`Qt5Qml`、`Qt5Sql`、`Qt5Svg`、`Qt5Xml`、`Qt5PrintSupport`、`Qt5OpenGL`、`Qt5Multimedia`、`Qt5MultimediaWidgets`、`Qt5WinExtras`。

> `Qt5Qml` 存在但**未被 `PixPin.exe` 导入**（导入表无 `Qt5Qml.dll`）→ 弱推断：QML 仅被某插件/可选路径使用。

**平台插件**：`plugins\platforms\qwindows.dll`（1,293,112 B，确证为 Qt 5.15.13）。

**imageformats 插件（直接决定导出能力）**：

| 插件 | 大小 | 格式 |
|---|---|---|
| `qwebp.dll` | 418,104 | **WebP** |
| `qjpeg.dll` | 402,232 | **JPEG** |
| `qtiff.dll` | 375,096 | **TIFF** |
| `qgif.dll` | 42,808 | **GIF** |
| `qico.dll` | 40,760 | **ICO** |
| `qsvg.dll` | 36,664 | **SVG** |
| `qtga.dll` | 35,640 | **TGA** |
| `qwbmp.dll` | 34,104 | **WBMP** |
| `qicns.dll` | 46,904 | **ICNS** |

其余插件：`audio\qtaudio_wasapi.dll`、`qtaudio_windows.dll`；`bearer\qgenericbearer.dll`（+ 根目录 `qgenericbearer.dll`）；`mediaservice\dsengine.dll`、`qtmedia_audioengine.dll`、`wmfengine.dll`；`printsupport\windowsprintersupport.dll`；`sqldrivers\qsqlite.dll`；`styles\qwindowsvistastyle.dll`。

> **导出格式结论**：**PNG / BMP / PPM / XBM / XPM 由 `Qt5Gui` 内建**（无 `qpng.dll` 是正常的）；插件额外提供 **JPEG、WebP、TIFF、GIF、ICO、SVG、TGA、WBMP、ICNS**。
> **重要例外**：超长截图路径**绕过 Qt**，由 `PixLongImageFileEncodeThread` 直接调用 **libpng 1.6.39 / libjpeg** 做流式分块编码（见 §4）。

#### 计算/媒体/加密

| 依赖 | 版本/证据 | 强度 |
|---|---|---|
| **ONNX Runtime** | `onnxruntime.dll` 14,448,952 B，**1.23.2**（`(Get-Item).VersionInfo`：`FileVer=1.23.2`，`Company=Microsoft Corporation`，`FileDescription=ONNX Runtime`）；另有 `onnxruntime_providers_shared.dll` | 确证 |
| **OpenCV 4.13.0** | 静态链接。二进制内源码路径 `C:\Users\PixPin\AppData\Local\.xmake\cache\packages\2604\o\opencv\4.13.0\source\modules\…`，构建工具 **xmake**（`.xmake\cache\packages`、构建用户名 `PixPin`）。磁盘上**无 `opencv_*.dll`**。OpenCV 代码出现在 `PixPin.exe`、`PixOCR.dll`、`PixOCR2.dll`、`PixVision.dll` | 确证 |
| **FFmpeg** | `PixAVCodec.dll` 8,008,504 B 内含 `ffmpeg-devel@ffmpeg.org`、`streams.videolan.org`、`If you want to help, upload a sample of this file to …`；**无 `avcodec-*.dll` 等外部库** → 静态链接 | 确证 |
| **Intel Media SDK / oneVPL + OpenH264** | `PixAVCodec.dll` 含 `.??AVMFXDefaultPlugins@MFX@@`、`MFXPluginFactory`、`mfxplugin64_hw.dll`、`mfxplugin64_sw.dll`、`.?AVCScrollDetection@WelsVP@@`（OpenH264 的 `WelsVP`）、`D3D11CreateDevice returned error, try next adapter` | 确证 |
| **libpng 1.6.39** | `PixPin.exe` 内字符串 `1.6.39` 紧邻 `[PixLongImageFileEncodeThread::encodePng] Failed to create png_struct.` | 确证 |
| **libjpeg** | `[PixLongImageFileEncodeThread::encodeJpeg] libjpeg reported an encoding failure:` | 确证 |
| **libxslt** | `PixLatex2MathML.dll` 导出 `xsltGenericError`/`xsltSetTransformErrorFunc` 等 | 确证 |
| **OpenSSL 1.1** | `libcrypto-1_1-x64.dll`（3,424,568 B）+ `libssl-1_1-x64.dll`（697,656 B）；`PixAuth.dll` 含 `AESNI-CBC+SHA1 stitch for x86_64, CRYPTOGAMS by <appro@openssl.org>`、`engines\e_capi.c`（Windows 证书存储引擎） | 确证 |
| **Crashpad + Sentry** | `Helpers\crashpad_handler.exe`(700,728)、`crashpad\crashpad_handler.exe`(555,264)、`QCrashpad.dll`；`crashpad\…run\__sentry-event`、`__sentry-breadcrumb1/2`、`session.json`；`PixUtils.dll` 字符串 `https://d3e76aa4570f16ecbf8d8973d5801b09@bugreport.pixpin.cn/1` | 确证 |
| **Windows Media Foundation** | `PixPin.exe` 与 `PixMovie.dll` 均导入 `MF.dll`、`MFPlat.DLL`、`MFReadWrite.dll` | 确证 |
| **libqxt** | `qxtglobalshortcut.dll`（62,264 B），导入 `RegisterHotKey` | 确证 |
| **D3DCompiler_47** | 根目录 4,751,792 B；`PixWinCapture.dll` 导入 `D3DCompile` | 确证 |
| **Lottie** | `PixLottie.dll`（自研 Lottie 播放器，导出 `?paint@PixLottie@@QEAAXPEAVQPainter@@…`） | 确证 |
| **TeX 排版** | `PixFormulaRec.dll` 含 `.?AVMacro@tex@@`、`.?AVScriptsAtom@tex@@`、`.?AVCumulativeScriptsAtom@tex@@`、`.?AVInflationMacroInfo@tex@@` | 强推断（很可能是 **MicroTeX** 一类 C++ TeX 引擎，未在磁盘找到该库文件，故不强断具体项目名） |

#### 模型与数据

| 文件 | 大小 | 识别 |
|---|---|---|
| `model\detect.caffemodel` | 965,430 | **Caffe** 模型（prototxt 首行 `layer {\n  name: "data"\n  type: "Input"`；权重含 `BatchNorm`、`data/bn/scale`）→ 文本检测 |
| `model\detect.prototxt` | 45,372 | Caffe 网络定义 |
| `model\sr.caffemodel` | 23,929 | Caffe 超分模型（含 `Convolution`、`data_data_0_split`） |
| `model\sr.prototxt` | 6,387 | Caffe 超分网络定义 |
| `model\paragraph_recognition.onnx` | 3,198,623 | **ONNX**，producer `pytorch 2.11.0+cpu` → 段落/文本识别 |
| `model\2478d381…bin` 等 **5 个** | 9.88 / 16.62 / 4.73 / 21.23 / 10.84 MB | **私有加密容器**（见 §8） |
| `OcrModel\` | 空目录 | — |
| `Data\PinWindowd.sqlite` | 106,496 | **SQLite format 3**（确证 magic `SQLite format 3\x00`）→ 贴图窗口持久状态 |
| `Data\*.meta` / `*.png` | 100 / 99 个 | 截图历史（`YYYY-MM-DD_HH-MM-SS-0.*`） |
| `History\_ScreenshotRecord\*.his` | 60+ 个，单个 0.6–10.7 MB | 截图历史（自定义 `.his` 格式） |
| `language\*.qm` | de/es/fr/ja/ko/pt/zh-cht/zh-cn | Qt 翻译；**同时保存英文源串与译文（UTF-16BE）** |

---

## 3. 截图技术栈判定 —— **WGC，决定性证据**

### 3.1 判定：**Windows.Graphics.Capture (WGC) 为唯一主路径；无 BitBlt；DXGI 仅用于枚举**

### 3.2 决定性证据 A —— 导入表（`llvm-readobj --coff-imports`）

**`PixWin32CaptureCore.dll` 的完整导入 DLL 列表**（**没有 `USER32.dll`，没有 `GDI32.dll`，没有 `dwmapi.dll`**）：
```
api-ms-win-core-libraryloader-l1-2-0.dll   api-ms-win-core-synch-l1-1-0.dll
api-ms-win-core-heap-l1-1-0.dll            api-ms-win-core-errorhandling-l1-1-0.dll
api-ms-win-core-winrt-error-l1-1-0.dll     api-ms-win-core-processthreads-l1-1-0.dll
api-ms-win-core-localization-l1-2-0.dll    api-ms-win-core-debug-l1-1-0.dll
api-ms-win-core-handle-l1-1-0.dll          api-ms-win-core-string-l1-1-0.dll
api-ms-win-core-synch-l1-2-0.dll           api-ms-win-core-sysinfo-l1-1-0.dll
MSVCP140.dll  VCRUNTIME140.dll  VCRUNTIME140_1.dll
api-ms-win-crt-{heap,string,stdio,runtime}-l1-1-0.dll
api-ms-win-core-memory-l1-1-0.dll          api-ms-win-core-rtlsupport-l1-1-0.dll
api-ms-win-core-processthreads-l1-1-1.dll  api-ms-win-core-profile-l1-1-0.dll
api-ms-win-core-interlocked-l1-1-0.dll
OLEAUT32.dll
api-ms-win-core-com-l1-1-0.dll             → CoCreateFreeThreadedMarshaler
api-ms-win-core-winrt-error-l1-1-1.dll     → RoOriginateLanguageException
api-ms-win-core-winrt-l1-1-0.dll           → RoGetActivationFactory
d3d11.dll                                  → D3D11CreateDevice, CreateDirect3D11DeviceFromDXGIDevice
```
关键函数：
- **`CreateDirect3D11DeviceFromDXGIDevice`**（`d3d11.dll`）—— 这是把 `ID3D11Device` 包成 WinRT `IDirect3DDevice` 的**唯一专用 API**，只为 `Direct3D11CaptureFramePool::CreateFreeThreaded` 服务。
- **`RoGetActivationFactory`**（`api-ms-win-core-winrt-l1-1-0.dll`）—— WinRT 类型激活，用于拿 `GraphicsCaptureItem` / `Direct3D11CaptureFramePool` 工厂。
- **`CoCreateFreeThreadedMarshaler`** —— 自由线程封送（对应 `CreateFreeThreaded` 跨线程用法）。

### 3.3 决定性证据 B —— 字符串（`strings -a -el`，UTF-16LE）

`PixWin32CaptureCore.utf16.txt` 全部 22 条中，关键 6 条：
```
Windows.Foundation.Metadata.ApiInformation
Windows.Graphics.Capture.GraphicsCaptureItem
Windows.Graphics.Capture.GraphicsCaptureSession
DirtyRegionMode
Windows.Graphics.Capture.Direct3D11CaptureFramePool
Windows.Foundation.UniversalApiContract
Win32CaptureSample.SampleWindow
```

ASCII 侧（`PixWin32CaptureCore.ascii.txt`）：
```
D:\Code\Private\PixPinProject\PixWinCapture\Win32CaptureSample\x64\Release\Generated Files\winrt\base.h
C++/WinRT version:2.0.240405.15
D:\Code\…\Generated Files\winrt\Windows.Foundation.h
D:\Code\…\Generated Files\winrt\Windows.Foundation.Collections.h
D:\Code\…\Generated Files\winrt\Windows.Foundation.Metadata.h
D:\Code\…\Generated Files\winrt\Windows.Graphics.Capture.h
D:\Code\…\packages\Microsoft.Windows.ImplementationLibrary.1.0.240803.1\include\wil\resource.h
D:\Code\…\packages\robmikh.common.0.0.23-beta\include\robmikh.common\d3d11Helpers.h
D:\Code\…\packages\robmikh.common.0.0.23-beta\include\robmikh.common\direct3d11.interop.h
D:\Code\…\packages\robmikh.common.0.0.23-beta\include\robmikh.common\capture.desktop.interop.h
D:\Code\Private\PixPinProject\PixWinCapture\Win32CaptureSample\PixWin32CaptureCore.cpp
```

**`robmikh.common` 的 `capture.desktop.interop.h` 正是 `IGraphicsCaptureItemInterop` 的封装头**（提供 `CreateForWindow` / `CreateForMonitor` 的辅助函数）。这解释了为什么直接搜 `IGraphicsCaptureItemInterop` / `CreateForWindow` / `CreateForMonitor` 字符串**未命中** —— 它们被封装在头文件里，编译后只留下模板实例化符号。

**WGC 会话选项控制（确证光标与边框行为）**：
```
[PixWinCapture::ApplyCaptureSessionOptions] IGraphicsCaptureSession3 is unavailable; capture border remains enabled
[PixWinCapture::ApplyCaptureSessionOptions] IGraphicsCaptureSession2 is unavailable; cursor capture remains enabled
```
→ `IGraphicsCaptureSession2::IsCursorCaptureEnabled`（Win10 2004+）、`IGraphicsCaptureSession3::IsBorderRequired`（Win11 22H2+，去黄色边框）。**PixPin 主动关闭采集边框**。

**WGC 能力探测与失败路径**：
```
Graphics Capture not supported on this system
PixWin32CaptureCoreStatic: ERROR - Graphics Capture not supported on this system
Failed to create capture item
Invalid capture item size
Window capture mode not implemented in static version
PixWin32CaptureCoreStatic: ERROR - Window capture mode not implemented
```
→ `Windows.Foundation.Metadata.ApiInformation` 用于运行时能力检测；**静态版只做显示器采集，窗口采集走实例版 `PixWin32CaptureCore`**。

### 3.4 决定性证据 C —— 帧搬运管线（确证）

```
createShaderTempTexture     Failed to create shader temp texture:
createStagingTexture        Failed to create staging texture:
POSITION / TEXCOORD          (HLSL 顶点布局)
copyToBuffer
Source texture is not available
D3D context is not initialized
Invalid mapped resource RowPitch
Failed to map staging texture:
Buffer width/height does not match captured width/height
Invalid stride value / Invalid bytes per pixel
captureFrame recreate session / captureFrame close session for resize
```
→ 管线为：**WGC frame → D3D11 源纹理 → 全屏三角形/四边形 + 像素着色器（格式转换 / HDR→SDR / 旋转）→ staging texture → `ID3D11DeviceContext::Map` → 逐行 `memcpy` 到 `ImageBuffer`**。`Params`/`ImageBuffer`/`Rect` 是导出的 POD 结构。

`PixWinCapture.dll` 编译期嵌入 HLSL（确证）：
```
float2 translatedCoord = texCoord - pivot;
rotatedCoord.x = translatedCoord.x * cosTheta - translatedCoord.y * sinTheta;
rotatedCoord.y = translatedCoord.x * sinTheta + rotatedCoord.y * cosTheta;
```

### 3.5 DXGI 与显示配置的角色（确证）

`PixWinCapture.dll` 导入：
```
dxgi.dll   : CreateDXGIFactory1
USER32.dll : GetMonitorInfoW, EnumDisplayMonitors, DisplayConfigGetDeviceInfo,
             GetDisplayConfigBufferSizes, QueryDisplayConfig
D3DCOMPILER_47.dll : D3DCompile
```
`PixScreenInfoManager` 私有导出：`enumDxgiInfo`、`enumDisplayConfig`、`findScreenInfoIndexByGdiDeviceName`、`findScreenInfoIndexByHMONITOR`、`genScreenDesc`、`getScreensDesc`。
→ **DXGI 仅用于枚举适配器/输出与建立 D3D11 设备**；`QueryDisplayConfig` 用于**每显示器刷新率/旋转/HDR 状态**。**没有 `IDXGIOutputDuplication` 字符串或导入（未命中）→ 不使用桌面复制 API。**

### 3.6 BitBlt / PrintWindow 的真实位置（确证）

| 位置 | 证据 | 用途 |
|---|---|---|
| `PixPin.exe` 导入 `USER32.dll :: PrintWindow` | 导入表 | **贴图窗口 ROI 实时刷新** |
| `PixPin.exe` 字符串 `PinWindowRoiMap::captureRoiImage - PrintWindow failed` | 字符串 | 同上 |
| `PixPin.exe` 字符串 `BitBlt`、导入 `GDI32.dll :: GetDIBits/SelectObject/DeleteDC/DeleteObject` | 字符串+导入 | GDI DIB 提取 |
| `PixScreenManager.dll` 导入 `GDI32.dll :: CreateCompatibleDC/SelectObject/CreateDIBSection/GetDIBits/GetObjectA/DeleteDC/DeleteObject` 与 `USER32.dll :: GetCursorInfo/GetIconInfo/DrawIconEx/LoadCursorA/GetDC/ReleaseDC` | 导入表 | **光标位图抓取与合成**（`PixCursorPixmap::GetCursor`），非屏幕采集 |
| `PixScreenManager.dll` 导入 `Qt5Gui :: QScreen::grabWindow` | 导入表 | Qt 回退/兼容采集路径 |

> **判定**：主截图 = WGC。`PrintWindow`/`BitBlt` 只服务于**贴图窗口刷新**与**光标抓取**这两个辅助场景。这与 `docs/19` 中"窗口级 WGC 为主"的对标基准**一致**，且 PixPin 的 WGC 内核**完全不含 GDI 依赖**，是干净的模块边界。

### 3.7 显示排除（SetWindowDisplayAffinity，确证）

`PixSystemUtils.dll` 导入 `USER32.dll :: SetWindowDisplayAffinity`、`GetWindowDisplayAffinity`；字符串：
```
[SetWindowNotRecord] SetWindowDisplayAffinity FAILED for widget:
SetWindowDisplayAffinity
```
导出：`?SetWindowNotRecord@`、`?GetWindowIsNotRecord@`。→ 用 `WDA_EXCLUDEFROMCAPTURE` 让贴图/工具栏窗口不出现在自己的截图里。
> **注**：字符串 `WDA_EXCLUDEFROMCAPTURE` / `WDA_MONITOR` **未命中**（宏在编译期折叠为立即数，属正常）。

### 3.8 高 DPI / HDR / 多屏（确证）

- Manifest：`PerMonitorV2, PerMonitor`。
- `PixSystemUtils.dll` 导出 **`?HasHDRMonitor@@YA_NXZ`** → 检测 HDR 显示器（配合 shader 做色调映射）。
- `PixSystemUtils.dll` 导出 `?IsInGameMode@@YA_NXZ` → 全屏游戏检测。
- `PixScreenManager` 提供 `availableGeometry`、`geometry`、`isPrimary`、`adapter`、`screenAt`、`wholeRect`、`widgetScreen`。
- 字符串 `EnumDisplayMonitors` / `GetDpiForMonitor` / `SetProcessDpiAwarenessContext`：前者在 `PixWinCapture` 导入表中**确证**；后两者**未命中**（DPI 由 Manifest 声明，运行时不调 API）。

---

## 4. 长截图实现证据（字符串 + 常量 + 导入表）

> 本节回应父代理的最高优先级请求。**逐条写明命中/未命中。**

### 4.1 用户可见文案（确证，双重来源）

**英文源串**（`PixPin.exe`，`.rdata`，UTF-8，fileoff `0xcf2300`–`0xcf2b00`）：
```
The screenshot is about to enter super long screenshot mode (over 29000 pixels).

Limitations:
1. Only saving is available.
2. PNG supports up to 2 million pixels, JPG supports up to 65000 pixels.
3. Some software may not open or process super long images correctly.

If you do not need a super long screenshot, stop now and finish the current screenshot.
Don't prompt again
Continue
The current screenshot has reached the maximum stitching range and cannot continue stitching.
If the green frame is not visible in the current preview window, it means the current screenshot area has exceeded the maximum stitching size (the current window content is not fully within the stitched image). Please scroll back a short distance and use the screenshot position where the green frame reappears as the reference.
Match Failed
Image matching failed. Possible reasons:
1. The scrolling speed is too fast.
...
```

**中文译文**（`language\zh-cn.qm`，**UTF-16BE**，fileoff `0x1601c` 起）：
```
当前截图已达到最大拼接范围，无法继续拼接。 如果当前预览窗口中的绿框未显示，说明当前截图区域已超出最大拼接尺寸（当前窗口内容未完全落在拼接图内）。请往回滚动一小段距离，并以绿框重新出现时的截图位置为准。
```
> **方法学提示**：Qt `.qm` 把译文存为 **UTF-16BE**。用 `strings -el`（UTF-16**LE**）扫 `.qm` 只能拿到英文源串，中文会全部漏掉。这是本次分析中一个重要的方法论修正。

`PixPin.exe` 中同时存在 **UTF-8 中文字面量**（非 `.qm`），例：`[QImage2MatGray] 错误：图像无效`。

**"不再提示" 配置键**（确证，字符串）：`LongShot.MaxLengthWarningNoAsk`、`LongShot.MatchFailWarningNoAsk`、`LongShot.SuperLongWarningNoAsk`、`LongShot.StopClearConfirmNoAsk`。

### 4.2 阈值常量 —— **29000 确证**

**字节扫描**（`struct.pack('<I', 29000)` = `48 71 00 00`）：

| 模块 | 命中数 |
|---|---|
| **`PixPin.exe`** | **9** |
| `PixAVCodec.dll` | 10（FFmpeg 常量表，非本逻辑） |
| `PixOCR.dll` | 1 |

`PixPin.exe` 的 9 处全部位于 `.text`（fileoff `0x20f7e5`, `0x20f92d`, `0x20fbc9`, `0x20fde1`, `0x21065c`, …），彼此密集聚集。

**反汇编确认**（`objdump -d -M intel`，注意 `VA = fileoff + 0xC00 + 0x140000000`）：

`VA 0x1402103e4`：
```asm
1402103d3:  mov    rcx,QWORD PTR [rsi+0xb8]     ; 取长图对象
1402103da:  test   rcx,rcx
1402103dd:  je     0x1402103f8
1402103df:  call   0x1403f1830                 ; height() / logicalLength()
1402103e4:  cmp    eax,0x7148                 ; <<< 29000
1402103e9:  jle    0x1402103f8                 ; <= 29000 -> 普通路径
1402103eb:  or     ebx,0x20                   ; > 29000 -> 置"超长模式"标志
```
`VA 0x1402107c8` / `0x1402109e0` / `0x14021125b` 为同一模式：
```asm
call  0x1403f1830
test  eax,eax
jle   <普通路径>
...
call  0x1403f1830
cmp   eax,0x7148        ; 29000
jg    <超长模式分支>
```
> **确证**：`29000` 是**逻辑输出高度**（`[obj+0xb8]` 对象的高度）的比较阈值，`> 29000` 进入超长模式。与权威文案 "over 29000 pixels" 完全一致。共 5 处以上同一 `cmp eax,0x7148` 模式，说明该判据在多个入口（截图/保存/复制/OCR）重复把关。

### 4.3 JPEG 边长上限 —— **65500 确证**

**字节扫描**（`0xffdc`）：`PixPin.exe` 命中 **1** 处（fileoff `0xc551e8`）。

**反汇编确认**（`VA 0x140c55de7`）：
```asm
140c55dd8:  mov    rax,QWORD PTR [rcx]
140c55ddb:  mov    DWORD PTR [rax+0x28],0x21     ; 先置错误码 0x21
140c55de2:  mov    rax,QWORD PTR [rcx]
140c55de5:  call   QWORD PTR [rax]
140c55de7:  mov    ecx,0xffdc                  ; <<< 65500
140c55dec:  cmp    DWORD PTR [r14],ecx
140c55def:  jg     0x140c55df5                 ; 高 > 65500 -> 失败
140c55df1:  cmp    DWORD PTR [rsi],ecx
140c55df3:  jle    0x140c55e0d                 ; 宽 <= 65500 -> 通过
140c55df5:  mov    rax,QWORD PTR [rbx]
140c55df8:  mov    DWORD PTR [rax+0x28],0x2a   ; <<< 错误码 42 = "JPEG edge is too large"
140c55dff:  mov    rax,QWORD PTR [rbx]
140c55e02:  mov    DWORD PTR [rax+0x2c],ecx     ; 把 65500 作为 limit 记入错误上下文
140c55e05:  mov    rcx,rbx
140c55e08:  mov    rax,QWORD PTR [rbx]
140c55e0b:  call   QWORD PTR [rax]
```
与字符串 `[PixLongImageFileEncodeThread::encodeJpeg] JPEG edge is too large:` 对应，错误码 `0x2a`。
> **确证**：JPEG 编码宽度/高度上限 = **65500**（文案写 65000，实际取 65500；65500 是 libjpeg 因 16 位 DCT 系数限制的经典上限）。

**`65000` (0xfde8)**：`PixPin.exe` 命中 16 处（多为巧合/其它常量）。**不是**本判据。

### 4.4 "最大拼接范围"上限 —— **未能确证（未命中）**

尝试过的全部手段与结果：

| 手段 | 目标 | 结果 |
|---|---|---|
| 字节扫描 `0x7fffffff` | INT_MAX | `PixPin.exe` **1515 次**，`.text` 12 / `.rdata` 2 / 其余遍布 —— 噪声淹没信号，**不可判定** |
| 字节扫描 `0x1fffffff` (=INT_MAX/4) | 536870911 px | `PixPin.exe` **69 次**；`PixVision.dll` 50；`PixModelRunner.dll` **10/10**（全部命中即说明是常规掩码，非专属常量）→ **不可判定** |
| 字节扫描 `0x20000000` (=512 MP) | 512×1024×1024 | `PixPin.exe` **6030 次** —— 典型的 SIMD/对齐/掩码常量 → **不可判定** |
| 字节扫描 `2000000` (0x1e8480) | PNG 像素上限 | `PixPin.exe` 7 处，但反汇编邻域与长图逻辑无关联 → **未关联** |
| 绝对指针搜索 | `TILE_MAX_HEIGHT` 等错误串的 8 字节绝对指针 | **0 命中** |
| RIP-relative 位移解析扫描 | 同上（自写 disp32 求解器） | **0 命中**（扫描器已用已知引用验证通过） |
| `objdump` 全 `.text`（3,484,967 行）grep `# 0x…` 注释 | 同上 | **0 命中** |

**RIP-relative 扫描器的有效性已自证**：用同一扫描器搜已知引用目标 `0x140cf3f88`（`objdump` 显示 `lea r8,[rip+0xae3a3d]` 位于 `VA 0x140210544`），扫描器**精确复现该地址**（并额外找到 5 处同类引用）：
```
VALIDATION: known ref to 0x140cf3f88
   found at VA=0x140210544 (instr len 7)   <-- 与 objdump 一致
```
因此"这些长图错误串在当前镜像中找不到直接 RIP-relative 引用"是一个**真实的观察**，最可能的解释是 **MSVC 字符串池（`/GF`）的后缀合并 / 内部偏移引用**，或引用点落在 `objdump` 线性扫描失步（输出中确实出现 `(bad)` / `.byte`）的区域。

> **诚实结论**：
> - 父代理的 **`INT_MAX/4 = 536870911 px ≈ 512 MP` 假说在数值上与官方产物 `1058 × 502649 = 531,802,642 px`（占 536,870,911 的 **99.06%**）高度自洽**，且 `1058 × 502649 × 4 B = 2,127,210,568 B` 距 `INT32_MAX = 2,147,483,647` 仅差 ~20 MB —— **这是一个很强的间接推断（强推断）**。
> - 但**我无法用静态字节/反汇编手段把该上限钉到某条具体指令或某个 `.rdata` 常量上**。报告中不作为"确证"。
> - **`TILE_MAX_HEIGHT` / `mMaxHeight` 的数值同样未能解出**（符号存在、值未解）。

### 4.5 拼接算法与分块长图架构（确证，符号级）

**(a) `PixStitching` 模块**（`PixPin.exe` 内，源码路径确证）：
```
PixStitching\src\DetectDisplacement.cpp
matchImageFast
img1.type() == CV_8UC1 && img2.type() == CV_8UC1 && cmpMask.type() == CV_8UC1
img1.size() == img2.size() && img1.size() == cmpMask.size()
[DetectDisplacement::clearOffsetMaskByScore] No valid offset candidates. maskRows:  scoreRows:
[PixStitching::tryAddImage] Failed to build legacy half-frame contact input, fallback to full frame.
```
→ **位移检测 = OpenCV 灰度（`CV_8UC1`）模板/块匹配 + 偏移候选掩码（`cmpMask`）+ 按得分筛选（`clearOffsetMaskByScore`，维护 `maskRows`/`scoreRows`）**。注意 `matchImageFast` 的命名与 "legacy half-frame" 回退逻辑：**它比较的是"半帧"（重叠区）而非整帧**，失败时退回整帧。

**(b) `PixLongImage` 分块容器**（确证，错误串完整暴露 API）：
```
[PixLongImage::contactImage] Invalid TILE_MAX_HEIGHT:
[PixLongImage::contactImage] Gap is not allowed when appending first image, startIndex:
[PixLongImage::contactImage] Gap is not allowed, startIndex:  currentEndIndex:
[PixLongImage::contactImage] Head insertion must reach current start, startIndex:
[PixLongImage::contactImage] Failed to put image when processing head, startIndex:  putSize:
[PixLongImage::contactImage] Head insertion size mismatch, addedHeight:  headInsertSize:
[PixLongImage::contactImage] Append must start at current end, startIndex:
[PixLongImage::contactImage] Failed to put image when appending, startIndex:
[PixLongImage::contactImage] Missing tile coverage at startIndex:
[PixLongImage::removeHead] / [PixLongImage::removeTail]
[PixLongImage::toImage] Failed to create result image, startIndex:  endIndex:
[PixLongImage::toImage] Incompatible tile image at startIndex:
[PixLongImage::toImage] Failed to copy full image range, copiedHeight:  expectedHeight:
[PixLongImage::startEncodeToFile] File path is empty. / Unsupported file suffix:  / Invalid fixed size:  / Encountered null tile image. / No valid image data to encode. / Encode task is already running.
Image channel mismatch, image bytesPerLine:  width:  channel:
[PixLongImageTile::putImage] Invalid input: newImage is null or putSize <= 0 or mMaxHeight <= 0.
[PixLongImageTile::putImage] Invalid input: startIndexInNewImage is out of bounds.
[PixLongImageTile::putInput] Invalid input: putSize exceeds mMaxHeight.
[PixLongImageTile::putImage] Required height exceeds maximum allowed height.
```
> **架构含义（对 SnapClip 极有价值）**：PixPin 把长图建为 **tile 列表**（`PixLongImageTile`，每块有 `mMaxHeight`），支持 **头部插入（往回滚）/ 尾部追加 / 头尾裁剪**。这直接对应"往回滚动自动裁剪"（`Long Screenshot Auto Crop`）和 `removeHead`/`removeTail`。**不是**一张无限 `QImage`。

**(c) 流式编码线程**（确证）：
```
[PixLongImageFileEncodeThread::encode]
[PixLongImageFileEncodeThread::prepareTileImages]  … Failed to rotate tile image for horizontal output.
                                                   Rotated tile height mismatch:  Failed to convert tile image.
[PixLongImageFileEncodeThread::encodePng]  Invalid output size:  Failed to open output file:
                                           Failed to create png_struct.  Failed to create png_info.
                                           libpng reported an encoding failure.  Encoding cancelled.
                                           Missing tile data at row:  Failed to commit output file:
1.6.39
[PixLongImageFileEncodeThread::encodeJpeg] Invalid output size:  JPEG edge is too large:
                                           libjpeg reported an encoding failure:  Encoding cancelled.
                                           Missing tile data at row:  Failed to commit output file:
Missing PNG write context / Failed to write PNG data
Row index is out of range:  Scanline buffer overflow, offset:  copyBytes:
Scanline width mismatch, actualBytes:  expectedBytes:
```
→ **逐行（scanline）从 tile 取数据 → 手写 `QImage`→ 直接喂 libpng/libjpeg**。`prepareTileImages` 支持**旋转 tile 以输出横向长图**。`Missing tile data at row:` 说明编码时按行回查 tile。

**(d) `LongShotWidget` 业务层**（确证）：
```
[LongShotWidget::saveSuperLongImageToFile] jpg is not supported for current logical length:
[LongShotWidget::startSuperLongImageSave] encode failed or was canceled / failed to start encode for / logical image is null
[LongShotWidget::pinToScreen] pin is disabled in super long mode
[LongShotWidget::copyOcrText] export image is null
[LongShotWidget::saveToFile] convert image to pixmap failed
[LongShotWidget::ApplyPostImageProcess] Failed to convert image to pixmap
[LongShotWidget::getClippedCaptureRect] Target screen not found, using original capture rect
[LongShotMaskOverlay::updateMask] Capture rect not in widget, window rect:
[LongShotWidget::activateForShortcut] Skip activation while closing / Skip reentrant activation
```
→ **超长模式下 `jpg` 被显式拒绝**（`jpg is not supported for current logical length`），**贴图（pin）被禁用**，与文案 "Only saving is available" 一致。

**(e) 脚本 API**（确证，字符串）：`longshot.startStop()`、`longshot.toggleAutoScroll()`、`longshot.cropStart()`、`longshot.cropEnd()`、`longshot.edit()`。

**(f) 相关 QSS 资源**：`:/qss/LongShotActionsBar`、`:/qss/LongShotSaveProgressDialog`、`D:\Code\PixPinProject\PixPin\res\qss\LongShotActionsBar.qss`。

### 4.6 自动滚动 / 滚轮钩子 / 输入合成（确证）

| 机制 | 证据 |
|---|---|
| **全局低层钩子宿主** | `PixKeyMouse.dll` 导入 `USER32.dll :: SetWindowsHookExA`、`UnhookWindowsHookEx`、`SendInput`、`GetAsyncKeyState`、`GetKeyState`、`WindowFromPoint`、`GetWindowThreadProcessId`、`GetAncestor`、`SetCursorPos` |
| 钩子类与事件 | `PixKeyMouse.dll` 导出 `?OnMouseWheelEvent@PixKeyMouseHook@@UEAA_NUPixWheelEvent@@@Z`、`?OnMouseWheelEvent@PixGloabalMouse@@EEAA_NUPixWheelEvent@@@Z`、`PixWheelEvent` 结构、`?GetMouseActionButton@@YAHPEAVQWheelEvent@@@Z` |
| 目标窗口判定 | `PixKeyMouse.dll` 导入 `PixWindowNotify.dll`（`?instance@`、`?activeProcessPath@`、`?refreshActiveWindow@`） |
| **合成滚轮** | `PixSystemUtils.dll` 导出 `?SimulateMouseScroll@@YAX_N0H@Z`（bool, bool, int）与 `?SimulateMouseWheel@@YAXVQPoint@@H@Z`（QPoint, int）；**导入 `SendInput`、`PostMessageW`、`SendMessageW`** |
| 超长模式自动滚动开关 | `PixPin.exe` UTF-16 字符串 **`LongShot_AutoScrollEnabled`** |
| 配置/文案键 | `LongScreenshotAutoCrop`（`PixAuth.dll` → VIP 门控）、`LongShot.MatchFailWarningNoAsk` 等 |

**`WH_KEYBOARD_LL` / `WH_MOUSE_LL` 字面量**：**未命中**（宏在编译期折为立即数 `13`/`14`，属正常）。但从 `SetWindowsHookExA` + `PixKeyMouseHook`/`PixGloabalMouse` + `PixWheelEvent` 可**确证**其为全局输入钩子。

**合成方式判定**：`SimulateMouseScroll` + 同时导入 `SendInput` 与 `PostMessageW`/`SendMessageW` → **强推断：优先 `SendInput`（合成真实滚轮事件，兼容现代应用），`PostMessage(WM_MOUSEWHEEL)` 作为兜底**。`MOUSEEVENTF_WHEEL` / `HWHEEL` / `WM_MOUSEWHEEL` 字面量**未命中**（同样为立即数）。

### 4.7 相关功能的 VIP 门控（确证）

`PixAuth.dll` 字符串：`LongScreenshotAutoCrop`、`Auto Stitch`、`Export to LaTeX Format`、`Intelligently recognize mathematical formulas in images and convert to LaTeX format.`、`Powered by AI, extract and translate any text on the screen with one click…`；`PixPinTutorial.dll` 含 `PixTutorialVipPage`。
→ **长截图自动裁剪、公式识别、翻译均为付费（VIP）功能**。

### 4.8 §4 命中/未命中汇总

| 目标 | 结果 |
|---|---|
| `29000` 超长阈值常量 | **命中**（`PixPin.exe` ×9；`cmp eax,0x7148` 反汇编确证） |
| `65500` JPEG 边长上限 | **命中**（`PixPin.exe` ×1；反汇编确证，错误码 `0x2a`） |
| `65000` 文案数值 | 字节命中 16 处但**非该判据**（代码用 65500） |
| `2000000` PNG 上限 | 字节命中 7 处，**未能关联**到长图逻辑 |
| `TILE_MAX_HEIGHT` / `mMaxHeight` 数值 | **未命中**（符号存在，值未解） |
| "最大拼接范围"上限常量 | **未命中**（`0x7fffffff`/`0x1fffffff`/`0x20000000` 全被噪声淹没） |
| `Stitch`/`overlap`/`matchImageFast`/`score`/`offset` | **命中**（`PixStitching\src\DetectDisplacement.cpp`、`clearOffsetMaskByScore`、`toImage` 遮罩、`Missing tile coverage`） |
| `maxHeight`/`MaxHeight` | `MaxHeight` 字节命中（`PixPin.exe` 等 3 文件，8 处），**未定位到具体符号** |
| `phaseCorrelate` / `matchTemplate` / `SAD` / `NCC` / `correlat` | **未命中**（`matchTemplate` 仅出现在 OpenCV 自带字符串池 `PixOCR/PixOCR2/PixVision`，非 PixPin 代码） |
| `scrollOffset` / `offsetY` / `maxStitch` | **未命中** |
| `opencv_world*.dll` / `opencv_core*.dll` 导入 | **未命中** → OpenCV **静态链接**（源码路径确证 4.13.0） |
| `SetWindowsHookEx` / `WH_MOUSE_LL` | `SetWindowsHookExA` **命中**（`PixKeyMouse.dll`）；`WH_MOUSE_LL` 字面量**未命中**（立即数） |
| `SendInput` / `mouse_event` / `PostMessage(WM_MOUSEWHEEL)` | `SendInput` **命中**；`mouse_event` **未命中**；`PostMessageW`/`SendMessageW` **命中**；`WM_MOUSEWHEEL` 字面量**未命中** |
| `png_write_row` / `libpng` | `1.6.39` + `png_struct`/`png_info` + `Missing PNG write context` + `Scanline buffer overflow` **命中** |
| `Windows.Graphics.Capture` / `Direct3D11CaptureFramePool` / `CreateForWindow` | 前两者 **命中**（UTF-16 类型名）；`CreateForWindow`/`CreateForMonitor`/`IGraphicsCaptureItemInterop` **未命中**（被 `robmikh.common capture.desktop.interop.h` 封装，头文件路径命中） |
| `BitBlt` / `PrintWindow` | **命中，但仅在 `PixPin.exe`**，非采集主路径 |

---

## 5. 滚动截图能力判定

### 5.1 结论：**存在，且为自研引擎 + 全局输入钩子驱动**

**判定依据（全部确证）**：
1. **触达路径**：`PixWidget.dll` 字符串 `StartLongShot` / `LongShot` / `CancelLongShot`；`PixPin.exe` `openLongScreenShot`、`longScreenshot`、`hasLongShotWidget`、`afterLongShotInitAction`、`closeLongShot`、`ScreenShotView::longShot`、`BuiltInShortcutManage::addLongShotItem`。
2. **自动滚动**：`LongShot_AutoScrollEnabled`（UTF-16）+ `longshot.toggleAutoScroll()` 脚本 API + `SimulateMouseScroll(bool,bool,int)`。
3. **滚动感知**：`PixKeyMouse.dll` 全局钩子（`SetWindowsHookExA`）+ `PixWheelEvent` + `PixWindowNotify` 目标进程判定 → **即使用户手动滚轮，PixPin 也能感知**（对应文案 "Hook Mouse Wheel" / "鼠标滚轮钩子"、"Mouse Move Hook" / "鼠标移动钩子"）。
4. **拼接**：`PixStitching::tryAddImage` + `DetectDisplacement::matchImageFast`（OpenCV，`CV_8UC1`）+ `clearOffsetMaskByScore`。
5. **分块容器**：`PixLongImage` / `PixLongImageTile`（`putImage` / `contactImage` / `removeHead` / `removeTail` / `toImage`）。
6. **流式编码**：`PixLongImageFileEncodeThread`（libpng 1.6.39 / libjpeg）。
7. **自动裁剪**：`LongScreenshotAutoCrop`（VIP）+ `removeHead`/`removeTail` + `longshot.cropStart()/cropEnd()`。
8. **UI 反馈**：`LongShotMaskOverlay::updateMask`（实现"绿框"重叠提示）、`LongShotSaveProgressDialog`、`LongShotDirCtrl`。

### 5.2 实现路径（强推断）

```
[用户按快捷键 F1 → 选区域 → 点"长截图"]
        │
        ├─ ScreenShotView::longShot()  →  LongShotWidget
        │       ├─ LongShotMaskOverlay  划定采集矩形 + 绘制"绿框"重叠区
        │       └─ 自动滚动：PixSystemUtils::SimulateMouseScroll / SimulateMouseWheel
        │                    （SendInput 合成滚轮，兜底 PostMessage(WM_MOUSEWHEEL)）
        │
        ├─ 每帧采集：PixScreenManager/PixScreenStreamer（底层 PixWinCapture → PixWin32CaptureCore = WGC）
        │
        ├─ PixStitching::tryAddImage
        │       └─ DetectDisplacement::matchImageFast(prevGray, curGray, cmpMask)
        │              → 位移 offset；clearOffsetMaskByScore 过滤候选取最佳
        │              → 失败 → "Match Failed" 警告（LongShot.MatchFailWarningNoAsk）
        │
        ├─ PixLongImage::contactImage(startIndex, putSize, gap)
        │       ├─ 尾部追加(append) / 头部插入(head, 往回滚时) / removeHead/removeTail(自动裁剪)
        │       └─ 各 PixLongImageTile.putImage 受 mMaxHeight 约束
        │
        ├─ 高度判据：logicalHeight > 29000  → 超长模式
        │       ├─ 仅允许保存；pin 禁用；Emit "SuperLongWarning"
        │       ├─ 高度再触顶 → "已达到最大拼接范围"（上限常量未解）
        │       └─ 输出走 PixLongImageFileEncodeThread
        │
        └─ PixLongImageFileEncodeThread::encodePng / encodeJpeg
                ├─ prepareTileImages（必要时旋转 tile 输出横向长图）
                ├─ 逐 scanline 从 tile 取数据 → libpng / libjpeg
                └─ JPEG: 任一边 > 65500 → 直接失败（错误码 0x2a）
```

### 5.3 与 SnapClip `docs/19` 的关系

- PixPin 的滚动截图**不依赖 UIA `ScrollPattern`**（`ScrollPattern`/`TextPattern` 字符串**均未命中**）。它走的是 **合成滚轮 + 图像拼接**，这与 `docs/19` 的"图像拼接为主"路线一致。
- 但 PixPin **额外**用全局钩子感知**用户手动滚轮**，因此能做"往回滚自动裁剪"。SnapClip 若只做"自动滚"，会缺失这个交互能力。
- PixPin 的 **tile 化长图 + 手写 libpng 流式编码**是绕开 Qt/内存上限的关键设计，值得直接借鉴。

---

## 6. 其他高级能力

### 6.1 OCR

| 项 | 证据 | 强度 |
|---|---|---|
| ONNX Runtime 1.23.2 | `onnxruntime.dll` 版本资源；`PixModelRunner` 字符串 `[PixModelRunnerPrivate::createOnnxRuntimeEnv] Creating ONNX Runtime environment.`、`onnxruntime.dll` | 确证 |
| 会话缓存 | `?setSessionCacheCount@PixModelRunner@@SAXH@Z` | 确证 |
| 两代 OCR 并存 | `PixOCR.dll`（101 导出，直连 `onnxruntime.dll`）vs `PixOCR2.dll`（219 导出，经 `PixModelRunner.dll`） | 确证 |
| 段落识别 | `model\paragraph_recognition.onnx`（PyTorch 2.11.0+cpu 导出） | 确证 |
| 文本检测 | `model\detect.caffemodel` + `detect.prototxt`（Caffe） | 确证 |
| 超分 | `model\sr.caffemodel` + `sr.prototxt`（Caffe，`Convolution`） | 确证 |
| 图像预处理 | OpenCV 4.13.0 静态链接（`PixOCR`/`PixOCR2`/`PixVision`/`PixPin.exe`）；灰度转换日志 `[QImage2MatGray] 错误：图像无效` | 确证 |
| 表格识别 | 配置项 `ScreenShot.ActBarFlag.OcrTable`；脚本 `ocr,longScreenshot,gif,translate,tableRecognition,formulaRecognition`；`mergeCell`/`mergeCells` | 确证 |
| OCR 后处理模块 | `PostProcess.Shot.*`（`blur`/`border` 参数在 `Config\PixPinConfig.json`） | 确证 |
| **PaddleOCR** | 字符串 `PaddleOCR`/`paddle`/`dbnet`/`crnn`/`ch_ppocr` **全部未命中** | — |

> **注意**：`PaddleOCR` 相关字符串**完全未命中**。因此**不能**断言使用 PaddleOCR。可确证的是：**自研 Caffe（检测+超分）+ ONNX（段落识别）+ OpenCV 预处理**的组合。

### 6.2 公式识别（LaTeX / MathML / Word OMML）

| 项 | 证据 | 强度 |
|---|---|---|
| 模型 | `PixFormulaRec.dll` 3.1 MB，导出 `PixFormulaRecDialog`、`?onRecognizeError@` | 确证 |
| LaTeX 渲染 | `PixFormulaRec.dll` 含 `tex::Macro`、`tex::ScriptsAtom`、`tex::CumulativeScriptsAtom`、`tex::InflationMacroInfo`；`?PixLatexGraphicsItem@@`、`?latexFormula@PixFormulaRender@`、`?isLatexLike@@YA_NAEBVQString@@@Z` | 强推断（疑为 MicroTeX 类 C++ TeX 引擎） |
| LaTeX ↔ MathML | `PixLatex2MathML.dll`：`?Latex2MathML@@YA?AVQString@@AEBV1@@Z`；Qt 资源 `:/PixLatex2MathML/latex_symbol.txt`、`mathml_normalize.xsl`、**`MML2OMML.XSL`** | 确证 |
| Word 粘贴 | `MML2OMML.XSL` 是 **Microsoft Office 自带**的 MathML→OMML 转换表 → 可直接粘贴进 Word 公式 | 确证 |
| 依赖 | `PixLatex2MathML.dll` 导出 **libxslt** 符号（`xsltGenericError` 等） | 确证 |
| UI/文案 | `IconCopyAsMathML`、`Copy as MathML formula`、`Export to LaTeX Format`、`latex_1x/1.5x/2x`（`PixAuth` 商业文案） | 确证 |
| `pix2tex` | **未命中** | — |

### 6.3 贴图（Pin）

- `PixSystemUtils`：`?SetWindowTopmost@`、`?SetWindowOnAllDesktops@`、`?SetWindowIgnoreMouse@`、`?SetWindowCanBeFocus@`、`?SetWindowShowInTaskbar@`、`?SetWindowDarkTitlebar@`、`?SetWindowNotRecord@`、`?IsWindowsTopmost@`、`?toggleWindowTopmostAtPoint@`、`?ActivateTopmostCandidateWindow@`、`?GetCandidateWindowZOrder@`、`?RestoreCandidateWindowZOrder@`（确证）
- 贴图窗口数据：`Data\PinWindowd.sqlite`（**SQLite format 3**，106,496 B）（确证）
- 贴图 ROI 动态刷新：`PinWindowRoiMap::captureRoiImage` + `PrintWindow`（确证）
- 配置：`Pin.ConfirmationFlags`、`Action.Pin#s.win` → `pixpin.pinFromClipBoard()`
- **推测**：`WS_EX_LAYERED` / `WS_EX_TOOLWINDOW` / `WS_EX_NOACTIVATE` / `UpdateLayeredWindow` 字面量**均未命中**（宏折叠为立即数）；`Layered` 命中但为无关词。**因此贴图窗口的具体样式标志未能从字符串确证**，仅能由上述导出函数推断为 topmost + 跨桌面 + 忽略鼠标 + 不录屏。

### 6.4 录屏 / GIF（`PixMovie.dll` + `PixAVCodec.dll`）

| 项 | 证据 | 强度 |
|---|---|---|
| 录制器 | `PixMovie.dll` 导出 `?recordingError@PixMultiTrackRecorder@@`（**多轨** = 屏幕 + 麦克风）；配置 `PixMovie.record.recordMicrophone` | 确证 |
| 音频采集 | 导入 `MF.dll`/`MFPlat.DLL`/`MFReadWrite.dll` + `Qt5Multimedia` + `plugins\mediaservice\wmfengine.dll`、`dsengine.dll` | 确证 |
| 编码 | `PixAVCodec.dll` = **静态 FFmpeg**（`ffmpeg-devel@ffmpeg.org`、`streams.videolan.org`） | 确证 |
| 硬件编码 | Intel MFX/oneVPL：`MFXDefaultPlugins`、`MFXPluginFactory`、`mfxplugin64_hw.dll`/`mfxplugin64_sw.dll`；`D3D11CreateDevice returned error, try next adapter` | 确证 |
| 软编 | OpenH264（`.?AVCScrollDetection@WelsVP@@`） | 确证 |
| GPU 着色 | `PixAVCodec.dll` 亦导入 `D3D11CreateDevice` | 确证 |
| GIF | `?error@pixGifEncoder@@`（自研 `pixGifEncoder`）；配置 `ScreenShot.ActBarFlag.GifShot`；UI 动画用 Lottie（`:/PixScreenRecord/wheel_arrow.json`） | 确证 |
| 区域提示 | 字符串 `The toolbar overlaps with the recording area.` | 确证 |

### 6.5 翻译

- `PixPin.exe` 硬编码 **百度翻译**：`https://api.fanyi.baidu.com/api/trans/sdk/picture`、`https://api.fanyi.baidu.com/api/trans/vip/translate`（确证）
- Qt 资源 `:/PixTranslateEngine/PixTranslateDialogQss`、`transBall`；字符串 `%PixTranslateEngine`（确证）
- 配置项 `System.Translate.TranslateKey` 的值为**加密串**：`"JNHG3z6eiksKzmIq9zr6/WbZJGK9HQzBZaXDC0LNRkI=|||0"`（Base64，`|||` 后为版本标记）→ **API Key 二次封装**（确证，来自 `Config\PixPinConfig.json`）
- 需登录：`PixAuth.dll` + `PixWebCallback.dll`（`?onUrlSchemeCall@`、`?getCallbackUrl@`、`?supportWebView@`）+ `PixNetwork.dll`（`?imageToBase64@`、`?imageToBase64Url@`）→ **OCR 结果/图片经 `api.pixpin.cn` 中转**（强推断）

### 6.6 上传 / 网络

| 项 | 证据 | 强度 |
|---|---|---|
| 自有后端 | `https://api.pixpin.cn/p`（`PixAuth.dll`） | 确证 |
| 崩溃上报 | Crashpad + **Sentry**：DSN `https://d3e76aa4570f16ecbf8d8973d5801b09@bugreport.pixpin.cn/1`（`PixUtils.dll`）；`crashpad\…__sentry-event` | 确证 |
| 遥测 | `PixStat.dll` → `PixTrack::trackEvent/postTrackData/initHeaders/getProfileID` | 确证 |
| 政策页 | `https://pixpin.cn/docs/policy/privacy_zh_CN`、`tos_zh_CN`、`member_policy_zh_CN`（`PixUtils`/`PixAuth`） | 确证 |
| **图床** | `imgur`、`sm.ms` **均未命中** → **无第三方图床上传** | 确证（未命中） |
| 安全存储 | `PixAuth.dll`：`PixSecureStorageStrategy2` + `mapSystemStoreError` + `libcrypto-1_1-x64` 的 `engines\e_capi.c`（Windows 证书存储）→ 凭据受 OS 保护 | 强推断 |
| TLS | OpenSSL 1.1（`libssl-1_1-x64.dll`）；`PixNetwork` 有 `?sslErrors@` 重试 | 确证 |
| 更新 | `PixDownload.dll`；`LocalStorage.data`（实为 INI，含 `NoCheckUpdateUnti…`）；`UpgradeFile\PixPin_2.0.0.3.exe`(36 MB) + `upgradeZIP.bat` + `UpgradeFiles\`(空) | 确证 |

**升级机制（确证）**：`upgradeZIP.bat` 全文显示为**离线 ZIP 热替换**：参数 `<程序目录> <ZIP> <软件文件夹名> <unzip.exe>` → 建 `unzip_temp` → 拷 `unzip.exe` → 解压 → `cd` 进子目录 → `xcopy /y /e /i * "%program_dir%"` 覆盖 → 删除临时目录。配合 `unins000.exe/.dat`（**Inno Setup** 安装器）与 `UpgradeFile\PixPin_2.0.0.3.exe`（内置的旧版升级包）。

### 6.7 脚本 / 自动化（**重要发现**）

`Config\PixPinConfig.json` 中每个动作对象含 **`script` 字段**（确证）：
```json
"Action.Pin#s.win": {"v":{"index":3,"isSystemAction":true,
  "script":"pixpin.pinFromClipBoard()","shortCut":"F3","showOnTray":true,"title":"Pin","type":256}}
"Action.Screenshot#s.win": {"v":{"index":0,"isSystemAction":true,
  "script":"pixpin.screenShotAndEdit()","shortCut":"F1","showOnTray":true,"title":"Screenshot","type":256}}
```
`PixPin.exe` 字符串（确证）：
```
[PixPinScript::EvaluateScript] Missing QJSEngine for
[PixPinScript::%1] Script error source=%2 line=%3 message=%4
[PixPinScript::runScriptWithLongShot] Missing long shot context
```
另有脚本 API：`longshot.startStop()`、`longshot.toggleAutoScroll()`、`longshot.cropStart()`、`longshot.cropEnd()`、`longshot.edit()`。
→ **PixPin 内置基于 `QJSEngine` 的 JavaScript 动作/自动化系统**，用户可自定义动作脚本并绑定快捷键。这是一个**独立的可扩展面**，SnapClip 的插件架构（`docs/22`）可对标。

### 6.8 动作栏 Flags（确证，来自 `Config\PixPinConfig.json`）

`ScreenShot.ActBarFlag.*` 的值呈 `组高字节 | 索引低字节` 编码：
```
LongShot=256  GifShot=257  CopyOcr=258  Translate=259  Pin=260
Save=261      Close=262    Copy=263
OcrTable=512  QuickSave=513  LatexRecognition=514  WinRoi=515
ImageEdit=769 Print=768
```
→ 截图工具栏 = **位标志集合**，分为组 `0x100`（基础动作）与 `0x200`（OCR/长图高级动作）、`0x300`（编辑/打印）。

标注工具项（`Mark.EditItemOrder`，确证顺序与分组）：
`Geometry`、`HighLight` | `Pencil`、`Marker` | `Arrow`、`BrokenLine`、`Magnifier` | `Text`、`Watermark` | `Serial` | `Mosaic`、`AutoMosaic` | `Eraser`，另有 `Recycle` 区。
马赛克参数：`Mosaic.MosaicMode`、`Mosaic.MosaicStrength`、`Mosaic.BlurStrength`、`Mosaic.PathMode`。

---

## 7. 对 SnapClip 的可吸收点清单

按"投入产出比"排序。**标注了 PixPin 中对应证据。**

### P0 —— 直接决定架构正确性

1. **WGC 内核保持零 GDI 依赖，单独编译为原生子工程。**
   PixPin 的 `PixWin32CaptureCore.dll` 不导入 `user32/gdi32/dwmapi`，是一个可独立测试的纯 WGC 单元；它只导出 `prepareCapture/captureToBuffer/createEmptyBuffer/hasCaptureChanged/releaseCapture/updateRect` 这样极小的 C++ ABI 面。
   → SnapClip 应把采集器做成**同样的窄接口 + 零 UI 依赖**，并用 `ApiInformation` 做能力探测。

2. **D3D11 staging + shader 的帧搬运路径，而不是 WGC 回调里直接读像素。**
   证据：`createShaderTempTexture`/`createStagingTexture`/`TEXCOORD`/`Map` + `Invalid mapped resource RowPitch`。
   → 颜色转换/旋转/HDR 色调映射在 GPU 上做；CPU 只做 `Map` + 逐行 `memcpy`。注意 `Invalid stride value` / `BytesPerPixel` 校验。

3. **采集会话选项要显式关闭边框与光标。**
   PixPin 用 `IGraphicsCaptureSession3::IsBorderRequired(false)`（Win11 22H2+）与 `IGraphicsCaptureSession2::IsCursorCaptureEnabled`，并对不可用情况降级日志。
   → SnapClip 需做**带版本探测的降级链**，否则会出现黄框污染截图。

4. **长图必须 tile 化，不能是一张无限 `QImage`。**
   证据：`PixLongImage` + `PixLongImageTile(mMaxHeight)` + `contactImage/removeHead/removeTail/toImage`。
   → 直接决定"往回滚自动裁剪"能否实现；SnapClip 应把长图建模为 **tile 列表 + 头/尾插入/裁剪**。

5. **长图编码自己写流式 scanline 编码器，绕开 Qt 与整图内存。**
   证据：`PixLongImageFileEncodeThread::encodePng/encodeJpeg` + libpng 1.6.39 + `Missing tile data at row:` + `Row index is out of range`。
   → 这是把 500k 像素高图写盘且不 OOM 的唯一可行方式。

### P1 —— 决定体验差异

6. **全局输入钩子感知"用户手动滚轮"，而不只做自动滚。**
   证据：`PixKeyMouse.dll`（`SetWindowsHookExA`）+ `PixWheelEvent` + `PixWindowNotify` 目标进程判定。
   → 这是 PixPin "往回滚自动裁剪"的能力基础，也是与纯"自动滚动+拼接"实现的体验分水岭。

7. **合成滚轮要双通道：`SendInput` 优先，`PostMessage(WM_MOUSEWHEEL)` 兜底。**
   证据：`SimulateMouseScroll(bool,bool,int)` + 同时导入 `SendInput`/`PostMessageW`/`SendMessageW`。
   → 部分应用（Chromium、UWP）不吃 `PostMessage`；部分游戏不吃 `SendInput`。

8. **位移检测用"半帧重叠区"匹配 + 得分掩码筛选，而非整帧相位相关。**
   证据：`matchImageFast`（`CV_8UC1` + `cmpMask`）、`clearOffsetMaskByScore(maskRows, scoreRows)`、`legacy half-frame … fallback to full frame`。
   → 半帧更快，全帧兜底；候选加掩码再按分数取优，比单一 `phaseCorrelate` 稳。

9. **明确区分"高图模式"与"普通模式"，并按高度切换能力集。**
   证据：阈值 `29000`（`cmp eax,0x7148`），超长模式下 `pin` 被禁用、仅允许保存、JPEG 被拒绝。
   → 让用户可以预期；避免在超长模式下尝试贴图/复制到剪贴板而失败。

10. **编码器硬限制要在进编码前拦截，并给出可解释错误。**
    证据：JPEG 任一边 `> 65500` → 错误码 `0x2a` 且把 limit 记入错误上下文。
    → SnapClip 应显式建模 `JpegMaxEdge = 65500`、PNG 尺寸限制，并提前拒绝。

### P2 —— 工程与生态

11. **内置 JS 脚本动作系统（`QJSEngine`）。**
    证据：`PixPinScript::EvaluateScript` + 配置中 `"script":"pixpin.screenShotAndEdit()"` + `longshot.*` API。
    → SnapClip 的插件架构可先做**轻量脚本动作层**，再演进到原生插件。

12. **动作栏用"位标志组"建模，工具栏项可配置可见性。**
    证据：`ScreenShot.ActBarFlag.*` 的 `0x100`/`0x200`/`0x300` 分组编码 + `showOnTray`。

13. **DPI：只靠 Manifest `PerMonitorV2`，不在运行时调 API。**
    证据：Manifest 声明 + `SetProcessDpiAwarenessContext` 字符串未命中。

14. **HDR 显式检测并单独走色调映射分支。**
    证据：导出 `HasHDRMonitor()`；`QueryDisplayConfig` 导入。

15. **贴图窗口需排除自身采集 + 跨虚拟桌面 + 忽略鼠标 + 不进任务栏。**
    证据：`SetWindowNotRecord`/`SetWindowOnAllDesktops`/`SetWindowIgnoreMouse`/`SetWindowShowInTaskbar`。

16. **崩溃上报用 Crashpad + Sentry（自建 DSN），遥测与崩溃分离。**
    证据：`Helpers\crashpad_handler.exe` + `__sentry-event` + `PixStat.dll` 独立遥测。

### P3 —— 需谨慎/不建议照搬

17. **私有加密模型容器**（`model\*.bin`）不可复用 —— 需自建模型分发方案。
18. **公式识别链路（Caffe 模型 + libxslt + `MML2OMML.XSL`）**：`MML2OMML.XSL` 属 Microsoft Office 资产，**授权需评估**。
19. **百度翻译 API 硬编码**：建议做成可配置 provider，而非硬编码厂商。
20. **`PixOCR` / `PixOCR2` 两代并存**是历史包袱，SnapClip 不应模仿双份 OCR 模块。

---

## 8. 原始证据附录（关键命令与真实输出）

> 所有命令均为只读；分析产物写入 `%TEMP%\pixpin_re\`，**未写入 `C:\A_Softwares\PixPin`**。

### E1 工具探测

```powershell
Get-Command python,python3,py,dumpbin,objdump,llvm-readobj,strings,7z,curl -ErrorAction SilentlyContinue
$ExecutionContext.SessionState.LanguageMode
```
```
python.exe       C:\A_Softwares\Python\python.exe
objdump.exe      C:\A_Softwares\mingw64_v15.1.0\bin\objdump.exe
llvm-readobj.exe C:\A_Softwares\LLVM\bin\llvm-readobj.exe
strings.exe      C:\A_Softwares\mingw64_v15.1.0\bin\strings.exe
7z.exe           C:\A_Softwares\7-Zip\7z.exe
---LANGMODE---
FullLanguage
```
`dumpbin` **不存在**；`pefile` Python 包**未安装** → 自写 `pe.py`。

### E2 PE 头

```powershell
python pe.py hdr PixPin.exe PixPinAuxiliary.exe PixPinContextMenu\PixPinContextMenuExt.dll
```
```
### C:\A_Softwares\PixPin\PixPin.exe
  SizeOnDisk        : 23662904 (22.57 MB)
  Machine           : 0x8664 (AMD64)
  Characteristics   : 0x0022 [EXECUTABLE_IMAGE, LARGE_ADDRESS_AWARE]
  OptionalMagic     : 0x020b (PE32+)
  LinkerVersion     : 14.44
  Subsystem         : 2 (WINDOWS_GUI)  SubsysVer 6  OSVer 6
  DllCharacteristics: 0x8160 [HIGH_ENTROPY_VA?, DYNAMIC_BASE, NX_COMPAT, TERMINAL_SERVER_AWARE]
  TimeDateStamp     : 0x6a9e92b4 (2026-09-07T10:32:20Z)
  AddressOfEntryPoint: 0xc73ca8
  ImageBase         : 0x140000000   SizeOfImage 0x173a000  Checksum 0x169eab9
  Sections(7)       : .text(13400576), .rdata(8931840), .data(391168), .pdata(361984), .qtmetad(512), .rsrc(461824), .reloc(102400)
  DD[ 0] Export       rva=0x014fc620 size=140
  DD[ 1] Import       rva=0x014fc6ac size=1380
  DD[ 2] Resource     rva=0x016b0000 size=461336
  DD[ 6] Debug        rva=0x013fae70 size=84
```
（`PixPinAuxiliary.exe`：`LinkerVersion 14.44`、**`Subsystem 3 (WINDOWS_CUI)`**、`0x6a9e9162`；`PixPinContextMenuExt.dll`：`ImageBase 0x180000000`、导出 3 个 `Dll*` 函数）

### E3 PDB 路径

```powershell
python pe.py pdb PixPin.exe PixWin32CaptureCore.dll PixWinCapture.dll PixScreenManager.dll UiRegionDetector.dll UiSpy.dll PixVision.dll
```
```
### …\PixPin.exe  (Debug dir rva=0x13fae70 size=84)
  DebugEntry[0] type=CODEVIEW size=91 ptr=0x143c650
    RSDS Guid=ded140d5225e164dadc811d10944d5d8 Age=1
    PDB Path: D:\Code\Private\PixPinProject\build\windows\x64\release\PixPin.pdb
### …\PixWin32CaptureCore.dll
    PDB Path: D:\Code\Private\PixPinProject\PixWinCapture\x64\Release\PixWin32CaptureCore.pdb
### …\PixWinCapture.dll
    PDB Path: D:\Code\Private\PixPinProject\build\windows\x64\release\PixWinCapture.pdb
### …\PixScreenManager.dll
    PDB Path: D:\Code\Private\PixPinProject\build\windows\x64\release\PixScreenManager.pdb
### …\UiRegionDetector.dll
    PDB Path: D:\Code\Private\PixPinProject\build\windows\x64\release\UiRegionDetector.pdb
### …\UiSpy.dll
    PDB Path: D:\Code\Private\PixPinProject\build\windows\x64\release\UiSpy.pdb
### …\PixVision.dll
    PDB Path: D:\Code\Private\PixPinProject\build\windows\x64\release\PixVision.pdb
```

### E4 版本资源（`PixPin.exe`）

自写递归 `VS_VERSIONINFO` 解析 + 原始 UTF-16 键值扫描：
```
blob len 748
off=6     'VS_VERSION_INFO'
off=98    'StringFileInfo'
off=134   '000404b0'
off=158   'CompanyName'
off=184   'Shenzhen Shendu Tujing Technology'
off=258   'FileDescription'
off=292   'PixPin'
off=314   'LegalCopyright'
off=344   'Copyright 2022-2024 Shenzhen Shendu Tujing Technology Co., Ltd. All rights reserved.'
off=522   'OriginalFilename'
off=556   'PixPin.exe'
off=586   'ProductName'
off=612   'PixPin'
off=634   'ProductVersion'
off=664   '3.5.5.1'
off=686   'VarFileInfo'
off=718   'Translation'
```
> **`FileVersion` 键不存在** —— 这解释了为何 `(Get-Item PixPin.exe).VersionInfo.FileVersion` 返回空串。

Qt 侧（原生 API，交叉验证）：
```
Qt5Core.dll   FileVer=5.15.13.0  ProdVer=5.15.13.0  Company='The Qt Company Ltd.'  Prod='Qt5'
onnxruntime.dll FileVer=1.23.2   ProdVer=1.23.2     Company='Microsoft Corporation'  Descr='ONNX Runtime'
```

### E5 导入表（决定性）

```powershell
llvm-readobj --coff-imports PixWin32CaptureCore.dll
```
```
Import {
  Name: api-ms-win-core-winrt-error-l1-1-0.dll
  Symbol: GetRestrictedErrorInfo (0)
  Symbol: SetRestrictedErrorInfo (18)
}
Import {
  Name: api-ms-win-core-com-l1-1-0.dll
  Symbol: CoCreateFreeThreadedMarshaler
}
Import {
  Name: api-ms-win-core-winrt-l1-1-0.dll
  Symbol: RoGetActivationFactory
}
Import {
  Name: d3d11.dll
  Symbol: D3D11CreateDevice
  Symbol: CreateDirect3D11DeviceFromDXGIDevice
}
```
**整个模块无 `USER32.dll` / `GDI32.dll` / `dwmapi.dll` / `dxgi.dll` 导入项。**

```powershell
llvm-readobj --coff-imports PixWinCapture.dll
```
```
  [PixWin32CaptureCore.dll] (11): ??1PixWin32CaptureCore@@QEAA@XZ, ?prepareCapture@PixWin32CaptureCoreStatic@@QEAA_NXZ,
        ?hasCaptureChanged@PixWin32CaptureCore@@QEAA_NXZ, ?captureToBuffer@…, ??0PixWin32CaptureCoreStatic@@QEAA@AEBUParams@0@@Z,
        ?releaseCapture@…, ?getExpectedImageSize@…, ?updateRect@…, ??1PixWin32CaptureCoreStatic@@QEAA@XZ, ??0PixWin32CaptureCore@@QEAA@AEBUParams@0@@Z
  [dxgi.dll] (1): CreateDXGIFactory1
  [USER32.dll] (5): GetMonitorInfoW, EnumDisplayMonitors, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig
  [D3DCOMPILER_47.dll] (1): D3DCompile
  [Qt5Gui.dll] (9): ?size@QImage@@, ?height@QImage@@, ?bits@QImage@@, ?bytesPerLine@QImage@@, ??0QImage@@QEAA@AEBVQSize@@W4Format@0@@Z, …
```

```powershell
llvm-readobj --coff-imports PixKeyMouse.dll   # (经自写解析器提取)
```
```
  [PixWindowNotify.dll]: ?activeProcessPath@PixWindowNotify@@, ?refreshActiveWindow@PixWindowNotify@@, ?instance@PixWindowNotify@@
  [USER32.dll]: WindowFromPoint, GetWindowThreadProcessId, GetAncestor, GetAsyncKeyState, SendInput,
                SetWindowsHookExA, UnhookWindowsHookEx, GetKeyState, SetCursorPos
```

```powershell
llvm-readobj --coff-imports PixSystemUtils.dll
```
```
  [USER32.dll]: SetForegroundWindow, SetActiveWindow, SendInput, SetFocus, IsIconic, SetWindowDisplayAffinity,
                GetWindowDisplayAffinity, SetWindowPos, IsWindow, PostMessageW, GetDC, GetParent, SendMessageW,
                GetWindowThreadProcessId, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
                MsgWaitForMultipleObjects, GetClipboardSequenceNumber, GetForegroundWindow, GetGUIThreadInfo,
                GetClassNameW, FindWindowExW, ReleaseDC, …
  [dwmapi.dll], [OLEACC.dll], [GDI32.dll]: GetPixel, [SHELL32.dll]: SHOpenFolderAndSelectItems, SHQueryUserNotificationState
```

`PixVision.dll` / `PixOCR.dll` / `PixOCR2.dll` 导入（**无 OpenCV DLL**）：
```
PixVision.dll   : CONCRT140.dll, OPENGL32.dll, Qt5Core.dll, Qt5Gui.dll, KERNEL32, MSVCP140, VCRUNTIME140*
PixOCR.dll      : 同上 + onnxruntime.dll, PixNetwork.dll, ole32.dll
PixOCR2.dll     : 同上 + PixModelRunner.dll
PixModelRunner  : onnxruntime.dll
PixMovie.dll    : MF.dll, MFPlat.DLL, PixAVCodec.dll, PixKeyMouse.dll, PixLottie.dll, PixNotification.dll,
                  PixPinIcon.dll, PixScreenManager.dll, PixStyle.dll, PixSystemUtils.dll, PixWidget.dll, Qt5Multimedia.dll
PixAVCodec.dll  : ADVAPI32, bcrypt, USER32, ole32, Qt5Core, Qt5Gui（FFmpeg 已静态链接）
```

`PixPin.exe` 导入的 `Pix*` 模块（全量）：
```
PixActionsBar, PixAuth, PixAVCodec, PixColorPalette, PixConfiguration, PixDownload, PixFormulaRec,
PixKeyMouse, PixLatex2MathML, PixLottie, PixModelRunner, PixMovie, PixNetwork, PixNotification,
PixOCR2, PixPinIcon, PixPinTutorial, PixProgramManage, PixScreenManager, PixStat, PixStyle,
PixSystemUtils, PixUtils, PixVision, PixWidget, PixWidget2, PixWin32CaptureCore, PixWinCapture,
PixWindowNotify, UiRegionDetector, qxtglobalshortcut
+ Qt5{Core,Gui,Widgets,Network,Qml,Sql,Svg,Xml,PrintSupport,Multimedia,WinExtras}
+ d3d11, dxgi, D3DCOMPILER_47, dwmapi, GDI32, USER32, ole32, OLEACC, OLEAUT32, SHLWAPI, SHELL32,
  USERENV, WTSAPI32, ADVAPI32, bcrypt, CONCRT140, OPENGL32, MF, MFPlat, MFReadWrite, onnxruntime
```

### E6 字符串提取与关键词命中

```powershell
strings.exe -a -n 5    <file> > <base>.ascii.txt
strings.exe -a -el -n 5 <file> > <base>.utf16.txt
```
提取规模（节选）：
```
PixWin32CaptureCore.dll   ascii=782    utf16=22
PixWinCapture.dll         ascii=643    utf16=19
PixScreenManager.dll      ascii=689    utf16=2
UiRegionDetector.dll      ascii=636    utf16=6
UiSpy.dll                 ascii=572    utf16=2
PixVision.dll             ascii=34626  utf16=171
PixMovie.dll              ascii=3377   utf16=27
PixPin.exe                ascii=109813 utf16=1623
PixAVCodec.dll            ascii=40961  utf16=258
PixOCR.dll                ascii=30708  utf16=11
```

**捕获 API 关键词命中表**：

| 关键词 | 结果 |
|---|---|
| `Windows.Graphics.Capture` | **命中** `PixWin32CaptureCore.utf16.txt`：`Windows.Graphics.Capture.GraphicsCaptureItem` / `.Direct3D11CaptureFramePool` / `.GraphicsCaptureSession` |
| `CreateDirect3D11DeviceFromDXGIDevice` | **命中** `PixWin32CaptureCore.ascii.txt` |
| `RoGetActivationFactory` | **命中** `PixWin32CaptureCore.ascii.txt` |
| `D3D11CreateDevice` | **命中** `PixWin32CaptureCore`、`PixAVCodec`（后者含 `D3D11CreateDevice returned error, try next adapter`） |
| `IGraphicsCaptureItemInterop` / `CreateForWindow` / `CreateForMonitor` | **未命中**（封装于 `robmikh.common\capture.desktop.interop.h`，该头文件路径**命中**） |
| `CreateFreeThreaded` | 字面量**未命中**；`CoCreateFreeThreadedMarshaler` **命中** |
| `PrintWindow` | **命中，仅 `PixPin.exe`**：`PinWindowRoiMap::captureRoiImage - PrintWindow failed`、`PrintWindow` |
| `BitBlt` | **命中，仅 `PixPin.exe`** |
| `DwmFlush` / `IDXGIOutputDuplication` | **未命中** |
| `DwmGetWindowAttribute` | **命中** `PixSystemUtils`、`UiRegionDetector`、`UiSpy` |
| `DWMWA_EXTENDED_FRAME_BOUNDS` / `GetWindowDC` | **未命中**（宏/未用） |

**滚动 / 拼接关键词**：
```
[HIT ] SimulateMouseScroll   PixSystemUtils : ?SimulateMouseScroll@@YAX_N0H@Z
[HIT ] SimulateMouseWheel    PixSystemUtils : ?SimulateMouseWheel@@YAXVQPoint@@H@Z
[HIT ] SendInput             PixKeyMouse, PixPin, PixSystemUtils, PixUtils
[HIT ] PostMessageW          PixPin, PixSystemUtils
[HIT ] LongShot_AutoScrollEnabled   PixPin (UTF-16)
[HIT ] PixStitching\src\DetectDisplacement.cpp   PixPin
[HIT ] matchImageFast / clearOffsetMaskByScore / maskRows / scoreRows   PixPin
[HIT ] TILE_MAX_HEIGHT / mMaxHeight / putSize / contactImage / removeHead / removeTail / toImage   PixPin
[MISS] MOUSEEVENTF_WHEEL / HWHEEL / WM_MOUSEWHEEL / WH_MOUSE_LL / FullPage / Panorama / scrollOffset / maxStitch
[MISS] phaseCorrelate / matchTemplate(PixPin 自有代码) / SAD / NCC
```
> `matchTemplate` 在 `PixOCR`/`PixOCR2`/`PixVision` 中命中，但那是 **OpenCV 自带字符串池**，非 PixPin 代码。

**UI 自动化关键词**：
```
[HIT ] UiRegionDetector.ascii : [UiRegionDetector::DirectGetRect] Failed to create IUIAutomation instance
[HIT ] UiSpy.ascii           : [UiSpy::DirectGetRect] Failed to create IUIAutomation instance
[HIT ] PixSystemUtils.ascii  : [QDirSelectedFileGrabber::SelectedFilesFromUiAutomation] Failed to create UI Automation
                               [QDirSelectedFileGrabber::SelectedFilesFromUiAutomation] Failed to get root element
                               [QDirSelectedFileGrabber::SelectedFilesFromUiAutomation] No selected UIA items
[HIT ] AccessibleObjectFromWindow (UiRegionDetector, UiSpy, PixSystemUtils)
[HIT ] OLEACC.dll  (UiRegionDetector, UiSpy, PixSystemUtils)
[HIT ] WindowFromPoint / ChildWindowFromPoint (UiRegionDetector, UiSpy)
[MISS] ScrollPattern / TextPattern / RealChildWindowFromPoint
```

**显示排除 / 热键**：
```
[HIT ] SetWindowDisplayAffinity   PixSystemUtils : [SetWindowNotRecord] SetWindowDisplayAffinity FAILED for widget:
[MISS] WDA_EXCLUDEFROMCAPTURE / WDA_MONITOR / RegisterHotKey(PixPin 自身) / WH_KEYBOARD_LL / LowLevelKeyboardProc
[HIT ] RegisterHotKey  qxtglobalshortcut.dll
[MISS] GetDpiForMonitor / SetProcessDpiAwarenessContext / PerMonitorV2  （DPI 仅在 Manifest）
```

**解码/编码 / OCR / 公式 / 网络 / 脚本**：
```
[HIT ] Windows.Graphics.Capture …（见上）
[HIT ] onnxruntime.dll (PixModelRunner, PixOCR, PixPin)
[HIT ] [PixModelRunnerPrivate::createOnnxRuntimeEnv] Creating ONNX Runtime environment.
[MISS] PaddleOCR / paddle / Paddle / rapidocr / dbnet / DBNet / crnn / CRNN / ch_ppocr
[HIT ] :/PixLatex2MathML/latex_symbol.txt, mathml_normalize.xsl, MML2OMML.XSL
[HIT ] Latex2MathML / MathML2OMML / Copy as MathML formula / IconCopyAsMathML
[MISS] pix2tex
[HIT ] https://api.fanyi.baidu.com/api/trans/sdk/picture
[HIT ] https://api.fanyi.baidu.com/api/trans/vip/translate
[HIT ] https://api.pixpin.cn/p
[HIT ] https://d3e76aa4570f16ecbf8d8973d5801b09@bugreport.pixpin.cn/1
[MISS] imgur / sm.ms
[HIT ] [PixPinScript::EvaluateScript] Missing QJSEngine for
[HIT ] [PixPinScript::%1] Script error source=%2 line=%3 message=%4
```

### E7 `PixWin32CaptureCore.dll` 关键字符串（全量节选）

```
d3d11.dll
D:\Code\Private\PixPinProject\PixWinCapture\Win32CaptureSample\x64\Release\Generated Files\winrt\base.h
C++/WinRT version:2.0.240405.15
…\Generated Files\winrt\Windows.Foundation.h
…\Generated Files\winrt\Windows.Foundation.Collections.h
…\Generated Files\winrt\Windows.Foundation.Metadata.h
…\Generated Files\winrt\Windows.Graphics.Capture.h
…\packages\Microsoft.Windows.ImplementationLibrary.1.0.240803.1\include\wil\resource.h
…\packages\robmikh.common.0.0.23-beta\include\robmikh.common\d3d11Helpers.h
…\packages\robmikh.common.0.0.23-beta\include\robmikh.common\direct3d11.interop.h
…\packages\robmikh.common.0.0.23-beta\include\robmikh.common\capture.desktop.interop.h
D:\Code\Private\PixPinProject\PixWinCapture\Win32CaptureSample\PixWin32CaptureCore.cpp
CoIncrementMTAUsage
DllGetActivationFactory
POSITION
TEXCOORD
Failed to create capture item
[PixWinCapture::ApplyCaptureSessionOptions] IGraphicsCaptureSession3 is unavailable; capture border remains enabled
[PixWinCapture::ApplyCaptureSessionOptions] IGraphicsCaptureSession2 is unavailable; cursor capture remains enabled
PixWin32CaptureCoreStatic: Window capture mode not implemented in static version
Graphics Capture not supported on this system
initializeCapture
createShaderTempTexture / createStagingTexture / copyToBuffer
prepareCapture close frame pool / prepareCapture close session / prepareCapture session / prepareCapture frame pool
captureFrame start capture / captureFrame recreate session / captureFrame close session for resize
Source texture is not available / D3D context is not initialized
Invalid mapped resource RowPitch / Mapped resource data is null / Failed to map staging texture:
Buffer too small / Invalid stride value / Invalid bytes per pixel
BuildSecurityDescriptorW / ACS…
```
Manifest（内嵌，784 B）：
```xml
<assembly manifestVersion="1.0" xmlns="urn:schemas-microsoft-com:asm.v1">
 <dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls"
   version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
 </dependentAssembly></dependency>
 <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges>
   <requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security></trustInfo>
 <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1"><application>
   <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application></compatibility>
</assembly>
```

### E8 `PixLongImage` / `PixStitching` 字符串块（`PixPin.exe`，`.rdata` fileoff `0xd9c400`–`0xd9d800`）

```
[QImage2MatGray] 错误：图像无效
[PixStitching::tryAddImage] Failed to build legacy half-frame contact input, fallback to full frame.
Invalid tile rebuild input, tileMaxHeight:   imageHeight:
Failed to rebuild tile, startIndex:
[PixLongImage::startEncodeToFile] File path is empty.
[PixLongImage::startEncodeToFile] Unsupported file suffix:
[PixLongImage::startEncodeToFile] Invalid fixed size:
[PixLongImage::startEncodeToFile] Encountered null tile image.
[PixLongImage::startEncodeToFile] No valid image data to encode.
[PixLongImage::startEncodeToFile] Encode task is already running.
Image channel mismatch, image bytesPerLine:  width:  channel:
[PixLongImage::contactImage] Invalid TILE_MAX_HEIGHT:
[PixLongImage::contactImage] Gap is not allowed when appending first image, startIndex:
[PixLongImage::contactImage] Gap is not allowed, startIndex:  currentEndIndex:
[PixLongImage::contactImage] Head insertion must reach current start, startIndex:
[PixLongImage::contactImage] Failed to put image when processing head, startIndex:  putSize:
[PixLongImage::contactImage] Failed to put image into first tile when processing head, putSize:
[PixLongImage::contactImage] Head insertion size mismatch, addedHeight:  headInsertSize:
Added height exceeds image height, addedHeight:  image height:
[PixLongImage::contactImage] Missing tile coverage at startIndex:
Failed to put image when processing middle, startIndex:
Added height exceeds image height when processing middle, addedHeight:
[PixLongImage::contactImage] Append must start at current end, startIndex:
[PixLongImage::contactImage] Failed to put image when appending, startIndex:
[PixLongImage::removeHead]      [PixLongImage::removeTail]
[PixLongImage::toImage] Failed to create result image, startIndex:  endIndex:
[PixLongImage::toImage] Incompatible tile image at startIndex:
[PixLongImage::toImage] Failed to copy full image range, copiedHeight:  expectedHeight:
PixStitching\src\DetectDisplacement.cpp
matchImageFast
img1.type() == CV_8UC1 && img2.type() == CV_8UC1 && cmpMask.type() == CV_8UC1
img1.size() == img2.size() && img1.size() == cmpMask.size()
[DetectDisplacement::clearOffsetMaskByScore] No valid offset candidates. maskRows:  scoreRows:
[PixLongImageTile::putImage] Invalid input: newImage is null or putSize <= 0 or mMaxHeight <= 0.
[PixLongImageTile::putImage] Invalid input: startIndexInNewImage is out of bounds.
[PixLongImageTile::putImage] Invalid input: putSize exceeds mMaxHeight.
[PixLongImageTile::putImage] Failed to create compatible image.
[PixLongImageTile::putImage] Incompatible images.
[PixLongImageTile::putImage] Inserted image is completely before the tile.
[PixLongImageTile::putImage] Inserted image is completely after the tile.
[PixLongImageTile::putImage] Required height exceeds maximum allowed height.
Missing PNG write context      Failed to write PNG data
[PixLongImageFileEncodeThread::encode] Invalid snapshot.
[PixLongImageFileEncodeThread::encode] Unsupported file suffix:
[PixLongImageFileEncodeThread::prepareTileImages] Encountered null tile image.
[PixLongImageFileEncodeThread::prepareTileImages] Failed to rotate tile image for horizontal output.
[PixLongImageFileEncodeThread::prepareTileImages] Rotated tile height mismatch:
[PixLongImageFileEncodeThread::prepareTileImages] Failed to convert tile image.
Row index is out of range:      Scanline buffer overflow, offset:  copyBytes:
Scanline width mismatch, actualBytes:  expectedBytes:
[PixLongImageFileEncodeThread::encodePng] Invalid output size:
[PixLongImageFileEncodeThread::encodePng] Failed to open output file:
1.6.39
[PixLongImageFileEncodeThread::encodePng] Failed to create png_struct.
[PixLongImageFileEncodeThread::encodePng] Failed to create png_info.
[PixLongImageFileEncodeThread::encodePng] libpng reported an encoding failure.
[PixLongImageFileEncodeThread::encodePng] Encoding cancelled.
[PixLongImageFileEncodeThread::encodePng] Missing tile data at row:
[PixLongImageFileEncodeThread::encodePng] Failed to commit output file:
[PixLongImageFileEncodeThread::encodeJpeg] Invalid output size:
[PixLongImageFileEncodeThread::encodeJpeg] JPEG edge is too large:
[PixLongImageFileEncodeThread::encodeJpeg] Failed to open output file:
[PixLongImageFileEncodeThread::encodeJpeg] libjpeg reported an encoding failure:
[PixLongImageFileEncodeThread::encodeJpeg] Encoding cancelled.
[PixLongImageFileEncodeThread::encodeJpeg] Missing tile data at row:
[PixLongImageFileEncodeThread::encodeJpeg] Failed to commit output file:
SingleApplication: Unable to lock memory block after create. / attach …
```

### E9 `LongShotWidget` 字符串（`PixPin.exe`）

```
[DetectDisplacement::clearOffsetMaskByScore] No valid offset candidates
[LongShotMaskOverlay::updateMask] Capture rect not in widget, window rect:
[LongShotMaskOverlay::updateMask] Geometry is empty, skipping mask update
[LongShotWidget::actionBarInit] ActionsBar was destroyed before LongShotWidget
[LongShotWidget::activateForShortcut] Skip activation while closing
[LongShotWidget::activateForShortcut] Skip reentrant activation
[LongShotWidget::activateForShortcut] Widget became unavailable while processing activation events
[LongShotWidget::activateForShortcut] Widget became unavailable while processing focus events
[LongShotWidget::ApplyPostImageProcess] Failed to convert image to pixmap
[LongShotWidget::copy] export image is null          [LongShotWidget::copyOcrText] export image is null
[LongShotWidget::edit] export image is null
[LongShotWidget::getClippedCaptureRect] Target screen not found, using original capture rect
[LongShotWidget::onFocusTimerTimeout] Widget was destroyed while processing focus click
[LongShotWidget::pinToScreen] pin is disabled in super long mode
[LongShotWidget::saveSuperLongImageToFile] jpg is not supported for current logical length:
[LongShotWidget::saveToFile] convert image to pixmap failed / save image is null
[LongShotWidget::startSuperLongImageSave] encode failed or was canceled / failed to start encode for / logical image is null
[PixPinScript::runScriptWithLongShot] Missing long shot context
[ScreenShotView::longShot] View was destroyed while emitting long-shot signal
[ScreenShotView::longShot] View was destroyed while preparing shot metadata
afterLongShotInitAction   closeLongShot   hasLongShotWidget   longshot   LongShotDir   LongShotDirCtrl
LongShotMaskOverlay   LongShotSaveProgressDialog   LongShotWidget
LongShot.MatchFailWarningNoAsk   LongShot.MaxLengthWarningNoAsk
LongShot.StopClearConfirmNoAsk   LongShot.SuperLongWarningNoAsk
longshot.cropEnd()  longshot.cropStart()  longshot.edit()  longshot.startStop()  longshot.toggleAutoScroll()
BuiltInShortcutManage::addLongShotItem
/* LongShot ActionsBar Styles */
:/qss/LongShotActionsBar   :/qss/LongShotSaveProgressDialog
D:\Code\PixPinProject\PixPin\res\qss\LongShotActionsBar.qss
```

### E10 常量扫描与反汇编

字节扫描（little-endian dword）：
```
module                         29000       65000       65500     2000000  0x7fffffff  0x1fffffff   536870911
PixPin.exe                         9          16           1           7        1515          69          69
PixAVCodec.dll                    10           1           0           3         483          21          21
PixOCR.dll                         1           0           0           1         371          39          39
PixModelRunner.dll                 0           0           0           0          20          10          10
PixWinCapture.dll                  0           0           0           0          12           2           2
PixWin32CaptureCore.dll            0           0           0           0          29           1           1
```
**29000 全部 9 处位于 `PixPin.exe .text`**：`0x20f7e5, 0x20f92d, 0x20fbc9, 0x20fde1, 0x21065c, …`

反汇编（`VA = fileoff + 0xC00 + 0x140000000`）：
```asm
; 29000 超长模式阈值（VA 0x1402103e4）
1402103d3:  mov    rcx,QWORD PTR [rsi+0xb8]
1402103da:  test   rcx,rcx
1402103dd:  je     0x1402103f8
1402103df:  call   0x1403f1830
1402103e4:  cmp    eax,0x7148
1402103e9:  jle    0x1402103f8
1402103eb:  or     ebx,0x20

; 65500 JPEG 上限（VA 0x140c55de7），错误码 0x2a
140c55de7:  mov    ecx,0xffdc
140c55dec:  cmp    DWORD PTR [r14],ecx
140c55def:  jg     0x140c55df5
140c55df1:  cmp    DWORD PTR [rsi],ecx
140c55df3:  jle    0x140c55e0d
140c55df5:  mov    rax,QWORD PTR [rbx]
140c55df8:  mov    DWORD PTR [rax+0x28],0x2a
140c55dff:  mov    rax,QWORD PTR [rbx]
140c55e02:  mov    DWORD PTR [rax+0x2c],ecx
```

RIP-relative 扫描器自校验（证明"未命中"可信）：
```
VALIDATION: known ref to 0x140cf3f88 (objdump showed lea r8,[rip+0xae3a3d] @VA 0x140210544)
   found at VA=0x140210544 (instr len 7)
   found at VA=0x1402107f1 / 0x140210a0c / 0x140211287 / 0x140215421 / 0x140215da5
```
对 `TILE_MAX_HEIGHT` / `mMaxHeight` / "maximum stitching" 串的 RIP-relative 解析结果：**0 命中**。

### E11 配置文件全文

**`ConfigurationWindowConfig.ini`**（44 B，逐字）：
```ini
[General]
Geometry=@Rect(895 445 768 500)
```

**`pixmeta.dat`**（30 B，二进制，逐字节）：
```
33 43 39 65 0e 9a 22 9b 56 5f 59 10 12 1e 68 43 27 61 07 94 38 94 13 00 5b 08 10 10 6a 1c
```
（高熵，无可读结构；**格式未解**）

**`Config\CustomScreenshot.int`**（160 B）：
```ini
[General]
PresetArea="--size 500,500\n--name 480p --size 640,480\n--name 720p --size 1280,720\n--name 1080p --size 1920,1080\n"
PresetAreaMigrationVersion=1
```

**`Config\PixPinConfig.json`**（4,028 B，全文）：
```json
{"Action.Close all pin window#s.win":{"t":1775435398},"Action.Custom screenshot#s.win":{"t":1775435398},"Action.Pin selected file#s.win":{"t":1775435398},"Action.Pin#s.win":{"t":1775435407,"v":{"index":3,"isSystemAction":true,"script":"pixpin.pinFromClipBoard()","shortCut":"F3","showOnTray":true,"title":"Pin","type":256}},"Action.Restore last closed#s.win":{"t":1775435398},"Action.Screenshot and copy#s.win":{"t":1775435398},"Action.Screenshot#s.win":{"t":1775435403,"v":{"index":0,"isSystemAction":true,"script":"pixpin.screenShotAndEdit()","shortCut":"F1","showOnTray":true,"title":"Screenshot","type":256}},"Action.Switch pin group#s.win":{"d":1775656506},"Appearance.ThemeFont#s.win":{"t":1775435398},"Appearance.ThemeMode":{"t":1775435398},"Appearance.TrayIcon":{"t":1775435398},"BIShortcut.pixpin.0#s.win":{"t":1775435398},"Mark.ActBarFlag.HighLight":{"t":1737617762,"v":66058},"Mark.EditItemOrder":{"t":1790398020,"v":{"Recycle":[],"Vaild":[{"list":["Geometry","HighLight"],"value":"Geometry"},{"list":["Pencil","Marker"],"value":"Pencil"},{"list":["Arrow","BrokenLine","Magnifier"],"value":"BrokenLine"},{"list":["Text","Watermark"],"value":"Text"},{"list":["Serial"],"value":"Serial"},{"list":["Mosaic","AutoMosaic"],"value":"Mosaic"},{"list":["Eraser"],"value":"Eraser"}]}},"MarkBar.Arrow.LineShape":{"t":1790397988,"v":4},"MarkBar.Arrow.PenWidth":{"t":1790397981,"v":8},"MarkBar.Common.Color":{"t":1790324315,"v":"ffd84a2f"},"MarkBar.Geometry.Filling":{"t":1790324311,"v":false},"MarkBar.Geometry.PathMode":{"t":1790321399,"v":2},"MarkBar.Geometry.PenWidth":{"t":1786981657,"v":9},"MarkBar.Geometry.RectRoundRadius":{"t":1790318913,"v":90},"MarkBar.HighLight.PathMode":{"t":1790318504,"v":3},"MarkBar.Mosaic.BlurStrength":{"t":1752583932,"v":28},"MarkBar.Mosaic.MosaicMode":{"t":1752583949,"v":0},"MarkBar.Mosaic.MosaicStrength":{"t":1752583845,"v":28},"MarkBar.Mosaic.PathMode":{"t":1752583943,"v":2},"MarkBar.Pencil.PenStyle":{"t":1790354365,"v":1},"MarkBar.Text.Size":{"t":1770814376,"v":20},"Pin.ConfirmationFlags":{"t":1791074286,"v":10},"PixMovie.record.recordMicrophone":{"t":1790993243,"v":false},"PostProcess.Shot.Enable":{"t":1790411926,"v":false},"PostProcess.Shot.EnableForEveryShot":{"t":1790411144,"v":false},"PostProcess.Shot.Modules":{"t":1790411146,"v":2},"PostProcess.blur.color":{"t":1790411145,"v":"ffa0a0a4"},"PostProcess.blur.strength":{"t":1790411145,"v":8},"PostProcess.border.color":{"t":1790411148,"v":"ffffffff"},"PostProcess.border.strength":{"t":1790411146,"v":27},"Save.SaveQuality":{"t":1791424028,"v":100},"ScreenShot.ActBarFlag.Close":{"t":1790318723,"v":262},"ScreenShot.ActBarFlag.Copy":{"t":1790318723,"v":263},"ScreenShot.ActBarFlag.CopyOcr":{"t":1790318723,"v":258},"ScreenShot.ActBarFlag.GifShot":{"t":1790318723,"v":257},"ScreenShot.ActBarFlag.ImageEdit":{"t":1790318723,"v":769},"ScreenShot.ActBarFlag.LatexRecognition":{"t":1790318723,"v":514},"ScreenShot.ActBarFlag.LongShot":{"t":1790318723,"v":256},"ScreenShot.ActBarFlag.OcrTable":{"t":1790318723,"v":512},"ScreenShot.ActBarFlag.Pin":{"t":1790318723,"v":260},"ScreenShot.ActBarFlag.Print":{"t":1790318723,"v":768},"ScreenShot.ActBarFlag.QuickSave":{"t":1790318723,"v":513},"ScreenShot.ActBarFlag.Save":{"t":1790318723,"v":261},"ScreenShot.ActBarFlag.Translate":{"t":1790318723,"v":259},"ScreenShot.ActBarFlag.WinRoi":{"t":1790318723,"v":515},"Screenshot.ConfirmOnCloseByKey":{"t":1775820008,"v":false},"Screenshot.SizeDisplayItems":{"t":1790437634,"v":7},"Screenshot.SizeUnit":{"t":1790411083,"v":0},"Screenshot.enableRoundRect":{"t":1790411131,"v":true},"Screenshot.roundRectRatio":{"t":1791127269,"v":0},"System.DesktopToolBar":{"t":1775435429,"v":2},"System.IgnoreCopyMacros":{"t":1775435398},"System.Language":{"t":1791096719,"v":"auto"},"System.Run After Boot":{"t":1775557268,"v":true},"System.Run After Boot.RunAsAdmin":{"t":1775435398},"System.Text Recognition.CopyTextConfirmation":{"t":1775993522,"v":5},"System.Translate.TranslateKey":{"t":1791096720,"v":"JNHG3z6eiksKzmIq9zr6/WbZJGK9HQzBZaXDC0LNRkI=|||0"}}
```
> 结构：`"<Key>": {"t": <unix 秒>, "v": <值>}`；`"#s.win"` 后缀表示 Windows 平台作用域键。

### E12 模型文件魔数与熵

```
2478d3813bea9afacf7fee18abb09d45.bin    9,880,512  H=7.56  head32=7f7959262f365bcad4b02558f33646630f7945455a581757373410635c193b3d
5f515ede591a02e400275eaa2e5c0ddf.bin   16,615,441  H=7.24  head32=7f7959262f365bb49e9a2658fd3646630f7947455a5817573734117c19687c70
94186d8ad4eacc8b8aaf3c1088eb9353.bin    4,729,474  H=7.62  head32=7f7471d3e196636fd56a33400c5638011f2c2549475b53010c5b0f3031076e6a
cb6cc28d4121651b3f5bc3daecc9188e.bin   21,234,344  H=7.37  head32=7f7559262f365ba4e8e02b58fa3646630f7946455a5817573734176a40401352
dbb5b4317e638ad5a21a42b035cfd159.bin   10,838,604  H=7.61  head32=7f747184c2a5646fee6933460c5638011f2c2549475b53010c5a156a40551352
detect.caffemodel                          965,430  head=0a00a206220a04646174611205496e7075742204646174615001  strings: BatchNorm, data/bn/scale
detect.prototxt                             45,372  head='layer {\r\n  name: "data"\r\n  type: "Input"…'
sr.caffemodel                               23,929  head=0a00a206220a04646174611205496e707574…  strings: data_data_0_split, Convolution
sr.prototxt                                  6,387  head='layer {\r\n  name: "data"\r\n  type: "Input"…'
paragraph_recognition.onnx               3,198,623  head=080a12077079746f7263681a0a322e31312e302b637075  strings: '2.11.0+cpu:', node_features, aten.sym_size.int
```
> 5 个 `.bin` 有共同前缀族（`7F 79 59 26 2F 36 5B` / `7F 74 71 …`），熵 ~7.5 bit/byte → **加密或压缩容器**；`7F 79 59 26 2F 36 5B` 族内多文件前 7 字节一致（仅第 2 字节不同），说明是**固定头部 + 高熵载荷**，而非纯 XOR 流。

### E13 升级机制

```
UpgradeFile\PixPin_2.0.0.3.exe   36,468,120 B
UpgradeFiles\                    (空)
unins000.exe  4,434,872 B / unins000.dat 376,831 B   (Inno Setup)
upgradeZIP.bat  1,394 B
zip.exe 146,744 B / unzip.exe 179,512 B
LocalStorage.data 30,954 B  → 实为 INI：'[General]\r\nUpdateGrayscale=18\r\nNoCheckUpdateUnti…'
```
`upgradeZIP.bat` 全文见 §6.6。

### E14 崩溃上报

```
Helpers\crashpad_handler.exe   700,728
Helpers\crashpad_wer.dll        24,888
crashpad\crashpad_handler.exe  555,264
crashpad\installation_id            70
crashpad\last_crash                 27   -> 2026-01-14T13:06:31.146852Z
crashpad\metadata                   16   -> 'DAPC\x01…'
crashpad\…run\__sentry-event       509
crashpad\…run\__sentry-breadcrumb1/2  0
crashpad\…run\session.json         252
```

### E15 `PinWindowd.sqlite` / 数据目录

```
Data\PinWindowd.sqlite  106,496 B   magic = 'SQLite format 3\x00'  (确证 SQLite)
Data\*.meta  100 个 / Data\*.png  99 个
History\_ScreenshotRecord\*.his  60+ 个（0.63–10.73 MB）
```

---

## 9. 未解疑点清单

| # | 疑点 | 已尝试手段 | 影响 |
|---|---|---|---|
| 1 | **"最大拼接范围"上限的精确常量与判据表达式**。父代理的 `INT_MAX/4 = 536,870,911 px` 假说数值上高度自洽（官方产物 531,802,642 px = 99.06%），但**无法用静态手段钉死**。 | 字节扫描 `0x7fffffff`(1515×) / `0x1fffffff`(69×) / `0x20000000`(6030×) —— 全部被噪声淹没；绝对指针搜索 0 命中；RIP-relative disp32 求解 0 命中（扫描器已自校验）；全 `.text` 反汇编 3.48M 行 grep 0 命中。 | 只影响"能否精确对齐上限"，不影响 tile 架构设计 |
| 2 | **`TILE_MAX_HEIGHT` / `mMaxHeight` 的数值**。符号与错误串确证存在，值未解。 | 同上。候选 8192/16384/32768/65536 均无法排除。 | 影响 tile 大小选型参考值 |
| 3 | **`model\*.bin` 的容器格式**（加密算法/密钥派生）。 | 魔数识别、熵分析、头部对比、8/4 字节指针搜索。 | SnapClip 无法复用其模型，需自建分发 |
| 4 | **`pixmeta.dat`（30 B）语义**。 | 逐字节 hex；无结构、无可读串。 | 低 |
| 5 | **`ClipLeak`/`PixPinContextMenuExt.dll` 的完整上下文菜单项定义**。仅确证 3 个 COM 导出 + 导入 `SHELL32`/`SHLWAPI`/`ole32`；菜单项文案在 `PixPinContextMenu.msix`(163,377 B) 内未展开。 | — | 低 |
| 6 | **`PixPinAuxiliary.exe`（`WINDOWS_CUI`，控制台子系统，749,880 B）的具体职责**。无资源、无版本信息；未确证其用途（疑为提权/自更新/预热辅助进程）。 | 未展开其字符串分析。 | 中（涉及提权与自更新架构） |
| 7 | **LaTeX 渲染引擎的具体开源项目**。`tex::Macro`/`tex::ScriptsAtom` 命名风格指向 MicroTeX 一类 C++ TeX 引擎，但磁盘上无对应库文件，未做强断。 | `PixFormulaRec.dll` 符号分析。 | 中（若 SnapClip 要做公式渲染） |
| 8 | **`UiSpy.dll` 与 `UiRegionDetector.dll` 的并存原因与分工**。两者导出几乎逐一对应，`UiRegionDetector` 的 `sigNewRectDetected` 多 5 个参数（`QPoint,qulonglong,qint64,QRect,QRect`）。**强推断** `UiRegionDetector` 为新版（多进程/Z-order 信息），`UiSpy` 为旧版；未从调用方确证。 | 导出表对比。 | 低 |
| 9 | **OCR 是否使用 PaddleOCR**。`PaddleOCR`/`paddle`/`dbnet`/`crnn`/`ch_ppocr` **全部未命中**。已确证的是 自研 Caffe(det+sr) + ONNX(paragraph) + OpenCV 预处理。`model\*.bin` 内是否藏着 Paddle 模型**无法确认**（已加密）。 | 全库字符串扫描。 | **中高** —— 影响 SnapClip 的 OCR 选型对标 |
| 10 | **`FileVersion` 缺失**（仅 `ProductVersion=3.5.5.1`）是否会影响 Windows 兼容性/升级判断。 | VERSION 资源全量键值扫描确证 `FileVersion` 键不存在。 | 低 |
| 11 | **`QJSEngine` 脚本系统的 API 完整面**。仅确证 `pixpin.pinFromClipBoard()`、`pixpin.screenShotAndEdit()`、`longshot.*`；完整 API 需运行时枚举。 | — | 中（若要对标插件/自动化） |
| 12 | **超长模式下 `ImgEdit`/`Print`/`OCR` 是否真的全部禁用**。文案称 "Only saving is available"，但 `PixLongImage::toImage` 支持任意 `startIndex/endIndex` 区间导出，理论上可供 OCR 分块使用。未确证。 | — | 低 |

---

### 附：结论来源标注

- **全部 §2–§6 的技术判定均来自本次静态分析**（PE 头/导出/导入/字符串/反汇编/常量扫描/配置与数据文件）。
- **公开资料**：本报告中**未使用**任何网络检索结论；唯一的"外部输入"是父代理提供的官方 3.2.3.1 更新说明截图中 `1058 × 502649` 的实测维度（已明确标注为**父代理提供**，用于 §4.4 的间接推断）。
- **未执行 `PixPin.exe`**；**未修改 `C:\A_Softwares\PixPin` 内任何文件**。所有中间产物位于 `%TEMP%\pixpin_re\`：
  `pe.py`、`exports_all.txt`、`imports.txt`、`strings\*.{ascii,utf16}.txt`、`needle_hits.json`、`PixPin.text.asm`(199 MB)、`consts.py`、`deep1.py`、`riprel.py`、`final_scan.py`、`anchor.py`、`budget.py`、`iat.py`、`imm_scan.py`、`Aux.{ascii,utf16}.txt`。

---

## 10. 补充调查（第二轮）：Auxiliary 职责 / tile 字节预算 / 滚轮注入语义

> **① `PixPinAuxiliary.exe` 职责 — 确证；② `TILE_MAX_HEIGHT` — 确证（128 MiB 字节预算，非固定高度）；③ 滚轮注入语义 — 确证。**
>
> **更正 §4.6 / §6.5 一处表述**：先前写的"`SendInput` 优先、`PostMessage` 兜底"**是错的**。`SimulateMouseScroll` 与 `SimulateMouseWheel` 是**两个机制完全不同、互不降级**的函数，详见 §10.3。

### 10.1 `PixPinAuxiliary.exe` 职责（三问逐条）

**PE 元数据（确证）**

| 项目 | 值 |
|---|---|
| 大小 | 749,880 B |
| Machine / OptionalMagic | `0x8664` AMD64 / PE32+ |
| **Subsystem** | **3 = WINDOWS_CUI（控制台子系统）** |
| **AddressOfEntryPoint** | `0x56834` |
| LinkerVersion | 14.44 |
| DllCharacteristics | `0x8160`（HIGH_ENTROPY_VA / DYNAMIC_BASE / NX_COMPAT / TERMINAL_SERVER_AWARE） |
| TimeDateStamp | `0x6a9e9162` = 2026-09-07T10:26:42Z（比 `PixPin.exe` 早 5 分 38 秒） |
| Sections | `.text`(547,840) `.rdata`(152,064) `.data`(9,216) `.pdata`(23,552) **`.fptable`**(512) `.reloc`(4,096) |
| 资源 / 导出 | **均无**（DD[2]=0、DD[0]=0）→ 无图标、无 manifest、无 VERSION、无导出 |
| **PDB** | `D:\Code\Private\PixPinProject\build\windows\x64\release\PixPinAuxiliary.pdb`（RSDS `bf400cc0e1df13429b5f4751a5e22b9f` / Age 1） |

**命令面（确证）** —— UTF-16 用法串逐字恢复：
```
Usage: %s Restart <mode> <exepath>                                          (== RestartProcessWmain)
Usage: %s CrashRestart <mode> <exepath> <pid> <statepath> <logpath>
Usage: %s Upgrade <TargetRootPath> <UpgradeFilePath> <ExeName>
Usage: %s TestDialog
Usage: RestartProcess <mode> <exepath>
Example: RestartProcess Elevated C:\Program Files\MyApp\App.exe
mode: Elevated - Restart process with administrator privileges
Mode must be either 'Elevated' or 'Normal'
[RestartProcessWmain] Invalid mode:          [PixPinAuxiliary::wmain] LogPath:
```
→ **多子命令控制台工具**：`RestartProcess` / `CrashRestart` / `Upgrade` / `TestDialog`。

#### ① 非提权主进程如何配合「以管理员身份运行」→ **确证：辅助进程做提权代理**

- **它确实是提权代理**（确证）。UTF-16 字符串：
  ```
  runas
  run_elevated with params={}
  [RestartProcess::StartRestartedProcess] Starting process in elevated mode
  [RestartProcess::StartRestartedProcess] Failed to start elevated process
  [RestartProcess::StartRestartedProcess] Starting process in normal mode
  [RestartProcess::StartRestartedProcess] Failed to start normal process
  ```
- **提权手段 = `ShellExecuteExW` + `runas` verb**（确证）：`SHELL32.dll` 的**唯一**导入就是 `ShellExecuteExW (431)`，与 `runas` 字符串并存 → 走 ShellExecute 的 `runas` 动词触发 UAC，**而非** `CreateProcessW`（后者不支持提权）。
- **令牌/SID 自检**（确证）：`ADVAPI32.dll` 仅 5 个导入且全部围绕身份 —— `OpenProcessToken`、`GetTokenInformation`、`LookupAccountSidW`、`GetUserNameW`、`GetSecurityInfo`。用于判断"当前是否已提权 / 当前用户是谁"，决定 `Elevated` 与 `Normal` 分支。
- **主进程侧对应符号**（第一轮确证）：`PixProgramManage.dll` 导出 `?restartToAdmin@`、`?setRunAsAdmin@`、`?isAdministrator@`。主进程 manifest 确为 **`asInvoker` + `uiAccess="false"`**。

**结论**：主进程保持 `asInvoker`；用户勾选"以管理员身份运行"（或崩溃后需提权重启）时，主进程调用 **`PixPinAuxiliary.exe RestartProcess Elevated <exepath>`**，由辅助进程用 **`ShellExecuteExW` + `runas`** 弹 UAC 并拉起提权实例。**是"辅助进程做提权代理"，不是主进程自提权。**（确证）

#### ② 是否参与捕获 → **未参与（未命中）**

| 检查项 | 结果 |
|---|---|
| `Windows.Graphics.Capture` / `GraphicsCaptureItem` / `Direct3D11CaptureFramePool` | **未命中** |
| `d3d11.dll` / `dxgi.dll` 导入或 `D3D11`/`dxgi` 字符串 | **未命中** |
| `PrintWindow` / `SetWindowDisplayAffinity` / `SetWindowsHookEx` / `SendInput` | **未命中** |
| `Capture` 字符串 | 仅命中 **`RtlCaptureContext`**（CRT 异常展开，与采集无关） |
| `BitBlt` | **命中**：`GDI32.dll :: BitBlt (19)`，且**同时导入** `CreateCompatibleDC (49)`、`CreateCompatibleBitmap (48)`、`CreateSolidBrush`、`CreatePen`、`CreateFontW`、`CreateRoundRectRgn` |

对 `BitBlt` 的定性（**强推断：非采集，而是自身分层窗口的双缓冲绘制**）：
- `CreateCompatibleDC` + `CreateCompatibleBitmap` + `BitBlt` 是 Win32 **标准双缓冲三件套**；配合 `CreateWindowExW` + `SetLayeredWindowAttributes` + `SetWindowRgn` + `CreateRoundRectRgn` + `DrawTextW` + `BeginPaint`/`EndPaint` + `RoundRect` + `Ellipse` + `SetTimer`/`KillTimer` + `SetProcessDPIAware` + `MonitorFromPoint`/`GetMonitorInfoW` → 一个**分层、圆角、无边框、居中于某显示器的自绘窗口**。
- 该窗口用途有直接佐证：`[Upgrade::UpgradeWmain] Creating hidden UpgradeDialog window`、`Hidden UpgradeDialog window ready`、`Showing UpgradeDialog for 5 seconds...` → **升级进度提示窗**。
- 反证：若为屏幕采集，应出现 `GetDC(NULL)` 取屏、`GetSystemMetrics`、整屏位图拷贝等特征，且通常需 `PrintWindow` —— 这些**均未命中**；且它是 CUI 子系统的短命令工具。

**结论**：**`PixPinAuxiliary.exe` 不参与屏幕采集**（确证：无 WGC/D3D11/DXGI/PrintWindow）。因此 **PixPin 在 UIPI/高权限窗口场景不存在第二条捕获通道** —— 采集只有 `PixWin32CaptureCore`(WGC) 一条路。
> 对 `docs/19` §3.5「输入失败时的手动模式」与 §6.2「UIPI 拒绝」的含义：**UIPI 场景下没有"换一条采集通道"的退路**，只能走"提示用户手动操作 / 降级"路线。

#### ③ 是否参与自更新/热替换 → **确证：它就是更新器**

- **`Upgrade` 子命令 = `upgradeZIP.bat` 逻辑的 C++ 内化版**（确证）：
  ```
  [Upgrade::UpgradeWmain] Starting upgrade process
  [Upgrade::UpgradeWmain] TargetRootPath:   UpgradeFilePath:   ExeName:
  [Upgrade::UpgradeWmain] Zip upgrade       [Upgrade::UpgradeWmain] Exe upgrade
  [Upgrade::UpgradeWmain] Waiting for process exit:
  [Upgrade::UpgradeWmain] Process is still running, upgrade failed
  [Upgrade::UpgradeWmain] Deleting upgrade file:
  [Upgrade::UpgradeWmain] Upgrade failed; keep the package and do not restart PixPin
  [Upgrade::zipUpgrade] Delete/Create unzip_temp directory / Unzip upgrade file
  [Upgrade::zipUpgrade] Upgrade package has a missing or empty required file:
  [Upgrade::zipUpgrade] Copy updated files and folders from unzip_temp to targetRootPath:
  [Upgrade::RemoveLegacyQtPluginDirectories] Remove legacy Qt plugin directory:
  [Upgrade::exeUpgrade] Run upgrade program:
  [Upgrade::exeUpgrade] Wait 1 second before starting installer to ensure all file handles are released
  ```
  与 `upgradeZIP.bat` 的 `unzip_temp`/`xcopy` 流程**逐字对应**，并多出 `RemoveLegacyQtPluginDirectories`（清理旧版 Qt 插件目录）与 `Exe upgrade`（转交 Inno Setup 安装器，即 `unins000.exe` 体系）两条路径。
- **并发保护**（确证）：`Global\PixPinUpdater` 互斥体 —— `Acquired global mutex: Global\PixPinUpdater`、`Another upgrade process is already running`、`Releasing global mutex`；导入 `CreateMutexW`/`ReleaseMutex`/`CreateToolhelp32Snapshot`/`Process32FirstW`/`Process32NextW`/`TerminateProcess`/`WaitForSingleObject`。
- **文件替换能力**（确证）：`MoveFileExW`、`CopyFileW`、`DeleteFileW`、`RemoveDirectoryW`、`CreateDirectoryW`、`SetFileAttributesW`、`SetEndOfFile`、`FindFirstFileW`/`FindNextFileW`、`SetFilePointerEx` → 典型"独立于主程序、可在主程序退出后替换自身文件"的更新器。
- **密码学**（确证）：`bcrypt.dll` 13 个导入 = `BCryptDeriveKeyPBKDF2`、`BCryptGenRandom`、`BCryptCreateHash`/`HashData`/`FinishHash`/`DestroyHash`、`BCryptEncrypt`、`BCryptGenerateSymmetricKey`/`DestroyKey`、`BCryptOpenAlgorithmProvider`/`CloseAlgorithmProvider`/`SetProperty`/`GetProperty`。
  → **PBKDF2 派生密钥 + 哈希校验 + 对称加密**（**强推断**：升级包完整性与解密）。**这与第一轮发现的 `model\*.bin` 私有加密容器、`pixmeta.dat`(30 B 高熵)、`System.Translate.TranslateKey` 的 `Base64|||0` 加密值属同一套 crypto 能力**（`PixPin.exe` 亦导入 `bcrypt.dll`）。
- **`CrashRestart` 子命令**（确证）—— 崩溃后重启 + **限流**：
  ```
  [CrashRestart::TryRecordCrashRestart] Recent restart count:
  [CrashRestart::TryRecordCrashRestart] Restart skipped because the 10-minute limit was reached
  [CrashRestart::WriteRestartTimes] Failed to create state directory: / Failed to open state file:
  ```
  参数 `<pid> <statepath> <logpath>`：等旧进程退出、把重启次数写入 state 文件、**10 分钟内不再重启**（防崩溃循环）。
- **是否参与「预热」(`initPreheatProgram`) → 未命中**：`?initPreheatProgram@@YAXXZ` 位于 **`PixProgramManage.dll`（主进程侧）**；`PixPinAuxiliary.exe` 中**无 `Preheat`/`preheat`**，其子命令仅 4 个。→ **预热由主进程自己做，辅助进程不参与。**

**结论**：`PixPinAuxiliary.exe` = **"提权代理 + 崩溃重启器 + 独立更新器"三合一控制台工具**，全部为非捕获职责（确证）。

### 10.2 `PixLongImage` / `PixLongImageTile` 尺寸预算 —— **确证：128 MiB 字节预算，tile 高度按宽度反算**

**定位方法（本轮关键突破）**：上一轮的 RIP-relative **精确地址匹配扫描 0 命中**。本轮改为**锚点区间扫描** —— 解析所有 `disp32` 得到目标 VA，判定其是否落入 `[字符串偏移-200, +128]`。结果**大量命中**（相对目标串的 delta 有 -151 / -101 / -53 / -45 / -29 / +17 / +22 / +35 / +56 / +84 / +123 等）。
→ **原因**：MSVC `/GF` 字符串池会合并相同后缀，代码引用的地址常落在池内**相邻/偏移**位置，**精确地址匹配必然失败**。

**决定性反汇编**（`PixLongImage::contactImage`，函数体约 `VA 0x1403f2930`–`0x1403f373e`）：
```asm
; ---- 惰性计算并缓存每 tile 高度上限 ----
1403f2a86:  mov    rax,QWORD PTR [r15+0x40]        ; this->mTileMaxHeight（缓存）
1403f2a8a:  test   rax,rax
1403f2a8d:  jne    0x1403f2aad                      ; 已算过 -> 复用
1403f2a8f:  lea    rcx,[rbp-0x39]
1403f2a93:  call   QWORD PTR [rip+0x8dd57f]        ; -> Qt5Gui!QImage::bytesPerLine()
1403f2a99:  mov    ecx,eax                          ; ecx = bytesPerLine
1403f2a9b:  mov    eax,DWORD PTR [rip+0x117321f]    ; -> [0x141565cc0] = 0x08000000
1403f2aa1:  cdq
1403f2aa2:  idiv   ecx                              ; eax = 0x08000000 / bytesPerLine
1403f2aa4:  cdqe
1403f2aa6:  mov    QWORD PTR [r15+0x40],rax         ; this->mTileMaxHeight = ...
1403f2aaa:  test   rax,rax
1403f2aad:  jg     0x1403f2afc                      ; <=0 -> 下面 "Invalid TILE_MAX_HEIGHT"
; ---- 失败分支 ----
1403f2acf:  lea    rdx,[rip+0x9ab06a]               ; -> 0x140d9db40
                                                      ;    "[PixLongImage::contactImage] Invalid TILE_MAX_HEIGHT:"
1403f2adf:  mov    rdx,QWORD PTR [r15+0x40]
1403f2ae6:  call   QWORD PTR [rip+0x8dabbc]        ; -> QDebug::operator<<(qlonglong)
```
IAT 解析（确证调用目标）：
```
0x140cd0018 -> Qt5Gui.dll :: ?bytesPerLine@QImage@@QEBAHXZ     <<< 除数来源
0x140ccfe40 -> Qt5Gui.dll :: ?width@QImage@@QEBAHXZ
0x140ccfe48 -> Qt5Gui.dll :: ?height@QImage@@QEBAHXZ
0x140ccfe38 -> Qt5Gui.dll :: ?isNull@QImage@@QEBA_NXZ
0x140ccf610 -> Qt5Gui.dll :: ?depth@QImage@@QEBAHXZ
0x140cceb90 / 0x140cceb80 / 0x140cce980 / 0x140cce988 / 0x140cce990
            -> Qt5Core.dll :: QMessageLogger ctor / critical() / QDebug<<(char const*) / <<(int) / ~QDebug
```
**常量本体（确证）**：`VA 0x141565cc0` 在 `.data`（fileoff `0x15644c0`），静态值 = `0x08000000` = **134,217,728 = 128 MiB**。
全 `.text` 中对它**只有 1 处引用**（正是 `1403f2a9b`），且**无任何写入点**（语句级 grep 验证：无 `mov [rip+…], … # 0x141565cc0`）→ **它是只读常量**。

#### 结论公式（确证）

> **`TILE_MAX_HEIGHT = 0x08000000 / QImage::bytesPerLine()`**
> **每 tile 字节预算固定 128 MiB，tile 高度按画布宽度反算**（整数除法）。
> 仅当 `bytesPerLine > 134,217,728`（宽度 > 33,554,432 px）时结果 ≤ 0，才触发 `Invalid TILE_MAX_HEIGHT:` —— 纯防御性检查，实际不可达。

#### 各分辨率下的每 tile 高度（由公式导出）

| 画布宽 | `bytesPerLine`(32bpp) | **`TILE_MAX_HEIGHT`** | tile 实际字节 | 502,649 px 高图需 |
|---:|---:|---:|---:|---:|
| 1058 | 4,232 | **31,714** | 134,213,648 | 16 tiles |
| 1280 | 5,120 | **26,214** | 134,215,680 | 20 |
| 1366 | 5,464 | **24,564** | 134,217,696 | 21 |
| 1440 | 5,760 | **23,301** | 134,213,760 | 22 |
| 1920 | 7,680 | **17,476** | 134,215,680 | 29 |
| 2560 | 10,240 | **13,107** | 134,215,680 | 39 |
| 3440 | 13,760 | **9,754** | 134,215,040 | 52 |
| 3840 | 15,360 | **8,738** | 134,215,680 | 58 |
| 5120 | 20,480 | **6,553** | 134,205,440 | 77 |

#### 三个子问题的回答

- **tile 宽度是否等于画布宽度 → 确证：是。** 全部 tile API 的参数**只有高度轴一维**，无任何 x / column / width 参数：
  `contactImage(startIndex, putSize, gap, addedHeight, headInsertSize, tileMaxHeight)`、`PixLongImageTile::putImage(newImage, putSize, startIndexInNewImage, mMaxHeight)`、`removeHead/removeTail`、`toImage(startIndex, endIndex)`，错误串中只有 `imageHeight`/`copiedHeight`/`expectedHeight`/`currentEndIndex`。
  两条独立佐证：① `[PixLongImageFileEncodeThread::prepareTileImages] Failed to rotate tile image for horizontal output.` + `Rotated tile height mismatch:` → 输出**横向**长图靠把整条 tile **旋转 90°**（只有"整宽条带"才能靠旋转变成整高条带）；② `Inserted image is completely before/after the tile.` → 判定是沿单一轴的前后关系。
  → **tile = 全宽水平条带，索引沿高度轴一维推进。**
- **`prepareTileImages` 附近缓冲尺寸**（强推断）：`Missing tile data at row:`、`Row index is out of range:`、`Scanline buffer overflow, offset:  copyBytes:`、`Scanline width mismatch, actualBytes:… expectedBytes:`、`Image channel mismatch, image bytesPerLine:  width:  channel:` → 编码线程按 **scanline（单行）** 粒度搬运并按 `bytesPerLine` 校验，**不是整图缓冲** —— 与"每 tile 128 MiB"预算自洽。
- **4096–1,048,576 量级 `mov r32, imm32` 候选 → 无法可靠归属（未命中）**。对 `PixPin.exe` `.text` 全量扫描 `B8+rd imm32` / `41 B8+rd imm32` / `C7 /0 imm32`：
  ```
  mov-imm32 in [0x1000, 0x100000] -> 2355 distinct values, 14315 sites
  ```
  但**频次最高者全是解码失步的假阳性**（`0x9024`×567、`0x8024`×525、`0xa024`×365…，低字节恒为 `0x24`，即 SIB/ModRM 字节被误读为立即数）。成整幂的候选（4096×67、8192×40、16384×125、32768×139、65536×42、131072×10、262144×13、524288×5、1048576×16）**均无法排除是 CRT/Qt/OpenCV 通用常量**。
  → **不要从这条线索反推 tile 尺寸**；权威来源是上面的 `0x08000000 / bytesPerLine` 公式。
  `Config\PixPinConfig.json` 中**无任何** `Tile`/`Max`/`Limit`/`Height` 键 → **tile 尺寸不是用户可配置项，是编译期常量**（确证）。

#### 与你 `INT_MAX/4` 假说的关系（机制已对上一半）

官方实测产物 `1058 × 502,649 = 531,802,642 px`（32bpp = 2,127,210,568 B）在**本轮 tile 公式**下恰好需要 **16 个 tile**：
```
TILE_MAX_HEIGHT(1058) = 134217728 / 4232 = 31,714
ceil(502649 / 31714)  = 16
16 × 128 MiB          = 2,147,483,648 = 0x80000000 = 2^31
```
→ **16 tiles × 128 MiB 恰好等于 `2^31` 字节**，而实测产物占 `INT32_MAX` 的 **99.06%**。

**这使你的假说从"数值巧合"升级为"有机制支撑的强推断"**：PixPin 的尺寸管理**整体是"字节预算 ÷ `bytesPerLine`"的风格**（tile 级已确证）；故"总上限 ≈ 2 GiB ÷ `bytesPerLine`"在**设计风格上完全一致**。
但**总上限判据本身仍未定位**（第一轮 6 种手段全失败，本轮未再投入）→ **仍标为强推断，不是确证。**

> **对 SnapClip 的建议（请写进评审报告）**
> **不要反推 PixPin 的总上限精确值。** 应**定义自己的显式常量**（如 `MAX_LONG_IMAGE_PIXELS`，或更贴合本证据风格的 `LONG_IMAGE_TILE_BYTE_BUDGET`），并把长图**强制 tile 化**。
> 现在有了**有对标依据的 tile 取值**：PixPin 每 tile 预算 **128 MiB 字节**，等价于 **`tile 高度 = 134217728 / (画布宽 × 每像素字节数)`**。
> 若想更保守/更省内存，可取 32 MiB 或 64 MiB（PixPin 的 1/4 ~ 1/2）再反算 tile 高度 —— 既有对标依据，又不必照抄其量级。**这比原先"拍的 512×512"有实质改进**；且务必注意 **PixPin 的 tile 宽度恒等于画布宽度**（不是正方形块）。

### 10.3 `SimulateMouseScroll` / `SimulateMouseWheel` 语义与注入路径 —— **确证，且二者机制不同**

`PixSystemUtils.dll` 导出 RVA（llvm-readobj）：`?SimulateMouseScroll@@YAX_N0H@Z` = **Ordinal 26, RVA `0x102A0`**；`?SimulateMouseWheel@@YAXVQPoint@@H@Z` = **Ordinal 27, RVA `0x10300`**。
（`PixSystemUtils.dll`：ImageBase `0x180000000`、`.text` va=`0x180001000` ro=`0x400` → `VA = RVA + 0x180000000`。）

#### (a) `SimulateMouseScroll(bool, bool, int)` —— **纯 `SendInput`，无 PostMessage**

```asm
1800102a0:  sub  rsp,0x58
1800102a4:  xor  eax,eax
1800102a6:  xorps xmm0,xmm0
1800102a9:  mov  QWORD PTR [rsp+0x24],rax       ; INPUT.mi.dwExtraInfo/time = 0
1800102b2:  mov  DWORD PTR [rsp+0x20],eax       ; INPUT.type = 0 (INPUT_MOUSE)
1800102b6:  movdqu XMMWORD PTR [rsp+0x38],xmm0
1800102bc:  test cl,cl                          ; <<< bool #1
1800102be:  je   0x1800102ce
1800102c0:  mov  DWORD PTR [rsp+0x34],0x800     ; cl!=0 -> MOUSEEVENTF_WHEEL  (垂直)
1800102c8:  test dl,dl                          ; <<< bool #2
1800102ca:  jne  0x1800102da
1800102cc:  jmp  0x1800102dd
1800102ce:  mov  DWORD PTR [rsp+0x34],0x1000    ; cl==0 -> MOUSEEVENTF_HWHEEL (水平)
1800102d6:  test dl,dl
1800102d8:  jne  0x1800102dd
1800102da:  neg  r8d                            ; dl==0 -> 取反（反向滚）
1800102dd:  mov  DWORD PTR [rsp+0x30],r8d       ; INPUT.mi.mouseData = amount
1800102e2:  lea  rdx,[rsp+0x20]                 ; &INPUT
1800102e7:  mov  r8d,0x28                       ; cbSize = sizeof(INPUT) = 40
1800102ed:  mov  ecx,0x1                        ; cInputs = 1
1800102f2:  call QWORD PTR [rip+0x53d0]         ; -> IAT 0x1800156c8
1800102f8:  add  rsp,0x58
1800102fc:  ret
```
IAT 解析：**`0x1800156c8 -> USER32.dll :: SendInput`**（确证）

**语义（确证）**

| 形参 | 寄存器 | 含义 |
|---|---|---|
| `bool` #1 | `cl` | **轴向**：`true` → **垂直滚轮** `MOUSEEVENTF_WHEEL (0x800)`；`false` → **水平滚轮** `MOUSEEVENTF_HWHEEL (0x1000)` |
| `bool` #2 | `dl` | **方向**：`true` → 直接用 `amount`；`false` → `amount = -amount`（反向） |
| `int` | `r8d` | 滚轮增量（`mouseData`） |

- `INPUT.type = 0` = `INPUT_MOUSE`；`cbSize = 0x28 = 40`（x64 `sizeof(INPUT)`）；`cInputs = 1`。
- **该函数路径内不存在任何 `PostMessage`/`SendMessage`** → 100% `SendInput`（确证）。
- `MOUSEEVENTF_WHEEL`/`HWHEEL` 之所以此前"字符串未命中"：它们是**立即数**，现已由反汇编直接读出。

#### (b) `SimulateMouseWheel(QPoint, int)` —— **`WindowFromPoint` → `ScreenToClient` → `PostMessageW(WM_MOUSEWHEEL)`；无窗口则仅记日志**

```asm
18001030f:  mov   edi,edx                       ; edi = int delta
180010311:  mov   DWORD PTR [rsp+0x70],ecx      ; QPoint.x
180010315:  shr   rcx,0x20
180010319:  mov   DWORD PTR [rsp+0x74],ecx      ; QPoint.y
18001031d:  mov   rcx,QWORD PTR [rsp+0x70]
180010322:  call  QWORD PTR [rip+0x5470]        ; -> IAT 0x180015798  USER32!WindowFromPoint(pt)
180010328:  mov   rbx,rax
18001032b:  test  rax,rax
18001032e:  jne   0x1800103a4                   ; 命中窗口 -> PostMessage 分支
; ---- hwnd == NULL 分支：仅构造 QDebug 警告后返回，不注入任何事件 ----
180010330:  xor   r9d,r9d / xor r8d,r8d / xor edx,edx
180010338:  lea   rcx,[rsp+0x30]
18001033d:  call  QWORD PTR [rip+0x5105]        ; -> IAT 0x180015448  QMessageLogger::QMessageLogger
18001034b:  call  QWORD PTR [rip+0x50ff]        ; -> IAT 0x180015450  QMessageLogger::warning()
180010352:  lea   rdx,[rip+0x6307] # 0x180016660; 警告文本
18001035c:  call  QWORD PTR [rip+0x51fe]        ; -> IAT 0x180015560  QDebug::operator<<(char const*)
18001036a:  inc   DWORD PTR [rcx+0x18]          ; QByteArray 引用计数（Qt 隐式共享）
18001038e:  ... IAT 0x180015558 = QDebug::~QDebug
18001039e:  add   rsp,0x50 / pop rdi / ret
; ---- hwnd != NULL 分支 ----
1800103a4:  lea   rdx,[rsp+0x70]                ; &pt
1800103a9:  mov   rcx,rbx                       ; hwnd
1800103ac:  call  QWORD PTR [rip+0x53de]        ; -> IAT 0x180015790  USER32!ScreenToClient(hwnd,&pt)
1800103b2:  movzx r9d,WORD PTR [rsp+0x74]       ; pt.y
1800103b8:  shl   r9d,0x10                      ; y << 16
1800103bc:  movzx eax,WORD PTR [rsp+0x70]       ; pt.x
1800103c1:  or    r9,rax                        ; lParam = MAKELPARAM(x, y)
1800103c4:  movzx eax,di                        ; delta
1800103c7:  imul  ecx,eax,0x78                  ; delta * 120   (0x78 = 120 = WHEEL_DELTA)
1800103ca:  movzx r8d,cx
1800103ce:  shl   r8,0x10                       ; wParam = (delta*120) << 16
1800103d2:  mov   edx,0x20a                     ; 0x20A = WM_MOUSEWHEEL
1800103d7:  mov   rcx,rbx                       ; hwnd
1800103da:  call  QWORD PTR [rip+0x5330]        ; -> IAT 0x180015710  USER32!PostMessageW(hwnd,msg,wParam,lParam)
1800103e0:  add   rsp,0x50 / pop rdi / ret
```
IAT 解析（**全部确证**）：
```
0x180015798 -> USER32.dll :: WindowFromPoint
0x180015790 -> USER32.dll :: ScreenToClient
0x180015710 -> USER32.dll :: PostMessageW
0x1800156c8 -> USER32.dll :: SendInput          (仅 SimulateMouseScroll 使用)
0x180015448 -> Qt5Core.dll :: QMessageLogger ctor
0x180015450 -> Qt5Core.dll :: QMessageLogger::warning()
0x180015560 -> Qt5Core.dll :: QDebug::operator<<(char const*)
0x180015558 -> Qt5Core.dll :: QDebug::~QDebug
```

**语义与分支条件（确证）**

| 条件 | 行为 |
|---|---|
| `WindowFromPoint(pt) != NULL` | `ScreenToClient(hwnd, &pt)` → **`PostMessageW(hwnd, WM_MOUSEWHEEL (0x20A), MAKEWPARAM(0, delta×120), MAKELPARAM(clientX, clientY))`** |
| `WindowFromPoint(pt) == NULL` | **不注入任何输入**；仅 `QMessageLogger::warning()` 记一条警告后返回 |

要点：
- `wParam = (delta × 120) << 16`，`0x78 = 120 = WHEEL_DELTA` —— 与 Win32 `WM_MOUSEWHEEL` 契约严格一致（高字为带符号滚动量，低字为按键状态，此处为 0）。
- `lParam = MAKELPARAM(clientX, clientY)` —— **先 `ScreenToClient` 再打包，这是正确做法**（`WM_MOUSEWHEEL` 要求**客户区**坐标，而 `WindowFromPoint` 给的是屏幕坐标）。
- **`SimulateMouseWheel` 路径内没有 `SendInput`**；**`SimulateMouseScroll` 路径内没有 `PostMessage`**（均确证）。

#### (c) 更正与对标结论

**更正第一轮表述**：§4.6 / §6.5 中"`SendInput` 优先、`PostMessage(WM_MOUSEWHEEL)` 兜底"**不成立**。真实设计是**按用途分工的两个独立函数，互不降级**：

| 函数 | 用途（由形参推断） | 机制 | 目标定位 |
|---|---|---|---|
| `SimulateMouseScroll(bool 轴向, bool 方向, int 步长)` | **自动滚动 / 长截图自动滚屏**（只需方向与步数，不需坐标） | **`SendInput` + `MOUSEEVENTF_WHEEL`/`HWHEEL`** | 送到**当前前台/光标所在**窗口（系统级注入，走正常输入队列） |
| `SimulateMouseWheel(QPoint 屏幕坐标, int delta)` | **对"某个具体位置的窗口"发一次滚轮**（需坐标） | **`PostMessageW(hwnd, WM_MOUSEWHEEL)`** | 由 `WindowFromPoint` 精确指定 **hwnd**，并已转为客户区坐标 |

**对 SnapClip「自动滚轮注入路径对标」的写法建议（有依据）**
1. **自动滚动主路径用 `SendInput` 合成真实滚轮事件** —— 与 PixPin 一致。这是穿透 Chromium/UWP/自绘 UI 的关键，因为 `SendInput` 走系统输入队列，会被 `GetMessage` 与低层钩子正常看到。
2. **需要"定点发给某窗口"时用 `PostMessageW(hwnd, WM_MOUSEWHEEL, delta*120<<16, MAKELPARAM(clientX, clientY))`** —— 与 PixPin 一致。**务必先 `ScreenToClient`**，否则很多应用会因坐标不匹配而忽略或滚错。
3. **不要把它写成"fallback 关系"。** PixPin 的实际证据是**两条并列路径**：`SendInput`（按方向自动滚）与 `PostMessage`（按坐标点发）。若 SnapClip 想做"`SendInput` 失败则退 `PostMessage`"的兜底，那是**超出 PixPin 行为的增强**，应明确标注为 SnapClip 自己的设计，而非对标 PixPin。
4. PixPin 的 `PostMessage` 路径**在 `WindowFromPoint` 返回 NULL 时直接放弃**（只打日志）。SnapClip 可能需要改进为"回退到 `SendInput`"，因为空点场景（光标在桌面空白处、或 `WindowFromPoint` 被 UIPI 阻挡）实际会发生。

### 10.4 本轮对 §9 未解疑点清单的更新

| # | 原疑点 | 本轮结果 |
|---|---|---|
| 6 | `PixPinAuxiliary.exe` 职责未知 | **已解（确证）**：提权代理（`ShellExecuteExW`+`runas`）+ 崩溃重启器（10 min 限流）+ 独立更新器（`Global\PixPinUpdater` 互斥 + zip/exe 双路径 + bcrypt 校验）。**不参与捕获**（未命中）。 |
| 1 | "最大拼接范围"上限常量未定位 | **部分推进**：tile 级预算已确证（`0x08000000`/`bytesPerLine`，单读点、无写点）；且 16 tiles × 128 MiB = `2^31` 与官方产物 99.06% 吻合 → 假说升级为**有机制支撑的强推断**。**总上限判据本身仍未定位。** |
| 2 | `TILE_MAX_HEIGHT` 数值未解 | **已解（确证）**：**不是固定高度**，而是 `134217728 / QImage::bytesPerLine()`；1058 px 宽 → 31,714 px/tile。 |
| 3 | `model\*.bin` 容器格式 | 仍未解。**新增关联线索**：`PixPinAuxiliary.exe` 导入完整 `bcrypt` 栈（PBKDF2 + 对称加密 + 哈希），`PixPin.exe` 亦导入 `bcrypt.dll` → 该私有容器很可能就用这套。 |
| 12 | 超长模式下 OCR/Print 是否真的禁用 | 仍未确证；已确证 `jpg` 被显式拒绝、`pin` 被禁用。 |

**本轮方法学要点（值得沉淀）**
1. **RIP-relative 精确地址匹配会失败**：MSVC `/GF` 字符串池会合并相同后缀，代码引用的地址常落在池内**相邻/偏移**位置（实测 delta 从 -151 到 +123）。**正确做法是"锚点区间扫描"**：解析所有 `disp32` 得目标 VA，判定其是否落入 `[字符串偏移-200, +128]`。
2. **IAT 槽位解析是把反汇编变成结论的关键一步**：仅凭调用约定猜 `SendInput`/`PostMessageW` 不可靠；把 IAT slot RVA 映射回 `dll :: symbol` 后，`0x800`/`0x1000`/`0x78`/`0x20A` 这些立即数的含义才闭合。
3. **`mov r32, imm32` 全量扫描在大型 x86-64 二进制上信噪比极低**（本轮假阳性率 >99%，低字节 `0x24` 是失步特征指纹）。**不要用它做常量归属结论**；优先走"定位引用点 → 反汇编 → 读操作数"的路径。
4. **`.data` 里的常量要先验证"是否可写/是否被写"**：本轮用"全 `.text` 引用计数 + 写入点 grep"证明 `0x141565cc0` 是只读常量（1 读点、0 写点），才敢把 `0x08000000` 当作真常量而非运行时初始化值。
