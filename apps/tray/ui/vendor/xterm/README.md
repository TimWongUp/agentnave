# xterm.js（内嵌终端用，ADR-0103）

从 npm 包原样复制的构建产物，只删去 `sourceMappingURL` 行：

| 文件 | 包 | 版本 |
| --- | --- | --- |
| `xterm.js`、`xterm.css` | `@xterm/xterm` | 6.0.0 |
| `addon-fit.js` | `@xterm/addon-fit` | 0.11.0 |
| `addon-webgl.js` | `@xterm/addon-webgl` | 0.19.0 |

许可证均为 MIT，见 `LICENSE`。升级时三个包一起换，并在预览版里目测终端。
