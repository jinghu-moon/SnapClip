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

普通选区的 GPUI 保存面板已迁移到 Yue 组件。接入依赖、与 HTML 原型的调整、独立预览命令和验证边界见 [50 · Yue 保存面板](docs/50-yue-save-panel.md)。开发时需要相邻的 `GPUI-Yue-Component` 工作区及消费根目录中的 Base / GPUI patch；旧 Tauri 链路不因此移除。

## 独立贴图能力

贴图窗口已迁入 `crates/snapclip-pin`。应用仅接入截图来源和轮询生命周期事件；Win32 宿主、拖动、缩放、圆角与资源回收由能力 crate 自己拥有。边界、API、测试命令与验收限制见 [51 · 贴图能力独立](docs/51-pin-crate.md)。
