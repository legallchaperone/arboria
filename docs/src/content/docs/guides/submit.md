---
title: 提交与批准
description: arb submit 只在本地生成审核单，arb approve 只在你的终端里提交；--from 和 --set、审核单哈希、计费、分段和幂等。
---

上集群的 run 分三步，第二步只能由你来做：

```bash
# 1. Claude、你或 notebook 里的 lab.run，在 idea worktree 里：只在本地生成快照和审核单
arb submit --reason "warmup 1k steps" --gpus 4 --time 60h -- python train.py --lr 3e-4
arb submit --from r003 --set lr=1e-4 --reason "lr 小一点"   # 同一快照，只改参数

# 2. 你在自己的终端里看过审核单后批准
arb card r003          # 随时重看审核单
arb approve r003       # 显示审核单，输入 r003 确认；或 arb reject r003 --reason "..."

# 3. 跟踪
arb watch              # 或 arb poll / arb logs r003 / arb remote ls r003
```

## 批准只在你自己的终端里

`arb approve` 和 `arb lesson accept` 在下面任一情况下直接拒绝：

- 环境里有 `CLAUDECODE` 或 `CLAUDE_CODE_ENTRYPOINT`（Claude Code 给它的工具设置的变量）；
- stdin 不是终端；
- 正在运行 notebook 命令。

拒绝时提示“批准只能在你自己的终端里做：请在终端运行 arb approve rNNN”。通过之后显示审核单，要你输入 run id 确认。批准 Decision 里记下 author = human、进程号和最多 6 层父进程（尽力而为）。

## 在 Mac app 里用 Touch ID 签字

Arboria 的 Mac app 里，run 页面会显示等你签字的审核单和“用 Touch ID 签字”。这条路和 `arb approve` 记下同一个人的批准 Decision，只是确认方式不同：

1. app 第一次启动时在 Secure Enclave 里生成一把签字密钥（每次签名都要 Touch ID 或密码），并显示它的指纹。
2. 在你自己的终端里信任它：`arb key trust`。终端显示指纹，你核对它和 app 里的一致，再输入这串指纹。`arb key list` 列出、`arb key revoke 指纹` 撤销。
3. 签字时 app 从 arbd 重新取审核单，系统的 Touch ID 弹窗写着 run id、预估 SU 和审核单哈希前 8 位（网页改不了这段文字）。签的是 `arboria-approve-v1`、项目 id、run id 和审核单哈希这一行消息；arbd 重算审核单、用信任的公钥验签，对得上才记下批准并提交。重放同一个签名不会再提交。

Decision 里记下 `confirmed_by: Touch ID`、密钥指纹和签名。`arb key …` 和 `arb approve` 一样只在你自己的终端里运行，`arb install-claude` 把它们和密钥文件（`approve-keys.json`、`~/Library/Application Support/Arboria/`）都设为 Claude 不能用、不能编辑。剩下的风险：在 P4 的 sandbox 之前，以你的身份运行的任何进程（包括 Claude Code 的 Bash）都能改这个文件，deny 规则只挡 Claude 的文件工具和 `arb key` 命令。

这是护栏，不是沙箱；硬保证是：**只有批准路径（`arb approve` 或验过签的 Touch ID）会调用后端的提交**，而且提交前代码会再查一次账本里有你的批准 Decision。没有任何自动批准的代码路径。

## 审核单

分为 检查 / 资源 / 估计 / 预算 四块，最后是审核单哈希。哈希覆盖快照 commit、命令、资源（含账户和计费卡数）、机器、分区、分段计划、环境（prelude、env_activate、路径）以及项目 id。

`arb approve` 按存下的请求和**当前**机器配置重算一遍，对不上就拒绝：“审核单已变化，请重新 arb submit”。

## --from 和 --set

- `--from rNNN` 复用那个 run 的快照（不再做新快照）。GPU、时长、节点、分区、内存、CPU 默认取它的；命令取它的命令或当前的 `[run] command`，再加上它的 override，用新的 `--set` 覆盖同名的。审核单上写“复用 rNNN 的快照”。
- `--set k=v` 可以写多次，按 `arboria.toml` 的 `[run] override_style` 拼到命令后面：`hydra` → `k=v`，`argparse` / `tyro` → `--k v`。
- 没有 `-- CMD` 时用 `[run] command`。命令按参数逐个加引号，不经过 shell 展开；多节点时请在命令里自己写 `srun`。

## 相同的提交只算一次

审核单哈希相同、而且没被拒绝/取消/失败的 run 已经存在时，`arb submit` 直接返回它，打印“已存在：rNNN”。想重复跑同一个实验就改点什么（例如 `--set seed=2`），或加 `--repeat`：请求里带一个随机 nonce，它也算进哈希，批准只对这一个 run 有效，审核单上写“⚠ 重复运行”。

## 计费

按 DeltaAI 的规则：

```text
计费卡数 = max(GPU 数, ⌈内存 / 每卡内存份额⌉, ⌈CPU / 每卡 CPU 份额⌉)
估计上限 = 计费卡数 × 时长（小时）× 分区倍率，按各段累加
```

默认按卡数配内存和 CPU，所以计费卡数 = 申请卡数；用 `--mem` / `--cpus` 多要时，审核单上会出现 ⚠ 和实际计费卡数。作业结束后的实际花费见[集群与机器配置](/guides/machines/#花费怎么算)。

## 分段

时长超过分区上限（例如 `ghx4` 48h）就拆成 ⌈T/M⌉ 段，用 `--dependency=afterany:<上一段>` 串起来；训练代码必须能从 `ARBORIA_CKPT_DIR` 续训。段数超过 `max_segments` 拒绝。某段失败时其余段会被取消；只有最后一段 `COMPLETED` 才算成功。

## 幂等和续交

每段 `sbatch` 之前先在账本里记一行 `submitting`，并先按完整作业名 `arb-<project>-<run>-s<k>` 在集群上查找（只认自己的、名字完全相同的作业）；已存在就直接采用，不重复提交。中途失败（包括需要重新登录）时，再运行一次 `arb approve` 续交。

## Duo 和 ssh

`SshSlurm` 每次远程调用前先运行 `ssh -O check <host>`；没有活着的主连接就报“请先在终端运行 `ssh <host>` 完成 Duo 认证，然后重试”，绝不自动重试。远程命令都带 `-o BatchMode=yes`，不会弹出密码或推送 Duo。

## 作业里的环境变量

| 变量 | 值 |
| --- | --- |
| `ARBORIA_RUN_ID` | run id，例如 `r003` |
| `ARBORIA_WANDB_ID` | `<项目 id>-<run id>`，几个仓库共用一个 wandb project 也不会撞 |
| `ARBORIA_IDEA` | idea 名 |
| `ARBORIA_SEGMENT` | 第几段 |
| `ARBORIA_DATA`、`ARBORIA_CACHE` | 数据和缓存目录（`HF_HOME=$ARBORIA_CACHE/hf`） |
| `ARBORIA_CKPT_DIR` | `<ckpt>/<project>/<run>` |
| `ARBORIA_OUT` | `<out>/<project>/<run>`，结束后拉回本地 |

命令在 `<runs>/<project>/<run>/code`（快照内容）里运行，日志在 `<runs>/<project>/<run>/logs/s<k>.<jobid>.out`。训练代码要遵守的四条见[训练代码约定](/guides/training-code/)。
