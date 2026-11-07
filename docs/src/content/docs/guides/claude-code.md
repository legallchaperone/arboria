---
title: Claude Code 集成
description: SessionStart hook、权限规则、CLAUDE.md 里由 arb 管理的区块和经验，以及把待办推给正在运行的会话。
---

```bash
arb install-claude          # 当前 worktree 的 .claude/settings.json
arb install-claude --main   # 主 worktree
arb install-claude --user   # ~/.claude/settings.json，对所有项目生效
arb install-claude --home   # 种子园：~/arboria/.claude/settings.json 和 ~/arboria/CLAUDE.md
```

它把 hook 和权限规则**合并**进 settings.json（保留已有配置，不重复添加），再同步 CLAUDE.md 区块。

## SessionStart hook

新会话开始时，`arb hook session-start` 注入研究树的**索引**：你在树上的位置（“你在这里”）、当前 idea 的假设、指标和最近几个 run、上一层和同一层的 idea、全项目最好的几个结果；然后是未完成的待办、待你决定的事和最新的已确认经验。上限约 4000 字符，不进 CLAUDE.md，所以 CLAUDE.md 不会被当前状态撑大。在主 worktree 里（不在某个 idea 里）索引列出进行中的 idea。

其余的历史 Claude 按需取：

```bash
arb tree                 # 整棵树
arb tree --idea exp2     # exp2 和从它分出来的
arb tree --depth 1 --json
```

每个 run 一行：parent、改了什么（`change_reason`）、结果、结论和它的来源。结果是这个 idea 的指标在 `metrics.jsonl` 里的最后一个值（键从指标文字里找，或用 `arboria.toml` 的 `[project] metric` 指定，见[配置](/reference/arboria-toml/)）；结论先用你对这个 run 写的笔记（`arb note --run rNNN "..."`，标“人”），没有就用 run 摘要的第一句（标“AI 摘要”）。树按 id 排序，同样的账本每次输出一样。超过 `--budget`（默认 16000 字符）时，不在你这条路上的已结束 idea 折成一行（最好的结果 + 经验），再不够就每个进行中的 idea 只留最好的和最近几个 run。

## UserPromptSubmit hook

会话进行中，你每发一条消息，`arb hook prompt-submit` 看一眼账本：自这个会话上一条消息（或开始）以来有 run 结束、你批准或退回了 run、有新待办、新确认的经验或路线图的改动，就注入**一行**，例如：

```text
[arboria] 自上一条消息：run 结束：r031 succeeded（val/loss 2.29）；新待办 t004（`arb todo list`）。
```

没有变化就什么都不加。arb 不往会话里推消息，只在你说话时顺带告诉 Claude。每个会话看到哪里记在 `.arboria/seen/<会话 id 的哈希>.json`（权限 0600，不进账本、不进备份，两周没动的会被清掉）；SessionStart 记下起点，所以第一条消息只报会话开始之后的事。

两个 hook 都绝不让会话失败：任何错误都只写进 `.arboria/hook.log`，以退出码 0 结束、不输出内容。在不是 arboria 的仓库里它们什么都不输出，所以 `--user` 全局安装是安全的。

## Skill

`arb install-claude` 还写一个 Claude Code Skill：`.claude/skills/arboria/SKILL.md`（`--user` 时是 `~/.claude/skills/arboria/`）。Claude 只在任务碰到 idea、run、待办、集群或账本时才加载它，所以平时不占上下文。里面是 Claude 能用的命令（读的都带 `--json`；它写的 `idea new`、`run summary`、`todo done/drop`、`lesson propose`、`inspect check/done`、`remote ls` 也带）、提交和批准的流程、训练代码的约定，以及遇到“只由人”时该怎么做：把命令告诉你，不要绕。

Skill 只是说明，不给任何权限：拒绝靠代码和权限规则。它和 settings.json 一样应该提交进仓库。重新运行 `install-claude` 会把它更新到当前版本；如果你想自己改它，删掉第一行注释（arboria 的标记），之后 arb 就不再覆盖。

## Sandbox

权限规则里的 `Edit(...)` deny 只管 Claude 的文件工具，管不到 Bash 里的 `sed -i`、`python -c` 或任何子进程。所以 `install-claude` 还打开 Claude Code 的 Bash sandbox（macOS 上是 Seatbelt，Linux / WSL2 上是 bubblewrap），在系统层面不让 Bash 写这些地方：

- 你的 notebook：主 worktree 的 `.arboria/notebooks/`，以及 idea worktree 里的 `notebook.py` 链接本身；
- 批准密钥：`~/.config/arboria/approve-keys.json` 和 `~/Library/Application Support/Arboria/`；
- `--home` 时还有种子园的 `seeds/`。

写进哪里：

- `.claude/settings.json`（提交）：`sandbox.enabled = true`、`autoAllowBashIfSandboxed = false`（照旧每条命令问你，sandbox 只是多加限制）、`excludedCommands = ["arb *"]`。`arb` 自己在 sandbox 外运行：它是写账本和新 notebook 的那段代码，“只由人”的命令在 Claude Code 里照样拒绝。
- `.claude/settings.local.json`（不提交）：`sandbox.filesystem.denyWrite` 和 `allowWrite`（项目的 `.arboria/commands/`，Claude 在那里写 notebook 命令）。路径是这台机器上的绝对路径，因为 notebook 在主 worktree 里，不在 idea 的 worktree 里。

你自己设过的值（例如 `sandbox.enabled: false`）会保留，`install-claude` 只提示一句。`--user` 不动 sandbox：那会影响这台机器上所有的 Claude 会话。

还挡不住的：原生 Windows 上 Claude Code 不开 sandbox；sandbox 起不来时默认照常不加限制地运行（可以自己设 `sandbox.failIfUnavailable: true`）；Claude 被 sandbox 拦下后可以请求在 sandbox 外重试，这一步会问你，别随手同意。兜底：Mac app 的首页会在 notebook 和备份里的不一样、又不是在 app 的 marimo 里改的时候提醒你，并给出 `git -C .arboria diff -- notebooks/NAME.py`。

## 权限规则

```json
{
  "hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": "arb hook session-start"}]}],
    "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "arb hook prompt-submit"}]}]
  },
  "permissions": {
    "deny": ["Bash(sbatch:*)", "Bash(srun:*)", "Bash(salloc:*)", "Bash(scancel:*)",
             "Bash(ssh:*)", "Bash(scp:*)", "Bash(rsync:*)",
             "Bash(arb approve:*)", "Bash(arb lesson accept:*)",
             "Bash(arb idea plant:*)", "Bash(arb idea set:*)", "Bash(arb idea revive:*)", "Bash(arb seed new:*)", "...",
             "Edit(**/.arboria/notebooks/**)", "Write(**/.arboria/notebooks/**)", "...",
             "Edit(**/notebook.py)", "Write(**/notebook.py)", "MultiEdit(**/notebook.py)", "NotebookEdit(**/notebook.py)"]
  }
}
```

- 文件规则对 Edit、Write、MultiEdit、NotebookEdit 各有一条，覆盖 `**/.arboria/notebooks/**`、`**/.arboria/ledger.db*`、`**/.arboria/backup/**`、`**/.arboria/*.toml`、`**/.arboria/sessions/**` 和 `**/notebook.py`。读取不受限制；`.arboria/commands/` 也不受限制（Claude 在那里写命令）。
- 全局命令目录（`~/.config/arboria/commands`，写成绝对路径）加进 `permissions.additionalDirectories`。这个路径只属于你这台机器，所以在项目里它写进**不提交**的 `.claude/settings.local.json`（arb 把它加进 `.git/info/exclude`），`.claude/settings.json` 保持可移植、可以提交。
- 只写种子的命令（`arb idea plant/set/revive` 和旧写法 `arb seed new/set/drop/revive/plant`）也设为 deny：它们本来就只由你在终端里运行，在 Claude Code 里代码会拒绝，这几条规则是多一层。
- `arb key …`（Touch ID 签字密钥）、改里程碑的 `arb milestone new/set/check/uncheck/done/reopen/drop`，以及对 `**/approve-keys.json`、`**/Library/Application Support/Arboria/**` 的编辑也是 deny；`arb milestone list/show` 可以用。
- 需要 Claude 上集群帮你排查时，可以自己在 settings.json 里放宽 `Bash(ssh:*)`，排查完再改回来。

:::caution[每个 worktree 是独立的文件夹]
Claude Code 只读取当前项目文件夹下的 `.claude/settings.json`。三选一：在主 worktree `arb install-claude --main` 并把 `.claude/settings.json` **提交**到主分支（之后新建的 idea worktree 都自带）；或在每个 idea worktree 里分别运行；或用 `--user`。
:::

## CLAUDE.md 区块与经验

- `arb` 只管理主 worktree 的 `CLAUDE.md` 里的**一个区块**（`<!-- arboria:begin … -->` 到 `<!-- arboria:end -->`）：使用 arboria 的稳定规则和**已确认的经验**，总长不超过 50 行。区块之外是你的，`arb` 一个字节也不改。
- `arb init`、`arb install-claude`、接受/退役经验时自动同步区块，并只提交 `CLAUDE.md` 这一个文件（`arboria: update CLAUDE.md`）。正在 merge/rebase、有冲突或没有 git 身份时只写文件、不提交；`[claudemd] autocommit = false` 关闭自动提交；`arb claudemd sync [--no-commit]` 手动重写。
- 经验由 Claude 用 `arb lesson propose "..."` 提出，你用 `arb lesson list` 查看，在**你自己的终端**里 `arb lesson accept ID` 或 `arb lesson reject ID`。已确认的经验最多 20 条；满了先 `arb lesson retire ID`。
- idea worktree 是更早分出来的，里面的 `CLAUDE.md` 可能是旧的；新确认的经验会立刻通过 hook 注入。

## 推送待办

notebook 里出现命令待办或读代码待办（[`lab.inspect`](/guides/inspect/)）时，arb 尽力把它发进这个 idea worktree 里正在运行的 Claude Code 会话：

- SessionStart hook 把会话的收件 socket（Claude Code 导出的 `CLAUDE_CODE_MESSAGING_SOCKET`）记在 `.arboria/sessions/<worktree 的哈希>.json`（权限 0600；进程已退出的条目会被清掉）。
- 消息很短：待办 id、命令名（或读代码的 view）和“运行 arb commands guide / arb inspect guide 查看约定”，只由校验过的字段拼成，不含你写的说明或问题原文。
- arb 不保存也不发送 `CLAUDE_CODE_MESSAGING_TOKEN`（notebook 的 kernel 不是那个会话的子进程），也绝不读进程环境里的 socket，只发给 hook 记下的会话。
- 没有会话、socket 不对、2 秒超时或任何错误都静默跳过，cell 里多一句“在 agent 会话里说'处理待办'即可”。`arboria.toml` 里 `[claude] push = false` 关掉推送。

会话收不收这条消息由 Claude Code 的 `crossSessionInbound` 设置决定。默认（没设置）时，普通会话直接收下，`bypassPermissions` 模式的会话会弹框等你批准。`arb install-claude` **不改**这个设置。想完全不收：`"crossSessionInbound": "refuse"`。
