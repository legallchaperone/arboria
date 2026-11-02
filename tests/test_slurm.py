"""Pure functions: walltime, billing, segments, sbatch rendering, card hash, profiles."""

from __future__ import annotations

from pathlib import Path

import pytest

from arboria import clusters, slurm
from arboria.errors import ArbError
from arboria.slurm import Ask, build_plan

from .clusterutil import DELTAAI, deltaai


def ask(**over) -> Ask:
    base = dict(command=["python", "train.py", "--lr", "3e-4"], gpus=4, nodes=1,
                walltime_s=12 * 3600, partition="ghx4", machine="deltaai")
    return Ask(**{**base, **over})


@pytest.mark.parametrize("text,seconds", [
    ("90m", 5400), ("12h", 43200), ("1d6h", 108000), ("45s", 45), ("1h30m", 5400),
    ("02:30:00", 9000), ("48:00:00", 172800), ("1-06:00:00", 108000), (" 2H ", 7200),
])
def test_parse_walltime(text: str, seconds: int) -> None:
    assert slurm.parse_walltime(text) == seconds


@pytest.mark.parametrize("text", ["", "abc", "10", "1x", "0h", "1:61:00", "h", "-1h", None])
def test_parse_walltime_rejects(text) -> None:
    with pytest.raises(ArbError):
        slurm.parse_walltime(text)


def test_time_formats() -> None:
    assert slurm.slurm_time(172800) == "48:00:00"
    assert slurm.slurm_time(5430) == "01:30:30"
    assert slurm.human_duration(108000) == "1d6h"
    assert slurm.human_duration(5400) == "1h30m"


def test_segment_plan() -> None:
    h = 3600
    assert slurm.segment_plan(12 * h, 48 * h, 4) == [12 * h]
    assert slurm.segment_plan(48 * h, 48 * h, 4) == [48 * h]
    assert slurm.segment_plan(100 * h, 48 * h, 4) == [48 * h, 48 * h, 4 * h]
    assert slurm.segment_plan(5 * h, 2 * h, 4) == [2 * h, 2 * h, 1 * h]
    with pytest.raises(ArbError, match="needs 5 segments.*max_segments = 4"):
        slurm.segment_plan(9 * h, 2 * h, 4)


def test_billing_rule() -> None:
    # max(gpus, ceil(mem / mem_per_gpu), ceil(cpus / cpus_per_gpu))
    assert slurm.billed_gpus(1, 100, 16, 100, 16) == 1
    assert slurm.billed_gpus(1, 200, 16, 100, 16) == 2
    assert slurm.billed_gpus(1, 101, 16, 100, 16) == 2
    assert slurm.billed_gpus(1, 50, 40, 100, 16) == 3
    assert slurm.billed_gpus(4, 10, 1, 100, 16) == 4


@pytest.mark.parametrize("gpus,nodes", [(1, 1), (2, 1), (4, 1), (8, 2), (16, 4)])
def test_renderer_requests_billed_equal_requested(gpus: int, nodes: int) -> None:
    prof = deltaai()
    plan = build_plan(prof, ask(gpus=gpus, nodes=nodes))
    per_node = gpus // nodes
    assert plan.mem_gb_per_node == per_node * 100 and plan.cpus_per_node == per_node * 16
    assert plan.billed_gpus == gpus and not plan.over_billed
    script = slurm.render_sbatch(plan, "r001", "exp1", 1)
    assert f"#SBATCH --mem={per_node * 100}G" in script
    assert f"#SBATCH --cpus-per-task={per_node * 16}" in script
    assert f"#SBATCH --gpus-per-node={per_node}" in script
    # the billing rule applied to what the script requests gives the requested GPU count
    assert slurm.billed_gpus(per_node, per_node * 100, per_node * 16, 100, 16) * nodes == gpus


def test_fractional_mem_share_never_overbills() -> None:
    plan = build_plan(deltaai(mem_per_gpu_gb=117.5), ask(gpus=3))
    assert plan.mem_gb_per_node == 352 and plan.billed_gpus == 3


def test_manual_over_request_is_billed_higher() -> None:
    plan = build_plan(deltaai(), ask(gpus=1, mem_gb=200))
    assert plan.over_billed and plan.billed_gpus == 2
    plan = build_plan(deltaai(), ask(gpus=1, cpus=48))
    assert plan.billed_gpus == 3
    with pytest.raises(ArbError, match="more than a whole"):
        build_plan(deltaai(), ask(gpus=1, mem_gb=500))


def test_plan_refusals() -> None:
    with pytest.raises(ArbError, match="no account"):
        build_plan(deltaai(account=""), ask())
    with pytest.raises(ArbError, match="no account"):
        build_plan(deltaai(account="<from-accounts>"), ask())
    with pytest.raises(ArbError, match="no command"):
        build_plan(deltaai(), ask(command=[]))
    with pytest.raises(ArbError, match="GPUs per node > 4"):
        build_plan(deltaai(), ask(gpus=8))
    with pytest.raises(ArbError, match="not divisible"):
        build_plan(deltaai(), ask(gpus=6, nodes=4))
    with pytest.raises(ArbError, match="no partition 'gpuA100'"):
        build_plan(deltaai(), ask(partition="gpuA100"))
    with pytest.raises(ArbError, match="max_segments"):
        build_plan(deltaai(), ask(walltime_s=9 * 3600, partition="ghx4-interactive"))


def test_segments_and_estimate() -> None:
    plan = build_plan(deltaai(), ask(gpus=4, walltime_s=100 * 3600))
    assert plan.segments == [48 * 3600, 48 * 3600, 4 * 3600]
    assert plan.est_su == 400.0
    plan = build_plan(deltaai(), ask(gpus=2, walltime_s=3 * 3600, partition="ghx4-interactive"))
    assert plan.segments == [7200, 3600] and plan.est_su == 2 * 3 * 2.0
    s2 = slurm.render_sbatch(plan, "r002", "exp1", 2)
    assert "#SBATCH --time=01:00:00" in s2 and "#SBATCH --partition=ghx4-interactive" in s2
    assert "ARBORIA_SEGMENT=2" in s2 and "--dependency" not in s2
    with pytest.raises(ArbError):
        slurm.render_sbatch(plan, "r002", "exp1", 3)


GOLDEN = """\
#!/bin/bash
# arboria run r007, segment 1/2, idea lr-warmup. Generated by arb; do not edit.
#SBATCH --job-name=arboria-r007-s1
#SBATCH --account=bdxx-dtai-gh
#SBATCH --partition=ghx4
#SBATCH --nodes=1
#SBATCH --ntasks-per-node=1
#SBATCH --gpus-per-node=2
#SBATCH --cpus-per-task=32
#SBATCH --mem=200G
#SBATCH --time=48:00:00
#SBATCH --output=/projects/bdxx/%u/arboria/runs/r007/logs/s1.%j.out
#SBATCH --error=/projects/bdxx/%u/arboria/runs/r007/logs/s1.%j.out
set -euo pipefail
set +u
module load cuda
source /projects/bdxx/$USER/env/bin/activate
set -u
export ARBORIA_RUN_ID=r007
export ARBORIA_IDEA=lr-warmup
export ARBORIA_SEGMENT=1
export ARBORIA_DATA="/work/nvme/bdxx/$USER/data"
export ARBORIA_CACHE="/projects/bdxx/$USER/cache"
export HF_HOME="$ARBORIA_CACHE/hf"
export ARBORIA_CKPT_DIR="/work/hdd/bdxx/$USER/ckpt/r007"
export ARBORIA_OUT="/projects/bdxx/$USER/arboria/out/r007"
mkdir -p "$ARBORIA_CACHE" "$HF_HOME" "$ARBORIA_CKPT_DIR" "$ARBORIA_OUT"
cd "/projects/bdxx/$USER/arboria/runs/r007/code"
python train.py --name 'a b' '$(rm -rf ~)' '--x=\\"q\\"' ';' 'it'"'"'s'
"""


def test_render_sbatch_golden() -> None:
    cmd = ["python", "train.py", "--name", "a b", "$(rm -rf ~)", '--x=\\"q\\"', ";", "it's"]
    plan = build_plan(deltaai(), ask(command=cmd, gpus=2, walltime_s=60 * 3600))
    assert slurm.render_sbatch(plan, "r007", "lr-warmup", 1) == GOLDEN


def test_render_without_setup_lines() -> None:
    plan = build_plan(deltaai(prelude=[], env_activate=[]), ask())
    script = slurm.render_sbatch(plan, "r001", "exp1", 1)
    assert "set +u" not in script and "set -euo pipefail\nexport ARBORIA_RUN_ID=r001" in script


def test_card_hash_binds_everything() -> None:
    prof = deltaai()
    base = slurm.card_hash("c1", build_plan(prof, ask()))
    assert base == slurm.card_hash("c1", build_plan(deltaai(), ask()))  # deterministic
    variants = [
        slurm.card_hash("c2", build_plan(prof, ask())),
        slurm.card_hash("c1", build_plan(prof, ask(command=["python", "train.py"]))),
        slurm.card_hash("c1", build_plan(prof, ask(gpus=2))),
        slurm.card_hash("c1", build_plan(prof, ask(walltime_s=60 * 3600))),
        slurm.card_hash("c1", build_plan(prof, ask(walltime_s=1800, partition="ghx4-interactive"))),
        slurm.card_hash("c1", build_plan(deltaai(env_activate=["pixi shell"]), ask())),
        slurm.card_hash("c1", build_plan(deltaai(account="other"), ask())),
        slurm.card_hash("c1", build_plan(deltaai(mem_per_gpu_gb=90), ask())),
        slurm.card_hash("c1", build_plan(prof, ask(machine="other"))),
    ]
    assert base not in variants and len(set(variants)) == len(variants)


def test_job_names() -> None:
    assert slurm.job_name("r012", 3) == "arboria-r012-s3"
    assert slurm.parse_job_name("arboria-r012-s3") == ("r012", 3)
    for bad in ("arboria-r012-s0", "arboria-r012", "x-r1-s1", "arboria-r1-s1;ls"):
        with pytest.raises(ArbError):
            slurm.parse_job_name(bad)


# ---------------------------------------------------------------- profiles


def test_profile_parses_example_values() -> None:
    p = deltaai()
    assert p.partitions["ghx4"].max_walltime_s == 48 * 3600
    assert p.partitions["ghx4-interactive"].charge == 2.0
    assert p.partition(None).name == "ghx4" and p.account_set


@pytest.mark.parametrize("over,msg", [
    ({"cpus_per_gpu": 0}, "cpus_per_gpu must be an integer >= 1 \\(TO CONFIRM"),
    ({"mem_per_gpu_gb": 0}, "mem_per_gpu_gb must be a number > 0 \\(TO CONFIRM"),
    ({"kind": "pbs"}, "kind must be one of"),
    ({"host": "delta ai"}, "host must be"),
    ({"default_partition": "nope"}, "default_partition"),
    ({"bogus": 1}, "unknown key"),
    ({"prelude": "module load x"}, "list of strings"),
    ({"prelude": ["a\nb"]}, "one shell line"),
    ({"partitions": {"ghx4": {"max_walltime": "forever"}}}, "max_walltime"),
    ({"paths": {**DELTAAI["paths"], "runs": "/projects/<proj>/$USER/runs"}}, "placeholder"),
    ({"paths": {**DELTAAI["paths"], "out": "relative/out"}}, "absolute"),
    ({"paths": {**DELTAAI["paths"], "out": "/p/$(whoami)/out"}}, "only letters"),
    ({"paths": {**DELTAAI["paths"], "out": "/p/$HOME/out"}}, "only letters"),
    ({"paths": {**DELTAAI["paths"], "out": "/p/../out"}}, "only letters"),
    ({"paths": {k: v for k, v in DELTAAI["paths"].items() if k != "ckpt"}}, "ckpt is required"),
])
def test_profile_validation(over: dict, msg: str) -> None:
    with pytest.raises(ArbError, match=msg):
        deltaai(**over)


def test_example_file_matches_loader(tmp_path: Path, monkeypatch) -> None:
    example = Path(__file__).parent.parent / "examples" / "clusters.toml"
    monkeypatch.setenv("ARBORIA_CONFIG_DIR", str(tmp_path))
    (tmp_path / "clusters.toml").write_text(example.read_text())
    # shipped with TO CONFIRM zeros and placeholders: refused with a clear message
    with pytest.raises(ArbError, match="TO CONFIRM"):
        clusters.load_profile("deltaai")
    filled = (example.read_text().replace("cpus_per_gpu = 0", "cpus_per_gpu = 16")
              .replace("mem_per_gpu_gb = 0", "mem_per_gpu_gb = 100").replace("<proj>", "bdxx")
              .replace('"<from `accounts`>"', '"bdxx-dtai-gh"'))
    (tmp_path / "clusters.toml").write_text(filled)
    p = clusters.load_profile("deltaai")
    assert p.host == "deltaai" and p.account == "bdxx-dtai-gh" and p.max_segments == 4
    with pytest.raises(ArbError, match="no machine 'polaris'.*known: deltaai"):
        clusters.load_profile("polaris")


def test_missing_and_invalid_config(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("ARBORIA_CONFIG_DIR", str(tmp_path))
    assert clusters.config_path() == tmp_path / "clusters.toml"
    with pytest.raises(ArbError, match="does not exist"):
        clusters.load_profile("deltaai")
    (tmp_path / "clusters.toml").write_text("[deltaai\n")
    with pytest.raises(ArbError, match="not valid TOML"):
        clusters.load_profile("deltaai")
    monkeypatch.delenv("ARBORIA_CONFIG_DIR")
    assert clusters.config_path() == Path.home() / ".config" / "arboria" / "clusters.toml"
