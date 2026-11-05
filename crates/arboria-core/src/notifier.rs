//! System notifications (ADR 0004): a reminder on the desktop, nothing more. New in the
//! Rust core (Python pushed to Claude sessions instead; ADR 0001 drops that).
//!
//! Invariants:
//! - The text is built by the caller from validated fields (run ids, statuses, SU,
//!   machine names, lines `monitor::notify` wrote); here it is cut to one short line with
//!   control characters removed.
//! - macOS: `osascript` with the text passed as arguments (`on run argv`), never spliced
//!   into the script. Linux: `notify-send` when installed. Anything that fails is
//!   silently skipped; `ARBORIA_NOTIFY=0` turns it off.

use std::process::{Command, Stdio};

/// `0` turns notifications off.
pub const ENV: &str = "ARBORIA_NOTIFY";
/// The longest body shown.
pub const MAX_BODY: usize = 200;

/// The AppleScript that shows `item 2` titled `item 1` (the text never enters the script).
const OSASCRIPT: [&str; 3] = [
    "on run argv",
    "display notification (item 2 of argv) with title (item 1 of argv)",
    "end run",
];

/// One line, no control characters, at most [`MAX_BODY`] chars.
pub fn clean(text: &str) -> String {
    let one: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let one = one.trim();
    if one.chars().count() > MAX_BODY {
        one.chars().take(MAX_BODY - 1).collect::<String>() + "…"
    } else {
        one.to_string()
    }
}

/// The program and arguments that show the notification on this system, if any.
pub fn command(title: &str, body: &str) -> Option<(String, Vec<String>)> {
    let (title, body) = (clean(title), clean(body));
    if cfg!(target_os = "macos") {
        let mut args = Vec::new();
        for line in OSASCRIPT {
            args.push("-e".to_string());
            args.push(line.to_string());
        }
        args.push(title);
        args.push(body);
        Some(("osascript".into(), args))
    } else if cfg!(target_os = "linux") {
        Some((
            "notify-send".into(),
            vec!["--app-name=Arboria".into(), title, body],
        ))
    } else {
        None
    }
}

/// Whether [`send`] does anything (`ARBORIA_NOTIFY` is not `0`).
pub fn enabled() -> bool {
    std::env::var(ENV).map_or(true, |v| v.trim() != "0")
}

/// Show a notification; every failure is ignored.
pub fn send(title: &str, body: &str) {
    if !enabled() {
        return;
    }
    let Some((program, args)) = command(title, body) else {
        return;
    };
    let _ = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shows a real notification (CI's macos job sets ARBORIA_TEST_NOTIFY=1): the
    /// command runs and succeeds. Skipped otherwise, so `cargo test` stays quiet.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_real_notification() {
        if std::env::var("ARBORIA_TEST_NOTIFY").as_deref() != Ok("1") {
            return;
        }
        let (program, args) = command("Arboria", "CI: r001 跑完了（11.86 SU）").unwrap();
        let out = std::process::Command::new(&program)
            .args(&args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{program}: {out:?}");
    }

    #[test]
    fn text_is_cleaned_and_passed_as_arguments() {
        assert_eq!(clean(" r001\n跑完了\t"), "r001 跑完了");
        let long = "x".repeat(300);
        assert_eq!(clean(&long).chars().count(), MAX_BODY);
        let (program, args) = command("Arboria", "r001 \"; do shell script \"rm\"").unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(program, "osascript");
            assert_eq!(args.last().unwrap(), "r001 \"; do shell script \"rm\"");
            assert!(args.iter().filter(|a| *a == "-e").count() == 3);
            assert!(!args[..6].iter().any(|a| a.contains("r001")));
        } else {
            assert_eq!(program, "notify-send");
            assert_eq!(args[2], "r001 \"; do shell script \"rm\"");
        }
    }
}
