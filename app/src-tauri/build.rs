fn main() {
    // An `allow-<command>` permission for each app command; capabilities/main.json grants
    // them to the main window only (ADR 0004).
    let commands: &'static [&'static str] = &[
        "platform",
        "projects",
        "board",
        "add_project",
        "card_shown",
        "approve",
        "helper_status",
        "helper_install",
        "open_notebook",
        "pty_open",
        "pty_write",
        "pty_resize",
        "pty_close",
        "pick_project",
        "clusters_read",
        "clusters_check",
        "clusters_save",
        "open_in_editor",
    ];
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(commands)),
    )
    .expect("tauri build");
}
