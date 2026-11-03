import marimo

__generated_with = "0.25.2"
app = marimo.App(width="full", app_title="Arboria 看板", css_file="dashboard.css")


@app.cell
def _():
    # Arboria 看板: read-only. Started by `arb ui` (marimo run ... -- --root MAIN [--idea NAME]).
    # Everything is read from the ledger (opened with mode=ro) and a few local files; nothing
    # here writes, submits, approves or talks to the cluster. Commands are shown as text for
    # the human to run in their own terminal.
    import os

    import marimo as mo

    from arboria.errors import ArbError
    from arboria.ui import data, views

    return ArbError, data, mo, os, views


@app.cell
def _(mo, os):
    _args = mo.cli_args()
    root = _args.get("root") or os.environ.get("ARBORIA_UI_ROOT")
    requested_idea = _args.get("idea") or os.environ.get("ARBORIA_UI_IDEA") or None
    if not root:
        from arboria.paths import locate  # `marimo run` by hand from inside the repo

        root = str(locate().main)
    refresh = mo.ui.refresh(options=["30s", "1m", "5m"], default_interval="30s",
                            label="自动刷新")
    show_dropped = mo.ui.switch(value=True, label="显示已放弃")
    get_selected, set_selected = mo.state(requested_idea)
    return get_selected, refresh, requested_idea, root, set_selected, show_dropped


@app.cell
def _(ArbError, data, mo, refresh, root):
    refresh  # re-read the ledger on every tick
    try:
        board = data.load(root)
        load_error = None
    except (ArbError, OSError) as _exc:
        board, load_error = None, str(_exc)
    mo.stop(board is None, mo.callout(mo.md(f"**看板读不到账本**：{load_error}"), kind="danger"))
    return (board,)


@app.cell
def _(board, mo, refresh, views):
    mo.Html(views.header(board, controls=refresh.text))
    return


@app.cell
def _(board, get_selected, mo, requested_idea, set_selected, views):
    _options = views.idea_options(board)
    _name = views.default_idea(board, get_selected() or requested_idea)
    _label = next((k for k, v in _options.items() if v == _name), None)
    picker = mo.ui.dropdown(options=_options, value=_label,
                            on_change=set_selected)
    return (picker,)


@app.cell
def _(board, mo, picker, show_dropped, views):
    # inbox on the left; the idea tree and the selected idea's detail on the right
    mo.Html(views.columns(
        views.inbox(board),
        views.tree(board, show_dropped.value, picker.value, controls=show_dropped.text)
        + views.idea_detail(board, picker.value, controls=picker.text)))
    return


@app.cell
def _(board, mo, views):
    mo.Html(views.budget(board))
    return


if __name__ == "__main__":
    app.run()
