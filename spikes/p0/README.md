# ADR 0004 的 P0 验证

每项一个目录、一份结论。`build/` 不提交。

| # | 项 | 状态 | 结论 |
|---|---|---|---|
| ① | 第二个 WKWebView 跑 marimo + 样式注入 | ✅ | 能跑，token cookie 正常，不需要 ATS 例外。`WKUserStyleSheet` 不是公开 API，改用 `WKUserScript` 插 `<link>`。见 `1-webviews/` |
| ② | 焦点、快捷键、剪贴板 | ✅ | 网页先拿键，marimo 拦的键（⌘K、⌘S…）菜单收不到；剪贴板要求薄壳带 Edit 菜单。见 `1-webviews/` |
| ③ | 自定义 scheme 字体 + CORS | ✅ | http 页面加载 `arboria://fonts/…`，响应带 `Access-Control-Allow-Origin` 即可。见 `1-webviews/` |
| ④ | xterm.js + pty 跑 `claude`、`--resume` | ✅ | `openpty` + `posix_spawn`，TUI 完整渲染，`--continue` 能续上。要清掉父进程的 `CLAUDE_CODE_*`（否则 transcript 不保存）。见 `4-pty-claude/` |
| ⑤ | Secure Enclave 签名 + Python 验签 | ✅ | CryptoKit + 文件里的 SE blob（钥匙串要开发者证书）；Python 要新增依赖 `cryptography`；Touch ID 弹窗文字待人确认。见 `5-enclave/` |
| ⑥ | ACP spike | ✅ | `@agentclientprotocol/claude-agent-acp@0.94.0` + `CLAUDE_CODE_EXECUTABLE` 用用户的 `claude`（`--omit=optional` 不装自带的 257 MB 二进制）；消息、工具调用、权限请求都是结构化的。建议抽屉用 ACP、终端保留。见 `6-acp/` |
| ⑦ | 后台 `claude -p` 用量从哪扣 | ✅ | 和交互共用 five_hour / seven_day 窗口，没看到单独额度。默认加载全部 MCP/skills（回个 ok 用了 17 万 token），后台必须带 `--strict-mcp-config --disable-slash-commands --setting-sources ""`（约 3 千）；`--bare` 订阅下不能用。见 `7-claude-p/` |
| 并行 | DeltaAI 上第一个真实 run | 人做 | `arb approve` 只在人的终端里，ssh 对 Claude 是 deny |

## 已并入 ADR 0004 的修改（2026-11-06）
1. “`WKUserStyleSheet` 注入 marimo”→“`WKUserScript`（document start）插 `<link href="arboria://app/marimo.css">`”。
2. 批准密钥：CryptoKit `SecureEnclave.P256.Signing.PrivateKey`，SE blob 存文件，不用钥匙串。
3. Python 核心新增运行时依赖 `cryptography`。
4. app 全局快捷键要避开 marimo 的键，或在 window 的 `performKeyEquivalent` 里先截。
5. AI 抽屉定为 ACP（`claude-agent-acp`，锁版本，`--omit=optional`），xterm.js 终端保留。
6. 后台 `claude -p` 和交互共用订阅窗口：后台任务要带精简参数，并在 `rate_limit_event` 利用率高时暂停。
7. 启动 `claude`（终端、ACP、后台）时清掉环境里的 `CLAUDECODE` / `CLAUDE_CODE_*`。
