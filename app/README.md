# Arboria.app

The window over the Rust core (ADR 0004, `docs/adr/0004-app-shell.md`): the inbox, the
overview (idea timeline, spend, machines, the background helper), each idea with its
runs and notes, the approval card signed with Touch ID, an embedded terminal (shell or
Claude Code), and the idea's marimo notebook in its own window.

```bash
npm ci                 # once
npm run tauri dev      # the app, against the real ledgers (arb init / arb projects add)
npm run tauri build    # Arboria.app and a .dmg under src-tauri/target/release/bundle
```

Needs Rust (rustup), Node 22 and, for notebooks, the Python package (`uv tool install
'.[ui]'` in the repo root puts `arb-py` on PATH). The background helper runs the `arb`
CLI: `cargo install --locked --path crates/arb`, then 全局 › 安装小助手 (or `arb helper
install`). Building on Linux needs webkit2gtk 4.1 (`libwebkit2gtk-4.1-dev`).

Checks: `npm run check` (TypeScript, also against the fixture), `npm test` (vitest),
`npm run screens` (screenshots over `src/fixtures/board.json` into `screens/`, with the
Playwright installed on the machine), `cargo clippy` / `cargo test` in `src-tauri/`.
The fixture is written by `python3 scripts/make_fixture.py`; the core's test
`cargo test -p arboria-core --test view` checks its shape. Dependencies are pinned to
releases at least two weeks old: npm with `npm install --before=<date>`, Rust with
`python3 ../scripts/check_crate_dates.py src-tauri/Cargo.lock`.
