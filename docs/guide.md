# Arboria 完整说明

这是 Arboria（命令 `arb`）的完整参考：每个命令、配置项、notebook 里的 `lab` API、自定义命令、看板、安全模型和 DeltaAI 上的实测说明。概览和快速开始见 [README](../README.md)。

**目录**

- [核心概念](#核心概念)
- [安装](#安装)
- [命令行上手](#命令行上手)
- [Notebook：每个 idea 一个](#notebook每个-idea-一个)
- [Notebook 里的 `lab` API](#notebook-里的-lab-api)
- [待办（todo）](#待办todo)
- [自定义命令](#自定义命令)
- [提交与批准](#提交与批准)
- [训练代码约定](#训练代码约定)
- [机器配置](#机器配置)
- [DeltaAI 说明](#deltaai-说明)
- [看板：`arb ui`](#看板arb-ui)
- [种子：还没有仓库的想法](#种子还没有仓库的想法)
- [Claude Code 设置](#claude-code-设置)
- [CLAUDE.md 与经验](#claudemd-与经验)
- [安全模型](#安全模型)
- [命令参考](#命令参考)
- [`arboria.toml` 配置](#arboriatoml-配置)
- [状态存放位置](#状态存放位置)
- [当前状态与已知限制](#当前状态与已知限制)
- [开发](#开发)

## 核心概念

Arboria 是一个本地命令行工具，用来记录 ML 研究中的 **想法（idea）、实验运行（run）和笔记（note）**，专为配合 Claude Code 工作而设计。

核心思路：Claude Code 的会话是“一次性的”，所有实验状态都存放在会话之外的账本（`.arboria/ledger.db`，SQLite）里，而且只能通过 `arb` 写入。LLM 只能写少数几个自由文本字段（假设、改动原因、run 总结、经验、待办结果），并且有长度校验；其余字段全部由程序生成和校验。

- 每个 idea 对应一个 git 分支 `idea/NAME` 和一个独立的 worktree 文件夹 `<主仓库目录名>-NAME`（与主仓库同级）。
- 每次 `arb run new` 会把 worktree 的当前状态（包括未提交和未跟踪但未被忽略的文件）做成快照，保存为隐藏的 git ref `refs/arboria/runs/<run id>`，**不会改动你的分支、暂存区和工作区**。
- 每个 idea 有一个 [marimo](https://marimo.io) notebook（`.arboria/notebooks/<idea>.py`，在 worktree 里链接成 `notebook.py`）：**你**在里面整理想法、发起实验、分析结果；Claude 只读它，不改它，它的活儿来自你在 notebook 里留下的待办。
- **Claude 写命令库，你写 notebook**：`lab.svd(r012, "对 attention 权重做 SVD，画前 20 个奇异值")` 这样的分析命令第一次调用时交给 Claude 去写，以后直接运行存下来的函数（不花 token、可复现），见[自定义命令](#自定义命令)。
- 新的 Claude Code 会话启动时，SessionStart hook 会自动注入当前 idea 的简短状态（含待办）。
- 批准上集群和接受经验**只在你自己的终端里**做，Claude 做不了。

置信度取值：`guess`、`prelim`、`confirmed`、`rejected`。

文本长度限制：hypothesis ≤300、metric ≤200、种子的一句话想法 ≤300、change reason ≤300、note ≤1000、summary ≤10 行且 ≤1500 字符、lesson ≤200、待办 ≤1000、待办结果 ≤600。

快照前会检查未跟踪文件：单个文件 >50 MB 或合计 >200 MB 时报警并拒绝，除非加 `--allow-large`（建议把数据/检查点加入 `.gitignore`）。

## 安装

需要 Python ≥ 3.11、git ≥ 2.31 和 [uv](https://docs.astral.sh/uv/)。

```bash
git clone https://github.com/legallchaperone/arboria.git
cd arboria
uv tool install --force '.[ui]'   # arb 出现在 PATH 中；[ui] 带上 marimo（notebook 和看板都要）
```

- 只要命令行、不要看板时 `uv tool install .` 也可以；之后要看板再运行上面那条。
- 看板会 import arboria，所以 marimo 必须装在 **arb 自己的环境里**（`'.[ui]'`，或 `pip install 'arboria[ui]'`）；单独 `uv tool install marimo` 只够打开 notebook，不够跑 `arb ui`。
- 在 notebook 里读 wandb 曲线还需要 `wandb`（有 `pandas` 时返回 DataFrame）；拉回的 `config.yaml` 在装了 PyYAML 时会被解析成 JSON。

## 命令行上手

```bash
cd ~/code/myproj                     # 你的实验代码仓库（主 worktree）
arb init                             # 创建 .arboria/、ledger.db、arboria.toml 模板
arb idea new lr-warmup --hypothesis "warmup 能稳定早期训练" --metric "val/loss @10k steps"
cd ../myproj-lr-warmup               # 新 idea 的 worktree（里面有 notebook.py）
marimo edit notebook.py              # 你的工作台
# ……修改代码（或在 notebook 里 lab.todo("...") 让 Claude 改）……
arb run new --reason "warmup 1k steps，其余与 baseline 相同"   # 输出 r001
arb note "loss 曲线更平滑" --run r001 --confidence prelim
arb status
arb idea close --merge               # 或 --drop（先存档最终快照再删除分支）
```

`arb init` 第一次运行后会打印下一步：编辑 `arboria.toml`（`default_machine`、`[wandb] entity/project`、`[run] command`）并提交；`arb install-claude`；`arb idea new …`；打开那个 worktree 里的 `notebook.py`（这个 idea 的 notebook）。

## Notebook：每个 idea 一个

- `arb idea new` 会同时创建 `.arboria/notebooks/<idea>.py`：一个普通的 marimo notebook（纯 Python 文件，创建时不需要装 marimo），开头是一个只显示渲染结果（代码折叠）的 markdown cell（idea 名、假设、指标、父 idea、创建日期），然后是 `from arboria import lab` 和 `lab.runs()` 两个 cell。worktree 里的 `notebook.py` 是指向它的符号链接。模板只在新建时写入，以后不会改你的 notebook。
- **打开 notebook**：在 VS Code 里装 marimo 扩展后直接打开 `notebook.py`，或者在 worktree 里 `marimo edit notebook.py`。
- **打开时自动运行**：marimo（0.25）默认打开 notebook 时**不运行** cell，要手动点运行。想一打开就看到 `lab.runs()`、run 的状态和图，在 `~/.config/marimo/marimo.toml`（有 `$XDG_CONFIG_HOME` 时在它下面）里写：

  ```toml
  [runtime]
  auto_instantiate = true
  ```

  这是你自己的设置，arb 不会替你改；notebook 文件里的同名设置 marimo 出于安全会忽略。打开就重跑是安全的：见下面“重复执行 cell 是安全的”。
- `notebook.py` 写进了 `.git/info/exclude`（所有 worktree 共用），**快照永远不包含它**，改 notebook 不会让快照变化。如果你的仓库里本来就有一个被跟踪的 `notebook.py`，arb 不会覆盖它，只提示你直接打开 `.arboria/notebooks/<idea>.py`。
- notebook 放在 `.arboria/` 里，所以关闭 idea（`--drop` 或 `--merge`）时只删掉符号链接，notebook 本身保留；备份仓库（`.arboria/` 的 git）每次备份提交都会带上 `notebooks/*.py`（marimo 的 `__marimo__/` 缓存不跟踪）。
- notebook 是你的：`arb install-claude` 会禁止 Claude 用 Edit/Write/MultiEdit/NotebookEdit 改 `**/notebook.py` 和 `.arboria/notebooks/**`，但它可以**读**，用来理解你的想法和分析。
- 装 marimo：`uv tool install marimo`，或者把 arboria 装成带可选依赖 `ui` 的包（`uv tool install --force '.[ui]'` / `uv pip install 'arboria[ui]'`）。模板按 marimo 0.25 的文件格式写，`marimo check --strict` 没有警告。
- 以前的 idea 没有 notebook（或者你删了它）：`arb idea notebook [NAME]` 补建 notebook 和 `notebook.py` 链接；已经有了就什么都不改，只提示一句。

## Notebook 里的 `lab` API

```python
from arboria import lab

r = lab.run(reason="warmup 1k", gpus=4, time="12h", set={"lr": 3e-4})   # 只生成审核单
r                       # 一行状态 + 一行提示（待批准时是 arb approve 命令），审核单折叠在下面
r2 = lab.run(reason="lr 小一点", from_=r, set={"lr": 1e-4})             # 同一快照，只改参数
lab.runs()              # 当前 idea 的 run 一览；lab.runs(raw=True) 是账本里的所有列
lab.get("r001")         # 按 id 取一个 run 句柄
r2.metrics(["val/loss"])                       # wandb 曲线（需要 wandb）
lab.compare(["r001", "r002"], "val/loss")
lab.todo("把 eval 改成每 500 步一次")          # 给 Claude 留待办；lab.todos() 列出待办
lab.fork("lr-warmup-cosine", from_=r, hypothesis="...", metric="...")   # 从 r 的快照分出新 idea
lab.conf("prelim")      # 改置信度；lab.metric("...") 改指标
lab.prune("loss 不降")   # = arb idea close --drop，理由记成你的笔记
```

- **当前 idea 怎么确定**（`lab.resolve()`，按顺序）：`lab.use("<idea>")` 手动指定 → 调用者的 `__file__`（marimo 里就是 `.arboria/notebooks/<idea>.py` 或指向它的 `notebook.py`）→ 正在运行的 marimo app（尽力而为）→ 当前目录所在的 idea worktree。都不行就报错，提示你 `lab.use(...)`。
- `lab.run(reason=..., from_=None, set=None, gpus=None, time=None, partition=None, nodes=None, machine=None, command=None)` 和 `arb submit` 完全一样：只在本地生成快照（或复用 `from_` 的快照）和审核单，**绝不批准**。命令默认取 `arboria.toml` 的 `[run] command`，`set` 按 `[run] override_style` 接在后面。
- **重复执行 cell 是安全的**：同一个 idea 里如果已经有一个审核单哈希相同（同一快照、命令、资源、环境）且没有被拒绝/取消/失败的 run，就直接返回它（`arb submit` 会打印“已存在：rNNN”），不会再建一个。
- **有意重复跑**（例如看种子方差）：`lab.run(..., repeat=True)` / `arb submit --repeat` 跳过上面的复用，每次都建一个新 run。请求里带一个随机 nonce，它也算进审核单哈希，所以批准只对这一个 run 有效；审核单上会写“⚠ 重复运行”。注意：带 `repeat=True` 的 cell 每执行一次就多一个待批准的 run。
- `Run` 句柄：`.id`、`.status()`、`.card()`、`.jobs()`、`.wandb_url`、`.metrics(keys)`、`.local_out`（拉回来的 `ARBORIA_OUT`）、`.config`（解析后的 config）。
- **`Run` 在 notebook 里怎么显示**：第一行是 `r003 · 待批准 · 原因 · 来自 r002`（状态用中文：草稿、待批准、已批准、排队中、运行中、成功、失败、已取消、已拒绝，和看板一致）；第二行随状态变：待批准时是 `arb approve r003`，运行中是已运行多久（分段时是第几段），成功时是用时、SU 和 wandb 链接，失败时是失败原因（`arb watch` 记下的那一行日志，没有时是失败作业的状态）。审核单、作业表、run 总结都折叠在下面（“审核单”/“详情”）。账本里的文字一律转义后显示。
- **`lab.runs()`** 的列：run、状态、父 run、原因、资源（`4 GPU × 12h`）、SU（结束后是实际花费，之前是 `≤ 估计上限`）、创建（本地时间，`月-日 时:分`）；空值是空字符串。要原始数据（`status`、`est_cost`、`request_json` 等全部列）用 `lab.runs(raw=True)`。有 pandas 时都是 DataFrame，否则是 dict 的列表。
- `lab` 里没有任何批准函数。写待办、置信度、指标、`prune` 记的是**你**的记录，在 Claude Code 里调用会被拒绝。
- 这些记录在 cell 重跑时也不会重复：`lab.conf(...)`、`lab.metric(...)` 的值没变就什么都不写（没有事件、没有备份提交）；`lab.todo("...")` 遇到这个 idea 已有一模一样的未完成待办时直接返回它。

## 待办（todo）

- 你在 notebook 里用 `lab.todo("...")` 写待办；SessionStart hook 会在当前 idea 的状态里列出未完成的待办（“来自 notebook 的待办：…”），Claude 据此干活。
- `arb todo list [--all] [--json]`：在 idea worktree 里列这个 idea 的待办，在主 worktree 里列所有 open idea 的。
- `arb todo done tNNN --result "..."`：Claude 做完后写一句结果（≤600 字符）；`arb todo drop tNNN [--reason "..."]`：不做了。
- `arb note` 只由你写（在 Claude Code 里会被拒绝）；Claude 的结论写进待办的 result 或 `arb run summary RUN "..."`。

## 自定义命令

原则：**Claude 写命令库，你写 notebook。** 你在 notebook 里直接写想要的分析：

```python
lab.svd(r012, "对 attention 权重做 SVD，画前 20 个奇异值")
```

- **第一次调用**（命令还不存在）：arb 写一个命令待办（`tNNN`，记下命令名、范围、这次调用的参数和你的说明），cell 显示“命令 svd 还不存在，已交给 Claude（t007）。写好后这个 cell 会自动重跑。”，不会报错。同名命令已经有未完成的待办时直接复用，重复执行 cell 不会堆待办。
- **Claude 去写**：它在自己的面板里干活，拿不准又没法用默认值解决时会在面板里问你；写好 `<名字>.py` 后 `arb todo done`。约定见 `arb commands guide`（ctx 的用法、返回值、只读、不联网、只写缓存目录、带 docstring）。
- **自动重跑**：marimo 正在运行时，调用命令的 cell 会盯着命令文件（每秒看一次，文件稳定一秒后才算），文件出现或改动就自动重跑这个 cell；结果下面有一行出处：“命令 svd · v1a2b3c4d · 项目”（版本 = 文件 sha256 前 8 位）。之后每次调用都只是运行存下来的函数：不花 token、结果可复现。返回值在 marimo 里是一个带出处的包装（原始值在 `.value`）；在普通 Python 里直接是原始值。每次调用会在账本里记一个轻量事件（不做备份提交）。
  - 实现细节：marimo 公开的响应式只重跑引用了被改动全局变量的 cell，`lab.svd(r)` 这种 cell 碰不到，所以这里用了 marimo 的私有接口（对着 0.25.2 核对过，测试守着）；拿不到时退回成“重新运行这个 cell”的提示。
- **修改**：`lab.svd.revise("改成 log 坐标")` 生成一个修改待办；改好后用到它的 cell 会自动重跑。`lab.svd.source()` 显示文件内容，`lab.svd.info()` 显示版本和描述，`lab.commands()` / `arb commands list [--json]` 列出所有命令（`lab.commands()` 在 marimo 里是一张表，在普通 Python 里每个命令一行：名字、范围、版本、描述）。
- **两个范围**：项目 `.arboria/commands/<名字>.py`（只属于这个仓库）和全局 `~/.config/arboria/commands/<名字>.py`（`ARBORIA_CONFIG_DIR` 可改位置，所有项目共用）。同名时项目的优先。默认写进项目；想要全局的就在第一次调用时加 `scope="global"`（`scope` 是 lab 自己用的参数，不会传给命令）。名字只能是小写字母、数字和 `_`，以字母开头，不能和 `lab` 已有的函数重名。
- **命令文件**：一个和文件同名的函数 `def svd(ctx, run, note=None, *, k=20)`，docstring 是描述。`ctx.run(r)` 给出 run 句柄（`.local_out`、`.config`、`.metrics(keys)`），`ctx.idea`（name、hypothesis、metric），`ctx.mo`（marimo 模块或 None），`ctx.cache_dir`（`.arboria/cache/commands/<名字>/`），`ctx.value(x)`（marimo 控件取 `.value`）。
- **`arb commands list` 不执行命令代码**：它只用 `ast` 读文件（版本、描述、被项目同名命令覆盖的、有问题的）。只有你 kernel 里的 `lab.<名字>(...)` 才 import 并运行它。

### 交给正在运行的 Claude Code 会话

- 尽力而为：SessionStart hook 把会话的收件 socket（Claude Code ≥ 2.1.224 导出的 `CLAUDE_CODE_MESSAGING_SOCKET`）记在 `.arboria/sessions/<worktree 的哈希>.json`（权限 0600；进程已退出的条目会被清掉）。notebook 生成命令待办时，arb 找到这个 idea 的 worktree 里活着的会话，发一句短消息（待办 id、命令名，以及“运行 arb commands guide 查看约定”；不含你写的说明原文）。没有会话、socket 不对、2 秒超时或任何错误都静默跳过，cell 里会多一句“在 Claude 面板里说'处理待办'即可”。在 `arboria.toml` 里设 `[claude] push = false` 可以关掉。
- arb 不保存也不发送 `CLAUDE_CODE_MESSAGING_TOKEN`：notebook 的 kernel 不是那个会话的子进程，不应该冒充它。也绝不读进程环境里的 socket，只发给 hook 记下的会话。
- **会话收不收这条消息**由 Claude Code 的 `crossSessionInbound` 决定。没有设置时（默认）：普通（会弹权限确认的）会话直接收下；`bypassPermissions` 模式的会话会弹一个对话框等你批准。`arb install-claude` **不改**这个设置：项目里的 `.claude/settings.json` 只能让它更严（`hold`/`refuse`），设 `accept` 不起作用；在用户设置里设 `"crossSessionInbound": "accept"` 会对你所有会话生效，意思是本机同一个系统用户的任何进程都能往你的会话里发消息——一般不需要。想完全不收：`"crossSessionInbound": "refuse"`。

### 命令的安全

命令代码由 Claude 写，却在**你的** notebook kernel 里、以**你的**权限运行（能读写你能读写的一切）。用之前先看 diff：Claude 写或改文件时它的面板里会显示改动；项目命令还会随下一次备份提交进 `.arboria/` 的备份仓库，`git -C .arboria log -p commands/` 能看到每一次改动；`lab.<名字>.source()` 显示当前内容。全局命令（`~/.config/arboria/commands/`）不在任何仓库里，想留历史就自己把那个目录放进 git。命令运行期间 `arb approve` / `arb lesson accept` 的代码路径会拒绝（“不能在 notebook 命令里运行”），这是护栏不是沙箱；提交上集群的硬保证仍然是账本里有你的批准 Decision。

## 提交与批准

上集群的 run 分三步，第二步只能由你来做：

```bash
# 1. Claude（或你，或 notebook 里的 lab.run）在 idea worktree 里：只在本地生成快照和审核单
arb submit --reason "warmup 1k steps" --gpus 4 --time 60h -- python train.py --lr 3e-4
arb submit --from r003 --set lr=1e-4 --reason "lr 小一点"   # 同一快照，只改参数
# 2. 你在自己的终端里看过审核单后批准
arb card r003          # 随时重看审核单
arb approve r003       # 显示审核单，输入 r003 确认；或 arb reject r003 --reason "..."
# 3. 跟踪
arb watch              # 或 arb poll / arb logs r003 / arb remote ls r003
```

- **批准只在你自己的终端里**：`arb approve` 和 `arb lesson accept` 在环境里有 `CLAUDECODE` 或 `CLAUDE_CODE_ENTRYPOINT`（Claude Code 给它的工具设置的变量）时，或者 stdin 不是终端时，直接拒绝：“批准只能在你自己的终端里做：请在终端运行 arb approve rNNN”。然后显示审核单，要你输入 run id（或经验 id）确认。批准 Decision 里记下 author = human、进程号和最多 6 层父进程（尽力而为）。这是护栏，不是沙箱；硬保证仍然是提交前代码核对账本里有你的批准 Decision。
- **`--from` 和 `--set`**：`--from rNNN` 复用那个 run 的快照（不再做新快照），GPU、时长、节点、分区、内存、CPU 默认取它的；命令取它的命令（它当时写了 `-- CMD` 时）或当前的 `[run] command`，再加上它的 override，用新的 `--set` 覆盖同名的。审核单上会写“复用 rNNN 的快照”。
- **相同的提交只算一次**：审核单哈希（快照、命令、资源、环境）相同、而且没被拒绝/取消/失败的 run 已经存在时，`arb submit` 直接返回它，打印“已存在：rNNN”。想重复跑同一个实验就改点什么（例如 `--set seed=2`），或加 `--repeat`。
- **审核单**分为 检查 / 资源 / 估计 / 预算。批准绑定在审核单哈希上：哈希覆盖快照 commit、命令、资源（含账户和计费卡数）、机器、分区、分段计划和环境（prelude、env_activate、路径）以及项目 id。`arb approve` 按存下的请求和**当前**机器配置重算，对不上就拒绝（“审核单已变化，请重新 arb submit”）。
- **计费**（DeltaAI 规则）：计费卡数 = max(GPU 数, ⌈内存 / 每卡内存份额⌉, ⌈CPU / 每卡 CPU 份额⌉)。默认按卡数配内存和 CPU，所以计费卡数 = 申请卡数；手动用 `--mem` / `--cpus` 多要时，审核单上会出现 ⚠ 和实际计费卡数。估计上限 = 计费卡数 × 时长（小时）× 分区倍率，按各段累加。
- **分段**：时长超过分区上限（`ghx4` 48h，`ghx4-interactive` 2h）就拆成 ⌈T/M⌉ 段，用 `--dependency=afterany:<上一段>` 串起来，训练代码必须能从 `ARBORIA_CKPT_DIR` 续训；段数超过 `max_segments` 拒绝。某段失败时其余段会被取消；只有最后一段 `COMPLETED` 才算成功。
- **幂等**：每段 `sbatch` 之前先在账本里记一行 `submitting`，并先按完整作业名 `arb-<project>-<run>-s<k>` 在集群上查找（只认自己的作业、名字完全相同的）；已存在就直接采用，不重复提交。中途失败（包括需要重新登录）可以再次运行 `arb approve` 续交。
- **Duo**：`SshSlurm` 每次远程调用前先运行 `ssh -O check <host>`；没有活着的主连接就报“请先在终端运行 `ssh <host>` 完成 Duo 认证，然后重试”，绝不自动重试。远程命令都带 `-o BatchMode=yes`，不会弹出密码或推送 Duo。`arb watch` 遇到这种情况只通知一次，然后暂停对该机器的远程调用，直到你重新启动 `arb watch`。
- **观察期**：`arb watch` 在作业开跑后的观察期（默认 `[watch] observation_minutes` = 10 分钟）内检查日志末尾；某行匹配 `[watch] fail_patterns`（默认：loss 为 NaN、CUDA OOM），或作业失败，就取消剩余作业并通知（`.arboria/notifications.log`，写明匹配的正则和那一行日志）。
- 作业里可用的环境变量：`ARBORIA_RUN_ID`、`ARBORIA_WANDB_ID`（= `<项目 id>-<run id>`，几个仓库共用一个 wandb project 也不会撞）、`ARBORIA_IDEA`、`ARBORIA_SEGMENT`、`ARBORIA_DATA`、`ARBORIA_CACHE`（`HF_HOME=$ARBORIA_CACHE/hf`）、`ARBORIA_CKPT_DIR=<ckpt>/<project>/<run>`、`ARBORIA_OUT=<out>/<project>/<run>`。命令在 `<runs>/<project>/<run>/code`（快照内容）里运行，日志在 `<runs>/<project>/<run>/logs/s<k>.<jobid>.out`。命令按参数逐个加引号，不经过 shell 展开；多节点时请在命令里自己写 `srun`。
- 看集群上的东西用 `arb logs RUN [--segment K] [-n 100]` 和 `arb remote ls RUN [SUBPATH] [--out]`，不用 ssh；它们只能读当前项目的 `<runs>/<project>`、`<out>/<project>`。

## 训练代码约定

托管的 CLAUDE.md 区块里也有这几条：

1. 启动时把完整的解析后配置导出为 `$ARBORIA_OUT/config.yaml`（`arb poll` 拉回后存进账本的 `runs.config_json`）。
2. `wandb.init(id=os.environ["ARBORIA_WANDB_ID"], group=os.environ["ARBORIA_IDEA"])`。
3. 路径只从 `ARBORIA_*` 变量读取。
4. checkpoint 存到 `ARBORIA_CKPT_DIR`，并且可以续训（超过分区上限的 run 会分段）。

## 机器配置

机器配置在 `~/.config/arboria/clusters.toml`（可用环境变量 `ARBORIA_CONFIG_DIR` 改位置），示例见 [`examples/clusters.toml`](../examples/clusters.toml)：

```toml
[deltaai]
kind = "slurm"            # "fake" 也可以，用于测试和演练
host = "deltaai"          # ~/.ssh/config 里配了 ControlMaster 的别名
arch = "aarch64"
account = "<from `accounts`>"
gpus_per_node = 4
cpus_per_gpu = 72         # 288 核 / 4 卡：72 个 CPU 按一张卡计费
mem_per_gpu_gb = 110.898  # DefMemPerGPU 113560 MB；再多就按更多卡计费
default_partition = "ghx4"
max_segments = 4
balance_cmd = "accounts"
billing_per_su = 1000     # sacct 里 billing=1000 跑一小时 = 1 SU（0：本地估算）
prelude = []              # shell 行，例如 module load
env_activate = []         # 激活环境的 shell 行（容器或 uv/pixi venv）
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

- 上面的数字是 2026-11 在 DeltaAI 上实测的（见 [DeltaAI 说明](#deltaai-说明)）；`arb machine init <名字>` 会在集群上重新测量并打印（加 `--write` 写入）这张表。
- `arb machine init NAME [--host H] [--account A] [--write [--replace]]` 通过你已登录的 ssh master，在登录节点上只读地测量机器（`accounts`、`scontrol` 的分区/节点、计费权重、`quota`），打印 profile；`--write` 写进 `clusters.toml`（已有同名 profile 要加 `--replace`，会留备份，并保留你的 paths、prelude、env_activate）。不改 `~/.ssh/config`，没配 ControlMaster/ControlPersist 会提示要加的配置。
- `arb doctor [NAME]` 只读检查：profile 是否有效、ssh 配置和 master、架构、账户余额、分区时限和倍率、各路径（或最近的上级目录）可写、配额。没有 master 时只提示重新登录，不重试。NAME 默认取 `arboria.toml` 的 `default_machine`。
- 花费：作业结束后从 `sacct` 的 `AllocTRES` 里读 `billing=N`，SU = N × 小时数 / `billing_per_su`（与 `jobcharge --detail` 一致，已含分区倍率）；读不到就按 elapsed × 计费卡数 × `charge` 本地估算。
- 标着“待确认”的值（`<…>` 占位符）会被拒绝，并提示是哪一项。路径必须是绝对路径，只能用字母、数字、`._-/` 和 `$USER`（`#SBATCH` 行里会换成 `%u`）。
- `kind = "fake"` 时还需要 `root = "/某个本地目录"`：`FakeSlurm` 把“集群”模拟在本地，路径就是本地路径，适合演练整个流程。
- 预算读 `arboria.toml` 的 `[budget] total_su`；默认机器读 `default_machine`。
- **几个仓库可以共用一个机器配置。** run id 在每个仓库里都从 `r001` 开始，所以作业名和远程目录都带上 `arboria.toml` 的 `[project] id`（`arb init` 生成：主 worktree 目录名的 slug + `-` + 其绝对路径 sha256 的前 4 个十六进制字符，例如 `myproj-3f2a`）：作业名 `arb-<project>-<run>-s<k>`，远程目录 `<runs>/<project>/<run>`。`arb logs`、`arb remote ls` 和拉回 `ARBORIA_OUT` 只能读当前项目的 `<runs>/<project>`、`<out>/<project>`。项目 id 记在每个 run 的请求里（也算进审核单哈希），已提交的 run 一直用提交时的名字；有 run 在集群上时不要改它。
- 旧版本建的 `arboria.toml` 没有 `[project] id`：`submit`/`approve`/`poll`/`watch`/`logs`/`remote`/`cancel` 会拒绝并提示运行 `arb init` 补上。加入项目 id 之前提交的 run 不能再 `approve`（包括续交），请重新 `arb submit`；它们已有的作业仍可 `poll`、`cancel`。

## DeltaAI 说明

在 NCSA DeltaAI 上实测（2026-11）的情况：

- **计费**：`TRESBillingWeights` 是 CPU=13.888888、Mem=9.01726G、GRES/gpu=1000，`PriorityFlags=MAX_TRES`，所以一张卡的份额是 72 个 CPU、110.898 GB 内存（`DefMemPerGPU=113560` MB；节点可分配 454240 MB = 4 × 113560，所以不能往上取整），再多就按更多卡计费。
- **分区**：`ghx4` 上限 48h，`ghx4-interactive` 上限 2h（倍率 2×，已含在 `sacct` 的 billing 里）。`full`、`test` 等分区只给别的账户。
- **SU**：`jobcharge` 不接受作业 id（只按时间段汇总），所以 `arb poll` 从 `sacct` 的 `AllocTRES` 读 `billing=N`，SU = N × 小时 / 1000，和 `jobcharge --detail` 对得上。
- **登录节点**：aarch64，有 apptainer，没有 uv/pixi。路径在 `/projects/<proj>`、`/work/nvme/<proj>`、`/work/hdd/<proj>` 下的 `$USER/...`。
- **Duo**：先在你自己的终端里 `ssh deltaai` 完成认证，让 ControlMaster 保持连接；arb 只复用这个主连接（见[提交与批准](#提交与批准)）。
- **环境**（还没定）：计算节点上的 Python 环境（uv venv 或 apptainer 拉的 NGC PyTorch 容器）定下来后填进 profile 的 `prelude` / `env_activate`。在计算节点上构建环境的短作业同样要经 `arb submit` + 你的 `arb approve`。

## 看板：`arb ui`

每个 idea 的 notebook 是你干活的地方；看板是总览：什么在等你、idea 怎么随时间分叉、算力花在哪了。**它只读，什么都不改。**

```bash
arb ui                    # 在主 worktree 或任一 idea worktree 里运行；浏览器自动打开
arb ui --idea lr-warmup   # 预先选中某个 idea（在 idea worktree 里运行时默认选中那个 idea）
arb ui --port 2718
```

- 需要 marimo，而且要装在 arb 自己的环境里（看板会 import arboria）：在 arboria 源码目录 `uv tool install --force '.[ui]'`，或 `pip install 'arboria[ui]'`。没装时 `arb ui` 会直接告诉你怎么装。
- `arb ui` 在 `127.0.0.1` 上运行包里自带的 marimo app（`marimo run`，带 marimo 的会话 token），项目根目录通过 `--` 后的参数和 `ARBORIA_UI_ROOT` 传进去。Ctrl-C 结束。
- **只读**：账本用 SQLite 的只读模式打开（`file:…?mode=ro`），看板的代码不 import 任何写入路径；不连网、不 ssh、不调 wandb API。数据只来自账本、`.arboria/notifications.log`、`.arboria/runs/<run>/out/` 的文件列表、`arboria.toml`、`clusters.toml`（重算审核单）和 notebooks 目录。集群状态是 `arb watch` / `arb poll` 最后写进账本的样子；页面顶部的“更新于 HH:MM:SS”是最后一次读账本的时间。每 30 秒自动重读一次（右上角可改间隔）。账本里的文字一律 HTML 转义。
- **没有会动手的按钮**：批准、接受经验、取消都留在你自己的终端里；看板只把要运行的命令显示成可以一键选中复制的文本，例如 `arb approve r014`。
- 布局：顶部一行数字（需要你处理、进行中的 idea、活跃作业、已用 SU / 预算）；宽屏（≥1100px）左边是“待你处理”，右边是 idea 树和 idea 详情；最下面是算力与预算。窄屏变成一列。先给简短的版本，细节都在可以点开的折叠里。跟随 marimo 的浅色/深色主题（样式在包里的 `ui/dashboard.css`，不用网络字体，离线可用）。
- 内容：
  1. **待你处理**：每项一行。待批准的 run 带 `arb approve` 命令，审核单全文（和 `arb card` 一样，配置变了会标出“审核单已变化”）和 `arb reject` 收在“审核单”里；新结束的 run（你上次给那个 idea 写笔记之后结束的，写一条笔记后就不再列出）；待确认的经验和 `arb lesson accept` / `reject`；等 Claude 处理的待办；最近的 watch 通知（显示 3 条，其余折叠）。空的组缩成一行灰字。
  2. **idea 树**：横轴是时间，每个 idea 一行，父 idea 到子 idea 有连线（从某个 run 的快照分出的从那个 run 连出），行尾写着置信度（已合并、已放弃也写在这里；已放弃的行变淡，可以用开关隐藏）。run 是行上的小竖条，只有失败（红）和待批准（琥珀色）有颜色。鼠标悬停看详情。
  3. **idea 详情**（右上角下拉框选 idea）：假设、指标、置信度、notebook（`vscode://` 链接和 `marimo edit` 命令）；父 idea、分支、worktree、创建时间在“更多”里。runs 表每行是 run、状态、相对父 run 的改动（`k: 旧 → 新`，没有参数改动时显示原因）、GPU × 时长、SU、wandb，点开一行看原因、父 run、分区、估计/实际 SU、创建时间和拉回的输出文件。笔记和 run 总结显示最新 3 条，其余折叠；还有未完成的待办。
  4. **算力与预算**：已用 / 总预算（`arboria.toml [budget] total_su`）和剩余、各 idea 已用 SU、累计花费曲线、活跃作业（有的时候才显示），已结束 run 的估计 vs 实际（折叠）。
- 有东西在等你（待批准的 run、待确认的经验、你还没看过的已结束 run）时，`arb status` 和 SessionStart hook 会多一行“看板：arb ui”。
- 种子园里有在思考中的种子时，标题下多一行灰字“种子园：N 个在思考中 · arb ui --home”。

## 种子：还没有仓库的想法

想法常常还在酝酿：不属于任何仓库，也许永远不需要代码。**种子**就是这样的想法，放在一个全局的种子园里，有自己的 marimo notebook；等想清楚了，再**种进**某个仓库，变成一个普通的 idea。

```bash
arb seed new rank-schedule "LoRA 的秩随层深度变化" --metric "同参数量下的 GLUE 平均分"
marimo edit ~/arboria/seeds/rank-schedule.py      # 或在 VS Code 里用 marimo 扩展打开
arb seed list                                      # 最近编辑的在前；--all 含已放弃的
arb seed set rank-schedule --hypothesis "浅层需要更高的秩"
cd ~/code/lm && arb seed plant rank-schedule       # 在这个仓库里建 idea（分支、worktree、notebook）
```

- **种子园**：`~/arboria/`（环境变量 `ARBORIA_HOME` 可改）。任何目录下的第一个 `arb seed …`（以及仓库外的 `arb ui`、`arb install-claude --home`）都会自动建好它，并打印一行说明位置。里面是 `seeds/<名字>.py`（种子 notebook，普通文件，好找好打开）和 `.arboria/`（和项目一样的账本和备份仓库）。种子园本身不是 git 仓库。
- **字段**：名字（规则和 idea 一样）、一句话想法（≤300，可空）、可选的假设（≤300）和指标（≤200）、状态（思考中 / 已种下 / 已放弃）、种在哪里（仓库、项目 id、idea 名，可以有多处）、创建和最近编辑时间（notebook 文件改过也算）。
- **种子只由你写**：`arb seed new` / `set` / `drop` / `revive` / `plant` 在 Claude Code 里会被拒绝（和 `arb note` 一样），`arb install-claude --home` 也把它们设为 deny。Claude 可以读：`arb seed list`、`arb seed show`、种子 notebook。
- **种下**（`arb seed plant NAME [--repo PATH] [--as IDEA] [--hypothesis T] [--metric T] [--again]`）：目标仓库默认是当前目录所在的仓库，必须已经 `arb init`。需要假设和指标：种子里没有就用 `--hypothesis` / `--metric` 给出。它走和 `arb idea new` 完全一样的路径（从主 worktree 的 HEAD 建分支、worktree、notebook）；新 idea 的 notebook 开头写着“来自种子 NAME”和种子 notebook 的路径，下面一个折叠的“种子里的思路”里是种子 notebook 的**全部 markdown cell**（代码 cell 不复制：它们可能假设了别的环境）。种子变成“已种下”，两个账本里各记一条事件（项目的 `idea_new` 带上 `seed`，种子园的 `seed_plant`）。一个种子默认只种一次；`--again` 可以再种进别的仓库（或换个名字），每处都记下。
- **放弃**：`arb seed drop NAME`，notebook 留着。改主意了：`arb seed revive NAME` 让它回到“思考中”（只对已放弃的种子；种在哪里的记录不变）。

### 种子 notebook 里的 `lab`

`from arboria import lab` 在种子 notebook 里认得出这是种子（文件在 `<种子园>/seeds/` 下）：

- `lab.todo("…")` / `lab.todos()`：待办记在种子园的账本里，给在 `~/arboria` 里打开的 Claude Code 会话（`arb todo list` / `arb todo done` 在种子园里就是这些待办）。
- `lab.metric("…")` 改种子的指标；`lab.conf` 拒绝（种子没有置信度）。
- `lab.run`、`lab.get`、`lab.runs`、`lab.compare`、`lab.fork`、`lab.prune` 报错：“这是一个种子（NAME），还没种进仓库：先运行 arb seed plant NAME --repo …”（已经种下的，会说种在了哪里）。
- 自定义命令：只能用**全局**命令（`~/.config/arboria/commands/`）。命令里 `ctx.idea` 是 None，`ctx.seed` 有 name、gist、hypothesis、metric、notebook；`ctx.run` / `ctx.runs` 报同样的错。缺的命令会变成全局命令待办，并尽力推给种子园里的 Claude Code 会话。

### 在种子园里用 Claude Code

```bash
arb install-claude --home     # ~/arboria/.claude/settings.json 和 ~/arboria/CLAUDE.md
cd ~/arboria && claude
```

- settings.json：同样的 hook；deny 集群命令、`arb approve`、`arb lesson accept`、会写种子的 `arb seed new/set/drop/revive/plant`，以及对 `**/seeds/**` 和 `**/.arboria/**` 的编辑（读不受限）。全局命令目录直接写进这个 settings.json（种子园不是仓库，没有要提交的文件）。
- CLAUDE.md：只有一个 arboria 区块，写着种子园的规则（读种子、不改；待办怎么做；种下由人做）；区块外是你的。
- SessionStart hook 在种子园里（或它的子目录里）注入：思考中 / 已种下 / 已放弃的个数，最近编辑的 5 个种子和它们的一句话想法，未完成的待办。`arb status` 在种子园里显示同样的内容（不截断）。

### 种子园看板

`arb ui` 在任何仓库之外运行（或 `arb ui --home`）打开**种子园**：顶部是思考中 / 已种下 / 已放弃的个数，下面是种子卡片（按最近编辑排序：名字、状态、一句话想法、假设、种在哪里、`marimo edit` 命令和 VS Code 链接），已放弃的折叠在最后（每个旁边是 `arb seed revive NAME` 命令文本），右边是等 Claude 的待办。和项目看板一样只读。在仓库里，`arb ui` 仍然是项目看板，标题下多一行灰字“种子园：N 个在思考中 · arb ui --home”。

## Claude Code 设置

```bash
arb install-claude          # 写入当前 worktree 的 .claude/settings.json
arb install-claude --main   # 写入主 worktree
arb install-claude --user   # 写入 ~/.claude/settings.json（对所有项目生效）
arb install-claude --home   # 种子园：~/arboria/.claude/settings.json 和 ~/arboria/CLAUDE.md（见“种子”）
```

会合并以下内容（保留已有配置，不会重复添加）：

```json
{
  "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "arb hook session-start"}]}]},
  "permissions": {
    "deny": ["Bash(sbatch:*)", "Bash(srun:*)", "Bash(salloc:*)", "Bash(scancel:*)",
             "Bash(ssh:*)", "Bash(scp:*)", "Bash(rsync:*)",
             "Bash(arb approve:*)", "Bash(arb lesson accept:*)",
             "Edit(**/.arboria/notebooks/**)", "Write(**/.arboria/notebooks/**)", "...",
             "Edit(**/notebook.py)", "Write(**/notebook.py)", "MultiEdit(**/notebook.py)", "NotebookEdit(**/notebook.py)"]
  }
}
```

文件规则对 Edit、Write、MultiEdit、NotebookEdit 四个工具各有一条，覆盖 `**/.arboria/notebooks/**`、`**/.arboria/ledger.db*`、`**/.arboria/backup/**`、`**/.arboria/*.toml`、`**/.arboria/sessions/**` 和 `**/notebook.py`；读取不受限制，`.arboria/` 里的其他文件（特别是 `.arboria/commands/`）也不受限制。旧版本放在 `ask` 里的 `arb approve` / `arb lesson accept` 会被挪到 `deny`。

另外把全局命令目录（`~/.config/arboria/commands`，按 `ARBORIA_CONFIG_DIR`，写绝对路径，不存在就先建好）加进 `permissions.additionalDirectories`，Claude 才能在项目之外写全局命令。这个绝对路径只属于你这台机器，所以在项目里它写进 **`.claude/settings.local.json`**（arb 把它加进 `.git/info/exclude`，不会被提交），`.claude/settings.json` 保持可移植、可以提交；旧版本写在 `settings.json` 里的这一项（以及别人机器上的 `…/arboria/commands`）会被挪走。`--user` 时仍写在 `~/.claude/settings.json` 里。项目里的设置要你接受过这个文件夹的 workspace trust 之后才生效。

真正的保证是**只有 `arb approve` 会提交作业**，而且提交前代码会再次核对账本里有你的批准 Decision；这些 deny 规则只是额外一层，让 Claude 不能自己直接碰集群。Claude 应该用 `arb logs` / `arb remote ls` 看集群上的东西，而不是 ssh。如果你需要 Claude 上集群帮你排查问题，可以自己在 settings.json 里放宽 `Bash(ssh:*)` 这条规则（建议排查完再改回来）。

**注意：** 每个 idea 的 worktree 是独立的文件夹，Claude Code 只读取当前项目文件夹下的 `.claude/settings.json`。因此需要满足以下任一条件：

1. 在主 worktree 运行 `arb install-claude --main` 并把 `.claude/settings.json` **提交**到主分支——之后新建的 idea worktree 都会自带这份配置；或
2. 在每个 idea worktree 中分别运行 `arb install-claude`；或
3. 使用用户级安装 `arb install-claude --user`。hook 在非 arboria 仓库中不输出任何内容，所以全局安装是安全的。

hook 绝不会让会话失败：任何错误都只会写入 `.arboria/hook.log`，并以退出码 0 结束、不输出内容。注入的上下文上限约 4000 字符。

## CLAUDE.md 与经验

- `arb` 只管理主 worktree 的 `CLAUDE.md` 里的**一个区块**（`<!-- arboria:begin … -->` 到 `<!-- arboria:end -->` 之间），内容是使用 arboria 的稳定规则和**已确认的经验**，总长不超过 50 行。区块之外的内容都是你的，`arb` 一个字节也不会改。区块内的手动修改会在下次同步时被覆盖；标记损坏（只有开头没有结尾、重复等）时 `arb` 拒绝写入并提示你手动修复。
- `arb init`、`arb install-claude`、接受/退役经验时会自动同步区块，并只提交 `CLAUDE.md` 这一个文件（提交信息 `arboria: update CLAUDE.md`，使用你的 git 身份；你暂存的其他文件不受影响）。主 worktree 正在 merge/rebase/cherry-pick、`CLAUDE.md` 有冲突或没有配置 git 身份时只写文件、不提交。在 `arboria.toml` 中设 `[claudemd] autocommit = false` 可关闭自动提交。`arb claudemd sync [--no-commit]` 手动重写区块。
- **当前状态不写进 CLAUDE.md**：idea、run、待决事项由 SessionStart hook 在每次会话开始时注入。
- **经验由 Claude 提出、只由你确认**：Claude 用 `arb lesson propose "..."` 提出，你用 `arb lesson list` 查看，在你自己的终端里用 `arb lesson accept ID` 或 `arb lesson reject ID` 决定（`arb lesson accept` 在 Claude Code 里会被拒绝，`install-claude` 也把它设为 deny）。已确认的经验最多 20 条；满了就先用 `arb lesson retire ID` 退役或合并一些。
- idea worktree 是更早从主分支分出来的，里面的 `CLAUDE.md` 可能是旧的：新确认的经验会**立即**通过 hook 注入（最新 10 条，以及待你确认的条数），合并回主分支之后也会出现在 worktree 的 `CLAUDE.md` 里。

## 安全模型

Arboria 防的是“Claude 顺手照做”：会话里一句话、一段日志或一个命令文件让 Claude 自己去提交作业、批准、改你的 notebook。它**不是**针对同一台机器上、以你的身份运行的蓄意对手的沙箱。

| 层 | 做什么 |
| --- | --- |
| 账本 | 只有 `arb` 能写；写入一律加锁（fcntl + `BEGIN IMMEDIATE`）。LLM 只能写少数自由文本字段，都有长度校验；其余字段由代码生成和校验。 |
| 唯一的提交路径 | 只有 `arb approve` 会调用后端的 `submit`，而且提交前代码再查一次账本里有你的批准 Decision。没有任何自动批准的代码路径；`lab` 里没有批准函数。 |
| 只在你的终端里批准 | `arb approve` / `arb lesson accept` 在有 `CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT`、stdin 不是 TTY 或正在运行 notebook 命令时拒绝，然后要你输入 id 确认。 |
| 审核单哈希 | 批准绑定在审核单哈希上（快照、命令、资源、账户、机器、分区、分段计划、环境、项目 id）；任何一项变了，`arb approve` 就拒绝。 |
| Claude Code 权限 | `install-claude` 把 `sbatch`/`srun`/`salloc`/`scancel`/`ssh`/`scp`/`rsync`、`arb approve`、`arb lesson accept` 设为 deny，并禁止 Claude 编辑 notebook、账本、备份、`.arboria/` 里的 toml 和会话记录（可以读）。 |
| 远端 | `SshSlurm` 只复用你登录好的 ssh master（BatchMode，不弹 Duo），只认本项目的作业名，读写路径限制在本项目的 `<runs>/<project>`、`<out>/<project>` 下。 |
| 快照 | 不动你的分支、暂存区和工作区（`tests/test_snapshot.py` 守着）。 |
| 看板 | 只读：账本以 `mode=ro` 打开，不起进程、不联网、没有会动手的控件；文字 HTML 转义。 |
| notebook 命令 | Claude 写、在你的 kernel 里以你的权限运行；`arb` 自己从不执行命令代码（`arb commands list` 只用 `ast` 读）。用之前看 diff。 |
| 会话推送 | 只发给 hook 记下的本机会话 socket；不存不发 messaging token；消息只由校验过的字段拼成；失败静默跳过。 |
| 种子 | 只由你写：`arb seed new/set/drop/plant` 在 Claude Code 里拒绝；`install-claude --home` 禁止 Claude 编辑种子 notebook 和种子园的 `.arboria/`。种下（建分支和 worktree）只在你的终端里做。 |

## 命令参考

| 命令 | 说明 |
| --- | --- |
| `arb init` | 初始化（幂等）：`.arboria/`（SQLite 账本 + 备份用 git 仓库）、`arboria.toml` 模板（已存在则不覆盖，只在缺 `[project] id` 时补上这一项，其余内容一个字节都不动）、在 `.git/info/exclude` 中排除 `.arboria/` |
| `arb idea new NAME --hypothesis T --metric T [--parent NAME]` | 从主 worktree 的 HEAD（或父 idea 的分支）创建分支 `idea/NAME`、worktree 和 notebook，输出 worktree 路径 |
| `arb idea list [--json]` | 列出所有 idea |
| `arb idea notebook [NAME]` | 给没有 notebook 的 idea 补建 notebook（和 `notebook.py` 链接）；已有就不改 |
| `arb idea close [NAME] (--merge \| --drop) [--confidence X] [--allow-large] [--json]` | 关闭 idea（NAME 默认取当前目录所属的 idea）。`--drop`：最终快照存到 `refs/arboria/ideas/NAME/final` 并打标签 `arboria/archive/NAME`，然后删除 worktree 和分支。`--merge`：要求 worktree 没有未提交改动，在主 worktree 中 `git merge --no-ff`，冲突时自动 `git merge --abort` 并报告 |
| `arb run new --reason T [--allow-large] [--json]` | 快照当前 idea 的 worktree，创建状态为 `draft` 的 run，输出 run id |
| `arb run summary RUN_ID TEXT` | 为 run 写结果总结（≤10 行 / 1500 字符） |
| `arb submit --reason T [--from RUN] [--set K=V ...] [--gpus N] [--time DUR] [--nodes 1] [--partition P] [--machine M] [--mem GB] [--cpus N] [--repeat] [--allow-large] [--json] [-- CMD...]` | **只在本地**：快照当前 idea 的 worktree（`--from` 时复用那个 run 的快照，资源默认取它的），生成请求、分段计划和审核单，run 状态为 `awaiting_approval`，并生成待人决定的 Decision。不连接集群。没有 `-- CMD` 时用 `[run] command`；`--set` 按 `[run] override_style` 拼到命令后面。和已有 run 完全相同时返回那个 run（“已存在：rNNN”），除非加 `--repeat` |
| `arb card RUN [--json]` | 按当前机器配置重新计算并显示审核单；配置变了会提示“审核单已变化” |
| `arb approve RUN` | **只能在你自己的终端里运行**：核对审核单哈希，显示审核单，要你输入 run id 确认，记录批准 Decision，然后发送代码、逐段 `sbatch`。中途失败（包括需要重新登录）可以再次运行同一命令续交，不会重复提交 |
| `arb reject RUN [--reason T]` | 拒绝一个待批准的 run（理由存为该 run 的笔记），不连接集群 |
| `arb cancel RUN` | 取消已提交 run 的所有作业 |
| `arb poll [RUN] [--json]` | 查询一次作业状态，更新 run 状态和花费；结束后把 `ARBORIA_OUT` 拉回 `.arboria/runs/RUN/out/` |
| `arb watch [--once] [--interval 60] [--window MIN]` | 常驻轮询所有活跃 run；观察期内日志匹配 `[watch] fail_patterns` 或作业失败，就取消剩余作业并通知 |
| `arb logs RUN [--segment K] [-n 100]` | 看集群上某段作业的日志末尾（代替 ssh） |
| `arb remote ls RUN [SUBPATH] [--out]` | 列出集群上 run 目录（或 `ARBORIA_OUT`）里的文件（代替 ssh） |
| `arb machine init NAME [--host H] [--account A] [--write [--replace]]` | 在登录节点上只读地测量机器并打印 profile；`--write` 写进 `clusters.toml` |
| `arb doctor [NAME]` | 只读检查机器 profile、ssh 和集群一侧 |
| `arb note TEXT [--run ID] [--confidence X]` | **只由你来写**：给当前 idea 写笔记；`--confidence` 同时更新 idea 的置信度（在 Claude Code 里被拒绝） |
| `arb todo list [--all] [--json]` / `done ID --result T` / `drop ID [--reason T]` | notebook 里留下的待办：列出 / 完成（结果 ≤600 字符）/ 放弃（在种子园里是种子的待办）。命令待办标着 `[command_new]` / `[command_revise]`，`--json` 里的 `payload_json` 有调用细节 |
| `arb commands list [--json]` | 列出项目和全局的 notebook 命令（版本、描述、被项目同名命令覆盖的、有问题的）；不执行命令代码 |
| `arb commands guide` | 打印写 notebook 命令的约定（给 Claude 看） |
| `arb status [--json] [--brief]` | 在 idea worktree 中：该 idea 的假设、指标、置信度、runs、最近 5 条笔记、待决事项；在主 worktree 中：所有 open idea 概览；在种子园里：种子和待办 |
| `arb ui [--idea NAME] [--port N] [--home]` | 打开只读看板（marimo，127.0.0.1）：待你处理的事、idea 树、idea 详情、算力与预算；在仓库外或 `--home` 时是种子园；需要可选依赖 `ui` |
| `arb seed new NAME ["一句话想法"] [--hypothesis T] [--metric T]` | **只由你**：新种子和它的 notebook（`<种子园>/seeds/NAME.py`），输出 notebook 路径；任何目录下都能用，第一次用时建好种子园 |
| `arb seed list [--all] [--json]` / `show NAME [--json]` | 种子（最近编辑的在前；`--all` 含已放弃的）/ 一个种子的全部字段和种在哪里 |
| `arb seed set NAME [--gist T] [--hypothesis T] [--metric T]` | **只由你**：改种子的字段（`""` 清空） |
| `arb seed drop NAME` | **只由你**：放弃一个种子，notebook 留着 |
| `arb seed revive NAME` | **只由你**：已放弃的种子回到“思考中” |
| `arb seed plant NAME [--repo PATH] [--as IDEA] [--hypothesis T] [--metric T] [--again]` | **只由你**：把种子种进一个已 `arb init` 的仓库，成为普通 idea（分支、worktree、notebook，开头链接回种子并带上种子的 markdown）；输出 worktree 路径 |
| `arb export [--out FILE]` | 导出所有 idea 的 Markdown（新的在前），可直接粘贴到 Claude Desktop |
| `arb lesson propose TEXT [--idea NAME \| --run ID]` | 提出一条经验（单行、≤200 字符），同时生成一个待人确认的 Decision；来源默认取当前目录所属的 idea |
| `arb lesson list [--all] [--json]` | 列出经验（默认只列待确认和已确认的，`--all` 含已退役/已拒绝的） |
| `arb lesson accept ID` / `reject ID` | 接受 / 拒绝一条待确认的经验（接受后更新 CLAUDE.md）。`accept` **只能在你自己的终端里运行**，要输入经验 id 确认 |
| `arb lesson retire ID` | 退役一条已确认的经验，并从 CLAUDE.md 中移除 |
| `arb claudemd sync [--no-commit]` | 重写主 worktree 中 CLAUDE.md 的 arboria 区块 |
| `arb hook session-start` | Claude Code SessionStart hook（从 stdin 读取 hook JSON） |
| `arb install-claude [--main \| --user \| --home]` | 把 hook 和权限规则合并进 `.claude/settings.json`（全局命令目录写进不提交的 `.claude/settings.local.json`），并同步 CLAUDE.md 区块；`--home`：种子园的 settings.json 和 CLAUDE.md |

## `arboria.toml` 配置

`arboria.toml` 位于主 worktree 根目录，由 `arb init` 生成，应由你提交到仓库。

| 键 | 作用 |
| --- | --- |
| `default_machine` | 没指定 `--machine` 时用的 `clusters.toml` profile |
| `[project] id` | 项目 id（`arb init` 生成），进作业名、远程目录、`ARBORIA_WANDB_ID` 和审核单哈希；有 run 在集群上时不要改 |
| `[budget] total_su` | 项目总预算（SU）；0 = 没设 |
| `[claudemd] autocommit` | 更新 CLAUDE.md 区块后是否只提交这一个文件（默认 `true`） |
| `[watch] observation_minutes`、`fail_patterns` | 观察期长度和失败正则（见下） |
| `[wandb] entity`、`project` | 两个都填了，`arb submit` 才会记下 `https://wandb.ai/<entity>/<project>/runs/<ARBORIA_WANDB_ID>`；notebook 里读曲线也用它们 |
| `[run] command`、`override_style` | 默认训练命令（默认 `python train.py`）；`--set k=v` 的拼法：`hydra` → `k=v`，`argparse` / `tyro` → `--k v` |
| `[claude] push` | 命令待办是否尽力推给正在运行的 Claude Code 会话（默认 `true`） |
| `[review] checklist` | 提交前审稿的检查清单（还没被读取，第 2 步用） |

```toml
[wandb]
entity = "my-team"
project = "lm"

[run]
command = "python train.py"   # 没有 `-- CMD` 时（以及 lab.run）用它
override_style = "hydra"      # hydra → k=v；argparse / tyro → --k v

[watch]
observation_minutes = 10
fail_patterns = ['(?i)\bloss\b[^\n]{0,40}\bnan\b', '(?i)\bnan\b[^\n]{0,40}\bloss\b', 'CUDA out of memory', 'torch\.OutOfMemoryError']
```

- 旧文件没有 `[wandb]` / `[run]` 也能用：只有真正需要时（例如没给 `-- CMD` 又没有 `[run] command`）才报错。
- 没有 `[watch]` 表时就用上面的默认值；写了 `fail_patterns` 就完全替换默认列表。正则按 Python `re` 语法逐行匹配日志末尾 50 行，加载时就检查，写错了会直接报出是第几条。默认只认 loss 为 NaN 和 CUDA OOM，像 `0 NaN batches skipped` 这样的行不会触发。

环境变量：`ARBORIA_CONFIG_DIR` 改变 `clusters.toml` 和全局命令库的位置（默认 `~/.config/arboria`）；`ARBORIA_HOME` 改变种子园的位置（默认 `~/arboria`）。

## 状态存放位置

- 主 worktree = `git rev-parse --git-common-dir` 的父目录；所有 worktree 共享 `<主 worktree>/.arboria/`。
- `.arboria/ledger.db`：SQLite（WAL 模式），schema 版本记录在 `PRAGMA user_version`（旧版本的账本原地迁移）。
- `.arboria/` 本身是一个只用于备份的小 git 仓库：在 idea 新建/关闭、笔记、新 run、决策、经验提出/接受/拒绝/退役、待办新建/完成/放弃、置信度和指标修改之后，以及 run 的提交、批准、进入队列、拒绝、取消、结束和存入 config 之后，会把数据库导出到 `.arboria/backup/ledger.sql`，连同 `notebooks/*.py` 和 `commands/*.py` 一起提交（轮询这类心跳更新只写数据库，不提交）。
- `.arboria/notebooks/<idea>.py`：每个 idea 的 notebook（备份仓库每次提交都带上）。
- `.arboria/commands/<名字>.py`：项目的 notebook 命令（Claude 写；随备份提交进备份仓库，不进快照）；`.arboria/cache/commands/<名字>/`：命令的缓存；`.arboria/sessions/`：hook 记下的 Claude Code 会话 socket（0600）。
- `.arboria/runs/<run>/out/`：拉回来的 `ARBORIA_OUT`；其中有 `config.yaml` 时，它的内容会存进 `runs.config_json`（装了 PyYAML 就解析成 JSON，否则原文存在 `_raw_yaml` 下，最多 200 KB）。
- `.arboria/notifications.log`：watch/poll 的通知；`.arboria/hook.log`：hook 的错误。
- `~/.config/arboria/clusters.toml`：机器配置；`~/.config/arboria/commands/`：全局 notebook 命令。
- `~/arboria/`（`ARBORIA_HOME`）：种子园。`seeds/<名字>.py` 是种子 notebook；`.arboria/` 和项目的一样（账本、备份仓库、`sessions/`、`cache/`），另有 `.arboria/seeds/`：每次备份提交前把 `seeds/*.py` 复制进来，备份仓库就版本化了种子 notebook。账本 schema 和项目的相同（v6 加了 `seeds`、`plantings`，待办属于一个 idea 或一个种子）。

## 当前状态与已知限制

- **已完成并有测试**：本地账本、idea 生命周期、快照、笔记、status/export、SessionStart hook、install-claude、托管的 CLAUDE.md 区块与经验；提交 / 审核单 / 批准 / 拒绝、分段、计费、Slurm 作业记录、poll / watch / cancel / logs / remote ls（用 `FakeSlurm` 和 mock 的 subprocess 测试）；`arb machine init` / `arb doctor`（用真实的 DeltaAI 探测输出测试，并已对 DeltaAI 跑通）；每个 idea 的 notebook、`lab` API、待办、notebook 命令和自动重跑（用真的 marimo 测试）；只读看板；种子、种下、种子园看板和种子园里的 Claude Code 设置。
- **还没在真集群上跑通**：通过 `SshSlurm` 的第一个真实 run（submit → approve → watch → 拉回 `ARBORIA_OUT`，核对命令行、日志路径、`sacct` 状态和花费），以及计算节点上的环境（`prelude` / `env_activate`）。
- `find_job` 用 `sacct --starttime=now-30days` 查找已有作业。
- 加入项目 id 之前提交的 run 不能再 approve，要重新 submit。
- 自动重跑依赖 marimo 的私有接口；marimo 改了接口时退回成“重新运行这个 cell”的提示。
- 之后的计划：提交前检查、审核包和独立的 AI 审稿人、基于历史的时长/花费估计（以及提交前的排队估计）；看板上的 wandb 曲线离线缓存。

## 开发

```bash
uv sync --all-extras --dev
uv run pytest            # 测试使用临时 git 仓库、FakeSlurm 和假 Unix socket，不碰真集群和真会话
```

- CI（`.github/workflows/tests.yml`）在 Python 3.11 和 3.13 上跑全部测试，包括真的 marimo（`marimo check --strict`、`marimo export html`、真 kernel 里的自动重跑）。
- macOS 可能给整个 `.venv` 加 `hidden` 标志，Python 3.13 会跳过 hidden 的 `.pth`，editable 安装因此失效（`No module named 'arboria'`）。所以 pytest 配了 `pythonpath = ["src"]`，起子进程的测试自己传 `PYTHONPATH`。
- 给开发这个工具的 Claude 看的约束在 [`CLAUDE.md`](../CLAUDE.md)。
