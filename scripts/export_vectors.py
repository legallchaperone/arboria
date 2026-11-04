#!/usr/bin/env python3
"""Export cross-language test vectors from the Python implementation (`src/arboria/`).

    uv run python scripts/export_vectors.py          # (re)write tests/vectors/*.json
    uv run python scripts/export_vectors.py --check  # exit 1 if a committed file differs

Every case runs the real Python function; nothing here re-implements arboria logic.
`tests/test_vectors.py` regenerates all files in memory and compares them byte for byte
with the committed ones, so the vectors cannot drift from Python. See tests/vectors/README.md.

File format (fixed; the Rust loader depends on it):
    {"name", "version": 1, "source": "arboria.<module>.<function>", "notes",
     "cases": [{"id", "input": {...}, "output": <json>} | {"id", "input", "error": "Cls: msg"}]}
written with sort_keys, ensure_ascii=False, indent=2 and a trailing newline.
"""

from __future__ import annotations

import ast
import json
import os
import shlex
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import asdict
from pathlib import Path, PurePosixPath
from types import SimpleNamespace
from typing import Any, Callable

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT / "src") not in sys.path:  # also works when the editable install is broken
    sys.path.insert(0, str(ROOT / "src"))

from arboria import (  # noqa: E402
    commands, config, db, ideas, inspection, machine, monitor, records, slurm, submission,
)
from arboria.backends import base as backend_base  # noqa: E402
from arboria.backends import ssh as backend_ssh  # noqa: E402
from arboria.clusters import Profile, check_remote_path, parse_profile  # noqa: E402
from arboria.errors import ArbError  # noqa: E402

VERSION = 1
OUT_DIR = ROOT / "tests" / "vectors"
FIXTURE_PROBE = ROOT / "tests" / "fixtures" / "deltaai_probe.txt"


# ================================================================ framework


class Vectors:
    """One vector file: cases are (id, input, thunk); the thunk calls the real code."""

    def __init__(self, name: str, source: str, notes: str):
        self.name, self.source, self.notes = name, source, notes
        self.cases: list[dict[str, Any]] = []

    def add(self, case_id: str, input: dict[str, Any], thunk: Callable[[], Any],
            errors: tuple[type[BaseException], ...] = (ArbError,)) -> None:
        if any(c["id"] == case_id for c in self.cases):
            raise ValueError(f"{self.name}: duplicate case id {case_id!r}")
        case: dict[str, Any] = {"id": case_id, "input": input}
        try:
            case["output"] = thunk()
        except errors as exc:
            case["error"] = f"{type(exc).__name__}: {exc}"
        self.cases.append(case)

    def document(self) -> dict[str, Any]:
        return {"name": self.name, "version": VERSION, "source": self.source,
                "notes": self.notes, "cases": self.cases}


def dumps(doc: Any) -> str:
    return json.dumps(doc, sort_keys=True, ensure_ascii=False, indent=2, allow_nan=False) + "\n"


# ================================================================ shared inputs

DELTAAI_TOML = """\
[deltaai]
kind = "slurm"
host = "deltaai"
arch = "aarch64"
account = "bjki-dtai-gh"
gpus_per_node = 4
cpus_per_gpu = 72
mem_per_gpu_gb = 110.898
default_partition = "ghx4"
max_segments = 4
balance_cmd = "accounts"
billing_per_su = 1000
prelude = []
env_activate = []
[deltaai.partitions.ghx4]
max_walltime = "48h"
charge = 1.0
[deltaai.partitions."ghx4-interactive"]
max_walltime = "2h"
charge = 2.0
[deltaai.paths]
runs = "/projects/bjki/$USER/arboria/runs"
data = "/work/nvme/bjki/$USER/data"
cache = "/projects/bjki/$USER/arboria/cache"
ckpt = "/work/hdd/bjki/$USER/arboria/ckpt"
out = "/projects/bjki/$USER/arboria/out"
"""

# Hypothetical environment lines (the user's real env is not settled yet, see CLAUDE.md).
DELTAAI_UV_TOML = DELTAAI_TOML.replace("[deltaai]", "[deltaai-uv]").replace(
    "[deltaai.", "[deltaai-uv.").replace(
    "prelude = []",
    'prelude = ["module load cuda", "export UV_CACHE_DIR=/work/hdd/bjki/$USER/uv-cache"]').replace(
    "env_activate = []",
    'env_activate = ["source /work/hdd/bjki/$USER/venvs/WaveFront-gh/bin/activate"]').replace(
    "/projects/bjki/$USER/arboria/out", "/projects/bjki/${USER}/arboria/out")

FAKE_TOML = """\
[fake]
kind = "fake"
root = "/tmp/arboria-fake/state"
arch = "aarch64"
account = "acct1"
gpus_per_node = 4
cpus_per_gpu = 16
mem_per_gpu_gb = 100
default_partition = "ghx4"
max_segments = 2
[fake.partitions.ghx4]
max_walltime = "48h"
[fake.partitions."ghx4-interactive"]
max_walltime = "2h"
charge = 2
[fake.paths]
runs = "/tmp/arboria-fake/remote/runs"
data = "/tmp/arboria-fake/remote/data"
cache = "/tmp/arboria-fake/remote/cache"
ckpt = "/tmp/arboria-fake/remote/ckpt"
out = "/tmp/arboria-fake/remote/out"
"""

EXAMPLE_TOML = (ROOT / "examples" / "clusters.toml").read_text(encoding="utf-8")


def load(toml_text: str, name: str, source: str = "clusters.toml") -> Profile:
    return parse_profile(name, tomllib.loads(toml_text)[name], source)


def profile_json(p: Profile) -> dict[str, Any]:
    """Normalized Profile (the shape `clusters_parse_profile` outputs and other vectors take)."""
    return {
        "name": p.name, "kind": p.kind, "host": p.host, "arch": p.arch, "account": p.account,
        "account_set": p.account_set, "gpus_per_node": p.gpus_per_node,
        "cpus_per_gpu": p.cpus_per_gpu, "mem_per_gpu_gb": p.mem_per_gpu_gb,
        "default_partition": p.default_partition, "max_segments": p.max_segments,
        "balance_cmd": p.balance_cmd, "billing_per_su": p.billing_per_su,
        "prelude": list(p.prelude), "env_activate": list(p.env_activate),
        "partitions": {n: {"name": q.name, "max_walltime_s": q.max_walltime_s,
                           "charge": q.charge} for n, q in p.partitions.items()},
        "partition_order": list(p.partitions),
        "paths": dict(p.paths), "root": p.root, "source": p.source,
    }


DELTAAI = load(DELTAAI_TOML, "deltaai")
DELTAAI_UV = load(DELTAAI_UV_TOML, "deltaai-uv")
FAKE = load(FAKE_TOML, "fake")
# the shipped example: account is a <placeholder> (paths filled in so it parses)
DELTAAI_UNSET = load(EXAMPLE_TOML.replace("<proj>", "bjki"), "deltaai", "examples/clusters.toml")
PROFILES = {"deltaai": DELTAAI, "deltaai-uv": DELTAAI_UV, "fake": FAKE,
            "deltaai-unset": DELTAAI_UNSET}

PROJECT = "wavefront-3f2a"
PROJECT_31 = "a" * 26 + "-0b1c"  # the longest project id PROJECT_RE allows
COMMIT_A = "5f0c2e7b9a1d4c3e8f6a0b2d4e6f8a1c3e5b7d9f"
COMMIT_B = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4"
TRAIN = ["python", "train.py"]


def ask_of(command=TRAIN, gpus=1, nodes=1, walltime_s=3600, partition="ghx4",
           machine="deltaai", mem_gb=None, cpus=None, repeat=None) -> slurm.Ask:
    return slurm.Ask(command=list(command), gpus=gpus, nodes=nodes, walltime_s=walltime_s,
                     partition=partition, machine=machine, mem_gb=mem_gb, cpus=cpus,
                     repeat=repeat)


# (id, profile key, ask, project, snapshot commit): realistic plans, as `arb submit` builds
# them (submit fills partition = default_partition, nodes = 1, mem/cpus = None unless given)
PLANS: list[tuple[str, str, slurm.Ask, str, str]] = [
    ("1gpu-1h", "deltaai", ask_of(), PROJECT, COMMIT_A),
    ("1gpu-1h-hydra", "deltaai", ask_of(command=[
        "python", "train.py", "model=dmd", "optim.lr=0.0003", "trainer.max_steps=200000"]),
     PROJECT, COMMIT_A),
    ("4gpu-12h", "deltaai", ask_of(gpus=4, walltime_s=12 * 3600), PROJECT, COMMIT_A),
    ("2nodes-8gpu-24h", "deltaai", ask_of(gpus=8, nodes=2, walltime_s=24 * 3600), PROJECT,
     COMMIT_B),
    ("1gpu-72h-2segments", "deltaai", ask_of(walltime_s=72 * 3600), PROJECT, COMMIT_A),
    ("4gpu-7d-4segments", "deltaai", ask_of(gpus=4, walltime_s=7 * 86400), PROJECT, COMMIT_A),
    ("4gpu-48h-exact-limit", "deltaai", ask_of(gpus=4, walltime_s=48 * 3600), PROJECT, COMMIT_A),
    ("48h1s-2segments", "deltaai", ask_of(walltime_s=48 * 3600 + 1), PROJECT, COMMIT_A),
    ("interactive-2h-charge2", "deltaai", ask_of(gpus=2, walltime_s=7200,
                                                 partition="ghx4-interactive"), PROJECT, COMMIT_A),
    ("mem110-1gpu-not-over", "deltaai", ask_of(mem_gb=110), PROJECT, COMMIT_A),
    ("mem111-1gpu-billed2", "deltaai", ask_of(mem_gb=111), PROJECT, COMMIT_A),
    ("mem221-2gpu-not-over", "deltaai", ask_of(gpus=2, mem_gb=221), PROJECT, COMMIT_A),
    ("mem222-2gpu-billed3", "deltaai", ask_of(gpus=2, mem_gb=222), PROJECT, COMMIT_A),
    ("mem443-4gpu-default-max", "deltaai", ask_of(gpus=4, mem_gb=443), PROJECT, COMMIT_A),
    ("cpus72-1gpu", "deltaai", ask_of(cpus=72), PROJECT, COMMIT_A),
    ("cpus73-1gpu-billed2", "deltaai", ask_of(cpus=73), PROJECT, COMMIT_A),
    ("cpus288-1gpu-billed4", "deltaai", ask_of(cpus=288, walltime_s=1800), PROJECT, COMMIT_A),
    ("small-mem-cpus", "deltaai", ask_of(mem_gb=8, cpus=4), PROJECT, COMMIT_A),
    ("repeat-nonce", "deltaai", ask_of(repeat="a1b2c3d4"), PROJECT, COMMIT_A),
    ("project-31-chars", "deltaai", ask_of(), PROJECT_31, COMMIT_A),
    ("command-quoting", "deltaai", ask_of(command=[
        "python", "-c", "print('hi there')", "--name=学习率 实验", "$HOME", "a\"b", "it's"]),
     PROJECT, COMMIT_A),
    ("argparse-overrides", "deltaai", ask_of(command=[
        "python", "train.py", "--lr", "0.0001", "--batch-size", "64", "--use-ema", "true"]),
     PROJECT, COMMIT_A),
    ("uv-env-prelude", "deltaai-uv", ask_of(gpus=4, walltime_s=6 * 3600, machine="deltaai-uv"),
     PROJECT, COMMIT_A),
    ("fake-machine", "fake", ask_of(gpus=2, walltime_s=60 * 3600, machine="fake"), PROJECT,
     COMMIT_A),
    ("min-walltime-60s", "deltaai", ask_of(walltime_s=60), PROJECT, COMMIT_A),
    ("est-su-tie-0.125", "deltaai", ask_of(walltime_s=450), PROJECT, COMMIT_A),
    ("est-su-tie-0.375", "deltaai", ask_of(walltime_s=1350), PROJECT, COMMIT_A),
    ("est-su-tie-0.625", "deltaai", ask_of(walltime_s=2250), PROJECT, COMMIT_A),
    ("est-su-odd-seconds", "deltaai", ask_of(gpus=3, walltime_s=12345,
                                             partition="ghx4-interactive"), PROJECT, COMMIT_A),
]

PLAN_INPUT_NOTE = (
    "input.profile is the normalized Profile (the output shape of clusters_parse_profile); "
    "input.ask is dataclasses.asdict(slurm.Ask). `arb submit` fills the Ask before build_plan: "
    "partition = profile.default_partition, nodes = 1, mem_gb/cpus = null unless given, "
    "walltime_s = parse_walltime(--time), repeat = secrets.token_hex(4) with --repeat. "
)


def plan_input(profile_key: str, ask: slurm.Ask, project: str, **extra: Any) -> dict[str, Any]:
    return {"profile": profile_json(PROFILES[profile_key]), "ask": asdict(ask),
            "project": project, **extra}


def build(profile_key: str, ask: slurm.Ask, project: str) -> slurm.Plan:
    return slurm.build_plan(PROFILES[profile_key], ask, project)


# ================================================================ JSON / card hash


def v_canonical_json() -> Vectors:
    v = Vectors("canonical_json", "arboria.slurm.canonical_json",
                "json.dumps(obj, sort_keys=True, separators=(',', ':'), ensure_ascii=False): "
                "the card hash input. Floats use Python repr (shortest round-trip; exponent form "
                "when exp < -4 or >= 16, written like 1e-05 / 1e+16; integral floats keep '.0'). "
                "Keys sort by code point (= UTF-8 byte order). Only \" \\\\ and U+0000..U+001F are "
                "escaped (\\n \\r \\t \\b \\f short forms, others \\u00xx lowercase); U+007F, "
                "U+2028 and non-ASCII are written raw. NaN/Infinity and ints outside i64/u64 are "
                "not representable in the vector files and are left out.")
    cases: list[tuple[str, Any]] = [
        ("mem-per-gpu", 110.898), ("1e-4", 1e-4), ("6e-4", 6e-4), ("1e-5", 1e-5),
        ("0.1+0.2", 0.1 + 0.2), ("float-integral", 1.0), ("float-2.0", 2.0),
        ("neg-zero", -0.0), ("1e16", 1e16), ("9.999e15", 9999999999999998.0),
        ("1.5e300", 1.5e300), ("5e-324", 5e-324), ("max-float", 1.7976931348623157e308),
        ("third", 1 / 3), ("su-0.13", 0.13), ("su-raw", 1803 * 259 / 3600 / 1000),
        ("int", 42), ("neg-int", -7), ("i64-max", 2 ** 63 - 1), ("i64-min", -(2 ** 63)),
        ("u64-max", 2 ** 64 - 1), ("zero", 0),
        ("null", None), ("true", True), ("false", False),
        ("empty-string", ""), ("ascii", "python train.py"),
        ("chinese", "批准这个 run 上集群吗？"), ("emoji-astral", "😀 ok"),
        ("escapes", "q\"b\\s/n\nr\rt\tb\bf\f"), ("controls", "\x00\x01\x1f\x7f"),
        ("line-sep", "a b c"), ("dollar-user", "/projects/bjki/$USER/arboria/runs"),
        ("empty-list", []), ("empty-dict", {}),
        ("nested", {"b": [1, 2.5, {"z": None, "a": [True, False]}], "a": {"y": "", "x": [[]]}}),
        ("key-order", {"b": 1, "a": 2, "B": 3, "_": 4, "a1": 5, "a-": 6, "10": 7, "9": 8}),
        ("key-order-unicode", {"é": 1, "z": 2, "中": 3, "￿": 4, "😀": 5, "Z": 6}),
        ("list-order-kept", ["b", "a", 3, 1.0]),
    ]
    for cid, obj in cases:
        v.add(cid, {"obj": obj}, lambda obj=obj: slurm.canonical_json(obj))
    for cid, key, commit, proj in [("card-payload-1gpu", "deltaai", COMMIT_A, PROJECT),
                                   ("card-payload-uv", "deltaai-uv", COMMIT_A, PROJECT)]:
        ask = ask_of(machine=key)
        payload = slurm.card_payload(commit, build(key, ask, proj))
        v.add(cid, {"obj": payload}, lambda p=payload: slurm.canonical_json(p))
    return v


def v_card_payload() -> Vectors:
    v = Vectors("card_payload", "arboria.slurm.card_payload",
                PLAN_INPUT_NOTE + "output is card_payload(snapshot_commit, build_plan(profile, "
                "ask, project)); `repeat` is present only when ask.repeat is not null.")
    for cid, key, ask, proj, commit in PLANS:
        v.add(cid, plan_input(key, ask, proj, snapshot_commit=commit),
              lambda key=key, ask=ask, proj=proj, commit=commit:
              slurm.card_payload(commit, build(key, ask, proj)))
    return v


def v_card_hash() -> Vectors:
    v = Vectors("card_hash", "arboria.slurm.card_hash",
                PLAN_INPUT_NOTE + "output is sha256(canonical_json(card_payload(...)).encode("
                "'utf-8')).hexdigest(); the same case ids as card_payload.")
    for cid, key, ask, proj, commit in PLANS:
        v.add(cid, plan_input(key, ask, proj, snapshot_commit=commit),
              lambda key=key, ask=ask, proj=proj, commit=commit:
              slurm.card_hash(commit, build(key, ask, proj)))
    return v


def v_build_plan() -> Vectors:
    v = Vectors("build_plan", "arboria.slurm.build_plan",
                PLAN_INPUT_NOTE + "output is Plan.request() (what goes into runs.request_json "
                "before submit adds command_source/base_command/overrides/override_style/"
                "parent_run) plus three Plan fields: billed_per_node, max_walltime_s and "
                "over_billed. est_su = round(sum(billed_gpus * s / 3600 * charge), 2) with "
                "Python's round() (correctly rounded on the exact binary value, ties to even: "
                "0.125 -> 0.12, 0.375 -> 0.38). Default mem = floor(gpus_per_node_asked * "
                "mem_per_gpu_gb) (never rounded up: 4 x 110.898 -> 443).")

    def run(key: str, ask: slurm.Ask, proj: str) -> dict[str, Any]:
        plan = build(key, ask, proj)
        return plan.request() | {"billed_per_node": plan.billed_per_node,
                                 "max_walltime_s": plan.max_walltime_s,
                                 "over_billed": plan.over_billed}

    for cid, key, ask, proj, _ in PLANS:
        v.add(cid, plan_input(key, ask, proj), lambda key=key, ask=ask, proj=proj:
              run(key, ask, proj))
    bad = [
        ("err-no-command", "deltaai", ask_of(command=[]), PROJECT),
        ("err-empty-arg", "deltaai", ask_of(command=["python", ""]), PROJECT),
        ("err-account-unset", "deltaai-unset", ask_of(), PROJECT),
        ("err-nodes-0", "deltaai", ask_of(nodes=0), PROJECT),
        ("err-gpus-0", "deltaai", ask_of(gpus=0), PROJECT),
        ("err-gpus-not-divisible", "deltaai", ask_of(gpus=6, nodes=4), PROJECT),
        ("err-too-many-per-node", "deltaai", ask_of(gpus=8), PROJECT),
        ("err-walltime-59s", "deltaai", ask_of(walltime_s=59), PROJECT),
        ("err-unknown-partition", "deltaai", ask_of(partition="gpuA100x4"), PROJECT),
        ("err-mem-0", "deltaai", ask_of(mem_gb=0), PROJECT),
        ("err-cpus-0", "deltaai", ask_of(cpus=0), PROJECT),
        ("err-mem-more-than-node", "deltaai", ask_of(mem_gb=444), PROJECT),
        ("err-cpus-more-than-node", "deltaai", ask_of(cpus=289), PROJECT),
        ("err-too-many-segments", "deltaai", ask_of(walltime_s=8 * 86400 + 1), PROJECT),
        ("err-too-many-segments-fake", "fake", ask_of(machine="fake", walltime_s=97 * 3600),
         PROJECT),
        ("err-interactive-segments", "deltaai", ask_of(partition="ghx4-interactive",
                                                       walltime_s=9 * 3600), PROJECT),
        ("err-bad-project", "deltaai", ask_of(), "WaveFront"),
        ("err-project-too-long", "deltaai", ask_of(), "a" * 32),
    ]
    for cid, key, ask, proj in bad:
        v.add(cid, plan_input(key, ask, proj), lambda key=key, ask=ask, proj=proj:
              run(key, ask, proj))
    return v


# ================================================================ walltime, durations, billing


def v_parse_walltime() -> Vectors:
    v = Vectors("parse_walltime", "arboria.slurm.parse_walltime",
                "Seconds. Input is stripped and lowercased (Python str.strip()/lower(), Unicode). "
                "Python's \\d matches any Unicode decimal digit and int() accepts them, so "
                "non-ASCII digits are accepted (see the arabic/fullwidth cases). Error messages "
                "use Python repr() of the input.")
    for cid, text in [
        ("90m", "90m"), ("12h", "12h"), ("1d6h", "1d6h"), ("45s", "45s"), ("1d", "1d"),
        ("full-units", "1d2h3m4s"), ("upper", "12H"), ("spaces", "  2h  "), ("48h", "48h"),
        ("2h", "2h"), ("leading-zero", "007m"), ("big-hours", "100h"), ("7d", "7d"),
        ("clock", "01:30:00"), ("clock-days", "2-00:00:00"), ("clock-1day", "1-00:00:00"),
        ("clock-hours-over-24", "100:00:00"), ("clock-1-digit-hour", "1:00:00"),
        ("clock-seconds", "00:00:59"), ("clock-max-ms", "00:59:59"),
        ("err-clock-minutes-60", "00:60:00"), ("err-clock-seconds-60", "00:00:60"),
        ("err-clock-1-digit-min", "1:2:3"), ("err-clock-mm-ss", "30:00"),
        ("err-zero-units", "0h"), ("err-zero-clock", "00:00:00"), ("err-empty", ""),
        ("err-blank", "   "), ("err-order", "6h1d"), ("err-unit-only", "h"),
        ("err-decimal", "1.5h"), ("err-word", "forever"), ("err-negative", "-1h"),
        ("err-space-inside", "1d 6h"), ("err-quote-repr", "12h'"), ("err-chinese", "两小时"),
        ("arabic-digits", "١٢h"), ("fullwidth-digits", "１２h"), ("err-fullwidth-unit", "１２Ｈ"),
    ]:
        v.add(cid, {"text": text}, lambda text=text: slurm.parse_walltime(text))
    for cid, text in [("err-int", 3600), ("err-null", None), ("err-float", 1.5),
                      ("err-list", ["1h"])]:
        v.add(cid, {"text": text}, lambda text=text: slurm.parse_walltime(text))
    return v


def v_slurm_time() -> Vectors:
    v = Vectors("slurm_time", "arboria.slurm.slurm_time",
                "`--time` value HH:MM:SS; hours are not wrapped at 24 and may have more than two "
                "digits. Inputs are >= 0 (negatives are never passed).")
    for s in [0, 1, 59, 60, 61, 3599, 3600, 3661, 7200, 43200, 86399, 86400, 86401, 90000,
              172800, 172801, 360000, 604800, 3600 * 1000]:
        v.add(str(s), {"seconds": s}, lambda s=s: slurm.slurm_time(s))
    return v


def v_human_duration() -> Vectors:
    v = Vectors("human_duration", "arboria.slurm.human_duration",
                "Compact d/h/m/s, zero parts omitted, '0s' for 0. Inputs are >= 0.")
    for s in [0, 1, 45, 59, 60, 90, 3599, 3600, 5400, 3661, 7200, 43200, 86400, 108000, 129600,
              172800, 172801, 259200, 604800, 691201]:
        v.add(str(s), {"seconds": s}, lambda s=s: slurm.human_duration(s))
    return v


def v_segment_plan() -> Vectors:
    v = Vectors("segment_plan", "arboria.slurm.segment_plan",
                "n = max(1, ceil(total_s / max_s)) (true division); all segments are max_s "
                "except the last.")
    h = 3600
    for cid, total, mx, n in [
        ("1h-of-48h", h, 48 * h, 4), ("48h-exact", 48 * h, 48 * h, 4),
        ("48h+1s", 48 * h + 1, 48 * h, 4), ("72h", 72 * h, 48 * h, 4),
        ("96h-exact-2", 96 * h, 48 * h, 4), ("7d", 7 * 86400, 48 * h, 4),
        ("8d-exact-4", 8 * 86400, 48 * h, 4), ("err-8d+1s", 8 * 86400 + 1, 48 * h, 4),
        ("interactive-3h", 3 * h, 2 * h, 4), ("err-interactive-9h", 9 * h, 2 * h, 4),
        ("max-segments-1", 47 * h, 48 * h, 1), ("err-max-segments-1", 49 * h, 48 * h, 1),
        ("60s", 60, 2 * h, 4), ("odd-max", 1000, 333, 4), ("err-odd-max", 1333, 333, 4),
        ("err-message-units", 9 * h + 61, 90 * 60, 6),
    ]:
        v.add(cid, {"total_s": total, "max_s": mx, "max_segments": n},
              lambda total=total, mx=mx, n=n: slurm.segment_plan(total, mx, n))
    return v


def v_billed_gpus() -> Vectors:
    v = Vectors("billed_gpus", "arboria.slurm.billed_gpus",
                "max(gpus, ceil(mem_gb / mem_per_gpu_gb - 1e-9), ceil(cpus / cpus_per_gpu - 1e-9)) "
                "in f64 (DeltaAI PriorityFlags=MAX_TRES). Real DeltaAI: mem_per_gpu_gb = 110.898, "
                "cpus_per_gpu = 72.")
    for cid, g, mem, cpus, mpg, cpg in [
        ("1gpu-default", 1, 110, 72, 110.898, 72), ("4gpu-default", 4, 443, 288, 110.898, 72),
        ("mem-110.898-exact", 1, 110.898, 72, 110.898, 72),
        ("mem-111", 1, 111, 72, 110.898, 72), ("mem-221", 2, 221, 144, 110.898, 72),
        ("mem-222", 2, 222, 144, 110.898, 72), ("mem-443", 1, 443, 1, 110.898, 72),
        ("mem-444", 1, 444, 1, 110.898, 72), ("mem-332", 3, 332, 216, 110.898, 72),
        ("mem-333", 3, 333, 216, 110.898, 72), ("cpus-73", 1, 1, 73, 110.898, 72),
        ("cpus-144", 1, 1, 144, 110.898, 72), ("cpus-145", 1, 1, 145, 110.898, 72),
        ("cpus-288", 1, 1, 288, 110.898, 72), ("tiny", 1, 1, 1, 110.898, 72),
        ("gpus-dominate", 4, 8, 4, 110.898, 72), ("int-mem-100", 1, 100, 16, 100.0, 16),
        ("int-mem-201", 2, 201, 16, 100.0, 16), ("eps-just-over", 1, 100.0000001, 16, 100.0, 16),
        ("eps-absorbed", 1, 100.0000000001, 16, 100.0, 16),
        ("float-ratio", 3, 0.3, 1, 0.1, 1), ("all-zero-shares", 2, 0, 0, 110.898, 72),
    ]:
        v.add(cid, {"gpus": g, "mem_gb": mem, "cpus": cpus, "mem_per_gpu_gb": mpg,
                    "cpus_per_gpu": cpg},
              lambda g=g, mem=mem, cpus=cpus, mpg=mpg, cpg=cpg:
              slurm.billed_gpus(g, mem, cpus, mpg, cpg))
    return v


# ================================================================ overrides and the command


def v_check_override() -> Vectors:
    v = Vectors("check_override", "arboria.slurm.check_override",
                "Output [key, value-string]. Values: bool -> true/false, null -> 'null', int -> "
                "decimal, float -> Python repr (1e-05, 1e+16, 3.0), str unchanged. JSON ints in "
                "the input are Python ints, JSON floats (with '.' or exponent) are Python floats.")
    for cid, key, value in [
        ("lr-float", "lr", 0.0003), ("lr-1e-4", "optim.lr", 1e-4), ("lr-6e-4", "optim.lr", 6e-4),
        ("lr-1e-5", "lr", 1e-5), ("float-integral", "scale", 3.0), ("float-1e16", "n", 1e16),
        ("neg-zero", "x", -0.0), ("0.1+0.2", "x", 0.1 + 0.2), ("int", "trainer.max_steps", 200000),
        ("big-int", "seed", 123456789012345678), ("neg-int", "offset", -3),
        ("true", "use_ema", True), ("false", "use_ema", False), ("null", "ckpt", None),
        ("string", "model", "dmd"), ("string-number", "lr", "1e-4"), ("empty-value", "tag", ""),
        ("unicode-value", "name", "学习率 实验"), ("spaces-value", "msg", "a b  c"),
        ("hydra-plus", "+trainer.devices", 4), ("hydra-plus2", "++trainer.devices", 4),
        ("hydra-tilde", "~callbacks", None), ("hydra-group", "model/encoder", "vit"),
        ("dash-key", "batch-size", 64), ("underscore-start", "_private", 1),
        ("err-newline", "msg", "a\nb"), ("err-cr", "msg", "a\rb"), ("err-nul", "msg", "a\0b"),
        ("err-list", "layers", [1, 2]), ("err-dict", "opt", {"lr": 1}),
        ("err-key-digit", "1lr", 1), ("err-key-empty", "", 1), ("err-key-space", "a b", 1),
        ("err-key-eq", "lr=", 1), ("err-key-triple-plus", "+++x", 1),
        ("err-key-unicode", "学习率", 1), ("err-key-int", 5, 1), ("err-key-null", None, 1),
        ("err-key-dot-start", ".lr", 1),
        ("quirk-key-trailing-newline-accepted", "lr\n", 1),
    ]:
        v.add(cid, {"key": key, "value": value},
              lambda key=key, value=value: slurm.check_override(key, value))
    return v


def v_parse_override() -> Vectors:
    v = Vectors("parse_override", "arboria.slurm.parse_override",
                "`--set k=v`: split at the first '=', key stripped (Python str.strip(), Unicode "
                "whitespace incl. U+001C..U+001F), value kept verbatim. Output [key, value].")
    for cid, text in [
        ("simple", "lr=0.1"), ("dotted", "optim.lr=3e-4"), ("value-with-eq", "expr=a=b"),
        ("empty-value", "tag="), ("key-spaces", "  lr  =0.1"), ("value-spaces", "lr= 0.1 "),
        ("hydra-plus", "+trainer.devices=4"), ("hydra-tilde", "~callbacks="),
        ("unicode-value", "name=实验一"), ("key-ideographic-space", "　lr=1"),
        ("key-unit-separator", "\x1flr=1"), ("err-no-eq", "lr"), ("err-empty-key", "=5"),
        ("err-empty", ""), ("err-newline-value", "msg=a\nb"), ("err-bad-key", "a b=1"),
    ]:
        v.add(cid, {"text": text}, lambda text=text: slurm.parse_override(text))
    return v


def v_merge_overrides() -> Vectors:
    v = Vectors("merge_overrides", "arboria.slurm.merge_overrides",
                "Pairs as [key, value] lists. A key in `new` replaces the first equal key of "
                "`base` in place (later duplicates in base are kept); unseen keys are appended "
                "in order.")
    for cid, b, n in [
        ("both-empty", [], []), ("base-only", [["lr", "0.1"]], []),
        ("new-only", [], [["lr", "0.1"]]), ("replace", [["lr", "0.1"], ["bs", "64"]],
                                             [["lr", "0.2"]]),
        ("append", [["lr", "0.1"]], [["bs", "64"]]),
        ("replace-and-append", [["a", "1"], ["b", "2"], ["c", "3"]],
         [["c", "30"], ["d", "4"], ["a", "10"]]),
        ("new-duplicates", [["a", "1"]], [["a", "2"], ["a", "3"]]),
        ("base-duplicates", [["a", "1"], ["a", "2"]], [["a", "9"]]),
        ("new-duplicate-new-key", [], [["x", "1"], ["x", "2"]]),
        ("prefix-keys-distinct", [["trainer.devices", "4"]], [["+trainer.devices", "8"]]),
    ]:
        v.add(cid, {"base": b, "new": n}, lambda b=b, n=n: slurm.merge_overrides(
            [tuple(x) for x in b], [tuple(x) for x in n]))
    return v


def v_render_overrides() -> Vectors:
    v = Vectors("render_overrides", "arboria.slurm.render_overrides",
                "hydra -> ['k=v']; argparse / tyro -> ['--k', 'v'] (hydra prefixes + / ~ refused).")
    ov = [["lr", "0.0003"], ["trainer.max_steps", "200000"], ["use_ema", "true"]]
    for cid, style, o in [
        ("hydra", "hydra", ov), ("argparse", "argparse", ov), ("tyro", "tyro", ov),
        ("hydra-empty", "hydra", []), ("argparse-empty", "argparse", []),
        ("hydra-prefixes", "hydra", [["+trainer.devices", "4"], ["~callbacks", "null"],
                                     ["++x", "1"]]),
        ("hydra-empty-value", "hydra", [["tag", ""]]),
        ("argparse-empty-value", "argparse", [["tag", ""]]),
        ("argparse-dash-key", "argparse", [["batch-size", "64"]]),
        ("err-argparse-plus", "argparse", [["lr", "1"], ["+x", "1"]]),
        ("err-tyro-tilde", "tyro", [["~x", "null"]]),
        ("err-unknown-style", "click", ov), ("err-unknown-style-empty", "", []),
    ]:
        v.add(cid, {"style": style, "overrides": o},
              lambda style=style, o=o: slurm.render_overrides(style, [tuple(x) for x in o]))
    return v


def v_build_command() -> Vectors:
    v = Vectors("build_command", "arboria.submission.build_command",
                "input.config_command is arboria.toml [run] command (a string, split with "
                "shlex.split; '' = not configured), input.override_style its override_style. "
                "explicit = the `-- CMD` argv or null, overrides = --set pairs, parent_request = "
                "the --from run's request_json or null. Output is the CommandSpec: command, base, "
                "source, overrides (as [k, v] lists), style, and request_extra().")

    def run(config_command: str, style: str, explicit, overrides, parent):
        cfg = config.ProjectConfig(
            default_machine="", total_su=0.0, project_id=PROJECT,
            watch=config.WatchConfig(config.DEFAULT_OBSERVATION_MINUTES, ()),
            run=config.RunConfig(tuple(shlex.split(config_command)), style))
        spec = submission.build_command(cfg, explicit, [tuple(x) for x in overrides], parent)
        return {"command": spec.command, "base": spec.base, "source": spec.source,
                "overrides": [list(kv) for kv in spec.overrides], "style": spec.style,
                "request_extra": spec.request_extra()}

    parent_cfg = {"command": ["python", "train.py", "lr=0.1"], "command_source": "config",
                  "base_command": ["python", "train.py"], "overrides": [["lr", "0.1"]],
                  "override_style": "hydra"}
    parent_explicit = {"command": ["python", "run.py", "--lr", "0.1"],
                       "command_source": "explicit", "base_command": ["python", "run.py"],
                       "overrides": [["lr", "0.1"]], "override_style": "argparse"}
    parent_v3 = {"command": ["python", "old.py", "--epochs", "3"]}
    for cid, cc, style, explicit, ov, parent in [
        ("explicit", "", "hydra", ["python", "train.py"], [], None),
        ("explicit-with-set", "python train.py", "hydra", ["python", "x.py"], [["lr", 0.1]], None),
        ("config", "python train.py", "hydra", None, [], None),
        ("config-hydra-set", "python train.py", "hydra", None,
         [["optim.lr", 3e-4], ["use_ema", True]], None),
        ("config-argparse-set", "python train.py --cfg base.yaml", "argparse", None,
         [["lr", 1e-4], ["batch-size", 64]], None),
        ("config-tyro-set", "uv run train.py", "tyro", None, [["seed", 0]], None),
        ("config-quoted", "python 'my train.py' --name \"a b\"", "hydra", None, [], None),
        ("from-config-parent", "python train.py", "hydra", None, [["lr", 0.2], ["bs", 32]],
         parent_cfg),
        ("from-explicit-parent", "python train.py", "hydra", None, [["lr", 0.2]],
         parent_explicit),
        ("from-v3-parent", "python train.py", "hydra", None, [["epochs", 5]], parent_v3),
        ("from-parent-explicit-wins", "python train.py", "hydra", ["python", "new.py"],
         [["lr", 1]], parent_cfg),
        ("err-no-command", "", "hydra", None, [], None),
        ("err-no-command-with-set", "", "hydra", None, [["lr", 1]], None),
        ("err-bad-override", "python train.py", "hydra", None, [["a b", 1]], None),
        ("err-prefix-argparse", "python train.py", "argparse", None, [["+x", 1]], None),
        ("err-parent-config-now-empty", "", "hydra", None, [], parent_cfg),
    ]:
        v.add(cid, {"config_command": cc, "override_style": style, "explicit": explicit,
                    "overrides": ov, "parent_request": parent},
              lambda cc=cc, style=style, explicit=explicit, ov=ov, parent=parent:
              run(cc, style, explicit, ov, parent))
    return v


# ================================================================ names and remote paths

PATHS = dict(DELTAAI.paths)
PATHS_BRACED = dict(DELTAAI_UV.paths)


def v_check_project() -> Vectors:
    v = Vectors("check_project", "arboria.slurm.check_project",
                "arboria.toml [project] id: ^[a-z0-9][a-z0-9-]{0,30}$ (1..31 chars). Returns the "
                "id unchanged.")
    for cid, p in [("typical", PROJECT), ("31-chars", PROJECT_31), ("1-char", "a"),
                   ("digit-start", "1abc"), ("trailing-dash", "proj-"), ("double-dash", "a--b"),
                   ("err-32-chars", "a" * 32), ("err-upper", "WaveFront"),
                   ("err-dash-start", "-proj"), ("err-underscore", "my_proj"), ("err-empty", ""),
                   ("err-dot", "a.b"), ("err-unicode", "项目"), ("err-null", None), ("err-int", 7),
                   ("quirk-trailing-newline-accepted", "proj\n"),
                   ("err-inner-newline", "pro\nj")]:
        v.add(cid, {"project": p}, lambda p=p: slurm.check_project(p))
    return v


def v_job_name() -> Vectors:
    v = Vectors("job_name", "arboria.slurm.job_name",
                "arb-<project>-<run>-s<segment>, at most 64 chars, must match JOB_NAME_RE "
                "(run id: [a-z0-9]+, segment >= 1).")
    for cid, p, r, s in [
        ("typical", PROJECT, "r001", 1), ("segment-4", PROJECT, "r001", 4),
        ("run-1000", PROJECT, "r1000", 2), ("project-31", PROJECT_31, "r001", 1),
        ("exactly-64", PROJECT_31, "r" + "0" * 24, 1), ("err-65", PROJECT_31, "r" + "0" * 25, 1),
        ("segment-10", PROJECT, "r001", 10), ("err-segment-0", PROJECT, "r001", 0),
        ("quirk-run-dash-accepted", PROJECT, "r-001", 1), ("err-run-upper", PROJECT, "R001", 1),
        ("err-run-empty", PROJECT, "", 1), ("err-bad-project", "Proj", "r001", 1),
    ]:
        v.add(cid, {"project": p, "run_id": r, "segment": s},
              lambda p=p, r=r, s=s: slurm.job_name(p, r, s))
    return v


def v_parse_job_name() -> Vectors:
    v = Vectors("parse_job_name", "arboria.slurm.parse_job_name",
                "Output [project, run id, segment]. JOB_NAME_RE is "
                "^arb-([a-z0-9][a-z0-9-]{0,30})-([a-z0-9]+)-s([1-9][0-9]*)$ with backtracking "
                "(greedy project group): the run id is the last '-'-free part before -sK. "
                "Names longer than 64 chars are refused.")
    names = [
        ("typical", f"arb-{PROJECT}-r001-s1"), ("segment-12", f"arb-{PROJECT}-r003-s12"),
        ("project-with-dashes", "arb-my-proj-1a2b-r003-s2"), ("short", "arb-a-b-s1"),
        ("greedy-backtrack", "arb-a-b-c-s1"), ("run-looks-like-segment", "arb-a-s1-s2"),
        ("project-31", f"arb-{PROJECT_31}-r001-s1"),
        ("exactly-64", f"arb-{PROJECT_31}-r{'0' * 24}-s1"),
        ("err-65", f"arb-{PROJECT_31}-r{'0' * 25}-s1"),
        ("err-legacy", "arboria-r001-s1"), ("err-segment-0", f"arb-{PROJECT}-r001-s0"),
        ("err-segment-leading-zero", f"arb-{PROJECT}-r001-s01"),
        ("err-upper", "arb-Proj-r001-s1"), ("err-no-segment", f"arb-{PROJECT}-r001"),
        ("err-empty", ""), ("err-suffix", f"arb-{PROJECT}-r001-s1x"),
        ("quirk-trailing-newline-accepted", f"arb-{PROJECT}-r001-s1\n"),
        ("err-project-32", f"arb-{'a' * 32}-r001-s1"),
    ]
    for cid, n in names:
        v.add(cid, {"name": n}, lambda n=n: slurm.parse_job_name(n))
    v.add("err-null", {"name": None}, lambda: slurm.parse_job_name(None))
    return v


def v_job_project() -> Vectors:
    v = Vectors("job_project", "arboria.slurm.job_project",
                "The project of an arboria job name, or null for anything else.")
    for cid, n in [("typical", f"arb-{PROJECT}-r001-s1"), ("dashes", "arb-my-proj-1a2b-r003-s2"),
                   ("legacy", "arboria-r001-s1"), ("other-job", "jupyter"), ("empty", ""),
                   ("too-long", f"arb-{PROJECT_31}-r{'0' * 25}-s1"),
                   ("segment-12", f"arb-{PROJECT}-r003-s12"), ("short", "arb-a-b-s1"),
                   ("upper", "arb-Proj-r001-s1"), ("segment-0", f"arb-{PROJECT}-r001-s0"),
                   ("null", None)]:
        v.add(cid, {"name": n}, lambda n=n: slurm.job_project(n))
    return v


def _path_fn_vectors(name: str, fn: Callable[..., str], args: list[str], notes: str) -> Vectors:
    v = Vectors(name, f"arboria.slurm.{name}", notes)
    combos = [
        ("deltaai", PATHS, PROJECT, "r001", 1, "4523101"),
        ("deltaai-segment-3", PATHS, PROJECT, "r042", 3, "4523190"),
        ("braced-user", PATHS_BRACED, PROJECT, "r001", 2, "12"),
        ("project-31", PATHS, PROJECT_31, "r1000", 1, "9"),
        ("fake", dict(FAKE.paths), PROJECT, "r007", 1, "1001"),
        ("array-job-id", PATHS, PROJECT, "r001", 1, "4523101_7"),
        ("segment-12", PATHS, PROJECT, "r042", 12, "4523190"),
        ("project-1-char", PATHS, "a", "r001", 1, "1"),
        ("run-id-not-validated", PATHS, PROJECT, "R-1/x", 1, "1"),
        ("err-bad-project", PATHS, "Bad_Project", "r001", 1, "1"),
        ("err-project-null", PATHS, None, "r001", 1, "1"),
    ]
    for cid, paths, p, r, s, j in combos:
        full = {"paths": paths, "project": p, "run_id": r, "segment": s, "job_id": j}
        inp = {k: full[k] for k in args}
        v.add(cid, inp, lambda inp=inp: fn(**inp))
    return v


def path_vectors() -> list[Vectors]:
    note = ("input.paths is a profile's [paths] table (validated, no trailing '/'). The run id "
            "is not validated (arb only passes ids it made); the project id is. ")
    return [
        _path_fn_vectors("run_dir", slurm.run_dir, ["paths", "project", "run_id"],
                         note + "<runs>/<project>/<run>; $USER stays a shell variable."),
        _path_fn_vectors("code_dir", slurm.code_dir, ["paths", "project", "run_id"],
                         note + "<run_dir>/code (where the snapshot is unpacked)."),
        _path_fn_vectors("script_path", slurm.script_path,
                         ["paths", "project", "run_id", "segment"],
                         note + "<run_dir>/sbatch/s<k>.sh."),
        _path_fn_vectors("log_pattern", slurm.log_pattern,
                         ["paths", "project", "run_id", "segment"],
                         note + "<run_dir>/logs/s<k>.%j.out (Slurm's %j)."),
        _path_fn_vectors("log_path", slurm.log_path,
                         ["paths", "project", "run_id", "segment", "job_id"],
                         note + "log_pattern with every %j replaced by the job id."),
        _path_fn_vectors("out_dir", slurm.out_dir, ["paths", "project", "run_id"],
                         note + "<out>/<project>/<run> (ARBORIA_OUT)."),
        _path_fn_vectors("ckpt_dir", slurm.ckpt_dir, ["paths", "project", "run_id"],
                         note + "<ckpt>/<project>/<run> (ARBORIA_CKPT_DIR)."),
        _path_fn_vectors("wandb_id", slurm.wandb_id, ["project", "run_id"],
                         "<project>-<run> (ARBORIA_WANDB_ID). The run id is not validated."),
    ]


def v_sbatch_value() -> Vectors:
    v = Vectors("sbatch_value", "arboria.slurm.sbatch_value",
                "#SBATCH lines are not shell-expanded: $USER and ${USER} become %u; $USERNAME, "
                "$USER_X etc. stay ($USER must not be followed by [A-Za-z0-9_]).")
    for cid, p in [("plain", "/projects/bjki/x/logs/s1.%j.out"),
                   ("dollar-user", "/projects/bjki/$USER/arboria/runs/p/r001/logs/s1.%j.out"),
                   ("braced", "/projects/bjki/${USER}/logs"), ("twice", "/a/$USER/b/${USER}/c"),
                   ("username", "/a/$USERNAME/b"), ("user-underscore", "/a/$USER_X/b"),
                   ("user-dot", "/a/$USER.old/b"), ("user-dash", "/a/$USER-old/b"),
                   ("user-end", "/a/$USER"), ("lowercase", "/a/$user/b"), ("empty", "")]:
        v.add(cid, {"path": p}, lambda p=p: slurm.sbatch_value(p))
    return v


def v_dq() -> Vectors:
    v = Vectors("dq", "arboria.slurm.dq",
                "Double-quote a validated remote path (so $USER still expands); refuses \" \\ ` "
                "and $(.")
    for cid, p in [("plain", "/projects/bjki/$USER/arboria/runs"), ("braced", "/a/${USER}/b"),
                   ("spaces", "/a b/c"), ("single-quote", "/a'b"), ("empty", ""),
                   ("err-double-quote", '/a"b'), ("err-backslash", "/a\\b"),
                   ("err-backtick", "/a`id`"), ("err-command-subst", "/a/$(id)"),
                   ("dollar-paren-apart", "/a/$ (x)")]:
        v.add(cid, {"path": p}, lambda p=p: slurm.dq(p))
    return v


def v_shell_quote() -> Vectors:

    v = Vectors("shell_quote", "shlex.join",
                "Python's shlex.join / shlex.quote, which render_sbatch uses for the command line "
                "and the ARBORIA_RUN_ID/WANDB_ID/IDEA exports. quote(): '' for the empty string; "
                "unchanged if every char is ASCII [A-Za-z0-9_@%+=:,./-]; else wrapped in single "
                "quotes with each ' written as '\"'\"'. Output {join, quoted}.")
    for cid, argv in [
        ("simple", ["python", "train.py"]), ("hydra", ["python", "train.py", "optim.lr=0.0003"]),
        ("spaces", ["python", "-c", "print('hi there')"]), ("empty-arg", ["echo", ""]),
        ("dollar", ["echo", "$HOME", "${USER}"]), ("double-quote", ['a"b']),
        ("single-quote", ["it's"]), ("only-quote", ["'"]), ("unicode", ["--name=学习率"]),
        ("safe-punct", ["a@b%c+d=e:f,g.h/i-j_k"]), ("glob", ["*.py", "a?b", "[x]"]),
        ("tilde-hash", ["~/x", "#c"]), ("semicolon", ["a;b", "a&&b", "a|b"]),
        ("newline", ["a\nb"]), ("tab", ["a\tb"]), ("backslash", ["a\\b"]),
        ("parens", ["(x)", "!x"]), ("latin1", ["é"]), ("dash-flags", ["--lr", "1e-4", "-v"]),
    ]:
        v.add(cid, {"argv": argv},
              lambda argv=argv: {"join": shlex.join(argv), "quoted": [shlex.quote(a) for a in argv]})
    return v


def v_shell_split() -> Vectors:

    v = Vectors("shell_split", "shlex.split",
                "Python's shlex.split(text) (POSIX mode, no comments), which config uses for "
                "arboria.toml [run] command. Errors are ValueError with shlex's own message; "
                "config wraps them (see arboria_toml err-run-unbalanced-quote).")
    for cid, text in [
        ("simple", "python train.py"), ("extra-space", "  python   train.py  "),
        ("single-quotes", "python 'my train.py'"), ("double-quotes", 'a "b c" d'),
        ("escaped-space", "a\\ b"), ("escaped-quote-in-double", 'a "b\\"c"'),
        ("backslash-in-single", "'a\\b'"), ("backslash-in-double", '"a\\b"'),
        ("backslash-dollar-in-double", '"a\\$b"'), ("adjacent-quotes", "a'b'\"c\""),
        ("empty-quoted", '""'), ("empty", ""), ("blank", " \t\n "), ("hash-literal", "a #b"),
        ("unicode", "python 训练.py --name 实验"), ("tabs-newlines", "a\tb\nc"),
        ("equals", "--cfg=base.yaml x=1"), ("err-unclosed-single", "python 'train.py"),
        ("err-unclosed-double", 'a "b'), ("err-trailing-backslash", "a\\"),
    ]:
        v.add(cid, {"text": text}, lambda text=text: shlex.split(text), errors=(ValueError,))
    return v


def v_render_sbatch() -> Vectors:
    v = Vectors("render_sbatch", "arboria.slurm.render_sbatch",
                PLAN_INPUT_NOTE + "Output is the full batch script text for one segment "
                "(plan = build_plan(profile, ask, project)); \\n line endings, trailing newline. "
                "The set +u / set -u block appears only when prelude or env_activate is non-empty.")
    picks = {"1gpu-1h", "1gpu-1h-hydra", "2nodes-8gpu-24h", "interactive-2h-charge2",
             "mem111-1gpu-billed2", "command-quoting", "argparse-overrides", "uv-env-prelude",
             "fake-machine", "project-31-chars", "repeat-nonce"}
    for cid, key, ask, proj, _ in PLANS:
        if cid in picks:
            v.add(cid, plan_input(key, ask, proj, run_id="r001", idea="dmd-distill", segment=1),
                  lambda key=key, ask=ask, proj=proj:
                  slurm.render_sbatch(build(key, ask, proj), "r001", "dmd-distill", 1))
    multi = [p for p in PLANS if p[0] in ("1gpu-72h-2segments", "4gpu-7d-4segments")]
    for cid, key, ask, proj, _ in multi:
        n = len(build(key, ask, proj).segments)
        for seg in range(1, n + 1):
            v.add(f"{cid}-s{seg}",
                  plan_input(key, ask, proj, run_id="r012", idea="long-run", segment=seg),
                  lambda key=key, ask=ask, proj=proj, seg=seg:
                  slurm.render_sbatch(build(key, ask, proj), "r012", "long-run", seg))
    _, key, ask, proj, _ = PLANS[0]
    for cid, run_id, idea, seg in [("run-1000", "r1000", "a", 1),
                                   ("err-segment-0", "r001", "x", 0),
                                   ("err-segment-2-of-1", "r001", "x", 2),
                                   ("err-bad-run-id", "R001", "x", 1)]:
        v.add(cid, plan_input(key, ask, proj, run_id=run_id, idea=idea, segment=seg),
              lambda run_id=run_id, idea=idea, seg=seg:
              slurm.render_sbatch(build(key, ask, proj), run_id, idea, seg))
    return v


# ================================================================ backends, sacct, run status


class FakeRunner(backend_ssh.Runner):
    """Answers `ssh -O check` with success and remote commands from canned stdout."""

    def __init__(self, replies: list[tuple[str, str]]):
        self.replies = list(replies)
        self.remote: list[str] = []

    def run(self, argv, *, input=None, timeout=0):
        if argv[:2] == ["ssh", "-O"]:
            return subprocess.CompletedProcess(argv, 0, "", "")
        cmd = argv[-1]
        self.remote.append(cmd)
        for i, (needle, out) in enumerate(self.replies):
            if cmd.startswith(needle):
                self.replies.pop(i)
                return subprocess.CompletedProcess(argv, 0, out, "")
        return subprocess.CompletedProcess(argv, 0, "", "")


def v_map_slurm_state() -> Vectors:
    v = Vectors("map_slurm_state", "arboria.backends.base.map_slurm_state",
                "sacct State -> jobs.state: first whitespace-separated word, trailing '+' "
                "removed, uppercased, looked up; anything else is 'unknown'.")
    for raw in sorted(backend_base._SLURM_STATES):
        v.add(raw, {"raw": raw}, lambda raw=raw: backend_base.map_slurm_state(raw))
    for cid, raw in [("cancelled-by", "CANCELLED by 123456"), ("plus", "CANCELLED+"),
                     ("lowercase", "completed"), ("padded", "  RUNNING  "), ("empty", ""),
                     ("blank", "   "), ("unknown", "WHAT"), ("tab", "FAILED\tx"),
                     ("plus-plus", "TIMEOUT++")]:
        v.add(cid, {"raw": raw}, lambda raw=raw: backend_base.map_slurm_state(raw))
    return v


def v_su_from_billing() -> Vectors:
    v = Vectors("su_from_billing", "arboria.backends.ssh.su_from_billing",
                "float(N) * elapsed_s / 3600 / billing_per_su, evaluated left to right in f64, "
                "from sacct AllocTRES `billing=N` (null when billing_per_su <= 0 or no billing "
                "item). Not rounded here (monitor rounds to 4 places). Real DeltaAI rows checked "
                "against `jobcharge --detail` (0.13, 0.22, 0.32, 0.18 SU after rounding).")
    for cid, tres, e, b in [
        ("deltaai-259s", "billing=1803,cpu=64,gres/gpu=1,mem=200G,node=1", 259, 1000),
        ("deltaai-449s", "billing=1803,cpu=64,gres/gpu=1,mem=200G,node=1", 449, 1000),
        ("deltaai-643s", "billing=1803,cpu=64,gres/gpu=1,mem=200G,node=1", 643, 1000),
        ("deltaai-644s", "billing=1000,cpu=72,gres/gpu=1,mem=100G,node=1", 644, 1000),
        ("4gpu-1h", "billing=4000,cpu=288,gres/gpu=4,mem=443G,node=1", 3600, 1000),
        ("interactive-2x", "billing=2000,cpu=72,gres/gpu=1,mem=110G,node=1", 1800, 1000),
        ("billing-not-first", "cpu=72,billing=1000,mem=100G", 3600, 1000),
        ("billing-last", "cpu=72,mem=100G,billing=1000", 7200, 1000),
        ("decimal-billing", "billing=1000.5,cpu=1", 3600, 1000),
        ("zero-elapsed", "billing=1000", 0, 1000), ("float-bps", "billing=1000", 3600, 1500.0),
        ("whitespace", "  billing=1000  ", 3600, 1000),
        ("none-no-billing", "cpu=72,mem=100G", 644, 1000),
        ("none-prefixed", "xbilling=5", 644, 1000), ("none-empty", "", 644, 1000),
        ("none-bps-0", "billing=1000", 3600, 0), ("none-bps-negative", "billing=1000", 3600, -1),
        ("none-billing-suffix", "billing=10x", 3600, 1000),
    ]:
        v.add(cid, {"alloc_tres": tres, "elapsed_s": e, "billing_per_su": b},
              lambda tres=tres, e=e, b=b: backend_ssh.su_from_billing(tres, e, b))
    return v


def v_sacct_poll() -> Vectors:
    v = Vectors("sacct_poll", "arboria.backends.ssh.SshSlurm.poll",
                "input.stdout is what `sacct -j <ids> -n -X -P -o JobID,State,ElapsedRaw,AllocTRES`"
                " printed; input.job_ids the ids asked for; input.billing_per_su the profile's. "
                "Output {command: the remote command string, result: {job_id: [state, elapsed_s, "
                "billed_su or null]}} (tuples as lists). Lines with < 3 fields or an id not asked "
                "for are skipped; a non-digit ElapsedRaw is 0; SU only for terminal states. "
                "Profile: real DeltaAI.")

    def run(ids, stdout, bps):
        prof = DELTAAI if bps == DELTAAI.billing_per_su else load(
            DELTAAI_TOML.replace("billing_per_su = 1000", f"billing_per_su = {bps}"), "deltaai")
        runner = FakeRunner([("sacct -j", stdout)])
        backend = backend_ssh.SshSlurm(prof, PROJECT, runner)
        result = backend.poll(ids)
        return {"command": runner.remote[-1] if runner.remote else None,
                "result": {k: list(t) for k, t in result.items()}}

    for cid, ids, out, bps in [
        ("deltaai-mixed", ["10", "11", "12", "13"],
         "10|COMPLETED|3600|billing=4000,cpu=288,gres/gpu=4,mem=443G,node=1\n"
         "11|CANCELLED by 123|5|\n12|PENDING|0|\n9|X|1|billing=9\n"
         "13|RUNNING|60|billing=1000,cpu=72\n", 1000),
        ("real-rows", ["4523101", "4523102", "4523103"],
         "4523101|COMPLETED|259|billing=1803,cpu=64,gres/gpu=1,mem=200G,node=1\n"
         "4523102|FAILED|449|billing=1803,cpu=64,gres/gpu=1,mem=200G,node=1\n"
         "4523103|TIMEOUT|643|billing=1803,cpu=64,gres/gpu=1,mem=200G,node=1\n", 1000),
        ("oom-nodefail", ["7", "8"],
         "7|OUT_OF_MEMORY|120|billing=1000,cpu=72,gres/gpu=1,mem=110G,node=1\n"
         "8|NODE_FAIL|30|billing=4000,cpu=288\n", 1000),
        ("pending-no-tres", ["20"], "20|PENDING|0|\n", 1000),
        ("requeued", ["21"], "21|REQUEUED|0|billing=1000\n", 1000),
        ("running-has-billing-no-su", ["22"], "22|RUNNING|1800|billing=4000\n", 1000),
        ("estimate-locally-bps-0", ["23"], "23|COMPLETED|3600|billing=4000\n", 0),
        ("no-alloc-field", ["24"], "24|COMPLETED|100\n", 1000),
        ("too-few-fields", ["25"], "25|COMPLETED\n", 1000),
        ("elapsed-blank", ["26"], "26|CANCELLED+||billing=1000\n", 1000),
        ("elapsed-nonnumeric", ["27"], "27|COMPLETED|abc|billing=1000\n", 1000),
        ("whitespace-lines", ["28"], "\n  28|COMPLETED|60|billing=1000  \n\n", 1000),
        ("duplicate-id-last-wins", ["29"], "29|RUNNING|10|\n29|COMPLETED|20|billing=1000\n", 1000),
        ("unknown-state", ["30"], "30|WEIRD|5|billing=1000\n", 1000),
        ("empty-stdout", ["31"], "", 1000),
        ("no-ids", [], "", 1000),
        ("array-job", ["40_1"], "40_1|COMPLETED|60|billing=1000\n", 1000),
    ]:
        v.add(cid, {"job_ids": ids, "stdout": out, "billing_per_su": bps},
              lambda ids=ids, out=out, bps=bps: run(ids, out, bps))
    v.add("err-bad-id", {"job_ids": ["12;rm"], "stdout": "", "billing_per_su": 1000},
          lambda: run(["12;rm"], "", 1000))
    return v


def v_find_job() -> Vectors:
    v = Vectors("find_job", "arboria.backends.ssh.SshSlurm.find_job",
                "input.squeue / input.sacct are the stdouts of `squeue --name=<n> -u \"$USER\" -h "
                "-o '%i %j'` and `sacct --name=<n> -n -X -P -o JobID,JobName "
                "--starttime=now-30days` (sacct only runs when squeue found nothing). Output "
                "{job_id, commands: the remote commands run}. Backend project: " + PROJECT + ".")

    def run(name, sq, sa):
        runner = FakeRunner([("squeue", sq), ("sacct", sa)])
        backend = backend_ssh.SshSlurm(DELTAAI, PROJECT, runner)
        try:
            jid = backend.find_job(name)
        except ArbError as exc:
            raise type(exc)(f"{exc}") from None
        return {"job_id": jid, "commands": runner.remote}

    n = f"arb-{PROJECT}-r003-s1"
    for cid, name, sq, sa in [
        ("in-squeue", n, f"555 {n}\n", ""), ("in-sacct", n, "", f"556|{n}\n"),
        ("none", n, "", ""), ("exact-name-only", n,
                              f"557 {n}0\n558 arb-other-1a2b-r003-s1\n559 x{n}\n",
                              f"560|{n}-x\n561|\n"),
        ("squeue-and-sacct-same", n, f"9 {n}\n", f"9|{n}\n"),
        ("err-several", n, f"1 {n}\n2 {n}\n", ""), ("err-several-sacct", n, "", f"1|{n}\n2|{n}\n"),
        ("same-id-twice", n, f"5 {n}\n5 {n}\n", ""),
        ("err-other-project", "arb-other-1a2b-r003-s1", "", ""),
        ("err-not-arboria", "jupyter", "", ""),
    ]:
        v.add(cid, {"name": name, "squeue": sq, "sacct": sa},
              lambda name=name, sq=sq, sa=sa: run(name, sq, sa))
    return v


def v_check_job_ids() -> Vectors:
    v = Vectors("check_job_ids", "arboria.backends.base.check_job_ids",
                "Slurm job ids ^[0-9]+(?:_[0-9]+)?$; all bad ones are listed in the error.")
    for cid, ids in [("ok", ["1", "4523101"]), ("array", ["40_1"]), ("empty", []),
                     ("err-letters", ["12a"]), ("err-two-bad", ["1", "x", "2;3"]),
                     ("err-empty-id", [""]), ("err-dot-step", ["12.batch"]),
                     ("err-plus-het", ["12+0"]), ("ints-stringified", [4523101, 7]),
                     ("err-negative", ["-1"]), ("err-leading-space", [" 1"]),
                     ("quirk-trailing-newline-accepted", ["4523101\n"])]:
        v.add(cid, {"job_ids": ids}, lambda ids=ids: backend_base.check_job_ids(ids))
    return v


def v_restrict_path() -> Vectors:
    v = Vectors("restrict_path", "arboria.backends.base.restrict_path",
                "Remote paths the backend may read: <runs>/<project>[/...] or <out>/<project>[/...]"
                ", segments [A-Za-z0-9._-]+ and not . or ..; trailing '/' removed. Profile: real "
                "DeltaAI; input.project the backend's project.")
    runs = f"{DELTAAI.paths['runs']}/{PROJECT}"
    out = f"{DELTAAI.paths['out']}/{PROJECT}"
    for cid, p, path in [
        ("runs-root", PROJECT, runs), ("runs-run", PROJECT, f"{runs}/r001"),
        ("log", PROJECT, f"{runs}/r001/logs/s1.4523101.out"), ("out-run", PROJECT, f"{out}/r001"),
        ("trailing-slash", PROJECT, f"{runs}/r001/"), ("err-dotdot", PROJECT, f"{runs}/../x"),
        ("err-dot", PROJECT, f"{runs}/./r001"), ("double-slash-kept", PROJECT, f"{runs}//r001"),
        ("err-other-project", PROJECT, f"{DELTAAI.paths['runs']}/other-1a2b/r001"),
        ("err-prefix-trick", PROJECT, f"{runs}x/r001"),
        ("err-runs-parent", PROJECT, DELTAAI.paths["runs"]), ("err-ckpt", PROJECT,
                                                              f"{DELTAAI.paths['ckpt']}/{PROJECT}"),
        ("err-root", PROJECT, "/"), ("err-empty", PROJECT, ""),
        ("err-space", PROJECT, f"{runs}/r 001"), ("err-bad-project", "Bad", runs),
    ]:
        v.add(cid, {"project": p, "path": path, "profile": profile_json(DELTAAI)},
              lambda p=p, path=path: backend_base.restrict_path(DELTAAI, p, path))
    return v


def v_derive_status() -> Vectors:
    v = Vectors("derive_status", "arboria.monitor.derive_status",
                "Run status from its jobs' states (segment order), the plan's segment count and "
                "the run's current status.")
    for cid, states, n, cur in [
        ("all-pending", ["pending"], 1, "queued"), ("running", ["running"], 1, "queued"),
        ("completed", ["completed"], 1, "running"), ("failed", ["failed"], 1, "running"),
        ("cancelled", ["cancelled"], 1, "running"), ("timeout-last", ["timeout"], 1, "running"),
        ("unknown", ["unknown"], 1, "queued"), ("submitting", ["submitting"], 1, "queued"),
        ("2seg-first-timeout-second-pending", ["timeout", "pending"], 2, "running"),
        ("2seg-timeout-then-completed", ["timeout", "completed"], 2, "running"),
        ("2seg-both-timeout", ["timeout", "timeout"], 2, "running"),
        ("2seg-completed-then-completed", ["completed", "completed"], 2, "running"),
        ("2seg-one-row-only", ["completed"], 2, "running"),
        ("2seg-one-row-approved", ["completed"], 2, "approved"),
        ("approved-pending", ["pending"], 2, "approved"),
        ("failed-beats-cancelled", ["cancelled", "failed"], 2, "running"),
        ("cancelled-beats-running", ["running", "cancelled"], 2, "running"),
        ("4seg-progress", ["timeout", "timeout", "running", "pending"], 4, "queued"),
        ("4seg-done", ["timeout", "timeout", "timeout", "completed"], 4, "running"),
        ("no-states", [], 1, "queued"), ("no-states-approved", [], 1, "approved"),
        ("completed-but-approved", ["completed"], 1, "approved"),
    ]:
        v.add(cid, {"states": states, "n_segments": n, "current": cur},
              lambda states=states, n=n, cur=cur: monitor.derive_status(states, n, cur))
    return v


def v_billed_su() -> Vectors:
    v = Vectors("billed_su", "arboria.monitor._billed",
                "jobs.billed_su: round(reported, 4) when sacct billing gave a value, else "
                "round(elapsed / 3600 * billed_gpus * charge, 4) (left to right, f64), null without "
                "elapsed. Python round() is correctly rounded (ties to even on the exact binary "
                "value). input.request holds the run's request_json fields used.")
    for cid, req, el, rep in [
        ("reported-deltaai", {"billed_gpus": 1, "charge": 1.0}, 259, 1803 * 259 / 3600 / 1000),
        ("reported-644s", {"billed_gpus": 1, "charge": 1.0}, 644, 1000 * 644 / 3600 / 1000),
        ("reported-int", {"billed_gpus": 4, "charge": 1.0}, 3600, 4),
        ("reported-zero", {"billed_gpus": 4, "charge": 1.0}, 3600, 0.0),
        ("estimate-1h", {"billed_gpus": 4, "charge": 1.0}, 3600, None),
        ("estimate-interactive", {"billed_gpus": 2, "charge": 2.0}, 1800, None),
        ("estimate-odd", {"billed_gpus": 3, "charge": 1.0}, 12345, None),
        ("estimate-7s", {"billed_gpus": 1, "charge": 1.0}, 7, None),
        ("estimate-9s", {"billed_gpus": 1, "charge": 1.0}, 9, None),
        ("estimate-zero", {"billed_gpus": 1, "charge": 1.0}, 0, None),
        ("tie-0.00005", {"billed_gpus": 1, "charge": 1.0}, None, 0.00005),
        ("tie-0.00015", {"billed_gpus": 1, "charge": 1.0}, None, 0.00015),
        ("tie-1.23445", {"billed_gpus": 1, "charge": 1.0}, None, 1.23445),
        ("no-elapsed", {"billed_gpus": 1, "charge": 1.0}, None, None),
    ]:
        v.add(cid, {"request": req, "elapsed": el, "reported": rep},
              lambda req=req, el=el, rep=rep: monitor._billed(req, el, rep))
    return v


def v_first_hit() -> Vectors:
    v = Vectors("first_hit", "arboria.monitor.first_hit",
                "Observation window: the first log line (Python str.splitlines(): \\n, \\r, \\r\\n, "
                "\\v, \\f, \\x1c-\\x1e, \\x85, U+2028, U+2029) matching any pattern, patterns tried in "
                "order per line. Output [pattern, line.strip()[:200]] (code points) or null. "
                "Patterns are Python `re` syntax; input.patterns null = arboria's "
                "DEFAULT_FAIL_PATTERNS (" + json.dumps(list(config.DEFAULT_FAIL_PATTERNS)) + ").")
    import re as re_mod

    def run(text, patterns):
        pats = tuple(re_mod.compile(p) for p in (patterns or config.DEFAULT_FAIL_PATTERNS))
        hit = monitor.first_hit(text, pats)
        return list(hit) if hit else None

    long_line = "step 100 " + "x" * 300 + " loss nan"
    for cid, text, pats in [
        ("loss-nan", "step 1 loss 2.3\nstep 2 loss nan\n", None),
        ("loss-NaN-case", "Step 5: Loss = NaN\n", None),
        ("nan-before-loss", "found nan in loss at step 3", None),
        ("nan-loss-far", "nan " + "y" * 41 + " loss", None),
        ("loss-nan-41-apart", "loss" + " " * 39 + "x nan", None),
        ("nanosecond-not-nan", "loss 0.1 after 5 nanoseconds", None),
        ("lossy-not-loss", "lossy nan", None), ("cuda-oom", "RuntimeError: CUDA out of memory.", None),
        ("torch-oom", "torch.OutOfMemoryError: CUDA out of memory. Tried to allocate", None),
        ("pattern-order-within-line", "torch.OutOfMemoryError CUDA out of memory", None),
        ("first-line-wins", "CUDA out of memory\nloss nan\n", None),
        ("clean", "epoch 1 loss 0.5\nepoch 2 loss 0.4\n", None), ("empty", "", None),
        ("strip-indent", "   \tloss: nan   \n", None), ("truncate-200", long_line, None),
        ("cr-separated", "loss 1\rloss nan\r", None), ("u2028-separated", "ok loss nan", None),
        ("vt-separated", "ok\x0bloss nan", None), ("chinese-line", "第3步 loss nan 了", None),
        ("custom", "Traceback (most recent call last):\n  File x\nValueError: bad\n",
         [r"^Traceback", r"Error:"]),
        ("custom-anchor", "  Traceback\nTraceback (x)", [r"^Traceback"]),
        ("custom-unicode-word", "错误：损失为 nan", [r"损失为\s*nan"]),
    ]:
        v.add(cid, {"text": text, "patterns": pats}, lambda text=text, pats=pats: run(text, pats))
    return v


# ================================================================ config files


class _ConfigFile:
    """Stands in for ctx.config_path: a fixed display path and in-memory text."""

    def __init__(self, text: str | None, shown: str = "/repo/arboria.toml"):
        self.text, self.shown = text, shown

    def is_file(self) -> bool:
        return self.text is not None

    def read_text(self, encoding: str = "utf-8") -> str:
        assert self.text is not None
        return self.text

    def __str__(self) -> str:
        return self.shown

    __fspath__ = __str__


def project_config_json(c: config.ProjectConfig) -> dict[str, Any]:
    return {"default_machine": c.default_machine, "total_su": c.total_su,
            "project_id": c.project_id,
            "watch": {"observation_minutes": c.watch.observation_minutes,
                      "fail_patterns": [p.pattern for p in c.watch.fail_patterns]},
            "wandb": {"entity": c.wandb.entity, "project": c.wandb.project,
                      "configured": c.wandb.configured,
                      "run_url_r001": c.wandb.run_url(f"{c.project_id or 'p'}-r001")},
            "run": {"command": list(c.run.command), "override_style": c.run.override_style},
            "claude": {"push": c.claude.push}}


def v_arboria_toml() -> Vectors:
    v = Vectors("arboria_toml", "arboria.config.project_config",
                "input.toml is the text of arboria.toml (null = no file); the file is shown as "
                "/repo/arboria.toml in messages. Output is the normalized ProjectConfig: "
                "fail_patterns as pattern strings (compiled with Python `re`), run.command split "
                "with shlex.split, wandb.run_url_r001 = run_url('<project_id or p>-r001'). "
                "Invalid-TOML and invalid-regex cases are left out (their messages come from "
                "tomllib / re).")
    full = """\
default_machine = "deltaai"

[project]
# 项目 id
id = "wavefront-3f2a"

[budget]
total_su = 200

[watch]
observation_minutes = 15
fail_patterns = ["(?i)loss[^\\\\n]{0,40}nan", "Traceback"]

[wandb]
entity = "cwx-lab"
project = "wavefront"

[run]
command = "uv run python train.py --config configs/base.yaml"
override_style = "argparse"

[claude]
push = false
"""
    for cid, text in [
        ("no-file", None), ("empty", ""), ("minimal-init", 'default_machine = ""\n\n[project]\n'
                                                           'id = "wavefront-3f2a"\n'),
        ("full", full), ("budget-float", "[budget]\ntotal_su = 150.5\n"),
        ("machine-stripped", 'default_machine = "  deltaai  "\n'),
        ("project-table-without-id", "[project]\nname = \"x\"\n"),
        ("wandb-partial", '[wandb]\nentity = "me"\n'), ("wandb-stripped",
                                                       '[wandb]\nentity = " me "\nproject = " p "\n'),
        ("run-quoted", "[run]\ncommand = \"python 'my train.py' --tag \\\"a b\\\"\"\n"),
        ("run-tyro", '[run]\ncommand = "python train.py"\noverride_style = "tyro"\n'),
        ("watch-empty-patterns", "[watch]\nfail_patterns = []\n"),
        ("err-machine-int", "default_machine = 3\n"), ("err-budget-not-table", "budget = 5\n"),
        ("err-budget-negative", "[budget]\ntotal_su = -1\n"),
        ("err-budget-bool", "[budget]\ntotal_su = true\n"),
        ("err-budget-string", '[budget]\ntotal_su = "100"\n'),
        ("err-project-not-table", 'project = "x"\n'),
        ("err-project-id-upper", '[project]\nid = "WaveFront"\n'),
        ("err-project-id-int", "[project]\nid = 5\n"),
        ("err-watch-not-table", "watch = 1\n"),
        ("err-watch-minutes-0", "[watch]\nobservation_minutes = 0\n"),
        ("err-watch-minutes-float", "[watch]\nobservation_minutes = 1.5\n"),
        ("err-watch-patterns-string", '[watch]\nfail_patterns = "nan"\n'),
        ("err-watch-pattern-empty", '[watch]\nfail_patterns = ["ok", ""]\n'),
        ("err-wandb-bad-name", '[wandb]\nentity = "my team"\n'),
        ("err-wandb-int", "[wandb]\nproject = 1\n"),
        ("err-run-command-list", '[run]\ncommand = ["python", "train.py"]\n'),
        ("err-run-unbalanced-quote", "[run]\ncommand = \"python 'train.py\"\n"),
        ("err-run-style", '[run]\noverride_style = "click"\n'),
        ("err-claude-push-string", '[claude]\npush = "yes"\n'),
        ("err-run-not-table", "run = 1\n"),
    ]:
        v.add(cid, {"toml": text}, lambda text=text: project_config_json(
            config.project_config(SimpleNamespace(config_path=_ConfigFile(text)))))
    return v


def v_add_project_id() -> Vectors:
    v = Vectors("add_project_id", "arboria.config.add_project_id",
                "`arb init`: arboria.toml text with [project] id added, every existing byte kept. "
                "The comment line is config.PROJECT_COMMENT.")
    for cid, text in [
        ("empty", ""), ("no-newline", 'default_machine = "deltaai"'),
        ("one-newline", 'default_machine = "deltaai"\n'),
        ("blank-line-end", 'default_machine = "deltaai"\n\n'),
        ("existing-header", '[project]\nname = "x"\n'),
        ("header-with-comment", "[ project ]  # ours\n[budget]\ntotal_su = 1\n"),
        ("header-later", 'default_machine = "d"\n\n[project]\n\n[run]\ncommand = "x"\n'),
        ("err-two-headers", "[project]\n[project]\n"),
        ("err-id-already", '[project]\nid = "old-1234"\n'),
        ("err-dotted-key", 'project.id = "x"\n'),
        ("crlf", 'default_machine = "deltaai"\r\n'),
    ]:
        v.add(cid, {"text": text, "project_id": PROJECT},
              lambda text=text: config.add_project_id(text, PROJECT))
    v.add("err-bad-id", {"text": "", "project_id": "Bad"},
          lambda: config.add_project_id("", "Bad"))
    return v


class _Resolved(PurePosixPath):
    """A main-worktree path that is already resolved (resolve() returns it unchanged)."""

    def resolve(self, strict: bool = False):  # type: ignore[override]
        return self


def v_derive_project_id() -> Vectors:
    v = Vectors("derive_project_id", "arboria.config.derive_project_id",
                "input.main is the main worktree path *after* Path.resolve() (absolute, symlinks "
                "resolved). slug = re.sub('[^a-z0-9]+', '-', name.lower()).strip('-')[:26]"
                ".rstrip('-') (Python str.lower(), Unicode), or 'proj' if empty; then '-' + first "
                "4 hex chars of sha256(str(main) as UTF-8).")
    for cid, main in [
        ("wavefront", "/Users/wchen38/code/WaveFront"), ("arboria", "/Users/wchen38/arboria"),
        ("linux-home", "/home/wchen38/src/my_repo"), ("spaces-punct", "/Users/me/My Repo! (v2)"),
        ("dashes-collapse", "/x/--a--b--"), ("long-name", "/x/" + "abcdefghij" * 4),
        ("cut-at-dash", "/x/" + "a" * 25 + "-bcd"), ("chinese", "/Users/me/研究项目"),
        ("mixed-unicode", "/Users/me/实验-wave2"), ("kelvin-sign", "/x/Kelvin"),
        ("dotted", "/x/.hidden.repo"), ("root-child", "/repo"), ("digits", "/x/2026"),
        ("worktree-like", "/Users/wchen38/code/WaveFront-dmd-distill"),
    ]:
        v.add(cid, {"main": main}, lambda main=main: config.derive_project_id(
            SimpleNamespace(main=_Resolved(main))))
    return v


def v_validate_project_id() -> Vectors:
    v = Vectors("validate_project_id", "arboria.config.validate_project_id",
                "Same rule as check_project, with config's message and `where` prefix.")
    for cid, val, where in [("ok", PROJECT, "project id"), ("err-upper", "WaveFront", "project id"),
                            ("err-where", "a_b", "/repo/arboria.toml: [project] id"),
                            ("err-int", 5, "project id"), ("err-long", "a" * 32, "project id"),
                            ("ok-31", PROJECT_31, "project id"), ("err-null", None, "project id"),
                            ("err-empty", "", "project id"), ("err-dash-start", "-a", "project id"),
                            ("quirk-trailing-newline-accepted", "proj\n", "project id")]:
        v.add(cid, {"value": val, "where": where},
              lambda val=val, where=where: config.validate_project_id(val, where))
    return v


def v_clusters_parse_profile() -> Vectors:
    v = Vectors("clusters_parse_profile", "arboria.clusters.parse_profile",
                "input.toml is clusters.toml text; the case parses it (tomllib) and validates "
                "table input.name with parse_profile(name, table, source). Output is the "
                "normalized Profile: partitions keyed by name plus partition_order (TOML order), "
                "mem_per_gpu_gb/billing_per_su/charge as floats, account stripped, account_set = "
                "non-empty and no '<' / '>'. Error messages name keys in TOML order where it "
                "matters (first bad partition wins).")
    base = DELTAAI_TOML

    def sub(old: str, new: str) -> str:
        assert old in base, old
        return base.replace(old, new)

    cases: list[tuple[str, str, str, str]] = [
        ("deltaai-measured", base, "deltaai", "/Users/wchen38/.config/arboria/clusters.toml"),
        ("deltaai-uv", DELTAAI_UV_TOML, "deltaai-uv", "clusters.toml"),
        ("fake", FAKE_TOML, "fake", "clusters.toml"),
        ("err-example-unfilled", EXAMPLE_TOML, "deltaai", "examples/clusters.toml"),
        ("example-proj-filled", EXAMPLE_TOML.replace("<proj>", "bjki"), "deltaai",
         "examples/clusters.toml"),
        ("minimal", '[m]\nhost = "h"\ngpus_per_node = 1\ncpus_per_gpu = 1\nmem_per_gpu_gb = 1\n'
                    'default_partition = "p"\n[m.partitions.p]\nmax_walltime = "1h"\n[m.paths]\n'
                    'runs = "/r"\ndata = "/d"\ncache = "/c"\nckpt = "/k"\nout = "/o/"\n', "m",
         "clusters.toml"),
        ("account-padded", sub('account = "bjki-dtai-gh"', 'account = "  bjki-dtai-gh "'),
         "deltaai", "clusters.toml"),
        ("account-empty", sub('account = "bjki-dtai-gh"', 'account = ""'), "deltaai",
         "clusters.toml"),
        ("partition-clock-time", sub('max_walltime = "48h"', 'max_walltime = "2-00:00:00"'),
         "deltaai", "clusters.toml"),
        ("charge-int", sub("charge = 2.0", "charge = 2"), "deltaai", "clusters.toml"),
        ("charge-default", sub("charge = 1.0\n", ""), "deltaai", "clusters.toml"),
        ("billing-float", sub("billing_per_su = 1000", "billing_per_su = 1000.5"), "deltaai",
         "clusters.toml"),
        ("path-trailing-slash", sub('out = "/projects/bjki/$USER/arboria/out"',
                                    'out = "/projects/bjki/$USER/arboria/out///"'), "deltaai",
         "clusters.toml"),
        ("err-charge-cmd", sub("billing_per_su = 1000", 'charge_cmd = "jobcharge"'), "deltaai",
         "clusters.toml"),
        ("err-unknown-keys", sub("max_segments = 4", "max_segments = 4\nqos = \"x\"\nzz = 1"),
         "deltaai", "clusters.toml"),
        ("err-kind", sub('kind = "slurm"', 'kind = "pbs"'), "deltaai", "clusters.toml"),
        ("err-host-missing", sub('host = "deltaai"\n', ""), "deltaai", "clusters.toml"),
        ("err-host-spaces", sub('host = "deltaai"', 'host = "delta ai"'), "deltaai",
         "clusters.toml"),
        ("quirk-host-trailing-newline-accepted", sub('host = "deltaai"', 'host = "deltaai\\n"'),
         "deltaai", "clusters.toml"),
        ("err-fake-no-root", FAKE_TOML.replace('root = "/tmp/arboria-fake/state"\n', ""), "fake",
         "clusters.toml"),
        ("err-root-on-slurm", sub('host = "deltaai"', 'host = "deltaai"\nroot = "/x"'), "deltaai",
         "clusters.toml"),
        ("err-account-int", sub('account = "bjki-dtai-gh"', "account = 5"), "deltaai",
         "clusters.toml"),
        ("err-account-two-words", sub('account = "bjki-dtai-gh"', 'account = "bjki dtai"'),
         "deltaai", "clusters.toml"),
        ("err-gpus-zero", sub("gpus_per_node = 4", "gpus_per_node = 0"), "deltaai",
         "clusters.toml"),
        ("err-gpus-bool", sub("gpus_per_node = 4", "gpus_per_node = true"), "deltaai",
         "clusters.toml"),
        ("err-cpus-missing", sub("cpus_per_gpu = 72\n", ""), "deltaai", "clusters.toml"),
        ("err-cpus-float", sub("cpus_per_gpu = 72", "cpus_per_gpu = 72.0"), "deltaai",
         "clusters.toml"),
        ("err-mem-zero", sub("mem_per_gpu_gb = 110.898", "mem_per_gpu_gb = 0"), "deltaai",
         "clusters.toml"),
        ("err-mem-string", sub("mem_per_gpu_gb = 110.898", 'mem_per_gpu_gb = "110"'), "deltaai",
         "clusters.toml"),
        ("err-max-segments-0", sub("max_segments = 4", "max_segments = 0"), "deltaai",
         "clusters.toml"),
        ("err-billing-negative", sub("billing_per_su = 1000", "billing_per_su = -1"), "deltaai",
         "clusters.toml"),
        ("err-arch-int", sub('arch = "aarch64"', "arch = 64"), "deltaai", "clusters.toml"),
        ("err-no-partitions", '[m]\nhost = "h"\ngpus_per_node = 1\ncpus_per_gpu = 1\n'
                              'mem_per_gpu_gb = 1\n[m.paths]\n', "m", "clusters.toml"),
        ("err-partition-unknown-key", sub('max_walltime = "48h"', 'max_walltime = "48h"\nqos = 1'),
         "deltaai", "clusters.toml"),
        ("err-partition-walltime", sub('max_walltime = "48h"', 'max_walltime = "2 days"'),
         "deltaai", "clusters.toml"),
        ("err-partition-walltime-missing", sub('max_walltime = "48h"\n', ""), "deltaai",
         "clusters.toml"),
        ("err-charge-zero", sub("charge = 2.0", "charge = 0"), "deltaai", "clusters.toml"),
        ("err-partition-name", sub('[deltaai.partitions."ghx4-interactive"]',
                                   '[deltaai.partitions."ghx4.int"]'), "deltaai", "clusters.toml"),
        ("err-default-partition", sub('default_partition = "ghx4"', 'default_partition = "gpu"'),
         "deltaai", "clusters.toml"),
        ("err-paths-missing", base.split("[deltaai.paths]")[0], "deltaai", "clusters.toml"),
        ("err-paths-unknown", sub('out = "/projects', 'tmp = "/tmp"\nout = "/projects'),
         "deltaai", "clusters.toml"),
        ("err-path-role-missing", sub('ckpt = "/work/hdd/bjki/$USER/arboria/ckpt"\n', ""),
         "deltaai", "clusters.toml"),
        ("err-path-relative", sub('runs = "/projects', 'runs = "projects'), "deltaai",
         "clusters.toml"),
        ("err-path-dotdot", sub('runs = "/projects/bjki', 'runs = "/projects/../bjki'), "deltaai",
         "clusters.toml"),
        ("err-path-space", sub('data = "/work/nvme/bjki', 'data = "/work/nvme/b jki'), "deltaai",
         "clusters.toml"),
        ("err-path-other-var", sub("cache = \"/projects/bjki/$USER", "cache = \"/projects/$HOME"),
         "deltaai", "clusters.toml"),
        ("err-path-root", sub('out = "/projects/bjki/$USER/arboria/out"', 'out = "/"'), "deltaai",
         "clusters.toml"),
        ("err-prelude-string", sub("prelude = []", 'prelude = "module load cuda"'), "deltaai",
         "clusters.toml"),
        ("err-prelude-newline", sub("prelude = []", 'prelude = ["a\\nb"]'), "deltaai",
         "clusters.toml"),
        ("err-bad-machine-name", "['bad name']\nkind = \"slurm\"\n", "bad name", "clusters.toml"),
    ]
    for cid, text, name, source in cases:
        v.add(cid, {"toml": text, "name": name, "source": source},
              lambda text=text, name=name, source=source:
              profile_json(parse_profile(name, tomllib.loads(text)[name], source)))
    v.add("err-not-a-table", {"toml": 'deltaai = "x"\n', "name": "deltaai",
                              "source": "clusters.toml"},
          lambda: profile_json(parse_profile("deltaai", "x", "clusters.toml")))
    return v


def v_check_remote_path() -> Vectors:
    v = Vectors("check_remote_path", "arboria.clusters.check_remote_path",
                "Remote profile path: absolute, segments [A-Za-z0-9._-]+ (not . or ..), $USER / "
                "${USER} the only variable; trailing '/' removed (a run of them).")
    for cid, val in [
        ("plain", "/projects/bjki/$USER/arboria/runs"), ("braced", "/work/hdd/bjki/${USER}/ckpt"),
        ("trailing", "/a/b/"), ("double-slash", "/a//b"), ("dots-in-name", "/a/.cache/v1.2"),
        ("user-suffix", "/a/$USER.old"), ("err-username", "/a/$USERNAME"),
        ("err-placeholder", "/projects/<proj>/$USER"), ("err-relative", "projects/x"),
        ("err-empty", ""), ("err-root", "/"), ("err-dotdot", "/a/../b"), ("err-dot", "/a/./b"),
        ("err-space", "/a b"), ("err-home", "~/x"), ("err-int", 5), ("err-unicode", "/数据"),
        ("quirk-trailing-newline-accepted", "/a/b\n"), ("quirk-inner-newline-accepted", "/a\n/b"),
    ]:
        v.add(cid, {"value": val, "where": "clusters.toml: [deltaai.paths] runs"},
              lambda val=val: check_remote_path(val, "clusters.toml: [deltaai.paths] runs"))
    return v


# ================================================================ machine init (fixture probe)


def v_machine_propose() -> Vectors:
    v = Vectors("machine_propose", "arboria.machine.propose",
                "input.probe is the raw output of machine.PROBE_SCRIPT (the DeltaAI case is "
                "tests/fixtures/deltaai_probe.txt); the case runs propose(name, host, "
                "split_sections(probe), account=..., existing=...). Output {values, warnings, "
                "other_partitions, accounts, tools, toml}; toml = render_profile(name, values, "
                "note) (values in propose's key order; floats written with Python repr).")
    probe = FIXTURE_PROBE.read_text(encoding="utf-8")
    existing = tomllib.loads(DELTAAI_UV_TOML)["deltaai-uv"]

    def run(name, host, text, account, ex, note):
        p = machine.propose(name, host, machine.split_sections(text), account=account,
                            existing=ex)
        return {"values": p.values, "warnings": p.warnings,
                "other_partitions": p.other_partitions,
                "accounts": [list(a) for a in p.accounts], "tools": p.tools,
                "toml": machine.render_profile(name, p.values, note)}

    two_accounts = probe.replace(
        "bjki-dtai-gh                               199               200  representation lear...",
        "bjki-dtai-gh                               199               200  representation lear...\n"
        "bdxx-dtai-gh                                50               100  other project")
    no_flags = probe.replace("PriorityFlags           = MAX_TRES", "PriorityFlags           = ")
    no_tools = probe.replace("/sw/user/scripts/accounts\n", "")
    for cid, name, host, text, account, ex, note in [
        ("deltaai", "deltaai", "deltaai", probe, None, None, "measured by arb machine init"),
        ("deltaai-account-given", "deltaai", "deltaai", probe, "bjki-dtai-gh", None, ""),
        ("deltaai-existing-kept", "deltaai", "deltaai", probe, None, existing, ""),
        ("two-accounts-unset", "deltaai", "deltaai", two_accounts, None, None, ""),
        ("two-accounts-chosen", "deltaai", "deltaai", two_accounts, "bdxx-dtai-gh", None, ""),
        ("no-max-tres", "deltaai", "deltaai", no_flags, None, None, ""),
        ("no-accounts-tool", "deltaai", "deltaai", no_tools, None, None, ""),
        ("quoted-name", "delta.ai", "deltaai", probe, None, None, ""),
        ("err-account-not-mine", "deltaai", "deltaai", probe, "bbka-dtai-gh", None, ""),
        ("err-no-partitions", "deltaai", "deltaai", "@@user\nme\n@@partitions\n@@end\n", None,
         None, ""),
    ]:
        v.add(cid, {"name": name, "host": host, "probe": text, "account": account,
                    "existing": ex, "note": note},
              lambda name=name, host=host, text=text, account=account, ex=ex, note=note:
              run(name, host, text, account, ex, note))
    return v


def v_parse_accounts() -> Vectors:
    v = Vectors("parse_accounts", "arboria.machine.parse_accounts",
                "NCSA `accounts` output -> [[account, balance]]: every non-blank line after the "
                "first line that fully matches [-\\s]{6,} and contains '-' (Python re \\s, i.e. "
                "Unicode whitespace) is a row; split on whitespace (Python str.split), column 1 "
                "is the account, column 2 the balance as text ('' if missing). Later dashed "
                "lines are skipped too; lines are split with str.splitlines (\\n, \\r\\n, "
                "\\r and the other Unicode line breaks). The real DeltaAI case is the @@accounts "
                "section of tests/fixtures/deltaai_probe.txt.")
    real = machine.split_sections(FIXTURE_PROBE.read_text(encoding="utf-8"))["accounts"]
    header = ("Project Summary for User 'wchen38':\n\n"
              "Account                        Balance(Hours)   Deposited(Hours)  Project\n"
              "----------------------------  ----------------  ----------------  "
              "----------------------\n")
    row = "bjki-dtai-gh                               199               200  representation lear..."
    for cid, text in [
        ("deltaai", real),
        ("two-accounts", header + row + "\nbdxx-dtai-gh                                50"
         "               100  other project\n"),
        ("blank-lines-skipped", header + "\n" + row + "\n\n   \n"),
        ("crlf", (header + row + "\n").replace("\n", "\r\n")),
        ("no-balance-column", header + "bjki-dtai-gh\n"),
        ("decimal-and-negative", header + "a-gh   12.5   20\nb-gh   -3   10\n"),
        ("header-only", header),
        ("no-rule", "Account Balance\nbjki-dtai-gh 199\n"),
        ("empty", ""),
        ("command-not-found", "bash: accounts: command not found\n"),
        ("rule-5-dashes-not-a-rule", "Account\n-----\nbjki 1\n"),
        ("rule-6-dashes", "Account\n------\nbjki 1\n"),
        ("rule-spaces-and-tabs", "Account\n--- \t ---\nbjki 1\n"),
        ("rule-all-spaces-not-a-rule", "Account\n        \nbjki 1\n"),
        ("later-rules-skipped", header + row + "\n----------\nbdxx-dtai-gh 50 100\n"),
        ("quirk-footer-is-a-row", header + row + "\nTotal: 199 hours\n"),
        ("leading-indent", header + "   bjki-dtai-gh   199   200\n"),
        ("unicode-space-rule", "Account\n---\u3000---\nbjki 1\n"),
    ]:
        v.add(cid, {"text": text},
              lambda text=text: [list(a) for a in machine.parse_accounts(text)])
    return v


def v_parse_weights() -> Vectors:
    v = Vectors("parse_weights", "arboria.machine.parse_weights",
                "TRESBillingWeights -> {cpu, mem (per GB), gpu}; mem units K/M/''/G/T scale by "
                "1024*1024 / 1024 / 1024 / 1 / 1/1024 (f64).")
    for cid, text in [
        ("ghx4", "CPU=13.888888,Mem=9.01726G,GRES/gpu=1000"),
        ("ghx4-interactive", "CPU=27.777776,Mem=18.03452G,GRES/gpu=2000"),
        ("full-tiny", "CPU=0.00000001,Mem=0.0000001G,GRES/gpu=0.00000001"),
        ("mem-mb", "Mem=0.25M"), ("mem-no-unit", "Mem=2"), ("mem-kb", "Mem=1K"),
        ("mem-tb", "Mem=1024T"), ("gpu-plain-key", "gpu=1000"), ("lowercase", "cpu=1,mem=2g"),
        ("bad-value", "CPU=abc,GRES/gpu=1000"), ("empty", ""), ("leading-dot", "CPU=.5"),
        ("other-tres", "Node=10,GRES/gpu:a100=500,CPU=2"),
    ]:
        v.add(cid, {"text": text}, lambda text=text: machine.parse_weights(text))
    return v


# ================================================================ ids, names, refs


class _IdsLedger:
    """What Ledger.next_id touches: `.conn` with a table of text ids."""

    def __init__(self, table: str, ids: list[str]):
        import sqlite3

        self.conn = sqlite3.connect(":memory:")
        self.conn.execute(f"CREATE TABLE {table} (id TEXT PRIMARY KEY)")
        self.conn.executemany(f"INSERT INTO {table} VALUES (?)", [(i,) for i in ids])


def v_next_id() -> Vectors:
    v = Vectors("next_id", "arboria.db.Ledger.next_id",
                "Next text id: prefix + (max numeric suffix + 1) zero-padded to 3 digits. The "
                "suffix is id[len(prefix):] whether or not the id starts with the prefix; "
                "suffixes that are not all digits are ignored (str.isdigit). Gaps are not filled. "
                "Prefixes in use: r (runs), d (decisions), t (tasks), l (lessons).")
    for cid, table, prefix, ids in [
        ("runs-empty", "runs", "r", []), ("runs-one", "runs", "r", ["r001"]),
        ("runs-9", "runs", "r", ["r001", "r009"]), ("runs-gap", "runs", "r", ["r001", "r005"]),
        ("runs-99", "runs", "r", ["r099"]), ("runs-999", "runs", "r", ["r999"]),
        ("runs-1000", "runs", "r", ["r999", "r1000"]), ("runs-unordered", "runs", "r",
                                                        ["r010", "r002", "r007"]),
        ("runs-short-pad", "runs", "r", ["r1"]), ("runs-non-numeric", "runs", "r",
                                                  ["rabc", "r00x", "r002"]),
        ("runs-only-non-numeric", "runs", "r", ["rabc"]),
        ("runs-foreign-prefix", "runs", "r", ["x005", "r001"]),
        ("decisions", "decisions", "d", ["d001", "d002"]), ("tasks", "tasks", "t", ["t041"]),
        ("tasks-empty", "tasks", "t", []), ("lessons", "lessons", "l", ["l003"]),
        ("runs-zero", "runs", "r", ["r000"]), ("runs-negative-like", "runs", "r", ["r-5", "r002"]),
    ]:
        v.add(cid, {"table": table, "prefix": prefix, "existing": ids},
              lambda table=table, prefix=prefix, ids=ids:
              db.Ledger.next_id(_IdsLedger(table, ids), table, prefix))  # type: ignore[arg-type]
    return v


def v_validate_name() -> Vectors:
    v = Vectors("validate_name", "arboria.db.validate_name",
                "Idea and seed names: ^[a-z0-9][a-z0-9-]{0,62}$ (1..63 chars). input.what is the "
                "word in the message ('idea' or 'seed').")
    for cid, name, what in [
        ("typical", "dmd-distill", "idea"), ("digit-start", "2nd-try", "idea"), ("one", "a", "idea"),
        ("63", "a" * 63, "idea"), ("err-64", "a" * 64, "idea"), ("err-upper", "DMD", "idea"),
        ("err-dash-start", "-x", "idea"), ("err-underscore", "a_b", "idea"),
        ("err-space", "a b", "idea"), ("err-empty", "", "idea"), ("err-dot", "v1.2", "seed"),
        ("err-chinese", "蒸馏", "seed"), ("quirk-trailing-newline-accepted", "abc\n", "idea"),
        ("seed-ok", "low-rank-adam", "seed"), ("trailing-dash", "x-", "seed"),
    ]:
        v.add(cid, {"name": name, "what": what},
              lambda name=name, what=what: db.validate_name(name, what))
    v.add("err-null", {"name": None, "what": "idea"}, lambda: db.validate_name(None))
    return v


def v_command_name() -> Vectors:
    v = Vectors("command_name", "arboria.commands.validate_name",
                "Notebook command names: ^[a-z][a-z0-9_]{0,40}$ (called with check_reserved=False;"
                " the reserved set is whatever `arboria.lab` defines and is not part of this "
                "vector).")
    for cid, name in [("typical", "svd"), ("underscore", "loss_curve"), ("digits", "top5"),
                      ("41", "a" * 41), ("err-42", "a" * 42), ("err-digit-start", "5x"),
                      ("err-dash", "loss-curve"), ("err-upper", "Svd"), ("err-empty", ""),
                      ("err-underscore-start", "_x"), ("err-int", 5)]:
        v.add(cid, {"name": name},
              lambda name=name: commands.validate_name(name, check_reserved=False))
    return v


def v_git_refs() -> Vectors:
    v = Vectors("git_refs", "arboria.records.run_ref (+ inspection.ref_name, ideas.branch_for, "
                "ideas.worktree_for)",
                "Git names arb writes. kind=run: records.run_ref (refs/arboria/runs/<run>); "
                "kind=inspect: inspection.ref_name (refs/arboria/inspect/<todo>); kind=branch: "
                "ideas.branch_for (idea/<name>); kind=worktree: ideas.worktree_for (sibling folder "
                "<main>-<idea> of the main worktree; input.main is that path).")
    for cid, kind, arg, main in [
        ("run", "run", "r001", None), ("run-1000", "run", "r1000", None),
        ("inspect", "inspect", "t007", None), ("branch", "branch", "dmd-distill", None),
        ("worktree", "worktree", "dmd-distill", "/Users/wchen38/code/WaveFront"),
        ("worktree-root", "worktree", "x", "/repo"),
        ("inspect-t1000", "inspect", "t1000", None), ("branch-digits", "branch", "lr-3e-4", None),
        ("worktree-trailing-slash", "worktree", "lr-3e-4", "/repo/"),
        ("worktree-cjk-parent", "worktree", "dmd", "/Users/wchen38/研究/WaveFront"),
    ]:
        def run(kind=kind, arg=arg, main=main):
            if kind == "run":
                return records.run_ref(arg)
            if kind == "inspect":
                return inspection.ref_name(arg)
            if kind == "branch":
                return ideas.branch_for(arg)
            return str(ideas.worktree_for(SimpleNamespace(main=PurePosixPath(main)), arg))
        v.add(cid, {"kind": kind, "arg": arg, "main": main}, run)
    return v


# ================================================================ LLM-writable fields


def v_validate_text() -> Vectors:
    v = Vectors("validate_text", "arboria.db.validate_text",
                "Length limits of the free-text fields an LLM may write (db.LIMITS, counted in "
                "Unicode code points after Python str.strip(), which also strips U+001C..U+001F, "
                "U+0085, U+00A0, U+3000 etc.). Output is the stripped value, or null for an "
                "optional (required=false) empty/None value. summary: at most 10 lines (counting "
                "'\\n' only). lesson: one line, no '<!--' / '-->'.")
    for field, limit in sorted(db.LIMITS.items()):
        for cid, value in [(f"{field}-at-limit", "x" * limit),
                           (f"err-{field}-over-limit", "x" * (limit + 1))]:
            v.add(cid, {"field": field, "value": value, "required": True},
                  lambda field=field, value=value: db.validate_text(field, value))
    cjk = "批" * db.LIMITS["metric"]
    for cid, field, value, req in [
        ("strip", "note", "  hello  \n", True), ("strip-ideographic-space", "note", "　hi　",
                                                  True),
        ("strip-unit-separator", "note", "\x1fhi\x1c", True), ("strip-nbsp", "note", "\xa0hi\x85",
                                                              True),
        ("zero-width-not-stripped", "note", "​hi", True),
        ("cjk-at-limit", "metric", cjk, True), ("err-cjk-over-limit", "metric", cjk + "批", True),
        ("emoji-counts-one", "metric", "😀" * db.LIMITS["metric"], True),
        ("combining-counts-two", "lesson", "é" * 100, True),
        ("err-combining-over", "lesson", "é" * 100 + "x", True),
        ("strip-then-count", "metric", " " * 50 + "x" * db.LIMITS["metric"] + " " * 50, True),
        ("err-required-none", "hypothesis", None, True), ("err-required-empty", "metric", "  ", True),
        ("optional-none", "note", None, False), ("optional-empty", "result", " \n ", False),
        ("optional-value", "result", "done", False),
        ("summary-10-lines", "summary", "\n".join(["l"] * 10), True),
        ("err-summary-11-lines", "summary", "\n".join(["l"] * 11), True),
        ("summary-trailing-newlines-stripped", "summary", "\n".join(["l"] * 10) + "\n\n\n", True),
        ("summary-cr-not-counted", "summary", "\r".join(["l"] * 20), True),
        ("lesson-ok", "lesson", "ghx4 节点上 torch.compile 要先设 TRITON_CACHE_DIR", True),
        ("err-lesson-newline", "lesson", "a\nb", True), ("err-lesson-cr", "lesson", "a\rb", True),
        ("err-lesson-comment-open", "lesson", "x <!-- y", True),
        ("err-lesson-comment-close", "lesson", "x --> y", True),
        ("note-multiline-ok", "note", "第一行\n第二行", True),
        ("gist", "gist", "用低秩 Adam 状态省显存", True),
        ("question", "question", "DMD 的训练循环", True),
    ]:
        v.add(cid, {"field": field, "value": value, "required": req},
              lambda field=field, value=value, req=req:
              db.validate_text(field, value, required=req))
    return v


def v_validate_question() -> Vectors:
    v = Vectors("validate_question", "arboria.inspection.validate_question",
                "lab.inspect's question: a string, validate_text('question') (<= 300), then "
                "internal whitespace runs collapsed to one space (Python str.split()).")
    for cid, q in [("plain", "DMD 的训练循环"), ("collapse", "  how   does\n\tthe loss　work "),
                   ("at-limit", "q" * 300), ("err-over", "q" * 301), ("err-empty", "   "),
                   ("err-int", 5), ("err-null", None), ("cjk-at-limit", "问" * 300),
                   ("err-limit-before-collapse", "a" + " " * 299 + "b"),
                   ("strip-then-limit", " " * 10 + "q" * 300 + "\n"),
                   ("inner-newlines", "DMD\n的\n\n训练循环")]:
        v.add(cid, {"question": q}, lambda q=q: inspection.validate_question(q))
    return v


def v_check_mermaid() -> Vectors:
    v = Vectors("check_mermaid", "arboria.inspection.check_mermaid",
                "The map view's Mermaid source: <= 8000 chars (stripped), no control chars except "
                "\\t \\n, first non-empty non-%% line starts with a diagram type, no %%{ "
                "directives, no click/href/javascript:, no HTML tags except <br>.")
    for cid, src in [
        ("flowchart", "flowchart TD\n  A[数据] --> B[训练]\n  B --> C{评估}"),
        ("graph-lr", "graph LR\nA-->B"), ("comment-first", "%% overview\nsequenceDiagram\nA->>B: hi"),
        ("blank-first", "\n\n  classDiagram\n  A <|-- B"), ("br-ok", "flowchart TD\nA[a<br>b<br/>c]"),
        ("br-upper", "graph TD\nA[a<BR />b]"), ("tab-ok", "graph TD\n\tA-->B"),
        ("stripped", "  graph TD\nA-->B  \n"), ("err-crlf", "graph TD\r\nA-->B\r\n"),
        ("arrow-not-tag", "graph TD\nA -- text --> B\nC<-->D"),
        ("err-empty", ""), ("err-no-type", "A-->B"), ("err-unknown-type", "chart TD\nA-->B"),
        ("err-only-comment", "%% nothing"), ("err-init", "%%{init: {'theme': 'dark'}}%%\ngraph TD"),
        ("err-init-later", "graph TD\nA-->B\n%%{init: {}}%%"),
        ("err-click", "graph TD\nA-->B\nclick A callback"),
        ("err-click-indented", "graph TD\n   click A href \"x\""),
        ("err-href", "graph TD\nA[HREF]"), ("err-javascript", "graph TD\nA[JavaScript:alert]"),
        ("err-html", "graph TD\nA[<b>bold</b>]"), ("err-html-spaced", "graph TD\nA[< img>]"),
        ("err-html-comment", "graph TD\nA[<!-- x -->]"), ("err-control", "graph TD\nA\x07-->B"),
        ("err-del", "graph TD\nA\x7f"), ("err-over-limit", "graph TD\n" + "A" * 7992),
        ("at-limit", "graph TD\n" + "A" * 7991), ("err-not-string", 5),
        ("clicked-word-ok", "graph TD\nclicked-->B"),
    ]:
        v.add(cid, {"src": src}, lambda src=src: inspection.check_mermaid(src))
    return v


def v_check_path() -> Vectors:
    v = Vectors("inspect_check_path", "arboria.inspection.check_path",
                "A span's file path: repo-relative, POSIX, 1..300 chars, no leading '/', no '\\\\', "
                "no empty / . / .. segment.")
    for cid, p in [("plain", "src/train.py"), ("top", "train.py"), ("dotfile", ".github/x.yml"),
                   ("unicode", "笔记/说明.md"), ("300", "a" * 300), ("err-301", "a" * 301),
                   ("err-abs", "/etc/passwd"), ("err-dotdot", "src/../x"), ("err-dot", "./x"),
                   ("err-double-slash", "a//b"), ("err-trailing-slash", "src/"),
                   ("err-backslash", "src\\x.py"), ("err-empty", ""), ("err-int", 3),
                   ("err-null", None)]:
        v.add(cid, {"path": p}, lambda p=p: inspection.check_path(p))
    return v


# ---------------------------------------------------------------- validate_result (real git)

INSPECT_FILES = {
    "src/train.py": "".join(f"line {i}\n" for i in range(1, 501)),
    "src/model.py": "import torch\n\n\nclass Net(torch.nn.Module):\n    def forward(self, x):\n"
                    "        return x  # 恒等\n",
    "crlf.py": "a = 1\r\nb = 2\r\n",
    "no_newline.py": "x = 1\ny = 2",
    "blank_lines.py": "\n\n\n",
    "README.md": "# 项目\n说明 <b>粗体</b>\n",
    "data.bin": "PK\u0000\u0001binary",
    "empty.py": "",
}


def make_tree(repo: Path, files: dict[str, str]) -> str:
    """Write `files` as blobs and a tree (no commit, no ref): its id depends only on content."""
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env["GIT_CONFIG_NOSYSTEM"] = "1"
    env["GIT_CONFIG_GLOBAL"] = os.devnull
    git = lambda *a, **kw: subprocess.run(["git", *a], cwd=repo, env=kw.pop("env", env),  # noqa
                                          check=True, capture_output=True, **kw)
    git("init", "-q", "--object-format=sha1")
    index_env = env | {"GIT_INDEX_FILE": str(repo / ".git" / "vectors-index")}
    for path, text in sorted(files.items()):
        sha = git("hash-object", "-w", "--no-filters", "--stdin",
                  input=text.encode("utf-8")).stdout.decode().strip()
        git("update-index", "--add", "--cacheinfo", f"100644,{sha},{path}", env=index_env)
    return git("write-tree", env=index_env).stdout.decode().strip()


def v_inspect_validate_result() -> Vectors:
    v = Vectors("inspect_validate_result", "arboria.inspection.validate_result",
                "input.files is the repo content at code_ref (UTF-8 text, written as git blobs "
                "into one tree, mode 100644); input.code_ref is that tree's sha1 (git accepts a "
                "tree-ish for <commit>:<path>; a Rust test can rebuild it with hash-object + "
                "update-index + write-tree and must get the same id). input.task is the inspect "
                "todo (payload_json as an object). The agent's answer is input.answer; output is "
                "what is stored (spans get 'lines' = the file's lines start..end, split on '\\n' "
                "only so '\\r' stays, a final empty line dropped; line_notes sorted by line).")
    with tempfile.TemporaryDirectory(prefix="arboria-vectors-") as tmp:
        repo = Path(tmp)
        tree = make_tree(repo, INSPECT_FILES)

        def task(view: str, kind: str = "inspect") -> dict[str, Any]:
            return {"id": "t007", "kind": kind, "payload": {
                "question": "DMD 的训练循环", "view": view, "target": "idea", "idea": "dmd-distill",
                "code_ref": tree, "tree": "4b825dc642cb6eb9a060e54bf8d69288fbee4904"}}

        def run(t: dict[str, Any], answer: Any) -> Any:
            row = {"id": t["id"], "kind": t["kind"], "payload_json": json.dumps(t["payload"])}
            return inspection.validate_result(repo, row, answer)

        span = {"path": "src/train.py", "start": 10, "end": 12}
        mer = "flowchart TD\n  load --> step\n  step --> eval_1"
        cases: list[tuple[str, str, Any]] = [
            ("code-minimal", "code", {"title": "训练循环", "spans": [span]}),
            ("code-full", "code", {"title": " 训练循环 ", "summary": "每步先算 loss", "spans": [
                {"id": "loop", "path": "src/train.py", "start": 1, "end": 3, "note": "主循环",
                 "line_notes": {"3": "第三行", "1": "第一行"}},
                {"path": "src/model.py", "start": 4, "end": 6, "line_notes": {"6": "恒等 <x>"}}]}),
            ("code-default-ids", "code", {"title": "t", "spans": [span, span, span]}),
            ("code-crlf", "code", {"title": "t", "spans": [{"path": "crlf.py", "start": 1,
                                                             "end": 2}]}),
            ("code-no-final-newline", "code", {"title": "t", "spans": [
                {"path": "no_newline.py", "start": 2, "end": 2}]}),
            ("code-blank-lines", "code", {"title": "t", "spans": [
                {"path": "blank_lines.py", "start": 1, "end": 3}]}),
            ("code-400-lines", "code", {"title": "t", "spans": [
                {"path": "src/train.py", "start": 1, "end": 200},
                {"path": "src/train.py", "start": 301, "end": 500}]}),
            ("code-null-optional", "code", {"title": "t", "summary": None, "spans": [
                {"path": "src/train.py", "start": 5, "end": 5, "note": None, "line_notes": None}]}),
            ("pseudo", "pseudo", {"title": "伪代码", "text": "for batch in data:\n  step()",
                                  "spans": [span]}),
            ("pseudo-no-spans", "pseudo", {"title": "伪代码", "text": "loop"}),
            ("simple-default-lang", "simple", {"title": "t", "text": "def f(): pass"}),
            ("simple-lang", "simple", {"title": "t", "text": "x", "lang": "c++"}),
            ("map", "map", {"title": "架构", "mermaid": mer, "spans": [
                dict(span, id="s-load"), {"id": "s_step", "path": "src/model.py", "start": 1,
                                          "end": 1}],
                "nodes": {"load": "s-load", "step": ["s_step", "s-load"]}}),
            ("map-no-nodes", "map", {"title": "t", "mermaid": "graph TD\nA-->B"}),
            ("err-not-object", "code", ["title"]),
            ("err-unknown-key-code", "code", {"title": "t", "spans": [span], "text": "x"}),
            ("err-unknown-key-map", "map", {"title": "t", "mermaid": "graph TD", "lang": "x"}),
            ("err-no-title", "code", {"spans": [span]}),
            ("err-title-long", "code", {"title": "标" * 81, "spans": [span]}),
            ("title-80", "code", {"title": "标" * 80, "spans": [span]}),
            ("err-title-int", "code", {"title": 5, "spans": [span]}),
            ("err-summary-long", "code", {"title": "t", "summary": "s" * 601, "spans": [span]}),
            ("err-code-no-spans", "code", {"title": "t", "spans": []}),
            ("err-spans-not-list", "pseudo", {"title": "t", "text": "x", "spans": {"a": 1}}),
            ("err-7-spans", "code", {"title": "t", "spans": [span] * 7}),
            ("err-span-not-object", "code", {"title": "t", "spans": ["src/train.py"]}),
            ("err-span-unknown-key", "code", {"title": "t", "spans": [dict(span, lines=[])]}),
            ("err-span-dup-id", "code", {"title": "t", "spans": [dict(span, id="a"),
                                                                 dict(span, id="a")]}),
            ("err-span-dup-default-id", "code", {"title": "t", "spans": [span, dict(span,
                                                                                     id="s1")]}),
            ("err-span-bad-id", "code", {"title": "t", "spans": [dict(span, id="a b")]}),
            ("err-span-path", "code", {"title": "t", "spans": [dict(span, path="../x")]}),
            ("err-span-missing-file", "code", {"title": "t", "spans": [dict(span,
                                                                            path="nope.py")]}),
            ("err-span-directory", "code", {"title": "t", "spans": [dict(span, path="src")]}),
            ("err-span-binary", "code", {"title": "t", "spans": [{"path": "data.bin", "start": 1,
                                                                   "end": 1}]}),
            ("err-span-start-string", "code", {"title": "t", "spans": [dict(span, start="10")]}),
            ("err-span-end-bool", "code", {"title": "t", "spans": [dict(span, end=True)]}),
            ("err-span-start-0", "code", {"title": "t", "spans": [dict(span, start=0)]}),
            ("err-span-end-before-start", "code", {"title": "t", "spans": [dict(span, start=5,
                                                                                end=4)]}),
            ("err-span-past-end", "code", {"title": "t", "spans": [{"path": "src/model.py",
                                                                     "start": 1, "end": 7}]}),
            ("err-span-empty-file", "code", {"title": "t", "spans": [{"path": "empty.py",
                                                                       "start": 1, "end": 1}]}),
            ("err-401-lines", "code", {"title": "t", "spans": [
                {"path": "src/train.py", "start": 1, "end": 200},
                {"path": "src/train.py", "start": 300, "end": 500}]}),
            ("err-line-notes-list", "code", {"title": "t", "spans": [dict(span,
                                                                          line_notes=["x"])]}),
            ("err-line-note-outside", "code", {"title": "t", "spans": [dict(span, line_notes={
                "13": "x"})]}),
            ("err-line-note-key-not-digits", "code", {"title": "t", "spans": [dict(
                span, line_notes={"+10": "x"})]}),
            ("err-line-note-empty", "code", {"title": "t", "spans": [dict(span, line_notes={
                "10": "  "})]}),
            ("err-line-note-long", "code", {"title": "t", "spans": [dict(span, line_notes={
                "10": "n" * 201})]}),
            ("line-note-leading-zero-key", "code", {"title": "t", "spans": [dict(
                span, line_notes={"010": "x"})]}),
            ("err-41-line-notes", "code", {"title": "t", "spans": [
                {"path": "src/train.py", "start": 1, "end": 30,
                 "line_notes": {str(i): "n" for i in range(1, 31)}},
                {"path": "src/train.py", "start": 31, "end": 41,
                 "line_notes": {str(i): "n" for i in range(31, 42)}}]}),
            ("err-span-note-long", "code", {"title": "t", "spans": [dict(span, note="n" * 601)]}),
            ("err-pseudo-no-text", "pseudo", {"title": "t"}),
            ("err-pseudo-text-long", "pseudo", {"title": "t", "text": "x" * 8001}),
            ("err-simple-bad-lang", "simple", {"title": "t", "text": "x", "lang": "Python 3"}),
            ("err-map-no-mermaid", "map", {"title": "t"}),
            ("err-map-bad-mermaid", "map", {"title": "t", "mermaid": "A-->B"}),
            ("err-map-nodes-list", "map", {"title": "t", "mermaid": mer, "nodes": ["load"]}),
            ("err-map-node-missing", "map", {"title": "t", "mermaid": mer, "spans": [span],
                                             "nodes": {"eval": "s1"}}),
            ("err-map-node-bad-id", "map", {"title": "t", "mermaid": mer, "spans": [span],
                                            "nodes": {"a b": "s1"}}),
            ("err-map-node-unknown-span", "map", {"title": "t", "mermaid": mer, "spans": [span],
                                                  "nodes": {"load": "s9"}}),
            ("err-map-node-empty-refs", "map", {"title": "t", "mermaid": mer, "spans": [span],
                                                "nodes": {"load": []}}),
            ("map-node-word-boundary", "map", {"title": "t", "mermaid": mer, "spans": [span],
                                               "nodes": {"eval_1": "s1"}}),
        ]
        for cid, view, answer in cases:
            t = task(view)
            v.add(cid, {"files": INSPECT_FILES, "code_ref": tree, "task": t, "answer": answer},
                  lambda t=t, answer=answer: run(t, answer))
        t = task("code", kind="plain")
        v.add("err-not-inspect-todo", {"files": INSPECT_FILES, "code_ref": tree, "task": t,
                                       "answer": {"title": "t"}},
              lambda: run(t, {"title": "t"}))
    return v


# ================================================================ constants and event kinds


def event_kinds() -> list[str]:
    """Every literal first argument of a `<x>.event(...)` call in src/arboria, plus the
    backup events (some kinds are chosen at runtime, e.g. todo_drop / run_cancel)."""
    kinds: set[str] = set()
    for path in sorted((ROOT / "src" / "arboria").rglob("*.py")):
        for node in ast.walk(ast.parse(path.read_text(encoding="utf-8"))):
            if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                    and node.func.attr == "event" and node.args
                    and isinstance(node.args[0], ast.Constant)
                    and isinstance(node.args[0].value, str)):
                kinds.add(node.args[0].value)
    return sorted(kinds | db.BACKUP_EVENTS)


def v_constants() -> Vectors:
    v = Vectors("constants", "arboria (module constants)",
                "Values the Rust port must agree on. Each case's input names the constant; "
                "tuples/frozensets as JSON lists (frozensets sorted), regexes as their pattern "
                "source (Python `re` syntax). event_kinds: every literal Ledger.event kind in "
                "src/arboria plus the backup events; backup_events: kinds after which the "
                "ledger is dumped and committed (db.BACKUP_EVENTS).")
    from arboria import clusters

    items: list[tuple[str, Any]] = [
        ("db.SCHEMA_VERSION", db.SCHEMA_VERSION), ("db.LIMITS", db.LIMITS),
        ("db.SUMMARY_MAX_LINES", db.SUMMARY_MAX_LINES), ("db.NAME_RE", db.NAME_RE.pattern),
        ("db.CONFIDENCE", db.CONFIDENCE), ("db.IDEA_STATUS", db.IDEA_STATUS),
        ("db.RUN_STATUS", db.RUN_STATUS), ("db.RUN_ACTIVE", db.RUN_ACTIVE),
        ("db.RUN_FINAL", db.RUN_FINAL), ("db.RUN_NOT_REUSABLE", db.RUN_NOT_REUSABLE),
        ("db.JOB_STATES", db.JOB_STATES), ("db.JOB_TERMINAL", db.JOB_TERMINAL),
        ("db.SUBJECT_TYPES", db.SUBJECT_TYPES), ("db.LESSON_STATUS", db.LESSON_STATUS),
        ("db.LESSON_SOURCES", db.LESSON_SOURCES), ("db.TASK_STATUS", db.TASK_STATUS),
        ("db.TASK_KINDS", db.TASK_KINDS), ("db.INSPECT_VIEWS", db.INSPECT_VIEWS),
        ("db.SEED_STATUS", db.SEED_STATUS), ("db.BACKUP_EVENTS", sorted(db.BACKUP_EVENTS)),
        ("event_kinds", event_kinds()),
        ("slurm.MIN_WALLTIME_S", slurm.MIN_WALLTIME_S), ("slurm.MAX_JOB_NAME", slurm.MAX_JOB_NAME),
        ("slurm.JOB_NAME_RE", slurm.JOB_NAME_RE.pattern), ("slurm.PROJECT_RE", slurm.PROJECT_RE.pattern),
        ("slurm._OVERRIDE_KEY_RE", slurm._OVERRIDE_KEY_RE.pattern),
        ("clusters.KINDS", clusters.KINDS), ("clusters.PATH_ROLES", clusters.PATH_ROLES),
        ("clusters.TOP_KEYS", sorted(clusters.TOP_KEYS)),
        ("clusters.PARTITION_KEYS", sorted(clusters.PARTITION_KEYS)),
        ("config.OVERRIDE_STYLES", config.OVERRIDE_STYLES),
        ("config.DEFAULT_OVERRIDE_STYLE", config.DEFAULT_OVERRIDE_STYLE),
        ("config.DEFAULT_OBSERVATION_MINUTES", config.DEFAULT_OBSERVATION_MINUTES),
        ("config.DEFAULT_FAIL_PATTERNS", config.DEFAULT_FAIL_PATTERNS),
        ("config.PROJECT_COMMENT", config.PROJECT_COMMENT),
        ("commands.NAME_RE", commands.NAME_RE.pattern),
        ("commands.RESERVED_KWARGS", commands.RESERVED_KWARGS),
        ("inspection.MAX_SPANS", inspection.MAX_SPANS), ("inspection.MAX_LINES", inspection.MAX_LINES),
        ("inspection.MAX_LINE_NOTES", inspection.MAX_LINE_NOTES),
        ("inspection.MAX_NODES", inspection.MAX_NODES),
        ("inspection.MAX_FILE_BYTES", inspection.MAX_FILE_BYTES),
        ("inspection.MERMAID_KINDS", inspection.MERMAID_KINDS),
        ("inspection.REF_PREFIX", inspection.REF_PREFIX),
        ("submission.QUESTION", submission.QUESTION), ("submission.OPTIONS", submission.OPTIONS),
        ("monitor.HIT_LINE_MAX", monitor.HIT_LINE_MAX), ("monitor.TAIL_LINES", monitor.TAIL_LINES),
        ("backends.ssh.SSH_OPTS", backend_ssh.SSH_OPTS),
        ("backends.ssh.FIND_WINDOW", backend_ssh.FIND_WINDOW),
        ("backends.base.JOB_ID_RE", backend_base.JOB_ID_RE.pattern),
        ("backends.base._SLURM_STATES", backend_base._SLURM_STATES),
    ]
    for cid, value in items:
        v.add(cid, {"name": cid}, lambda value=value: list(value)
              if isinstance(value, tuple) else value)
    return v


# ================================================================ all


BUILDERS: list[Callable[[], Vectors | list[Vectors]]] = [
    v_canonical_json, v_card_payload, v_card_hash, v_build_plan,
    v_parse_walltime, v_slurm_time, v_human_duration, v_segment_plan, v_billed_gpus,
    v_check_override, v_parse_override, v_merge_overrides, v_render_overrides, v_build_command,
    v_check_project, v_job_name, v_parse_job_name, v_job_project, path_vectors,
    v_sbatch_value, v_dq, v_shell_quote, v_shell_split, v_render_sbatch,
    v_map_slurm_state, v_su_from_billing, v_sacct_poll, v_find_job, v_check_job_ids,
    v_restrict_path, v_derive_status, v_billed_su, v_first_hit,
    v_arboria_toml, v_add_project_id, v_derive_project_id, v_validate_project_id,
    v_clusters_parse_profile, v_check_remote_path, v_machine_propose, v_parse_accounts, v_parse_weights,
    v_next_id, v_validate_name, v_command_name, v_git_refs,
    v_validate_text, v_validate_question, v_check_mermaid, v_check_path,
    v_inspect_validate_result, v_constants,
]


def generate() -> dict[str, str]:
    """{file name: file text} for every vector file."""
    out: dict[str, str] = {}
    for builder in BUILDERS:
        made = builder()
        for vec in made if isinstance(made, list) else [made]:
            fname = f"{vec.name}.json"
            if fname in out:
                raise ValueError(f"duplicate vector file {fname}")
            out[fname] = dumps(vec.document())
    return out


def main(argv: list[str]) -> int:
    files = generate()
    if "--check" in argv:
        stale = [n for n, t in files.items()
                 if not (OUT_DIR / n).is_file() or (OUT_DIR / n).read_text(encoding="utf-8") != t]
        extra = sorted(p.name for p in OUT_DIR.glob("*.json") if p.name not in files)
        for n in stale + extra:
            print(f"out of date: tests/vectors/{n}")
        return 1 if stale or extra else 0
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for p in OUT_DIR.glob("*.json"):
        if p.name not in files:
            p.unlink()
    for name, text in files.items():
        (OUT_DIR / name).write_text(text, encoding="utf-8")
    total = sum(len(json.loads(t)["cases"]) for t in files.values())
    print(f"wrote {len(files)} files, {total} cases to {OUT_DIR.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
