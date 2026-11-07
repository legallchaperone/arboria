import Foundation

/// The Python core: `arb arbd --socket S --project P`, spoken to as newline-delimited JSON.
/// arbd exits when our pipe to its stdin closes, i.e. when the app goes away.
final class Arbd {
    let socketPath: String
    private let proc = Process()
    private let stdinPipe = Pipe()
    private var fd: Int32 = -1
    private var buffer = Data()
    private var nextId = 0
    private let queue = DispatchQueue(label: "arbd")

    init(project: String) throws {
        // macOS caps Unix socket paths at 104 bytes: keep it short and private to us.
        let dir = FileManager.default.temporaryDirectory
        socketPath = dir.appendingPathComponent("arbd-\(getpid()).sock").path
        let cmd = ProcessInfo.processInfo.environment["ARBORIA_ARBD_CMD"] ?? "arb"
        proc.executableURL = URL(fileURLWithPath: "/bin/zsh")
        // A login shell finds the user's `arb` (and uv, marimo) on their PATH.
        proc.arguments = ["-lc", "exec \(cmd) arbd --socket \(shellQuote(socketPath)) --project \(shellQuote(project))"]
        proc.environment = cleanEnvironment()
        let out = Pipe()
        proc.standardInput = stdinPipe
        proc.standardOutput = out
        try proc.run()
        // First stdout line is {"ready": true, ...}; anything else is an error message.
        let first = Arbd.readLine(out.fileHandleForReading)
        guard let data = first.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any], obj["ready"] as? Bool == true else {
            throw ArbdError.message("arbd did not start: \(first.isEmpty ? "exited with \(proc.terminationStatus)" : first)")
        }
        try connect()
    }

    private static func readLine(_ h: FileHandle) -> String {
        var line = Data()
        while true {
            let b = h.readData(ofLength: 1)
            if b.isEmpty || b == Data([0x0a]) { break }
            line.append(b)
        }
        return String(decoding: line, as: UTF8.self)
    }

    private func connect() throws {
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            socketPath.utf8CString.withUnsafeBytes { src in raw.copyMemory(from: UnsafeRawBufferPointer(rebasing: src.prefix(raw.count))) }
        }
        let rc = withUnsafePointer(to: &addr) { p in
            p.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        if rc != 0 { throw ArbdError.message("cannot connect to arbd at \(socketPath): errno \(errno)") }
    }

    /// Call a method off the main thread; the completion runs on the main thread.
    func call(_ method: String, _ params: [String: Any] = [:], done: @escaping (Result<Any, ArbdError>) -> Void) {
        queue.async {
            let r = self.callSync(method, params)
            DispatchQueue.main.async { done(r) }
        }
    }

    private func callSync(_ method: String, _ params: [String: Any]) -> Result<Any, ArbdError> {
        nextId += 1
        let id = nextId
        guard var line = try? JSONSerialization.data(withJSONObject: ["id": id, "method": method, "params": params]) else {
            return .failure(.message("cannot encode request"))
        }
        line.append(0x0a)
        let sent = line.withUnsafeBytes { write(fd, $0.baseAddress, $0.count) }
        if sent != line.count { return .failure(.message("arbd went away")) }
        while true {
            if let nl = buffer.firstIndex(of: 0x0a) {
                let msg = buffer[buffer.startIndex..<nl]
                buffer.removeSubrange(buffer.startIndex...nl)
                guard let obj = try? JSONSerialization.jsonObject(with: msg) as? [String: Any] else { continue }
                if obj["id"] as? Int != id { continue }
                if let err = obj["error"] as? [String: Any] { return .failure(.message(err["message"] as? String ?? "error")) }
                return .success(obj["result"] ?? NSNull())
            }
            var chunk = [UInt8](repeating: 0, count: 65536)
            let n = read(fd, &chunk, chunk.count)
            if n <= 0 { return .failure(.message("arbd went away")) }
            buffer.append(contentsOf: chunk[0..<n])
        }
    }

    func stop() {
        try? stdinPipe.fileHandleForWriting.close()  // arbd stops its marimo servers and exits
        if fd >= 0 { close(fd) }
    }
}

enum ArbdError: Error, CustomStringConvertible {
    case message(String)
    var description: String { if case .message(let m) = self { return m }; return "" }
}

func shellQuote(_ s: String) -> String { "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'" }

/// Our environment minus any Claude Code session we may have been started from
/// (CLAUDE_CODE_CHILD_SESSION turns transcripts off, so --resume would break).
func cleanEnvironment() -> [String: String] {
    ProcessInfo.processInfo.environment.filter { $0.key != "CLAUDECODE" && !$0.key.hasPrefix("CLAUDE_CODE_") && !$0.key.hasPrefix("ARB_PROBE") }
}
