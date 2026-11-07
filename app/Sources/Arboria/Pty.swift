import Foundation
import Darwin

/// A child process on a pseudo-terminal (P0 spike 4): openpty + posix_spawn with
/// POSIX_SPAWN_SETSID, the slave re-opened in the child so it is the controlling tty.
final class Pty {
    private(set) var master: Int32 = -1
    private(set) var pid: pid_t = 0
    private var source: DispatchSourceRead?
    var onOutput: ((Data) -> Void)?
    var onExit: (() -> Void)?

    init(command: String, cwd: String, cols: UInt16, rows: UInt16) throws {
        var m: Int32 = 0, s: Int32 = 0
        var ws = winsize(ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0)
        guard openpty(&m, &s, nil, nil, &ws) == 0 else { throw ArbdError.message("openpty failed") }
        let slavePath = String(cString: ptsname(m))
        close(s)
        master = m
        var fa: posix_spawn_file_actions_t? = nil
        posix_spawn_file_actions_init(&fa); defer { posix_spawn_file_actions_destroy(&fa) }
        posix_spawn_file_actions_addopen(&fa, 0, slavePath, O_RDWR, 0)
        posix_spawn_file_actions_adddup2(&fa, 0, 1)
        posix_spawn_file_actions_adddup2(&fa, 0, 2)
        posix_spawn_file_actions_addclose(&fa, m)
        posix_spawn_file_actions_addchdir_np(&fa, cwd)
        var at: posix_spawnattr_t? = nil
        posix_spawnattr_init(&at); defer { posix_spawnattr_destroy(&at) }
        posix_spawnattr_setflags(&at, Int16(POSIX_SPAWN_SETSID))
        var env = cleanEnvironment()
        env["TERM"] = "xterm-256color"; env["COLORTERM"] = "truecolor"
        let args: [String] = ["/bin/zsh", "-lc", command]
        let argv: [UnsafeMutablePointer<CChar>?] = args.map { strdup($0) } + [nil]
        let envp: [UnsafeMutablePointer<CChar>?] = env.map { strdup("\($0.key)=\($0.value)") } + [nil]
        defer { argv.forEach { free($0) }; envp.forEach { free($0) } }
        let rc = posix_spawn(&pid, "/bin/zsh", &fa, &at, argv, envp)
        if rc != 0 { close(m); throw ArbdError.message("posix_spawn failed: \(rc)") }
        let src = DispatchSource.makeReadSource(fileDescriptor: m, queue: .main)
        src.setEventHandler { [weak self] in
            var buf = [UInt8](repeating: 0, count: 65536)
            let n = read(m, &buf, buf.count)
            if n <= 0 { self?.source?.cancel(); self?.onExit?(); return }
            self?.onOutput?(Data(buf[0..<n]))
        }
        source = src
        src.resume()
    }

    func write(_ s: String) {
        _ = s.utf8CString.withUnsafeBufferPointer { Darwin.write(master, $0.baseAddress, $0.count - 1) }
    }

    func resize(cols: UInt16, rows: UInt16) {
        var ws = winsize(ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0)
        _ = ioctl(master, TIOCSWINSZ, &ws)
    }

    func terminate() {
        if pid > 0 { kill(pid, SIGHUP) }
        source?.cancel()
        if master >= 0 { close(master); master = -1 }
    }
}
