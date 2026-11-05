---
title: 路线图与状态
description: 哪些在测试里验证过，哪些在真集群上跑过，哪些还没做，以及已知限制和开发方式。
---

Arboria 是一个个人研究工具，正在积极开发，接口可能会变。

## 已完成，有测试

CI 在 Python 3.11 和 3.13 上运行全部测试，测试用临时 git 仓库、`FakeSlurm`、mock 的 ssh 和假 Unix socket，不碰真集群和真会话。

- 本地账本、idea 生命周期、快照、笔记、status / export、SessionStart hook、install-claude、托管的 CLAUDE.md 区块与经验。
- 提交、审核单、批准、拒绝、分段、计费、Slurm 作业记录、poll / watch / cancel / logs / remote ls。
- 每个 idea 的 notebook、`lab` API、待办、notebook 命令和自动重跑（用真的 marimo）。
- 只读看板和种子园看板。
- 种子、种下、种子的放弃和恢复，种子园里的 Claude Code 设置。
- Rust 核心和 `arb`（第 1 期）：和 Python 版逐字节对照的测试向量、输出对比。
- Mac app 的壳（第 2 期，ADR 0004）：收件箱、全局、idea 页、审核单和触控 ID 签字、嵌入终端、marimo 窗口、后台小助手（`arb helper`）和系统通知、项目登记（`arb projects`）。核心部分在 Linux 上测试；app 在 Linux 上构建并实际启动过（Xvfb），界面截图用演示数据。

## 在 DeltaAI 上实测过

- 机器 profile 和计费权重（2026-11）。
- `arb machine init` / `arb doctor`，全部检查通过。

## 还没做

- 在 Mac 上验收 app：`npm run tauri build`、触控 ID 签字、LaunchAgent 小助手的通知（这些只能在 Mac 上测；CI 的 macOS 任务只检查能编译）。签名和公证需要 Apple 开发者账号。

- 通过 `SshSlurm` 的**第一个真实 run**：submit → approve → watch → 拉回 `ARBORIA_OUT`，核对命令行、日志路径、`sacct` 状态和花费。
- 计算节点上的 Python 环境（uv venv 或 NGC PyTorch 容器），以及计算节点能不能上网。

## 之后

- 提交前检查、审核包和独立的 AI 审稿人（只看资源申请是否合理）。
- 基于历史的时长和花费估计，以及提交前的排队估计。
- 看板上 wandb 曲线的离线缓存（看板本身不联网）。

## 已知限制

- `find_job` 用 `sacct --starttime=now-30days` 查找已有作业。
- 加入项目 id 之前提交的 run 不能再 approve，要重新 submit。
- 自动重跑依赖 marimo 的私有接口；marimo 改了接口时退回成“重新运行这个 cell”的提示。
- `refs/arboria/*` 默认不会被 push，换机器 clone 时不会带过去。

## 开发

```bash
uv sync --all-extras --dev
uv run pytest
```

- macOS 可能给整个 `.venv` 加 `hidden` 标志，Python 3.13 会跳过 hidden 的 `.pth`，editable 安装因此失效（`No module named 'arboria'`）。所以 pytest 配了 `pythonpath = ["src"]`。
- 这个网站在仓库的 `docs/` 里（Astro + Starlight）：`cd docs && npm ci && npm run dev`。CLI 参考页由 `uv run python docs/scripts/gen_cli.py` 生成，测试会检查它是不是最新的。
- 给开发这个工具的 Claude 看的约束在仓库根目录的 [`CLAUDE.md`](https://github.com/legallchaperone/arboria/blob/main/CLAUDE.md)。
