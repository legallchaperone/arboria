"""SshSlurm (subprocess mocked: exact command lines) and FakeSlurm."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from arboria import clusters, slurm
from arboria.backends import BackendError, NeedLogin, get_backend, map_slurm_state
from arboria.backends.fake import FakeSlurm
from arboria.backends.ssh import Runner, SshSlurm, parse_charge
from arboria.errors import ArbError

from .clusterutil import deltaai, fake_toml
from .conftest import git

SSH = ["ssh", "-o", "BatchMode=yes", "-o", "ControlMaster=no", "deltaai"]
CHECK = ["ssh", "-O", "check", "deltaai"]
RUNS = "/projects/bdxx/$USER/arboria/runs"


class FakeRunner(Runner):
    """Records argv; answers from a list of (predicate, CompletedProcess)."""

    def __init__(self, master: bool = True):
        self.master = master
        self.calls: list[tuple[list[str], str | None]] = []
        self.replies: list[tuple[str, int, str, str]] = []  # (substring, rc, stdout, stderr)
        self.pipes: list[tuple[list[str], Path, list[str]]] = []

    def reply(self, needle: str, stdout: str = "", rc: int = 0, stderr: str = "") -> None:
        self.replies.append((needle, rc, stdout, stderr))

    def run(self, argv, *, input=None, timeout=0):
        self.calls.append((list(argv), input))
        if argv == ["ssh", "-O", "check", "deltaai"]:
            return subprocess.CompletedProcess(argv, 0 if self.master else 255, "",
                                               "" if self.master else "Control socket missing")
        cmd = " ".join(argv)
        for i, (needle, rc, out, err) in enumerate(self.replies):
            if needle in cmd:
                self.replies.pop(i)
                return subprocess.CompletedProcess(argv, rc, out, err)
        return subprocess.CompletedProcess(argv, 0, "", "")

    def pipe(self, producer, cwd, consumer, timeout=0):
        self.pipes.append((producer, cwd, consumer))
        return 0, 0, ""

    @property
    def argvs(self) -> list[list[str]]:
        return [a for a, _ in self.calls]


@pytest.fixture
def ssh() -> tuple[SshSlurm, FakeRunner]:
    runner = FakeRunner()
    return SshSlurm(deltaai(), runner), runner


def test_get_backend_kinds(tmp_path: Path) -> None:
    assert isinstance(get_backend(deltaai()), SshSlurm)
    import tomllib

    prof = clusters.parse_profile("fake", tomllib.loads(fake_toml(tmp_path))["fake"])
    assert isinstance(get_backend(prof), FakeSlurm)


def test_every_remote_call_is_gated_and_batchmode(ssh) -> None:
    backend, runner = ssh
    runner.reply("sbatch", "12345\n")
    runner.reply("sacct -j", "12345|RUNNING|30\n")
    backend.submit("#!/bin/bash\n", "arboria-r001-s1", None)
    backend.find_job("arboria-r001-s1")
    backend.poll(["12345"])
    backend.cancel(["12345"])
    backend.tail(f"{RUNS}/r001/logs/s1.12345.out", 20)
    backend.list_dir(f"{RUNS}/r001")
    backend.balance()
    remote = [a for a in runner.argvs if a != CHECK]
    assert remote and all(a[:6] == SSH and len(a) == 7 for a in remote)
    # each remote command is immediately preceded by the -O check gate
    for i, a in enumerate(runner.argvs):
        if a != CHECK:
            assert runner.argvs[i - 1] == CHECK


def test_exact_command_lines(ssh) -> None:
    backend, runner = ssh
    runner.reply("sbatch", "4242;deltaai\n")
    assert backend.submit("SCRIPT", "arboria-r003-s2", "4241") == "4242"
    argv, stdin = runner.calls[-1]
    assert argv == [*SSH,
                    f'mkdir -p "{RUNS}/r003/sbatch" "{RUNS}/r003/logs" && '
                    f'cat > "{RUNS}/r003/sbatch/s2.sh" && '
                    f'sbatch --parsable --dependency=afterany:4241 "{RUNS}/r003/sbatch/s2.sh"']
    assert stdin == "SCRIPT"

    runner.reply("sbatch", "77\n")
    backend.submit("S", "arboria-r003-s1", None)
    assert runner.argvs[-1][-1].endswith('sbatch --parsable "' + RUNS + '/r003/sbatch/s1.sh"')

    runner.reply("squeue", "")
    runner.reply("sacct --name", "555\n")
    assert backend.find_job("arboria-r003-s1") == "555"
    assert runner.argvs[-3] == [*SSH, "squeue --name=arboria-r003-s1 -h -o %i"]
    assert runner.argvs[-1] == [*SSH, "sacct --name=arboria-r003-s1 -n -X -P -o JobID "
                                      "--starttime=now-30days"]
    runner.reply("squeue", "556\n")
    assert backend.find_job("arboria-r003-s1") == "556"
    assert "sacct" not in runner.argvs[-1][-1]
    runner.reply("squeue", "")
    assert backend.find_job("arboria-r003-s9") is None
    runner.reply("squeue", "1\n2\n")
    with pytest.raises(BackendError, match="several jobs"):
        backend.find_job("arboria-r003-s1")

    runner.reply("sacct -j", "10|COMPLETED|3600\n11|CANCELLED by 123|5\n12|PENDING|0\n9|X|1\n")
    runner.reply("jobcharge 10", "Job 10 charge: 4.00 SU\n")
    runner.reply("jobcharge 11", "no idea\n")
    assert backend.poll(["10", "11", "12"]) == {
        "10": ("completed", 3600, 4.0), "11": ("cancelled", 5, None), "12": ("pending", 0, None)}
    assert [*SSH, "sacct -j 10,11,12 -n -X -P -o JobID,State,ElapsedRaw"] in runner.argvs
    assert [*SSH, "jobcharge 10"] in runner.argvs
    assert not any("jobcharge 12" in a[-1] for a in runner.argvs)  # only terminal jobs

    backend.cancel(["10", "11"])
    assert runner.argvs[-1] == [*SSH, "scancel 10 11"]
    backend.tail(f"{RUNS}/r003/logs/s1.10.out", 50)
    assert runner.argvs[-1] == [*SSH, f'tail -n 50 "{RUNS}/r003/logs/s1.10.out"']
    runner.reply("ls -1", "code\nlogs\n")
    assert backend.list_dir(f"{RUNS}/r003") == ["code", "logs"]
    assert runner.argvs[-1] == [*SSH, f'ls -1 "{RUNS}/r003"']
    runner.reply("accounts", "bdxx-dtai-gh  1000 SU left\n")
    assert "1000 SU" in backend.balance()
    assert runner.argvs[-1] == [*SSH, "accounts"]


def test_ship_pipes_git_archive_into_remote_tar(ssh, tmp_path: Path) -> None:
    backend, runner = ssh
    backend.ship(tmp_path, "a" * 40, f"{RUNS}/r001/code")
    assert runner.argvs[-1] == CHECK
    producer, cwd, consumer = runner.pipes[-1]
    assert producer == ["git", "archive", "--format=tar", "a" * 40] and cwd == tmp_path
    assert consumer == [*SSH, f'mkdir -p "{RUNS}/r001/code" && tar -x -C "{RUNS}/r001/code"']
    with pytest.raises(BackendError, match="invalid commit"):
        backend.ship(tmp_path, "HEAD; rm -rf /", f"{RUNS}/r001/code")


def test_need_login_never_retries(tmp_path: Path) -> None:
    runner = FakeRunner(master=False)
    backend = SshSlurm(deltaai(), runner)
    calls = [lambda: backend.submit("S", "arboria-r001-s1", None),
             lambda: backend.find_job("arboria-r001-s1"), lambda: backend.poll(["1"]),
             lambda: backend.cancel(["1"]), lambda: backend.tail(f"{RUNS}/r001/x", 5),
             lambda: backend.list_dir(f"{RUNS}/r001"), lambda: backend.balance(),
             lambda: backend.fetch("/projects/bdxx/$USER/arboria/out/r001", tmp_path / "o"),
             lambda: backend.ship(tmp_path, "b" * 40, f"{RUNS}/r001/code")]
    for call in calls:
        before = len(runner.calls)
        with pytest.raises(NeedLogin, match=r"请先在终端运行 `ssh deltaai` 完成 Duo 认证"):
            call()
        assert [a for a, _ in runner.calls[before:]] == [CHECK]  # one check, nothing else
    assert runner.pipes == []


def test_remote_paths_are_restricted(ssh) -> None:
    backend, runner = ssh
    for bad in ("/etc/passwd", f"{RUNS}/../../secrets", f"{RUNS}/r001/$(id)",
                "/projects/bdxx/$USER/arboria/runsX/r001", f'{RUNS}/r001/"x"', "~/.ssh/id_rsa"):
        with pytest.raises(ArbError, match="refused"):
            backend.tail(bad, 10)
        with pytest.raises(ArbError, match="refused"):
            backend.list_dir(bad)
    assert runner.calls == []  # refused before any ssh
    backend.list_dir("/projects/bdxx/$USER/arboria/out/r001/plots")


def test_invalid_ids_and_names_refused(ssh) -> None:
    backend, runner = ssh
    with pytest.raises(BackendError, match="invalid Slurm job id"):
        backend.cancel(["1; rm -rf ~"])
    with pytest.raises(ArbError, match="not an arboria job name"):
        backend.find_job("x; ls")
    with pytest.raises(BackendError, match="invalid Slurm job id"):
        backend.submit("S", "arboria-r001-s2", "1 && reboot")
    runner.reply("sbatch", "Submitted batch job 5\n")
    with pytest.raises(BackendError, match="no job id"):
        backend.submit("S", "arboria-r001-s1", None)
    runner.reply("sbatch", "", rc=1, stderr="sbatch: error: Invalid account")
    with pytest.raises(BackendError, match="Invalid account"):
        backend.submit("S", "arboria-r001-s1", None)


def test_fetch_rsync_and_scp_fallback(ssh, tmp_path: Path, monkeypatch) -> None:
    backend, runner = ssh
    out = "/projects/bdxx/$USER/arboria/out/r001"
    monkeypatch.setattr("arboria.backends.ssh.shutil.which", lambda name: "/usr/bin/rsync")
    runner.reply("pwd -P", "/projects/bdxx/alice/arboria/out/r001\n")
    backend.fetch(out, tmp_path / "local")
    assert runner.argvs[-2] == [*SSH, f'if [ -d "{out}" ]; then cd "{out}" && pwd -P; fi']
    assert runner.argvs[-1] == ["rsync", "-a", "-e", "ssh -o BatchMode=yes -o ControlMaster=no",
                                "deltaai:/projects/bdxx/alice/arboria/out/r001/",
                                f"{tmp_path / 'local'}/"]
    # nothing written remotely -> nothing fetched, no rsync
    n = len(runner.calls)
    backend.fetch(out, tmp_path / "local")
    assert [a[0] for a in runner.argvs[n:]] == ["ssh", "ssh"]

    monkeypatch.setattr("arboria.backends.ssh.shutil.which", lambda name: None)
    runner.reply("pwd -P", "/projects/bdxx/alice/arboria/out/r001\n")
    backend.fetch(out, tmp_path / "local")
    argv = runner.argvs[-1]
    assert argv[:7] == ["scp", "-r", "-q", "-o", "BatchMode=yes", "-o", "ControlMaster=no"]
    assert argv[7] == "deltaai:/projects/bdxx/alice/arboria/out/r001"
    with pytest.raises(ArbError, match="refused"):
        backend.fetch("/home/alice", tmp_path / "x")


def test_default_runner_uses_subprocess(monkeypatch) -> None:
    seen = {}

    def fake_run(argv, **kw):
        seen.update(argv=argv, **kw)
        return subprocess.CompletedProcess(argv, 0, "ok", "")

    monkeypatch.setattr("arboria.backends.ssh.subprocess.run", fake_run)
    Runner().run(["ssh", "-O", "check", "deltaai"])
    assert seen["argv"] == ["ssh", "-O", "check", "deltaai"]
    assert seen["stdin"] == subprocess.DEVNULL and seen["capture_output"] and seen["timeout"]


def test_parse_charge_and_states() -> None:
    assert parse_charge("Charge: 12.5") == 12.5
    assert parse_charge("Job 1 used 3 SU") == 3.0
    assert parse_charge("SU charged: 3 (3 SU)") == 3.0
    assert parse_charge("2 SU and 3 SU") is None and parse_charge("") is None
    assert map_slurm_state("CANCELLED by 1234") == "cancelled"
    assert map_slurm_state("COMPLETED") == "completed" and map_slurm_state("TIMEOUT") == "timeout"
    assert map_slurm_state("OUT_OF_MEMORY") == "failed" and map_slurm_state("WHAT") == "unknown"


# ---------------------------------------------------------------- FakeSlurm


@pytest.fixture
def fake(tmp_path: Path) -> FakeSlurm:
    import tomllib

    base = tmp_path / "cl"
    return FakeSlurm(clusters.parse_profile("fake", tomllib.loads(fake_toml(base))["fake"]))


def _script(fake: FakeSlurm, run: str, k: int) -> str:
    paths = fake.profile.paths
    return (f"#SBATCH --output={slurm.sbatch_value(slurm.log_pattern(paths, run, k))}\n"
            f'export ARBORIA_OUT="{slurm.out_dir(paths, run)}"\n')


def test_fake_dependencies_and_outcomes(fake: FakeSlurm, tmp_path: Path) -> None:
    fake.script("arboria-r001-s1", state="timeout", run_s=100, log="step 1 loss 2.3\n")
    fake.script("arboria-r001-s2", state="completed", run_s=50, billed=7.5,
                out_files={"metrics.json": "{}"})
    j1 = fake.submit(_script(fake, "r001", 1), "arboria-r001-s1", None)
    j2 = fake.submit(_script(fake, "r001", 2), "arboria-r001-s2", j1)
    assert (j1, j2) == ("1001", "1002")
    assert fake.poll([j1, j2]) == {j1: ("running", 0, None), j2: ("pending", 0, None)}
    log = f"{fake.profile.paths['runs']}/r001/logs/s1.{j1}.out"
    assert fake.tail(log, 5) == "step 1 loss 2.3\n"
    fake.advance(60)
    assert fake.poll([j1, j2])[j1] == ("running", 60, None)
    fake.advance(60)  # s1 times out at 100, s2 starts then
    assert fake.poll([j1, j2]) == {j1: ("timeout", 100, None), j2: ("running", 20, None)}
    fake.advance(100)
    assert fake.poll([j2]) == {j2: ("completed", 50, 7.5)}
    assert fake.find_job("arboria-r001-s2") == j2 and fake.find_job("arboria-r009-s1") is None
    out = f"{fake.profile.paths['out']}/r001"
    assert fake.list_dir(out) == ["metrics.json"]
    fake.fetch(out, tmp_path / "local")
    assert (tmp_path / "local" / "metrics.json").read_text() == "{}"
    with pytest.raises(ArbError, match="refused"):
        fake.tail("/etc/passwd", 3)
    assert [c[0] for c in fake.calls()][:2] == ["submit", "submit"]


def test_fake_cancel_and_need_login(fake: FakeSlurm) -> None:
    j1 = fake.submit(_script(fake, "r002", 1), "arboria-r002-s1", None)
    fake.advance(10)
    fake.cancel([j1])
    assert fake.poll([j1]) == {j1: ("cancelled", 10, None)}
    fake.set_need_login()
    with pytest.raises(NeedLogin):
        fake.poll([j1])
    fake.set_need_login(False)
    fake.script("arboria-r002-s2", submit_error=True)
    with pytest.raises(BackendError, match="scripted"):
        fake.submit("", "arboria-r002-s2", None)
    assert fake.submit("", "arboria-r002-s2", None) == "1002"  # only once


def test_fake_ship_extracts_commit(fake: FakeSlurm, repo: Path) -> None:
    (repo / "extra.py").write_text("untracked, not in commit\n")
    head = git(repo, "rev-parse", "HEAD")
    dest = f"{fake.profile.paths['runs']}/r001/code"
    fake.ship(repo, head, dest)
    assert sorted(p.name for p in Path(dest).iterdir()) == [".gitignore", "README.md", "train.py"]
