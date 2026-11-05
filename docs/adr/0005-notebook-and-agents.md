# ADR 0005：第 3 期，我们自己的 notebook 和派活

状态：已定（2026-11-05）。前提见 ADR 0001、0004（方案全文 https://claude.ai/artifact/AbJrewmFfKDfNtoPotL2zm 第 3 期，编辑器原型 https://claude.ai/artifact/KNTafPRmEDfoQJZBQMwcWn）。实现时以本文为准；有冲突先问人。

第 3 期交付：文字块加命令块的 notebook、kernel、结构化输出、`claude -p` 队列和 agent 面板、marimo 迁移脚本。用户得到：一行命令出图，缺命令自动派活，marimo 下线。

## 代码在哪
| 部分 | 位置 | 说明 |
|---|---|---|
| 文件格式 | `crates/arboria-core/src/nbdoc.rs` | 解析 / 生成 notebook 的 markdown，块 id，新 notebook 的模板 |
| 输出缓存 | `crates/arboria-core/src/nbout.rs` | `.arboria/outputs/<notebook>/<块>.json` |
| kernel 驱动 | `crates/arboria-core/src/nbkernel.rs` | 起 Python kernel、发请求、收事件、中断、重启 |
| kernel | `src/arboria/kernel.py`、`src/arboria/output.py` | 在人的 Python 里执行块，把结果转成结构化输出 |
| 派活 | `crates/arboria-core/src/agent.rs`，`arb agent run/list/log/queue/stop` | `claude -p` 跑 agent 队列里的待办 |
| 迁移 | `src/arboria/migrate.py`，`arb idea migrate` | marimo `.py` → `.md` |
| 编辑器 | `app/src/notebook/` | TipTap 3（文字）+ CodeMirror 6（命令块）+ uPlot（图），照原型 |
| agent 面板 | `app/src/views/AgentView.tsx` | 队列、每个任务的模型、轮数、token、日志 |

## 文件格式
idea 的 notebook 是 `.arboria/notebooks/<idea>.md`（worktree 里链接成 `notebook.md`，`info/exclude` 排除，备份仓库跟踪）；种子的是 `<home>/seeds/<名字>.md`。

```
---
arboria: 1
idea: warmup-long
---
前 2k 步的尖峰可能和 warmup 太短有关，先对比两条曲线。

```arb {#k3f9}
/plot r001 r002 val/loss
```
```

- 开头的 frontmatter 只有 `arboria`（格式版本，现在是 1）、`idea` 或 `seed`。别的键原样保留。
- 文字用受限的 markdown 子集（第 0 期试验 2 验证过：标题、段落、列表、粗斜体、行内代码、链接）。子集之外的写法降级成文本。
- 命令块是 ` ```arb {#id} `，id 是 4 位小写字母和数字，在 notebook 里唯一。没有 id 或 id 重复的块，打开时补一个新 id（只改那一行）。普通代码块原样保留，不执行。
- 结果不写进文件。

## 命令块
三种写法都行：
- 斜杠命令：`/plot r001 r002 val/loss`。kernel 用 `shlex` 切开，`k=v` 是关键字参数，值能按 Python 字面量读（数字、`True`、`None`、带引号的字符串）就按字面量，否则当字符串，然后调 `lab.<名字>(...)`。一行一个命令。
- `lab.xxx(...)`。
- 几行 Python。超过两三行的分析交给 agent 写进命令库。

kernel 的命名空间里预先有 `lab`（`from arboria import lab`）。块里的代码 `__file__` 是 notebook 的路径，`lab` 由此认出 idea（和 marimo 一样）。

## 结构化输出
一个块的结果是一串输出，每个是带 `kind` 的 JSON。界面按 kind 画，从不渲染 HTML（`app/src/lib/rules.test.ts` 守住）：

| kind | 字段 | 来自 |
|---|---|---|
| `text` | text、truncated | print、repr |
| `markdown` | text | `_repr_markdown_`；界面用自己的小渲染器（行内代码、粗斜体、链接、列表、表格），不出 HTML |
| `table` | columns、rows、total | DataFrame、list[dict]、`lab.runs()` |
| `plot` | title、x、y、log、style（line / bar）、series [{name, x[], y[]}] | `lab.plot`、返回 `_arboria_` 的命令 |
| `image` | mime（png / jpeg）、data（base64） | matplotlib 的 Figure、PIL |
| `error` | ename、evalue、traceback | 异常 |
| `pending` | task、what（command / inspect）、name、ask | 命令还没有、交给 agent 了 |
| `run` | run、status、change、est_su、card | `lab.run(...)` 生成的审核单；界面给“去签字”，签字仍然走审核单面板（ADR 0004） |
| `inspect` | 读代码的答案（校验过的字段） | `lab.inspect` |

- 任何对象可以定义 `_arboria_()`，返回上面某个 kind 的 dict（或它们的 list）；`arboria.output` 校验字段和大小。
- 大小：文本 200 KB、表格 2000 行、图每条线 20 万点、图片 8 MB，超了截断并写 `truncated`。
- 每块的结果存 `.arboria/outputs/<notebook 名>/<块 id>.json`：代码的哈希、运行时刻、耗时、输出、`defines` / `uses`（`ast` 读出的顶层名字）。打开 notebook 时显示缓存的结果，代码变了显示“代码改过”，账本比结果新显示“截至 …”。

## 依赖和重跑
- 一个块重跑后，下面 `uses` 和它的 `defines` 有交集、而且比它早跑的块标“过期”，不自动跑。
- 输出里有 `pending`（命令交给了 agent）的块：那个待办变成 done 后，app 自动重跑这个块一次（只在 notebook 开着、kernel 活着时）。

## kernel
- `python -m arboria.kernel`，解释器是 `commands::python_exe()`（`ARBORIA_PYTHON`，否则 `arb-py` 的解释器），工作目录是 idea 的 worktree（种子：种子园）。
- 协议是 JSON 行：请求从 stdin 进，回复从 stdout 出。kernel 启动时把原来的 stdout 留给协议，fd 1 改指 stderr，用户代码和子进程随手写的东西进不了协议通道；Python 层的 `sys.stdout` / `sys.stderr` 变成流式的 `stream` 事件。
- 请求：`{"id", "op": "exec", "block", "code", "file"}`，`{"id", "op": "complete", "code", "pos"}`。事件：`ready`、`stream`、`output`、`done`（ok、defines、uses、duration_ms）、`error`。
- 中断：给 kernel 进程发 SIGINT（只在执行时变成 `KeyboardInterrupt`）。重启：杀掉再起。一个 notebook 一个 kernel，app 关时都停。
- 选它而不是 Jupyter 协议：第 0 期试验 3 发现纯 Rust 的 zeromq 收超过约 1.5 MB 的消息会卡死；管道没有这个问题，也不用新依赖。kernel 能做的和 marimo 一样（人的 Python，人的权限）。

## 派活（agent 队列）
账本里的队列是 v8 就有的（`tasks.status`：open → queued → running → done / check_failed / needs_human，`claim_next`、`finish_attempt`）。第 3 期加上跑它的东西：

- `arb agent run [--once]`：取队列里最早的一个，按种类写提示，跑 `claude -p`：
  - 命令 `$ARBORIA_CLAUDE`，否则 PATH 上的 `claude`（人自己登录的那个，只用订阅）；
  - `--output-format stream-json --verbose`、`--model`（默认 `sonnet`）、`--max-turns`（默认 20）、`--append-system-prompt`（任务说明：`arb commands guide` / `arb inspect guide` 的约定、仓库简介、这个待办）、`--allowedTools`（Read、Grep、Glob、Edit、Write、`Bash(arb commands *)`、`Bash(arb inspect *)`、`Bash(arb todo *)`、`Bash(git diff*)` 等）、`--disallowedTools`（`Bash(arb approve*)`、notebook 和 `.arboria/` 的 Edit/Write）；
  - 在 idea 的 worktree 里跑（种子：种子园），日志（stream-json 原样）存 `.arboria/agent/<待办>-<第几次>.jsonl`。
- 跑完读最后的 `result` 事件：session id、轮数、token（含缓存读）、花费，记进 `tasks.usage_json`。agent 自己用 `arb todo done` / `arb inspect done` / 写好命令文件关掉待办；没关掉就是 `check_failed`，在同一个会话里（`--resume`）再修一次；还不行 `needs_human`。
- 自动派活：命令待办（command_new / command_revise）和读代码待办一出现就进队列，普通待办由人在 app 里点“交给 agent”。设置（`<config>/agent.toml`）：`auto`（默认开）、`model`、`max_turns`、`daily_limit`（每天最多跑几次，默认 30）、`claude`（命令路径）。
- 同一个项目同时只跑一个（`.arboria/agent.lock` 文件锁）。app 开着时后台每 10 秒看一次队列；关了就由 `arb agent run` 跑。
- 边界不变：`claude -p` 里的工具进程带着 `CLAUDECODE`，`arb approve` / `arb note` / 种子的写入照样被拒绝；agent 不改 notebook；命令更新后第一次运行仍然是人点的。

## agent 面板
每个待办一行：种类、名字、状态、模型、第几次、轮数、token（输入 / 输出 / 缓存读）、花费、耗时；展开看日志（stream-json 里的文字和工具调用，文本渲染）。按钮：交给 agent（open → queued）、停止（杀掉进程，记 check_failed）、重试（needs_human → queued）。

## 迁移（marimo 下线）
- `arb idea migrate [名字…] [--all]`（只由人；种子园里迁种子）：`.py` 用 `ast` 读（不执行）。`mo.md(...)` 的 cell 变成文字，其他 cell 变成命令块（去掉 marimo 的 `def _(…):` 外壳和最后的 `return`），只 import marimo / arboria 的 cell 去掉。原来的 `.py` 留着。
- `arb idea new`、种子、种下从现在起写 `.md`；种下把种子的文字（不含命令块）放进“来自种子”。
- app 打开 idea 时：有 `.md` 用自己的 notebook；只有 `.py` 时显示“迁移到新 notebook”，迁移前仍可用 marimo 打开。
- Python 版（`arb-py`）继续写 marimo，`cli_parity.rs` 不再比 notebook 文件。

## 不变的约束
- 没有任何自动批准的路径：kernel、`agent.rs`、`nbkernel.rs` 不碰 `approve`、`Confirmed`、`decisions`（`tests/guards.rs` 扫描）。
- agent 写的东西只经已有的校验（命令文件、`arb inspect done`、`arb todo done`），长度上限不变。
- 界面不渲染 HTML，样式只用设计 token。
