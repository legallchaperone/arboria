"""Fake `lab` for the spike: outputs use Arboria's structured mime types, never HTML."""
from IPython.display import display

def _out(kind, payload):
    display({f"application/vnd.arboria.{kind}+json": payload, "text/plain": f"<arboria {kind}>"}, raw=True)

def runs():
    _out("table", {"columns": ["run", "状态", "val/loss"], "rows": [["r011", "完成", 2.915], ["r012", "完成", 2.879], ["r013", "运行中", 2.860]]})

def plot(*runs, key="val/loss", n=200_000):
    import math
    series = [{"id": r, "x": list(range(n)), "y": [3 + math.exp(-i / 2000) for i in range(n)]} for r in runs]
    _out("chart", {"kind": "line", "metric": key, "series": series})
