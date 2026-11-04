---
title: 自定义命令
description: notebook 的中心是命令库：内置的分析加上 Claude 为你写的命令。lab.<名字>(...) 第一次调用时交给 Claude，写好后 cell 自动重跑。
---

原则：**研究你来做，代码交给 Claude。** 你在 notebook 里直接写想要的分析，一行一个：

```python
lab.svd(r012, ask="对 attention 权重做 SVD，画前 20 个奇异值")
```

`ask=` 是写给 Claude 的一句话，只在第一次用得上；命令写好以后它不会传给命令，留在 cell 里也没关系。

## 命令表

```python
lab.commands()
```

一张表列出 `lab` 能做的所有事，一行一个：种类（内置 / 项目 / 全局）、用法、版本、一句说明。

```text
内置  runs(raw=False)                    这个 idea 的 run 一览
      plot(runs, key, *, log=False)      几个 run 的一条 wandb 曲线画在一起
      diff(a, b)                         两个 run 哪里不同：命令、资源、代码、config
      spend(idea=None)                   各 idea 用了多少 SU 和 GPU 时；给 idea 就逐个 run
      …
项目  svd(run, *, k=20, log=False)  v1a2b3c4  对 run 的 attention 权重做 SVD，画前 k 个奇异值。
全局  loss_curve(*runs, smooth=0)   v9f8e7d6  画 loss，可平滑。
```

内置的几条见 [lab API](/guides/notebook/#lab-api)。只写 `lab.svd`（不调用）显示这条命令的帮助：用法、范围、版本和 docstring，都是用 `ast` 从文件里读的，不运行代码。

## 第一次调用

命令还不存在时，arb 写一个命令待办（`tNNN`，记下命令名、范围、这次调用的参数和 `ask=` 的说明），cell 显示：

```text
命令 svd 还不存在，已交给 Claude（t007）。写好后这个 cell 会自动重跑。
```

不会报错。同名命令已经有未完成的待办时直接复用，重复执行 cell 不会堆待办。

旧的写法 `lab.svd(r, "…")`（说明是最后一个位置参数）照样能用：命令写好后，如果它的签名放不下这个多出来的字符串，lab 会去掉它。

## Claude 去写

它在自己的面板里干活，拿不准又没法用默认值解决时会在面板里问你；写好 `<名字>.py` 后 `arb todo done`。约定见 `arb commands guide`：ctx 的用法、返回值、只读、不联网、只写缓存目录、带 docstring。

托管的 CLAUDE.md 区块还告诉 Claude：同一类分析在 notebook 里反复出现、或者待办要的是一次分析时，就写成命令。命令库会随着研究慢慢长起来。

能推的话，arb 会把待办直接发进这个 idea worktree 里正在运行的 Claude Code 会话，见 [Claude Code 集成](/guides/claude-code/#推送待办)。

## 自动重跑

marimo 正在运行时，调用命令的 cell 会盯着命令文件（每秒看一次，文件稳定一秒后才算）；文件出现或改动就自动重跑这个 cell。结果下面有一行出处：

```text
命令 svd · v1a2b3c4d · 项目
```

版本是文件 sha256 的前 8 位。之后每次调用都只是运行存下来的函数：不花 token，结果可复现。返回值在 marimo 里是一个带出处的包装（原始值在 `.value`），在普通 Python 里直接是原始值。

:::note[实现细节]
marimo 公开的响应式只重跑引用了被改动全局变量的 cell，`lab.svd(r)` 这种 cell 碰不到，所以这里用了 marimo 的私有接口（对着 0.25.2 核对过，测试守着）。拿不到时退回成“重新运行这个 cell”的提示。
:::

## 修改、查看、列出

```python
lab.svd                           # 帮助：用法、范围、版本、docstring
lab.svd.revise("改成 log 坐标")   # 修改待办；改好后用到它的 cell 自动重跑
lab.svd.source()                  # 显示文件内容
lab.svd.info()                    # 用法、版本和描述（dict）
lab.commands()                    # 命令表
```

命令行：`arb commands list [--json]`。它**不执行命令代码**，只用 `ast` 读文件（用法、版本、描述、被项目同名命令覆盖的、有问题的）。只有你 kernel 里的 `lab.<名字>(...)` 才会 import 并运行它。

## 两个范围

| 范围 | 位置 | 说明 |
| --- | --- | --- |
| 项目 | `.arboria/commands/<名字>.py` | 只属于这个仓库；随备份提交进 `.arboria/` 的备份仓库，不进快照 |
| 全局 | `~/.config/arboria/commands/<名字>.py` | 所有项目共用；`ARBORIA_CONFIG_DIR` 可改位置 |

同名时项目的优先。默认写进项目；想要全局的，第一次调用时加 `scope="global"`（`scope` 和 `ask` 是 lab 自己用的参数，不会传给命令）。名字只能是小写字母、数字和 `_`，以字母开头，不能和 `lab` 已有的函数重名。

## 命令文件长什么样

一个和文件同名的函数，第一个参数是 `ctx`；docstring 第一行是命令表里的说明，其余的是帮助：

```python
def svd(ctx, run, *, k=20, log=False):
    """对一个 run 的 attention 权重做 SVD，画前 k 个奇异值。

    k：画几个奇异值；log：纵轴用对数。
    """
    r = ctx.run(run)            # run 句柄：.local_out、.config、.metrics(keys)
    ...
```

`ctx` 还有：`ctx.idea`（name、hypothesis、metric）、`ctx.mo`（marimo 模块或 None）、`ctx.cache_dir`（`.arboria/cache/commands/<名字>/`）、`ctx.value(x)`（marimo 控件取 `.value`）。种子 notebook 里 `ctx.idea` 是 None，`ctx.seed` 有种子的字段。

## 命令的安全

命令代码由 Claude 写，却在**你的** notebook kernel 里、以**你的**权限运行。用之前先看 diff：

- Claude 写或改文件时，它的面板里会显示改动；
- 项目命令随下一次备份提交进备份仓库，`git -C .arboria log -p commands/` 能看到每一次改动；
- `lab.<名字>.source()` 显示当前内容。

全局命令不在任何仓库里，想留历史就自己把那个目录放进 git。命令运行期间，`arb approve` / `arb lesson accept` 会拒绝（“不能在 notebook 命令里运行”）。这是护栏，不是沙箱；上集群的硬保证仍然是账本里有你的批准。
