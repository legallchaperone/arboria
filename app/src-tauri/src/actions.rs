//! The everyday actions the app's buttons do (new idea, note, reject, cancel, close an
//! idea, seeds): each one is a fixed `arb` command line, run as the human's own `arb`
//! would be, so every check (lengths, slugs, the human-only refusals inside Claude Code)
//! stays in one place. The webview never passes an argv: it picks an [`Action`] and its
//! fields; names and run ids are checked here, free text goes after `--`.
//!
//! None of these approve anything (`arb approve` is not reachable from here; signing is
//! `submission::approve_on_device`, see lib.rs). `tests::nothing_here_approves` keeps it so.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// `arb idea new NAME --hypothesis H --metric M [--parent P]` in the project.
    IdeaNew {
        name: String,
        hypothesis: String,
        metric: String,
        parent: Option<String>,
    },
    /// `arb idea close NAME --merge|--drop`.
    IdeaClose { name: String, merge: bool },
    /// `arb note TEXT [--run R]` in the idea's worktree.
    Note {
        idea: String,
        text: String,
        run: Option<String>,
    },
    /// `arb reject R [--reason WHY]`.
    Reject { run: String, reason: Option<String> },
    /// `arb cancel R`.
    Cancel { run: String },
    /// `arb logs R -n 200` (reads only).
    Logs { run: String },
    /// `arb todo drop T`.
    TodoDrop { task: String },
    /// `arb idea new NAME GIST [--hypothesis H] [--metric M]` in the seed home.
    SeedNew {
        name: String,
        gist: String,
        hypothesis: Option<String>,
        metric: Option<String>,
    },
    /// `arb idea drop NAME` in the seed home.
    SeedDrop { name: String },
    /// `arb idea revive NAME` in the seed home.
    SeedRevive { name: String },
    /// `arb idea plant NAME [--as AS] [--hypothesis H] [--metric M]` in the target
    /// project (`root`); empty: the seed's own.
    SeedPlant {
        name: String,
        as_name: Option<String>,
        hypothesis: Option<String>,
        metric: Option<String>,
    },
}

#[derive(Debug, Serialize)]
pub struct Done {
    /// What `arb` printed (stdout, then stderr), for the toast.
    pub output: String,
}

fn slug(what: &str, s: &str) -> Result<(), String> {
    let ok = !s.is_empty()
        && s.len() <= 64
        && s.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{what} 只能用小写字母、数字和 -（以字母或数字开头）：{s:?}"
        ))
    }
}

fn id(prefix: char, what: &str, s: &str) -> Result<(), String> {
    let ok = s.len() >= 2 && s.starts_with(prefix) && s[1..].chars().all(|c| c.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(format!("不是 {what}：{s:?}"))
    }
}

fn text(what: &str, s: &str) -> Result<String, String> {
    let t = s.trim();
    if t.is_empty() {
        Err(format!("{what} 不能是空的"))
    } else {
        Ok(t.to_string())
    }
}

/// Where `arb` runs (the project, an idea's worktree, the seed home) and its arguments.
/// `worktree` finds an open idea's worktree; `home` is the seed home.
pub fn argv(
    root: &Path,
    action: &Action,
    worktree: impl Fn(&str) -> Option<PathBuf>,
    home: &Path,
) -> Result<(PathBuf, Vec<String>), String> {
    let s = |x: &str| x.to_string();
    let mut cwd = root.to_path_buf();
    let args: Vec<String> = match action {
        Action::IdeaNew {
            name,
            hypothesis,
            metric,
            parent,
        } => {
            slug("idea 名字", name)?;
            let mut a = vec![
                s("idea"),
                s("new"),
                format!("--hypothesis={}", text("假设", hypothesis)?),
                format!("--metric={}", text("指标", metric)?),
            ];
            if let Some(p) = parent.as_deref().filter(|p| !p.is_empty()) {
                slug("父 idea", p)?;
                a.push(format!("--parent={p}"));
            }
            a.extend([s("--"), name.clone()]);
            a
        }
        Action::IdeaClose { name, merge } => {
            slug("idea 名字", name)?;
            let mode = if *merge { "--merge" } else { "--drop" };
            vec![s("idea"), s("close"), s(mode), s("--"), name.clone()]
        }
        Action::Note { idea, text: t, run } => {
            slug("idea 名字", idea)?;
            cwd = worktree(idea).ok_or_else(|| format!("{idea} 没有 worktree（已经关掉了？）"))?;
            let mut a = vec![s("note")];
            if let Some(r) = run.as_deref().filter(|r| !r.is_empty()) {
                id('r', "run id", r)?;
                a.push(format!("--run={r}"));
            }
            a.extend([s("--"), text("笔记", t)?]);
            a
        }
        Action::Reject { run, reason } => {
            id('r', "run id", run)?;
            let mut a = vec![s("reject")];
            if let Some(why) = reason.as_deref().map(str::trim).filter(|w| !w.is_empty()) {
                a.push(format!("--reason={why}"));
            }
            a.extend([s("--"), run.clone()]);
            a
        }
        Action::Cancel { run } => {
            id('r', "run id", run)?;
            vec![s("cancel"), s("--"), run.clone()]
        }
        Action::Logs { run } => {
            id('r', "run id", run)?;
            vec![s("logs"), s("-n"), s("200"), s("--"), run.clone()]
        }
        Action::TodoDrop { task } => {
            id('t', "待办 id", task)?;
            vec![s("todo"), s("drop"), s("--"), task.clone()]
        }
        Action::SeedNew {
            name,
            gist,
            hypothesis,
            metric,
        } => {
            slug("种子名字", name)?;
            cwd = home.to_path_buf();
            let mut a = vec![s("idea"), s("new")];
            if let Some(h) = hypothesis
                .as_deref()
                .map(str::trim)
                .filter(|h| !h.is_empty())
            {
                a.push(format!("--hypothesis={h}"));
            }
            if let Some(m) = metric.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
                a.push(format!("--metric={m}"));
            }
            a.extend([s("--"), name.clone(), text("一句话", gist)?]);
            a
        }
        Action::SeedDrop { name } | Action::SeedRevive { name } => {
            slug("种子名字", name)?;
            cwd = home.to_path_buf();
            let verb = if matches!(action, Action::SeedDrop { .. }) {
                "drop"
            } else {
                "revive"
            };
            vec![s("idea"), s(verb), s("--"), name.clone()]
        }
        Action::SeedPlant {
            name,
            as_name,
            hypothesis,
            metric,
        } => {
            slug("种子名字", name)?;
            let mut a = vec![s("idea"), s("plant")];
            if let Some(n) = as_name.as_deref().filter(|n| !n.is_empty()) {
                slug("idea 名字", n)?;
                a.push(format!("--as={n}"));
            }
            if let Some(h) = hypothesis
                .as_deref()
                .map(str::trim)
                .filter(|h| !h.is_empty())
            {
                a.push(format!("--hypothesis={h}"));
            }
            if let Some(m) = metric.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
                a.push(format!("--metric={m}"));
            }
            a.extend([s("--"), name.clone()]);
            a
        }
    };
    Ok((cwd, args))
}

/// Run `arb` (stdin closed, the app's environment: a Claude Code launch is still refused
/// by `arb` itself). Its error message is what the human sees.
pub fn run(arb: &Path, cwd: &Path, args: &[String]) -> Result<Done, String> {
    let out = Command::new(arb)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .env("NO_COLOR", "1")
        .output()
        .map_err(|e| format!("{}: {e}", arb.display()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if out.status.success() {
        if !err.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&err);
        }
        Ok(Done { output: text })
    } else {
        Err(if err.is_empty() { text } else { err })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Action> {
        vec![
            Action::IdeaNew {
                name: "lr-sweep".into(),
                hypothesis: "-h".into(),
                metric: "val".into(),
                parent: Some("base".into()),
            },
            Action::IdeaClose {
                name: "lr-sweep".into(),
                merge: true,
            },
            Action::Note {
                idea: "lr-sweep".into(),
                text: "--run r9 looks off".into(),
                run: Some("r002".into()),
            },
            Action::Reject {
                run: "r002".into(),
                reason: Some("wrong lr".into()),
            },
            Action::Cancel { run: "r002".into() },
            Action::Logs { run: "r002".into() },
            Action::TodoDrop {
                task: "t003".into(),
            },
            Action::SeedNew {
                name: "moe".into(),
                gist: "try experts".into(),
                hypothesis: None,
                metric: None,
            },
            Action::SeedDrop { name: "moe".into() },
            Action::SeedRevive { name: "moe".into() },
            Action::SeedPlant {
                name: "moe".into(),
                as_name: None,
                hypothesis: None,
                metric: None,
            },
        ]
    }

    fn of(a: &Action) -> Result<(PathBuf, Vec<String>), String> {
        argv(
            Path::new("/p"),
            a,
            |i| Some(PathBuf::from(format!("/p-{i}"))),
            Path::new("/home"),
        )
    }

    #[test]
    fn nothing_here_approves() {
        for a in all() {
            let (_, args) = of(&a).unwrap();
            let words: Vec<&str> = args
                .iter()
                .take_while(|x| *x != "--")
                .map(String::as_str)
                .collect();
            for bad in ["approve", "accept", "submit", "enable"] {
                assert!(!words.contains(&bad), "{a:?}: {args:?}");
            }
        }
    }

    #[test]
    fn free_text_cannot_become_a_flag() {
        let (cwd, args) = of(&all()[2]).unwrap();
        assert_eq!(cwd, PathBuf::from("/p-lr-sweep"));
        assert_eq!(args, ["note", "--run=r002", "--", "--run r9 looks off"]);
        let (_, args) = of(&all()[0]).unwrap();
        assert_eq!(
            args,
            [
                "idea",
                "new",
                "--hypothesis=-h",
                "--metric=val",
                "--parent=base",
                "--",
                "lr-sweep"
            ]
        );
        let (cwd, args) = of(&all()[7]).unwrap();
        assert_eq!(cwd, PathBuf::from("/home"));
        assert_eq!(args, ["idea", "new", "--", "moe", "try experts"]);
    }

    #[test]
    fn names_and_ids_are_checked() {
        let bad = [
            Action::Cancel {
                run: "r1; rm".into(),
            },
            Action::Cancel { run: "-r1".into() },
            Action::IdeaClose {
                name: "--merge".into(),
                merge: false,
            },
            Action::TodoDrop {
                task: "r001".into(),
            },
            Action::Note {
                idea: "x".into(),
                text: "  ".into(),
                run: None,
            },
        ];
        for a in bad {
            assert!(of(&a).is_err(), "{a:?}");
        }
    }

    /// Every action through the real `arb`, in a throwaway repo and seed home
    /// (`ARBORIA_TEST_ARB=<path to arb>`; the notebooks need `ARBORIA_PYTHON` too).
    #[test]
    fn with_the_real_arb() {
        let Some(arb) = std::env::var_os("ARBORIA_TEST_ARB").map(PathBuf::from) else {
            eprintln!("skipped: set ARBORIA_TEST_ARB");
            return;
        };
        let _env = crate::clone::tests::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("arboria-actions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (repo, home, cfg) = (tmp.join("lm"), tmp.join("home"), tmp.join("cfg"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("ARBORIA_HOME", &home);
        std::env::set_var("ARBORIA_CONFIG_DIR", &cfg);
        std::env::set_var("ARBORIA_CACHE_DIR", tmp.join("cache"));
        for (k, v) in [
            ("GIT_AUTHOR_NAME", "t"),
            ("GIT_AUTHOR_EMAIL", "t@t"),
            ("GIT_COMMITTER_NAME", "t"),
            ("GIT_COMMITTER_EMAIL", "t@t"),
        ] {
            std::env::set_var(k, v);
        }
        std::fs::write(
            cfg.join("clusters.toml"),
            format!(
                "[box]\nkind = \"fake\"\nroot = \"{}\"\naccount = \"a\"\ngpus_per_node = 4\n\
                 cpus_per_gpu = 8\nmem_per_gpu_gb = 80\ndefault_partition = \"gpu\"\n\
                 max_segments = 1\nenv_activate = []\n[box.partitions.gpu]\nmax_walltime = \"48h\"\n\
                 [box.paths]\nruns = \"/r/runs\"\ndata = \"/r/data\"\ncache = \"/r/cache\"\n\
                 ckpt = \"/r/ckpt\"\nout = \"/r/out\"\n",
                tmp.join("box").display()
            ),
        )
        .unwrap();
        let sh = |cwd: &Path, args: &[&str]| {
            let o = Command::new(args[0])
                .args(&args[1..])
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(o.status.success(), "{args:?}: {o:?}");
            String::from_utf8_lossy(&o.stdout).into_owned()
        };
        sh(&repo, &["git", "init", "-q", "-b", "main"]);
        std::fs::write(repo.join("train.py"), "print(1)\n").unwrap();
        sh(&repo, &["git", "add", "-A"]);
        sh(&repo, &["git", "commit", "-q", "-m", "init"]);
        let arb_s = arb.to_str().unwrap();
        sh(&repo, &[arb_s, "init"]);
        let toml = std::fs::read_to_string(repo.join("arboria.toml")).unwrap();
        std::fs::write(
            repo.join("arboria.toml"),
            toml.replace("default_machine = \"\"", "default_machine = \"box\""),
        )
        .unwrap();
        let repo = arboria_core::paths::resolve(&repo);
        let board = || arboria_core::view::board(&repo).unwrap();
        let wt = |idea: &str| {
            board()
                .ideas
                .into_iter()
                .find(|i| i.name == idea && i.stage == "open")
                .and_then(|i| i.worktree)
                .map(PathBuf::from)
        };
        let act = |root: &Path, a: Action| {
            let (cwd, args) = argv(root, &a, wt, &home)?;
            run(&arb, &cwd, &args)
        };

        // inside Claude Code a note is refused by arb itself
        std::env::set_var("CLAUDECODE", "1");
        act(
            &repo,
            Action::IdeaNew {
                name: "warm".into(),
                hypothesis: "--warmup helps".into(),
                metric: "val/loss".into(),
                parent: None,
            },
        )
        .unwrap();
        let note = Action::Note {
            idea: "warm".into(),
            text: "from claude".into(),
            run: None,
        };
        assert!(act(&repo, note).is_err(), "a note inside Claude Code");
        std::env::remove_var("CLAUDECODE");
        std::env::remove_var("CLAUDE_CODE_ENTRYPOINT");

        act(
            &repo,
            Action::IdeaNew {
                name: "warm2".into(),
                hypothesis: "longer".into(),
                metric: "val/loss".into(),
                parent: Some("warm".into()),
            },
        )
        .unwrap();
        let b = board();
        let w = b.ideas.iter().find(|i| i.name == "warm").unwrap();
        assert_eq!(w.hypothesis.as_deref(), Some("--warmup helps"));
        assert_eq!(
            b.ideas
                .iter()
                .find(|i| i.name == "warm2")
                .unwrap()
                .parent
                .as_deref(),
            Some("warm")
        );

        // a run to reject
        let wdir = wt("warm").unwrap();
        sh(
            &wdir,
            &[
                arb_s, "submit", "--reason", "try", "--gpus", "1", "--time", "1h", "--", "python",
                "train.py",
            ],
        );
        let run_id = board().runs[0].id.clone();
        act(
            &repo,
            Action::Note {
                idea: "warm".into(),
                text: "-- spikes at 2k".into(),
                run: Some(run_id.clone()),
            },
        )
        .unwrap();
        assert!(board()
            .notes
            .iter()
            .any(|n| n.text == "-- spikes at 2k" && n.run.as_deref() == Some(run_id.as_str())));
        act(
            &repo,
            Action::Reject {
                run: run_id.clone(),
                reason: Some("lr too high".into()),
            },
        )
        .unwrap();
        assert_eq!(board().runs[0].status, "rejected");
        let e = act(&repo, Action::Cancel { run: run_id }).unwrap_err();
        assert!(!e.is_empty());

        act(
            &repo,
            Action::IdeaClose {
                name: "warm2".into(),
                merge: false,
            },
        )
        .unwrap();
        act(
            &repo,
            Action::IdeaClose {
                name: "warm".into(),
                merge: true,
            },
        )
        .unwrap();
        let b = board();
        let stage = |n: &str| b.ideas.iter().find(|i| i.name == n).unwrap().stage.clone();
        assert_eq!(
            (stage("warm").as_str(), stage("warm2").as_str()),
            ("merged", "dropped")
        );

        // seeds
        act(
            &home,
            Action::SeedNew {
                name: "moe".into(),
                gist: "try experts".into(),
                hypothesis: Some("cheaper".into()),
                metric: None,
            },
        )
        .unwrap();
        let seeds = || arboria_core::view::board(&home).unwrap().ideas;
        assert_eq!(seeds()[0].stage, "seed");
        act(&home, Action::SeedDrop { name: "moe".into() }).unwrap();
        assert_eq!(seeds()[0].stage, "dropped");
        act(&home, Action::SeedRevive { name: "moe".into() }).unwrap();
        let e = act(
            &repo,
            Action::SeedPlant {
                name: "moe".into(),
                as_name: Some("moe-a".into()),
                hypothesis: None,
                metric: None,
            },
        )
        .unwrap_err();
        assert!(e.contains("指标"), "planting a seed with no metric: {e}");
        act(
            &repo,
            Action::SeedPlant {
                name: "moe".into(),
                as_name: Some("moe-a".into()),
                hypothesis: None,
                metric: Some("val/loss".into()),
            },
        )
        .unwrap();
        assert!(wt("moe-a").is_some());
        assert_eq!(seeds()[0].stage, "planted");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
