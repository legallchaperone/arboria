// P0 spike 5: Secure Enclave P-256 key with .userPresence, signs a card hash; Touch ID prompt
// carries run id / SU / hash prefix in localizedReason (drawn by the system, not by any web page).
import Foundation
import CryptoKit
import LocalAuthentication
import Security

let args = CommandLine.arguments
let dir = URL(fileURLWithPath: ProcessInfo.processInfo.environment["SPIKE_DIR"] ?? FileManager.default.currentDirectoryPath)
let blob = dir.appendingPathComponent("se-key.blob")
func out(_ s: String) { print(s); fflush(stdout) }

func accessControl() -> SecAccessControl {
    var err: Unmanaged<CFError>?
    let ac = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, [.privateKeyUsage, .userPresence], &err)!
    return ac
}

switch args.count > 1 ? args[1] : "" {
case "keygen-cryptokit":
    // Route A: CryptoKit; persist the enclave-wrapped key blob in a file (no keychain entitlement).
    do {
        let k = try SecureEnclave.P256.Signing.PrivateKey(accessControl: accessControl())
        try k.dataRepresentation.write(to: blob)
        out("ok blob=\(k.dataRepresentation.count)B pub(x963,b64)=\(k.publicKey.x963Representation.base64EncodedString())")
    } catch { out("keygen-cryptokit failed: \(error)") }
case "keygen-keychain":
    // Route B: permanent key in the keychain (needs keychain-access-groups entitlement?).
    let attrs: [String: Any] = [
        kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom, kSecAttrKeySizeInBits as String: 256,
        kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave,
        kSecPrivateKeyAttrs as String: [kSecAttrIsPermanent as String: true, kSecAttrApplicationTag as String: "world.cwx.arboria.p0.approve".data(using: .utf8)!,
                                        kSecAttrAccessControl as String: accessControl()]]
    var err: Unmanaged<CFError>?
    if let k = SecKeyCreateRandomKey(attrs as CFDictionary, &err) { out("ok keychain key \(k)") }
    else { out("keygen-keychain failed: \(err!.takeRetainedValue())") }
case "pub":
    let k = try! SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: Data(contentsOf: blob))
    out(k.publicKey.x963Representation.base64EncodedString())
case "sign":
    // sign <run> <su> <card-hash-hex>
    let (run, su, hash) = (args[2], args[3], args[4])
    let ctx = LAContext()
    ctx.localizedReason = "批准 \(run)：预估 \(su) SU，审核单 \(hash.prefix(8))"
    do {
        let k = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: Data(contentsOf: blob), authenticationContext: ctx)
        let sig = try k.signature(for: Data(hash.utf8))  // SHA-256 over the hash string, ECDSA
        out("sig(der,b64)=\(sig.derRepresentation.base64EncodedString())")
    } catch { out("sign failed: \(error)") }
default:
    out("usage: keygen-cryptokit | keygen-keychain | pub | sign RUN SU HASH")
}
