---
title: CLI 命令参考
description: arb 的每个命令、参数和默认值，从真实的命令行生成。
---

<!-- 生成的文件，不要手改：uv run python docs/scripts/gen_cli.py -->

这一页由 [`docs/scripts/gen_cli.py`](https://github.com/legallchaperone/arboria/blob/main/docs/scripts/gen_cli.py) 从 `arb` 的 Typer 定义生成，和 `arb … --help` 一致（说明文字是命令行里的英文原文）。标着 <span class="arb-tag">只由你</span> 的命令在 Claude Code 里会被拒绝。

用法行的写法和 `--help` 相同：`{name}` 是必填参数，`[name]` 可以省略，`...` 表示可以有多个；`<str>`、`<int>` 是取值类型。

改了命令行之后重新生成（CI 会检查这一页是不是最新的）：

```bash
uv run python docs/scripts/gen_cli.py          # 或在 docs/ 里 npm run cli-ref
```

## 一览

| 命令 | 作用 |
| --- | --- |
| [`arb init`](#arb-init) | Set up arboria for this repository (idempotent). |
| [`arb idea new`](#arb-idea-new) | Create branch idea/NAME and its worktree folder; prints the worktree path. |
| [`arb idea list`](#arb-idea-list) | List all ideas. |
| [`arb idea notebook`](#arb-idea-notebook) | Create the notebook (and notebook.py link) of an idea that has none. |
| [`arb idea close`](#arb-idea-close) | Close an idea with --merge or --drop; removes its worktree and branch. |
| [`arb run new`](#arb-run-new) | Snapshot the current idea worktree as a draft run; prints the run id. |
| [`arb run summary`](#arb-run-summary) | Set a run's result summary (run must belong to the current idea). |
| [`arb note`](#arb-note) | (Human only) add a note to the current idea (--confidence also updates the idea). |
| [`arb status`](#arb-status) | Current idea (inside an idea worktree), an overview of open ideas, or (in the seed home) the seeds. |
| [`arb export`](#arb-export) | Markdown of all ideas, notes and runs (newest first). |
| [`arb seed new`](#arb-seed-new) | (Human only) new seed and its notebook &lt;home&gt;/seeds/NAME.py (prints its path). |
| [`arb seed list`](#arb-seed-list) | Seeds, most recently edited first (dropped ones only with --all). |
| [`arb seed show`](#arb-seed-show) | One seed: gist, hypothesis, metric, status, notebook, where it was planted. |
| [`arb seed set`](#arb-seed-set) | (Human only) change a seed's gist, hypothesis or metric. |
| [`arb seed drop`](#arb-seed-drop) | (Human only) give up on a seed; its notebook is kept. |
| [`arb seed revive`](#arb-seed-revive) | (Human only) bring a dropped seed back to thinking. |
| [`arb seed plant`](#arb-seed-plant) | (Human only) plant a seed into a repo as a new idea, whose notebook links back. |
| [`arb submit`](#arb-submit) | Snapshot the idea worktree (or reuse --from's) and ask the human to approve a cluster run (local only). An identical earlier run is returned instead of a new one, unless --repeat. |
| [`arb card`](#arb-card) | Show a submitted run's approval card, recomputed now (local only). |
| [`arb approve`](#arb-approve) | (Human only, in your own terminal) approve a run's card and submit it to the cluster; you type the run id to confirm. Re-run to resume. |
| [`arb reject`](#arb-reject) | Reject a run awaiting approval (nothing is sent to the cluster). |
| [`arb cancel`](#arb-cancel) | Cancel a submitted run's jobs on the cluster. |
| [`arb poll`](#arb-poll) | One pass: update job states, run status and cost; fetch ARBORIA\_OUT when done. |
| [`arb watch`](#arb-watch) | Keep polling active runs; in the observation window cancel on crash or a log line matching arboria.toml [watch] fail\_patterns (default: loss NaN, CUDA OOM). |
| [`arb logs`](#arb-logs) | Tail a run's Slurm log on the cluster (use this instead of ssh). |
| [`arb remote ls`](#arb-remote-ls) | List a run's directory (or its ARBORIA\_OUT) on the cluster. |
| [`arb todo list`](#arb-todo-list) | Todos of the current idea (in an idea worktree), of every open idea, or (in the seed home) of the seeds. |
| [`arb todo done`](#arb-todo-done) | Mark a todo done with a short result for the human. |
| [`arb todo drop`](#arb-todo-drop) | Drop a todo that will not be done. |
| [`arb commands list`](#arb-commands-list) | Commands of this project and the global library (project wins on a clash). |
| [`arb commands guide`](#arb-commands-guide) | The conventions for writing a notebook command (for Claude). |
| [`arb ui`](#arb-ui) | Open the read-only dashboard (看板): what needs you, the idea tree, SU budget; or, outside any repo (or with --home), the seed home (种子园). |
| [`arb machine init`](#arb-machine-init) | Measure a Slurm machine over your ssh master (read-only) and print its profile. |
| [`arb doctor`](#arb-doctor) | Read-only checks of a machine profile, the ssh setup and the cluster side. |
| [`arb install-claude`](#arb-install-claude) | Add the SessionStart hook and permission rules to .claude/settings.json (the machine-specific global commands dir goes into .claude/settings.local.json). |
| [`arb hook session-start`](#arb-hook-session-start) | SessionStart hook: reads hook JSON on stdin, prints additionalContext JSON. |
| [`arb claudemd sync`](#arb-claudemd-sync) | Write the arboria block (rules + accepted lessons) into the main worktree's CLAUDE.md. |
| [`arb lesson propose`](#arb-lesson-propose) | Propose a lesson; it only goes into CLAUDE.md after the human accepts it. |
| [`arb lesson list`](#arb-lesson-list) | List lessons (default: proposed + accepted), newest first. |
| [`arb lesson accept`](#arb-lesson-accept) | (Human only, in your own terminal) accept a proposed lesson and update CLAUDE.md; you type the lesson id to confirm. |
| [`arb lesson reject`](#arb-lesson-reject) | Reject a proposed lesson. |
| [`arb lesson retire`](#arb-lesson-retire) | Retire an accepted lesson and remove it from CLAUDE.md. |

## 项目与 idea

在实验代码仓库里：初始化、idea 的生命周期、本地 run、笔记和概览。

### arb init

Set up arboria for this repository (idempotent).

```text
arb init [OPTIONS]
```

### arb idea new

Create branch idea/NAME and its worktree folder; prints the worktree path.

```text
arb idea new [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Slug: lowercase letters, digits, '-'. | 必填 |
| `--hypothesis <str>` | What you expect and why (≤300 chars). | 必填 |
| `--metric <str>` | How success is measured (≤200 chars). | 必填 |
| `--parent <str>` | Branch off this open idea. |  |

### arb idea list

List all ideas.

```text
arb idea list [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--json` | Print machine-readable JSON. |  |

### arb idea notebook

Create the notebook (and notebook.py link) of an idea that has none.

```text
arb idea notebook [OPTIONS] [name]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Defaults to the idea of the current directory. |  |

### arb idea close

Close an idea with --merge or --drop; removes its worktree and branch.

```text
arb idea close [OPTIONS] [name]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Defaults to the idea of the current directory. |  |
| `--merge` | Merge idea/NAME into main's branch. |  |
| `--drop` | Archive a final snapshot and delete the branch. |  |
| `--confidence <str>` | guess \| prelim \| confirmed \| rejected |  |
| `--allow-large` | Snapshot large untracked files anyway. |  |
| `--json` | Print machine-readable JSON. |  |

### arb run new

Snapshot the current idea worktree as a draft run; prints the run id.

```text
arb run new [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--reason <str>` | What changed vs. the previous run and why (≤300 chars). | 必填 |
| `--allow-large` | Snapshot large untracked files anyway. |  |
| `--json` | Print machine-readable JSON. |  |

### arb run summary

Set a run's result summary (run must belong to the current idea).

```text
arb run summary [OPTIONS] {run_id} {text}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `run_id` | Run id, e.g. r001. | 必填 |
| `text` | Result summary (≤10 lines, ≤1500 chars). | 必填 |

### arb note

<span class="arb-tag">只由你</span>

(Human only) add a note to the current idea (--confidence also updates the idea).

```text
arb note [OPTIONS] {text}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `text` | The note (≤1000 chars). | 必填 |
| `--run <str>` | Attach to this run id. |  |
| `--confidence <str>` | guess \| prelim \| confirmed \| rejected |  |

### arb status

Current idea (inside an idea worktree), an overview of open ideas, or (in the seed home) the seeds.

```text
arb status [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--json` | Print machine-readable JSON. |  |
| `--brief` | Compact text (as used by the hook). |  |

### arb export

Markdown of all ideas, notes and runs (newest first).

```text
arb export [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--out <path>` | Write to FILE instead of stdout. |  |

## 种子

还没有仓库的想法，放在种子园（默认 `~/arboria`）。任何目录下都能用。

### arb seed new

<span class="arb-tag">只由你</span>

(Human only) new seed and its notebook &lt;home&gt;/seeds/NAME.py (prints its path).

```text
arb seed new [OPTIONS] {name} [gist]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Slug: lowercase letters, digits, '-'. | 必填 |
| `gist` | The idea in one line (≤300 chars). |  |
| `--hypothesis <str>` | What you expect and why (≤300 chars). |  |
| `--metric <str>` | How success would be measured (≤200 chars). |  |

### arb seed list

Seeds, most recently edited first (dropped ones only with --all).

```text
arb seed list [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--all` | Include dropped seeds. |  |
| `--json` | Print machine-readable JSON. |  |

### arb seed show

One seed: gist, hypothesis, metric, status, notebook, where it was planted.

```text
arb seed show [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Seed name. | 必填 |
| `--json` | Print machine-readable JSON. |  |

### arb seed set

<span class="arb-tag">只由你</span>

(Human only) change a seed's gist, hypothesis or metric.

```text
arb seed set [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Seed name. | 必填 |
| `--gist <str>` | One line (≤300 chars); "" clears it. |  |
| `--hypothesis <str>` | ≤300 chars; "" clears it. |  |
| `--metric <str>` | ≤200 chars; "" clears it. |  |

### arb seed drop

<span class="arb-tag">只由你</span>

(Human only) give up on a seed; its notebook is kept.

```text
arb seed drop [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Seed name. | 必填 |

### arb seed revive

<span class="arb-tag">只由你</span>

(Human only) bring a dropped seed back to thinking.

```text
arb seed revive [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Seed name. | 必填 |

### arb seed plant

<span class="arb-tag">只由你</span>

(Human only) plant a seed into a repo as a new idea, whose notebook links back.

```text
arb seed plant [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Seed name. | 必填 |
| `--repo <path>` | Target repo (default: the repo of the current directory); must be arb-initialized. |  |
| `--as <str>` | Idea name (default: the seed's name). |  |
| `--hypothesis <str>` | Default: the seed's. |  |
| `--metric <str>` | Default: the seed's. |  |
| `--again` | Plant a seed that is already planted (into another repo, or under another name). |  |

## 提交、批准与集群

`submit` 只在本地生成审核单；只有 `approve` 会把作业交给 Slurm。

### arb submit

Snapshot the idea worktree (or reuse --from's) and ask the human to approve a cluster run (local only). An identical earlier run is returned instead of a new one, unless --repeat.

```text
arb submit [OPTIONS] [command]...
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `command...` | The training command, after `--` (default: run.command in arboria.toml, or the --from run's command). |  |
| `--reason <str>` | What changed vs. the previous run and why (≤300 chars). | 必填 |
| `--from <str>` | Reuse this run's snapshot, resources and overrides (no new snapshot). |  |
| `--set <str> …` | Override key=value (repeatable), rendered per run.override\_style. |  |
| `--gpus <int>` | Total GPUs (default with --from: the parent's). |  |
| `--time <str>` | Walltime: 90m, 12h, 1d6h or HH:MM:SS (default with --from: the parent's). |  |
| `--nodes <int>` | Nodes (GPUs are split evenly; default 1). |  |
| `--partition <str>` | Default: the machine's default\_partition. |  |
| `--machine <str>` | Default: arboria.toml default\_machine. |  |
| `--mem <int>` | Memory per node in GB (default: GPU share; more is billed as extra GPUs). |  |
| `--cpus <int>` | CPUs per node (default: GPU share; more is billed as extra GPUs). |  |
| `--allow-large` | Snapshot large untracked files anyway. |  |
| `--repeat` | Always a new run, even if identical to an earlier one (e.g. seed variance). |  |
| `--json` | Print machine-readable JSON. |  |

### arb card

Show a submitted run's approval card, recomputed now (local only).

```text
arb card [OPTIONS] {run_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `run_id` | Run id, e.g. r001. | 必填 |
| `--json` | Print machine-readable JSON. |  |

### arb approve

<span class="arb-tag">只由你</span>

(Human only, in your own terminal) approve a run's card and submit it to the cluster; you type the run id to confirm. Re-run to resume.

```text
arb approve [OPTIONS] {run_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `run_id` | Run id, e.g. r001. | 必填 |

### arb reject

Reject a run awaiting approval (nothing is sent to the cluster).

```text
arb reject [OPTIONS] {run_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `run_id` | Run id, e.g. r001. | 必填 |
| `--reason <str>` | Why (saved as a note, ≤1000 chars). |  |

### arb cancel

Cancel a submitted run's jobs on the cluster.

```text
arb cancel [OPTIONS] {run_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `run_id` | Run id, e.g. r001. | 必填 |

### arb poll

One pass: update job states, run status and cost; fetch ARBORIA\_OUT when done.

```text
arb poll [OPTIONS] [run_id]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `run_id` | Default: every active run. |  |
| `--json` | Print machine-readable JSON. |  |

### arb watch

Keep polling active runs; in the observation window cancel on crash or a log line matching arboria.toml [watch] fail\_patterns (default: loss NaN, CUDA OOM).

```text
arb watch [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--once` | One pass, then exit. |  |
| `--interval <int>` | Seconds between passes. | `60` |
| `--window <int>` | Observation window after a job starts, in minutes (default: arboria.toml [watch] observation\_minutes). |  |

### arb logs

Tail a run's Slurm log on the cluster (use this instead of ssh).

```text
arb logs [OPTIONS] {run_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `run_id` | Run id, e.g. r001. | 必填 |
| `--segment <int>` | Default: the latest started segment. |  |
| `-n <int>` | Number of lines. | `100` |

### arb remote ls

List a run's directory (or its ARBORIA\_OUT) on the cluster.

```text
arb remote ls [OPTIONS] {run_id} [subpath]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `run_id` | Run id, e.g. r001. | 必填 |
| `subpath` | Relative path inside the run dir. |  |
| `--out` | List ARBORIA\_OUT instead of the run dir. |  |

## 待办与 notebook 命令

notebook 里 `lab.todo(...)` 和 `lab.<名字>(...)` 留下的活。

### arb todo list

Todos of the current idea (in an idea worktree), of every open idea, or (in the seed home) of the seeds.

```text
arb todo list [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--all` | Include done and dropped todos. |  |
| `--json` | Print machine-readable JSON. |  |

### arb todo done

Mark a todo done with a short result for the human.

```text
arb todo done [OPTIONS] {task_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `task_id` | Todo id, e.g. t001. | 必填 |
| `--result <str>` | What was done / found (≤600 chars). | 必填 |

### arb todo drop

Drop a todo that will not be done.

```text
arb todo drop [OPTIONS] {task_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `task_id` | Todo id, e.g. t001. | 必填 |
| `--reason <str>` | Why (≤600 chars). |  |

### arb commands list

Commands of this project and the global library (project wins on a clash).

```text
arb commands list [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--json` | Print machine-readable JSON. |  |

### arb commands guide

The conventions for writing a notebook command (for Claude).

```text
arb commands guide [OPTIONS]
```

## 看板

### arb ui

Open the read-only dashboard (看板): what needs you, the idea tree, SU budget; or, outside any repo (or with --home), the seed home (种子园).

Runs `marimo run` on 127.0.0.1; it reads the ledger and never changes anything.

```text
arb ui [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--idea <str>` | Idea to select (default: the idea of the current worktree, else the most recently active open one). |  |
| `--port <int>` | Port (default: marimo picks a free one). |  |
| `--home` | The seed home (种子园) instead; the default outside any repo. |  |

## 机器

机器配置在 `~/.config/arboria/clusters.toml`。

### arb machine init

Measure a Slurm machine over your ssh master (read-only) and print its profile.

```text
arb machine init [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Machine name, e.g. deltaai. | 必填 |
| `--host <str>` | ~/.ssh/config alias (default: the profile's host, else NAME). |  |
| `--account <str>` | Slurm account, if you have several. |  |
| `--write` | Write the profile into clusters.toml. |  |
| `--replace` | With --write: rewrite an existing profile (keeps a backup). |  |

### arb doctor

Read-only checks of a machine profile, the ssh setup and the cluster side.

```text
arb doctor [OPTIONS] [name]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Machine (default: arboria.toml default\_machine). |  |

## Claude Code 与经验

### arb install-claude

Add the SessionStart hook and permission rules to .claude/settings.json (the machine-specific global commands dir goes into .claude/settings.local.json).

```text
arb install-claude [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--main` | Write into the main worktree instead. |  |
| `--user` | Write into ~/.claude/settings.json instead. |  |
| `--home` | Set up the seed home (~/arboria): its .claude/settings.json and CLAUDE.md. |  |

### arb hook session-start

SessionStart hook: reads hook JSON on stdin, prints additionalContext JSON.

```text
arb hook session-start [OPTIONS]
```

### arb claudemd sync

Write the arboria block (rules + accepted lessons) into the main worktree's CLAUDE.md.

```text
arb claudemd sync [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--no-commit` | Write CLAUDE.md but do not commit it. |  |

### arb lesson propose

Propose a lesson; it only goes into CLAUDE.md after the human accepts it.

```text
arb lesson propose [OPTIONS] {text}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `text` | One line, ≤200 chars: a reusable project pitfall. | 必填 |
| `--idea <str>` | Source idea (default: current idea). |  |
| `--run <str>` | Source run id. |  |

### arb lesson list

List lessons (default: proposed + accepted), newest first.

```text
arb lesson list [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--all` | Include retired/rejected lessons. |  |
| `--json` | Print machine-readable JSON. |  |

### arb lesson accept

<span class="arb-tag">只由你</span>

(Human only, in your own terminal) accept a proposed lesson and update CLAUDE.md; you type the lesson id to confirm.

```text
arb lesson accept [OPTIONS] {lesson_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `lesson_id` | Lesson id, e.g. l001. | 必填 |

### arb lesson reject

Reject a proposed lesson.

```text
arb lesson reject [OPTIONS] {lesson_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `lesson_id` | Lesson id, e.g. l001. | 必填 |

### arb lesson retire

Retire an accepted lesson and remove it from CLAUDE.md.

```text
arb lesson retire [OPTIONS] {lesson_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `lesson_id` | Lesson id, e.g. l001. | 必填 |
