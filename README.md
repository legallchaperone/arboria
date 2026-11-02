# Arboria

Arboria（命令 `arb`）是一个本地命令行工具，用来记录 ML 研究中的 **想法（idea）、实验运行（run）和笔记（note）**，专为配合 Claude Code 工作而设计。

核心思路：Claude Code 的会话是"一次性的"，所有实验状态都存放在会话之外的账本（`.arboria/ledger.db`，SQLite）里，而且只能通过 `arb` 写入。LLM 只能写少数几个自由文本字段（假设、改动原因、笔记、总结），并且有长度校验；其余字段全部由程序生成和校验。

- 每个 idea 对应一个 git 分支 `idea/NAME` 和一个独立的 worktree 文件夹 `<主仓库目录名>-NAME`（与主仓库同级）。
- 每次 `arb run new` 会把 worktree 的当前状态（包括未提交和未跟踪但未被忽略的文件）做成快照，保存为隐藏的 git ref `refs/arboria/runs/<run id>`，**不会改动你的分支、暂存区和工作区**。
- 新的 Claude Code 会话启动时，SessionStart hook 会自动注入当前 idea 的简短状态。

当前版本（step 1a）只包含本地功能：git + SQLite，没有集群/远程功能。

## 安装

需要 Python ≥ 3.11、git ≥ 2.31 和 [uv](https://docs.astral.sh/uv/)。

```bash
cd arboria
uv tool install .        # 安装后 `arb` 会出现在 PATH 中
# 开发/测试
uv run pytest
```

## 快速上手

```bash
cd ~/code/myproj                     # 你的实验代码仓库（主 worktree）
arb init                             # 创建 .arboria/、ledger.db、arboria.toml 模板
arb idea new lr-warmup --hypothesis "warmup 能稳定早期训练" --metric "val/loss @10k steps"
cd ../myproj-lr-warmup               # 新 idea 的 worktree
# ……修改代码……
arb run new --reason "warmup 1k steps，其余与 baseline 相同"   # 输出 r001
arb note "loss 曲线更平滑" --run r001 --confidence prelim
arb status
arb idea close --merge               # 或 --drop（先存档最终快照再删除分支）
```

## 命令列表

| 命令 | 说明 |
| --- | --- |
| `arb init` | 初始化（幂等）：`.arboria/`（SQLite 账本 + 备份用 git 仓库）、`arboria.toml` 模板（已存在则不覆盖）、在 `.git/info/exclude` 中排除 `.arboria/` |
| `arb idea new NAME --hypothesis T --metric T [--parent NAME]` | 从主 worktree 的 HEAD（或父 idea 的分支）创建分支 `idea/NAME` 和 worktree，输出 worktree 路径 |
| `arb idea list [--json]` | 列出所有 idea |
| `arb idea close [NAME] (--merge \| --drop) [--confidence X] [--allow-large]` | 关闭 idea（NAME 默认取当前目录所属的 idea）。`--drop`：最终快照存到 `refs/arboria/ideas/NAME/final` 并打标签 `arboria/archive/NAME`，然后删除 worktree 和分支。`--merge`：要求 worktree 没有未提交改动，在主 worktree 中 `git merge --no-ff`，冲突时自动 `git merge --abort` 并报告 |
| `arb run new --reason T [--allow-large] [--json]` | 快照当前 idea 的 worktree，创建状态为 `draft` 的 run，输出 run id |
| `arb run summary RUN_ID TEXT` | 为 run 写结果总结（≤10 行 / 1500 字符） |
| `arb note TEXT [--run ID] [--confidence X]` | 给当前 idea 写笔记；`--confidence` 同时更新 idea 的置信度 |
| `arb status [--json] [--brief]` | 在 idea worktree 中：该 idea 的假设、指标、置信度、runs、最近 5 条笔记、待决事项；在主 worktree 中：所有 open idea 概览 |
| `arb export [--out FILE]` | 导出所有 idea 的 Markdown（新的在前），可直接粘贴到 Claude Desktop |
| `arb lesson propose TEXT [--idea NAME \| --run ID]` | 提出一条经验（单行、≤200 字符），同时生成一个待人确认的 Decision；来源默认取当前目录所属的 idea |
| `arb lesson list [--all] [--json]` | 列出经验（默认只列待确认和已确认的，`--all` 含已退役/已拒绝的） |
| `arb lesson accept ID` / `reject ID` | 接受 / 拒绝一条待确认的经验（接受后更新 CLAUDE.md）。**只由你来运行** |
| `arb lesson retire ID` | 退役一条已确认的经验，并从 CLAUDE.md 中移除 |
| `arb claudemd sync [--no-commit]` | 重写主 worktree 中 CLAUDE.md 的 arboria 区块 |
| `arb hook session-start` | Claude Code SessionStart hook（从 stdin 读取 hook JSON） |
| `arb install-claude [--main \| --user]` | 把 hook 和权限规则合并进 `.claude/settings.json`，并同步 CLAUDE.md 区块 |

置信度取值：`guess`、`prelim`、`confirmed`、`rejected`。

文本长度限制：hypothesis ≤300、metric ≤200、change reason ≤300、note ≤1000、summary ≤10 行且 ≤1500 字符。

快照前会检查未跟踪文件：单个文件 >50 MB 或合计 >200 MB 时报警并拒绝，除非加 `--allow-large`（建议把数据/检查点加入 `.gitignore`）。

## Claude Code 设置

```bash
arb install-claude          # 写入当前 worktree 的 .claude/settings.json
arb install-claude --main   # 写入主 worktree
arb install-claude --user   # 写入 ~/.claude/settings.json（对所有项目生效）
```

会合并以下内容（保留已有配置，不会重复添加）：

```json
{
  "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "arb hook session-start"}]}]},
  "permissions": {
    "ask": ["Bash(arb approve:*)", "Bash(arb lesson accept:*)"],
    "deny": ["Bash(sbatch:*)", "Bash(ssh:* sbatch*)"]
  }
}
```

**注意：** 每个 idea 的 worktree 是独立的文件夹，Claude Code 只读取当前项目文件夹下的 `.claude/settings.json`。因此需要满足以下任一条件：

1. 在主 worktree 运行 `arb install-claude --main` 并把 `.claude/settings.json` **提交**到主分支——之后新建的 idea worktree 都会自带这份配置；或
2. 在每个 idea worktree 中分别运行 `arb install-claude`；或
3. 使用用户级安装 `arb install-claude --user`。hook 在非 arboria 仓库中不输出任何内容，所以全局安装是安全的。

hook 绝不会让会话失败：任何错误都只会写入 `.arboria/hook.log`，并以退出码 0 结束、不输出内容。注入的上下文上限约 4000 字符。

## CLAUDE.md 与经验

- `arb` 只管理主 worktree 的 `CLAUDE.md` 里的**一个区块**（`<!-- arboria:begin … -->` 到 `<!-- arboria:end -->` 之间），内容是使用 arboria 的稳定规则和**已确认的经验**，总长不超过 50 行。区块之外的内容都是你的，`arb` 一个字节也不会改。区块内的手动修改会在下次同步时被覆盖；标记损坏（只有开头没有结尾、重复等）时 `arb` 拒绝写入并提示你手动修复。
- `arb init`、`arb install-claude`、接受/退役经验时会自动同步区块，并只提交 `CLAUDE.md` 这一个文件（提交信息 `arboria: update CLAUDE.md`，使用你的 git 身份；你暂存的其他文件不受影响）。主 worktree 正在 merge/rebase/cherry-pick、`CLAUDE.md` 有冲突或没有配置 git 身份时只写文件、不提交。在 `arboria.toml` 中设 `[claudemd] autocommit = false` 可关闭自动提交。
- **当前状态不写进 CLAUDE.md**：idea、run、待决事项由 SessionStart hook 在每次会话开始时注入。
- **经验由 Claude 提出、只由你确认**：Claude 用 `arb lesson propose "..."` 提出，你用 `arb lesson list` 查看，用 `arb lesson accept ID` 或 `arb lesson reject ID` 决定（`install-claude` 会把 `arb lesson accept` 设为需要询问）。已确认的经验最多 20 条；满了就先用 `arb lesson retire ID` 退役或合并一些。
- idea worktree 是更早从主分支分出来的，里面的 `CLAUDE.md` 可能是旧的：新确认的经验会**立即**通过 hook 注入（最新 10 条，以及待你确认的条数），合并回主分支之后也会出现在 worktree 的 `CLAUDE.md` 里。

## 状态存放位置

- 主 worktree = `git rev-parse --git-common-dir` 的父目录；所有 worktree 共享 `<主 worktree>/.arboria/`。
- `.arboria/ledger.db`：SQLite（WAL 模式），schema 版本记录在 `PRAGMA user_version`。
- `.arboria/` 本身是一个只用于备份的小 git 仓库：在 idea 新建/关闭、笔记、新 run、决策、经验提出/接受/拒绝/退役之后，会把数据库导出到 `.arboria/backup/ledger.sql` 并提交。
- `arboria.toml` 位于主 worktree 根目录，应由你提交到仓库（预算、默认机器、review 检查清单目前尚未被读取；`[claudemd] autocommit` 已生效）。
