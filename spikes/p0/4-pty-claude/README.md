# P0 ④：xterm.js + pty 跑用户自己的 `claude`、续会话

`./build.sh`（只用 Command Line Tools；xterm.js 从 npm 取 `@xterm/xterm@5.5.0` 放进 `build/`，不提交）。
`SPIKE_SCRIPT='延迟:文字|延迟:SNAP'` 让 app 自动往 pty 打字并保存屏幕文字和截图。

```
SPIKE_CWD=<目录> SPIKE_OUT=<输出> SPIKE_QUIT=1 SPIKE_ARGS="--strict-mcp-config --disable-slash-commands" \
SPIKE_SCRIPT='8:\r|6:Remember the codeword: heron-42. Reply with exactly: noted\r|25:SNAP' \
  build/ClaudeTerm.app/Contents/MacOS/ClaudeTerm
# 再用 SPIKE_ARGS="… --continue" 问 codeword
```

## 结论（2026-11-06，claude 2.1.318，订阅登录）
| 项 | 结果 |
|---|---|
| pty | ✅ `openpty` + `posix_spawn`（`POSIX_SPAWN_SETSID`，在子进程里重新 open slave，这样它就是控制终端）+ `TIOCSWINSZ`。不用 `fork`。经 `zsh -lc 'exec claude …'` 启动，拿到用户的 PATH。 |
| xterm.js | ✅ Claude Code 的 TUI（logo、输入框、颜色、状态行）完整渲染；按键 `term.onData` → `WKScriptMessageHandler` → 写 pty；输出 base64 → `term.write`。字体用 `arboria://fonts/` 的 Maple Mono CN。 |
| 订阅 | ✅ 状态行显示 “Claude Ultra”，用的是用户自己的登录，app 不碰 token。 |
| 续会话 | ✅ 第一次让它记住 `heron-42`；新进程 `claude --continue` 后问 codeword，答 `heron-42`。`--resume <id>` 走同一套 transcript，可用。 |
| **环境变量** | ⚠️ 必须清掉父进程里所有 `CLAUDECODE` / `CLAUDE_CODE_*`：继承了 `CLAUDE_CODE_CHILD_SESSION` 时 claude 会关掉 transcript 保存，`--resume` 就失效。从 Finder 启动的 app 本来没有这些变量，但从终端启动开发版时会有。 |
| 上下文 | 交互会话也一样：用户全局的 MCP / skills 会全部加载（见 ⑦）。给 idea 开的终端要决定带不带 `--strict-mcp-config`，或者只用 `--mcp-config` 给 arboria 自己的。 |

### 留给 P1
- xterm.js 现在是固定 110×34，要加 fit addon，窗口大小变了就发 `TIOCSWINSZ`。
- 新目录第一次运行可能有“信任这个文件夹”的对话框（这次在临时目录下没出现）；worktree 是 arboria 建的，P1 看能不能预先信任，或者第一次让人自己点。
- 后台窗口里 `takeSnapshot` 可能不是最新画面，屏幕文字以 `term.buffer` 为准。
