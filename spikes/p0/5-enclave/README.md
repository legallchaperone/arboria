# P0 ⑤：Secure Enclave 签名、Python 验签、Touch ID 弹窗文字

```
spikes/p0/5-enclave/build.sh
B=spikes/p0/5-enclave/build/Enclave.app/Contents/MacOS/Enclave; export SPIKE_DIR=spikes/p0/5-enclave/build
$B keygen-cryptokit            # 生成 .userPresence 的 P-256 密钥，存被 SE 包裹的 blob
$B sign r031 12.5 <card-hash>  # 弹 Touch ID，输出 DER 签名
uv run --with cryptography python spikes/p0/5-enclave/verify.py "$($B pub)" <sig> <card-hash>
```

## 结论（2026-11-06，Mac17,3，macOS 26.6.2，只用 Command Line Tools）
| 项 | 结果 |
|---|---|
| 钥匙串里的永久 SE 密钥（`SecKeyCreateRandomKey` + `kSecAttrIsPermanent`） | ❌ ad-hoc 签名报 `-34018`（errSecMissingEntitlement）：要 `keychain-access-groups` entitlement，也就是要 Apple 开发者证书 + provisioning。 |
| CryptoKit `SecureEnclave.P256.Signing.PrivateKey` | ✅ ad-hoc 签名就能用。私钥不出 SE，`dataRepresentation`（427 B）是 SE 加密过的句柄，只在这台机器的 SE 里能用，存成普通文件即可，不需要钥匙串。access control：`[.privateKeyUsage, .userPresence]`，`WhenUnlockedThisDeviceOnly`。 |
| Touch ID 弹窗 | ✅ 签名时弹出、按指纹后出签名。`LAContext.localizedReason` 设为“批准 r031：预估 12.5 SU，审核单 xxxxxxxx”，通过 `authenticationContext:` 传给私钥。弹窗截图没拿到（`screencapture` 没有屏幕录制权限），**弹窗里实际显示的文字要人确认一次**。 |
| Python 验签 | ✅ `cryptography` 用 X9.63 公钥 + DER 签名 + ECDSA/SHA-256 验过；改一个字符的哈希验不过。 |

### 对计划的修改
- 密钥走 CryptoKit + 文件里的 SE blob，不走钥匙串：不需要开发者证书就能用（以后正式签名发布时也可以继续这样）。blob 偷走也没用（离不开这台机器的 SE，每次用都要 Touch ID）。
- 公钥由 arbd 记录（建议在账本里，第一次配对时由人在终端确认指纹），之后只认这把。
- **Python 核心要新增运行时依赖 `cryptography`**（标准库没有 ECDSA）。这次只在 spike 里用 `uv run --with`，没有加进 `pyproject.toml`，等人点头。
- 签名的消息是审核单哈希的十六进制字符串；正式做时消息里要再带上 run id 和用途（例如 `arboria-approve-v1\n<project>\n<run>\n<hash>`），防止签名被挪用。
