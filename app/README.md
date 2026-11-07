# Arboria.app（ADR 0004 的 Swift 薄壳）

打开项目、换 idea、编辑 idea 的 notebook、在 idea 的 worktree 里和 agent（用户自己的 `claude`）对话。布局和样式按 Arboria 设计系统（P2）：标题栏只有面包屑 `项目 › idea ▾`；没打开 idea 时是“待你处理”首页；⌘P 换 idea，⌘J 开关 agent 面板（默认收起），⌘0 回首页。

```
app/build.sh                                   # 只要 Command Line Tools；产出 app/build/Arboria.app
open app/build/Arboria.app --args /path/to/project   # 或不带参数，弹窗选项目
```

- WKWebView：我们的页面 `arboria://app/index.html`（标题栏；首页和 ⌘P 面板时铺满窗口，透明地盖在 notebook 上）、notebook（arbd 起的 `marimo edit`，127.0.0.1 + token）、agent 面板（xterm.js，每个 idea 一个 pty 跑用户自己的 `claude`，cwd 是 worktree，第一次打开面板时才启动）。
- ⌘P、⌘J、⌘0 由窗口先处理（`AppWindow`），因为 marimo 自己绑了 ⌘P、⌘J，网页又先拿键（P0 ②）。
- 样式：`design/` 是设计系统 `tokens.css`、`bundle.css` 的逐字副本，`design/derive.py` 在构建时生成跟随系统外观的 `tokens.css` 和给 marimo 用的 `--ab-*` 版本。`web/marimo.css` 是 marimo 的全部覆盖（选择器只在这一个文件里，对着 marimo 0.25.2）：它的变量映射到我们的 token、全部用 Maple Mono CN、cell 是 `fill` 块、输出直接在底上、不用的界面（文件名、侧栏、页脚、菜单、AI 入口）隐藏。它也注入进每个 shadow root（marimo 的控件在里面），并让 marimo 的亮暗 class 跟随系统。marimo 自带的 AI 只是在界面上藏起来，要彻底关掉还得看它的配置。
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
| `ARB_PROBE=1`、`ARB_PROBE_OUT`、`ARB_PROBE_PROMPT` | 无人值守验证：首页 → 第一个 idea 的 notebook（上、下）→ ⌘P 面板 → agent 面板，每步存整窗截图并打印页面内容，可往 agent 输入一句话，然后退出。窗口被遮住时 WebKit 不画 xterm，agent 截图会是空的 |
| `ARB_APPEARANCE=light\|dark` | 强制外观（截两套图用） |
