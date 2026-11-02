# Arboria

Arboria（命令 `arb`）是一个本地命令行工具，用来记录 ML 研究中的 **想法（idea）、实验运行（run）和笔记（note）**，专为配合 Claude Code 工作而设计。

核心思路：Claude Code 的会话是"一次性的"，所有实验状态都存放在会话之外的账本（`.arboria/ledger.db`，SQLite）里，而且只能通过 `arb` 写入。LLM 只能写少数几个自由文本字段（假设、改动原因、笔记、总结），并且有长度校验；其余字段全部由程序生成和校验。

- 每个 idea 对应一个 git 分支 `idea/NAME` 和一个独立的 worktree 文件夹 `<主仓库目录名>-NAME`（与主仓库同级）。
- 每次 `arb run new` 会把 worktree 的当前状态（包括未提交和未跟踪但未被忽略的文件）做成快照，保存为隐藏的 git ref `refs/arboria/runs/<run id>`，**不会改动你的分支、暂存区和工作区**。
- 新的 Claude Code 会话启动时，SessionStart hook 会自动注入当前 idea 的简短状态。

当前版本：step 1a（本地账本）+ step 1b 中不依赖真集群的部分（提交、人工批准、Slurm 作业记录、`FakeSlurm` 测试后端和 `SshSlurm` 真实后端）。`SshSlurm` 还没有在真集群（NCSA DeltaAI）上跑通过，见下文“提交与批准”。

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
| `arb submit --reason T --gpus N --time DUR [--nodes 1] [--partition P] [--machine M] [--mem GB] [--cpus N] [--allow-large] [--json] -- CMD...` | **只在本地**：快照当前 idea 的 worktree，生成请求、分段计划和审核单，run 状态为 `awaiting_approval`，并生成待人决定的 Decision。不连接集群 |
| `arb card RUN [--json]` | 按当前机器配置重新计算并显示审核单；配置变了会提示“审核单已变化” |
| `arb approve RUN` | **只由你来运行**：核对审核单哈希，记录批准 Decision，然后发送代码、逐段 `sbatch`。中途失败（包括需要重新登录）可以再次运行同一命令续交，不会重复提交 |
| `arb reject RUN [--reason T]` | 拒绝一个待批准的 run（理由存为该 run 的笔记），不连接集群 |
| `arb cancel RUN` | 取消已提交 run 的所有作业 |
| `arb poll [RUN] [--json]` | 查询一次作业状态，更新 run 状态和花费；结束后把 `ARBORIA_OUT` 拉回 `.arboria/runs/RUN/out/` |
| `arb watch [--once] [--interval 60] [--window 10]` | 常驻轮询所有活跃 run；作业开跑后的观察期内日志出现 NaN 或 `CUDA out of memory`、或作业失败，就取消剩余作业并通知（`.arboria/notifications.log`） |
| `arb logs RUN [--segment K] [-n 100]` | 看集群上某段作业的日志末尾（代替 ssh） |
| `arb remote ls RUN [SUBPATH] [--out]` | 列出集群上 run 目录（或 `ARBORIA_OUT`）里的文件（代替 ssh） |
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
    "deny": ["Bash(sbatch:*)", "Bash(srun:*)", "Bash(salloc:*)", "Bash(scancel:*)",
             "Bash(ssh:*)", "Bash(scp:*)", "Bash(rsync:*)"]
  }
}
```

真正的保证是**只有 `arb approve` 会提交作业**，而且提交前代码会再次核对账本里有你的批准 Decision；这些 deny 规则只是额外一层，让 Claude 不能自己直接碰集群。Claude 应该用 `arb logs` / `arb remote ls` 看集群上的东西，而不是 ssh。如果你需要 Claude 上集群帮你排查问题，可以自己在 settings.json 里放宽 `Bash(ssh:*)` 这条规则（建议排查完再改回来）。

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

## 提交与批准

上集群的 run 分三步，第二步只能由你来做：

```bash
# 1. Claude（或你）在 idea worktree 里：只在本地生成快照和审核单
arb submit --reason "warmup 1k steps" --gpus 4 --time 60h -- python train.py --lr 3e-4
# 2. 你看过审核单后批准（Claude Code 里这条命令设为需要询问）
arb card r003          # 随时重看审核单
arb approve r003       # 或 arb reject r003 --reason "..."
# 3. 跟踪
arb watch              # 或 arb poll / arb logs r003 / arb remote ls r003
```

- **审核单**分为 检查 / 资源 / 估计 / 预算 / 审稿（第 2 步加入，暂无）。批准绑定在审核单哈希上：哈希覆盖快照 commit、命令、资源（含账户和计费卡数）、机器、分区、分段计划和环境（prelude、env_activate、路径）。`arb approve` 按存下的请求和**当前**机器配置重算，对不上就拒绝（“审核单已变化，请重新 arb submit”）。
- **计费**（DeltaAI 规则）：计费卡数 = max(GPU 数, ⌈内存 / 每卡内存份额⌉, ⌈CPU / 每卡 CPU 份额⌉)。默认按卡数配内存和 CPU，所以计费卡数 = 申请卡数；手动用 `--mem` / `--cpus` 多要时，审核单上会出现 ⚠ 和实际计费卡数。估计上限 = 计费卡数 × 时长（小时）× 分区倍率，按各段累加。
- **分段**：时长超过分区上限（`ghx4` 48h，`ghx4-interactive` 2h）就拆成 ⌈T/M⌉ 段，用 `--dependency=afterany:<上一段>` 串起来，训练代码必须能从 `ARBORIA_CKPT_DIR` 续训；段数超过 `max_segments` 拒绝。某段失败时其余段会被取消；只有最后一段 `COMPLETED` 才算成功。
- **幂等**：每段 `sbatch` 之前先在账本里记一行 `submitting`，并先按作业名 `arboria-<run>-s<k>` 在集群上查找；已存在就直接采用，不重复提交。
- **Duo**：`SshSlurm` 每次远程调用前先运行 `ssh -O check <host>`；没有活着的主连接就报“请先在终端运行 `ssh <host>` 完成 Duo 认证，然后重试”，绝不自动重试。远程命令都带 `-o BatchMode=yes`，不会弹出密码或推送 Duo。`arb watch` 遇到这种情况只通知一次，然后暂停对该机器的远程调用，直到你重新启动 `arb watch`。
- 作业里可用的环境变量：`ARBORIA_RUN_ID`、`ARBORIA_IDEA`、`ARBORIA_SEGMENT`、`ARBORIA_DATA`、`ARBORIA_CACHE`（`HF_HOME=$ARBORIA_CACHE/hf`）、`ARBORIA_CKPT_DIR=<ckpt>/<run>`、`ARBORIA_OUT=<out>/<run>`。命令在 `<runs>/<run>/code`（快照内容）里运行，日志在 `<runs>/<run>/logs/s<k>.<jobid>.out`。命令按参数逐个加引号，不经过 shell 展开；多节点时请在命令里自己写 `srun`。

### 机器配置

机器配置在 `~/.config/arboria/clusters.toml`（可用环境变量 `ARBORIA_CONFIG_DIR` 改位置），示例见 [`examples/clusters.toml`](examples/clusters.toml)：

```toml
[deltaai]
kind = "slurm"            # "fake" 也可以，用于测试和演练
host = "deltaai"          # ~/.ssh/config 里配了 ControlMaster 的别名
arch = "aarch64"
account = "<from `accounts`>"
gpus_per_node = 4
cpus_per_gpu = 0          # 待 machine init 时确认；必须 > 0
mem_per_gpu_gb = 0        # 待 machine init 时确认；必须 > 0
default_partition = "ghx4"
max_segments = 4
balance_cmd = "accounts"
charge_cmd = "jobcharge"
prelude = []              # shell 行，例如 module load
env_activate = []         # 激活环境的 shell 行（容器或 pixi），machine init 时决定
[deltaai.partitions.ghx4]
max_walltime = "48h"
charge = 1.0
[deltaai.partitions."ghx4-interactive"]
max_walltime = "2h"
charge = 2.0
[deltaai.paths]
runs = "/projects/<proj>/$USER/arboria/runs"
data = "/work/nvme/<proj>/$USER/data"
cache = "/projects/<proj>/$USER/cache"
ckpt = "/work/hdd/<proj>/$USER/ckpt"
out  = "/projects/<proj>/$USER/arboria/out"
```

- 标着“待确认”的值（0、`<…>` 占位符）会被拒绝，并提示是哪一项。路径必须是绝对路径，只能用字母、数字、`._-/` 和 `$USER`（`#SBATCH` 行里会换成 `%u`）。
- `kind = "fake"` 时还需要 `root = "/某个本地目录"`：`FakeSlurm` 把“集群”模拟在本地，路径就是本地路径，适合演练整个流程。
- 预算读 `arboria.toml` 的 `[budget] total_su`；默认机器读 `default_machine`。
- **一个机器配置只给一个仓库用。** run id 在每个仓库里都从 `r001` 开始，作业名 `arboria-<run>-s<k>` 和远程目录 `<runs>/<run>` 不带仓库名；两个仓库共用同一组路径会互相覆盖，续交时还可能把另一个仓库的同名作业当成自己的。多个仓库请各用一个机器名（`host` 可以相同），各配不同的 `paths`。

## 状态存放位置

- 主 worktree = `git rev-parse --git-common-dir` 的父目录；所有 worktree 共享 `<主 worktree>/.arboria/`。
- `.arboria/ledger.db`：SQLite（WAL 模式），schema 版本记录在 `PRAGMA user_version`。
- `.arboria/` 本身是一个只用于备份的小 git 仓库：在 idea 新建/关闭、笔记、新 run、决策、经验提出/接受/拒绝/退役之后，以及 run 的提交、批准、进入队列、拒绝、取消和结束之后，会把数据库导出到 `.arboria/backup/ledger.sql` 并提交（轮询这类心跳更新只写数据库，不提交）。
- `.arboria/runs/<run>/out/`：拉回来的 `ARBORIA_OUT`；`.arboria/notifications.log`：watch/poll 的通知。
- `arboria.toml` 位于主 worktree 根目录，应由你提交到仓库（`default_machine`、`[budget] total_su`、`[claudemd] autocommit` 已生效；review 检查清单尚未被读取）。
