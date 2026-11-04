# 读代码待办（lab.inspect）：给 agent 的约定

人在 notebook 里写 `lab.inspect("问题", view=...)`，想让你讲一段代码。你交付的答案会缓存在
账本里，之后这个 cell 每次重跑都直接显示，不再花 token。所以答案要准、要短、只讲核心。

不管你是在人的会话里、作为子代理，还是被单独启动来做这一个待办，步骤都一样：所有东西
都在待办和下面这几个 `arb inspect` 命令里，在这个仓库的任何一个 worktree 里都能运行。

## 先看待办
`arb todo list --json`：kind 是 inspect，payload 里有 question（人的问题）、view（code /
pseudo / simple / map）、target（idea 或 run id）和 code_ref（代码所在的 commit）。

答案绑定在 code_ref 这个版本的代码上。读代码一律用
`arb inspect source tNNN PATH`（带行号，和 arb 取代码用的是同一个版本），
`arb inspect source tNNN` 列出这个版本里的文件。工作区里的文件可能已经改过，行号会对不上。

## 关键：代码由 arb 来取
你只给位置（path + 起止行号）和说明；arb 自己从 git 里取出那几行显示给人，所以人看到的
代码和仓库里逐字一致。不要把代码抄进说明里。行号要准：start、end 都包含在内，从 1 开始。

## 答案（JSON）
```json
{{
  "title": "DMD 的训练循环（≤80 字）",
  "summary": "两三句话讲清它在做什么（≤600 字）",
  "spans": [
    {{"id": "loop", "path": "train.py", "start": 120, "end": 168,
     "note": "这一段在做什么、为什么（≤600 字）",
     "line_notes": {{"131": "这一行的要点（≤200 字）"}}}}
  ]
}}
```
- 只讲核心：最多 {max_spans} 段、合计不超过 {max_lines} 行、批注合计最多 {max_notes} 条。
  日志、分布式、参数解析这类样板不要放进来。
- 批注写给人看，用人的语言（中文）；说“为什么”，不要逐行复述代码。
- view=code：spans 至少一段。
- view=pseudo：加 "text"（伪代码，≤8000 字）；spans 列出它对应的源码段（人可以展开看原代码）。
- view=simple：加 "text"（去掉日志、分布式、样板之后的简化代码，≤8000 字），可选 "lang"
  （默认 python）；spans 同上。
- view=map：加 "mermaid"（Mermaid 源码，≤8000 字，第一行是图的类型，如 `flowchart TD`），
  可选 "nodes"：{{"节点 id": ["span id", ...]}} 把图里的节点连到源码段。标签里只用纯文字
  （换行用 <br>），不要 `%%{{init}}%%`、`click`、链接或 HTML。
- 不认识的键会被拒绝。

## 先预览，再交付
```bash
arb inspect check tNNN --file - <<'EOF'
{{ ...答案... }}
EOF
```
`check` 只校验并打印人会看到的样子（取出的代码和你的批注），不保存。没问题再把同样的
内容交给 `arb inspect done tNNN --file -`（或 `--file answer.json`）。交付后 notebook 里的
cell 会自动重跑。读代码待办不用 `arb todo done`；做不了就 `arb todo drop tNNN --reason "..."`。

## 不要
- 不改 notebook，不改代码：读代码待办只读。
- 不编造：只讲 code_ref 里真有的东西；不确定就在说明里写“不确定”。
