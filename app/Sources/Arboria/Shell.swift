import AppKit
import WebKit

func log(_ s: String) { FileHandle.standardError.write((s + "\n").data(using: .utf8)!) }

func jsString(_ s: String) -> String {
    let d = try! JSONSerialization.data(withJSONObject: [s])
    return String(String(decoding: d, as: UTF8.self).dropFirst().dropLast())
}

/// The app's own shortcuts win over the page's (marimo binds ⌘P and ⌘J itself; P0 spike 2:
/// the page sees keys first), so the window checks them before any view does.
final class AppWindow: NSWindow {
    var shortcuts: [String: () -> Void] = [:]
    override func performKeyEquivalent(with e: NSEvent) -> Bool {
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if mods == .command, let k = e.charactersIgnoringModifiers?.lowercased(), let f = shortcuts[k] { f(); return true }
        return super.performKeyEquivalent(with: e)
    }
}

/// Our pages' WebView: remembers the last mouse-down so a title-bar drag asked for by the
/// page (`{cmd: "drag"}`) can be handed to the window.
final class ChromeWebView: WKWebView {
    var lastMouseDown: NSEvent?
    override func mouseDown(with e: NSEvent) { lastMouseDown = e; super.mouseDown(with: e) }
}

/// Lays out the title bar page (full window when it shows home or the ⌘P switcher), the
/// notebook under it and the agent panel on the right.
final class RootView: NSView {
    weak var chrome: NSView?, notebook: NSView?, panel: NSView?
    var expanded = true { didSet { needsLayout = true } }
    var panelOpen = false { didSet { needsLayout = true } }
    let bar: CGFloat = 40, panelWidth: CGFloat = 500
    override var isFlipped: Bool { true }
    override func layout() {
        super.layout()
        let b = bounds
        let pw = panelOpen ? min(panelWidth, b.width * 0.5) : 0
        notebook?.frame = NSRect(x: 0, y: bar, width: b.width - pw, height: max(0, b.height - bar))
        panel?.frame = NSRect(x: b.width - pw, y: bar, width: pw, height: max(0, b.height - bar))
        panel?.isHidden = !panelOpen
        chrome?.frame = expanded ? b : NSRect(x: 0, y: 0, width: b.width, height: bar)
    }
}

/// One agent terminal per idea: xterm.js in a page of ours + a pty running the user's own
/// `claude` in the idea's worktree. Started the first time the panel shows that idea.
final class Terminal: NSObject, WKScriptMessageHandler {
    let idea: String, cwd: String
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
        let cols = UInt16(msg["cols"] as? Int ?? 80), rows = UInt16(msg["rows"] as? Int ?? 24)
        switch kind {
        case "ready": web.evaluateJavaScript("setIdea(\(jsString(idea)))"); start(cols: cols, rows: rows)
        case "data": pty?.write(msg["data"] as? String ?? "")
        case "resize": pty?.resize(cols: cols, rows: rows)
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
            web.evaluateJavaScript("fitNow()")  // the pane may have been laid out after xterm measured
        } catch {
            web.evaluateJavaScript("term.write(\(jsString("无法启动 agent：\(error)\r\n")))")
        }
    }

    func send(_ s: String) { pty?.write(s) }
    func stop() { pty?.terminate(); web.configuration.userContentController.removeScriptMessageHandler(forName: "term") }
}

final class Shell: NSObject, NSApplicationDelegate, NSWindowDelegate, WKScriptMessageHandler, WKNavigationDelegate {
    var arbd: Arbd!
    var win: AppWindow!
    let root = RootView()
    var chrome: ChromeWebView!
    var notebook: WKWebView!
    let panelHost = NSView()
    var terminals: [String: Terminal] = [:]
    var current: String?
    var worktrees: [String: String] = [:]
    var overlayOpen = false
    var panelOpen = false
    var notebookOrigin: String?
    let scheme = BundleScheme()
    var appConfig: WKWebViewConfiguration!
    var project = ""

    func applicationDidFinishLaunching(_ n: Notification) {
        if let a = ProcessInfo.processInfo.environment["ARB_APPEARANCE"] {  // for screenshots
            NSApp.appearance = NSAppearance(named: a == "dark" ? .darkAqua : .aqua)
        }
        buildMenu()
        guard let proj = pickProject() else { NSApp.terminate(nil); return }
        project = proj
        do { arbd = try Arbd(project: proj) } catch {
            alert("无法启动 arbd", "\(error)\n\n需要能在终端里运行的 arb（或设置 ARBORIA_ARBD_CMD）。")
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
        if let p = CommandLine.arguments.dropFirst().first(where: { !$0.hasPrefix("-") }) { return p }
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true; panel.canChooseFiles = false
        panel.message = "选择一个用 arb init 初始化过的项目"
        return panel.runModal() == .OK ? panel.url?.path : nil
    }

    // MARK: window

    private func buildWindow() {
        appConfig = WKWebViewConfiguration()
        appConfig.setURLSchemeHandler(scheme, forURLScheme: "arboria")
        let chromeConfig = appConfig.copy() as! WKWebViewConfiguration
        chromeConfig.userContentController.add(self, name: "app")
        chrome = ChromeWebView(frame: .zero, configuration: chromeConfig)
        chrome.setValue(false, forKey: "drawsBackground")  // the title bar strip shows through to the scrim
        chrome.navigationDelegate = self

        let nb = WKWebViewConfiguration()
        nb.setURLSchemeHandler(scheme, forURLScheme: "arboria")
        nb.userContentController.addUserScript(WKUserScript(source: Shell.notebookScript, injectionTime: .atDocumentStart, forMainFrameOnly: false))
        notebook = WKWebView(frame: .zero, configuration: nb)
        notebook.navigationDelegate = self
        if #available(macOS 13.3, *) { chrome.isInspectable = true; notebook.isInspectable = true }

        root.addSubview(notebook)
        root.addSubview(panelHost)
        root.addSubview(chrome)  // on top: it grows over everything for home and ⌘P
        root.chrome = chrome; root.notebook = notebook; root.panel = panelHost
        notebook.isHidden = true

        win = AppWindow(contentRect: NSRect(x: 80, y: 80, width: 1360, height: 880),
                        styleMask: [.titled, .closable, .resizable, .miniaturizable, .fullSizeContentView],
                        backing: .buffered, defer: false)
        win.titlebarAppearsTransparent = true
        win.titleVisibility = .hidden
        win.title = "Arboria — " + (project as NSString).lastPathComponent
        win.contentView = root
        win.delegate = self
        win.minSize = NSSize(width: 720, height: 480)
        win.setFrameAutosaveName("ArboriaMain")
        win.shortcuts = ["p": { [weak self] in self?.togglePalette() },
                         "j": { [weak self] in self?.togglePanel() },
                         "0": { [weak self] in self?.goHome() }]
        win.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        chrome.load(URLRequest(url: URL(string: "arboria://app/index.html")!))
    }

    /// Injected into the notebook page at document start: our stylesheet (also into every
    /// shadow root: marimo's widgets live in them), and marimo's light/dark classes kept in
    /// step with the system appearance (WKUserStyleSheet is not public API; P0 spike 1).
    static let notebookScript = """
    (function () {
      var HREF = 'arboria://app/marimo.css';
      function link() { var l = document.createElement('link'); l.rel = 'stylesheet'; l.href = HREF; return l; }
      (document.head || document.documentElement).appendChild(link());
      var attach = Element.prototype.attachShadow;
      Element.prototype.attachShadow = function (init) { var r = attach.call(this, init); try { r.appendChild(link()); } catch (e) {} return r; };
      var mq = matchMedia('(prefers-color-scheme: dark)');
      function theme() {
        var b = document.body; if (!b) return;
        var d = mq.matches, want = d ? ['dark', 'dark-theme'] : ['light', 'light-theme'], drop = d ? ['light', 'light-theme'] : ['dark', 'dark-theme'];
        if (drop.some(function (c) { return b.classList.contains(c); }) || !want.every(function (c) { return b.classList.contains(c); })) {
          b.classList.remove.apply(b.classList, drop); b.classList.add.apply(b.classList, want);
        }
        document.documentElement.style.colorScheme = d ? 'dark' : 'light';
      }
      mq.addEventListener('change', theme);
      document.addEventListener('DOMContentLoaded', function () {
        theme(); new MutationObserver(theme).observe(document.body, { attributes: true, attributeFilter: ['class'] });
      });
    })();
    """

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
        // shown for discoverability; the window handles these keys before the page (AppWindow)
        let go = sub("前往")
        for (t, k, s) in [("待你处理", "0", #selector(goHome)), ("换 idea…", "p", #selector(togglePalette)), ("agent", "j", #selector(togglePanel))] {
            go.addItem(withTitle: t, action: s, keyEquivalent: k).target = self
        }
        NSApp.mainMenu = main
    }

    func windowDidBecomeKey(_ n: Notification) { chrome?.evaluateJavaScript("window.arb && arb.active(true)"); refresh() }
    func windowDidResignKey(_ n: Notification) { chrome?.evaluateJavaScript("window.arb && arb.active(false)") }

    // MARK: state → page

    private func layoutChrome() {
        root.expanded = current == nil || overlayOpen
        root.panelOpen = panelOpen && current != nil
        notebook.isHidden = current == nil
    }

    func refresh() {
        arbd.call("project") { [weak self] r in
            guard let self else { return }
            switch r {
            case .success(let v):
                if let p = v as? [String: Any], let ideas = p["ideas"] as? [[String: Any]] {
                    for i in ideas { if let n = i["name"] as? String, let w = i["worktree"] as? String { self.worktrees[n] = w } }
                }
                self.render(["project": v])
            case .failure(let e): self.render(["error": e.description])
            }
        }
        arbd.call("inbox") { [weak self] r in
            switch r {
            case .success(let v): self?.render(["inbox": v])
            case .failure(let e): self?.render(["inboxError": e.description])
            }
        }
    }

    private func render(_ obj: [String: Any]) {
        var o = obj
        o["current"] = current ?? NSNull()
        o["panel"] = panelOpen
        guard let d = try? JSONSerialization.data(withJSONObject: o) else { return }
        chrome.evaluateJavaScript("window.arb && arb.render(\(String(decoding: d, as: UTF8.self)))")
    }

    func userContentController(_ u: WKUserContentController, didReceive m: WKScriptMessage) {
        guard m.webView === chrome, let msg = m.body as? [String: Any], let cmd = msg["cmd"] as? String else { return }
        switch cmd {
        case "ready": refresh()
        case "select": if let idea = msg["idea"] as? String { select(idea) }
        case "home": goHome()
        case "panel": togglePanel()
        case "overlay": overlayOpen = msg["open"] as? Bool ?? false; layoutChrome()
        case "focusNotebook": if current != nil { win.makeFirstResponder(notebook) }
        case "drag": if let e = chrome.lastMouseDown { win.performDrag(with: e) }
        case "zoom": win.zoom(nil)
        default: break
        }
    }

    // MARK: actions

    @objc func goHome() {
        current = nil; overlayOpen = false
        layoutChrome(); render([:]); refresh()
        win.makeFirstResponder(chrome)
    }

    @objc func togglePalette() {
        chrome.evaluateJavaScript("arb.palette()")
        win.makeFirstResponder(chrome)
    }

    @objc func togglePanel() {
        guard let idea = current else { return }
        panelOpen.toggle()
        layoutChrome(); render([:])
        if panelOpen { showTerminal(idea) }  // after layout: xterm measures its pane once it loads
        if panelOpen, let t = terminals[idea] { win.makeFirstResponder(t.web) } else { win.makeFirstResponder(notebook) }
    }

    func select(_ idea: String, done: ((String?) -> Void)? = nil) {
        arbd.call("notebook.open", ["idea": idea]) { [weak self] r in
            guard let self else { return }
            switch r {
            case .success(let v):
                guard let o = v as? [String: Any], let url = o["url"] as? String, let wt = o["worktree"] as? String,
                      let u = URL(string: url), let host = u.host, let port = u.port else { done?(nil); return }
                self.current = idea
                self.worktrees[idea] = wt
                self.notebookOrigin = "http://\(host):\(port)"
                self.notebook.load(URLRequest(url: u))
                self.layoutChrome(); self.render([:])
                if self.panelOpen { self.showTerminal(idea) }
                self.win.makeFirstResponder(self.notebook)
                done?(url)
            case .failure(let e):
                self.render(["inboxError": e.description])
                done?(nil)
            }
        }
    }

    func showTerminal(_ idea: String) {
        guard let wt = worktrees[idea] else { return }
        let t = terminals[idea] ?? Terminal(idea: idea, cwd: wt, config: appConfig)
        terminals[idea] = t
        panelHost.subviews.forEach { $0.removeFromSuperview() }
        root.layoutSubtreeIfNeeded()
        t.web.frame = panelHost.bounds
        t.web.autoresizingMask = [.width, .height]
        panelHost.addSubview(t.web)
    }

    // MARK: navigation policy: our pages stay on arboria://app, the notebook on its marimo server

    func webView(_ w: WKWebView, decidePolicyFor a: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = a.request.url else { decisionHandler(.cancel); return }
        if w === chrome { decisionHandler(url.scheme == "arboria" && url.host == "app" ? .allow : .cancel); return }
        if let o = notebookOrigin, url.absoluteString == o || url.absoluteString.hasPrefix(o + "/") { decisionHandler(.allow); return }
        if ["about", "blob", "data"].contains(url.scheme ?? "") { decisionHandler(.allow); return }
        if a.navigationType == .linkActivated, ["http", "https"].contains(url.scheme ?? "") { NSWorkspace.shared.open(url) }
        decisionHandler(.cancel)
    }

    private func alert(_ title: String, _ text: String) {
        let a = NSAlert(); a.messageText = title; a.informativeText = text; a.runModal()
    }
}
