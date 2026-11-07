# Arboria.app（ADR 0004 的 Swift 薄壳）

P1：打开项目、在侧栏切 idea、编辑 idea 的 notebook、在 idea 的 worktree 里和 Claude 对话。界面还没设计（P2）。

```
app/build.sh                                   # 只要 Command Line Tools；产出 app/build/Arboria.app
open app/build/Arboria.app --args /path/to/project   # 或不带参数，弹窗选项目
```

- 三个 WKWebView：侧栏（`arboria://app/index.html`）、notebook（arbd 起的 `marimo edit`，127.0.0.1 + token）、Claude 终端（xterm.js，每个 idea 一个 pty 跑用户自己的 `claude`，cwd 是 worktree）。
- `arboria://` 只读 app bundle 里的文件（`Scheme.swift`）；侧栏只能导航到 `arboria://app`，notebook 只能留在它的 marimo 服务器上，外部链接交给默认浏览器。
- 核心是 Python 的 `arb arbd`（`src/arboria/arbd.py`），经登录 shell 启动，所以要在终端里能运行 `arb`。
- 起 `claude` 和 `arbd` 前清掉 `CLAUDECODE` / `CLAUDE_CODE_*`（P0 ④）。
- 新 worktree 第一次开终端时，Claude Code 会问是否信任这个文件夹，由人回答。

环境变量（开发用）：
| 变量 | 作用 |
|---|---|
| `ARBORIA_PROJECT` | 项目路径（代替参数 / 选择框） |
| `ARBORIA_ARBD_CMD` | 代替 `arb`，例如本仓库的开发版 |
| `ARBORIA_CLAUDE_CMD` | 代替 `exec claude` |
| `ARB_PROBE=1`、`ARB_PROBE_OUT`、`ARB_PROBE_PROMPT` | 无人值守验证：依次选前两个 idea，记录每个面板的文字和截图，可往第一个终端输入一句话，然后退出 |
