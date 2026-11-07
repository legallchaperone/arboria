import AppKit
import WebKit

let res = Bundle.main.resourceURL!
let env = ProcessInfo.processInfo.environment
let marimoURL = env["MARIMO_URL"] ?? "http://127.0.0.1:2719/?access_token=spike"
let outDir = env["SPIKE_OUT"] ?? NSTemporaryDirectory()
func log(_ s: String) { print(s); fflush(stdout) }

final class Scheme: NSObject, WKURLSchemeHandler {
    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        let url = task.request.url!
        let sub = url.host == "fonts" ? "fonts" : "web"
        let file = res.appendingPathComponent(sub).appendingPathComponent(url.path)
        guard let data = try? Data(contentsOf: file) else {
            task.didFailWithError(URLError(.fileDoesNotExist)); return
        }
        let ext = file.pathExtension
        let mime = ["html": "text/html", "css": "text/css", "js": "text/javascript", "woff2": "font/woff2"][ext] ?? "application/octet-stream"
        let resp = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: [
            "Content-Type": mime, "Access-Control-Allow-Origin": "*", "Content-Length": "\(data.count)"])!
        log("scheme \(url.absoluteString) from=\(webView === A ? "A" : "B") \(data.count)B")
        task.didReceive(resp); task.didReceive(data); task.didFinish()
    }
    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {}
}

final class Nav: NSObject, WKNavigationDelegate {
    func webView(_ w: WKWebView, didFinish n: WKNavigation!) { log("didFinish \(w === A ? "A" : "B") \(w.url?.absoluteString ?? "")") }
    func webView(_ w: WKWebView, didFail n: WKNavigation!, withError e: Error) { log("didFail \(e)") }
    func webView(_ w: WKWebView, didFailProvisionalNavigation n: WKNavigation!, withError e: Error) { log("didFailProvisional \(w === A ? "A" : "B") \(e)") }
}

let scheme = Scheme(), nav = Nav()
func makeConfig(injectCSS: Bool) -> WKWebViewConfiguration {
    let c = WKWebViewConfiguration()
    c.setURLSchemeHandler(scheme, forURLScheme: "arboria")
    if injectCSS {
        // Public API has no WKUserStyleSheet; inject a <link> at document start (all frames).
        let js = """
        (function(){ const l=document.createElement('link'); l.rel='stylesheet'; l.href='arboria://app/marimo.css';
          (document.head||document.documentElement).appendChild(l); })();
        """
        c.userContentController.addUserScript(WKUserScript(source: js, injectionTime: .atDocumentStart, forMainFrameOnly: false))
    }
    return c
}
var A: WKWebView!
var B: WKWebView!

let d = Delegate()
final class Delegate: NSObject, NSApplicationDelegate {
    var win: NSWindow!
    func applicationDidFinishLaunching(_ n: Notification) {
        buildMenu()
        win = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 1400, height: 860), styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        win.title = "Arboria P0"
        A = WKWebView(frame: .zero, configuration: makeConfig(injectCSS: false))
        B = WKWebView(frame: .zero, configuration: makeConfig(injectCSS: true))
        A.navigationDelegate = nav; B.navigationDelegate = nav
        if #available(macOS 13.3, *) { A.isInspectable = true; B.isInspectable = true }
        let split = NSSplitView(); split.isVertical = true; split.dividerStyle = .thin
        split.addArrangedSubview(A); split.addArrangedSubview(B)
        win.contentView = split
        split.setPosition(420, ofDividerAt: 0)
        win.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        A.load(URLRequest(url: URL(string: "arboria://app/index.html")!))
        B.load(URLRequest(url: URL(string: marimoURL)!))
        if env["SPIKE_PROBE"] != nil { DispatchQueue.main.asyncAfter(deadline: .now() + 8) { probe() } }
    }
    func buildMenu() {
        let main = NSMenu()
        let appItem = NSMenuItem(); main.addItem(appItem)
        let appMenu = NSMenu(); appItem.submenu = appMenu
        appMenu.addItem(withTitle: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        let editItem = NSMenuItem(); main.addItem(editItem)
        let edit = NSMenu(title: "Edit"); editItem.submenu = edit
        edit.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        edit.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        let goItem = NSMenuItem(); main.addItem(goItem)
        let go = NSMenu(title: "Go"); goItem.submenu = go
        let i1 = go.addItem(withTitle: "Focus App", action: #selector(focusA), keyEquivalent: "1"); i1.target = self
        let i2 = go.addItem(withTitle: "Focus Notebook", action: #selector(focusB), keyEquivalent: "2"); i2.target = self
        let k = go.addItem(withTitle: "Command Palette (menu)", action: #selector(menuK), keyEquivalent: "k"); k.target = self
        let s = go.addItem(withTitle: "Save (menu)", action: #selector(menuS), keyEquivalent: "s"); s.target = self
        NSApp.mainMenu = main
    }
    @objc func focusA() { win.makeFirstResponder(A); log("menu focusA") }
    @objc func focusB() { win.makeFirstResponder(B); log("menu focusB") }
    @objc func menuK() { log("menu cmd+K fired, firstResponder=\(String(describing: win.firstResponder))") }
    @objc func menuS() { log("menu cmd+S fired") }
}

func snap(_ w: WKWebView, _ name: String, done: @escaping () -> Void) {
    w.takeSnapshot(with: nil) { img, err in
        if let img, let tiff = img.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
           let png = rep.representation(using: .png, properties: [:]) {
            let p = (outDir as NSString).appendingPathComponent(name)
            try? png.write(to: URL(fileURLWithPath: p)); log("snapshot \(p)")
        } else { log("snapshot failed \(String(describing: err))") }
        done()
    }
}

func probe() {
    let js = """
    (async () => {
      await document.fonts.ready;
      const q = s => document.querySelector(s);
      const fam = el => el ? getComputedStyle(el).fontFamily : null;
      const fonts = [...document.fonts].map(f => f.family + ':' + f.status);
      let shadows = 0; document.querySelectorAll('*').forEach(e => { if (e.shadowRoot) shadows++; });
      return JSON.stringify({
        url: location.href, title: document.title,
        injected: !!document.querySelector('link[href="arboria://app/marimo.css"]'),
        bodyFont: fam(document.body), h1Font: fam(q('h1')), codeFont: fam(q('.cm-content')),
        bg: getComputedStyle(document.body).backgroundColor,
        notoLoaded: document.fonts.check('16px "Noto Sans SC"'),
        fonts, shadows, cookie: document.cookie.length,
        cells: document.querySelectorAll('[data-cell-id]').length
      });
    })()
    """
    let group = DispatchGroup()
    for (w, n) in [(A!, "A"), (B!, "B")] {
        group.enter()
        w.callAsyncJavaScript("return await " + js, arguments: [:], in: nil, in: .page) { r in
            switch r { case .success(let v): log("probe \(n) \(v)"); case .failure(let e): log("probe \(n) error \(e)") }
            snap(w, "snap-\(n).png") { group.leave() }
        }
    }
    group.notify(queue: .main) { keys() }
}

// Spike 2: synthesize key events through NSApp.sendEvent (same dispatch path as real keys:
// window.performKeyEquivalent -> WKWebView -> main menu), then read back what each side saw.
func key(_ chars: String, _ code: UInt16, cmd: Bool = true) {
    let w = d.win!
    for type in [NSEvent.EventType.keyDown, .keyUp] {
        let e = NSEvent.keyEvent(with: type, location: .zero, modifierFlags: cmd ? [.command] : [], timestamp: ProcessInfo.processInfo.systemUptime,
                                 windowNumber: w.windowNumber, context: nil, characters: chars, charactersIgnoringModifiers: chars, isARepeat: false, keyCode: code)!
        NSApp.sendEvent(e)
    }
}
func js(_ w: WKWebView, _ src: String, _ label: String, then: @escaping () -> Void) {
    w.callAsyncJavaScript(src, arguments: [:], in: nil, in: .page) { r in log("\(label): \(r)"); then() }
}
func after(_ s: Double, _ f: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + s, execute: f) }
let paletteJS = "return JSON.stringify({dialog: document.querySelectorAll('[role=dialog]').length, active: document.activeElement?.className?.toString().slice(0,60)})"
func keys() {
    let win = d.win!
    // 1. focus notebook, focus a cell editor, press cmd+K (marimo command palette) and cmd+S (marimo save)
    win.makeFirstResponder(B)
    js(B, "document.querySelector('.cm-content')?.focus(); return document.activeElement?.className", "B focus editor") {
      after(0.5) { log("-- send cmd+K with B focused"); key("k", 40)
      after(1.0) { js(B, paletteJS, "B after cmd+K") {
        key("\u{1b}", 53, cmd: false)
        after(0.5) { log("-- send cmd+S with B focused"); key("s", 1)
        after(1.0) { log("-- send cmd+1 (menu focusA)"); key("1", 18)
        after(0.5) { log("firstResponder now \(String(describing: win.firstResponder))")
          // 3. clipboard: native pasteboard -> A's input via cmd+V; then A -> pasteboard via cmd+C
          NSPasteboard.general.clearContents(); NSPasteboard.general.setString("从原生剪贴板来 r029", forType: .string)
          js(A, "const t=document.getElementById('t'); t.focus(); return 'ok'", "A focus input") {
            key("v", 9)
            after(0.5) { js(A, "return document.getElementById('t').value", "A input after cmd+V") {
              js(A, "const t=document.getElementById('t'); t.value='从A复制'; t.select(); return 'sel'", "A select") {
                key("c", 8)
                after(0.5) { log("pasteboard after cmd+C in A: \(NSPasteboard.general.string(forType: .string) ?? "nil")")
                  log("-- send cmd+K with A focused"); key("k", 40)
                  after(0.5) { js(A, "return document.getElementById('log').textContent", "A keydown log") {
                    if env["SPIKE_QUIT"] != nil { NSApp.terminate(nil) } } }
                } } } } } } } } } } } }
}

let app = NSApplication.shared
app.delegate = d
app.setActivationPolicy(.regular)
app.run()
