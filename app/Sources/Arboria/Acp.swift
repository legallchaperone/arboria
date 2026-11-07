import Foundation

/// The agent drawer's connection to the user's own `claude` (ADR 0004, P0 spike 6): the
/// pinned `claude-agent-acp` adapter (in the bundle, installed without its bundled claude)
/// on stdio, newline-delimited JSON-RPC. `CLAUDE_CODE_EXECUTABLE` points it at the `claude`
/// on the user's login-shell PATH, so the user's login, settings and the project's hooks,
/// permission rules and sandbox apply as in their terminal.
///
/// What this side enforces, whatever the page asks: never the bypassPermissions mode (the
/// session is created with bypass disallowed, a mode list without it, a refused
/// `set_mode`, and permission options that would switch to it are dropped); a permission
/// answer only for a request that is pending and with one of its own options; no file or
/// terminal access for the agent through us (we advertise neither capability).
final class Acp {
    enum Event {
        case status(String)
        case session(modes: [[String: Any]], current: String)
        case update([String: Any])
        case permission(id: Int, toolCall: [String: Any], options: [[String: Any]])
        case done(stopReason: String, usage: [String: Any])
        case failed(String)
        case exited
    }

    static let forbiddenMode = "bypassPermissions"
    let cwd: String
    var onEvent: ((Event) -> Void)?
    private(set) var sessionId: String?
    private(set) var busy = false
    private let proc = Process()
    private let input = Pipe(), output = Pipe()
    private var buffer = Data()
    private var nextId = 0
    private var replies: [Int: (Result<Any, ArbdError>) -> Void] = [:]
    private var permissions: [Int: Set<String>] = [:]  // pending request id -> its option ids

    /// The adapter's entry point in the bundle (app/build.sh), or ARBORIA_ACP_CMD for tests.
    static func command() -> String? {
        let env = ProcessInfo.processInfo.environment
        if let c = env["ARBORIA_ACP_CMD"] { return c }
        guard let res = Bundle.main.resourcePath else { return nil }
        let js = res + "/acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"
        guard FileManager.default.fileExists(atPath: js) else { return nil }
        return "exec node " + shellQuote(js)
    }

    init(cwd: String) throws {
        self.cwd = cwd
        guard let cmd = Acp.command() else { throw ArbdError.message("app 里没有 agent 适配器（重新运行 app/build.sh）") }
        proc.executableURL = URL(fileURLWithPath: "/bin/zsh")
        // the login shell's PATH finds the user's node and claude, as in their terminal
        proc.arguments = ["-lc", "export CLAUDE_CODE_EXECUTABLE=\"$(command -v claude)\"; "
                          + "[ -n \"$CLAUDE_CODE_EXECUTABLE\" ] || { echo 'claude is not on PATH' >&2; exit 127; }; " + cmd]
        proc.environment = cleanEnvironment()
        proc.currentDirectoryURL = URL(fileURLWithPath: cwd)
        proc.standardInput = input
        proc.standardOutput = output
        let errLog = (NSTemporaryDirectory() as NSString).appendingPathComponent("arboria-acp.log")
        FileManager.default.createFile(atPath: errLog, contents: nil)
        proc.standardError = FileHandle(forWritingAtPath: errLog)
        proc.terminationHandler = { [weak self] _ in DispatchQueue.main.async { self?.ended() } }
        output.fileHandleForReading.readabilityHandler = { [weak self] h in
            let d = h.availableData
            DispatchQueue.main.async { self?.received(d) }
        }
        try proc.run()
    }

    // MARK: lifecycle

    func start() {
        onEvent?(.status("正在启动 agent…"))
        request("initialize", ["protocolVersion": 1,
                               "clientCapabilities": ["fs": ["readTextFile": false, "writeTextFile": false], "terminal": false],
                               "clientInfo": ["name": "arboria", "version": "0.1"]]) { [weak self] r in
            guard let self else { return }
            if case .failure(let e) = r { self.onEvent?(.failed("agent 没有启动：\(e)")); return }
            self.request("session/new", ["cwd": self.cwd, "mcpServers": [],
                                         "_meta": ["claudeCode": ["options": ["allowDangerouslySkipPermissions": false]]]]) { r in
                switch r {
                case .failure(let e): self.onEvent?(.failed("会话没有建立：\(e)"))
                case .success(let v):
                    guard let o = v as? [String: Any], let sid = o["sessionId"] as? String else {
                        self.onEvent?(.failed("会话没有建立")); return
                    }
                    self.sessionId = sid
                    let m = o["modes"] as? [String: Any] ?? [:]
                    let modes = (m["availableModes"] as? [[String: Any]] ?? []).filter { ($0["id"] as? String) != Acp.forbiddenMode }
                    var current = m["currentModeId"] as? String ?? "default"
                    if current == Acp.forbiddenMode {  // belt and braces: the adapter should not allow it
                        current = "default"
                        self.request("session/set_mode", ["sessionId": sid, "modeId": current]) { _ in }
                    }
                    self.onEvent?(.session(modes: modes, current: current))
                }
            }
        }
    }

    func stop() {
        output.fileHandleForReading.readabilityHandler = nil
        if proc.isRunning { proc.terminate() }
    }

    private func ended() {
        for (_, f) in replies { f(.failure(.message("agent 已退出"))) }
        replies = [:]; permissions = [:]; busy = false
        onEvent?(.exited)
    }

    // MARK: what the page may do

    func prompt(_ text: String) {
        guard let sid = sessionId, !busy, !text.isEmpty else { return }
        busy = true
        request("session/prompt", ["sessionId": sid, "prompt": [["type": "text", "text": text]]]) { [weak self] r in
            guard let self else { return }
            self.busy = false
            switch r {
            case .success(let v):
                let o = v as? [String: Any] ?? [:]
                self.onEvent?(.done(stopReason: o["stopReason"] as? String ?? "end_turn", usage: o["usage"] as? [String: Any] ?? [:]))
            case .failure(let e): self.onEvent?(.failed("\(e)"))
            }
        }
    }

    func cancel() {
        guard let sid = sessionId else { return }
        for id in permissions.keys { answer(id, option: nil) }  // a pending question is answered "cancelled"
        send(["jsonrpc": "2.0", "method": "session/cancel", "params": ["sessionId": sid]])
    }

    func setMode(_ id: String) {
        guard let sid = sessionId, id != Acp.forbiddenMode else { return }
        request("session/set_mode", ["sessionId": sid, "modeId": id]) { [weak self] r in
            if case .failure(let e) = r { self?.onEvent?(.failed("没有切换模式：\(e)")) }
        }
    }

    /// The person's answer to a permission request: one of that request's options, or nil.
    func answer(_ id: Int, option: String?) {
        guard let allowed = permissions.removeValue(forKey: id) else { return }
        let outcome: [String: Any]
        if let o = option, allowed.contains(o) { outcome = ["outcome": "selected", "optionId": o] }
        else { outcome = ["outcome": "cancelled"] }
        send(["jsonrpc": "2.0", "id": id, "result": ["outcome": outcome]])
    }

    // MARK: JSON-RPC

    private func request(_ method: String, _ params: [String: Any], _ done: @escaping (Result<Any, ArbdError>) -> Void) {
        nextId += 1
        replies[nextId] = done
        send(["jsonrpc": "2.0", "id": nextId, "method": method, "params": params])
    }

    private func send(_ obj: [String: Any]) {
        guard proc.isRunning, var d = try? JSONSerialization.data(withJSONObject: obj) else { return }
        d.append(0x0a)
        try? input.fileHandleForWriting.write(contentsOf: d)
    }

    private func received(_ d: Data) {
        buffer.append(d)
        while let nl = buffer.firstIndex(of: 0x0a) {
            let line = buffer.subdata(in: buffer.startIndex..<nl)
            buffer.removeSubrange(buffer.startIndex...nl)
            guard !line.isEmpty, let msg = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any] else { continue }
            handle(msg)
        }
    }

    private func handle(_ msg: [String: Any]) {
        let method = msg["method"] as? String
        if method == nil, let id = msg["id"] as? Int, let f = replies.removeValue(forKey: id) {
            if let e = msg["error"] as? [String: Any] { f(.failure(.message(e["message"] as? String ?? "error"))) }
            else { f(.success(msg["result"] ?? NSNull())) }
            return
        }
        let params = msg["params"] as? [String: Any] ?? [:]
        switch method {
        case "session/update":
            if let u = params["update"] as? [String: Any] {
                if u["sessionUpdate"] as? String == "current_mode_update", u["currentModeId"] as? String == Acp.forbiddenMode {
                    setMode("default")
                    return
                }
                onEvent?(.update(u))
            }
        case "session/request_permission":
            guard let id = msg["id"] as? Int else { return }
            let options = (params["options"] as? [[String: Any]] ?? []).filter { o in
                let text = "\(o["optionId"] ?? "") \(o["name"] ?? "")".lowercased()
                return !text.contains("bypass")
            }
            permissions[id] = Set(options.compactMap { $0["optionId"] as? String })
            onEvent?(.permission(id: id, toolCall: params["toolCall"] as? [String: Any] ?? [:], options: options))
        default:
            if let id = msg["id"] { send(["jsonrpc": "2.0", "id": id, "error": ["code": -32601, "message": "not supported by Arboria"]]) }
        }
    }
}
