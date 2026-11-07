---
title: 路线图与里程碑
description: 你定的里程碑：截止日期、算力预算、包含的 idea 和清单；花了多少 SU 从账本算，审核单上会写这个 run 算在哪个里程碑里。
---

里程碑是你定的：截止日期、算力预算、要做的 idea，加上几条自己打勾的清单。Claude 能读，不能改。

```sh
arb milestone new baseline --title "复现 baseline 并和论文数字对上" \
    --deadline 2026-12-01 --su 300 --idea exp1 --idea longer-ctx \
    --item "eval 脚本" --item "和论文表 2 对上"
arb milestone list                 # 未完成的，按截止日期排；--all 连已完成/已放弃的
arb milestone check baseline 1     # 打勾第 1 条（uncheck 取消）
arb milestone set baseline --su 400 --add-idea warmup --add-item "写进周报"
arb milestone done baseline        # 也有 reopen、drop
```

`set` 还有 `--title`、`--deadline` / `--no-deadline`、`--no-su`、`--remove-idea`、`--remove-item 编号`。名字用小写字母、数字和 `-`；标题 ≤80 字，清单每条 ≤120 字，最多 30 条、20 个 idea。

## 和算力挂钩

- **已用**：里程碑里各 idea 的 run 实际计费的 SU（`arb poll` 写进账本的），不是估计。
- **等签字**：这些 idea 里等你批准的 run 的预估 SU。
- **审核单**：run 所在的 idea 属于某个未完成的里程碑时，审核单的“预算”里多一行 `里程碑 baseline：已用 120.00 / 300 SU，剩余 180.00，截止 2026-12-01（还有 12 天）`；预估上限超过剩余时，“检查”里多一条 ⚠。只是提醒，不会拦着你批准。
- 一个 idea 可以在几个里程碑里，它的花费在每个里都算。

## 在哪里看

- `arb milestone list` / `show NAME`（`--json` 给 agent 用）。
- Mac app 的“路线图”页：时间轴（今天到最后一个截止日期）、每个里程碑的预算条、idea 和清单。逾期、7 天内截止、超预算、等签字的超过剩余时，首页“待你处理”里会有一条。

## 只由你改

`new`、`set`、`check`、`uncheck`、`done`、`reopen`、`drop` 在 Claude Code 里直接拒绝（代码里检查），`arb install-claude` 也把它们设为 deny；`list`、`show` 谁都能用。里程碑不是单独的表：每次改动是账本里的一条 `milestone` 事件，路线图是把它们按顺序重放出来的，所以和其他账本写入一样加锁，每次改动都会触发备份提交。
