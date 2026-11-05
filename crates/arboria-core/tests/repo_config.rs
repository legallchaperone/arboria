//! paths / config / clusters against real files and a real git repo (the pure parts are
//! covered by the vectors).

mod common;

use std::path::Path;
use std::process::Command;

use arboria_core::paths::{self, Home, RepoContext};
use arboria_core::{clusters, config, Error};
use common::TempDir;

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn ctx_at(main: &Path) -> RepoContext {
    RepoContext {
        cwd: main.to_path_buf(),
        common_dir: main.join(".git"),
        main: main.to_path_buf(),
        toplevel: Some(main.to_path_buf()),
    }
}

#[test]
fn locate_main_worktree_and_linked_worktree() {
    let tmp = TempDir::new("locate");
    let root = tmp.path().canonicalize().unwrap();
    let main = root.join("main");
    std::fs::create_dir_all(main.join("src/deep")).unwrap();
    git(&main, &["init", "-q"]);
    git(&main, &["commit", "-q", "--allow-empty", "-m", "x"]);

    let ctx = paths::locate(Some(&main.join("src/deep"))).unwrap();
    assert_eq!(ctx.main, main);
    assert_eq!(ctx.common_dir, main.join(".git"));
    assert_eq!(ctx.toplevel.as_deref(), Some(main.as_path()));
    assert_eq!(ctx.cwd, main.join("src/deep"));
    assert!(ctx.in_main());
    assert!(!ctx.initialized());
    assert_eq!(ctx.state_dir(), main.join(".arboria"));

    git(&main, &["worktree", "add", "-q", "../wt", "-b", "idea/x"]);
    let wt = paths::locate(Some(&root.join("wt"))).unwrap();
    assert_eq!(wt.main, main, "the state dir is the main worktree's");
    assert_eq!(wt.toplevel.as_deref(), Some(root.join("wt").as_path()));
    assert!(!wt.in_main());

    // inside .git there is no worktree
    let inside = paths::locate(Some(&main.join(".git"))).unwrap();
    assert_eq!(inside.toplevel, None);

    let outside = root.join("plain");
    std::fs::create_dir_all(&outside).unwrap();
    match paths::locate(Some(&outside)) {
        Err(Error::NotARepo(msg)) => {
            assert_eq!(
                msg,
                format!("{} is not inside a git repository", outside.display())
            )
        }
        other => panic!("{other:?}"),
    }
    let err = ctx.require_initialized().unwrap_err();
    assert_eq!(err.python_class(), "NotInitializedError");
}

#[test]
fn ensure_excluded_adds_once() {
    let tmp = TempDir::new("exclude");
    let common = tmp.path().join(".git");
    assert!(paths::ensure_state_excluded(&common).unwrap());
    assert!(!paths::ensure_state_excluded(&common).unwrap());
    let path = common.join("info/exclude");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# arboria state (local only)\n/.arboria/\n"
    );
    // appended after a last line without a newline; an indented copy counts as present
    std::fs::write(&path, "*.log").unwrap();
    assert!(paths::ensure_excluded(&common, "/x/", "c").unwrap());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "*.log\n# c\n/x/\n");
    std::fs::write(&path, "  /x/  \n").unwrap();
    assert!(!paths::ensure_excluded(&common, "/x/", "c").unwrap());
}

#[test]
fn seed_notebooks_and_home() {
    let tmp = TempDir::new("home");
    let root = tmp.path().canonicalize().unwrap();
    let home = Home { root: root.clone() };
    std::fs::create_dir_all(home.seeds_dir()).unwrap();
    let nb = home.notebook("wave-idea");
    std::fs::write(&nb, "").unwrap();
    assert_eq!(paths::seed_of_notebook(&nb), None, "home not initialized");

    std::fs::create_dir_all(home.mirror_dir()).unwrap();
    std::fs::write(home.db_path(), "").unwrap();
    assert!(home.initialized());
    assert_eq!(
        paths::seed_of_notebook(&nb),
        Some((home.clone(), "wave-idea".to_string()))
    );
    assert_eq!(
        paths::seed_of_notebook(&root.join("seeds/missing.py")),
        None
    );
    let txt = root.join("seeds/notes.txt");
    std::fs::write(&txt, "").unwrap();
    assert_eq!(paths::seed_of_notebook(&txt), None);
    let elsewhere = root.join("other.py");
    std::fs::write(&elsewhere, "").unwrap();
    assert_eq!(paths::seed_of_notebook(&elsewhere), None);

    assert!(home.contains(&root));
    assert!(home.contains(&nb));
    assert!(home.contains(&root.join("not/yet/there")));
    assert!(!home.contains(root.parent().unwrap()));
    assert!(!home.contains(Path::new(&format!("{}-other", root.display()))));
}

#[test]
fn resolve_is_non_strict() {
    let tmp = TempDir::new("resolve");
    let root = tmp.path().canonicalize().unwrap();
    assert_eq!(paths::resolve(&root.join("a/../b/./c")), root.join("b/c"));
    assert_eq!(paths::resolve(&root), root);
}

/// `env_overrides` and `the_apps_clusters_editor` set process-wide variables.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn env_overrides() {
    // with the_apps_clusters_editor, the only tests here that read or set these variables
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = TempDir::new("env");
    let root = tmp.path().canonicalize().unwrap();
    std::env::set_var(paths::HOME_ENV, root.join("h"));
    std::env::set_var(paths::CACHE_ENV, root.join("c"));
    std::env::set_var(clusters::CONFIG_DIR_ENV, root.join("cfg"));
    assert_eq!(paths::home_root(), root.join("h"));
    assert_eq!(paths::home().root, root.join("h"));
    assert_eq!(paths::cache_dir(), root.join("c"));
    assert_eq!(clusters::config_path(), root.join("cfg/clusters.toml"));
    std::env::remove_var(paths::CACHE_ENV);
    std::env::set_var("XDG_CACHE_HOME", root.join("xdg"));
    assert_eq!(paths::cache_dir(), root.join("xdg/arboria"));
    let msg = clusters::load_profile("deltaai", None)
        .unwrap_err()
        .to_string();
    assert_eq!(
        msg,
        format!(
            "no cluster profiles: {} does not exist (see examples/clusters.toml; `arb machine init` will write it)",
            root.join("cfg/clusters.toml").display()
        )
    );
}

#[test]
fn project_config_from_file() {
    let tmp = TempDir::new("config");
    let main = tmp.path().canonicalize().unwrap();
    let ctx = ctx_at(&main);
    // no file: defaults, and no project id
    let cfg = config::project_config(&ctx).unwrap();
    assert_eq!(cfg.total_su, 0.0);
    assert_eq!(cfg.watch.fail_patterns.len(), 4);
    let msg = config::require_project_id(&ctx).unwrap_err().to_string();
    assert!(msg.starts_with(&format!(
        "{} has no [project] id",
        ctx.config_path().display()
    )));

    let text = config::add_project_id("[budget]\ntotal_su = 12\n", &config::project_id_for(&main))
        .unwrap();
    std::fs::write(ctx.config_path(), &text).unwrap();
    assert_eq!(config::project_config(&ctx).unwrap().total_su, 12.0);
    assert_eq!(
        config::require_project_id(&ctx).unwrap(),
        config::derive_project_id(&ctx)
    );

    std::fs::write(ctx.config_path(), "budget = [").unwrap();
    let msg = config::project_config(&ctx).unwrap_err().to_string();
    let path = ctx.config_path().display().to_string();
    assert!(
        msg.starts_with(&format!("{path} is not valid TOML (")),
        "{msg}"
    );
    assert!(msg.ends_with("); fix the project config first"), "{msg}");

    // `regex` cannot compile look-around: refused, never skipped
    std::fs::write(
        ctx.config_path(),
        "[watch]\nfail_patterns = [\"CUDA\", \"(?<=loss )nan\"]\n",
    )
    .unwrap();
    let msg = config::project_config(&ctx).unwrap_err().to_string();
    assert!(
        msg.starts_with(&format!(
            "{path}: [watch] fail_patterns[1] '(?<=loss )nan' is not a valid regular expression: "
        )),
        "{msg}"
    );
    // the defaults and typical Python patterns compile
    for p in config::DEFAULT_FAIL_PATTERNS.iter().chain(&[
        r"(?i)loss[^\n]{0,40}nan",
        r"Traceback \(most recent call last\)",
        r"\d+\s*OOM",
    ]) {
        config::compile_fail_pattern(p).unwrap();
    }
}

#[test]
fn load_profile_from_file() {
    let tmp = TempDir::new("clusters");
    let path = tmp.path().join("clusters.toml");
    let example =
        std::fs::read_to_string(common::repo_root().join("examples/clusters.toml")).unwrap();
    // the example's placeholders are refused until filled in
    std::fs::write(&path, &example).unwrap();
    let msg = clusters::load_profile("deltaai", Some(&path))
        .unwrap_err()
        .to_string();
    assert!(msg.contains("still has a <placeholder>"), "{msg}");

    let filled = example.replace("<proj>", "bjki");
    std::fs::write(&path, &filled).unwrap();
    let p = clusters::load_profile("deltaai", Some(&path)).unwrap();
    assert_eq!(p.source, path.display().to_string());
    assert_eq!(p.partition(None).unwrap().name, p.default_partition);
    assert!(!p.account_set(), "the example's account is a placeholder");

    let msg = clusters::load_profile("nope", Some(&path))
        .unwrap_err()
        .to_string();
    assert!(
        msg.starts_with(&format!("no machine 'nope' in {} (known: ", path.display())),
        "{msg}"
    );
    let msg = p.partition(Some("gpuA100")).unwrap_err().to_string();
    assert!(
        msg.starts_with("machine 'deltaai' has no partition 'gpuA100'; known: "),
        "{msg}"
    );
}

#[test]
fn the_apps_clusters_editor() {
    use arboria_core::clusters;
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let root = common::TempDir::new("clusters-edit");
    std::env::set_var(clusters::CONFIG_DIR_ENV, root.path().join("cfg2"));
    // no file yet: the editor starts from the example, which `arb` refuses (placeholders)
    let start = clusters::read_checked().unwrap();
    assert!(!start.exists);
    assert_eq!(start.text, clusters::TEMPLATE);
    assert!(!start.profiles.is_empty());
    let bad = clusters::check_text("[x\n");
    assert!(bad.syntax_error.is_some() && !bad.ok());
    let fake = |r: &str| {
        format!(
            "[fake]\nkind = \"fake\"\nroot = \"{r}/state\"\naccount = \"a\"\ngpus_per_node = 4\n\
             cpus_per_gpu = 16\nmem_per_gpu_gb = 100\ndefault_partition = \"ghx4\"\nmax_segments = 2\n\
             env_activate = []\n[fake.partitions.ghx4]\nmax_walltime = \"48h\"\n[fake.paths]\n\
             runs = \"{r}/runs\"\ndata = \"{r}/data\"\ncache = \"{r}/cache\"\nckpt = \"{r}/ckpt\"\nout = \"{r}/out\"\n"
        )
    };
    let good = fake("/tmp/f");
    let c = clusters::check_text(&good);
    assert!(c.ok(), "{c:?}");
    assert_eq!(c.profiles[0].kind.as_deref(), Some("fake"));
    // a wrong value: refused, nothing written
    let wrong = good.replace("gpus_per_node = 4", "gpus_per_node = \"four\"");
    let e = clusters::save_text(&wrong).unwrap_err().to_string();
    assert!(
        e.contains("[fake]") && e.ends_with("nothing was saved"),
        "{e}"
    );
    assert!(!clusters::config_path().exists());
    let saved = clusters::save_text(&good).unwrap();
    assert!(saved.exists && saved.ok());
    assert_eq!(
        std::fs::read_to_string(clusters::config_path()).unwrap(),
        good
    );
    assert_eq!(clusters::load_profile("fake", None).unwrap().name, "fake");
    clusters::save_text(&fake("/tmp/g")).unwrap();
    let bak = clusters::config_path().with_file_name("clusters.toml.bak");
    assert_eq!(
        std::fs::read_to_string(bak).unwrap(),
        good,
        "the previous file is kept"
    );
}
