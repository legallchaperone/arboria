//! The Rust clap tree must match the Python CLI: command paths, group vs command, hidden
//! flags, help panels and parameters (`python_commands.txt`, generated once from
//! `src/arboria/cli.py` by `gen_python_commands.py`; regenerate it when the Python CLI
//! changes). Line format: see the generator's docstring.

use std::collections::BTreeSet;

use arb::cli::{command, HELP_PANELS};
use clap::{ArgAction, Command};

/// Commands only the Rust CLI has, left out of the comparison on purpose:
/// `arb probe …` (ADR 0003) is new in the Rust core and has no Python counterpart.
const RUST_ONLY: &[&str] = &["probe"];

fn param_token(arg: &clap::Arg) -> Option<String> {
    let id = arg.get_id().as_str();
    if matches!(id, "help" | "version" | "json") {
        return None;
    }
    if arg.is_positional() {
        let variadic = matches!(arg.get_action(), ArgAction::Append)
            || arg.get_num_args().is_some_and(|n| n.max_values() > 1);
        return Some(if variadic {
            format!("{id}...")
        } else if arg.is_required_set() {
            id.to_string()
        } else {
            format!("{id}?")
        });
    }
    let name = match (arg.get_long(), arg.get_short()) {
        (Some(l), _) => format!("--{l}"),
        (None, Some(s)) => format!("-{s}"),
        (None, None) => panic!("option {id} without a flag"),
    };
    if matches!(arg.get_action(), ArgAction::SetTrue) {
        return Some(name);
    }
    let mut tok = format!("{name}=V");
    if matches!(arg.get_action(), ArgAction::Append) {
        tok.push_str("...");
    }
    if arg.is_required_set() {
        tok.push('!');
    }
    if let Some(d) = arg.get_default_values().first() {
        tok.push_str(&format!("({})", d.to_string_lossy()));
    }
    Some(tok)
}

fn walk(cmd: &Command, path: &[&str], panel: &str, lines: &mut Vec<String>) {
    let kind = if cmd.has_subcommands() {
        "group"
    } else {
        "command"
    };
    let vis = if cmd.is_hide_set() {
        "hidden"
    } else {
        "visible"
    };
    let params: Vec<String> = cmd.get_arguments().filter_map(param_token).collect();
    let line = [
        path.join(" "),
        kind.into(),
        vis.into(),
        panel.into(),
        params.join(" "),
    ]
    .join("\t");
    lines.push(line.trim_end_matches('\t').to_string());
    let mut subs: Vec<&Command> = cmd.get_subcommands().collect();
    subs.sort_by_key(|c| c.get_name());
    for sub in subs {
        let mut p = path.to_vec();
        p.push(sub.get_name());
        walk(sub, &p, "-", lines);
    }
}

fn panel_of(name: &str) -> &'static str {
    HELP_PANELS
        .iter()
        .find(|(_, names)| names.contains(&name))
        .map_or("-", |(p, _)| p)
}

fn rust_lines() -> Vec<String> {
    let root = command();
    let mut subs: Vec<&Command> = root.get_subcommands().collect();
    subs.sort_by_key(|c| c.get_name());
    let mut lines = Vec::new();
    for sub in subs {
        if RUST_ONLY.contains(&sub.get_name()) {
            continue;
        }
        walk(sub, &[sub.get_name()], panel_of(sub.get_name()), &mut lines);
    }
    lines
}

fn python_lines() -> Vec<String> {
    include_str!("python_commands.txt")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(String::from)
        .collect()
}

#[test]
fn command_tree_matches_python() {
    let rust = rust_lines();
    let python = python_lines();
    let r: BTreeSet<&String> = rust.iter().collect();
    let p: BTreeSet<&String> = python.iter().collect();
    let only_rust: Vec<_> = r.difference(&p).collect();
    let only_python: Vec<_> = p.difference(&r).collect();
    assert!(
        only_rust.is_empty() && only_python.is_empty(),
        "clap tree differs from the Python CLI\n  only in Rust:\n    {}\n  only in Python:\n    {}",
        only_rust
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n    "),
        only_python
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n    ")
    );
    assert_eq!(rust, python, "same lines, same order");
    assert_eq!(python.len(), 79, "python_commands.txt has every command");
}

#[test]
fn every_visible_top_level_command_is_in_exactly_one_panel() {
    let root = command();
    for sub in root.get_subcommands() {
        let n = HELP_PANELS
            .iter()
            .filter(|(_, names)| names.contains(&sub.get_name()))
            .count();
        if sub.is_hide_set() {
            assert_eq!(n, 0, "hidden {} is in a panel", sub.get_name());
        } else {
            assert_eq!(n, 1, "{} must be in exactly one panel", sub.get_name());
        }
    }
    for (_, names) in HELP_PANELS {
        for name in *names {
            assert!(
                root.find_subcommand(name).is_some(),
                "panel lists unknown command {name}"
            );
        }
    }
}

#[test]
fn help_shows_the_panels_in_order() {
    let help = command().render_help().to_string();
    let mut last = 0;
    for (panel, names) in HELP_PANELS {
        let at = help
            .find(&format!("{panel}:\n"))
            .unwrap_or_else(|| panic!("panel {panel} missing:\n{help}"));
        assert!(at >= last, "panels out of order");
        last = at;
        for name in *names {
            assert!(
                help[at..].contains(&format!("\n  {name} ")),
                "{name} under {panel}"
            );
        }
    }
    assert!(!help.contains("  seed "), "the seed alias is hidden");
    assert!(!help.contains("  probe "), "probe is hidden until it ships");
}

#[test]
fn the_tree_is_valid_and_probe_is_there() {
    command().debug_assert();
    let root = command();
    let probe = root.find_subcommand("probe").expect("arb probe");
    assert!(probe.is_hide_set());
    let names: Vec<&str> = probe.get_subcommands().map(|c| c.get_name()).collect();
    assert_eq!(names, ["enable", "run", "check", "guide"]);
}

#[test]
fn parsing_follows_python() {
    use arb::cli::{Cli, Cmd};
    use clap::FromArgMatches;
    let parse = |args: &[&str]| {
        let m = command()
            .try_get_matches_from(args)
            .map_err(|e| e.to_string())?;
        Cli::from_arg_matches(&m).map_err(|e| e.to_string())
    };
    // everything after the first positional of submit is the command (allow_interspersed_args=False)
    let cli = parse(&[
        "arb", "submit", "--reason", "r", "--gpus", "4", "--set", "a=1", "--set", "b=2", "--",
        "python", "train.py", "--lr", "3",
    ])
    .unwrap();
    let Cmd::Submit(a) = cli.command else {
        panic!()
    };
    assert_eq!(a.command, ["python", "train.py", "--lr", "3"]);
    assert_eq!(a.set, ["a=1", "b=2"]);
    assert!(
        parse(&["arb", "submit", "--gpus", "1"]).is_err(),
        "--reason is required"
    );
    // global --json anywhere
    assert!(parse(&["arb", "status", "--json"]).unwrap().json);
    assert!(parse(&["arb", "--json", "idea", "list"]).unwrap().json);
    // -n default and the --as / --from spellings
    let Cmd::Logs { n, .. } = parse(&["arb", "logs", "r001"]).unwrap().command else {
        panic!()
    };
    assert_eq!(n, 100);
    assert!(parse(&["arb", "idea", "plant", "s", "--as", "x"]).is_ok());
    assert!(parse(&["arb", "seed", "list", "--all"]).is_ok());
    assert!(
        parse(&["arb", "inspect", "done", "t001"]).is_err(),
        "--file is required"
    );
}
