# ADR 0004：从 marimo 开始（Swift 薄壳 + Python 核心）

状态：已定（2026-11-06）。**取代 ADR 0001、0002**；ADR 0003（probe）暂缓。实现时以本文为准；有冲突先问人。

## 目标与不做的
- Mac app，好看；project → idea → experiment 三层；人的写作是主体，AI 写代码、跑实验、出图，它的文字分开存；算力管理和“人批准才提交”不变；只用 Claude 订阅。
- 不做：自己的编辑器、Rust 核心、本地模型补全、另做的看板 app。

## 架构
```
Arboria.app
 ├─ Swift 薄壳：窗口、菜单/快捷键、Touch ID 签名（Secure Enclave）、钥匙串、通知、pty
 └─ 一个 WKWebView，所有看得见的界面都在里面，同一套 CSS
     ├─ 我们的界面：侧栏树、experiment 页、对比、算力、AI 抽屉、xterm.js 终端
     └─ marimo（iframe，`marimo edit`，127.0.0.1 + token）：idea 的 notebook
arbd：附带的 Python 进程（现在的 arboria 核心），Unix socket 上的 JSON API
arb：CLI，给终端和 Claude 用
```
- 原生控件不上屏（原生的不够好看）。Swift 只做 web 做不到的事；预计 1–2k 行。
- 一套设计系统：沿用 Arboria 设计系统的 token（https://claude.ai/artifact/LeSPASKLSUE39HtVTiiGfk ），写成 `arboria.css`，同时给我们的界面和 marimo 的 `css_file`。marimo 的每一块（cell、输出、markdown 排版、工具栏）都要重新设计；用不到的界面隐藏；marimo 自带的 AI 关掉（不能有第二条不受管的 AI 路径）。
- 字体由 app 用自定义 scheme（`arboria://fonts/…`）直接提供，不再用 data URI。
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
  - 交互：xterm.js + pty 跑 `claude`，cwd 是 idea 的 worktree；一个 idea 一个会话，`--resume` 接上。
  - 后台：`claude -p --output-format stream-json`，每任务一进程，限工具和轮数，用量记账本。
- `arb mcp`：`research_tree`、`get_experiment`、`propose_experiment`（只生成审核单）、`write_ai_note`、`todo_*`。没有任何批准工具。
- context：SessionStart 给**整棵研究树**，由代码从账本确定性生成。每个节点一行：parent、改了什么（change_reason）、结果、结论，标出“你在这里”。结论优先级：人的 `verdict` > AI 的 `ai_oneline` > 只有数字，来源都标出。超预算（先定约 4k token）时，当前 idea 所在路径完整展开，已结束的子树各折成一行（结果 + 经验）。`UserPromptSubmit` 只在有变化时注入一行（“r031 结束：2.29”）。不往会话里推消息。
- 人的 notebook：Claude 能读不能写（deny 规则 + PreToolUse hook 按路径拒绝）。

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
- 批准面板：审核单全文、代码 diff、预估 SU，然后 Touch ID。Secure Enclave 里有一把 `.userPresence` 的 P-256 密钥给审核单哈希签名，arbd 写 Decision 前用存好的公钥验签；Claude 连得上 socket 也伪造不了。终端 `arb approve`（TTY 检查）保留作后备。

## 分期
| 期 | 内容 | 验收 |
|---|---|---|
| P0 验证（约 1 周，要在 Mac 上） | ① WKWebView 里 iframe 嵌 marimo，`css_file` 能改到什么程度 ② iframe 里 marimo 的快捷键、焦点、剪贴板 ③ 自定义 scheme 给字体 ④ xterm.js + pty 跑 `claude`、`--resume` ⑤ Secure Enclave 签名、Python 验签 | 每项一个 spike 和结论；做不到的改计划 |
| P1 外壳 | Swift 薄壳、侧栏树、idea 页（marimo）、Claude 终端、arbd JSON API | 能打开项目、切 idea、编辑 notebook、和 Claude 对话 |
| P2 设计 | `arboria.css` 全套（我们的界面 + marimo 每一块） | 截图评审，人点头才算过 |
| P3 experiment 与算力 | experiment 页、对比、`metrics.jsonl`、甘特图和预算、Touch ID 批准 | FakeSlurm 全流程；然后 DeltaAI 第一个真实 run |
| P4 AI | `arb mcp`、研究树简报、两个 hook、AI 抽屉、后台 `claude -p` | 新会话能说出“做过什么、什么有用” |

## 现有代码
- 保留：db、snapshot、slurm / backends / submission / monitor / cards、machine、lab、commands、inspection、human。
- 改：种子园并进 idea 的 draft；`install-claude` 改成 MCP + hook 配置；新增 `research_tree`。
- 冻结（不再开发，不删）：`crates/`、`tests/vectors/`、`spikes/`、`ui/` 下的 marimo 看板、会话 socket 推送。

## 风险
- marimo 的 DOM 和私有接口（`_cell_rerunner`）随版本变：锁版本、选择器集中、截图回归。
- iframe 里的键盘焦点：P0 验证。
- 研究树太大：靠折叠规则；测 100 个 experiment 时简报的 token 数。
