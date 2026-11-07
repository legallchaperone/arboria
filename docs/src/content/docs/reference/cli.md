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
| [`arb idea new`](#arb-idea-new) | New idea: in a repo, branch idea/NAME and its worktree folder (prints its path). |
| [`arb idea list`](#arb-idea-list) | List all ideas; outside any repo, the seeds (most recently edited first). |
| [`arb idea show`](#arb-idea-show) | One idea, or (outside any repo) one seed. |
| [`arb idea set`](#arb-idea-set) | (Human only) change a seed's gist, hypothesis or metric (outside any repo). |
| [`arb idea drop`](#arb-idea-drop) | Drop an idea (= `close --drop`), or (outside any repo) a seed. |
| [`arb idea revive`](#arb-idea-revive) | (Human only) bring a dropped seed back to thinking (outside any repo). |
| [`arb idea plant`](#arb-idea-plant) | (Human only) plant a seed into this repo as a new idea, whose notebook links back. |
| [`arb idea notebook`](#arb-idea-notebook) | Create the notebook (and notebook.py link) of an idea that has none. |
| [`arb idea style`](#arb-idea-style) | Give older notebooks the Arboria font (Maple Mono CN). |
| [`arb idea close`](#arb-idea-close) | Close an idea with --merge or --drop; removes its worktree and branch. |
| [`arb init`](#arb-init) | Set up arboria for this repository (idempotent). |
| [`arb run summary`](#arb-run-summary) | Set a run's result summary (run must belong to the current idea). |
| [`arb note`](#arb-note) | (Human only) add a note to the current idea (--confidence also updates the idea). |
| [`arb status`](#arb-status) | Where things stand: the current idea, the open ideas, or the seeds. |
| [`arb tree`](#arb-tree) | The research tree: ideas and their runs, one line each (parent, change, result, conclusion and who wrote it). |
| [`arb export`](#arb-export) | Markdown of all ideas, notes and runs (newest first). |
| [`arb submit`](#arb-submit) | Ask the human to approve a cluster run (local only: snapshot + approval card). |
| [`arb card`](#arb-card) | Show a submitted run's approval card, recomputed now (local only). |
| [`arb approve`](#arb-approve) | (Human only, in your own terminal) approve a run's card and submit it to the cluster; you type the run id to confirm. Re-run to resume. |
| [`arb reject`](#arb-reject) | Reject a run awaiting approval (nothing is sent to the cluster). |
| [`arb cancel`](#arb-cancel) | Cancel a submitted run's jobs on the cluster. |
| [`arb poll`](#arb-poll) | One pass: update job states, run status and cost; fetch ARBORIA\_OUT when done. |
| [`arb watch`](#arb-watch) | Keep polling active runs; cancel a run that crashes or logs a failure early on. |
| [`arb logs`](#arb-logs) | Tail a run's Slurm log on the cluster (use this instead of ssh). |
| [`arb remote ls`](#arb-remote-ls) | List a run's directory (or its ARBORIA\_OUT) on the cluster. |
| [`arb key trust`](#arb-key-trust) | (Human only) Trust the app's Touch ID key; you type the fingerprint the app shows. |
| [`arb key list`](#arb-key-list) | List trusted Touch ID approval keys. |
| [`arb key revoke`](#arb-key-revoke) | (Human only) Stop trusting a Touch ID approval key. |
| [`arb milestone list`](#arb-milestone-list) | The roadmap: open milestones by deadline, with spend and checklist. |
| [`arb milestone show`](#arb-milestone-show) | One milestone. |
| [`arb milestone new`](#arb-milestone-new) | (Human only) Add a milestone to the roadmap. |
| [`arb milestone set`](#arb-milestone-set) | (Human only) Change a milestone. |
| [`arb milestone check`](#arb-milestone-check) | (Human only) Tick a checklist item. |
| [`arb milestone uncheck`](#arb-milestone-uncheck) | (Human only) Untick a checklist item. |
| [`arb milestone done`](#arb-milestone-done) | (Human only) Mark a milestone done. |
| [`arb milestone reopen`](#arb-milestone-reopen) | (Human only) Reopen a done or dropped milestone. |
| [`arb milestone drop`](#arb-milestone-drop) | (Human only) Drop a milestone (it stays in the history). |
| [`arb todo list`](#arb-todo-list) | Todos of the current idea (in an idea worktree), of every open idea, or (in the seed home) of the seeds. |
| [`arb todo done`](#arb-todo-done) | Mark a todo done with a short result for the human. |
| [`arb todo drop`](#arb-todo-drop) | Drop a todo that will not be done. |
| [`arb commands list`](#arb-commands-list) | Commands of this project and the global library (project wins on a clash). |
| [`arb commands guide`](#arb-commands-guide) | The conventions for writing a notebook command (for the agent). |
| [`arb commands promote`](#arb-commands-promote) | Copy a project command into the global library (~/.config/arboria/commands). |
| [`arb inspect guide`](#arb-inspect-guide) | The conventions for answering a lab.inspect todo (for the agent). |
| [`arb inspect source`](#arb-inspect-source) | The code a 读代码 todo is about, at its code\_ref (read from git; nothing is changed). |
| [`arb inspect check`](#arb-inspect-check) | Validate an answer and print what the human will see; saves nothing. |
| [`arb inspect done`](#arb-inspect-done) | Deliver the answer to a 读代码 todo: arb extracts the code, stores it, closes the todo. |
| [`arb agent run`](#arb-agent-run) | Hand one todo to `claude -p` (your own claude) and wait for it. Refused inside Claude Code. |
| [`arb agent list`](#arb-agent-list) | The background agent's recent runs (newest first): outcome, tokens, five\_hour window. |
| [`arb ui`](#arb-ui) | Open the read-only dashboard (看板). |
| [`arb machine init`](#arb-machine-init) | Measure a Slurm machine over your ssh master (read-only) and print its profile. |
| [`arb doctor`](#arb-doctor) | Read-only checks of a machine profile, the ssh setup and the cluster side. |
| [`arb install-claude`](#arb-install-claude) | Add the hooks and permission rules to .claude/settings.json (the machine-specific global commands dir goes into .claude/settings.local.json) and the arboria Skill to .claude/skills/arboria/SKILL.md. |
| [`arb hook session-start`](#arb-hook-session-start) | SessionStart hook: reads hook JSON on stdin, prints additionalContext JSON. |
| [`arb hook prompt-submit`](#arb-hook-prompt-submit) | UserPromptSubmit hook: one line of context when the ledger changed since the last prompt. |
| [`arb claudemd sync`](#arb-claudemd-sync) | Write the arboria block (rules + accepted lessons) into the main worktree's CLAUDE.md. |
| [`arb lesson propose`](#arb-lesson-propose) | Propose a lesson; it only goes into CLAUDE.md after the human accepts it. |
| [`arb lesson list`](#arb-lesson-list) | List lessons (default: proposed + accepted), newest first. |
| [`arb lesson accept`](#arb-lesson-accept) | (Human only, in your own terminal) accept a proposed lesson and update CLAUDE.md; you type the lesson id to confirm. |
| [`arb lesson reject`](#arb-lesson-reject) | Reject a proposed lesson. |
| [`arb lesson retire`](#arb-lesson-retire) | Retire an accepted lesson and remove it from CLAUDE.md. |

## idea 与种子

`arb idea` 管一个 idea 的两个阶段：在初始化过的仓库里是 idea（分支 + worktree）；在仓库外（或种子园 `~/arboria` 里）是种子，也就是还没有仓库的 idea。在没有 `arb init` 的仓库里会报错，不会悄悄记成种子。`arb idea plant` 在目标仓库里运行，把种子种成 idea。旧写法 `arb seed …` 仍然可用，是同一组种子命令的别名。

### arb idea new

New idea: in a repo, branch idea/NAME and its worktree folder (prints its path).

Outside any repo: (human only) a seed in ~/arboria and its notebook seeds/NAME.py, to plant into a repo later with `arb idea plant NAME`.

```text
arb idea new [OPTIONS] {name} [gist]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Slug: lowercase letters, digits, '-'. | 必填 |
| `gist` | Seeds only: the idea in one line (≤300 chars). |  |
| `--hypothesis <str>` | What you expect and why (≤300 chars); required in a repo. |  |
| `--metric <str>` | How success is measured (≤200 chars); required in a repo. |  |
| `--parent <str>` | Branch off this open idea (in a repo). |  |
| `--json` | Print machine-readable JSON. |  |

### arb idea list

List all ideas; outside any repo, the seeds (most recently edited first).

```text
arb idea list [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--all` | Seeds: include dropped ones. |  |
| `--json` | Print machine-readable JSON. |  |

### arb idea show

One idea, or (outside any repo) one seed.

An idea: hypothesis, metric, status, branch, notebook and runs. A seed: gist, hypothesis, metric, status, notebook, where it was planted.

```text
arb idea show [OPTIONS] [name]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Default (in a repo): the idea of the current directory. |  |
| `--json` | Print machine-readable JSON. |  |

### arb idea set

<span class="arb-tag">只由你</span>

(Human only) change a seed's gist, hypothesis or metric (outside any repo).

```text
arb idea set [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Seed name. | 必填 |
| `--gist <str>` | One line (≤300 chars); "" clears it. |  |
| `--hypothesis <str>` | ≤300 chars; "" clears it. |  |
| `--metric <str>` | ≤200 chars; "" clears it. |  |

### arb idea drop

Drop an idea (= `close --drop`), or (outside any repo) a seed.

An idea: archive a final snapshot, remove its worktree and branch. A seed (human only): give up on it; its notebook is kept.

```text
arb idea drop [OPTIONS] [name]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Default (in a repo): the idea of the current directory. |  |
| `--confidence <str>` | guess \| prelim \| confirmed \| rejected |  |
| `--allow-large` | Snapshot large untracked files anyway. |  |
| `--json` | Print machine-readable JSON. |  |

### arb idea revive

<span class="arb-tag">只由你</span>

(Human only) bring a dropped seed back to thinking (outside any repo).

```text
arb idea revive [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Seed name. | 必填 |

### arb idea plant

<span class="arb-tag">只由你</span>

(Human only) plant a seed into this repo as a new idea, whose notebook links back.

Run it inside the target repo (or pass --repo).

```text
arb idea plant [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Seed name. | 必填 |
| `--repo <path>` | Target repo (default: the repo of the current directory); must be arb-initialized. |  |
| `--as <str>` | Idea name (default: the seed's name). |  |
| `--hypothesis <str>` | Default: the seed's. |  |
| `--metric <str>` | Default: the seed's. |  |
| `--again` | Plant a seed that is already planted (into another repo, or under another name). |  |

### arb idea notebook

Create the notebook (and notebook.py link) of an idea that has none.

```text
arb idea notebook [OPTIONS] [name]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Defaults to the idea of the current directory. |  |

### arb idea style

Give older notebooks the Arboria font (Maple Mono CN).

In a repo: every idea notebook; outside any repo: every seed notebook. Refreshes the arboria.css next to them and adds css\_file="arboria.css" to marimo.App(...) where it is missing; no cell is touched. Idempotent. Yours only: refused inside Claude Code (it writes the notebooks).

```text
arb idea style [OPTIONS]
```

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

## 项目、run 与笔记

在实验代码仓库里：初始化、run 的结果、笔记和概览。

### arb init

Set up arboria for this repository (idempotent).

```text
arb init [OPTIONS]
```

### arb run summary

Set a run's result summary (run must belong to the current idea).

```text
arb run summary [OPTIONS] {run_id} {text}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `run_id` | Run id, e.g. r001. | 必填 |
| `text` | Result summary (≤10 lines, ≤1500 chars). | 必填 |
| `--json` | Print machine-readable JSON. |  |

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

Where things stand: the current idea, the open ideas, or the seeds.

Inside an idea worktree, that idea; elsewhere in a repo, an overview of its open ideas; in the seed home, the seeds.

```text
arb status [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--json` | Print machine-readable JSON. |  |
| `--brief` | Compact text (as used by the hook). |  |

### arb tree

The research tree: ideas and their runs, one line each (parent, change, result, conclusion and who wrote it).

Built from the ledger and the fetched metrics.jsonl only; the order is stable. Over the budget, finished ideas off your path fold to one line.

```text
arb tree [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--idea <str>` | Only this idea and what grew from it. |  |
| `--depth <int range>` | Levels of ideas to show. |  |
| `--budget <int range>` | Characters before finished subtrees fold. | `16000` |
| `--json` | Print machine-readable JSON. |  |

### arb export

Markdown of all ideas, notes and runs (newest first).

```text
arb export [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--out <path>` | Write to FILE instead of stdout. |  |

## 提交、批准与集群

`submit` 只在本地生成审核单；只有 `approve` 会把作业交给 Slurm。

### arb submit

Ask the human to approve a cluster run (local only: snapshot + approval card).

Snapshots the idea worktree (or reuses --from's). An identical earlier run is returned instead of a new one, unless --repeat.

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

Keep polling active runs; cancel a run that crashes or logs a failure early on.

In the observation window a run is cancelled on a crash or a log line matching watch.fail\_patterns in arboria.toml (default: loss NaN, CUDA OOM).

```text
arb watch [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--once` | One pass, then exit. |  |
| `--interval <int>` | Seconds between passes. | `60` |
| `--window <int>` | Observation window after a job starts, in minutes (default: watch.observation\_minutes in arboria.toml). |  |

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
| `--json` | Print machine-readable JSON. |  |

## Touch ID 签字密钥

Mac app 用 Secure Enclave 里的密钥给审核单签字；只有你在自己的终端里信任过的密钥签的字才算批准。

### arb key trust

<span class="arb-tag">只由你</span>

(Human only) Trust the app's Touch ID key; you type the fingerprint the app shows.

```text
arb key trust [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--public-key <str>` | Base64 X9.63 P-256 public key (default: the one the app saved). |  |
| `--label <str>` | A name for this device. | `Mac` |

### arb key list

List trusted Touch ID approval keys.

```text
arb key list [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--json` | Print machine-readable JSON. |  |

### arb key revoke

<span class="arb-tag">只由你</span>

(Human only) Stop trusting a Touch ID approval key.

```text
arb key revoke [OPTIONS] {fingerprint}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `fingerprint` | Fingerprint, as `arb key list` shows it. | 必填 |

## 路线图（里程碑）

你定的里程碑：截止日期、算力预算、包含的 idea 和清单。改动只由你在自己的终端里做，`list` / `show` 谁都能看。

### arb milestone list

The roadmap: open milestones by deadline, with spend and checklist.

```text
arb milestone list [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--all` | Also done and dropped milestones. |  |
| `--json` | Print machine-readable JSON. |  |

### arb milestone show

One milestone.

```text
arb milestone show [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` |  | 必填 |
| `--json` | Print machine-readable JSON. |  |

### arb milestone new

<span class="arb-tag">只由你</span>

(Human only) Add a milestone to the roadmap.

```text
arb milestone new [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | Short name (lowercase, digits, '-'). | 必填 |
| `--title <str>` | What it is (≤80 chars). | 必填 |
| `--deadline <str>` | YYYY-MM-DD. |  |
| `--su <float>` | SU budget for the runs of its ideas. |  |
| `--idea <str> …` | An idea of this project (repeatable). |  |
| `--item <str> …` | A checklist item (≤120 chars, repeatable). |  |

### arb milestone set

<span class="arb-tag">只由你</span>

(Human only) Change a milestone.

```text
arb milestone set [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` |  | 必填 |
| `--title <str>` |  |  |
| `--deadline <str>` | YYYY-MM-DD. |  |
| `--no-deadline` | Remove the deadline. |  |
| `--su <float>` | SU budget. |  |
| `--no-su` | Remove the SU budget. |  |
| `--add-idea <str> …` |  |  |
| `--remove-idea <str> …` |  |  |
| `--add-item <str> …` |  |  |
| `--remove-item <int> …` | Checklist item number. |  |

### arb milestone check

<span class="arb-tag">只由你</span>

(Human only) Tick a checklist item.

```text
arb milestone check [OPTIONS] {name} {item}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` |  | 必填 |
| `item` | Checklist item number. | 必填 |

### arb milestone uncheck

<span class="arb-tag">只由你</span>

(Human only) Untick a checklist item.

```text
arb milestone uncheck [OPTIONS] {name} {item}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` |  | 必填 |
| `item` | Checklist item number. | 必填 |

### arb milestone done

<span class="arb-tag">只由你</span>

(Human only) Mark a milestone done.

```text
arb milestone done [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` |  | 必填 |

### arb milestone reopen

<span class="arb-tag">只由你</span>

(Human only) Reopen a done or dropped milestone.

```text
arb milestone reopen [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` |  | 必填 |

### arb milestone drop

<span class="arb-tag">只由你</span>

(Human only) Drop a milestone (it stays in the history).

```text
arb milestone drop [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` |  | 必填 |

## 待办与 notebook 命令

notebook 里 `lab.todo(...)`、`lab.<名字>(...)` 和 `lab.inspect(...)` 留下的活。

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
| `--json` | Print machine-readable JSON. |  |

### arb todo drop

Drop a todo that will not be done.

```text
arb todo drop [OPTIONS] {task_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `task_id` | Todo id, e.g. t001. | 必填 |
| `--reason <str>` | Why (≤600 chars). |  |
| `--json` | Print machine-readable JSON. |  |

### arb commands list

Commands of this project and the global library (project wins on a clash).

```text
arb commands list [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `--json` | Print machine-readable JSON. |  |

### arb commands guide

The conventions for writing a notebook command (for the agent).

```text
arb commands guide [OPTIONS]
```

### arb commands promote

Copy a project command into the global library (~/.config/arboria/commands).

```text
arb commands promote [OPTIONS] {name}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `name` | The project command to copy. | 必填 |
| `--force` | Replace a different global one. |  |

### arb inspect guide

The conventions for answering a lab.inspect todo (for the agent).

```text
arb inspect guide [OPTIONS]
```

### arb inspect source

The code a 读代码 todo is about, at its code\_ref (read from git; nothing is changed).

```text
arb inspect source [OPTIONS] {task_id} [path]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `task_id` | Todo id, e.g. t007. | 必填 |
| `path` | File to print with line numbers (omit to list the files). |  |

### arb inspect check

Validate an answer and print what the human will see; saves nothing.

```text
arb inspect check [OPTIONS] {task_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `task_id` | Todo id, e.g. t007. | 必填 |
| `--file <str>` | The answer as JSON: a file, or - for stdin. | 必填 |
| `--json` | Print machine-readable JSON. |  |

### arb inspect done

Deliver the answer to a 读代码 todo: arb extracts the code, stores it, closes the todo.

```text
arb inspect done [OPTIONS] {task_id}
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `task_id` | Todo id, e.g. t007. | 必填 |
| `--file <str>` | The answer as JSON: a file, or - for stdin. | 必填 |
| `--json` | Print machine-readable JSON. |  |

### arb agent run

Hand one todo to `claude -p` (your own claude) and wait for it. Refused inside Claude Code.

```text
arb agent run [OPTIONS] [task_id]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `task_id` | Todo id (default: the oldest open todo it can do, in this idea when in its worktree). |  |
| `--dry-run` | Print the prompt and the claude command line; start nothing. |  |
| `--json` | Print machine-readable JSON. |  |

### arb agent list

The background agent's recent runs (newest first): outcome, tokens, five\_hour window.

```text
arb agent list [OPTIONS]
```

| 参数 | 说明 | 默认 |
| --- | --- | --- |
| `-n <int>` | How many. | `20` |
| `--json` | Print machine-readable JSON. |  |

## 看板

### arb ui

Open the read-only dashboard (看板).

In a repo: what needs you, the idea tree, SU budget; outside any repo (or with --home), the seed home (种子园). Runs `marimo run` on 127.0.0.1; it reads the ledger and never changes anything.

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

Add the hooks and permission rules to .claude/settings.json (the machine-specific global commands dir goes into .claude/settings.local.json) and the arboria Skill to .claude/skills/arboria/SKILL.md.

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

### arb hook prompt-submit

UserPromptSubmit hook: one line of context when the ledger changed since the last prompt.

```text
arb hook prompt-submit [OPTIONS]
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
| `--json` | Print machine-readable JSON. |  |

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
