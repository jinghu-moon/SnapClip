# SnapClip

SnapClip 是面向 Windows 的截图、标注、OCR、贴图和剪贴板历史工具。

项目采用 Tauri 2、Rust 2024、Vue 3、TypeScript 和 SQLite，具体职责与阶段计划见：

- [架构文档 v2](docs/01-snapclip-architecture-v2.md)
- [技术选型方案](docs/02-snapclip-technology-selection.md)

## 开发环境

- Node.js 与 npm
- Rust stable toolchain
- Windows WebView2 Runtime
- Windows 桌面开发工具链（Tauri 2）

## 开发命令

```bash
npm ci
npm run dev
npm run typecheck
npm run build
npm run tauri dev
```

Rust 依赖位于 `src-tauri/`。发布、修改和网络交互相关许可义务以根目录 `LICENSE` 中的 GNU Affero General Public License v3 为准。

## GPUI / Yue 保存面板

普通选区的 GPUI 保存面板已迁移到 Yue 组件。接入依赖、与 HTML 原型的调整、独立预览命令和验证边界见 [50 · Yue 保存面板](docs/50-yue-save-panel.md)。开发时需要相邻的 `GPUI-Yue-Component` 工作区及消费根目录中的 Base / GPUI core / Windows后端patch；旧 Tauri 链路不因此移除。

## 独立贴图能力

贴图由 `crates/snapclip-pin` 管理：F3 图片、文档与文件以及 F6 原位截图共用 GPUI/Yue 宿主，图片预览复用 YueImagePreview。应用负责 F3/F6、来源准备和系统复制，能力 crate 负责展示及生命周期。边界与 API 见 [51 · 贴图能力独立](docs/51-pin-crate.md)及 [53 · HDR 接入方案 §16](docs/53-hdr-capture.md#16-f6-共用-gpui-yue-宿主p3-i2026-11-03)。

F3 已接入当前剪贴板：图片、普通/富文本、表格、代码、链接、路径和文件集合按内容展示；F6 保留最近截图的一次性消费。复制、模式切换、文件预览和撤销关闭已实现；Windows 热键、来源应用、DPI 和读屏仍需实机验收。实现范围、测试证据和未交付的原型探索见 [52 · F3 剪贴板贴图](docs/52-clipboard-pin-design.md#11-实现与验证记录2026-11-03)。

运行实际应用：`cargo run -p snapclip-app`。仅看生产贴图窗口：`cargo run -p snapclip-pin --example pin_preview -- table`；可选 `text / rich / table / code / link / path / files / image`。预览的复制反馈为模拟，不读写系统剪贴板；实际应用才执行系统复制。

HDR 捕获、原始母版、保存面板、F3／F6 与剪贴板的分批实现与验证记录见 [53 · HDR 接入方案](docs/53-hdr-capture.md)，通用 GPUI 输出方案在相邻 Yue 仓库。内部颜色与有界导出已有软件验证，真实显示／跨屏／DPI／VRAM 及完整工作流仍未验收，生产 HDR 捕获门禁继续关闭。

可选 CPU AVIF 接线：`cargo run -p snapclip-app --features avif`。HDR 保存面板增加 PQ10 AVIF，历史支持从 SCPX 行带原子导出，贴图与保存预览共用 `snapclip-avif` 解码器；Yue 的 MIT 依赖图不引入 AVIF codec。AVIF 是量化导出，原始 FP16 母版不变。交互解码仍限 64 MiB／24MP，长图导出使用独立有界行带预算。该 feature 默认关闭，不能据此认定 HDR 捕获、显示质量或编码延迟已经达标。

CPU 资格入口：`powershell -NoProfile -File tools/test-hdr-avif.ps1 -Ffmpeg <ffmpeg.exe> -GridResources -BandResources -SpoolResources`。包含独立格式、颜色、Alpha、异常元数据和磁盘后备资源验证。`tools/check-hdr-codec-licenses.py` 校验固定源码／许可清单；`--bundle <目录>` 生成源码、原文、补丁和 SPDX 审计包。若仍缺原始许可正文，打包会标记 `NOT-RELEASE-READY` 并退出 1；当前 `simd_helpers 0.1.0` 的这一问题尚未解决，不能将审计包直接作为完整发行交付。
