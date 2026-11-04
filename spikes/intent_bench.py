# /// script
# requires-python = ">=3.11"
# dependencies = ["mlx-lm==0.32.0"]
# ///
"""Spike: can a small local model turn a Chinese intent into one Arboria command line?

Run on an Apple-silicon Mac:   uv run spikes/intent_bench.py
Optional:                      uv run spikes/intent_bench.py --models mlx-community/Qwen3.5-2B-4bit

For each model it asks 50 intents, parses the JSON answer, checks it against the
command signatures, and reports: valid JSON, right command, right arguments,
"no suggestion" when nothing fits, and latency (p50 / p95, after one warm-up).
The model only picks a command and fills arguments; Arboria builds the line itself.
"""
from __future__ import annotations

import argparse
import json
import re
import statistics
import time
from pathlib import Path

COMMANDS = {
    "runs": {"args": {}, "desc": "列出这个 idea 的 run"},
    "plot": {"args": {"runs": "list[run]", "key": "metric", "log": "bool"}, "desc": "几个 run 的一条曲线画在一起"},
    "compare": {"args": {"runs": "list[run]", "key": "metric"}, "desc": "几个 run 的曲线数据（表）"},
    "diff": {"args": {"a": "run", "b": "run"}, "desc": "两个 run 哪里不同：命令、资源、代码、config"},
    "spend": {"args": {"idea": "str|null"}, "desc": "各 idea 花了多少 SU 和 GPU 时"},
    "inspect": {"args": {"question": "str", "view": "code|pseudo|simple|map", "run": "run|null"}, "desc": "让 agent 读代码：核心源码、伪代码、简化版或框架图"},
    "svd": {"args": {"run": "run", "k": "int", "log": "bool"}, "desc": "（项目命令）run 的权重做 SVD，画前 k 个奇异值"},
    "attn_entropy": {"args": {"run": "run", "layers": "list[int]|null"}, "desc": "（项目命令）每层 attention 的平均熵"},
    "todos": {"args": {}, "desc": "待办列表"},
    "run": {"args": {"reason": "str", "from_": "run", "set": "dict"}, "desc": "生成审核单（新的 run，要人签字）"},
}
RUNS = {
    "r011": "warmup 2k · lr 3e-4 · 完成 · val/loss 2.915",
    "r012": "warmup 4k · lr 3e-4 · 完成 · val/loss 2.879",
    "r013": "warmup 8k · lr 6e-4 · 运行中 · val/loss 2.860",
    "r014": "warmup 16k · lr 6e-4 · 待签字",
}
METRICS = ["val/loss", "train/loss", "val/acc", "grad_norm", "lr"]

# (intent, expected command or None, expected args subset)
CASES = [
    ("把 r011 和 r012 的 val/loss 画在一起", "plot", {"runs": ["r011", "r012"], "key": "val/loss"}),
    ("三个跑完的 run 的训练 loss 对比一下", "plot", {"runs": ["r011", "r012", "r013"], "key": "train/loss"}),
    ("r013 的梯度范数曲线", "plot", {"runs": ["r013"], "key": "grad_norm"}),
    ("val/acc 用对数坐标画 r012 r013", "plot", {"runs": ["r012", "r013"], "key": "val/acc", "log": True}),
    ("看看 lr 是怎么变的，r011", "plot", {"runs": ["r011"], "key": "lr"}),
    ("所有 run 的验证损失", "plot", {"key": "val/loss"}),
    ("学习率大于 1e-4 的 run 画在一起", "plot", {"key": "val/loss"}),
    ("warmup 最长的两个 run 比一下 loss", "plot", {"runs": ["r013", "r014"]}),
    ("r011 和 r013 的 val/loss 数据给我一张表", "compare", {"runs": ["r011", "r013"], "key": "val/loss"}),
    ("r012 r013 的 train/loss 原始数字", "compare", {"runs": ["r012", "r013"], "key": "train/loss"}),
    ("r012 和 r013 有什么不同", "diff", {"a": "r012", "b": "r013"}),
    ("对比一下 r011 跟 r012 的配置差别", "diff", {"a": "r011", "b": "r012"}),
    ("r013 相比 r012 改了哪些代码", "diff", {"a": "r012", "b": "r013"}),
    ("r014 和 r013 的区别", "diff", {"a": "r013", "b": "r014"}),
    ("花了多少算力", "spend", {}),
    ("每个 idea 用了多少 SU", "spend", {}),
    ("warmup-long 这个 idea 花了多少", "spend", {"idea": "warmup-long"}),
    ("预算还剩多少", "spend", {}),
    ("列出所有 run", "runs", {}),
    ("现在有哪些 run 在跑", "runs", {}),
    ("这个 idea 下面的实验一览", "runs", {}),
    ("有哪些待办", "todos", {}),
    ("agent 手上还有什么活", "todos", {}),
    ("训练循环是怎么写的", "inspect", {"view": "code"}),
    ("给我 DMD 损失的伪代码", "inspect", {"view": "pseudo"}),
    ("把数据加载那部分简化一下给我看", "inspect", {"view": "simple"}),
    ("整个仓库的架构图", "inspect", {"view": "map"}),
    ("r012 当时跑的那版代码里 lr schedule 怎么算的", "inspect", {"run": "r012"}),
    ("模型的 forward 里注意力是怎么实现的", "inspect", {"view": "code"}),
    ("画一下系统设计", "inspect", {"view": "map"}),
    ("r012 的前 20 个奇异值", "svd", {"run": "r012", "k": 20}),
    ("r011 权重的奇异值谱，对数坐标", "svd", {"run": "r011", "log": True}),
    ("看看 r013 前 50 个奇异值", "svd", {"run": "r013", "k": 50}),
    ("r011 第 9 到 12 层的注意力熵", "attn_entropy", {"run": "r011", "layers": [9, 10, 11, 12]}),
    ("r012 每层 attention 熵", "attn_entropy", {"run": "r012"}),
    ("attention 有没有塌成 one-hot，看 r011", "attn_entropy", {"run": "r011"}),
    ("从 r013 再跑一个 warmup 16000 的", "run", {"from_": "r013", "set": {"warmup": 16000}}),
    ("基于 r012 把 lr 改成 1e-3 再跑一次", "run", {"from_": "r012", "set": {"lr": 0.001}}),
    ("r011 换三个 seed 重跑", "run", {"from_": "r011"}),
    ("用 r013 的配置，batch size 翻倍", "run", {"from_": "r013"}),
    ("把 r012 的 dropout 关掉试试", "run", {"from_": "r012"}),
    ("比较 r012 和 r013 的 val/loss 曲线", "plot", {"runs": ["r012", "r013"], "key": "val/loss"}),
    ("r013 最新的 val/acc 怎么样了", "plot", {"runs": ["r013"], "key": "val/acc"}),
    ("前 2k 步的尖峰在 r011 r012 里对比", "plot", {"runs": ["r011", "r012"]}),
    ("今天天气怎么样", None, {}),
    ("帮我写一封邮件给导师", None, {}),
    ("这个尖峰可能是什么原因", None, {}),
    ("把 r011 删掉", None, {}),
    ("给 r014 签字", None, {}),
    ("提交到集群跑", None, {}),
]
assert len(CASES) == 50

SYSTEM = f"""你把研究者的一句话翻译成 Arboria 的一条命令，只输出一个 JSON 对象，不要解释。
格式：{{"command": "<命令名>", "args": {{...}}}}；没有合适的命令时输出 {{"command": null}}。
只能用下面的命令和参数，参数名照抄，没提到的参数不写。
命令：
{json.dumps(COMMANDS, ensure_ascii=False)}
run（实验）：
{json.dumps(RUNS, ensure_ascii=False)}
指标：{", ".join(METRICS)}
注意：签字、删除、提交到集群都不是命令（返回 null）；改参数再跑一次用 run。
例子：
"r011 和 r012 的 loss 对比" → {{"command": "plot", "args": {{"runs": ["r011", "r012"], "key": "val/loss"}}}}
"r013 跟 r012 哪里不一样" → {{"command": "diff", "args": {{"a": "r012", "b": "r013"}}}}
"晚饭吃什么" → {{"command": null}}"""


def parse(text: str):
    text = re.sub(r"<think>.*?</think>", "", text, flags=re.S)
    m = re.search(r"\{.*\}", text, flags=re.S)
    if not m:
        return None
    try:
        return json.loads(m.group(0))
    except json.JSONDecodeError:
        return None


def valid(ans) -> bool:
    """Shape check against the signatures: the gate a ghost suggestion must pass."""
    if not isinstance(ans, dict) or "command" not in ans:
        return False
    if ans["command"] is None:
        return True
    spec = COMMANDS.get(ans["command"])
    args = ans.get("args", {}) or {}
    if spec is None or not isinstance(args, dict) or any(k not in spec["args"] for k in args):
        return False
    runs = [*(args.get("runs") or []), *[args[k] for k in ("a", "b", "run", "from_") if isinstance(args.get(k), str)]]
    return all(r in RUNS for r in runs) and (args.get("key") is None or args["key"] in METRICS)


def args_ok(got: dict, want: dict) -> bool:
    for k, v in want.items():
        g = got.get(k)
        if isinstance(v, list) and isinstance(g, list):
            if sorted(map(str, v)) != sorted(map(str, g)):
                return False
        elif isinstance(v, dict) and isinstance(g, dict):
            if any(str(g.get(kk)) != str(vv) and not _num_eq(g.get(kk), vv) for kk, vv in v.items()):
                return False
        elif g != v and not _num_eq(g, v):
            return False
    return True


def _num_eq(a, b) -> bool:
    try:
        return abs(float(a) - float(b)) < 1e-12
    except (TypeError, ValueError):
        return False


def bench(model_id: str, max_tokens: int) -> dict:
    from mlx_lm import generate, load

    t0 = time.time()
    model, tok = load(model_id)
    load_s = time.time() - t0

    def ask(intent: str) -> tuple[str, float]:
        msgs = [{"role": "system", "content": SYSTEM}, {"role": "user", "content": intent}]
        try:
            prompt = tok.apply_chat_template(msgs, add_generation_prompt=True, enable_thinking=False)
        except TypeError:
            prompt = tok.apply_chat_template(msgs, add_generation_prompt=True)
        t = time.time()
        out = generate(model, tok, prompt=prompt, max_tokens=max_tokens, verbose=False)
        return out, time.time() - t

    ask("热身")  # warm-up, not counted
    rows, lat = [], []
    for intent, want_cmd, want_args in CASES:
        out, s = ask(intent)
        lat.append(s)
        ans = parse(out)
        ok_json, ok_shape = ans is not None, valid(ans)
        got_cmd = ans.get("command") if isinstance(ans, dict) else "?"
        ok_cmd = ok_shape and got_cmd == want_cmd
        ok_args = ok_cmd and (want_cmd is None or args_ok(ans.get("args") or {}, want_args))
        rows.append({"intent": intent, "want": want_cmd, "got": ans, "raw": out.strip()[:200], "json": ok_json,
                     "valid": ok_shape, "cmd": ok_cmd, "args": ok_args, "secs": round(s, 3)})
    lat.sort()
    n = len(rows)
    null_cases = [r for r in rows if r["want"] is None]
    return {
        "model": model_id, "load_s": round(load_s, 1),
        "json_ok": sum(r["json"] for r in rows) / n, "valid": sum(r["valid"] for r in rows) / n,
        "command_ok": sum(r["cmd"] for r in rows) / n, "args_ok": sum(r["args"] for r in rows) / n,
        "null_ok": sum(r["cmd"] for r in null_cases) / len(null_cases),
        "false_suggest": sum(1 for r in null_cases if r["valid"] and r["got"].get("command")) / len(null_cases),
        "p50_ms": round(statistics.median(lat) * 1000), "p95_ms": round(lat[int(0.95 * n) - 1] * 1000),
        "rows": rows,
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--models", nargs="+", default=["mlx-community/Qwen3.5-2B-4bit", "mlx-community/Qwen3.5-4B-4bit"])
    ap.add_argument("--max-tokens", type=int, default=96)
    ap.add_argument("--out", default="intent_bench_results.json")
    a = ap.parse_args()
    results = [bench(m, a.max_tokens) for m in a.models]
    print(f"\n{'模型':38} {'JSON':>5} {'合法':>5} {'命令对':>6} {'参数对':>6} {'该空则空':>8} {'误建议':>6} {'p50':>7} {'p95':>7}")
    for r in results:
        print(f"{r['model']:38} {r['json_ok']:5.0%} {r['valid']:5.0%} {r['command_ok']:6.0%} {r['args_ok']:6.0%} "
              f"{r['null_ok']:8.0%} {r['false_suggest']:6.0%} {r['p50_ms']:5d}ms {r['p95_ms']:5d}ms")
    for r in results:
        bad = [x for x in r["rows"] if not x["args"]]
        print(f"\n{r['model']} 错的 {len(bad)} 条：")
        for x in bad:
            print(f"  · {x['intent']}  → 期望 {x['want']}，得到 {json.dumps(x['got'], ensure_ascii=False) if x['got'] else x['raw']!r}")
    Path(a.out).write_text(json.dumps(results, ensure_ascii=False, indent=2))
    print(f"\n完整结果：{a.out}")


if __name__ == "__main__":
    main()
