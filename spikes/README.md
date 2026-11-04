# 第 0 期的三个小试验

一次性的验证代码，不是产品代码。结论写进方案：https://claude.ai/artifact/AbJrewmFfKDfNtoPotL2zm

## 1. 本地模型懂不懂中文意图（`intent_bench.py`，在 Mac 上跑）

```bash
uv run spikes/intent_bench.py
```

用 MLX 跑 `mlx-community/Qwen3.5-2B-4bit` 和 `Qwen3.5-4B-4bit`：50 句中文意图（其中 6 句不该给建议），模型只输出 `{"command", "args"}`，按命令签名校验。表里的列：JSON 能解析、合法（命令和参数都在签名里）、命令对、参数对、该空则空、误建议、p50 / p95 延迟。结果另存在 `intent_bench_results.json`。

## 2. notebook 存成 markdown 会不会丢东西（`editor/`）

```bash
cd spikes/editor && npm ci && N=20000 node roundtrip.mjs
```

ProseMirror 文档 ↔ 受限的 markdown 子集（标题、段落、列表、粗斜体、行内代码、链接、` ```arb {#id} ` 命令块、普通代码块），fast-check 随机生成文档。

结果（每项 20000 份）：
- 编辑器里写出来的文档：存成 markdown 再读回，结构完全一样；再存一次，文本一字不变；命令块的原文和 id 一字不差。
- 为此修了库的 6 处：prosemirror-markdown 不转义行尾的 `1.`、`+`、`-`；换行（hard break）之后的 `---` 会变成标题下划线；标题末尾的 `#` 会被当成结束符；相邻的两个列表会合并（改成交替用 `*` / `-`、`.` / `)`）；段落首尾空格存不下（保存时去掉）；外部写的换行要保留（软换行当 hard break）。
- 外部编辑器里随手写的 markdown：子集之外的写法降级成文本（表格保留成多行、引用和分隔线加反斜杠、图片成文字），`####` 以下标题和普通代码块原样保留。少数乱写的边角情况（空链接文字、未闭合的代码块后跟空行）打开再存要两次才稳定：打开外部改过的文件时，如果整理后的文本和原文不同，提示一次“已整理成标准格式”。
- 公式先当文本保存（`$…$` 原样），渲染时再交给 KaTeX。

## 3. app 能不能稳稳驱动 Python kernel（`kernel/`）

```bash
cd spikes/kernel && uv venv .venv && uv pip install --python .venv/bin/python "ipykernel<7.1" jupyter_client
cargo run --release            # Rust：jupyter-zmq-client 1.0.1 + jupyter-protocol 2.0.2（纯 Rust 的 zeromq 0.6）
.venv/bin/python pyclient.py   # 对照：官方 Python 客户端（libzmq）
```

结果（Linux 容器里，ipykernel 7.0.1）：
- 启动到能回 kernel_info 约 0.7 秒；`1 + 1` 往返约 10 ms；`lab.runs()` 的结构化输出（`application/vnd.arboria.table+json`）约 7 ms；变量留在 kernel 里；报错拿到 `ZeroDivisionError`；0.8 秒时发 `interrupt_request`，0.82 秒就回到 idle，拿到 `KeyboardInterrupt`，之后状态还在。
- **问题：单条消息超过约 1.5 MB（`print('x' * 1_600_000)`），Rust 客户端卡死**，连外层的超时都不触发；同一个 kernel 用官方 Python 客户端，50 MB 只要 0.69 秒。问题在纯 Rust 的 zeromq 0.6。
- 因为连接时 kernel 可能还没监听端口，要先等端口能连上再连（zeromq-rs 的 connect 不重试）。

对设计的影响：
1. 结构化输出不走 iopub 传大块数据：大的图表数据、表格写进 `.arboria/outputs/`，iopub 只传引用和摘要。这本来就和“输出按块缓存在 `.arboria/` 里”的设计一致。
2. 用户代码随手 `print` 出大文本也不能把 app 卡死：kernel 通道改用 libzmq（`zmq` crate，C 库，和 ipykernel 用的是同一个），或者等 zeromq 0.7 发布满两周后复测；同时在 app 里截断超大的文本输出。
3. `jupyter-zmq-client` 最新是 3.0.1（10-28 发布，还不满两周），1.x 和 3.x 的接口相近，复测成本低。
