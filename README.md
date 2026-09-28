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
