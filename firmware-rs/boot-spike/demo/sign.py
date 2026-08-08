#!/usr/bin/env python3
"""Dev-only Ed25519 keygen + image signing for the boot spike.

THROWAWAY KEYS ONLY. The private key is gitignored and can be regenerated at
will; nothing produced here may ever sign a production image. The production
signing path (SPEC.md par.8) lives in backend deploy secrets + a tools/
script, and gets its own key.

Signature scheme — must match embassy-boot's ed25519-dalek adapter
(embassy-boot-0.7.0/src/digest_adapters/ed25519_dalek.rs + the
verify_and_mark_updated flow): the 64-byte detached signature is plain
Ed25519 over the 64-byte SHA-512 digest of the image bytes:

    sig = Ed25519_sign(priv, SHA512(image))

Usage:
    python sign.py keygen             writes keys/dev_priv.bin + keys/dev_pub.bin
    python sign.py sign IMAGE.bin     writes IMAGE.bin.sig next to the image
"""

import hashlib
import sys
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
)

KEYS = Path(__file__).parent / "keys"
PRIV = KEYS / "dev_priv.bin"
PUB = KEYS / "dev_pub.bin"


def keygen() -> None:
    KEYS.mkdir(exist_ok=True)
    key = Ed25519PrivateKey.generate()
    PRIV.write_bytes(key.private_bytes_raw())
    PUB.write_bytes(key.public_key().public_bytes_raw())
    print(f"wrote {PRIV} (gitignored) and {PUB} (committed)")


def sign(image_path: Path) -> None:
    if not PRIV.exists():
        sys.exit(f"{PRIV} missing - run `python sign.py keygen` first")
    key = Ed25519PrivateKey.from_private_bytes(PRIV.read_bytes())
    message = hashlib.sha512(image_path.read_bytes()).digest()
    sig = key.sign(message)
    key.public_key().verify(sig, message)  # sanity: round-trips locally
    out = image_path.with_suffix(image_path.suffix + ".sig")
    out.write_bytes(sig)
    print(f"wrote {out} ({len(sig)} bytes)")


def main() -> None:
    match sys.argv[1:]:
        case ["keygen"]:
            keygen()
        case ["sign", image]:
            sign(Path(image))
        case _:
            sys.exit(__doc__)


if __name__ == "__main__":
    main()
