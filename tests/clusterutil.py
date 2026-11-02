"""Cluster profile helpers shared by tests."""

from __future__ import annotations

from pathlib import Path

from arboria.clusters import parse_profile

DELTAAI = {
    "kind": "slurm",
    "host": "deltaai",
    "arch": "aarch64",
    "account": "bdxx-dtai-gh",
    "gpus_per_node": 4,
    "cpus_per_gpu": 16,
    "mem_per_gpu_gb": 100,
    "default_partition": "ghx4",
    "max_segments": 4,
    "balance_cmd": "accounts",
    "charge_cmd": "jobcharge",
    "prelude": ["module load cuda"],
    "env_activate": ["source /projects/bdxx/$USER/env/bin/activate"],
    "partitions": {
        "ghx4": {"max_walltime": "48h", "charge": 1.0},
        "ghx4-interactive": {"max_walltime": "2h", "charge": 2.0},
    },
    "paths": {
        "runs": "/projects/bdxx/$USER/arboria/runs",
        "data": "/work/nvme/bdxx/$USER/data",
        "cache": "/projects/bdxx/$USER/cache",
        "ckpt": "/work/hdd/bdxx/$USER/ckpt",
        "out": "/projects/bdxx/$USER/arboria/out",
    },
}


def deltaai(**over):
    raw = {**DELTAAI, **over}
    return parse_profile("deltaai", raw)


def fake_toml(root: Path, name: str = "fake", **over) -> str:
    """A clusters.toml with one fake machine whose 'remote' lives under `root`."""
    vals = {"account": "acct1", "cpus_per_gpu": 16, "mem_per_gpu_gb": 100, "max_segments": 4}
    vals.update(over)
    r = root.as_posix()
    return f"""
[{name}]
kind = "fake"
root = "{r}/state"
arch = "aarch64"
account = "{vals['account']}"
gpus_per_node = 4
cpus_per_gpu = {vals['cpus_per_gpu']}
mem_per_gpu_gb = {vals['mem_per_gpu_gb']}
default_partition = "ghx4"
max_segments = {vals['max_segments']}
prelude = ["module load cuda"]
env_activate = []
[{name}.partitions.ghx4]
max_walltime = "48h"
charge = 1.0
[{name}.partitions."ghx4-interactive"]
max_walltime = "2h"
charge = 2.0
[{name}.paths]
runs = "{r}/remote/runs"
data = "{r}/remote/data"
cache = "{r}/remote/cache"
ckpt = "{r}/remote/ckpt"
out = "{r}/remote/out"
"""
