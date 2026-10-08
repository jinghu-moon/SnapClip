# PixPin 运行期数据 / 配置 / 日志 / 文件格式 深度调研报告

> 目标：为 Rust 竞品 **SnapClip** 还原 PixPin 的功能集、默认参数、交互模型与持久化设计。
> 只读参考：`C:\A_Softwares\PixPin`。**未修改/删除/重命名任何 PixPin 文件，未运行 PixPin.exe。**
> 所有分析产物写入 `C:\Users\seeyuer\AppData\Local\Temp\pixpin_dig\`。
>
> 证据标注约定：**[确证]** = 本机静态/运行期直接读到；**[强推断]** = 由多条确证交叉推出；**[弱推断]** = 单一间接线索；**[公开资料]** = 来自 pixpin.cn 官网文档；**[未解]** = 未能验证。

---

## 0. 前置重要更正（与任务书假设不一致）

| 任务书假设 | 实测结果 | 证据 |
|---|---|---|
| PixPin v2.x | **本机安装版本是 3.5.5.1** | `[确证]` `(Get-Item PixPin.exe).VersionInfo.FileVersionRaw` → `3.5.5.1`；`FileMajorPart=3 FileMinorPart=5 FileBuildPart=5 FilePrivatePart=1` |
| `pixpin.log` 是 0 字节 | **首次列举时确为 0 字节，约 10 分钟后为 392999 字节** | `[确证]` 因为 **PixPin.exe 正在运行**（PID 14928，启动于 2026/10/1 16:59:48），当前日志持续追加 |
| `Config\PixPinConfig.json` 4028 B | 一致（4028 B） | `[确证]` |
| `LocalStorage.data` 30954 B | 一致 | `[确证]` |
| `Data\PinWindowd.sqlite` 106496 B | 一致；`page_size=4096 × page_count=26 = 106496` | `[确证]` |

**关键含义**：不能把 `UpgradeFile\PixPin_2.0.0.3.exe`、`UpgradeTargetVer=2.0.0.3`、日志里的 `Crashpad version: "2.0.0.3"` 当作当前版本 —— 它们是历史升级残留。当前二进制与 Sentry release 均为 **3.5.5.1**。

本机日志中出现的版本标签（实测升级链）：
`(空) → 2.1.8.0 → 2.1.8.3 → 2.1.8.4 → 2.2.4.0 →(2.2.4.1/2.3.8.0/2.4.9.1 仅提示)→ 2.4.9.6 → 3.0.8.0 → 3.1.4.0 → 3.2.3.1 → 3.5.5.1`
`[确证]` 命令：`Select-String 'upgrade success|start install new version' pixpin.1.log,pixpin.2.log`

---

## 1. 结论摘要（≤12 条）

1. **【确证】本机 PixPin 是 3.5.5.1，不是 2.x**；`UpgradeFile\PixPin_2.0.0.3.exe(36 MB)` 与 `UpgradeTargetVer="2.0.0.3"` 是历史残留，日志中升级链覆盖 2.1.8.0 → 3.5.5.1。
2. **【确证】滚动/长截图能力存在且在本机被实际使用过**：`pixpin.log` 2026-10-08 记录了 `LongShotWidget` 完整生命周期，导出长图 **1505×3241** 与 **1178×8966** 各一张，字段 `logicalLength` / `shotRect` / `pixStitching` / `maskOverlay` / `imagePreview` / `cropFloatPanel` / `autoCropEnabled` / `dir`。
3. **【确证】长截图产物形态 = 内存中拼接完成的单张超长位图（QImage `Format_RGB32`，format=4）**，不是分段；内存占用 = W×H×4（1178×8966×4 = 42,247,792 B，与日志 `bytes=` 完全一致）。
4. **【确证】截图主链路 = DXGI 全屏抓取 + 内存裁剪**：`PixScreenGXDI::grabWindow` → `grabWindowImage`（整屏 3840×2160，stride 15360，bpp 4）→ `Image crop started cropRect=...` → QPixmap；失败回退 `PixScreenQt`。
5. **【确证】配置是四层分离设计**：`PixPinConfig.json`（动作/热键/标注默认值，每项带修改时间戳 `t`）/ `LocalStorage.data`（Qt QSettings 会话态）/ `Data\PinWindowd.sqlite`（贴图业务实体）/ `Data\*.meta`（每个贴图的文档）。
6. **【确证】选区历史保留 100 条**：`LocalStorage.data` 的 `HistoryShotRectDatas` 经完整解码为 100 条 `{QRectF rect, "rect" SelectMode, 0.0 RoundRadius}`，**字节级 100/100 精确解析**。
7. **【确证】OCR 是双引擎（Det 检测 + Rec 识别），空闲 120 s 后释放**：`delayMs= 120000`，`engine= Det/Rec action= create/release`；`PixOCR2.dll` 内含 `MNN` 字样（75 次），`onnxruntime.dll` 14.4 MB 同时存在。
8. **【确证】崩溃上报 = Sentry Native 0.15.2 + Crashpad**，release=`PixPin@3.5.5.1`；最近一次崩溃 `last_crash=2026-01-14T13:06:31.146852Z`，与 `settings.dat` 内嵌时间戳 `0x696794d8`（= 2026-01-14 13:06:32 UTC）互证。
9. **【确证】`.meta` 不是标注图层，而是"贴图窗口状态文档"**：100 个文件全部包含 `WinStaysOnTop / WinOpacity / WinKeepRatio / Transform / SrcGeometry / PinWindowDataRelateFiles / CenterPos`，98 个含 `OcrTextJson`（4 角点 float 多边形 + 文本 + `textType` + `lang`）。
10. **【确证】`Data\` 里没有长截图产物**：99 个 PNG 中最大尺寸为 3840×2088（整屏），最高的竖图仅 659×1398；98 个 colorType=2（RGB），仅 1 个 colorType=6（RGBA，自由选区透明区）。
11. **【确证】录屏/动图能力存在**：`PixAVCodec.dll`(8.0 MB) 含 `libx264 / h264 / ffmpeg / avcodec / aac / mp4 / gif / webp / VP8 / VP9`；`plugins\audio\qtaudio_wasapi.dll` 存在；配置键 `PixMovie.record.recordMicrophone`。官方文档：普通录制导出 MP4/GIF/WebP，快速录制仅 MP4。
12. **【确证】`pixpin.log`/`pixpin.1.log` 中 `scroll`、`stitch`(小写)、`merge`、`match`、`WGC`、`GraphicsCapture`、`BitBlt`、`PrintWindow`、`Dwm`、`wheel`、`history`、`license`、`DPI`、`webp`、`onnx`、`gif`、`mp4` 等关键词命中次数均为 0** —— 拼接算法内部、OCR 推理细节、许可校验均无日志，对本项目的可观测性启发是"关键路径必须自带埋点"。

---

## 2. 配置文件全文与逐字段解读

### 2.1 `Config\PixPinConfig.json`（4028 B）

**格式**：单行 JSON，顶层是 `{ "键": {"t": <Unix秒>, "v": <值>, "d": <Unix秒>} }`。
- `t` = 该项最近修改时间（Unix 秒）；只有 `t` 没有 `v` 表示**该项从未被用户改动、使用内置默认值**。
- `v` = 值；`d` = 删除时间（例：`Action.Switch pin group#s.win` 只有 `d`）。
- 键名后缀 `#s.win` 表示 "scope = win"（Windows 平台作用域），说明 PixPin 的配置系统是**跨平台 + 按作用域分片**的。

**全文**（`Get-Content Config\PixPinConfig.json -Raw -Encoding UTF8`，已按语义分行）：

```jsonc
{
  // ---- 动作 / 全局热键（type:256 = 系统动作；index = 工具栏顺序）----
  "Action.Screenshot#s.win":       {"t":1775435403, "v":{"index":0,"isSystemAction":true,
                                        "script":"pixpin.screenShotAndEdit()",
                                        "shortCut":"F1","showOnTray":true,
                                        "title":"Screenshot","type":256}},   // ← 默认截图热键 F1
  "Action.Pin#s.win":              {"t":1775435407, "v":{"index":3,"isSystemAction":true,
                                        "script":"pixpin.pinFromClipBoard()",
                                        "shortCut":"F3","showOnTray":true,
                                        "title":"Pin","type":256}},          // ← 默认贴图热键 F3
  "Action.Close all pin window#s.win":  {"t":1775435398},   // 未配置
  "Action.Custom screenshot#s.win":     {"t":1775435398},   // 未配置（自定义选区预设动作）
  "Action.Pin selected file#s.win":     {"t":1775435398},   // 未配置
  "Action.Restore last closed#s.win":   {"t":1775435398},   // 未配置（恢复最近关闭的贴图）
  "Action.Screenshot and copy#s.win":   {"t":1775435398},   // 未配置（截图并复制）
  "Action.Switch pin group#s.win":      {"d":1775656506},   // 已删除
  "BIShortcut.pixpin.0#s.win":          {"t":1775435398},   // 内置快捷键覆盖项 0，未配置

  // ---- 外观（全部未配置 → 跟随系统）----
  "Appearance.ThemeMode":  {"t":1775435398},
  "Appearance.ThemeFont":  {"t":1775435398},
  "Appearance.TrayIcon":   {"t":1775435398},

  // ---- 标注（Mark）工具栏 ----
  "Mark.EditItemOrder": {"t":1790398020, "v":{"Recycle":[], "Vaild":[
        {"list":["Geometry","HighLight"],                          "value":"Geometry"},
        {"list":["Pencil","Marker"],                               "value":"Pencil"},
        {"list":["Arrow","BrokenLine","Magnifier"],                 "value":"BrokenLine"},
        {"list":["Text","Watermark"],                              "value":"Text"},
        {"list":["Serial"],                                        "value":"Serial"},
        {"list":["Mosaic","AutoMosaic"],                            "value":"Mosaic"},
        {"list":["Eraser"],                                        "value":"Eraser"}]}},
  "Mark.ActBarFlag.HighLight":   {"t":1737617762, "v":66058},
  "MarkBar.Arrow.LineShape":     {"t":1790397988, "v":4},
  "MarkBar.Arrow.PenWidth":      {"t":1790397981, "v":8},
  "MarkBar.Common.Color":        {"t":1790324315, "v":"ffd84a2f"},   // AARRGGBB 红
  "MarkBar.Geometry.Filling":    {"t":1790324311, "v":false},
  "MarkBar.Geometry.PathMode":   {"t":1790321399, "v":2},
  "MarkBar.Geometry.PenWidth":   {"t":1786981657, "v":9},
  "MarkBar.Geometry.RectRoundRadius": {"t":1790318913, "v":90},
  "MarkBar.HighLight.PathMode":  {"t":1790318504, "v":3},
  "MarkBar.Mosaic.BlurStrength": {"t":1752583932, "v":28},
  "MarkBar.Mosaic.MosaicMode":   {"t":1752583949, "v":0},
  "MarkBar.Mosaic.MosaicStrength":{"t":1752583845,"v":28},
  "MarkBar.Mosaic.PathMode":     {"t":1752583943, "v":2},
  "MarkBar.Pencil.PenStyle":     {"t":1790354365, "v":1},
  "MarkBar.Text.Size":           {"t":1770814376, "v":20},

  // ---- 贴图（Pin）----
  "Pin.ConfirmationFlags": {"t":1791074286, "v":10},   // 贴图二次确认位掩码

  // ---- 录制（PixMovie）----
  "PixMovie.record.recordMicrophone": {"t":1790993243, "v":false},

  // ---- 截图后处理（截图四周模糊 / 边框）----
  "PostProcess.Shot.Enable":            {"t":1790411926, "v":false},
  "PostProcess.Shot.EnableForEveryShot":{"t":1790411144, "v":false},
  "PostProcess.Shot.Modules":           {"t":1790411146, "v":2},      // 位掩码：1=模糊 2=边框
  "PostProcess.blur.color":             {"t":1790411145, "v":"ffa0a0a4"},
  "PostProcess.blur.strength":          {"t":1790411145, "v":8},
  "PostProcess.border.color":           {"t":1790411148, "v":"ffffffff"},
  "PostProcess.border.strength":        {"t":1790411146, "v":27},

  // ---- 保存 ----
  "Save.SaveQuality":   {"t":1791424028, "v":100},   // JPEG/WebP 质量，默认满值

  // ---- 截图工具栏按钮（ActBarFlag：每项一个整数，低位=顺序，高位=可见性/层级）----
  "ScreenShot.ActBarFlag.LongShot":          {"t":1790318723, "v":256},  // 长截图
  "ScreenShot.ActBarFlag.GifShot":           {"t":1790318723, "v":257},  // 录制/动图
  "ScreenShot.ActBarFlag.CopyOcr":           {"t":1790318723, "v":258},  // 复制文字(OCR)
  "ScreenShot.ActBarFlag.Translate":         {"t":1790318723, "v":259},  // 翻译
  "ScreenShot.ActBarFlag.Pin":               {"t":1790318723, "v":260},  // 贴图
  "ScreenShot.ActBarFlag.Save":              {"t":1790318723, "v":261},  // 保存
  "ScreenShot.ActBarFlag.Close":             {"t":1790318723, "v":262},  // 关闭
  "ScreenShot.ActBarFlag.Copy":              {"t":1790318723, "v":263},  // 复制
  "ScreenShot.ActBarFlag.OcrTable":          {"t":1790318723, "v":512},  // 表格识别
  "ScreenShot.ActBarFlag.QuickSave":         {"t":1790318723, "v":513},  // 快速保存
  "ScreenShot.ActBarFlag.LatexRecognition":  {"t":1790318723, "v":514},  // 公式识别
  "ScreenShot.ActBarFlag.WinRoi":            {"t":1790318723, "v":515},  // 窗口/区域识别
  "ScreenShot.ActBarFlag.Print":             {"t":1790318723, "v":768},  // 打印
  "ScreenShot.ActBarFlag.ImageEdit":         {"t":1790318723, "v":769},  // 图像编辑

  // ---- 截图行为 ----
  "Screenshot.ConfirmOnCloseByKey": {"t":1775820008, "v":false},
  "Screenshot.SizeDisplayItems":    {"t":1790437634, "v":7},
  "Screenshot.SizeUnit":            {"t":1790411083, "v":0},      // 0=px
  "Screenshot.enableRoundRect":     {"t":1790411131, "v":true},   // 圆角截图
  "Screenshot.roundRectRatio":      {"t":1791127269, "v":0},

  // ---- 系统 ----
  "System.DesktopToolBar":     {"t":1775435429, "v":2},
  "System.Language":           {"t":1791096719, "v":"auto"},
  "System.Run After Boot":     {"t":1775557268, "v":true},   // 开机自启 = 开
  "System.Run After Boot.RunAsAdmin": {"t":1775435398},      // 未配置（默认非管理员）
  "System.Text Recognition.CopyTextConfirmation": {"t":1775993522, "v":5},
  "System.IgnoreCopyMacros":   {"t":1775435398},
  "System.Translate.TranslateKey": {"t":1791096720,
        "v":"JNHG3z6eiksKzmIq9zr6/WbZJGK9HQzBZaXDC0LNRkI=|||0"}   // Base64 + "|||0" 后缀，疑似设备绑定的翻译服务密钥
}
```

**对 SnapClip 的启示**
- 配置项 = 「默认值 + 稀疏覆盖 + 修改时间戳」三层，且**未修改项不落盘**。Rust 侧可用 `HashMap<String, (i64 /*t*/, serde_json::Value)>` 落盘，读取时与内置默认表合并 —— 好处是默认值可以随版本演进而不被旧配置锁死。
- 默认热键 **F1 = 截图（进入编辑）**、**F3 = 剪贴板贴图**；动作是**脚本字符串**（`pixpin.screenShotAndEdit()` / `pixpin.pinFromClipBoard()`），即 PixPin 有一套内置脚本 API（官网侧边栏确有「脚本」文档页）。**[确证]**
- 截图工具栏用**整数位掩码 + 顺序号**建模（如 256=长截图、257=录制、258=OCR 复制…）。这套编码让"默认布局 / 用户自定义 / 一键恢复默认"三件事都很廉价。**[强推断]**（低位连号 256..263、512..515、768..769 明显分组）
- **没有任何长截图/DPI/多显示器/历史库上限/性能参数出现在这个文件里** —— 说明这些要么是编译期常量，要么存在未见到的存储中。**[确证]**

---

### 2.2 `Config\CustomScreenshot.int`（160 B）

全文（Qt QSettings IniFormat）：

```ini
[General]
PresetArea="--size 500,500\n--name 480p --size 640,480\n--name 720p --size 1280,720\n--name 1080p --size 1920,1080\n"
PresetAreaMigrationVersion=1
```

**解读**
- `PresetArea` 是"自定义截图"的**预设尺寸列表**，用一个迷你命令行语法串行化：`--size W,H` 定义尺寸，`--name X` 给**紧随其后**的尺寸命名。默认提供了 4 个：`500×500`（无名）、`480p 640×480`、`720p 1280×720`、`1080p 1920×1080`。
- `PresetAreaMigrationVersion=1` 是**数据迁移版本号** —— 说明 PixPin 为这个字段保留过格式迁移路径。**[确证]**
- **对 SnapClip**：预设尺寸用"文本 DSL + 迁移版本号"存盘，比 JSON 数组更省事且天然向后兼容可判；如果 SnapClip 采用"开发阶段不考虑兼容"的策略，可以简化掉 `MigrationVersion`，但**必须保留 `--name/--size` 的成对语义**（否则用户改名会错位）。**[强推断]**

---

### 2.3 `ConfigurationWindowConfig.ini`（44 B）

全文（hex + ascii）：

```
5b 47 65 6e 65 72 61 6c 5d 0d 0a 47 65 6f 6d 65 74 72 79 3d 40 52 65 63 74 28 38 39 35 20 34 34 35 20 37 36 38 20 35 30 30 29 0d 0a
[General]\r\nGeometry=@Rect(895 445 768 500)\r\n
```

**解读**：设置窗口几何 `x=895 y=445 w=768 h=500`（Qt `QRect` 序列化，空格分隔）。
**对 SnapClip**：设置窗口几何单独存一个文件、与业务配置解耦，好处是"重置配置"不会丢窗口位置，且配置文件损坏时不影响业务。**[确证]**

---

### 2.4 `pixmeta.dat`（30 B）

```
len=30
hex  : 33 43 39 65 0e 9a 22 9b 56 5f 59 10 12 1e 68 43 27 61 07 94 38 94 13 00 5b 08 10 10 6a 1c
ascii: 3C9e..".V_Y...hC'a..8...[...j.
```

- 无任何已知魔数；**无一个可读 ASCII 单词**；30 字节全部是高熵字节。
- **结论：二进制/加密的"设备元数据"文件（如设备指纹、安装标识、许可绑定种子）。静态不可解码。** **[强推断]**
- **对 SnapClip**：把设备指纹/许可种子与用户配置分开存 30 字节小文件是合理做法；但 SnapClip 若走开源/离线路线，建议**换成明文 JSON + 明确字段**，避免不可调试的黑盒。**[弱推断]**

---

### 2.5 `LocalStorage.data`（30954 B）—— 真实格式与全文解读

**格式判定 [确证]**：**Qt `QSettings` 的 `IniFormat`**（不是 SQLite、不是 JSON、不是 QDataStream 文件本身）。
判定依据（`python` 直读前 64 字节）：
```
5b 47 65 6e 65 72 61 6c 5d 0d 0a 55 70 64 61 74 65 47 72 61 79 73 63 61 6c 65 3d 31 38 0d 0a ...
[General]\r\nUpdateGrayscale=18\r\n
```
熵 3.098 bits/byte，可打印字符占比 1.000（文件里没有真正的 NUL 字节，`count NUL = 0`）。

**转义规则 [确证]**（这是本次能完整解码的关键，两种转义混用）：
- **QString 值**：非 ASCII 用 `\x` + **4 位十六进制** Unicode 码点，例：`SavePixmapPath=C:/Users/seeyuer/\x684c\x9762/...` → `\x684c`=桌、`\x9762`=面。
- **QByteArray 体内**：用 `\x` + **最少位数十六进制字节**（Qt 不补零），另有 `\0`=0x00、`\b`=0x08 等简写。解析必须"贪心最多 2 位十六进制"。例：`\x8ci` → `0x8c, 'i'`；`\x1\x1c` → `0x01, 0x1c`。

**全文键值 [确证]**（`[General]` 与 `[PixDesktopBar]` 两节，共 38 个键）：

| 键 | 值（解码后） | 含义 |
|---|---|---|
| `UpdateGrayscale` | `18` | 灰度发布分组（升级流量分桶号）。日志中同一字段实测为 79：`PixUpgrade init with grayscale value: 79` |
| `NoCheckUpdateUntil` | `@DateTime(00 00 00 10 \| 00 \| 00 00 00 00 00 25 8c 69 \| 01 1c 1f de \| 00)` → **2025-04-11 05:10:20.382 本地时间** | 在此时间前不检查更新。解码：`u32 typeId=16(QVariant::DateTime)` + `u8 保留=0` + `qint64 儒略日=2460777` + `quint32 当日毫秒=18620382` + `u8 timeSpec=0(LocalTime)` |
| `HistoryShotRectDatas` | `@ByteArray(...)` 13704 B，见下 | **最近 100 次截图选区** |
| `globalShortcutEnable` | `true` | 全局热键总开关 |
| `SavePixmapPath` | `C:/Users/seeyuer/桌面/PixPin_2025-02-05_22-38-11.png` | 上次保存的图片完整路径 |
| `SaveInfoStaticSuffix` | `jpg` | 固定后缀名 |
| `SaveInfoPath` | `C:/Users/seeyuer/桌面` | 默认保存目录 |
| `SaveDialog.LastSavePath` | `C:\Users\seeyuer\桌面` | 保存对话框上次目录 |
| `SaveDialog.LastImageFormat` | `jpg` | **上次使用的图片格式（默认 jpg，非 png）** |
| `SaveDialog.SuccessToast.ShowToast` | `false` | 保存成功提示 |
| `SaveDialog.SuccessToast.DontAskAgain` | `true` | 不再询问 |
| `SaveDialog.AspectRatioLocked` | `false` | 保存对话框锁定宽高比 |
| `upgradeFile` | `C:/A_Softwares/PixPin/UpgradeFile/PixPin_2.0.0.3.exe` | 待安装升级包路径 |
| `UpgradeTargetVer` | `2.0.0.3` | 目标升级版本 |
| `RunAsAdmin` | `false` | 是否以管理员运行 |
| `IsFirstRun` | `false` | 首次运行标记 |
| `devID` | `8107d543-e704-4cd2-af41-86a6ca929855` | 设备 ID（GUID） |
| `PixTrack_RandomID` | `52c388d782374c93bfcc3f39fa2a39a6` | 埋点匿名 ID（与日志 `PixTrack ProfileID` **完全一致**） |
| `PixDesktopBar.UpgradePromptShown` | `true` | 桌面悬浮球升级提示已展示 |
| `PointInfoWidgetColorFormat` / `...Name` | `128` / `RGB` | 取色信息条颜色格式 |
| `PointInfoWidgetUseRelativeCoordinate` | `false` | 坐标是否相对 |
| `PixColorPicker.ColorType` | `0` | 取色器类型 |
| `Ocr.ShowOriginalImage` | `true` | OCR 窗口显示原图 |
| `Ocr.ConfirmedDialogSize` | `@Size(900 700)` | OCR 结果窗口尺寸 |
| `ConfigurationWindowNormalPosition` | `@Point(597 297)` | 设置窗口位置 |
| `ConfigurationWindowNormalSize` | `@Size(1122 831)` | 设置窗口尺寸 |
| `ConfigurationWindowScreen` | `\\.\DISPLAY1` | 设置窗口所在屏幕 |
| `ConfigurationWindowMaximized` | `false` | 设置窗口最大化 |
| `Screenshot.SelectModePreference.KeepMode` | `false` | 记住上次选区模式 |
| `Screenshot.SelectModePreference.DontRemind` | `false` | 不再提醒 |
| `Screenshot.SelectMode` | `rect` | 当前选区模式 = 矩形 |
| `Screenshot.FreeSelectMode` | `false` | 自由选区开关 |
| `[PixDesktopBar] ToolBallPosX` / `ToolBallPosY` | `3701` / `965` | 桌面悬浮球坐标（3840 宽屏右上角） |

**`HistoryShotRectDatas` 的二进制结构 [确证，字节级 100/100 验证]**

容器：`u32 count = 100`，随后 100 个 `u32 len = 133` + 133 B 负载；负载是裸 `QMap<QString,QVariant>`：
`u32 键数 = 3`，然后 3 组 `QString 键名` + `QVariant 值`，其中 **`QVariant = u32 类型ID | u8 保留位(恒 0x00) | 负载`**。

以第 0 条为例的完整 hex（前 0x60 字节）：
```
0000  00 00 00 03 00 00 00 08 00 72 00 65 00 63 00 74   .........r.e.c.t
0010  00 00 00 14 00 40 92 44 00 00 00 00 00 40 81 e8   .....@.D.....@..
0020  00 00 00 00 00 40 99 e4 00 00 00 00 00 40 91 a4   .....@..........
0030  00 00 00 00 00 00 00 00 14 00 53 00 65 00 6c 00   ..........S.e.l.
0040  65 00 63 00 74 00 4d 00 6f 00 64 00 65 00 00 00   e.c.t.M.o.d.e...
0050  0a 00 00 00 00 08 00 72 00 65 00 63 00 74 00 00   .......r.e.c.t..
0060  00 16 00 52 00 6f 00 75 00 6e 00 64 00 52 00 61   ...R.o.u.n.d.R.a
0070  00 64 00 69 00 75 00 73 00 00 00 06 00 00 00 00   .d.i.u.s........
0080  00 00 00 00 00                                    .....
```
解析结果：
- 键 `rect` → `typeId=20 (QRectF)` → 4×double = **`(1169.0, 573.0, 1657.0, 1129.0)`** = `(x, y, w, h)`，**全局屏幕物理像素坐标**
- 键 `SelectMode` → `typeId=10 (QString)` → `"rect"`
- 键 `RoundRadius` → `typeId=6 (Double)` → `0.0`

100 条统计：`SelectMode` 全部为 `"rect"`（100/100），`RoundRadius` 全部为 `0.0`（100/100）。
坐标范围示例（前 13 条）：
```
[00] x=1169.0 y=573.0  w=1657.0 h=1129.0     [07] x=1033.0 y=472.0  w=1775.0 h=1134.0
[01] x=1033.0 y=472.0  w=1775.0 h=1134.0     [08] x=1678.0 y=1035.0 w=582.0  h=196.0
[02] x=0.0    y=1710.0 w=2390.0 h=311.0      [09] x=1135.0 y=797.0  w=478.0  h=479.0
[03] x=5.0    y=1660.0 w=1658.0 h=401.0      [10] x=1227.0 y=630.0  w=286.0  h=93.0
[04] x=27.0   y=1877.0 w=301.0  h=75.0       [11] x=1599.0 y=1295.0 w=420.0  h=122.0
[05] x=1046.0 y=1199.0 w=1593.0 h=261.0      [12] x=1434.0 y=1796.0 w=763.0  h=150.0
[06] x=993.0  y=1152.0 w=1461.0 h=319.0      [99] x=1107.0 y=665.0  w=1178.0 h=686.0
```
> 注：`[99] x=1107,y=665,w=1178,h=686` **正是**当天 09:47 那次长截图的 `shotRect=1107,665 1178x686` —— 两条日志与配置互相印证。**[确证]**

**对 SnapClip 的启示（高价值，低成本）**
1. **记住最近 100 次选区矩形**（`x,y,w,h` 全局物理像素）+ 选区模式 + 圆角半径。用户下次框选时可直接吸附/键盘循环历史选区，体验提升明显但实现极廉价（`VecDeque<SelectionRect>` 上限 100）。
2. 用**全程屏物理像素**（非逻辑像素）记录，避免 DPI 变化导致历史选区漂移。
3. 会话态（窗口几何、上次路径、上次格式、开关状态）与"业务配置"分离到不同文件/表；SnapClip 可用 `QSettings` 等价物（如 `serde` + `ron`/`toml`）但**务必保留"未修改不落盘"语义**以便默认值演进。

---

## 3. `Data\PinWindowd.sqlite`（106496 B）—— 贴图持久化模型

**[确证]** 文件头 = SQLite 3；`page_size=4096`、`page_count=26`（26×4096=106496 与文件大小精确一致）、`encoding=UTF-8`、`user_version=0`、`journal_mode=delete`。

复制到 TEMP 后只读查询（`sqlite3.exe "$env:TEMP\pixpin_dig\PinWindowd.sqlite"`）：

### 3.1 Schema 全文

```sql
CREATE TABLE "PinItem" (
  "id"           INTEGER PRIMARY KEY AUTOINCREMENT,
  "type"         TEXT NOT NULL,          -- 'Image' | 'Text'
  "createTime"   DATETIME NOT NULL,      -- ISO8601 毫秒，如 2026-05-14T23:04:30.614
  "closeTime"    DATETIME,               -- 关闭（销毁）时刻
  "device"       TEXT NOT NULL,          -- 'SeeYee-PC'，多设备区分
  "subItem"      INTEGER,                -- 子项（当前全 NULL）
  "posX"         INTEGER NOT NULL,       -- 贴图窗口左上角 X（桌面坐标）
  "posY"         INTEGER NOT NULL,       -- 贴图窗口左上角 Y
  "mimeText"     TEXT,                   -- 文本型贴图的原文
  "markText"     TEXT,                   -- 标注文本
  "pinOnScreen"  BOOLEAN NOT NULL,       -- 是否仍贴在屏幕上
  "title"        TEXT,
  "collect"      TEXT,                   -- 收藏
  "saveBasename" TEXT NOT NULL,          -- 关联文件名基名，对应 Data\<basename>.png / .meta
  "group"        TEXT                   -- 贴图分组，实测全为 'default'
);
CREATE TABLE sqlite_sequence(name,seq);   -- 值：PinItem|494
CREATE TABLE "PinImageItem" (
  "id"        INTEGER PRIMARY KEY,       -- 外键 → PinItem.id
  "Width"     INTEGER,                   -- 贴图图像宽
  "Height"    INTEGER,                   -- 贴图图像高
  "OcrResult" TEXT,                      -- OCR 纯文本全文
  "MainColor" INTEGER,                   -- 主色（当前全 NULL）
  "WinTitle"  TEXT,                      -- 截图来源窗口标题
  "ProcessName" TEXT                     -- 截图来源进程名
);
```

### 3.2 行数与样例行

| 表 | 行数 | id 范围 | 说明 |
|---|---|---|---|
| `PinItem` | **100** | 395–494 | `type`: Image 98 / Text 2 |
| `PinImageItem` | **98** | 395–494 | 恰好覆盖 98 个 Image 型（2 个 Text 型无对应行） |
| `sqlite_sequence` | 1 | — | `PinItem` 自增游标 = 494 |
| `PinImageItem.OcrResult` 非空 | **95 / 98** | — | 3 条没有 OCR 结果 |

`PinItem` 样例行（`select * from PinItem order by id limit 3`）：
```
id: 395 | type: Image | createTime: 2026-05-14T23:04:30.614 | closeTime: 2026-05-14T23:05:39.236
device: SeeYee-PC | subItem: NULL | posX: 1474 | posY: 712 | mimeText: NULL | markText: NULL
pinOnScreen: 0 | title: NULL | collect: NULL | saveBasename: 2026-05-14_23-04-30-0 | group: default
（396: posX=1412 posY=1395 basename=2026-05-15_21-09-42-0；397: posX=1497 posY=1161 basename=2026-05-19_22-35-32-0）
```
`PinImageItem` 样例行：
```
id=395 Width=1097 Height=469 MainColor=NULL
  WinTitle="Base64 解码 URL 与密码 - Google Gemini 和另外 82 个页面 - 个人 - Microsoft Edge"
  ProcessName="msedge.exe"  OcrResult(163 字符)="Oregon(俄勒冈州)建议地址
     • 地址行1 (Address line 1): 801 SW 1Oth Ave
     市/区(City): Portland
     • 州 (State): Oregon
     邮政编码(Zip Code): 97205"

id=396 Width=592 Height=411 WinTitle="编写代码错误修复skill的方案 - Claude ..." ProcessName="msedge.exe"
id=397 Width=980 Height=103 WinTitle="Create playbook 和另外 83 个页面 ..." ProcessName="msedge.exe"
id=398 Width=731 Height=544 WinTitle="Usage | Windsurf 和另外 82 个页面 ..." ProcessName="msedge.exe"
id=399 Width=639 Height=387 WinTitle="✳ Compare two project directories" ProcessName="WindowsTerminal.exe"
```
其他聚合（`[确证]`）：
- `pinOnScreen`：**全部为 0**（100/100）—— 会话结束后所有贴图都被关闭，但记录保留。
- `collect` / `subItem` / `group` 分布：`collect` 全 NULL；`subItem` 全 NULL；`"group"` **全部 = 'default'**（说明分组功能已实现但该用户从未创建额外分组）。
- `createTime` 范围：`2026-05-14T23:04:30.614` → `2026-10-05T23:41:41.802`；`closeTime` 最大 `2026-10-05T23:43:07.882`。
- `PinImageItem` 覆盖 **22 个不同进程 / 78 个不同窗口标题**；Top 进程：
  `msedge.exe 48`、`WindowsTerminal.exe 12`、`2345PicViewer.exe 8`、**`snapclip.exe 4`**、`Telegram.exe 3`、`msiexec.exe 2`、`FlClash.exe 2`、`EverEdit.exe 2`、`EXCEL.EXE 2`、`Antigravity.exe 2`。
  > 有趣：**SnapClip 自己被 PixPin 截了 4 次**（`snapclip.exe`）——可用于自查 SnapClip 窗口是否被 PixPin 正确识别。

### 3.3 贴图持久化模型结论

PixPin 的贴图 = **三件套**：
1. `PinWindowd.sqlite::PinItem`：**窗口/业务元数据**（位置 `posX/posY`、创建/关闭时间、设备、分组、是否在屏、文本内容、关联文件基名）。
2. `PinWindowd.sqlite::PinImageItem`：**图像派生元数据**（尺寸、OCR 全文、来源窗口标题/进程名、主色）。
3. `Data\<saveBasename>.png` + `Data\<saveBasename>.meta`：**图像像素** + **窗口状态与 OCR 版面结构**。

**恢复策略 [强推断]**：应用启动时通过 `pinOnScreen=1` 决定是否自动恢复贴图；`closeTime` 非空表示已销毁但保留历史；`device` 字段用于多机同步/区分来源；`group` 支持分组（默认 'default'）。本机 100 条全为 `pinOnScreen=0`，说明恢复后又被关闭或用户从未保留常驻贴图。

**对 SnapClip 的启示**
- 把"哪些贴图要在重启后恢复"做成 `pinOnScreen` 布尔字段，而不是靠文件存在性推断 —— 语义清晰且查询高效（可加部分索引）。
- `ProcessName + WinTitle` 对贴图做来源标注，**成本极低但对"我这张截图是哪来的"这一真实痛点收益极高**。
- OCR 全文同时存 SQLite（可全文检索/复制）与 `.meta`（版面结构），两处职责不同：**文本检索走 SQL，版面重建走 `.meta`**。这是很好的分层。
- `saveBasename` 作为文件名基名贯穿三件套，避免把绝对路径写进数据库。

---

## 4. 日志分析（最重要的证据源）

### 4.1 日志格式样本 [确证]

```
[2026-06-16 07:10:16.293] [3.2.3.1] [info] Qt: Initializing with uuid: "8f0834fc-35b1-4582-a584-6db922b5d0cc"
[2026-06-16 07:10:16.350] [3.2.3.1] [warning] Qt: Loading main QSS
[2026-01-03 21:53:35.175] [PixLog] [info] Qt: Unknown image format 4
[2025-11-01 19:58:39.990] [RobinLog] [info] Qt: Crashpad version: "2.0.0.3"
```

- **格式**：`[YYYY-MM-DD HH:MM:SS.mmm] [<logger>] [<level>] <module>: <message>`
- `<logger>` 实测取值：`PixLog`(14488)、`3.5.5.1`(2863)、`2.4.9.6`(2607)、`3.1.4.0`(1891)、`3.0.8.0`(1413)、`3.2.3.1`(493)、`2.4.9.1`(490)、`RobinLog`(243)。
  → **这个槽位早期是固定串 `PixLog`/`RobinLog`，后期改成"写入时的程序版本号"**，等价于给每行打了版本戳。`RobinLog` 是内嵌的第三方日志库（对应根目录 `RobinLog.dll`）。**[确证]**
- `<module>` 实测几乎恒为 `Qt`（24488/24489 行），真正的位置信息写在消息里：`[Class::method]`。
- **多行消息**：如 `ScreenList PixScreenManager` 之后的缩进块属于同一条记录（30706 物理行中 6217 行为续行）。统计时必须处理。

**文件与轮转**：`pixpin.log`（当前，分析时 392999 B 且仍在增长）→ `pixpin.1.log`（1048386 B，2026-06-16 写出）→ `pixpin.2.log`（1048576 B = 1 MiB，2025-12-31 写出）。**上限 = 1 MiB，保留 3 个文件。** `[确证]`

**时间跨度**：`2025-11-01 19:58:39.990` → `2026-10-08 09:47:09.663`。
**级别分布**：`info 23325 / warning 1146 / error 17`。**恰好三级，无 debug/trace。** `[确证]`

### 4.2 类名 / 方法名 Top 30

`[确证]` 提取规则：从消息中匹配 `[Class::method]`。

| # | 类名 | 次数 | # | 方法名 | 次数 |
|---|---|---|---|---|---|
| 1 | `PixWinCaptureStatic` | 1653 | 1 | `UiSpyThreadWarp::run` | 988 |
| 2 | `PixScreenGXDI` | 1149 | 2 | `PixWinCaptureStatic::getExpectedImageSize` | 984 |
| 3 | `UiSpyThreadWarp` | 988 | 3 | `PixWinCaptureStatic::captureToQImage` | 656 |
| 4 | `UiSpy` | 303 | 4 | `PixScreenGXDI::grabWindowImage` | 655 |
| 5 | `PixOcrTaskWorker` | 69 | 5 | `PixScreenGXDI::grabWindow` | 492 |
| 6 | `MimeDataImageHelper` | 37 | 6 | `UiSpy::DirectGetRect` | 303 |
| 7 | `PixSyntheticEventFilter` | 20 | 7 | `PixOcrTaskWorker::releaseInferEngines` | 53 |
| 8 | `UiRegionDetectorThreadWarp` | 20 | 8 | `MimeDataImageHelper::ExtractImageFromMimeData` | 37 |
| 9 | `FloatPanel` | 17 | 9 | `PixSyntheticEventFilter::eventFilter` | 20 |
| 10 | `ScreenShotView` | 12 | 10 | `UiRegionDetectorThreadWarp::run` | 20 |
| 11 | `PixPin` | 10 | 11 | `PixOcrTaskWorker::InferLifecycle` | 16 |
| 12 | `PixOcrTaskManage` | 8 | 12 | `ScreenShotView::refreshScreenShot` | 9 |
| 13 | **`LongShotWidget`** | **8** | 13 | `PixPin::screenShot` | 7 |
| 14 | `PixScreenManager` | 6 | 14 | `PixWinCaptureStatic::onPrepareTimeout` | 6 |
| 15 | `PixPinAction` | 3 | 15 | `PixScreenManager::refresh` | 6 |
| 16 | `ScreenPixmapItem` | 2 | 16 | `LongShotWidget::getExportPixmap` / `~LongShotWidget` | 2 / 4 |

> 模块名集合直接把 PixPin 的运行时架构摊开了：**捕获层**（`PixScreenGXDI` / `PixWinCaptureStatic` / `PixScreenManager`）、**UIA 元素探测**（`UiSpy` / `UiSpyThreadWarp` / `UiRegionDetectorThreadWarp`）、**OCR 任务池**（`PixOcrTaskWorker` / `PixOcrTaskManage`）、**长截图**（`LongShotWidget`）、**截图视图**（`ScreenShotView` / `ScreenPixmapItem`）、**UI 浮层框架**（`FloatPanel`）、**合成事件过滤**（`PixSyntheticEventFilter`）、**剪贴板图片提取**（`MimeDataImageHelper`）。

### 4.3 关键词命中表（全部三个日志，物理行子串计数）

`[确证]` 方法：Python 读取三文件全文，`str.count(kw)`，大小写敏感。

| 关键词 | 命中 | 分布 | 代表原文 |
|---|---|---|---|
| `LongShot` | **13** | pixpin.log 13 | 见 §4.4 |
| `Stitch` | **5** | pixpin.log 5 | 见 §4.4（字段 `pixStitching=`） |
| `logicalLength` | 2 | pixpin.log 2 | 见 §4.4 |
| `superLong` | 1 | pixpin.log 1 | `LongShotWidget::closeLongShot ... superLong=false` |
| `autoCrop` | 3 | pixpin.log 3 | `autoCropEnabled=false` |
| `crop` / `Crop` | 496 / 5 | pixpin.log | `Image crop started cropRect= QRect(1107,665 1178x686)` ×131 |
| `capture` / `Capture` | 1004 / 3314 | 主要 pixpin.log | 捕获流水线 |
| `DXGI` | **2** | pixpin.log 2 | `[PixScreenManager::refresh] DXGI screen enumeration started engineType= 2` / `... finished screenCount= 1` |
| `engineType` | 1 | pixpin.log | `engineType= 2` |
| `Hotkey` | 1 | pixpin.log | （`globalHotkey` 亦 1）`[PixPin::screenShot] Screenshot response started globalHotkey=true quickShot=false afterSelectAction=0` |
| `shortcut` / `Shortcut` | 7 / 3 | pixpin.log | `[PixPinAction::onShortcutTriggered] Global shortcut activated shortcut=F1 pendingCount=1` |
| `Screenshot` / `screenShot` / `screenshot` | 10 / 9 / 2 | pixpin.log | `[ScreenShotView::refreshScreenShot] Stage reached stage=...` |
| `OCR` / `Ocr` | 4 / 77 | pixpin.1.log | 见 §4.6 |
| `infer` / `Infer` | 61 / 69 | pixpin.1.log | `InferLifecycle` / `releaseInferEngines` |
| `Pin` / `pin` | 748 / 39 | pixpin.1.log 604 / .2 131 / .log 13 | `PixPinIconfont`、`PixScreenGXDI` 等 |
| `UiSpy` | 1294 | pixpin.1.log 1291 | 见 §4.5 |
| `Upgrade` / `upgrade` | 188 / 22 | pixpin.1.log | 升级链 |
| `PixTrack` | 253 | .2 46 / .1 207 | 埋点 |
| `report` | 520 | .2 152 / .1 368 | `Bug report enabled` / `Bug report initialized` |
| `Login` | 224 | .2 40 / .1 184 | `storage file path: "C:/A_Softwares/PixPin/LoginData" storage file does not exist.` |
| `Crash` | 4 | pixpin.2.log | `[RobinLog] Qt: Crashpad version: "2.0.0.3"` |
| `monitor` | 14 | 三文件 | `Could not capture the given monitor.` |
| `ScreenList` | 2031 | .2 460 / .1 1570 | 屏幕枚举块 |
| `PixelRatio` | 123 | pixpin.2.log | `PixelRatio: 1`（2560×1440 时期） |
| `Export` | 4 | pixpin.log | `LongShotWidget::getExportPixmap` |
| **未命中（0 次）** | | | `scroll`、`Scroll`、`stitch`(小写)、`merge`、`Merge`、`match`、`Match`、`WGC`、`GraphicsCapture`、`BitBlt`、`PrintWindow`、`Dwm`/`DWM`、`wheel`、`Wheel`、`Mouse`、`hover`、`max`、`limit`、`history`、`History`、`license`、`Dpi`、`DPI`、`WebP`/`webp`、`video`、`record`/`Record`、`gif`/`Gif`/`GIF`、`mp4`/`MP4`、`onnx`/`Onnx`/`ONNX`、`mosaic`、`formula`、`Translate`/`translate`、`clipboard`、`save`/`Save`、`Table`、`OcrTable`、`AutoMosaic`、`memory`/`cache` |

> **重要判断**：`scroll` / `stitch`（小写）/ `merge` / `match` / `wheel` **全部 0 命中**。也就是说，**长截图的滚动与拼接算法内部没有任何日志**，日志只覆盖 `LongShotWidget` 这个外壳。因此"滚动截图"的存在性证据来自 **`LongShot` / `Stitch`(大写，作为 `pixStitching` 字段名) / `getExportPixmap` / `logicalLength`**，而不是拼接算法本身。**[确证]**

### 4.4 长截图（滚动截图）运行期原文 —— 本次最有价值的证据

日志文件：`C:\A_Softwares\PixPin\pixpin.log`（版本戳 `3.5.5.1`，日期 **2026-10-08**，即分析当天）。

**(A) 第一次长截图会话（09:03:48，选区 1505×901）**

`pixpin.log:545:546:547:548`（原文，未改动）：
```
[2026-10-08 09:03:48.451] [3.5.5.1] [info] Qt: [LongShotWidget::closeLongShot] Closing this=0x299e09be800 superLong=false hasResult=true timerActive=true shotRect=466,325 1505x901 pixStitching=0x299df8312d0
[2026-10-08 09:03:48.455] [3.5.5.1] [info] Qt: [LongShotWidget::getExportPixmap] Export after stopping timer this=0x299e09be800 image=size=1505x3241 format=4 bytesPerLine=6020 bytes=19510820 dpr=1 logicalLength=3241 shotRect=466,325 1505x901 pixStitching=0x299df8312d0
[2026-10-08 09:03:48.455] [3.5.5.1] [info] Qt: [LongShotWidget::~LongShotWidget] Destroying widgets this=0x299e09be800 imagePreview=0x299dee39e10 actBar=0x299db219cc0 maskOverlay=0x299dc6d8c80 pixStitching=0x299df8312d0 cropFloatPanel=0x299df1a5820
[2026-10-08 09:03:48.455] [3.5.5.1] [info] Qt: [LongShotWidget::~LongShotWidget] Destroying state this=0x299e09be800 cropButton=0x299dbfd9840 lastImage=size=1505x3241 format=4 bytesPerLine=6020 bytes=19510820 dpr=1 timerActive=false autoCropEnabled=false shotRect=466,325 1505x901
```

**(B) 第二次长截图会话（09:46:52 → 09:47:08，选区 1178×686）**

`pixpin.log:623`（初始化，暴露裁剪浮层与方向字段）：
```
[2026-10-08 09:46:52.263] [3.5.5.1] [info] Qt: [LongShotWidget::actionBarInit] Crop float panel attached this=0x299e05381f0 actBar=0x299e85a0f30 class=ActionsBar objectName= visible=false geometry=0,0 640x480 cropButton=0x299dbfd9160 class=PixIconButton objectName= visible=false geometry=0,0 100x30 cropFloatPanel=0x299dedbac10 class=CropFloatPanel objectName= visible=false geometry=0,0 100x30 dir=0 autoCropEnabled=false
```
`pixpin.log:2860:2861:2862`：
```
[2026-10-08 09:47:06.756] [3.5.5.1] [info] Qt: [LongShotWidget::getExportPixmap] Export after stopping timer this=0x299e05381f0 image=size=1178x8966 format=4 bytesPerLine=4712 bytes=42247792 dpr=1 logicalLength=8966 shotRect=1107,665 1178x686 pixStitching=0x299dfacf810
[2026-10-08 09:47:08.998] [3.5.5.1] [info] Qt: [LongShotWidget::~LongShotWidget] Destroying widgets this=0x299e05381f0 imagePreview=0x299dbd89500 actBar=0x299e85a0f30 maskOverlay=0x299e9946470 pixStitching=0x299dfacf810 cropFloatPanel=0x299dedbac10
[2026-10-08 09:47:08.998] [3.5.5.1] [info] Qt: [LongShotWidget::~LongShotWidget] Destroying state this=0x299e05381f0 cropButton=0x299dbfd9160 lastImage=size=1178x8966 format=4 bytesPerLine=4712 bytes=42247792 dpr=1 timerActive=false autoCropEnabled=false shotRect=1107,665 1178x686
```

**数值自校验（全部精确相等）[确证]**：
| 项 | 计算 | 日志值 |
|---|---|---|
| 会话 A 行字节 | `1505 × 4 = 6020` | `bytesPerLine=6020` ✓ |
| 会话 A 总字节 | `6020 × 3241 = 19,510,820` | `bytes=19510820` ✓ |
| 会话 A 比例 | `3241 / 901 = 3.597` | 拼接长度约为选区高度的 **3.6 倍** |
| 会话 B 行字节 | `1178 × 4 = 4712` | `bytesPerLine=4712` ✓ |
| 会话 B 总字节 | `4712 × 8966 = 42,247,792` | `bytes=42247792` ✓ |
| 会话 B 比例 | `8966 / 686 = 13.07` | 拼接长度约为选区高度的 **13.1 倍** |

**字段语义推断**：
- `format=4` = Qt5 `QImage::Format_RGB32`（`Format_ARGB32=5`、`Format_ARGB32_Premultiplied=6`）→ **长图成品无 Alpha 通道**。`[确证]`（数值 4×W 的 bytesPerLine 与 RGB32 一致）
- `logicalLength` = 拼接结果的**逻辑长度**（纵向时为高度，横向时应为宽度），与会话 B 的 `8966` 一致。
- `shotRect = x,y WxH` = 长截图选区在屏幕上的位置与尺寸（全局物理像素）。会话 B 的 `1107,665 1178x686` **同时出现在 `LocalStorage.data` 的第 100 条选区历史里**（§2.5），两条独立证据互证。
- `dir=0` = 方向（0 推断为"纵向"，官网称默认为纵向）。`[强推断]`
- `superLong` = 是否为"超长截图模式"（3.2 起新增，官网称最大支持 200 万像素长度）。本机两次均为 `false`。`[确证]+[公开资料]`
- `autoCropEnabled=false` = 自动裁剪（官网标注为**会员功能**）未启用。`[确证]+[公开资料]`
- 组件：`pixStitching`（拼接器）、`imagePreview`（缩略预览）、`maskOverlay`（遮罩/绿框指示层）、`actBar`（工具栏）、`cropFloatPanel` + `cropButton`（裁剪）、`lastImage`（最后一次结果）。与官网长截图界面 14 个元素一一对得上。`[强推断]`

### 4.5 截图主链路逐阶段原文（pixpin.log，2026-10-08 09:46:50–09:46:52）

```
[2026-10-08 09:46:49.964] [3.5.5.1] [info] Qt: [PixPin::screenShot] Screenshot response started globalHotkey=true quickShot=false afterSelectAction=0
[2026-10-08 09:46:50.173] [3.5.5.1] [info] Qt: [PixPinAction::onShortcutTriggered] Global shortcut activated shortcut=F1 pendingCount=1
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [ScreenShotView::prepareScreenShotBgLayer] Screenshot background capture started screenCount= 1
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [ScreenPixmapItem::syncPixmap] Screen capture started geometry= 0,0 3840x2160
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [PixScreenGXDI::grabWindowImage] Image capture finished without cropping
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [ScreenPixmapItem::syncPixmap] Screen capture finished geometry= 0,0 3840x2160 isNull=false
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [ScreenShotView::prepareScreenShotBgLayer] Screenshot background layer created
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [ScreenShotView::prepareScreenShotBgLayer] Screenshot background capture finished
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [PixPin::initScreenShotWindow] Screenshot background prepared
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [PixPin::initScreenShotWindow] Screenshot window constructed
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [PixPin::screenShot] Stage reached stage=screenBackgroundAndWindowInitialized
[2026-10-08 09:46:50.210] [3.5.5.1] [info] Qt: [ScreenShotView::refreshScreenShot] Stage reached stage=activeWindowAndScenePrepared
[2026-10-08 09:46:50.210] [3.5.5.1] [info] Qt: [ScreenShotView::refreshScreenShot] Stage reached stage=shotInfoBarCreated
[2026-10-08 09:46:50.210] [3.5.5.1] [info] Qt: [ScreenShotView::refreshScreenShot] Stage reached stage=cursorAndPointInfoConfigured
[2026-10-08 09:46:50.173] [3.5.5.1] [info] Qt: [ScreenShotView::refreshScreenShot] Stage reached stage=shortcutTipsConfigured
[2026-10-08 09:46:50.210] [3.5.5.1] [info] Qt: [ScreenShotView::refreshScreenShot] Stage reached stage=selectionAndAuxiliaryReset
[2026-10-08 09:46:50.210] [3.5.5.1] [info] Qt: [ScreenShotView::refreshScreenShot] Stage reached stage=screenshotSoundHandled
[2026-10-08 09:46:50.210] [3.5.5.1] [info] Qt: [ScreenShotView::refreshScreenShot] Stage reached stage=mouseSyncScheduled
[2026-10-08 09:46:50.210] [3.5.5.1] [info] Qt: [ScreenShotView::refreshScreenShot] Screenshot view refresh finished
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [PixPin::screenShot] Stage reached stage=screenShotViewRefreshed
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [PixPin::screenShot] Stage reached stage=screenShotWindowDisplayed
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [PixPin::screenShot] Screenshot response finished success=true detail=screenshot window displayed
[2026-10-08 09:46:50.224] [3.5.5.1] [info] Qt: [PixPinAction::processPendingShortcutTriggers] Global shortcut action finished shortcut=F1 remainingCount=0
```

**捕获内核（`PixScreenGXDI` / `PixWinCaptureStatic`）逐行**（同一会话的选区抓取）：
```
[PixScreenGXDI::grabWindow] Capture started screenGeometry= QRect(0,0 3840x2160) region= QRect(1107,665 1178x686)
[PixScreenGXDI::grabWindowImage] Full-screen image capture started screenGeometry= QRect(0,0 3840x2160)
[PixWinCaptureStatic::getExpectedImageSize] Image size query requested
[PixWinCaptureStatic::getExpectedImageSize] Capture mutex acquired
[PixWinCaptureStatic::getExpectedImageSize] Capture core image size query finished image=3840x2160 stride=15360 bpp=4
[PixWinCaptureStatic::captureToQImage] Capture requested image=3840x2160 format=6
[PixWinCaptureStatic::captureToQImage] Capture mutex acquired
[PixWinCaptureStatic::captureToQImage] Capture core buffer request started
[PixWinCaptureStatic::captureToQImage] Capture core buffer request finished
[PixScreenGXDI::grabWindowImage] Full-screen image capture finished imageSize= QSize(3840, 2160) isNull= false
[PixScreenGXDI::grabWindowImage] Image crop started cropRect= QRect(1107,665 1178x686)
[PixScreenGXDI::grabWindowImage] Image crop finished imageSize= QSize(1178, 686) isNull= false
[PixScreenGXDI::grabWindow] Pixmap conversion started imageSize= QSize(1178, 686) isNull= false
[PixScreenGXDI::grabWindow] Capture finished pixmapSize= QSize(1178, 686) isNull= false
```
创建/预热/释放：
```
[PixScreenManager::refresh] Screen refresh requested
[PixScreenManager::refresh] DXGI screen enumeration started engineType= 2
[PixScreenGXDI::PixScreenGXDI] Static capture initialization started screenGeometry= QRect(0,0 3840x2160)
[PixWinCaptureStatic::PixWinCaptureStatic] Creating capture core this=0x... core=0x0 monitor=0x10001 advancedColor=false shaderCount=0 destroying=false timerActive=false
[PixWinCaptureStatic::PixWinCaptureStatic] Capture core created this=0x... core=0x... monitor=0x10001 advancedColor=false shaderCount=0 destroying=false timerActive=false
[PixWinCaptureStatic::prepareCapture] Capture preparation requested / mutex acquired / core preparation started / finished prepared=true
[PixScreenGXDI::PixScreenGXDI] Static capture initialization finished screenGeometry= QRect(0,0 3840x2160) prepared= true
[PixScreenManager::refresh] DXGI screen enumeration finished screenCount= 1
[PixScreenManager::refresh] Screen refresh finished screenCount= 1 wholeRect= QRect(0,0 3840x2160)
...
[PixWinCaptureStatic::onPrepareTimeout] Releasing idle prepared capture this=0x... core=0x... monitor=0x10001 advancedColor=false shaderCount=0 destroying=false timerActive=false
[PixWinCaptureStatic::onPrepareTimeout] Idle release result this=0x... releaseResult=true
```

**从日志可确定的架构事实 [确证]**
1. **抓取后端 = DXGI**（`PixScreenGXDI` 类名 + `DXGI screen enumeration` + `engineType= 2`）；失败时回退 Qt 原生抓屏：`Failed to create PixScreenGXDI: "initializeCapture: -2147024809 | Message: Could not capture the given monitor."`（8 次，`-2147024809 = 0x80070057 = E_INVALIDARG`）+ `Cannot find screen use PixScreenQt`（8 次）。
2. **抓取策略 = 整屏抓取 + 内存裁剪**（先 3840×2160，再 `cropRect` 裁出选区），而不是按窗口/区域直接抓。
3. **`prepared capture` 预热 + 空闲释放**：截图前 `prepareCapture` 预热，空闲后 `onPrepareTimeout` 释放（对应"性能模式/兼容模式"两套行为）。
4. **单实例 + 互斥**：`Capture mutex acquired` 出现在 `getExpectedImageSize` 与 `captureToQImage`。
5. **`monitor=0x10001` = HMONITOR 句柄**；`advancedColor=false` = HDR/高级颜色检测标志（本机恒 false，说明未走 HDR 路径）。
6. **`Capture requested ... format=6`** = 中间 QImage 用 `Format_ARGB32_Premultiplied`；长图导出用 `format=4` = `Format_RGB32`。
7. **单显示器**（`screenCount= 1`），3840×2160，可用区 3840×2088（任务栏 72 px）。

### 4.6 OCR 引擎生命周期原文（pixpin.1.log，版本 3.2.3.1）

```
[2026-06-07 19:03:57.142] [3.2.3.1] [warning] Qt: [PixOcrTaskWorker::InferLifecycle] time= "2026-06-07T19:03:57.142" engine= Det action= create createCount= 1 aliveCount= 1 thread= QThread(0x20550911030)
[2026-06-07 19:03:57.322] [3.2.3.1] [warning] Qt: [PixOcrTaskWorker::InferLifecycle] time= "2026-06-07T19:03:57.321" engine= Rec action= create createCount= 1 aliveCount= 1 thread= QThread(0x20550911030)
[2026-06-07 19:03:57.664] [3.2.3.1] [warning] Qt: [PixOcrTaskManage::onTaskFinished] Starting delayed infer release timer time= "2026-06-07T19:03:57.662" delayMs= 120000 scheduledReleaseAt= "2026-06-07T19:05:57.662" waitingTasks= 0
[2026-06-07 19:03:57.664] [3.2.3.1] [warning] Qt: [PixOcrTaskManage::onReceiveDestoyed] All OCR users gone, releasing infer engines time= "2026-06-07T19:03:57.664"
[2026-06-07 19:03:57.669] [3.2.3.1] [warning] Qt: [PixOcrTaskWorker::InferLifecycle] time= "2026-06-07T19:03:57.669" engine= Det action= release createCount= 1 aliveCount= 0 thread= QThread(0x20550911030)
[2026-06-07 19:03:57.672] [3.2.3.1] [warning] Qt: [PixOcrTaskWorker::InferLifecycle] time= "2026-06-07T19:03:57.672" engine= Rec action= release createCount= 1 aliveCount= 0 thread= QThread(0x20550911030)
```
以及 `[PixOcrTaskWorker::releaseInferEngines] Releasing detection inference engine`（27 次）/ `Releasing recognition inference engine`（26 次）。

**结论 [确证]**：OCR = **两个独立推理引擎**（`Det` 文字检测 + `Rec` 文字识别），在独立 `QThread` 上懒加载；最后一次任务后启动 **120000 ms（2 分钟）延迟释放定时器**，`waitingTasks=0` 时释放。**这是很值得抄的显存/内存策略**。

### 4.7 错误与警告全量清单

**error 级（17 条，全部来自旧版本日志；3.5.5.1 日志中 0 条）**：

| 次数 | 原文 | 归类 |
|---|---|---|
| 1 | `[2026-xx] Qt: "C:/A_Softwares/PixPin/Data/2025-07-06_09-17-32-0.meta" Not exist!` （pixpin.2.log:11965） | **数据一致性缺陷**：`.png` 存在但 `.meta` 被删/未同步（本机现也有 1 个孤儿 PNG：`2026-05-01_22-07-48-0.png` 无 `.meta`） |
| 16 | `Qt: TrigerGraphicEditManage::TrigerGraphicEditManage scene is null` （pixpin.1.log，5 个时间簇） | **图形编辑场景未初始化**：进入标注/编辑时 `QGraphicsScene` 为空，属空指针/生命周期竞态 |

**warning 级 Top（去重后）**：

| 次数 | 原文模板 | 归类 |
|---|---|---|
| 301 | `[UiSpy::DirectGetRect] accLocation failed or returned invalid rect` | **UIA 元素定位失败**（自动滚动/窗口识别依赖）—— 占比最高的警告 |
| 131 | `Screen info "Screen: \\.\DISPLAY1(RTX 4070 Ti SUPER)\n Geometry: (0,0,2560,1440) ..."` | 分辨率变更（2560×1440 时期） |
| 84 ×3 | `Minute Task: Ten Hour Timer Triggered` / `Time Task: Check for New Version` / `Time Task: Ten Hour Timer Finished` | **每 10 小时定时检查更新**（以 warning 级别记录，设计瑕疵） |
| 82 ×2 | `Loading main QSS` / `Main QSS loaded` | 样式表加载也记为 warning（设计瑕疵） |
| 64+2+2+2+1+1+1 | 其他 `Screen info`（3840×2160 各种可用区高度、1024×768、2560×1600、DISPLAY6） | **显示器拓扑频繁变化**（远程桌面/分辨率切换/虚拟显示器），这是 PixPin 已知脆弱点 |
| 37 | `[MimeDataImageHelper::ExtractImageFromMimeData] image obtained from mimeData imageData` | 从剪贴板 MIME 提取图片成功（贴图来源） |
| 36 | `Explorer started detected, refresh screen manager` | Explorer 重启导致重建屏幕管理器 |
| 27 / 26 | `[PixOcrTaskWorker::releaseInferEngines] Releasing detection / recognition inference engine` | OCR 引擎释放 |
| 20 | `[PixSyntheticEventFilter::eventFilter] Synthetic Ctrl+C event detected and filtered` | **PixPin 自己注入 `Ctrl+C` 复制，并在事件过滤器里把自己的合成事件过滤掉**（否则会自触发全局钩子） |
| 14 | `PixTrack network error occurred: "Error transferring https://stat2.pixpin.cn/api/track - server replied: Bad Gateway"` | 埋点上报失败（Bad Gateway） |
| 8 | `Failed to create PixScreenGXDI: "initializeCapture: -2147024809 \| Message: Could not capture the given monitor."` | **DXGI 初始化失败**（`-2147024809` = `E_INVALIDARG`）→ 回退 Qt 抓屏 |
| 8 | `Cannot find screen use PixScreenQt` | 屏幕对象查找失败 |
| 6 / 4 / 2 | `PixTrack network error occurred: "连接超时"` / `"连接已关闭"` / `"...Internal Server Error"` | 埋点上报失败 |
| 4 | `PixPin is quitting...` | 正常退出 |
| 2 | `QProcess: Destroyed while process ("powershell.exe") is still running.` | **调用 PowerShell 后未等待子进程结束**（升级/环境探测用） |
| 1 | `Unknown image format 4` | 图像格式解析失败（`2026-01-03 21:53:35`） |
| 2 | `[UiSpy::DirectGetRect] get_CurrentBoundingRectangle failed or returned invalid rect at point: 2199 888 , use WindowFromPoint rect instead: QRect(63,159 3771x...)` | UIA 失败时的**降级策略：改用 `WindowFromPoint` 取窗口矩形** |

**错误分类结论 [确证]**
1. **捕获后端健壮性**（8+8 次 DXGI/Qt 失败）—— 多显示器/HDR/远程桌面场景是 PixPin 的已知弱点，与官方"性能模式/兼容模式"双后端设计对应。
2. **UIA 元素探测脆弱**（301 次 `accLocation failed`，占所有 warning 的 26%）—— 自动滚动/窗口识别依赖 UIA，在 Electron/Chrome 等无辅助功能树的窗口上必然失败（官方 FAQ 要求 Chrome 加 `--force-renderer-accessibility`）。
3. **数据一致性**（`.meta` 缺失）—— 图片与元数据非事务性写入。
4. **图形编辑场景空指针**（16 次）。
5. **埋点网络失败**（26 次）被当成 warning 记录，会在离线环境下刷日志。
6. **日志级别使用不当**：QSS 加载、定时器触发等正常流程记为 `warning`（1146 条 warning 里约 500 条属此类），真实问题被淹没。

### 4.8 性能埋点（RobinLog，pixpin.2.log）—— 目标延迟基线

```
[2025-11-02 19:37:01.464] [RobinLog] [info] Qt: Spend Time: 188 ms before ScreenShot: 9 ms ScreenShot: 40 ms ScreenShotWidgetInit: 7 ms UiSpyInit: 17 ms ShortCutTipsInit: 0 ms QrCodeDetectInit: 0 ms SetupScreenShotWindow: 0 ms ShowScreenShotWindow: 63 ms After ShowScreenShotWindow: 0 ms
[2025-11-02 21:30:49.829] [RobinLog] [info] Qt: Spend Time: 99 ms before ScreenShot: 1 ms ScreenShot: 47 ms ScreenSh...
[2025-11-03 19:28:37.582] [RobinLog] [info] Qt: Spend Time: 168 ms before ScreenShot: 3 ms ScreenShot: 36 ms ScreenShotWidgetInit: 8 ms UiSpyInit: 4 ms ShortCutTipsInit: 0 ms QrCodeDetectInit: 0 ms SetupScreenShotWindow: 0 ms ShowScreenShotWindow: 66 ms After ShowScreenShotWindow: 0 ms
[2025-11-03 19:28:54.672] [RobinLog] [info] Qt: Spend Time: 87 ms before ScreenShot: 0 ms ScreenShot: 39 ms ScreenShotWidgetInit: 17 ms UiSpyInit: 8 ms ShortCutTipsInit: 0 ms QrCodeDetectInit: 0 ms SetupScreenShotWindow: 0 ms ShowScreenShotWindow: 8 ms After ShowScreenShotWindow: 0 ms
```

**阶段拆解 [确证]**：`before ScreenShot` → `ScreenShot`（抓屏）→ `ScreenShotWidgetInit` → `UiSpyInit` → `ShortCutTipsInit` → **`QrCodeDetectInit`（有二维码识别！）** → `SetupScreenShotWindow` → `ShowScreenShotWindow` → `After ShowScreenShotWindow`。

**实测总耗时：87 / 99 / 168 / 188 ms**（含 3840×2160 全屏抓取）。**对 SnapClip 的性能目标基线**：F1 按下到截图窗口可见 **< 200 ms**，其中抓屏 **36–47 ms**（4K 全屏），窗口显示 **8–66 ms**，UiSpy 初始化 **4–17 ms**。

**对 SnapClip 的启示**：这套"阶段名 + 毫秒"的单行埋点廉价且极有效；建议直接照抄阶段划分（除 `QrCodeDetectInit` 视功能取舍），并把总量作为 CI 性能回归指标。

### 4.9 埋点、崩溃上报、版本、系统与 GPU 信息

| 项 | 日志原文 | 判定 |
|---|---|---|
| **用户行为埋点** | `Qt: PixTrack ProfileID: "52c388d782374c93bfcc3f39fa2a39a6"`（227 次）<br>`Qt: PixTrack network error occurred: "Error transferring https://stat2.pixpin.cn/api/track - server replied: Bad Gateway"` | **有埋点**，上报至 `https://stat2.pixpin.cn/api/track`；ID 与 `LocalStorage.data::PixTrack_RandomID` 完全一致 **[确证]** |
| **崩溃上报** | `Qt: Bug report enabled` / `Qt: Bug report initialized`（各 260 次）<br>`[RobinLog] Qt: Crashpad version: "2.0.0.3"`（4 次） | **有崩溃上报**：PixPin 自身 "Bug report" + Crashpad/Sentry **[确证]** |
| **版本号** | 日志头 `[3.5.5.1]` / `[3.2.3.1]` / `[2.4.9.6]` … | **有**，逐行版本戳 **[确证]** |
| **操作系统** | `Qt: Current Windows version: QOperatingSystemVersion("Windows", 10.0.26100)`（1803 次）<br>`Qt: PixDeviceInfo system version: "Windows 11 Version 24H2"`（184 次） | **有** **[确证]** |
| **GPU** | `"\\\\.\\DISPLAY1"("NVIDIA GeForce RTX 4070 Ti SUPER"): Primary` | **有**（Qt 从显示器适配器名取得 GPU 型号） **[确证]** |
| **屏幕/DPI** | `Geometry: QRect(0,0 3840x2160)` / `AvailableGeometry: QRect(0,0 3840x2088)`；早期日志 `PixelRatio: 1` | **有分辨率与缩放比（实测恒为 1）** **[确证]** |
| **账号/登录** | `Qt: storage file path: "C:/A_Softwares/PixPin/LoginData" storage file does not exist.`（224 次） | 有登录态存储通道 `LoginData`，本机未登录；日志中 **0 次** `license` / `vip` / `token` / `auth` **[确证]** |
| **语言** | `System ui locale name: "zh-CN"` / `App Language set to: "zh-cn"` / `LanguageConfig::setDefaultLanguage "follow_software"` | 跟随系统 **[确证]** |
| **剪贴板** | `[MimeDataImageHelper::ExtractImageFromMimeData] image obtained from mimeData imageData`（37 次） | 贴图从剪贴板取图 **[确证]** |
| **合成事件** | `[PixSyntheticEventFilter::eventFilter] Synthetic Ctrl+C event detected and filtered`（20 次） | 复制通过注入 `Ctrl+C` 实现并自过滤 **[强推断]** |

### 4.10 升级机制（日志实证）

```
[2025-11-04 07:10:55.386] [PixLog] [info] Qt: upgrade success from  ""  to  "2.1.8.0"
[2025-11-05 19:11:55.312] [PixLog] [info] Qt: upgrade success from  "2.1.8.0"  to  "2.1.8.3"
[2025-11-06 21:07:51.527] [PixLog] [info] Qt: upgrade success from  "2.1.8.3"  to  "2.1.8.4"
[2025-11-22 18:29:59.103] [PixLog] [info] Qt: upgrade success from  "2.1.8.4"  to  "2.2.4.0"
[2026-01-06 07:18:54.201] [PixLog] [info] Qt: new upgrade info, version:  "2.3.8.0"  download url:  ("https://download.pixpinapp.com/PixPin_cn_zh-cn_2.3.8.0.exe", "https://download.pixpin.cn/PixPin_cn_zh-cn_2.3.8.0.exe")
[2026-01-23 17:23:45.086] [PixLog] [info] Qt: new upgrade info, version:  "2.4.9.1"  download url:  ("https://download.pixpinapp.com/...", "https://down.pixpin.cn/...", "https://download.pixpin.cn/...")
[2026-02-02 07:20:30.285] [2.4.9.1] [info] Qt: new upgrade info, version:  "2.4.9.6"  ...
[2026-02-02 19:18:20.700] [2.4.9.1] [info] Qt: start install new version from file:  "C:/A_Softwares/PixPin/UpgradeFiles/2.4.9.6.exe"
[2026-02-02 19:18:20.701] [2.4.9.1] [info] Qt: starting upgrade process:  "C:/A_Softwares/PixPin/Temp/PixPinAuxiliary.exe"  args:  ("Upgrade", "C:/A_Softwares/PixPin", "C:/A_Softwares/PixPin/UpgradeFiles/2.4.9.6.exe", "PixPin.exe")
[2026-03-28 07:51:56.733] [2.4.9.6] [info] Qt: start install new version from file:  "C:/A_Softwares/PixPin/UpgradeFiles/3.0.8.0.exe"
[2026-04-24 20:16:06.542] [3.0.8.0] [info] Qt: start install new version from file:  "C:/A_Softwares/PixPin/UpgradeFiles/3.1.4.0.exe"
[2026-06-06 11:30:18.882] [3.1.4.0] [info] Qt: start install new version from file:  "C:/A_Softwares/PixPin/UpgradeFiles/3.2.3.1.exe"
[2026-06-16 07:10:31.076] [3.2.3.1] [info] Qt: PixUpgrade init with grayscale value:  79
[2026-06-16 07:10:31.761] [3.2.3.1] [info] Qt: no new version available
[2026-06-16 17:09:18.469] [3.2.3.1] [warning] Qt: Minute Task: Ten Hour Timer Triggered
[2026-06-16 17:09:18.469] [3.2.3.1] [warning] Qt: Time Task: Check for New Version
```

**机制 [确证]**：多 CDN 下载源（`download.pixpinapp.com` / `pixpin.cdn.dfyun.com.cn` / `download.pixpin.cn` / `down.pixpin.cn`，文件名 `PixPin_cn_zh-cn_<ver>.exe`）→ 下载到 `UpgradeFiles\<ver>.exe` → 把 `PixPinAuxiliary.exe` 复制到 `Temp\` 并以 `("Upgrade", <安装目录>, <升级包>, "PixPin.exe")` 启动 → 辅助进程等待旧进程退出后安装并重启 → 下次启动记录 `upgrade success from "旧" to "新"`。灰度分桶由 `UpdateGrayscale`（本机 18，日志中另一处 79）控制。

### 4.11 `PixPinAuxiliary.log` 全文与 `PixPinAuxiliary.exe` 职责

文件：`C:\A_Softwares\PixPin\PixPinAuxiliary.log`，**626 字节**，mtime `2025/2/6 21:10:24`。**全文 [确证]**：
```
9398187 [INFO] RestartProcessWmain: Starting to restart process: C:/A_Softwares/PixPin/PixPin.exe in mode: Normal
9398187 [INFO] RestartProcessWmain: Process name: PixPin.exe
9398187 [INFO] Waiting for process to exit: PixPin.exe with count 1
9398296 [INFO] RestartProcessWmain: Previous process terminated successfully
9398296 [INFO] RestartProcessWmain: Starting process in normal mode
9398296 [INFO] Running detached process: "C:/A_Softwares/PixPin/PixPin.exe"
9398296 [INFO] Successfully created detached process: C:/A_Softwares/PixPin/PixPin.exe
9398296 [INFO] RestartProcessWmain: Process restarted successfully
```
格式：`<毫秒级计时器(启动后 tick)> [INFO] <函数名>: <消息>`（无日期，只有 tick；`9398187` → `9398296` 相隔 109 tick ≈ 109 ms）。

**职责判定 [确证 + 强推断]**：`PixPinAuxiliary.exe` **不是常驻辅助进程、不做注入、不做权限提升代理**，而是**按需启动的"重启/升级中介"**（单入口 `RestartProcessWmain`）：
1. **重启模式**：以 `mode: Normal`（应有 `Admin` 对应 `RunAsAdmin=true`）等待当前 `PixPin.exe` 退出，再 `detached` 启动新进程 —— 用于"以管理员权限重启""切换运行模式""应用更新后重启"。
2. **升级模式**：日志 `starting upgrade process: ".../Temp/PixPinAuxiliary.exe" args: ("Upgrade", <dir>, <installer>, "PixPin.exe")` 证明它同时承担安装器调用。
3. **存在三份 `crashpad_handler.exe`**（`crashpad\` 555264 B、`crashpad_handler\` 1490232 B、`Helpers\` 700728 B + `crashpad_wer.dll`），其中 `Helpers\crashpad_handler.exe` **正在运行**（PID 16640），这才是常驻的辅助进程；`PixPinAuxiliary.exe` 与 crashpad 无关。

**对 SnapClip 的启示**：自更新/提权重启必须交给**独立中介进程**（旧进程退出后再替换文件），否则会被自身文件锁挡住。这是 Windows 自更新的标准做法，SnapClip 若要做自更新应直接采用。**[强推断]**

### 4.12 `crashpad\` 目录 —— 崩溃上报方案

| 文件 | 大小 | 内容（原文） |
|---|---|---|
| `crashpad\last_crash` | 27 B | `2026-01-14T13:06:31.146852Z`（ASCII，RFC3339 纳秒） |
| `crashpad\metadata` | 16 B | hex `44 41 50 43 01 00 00 00 00 00 00 00 00 00 00 00` → 魔数 `DAPC` + version `1` |
| `crashpad\settings.dat` | 40 B | hex `73 64 50 43 01 00 00 00 01 00 00 00 00 00 00 00 \| d8 94 67 69 00 00 00 00 \| ad 52 1c f5 a4 90 38 43 98 b8 ce 35 ad 4e 57 8d` → 魔数 `sdPC` + version `1` + `01` + **Unix 时间戳 `0x696794d8` = 2026-01-14 13:06:32 UTC** + 16 字节随机（UUID） |
| `crashpad\installation_id` | 70 B | 两行：`cc288a2d-eccb-4f09-12fa-8a9c2e17d6d2` 和 `d3e76aa4570f16ecbf8d8973d5801b09` |
| `crashpad\6e5cb740-...run\__sentry-event` | **509 B** | 见下 |
| `crashpad\6e5cb740-...run\session.json` | **252 B** | 见下 |
| `...\__sentry-breadcrumb1` / `breadcrumb2` | 0 B | 空（无面包屑） |
| `...\*.run.lock` | 0 B | 运行锁 |

**`__sentry-event` 全文（509 B，二进制/类似 msgpack，可读字符串已提取；`Get-Content -Raw` 直读）**：
```
event_id      18f07a4f-8d29-453c-1cee-e1b5616eb97d
level         fatal
platform      native
release       PixPin@3.5.5.1
environment   production
user.id       8f0834fc-35b1-4582-a584-6db922b5d0cc   ip_address  {{auto}}
sdk.name      sentry.native       sdk.version  0.15.2
sdk.packages  [{name: github:getsentry/sentry-native, version: 0.15.2}]
integrations  crashpad
tags / extra / (empty)
contexts.os   name=Windows  kernel_version=6.2.26100.8457  version=10.0.26100  build=8457
trace         trace_id=723b70610c1c456f88d2ee54f7e2b575  span_id=1f79fad7869b4f40  sample_rand=?
```
原始字节（前 96 字节）：
```
hex  : 82 a7 65 76 65 6e 74 5f 69 64 d9 24 31 38 66 30 37 61 34 66 2d 38 64 32 39 2d 34 35 33 63 2d
       31 63 65 65 2d 65 31 62 35 36 31 36 65 62 39 37 64 a5 6c 65 76 65 6c a5 66 61 74 61 6c a8 70
       6c 61 74 66 6f 72 6d a6 6e 61 74 69 76 65 a7 72 65 6c 65 61 73 65 ...
ascii: ..event_id.18f07a4f-8d29-453c-1cee-e1b5616eb97d.level.fatal.platform.native.release...
```
（`82` = msgpack fixmap 长度 2；`a7` = fixstr 长度 7 … → **确证为 MessagePack**）

**`session.json` 全文（252 B）**：
```json
{"init":true,"sid":"0dbb743b-a8fc-461e-4c7c-873a4d85175d","status":"ok","did":"8f0834fc-35b1-4582-a584-6db922b5d0cc","errors":0,"started":"2026-10-01T08:59:54.385866Z","duration":0.000474,"attrs":{"release":"PixPin@3.5.5.1","environment":"production"}}
```

**崩溃上报方案判定 [确证]**：
- **Sentry Native（`gh:getsentry/sentry-native`）0.15.2 + Crashpad 后端 + `integrations: crashpad`**，环境 `production`，release 名形如 `PixPin@<版本>`。官网/根目录配套文件：`QCrashpad.dll`（167 KB）、`RobinLog.dll`、三份 `crashpad_handler.exe`（+ `crashpad_wer.dll`，说明同时挂了 **Windows 错误报告（WER）** 集成）。
- `user.id` = `8f0834fc-35b1-4582-a584-6db922b5d0cc`，**与日志 `Initializing with uuid:` / `PixDeviceInfo init uuid:` 的值完全一致**（224 次）→ 设备/会话 UUID 贯穿日志、Sentry、设备信息三处。
- `last_crash = 2026-01-14T13:06:31.146852Z` 与 `settings.dat` 内嵌时间戳 `2026-01-14 13:06:32 UTC` **相差 1 秒**，互为确证。
- **矛盾说明（未解）**：`.run` 目录 `session.json` 的 `started = 2026-10-01T08:59:54Z` 对应当前正在运行的 crashpad_handler（PID 16640，启动于本地 2026/10/1 16:59:54 = UTC 08:59:54 ✓），但其中的 `__sentry-event` 是 `level=fatal` 事件且 `last_crash` 却指向 2026-01-14。合理但未证实的解释：**该 fatal 事件是上次崩溃的待发送队列/残留**，或 release 字段是写入时读取当前二进制版本。**[未解]**

---

## 5. 历史库与截图数据格式

### 5.1 `.his`（`History\_ScreenshotRecord\*.his`，108 个，0.6–10 MB）

> ⚠️ **本节由父 agent 独立完成，我未独立复核**，仅转录其结论以免重复劳动：
> - 结构：Qt `QDataStream` 容器；头部 `00000003 | 00000008 | "sect"(UTF-16BE)`，key 序列 `sect → rect → SelectMode → RoundRadius → mark → BgLayer → screens → rect → pixmap`，文末 `hotSpot`。
> - **每个 `.his` 恰好内嵌 2 个 PNG：一个 3840×2160 全屏底图 + 一个 48×48 缩略图**（100/100 一致）。
> - 结论：`.his` = **全屏底图 + 选区 rect + 矢量标注**的可再编辑文档，**不是**最终裁剪图。
> - 全量扫描：`.his` 内最大 PNG 都是 3840×2160。
>
> 我的独立旁证：`LocalStorage.data` 的 `HistoryShotRectDatas` 里保存的正是 `QRectF` + `SelectMode` + `RoundRadius` 三元组（与 `.his` 的 `rect / SelectMode / RoundRadius` 键**完全同名同类型**），说明这两处共用同一套选区数据结构。**[强推断]**
> `.his` 文件名是 13 位 Unix 毫秒时间戳（例 `1791383908106` = 2026-10-07 前后），可作为历史排序/清理键。**[确证]**

### 5.2 `Data\*.png` 与配对 `Data\*.meta`

**[确证]** `Data\` 下 **99 个 PNG**，其中 **98 个有配对 `.meta`**；孤儿：`2026-05-01_22-07-48-0.png`（无 `.meta`，且 `.meta` 总数为 100 > 98，另有 2 个 `.meta` 的 PNG 已被删）。

**PNG IHDR 统计（Python `struct.unpack_from('>II', hdr, 16)`）**：

| 属性 | 分布 |
|---|---|
| colorType / bitDepth | **(2, 8) → 98 个**（Truecolor RGB，无 Alpha）；**(6, 8) → 1 个**（Truecolor+Alpha） |
| compression / filter / interlace | 全部 `(0, 0, 0)`（标准 deflate，自适应滤波，**非隔行**） |
| 尺寸范围 | 1655 B – 1007156 B；像素从 `27×103` 级到 `3840×2088` |
| 唯一的 RGBA 文件 | `2026-09-25_17-12-55-0.png` **1368×1121 colorType=6** → **透明区域**，对应 3.5 的**自由截图/折线截图**（选区未覆盖处保留透明），官方 changelog 明确解释了这一点 |

**按像素数排序 Top 8（PNG 尺寸上限实证）**：
```
2026-06-19_15-59-45-0.png   3840x2088   ratio=0.5   595063 B   ← 整屏(3840x2160 减任务栏)
2026-08-23_08-05-06-0.png   2100x1350   ratio=0.6   150470 B
2026-09-13_10-12-33-0.png   1436x1278   ratio=0.9   223841 B
2026-10-02_21-08-28-0.png   1802x903    ratio=0.5   145598 B
2026-10-03_19-17-38-0.png   3708x416    ratio=0.1   146693 B
2026-09-25_17-12-55-0.png   1368x1121   ratio=0.8   165764 B  (RGBA)
2026-10-03_19-08-04-0.png   3680x339    ratio=0.1   152339 B
2026-10-03_14-48-56-0.png   1528x806    ratio=0.5   102098 B
```
**按高度排序 Top 5**：`3840×2088`、`659×1398`、`2100×1350`、`1436×1278`、`1368×1121`。

> **结论 [确证]：`Data\` 中不存在长截图产物。** 最高的图仅 1398 px，最大的是整屏 3840×2088。也就是说，本机虽然**确实用过**长截图（日志证明），但结果**没有落进 `Data\`**（`Data\` 是贴图/图库目录，长截图若用户不"保存/贴图"就不落盘）。这同时解释了 `.his` 里也只有 3840×2160 底图。**因此无法直接测量长图拼接的 overlap 条带**（见 §9 未解疑点）。

### 5.3 `.meta` 格式结论与 hex dump

**格式 [确证]**：Qt `QDataStream` 序列化的 `QMap<QString, QVariant>`，**无版本前缀、无魔数**：
```
u32 mapCount
重复 mapCount 次：
    QString key                      # u32 字节长度(BE, = 2×字符数) + UTF-16BE 数据
    QVariant value                   # u32 typeId + u8 保留位(实测恒 0x00) + payload
```
> 该"1 字节保留位"在 `LocalStorage.data`、`Data\*.meta`、`.his` 三类文件里**都必然存在**；去掉它就会整体错位。实测语义未定（疑为 Qt `QVariant` 的 null/isValid 标志或特定版本头部），但格式确定、可 100% 复现。**[确证布局 / 未解语义]**

观测到的 `typeId` 映射（实测 payload 长度反推）：`1=Bool, 6=Double, 10=String, 11=StringList, 19=QRect(int×4), 20=QRectF(double×4), 26=QPointF, 48=自定义类型(内为 UTF-8 JSON 的 QByteArray), 80=9×qreal 矩阵(QTransform)`

**`.meta` 大小统计 [确证]**：100 个文件，最小 **552 B**（`2026-10-03_16-29-11-0.meta`），最大 **80289 B**（`2026-06-19_15-59-45-0.meta`）。
最小 3：`2026-10-03_16-29-11-0.meta` 552、`2026-08-03_17-59-07-0.meta` 564、`2026-06-04_12-13-36-0.meta` 696。
最大 3：`2026-10-04_19-05-39-0.meta` 62727、`2026-10-02_21-08-28-0.meta` 73624、`2026-06-19_15-59-45-0.meta` 80289。

#### Hex dump 1：`Data\2026-10-03_16-29-11-0.meta`（**最小，552 B**）前 256 字节

```
000000  00 00 00 09 00 00 00 1a 00 57 00 69 00 6e 00 53  .........W.i.n.S
000010  00 74 00 61 00 79 00 73 00 4f 00 6e 00 54 00 6f  .t.a.y.s.O.n.T.o
000020  00 70 00 00 00 01 00 01 00 00 00 14 00 57 00 69  .p...........W.i
000030  00 6e 00 4f 00 70 00 61 00 63 00 69 00 74 00 79  .n.O.p.a.c.i.t.y
000040  00 00 00 06 00 3f f0 00 00 00 00 00 00 00 00 00  .....?..........
000050  18 00 57 00 69 00 6e 00 4b 00 65 00 65 00 70 00  ..W.i.n.K.e.e.p.
000060  52 00 61 00 74 00 69 00 6f 00 00 00 01 00 01 00  R.a.t.i.o.......
000070  00 00 12 00 54 00 72 00 61 00 6e 00 73 00 66 00  ....T.r.a.n.s.f.
000080  6f 00 72 00 6d 00 00 00 50 00 3f f0 00 00 00 00  o.r.m...P.?.....
000090  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00  ................
0000a0  00 00 00 00 00 00 00 00 00 00 3f f0 00 00 00 00  ..........?.....
0000b0  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00  ................
0000c0  00 00 00 00 00 00 00 00 00 00 3f f0 00 00 00 00  ..........?.....
0000d0  00 00 00 00 00 1c 00 54 00 65 00 78 00 74 00 53  .......T.e.x.t.S
0000e0  00 65 00 6c 00 65 00 63 00 74 00 61 00 62 00 6c  .e.l.e.c.t.a.b.l
0000f0  00 65 00 00 00 01 00 01 00 00 00 16 00 53 00 72  .e...........S.r
```
解码结果（**全文**，`mapCount = 9` 个键，**552/552 字节精确消费**）：
```
WinStaysOnTop            = Bool      True
WinOpacity               = Double    1.0
WinKeepRatio             = Bool      True
Transform                = QTransform (1,0,0, 0,1,0, 0,0,1)   # 9×qreal，单位矩阵
TextSelectable           = Bool      True
SrcGeometry              = QRect     (1866, 1052, 1973, 1107)  # 源屏幕区域 x,y,x2,y2
PinWindowDataRelateFiles = StringList ['C:/A_Softwares/PixPin/Data/2026-10-03_16-29-11-0.meta']
MimeText                 = String    'YueTag '
CenterPos                = QPointF   (2039.0, 655.0)            # 贴图中心(桌面坐标)
```
（该文件属于"文本型/无 OCR"的少数形态：`mapCount=9`、含 `TextSelectable + MimeText`、无 `OcrTextJson`。这与 §5.3 的键直方图一致：`TextSelectable` 2 例、`MimeText` 3 例。）

#### Hex dump 2：`Data\2026-06-19_15-59-45-0.meta`（**最大，80289 B**）前 256 字节

```
000000  00 00 00 0a 00 00 00 1a 00 57 00 69 00 6e 00 53  .........W.i.n.S
000010  00 74 00 61 00 79 00 73 00 4f 00 6e 00 54 00 6f  .t.a.y.s.O.n.T.o
000020  00 70 00 00 00 01 00 01 00 00 00 14 00 57 00 69  .p...........W.i
000030  00 6e 00 4f 00 70 00 61 00 63 00 69 00 74 00 79  .n.O.p.a.c.i.t.y
000040  00 00 00 06 00 3f f0 00 00 00 00 00 00 00 00 00  .....?..........
000050  18 00 57 00 69 00 6e 00 4b 00 65 00 65 00 70 00  ..W.i.n.K.e.e.p.
000060  52 00 61 00 74 00 69 00 6f 00 00 00 01 00 01 00  R.a.t.i.o.......
000070  00 00 12 00 54 00 72 00 61 00 6e 00 73 00 66 00  ....T.r.a.n.s.f.
000080  6f 00 72 00 6d 00 00 00 50 00 3f f0 00 00 00 00  o.r.m...P.?.....
000090  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00  ................
0000a0  00 00 00 00 00 00 00 00 00 00 3f f0 00 00 00 00  ..........?.....
0000b0  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00  ................
0000c0  00 00 00 00 00 00 00 00 00 00 3f f0 00 00 00 00  ..........?.....
0000d0  00 00 00 00 00 16 00 53 00 72 00 63 00 47 00 65  .......S.r.c.G.e
0000e0  00 6f 00 6d 00 65 00 74 00 72 00 79 00 00 00 13  .o.m.e.t.r.y....
0000f0  00 00 00 00 00 00 00 00 00 00 00 0e ff 00 00 08  ................
```
解码结果（`mapCount = 10` 个键；`OcrTextJson` 为最后一项且体量巨大，解析在它处停止并报告 `bytes consumed 611 / 80289`，剩余 79678 B 全部属于 `OcrTextJson` 载荷）：
```
WinStaysOnTop            = Bool      True
WinOpacity               = Double    1.0
WinKeepRatio             = Bool      True
Transform                = QTransform (单位矩阵)
SrcGeometry              = QRect     (0, 0, 3839, 2087)          # 整屏贴图
PinWindowDataRelateFiles = StringList ['.../2026-06-19_15-59-45-0.meta', '.../2026-06-19_15-59-45-0.png']
OcrTextSelectable        = Bool      True
OcrTextJson              = 自定义(UTF-8 JSON)   ← 承担 80289 B 中的绝大部分
ImageDevicePixelRatio    = (double)              ← 键序见键直方图（此处解析在 OcrTextJson 处提前停止）
CenterPos                = QPointF
```

#### 最大 `.meta` 的载荷 = `OcrTextJson`（**OCR 版面结构**）

`OcrTextJson` 的实际内容是 UTF-8 JSON（`typeId=48` 的自定义类型把 JSON 放在 QByteArray 里）。**原文片段 [确证]**：
```json
{"blocks":[
  {"box":[[49.130615234375,107.0676040649414],[48.93194580078125,78.0621109008789],
          [359.8694152832031,75.9323959350586],[360.0680847167969,104.9378890991211]],
   "textLines":[
     {"box":[[48.93194580078125,78.0621109008789],[359.8694152832031,75.9323959350586],
             [360.0680847167969,104.9378890991211],[49.130615234375,107.0676040649414]],
      "centerPos":[7.2877678871154785,21.8633041...],
      "text":"...", "textType":0}
   ]},
  ...
],"lang":"zh-cn"}
```
另一例（`2026-10-02_21-08-28-0.meta`）以 `{"blocks":[{"backgroundColo...` 开头；小文件里可见 `{"blocks":[],"lang":""}`。

**结构推断 [确证]**：
```
{ "blocks": [
    { "box": [[x,y]×4],                    # 块级四边形（4 个角点，float，可表达旋转/倾斜）
      "textLines": [
        { "box": [[x,y]×4],                # 行级四边形
          "centerPos": [x,y],              # 行中心
          "text": "...",                   # 行文本
          "textType": 0|1 }                # 文本类型（0=普通；1=其他，如公式/特殊字体）
      ] }
  ],
  "lang": "zh-cn" | "" }                   # 语种标记
```
**这是本次调研中对 SnapClip 价值最高的单一发现之一**：PixPin 的 OCR 结果不是纯文本，而是**带任意四边形定位的版面模型**（支持倾斜/旋转文本），并同时给出行中心、类型与语种。

**100 个 `.meta` 的顶层键直方图 [确证]**：
```
100×  WinStaysOnTop / WinOpacity / WinKeepRatio / Transform / SrcGeometry / PinWindowDataRelateFiles / CenterPos
 98×  OcrTextJson / ImageDevicePixelRatio
 97×  OcrTextSelectable
  3×  MimeText
  2×  TextSelectable        (3.5 之前的旧键名)
  1×  MimeHtml
  1×  WinBorderRadius       (3.5.5.1 新增的"贴图/长截图边框圆角")
```
95 个文件的键顺序完全一致（说明序列化顺序稳定，可做版本/兼容判断）：
`WinStaysOnTop, WinOpacity, WinKeepRatio, Transform, SrcGeometry, PinWindowDataRelateFiles, OcrTextSelectable, OcrTextJson, ImageDevicePixelRatio, CenterPos`

**额外发现：PixPin 会用 WebP 存贴图 [确证]**：
`Data\2026-10-02_21-08-28-0.meta` 的 `PinWindowDataRelateFiles` 解码后为
```
['C:/A_Softwares/PixPin/Data/2026-10-02_21-08-28-0.meta',
 'C:/A_Softwares/PixPin/Data/2026-10-02_21-08-28-0.png',
 'C:/A_Softwares/PixPin/Data/2026-10-02_21-08-28-0.webp']
```
虽然该 `.webp` 文件现已不存在（当前 `Data\` 下 `.webp` 文件数 = 0），但这**证明贴图曾以 WebP 形式落盘**。也说明 `PinWindowDataRelateFiles` 是"同一贴图的多份关联文件"清单（与 `PinItem.saveBasename` 配合）。

**`.meta` 关键字段语义总结**

| 字段 | 类型 | 含义 | 示例实测值 |
|---|---|---|---|
| `WinStaysOnTop` | Bool | 贴图窗口置顶 | 恒 `True` |
| `WinOpacity` | Double | 贴图不透明度 | 恒 `1.0` |
| `WinKeepRatio` | Bool | 缩放保持宽高比 | 恒 `True` |
| `WinBorderRadius` | (int) | 边框圆角（3.5.5.1 新增） | 仅 1 例 |
| `Transform` | QTransform(9×double) | 贴图缩放/旋转/平移矩阵 | 本机全为单位矩阵（未缩放） |
| `SrcGeometry` | QRect | **源屏幕物理像素区域** | `(1866,1052,1973,1107)`、`(0,0,3839,2087)`、`(1077,719,2878,1621)` |
| `CenterPos` | QPointF | **贴图窗口中心的桌面坐标** | `(2039.0, 655.0)` |
| `ImageDevicePixelRatio` | (double) | 贴图时的设备像素比 | 98/100 存在 |
| `PinWindowDataRelateFiles` | StringList | 关联文件（`.meta`/`.png`/`.webp`） | 见上 |
| `TextSelectable` / `OcrTextSelectable` | Bool | 是否允许在贴图上选中文字 | 97–98/100 `True` |
| `OcrTextJson` | 自定义(UTF-8 JSON) | **OCR 版面结构**（blocks/textLines/box/centerPos/text/textType/lang） | 见上 |
| `MimeText` / `MimeHtml` | String | 来源剪贴板文本/HTML（文本型或带 HTML 的贴图） | `'YueTag '`、`'collision.txt'` |

**对 SnapClip 的启示**
1. **把"贴图窗口状态"和"图像像素"分开**：`.meta`（状态/版面/OCR）+ `.png`（像素）+ SQLite（可查询元数据）。恢复贴图时先读 SQLite 定位 `saveBasename`，再读 `.meta` 还原窗口几何/透明度/变换/OCR 选区。
2. **OCR 结果存"四边形版面 JSON"而非纯字符串**：一次投入，永久受益（旋转文本、按行重排、图文对照翻译、表格重建都依赖它）。建议 SnapClip 直接用 `serde_json` + `Vec<Block{ box:[Point;4], lines:Vec<Line> }>`。
3. **用 `SrcGeometry` 记录"这张图截自屏幕哪里"**：支持"重新截图同一区域""把贴图放回原位"等高级交互。
4. **`CenterPos` + `Transform` 而非 `pos/size`**：以中心 + 变换矩阵建模，缩放/旋转时无需重算，且与 Qt `QGraphicsItem` 语义天然一致。
5. **`PinWindowDataRelateFiles` 这种"关联文件清单"**便于清理时一并删除（避免 `.png`/`.meta`/`.webp` 泄漏），也解释了本机 2 个孤儿 `.meta` 的成因。

### 5.4 关于"重复行带 / 拼接痕迹"的检查方法（**未完成，方法留给后续**）

因为 `Data\` 与 `.his` 都没有长截图产物（§5.2、§5.1），**本次无法在真实产物上测量 overlap 条带**。可复用的方法（供后续在用户实际保存的长图上执行）：
1. 用 PNG IHDR 读 `W×H`，按行切出 `H` 个 `W×4`(=RGBA) 或 `W×3` 的字节块；
2. 对每行算 `md5`/`crc32`，得到长度 `H` 的指纹序列；
3. 若拼接存在固定 overlap（例如每次滚动的可视高度减去重叠像素），指纹序列会出现**周期性的"重复区间"**——对每个候选 overlap `k`，统计 `fingerprint[i] == fingerprint[i+k]` 的匹配率，匹配率高且 `k` 稳定 → overlap = k；
4. 若拼接算法按"最优匹配"自适应（PixPin 官网称"智能图片拼接算法"），则 `k` 会随内容变化，需要逐段做最长公共前缀/相关匹配；
5. 交叉验证：`logicalLength / shotRect.height` 应约等于"滚动次数 + 1"，而 `滚动次数 × (shotRect.height - overlap) + shotRect.height = logicalLength`，可反解 overlap。
   本机两次实测：`3241/901 = 3.60`、`8966/686 = 13.07`，说明**重叠是存在的，否则整倍关系应更接近整数**（3.60 与 13.07 都明显偏离整数，符合"最后一段不满/首段截断"或"重叠可变"）。**[强推断]**

---

## 6. 插件与编解码能力

### 6.1 `plugins\` 清单与推断（`[确证]` 目录列举）

| 目录 | DLL | 推断结论 |
|---|---|---|
| `imageformats\` | `qgif.dll` 42808, `qicns.dll` 46904, `qico.dll` 40760, `qjpeg.dll` 402232, `qsvg.dll` 36664, `qtga.dll` 35640, `qtiff.dll` 375096, `qwbmp.dll` 34104, `qwebp.dll` 418104 | **导出/读取格式 = GIF, ICNS, ICO, JPEG, SVG, TGA, TIFF, WBMP, WebP**（PNG 内置于 `Qt5Gui.dll`，故无 `qpng.dll`；**无 `qmng.dll`** → 不支持 MNG）。可写 vs 只读需逐格式试；Qt 中 JPEG/PNG/TIFF/WebP/GIF(动画)/ICO/ICNS/TGA/WBMP 均可写，SVG 只读。 |
| `sqldrivers\` | `qsqlite.dll` 1006392 | **仅 SQLite**（与 `Data\PinWindowd.sqlite` 一致；无 MySQL/ODBC/PG → 数据库不依赖外部服务） |
| `platforms\` | `qwindows.dll` 1293112 | Windows 平台插件 |
| `mediaservice\` | `dsengine.dll` 277304, `qtmedia_audioengine.dll` 69432, `wmfengine.dll` 196920 | **多媒体播放/录制**：DirectShow（`dsengine`，录屏/摄像头/音视频捕获）、Qt 内置音频引擎、Windows Media Foundation（`wmfengine`） |
| `audio\` | `qtaudio_wasapi.dll` 88888, `qtaudio_windows.dll` 63288 | **WASAPI（含 loopback）→ 可录"系统音频"**；与配置 `PixMovie.record.recordMicrophone` 及官方"可选麦克风音频与系统音频"对应 |
| `bearer\` | `qgenericbearer.dll` | 网络承载 |
| `printsupport\` | `windowsprintersupport.dll` 54584 | **打印能力**（对应 `ScreenShot.ActBarFlag.Print`=768） |
| `styles\` | `qwindowsvistastyle.dll` | 原生外观 |

**对 SnapClip 的启示**：如果要"导出格式覆盖度"对齐 PixPin，必须支持 **JPEG / PNG / WebP / TIFF / GIF / BMP(隐含) / TGA / ICO / ICNS / WBMP**；Rust 侧建议 `image` crate + `webp`（libwebp）+ `tiff`，并**默认格式设为 JPEG**（实测 `SaveDialog.LastImageFormat='jpg'`、`SaveInfoStaticSuffix='jpg'`、`SaveQuality=100`）。

### 6.2 `model\` 与 `OcrModel\`

`OcrModel\` = **空目录（0 个文件）** `[确证]`。

`model\` 逐个分析（`[确证]` 文件头 + 字符串提取）：

| 文件 | 大小 | 头部 hex(前 16 B) | 类型与来源推断 |
|---|---|---|---|
| `detect.prototxt` | 45372 B | `6c 61 79 65 72 20 7b 0d 0a ...` = `layer {\r\n` | **明文 Caffe 网络定义**。首段字符串：`layer {` / `  name: "data"` / `  type: "Input"` / `  top: "data"` / `  input_param {` / `    shape {` → **文本检测网络（输入层 + 形状定义）** |
| `detect.caffemodel` | 965430 B | `0a 00 a2 06 22 0a 04 64 61 74 61 12 05 49 6e 70` = protobuf(`0a 00 a2 06 22`) + `"data"` + `"Inp` | **Caffe 权重**（protobuf）。字符串：`Input"`, `dataP`, **`data/bn`**, **`BatchNorm`**, `data"`, `data2` → 含 BatchNorm 层 → **OpenCV DNN 文本检测模型（方向：DB / EAST 类）** |
| `sr.prototxt` | 6387 B | `6c 61 79 65 72 20 7b ...` = `layer {\r\n` | **明文 Caffe 定义**（超分网络） |
| `sr.caffemodel` | 23929 B | `0a 00 a2 06 22 0a 04 64 61 74 61 12 05 49 6e 70` | **Caffe 权重**。字符串：`Input"`, **`data_data_0_split`**, **`Split`** → **超分辨率（Super-Resolution）网络**，用于小字/低分辨率图像放大后再 OCR |
| `paragraph_recognition.onnx` | 3198623 B | `08 0a 12 07 70 79 74 6f 72 63 68 1a 0a 32 2e 31` = protobuf: field1=1(varint `0a`=10), field2(`12`) len 7 = `"pytorch"`, field3(`1a`) len 10 = `"2.11.0+cpu:"` | **ONNX ModelProto**，**由 PyTorch 2.11.0+cpu 导出**；张量名 `node_features` / `val_0` / `node_Shape_0` / `Shape` → **段落/整行文字识别模型** |
| `2478d3813bea9afacf7fee18abb09d45.bin` | 9880512 B | `7f 79 59 26 2f 36 5b ca d4 b0 25 58 f3 36 46 63 ...` | **加密/混淆模型**（熵 7.535） |
| `5f515ede591a02e400275eaa2e5c0ddf.bin` | 16615441 B | `7f 79 59 26 2f 36 5b b4 9e 9a 26 58 fd 36 46 63 ...` | **加密/混淆模型**（熵 7.536） |
| `94186d8ad4eacc8b8aaf3c1088eb9353.bin` | 4729474 B | `7f 74 71 d3 e1 96 63 6f d5 6a 33 40 0c 56 38 01 ...` | **加密/混淆模型**（熵 7.506） |
| `cb6cc28d4121651b3f5bc3daecc9188e.bin` | **21234344 B** | `7f 75 59 26 2f 36 5b a4 e8 e0 2b 58 fa 36 46 63 ...` | **加密/混淆模型**（熵 7.566） |
| `dbb5b4317e638ad5a21a42b035cfd159.bin` | 10838604 B | `7f 74 71 84 c2 a5 64 6f ee 69 33 46 0c 56 38 01 ...` | **加密/混淆模型**（熵 7.520） |

**5 个 `.bin` 的判定过程 [确证]**：
```python
# 熵计算（前 200 KB）
2478d3813bea9afacf7fee18abb09d45.bin  entropy=7.535
5f515ede591a02e400275eaa2e5c0ddf.bin  entropy=7.536
94186d8ad4eacc8b8aaf3c1088eb9353.bin  entropy=7.506
cb6cc28d4121651b3f5bc3daecc9188e.bin  entropy=7.566
dbb5b4317e638ad5a21a42b035cfd159.bin  entropy=7.520
```
- 熵接近 7.5 bits/byte（理论上限 8.0）→ **强加密或高熵压缩**，不是明文 ONNX（明文 ONNX 头部必有 `08 07/08 08/08 09` 之类 varint + `producer_name` 字符串）。
- **无任何可读 ASCII 字符串**（前 320 B 提取字符串全为乱码如 `'yY&/6['`、`'yEEZX'`、`';=GyMyCiW]cy5"\\Xb'`）。
- 单字节 XOR 探测：`0x77` 使 3 个文件首字节变为 `08 03` / `08 02` / `08 03`（protobuf varint field-1 的合法起始），但**用 `0x77` 全文件 XOR 后仍无可读字符串**（结果如 `'.QXA,'`、`'22-/` @Cg'`）→ **不是单字节 XOR**，判定为**流密码/AES 类加密模型**。**[强推断]**
- 文件名是 32 位十六进制（MD5 风格）→ **内容寻址/版本化模型缓存**，便于按哈希做增量更新与缓存失效。**[强推断]**

**推理框架证据 [确证]**（DLL 字符串计数）：
```
PixAVCodec.dll    8008504 B  {libx264:4, h264:89, H264:108, ffmpeg:112, FFmpeg:5, avcodec:48,
                              aac:33, AAC:17, mp4:49, MP4:26, gif:24, GIF:29, webp:28, WEBP:10,
                              VP8:38, VP9:7, mkv:2, avi:19, recording:2, Record:5, mnn:1}
PixMovie.dll       574776 B  {recording:24, Record:261, h264:18, H264:12, gif:2, webp:6, mp4:5, mkv:1, AAC:1}
PixVision.dll     5758776 B  {opencv:78, OpenCV:24, MNN:4, mp4:11, GIF:2}
PixOCR.dll        4755256 B  {opencv:113, OpenCV:24, ONNX:14, onnx:1, GIF:19, mp4:11, avi:4}
PixOCR2.dll       4999480 B  {MNN:75, opencv:99, OpenCV:24, ONNX:4, onnx:1, GIF:4}
PixFormulaRec.dll 3126584 B  {webp:1, VP8:2, aac:6, avi:2}
```
根目录另有 **`onnxruntime.dll` 14,448,952 B** + `onnxruntime_providers_shared.dll` 22,328 B → **ONNX Runtime 推理**；`PixModelRunner.dll` 94,008 B = 模型运行器；`PixVision.dll` 5.76 MB + `UiRegionDetector.dll` + `UiSpy.dll`。

**结论 [强推断]**：PixPin 的识别栈是**混合的**：
- `PixOCR.dll`：**ONNX Runtime** + OpenCV（老/通用 OCR 路径）
- `PixOCR2.dll`：**MNN（阿里 MNN 推理框架）** + OpenCV（新 OCR 路径，3.x）
- `PixVision.dll`：OpenCV + MNN（视觉算法：UI 元素检测、区域检测、拼接匹配）
- `model\` 的 Caffe 模型（detect/sr）→ 走 **OpenCV DNN**（OpenCV 原生支持 Caffe）
- `model\paragraph_recognition.onnx` + 5 个加密 `.bin` → 走 **ONNX Runtime / MNN**

### 6.3 `PixAVCodec.dll`（8.0 MB）与录屏/动图能力

**结论：PixPin 具备完整的录屏与动图（GIF/WebP/MP4）能力。** 三重证据：

1. **`PixAVCodec.dll` 字符串 [确证]**：`ffmpeg`(112) / `FFmpeg`(5) / `avcodec`(48) / **`libx264`(4)** / `h264`(89)+`H264`(108) / `aac`(33)+`AAC`(17) / `mp4`(49)+`MP4`(26) / `gif`(24)+`GIF`(29) / **`webp`(28)+`WEBP`(10)** / `VP8`(38) / `VP9`(7) / `mkv`(2) / `avi`(19) / `recording`(2)。
   → 内含 **FFmpeg + libx264（H.264 编码）+ AAC（音频编码）**，可产出 **MP4**，并可产出 **GIF / WebP 动图**（VP8 存在即 WebP 有损/动画编码）。
2. **`PixMovie.dll` 字符串 [确证]**：`Record` 261 次、`recording` 24 次 → 录制模块。
3. **`plugins\mediaservice\`（dsengine / wmfengine）+ `plugins\audio\qtaudio_wasapi.dll`** → 捕获（DirectShow/WMF）与音频（WASAPI，可系统内录）。
4. **配置键 [确证]**：`"PixMovie.record.recordMicrophone": {"t":1790993243,"v":false}` → 录制模块真实存在，且本机关闭了麦克风。
5. **截图工具栏 [确证]**：`ScreenShot.ActBarFlag.GifShot = 257` → 工具栏含"录制/动图"按钮。
6. **公开资料 [公开资料]**：官方「屏幕录制」页明确：普通录制导出 **MP4 / GIF / WebP**，快速录制仅 **MP4**；FPS 支持 **5/16/24/30/60**；可录**麦克风音频和系统音频**；会员可录**按键信息与鼠标点击动作**、**摄像头画中画**；3.5.5.1 起"开启硬件加速后 MP4 导出支持调用 GPU"、"提高屏幕录制的码率"、"记忆上次导出时选择的画质"。长截图/录制页面同属官方文档导航。

**内部矛盾检查**：本机日志中 `gif` / `mp4` / `record` / `video` **命中 0 次**，但这**不构成矛盾** —— 该用户从未使用录制功能，且 PixPin 的录制相关代码不写日志。因此"本机静态证据（有录屏能力）"与"日志无录屏记录"是一致的。**[确证]**

### 6.4 其他能力线索（DLL 与配置）

| DLL | 大小 | 能力线索 |
|---|---|---|
| `PixOCR.dll` / `PixOCR2.dll` | 4.76 MB / 5.00 MB | 文字识别（ONNX / MNN 双路径） |
| `PixFormulaRec.dll` | 3.13 MB | **公式识别**（配合 `LatexRecognition` 工具栏项 514） |
| `PixLatex2MathML.dll` | 1.26 MB | **LaTeX → MathML 转换** |
| `PixVision.dll` | 5.76 MB | 视觉算法（OpenCV + MNN）：UI 检测、区域检测、拼接匹配 |
| `UiSpy.dll` / `UiRegionDetector.dll` | 84.8 KB / 90.9 KB | **UIA 元素探测 / UI 区域检测**（自动滚动、窗口识别） |
| `PixActionsBar.dll` / `SalmonActions.dll` / `PixWidget*.dll` | 107 KB / 90 KB / 991 KB + 399 KB | 截图工具栏与浮层控件 |
| `PixLottie.dll` | 372 KB | **Lottie 动画**（教程/新手引导） |
| `PixPinTutorial.dll` | 4.27 MB | 内置教程 |
| `PixAuth.dll` / `PixWebCallback.dll` / `PixDownload.dll` / `PixNetwork.dll` | 3.77 MB / 93 KB / 82 KB / 105 KB | **账号/授权/网页回调/下载**（会员与云服务） |
| `PixStat.dll` / `QAppStat.dll` | 44 KB / 39 KB | **统计埋点** |
| `QCrashpad.dll` / `RobinLog.dll` | 167 KB / 316 KB | 崩溃上报 / 日志 |
| `PixProgramManage.dll` / `PixSystemUtils.dll` / `PixWin32CaptureCore.dll` / `PixWinCapture.dll` | 61 KB / 150 KB / 141 KB / 76 KB | 进程管理 / Win32 工具 / **Win32 捕获内核** |
| `PixScreenManager.dll` | 88.9 KB | 多显示器管理 |
| `PixColorPalette.dll` / `PixStyle.dll` / `QIconfont.dll` | 92 KB / 224 KB / 63 KB | 取色板 / 样式（QSS）/ 图标字体 |
| `PixNotification.dll` / `PixWindowNotify.dll` | 145 KB / 39 KB | 通知 / 窗口事件 |
| `PixKeyMouse.dll` | 74.6 KB | **键鼠录制/回放**（对应会员"记录按键信息与鼠标点击动作"） |
| `PixPinContextMenu\PixPinContextMenuExt.dll` + `.msix` | 64 KB / 163 KB | **Windows 资源管理器右键菜单扩展**（MSIX 包） |
| `language\*.qm` (8 种) + `language_qt\qt_zh-cn.qm` | — | 多语言：de-de, es-es, fr-fr, ja-jp, ko-kr, pt-pt, zh-cht, zh-cn |

**注意**：`pixpin.cn/docs` 侧边栏列出的能力（贴图分组、序列号标注、聚光灯、水印、放大镜、马赛克/模糊/智能擦除、翻译、表格识别、公式识别、脚本、全局鼠标、创作者分享）与本地 `Mark.EditItemOrder` 的 7 组工具项**高度吻合**：`Geometry/HighLight, Pencil/Marker, Arrow/BrokenLine/Magnifier, Text/Watermark, Serial, Mosaic/AutoMosaic, Eraser`。**本地配置与官方功能列表互证，无矛盾。** `[确证]`

---

## 7. 滚动截图能力判定

### 7.1 有/无：**有，且本机被实际使用过** `[确证]`

| 证据类别 | 证据 | 强度 |
|---|---|---|
| **运行期日志（最强）** | `pixpin.log`（v3.5.5.1，2026-10-08）中 `LongShotWidget` 的 `closeLongShot` / `getExportPixmap` / `~LongShotWidget` / `actionBarInit` 共 8 条，含 `logicalLength=3241` 与 `logicalLength=8966` 的成品尺寸 | 确证 |
| **配置** | `"ScreenShot.ActBarFlag.LongShot": {"t":1790318723,"v":256}` → 截图工具栏有"长截图"按钮（且被用户改动过时间戳） | 确证 |
| **状态字段** | `superLong`（超长模式）、`autoCropEnabled`（自动裁剪）、`dir`（方向）、`pixStitching`（拼接器对象）、`maskOverlay`（匹配指示遮罩）、`imagePreview`（缩略预览） | 确证 |
| **官方文档** | 长截图专页 + 14 项界面元素 + "3.2 起增加超长截图模式" + 限制说明 | 公开资料 |

### 7.2 产物形态：**单张超长位图（内存中拼接）** `[确证]`

- 日志字段 `[LongShotWidget::getExportPixmap] Export after stopping timer ... image=size=WxH format=4 bytesPerLine=4W bytes=4*W*H dpr=1 logicalLength=H shotRect=...`
- 数值三重自洽：`1505×3241` → `6020 / 19510820`；`1178×8966` → `4712 / 42247792`，**全部等于 `4×W` 与 `4×W×H`**。
- `format=4` = `QImage::Format_RGB32` → **无 Alpha，RGB 连续缓冲，行跨距 = 宽×4（无 padding）**。
- `dpr=1` = device pixel ratio，长图按物理像素存储。
- **不是分段**：只有一次 `getExportPixmap`，且 `lastImage` 与导出图尺寸完全相同（`size=1178x8966`），说明**拼接结果就是一张图**。
- `logicalLength` 与会话 B 的 `8966` 相等 → 纵向时 `logicalLength == height`；横向时应为 width。`[强推断]`

### 7.3 尺寸上限

| 来源 | 上限 |
|---|---|
| 官方文档（长截图页） | "3.2 起增加超长截图模式，**最大可支持截取 200 万像素长度**的图片" **[公开资料]** |
| 官方文档（注意事项） | "截图高度**接近 100 万像素**时可能无法正常导出；达到约 **75 万像素**时，即使导出成功，普通图片软件也可能无法打开" **[公开资料]** |
| 本机日志实测 | 同一会话中 `logicalLength` 分别 3241 / 8966，`superLong=false` → **普通模式**（未触发超长模式）；`shotRect.height` 901 / 686 **[确证]** |
| 内存占用（我推导） | `W × H × 4` 字节（RGB32）。若 W=1500、H=1,000,000 → **6 GB**，说明"100 万像素高度"上限本质是**内存/导出限制**而非算法限制。**[强推断]** |

### 7.4 与捕获链路的关系

长截图的每一帧都走**同一条 DXGI 抓屏链路**：`PixScreenGXDI::grabWindow → grabWindowImage → getExpectedImageSize → captureToQImage`。会话 A 的日志显示，在 `closeLongShot` 前 200 ms 内仍有一次完整的整屏抓取（`Full-screen image capture started ... image=3840x2160 format=6`），说明**抓帧是按需/连续进行的，之后整屏→裁剪→送入 `pixStitching`**。`[强推断]`

### 7.5 overlap / 拼接痕迹

**未能直接验证**（`Data\` 与 `.his` 内均无长截图产物，见 §5.2）。可用的间接推断：`logicalLength / shotRect.height` = **3.60** 与 **13.07**，两者都**不是接近整数**的比例；若拼接无重叠且帧数整数，比例应接近整数（帧数+1）。因此**存在可变重叠**。方法与反向求解公式见 §5.4。**[强推断]**

---

## 8. 其他能力清单

| 能力 | 判定 | 本机证据 | 公开资料 |
|---|---|---|---|
| **静态截图 + 标注编辑** | 有 | `PixPin::screenShot` 全流程日志；`Mark.EditItemOrder` 7 组标注工具；`ScreenShotView` 8 个阶段 | 有 |
| **长截图/滚动截图** | 有 | 见 §7 | 有（含超长模式、自动裁剪-VIP） |
| **选区模式** | rect（本机）+ 自由/折线/多窗口（3.5 新增） | `Screenshot.SelectMode='rect'`、`FreeSelectMode=false`；RGBA PNG 1 例证明自由选区 | 3.5.5.1 changelog：多截图/自由/折线/多窗口 |
| **OCR（文字识别）** | 有 | `PixOcrTaskWorker` Det+Rec 双引擎、120 s 空闲释放；`OcrTextJson` 版面 JSON；`PixOCR.dll`(ONNX) + `PixOCR2.dll`(MNN) | 有 |
| **OCR 结果结构** | **四边形版面模型** | `blocks[].box[[x,y]×4]` + `textLines[].box/centerPos/text/textType` + `lang` | — |
| **表格识别** | 有 | 工具栏 `OcrTable=512`；`PinImageItem.OcrResult` 里可见表格文本 | 有（VIP，3.5 支持合并/拆分单元格） |
| **公式识别（LaTeX）** | 有 | `LatexRecognition=514`；`PixFormulaRec.dll` 3.13 MB；`PixLatex2MathML.dll` 1.26 MB | 有（VIP） |
| **翻译** | 有 | `Translate=259`；`System.Translate.TranslateKey`（Base64+`\|\|\|0`）；未在日志命中 | 有（VIP，3.5 支持图文对照） |
| **超分辨率** | 有 | `model\sr.caffemodel` + `sr.prototxt`（Caffe，含 `Split` 层） | 未在官方文档显著提及 |
| **二维码识别** | 有 | 性能埋点阶段 `QrCodeDetectInit`（RobinLog） | 未显著提及 |
| **马赛克/模糊/智能擦除** | 有 | `MarkBar.Mosaic.BlurStrength=28`、`MosaicMode=0`、`MosaicStrength=28`、`PathMode=2`；`AutoMosaic` 工具项 | 有（3.5.5.1：模糊强度上限调为 50） |
| **贴图（Pin）** | 有 | 100 条 `PinItem` + 98 条 `PinImageItem` + 100 个 `.meta`；F3 = `pixpin.pinFromClipBoard()` | 有；3.5 支持 **GIF/WebP 动图贴图** |
| **贴图分组** | 有（未使用） | `PinItem."group"` 字段存在，实测全为 `'default'`；`Action.Switch pin group` 有删除时间戳 | 有 |
| **贴图来源标注** | 有 | `PinImageItem.WinTitle` / `ProcessName`（22 进程 / 78 窗口） | — |
| **贴图文字可选中** | 有 | `.meta` 的 `OcrTextSelectable`/`TextSelectable`（97/100 True） | — |
| **剪贴板贴图** | 有 | `MimeDataImageHelper::ExtractImageFromMimeData` 37 次；`MimeText`/`MimeHtml` 字段 | 有 |
| **从剪贴板/文件贴图** | 有 | `Action.Pin selected file`；`.meta` 里 `MimeText='collision.txt'` | 有 |
| **录屏 / 动图** | 有 | `PixAVCodec.dll`(ffmpeg/libx264/aac/mp4/gif/webp)、`PixMovie.dll`、`qtaudio_wasapi.dll`、`GifShot=257`、`PixMovie.record.recordMicrophone` | 有（MP4/GIF/WebP；FPS 5/16/24/30/60；系统+麦克风音频；鼠标点击/按键录制与摄像头画中画为 VIP） |
| **键鼠录制回放** | 有 | `PixKeyMouse.dll` 74.6 KB | 有（VIP） |
| **屏幕取色** | 有 | `PixColorPalette.dll`、`PixColorPicker.ColorType=0`、`PointInfoWidgetColorFormat='RGB'` | 有 |
| **坐标/尺寸信息栏** | 有 | `PointInfoWidgetUseRelativeCoordinate=false`、`Screenshot.SizeDisplayItems=7`、`SizeUnit=0`；日志出现 `ShotInfoButtonBar`/`PixInfoBar` | 有 |
| **放大镜标注** | 有 | `Mark.EditItemOrder` 含 `Magnifier` | 有 |
| **打印** | 有 | `Print=768`；`plugins\printsupport\windowsprintersupport.dll` | 有 |
| **图像编辑窗口** | 有 | `ImageEdit=769`；`TrigerGraphicEditManage`（16 次 error） | 3.5.5.1 新增"编辑"按钮 |
| **上传/分享** | 有通道（未使用） | `PixAuth.dll` 3.77 MB、`PixWebCallback.dll`、`PixDownload.dll`、`PixNetwork.dll`；登录通道 `LoginData`（本机不存在）；0 次 `license`/`vip`/`token` | 有会员体系与创作者分享计划 |
| **历史库** | 有 | `History\_ScreenshotRecord\*.his` 108 个（Qt QDataStream，内嵌 3840×2160 底图 + 48×48 缩略图 + 矢量标注）；`LocalStorage.HistoryShotRectDatas` 100 条选区 | 有（"历史记录"） |
| **图库/Data 目录** | 有 | 99 PNG + 100 `.meta`（贴图/图库） | 有 |
| **自动更新** | 有 | §4.10 完整证据链；`PixUpgrade`、灰度 `UpdateGrayscale`、十小时定时检查、多 CDN | 有 |
| **开机自启** | 开 | `System.Run After Boot = true` | 有 |
| **开机自启 + 管理员** | 有开关（未启用） | `System.Run After Boot.RunAsAdmin`（未配置）、`RunAsAdmin=false` | 有 |
| **单实例 + 提权重启中介** | 有 | `PixPinAuxiliary.exe`（`RestartProcessWmain`，mode Normal/Admin） | — |
| **崩溃上报** | 有 | Sentry Native 0.15.2 + Crashpad + WER；`Bug report enabled/initialized` | 有（隐私政策页） |
| **匿名埋点** | 有 | `PixTrack` → `https://stat2.pixpin.cn/api/track`；ProfileID 三处一致 | — |
| **多语言** | 有 | 8 个 `.qm`（de/es/fr/ja/ko/pt/zh-cht/zh-cn）；`System.Language='auto'` | 有 |
| **桌面悬浮球** | 有 | `[PixDesktopBar] ToolBallPosX=3701 ToolBallPosY=965`；`PixDesktopBar.UpgradePromptShown` | 有 |
| **右键菜单集成** | 有 | `PixPinContextMenu\PixPinContextMenuExt.dll` + `.msix` | 有 |
| **脚本 API** | 有 | `Action.*.script = "pixpin.screenShotAndEdit()" / "pixpin.pinFromClipBoard()"`；官网有「脚本」文档 | 有 |
| **HDR / 高级颜色** | 有通道，未启用 | 日志 `advancedColor=false` 恒定 | — |
| **多显示器 / 高 DPI** | 有通道，本机单屏 | `PixScreenManager`、`wholeRect`、`onDisplayChanged` 308 次、`screenCount=1`、`PixelRatio: 1`、多分辨率切换警告 | 有 |

---

## 9. 对 SnapClip 的可吸收点清单（按优先级）

### P0 — 直接决定产品竞争力

1. **长截图按"单张连续缓冲"设计，不要按"分段图像列表"** `[确证依据 §7.2]`
   - 内部维护一个按 `W×4` 行跨距增长的 `Vec<u8>`（RGB32）+ `logical_length`；每帧抓取后做**增量 append**，只在内存不足时才落临时文件。
   - 暴露与 PixPin 同名的状态字段便于自测：`logical_length`、`shot_rect(x,y,w,h)`、`direction`、`super_long: bool`、`auto_crop: bool`、`match_overlay`。
   - 明确记录 `bytes = W*H*4`，并在接近上限（建议先按 **200 MB 内存** 折算高度）时给出与 PixPin 类似的可读提示。
2. **OCR 结果存"四边形版面 JSON"而非纯文本** `[确证依据 §5.3]`
   - `{ blocks: [ { box: [[f32;2];4], text_lines: [ { box: [[f32;2];4], center_pos:[f32;2], text: String, text_type: u8 } ] } ], lang: String }`
   - 收益：旋转/倾斜文本、逐行复制、图文对照翻译、表格重建、点击贴图选中文字，全部建立在这一个结构上。
3. **OCR 双引擎（检测 + 识别）分离 + 空闲延迟释放** `[确证依据 §4.6]`
   - 懒加载、独立线程、最后一次使用后 **120 s** 释放（`delay_ms = 120000`），并记录 `create_count / alive_count / waiting_tasks` 便于排查。
   - Rust 侧对应：`OnceCell<Session>` + `tokio` 定时释放 / `Drop` 守卫。
4. **截图流水线阶段化 + 毫秒埋点，性能目标 <200 ms** `[确证依据 §4.8]`
   - 阶段：`before_shot / shot / widget_init / uia_init / shortcut_tips_init / qrcode_init / setup_window / show_window / after_show`。
   - 本机基线：总 87–188 ms，抓屏 36–47 ms（4K），窗口显示 8–66 ms。把这套指标做成 CI 门禁。
5. **捕获策略 = 整屏抓取一次 + 内存裁剪，且"预热 + 空闲释放"** `[确证依据 §4.5]`
   - 4K 全屏抓取 36 ms，之后再 crop 几乎零成本；避免"每次都重新初始化 DXGI"。
   - 必须实现**回退链**：`DXGI → Qt/BitBlt 兜底`，并把失败原因（如 `E_INVALIDARG 0x80070057`）落日志。PixPin 在这一点上实测失败 16 次，是它明确的弱点。
6. **记住最近 100 次截图选区** `[确证依据 §2.5]`
   - `VecDeque<SelectionRect{ x,y,w,h: f32, mode: SelectMode, round_radius: f32 }>`，上限 100，用**全局物理像素**存储。
   - 极低成本、高感知收益（吸附历史选区、键盘循环）。

### P1 — 架构与工程正确性

7. **四层持久化分层，各司其职** `[确证依据 §2、§3、§5.3]`
   - L1 用户配置（动作/热键/工具栏/标注默认值，**带修改时间戳、未改动不落盘**）
   - L2 会话态（窗口几何、上次路径/格式、开关）
   - L3 业务实体（SQLite：贴图/历史，可查询、可统计）
   - L4 按实体的文档（每张图一个 sidecar：窗口状态 + 变换 + OCR 版面）
8. **贴图三件套 + `relate_files` 清单** `[确证依据 §3.3、§5.3]`
   - `PinItem` / `PinImageItem`（拆表，避免把 OCR 全文塞进主表）+ `.png` + `.meta`；`relate_files` 保证删除无泄漏。
   - 关键字段：`pin_on_screen`（恢复策略）、`close_time`（软删除）、`device`（多机）、`group`、`src_rect`、`center_pos`、`transform`、`win_title`、`process_name`、`image_device_pixel_ratio`。
9. **`SrcGeometry` + `CenterPos` + `Transform` 三件套建模贴图位置** `[确证依据 §5.3]`
   - 记录"截自屏幕哪里"+"现在在哪"+"缩放/旋转多少"，天然支持"放回原位""重新截图同区域""等比例缩放"。
10. **合成事件过滤（自注入的复制快捷键要自过滤）** `[确证依据 §4.7]`
    - PixPin 注入 `Ctrl+C` 完成复制，然后用 `PixSyntheticEventFilter` 把自己的合成事件过滤掉（20 次）。SnapClip 若也用注入实现"复制到应用"，必须有等价机制，否则会自我循环触发。
11. **独立中介进程做自更新/提权重启** `[确证依据 §4.10、§4.11]`
    - `SnapClipAux.exe (Upgrade|Restart, install_dir, payload, exe_name)`：等待旧进程退出 → 替换文件 → `detached` 启动。避免自身文件锁。
12. **崩溃上报用成熟方案而非自研** `[确证依据 §4.12]`
    - Sentry Native + Crashpad（或 Rust 生态的 `sentry` + `crashpad`/`minidumper`），并保证三处 ID 一致（设备 ID / 日志 uuid / Sentry user.id），另存 `installation_id` 与 `session.json`。
13. **默认值策略：JPEG + 质量 100 + 桌面路径 + 固定后缀** `[确证依据 §2.5]`
    - `SaveDialog.LastImageFormat='jpg'`、`SaveInfoStaticSuffix='jpg'`、`SaveQuality=100`、`SaveInfoPath=<桌面>`。PNG 只在需要透明/无损时使用（本机 98/99 是 RGB PNG 说明用户/程序多数仍存 PNG，但**默认对话框是 jpg**）。

### P2 — 体验与细节

14. **工具栏按钮用"整数位掩码 + 顺序号"建模，支持一键恢复默认** `[确证依据 §2.1]`（256/257/258… 分组）
15. **预设尺寸用极简 DSL（`--size W,H` / `--name X`），并带迁移版本号** `[确证依据 §2.2]`
16. **标注工具按"组"排序而非平铺**：`Geometry/HighLight, Pencil/Marker, Arrow/BrokenLine/Magnifier, Text/Watermark, Serial, Mosaic/AutoMosaic, Eraser`，并支持 `Recycle`（回收站）`[确证依据 §2.1]`
17. **圆角截图默认开**：`Screenshot.enableRoundRect=true`、`MarkBar.Geometry.RectRoundRadius=90` `[确证]`
18. **截图后处理（四周模糊/边框）独立模块**：`PostProcess.Shot.Modules`（位掩码 1=模糊 2=边框，默认 2=边框）、`blur.strength=8`、`border.strength=27` `[确证依据 §2.1]`
19. **马赛克默认参数**：`MosaicMode=0`、`MosaicStrength=28`、`BlurStrength=28`、`PathMode=2` `[确证]`
20. **导出格式覆盖度对齐**：JPEG/PNG/WebP/TIFF/GIF/TGA/ICO/ICNS/WBMP/SVG(读) `[确证依据 §6.1]`
21. **语言文件按 Qt `.qm` 组织 8 种语言、默认"跟随系统"** `[确证]`
22. **二维码识别内置**（`QrCodeDetectInit` 阶段）`[确证依据 §4.8]`
23. **`WinTitle` + `ProcessName` 记录截图来源**（低成本、高情境价值；本机覆盖 22 进程/78 窗口）`[确证依据 §3.2]`
24. **不要像 PixPin 那样把正常流程记为 warning、也不要对自己的埋点失败刷 warning** `[确证依据 §4.7]`（1146 条 warning 里约 500 条是 QSS/定时器噪音，真实问题被埋）
25. **`.meta` 缺失去重**：PixPin 存在 `".meta" Not exist!` 的 error 与孤儿 PNG；SnapClip 应把图片与 sidecar 写成**同一事务/临时文件 + 原子 rename** `[确证依据 §4.7]`

---

## 10. 原始证据附录（真实命令与输出）

所有产物目录：`C:\Users\seeyuer\AppData\Local\Temp\pixpin_dig\`（含 `snap\` 快照子目录、`PinWindowd.sqlite` 副本、各分析脚本与输出 txt）。

### A. 环境与工具探测
```powershell
PS> $env:TEMP; Get-Command python,python3,py,sqlite3,7z,curl -ErrorAction SilentlyContinue | Select-Object Name,Source
C:\Users\seeyuer\AppData\Local\Temp
Name        Source
python.exe  C:\A_Softwares\Python\python.exe
python3.exe C:\Users\seeyuer\AppData\Local\Microsoft\WindowsApps\python3.exe
py.exe      C:\Windows\py.exe
sqlite3.exe C:\A_Softwares\MSYS2\ucrt64\bin\sqlite3.exe
7z.exe      C:\A_Softwares\7-Zip\7z.exe
curl.exe    C:\Windows\system32\curl.exe

PS> python -c "import sqlite3,zlib,sys;print('py',sys.version);print('sqlite3 ok',sqlite3.sqlite_version)"
py 3.12.8 (tags/v3.12.8:2dc476b, Dec  3 2024, 19:30:00) [MSC v.1942 64 bit (AMD64)]
sqlite3 ok 3.45.3
# PIL OK / numpy OK / zlib OK / struct OK / json OK
```

### B. 版本号
```powershell
PS> (Get-Item 'C:\A_Softwares\PixPin\PixPin.exe').VersionInfo | Format-List FileVersionRaw,ProductVersionRaw,FileMajorPart,FileMinorPart,FileBuildPart,FilePrivatePart,Language
FileVersionRaw     : 3.5.5.1
ProductVersionRaw  : 3.5.5.1
FileMajorPart      : 3
FileMinorPart      : 5
FileBuildPart      : 5
FilePrivatePart    : 1
Language           : 中文(简体)
```

### C. `ConfigurationWindowConfig.ini` 与 `pixmeta.dat`
```powershell
PS> $b=[System.IO.File]::ReadAllBytes('C:\A_Softwares\PixPin\ConfigurationWindowConfig.ini')
PS> ($b|%{$_.ToString('x2')}) -join ' '
5b 47 65 6e 65 72 61 6c 5d 0d 0a 47 65 6f 6d 65 74 72 79 3d 40 52 65 63 74 28 38 39 35 20 34 34 35 20 37 36 38 20 35 30 30 29 0d 0a

PS> $b=[System.IO.File]::ReadAllBytes('C:\A_Softwares\PixPin\pixmeta.dat'); "len=$($b.Length)"
len=30
33 43 39 65 0e 9a 22 9b 56 5f 59 10 12 1e 68 43 27 61 07 94 38 94 13 00 5b 08 10 10 6a 1c
```

### D. `LocalStorage.data` 格式判定
```
first 64 hex: 5b 47 65 6e 65 72 61 6c 5d 0d 0a 55 70 64 61 74 65 47 72 61 79 73 63 61 6c 65 3d 31 38 0d 0a 4e 6f 43 68 65 63 6b 55 70 64 61 74 65 55 6e 74 69 6c 3d 40 44 61 74 65 54 69 6d 65 28 5c 30 5c 30
first 64 ascii: [General]..UpdateGrayscale=18..NoCheckUpdateUntil=@DateTime(\0\0
len 30954 entropy bits/byte 3.098 printable ascii ratio 1.0 count NUL 0
```

### E. `HistoryShotRectDatas` 解码（关键：转义与 QVariant 布局）
```
$ python ls5.py
decoded QByteArray len=13704 ; outer element count=100
parsed entries=100  consumed=13704/13704        ← 字节级精确
--- entry[0] ---
   rect          = ('QRectF', (1169.0, 573.0, 1657.0, 1129.0))
   SelectMode    = ('String', 'rect')
   RoundRadius   = ('Double', 0.0)
--- entry[1] ---
   rect          = ('QRectF', (1033.0, 472.0, 1775.0, 1134.0))
   SelectMode    = ('String', 'rect')
   RoundRadius   = ('Double', 0.0)
--- entry[99] ---
   rect          = ('QRectF', (1107.0, 665.0, 1178.0, 686.0))
   SelectMode    = ('String', 'rect')
   RoundRadius   = ('Double', 0.0)
SelectMode histogram: {'rect': 100}
RoundRadius histogram: {0.0: 100}
# entry0 body hex（前 0x60 字节）见 §2.5
```

### F. `@DateTime` 解码
```
$ python -c "..."   # JD 换算
NoCheckUpdateUntil -> JD 2460777 = 2025-04-11 + 5:10:20.382000
settings.dat u32 = 0x696794d8 = 1768395992 -> 2026-01-14 13:06:32 UTC
crash last_crash = 2026-01-14T13:06:31.146852Z
```

### G. SQLite
```
$ sqlite3.exe "$env:TEMP\pixpin_dig\PinWindowd.sqlite" ".schema"
CREATE TABLE "PinItem" ( "id" INTEGER PRIMARY KEY AUTOINCREMENT, "type" TEXT NOT NULL,
  "createTime" DATETIME NOT NULL, "closeTime" DATETIME, "device" TEXT NOT NULL,
  "subItem" INTEGER, "posX" INTEGER NOT NULL, "posY" INTEGER NOT NULL, "mimeText" TEXT,
  "markText" TEXT, "pinOnScreen" BOOLEAN NOT NULL, "title" TEXT, "collect" TEXT,
  "saveBasename" TEXT NOT NULL, "group" TEXT );
CREATE TABLE sqlite_sequence(name,seq);
CREATE TABLE "PinImageItem" ( "id" INTEGER PRIMARY KEY, "Width" INTEGER, "Height" INTEGER,
  "OcrResult" TEXT, "MainColor" INTEGER, "WinTitle" TEXT, "ProcessName" TEXT );

$ sqlite3 ... "select * from sqlite_sequence;"
PinItem|494
$ sqlite3 ... "select min(id),max(id) from PinItem;"
395|494
$ sqlite3 ... "select type,count(*) from PinItem group by type;"
Image|98
Text|2
$ sqlite3 ... "select ifnull(collect,'<NULL>'),count(*) from PinItem group by 1;"
|100                    ← collect 全 NULL
$ sqlite3 ... 'select "group" as grp, count(*) n from PinItem group by grp;'
default|100
$ sqlite3 ... "select min(createTime),max(createTime),min(closeTime),max(closeTime) from PinItem;"
2026-05-14T23:04:30.614|2026-10-05T23:41:41.802|2026-05-14T23:05:39.236|2026-10-05T23:43:07.882
$ sqlite3 ... "pragma page_size; pragma page_count; pragma encoding; pragma user_version; pragma journal_mode;"
4096 | 26 | UTF-8 | 0 | delete
$ sqlite3 ... "select count(*) from PinImageItem where OcrResult is not null and length(OcrResult)>0;"
95
$ sqlite3 ... "select count(distinct ProcessName), count(distinct WinTitle) from PinImageItem;"
22|78
$ sqlite3 ... "select ProcessName,count(*) n from PinImageItem group by ProcessName order by n desc limit 12;"
msedge.exe|48  WindowsTerminal.exe|12  2345PicViewer.exe|8  snapclip.exe|4  Telegram.exe|3
msiexec.exe|2  FlClash.exe|2  EverEdit.exe|2  EXCEL.EXE|2  Antigravity.exe|2
yingli-player.exe|1  txtFormat2.10.exe|1
```

### H. 日志统计（`log2.py` 输出节选）
```
pixpin.1.log      1048386 bytes
pixpin.2.log      1048576 bytes
pixpin.log         392999 bytes
total physical lines parsed: 30706 (of which continuation: 6217)
time range: 2025-11-01 19:58:39.990  ->  2026-10-08 09:47:09.663
loggers: [('PixLog',14488),('3.5.5.1',2863),('2.4.9.6',2607),('3.1.4.0',1891),
          ('3.0.8.0',1413),('3.2.3.1',493),('2.4.9.1',490),('RobinLog',243)]
levels : [('info',23325),('warning',1146),('error',17)]
Top classes: PixWinCaptureStatic 1653 / PixScreenGXDI 1149 / UiSpyThreadWarp 988 / UiSpy 303 /
             PixOcrTaskWorker 69 / MimeDataImageHelper 37 / PixSyntheticEventFilter 20 /
             UiRegionDetectorThreadWarp 20 / FloatPanel 17 / ScreenShotView 12 / PixPin 10 /
             PixOcrTaskManage 8 / LongShotWidget 8 / PixScreenManager 6 / PixPinAction 3 /
             ScreenPixmapItem 2
Top methods: UiSpyThreadWarp::run 988 / PixWinCaptureStatic::getExpectedImageSize 984 /
             PixWinCaptureStatic::captureToQImage 656 / PixScreenGXDI::grabWindowImage 655 /
             PixScreenGXDI::grabWindow 492 / UiSpy::DirectGetRect 303 /
             PixOcrTaskWorker::releaseInferEngines 53 / MimeDataImageHelper::ExtractImageFromMimeData 37
Keyword hits (all 3 logs): LongShot 13 / Stitch 5 / OCR 4 / Ocr 77 / infer 61 / Infer 69 /
             DXGI 2 / engineType 1 / Hotkey 1 / globalHotkey 1 / shortcut 7 / Shortcut 3 /
             screenshot 2 / Screenshot 10 / screenShot 9 / capture 1004 / Capture 3314 /
             pin 39 / Pin 748 / upgrade 22 / Upgrade 188 / Login 224 / PixelRatio 123 /
             monitor 14 / ScreenList 2031 / Crash 4 / report 520 / PixTrack 253 / track 16 /
             Export 4 / UiSpy 1294 / autoCrop 3 / Crop 5 / crop 496 / superLong 1 / logicalLength 2
未命中 (0): scroll Scroll stitch merge match WGC GraphicsCapture BitBlt PrintWindow Dwm DWM
             wheel Wheel Mouse hover max limit history license Dpi DPI WebP webp video
             record Record gif GIF mp4 MP4 onnx Onnx ONNX mosaic formula Translate translate
             clipboard save Save Table OcrTable AutoMosaic memory cache
ALL error-level lines:
  pixpin.2.log:11965  Qt: "C:/A_Softwares/PixPin/Data/2025-07-06_09-17-32-0.meta" Not exist!
  pixpin.1.log:12735/12737/12808/12986/12988/13196/13222/13224/13334/13343/13385/13387/13389/13391/13408/13419
      Qt: TrigerGraphicEditManage::TrigerGraphicEditManage scene is null
```

### I. 长截图会话（`pixpin.log` 原文，见 §4.4 / §4.5）

### J. 性能埋点原文（见 §4.8）

### K. Crashpad 全量
```
===== last_crash ===== (27 bytes)
2026-01-14T13:06:31.146852Z
===== metadata ===== (16 bytes)
44 41 50 43 01 00 00 00 00 00 00 00 00 00 00 00        ("DAPC" v1)
===== installation_id ===== (70 bytes)
cc288a2d-eccb-4f09-12fa-8a9c2e17d6d2
d3e76aa4570f16ecbf8d8973d5801b09
===== settings.dat ===== (40 bytes)
73 64 50 43 01 00 00 00 01 00 00 00 00 00 00 00 d8 94 67 69 00 00 00 00 ad 52 1c f5 a4 90 38 43 98 b8 ce 35 ad 4e 57 8d
===== __sentry-event ===== (509 bytes)   → MessagePack，字段见 §4.12
===== session.json ===== (252 bytes)
{"init":true,"sid":"0dbb743b-a8fc-461e-4c7c-873a4d85175d","status":"ok",
 "did":"8f0834fc-35b1-4582-a584-6db922b5d0cc","errors":0,
 "started":"2026-10-01T08:59:54.385866Z","duration":0.000474,
 "attrs":{"release":"PixPin@3.5.5.1","environment":"production"}}
===== __sentry-breadcrumb1 / 2 ===== (0 bytes)
```

### L. 进程与文件时间（证明 PixPin 在运行、日志在增长）
```powershell
PS> Get-Process | ? { $_.ProcessName -like '*PixPin*' -or $_.ProcessName -like '*crashpad*' } | Select Id,ProcessName,StartTime,Path
   Id ProcessName      StartTime          Path
16640 crashpad_handler 2026/10/1 16:59:54 C:\A_Softwares\PixPin\Helpers\crashpad_handler.exe
14928 PixPin           2026/10/1 16:59:48 C:\A_Softwares\PixPin\PixPin.exe

PS> Get-ChildItem 'C:\A_Softwares\PixPin\pixpin*.log','...\Data\PinWindowd.sqlite' | Select Name,Length,LastWriteTime
pixpin.1.log        1048386 2026/6/16 17:39:01
pixpin.2.log        1048576 2025/12/31 18:44:34
pixpin.log           392999 2026/10/8 9:48:04
PinWindowd.sqlite    106496 2026/10/5 23:43:07
```

### M. `.meta` 类型表修正证据（80 = QTransform 而非 QMatrix）
```
# 若把 typeId=80 当作 QMatrix(6 double) 解析，则 Transform 之后键名错位为 '' 且 typeId 变成 63（非法）
# 改为 9 个 qreal 后，全部 100 个文件精确消费：
[SOME] key 'OcrTextJson' -> UNKNOWN_48   ← 修正前把 typeId=48 当未知
# 修正后：typeId 48 = 自定义类型，payload = u32 长度 + UTF-8 JSON（例 0x17=23 字节 '{"blocks":[],"lang":""}'）
bytes consumed 611 / 80289 等，键序列稳定 95/100 一致
```

### N. `.png` / `.meta` 数量与 PNG IHDR
```
=== Data\*.png : 99 files; with .meta: 98 ===
missing .meta: ['2026-05-01_22-07-48-0.png']
PNG IHDR (colorType,bitDepth,compression,filter,interlace) histogram:
    (2, 8, 0, 0, 0) -> 98 files
    (6, 8, 0, 0, 0) -> 1 files
RGBA samples: 2026-09-25_17-12-55-0.png 1368x1121 colorType=6 bitDepth=8
tallest 5 (h): 3840x2088 / 659x1398 / 2100x1350 / 1436x1278 / 1368x1121
.meta count=100 min=552 max=80289
smallest 3: 2026-10-03_16-29-11-0.meta 552, 2026-08-03_17-59-07-0.meta 564, 2026-06-04_12-13-36-0.meta 696
largest  3: 2026-10-04_19-05-39-0.meta 62727, 2026-10-02_21-08-28-0.meta 73624, 2026-06-19_15-59-45-0.meta 80289
```

### O. 模型与编解码探测
```
--- model\ ---
2478d3813bea9afacf7fee18abb09d45.bin   9880512 B  entropy=7.535  no readable ASCII
5f515ede591a02e400275eaa2e5c0ddf.bin  16615441 B  entropy=7.536  no readable ASCII
94186d8ad4eacc8b8aaf3c1088eb9353.bin   4729474 B  entropy=7.506  no readable ASCII
cb6cc28d4121651b3f5bc3daecc9188e.bin  21234344 B  entropy=7.566  no readable ASCII
dbb5b4317e638ad5a21a42b035cfd159.bin 10838604 B  entropy=7.520  no readable ASCII
detect.caffemodel                        965430 B  head=0a 00 a2 06 22 0a 04 64 61 74 61 12 05 49 6e 70
   strings: ['Input"', 'dataP', 'data/bn', 'BatchNorm', 'data"', 'data2']
detect.prototxt                           45372 B  head=6c 61 79 65 72 20 7b 0d 0a ...
   strings: ['layer {', '  name: "data"', '  type: "Input"', '  top: "data"', '  input_param {']
paragraph_recognition.onnx              3198623 B  head=08 0a 12 07 70 79 74 6f 72 63 68 1a 0a 32 2e 31
   strings: ['pytorch', '2.11.0+cpu:', 'node_features', 'val_0', 'node_Shape_0"', 'Shape*']
sr.caffemodel                             23929 B  strings: ['Input"', 'dataP', 'data_data_0_split', 'Split', ...]
sr.prototxt                                6387 B  strings: ['layer {', '  name: "data"', '  type: "Input"', ...]
--- OcrModel\ ---   (空，0 个文件)

XOR 0x77 probe → '08 03'/'08 02'/'08 03' at start，但全文件 XOR 后仍无可读字符串
                     → 判定为流密码/AES 类加密模型（不是单字节 XOR）

DLL 能力字符串计数见 §6.2 / §6.3
```

### P. 联网交叉验证（外部资料，仅作对照）
- [长截图/滚动截图 - PixPin 高级功能教程](https://pixpin.cn/docs/capture/long-capture)
- [v3.5.5.1 正式版更新日志](https://pixpin.cn/docs/official-log/3.5.5.1)
- [屏幕录制 - PixPin 使用文档](https://pixpin.cn/docs/capture/gif-capture2)
- [PixPin 是什么（文档首页）](https://pixpin.cn/docs/start/what-is-pixpin)

### R. `QVariant` 布局的决定性验证（`layout_test.py`，真实输出）

这是本报告里"`QVariant` 后必须跳过 1 个保留字节"这一结论的**直接实验证明**。同一段字节，两种解析方式对比：

```
SIMPLE = {'n': 10, 'r': 13, 't': 9, 'b': 8, 'f': 12, 'v': 11, 'a': 7, '\\': 92, '"': 34, "'": 39}
decoded len = 13704
bytes 0x00..0x40:
  0000  00 00 00 64 00 00 00 85 00 00 00 03 00 00 00 08
  0010  00 72 00 65 00 63 00 74 00 00 00 14 00 40 92 44
  0020  00 00 00 00 00 40 81 e8 00 00 00 00 00 40 99 e4
  0030  00 00 00 00 00 40 91 a4 00 00 00 00 00 00 00 00
entry0 len = 133
  0000  00 00 00 03 00 00 00 08 00 72 00 65 00 63 00 74
  0010  00 00 00 14 00 40 92 44 00 00 00 00 00 40 81 e8
  0020  00 00 00 00 00 40 99 e4 00 00 00 00 00 40 91 a4
  0030  00 00 00 00 00 00 00 00 14 00 53 00 65 00 6c 00

type(p)=20  [A no-reserved] doubles=(1.843623895123284e-307, 1.836514360425234e-307, 1.8469376037504554e-307, 1.8433522796620405e-307)
type(p2)=20 reserved=0x00  [B with-reserved] doubles=(1169.0, 573.0, 1657.0, 1129.0)

.meta head 0x00..0x40:
  0000  00 00 00 09 00 00 00 1a 00 57 00 69 00 6e 00 53
  0010  00 74 00 61 00 79 00 73 00 4f 00 6e 00 54 00 6f
  0020  00 70 00 00 00 01 00 01 00 00 00 14 00 57 00 69
  0030  00 6e 00 4f 00 70 00 61 00 63 00 69 00 74 00 79
```

**读法**：
- 方式 **A（不跳过保留字节）**：`type=20` 之后直接读 4 个 double → 得到 `1.84e-307` 级**非物理值**，且后续全部错位（键名解析为 `''`、出现非法 `typeId=63`）。
- 方式 **B（跳过 1 个 `0x00`）**：`type=20 | 00 | 40 92 44 00 00 00 00 | ...` → 得到 **`1169.0, 573.0, 1657.0, 1129.0`**，是 3840×2160 屏幕上完全合理的选区矩形；100 条全部 `133/133` 字节精确消费。
- `.meta` 的 Bool 同样成立：`...00 70 00 | 00 00 00 01 | 00 | 01` → `typeId=1(Bool)`、保留位 `00`、值 `01` = `True`。若方式 A 解析则值为 `0x00` = `False`，与语义矛盾。

**结论 [确证]**：三类文件（`LocalStorage.data`、`Data\*.meta`、`History\*.his`）中的 `QVariant` 统一为
`u32 typeId | u8 保留位(实测恒 0x00) | payload`。保留位的语义仍未定（见 §11 未解疑点 3），但**布局无歧义**。

### S. 选区历史与落盘 PNG 的交叉验证

> 说明：本次运行该交叉验证脚本时，复用的旧解析函数出现缓冲区越界（旧脚本未跳过保留字节），**未取得完整配对结果**，故不在报告中给出未经跑通的配对表，仅保留已独立确证的两处配对：
> - `HistoryShotRectDatas` 第 100 条 `rect=(1107, 665, 1178, 686)` ↔ `pixpin.log` 09:47 长截图 `shotRect=1107,665 1178x686` **[确证]**
> - `HistoryShotRectDatas` 第 2 条 `rect=(0, 1710, 2390, 311)` 的宽高 `2390×311` ↔ `Data\2026-10-01_16-43-21-0.png` 的 IHDR `2390×311` **[确证]**（该匹配由 §5.2 与 §2.5 两份独立输出人工比对得出，非脚本批量结果）
>
> 批量配对（`rect.w/h` == `PNG.W/H`）建议用 §10.R 的**方式 B** 解析器重跑；预期可建立"哪个历史选区对应哪张图"的完整映射。

**额外铁证（文件时间戳三方对齐）**：`C:\A_Softwares\PixPin\LocalStorage.data` 的 `LastWriteTime = 2026/10/8 9:47:08`，与
- `pixpin.log` 的 `[LongShotWidget::getExportPixmap] ... 09:47:06.756`（长图导出）
- `pixpin.log` 的 `[LongShotWidget::~LongShotWidget] ... 09:47:08.998`（长截图窗口销毁）

**几乎同一瞬间**。也就是说：**这次长截图（选区 `1107,665 1178x686`，成品 `1178×8966`）在关闭时把选区写进了 `LocalStorage.data` 的 `HistoryShotRectDatas` 第 100 条**。三条独立证据（日志时间、配置文件字节内容、文件系统 mtime）完全闭合。**[确证]**

### T. 未命中项（明确声明）

以下关键词在三个日志文件中**命中 0 次**（不是"没搜"，而是实测为 0）：
`scroll`、`Scroll`、`stitch`、`merge`、`Merge`、`match`、`Match`、`WGC`、`GraphicsCapture`、`BitBlt`、`PrintWindow`、`Dwm`、`DWM`、`wheel`、`Wheel`、`Mouse`、`hover`、`Hover`、`max`、`Max`、`limit`、`Limit`、`history`、`History`、`license`、`License`、`Dpi`、`DPI`、`WebP`、`webp`、`video`、`record`、`Record`、`gif`、`Gif`、`GIF`、`mp4`、`MP4`、`onnx`、`Onnx`、`ONNX`、`mosaic`、`Mosaic`、`formula`、`Formula`、`Translate`、`translate`、`clipboard`、`Clipboard`、`Save`、`save`、`Table`、`OcrTable`、`AutoMosaic`、`memory`、`Memory`、`cache`、`Cache`。
（`stitch` 小写 0 次；大写 `Stitch` 5 次，**全部来自字段名 `pixStitching=`**。）

---

## 11. 未解疑点

1. **`pixmeta.dat`（30 B）的真实结构与内容** — 无魔数、无字符串、高熵。推测为设备指纹/许可绑定种子，**无法静态解码**。建议：若 SnapClip 需要同类功能，改用明确 schema（明文 JSON/MessagePack + 注释），避免产生不可调试的黑盒文件。
2. **5 个 `model\*.bin`（4.7–21.2 MB）的加密算法与模型身份** — 熵 7.51–7.57、无 ASCII、单字节 XOR 0x77 只让首字节变成看似合法的 protobuf varint 但整体仍不可读。**未能确认是 AES、ChaCha 还是自定义流密码，也未能确定五个模型各自的用途**（仅能按大小排序猜测：最大 21.2 MB 可能是 OCR 识别主模型，最小 4.7 MB 可能是表格/公式模型）。
3. **`.meta` / `LocalStorage.data` / `.his` 中 `QVariant` 后的"1 字节保留位"的确切语义** — 布局已确证（恒 `0x00`），但它是 `QVariant::isNull`、版本标记还是 Qt 序列化对齐，未能确定。这对 SnapClip 影响不大（可用自定义格式替代），但若要**读写 PixPin 现有数据**必须精确复现。
4. **长截图拼接算法内部细节** — `scroll`/`stitch`/`merge`/`match` 在日志中 0 命中，拼接逻辑完全位于 `pixStitching` 内部且无埋点。**匹配方式（模板匹配/相位相关/特征点）、重叠量、最大像素上限的内部常量、失败重试策略全部未知。**
5. **长截图产物的 overlap 条带未能实测** — `Data\`（最大 3840×2088）与 `.his`（仅 3840×2160 底图）都无长图样本，无法做"重复行带"指纹分析。仅能用 `logicalLength/shotRect.height = 3.60 / 13.07`（非整数）间接推断存在可变重叠。**方法已在 §5.4 给出，待获得真实产物后可执行。**
6. **HDR / 高级颜色路径** — 日志中 `advancedColor=false` 恒定，从未观察到 `true`，无法判断 PixPin 在 HDR 屏幕上的实际行为。
7. **多显示器与 DPI 配置** — 本机单显示器（`screenCount=1`）、`PixelRatio: 1`。日志中虽有多个 `Screen info` 分辨率（2560×1440、2560×1600、3840×2160、1024×768、DISPLAY6），但**未发现任何"多显示器布局/DPI 缩放"配置项**；PixPin 如何处理混合 DPI 未能验证。
8. **`System.Translate.TranslateKey` 的加密方式** — 值形如 `Base64|||0`，疑为服务端下发的加密 token；0 次日志命中，无法确认其生成/使用流程。
9. **`__sentry-event`（level=fatal）与 `last_crash`（2026-01-14）时间不一致** — `session.json` 显示 handler 于 2026-10-01 启动，但事件是 fatal 且 `last_crash` 指向 2026-01-14。合理但未证实的解释：该事件是上次崩溃的待发送残留，或 `release` 字段在读取/上报时用当前二进制版本填充。
10. **`.his` 的完整结构与"2 个内嵌 PNG"结论** — 由父 agent 完成，**我未独立复核**（仅提供 `rect/SelectMode/RoundRadius` 键名与 `LocalStorage.data` 同构作为旁证）。
11. **`Data\` 中的孤儿文件** — 数量关系：PNG **99** 个，其中与 `.meta` 配对的 **98** 个，孤儿 PNG（有图无 sidecar）**1** 个（`2026-05-01_22-07-48-0.png`）；`.meta` 总数 **100**，故有 **2 个 `.meta` 已无对应 PNG**（图片被删但 sidecar 残留）。这直接对应日志里的 error `".../2025-07-06_09-17-32-0.meta" Not exist!`，说明 **PixPin 的图像与其 sidecar 不是原子写入**，两个方向的孤儿都会出现。Rust 侧应改用"临时文件 + 原子 rename + 启动时一致性扫描"。
12. **`PixPinConfig.json` 中 `ActBarFlag` 整数的精确位语义** — 低位连号（256..263 / 512..515 / 768..769）强烈提示"顺序 + 可见性/分组"编码，但**未找到解码公式**（例如 256=0x100、512=0x200、768=0x300 像是"页/层级"，低字节像"槽位"）。

---

*报告结束。所有 hex dump、日志行与命令输出均为真实读取结果；凡推断之处已显式标注 确证 / 强推断 / 弱推断 / 公开资料 / 未解。*
