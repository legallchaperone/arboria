"""`arb --help` reads in groups: every visible top-level command sits in one panel."""

from __future__ import annotations

import typer
from typer.testing import CliRunner

from arboria import cli


def test_every_visible_command_is_in_exactly_one_panel() -> None:
    group = typer.main.get_command(cli.app)
    visible = {n for n, c in group.commands.items() if not c.hidden}
    listed = [n for names in cli.HELP_PANELS.values() for n in names]
    assert len(listed) == len(set(listed)), "a command is in two panels"
    assert set(listed) == visible
    for panel, names in cli.HELP_PANELS.items():
        assert "Claude" not in panel
        for n in names:
            assert group.commands[n].rich_help_panel == panel, n


def test_help_shows_the_panels_in_order() -> None:
    out = CliRunner().invoke(cli.app, ["--help"], env={"COLUMNS": "120"}).stdout
    at = [out.index(f"─ {panel} ") for panel in cli.HELP_PANELS]
    assert at == sorted(at)
    assert "─ Commands " not in out  # nothing left outside a panel
    assert " seed " not in out  # seeds live under `arb idea`; `arb seed` is a hidden alias


def test_run_new_is_hidden_but_still_there() -> None:
    runner = CliRunner()
    out = runner.invoke(cli.app, ["run", "--help"], env={"COLUMNS": "120"}).stdout
    assert "summary" in out and " new " not in out  # superseded by `arb submit`
    assert runner.invoke(cli.app, ["run", "new", "--help"]).exit_code == 0
