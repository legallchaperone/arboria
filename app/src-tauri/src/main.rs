fn main() {
    // `arboria --notify TITLE BODY`: post one notification as Arboria.app and exit (the
    // helper runs this; macOS shows banners only for an app the human allowed)
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--notify") {
        let (title, body) = (
            args.get(1).cloned().unwrap_or_default(),
            args.get(2).cloned().unwrap_or_default(),
        );
        match arboria_macos::notify(&title, &body) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("arboria --notify: {e}");
                std::process::exit(1)
            }
        }
    }
    if args.first().map(String::as_str) == Some("--notify-status") {
        println!("{:?}", arboria_macos::notification_status());
        return;
    }
    arboria_app::run()
}
