"""
Over-the-air app updates via the ROMFS partition.

DEPENDENCY-FREE BY DESIGN: this module lives on littlefs (never in ROMFS)
and uses only built-in modules (socket, ssl, json, hashlib, vfs, machine,
os, time, network). It must keep working when the ROMFS partition — and
therefore the entire app, including the vendored aiohttp — is corrupt.

The app's identity is the sha256 of its ROMFS image:
  - /app_version   sha of the image currently in the partition
  - /ota_staging   downloaded candidate image (littlefs has ~2.4 MB free)
  - /ota_pending   flag file containing the staged image's expected sha

Update lifecycle (safety model: stage at runtime, APPLY AT EARLY BOOT):
  1. check_and_stage() runs from the app (daily task): manifest GET, and if
     the sha differs from /app_version, streams the image to /ota_staging,
     verifies its sha, writes /ota_pending, returns True. The caller then
     logs and machine.reset()s.
  2. apply_staged() runs from main.py at early boot — after the safe-mode
     check, BEFORE any app import. At that moment zero ROMFS bytecode is
     running or imported and Core 1 hasn't started, so rewriting the
     partition can't erase code mid-execution. Power loss mid-write is
     safe: staging + flag survive and the next boot re-applies.
  3. recover() runs from main.py's import guard when the app can't load
     (truly corrupt ROMFS): raw WiFi connect, forced re-download, apply,
     reset — looping with backoff until the device heals itself.

Synchronous on purpose: the download blocks Core 0 for ~10 s once a day;
Core 1 keeps rendering the game throughout.
"""

import hashlib
import json
import machine
import os
import socket
import ssl
import time
import vfs

VERSION_FILE = "/app_version"
STAGING_FILE = "/ota_staging"
PENDING_FILE = "/ota_pending"

_CHUNK = 4096


def _log(msg):
    # print only: scoreboard.logger lives in ROMFS and may not be importable
    # in the recovery paths where this module matters most.
    print("[OTA]", msg)


# --------------------------------------------------------------------------
# Minimal HTTPS GET (raw socket + ssl). Returns (status, header_dict) and
# leaves the socket positioned at the body start for the caller to stream.
# --------------------------------------------------------------------------

def _https_get(url, api_key):
    proto, _, host, path = url.split("/", 3)
    if proto != "https:":
        raise ValueError("https URL required")
    port = 443
    if ":" in host:
        host, port = host.split(":", 1)
        port = int(port)

    addr = socket.getaddrinfo(host, port)[0][-1]
    raw = socket.socket()
    raw.settimeout(30)
    raw.connect(addr)
    s = ssl.wrap_socket(raw, server_hostname=host)

    s.write(
        b"GET /%s HTTP/1.0\r\nHost: %s\r\nX-Api-Key: %s\r\n\r\n"
        % (path.encode(), host.encode(), api_key.encode())
    )

    # Status line + headers (HTTP/1.0: connection closes at body end)
    status = int(_readline(s).split(b" ")[1])
    headers = {}
    while True:
        line = _readline(s)
        if line in (b"", b"\r\n", b"\n"):
            break
        key, _, value = line.partition(b":")
        headers[key.strip().lower().decode()] = value.strip().decode()
    return s, status, headers


def _readline(s):
    line = b""
    while not line.endswith(b"\n"):
        c = s.read(1)
        if not c:
            break
        line += c
    return line


def _read_exact(s, n, sink):
    """Stream n bytes from socket s into sink(buf) in _CHUNK pieces."""
    buf = bytearray(_CHUNK)
    mv = memoryview(buf)
    remaining = n
    while remaining > 0:
        want = min(_CHUNK, remaining)
        got = s.readinto(mv[:want])
        if not got:
            raise OSError("connection closed mid-body")
        sink(mv[:got])
        remaining -= got


# --------------------------------------------------------------------------
# Local state helpers
# --------------------------------------------------------------------------

def current_version():
    try:
        with open(VERSION_FILE) as f:
            return f.read().strip()
    except OSError:
        return None


def _write_file(path, text):
    with open(path, "w") as f:
        f.write(text)


def _remove(path):
    try:
        os.remove(path)
    except OSError:
        pass


def _file_sha256(path):
    h = hashlib.sha256()
    buf = bytearray(_CHUNK)
    mv = memoryview(buf)
    with open(path, "rb") as f:
        while True:
            got = f.readinto(buf)
            if not got:
                break
            h.update(mv[:got])
    return "".join("%02x" % b for b in h.digest())


# --------------------------------------------------------------------------
# Stage (runtime) and apply (early boot)
# --------------------------------------------------------------------------

def fetch_manifest(api_url, api_key):
    """Return {'sha256': ..., 'size': ...} or raise."""
    s, status, headers = _https_get(api_url.rstrip("/") + "/app/manifest", api_key)
    try:
        if status != 200:
            raise OSError("manifest HTTP %d" % status)
        length = int(headers.get("content-length", "0"))
        body = b""
        while True:
            # TLS reads may return partial data; drain until length (or EOF
            # when the server didn't send a content-length).
            want = (length - len(body)) if length else 256
            if length and want <= 0:
                break
            part = s.read(want)
            if not part:
                break
            body += part
        return json.loads(body)
    finally:
        s.close()


def check_and_stage(api_url, api_key, tick=None):
    """Download and stage a new app image if the backend has a different one.

    Returns True when an update is staged (caller should reset), False when
    already current. Raises on network/verification errors — callers treat
    any exception as "try again next cycle". `tick`, if given, is called per
    downloaded chunk (the caller feeds the hardware watchdog through it —
    this download deliberately blocks the asyncio loop).
    """
    manifest = fetch_manifest(api_url, api_key)
    new_sha = manifest["sha256"]
    if new_sha == current_version():
        return False

    size = manifest["size"]
    _log("update available: %s (%d bytes); staging..." % (new_sha[:12], size))

    s, status, headers = _https_get(api_url.rstrip("/") + "/app/image", api_key)
    try:
        if status != 200:
            raise OSError("image HTTP %d" % status)
        length = int(headers.get("content-length", "0"))
        if length != size:
            raise OSError("image length %d != manifest size %d" % (length, size))
        h = hashlib.sha256()
        with open(STAGING_FILE, "wb") as out:
            def sink(chunk):
                h.update(chunk)
                out.write(chunk)
                if tick is not None:
                    tick()
            _read_exact(s, size, sink)
    finally:
        s.close()

    got_sha = "".join("%02x" % b for b in h.digest())
    if got_sha != new_sha:
        _remove(STAGING_FILE)
        raise OSError("staged sha %s != manifest %s" % (got_sha[:12], new_sha[:12]))

    _write_file(PENDING_FILE, new_sha)
    _log("staged and verified; apply on next boot")
    return True


def apply_staged():
    """Apply a staged image to the ROMFS partition, if one is pending.

    ONLY call from early boot (before any app import, before Core 1 starts)
    or from recover(): rewriting the partition while ROMFS bytecode is
    running would erase code mid-execution. Idempotent: re-verifies the
    staged sha first, so a power loss mid-write just re-applies next boot.
    Returns True if an image was applied.
    """
    try:
        with open(PENDING_FILE) as f:
            expected_sha = f.read().strip()
    except OSError:
        return False

    _log("pending update %s; verifying staging..." % expected_sha[:12])
    try:
        if _file_sha256(STAGING_FILE) != expected_sha:
            raise OSError("staging sha mismatch")
        size = os.stat(STAGING_FILE)[6]
    except OSError as e:
        # Corrupt/missing staging: discard and boot the current app.
        _log("staged image invalid (%s); discarding" % e)
        _remove(STAGING_FILE)
        _remove(PENDING_FILE)
        return False

    dev = vfs.rom_ioctl(2, 0)  # ROMFS partition as a block device
    block_size = dev.ioctl(5, 0)
    block_count = dev.ioctl(4, 0)
    if size > block_size * block_count:
        _log("image (%d) exceeds partition (%d); discarding" % (size, block_size * block_count))
        _remove(STAGING_FILE)
        _remove(PENDING_FILE)
        return False

    _log("writing %d bytes to ROMFS..." % size)
    blocks_needed = (size + block_size - 1) // block_size
    for block in range(blocks_needed):
        dev.ioctl(6, block)  # erase

    buf = bytearray(block_size)
    mv = memoryview(buf)
    with open(STAGING_FILE, "rb") as f:
        for block in range(blocks_needed):
            got = f.readinto(buf)
            if got < block_size:
                # Pad the final partial block; flash was just erased (0xFF)
                for i in range(got, block_size):
                    buf[i] = 0xFF
            dev.writeblocks(block, mv)

    _write_file(VERSION_FILE, expected_sha)
    _remove(STAGING_FILE)
    _remove(PENDING_FILE)
    _log("applied %s" % expected_sha[:12])
    return True


# --------------------------------------------------------------------------
# Recovery (called from main.py's import guard when the app can't load)
# --------------------------------------------------------------------------

def recover():
    """Self-heal a device whose app won't import: connect WiFi with raw
    network APIs, force a re-download of the current image, apply, reset.
    Loops with backoff; never returns except by machine.reset()."""
    import network

    try:
        with open("/config.json") as f:
            cfg = json.load(f)
        ssid = cfg["network"]["ssid"]
        password = cfg["network"]["password"]
        api_url = cfg["api"]["url"]
        api_key = cfg["api"]["key"]
    except (OSError, ValueError, KeyError) as e:
        _log("recovery impossible: config unreadable (%s); REPL is free" % e)
        return

    if not ssid:
        _log("recovery impossible: no WiFi configured; REPL is free")
        return

    delay = 10
    while True:
        try:
            wlan = network.WLAN(network.STA_IF)
            wlan.active(True)
            if not wlan.isconnected():
                _log("recovery: connecting to %s..." % ssid)
                wlan.connect(ssid, password)
                for _ in range(30):
                    if wlan.isconnected():
                        break
                    time.sleep(1)
            if not wlan.isconnected():
                raise OSError("wifi connect timeout")

            _log("recovery: downloading current app...")
            _remove(VERSION_FILE)  # force check_and_stage to re-download
            if check_and_stage(api_url, api_key):
                apply_staged()
            _log("recovery: applied; resetting")
            time.sleep(1)
            machine.reset()
        except Exception as e:
            _log("recovery attempt failed: %s; retrying in %ds" % (e, delay))
            time.sleep(delay)
            delay = min(delay * 2, 300)
