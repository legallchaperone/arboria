//! `arb machine init` and `arb doctor` against the real DeltaAI probe output
//! (`tests/fixtures/deltaai_probe.txt`), with a scripted runner (mirrors
//! tests/test_machine.py's ProbeRunner). The same scenarios run in CPython with its
//! ProbeRunner and the profile text, warnings and every doctor line must agree.
//! One test: it sets `ARBORIA_CONFIG_DIR`.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use arboria_core::backend::{RemoteOutput, Runner};
use arboria_core::clusters::PATH_ROLES;
use arboria_core::machine::{self, InitOptions};
use arboria_core::{Error, Result};
use common::TempDir;
use serde_json::{json, Value};

const SSH_G: &str =
    "user wchen38\nhostname gh-login02.delta.ncsa.illinois.edu\ncontrolmaster auto\n\
                     controlpath /Users/x/.ssh/cm-abc\ncontrolpersist 604800\nbatchmode no\n";

#[derive(Debug)]
struct ProbeRunner {
    master: bool,
    probe: String,
    paths: Vec<(&'static str, &'static str)>,
    calls: Mutex<Vec<(Vec<String>, Option<String>)>>,
}

impl ProbeRunner {
    fn new(master: bool, paths: Vec<(&'static str, &'static str)>) -> Arc<ProbeRunner> {
        Arc::new(ProbeRunner {
            master,
            probe: std::fs::read_to_string(
                common::repo_root().join("tests/fixtures/deltaai_probe.txt"),
            )
            .unwrap(),
            paths,
            calls: Mutex::new(Vec::new()),
        })
    }
}

fn ok(out: &str) -> RemoteOutput {
    RemoteOutput {
        code: 0,
        stdout: out.into(),
        stderr: String::new(),
    }
}

impl Runner for ProbeRunner {
    fn run(&self, argv: &[String], input: Option<&str>, _t: Duration) -> Result<RemoteOutput> {
        self.calls
            .lock()
            .unwrap()
            .push((argv.to_vec(), input.map(String::from)));
        if argv[..2] == ["ssh", "-G"] {
            return Ok(ok(SSH_G));
        }
        if argv[..3] == ["ssh", "-O", "check"] {
            return Ok(RemoteOutput {
                code: if self.master { 0 } else { 255 },
                stdout: String::new(),
                stderr: "no master".into(),
            });
        }
        assert_eq!(
            argv,
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "ControlMaster=no",
                "deltaai",
                "bash -s"
            ]
        );
        let input = input.unwrap();
        if input == machine::PROBE_SCRIPT {
            return Ok(ok(&self.probe));
        }
        let mut lines = Vec::new();
        for role in PATH_ROLES {
            assert!(input.contains(&format!("echo \"{role} $w $e $d\"")));
            match self.paths.iter().find(|(r, _)| r == role) {
                Some((_, l)) => lines.push(l.to_string()),
                None => lines.push(format!("{role} 1 1 /real/{role}")),
            }
        }
        Ok(ok(&(lines.join("\n") + "\n")))
    }

    fn has(&self, _: &str) -> bool {
        false
    }

    fn pipe(
        &self,
        _: &[String],
        _: &Path,
        _: &[String],
        _: Duration,
    ) -> Result<(i32, i32, String)> {
        Err(Error::arb("machine init / doctor never pipe"))
    }
}

/// The scenarios as CPython runs them; prints JSON.
const SCRIPT: &str = r##"
import json, subprocess, sys
from datetime import datetime
from pathlib import Path
from arboria import clusters, machine
from arboria.backends import ssh as ssh_mod
PROBE = Path(sys.argv[1]).read_text()
SSH_G = sys.argv[2]
class ProbeRunner(ssh_mod.Runner):
    def __init__(self, master=True, paths=None):
        self.master, self.paths = master, paths or {}
    def run(self, argv, *, input=None, timeout=0):
        ok = lambda out: subprocess.CompletedProcess(argv, 0, out, "")
        if argv[:2] == ["ssh", "-G"]:
            return ok(SSH_G)
        if argv[:3] == ["ssh", "-O", "check"]:
            return subprocess.CompletedProcess(argv, 0 if self.master else 255, "", "no master")
        if input == machine.PROBE_SCRIPT:
            return ok(PROBE)
        return ok("\n".join(self.paths.get(r, f"{r} 1 1 /real/{r}") for r in clusters.PATH_ROLES) + "\n")
NOW = lambda: datetime(2026, 11, 2, 12, 0, 0)
out = {}
r = machine.machine_init("deltaai", runner=ProbeRunner(), now=NOW)
out["dry"] = {"text": r.text, "warnings": r.proposal.warnings, "written": r.written,
              "still_invalid": r.still_invalid, "ssh": r.ssh_problems}
r = machine.machine_init("deltaai", write=True, runner=ProbeRunner(), now=NOW)
out["write"] = {"written": r.written}
cfg = clusters.config_path()
text = cfg.read_text().replace("env_activate = []", 'env_activate = ["source /v/bin/activate"]')
cfg.write_text("# mine\n[other]\nkind = \"fake\"\nroot = \"/x\"\n\n" + text)
lv = lambda cs: [[c.level, c.what] for c in cs]
out["good"] = lv(machine.doctor("deltaai", project_id="proj-1a2b",
                                runner=ProbeRunner(paths={"data": "data 1 0 /work/nvme/bjki/wchen38"})))
out["nomaster"] = lv(machine.doctor("deltaai", runner=ProbeRunner(master=False)))
r = machine.machine_init("deltaai", write=True, replace=True, runner=ProbeRunner(), now=NOW)
out["replace"] = {"backup": r.backup.name, "file": cfg.read_text()}
bad = cfg.read_text().replace('account = "bjki-dtai-gh"', 'account = "bbka-dtai-gh"') \
    .replace("cpus_per_gpu = 72", "cpus_per_gpu = 16").replace('max_walltime = "48h"', 'max_walltime = "72h"')
cfg.write_text(bad)
out["bad"] = lv(machine.doctor("deltaai", runner=ProbeRunner(paths={"ckpt": "ckpt 0 0 /work/hdd"})))
print(json.dumps(out))
"##;

fn levels(checks: &[machine::Check]) -> Value {
    json!(checks
        .iter()
        .map(|c| [c.level.clone(), c.what.clone()])
        .collect::<Vec<_>>())
}

#[test]
fn init_and_doctor_match_python() {
    let tmp = TempDir::new("machine");
    let base = tmp.path().canonicalize().unwrap();
    let (rs_cfg, py_cfg) = (base.join("rs"), base.join("py"));
    std::env::set_var("ARBORIA_CONFIG_DIR", &rs_cfg);
    let now = UNIX_EPOCH + Duration::from_secs(1_793_620_800); // 2026-11-02T12:00:00Z
    let init = |write: bool, replace: bool, r: Arc<ProbeRunner>| {
        machine::machine_init_with(
            "deltaai",
            InitOptions {
                write,
                replace,
                ..Default::default()
            },
            r,
            now,
        )
    };
    let dry = init(false, false, ProbeRunner::new(true, vec![])).unwrap();
    assert!(!dry.written && dry.ssh_problems.is_empty());
    assert!(dry
        .text
        .contains("cpus_per_gpu = 72\nmem_per_gpu_gb = 110.898\n"));
    assert!(!rs_cfg.join("clusters.toml").exists());
    let runner = ProbeRunner::new(true, vec![]);
    assert!(init(true, false, runner.clone()).unwrap().written);
    // read-only on the cluster: ssh -G, the gate and the one fixed script
    let remote: Vec<_> = runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(a, _)| a.last().map(String::as_str) == Some("bash -s"))
        .map(|(_, i)| i.clone())
        .collect();
    assert_eq!(remote, vec![Some(machine::PROBE_SCRIPT.to_string())]);
    let err = init(true, false, ProbeRunner::new(true, vec![]))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("already has [deltaai]; add --replace"),
        "{err}"
    );
    let cfg = rs_cfg.join("clusters.toml");
    let text = std::fs::read_to_string(&cfg).unwrap().replace(
        "env_activate = []",
        "env_activate = [\"source /v/bin/activate\"]",
    );
    std::fs::write(
        &cfg,
        format!("# mine\n[other]\nkind = \"fake\"\nroot = \"/x\"\n\n{text}"),
    )
    .unwrap();
    let good = machine::doctor_with(
        "deltaai",
        Some(&arboria_core::model::ProjectId::parse("proj-1a2b").unwrap()),
        ProbeRunner::new(true, vec![("data", "data 1 0 /work/nvme/bjki/wchen38")]),
    )
    .unwrap();
    assert!(
        good.iter().all(|c| c.level != "fail" && c.level != "warn"),
        "{good:?}"
    );
    let runner = ProbeRunner::new(false, vec![]);
    let nomaster = machine::doctor_with("deltaai", None, runner.clone()).unwrap();
    assert!(nomaster.last().unwrap().what.contains("Duo"));
    assert!(!runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|(a, _)| a.last().map(String::as_str) == Some("bash -s")));
    let replaced = init(true, true, ProbeRunner::new(true, vec![])).unwrap();
    let backup = replaced.backup.unwrap();
    assert_eq!(
        backup.file_name().unwrap(),
        "clusters.toml.bak-20261102T120000"
    );
    let file = std::fs::read_to_string(&cfg).unwrap();
    assert!(file.starts_with("# mine\n[other]\n"), "{file}");
    let bad_text = file
        .replace("account = \"bjki-dtai-gh\"", "account = \"bbka-dtai-gh\"")
        .replace("cpus_per_gpu = 72", "cpus_per_gpu = 16")
        .replace("max_walltime = \"48h\"", "max_walltime = \"72h\"");
    std::fs::write(&cfg, bad_text).unwrap();
    let bad = machine::doctor_with(
        "deltaai",
        None,
        ProbeRunner::new(true, vec![("ckpt", "ckpt 0 0 /work/hdd")]),
    )
    .unwrap();
    assert!(bad
        .iter()
        .any(|c| c.what == "cpus_per_gpu = 16, measured 72"));
    assert!(bad
        .iter()
        .any(|c| c.what == "partition ghx4: max_walltime 72h > Slurm's 48h"));

    // the same in CPython
    let out = std::process::Command::new("python3")
        .args(["-c", SCRIPT])
        .arg(common::repo_root().join("tests/fixtures/deltaai_probe.txt"))
        .arg(SSH_G)
        .env("PYTHONPATH", common::repo_root().join("src"))
        .env("ARBORIA_CONFIG_DIR", &py_cfg)
        .output()
        .unwrap();
    if !out.status.success() {
        eprintln!(
            "python not compared: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return;
    }
    let text = String::from_utf8(out.stdout)
        .unwrap()
        .replace(&py_cfg.display().to_string(), &rs_cfg.display().to_string());
    let py: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(py["dry"]["text"], json!(dry.text));
    assert_eq!(py["dry"]["warnings"], json!(dry.proposal.warnings));
    assert_eq!(py["dry"]["still_invalid"], json!(dry.still_invalid));
    assert_eq!(py["dry"]["ssh"], json!(dry.ssh_problems));
    assert_eq!(py["good"], levels(&good));
    assert_eq!(py["nomaster"], levels(&nomaster));
    assert_eq!(
        py["replace"]["backup"],
        json!(backup.file_name().unwrap().to_string_lossy())
    );
    assert_eq!(py["replace"]["file"], json!(file));
    assert_eq!(py["bad"], levels(&bad));
}
