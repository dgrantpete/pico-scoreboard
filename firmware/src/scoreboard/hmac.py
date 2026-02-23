"""
HMAC-SHA256 implementation for MicroPython.

MicroPython on RP2040 doesn't include the `hmac` module,
so this implements RFC 2104 HMAC using uhashlib.sha256.
"""

import uhashlib


def hmac_sha256(key: bytes, message: bytes) -> bytes:
    """
    Compute HMAC-SHA256.

    Args:
        key: Secret key bytes
        message: Message to authenticate

    Returns:
        32-byte HMAC digest
    """
    block_size = 64  # SHA-256 block size

    # Hash key if longer than block size
    if len(key) > block_size:
        key = uhashlib.sha256(key).digest()

    # Pad key to block size
    key = key + b'\x00' * (block_size - len(key))

    # XOR key with outer and inner pads
    o_key_pad = bytes(b ^ 0x5c for b in key)
    i_key_pad = bytes(b ^ 0x36 for b in key)

    # Inner hash: H(i_key_pad || message)
    inner = uhashlib.sha256()
    inner.update(i_key_pad)
    inner.update(message)

    # Outer hash: H(o_key_pad || inner_hash)
    outer = uhashlib.sha256()
    outer.update(o_key_pad)
    outer.update(inner.digest())

    return outer.digest()


def sign_path(api_key: str, path: str, expires: int) -> str:
    """
    Generate an HMAC-SHA256 signature for a URL path with expiry.

    Signs the message "{path}|{expires}" using the API key.

    Args:
        api_key: The shared secret (API key string)
        path: URL path to sign (e.g., "/api/games")
        expires: Unix timestamp when the signature expires

    Returns:
        Hex-encoded signature string
    """
    message = f"{path}|{expires}".encode()
    digest = hmac_sha256(api_key.encode(), message)
    return ''.join('{:02x}'.format(b) for b in digest)
