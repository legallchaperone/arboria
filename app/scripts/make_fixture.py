"""Write src/fixtures/board.json: the app's screenshot data (the 主界面画布 example), in
the exact shape of the core's view structs (crates/arboria-core/src/view.rs; its test
compares key order and types). Run from app/ after changing either."""

import json
from pathlib import Path

NOW = "2026-11-04T12:00:00Z"
ROOT = "/Users/you/lm"
HOME = "/Users/you/arboria"


def project(root, name, kind, pid, total):
    return {"root": root, "name": name, "kind": kind, "project_id": pid, "total_su": total,
            "config_error": None, "loaded_at": NOW}


def idea(i, name, stage, hyp, metric, parent, from_run, created, closed, last, su, runs,
         conf="guess", gist=None):
    wt = None if stage == "seed" else f"{ROOT}-{name}"
    nb = f"{HOME}/seeds/{name}.py" if stage == "seed" else f"{ROOT}/.arboria/notebooks/{name}.py"
    return {"id": i, "name": name, "stage": stage, "confidence": conf, "gist": gist,
            "hypothesis": hyp, "metric": metric, "parent": parent, "from_run": from_run,
            "created_at": created, "closed_at": closed, "last_active": last, "su_used": su,
            "runs": runs, "worktree": wt if stage == "open" else None, "notebook": nb}


def job(seg, jid, state, el, su):
    return {"segment": seg, "job_id": jid, "state": state, "elapsed_s": el, "billed_su": su}


def run(rid, idea_, status, reason, created, updated, su, est, finished=None, summary=None,
        parent=None, gpus=4, wall="12h"):
    jobs = []
    if status not in ("awaiting_approval", "rejected"):
        state = {"succeeded": "completed", "failed": "failed", "running": "running",
                 "cancelled": "cancelled"}.get(status, "pending")
        jobs = [job(1, str(4400000 + int(rid[1:])), state, int(su / 4 * 3600) if su else None,
                    su or None)]
    return {"id": rid, "idea": idea_, "parent_run": parent, "status": status,
            "change_reason": reason, "machine": "deltaai", "partition": "ghx4", "gpus": gpus,
            "walltime": wall, "est_su": est, "billed_su": su, "summary": summary,
            "wandb_url": None, "created_at": created, "updated_at": updated,
            "finished_at": finished, "jobs": jobs}


def item(kind, when, title, meta, idea_=None, run_=None, task=None, lesson=None, probe=None,
         command=None):
    return {"kind": kind, "when": when, "title": title, "meta": meta, "idea": idea_,
            "run": run_, "task": task, "lesson": lesson, "probe": probe, "command": command}


CARD = """审核单 r014（idea warmup-long）
检查:
  ✓ 快照 7c41e09ab2d3（不含被 .gitignore 忽略的文件）
  ✓ 账户 bjki-dtai-gh
  ✓ 计费卡数 = 申请卡数 = 4
资源:
  机器 deltaai（aarch64），分区 ghx4，4 GPU
  每节点 CPU 288，内存 443.592G
  时长 12h
  命令 python train.py warmup=16000
  项目 lm-3f2a：作业名 arb-lm-3f2a-r014-s1，远程目录 /projects/bjki/you/runs/lm-3f2a/r014
估计:
  上限 4 卡 × 12.00 h × 1 = 48.00 SU（按申请的时长算满）
预算:
  项目已用 115.9 / 400 SU，剩余 284.1
  本 idea 已用 35 SU
审核单哈希: 3f9ac21e77b04d1c"""

ideas = [
    idea(1, "data-dedup", "merged", "去重后的数据让 val/loss 降 0.05", "val/loss", None, None,
         "2026-10-20T09:00:00Z", "2026-10-26T18:00:00Z", "2026-10-26T18:00:00Z", 46.2, 5,
         conf="confirmed"),
    idea(2, "no-dropout", "dropped", "去掉 dropout 收敛更快", "val/loss", "data-dedup", "r003",
         "2026-10-22T10:00:00Z", "2026-10-27T12:00:00Z", "2026-10-27T12:00:00Z", 10.6, 2,
         conf="rejected"),
    idea(3, "label-smooth", "open", "label smoothing 0.1 改善 ppl", "val/ppl", "data-dedup",
         "r005", "2026-10-27T09:00:00Z", None, "2026-11-03T11:00:00Z", 24.1, 3, conf="prelim"),
    idea(4, "warmup-long", "open", "warmup 拉长到 8k，前 3k 步的尖峰会消失", "val/loss",
         "data-dedup", "r005", "2026-10-28T09:00:00Z", None, "2026-11-04T11:48:00Z", 35.0, 4),
]
runs = [
    run("r001", "data-dedup", "succeeded", "基线", "2026-10-20T10:00:00Z", "2026-10-20T22:00:00Z", 9.4, 48, "2026-10-20T22:00:00Z", "val/loss 3.02"),
    run("r002", "data-dedup", "succeeded", "去重 v1", "2026-10-21T10:00:00Z", "2026-10-21T22:00:00Z", 9.1, 48, "2026-10-21T22:00:00Z"),
    run("r003", "data-dedup", "succeeded", "去重 v2（minhash）", "2026-10-22T09:00:00Z", "2026-10-22T21:00:00Z", 9.3, 48, "2026-10-22T21:00:00Z"),
    run("r004", "data-dedup", "succeeded", "v2 + 长序列", "2026-10-23T09:00:00Z", "2026-10-23T21:00:00Z", 9.2, 48, "2026-10-23T21:00:00Z"),
    run("r005", "data-dedup", "succeeded", "v2 定稿", "2026-10-24T09:00:00Z", "2026-10-24T21:00:00Z", 9.2, 48, "2026-10-24T21:00:00Z", "val/loss 2.97"),
    run("r006", "no-dropout", "succeeded", "dropout 0", "2026-10-25T09:00:00Z", "2026-10-25T15:00:00Z", 5.3, 24, "2026-10-25T15:00:00Z", None, "r003", 4, "6h"),
    run("r007", "no-dropout", "succeeded", "dropout 0 + wd 0.1", "2026-10-26T09:00:00Z", "2026-10-26T15:00:00Z", 5.3, 24, "2026-10-26T15:00:00Z", None, "r006", 4, "6h"),
    run("r008", "label-smooth", "succeeded", "smoothing 0.1", "2026-10-29T09:00:00Z", "2026-10-29T21:00:00Z", 11.7, 48, "2026-10-29T21:00:00Z"),
    run("r009", "label-smooth", "succeeded", "smoothing 0.05", "2026-10-31T09:00:00Z", "2026-10-31T21:00:00Z", 11.78, 48, "2026-10-31T21:00:00Z"),
    run("r010", "label-smooth", "failed", "baseline + bf16", "2026-11-03T09:00:00Z", "2026-11-03T09:40:00Z", 0.62, 48, "2026-11-03T09:40:00Z"),
    run("r011", "warmup-long", "succeeded", "warmup 2k", "2026-11-01T09:00:00Z", "2026-11-01T21:00:00Z", 11.6, 48, "2026-11-01T21:00:00Z", "val/loss 2.915，1.2k–3k 步三次尖峰"),
    run("r012", "warmup-long", "succeeded", "warmup 4k", "2026-11-02T09:00:00Z", "2026-11-04T10:00:00Z", 11.86, 48, "2026-11-04T10:00:00Z", "val/loss 2.879，没有尖峰", "r011"),
    run("r013", "warmup-long", "running", "warmup 8k", "2026-11-03T12:00:00Z", "2026-11-04T11:30:00Z", 11.54, 48, None, None, "r011"),
    run("r014", "warmup-long", "awaiting_approval", "warmup 16k", "2026-11-04T11:48:00Z", "2026-11-04T11:48:00Z", 0.0, 48, None, None, "r013"),
]
inbox = [
    item("approve", "2026-11-04T11:48:00Z", "r014 待签字",
         "warmup-long · 4 GPU × 12h · deltaai · ≈ 48 SU · 预算剩 284.1 SU", "warmup-long", "r014",
         command="arb approve r014"),
    item("failed", "2026-11-03T09:40:00Z", "r010 失败",
         "label-smooth · baseline + bf16 · 第 1 段 · CUDA out of memory · 用了 0.62 SU",
         "label-smooth", "r010"),
    item("finished", "2026-11-04T10:00:00Z", "r012 跑完了",
         "warmup-long · warmup 4k · val/loss 2.879，没有尖峰 · 花了 11.86 SU", "warmup-long", "r012"),
    item("agent_done", "2026-11-04T09:55:00Z", "agent 写好了命令（t007）",
         "warmup-long · svd：前 20 个奇异值 · 写好了 lab.svd(run, step=…)", "warmup-long", task="t007"),
]
card = {"run": "r014", "text": CARD, "card_hash": "3f9ac21e77b04d1c" + "0" * 48, "matches": True,
        "est_su": 48.0,
        "resources": ["机器 deltaai（aarch64），分区 ghx4，4 GPU", "每节点 CPU 288，内存 443.592G",
                      "时长 12h", "命令 python train.py warmup=16000",
                      "项目 lm-3f2a：作业名 arb-lm-3f2a-r014-s1，远程目录 /projects/bjki/you/runs/lm-3f2a/r014"],
        "estimate": ["上限 4 卡 × 12.00 h × 1 = 48.00 SU（按申请的时长算满）"],
        "budget": ["项目已用 115.9 / 400 SU，剩余 284.1", "本 idea 已用 35 SU"],
        "error": None}
todos = [
    {"id": "t007", "idea": "warmup-long", "kind": "command_new", "status": "done",
     "text": "svd：前 20 个奇异值", "created_at": "2026-11-04T09:30:00Z",
     "done_at": "2026-11-04T09:55:00Z", "result": "写好了 lab.svd(run, step=…)"},
    {"id": "t008", "idea": "warmup-long", "kind": "inspect", "status": "running",
     "text": "attention logit 在哪里缩放", "created_at": "2026-11-04T11:00:00Z",
     "done_at": None, "result": None},
]
notes = [
    {"id": 3, "idea": "warmup-long", "run": "r011", "text": "r011 在 1.2k 到 3k 步之间有三次尖峰，都出现在 lr 爬到 2e-4 左右的时候。",
     "author": "human", "created_at": "2026-11-02T08:00:00Z"},
    {"id": 2, "idea": "label-smooth", "run": None, "text": "0.05 和 0.1 差不多，先看 bf16 能不能省时间。",
     "author": "human", "created_at": "2026-11-01T10:00:00Z"},
    {"id": 1, "idea": "data-dedup", "run": "r005", "text": "minhash 去重定稿，合进 main。",
     "author": "human", "created_at": "2026-10-26T17:00:00Z"},
]
board = {
    "project": project(ROOT, "lm", "project", "lm-3f2a", 400.0),
    "ideas": ideas, "runs": runs, "inbox": inbox, "cards": [card], "todos": todos,
    "notes": notes,
    "lessons": [],
    "notifications": [
        {"at": "2026-11-04T10:00:05Z", "text": "r012 已成功（11.86 SU）"},
        {"at": "2026-11-03T09:40:10Z", "text": "r010 失败：s1 日志出现异常（匹配 CUDA out of memory）"},
    ],
    "spend": {"used": 115.9, "total": 400.0,
              "by_idea": [{"idea": "data-dedup", "su": 46.2}, {"idea": "warmup-long", "su": 35.0},
                          {"idea": "label-smooth", "su": 24.1}, {"idea": "no-dropout", "su": 10.6}]},
    "probes": [
        {"machine": "deltaai", "probe": "builtin:balance", "at": "2026-11-04T11:40:00Z",
         "result": {"kind": "number", "label": "余额", "value": 284, "of": 400, "unit": "SU"}},
        {"machine": "deltaai", "probe": "project:queue", "at": "2026-11-04T11:40:00Z",
         "result": {"kind": "table", "label": "排队", "columns": ["分区", "排队", "运行"],
                    "rows": [["ghx4", 31, 112], ["ghx4-interactive", 2, 9]]}},
    ],
}
home = {
    "project": project(HOME, "种子园", "home", None, 0.0),
    "ideas": [idea(1, "rank-schedule", "seed", None, "val/loss", None, None,
                   "2026-10-30T09:00:00Z", None, "2026-11-02T09:00:00Z", 0.0, 0,
                   gist="LoRA 的 rank 随训练增长，前期小 rank 省算力")],
    "runs": [], "inbox": [], "cards": [], "todos": [], "notes": [], "lessons": [],
    "notifications": [], "spend": {"used": 0.0, "total": 0.0, "by_idea": []}, "probes": [],
}


def summary(b):
    p = b["project"]
    return {"root": p["root"], "name": p["name"], "kind": p["kind"], "error": None,
            "open_ideas": sum(i["stage"] == "open" for i in b["ideas"]),
            "seeds": sum(i["stage"] == "seed" for i in b["ideas"]), "inbox": b["inbox"]}


out = {
    "now": NOW,
    "platform": {"os": "macos", "touch_id": True, "touch_id_why": None, "in_claude_code": None, "min_shown_ms": 2000},
    "helper": {"service": "/Users/you/Library/LaunchAgents/world.cwx.arboria.helper.plist",
               "installed": True, "running": True, "last_tick": "2026-11-04T11:58:00Z",
               "needs_login": {}, "errors": []},
    "projects": [summary(board), summary(home)],
    "boards": [board, home],
}
Path("src/fixtures/board.json").write_text(json.dumps(out, ensure_ascii=False, indent=2) + "\n")
