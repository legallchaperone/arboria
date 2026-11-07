"""Generate the CLI reference page of the docs site from the real Typer app.

    uv run python docs/scripts/gen_cli.py           # rewrite the page
    uv run python docs/scripts/gen_cli.py --check   # exit 1 if the committed page is stale

The page is docs/src/content/docs/reference/cli.md. It only reads the click command
tree (names, help, parameters); it never runs a command.
"""

from __future__ import annotations

import argparse
import html
import inspect
import re
import sys
from pathlib import Path
from typing import Any

import typer

from arboria.cli import app

ROOT = Path(__file__).resolve().parents[2]
PAGE = ROOT / "docs" / "src" / "content" / "docs" / "reference" / "cli.md"

# (section title, intro, command paths). Commands not listed land in "其他".
SECTIONS: list[tuple[str, str, list[str]]] = [
    ("idea 与种子", "`arb idea` 管一个 idea 的两个阶段：在初始化过的仓库里是 idea（分支 + "
     "worktree）；在仓库外（或种子园 `~/arboria` 里）是种子，也就是还没有仓库的 idea。"
     "在没有 `arb init` 的仓库里会报错，不会悄悄记成种子。`arb idea plant` 在目标仓库里运行，"
     "把种子种成 idea。旧写法 `arb seed …` 仍然可用，是同一组种子命令的别名。", [
        "idea new", "idea list", "idea show", "idea set", "idea drop", "idea revive",
        "idea plant", "idea notebook", "idea style", "idea close",
    ]),
    ("项目、run 与笔记", "在实验代码仓库里：初始化、run 的结果、笔记和概览。", [
        "init", "run summary", "note", "status", "export",
    ]),
    ("提交、批准与集群", "`submit` 只在本地生成审核单；只有 `approve` 会把作业交给 Slurm。", [
        "submit", "card", "approve", "reject", "cancel", "poll", "watch", "logs", "remote ls",
    ]),
    ("Touch ID 签字密钥", "Mac app 用 Secure Enclave 里的密钥给审核单签字；只有你在自己的终端里信任过的"
     "密钥签的字才算批准。", ["key trust", "key list", "key revoke"]),
    ("路线图（里程碑）", "你定的里程碑：截止日期、算力预算、包含的 idea 和清单。改动只由你在自己的终端里做，"
     "`list` / `show` 谁都能看。", [
        "milestone list", "milestone show", "milestone new", "milestone set", "milestone check",
        "milestone uncheck", "milestone done", "milestone reopen", "milestone drop",
    ]),
    ("待办与 notebook 命令", "notebook 里 `lab.todo(...)`、`lab.<名字>(...)` 和 "
     "`lab.inspect(...)` 留下的活。", [
        "todo list", "todo done", "todo drop", "commands list", "commands guide",
        "commands promote", "inspect guide", "inspect source", "inspect check", "inspect done",
    ]),
    ("看板", "", ["ui"]),
    ("机器", "机器配置在 `~/.config/arboria/clusters.toml`。", ["machine init", "doctor"]),
    ("Claude Code 与经验", "", [
        "install-claude", "hook session-start", "claudemd sync",
        "lesson propose", "lesson list", "lesson accept", "lesson reject", "lesson retire",
    ]),
]

HEADER = """\
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
"""


# Typer vendors its own click, so the command tree is duck-typed here.
Command = Any


def walk(group: Command, prefix: str = "") -> dict[str, Command]:
    out: dict[str, Command] = {}
    for name, cmd in group.commands.items():
        path = f"{prefix}{name}"
        if getattr(cmd, "hidden", False):  # aliases and superseded commands
            continue
        if hasattr(cmd, "commands"):
            out.update(walk(cmd, path + " "))
        else:
            out[path] = cmd
    return out


def md_text(text: str) -> str:
    """Escape prose for a Markdown table cell or paragraph, keeping `code` spans."""
    parts = re.split(r"(`[^`]*`)", text)
    out = []
    for part in parts:
        if part.startswith("`") and part.endswith("`") and len(part) > 1:
            out.append(part.replace("|", "\\|"))
        else:
            part = html.escape(part, quote=False)
            part = re.sub(r"([|*_{}\\])", r"\\\1", part)
            out.append(part)
    return "".join(out)


def help_text(cmd: Command) -> str:
    raw = inspect.cleandoc(cmd.help or "")
    paras = [" ".join(p.split()) for p in raw.split("\n\n") if p.strip()]
    return "\n\n".join(paras)


def usage(path: str, cmd: Command) -> str:
    ctx = typer.Context(cmd, info_name=f"arb {path}")
    pieces = cmd.collect_usage_pieces(ctx)
    return " ".join([f"arb {path}", *pieces])


def param_row(cmd: Command, p: Any) -> str | None:
    ctx = typer.Context(cmd, info_name=cmd.name)
    if p.param_type_name == "option":
        if "--help" in p.opts:
            return None
        names = ", ".join(p.opts)
        if not p.is_flag:
            names += f" {p.make_metavar(ctx)}"
        if p.multiple:
            names += " …"
        what = getattr(p, "help", None) or ""
        if p.required:
            default = "必填"
        elif p.is_flag or p.default is None or p.default == () or p.default is False:
            default = ""
        else:
            default = f"`{p.default}`"
    else:
        metavar = p.make_metavar(ctx)
        names = metavar
        what = getattr(p, "help", None) or ""
        default = "必填" if p.required else ""
    return f"| `{names}` | {md_text(what)} | {default} |"


def render() -> str:
    command = typer.main.get_command(app)
    commands = walk(command)
    listed = [c for _, _, cmds in SECTIONS for c in cmds]
    sections = [(t, i, [c for c in cmds if c in commands]) for t, i, cmds in SECTIONS]
    rest = [c for c in commands if c not in listed]
    if rest:
        sections.append(("其他", "", rest))

    out = [HEADER]
    out.append("## 一览\n")
    out.append("| 命令 | 作用 |\n| --- | --- |")
    for _, _, cmds in sections:
        for path in cmds:
            first = help_text(commands[path]).split("\n\n")[0]
            out.append(f"| [`arb {path}`](#{anchor(path)}) | {md_text(first)} |")
    out.append("")

    for title, intro, cmds in sections:
        if not cmds:
            continue
        out.append(f"## {title}\n")
        if intro:
            out.append(intro + "\n")
        for path in cmds:
            cmd = commands[path]
            text = help_text(cmd)
            out.append(f"### arb {path}\n")
            if text.startswith("(Human only"):
                out.append('<span class="arb-tag">只由你</span>\n')
            out.append(md_text(text) + "\n")
            out.append(f"```text\n{usage(path, cmd)}\n```\n")
            rows = [r for r in (param_row(cmd, p) for p in cmd.params) if r]
            if rows:
                out.append("| 参数 | 说明 | 默认 |\n| --- | --- | --- |")
                out.extend(rows)
                out.append("")
    return "\n".join(out).rstrip("\n") + "\n"


def anchor(path: str) -> str:
    """The id Starlight gives the heading `arb <path>` (github-slugger rules)."""
    return "arb-" + re.sub(r"[^a-z0-9 -]", "", path.lower()).replace(" ", "-")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--check", action="store_true", help="fail if the page is stale")
    args = parser.parse_args()
    text = render()
    if args.check:
        current = PAGE.read_text(encoding="utf-8") if PAGE.exists() else ""
        if current != text:
            print(f"{PAGE.relative_to(ROOT)} is stale: run `uv run python docs/scripts/gen_cli.py`",
                  file=sys.stderr)
            return 1
        print(f"{PAGE.relative_to(ROOT)} is up to date")
        return 0
    PAGE.parent.mkdir(parents=True, exist_ok=True)
    PAGE.write_text(text, encoding="utf-8")
    print(f"wrote {PAGE.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
