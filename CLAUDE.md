# Arboria：给开发这个工具的 Claude 看的说明

Arboria（命令 `arb`）把 ML 研究的状态从 Claude 对话里搬出来：idea、run、note、decision 都存在 SQLite 账本里，只有 `arb` 能写。设计说明（中文网页）：https://claude.ai/artifact/NCqpKzbhDfAXSURgGTc1zn

## 不能破坏的约束
- LLM 能写的字段只有：hypothesis、metric、change_reason、note、run summary、lesson、todo result（≤600）、读代码答案（`arb inspect done`：title ≤80、summary 和段说明 ≤600、行批注 ≤200（≤40 条）、伪代码/简化代码 ≤8000、Mermaid ≤8000，`inspection.validate_result` 校验，显示一律 HTML 转义；源码不由 LLM 写，由 arb 从 git 取），以及以后的 review。（note 在 Claude Code 里默认拒绝，只由人写。）其余字段全部由代码写入并校验。长度校验不能删。种子（gist ≤300、hypothesis、metric）只由人写：所有写种子的命令（作用于种子时的 `arb idea new/set/drop/revive`、`arb idea plant`，以及别名 `arb seed …`）和 `lab.metric`/`lab.todo` 在 Claude Code 里拒绝。这一条靠代码（`seeds.require_human`），不能只靠前缀 deny 规则：`arb idea new/drop` 在仓库里是 Claude 可以用的。
- 账本写入一律走 `db` 里的加锁写入（fcntl + `BEGIN IMMEDIATE`）。备份 commit 只在指定的事件类型后触发，心跳类更新不触发。
- 快照绝不能动用户的分支、index 和工作区。`tests/test_snapshot.py` 就是这条约束的契约。
- `arb hook session-start` 绝不能失败，也不能输出多余内容：出任何错都 exit 0、不输出。
- 任何东西提交到 Slurm 之前，都必须有人类批准过的 Decision。不要写任何自动批准的代码路径。`arb approve` 和 `arb lesson accept` 只能在人自己的终端里做（`human.require_terminal`：有 `CLAUDECODE`/`CLAUDE_CODE_ENTRYPOINT` 或 stdin 不是 TTY 就拒绝，然后要人输入 id），install-claude 把它们设为 deny。`lab` 里没有任何批准函数。
- 1a 的代码路径里不发网络请求。远端操作放在后端接口后面。
- 看板（`arb ui`）只读：账本用 `file:…?mode=ro` 打开；`ui/data.py`、`views.py`、`dashboard.py`、`garden.py` 只能 import cards/config/errors/paths/slurm，不起进程、不联网；不加会动手的控件（批准、接受经验、取消只显示成命令文本）。`tests/test_ui.py` 用源码扫描和 spy（写入函数、后端、subprocess、socket）守住。ledger 里的文字一律 HTML 转义。
- notebook 命令（`commands`）：arb 的命令行绝不执行命令代码（`arb commands list` 只用 `ast` 读）；只有人的 kernel 里 `lab.<名字>(...)` 才 import 并运行它。命令运行期间 `human.require_terminal` 拒绝（批准不能从 Claude 写的代码里发起）。命令待办的 kind/payload 由代码写，文字长度照常校验。
- 种子园（`seeds`、`paths.Home`，默认 `~/arboria`，`ARBORIA_HOME`）：不是 git 仓库，`.arboria/` 复用项目的 Ledger 和备份仓库（同一个 schema）。种子 notebook 是 `seeds/<名字>.py` 普通文件，备份提交前复制进 `.arboria/seeds/`（`db.mirror_seeds`；这个目录也是种子园的标记）。种子就是还没有仓库的 idea，命令都在 `arb idea` 下，去向固定（`cli._idea_route`）：在种子园里或不在任何 git 仓库里 → 种子；在没有 `arb init` 的仓库里 → 照旧报错，绝不悄悄记成种子；在初始化过的仓库里 → idea。`arb idea plant` 在目标仓库里运行。种下只在人的终端里（会建分支和 worktree），走 `ideas.new_idea`，只复制种子 notebook 的 markdown cell（用 `ast` 读，不执行）。`arb install-claude --home` 禁止 Claude 编辑 `seeds/**` 和 `.arboria/**`。测试绝不碰真的 `~/arboria`：`tests/conftest.py` 把 `ARBORIA_HOME` 指到临时目录。
- 推送到 Claude 会话（`sessions`）：只发给 SessionStart hook 记下的会话（`.arboria/sessions/*.json`，0600），绝不读进程环境里的 socket；不存不发 `CLAUDE_CODE_MESSAGING_TOKEN`（notebook 不是那个会话的子进程）；消息只由校验过的字段拼成；任何失败静默跳过。测试绝不能碰真会话：`tests/conftest.py` 清掉 `CLAUDE_CODE_MESSAGING_*` 并把 `ARBORIA_CONFIG_DIR` 指到临时目录，消息测试只用假 Unix socket。

## 代码结构
`src/arboria/`：cli（typer）、paths、gitutil、db、state（init）、snapshot、ideas、records、report（status/export）、hook、install、claudemd、clusters、config、slurm、backends/、submission、monitor、machine（machine init / doctor）、human（只在人的终端里批准）、notebooks、tasks（待办）、lab（notebook API）、commands（notebook 命令库、命令待办、给 Claude 的 guide）、inspection（`lab.inspect`：读代码待办、答案校验、从 git 取源码、渲染、guide）、analysis（内置分析 plot/diff/spend，只读）、seeds（种子园：种子的增改、种下）、sessions（会话 socket 的记录与推送）、style（字体：生成嵌入 Noto Sans SC 和 Maple Mono CN 的样式表）、cards（审核单，纯读；submission 再导出）、`ui/`（看板：data 只读加载账本、views 出 HTML/内联 SVG、dashboard.py 是包里自带的 marimo app、garden.py 是种子园的 marimo app，launch 起 `marimo run`）。`tests/` 使用临时 git 仓库；`tests/test_marimo.py` 用真的 marimo（dev 依赖）跑 `marimo check --strict`、`marimo export html` 和真 kernel 里的自动重跑（`tests/marimo_session.py`，用了 marimo 私有接口，接口变了就 skip）。

## 开发
- `uv run pytest` 必须保持全绿。用 `uv tool install .` 装好后试 `arb`。
- CI：`.github/workflows/tests.yml` 在 Python 3.11 和 3.13 上跑 `uv sync --all-extras --dev` + `uv run pytest`（包括真的 marimo）。
- 用户文档：`README.md` 只放概览和快速开始，完整说明在文档网站 https://arboria.cwx.world（Astro + Starlight，源码在 `docs/`：页面在 `docs/src/content/docs/`，入门 / 指南 / 参考 / 项目）；改了命令或行为要同步对应页面。CLI 参考页 `reference/cli.md` 由 `uv run python docs/scripts/gen_cli.py` 生成，`tests/test_docs_cli.py` 和 docs CI 检查它是最新的；新命令要在脚本的 `SECTIONS` 里归类。本地预览 `cd docs && npm ci && npm run dev`；部署见 `docs/DEPLOY.md`。
- 跨语言测试向量（`tests/vectors/`，Rust 版要逐字节一致）：改了被覆盖的 Python 行为后运行 `uv run python scripts/export_vectors.py` 重新生成并提交，`tests/test_vectors.py` 会检查它们是最新的。`quirk-` 开头的用例记录的是现在的 Python 行为（可能是 bug），Rust 先照抄。
- Rust 核心（`crates/`，契约见 `docs/adr/0002-rust-contract.md`）：`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 必须全绿（CI：`.github/workflows/rust.yml`）。改了 Python CLI 或 `db.py` 的 schema 要重新生成 `crates/arb/tests/python_commands.txt`（`gen_python_commands.py`）和 `crates/arboria-core/tests/fixtures/`（`gen_fixtures.py`）。
- commit 要小、逻辑清楚。
- 本机（macOS）会给整个 `.venv` 加 `hidden` 标志，Python 3.13 跳过 hidden 的 `.pth`，editable 安装就失效（`No module named 'arboria'`）。所以 pytest 配了 `pythonpath = ["src"]`，起子进程的测试要自己传 `PYTHONPATH`。

## 进度
- 已完成 1a：本地账本、idea 生命周期、快照、note、status/export、hook、install-claude、托管的 CLAUDE.md 区块与 lesson。
- 已完成 1b 中不需要真集群的部分（全部用 FakeSlurm 和 mock 的 subprocess 测试）：
  - schema v3（runs 加 `rejected`、`request_json`、`card_hash`；`jobs` 表每段一行），v1/v2→v3 原地迁移。
  - `clusters`（`~/.config/arboria/clusters.toml`，`ARBORIA_CONFIG_DIR` 可覆盖，示例 `examples/clusters.toml`）、`config`（arboria.toml）、`slurm`（纯函数：walltime、DeltaAI 计费、分段、sbatch 渲染、审核单哈希）。
  - `backends/`：`Backend` 接口、`FakeSlurm`（本地模拟、可编排结果、手动时钟）、`SshSlurm`（`ssh -O check` 门禁 + `NeedLogin`、BatchMode、读取和发送路径限制在本项目的 `<runs>/<project>`、`<out>/<project>` 下）。
  - `submission`：`arb submit`（纯本地）/`card`/`approve`/`reject`。`_guarded_submit` 是 `Backend.submit` 唯一的调用点，提交前再查账本里人的批准 Decision；`tests/test_submit.py` 用源码扫描和 spy 守住这一点。
  - `monitor`：`arb poll`/`cancel`/`watch`（观察期内日志匹配 `arboria.toml [watch] fail_patterns` 就取消，通知里写明正则和那一行；NeedLogin 时暂停该机器）/`logs`/`remote ls`。
  - 项目 id（`arboria.toml [project] id`，`arb init` 生成/补上）：作业名 `arb-<project>-<run>-s<k>`，远程目录 `<runs>/<project>/<run>`，ckpt/out 同理。项目 id 存在 request_json 里、算进审核单哈希；后端绑定一个项目，只认本项目的作业名和路径。
  - install-claude 的 deny 规则（sbatch/srun/salloc/scancel/ssh/scp/rsync），CLAUDE.md 区块和 hook 文案更新。
- 1b 在用户本机上做的部分：
  1. ✅ profile 已实测（2026-11-02）并写入用户的 `~/.config/arboria/clusters.toml`：account `bjki-dtai-gh`，`cpus_per_gpu = 72`、`mem_per_gpu_gb = 110.898`（TRESBillingWeights CPU=13.888888、Mem=9.01726G、GRES/gpu=1000，PriorityFlags=MAX_TRES；DefMemPerGPU=113560 MB，节点可分配 454240 MB = 4×113560，所以不能往上取整），ghx4 48h ×1、ghx4-interactive 2h ×2（`full`/`test` 只给别的账户）。路径在 `/projects/bjki`、`/work/nvme/bjki`、`/work/hdd/bjki` 下的 `$USER/...`。登录节点 aarch64，有 apptainer，没有 uv/pixi。
     - `jobcharge` 不接受作业 id（只按时间段汇总），原来的 `jobcharge <id>` 永远失败。现在 `poll` 从 `sacct` 的 `AllocTRES` 读 `billing=N`，SU = N × 小时 / `billing_per_su`（1000），和 `jobcharge --detail` 对得上；profile 里 `charge_cmd` 换成了 `billing_per_su`。
  2. ✅ `arb machine init` / `arb doctor`（`machine.py`，测试用 `tests/fixtures/deltaai_probe.txt` 里真实的探测输出）。对 deltaai 跑过，全部通过。
  3. 环境（还没做）：用户已有 uv 的做法（`/work/hdd/bjki/$USER/opt/uv-aarch64`、`venvs/WaveFront-gh`、`/work/nvme/bjki/$USER/envproj` 里的 uv.lock），倾向 uv；NGC PyTorch 容器要用 apptainer 自己拉（没有对应 module）。计算节点能不能上网还没确认。需要短作业在计算节点上构建 → 必须经 `arb submit` + 用户 `arb approve`，Claude 不能自己 sbatch。定下来后填 `prelude` / `env_activate`。
  4. 通过 `SshSlurm` 跑通第一个真实 run（还没做）：submit → approve → watch → 拉回 `ARBORIA_OUT`，核对命令行、`%u` 日志路径、`sacct` 状态和花费。
- 已知限制：`find_job` 用 `sacct --starttime=now-30days`；加入项目 id 之前提交的 run 不能再 approve，要重新 submit。
- **分支 `notebook-api`**（基于 1b 之后的 main，没在真集群上跑过）：每个 idea 一个 marimo notebook（`.arboria/notebooks/<idea>.py`，worktree 里链接成 `notebook.py`，`info/exclude` 排除，备份仓库跟踪，Claude 只读）；`arboria.lab` notebook API（`lab.run` 只生成审核单，不批准）；schema v4（`runs.parent_run`、`decisions.payload_json`、`notes.author`、`tasks`）；`arb submit --from rNNN --set k=v` 和按审核单哈希的幂等提交；`arboria.toml [wandb]`/`[run]`、`ARBORIA_WANDB_ID`、`runs.wandb_url`、拉回 `out/config.yaml` 存进 `runs.config_json`；`arb todo list/done/drop`，hook 列出待办；批准只在人的终端里（见上面的约束）；`arb note` 在 Claude Code 里被拒绝。用户的真集群工作合进 main 之后，这个分支要再 rebase/合并进 main。
- 已完成看板 `arb ui [--idea] [--port]`：127.0.0.1 上的 `marimo run`（带 token），每 30 秒重读账本；待你处理（待批准 run 的审核单全文、你上次写笔记后结束的 run、待确认经验、待办、最近通知）、idea 树（SVG 时间轴，从 idea_new 事件读 fork 自哪个 run）、idea 详情、算力与预算。`arb status`/hook 在有事等人时多一行“看板：arb ui”（`report.FINISHED_UNSEEN_SQL` 与看板同一规则）。install-claude 把全局命令目录写进不提交的 `.claude/settings.local.json`，并从 settings.json 挪走旧的。
- 之后：第 2 步做提交前检查、审核包和独立 AI 审稿人、基于历史的估计（以及提交前的排队估计）；第 3 步在看板上继续做（例如 wandb 曲线的离线缓存，看板本身不联网）。

- 已完成 notebook 命令：schema v5（`tasks.kind` ∈ plain/command_new/command_revise，`tasks.payload_json`）；`lab.<名字>(...)`（模块级 `__getattr__`）、`.revise`/`.source`/`.info`、`lab.commands()`、`arb commands list|guide`；缺命令时写待办（同名复用）并尽力推给会话（`[claude] push`）；项目命令随备份提交版本化。自动重跑：marimo 公开的响应式（`mo.state`、`mo.watch.file`、`mo.ui.refresh`）只重跑**引用了绑定到该对象的全局变量**的 cell，`lab.svd(r)` 这种 cell 碰不到，所以 `lab._cell_rerunner` 用私有接口（`ctx._kernel.graph.set_stale` + `_execute_stale_cells_callback`，对着 0.25.2 核对过，`test_marimo_private_api_still_there` 守着），拿不到就退回“重新运行这个 cell”的提示。另有 `lab.run(repeat=True)` / `arb submit --repeat`（nonce 进审核单哈希）和 `arb idea notebook`。

- 已完成种子（分支 `seeds`）：schema v6（`seeds`、`plantings`；`tasks` 重建为 idea_id 或 seed_id 二选一）；`arb seed new/list/show/set/drop/plant`，任何目录可用、第一次用时建好种子园；种下的 idea notebook 开头“来自种子”并带折叠的“种子里的思路”（marimo `/// details`）；`lab` 认得种子 notebook（`SeedTarget`：run/fork/prune 报错指向 `arb seed plant`，todo/todos/metric 可用，只能用全局命令，`ctx.seed`）；种子园里的 hook / `arb status` / `arb todo`；`arb install-claude --home`；种子园看板 `arb ui --home`（仓库外默认），项目看板标题下有思考中的种子数。
- 已完成小修（分支 `small-fixes`）：`arb seed revive NAME`（已放弃 → 思考中，`seed_revive` 事件触发备份，只由人；种子园看板“已放弃”里每个旁边是这条命令的文本）；`lab.todo` 返回 `lab.Todo`（Mapping，`.id`/`.data`/`.existed`，显示成一行“已交给 Claude（t001）：…”），`lab.todos()` 返回 `lab.TodoList`（一行一个：id · 状态 · 文字 · 多久前），不再是 dict/DataFrame 的 JSON 树。

- 已完成文档网站（分支 `site`）：https://arboria.cwx.world，Astro 7.3.8 + Starlight 0.42.6（`docs/`，依赖用 `npm install --before` 锁到至少几周前的版本，锁文件提交），中文为主、`/en/` 是简短的英文首页；Pagefind 中文搜索；`docs/guide.md` 拆成入门 / 指南 / 参考 / 项目各页后删除；CLI 参考由 `docs/scripts/gen_cli.py` 生成；`.github/workflows/docs.yml` 构建并检查；Cloudflare Pages 部署（`docs/DEPLOY.md`），`pages.yml` 是关着的 GitHub Pages 备选。网站不加载外部资源。

- 已完成 CLI 整理（分支 `cli-tidy`）：用户面对的概念只有 idea、run、待办，加上批准和记录。`arb --help` 分成四组（`cli.HELP_PANELS`：你做的 / 集群 / agent 做的 / 安装与内部；`tests/test_cli_help.py` 要求每个可见的顶层命令恰好在一组里，新命令要加进去）；`arb run new` 隐藏（被 `arb submit` 取代，测试仍在用），不再写进 CLAUDE.md 规则；种子并进 `arb idea`（new/list/show/set/drop/revive/plant，去向规则见上面的种子园约束），`arb idea show`、`arb idea drop`（= `close --drop`）也给 idea 用；`arb seed …` 是隐藏的别名；deny 规则加上只写种子的 `arb idea set/revive/plant`（项目里也加，plant 在仓库里运行），`--home` 再加 `arb idea new/drop`；`gen_cli.py` 跳过隐藏命令。
- 已完成命令库为中心（分支 `commands-center`）：`lab.commands()` 一张表（内置 / 项目 / 全局 · 签名 · 版本 · 说明；内置清单是 `commands.BUILTINS`，种子里只列 todo/todos/metric），自定义命令的签名由 `commands.describe` 用 `ast` 读；只写 `lab.svd` 显示帮助（`lab.Help`）。`ask="…"` 是 lab 自己消费的第一次说明（和 `scope=` 同为 `RESERVED_KWARGS`），旧写法末尾的位置字符串在签名放不下时由 `commands.fit_args` 去掉。内置分析 `lab.plot`（内联 SVG，不加依赖）、`lab.diff`（只读的 `git diff --stat`）、`lab.spend` 在 `analysis.py`（只读、不联网，看板不 import 它）。`arb commands promote NAME [--force]` 复制到全局命令库，`command_promote` 事件触发备份。托管 CLAUDE.md 区块多一条：反复出现的分析写成命令。

- 已完成统一字体（分支 `font`）：notebook 和看板的正文、标题、marimo 界面文字用 Noto Sans SC（2.004，可变字重 WOFF2 7.5 MB），代码（看板里的命令、id、数字）用 Maple Mono CN（v7.9，Regular/Bold WOFF2 各约 5.4 MB，带连字；`"calt" 1, "cv01" 1`，cv01 把 `@ $ & % Q => ->` 换成普通字形）。字体文件是 vendored 的（`src/arboria/ui/fonts/`，都是 SIL OFL 1.1：`OFL-NotoSansSC.txt`、`OFL-MapleMono.txt`，来源见 `NOTICE`，都写进 `license-files`），wheel 约 18 MB，生成的样式表约 25 MB。marimo 把 `css_file` 内联进页面，`url()` 只能按服务器解析，所以字体以 `data:` URI 嵌进生成的样式表（`style.py`，首行带摘要，内容不变不重写）：notebook 用 `App(css_file="arboria.css")`，arb 写 `.arboria/notebooks/arboria.css`（worktree 里链接成 `arboria.css`，`info/exclude` 排除；marimo 不解析链接，按打开的路径找）和 `<home>/seeds/arboria.css`；备份仓库只收 `*.py`，字体不进去。看板：`arb ui` 让 marimo 运行缓存目录（`paths.cache_dir`，`ARBORIA_CACHE_DIR`）里指向包内 app 的链接，旁边是带字体的 `dashboard.css`；包里的 `dashboard.css` 不带字体（独立 HTML 退回系统字体）。旧 notebook 用 `arb idea style`（只由人；只改 `marimo.App(...)` 那一行，cell 字节不变，幂等，install-claude 也 deny）。测试里 `conftest.small_font` 把字体换成一行占位。
- 已完成读代码（分支 `inspect`）：`lab.inspect(question, *, view="code"|"pseudo"|"simple"|"map", run=None, refresh=False)` 在人的 kernel 里第一次调用写一个 `inspect` 待办（schema v7：`tasks.kind` 加 `inspect`，`inspections` 表存答案，按 (idea, question, view, target) 查缓存），像命令待办一样尽力推给会话（`inspection.push_text`，只用校验过的字段），`lab._watch_probe` 盯待办状态、交付后自动重跑。代码版本：`run=` 用 run 的快照；否则用 `snapshot.write_tree` 只读地取 worktree 现状（含未提交），建待办时才 `commit_tree`，挂在 `refs/arboria/inspect/<待办>`；树变了显示“代码已变”，`.refresh()` 只在变了（或上次被 drop）时再交，绝不自动重生成。agent 只给位置和批注：`arb inspect check|done tNNN --file -` 校验路径/行号/上限并用 `git cat-file blob <code_ref>:<path>`（字节，保留 `\r`）取真实行存进 result_json，`arb inspect source` 打印 code_ref 的文件，`arb inspect guide` 是约定（和谁来执行无关：会话、子代理或无头作业都一样）。`arb todo done` 拒绝 inspect 待办；`inspect_done` 触发备份。map 用 `mo.mermaid`（marimo 自带 mermaid，离线可用：只许连 127.0.0.1 的 Chrome 截图验证过）。Mermaid 校验：首行是图类型，不许 `%%{init}%%`、`click`、链接、HTML（只许 `<br>`）。

## 使用 arboria 的训练代码要遵守的约定
1. 启动时把完整的解析后配置导出为 `$ARBORIA_OUT/config.yaml`（`poll` 拉回后存进 `runs.config_json`）。
2. `wandb.init(id=ARBORIA_WANDB_ID, group=ARBORIA_IDEA)`（`ARBORIA_WANDB_ID` = `<项目 id>-<run id>`，几个仓库共用一个 wandb project 也不冲突）。
3. 路径只从 `ARBORIA_*` 角色变量读取。
4. checkpoint 存到 `ARBORIA_CKPT_DIR`，并且可以续训。

- **Rust 重写（进行中，2026-11-04 起）**。决定在 `docs/adr/`：0001（Mac app + Rust 核心）、0002（第 1 期契约：crate / 模块对照、schema v8、哪些已实现）、0003（扩展：第 1 期只有 probe）。第 0 期的试验和结论在 `spikes/`（编辑器 markdown 往返、Rust 驱动 kernel、本地意图模型 Qwen3.5-4B + KV 缓存）。设计：Arboria 设计系统 https://claude.ai/artifact/LeSPASKLSUE39HtVTiiGfk 、主界面画布 https://claude.ai/artifact/DGQD3R9dMC8s8ErYq5Rqd6 、编辑器原型 https://claude.ai/artifact/KNTafPRmEDfoQJZBQMwcWn 、方案 https://claude.ai/artifact/AbJrewmFfKDfNtoPotL2zm 。
  - 已合并：跨语言测试向量（`tests/vectors/`，59 个文件 1215 个用例）、Rust 工作区骨架（`crates/arboria-core`、`crates/arb` → `arb-rs`），真实实现的有 canonical、model、schema v8 + 迁移、db、records、human、probe 头部解析与结果校验；其余 `todo!()`，51 个向量测试 `#[ignore]` 等着接。
  - 已移植（分支 `rust-slurm-cards`）：slurm、cards（sha2 0.11.1）、config（`fail_patterns` 用 regex 1.13.1，编译不了的报错）、clusters（toml `preserve_order`）、paths、gitutil 的 git 调用；向量又接上 33 个文件，`tests/cards.rs` 拿审核单全文和 CPython 的输出逐字比，`tests/repo_config.rs` 测文件和真 git 仓库那一侧。
  - 下一步按顺序移植（每接一个模块就去掉对应向量测试的 `#[ignore]`）：db 上的 records / tasks / ideas → snapshot（调 git 命令）→ backend：FakeSlurm、SshSlurm、monitor（sacct 解析、计费）→ submission（唯一提交点）→ CLI 接线和 `--json`。Python 版要小改一处：能只读打开 v8 账本。
  - 已修：Python 的校验正则原来用 `re.match(r"^…$")`，`$` 放过结尾换行（远程路径里的换行会进 sbatch 脚本）；现在一律 `fullmatch`，Rust 整串匹配，新的校验正则也要这样。
  - 省 token 的规矩（用户定的）：任务需要的并行 subagent 少于 5 个就不开 subagent，自己做。
