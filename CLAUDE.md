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
- 下一步 1b，接入 NCSA DeltaAI（aarch64 GH200、Slurm、登录需 Duo）。必须在用户本机上做：
  1. ssh：host `deltaai` 配 ControlMaster/ControlPersist。`arb doctor` 发现连接断了就提示用户重新登录，绝不循环重试，因为每次重试都会推送一次 Duo。
  2. `arb machine init deltaai` 生成 `~/.config/arboria/clusters.toml`，内容包括：登录节点架构、账户名（来自 `accounts`）、项目目录、文件数配额（来自 `quota`）、计算节点能否上网。
  3. 环境：在 NGC PyTorch arm64 容器 + uv 和 pixi 之间实测选一个。用一个短作业在计算节点上构建，缓存 key 为 lockfile 哈希 + 架构 + CUDA 版本。环境放在 /projects 下。
  4. Slurm 后端（status/check/submit/poll/cancel）：
     - 永远设置 `--time` 和 `--account`。
     - DeltaAI 按 GPU、内存、CPU 三者折算后的最大值计费，所以 `--mem` 和 `--cpus` 要按卡数配比，保证计费卡数等于申请卡数。
     - 快照用 `git archive | ssh tar` 发送。
     - walltime 超过分区上限时用 `--dependency` 分段。分区上限：`ghx4` 48h；`ghx4-interactive` 2h，计费 2 倍。
     - 实际花费从 `jobcharge` / `sacct` 取。
  5. `arb submit` 生成状态为 awaiting_approval 的 run 和审核单（检查、资源、预算）。`arb approve` / `arb reject` 生成 Decision。
  6. 本机常驻 watcher，每分钟轮询一次。开跑后约 10 分钟是观察期：崩溃或 NaN 就取消并通知。跑完把 `ARBORIA_OUT` 拉回本地。
- 之后：第 2 步做提交前检查、审核包和独立 AI 审稿人、基于历史的估计；第 3 步做 marimo 面板。

## 使用 arboria 的训练代码要遵守的约定
1. 启动时把完整的解析后配置导出为 yaml。
2. `wandb.init(id=ARBORIA_RUN_ID, group=ARBORIA_IDEA)`。
3. 路径只从 `ARBORIA_*` 角色变量读取。
4. checkpoint 存到 `ARBORIA_CKPT_DIR`，并且可以续训。
