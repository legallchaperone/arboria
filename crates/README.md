# Rust crates

The Rust core of Arboria (ADR 0001) — phase 1: every module is ported; `arb` prints
what the Python CLI (now `arb-py`) prints (`crates/arb/tests/cli_parity.rs`). Contract, module map, schema v8 and porting rules:
[`docs/adr/0002-rust-contract.md`](../docs/adr/0002-rust-contract.md).

- `arboria-core/` — the library (ledger, ideas, runs, approvals, machines, probes). One
  file per module; each file's header names the Python module it ports and the
  invariants it keeps.
- `arb/` — the command line `arb` (`cargo install --locked --path crates/arb`). Its clap
  tree mirrors `src/arboria/cli.py`; `arb lab` (hidden) takes the notebook kernel's writes.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # all 60 vector files; Python comparisons skip without python3
```

Snapshots generated from the Python side (regenerate when it changes; CI checks):

```sh
uv run python crates/arb/tests/gen_python_commands.py              # CLI tree
uv run python crates/arboria-core/tests/fixtures/gen_fixtures.py   # schema v1–v7, a v7 ledger
uv run python scripts/export_vectors.py                            # tests/vectors/
```

The Python package (`src/arboria/`) is still needed: the notebook kernel (`lab`), the
marimo notebooks and the dashboard run there, in the interpreter of `arb-py` (or
`$ARBORIA_PYTHON`). It reads the v8 ledger the Rust `arb` writes, and never writes it.
