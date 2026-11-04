# ADR 0003：插件系统（草案，待评审）

状态：草案。前提见 ADR 0001。

## 问题
现在能自定义的只有：`lab.xxx` 命令（notebook 里、人调用时运行一次）、`clusters.toml` / `arboria.toml` 的参数。用户想要的例子：**在全局页看到 DeltaAI 的剩余额度（access credits）**——它在界面上而不在 notebook 里、要定时刷新、要在集群上跑一条只读命令、属于机器而不属于 idea。`lab.` 命令覆盖不了。算力层我们只提供基本的，其余让用户（通过 agent）自己开发。

## 提议：一个插件 = 一个文件夹 + 一份清单，向固定的扩展点贡献东西
`lab.xxx` 命令就是最小的插件（只贡献一个命令）；`.arboria/commands/x.py` 继续作为简写可用。

扩展点：
| 扩展点 | 做什么 | 例子 | 期 |
|---|---|---|---|
| command | 现在的 `lab.xxx`，在人的 kernel 里运行 | `lab.svd` | 1 |
| machine | 新的算力类型（实现 Backend 接口），或给机器加 probe（只读查询，结果进机器状态） | 实验室自己的调度器；查 credits | 1 |
| panel | 在全局 / 收件箱 / notebook 侧边放一块内容，按设定刷新 | 剩余额度、本周排队 | 2 |
| check | 给审核单加提醒（warning / info），**不能批准、不能拦截以外的任何事** | “用了 fp32”“比上次贵 3 倍” | 之后 |
| event | run 结束 / 失败 / 待签字时：建待办、发通知 | 失败推 Slack | 之后 |

原则：
1. **内置的五种机器（本机、ssh、Slurm、RunPod、Modal）也写成插件**，用同一套接口（dogfooding；接口够不够，自己先知道）。
2. **插件用 Python 写，在单独的进程里运行**（不是人的 kernel），Rust 核心经 stdin/stdout 的 JSON-RPC 和它说话，像 LSP。每个插件进程用项目的 uv 环境（或插件自带的依赖声明）。
3. **界面输出只能是结构化输出**（数字、表格、曲线、markdown、状态），由 app 统一渲染；不渲染 HTML。
4. **权限写在清单里，启用时人看一眼**：例如 `remote_exec = [{machine = "deltaai", argv = ["accounts", "-u", "$USER"]}]`、`schedule = "30m"`、`network = false`、`read_ledger = ["runs", "jobs"]`、`write = ["todo", "notify"]`。核心只放行清单里写的；远程命令经核心的 ssh 通道执行，插件拿不到 ssh 本身。插件永远碰不到批准与提交；最多建待办、发通知、给审核单加提醒。
5. **谁写插件**：人说“全局里显示 DeltaAI 的 credits” → 一条待办（kind=plugin）→ agent 写插件 → 人看清单摘要和改动 → 启用。插件新版本第一次运行要人确认（同命令）。
6. **位置**：全局 `~/.config/arboria/plugins/<name>/`，项目 `.arboria/plugins/<name>/`；以后 `arb plugin add <git url>@<commit>`，按内容哈希固定。

清单草样（`plugin.toml`）：
```toml
name = "deltaai-credits"
version = "0.1.0"
api = 1
entry = "main.py"

[[contributes.probe]]
machine = "deltaai"
id = "credits"
schedule = "30m"

[[contributes.panel]]
id = "credits"
where = "overview.compute"
title = "DeltaAI 余额"
from_probe = "credits"

[permissions]
remote_exec = [{ machine = "deltaai", argv = ["accounts"] }]
network = false
```
`main.py`：`def probe_credits(ctx) -> dict`，用 `ctx.remote("deltaai", ["accounts"])` 拿输出、解析，返回 `{"kind": "number", "label": "剩余 SU", "value": 12345, "of": 50000}`。

## 对开发计划的影响
第 1 期：清单格式、插件宿主进程与 JSON-RPC、command 和 machine 两个扩展点、内置机器按插件接口写。第 2 期（app）：panel。之后：check、event。

## 待评审的问题
1. 这套扩展点切得对不对，缺什么、多什么？
2. 插件跑在单独进程 + JSON-RPC，还是别的（WASM、嵌入 Python、直接 Rust 动态库）？
3. 权限模型（清单声明 + 核心代执行远程命令）够不够；有没有绕过的路？
4. 内置机器做成插件，会不会让核心的关键路径（提交、监控、计费、审核单哈希）变脆？哪些必须留在核心里？
5. 命令和插件的关系：统一还是并存？
6. 插件的版本、依赖、API 兼容怎么管？
