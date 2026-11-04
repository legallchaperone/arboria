---
title: 五分钟上手
description: 从一个种子开始，种进仓库成为 idea，打开它的 notebook，再用本地模拟的机器生成第一个审核单。
---

这一页走一遍完整的路径：种子 → idea → notebook → 第一个 run。前四步只用本机；第五步要一台**机器配置**，没有集群时可以先用本地模拟的 FakeSlurm 看审核单长什么样。

:::note[谁来运行]
带 <span class="arb-tag">只由你</span> 的命令（写种子、种下、批准、笔记）在 Claude Code 里会被拒绝，请在你自己的终端里运行。
:::

## 1. 记一个种子（可选）

还没想好放进哪个仓库的想法，先记成种子。任何目录下都可以：

```bash
arb seed new loss-spikes "大 batch 里的 loss 尖峰是不是 Adam 二阶矩滞后"
marimo edit ~/arboria/seeds/loss-spikes.py
```

第一次运行会建好种子园 `~/arboria/`。在 notebook 里写观察、猜想、玩具实验；`arb seed list` 列出所有种子。详见[种子园](/guides/seeds/)。

## 2. 准备实验仓库

```bash
cd ~/code/myproj          # 你的实验代码仓库（git，主 worktree）
arb init                  # .arboria/ 账本、备份仓库、arboria.toml 模板、CLAUDE.md 区块
$EDITOR arboria.toml      # default_machine、[wandb] entity/project、[run] command
git add arboria.toml && git commit -m "arboria.toml"
arb install-claude        # SessionStart hook、权限规则（可以晚点再做）
```

`arb init` 是幂等的，最后会打印下一步。`arboria.toml` 的每一项见[配置参考](/reference/arboria-toml/)。

## 3. 种进仓库，成为 idea

```bash
arb seed plant loss-spikes --hypothesis "把 β2 从 0.999 降到 0.95，大部分尖峰会消失" \
                           --metric "前 1B tokens 内的尖峰次数"
```

种子里已经有假设和指标时可以不写这两项。没有种子也一样，直接建 idea：

```bash
arb idea new lr-warmup --hypothesis "warmup 能稳定早期训练" --metric "val/loss @10k steps"
```

两条命令都会建分支 `idea/NAME`、同级的 worktree 文件夹 `myproj-NAME` 和这个 idea 的 notebook，并输出 worktree 路径。

## 4. 打开 notebook

```bash
cd ../myproj-loss-spikes
marimo edit notebook.py   # 或在 VS Code 里用 marimo 扩展打开
```

notebook 开头是 idea 的假设和指标（种下的还带着折叠的“种子里的思路”），然后是 `from arboria import lab` 和 `lab.runs()`。这个 notebook 只有你写；要 Claude 改代码，就在里面留一个待办：

```python
lab.todo("把 eval 改成每 500 步一次")
```

在这个 worktree 里打开 Claude Code，hook 会把待办带进会话。

## 5. 第一个 run

`lab.run` 和 `arb submit` 都要知道往哪台机器交，所以需要 `~/.config/arboria/clusters.toml` 里的一个 profile。

- **有集群**：在登录节点上实测生成，见[集群与机器配置](/guides/machines/)。
- **还没有**：用 `kind = "fake"` 的本地模拟机器先看看。把下面这段放进 `~/.config/arboria/clusters.toml`，路径换成你自己的绝对路径，再在 `arboria.toml` 里设 `default_machine = "local"`：

```toml
[local]
kind = "fake"                         # FakeSlurm：在本地模拟一个集群
root = "/home/you/arb-fake/state"
arch = "x86_64"
account = "demo"
gpus_per_node = 4
cpus_per_gpu = 16
mem_per_gpu_gb = 64
default_partition = "gpu"
[local.partitions.gpu]
max_walltime = "48h"
charge = 1.0
[local.paths]
runs  = "/home/you/arb-fake/runs"
data  = "/home/you/arb-fake/data"
cache = "/home/you/arb-fake/cache"
ckpt  = "/home/you/arb-fake/ckpt"
out   = "/home/you/arb-fake/out"
```

然后在 notebook 里：

```python
r = lab.run(reason="baseline", gpus=1, time="2h")   # 只生成审核单
r                                                    # r001 · 待批准，下面一行是 arb approve r001
```

或者在 worktree 里用命令行 `arb submit --reason "baseline" --gpus 1 --time 2h`。审核单大致是这样（FakeSlurm 和预算都会被标出来）：

```text
审核单 r001（idea loss-spikes）
检查:
  ✓ 快照 8a7dbf0f3c2a（不含被 .gitignore 忽略的文件）
  ✓ 账户 demo
  ✓ 计费卡数 = 申请卡数 = 1
  ⚠ 机器 local 是 FakeSlurm（本地模拟，不是真集群）
  ⚠ 未设项目预算（arboria.toml [budget] total_su = 0）
资源:
  机器 local（x86_64），分区 gpu，1 GPU
  每节点 CPU 16，内存 64G
  时长 2h
  命令 python train.py
  …
估计:
  上限 1 卡 × 2.00 h × 1 = 2.00 SU（按申请的时长算满）
审核单哈希: e6ce3ddf46218ff2
由你批准：arb approve r001
```

最后在**你自己的终端**里：

```bash
arb card r001       # 随时重看审核单
arb approve r001    # 显示审核单，输入 r001 确认后提交
arb poll            # 查一次状态；arb watch 持续跟踪
```

:::caution[FakeSlurm 只是模拟]
FakeSlurm 是测试用的：它把快照解包到 `runs` 目录、记下作业，但**不执行训练命令**，模拟时钟也不会自己走，所以作业会一直停在“运行中”。用它看完审核单、批准和状态之后，`arb cancel r001` 结束它。真正跑起来需要一台 `kind = "slurm"` 的机器。
:::

## 接下来

- [核心概念](/start/concepts/)：种子、idea、run、审核单、待办、经验和账本怎么连在一起。
- [notebook 与 lab API](/guides/notebook/)：比较 run、读曲线、分叉、剪枝。
- [提交与批准](/guides/submit/)：`--from`、`--set`、分段和计费。
