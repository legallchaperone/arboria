# 设计系统的副本

`tokens.css` 和 `bundle.css` 逐字复制自 Arboria 设计系统（https://claude.ai/artifact/LeSPASKLSUE39HtVTiiGfk ，`project/tokens.css`、`project/components/bundle.css`）。设计系统改了就重新复制，不在这里手改。

`derive.py` 在构建时（`app/build.sh`）从 `tokens.css` 生成 app 用的两份：
- `tokens.css`：亮色是 `:root`，暗色放进 `@media (prefers-color-scheme: dark)`，跟随系统外观；字体 URL 换成 `arboria://fonts/…`。
- `ab-tokens.css`：同样的变量加 `--ab-` 前缀，给注入 marimo 的样式表用（marimo 自己有 `--primary`、`--accent` 等同名变量）。
