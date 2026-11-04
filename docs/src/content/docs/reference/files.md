---
title: 数据与文件布局
description: 账本、备份仓库、notebook、命令、拉回的输出、种子园和机器配置分别放在哪里。
---

## 项目里

主 worktree = `git rev-parse --git-common-dir` 的父目录。所有 worktree 共享 `<主 worktree>/.arboria/`，它写进了 `.git/info/exclude`，不属于任何分支。

```text
myproj/                       主 worktree
├── arboria.toml              项目配置（提交它）
├── CLAUDE.md                 其中一个区块由 arb 管理
└── .arboria/
    ├── ledger.db             SQLite 账本（WAL），schema 版本在 PRAGMA user_version
    ├── backup/ledger.sql     备份提交时导出的数据库
    ├── notebooks/<idea>.py   每个 idea 的 notebook
    ├── commands/<名字>.py    项目的 notebook 命令（Claude 写）
    ├── cache/commands/<名字>/ 命令的缓存
    ├── runs/<run>/out/       拉回来的 ARBORIA_OUT
    ├── sessions/             hook 记下的 Claude Code 会话 socket（0600）
    ├── notifications.log     watch / poll 的通知
    └── hook.log              hook 的错误
myproj-<idea>/                每个 idea 的 worktree（分支 idea/<idea>）
└── notebook.py → ../myproj/.arboria/notebooks/<idea>.py
```

- 旧版本的账本原地迁移。
- 快照存成隐藏的 git ref `refs/arboria/runs/<run>`；放弃的 idea 的最终快照在 `refs/arboria/ideas/<name>/final`，打标签 `arboria/archive/<name>`。这些 ref 默认不会被 push。
- 拉回的 `config.yaml` 存进 `runs.config_json`：装了 PyYAML 就解析成 JSON，否则原文存在 `_raw_yaml` 下，最多 200 KB。

## 备份仓库

`.arboria/` 本身是一个只做备份的小 git 仓库。下面这些事件之后，arb 把数据库导出到 `backup/ledger.sql`，连同 `notebooks/*.py` 和 `commands/*.py` 一起提交：

- idea 新建、关闭；笔记；新 run；决定；
- 经验的提出、接受、拒绝、退役；待办的新建、完成、放弃；置信度和指标修改；
- run 的提交、批准、进入队列、拒绝、取消、结束和存入 config。

轮询这类心跳更新只写数据库，不提交。marimo 的 `__marimo__/` 缓存不跟踪。

## 你的机器上

| 位置 | 内容 |
| --- | --- |
| `~/.config/arboria/clusters.toml` | 机器配置（`ARBORIA_CONFIG_DIR`） |
| `~/.config/arboria/commands/` | 全局 notebook 命令 |
| `~/arboria/`（`ARBORIA_HOME`） | 种子园：`seeds/<名字>.py` 是种子 notebook；`.arboria/` 和项目的一样，另有 `.arboria/seeds/`（每次备份提交前把种子 notebook 复制进来） |

种子园不是 git 仓库；账本的 schema 和项目的相同（`seeds`、`plantings` 两张表，待办属于一个 idea 或一个种子）。

## 集群上

| 路径 | 内容 |
| --- | --- |
| `<runs>/<project>/<run>/code` | 快照内容，命令在这里运行 |
| `<runs>/<project>/<run>/logs/s<k>.<jobid>.out` | 每段的日志 |
| `<ckpt>/<project>/<run>` | `ARBORIA_CKPT_DIR` |
| `<out>/<project>/<run>` | `ARBORIA_OUT`，结束后拉回 |
