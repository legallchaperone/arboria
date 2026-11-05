//! The app's settings pages (`settings`): machines and a project's arboria.toml as forms.
//! One test: it sets `ARBORIA_CONFIG_DIR`.

mod common;

use std::process::Command;

use arboria_core::settings::{self, PartitionForm};
use arboria_core::{clusters, paths, state};
use common::TempDir;

const CLUSTERS: &str = r#"# my machines
[box]
kind = "fake"
root = "/tmp/box-state"   # local state
account = "acct"
gpus_per_node = 4
cpus_per_gpu = 16
mem_per_gpu_gb = 100
default_partition = "gpu"
max_segments = 2
env_activate = []

[box.partitions.gpu]
max_walltime = "48h"

[box.paths]
runs = "/r/runs"
data = "/r/data"
cache = "/r/cache"
ckpt = "/r/ckpt"
out = "/r/out"

[todo]
kind = "slurm"
host = "login"
account = "<your account>"
"#;

#[test]
fn machines_and_project_settings_as_forms() {
    let tmp = TempDir::new("settings");
    let root = tmp.path().canonicalize().unwrap();
    std::env::set_var(clusters::CONFIG_DIR_ENV, root.join("cfg"));
    std::fs::create_dir_all(root.join("cfg")).unwrap();
    std::fs::write(clusters::config_path(), CLUSTERS).unwrap();

    // read leniently: a machine arb refuses still opens, with the reason
    let m = settings::machines().unwrap();
    assert_eq!(m.machines.len(), 2);
    let boxm = &m.machines[0];
    assert!(boxm.error.is_none(), "{:?}", boxm.error);
    assert_eq!(boxm.form.kind, "fake");
    assert_eq!(boxm.form.partitions[0].name, "gpu");
    assert_eq!(boxm.form.paths["ckpt"], "/r/ckpt");
    assert!(m.machines[1].error.is_some());
    assert_eq!(m.machines[1].form.host, "login");

    // edit: the comments and the other machine stay
    let mut f = boxm.form.clone();
    f.gpus_per_node = 8;
    f.partitions.push(PartitionForm {
        name: "short".into(),
        max_walltime: "2h".into(),
        charge: 2.0,
    });
    settings::save_machine(Some("box"), &f).unwrap();
    let text = std::fs::read_to_string(clusters::config_path()).unwrap();
    assert!(text.starts_with("# my machines\n"), "{text}");
    assert!(text.contains("root = \"/tmp/box-state\"   # local state"), "{text}");
    assert!(text.contains("gpus_per_node = 8"));
    assert!(text.contains("[box.partitions.short]\nmax_walltime = \"2h\"\ncharge = 2"), "{text}");
    assert!(text.contains("[todo]"));
    assert_eq!(clusters::load_profile("box", None).unwrap().gpus_per_node, 8);
    assert!(clusters::config_path().with_file_name("clusters.toml.bak").is_file());

    // a value arb refuses: nothing is written
    let before = std::fs::read_to_string(clusters::config_path()).unwrap();
    let mut bad = f.clone();
    bad.default_partition = "nope".into();
    let e = settings::save_machine(Some("box"), &bad).unwrap_err().to_string();
    assert!(e.contains("default_partition") && e.ends_with("nothing was saved"), "{e}");
    assert_eq!(std::fs::read_to_string(clusters::config_path()).unwrap(), before);

    // a new machine, a rename, a duplicate name, a removal
    let mut new = settings::blank_machine("lab");
    new.kind = "fake".into();
    new.root = Some("/tmp/lab".into());
    new.account = "a".into();
    new.mem_per_gpu_gb = 80.0;
    new.cpus_per_gpu = 8;
    for (role, p) in new.paths.iter_mut() {
        *p = format!("/lab/{role}");
    }
    settings::save_machine(None, &new).unwrap();
    assert!(settings::save_machine(None, &new).is_err(), "duplicate");
    let mut renamed = new.clone();
    renamed.name = "lab2".into();
    let m = settings::save_machine(Some("lab"), &renamed).unwrap();
    let names: Vec<&str> = m.machines.iter().map(|x| x.form.name.as_str()).collect();
    assert_eq!(names, ["box", "todo", "lab2"]);
    let m = settings::remove_machine("todo").unwrap();
    assert_eq!(m.machines.len(), 2);

    // a project's arboria.toml
    let main = root.join("lm");
    std::fs::create_dir_all(&main).unwrap();
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .args(args)
            .current_dir(&main)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(o.status.success(), "{o:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(main.join("train.py"), "print(1)\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    let ctx = paths::locate(Some(&main)).unwrap();
    state::init_state(&ctx, &mut |_| {}).unwrap();
    let p = settings::project(&ctx.main).unwrap();
    assert!(p.error.is_none(), "{:?}", p.error);
    assert_eq!(p.form.default_machine, "");
    assert_eq!(p.machines, ["box", "lab2"]);
    assert!(p.project_id.is_some());
    let mut form = p.form.clone();
    form.default_machine = "nope".into();
    let e = settings::save_project(&ctx.main, &form).unwrap_err().to_string();
    assert!(e.contains("not in clusters.toml"), "{e}");
    form.default_machine = "box".into();
    form.total_su = 400.0;
    form.wandb_entity = "me".into();
    form.override_style = "argparse".into();
    let saved = settings::save_project(&ctx.main, &form).unwrap();
    assert!(saved.error.is_none());
    let text = std::fs::read_to_string(ctx.config_path()).unwrap();
    assert!(text.contains("default_machine = \"box\""), "{text}");
    assert!(text.contains("total_su = 400"));
    assert!(text.contains("# Total service units"), "comments kept: {text}");
    assert!(text.contains("[review]"), "sections the form does not show are kept");
    let cfg = arboria_core::config::project_config(&ctx).unwrap();
    assert_eq!((cfg.default_machine.as_str(), cfg.total_su), ("box", 400.0));
    assert_eq!(cfg.project_id.map(|p| p.as_str().to_string()), p.project_id);
    form.override_style = "weird".into();
    assert!(settings::save_project(&ctx.main, &form).is_err());
}
