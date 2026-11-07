import CryptoKit
import Foundation
import LocalAuthentication

/// The approval key (P0 spike 5): a P-256 key in the Secure Enclave that needs user presence
/// (Touch ID or the password) for every signature. Its enclave-wrapped handle is a file;
/// it is useless off this Mac's Secure Enclave. The public key is saved for `arb key trust`
/// (the human pairs it in their own terminal, comparing the fingerprint shown here).
final class Approver {
    static let dir = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/Arboria", isDirectory: true)
    let key: SecureEnclave.P256.Signing.PrivateKey

    init() throws {
        let blob = Approver.dir.appendingPathComponent("approve-key.blob")
        let pubFile = Approver.dir.appendingPathComponent("approve-key.pub")
        try FileManager.default.createDirectory(at: Approver.dir, withIntermediateDirectories: true,
                                                attributes: [.posixPermissions: 0o700])
        if let data = try? Data(contentsOf: blob), let k = try? SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: data) {
            key = k
        } else {
            var err: Unmanaged<CFError>?
            guard let ac = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
                                                           [.privateKeyUsage, .userPresence], &err) else {
                throw ArbdError.message("cannot create the key's access control")
            }
            key = try SecureEnclave.P256.Signing.PrivateKey(accessControl: ac)
            try key.dataRepresentation.write(to: blob, options: .atomic)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: blob.path)
        }
        // always rewrite: what `arb key trust` reads is this key's, whatever else was there
        try (publicKey + "\n").write(to: pubFile, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: pubFile.path)
    }

    var publicKey: String { key.publicKey.x963Representation.base64EncodedString() }

    /// Same as approvekeys.fingerprint: sha256 of the X9.63 point, 12 hex digits in 3 groups.
    var fingerprint: String {
        let hex = SHA256.hash(data: key.publicKey.x963Representation).map { String(format: "%02x", $0) }.joined()
        let h = Array(hex.prefix(12))
        return [String(h[0..<4]), String(h[4..<8]), String(h[8..<12])].joined(separator: "-")
    }

    /// Sign `arboria-approve-v1\n<project>\n<run>\n<hash>`. The Touch ID prompt is drawn by
    /// the system and names the run, the estimate and the card hash: a web page cannot
    /// change what it says (ADR 0004, what you see is what you sign).
    func sign(project: String, run: String, cardHash: String, estSU: Double?,
              done: @escaping (Result<String, Error>) -> Void) {
        let su = estSU.map { "≈ \(Int($0.rounded())) SU，" } ?? ""
        let ctx = LAContext()
        ctx.localizedReason = "批准 \(run)：\(su)审核单 \(cardHash.prefix(8))"
        let message = Data("arboria-approve-v1\n\(project)\n\(run)\n\(cardHash)".utf8)
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let k = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: self.key.dataRepresentation,
                                                                   authenticationContext: ctx)
                let sig = try k.signature(for: message).derRepresentation.base64EncodedString()
                DispatchQueue.main.async { done(.success(sig)) }
            } catch {
                DispatchQueue.main.async { done(.failure(error)) }
            }
        }
    }
}
