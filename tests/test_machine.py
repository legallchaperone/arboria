"""arb machine init / arb doctor, against real DeltaAI probe output (fixtures/)."""

from __future__ import annotations

import subprocess
import tomllib
from datetime import datetime
from pathlib import Path

import pytest

from arboria import clusters, machine
from arboria.backends import NeedLogin
from arboria.backends import ssh as ssh_mod
from arboria.errors import ArbError

PROBE = (Path(__file__).parent / "fixtures" / "deltaai_probe.txt").read_text()
SSH_G = ("user wchen38\nhostname gh-login02.delta.ncsa.illinois.edu\ncontrolmaster auto\n"
         "controlpath /Users/x/.ssh/cm-abc\ncontrolpersist 604800\nbatchmode no\n")
REMOTE = ["ssh", "-o", "BatchMode=yes", "-o", "ControlMaster=no", "deltaai", "bash -s"]
NOW = lambda: datetime(2026, 11, 2, 12, 0, 0)  # noqa: E731


class ProbeRunner(ssh_mod.Runner):
    """Answers ssh -G, ssh -O check, the probe script and the path-check script."""

    def __init__(self, master: bool = True, probe: str = PROBE, ssh_g: str = SSH_G,
                 paths: dict[str, str] | None = None):
        self.master, self.probe, self.ssh_g = master, probe, ssh_g
        self.paths = paths if paths is not None else {}
        self.calls: list[tuple[list[str], str | None]] = []

    def run(self, argv, *, input=None, timeout=0):
        self.calls.append((list(argv), input))
        ok = lambda out: subprocess.CompletedProcess(argv, 0, out, "")  # noqa: E731
        if argv[:2] == ["ssh", "-G"]:
            return ok(self.ssh_g)
        if argv[:3] == ["ssh", "-O", "check"]:
            return subprocess.CompletedProcess(argv, 0 if self.master else 255, "", "no master")
        assert argv == REMOTE, argv
        if input == machine.PROBE_SCRIPT:
            return ok(self.probe)
        lines = []
        for role in clusters.PATH_ROLES:
            assert f"echo \"{role} $w $e $d\"" in input
            lines.append(self.paths.get(role, f"{role} 1 1 /real/{role}"))
        return ok("\n".join(lines) + "\n")

    def pipe(self, *a, **k):  # pragma: no cover
        raise AssertionError("machine init / doctor never pipe")


@pytest.fixture
def cfgdir(tmp_path: Path, monkeypatch) -> Path:
    monkeypatch.setenv("ARBORIA_CONFIG_DIR", str(tmp_path / "cfg"))
    return tmp_path / "cfg"


def sections() -> dict[str, str]:
    return machine.split_sections(PROBE)


# ------------------------------------------------------------------ parsing / proposal

def test_proposal_from_real_deltaai_probe() -> None:
    prop = machine.propose("deltaai", "deltaai", sections())
    v = prop.values
    assert (v["arch"], v["account"], v["gpus_per_node"]) == ("aarch64", "bjki-dtai-gh", 4)
    assert (v["cpus_per_gpu"], v["mem_per_gpu_gb"]) == (72, 110.898)
    assert v["default_partition"] == "ghx4" and v["billing_per_su"] == 1000
    assert v["partitions"] == {"ghx4": {"max_walltime": "48h", "charge": 1.0},
                               "ghx4-interactive": {"max_walltime": "2h", "charge": 2.0}}
    # `full` and `test` only allow another account
    assert prop.other_partitions == [] and prop.warnings == []
    assert v["paths"] == {
        "runs": "/projects/bjki/$USER/arboria/runs", "data": "/work/nvme/bjki/$USER/data",
        "cache": "/projects/bjki/$USER/arboria/cache",
        "ckpt": "/work/hdd/bjki/$USER/arboria/ckpt", "out": "/projects/bjki/$USER/arboria/out"}
    assert v["balance_cmd"] == "accounts" and "apptainer" in prop.tools
    # the rendered table is a valid profile
    text = machine.render_profile("deltaai", v, "note")
    prof = clusters.parse_profile("deltaai", tomllib.loads(text)["deltaai"])
    assert prof.partitions["ghx4-interactive"].charge == 2.0 and prof.billing_per_su == 1000


def test_proposal_keeps_user_choices() -> None:
    existing = {"account": "bjki-dtai-gh", "max_segments": 2, "prelude": ["module load cuda"],
                "env_activate": ["source /x/bin/activate"], "billing_per_su": 0,
                "paths": {r: f"/mine/{r}" for r in clusters.PATH_ROLES}}
    v = machine.propose("deltaai", "deltaai", sections(), existing=existing).values
    assert v["max_segments"] == 2 and v["prelude"] == ["module load cuda"]
    assert v["env_activate"] == ["source /x/bin/activate"] and v["billing_per_su"] == 0
    assert v["paths"]["runs"] == "/mine/runs"
    # placeholder paths are re-derived
    existing["paths"] = {**existing["paths"], "runs": "/projects/<proj>/$USER/runs"}
    v = machine.propose("deltaai", "deltaai", sections(), existing=existing).values
    assert v["paths"]["runs"] == "/projects/bjki/$USER/arboria/runs"


def test_accounts_choice() -> None:
    with pytest.raises(ArbError, match="not one of your accounts"):
        machine.propose("deltaai", "deltaai", sections(), account="nope")
    two = PROBE.replace("bjki-dtai-gh                               199",
                        "bjki-dtai-gh  199\nbbka-dtai-gh  50")
    prop = machine.propose("deltaai", "deltaai", machine.split_sections(two))
    assert prop.values["account"] == "<from `accounts`>"
    assert any("pass --account" in w for w in prop.warnings)
    # with the other account, `full` and `test` become usable (and are only listed)
    prop = machine.propose("deltaai", "deltaai", machine.split_sections(two),
                           account="bbka-dtai-gh")
    assert prop.other_partitions == ["full", "test"]


def test_weights_units_and_share_cross_checks() -> None:
    assert machine.parse_weights("CPU=13.888888,Mem=9.01726G,GRES/gpu=1000") == {
        "cpu": 13.888888, "mem": 9.01726, "gpu": 1000.0}
    assert machine.parse_weights("Mem=0.25")["mem"] == 256.0  # no suffix: per MB
    assert machine.parse_weights("Mem=2T")["mem"] == 2 / 1024
    # a cluster that bills memory more steeply than the node's share: the smaller wins
    steep = PROBE.replace("Mem=9.01726G,GRES/gpu=1000", "Mem=10G,GRES/gpu=1000")
    prop = machine.propose("x", "x", machine.split_sections(steep))
    assert prop.values["mem_per_gpu_gb"] == 100.0
    assert any("using the smaller" in w for w in prop.warnings)
    flat = PROBE.replace("PriorityFlags           = MAX_TRES", "PriorityFlags           =")
    assert any("MAX_TRES" in w for w in
               machine.propose("x", "x", machine.split_sections(flat)).warnings)


def test_replace_table_only_touches_that_machine() -> None:
    text = ('# top\n[other]\nkind = "fake"\n\n# old deltaai note\n[deltaai]\nhost = "a"\n'
            '[deltaai.partitions."ghx4-interactive"]\nmax_walltime = "2h"\n'
            '[deltaai.paths]\nruns = "/x"\n[other.paths]\nruns = "/o"\n')
    out = machine.replace_table(text, "deltaai", "# new\n[deltaai]\nhost = \"b\"\n")
    assert out == ('# top\n[other]\nkind = "fake"\n\n# new\n[deltaai]\nhost = "b"\n'
                   '[other.paths]\nruns = "/o"\n')
    assert machine.replace_table("[a]\nk = 1\n", "b", "[b]\nk = 2\n") == \
        "[a]\nk = 1\n\n[b]\nk = 2\n"


# ------------------------------------------------------------------ machine init

def test_machine_init_only_reads_and_writes_on_request(cfgdir: Path) -> None:
    runner = ProbeRunner()
    res = machine.machine_init("deltaai", runner=runner, now=NOW)
    assert not res.written and not (cfgdir / "clusters.toml").exists()
    assert res.still_invalid is None and res.ssh_problems == []
    assert [a for a, _ in runner.calls] == [["ssh", "-G", "deltaai"],
                                            ["ssh", "-O", "check", "deltaai"], REMOTE]
    assert runner.calls[-1][1] == machine.PROBE_SCRIPT

    res = machine.machine_init("deltaai", write=True, runner=ProbeRunner(), now=NOW)
    assert res.written and res.backup is None
    prof = clusters.load_profile("deltaai")
    assert (prof.account, prof.cpus_per_gpu, prof.mem_per_gpu_gb) == ("bjki-dtai-gh", 72, 110.898)

    with pytest.raises(ArbError, match="--replace"):
        machine.machine_init("deltaai", write=True, runner=ProbeRunner(), now=NOW)


def test_machine_init_replace_keeps_others_and_choices(cfgdir: Path) -> None:
    cfgdir.mkdir()
    (cfgdir / "clusters.toml").write_text(
        '[fake1]\nkind = "fake"\nroot = "/tmp/x"\n\n'
        '[deltaai]\nhost = "deltaai"\naccount = "bjki-dtai-gh"\ncharge_cmd = "jobcharge"\n'
        'env_activate = ["source /v/bin/activate"]\n[deltaai.paths]\nruns = "/mine/runs"\n')
    res = machine.machine_init("deltaai", write=True, replace=True, runner=ProbeRunner(),
                               now=NOW)
    assert res.backup is not None and res.backup.name == "clusters.toml.bak-20261102T120000"
    assert "charge_cmd" in res.backup.read_text()
    raw = clusters.load_all_raw()
    assert raw["fake1"] == {"kind": "fake", "root": "/tmp/x"}
    assert "charge_cmd" not in raw["deltaai"]
    assert raw["deltaai"]["env_activate"] == ["source /v/bin/activate"]
    # only runs was set, so the paths are re-derived as a whole
    assert raw["deltaai"]["paths"]["runs"] == "/projects/bjki/$USER/arboria/runs"
    assert clusters.load_profile("deltaai").env_activate == ("source /v/bin/activate",)


def test_machine_init_needs_master_and_never_retries(cfgdir: Path) -> None:
    runner = ProbeRunner(master=False)
    with pytest.raises(NeedLogin, match="ssh deltaai"):
        machine.machine_init("deltaai", runner=runner)
    assert [a[:3] for a, _ in runner.calls] == [["ssh", "-G", "deltaai"],
                                                ["ssh", "-O", "check"]]


def test_machine_init_reports_missing_control_master(cfgdir: Path) -> None:
    res = machine.machine_init("deltaai", runner=ProbeRunner(ssh_g="controlmaster no\n"),
                               now=NOW)
    assert any("ControlMaster" in p for p in res.ssh_problems)
    assert any("ControlPersist" in p for p in res.ssh_problems)


def test_machine_init_cli(cfgdir: Path, tmp_path: Path, monkeypatch, arb) -> None:
    monkeypatch.setattr(ssh_mod, "Runner", lambda: ProbeRunner())
    res = arb(tmp_path, "machine", "init", "deltaai")
    assert "cpus_per_gpu = 72" in res.stdout and "not written" in res.stdout
    assert "env_activate is empty" in res.stderr
    monkeypatch.setattr(ssh_mod, "Runner", lambda: ProbeRunner(master=False))
    res = arb(tmp_path, "machine", "init", "deltaai", ok=False)
    assert res.exit_code == 1 and "Duo" in res.stderr


# ------------------------------------------------------------------ doctor

def write_profile(cfgdir: Path, **over) -> None:
    v = machine.propose("deltaai", "deltaai", sections()).values
    v["env_activate"] = ["source /v/bin/activate"]
    for k, val in over.items():
        v[k] = val
    cfgdir.mkdir(exist_ok=True)
    (cfgdir / "clusters.toml").write_text(machine.render_profile("deltaai", v))


def levels(checks) -> dict[str, list[str]]:
    out: dict[str, list[str]] = {}
    for c in checks:
        out.setdefault(c.level, []).append(c.what)
    return out


def test_doctor_all_good(cfgdir: Path) -> None:
    write_profile(cfgdir)
    runner = ProbeRunner(paths={"data": "data 1 0 /work/nvme/bjki/wchen38"})
    got = levels(machine.doctor("deltaai", project_id="proj-1a2b", runner=runner))
    assert "fail" not in got and "warn" not in got, got
    assert any("balance 199" in w for w in got["ok"])
    assert any("paths.data" in w and "will be created under /work/nvme/bjki/wchen38" in w
               for w in got["ok"])
    assert any("quota /work/hdd/bjki 524.5G" in w for w in got["info"])
    # read-only: only ssh -G, the gate and the two fixed scripts
    remote = [i for a, i in runner.calls if a == REMOTE]
    assert remote[0] == machine.PROBE_SCRIPT and len(remote) == 2
    for word in ("sbatch ", "srun", "salloc", "scancel", "mkdir", "rm ", ">"):
        assert word not in remote[1]


def test_doctor_failures(cfgdir: Path) -> None:
    write_profile(cfgdir, account="bbka-dtai-gh", cpus_per_gpu=16, arch="x86_64",
                  partitions={"ghx4": {"max_walltime": "72h", "charge": 1.0},
                              "gpuA100x4": {"max_walltime": "1h", "charge": 1.0}})
    runner = ProbeRunner(paths={"ckpt": "ckpt 0 0 /work/hdd"})
    got = levels(machine.doctor("deltaai", runner=runner))
    fails = "\n".join(got["fail"])
    assert "login node is aarch64" in fails and "account bbka-dtai-gh is not yours" in fails
    assert "partition ghx4: max_walltime 72h > Slurm's 48h" in fails
    assert "partition gpuA100x4 does not exist or does not take account" in fails
    assert "paths.ckpt" in fails
    assert any("cpus_per_gpu = 16, measured 72" in w for w in got["warn"])


def test_doctor_stops_without_master(cfgdir: Path) -> None:
    write_profile(cfgdir)
    runner = ProbeRunner(master=False)
    got = levels(machine.doctor("deltaai", runner=runner))
    assert any("Duo" in f for f in got["fail"])
    assert not any(a == REMOTE for a, _ in runner.calls)


def test_doctor_invalid_profile_and_fake(cfgdir: Path, tmp_path: Path, monkeypatch,
                                         arb) -> None:
    cfgdir.mkdir()
    (cfgdir / "clusters.toml").write_text(
        '[deltaai]\nhost = "deltaai"\ncpus_per_gpu = 0\n[f]\nkind = "fake"\nroot = "/x"\n')
    got = levels(machine.doctor("deltaai", runner=ProbeRunner()))
    assert any("gpus_per_node must be" in f for f in got["fail"])
    assert any("ssh master" in o for o in got["ok"])  # ssh side still checked
    with pytest.raises(ArbError, match="arb machine init nope"):
        machine.doctor("nope", runner=ProbeRunner())
    # CLI: outside a repo, exit 1 on failures
    monkeypatch.setattr(ssh_mod, "Runner", lambda: ProbeRunner())
    res = arb(tmp_path, "doctor", "deltaai", ok=False)
    assert res.exit_code == 1 and "FAIL" in res.stdout and "problem(s)" in res.stdout
    res = arb(tmp_path, "doctor", ok=False)
    assert res.exit_code == 1 and "which machine" in res.stderr
