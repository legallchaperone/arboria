//! `arb-rs`: parse, dispatch, print `error: <message>` and exit 1 on failure.

use std::process::ExitCode;

fn main() -> ExitCode {
    // `arb hook session-start` must never fail and never print anything extra: no clap
    // error output, no panic message, exit 0 whatever happens (CLAUDE.md invariant).
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("hook")
        && args.get(2).map(String::as_str) == Some("session-start")
    {
        std::panic::set_hook(Box::new(|_| {}));
        let _ = std::panic::catch_unwind(|| {
            arb::cli::run(arb::cli::Cli {
                json: false,
                command: arb::cli::Cmd::Hook(arb::cli::HookCmd::SessionStart),
            })
        });
        return ExitCode::SUCCESS;
    }

    let cli = arb::cli::parse();
    match arb::cli::run(cli) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}
