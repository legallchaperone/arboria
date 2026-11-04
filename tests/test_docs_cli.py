"""The docs site's CLI reference is generated from the Typer app and must stay current."""

from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "docs" / "scripts" / "gen_cli.py"


def load_generator():
    spec = importlib.util.spec_from_file_location("gen_cli", SCRIPT)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def test_cli_reference_is_up_to_date() -> None:
    gen = load_generator()
    assert gen.PAGE.read_text(encoding="utf-8") == gen.render(), (
        "docs/src/content/docs/reference/cli.md is stale: "
        "run `uv run python docs/scripts/gen_cli.py`")


def test_cli_reference_lists_every_command() -> None:
    gen = load_generator()
    text = gen.PAGE.read_text(encoding="utf-8")
    import typer

    from arboria.cli import app

    for path in gen.walk(typer.main.get_command(app)):
        assert f"### arb {path}\n" in text, path
    assert "## 其他" not in text, "a new command needs a section in docs/scripts/gen_cli.py"
