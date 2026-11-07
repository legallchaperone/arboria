"""Approval keys: the Mac app's Touch ID signatures (ADR 0004, P0 spike 5).

The app holds a P-256 key in the Secure Enclave (user presence required for every
signature) and signs one exact message per approval:

    arboria-approve-v1\\n<project id>\\n<run id>\\n<card hash>

arbd verifies it against the keys trusted here before recording the human's approval
Decision (`submission.approve_signed`). Keys are per device, trusted only in the human's
own terminal (`arb key trust`, which shows the key's fingerprint for the human to compare
with the one the app shows, and to type). They live in `$ARBORIA_CONFIG_DIR/approve-keys.json`
(0600).

Residual risk, stated plainly: like everything else a user owns, that file can be written
by any process running as the user, Claude Code's Bash included, until the Claude Code
sandbox (`denyWrite`, ADR 0004 P4) covers it. `arb install-claude` denies the `arb key`
commands and editing the file with Claude's file tools.
"""

from __future__ import annotations

import base64
import binascii
import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from . import human
from .clusters import config_dir
from .errors import ArbError

KEYS_FILE = "approve-keys.json"
MESSAGE_VERSION = "arboria-approve-v1"
# where the app leaves its public key for `arb key trust` (macOS)
APP_PUBLIC_KEY = Path("~/Library/Application Support/Arboria/approve-key.pub")


def keys_path() -> Path:
    return config_dir() / KEYS_FILE


def message(project: str, run_id: str, card_hash: str) -> bytes:
    for part in (project, run_id, card_hash):
        if not part or "\n" in part:
            raise ArbError("approval message parts must be non-empty single lines")
    return f"{MESSAGE_VERSION}\n{project}\n{run_id}\n{card_hash}".encode()


def _public_key_bytes(pub_b64: str) -> bytes:
    from cryptography.hazmat.primitives.asymmetric import ec

    try:
        raw = base64.b64decode(pub_b64.strip(), validate=True)
        ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), raw)
    except (binascii.Error, ValueError) as exc:
        raise ArbError(f"not a P-256 public key (base64 X9.63): {exc}") from None
    return raw


def fingerprint(pub_b64: str) -> str:
    """12 hex digits in three groups, what the app shows next to its key."""
    digest = hashlib.sha256(_public_key_bytes(pub_b64)).hexdigest()[:12]
    return "-".join(digest[i:i + 4] for i in (0, 4, 8))


def load() -> list[dict[str, Any]]:
    path = keys_path()
    if not path.is_file():
        return []
    try:
        data = json.loads(path.read_text())
        keys = data["keys"]
        assert isinstance(keys, list)
    except (ValueError, KeyError, AssertionError, OSError) as exc:
        raise ArbError(f"{path} is not a valid approval key file: {exc}") from None
    return keys


def _save(keys: list[dict[str, Any]]) -> None:
    path = keys_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as fh:
        json.dump({"keys": keys}, fh, ensure_ascii=False, indent=2)
    os.replace(tmp, path)
    os.chmod(path, 0o600)


def trust(pub_b64: str, label: str) -> dict[str, Any]:
    """Human only: show the fingerprint, the human types it, the key is trusted."""
    human.require_terminal("arb key trust")
    fp = fingerprint(pub_b64)
    keys = load()
    if any(k["fingerprint"] == fp for k in keys):
        raise ArbError(f"key {fp} is already trusted")
    print(f"要信任的签字密钥：{fp}（{label}）\n确认 Arboria app 里显示的是同一串。")
    human.confirm_typed(fp)
    entry = {"fingerprint": fp, "public_key": pub_b64.strip(), "label": label,
             "added_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}
    _save(keys + [entry])
    return entry


def revoke(fp: str) -> dict[str, Any]:
    human.require_terminal("arb key revoke")
    keys = load()
    hit = next((k for k in keys if k["fingerprint"] == fp), None)
    if hit is None:
        raise ArbError(f"no trusted key {fp}")
    human.confirm_typed(fp)
    _save([k for k in keys if k["fingerprint"] != fp])
    return hit


def verify(project: str, run_id: str, card_hash: str, signature_b64: str) -> str:
    """The fingerprint of the trusted key that signed this exact approval; else ArbError."""
    from cryptography.exceptions import InvalidSignature
    from cryptography.hazmat.primitives import hashes
    from cryptography.hazmat.primitives.asymmetric import ec

    keys = load()
    if not keys:
        raise ArbError("no trusted approval key: pair the app first (`arb key trust` in your "
                       "own terminal)")
    try:
        sig = base64.b64decode(signature_b64, validate=True)
    except binascii.Error:
        raise ArbError("signature is not base64") from None
    msg = message(project, run_id, card_hash)
    for k in keys:
        pub = ec.EllipticCurvePublicKey.from_encoded_point(
            ec.SECP256R1(), _public_key_bytes(k["public_key"]))
        try:
            pub.verify(sig, msg, ec.ECDSA(hashes.SHA256()))
        except InvalidSignature:
            continue
        return k["fingerprint"]
    raise ArbError(f"the signature for {run_id} does not match any trusted approval key")
