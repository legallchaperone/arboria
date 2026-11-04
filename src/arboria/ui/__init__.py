"""`arb ui`: a read-only marimo dashboard (看板) over the ledger.

- `data`: opens the ledger read-only (`file:...?mode=ro`) and gathers everything the
  dashboard shows into one `Board`; no writes, no network, no subprocesses.
- `views`: renders a `Board` as HTML / inline SVG (no marimo needed, unit-tested).
- `dashboard.py`: the marimo app (`marimo run`), a thin layer over the two.
- `garden.py`: the same for the seed home (种子园, `arb ui --home`): `data.load_home`
  and the `garden_*` views.

Nothing here may import a write path of arboria (records, submission, monitor, tasks,
lessons, ideas, db's Ledger, backends); `tests/test_ui.py` scans the imports.
"""

from __future__ import annotations

from pathlib import Path

APP = Path(__file__).with_name("dashboard.py")
GARDEN = Path(__file__).with_name("garden.py")
