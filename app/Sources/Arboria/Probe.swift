import AppKit
import WebKit

/// ARB_PROBE=1: drive the app headlessly for verification (like the P0 spikes). Selects up to
/// two open ideas, records what each pane shows (text + PNG in ARB_PROBE_OUT), optionally
/// types ARB_PROBE_PROMPT into the first idea's Claude terminal, then quits.
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

    private func snap(_ w: WKWebView, _ name: String, then: @escaping () -> Void) {
        w.takeSnapshot(with: nil) { img, _ in
            if let img, let t = img.tiffRepresentation, let png = NSBitmapImageRep(data: t)?.representation(using: .png, properties: [:]) {
                let p = (self.out as NSString).appendingPathComponent(name + ".png")
                try? png.write(to: URL(fileURLWithPath: p)); self.say("snapshot \(p)")
            }
            then()
        }
    }

    func run() {
        after(3) {
            self.eval(self.shell.sidebar, "return document.body.innerText", "sidebar") {
                self.shell.arbd.call("project") { r in
                    guard case .success(let v) = r, let p = v as? [String: Any], let ideas = p["ideas"] as? [[String: Any]] else {
                        self.say("project failed: \(r)"); NSApp.terminate(nil); return
                    }
                    let names = ideas.filter { $0["has_notebook"] as? Bool == true }.compactMap { $0["name"] as? String }
                    self.say("openable ideas: \(names)")
                    self.visit(Array(names.prefix(2)), index: 0)
                }
            }
        }
    }

    private func visit(_ names: [String], index: Int) {
        guard index < names.count else { after(1) { NSApp.terminate(nil) }; return }
        let idea = names[index]
        say("select \(idea)")
        shell.select(idea) { url in
            self.say("notebook url for \(idea): \(url == nil ? "none" : "ok")")
            self.after(10) {
                let js = """
                return JSON.stringify({url: location.pathname, title: document.title,
                  cells: document.querySelectorAll('[data-cell-id]').length,
                  injected: !!document.querySelector('link[href="arboria://app/marimo.css"]'),
                  bodyFont: getComputedStyle(document.body).fontFamily,
                  text: document.body.innerText.replace(/\\s+/g,' ').slice(0, 160)})
                """
                self.eval(self.shell.notebook, js, "notebook[\(idea)]") {
                    let term = self.shell.terminals[idea]!.web
                    let typeThen: (@escaping () -> Void) -> Void = { next in
                        guard index == 0, let p = self.prompt else { next(); return }
                        // A new worktree gets Claude Code's "trust this folder?" question first;
                        // the probe answers it with Enter (the human does this in real use).
                        self.say("typing prompt into \(idea)'s terminal")
                        self.shell.terminals[idea]!.send("\u{1b}[B")  // ↓ to "Yes, I trust this folder"
                        self.after(0.5) { self.shell.terminals[idea]!.send("\r") }
                        self.after(4) { self.shell.terminals[idea]!.send(p) }
                        self.after(4.5) { self.shell.terminals[idea]!.send("\r"); self.after(30, next) }
                    }
                    typeThen {
                        self.eval(term, "return screenText().split('\\n').slice(-14).join('\\n')", "terminal[\(idea)]") {
                            self.snap(self.shell.sidebar, "sidebar-\(index)") {
                                self.snap(self.shell.notebook, "notebook-\(index)") {
                                    self.snap(term, "terminal-\(index)") { self.visit(names, index: index + 1) }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
