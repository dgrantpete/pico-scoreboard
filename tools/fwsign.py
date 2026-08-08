#!/usr/bin/env python3
"""Ed25519 keygen and image signing for the Rust firmware's OTA pipeline.

This is the production counterpart of `firmware-rs/boot-spike/demo/sign.py`,
which signed throwaway keys for the bench. The scheme is identical because it
has to be — it is dictated by embassy-boot's ed25519 digest adapter
(`embassy-boot-0.7.0/src/digest_adapters/ed25519_dalek.rs`, reached through
`BlockingFirmwareUpdater::verify_and_mark_updated`):

    signature = Ed25519_sign(private, SHA512(image_bytes))

Plain Ed25519 over the 64-byte SHA-512 digest — NOT Ed25519ph, and not Ed25519
over the raw image. Getting this wrong produces a signature the device rejects
after a multi-minute download, so `selftest` below pins it against a committed
vector that `crates/scoreboard-ota/tests/signature.rs` checks from the Rust
side. If the two ever disagree, one of them fails loudly.

THE PRIVATE KEY IS NEVER COMMITTED. `keygen` writes it to a gitignored path and
prints the public half as a Rust literal to paste into
`firmware-rs/app/src/ota/key.rs`. The public key is the device's entire trust
root: an image signed by anything else is refused by the bootloader's updater,
which is what lets the whole OTA path run over plain HTTP.

Usage:
    python tools/fwsign.py keygen [--force]
        Generate a production keypair. Writes the private key to
        backend/.fw-signing-key (gitignored) and prints the public key.

    python tools/fwsign.py pubkey
        Print the public key of the configured private key, as hex and as the
        Rust literal the firmware expects.

    python tools/fwsign.py sign IMAGE.bin
        Print the detached signature over IMAGE.bin as 128 lowercase hex
        digits — the `signature` field of the /fw/manifest sidecar.

    python tools/fwsign.py selftest
        Verify the committed test vector. Needs no private key.

Requires: pip install cryptography
"""

import argparse
import hashlib
import sys
from pathlib import Path

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)

root_directory = Path(__file__).parent.parent

# Beside `.maxmind-key`, the repo's other deploy secret, and gitignored the
# same way. It is deliberately NOT under tools/ or firmware-rs/: those trees
# get archived, copied and shared as a unit, and a signing key should never
# ride along with one.
PRIVATE_KEY_PATH = root_directory / 'backend' / '.fw-signing-key'

# The committed cross-language test vector. The private half IS committed and
# that is safe by construction: it has signed nothing but these bytes and the
# firmware does not carry its public key, so a signature made with it is
# refused by every real device. Its only job is to make the Python signer and
# the Rust verifier compare notes.
VECTOR_DIR = root_directory / 'crates' / 'scoreboard-ota' / 'tests' / 'vector'


def _load_private(path: Path = PRIVATE_KEY_PATH) -> Ed25519PrivateKey:
    if not path.exists():
        sys.exit(
            f"No signing key at {path.relative_to(root_directory)}.\n"
            "  Generate one:  python tools/fwsign.py keygen\n"
            "  Or restore it from wherever the production key is kept — a NEW\n"
            "  key means every deployed device refuses every future image."
        )
    raw = path.read_bytes()
    if len(raw) != 32:
        sys.exit(f"{path} is {len(raw)} bytes; an ed25519 private key is 32.")
    return Ed25519PrivateKey.from_private_bytes(raw)


def _rust_literal(public: bytes) -> str:
    rows = []
    for start in range(0, 32, 8):
        row = ', '.join(f'0x{b:02x}' for b in public[start:start + 8])
        rows.append(f'    {row},')
    body = '\n'.join(rows)
    return f'pub const PUBLIC_KEY: [u8; 32] = [\n{body}\n];'


def sign_bytes(private: Ed25519PrivateKey, image: bytes) -> bytes:
    """The one line that has to match embassy-boot. See the module docstring."""
    return private.sign(hashlib.sha512(image).digest())


def verify_bytes(public: Ed25519PublicKey, image: bytes, signature: bytes) -> bool:
    try:
        public.verify(signature, hashlib.sha512(image).digest())
        return True
    except InvalidSignature:
        return False


def keygen(force: bool) -> int:
    if PRIVATE_KEY_PATH.exists() and not force:
        sys.exit(
            f"{PRIVATE_KEY_PATH.relative_to(root_directory)} already exists.\n"
            "  Overwriting it orphans every device running an image signed by\n"
            "  the old key: they will refuse every future update and can only\n"
            "  be recovered over USB. Pass --force if that is genuinely what\n"
            "  you want."
        )
    private = Ed25519PrivateKey.generate()
    PRIVATE_KEY_PATH.parent.mkdir(parents=True, exist_ok=True)
    PRIVATE_KEY_PATH.write_bytes(private.private_bytes_raw())
    public = private.public_key().public_bytes_raw()

    print(f"Wrote {PRIVATE_KEY_PATH.relative_to(root_directory)} (gitignored).")
    print()
    print("BACK THIS FILE UP SOMEWHERE OUTSIDE THIS REPOSITORY. It is the only")
    print("thing that can sign an update for every device that ships with the")
    print("public key below; losing it means every unit needs a USB flash.")
    print()
    print(f"Public key: {public.hex()}")
    print()
    print("Paste into firmware-rs/app/src/ota/key.rs:")
    print()
    print(_rust_literal(public))
    return 0


def pubkey() -> int:
    public = _load_private().public_key().public_bytes_raw()
    print(public.hex())
    print()
    print(_rust_literal(public))
    return 0


def sign(image_path: Path) -> int:
    if not image_path.exists():
        sys.exit(f"{image_path} does not exist")
    private = _load_private()
    image = image_path.read_bytes()
    signature = sign_bytes(private, image)
    # Round-trip locally before printing: a signature that does not verify
    # against its own public key means a broken key file, and finding that out
    # here costs nothing while finding it out on the device costs a download.
    if not verify_bytes(private.public_key(), image, signature):
        sys.exit("the signature did not verify against its own public key")
    print(signature.hex())
    return 0


def selftest() -> int:
    """Check the committed vector both ways round."""
    image = (VECTOR_DIR / 'image.bin').read_bytes()
    public = Ed25519PublicKey.from_public_bytes((VECTOR_DIR / 'public.bin').read_bytes())
    signature = bytes.fromhex((VECTOR_DIR / 'signature.hex').read_text().strip())

    if not verify_bytes(public, image, signature):
        sys.exit("FAIL: the committed signature does not verify. The signing "
                 "scheme has changed and the device will reject every image.")

    # And that a tampered image is rejected — a verifier that accepts
    # everything would pass the check above too.
    tampered = bytearray(image)
    tampered[0] ^= 0xFF
    if verify_bytes(public, bytes(tampered), signature):
        sys.exit("FAIL: a tampered image verified. The scheme is not binding "
                 "the image at all.")

    # Re-signing the vector with its own private key must reproduce the
    # committed signature exactly. Ed25519 is deterministic, so this pins the
    # signer rather than merely the verifier.
    private = Ed25519PrivateKey.from_private_bytes((VECTOR_DIR / 'private.bin').read_bytes())
    if sign_bytes(private, image) != signature:
        sys.exit("FAIL: re-signing the vector produced different bytes. The "
                 "signer has changed.")

    print(f"OK: vector verifies, tampering is rejected, signing is reproducible "
          f"({len(image)} byte image, sha512 "
          f"{hashlib.sha512(image).hexdigest()[:16]}...)")
    return 0


def _make_vector() -> int:
    """Regenerate the committed test vector. Run once, by hand, ever."""
    import os
    VECTOR_DIR.mkdir(parents=True, exist_ok=True)
    private = Ed25519PrivateKey.generate()
    image = bytes(os.urandom(4096))
    signature = sign_bytes(private, image)
    (VECTOR_DIR / 'private.bin').write_bytes(private.private_bytes_raw())
    (VECTOR_DIR / 'public.bin').write_bytes(private.public_key().public_bytes_raw())
    (VECTOR_DIR / 'image.bin').write_bytes(image)
    (VECTOR_DIR / 'signature.hex').write_text(signature.hex() + '\n')
    print(f"wrote a new vector into {VECTOR_DIR.relative_to(root_directory)}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    subparsers = parser.add_subparsers(dest='command', required=True)

    keygen_parser = subparsers.add_parser('keygen', help='generate a production keypair')
    keygen_parser.add_argument('--force', action='store_true',
                               help='overwrite an existing key (orphans every deployed device)')

    subparsers.add_parser('pubkey', help='print the configured key\'s public half')

    sign_parser = subparsers.add_parser('sign', help='sign an image, printing hex')
    sign_parser.add_argument('image', type=Path)

    subparsers.add_parser('selftest', help='check the committed test vector')
    subparsers.add_parser('make-vector', help=argparse.SUPPRESS)

    args = parser.parse_args()
    match args.command:
        case 'keygen':
            return keygen(args.force)
        case 'pubkey':
            return pubkey()
        case 'sign':
            return sign(args.image)
        case 'selftest':
            return selftest()
        case 'make-vector':
            return _make_vector()
    return 1


if __name__ == '__main__':
    sys.exit(main())
