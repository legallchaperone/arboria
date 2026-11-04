# notebook 命令：给 Claude 的约定

人在 marimo notebook 里调用 `lab.<名字>(..., ask="想要什么")`；命令不存在时会生成一个
命令待办（`arb todo list --json` 里 kind 是 command_new / command_revise，payload 里有
name、scope、调用的参数 args/kwargs、说明 description、idea 和要写的文件 path）。
你的活就是写好这个文件。notebook 是人的：读它，绝不修改。

先看 `arb commands list`：内置的（runs、compare、plot、diff、spend…）和已有的命令
能做的，不要再写一个。

## 文件
- 项目范围：{project}/<名字>.py；全局：{global_}/<名字>.py（项目里的同名文件优先）。
  写到待办 payload 的 path；修改（command_revise）就改现有的那个文件。
- 名字 ^[a-z][a-z0-9_]{{0,40}}$，不能和 lab 已有的函数重名。
- 文件里定义同名函数，第一个参数是 ctx。docstring 第一行是描述（命令表里显示它），
  后面几行说明参数；人在 notebook 里输入 `lab.<名字>` 看到的帮助就是签名加 docstring：

```python
def svd(ctx, run, *, k=20, log=False):
    """对 run 的 attention 权重做 SVD，画前 k 个奇异值。

    k：画几个奇异值；log：纵轴用对数。
    """
    r = ctx.run(run)
    ...
    return fig
```

- 函数要能接受第一次调用的形状（payload 里的 args/kwargs）。说明（`ask=`）只是给你
  看的，不会传进来，不要为它留参数；旧 notebook 里说明写成末尾的位置字符串时，lab
  发现签名放不下它就会去掉。`scope=`、`ask=` 是 lab 自己用的，不能当参数名。
- 好的项目命令以后可以 `arb commands promote <名字>` 复制进全局命令库（人决定）。

## ctx
- `ctx.run(run_or_id)`：run 句柄（`lab.Run`）。`.id`、`.status()`、`.row()`、
  `.local_out`（拉回来的 ARBORIA_OUT：.arboria/runs/<run>/out/，可能还不存在）、
  `.config`（解析后的 config，dict 或 None）、`.metrics(keys)`（wandb 曲线，需要 wandb）。
- `ctx.idea`：name、hypothesis、metric。在种子的 notebook 里（`~/arboria/seeds/`）
  `ctx.idea` 是 None，`ctx.seed` 有 name、gist、hypothesis、metric、notebook；种子没有
  run，`ctx.run` / `ctx.runs` 会报错，所以给种子用的命令只能是全局命令、不碰 run。
- `ctx.mo`：marimo 模块（不在 marimo 里时是 None）。
- `ctx.cache_dir`：这个命令自己的缓存目录（.arboria/cache/commands/<名字>/）。
- `ctx.value(x)`：x 是 marimo 控件时取 `.value`，否则原样返回。

## 写法
- 返回值就是 notebook 里显示的东西（图、表、`ctx.mo.md(...)`、`ctx.mo.vstack([...])`）。
  返回 None 只显示出处。lab 会在下面加一行“命令 <名字> · v<版本> · 项目/全局”。
- 有歧义的选择不要先问人：做成带合理默认值的关键字参数（k=20、log=False），在输出里
  写明用了什么、怎么改（例如“k=20，改成 lab.svd(r, k=50)”）；参数也接受 marimo 控件
  （人把 `k = mo.ui.slider(...)` 绑定到变量后传进来，用 `ctx.value(k)` 取值）；几种看法
  都便宜时可以用 `ctx.mo.ui.tabs` 一起给出。只有实在没法继续时，才在 Claude 面板里问人。
- 只读 run 的输出和账本：不改 `.arboria/runs/`、不写 notebook、不碰集群。
- 不联网（wandb 曲线用 `ctx.run(r).metrics(...)`）。
- 只在 `ctx.cache_dir` 下写文件。
- 文件要短，一个命令一件事，开头有 docstring；依赖只用人环境里已有的包。
- 代码在人的 kernel 里、以人的权限运行；人会看 diff 再用，写得让人一眼能看懂。

## 做完
- 存盘后 notebook 里那个 cell 会自动重跑（marimo 正在运行时）。
- 能在终端里试的就试：`python -c 'from arboria import lab; lab.use("<idea>"); print(lab.<名字>("r001"))'`。
- `arb todo done tNNN --result "写了 <名字>：…，默认 k=20"`（≤600 字）。
- `arb commands list` 查看现有命令；`lab.<名字>.revise("...")` 是人要你改的时候用的。
