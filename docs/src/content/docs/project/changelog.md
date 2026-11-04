---
title: 更新日志
description: 按日期整理的主要变化，来自 git 历史。
---

还没有发布版本，版本号是 0.1.0。下面按日期列出主要变化；细节见 [提交历史](https://github.com/legallchaperone/arboria/commits/main)。

## 2026-11-04

- **读代码** `lab.inspect(question, view=..., run=...)`：agent 讲代码（代码摘录加批注、伪代码、简化代码、Mermaid 架构图），答案缓存在账本里（schema v7，`inspections`）。agent 只给位置和批注，代码由 arb 从 git 取出；代码变了标出来，`.refresh()` 再问。`arb inspect guide/source/check/done`。
- **种子并进 `arb idea`**：种子就是还没有仓库的 idea。在仓库外（或种子园里），`arb idea new/list/show/set/drop/revive` 作用于种子；在没有 `arb init` 的仓库里照旧报错；在仓库里照旧是 idea。`arb idea plant` 在目标仓库里运行。idea 也有了 `arb idea show` 和 `arb idea drop`（= `close --drop`）。`arb seed …` 保留为隐藏的别名。
- `arb --help` 分成四组：你做的、集群、agent 做的、安装与内部。只做本地快照的 `arb run new` 不再列出（被 `arb submit` 取代，仍然可用）。
- **网站**：项目主页和文档搬到 arboria.cwx.world（Astro + Starlight），取代 `docs/guide.md`；CLI 参考从命令行生成。
- **种子**：schema v6（`seeds`、`plantings`，待办属于一个 idea 或一个种子）。`arb seed new/list/show/set/drop/plant`，任何目录可用；种下的 idea notebook 链接回种子并带上种子的 markdown。
- 种子 notebook 里的 `lab`：待办和全局命令可用，run 相关的函数提示先种下。
- 种子园里的 Claude Code：hook、`arb status`、`arb todo`、`arb install-claude --home`。
- 种子园看板 `arb ui --home`（在仓库外默认）。
- `arb seed revive`：已放弃的种子回到思考中。
- `lab.todo` / `lab.todos()` 在 notebook 里显示成简短的一行，不再是 JSON 树。

## 2026-11-03

- **notebook**：每个 idea 一个 marimo notebook，链接进 worktree；`arboria.lab` API；待办（schema v4）。
- 批准只在人的终端里；笔记只由人写。
- `arb submit --from RUN --set k=v`，相同的提交返回已有的 run；`--repeat`。
- run 的 config、wandb id 和拉回的 `config.yaml`。
- **notebook 命令**（schema v5）：`lab.<名字>(...)`、命令待办、修改、自动重跑；推送待办给正在运行的 Claude Code 会话。
- **看板** `arb ui`：只读的 marimo app，待你处理、idea 树、idea 详情、算力与预算；浅色和深色。
- install-claude：全局命令目录写进不提交的 `.claude/settings.local.json`。
- CI 在 Python 3.11 / 3.13 上运行；MIT 许可证；README 精简，完整说明放进 `docs/guide.md`。

## 2026-11-02

- 第一版：uv 包和 `arb` 命令；SQLite 账本（迁移、加锁、git 备份）；worktree 快照、idea 生命周期、run、笔记和决定；status / export、SessionStart hook、install-claude。
- 托管的 CLAUDE.md 区块和经验（schema v2）。
- **集群**（schema v3）：机器 profile、Slurm 规划（时长、DeltaAI 计费、分段、sbatch 渲染、审核单哈希）、`FakeSlurm` 和 `SshSlurm` 后端；`arb submit/card/approve/reject` 和 `poll/cancel/watch/logs`。
- 项目 id 进作业名和远程目录；可配置的观察期失败正则。
- 花费改从 `sacct` 的 billing 读取；DeltaAI 实测的 CPU / 内存份额。
- `arb machine init` 和 `arb doctor`。
