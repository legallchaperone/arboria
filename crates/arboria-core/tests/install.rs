//! `arb install-claude`'s settings merge (mirrors tests/test_install.py), and the files it
//! writes compared with CPython's `install.install` given the same rules.

mod common;

use std::process::Command;

use arboria_core::install::{self, LOCAL_NAME, SETTINGS_NAME};
use common::TempDir;
use serde_json::{json, Value};

fn read(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn merge_is_additive_and_idempotent() {
    let tmp = TempDir::new("install");
    let root = tmp.path();
    let claude = root.join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    let old_dir = "/Users/x/.config/arboria/commands";
    std::fs::write(
        claude.join(SETTINGS_NAME),
        json!({
            "model": "opus",
            "permissions": {"ask": ["Bash(arb approve:*)", "Bash(ls:*)"],
                            "deny": ["Bash(sbatch:*)"],
                            "additionalDirectories": [old_dir, "/data"]},
        })
        .to_string(),
    )
    .unwrap();
    let dirs = vec!["/cfg/arboria/commands".to_string()];
    let rules = install::deny_rules();
    let (path, changes) = install::install(root, Some(&dirs), true, &rules).unwrap();
    assert_eq!(path, claude.join(SETTINGS_NAME));
    assert!(changes.contains(&"hooks.SessionStart += arb hook session-start".to_string()));
    assert!(!changes.contains(&"permissions.deny += Bash(sbatch:*)".to_string()));
    assert!(changes.contains(&"permissions.deny += Bash(arb probe enable:*)".to_string()));
    assert!(changes.contains(&"permissions.ask -= Bash(arb approve:*)".to_string()));
    assert!(changes.contains(&format!(
        "permissions.additionalDirectories -= {old_dir} (machine-specific; now in {LOCAL_NAME})"
    )));
    assert!(changes.contains(&format!(
        "{LOCAL_NAME}: permissions.additionalDirectories += /cfg/arboria/commands"
    )));
    let s = read(&path);
    assert_eq!(s["model"], "opus");
    assert_eq!(s["permissions"]["ask"], json!(["Bash(ls:*)"]));
    assert_eq!(s["permissions"]["additionalDirectories"], json!(["/data"]));
    assert_eq!(
        s["permissions"]["deny"].as_array().unwrap().len(),
        rules.len()
    );
    let local = read(&claude.join(LOCAL_NAME));
    assert_eq!(
        local,
        json!({"permissions": {"additionalDirectories": ["/cfg/arboria/commands"]}})
    );
    let (_, again) = install::install(root, Some(&dirs), true, &rules).unwrap();
    assert!(again.is_empty(), "{again:?}");

    // a malformed section names the file and is left alone
    std::fs::write(&path, "{\"hooks\": []}").unwrap();
    let err = install::install(root, Some(&dirs), true, &rules)
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        format!("{}: `hooks` must be a dict, found list", path.display())
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"hooks\": []}");
    std::fs::write(&path, "[1]").unwrap();
    assert_eq!(
        install::install(root, Some(&dirs), true, &rules)
            .unwrap_err()
            .to_string(),
        format!("{} must contain a JSON object", path.display())
    );
}

#[test]
fn rule_lists_are_pythons_plus_probe_enable_and_the_notebook() {
    let py = Command::new("python3")
        .args([
            "-c",
            "import json; from arboria import install as i; \
             print(json.dumps([i.DENY_RULES, i.HOME_DENY_RULES]))",
        ])
        .env("PYTHONPATH", common::repo_root().join("src"))
        .output();
    let Some(out) = py.ok().filter(|o| o.status.success()) else {
        eprintln!("python not available, rules not compared");
        return;
    };
    let lists: Vec<Vec<String>> = serde_json::from_slice(&out.stdout).unwrap();
    let probe = install::PROBE_RULES.iter().map(|s| s.to_string());
    let mut deny = lists[0].clone();
    deny.extend(probe.clone());
    let mut home = lists[1].clone();
    home.extend(probe);
    // the app's own notebook (ADR 0005) only exists on the Rust side
    let ours = |rules: Vec<String>| -> Vec<String> {
        rules
            .into_iter()
            .filter(|r| {
                ![
                    "notebook.md",
                    ".arboria/outputs/",
                    ".arboria/agent/",
                    "arb idea migrate",
                ]
                .iter()
                .any(|m| r.contains(m))
            })
            .collect()
    };
    assert_eq!(ours(install::deny_rules()), deny);
    assert_eq!(ours(install::home_deny_rules()), home);
}

#[test]
fn written_files_are_byte_identical_to_pythons() {
    let tmp = TempDir::new("install-py");
    let (ours, theirs) = (tmp.path().join("rust"), tmp.path().join("python"));
    let seed = "{\"env\": {\"A\": \"ü\"}, \"n\": 1.5, \"permissions\": {\"allow\": []}}";
    for root in [&ours, &theirs] {
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(".claude").join(SETTINGS_NAME), seed).unwrap();
    }
    let rules = install::deny_rules();
    let dirs = vec!["/cfg/arboria/commands".to_string()];
    let (_, changes) = install::install(&ours, Some(&dirs), true, &rules).unwrap();
    let script = "import json, sys; from pathlib import Path; from arboria import install as i\n\
                  root, rules = Path(sys.argv[1]), json.loads(sys.argv[2])\n\
                  _, changes = i.install(root, ['/cfg/arboria/commands'], local=True, deny_rules=rules)\n\
                  print(json.dumps(changes))";
    let out = Command::new("python3")
        .args(["-c", script])
        .arg(&theirs)
        .arg(serde_json::to_string(&rules).unwrap())
        .env("PYTHONPATH", common::repo_root().join("src"))
        .output();
    let Some(out) = out.ok().filter(|o| o.status.success()) else {
        eprintln!("python not available, files not compared");
        return;
    };
    let py_changes: Vec<String> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(changes, py_changes);
    for name in [SETTINGS_NAME, LOCAL_NAME] {
        assert_eq!(
            std::fs::read_to_string(ours.join(".claude").join(name)).unwrap(),
            std::fs::read_to_string(theirs.join(".claude").join(name)).unwrap(),
            "{name}"
        );
    }
}
