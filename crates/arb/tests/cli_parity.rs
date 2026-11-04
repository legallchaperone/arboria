//! `arb-rs` against the Python `arb` (`.venv/bin/arb`, skipped when it is not installed):
//! the same commands in two identical fresh repos must print the same thing and exit the
//! same way. Paths, project ids, dates and commit hashes are normalized; stderr is
//! compared only for errors (`error: …`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One implementation's sandbox: `<tmp>/<tag>/WaveFront` plus its own home and config.
struct Side {
    exe: PathBuf,
    root: PathBuf,
    main: PathBuf,
}

impl Side {
    fn new(tmp: &Path, tag: &str, exe: PathBuf) -> Side {
        let root = tmp.join(tag);
        let main = root.join("WaveFront");
        std::fs::create_dir_all(&main).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&main)
                .status()
                .unwrap()
                .success());
        }
        Side { exe, root, main }
    }

    fn run(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(&self.exe);
        cmd.args(args).current_dir(cwd);
        for (k, _) in std::env::vars() {
            if k.starts_with("CLAUDE") || k.starts_with("ARBORIA_") || k.starts_with("GIT_") {
                cmd.env_remove(k);
            }
        }
        cmd.env("ARBORIA_HOME", self.root.join("home"))
            .env("ARBORIA_CONFIG_DIR", self.root.join("config"))
            .env("ARBORIA_CACHE_DIR", self.root.join("cache"))
            .env("HOME", self.root.join("userhome"))
            .env("PYTHONPATH", repo_root().join("src"))
            .env("NO_COLOR", "1")
            .env("COLUMNS", "200")
            .envs(env.iter().copied())
            .stdin(std::process::Stdio::null());
        cmd.output().unwrap()
    }

    /// What a run printed, with this side's paths and ids made generic.
    fn normalize(&self, text: &[u8]) -> String {
        let mut s = String::from_utf8_lossy(text).into_owned();
        s = s.replace(&self.root.display().to_string(), "<ROOT>");
        let pid = std::fs::read_to_string(self.main.join("arboria.toml"))
            .ok()
            .and_then(|t| {
                t.lines().find_map(|l| {
                    l.strip_prefix("id = \"")
                        .map(|r| r.trim_end_matches('"').to_string())
                })
            });
        if let Some(pid) = pid {
            s = s.replace(&pid, "<PID>");
        }
        mask(&s)
    }
}

/// Dates, times and hex commit ids are the run's, not the implementation's.
fn mask(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // YYYY-MM-DD[THH:MM:SSZ]
        let date = chars.len() - i >= 10
            && chars[i..i + 4].iter().all(char::is_ascii_digit)
            && chars[i + 4] == '-'
            && chars[i + 7] == '-';
        if date {
            out.push_str("<DATE>");
            i += 10;
            if chars.len() - i >= 10 && chars[i] == 'T' {
                i += 10;
            }
            continue;
        }
        let hex = chars[i..]
            .iter()
            .take_while(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            .count();
        let boundary = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
        if boundary && hex >= 10 && chars[i..i + hex].iter().any(|c| c.is_ascii_alphabetic()) {
            out.push_str("<HEX>");
            i += hex;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[derive(Debug, Clone, Copy)]
enum At {
    /// The main worktree.
    Main,
    /// The base idea's worktree.
    Wt,
    /// A directory outside any repo.
    Outside,
    /// The seed home.
    Home,
}

#[test]
fn same_output_as_python() {
    let py = repo_root().join(".venv/bin/arb");
    if !py.exists() {
        eprintln!("no {} (uv sync), parity not checked", py.display());
        return;
    }
    let tmp = std::env::temp_dir().join(format!("arb-parity-{}", std::process::id()));
    let _guard = Tmp(tmp.clone());
    let rs = Side::new(&tmp, "rs", PathBuf::from(env!("CARGO_BIN_EXE_arb-rs")));
    let pyside = Side::new(&tmp, "py", py);
    let dir = |s: &Side, at: At| match at {
        At::Main => s.main.clone(),
        At::Wt => s.root.join("WaveFront-base"),
        At::Outside => {
            let d = s.root.join("elsewhere");
            std::fs::create_dir_all(&d).unwrap();
            d
        }
        At::Home => s.root.join("home"),
    };
    let claude: &[(&str, &str)] = &[("CLAUDECODE", "1")];

    type Step = (
        At,
        &'static [&'static str],
        &'static [(&'static str, &'static str)],
    );
    use At::{Home, Main, Outside, Wt};
    let steps: Vec<Step> = vec![
        (Main, &["status"], &[]),
        (Main, &["init"], &[]),
        (Main, &["init"], &[]),
        (Main, &["status"], &[]),
        (Main, &["status", "--brief"], &[]),
        (Main, &["idea", "list"], &[]),
        (
            Main,
            &[
                "idea",
                "new",
                "base",
                "--hypothesis",
                "wider helps",
                "--metric",
                "val loss",
            ],
            &[],
        ),
        (Main, &["idea", "new", "x", "one line"], &[]),
        (Main, &["idea", "new", "y", "--hypothesis", "h"], &[]),
        (Main, &["idea", "list"], &[]),
        (Main, &["idea", "show", "base"], &[]),
        (Wt, &["status"], &[]),
        (Wt, &["status", "--brief"], &[]),
        (Wt, &["note", "first look"], claude),
        (Wt, &["note", "first look", "--confidence", "prelim"], &[]),
        (Wt, &["lesson", "propose", "pin the seed"], &[]),
        (Wt, &["lesson", "propose", "pin the seed"], &[]),
        (Main, &["lesson", "list"], &[]),
        (Main, &["lesson", "accept", "l001"], &[]),
        (Main, &["lesson", "retire", "l001"], &[]),
        (Main, &["lesson", "reject", "l001"], &[]),
        (Main, &["lesson", "list", "--all"], &[]),
        (Main, &["todo", "list"], &[]),
        (Main, &["todo", "list", "--all"], &[]),
        (Main, &["claudemd", "sync"], &[]),
        (Main, &["install-claude", "--main"], &[]),
        (Main, &["install-claude", "--main"], &[]),
        (Main, &["install-claude", "--main", "--user"], &[]),
        (Main, &["status"], &[]),
        (Main, &["export"], &[]),
        (Main, &["idea", "close", "base"], &[]),
        (Main, &["idea", "close", "base", "--drop"], &[]),
        (Main, &["idea", "list"], &[]),
        (Main, &["status"], &[]),
        (Main, &["lesson", "list", "--all", "--json"], &[]),
        (Main, &["todo", "list", "--json"], &[]),
        // the seeds: outside any repo, then in the seed home
        (Outside, &["idea", "list"], &[]),
        (
            Outside,
            &["idea", "new", "sparse", "attention is sparse"],
            claude,
        ),
        (
            Outside,
            &["idea", "new", "sparse", "attention is sparse"],
            &[],
        ),
        (Outside, &["idea", "new", "sparse", "again"], &[]),
        (Outside, &["idea", "new", "kid", "--parent", "sparse"], &[]),
        (Outside, &["idea", "show"], &[]),
        (Outside, &["idea", "set", "sparse"], &[]),
        (
            Outside,
            &["idea", "set", "sparse", "--hypothesis", "top-k is enough"],
            &[],
        ),
        (Main, &["idea", "set", "sparse", "--metric", "ppl"], &[]),
        (
            Outside,
            &["idea", "drop", "sparse", "--confidence", "guess"],
            &[],
        ),
        (Outside, &["idea", "drop", "sparse"], &[]),
        (Outside, &["idea", "drop", "sparse"], &[]),
        (Outside, &["idea", "list"], &[]),
        (Outside, &["idea", "list", "--all"], &[]),
        (Outside, &["idea", "revive", "sparse"], &[]),
        (Home, &["status"], &[]),
        (Home, &["status", "--brief"], &[]),
        (Home, &["todo", "list"], &[]),
        (Home, &["idea", "plant", "sparse"], &[]),
        (Home, &["install-claude", "--home"], &[]),
        (Home, &["install-claude", "--home"], &[]),
        (
            Main,
            &["idea", "plant", "sparse", "--as", "sparse-attn"],
            claude,
        ),
        (
            Main,
            &["idea", "plant", "sparse", "--as", "sparse-attn"],
            &[],
        ),
        (Main, &["idea", "plant", "sparse"], &[]),
        (Main, &["idea", "list"], &[]),
        (Outside, &["idea", "list"], &[]),
        (Outside, &["idea", "show", "sparse"], &[]),
    ];
    let mut failures = Vec::new();
    for (at, args, env) in steps {
        let mut outs = Vec::new();
        for side in [&rs, &pyside] {
            let cwd = dir(side, at);
            let o = side.run(&cwd, args, env);
            let code = o.status.code();
            let err = side.normalize(&o.stderr);
            let err = if code == Some(0) {
                String::new()
            } else {
                err.lines()
                    .filter(|l| l.starts_with("error: "))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            // on purpose (ADR 0003): the Rust CLI also denies `arb probe enable`
            let out = side
                .normalize(&o.stdout)
                .replace("permissions.deny += Bash(arb probe enable:*)\n", "");
            outs.push((code, out, err));
        }
        if std::env::var_os("ARB_PARITY_DUMP").is_some() {
            eprintln!(
                "$ arb {}\n{:?}\n{}{}",
                args.join(" "),
                outs[0].0,
                outs[0].1,
                outs[0].2
            );
        }
        if outs[0] != outs[1] {
            failures.push(format!(
                "arb {}\n--- rust: {:?}\n{}\n{}\n--- python: {:?}\n{}\n{}",
                args.join(" "),
                outs[0].0,
                outs[0].1,
                outs[0].2,
                outs[1].0,
                outs[1].1,
                outs[1].2
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
