---
title: 读代码
description: lab.inspect 请 agent 讲一段代码：代码摘录加批注、伪代码、简化代码或架构图。答案留在 notebook 里，代码由 arb 从 git 取出，和仓库逐字一致。
---

代码交给 agent 写，也交给 agent 讲。你在 notebook 里问一句：

```python
lab.inspect("DMD 的训练循环")                  # 核心代码摘录 + agent 的批注（默认 view="code"）
lab.inspect("DMD 的训练循环", view="pseudo")   # 伪代码
lab.inspect("DMD 的训练循环", view="simple")   # 简化代码：去掉日志、分布式和样板
lab.inspect("整个仓库", view="map")            # 架构图（Mermaid）
lab.inspect("训练循环", run="r003")            # 问某个 run 的代码快照，而不是 idea 现在的代码
```

第一次调用交给 agent，答案到了 cell 自动重跑；之后每次重跑都直接从账本里显示，不再花 token。

## 代码是仓库里的

agent 只给**位置**（文件和起止行号）和说明，不抄代码。交付时 arb 自己从 git 里取出那几行（`git cat-file blob <commit>:<path>`，只读），和答案一起存进账本。所以 notebook 里显示的代码就是仓库里那个版本的代码，一个字都不差；agent 的批注是单独的一行，左边一道蓝线、标着“agent：”，不混进源码。

- `view="code"`：每段一个标题 `train.py:120–168 @ abc1234`，agent 对这段的说明，然后是带行号的真实代码，逐行批注插在对应的行下面。
- `view="pseudo"` / `"simple"`：agent 写的伪代码或简化代码，标明“agent 写的（不是仓库里的代码）”；下面列出它对应的源码段，点开就是 arb 取出的原代码。
- `view="map"`：agent 画的 Mermaid 图，用 marimo 的 `mo.mermaid` 显示。Mermaid 跟着 marimo 一起装好，不联网。图里的节点可以连到源码段，同样点开看原代码。

只讲核心：最多 6 段、合计不超过 400 行。

## 问的是哪份代码

- 默认是 idea **现在**的代码：worktree 原样，包括没提交的改动和新文件。arb 用和 run 快照同样的办法读它（临时 index，不碰你的分支、index 和文件），交给 agent 时记成一个隐藏的 commit，挂在 `refs/arboria/inspect/<待办>` 上。
- `run="r003"`：那个 run 的快照，永远不会变。

答案绑定在那份代码上。之后 worktree 变了，cell 里照样显示原来的答案，多一行灰色的提示：

```text
代码已变（答案基于 abc1234，之后工作区改过），.refresh() 重新生成
```

arb 不会自己重新生成。要新的就 `lab.inspect(...).refresh()`（或 `refresh=True`）：代码没变时什么也不做，变了才再交给 agent，答案到之前先显示上一版。

## 交给 agent

和[命令](/guides/commands/)一样：cell 显示 `已交给 agent（t007）：代码摘录 · DMD 的训练循环。`，待办尽力推给正在运行的 Claude Code 会话；同一个问题（同样的 view、同一个 idea 或 run）已经有未完成的待办时直接复用，重跑 cell 不会堆待办。

agent 那边，不管是你开着的会话、一个子代理，还是单独启动来做这一个待办，步骤都一样：

```bash
arb inspect guide                       # 约定：答案的格式、上限、怎么写批注
arb todo list --json                    # 待办里有问题、view 和 code_ref
arb inspect source t007                 # 这份代码里有哪些文件
arb inspect source t007 train.py        # 带行号的文件（和 arb 取代码用的同一个版本）
arb inspect check t007 --file - < answer.json   # 校验并预览你会看到的样子，不保存
arb inspect done t007 --file - < answer.json    # 交付：取代码、存进账本、关掉待办
```

答案是一段 JSON：

```json
{
  "title": "DMD 的训练循环",
  "summary": "每一步交替更新生成器和假分数网络……",
  "spans": [
    {"id": "loss", "path": "train.py", "start": 14, "end": 21,
     "note": "DMD 的核心：只比较两个分数网络的输出",
     "line_notes": {"19": "两个分数之差就是分布匹配的梯度方向"}}
  ]
}
```

`pseudo` / `simple` 另加 `"text"`（`simple` 可选 `"lang"`），`map` 另加 `"mermaid"` 和可选的 `"nodes"`（节点 id → 源码段 id）。校验失败（路径不存在、行号越界、太长、不认识的键、Mermaid 里有 HTML、`click` 或 `%%{init}%%`）时 `check` 和 `done` 都报错，什么也不存。读代码待办不能用 `arb todo done` 关掉；做不了就 `arb todo drop t007 --reason "..."`，cell 里会显示 agent 没有做和原因，`.refresh()` 再交一次。

## 上限

agent 写的每段文字都有长度上限，显示时一律 HTML 转义：

| 字段 | 上限 |
| --- | --- |
| `title` | 80 字 |
| `summary`、每段 `note` | 600 字 |
| 每条 `line_notes` | 200 字，合计最多 40 条 |
| `text`（伪代码 / 简化代码）、`mermaid` | 8000 字 |
| 源码段 | 最多 6 段，合计 400 行 |

你的问题最多 300 字。种子还没有仓库，种子 notebook 里的 `lab.inspect` 会提示先种下。
