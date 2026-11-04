//! Notebook commands, the file side (mirrors tests/test_commands.py where the behaviour is
//! the core's): `describe` against CPython's `commands.describe` on the same sources, the
//! reserved names and the guide against the Python package, then discover / promote /
//! request in a real repo. One test sets `ARBORIA_CONFIG_DIR` (no other test here reads it).

mod common;

use std::path::Path;
use std::process::Command;

use arboria_core::commands::{self, Call, Scope};
use arboria_core::db::{self, Ledger};
use arboria_core::ideas::{self, NewIdea};
use arboria_core::model::{IdeaName, TaskKind};
use arboria_core::paths;
use common::TempDir;
use serde_json::Value;

/// stdout of `python3 -c code` with the repo's `src` importable; None without Python.
fn python(code: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("python3")
        .args(["-c", code])
        .args(args)
        .env("PYTHONPATH", common::repo_root().join("src"))
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "python not compared: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(String::from_utf8(out.stdout).unwrap())
}

const SOURCES: &[(&str, &str)] = &[
    (
        "svd",
        "def svd(ctx, run, *, k=20, log=False):\n    \"\"\"对 run 做 SVD。\n\n    k：几个。\n    \"\"\"\n    return 1\n",
    ),
    (
        "svd",
        "def svd(ctx, run, /, a, b=[1, 2,\n 3], *rest, label='x' * 40, **kw):\n    return 1\n",
    ),
    ("svd", "async def svd(ctx):\n    \"\"\"doc\"\"\"\n"),
    ("svd", "def svd():\n    \"\"\"doc\"\"\"\n"),
    ("svd", "def svd(ctx, scope='project'):\n    \"\"\"doc\"\"\"\n"),
    ("svd", "\"\"\"module doc\"\"\"\ndef other(ctx):\n    pass\n"),
    ("svd", "def svd(ctx:\n"),
    ("svd", "import os\nos.system('touch PWNED')\ndef svd(ctx): 'one line'\n"),
];

#[test]
fn describe_is_pythons_ast_and_never_runs_the_file() {
    let tmp = TempDir::new("describe");
    for (i, (name, src)) in SOURCES.iter().enumerate() {
        let path = tmp.path().join(format!("c{i}.py"));
        std::fs::write(&path, src).unwrap();
        let ours = commands::describe(&path, name).unwrap();
        let script = "import json, sys; from pathlib import Path; from arboria import commands as c\n\
                      d = c.describe(Path(sys.argv[1]), sys.argv[2])\n\
                      ps = None if d.params is None else [p.__dict__ for p in d.params]\n\
                      print(json.dumps({'description': d.description, 'problem': d.problem, 'params': ps, \
                      'summary': d.summary, 'sig': d.signature(sys.argv[2]), 'sig20': d.signature(sys.argv[2], 20)}))";
        let Some(py) = python(script, &[path.to_str().unwrap(), name]) else {
            return;
        };
        let py: Value = serde_json::from_str(&py).unwrap();
        assert_eq!(
            serde_json::to_value(&ours).unwrap()["description"],
            py["description"],
            "{src}"
        );
        assert_eq!(
            serde_json::to_value(&ours).unwrap()["params"],
            py["params"],
            "{src}"
        );
        // the parse error text is the interpreter's own; only its start is ours
        match py["problem"].as_str() {
            Some(p) if p.starts_with("cannot parse: ") => {
                assert!(ours
                    .problem
                    .as_deref()
                    .unwrap()
                    .starts_with("cannot parse: "))
            }
            p => assert_eq!(ours.problem.as_deref(), p, "{src}"),
        }
        assert_eq!(ours.summary(), py["summary"].as_str().unwrap());
        assert_eq!(ours.signature(name, None), py["sig"].as_str().unwrap());
        assert_eq!(
            ours.signature(name, Some(20)),
            py["sig20"].as_str().unwrap()
        );
    }
    assert!(!tmp.path().join("PWNED").exists());
    assert!(!Path::new("PWNED").exists());
}

#[test]
fn reserved_names_and_guide_are_pythons() {
    let Some(py) = python(
        "import json, re; from arboria import commands as c\n\
         names = sorted(n for n in c.reserved_names() if c.NAME_RE.fullmatch(n))\n\
         print(json.dumps({'names': names, 'builtins': list(c.BUILTINS.items()), 'guide': c.GUIDE}))",
        &[],
    ) else {
        return;
    };
    let py: Value = serde_json::from_str(&py).unwrap();
    let names: Vec<&str> = py["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, commands::LAB_NAMES);
    let builtins: Vec<(String, String)> = serde_json::from_value(py["builtins"].clone()).unwrap();
    let ours: Vec<(String, String)> = commands::BUILTINS
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    assert_eq!(builtins, ours);
    assert_eq!(
        include_str!("../src/commands_guide.md"),
        py["guide"].as_str().unwrap(),
        "regenerate src/commands_guide.md from arboria.commands.GUIDE"
    );
    assert_eq!(
        commands::validate_name("runs", true)
            .unwrap_err()
            .to_string(),
        "'runs' is already part of lab (lab.runs); pick another command name"
    );
}

fn git(cwd: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn discover_promote_request() {
    let tmp = TempDir::new("commands");
    let root = tmp.path().canonicalize().unwrap();
    std::env::set_var("ARBORIA_CONFIG_DIR", root.join("config"));
    let main = root.join("WaveFront");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let ctx = paths::locate(Some(&main)).unwrap();
    let state = ctx.state_dir();
    let mut ledger = Ledger::open(&state).unwrap();
    db::init_backup_repo(&state).unwrap();

    let project = commands::project_dir(&state);
    let global = commands::global_dir();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(project.join("svd.py"), SOURCES[0].1).unwrap();
    std::fs::write(
        global.join("svd.py"),
        "def svd(ctx):\n    \"\"\"old\"\"\"\n",
    )
    .unwrap();
    std::fs::write(
        global.join("runs.py"),
        "def runs(ctx):\n    \"\"\"x\"\"\"\n",
    )
    .unwrap();
    std::fs::write(global.join("Bad-Name.py"), "x").unwrap();
    let rows = commands::discover(Some(&state)).unwrap();
    let brief: Vec<(&str, Scope, bool, &str, Option<&str>)> = rows
        .iter()
        .map(|r| {
            (
                r.name.as_str(),
                r.scope,
                r.active,
                r.signature.as_str(),
                r.problem.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        brief,
        [
            (
                "runs",
                Scope::Global,
                true,
                "runs()",
                Some("shadowed by lab.runs; rename it")
            ),
            (
                "svd",
                Scope::Project,
                true,
                "svd(run, *, k=20, log=False)",
                None
            ),
            ("svd", Scope::Global, false, "svd()", None),
        ]
    );
    assert_eq!(rows[1].description, "对 run 做 SVD。");
    let found = commands::find("svd", Some(&state)).unwrap().unwrap();
    assert_eq!(found.scope, Scope::Project);

    // promote: refuses to replace a different global file without force
    let err = commands::promote(&mut ledger, "svd", false)
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("the global library already has svd (v"),
        "{err}"
    );
    let p = commands::promote(&mut ledger, "svd", true).unwrap();
    assert!(p.changed && p.replaced.is_some());
    assert_eq!(
        std::fs::read_to_string(global.join("svd.py")).unwrap(),
        SOURCES[0].1
    );
    let again = commands::promote(&mut ledger, "svd", false).unwrap();
    assert!(!again.changed);
    assert_eq!(
        commands::promote(&mut ledger, "nope", false)
            .unwrap_err()
            .to_string(),
        format!("no project command 'nope' ({}/nope.py)", project.display())
    );

    // request: one todo per missing command, reused on re-run
    let wt = ideas::new_idea(
        &ctx,
        &mut ledger,
        &IdeaName::parse("base").unwrap(),
        NewIdea {
            hypothesis: "h",
            metric: "m",
            ..Default::default()
        },
        &mut |_| {},
    )
    .unwrap();
    let args = vec!["Run('r001')".to_string()];
    let kwargs = vec![("k".to_string(), "20".to_string())];
    let call = || Call {
        args: &args,
        kwargs: &kwargs,
        path: None,
    };
    let (t, created) = commands::request(
        &mut ledger,
        &wt,
        TaskKind::CommandNew,
        "loss_curve",
        Scope::Project,
        Some("  画 loss 曲线 "),
        call(),
    )
    .unwrap();
    assert!(created);
    assert_eq!(
        t.task.text,
        "写 notebook 命令 loss_curve（项目）：画 loss 曲线\n调用：lab.loss_curve(Run('r001'), k=20)\n约定见 `arb commands guide`"
    );
    let payload = arboria_core::tasks::payload(&t.task).unwrap();
    assert_eq!(payload["description"], "画 loss 曲线");
    assert_eq!(payload["idea"], "base");
    assert_eq!(
        payload["path"],
        project.join("loss_curve.py").display().to_string()
    );
    let (t2, created) = commands::request(
        &mut ledger,
        &wt,
        TaskKind::CommandNew,
        "loss_curve",
        Scope::Project,
        None,
        call(),
    )
    .unwrap();
    assert!(!created);
    assert_eq!(t2.task.id, t.task.id);
    assert_eq!(
        commands::request(
            &mut ledger,
            &wt,
            TaskKind::CommandRevise,
            "loss_curve",
            Scope::Project,
            Some(" "),
            call()
        )
        .unwrap_err()
        .to_string(),
        "say what to change, e.g. lab.svd.revise(\"改成 log 坐标\")"
    );
    if let Some(py) = python(
        "import sys; from arboria import commands as c\n\
         sys.stdout.write(c.todo_text('command_revise', 'svd', 'global', 'x' * 2000, 'lab.svd(' + 'y' * 900 + ')'))",
        &[],
    ) {
        let ours = commands::todo_text(
            TaskKind::CommandRevise,
            "svd",
            Scope::Global,
            Some(&"x".repeat(2000)),
            &format!("lab.svd({})", "y".repeat(900)),
        );
        assert_eq!(ours, py);
    }
    assert!(commands::guide().contains(&format!("全局：{}/<名字>.py", global.display())));
    assert!(commands::guide().contains("^[a-z][a-z0-9_]{0,40}$"));
}
