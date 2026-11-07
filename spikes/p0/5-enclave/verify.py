"""P0 spike 5: verify the Secure Enclave signature in Python (what arbd would do before writing a Decision).

uv run --with cryptography python spikes/p0/5-enclave/verify.py PUB_B64 SIG_DER_B64 HASH
"""
import base64
import sys

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import ec

pub_b64, sig_b64, card_hash = sys.argv[1:4]
pub = ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), base64.b64decode(pub_b64))
sig = base64.b64decode(sig_b64)
for label, msg in [("real", card_hash), ("tampered", card_hash[:-1] + ("0" if card_hash[-1] != "0" else "1"))]:
    try:
        pub.verify(sig, msg.encode(), ec.ECDSA(hashes.SHA256()))
        print(label, "valid")
    except InvalidSignature:
        print(label, "INVALID")
