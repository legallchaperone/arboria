# ADR 0004：第 2 期，app 的壳

状态：已定（2026-11-05）。前提见 ADR 0001（方案全文 https://claude.ai/artifact/AbJrewmFfKDfNtoPotL2zm 第 2 期）。实现时以本文为准；有冲突先问人。

第 2 期交付：看板、idea 树、Touch ID 批准、嵌入终端、后台小助手和通知。notebook 这时还是 marimo（app 里开一个窗口跑 `marimo edit`）。用户得到：不用开终端就能看作业、签字。

## 代码在哪
| 部分 | 位置 | 说明 |
|---|---|---|
| 项目登记 | `crates/arboria-core/src/registry.rs` | `<config>/projects.json`：app 和小助手要看哪些仓库 |
| 只读视图 | `crates/arboria-core/src/view.rs` | app 的全部数据：收件箱、idea 树、idea 详情、花费、probe |
| 在本机批准 | `human::confirm_on_device`、`submission::approve_on_device` | Touch ID；只在 macOS |
| Touch ID | `crates/arboria-macos` | objc2 的 LocalAuthentication；别的系统不编译 |
| 通知 | `crates/arboria-core/src/notifier.rs` | 系统通知：macOS `osascript`、Linux `notify-send` |
| 小助手 | `crates/arboria-core/src/helper.rs`，`arb helper run/install/uninstall/status` | 定时 poll、probe、通知；LaunchAgent（macOS）/ systemd 用户服务（Linux） |
| app 后台 | `app/src-tauri/`（单独的 Cargo 工作区） | Tauri 2.12.5：命令、终端（portable-pty）、账本变化、marimo 窗口 |
| app 界面 | `app/src/`（Vite + React + TypeScript） | 收件箱、全局、idea、审核单、终端、⌘K / ⌘P |

app 是单独的 Cargo 工作区（`app/src-tauri/Cargo.lock`），根工作区 `exclude` 它：核心和 CLI 的 CI 不需要 webkit。

## 项目登记（`registry`）
- 文件 `<ARBORIA_CONFIG_DIR 或 ~/.config/arboria>/projects.json`：`{"projects": [{"path": "<主 worktree 绝对路径>", "added_at": "…"}]}`。写入用临时文件 + rename，加文件锁。
- 登记：`arb init` 成功后（CLI 层做，失败只警告）、`arb projects add [PATH]`、app 的“添加项目”。`arb projects list|remove`。种子园（`paths::home()`）总在列表里，不用登记。
- 列表里的仓库没了或没 `arb init` 过：视图里显示成一行错误，不删条目。

## 只读视图（`view`）
- 账本一律 `db::open_read_only`（`mode=ro`）；`view.rs` 不 import 写入函数、后端、子进程（源码扫描测试守住，和 Python `tests/test_ui.py` 同一条约束）。
- `view::board(root) -> Board`（JSON，键 snake_case）：
  - `project`：root、名字、project_id、total_su、config_error、schema、data_version、loaded_at。
  - `ideas`：每个 idea（含种子）的 name、stage、hypothesis、metric、confidence、parent、from_run、created_at、closed_at、last_active、su_used、run 数、notebook 是否存在。
  - `runs`：id、idea、parent_run、status、change_reason、machine、partition、gpus、walltime、est_su、billed_su、summary、wandb_url、created/updated/finished_at、jobs（segment、job_id、state、elapsed_s、billed_su）。
  - `inbox`：要人处理的事，`kind` ∈ `approve`（待签字）、`finished`（跑完了，规则同 `report.FINISHED_UNSEEN_SQL`：idea 最后一条人写的 note 之后结束的）、`failed`（失败 / 取消，同一规则）、`agent_done`（同一规则下 agent 做完的待办）、`lesson`（待确认经验）、`probe_reenable`（probe 待重新启用）。每条带 when、title、meta 和引用（run / idea / task / lesson）。
  - `cards`：每个待签字 run 的审核单：`text`（和 `arb card` 一字不差）、`card_hash`、`matches`（重新算的哈希是否等于账本里的）、`est_su`、`error`。
  - `todos`：没做完的待办（agent 的队列）。`notifications`：`notifications.log` 最后 20 行，新的在前。
  - `spend`：已用、预算、按 idea 分。`probes`：每个 (machine, probe) 最新一条结果。
- `view::garden()`：种子园（种子列表、思考中的数）。`view::inbox_all()`：所有登记的项目合成一个收件箱。
- 界面上 ledger 里的文字一律当文本渲染（React 不用 `dangerouslySetInnerHTML`，测试扫描源码）。

## 在 app 里批准（Touch ID）
入口仍然只有两个：终端里的 `arb approve`，和 app 的审核单。app 这条路：
1. 界面打开审核单时调 `card_shown(root, run)`：后台重算审核单，记下 (run, card_hash, 显示时刻)，把全文给界面。
2. 人按“签字”（⌘⏎）：`approve(root, run, card_hash)`。后台检查：这个 run 的审核单确实显示过、哈希和显示的一致、显示了至少 `human::MIN_SHOWN`（2 秒）。
3. `submission::approve_on_device` → `human::confirm_on_device(run, card_hash, shown_for)`：进程环境里有 `CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT` 就拒绝；显示不到 2 秒拒绝；然后 Touch ID（`LAPolicyDeviceOwnerAuthentication`：指纹，或者系统密码），提示里写着 run id 和审核单哈希前 8 位。通过了才造出 `Confirmed`（provenance 的 `confirmed_by` = `Touch ID in Arboria.app`）。之后和 `arb approve` 走同一个函数：Decision 绑定审核单哈希，再提交。
4. 不是 macOS：`confirm_on_device` 一律拒绝；界面显示命令文本 `arb approve rNNN`（可复制，或在嵌入终端里运行）。

不变的：`Confirmed` 只在 `human.rs` 里造（`confirm`、`confirm_on_device`），源码扫描守住；没有任何自动批准的路径；Touch ID 实现在 `arboria-macos`，核心只在 macOS 上链接它。

webview 的边界：只有主窗口（label `main`）里 app 自带的页面有 IPC 权限（`app/src-tauri/capabilities/main.json`）；内嵌的 marimo 是 `http://127.0.0.1:…` 的 iframe，另一个源，没有任何 capability，也碰不到主页面，notebook 里的代码调不到 `approve`。CSP 只许 app 自己的资源和 `ipc:`。测试读 capabilities 文件守住。嵌入终端里的 Claude Code 是 pty 的子进程，碰不到 webview 的 IPC；`arb approve` 在那里照样被 `require_terminal` 拒绝。护栏，不是沙箱：硬保证仍然是 Touch ID 要人的手指，和提交前查账本里人的 Decision。

## 小助手（`helper`）
- `arb helper run [--every 秒] [--once]`：每轮对每个登记的项目和种子园：
  1. poll 所有活跃 run（`monitor::poll_run` + `observe`，和 `arb watch` 同一套），NeedLogin 的机器这一轮跳过；
  2. 跑到期的 probe（已启用、哈希一致、距上次结果超过 `every`），内置 `balance` 每 30 分钟一次（有 `balance_cmd` 的机器，结果存种子园账本）；
  3. 发系统通知：run 结束（成功 / 失败 / 取消）、新的待签字、日志匹配失败模式、机器需要登录（每台机器断一次只发一次，恢复后重置）。
- 只写 poll / probe 本来就写的东西；`helper.rs` 不碰 `approve`、`Confirmed`、`decisions`（源码扫描守住）。
- 状态（每个项目看到的最后一个事件 id、需要登录的机器）在 `<cache>/helper.json`；同时只跑一个（`<cache>/helper.lock` 文件锁）；每轮写 `<cache>/helper.json` 的 `last_tick`，app 显示“小助手：x 分钟前”。
- `arb helper install`：macOS 写 `~/Library/LaunchAgents/world.cwx.arboria.helper.plist`（`RunAtLoad`、`KeepAlive`、日志在 `~/Library/Logs/Arboria/`、带上安装时的 `PATH`）并 `launchctl bootstrap`；Linux 写 `~/.config/systemd/user/arboria-helper.service` 并 `systemctl --user enable --now`。`uninstall` 反过来，`status` 显示装没装、上次一轮。app 的全局页有同样的按钮。
- 默认每 120 秒一轮（ssh 走 ControlPersist，便宜）。

## 通知（`notifier`）
系统通知只是提醒，内容只由校验过的字段拼成（run id、状态、SU、机器名）。macOS 用 `osascript`（参数经 `on run argv` 传，不拼进脚本），Linux 用 `notify-send`（没有就跳过）。任何失败都静默。`ARBORIA_NOTIFY=0` 关掉（测试里默认关）。

## app
- 界面（2026-11-05 第一次实机使用后重做，参照 Linear）：左侧栏（收件箱、项目和它们进行中的 idea、种子园、设置），中间是页面，右边是详情面板（run、待签字的审核单：摘要在上，全文折叠），终端可停在下 / 左 / 右；正文用无衬线（PingFang SC / Noto Sans SC），等宽只给代码、id、数字；设置页用表单改 clusters.toml 和 arboria.toml（核心 `settings`，toml_edit 保留注释，按 arb 的读法检查后才写）。原来的三个主界面（主界面画布，都选 A）：**收件箱**（没打开 idea 时的首页：一列，按类分组，⏎ 打开）、**全局**（idea 和 run 的时间线、花费条、种子、每台机器的 probe 卡）、**idea**（第 2 期：标题、假设、指标、run 列表和状态、笔记、待办，“打开 notebook”开 marimo 窗口）。标题栏：面包屑、“待你处理 N”、⌘P 换 idea、⌘K 命令、⌘` 终端。
- 日常操作（2026-11-05 补上，之前都只给“去终端里敲”的命令文本）：从列表移除项目（`registry::remove`，文件不动）、在 Finder 中显示、新建 idea（可选父 idea）、写笔记（可挂在 run 上）、合并 / 放弃 idea、拒绝 run、取消作业、放弃待办、种子的新建 / 放弃 / 恢复 / 种下。每个都是 `app/src-tauri/src/actions.rs` 里一个带类型的 `Action`，固定地映射成一条 `arb` 命令行（名字和 id 在这里先查，自由文字放在 `--` 后面，不能变成选项），用人自己的 `arb` 跑：长度校验、只由人的检查（app 从 Claude Code 里启动时写笔记和改种子被拒绝）都还在 `arb` 里，只有一份。界面不传 argv。没有一个会批准（`nothing_here_approves` 测试；签字只走 `approve_on_device`）。`with_the_real_arb` 测试（`ARBORIA_TEST_ARB=<arb>`）在临时仓库和种子园里用真的 `arb` 把每个操作跑一遍。
- 审核单（ab-slip）：全文、资源、花费、哈希；“签字 ⌘⏎”在显示满 2 秒前不可按；“稍后 esc”。
- 终端：xterm.js + portable-pty，在项目主 worktree 里开登录 shell；“Claude Code”按钮开一个运行 `claude` 的标签；需要登录时提示“在终端里登录 <机器>”并给出命令。
- 账本变化：后台每秒查每个项目的 `PRAGMA data_version` 和 `notifications.log` 的大小，变了就发 `ledger-changed`，界面重读。
- notebook（2026-11-05 改为内嵌）：`open_notebook(root, idea)` 先 `ideas::ensure_notebook`（只补缺的 notebook，不覆盖），再在 worktree 里起 `marimo edit notebook.py --headless --host 127.0.0.1 --port <空闲> --token`，把 URL 交给页面，idea 页的 Notebook 标签用带 sandbox 的 iframe 嵌入（CSP `frame-src` 只许 `http://127.0.0.1:*`）；每个 idea 一个进程，复用到 app 退出。
- 样式：设计系统的 `tokens.css` + `bundle.css`（ab-* 组件）原样放进 `app/src/styles/`，app 自己的 CSS 只准用 token（测试扫描：不许出现 token 以外的颜色字面量）。不加 Tailwind（方案里写了 Tailwind；设计系统已经给了组件 CSS，再加一层没有好处）。字体 Maple Mono CN 用包里 vendored 的那份（`src/arboria/ui/fonts/`）。
- 测试：`npm test`（vitest，纯函数：收件箱分组、时间线布局、格式化）；`npm run check`（tsc，fixture JSON 要符合类型）；Playwright 截图（`app/scripts/screens.mjs`，用 `@tauri-apps/api/mocks` 的假 IPC 和 fixture）。fixture `app/src/fixtures/board.json` 由核心测试生成（`UPDATE_FIXTURES=1 cargo test -p arboria-core --test view`），测试检查它是最新的：Rust 输出和 TypeScript 类型对得上。

## 依赖
都按惯例锁到发布满两周的版本（Rust 用 `cargo update --precise`，npm 用 `npm install --before`），锁文件提交。Rust：tauri 2.12.5、tauri-build 2.7.4、portable-pty 0.9.0、objc2 0.6.x、objc2-local-authentication 0.3.x、block2 0.6.x。npm：react、@tauri-apps/api、@xterm/xterm、vite、typescript、vitest。

## 明确不做（第 2 期）
自己的 notebook（第 3 期）、`claude -p` 队列和 agent 面板（第 3 期）、ghost code（第 4 期）、自动更新、签名和公证（要 Apple 开发者账号，等人决定）。

## 验收（人做）
在 Mac 上：`cd app && npm ci && npm run tauri build`，打开 Arboria.app：收件箱里有待签字的 run → 打开审核单 → 等 2 秒 → Touch ID → 提交成功；`arb helper install` 后关掉 app，run 结束时收到系统通知；app 里的终端能跑 `claude`。
