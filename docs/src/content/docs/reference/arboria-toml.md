---
title: arboria.toml
description: 项目配置：默认机器、项目 id、预算、wandb、训练命令、观察期和推送。
---

`arboria.toml` 在主 worktree 的根目录，由 `arb init` 生成，应该由你提交到仓库。已存在时 `arb init` 不覆盖它，只在缺 `[project] id` 时补上这一项。

| 键 | 默认 | 作用 |
| --- | --- | --- |
| `default_machine` | `""` | 没有 `--machine` 时用的 `clusters.toml` profile |
| `[project] id` | `arb init` 生成 | 进作业名、远程目录、`ARBORIA_WANDB_ID` 和审核单哈希；有 run 在集群上时不要改 |
| `[project] metric` | `""` | 研究树（`arb tree`）里“结果”取 `metrics.jsonl` 的哪个键；空 = 从每个 idea 的指标文字里找 |
| `[project] goal` | `""` | `"min"` 或 `"max"`；空 = 从指标文字推断（`↓`、`lower`、`越低` → min；`↑`、`higher`、`越高` → max；都没有时 acc / f1 / reward 一类的键算 max，其余 min） |
| `[budget] total_su` | `0` | 项目总预算（SU）；0 = 没设，审核单上会提醒 |
| `[claudemd] autocommit` | `true` | 更新 CLAUDE.md 区块后，是否只提交这一个文件 |
| `[watch] observation_minutes` | `10` | 观察期长度（分钟） |
| `[watch] fail_patterns` | 见下 | 观察期内匹配就取消的正则 |
| `[wandb] entity`、`project` | `""` | 两个都填了，`arb submit` 才记下 wandb 链接；notebook 里读曲线也用它们 |
| `[run] command` | `"python train.py"` | 没有 `-- CMD` 时（以及 `lab.run`）的训练命令 |
| `[run] override_style` | `"hydra"` | `--set k=v` 的拼法：`hydra` → `k=v`，`argparse` / `tyro` → `--k v` |
| `[claude] push` | `true` | 命令待办是否尽力推给正在运行的 Claude Code 会话 |
| `[agent] model` | `""` | 后台 agent（`arb agent run`）用的模型，例如 `"sonnet"`；空 = claude 的默认 |
| `[agent] max_turns` | `30` | 后台 agent 一次最多几轮 |
| `[agent] timeout_minutes` | `30` | 超过就停下 |
| `[agent] daily_limit` | `10` | 24 小时内最多跑几次；`0` = 不跑 |
| `[agent] pause_above` | `0.7` | 订阅的 five_hour 窗口用到这个比例就停下，窗口重置前不再开始 |
| `[review] checklist` | 三条示例 | 提交前审稿的检查清单（还没被读取，留给以后的审稿人） |

## 例子

```toml
default_machine = "deltaai"

[project]
id = "myproj-3f2a"

[budget]
total_su = 400

[wandb]
entity = "my-team"
project = "lm"

[run]
command = "python train.py"
override_style = "hydra"

[watch]
observation_minutes = 10
fail_patterns = ['(?i)\bloss\b[^\n]{0,40}\bnan\b', '(?i)\bnan\b[^\n]{0,40}\bloss\b', 'CUDA out of memory', 'torch\.OutOfMemoryError']
```

## [watch]

- 没有 `[watch]` 表时用上面的默认值；写了 `fail_patterns` 就**完全替换**默认列表。
- 正则按 Python `re` 语法逐行匹配日志末尾 50 行。加载时就检查，写错了会直接报出是第几条。
- 默认只认 loss 为 NaN 和 CUDA OOM，`0 NaN batches skipped` 这样的行不会触发。

## 旧文件

没有 `[wandb]` / `[run]` 的旧文件也能用：只有真正需要时（例如没给 `-- CMD` 又没有 `[run] command`）才报错。没有 `[project] id` 时，`submit`、`approve`、`poll`、`watch`、`logs`、`remote`、`cancel` 会拒绝并提示运行 `arb init` 补上。

## 环境变量

| 变量 | 默认 | 作用 |
| --- | --- | --- |
| `ARBORIA_CONFIG_DIR` | `~/.config/arboria` | `clusters.toml` 和全局命令库的位置 |
| `ARBORIA_HOME` | `~/arboria` | 种子园的位置 |
