# ADR 0003：扩展（第 1 期只有 probe）

状态：已定（2026-11-04），经 Fable 评审修改。前提见 ADR 0001。

## 结论
- 用户的例子“全局页显示 DeltaAI 剩余额度”**不需要插件**：`Profile.balance_cmd`（`machine init` 自动填 `accounts`）、`Backend.balance()`、`machine.parse_accounts()` 都已有，只缺核心的调度、存储和显示。由核心直接做。
- 真正需要用户（agent）写代码的只有一处：**别的站点的查询命令和输出格式不同，解析不同**。所以第 1 期只做一个扩展点：**probe**。
- 分界线：**核心绝不根据扩展的输出做任何动作，只把它显示给人，或变成待办交给 agent。**

## 必须留在 Rust 核心的（不是插件）
审核单哈希、sbatch 渲染、分段、作业名、远程路径限制、ship / submit / cancel、ssh 门禁（`ssh -O check`）、钥匙串、账本写入、poll 的状态映射和计费、approve 和一切人的意图入口。内置的五种机器（本机、ssh、Slurm、RunPod、Modal）是 Rust 里 `Backend` trait 的实现，不是插件；dogfooding 的层次是这个 trait，不是进程边界。真出现“实验室自己的调度器”时，另开 ADR 设计契约极窄的 driver 接口（job id 正则、状态枚举、费用由核心从原始数字算）。

## probe
一个文件：`.arboria/probes/<名字>.py`（项目）或 `~/.config/arboria/probes/<名字>.py`（全局）。元数据写在文件头（借 Raycast Script Commands 的形态；格式同 PEP 723 的注释块）：

```python
# /// arb-probe
# api = 1
# machine = "deltaai"
# argv = ["accounts"]          # 写死；核心执行，shlex 拼接
# every = "30m"
# kind = "number"              # number | table | text
# ///
def deltaai_credits(text: str) -> dict:
    """DeltaAI 剩余 SU。"""
    ...
    return {"label": "剩余 SU", "value": 199, "of": 200}
```

- **插件是纯函数**：核心按 `every` 调度（LaunchAgent helper），`ssh -O check` 不过就跳过（NeedLogin 不重试），核心自己跑 `argv`，把 stdout 交给短命进程 `uv run --project <项目> python -I arb_probe_host.py`（stdin 进 JSON、stdout 出 JSON，超时杀进程组，输入输出都截断），校验返回（schema、长度、无 HTML，显示一律转义），写 `probe_results(machine, probe, at, input_hash, result_json)`。心跳类写入，不触发备份提交。**没有** `ctx.remote()`、没有 `network` 权限、插件无法请求核心做任何事。
- **启用是人的 Decision，绑定文件内容哈希**：`arb probe enable NAME` 只在人的终端（`human.require_terminal`）或 app 的 Touch ID，显示 machine、argv、every、哈希，要人输入名字；helper 只运行哈希等于已启用哈希的文件，改一个字节就自动停用，`arb status` 显示“待重新启用”。install-claude deny `arb probe enable`。
- **诚实的边界**：同机同用户，插件进程本来就能自己调 ssh（用户的 ControlMaster socket）；install-claude 的 deny 管不到 helper 运行的代码。所以唯一的控制是“只运行人启用过的那份字节”。护栏，不是沙箱。`.arboria/probes/` 若进 git，就是供应链入口，同样由哈希启用覆盖。
- **内置 probe `balance`**：用 `balance_cmd` + `parse_accounts`，解析失败则 kind=text 原样显示。DeltaAI 的例子零插件代码。
- **agent 的路**：待办 kind `probe_new` / `probe_revise`（同命令待办）；`arb probe guide`；`arb probe check NAME --input 文件`（只跑解析，不碰远端，agent 自测用）；`arb probe run NAME`（人手动跑一次）。
- **显示**：全局页每台机器一张卡，列出所有 probe 结果、时间、“需登录”；`arb status` 一行。
- **守住**：源码扫描 + spy 证明 probe 宿主只经固定函数写账本（`set_probe_result`、`add_todo`、`notify`），触达不了 `decisions` / `submit`；哈希不等于已启用哈希就不运行。

## 和命令的关系
并存，共享文件约定，不共享运行时：命令是人在 kernel 里调、返回富显示；probe 是 helper 定时跑、返回校验过的结构。共享：一个 `.py` 文件、`ast` 读头部、`describe`、promote 到全局、待办交给 agent。

## 版本与依赖
身份 = 内容哈希（启用绑定它）；`api` 整数写在文件头，宿主不认就停用并说明。第 1 期只允许项目环境 + 标准库；以后用 PEP 723 `dependencies` 和 `uv lock --script`，不自己发明依赖声明。

## 明确不做（第 1 期）
machine driver 插件、panel、`plugin.toml` 文件夹、JSON-RPC 常驻宿主、`network` 权限、`ctx.remote()`、`arb plugin add`。

## 之后再议
`check`（给审核单加提醒，不能批准；之前推迟的“AI 审核资源用量”就是它；可能需要常驻宿主，到时借 LSP 的 stdio JSON-RPC 和能力协商）、`event`（run 结束 / 失败时建待办、发通知）、跨机器的 panel、PEP 723 依赖、远程安装。到那时再决定要不要把 probe 和 command 统一叫“扩展”。

## 借鉴
Raycast Script Commands（单文件、头部元数据、定时刷新、一行显示）；VS Code（声明优先，扩展宿主单独进程）；Deno / Chrome MV3（默认全拒、声明式权限、启用时给人看）。不借 Obsidian（进程内全权限）。
