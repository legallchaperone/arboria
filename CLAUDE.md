# Arboria：给开发这个工具的 Claude 看的说明

Arboria（命令 `arb`）把 ML 研究的状态从 Claude 对话里搬出来：idea、run、note、decision 都存在 SQLite 账本里，只有 `arb` 能写。设计说明（中文网页）：https://claude.ai/artifact/NCqpKzbhDfAXSURgGTc1zn

## 不能破坏的约束
- LLM 能写的字段只有：hypothesis、metric、change_reason、note、run summary、lesson、todo result（≤600），以及以后的 review。（note 在 Claude Code 里默认拒绝，只由人写。）其余字段全部由代码写入并校验。长度校验不能删。
- 账本写入一律走 `db` 里的加锁写入（fcntl + `BEGIN IMMEDIATE`）。备份 commit 只在指定的事件类型后触发，心跳类更新不触发。
- 快照绝不能动用户的分支、index 和工作区。`tests/test_snapshot.py` 就是这条约束的契约。
- `arb hook session-start` 绝不能失败，也不能输出多余内容：出任何错都 exit 0、不输出。
- 任何东西提交到 Slurm 之前，都必须有人类批准过的 Decision。不要写任何自动批准的代码路径。`arb approve` 和 `arb lesson accept` 只能在人自己的终端里做（`human.require_terminal`：有 `CLAUDECODE`/`CLAUDE_CODE_ENTRYPOINT` 或 stdin 不是 TTY 就拒绝，然后要人输入 id），install-claude 把它们设为 deny。`lab` 里没有任何批准函数。
- 1a 的代码路径里不发网络请求。远端操作放在后端接口后面。

## 代码结构
`src/arboria/`：cli（typer）、paths、gitutil、db、state（init）、snapshot、ideas、records、report（status/export）、hook、install、claudemd、clusters、config、slurm、backends/、submission、monitor、machine（machine init / doctor）、human（只在人的终端里批准）、notebooks、tasks（待办）、lab（notebook API）。`tests/` 使用临时 git 仓库。

## 开发
- `uv run pytest` 必须保持全绿。用 `uv tool install .` 装好后试 `arb`。
- commit 要小、逻辑清楚。
- 本机（macOS）会给整个 `.venv` 加 `hidden` 标志，Python 3.13 跳过 hidden 的 `.pth`，editable 安装就失效（`No module named 'arboria'`）。所以 pytest 配了 `pythonpath = ["src"]`，起子进程的测试要自己传 `PYTHONPATH`。

## 进度
- 已完成 1a：本地账本、idea 生命周期、快照、note、status/export、hook、install-claude、托管的 CLAUDE.md 区块与 lesson。
- 已完成 1b 中不需要真集群的部分（全部用 FakeSlurm 和 mock 的 subprocess 测试）：
  - schema v3（runs 加 `rejected`、`request_json`、`card_hash`；`jobs` 表每段一行），v1/v2→v3 原地迁移。
  - `clusters`（`~/.config/arboria/clusters.toml`，`ARBORIA_CONFIG_DIR` 可覆盖，示例 `examples/clusters.toml`）、`config`（arboria.toml）、`slurm`（纯函数：walltime、DeltaAI 计费、分段、sbatch 渲染、审核单哈希）。
  - `backends/`：`Backend` 接口、`FakeSlurm`（本地模拟、可编排结果、手动时钟）、`SshSlurm`（`ssh -O check` 门禁 + `NeedLogin`、BatchMode、读取和发送路径限制在本项目的 `<runs>/<project>`、`<out>/<project>` 下）。
  - `submission`：`arb submit`（纯本地）/`card`/`approve`/`reject`。`_guarded_submit` 是 `Backend.submit` 唯一的调用点，提交前再查账本里人的批准 Decision；`tests/test_submit.py` 用源码扫描和 spy 守住这一点。
  - `monitor`：`arb poll`/`cancel`/`watch`（观察期内日志匹配 `arboria.toml [watch] fail_patterns` 就取消，通知里写明正则和那一行；NeedLogin 时暂停该机器）/`logs`/`remote ls`。
  - 项目 id（`arboria.toml [project] id`，`arb init` 生成/补上）：作业名 `arb-<project>-<run>-s<k>`，远程目录 `<runs>/<project>/<run>`，ckpt/out 同理。项目 id 存在 request_json 里、算进审核单哈希；后端绑定一个项目，只认本项目的作业名和路径。
  - install-claude 的 deny 规则（sbatch/srun/salloc/scancel/ssh/scp/rsync），CLAUDE.md 区块和 hook 文案更新。
- 1b 在用户本机上做的部分：
  1. ✅ profile 已实测（2026-11-02）并写入用户的 `~/.config/arboria/clusters.toml`：account `bjki-dtai-gh`，`cpus_per_gpu = 72`、`mem_per_gpu_gb = 110.898`（TRESBillingWeights CPU=13.888888、Mem=9.01726G、GRES/gpu=1000，PriorityFlags=MAX_TRES；DefMemPerGPU=113560 MB，节点可分配 454240 MB = 4×113560，所以不能往上取整），ghx4 48h ×1、ghx4-interactive 2h ×2（`full`/`test` 只给别的账户）。路径在 `/projects/bjki`、`/work/nvme/bjki`、`/work/hdd/bjki` 下的 `$USER/...`。登录节点 aarch64，有 apptainer，没有 uv/pixi。
     - `jobcharge` 不接受作业 id（只按时间段汇总），原来的 `jobcharge <id>` 永远失败。现在 `poll` 从 `sacct` 的 `AllocTRES` 读 `billing=N`，SU = N × 小时 / `billing_per_su`（1000），和 `jobcharge --detail` 对得上；profile 里 `charge_cmd` 换成了 `billing_per_su`。
  2. ✅ `arb machine init` / `arb doctor`（`machine.py`，测试用 `tests/fixtures/deltaai_probe.txt` 里真实的探测输出）。对 deltaai 跑过，全部通过。
  3. 环境（还没做）：用户已有 uv 的做法（`/work/hdd/bjki/$USER/opt/uv-aarch64`、`venvs/WaveFront-gh`、`/work/nvme/bjki/$USER/envproj` 里的 uv.lock），倾向 uv；NGC PyTorch 容器要用 apptainer 自己拉（没有对应 module）。计算节点能不能上网还没确认。需要短作业在计算节点上构建 → 必须经 `arb submit` + 用户 `arb approve`，Claude 不能自己 sbatch。定下来后填 `prelude` / `env_activate`。
  4. 通过 `SshSlurm` 跑通第一个真实 run（还没做）：submit → approve → watch → 拉回 `ARBORIA_OUT`，核对命令行、`%u` 日志路径、`sacct` 状态和花费。
- 已知限制：`find_job` 用 `sacct --starttime=now-30days`；加入项目 id 之前提交的 run 不能再 approve，要重新 submit。
- **分支 `notebook-api`**（基于 1b 之后的 main，没在真集群上跑过）：每个 idea 一个 marimo notebook（`.arboria/notebooks/<idea>.py`，worktree 里链接成 `notebook.py`，`info/exclude` 排除，备份仓库跟踪，Claude 只读）；`arboria.lab` notebook API（`lab.run` 只生成审核单，不批准）；schema v4（`runs.parent_run`、`decisions.payload_json`、`notes.author`、`tasks`）；`arb submit --from rNNN --set k=v` 和按审核单哈希的幂等提交；`arboria.toml [wandb]`/`[run]`、`ARBORIA_WANDB_ID`、`runs.wandb_url`、拉回 `out/config.yaml` 存进 `runs.config_json`；`arb todo list/done/drop`，hook 列出待办；批准只在人的终端里（见上面的约束）；`arb note` 在 Claude Code 里被拒绝。用户的真集群工作合进 main 之后，这个分支要再 rebase/合并进 main。
- 之后：第 2 步做提交前检查、审核包和独立 AI 审稿人、基于历史的估计（以及提交前的排队估计）；第 3 步做 marimo 面板。

## 使用 arboria 的训练代码要遵守的约定
1. 启动时把完整的解析后配置导出为 `$ARBORIA_OUT/config.yaml`（`poll` 拉回后存进 `runs.config_json`）。
2. `wandb.init(id=ARBORIA_WANDB_ID, group=ARBORIA_IDEA)`（`ARBORIA_WANDB_ID` = `<项目 id>-<run id>`，几个仓库共用一个 wandb project 也不冲突）。
3. 路径只从 `ARBORIA_*` 角色变量读取。
4. checkpoint 存到 `ARBORIA_CKPT_DIR`，并且可以续训。
