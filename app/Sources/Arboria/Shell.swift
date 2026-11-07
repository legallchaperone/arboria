import AppKit
import WebKit

func log(_ s: String) { FileHandle.standardError.write((s + "\n").data(using: .utf8)!) }

/// One Claude Code terminal per idea: xterm.js in a WebView of ours + a pty running `claude`
/// in the idea's worktree. Switching ideas swaps which terminal is visible.
final class Terminal: NSObject, WKScriptMessageHandler {
    let idea: String
    let cwd: String
    let web: WKWebView
    private var pty: Pty?

    init(idea: String, cwd: String, config: WKWebViewConfiguration) {
        self.idea = idea; self.cwd = cwd
        let c = config.copy() as! WKWebViewConfiguration
        c.userContentController = WKUserContentController()
        web = WKWebView(frame: .zero, configuration: c)
        super.init()
        c.userContentController.add(self, name: "term")
        web.load(URLRequest(url: URL(string: "arboria://app/term.html")!))
    }

    func userContentController(_ u: WKUserContentController, didReceive m: WKScriptMessage) {
        guard let msg = m.body as? [String: Any], let kind = msg["kind"] as? String else { return }
        switch kind {
        case "ready":
            let cols = UInt16(msg["cols"] as? Int ?? 100), rows = UInt16(msg["rows"] as? Int ?? 30)
            start(cols: cols, rows: rows)
        case "data":
            pty?.write(msg["data"] as? String ?? "")
        case "resize":
            pty?.resize(cols: UInt16(msg["cols"] as? Int ?? 100), rows: UInt16(msg["rows"] as? Int ?? 30))
        default: break
        }
    }

    private func start(cols: UInt16, rows: UInt16) {
        let cmd = ProcessInfo.processInfo.environment["ARBORIA_CLAUDE_CMD"] ?? "exec claude"
        do {
            let p = try Pty(command: cmd, cwd: cwd, cols: cols, rows: rows)
            p.onOutput = { [weak self] d in self?.web.evaluateJavaScript("term.write(b64('\(d.base64EncodedString())'))") }
            p.onExit = { [weak self] in self?.web.evaluateJavaScript("ended()") }
            pty = p
            // The pane may have been laid out after xterm measured itself: resend the real size.
            web.evaluateJavaScript("fitNow()")
        } catch {
            web.evaluateJavaScript("term.write(\(jsString("无法启动 claude：\(error)\r\n")))")
        }
    }

    func send(_ s: String) { pty?.write(s) }
    func stop() { pty?.terminate(); web.configuration.userContentController.removeScriptMessageHandler(forName: "term") }
}

func jsString(_ s: String) -> String {
    let d = try! JSONSerialization.data(withJSONObject: [s])
    let a = String(decoding: d, as: UTF8.self)
    return String(a.dropFirst().dropLast())
}

final class Shell: NSObject, NSApplicationDelegate, WKScriptMessageHandler, WKNavigationDelegate {
    var arbd: Arbd!
    var win: NSWindow!
    var sidebar: WKWebView!
    var notebook: WKWebView!
    let termHost = NSView()
    var terminals: [String: Terminal] = [:]
    var current: String?
    var notebookOrigin: String?
    let scheme = BundleScheme()
    var appConfig: WKWebViewConfiguration!
    var project: String = ""

    func applicationDidFinishLaunching(_ n: Notification) {
        buildMenu()
        guard let proj = pickProject() else { NSApp.terminate(nil); return }
        project = proj
        do { arbd = try Arbd(project: proj) } catch {
            alert("无法启动 arbd", "\(error)\n\n需要能在终端里运行的 `arb`（或设置 ARBORIA_ARBD_CMD）。")
            NSApp.terminate(nil); return
        }
        buildWindow()
        refresh()
        Timer.scheduledTimer(withTimeInterval: 15, repeats: true) { [weak self] _ in self?.refresh() }
        if ProcessInfo.processInfo.environment["ARB_PROBE"] != nil { Probe(shell: self).run() }
    }

    func applicationWillTerminate(_ n: Notification) {
        terminals.values.forEach { $0.stop() }
        arbd?.stop()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ s: NSApplication) -> Bool { true }

    private func pickProject() -> String? {
        let env = ProcessInfo.processInfo.environment
        if let p = env["ARBORIA_PROJECT"] { return p }
        let args = CommandLine.arguments.dropFirst().filter { !$0.hasPrefix("-") }
        if let p = args.first { return p }
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true; panel.canChooseFiles = false
        panel.message = "选择一个用 arb init 初始化过的项目"
        return panel.runModal() == .OK ? panel.url?.path : nil
    }

    // MARK: window

    private func buildWindow() {
        appConfig = WKWebViewConfiguration()
        appConfig.setURLSchemeHandler(scheme, forURLScheme: "arboria")
        let sideConfig = appConfig.copy() as! WKWebViewConfiguration
        sideConfig.userContentController.add(self, name: "app")
        sidebar = WKWebView(frame: .zero, configuration: sideConfig)
        sidebar.navigationDelegate = self

        let nbConfig = WKWebViewConfiguration()
        nbConfig.setURLSchemeHandler(scheme, forURLScheme: "arboria")
        // WKUserStyleSheet is not public API: insert our stylesheet at document start (P0 spike 1).
        let inject = """
        (function(){ var l=document.createElement('link'); l.rel='stylesheet'; l.href='arboria://app/marimo.css';
          (document.head||document.documentElement).appendChild(l); })();
        """
        nbConfig.userContentController.addUserScript(WKUserScript(source: inject, injectionTime: .atDocumentStart, forMainFrameOnly: false))
        notebook = WKWebView(frame: .zero, configuration: nbConfig)
        notebook.navigationDelegate = self
        if #available(macOS 13.3, *) { sidebar.isInspectable = true; notebook.isInspectable = true }

        let split = NSSplitView()
        split.isVertical = true
        split.dividerStyle = .thin
        split.addArrangedSubview(sidebar)
        split.addArrangedSubview(notebook)
        split.addArrangedSubview(termHost)
        win = NSWindow(contentRect: NSRect(x: 80, y: 80, width: 1500, height: 900),
                       styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        win.title = "Arboria — " + (project as NSString).lastPathComponent
        win.contentView = split
        win.setFrameAutosaveName("ArboriaMain")
        win.makeKeyAndOrderFront(nil)
        split.setPosition(240, ofDividerAt: 0)
        split.setPosition(win.frame.width - 520, ofDividerAt: 1)
        NSApp.activate(ignoringOtherApps: true)
        sidebar.load(URLRequest(url: URL(string: "arboria://app/index.html")!))
    }

    private func buildMenu() {
        let main = NSMenu()
        func sub(_ title: String) -> NSMenu { let i = NSMenuItem(); main.addItem(i); let m = NSMenu(title: title); i.submenu = m; return m }
        let app = sub("Arboria")
        app.addItem(withTitle: "退出 Arboria", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        // Without an Edit menu ⌘C/⌘V do nothing inside WKWebView (P0 spike 2).
        let edit = sub("编辑")
        edit.addItem(withTitle: "撤销", action: Selector(("undo:")), keyEquivalent: "z")
        edit.addItem(withTitle: "重做", action: Selector(("redo:")), keyEquivalent: "Z")
        edit.addItem(.separator())
        edit.addItem(withTitle: "剪切", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        edit.addItem(withTitle: "复制", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        edit.addItem(withTitle: "粘贴", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        edit.addItem(withTitle: "全选", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        // ⌘1/2/3 are not marimo shortcuts, so they reach the menu (P0 spike 2).
        let go = sub("前往")
        for (t, k, s) in [("侧栏", "1", #selector(focusSidebar)), ("Notebook", "2", #selector(focusNotebook)), ("Claude", "3", #selector(focusTerminal))] {
            go.addItem(withTitle: t, action: s, keyEquivalent: k).target = self
        }
        go.addItem(withTitle: "刷新", action: #selector(refreshMenu), keyEquivalent: "r").keyEquivalentModifierMask = [.command, .shift]
        go.items.last?.target = self
        NSApp.mainMenu = main
    }

    @objc func focusSidebar() { win.makeFirstResponder(sidebar) }
    @objc func focusNotebook() { win.makeFirstResponder(notebook) }
    @objc func focusTerminal() { if let t = current.flatMap({ terminals[$0] }) { win.makeFirstResponder(t.web) } }
    @objc func refreshMenu() { refresh() }

    // MARK: sidebar ↔ arbd

    func refresh() {
        arbd.call("project") { [weak self] r in
            guard let self else { return }
            switch r {
            case .success(let v): self.render(["project": v, "current": self.current ?? NSNull()])
            case .failure(let e): self.render(["error": e.description])
            }
        }
    }

    private func render(_ obj: [String: Any]) {
        guard let d = try? JSONSerialization.data(withJSONObject: obj) else { return }
        sidebar.evaluateJavaScript("window.arb && arb.render(\(String(decoding: d, as: UTF8.self)))")
    }

    func userContentController(_ u: WKUserContentController, didReceive m: WKScriptMessage) {
        guard m.webView === sidebar, let msg = m.body as? [String: Any], let cmd = msg["cmd"] as? String else { return }
        switch cmd {
        case "ready": refresh()
        case "select": if let idea = msg["idea"] as? String { select(idea) }
        default: break
        }
    }

    func select(_ idea: String, done: ((String?) -> Void)? = nil) {
        current = idea
        render(["status": "正在打开 \(idea) 的 notebook…", "current": idea])
        arbd.call("notebook.open", ["idea": idea]) { [weak self] r in
            guard let self else { return }
            switch r {
            case .success(let v):
                guard self.current == idea, let o = v as? [String: Any], let url = o["url"] as? String,
                      let wt = o["worktree"] as? String, let u = URL(string: url) else { done?(nil); return }
                self.notebookOrigin = "\(u.scheme!)://\(u.host!):\(u.port!)"
                self.notebook.load(URLRequest(url: u))
                self.showTerminal(idea: idea, cwd: wt)
                self.refresh()
                done?(url)
            case .failure(let e):
                self.render(["status": e.description, "current": idea])
                done?(nil)
            }
        }
    }

    private func showTerminal(idea: String, cwd: String) {
        let t = terminals[idea] ?? Terminal(idea: idea, cwd: cwd, config: appConfig)
        terminals[idea] = t
        termHost.subviews.forEach { $0.removeFromSuperview() }
        t.web.frame = termHost.bounds  // before xterm measures itself in term.html
        t.web.autoresizingMask = [.width, .height]
        termHost.addSubview(t.web)
    }

    // MARK: navigation policy: our pages stay on arboria://app, the notebook on its marimo server

    func webView(_ w: WKWebView, decidePolicyFor a: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = a.request.url else { decisionHandler(.cancel); return }
        if w === sidebar { decisionHandler(url.scheme == "arboria" && url.host == "app" ? .allow : .cancel); return }
        if let o = notebookOrigin, url.absoluteString.hasPrefix(o + "/") || url.absoluteString == o { decisionHandler(.allow); return }
        if url.scheme == "about" || url.scheme == "blob" || url.scheme == "data" { decisionHandler(.allow); return }
        if a.navigationType == .linkActivated, ["http", "https"].contains(url.scheme ?? "") { NSWorkspace.shared.open(url) }
        decisionHandler(.cancel)
    }

    private func alert(_ title: String, _ text: String) {
        let a = NSAlert(); a.messageText = title; a.informativeText = text; a.runModal()
    }
}
