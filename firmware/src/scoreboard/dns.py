"""
Captive Portal DNS Server.

Minimal DNS responder that hijacks all DNS queries and returns the Pico's IP.
This triggers captive portal detection on most devices when they connect to AP mode.
"""

import socket
import uasyncio as asyncio
import scoreboard.logger as logger


async def run_dns_server(ip_address: str = '192.168.4.1') -> None:
    """
    Simple DNS server that responds to all queries with the given IP.
    Runs as an async task alongside the web server.

    This task must never die: captive-portal detection depends on it, so a
    malformed packet is logged and dropped rather than allowed to raise.

    Args:
        ip_address: The IP to return for all DNS queries (default: 192.168.4.1)
    """
    # Convert IP string to bytes
    ip_bytes: bytes = bytes(map(int, ip_address.split('.')))

    # Create UDP socket
    sock: socket.socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setblocking(False)
    sock.bind(('0.0.0.0', 53))

    logger.debug(f"[DNS] server started: ip={ip_address}")

    while True:
        try:
            # Non-blocking receive
            data, addr = sock.recvfrom(512)
        except OSError:
            # No data available, yield to other tasks
            await asyncio.sleep_ms(50)
            continue

        try:
            response = _build_dns_response(data, ip_bytes)
            sock.sendto(response, addr)
        except Exception as e:
            # Malformed query (or send failure): drop it, keep serving.
            logger.error(f"[DNS] dropped bad packet from {addr}: {e}")

        # Yield after every packet so a burst of queries can't starve the
        # web server on this same asyncio loop.
        await asyncio.sleep_ms(0)


def _build_dns_response(query: bytes, ip_bytes: bytes) -> bytes:
    """
    Build a DNS response that returns the given IP for any A record query.

    Args:
        query: The raw DNS query packet
        ip_bytes: The IP address as bytes (4 bytes for IPv4)

    Returns:
        The raw DNS response packet
    """
    if len(query) < 12:
        raise ValueError(f"query too short: {len(query)} bytes")

    # Transaction ID (first 2 bytes of query)
    transaction_id: bytes = query[:2]

    # Flags: standard response, no error
    flags: bytes = b'\x81\x80'

    # Questions: 1, Answers: 1, Authority: 0, Additional: 0
    counts: bytes = b'\x00\x01\x00\x01\x00\x00\x00\x00'

    # Find the question section (starts at byte 12). Walk the length-prefixed
    # name labels with bounds checks — a truncated packet must raise a clean
    # ValueError, not IndexError from a wild read.
    question_end: int = 12
    while question_end < len(query) and query[question_end] != 0:
        question_end += query[question_end] + 1
    question_end += 5  # null byte + qtype (2) + qclass (2)
    if question_end > len(query):
        raise ValueError("truncated question section")

    question: bytes = query[12:question_end]

    # Answer section
    # Name pointer to question (0xC00C = pointer to offset 12)
    answer: bytes = b'\xc0\x0c'
    # Type A (1), Class IN (1)
    answer += b'\x00\x01\x00\x01'
    # TTL (60 seconds)
    answer += b'\x00\x00\x00\x3c'
    # Data length (4 bytes for IPv4)
    answer += b'\x00\x04'
    # IP address
    answer += ip_bytes

    return transaction_id + flags + counts + question + answer
