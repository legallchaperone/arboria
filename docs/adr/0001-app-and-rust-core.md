# ADR 0001：Mac app 与 Rust 核心

状态：**已被 ADR 0004 取代**（2026-11-06，从 marimo 开始、Swift 薄壳、Python 核心；Rust 冻结）。原状态：已定（2026-11-04）。方案全文：https://claude.ai/artifact/AbJrewmFfKDfNtoPotL2zm 。实现时以本文为准；有冲突先问人。

## 产品
- 定位：**Research, fast and slow.** 快的（改代码、跑实验）交给 agent，慢的（看数据、想 idea）留给人。
- 形态：Mac app（Tauri 2，React + TypeScript 前端，Rust 核心）。只支持 macOS（Apple silicon）；核心和 CLI 也要能在 Linux 上编译和测试（CI、集群登录节点）。
- notebook 是核心：文字块 + 命令块（`/cmd args`、`lab.x(...)`、一行 Python）。文件是 markdown + ` ```arb {#id} ` fence（见 `spikes/editor/`）；输出不进文件，缓存在 `.arboria/outputs/`。
- 设计：Arboria 设计系统 https://claude.ai/artifact/LeSPASKLSUE39HtVTiiGfk （全部 Maple Mono CN；sage + indigo；主按钮墨色）。主界面：专注（只有 notebook）、收件箱（一列按类分组）、全局（时间线），见 https://claude.ai/artifact/DGQD3R9dMC8s8ErYq5Rqd6 。

## 进程与数据
- `arboria-core` 是**库**，被 app、CLI `arb`、LaunchAgent helper 共同链接；没有守护进程。核心库同步（不依赖 tokio）；异步只在 app / helper。
- SQLite 账本是唯一仲裁点：rusqlite（bundled）、WAL、`busy_timeout`、写入用 `BEGIN IMMEDIATE`。变更通知用 `PRAGMA data_version`。备份 git 仓库的提交加文件锁、幂等。
- schema v8（现在 Python 版是 v7）：**只加不拆**，Python 版过渡期能只读打开。seed 并进 ideas 的 `stage`（seed / open / merged / dropped）；tasks 扩成 agent 队列（open / queued / running / check_failed / needs_human / done / dropped，加 attempts、session_id、model、usage_json、started_at、finished_at、block_id）；runs 加 block_id、nonce。具体列在第 1 期的契约 PR 里定。
- git 和 ssh **调用系统命令**（`git`、`ssh`、`scp`），不用 git2/gix/russh：快照仍是临时 index → write-tree → commit-tree，绝不碰用户的分支、index、工作区；ssh 靠用户的 ControlMaster/ControlPersist，`ssh -O check` 做门禁，BatchMode，NeedLogin 不重试。

## 必须字节级兼容 Python 版的
审核单哈希（JSON 规范化，浮点格式如 `110.898`）、sbatch 渲染、分段、计费、作业名、远程路径、run/task id 格式、事件 kind 名。由 Python 版导出**跨语言测试向量**（`tests/vectors/*.json`），pytest 和 cargo test 同时消费；向量先于 Rust 实现。

## 机器（算力层）
- 第一版：本机、ssh 工作站、Slurm、RunPod、Modal。统一接口：准备环境、送代码、提交、查状态、日志、取消、拉回结果、花费。
- 差异写进每台机器的 profile，不写进代码；`arb machine init` 只在登录节点查，其余由第一个真实作业顺便测，之前用保守默认值（假设计算节点不能上网等）。
- 环境只认项目的 `uv.lock`。云平台 key 存钥匙串，agent 拿不到。

## agent（派活）
- 三档：确定性补全（账本 + 命令签名）→ 本地模型 ghost code（MLX + Qwen3.5-4B 4-bit，复用系统提示 KV 缓存，按 ⌘↩ 才出，见 `spikes/intent_bench.py`）→ 后台 `claude -p`（每任务一进程，替换系统提示 + 按 commit 缓存的仓库简介，限工具 / 轮数 / 预算，静态检查不过 `--resume` 一次，再升级模型一次，再交人；用量记进 tasks）。交互式 Claude Code 只在人聊天时用。删掉会话 socket 推送。
- 只用订阅：通过用户自己装的、自己登录的 `claude`，绝不自己取 OAuth token。

## 不变的约束（照搬 CLAUDE.md）
只有账本里人的 Decision 能让核心提交作业；入口是终端 `arb approve` 和 app 的批准面板（Touch ID）。agent 不改 notebook；命令只在人的 kernel 里运行；LLM 可写字段有长度上限并转义；看板只读。护栏，不是沙箱。

## 已知的坑（第 0 期试验）
- 纯 Rust zeromq 0.6 单条消息 > 约 1.5 MB 会卡死：大输出写文件、iopub 只传引用；kernel 通道改用 libzmq（`zmq` crate）或复测新版。
- prosemirror-markdown 有 6 处转义问题（见 `spikes/README.md`）。

## 流程
- 每期先出契约 PR（公开类型、CLI JSON 输出、测试向量）；之后按模块分给 agent，两个并行任务不碰同一个文件，同时最多 5 个。
- Rust：`cargo fmt`、`cargo clippy -D warnings`、`cargo test` 全绿；Python：`uv run pytest` 全绿。
- Python 版（`src/arboria/`）在 Rust 版追平前继续可用，不删。
