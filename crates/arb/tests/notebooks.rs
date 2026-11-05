//! The app's notebooks through the real `arb` (ADR 0005): `arb idea new` writes the
//! markdown notebook and links it, `arb nb run` runs its blocks in the real kernel and
//! stores the results, `arb idea migrate` turns a marimo file into one, seeds write
//! `.md` and planting carries their text over. Needs a Python with arboria's
//! dependencies: `ARBORIA_PYTHON` (else the checkout's `.venv`), skipped without one.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Sandbox {
    root: PathBuf,
    python: String,
}

impl Sandbox {
    fn new(tag: &str) -> Option<Sandbox> {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let python = std::env::var("ARBORIA_PYTHON").ok().or_else(|| {
            let p = repo.join(".venv/bin/python");
            p.exists().then(|| p.display().to_string())
        })?;
        let root = std::env::temp_dir().join(format!("arb-nb-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["cfg", "home", "cache"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let proj = root.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&proj)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(proj.join("train.py"), "print(1)\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "init"]);
        Some(Sandbox {
            root: root.canonicalize().unwrap(),
            python,
        })
    }

    fn arb(&self, cwd: &Path, args: &[&str]) -> Output {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src");
        Command::new(env!("CARGO_BIN_EXE_arb"))
            .args(args)
            .current_dir(cwd)
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .env("ARBORIA_CONFIG_DIR", self.root.join("cfg"))
            .env("ARBORIA_HOME", self.root.join("home"))
            .env("ARBORIA_CACHE_DIR", self.root.join("cache"))
            .env("ARBORIA_PYTHON", &self.python)
            .env("ARBORIA_ARB", env!("CARGO_BIN_EXE_arb"))
            .env("PYTHONPATH", src)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap()
    }

    fn arb_env(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src");
        let mut c = Command::new(env!("CARGO_BIN_EXE_arb"));
        c.args(args)
            .current_dir(cwd)
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .env("ARBORIA_CONFIG_DIR", self.root.join("cfg"))
            .env("ARBORIA_HOME", self.root.join("home"))
            .env("ARBORIA_CACHE_DIR", self.root.join("cache"))
            .env("ARBORIA_PYTHON", &self.python)
            .env("ARBORIA_ARB", env!("CARGO_BIN_EXE_arb"))
            .env("PYTHONPATH", src);
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let o = self.arb(cwd, args);
        assert!(
            o.status.success(),
            "arb {}: {}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn new_ideas_get_a_markdown_notebook_that_runs() {
    let Some(s) = Sandbox::new("run") else {
        eprintln!("skipped: no Python (ARBORIA_PYTHON)");
        return;
    };
    let proj = s.root.join("proj");
    s.ok(&proj, &["init"]);
    s.ok(
        &proj,
        &[
            "idea",
            "new",
            "warm",
            "--hypothesis",
            "长 warmup",
            "--metric",
            "val/loss",
        ],
    );
    let nb = proj.join(".arboria/notebooks/warm.md");
    let text = std::fs::read_to_string(&nb).unwrap();
    assert!(
        text.starts_with("---\narboria: 1\nidea: warm\n---\n# warm"),
        "{text}"
    );
    let wt = s.root.join("proj-warm");
    assert_eq!(std::fs::read_link(wt.join("notebook.md")).unwrap(), nb);
    let exclude = std::fs::read_to_string(proj.join(".git/info/exclude")).unwrap();
    assert!(exclude.contains("/notebook.md"), "{exclude}");
    // the human adds blocks (one without an id: arb gives it one)
    std::fs::write(
        &nb,
        format!(
            "{text}\n```arb\nx = 41\nprint('hello')\nx + 1\n```\n\n```arb {{#bad1}}\n1 / 0\n```\n"
        ),
    )
    .unwrap();
    let o = s.arb(&wt, &["nb", "run"]);
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("hello\n42"), "{out}");
    assert!(out.contains("ZeroDivisionError"), "{out}");
    let text = std::fs::read_to_string(&nb).unwrap();
    let doc = arboria_core::nbdoc::parse(&text);
    assert_eq!(doc.blocks().count(), 3);
    assert!(doc.blocks().all(|b| arboria_core::nbdoc::valid_id(&b.id)));
    let cached = arboria_core::nbout::load(&proj.join(".arboria/outputs/warm"));
    assert_eq!(cached.len(), 3, "{cached:?}");
    let x = doc.blocks().find(|b| b.code.starts_with("x = 41")).unwrap();
    assert!(cached[&x.id].ok && cached[&x.id].defines == ["x"]);
    assert!(!cached["bad1"].ok);
    // one block only
    let out = s.ok(&wt, &["nb", "run", "--block", &x.id]);
    assert!(out.contains("42") && !out.contains("ZeroDivision"), "{out}");
}

#[test]
fn marimo_notebooks_migrate_and_seeds_carry_their_text() {
    let Some(s) = Sandbox::new("migrate") else {
        eprintln!("skipped: no Python (ARBORIA_PYTHON)");
        return;
    };
    let proj = s.root.join("proj");
    s.ok(&proj, &["init"]);
    s.ok(
        &proj,
        &["idea", "new", "old", "--hypothesis", "h", "--metric", "m"],
    );
    let state = proj.join(".arboria/notebooks");
    std::fs::remove_file(state.join("old.md")).unwrap();
    std::fs::remove_file(s.root.join("proj-old/notebook.md")).unwrap();
    std::fs::write(
        state.join("old.py"),
        "import marimo\n\napp = marimo.App()\n\n\n@app.cell(hide_code=True)\ndef _(mo):\n    mo.md(r\"\"\"\n    # old\n\n    一些笔记。\n    \"\"\")\n    return\n\n\n@app.cell\ndef _():\n    import marimo as mo\n    from arboria import lab\n    return lab, mo\n\n\n@app.cell\ndef _():\n    n = 21\n    return (n,)\n\n\n@app.cell\ndef _(n):\n    n * 2\n    return\n",
    )
    .unwrap();
    let o = s.arb(&proj, &["nb", "run", "old"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("arb idea migrate old"));
    assert!(s.ok(&proj, &["idea", "migrate"]).contains("old: 已迁移"));
    assert!(s
        .ok(&proj, &["idea", "migrate", "old"])
        .contains("已经有 .md"));
    let md = std::fs::read_to_string(state.join("old.md")).unwrap();
    assert!(md.contains("# old\n\n一些笔记。"), "{md}");
    assert!(!md.contains("import marimo"), "{md}");
    assert!(state.join("old.py").is_file(), "the marimo file stays");
    assert!(s.ok(&proj, &["nb", "run", "old"]).contains("42"));
    // refused inside Claude Code
    let o = Command::new(env!("CARGO_BIN_EXE_arb"))
        .args(["idea", "migrate"])
        .current_dir(&proj)
        .env("CLAUDECODE", "1")
        .env("ARBORIA_CONFIG_DIR", s.root.join("cfg"))
        .env("ARBORIA_HOME", s.root.join("home"))
        .output()
        .unwrap();
    assert!(!o.status.success());
    // seeds: an .md notebook; planting copies the human's text, not the generated lines
    s.ok(
        &s.root,
        &[
            "idea",
            "new",
            "rank",
            "秩调度",
            "--hypothesis",
            "低秩先",
            "--metric",
            "ppl",
        ],
    );
    let seed = s.root.join("home/seeds/rank.md");
    let mut text = std::fs::read_to_string(&seed).unwrap();
    text.push_str("\n自己的笔记。\n\n```arb\n1 + 1\n```\n");
    std::fs::write(&seed, text).unwrap();
    assert!(s
        .ok(&s.root.join("home"), &["nb", "run", "rank"])
        .contains("\n2\n"));
    s.ok(&proj, &["idea", "plant", "rank"]);
    let planted = std::fs::read_to_string(state.join("rank.md")).unwrap();
    assert!(
        planted.contains("## 种子里的思路\n\n> 秩调度\n>\n> 自己的笔记。"),
        "{planted}"
    );
    assert!(
        !planted.contains("1 + 1") && !planted.contains("还没有仓库"),
        "{planted}"
    );
}

/// A stand-in for `claude -p`: records its arguments, then (FAKE_MODE=write) writes the
/// command the todo asks for, tries `arb approve` (must be refused) and closes the todo
/// with `arb todo done`, as the agent is told to; FAKE_MODE=nothing leaves it open.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_LOG"
echo '{"type":"system","subtype":"init","session_id":"sess-1"}'
if [ "$FAKE_MODE" = write ]; then
  mkdir -p "$PROJ/.arboria/commands"
  cat > "$PROJ/.arboria/commands/svd.py" <<'PY'
def svd(ctx, x, *, k=3):
    """前 k 个奇异值（假的）。"""
    return {"kind": "plot", "style": "bar", "y": "sigma", "series": [{"name": "r", "x": [1, 2, 3], "y": [3.0, 2.0, 1.0]}]}
PY
  if CLAUDECODE=1 "$ARBORIA_ARB" approve r001 >/dev/null 2>&1; then echo APPROVED >> "$FAKE_LOG"; fi
  CLAUDECODE=1 "$ARBORIA_ARB" todo done "$ARBORIA_AGENT_TASK" --result "写了 svd" >> "$FAKE_LOG" 2>&1
fi
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"写 svd"}]},"session_id":"sess-1"}'
echo '{"type":"result","subtype":"success","is_error":false,"num_turns":2,"result":"ok","session_id":"sess-1","total_cost_usd":0.01,"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100}}'
"#;

#[test]
fn a_missing_command_goes_to_the_agent_and_the_block_then_works() {
    let Some(s) = Sandbox::new("agent") else {
        eprintln!("skipped: no Python (ARBORIA_PYTHON)");
        return;
    };
    let proj = s.root.join("proj");
    s.ok(&proj, &["init"]);
    s.ok(
        &proj,
        &["idea", "new", "exp", "--hypothesis", "h", "--metric", "m"],
    );
    let wt = s.root.join("proj-exp");
    let nb = proj.join(".arboria/notebooks/exp.md");
    let text = std::fs::read_to_string(&nb).unwrap();
    std::fs::write(
        &nb,
        format!("{text}\n```arb {{#svd1}}\nlab.svd(3, ask=\"前三个奇异值\")\n```\n"),
    )
    .unwrap();
    let out = s.ok(&wt, &["nb", "run", "--block", "svd1"]);
    assert!(out.contains("已交给 agent（t001）"), "{out}");

    let fake = s.root.join("claude");
    std::fs::write(&fake, FAKE_CLAUDE).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let log = s.root.join("fake.log");
    let env = |mode: &'static str| {
        vec![
            ("ARBORIA_CLAUDE", fake.to_str().unwrap().to_string()),
            ("FAKE_LOG", log.to_str().unwrap().to_string()),
            ("FAKE_MODE", mode.to_string()),
            ("PROJ", proj.to_str().unwrap().to_string()),
        ]
    };
    let run = |mode: &'static str, args: &[&str]| {
        let e = env(mode);
        let e: Vec<(&str, &str)> = e.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let o = s.arb_env(&wt, args, &e);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    // first attempt: the agent does not close it → check_failed
    let out = run("nothing", &["agent", "run", "--once"]);
    assert!(out.contains("t001 · check_failed"), "{out}");
    let first = std::fs::read_to_string(&log).unwrap();
    assert!(first.contains("--output-format stream-json"), "{first}");
    assert!(first.contains("--disallowedTools") && first.contains("Bash(arb approve:*)"));
    assert!(!first.contains("--resume"), "{first}");
    // second: resumed in the same session, writes the command and closes the todo
    let out = run("write", &["agent", "run"]);
    assert!(out.contains("t001 · done"), "{out}");
    assert!(out.contains("队列是空的"), "{out}");
    let all = std::fs::read_to_string(&log).unwrap();
    assert!(all.contains("--resume sess-1"), "{all}");
    assert!(
        !all.contains("APPROVED"),
        "the agent must not be able to approve: {all}"
    );
    // usage over both attempts, the log of the latest
    let list = run("nothing", &["--json", "agent", "list", "--all"]);
    let rows: serde_json::Value = serde_json::from_str(&list).unwrap();
    let usage: serde_json::Value =
        serde_json::from_str(rows[0]["usage_json"].as_str().unwrap()).unwrap();
    assert_eq!(usage["turns"], 4);
    assert_eq!(rows[0]["attempts"], 2);
    let logged = run("nothing", &["agent", "log", "t001"]);
    assert!(
        logged.contains("[text] 写 svd") && logged.contains("[done] ok"),
        "{logged}"
    );
    // the block now runs the command the agent wrote
    let out = s.ok(&wt, &["nb", "run", "--block", "svd1"]);
    assert!(out.contains("[图] sigma"), "{out}");
    // inside Claude Code, `arb agent run` refuses
    let o = s.arb_env(&wt, &["agent", "run"], &[("CLAUDECODE", "1")]);
    assert!(!o.status.success());
}
