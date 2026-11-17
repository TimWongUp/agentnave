# dockview（终端页的分屏布局，ADR-0101）

从 npm 原样复制的构建产物，只删去 `sourceMappingURL` 行，许可证 MIT，见 `LICENSE`：

- `dockview-core.min.noStyle.js`：`dockview-core` 8.9.0 的 `dist/dockview-core.min.noStyle.js`（UMD，全局 `window["dockview-core"]`，不往页面插样式）。
- `dockview.css`：`dockview` 8.9.0 的 `dist/styles/dockview.css`。

终端页用 `dockview-theme-abyss-spaced` 的结构，颜色在 `terminal.css` 里换成 App 的主题变量。升级时两个文件同版本一起换，并在预览版里目测分屏、拖拽和标签页。
