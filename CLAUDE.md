# Arboria：给开发这个工具的 Claude 看的说明

Arboria（命令 `arb`）把 ML 研究的状态从 Claude 对话里搬出来：idea、run、note、decision 都存在 SQLite 账本里，只有 `arb` 能写。设计说明（中文网页）：https://claude.ai/artifact/NCqpKzbhDfAXSURgGTc1zn

## 不能破坏的约束
- LLM 能写的字段只有：hypothesis、metric、change_reason、note、run summary、lesson，以及以后的 review。其余字段全部由代码写入并校验。长度校验不能删。
- 账本写入一律走 `db` 里的加锁写入（fcntl + `BEGIN IMMEDIATE`）。备份 commit 只在指定的事件类型后触发，心跳类更新不触发。
- 快照绝不能动用户的分支、index 和工作区。`tests/test_snapshot.py` 就是这条约束的契约。
- `arb hook session-start` 绝不能失败，也不能输出多余内容：出任何错都 exit 0、不输出。
- 任何东西提交到 Slurm 之前，都必须有人类批准过的 Decision。不要写任何自动批准的代码路径。`arb approve` 和 `arb lesson accept` 在 Claude Code 里设为 ask。
- 1a 的代码路径里不发网络请求。远端操作放在后端接口后面。

## 代码结构
`src/arboria/`：cli（typer）、paths、gitutil、db、state（init）、snapshot、ideas、records、report（status/export）、hook、install、claudemd。`tests/` 使用临时 git 仓库。

## 开发
- `uv run pytest` 必须保持全绿。用 `uv tool install .` 装好后试 `arb`。
- commit 要小、逻辑清楚。

## 进度
- 已完成 1a：本地账本、idea 生命周期、快照、note、status/export、hook、install-claude、托管的 CLAUDE.md 区块与 lesson。
- 已完成 1b 中不需要真集群的部分（全部用 FakeSlurm 和 mock 的 subprocess 测试）：
  - schema v3（runs 加 `rejected`、`request_json`、`card_hash`；`jobs` 表每段一行），v1/v2→v3 原地迁移。
  - `clusters`（`~/.config/arboria/clusters.toml`，`ARBORIA_CONFIG_DIR` 可覆盖，示例 `examples/clusters.toml`）、`config`（arboria.toml）、`slurm`（纯函数：walltime、DeltaAI 计费、分段、sbatch 渲染、审核单哈希）。
  - `backends/`：`Backend` 接口、`FakeSlurm`（本地模拟、可编排结果、手动时钟）、`SshSlurm`（`ssh -O check` 门禁 + `NeedLogin`、BatchMode、只读路径限制在 runs/out 下）。
  - `submission`：`arb submit`（纯本地）/`card`/`approve`/`reject`。`_guarded_submit` 是 `Backend.submit` 唯一的调用点，提交前再查账本里人的批准 Decision；`tests/test_submit.py` 用源码扫描和 spy 守住这一点。
  - `monitor`：`arb poll`/`cancel`/`watch`（观察期 NaN / CUDA OOM 取消，NeedLogin 时暂停该机器）/`logs`/`remote ls`。
  - install-claude 的 deny 规则（sbatch/srun/salloc/scancel/ssh/scp/rsync），CLAUDE.md 区块和 hook 文案更新。
- 1b 剩下的部分必须在用户本机上做（会碰真集群和 Duo）：
  1. 确认 profile 的值：`cpus_per_gpu`、`mem_per_gpu_gb`（决定计费卡数 = 申请卡数）、`account`（来自 `accounts`）、各 `paths`、登录节点架构、计算节点能否上网；`jobcharge` 的输出格式（`SshSlurm.parse_charge` 目前是猜的，解析不了就按 elapsed × 计费卡数 × 倍率本地估算）。
  2. `arb machine init deltaai` 和 `arb doctor`（都还没写）：ssh host `deltaai` 配 ControlMaster/ControlPersist；连接断了只提示用户重新登录，绝不循环重试。
  3. 环境：在 NGC PyTorch arm64 容器 + uv 和 pixi 之间实测选一个，用短作业在计算节点上构建（缓存 key = lockfile 哈希 + 架构 + CUDA 版本，放在 /projects 下），定下 profile 的 `prelude` / `env_activate`。
  4. 通过 `SshSlurm` 跑通第一个真实 run：submit → approve → watch → 拉回 `ARBORIA_OUT`，核对命令行、`%u` 日志路径、`sacct` 状态和花费。
- 已知限制：作业名和远程目录不带仓库名，一个机器配置只能给一个仓库用（README 有说明）；`find_job` 用 `sacct --starttime=now-30days`。
- 之后：第 2 步做提交前检查、审核包和独立 AI 审稿人、基于历史的估计（以及提交前的排队估计）；第 3 步做 marimo 面板。

## 使用 arboria 的训练代码要遵守的约定
1. 启动时把完整的解析后配置导出为 yaml。
2. `wandb.init(id=ARBORIA_RUN_ID, group=ARBORIA_IDEA)`。
3. 路径只从 `ARBORIA_*` 角色变量读取。
4. checkpoint 存到 `ARBORIA_CKPT_DIR`，并且可以续训。
