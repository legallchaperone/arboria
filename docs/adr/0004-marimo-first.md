# ADR 0004：从 marimo 开始（Swift 薄壳 + Python 核心）

状态：已定（2026-11-06；同日按评审修订：marimo 单独 WebView、所见即所签、sandbox、ACP spike、CLI + Skill 先行、按需取研究树、真实 run 提前；同日按 P0 结论修订：样式注入方式、批准密钥存法、抽屉定 ACP、后台参数，见 `spikes/p0/`）。**取代 ADR 0001、0002**；ADR 0003（probe）暂缓。实现时以本文为准；有冲突先问人。

## 目标与不做的
- Mac app，好看；project → idea → experiment 三层；人的写作是主体，AI 写代码、跑实验、出图，它的文字分开存；算力管理和“人批准才提交”不变；只用 Claude 订阅。
- 不做：自己的编辑器、Rust 核心、本地模型补全、另做的看板 app。

## 架构
```
Arboria.app
 ├─ Swift 薄壳：窗口、菜单/快捷键、Touch ID 签名（Secure Enclave）、通知、pty、Edit 菜单（没有它 WebView 里 ⌘C/⌘V 不工作）
 ├─ WKWebView A（`arboria://app`，只加载 app bundle 里签过名的资源）
 │   我们的界面：侧栏树、experiment 页、对比、算力、批准面板、AI 抽屉、xterm.js 终端
 └─ WKWebView B（`marimo edit`，127.0.0.1 + token，顶层页面）：idea 的 notebook
     两个 WebView 由 Swift 并排布局，看起来是一个界面，同一套 CSS
arbd：附带的 Python 进程（现在的 arboria 核心），Unix socket 上的 JSON API
arb：CLI，给终端和 Claude 用
```
- 原生控件不上屏（原生的不够好看）。Swift 只做 web 做不到的事；预计 1–2k 行。
- marimo 不用 iframe，用第二个 WKWebView，原因：
  - 跨站 iframe（`arboria://` 里嵌 `http://127.0.0.1`）在 WebKit 下 cookie / 存储受限（ITP，WKWebView 里 `requestStorageAccess` 不可用），marimo 的 token 可能失效。
  - 改成同源反代能解决 cookie，但命令库是 Claude 写的，命令输出的 HTML/JS 跑在 marimo 里；同源就能操作我们的界面（包括批准面板）。分开的 WebView 天然隔离。
  - 键盘焦点和菜单快捷键冲突由原生的 first responder 处理，比 iframe 简单。
- 一套设计系统：沿用 Arboria 设计系统的 token（https://claude.ai/artifact/LeSPASKLSUE39HtVTiiGfk ），写成 `arboria.css`，给我们的界面，并注入 marimo 的 WebView：`WKUserStyleSheet` 不是公开 API，改用 `WKUserScript`（document start、所有 frame）插 `<link href="arboria://app/marimo.css">`（比 `css_file` 改得彻底，marimo 里没有 shadow DOM，CSS 到得了每一处；`css_file` 留给浏览器里直接打开 notebook 的情况）。marimo 的每一块（cell、输出、markdown 排版、工具栏）都要重新设计；用不到的界面隐藏；marimo 自带的 AI 关掉（不能有第二条不受管的 AI 路径）。
- 字体由 app 用自定义 scheme（`arboria://fonts/…`）直接提供，不再用 data URI。marimo 的页面是 http 源，加载 `arboria://` 字体是跨域，scheme handler 的响应要带 `Access-Control-Allow-Origin`。
- 不 fork marimo。锁定版本，CSS 选择器集中在一个文件，加截图回归。

## 数据模型
| 层 | 是什么 | 谁写 |
|---|---|---|
| Project | git 仓库 + `arboria.toml` 主指标 | 人 |
| Idea | 分支 + worktree + `notebook.py`；状态 draft / open / merged / dropped（种子 = draft） | 人：notebook、假设、指标 |
| Experiment | 一次提交：快照 commit、配置、作业、`metrics.jsonl`、花费 | 代码写记录；人写 `verdict`；AI 写 `ai_oneline` 和 `summary` |

- experiment 新增 `verdict`（人，≤120）、`ai_oneline`（AI，≤120），给研究树用；`summary`（AI，≤600）只在 experiment 页上。
- 训练代码约定加一条：写 `$ARBORIA_OUT/metrics.jsonl`，随结果拉回；曲线不依赖 wandb、不联网。

## notebook：声明就是代码
- 我们不喜欢看代码；留下代码只因为要知道一张图在语义上是怎么来的。
- 所以 cell 里的代码就是声明：一个出图的 cell 只写 `lab.xxx(...)` 调用，例如 `lab.compare(r029, r027, metric="val_loss")`，照常显示。
- 实现放在命令库里，notebook 里不展示实现（`lab.commands()` 只列名字、签名、版本、说明，这是语义的词表）。命令名要表达意思，参数有类型。
- 出图的 cell 不止一个 `lab` 调用时提示一句“写成命令”，只提示不拦。
- 缺命令：照现在的流程写待办 → Claude 写进命令库 → cell 自动重跑。

## AI
- 订阅：只驱动用户自己装好、自己登录的 `claude`，绝不自己取 OAuth token。
  - 交互：xterm.js + pty 跑 `claude`，cwd 是 idea 的 worktree。默认每个任务开新会话（账本才是记忆，长会话会被压缩、变差）；`--resume` 可选。
  - AI 抽屉：ACP（Agent Client Protocol）。适配器 `@agentclientprotocol/claude-agent-acp`（旧名 `claude-code-acp` 已废弃），锁定稳定版，`npm install --omit=optional` 不装它自带的 claude 二进制，`CLAUDE_CODE_EXECUTABLE` 指向用户的 `claude`。拿结构化的消息、工具调用、权限请求、用量，用 `arboria.css` 画；不提供 `bypassPermissions` 模式。终端（xterm.js）始终保留，作为完整的 Claude Code。
  - 启动 `claude`（终端、ACP、后台）前清掉环境里的 `CLAUDECODE` 和 `CLAUDE_CODE_*`：继承了 `CLAUDE_CODE_CHILD_SESSION` 会关掉 transcript，`--resume` 失效。
  - 后台：`claude -p --output-format stream-json`，每任务一进程，限工具和轮数，用量（token 数和 `rate_limit_event` 的窗口利用率）记账本；默认并发 1、每日上限，five_hour 利用率高时暂停。必须带 `--strict-mcp-config --disable-slash-commands --setting-sources ""`，只用 `--mcp-config` / `--allowedTools` 给需要的东西：默认会加载用户全部 MCP / skills（P0 实测回一个 “ok” 用掉 17 万 token，精简后约 3 千）。`--bare` 订阅登录下不能用。
  - 条款：官方允许在产品里托管**未修改的** `claude` 二进制、由用户自己的订阅登录；额度按“普通个人使用”。P0 实测（claude 2.1.318）：`claude -p` 和 Agent SDK 与交互会话扣同一组 five_hour / seven_day 窗口，没有单独额度；`overageStatus: rejected`，不会超额计费。
- 给 Claude 的接口先做 `arb … --json` + 一个 Skill（只在用到时加载说明），不先做 MCP：`arb` 本身已经校验参数，两套接口并存模型会挑着用、校验也要维护两份。确实需要结构化输入的（例如 `propose_experiment`，只生成审核单）再做一层薄的 `arb mcp`，和 CLI 调同一批函数。任何接口都没有批准工具。
- context：研究树由代码从账本确定性生成，按需取。SessionStart 只给索引（约 1–1.5k token）：当前 idea 的路径、兄弟节点、最好的几个结果，标出“你在这里”；其余用 `arb tree`（可指定子树、深度）取。每个节点一行：parent、改了什么（change_reason）、结果、结论。结论优先级：人的 `verdict` > AI 的 `ai_oneline` > 只有数字，来源都标出。`arb tree` 超预算时已结束的子树各折成一行（结果 + 经验）。输出顺序稳定，利于 prompt cache。`UserPromptSubmit` 只在有变化时注入一行（“r031 结束：2.29”）。不往会话里推消息。
- 人的 notebook：Claude 能读不能写。`Edit` deny 规则只管文件工具，管不到 Bash 里的 `sed -i`、`python -c`，PreToolUse 匹配命令字符串也不可靠，所以：
  - 开 Claude Code sandbox，`sandbox.filesystem.denyWrite` 挡住 notebook 路径（系统层面，子进程也生效）；`Edit` deny 规则照旧挡文件工具。
  - 兜底：arbd 对比备份仓库里 notebook 的哈希，不是人改的就提醒。
  - sandbox 不覆盖 MCP server 和 hook；那些是我们自己的代码。

## 人写为主
- 人：notebook 的 markdown cell，加 experiment 的 `verdict`。
- AI：AI 笔记和 experiment 摘要，有长度上限、HTML 转义；显示在 AI 抽屉和 experiment 页，排版不同、标明是 AI；不能在树里建页面，不能往 notebook 写字。AI 想让人看到图，只能写成命令，由人决定放不放。

## 可视化（只回答四个问题）
1. 这次改动有没有用：experiment 与 parent 对比（曲线叠加、配置 diff、`diff --stat`）。
2. 这个 idea 走到哪了：指标对时间 / SU 的散点 + 历史最佳线。
3. 项目全局：研究树（和给 agent 的同一棵），节点标最好指标，放弃的变灰。
4. 在跑什么、花了多少：每台机器一条作业甘特图 + 预算消耗。

## 算力
- 后端、审核单哈希、计费、watch 全部沿用。
- 批准面板：审核单全文、代码 diff、预估 SU，然后 Touch ID。Secure Enclave 里有一把 `.userPresence` 的 P-256 密钥（CryptoKit `SecureEnclave.P256.Signing.PrivateKey`；SE 包裹的句柄存成文件，不进钥匙串，钥匙串要开发者证书的 entitlement）给审核单签名，签的消息带用途、项目、run 和哈希（`arboria-approve-v1\n<project>\n<run>\n<hash>`），arbd 写 Decision 前用存好的公钥验签（Python 核心为此新增依赖 `cryptography`；公钥第一次配对时由人在终端确认指纹）；Claude 连得上 socket 也伪造不了。终端 `arb approve`（TTY 检查）保留作后备。
- 所见即所签：
  - 要签的审核单由 Swift 直接从 arbd 取，不经过网页；网页只能请求“批准 rNNN”。
  - Touch ID 弹窗的 `localizedReason` 是系统画的，写明 run id、预估 SU、审核单哈希前 8 位；网页被攻破也改不了它。
  - WebView A 只加载 app bundle 里签过名的资源，绝不从仓库或可写目录读。

## 分期
| 期 | 内容 | 验收 |
|---|---|---|
| P0 验证（✅ 2026-11-06 完成，结论在 `spikes/p0/`） | ① 第二个 WKWebView 跑 marimo，`WKUserStyleSheet` 能改到什么程度 ② 两个 WebView 间的快捷键、焦点、剪贴板，和菜单快捷键的冲突 ③ 自定义 scheme 给字体，含 http 页面跨域加载（CORS） ④ xterm.js + pty 跑 `claude`、`--resume` ⑤ Secure Enclave 签名、Python 验签、`localizedReason` 显示审核单摘要 ⑥ ACP spike：`claude-code-acp` 驱动抽屉 ⑦ 后台 `claude -p` 的用量从哪里扣 | 每项一个 spike 和结论；做不到的改计划 |
| 并行：第一个真实 run | 不依赖 app，用现有 CLI，在用户配置的任意 Slurm 集群（`clusters.toml` 里的 profile）上 submit → approve → watch → 拉回（1b 第 4 步） | 命令行、日志路径、`sacct` 状态和花费都对上 |
| P1 外壳（✅ 2026-11-06，`app/`、`arb arbd`） | Swift 薄壳、侧栏树、idea 页（marimo）、Claude 终端、arbd JSON API | 能打开项目、切 idea、编辑 notebook、和 Claude 对话 |
| P2 设计 | `arboria.css` 全套（我们的界面 + marimo 每一块） | 截图评审，人点头才算过 |
| P3 experiment 与算力 | experiment 页、对比、`metrics.jsonl`、甘特图和预算、Touch ID 批准 | FakeSlurm 全流程；然后在一个真实集群上用 Touch ID 批准一个 run |
| P4 AI | `arb … --json` + Skill、`arb tree` 与 SessionStart 索引、两个 hook、sandbox 配置、AI 抽屉（ACP）、后台 `claude -p`；需要时再加薄的 `arb mcp` | 新会话能说出“做过什么、什么有用” |

## 现有代码
- 保留：db、snapshot、slurm / backends / submission / monitor / cards、machine、lab、commands、inspection、human。
- 改：种子园并进 idea 的 draft；`install-claude` 改成 Skill + hook + sandbox 配置（以后加 MCP）；新增 `arb tree`。
- 冻结（不再开发，不删）：`crates/`、`tests/vectors/`、`spikes/`、`ui/` 下的 marimo 看板、会话 socket 推送。

## 风险
- marimo 的 DOM 和私有接口（`_cell_rerunner`）随版本变：锁版本、选择器集中、截图回归。
- 两个 WebView 间的键盘焦点、marimo 快捷键和菜单冲突：P0 结论是网页先拿键，marimo 拦下的键（⌘K、⌘S…）菜单收不到；app 全局快捷键避开 marimo 的键，确实要抢的在 window 的 `performKeyEquivalent` 里先截（P1 验证）。
- 命令库的输出是 Claude 写的 HTML/JS：靠 WebView 隔离和所见即所签，不靠审查输出。
- 订阅条款和后台额度会变：只驱动未修改的 `claude`，后台限并发和每日上限。
- 研究树太大：SessionStart 只给索引，`arb tree` 按需取并折叠；测 100 个 experiment 时索引和整树的 token 数。
