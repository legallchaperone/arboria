---
title: 集群与机器配置
description: clusters.toml 里的机器 profile、arb machine init 和 arb doctor、DeltaAI 上实测的要点和计费规则。
---

机器配置在 `~/.config/arboria/clusters.toml`（`ARBORIA_CONFIG_DIR` 可改位置），一张表一台机器。每一项的说明见 [clusters.toml 参考](/reference/clusters-toml/)；仓库里有示例 [`examples/clusters.toml`](https://github.com/legallchaperone/arboria/blob/main/examples/clusters.toml)。

## 先登录一次

DeltaAI 登录要过 Duo。arb 只复用你已经登录好的 ssh 主连接，所以 `~/.ssh/config` 里要有 ControlMaster：

```text
Host deltaai
  HostName dtai-login.delta.ncsa.illinois.edu
  User <NCSA 用户名>
  ControlMaster auto
  ControlPath ~/.ssh/cm-%r@%h:%p
  ControlPersist 12h
```

然后在你自己的终端里 `ssh deltaai` 完成认证。arb 不改 `~/.ssh/config`；没配好时会提示要加的内容。

## 测量并写入 profile

```bash
arb machine init deltaai                   # 只读地测量，打印 profile
arb machine init deltaai --write           # 写进 clusters.toml
arb machine init deltaai --write --replace # 覆盖已有的（留备份，保留 paths、prelude、env_activate）
arb doctor deltaai                         # 只读检查，随时可以跑
```

- `arb machine init NAME [--host H] [--account A]` 通过 ssh 主连接在登录节点上运行 `accounts`、`scontrol`（分区、节点、计费权重）和 `quota`，只读。
- `arb doctor [NAME]` 检查：profile 是否有效、ssh 配置和主连接、架构、账户余额、分区时限和倍率、各路径（或最近的上级目录）可写、配额。没有主连接时只提示重新登录，不重试。NAME 默认取 `arboria.toml` 的 `default_machine`。
- 标着 `<…>` 的占位值会被拒绝，并提示是哪一项。

## DeltaAI：实测要点

2026-11 在 NCSA DeltaAI 上实测：

| 项 | 实测 |
| --- | --- |
| 架构 | 登录节点 aarch64（Grace），每节点 4 张 GH200；有 apptainer，没有 uv/pixi |
| 分区 | `ghx4` 上限 48h，倍率 1×；`ghx4-interactive` 上限 2h，倍率 2×（已含在 `sacct` 的 billing 里）；`full`、`test` 只给别的账户 |
| 计费权重 | `TRESBillingWeights` = CPU 13.888888、Mem 9.01726G、GRES/gpu 1000，`PriorityFlags=MAX_TRES` |
| 每卡份额 | 72 个 CPU、110.898 GB 内存（`DefMemPerGPU=113560` MB；节点可分配 454240 MB = 4 × 113560，所以不能往上取整） |
| 路径 | `/projects/<proj>`、`/work/nvme/<proj>`、`/work/hdd/<proj>` 下的 `$USER/...` |
| 环境 | 计算节点上的 Python 环境（uv venv 或 apptainer 拉的 NGC PyTorch 容器）还没定 |

在计算节点上构建环境的短作业同样要经 `arb submit` + 你的 `arb approve`。环境定下来后填进 profile 的 `prelude` / `env_activate`。

## 计费规则

`MAX_TRES` 的意思是：按申请的 GPU、内存折算的 GPU、CPU 折算的 GPU 三者取最大值收费。

```text
计费卡数 = max(GPU 数, ⌈内存 / 每卡内存份额⌉, ⌈CPU / 每卡 CPU 份额⌉)
```

例如申请 1 卡加 220 GB 内存，按 2 卡收费。arb 默认按卡数配内存和 CPU，计费卡数就等于申请卡数；用 `--mem` / `--cpus` 多要时，审核单上会出现 ⚠ 和实际计费卡数。

## 花费怎么算

- 审核单上的**估计上限** = 计费卡数 × 时长（小时）× 分区倍率，按各段累加。
- 作业结束后，`arb poll` 从 `sacct` 的 `AllocTRES` 读 `billing=N`：SU = N × 小时数 / `billing_per_su`（DeltaAI 是 1000），已含分区倍率，和 `jobcharge --detail` 对得上。
- `jobcharge` 不接受作业 id（只按时间段汇总），所以不用它。读不到 billing 时按 elapsed × 计费卡数 × 倍率本地估算。
- 预算读 `arboria.toml` 的 `[budget] total_su`。

## 几个仓库共用一台机器

run id 在每个仓库里都从 `r001` 开始，所以作业名和远程目录都带上 `arboria.toml` 的 `[project] id`（`arb init` 生成，例如 `myproj-3f2a`）：

- 作业名 `arb-<project>-<run>-s<k>`，远程目录 `<runs>/<project>/<run>`，ckpt 和 out 同理。
- `arb logs`、`arb remote ls` 和拉回 `ARBORIA_OUT` 只能读当前项目的目录。
- 项目 id 算进审核单哈希；有 run 在集群上时不要改它。旧的 `arboria.toml` 没有项目 id 时，相关命令会拒绝并提示运行 `arb init` 补上。

## 本地模拟

`kind = "fake"` 加 `root = "/某个本地目录"`：FakeSlurm 把“集群”模拟在本地，路径就是本地路径。它用于测试和演练：记下作业、解包快照，但不执行训练命令，时钟也不会自己走。用法见[五分钟上手](/start/quickstart/#5-第一个-run)。
