# ADR 0002：Rust 核心第 1 期契约

状态：**已被 ADR 0004 取代**（2026-11-06，从 marimo 开始、Swift 薄壳、Python 核心；Rust 冻结）。原状态：已定（2026-11-04）。前提见 ADR 0001（app 与 Rust 核心）、ADR 0003（probe）。本期只定公开 API：能编译的 Cargo workspace，大部分函数体是 `todo!("port of arboria.<module>.<fn>")`，之后按模块分给 agent 实现。

## Workspace
- 根 `Cargo.toml`：resolver 2，edition 2021，`rust-toolchain.toml` 固定 1.97.0（带 rustfmt、clippy），`unsafe_code = "forbid"`。`Cargo.lock` 提交。
- 依赖（精确版本，发布都早于 2026-10-21；传递依赖也按这条锁住）：clap 4.6.7（derive）、rusqlite 0.40.2（bundled）、serde 1.0.229、serde_json 1.0.154（float_roundtrip）、toml 1.1.8、thiserror 2.0.21、tracing 0.1.44；anyhow 1.0.104 只给 `arb`。核心不用 tokio，不用 git2/gix/russh（`tests/guards.rs` 守着）。以后要加的：sha2（审核单哈希、probe 哈希）、正则（`fail_patterns`，Python `re` 语法）、YAML（`config.yaml`），同样精确锁版本。
- `crates/arboria-core`（lib）：每个模块一个文件，文件头注释写明移植哪个 Python 模块和要守的约束。
- `crates/arb`（bin `arb-rs`，替换 Python `arb` 之前用这个名字）：clap 树和 Python CLI 一致。

## 模块对照
| Rust 模块 | 移植自 | 备注 |
|---|---|---|
| `paths` | `paths` | |
| `config` | `config` | `arboria.toml` |
| `clusters` | `clusters` | 机器 profile；新增 kind local/ssh/runpod/modal，键还没设计，解析时先拒绝 |
| `canonical` | `slurm.canonical_json`、`json.dumps(sort_keys, ensure_ascii=False)` | **已实现** |
| `slurm` | `slurm` | 纯函数 |
| `cards` | `cards`、`slurm.card_payload/card_hash` | |
| `schema` | `db` 的 SQL 常量 | **已实现**：v1–v7 逐字节照抄，v8 见下 |
| `db` | `db`（连接、迁移、`Ledger`、备份仓库） | **已实现**：open、迁移到 v8、`write`（flock + `BEGIN IMMEDIATE`）、`event`、`next_id`、`data_version`、只读打开；备份仓库未实现 |
| `model` | `db` 的枚举元组 + 账本行 | **已实现**：id newtype 与校验、枚举、事件 kind、触发备份的事件 |
| `records` | `records` + `db.validate_text/validate_name/validate_choice` | **已实现**：长度校验；写入未实现 |
| `gitutil` / `snapshot` | 同名 | 系统 `git`；快照绝不碰用户的分支、index、工作区 |
| `ideas` | `ideas` + `seeds` + `cli._idea_route` | 种子是 stage=seed 的 idea |
| `submission` | `submission` | `SubmitPermit` 只有本模块能造，`Backend::submit` 要它 |
| `human` | `human` | **已实现**：终端检查；输入确认、进程链未实现 |
| `backend` | `backends/`（base、fake、ssh） | `Backend` trait；SshSlurm、本机、ssh 工作站、RunPod、Modal、FakeSlurm 都是桩；`check_job_ids` 已实现 |
| `monitor` | `monitor` | |
| `tasks` | `tasks` + v8 agent 队列 | |
| `commands` | `commands`（文件一侧） | 命令只在人的 kernel 里运行 |
| `inspection` | `inspection`（账本、git、校验一侧） | 渲染归 app |
| `report` / `hook` | 同名 | hook 不再记录会话 socket（ADR 0001） |
| `probe` | 新（ADR 0003） | **已实现**：文件头解析、结果校验、`set_probe_result`、`enabled_hash`；发现、启用、运行、宿主未实现 |
| `lessons` / `claudemd` / `install` / `state` / `machine` / `ui` | 同名 | `ui` 以后由 app 取代 |

不移植：`lab`、`notebooks`、`analysis`、`style`（留在 Python kernel 一侧或被 app 取代）、`sessions`（ADR 0001 删掉推送）。

## schema v8（相对 Python v7，只加不拆）
迁移 SQL 在 `schema::MIGRATION_V8`，每一列的说明在 `schema` 的模块注释里。
- `ideas` 重建：`hypothesis`、`metric`、`branch`、`worktree_path` 可空；加 `stage`（seed / open / merged / dropped）、`gist`、`updated_at`、`legacy_seed_id`；`status` 保留给 Python 读，CHECK 保证 `status = CASE stage WHEN 'seed' THEN 'open' ELSE stage END`。v7 的 `seeds` 每行变成一个 ideas 行（id = 种子 id + 原 ideas 最大 id），`seeds` 表冻结不再写。
- `plantings` 重建：加 `seed_idea_id`（种子的 ideas 行），`seed_id` 变成可空的旧列。
- `tasks` 重建为 agent 队列：`idea_id` 非空；`status` 加 queued / running / check_failed / needs_human，CHECK 改成 `done_at` 非空 ⇔ done / dropped；加 `attempts`、`session_id`、`model`、`usage_json`、`started_at`、`finished_at`、`block_id`；`kind` 加 probe_new / probe_revise；索引 `tasks_queue`。
- `runs` 加列：`block_id`、`nonce`（从 `request_json` 的 `ask.repeat` 回填；审核单哈希仍从 request 读）。
- `decisions` 重建：`subject_type` 加 `probe`；加 `bound_hash`（probe 启用时绑定的文件 sha256，CHECK 要求 enable 必须有）。probe 的启用就是 `subject_type='probe'`、`subject_id='<scope>:<名字>'`、`choice` ∈ enable / disable、`decided_by='human'` 的 Decision；当前启用的哈希 = 最新一条已决定的 probe Decision 是人的 enable 时的 `bound_hash`。
- 新表 `probe_results(machine, probe, at, input_hash, result_json, probe_hash)`：心跳类，不写事件、不触发备份；只有 `probe::set_probe_result` 写。
- 新事件 kind：todo_queue、todo_needs_human、probe_enable、probe_disable（触发备份），todo_attempt（不触发）。

Python 版要读 v8 账本，需要一个小改动：`db.migrate` 接受 8 且只读，看板不再要求恰好 v7。Python 绝不写 v8 账本。

## 测试
- `crates/arboria-core/tests/schema.rs`：v1–v7 与 `tests/fixtures/migrations_v1_v7.sql`（由 `gen_fixtures.py` 从 Python 生成）逐字节一致；v7 的 `sqlite_master` 与 Python 账本一致；Python 生成的 v7 账本迁移到 v8 不丢行；v8 的 CHECK；锁、回滚、`BEGIN IMMEDIATE`、`data_version`；probe 的写入和启用哈希。
- `tests/vectors.rs`：读 `tests/vectors/<name>.json`。已接上 canonical_json、validate_text、validate_name、check_project、wandb_id、next_id、check_job_ids、constants；其余每个文件一个 `#[ignore]` 测试，`every_vector_file_is_known` 保证新文件不会被漏掉。Python 的 `quirk-*`（正则 `$` 放过结尾换行）照做。
- `tests/guards.rs`：源码扫描（只有 submission 调 `.submit(`、`probe_results` 只有一个写入点、probe 碰不到 decisions / submit、核心不联网不异步）。
- `crates/arb/tests/cli_tree.rs`：clap 树与 `python_commands.txt`（`gen_python_commands.py` 从 Python CLI 生成）逐行一致，帮助分组同 `cli.HELP_PANELS`；`arb probe` 是 Rust 独有的，比较时排除。CI 的 `python-snapshots` 任务重新生成这两类快照，过期就失败。

## 接手一个模块的规则
1. 一个 agent 一个模块（加它的测试），两个并行任务不碰同一个文件。改别的模块的公开签名要先在本文件记一笔。
2. 先接向量：把 `tests/vectors.rs` 里对应的 `#[ignore]` 去掉、接到实现上，再写实现；错误消息照 Python 原文（含 `repr()` 的引号）。
3. 只用已有依赖；要新依赖（sha2、正则、YAML）就精确锁一个发布满两周的版本，`Cargo.lock` 一起提交。
4. 账本写入一律走 `Ledger::write`；触发备份的事件只用 `EventKind::BACKUP` 里的；不加任何自动批准的路径。
5. 删掉文件头的 `#![allow(unused_variables)]`；`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全绿。

## 移植时别扭的地方（记下来，接手的人先看）
- 命令的签名和 docstring 靠 Python `ast` 读（`commands.describe`）：Rust 没有现成的 Python 解析器，要么起 `python -I` 跑一个只读脚本（像 probe 宿主），要么用 tree-sitter-python。绝不 import。
- `fail_patterns` 是 Python `re` 语法（`(?i)\b…`）：Rust 的 regex 大体兼容，但回溯、look-around 不支持；只能接受能编译的，不能静默跳过。
- Python 在 `ledger.write()` 里跑 git（`submit` 的快照在锁里做）；Rust 版最好先快照、再进事务。
- 行是 `dict`，`submit` 返回 `run | {"existed": …}`：Rust 用 `#[serde(flatten)]` 的结构体，`--json` 的键顺序会跟 Python 不同（内容相同）。
- v8 里种子的 `status` 是 `open`：Python 的 `WHERE status = 'open'` 在家目录账本里会把种子算成 idea；Rust 一律按 `stage` 查。
- `iterdump` 没有 Rust 对应物：备份的 `ledger.sql` 要自己写导出，并保持格式稳定，否则备份历史的 diff 不好读。
- Python 的 `$` 放过结尾换行（`quirk-*` 向量），名字 / 项目 id / 作业 id 都受影响；现在照做（`model::py_dollar`），修的时候两边一起改。
