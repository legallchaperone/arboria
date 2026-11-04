"""Cross-language test vectors (tests/vectors/*.json) are exactly what the Python code produces.

`scripts/export_vectors.py` runs the real functions; this test regenerates every file in
memory and compares it byte for byte with the committed one, so the vectors the Rust port
is tested against can never drift from Python. After an intended behaviour change:
`uv run python scripts/export_vectors.py`, review the diff, commit it.
"""

from __future__ import annotations

import importlib.util
import json
import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
VECTORS = ROOT / "tests" / "vectors"
REGEN = "regenerate with `uv run python scripts/export_vectors.py` and review the diff"


def _exporter():
    spec = importlib.util.spec_from_file_location("export_vectors",
                                                  ROOT / "scripts" / "export_vectors.py")
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def generated() -> dict[str, str]:
    return _exporter().generate()


def test_committed_vectors_match_python(generated: dict[str, str]) -> None:
    committed = {p.name: p.read_text(encoding="utf-8") for p in VECTORS.glob("*.json")}
    assert sorted(committed) == sorted(generated), REGEN
    stale = sorted(n for n, text in generated.items() if committed[n] != text)
    assert not stale, f"out of date: {', '.join(stale)}; {REGEN}"


def test_vector_file_format(generated: dict[str, str]) -> None:
    for fname, text in generated.items():
        doc = json.loads(text)
        assert set(doc) == {"name", "version", "source", "notes", "cases"}, fname
        assert fname == f"{doc['name']}.json" and doc["version"] == 1
        assert doc["source"] and doc["notes"] and doc["cases"], fname
        assert text == json.dumps(doc, sort_keys=True, ensure_ascii=False, indent=2) + "\n"
        ids = [c["id"] for c in doc["cases"]]
        assert len(ids) == len(set(ids)), f"{fname}: duplicate case ids"
        for case in doc["cases"]:
            assert isinstance(case["input"], dict), (fname, case["id"])
            assert set(case) in ({"id", "input", "output"}, {"id", "input", "error"}), \
                (fname, case["id"])
            if "error" in case:
                assert re.match(r"^[A-Za-z]+: ", case["error"]), (fname, case["id"])
                # an "err-" id documents a refusal; anything else is meant to succeed
                assert case["id"].startswith("err-"), (fname, case["id"], case["error"])
            else:
                assert not case["id"].startswith("err-"), (fname, case["id"])


def test_readme_lists_every_vector_file(generated: dict[str, str]) -> None:
    readme = (VECTORS / "README.md").read_text(encoding="utf-8")
    missing = [n for n in generated if f"`{n.removesuffix('.json')}`" not in readme]
    assert not missing, f"tests/vectors/README.md does not list {', '.join(missing)}"
