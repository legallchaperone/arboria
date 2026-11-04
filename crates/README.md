# Rust crates

The Rust core of Arboria (ADR 0001) — phase 1: the public API is settled, most function
bodies are `todo!()`. Contract, module map, schema v8 and porting rules:
[`docs/adr/0002-rust-contract.md`](../docs/adr/0002-rust-contract.md).

- `arboria-core/` — the library (ledger, ideas, runs, approvals, machines, probes). One
  file per module; each file's header names the Python module it ports and the
  invariants it keeps.
- `arb/` — the command line, built as `arb-rs` until it replaces the Python `arb`. Its
  clap tree mirrors `src/arboria/cli.py`.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # 51 vector tests are #[ignore]d until ported
```

Snapshots generated from the Python side (regenerate when it changes; CI checks):

```sh
uv run python crates/arb/tests/gen_python_commands.py              # CLI tree
uv run python crates/arboria-core/tests/fixtures/gen_fixtures.py   # schema v1–v7, a v7 ledger
uv run python scripts/export_vectors.py                            # tests/vectors/
```

The Python package (`src/arboria/`) stays the working `arb` until the Rust one catches up.
