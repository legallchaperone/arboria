"""The arb-managed block in the main worktree's CLAUDE.md: stable rules + accepted lessons.

Only the text between BEGIN and END is ours; everything else in the file is the
user's and is preserved byte for byte. Current experiment state is *not* written
here: the SessionStart hook delivers that.
"""

from __future__ import annotations

import os
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from . import gitutil
from .db import Ledger
from .errors import ArbError
from .paths import RepoContext

FILENAME = "CLAUDE.md"
BEGIN = "<!-- arboria:begin — 由 arb 管理，块内修改会被覆盖 -->"
END = "<!-- arboria:end -->"
BEGIN_PREFIX = "<!-- arboria:begin"
END_PREFIX = "<!-- arboria:end"
MAX_BLOCK_LINES = 50
MAX_ACCEPTED_LESSONS = 20
COMMIT_MESSAGE = "arboria: update CLAUDE.md"

RULES = """\
## 研究工作流（Arboria）
- 实验状态只通过 `arb` 读写，不要手改 `.arboria/`。
- 当前状态（idea、run、待办、待批准的 run、最新经验）由会话开始时的 hook 注入；本文件只放稳定规则和已确认的经验。
- 一个 idea 一个文件夹（worktree）：只在当前 idea 的文件夹里改代码。
- `notebook.py`（marimo，链接到 `.arboria/notebooks/<idea>.py`）是人的：读它了解人的想法和分析，绝不修改。
- 你的工作来自待办：`arb todo list` 查看，做完用 `arb todo done tNNN --result "..."`（≤600 字）交代结果。
- 命令待办（人在 notebook 里调用了 `lab.<名字>(...)`）：先运行 `arb commands guide` 看约定，只写 `.arboria/commands/<名字>.py`（或全局命令目录）里的那个文件，再 `arb todo done`。读代码待办（`lab.inspect(...)`）按 `arb inspect guide`，用 `arb inspect done` 交付。
- 同一类分析在 notebook 里反复出现，或待办要的是一次分析：写成命令（默认项目范围），在 `arb todo done` 里写明 `lab.<名字>(...)` 怎么调用。
- 看状态用 `arb status`。`arb note` 只由人写；你的结论写进 `arb todo done --result` 或 `arb run summary RUN "..."`。
- 上集群：`arb submit --reason "..." --gpus N --time 12h [-- CMD]`，或 `arb submit --from rNNN --set k=v --reason "..."`。它只在本地生成快照和审核单，不会提交。
- 批准只能由人在自己的终端里做：你不能运行 `arb approve` 和 `arb lesson accept`；绝不直接 `sbatch`/`srun`，也不要 ssh 上集群。
- 看集群上的日志和文件用 `arb logs RUN`、`arb remote ls RUN`，不用 ssh；跑完的输出在 `.arboria/runs/RUN/out/`。
- 发现可复用的教训用 `arb lesson propose "..."` 提出，由人确认；不要直接写进 CLAUDE.md。
- 训练代码约定：
  1. 启动时把完整的解析后配置导出为 `$ARBORIA_OUT/config.yaml`。
  2. `wandb.init(id=ARBORIA_WANDB_ID, group=ARBORIA_IDEA)`。
  3. 路径只从 `ARBORIA_*` 变量读取。
  4. 可续训：checkpoint 存到 `ARBORIA_CKPT_DIR`，重启后能接着跑（超过分区上限的 run 会分段）。"""

LESSONS_HEADING = "## 经验（已确认）"

# The block in the seed home's CLAUDE.md (`arb install-claude --home`): no lessons there
# (they are per project), and the home is not a repo, so nothing is committed.
HOME_RULES = """\
## 种子园（Arboria）
- 这里是人的种子园：还没有仓库的研究想法。当前的种子和待办由会话开始时的 hook 注入。
- `seeds/<名字>.py`（marimo）是人的 notebook：读它了解人的想法，绝不修改；也不要改 `.arboria/`。
- 种子就是还没有仓库的 idea：在这里（仓库外）`arb idea` 管的是种子。看种子用 `arb idea list`、`arb idea show NAME`。种子只由人写：`arb idea new/set/drop/revive`（以及旧名 `arb seed …`）你都不能运行。
- 种下（在目标仓库里 `arb idea plant NAME`）由人在自己的终端里做：它会在那个仓库里建 idea 的分支和 worktree。
- 你的工作来自待办：`arb todo list` 查看，做完用 `arb todo done tNNN --result "..."`（≤600 字）交代结果。
- 命令待办（人在种子 notebook 里调用了 `lab.<名字>(...)`）：先运行 `arb commands guide`，只写全局命令目录里的那个文件；种子没有 run，命令里不用 `ctx.run`。
- 讨论想法时引用种子 notebook 里的内容；你的结论写进 `arb todo done --result`，不写进 notebook。"""


# ---------------------------------------------------------------- lessons


def accepted_lessons(ledger: Ledger, limit: int | None = None) -> list[dict[str, Any]]:
    """Accepted lessons, newest first."""
    sql = ("SELECT * FROM lessons WHERE status = 'accepted'"
           " ORDER BY COALESCE(decided_at, created_at) DESC, id DESC")
    if limit:
        sql += f" LIMIT {int(limit)}"
    return ledger.all(sql)


def lesson_line(lesson: dict[str, Any]) -> str:
    """`- <text>（来自 <source>，<YYYY-MM-DD>）`."""
    date = (lesson.get("decided_at") or lesson["created_at"])[:10]
    origin = f"来自 {lesson['source_id']}，" if lesson.get("source_id") else ""
    return f"- {lesson['text']}（{origin}{date}）"


# ---------------------------------------------------------------- block


def render_block(lessons: list[dict[str, Any]]) -> str:
    """The whole managed block (markers included, trailing newline); enforces the line cap."""
    lines = [BEGIN, *RULES.splitlines(), "", LESSONS_HEADING]
    lines += [lesson_line(l) for l in lessons] or ["- 暂无"]
    lines.append(END)
    if len(lines) > MAX_BLOCK_LINES:
        raise ArbError(
            f"the CLAUDE.md block would have {len(lines)} lines (max {MAX_BLOCK_LINES}); "
            "retire or merge some lessons first (`arb lesson retire ID`)"
        )
    return "\n".join(lines) + "\n"


def _line_spans(text: str) -> list[tuple[int, int, str]]:
    """(start, end-including-terminator, stripped content) for each line."""
    spans = []
    pos = 0
    for line in text.splitlines(keepends=True):
        spans.append((pos, pos + len(line), line.strip()))
        pos += len(line)
    return spans


def find_block(text: str, where: str = FILENAME) -> tuple[int, int] | None:
    """Byte range [start, end) of the managed block, None if absent; ArbError if malformed."""
    spans = _line_spans(text)
    begins = [s for s in spans if s[2].startswith(BEGIN_PREFIX)]
    ends = [s for s in spans if s[2].startswith(END_PREFIX)]
    if not begins and not ends:
        return None
    hint = (f"; fix the markers in {where} by hand (keep exactly one `{BEGIN_PREFIX} … -->` "
            f"line followed by one `{END}` line, or delete both), then run `arb claudemd sync`")
    if len(begins) > 1 or len(ends) > 1:
        raise ArbError(f"{where} has {len(begins)} arboria:begin and {len(ends)} arboria:end "
                       f"markers (expected one of each){hint}")
    if not begins:
        raise ArbError(f"{where} has an arboria:end marker without arboria:begin{hint}")
    if not ends:
        raise ArbError(f"{where} has an arboria:begin marker without arboria:end{hint}")
    if ends[0][0] < begins[0][0]:
        raise ArbError(f"{where} has arboria:end before arboria:begin{hint}")
    return begins[0][0], ends[0][1]


def apply_block(text: str | None, block: str, where: str = FILENAME) -> tuple[str, str]:
    """New file text and the action taken: created | replaced | appended."""
    if text is None:
        return block, "created"
    span = find_block(text, where)
    if span is not None:
        start, end = span
        return text[:start] + block + text[end:], "replaced"
    if not text:
        return block, "appended"
    sep = "" if text.endswith("\n\n") else "\n" if text.endswith("\n") else "\n\n"
    return text + sep + block, "appended"


# ---------------------------------------------------------------- config / git


def autocommit_enabled(ctx: RepoContext) -> bool:
    """`[claudemd] autocommit` from arboria.toml (default true)."""
    path = ctx.config_path
    if not path.is_file():
        return True
    try:
        cfg = tomllib.loads(path.read_text(encoding="utf-8"))
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as exc:
        raise ArbError(f"{path} is not valid TOML ({exc}); fix it first") from exc
    section = cfg.get("claudemd", {})
    value = section.get("autocommit", True) if isinstance(section, dict) else True
    if not isinstance(value, bool):
        raise ArbError(f"{path}: [claudemd] autocommit must be true or false")
    return value


def _user_identity_configured(cwd: Path) -> bool:
    """True if git has an explicit author and committer identity (no auto-detection)."""
    def cfg(key: str) -> bool:
        return bool(gitutil.run(["config", "--get", key], cwd, check=False).stdout)

    for role in ("author", "committer"):
        env = role.upper()
        name = os.environ.get(f"GIT_{env}_NAME") or cfg(f"{role}.name") or cfg("user.name")
        email = (os.environ.get(f"GIT_{env}_EMAIL") or cfg(f"{role}.email") or cfg("user.email")
                 or os.environ.get("EMAIL"))
        if not (name and email):
            return False
    return True


def _operation_in_progress(main: Path) -> str | None:
    markers = {
        "MERGE_HEAD": "a merge", "CHERRY_PICK_HEAD": "a cherry-pick", "REVERT_HEAD": "a revert",
        "rebase-merge": "a rebase", "rebase-apply": "a rebase or am",
    }
    for name, what in markers.items():
        path = Path(gitutil.out(["rev-parse", "--path-format=absolute", "--git-path", name], main))
        if path.exists():
            return what
    return None


def _commit_skip_reason(main: Path) -> str | None:
    op = _operation_in_progress(main)
    if op:
        return f"the main worktree is in the middle of {op}"
    if gitutil.out(["ls-files", "-u", "--", FILENAME], main):
        return f"{FILENAME} has merge conflicts"
    if not gitutil.ok(["symbolic-ref", "--quiet", "HEAD"], main):
        return "the main worktree is on a detached HEAD"
    if gitutil.ok(["check-ignore", "-q", "--", FILENAME], main):
        return f"{FILENAME} is ignored by git"
    if not _user_identity_configured(main):
        return "no git identity is configured (set user.name and user.email)"
    return None


def commit_claudemd(main: Path) -> tuple[bool, str | None]:
    """Commit only CLAUDE.md in the main worktree. Returns (committed, hint)."""
    reason = _commit_skip_reason(main)
    if reason:
        return False, f"{reason}; commit {FILENAME} yourself later"
    has_head = gitutil.rev_parse("HEAD", main) is not None
    tracked = gitutil.ok(["ls-files", "--error-unmatch", "--", FILENAME], main)
    if tracked and has_head and gitutil.ok(["diff", "--quiet", "HEAD", "--", FILENAME], main):
        return False, None  # already identical to HEAD's version
    if not tracked:
        gitutil.run(["add", "--", FILENAME], main)
    proc = gitutil.run(["commit", "--only", "-q", "-m", COMMIT_MESSAGE, "--", FILENAME],
                       main, check=False)
    if proc.returncode != 0:
        if not tracked:
            gitutil.run(["rm", "--cached", "-q", "--", FILENAME], main, check=False)
        detail = (proc.stderr or proc.stdout).strip().splitlines()
        return False, (f"git commit of {FILENAME} failed ({detail[-1] if detail else 'no output'}); "
                       f"commit it yourself")
    return True, None


# ---------------------------------------------------------------- sync


@dataclass
class SyncResult:
    path: Path
    action: str | None  # created | replaced | appended | None (unchanged)
    committed: bool = False
    hints: list[str] = field(default_factory=list)

    @property
    def changed(self) -> bool:
        return self.action is not None

    def messages(self) -> list[str]:
        if not self.changed:
            return [f"{self.path} already up to date"]
        out = [f"{self.action} arboria block in {self.path}"]
        if self.committed:
            out.append(f"committed {FILENAME} in the main worktree")
        out += [f"hint: {h}" for h in self.hints]
        return out


def plan(ctx: RepoContext, lessons: list[dict[str, Any]]) -> tuple[Path, bytes | None, bytes, str]:
    """(path, old bytes, new bytes, action) without writing anything; raises on problems."""
    path = ctx.main / FILENAME
    old = path.read_bytes() if path.exists() else None
    text = None if old is None else old.decode("utf-8", errors="surrogateescape")
    new_text, action = apply_block(text, render_block(lessons), str(path))
    return path, old, new_text.encode("utf-8", errors="surrogateescape"), action


def check(ctx: RepoContext, lessons: list[dict[str, Any]]) -> None:
    """Raise ArbError if syncing with `lessons` would be refused (markers, line cap, config)."""
    plan(ctx, lessons)
    autocommit_enabled(ctx)


def sync(ctx: RepoContext, ledger: Ledger, *, commit: bool = True) -> SyncResult:
    """Write the block into <main>/CLAUDE.md and (optionally) commit just that file."""
    path, old, new, action = plan(ctx, accepted_lessons(ledger))
    if old == new:
        return SyncResult(path, None)
    autocommit = autocommit_enabled(ctx)
    tmp = path.with_name(f".{FILENAME}.arboria-tmp")
    tmp.write_bytes(new)
    if path.exists():
        os.chmod(tmp, path.stat().st_mode & 0o7777)
    os.replace(tmp, path)
    result = SyncResult(path, action)
    if not commit:
        return result
    if not autocommit:
        result.hints.append(f"[claudemd] autocommit is off; commit {FILENAME} yourself")
        return result
    result.committed, hint = commit_claudemd(ctx.main)
    if hint:
        result.hints.append(hint)
    return result


# ---------------------------------------------------------------- the seed home


def render_home_block() -> str:
    return "\n".join([BEGIN, *HOME_RULES.splitlines(), END]) + "\n"


def sync_home(root: Path) -> SyncResult:
    """Write the home block into <home>/CLAUDE.md (the rest of the file is kept)."""
    path = Path(root) / FILENAME
    old = path.read_bytes() if path.exists() else None
    text = None if old is None else old.decode("utf-8", errors="surrogateescape")
    new_text, action = apply_block(text, render_home_block(), str(path))
    new = new_text.encode("utf-8", errors="surrogateescape")
    if old == new:
        return SyncResult(path, None)
    tmp = path.with_name(f".{FILENAME}.arboria-tmp")
    tmp.write_bytes(new)
    if path.exists():
        os.chmod(tmp, path.stat().st_mode & 0o7777)
    os.replace(tmp, path)
    return SyncResult(path, action)
