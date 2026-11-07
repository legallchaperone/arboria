# ADR 0004 的 P0 验证

每项一个目录、一份结论。`build/` 不提交。

| # | 项 | 状态 | 结论 |
|---|---|---|---|
| ① | 第二个 WKWebView 跑 marimo + 样式注入 | ✅ | 能跑，token cookie 正常，不需要 ATS 例外。`WKUserStyleSheet` 不是公开 API，改用 `WKUserScript` 插 `<link>`。见 `1-webviews/` |
| ② | 焦点、快捷键、剪贴板 | ✅ | 网页先拿键，marimo 拦的键（⌘K、⌘S…）菜单收不到；剪贴板要求薄壳带 Edit 菜单。见 `1-webviews/` |
| ③ | 自定义 scheme 字体 + CORS | ✅ | http 页面加载 `arboria://fonts/…`，响应带 `Access-Control-Allow-Origin` 即可。见 `1-webviews/` |
| ④ | xterm.js + pty 跑 `claude`、`--resume` | 待做 | 会用订阅额度，等人同意 |
| ⑤ | Secure Enclave 签名 + Python 验签 | ✅ | CryptoKit + 文件里的 SE blob（钥匙串要开发者证书）；Python 要新增依赖 `cryptography`；Touch ID 弹窗文字待人确认。见 `5-enclave/` |
| ⑥ | ACP spike | 待做 | 要装 npm 包（`@agentclientprotocol/claude-agent-acp` 0.101.0 或 `@zed-industries/claude-code-acp` 0.16.2）并用额度，等人同意 |
| ⑦ | 后台 `claude -p` 用量从哪扣 | 待做 | 要用额度，等人同意 |
| 并行 | DeltaAI 上第一个真实 run | 人做 | `arb approve` 只在人的终端里，ssh 对 Claude 是 deny |

## 需要改 ADR 0004 的地方（等人确认）
1. “`WKUserStyleSheet` 注入 marimo”→“`WKUserScript`（document start）插 `<link href="arboria://app/marimo.css">`”。
2. 批准密钥：CryptoKit `SecureEnclave.P256.Signing.PrivateKey`，SE blob 存文件，不用钥匙串。
3. Python 核心新增运行时依赖 `cryptography`。
4. app 全局快捷键要避开 marimo 的键，或在 window 的 `performKeyEquivalent` 里先截。
