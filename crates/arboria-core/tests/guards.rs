//! Source scans for the invariants the type system does not fully cover (the Rust side of
//! Python's `tests/test_submit.py` / `tests/test_ui.py` scans).

mod common;

use std::path::{Path, PathBuf};

fn sources() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p: PathBuf = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let rel = p
                    .strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// Lines containing `needle`, outside `//` comments.
fn code_hits<'a>(text: &'a str, needle: &str) -> Vec<&'a str> {
    text.lines()
        .filter(|l| {
            let code = l.split("//").next().unwrap_or("");
            code.contains(needle)
        })
        .collect()
}

#[test]
fn only_submission_calls_backend_submit() {
    for (file, text) in sources() {
        if file == "submission.rs" {
            continue;
        }
        let hits = code_hits(&text, ".submit(");
        assert!(
            hits.is_empty(),
            "{file} calls .submit(: {hits:?} (only submission.rs may)"
        );
        let lits = code_hits(&text, "SubmitPermit {");
        assert!(lits.is_empty(), "{file} builds a SubmitPermit: {lits:?}");
    }
}

#[test]
fn probe_results_have_one_writer() {
    let mut writers = Vec::new();
    for (file, text) in sources() {
        for needle in [
            "INSERT INTO probe_results",
            "UPDATE probe_results",
            "DELETE FROM probe_results",
        ] {
            for _ in code_hits(&text, needle) {
                writers.push(file.clone());
            }
        }
    }
    assert_eq!(
        writers,
        vec!["probe.rs".to_string()],
        "probe_results is written only by probe::set_probe_result"
    );
    let probe = sources()
        .into_iter()
        .find(|(f, _)| f == "probe.rs")
        .unwrap()
        .1;
    let set = probe.find("pub fn set_probe_result").unwrap();
    let insert = probe.find("INSERT INTO probe_results").unwrap();
    let next_fn = probe[set + 1..]
        .find("\npub fn ")
        .map(|i| i + set + 1)
        .unwrap();
    assert!(
        set < insert && insert < next_fn,
        "the INSERT is inside set_probe_result"
    );
}

#[test]
fn probes_cannot_reach_decisions_or_submit() {
    let probe = sources()
        .into_iter()
        .find(|(f, _)| f == "probe.rs")
        .unwrap()
        .1;
    for needle in [
        "INSERT INTO decisions",
        "UPDATE decisions",
        "submission::",
        "Backend",
    ] {
        assert!(
            code_hits(&probe, needle).is_empty(),
            "probe.rs must not use {needle}"
        );
    }
}

#[test]
fn core_is_synchronous_and_offline() {
    let manifest: toml::Table =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
    let lock = std::fs::read_to_string(common::repo_root().join("Cargo.lock")).unwrap();
    for dep in [
        "tokio",
        "async-std",
        "reqwest",
        "hyper",
        "git2",
        "gix",
        "russh",
    ] {
        for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
            let has = manifest
                .get(section)
                .and_then(|t| t.as_table())
                .is_some_and(|t| t.contains_key(dep));
            assert!(!has, "arboria-core must not depend on {dep} (ADR 0001)");
        }
        assert!(
            !lock.contains(&format!("name = \"{dep}\"")),
            "{dep} is in Cargo.lock (ADR 0001)"
        );
    }
    for (file, text) in sources() {
        for needle in ["std::net", "TcpStream", "UdpSocket", "async fn"] {
            let hits = code_hits(&text, needle);
            assert!(
                hits.is_empty(),
                "{file}: {needle} (the core makes no network calls; machines use the system ssh)"
            );
        }
    }
}

#[test]
fn no_automatic_approval() {
    // The human's decisions are recorded with decided_by = 'human' only after
    // human::require_human; no source sets it as a literal outside the modules that call it.
    let allowed = [
        "submission.rs",
        "lessons.rs",
        "probe.rs",
        "human.rs",
        "model.rs",
        "records.rs",
    ];
    for (file, text) in sources() {
        if allowed.contains(&file.as_str()) {
            continue;
        }
        let hits = code_hits(&text, "\"human\"");
        assert!(hits.is_empty(), "{file} mentions \"human\": {hits:?}");
    }
}

#[test]
fn only_the_human_check_confirms() {
    // human::Confirmed (what approve needs) is built only in human.rs; its test-only
    // constructor is used only inside #[cfg(test)] modules.
    for (file, text) in sources() {
        if file != "human.rs" {
            assert!(
                code_hits(&text, "Confirmed {").is_empty(),
                "{file} builds human::Confirmed"
            );
        }
        let tests_at = text.find("#[cfg(test)]\nmod tests").unwrap_or(text.len());
        for (i, _) in text.match_indices("Confirmed::for_tests") {
            assert!(
                i > tests_at,
                "{file} uses Confirmed::for_tests outside its test module"
            );
        }
    }
}

#[test]
fn only_submission_asks_for_touch_id() {
    // human::confirm_on_device turns a Touch ID success into Confirmed; only
    // submission::approve_on_device asks for it, and arboria_macos is reached only
    // through it.
    for (file, text) in sources() {
        if !["human.rs", "submission.rs"].contains(&file.as_str()) {
            assert!(
                code_hits(&text, "confirm_on_device").is_empty(),
                "{file} calls confirm_on_device"
            );
        }
        if file != "human.rs" {
            assert!(
                code_hits(&text, "arboria_macos").is_empty(),
                "{file} uses arboria_macos"
            );
        }
        if file != "submission.rs" {
            assert!(
                code_hits(&text, "approve_on_device").is_empty(),
                "{file} calls approve_on_device"
            );
        }
    }
}
