"""The Claude Code Skill `arb install-claude` writes (`.claude/skills/arboria/SKILL.md`).

ADR 0004: Claude's interface is `arb … --json` plus this Skill, loaded only when a task
needs it, so the SessionStart context stays an index. The Skill explains the CLI; it
grants nothing. Every refusal it describes is enforced by code and deny rules, not by
this text. arb rewrites the file only while it still carries MARKER (a human who edits
the Skill and removes the marker keeps their version).
"""

from __future__ import annotations

from pathlib import Path

SKILL_DIR = Path(".claude") / "skills" / "arboria"
SKILL_FILE = "SKILL.md"
MARKER = "<!-- arboria: written by `arb install-claude`; remove this line to keep your own edits -->"

SKILL = f"""---
name: arboria
description: Work in an Arboria ML research repo through the `arb` CLI. Read what was tried and what worked (`arb tree`), pick up the human's todos, propose cluster runs (approval cards), write run summaries, notebook commands and code readings, propose lessons. Use whenever a task touches ideas, runs, experiments, results, todos, the cluster, Slurm or the `.arboria/` ledger.
---
{MARKER}

# Working with arb

All research state (ideas, runs, notes, decisions, todos, lessons) lives in a SQLite
ledger that only `arb` writes. Never edit `.arboria/` by hand and never run Slurm or ssh
yourself. Read with the commands below; every one takes `--json`.

The human owns the notebook (`notebook.py`: read it, never edit it), approvals, notes,
seeds, the roadmap and the hypothesis/metric of an idea. Those commands refuse to run in
Claude Code; do not look for a way around that, ask the human instead.

## Start here
- `arb status --json`: the current idea (or the open ideas), its todos and open decisions.
- `arb tree --json` (`--idea NAME` for a subtree, `--depth N`): every idea and run with
  parent, change, result and conclusion. Read it before proposing a run so you do not
  repeat what was tried. A conclusion's source is `note` (the human) or `summary` (AI);
  trust the human's.
- `arb todo list --json`: the work the human left for you.

## Read
| command | what |
| --- | --- |
| `arb idea list --json`, `arb idea show [NAME] --json` | ideas, their hypothesis and metric |
| `arb card RUN --json` | a run's approval card (what will run, estimated SU, checks) |
| `arb poll --json` | refresh cluster state of the submitted runs |
| `arb logs RUN [-n 200]` | a run's Slurm log (text) |
| `arb remote ls RUN [SUBPATH] [--out] --json` | the run's directory on the cluster |
| `arb lesson list --json` | accepted lessons (project pitfalls) |
| `arb commands list --json` | the notebook command library |
| `arb milestone list --json` | the human's roadmap: deadlines and SU budgets |

## Write (yours)
| command | when |
| --- | --- |
| `arb idea new NAME --hypothesis "…" --metric "…" [--parent IDEA] --json` | a new direction: a branch and a worktree (`worktree` in the JSON); work there |
| `arb submit --reason "…" --gpus N --time 4h [-- CMD]` | propose a run: makes an approval card, submits nothing |
| `arb submit --from rNNN --set lr=1e-4 --reason "…"` | the same run with changed config |
| `arb run summary RUN "…" --json` | after a run ends: the result in ≤10 lines. The first sentence is what the tree shows, so lead with the conclusion ("warmup 500 lowers val/loss 2.5 → 2.2; 2000 is too long.") |
| `arb todo done tNNN --result "…" --json` | a plain todo is done (≤600 chars, for the human) |
| `arb todo drop tNNN --reason "…" --json` | a todo you will not do, and why |
| `arb lesson propose "…" [--run RUN] --json` | a reusable pitfall; the human accepts it |

`--reason` is the change since the parent run, in one line: it is the run's line in the tree.

## Runs and approval
1. `arb submit …` writes a card. Tell the human the run id and `arb card RUN`.
2. The human approves (Touch ID in the Arboria app, or `arb approve RUN` in their own
   terminal). You cannot approve, and nothing is submitted without it.
3. `arb watch` / `arb poll` follow the jobs; `arb logs RUN` when something fails.
4. When it ends, read the curves the training code wrote (`metrics.jsonl`, fetched into
   the run's out dir) via `arb tree --idea NAME --json`, then `arb run summary`.

Training code must: write the resolved config to `$ARBORIA_OUT/config.yaml`; append
`{{"step": …, "<metric>": …}}` lines to `$ARBORIA_OUT/metrics.jsonl`; call
`wandb.init(id=$ARBORIA_WANDB_ID, group=$ARBORIA_IDEA)` if it uses wandb; read paths only
from `ARBORIA_*` variables; save resumable checkpoints to `$ARBORIA_CKPT_DIR`.

## Todos of other kinds
- Command todos (`lab.<name>(...)` in the notebook had no command): read
  `arb commands guide` first and follow it.
- 读代码 todos (`lab.inspect`): read `arb inspect guide`, check with
  `arb inspect check tNNN --file - --json`, deliver with `arb inspect done tNNN --file - --json`
  (not `arb todo done`).

## When arb refuses
Errors go to stderr with exit code 1 and say what to do. "只由人" means the command is the
human's; tell them the exact command to run instead of retrying.
"""


def path(root: Path) -> Path:
    return Path(root) / SKILL_DIR / SKILL_FILE


def install(root: Path) -> str | None:
    """Write or refresh the Skill under `root`; returns a change line, or None when it is
    already current or the human made it their own (no marker)."""
    p = path(root)
    if p.exists():
        current = p.read_text(encoding="utf-8")
        if current == SKILL:
            return None
        if MARKER not in current:
            return f"left {p} as is (edited by you: it no longer has the arboria marker)"
    p.parent.mkdir(parents=True, exist_ok=True)
    existed = p.exists()
    p.write_text(SKILL, encoding="utf-8")
    return f"{'updated' if existed else 'wrote'} skill {p}"
