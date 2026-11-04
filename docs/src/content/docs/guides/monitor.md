---
title: 监控
description: arb watch、arb poll 和观察期：跟踪作业、记下花费、出问题早取消，以及不用 ssh 看日志和文件。
---

批准之后，作业的状态和花费由 `arb` 跟踪，结果拉回本地。

```bash
arb watch              # 常驻：轮询所有活跃 run，默认每 60 秒一次
arb watch --once       # 只跑一轮
arb poll [RUN]         # 查一次，更新状态和花费；结束后拉回 ARBORIA_OUT
arb logs r003 -n 200   # 看集群上的日志末尾（默认最近开始的那一段）
arb remote ls r003 --out
arb cancel r003        # 取消一个 run 的所有作业
```

## 观察期

出了问题要尽早暴露：开跑后的几分钟，而不是 8 小时之后。

- `arb watch` 在作业开跑后的观察期（默认 10 分钟，`arboria.toml` 的 `[watch] observation_minutes`，或 `--window MIN`）里检查日志末尾 50 行。
- 某一行匹配 `[watch] fail_patterns`，或作业失败，就取消这个 run 剩下的作业并通知。
- 默认只认 loss 为 NaN 和 CUDA OOM；像 `0 NaN batches skipped` 这样的行不会触发。正则怎么写见 [arboria.toml](/reference/arboria-toml/#watch)。

通知写在 `.arboria/notifications.log`，写明匹配的正则和那一行日志；看板的“最近通知”也会显示。

## 需要重新登录时

远程调用发现 ssh 主连接不在了（Duo 过期），`arb watch` 只通知一次，然后暂停对这台机器的远程调用，直到你在终端里 `ssh <host>` 重新认证、再启动 `arb watch`。它不会反复重试去刷 Duo。

## 结束之后

- 状态：`sacct` 给出的最终状态；分段的 run 只有最后一段 `COMPLETED` 才算成功。
- 花费：从 `sacct` 的 `AllocTRES` 读 `billing=N`，算出 SU（见[花费怎么算](/guides/machines/#花费怎么算)）。
- 输出：`ARBORIA_OUT` 拉回 `.arboria/runs/<run>/out/`；其中的 `config.yaml` 存进账本（`lab.get(...).config`）。
- 已结束、你还没写过笔记的 run 会出现在看板的“待你处理”里，`arb status` 和 hook 里多一行“看板：arb ui”。

## 不用 ssh

`arb logs RUN [--segment K] [-n 100]` 和 `arb remote ls RUN [SUBPATH] [--out]` 代替 ssh。它们只能读当前项目的 `<runs>/<project>`、`<out>/<project>`。Claude 也应该用它们，`arb install-claude` 把 `ssh`、`scp`、`rsync` 设成了 deny。
