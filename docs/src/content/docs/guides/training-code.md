---
title: 训练代码约定
description: 使用 arboria 的训练代码要遵守的四条：导出配置、wandb id、只读角色变量、可续训的 checkpoint。
---

配置库负责输入，wandb 负责训练过程，arboria 负责决策和账本，三者靠同一个 run id 连起来。arboria 不强制用哪个配置库。托管的 CLAUDE.md 区块里也写着这四条。

## 1. 导出解析后的配置

启动时把完整的、解析后的配置写到 `$ARBORIA_OUT/config.yaml`。`arb poll` 拉回后把它存进账本（`runs.config_json`），notebook 里是 `lab.get("r003").config`。

```python
import os, yaml
out = os.environ["ARBORIA_OUT"]
os.makedirs(out, exist_ok=True)
with open(os.path.join(out, "config.yaml"), "w") as f:
    yaml.safe_dump(cfg, f)
```

## 2. wandb 用 arboria 给的 id

```python
wandb.init(id=os.environ["ARBORIA_WANDB_ID"], group=os.environ["ARBORIA_IDEA"], config=cfg)
```

`ARBORIA_WANDB_ID` = `<项目 id>-<run id>`，几个仓库共用一个 wandb project 也不冲突。`arboria.toml` 的 `[wandb] entity`、`project` 都填了时，`arb submit` 记下 run 的 wandb 链接，notebook 里 `r.metrics([...])` 也用它们。

## 3. 路径只从角色变量读

`ARBORIA_DATA`、`ARBORIA_CACHE`、`ARBORIA_CKPT_DIR`、`ARBORIA_OUT`，不写死路径。机器之间的差异都在机器 profile 里。完整列表见[作业里的环境变量](/guides/submit/#作业里的环境变量)。

## 4. checkpoint 可以续训

checkpoint 存到 `ARBORIA_CKPT_DIR`，启动时有就从最新的续训。超过分区上限的 run 会被拆成几段串起来跑（`ARBORIA_SEGMENT` 是第几段），靠的就是这一条。

## 覆盖参数

`lab.run(set={...})` 和 `arb submit --set k=v` 把参数接在 `[run] command` 后面，拼法由 `[run] override_style` 决定：

| override_style | `--set lr=1e-4` 变成 |
| --- | --- |
| `hydra` | `lr=1e-4` |
| `argparse` / `tyro` | `--lr 1e-4` |

新项目可以考虑 dataclass + tyro：键名或类型写错时立刻报错。已经用 Hydra 或 dict 的项目照旧。
