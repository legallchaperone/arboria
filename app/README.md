# Arboria.app（ADR 0004 的 Swift 薄壳）

打开项目、换 idea、编辑 idea 的 notebook、在 idea 的 worktree 里和 agent（用户自己的 `claude`）对话。布局和样式按 Arboria 设计系统（P2）：标题栏只有面包屑 `项目 › idea ▾`；没打开 idea 时是“待你处理”首页；⌘P 换 idea，⌘J 开关 agent 面板（默认收起），⌘0 回首页。

```
app/build.sh                                   # 只要 Command Line Tools；产出 app/build/Arboria.app
open app/build/Arboria.app --args /path/to/project   # 或不带参数，弹窗选项目
```

- WKWebView：我们的页面 `arboria://app/index.html`（标题栏；首页和 ⌘P 面板时铺满窗口，透明地盖在 notebook 上）、notebook（arbd 起的 `marimo edit`，127.0.0.1 + token）、agent 面板（每个 idea 一个，cwd 是 worktree，第一次打开时才启动）。面板有两个标签：“对话”是 agent 抽屉（下面），“终端”是 xterm.js + pty 跑用户自己的 `claude`，即完整的 Claude Code。
- ⌘P、⌘J、⌘0 由窗口先处理（`AppWindow`），因为 marimo 自己绑了 ⌘P、⌘J，网页又先拿键（P0 ②）。
- 样式：`design/` 是设计系统 `tokens.css`、`bundle.css` 的逐字副本，`design/derive.py` 在构建时生成跟随系统外观的 `tokens.css` 和给 marimo 用的 `--ab-*` 版本。`web/marimo.css` 是 marimo 的全部覆盖（选择器只在这一个文件里，对着 marimo 0.25.2）：它的变量映射到我们的 token、全部用 Maple Mono CN、cell 是 `fill` 块、输出直接在底上、不用的界面（文件名、侧栏、页脚、菜单、AI 入口）隐藏。它也注入进每个 shadow root（marimo 的控件在里面），并让 marimo 的亮暗 class 跟随系统。marimo 自带的 AI 只是在界面上藏起来，要彻底关掉还得看它的配置。
- `arboria://` 只读 app bundle 里的文件（`Scheme.swift`）；侧栏只能导航到 `arboria://app`，notebook 只能留在它的 marimo 服务器上，外部链接交给默认浏览器。
- 核心是 Python 的 `arb arbd`（`src/arboria/arbd.py`），经登录 shell 启动，所以要在终端里能运行 `arb`。
- 起 `claude`、适配器和 `arbd` 前清掉环境里 Claude Code 会话留下的 `CLAUDE*` 变量（`CLAUDE_CONFIG_DIR` 保留；P0 ④）。
- 新 worktree 第一次开终端时，Claude Code 会问是否信任这个文件夹，由人回答。

环境变量（开发用）：
| 变量 | 作用 |
|---|---|
| `ARBORIA_PROJECT` | 项目路径（代替参数 / 选择框） |
| `ARBORIA_ARBD_CMD` | 代替 `arb`，例如本仓库的开发版 |
| `ARBORIA_CLAUDE_CMD` | 代替 `exec claude` |
| `ARB_PROBE=1`、`ARB_PROBE_OUT`、`ARB_PROBE_PROMPT` | 无人值守验证：首页 → 第一个 idea 的 notebook（上、下）→ ⌘P 面板 → agent 面板，每步存整窗截图并打印页面内容，可往 agent 输入一句话，然后退出。窗口被遮住时 WebKit 不画 xterm，agent 截图会是空的 |
| `ARB_PROBE=drawer` | 首页 → notebook → ⌘P → agent 抽屉：等会话建好，打印模式列表，输入 `ARB_PROBE_PROMPT`，用页面上的主按钮（允许一次）回答每个权限请求，截图后切到终端标签再切回 |
| `ARBORIA_ACP_CMD` | 代替 bundle 里的适配器（`exec node …/index.js`），例如假的 ACP agent |
| `ARB_PROBE=pages` | 首页 → 算力页 → 最新的至多 3 个 run 页面，各存一张截图，并打印签字密钥指纹和是否已被信任 |
| `ARB_APPEARANCE=light\|dark` | 强制外观（截两套图用） |

## agent 抽屉（P4）

- ACP（Agent Client Protocol）：`Acp.swift` 起 bundle 里锁定版本的 `@agentclientprotocol/claude-agent-acp`（`acp/package.json` + `package-lock.json`，`build.sh` 用 `npm ci --omit=optional` 装，不装 SDK 自带的 257 MB claude），经登录 shell 用 `node` 运行，`CLAUDE_CODE_EXECUTABLE` 指向登录 shell 里的 `claude`。所以用的是人自己的登录、设置，以及项目里 install-claude 装的 hook、权限规则和 sandbox。要求：登录 shell 里有 `node` 和 `claude`。
- 页面 `web/drawer.html`/`drawer.js`/`drawer.css` 画文字、思考（折叠）、工具调用（状态、改动的 diff、输出）、计划、权限请求、上下文用量；agent 的文字一律 `textContent`，只认 ``` 代码块和 `行内代码`。
- 页面只转发人的输入（发送、停止、换模式、回答权限、重新启动）；能造成什么由 `Acp` 决定：**没有 bypassPermissions**（建会话时 `allowDangerouslySkipPermissions: false`，模式列表里去掉，`set_mode` 拒绝，切到它的权限选项丢掉）；权限回答只对挂着的请求、只能选它自己的选项；不给 agent 文件和终端能力（ACP 的 fs/terminal 都声明为 false，其他请求一律拒绝）。`tests/test_app_drawer.py` 扫源码守住这些。
- 这里的“允许”只是 Claude Code 的工具权限，和 Slurm 的批准无关：提交 run 仍然只有审核单 + Touch ID / 终端。
- 实测（2026-11-07，claude-agent-acp 0.94.0，claude 2.1.318）：会话建立，模式只有 每次问 / 自动改文件 / 只做计划 / agent 判断；让它新建文件并运行 `arb tree`，两次权限请求都在抽屉里出现，点“允许一次”后才执行，回答正确。
- 已知：在 `~/Documents` 下构建时 `codesign` 会报 “resource fork, Finder information…”：那个文件夹的文件同步会给 bundle 根目录加回 Finder 信息。app 照样能运行；挪到别处（如 `/tmp`）签名正常。

## Touch ID 批准（P3）

- 第一次启动时 `Approver` 在 Secure Enclave 里建一把 P-256 签字密钥（`.privateKeyUsage` + `.userPresence`），SE 包裹的句柄存成 `~/Library/Application Support/Arboria/approve-key.blob`（0600，离开这台 Mac 的 SE 没用），公钥写到旁边的 `approve-key.pub`。
- 人在自己的终端里 `arb key trust`（读那个 .pub、显示指纹、要人输入），之后 run 页面的“用 Touch ID 签字”才可用。
- 签字：网页只发 `{cmd: "approve", run}`；Swift 自己向 arbd 取审核单（`card`），对不上就停；系统 Touch ID 弹窗写 run id、预估 SU、哈希前 8 位；签名交给 arbd `approve.signed`，arbd 重算审核单、验签后才记 Decision 并提交。
- 网页能经 Swift 调的 arbd 方法只有只读的 `runs` / `run` / `compute` / `card`（`Shell.readMethods`）。
