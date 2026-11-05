//! Probes end to end (ADR 0003): discovery and shadowing, `check` through the Python host,
//! `run` of an enabled probe with a scripted ssh runner (the ledger row, the hash gate),
//! the built-in balance. Enabling itself is the human's (a unit test in `probe.rs` uses
//! the test-only token); here the Decision row is written as the migration tests do.
//! One test: it sets `ARBORIA_CONFIG_DIR`.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use arboria_core::backend::{RemoteOutput, Runner};
use arboria_core::db::{self, Ledger};
use arboria_core::probe::{self, ProbeResult, ProbeScope, ProbeState};
use arboria_core::{machine, Result};
use common::TempDir;

const ACCOUNTS: &str = "Account   Balance(Hours)\n--------  --------------\nbjki-dtai-gh   199\n";

#[derive(Debug)]
struct Accounts;

impl Runner for Accounts {
    fn run(&self, argv: &[String], _: Option<&str>, _: Duration) -> Result<RemoteOutput> {
        let stdout = if argv[..3] == ["ssh", "-O", "check"] {
            String::new()
        } else {
            assert_eq!(argv.last().unwrap(), "accounts", "{argv:?}");
            ACCOUNTS.to_string()
        };
        Ok(RemoteOutput {
            code: 0,
            stdout,
            stderr: String::new(),
        })
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
        unreachable!()
    }
}

const PARSER: &str = "# /// arb-probe
# api = 1
# machine = \"deltaai\"
# argv = [\"accounts\"]
# every = \"30m\"
# kind = \"number\"
# ///
def credits(text):
    \"\"\"DeltaAI 剩余 SU。\"\"\"
    for line in text.splitlines():
        parts = line.split()
        if parts and parts[0] == \"bjki-dtai-gh\":
            return {\"label\": \"剩余 SU\", \"value\": int(parts[1]), \"of\": 200}
    raise ValueError(\"no account line\")
";

fn python_ok() -> bool {
    std::process::Command::new("python3")
        .arg("-c")
        .arg("pass")
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn discover_check_run_balance() {
    let tmp = TempDir::new("probe");
    let root = tmp.path().canonicalize().unwrap();
    let cfg = root.join("config");
    std::env::set_var("ARBORIA_CONFIG_DIR", &cfg);
    std::fs::create_dir_all(cfg.join("probes")).unwrap();
    // a real profile, from the DeltaAI probe output
    let fixture =
        std::fs::read_to_string(common::repo_root().join("tests/fixtures/deltaai_probe.txt"))
            .unwrap();
    let p = machine::propose(
        "deltaai",
        "deltaai",
        &machine::split_sections(&fixture),
        None,
        None,
    )
    .unwrap();
    std::fs::write(
        cfg.join("clusters.toml"),
        machine::render_profile("deltaai", &p, ""),
    )
    .unwrap();

    let state = root.join("state");
    std::fs::create_dir_all(state.join("probes")).unwrap();
    let mut ledger = Ledger::open(&state).unwrap();
    db::init_backup_repo(&state).unwrap();
    std::fs::write(state.join("probes/credits.py"), PARSER).unwrap();
    std::fs::write(cfg.join("probes/credits.py"), "shadowed").unwrap();
    std::fs::write(cfg.join("probes/broken.py"), "# no header\n").unwrap();
    std::fs::write(cfg.join("probes/Bad-Name.py"), "x").unwrap();
    let files = probe::discover(Some(&state)).unwrap();
    let names: Vec<(String, ProbeScope)> = files
        .iter()
        .map(|f| (f.key.name.clone(), f.key.scope))
        .collect();
    assert_eq!(
        names,
        [
            ("broken".to_string(), ProbeScope::Global),
            ("credits".to_string(), ProbeScope::Project)
        ]
    );
    assert!(matches!(
        probe::state(ledger.conn(), &files[0]).unwrap(),
        ProbeState::Invalid(e) if e.contains("missing")
    ));
    let file = files[1].clone();
    assert_eq!(
        probe::state(ledger.conn(), &file).unwrap(),
        ProbeState::Disabled
    );

    // not enabled: run refuses before anything remote
    let err = probe::run_once_with(&mut ledger, &file, Arc::new(Accounts)).unwrap_err();
    assert!(err.to_string().contains("is not enabled"), "{err}");

    if python_ok() {
        let r = probe::check(&file, ACCOUNTS.as_bytes()).unwrap();
        assert_eq!(probe::result_text(&r), "剩余 SU: 199 / 200");
        let err = probe::check(&file, b"nothing here")
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "probe result: the parser failed: ValueError: no account line"
        );
    }

    // the human's enable Decision, bound to the file's hash
    let key = file.key.to_string();
    ledger
        .conn()
        .execute(
            "INSERT INTO decisions (id, subject_type, subject_id, question, options_json, choice, \
             decided_by, created_at, bound_hash) VALUES ('d001', 'probe', ?1, 'q', '[]', 'enable', \
             'human', '2026-11-05T00:00:00Z', ?2)",
            rusqlite::params![key, file.sha256],
        )
        .unwrap();
    assert_eq!(
        probe::state(ledger.conn(), &file).unwrap(),
        ProbeState::Enabled
    );
    if python_ok() {
        let r = probe::run_once_with(&mut ledger, &file, Arc::new(Accounts)).unwrap();
        assert!(matches!(&r, ProbeResult::Number(n) if n.value.as_i64() == Some(199)));
        let (machine, probe_key, hash): (String, String, String) = ledger
            .conn()
            .query_row(
                "SELECT machine, probe, probe_hash FROM probe_results",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (machine.as_str(), probe_key.as_str()),
            ("deltaai", "project:credits")
        );
        assert_eq!(hash, file.sha256);
        // heartbeat-like: no event
        let events: i64 = ledger
            .conn()
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 0);
    }
    // one changed byte: not run any more
    std::fs::write(state.join("probes/credits.py"), format!("{PARSER}\n")).unwrap();
    let changed = probe::discover(Some(&state)).unwrap().remove(1);
    assert_eq!(
        probe::state(ledger.conn(), &changed).unwrap(),
        ProbeState::NeedsReenable
    );
    let err = probe::run_once_with(&mut ledger, &changed, Arc::new(Accounts)).unwrap_err();
    assert!(
        err.to_string().contains("changed since it was enabled"),
        "{err}"
    );

    // the built-in balance needs no code
    let profile = arboria_core::clusters::load_profile("deltaai", None).unwrap();
    let r = probe::builtin_balance(&profile, &fixture_accounts(&fixture));
    assert_eq!(probe::result_text(&r), "余额: 199 小时");
    let r = probe::builtin_balance(&profile, "accounts: <b>down</b>\x07\n");
    assert_eq!(probe::result_text(&r), "余额:\naccounts: ‹b>down‹/b>");
    assert!(probe::guide().contains("arb probe check"));
}

fn fixture_accounts(probe: &str) -> String {
    machine::split_sections(probe)["accounts"].clone()
}
