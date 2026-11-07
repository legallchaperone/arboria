# P0 ⑥：ACP（`claude-agent-acp`）驱动用户的 `claude`

```
npm install --prefix build/npm --omit=optional @agentclientprotocol/claude-agent-acp@0.94.0   # 不提交
python3 client.py build/npm/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js <cwd>
```
`client.py` 是最小的 ACP 客户端（stdio 上逐行 JSON-RPC）：`initialize` → `session/new` → 两次 `session/prompt`，回应 `session/request_permission`，打印抽屉要画的东西。

## 结论（2026-11-06，claude-agent-acp 0.94.0 / agent SDK 0.3.303，claude 2.1.318）
| 项 | 结果 |
|---|---|
| 包 | `@zed-industries/claude-code-acp` 已废弃，改名为 `@agentclientprotocol/claude-agent-acp`。后者更新很勤（当天就有 preview），锁定一个至少两周前的稳定版。 |
| 用用户的 `claude` | ✅ `CLAUDE_CODE_EXECUTABLE=$(which claude)` 时，适配器把它作为 `pathToClaudeCodeExecutable` 交给 agent SDK。默认它会用 SDK 自带的 **257 MB** 二进制（可选依赖 `claude-agent-sdk-darwin-arm64`），用 `--omit=optional` 不装，满足“只驱动用户自己装好的未修改 `claude`”。登录走用户自己的（`authMethods: []`，没有要 token）。 |
| 结构化消息 | ✅ `agent_message_chunk`（文字）、`tool_call` / `tool_call_update`（标题 “Write hello.txt”、kind `edit`、状态 pending → completed）、`usage_update`（已用 / 窗口大小 / 名义花费）、`available_commands_update`、`session_info_update`。`session/prompt` 的结果带 token 用量。这些足够用 `arboria.css` 画一个抽屉。 |
| 权限 | ✅ 写文件前发 `session/request_permission`，选项 allow_once / allow_always / reject_once；选了 allow_once 后才写，文件确实出现。批准权限就在我们的 UI 里，**这和 Slurm 的批准无关**，后者仍只走 Touch ID / 终端。 |
| 模式 | `default`、`acceptEdits`、`plan`、`auto`、`bypassPermissions`；抽屉里不能给 `bypassPermissions`。 |
| 上下文 | 第一轮约 2.6 万 token：没有加载用户在 claude.ai 上的 MCP connector（`mcpServers: []`，对比 ⑦ 默认的 17 万）。 |
| 额度 | 走 agent SDK，和 `claude -p` 一样扣 five_hour / seven_day 窗口（见 ⑦）。 |

## 和 xterm.js 比（④）
- ACP：消息、工具调用、权限都是结构化的，可以按设计系统画，可以把 AI 的文字和人的文字分开存；缺点是多一个 Node 进程和一个更新很快的 npm 依赖（要锁版本），抽屉里看不到 Claude Code 的完整 TUI 和斜杠命令。
- xterm.js：零依赖，用户看到的就是原本的 Claude Code；但只有字节流，结构化信息拿不到。
- 建议：**抽屉用 ACP，终端（xterm.js）一直保留**作为“完整的 Claude Code”。两者可以共享会话（ACP 支持 `loadSession`，终端可 `--resume`），P4 再验证。
