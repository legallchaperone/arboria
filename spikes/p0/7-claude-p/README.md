# P0 ⑦：后台 `claude -p` 的用量从哪里扣

claude 2.1.318，订阅登录（`apiKeySource: none`），2026-11-06。

```
claude -p "…" --output-format stream-json --verbose --max-turns 1 --tools "" \
  --strict-mcp-config --disable-slash-commands --setting-sources "" --no-session-persistence
```

## 结论
| 项 | 结果 |
|---|---|
| 扣哪份额度 | stream-json 里有 `rate_limit_event`：`unifiedWindows` 只有 `five_hour` 和 `seven_day`，和交互会话是**同一组**窗口（这次 five_hour 0 → 0.01，seven_day 0.42 → 0.43）。没看到单独的“程序化 / Agent SDK”窗口；二手报道说的“单独月度额度”在这个版本上**没有出现**。`overageStatus: rejected`，不会超额计费。 |
| 记账 | `result` 事件有 `usage`（input / cache_creation / cache_read / output）和 `total_cost_usd`；后者 `costBasis: "list"`，是按 API 标价折算的名义值，不是真扣钱。账本存 token 数和 `rate_limit_event` 的利用率，名义美元只做参考。 |
| **上下文开销** | ⚠️ 默认会加载用户全部 MCP、skills、插件：只回一个 “ok” 写了 **17.3 万** token 的缓存（名义 $1.38）。加上 `--strict-mcp-config --disable-slash-commands --setting-sources ""` 后降到约 **2.9 千**（名义 $0.02）。后台任务必须带这些参数，再用 `--mcp-config` / `--allowedTools` 只给它需要的东西。 |
| `--bare` | ❌ 订阅登录下不能用（“Not logged in”：`--bare` 只认 API key）。 |
| 并发与上限 | 后台和交互共用 five_hour 窗口，计划里的“默认并发 1、每日上限”是必要的；可以在 `rate_limit_event` 利用率超过阈值（比如 five_hour > 0.7）时暂停后台任务。 |
