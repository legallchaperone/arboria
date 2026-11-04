# 跨语言测试向量

Rust 版必须和 Python 版逐字节一致的行为（见 `docs/adr/0001-app-and-rust-core.md`），由 Python 实现导出成 JSON。pytest（`tests/test_vectors.py`）和 cargo test 都读这些文件。

- 重新生成：`uv run python scripts/export_vectors.py`（`--check` 只检查不写）。脚本 import `src/arboria/` 的真实函数逐个跑，不重新实现任何逻辑。
- `tests/test_vectors.py` 在内存里重新生成全部文件，和提交的逐字节比较；改了 Python 行为就重新生成、看 diff、一起提交。
- 文件里没有时间、随机数和本机路径：`--repeat` 的 nonce、快照 commit、项目路径都作为输入给定；`now()` 的格式（`%Y-%m-%dT%H:%M:%SZ`，UTC）不在向量里。

## 格式

每个 `tests/vectors/<name>.json`：

```json
{"name": "<name>", "version": 1, "source": "arboria.<module>.<function>", "notes": "…",
 "cases": [{"id": "…", "input": {…}, "output": …}, {"id": "…", "input": {…}, "error": "ArbError: …"}]}
```

- `input` 是具名参数；`output` 是返回值（tuple 写成数组，dataclass 写成对象，具体在各文件 `notes` 里）；抛异常的写 `error`：`<异常类名>: <消息>`。
- 写法：`sort_keys`、`ensure_ascii=False`、缩进 2、末尾换行。浮点数照 Python 原样输出（`110.898`、`1e-05`、`0.30000000000000004`）；JSON 里带小数点或指数的是 float，否则是 int（Rust 读的时候要保留这个区别）。
- 对象键已排序，原来的顺序丢了；顺序有意义时另给（如 `partition_order`），或者把输入写成原文（TOML、探测输出）。
- id 以 `err-` 开头的必然是 `error`，其余必然是 `output`。以 `quirk-` 开头的是 Python 现在的行为，但大概是 bug（比如正则 `$` 放过结尾的换行）；Python 修好之前 Rust 照做。
- 错误消息里常有 Python 的 `repr()`（`'abc'`、`"it's"`、`None`、`True`），Rust 要照写。

## 文件

审核单与计划
- `canonical_json`：审核单哈希用的 JSON 规范化（键排序、分隔符、浮点、转义）。
- `card_payload`：真实 DeltaAI 计划的审核单内容。
- `card_hash`：同一批计划的审核单哈希（sha256）。
- `build_plan`：计划 → `request_json` 的内容、计费卡数、est_su，和全部拒绝情形。
- `build_command`：`--` 命令 / `[run] command` / `--from` 父 run 加 `--set` 覆盖得出的命令。

Slurm 纯函数
- `parse_walltime`：`90m`、`1d6h`、`HH:MM:SS`、`D-HH:MM:SS` → 秒。
- `slurm_time`：秒 → `--time` 的 `HH:MM:SS`。
- `human_duration`：秒 → `1d6h` 这种短写。
- `segment_plan`：超过分区上限的时长怎么分段。
- `billed_gpus`：DeltaAI MAX_TRES 计费卡数（内存不向上取整的边界）。
- `check_override`：一条 `k=v` 覆盖的校验和值的字符串化。
- `parse_override`：命令行 `--set k=v`。
- `merge_overrides`：父 run 的覆盖加新覆盖。
- `render_overrides`：hydra / argparse / tyro 三种写法。
- `render_sbatch`：完整的 sbatch 脚本（单段、多段每一段、带 prelude）。
- `sbatch_value`：`#SBATCH` 行里 `$USER` → `%u`。
- `dq`：远程路径加双引号。
- `shell_quote`：`shlex.join` / `shlex.quote`（sbatch 里的命令行）。
- `shell_split`：`shlex.split`（`[run] command`）。

名字与远程路径
- `check_project`：项目 id 规则。
- `job_name`：作业名 `arb-<project>-<run>-s<k>`。
- `parse_job_name`：作业名 → (项目, run, 段)。
- `job_project`：作业名所属项目（不是 arb 的作业为 null）。
- `run_dir`：`<runs>/<project>/<run>`。
- `code_dir`：`<run_dir>/code`。
- `script_path`：`<run_dir>/sbatch/s<k>.sh`。
- `log_pattern`：`<run_dir>/logs/s<k>.%j.out`。
- `log_path`：日志路径（代入作业 id）。
- `out_dir`：`ARBORIA_OUT`。
- `ckpt_dir`：`ARBORIA_CKPT_DIR`。
- `wandb_id`：`ARBORIA_WANDB_ID`。
- `restrict_path`：后端允许读的远程路径。
- `git_refs`：`refs/arboria/runs/…`、`refs/arboria/inspect/…`、`idea/<name>` 分支、worktree 目录。

sacct 与 run 状态
- `map_slurm_state`：sacct State → jobs.state。
- `su_from_billing`：AllocTRES `billing=N` → SU（对过 `jobcharge --detail`）。
- `sacct_poll`：`SshSlurm.poll` 发的命令和解析结果。
- `find_job`：`squeue` / `sacct` 按作业名找作业（幂等提交）。
- `check_job_ids`：作业 id 校验。
- `derive_status`：各段状态 → run 状态。
- `billed_su`：jobs.billed_su（sacct 值或本地估计，四位小数）。
- `first_hit`：观察期日志匹配 `fail_patterns`。

配置
- `arboria_toml`：`arboria.toml` → 规范化的项目配置。
- `add_project_id`：`arb init` 往 `arboria.toml` 加 `[project] id`（原字节不变）。
- `derive_project_id`：主 worktree 路径 → 项目 id。
- `validate_project_id`：`arboria.toml` 里项目 id 的校验消息。
- `clusters_parse_profile`：`clusters.toml` → 规范化的机器 profile（全部校验错误）。
- `check_remote_path`：profile 里的远程路径。
- `machine_propose`：`arb machine init` 从探测输出（`tests/fixtures/deltaai_probe.txt`）提出的 profile 和 TOML 文本。
- `parse_accounts`：NCSA `accounts` 输出 → [账户, 余额]（余额 probe 用）。
- `parse_weights`：`TRESBillingWeights` 解析。

id、名字与 LLM 可写字段
- `next_id`：下一个 r/d/t/l id。
- `validate_name`：idea / 种子名。
- `command_name`：notebook 命令名（不含保留名）。
- `validate_text`：LLM 可写字段的长度上限（按码点、先 strip）和 summary / lesson 的额外规则。
- `validate_question`：`lab.inspect` 的问题。
- `check_mermaid`：读代码 map 视图的 Mermaid 校验。
- `inspect_check_path`：读代码答案里的文件路径。
- `inspect_validate_result`：`arb inspect done` 的答案校验和存下的内容（用真的 git tree 取行）。
- `constants`：上限、状态枚举、正则、事件 kind 和触发备份的事件。

## 不在向量里、靠实时对照的

- 快照（临时 index → write-tree → commit-tree）的 tree / commit 哈希：要 git 和真实工作区，由 Rust 集成测试对同一仓库跑两边比较。
- `inspect_validate_result` 用的 tree 哈希只取决于文件内容：Rust 测试用 `git hash-object -w` + `update-index --cacheinfo` + `write-tree` 重建，应得到同一个 `code_ref`。
- 账本（SQLite schema、迁移、`request_json` / `payload_json` 的写法）、备份提交、审核单正文（`cards.build_card`，要读账本）、看板 HTML、hook 输出：由 Rust 版打开 Python 版的账本、两边跑同一组命令比较。
- ssh / scp / rsync 的完整命令行（`SshSlurm` 里除 poll / find_job 之外的）、`arb doctor` 的远程检查：用假的 Runner 两边比较 argv。
- 依赖 Python 库报错原文的情形（无效 TOML、无效正则）没有收进来。
