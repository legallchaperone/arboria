---
title: 安全模型
description: Arboria 防什么、不防什么，以及每一层护栏具体做了什么。
---

Arboria 防的是 **“Claude 顺手照做”**：会话里一句话、一段日志或一个命令文件让 Claude 自己去提交作业、批准、改你的 notebook。它**不是**针对同一台机器上、以你的身份运行的蓄意对手的沙箱：理论上，同一台机器上的进程可以直接改数据库。

| 层 | 做什么 |
| --- | --- |
| 账本 | 只有 `arb` 能写；写入一律加锁（fcntl + `BEGIN IMMEDIATE`）。LLM 只能写少数自由文本字段，都有长度校验；其余字段由代码生成和校验。 |
| 唯一的提交路径 | 只有 `arb approve` 会调用后端的提交，而且提交前代码再查一次账本里有你的批准 Decision。没有任何自动批准的代码路径；`lab` 里没有批准函数。测试用源码扫描和 spy 守着这一点。 |
| 只在你的终端里批准 | `arb approve` / `arb lesson accept` 在有 `CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT`、stdin 不是 TTY 或正在运行 notebook 命令时拒绝，然后要你输入 id 确认。 |
| 审核单哈希 | 批准绑定在审核单哈希上（快照、命令、资源、账户、机器、分区、分段计划、环境、项目 id）；任何一项变了，`arb approve` 就拒绝。 |
| Claude Code 权限 | `install-claude` 把 `sbatch`/`srun`/`salloc`/`scancel`/`ssh`/`scp`/`rsync`、`arb approve`、`arb lesson accept`、改 notebook 文件的 `arb idea style` 和只写种子的 `arb idea plant/set/revive`（以及 `arb seed …`）设为 deny，并禁止 Claude 编辑 notebook、账本、备份、`.arboria/` 里的 toml 和会话记录（可以读）。 |
| 只由你写的记录 | `arb note`、`arb idea style`、写种子的命令（仓库外的 `arb idea new/set/drop/revive`、`arb idea plant`，旧写法 `arb seed …`）、`lab.todo`/`lab.conf`/`lab.metric`/`lab.prune` 在 Claude Code 里拒绝。 |
| 远端 | `SshSlurm` 只复用你登录好的 ssh 主连接（BatchMode，不弹 Duo），只认本项目的作业名，读写路径限制在本项目的 `<runs>/<project>`、`<out>/<project>` 下。 |
| 快照 | 不动你的分支、暂存区和工作区（`tests/test_snapshot.py` 是这条的契约）。 |
| 看板 | 只读：账本以 `mode=ro` 打开，不起进程、不联网、没有会动手的控件；文字 HTML 转义。 |
| notebook 命令 | Claude 写、在你的 kernel 里以你的权限运行；`arb` 自己从不执行命令代码（`arb commands list` 只用 `ast` 读）。用之前看 diff。 |
| 会话推送 | 只发给 hook 记下的本机会话 socket；不存不发 messaging token；消息只由校验过的字段拼成；失败静默跳过。 |
| 种子园 | `install-claude --home` 禁止 Claude 编辑种子 notebook 和种子园的 `.arboria/`。种下（建分支和 worktree）只在你的终端里做。 |

## 诚实地说

- 终端检查看的是环境变量和 TTY，这是护栏：它挡住 Claude 在自己的工具里运行 `arb approve`，挡不住一个有意绕开它的程序。
- 真正的保证在代码路径上：提交只有一个调用点，提交前核对账本里的人类批准，批准绑定内容哈希。
- notebook 命令是 Claude 写的代码，在你的权限下运行。它能做你能做的一切，所以要像审代码一样看它的 diff。
