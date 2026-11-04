---
title: clusters.toml
description: 机器 profile 的每一项：类型、账户、计费份额、分区、路径和环境。
---

`~/.config/arboria/clusters.toml`（或 `$ARBORIA_CONFIG_DIR/clusters.toml`），一张顶层表一台机器。`arb machine init NAME --write` 会在集群上实测后写入；只有用到的那台 profile 会被校验。

```toml
[deltaai]
kind = "slurm"
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

## 顶层键

| 键 | 必填 | 说明 |
| --- | --- | --- |
| `kind` | 否，默认 `"slurm"` | `"slurm"`（真集群，经 ssh）或 `"fake"`（本地模拟） |
| `host` | slurm 必填 | `~/.ssh/config` 里的别名（字母、数字、`._-`） |
| `root` | fake 必填 | FakeSlurm 的本地状态目录；只能用于 `kind = "fake"` |
| `arch` | 否 | 例如 `aarch64`，显示在审核单上 |
| `account` | 否 | Slurm 账户，一个词；`<…>` 占位表示还没填 |
| `gpus_per_node` | 是 | 每节点 GPU 数，≥ 1 |
| `cpus_per_gpu` | 是 | 一张卡对应的 CPU 份额（计费用），≥ 1 |
| `mem_per_gpu_gb` | 是 | 一张卡对应的内存份额（GB，计费用），> 0 |
| `default_partition` | 是 | 必须是下面 `partitions` 里的一个 |
| `max_segments` | 否，默认 `4` | 一个 run 最多拆成几段 |
| `balance_cmd` | 否 | 查余额的命令，例如 `accounts` |
| `billing_per_su` | 否，默认 `0` | `sacct` 的 billing 单位里多少算 1 SU·小时；0 = 本地估算 |
| `prelude` | 否 | shell 行的列表，写进 sbatch 脚本（例如 `module load`） |
| `env_activate` | 否 | 激活作业环境的 shell 行 |

旧的 `charge_cmd` 已经去掉，写了会报错并提示改用 `billing_per_su`。

## [NAME.partitions.PART]

| 键 | 说明 |
| --- | --- |
| `max_walltime` | 分区时长上限，例如 `48h`、`2h`；超过就分段 |
| `charge` | 计费倍率，默认 `1.0`，> 0 |

至少要有一个分区。

## [NAME.paths]

五个都必填：`runs`、`data`、`cache`、`ckpt`、`out`。

- 必须是绝对路径，只能用字母、数字、`._-/` 和 `$USER`（`#SBATCH` 行里会换成 `%u`）；不能有 `.`、`..` 段，不能有 `<…>` 占位。
- 远程目录都按项目分开：`<runs>/<project>/<run>`、`<ckpt>/<project>/<run>`、`<out>/<project>/<run>`。
- `kind = "fake"` 时它们就是本地路径。
