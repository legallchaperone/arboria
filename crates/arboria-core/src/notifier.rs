//! System notifications (ADR 0004): a reminder on the desktop, nothing more. New in the
//! Rust core (Python pushed to Claude sessions instead; ADR 0001 drops that).
//!
//! Invariants:
//! - The text is built by the caller from validated fields (run ids, statuses, SU,
//!   machine names, lines `monitor::notify` wrote); here it is cut to one short line with
//!   control characters removed.
//! - macOS: Arboria.app's executable with `--notify TITLE BODY` (posts through
//!   UserNotifications as the app, so the banner shows once the human allowed Arboria:
//!   `arboria-macos::notify`): `$ARBORIA_NOTIFIER`, else `/Applications/Arboria.app` or
//!   `~/Applications/Arboria.app`. Without the app: `osascript` with the text passed as
//!   arguments (`on run argv`), never spliced into the script; macOS files that under
//!   Script Editor, which often may not show banners. Linux: `notify-send` when installed. Anything that fails is
//!   silently skipped; `ARBORIA_NOTIFY=0`, or the app's setting (`<config>/app.json`
//!   `"notifications": false`), turns it off.

use std::process::{Command, Stdio};

/// `0` turns notifications off.
pub const ENV: &str = "ARBORIA_NOTIFY";
/// The program that posts as Arboria.app (`<it> --notify TITLE BODY`).
pub const NOTIFIER_ENV: &str = "ARBORIA_NOTIFIER";
/// Where the app's executable sits inside the bundle.
pub const APP_EXE: &str = "Arboria.app/Contents/MacOS/arboria";

/// Arboria.app's executable, if there is one to post through.
pub fn app_notifier() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os(NOTIFIER_ENV).filter(|p| !p.is_empty()) {
        let p = std::path::PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let home = std::env::home_dir().unwrap_or_default();
    [
        std::path::PathBuf::from("/Applications").join(APP_EXE),
        home.join("Applications").join(APP_EXE),
    ]
    .into_iter()
    .find(|p| p.is_file())
}
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
        if let Some(app) = app_notifier() {
            return Some((
                app.display().to_string(),
                vec!["--notify".into(), title, body],
            ));
        }
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

/// The app's settings file (`{"notifications": false}` turns notifications off).
pub fn settings_path() -> std::path::PathBuf {
    crate::clusters::config_dir().join("app.json")
}

/// Whether notifications are on in the app's settings (default: on).
pub fn setting_on() -> bool {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["notifications"].as_bool())
        .unwrap_or(true)
}

/// Whether [`send`] does anything: `ARBORIA_NOTIFY` is not `0` and the app's setting is on.
pub fn enabled() -> bool {
    std::env::var(ENV).map_or(true, |v| v.trim() != "0") && setting_on()
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
        if cfg!(target_os = "macos") && app_notifier().is_some() {
            return; // an installed Arboria.app is used instead of osascript
        }
        assert_eq!(clean(" r001\n跑完了\t"), "r001 跑完了");
        let long = "x".repeat(300);
        assert_eq!(clean(&long).chars().count(), MAX_BODY);
        let (program, args) = command("Arboria", "r001 \"; do shell script \"rm\"").unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(program, "osascript");
            assert_eq!(args.last().unwrap(), "r001 \"; do shell script \"rm\"");
            assert!(args.iter().filter(|a| *a == "-e").count() == 3);
            assert!(!args[..6].iter().any(|a| a.contains("r001")));
            // with Arboria.app (here: any file standing in for its executable)
            let exe = std::env::temp_dir().join(format!("arboria-notifier-{}", std::process::id()));
            std::fs::write(&exe, "").unwrap();
            std::env::set_var(NOTIFIER_ENV, &exe);
            let (program, args) = command("Arboria", "r001 跑完了").unwrap();
            std::env::remove_var(NOTIFIER_ENV);
            let _ = std::fs::remove_file(&exe);
            assert_eq!(program, exe.display().to_string());
            assert_eq!(args, ["--notify", "Arboria", "r001 跑完了"]);
        } else {
            assert_eq!(program, "notify-send");
            assert_eq!(args[2], "r001 \"; do shell script \"rm\"");
        }
    }
}
