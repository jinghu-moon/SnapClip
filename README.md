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

贴图由 `crates/snapclip-pin` 管理：F3 图片、文档与文件使用 GPUI/Yue 窗口，图片预览复用 YueImagePreview；F6 原位截图保留原生 Win32 宿主。应用负责 F3/F6、来源准备和系统复制，能力 crate 负责展示及生命周期。边界与 API 见 [51 · 贴图能力独立](docs/51-pin-crate.md)。

F3 已接入当前剪贴板：图片、普通/富文本、表格、代码、链接、路径和文件集合按内容展示；F6 保留最近截图的一次性消费。复制、模式切换、文件预览和撤销关闭已实现；Windows 热键、来源应用、DPI 和读屏仍需实机验收。实现范围、测试证据和未交付的原型探索见 [52 · F3 剪贴板贴图](docs/52-clipboard-pin-design.md#11-实现与验证记录2026-11-03)。

运行实际应用：`cargo run -p snapclip-app`。仅看生产贴图窗口：`cargo run -p snapclip-pin --example pin_preview -- table`；可选 `text / rich / table / code / link / path / files / image`。预览的复制反馈为模拟，不读写系统剪贴板；实际应用才执行系统复制。

HDR 捕获、原始母版、保存面板、F3／F6 与剪贴板的改造计划见 [53 · HDR 接入方案](docs/53-hdr-capture.md)，通用 GPUI 输出方案在相邻 Yue 仓库。当前为待实施设计，不表示这些链路已经支持 HDR。
