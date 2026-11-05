---
title: 安装
description: 安装 arb（Rust）和 Python 包（marimo、notebook 里的 lab），以及 notebook 打开时自动运行的设置。
---

需要 Python ≥ 3.11、git ≥ 2.31、[uv](https://docs.astral.sh/uv/) 和 [Rust](https://rustup.rs/)（装 rustup 即可，版本由仓库里的 `rust-toolchain.toml` 定）。

```bash
git clone https://github.com/legallchaperone/arboria.git
cd arboria
uv tool install --force '.[ui]'            # Python 包：arb-py 出现在 PATH 里；[ui] 带上 marimo
cargo install --locked --path crates/arb   # arb（Rust）出现在 ~/.cargo/bin
arb --help
```

- `arb` 是 Rust 写的命令行。notebook 模板、看板和 notebook 里的 `lab` 仍在 Python 包里：`arb` 用 `arb-py` 的解释器运行它们（要换就设 `ARBORIA_PYTHON`），所以两个都要装。
- `[ui]` 是 marimo：idea notebook 和看板（`arb ui`）都要它。看板会 import arboria，所以 marimo 必须装在 **arb-py 的环境里**。单独 `uv tool install marimo` 只够打开 notebook，不够跑 `arb ui`。
- 账本是 schema v8：`arb` 第一次打开旧账本时原地升级。之后 Python 版（`arb-py`、notebook 里的 `lab`）只读它，`lab` 的写入交给 `arb`（找 `ARBORIA_ARB`，否则 PATH 上的 `arb`）。
- 在 notebook 里读 wandb 曲线还需要 `wandb`（有 `pandas` 时返回 DataFrame）；装了 PyYAML 时，拉回的 `config.yaml` 会被解析成 JSON。
- Mac app（收件箱、idea 树、触控 ID 签字、后台小助手）另外构建：见 [Mac app](/guides/app/)。
- 更新：在源码目录 `git pull`，再运行上面两条安装命令。

## 让 notebook 一打开就运行

marimo 默认打开 notebook 时**不运行** cell。想一打开就看到 `lab.runs()`、run 的状态和图，在 `~/.config/marimo/marimo.toml`（有 `$XDG_CONFIG_HOME` 时在它下面）里写：

```toml
[runtime]
auto_instantiate = true
```

这是你自己的设置，arb 不会替你改；notebook 文件里的同名设置 marimo 出于安全会忽略。打开就重跑是安全的：`lab.run` 遇到相同的审核单直接返回已有的 run，不会再建一个。

## 打开 notebook 的两种方式

- VS Code 装 marimo 扩展，直接打开 `notebook.py`，旁边就是 Claude Code。
- 终端里 `marimo edit notebook.py`，在浏览器里编辑同一个文件。

下一步：[五分钟上手](/start/quickstart/)。
