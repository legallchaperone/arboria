import marimo

__generated_with = "0.25.2"
app = marimo.App(width="full", app_title="Arboria 种子园", css_file="dashboard.css")


@app.cell
def _():
    # Arboria 种子园: read-only, like the 看板. Started by `arb ui --home` (marimo run ...
    # -- --home ROOT). Reads the home's ledger (mode=ro) and its notebooks' mtimes; nothing
    # here writes. Commands are shown as text for the human's own terminal.
    import os

    import marimo as mo

    from arboria.errors import ArbError
    from arboria.paths import home_root
    from arboria.ui import data, views

    return ArbError, data, home_root, mo, os, views


@app.cell
def _(home_root, mo, os):
    root = mo.cli_args().get("home") or os.environ.get("ARBORIA_UI_HOME") or str(home_root())
    refresh = mo.ui.refresh(options=["30s", "1m", "5m"], default_interval="30s",
                            label="自动刷新")
    return refresh, root


@app.cell
def _(ArbError, data, mo, refresh, root):
    refresh  # re-read on every tick
    try:
        garden = data.load_home(root)
        load_error = None
    except (ArbError, OSError) as _exc:
        garden, load_error = None, str(_exc)
    mo.stop(garden is None, mo.callout(mo.md(f"**种子园读不到账本**：{load_error}"),
                                       kind="danger"))
    return (garden,)


@app.cell
def _(garden, mo, refresh, views):
    mo.Html(views.garden_header(garden, controls=refresh.text))
    return


@app.cell
def _(garden, mo, views):
    mo.Html(views.garden_body(garden))
    return


if __name__ == "__main__":
    app.run()
