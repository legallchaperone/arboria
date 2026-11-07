import AppKit
import WebKit

/// ARB_PROBE=1: drive the app without a person, for verification and design review. Saves
/// whole-window pictures (composited from each WebView's snapshot) to ARB_PROBE_OUT:
/// home, the first idea's notebook (top and further down), the ⌘P switcher, the agent
/// panel; prints what each page shows; then quits. ARB_PROBE_PROMPT is typed into the agent.
final class Probe {
    let shell: Shell
    let out: String
    let prompt: String?
    init(shell: Shell) {
        self.shell = shell
        let env = ProcessInfo.processInfo.environment
        out = env["ARB_PROBE_OUT"] ?? NSTemporaryDirectory()
        prompt = env["ARB_PROBE_PROMPT"]
    }

    private func after(_ s: Double, _ f: @escaping () -> Void) { DispatchQueue.main.asyncAfter(deadline: .now() + s, execute: f) }
    private func say(_ s: String) { print("probe " + s); fflush(stdout) }

    private func eval(_ w: WKWebView, _ js: String, _ label: String, then: @escaping () -> Void) {
        w.callAsyncJavaScript(js, arguments: [:], in: nil, in: .page) { r in
            switch r {
            case .success(let v): self.say("\(label): \(v)")
            case .failure(let e): self.say("\(label) error: \(e)")
            }
            then()
        }
    }

    /// The window as the person sees it: each visible WebView's snapshot drawn at its frame,
    /// bottom to top (the title bar page last; it is transparent where it shows the notebook).
    private func shot(_ name: String, then: @escaping () -> Void) {
        let root = shell.root
        var all: [WKWebView] = [shell.notebook]
        all.append(contentsOf: shell.panelHost.subviews.compactMap { $0 as? WKWebView })
        all.append(shell.chrome)
        let views = all.filter { v in
            let hostHidden = v.superview?.isHidden ?? true
            return !v.isHidden && !hostHidden && v.frame.width > 0
        }
        let scale = shell.win.backingScaleFactor
        let size = NSSize(width: root.bounds.width * scale, height: root.bounds.height * scale)
        let canvas = NSImage(size: size)
        var pictures: [(NSRect, NSImage)] = []
        func next(_ i: Int) {
            guard i < views.count else {
                canvas.lockFocus()
                for (r, img) in pictures {
                    let frame = NSRect(x: r.minX * scale, y: (root.bounds.height - r.maxY) * scale, width: r.width * scale, height: r.height * scale)
                    img.draw(in: frame)
                }
                canvas.unlockFocus()
                if let t = canvas.tiffRepresentation, let png = NSBitmapImageRep(data: t)?.representation(using: .png, properties: [:]) {
                    let p = (out as NSString).appendingPathComponent(name + ".png")
                    try? png.write(to: URL(fileURLWithPath: p)); say("shot \(p)")
                }
                then(); return
            }
            let v = views[i]
            let frame = v.convert(v.bounds, to: root)
            v.takeSnapshot(with: nil) { img, _ in
                if let img { pictures.append((frame, img)) }
                next(i + 1)
            }
        }
        next(0)
    }

    func run() {
        after(4) {
            self.eval(self.shell.chrome, "return document.body.innerText.replace(/\\n+/g, ' | ')", "home") {
                self.shot("1-home") {
                    if ProcessInfo.processInfo.environment["ARB_PROBE"] == "pages" { self.pages() } else { self.openIdea() }
                }
            }
        }
    }

    private func openIdea() {
        shell.arbd.call("project") { r in
            guard case .success(let v) = r, let p = v as? [String: Any], let ideas = p["ideas"] as? [[String: Any]],
                  let first = ideas.first(where: { $0["has_notebook"] as? Bool == true })?["name"] as? String else {
                self.say("no idea to open"); NSApp.terminate(nil); return
            }
            self.say("select \(first)")
            self.shell.select(first) { _ in
                self.after(12) {
                    let js = "return JSON.stringify({cells: document.querySelectorAll('[data-cell-id]').length, theme: document.body.className, font: getComputedStyle(document.body).fontFamily.slice(0, 40), shadows: [...document.querySelectorAll('*')].filter(e => e.shadowRoot && e.shadowRoot.querySelector('link[href=\"arboria://app/marimo.css\"]')).length})"
                    self.eval(self.shell.notebook, js, "notebook") {
                        self.shot("2-notebook") {
                            self.shell.notebook.evaluateJavaScript("document.querySelector('marimo-table, [data-testid=\"cell-column\"] > *:nth-child(5)')?.scrollIntoView({block: 'center'})")
                            self.after(2) { self.shot("3-notebook-lower") { self.palette() } }
                        }
                    }
                }
            }
        }
    }

    private func palette() {
        shell.togglePalette()
        after(1.5) {
            self.shot("4-switcher") {
                self.shell.chrome.evaluateJavaScript("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape'}))")
                self.after(1) { self.panel() }
            }
        }
    }

    private func panel() {
        // WebKit stops painting (requestAnimationFrame) in occluded windows, and xterm draws on
        // animation frames: make sure the window is on screen before the agent starts.
        NSApp.activate(ignoringOtherApps: true)
        shell.win.orderFrontRegardless()
        say("window occluded: \(!shell.win.occlusionState.contains(.visible))")
        if ProcessInfo.processInfo.environment["ARB_PROBE"] == "drawer" { drawer(); return }
        shell.panelTab = "terminal"
        shell.togglePanel()
        guard let idea = shell.current, let t = shell.terminals[idea] else { finish(); return }
        after(8) {
            if let p = self.prompt {
                t.send("\u{1b}[B"); self.after(0.5) { t.send("\r") }  // a new worktree: "trust this folder?" → yes
                self.after(4) { t.send(p) }
                self.after(4.5) { t.send("\r") }
                self.after(30) { self.panelShot(t) }
            } else { self.panelShot(t) }
        }
    }

    private func panelShot(_ t: Terminal) {
        let js = "const r = document.querySelector('.xterm-rows > div'); const sp = document.elementFromPoint(60, 60); return JSON.stringify({fonts: [...document.fonts].map(f => f.weight + ':' + f.status), rows: document.querySelectorAll('.xterm-rows > div').length, first: r && r.textContent.slice(0, 40), top: sp && (sp.tagName + '.' + sp.className), screen: document.querySelector('.xterm-screen').getBoundingClientRect().width})"
        eval(t.web, js, "agent-dom") {
            self.eval(t.web, "return screenText().split('\\n').slice(-12).join('\\n')", "agent") {
                self.shot("5-agent") { self.after(4) { self.shot("6-agent-later") { self.finish() } } }
            }
        }
    }

    /// ARB_PROBE=drawer: the agent drawer: wait for the session, ask ARB_PROBE_PROMPT, answer
    /// every permission request with the page's primary button (allow once; a picture of the first), then the
    /// terminal tab and back.
    private func drawer() {
        shell.panelTab = "agent"
        shell.togglePanel()
        guard let idea = shell.current, let d = shell.drawers[idea] else { finish(); return }
        let text = "return document.getElementById('log').innerText.replace(/\\n+/g, ' | ').slice(-1500)"
        after(12) {
            self.eval(d.web, "return [...document.getElementById('mode').options].map(o => o.value + '=' + o.text).join(',') + ' / ' + document.getElementById('status').innerText", "drawer-modes") {
                self.shot("5-drawer") {
                    d.send(self.prompt ?? "用一句话说说这个 idea 在研究什么。")
                    self.waitDrawer(d, tries: 30, clicked: false) {
                        self.eval(d.web, text, "drawer") {
                            self.shot("6-drawer-answer") {
                                self.shell.switchTab("terminal")
                                self.after(3) { self.shot("7-terminal-tab") { self.shell.switchTab("agent"); self.after(1) { self.finish() } } }
                            }
                        }
                    }
                }
            }
        }
    }

    private func waitDrawer(_ d: Drawer, tries: Int, clicked: Bool, then: @escaping () -> Void) {
        let js = "const c = document.querySelector('.ask-card:not(.is-answered) .ab-btn-primary'); return JSON.stringify({ask: !!c, busy: !document.getElementById('stop').hidden})"
        d.web.callAsyncJavaScript(js, arguments: [:], in: nil, in: .page) { r in
            guard case .success(let v) = r, let s = v as? String, let o = try? JSONSerialization.jsonObject(with: Data(s.utf8)) as? [String: Bool] else {
                self.say("drawer wait: \(r)"); then(); return
            }
            self.say("drawer wait \(tries): \(s)")
            if o["ask"] == true {
                let allow = {
                    d.web.evaluateJavaScript("document.querySelector('.ask-card:not(.is-answered) .ab-btn-primary').click()")
                    self.say("drawer: allowed once")
                    self.after(2) { self.waitDrawer(d, tries: tries - 1, clicked: true, then: then) }
                }
                if clicked { allow() } else { self.shot("6-drawer-permission") { allow() } }
            } else if o["busy"] == true && tries > 0 {
                self.after(3) { self.waitDrawer(d, tries: tries - 1, clicked: clicked, then: then) }
            } else { then() }
        }
    }

    /// ARB_PROBE=pages: the compute page, then each run of the project (newest first, at most 3).
    private func pages() {
        say("key \(shell.approver?.fingerprint ?? "none") trusted \(shell.keyTrusted)")
        shell.chrome.evaluateJavaScript("arb.open({kind: 'compute', idea: null})")
        after(2) {
            self.eval(self.shell.chrome, "return document.getElementById('page-body').innerText.replace(/\\n+/g, ' | ').slice(0, 400)", "compute") {
                self.shot("7-compute") {
                    self.shell.arbd.call("runs") { r in
                        guard case .success(let v) = r, let runs = (v as? [String: Any])?["runs"] as? [[String: Any]] else { self.finish(); return }
                        self.runPages(runs.compactMap { $0["id"] as? String }.prefix(3).map { $0 })
                    }
                }
            }
        }
    }

    private func runPages(_ ids: [String]) {
        guard let id = ids.first else { finish(); return }
        shell.chrome.evaluateJavaScript("arb.open({kind: 'run', id: '\(id)'})")
        after(2) {
            self.eval(self.shell.chrome, "return document.getElementById('page-body').innerText.replace(/\\n+/g, ' | ').slice(0, 300)", id) {
                self.shot("8-\(id)") { self.runPages(Array(ids.dropFirst())) }
            }
        }
    }

    private func finish() { after(1) { NSApp.terminate(nil) } }
}
