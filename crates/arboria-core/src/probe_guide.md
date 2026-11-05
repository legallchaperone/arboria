# probe：给 agent 的约定

probe 是一个很小的纯函数：把一台机器上某个查询命令的输出解析成一个结构化结果，由 arb 定时
运行、显示给人。人在待办里要你写一个 probe（kind 是 probe_new / probe_revise），你只写这一个
文件。arb 绝不根据 probe 的结果做任何动作：它只显示给人看，或者变成待办。

## 文件
- 项目范围：`.arboria/probes/<名字>.py`；全局：`~/.config/arboria/probes/<名字>.py`
  （项目里的同名文件优先）。名字 `^[a-z][a-z0-9_]{0,40}$`，文件里定义同名函数。
- 文件头是一段注释（和 PEP 723 一样的格式），arb 只读注释，不执行文件：

```python
# /// arb-probe
# api = 1
# machine = "deltaai"          # clusters.toml 里的机器名
# argv = ["accounts"]          # 要运行的命令，写死；arb 自己在机器上运行它
# every = "30m"                # 多久跑一次：1m 到 7d
# kind = "number"              # number | table | text
# ///
def deltaai_credits(text: str) -> dict:
    """DeltaAI 剩余 SU。"""
    ...
    return {"label": "剩余 SU", "value": 199, "of": 200}
```

## 函数
- 参数只有一个：命令的标准输出（字符串，最多 256 KiB）。返回一个 dict：
  - number：`{"label": "…", "value": 199, "of": 200, "unit": "SU"}`（of、unit 可选）
  - table：`{"label": "…", "columns": ["账户", "余额"], "rows": [["bjki", 199]]}`
    （最多 8 列、50 行；单元格是字符串、数字或 null）
  - text：`{"label": "…", "text": "…"}`（最多 2000 字）
- 只用标准库；不联网、不起进程、不读写文件、不碰 ssh。解析不出来就抛异常（arb 会记下失败），
  不要编一个结果。
- label ≤40 字，不要 HTML，不要控制字符；数字必须是有限的。

## 自测，然后交给人
- 把命令的一份真实输出存成文件（人可以给你，或者在待办里），然后
  `arb probe check <名字> --input 输出.txt`：只跑你的解析函数，不碰远端，打印校验后的结果。
- 写完用 `arb todo done tNNN --result "…"` 交代。启用只能由人在自己的终端里做
  （`arb probe enable <名字>`，你不能运行）：启用绑定文件内容的哈希，你之后再改一个字节，
  这个 probe 就会停用，等人重新启用。
