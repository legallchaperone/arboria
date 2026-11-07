// P0 spike 4: xterm.js in a WKWebView + a real pty running the user's own `claude`.
import AppKit
import WebKit
import Darwin

let env = ProcessInfo.processInfo.environment
let res = Bundle.main.resourceURL!
let outDir = env["SPIKE_OUT"] ?? NSTemporaryDirectory()
let cwd = env["SPIKE_CWD"] ?? FileManager.default.currentDirectoryPath
let claudeArgs = (env["SPIKE_ARGS"] ?? "").split(separator: " ").map(String.init)
let script = (env["SPIKE_SCRIPT"] ?? "").components(separatedBy: "|")  // "delay:text|delay:text"
func log(_ s: String) { print(s); fflush(stdout) }

final class Scheme: NSObject, WKURLSchemeHandler {
    func webView(_ w: WKWebView, start t: WKURLSchemeTask) {
        let u = t.request.url!
        let f = res.appendingPathComponent(u.host == "fonts" ? "fonts" : "web").appendingPathComponent(u.path)
        guard let d = try? Data(contentsOf: f) else { t.didFailWithError(URLError(.fileDoesNotExist)); return }
        let mime = ["html": "text/html", "css": "text/css", "js": "text/javascript", "woff2": "font/woff2"][f.pathExtension] ?? "application/octet-stream"
        t.didReceive(HTTPURLResponse(url: u, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": mime])!)
        t.didReceive(d); t.didFinish()
    }
    func webView(_ w: WKWebView, stop t: WKURLSchemeTask) {}
}

var master: Int32 = -1
var child: pid_t = 0
var web: WKWebView!

func spawnClaude(cols: UInt16, rows: UInt16) {
    var m: Int32 = 0, s: Int32 = 0
    var ws = winsize(ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0)
    guard openpty(&m, &s, nil, nil, &ws) == 0 else { log("openpty failed"); return }
    let slavePath = String(cString: ptsname(m)); close(s)
    master = m
    var fa: posix_spawn_file_actions_t? = nil; posix_spawn_file_actions_init(&fa)
    // Open the slave in the child *after* setsid so it becomes the controlling terminal.
    posix_spawn_file_actions_addopen(&fa, 0, slavePath, O_RDWR, 0)
    posix_spawn_file_actions_adddup2(&fa, 0, 1); posix_spawn_file_actions_adddup2(&fa, 0, 2)
    posix_spawn_file_actions_addclose(&fa, m)
    posix_spawn_file_actions_addchdir_np(&fa, cwd)
    var at: posix_spawnattr_t? = nil; posix_spawnattr_init(&at)
    posix_spawnattr_setflags(&at, Int16(POSIX_SPAWN_SETSID))
    // Never leak a parent Claude session into the child (CLAUDE_CODE_CHILD_SESSION turns transcripts off).
    var e = env.filter { $0.key != "CLAUDECODE" && !$0.key.hasPrefix("CLAUDE_CODE_") && !$0.key.hasPrefix("SPIKE_") }
    e["TERM"] = "xterm-256color"; e["COLORTERM"] = "truecolor"
    let argv = ["/bin/zsh", "-lc", "exec claude " + claudeArgs.map { "'\($0)'" }.joined(separator: " ")]
    let cargv = argv.map { strdup($0) } + [nil]
    let cenv = e.map { strdup("\($0.key)=\($0.value)") } + [nil]
    let rc = posix_spawn(&child, "/bin/zsh", &fa, &at, cargv, cenv)
    log("spawn rc=\(rc) pid=\(child) tty=\(slavePath) argv=\(argv)")
    let src = DispatchSource.makeReadSource(fileDescriptor: m, queue: .main)
    src.setEventHandler {
        var buf = [UInt8](repeating: 0, count: 65536)
        let n = read(m, &buf, buf.count)
        if n <= 0 { src.cancel(); log("pty closed"); return }
        let b64 = Data(buf[0..<n]).base64EncodedString()
        web.evaluateJavaScript("w('\(b64)')")
    }
    src.resume()
}
func ptyWrite(_ s: String) { _ = s.utf8CString.withUnsafeBufferPointer { write(master, $0.baseAddress, $0.count - 1) } }

final class Bridge: NSObject, WKScriptMessageHandler {
    func userContentController(_ u: WKUserContentController, didReceive m: WKScriptMessage) {
        if m.name == "pty", let s = m.body as? String { ptyWrite(s) }
        if m.name == "ready" { spawnClaude(cols: 110, rows: 34); runScript(0) }
    }
}

func runScript(_ i: Int) {
    guard i < script.count, let colon = script[i].firstIndex(of: ":"), let delay = Double(script[i][..<colon]) else {
        if env["SPIKE_QUIT"] != nil { DispatchQueue.main.asyncAfter(deadline: .now() + 1) { kill(child, SIGTERM); NSApp.terminate(nil) } }
        return
    }
    let action = String(script[i][script[i].index(after: colon)...])
    DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
        if action == "SNAP" { snap(i) { runScript(i + 1) }; return }
        let text = action.replacingOccurrences(of: "\\r", with: "\r")
        log("type \(text.debugDescription)")
        // Type the text, then press Enter separately (a TUI may treat a pasted "\r" as part of the paste).
        if text.hasSuffix("\r") { ptyWrite(String(text.dropLast())); DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { ptyWrite("\r"); runScript(i + 1) } }
        else { ptyWrite(text); runScript(i + 1) }
    }
}
func snap(_ i: Int, done: @escaping () -> Void) {
    web.evaluateJavaScript("screenText()") { r, _ in
        let txt = (r as? String) ?? ""
        let p = (outDir as NSString).appendingPathComponent("term-\(i).txt")
        try? txt.write(toFile: p, atomically: true, encoding: .utf8); log("screen text -> \(p)")
        web.takeSnapshot(with: nil) { img, _ in
            if let img, let t = img.tiffRepresentation, let png = NSBitmapImageRep(data: t)?.representation(using: .png, properties: [:]) {
                let pp = (outDir as NSString).appendingPathComponent("term-\(i).png"); try? png.write(to: URL(fileURLWithPath: pp)); log("snapshot -> \(pp)")
            }
            done()
        }
    }
}

final class Delegate: NSObject, NSApplicationDelegate {
    var win: NSWindow!
    let bridge = Bridge(), scheme = Scheme()
    func applicationDidFinishLaunching(_ n: Notification) {
        let main = NSMenu(); let ai = NSMenuItem(); main.addItem(ai); let am = NSMenu(); ai.submenu = am
        am.addItem(withTitle: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        let ei = NSMenuItem(); main.addItem(ei); let em = NSMenu(title: "Edit"); ei.submenu = em
        em.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        em.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        NSApp.mainMenu = main
        let c = WKWebViewConfiguration()
        c.setURLSchemeHandler(scheme, forURLScheme: "arboria")
        c.userContentController.add(bridge, name: "pty"); c.userContentController.add(bridge, name: "ready")
        web = WKWebView(frame: NSRect(x: 0, y: 0, width: 1100, height: 760), configuration: c)
        win = NSWindow(contentRect: NSRect(x: 120, y: 120, width: 1100, height: 760), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        win.title = "Arboria P0 · claude"; win.contentView = web; win.makeKeyAndOrderFront(nil); win.makeFirstResponder(web)
        NSApp.activate(ignoringOtherApps: true)
        web.load(URLRequest(url: URL(string: "arboria://app/term.html")!))
    }
}
let app = NSApplication.shared
let d = Delegate(); app.delegate = d; app.setActivationPolicy(.regular); app.run()
