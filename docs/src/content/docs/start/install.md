---
title: 安装
description: 用 uv 安装 arb 和 marimo，以及 notebook 打开时自动运行的设置。
---

需要 Python ≥ 3.11、git ≥ 2.31 和 [uv](https://docs.astral.sh/uv/)。

```bash
git clone https://github.com/legallchaperone/arboria.git
cd arboria
uv tool install --force '.[ui]'   # arb 出现在 PATH 里；[ui] 带上 marimo
arb --help
```

- `[ui]` 是 marimo：idea notebook 和看板（`arb ui`）都要它。只要命令行时 `uv tool install .` 也可以，之后要看板再运行上面那条。
- 看板会 import arboria，所以 marimo 必须装在 **arb 自己的环境里**。单独 `uv tool install marimo` 只够打开 notebook，不够跑 `arb ui`。
- 在 notebook 里读 wandb 曲线还需要 `wandb`（有 `pandas` 时返回 DataFrame）；装了 PyYAML 时，拉回的 `config.yaml` 会被解析成 JSON。
- 更新：在源码目录 `git pull`，再运行一次 `uv tool install --force '.[ui]'`。

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
